//! Official-shaped fixtures for the supervised Streamable-HTTP `/mcp` lease.
//!
//! These tests intentionally use raw TCP rather than the in-process JSON helper
//! so method, header, status, and empty-notification behavior are exercised at
//! the same boundary an MCP client sees.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agentcowork_mcp::{LoopbackPool, McpServer, SHARED_FACADES, ToolCallHandler};
use serde_json::Value;

struct EchoHandler {
    calls: Arc<AtomicUsize>,
}

impl ToolCallHandler for EchoHandler {
    fn call(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::json!({"tool": name, "arguments": arguments}))
    }
}

struct BlockingHandler {
    entered: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
}

impl ToolCallHandler for BlockingHandler {
    fn call(&mut self, _name: &str, _arguments: &Value) -> Result<Value, String> {
        self.entered.store(true, Ordering::SeqCst);
        while !self.release.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(2));
        }
        Ok(Value::Null)
    }
}

struct SlowHandler {
    calls: Arc<AtomicUsize>,
    delay: Duration,
}

impl ToolCallHandler for SlowHandler {
    fn call(&mut self, name: &str, _arguments: &Value) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        thread::sleep(self.delay);
        Ok(serde_json::json!({"tool": name, "completed": true}))
    }
}

struct PanicHandler {
    calls: Arc<AtomicUsize>,
}

impl ToolCallHandler for PanicHandler {
    fn call(&mut self, _name: &str, _arguments: &Value) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        panic!("fixture handler panic after possible effect");
    }
}

struct ApprovalHandler {
    calls: Arc<AtomicUsize>,
    approved: Arc<AtomicBool>,
}

impl ToolCallHandler for ApprovalHandler {
    fn call(&mut self, name: &str, _arguments: &Value) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.approved.load(Ordering::SeqCst) {
            Ok(serde_json::json!({"tool": name, "approved": true}))
        } else {
            Err("approval required: waiting for the human gate".to_string())
        }
    }
}

struct TerminalFailureHandler {
    calls: Arc<AtomicUsize>,
}

impl ToolCallHandler for TerminalFailureHandler {
    fn call(&mut self, _name: &str, _arguments: &Value) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err("invalid argument: terminal rejection".to_string())
    }
}

struct UnknownFailureHandler {
    calls: Arc<AtomicUsize>,
}

impl ToolCallHandler for UnknownFailureHandler {
    fn call(&mut self, _name: &str, _arguments: &Value) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err("post-effect outcome unknown".to_string())
    }
}

struct HttpResponse {
    status: u16,
    headers: String,
    body: String,
}

impl HttpResponse {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|error| panic!("expected JSON response, got {:?}: {error}", self.body))
    }
}

fn request(
    addr: SocketAddr,
    token: &str,
    method: &str,
    path: &str,
    host: Option<&str>,
    origin: Option<&str>,
    content_type: Option<&str>,
    protocol: Option<&str>,
    mcp_method: Option<&str>,
    mcp_name: Option<&str>,
    body: &str,
) -> HttpResponse {
    let port = addr.port();
    let host = host
        .map(|value| value.to_string())
        .unwrap_or_else(|| format!("127.0.0.1:{port}"));
    let origin = origin
        .map(|value| value.to_string())
        .unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
    let mut stream = TcpStream::connect(addr).expect("connect to lease");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set client read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .expect("set client write timeout");
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nAuthorization: Bearer {token}\r\nContent-Type: {}\r\nAccept: application/json, text/event-stream\r\n",
        content_type.unwrap_or("application/json")
    );
    if let Some(protocol) = protocol {
        head.push_str(&format!("MCP-Protocol-Version: {protocol}\r\n"));
    }
    if let Some(mcp_method) = mcp_method {
        head.push_str(&format!("Mcp-Method: {mcp_method}\r\n"));
    }
    if let Some(mcp_name) = mcp_name {
        head.push_str(&format!("Mcp-Name: {mcp_name}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    stream.write_all(head.as_bytes()).expect("write request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read response");
    let raw = String::from_utf8(raw).expect("response is UTF-8");
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .expect("response has headers and body");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())
        .expect("HTTP status");
    HttpResponse {
        status,
        headers: head.to_string(),
        body: body.to_string(),
    }
}

fn raw_request(addr: SocketAddr, raw: &str) -> HttpResponse {
    let mut stream = TcpStream::connect(addr).expect("connect to lease");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set client read timeout");
    stream.write_all(raw.as_bytes()).expect("write raw request");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read raw response");
    let response = String::from_utf8(response).expect("response is UTF-8");
    let (head, body) = response
        .split_once("\r\n\r\n")
        .expect("response has headers and body");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())
        .expect("HTTP status");
    HttpResponse {
        status,
        headers: head.to_string(),
        body: body.to_string(),
    }
}

