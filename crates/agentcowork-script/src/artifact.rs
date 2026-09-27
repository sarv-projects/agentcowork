//! P15-H29 — local dashboard artifacts (doc 67 §1, bolt.diy STEAL).
//!
//! The typed **agent → runtime action stream**: the coordinator parses the
//! agent's stream into [`ArtifactAction`] rows (file writes, shell commands,
//! start/complete markers) and feeds them to an [`ActionRunner`] — the
//! per-action state machine (`pending / running / complete / aborted /
//! failed`) with abort signals and formatted-output errors — exactly the
//! bolt.diy `BoltAction` → `ActionRunner` contract, rebuilt for our
//! `agentcowork-script` sandbox chain.
//!
//! [`ArtifactServer`] then serves the guarded workspace folder on a
//! loopback-only `127.0.0.1:<port>` socket so the views rail can preview the
//! artifact live (the local-first "Sites" surface). Serve/stop are
//! Guard-2-ticketed by the caller — this module owns the runner + the
//! transport, never the approval.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use thiserror::Error;

/// The typed action stream (bolt.diy message-parser shape, JSON-lines).
///
/// Every action carries no id of its own — the *stream position* is the id,
/// so the runner can report which step failed and the UI renders the
/// checklist in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ArtifactAction {
    /// Write (or overwrite) a file inside the workspace.
    #[serde(rename_all = "camelCase")]
    FileWrite {
        path: String,
        content: String,
        #[serde(default)]
        complete: bool,
    },
    /// Run a shell/command step. Execution is the caller's executor seam
    /// (sandbox or host); the runner tracks its lifecycle here.
    #[serde(rename_all = "camelCase")]
    ShellCommand { command: String },
    /// Marks the build phase complete and names the artifact entry point.
    #[serde(rename_all = "camelCase")]
    Start { title: String, entry: String },
    /// Whole-artifact completion marker (the stream is done).
    #[serde(rename_all = "camelCase")]
    Complete { message: String },
}

impl ArtifactAction {
    /// A short human label for the inline checklist.
    pub fn label(&self) -> String {
        match self {
            ArtifactAction::FileWrite { path, .. } => format!("write {path}"),
            ArtifactAction::ShellCommand { command } => format!("run {command}"),
            ArtifactAction::Start { title, .. } => format!("start {title}"),
            ArtifactAction::Complete { .. } => "finish".into(),
        }
    }

    /// The message the action surfaces to the user (snippet-bounded).
    pub fn detail(&self) -> String {
        match self {
            ArtifactAction::FileWrite { content, .. } => {
                let head: String = content.chars().take(140).collect();
                if content.chars().count() > 140 {
                    format!("{head}…")
                } else {
                    head
                }
            }
            ArtifactAction::ShellCommand { command } => command.clone(),
            ArtifactAction::Start { title, entry } => format!("{title} → {entry}"),
            ArtifactAction::Complete { message } => message.clone(),
        }
    }
}

/// Parse an action stream. Accepts both newline-delimited JSON and a JSON
/// array; tolerates blank lines; stops at the first malformed line (the
/// stream contract is strict — a truncated artifact is never half-applied).
pub fn parse_action_stream(input: &str) -> Result<Vec<ArtifactAction>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if let Ok(actions) = serde_json::from_str::<Vec<ArtifactAction>>(trimmed) {
        return Ok(actions);
    }
    let mut actions = Vec::new();
    for (i, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let action: ArtifactAction =
            serde_json::from_str(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        actions.push(action);
    }
    Ok(actions)
}

/// Per-action lifecycle state (bolt.diy `pending | running | complete |
/// aborted | failed` — plus the formatted-output error payload).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum ActionState {
    Pending,
    Running,
    Complete,
    Aborted,
    Failed { formatted: String },
}

impl ActionState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ActionState::Complete | ActionState::Aborted | ActionState::Failed { .. }
        )
    }
}

/// One action + its lifecycle state (the runner's checklist row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRow {
    pub index: usize,
    pub action: ArtifactAction,
    pub state: ActionState,
}

/// The artifact execution state machine: actions advance only forward, an
/// abort signal flips everything unfinished to aborted, and errors carry a
/// formatted (bounded) payload for the UI.
#[derive(Debug, Clone, Default)]
pub struct ActionRunner {
    rows: Vec<ActionRow>,
    aborted: bool,
}

