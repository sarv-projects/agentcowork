//! LSP session runtime (I11 — doc 63 §2.1, neovim `vim.lsp` pattern): the
//! spawn + keep-alive half of the code-intel client. A [`LspSession`] wraps a
//! [`LspTransport`] and drives the JSON-RPC lifecycle — `initialize` →
//! `initialized` notification → requests/notifications → `shutdown`/`exit`.
//!
//! The transport is a trait so tests drive the handshake with a scripted mock
//! while the real [`ProcessTransport`] spawns a language-server binary over
//! stdio (the LSP wire protocol).

use crate::lsp::{decode_messages, encode_message};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{self, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use thiserror::Error;

/// Errors surfaced by an LSP session.
#[derive(Debug, Error)]
pub enum LspSessionError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("language server closed the stream (EOF)")]
    Eof,
    #[error("malformed server message: {0}")]
    Malformed(String),
    #[error("server returned an error response: {0}")]
    ServerError(String),
    #[error("response id mismatch: expected {expected}, got {got}")]
    IdMismatch { expected: u64, got: u64 },
}

/// A bidirectional JSON-RPC transport to a language server.
pub trait LspTransport {
    /// Send one JSON-RPC message (framed with `Content-Length`).
    fn send(&mut self, json: &str) -> io::Result<()>;
    /// Blocking read of the next complete message. `Ok(None)` on EOF.
    fn recv(&mut self) -> io::Result<Option<String>>;
    /// Is the server process still alive?
    fn is_alive(&mut self) -> bool;
    /// Tear the process down (kill + reap).
    fn shutdown(&mut self);
}

/// Borrowing a transport is a transport (lets a session share a mock while
/// the caller inspects what was sent).
impl<T: LspTransport + ?Sized> LspTransport for &mut T {
    fn send(&mut self, json: &str) -> io::Result<()> {
        (**self).send(json)
    }
    fn recv(&mut self) -> io::Result<Option<String>> {
        (**self).recv()
    }
    fn is_alive(&mut self) -> bool {
        (**self).is_alive()
    }
    fn shutdown(&mut self) {
        (**self).shutdown();
    }
}

/// stdio transport over a spawned language-server process (the standard LSP
/// wire transport: JSON-RPC frames over stdin/stdout, stderr discarded).
pub struct ProcessTransport {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    /// Partial frames from the server accumulate here until complete.
    buf: Vec<u8>,
    /// Decoded frames not yet handed out. `decode_messages` can yield several
    /// complete frames from one read; queue them rather than dropping them, or
    /// a fast server's response can be lost and the caller block forever.
    pending: VecDeque<String>,
}

impl ProcessTransport {
    /// Spawn `command` with `args` and take over its stdio.
    pub fn spawn(command: &str, args: &[&str]) -> io::Result<Self> {
        Self::spawn_env(command, args, &[])
    }

    /// Spawn `command` with `args` + `env` overrides and take over its stdio
    /// (the live-LSP seam for `lsp-config.json` per-server environment).
    pub fn spawn_env(command: &str, args: &[&str], env: &[(&str, &str)]) -> io::Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "no stdin on spawned server")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "no stdout on spawned server")
        })?;
        Ok(Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            buf: Vec::new(),
            pending: VecDeque::new(),
        })
    }
}

impl LspTransport for ProcessTransport {
    fn send(&mut self, json: &str) -> io::Result<()> {
        self.stdin.write_all(encode_message(json).as_bytes())?;
        self.stdin.flush()
    }

