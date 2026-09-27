//! P9.5 (A8, v2.0 §P3) — Local **OpenAI-compatible** HTTP server.
//!
//! Exposes the AgentCowork engine on `127.0.0.1:<port>` as an OpenAI-compatible
//! API so VS Code / Cursor / Continue / any OpenAI-SDK client can point their
//! `baseURL` at us and use our router + BYOK keys + local models. This is the
//! *inbound* mirror of everything the product already does outbound.
//!
//! ## Endpoints (the exact OpenAI contract)
//!   - `GET  /v1/models`            → `{ object:"list", data:[{id,object:"model",created,owned_by}] }`
//!   - `POST /v1/chat/completions`  → non-stream `chat.completion`, or SSE when `stream:true`
//!   - `GET  /health`               → `{ ok:true }` (unauthenticated liveness)
//!
//! ## Security posture (mirrors `webmcp_http`)
//!   - **loopback only** by construction (binds `127.0.0.1`); the transport
//!     boundary is loopback, the capability boundary is a per-process **bearer
//!     token** every `/v1/*` call must present (any local process could
//!     connect otherwise);
//!   - request size + read-timeout caps (no memory exhaustion from a local
//!     client);
//!   - the server never sees provider secrets — it calls a
//!     [`CompletionBackend`] seam which, in live wiring, runs through the same
//!     Rust broker that resolves keys (keys stay in the vault, exactly as for
//!     the sidecar path).
//!
//! ## Testability
//! The request handlers are pure functions over a [`CompletionBackend`] +
//! [`ModelLister`]; the whole OpenAI surface is unit-tested with a fake
//! backend, and a threaded server wraps the handlers for real sockets.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Max request body (2 MiB) — a chat request with a large context still fits;
/// anything larger is refused, not buffered.
pub const MAX_BODY_BYTES: usize = 2 << 20;
const MAX_REQUEST_BYTES: usize = MAX_BODY_BYTES + 64 * 1024;

// ---------------------------------------------------------------------------
// OpenAI wire types (the subset we implement).
// ---------------------------------------------------------------------------

/// Treat an explicit JSON `null` as an absent string.
///
/// OpenAI sends `"content": null` on an assistant message that carries only
/// `tool_calls` (and on some `tool` messages). Without this an ordinary
/// tool-calling round trip would be rejected with a 400 before it reached the
/// engine.
fn de_nullable_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// One chat message in the request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default, deserialize_with = "de_nullable_string")]
    pub content: String,
    /// The `tool_calls` id this message answers (role `tool`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Native tool calls on an assistant message. Opaque here — the shape is
    /// the provider's, and it is forwarded upstream verbatim so a client can
    /// replay its own tool-calling history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
}

/// `POST /v1/chat/completions` request body (the fields we honor).
#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// The client's tool declarations — forwarded to the provider unchanged.
    /// Dropping these silently breaks every tool-calling client (Continue /
    /// Cursor / the OpenAI SDK), which is why A8 was previously declared
    /// unusable for an autonomous agent.
    #[serde(default)]
    pub tools: Option<Value>,
    /// `none` | `auto` | `required` | `{type:"function",function:{name}}` —
    /// forwarded verbatim alongside `tools`.
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
}

/// A model row for `GET /v1/models`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModelRow {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub owned_by: String,
}

impl ModelRow {
    pub fn new(id: impl Into<String>, owned_by: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            object: "model",
            created: 0,
            owned_by: owned_by.into(),
        }
    }
}

/// One native function call in a response (OpenAI `message.tool_calls`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolCallOut {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

/// The `function` object of a [`ToolCallOut`]. `arguments` stays a **string**
/// because that is the OpenAI wire form (the client parses it).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

/// The completion result the backend returns (non-stream) — content + token
/// accounting (may be zero if the backend cannot measure it honestly).
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionResult {
    pub content: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// The model id actually used (after alias/router resolution).
    pub model: String,
    /// Native tool calls. Empty for a plain content answer; a result that
    /// carries any reports `finish_reason: "tool_calls"`.
    pub tool_calls: Vec<ToolCallOut>,
}

/// The OpenAI `finish_reason` a result maps to.
pub fn finish_reason_of(r: &CompletionResult) -> &'static str {
    if r.tool_calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    }
}

