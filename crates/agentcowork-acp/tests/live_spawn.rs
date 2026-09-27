//! Live ACP CLI spawn E2E (the "external binary" honest ceiling, closed).
//!
//! - [`spawned_mock_cli_full_handshake`] — spawns the *real* `mock-acp-agent`
//!   process (compiled from `src/bin/mock_acp_agent.rs`) via
//!   `ProcessTransport` and runs the full initialize → session/new handshake.
//!   No credentials, deterministic, runs in CI.
//! - [`live_claude_agent_acp_initialize`] — `#[ignore]` + env-gated: when
//!   `AGENTCOWORK_LIVE_TEST=1` **and** the official `@agentclientprotocol/
//!   claude-agent-acp` CLI is resolvable, spawns it and drives the real
//!   `initialize` handshake (no subscription required for initialize; the
//!   auth-required surface is handled by the caller, not this test).

use agentcowork_acp::{AcpSession, ClientInfo, ProcessTransport};

/// Path to the compiled mock binary (cargo sets this in the test process
/// env; the var name uppercases the target and swaps `-` for `_`).
fn mock_bin() -> Option<String> {
    std::env::var("CARGO_BIN_EXE_MOCK_ACP_AGENT").ok()
}

#[test]
fn spawned_mock_cli_full_handshake() {
    let Some(bin) = mock_bin() else {
        eprintln!("skipped: CARGO_BIN_EXE_MOCK_ACP_AGENT unset");
        return;
    };
    let transport = ProcessTransport::spawn(&bin, &[], &[]).expect("spawn mock agent");
    let mut session = AcpSession::new(transport);
    session
        .initialize(ClientInfo {
            name: "agentcowork".into(),
            title: "AgentCowork".into(),
            version: "0.1.0".into(),
        })
        .expect("initialize handshake over the real process");
    assert!(session.is_authenticated());
    assert!(session.is_alive());
    let sid = session
        .session_new("/tmp", vec![])
        .expect("session/new over the real process");
    assert!(sid.starts_with("mock-session-"), "got {sid}");
    assert!(session.is_alive());

    // Prompt turn also round-trips (echo text comes back as a session/update).
    let outcome = session
        .prompt("hello mock", |_| {
            agentcowork_acp::PermissionDecision::allow()
        })
        .expect("prompt turn over the real process");
    assert!(
        outcome.updates.iter().any(|u| u
            .content
            .iter()
            .any(|c| c.text.contains("echo: hello mock"))),
        "expected echo update, got {:?}",
        outcome.updates
    );

    session.shutdown();
    assert!(!session.is_alive(), "shutdown kills the process");
}

/// The real CLI — only when the user opts in AND the binary resolves.
#[test]
#[ignore = "requires AGENTCOWORK_LIVE_TEST=1 and the claude-agent-acp CLI"]
fn live_claude_agent_acp_initialize() {
    if agentcowork_types::env_compat::get("LIVE_TEST").as_deref() != Some("1") {
        eprintln!("skipped: AGENTCOWORK_LIVE_TEST != 1");
        return;
    }
    // Resolve `npx @agentclientprotocol/claude-agent-acp` (or a raw `claude`
    // on PATH). Initialize needs no credentials, so this is a real spawn of
    // the official CLI without any subscription.
    let (command, args) = if which("claude").is_some() {
        (
            "claude".to_string(),
            vec!["--protocol".to_string(), "acp".to_string()],
        )
    } else {
        (
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@agentclientprotocol/claude-agent-acp".to_string(),
            ],
        )
    };
    let Ok(transport) = ProcessTransport::spawn(
        &command,
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        &[],
    ) else {
        eprintln!("skipped: {command} not resolvable");
        return;
    };
    let mut session = AcpSession::new(transport);
    session
        .initialize(ClientInfo {
            name: "agentcowork".into(),
            title: "AgentCowork".into(),
            version: "0.1.0".into(),
        })
        .expect("real CLI initialize handshake");
    assert!(session.is_authenticated());
    session.shutdown();
}

fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}
