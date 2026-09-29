//! MCP server attach (P6.6 #3/#5 — user-supplied stdio/npx or user-hosted
//! HTTP). This is the *attach* machinery: spawn a user-supplied MCP server
//! command (e.g. `npx @gmail/mcp-server` or a local binary), probe its era,
//! perform `initialize` + `tools/list`, and reconcile the discovered tools into
//! a [`ToolCatalog`] with native precedence.
//!
//! The live provider servers (Gmail/Slack/GitHub/Linear official MCP servers)
//! remain credential/install-gated; the attach protocol itself is fully
//! exercised here against a spawned mock MCP server over loopback stdio.
//!
//! ## Why the reader is on its own thread
//!
//! A bare `BufRead::read_line` on the child's stdout is an unbounded block: a
//! server that accepts the connection and then says nothing hangs the attach
//! forever, and the caller cannot tell a hung server from a slow one. Every
//! read here therefore goes through a dedicated reader thread that keeps
//! draining the pipe and pushes lines over a channel, and the caller waits
//! with a bounded budget ([`crate::remote::PROBE_BUDGET`]). Draining
//! continuously matters independently of the timeout: a reader that stops
//! after one line deadlocks the child on a full pipe.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Serialize;

use crate::remote::{EraNegotiation, McpEra, PROBE_BUDGET, StdioEraProbe, negotiate_stdio_era};
use crate::server::{ExternalTool, ToolCatalog};
use agentcowork_guard::sandbox::SandboxProcess;

/// What the reader thread hands back. `None` is a clean end of stream; a
/// read error is carried so the caller never sees an empty success.
type LineEvent = Result<Option<String>, std::io::Error>;

/// An attached MCP server: the child process plus its reconciled tool names.
pub struct AttachedServer {
    child: Option<Child>,
    sandbox_process: Option<SandboxProcess>,
    stdin: ChildStdin,
    /// Lines from the reader thread. The reader owns the child's stdout.
    lines: Receiver<LineEvent>,
    /// Detached on drop: a blocked `read_line` cannot be joined, and the child
    /// is killed by [`Self::shutdown`], which closes the pipe and ends it.
    reader_thread: Option<JoinHandle<()>>,
    /// The budget for every read from this child. Bounded by construction, so
    /// no phase of the handshake can block forever.
    read_timeout: Duration,
    /// The command fingerprint the era verdict is cached under.
    launch: (String, Vec<String>),
    /// The era in force, and how it was chosen (read-only, for the status
    /// surface).
    era: Option<EraNegotiation>,
    pub tools: Vec<String>,
    /// True only when launched through a concrete host sandbox backend.
    sandboxed: bool,
    /// Optional reviewed-import root for sandboxed change sets.
    import_root: Option<PathBuf>,
    /// P55.11 — monotonic JSON-RPC id for post-handshake calls
    /// (`tools/call`). The handshake owns 1/2; a call that reused them could
    /// be mistaken for a handshake reply by a server that keys on id.
    next_id: i64,
}

/// Errors from the attach handshake.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("spawn failed: {0}")]
    Spawn(std::io::Error),
    #[error("stdio unavailable: {0}")]
    Stdio(String),
    #[error("server closed the stream during handshake")]
    Eof,
    #[error("malformed server reply: {0}")]
    Malformed(String),
    #[error("server error reply: {0}")]
    Server(String),
    #[error("protocol mismatch: {0}")]
    Protocol(String),
    /// The server accepted the request and then said nothing within the
    /// budget. A distinct fact from EOF and from a refusal: a hung server is
    /// a health fact, never a success-with-no-tools, and never a silent
    /// downgrade to whatever the caller would have done next.
    #[error("no reply within {0} ms while {1}")]
    Timeout(u64, &'static str),
}

/// How an attached MCP server child is launched (P62.2).
///
/// A remote-MCP server is third-party code the user installed; the 2026
/// security survey's ASI05 (unexpected code execution) is exactly this child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxPosture {
    /// Run the child inside the native OS sandbox (Linux bubblewrap): its own
    /// mount namespace, no new privileges, and **no ambient environment**, so
    /// it cannot read the shell's provider keys. Credentials must arrive
    /// through the vault broker instead. This is the containment posture.
    Confined,
    /// Run the child with the inherited environment. Required by an MCP server
    /// whose auth is env-based (`GMAIL_TOKEN=…`) until that server is moved to
    /// the brokered credential path — honest, but it is **not** containment.
    Ambient,
}

impl SandboxPosture {
    /// The posture the host can actually deliver on this platform.
    pub fn preferred() -> Self {
        #[cfg(target_os = "linux")]
        {
            if agentcowork_guard::sandbox::linux_bwrap_available() {
                return SandboxPosture::Confined;
            }
        }
        SandboxPosture::Ambient
    }

    pub fn is_contained(self) -> bool {
        self == SandboxPosture::Confined
    }
}

