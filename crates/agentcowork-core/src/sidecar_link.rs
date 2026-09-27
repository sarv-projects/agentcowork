//! SidecarLink — framed bidirectional link to the TS coordinator sidecar (P1.4).
//!
//! Both directions share one stdio pair (JSON-RPC 2.0 + `[u32 LE len][payload]`
//! framing, `agentcowork-ipc`):
//!
//! ```text
//! Rust ──writes──▶ coordinator stdin   (requests + provider_chunk notifications)
//! Rust ◀─reads──── coordinator stdout  (responses + provider/stream requests
//!                                       + chat/* notifications)
//! ```
//!
//! The reader thread classifies every inbound frame:
//! - **response** to one of our requests (has `id` + `result`/`error`, no
//!   `method`) → completes the matching pending [`SidecarLink::request`];
//! - **coordinator→us request** (has `method` + `id`) or **notification**
//!   (has `method`, no `id`) → pushed to the inbound channel for the relay
//!   loop ([`Inbound`]).
//!
//! Fail-closed: when the sidecar's stdout hits EOF (process died), every
//! pending request errors instead of hanging.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvError, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentcowork_ipc::frame;
use agentcowork_ipc::message::Request;

/// Timeout for a [`SidecarLink::request`] awaiting its response.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// In-flight request completions: request id → channel to the awaiting caller.
type PendingRequests = HashMap<String, Sender<Result<serde_json::Value, String>>>;

/// An inbound frame from the coordinator that is NOT a response to our
/// requests: either a coordinator→us request awaiting [`SidecarLink::reply`],
/// or a fire-and-forget notification.
#[derive(Debug, Clone)]
pub enum Inbound {
    Request {
        id: serde_json::Value,
        method: String,
        params: serde_json::Value,
    },
    Notification {
        method: String,
        params: serde_json::Value,
    },
}

/// Cloneable write half — the broker thread pushes `chat/provider_chunk`
/// notifications through this while the relay loop owns the link.
/// `Arc<Mutex<W>>` is `Clone` for any `W`, so the manual impl avoids the
/// spurious `W: Clone` bound a derive would add.
pub struct WriterHandle<W> {
    writer: Arc<Mutex<W>>,
}

impl<W> Clone for WriterHandle<W> {
    fn clone(&self) -> Self {
        Self {
            writer: Arc::clone(&self.writer),
        }
    }
}

impl<W: Write> WriterHandle<W> {
    /// Send a fire-and-forget JSON-RPC notification (no `id`).
    pub fn notify(&self, method: &str, params: serde_json::Value) -> Result<(), LinkError> {
        let req = Request {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params: Some(params),
            id: None,
        };
        let bytes = serde_json::to_vec(&req)?;
        let mut w = self
            .writer
            .lock()
            .map_err(|_| LinkError::Poisoned("writer".into()))?;
        frame::write_frame(&mut *w, &bytes).map_err(|e| LinkError::Io(e.to_string()))
    }

    /// Reply to a coordinator→us request.
    pub fn reply(&self, id: serde_json::Value, result: serde_json::Value) -> Result<(), LinkError> {
        let resp = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
        self.write_frame(&serde_json::to_vec(&resp)?)
    }

    /// Reply to a coordinator→us request with an error.
    pub fn reply_error(&self, id: serde_json::Value, message: &str) -> Result<(), LinkError> {
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32603, "message": message },
        });
        self.write_frame(&serde_json::to_vec(&resp)?)
    }

    fn write_frame(&self, payload: &[u8]) -> Result<(), LinkError> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| LinkError::Poisoned("writer".into()))?;
        frame::write_frame(&mut *w, payload).map_err(|e| LinkError::Io(e.to_string()))
    }
}

/// The framed link. Generic over the pipe ends so tests can run it over a
/// `UnixStream::pair()` with a scripted fake sidecar.
///
/// The inbound receiver is shared (the chat relay's consumer loop owns one
/// clone for its lifetime while the relay keeps using `request`/`reply`).
/// `R` is consumed by the reader thread inside [`SidecarLink::new`], so it is
/// not a field — the marker keeps the type parameter meaningful.
pub struct SidecarLink<W, R> {
    writer: Arc<Mutex<W>>,
    pending: Arc<Mutex<PendingRequests>>,
    inbound: Arc<Mutex<Receiver<Inbound>>>,
    _reader: std::marker::PhantomData<R>,
}