fn json_request(addr: SocketAddr, token: &str, method: &str, value: Value) -> HttpResponse {
    let mcp_name = (method == "tools/call")
        .then(|| {
            value
                .get("params")
                .and_then(|params| params.get("name"))
                .cloned()
        })
        .flatten()
        .and_then(|name| name.as_str().map(ToString::to_string));
    request(
        addr,
        token,
        "POST",
        "/mcp",
        None,
        None,
        Some("application/json"),
        Some("2026-07-28"),
        Some(method),
        mcp_name.as_deref(),
        &value.to_string(),
    )
}

fn mutation_request(id: u64, key: &str, action: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {
            "name": "browser.operate",
            "arguments": {"action": action},
            "idempotencyKey": key
        }
    })
}

fn start_echo_lease() -> (agentcowork_mcp::McpHttpLease<EchoHandler>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let lease = McpServer::new(EchoHandler {
        calls: Arc::clone(&calls),
    })
    .into_http_lease()
    .expect("start lease");
    (lease, calls)
}

#[test]
fn initialize_list_and_call_follow_the_http_contract() {
    let (lease, calls) = start_echo_lease();
    let addr = lease.local_addr();
    let token = lease.token().to_string();

    let initialize = json_request(
        addr,
        &token,
        "initialize",
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2026-07-28",
                "capabilities": {},
                "clientInfo": {"name": "fixture", "version": "1"}
            }
        }),
    );
    assert_eq!(initialize.status, 200);
    assert_eq!(initialize.json()["result"]["protocolVersion"], "2026-07-28");
    assert!(
        initialize
            .headers
            .to_ascii_lowercase()
            .contains("mcp-protocol-version: 2026-07-28")
    );

    let list = json_request(
        addr,
        &token,
        "tools/list",
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    );
    assert_eq!(list.status, 200);
    let list_json = list.json();
    let names: Vec<&str> = list_json["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect();
    let expected: Vec<&str> = SHARED_FACADES.iter().map(|facade| facade.name).collect();
    assert_eq!(names, expected);
    assert!(
        list_json["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool["inputSchema"]["properties"]
                .as_object()
                .is_some_and(|props| props.is_empty()))
    );

    let call = json_request(
        addr,
        &token,
        "tools/call",
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "browser.extract", "arguments": {"query": "fixture"}}
        }),
    );
    assert_eq!(call.status, 200);
    assert_eq!(
        call.json()["result"]["structuredContent"]["tool"],
        "browser.extract"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    lease.close();
}

#[test]
fn duplicate_mutation_idempotency_key_replays_one_result() {
    let (lease, calls) = start_echo_lease();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": {
            "name": "browser.operate",
            "arguments": {"action": "fixture"},
            "idempotencyKey": "mutation-10"
        }
    })
    .to_string();
    let first = json_request(
        lease.local_addr(),
        lease.token(),
        "tools/call",
        serde_json::from_str(&body).unwrap(),
    );
    let second_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": {
            "name": "browser.operate",
            "arguments": {"action": "fixture"},
            "idempotencyKey": "mutation-10"
        }
    })
    .to_string();
    let second = json_request(
        lease.local_addr(),
        lease.token(),
        "tools/call",
        serde_json::from_str(&second_body).unwrap(),
    );
    assert_eq!(first.status, 200);
    assert_eq!(second.status, 200);
    assert_eq!(first.json()["id"], 10);
    assert_eq!(second.json()["id"], 11);
    assert_eq!(
        first.json()["result"]["structuredContent"],
        second.json()["result"]["structuredContent"]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    lease.close();
}