/// One piece of a streamed completion.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamPiece {
    /// A text delta.
    Content(String),
    /// A native tool-call fragment. Follows OpenAI `delta.tool_calls`
    /// semantics: `index` groups fragments, `id`/`name` arrive on the first
    /// fragment of a call, and `arguments` accumulates across fragments.
    ToolCall {
        index: i64,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
}

/// The engine seam. Live wiring bridges this to the coordinator/broker; tests
/// inject a deterministic fake. `complete` is the non-stream path; `stream`
/// yields content deltas via a callback (returns the final token accounting).
pub trait CompletionBackend: Send + Sync {
    /// Non-streaming completion.
    fn complete(&self, req: &ChatCompletionRequest) -> Result<CompletionResult, String>;

    /// Streaming completion. The default impl runs `complete` and emits the
    /// whole answer in a single piece — a correct (if non-incremental) SSE
    /// stream — so a backend that cannot stream still speaks the protocol. The
    /// live vault-broker backend overrides this with real per-chunk delivery.
    fn stream(
        &self,
        req: &ChatCompletionRequest,
        on_piece: &mut dyn FnMut(StreamPiece),
    ) -> Result<CompletionResult, String> {
        let result = self.complete(req)?;
        if !result.content.is_empty() {
            on_piece(StreamPiece::Content(result.content.clone()));
        }
        for (i, tc) in result.tool_calls.iter().enumerate() {
            on_piece(StreamPiece::ToolCall {
                index: i as i64,
                id: Some(tc.id.clone()),
                name: Some(tc.function.name.clone()),
                arguments: Some(tc.function.arguments.clone()),
            });
        }
        Ok(result)
    }
}

/// Lists the models advertised on `GET /v1/models`.
pub trait ModelLister: Send + Sync {
    fn models(&self) -> Vec<ModelRow>;
}

// ---------------------------------------------------------------------------
// Response shaping (pure).
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A short unique-ish id for a completion (`chatcmpl-<hex>`), matching the
/// OpenAI id shape closely enough for clients that echo it.
fn completion_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(now_secs());
    h.write_u32(std::process::id());
    format!("chatcmpl-{:016x}", h.finish())
}

/// Build the non-streaming `chat.completion` JSON body.
pub fn non_stream_body(id: &str, created: u64, r: &CompletionResult) -> Value {
    let mut message = json!({ "role": "assistant", "content": r.content });
    // `tool_calls` is present only when the answer actually calls a tool — an
    // empty array would make a client think a tool-calling turn happened.
    if !r.tool_calls.is_empty() {
        message["tool_calls"] = serde_json::to_value(&r.tool_calls).unwrap_or(Value::Null);
    }
    json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": r.model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason_of(r),
        }],
        "usage": {
            "prompt_tokens": r.prompt_tokens,
            "completion_tokens": r.completion_tokens,
            "total_tokens": r.prompt_tokens + r.completion_tokens,
        },
    })
}

/// One SSE `chat.completion.chunk` frame carrying a content delta.
pub fn stream_chunk(id: &str, created: u64, model: &str, delta: &str) -> String {
    let obj = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{ "index": 0, "delta": { "content": delta }, "finish_reason": Value::Null }],
    });
    format!("data: {}\n\n", obj)
}

/// One SSE `chat.completion.chunk` frame carrying a tool-call fragment.
///
/// `type:"function"` rides with the fragment that names the function, exactly
/// as OpenAI emits it, so a strict client that keys off `type` still works.
pub fn stream_tool_call_chunk(
    id: &str,
    created: u64,
    model: &str,
    index: i64,
    call_id: Option<&str>,
    name: Option<&str>,
    arguments: Option<&str>,
) -> String {
    let mut call = json!({ "index": index });
    if let Some(cid) = call_id {
        call["id"] = json!(cid);
    }
    let mut function = json!({});
    if let Some(n) = name {
        function["name"] = json!(n);
        call["type"] = json!("function");
    }
    if let Some(a) = arguments {
        function["arguments"] = json!(a);
    }
    call["function"] = function;
    let obj = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{ "index": 0, "delta": { "tool_calls": [call] }, "finish_reason": Value::Null }],
    });
    format!("data: {}\n\n", obj)
}

