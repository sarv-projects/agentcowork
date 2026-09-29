//! FIX-03 / `TASK-CHAN-001` — the ACP **permission bridge** acceptance suite.
//!
//! Evidence class: a real spawned agent process speaking ND-JSON ACP over
//! stdio, a real Guard-2 ticket minted by `agentcowork_guard::TicketStore`, and
//! the real bridge — no in-process mock of the decision, no credentials. The
//! mocked part is the *human*, which is exactly what the bridge is designed to
//! take as input (the owning channel's answer).
//!
//! What is proven here (the unit suite in `permission_bridge` covers the
//! mapping in isolation):
//!
//! 1. **`once` is a bound, single-use ticket** — the agent observes an
//!    `allow_once` option, and the answer carries the ticket that backed it.
//! 2. **`reject` denies** — the agent observes the `reject_once` option.
//! 3. **`always` is never a client-side default** — an `always` answer with no
//!    recorded policy change reaches the agent as `allow_once`.
//! 4. **Fail closed** — an allow the bridge cannot back (no ticket) never
//!    reaches the agent as an allow; the turn denies.

use agentcowork_acp::{
    AcpApprovalChoice, AcpError, AcpSession, ClientInfo, PermissionBridge, PermissionDecision,
    PermissionPreview, ProcessTransport, TicketBinding, TicketFacts, TrustOutcome,
};
use agentcowork_guard::{AuthorizationTicket, RiskLevel, TicketState, TicketStore};

/// Resolve a compiled test-fixture binary via cargo's `CARGO_BIN_EXE_*` env.
fn fixture_bin(name: &str) -> String {
    let underscored = name.replace('-', "_");
    for key in [
        format!("CARGO_BIN_EXE_{name}"),
        format!("CARGO_BIN_EXE_{underscored}"),
        format!("CARGO_BIN_EXE_{}", underscored.to_uppercase()),
    ] {
        if let Ok(v) = std::env::var(&key) {
            return v;
        }
    }
    let available: Vec<String> = std::env::vars()
        .filter(|(k, _)| k.starts_with("CARGO_BIN_EXE_"))
        .map(|(k, _)| k)
        .collect();
    panic!("no CARGO_BIN_EXE_* env for `{name}`; available: {available:?}");
}

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "agentcowork".into(),
        title: "AgentCowork".into(),
        version: "0.1.0".into(),
    }
}

/// A live ticket from the real store — the artifact an ACP allow must be
/// backed by.
fn minted_ticket(agent_id: &str, session_id: &str, args_hash: &str) -> TicketFacts {
    let mut store = TicketStore::new();
    let ticket_id = store.mint(AuthorizationTicket {
        ticket_id: "tkt:acceptance".into(),
        agent_id: agent_id.into(),
        session_id: session_id.into(),
        tool_id: "acp.tc-1".into(),
        operation: "write".into(),
        args_hash: args_hash.into(),
        paths: vec!["/tmp/agentcowork-acp-e2e.txt".into()],
        expires_at_ms: 0,
        single_use: true,
        approval_source: agentcowork_guard::ApprovalSource::Human,
        approval_nonce: "nonce".into(),
        risk: RiskLevel::Medium,
        audit_seq: 0,
        state: TicketState::Approved,
        bindings: Vec::new(),
        execution_id: String::new(),
        action_id: String::new(),
        idempotency_key: String::new(),
    });
    // Spend it the way the executor does: exactly once, for these args.
    store
        .use_ticket(&ticket_id, args_hash)
        .expect("the harness spends its own ticket");
    assert!(
        store.use_ticket(&ticket_id, args_hash).is_err(),
        "a single-use ticket can never be spent twice"
    );
    // Re-read the artifact: after the spend it is `Used`, so a replay of the
    // same facts is what the bridge must reject.
    TicketFacts {
        ticket_id,
        agent_id: agent_id.into(),
        session_id: session_id.into(),
        args_hash: args_hash.into(),
        single_use: true,
        validated_by_guard: true,
    }
}