#[test]
fn handler_panic_marks_the_mutation_uncertain_and_never_replays_it() {
    let calls = Arc::new(AtomicUsize::new(0));
    let lease = McpServer::new(PanicHandler {
        calls: Arc::clone(&calls),
    })
    .into_http_lease()
    .expect("start panic lease");

    let first = json_request(
        lease.local_addr(),
        lease.token(),
        "tools/call",
        mutation_request(20, "panic-20", "panic"),
    );
    assert_eq!(first.status, 200);
    assert_eq!(first.json()["error"]["code"], -32603);
    assert!(
        first.json()["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("unknown"))
    );

    let second = json_request(
        lease.local_addr(),
        lease.token(),
        "tools/call",
        mutation_request(21, "panic-20", "panic"),
    );
    assert_eq!(second.status, 200);
    assert_eq!(second.json()["error"]["code"], -32603);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    lease.close();
}

#[test]
fn approval_pending_is_not_cached_as_a_terminal_mutation() {
    let calls = Arc::new(AtomicUsize::new(0));
    let approved = Arc::new(AtomicBool::new(false));
    let lease = McpServer::new(ApprovalHandler {
        calls: Arc::clone(&calls),
        approved: Arc::clone(&approved),
    })
    .into_http_lease()
    .expect("start approval lease");

    let denied = json_request(
        lease.local_addr(),
        lease.token(),
        "tools/call",
        mutation_request(30, "approval-30", "approve"),
    );
    assert_eq!(denied.status, 200);
    assert_eq!(denied.json()["error"]["code"], -32001);

    let changed = json_request(
        lease.local_addr(),
        lease.token(),
        "tools/call",
        mutation_request(32, "approval-30", "different"),
    );
    assert_eq!(changed.json()["error"]["code"], -32009);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    approved.store(true, Ordering::SeqCst);
    let approved_response = json_request(
        lease.local_addr(),
        lease.token(),
        "tools/call",
        mutation_request(31, "approval-30", "approve"),
    );
    assert_eq!(approved_response.status, 200);
    assert_eq!(
        approved_response.json()["result"]["structuredContent"]["approved"],
        true
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    lease.close();
}

#[test]
fn terminal_failures_replay_but_unknown_failures_remain_fenced() {
    let terminal_calls = Arc::new(AtomicUsize::new(0));
    let terminal_lease = McpServer::new(TerminalFailureHandler {
        calls: Arc::clone(&terminal_calls),
    })
    .into_http_lease()
    .expect("start terminal lease");
    let terminal_first = json_request(
        terminal_lease.local_addr(),
        terminal_lease.token(),
        "tools/call",
        mutation_request(40, "terminal-40", "reject"),
    );
    let terminal_second = json_request(
        terminal_lease.local_addr(),
        terminal_lease.token(),
        "tools/call",
        mutation_request(41, "terminal-40", "reject"),
    );
    assert_eq!(terminal_first.status, 200);
    assert_eq!(terminal_second.status, 200);
    assert_eq!(terminal_first.json()["error"]["code"], -32001);
    assert_eq!(terminal_second.json()["error"]["code"], -32001);
    assert_eq!(terminal_calls.load(Ordering::SeqCst), 1);
    terminal_lease.close();

    let unknown_calls = Arc::new(AtomicUsize::new(0));
    let unknown_lease = McpServer::new(UnknownFailureHandler {
        calls: Arc::clone(&unknown_calls),
    })
    .into_http_lease()
    .expect("start unknown lease");
    let unknown_first = json_request(
        unknown_lease.local_addr(),
        unknown_lease.token(),
        "tools/call",
        mutation_request(42, "unknown-42", "unknown"),
    );
    let unknown_second = json_request(
        unknown_lease.local_addr(),
        unknown_lease.token(),
        "tools/call",
        mutation_request(43, "unknown-42", "unknown"),
    );
    assert_eq!(unknown_first.json()["error"]["code"], -32004);
    assert_eq!(unknown_second.json()["error"]["code"], -32004);
    assert_eq!(unknown_calls.load(Ordering::SeqCst), 1);
    unknown_lease.close();
}

#[test]
fn changed_arguments_reject_an_existing_idempotency_key() {
    let calls = Arc::new(AtomicUsize::new(0));
    let lease = McpServer::new(EchoHandler {
        calls: Arc::clone(&calls),
    })
    .into_http_lease()
    .expect("start echo lease");
    let first = json_request(
        lease.local_addr(),
        lease.token(),
        "tools/call",
        mutation_request(50, "conflict-50", "first"),
    );
    let changed = json_request(
        lease.local_addr(),
        lease.token(),
        "tools/call",
        mutation_request(51, "conflict-50", "second"),
    );
    assert_eq!(first.status, 200);
    assert_eq!(changed.status, 200);
    assert_eq!(changed.json()["error"]["code"], -32009);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    lease.close();
}

#[test]
fn bounded_mutation_ledger_evicts_terminal_entries_for_future_mutations() {
    let calls = Arc::new(AtomicUsize::new(0));
    let lease = McpServer::new(EchoHandler {
        calls: Arc::clone(&calls),
    })
    .with_mutation_cache_capacity(2)
    .into_http_lease()
    .expect("start bounded lease");
    for (id, key) in [(60, "bounded-60"), (61, "bounded-61"), (62, "bounded-62")] {
        let response = json_request(
            lease.local_addr(),
            lease.token(),
            "tools/call",
            mutation_request(id, key, "bounded"),
        );
        assert_eq!(response.status, 200, "mutation {key} was rejected");
        assert!(response.json()["result"].is_object());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    lease.close();
}

#[test]
fn mutating_timeout_does_not_execute_a_second_time() {
    let calls = Arc::new(AtomicUsize::new(0));
    let lease = McpServer::new(SlowHandler {
        calls: Arc::clone(&calls),
        delay: Duration::from_millis(900),
    })
    .into_http_lease()
    .expect("start slow lease");
    let first_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 77,
        "method": "tools/call",
        "params": {
            "name": "browser.operate",
            "arguments": {"action": "slow"},
            "idempotencyKey": "timeout-77"
        }
    });
    let first = json_request(
        lease.local_addr(),
        lease.token(),
        "tools/call",
        first_body.clone(),
    );
    assert_eq!(first.status, 408);
    assert_eq!(first.json()["id"], 77);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let second_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 78,
        "method": "tools/call",
        "params": {
            "name": "browser.operate",
            "arguments": {"action": "slow"},
            "idempotencyKey": "timeout-77"
        }
    });
    let second = json_request(lease.local_addr(), lease.token(), "tools/call", second_body);
    assert_eq!(second.status, 200);
    assert_eq!(second.json()["id"], 78);
    assert_eq!(
        second.json()["result"]["structuredContent"]["completed"],
        true
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    lease.close();
}