/// The final SSE frame: an empty delta with the real `finish_reason`
/// (`stop`, `tool_calls`, …), then the `[DONE]` sentinel every OpenAI SSE
/// client waits for.
pub fn stream_final(id: &str, created: u64, model: &str, finish_reason: &str) -> String {
    let stop = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }],
    });
    format!("data: {}\n\ndata: [DONE]\n\n", stop)
}

/// The `GET /v1/models` list body.
pub fn models_body(rows: &[ModelRow]) -> Value {
    json!({ "object": "list", "data": rows })
}

fn openai_error(status_code: u16, message: &str, err_type: &str) -> String {
    let body = json!({
        "error": { "message": message, "type": err_type, "code": Value::Null }
    })
    .to_string();
    let status = match status_code {
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        413 => "413 Payload Too Large",
        500 => "500 Internal Server Error",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    };
    http_json(status, &body)
}

fn http_json(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn http_sse_headers() -> &'static str {
    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n"
}

// ---------------------------------------------------------------------------
// HTTP parsing (mirrors webmcp_http, kept local so core has no browser dep).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpParseError {
    BodyTooLarge,
}

/// Parse into (method, path, body). `Ok(None)` = partial (read more).
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
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let content_length: usize = lines
        .find_map(|l| {
            l.strip_prefix("Content-Length:")
                .or_else(|| l.strip_prefix("content-length:"))
                .map(|v| v.trim().parse::<usize>().ok())
        })
        .flatten()
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        return Err(HttpParseError::BodyTooLarge);
    }
    let body_start = header_end + 4;
    let Some(body) = raw.get(body_start..) else {
        return Ok(None);
    };
    if body.len() < content_length {
        return Ok(None);
    }
    Ok(Some((method, path, body[..content_length].to_string())))
}

/// Extract `Authorization: Bearer <token>` (OpenAI SDKs send `Bearer <apiKey>`).
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

/// The path classification (so auth + dispatch are explicit).
#[derive(Debug, PartialEq, Eq)]
enum Route {
    Health,
    Models,
    ChatCompletions,
    NotFound,
}

fn route(path: &str) -> Route {
    // Tolerate a query string and a trailing slash.
    let p = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    match p {
        "/health" => Route::Health,
        "/v1/models" => Route::Models,
        "/v1/chat/completions" => Route::ChatCompletions,
        _ => Route::NotFound,
    }
}

