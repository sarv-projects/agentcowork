//! Track 2 / P65.8 — External-agent ACP handshake acceptance suite.
//!
//! Evidence-gated readiness (§17.12.6): this drives the *public* ACP client
//! against real spawned agent processes speaking newline-delimited JSON-RPC
//! over stdio — no mocks in-process, no credentials:
//!
//! 1. **Framing conformance** — `encode_message` emits exactly one frame;
//!    `decode_messages` returns only complete lines and preserves a partial
//!    remainder for the next read.
//! 2. **Full handshake** — `initialize` (version + capability negotiation,
//!    auth), `session/new`, `session/prompt` (streamed `session/update`), and
//!    `shutdown` against the real `mock-acp-agent` process.
//! 3. **Ticketed permission round-trip** — the agent issues
//!    `session/request_permission`; the harness answers allow/deny and the
//!    decision is recorded on `PromptOutcome` (the Guard-2 seam).
//!
//! Cross-platform: the spawned fixtures are plain Rust binaries. The Windows
//! ConPTY/terminal-integration run of third-party agents remains open — see
//! TODO P65.8.

use agentcowork_acp::{
    AcpSession, ClientCapabilities, ClientInfo, FsCapabilities, PROTOCOL_VERSION,
    PermissionDecision, ProcessTransport, decode_messages, encode_message,
};

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "agentcowork".into(),
        title: "AgentCowork".into(),
        version: "0.1.0".into(),
    }
}

/// Resolve a compiled test-fixture binary via cargo's `CARGO_BIN_EXE_*` env.
/// Cargo's naming has drifted across versions, so probe the common spellings.
fn fixture_bin(name: &str) -> String {
    let underscored = name.replace('-', "_");
    let candidates = [
        format!("CARGO_BIN_EXE_{name}"),
        format!("CARGO_BIN_EXE_{}", underscored),
        format!("CARGO_BIN_EXE_{}", underscored.to_uppercase()),
        format!("CARGO_BIN_EXE_{}", name.to_uppercase()),
    ];
    for key in &candidates {
        if let Ok(v) = std::env::var(key) {
            return v;
        }
    }
    let available: Vec<String> = std::env::vars()
        .filter(|(k, _)| k.starts_with("CARGO_BIN_EXE_"))
        .map(|(k, _)| k)
        .collect();
    panic!("no CARGO_BIN_EXE_* env for `{name}`; available: {available:?}");
}