#[test]
fn loopback_pool_speaks_the_same_strict_headers() {
    let (lease, _calls) = start_echo_lease();
    let mut pool = LoopbackPool::connect(lease.local_addr(), Some(lease.token().to_string()));
    let response = pool
        .request(
            &serde_json::json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list"}).to_string(),
        )
        .expect("pooled request");
    assert!(response.contains("\"tools\""));
    pool.close();
    lease.close();
}

#[test]
fn notification_is_202_with_no_body() {
    let (lease, _calls) = start_echo_lease();
    let response = json_request(
        lease.local_addr(),
        lease.token(),
        "notifications/initialized",
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    );
    assert_eq!(response.status, 202);
    assert!(response.body.is_empty());
    assert!(
        !response
            .headers
            .to_ascii_lowercase()
            .contains("content-type:")
    );
    lease.close();
}

#[test]
fn strict_accept_and_framing_failures_are_json_protocol_errors() {
    let (lease, _calls) = start_echo_lease();
    let addr = lease.local_addr();
    let token = lease.token();
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 41, "method": "tools/list", "params": {}
    })
    .to_string();
    let no_accept = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/list\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}",
        port = addr.port(),
        length = body.len(),
        body = body,
        token = token,
    );
    let no_accept = raw_request(addr, &no_accept);
    assert_eq!(no_accept.status, 406);
    assert!(
        no_accept
            .headers
            .to_ascii_lowercase()
            .contains("content-type: application/json")
    );
    assert_eq!(no_accept.json()["id"], 41);

    let wrong_accept = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: application/json\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/list\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}",
        port = addr.port(),
        length = body.len(),
        body = body,
        token = token,
    );
    assert_eq!(raw_request(addr, &wrong_accept).status, 406);

    let missing_length = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/list\r\nConnection: close\r\n\r\n{body}",
        port = addr.port(),
        body = body,
        token = token,
    );
    let missing_length = raw_request(addr, &missing_length);
    assert_eq!(missing_length.status, 411);
    assert!(
        missing_length
            .headers
            .to_ascii_lowercase()
            .contains("content-type: application/json")
    );
    assert_eq!(missing_length.json()["error"]["code"], -32600);
    lease.close();
}

