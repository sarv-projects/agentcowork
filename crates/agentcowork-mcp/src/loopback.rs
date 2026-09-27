//! P39.3 — loopback MCP client with keep-alive + connection pooling.
//!
//! The host side of the loopback HTTP transport: repeated tool calls from one
//! agent reuse a single pooled TCP connection instead of paying a fresh TCP
//! handshake per request (`serve_http_once` was one-request-per-connection).
//! MRTR (multi-round-trip continuation) stays the long-running path — this
//! pool is purely the short-call hot path.
//!
//! The pool is deliberately tiny (one connection): the loopback server is
//! single-threaded per connection, so a pool of N would serialize anyway.
//! Stats are exposed for the debug counter so tests (and the perf harness)
//! can assert the handshake cost is actually gone.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::server::SUPPORTED_PROTOCOL_VERSION;

/// Connection reuse statistics — the P39.3 acceptance counter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolStats {
    /// Fresh TCP connections established.
    pub opened: u64,
    /// Requests served over a reused (already-open) connection.
    pub reused: u64,
    /// Requests that failed and had to retry on a fresh connection.
    pub retried: u64,
}

fn safe_header_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.chars().any(|character| character.is_control())
}

fn valid_header_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn body_is_mutating_call(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    if value.get("method").and_then(serde_json::Value::as_str) != Some("tools/call") {
        return false;
    }
    let Some(name) = value
        .get("params")
        .and_then(|params| params.get("name"))
        .and_then(serde_json::Value::as_str)
    else {
        return true;
    };
    if let Some(facade) = crate::find_facade(name) {
        return !facade.read_only;
    }
    if let Some(tool) = crate::find_inbuilt_tool(name) {
        return !tool.read_only;
    }
    // An unknown/external name is handled by the server's catalog. Treat it
    // conservatively: never transparently retry an effect whose annotation
    // cannot be inspected here.
    true
}

fn explicit_idempotency_key(value: &serde_json::Value) -> Option<String> {
    let candidate = value
        .get("idempotencyKey")
        .or_else(|| value.get("idempotency_key"))
        .or_else(|| {
            value
                .get("params")
                .and_then(|params| params.get("idempotencyKey"))
        })
        .or_else(|| {
            value
                .get("params")
                .and_then(|params| params.get("idempotency_key"))
        })
        .or_else(|| {
            value
                .get("params")
                .and_then(|params| params.get("_meta"))
                .and_then(|meta| meta.get("idempotencyKey"))
        })
        .or_else(|| {
            value
                .get("params")
                .and_then(|params| params.get("_meta"))
                .and_then(|meta| meta.get("idempotency_key"))
        })
        .or_else(|| {
            value
                .get("params")
                .and_then(|params| params.get("arguments"))
                .and_then(|arguments| arguments.get("idempotencyKey"))
        })
        .or_else(|| {
            value
                .get("params")
                .and_then(|params| params.get("arguments"))
                .and_then(|arguments| arguments.get("idempotency_key"))
        })?;
    candidate.as_str().and_then(|key| {
        (!key.is_empty()
            && key.len() <= 256
            && !key
                .chars()
                .any(|character| character.is_control() || character.is_whitespace()))
        .then(|| key.to_string())
    })
}

fn stable_idempotency_key(body: &str) -> String {
    let explicit = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| explicit_idempotency_key(&value));
    explicit.unwrap_or_else(|| {
        let digest = Sha256::digest(body.as_bytes());
        format!(
            "mcp-auto:{}",
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    })
}

const MAX_RESPONSE_HEADER_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;

fn invalid_response(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

fn request_id(body: &str) -> std::io::Result<Option<serde_json::Value>> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("request is not valid JSON: {error}"),
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "JSON-RPC request must be an object",
        )
    })?;
    if object.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "JSON-RPC request must use version 2.0",
        ));
    }
    if object
        .get("method")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|method| method.is_empty() || method.chars().any(char::is_control))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "JSON-RPC request method is invalid",
        ));
    }
    match object.get("id") {
        None => Ok(None),
        Some(value) if value.is_string() || value.is_number() || value.is_null() => {
            Ok(Some(value.clone()))
        }
        Some(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "JSON-RPC request id is invalid",
        )),
    }
}

fn is_json_content_type(value: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
}

