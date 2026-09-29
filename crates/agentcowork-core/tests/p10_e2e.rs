#![cfg(unix)]

//! P10.1 — E2E integration suites (cross-cutting validation of P0–P9).
//!
//! Each test composes the REAL public API of the shipped crates (no mocks
//! beyond the injectable transport seams the codebase already defines). The
//! browser-pipeline, office byte-stability, ACP harness-driving, and MCP
//! external-client rows live in their owning crates' tests (they need that
//! crate's fixture binaries / `pub(crate)` fixtures):
//!
//! - browser pipeline  → `agentcowork-browser/tests/p10_pipeline.rs`
//! - office byte-stability → `agentcowork-office` docx unit test
//! - ACP harness-driving → `agentcowork-acp/tests/p10_harness_drive.rs`
//! - MCP external client → `agentcowork-mcp/tests/p10_external_client.rs`

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agentcowork_blueprint::crystallize::{
    StepClass, WorkflowDetector, WorkflowStep, compile_to_script, decrystallize_check,
};
use agentcowork_blueprint::spec::TaskSpec;
use agentcowork_blueprint::subagent::{
    DelegationPolicy, SubAgentError, SubAgentLimits, SubAgentResult, SubAgentSpec, parent_view,
};
use agentcowork_blueprint::{ScriptLanguage, TaskStatus};
use agentcowork_core::work_gateway::{DomainEvent, WorkEvent, WorkGateway};
// P71.3f — the delegation gate judges the canonical readiness state.
use agentcowork_core::chat::{ChatRelay, ChatWireEvent};
use agentcowork_core::connector_hub::{ConnectorHub, Engine};
use agentcowork_core::connectors::gmail::GmailConnector;
use agentcowork_core::connectors::{
    HttpTransport, TokenSource, TransportError, TransportErrorKind, VaultTokenRef,
};
use agentcowork_core::guard_service::GuardService;
use agentcowork_core::memory_service::MemoryService;
use agentcowork_core::messaging::{InboundMessage, MessageDispatcher, StubAdapter};
use agentcowork_core::providers::{ProviderConfig, ProviderKey, ProvidersFile};
use agentcowork_core::scheduler_service::{SchedulePolicy, SchedulerService, TriggerSpec};
use agentcowork_core::sidecar_link::SidecarLink;
use agentcowork_core::tools::ToolService;
use agentcowork_guard::granter::{CapabilityGranter, GrantRequest, HostGrant, TrustFlags};
use agentcowork_types::AgentReadiness;
use agentcowork_vault::Vault;

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("agentcowork-p10-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn pair() -> (UnixStream, UnixStream) {
    UnixStream::pair().expect("socketpair")
}

fn link_from(a: UnixStream) -> SidecarLink<UnixStream, UnixStream> {
    let reader = a.try_clone().expect("clone");
    SidecarLink::new(a, reader)
}