    fn recv(&mut self) -> io::Result<Option<String>> {
        loop {
            // Serve frames already decoded but not yet returned (they arrived
            // together in one read chunk).
            if let Some(m) = self.pending.pop_front() {
                return Ok(Some(m));
            }
            if let Ok(msgs) = decode_messages(&mut self.buf) {
                if !msgs.is_empty() {
                    self.pending.extend(msgs);
                    if let Some(m) = self.pending.pop_front() {
                        return Ok(Some(m));
                    }
                }
            }
            let mut chunk = [0u8; 8192];
            let n = self.reader.read(&mut chunk)?;
            if n == 0 {
                // EOF: flush any partial trailing frame before signalling end.
                if let Ok(msgs) = decode_messages(&mut self.buf) {
                    if let Some(m) = msgs.into_iter().next() {
                        return Ok(Some(m));
                    }
                }
                return Ok(None); // EOF — server closed stdout
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// An LSP session: one server process + the JSON-RPC request/response state.
pub struct LspSession<T: LspTransport> {
    transport: T,
    next_id: u64,
    initialized: bool,
}

impl<T: LspTransport> LspSession<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_id: 1,
            initialized: false,
        }
    }

    /// Perform the LSP handshake: send `initialize`, wait for its response,
    /// then send the `initialized` notification. Returns the server's
    /// capabilities result (best-effort — callers read what they need).
    pub fn initialize(
        &mut self,
        root_uri: &str,
        client_name: &str,
    ) -> Result<Value, LspSessionError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "capabilities": {},
                "clientInfo": { "name": client_name }
            }
        });
        self.transport.send(&req.to_string())?;
        let resp = self.read_response(id)?;
        let initialized = json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} });
        self.transport.send(&initialized.to_string())?;
        self.initialized = true;
        Ok(resp)
    }

    /// Send a request and wait for the response with the matching id.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, LspSessionError> {
        if !self.initialized {
            return Err(LspSessionError::ServerError(
                "session not initialized".into(),
            ));
        }
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        self.transport.send(&req.to_string())?;
        self.read_response(id)
    }

    /// Send a fire-and-forget notification (no response expected).
    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), LspSessionError> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        self.transport.send(&msg.to_string())?;
        Ok(())
    }

    /// Is the underlying server process still alive? (keep-alive probe)
    pub fn is_alive(&mut self) -> bool {
        self.transport.is_alive()
    }

    /// Read the next server message and, if it is a
    /// `textDocument/publishDiagnostics` notification, return the parsed
    /// batch. Returns `Ok(None)` when the next message is *not* a
    /// diagnostics notification (e.g. a log/telemetry message) or the stream
    /// hit EOF. Used by the LSP-config diagnostics service (P6.8).
    pub fn recv_diagnostics(
        &mut self,
    ) -> Result<Option<crate::lsp_config::DiagnosticBatch>, LspSessionError> {
        let Some(raw) = self.transport.recv()? else {
            return Ok(None);
        };
        let v: Value =
            serde_json::from_str(&raw).map_err(|e| LspSessionError::Malformed(e.to_string()))?;
        if v.get("method").and_then(Value::as_str) != Some("textDocument/publishDiagnostics") {
            return Ok(None);
        }
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        let uri = params
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let diags: Vec<crate::lsp::Diagnostic> = params
            .get("diagnostics")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| LspSessionError::Malformed(e.to_string()))?
            .unwrap_or_default();
        Ok(Some(crate::lsp_config::DiagnosticBatch {
            uri,
            diagnostics: diags,
        }))
    }

    /// Graceful shutdown: `shutdown` request → `exit` notification → reap.
    pub fn shutdown(&mut self) {
        if self.initialized {
            let _ = self.request("shutdown", json!(null));
            let _ = self.notify("exit", json!(null));
        }
        self.transport.shutdown();
    }

    /// Read a response and check its id; surfaces server errors.
    fn read_response(&mut self, expected_id: u64) -> Result<Value, LspSessionError> {
        let Some(raw) = self.transport.recv()? else {
            return Err(LspSessionError::Eof);
        };
        let v: Value =
            serde_json::from_str(&raw).map_err(|e| LspSessionError::Malformed(e.to_string()))?;
        let got = v
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| LspSessionError::Malformed("response without id".into()))?;
        if got != expected_id {
            return Err(LspSessionError::IdMismatch {
                expected: expected_id,
                got,
            });
        }
        if let Some(err) = v.get("error") {
            return Err(LspSessionError::ServerError(err.to_string()));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A scripted mock transport: canned responses, records everything sent.
    struct MockTransport {
        responses: VecDeque<String>,
        sent: Vec<String>,
        alive: bool,
    }

    impl MockTransport {
        fn new(responses: Vec<&str>) -> Self {
            Self {
                responses: responses.into_iter().map(str::to_string).collect(),
                sent: Vec::new(),
                alive: true,
            }
        }
    }

    impl LspTransport for MockTransport {
        fn send(&mut self, json: &str) -> io::Result<()> {
            self.sent.push(json.to_string());
            Ok(())
        }
        fn recv(&mut self) -> io::Result<Option<String>> {
            Ok(self.responses.pop_front())
        }
        fn is_alive(&mut self) -> bool {
            self.alive
        }
        fn shutdown(&mut self) {
            self.alive = false;
        }
    }

    fn result_response(id: u64, result: Value) -> String {
        json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
    }

    #[test]
    fn initialize_handshake_sends_both_messages() {
        let mut t = MockTransport::new(vec![&result_response(
            1,
            json!({ "capabilities": { "hoverProvider": true } }),
        )]);
        let mut session = LspSession::new(&mut t);
        let caps = session
            .initialize("file:///workspace", "agentcowork")
            .unwrap();
        assert_eq!(caps["capabilities"]["hoverProvider"], true);
        // Sent initialize, then the initialized notification.
        assert_eq!(t.sent.len(), 2);
        let first: Value = serde_json::from_str(&t.sent[0]).unwrap();
        assert_eq!(first["method"], "initialize");
        assert_eq!(first["params"]["rootUri"], "file:///workspace");
        let second: Value = serde_json::from_str(&t.sent[1]).unwrap();
        assert_eq!(second["method"], "initialized");
    }

    #[test]
    fn request_waits_for_matching_id() {
        // One canned response per read: initialize (id 1) then hover (id 2).
        let mut t = MockTransport::new(vec![
            &result_response(1, json!({ "capabilities": {} })),
            &result_response(2, json!({ "contents": "x" })),
        ]);
        let mut session = LspSession::new(&mut t);
        session.initialize("file:///w", "agentcowork").unwrap();
        let res = session
            .request("textDocument/hover", json!({ "uri": "file:///a.rs" }))
            .unwrap();
        assert_eq!(res["contents"], "x");
        // The request carried id 2 (id 1 was initialize); sent[2] is the
        // hover request (initialize + initialized notifications came first).
        let req: Value = serde_json::from_str(&t.sent[2]).unwrap();
        assert_eq!(req["id"], 2);
        assert_eq!(req["method"], "textDocument/hover");
    }

    #[test]
    fn request_before_initialize_fails() {
        let mut t = MockTransport::new(vec![]);
        let mut session = LspSession::new(&mut t);
        assert!(matches!(
            session.request("textDocument/hover", json!({})),
            Err(LspSessionError::ServerError(_))
        ));
    }

    #[test]
    fn id_mismatch_and_server_error_surface() {
        // Response id 99 vs the initialize id 1 → IdMismatch.
        let mut t = MockTransport::new(vec![&result_response(99, json!({}))]);
        let mut session = LspSession::new(&mut t);
        assert!(matches!(
            session.initialize("file:///w", "x"),
            Err(LspSessionError::IdMismatch { .. })
        ));

        let mut t2 = MockTransport::new(vec![
            &json!({
                "jsonrpc": "2.0", "id": 1,
                "error": { "code": -32603, "message": "boom" }
            })
            .to_string(),
        ]);
        let mut session2 = LspSession::new(&mut t2);
        assert!(matches!(
            session2.initialize("file:///w", "x"),
            Err(LspSessionError::ServerError(_))
        ));
    }

    #[test]
    fn eof_is_surfaced() {
        let mut t = MockTransport::new(vec![]); // no responses → EOF on first recv
        let mut session = LspSession::new(&mut t);
        assert!(matches!(
            session.initialize("file:///w", "x"),
            Err(LspSessionError::Eof)
        ));
    }

    #[test]
    fn shutdown_marks_dead() {
        let mut t = MockTransport::new(vec![&result_response(1, json!({}))]);
        let mut session = LspSession::new(&mut t);
        session.initialize("file:///w", "x").unwrap();
        assert!(session.is_alive());
        session.shutdown();
        assert!(!session.is_alive());
    }

    /// Real process smoke test: spawn `cat` (echoes its stdin) over the stdio
    /// transport and verify a framed message round-trips through the pipes.
    #[cfg(unix)]
    #[test]
    fn process_transport_roundtrips_through_stdio() {
        let mut t = ProcessTransport::spawn("cat", &[]).unwrap();
        assert!(t.is_alive());
        t.send(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
            .unwrap();
        let echoed = t.recv().unwrap().expect("cat must echo");
        assert_eq!(echoed, r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
        t.shutdown();
        assert!(!t.is_alive());
    }

    #[test]
    fn spawn_missing_binary_errors() {
        assert!(ProcessTransport::spawn("definitely-not-a-real-lsp", &[]).is_err());
    }

    /// Regression: two frames arriving in one read chunk must BOTH be
    /// delivered — the old `recv` kept only the first and dropped the rest.
    /// The fixture emits two proper Content-Length frames back-to-back.
    #[cfg(unix)]
    #[test]
    fn recv_queues_multiple_frames_from_one_chunk() {
        let frames = "Content-Length: 3\r\n\r\nabcContent-Length: 3\r\n\r\ndef";
        let cmd = format!("printf '{}'", frames);
        let mut t = ProcessTransport::spawn("sh", &["-c", &cmd]).unwrap();
        assert_eq!(t.recv().unwrap().as_deref(), Some("abc"));
        assert_eq!(t.recv().unwrap().as_deref(), Some("def"));
        assert_eq!(t.recv().unwrap(), None);
        t.shutdown();
    }
}