impl<W: Write + Send + 'static, R: Read + Send + 'static> SidecarLink<W, R> {
    /// Own the coordinator's stdin (write) and stdout (read) ends and start
    /// the reader thread.
    pub fn new(stdin: W, stdout: R) -> Self {
        Self::new_with_activity(stdin, stdout, None)
    }

    /// Like [`SidecarLink::new`], but re-arms an optional watchdog activity
    /// clock on every decoded frame. The sidecar's `session/ready` (first
    /// byte) + periodic `session/heartbeat` frames keep the supervisor's idle
    /// watchdog from falsely killing a healthy-but-idle process.
    pub fn new_with_activity(stdin: W, stdout: R, activity: Option<Arc<AtomicU64>>) -> Self {
        let writer = Arc::new(Mutex::new(stdin));
        let pending: Arc<Mutex<PendingRequests>> = Arc::new(Mutex::new(HashMap::new()));
        let (inbound_tx, inbound) = channel();

        let reader_pending = Arc::clone(&pending);
        let reader_inbound = inbound_tx.clone();
        let inbound = Arc::new(Mutex::new(inbound));
        std::thread::spawn(move || {
            let mut reader = stdout;
            while let Ok(Some(payload)) = frame::decode(&mut reader) {
                if let Some(clock) = &activity {
                    clock.store(now_ms(), Ordering::Relaxed);
                }
                let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload) else {
                    continue;
                };
                if value.get("method").is_some() {
                    // Coordinator → us: request (id) or notification.
                    let method = value
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string();
                    let params = value
                        .get("params")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let item = if value.get("id").is_some() {
                        Inbound::Request {
                            id: value.get("id").cloned().unwrap_or(serde_json::Value::Null),
                            method,
                            params,
                        }
                    } else {
                        Inbound::Notification { method, params }
                    };
                    let _ = reader_inbound.send(item);
                } else if let Some(id) = value.get("id").and_then(|i| i.as_str()) {
                    // A response to one of our requests.
                    let done = reader_pending
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(id);
                    if let Some(tx) = done {
                        let out = if value.get("error").is_some() {
                            Err(value
                                .get("error")
                                .and_then(|e| e.get("message"))
                                .and_then(|m| m.as_str())
                                .unwrap_or("sidecar error")
                                .to_string())
                        } else {
                            Ok(value
                                .get("result")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null))
                        };
                        let _ = tx.send(out);
                    }
                }
                // Unknown shape → ignored.
            }
            // Fail-closed: resolve every pending request with an error.
            let mut p = reader_pending.lock().unwrap_or_else(|e| e.into_inner());
            for (_, tx) in p.drain() {
                let _ = tx.send(Err("sidecar stdout closed".into()));
            }
        });

        Self {
            writer,
            pending,
            inbound,
            _reader: std::marker::PhantomData,
        }
    }

    /// Cloneable write half for background threads.
    pub fn writer(&self) -> WriterHandle<W> {
        WriterHandle {
            writer: Arc::clone(&self.writer),
        }
    }

    /// Cloneable inbound receiver (the chat relay's consumer loop).
    pub fn receiver(&self) -> Arc<Mutex<Receiver<Inbound>>> {
        Arc::clone(&self.inbound)
    }

    /// Send a request and await the response (fail-closed on sidecar death).
    pub fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, LinkError> {
        let id = next_request_id();
        let (tx, rx) = channel();
        self.pending
            .lock()
            .map_err(|_| LinkError::Poisoned("pending".into()))?
            .insert(id.clone(), tx);
        let req = Request {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params: Some(params),
            id: Some(serde_json::json!(id)),
        };
        self.write_frame(&serde_json::to_vec(&req)?)?;
        match rx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(LinkError::Remote(e)),
            Err(_) => Err(LinkError::Timeout(method.into())),
        }
    }

    /// Reply to a coordinator→us request.
    pub fn reply(&self, id: serde_json::Value, result: serde_json::Value) -> Result<(), LinkError> {
        let resp = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
        self.write_frame(&serde_json::to_vec(&resp)?)
    }

    /// Reply to a coordinator→us request with an error.
    pub fn reply_error(&self, id: serde_json::Value, message: &str) -> Result<(), LinkError> {
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32603, "message": message },
        });
        self.write_frame(&serde_json::to_vec(&resp)?)
    }

    /// Block until the next coordinator request/notification.
    pub fn next_inbound(&self) -> Result<Inbound, RecvError> {
        self.inbound
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .recv()
    }

    /// Non-blocking inbound peek.
    pub fn try_inbound(&self) -> Result<Inbound, TryRecvError> {
        self.inbound
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_recv()
    }

    fn write_frame(&self, payload: &[u8]) -> Result<(), LinkError> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| LinkError::Poisoned("writer".into()))?;
        frame::write_frame(&mut *w, payload).map_err(|e| LinkError::Io(e.to_string()))
    }
}