#[test]
fn wrong_method_content_type_host_origin_and_path_are_refused() {
    let (lease, _calls) = start_echo_lease();
    let addr = lease.local_addr();
    let token = lease.token().to_string();
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string();

    let wrong_method = request(
        addr,
        &token,
        "GET",
        "/mcp",
        None,
        None,
        Some("application/json"),
        Some("2026-07-28"),
        Some("tools/list"),
        None,
        &body,
    );
    assert_eq!(wrong_method.status, 405);

    let wrong_type = request(
        addr,
        &token,
        "POST",
        "/mcp",
        None,
        None,
        Some("text/plain"),
        Some("2026-07-28"),
        Some("tools/list"),
        None,
        &body,
    );
    assert_eq!(wrong_type.status, 415);

    let wrong_host = request(
        addr,
        &token,
        "POST",
        "/mcp",
        Some(&format!("localhost.evil:{}", addr.port())),
        None,
        Some("application/json"),
        Some("2026-07-28"),
        Some("tools/list"),
        None,
        &body,
    );
    assert_eq!(wrong_host.status, 403);

    let wrong_origin = request(
        addr,
        &token,
        "POST",
        "/mcp",
        None,
        Some(&format!("http://localhost.evil:{}", addr.port())),
        Some("application/json"),
        Some("2026-07-28"),
        Some("tools/list"),
        None,
        &body,
    );
    assert_eq!(wrong_origin.status, 403);

    let wrong_path = request(
        addr,
        &token,
        "POST",
        "/not-mcp",
        None,
        None,
        Some("application/json"),
        Some("2026-07-28"),
        Some("tools/list"),
        None,
        &body,
    );
    assert_eq!(wrong_path.status, 404);
    lease.close();
}

#[test]
fn protocol_negotiation_and_metadata_mismatch_fail_closed() {
    let (lease, _calls) = start_echo_lease();
    let addr = lease.local_addr();
    let token = lease.token().to_string();
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": "not-a-real-version"}
    })
    .to_string();
    let negotiated = request(
        addr,
        &token,
        "POST",
        "/mcp",
        None,
        None,
        Some("application/json"),
        Some("2026-07-28"),
        Some("initialize"),
        None,
        &initialize,
    );
    assert_eq!(negotiated.status, 200);
    assert_eq!(negotiated.json()["result"]["protocolVersion"], "2026-07-28");
    assert_ne!(
        negotiated.json()["result"]["protocolVersion"],
        "not-a-real-version"
    );

    let mismatch = request(
        addr,
        &token,
        "POST",
        "/mcp",
        None,
        None,
        Some("application/json"),
        Some("2099-01-01"),
        Some("tools/list"),
        None,
        &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}).to_string(),
    );
    assert_eq!(mismatch.status, 400);

    let missing_protocol = request(
        addr,
        &token,
        "POST",
        "/mcp",
        None,
        None,
        Some("application/json"),
        None,
        Some("tools/list"),
        None,
        &serde_json::json!({"jsonrpc":"2.0","id":6,"method":"tools/list"}).to_string(),
    );
    assert_eq!(missing_protocol.status, 400);
    assert_eq!(missing_protocol.json()["id"], 6);

    let legacy_protocol = request(
        addr,
        &token,
        "POST",
        "/mcp",
        None,
        None,
        Some("application/json"),
        Some("2025-06-18"),
        Some("tools/list"),
        None,
        &serde_json::json!({"jsonrpc":"2.0","id":7,"method":"tools/list"}).to_string(),
    );
    assert_eq!(legacy_protocol.status, 400);

    let method_mismatch = request(
        addr,
        &token,
        "POST",
        "/mcp",
        None,
        None,
        Some("application/json"),
        Some("2026-07-28"),
        Some("tools/list"),
        None,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "ping"
        })
        .to_string(),
    );
    assert_eq!(method_mismatch.status, 400);

    let malformed = request(
        addr,
        &token,
        "POST",
        "/mcp",
        None,
        None,
        Some("application/json"),
        Some("2026-07-28"),
        Some("tools/list"),
        None,
        "not-json",
    );
    assert_eq!(malformed.status, 400);

    let malformed_rpc = request(
        addr,
        &token,
        "POST",
        "/mcp",
        None,
        None,
        Some("application/json"),
        Some("2026-07-28"),
        Some("tools/list"),
        None,
        &serde_json::json!({"jsonrpc": "1.0", "id": 4, "method": "tools/list"}).to_string(),
    );
    assert_eq!(malformed_rpc.status, 400);

    let name_mismatch = request(
        addr,
        &token,
        "POST",
        "/mcp",
        None,
        None,
        Some("application/json"),
        Some("2026-07-28"),
        Some("tools/call"),
        Some("office.edit"),
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": {"name": "browser.extract", "arguments": {}}
        })
        .to_string(),
    );
    assert_eq!(name_mismatch.status, 400);
    lease.close();
}