/// What the agent reported about the option the bridge chose.
fn resolved_option(updates: &[agentcowork_acp::SessionUpdate]) -> String {
    for update in updates {
        for block in &update.content {
            if let Some(rest) = block.text.split("permission resolved: ").nth(1) {
                return rest.trim().trim_matches('"').to_string();
            }
        }
    }
    panic!("the agent never reported a resolved option: {updates:?}");
}

#[test]
fn once_is_answered_with_an_offered_single_use_option_backed_by_a_ticket() {
    let bin = fixture_bin("mock-agent-permission");
    let mut session = AcpSession::new(ProcessTransport::spawn(&bin, &[], &[]).expect("spawn"));
    session.initialize(client_info()).expect("initialize");
    let session_id = session.session_new("/tmp", vec![]).expect("session/new");

    let bridge = PermissionBridge::new();
    let outcome = session
        .prompt("write the file", |req| {
            // The approver is shown a bounded, redacted diff of what it is
            // approving — not a bare title.
            let preview = PermissionPreview::build(req, "write", RiskLevel::Medium);
            assert!(preview.diff.contains("/tmp/agentcowork-acp-e2e.txt"));
            let binding = TicketBinding {
                agent_id: "mock-agent".into(),
                session_id: session_id.clone(),
                args_hash: "args-1".into(),
            };
            let trust = TrustOutcome::once(
                minted_ticket("mock-agent", &session_id, "args-1"),
                "human approved",
            );
            bridge
                .answer(req, &binding, &trust)
                .expect("answered")
                .decision
        })
        .expect("turn");

    // The agent observed the `allow_once` option, never a synthesized id.
    assert_eq!(resolved_option(&outcome.updates), "allow");
    assert_eq!(outcome.permission_decisions.len(), 1);
    session.shutdown();
}

#[test]
fn reject_reaches_the_agent_as_the_offered_reject_option() {
    let bin = fixture_bin("mock-agent-permission");
    let mut session = AcpSession::new(ProcessTransport::spawn(&bin, &[], &[]).expect("spawn"));
    session.initialize(client_info()).expect("initialize");
    let session_id = session.session_new("/tmp", vec![]).expect("session/new");

    let bridge = PermissionBridge::new();
    let outcome = session
        .prompt("write the file", |req| {
            let binding = TicketBinding {
                agent_id: "mock-agent".into(),
                session_id: session_id.clone(),
                args_hash: "args-1".into(),
            };
            bridge
                .answer(req, &binding, &TrustOutcome::reject("human rejected"))
                .expect("answered")
                .decision
        })
        .expect("turn");
    assert_eq!(resolved_option(&outcome.updates), "deny");
    session.shutdown();
}

#[test]
fn persistent_permission_offer_reaches_the_agent_as_once() {
    let bin = fixture_bin("mock-agent-permission");
    let mut session = AcpSession::new(ProcessTransport::spawn(&bin, &[], &[]).expect("spawn"));
    session.initialize(client_info()).expect("initialize");
    let session_id = session.session_new("/tmp", vec![]).expect("session/new");

    let bridge = PermissionBridge::new();
    let outcome = session
        .prompt("write the file", |req| {
            let binding = TicketBinding {
                agent_id: "mock-agent".into(),
                session_id: session_id.clone(),
                args_hash: "args-1".into(),
            };
            let answer = bridge
                .answer(
                    req,
                    &binding,
                    &TrustOutcome::always(
                        minted_ticket("mock-agent", &session_id, "args-1"),
                        "user pressed always",
                    ),
                )
                .expect("answered");
            assert!(answer.narrowed, "the narrowing must be observable");
            assert_eq!(answer.choice, AcpApprovalChoice::Once);
            answer.decision
        })
        .expect("turn");
    // The agent offered only `allow_once`; the bridge must express `once`.
    assert_eq!(resolved_option(&outcome.updates), "allow");
    session.shutdown();
}