/// Spin a fake OpenAI-compatible endpoint (same pattern as the chat.rs unit
/// tests): returns the base URL the relay should route `nvidia` to.
#[allow(dead_code)]
fn mock_openai(respond: impl Fn(&str) -> (u16, String) + Send + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let mut buf = [0u8; 16_384];
            let n = match s.read(&mut buf) {
                Ok(n) => n,
                Err(_) => continue,
            };
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let (code, body) = respond(&req);
            let resp = format!(
                "HTTP/1.1 {code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    format!("http://{addr}")
}

#[allow(dead_code)]
fn wait_events(events: &Arc<Mutex<Vec<ChatWireEvent>>>, min: usize, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if events.lock().unwrap_or_else(|e| e.into_inner()).len() >= min {
            return true;
        }
        if start.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------
// P10.1.1 — full user journey: install → first boot → add BYOK key →
// chat → tool call → response
// ---------------------------------------------------------------------------

#[test]
fn journey_install_byok_chat_tool_call() {
    let dir = temp_dir("journey");

    // "install → first boot": the data dir is created and an empty
    // providers.toml is materialized by the loader.
    let providers_path = ProvidersFile::path(&dir);
    let mut pf = ProvidersFile::load_from(&providers_path).unwrap();
    assert!(pf.providers.is_empty());

    // "add BYOK key": write a provider + key pool, persist, reload.
    pf.providers.push(ProviderConfig {
        name: "nvidia".into(),
        base_url: None,
        keys: vec![ProviderKey {
            id: "my-byok".into(),
            value: "sk-test".into(),
        }],
    });
    pf.save(&providers_path).unwrap();
    let reloaded = ProvidersFile::load_from(&providers_path).unwrap();
    let pool = reloaded.pool("nvidia").expect("pool after reload");
    assert_eq!(pool.len(), 1);
    let key = pool.select().unwrap();
    assert_eq!(key.id, "my-byok");

    // "chat": `P71.2c` deleted the built-in engine (ADR-0005 §2), so there is no
    // AgentCowork turn to dispatch. What a journey now proves here is the
    // **engine-optional** guarantee (`ARCH/AGENT.md` §2, `P71.6b`): the shell
    // holds its own credentials and governance without a built-in binding being
    // present, and the J11 budget pre-flight still refuses a session over limit
    // before anything is dispatched — the check moved from the deleted
    // `start_stream` to `preflight_session_budget`, which is what the live ACP
    // turn path calls. The chat leg itself runs on the bound agent's channel and
    // is gated live by `P70.E5`.
    let vault = Arc::new(Mutex::new(Vault::open_in_memory("test-key").unwrap()));
    let (a, _b) = pair();
    let relay = ChatRelay::new(link_from(a), Arc::clone(&vault), |_| {});
    assert!(
        relay.preflight_session_budget("s1").is_ok(),
        "a session with no spend passes the budget pre-flight"
    );
    {
        let v = vault.lock().unwrap();
        v.record_usage(&agentcowork_vault::UsageRow {
            session: "s1".into(),
            provider: "nvidia".into(),
            model: "m".into(),
            key_id: "k".into(),
            usage: agentcowork_vault::Usage::default(),
            cost: agentcowork_vault::DEFAULT_SESSION_BUDGET_USD,
            tool: None,
            task_id: String::new(),
            run_id: String::new(),
            work_id: String::new(),
        })
        .unwrap();
    }
    assert!(
        relay.preflight_session_budget("s1").is_err(),
        "an over-budget session is refused before dispatch"
    );

    // "tool call": the guard-gated executor writes a file (ask → approve →
    // commit → audit row).
    let guard = Arc::new(Mutex::new(GuardService::new()));
    let mut tools = ToolService::new(Arc::clone(&guard), dir.join("workspace"));
    let args = serde_json::json!({ "path": "notes.txt", "content": "journey complete" });
    let pre = tools
        .handle(
            "tool/exec",
            &serde_json::json!({ "toolId": "file_ops.write", "sessionId": "s1", "agentId": "a1", "args": args }),
        )
        .unwrap();
    assert_eq!(pre["action"], "ask", "default write policy asks");
    let tid = pre["ticketId"].as_str().unwrap().to_string();
    assert!(guard.lock().unwrap().approve(&tid));
    let commit = tools
        .handle(
            "tool/commit",
            &serde_json::json!({
                "toolId": "file_ops.write",
                "ticketId": tid,
                "argsHash": pre["argsHash"],
                "args": args,
            }),
        )
        .unwrap();
    assert_eq!(commit["ok"], true, "{commit}");
    assert_eq!(commit["auditSeq"], 1, "tool call lands an audit row");
    let text = std::fs::read_to_string(dir.join("workspace/notes.txt")).unwrap();
    assert_eq!(text, "journey complete");

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// P10.1.2 — multi-turn session with memory persistence (close → reopen →
// recall works)
// ---------------------------------------------------------------------------

#[test]
fn memory_persists_across_restart() {
    let dir = temp_dir("memory");
    let db = dir.join("memory.json");

    // "session one": write facts, close (drop) the service, persist.
    {
        let mut mem = MemoryService::new();
        let written = mem.write(
            "s1",
            &[
                "the project is called agentcowork".to_string(),
                "the vault is sqlcipher-encrypted".to_string(),
            ],
        );
        assert_eq!(written, 2);
        mem.save_to(&db).unwrap();
    } // dropped — "app closed"

    // "reopen": a fresh service loads the same file; recall works. `read`
    // returns the matched fact ids; the persisted content is intact.
    let reopened = MemoryService::load_from(&db).unwrap();
    let hits = reopened.read("agentcowork", 5);
    assert!(
        !hits.is_empty(),
        "recall returns matching fact ids: {hits:?}"
    );
    let facts = reopened.core_facts();
    assert!(
        facts.iter().any(|f| f.contains("called agentcowork")),
        "persisted content missing after restart: {facts:?}"
    );
    // The second fact is also still there.
    assert!(facts.iter().any(|f| f.contains("sqlcipher-encrypted")));

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// P10.1.5 — sub-agent workflow (planner → 2 sub-agents → merge → final output)
// ---------------------------------------------------------------------------

#[test]
fn subagent_planner_two_agents_merge_results() {
    // P71.3a — a planner delegates two research agents. Delegation is child
    // Work/Runs in the one Work Gateway (I8), the admission is judged from that
    // same graph, and the planner sees the summary-only view of each child.
    let policy = DelegationPolicy::new(SubAgentLimits::default());
    let mut gw = WorkGateway::new();
    gw.create_work("root-work", None, Some("s-1".into()), "session work");

    // The planner is the root Work's child → depth 1, nothing active yet.
    let gauge = gw.delegation_gauge("root-work").unwrap();
    assert_eq!(gauge.child_depth, 1);
    policy
        .admit("planner", gauge, "planner-agent", AgentReadiness::Ready)
        .unwrap();
    let planner_spec = SubAgentSpec::new(
        TaskSpec::new("planner", "coordinate research"),
        "nvidia",
        "/tmp/work",
    );
    // Per-agent model selection belongs to the spec, not to any runtime.
    assert_eq!(planner_spec.model, "nvidia");
    let planner = gw
        .delegate_child_work(
            "root-work",
            "planner",
            "coordinate research",
            "planner-agent",
            None,
        )
        .unwrap();

    // The planner's own children land on depth 2 (the cap), two of them, and
    // both are admitted against the graph's gauge.
    let gauge = gw.delegation_gauge(&planner.work_id).unwrap();
    assert_eq!(gauge.child_depth, 2);
    policy
        .admit("child-a", gauge, "child-a-agent", AgentReadiness::Ready)
        .unwrap();
    let child_a = gw
        .delegate_child_work(
            &planner.work_id,
            "child-a",
            "research the storage engine",
            "child-a-agent",
            None,
        )
        .unwrap();
    let gauge = gw.delegation_gauge(&planner.work_id).unwrap();
    assert_eq!(gauge.active, 1);
    policy
        .admit("child-b", gauge, "child-b-agent", AgentReadiness::Ready)
        .unwrap();
    let child_b = gw
        .delegate_child_work(
            &planner.work_id,
            "child-b",
            "research the guard engine",
            "child-b-agent",
            None,
        )
        .unwrap();
    let gauge = gw.delegation_gauge(&planner.work_id).unwrap();
    assert_eq!((gauge.active, gauge.total, gauge.child_depth), (2, 2, 2));
    // A grandchild of a depth-2 child would be depth 3 → recursion, refused.
    assert!(matches!(
        policy.admit(
            "grandchild",
            gw.delegation_gauge(&child_a.work_id).unwrap(),
            "grandchild-agent",
            AgentReadiness::Ready,
        ),
        Err(SubAgentError::DepthExceeded {
            depth: 3,
            max_depth: 2,
            ..
        })
    ));

    // Both children complete: terminal Run events on their own timelines.
    gw.finish_child_work(
        &planner.work_id,
        "child-a",
        agentcowork_types::WorkState::Completed,
        None,
    )
    .unwrap();
    gw.finish_child_work(
        &planner.work_id,
        "child-b",
        agentcowork_types::WorkState::Completed,
        None,
    )
    .unwrap();

    // The planner (parent) sees mergeable summaries — never raw child context.
    let merged: Vec<String> = [
        (
            &child_a,
            "research the storage engine",
            "storage uses FTS5 + trigram",
        ),
        (
            &child_b,
            "research the guard engine",
            "guard uses tickets + nonce",
        ),
    ]
    .into_iter()
    .map(|(child, goal, summary)| {
        let result = SubAgentResult {
            task_id: child.work_id.clone(),
            summary: summary.to_string(),
            status: TaskStatus::Done,
            artifacts: vec![format!("{goal}.md")],
        };
        let view = parent_view(&result);
        // Four keys: the transcript is not among them by construction.
        assert_eq!(view.as_object().unwrap().len(), 4);
        view["summary"].as_str().unwrap().to_string()
    })
    .collect();
    assert_eq!(merged.len(), 2);
    assert!(merged[0].contains("FTS5"));
    assert!(merged[1].contains("tickets"));

    // Terminal children free concurrency but stay counted in the total — both
    // of which the graph reports without any registry remembering them.
    let gauge = gw.delegation_gauge(&planner.work_id).unwrap();
    assert_eq!((gauge.active, gauge.total), (0, 2));
    // The graph also knows the whole tree: one planner, two children under it.
    assert_eq!(gw.children_of("root-work").len(), 1);
    assert_eq!(gw.children_of(&planner.work_id).len(), 2);
    // Deterministic child ids: the whole tree is addressable from (parent,
    // task) alone, which is what makes the graph a usable record.
    assert_eq!(planner.work_id, "root-work/subagent/planner");
    assert_eq!(
        child_a.work_id,
        "root-work/subagent/planner/subagent/child-a"
    );
    assert_eq!(
        child_b.run_id,
        "root-work/subagent/planner/subagent/child-b/run"
    );
}

// ---------------------------------------------------------------------------
// P10.1.6 — crystallization (run workflow 3× → 4th run = 0 tokens)
// ---------------------------------------------------------------------------

#[test]
fn crystallization_fourth_run_is_zero_token() {
    // The workflow: 3 identical successful runs of (transform → notify).
    let steps = || {
        vec![
            WorkflowStep {
                tool: "file_ops.write".into(),
                args: r#"{"path":"r.txt","content":"ok"}"#.into(),
                class: StepClass::Transform,
            },
            WorkflowStep {
                tool: "notify".into(),
                args: r#"{"to":"me"}"#.into(),
                class: StepClass::Notify,
            },
        ]
    };

    let mut detector = WorkflowDetector::new(3);
    for _ in 0..3 {
        detector.record_success(steps());
    }
    // After 3 identical successes the workflow is a crystallization candidate.
    let candidates = detector.candidates();
    assert_eq!(
        candidates.len(),
        1,
        "third identical success promotes the workflow"
    );
    assert!(
        candidates[0]
            .steps
            .iter()
            .all(|s| s.class.is_crystallizable()),
        "no cognitive step → crystallizable"
    );
    assert_eq!(candidates[0].successes, 3);

    // Compile to a deterministic script — the "0-token run" (no LLM call).
    let skill = compile_to_script("weekly-report", candidates[0], ScriptLanguage::Ts);
    assert!(skill.source.contains("0-token deterministic run"));
    assert!(skill.source.contains("file_ops_write"));
    // The compiled run produces the recorded expected output → no drift, so
    // the 4th run executes the script instead of calling the model.
    assert_eq!(
        decrystallize_check(&skill, &skill.expected_output),
        agentcowork_blueprint::crystallize::Drift::Match
    );
}

// ---------------------------------------------------------------------------
// P10.1.7 — connector hub (browser-session connector → Gmail read → respond)
// ---------------------------------------------------------------------------

/// Minimal mock HTTP transport (the injectable seam the codebase defines).
struct MockTransport {
    responses: std::cell::RefCell<Vec<Result<Vec<u8>, TransportError>>>,
}

impl MockTransport {
    fn new(responses: Vec<Result<Vec<u8>, TransportError>>) -> Self {
        Self {
            responses: std::cell::RefCell::new(responses),
        }
    }
}

impl HttpTransport for MockTransport {
    fn post_json(
        &self,
        _url: &str,
        _headers: &[(&str, &str)],
        _body: &[u8],
    ) -> Result<Vec<u8>, TransportError> {
        self.responses
            .borrow_mut()
            .pop()
            .unwrap_or(Err(TransportError {
                kind: TransportErrorKind::Other,
                message: "no more mock responses".into(),
            }))
    }
    fn get(&self, _url: &str, _headers: &[(&str, &str)]) -> Result<Vec<u8>, TransportError> {
        self.responses
            .borrow_mut()
            .pop()
            .unwrap_or(Err(TransportError {
                kind: TransportErrorKind::Other,
                message: "no more mock responses".into(),
            }))
    }
}

/// FIX-01: a use-style token *source* (the connector holds a vault reference,
/// never the token bytes).
struct MockRefresher;
impl TokenSource for MockRefresher {
    fn with_token<R>(
        &self,
        _ref_: &VaultTokenRef,
        f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
    ) -> Result<R, TransportError> {
        f("tok")
    }

    fn refresh<R>(
        &self,
        _ref_: &VaultTokenRef,
        f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
    ) -> Result<R, TransportError> {
        f("refreshed-token")
    }
}

fn gmail_tokens() -> VaultTokenRef {
    VaultTokenRef::new("k-gmail-e2e", "gmail")
}

#[test]
fn connector_hub_gmail_read_respond() {
    // Register the connection in the hub (browser-session engine).
    let mut hub = ConnectorHub::new();
    let id = hub
        .connect("gmail", "me@example.com", Engine::BrowserSession)
        .unwrap();
    assert!(hub.is_connected("gmail", "me@example.com"));
    assert_eq!(hub.get(&id).unwrap().engine.as_str(), "browser_session");

    // Gmail read path: search → get_message (mock responses, pop order).
    let search_resp = serde_json::json!({ "messages": [{ "id": "m1", "threadId": "t1" }], "resultSizeEstimate": 1 });
    let msg_resp = serde_json::json!({
        "id": "m1", "threadId": "t1", "snippet": "need the report",
        "labelIds": ["INBOX", "UNREAD"],
        "payload": {
            "mimeType": "text/plain",
            "headers": [
                { "name": "Subject", "value": "Re: status" },
                { "name": "From", "value": "boss@example.com" },
                { "name": "To", "value": "me@example.com" },
                { "name": "Date", "value": "Mon, 01 Jan 2026 00:00:00 +0000" }
            ],
            "body": { "data": "bmVlZCB0aGUgcmVwb3J0" }
        }
    });
    let transport = MockTransport::new(vec![
        Ok(serde_json::to_vec(&msg_resp).unwrap()),
        Ok(serde_json::to_vec(&search_resp).unwrap()),
    ]);
    let mut gmail = GmailConnector::new(transport, MockRefresher, gmail_tokens(), "me".into());
    let found = gmail.search("from:boss", 5, None).unwrap();
    assert_eq!(found.messages.len(), 1);
    assert_eq!(found.messages[0].subject, "Re: status");
    assert_eq!(
        found.messages[0].body_plain.as_deref(),
        Some("need the report")
    );

    // Respond: send a reply through the same connector.
    let send_resp = serde_json::json!({ "id": "sent-1", "threadId": "t1" });
    let send_transport = MockTransport::new(vec![Ok(serde_json::to_vec(&send_resp).unwrap())]);
    let mut gmail2 =
        GmailConnector::new(send_transport, MockRefresher, gmail_tokens(), "me".into());
    let sent = gmail2
        .send_message("boss@example.com", "Re: status", "report attached")
        .unwrap();
    assert_eq!(sent.message_id, "sent-1");
}

// ---------------------------------------------------------------------------
// P10.1.9 — scheduled task fires headless from the tray daemon
// ---------------------------------------------------------------------------

#[test]
fn scheduled_task_fires_headless() {
    let mut sched = SchedulerService::new();
    // A cron job that is due "now" (every minute), plus an interval job.
    sched.upsert(
        "job-cron",
        "nightly digest",
        "s-headless",
        TriggerSpec::Cron {
            expr: "* * * * *".into(),
        },
        vec![],
        Some(SchedulePolicy::default()),
        1_700_000_000,
    );
    sched.upsert(
        "job-int",
        "heartbeat",
        "s-headless",
        TriggerSpec::Interval { secs: 60 },
        vec![],
        Some(SchedulePolicy::default()),
        1_700_000_000,
    );

    // A headless tick admits durable occurrences. The host then persists the
    // matching Work/Run pair before returning a receipt that advances each
    // trigger; a trigger-only mark_fired call is deliberately forbidden.
    let due = sched.admit_due(1_700_000_060).unwrap();
    assert_eq!(due.len(), 2);
    let admitted_ids = due
        .iter()
        .map(|occurrence| {
            sched
                .job_id_for_automation(&occurrence.automation_id)
                .unwrap()
        })
        .collect::<std::collections::HashSet<_>>();
    assert!(admitted_ids.contains("job-cron"));
    assert!(admitted_ids.contains("job-int"));

    let journal_dir = temp_dir("headless-scheduled-work");
    let mut gateway = WorkGateway::open(journal_dir.join("work.jsonl")).unwrap();
    for occurrence in &due {
        let work_id = sched.expected_work_id(occurrence);
        let run_id = sched.expected_run_id(occurrence);
        let session_id = format!("automation-session:{}", occurrence.trigger_occurrence_id);
        gateway
            .create_work_in_session(
                work_id.clone(),
                None,
                Some(session_id),
                agentcowork_types::SessionKind::Automation,
                occurrence.revision.automation().name,
            )
            .unwrap();
        gateway
            .append(
                &work_id,
                WorkEvent::Domain(DomainEvent::WorkUpdated {
                    patch: serde_json::json!({
                        "automationId": occurrence.automation_id,
                        "revisionId": occurrence.revision_id,
                        "automationGeneration": occurrence.revision.generation(),
                        "triggerOccurrenceId": occurrence.trigger_occurrence_id,
                        "payloadDigest": occurrence.payload_digest,
                        "dedupDigest": occurrence.dedup_digest,
                    }),
                }),
                None,
            )
            .unwrap();
        gateway.bind_execution(&work_id, &run_id).unwrap();
        gateway
            .record_execution_transition(&work_id, &run_id, agentcowork_types::WorkState::Ready)
            .unwrap();
        let receipt = sched
            .receipt_for_occurrence(&occurrence.trigger_occurrence_id)
            .unwrap();
        sched
            .mark_occurrence_fired_with_receipt(
                &occurrence.trigger_occurrence_id,
                1_700_000_060,
                &receipt,
            )
            .unwrap();
    }

    let cron = sched.get("job-cron").unwrap();
    assert_eq!(cron.last_fired_at, Some(1_700_000_060));
    assert_eq!(cron.recent_fires, vec![1_700_000_060]);
    assert!(!sched.due(1_700_000_060).contains(&"job-cron".to_string()));
    assert!(sched.due(1_700_000_120).contains(&"job-cron".to_string()));
    let _ = std::fs::remove_dir_all(journal_dir);
}

// ---------------------------------------------------------------------------
// P10.1.10 — messaging bridge stub (message in → agent loop → reply out)
// ---------------------------------------------------------------------------

#[test]
fn messaging_bridge_stub_roundtrip() {
    let mut dispatcher = MessageDispatcher::new();
    let mut adapter = StubAdapter::new("telegram");
    adapter.inbox.push(InboundMessage {
        channel: "telegram".into(),
        from: "user-42".into(),
        text: "what is the weather?".into(),
        message_id: "msg-1".into(),
        conversation_id: Some("conv-1".into()),
    });
    dispatcher.register(Box::new(adapter));

    // The agent loop: a handler that turns a message into a reply.
    let replies = dispatcher.dispatch(|msg: &InboundMessage| {
        if msg.text.contains("weather") {
            "sunny, 24°C".to_string()
        } else {
            "I don't know yet".to_string()
        }
    });
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].to, "user-42");
    assert_eq!(replies[0].text, "sunny, 24°C");
    assert_eq!(replies[0].conversation_id, "conv-1");
    // The conversation is remembered for memory reuse across turns.
    let remembered = dispatcher.remembered("conv-1").unwrap();
    assert!(remembered.iter().any(|m| m.contains("weather")));
}

// ---------------------------------------------------------------------------
// P10.1.11 — extension loads lazily → executes a tool → respects the
// capability boundary
// ---------------------------------------------------------------------------

#[test]
fn lazy_extension_loads_and_respects_capability_boundary() {
    let dir = temp_dir("plugin");
    let mut registry = agentcowork_blueprint::plugin::PluginRegistry::new(dir.clone());

    // A plugin manifest (TOML) that asks for a bounded capability set.
    let manifest = r#"abi_version = 1
name = "csv-tools"
version = "1.0.0"
description = "csv utilities"
author = "test"

[trust]
sandboxed = true

[capabilities]
allow = ["fs.read:/tmp/**"]
deny = ["fs.read:/etc/**"]

[agents]
bind = ["data-agent"]
"#;
    std::fs::create_dir_all(dir.join("csv-tools")).unwrap();
    std::fs::write(dir.join("csv-tools/manifest.toml"), manifest).unwrap();

    // Lazy load: `scan` registers the plugin (Registered) but never loads
    // it; only an explicit first use (`activate`) loads it (Activated).
    assert!(registry.names().is_empty());
    let scanned = registry.scan().unwrap();
    assert!(scanned.contains(&"csv-tools".to_string()));
    assert_eq!(registry.names(), vec!["csv-tools".to_string()]);
    let registered = registry.get("csv-tools").unwrap();
    assert_eq!(
        registered.state,
        agentcowork_blueprint::plugin::PluginState::Registered
    );
    let entry = registry.activate("csv-tools").unwrap();
    assert_eq!(
        entry.state,
        agentcowork_blueprint::plugin::PluginState::Activated
    );

    // The host grants only a narrow set; the granter refines to the manifest's
    // allow ∩ host ∩ (allow − deny).
    let host = HostGrant {
        trusted_agents: vec!["data-agent".into()],
        capabilities: vec![
            "fs.read:/tmp/**".into(),
            "fs.read:/etc/**".into(),
            "fs.write:/tmp/**".into(),
            "network:https".into(),
            "shell".into(),
        ],
    };
    let granter = CapabilityGranter::new(host);
    let granted = granter.grant(&entry.manifest.grant_request()).unwrap();
    // The plugin may read /tmp but the explicit deny on /etc wins.
    assert!(CapabilityGranter::granted_has(
        &granted,
        "fs.read:/tmp/x.csv"
    ));
    assert!(
        !CapabilityGranter::granted_has(&granted, "fs.read:/etc/shadow"),
        "explicit deny must win"
    );
    assert!(
        !CapabilityGranter::granted_has(&granted, "shell"),
        "host capability not requested by manifest is never granted"
    );

    // Over-capability request → denied (the P10.2 gate, cross-checked here).
    let greedy = GrantRequest {
        name: "greedy".into(),
        agent_bindings: vec!["data-agent".into()],
        trust: TrustFlags {
            network: true,
            shell: true,
            files_write: true,
            approval_required: false,
            sandboxed: false,
        },
        capabilities_allow: vec!["fs.write:/".into(), "shell".into(), "network:https".into()],
        capabilities_deny: vec![],
    };
    let denied = granter.grant(&greedy);
    assert!(
        denied.is_err(),
        "capabilities outside the host grant must be refused"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