#[test]
fn acp_frame_roundtrips_and_preserves_partial_lines() {
    // `encode_message` emits exactly one newline-terminated frame.
    let frame = encode_message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#);
    assert!(frame.ends_with('\n'), "a frame must be newline-terminated");
    assert_eq!(
        frame.matches('\n').count(),
        1,
        "exactly one trailing newline"
    );

    // Two complete frames + one partial line: only the complete ones decode,
    // and the partial bytes stay buffered.
    let mut buf = Vec::new();
    buf.extend_from_slice(encode_message(r#"{"id":1}"#).as_bytes());
    buf.extend_from_slice(encode_message(r#"{"id":2}"#).as_bytes());
    buf.extend_from_slice(br#"{"id":3,"partial"#);
    let decoded = decode_messages(&mut buf).expect("decode");
    assert_eq!(
        decoded,
        vec![r#"{"id":1}"#.to_string(), r#"{"id":2}"#.to_string()]
    );
    assert!(!buf.is_empty(), "the partial line must remain buffered");

    // Completing the partial frame yields it on the next read.
    buf.push(b'}');
    buf.extend_from_slice(b"\n");
    let decoded = decode_messages(&mut buf).expect("decode 2");
    assert_eq!(decoded, vec![r#"{"id":3,"partial}"#.to_string()]);
    assert!(
        buf.is_empty(),
        "the buffer is drained after a complete read"
    );
}

#[test]
fn spawned_agent_full_handshake_and_capability_negotiation() {
    let bin = fixture_bin("mock_acp_agent");
    let transport = ProcessTransport::spawn(&bin, &[], &[]).expect("spawn mock agent");
    let mut session = AcpSession::new(transport);

    // initialize: version + capability negotiation.
    let result = session
        .initialize(client_info())
        .expect("initialize handshake over the real process");
    assert_eq!(result.protocol_version, PROTOCOL_VERSION);
    assert!(
        result.agent_capabilities.load_session,
        "loadSession advertised"
    );
    assert!(
        session.is_authenticated(),
        "empty authMethods ⇒ no auth needed"
    );
    assert!(session.is_alive());
    assert_eq!(
        session.agent_info().map(|i| i.name.as_str()),
        Some("mock-acp")
    );

    // session/new: a fresh session id comes back.
    let sid = session.session_new("/tmp", vec![]).expect("session/new");
    assert!(sid.starts_with("mock-session-"), "got {sid}");
    assert_eq!(session.session_id(), Some(sid.as_str()));

    // ACP v1's official empty session/load result retains the requested id.
    let loaded = session
        .session_load(&sid, "/tmp", vec![])
        .expect("session/load with an empty official result");
    assert_eq!(loaded, sid);
    assert_eq!(session.session_id(), Some(sid.as_str()));

    // session/prompt: the turn streams a session/update and ends cleanly.
    let outcome = session
        .prompt("acceptance handshake", |_| PermissionDecision::allow())
        .expect("prompt turn");
    assert!(
        outcome.updates.iter().any(|u| u
            .content
            .iter()
            .any(|c| c.text.contains("echo: acceptance handshake"))),
        "expected the echoed update, got {:?}",
        outcome.updates
    );

    // shutdown: the child is reaped, not leaked.
    session.shutdown();
    assert!(!session.is_alive(), "shutdown must terminate the process");
}

#[test]
fn mediated_client_capabilities_are_advertised() {
    let bin = fixture_bin("mock_acp_agent");
    let transport = ProcessTransport::spawn(&bin, &[], &[]).expect("spawn mock agent");
    let mut session = AcpSession::new(transport);

    // The mediated surface: advertise fs read/write + terminal so a
    // sandbox-aware agent may delegate those ops back to us.
    let caps = ClientCapabilities {
        fs: FsCapabilities {
            read_text_file: true,
            write_text_file: true,
        },
        terminal: true,
        session: None,
    };
    let result = session
        .initialize_with_caps(client_info(), caps)
        .expect("initialize with mediated caps");
    assert_eq!(result.protocol_version, PROTOCOL_VERSION);
    assert!(session.is_authenticated());
    session.shutdown();
}

#[test]
fn permission_round_trip_is_recorded_on_the_outcome() {
    let bin = fixture_bin("mock-agent-permission");
    let transport = ProcessTransport::spawn(&bin, &[], &[]).expect("spawn permission agent");
    let mut session = AcpSession::new(transport);
    session.initialize(client_info()).expect("initialize");
    let _ = session.session_new("/tmp", vec![]).expect("session/new");

    // Deny: the harness's Guard-2 decision is captured and the agent observes
    // the chosen option id ("deny").
    let denied = session
        .prompt("write the file", |_| PermissionDecision::deny())
        .expect("deny turn");
    assert_eq!(
        denied.permissions.len(),
        1,
        "exactly one permission request"
    );
    let req = &denied.permissions[0];
    assert_eq!(req.tool_call.title, "write test file");
    assert_eq!(req.options.len(), 2, "allow + deny options offered");
    assert_eq!(
        denied.permission_decisions,
        vec![PermissionDecision::deny()]
    );
    assert!(
        denied.updates.iter().any(|u| u
            .content
            .iter()
            .any(|c| c.text.contains(r#"permission resolved: "deny""#))),
        "the agent must observe the deny option: {:?}",
        denied.updates
    );

    // Allow on a second turn round-trips independently.
    let allowed = session
        .prompt("write the file again", |_| PermissionDecision::allow())
        .expect("allow turn");
    assert_eq!(
        allowed.permission_decisions,
        vec![PermissionDecision::allow()]
    );
    assert!(
        allowed.updates.iter().any(|u| u
            .content
            .iter()
            .any(|c| c.text.contains(r#"permission resolved: "allow""#))),
        "the agent must observe the allow option: {:?}",
        allowed.updates
    );

    session.shutdown();
}
