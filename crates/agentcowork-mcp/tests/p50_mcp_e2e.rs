//! P50.5.4 — Real connector/MCP E2E (attach a test server → list tools →
//! invoke the read path and the approval-gated mutation path → unknown tools
//! refused → no direct bypass → reconnect works).
//!
//! No network, no env: the mock MCP server binary speaks real NDJSON-RPC over
//! a real stdio child process, and `McpServer::handle_json` runs the real
//! dispatch. The approval gate is modeled at the seam the shell's Guard-2
//! ticket path already enforces end to end (P50.3.4/P50.3.5): the test handler
//! refuses the mutation until an explicit approval flips it — the call must
//! fail closed first and succeed only after approval, never the reverse.

use std::sync::{Arc, Mutex};

use agentcowork_mcp::ToolCatalog;
use agentcowork_mcp::attach::AttachedServer;
use agentcowork_mcp::server::{McpServer, ToolCallHandler};

fn mock_server() -> &'static str {
    env!("CARGO_BIN_EXE_mock-mcp-server")
}

/// Handler with an explicit approval latch: reads always run, the `gmail_send`
/// mutation runs only after `approve()` — the executable shape of the
/// request→guard-window→commit flow. The latch is shared so the test (the
/// "human card") can flip it while the server owns the handler.
#[derive(Clone)]
struct GateHandler {
    approved: Arc<Mutex<bool>>,
}

impl GateHandler {
    fn new() -> (Self, Arc<Mutex<bool>>) {
        let approved = Arc::new(Mutex::new(false));
        (
            Self {
                approved: Arc::clone(&approved),
            },
            approved,
        )
    }
}

impl ToolCallHandler for GateHandler {
    fn call(
        &mut self,
        name: &str,
        _arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match name {
            "gmail_list" => Ok(serde_json::json!({"threads": ["t1", "t2"]})),
            "gmail_send" if *self.approved.lock().unwrap() => Ok(serde_json::json!({"sent": true})),
            "gmail_send" => Err("approval required: gmail_send is a mutation".into()),
            other => Err(format!("unknown tool: {other}")),
        }
    }
}

fn call(server: &mut McpServer<GateHandler>, name: &str) -> serde_json::Value {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": name, "arguments": {} },
    })
    .to_string();
    let out = server.handle_json(&body);
    serde_json::from_str(&out).expect("dispatch replies with JSON")
}

#[test]
fn real_connector_attach_list_and_read_path() {
    // Attach + authenticate surface: spawn the test server, list its tools.
    let mut catalog = ToolCatalog::new();
    let mut child = AttachedServer::spawn(mock_server(), &[]).expect("spawn test server");
    let names = child
        .attach(&mut catalog, "mcp:e2e")
        .expect("attach must succeed");
    assert!(names.contains(&"gmail_list".to_string()));
    assert!(names.contains(&"gmail_send".to_string()));
    assert_eq!(catalog.origin("gmail_list"), Some("mcp:e2e"));

    // Read path: gmail_list invokes through the real dispatch and answers.
    // (Success envelopes carry the value under result.structuredContent.)
    let (handler, _latch) = GateHandler::new();
    let mut server = McpServer::new(handler).with_catalog(catalog);
    let res = call(&mut server, "gmail_list");
    assert!(
        res.get("result").is_some(),
        "P50.5.4: read-path call failed: {res}"
    );
    assert_eq!(
        res["result"]["structuredContent"]["threads"],
        serde_json::json!(["t1", "t2"]),
        "P50.5.4: read-path answer mismatch: {res}"
    );
    eprintln!("P50.5.4: attach → list → read-path call verified");

    // Reconnect: shutdown and re-attach works (children die, identity lives).
    child.shutdown();
    let mut catalog2 = ToolCatalog::new();
    let mut child2 = AttachedServer::spawn(mock_server(), &[]).expect("respawn test server");
    let names2 = child2
        .attach(&mut catalog2, "mcp:e2e")
        .expect("re-attach must succeed");
    assert!(names2.contains(&"gmail_list".to_string()));
    child2.shutdown();
    eprintln!("P50.5.4: reconnect after shutdown verified");
}

#[test]
fn real_connector_mutation_needs_approval_first() {
    let mut catalog = ToolCatalog::new();
    let mut child = AttachedServer::spawn(mock_server(), &[]).expect("spawn test server");
    child
        .attach(&mut catalog, "mcp:e2e")
        .expect("attach must succeed");
    let (handler, latch) = GateHandler::new();
    let mut server = McpServer::new(handler).with_catalog(catalog);

    // Mutation WITHOUT approval fails closed (the Guard-2 card was never
    // answered) — handler errors surface as -32001, never a silent send.
    let denied = call(&mut server, "gmail_send");
    let denied_err = denied
        .get("error")
        .expect("P50.5.4: unapproved mutation must error");
    assert_eq!(
        denied_err.get("code").and_then(|c| c.as_i64()),
        Some(-32001),
        "P50.5.4: unapproved mutation must be -32001, got: {denied}"
    );
    assert!(
        denied_err.to_string().contains("approval required"),
        "P50.5.4: unapproved mutation failed for the wrong reason: {denied}"
    );

    // Approval (the human card) → the identical call now executes.
    *latch.lock().unwrap() = true;
    let sent = call(&mut server, "gmail_send");
    assert_eq!(
        sent["result"]["structuredContent"]["sent"],
        serde_json::Value::Bool(true),
        "P50.5.4: approved mutation must execute: {sent}"
    );
    eprintln!("P50.5.4: mutation denied-before-approval, executed-after verified");
    child.shutdown();
}

#[test]
fn real_connector_unknown_tool_and_bypass_refused() {
    let mut catalog = ToolCatalog::new();
    let mut child = AttachedServer::spawn(mock_server(), &[]).expect("spawn test server");
    child
        .attach(&mut catalog, "mcp:e2e")
        .expect("attach must succeed");
    let (handler, latch) = GateHandler::new();
    *latch.lock().unwrap() = true;
    let mut server = McpServer::new(handler).with_catalog(catalog);

    // Unknown tool: the dispatcher refuses with method/tool-not-found, even
    // for an approved session — approval never stretches to new tools.
    let unknown = call(&mut server, "gmail_nuke_everything");
    let code = unknown
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_i64())
        .expect("P50.5.4: unknown tool must error");
    assert_eq!(
        code, -32602,
        "P50.5.4: unknown tool must be -32602, got: {unknown}"
    );

    // No direct bypass: spawning a missing binary fails honestly (no phantom
    // server), and malformed wire bytes never dispatch.
    assert!(
        AttachedServer::spawn("agentcowork-definitely-not-a-binary", &[]).is_err(),
        "P50.5.4: missing binary must fail to spawn"
    );
    let garbage = server.handle_json("this is not json");
    let garbage_v: serde_json::Value =
        serde_json::from_str(&garbage).expect("dispatcher answers JSON even for garbage");
    assert!(
        garbage_v.get("error").is_some(),
        "P50.5.4: malformed wire bytes must error, got: {garbage}"
    );
    eprintln!("P50.5.4: unknown-tool refusal + no-bypass verified");
    child.shutdown();
}
