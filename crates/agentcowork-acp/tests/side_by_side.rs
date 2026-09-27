//! P6.8 — two external agent processes run side-by-side via ACP.
//!
//! The live "two real CLIs" item needs installed agent binaries; this test
//! proves the harness drives **two concurrent agent processes** through the
//! real `ProcessTransport` (stdio spawn), each with its own
//! initialize → session/new → prompt → cancel lifecycle, so the
//! side-by-side concurrency claim is exercised at the process level, not just
//! with an in-process mock.

use agentcowork_acp::client::{AcpSession, ProcessTransport};
use agentcowork_acp::{ClientInfo, PermissionDecision};

fn mock_agent_path() -> &'static str {
    env!("CARGO_BIN_EXE_mock-agent")
}

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "agentcowork".into(),
        title: "AgentCowork".into(),
        version: "0.1.0".into(),
    }
}

#[test]
fn two_mock_agents_run_side_by_side() {
    // Two real child processes, spawned concurrently.
    let t1 = ProcessTransport::spawn(mock_agent_path(), &["agent-one"], &[]).unwrap();
    let t2 = ProcessTransport::spawn(mock_agent_path(), &["agent-two"], &[]).unwrap();

    let mut a = AcpSession::new(t1);
    let mut b = AcpSession::new(t2);

    // Both initialize (id 1 each — independent id counters).
    let ra = a.initialize(client_info()).unwrap();
    let rb = b.initialize(client_info()).unwrap();
    assert_eq!(ra.protocol_version, 1);
    assert_eq!(rb.protocol_version, 1);
    assert_eq!(a.agent_info().unwrap().name, "agent-one");
    assert_eq!(b.agent_info().unwrap().name, "agent-two");

    // Both create sessions.
    let sa = a.session_new("/workspace", vec![]).unwrap();
    let sb = b.session_new("/workspace", vec![]).unwrap();
    assert_eq!(sa, "sess-agent-one");
    assert_eq!(sb, "sess-agent-two");

    // Interleave prompts — the harness must keep both sessions alive.
    let out_a = a
        .prompt("hello from the other side", |_| PermissionDecision::Deny {
            option_id: None,
        })
        .unwrap();
    let out_b = b
        .prompt("hello back", |_| PermissionDecision::Deny {
            option_id: None,
        })
        .unwrap();
    assert_eq!(out_a.stop_reason.as_str(), "end_turn");
    assert_eq!(out_b.stop_reason.as_str(), "end_turn");
    // The session/update notification from each agent landed in its own
    // outcome (isolated per process).
    assert_eq!(out_a.updates.len(), 1);
    assert_eq!(out_b.updates.len(), 1);

    // Both stay alive and can be torn down independently.
    assert!(a.is_alive());
    assert!(b.is_alive());
    a.shutdown();
    b.shutdown();
}

#[test]
fn cancel_targets_only_the_named_session() {
    let t1 = ProcessTransport::spawn(mock_agent_path(), &["cancel-a"], &[]).unwrap();
    let t2 = ProcessTransport::spawn(mock_agent_path(), &["cancel-b"], &[]).unwrap();
    let mut a = AcpSession::new(t1);
    let mut b = AcpSession::new(t2);
    a.initialize(client_info()).unwrap();
    b.initialize(client_info()).unwrap();
    a.session_new("/workspace", vec![]).unwrap();
    b.session_new("/workspace", vec![]).unwrap();

    // Cancelling A must not affect B's next prompt.
    a.cancel().unwrap();
    let out_b = b
        .prompt("still working", |_| PermissionDecision::Deny {
            option_id: None,
        })
        .unwrap();
    assert_eq!(out_b.stop_reason.as_str(), "end_turn");
    a.shutdown();
    b.shutdown();
}