fn validate_json_rpc_response(body: &str, expected_id: &serde_json::Value) -> std::io::Result<()> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| invalid_response(format!("response is not valid JSON: {error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| invalid_response("JSON-RPC response must be an object"))?;
    if object.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0") {
        return Err(invalid_response("response jsonrpc must be exactly 2.0"));
    }
    if object.contains_key("method") {
        return Err(invalid_response("JSON-RPC response contains a method"));
    }
    let response_id = object
        .get("id")
        .ok_or_else(|| invalid_response("JSON-RPC response is missing id"))?;
    if response_id != expected_id {
        return Err(invalid_response(
            "JSON-RPC response id does not match request",
        ));
    }
    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");
    if has_result == has_error {
        return Err(invalid_response(
            "JSON-RPC response must contain exactly one of result or error",
        ));
    }
    if let Some(error) = object.get("error") {
        let error = error
            .as_object()
            .ok_or_else(|| invalid_response("JSON-RPC error must be an object"))?;
        if !error
            .get("code")
            .is_some_and(|code| code.is_i64() || code.is_u64())
            || !error
                .get("message")
                .is_some_and(|message| message.is_string())
        {
            return Err(invalid_response("JSON-RPC error shape is invalid"));
        }
    }
    Ok(())
}

/// A single-slot loopback connection pool with transparent reconnect.
pub struct LoopbackPool {
    addr: SocketAddr,
    bearer: Option<String>,
    stream: Option<TcpStream>,
    read_buffer: Vec<u8>,
    /// 0 = pool is closed (explicit drop / fatal error).
    pub stats: PoolStats,
}

impl LoopbackPool {
    pub fn connect(addr: SocketAddr, bearer: Option<String>) -> Self {
        Self {
            addr,
            bearer,
            stream: None,
            read_buffer: Vec::new(),
            stats: PoolStats::default(),
        }
    }

    /// Send one JSON-RPC request and return the JSON-RPC response body.
    /// Reuses the pooled connection when healthy; reconnects once on failure.
    pub fn request(&mut self, body: &str) -> std::io::Result<String> {
        let retryable = !body_is_mutating_call(body);
        match self.request_once(body) {
            Ok(resp) => {
                self.stats.reused += 1;
                Ok(resp)
            }
            Err(first_err) => {
                // A transport error or non-2xx response after a mutating call
                // is uncertain: the server may already have executed the tool.
                // Never replay it automatically. The caller can safely retry
                // the identical request; the server's idempotency ledger then
                // returns a known terminal result or an explicit uncertainty.
                self.stream = None;
                self.read_buffer.clear();
                if first_err.kind() == std::io::ErrorKind::InvalidInput || !retryable {
                    return Err(first_err);
                }
                // The pooled connection may have died (server restart, idle
                // timeout). Drop it and try exactly once on a fresh socket.
                self.stats.retried += 1;
                match self.request_once(body) {
                    Ok(response) => Ok(response),
                    Err(_) => {
                        self.stream = None;
                        self.read_buffer.clear();
                        Err(first_err)
                    }
                }
            }
        }
    }

    /// Close the pooled connection (e.g. when tearing down a session).
    pub fn close(&mut self) {
        self.stream = None;
        self.read_buffer.clear();
    }

    fn request_once(&mut self, body: &str) -> std::io::Result<String> {
        let expected_id = request_id(body)?;
        if self.stream.is_none() {
            self.stream = Some(self.open()?);
        }
        let stream = self.stream.as_mut().expect("stream just ensured");
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;

        let mut head = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\nOrigin: http://127.0.0.1:{}\r\n",
            self.addr.port(),
            body.len(),
            self.addr.port(),
        );
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(method) = value.get("method").and_then(serde_json::Value::as_str) {
                let protocol = SUPPORTED_PROTOCOL_VERSION;
                head.push_str(&format!("MCP-Protocol-Version: {protocol}\r\n"));
                if safe_header_value(method) {
                    head.push_str(&format!("Mcp-Method: {method}\r\n"));
                }
                if method == "tools/call" {
                    if let Some(name) = value
                        .get("params")
                        .and_then(|params| params.get("name"))
                        .and_then(serde_json::Value::as_str)
                    {
                        if safe_header_value(name) {
                            head.push_str(&format!("Mcp-Name: {name}\r\n"));
                        }
                    }
                    if body_is_mutating_call(body) {
                        head.push_str(&format!(
                            "Idempotency-Key: {}\r\n",
                            stable_idempotency_key(body)
                        ));
                    }
                }
            }
        }
        if let Some(token) = &self.bearer {
            if safe_header_value(token) {
                head.push_str(&format!("Authorization: Bearer {token}\r\n"));
            }
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.write_all(body.as_bytes())?;
        stream.flush()?;

        let read_buffer = &mut self.read_buffer;
        read_http_response(stream, read_buffer, expected_id.as_ref())
    }

    fn open(&mut self) -> std::io::Result<TcpStream> {
        let stream = TcpStream::connect(self.addr)?;
        self.stats.opened += 1;
        Ok(stream)
    }
}

