//! P1.4 — chat streaming relay: "sidecar proposes (engine), Rust disposes
//! (broker + budget)".
//!
//! One relay owns the [`SidecarLink`] for the app's lifetime:
//!
//! 1. [`ChatRelay::start_stream`] — J11 **budget pre-flight** (refuses a
//!    session at/over its $ limit with the "stopped: $X limit" surface BEFORE
//!    any sidecar dispatch), then forwards `chat/stream` to the coordinator,
//!    where the reused ConversationEngine runs.
//! 2. The consumer loop (spawned once) handles the coordinator's
//!    `provider/stream` requests — the **broker runs HERE** (keys never leave
//!    Rust): `agentcowork-vault::Broker::chat_completion_stream`, chunks pushed
//!    back as `chat/provider_chunk` notifications the engine consumes.
//! 3. `chat/*` notifications from the coordinator are relayed to the UI
//!    (`on_event` → Tauri `chat-event` emit).
//! 4. When a turn's `chat/done` lands, the relay re-checks the ledger: a
//!    session that just crossed its $ limit gets a `BudgetExceeded` event
//!    ("stopped: $X limit") — the J11 kill surfaced at the turn boundary.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use agentcowork_guard::CapabilityBroker;
use agentcowork_vault::{DEFAULT_SESSION_BUDGET_USD, Vault};
use serde_json::Value;

use crate::eval_service::EvalService;
use crate::execution::ExecutionKernel;
use crate::guard_service::GuardService;
use crate::memory_service::MemoryService;
use crate::plan_service::PlanService;
use crate::scheduler_service::SchedulerService;
use crate::sidecar_link::{Inbound, SidecarLink};
use crate::tools::ToolService;

fn load_persistent_memory() -> MemoryService {
    let path = crate::default_data_dir().join("memory.json");
    match MemoryService::load_from(&path) {
        Ok(memory) => memory,
        Err(_) => MemoryService::new(),
    }
}

fn persist_memory(memory: &MemoryService) {
    let path = crate::default_data_dir().join("memory.json");
    let _ = std::fs::create_dir_all(crate::default_data_dir());
    let _ = memory.save_to(&path);
}

/// UI event sink (pre-existing; alias keeps clippy's type_complexity quiet).
type EventSink = Box<dyn Fn(ChatWireEvent) + Send>;

/// Identity carried by the coordinator's normalized event envelope. The
/// stream/session pair is kept as the stable routing key; these additional
/// fields preserve durable Work/execution correlation through the Rust/Tauri
/// hop instead of silently dropping it at the enum conversion boundary.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatEventMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    work_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<u64>,
}

fn event_metadata(params: &serde_json::Value) -> ChatEventMetadata {
    ChatEventMetadata {
        work_id: params
            .get("workId")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        execution_id: params
            .get("executionId")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        run_id: params
            .get("runId")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        event_id: params
            .get("eventId")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        sequence: params.get("sequence").and_then(|v| v.as_u64()),
        schema_version: params.get("schemaVersion").and_then(|v| v.as_u64()),
        timestamp: params.get("timestamp").and_then(|v| v.as_u64()),
    }
}