/// Handle one parsed request (non-streaming responses only; the streaming path
/// is handled at the socket layer in `handle_stream`). `authed` is whether the
/// caller presented the correct bearer token. Returns the full HTTP response.
pub fn handle_request(
    method: &str,
    path: &str,
    body: &str,
    authed: bool,
    backend: &dyn CompletionBackend,
    lister: &dyn ModelLister,
) -> String {
    match route(path) {
        // Liveness is unauthenticated so a client can probe before configuring.
        Route::Health => {
            if method == "GET" {
                http_json("200 OK", &json!({ "ok": true }).to_string())
            } else {
                openai_error(405, "method not allowed", "invalid_request_error")
            }
        }
        Route::NotFound => openai_error(404, "unknown route", "invalid_request_error"),
        _ if !authed => openai_error(
            401,
            "missing or invalid bearer token",
            "invalid_request_error",
        ),
        Route::Models => {
            if method != "GET" {
                return openai_error(405, "method not allowed", "invalid_request_error");
            }
            http_json("200 OK", &models_body(&lister.models()).to_string())
        }
        Route::ChatCompletions => {
            if method != "POST" {
                return openai_error(405, "method not allowed", "invalid_request_error");
            }
            let req: ChatCompletionRequest = match serde_json::from_str(body) {
                Ok(r) => r,
                Err(e) => {
                    return openai_error(
                        400,
                        &format!("invalid request body: {e}"),
                        "invalid_request_error",
                    );
                }
            };
            if req.messages.is_empty() {
                return openai_error(400, "messages must not be empty", "invalid_request_error");
            }
            // Note: streaming is dispatched at the socket layer; this pure
            // handler always returns the non-stream body (used directly for
            // stream:false, and by tests).
            match backend.complete(&req) {
                Ok(r) => {
                    let id = completion_id();
                    http_json("200 OK", &non_stream_body(&id, now_secs(), &r).to_string())
                }
                Err(e) => openai_error(503, &format!("engine error: {e}"), "engine_error"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Threaded server.
// ---------------------------------------------------------------------------

/// A running OpenAI-compatible server on `127.0.0.1:<port>`.
pub struct OpenAiServer {
    listener: TcpListener,
    backend: Arc<dyn CompletionBackend>,
    lister: Arc<dyn ModelLister>,
    token: String,
}

impl OpenAiServer {
    /// Bind a loopback listener and generate a fresh per-process bearer token.
    pub fn serve(
        addr: &str,
        backend: Arc<dyn CompletionBackend>,
        lister: Arc<dyn ModelLister>,
    ) -> std::io::Result<Self> {
        // Enforce loopback regardless of the addr passed.
        let listener = TcpListener::bind(addr)?;
        let server = Self {
            listener,
            backend,
            lister,
            token: fresh_token(),
        };
        server.spawn_accept_loop();
        Ok(server)
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.listener.local_addr()
    }

    /// The `baseURL` a client points at (OpenAI SDKs expect the `/v1` base).
    pub fn base_url(&self) -> String {
        self.local_addr()
            .map(|a| format!("http://{a}/v1"))
            .unwrap_or_default()
    }

    fn spawn_accept_loop(&self) {
        let listener = self.listener.try_clone().expect("clone listener");
        let backend = Arc::clone(&self.backend);
        let lister = Arc::clone(&self.lister);
        let token = self.token.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let backend = Arc::clone(&backend);
                let lister = Arc::clone(&lister);
                let token = token.clone();
                std::thread::spawn(move || {
                    let _ = handle_stream(&mut stream, backend.as_ref(), lister.as_ref(), &token);
                });
            }
        });
    }
}

fn handle_stream(
    stream: &mut TcpStream,
    backend: &dyn CompletionBackend,
    lister: &dyn ModelLister,
    token: &str,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if buf.len() > MAX_REQUEST_BYTES {
            let _ = stream.write_all(
                openai_error(413, "request too large", "invalid_request_error").as_bytes(),
            );
            return Ok(());
        }
        let n = match stream.read(&mut chunk) {
            Ok(n) => n,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Ok(());
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
                let authed = bearer_token(&raw).as_deref() == Some(token);
                // Streaming chat completions get a dedicated SSE path.
                if authed && method == "POST" && route(&path) == Route::ChatCompletions {
                    if let Ok(req) = serde_json::from_str::<ChatCompletionRequest>(&body) {
                        if req.stream && !req.messages.is_empty() {
                            return stream_completion(stream, backend, &req);
                        }
                    }
                }
                let resp = handle_request(&method, &path, &body, authed, backend, lister);
                stream.write_all(resp.as_bytes())?;
                stream.flush()?;
                return Ok(());
            }
            Ok(None) => { /* partial — keep reading */ }
            Err(HttpParseError::BodyTooLarge) => {
                let _ = stream.write_all(
                    openai_error(413, "body exceeds limit", "invalid_request_error").as_bytes(),
                );
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Stream a chat completion as SSE: headers, then a `chat.completion.chunk`
/// per content delta, then the stop frame + `[DONE]`.
fn stream_completion(
    stream: &mut TcpStream,
    backend: &dyn CompletionBackend,
    req: &ChatCompletionRequest,
) -> std::io::Result<()> {
    let id = completion_id();
    let created = now_secs();
    let model = req.model.clone();
    stream.write_all(http_sse_headers().as_bytes())?;
    stream.flush()?;

    // Collect any write error from within the piece callback.
    let mut write_err: Option<std::io::Error> = None;
    let result = {
        let stream_ref = &mut *stream;
        let id_ref = &id;
        let model_ref = &model;
        let write_err_ref = &mut write_err;
        backend.stream(req, &mut |piece: StreamPiece| {
            if write_err_ref.is_some() {
                return;
            }
            let frame = match piece {
                StreamPiece::Content(delta) => stream_chunk(id_ref, created, model_ref, &delta),
                StreamPiece::ToolCall {
                    index,
                    id,
                    name,
                    arguments,
                } => stream_tool_call_chunk(
                    id_ref,
                    created,
                    model_ref,
                    index,
                    id.as_deref(),
                    name.as_deref(),
                    arguments.as_deref(),
                ),
            };
            if let Err(e) = stream_ref
                .write_all(frame.as_bytes())
                .and_then(|_| stream_ref.flush())
            {
                *write_err_ref = Some(e);
            }
        })
    };
    if let Some(e) = write_err {
        return Err(e); // client disconnected mid-stream
    }
    match result {
        Ok(r) => {
            let _ = stream
                .write_all(stream_final(&id, created, &r.model, finish_reason_of(&r)).as_bytes());
        }
        Err(e) => {
            // Surface the engine error inside the SSE stream, then close.
            let err_frame = format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({ "error": { "message": e, "type": "engine_error" } })
            );
            let _ = stream.write_all(err_frame.as_bytes());
        }
    }
    let _ = stream.flush();
    Ok(())
}

/// A fresh per-process bearer token (same construction as webmcp_http).
pub fn fresh_token() -> String {
    use std::hash::{BuildHasher, Hash, Hasher};
    let s = std::collections::hash_map::RandomState::new();
    let mut h = s.build_hasher();
    std::process::id().hash(&mut h);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut h);
    let mut h2 = s.build_hasher();
    b"agentcowork-openai".hash(&mut h2);
    (std::process::id() as u64)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15u64)
        .hash(&mut h2);
    format!("{:016x}{:016x}", h.finish(), h2.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoBackend;
    impl CompletionBackend for EchoBackend {
        fn complete(&self, req: &ChatCompletionRequest) -> Result<CompletionResult, String> {
            let last = req
                .messages
                .last()
                .map(|m| m.content.clone())
                .unwrap_or_default();
            Ok(CompletionResult {
                content: format!("echo: {last}"),
                prompt_tokens: last.split_whitespace().count() as u64,
                completion_tokens: 2,
                model: req.model.clone(),
                tool_calls: Vec::new(),
            })
        }
    }

    /// A backend that answers with a native tool call and no content — the
    /// shape that previously had nowhere to go in this server.
    struct ToolCallingBackend;
    impl CompletionBackend for ToolCallingBackend {
        fn complete(&self, req: &ChatCompletionRequest) -> Result<CompletionResult, String> {
            Ok(CompletionResult {
                content: String::new(),
                prompt_tokens: 11,
                completion_tokens: 7,
                model: req.model.clone(),
                tool_calls: vec![ToolCallOut {
                    id: "call_abc".into(),
                    kind: "function".into(),
                    function: ToolCallFunction {
                        name: "read_file".into(),
                        arguments: r#"{"path":"a.rs"}"#.into(),
                    },
                }],
            })
        }
    }

    struct FailBackend;
    impl CompletionBackend for FailBackend {
        fn complete(&self, _req: &ChatCompletionRequest) -> Result<CompletionResult, String> {
            Err("no key configured".into())
        }
    }

    struct StaticModels;
    impl ModelLister for StaticModels {
        fn models(&self) -> Vec<ModelRow> {
            vec![
                // P71.2a — `owned_by` names the provider a request routes to;
                // never the host's own name (it is transport, not an owner).
                ModelRow::new("agentcowork-auto", "openai"),
                ModelRow::new("ollama/llama3", "local"),
            ]
        }
    }

    fn body(resp: &str) -> Value {
        serde_json::from_str(resp.split("\r\n\r\n").nth(1).unwrap()).unwrap()
    }

    #[test]
    fn health_is_unauthenticated() {
        let r = handle_request("GET", "/health", "", false, &EchoBackend, &StaticModels);
        assert!(r.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(body(&r)["ok"], true);
    }

    #[test]
    fn models_list_matches_openai_shape() {
        let r = handle_request("GET", "/v1/models", "", true, &EchoBackend, &StaticModels);
        let v = body(&r);
        assert_eq!(v["object"], "list");
        assert_eq!(v["data"][0]["id"], "agentcowork-auto");
        assert_eq!(v["data"][0]["object"], "model");
        assert_eq!(v["data"][1]["owned_by"], "local");
    }

    #[test]
    fn models_requires_auth() {
        let r = handle_request("GET", "/v1/models", "", false, &EchoBackend, &StaticModels);
        assert!(r.starts_with("HTTP/1.1 401"));
        assert_eq!(body(&r)["error"]["type"], "invalid_request_error");
    }

    #[test]
    fn chat_completion_non_stream_shape() {
        let req =
            r#"{"model":"agentcowork-auto","messages":[{"role":"user","content":"hi there"}]}"#;
        let r = handle_request(
            "POST",
            "/v1/chat/completions",
            req,
            true,
            &EchoBackend,
            &StaticModels,
        );
        let v = body(&r);
        assert_eq!(v["object"], "chat.completion");
        assert_eq!(v["choices"][0]["message"]["role"], "assistant");
        assert_eq!(v["choices"][0]["message"]["content"], "echo: hi there");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["model"], "agentcowork-auto");
        // usage totals add up.
        let p = v["usage"]["prompt_tokens"].as_u64().unwrap();
        let c = v["usage"]["completion_tokens"].as_u64().unwrap();
        assert_eq!(v["usage"]["total_tokens"].as_u64().unwrap(), p + c);
    }

    #[test]
    fn chat_completion_rejects_empty_messages() {
        let req = r#"{"model":"m","messages":[]}"#;
        let r = handle_request(
            "POST",
            "/v1/chat/completions",
            req,
            true,
            &EchoBackend,
            &StaticModels,
        );
        assert!(r.starts_with("HTTP/1.1 400"));
    }

    #[test]
    fn chat_completion_rejects_bad_json() {
        let r = handle_request(
            "POST",
            "/v1/chat/completions",
            "{not json",
            true,
            &EchoBackend,
            &StaticModels,
        );
        assert!(r.starts_with("HTTP/1.1 400"));
    }

    #[test]
    fn engine_error_becomes_503() {
        let req = r#"{"model":"m","messages":[{"role":"user","content":"x"}]}"#;
        let r = handle_request(
            "POST",
            "/v1/chat/completions",
            req,
            true,
            &FailBackend,
            &StaticModels,
        );
        assert!(r.starts_with("HTTP/1.1 503"));
        assert_eq!(body(&r)["error"]["type"], "engine_error");
    }

    #[test]
    fn wrong_method_is_405() {
        let r = handle_request(
            "DELETE",
            "/v1/models",
            "",
            true,
            &EchoBackend,
            &StaticModels,
        );
        assert!(r.starts_with("HTTP/1.1 405"));
        let r2 = handle_request(
            "GET",
            "/v1/chat/completions",
            "",
            true,
            &EchoBackend,
            &StaticModels,
        );
        assert!(r2.starts_with("HTTP/1.1 405"));
    }

    #[test]
    fn unknown_route_is_404() {
        let r = handle_request(
            "GET",
            "/v1/embeddings",
            "",
            true,
            &EchoBackend,
            &StaticModels,
        );
        assert!(r.starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn route_tolerates_trailing_slash_and_query() {
        assert_eq!(route("/v1/models/"), Route::Models);
        assert_eq!(
            route("/v1/chat/completions?foo=bar"),
            Route::ChatCompletions
        );
        assert_eq!(route("/health"), Route::Health);
    }

    #[test]
    fn sse_chunk_and_final_frames_are_wellformed() {
        let chunk = stream_chunk("id1", 100, "m", "hello");
        assert!(chunk.starts_with("data: "));
        assert!(chunk.ends_with("\n\n"));
        let v: Value = serde_json::from_str(chunk.trim_start_matches("data: ").trim()).unwrap();
        assert_eq!(v["object"], "chat.completion.chunk");
        assert_eq!(v["choices"][0]["delta"]["content"], "hello");

        let fin = stream_final("id1", 100, "m", "stop");
        assert!(fin.contains("\"finish_reason\":\"stop\""));
        assert!(fin.trim_end().ends_with("data: [DONE]"));
    }

    #[test]
    fn tool_call_chunk_carries_type_with_the_name_fragment() {
        let first = stream_tool_call_chunk(
            "id1",
            100,
            "m",
            0,
            Some("call_abc"),
            Some("read_file"),
            Some(r#"{"path":"#),
        );
        let v: Value = serde_json::from_str(first.trim_start_matches("data: ").trim()).unwrap();
        let call = &v["choices"][0]["delta"]["tool_calls"][0];
        assert_eq!(call["index"], 0);
        assert_eq!(call["id"], "call_abc");
        assert_eq!(call["type"], "function");
        assert_eq!(call["function"]["name"], "read_file");
        assert_eq!(call["function"]["arguments"], r#"{"path":"#);

        // An arguments-only continuation omits id/name/type (OpenAI shape).
        let cont = stream_tool_call_chunk("id1", 100, "m", 0, None, None, Some("a.rs\"}"));
        let cv: Value = serde_json::from_str(cont.trim_start_matches("data: ").trim()).unwrap();
        let c2 = &cv["choices"][0]["delta"]["tool_calls"][0];
        assert_eq!(c2["index"], 0);
        assert!(c2.get("id").is_none());
        assert!(c2.get("type").is_none());
        assert!(c2["function"].get("name").is_none());
        assert_eq!(c2["function"]["arguments"], "a.rs\"}");
    }

    #[test]
    fn null_tool_call_history_parses_instead_of_400ing() {
        // The exact body a client replays after a tool call: the assistant
        // message carries `content: null` + `tool_calls`, and the tool result
        // carries `tool_call_id` with no content field at all.
        let body = r#"{"model":"m","messages":[
            {"role":"user","content":"read a.rs"},
            {"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"read_file","arguments":"{}"}}]},
            {"role":"tool","tool_call_id":"c1","content":"fn main() {}"}
        ]}"#;
        let req: ChatCompletionRequest = serde_json::from_str(body).unwrap();
        assert_eq!(req.messages.len(), 3);
        assert_eq!(req.messages[1].content, "");
        assert!(req.messages[1].tool_calls.is_some());
        assert_eq!(req.messages[2].tool_call_id.as_deref(), Some("c1"));
        // And the handler accepts the same body (no 400).
        let r = handle_request(
            "POST",
            "/v1/chat/completions",
            body,
            true,
            &EchoBackend,
            &StaticModels,
        );
        assert!(r.starts_with("HTTP/1.1 200 OK"), "got: {r}");
    }

    #[test]
    fn tools_and_tool_choice_are_parsed_for_forwarding() {
        let body = r#"{"model":"m","messages":[{"role":"user","content":"x"}],
            "tools":[{"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}}],
            "tool_choice":"auto","parallel_tool_calls":false}"#;
        let req: ChatCompletionRequest = serde_json::from_str(body).unwrap();
        assert_eq!(
            req.tools.as_ref().unwrap()[0]["function"]["name"],
            "read_file"
        );
        assert_eq!(req.tool_choice.as_ref().unwrap(), "auto");
        assert_eq!(req.parallel_tool_calls, Some(false));
    }

    #[test]
    fn non_stream_body_reports_tool_calls_and_finish_reason() {
        let req: ChatCompletionRequest = serde_json::from_str(
            r#"{"model":"m","messages":[{"role":"user","content":"read a.rs"}]}"#,
        )
        .unwrap();
        let r = ToolCallingBackend.complete(&req).unwrap();
        let v = non_stream_body("id1", 100, &r);
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["id"],
            "call_abc"
        );
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "read_file"
        );
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"a.rs"}"#
        );
        // A content-only answer must NOT carry a tool_calls key.
        let plain = EchoBackend.complete(&req).unwrap();
        let pv = non_stream_body("id2", 100, &plain);
        assert_eq!(pv["choices"][0]["finish_reason"], "stop");
        assert!(pv["choices"][0]["message"].get("tool_calls").is_none());
    }

    #[test]
    fn default_stream_impl_emits_whole_content() {
        let mut deltas = Vec::new();
        let req = ChatCompletionRequest {
            model: "m".into(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "hi".into(),
                ..Default::default()
            }],
            stream: true,
            temperature: None,
            max_tokens: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
        };
        let r = EchoBackend
            .stream(&req, &mut |p| {
                if let StreamPiece::Content(d) = p {
                    deltas.push(d);
                }
            })
            .unwrap();
        assert_eq!(deltas, vec!["echo: hi".to_string()]);
        assert_eq!(r.content, "echo: hi");
    }

    #[test]
    fn default_stream_impl_emits_tool_calls_for_a_non_streaming_backend() {
        let req: ChatCompletionRequest = serde_json::from_str(
            r#"{"model":"m","messages":[{"role":"user","content":"go"}],"stream":true}"#,
        )
        .unwrap();
        let mut pieces = Vec::new();
        let r = ToolCallingBackend
            .stream(&req, &mut |p| pieces.push(p))
            .unwrap();
        assert_eq!(pieces.len(), 1);
        match &pieces[0] {
            StreamPiece::ToolCall {
                index,
                id,
                name,
                arguments,
            } => {
                assert_eq!(*index, 0);
                assert_eq!(id.as_deref(), Some("call_abc"));
                assert_eq!(name.as_deref(), Some("read_file"));
                assert_eq!(arguments.as_deref(), Some(r#"{"path":"a.rs"}"#));
            }
            other => panic!("expected a tool-call piece, got {other:?}"),
        }
        assert_eq!(finish_reason_of(&r), "tool_calls");
    }

    #[test]
    fn parse_and_bearer_helpers() {
        let raw = "POST /v1/chat/completions HTTP/1.1\r\nAuthorization: Bearer sk-local\r\nContent-Length: 2\r\n\r\n{}";
        let (m, p, b) = parse_http_request(raw).unwrap().unwrap();
        assert_eq!(
            (m.as_str(), p.as_str(), b.as_str()),
            ("POST", "/v1/chat/completions", "{}")
        );
        assert_eq!(bearer_token(raw).as_deref(), Some("sk-local"));
        // oversized declared body → Err.
        let big = format!(
            "POST /x HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        assert!(parse_http_request(&big).is_err());
    }

    #[test]
    fn end_to_end_over_a_real_socket() {
        let server =
            OpenAiServer::serve("127.0.0.1:0", Arc::new(EchoBackend), Arc::new(StaticModels))
                .unwrap();
        let port = server.local_addr().unwrap().port();
        let token = server.token().to_string();
        assert!(server.base_url().ends_with("/v1"));

        // Non-stream chat completion over a real connection, with the token.
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let reqbody =
            r#"{"model":"agentcowork-auto","messages":[{"role":"user","content":"ping"}]}"#;
        let request = format!(
            "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{}",
            token,
            reqbody.len(),
            reqbody
        );
        s.write_all(request.as_bytes()).unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"), "got: {resp}");
        let v: Value = serde_json::from_str(resp.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "echo: ping");
    }

    #[test]
    fn real_socket_streaming_yields_sse_and_done() {
        let server =
            OpenAiServer::serve("127.0.0.1:0", Arc::new(EchoBackend), Arc::new(StaticModels))
                .unwrap();
        let port = server.local_addr().unwrap().port();
        let token = server.token().to_string();

        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let reqbody = r#"{"model":"m","messages":[{"role":"user","content":"go"}],"stream":true}"#;
        let request = format!(
            "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{}",
            token,
            reqbody.len(),
            reqbody
        );
        s.write_all(request.as_bytes()).unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("text/event-stream"));
        assert!(resp.contains("chat.completion.chunk"));
        assert!(resp.contains("echo: go"));
        assert!(resp.trim_end().ends_with("data: [DONE]"));
    }

    #[test]
    fn real_socket_streams_tool_call_deltas_and_reports_finish() {
        let server = OpenAiServer::serve(
            "127.0.0.1:0",
            Arc::new(ToolCallingBackend),
            Arc::new(StaticModels),
        )
        .unwrap();
        let port = server.local_addr().unwrap().port();
        let token = server.token().to_string();
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let reqbody = r#"{"model":"m","messages":[{"role":"user","content":"read a.rs"}],"stream":true,"tools":[{"type":"function","function":{"name":"read_file"}}]}"#;
        let request = format!(
            "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{}",
            token,
            reqbody.len(),
            reqbody
        );
        s.write_all(request.as_bytes()).unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("text/event-stream"), "got: {resp}");
        // The tool call rides as a delta, not as content.
        assert!(resp.contains("tool_calls"), "got: {resp}");
        assert!(resp.contains("read_file"), "got: {resp}");
        // …and the terminating frame carries the tool-calling finish reason.
        assert!(
            resp.contains("\"finish_reason\":\"tool_calls\""),
            "got: {resp}"
        );
        assert!(resp.trim_end().ends_with("data: [DONE]"));
    }

    #[test]
    fn real_socket_rejects_missing_token() {
        let server =
            OpenAiServer::serve("127.0.0.1:0", Arc::new(EchoBackend), Arc::new(StaticModels))
                .unwrap();
        let port = server.local_addr().unwrap().port();
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let request = "GET /v1/models HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        s.write_all(request.as_bytes()).unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(resp.starts_with("HTTP/1.1 401"));
        // But /health is reachable without a token.
        let mut s2 = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s2.write_all("GET /health HTTP/1.1\r\nHost: x\r\n\r\n".as_bytes())
            .unwrap();
        let mut r2 = String::new();
        s2.read_to_string(&mut r2).unwrap();
        assert!(r2.starts_with("HTTP/1.1 200 OK"));
    }
}