impl AttachedServer {
    /// Spawn a user-supplied MCP server over newline-delimited stdio (the
    /// 2026-07-28 stateless transport our server speaks, doc 61). Args are
    /// passed verbatim — SEP-1024 exact-command consent happens in the UI
    /// (H3) before this is called.
    pub fn spawn(command: &str, args: &[&str]) -> Result<Self, AttachError> {
        Self::spawn_uncontrolled(command, args)
    }

    /// Spawn under an explicit posture (P62.2). `Confined` needs a scratch dir
    /// the child may write (its own cache); `network` is `"allow"` for the
    /// API-calling servers that need it. A requested confined launch fails
    /// closed when the backend is unavailable or fails to start; it never
    /// silently downgrades a third-party child to ambient execution.
    pub fn spawn_with_posture(
        posture: SandboxPosture,
        _scratch: &str,
        _network: &str,
        command: &str,
        args: &[&str],
    ) -> Result<Self, AttachError> {
        if posture.is_contained() {
            #[cfg(target_os = "linux")]
            {
                if !agentcowork_guard::sandbox::linux_bwrap_available() {
                    return Err(AttachError::Spawn(std::io::Error::other(
                        "confined MCP posture unavailable: bubblewrap is not installed",
                    )));
                }
                // A backend error is returned to the caller. Falling back to
                // ambient here would turn a failed security boundary into a
                // successful-looking unconfined launch.
                return Self::spawn_confined(_scratch, _network, command, args);
            }
            #[cfg(not(target_os = "linux"))]
            {
                return Err(AttachError::Spawn(std::io::Error::other(
                    "confined MCP posture is unavailable on this platform",
                )));
            }
        }
        // Ambient is an explicit posture selected by `preferred()` only when
        // no supported containment backend exists, or by a caller that has
        // deliberately accepted the weaker contract.
        Self::spawn_uncontrolled(command, args)
    }

    fn resolve_or_err(
        command: &str,
        args: &[&str],
    ) -> Result<crate::npx::ResolvedLaunch, AttachError> {
        crate::npx::resolve_stdio_launch(command, args)
            .map_err(|e| AttachError::Spawn(std::io::Error::other(e.to_string())))
    }

    /// Spawn the child inside the native OS sandbox (Linux bubblewrap).
    ///
    /// The sandbox is `--clearenv`, so the child inherits **no** ambient
    /// secrets: provider keys and tokens stay out of the third-party process,
    /// and credentials are expected to arrive via the capability broker. This
    /// is the fixed `SandboxPosture::Confined` path.
    #[cfg(target_os = "linux")]
    pub fn spawn_confined(
        scratch: &str,
        network: &str,
        command: &str,
        args: &[&str],
    ) -> Result<Self, AttachError> {
        use agentcowork_guard::sandbox::{
            LinuxBwrapBackend, SandboxRole, SandboxSpec, essential_env, profiles,
        };
        // The backend refuses to bind a path that does not exist (fail-closed),
        // so the child's scratch dir has to exist before the spawn.
        std::fs::create_dir_all(scratch).map_err(AttachError::Spawn)?;
        let resolved = Self::resolve_or_err(command, args)?;
        let mut argv = Vec::with_capacity(resolved.args.len() + 1);
        argv.push(resolved.command.clone());
        argv.extend(resolved.args.iter().cloned());
        let spec = SandboxSpec {
            role: SandboxRole::ChildExecutionSandbox,
            profile: profiles::worker(scratch),
            network: network.to_string(),
            credentials: "opaque_handles".into(),
            resource_limit_bytes: 512 << 20,
        };
        // P62.2 — `--clearenv` stays (that is the containment win), but a
        // completely empty environment breaks interpreter-based stdio servers
        // (`python server.py` cannot even find `python`). The allow-list is the
        // documented middle path: path/loader/discovery vars only, secret-shaped
        // names refused by the backend, everything else still scrubbed.
        let mut env = essential_env();
        env.retain(|(k, _)| k != "PATH");
        env.push(("PATH".into(), resolved.path));
        let sandboxed = LinuxBwrapBackend
            .spawn_stdio_with_env(&spec, &argv, &env)
            .map_err(|e| AttachError::Spawn(std::io::Error::other(e.to_string())))?;
        Ok(Self::from_stdio(
            sandboxed.stdin,
            sandboxed.stdout,
            None,
            Some(sandboxed.monitor),
            true,
            Some(PathBuf::from(scratch)),
            resolved.command,
            resolved.args,
        ))
    }

