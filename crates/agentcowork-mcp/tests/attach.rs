//! P6.6 — MCP server attach loopback tests (spawn the mock server binary and
//! reconcile its tools with native precedence).

use agentcowork_mcp::ToolCatalog;
use agentcowork_mcp::attach::AttachedServer;

fn mock_server() -> &'static str {
    env!("CARGO_BIN_EXE_mock-mcp-server")
}

#[test]
fn attach_spawns_user_server_and_reconciles_tools() {
    let mut catalog = ToolCatalog::new();
    let mut server = AttachedServer::spawn(mock_server(), &[]).unwrap();
    let names = server
        .attach(&mut catalog, "mcp:mock")
        .expect("attach should succeed");
    assert!(names.contains(&"gmail_list".to_string()));
    assert!(names.contains(&"gmail_send".to_string()));
    assert_eq!(catalog.external_count(), 2);
    assert_eq!(catalog.origin("gmail_list"), Some("mcp:mock"));
    assert_eq!(catalog.origin("snapshot"), Some("native"));
    server.shutdown();
}

#[test]
fn native_collision_is_not_registered() {
    let mut catalog = ToolCatalog::new();
    let mut server = AttachedServer::spawn(mock_server(), &[]).unwrap();
    let names = server.attach(&mut catalog, "mcp:mock").unwrap();
    assert!(!names.contains(&"snapshot".to_string()));
    assert_eq!(catalog.origin("snapshot"), Some("native"));
    server.shutdown();
}

/// P55.11 — the call half of the loop: a tool the handshake advertised is
/// callable on the *same* child, and a server error is surfaced (never read as
/// an empty success).
#[test]
fn call_tool_runs_on_the_attached_child() {
    let mut catalog = ToolCatalog::new();
    let mut server = AttachedServer::spawn(mock_server(), &[]).unwrap();
    let names = server.attach(&mut catalog, "mcp:mock").unwrap();
    assert!(names.contains(&"gmail_list".to_string()));

    let result = server
        .call_tool("gmail_list", &serde_json::json!({}))
        .expect("tools/call should succeed");
    assert_eq!(result["content"][0]["text"], "call:gmail_list");
    assert_eq!(result["isError"], false);

    let err = server
        .call_tool("gmail_list", &serde_json::json!({ "fail": true }))
        .expect_err("a server error reply must not decode as a result");
    assert!(
        err.to_string().contains("tool exploded"),
        "error should carry the server message, got: {err}"
    );
    server.shutdown();
}

/// A command that exists on every host and is not an MCP server: on POSIX `true`
/// (exits immediately), on Windows `where.exe` (System32 console binary). A
/// shell launcher is deliberately avoided — `resolve_stdio_launch` refuses
/// `sh`/`bash`/`cmd`/`powershell`/`pwsh` as launchers, and a bare `true` has no
/// extension so `find_on_path` cannot find it on Windows.
fn non_server_fixture() -> &'static str {
    if cfg!(windows) { "where.exe" } else { "true" }
}

#[test]
fn dead_server_fails_cleanly() {
    let mut catalog = ToolCatalog::new();
    let mut server = AttachedServer::spawn(non_server_fixture(), &[])
        .unwrap_or_else(|e| panic!("fixture {} must be spawnable: {e}", non_server_fixture()));
    assert!(server.attach(&mut catalog, "mcp:dead").is_err());
}