impl ActionRunner {
    pub fn new(actions: Vec<ArtifactAction>) -> Self {
        let rows = actions
            .into_iter()
            .enumerate()
            .map(|(index, action)| ActionRow {
                index,
                action,
                state: ActionState::Pending,
            })
            .collect();
        Self {
            rows,
            aborted: false,
        }
    }

    pub fn rows(&self) -> &[ActionRow] {
        &self.rows
    }

    /// The next not-yet-finished action (None when everything is terminal).
    pub fn next_pending(&self) -> Option<&ActionRow> {
        self.rows.iter().find(|r| !r.state.is_terminal())
    }

    pub fn is_aborted(&self) -> bool {
        self.aborted
    }

    pub fn is_complete(&self) -> bool {
        !self.aborted && self.rows.iter().all(|r| r.state.is_terminal())
    }

    /// Mark a row running. Errors if it is already terminal.
    pub fn begin(&mut self, index: usize) -> Result<(), String> {
        let row = self
            .rows
            .get_mut(index)
            .ok_or_else(|| format!("no such action {index}"))?;
        if row.state.is_terminal() {
            return Err(format!(
                "action {index} already {}",
                serde_json::to_string(&row.state).unwrap_or_default()
            ));
        }
        row.state = ActionState::Running;
        Ok(())
    }

    /// Mark a row complete. Aborted rows stay aborted (output is discarded).
    pub fn finish(&mut self, index: usize) -> Result<(), String> {
        let row = self
            .rows
            .get_mut(index)
            .ok_or_else(|| format!("no such action {index}"))?;
        if row.state == ActionState::Aborted {
            return Ok(());
        }
        row.state = ActionState::Complete;
        Ok(())
    }

    /// Fail a row with a formatted error payload.
    pub fn fail(&mut self, index: usize, formatted: String) -> Result<(), String> {
        let row = self
            .rows
            .get_mut(index)
            .ok_or_else(|| format!("no such action {index}"))?;
        row.state = ActionState::Failed { formatted };
        Ok(())
    }

    /// Abort signal: every non-terminal action flips to aborted (the caller
    /// owns cancelling real work — the stream stops here).
    pub fn abort(&mut self) {
        self.aborted = true;
        for row in &mut self.rows {
            if !row.state.is_terminal() {
                row.state = ActionState::Aborted;
            }
        }
    }

    /// Progress summary for the status bar, e.g. `3/7 · 1 failed`.
    pub fn summary(&self) -> String {
        let done = self.rows.iter().filter(|r| r.state.is_terminal()).count();
        let failed = self
            .rows
            .iter()
            .filter(|r| matches!(r.state, ActionState::Failed { .. }))
            .count();
        match failed {
            0 => format!("{done}/{}", self.rows.len()),
            n => format!("{done}/{} · {n} failed", self.rows.len()),
        }
    }
}

/// Guarded workspace root for artifact writes: every file path must resolve
/// inside it (`..` and absolute paths are refused — the artifact can never
/// escape its folder).
#[derive(Debug, Clone)]
pub struct WorkspaceRoot {
    root: PathBuf,
}

impl WorkspaceRoot {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve `rel` inside the root, refusing escapes. `rel` is a
    /// forward-slash virtual path from the action stream.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, ArtifactError> {
        let rel_path = Path::new(rel);
        if rel_path.is_absolute() {
            return Err(ArtifactError::PathEscape(rel.to_string()));
        }
        let mut out = self.root.clone();
        for comp in rel_path.components() {
            match comp {
                Component::Normal(part) => out.push(part),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(ArtifactError::PathEscape(rel.to_string()));
                }
            }
        }
        if !out.starts_with(&self.root) {
            return Err(ArtifactError::PathEscape(rel.to_string()));
        }
        Ok(out)
    }

    /// Apply one `FileWrite` action (creates parent dirs).
    pub fn write(&self, action: &ArtifactAction) -> Result<PathBuf, ArtifactError> {
        match action {
            ArtifactAction::FileWrite { path, content, .. } => {
                let target = self.resolve(path)?;
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| ArtifactError::Io(e.to_string()))?;
                }
                std::fs::write(&target, content).map_err(|e| ArtifactError::Io(e.to_string()))?;
                Ok(target)
            }
            _ => Err(ArtifactError::NotAWrite),
        }
    }
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("path escapes the workspace: {0}")]
    PathEscape(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("action is not a file write")]
    NotAWrite,
    #[error("server error: {0}")]
    Server(String),
}