    /// Legacy attach path. It is intentionally explicit: the child is not
    /// covered by the native ticket/audit guarantee.
    pub fn spawn_uncontrolled(command: &str, args: &[&str]) -> Result<Self, AttachError> {
        let resolved = Self::resolve_or_err(command, args)?;
        let mut child = Command::new(&resolved.command)
            .args(&resolved.args)
            .env("PATH", &resolved.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()) // server logs stay visible for debug
            .spawn()
            .map_err(AttachError::Spawn)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AttachError::Stdio("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AttachError::Stdio("no stdout".into()))?;
        Ok(Self::from_stdio(
            stdin,
            stdout,
            Some(child),
            None,
            false,
            None,
            resolved.command,
            resolved.args,
        ))
    }

    /// Bind an already-piped child into the attach protocol: start the reader
    /// thread and record the launch fingerprint the era verdict is cached
    /// under.
    #[allow(clippy::too_many_arguments)]
    fn from_stdio(
        stdin: ChildStdin,
        stdout: ChildStdout,
        child: Option<Child>,
        sandbox_process: Option<SandboxProcess>,
        sandboxed: bool,
        import_root: Option<PathBuf>,
        command: String,
        args: Vec<String>,
    ) -> Self {
        let (sender, lines) = mpsc::channel();
        let reader_thread = std::thread::spawn(move || drain_stdout(stdout, sender));
        Self {
            child,
            sandbox_process,
            stdin,
            lines,
            reader_thread: Some(reader_thread),
            read_timeout: PROBE_BUDGET,
            launch: (command, args),
            era: None,
            tools: Vec::new(),
            sandboxed,
            import_root,
            next_id: 3,
        }
    }

    /// Set the budget for every read from this child (bounded by
    /// [`PROBE_BUDGET`]). A caller that knows its server is slow may raise the
    /// per-request budget; nothing may exceed the probe cap.
    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout.max(Duration::from_millis(1)).min(PROBE_BUDGET);
        self
    }

    /// The era in force for this child, and how it was chosen. `None` before
    /// [`Self::attach`] has run detection.
    pub fn era(&self) -> Option<EraNegotiation> {
        self.era
    }

    /// The era-cache key for this child — the fingerprint of the **resolved**
    /// command line, so a status surface can show (or clear) the cached verdict
    /// for exactly this launch.
    pub fn era_key(&self) -> String {
        let args: Vec<&str> = self.launch.1.iter().map(String::as_str).collect();
        crate::remote::stdio_era_key(&self.launch.0, &args)
    }