/// Read one strictly framed HTTP response from the stream.
fn read_http_response(
    stream: &mut TcpStream,
    buffered: &mut Vec<u8>,
    expected_id: Option<&serde_json::Value>,
) -> std::io::Result<String> {
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(position) = buffered.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        if buffered.len() > MAX_RESPONSE_HEADER_BYTES {
            return Err(invalid_response("response headers too large"));
        }
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "loopback server closed before a complete response",
            ));
        }
        buffered.extend_from_slice(&chunk[..read]);
    };
    if header_end > MAX_RESPONSE_HEADER_BYTES {
        return Err(invalid_response("response headers too large"));
    }
    let header = std::str::from_utf8(&buffered[..header_end])
        .map_err(|_| invalid_response("response headers are not valid UTF-8"))?;
    let mut lines = header.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| invalid_response("response status line is missing"))?;
    let status_parts: Vec<&str> = status_line.split(' ').collect();
    if status_parts.len() != 3
        || status_parts[0] != "HTTP/1.1"
        || status_parts[1].len() != 3
        || !status_parts[1].bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid_response("malformed HTTP status line"));
    }
    let status: u16 = status_parts[1]
        .parse()
        .map_err(|_| invalid_response("malformed HTTP status code"))?;

    let mut headers = std::collections::BTreeMap::<String, String>::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| invalid_response("malformed response header"))?;
        if name.trim() != name || !valid_header_name(name) {
            return Err(invalid_response("malformed response header name"));
        }
        let value = value.trim();
        if value
            .chars()
            .any(|character| character.is_control() && character != '\t')
        {
            return Err(invalid_response("malformed response header"));
        }
        if headers
            .insert(name.to_ascii_lowercase(), value.to_string())
            .is_some()
        {
            return Err(invalid_response("duplicate response header"));
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err(invalid_response(
            "response transfer-encoding is unsupported",
        ));
    }
    let content_length = headers
        .get("content-length")
        .ok_or_else(|| invalid_response("response Content-Length is required"))?;
    if content_length.is_empty() || !content_length.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_response("response Content-Length is invalid"));
    }
    let content_length: usize = content_length
        .parse()
        .map_err(|_| invalid_response("response Content-Length is invalid"))?;
    if content_length > MAX_RESPONSE_BODY_BYTES {
        return Err(invalid_response("response body is too large"));
    }
    let protocol = headers
        .get("mcp-protocol-version")
        .map(String::as_str)
        .ok_or_else(|| invalid_response("response MCP protocol metadata is missing"))?;
    if protocol != SUPPORTED_PROTOCOL_VERSION {
        return Err(invalid_response(
            "response MCP protocol metadata is unsupported",
        ));
    }
    if status != 200 && status != 202 {
        return Err(std::io::Error::other(format!(
            "loopback server returned HTTP {status}"
        )));
    }

    let body_end = header_end
        .checked_add(content_length)
        .ok_or_else(|| invalid_response("response Content-Length overflows"))?;
    while buffered.len() < body_end {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "loopback server closed before a complete response body",
            ));
        }
        buffered.extend_from_slice(&chunk[..read]);
    }
    let body = &buffered[header_end..body_end];
    if buffered.len() > body_end {
        return Err(invalid_response("unexpected bytes after response body"));
    }
    let response = if status == 202 {
        if expected_id.is_some() {
            return Err(invalid_response(
                "202 response cannot answer a request with an id",
            ));
        }
        if content_length != 0 {
            return Err(invalid_response("202 response must have an empty body"));
        }
        if let Some(content_type) = headers.get("content-type") {
            if !is_json_content_type(content_type) {
                return Err(invalid_response("202 response Content-Type is invalid"));
            }
        }
        String::new()
    } else {
        let expected_id = expected_id.ok_or_else(|| {
            invalid_response("notification must receive a 202 response, not JSON")
        })?;
        if content_length == 0 {
            return Err(invalid_response("200 response body must not be empty"));
        }
        let content_type = headers
            .get("content-type")
            .ok_or_else(|| invalid_response("response application/json is required"))?;
        if !is_json_content_type(content_type) {
            return Err(invalid_response("response application/json is required"));
        }
        let body = std::str::from_utf8(body)
            .map_err(|_| invalid_response("response body is not valid UTF-8"))?;
        validate_json_rpc_response(body, expected_id)?;
        body.to_string()
    };
    buffered.drain(..body_end);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{McpServer, ToolCallHandler};
    use serde_json::Value;
    use std::net::TcpListener;
    use std::thread;

    struct Fake;
    impl ToolCallHandler for Fake {
        fn call(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
            Ok(serde_json::json!({"tool": name, "args": arguments}))
        }
    }

    /// Accept exactly `connections` connections, serving each until the peer
    /// closes it, then exit — so tests can `join` without an eternal loop.
    fn spawn_server(
        bearer: Option<String>,
        connections: usize,
    ) -> (SocketAddr, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let token = bearer.unwrap_or_else(|| "fixture".to_string());
        let handle = thread::spawn(move || {
            for _ in 0..connections {
                let Ok((mut stream, _)) = listener.accept() else {
                    continue;
                };
                let mut server = McpServer::new(Fake)
                    .with_bearer_token(token.clone())
                    .with_http_path("/mcp");
                let _ = server.serve_http_connection(&mut stream);
            }
        });
        (addr, handle)
    }

    fn list_request(id: u64) -> String {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {}})
            .to_string()
    }

    fn mutating_request() -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "browser.operate",
                "arguments": {},
                "idempotencyKey": "strict-response-test"
            }
        })
        .to_string()
    }

    fn raw_response(headers: &str, body: &[u8]) -> Vec<u8> {
        let mut response = format!("HTTP/1.1 200 OK\r\n{headers}\r\n").into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn spawn_raw_response(response: Vec<u8>) -> (SocketAddr, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&response);
            let _ = stream.flush();
        });
        (addr, handle)
    }

    #[test]
    fn strict_response_parser_rejects_invalid_framing_content_and_envelopes() {
        let valid_json = br#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        let valid_headers = |length: usize| {
            format!(
                "Content-Type: application/json\r\nMCP-Protocol-Version: 2026-07-28\r\nContent-Length: {length}\r\nConnection: close\r\n"
            )
        };
        let mismatched = br#"{"jsonrpc":"2.0","id":2,"result":{}}"#;
        let cases = vec![
            (
                "missing content length",
                raw_response(
                    "Content-Type: application/json\r\nMCP-Protocol-Version: 2026-07-28\r\nConnection: close\r\n",
                    valid_json,
                ),
            ),
            (
                "invalid content length",
                raw_response(
                    "Content-Type: application/json\r\nMCP-Protocol-Version: 2026-07-28\r\nContent-Length: nope\r\nConnection: close\r\n",
                    valid_json,
                ),
            ),
            (
                "wrong content type",
                raw_response(
                    &format!(
                        "Content-Type: text/html\r\nMCP-Protocol-Version: 2026-07-28\r\nContent-Length: {}\r\nConnection: close\r\n",
                        valid_json.len()
                    ),
                    b"<html>not json</html>",
                ),
            ),
            (
                "missing protocol metadata",
                raw_response(
                    &format!(
                        "Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                        valid_json.len()
                    ),
                    valid_json,
                ),
            ),
            (
                "unsupported protocol metadata",
                raw_response(
                    &format!(
                        "Content-Type: application/json\r\nMCP-Protocol-Version: 1999-01-01\r\nContent-Length: {}\r\nConnection: close\r\n",
                        valid_json.len()
                    ),
                    valid_json,
                ),
            ),
            ("invalid utf8", raw_response(&valid_headers(1), &[0xff])),
            (
                "truncated body",
                raw_response(&valid_headers(100), valid_json),
            ),
            ("empty body", raw_response(&valid_headers(0), b"")),
            (
                "invalid json-rpc envelope",
                raw_response(&valid_headers(5), b"nope!"),
            ),
            (
                "mismatched id",
                raw_response(&valid_headers(mismatched.len()), mismatched),
            ),
        ];
        for (name, response) in cases {
            let (addr, handle) = spawn_raw_response(response);
            let mut pool = LoopbackPool::connect(addr, Some("fixture".to_string()));
            let error = pool
                .request(&mutating_request())
                .expect_err("invalid response must be rejected");
            assert!(!error.to_string().is_empty(), "{name} was not rejected");
            assert_eq!(pool.stats.retried, 0, "mutations must not retry");
            pool.close();
            handle.join().unwrap();
        }
    }

    #[test]
    fn sequential_calls_reuse_one_pooled_connection() {
        let (addr, handle) = spawn_server(None, 1);
        let mut pool = LoopbackPool::connect(addr, Some("fixture".to_string()));
        for id in 1..=5u64 {
            let resp = pool.request(&list_request(id)).unwrap();
            assert!(resp.contains("\"id\":"));
            assert!(resp.contains("tools"));
        }
        // 5 calls, exactly 1 TCP handshake, 4 reuses, 0 retries.
        assert_eq!(pool.stats.opened, 1);
        assert_eq!(pool.stats.reused, 5);
        assert_eq!(pool.stats.retried, 0);
        pool.close();
        handle.join().unwrap();
    }

    #[test]
    fn keep_alive_serves_all_requests_on_one_connection() {
        // Server-side counter: one connection must serve all N requests.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut server = McpServer::new(Fake)
                .with_bearer_token("fixture")
                .with_http_path("/mcp");
            server.serve_http_connection(&mut stream).unwrap()
        });
        let mut pool = LoopbackPool::connect(addr, Some("fixture".to_string()));
        for id in 1..=4u64 {
            assert!(pool.request(&list_request(id)).unwrap().contains("tools"));
        }
        pool.close();
        let served = handle.join().unwrap();
        assert_eq!(served, 4);
    }

    #[test]
    fn bearer_token_still_enforced_on_pooled_connection() {
        // Good client: 1 connection (all requests reuse it).
        let (addr, _handle) = spawn_server(Some("secret".to_string()), 1);
        let mut pool = LoopbackPool::connect(addr, Some("secret".to_string()));
        assert!(pool.request(&list_request(1)).unwrap().contains("tools"));
        pool.close();

        // Bad client: initial attempt + one retry = 2 connections.
        let (addr, handle) = spawn_server(Some("secret".to_string()), 2);
        let mut bad = LoopbackPool::connect(addr, None);
        assert!(bad.request(&list_request(1)).is_err());
        bad.close();
        handle.join().unwrap();
    }

    #[test]
    fn mutations_are_not_replayed_after_transport_loss() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            // Simulate a response lost after the request reached the server.
        });
        let mut pool = LoopbackPool::connect(addr, None);
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "browser.operate",
                "arguments": {},
                "idempotencyKey": "lost-response-1"
            }
        })
        .to_string();
        assert!(pool.request(&body).is_err());
        assert_eq!(pool.stats.opened, 1);
        assert_eq!(pool.stats.retried, 0);
        pool.close();
        server.join().unwrap();
    }

    #[test]
    fn reconnect_after_server_restart() {
        // A pool whose connection dies must transparently reconnect (once).
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let first = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut server = McpServer::new(Fake)
                .with_bearer_token("fixture")
                .with_http_path("/mcp");
            let _ = server.serve_http_connection(&mut stream);
        });
        let mut pool = LoopbackPool::connect(addr, Some("fixture".to_string()));
        assert!(pool.request(&list_request(1)).unwrap().contains("tools"));
        first.join().unwrap(); // server gone → pooled socket is dead

        // Second server on the same addr.
        let listener2 = TcpListener::bind(addr).unwrap();
        let second = thread::spawn(move || {
            let (mut stream, _) = listener2.accept().unwrap();
            let mut server = McpServer::new(Fake)
                .with_bearer_token("fixture")
                .with_http_path("/mcp");
            let _ = server.serve_http_connection(&mut stream);
        });
        assert!(pool.request(&list_request(2)).unwrap().contains("tools"));
        assert_eq!(pool.stats.opened, 2); // one reconnect
        assert_eq!(pool.stats.retried, 1);
        pool.close();
        second.join().unwrap();
    }
}