/// Minimal safe content-type map for previewed artifacts.
pub fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "txt" | "md" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// A running artifact server on `127.0.0.1:<port>`.
#[derive(Debug)]
pub struct ServerHandle {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ServerHandle {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// Stop the server (idempotent). Guard-2 ticketing is the caller's job —
    /// this is the transport half.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Serve the workspace folder on an ephemeral loopback port. Loopback-only
/// by construction (binds 127.0.0.1), GET-only, path-floored — the artifact
/// can never be reached from the network or read outside its folder.
pub fn serve(workspace: &Path) -> Result<ServerHandle, ArtifactError> {
    let root = std::fs::canonicalize(workspace).map_err(|e| ArtifactError::Io(e.to_string()))?;
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| ArtifactError::Server(e.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|e| ArtifactError::Server(e.to_string()))?
        .port();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let thread = std::thread::spawn(move || {
        listener.set_nonblocking(true).expect("set_nonblocking");
        loop {
            if stop_thread.load(Ordering::SeqCst) {
                return;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let root = root.clone();
                    std::thread::spawn(move || handle_connection(stream, &root));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });
    Ok(ServerHandle {
        port,
        stop,
        thread: Some(thread),
    })
}

fn handle_connection(mut stream: TcpStream, root: &Path) {
    let mut buf = [0u8; 8192];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };
    let request = String::from_utf8_lossy(&buf[..n]);
    let mut lines = request.lines();
    let request_line = lines.next().unwrap_or("");
    let get = request_line.starts_with("GET ");
    let path_part = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/");
    let rel = if path_part == "/" {
        "index.html"
    } else {
        path_part.trim_start_matches('/')
    };

    if !get {
        let _ = write_response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain",
            b"GET only",
        );
        return;
    }
    // Path floor: refuse `..`, absolute, and anything not Normal/CurDir.
    let rel_path = Path::new(rel);
    let safe = rel_path
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if !safe {
        let _ = write_response(&mut stream, "404 Not Found", "text/plain", b"not found");
        return;
    }
    let mut target = root.to_path_buf();
    for comp in rel_path.components() {
        if let Component::Normal(p) = comp {
            target.push(p);
        }
    }
    if !target.starts_with(root) || !target.is_file() {
        let _ = write_response(&mut stream, "404 Not Found", "text/plain", b"not found");
        return;
    }
    let body = match std::fs::read(&target) {
        Ok(b) => b,
        Err(_) => {
            let _ = write_response(&mut stream, "404 Not Found", "text/plain", b"not found");
            return;
        }
    };
    let _ = write_response(&mut stream, "200 OK", content_type(&target), &body);
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    ctype: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nX-Agentcowork-Artifact: 1\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream() -> &'static str {
        r#"
{"type":"file-write","path":"index.html","content":"<h1>hi</h1>"}
{"type":"shell-command","command":"echo built"}
{"type":"start","title":"q3 dashboard","entry":"index.html"}
{"type":"complete","message":"done"}
"#
    }

    #[test]
    fn parses_action_stream_jsonl() {
        let actions = parse_action_stream(stream()).unwrap();
        assert_eq!(actions.len(), 4);
        assert!(
            matches!(actions[0], ArtifactAction::FileWrite { path: ref p, complete: false, .. } if p == "index.html")
        );
        assert!(
            matches!(actions[1], ArtifactAction::ShellCommand { ref command } if command == "echo built")
        );
        assert!(
            matches!(actions[2], ArtifactAction::Start { entry: ref e, .. } if e == "index.html")
        );
        assert!(matches!(actions[3], ArtifactAction::Complete { .. }));
        assert_eq!(actions[0].label(), "write index.html");
    }