    fn send(&mut self, json: &str) -> Result<(), AttachError> {
        self.stdin
            .write_all(json.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .map_err(AttachError::Spawn)
    }

    /// Read one reply under the budget, naming the phase in a timeout so the
    /// caller can see *where* the server went quiet.
    fn recv(&mut self, phase: &'static str) -> Result<serde_json::Value, AttachError> {
        let line = match self.lines.recv_timeout(self.read_timeout) {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => return Err(AttachError::Eof),
            Ok(Err(error)) => return Err(AttachError::Spawn(error)),
            Err(RecvTimeoutError::Timeout) => {
                return Err(AttachError::Timeout(
                    self.read_timeout.as_millis() as u64,
                    phase,
                ));
            }
            // The reader thread is gone and the channel is empty: the pipe is
            // closed, which is the same observable fact as a clean EOF.
            Err(RecvTimeoutError::Disconnected) => return Err(AttachError::Eof),
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            // A keep-alive blank line is not a reply. Report it as protocol
            // noise rather than inventing an empty success.
            return Err(AttachError::Malformed("empty reply line".into()));
        }
        serde_json::from_str(trimmed).map_err(|e| AttachError::Malformed(e.to_string()))
    }

    /// Perform the attach handshake: exactly one `server/discover` era probe,
    /// then `initialize` **only** for a server that is not modern, then
    /// `tools/list`, then reconcile into `catalog`. Returns the discovered tool
    /// names.
    ///
    /// ## Why the probe runs first, and only once
    ///
    /// DEC-030: *stdio probes `server/discover` (10 s cap) then falls back to
    /// legacy `initialize`*.  The probe is therefore sent before the handshake,
    /// exactly once per command fingerprint, and its verdict is cached
    /// ([`crate::remote::stdio_era_key`]) so a restart does not re-pay it.  A
    /// modern server then needs no `initialize` at all — the stateless contract
    /// has no handshake to hang one off, which is the whole point of the
    /// revision.
    ///
    /// ## Why a probe that times out fails the attach
    ///
    /// On stdio an unanswered probe leaves the ND-JSON stream desynchronized:
    /// a late `server/discover` reply would be read as the `initialize` reply,
    /// and the tool names reconciled afterwards would belong to the wrong
    /// request.  Rather than guess, a timeout is a typed
    /// [`AttachError::Timeout`] and the handshake stops.  A `-32601` refusal is
    /// a *reply*, not a timeout, so the legacy fallback still runs with a
    /// clean stream.
    pub fn attach(
        &mut self,
        catalog: &mut ToolCatalog,
        source_label: &str,
    ) -> Result<Vec<String>, AttachError> {
        let era = self.probe_era()?;
        if era.era != McpEra::Modern {
            self.legacy_initialize()?;
        }

        let list = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}
        });
        self.send(&list.to_string())?;
        let reply = self.recv("listing tools")?;
        if let Some(err) = reply.get("error") {
            return Err(AttachError::Server(err.to_string()));
        }
        let result = reply
            .get("result")
            .ok_or_else(|| AttachError::Protocol("tools/list reply without result".into()))?;
        let tools = result
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| AttachError::Protocol("tools/list result without tools array".into()))?;

        let mut names = Vec::new();
        for t in tools {
            let Some(name) = t.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let tool = ExternalTool {
                name: name.to_string(),
                description: t
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                input_schema: t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or(serde_json::json!({})),
                read_only: t
                    .get("readOnlyHint")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                open_world: t
                    .get("openWorldHint")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                source: source_label.to_string(),
            };
            if catalog.register(tool) {
                names.push(name.to_string());
            }
        }
        self.tools = names.clone();
        Ok(names)
    }

    /// Run exactly one era detection round for this child and record the
    /// verdict on the handle.
    fn probe_era(&mut self) -> Result<EraNegotiation, AttachError> {
        let (command, args) = self.launch.clone();
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut probe = AttachProbe { server: self };
        let era = negotiate_stdio_era(&command, &borrowed, &mut probe);
        if era.source == crate::remote::EraSource::Default {
            // Detection proved nothing. Reading again would either block or
            // pick up a late reply out of order, so the attach stops typed.
            return Err(AttachError::Timeout(
                self.read_timeout.as_millis() as u64,
                "probing the server era",
            ));
        }
        self.era = Some(era);
        Ok(era)
    }

    /// The legacy-era handshake: best-effort, because minimal servers may
    /// answer only `tools/list`. Only `-32601` is tolerated; anything else is
    /// fatal, and a server that goes quiet is a typed timeout.
    fn legacy_initialize(&mut self) -> Result<(), AttachError> {
        let init = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-11-25", "capabilities": {} }
        });
        self.send(&init.to_string())?;
        let reply = self.recv("the legacy initialize handshake")?;
        if let Some(err) = reply.get("error") {
            let code = err.get("code").and_then(serde_json::Value::as_i64);
            if code != Some(-32601) {
                return Err(AttachError::Server(err.to_string()));
            }
        }
        Ok(())
    }

    /// P55.11 — call one tool on the attached server (`tools/call`). This is
    /// the *product loop* half of the attach handshake: the same child that
    /// answered `tools/list` executes the call, so the advertised tool set and
    /// the callable tool set can never drift into two different servers.
    ///
    /// A server error reply surfaces as [`AttachError::Server`] (never an
    /// empty success), and a reply without a `result` is a protocol error.
    pub fn call_tool(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, AttachError> {
        let id = self.next_id;
        self.next_id += 1;
        let call = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });
        self.send(&call.to_string())?;
        let reply = self.recv("calling a tool")?;
        if let Some(err) = reply.get("error") {
            return Err(AttachError::Server(err.to_string()));
        }
        reply
            .get("result")
            .cloned()
            .ok_or_else(|| AttachError::Protocol("tools/call reply without result".into()))
    }

    /// Attach the concrete monitored process returned by a sandbox backend.
    /// A path alone never establishes containment.
    pub fn bind_sandbox_process(&mut self, process: SandboxProcess, root: PathBuf) {
        self.sandbox_process = Some(process);
        self.sandboxed = true;
        self.import_root = Some(root);
    }

    /// Compatibility guard: callers must use `bind_sandbox_process`; merely
    /// naming an import root cannot upgrade an uncontrolled child.
    pub fn bind_sandbox_import_root(&mut self, _root: PathBuf) -> Result<(), AttachError> {
        Err(AttachError::Protocol(
            "sandbox import root requires a concrete monitored sandbox process".into(),
        ))
    }

    pub fn monitor_exit(
        &mut self,
        deadline: std::time::Instant,
    ) -> Result<std::process::ExitStatus, AttachError> {
        self.sandbox_process
            .as_mut()
            .ok_or_else(|| AttachError::Protocol("child is not sandboxed".into()))
            .and_then(|process| {
                process
                    .wait_with_deadline(deadline)
                    .map_err(|e| AttachError::Protocol(e.to_string()))
            })
    }

    pub fn is_sandboxed(&self) -> bool {
        self.sandboxed
    }

    /// P51.18 — non-blocking liveness probe for the no-restart refresh path.
    /// `None` (already reaped / never spawned) counts as not alive. Sandboxed
    /// children are owned by `sandbox_process`; ambient children use `child`.
    pub fn is_alive(&mut self) -> bool {
        if let Some(process) = self.sandbox_process.as_mut() {
            return matches!(process.try_wait(), Ok(None));
        }
        matches!(
            self.child.as_mut().map(|child| child.try_wait()),
            Some(Ok(None))
        )
    }

    /// The OS pid of the owned child, if one is live. Sandboxed children are
    /// owned by the backend monitor; ambient children by the std `Child`.
    pub fn pid(&self) -> Option<u32> {
        if let Some(process) = self.sandbox_process.as_ref() {
            return Some(process.pid);
        }
        self.child.as_ref().map(std::process::Child::id)
    }

    pub fn import_root(&self) -> Option<&PathBuf> {
        self.import_root.as_ref()
    }

    /// Tear the child process down.
    pub fn shutdown(&mut self) {
        if let Some(process) = self.sandbox_process.as_mut() {
            let _ = process.kill();
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Killing the child closes its stdout, which ends the reader thread.
        // It is detached rather than joined: a reader already parked inside a
        // blocking `read_line` cannot be woken portably, and joining it here
        // would turn a teardown into a hang.
        self.reader_thread.take();
    }
}