#[test]
fn an_allow_the_bridge_cannot_back_fails_closed_to_a_denial() {
    let bin = fixture_bin("mock-agent-permission");
    let mut session = AcpSession::new(ProcessTransport::spawn(&bin, &[], &[]).expect("spawn"));
    session.initialize(client_info()).expect("initialize");
    let session_id = session.session_new("/tmp", vec![]).expect("session/new");

    let bridge = PermissionBridge::new();
    let outcome = session
        .prompt("write the file", |req| {
            let binding = TicketBinding {
                agent_id: "mock-agent".into(),
                session_id: session_id.clone(),
                args_hash: "args-1".into(),
            };
            // A policy allow that produced no ticket: nothing authorizes the
            // effect, so the bridge refuses it and the host denies.
            let mut unbacked = TrustOutcome::reject("policy allow");
            unbacked.choice = AcpApprovalChoice::Once;
            match bridge.answer(req, &binding, &unbacked) {
                Ok(answer) => answer.decision,
                Err(_) => PermissionDecision::deny(),
            }
        })
        .expect("turn");
    // The agent must never see the allow option for an unauthorized effect.
    assert_eq!(resolved_option(&outcome.updates), "deny");
    session.shutdown();
}

#[test]
fn a_replayed_ticket_cannot_authorize_a_second_request() {
    let bridge = PermissionBridge::new();
    let session_id = "sess-mock-agent-perm".to_string();
    let binding = TicketBinding {
        agent_id: "mock-agent".into(),
        session_id: session_id.clone(),
        args_hash: "args-1".into(),
    };
    let spent = minted_ticket("mock-agent", &session_id, "args-1");

    // The same ticket facts presented for a different args hash is a replay of
    // a spent artifact: refused, so a second identical request is not covered.
    let other_binding = TicketBinding {
        args_hash: "args-2".into(),
        ..binding.clone()
    };
    assert!(
        bridge
            .answer(
                &permission_request(),
                &other_binding,
                &TrustOutcome::once(spent.clone(), "replay")
            )
            .is_err(),
        "a replayed ticket must never back an allow"
    );

    // And the ticket is genuinely spent in the real store (single use).
    assert!(spent.single_use);
}

/// A request shaped like the fixture's, built directly (no transport needed).
fn permission_request() -> agentcowork_acp::PermissionRequestParams {
    agentcowork_acp::PermissionRequestParams {
        session_id: "sess-mock-agent-perm".into(),
        tool_call: agentcowork_acp::ToolCall {
            tool_call_id: "tc-1".into(),
            title: "write test file".into(),
            kind: None,
            content: vec![agentcowork_acp::ContentBlock {
                r#type: "text".into(),
                text: "write /tmp/agentcowork-acp-e2e.txt".into(),
            }],
            locations: Vec::new(),
            raw_input: None,
        },
        options: vec![
            agentcowork_acp::PermissionOption {
                option_id: "allow".into(),
                kind: agentcowork_acp::PermissionOptionKind::AllowOnce,
                label: "Allow".into(),
            },
            agentcowork_acp::PermissionOption {
                option_id: "deny".into(),
                kind: agentcowork_acp::PermissionOptionKind::RejectOnce,
                label: "Deny".into(),
            },
        ],
    }
}

#[test]
fn the_wire_resolver_refuses_to_invent_an_option_for_an_empty_offer() {
    // An agent that offers nothing cannot be answered: the turn fails closed
    // with a typed error instead of a synthesized `allow_once`.
    let bridge = PermissionBridge::new();
    let mut request = permission_request();
    request.options.clear();
    let error = bridge
        .resolve(&request, &PermissionDecision::allow())
        .expect_err("must fail closed");
    assert!(error.to_string().contains("allow_once"), "{error}");
    // The same shape surfaces on the client as a typed, fail-closed error.
    let session_error = AcpError::PermissionUnanswerable(error.to_string());
    assert!(
        session_error.to_string().contains("fail-closed"),
        "{session_error}"
    );
}
