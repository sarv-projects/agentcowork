//! P10.1.8 — ACP harness-driving E2E:
//! spawn an agent via ACP → permission request → harness decision → audit →
//! stop.
//!
//! The fixture (`mock-agent-permission`) is a real child process that issues
//! a `session/request_permission` on its first prompt turn — exactly the
//! Guard-2 seam the harness must answer. Real CLIs (Claude Code, Codex)
//! remain credential/install gated; this proves the harness drives the full
//! permission lifecycle over a real stdio transport.

use agentcowork_acp::client::{AcpSession, ProcessTransport};
use agentcowork_acp::{ClientInfo, PermissionDecision};

fn mock_agent_path() -> &'static str {
    env!("CARGO_BIN_EXE_mock-agent-permission")
}

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "agentcowork".into(),
        title: "AgentCowork".into(),
        version: "0.1.0".into(),
    }
}

#[test]
fn harness_drives_agent_permission_audit_stop() {
    // Spawn the agent process (the harness's "spawn Claude Code via ACP").
    let transport = ProcessTransport::spawn(mock_agent_path(), &["perm-agent"], &[]).unwrap();
    let mut session = AcpSession::new(transport);

    // Initialize + session/new.
    let init = session.initialize(client_info()).unwrap();
    assert_eq!(init.protocol_version, 1);
    assert_eq!(session.agent_info().unwrap().name, "perm-agent");
    let sid = session.session_new("/tmp", vec![]).unwrap();
    assert!(sid.starts_with("sess-"));

    // Drive one prompt; the agent requests permission to write a file.
    // The harness decides: allow option "allow" (the Guard-2 approval path).
    let outcome = session
        .prompt("write the test file", |_req| PermissionDecision::Allow {
            option_id: Some("allow".into()),
        })
        .unwrap();

    // Audit: the permission request + our decision were both recorded.
    assert_eq!(
        outcome.permissions.len(),
        1,
        "permission request must be audited"
    );
    assert_eq!(outcome.permissions[0].tool_call.title, "write test file");
    assert_eq!(outcome.permission_decisions.len(), 1);
    assert!(matches!(
        outcome.permission_decisions[0],
        PermissionDecision::Allow { .. }
    ));
    assert_eq!(outcome.stop_reason, agentcowork_acp::StopReason::EndTurn);

    // The agent's update reflects the resolved decision.
    assert!(outcome.updates.iter().any(|u| {
        u.content
            .iter()
            .any(|c| c.text.contains("permission resolved"))
    }));

    // Stop the harness cleanly.
    session.shutdown();
    assert!(!session.is_alive());
}