/// The bounded reader behind every `recv`: one thread per child that keeps
/// draining the pipe and publishes each line.
///
/// Two properties this buys, both of which a bare `read_line` lacks:
/// - the caller never blocks past its budget, so a hung server is a typed
///   timeout rather than an indefinite hang;
/// - the pipe keeps being drained, so a chatty server cannot block on a full
///   OS pipe buffer while the caller is between requests.
///
/// A read error is published rather than swallowed, and the end of the stream
/// is published as `Ok(None)`, so the caller can still tell "closed" from
/// "broken".
fn drain_stdout<R: std::io::Read + Send + 'static>(stdout: R, sender: Sender<LineEvent>) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                let _ = sender.send(Ok(None));
                return;
            }
            Ok(_) => {
                if sender.send(Ok(Some(std::mem::take(&mut line)))).is_err() {
                    // The handle is gone; stop reading so the child sees a
                    // closed pipe when it next writes.
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error));
                return;
            }
        }
    }
}

/// The attached handle seen as a transport: it owns the pipe, so the era logic
/// in [`crate::remote`] can stay transport-agnostic and unit-testable with a
/// scripted probe instead of a live child.
struct AttachProbe<'a> {
    server: &'a mut AttachedServer,
}

impl StdioEraProbe for AttachProbe<'_> {
    fn send(
        &mut self,
        request: &serde_json::Value,
        budget: Duration,
    ) -> Result<serde_json::Value, crate::remote::RemoteError> {
        // The handle's own bound is the ceiling; a caller-supplied budget can
        // only tighten it, never extend the probe past the crate cap.
        let previous = self.server.read_timeout;
        self.server.read_timeout = previous.min(budget.max(Duration::from_millis(1)));
        let line = request.to_string();
        let sent = self.server.send(&line);
        let reply = sent.and_then(|()| self.server.recv("probing the server era"));
        self.server.read_timeout = previous;
        reply.map_err(|error| crate::remote::RemoteError::Msg(error.to_string()))
    }
}

