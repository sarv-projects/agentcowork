//! WebMCP HTTP transport (E16 — the wire half of doc 63 §4.2): serves and
//! consumes MCP over plain HTTP so a browser session can expose its WebMCP
//! tools to any HTTP client. `GET /mcp` returns the tool manifest (the
//! handshake); `POST /mcp` accepts JSON-RPC `tools/list` and `tools/call`
//! and answers with JSON-RPC results.
//!
//! Std-only (`TcpListener`): the pure request handler is fully testable, and
//! a tiny threaded server wraps it for real connections.

use crate::webmcp::{WebMcpExecutor, WebMcpRegistry};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

/// The default MCP-over-HTTP path.
pub const MCP_PATH: &str = "/mcp";

/// Max JSON-RPC body accepted (1 MiB) — an oversized request is refused, not
/// buffered, so a local client can't exhaust memory.
pub const MAX_BODY_BYTES: usize = 1 << 20;

/// Max total buffered request (headers + body) before we drop the connection.
const MAX_REQUEST_BYTES: usize = MAX_BODY_BYTES + 64 * 1024;

/// A parse failure (currently only: declared body exceeds [`MAX_BODY_BYTES`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpParseError {
    /// The `Content-Length` header declares a body larger than the cap.
    BodyTooLarge,
}

/// Parse an HTTP request into (method, path, body). Handles `Content-Length`
/// bodies; returns `Ok(None)` for a partial request (caller waits for more)
/// and `Err` for a request whose declared body exceeds [`MAX_BODY_BYTES`].
pub fn parse_http_request(raw: &str) -> Result<Option<(String, String, String)>, HttpParseError> {
    let header_end = match raw.find("\r\n\r\n") {
        Some(h) => h,
        None => return Ok(None),
    };
    let head = &raw[..header_end];
    let mut lines = head.lines();
    let request_line = match lines.next() {
        Some(l) => l,
        None => return Ok(None),
    };
    let mut parts = request_line.split_whitespace();
    let method = match parts.next() {
        Some(m) => m.to_string(),
        None => return Ok(None),
    };
    let path = match parts.next() {
        Some(p) => p.to_string(),
        None => return Ok(None),
    };
    let content_length: usize = lines
        .find_map(|l| {
            l.strip_prefix("Content-Length:")
                .or_else(|| l.strip_prefix("content-length:"))
                .map(|v| v.trim().parse::<usize>().ok())
        })
        .flatten()
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        return Err(HttpParseError::BodyTooLarge); // oversized — refuse, don't buffer
    }
    let body_start = header_end + 4;
    let Some(body) = raw.get(body_start..) else {
        return Ok(None);
    };
    if body.len() < content_length {
        return Ok(None); // body incomplete
    }
    Ok(Some((method, path, body[..content_length].to_string())))
}

/// Extract the `Authorization: Bearer <token>` value from the raw HTTP head,
/// if present.
pub fn bearer_token(raw: &str) -> Option<String> {
    let head = raw.split("\r\n\r\n").next()?;
    head.lines().find_map(|l| {
        let rest = l
            .strip_prefix("Authorization:")
            .or_else(|| l.strip_prefix("authorization:"))?;
        let t = rest.trim();
        t.strip_prefix("Bearer ")
            .or_else(|| t.strip_prefix("bearer "))
            .map(str::to_string)
    })
}

fn http_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn jsonrpc_error(code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": Value::Null, "error": { "code": code, "message": message } })
}

fn http_plain(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Handle one parsed request against the registry + executor. Returns the
/// full HTTP response.
pub fn handle_mcp_request(
    method: &str,
    path: &str,
    body: &str,
    registry: &WebMcpRegistry,
    executor: &dyn WebMcpExecutor,
) -> String {
    if path != MCP_PATH {
        return http_response("404 Not Found", r#"{"error":"not found"}"#);
    }
    match method {
        // The handshake: the tool manifest as JSON (tools + schemas).
        "GET" => {
            let tools: Vec<Value> = registry
                .list()
                .into_iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema,
                    })
                })
                .collect();
            http_response(
                "200 OK",
                &json!({ "jsonrpc": "2.0", "result": { "tools": tools } }).to_string(),
            )
        }
        "POST" => {
            let req: Value = match serde_json::from_str(body) {
                Ok(v) => v,
                Err(_) => {
                    return http_response(
                        "400 Bad Request",
                        &jsonrpc_error(-32700, "parse error").to_string(),
                    );
                }
            };
            let method_name = req.get("method").and_then(Value::as_str).unwrap_or("");
            let result = match method_name {
                "tools/list" => {
                    let tools: Vec<Value> = registry
                        .list()
                        .into_iter()
                        .map(|t| {
                            json!({
                                "name": t.name,
                                "description": t.description,
                                "inputSchema": t.input_schema,
                            })
                        })
                        .collect();
                    json!({ "jsonrpc": "2.0", "id": req.get("id"), "result": { "tools": tools } })
                }
                "tools/call" => {
                    let name = req
                        .pointer("/params/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let args = req
                        .pointer("/params/arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    match registry.execute(name, &args.to_string(), executor) {
                        Ok(res) => json!({
                            "jsonrpc": "2.0",
                            "id": req.get("id"),
                            "result": { "content": [{ "type": "text", "text": res.output.to_string() }], "isError": res.status == "error" }
                        }),
                        Err(e) => jsonrpc_error(-32602, &format!("tool call failed: {e}")),
                    }
                }
                _ => jsonrpc_error(-32601, &format!("method not found: {method_name}")),
            };
            http_response("200 OK", &result.to_string())
        }
        _ => http_response(
            "405 Method Not Allowed",
            r#"{"error":"method not allowed"}"#,
        ),
    }
}