    #[test]
    fn parses_action_stream_json_array() {
        let json = r#"[{"type":"file-write","path":"a.txt","content":"x"},{"type":"complete","message":"d"}]"#;
        let actions = parse_action_stream(json).unwrap();
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn parse_rejects_malformed_line() {
        assert!(parse_action_stream("{not json}\n").is_err());
        assert!(parse_action_stream("").unwrap().is_empty());
    }

    #[test]
    fn runner_state_machine_advances_forward() {
        let actions = parse_action_stream(stream()).unwrap();
        let mut runner = ActionRunner::new(actions);
        assert_eq!(runner.rows().len(), 4);
        assert!(!runner.is_complete());

        runner.begin(0).unwrap();
        assert_eq!(runner.rows()[0].state, ActionState::Running);
        runner.finish(0).unwrap();
        assert_eq!(runner.rows()[0].state, ActionState::Complete);

        // Cannot re-begin a complete action.
        assert!(runner.begin(0).is_err());

        runner.begin(1).unwrap();
        runner.fail(1, "exit 1: command not found".into()).unwrap();
        assert!(
            matches!(runner.rows()[1].state, ActionState::Failed { ref formatted } if formatted == "exit 1: command not found")
        );
        assert_eq!(runner.summary(), "2/4 · 1 failed");

        runner.abort();
        assert!(runner.is_aborted());
        assert!(runner.rows().iter().all(|r| r.state.is_terminal()));
        // Aborted actions ignore finish.
        assert!(runner.begin(2).is_err());
        runner.finish(2).unwrap();
        assert_eq!(runner.rows()[2].state, ActionState::Aborted);
        assert!(runner.rows()[3].state.is_terminal());
    }

    #[test]
    fn workspace_root_rejects_escapes() {
        let tmp = std::env::temp_dir().join(format!("artifact-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let root = WorkspaceRoot::new(&tmp);
        assert!(root.resolve("index.html").is_ok());
        assert!(root.resolve("sub/page.html").is_ok());
        assert!(root.resolve("../evil.html").is_err());
        assert!(root.resolve("/etc/passwd").is_err());
        assert!(root.resolve("a/../../evil").is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn workspace_write_creates_parents() {
        let tmp = std::env::temp_dir().join(format!("artifact-test-{}-w", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let root = WorkspaceRoot::new(tmp.clone());
        let target = root
            .write(&ArtifactAction::FileWrite {
                path: "app/main.js".into(),
                content: "console.log(1)".into(),
                complete: true,
            })
            .unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "console.log(1)");
        assert!(tmp.join("app/main.js").is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn server_serves_index_and_404s() {
        let tmp = std::env::temp_dir().join(format!("artifact-test-{}-s", std::process::id()));
        std::fs::create_dir_all(tmp.join("sub")).unwrap();
        std::fs::write(tmp.join("index.html"), "<h1>hello artifact</h1>").unwrap();
        std::fs::write(tmp.join("sub/data.json"), r#"{"ok":true}"#).unwrap();

        let handle = serve(&tmp).unwrap();
        assert!(handle.url().starts_with("http://127.0.0.1:"));
        let url = handle.url();
        let root_resp = http_get(&url);
        assert!(root_resp.starts_with("200"), "got {root_resp}");
        assert!(root_resp.contains("hello artifact"));
        let sub = http_get(&format!("{url}sub/data.json"));
        assert!(sub.starts_with("200") && sub.contains(r#"{"ok":true}"#));
        let missing = http_get(&format!("{url}nope.html"));
        assert!(missing.starts_with("404"));
        let escape = http_get(&format!("{url}../etc/passwd"));
        assert!(escape.starts_with("404"));
        handle.stop();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Minimal GET for the loopback server: returns "STATUS " + body.
    fn http_get(url: &str) -> String {
        let rest = url.trim_start_matches("http://");
        let (host_port, path) = match rest.find('/') {
            Some(i) => rest.split_at(i),
            None => (rest, "/"),
        };
        let (host, port) = host_port.rsplit_once(':').unwrap();
        let mut stream =
            std::net::TcpStream::connect((host, port.parse::<u16>().unwrap())).unwrap();
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n",
            if path.is_empty() { "/" } else { path }
        );
        let _ = stream.write_all(req.as_bytes());
        let mut out = String::new();
        let _ = stream.read_to_string(&mut out);
        let mut lines = out.lines();
        let status = lines
            .next()
            .unwrap_or("")
            .trim_start_matches("HTTP/1.1 ")
            .to_string();
        let mut body = String::new();
        let mut in_body = false;
        for line in lines {
            if in_body {
                body.push_str(line);
                body.push('\n');
            }
            if line.is_empty() {
                in_body = true;
            }
        }
        format!("{status} {body}")
    }
}