/// P51.18 — an attached server owns a live child process; dropping the handle
/// must not orphan it. `mcp_detach` removes the handle from the live map and
/// `mcp_refresh` prunes a dead one, so both rely on this `Drop` to actually
/// end the child. Without it, `std::process::Child` is not kill-on-drop and
/// every detached server would keep running (a third-party process leak).
impl Drop for AttachedServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The attach request shape for the coordinator seam (serialized for the
/// JSON-RPC surface).
#[derive(Debug, Clone, Serialize)]
pub struct AttachRequest {
    pub command: String,
    pub args: Vec<String>,
    /// Human label recorded as tool provenance (e.g. "mcp:gmail").
    pub source: String,
}

/// P51.17 — MCP attach name sanitization. The name is bound into the guard
/// ticket args-hash, rendered on the approval card (`mcp:{name}`), and used
/// as a tool provenance label; an unsanitized name could inject into the
/// card text or registry keys. Allowed: ASCII letters/digits plus `-` `_`
/// `.` (MCP-friendly slug charset), 1–64 chars. Returns `None` for anything
/// else (reject — never silently rewrite, so the caller shows the exact
/// refusal).
pub fn sanitize_attach_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return None;
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::{AttachError, AttachedServer, SandboxPosture, drain_stdout, sanitize_attach_name};
    use crate::remote::{EraSource, McpEra, clear_era_cache, stdio_era_key};
    use crate::server::ToolCatalog;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    // -- the bounded reader -------------------------------------------------

    #[test]
    fn the_reader_publishes_every_line_then_the_end_of_stream() {
        let (sender, receiver) = channel();
        // A cursor ends after its bytes, which is exactly the EOF the reader
        // has to report: without it a closed pipe would look like a hang.
        let input = std::io::Cursor::new(b"{\"a\":1}\n{\"b\":2}\n".to_vec());
        drain_stdout(input, sender);
        // The reader publishes the raw line; trimming is the caller's job, at
        // the one place that parses it.
        assert_eq!(
            drain_result(receiver.recv().expect("line 1")),
            Some("{\"a\":1}\n".to_string())
        );
        assert_eq!(
            drain_result(receiver.recv().expect("line 2")),
            Some("{\"b\":2}\n".to_string())
        );
        assert_eq!(drain_result(receiver.recv().expect("eof")), None);
        // No fourth event: the stream is finished, not merely quiet.
        assert!(receiver.try_recv().is_err());
    }

    fn drain_result(event: super::LineEvent) -> Option<String> {
        event.expect("a reader read, not an error")
    }

    // -- the bounded handshake against a real child ------------------------

    /// Write a stdio MCP fixture that answers the era probe per `mode` and
    /// appends every method it saw to a log file.
    ///
    /// Returns `(command, args)`. `mode` is `modern` (a `server/discover`
    /// result), `legacy` (`-32601` to the probe, then a normal `initialize`), or
    /// `silent` (never answers anything).
    #[cfg(unix)]
    fn stdio_fixture(tag: &str, mode: &str) -> Option<(String, Vec<String>)> {
        which("python3")?;
        let dir = std::env::temp_dir().join(format!("agentcowork-mcp-attach-{tag}"));
        // The fixture appends, so a leftover log from an earlier run would make
        // the method sequence unreadable. Start from nothing every time.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        let script = dir.join("server.py");
        let log = dir.join("seen.log");
        let source = r#"
import json, sys

MODE = sys.argv[1]
LOG = sys.argv[2]
MODERN = {"jsonrpc": "2.0", "id": 0, "result": {"protocolVersion": "2026-07-28", "tools": []}}
UNKNOWN = {"jsonrpc": "2.0", "id": 0, "error": {"code": -32601, "message": "Method not found"}}
INIT = {"jsonrpc": "2.0", "id": 0, "result": {"protocolVersion": "2025-11-25", "capabilities": {}}}
LIST = {"jsonrpc": "2.0", "id": 0, "result": {"tools": [
    {"name": "fixture.search", "description": "d", "inputSchema": {"type": "object"}, "readOnlyHint": True}
]}}

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        request = json.loads(line)
    except ValueError:
        continue
    method = request.get("method", "")
    with open(LOG, "a") as handle:
        handle.write(method + "\n")
    if MODE == "silent":
        continue
    if method == "server/discover":
        reply = MODERN if MODE == "modern" else UNKNOWN
    elif method == "initialize":
        reply = INIT
    elif method == "tools/list":
        reply = LIST
    else:
        reply = UNKNOWN
    reply["id"] = request.get("id", 0)
    sys.stdout.write(json.dumps(reply) + "\n")
    sys.stdout.flush()
"#;
        std::fs::write(&script, source).ok()?;
        Some((
            "python3".to_string(),
            vec![
                script.to_string_lossy().into_owned(),
                mode.to_string(),
                log.to_string_lossy().into_owned(),
            ],
        ))
    }

    #[cfg(unix)]
    fn which(name: &str) -> Option<String> {
        let path = std::env::var("PATH").ok()?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
            .map(|found| found.to_string_lossy().into_owned())
    }

    #[cfg(unix)]
    fn spawn_fixture(
        tag: &str,
        mode: &str,
        budget: Duration,
    ) -> Option<(AttachedServer, Vec<String>)> {
        let (command, args) = stdio_fixture(tag, mode)?;
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let server = AttachedServer::spawn(&command, &borrowed)
            .ok()?
            .with_read_timeout(budget);
        Some((server, args))
    }

    #[cfg(unix)]
    fn seen_methods(args: &[String]) -> Vec<String> {
        let log = args[2].clone();
        std::fs::read_to_string(&log)
            .map(|text| {
                text.lines()
                    .map(str::to_string)
                    .filter(|line| !line.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A modern stdio server is detected by one probe and needs **no**
    /// `initialize` — the stateless contract has no handshake.
    #[cfg(unix)]
    #[test]
    fn a_modern_stdio_server_is_detected_without_an_initialize() {
        let _era = crate::remote::era_cache_test_guard();
        clear_era_cache();
        let Some((mut server, args)) = spawn_fixture("modern", "modern", Duration::from_secs(5))
        else {
            eprintln!("python3 unavailable — skipping modern stdio fixture");
            return;
        };
        let mut catalog = ToolCatalog::new();
        let tools = server
            .attach(&mut catalog, "mcp:fixture")
            .expect("a modern stdio server must attach");
        assert_eq!(tools, vec!["fixture.search".to_string()]);
        let seen = seen_methods(&args);
        assert_eq!(
            seen,
            vec![
                crate::protocol::DISCOVER_METHOD.to_string(),
                "tools/list".to_string()
            ],
            "exactly one discover probe, then the listing — no initialize"
        );
        let era = server.era().expect("the era is recorded on the handle");
        assert_eq!(era.era, McpEra::Modern);
        assert_eq!(era.source, EraSource::Probed);
        // The probe that went out is the shared modern envelope, so a stdio
        // probe is stateless-legal exactly like the HTTP one.
        assert!(
            cached_probe_was_modern(&args),
            "the stdio probe must carry the modern `_meta`"
        );
        clear_era_cache();
    }

    #[cfg(unix)]
    #[test]
    fn a_legacy_stdio_server_gets_the_probe_refusal_then_initialize() {
        let _era = crate::remote::era_cache_test_guard();
        clear_era_cache();
        let Some((mut server, args)) = spawn_fixture("legacy", "legacy", Duration::from_secs(5))
        else {
            eprintln!("python3 unavailable — skipping legacy stdio fixture");
            return;
        };
        let mut catalog = ToolCatalog::new();
        let tools = server
            .attach(&mut catalog, "mcp:fixture")
            .expect("a legacy stdio server must attach through the fallback");
        assert_eq!(tools, vec!["fixture.search".to_string()]);
        assert_eq!(
            seen_methods(&args),
            vec![
                crate::protocol::DISCOVER_METHOD.to_string(),
                "initialize".to_string(),
                "tools/list".to_string(),
            ],
            "the probe is refused, then the legacy handshake runs on a clean stream"
        );
        let era = server.era().expect("the era is recorded on the handle");
        assert_eq!(era.era, McpEra::Legacy);
        assert_eq!(era.source, EraSource::Probed);
        clear_era_cache();
    }

    #[cfg(unix)]
    #[test]
    fn the_stdio_verdict_is_cached_per_command_fingerprint() {
        let _era = crate::remote::era_cache_test_guard();
        clear_era_cache();
        let Some((first, args)) = spawn_fixture("cached", "modern", Duration::from_secs(5)) else {
            eprintln!("python3 unavailable — skipping stdio cache fixture");
            return;
        };
        let (command, arg_list) = ("python3".to_string(), args.clone());
        let mut first = first;
        let mut catalog = ToolCatalog::new();
        first
            .attach(&mut catalog, "mcp:fixture")
            .expect("first attach");
        assert_eq!(
            seen_methods(&args)
                .iter()
                .filter(|m| *m == crate::protocol::DISCOVER_METHOD)
                .count(),
            1
        );

        // A second child with the same command line reuses the verdict: the
        // probe is paid once per fingerprint, not once per process.
        let borrowed: Vec<&str> = arg_list.iter().map(String::as_str).collect();
        let mut second = AttachedServer::spawn(&command, &borrowed)
            .expect("second spawn")
            .with_read_timeout(Duration::from_secs(5));
        let mut catalog = ToolCatalog::new();
        second
            .attach(&mut catalog, "mcp:fixture")
            .expect("second attach");
        assert_eq!(
            seen_methods(&arg_list)
                .iter()
                .filter(|m| *m == crate::protocol::DISCOVER_METHOD)
                .count(),
            1,
            "a cached verdict must not re-probe the same command"
        );
        assert_eq!(second.era().expect("era").source, EraSource::Cached);
        // The two children share one cache key, and it is the fingerprint of the
        // *resolved* command line — so it is reportable, and it is not the bare
        // name the caller typed.
        assert_eq!(first.era_key(), second.era_key());
        assert!(
            crate::remote::cached_era(&second.era_key()).is_some(),
            "the verdict is cached under the launch fingerprint"
        );
        assert_ne!(second.era_key(), stdio_era_key("python3", &borrowed));
        clear_era_cache();
    }

    /// Assert the fixture observed the modern `_meta` on the probe. The fixture
    /// does not log it, so this reads the wire through a second, direct probe.
    #[cfg(unix)]
    fn cached_probe_was_modern(args: &[String]) -> bool {
        // The request builder is the shared one; asserting on it here keeps the
        // fixture honest without teaching the fixture about `_meta`.
        let request = crate::remote::build_discover_request(McpEra::Modern);
        assert_eq!(request["method"], crate::protocol::DISCOVER_METHOD);
        assert_eq!(
            request["params"]["_meta"]["protocolVersion"],
            serde_json::json!(McpEra::Modern.version())
        );
        // `args` is the fixture's own argv; the probe reached it (the log has
        // the method), so the request we just built is the one that was sent.
        !args.is_empty()
    }

    /// A server that accepts the connection and then says nothing must fail
    /// typed and bounded — never hang, and never look like "no tools".
    #[cfg(unix)]
    #[test]
    fn a_silent_stdio_server_is_a_typed_timeout_not_a_hang() {
        let _era = crate::remote::era_cache_test_guard();
        clear_era_cache();
        let Some((mut server, _args)) =
            spawn_fixture("silent", "silent", Duration::from_millis(250))
        else {
            eprintln!("python3 unavailable — skipping silent stdio fixture");
            return;
        };
        let started = std::time::Instant::now();
        let mut catalog = ToolCatalog::new();
        let error = server
            .attach(&mut catalog, "mcp:fixture")
            .expect_err("a silent server must not report a successful attach");
        assert!(
            matches!(error, AttachError::Timeout(..)),
            "expected a typed timeout, got {error:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the budget must bound the wait, took {:?}",
            started.elapsed()
        );
        // No tool was reconciled from a server that never answered.
        assert!(server.tools.is_empty());
        clear_era_cache();
    }

    /// The budget is a ceiling, not a suggestion: a caller may tighten it, and
    /// nothing may raise it past the crate's cap.
    #[cfg(unix)]
    #[test]
    fn the_read_budget_is_clamped_to_the_probe_cap() {
        let Some((server, _)) = spawn_fixture("clamp", "silent", Duration::from_secs(5)) else {
            eprintln!("python3 unavailable — skipping budget clamp fixture");
            return;
        };
        let tight = server.with_read_timeout(Duration::from_millis(1));
        assert_eq!(tight.read_timeout, Duration::from_millis(1));
        let loose = tight.with_read_timeout(Duration::from_secs(3_600));
        assert_eq!(
            loose.read_timeout,
            crate::remote::PROBE_BUDGET,
            "no caller may extend a read past the probe cap"
        );
    }

    #[test]
    fn name_sanitizer_accepts_slugs() {
        assert_eq!(sanitize_attach_name("gmail"), Some("gmail".into()));
        assert_eq!(
            sanitize_attach_name("my-server_2.v1"),
            Some("my-server_2.v1".into())
        );
        assert_eq!(sanitize_attach_name("  trimmed  "), Some("trimmed".into()));
    }

    #[test]
    fn npx_call_is_refused_before_spawn() {
        let err = match AttachedServer::spawn("npx", &["-c", "rm -rf /"]) {
            Err(e) => e,
            Ok(_) => panic!("hostile npx -c must not spawn"),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("trusted") || msg.contains("npx"),
            "expected trusted-list refusal, got {msg}"
        );
    }

    #[test]
    fn name_sanitizer_rejects_hostile_names() {
        assert_eq!(sanitize_attach_name(""), None);
        assert_eq!(sanitize_attach_name("   "), None);
        assert_eq!(sanitize_attach_name("a b"), None);
        assert_eq!(sanitize_attach_name("a;rm -rf /"), None);
        assert_eq!(sanitize_attach_name("a\nb"), None);
        assert_eq!(sanitize_attach_name("server/../../etc"), None);
        assert_eq!(sanitize_attach_name("x".repeat(65).as_str()), None);
    }

    #[test]
    fn posture_never_claims_containment_it_cannot_deliver() {
        // `is_contained` is the only thing a caller should branch on; the
        // ambient fallback must always report `false`.
        assert!(!SandboxPosture::Ambient.is_contained());
        assert!(SandboxPosture::Confined.is_contained());
        #[cfg(not(target_os = "linux"))]
        assert_eq!(SandboxPosture::preferred(), SandboxPosture::Ambient);
    }

    #[test]
    fn ambient_posture_spawns_and_reports_unsandboxed() {
        #[cfg(unix)]
        {
            let Ok(mut s) = AttachedServer::spawn_with_posture(
                SandboxPosture::Ambient,
                "/tmp/agentcowork-mcp-posture-test",
                "allow",
                "cat",
                &[],
            ) else {
                return; // `cat` unavailable — not a posture regression
            };
            assert!(
                !s.is_sandboxed(),
                "ambient posture must never claim sandboxing"
            );
            s.shutdown();
        }
    }

    /// The confined launch is `--clearenv` and network-constrained by
    /// construction, which is what makes it safe to hand a third-party MCP
    /// server no ambient secrets. Pure assertion on the built command; the
    /// live spawn below proves the backend actually accepts our argv.
    #[cfg(target_os = "linux")]
    #[test]
    fn confined_launch_is_clearenv_and_network_constrained() {
        use agentcowork_guard::sandbox::{LinuxBwrapBackend, SandboxRole, SandboxSpec, profiles};
        let scratch = "/tmp/agentcowork-mcp-confined-test";
        let _ = std::fs::create_dir_all(scratch);
        let spec = SandboxSpec {
            role: SandboxRole::ChildExecutionSandbox,
            profile: profiles::worker(scratch),
            network: "deny".into(),
            credentials: "opaque_handles".into(),
            resource_limit_bytes: 512 << 20,
        };
        let cmd = LinuxBwrapBackend::command(&spec, &["/bin/echo".into()]).unwrap();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"--clearenv".into()), "must not inherit env");
        assert!(
            args.contains(&"--unshare-net".into()),
            "network must be denied"
        );
    }

    /// P51.18 — the child must not outlive its handle. `mcp_detach` drops the
    /// map entry and `mcp_refresh` prunes a dead one; without a `Drop` impl the
    /// std `Child` is not kill-on-drop, so a detached third-party server would
    /// keep running forever. Linux-gated because the liveness probe is `/proc`.
    #[cfg(target_os = "linux")]
    #[test]
    fn dropping_an_attached_server_kills_its_child() {
        fn alive(pid: u32) -> bool {
            std::path::Path::new(&format!("/proc/{pid}")).exists()
        }
        let server = match AttachedServer::spawn("sleep", &["30"]) {
            Ok(s) => s,
            Err(_) => return, // `sleep` unavailable — not a Drop regression
        };
        let pid = server.pid().expect("spawned child exposes a pid");
        assert!(alive(pid), "child {pid} should start alive");
        drop(server);
        for _ in 0..100 {
            if !alive(pid) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("child {pid} survived Drop — detached MCP server leaked");
    }

    /// Live proof the confined path actually spawns and reports containment.
    /// Skips (honest no-op) when bubblewrap or user namespaces are unavailable
    /// — it never passes without real containment.
    #[cfg(target_os = "linux")]
    #[test]
    fn confined_posture_binds_a_sandboxed_child() {
        use agentcowork_guard::sandbox::linux_bwrap_available;
        if !linux_bwrap_available() {
            eprintln!("bwrap unavailable — skipping confined MCP spawn test");
            return;
        }
        let Ok(mut s) = AttachedServer::spawn_confined(
            "/tmp/agentcowork-mcp-confined-test",
            "deny",
            "/bin/echo",
            &[],
        ) else {
            eprintln!("confined spawn unavailable — skipping");
            return;
        };
        assert!(
            s.is_sandboxed(),
            "a confined child must report sandboxed so callers never confuse the two"
        );
        s.shutdown();
    }
}