/// UNIX millisecond timestamp (watchdog activity clock source).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

static REQUEST_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_request_id() -> String {
    format!(
        "r{}",
        REQUEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("io error: {0}")]
    Io(String),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("mutex poisoned: {0}")]
    Poisoned(String),
    #[error("sidecar error: {0}")]
    Remote(String),
    #[error("request '{0}' timed out")]
    Timeout(String),
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;
    #[cfg(unix)]
    use std::os::unix::net::UnixStream;

    /// A scripted fake sidecar: reads frames from its end of the pair and
    /// responds/notifies per the script, then exits (the real coordinator
    /// keeps its stdout open; the fake must not — the reader thread holds the
    /// Rust end open, so an EOF-waiting fake would never see EOF and would
    /// deadlock the test's join).
    #[cfg(unix)]
    fn fake_sidecar(stream: UnixStream) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let mut stream = stream;
            let mut seen = Vec::new();
            while let Ok(Some(payload)) = frame::decode(&mut stream) {
                let value: serde_json::Value = serde_json::from_slice(&payload).unwrap_or_default();
                let method = value
                    .get("method")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string();
                seen.push(method.clone());
                match method.as_str() {
                    "ping" => {
                        let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
                        let resp = serde_json::json!({
                            "jsonrpc": "2.0", "id": id, "result": { "pong": true },
                        });
                        let _ =
                            frame::write_frame(&mut stream, &serde_json::to_vec(&resp).unwrap());
                        break;
                    }
                    "chat/stream" => {
                        let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
                        let resp = serde_json::json!({
                            "jsonrpc": "2.0", "id": id, "result": { "accepted": true },
                        });
                        let _ =
                            frame::write_frame(&mut stream, &serde_json::to_vec(&resp).unwrap());
                        // Then stream a batch + done back to Rust.
                        let n = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "chat/batch",
                            "params": { "streamId": "st-1", "text": "hi", "tokenCount": 1 },
                        });
                        let _ = frame::write_frame(&mut stream, &serde_json::to_vec(&n).unwrap());
                        let d = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "chat/done",
                            "params": { "streamId": "st-1", "turnId": "s1:1", "fullText": "hi", "totalTokens": 1 },
                        });
                        let _ = frame::write_frame(&mut stream, &serde_json::to_vec(&d).unwrap());
                        break;
                    }
                    _ => {}
                }
            }
            seen
        })
    }

    #[cfg(unix)]
    fn pair() -> (UnixStream, UnixStream) {
        UnixStream::pair().expect("socketpair")
    }

    #[cfg(unix)]
    #[test]
    fn request_response_roundtrip() {
        let (a, b) = pair();
        let side = fake_sidecar(b);
        // UnixStream is full-duplex: one end serves as both writer and reader.
        let reader = a.try_clone().expect("clone");
        let link = SidecarLink::new(a, reader);
        let resp = link
            .request("ping", serde_json::json!({}))
            .expect("request");
        assert_eq!(resp["pong"], serde_json::json!(true));
        drop(link);
        let seen = side.join().unwrap();
        assert_eq!(seen, vec!["ping"]);
    }

    #[cfg(unix)]
    #[test]
    fn eof_fails_pending_requests() {
        // Fake sidecar that never responds and closes stdout. Fail-closed: the
        // request must error (either the reader sees EOF → Remote, or the
        // write to the closed peer → Io) — never hang.
        let (a, b) = pair();
        let reader = a.try_clone().expect("clone");
        let link = SidecarLink::new(a, reader);
        drop(b); // sidecar end gone → reader thread sees EOF
        let err = link.request("ping", serde_json::json!({})).unwrap_err();
        assert!(
            matches!(err, LinkError::Remote(_) | LinkError::Io(_)),
            "expected fail-closed error, got {err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn chat_stream_relay_forwards_events() {
        let (a, b) = pair();
        let side = fake_sidecar(b);
        let reader = a.try_clone().expect("clone");
        let link = SidecarLink::new(a, reader);
        let _ = link
            .request(
                "chat/stream",
                serde_json::json!({ "sessionId": "s1", "streamId": "st-1" }),
            )
            .expect("ack");
        // Batch + done notifications arrive on the inbound channel.
        let first = link.next_inbound().expect("batch");
        match first {
            Inbound::Notification { method, params } => {
                assert_eq!(method, "chat/batch");
                assert_eq!(params["text"], serde_json::json!("hi"));
            }
            other => panic!("expected notification, got {other:?}"),
        }
        let second = link.next_inbound().expect("done");
        match second {
            Inbound::Notification { method, params } => {
                assert_eq!(method, "chat/done");
                assert_eq!(params["turnId"], serde_json::json!("s1:1"));
            }
            other => panic!("expected notification, got {other:?}"),
        }
        drop(link);
        let seen = side.join().unwrap();
        assert_eq!(seen, vec!["chat/stream"]);
    }

    #[cfg(unix)]
    #[test]
    fn coordinator_request_is_dispatched_and_replied() {
        // A sidecar that sends a `provider/stream` REQUEST to Rust.
        let (a, b) = pair();
        let side = std::thread::spawn(move || {
            let mut stream = b;
            let req = serde_json::json!({
                "jsonrpc": "2.0", "id": "p1", "method": "provider/stream",
                "params": { "provider": "nvidia", "model": "m" },
            });
            let _ = frame::write_frame(&mut stream, &serde_json::to_vec(&req).unwrap());
            // Read our reply.
            let reply = frame::decode(&mut stream).expect("decode");
            serde_json::from_slice::<serde_json::Value>(&reply.unwrap()).unwrap()
        });
        let reader = a.try_clone().expect("clone");
        let link = SidecarLink::new(a, reader);
        let inbound = link.next_inbound().expect("inbound request");
        match inbound {
            Inbound::Request { id, method, params } => {
                assert_eq!(method, "provider/stream");
                assert_eq!(params["provider"], serde_json::json!("nvidia"));
                link.reply(id, serde_json::json!({ "accepted": true }))
                    .unwrap();
            }
            other => panic!("expected request, got {other:?}"),
        }
        let reply = side.join().unwrap();
        assert_eq!(reply["result"]["accepted"], serde_json::json!(true));
    }

    #[cfg(unix)]
    #[test]
    fn writer_handle_pushes_notifications() {
        let (a, b) = pair();
        let seen = std::thread::spawn(move || {
            let mut stream = b;
            let mut out = Vec::new();
            while let Ok(Some(payload)) = frame::decode(&mut stream) {
                let value: serde_json::Value = serde_json::from_slice(&payload).unwrap();
                out.push(
                    value
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string(),
                );
                if out.len() == 2 {
                    break;
                }
            }
            out
        });
        let reader = a.try_clone().expect("clone");
        let link = SidecarLink::new(a, reader);
        let w = link.writer();
        w.notify(
            "chat/provider_chunk",
            serde_json::json!({ "streamId": "s", "delta": "x" }),
        )
        .unwrap();
        w.notify(
            "chat/provider_chunk",
            serde_json::json!({ "streamId": "s", "ended": true }),
        )
        .unwrap();
        drop(link);
        let out = seen.join().unwrap();
        assert_eq!(out, vec!["chat/provider_chunk", "chat/provider_chunk"]);
    }
}