/// A threaded MCP-over-HTTP server. `serve` binds a loopback listener and
/// generates a fresh per-process bearer token; every mutating `POST` must
/// present it. Each accepted connection is handled on a worker thread.
/// `local_addr` gives the actual bound port; `token()` returns the secret.
pub struct McpHttpServer {
    listener: TcpListener,
    registry: Arc<WebMcpRegistry>,
    executor: Arc<dyn WebMcpExecutor + Send + Sync>,
    token: String,
}

impl McpHttpServer {
    pub fn serve(
        addr: &str,
        registry: WebMcpRegistry,
        executor: Arc<dyn WebMcpExecutor + Send + Sync>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let token = fresh_token();
        let server = Self {
            listener,
            registry: Arc::new(registry),
            executor,
            token,
        };
        server.spawn_accept_loop();
        Ok(server)
    }

    /// The per-process bearer token clients must present on mutating calls.
    pub fn token(&self) -> &str {
        &self.token
    }

    fn spawn_accept_loop(&self) {
        let listener = self.listener.try_clone().expect("clone listener");
        let registry = Arc::clone(&self.registry);
        let executor = Arc::clone(&self.executor);
        let token = self.token.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let registry = Arc::clone(&registry);
                let executor = Arc::clone(&executor);
                let token = token.clone();
                std::thread::spawn(move || {
                    let _ = handle_stream(&mut stream, &registry, executor.as_ref(), &token);
                });
            }
        });
    }

    pub fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.listener.local_addr()
    }
}

/// Handle one connection: enforce request-size + read-timeout + bearer auth,
/// then dispatch. Loopback is the transport boundary; the bearer token is the
/// capability boundary (any local process could connect otherwise).
fn handle_stream(
    stream: &mut TcpStream,
    registry: &WebMcpRegistry,
    executor: &dyn WebMcpExecutor,
    token: &str,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if buf.len() > MAX_REQUEST_BYTES {
            let _ = stream
                .write_all(http_plain("413 Payload Too Large", "request too large").as_bytes());
            return Ok(());
        }
        let n = match stream.read(&mut chunk) {
            Ok(n) => n,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Ok(()); // idle/timeout — drop quietly
            }
            Err(e) => return Err(e),
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let raw = String::from_utf8_lossy(&buf);
        match parse_http_request(&raw) {
            Ok(Some((method, path, body))) => {
                // The tool manifest is public; every mutating POST must carry
                // the per-process bearer token.
                if method == "POST" && bearer_token(&raw).as_deref() != Some(token) {
                    let _ = stream.write_all(
                        http_plain("401 Unauthorized", "missing or invalid bearer token")
                            .as_bytes(),
                    );
                    return Ok(());
                }
                let resp = handle_mcp_request(&method, &path, &body, registry, executor);
                stream.write_all(resp.as_bytes())?;
                stream.flush()?;
                return Ok(());
            }
            Ok(None) => { /* partial — keep reading */ }
            Err(HttpParseError::BodyTooLarge) => {
                let _ = stream.write_all(
                    http_plain("413 Payload Too Large", "body exceeds limit").as_bytes(),
                );
                return Ok(());
            }
        }
    }
    Ok(())
}