#[test]
fn bearer_isolation_and_shared_only_admission_are_per_lease() {
    let (first, first_calls) = start_echo_lease();
    let (second, second_calls) = start_echo_lease();
    let list = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string();

    let wrong = request(
        second.local_addr(),
        first.token(),
        "POST",
        "/mcp",
        None,
        None,
        Some("application/json"),
        Some("2026-07-28"),
        Some("tools/list"),
        None,
        &list,
    );
    assert_eq!(wrong.status, 401);

    let guessed = request(
        first.local_addr(),
        first.token(),
        "POST",
        "/mcp",
        None,
        None,
        Some("application/json"),
        Some("2026-07-28"),
        Some("tools/call"),
        Some("snapshot"),
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "snapshot", "arguments": {}}
        })
        .to_string(),
    );
    assert_eq!(guessed.status, 200);
    assert_eq!(guessed.json()["error"]["code"], -32602);
    assert_eq!(first_calls.load(Ordering::SeqCst), 0);
    assert_eq!(second_calls.load(Ordering::SeqCst), 0);
    first.close();
    second.close();
}

#[test]
fn shutdown_is_bounded_when_handler_never_returns() {
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let mut lease = McpServer::new(BlockingHandler {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
    })
    .with_shutdown_timeout(Duration::from_millis(100))
    .into_http_lease()
    .expect("start blocking lease");
    let addr = lease.local_addr();
    let token = lease.token().to_string();
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "browser.extract", "arguments": {}}
    })
    .to_string();
    let request_thread = thread::spawn(move || {
        request(
            addr,
            &token,
            "POST",
            "/mcp",
            None,
            None,
            Some("application/json"),
            Some("2026-07-28"),
            Some("tools/call"),
            Some("browser.extract"),
            &body,
        );
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while !entered.load(Ordering::SeqCst) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert!(entered.load(Ordering::SeqCst), "handler entered");

    let started = Instant::now();
    lease.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "shutdown must be bounded, took {:?}",
        started.elapsed()
    );
    release.store(true, Ordering::SeqCst);
    let _ = request_thread.join();
}

#[test]
fn partial_http_request_has_a_bounded_deadline() {
    let (lease, _calls) = start_echo_lease();
    let mut stream = TcpStream::connect(lease.local_addr()).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    stream
        .write_all(b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\n")
        .expect("write partial headers");
    thread::sleep(Duration::from_millis(900));
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    assert!(
        response.starts_with("HTTP/1.1 408")
            || response.is_empty()
            || response.contains("HTTP/1.1 400"),
        "partial request must not wait forever: {response:?}"
    );
    lease.close();
}

#[test]
fn shutdown_closes_idle_active_connections() {
    let (lease, _calls) = start_echo_lease();
    let addr = lease.local_addr();
    let mut idle = TcpStream::connect(addr).expect("open active connection");
    idle.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    thread::sleep(Duration::from_millis(30));
    lease.close();
    let mut byte = [0u8; 1];
    let result = idle.read(&mut byte);
    assert!(
        result.is_err() || matches!(result, Ok(0)),
        "active connection must be closed"
    );
    assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err());
}