/// Wire events forwarded to the UI (Tauri emits a single `chat-event`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatWireEvent {
    Ttft {
        stream_id: String,
        session_id: String,
        latency_ms: u64,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    Batch {
        stream_id: String,
        session_id: String,
        text: String,
        token_count: u64,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    Reasoning {
        stream_id: String,
        session_id: String,
        text: String,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    Stage {
        stream_id: String,
        session_id: String,
        stage: String,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    ToolCall {
        #[serde(rename = "streamId")]
        stream_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "toolId")]
        tool_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        risk: Option<String>,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    ToolResult {
        #[serde(rename = "streamId")]
        stream_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "toolId")]
        tool_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    /// P41.4 — K1 verification receipt for the editor's Diff rail
    /// (model-reported pass/fail per plan-task check; `passed: null` =
    /// ambiguous — never claimed as executed).
    Verification {
        #[serde(rename = "streamId")]
        stream_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "checks")]
        checks: Vec<String>,
        #[serde(rename = "report")]
        report: String,
        #[serde(rename = "passed")]
        passed: Option<bool>,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    Done {
        stream_id: String,
        session_id: String,
        turn_id: String,
        full_text: String,
        total_tokens: u64,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    Error {
        stream_id: String,
        session_id: String,
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none", rename = "toolId")]
        tool_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        retryable: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<serde_json::Value>,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    Cancelled {
        stream_id: String,
        session_id: String,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    /// J11 kill surface: "stopped: $X limit".
    BudgetExceeded {
        stream_id: String,
        session_id: String,
        limit: f64,
        spent: f64,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    /// Stage-0 plan executor: a circuit-break MCQ card for the H2 cockpit
    /// (the coordinator emitted `chat/interrupt` when `CircuitBreaker::step`
    /// tripped). `options` are the McqOption values; the UI maps them to
    /// actionable labels and returns the choice via `plan/respond`.
    Interrupt {
        stream_id: String,
        session_id: String,
        plan_id: String,
        break_id: String,
        title: String,
        description: String,
        options: Vec<String>,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    /// Stage-0 plan executor: the plan finished (or halted). `error` is
    /// present when it halted on an interrupt/escalation.
    PlanDone {
        stream_id: String,
        session_id: String,
        plan_id: String,
        tasks_done: u32,
        error: Option<String>,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    /// P6.4 / H2 — monitoring verdict for the UI badge (notify vs silent).
    Monitor {
        #[serde(rename = "streamId")]
        stream_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "jobId")]
        job_id: String,
        changed: bool,
        notified: bool,
        stopped: bool,
        current: String,
        notifications: u32,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    /// Plan lifecycle events use the same normalized chat-event channel as
    /// ordinary turns. Keeping them here prevents plan_start/plan_step from
    /// disappearing at the Rust relay boundary.
    PlanStart {
        #[serde(rename = "streamId")]
        stream_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "planId")]
        plan_id: String,
        tasks: u32,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    PlanStep {
        #[serde(rename = "streamId")]
        stream_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "planId")]
        plan_id: String,
        #[serde(rename = "taskId")]
        task_id: String,
        status: String,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    MemoryExtracted {
        #[serde(rename = "streamId")]
        stream_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        facts: Vec<String>,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    /// P52.20 — numbered citations produced from live `search.query` hits.
    Citations {
        #[serde(rename = "streamId")]
        stream_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        citations: Vec<serde_json::Value>,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
    /// P51.10 — ordered Changes Walkthrough stops from `execution/multirun`.
    Walkthrough {
        #[serde(rename = "streamId")]
        stream_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        stops: Vec<serde_json::Value>,
        #[serde(flatten)]
        metadata: ChatEventMetadata,
    },
}

/// Relay errors. P71.2c removed `SidecarRejected` and `AgentNotReady` with the
/// built-in dispatch they belonged to: a relay call is now always served
/// in-process, and the readiness refusal is a plain refusal at the ACP turn
/// boundary (`src-tauri`'s `acp_prompt`), which is where the state is known.
#[derive(Debug, thiserror::Error)]
pub enum ChatRelayError {
    #[error("link error: {0}")]
    Link(#[from] crate::sidecar_link::LinkError),
    #[error("vault error: {0}")]
    Vault(#[from] agentcowork_vault::VaultError),
    /// The durable Work journal is the relay's startup dependency. There is no
    /// safe in-memory substitute for it.
    #[error("work gateway startup failed: {0}")]
    WorkGateway(String),
    /// Recovery/reconciliation refused a new turn. This is deliberately a
    /// startup/prompt error rather than a best-effort warning.
    #[error("work recovery refused: {0}")]
    Recovery(String),
    /// J11 pre-flight refusal — the message carries the UI surface string.
    #[error("session '{session}' stopped: ${limit:.2} limit (spent ${spent:.2})")]
    BudgetExceeded {
        session: String,
        limit: f64,
        spent: f64,
    },
}

/// The relay: owns the link + vault + UI callback + stream→session map.
pub struct ChatRelay<W, R> {
    link: SidecarLink<W, R>,
    vault: Arc<Mutex<Vault>>,
    /// stream_id → session_id (for post-turn budget checks).
    sessions: Arc<Mutex<HashMap<String, String>>>,
    /// P5.1/P5.3/P5.4/P5.9: the in-process memory dispatch (facts, planner,
    /// ghost index, usage ledger) the sidecar calls via `memory/*` methods.
    memory: Arc<Mutex<MemoryService>>,
    /// P7.5/J21: the Guard-2 pre-flight (tickets/policy/estop/profile) the
    /// coordinator drives via `guard/*` methods; shared with the Tauri cards.
    guard: Arc<Mutex<GuardService>>,
    /// P6.3 Stage-0: per-plan circuit-breaker state the coordinator steps via
    /// `plan/*` methods; trips become `chat/interrupt` → `ChatWireEvent::Interrupt`.
    plan: Arc<Mutex<PlanService>>,
    /// P6.4 (B7): the durable scheduled-task core (cron/interval/event/webhook
    /// triggers, leases, retry, battery policy, nudge sentinels). The
    /// coordinator drives it via `scheduler/*` methods.
    scheduler: Arc<Mutex<SchedulerService>>,
    /// Stage 0: guard-gated tool executor (`tool/list`/`tool/exec`/`tool/commit`).
    tools: Arc<Mutex<ToolService>>,
    /// S0.7 EV1 runtime: `eval/verify` at task completion.
    evals: Arc<Mutex<EvalService>>,
    /// H3 unified execution kernel.
    executions: Arc<Mutex<ExecutionKernel>>,
    /// P64.4/P71.3a — sub-agent spawn policy (depth/concurrency/total). The
    /// LLM execution stays in the coordinator and the durable state is the
    /// child Work in `work_gateway` (I8), so this is the judgement alone: an
    /// immutable value, not shared accounting state.
    delegation: agentcowork_blueprint::DelegationPolicy,
    /// P71.3f — the shell-mounted readiness source. The picker, the delegation
    /// gate and the turn gate all read this one state; unmounted ⇒ `Unknown`,
    /// which is never ready (fail-closed).
    readiness: crate::tools::SharedAgentReadiness,
    /// P64.8 — the distilled-skill store `skill/*` serves. A field rather than
    /// a store built per call so tests can re-seat it (the default home is the
    /// developer's real `~/.everyaios/skills/`), matching `scheduler`.
    skill_store: Arc<Mutex<agentcowork_blueprint::SkillStore>>,
    /// P54.5 — read-only view of the one PTY plane, behind `terminal/*`. The
    /// host attaches the same `PtyHost` the Shell view uses, so the agent's
    /// picture of its own shell is the user's picture of it. Absent on a host
    /// with no PTY host, where the arm answers honestly instead of inventing
    /// sessions. Carries no run capability by construction — see
    /// [`crate::terminal::TerminalPlaneObserver`].
    terminal_plane: Arc<Mutex<Option<Arc<dyn crate::terminal::TerminalPlaneObserver>>>>,
    /// P49 V1-local Work Gateway projection and event journal.
    work_gateway: Arc<Mutex<crate::work_gateway::WorkGateway>>,
    /// P49.7 capability grants; secrets remain exclusively in the vault.
    capabilities: Arc<Mutex<agentcowork_guard::LocalCapabilityBroker>>,
    /// H3 data egress engine.
    egress: Arc<Mutex<agentcowork_guard::EgressEngine>>,
    /// P43 (B7 v3.53): the detached-work task ledger (BackgroundTaskRecord
    /// lifecycle, push completion, lost-state grace, 7-day retention). Rust
    /// owns the state machine; the coordinator + Tauri shell drive it via
    /// `tasks/*` methods.
    tasks: Arc<Mutex<crate::task_ledger::TaskLedger>>,
    on_event: Arc<Mutex<EventSink>>,
    /// P11.5.11 — AG-UI live transport: forwards `agui/event` lines to the UI.
    agui: crate::agui::AguiRelay,
}

/// The skills root every part of the app shares — the same `SkillStore`
/// default the UI's skills commands use, so a distilled skill is immediately
/// visible to both surfaces.
fn skills_root() -> std::path::PathBuf {
    agentcowork_blueprint::SkillStore::default_home()
}

/// The `skill/*` methods the coordinator drives.
///
/// P64.8 — validated distillation. The coordinator calls this only after its
/// own verify gate passed, but that claim is **not** trusted here:
/// `grow_from_task` runs the real gate (tests verdict, 500-line budget,
/// manifest validation) before anything reaches disk.
fn skill_rpc(
    method: &str,
    params: &serde_json::Value,
    store: &agentcowork_blueprint::SkillStore,
) -> Result<serde_json::Value, String> {
    match method {
        "skill/grow" => {
            let task_name = params
                .get("taskName")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if task_name.trim().is_empty() {
                return Err("skill/grow requires taskName".to_string());
            }
            let solution = params
                .get("solution")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let author = params.get("author").and_then(|v| v.as_str()).unwrap_or("");
            let version = params
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("0.1.0");
            let skill =
                agentcowork_blueprint::grow_from_task(store, task_name, solution, author, version)
                    .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({
                "ok": true,
                "name": skill.manifest.name,
                "version": skill.manifest.version,
            }))
        }
        // P51.28 — model catalog omits disable-model-invocation (Zed
        // select_catalog_skills / Crush user-only skills). Slash names stay.
        "skill/warm_set" => {
            let skills = store.scan().map_err(|e| e.to_string())?;
            let rows: Vec<serde_json::Value> = skills
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "name": s.manifest.name,
                        "description": s.manifest.description,
                        "userInvocable": agentcowork_blueprint::may_user_slash_invoke(&s.manifest),
                        "disableModelInvocation": s.manifest.disable_model_invocation,
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "skills": agentcowork_blueprint::model_warm_set(&skills),
                "slash": agentcowork_blueprint::user_slash_catalog(&skills),
                "rows": rows,
            }))
        }
        "skill/compose" => {
            let stack: Vec<String> = params
                .get("stack")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let invoke = if params
                .get("userExplicit")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                agentcowork_blueprint::InvokeKind::UserExplicit
            } else {
                agentcowork_blueprint::InvokeKind::ModelAuto
            };
            let skills = store.scan().map_err(|e| e.to_string())?;
            let index = agentcowork_blueprint::SkillsIndexFile::from_skills(&skills);
            let out = agentcowork_blueprint::compose_stack_for(&index, &stack, &[], query, invoke);
            serde_json::to_value(out).map_err(|e| e.to_string())
        }
        other => Err(format!("method not found: {other}")),
    }
}

/// The `subagent/*` methods the coordinator drives.
///
/// P64.4/P71.3a — the spawn **admission** seam, with the executor deleted. I8
/// makes subagents child Work/Runs: the durable record of a delegation is the
/// child Work the gateway mints, and the admission numbers are read back out of
/// that same graph, so there is no runtime here to hold them and nothing to
/// lose on restart. The judgement itself is
/// [`agentcowork_blueprint::DelegationPolicy`] — pure over a gauge it did not
/// build.
///
/// The LLM execution stays coordinator-side, so the reply is the *admitted*
/// spec reported as running — deliberately not a fabricated `done` with an
/// invented summary. Refusals arrive as errors, never as a silent acceptance.
///
/// P69.D14 — a delegated task is registered as a **child Work** in the one Work
/// Gateway (parent link + Run + ephemeral AgentSession) from the `workId` the
/// caller names; `subagent/complete`/`subagent/fail` close that child with the
/// terminal Run event on the child's own timeline, which is also how they
/// refuse a task that was never delegated.
fn subagent_rpc(
    method: &str,
    params: &serde_json::Value,
    policy: &agentcowork_blueprint::DelegationPolicy,
    gateway: &Arc<Mutex<crate::work_gateway::WorkGateway>>,
    readiness: agentcowork_types::AgentReadiness,
) -> Result<serde_json::Value, String> {
    match method {
        "subagent/spawn" => {
            let raw = params
                .get("spec")
                .cloned()
                .ok_or("subagent/spawn requires spec")?;
            let task: agentcowork_blueprint::TaskSpec = serde_json::from_value(raw)
                .map_err(|e| format!("subagent/spawn requires a valid spec: {e}"))?;
            let goal = task.goal.clone();
            let str_list = |key: &str| -> Vec<String> {
                params
                    .get(key)
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let mut spec = agentcowork_blueprint::SubAgentSpec::new(
                task,
                params.get("model").and_then(|v| v.as_str()).unwrap_or(""),
                params
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            );
            spec.parent_id = params
                .get("parentId")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            spec.tools = str_list("tools");
            spec.blocked_tools = str_list("blockedTools");
            // P60.3 — Scout/Verifier never inherit writes even if the parent
            // listed them. Worker keeps the derived grant set.
            let role = params
                .get("role")
                .and_then(|v| v.as_str())
                .and_then(crate::cua::DelegationRole::parse);
            if let Some(role) = role {
                spec.tools = crate::cua::filter_tools_for_role(role, &spec.tools);
            }
            // P60.1 — harness and model are independent; a CLI-named
            // "*-subagent" identity is refused. ADR-0005 — there is no
            // built-in engine to default to: an absent harness stays empty and
            // `bind_runtime` refuses it ("harness required") instead of naming
            // an engine that no longer exists.
            let harness = params.get("harness").and_then(|v| v.as_str()).unwrap_or("");
            let model = params.get("model").and_then(|v| v.as_str()).unwrap_or("");
            let binding = if !model.is_empty() {
                Some(crate::bind_runtime(
                    harness,
                    model,
                    role,
                    params
                        .get("chief")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                )?)
            } else {
                crate::refuse_cli_named_subagent(harness)?;
                None
            };
            let task_id = spec.spec.id.clone();
            // P71.3a — the graph is the only place a delegated child, its depth
            // and the live counts exist. A caller that names no parent Work gets
            // policy-only admission (depth from its own spec, zero active /
            // total) and the reply says so, rather than passing a partial check
            // off as a full one.
            let parent_work_id = params
                .get("workId")
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .map(str::to_string);
            let (gauge, accounted_from) = match parent_work_id.as_deref() {
                Some(parent) => {
                    let child_work_id =
                        crate::work_gateway::WorkGateway::child_work_id(parent, &task_id);
                    let gw = gateway.lock().unwrap_or_else(|e| e.into_inner());
                    if gw.get_work(&child_work_id).is_some() {
                        // Deterministic child ids make "already spawned" a
                        // lookup, not a remembered set — which is why the Work
                        // graph can hold what the deleted registry used to.
                        return Err(agentcowork_blueprint::SubAgentError::DuplicateTask {
                            task_id: task_id.clone(),
                        }
                        .to_string());
                    }
                    (gw.delegation_gauge(parent)?, "work_graph")
                }
                None => (
                    agentcowork_blueprint::DelegationGauge {
                        child_depth: params.get("depth").and_then(|v| v.as_u64()).unwrap_or(0)
                            as u32,
                        active: 0,
                        total: 0,
                    },
                    "policy_only",
                ),
            };
            let member_agent_id = params
                .get("agentId")
                .and_then(|v| v.as_str())
                .unwrap_or(harness);
            policy
                .admit(&task_id, gauge, member_agent_id, readiness)
                .map_err(|e| e.to_string())?;
            // P64.4 — the child's grant set: denies inherited, `delegate` never
            // re-granted, `todo`/`task` default-deny unless this spec lists
            // them. The parent-grant side is the AgentBridge credential's to
            // prove (P69.B4); until then the spec is both the grant list and
            // the request, exactly as the coordinator sends it.
            spec.tools = agentcowork_blueprint::derive_child_permissions(
                &spec.tools,
                &spec.blocked_tools,
                &spec.tools,
            );
            spec.depth = gauge.child_depth;
            // P64.4 — the execution binding the child's executor starts from:
            // the spec-only starting prompt (fresh context — the parent's
            // transcript is never handed down). The isolated workspace address
            // is planned below, against the minted child Work.
            let starting_prompt = spec.starting_prompt();
            // P69.D14 — the child Work itself. The caller (the turn loop) names
            // the parent Work it is already inside, so the delegation lands in
            // the one Work graph with a parent link, its own Run and an
            // ephemeral AgentSession.
            let child = match parent_work_id.as_deref() {
                Some(parent_work_id) => {
                    let agent_id = params
                        .get("agentId")
                        .and_then(|v| v.as_str())
                        .unwrap_or(harness);
                    let mut gw = gateway.lock().unwrap_or_else(|e| e.into_inner());
                    Some(
                        gw.delegate_child_work(
                            parent_work_id,
                            &task_id,
                            &goal,
                            agent_id,
                            params
                                .get("worktreeId")
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                        )?,
                    )
                }
                None => None,
            };
            // P64.4 — the isolated workspace address (worktree segment +
            // 3-file blackboard paths) planned against the minted child Work
            // (EXTERNAL-AGENTS §3.1 step 3). The plan is advisory — the actual
            // `git worktree add` runs at execution — so an unmappable task id
            // soft-fails the provision (named, null) rather than failing an
            // admitted spawn.
            let (provision, provision_error): (serde_json::Value, serde_json::Value) = match child
                .as_ref()
            {
                Some(c) => match crate::execution::plan_subagent_worktree(&task_id, &c.work_id) {
                    Ok(p) => (
                        serde_json::to_value(&p).unwrap_or(serde_json::Value::Null),
                        serde_json::Value::Null,
                    ),
                    Err(e) => (serde_json::Value::Null, serde_json::json!(e)),
                },
                None => (serde_json::Value::Null, serde_json::Value::Null),
            };
            Ok(serde_json::json!({
                "task_id": task_id,
                "summary": "",
                "status": "running",
                "artifacts": [],
                "role": params.get("role").cloned().unwrap_or(serde_json::Value::Null),
                "harness": harness,
                "binding": binding,
                "planes": crate::RUNTIME_PLANES.len(),
                "tools": spec.tools,
                "depth": spec.depth,
                "accountedFrom": accounted_from,
                "startingPrompt": starting_prompt,
                "provision": provision,
                "provisionError": provision_error,
                "workId": child.as_ref().map(|c| c.work_id.clone()),
                "runId": child.as_ref().map(|c| c.run_id.clone()),
                "agentSessionId": child.as_ref().map(|c| c.agent_session_id.clone()),
            }))
        }
        "subagent/complete" | "subagent/fail" => {
            let task_id = params
                .get("taskId")
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .ok_or("subagent/complete requires taskId")?
                .to_string();
            let summary = params
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let artifacts: Vec<String> = params
                .get("artifacts")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let failed = method == "subagent/fail";
            // The child Work *is* the record of the delegation (I8), so closing
            // one without naming its parent is refused: there would be nothing
            // to verify the task against and nothing to append the terminal
            // event to. A task the graph does not know is an error, not a
            // fabricated summary.
            let parent_work_id = params
                .get("workId")
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .ok_or(
                    "subagent/complete requires workId — the child Work is the durable record of \
                     a delegated task (I8)",
                )?
                .to_string();
            let child_work_id =
                crate::work_gateway::WorkGateway::child_work_id(&parent_work_id, &task_id);
            {
                let mut gw = gateway.lock().unwrap_or_else(|e| e.into_inner());
                if gw.get_work(&child_work_id).is_none() {
                    return Err(agentcowork_blueprint::SubAgentError::UnknownTask {
                        task_id: task_id.clone(),
                    }
                    .to_string());
                }
                // Terminal Run event on the child's own timeline + the
                // ephemeral session's termination.
                gw.finish_child_work(
                    &parent_work_id,
                    &task_id,
                    if failed {
                        agentcowork_types::WorkState::Failed
                    } else {
                        agentcowork_types::WorkState::Completed
                    },
                    params.get("reason").and_then(|v| v.as_str()),
                )?;
            }
            let result = agentcowork_blueprint::SubAgentResult {
                task_id,
                summary,
                status: if failed {
                    agentcowork_blueprint::TaskStatus::Failed
                } else {
                    agentcowork_blueprint::TaskStatus::Done
                },
                artifacts,
            };
            // Summary-only by construction (WORK §8): one projection, no
            // transcript, plus the child Work id the caller closes.
            let mut view = agentcowork_blueprint::parent_view(&result);
            view["workId"] = serde_json::json!(child_work_id);
            Ok(view)
        }
        other => Err(format!("method not found: {other}")),
    }
}

/// P71.1 — the delegation bridge behind the `delegate.*` shared-plane façades.
///
/// It holds the same two pieces the coordinator's `subagent/*` seam holds —
/// the spawn policy and the one Work Gateway — so a delegated task from an
/// external agent and one from the turn loop are the *same* code path:
/// admission limits, child Work, Run and ephemeral AgentSession. There is no
/// second delegation registry, and an unmounted bridge fails honestly rather
/// than fabricating a spawn.
struct DelegationBridge {
    policy: agentcowork_blueprint::DelegationPolicy,
    gateway: Arc<Mutex<crate::work_gateway::WorkGateway>>,
    /// P71.3f — the shell-mounted readiness facts. The spawn gate reads the
    /// member's state here rather than trusting a boolean on the wire.
    readiness: crate::tools::SharedAgentReadiness,
}

/// P71.3f — the readiness that gates a spawned member: an external agent's
/// state is read from the mounted source, and an unmounted source is `Unknown`
/// (not admitted).
///
/// ADR-0005: nothing ships with the app, so there is no built-in member that
/// is `Ready` by construction — an unnamed agent reads as `Unknown` and the
/// policy refuses it by name instead of silently substituting an engine.
fn member_readiness(
    source: &crate::tools::SharedAgentReadiness,
    agent_id: &str,
) -> agentcowork_types::AgentReadiness {
    crate::tools::read_agent_readiness(source, agent_id)
}

impl crate::tools::DelegationToolBackend for DelegationBridge {
    fn spawn(&self, params: &serde_json::Value) -> Result<serde_json::Value, String> {
        let work_id = params.get("workId").and_then(|v| v.as_str()).unwrap_or("");
        if work_id.is_empty() {
            // The delegating Work is required: delegation without a parent Work
            // would mint an unlinked run outside the Work graph (I4). Identity
            // derivation from the bridge credential lands with the AgentBridge
            // (P69.B4); until then the caller must name it. Never inferred from
            // polite arguments.
            return Err(
                "delegate.spawn requires workId — the delegating Work (bridge-credential identity \
                 derivation is not mounted yet)"
                    .to_string(),
            );
        }
        // P71.3f — the member's readiness is read from the mounted source; an
        // unprobed or unnamed agent is `Unknown` and `DelegationPolicy::admit`
        // refuses it by name. No harness is assumed: an unnamed spawn must
        // never fall back to a built-in identity (ADR-0005).
        let agent_id = params
            .get("agentId")
            .and_then(|v| v.as_str())
            .or_else(|| params.get("harness").and_then(|v| v.as_str()))
            .unwrap_or("");
        let readiness = member_readiness(&self.readiness, agent_id);
        subagent_rpc(
            "subagent/spawn",
            params,
            &self.policy,
            &self.gateway,
            readiness,
        )
    }

    fn status(&self, params: &serde_json::Value) -> Result<serde_json::Value, String> {
        let gateway = self.gateway.lock().unwrap_or_else(|e| e.into_inner());
        let child_work_id = match params
            .get("childWorkId")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
        {
            Some(id) => id.to_string(),
            None => {
                let parent = params.get("workId").and_then(|v| v.as_str()).unwrap_or("");
                let task = params.get("taskId").and_then(|v| v.as_str()).unwrap_or("");
                if parent.is_empty() || task.is_empty() {
                    return Err("delegate.status requires childWorkId, or workId + taskId".into());
                }
                crate::work_gateway::WorkGateway::child_work_id(parent, task)
            }
        };
        let Some(work) = gateway.get_work(&child_work_id) else {
            return Err(format!("no delegated child Work `{child_work_id}`"));
        };
        let presence = gateway.presence(&child_work_id);
        Ok(serde_json::json!({
            "childWorkId": work.work_id.as_str(),
            "parentWorkId": work.parent_work_id,
            "runId": work.current_run_id,
            "version": work.version,
            "presence": presence,
            "children": gateway.children_of(&child_work_id).len(),
        }))
    }

    fn cancel(&self, params: &serde_json::Value) -> Result<serde_json::Value, String> {
        let parent = params
            .get("workId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let task = params
            .get("taskId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if parent.is_empty() || task.is_empty() {
            return Err("delegate.cancel requires workId + taskId".into());
        }
        let reason = params
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("cancelled by caller");
        // Closing the child Work **is** the release (P71.3a): the terminal
        // `RunCancelled` event on the child's own timeline lands in the same
        // step that stops the graph counting it as active, so there is no
        // separate accounting left to hand back. Cancelling an already-closed
        // child is idempotent.
        let child = {
            let mut gw = self.gateway.lock().unwrap_or_else(|e| e.into_inner());
            gw.finish_child_work(
                &parent,
                &task,
                agentcowork_types::WorkState::Cancelled,
                Some(reason),
            )?
        };
        let released = {
            let gw = self.gateway.lock().unwrap_or_else(|e| e.into_inner());
            gw.presence_is_terminal(&child.work_id)
        };
        Ok(serde_json::json!({
            "childWorkId": child.work_id,
            "runId": child.run_id,
            "status": "cancelled",
            "released": released,
        }))
    }
}

/// The `terminal/*` methods the coordinator drives.
///
/// **Read-only on purpose.** The agent's *privileged* shell path is the
/// ticketed `script.run` tool (`tool/exec` → `tool/commit` → Guard-2 →
/// `TerminalExecutor::run`). Adding a run method here would be a second,
/// unticketed execution path for a privileged effect, which is what the
/// no-bypass invariant forbids — so this arm only ever *observes*.
///
/// What the coordinator cannot otherwise see is the plane's state: whether a
/// shell exists on this host, which sessions are live with what provenance and
/// cwd, and what the shell itself reported about the commands it ran. That is
/// exactly the agent's own situational awareness, and it is served from the
/// same [`crate::terminal::TerminalPlaneObserver`] row builders the Shell view
/// reads, so the two cannot drift.
///
/// No plane attached is not an error: `terminal/status` answers
/// `attached: false` rather than an empty list that reads as "a shell with
/// nothing running". The per-session reads do refuse, because a session id on
/// a host with no PTY host is a caller bug, not an empty result.
fn terminal_rpc(
    method: &str,
    params: &serde_json::Value,
    plane: Option<&dyn crate::terminal::TerminalPlaneObserver>,
) -> Result<serde_json::Value, String> {
    let param_usize = |key: &str, default: usize| -> usize {
        params
            .get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(default)
    };
    let pty_id = || -> Result<&str, String> {
        params
            .get("ptyId")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("{method} requires ptyId"))
    };
    match method {
        "terminal/status" => {
            let status = match plane {
                Some(p) => p.plane_status(),
                None => crate::terminal::detached_plane_status(),
            };
            serde_json::to_value(status).map_err(|e| e.to_string())
        }
        "terminal/commands" => {
            let id = pty_id()?;
            let plane = plane.ok_or(NO_TERMINAL_PLANE)?;
            let rows = plane.commands(id, param_usize("limit", 50))?;
            let cwd = plane.session(id).map(|s| s.cwd).unwrap_or_default();
            serde_json::to_value(serde_json::json!({
                "ptyId": id,
                "cwd": cwd,
                "count": rows.len(),
                "commands": rows,
            }))
            .map_err(|e| e.to_string())
        }
        "terminal/last_command" => {
            let id = pty_id()?;
            let plane = plane.ok_or(NO_TERMINAL_PLANE)?;
            let block = plane.last_command(id, param_usize("maxChars", 6000))?;
            Ok(serde_json::json!({ "ptyId": id, "block": block }))
        }
        "terminal/history" => {
            let id = pty_id()?;
            let plane = plane.ok_or(NO_TERMINAL_PLANE)?;
            let block =
                plane.history(id, param_usize("limit", 10), param_usize("maxChars", 4000))?;
            Ok(serde_json::json!({ "ptyId": id, "block": block }))
        }
        other => Err(format!("method not found: {other}")),
    }
}

/// The refusal a per-session `terminal/*` read gets on a host with no PTY host.
/// Worded so the coordinator can surface it as a fact about the host rather
/// than retrying it as a transient failure.
const NO_TERMINAL_PLANE: &str = "terminal plane not attached — this host has no shell";

/// P64.3 — repo-map façade defaults. The coordinator applies its own token
/// budget on top of the returned rows, so this bound is only about how much of
/// the tree is walked.
const REPOMAP_MAX_FILES: usize = 200;
const REPOMAP_MAX_FILES_CAP: usize = 2000;

/// The `codeintel/*` methods the coordinator drives.
///
/// Served from `agentcowork-codeintel` — the same implementation behind the UI's
/// `repomap_build` command — so the agent-facing and UI-facing façades cannot
/// drift apart. `workspace` is the tool layer's floored root, so the map covers
/// the tree the edit tools actually operate on rather than a root of its own.
/// P64.6 — bind a shadow-preflight request's `root` to the workspace floor.
///
/// An **absolute** root is honoured as given (a caller may legitimately point
/// the check at a monorepo package); an **absent or relative** root resolves
/// against `workspace`, which is how every `file_ops` path is floored. The
/// coordinator's default is `.` because it does not know the workspace, so
/// without this resolution a fired preflight would stage a shadow tree of the
/// *sidecar's* own cwd — checking a project the user never edited, and paying a
/// `git worktree add` on the wrong repository to do it.
///
/// Resolution lives in the router rather than the kernel on purpose: the kernel
/// deliberately knows no workspace (it takes a root and uses it), and this arm
/// is the only layer holding both the kernel and the tool service that owns the
/// floored path.
fn with_preflight_root(params: &Value, workspace: &std::path::Path) -> Value {
    let raw = params
        .get("root")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let requested = std::path::Path::new(raw);
    // `.` (and an empty root) is how a caller that knows no workspace says "the
    // workspace itself". Joining it would leave a stray `.` component in the
    // path the receipt quotes, so it is answered directly.
    let resolved = if raw.is_empty() || raw == "." {
        workspace.to_path_buf()
    } else if requested.is_relative() {
        workspace.join(requested)
    } else {
        requested.to_path_buf()
    };
    let mut out = params.clone();
    if let Some(obj) = out.as_object_mut() {
        obj.insert(
            "root".to_string(),
            Value::String(resolved.to_string_lossy().into_owned()),
        );
    }
    out
}

fn codeintel_rpc(
    method: &str,
    params: &serde_json::Value,
    workspace: &std::path::Path,
) -> Result<serde_json::Value, String> {
    match method {
        "codeintel/repomap" => {
            let max_files = params
                .get("maxFiles")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(REPOMAP_MAX_FILES)
                .min(REPOMAP_MAX_FILES_CAP);
            let tags = agentcowork_codeintel::repomap::ranked_tags(workspace, max_files);
            Ok(serde_json::json!({ "tags": tags }))
        }
        other => Err(format!("method not found: {other}")),
    }
}

impl<W: Write + Send + 'static, R: Read + Send + 'static> ChatRelay<W, R> {
    /// Construct a relay, refusing startup if the durable Work journal cannot
    /// be opened. This compatibility constructor keeps the historical
    /// infallible API; use [`Self::try_new`] when the caller can handle the
    /// typed startup error.
    pub fn new(
        link: SidecarLink<W, R>,
        vault: Arc<Mutex<Vault>>,
        on_event: impl Fn(ChatWireEvent) + Send + 'static,
    ) -> Self {
        Self::try_new(link, vault, on_event)
            .unwrap_or_else(|error| panic!("chat relay startup refused: {error}"))
    }

    /// Fallible relay constructor. A Work journal failure is a startup error,
    /// never permission to run with an ephemeral gateway.
    pub fn try_new(
        link: SidecarLink<W, R>,
        vault: Arc<Mutex<Vault>>,
        on_event: impl Fn(ChatWireEvent) + Send + 'static,
    ) -> Result<Self, ChatRelayError> {
        Self::try_new_with_guard(
            link,
            vault,
            Arc::new(Mutex::new(GuardService::new())),
            on_event,
        )
    }

    /// Construct with a **shared** [`GuardService`] (the Tauri shell owns it,
    /// so approval cards and the coordinator's `guard/*` dispatch read/write
    /// one ticket store — single source of truth). The infallible compatibility
    /// wrapper explicitly refuses startup; new callers should use
    /// [`Self::try_new_with_guard`] to receive the typed error.
    pub fn new_with_guard(
        link: SidecarLink<W, R>,
        vault: Arc<Mutex<Vault>>,
        guard: Arc<Mutex<GuardService>>,
        on_event: impl Fn(ChatWireEvent) + Send + 'static,
    ) -> Self {
        Self::try_new_with_guard(link, vault, guard, on_event)
            .unwrap_or_else(|error| panic!("chat relay startup refused: {error}"))
    }

    /// Fallible constructor for hosts that can report a startup refusal to the
    /// user instead of terminating the relay thread.
    pub fn try_new_with_guard(
        link: SidecarLink<W, R>,
        vault: Arc<Mutex<Vault>>,
        guard: Arc<Mutex<GuardService>>,
        on_event: impl Fn(ChatWireEvent) + Send + 'static,
    ) -> Result<Self, ChatRelayError> {
        let egress = Arc::new(Mutex::new(agentcowork_guard::EgressEngine::new(
            agentcowork_guard::ConnectivityMode::ThirdParty,
        )));
        let capabilities = Arc::new(Mutex::new(agentcowork_guard::LocalCapabilityBroker::new()));
        let mut tool_service = ToolService::new_with_egress(
            Arc::clone(&guard),
            crate::default_data_dir().join("workspace"),
            Arc::clone(&egress),
        );
        tool_service.attach_capability_broker(Arc::clone(&capabilities));
        // P71.1 — the `delegate.*` façade seam: the same spawn policy and Work
        // Gateway the `subagent/*` arm uses, so delegation from an external
        // agent cannot diverge from coordinator delegation.
        let delegation = agentcowork_blueprint::DelegationPolicy::new(
            agentcowork_blueprint::SubAgentLimits::default(),
        );
        let work_gateway = crate::work_gateway::WorkGateway::open_default()
            .map_err(ChatRelayError::WorkGateway)?;
        // The journal is authoritative. An ExecutionKernel snapshot is only a
        // cache and is accepted solely after its identities/states validate
        // against the replayed Work events.
        let checkpoint_path = crate::default_data_dir()
            .join("work")
            .join("execution-kernel.checkpoint.json");
        let recovered_executions = ExecutionKernel::recover_from_work_gateway_with_checkpoint(
            &work_gateway,
            Some(&checkpoint_path),
        )
        .map_err(ChatRelayError::Recovery)?;
        let work_gateway = Arc::new(Mutex::new(work_gateway));
        // P71.3f — one readiness handle shared by the delegation seam and the
        // relay's own gates. The shell mounts the facts (`mount_readiness`);
        // until then every read is `Unknown` and nothing is admitted.
        let readiness: crate::tools::SharedAgentReadiness = Arc::new(Mutex::new(None));
        tool_service.attach_delegation(Arc::new(DelegationBridge {
            policy: delegation,
            gateway: Arc::clone(&work_gateway),
            readiness: Arc::clone(&readiness),
        }));
        let tools = Arc::new(Mutex::new(tool_service));
        Ok(Self {
            link,
            vault,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            memory: Arc::new(Mutex::new(load_persistent_memory())),
            guard,
            plan: Arc::new(Mutex::new(PlanService::new())),
            scheduler: Arc::new(Mutex::new(SchedulerService::load_or_new(
                crate::default_data_dir().join("scheduler.json"),
            ))),
            tools,
            evals: Arc::new(Mutex::new(EvalService::new())),
            executions: Arc::new(Mutex::new(recovered_executions)),
            delegation,
            readiness,
            skill_store: Arc::new(Mutex::new(agentcowork_blueprint::SkillStore::new(
                skills_root(),
            ))),
            terminal_plane: Arc::new(Mutex::new(None)),
            work_gateway,
            capabilities,
            egress,
            tasks: Arc::new(Mutex::new(crate::task_ledger::TaskLedger::new(Box::new(
                crate::task_ledger::FileStore::new(crate::default_data_dir().join("tasks.json")),
            )))),
            on_event: Arc::new(Mutex::new(Box::new(on_event))),
            agui: crate::agui::AguiRelay::new(),
        })
    }

    /// Unified execution kernel (chat / plan / scheduler / ACP).
    pub fn executions(&self) -> Arc<Mutex<crate::execution::ExecutionKernel>> {
        Arc::clone(&self.executions)
    }

    /// P49 local Work Gateway handle.
    pub fn work_gateway(&self) -> Arc<Mutex<crate::work_gateway::WorkGateway>> {
        Arc::clone(&self.work_gateway)
    }

    /// Authorize and durably record the beginning of a capability-backed
    /// effect. Only the opaque grant id is projected into Work events; no
    /// credential material crosses this boundary.
    pub fn record_capability_effect(
        &self,
        work_id: &str,
        effect_id: &str,
        request: agentcowork_guard::CapabilityRequest,
        grant_id: &str,
    ) -> Result<(), String> {
        // Durable-before-effect: the Work journal must acknowledge the attempt
        // before the broker is allowed to invoke the capability.
        self.work_gateway
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record_effect_with_grant(work_id, effect_id, "attempted", "", Some(grant_id))?;
        self.capabilities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .invoke(grant_id, &request)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The memory service handle (tests + the Tauri `usage_snapshot` command
    /// read from it; the sidecar writes through `memory/*` requests).
    pub fn memory(&self) -> Arc<Mutex<MemoryService>> {
        Arc::clone(&self.memory)
    }

    /// The Guard-2 service handle (the Tauri approval cards read from it; the
    /// coordinator drives `guard/*` requests against it).
    pub fn guard(&self) -> Arc<Mutex<GuardService>> {
        Arc::clone(&self.guard)
    }

    pub fn tools(&self) -> Arc<Mutex<ToolService>> {
        Arc::clone(&self.tools)
    }

    /// Attach a live CDP backend to the tool executor (after `browser_start`).
    pub fn attach_browser(&self, browser: Arc<dyn crate::tools::BrowserBackend>) {
        if let Ok(mut tools) = self.tools.lock() {
            tools.attach_browser(browser);
        }
    }

    /// P68.9 — attach the **one PTY plane** as the `script.run` executor.
    ///
    /// The host calls this at boot with the same `PtyHost` the Shell view uses,
    /// so an agent command runs on the automation profile with `Agent`
    /// provenance: audited as `terminal.agent_run` and rendered as a labelled
    /// read-only tab. Until it is attached, `script.run` fails honestly rather
    /// than quietly substituting a private pipe nobody can watch.
    pub fn attach_terminal(&self, terminal: Arc<dyn crate::tools::TerminalExecutor>) {
        if let Ok(mut tools) = self.tools.lock() {
            tools.attach_terminal(terminal);
        }
    }

    /// P54.5 — attach the **read-only** view of that same plane for `terminal/*`.
    ///
    /// This is the observation half of the seam, not a second execution path:
    /// the observer cannot run anything, so the only way to *cause* a shell
    /// effect from the sidecar stays the ticketed `script.run` tool. The host
    /// passes the same `PtyHost` object it gave `attach_terminal`, so a session
    /// the agent spawned is the session the coordinator can see.
    pub fn attach_terminal_plane(&self, plane: Arc<dyn crate::terminal::TerminalPlaneObserver>) {
        if let Ok(mut slot) = self.terminal_plane.lock() {
            *slot = Some(plane);
        }
    }

    /// P48.3 — attach the live desktop engine as the `desktop.*` executor.
    ///
    /// The host calls this at boot when a platform backend attaches, and again
    /// after a user-triggered `desktop_attach`. Until it is attached, every
    /// desktop tool fails closed with `desktop session not attached` — the
    /// honest headless / no-display posture, never a silent substitute.
    ///
    /// The backend is expected to declare **agent** provenance for its acts, so
    /// an agent-initiated desktop action is audited as an agent action rather
    /// than as the user's own gesture.
    pub fn attach_desktop(&self, desktop: Arc<dyn crate::tools::DesktopBackend>) {
        if let Ok(mut tools) = self.tools.lock() {
            tools.attach_desktop(desktop);
        }
    }

    /// The Stage-0 plan service handle (the coordinator steps per-plan
    /// circuit breakers via `plan/*`; trips surface as chat interrupts).
    pub fn plan(&self) -> Arc<Mutex<PlanService>> {
        Arc::clone(&self.plan)
    }

    /// The P6.4 scheduled-task service handle (the coordinator + Tauri shell
    /// drive it via `scheduler/*` methods).
    pub fn scheduler(&self) -> Arc<Mutex<SchedulerService>> {
        Arc::clone(&self.scheduler)
    }

    /// The P43 detached-work task ledger handle (the coordinator + Tauri
    /// shell drive it via `tasks/*` methods).
    pub fn tasks(&self) -> Arc<Mutex<crate::task_ledger::TaskLedger>> {
        Arc::clone(&self.tasks)
    }

    /// Load the J21 policy file into the Guard-2 service (builder pattern —
    /// the shell calls this at boot with `<data_dir>/permissions.toml`).
    pub fn with_policy(&self, path: &std::path::Path) -> &Self {
        self.guard
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .load_policy_from(path);
        self
    }

    /// The sidecar link (cancel path + tests).
    pub fn link(&self) -> &SidecarLink<W, R> {
        &self.link
    }

    /// Attach the AG-UI UI sink (P11.5.11). The shell calls this at boot so
    /// `agui/event` notifications from the coordinator are forwarded as
    /// `agui-event` emits. Returns the relay (the shell's `agui_send` command
    /// uses it to push UI→coordinator events into the sidecar link).
    ///
    /// The emit is **unconsumed today**: no UI code listens for `agui-event`
    /// (and `agui_send`/`agui_listen` are never invoked from the UI), because
    /// the generative-UI surface that would consume them is the deferred half
    /// of P11.5.11. Attaching the sink does not make AG-UI live.
    pub fn with_agui(&self, sink: impl Fn(String) + Send + 'static) -> crate::agui::AguiRelay {
        self.agui.attach(sink);
        self.agui.clone()
    }

    /// The AG-UI relay handle (Tauri `agui_send`/`agui_stream` commands).
    pub fn agui(&self) -> crate::agui::AguiRelay {
        self.agui.clone()
    }

    /// Push a UI→coordinator AG-UI event into the sidecar link as an
    /// `agui/event` notification (e.g. `interrupt_resolved`).
    pub fn send_agui(&self, line: &str) -> Result<(), crate::sidecar_link::LinkError> {
        self.link
            .writer()
            .notify("agui/event", serde_json::json!({ "line": line }))
    }

    /// Start the long-lived consumer loop (call ONCE per link). Serves the
    /// in-process services the coordinator drives (`memory/*`, `guard/*`,
    /// `tool/*`, `subagent/*`, `skill/*`, `usage/recent`, `agent/readiness`, …)
    /// and forwards `chat/*` notifications to `on_event`, including the
    /// post-turn budget kill.
    ///
    /// P71.2c — it no longer brokers **provider inference**: `provider/stream`
    /// and every endpoint/profile map that fed it were deleted with the
    /// built-in engine. A turn now runs on the bound external agent's own
    /// channel (`acp_prompt`), which owns its provider, model and credentials
    /// (ADR-0005 §2, `ARCH/ROUTING.md` §1).
    pub fn spawn(&self) {
        let vault = Arc::clone(&self.vault);
        let receiver = self.link.receiver();
        let writer = self.link.writer();
        let sessions = Arc::clone(&self.sessions);
        let on_event = Arc::clone(&self.on_event);
        let memory = Arc::clone(&self.memory);
        let guard = Arc::clone(&self.guard);
        let plan = Arc::clone(&self.plan);
        let scheduler = Arc::clone(&self.scheduler);
        let tools = Arc::clone(&self.tools);
        let evals = Arc::clone(&self.evals);
        let executions = Arc::clone(&self.executions);
        let delegation = self.delegation;
        let skill_store = Arc::clone(&self.skill_store);
        let terminal_plane = Arc::clone(&self.terminal_plane);
        let work_gateway = Arc::clone(&self.work_gateway);
        let capabilities = Arc::clone(&self.capabilities);
        let egress = Arc::clone(&self.egress);
        let tasks = Arc::clone(&self.tasks);
        let readiness = Arc::clone(&self.readiness);
        let agui = self.agui.clone();

        std::thread::spawn(move || {
            loop {
                let inbound = receiver.lock().unwrap_or_else(|e| e.into_inner()).recv();
                let Ok(inbound) = inbound else {
                    break; // reader thread gone — sidecar is dead
                };
                match inbound {
                    Inbound::Request { id, method, params } => match method.as_str() {
                        // ARCH/05 durable-observation seam: the coordinator
                        // hydrates its RouteDecision ring at boot from the vault's
                        // `token_usage` ledger (provider/model/cost per completed
                        // call) so routing survives restarts. Wrapped: vault read
                        // is a small indexed query on the consumer loop.
                        "usage/recent" => {
                            let v = vault.lock().unwrap_or_else(|e| e.into_inner());
                            let limit = params.get("limit").and_then(|x| x.as_u64()).unwrap_or(100);
                            match v.recent_usage(limit) {
                                Ok(rows) => {
                                    let _ = writer.reply(
                                        id,
                                        serde_json::to_value(rows)
                                            .unwrap_or_else(|_| serde_json::json!([])),
                                    );
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e.to_string());
                                }
                            }
                        }
                        // P71.3f — one readiness read for every caller: the picker
                        // façade, the trigger plane's doctor and the sidecar's own
                        // gates all ask this. Served from the shell's mounted
                        // source; unmounted ⇒ `unknown`, never a guessed `ready`.
                        "agent/readiness" => {
                            let agent_id = params
                                .get("agentId")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("");
                            let state = crate::tools::read_agent_readiness(&readiness, agent_id);
                            let _ = writer.reply(
                                id,
                                serde_json::json!({
                                    "agentId": agent_id,
                                    "readiness": state.as_str(),
                                    "ready": state.is_ready(),
                                    "installed": state.is_installed(),
                                    "launchable": state.is_launchable(),
                                    "needsAuth": state.needs_auth(),
                                    "canDelegate": state.can_delegate(),
                                    "reason": state.summary(),
                                }),
                            );
                        }
                        // P5.1/P5.3/P5.4/P5.9: memory + usage dispatch. Runs on
                        // the consumer loop (fast, deterministic, no I/O) so the
                        // reply is synchronous and the sidecar can await it.
                        method if method.starts_with("memory/") || method == "usage/snapshot" => {
                            let mut svc = memory.lock().unwrap_or_else(|e| e.into_inner());
                            let is_mutation = matches!(
                                method,
                                "memory/write"
                                    | "memory/forget"
                                    | "memory/ghost"
                                    | "memory/ghost_batch"
                                    | "memory/consolidate"
                                    | "memory/tick"
                                    | "memory/scope"
                                    | "memory/assess"
                                    | "memory/load"
                            );
                            match svc.handle(method, &params) {
                                Ok(mut out) => {
                                    if is_mutation {
                                        persist_memory(&svc);
                                    }
                                    // P51.28 — learnedSkills on memory/plan is the
                                    // model warm set (disable-model-invocation omitted).
                                    if method == "memory/plan" {
                                        let store =
                                            skill_store.lock().unwrap_or_else(|e| e.into_inner());
                                        if let Ok(skills) = store.scan() {
                                            if let Some(obj) = out.as_object_mut() {
                                                obj.insert(
                                                    "learnedSkills".into(),
                                                    serde_json::json!(
                                                        agentcowork_blueprint::model_warm_set(
                                                            &skills
                                                        )
                                                    ),
                                                );
                                            }
                                        }
                                    }
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        // P43 (B7 v3.53): detached-work task ledger dispatch. The
                        // coordinator + Tauri shell drive the same Rust-owned
                        // state machine (tasks/list, start, complete, cancel,
                        // retry, reap, prune) — completion wakes watchers
                        // (push-driven, never polled).
                        method if method.starts_with("tasks/") => {
                            let mut svc = tasks.lock().unwrap_or_else(|e| e.into_inner());
                            match svc.handle(method, &params) {
                                Ok(out) => {
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        // P7.5/J21: Guard-2 pre-flight + executor call-sites. The
                        // sidecar drives the *restricted* surface (`handle_sidecar`):
                        // it can evaluate + use tickets + read, but never
                        // approve/reject/reset/estop/profile (human-only).
                        method if method.starts_with("guard/") => {
                            let mut svc = guard.lock().unwrap_or_else(|e| e.into_inner());
                            match svc.handle_sidecar(method, &params) {
                                Ok(out) => {
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        // P6.3 Stage-0: per-plan circuit-breaker stepping. The
                        // coordinator proposes each step; Rust disposes (the
                        // breaker state lives here). Trips come back as
                        // `{ok:false, interrupt}` and become chat/interrupt.
                        method if method.starts_with("plan/") => {
                            let mut svc = plan.lock().unwrap_or_else(|e| e.into_inner());
                            match svc.handle(method, &params) {
                                Ok(out) => {
                                    if method == "plan/begin" {
                                        if let Some(pid) =
                                            params.get("planId").and_then(|v| v.as_str())
                                        {
                                            let mut k = executions
                                                .lock()
                                                .unwrap_or_else(|e| e.into_inner());
                                            let ex = k.begin(
                                                crate::execution::ExecutionTrigger::Plan,
                                                pid,
                                                pid,
                                                None,
                                                String::new(),
                                                format!(r#"{{"planId":"{pid}"}}"#),
                                                vec![],
                                            );
                                            k.alias(&format!("plan:{pid}"), &ex.id);
                                            let _ = k.transition(
                                                &ex.id,
                                                crate::execution::ExecutionPhase::Running,
                                            );
                                        }
                                    }
                                    if method == "plan/end" {
                                        if let Some(pid) =
                                            params.get("planId").and_then(|v| v.as_str())
                                        {
                                            let mut k = executions
                                                .lock()
                                                .unwrap_or_else(|e| e.into_inner());
                                            if let Some(id) = k
                                                .by_alias(&format!("plan:{pid}"))
                                                .map(|e| e.id.clone())
                                            {
                                                let _ = k.transition(
                                                    &id,
                                                    crate::execution::ExecutionPhase::Verifying,
                                                );
                                                let _ = k.transition(
                                                    &id,
                                                    crate::execution::ExecutionPhase::Completed,
                                                );
                                            }
                                        }
                                    }
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        // P6.4 (B7) / P71.3d — scheduled-task dispatch over the
                        // **trigger plane**. The coordinator ticks
                        // `scheduler/due`, records firings (`mark_fired`) and
                        // fires events + webhooks; Rust owns the trigger registry
                        // only. Execution is the Work kernel's business (the run
                        // lifecycle mirrors below keep their ExecutionLedger
                        // presence via `scheduler/due` + `scheduler/mark_fired`).
                        method if method.starts_with("scheduler/") => {
                            let mut svc = scheduler.lock().unwrap_or_else(|e| e.into_inner());
                            match svc.handle(method, &params) {
                                Ok(out) => {
                                    // A trigger firing becomes an ExecutionLedger
                                    // run (alias `job:<id>`) when the host starts
                                    // executing it; `mark_fired` closes it.
                                    if method == "scheduler/due" {
                                        if let Some(jobs) =
                                            out.get("due").and_then(|v| v.as_array())
                                        {
                                            for jid in jobs.iter().filter_map(|v| v.as_str()) {
                                                let mut k = executions
                                                    .lock()
                                                    .unwrap_or_else(|e| e.into_inner());
                                                if k.by_alias(&format!("job:{jid}")).is_some() {
                                                    continue; // already in flight
                                                }
                                                let ex = k.begin_named(
                                                    format!("sched-{jid}"),
                                                    crate::execution::ExecutionTrigger::Scheduler,
                                                    jid,
                                                    jid,
                                                    None,
                                                    String::new(),
                                                    format!(r#"{{"jobId":"{jid}"}}"#),
                                                    vec![],
                                                );
                                                k.alias(&format!("job:{jid}"), &ex.id);
                                                let _ = k.transition(
                                                    &ex.id,
                                                    crate::execution::ExecutionPhase::Running,
                                                );
                                            }
                                        }
                                    }
                                    if method == "scheduler/mark_fired" {
                                        if let Some(jid) = params.get("id").and_then(|v| v.as_str())
                                        {
                                            let mut k = executions
                                                .lock()
                                                .unwrap_or_else(|e| e.into_inner());
                                            if let Some(eid) = k
                                                .by_alias(&format!("job:{jid}"))
                                                .map(|e| e.id.clone())
                                            {
                                                let _ = k.transition(
                                                    &eid,
                                                    crate::execution::ExecutionPhase::Verifying,
                                                );
                                                let _ = k.transition(
                                                    &eid,
                                                    crate::execution::ExecutionPhase::Completed,
                                                );
                                            }
                                        }
                                    }
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        method if method.starts_with("tool/") => {
                            let mut svc = tools.lock().unwrap_or_else(|e| e.into_inner());
                            let effect_id = params
                                .get("effectId")
                                .and_then(|v| v.as_str())
                                .or_else(|| params.get("ticketId").and_then(|v| v.as_str()))
                                .unwrap_or("")
                                .to_string();
                            let work_id = params.get("workId").and_then(|v| v.as_str());
                            let grant_id = params.get("capabilityGrantId").and_then(|v| v.as_str());
                            if method == "tool/commit" && !effect_id.is_empty() {
                                if let Some(work_id) = work_id {
                                    if let Err(error) = work_gateway
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .record_effect_with_grant(
                                            work_id,
                                            &effect_id,
                                            "attempted",
                                            "",
                                            grant_id,
                                        )
                                    {
                                        let _ = writer.reply_error(id, &error);
                                        continue;
                                    }
                                }
                            }
                            let result = svc.handle(method, &params);
                            let mut event_error = None;
                            if method == "tool/commit" && !effect_id.is_empty() {
                                if let Some(work_id) = work_id {
                                    let mut gateway =
                                        work_gateway.lock().unwrap_or_else(|e| e.into_inner());
                                    let record = match &result {
                                        Ok(out) => {
                                            let ok = out
                                                .get("ok")
                                                .and_then(|v| v.as_bool())
                                                .unwrap_or(false);
                                            let outcome = out
                                                .get("state")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or(if ok { "ok" } else { "failed" });
                                            gateway
                                                .record_effect(
                                                    work_id, &effect_id, "observed", outcome,
                                                )
                                                .and_then(|_| {
                                                    gateway.record_effect(
                                                        work_id,
                                                        &effect_id,
                                                        "verified",
                                                        if ok { "true" } else { "false" },
                                                    )
                                                })
                                        }
                                        Err(error) => gateway
                                            .record_effect(
                                                work_id,
                                                &effect_id,
                                                "observed",
                                                error.as_str(),
                                            )
                                            .and_then(|_| {
                                                gateway.record_effect(
                                                    work_id, &effect_id, "verified", "false",
                                                )
                                            }),
                                    };
                                    if let Err(error) = record {
                                        event_error = Some(error);
                                    }
                                }
                            }
                            if let Some(error) = event_error {
                                let _ = writer.reply_error(id, &error);
                                continue;
                            }
                            match result {
                                Ok(out) => {
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        method if method.starts_with("eval/") => {
                            let mut svc = evals.lock().unwrap_or_else(|e| e.into_inner());
                            match svc.handle(method, &params) {
                                Ok(out) => {
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        method if method.starts_with("execution/") => {
                            // P64.6 — resolve the shadow-preflight root before the
                            // kernel sees the request. The coordinator sends `.`
                            // (it has no workspace), and the kernel has none either,
                            // so this arm — holding both the kernel and the tool
                            // service — is where `.` becomes the floored workspace.
                            let rooted: Option<Value> = if method == "execution/preflight" {
                                let workspace = {
                                    let svc = tools.lock().unwrap_or_else(|e| e.into_inner());
                                    svc.workspace().to_path_buf()
                                };
                                Some(with_preflight_root(&params, &workspace))
                            } else {
                                None
                            };
                            // Validate the kernel edge before either owner mutates;
                            // this keeps a refused Work transition from advancing a
                            // cache that the journal would reject.
                            if method == "execution/transition" {
                                if let (Some(execution_id), Some(state)) = (
                                    params.get("id").and_then(Value::as_str),
                                    params
                                        .get("wait")
                                        .filter(|value| !value.is_null())
                                        .and_then(|value| {
                                            serde_json::from_value::<
                                                    agentcowork_types::WaitCondition,
                                                >(
                                                    value.clone()
                                                )
                                                .ok()
                                        })
                                        .map(|wait| wait.work_state())
                                        .or_else(|| {
                                            params
                                                .get("state")
                                                .and_then(Value::as_str)
                                                .and_then(agentcowork_types::WorkState::try_parse)
                                        })
                                        .map(crate::execution::ExecutionPhase::from_work_state),
                                ) {
                                    let check =
                                        executions.lock().unwrap_or_else(|e| e.into_inner());
                                    if let Err(error) =
                                        check.validate_transition(execution_id, state)
                                    {
                                        let _ = writer.reply_error(id, &error);
                                        continue;
                                    }
                                }
                            }
                            let mut transition_committed = false;
                            if method == "execution/transition" {
                                if let (Some(work_id), Some(execution_id), Some(state)) = (
                                    params.get("workId").and_then(Value::as_str),
                                    params.get("id").and_then(Value::as_str),
                                    params.get("state").and_then(Value::as_str),
                                ) {
                                    if let Some(parsed) =
                                        agentcowork_types::WorkState::try_parse(state)
                                    {
                                        let wait = params
                                            .get("wait")
                                            .filter(|value| !value.is_null())
                                            .and_then(|value| {
                                                serde_json::from_value::<
                                                    agentcowork_types::WaitCondition,
                                                >(
                                                    value.clone()
                                                )
                                                .ok()
                                            });
                                        let outcome = {
                                            let mut gateway = work_gateway
                                                .lock()
                                                .unwrap_or_else(|e| e.into_inner());
                                            match wait {
                                                Some(condition) => gateway.record_wait(
                                                    work_id,
                                                    execution_id,
                                                    &condition,
                                                ),
                                                None => gateway.record_execution_transition(
                                                    work_id,
                                                    execution_id,
                                                    parsed,
                                                ),
                                            }
                                        };
                                        if let Err(error) = outcome {
                                            let _ = writer.reply_error(id, &error);
                                            continue;
                                        }
                                        transition_committed = true;
                                    }
                                }
                            }
                            // Approval facts are owned by the Work journal.  The
                            // in-memory ExecutionKernel is only its validated
                            // projection, so journal-first is required here rather
                            // than a post-hoc best-effort annotation.
                            let mut execution_params = params.clone();
                            if method == "execution/record_approval" {
                                let Some(work_id) = params.get("workId").and_then(Value::as_str)
                                else {
                                    let _ = writer.reply_error(
                                    id,
                                    "execution/record_approval requires workId for durable recovery",
                                );
                                    continue;
                                };
                                let Some(execution_id) = params.get("id").and_then(Value::as_str)
                                else {
                                    let _ = writer
                                        .reply_error(id, "execution/record_approval requires id");
                                    continue;
                                };
                                let Some(ticket_id) =
                                    params.get("ticketId").and_then(Value::as_str)
                                else {
                                    let _ = writer.reply_error(
                                        id,
                                        "execution/record_approval requires ticketId",
                                    );
                                    continue;
                                };
                                {
                                    let check =
                                        executions.lock().unwrap_or_else(|e| e.into_inner());
                                    if let Err(error) = check
                                        .validate_pending_approval_request(execution_id, ticket_id)
                                    {
                                        let _ = writer.reply_error(id, &error);
                                        continue;
                                    }
                                }
                                let requested_at_ms = execution_params
                                    .get("requestedAtMs")
                                    .and_then(Value::as_u64)
                                    .unwrap_or_else(|| {
                                        std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .map(|duration| duration.as_millis() as u64)
                                            .unwrap_or_default()
                                    });
                                if let Some(object) = execution_params.as_object_mut() {
                                    object.insert(
                                        "requestedAtMs".into(),
                                        serde_json::Value::from(requested_at_ms),
                                    );
                                }
                                let outcome = {
                                    let mut gateway =
                                        work_gateway.lock().unwrap_or_else(|e| e.into_inner());
                                    gateway.record_pending_approval_for_run(
                                        work_id,
                                        execution_id,
                                        ticket_id,
                                        execution_params
                                            .get("toolId")
                                            .and_then(Value::as_str)
                                            .unwrap_or(""),
                                        execution_params
                                            .get("argsHash")
                                            .and_then(Value::as_str)
                                            .unwrap_or(""),
                                        execution_params
                                            .get("riskTier")
                                            .and_then(Value::as_str)
                                            .unwrap_or("R1"),
                                        requested_at_ms,
                                    )
                                };
                                if let Err(error) = outcome {
                                    let _ = writer.reply_error(id, &error);
                                    continue;
                                }
                            }
                            if method == "execution/resolve_approval" {
                                let Some(work_id) = params.get("workId").and_then(Value::as_str)
                                else {
                                    let _ = writer.reply_error(
                                    id,
                                    "execution/resolve_approval requires workId for durable recovery",
                                );
                                    continue;
                                };
                                let Some(execution_id) = params.get("id").and_then(Value::as_str)
                                else {
                                    let _ = writer
                                        .reply_error(id, "execution/resolve_approval requires id");
                                    continue;
                                };
                                let Some(ticket_id) =
                                    params.get("ticketId").and_then(Value::as_str)
                                else {
                                    let _ = writer.reply_error(
                                        id,
                                        "execution/resolve_approval requires ticketId",
                                    );
                                    continue;
                                };
                                let Some(approved) =
                                    params.get("approved").and_then(Value::as_bool)
                                else {
                                    let _ = writer.reply_error(
                                        id,
                                        "execution/resolve_approval requires approved (bool)",
                                    );
                                    continue;
                                };
                                {
                                    let check =
                                        executions.lock().unwrap_or_else(|e| e.into_inner());
                                    if let Err(error) = check.validate_pending_approval_resolution(
                                        execution_id,
                                        ticket_id,
                                    ) {
                                        let _ = writer.reply_error(id, &error);
                                        continue;
                                    }
                                }
                                let outcome = {
                                    let mut gateway =
                                        work_gateway.lock().unwrap_or_else(|e| e.into_inner());
                                    gateway.resolve_pending_approval_for_run(
                                        work_id,
                                        execution_id,
                                        ticket_id,
                                        approved,
                                    )
                                };
                                if let Err(error) = outcome {
                                    let _ = writer.reply_error(id, &error);
                                    continue;
                                }
                            }
                            let result = {
                                let mut svc = executions.lock().unwrap_or_else(|e| e.into_inner());
                                svc.handle(method, rooted.as_ref().unwrap_or(&execution_params))
                            };
                            let mut event_error: Option<String> = None;
                            if let Ok(out) = &result {
                                if method == "execution/attach_receipt" {
                                    if let (Some(work_id), Some(receipt_id)) = (
                                        params.get("workId").and_then(|v| v.as_str()),
                                        params.get("receiptId").and_then(|v| v.as_str()),
                                    ) {
                                        if let Err(error) = work_gateway
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner())
                                            .record_artifact(work_id, receipt_id, true)
                                        {
                                            event_error = Some(error);
                                        }
                                    }
                                }
                                if event_error.is_none() && method == "execution/begin" {
                                    if let (Some(work_id), Some(execution_id)) = (
                                        params.get("workId").and_then(|v| v.as_str()),
                                        out.get("id").and_then(|v| v.as_str()),
                                    ) {
                                        if let Err(error) = work_gateway
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .bind_execution_with_metadata(
                                            work_id,
                                            execution_id,
                                            &serde_json::json!({
                                                "trigger": params.get("trigger").and_then(Value::as_str).unwrap_or("chat"),
                                                "sessionId": params.get("sessionId").and_then(Value::as_str),
                                                "objective": out.get("objective").and_then(Value::as_str),
                                                "policySnapshot": params.get("policySnapshot").and_then(Value::as_str),
                                                "contextSnapshot": params.get("contextSnapshot").and_then(Value::as_str),
                                                "capabilityScope": params.get("capabilityScope").cloned().unwrap_or_else(|| serde_json::json!([])),
                                                "idempotencyKey": out.get("idempotencyKey").and_then(Value::as_str),
                                            }),
                                        )
                                    {
                                        event_error = Some(error);
                                    }
                                    }
                                }
                                if event_error.is_none()
                                    && !transition_committed
                                    && method == "execution/transition"
                                {
                                    // P71.3g — the wire state is the canonical
                                    // `WorkState` spelling; an unknown spelling is
                                    // refused (never silently coerced into a made-up
                                    // state), and a supplied wait condition parks the
                                    // run with its reason attached.
                                    if let (Some(work_id), Some(execution_id), Some(state)) = (
                                        params.get("workId").and_then(|v| v.as_str()),
                                        params.get("id").and_then(|v| v.as_str()),
                                        params.get("state").and_then(|v| v.as_str()),
                                    ) {
                                        match agentcowork_types::WorkState::try_parse(state) {
                                            Some(parsed) => {
                                                let mut gw = work_gateway
                                                    .lock()
                                                    .unwrap_or_else(|e| e.into_inner());
                                                let wait = params
                                                    .get("wait")
                                                    .filter(|v| !v.is_null())
                                                    .and_then(|v| {
                                                        serde_json::from_value::<
                                                            agentcowork_types::WaitCondition,
                                                        >(
                                                            v.clone()
                                                        )
                                                        .ok()
                                                    });
                                                let outcome = match wait {
                                                    Some(condition) => gw.record_wait(
                                                        work_id,
                                                        execution_id,
                                                        &condition,
                                                    ),
                                                    None => gw.record_execution_transition(
                                                        work_id,
                                                        execution_id,
                                                        parsed,
                                                    ),
                                                };
                                                if let Err(error) = outcome {
                                                    event_error = Some(error);
                                                }
                                            }
                                            None => {
                                                event_error = Some(format!(
                                                    "execution/transition: unknown state {state:?} \
                                                 (expected a canonical WorkState spelling)"
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some(error) = event_error {
                                let _ = writer.reply_error(id, &error);
                                continue;
                            }
                            match result {
                                Ok(out) => {
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        "capability/allow" => {
                            let run_id = params.get("runId").and_then(|v| v.as_str()).unwrap_or("");
                            let scopes = params
                                .get("capabilities")
                                .and_then(|v| v.as_array())
                                .map(|items| {
                                    items
                                        .iter()
                                        .filter_map(|v| v.as_str().map(str::to_string))
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            if run_id.is_empty() || scopes.is_empty() {
                                let _ =
                                    writer.reply_error(id, "runId and capabilities are required");
                            } else {
                                capabilities
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .allow_for_run(run_id, scopes);
                                let _ = writer.reply(id, serde_json::json!({ "ok": true }));
                            }
                        }
                        "capability/authorize" => {
                            let request: Result<agentcowork_guard::CapabilityRequest, _> =
                                serde_json::from_value(params.clone());
                            let ttl = params
                                .get("ttlMs")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(60_000);
                            match request {
                                Ok(request) => match capabilities
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .authorize(request, ttl)
                                {
                                    Ok(grant) => {
                                        let _ = writer.reply(
                                            id,
                                            serde_json::to_value(grant)
                                                .unwrap_or_else(|_| serde_json::json!({})),
                                        );
                                    }
                                    Err(e) => {
                                        let _ = writer.reply_error(id, &e.to_string());
                                    }
                                },
                                Err(e) => {
                                    let _ = writer.reply_error(
                                        id,
                                        &format!("invalid capability request: {e}"),
                                    );
                                }
                            }
                        }
                        "capability/revoke" => {
                            let grant_id =
                                params.get("grantId").and_then(|v| v.as_str()).unwrap_or("");
                            let revoked = capabilities
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .revoke(grant_id);
                            let _ = writer.reply(id, serde_json::json!({ "revoked": revoked }));
                        }
                        "capability/manifest" => {
                            let commit =
                                params.get("commit").and_then(|c| c.as_str()).unwrap_or("");
                            let man = crate::capability_manifest::generate_manifest(commit);
                            match serde_json::to_value(&man) {
                                Ok(v) => {
                                    let _ = writer.reply(id, v);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e.to_string());
                                }
                            }
                        }
                        method if method.starts_with("egress/") => {
                            let mut eng = egress.lock().unwrap_or_else(|e| e.into_inner());
                            match method {
                                "egress/mode" => {
                                    if let Some(m) = params.get("mode").and_then(|v| v.as_str()) {
                                        let mode = match m {
                                            "offline" => {
                                                agentcowork_guard::ConnectivityMode::Offline
                                            }
                                            "local" => agentcowork_guard::ConnectivityMode::Local,
                                            "byok" => agentcowork_guard::ConnectivityMode::Byok,
                                            _ => agentcowork_guard::ConnectivityMode::ThirdParty,
                                        };
                                        eng.set_mode(mode);
                                    }
                                    let _ = writer.reply(id, serde_json::json!({ "ok": true }));
                                }
                                "egress/check" => {
                                    let dest = params
                                        .get("destination")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    let kind = params
                                        .get("kind")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("network");
                                    let plan = eng.plan(dest, kind, None, "check", &[]);
                                    let _ = writer.reply(
                                        id,
                                        serde_json::to_value(plan).unwrap_or(serde_json::json!({})),
                                    );
                                }
                                "egress/inventory" => {
                                    let _ = writer.reply(
                                        id,
                                        serde_json::json!({ "inventory": eng.inventory() }),
                                    );
                                }
                                _ => {
                                    let _ = writer.reply_error(id, "method not found");
                                }
                            }
                        }
                        // P49.10–12: session-runtime lifecycle. The agent loop
                        // drives PtySession / WorktreeBinding / AgentSession as
                        // first-class tools; the gateway owns the durable state +
                        // WorkEvent fan-out (survives client disconnect). This is
                        // the sole `work/*` arm: WorkGateway::handle_rpc also owns
                        // the legacy method vocabulary, so newer methods cannot
                        // be shadowed by a partial helper.
                        method if method.starts_with("work/") => {
                            let mut gw = work_gateway.lock().unwrap_or_else(|e| e.into_inner());
                            match gw.handle_rpc(method, &params) {
                                Ok(out) => {
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        // P64.8 — skill distillation. Was `method not found`, so the
                        // coordinator's "best-effort" catch swallowed it and nothing
                        // was ever distilled.
                        method if method.starts_with("skill/") => {
                            let store = skill_store.lock().unwrap_or_else(|e| e.into_inner());
                            match skill_rpc(method, &params, &store) {
                                Ok(out) => {
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        // P64.4 — sub-agent spawn admission. Was `method not found`,
                        // which made `dispatchSubAgent` throw "native runtime not
                        // wired" on every delegation.
                        method if method.starts_with("subagent/") => {
                            // P71.3f — the readiness gate reads the same mounted
                            // source the delegation seam and the turn gate use. An
                            // unnamed member is `Unknown`, never a built-in
                            // fallback (ADR-0005).
                            let agent_id = params
                                .get("agentId")
                                .and_then(serde_json::Value::as_str)
                                .or_else(|| {
                                    params.get("harness").and_then(serde_json::Value::as_str)
                                })
                                .unwrap_or("");
                            let member = member_readiness(&readiness, agent_id);
                            match subagent_rpc(method, &params, &delegation, &work_gateway, member)
                            {
                                Ok(out) => {
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        // P64.3 — the repo-map façade the coordinator drives with
                        // `codeintel/repomap`. While this arm was missing the
                        // request fell through to `method not found`, and because
                        // the coordinator treats the map as best-effort the failure
                        // was silent: the repo map was never injected in production.
                        method if method.starts_with("codeintel/") => {
                            let workspace = {
                                let svc = tools.lock().unwrap_or_else(|e| e.into_inner());
                                svc.workspace().to_path_buf()
                            };
                            match codeintel_rpc(method, &params, &workspace) {
                                Ok(out) => {
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        // P54.5 — the read-only view of the one PTY plane. The
                        // privileged shell path is `script.run` on `tool/exec` →
                        // `tool/commit` (Guard-2 ticketed); this arm deliberately
                        // has no run method, so it cannot become a second path to a
                        // privileged effect.
                        method if method.starts_with("terminal/") => {
                            let slot = terminal_plane.lock().unwrap_or_else(|e| e.into_inner());
                            let plane: Option<&dyn crate::terminal::TerminalPlaneObserver> =
                                slot.as_deref();
                            match terminal_rpc(method, &params, plane) {
                                Ok(out) => {
                                    let _ = writer.reply(id, out);
                                }
                                Err(e) => {
                                    let _ = writer.reply_error(id, &e);
                                }
                            }
                        }
                        _ => {
                            let _ = writer.reply_error(id, &format!("method not found: {method}"));
                        }
                    },
                    Inbound::Notification { method, params } => {
                        let stream_id = params
                            .get("streamId")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string();
                        // The coordinator carries the originating session on the
                        // event. The stream registry is the compatibility path
                        // for older sidecars and for events emitted immediately
                        // after the Rust request is acknowledged. Never infer a
                        // session from the UI's active tab.
                        let event_session_id = params
                            .get("sessionId")
                            .and_then(|s| s.as_str())
                            .map(str::to_string)
                            .or_else(|| {
                                sessions
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .get(&stream_id)
                                    .cloned()
                            })
                            .unwrap_or_default();
                        match method.as_str() {
                            "chat/ttft" => emit(
                                &on_event,
                                ChatWireEvent::Ttft {
                                    session_id: event_session_id.clone(),
                                    latency_ms: params
                                        .get("latencyMs")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0),
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            "chat/batch" => emit(
                                &on_event,
                                ChatWireEvent::Batch {
                                    session_id: event_session_id.clone(),
                                    text: params
                                        .get("text")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    token_count: params
                                        .get("tokenCount")
                                        .and_then(|t| t.as_u64())
                                        .unwrap_or(0),
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            "chat/reasoning" => emit(
                                &on_event,
                                ChatWireEvent::Reasoning {
                                    session_id: event_session_id.clone(),
                                    text: params
                                        .get("text")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            "chat/stage" => emit(
                                &on_event,
                                ChatWireEvent::Stage {
                                    session_id: event_session_id.clone(),
                                    stage: params
                                        .get("stage")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            "chat/tool_call" => emit(
                                &on_event,
                                ChatWireEvent::ToolCall {
                                    session_id: event_session_id.clone(),
                                    tool_id: params
                                        .get("toolId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    args: params.get("args").cloned(),
                                    risk: params
                                        .get("risk")
                                        .and_then(|r| r.as_str())
                                        .map(str::to_string),
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            // P11.5.11 — AG-UI live transport: forward the raw
                            // encoded envelope line to the UI (`agui-event` emit).
                            "agui/event" => {
                                if let Some(line) = params.get("line").and_then(|l| l.as_str()) {
                                    agui.forward(line);
                                } else if let Some(raw) = params.get("envelope") {
                                    agui.forward(&serde_json::to_string(raw).unwrap_or_default());
                                }
                            }
                            "chat/verification" => emit(
                                &on_event,
                                ChatWireEvent::Verification {
                                    stream_id,
                                    session_id: event_session_id.clone(),
                                    task_id: params
                                        .get("taskId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    checks: params
                                        .get("checks")
                                        .and_then(|c| c.as_array())
                                        .map(|arr| {
                                            arr.iter()
                                                .filter_map(|v| v.as_str().map(str::to_string))
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                    report: params
                                        .get("report")
                                        .and_then(|r| r.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    passed: params.get("passed").and_then(|p| p.as_bool()),
                                    metadata: event_metadata(&params),
                                },
                            ),
                            "chat/tool_result" => emit(
                                &on_event,
                                ChatWireEvent::ToolResult {
                                    session_id: event_session_id.clone(),
                                    tool_id: params
                                        .get("toolId")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    result: params.get("result").cloned(),
                                    error: params
                                        .get("error")
                                        .and_then(|e| e.as_str())
                                        .map(str::to_string)
                                        .or_else(|| {
                                            params.get("result").and_then(|r| {
                                                r.get("error")
                                                    .and_then(|e| e.as_str())
                                                    .map(str::to_string)
                                            })
                                        }),
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            "chat/done" => {
                                // Check the post-turn ledger before publishing a
                                // successful terminal event. Budget enforcement is
                                // terminal: the UI must never briefly mark a run
                                // completed and then replace it with a kill card.
                                let spent = if event_session_id.is_empty() {
                                    0.0
                                } else {
                                    vault
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .session_spend(&event_session_id)
                                        .unwrap_or(0.0)
                                };
                                if !event_session_id.is_empty()
                                    && spent >= DEFAULT_SESSION_BUDGET_USD
                                {
                                    emit(
                                        &on_event,
                                        ChatWireEvent::BudgetExceeded {
                                            stream_id: stream_id.clone(),
                                            session_id: event_session_id.clone(),
                                            limit: DEFAULT_SESSION_BUDGET_USD,
                                            spent,
                                            metadata: event_metadata(&params),
                                        },
                                    );
                                } else {
                                    emit(
                                        &on_event,
                                        ChatWireEvent::Done {
                                            session_id: event_session_id.clone(),
                                            turn_id: params
                                                .get("turnId")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                            full_text: params
                                                .get("fullText")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                            total_tokens: params
                                                .get("totalTokens")
                                                .and_then(|t| t.as_u64())
                                                .unwrap_or(0),
                                            stream_id: stream_id.clone(),
                                            metadata: event_metadata(&params),
                                        },
                                    );
                                }
                            }
                            "chat/error" => emit(
                                &on_event,
                                ChatWireEvent::Error {
                                    session_id: event_session_id.clone(),
                                    code: params
                                        .get("code")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("engine")
                                        .to_string(),
                                    message: params
                                        .get("message")
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    tool_id: params
                                        .get("toolId")
                                        .and_then(|t| t.as_str())
                                        .map(str::to_string),
                                    retryable: params.get("retryable").and_then(|r| r.as_bool()),
                                    args: params.get("args").cloned(),
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            "chat/cancelled" => emit(
                                &on_event,
                                ChatWireEvent::Cancelled {
                                    stream_id,
                                    session_id: event_session_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            // Stage-0 (P6.3): a plan executor circuit-break trip.
                            // The coordinator emits the full MCQ card payload;
                            // Rust relays it to the UI verbatim.
                            "chat/interrupt" => {
                                let plan_id = params
                                    .get("planId")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let break_id = params
                                    .get("breakId")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let title = params
                                    .get("title")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("Agent needs a decision")
                                    .to_string();
                                let description = params
                                    .get("description")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let options = params
                                    .get("options")
                                    .and_then(|v| v.as_array())
                                    .map(|a| {
                                        a.iter()
                                            .filter_map(|o| o.as_str().map(str::to_string))
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                emit(
                                    &on_event,
                                    ChatWireEvent::Interrupt {
                                        stream_id: stream_id.clone(),
                                        session_id: event_session_id.clone(),
                                        plan_id,
                                        break_id,
                                        title,
                                        description,
                                        options,
                                        metadata: event_metadata(&params),
                                    },
                                );
                            }
                            "chat/plan_done" => {
                                emit(
                                    &on_event,
                                    ChatWireEvent::PlanDone {
                                        session_id: event_session_id.clone(),
                                        plan_id: params
                                            .get("planId")
                                            .and_then(|s| s.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        tasks_done: params
                                            .get("tasksDone")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0)
                                            as u32,
                                        error: params
                                            .get("error")
                                            .and_then(|s| s.as_str())
                                            .map(str::to_string),
                                        stream_id,
                                        metadata: event_metadata(&params),
                                    },
                                );
                            }
                            "chat/plan_start" => emit(
                                &on_event,
                                ChatWireEvent::PlanStart {
                                    session_id: event_session_id.clone(),
                                    plan_id: params
                                        .get("planId")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    tasks: params.get("tasks").and_then(|v| v.as_u64()).unwrap_or(0)
                                        as u32,
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            "chat/plan_step" => emit(
                                &on_event,
                                ChatWireEvent::PlanStep {
                                    session_id: event_session_id.clone(),
                                    plan_id: params
                                        .get("planId")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    task_id: params
                                        .get("taskId")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    status: params
                                        .get("status")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            "chat/memory_extracted" => emit(
                                &on_event,
                                ChatWireEvent::MemoryExtracted {
                                    session_id: event_session_id.clone(),
                                    facts: params
                                        .get("facts")
                                        .and_then(|v| v.as_array())
                                        .map(|a| {
                                            a.iter()
                                                .filter_map(|v| v.as_str().map(str::to_string))
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            "chat/citations" => emit(
                                &on_event,
                                ChatWireEvent::Citations {
                                    session_id: event_session_id.clone(),
                                    citations: params
                                        .get("citations")
                                        .and_then(|c| c.as_array())
                                        .cloned()
                                        .unwrap_or_default(),
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            "chat/walkthrough" => emit(
                                &on_event,
                                ChatWireEvent::Walkthrough {
                                    session_id: event_session_id.clone(),
                                    stops: params
                                        .get("stops")
                                        .and_then(|c| c.as_array())
                                        .cloned()
                                        .unwrap_or_default(),
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            "chat/monitor" => emit(
                                &on_event,
                                ChatWireEvent::Monitor {
                                    session_id: event_session_id.clone(),
                                    job_id: params
                                        .get("jobId")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    changed: params
                                        .get("changed")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false),
                                    notified: params
                                        .get("notified")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false),
                                    stopped: params
                                        .get("stopped")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false),
                                    current: params
                                        .get("current")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    notifications: params
                                        .get("notifications")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32,
                                    stream_id,
                                    metadata: event_metadata(&params),
                                },
                            ),
                            _ => {}
                        }
                    }
                }
            }
        });
    }

    /// P71.3f — mount the one readiness source (the shell owns the runtime
    /// facts). Until this is called, every read is `AgentReadiness::Unknown`
    /// and the delegation/turn gates refuse external agents by name.
    pub fn mount_readiness(&self, source: Arc<dyn crate::tools::AgentReadinessSource>) -> &Self {
        *self.readiness.lock().unwrap_or_else(|e| e.into_inner()) = Some(source);
        self
    }

    /// P71.3f — the one readiness read (the relay's `agent/readiness` RPC and
    /// the turn gate both use it; one accessor, one state).
    pub fn agent_readiness(&self, agent_id: &str) -> agentcowork_types::AgentReadiness {
        crate::tools::read_agent_readiness(&self.readiness, agent_id)
    }

    /// Refuse a new prompt while the durable Work is waiting on recovery. An
    /// uncertain effect is never treated as a failed call and is never retried
    /// merely because a fresh process happened to start.
    pub fn reconcile_before_prompt(&self, session_id: &str) -> Result<(), ChatRelayError> {
        let gateway = self
            .work_gateway
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let work_id = gateway
            .get_work(session_id)
            .map(|address| address.work_id.as_str().to_string())
            .or_else(|| {
                gateway
                    .list_work()
                    .into_iter()
                    .find(|address| address.session_id.as_deref() == Some(session_id))
                    .map(|address| address.work_id.as_str().to_string())
            });
        let Some(work_id) = work_id else {
            return Ok(());
        };
        let Some(address) = gateway.get_work(&work_id) else {
            return Err(ChatRelayError::Recovery(format!(
                "Work `{work_id}` disappeared during recovery"
            )));
        };
        if matches!(
            address.session_kind,
            agentcowork_types::SessionKind::Automation
        ) && address.provenance.is_legacy()
        {
            return Err(ChatRelayError::Recovery(format!(
                "automation Work `{work_id}` has only legacy provenance; reconcile its admission before prompting"
            )));
        }
        if gateway.has_unresolved_uncertain_effects(&work_id) {
            return Err(ChatRelayError::Recovery(format!(
                "Work `{work_id}` has an unresolved uncertain effect; reconcile it before prompting"
            )));
        }
        let Some(run_id) = gateway.execution_id(&work_id) else {
            return Ok(());
        };
        let executions = self
            .executions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(work) = executions.get(run_id) else {
            return Err(ChatRelayError::Recovery(format!(
                "Work `{work_id}` points at missing Run `{run_id}`"
            )));
        };
        if work.state.is_terminal() && address.binding_id.is_some() {
            return Err(ChatRelayError::Recovery(format!(
                "Work `{work_id}` is terminal ({:?}); a new prompt cannot overwrite its bound Run",
                work.state
            )));
        }
        if matches!(
            work.state,
            crate::execution::ExecutionPhase::Recoverable
                | crate::execution::ExecutionPhase::Paused
                | crate::execution::ExecutionPhase::Checkpointed
                | crate::execution::ExecutionPhase::WaitingTool
                | crate::execution::ExecutionPhase::WaitingApproval
                | crate::execution::ExecutionPhase::WaitingUser
        ) {
            return Err(ChatRelayError::Recovery(format!(
                "Work `{work_id}` is {:?}; explicit reconciliation/resume is required",
                work.state
            )));
        }
        Ok(())
    }

    /// **J11** — the session budget pre-flight, **re-homed** by `P71.2c`.
    ///
    /// It used to run at the top of `start_stream`; that dispatch is deleted
    /// with the built-in engine, and the live turn moved to the ACP channel, so
    /// the refusal now happens where a turn is actually started
    /// (`src-tauri`'s `acp_prompt`, before the prompt reaches the agent). The
    /// rule is unchanged and deliberately fail-closed: a session at or over its
    /// hard budget refuses **before** anything is dispatched, with the J11
    /// "stopped: $X limit" surface the UI already renders.
    pub fn preflight_session_budget(&self, session_id: &str) -> Result<(), ChatRelayError> {
        self.reconcile_before_prompt(session_id)?;
        let spent = self
            .vault
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .session_spend(session_id)?;
        if spent >= DEFAULT_SESSION_BUDGET_USD {
            return Err(ChatRelayError::BudgetExceeded {
                session: session_id.to_string(),
                limit: DEFAULT_SESSION_BUDGET_USD,
                spent,
            });
        }
        Ok(())
    }
}

fn emit(on_event: &Arc<Mutex<EventSink>>, ev: ChatWireEvent) {
    on_event.lock().unwrap_or_else(|e| e.into_inner())(ev);
}

/// Run the broker for a coordinator `provider/stream` request and push the
/// deltas back as `chat/provider_chunk` notifications. Runs on its own thread;
/// keys never leave this process (the sidecar only sees chunk deltas).
#[cfg(test)]
mod tests {
    /// P64.6 — a preflight `root` that the coordinator left at `.` (it has no
    /// workspace) must resolve to the floored workspace, while an absolute root
    /// is honoured so a caller can still point the check at a package.
    #[test]
    fn preflight_root_resolves_against_the_workspace_floor() {
        let ws = std::path::Path::new("/srv/ws");
        let resolve = |params: serde_json::Value| {
            super::with_preflight_root(&params, ws)
                .get("root")
                .and_then(|v| v.as_str())
                .unwrap_or("<missing>")
                .to_string()
        };
        // The coordinator's default — and the case that staged the wrong tree.
        assert_eq!(resolve(serde_json::json!({ "root": "." })), "/srv/ws");
        // An omitted root is the same statement as `.`.
        assert_eq!(resolve(serde_json::json!({})), "/srv/ws");
        assert_eq!(resolve(serde_json::json!({ "root": "" })), "/srv/ws");
        // A relative sub-path floors under the workspace, like every file path.
        assert_eq!(
            resolve(serde_json::json!({ "root": "packages/coordinator" })),
            "/srv/ws/packages/coordinator"
        );
        // An absolute root is the caller's business, not ours.
        assert_eq!(
            resolve(serde_json::json!({ "root": "/other/repo" })),
            "/other/repo"
        );
        // Every other field survives the rewrite.
        let out = super::with_preflight_root(
            &serde_json::json!({ "id": "ex:1", "root": ".", "filesChanged": 2 }),
            ws,
        );
        assert_eq!(out.get("id").and_then(|v| v.as_str()), Some("ex:1"));
        assert_eq!(out.get("filesChanged").and_then(|v| v.as_u64()), Some(2));
    }

    /// P64.3 — the coordinator reads `symbol/kind/file/line/rank` out of
    /// `tags`; this pins that wire shape and the refusal of unknown methods.
    /// Fully qualified because the `use super::*` glob below is unix-gated.
    #[test]
    fn codeintel_rpc_serves_ranked_repo_map_tags() {
        let dir = std::env::temp_dir().join(format!("eaios-codeintel-rpc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("m.rs"), "fn alpha() {}\n").unwrap();

        let out = super::codeintel_rpc("codeintel/repomap", &serde_json::json!({}), &dir)
            .expect("codeintel/repomap is served");
        let tags = out
            .get("tags")
            .and_then(|t| t.as_array())
            .expect("tags array");
        let first = tags.first().expect("at least one tag");
        for key in ["symbol", "kind", "file", "line", "rank"] {
            assert!(first.get(key).is_some(), "missing {key} in the wire row");
        }
        assert!(
            tags.iter()
                .any(|t| t.get("symbol").and_then(|s| s.as_str()) == Some("alpha"))
        );

        // The old failure mode was a silent `method not found`; an unknown
        // method must still be an error rather than an empty tag list.
        assert!(super::codeintel_rpc("codeintel/nope", &serde_json::json!({}), &dir).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The previous failure mode was a silent `method not found`; an empty or
    /// unknown request must be a refusal, never a no-op that reads as success.
    #[test]
    fn skill_rpc_refuses_a_grow_without_a_task_name() {
        let (dir, _vault) = temp_vault("skill-rpc");
        let store = agentcowork_blueprint::SkillStore::new(dir.join("skills"));
        assert!(super::skill_rpc("skill/grow", &serde_json::json!({}), &store).is_err());
        assert!(
            super::skill_rpc(
                "skill/grow",
                &serde_json::json!({ "taskName": "   " }),
                &store
            )
            .is_err()
        );
        assert!(super::skill_rpc("skill/nope", &serde_json::json!({}), &store).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P51.28 — skill/warm_set hides disable-model-invocation from the model
    /// catalog and keeps it on the user slash list (Zed/Crush).
    #[test]
    fn skill_rpc_warm_set_omits_disabled_model_invocation() {
        let (dir, _vault) = temp_vault("skill-warm");
        let store = agentcowork_blueprint::SkillStore::new(dir.join("skills"));
        let notes = agentcowork_blueprint::Skill {
            manifest: agentcowork_blueprint::SkillManifest {
                name: "notes".into(),
                description: "Take notes".into(),
                author: "t".into(),
                created: "2026-09-18".into(),
                version: "1".into(),
                ..Default::default()
            },
            body: "Take notes.".into(),
        };
        let deploy = agentcowork_blueprint::Skill {
            manifest: agentcowork_blueprint::SkillManifest {
                name: "deploy".into(),
                description: "Deploy the branch".into(),
                author: "t".into(),
                created: "2026-09-18".into(),
                version: "1".into(),
                user_invocable: true,
                disable_model_invocation: true,
                ..Default::default()
            },
            body: "Deploy.".into(),
        };
        store.save(&notes, true).expect("save notes");
        store.save(&deploy, true).expect("save deploy");
        let warm = super::skill_rpc("skill/warm_set", &serde_json::json!({}), &store).unwrap();
        let skills = warm["skills"].as_array().expect("skills");
        let joined: Vec<&str> = skills.iter().filter_map(|v| v.as_str()).collect();
        assert!(joined.iter().any(|s| s.starts_with("notes:")));
        assert!(!joined.iter().any(|s| s.starts_with("deploy:")));
        let slash = warm["slash"].as_array().expect("slash");
        assert!(slash.iter().any(|v| v.as_str() == Some("deploy")));
        let auto = super::skill_rpc(
            "skill/compose",
            &serde_json::json!({ "stack": ["deploy"], "query": "deploy" }),
            &store,
        )
        .unwrap();
        assert!(!auto["rejected"].as_array().unwrap().is_empty());
        let user = super::skill_rpc(
            "skill/compose",
            &serde_json::json!({
                "stack": ["deploy"],
                "query": "deploy",
                "userExplicit": true
            }),
            &store,
        )
        .unwrap();
        assert!(user["rejected"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P64.4/P71.3a — the spawn seam must enforce the shared limits against the
    /// Work graph, and must report an admission rather than a fabricated
    /// completion. There is no runtime left to hold the counts: the child Work
    /// *is* the record.
    #[test]
    fn subagent_rpc_admits_a_spawn_and_enforces_the_limits() {
        let policy = agentcowork_blueprint::DelegationPolicy::new(
            agentcowork_blueprint::SubAgentLimits::default(),
        );
        let gw = Arc::new(Mutex::new(crate::work_gateway::WorkGateway::new()));
        gw.lock()
            .unwrap()
            .create_work("w-parent", None, Some("s-1".into()), "parent objective");
        let out = super::subagent_rpc(
            "subagent/spawn",
            &serde_json::json!({
                "spec": { "id": "t1", "goal": "do the thing", "context": [], "acceptance": [] },
                "model": "m",
                "harness": "claude-code",
                "workspace": ".everyaios/worktrees/task-t1",
                "parentId": null,
                "tools": ["todo"],
                "blockedTools": [],
                "workId": "w-parent",
            }),
            &policy,
            &gw,
            agentcowork_types::AgentReadiness::Ready,
        )
        .expect("an in-limits spawn is admitted");
        assert_eq!(out["task_id"], "t1");
        // The LLM execution is coordinator-side, so this is an admission.
        assert_eq!(out["status"], "running");
        assert_eq!(out["accountedFrom"], "work_graph");
        // P64.4 — the derived grant set is reported, not just stored.
        assert_eq!(out["tools"][0], "todo");
        assert_eq!(out["depth"], 1);
        // The accounting is the graph, not a counter: one delegated child.
        assert_eq!(
            gw.lock()
                .unwrap()
                .delegation_gauge("w-parent")
                .unwrap()
                .total,
            1
        );
        // P69.D14 — the spawn is a child Work with a parent link (plus its own
        // Run and ephemeral session), not a parallel runtime.
        let child = out["workId"].as_str().unwrap().to_string();
        assert_eq!(child, "w-parent/subagent/t1");
        {
            let gw = gw.lock().unwrap();
            let kids = gw.children_of("w-parent");
            assert_eq!(kids.len(), 1);
            assert_eq!(kids[0].parent_work_id.as_deref(), Some("w-parent"));
            assert_eq!(gw.agent_sessions_for(&child).len(), 1);
            // The child is running, so it is an active delegation.
            let gauge = gw.delegation_gauge("w-parent").unwrap();
            assert_eq!((gauge.active, gauge.total, gauge.child_depth), (1, 1, 1));
        }

        // A duplicate task id is refused from the graph (deterministic child
        // Work id), not silently accepted.
        assert!(
            super::subagent_rpc(
                "subagent/spawn",
                &serde_json::json!({
                    "spec": { "id": "t1", "goal": "again", "context": [], "acceptance": [] },
                    "workId": "w-parent",
                }),
                &policy,
                &gw,
                agentcowork_types::AgentReadiness::Ready,
            )
            .is_err()
        );
        // Completion closes the child's own timeline (terminal Run event +
        // ephemeral session termination) and reports a summary-only result.
        let done = super::subagent_rpc(
            "subagent/complete",
            &serde_json::json!({
                "taskId": "t1",
                "summary": "did the thing",
                "artifacts": ["src/a.rs"],
                "workId": "w-parent",
            }),
            &policy,
            &gw,
            agentcowork_types::AgentReadiness::Ready,
        )
        .expect("completion of a delegated task is accepted");
        assert_eq!(done["summary"], "did the thing");
        assert_eq!(done["artifacts"][0], "src/a.rs");
        assert_eq!(done["workId"], "w-parent/subagent/t1");
        {
            let gw = gw.lock().unwrap();
            let sessions = gw.agent_sessions_for(&child);
            assert_eq!(sessions[0].runtime_state, "terminated");
            // Terminal children free concurrency but stay counted in total.
            let gauge = gw.delegation_gauge("w-parent").unwrap();
            assert_eq!((gauge.active, gauge.total), (0, 1));
        }
        // Closing a task the graph never saw is an error, not a summary.
        assert!(
            super::subagent_rpc(
                "subagent/complete",
                &serde_json::json!({ "taskId": "ghost", "workId": "w-parent" }),
                &policy,
                &gw,
                agentcowork_types::AgentReadiness::Ready,
            )
            .is_err()
        );
        // A spawn that names no parent Work is admitted policy-only, and says so.
        let loose = super::subagent_rpc(
            "subagent/spawn",
            &serde_json::json!({
                "spec": { "id": "t2", "goal": "loose", "context": [], "acceptance": [] },
                "depth": 1,
            }),
            &policy,
            &gw,
            agentcowork_types::AgentReadiness::Ready,
        )
        .expect("a parentless spawn is admitted policy-only");
        assert_eq!(loose["accountedFrom"], "policy_only");
        assert!(loose["workId"].is_null());
        // A spec-less call is a refusal, and unknown methods stay errors.
        assert!(
            super::subagent_rpc(
                "subagent/spawn",
                &serde_json::json!({}),
                &policy,
                &gw,
                agentcowork_types::AgentReadiness::Ready,
            )
            .is_err()
        );
        assert!(
            super::subagent_rpc(
                "subagent/nope",
                &serde_json::json!({}),
                &policy,
                &gw,
                agentcowork_types::AgentReadiness::Ready,
            )
            .is_err()
        );
    }

    /// P64.4 — the spawn reply carries the execution binding: the spec-only
    /// starting prompt (fresh context) plus the isolated workspace provision
    /// planned against the minted child Work. A policy-only spawn (no parent
    /// Work) still starts spec-only but names no workspace.
    #[test]
    fn subagent_spawn_reply_carries_spec_only_start_and_worktree_provision() {
        let policy = agentcowork_blueprint::DelegationPolicy::new(
            agentcowork_blueprint::SubAgentLimits::default(),
        );
        let gw = Arc::new(Mutex::new(crate::work_gateway::WorkGateway::new()));
        gw.lock()
            .unwrap()
            .create_work("w-parent", None, Some("s-1".into()), "parent objective");
        let out = super::subagent_rpc(
            "subagent/spawn",
            &serde_json::json!({
                "spec": { "id": "t9", "goal": "write the health endpoint", "context": [], "acceptance": [] },
                "model": "m",
                "harness": "claude-code",
                "workId": "w-parent",
            }),
            &policy,
            &gw,
            agentcowork_types::AgentReadiness::Ready,
        )
        .expect("an in-limits spawn is admitted");
        // Fresh context: the child's start is its spec only — the goal is in,
        // the parent's objective and history are not.
        let prompt = out["startingPrompt"].as_str().unwrap().to_string();
        assert!(
            prompt.contains("write the health endpoint"),
            "got: {prompt}"
        );
        assert!(
            !prompt.contains("parent objective"),
            "the parent transcript must never be handed down, got: {prompt}"
        );
        // Worktree isolation address: segment + branch + 3-file blackboard.
        assert!(out["provisionError"].is_null());
        assert_eq!(
            out["provision"]["worktree_segment"],
            ".everyaios/worktrees/task-t9"
        );
        assert_eq!(out["provision"]["branch"], "subtask/t9");
        assert_eq!(
            out["provision"]["plan_file"],
            ".everyaios/worktrees/task-t9/.everyaios/task_plan.md"
        );
        assert_eq!(
            out["provision"]["findings_file"],
            ".everyaios/worktrees/task-t9/.everyaios/findings.md"
        );
        assert_eq!(
            out["provision"]["receipts_dir"],
            ".everyaios/worktrees/task-t9/.everyaios/receipts"
        );
        // Policy-only: spec-only start, but no workspace address without a
        // child Work.
        let loose = super::subagent_rpc(
            "subagent/spawn",
            &serde_json::json!({
                "spec": { "id": "t10", "goal": "loose", "context": [], "acceptance": [] },
                "depth": 1,
            }),
            &policy,
            &gw,
            agentcowork_types::AgentReadiness::Ready,
        )
        .expect("a parentless spawn is admitted policy-only");
        assert!(loose["startingPrompt"].as_str().unwrap().contains("loose"));
        assert!(loose["provision"].is_null());
    }

    #[test]
    fn subagent_spawn_refuses_an_unready_member_by_name() {
        // P71.3f — installed is not ready: the delegation gate refuses the
        // member and names the state, instead of spawning into a failure.
        let policy = agentcowork_blueprint::DelegationPolicy::new(
            agentcowork_blueprint::SubAgentLimits::default(),
        );
        let gw = Arc::new(Mutex::new(crate::work_gateway::WorkGateway::new()));
        gw.lock()
            .unwrap()
            .create_work("w-parent", None, Some("s-1".into()), "parent objective");
        let err = super::subagent_rpc(
            "subagent/spawn",
            &serde_json::json!({
                "spec": { "id": "t1", "goal": "do the thing", "context": [], "acceptance": [] },
                "agentId": "claude",
                "harness": "acp",
                "workId": "w-parent",
            }),
            &policy,
            &gw,
            agentcowork_types::AgentReadiness::AuthRequired,
        )
        .expect_err("an auth-required member must not be admitted");
        assert!(err.contains("claude"), "got: {err}");
        assert!(err.contains("auth_required"), "got: {err}");
        // Nothing was minted: the graph holds no delegated child.
        assert!(gw.lock().unwrap().children_of("w-parent").is_empty());
    }

    #[test]
    fn subagent_rpc_scout_strips_writes() {
        let policy = agentcowork_blueprint::DelegationPolicy::new(
            agentcowork_blueprint::SubAgentLimits::default(),
        );
        let gw = Arc::new(Mutex::new(crate::work_gateway::WorkGateway::new()));
        let out = super::subagent_rpc(
            "subagent/spawn",
            &serde_json::json!({
                "spec": { "id": "scout-1", "goal": "map", "context": [], "acceptance": [] },
                "model": "m",
                "harness": "claude-code",
                "workspace": ".everyaios/worktrees/task-scout-1",
                "parentId": "root",
                "tools": ["file_ops.read", "file_ops.write", "search.query", "desktop.act"],
                "blockedTools": [],
                "role": "scout",
            }),
            &policy,
            &gw,
            agentcowork_types::AgentReadiness::Ready,
        )
        .expect("scout spawn is admitted");
        assert_eq!(out["role"], "scout");
        let granted: Vec<&str> = out["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(granted.contains(&"file_ops.read"));
        assert!(granted.contains(&"search.query"));
        assert!(
            !granted
                .iter()
                .any(|t| *t == "file_ops.write" || *t == "desktop.act")
        );
        assert_eq!(out["planes"], 5);
        assert_eq!(out["harness"], "claude-code");
        assert_eq!(out["binding"]["model"], "m");
    }

    #[test]
    fn subagent_rpc_refuses_cli_named_subagent_harness() {
        let policy = agentcowork_blueprint::DelegationPolicy::new(
            agentcowork_blueprint::SubAgentLimits::default(),
        );
        let gw = Arc::new(Mutex::new(crate::work_gateway::WorkGateway::new()));
        let err = super::subagent_rpc(
            "subagent/spawn",
            &serde_json::json!({
                "spec": { "id": "x", "goal": "g", "context": [], "acceptance": [] },
                "model": "opus",
                "workspace": ".",
                "parentId": "root",
                "harness": "claude-subagent",
                "tools": [],
                "blockedTools": [],
            }),
            &policy,
            &gw,
            agentcowork_types::AgentReadiness::Ready,
        );
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("CLI-named"));
    }

    // P71.2c — the broker tests' `Read`/`Write`/`TcpListener`/`KeySpec`/
    // `KeyStatus` imports went with the fake provider endpoint they served.
    #[cfg(unix)]
    use super::*;
    #[cfg(unix)]
    use std::os::unix::net::UnixStream;
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    #[cfg(unix)]
    use agentcowork_ipc::frame;
    #[cfg(unix)]
    use agentcowork_vault::{Usage, UsageRow};

    #[cfg(unix)]
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "agentcowork-core-chat-{tag}-{}",
            std::process::id()
        ))
    }

    #[cfg(unix)]
    fn temp_vault(tag: &str) -> (std::path::PathBuf, Vault) {
        let dir = temp_dir(tag);
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("vault.db");
        let vault = Vault::open(&path, "test-key").expect("open vault");
        (dir, vault)
    }

    #[cfg(unix)]
    fn pair() -> (UnixStream, UnixStream) {
        UnixStream::pair().expect("socketpair")
    }

    #[cfg(unix)]
    fn link_from(a: UnixStream) -> SidecarLink<UnixStream, UnixStream> {
        let reader = a.try_clone().expect("clone");
        SidecarLink::new(a, reader)
    }

    // P71.2c — the `spec` / `mock_server` helpers that stood up a fake
    // OpenAI-compatible endpoint for the provider-broker tests were deleted with
    // the broker itself (ADR-0005 §2). Nothing in this crate dials a provider
    // any more; the vault's own broker tests keep their own fixtures.

    #[cfg(unix)]
    fn wait_events(events: &Arc<Mutex<Vec<ChatWireEvent>>>, min: usize, timeout: Duration) -> bool {
        let start = Instant::now();
        loop {
            {
                let guard = events.lock().unwrap_or_else(|e| e.into_inner());
                if guard.len() >= min {
                    return true;
                }
            }
            if start.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[test]
    fn session_budget_preflight_refuses_over_limit() {
        // J11 — a session already at/over its $ limit is refused BEFORE anything
        // is dispatched, with the "stopped: $X limit" surface. `P71.2c` re-homed
        // this check from the deleted `start_stream` to `preflight_session_budget`,
        // which is what the live ACP turn path (`src-tauri::acp_prompt`) calls.
        let (dir, vault) = temp_vault("preflight");
        vault
            .record_usage(&UsageRow {
                session: "s-over".into(),
                provider: "nvidia".into(),
                model: "m".into(),
                key_id: "k".into(),
                usage: Usage::default(),
                cost: 2.50,
                tool: None,
                task_id: String::new(),
                run_id: String::new(),
                work_id: String::new(),
            })
            .unwrap();
        let vault = Arc::new(Mutex::new(vault));
        let (a, _b) = pair();
        let relay = ChatRelay::new(link_from(a), vault, |_| {});

        let err = relay.preflight_session_budget("s-over").unwrap_err();
        let msg = err.to_string();
        match err {
            ChatRelayError::BudgetExceeded {
                session,
                limit,
                spent,
            } => {
                assert_eq!(session, "s-over");
                assert_eq!(limit, DEFAULT_SESSION_BUDGET_USD);
                assert!(spent >= limit);
                assert!(msg.contains("stopped:"), "msg: {msg}");
            }
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
        // A session under the limit passes.
        assert!(relay.preflight_session_budget("s-clear").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn relay_forwards_chat_events() {
        // `P71.2c` — the plane is now **receive-only**: the producer (the
        // built-in turn) is deleted, so this drives the notifications straight
        // off the link, which is what the arms under test read. Nothing in Rust
        // asks the sidecar to send these any more; the arms exist for the
        // projection `P71.9c` wires the ACP live updates into.
        let (a, b) = pair();
        let side = std::thread::spawn(move || {
            let mut s = b;
            let n = serde_json::json!({
                "jsonrpc": "2.0", "method": "chat/batch",
                "params": { "streamId": "st-1", "sessionId": "s1", "text": "hi", "tokenCount": 1 },
            });
            let _ = frame::write_frame(&mut s, &serde_json::to_vec(&n).unwrap());
            let d = serde_json::json!({
                "jsonrpc": "2.0", "method": "chat/done",
                "params": { "streamId": "st-1", "sessionId": "s1", "turnId": "s1:1", "fullText": "hi", "totalTokens": 1 },
            });
            let _ = frame::write_frame(&mut s, &serde_json::to_vec(&d).unwrap());
            std::thread::sleep(Duration::from_millis(200));
        });

        let (_dir, vault) = temp_vault("forward");
        let vault = Arc::new(Mutex::new(vault));
        let events: Arc<Mutex<Vec<ChatWireEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let ev = Arc::clone(&events);
        let relay = ChatRelay::new(link_from(a), vault, move |e| {
            ev.lock().unwrap_or_else(|x| x.into_inner()).push(e);
        });
        relay.spawn();

        assert!(
            wait_events(&events, 2, Duration::from_secs(5)),
            "expected Batch+Done events, got {:?}",
            events.lock().unwrap_or_else(|x| x.into_inner())
        );
        let evs = events.lock().unwrap_or_else(|x| x.into_inner());
        assert!(matches!(evs[0], ChatWireEvent::Batch { ref text, .. } if text == "hi"));
        assert!(matches!(evs[1], ChatWireEvent::Done { ref turn_id, .. } if turn_id == "s1:1"));
        assert!(
            !evs.iter()
                .any(|e| matches!(e, ChatWireEvent::BudgetExceeded { .. }))
        );
        drop(evs);
        side.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn relay_forwards_plan_interrupt_notifications() {
        // Stage-0 (P6.3): a `chat/interrupt` notification arrives as a
        // ChatWireEvent::Interrupt — the H2 MCQ card payload. `P71.2c` deleted
        // the plan executor that *emitted* it (`start_plan`/`plan/execute`) with
        // the built-in engine, so the notification is written straight onto the
        // link: the arm under test is the receive half, which the cockpit still
        // renders when an interrupt arrives (the resolution path is
        // `control::interrupt_response`).
        let (a, b) = pair();
        let side = std::thread::spawn(move || {
            let mut s = b;
            let n = serde_json::json!({
                "jsonrpc": "2.0", "method": "chat/interrupt",
                "params": {
                    "streamId": "st-1", "sessionId": "s1", "planId": "p1", "breakId": "b1",
                    "title": "Loop detected (3× repeat)",
                    "description": "The agent repeated the same tool call 3 times.",
                    "options": ["skip", "retry", "escalate", "takeover"],
                },
            });
            let _ = frame::write_frame(&mut s, &serde_json::to_vec(&n).unwrap());
            let d = serde_json::json!({
                "jsonrpc": "2.0", "method": "chat/plan_done",
                "params": { "streamId": "st-1", "sessionId": "s1", "planId": "p1", "tasksDone": 2 },
            });
            let _ = frame::write_frame(&mut s, &serde_json::to_vec(&d).unwrap());
            std::thread::sleep(Duration::from_millis(200));
        });

        let (_dir, vault) = temp_vault("interrupt");
        let vault = Arc::new(Mutex::new(vault));
        let events: Arc<Mutex<Vec<ChatWireEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let ev = Arc::clone(&events);
        let relay = ChatRelay::new(link_from(a), vault, move |e| {
            ev.lock().unwrap_or_else(|x| x.into_inner()).push(e);
        });
        relay.spawn();

        assert!(
            wait_events(&events, 2, Duration::from_secs(5)),
            "expected Interrupt+PlanDone, got {:?}",
            events.lock().unwrap_or_else(|x| x.into_inner())
        );
        let evs = events.lock().unwrap_or_else(|x| x.into_inner());
        match &evs[0] {
            ChatWireEvent::Interrupt {
                plan_id,
                break_id,
                options,
                title,
                ..
            } => {
                assert_eq!(plan_id, "p1");
                assert_eq!(break_id, "b1");
                assert!(title.contains("Loop detected"));
                assert_eq!(options.len(), 4);
            }
            other => panic!("expected Interrupt, got {other:?}"),
        }
        assert!(
            matches!(evs[1], ChatWireEvent::PlanDone { .. }),
            "expected PlanDone second, got {:?}",
            evs[1]
        );
        drop(evs);
        side.join().unwrap();
        let _ = std::fs::remove_dir_all(&_dir);
    }

    #[test]
    fn capability_invocation_metadata_is_secret_free_and_validated() {
        let invocation = agentcowork_guard::CapabilityInvocation {
            grant_id: "grant:1".into(),
            run_id: "run-1".into(),
            capability: "connector:gmail.read".into(),
            operation: "list".into(),
        };
        assert!(invocation.validate().is_ok());
        let encoded = serde_json::to_string(&invocation).unwrap();
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("Bearer"));
    }

    #[cfg(unix)]
    #[test]
    fn post_turn_budget_kill_surfaces_stopped() {
        // J11 end-to-end: the durable ledger is what the post-turn check reads,
        // so a session that crosses its limit during a turn is stopped
        // terminally. `P71.2c` deleted the broker that used to record the turn's
        // cost here, so the ledger rows are written directly — the arm under
        // test (`chat/done` → post-turn ledger check → BudgetExceeded before
        // `done`) is unchanged, and the pre-flight half is asserted through
        // `preflight_session_budget`, which is where the live ACP turn path
        // calls it.
        let (dir, vault) = temp_vault("kill");
        for cost in [1.99_f64, 0.02] {
            vault
                .record_usage(&UsageRow {
                    session: "s-kill".into(),
                    provider: "nvidia".into(),
                    model: "m".into(),
                    key_id: "k".into(),
                    usage: Usage::default(),
                    cost,
                    tool: None,
                    task_id: String::new(),
                    run_id: String::new(),
                    work_id: String::new(),
                })
                .unwrap();
        }
        let vault = Arc::new(Mutex::new(vault));

        let (a, b) = pair();
        let events: Arc<Mutex<Vec<ChatWireEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let ev = Arc::clone(&events);
        let relay = ChatRelay::new(link_from(a), vault, move |e| {
            ev.lock().unwrap_or_else(|x| x.into_inner()).push(e);
        });
        relay.spawn();

        let side = std::thread::spawn(move || {
            let mut s = b;
            let d = serde_json::json!({
                "jsonrpc": "2.0", "method": "chat/done",
                "params": { "streamId": "st-1", "sessionId": "s-kill", "turnId": "s-kill:1", "fullText": "x", "totalTokens": 1 },
            });
            let _ = frame::write_frame(&mut s, &serde_json::to_vec(&d).unwrap());
            std::thread::sleep(Duration::from_millis(300));
        });

        assert!(
            wait_events(&events, 1, Duration::from_secs(5)),
            "expected BudgetExceeded, got {:?}",
            events.lock().unwrap_or_else(|x| x.into_inner())
        );
        let evs = events.lock().unwrap_or_else(|x| x.into_inner());
        // Budget enforcement is terminal and is emitted before `done`, so a
        // queued follow-up cannot observe a transient completed state.
        assert!(matches!(
            evs[0],
            ChatWireEvent::BudgetExceeded { ref session_id, spent, .. }
                if session_id == "s-kill" && spent >= 2.01
        ));
        assert!(!evs.iter().any(|e| matches!(e, ChatWireEvent::Done { .. })));
        // The next turn is refused at pre-flight.
        assert!(matches!(
            relay.preflight_session_budget("s-kill"),
            Err(ChatRelayError::BudgetExceeded { .. })
        ));
        drop(evs);
        side.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn relay_dispatches_scheduler_requests() {
        // P6.4: `scheduler/*` requests from the coordinator hit the shared
        // SchedulerService; the same job state is visible via the relay handle.
        let (a, b) = pair();
        let side = std::thread::spawn(move || {
            // Coordinator role: issue upsert + due to Rust, drain the acks.
            let mut s = b;
            let up = serde_json::json!({
                "jsonrpc": "2.0", "id": "u1", "method": "scheduler/upsert",
                "params": {
                    "id": "j1", "name": "probe", "sessionId": "s1",
                    "trigger": { "type": "interval", "secs": 60 },
                    "steps": [],
                    "now": 1_750_000_000,
                },
            });
            let _ = frame::write_frame(&mut s, &serde_json::to_vec(&up).unwrap());
            let due = serde_json::json!({
                "jsonrpc": "2.0", "id": "d1", "method": "scheduler/due",
                "params": { "now": 1_750_000_061 },
            });
            let _ = frame::write_frame(&mut s, &serde_json::to_vec(&due).unwrap());
            let mut acks = Vec::new();
            while let Ok(Some(payload)) = frame::decode(&mut s) {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&payload) {
                    if v.get("id").is_some() {
                        acks.push(v);
                    }
                    if acks.len() == 2 {
                        break;
                    }
                }
            }
            acks
        });
        let (dir, vault) = temp_vault("scheduler");
        let vault = Arc::new(Mutex::new(vault));
        let mut relay = ChatRelay::new(link_from(a), vault, |_| {});
        // Test isolation: `ChatRelay::new` loads the *developer's* real
        // `<data_dir>/scheduler.json`, so a job left there by an earlier run
        // (or by the app itself) decided the `due` result and made this test
        // depend on machine state. Re-seat the service on an empty temp file so
        // the assertion is about the dispatched job, not the disk.
        relay.scheduler = Arc::new(Mutex::new(SchedulerService::load_or_new(
            dir.join("scheduler.json"),
        )));
        relay.spawn();

        // The sidecar thread drives the protocol — the relay just needs to
        // be alive; the ack content is asserted on the coordinator side.
        let acks = side.join().unwrap();
        assert_eq!(acks.len(), 2);
        let up_ack = &acks[0];
        assert_eq!(up_ack["result"]["ok"], serde_json::json!(true));
        let due_ack = &acks[1];
        assert_eq!(due_ack["result"]["due"], serde_json::json!(["j1"]));
    }

    /// P64.3/P64.4/P64.5/P64.8 — the *native plane* seams driven end to end.
    ///
    /// The lane's earlier tests called the RPC helpers directly, which cannot
    /// distinguish "the arm is mounted" from "the helper works" — exactly how
    /// `codeintel/repomap`, `subagent/spawn` and `skill/grow` sat unhandled
    /// behind a catch-all while every helper-level test stayed green. This test
    /// pushes real JSON-RPC frames through `ChatRelay::spawn()` and asserts on
    /// the replies the coordinator actually receives, so a missing arm fails
    /// here rather than silently in production.
    #[cfg(unix)]
    #[test]
    fn relay_dispatches_native_plane_requests() {
        let (a, b) = pair();
        // The client half is interactive: `execution/record_edit` and
        // `execution/record_preflight` can only target an execution that
        // really exists, so the id is read back from the `begin` ack first.
        let side = std::thread::spawn(move || {
            let mut s = b;
            let mut call = |id: &str, method: &str, params: serde_json::Value| {
                let v = serde_json::json!({
                    "jsonrpc": "2.0", "id": id, "method": method, "params": params,
                });
                let _ = frame::write_frame(&mut s, &serde_json::to_vec(&v).unwrap());
                loop {
                    match frame::decode(&mut s) {
                        Ok(Some(payload)) => {
                            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&payload) {
                                if v.get("id").and_then(|i| i.as_str()) == Some(id) {
                                    return v;
                                }
                            }
                        }
                        _ => panic!("relay closed the link before acking {method}"),
                    }
                }
            };

            let repomap = call(
                "r1",
                "codeintel/repomap",
                serde_json::json!({ "maxFiles": 50 }),
            );
            let grown = call(
                "g1",
                "skill/grow",
                serde_json::json!({
                    "taskName": "Frame Dispatch Probe",
                    "solution": "1. Do the thing.\n2. Verify it.",
                    "author": "test",
                    "version": "0.1.0",
                }),
            );
            let spawned = call(
                "s1",
                "subagent/spawn",
                serde_json::json!({
                    "spec": { "id": "t-frame", "goal": "probe", "context": [], "acceptance": [] },
                    "model": "m",
                    "harness": "claude-code",
                    "workspace": ".everyaios/worktrees/task-t-frame",
                    "parentId": null,
                    "tools": ["todo"],
                    "blockedTools": [],
                    "depth": 1,
                }),
            );
            let began = call(
                "b1",
                "execution/begin",
                serde_json::json!({ "trigger": "chat", "sessionId": "s-frame", "objective": "probe" }),
            );
            let ex_id = began["result"]["id"]
                .as_str()
                .expect("begin returns the execution id")
                .to_string();

            // Guard-2 gate: a receipt without a ticket is refused, and a
            // receipt for an unknown execution is refused too.
            let no_ticket = call(
                "e0",
                "execution/record_edit",
                serde_json::json!({ "id": ex_id, "strategy": "exact", "path": "src/a.rs", "ticketId": "", "auditSeq": 1 }),
            );
            let unknown = call(
                "e1",
                "execution/record_edit",
                serde_json::json!({ "id": "nosuch", "strategy": "exact", "path": "src/a.rs", "ticketId": "guard-ticket-1", "auditSeq": 2 }),
            );
            let edit = call(
                "e2",
                "execution/record_edit",
                serde_json::json!({ "id": ex_id, "strategy": "exact", "path": "src/a.rs", "ticketId": "guard-ticket-1", "auditSeq": 7 }),
            );
            let pre = call(
                "p1",
                "execution/record_preflight",
                serde_json::json!({ "id": ex_id, "passed": true, "output": "tsc clean" }),
            );
            (
                repomap, grown, spawned, began, no_ticket, unknown, edit, pre,
            )
        });

        let (dir, vault) = temp_vault("native-plane");
        let vault = Arc::new(Mutex::new(vault));
        let mut relay = ChatRelay::new(link_from(a), vault, |_| {});
        // P71.3f — the spawn gate reads mounted readiness; without a source
        // every agent is `Unknown` and the member is refused. The test mounts
        // the facts directly (the shell does this in production).
        struct ReadyEverywhere;
        impl crate::tools::AgentReadinessSource for ReadyEverywhere {
            fn readiness(&self, _agent_id: &str) -> agentcowork_types::AgentReadiness {
                agentcowork_types::AgentReadiness::Ready
            }
        }
        relay.mount_readiness(Arc::new(ReadyEverywhere));
        // Isolation: the defaults are the developer's real data dir and
        // `~/.everyaios/skills`. Point both at the temp tree so the assertions
        // are about the dispatch, and no frame can write to the real home.
        relay.skill_store = Arc::new(Mutex::new(agentcowork_blueprint::SkillStore::new(
            dir.join("skills"),
        )));
        let workspace = dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("m.rs"), "fn alpha() {}\n").unwrap();
        {
            let mut svc = ToolService::new_with_egress(
                Arc::clone(&relay.guard),
                workspace,
                Arc::clone(&relay.egress),
            );
            svc.attach_capability_broker(Arc::clone(&relay.capabilities));
            relay.tools = Arc::new(Mutex::new(svc));
        }
        relay.spawn();

        let (repomap, grown, spawned, began, no_ticket, unknown, edit, pre) = side.join().unwrap();

        // P64.3 — a real map over the temp workspace, in the wire shape the
        // coordinator reads. `method not found` used to arrive as this.
        assert!(repomap.get("error").is_none(), "repomap: {repomap}");
        let tags = repomap["result"]["tags"].as_array().expect("tags array");
        assert!(
            tags.iter()
                .any(|t| t["symbol"] == serde_json::json!("alpha")),
            "repo map did not surface the workspace symbol: {repomap}"
        );

        // P64.8 — distillation really reached the (temp) store.
        assert!(grown.get("error").is_none(), "skill/grow: {grown}");
        assert_eq!(grown["result"]["ok"], serde_json::json!(true));
        assert!(
            dir.join("skills")
                .join("frame-dispatch-probe")
                .join("SKILL.md")
                .exists()
        );

        // P64.4 — an admission reported as running, never a fabricated done.
        assert!(spawned.get("error").is_none(), "subagent/spawn: {spawned}");
        assert_eq!(spawned["result"]["task_id"], serde_json::json!("t-frame"));
        assert_eq!(spawned["result"]["status"], serde_json::json!("running"));

        // P64.5 — receipts attach only behind a real Guard-2 ticket.
        assert!(began["result"]["id"].is_string(), "begin: {began}");
        assert!(
            no_ticket.get("error").is_some(),
            "ticketless edit must fail"
        );
        assert!(
            unknown.get("error").is_some(),
            "unknown execution must fail"
        );
        assert!(edit.get("error").is_none(), "verified edit: {edit}");
        assert_eq!(edit["result"]["strategy"], serde_json::json!("exact"));
        assert_eq!(
            edit["result"]["ticketId"],
            serde_json::json!("guard-ticket-1")
        );

        // P64.6 — the preflight receipt records its verdict.
        assert!(pre.get("error").is_none(), "preflight: {pre}");
        assert_eq!(pre["result"]["passed"], serde_json::json!(true));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stand-in plane for the *arm* tests.
    ///
    /// The arm's job is to carry the observer's rows onto the wire and to be
    /// honest about a host with no plane, so a deterministic observer is the
    /// right instrument — the real `PtyHost` read models are covered against a
    /// real spawned shell in `terminal::tests`.
    struct FakePlane;

    impl crate::terminal::TerminalPlaneObserver for FakePlane {
        fn plane_status(&self) -> crate::terminal::TerminalPlaneStatus {
            crate::terminal::TerminalPlaneStatus {
                attached: true,
                count: 1,
                ptys: vec![crate::terminal::TerminalSessionView {
                    pty_id: "pty-1".into(),
                    profile_id: "bash".into(),
                    backend: crate::terminal::TerminalBackend::Local,
                    origin: crate::terminal::TerminalOrigin::Agent,
                    label: Some("script.run".into()),
                    integration: Some("Rich"),
                    cwd: "/w".into(),
                    pid: Some(42),
                    running: true,
                    exit_code: None,
                }],
            }
        }

        fn session(&self, pty_id: &str) -> Option<crate::terminal::TerminalSessionView> {
            self.plane_status()
                .ptys
                .into_iter()
                .find(|s| s.pty_id == pty_id)
        }

        fn commands(
            &self,
            pty_id: &str,
            _limit: usize,
        ) -> Result<Vec<crate::terminal::TerminalCommandView>, String> {
            if pty_id != "pty-1" {
                return Err(format!("no such pty: {pty_id}"));
            }
            Ok(vec![crate::terminal::TerminalCommandView {
                command: "cargo test -p agentcowork-core".into(),
                cwd: "/w".into(),
                exit_code: Some(0),
                output: "2593 passed".into(),
                trusted: true,
                failed: false,
            }])
        }

        fn last_command(&self, pty_id: &str, _max_chars: usize) -> Result<Option<String>, String> {
            if pty_id != "pty-1" {
                return Err(format!("no such pty: {pty_id}"));
            }
            Ok(Some("$ cargo test -p agentcowork-core\nexit 0".into()))
        }

        fn history(
            &self,
            _pty_id: &str,
            _limit: usize,
            _max_chars: usize,
        ) -> Result<Option<String>, String> {
            Ok(None)
        }
    }

    /// P54.5 — the `terminal/*` arm driven end to end over real JSON-RPC frames.
    ///
    /// Asserted on the replies the coordinator actually receives, not on the
    /// helper: an unmounted arm answers `method not found`, which the
    /// coordinator's best-effort catches would swallow into "the agent has no
    /// shell state", which is how this seam would ship silently broken.
    #[cfg(unix)]
    #[test]
    fn relay_dispatches_terminal_plane_requests() {
        let (a, b) = pair();
        let side = std::thread::spawn(move || {
            let mut s = b;
            let mut call = |id: &str, method: &str, params: serde_json::Value| {
                let v = serde_json::json!({
                    "jsonrpc": "2.0", "id": id, "method": method, "params": params,
                });
                let _ = frame::write_frame(&mut s, &serde_json::to_vec(&v).unwrap());
                loop {
                    match frame::decode(&mut s) {
                        Ok(Some(payload)) => {
                            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&payload) {
                                if v.get("id").and_then(|i| i.as_str()) == Some(id) {
                                    return v;
                                }
                            }
                        }
                        _ => panic!("relay closed the link before acking {method}"),
                    }
                }
            };
            let status = call("t1", "terminal/status", serde_json::json!({}));
            let commands = call(
                "t2",
                "terminal/commands",
                serde_json::json!({ "ptyId": "pty-1", "limit": 50 }),
            );
            let last = call(
                "t3",
                "terminal/last_command",
                serde_json::json!({ "ptyId": "pty-1", "maxChars": 6000 }),
            );
            // A session that is not on the plane is a caller bug and must be
            // refused rather than answered with an empty record list.
            let unknown = call(
                "t4",
                "terminal/commands",
                serde_json::json!({ "ptyId": "pty-nope" }),
            );
            // A ptyId-less read cannot be guessed at either.
            let missing = call("t5", "terminal/last_command", serde_json::json!({}));
            (status, commands, last, unknown, missing)
        });

        let (dir, vault) = temp_vault("terminal-plane");
        let vault = Arc::new(Mutex::new(vault));
        let relay = ChatRelay::new(link_from(a), vault, |_| {});
        relay.attach_terminal_plane(Arc::new(FakePlane));
        relay.spawn();

        let (status, commands, last, unknown, missing) = side.join().unwrap();

        assert!(status.get("error").is_none(), "terminal/status: {status}");
        assert_eq!(status["result"]["attached"], serde_json::json!(true));
        assert_eq!(status["result"]["count"], serde_json::json!(1));
        let row = &status["result"]["ptys"][0];
        assert_eq!(row["ptyId"], serde_json::json!("pty-1"));
        // Provenance and cwd travel with the row: this is what lets the
        // coordinator tell an agent session from the user's own shell.
        assert_eq!(row["origin"], serde_json::json!("agent"));
        assert_eq!(row["backend"], serde_json::json!("local"));
        assert_eq!(row["integration"], serde_json::json!("Rich"));
        assert_eq!(row["cwd"], serde_json::json!("/w"));

        assert!(
            commands.get("error").is_none(),
            "terminal/commands: {commands}"
        );
        assert_eq!(commands["result"]["count"], serde_json::json!(1));
        assert_eq!(commands["result"]["cwd"], serde_json::json!("/w"));
        let rec = &commands["result"]["commands"][0];
        assert_eq!(rec["exitCode"], serde_json::json!(0));
        assert_eq!(rec["trusted"], serde_json::json!(true));
        assert_eq!(rec["failed"], serde_json::json!(false));

        assert!(last.get("error").is_none(), "terminal/last_command: {last}");
        assert_eq!(
            last["result"]["block"],
            serde_json::json!("$ cargo test -p agentcowork-core\nexit 0")
        );

        assert!(
            unknown.get("error").is_some(),
            "unknown pty must be refused"
        );
        assert!(missing.get("error").is_some(), "ptyId is required");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A host with no PTY host must say so, and must not hand back an empty
    /// session list that reads as "a shell with nothing running" — nor pretend
    /// a named session exists.
    #[cfg(unix)]
    #[test]
    fn relay_reports_a_detached_terminal_plane_honestly() {
        let (a, b) = pair();
        let side = std::thread::spawn(move || {
            let mut s = b;
            let mut call = |id: &str, method: &str, params: serde_json::Value| {
                let v = serde_json::json!({
                    "jsonrpc": "2.0", "id": id, "method": method, "params": params,
                });
                let _ = frame::write_frame(&mut s, &serde_json::to_vec(&v).unwrap());
                loop {
                    match frame::decode(&mut s) {
                        Ok(Some(payload)) => {
                            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&payload) {
                                if v.get("id").and_then(|i| i.as_str()) == Some(id) {
                                    return v;
                                }
                            }
                        }
                        _ => panic!("relay closed the link before acking {method}"),
                    }
                }
            };
            let status = call("d1", "terminal/status", serde_json::json!({}));
            let commands = call(
                "d2",
                "terminal/commands",
                serde_json::json!({ "ptyId": "pty-1" }),
            );
            (status, commands)
        });

        let (dir, vault) = temp_vault("terminal-detached");
        let vault = Arc::new(Mutex::new(vault));
        // No `attach_terminal_plane` — the headless posture.
        let relay = ChatRelay::new(link_from(a), vault, |_| {});
        relay.spawn();

        let (status, commands) = side.join().unwrap();
        assert!(status.get("error").is_none(), "terminal/status: {status}");
        assert_eq!(status["result"]["attached"], serde_json::json!(false));
        assert_eq!(status["result"]["count"], serde_json::json!(0));
        assert_eq!(status["result"]["ptys"], serde_json::json!([]));
        assert!(
            commands.get("error").is_some(),
            "a session read on a host with no shell must be refused"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The old broad `work/*` arm answered from a helper that knew only the
    /// legacy methods. A newer WorkGateway method must still cross the real
    /// relay boundary, not disappear behind that shadow.
    #[cfg(unix)]
    #[test]
    fn relay_routes_newer_work_gateway_methods() {
        let (a, b) = pair();
        let side = std::thread::spawn(move || {
            let mut s = b;
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": "agent-1",
                "method": "work/agent_spawn",
                "params": {
                    "workId": "relay-work",
                    "runId": "relay-run",
                    "agentSessionId": "relay-session",
                    "agentId": "agent-a",
                    "lifetime": "ephemeral_child"
                }
            });
            frame::write_frame(&mut s, &serde_json::to_vec(&request).unwrap()).unwrap();
            loop {
                match frame::decode(&mut s) {
                    Ok(Some(payload)) => {
                        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload) {
                            if value.get("id").and_then(Value::as_str) == Some("agent-1") {
                                return value;
                            }
                        }
                    }
                    _ => panic!("relay closed before acknowledging work/agent_spawn"),
                }
            }
        });

        let (dir, vault) = temp_vault("work-relay-routing");
        let vault = Arc::new(Mutex::new(vault));
        let relay = ChatRelay::new(link_from(a), vault, |_| {});
        relay
            .work_gateway()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .create_work("relay-work", None, None, "route newer methods");
        relay.spawn();

        let response = side.join().unwrap();
        assert!(
            response.get("error").is_none(),
            "work/agent_spawn: {response}"
        );
        assert_eq!(
            response["result"]["workId"],
            serde_json::json!("relay-work")
        );
        assert!(response["result"]["sequence"].as_u64().is_some());

        let _ = std::fs::remove_dir_all(dir);
    }
}