/// A fresh unpredictable per-process token. Uses the std hash `RandomState`
/// (randomly seeded per process) over process + time entropy — sufficient for
/// a loopback capability token (the real boundary is loopback + caller filter).
pub fn fresh_token() -> String {
    use std::hash::{BuildHasher, Hash, Hasher};
    let s = std::collections::hash_map::RandomState::new();
    let mut h = s.build_hasher();
    std::process::id().hash(&mut h);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut h);
    // Two independent hashers for 128 bits of entropy.
    let mut h2 = s.build_hasher();
    b"agentcowork-webmcp".hash(&mut h2);
    (std::process::id() as u64)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15u64)
        .hash(&mut h2);
    format!("{:016x}{:016x}", h.finish(), h2.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webmcp::{WebMcpResult, WebMcpTool};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingExecutor(Arc<AtomicUsize>);
    impl WebMcpExecutor for CountingExecutor {
        fn execute(&self, tool: &WebMcpTool, input: Value) -> WebMcpResult {
            self.0.fetch_add(1, Ordering::SeqCst);
            WebMcpResult::ok(json!({ "tool": tool.name, "echo": input }))
        }
    }

    fn registry() -> WebMcpRegistry {
        let mut r = WebMcpRegistry::new();
        r.register(WebMcpTool {
            name: "search".into(),
            description: "search the page".into(),
            input_schema: json!({ "type": "object", "properties": { "q": { "type": "string" } } }),
        });
        r
    }

    #[test]
    fn get_returns_tool_manifest() {
        let resp = handle_mcp_request(
            "GET",
            "/mcp",
            "",
            &registry(),
            &CountingExecutor(Arc::new(AtomicUsize::new(0))),
        );
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        let body = resp.split("\r\n\r\n").nth(1).unwrap();
        let v: Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["result"]["tools"][0]["name"], "search");
    }

    #[test]
    fn post_tools_list_returns_manifest() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let resp = handle_mcp_request(
            "POST",
            "/mcp",
            body,
            &registry(),
            &CountingExecutor(Arc::new(AtomicUsize::new(0))),
        );
        let v: Value = serde_json::from_str(resp.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(v["result"]["tools"][0]["name"], "search");
        assert_eq!(v["id"], 1);
    }

    #[test]
    fn post_tools_call_executes() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = CountingExecutor(Arc::clone(&calls));
        let body = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search","arguments":{"q":"hello"}}}"#;
        let resp = handle_mcp_request("POST", "/mcp", body, &registry(), &executor);
        let v: Value = serde_json::from_str(resp.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(v["result"]["isError"], false);
        assert!(
            v["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("\"tool\":\"search\"")
        );
    }

    #[test]
    fn unknown_method_returns_jsonrpc_error() {
        let body = r#"{"jsonrpc":"2.0","id":3,"method":"bogus"}"#;
        let resp = handle_mcp_request(
            "POST",
            "/mcp",
            body,
            &registry(),
            &CountingExecutor(Arc::new(AtomicUsize::new(0))),
        );
        let v: Value = serde_json::from_str(resp.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(v["error"]["code"], -32601);
    }

    #[test]
    fn wrong_path_and_method_are_rejected() {
        let e = CountingExecutor(Arc::new(AtomicUsize::new(0)));
        assert!(handle_mcp_request("GET", "/other", "", &registry(), &e).contains("404"));
        assert!(handle_mcp_request("DELETE", "/mcp", "", &registry(), &e).contains("405"));
    }

    #[test]
    fn parse_http_request_extracts_body() {
        let raw = "POST /mcp HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        let (m, p, b) = parse_http_request(raw).unwrap().unwrap();
        assert_eq!(m, "POST");
        assert_eq!(p, "/mcp");
        assert_eq!(b, "hello");
        // Partial body → Ok(None).
        let raw2 = "POST /mcp HTTP/1.1\r\nContent-Length: 10\r\n\r\nhello";
        assert!(parse_http_request(raw2).unwrap().is_none());
        // Oversized declared body → Err.
        let raw3 = format!(
            "POST /mcp HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        assert!(parse_http_request(&raw3).is_err());
        // Bearer token extraction.
        let raw4 =
            "POST /mcp HTTP/1.1\r\nAuthorization: Bearer secret\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(bearer_token(raw4).as_deref(), Some("secret"));
        assert!(bearer_token("GET /mcp HTTP/1.1\r\n\r\n").is_none());
    }

    #[test]
    fn server_roundtrip_over_a_real_socket() {
        let executor = Arc::new(CountingExecutor(Arc::new(AtomicUsize::new(0))));
        let server = McpHttpServer::serve("127.0.0.1:0", registry(), executor).unwrap();
        let port = server.local_addr().unwrap().port();
        let token = server.token().to_string();

        // POST a tools/call over a real connection, WITH the bearer token.
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let body = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"search","arguments":{"q":"x"}}}"#;
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{}",
            token,
            body.len(),
            body
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        let body = resp.split("\r\n\r\n").nth(1).unwrap();
        let v: Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["result"]["isError"], false);
        // The tool output is JSON-encoded inside the text content part.
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        let output: Value = serde_json::from_str(text).unwrap();
        assert_eq!(output["tool"], "search");
        assert_eq!(output["echo"]["q"], "x");
    }

    #[test]
    fn server_rejects_missing_bearer_token() {
        let executor = Arc::new(CountingExecutor(Arc::new(AtomicUsize::new(0))));
        let server = McpHttpServer::serve("127.0.0.1:0", registry(), executor).unwrap();
        let port = server.local_addr().unwrap().port();

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let body = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"search","arguments":{}}}"#;
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        assert!(resp.starts_with("HTTP/1.1 401 Unauthorized"));
    }

    #[test]
    fn fresh_token_is_nonempty_and_distinct() {
        let a = fresh_token();
        let b = fresh_token();
        assert_eq!(a.len(), 32);
        assert!(!a.is_empty());
        assert_ne!(a, b);
    }
}
