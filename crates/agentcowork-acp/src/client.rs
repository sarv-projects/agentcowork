//! ACP client session (F12/J17 — doc 45 §1). Our app plays the **Client**
//! role: it spawns an agent subprocess and drives
//! `initialize` → `session/new` → `session/prompt`, while answering the
//! agent's inbound `session/request_permission` (the Guard-2 seam) and
//! collecting `session/update` notifications for the audit trail.
//!
//! The transport is a trait so tests drive the handshake with a scripted mock;
//! the real [`ProcessTransport`] spawns the agent CLI over stdio (newline-
//! delimited JSON-RPC, stderr = free-form logs).

use crate::frame::{MAX_ACP_FRAME_BYTES, decode_messages, finish_decode, try_encode_message};
use crate::messages::*;
use agentcowork_types::env_compat;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use agentcowork_guard::sandbox::LinuxBwrapBackend;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AcpError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("agent closed the stream (EOF)")]
    Eof,
    #[error("malformed agent message: {0}")]
    Malformed(String),
    #[error("agent returned an error response: {0}")]
    ServerError(String),
    #[error("session not initialized / no active session")]
    NotReady,
    #[error("agent did not negotiate the ACP v1 loadSession capability")]
    SessionLoadUnsupported,
    #[error("session/load returned an invalid provider session id")]
    InvalidSessionId,
    #[error("agent did not negotiate the ACP v1 MCP HTTP capability")]
    McpHttpUnsupported,
    #[error("agent did not negotiate the ACP v1 MCP SSE capability")]
    McpSseUnsupported,
    #[error("inbound ACP frame belongs to a different provider session")]
    CrossSessionFrame,
    /// A `session/request_permission` request could not be answered with an
    /// option the agent actually offered (FIX-03). The bridge fails closed: the
    /// agent receives a typed error and the turn ends — no synthesized option
    /// id, no implicit allow.
    #[error("permission request could not be answered fail-closed: {0}")]
    PermissionUnanswerable(String),
    #[error("ACP pending frame queue reached its bounded capacity")]
    PendingQueueFull,
    #[error("ACP frame exceeds the {MAX_ACP_FRAME_BYTES}-byte limit")]
    FrameTooLarge,
    #[error("ACP transport is quarantined after a protocol failure: {0}")]
    Quarantined(&'static str),
    #[error(
        "session/load returned a mismatched provider session id (requested {requested}, got {returned})"
    )]
    SessionIdMismatch { requested: String, returned: String },
    #[error("invalid ACP working directory: {0}")]
    InvalidCwd(String),
    #[error("invalid ACP MCP server descriptor: {0}")]
    InvalidMcpServer(String),
    #[error("protocol version mismatch: agent speaks {0}")]
    ProtocolMismatch(u64),
    /// The agent requires authentication before it will create sessions
    /// (`auth_required` error, code -32000). The client must call
    /// [`AcpSession::authenticate`] with one of the advertised methods.
    #[error("agent requires authentication (auth_required)")]
    AuthRequired,
    /// The provider explicitly acknowledged cancellation for the active turn.
    ///
    /// A local cancellation request is not evidence of provider completion and
    /// must not be represented by this variant.  Callers that need to report a
    /// local request which has no provider acknowledgement should use
    /// [`AcpError::CancellationOutcomeUnknown`] (or the underlying transport
    /// error) instead.
    #[error("ACP provider acknowledged turn cancellation")]
    Cancelled,
    /// A local cancellation was requested, but the provider did not return a
    /// terminal acknowledgement before the transport ended or failed.
    ///
    /// This is intentionally distinct from [`AcpError::Cancelled`]: the
    /// durable outcome is unknown, so callers must fail closed rather than
    /// settling a run as cancelled.
    #[error("ACP cancellation outcome is unknown (provider acknowledgement not observed)")]
    CancellationOutcomeUnknown,
}

/// ACP protocol-specific error codes (official schema).
const ERROR_AUTH_REQUIRED: i64 = -32000;
/// The client could not answer a permission request with an offered option
/// (FIX-03). JSON-RPC reserves -32000..-32099 for implementation-defined
/// server errors; the agent must treat this as a refusal, never as consent.
const ERROR_PERMISSION_UNANSWERABLE: i64 = -32001;
/// Bound the interleaving queue so a peer cannot make a blocked client grow
/// memory without limit. Overflow is surfaced as a cancellation/shutdown gap,
/// never silently discarded.
const MAX_PENDING_FRAMES: usize = 256;

/// The provider gets a short, bounded window to acknowledge a local cancel.
/// If it does not answer in that window, the process transport tears down the
/// child and the turn is reported as an unknown outcome (never as a confirmed
/// cancellation).
const CANCELLATION_ACK_GRACE: Duration = Duration::from_millis(250);
/// Polling keeps cancellation/shutdown responsive even when the child never
/// writes another stdout frame.
const TRANSPORT_READ_POLL: Duration = Duration::from_millis(25);

/// One cancellation-ack deadline, keyed by the exact turn generation that
/// requested it. A new generation always receives a new full grace window.
#[derive(Debug, Default)]
struct CancellationDeadline {
    active: Option<(u64, Instant)>,
}

impl CancellationDeadline {
    fn observe(&mut self, generation: Option<u64>, now: Instant) -> bool {
        let Some(generation) = generation else {
            self.active = None;
            return false;
        };
        let deadline = match self.active {
            Some((active_generation, deadline)) if active_generation == generation => deadline,
            _ => {
                let deadline = now.checked_add(CANCELLATION_ACK_GRACE).unwrap_or(now);
                self.active = Some((generation, deadline));
                deadline
            }
        };
        now >= deadline
    }
}

/// Transport callback used to write a framed `session/cancel` notification.
pub type AcpCancelSender = Arc<dyn Fn(&str) -> io::Result<()> + Send + Sync>;
/// Transport callback used to request a bounded shutdown without borrowing
/// the session/transport mutex.  A callback should only signal the concrete
/// transport; the owning thread still performs final kill/reap when it can.
pub type AcpShutdownSender = Arc<dyn Fn() -> io::Result<()> + Send + Sync>;

/// A process-independent cancellation hook for an ACP session.
///
/// The hook is deliberately separate from [`AcpSession`]: the shell can keep
/// the session in a per-handle mutex while a prompt is blocked in provider I/O,
/// then request cancellation without taking that mutex or the global handle-map
/// lock. Process transports install a writer for the `session/cancel`
/// notification; scripted transports may omit the writer and still observe the
/// local cancellation flag.
///
/// `generation` and `active_generation` are a small per-turn fence. A request
/// arriving after a provider response has been accepted is not allowed to
/// rewrite that response, and a request from an older turn cannot silently
/// become the cancellation state of a later turn.
#[derive(Clone)]
pub struct AcpCancelHandle {
    requested: Arc<std::sync::atomic::AtomicBool>,
    generation: Arc<std::sync::atomic::AtomicU64>,
    active_generation: Arc<std::sync::atomic::AtomicU64>,
    /// Protects turn-state transitions.
    gate: Arc<Mutex<()>>,
    /// Serializes the actual cancellation write with opening a later turn. The
    /// write guard is separate from `gate` so an old turn can still complete
    /// while a cancellation pipe write is in flight.
    write_gate: Arc<Mutex<()>>,
    /// Set when the out-of-band cancellation writer fails. The owning session
    /// checks this before any later request, so a write fault cannot be hidden
    /// by a successful retry on the same poisoned handle.
    faulted: Arc<std::sync::atomic::AtomicBool>,
    sender: Option<AcpCancelSender>,
    shutdown_sender: Option<AcpShutdownSender>,
}

impl AcpCancelHandle {
    /// Build a cancellation hook for a custom transport. `sender` receives the
    /// raw JSON-RPC notification and should write it using the transport's
    /// framing rules.
    pub fn new(
        requested: Arc<std::sync::atomic::AtomicBool>,
        sender: Option<AcpCancelSender>,
    ) -> Self {
        Self {
            requested,
            generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            active_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            gate: Arc::new(Mutex::new(())),
            write_gate: Arc::new(Mutex::new(())),
            faulted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sender,
            shutdown_sender: None,
        }
    }

    fn with_shutdown(
        requested: Arc<std::sync::atomic::AtomicBool>,
        sender: Option<AcpCancelSender>,
        shutdown_sender: Option<AcpShutdownSender>,
    ) -> Self {
        Self {
            requested,
            generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            active_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            gate: Arc::new(Mutex::new(())),
            write_gate: Arc::new(Mutex::new(())),
            faulted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sender,
            shutdown_sender,
        }
    }

    fn next_generation(&self) -> u64 {
        loop {
            let next = self
                .generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                .wrapping_add(1);
            if next != 0 {
                return next;
            }
        }
    }

    fn mark_requested(&self) -> io::Result<()> {
        let _gate = self
            .gate
            .lock()
            .map_err(|_| io::Error::other("ACP cancellation gate poisoned"))?;
        self.requested
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    /// Request cancellation of `session_id` and return the transport writer's
    /// result. A missing writer is still a successful local cancellation.
    ///
    /// If the provider has already completed the current turn, this records
    /// only local state and emits no notification. The prompt fence
    /// deliberately does not translate a local request into
    /// [`AcpError::Cancelled`]; only a provider response can do so.
    pub fn request(&self, session_id: &str) -> io::Result<()> {
        // Capture the generation targeted by this local request before any
        // potentially blocking write-gate acquisition. If the old turn finishes
        // while the request is delayed, the generation recheck below refuses to
        // apply it to a later turn.
        let target_generation = {
            let _gate = self
                .gate
                .lock()
                .map_err(|_| io::Error::other("ACP cancellation gate poisoned"))?;
            self.requested
                .store(true, std::sync::atomic::Ordering::Release);
            self.active_generation
                .load(std::sync::atomic::Ordering::Acquire)
        };
        // A request with no active turn is recorded as local state only. It
        // never targets a future turn.
        if target_generation == 0 {
            return Ok(());
        }
        // Serialize the write with opening a later turn. The state gate stays
        // free so the old prompt can finish, while the write guard prevents a
        // later turn from becoming active until this targeted write completes.
        let _write_gate = self
            .write_gate
            .lock()
            .map_err(|_| io::Error::other("ACP cancellation write gate poisoned"))?;
        if self
            .active_generation
            .load(std::sync::atomic::Ordering::Acquire)
            != target_generation
        {
            return Ok(());
        }
        let Some(sender) = &self.sender else {
            return Ok(());
        };
        let message = json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": { "sessionId": session_id }
        });
        let result = sender(&message.to_string());
        if result.is_err() {
            self.faulted
                .store(true, std::sync::atomic::Ordering::Release);
        }
        result
    }

    /// Signal a bounded transport shutdown without borrowing the session.
    ///
    /// Custom transports that cannot provide an out-of-band shutdown callback
    /// fail closed with `Unsupported`; callers must not treat a local request
    /// as proof that the provider stopped.
    pub fn request_shutdown(&self) -> io::Result<()> {
        let Some(sender) = &self.shutdown_sender else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "ACP transport has no out-of-band shutdown hook",
            ));
        };
        sender()
    }

    fn has_active_turn(&self) -> bool {
        self.active_generation
            .load(std::sync::atomic::Ordering::Acquire)
            != 0
    }

    /// Return the active turn generation when cancellation is currently
    /// requested. Process transports key their acknowledgement deadline by
    /// this value so an expired deadline from an older turn cannot suppress a
    /// fresh turn's grace window.
    fn cancellation_generation(&self) -> Option<u64> {
        let generation = self
            .active_generation
            .load(std::sync::atomic::Ordering::Acquire);
        (generation != 0 && self.requested.load(std::sync::atomic::Ordering::Acquire))
            .then_some(generation)
    }

    /// Whether a cancellation has been requested for the current turn.
    pub fn is_requested(&self) -> bool {
        self.requested.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Clear the flag before starting a new turn on the same provider session.
    /// The write gate keeps this transition ordered with any in-flight
    /// cancellation notification.
    pub fn reset(&self) {
        if let Ok(_write_gate) = self.write_gate.lock() {
            if let Ok(_gate) = self.gate.lock() {
                self.active_generation
                    .store(0, std::sync::atomic::Ordering::Release);
                self.requested
                    .store(false, std::sync::atomic::Ordering::Release);
            }
        }
    }

    /// Open a fenced turn. The returned guard owns the active generation until
    /// it is completed or dropped, so a late request cannot mutate the turn's
    /// terminal decision.
    fn begin_turn(&self) -> TurnFence {
        let _write_gate = self
            .write_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let generation = {
            let _gate = self
                .gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let generation = self.next_generation();
            self.active_generation
                .store(generation, std::sync::atomic::Ordering::Release);
            self.requested
                .store(false, std::sync::atomic::Ordering::Release);
            generation
        };
        TurnFence {
            handle: self.clone(),
            generation,
            complete: false,
        }
    }

    fn finish_turn(&self, generation: u64) {
        let _gate = self
            .gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self
            .active_generation
            .compare_exchange(
                generation,
                0,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            // Do not let a completed turn leave a cancellation-looking flag for
            // the next turn. A request that linearizes after this fence is
            // still observable, but it can no longer rewrite this turn.
            self.requested
                .store(false, std::sync::atomic::Ordering::Release);
        }
    }

    fn requested_for(&self, generation: u64) -> bool {
        self.active_generation
            .load(std::sync::atomic::Ordering::Acquire)
            == generation
            && self.requested.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// RAII fence for one prompt turn. Completing the fence before returning the
/// provider result establishes the linearization point against a concurrent
/// local cancellation request.
struct TurnFence {
    handle: AcpCancelHandle,
    generation: u64,
    complete: bool,
}

impl TurnFence {
    fn requested(&self) -> bool {
        self.handle.requested_for(self.generation)
    }

    fn complete(&mut self) {
        if !self.complete {
            self.handle.finish_turn(self.generation);
            self.complete = true;
        }
    }
}

impl Drop for TurnFence {
    fn drop(&mut self) {
        self.complete();
    }
}

/// A bidirectional newline-delimited JSON-RPC transport to an agent.
pub trait AcpTransport {
    fn send(&mut self, json: &str) -> io::Result<()>;
    fn recv(&mut self) -> io::Result<Option<String>>;
    fn is_alive(&mut self) -> bool;
    fn shutdown(&mut self);
    /// Return a cancellation/shutdown hook that can be used without borrowing
    /// the transport. Transports without a concurrent writer may omit it;
    /// [`AcpCancelHandle::request_shutdown`] then fails closed as
    /// `Unsupported` rather than claiming the provider stopped.
    fn cancellation_handle(&self) -> Option<AcpCancelHandle> {
        None
    }
}

impl<T: AcpTransport + ?Sized> AcpTransport for &mut T {
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
    fn cancellation_handle(&self) -> Option<AcpCancelHandle> {
        (**self).cancellation_handle()
    }
}

/// A child-environment policy failure is explicit rather than a silently
/// credential-free downgrade. The external agent remains responsible for its
/// own authentication; a host that tries to supply a secret must use an
/// ADR-approved delegated-bearer mechanism instead.
#[derive(Debug, Error)]
pub enum ProcessTransportError {
    #[error("io error while preparing or spawning the ACP child: {0}")]
    Io(#[from] io::Error),
    #[error("environment variable {name:?} is not in the reviewed ACP child allowlist: {reason}")]
    DisallowedEnvVar { name: String, reason: &'static str },
    #[error("invalid ACP child environment variable name {name:?}: {reason}")]
    InvalidEnvName { name: String, reason: &'static str },
    #[error("invalid value for ACP child environment variable {name:?}: {reason}")]
    InvalidEnvValue { name: String, reason: &'static str },
}

/// A reviewed, non-secret variable that may be copied into an ACP child.
///
/// The enum is intentionally closed. Parent-runtime names and per-agent
/// backend names are separate sets: metadata cannot replace `PATH`, inject a
/// loader/interpreter variable, or smuggle a proxy/URI/host setting merely by
/// choosing a new spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildEnvName {
    Path,
    Home,
    TmpDir,
    Temp,
    Tmp,
    Lang,
    Language,
    LcAll,
    LcCtype,
    Term,
    Tz,
    User,
    Logname,
    XdgRuntimeDir,
    XdgDataHome,
    XdgCacheHome,
    XdgConfigHome,
    SystemRoot,
    WinDir,
    PathExt,
    UserProfile,
    HomeDrive,
    HomePath,
    AppData,
    LocalAppData,
    ProgramData,
    ProgramFiles,
    ProgramFilesX86,
    ProcessorArchitecture,
    ProcessorArchitectureWow6432,
    NumberOfProcessors,
    Os,
    /// Reviewed fixed-agent backend settings.
    AnthropicBaseUrl,
    AnthropicModel,
    GroqBaseUrl,
}

impl ChildEnvName {
    /// Return the canonical spelling emitted to the child.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Path => "PATH",
            Self::Home => "HOME",
            Self::TmpDir => "TMPDIR",
            Self::Temp => "TEMP",
            Self::Tmp => "TMP",
            Self::Lang => "LANG",
            Self::Language => "LANGUAGE",
            Self::LcAll => "LC_ALL",
            Self::LcCtype => "LC_CTYPE",
            Self::Term => "TERM",
            Self::Tz => "TZ",
            Self::User => "USER",
            Self::Logname => "LOGNAME",
            Self::XdgRuntimeDir => "XDG_RUNTIME_DIR",
            Self::XdgDataHome => "XDG_DATA_HOME",
            Self::XdgCacheHome => "XDG_CACHE_HOME",
            Self::XdgConfigHome => "XDG_CONFIG_HOME",
            Self::SystemRoot => "SYSTEMROOT",
            Self::WinDir => "WINDIR",
            Self::PathExt => "PATHEXT",
            Self::UserProfile => "USERPROFILE",
            Self::HomeDrive => "HOMEDRIVE",
            Self::HomePath => "HOMEPATH",
            Self::AppData => "APPDATA",
            Self::LocalAppData => "LOCALAPPDATA",
            Self::ProgramData => "PROGRAMDATA",
            Self::ProgramFiles => "PROGRAMFILES",
            Self::ProgramFilesX86 => "PROGRAMFILES(X86)",
            Self::ProcessorArchitecture => "PROCESSOR_ARCHITECTURE",
            Self::ProcessorArchitectureWow6432 => "PROCESSOR_ARCHITEW6432",
            Self::NumberOfProcessors => "NUMBER_OF_PROCESSORS",
            Self::Os => "OS",
            Self::AnthropicBaseUrl => "ANTHROPIC_BASE_URL",
            Self::AnthropicModel => "ANTHROPIC_MODEL",
            Self::GroqBaseUrl => "GROQ_BASE_URL",
        }
    }

    /// Parse a name supplied by a caller, registry, or installer. This is an
    /// exact reviewed set; unknown names fail closed rather than being
    /// pattern-matched after the fact.
    pub fn from_explicit_name(name: &str) -> Result<Self, ProcessTransportError> {
        if name.is_empty()
            || name.contains('=')
            || name.contains('\0')
            || name.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
        {
            return Err(ProcessTransportError::InvalidEnvName {
                name: name.to_string(),
                reason: "names must be non-empty printable identifiers without '=' or NUL",
            });
        }
        let canonical = name.to_ascii_uppercase();
        let value = match canonical.as_str() {
            "ANTHROPIC_BASE_URL" => Self::AnthropicBaseUrl,
            "ANTHROPIC_MODEL" => Self::AnthropicModel,
            "GROQ_BASE_URL" => Self::GroqBaseUrl,
            _ => {
                return Err(ProcessTransportError::DisallowedEnvVar {
                    name: name.to_string(),
                    reason: explicit_denial_reason(name),
                });
            }
        };
        Ok(value)
    }

    fn from_parent_name(name: &str) -> Option<Self> {
        let value = match name {
            "PATH" => Self::Path,
            "HOME" => Self::Home,
            "TMPDIR" => Self::TmpDir,
            "TEMP" => Self::Temp,
            "TMP" => Self::Tmp,
            "LANG" => Self::Lang,
            "LANGUAGE" => Self::Language,
            "LC_ALL" => Self::LcAll,
            "LC_CTYPE" => Self::LcCtype,
            "TERM" => Self::Term,
            "TZ" => Self::Tz,
            "USER" => Self::User,
            "LOGNAME" => Self::Logname,
            "XDG_RUNTIME_DIR" => Self::XdgRuntimeDir,
            "XDG_DATA_HOME" => Self::XdgDataHome,
            "XDG_CACHE_HOME" => Self::XdgCacheHome,
            "XDG_CONFIG_HOME" => Self::XdgConfigHome,
            "SYSTEMROOT" => Self::SystemRoot,
            "WINDIR" => Self::WinDir,
            "PATHEXT" => Self::PathExt,
            "USERPROFILE" => Self::UserProfile,
            "HOMEDRIVE" => Self::HomeDrive,
            "HOMEPATH" => Self::HomePath,
            "APPDATA" => Self::AppData,
            "LOCALAPPDATA" => Self::LocalAppData,
            "PROGRAMDATA" => Self::ProgramData,
            "PROGRAMFILES" => Self::ProgramFiles,
            "PROGRAMFILES(X86)" => Self::ProgramFilesX86,
            "PROCESSOR_ARCHITECTURE" => Self::ProcessorArchitecture,
            "PROCESSOR_ARCHITEW6432" => Self::ProcessorArchitectureWow6432,
            "NUMBER_OF_PROCESSORS" => Self::NumberOfProcessors,
            "OS" => Self::Os,
            _ => return None,
        };
        Some(value)
    }

    fn is_url(self) -> bool {
        matches!(self, Self::AnthropicBaseUrl | Self::GroqBaseUrl)
    }
}

/// Explain a rejected metadata name without ever including its value. The
/// explicit allowlist remains the authority; these categories make common
/// loader/proxy/URI attempts actionable in a status card.
fn explicit_denial_reason(name: &str) -> &'static str {
    let upper = name.to_ascii_uppercase();
    if upper.starts_with("LD_")
        || upper.starts_with("DYLD_")
        || matches!(
            upper.as_str(),
            "BASH_ENV"
                | "ENV"
                | "NODE_OPTIONS"
                | "NODE_PATH"
                | "PYTHONPATH"
                | "PYTHONHOME"
                | "PERL5LIB"
                | "RUBYOPT"
                | "JAVA_TOOL_OPTIONS"
                | "SSL_CERT_FILE"
                | "SSL_CERT_DIR"
        )
    {
        "loader/interpreter variables are not allowed"
    } else if upper == "PROXY"
        || upper.ends_with("_PROXY")
        || upper == "DATABASE_URL"
        || upper == "KUBECONFIG"
    {
        "proxy or service-location variables are not allowed"
    } else if env_compat::is_host_control_name(&upper) {
        // Both the current `AGENTCOWORK_*` and the retired `EVERYAIOS_*`
        // spellings: a renamed credential must not pass a filter that only
        // knew the old name (DEC-053).
        "host-control variables are not allowed"
    } else {
        "only reviewed per-agent non-secret names are allowed"
    }
}

/// Normalize environment names for collision-safe parent selection. Windows
/// treats environment names case-insensitively (notably `Path` vs `PATH`);
/// Unix keeps its native spelling.
fn canonical_env_name(name: &OsStr) -> Option<String> {
    let name = name.to_str()?;
    Some(if cfg!(windows) {
        name.to_ascii_uppercase()
    } else {
        name.to_owned()
    })
}

/// Build the child environment from the closed parent-runtime allowlist.
/// `BTreeMap` makes selection deterministic and removes duplicate spellings on
/// Windows before `Command` can observe them.
fn hermetic_parent_environment<I>(vars: I) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let mut selected: BTreeMap<String, (OsString, OsString)> = BTreeMap::new();
    for (name, value) in vars {
        let Some(canonical) = canonical_env_name(&name) else {
            continue;
        };
        let Some(reviewed) = ChildEnvName::from_parent_name(&canonical) else {
            continue;
        };
        selected
            .entry(reviewed.as_str().to_string())
            .or_insert((OsString::from(reviewed.as_str()), value));
    }
    selected.into_values().collect()
}

fn hermetic_environment_from_process() -> Vec<(OsString, OsString)> {
    hermetic_parent_environment(std::env::vars_os())
}

fn valid_explicit_env_value(
    name: ChildEnvName,
    value: &str,
) -> Result<String, ProcessTransportError> {
    if value.is_empty()
        || value.len() > 8192
        || value.chars().any(|character| character.is_control())
    {
        return Err(ProcessTransportError::InvalidEnvValue {
            name: name.as_str().to_string(),
            reason: "values must be non-empty, bounded, and free of control characters",
        });
    }
    if name.is_url() {
        let canonical = crate::agent_backend::validate_base_url(value).map_err(|_| {
            ProcessTransportError::InvalidEnvValue {
                name: name.as_str().to_string(),
                reason: "URL values must pass the Guard/netfloor policy",
            }
        })?;
        return Ok(canonical);
    }
    if name == ChildEnvName::AnthropicModel {
        crate::agent_backend::validate_model_id(value).map_err(|_| {
            ProcessTransportError::InvalidEnvValue {
                name: name.as_str().to_string(),
                reason: "model values must be bounded identifiers, not URIs or secrets",
            }
        })?;
    }
    Ok(value.to_string())
}

/// Apply caller-authored metadata on top of the hermetic parent subset. Only
/// the exact reviewed per-agent names are accepted; unknown, loader,
/// interpreter, proxy, URI, and host-control names fail before process
/// creation.
fn prepare_child_environment(
    env: &[(&str, &str)],
) -> Result<Vec<(OsString, OsString)>, ProcessTransportError> {
    let mut prepared: BTreeMap<String, (OsString, OsString)> = hermetic_environment_from_process()
        .into_iter()
        .filter_map(|(name, value)| {
            canonical_env_name(&name).map(|canonical| (canonical, (name, value)))
        })
        .collect();

    for (name, value) in env {
        let reviewed = ChildEnvName::from_explicit_name(name)?;
        let canonical_value = valid_explicit_env_value(reviewed, value)?;
        prepared.insert(
            reviewed.as_str().to_string(),
            (
                OsString::from(reviewed.as_str()),
                OsString::from(canonical_value),
            ),
        );
    }

    Ok(prepared.into_values().collect())
}

/// Start a bounded reader for a child stdout pipe.  The reader owns the
/// blocking `read`; the transport thread only waits on the channel with a
/// short timeout.  That separation is what lets a cancellation request make
/// progress even when an agent never writes another frame.
fn spawn_process_reader(
    stdout: ChildStdout,
) -> (Receiver<io::Result<Option<String>>>, JoinHandle<()>) {
    // Keep the reader's hand-off queue bounded so a noisy agent cannot move
    // the old unbounded pipe-read behavior into an in-memory queue.
    let (sender, receiver) = mpsc::sync_channel(MAX_PENDING_FRAMES);
    let handle = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::new();
        loop {
            match decode_messages(&mut buf) {
                Ok(messages) => {
                    for message in messages {
                        if sender.send(Ok(Some(message))).is_err() {
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error));
                    return;
                }
            }
            let mut chunk = [0u8; 8192];
            match reader.read(&mut chunk) {
                Ok(0) => {
                    if let Err(error) = finish_decode(&buf) {
                        let _ = sender.send(Err(error));
                    } else {
                        let _ = sender.send(Ok(None));
                    }
                    return;
                }
                Ok(read) => buf.extend_from_slice(&chunk[..read]),
                Err(error) => {
                    let _ = sender.send(Err(error));
                    return;
                }
            }
        }
    });
    (receiver, handle)
}

/// stdio transport over a spawned agent process (the ACP wire transport).
pub struct ProcessTransport {
    child: Option<Child>,
    #[cfg(target_os = "linux")]
    monitor: Option<agentcowork_guard::sandbox::SandboxProcess>,
    stdin: Arc<Mutex<ChildStdin>>,
    cancel_handle: AcpCancelHandle,
    reader_rx: Option<Receiver<io::Result<Option<String>>>>,
    reader_thread: Option<JoinHandle<()>>,
    shutdown_requested: Arc<std::sync::atomic::AtomicBool>,
    transport_quarantined: Option<&'static str>,
    cancel_deadline: CancellationDeadline,
    /// Decoded messages not yet returned to the caller. `decode_messages` can
    /// yield several complete frames from one read; they must be queued, not
    /// dropped, or a fast agent's result can be lost and the caller will block
    /// forever waiting for it.
    pending: VecDeque<String>,
}

impl ProcessTransport {
    /// Spawn `command` with `args` and a hermetic child environment.
    ///
    /// The parent environment is never inherited implicitly: only the
    /// [`ChildEnvName`] parent-runtime allowlist is copied, then caller-
    /// supplied metadata is accepted only for the closed per-agent
    /// non-secret allowlist. All names and values are checked before process
    /// creation.
    pub fn spawn(
        command: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<Self, ProcessTransportError> {
        let child_env = prepare_child_environment(env)?;
        let mut cmd = Command::new(command);
        cmd.env_clear()
            .envs(child_env)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()); // ACP: stderr is free-form logs, not protocol
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "no stdin on spawned agent")
        })?;
        let stdin = Arc::new(Mutex::new(stdin));
        let cancel_stdin = Arc::clone(&stdin);
        let cancel_sender = Arc::new(move |message: &str| {
            let mut stdin = cancel_stdin
                .lock()
                .map_err(|_| io::Error::other("ACP stdin lock poisoned"))?;
            stdin.write_all(try_encode_message(message)?.as_bytes())?;
            stdin.flush()
        });
        let cancel_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown_requested);
        let shutdown_sender: AcpShutdownSender = Arc::new(move || {
            shutdown_flag.store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        });
        let stdout = child.stdout.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "no stdout on spawned agent")
        })?;
        let (reader_rx, reader_thread) = spawn_process_reader(stdout);
        Ok(Self {
            child: Some(child),
            #[cfg(target_os = "linux")]
            monitor: None,
            stdin,
            cancel_handle: AcpCancelHandle::with_shutdown(
                Arc::clone(&cancel_requested),
                Some(cancel_sender),
                Some(shutdown_sender),
            ),
            reader_rx: Some(reader_rx),
            reader_thread: Some(reader_thread),
            shutdown_requested,
            transport_quarantined: None,
            cancel_deadline: CancellationDeadline::default(),
            pending: VecDeque::new(),
        })
    }

    /// Build a transport from stdio owned by a concrete sandbox launcher
    /// (Linux bubblewrap). The monitor is retained so shutdown/reaping stays
    /// controlled by the sandbox handle rather than an uncontrolled child
    /// constructor: `is_alive`/`shutdown` observe the sandboxed process, and
    /// the child is never reaped outside the sandbox backend.
    #[cfg(target_os = "linux")]
    pub fn spawn_sandboxed(
        spec: &agentcowork_guard::sandbox::SandboxSpec,
        command: &[String],
    ) -> io::Result<Self> {
        // `--clearenv` remains the outer containment boundary. Add only the
        // same explicit runtime subset used by the ordinary ProcessTransport
        // path so an external executable/interpreter is not launched into an
        // unusable environment.
        let env = hermetic_environment_from_process()
            .into_iter()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect::<Vec<_>>();
        let sandboxed = LinuxBwrapBackend
            .spawn_stdio_with_env(spec, command, &env)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let stdin = Arc::new(Mutex::new(sandboxed.stdin));
        let cancel_stdin = Arc::clone(&stdin);
        let cancel_sender = Arc::new(move |message: &str| {
            let mut stdin = cancel_stdin
                .lock()
                .map_err(|_| io::Error::other("ACP stdin lock poisoned"))?;
            stdin.write_all(try_encode_message(message)?.as_bytes())?;
            stdin.flush()
        });
        let cancel_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown_requested);
        let shutdown_sender: AcpShutdownSender = Arc::new(move || {
            shutdown_flag.store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        });
        let (reader_rx, reader_thread) = spawn_process_reader(sandboxed.stdout);
        Ok(Self {
            child: None,
            monitor: Some(sandboxed.monitor),
            stdin,
            cancel_handle: AcpCancelHandle::with_shutdown(
                Arc::clone(&cancel_requested),
                Some(cancel_sender),
                Some(shutdown_sender),
            ),
            reader_rx: Some(reader_rx),
            reader_thread: Some(reader_thread),
            shutdown_requested,
            transport_quarantined: None,
            cancel_deadline: CancellationDeadline::default(),
            pending: VecDeque::new(),
        })
    }

    fn cancellation_expired(&mut self) -> bool {
        self.cancel_deadline
            .observe(self.cancel_handle.cancellation_generation(), Instant::now())
    }

    fn terminate_process(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(monitor) = self.monitor.as_mut() {
            let _ = monitor.kill();
            return;
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl AcpTransport for ProcessTransport {
    fn send(&mut self, json: &str) -> io::Result<()> {
        if let Some(reason) = self.transport_quarantined {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("ACP transport is quarantined: {reason}"),
            ));
        }
        let result = (|| {
            let mut stdin = self
                .stdin
                .lock()
                .map_err(|_| io::Error::other("ACP stdin lock poisoned"))?;
            stdin.write_all(try_encode_message(json)?.as_bytes())?;
            stdin.flush()
        })();
        if result.is_err() {
            self.transport_quarantined = Some("stdin write failed");
        }
        result
    }

    fn cancellation_handle(&self) -> Option<AcpCancelHandle> {
        Some(self.cancel_handle.clone())
    }

    fn recv(&mut self) -> io::Result<Option<String>> {
        loop {
            if let Some(reason) = self.transport_quarantined {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    format!("ACP transport is quarantined: {reason}"),
                ));
            }
            if self
                .shutdown_requested
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "ACP transport shutdown requested",
                ));
            }
            // Serve any frames already decoded but not yet handed out (they
            // arrived together in one read chunk).
            if let Some(message) = self.pending.pop_front() {
                return Ok(Some(message));
            }
            let received = self
                .reader_rx
                .as_ref()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "ACP stdout reader is closed")
                })?
                .recv_timeout(TRANSPORT_READ_POLL);
            match received {
                Ok(Ok(Some(message))) => {
                    // Do not extend a cancellation deadline merely because
                    // the provider emitted unrelated interleaved data.
                    if self.cancellation_expired() {
                        self.transport_quarantined = Some("cancellation acknowledgement timed out");
                        self.terminate_process();
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "ACP cancellation acknowledgement timed out",
                        ));
                    }
                    return Ok(Some(message));
                }
                Ok(Ok(None)) => return Ok(None),
                Ok(Err(error)) => {
                    self.transport_quarantined = Some("stdout framing failed");
                    self.terminate_process();
                    return Err(error);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.transport_quarantined = Some("stdout reader stopped unexpectedly");
                    self.terminate_process();
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "ACP stdout reader stopped",
                    ));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self
                        .shutdown_requested
                        .load(std::sync::atomic::Ordering::Acquire)
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "ACP transport shutdown requested",
                        ));
                    }
                    if self.cancellation_expired() {
                        // No provider acknowledgement arrived within the
                        // bounded grace period. Kill/reap the child and
                        // surface an interrupted transport; the caller must
                        // classify the turn as unknown, not cancelled.
                        self.transport_quarantined = Some("cancellation acknowledgement timed out");
                        self.terminate_process();
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "ACP cancellation acknowledgement timed out",
                        ));
                    }
                }
            }
        }
    }

    fn is_alive(&mut self) -> bool {
        #[cfg(target_os = "linux")]
        if let Some(monitor) = self.monitor.as_mut() {
            return matches!(monitor.try_wait(), Ok(None));
        }
        matches!(
            self.child.as_mut().and_then(|child| child.try_wait().ok()),
            Some(None)
        )
    }

    fn shutdown(&mut self) {
        self.shutdown_requested
            .store(true, std::sync::atomic::Ordering::Release);
        // Closing the receiver releases a reader blocked on the bounded
        // hand-off queue. The child is then killed/reaped, so the detached
        // reader can observe either a send error or EOF without extending the
        // shutdown critical section.
        let _ = self.reader_rx.take();
        self.terminate_process();
        // Do not join an arbitrary reader thread here. The child has been
        // killed/reaped, so the thread will observe EOF; detaching keeps
        // shutdown bounded even if a platform pipe teardown is delayed.
        let _ = self.reader_thread.take();
    }
}

impl Drop for ProcessTransport {
    fn drop(&mut self) {
        self.shutdown_requested
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = self.reader_rx.take();
        self.terminate_process();
        let _ = self.reader_thread.take();
    }
}

/// The result of one driven prompt turn.
#[derive(Debug, Clone, Default)]
pub struct PromptOutcome {
    pub stop_reason: StopReason,
    /// P71.4 — the agent's own token/cost report for this turn, exactly as it
    /// sent it. `None` = the agent reported no usage; the ledger records that
    /// as an unreported turn rather than a measured zero (`ARCH/ROUTING.md` §5).
    pub usage: Option<PromptUsage>,
    /// `session/update` notifications collected during the turn.
    pub updates: Vec<SessionUpdate>,
    /// Permission requests the agent made (audit + Guard-2 trail).
    pub permissions: Vec<PermissionRequestParams>,
    /// The decisions handed back for those requests.
    pub permission_decisions: Vec<PermissionDecision>,
    /// Agent→client mediated calls serviced during the turn (P69.C2): each is
    /// routed through the host's [`ClientMediation`] seam and recorded here so
    /// the audit trail carries the same evidence the permission path does.
    pub mediated: Vec<MediatedCall>,
}

/// The result of an ACP v1 `session/load` request.
///
/// The wire response is normally just a session id.  `updates` contains any
/// `session/update` notifications that arrived while the load response was
/// being read, so a host can replay them into its normal event stream.
#[derive(Debug, Clone, Default)]
pub struct SessionLoadOutcome {
    pub session_id: String,
    pub updates: Vec<SessionUpdate>,
}

/// One agent→client request serviced through the mediation seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediatedCall {
    /// The ACP method (`fs/read_text_file`, `terminal/create`, …).
    pub method: String,
    /// Whether the host's mediator completed it (a refusal records `false`).
    pub ok: bool,
    /// The refusal/failure reason when `ok` is false.
    pub error: Option<String>,
}

/// The host's **mediation seam** for agent→client requests (P69.C2).
///
/// Mediated mode means the agent asks *us* to read/write files or run a
/// terminal command, and we service that request through the canonical
/// capability executor (Guard → ticket → executor → observation + receipt).
/// The ACP layer therefore never performs the effect itself — it hands the
/// method + params to the host and returns the host's result.
///
/// The default (no mediator attached) is an explicit fail-closed refusal, so a
/// mediated-mode session can never silently service an ungoverned effect.
pub trait ClientMediation: Send + Sync {
    /// Handle one request and return the JSON `result` payload, or a refusal
    /// message. Implementations must not invent a success they did not
    /// perform.
    fn call(&self, method: &str, params: &serde_json::Value) -> Result<serde_json::Value, String>;
}

/// The ACP v1 client-side surface the mediator answers (P69.C2). ACP v2
/// removes this surface in favour of `mcpServers`; v1-only agents remain
/// common, so mediated mode must service these names while Channel B (the MCP
/// catalogue) is the durable path.
pub fn is_mediated_client_method(method: &str) -> bool {
    matches!(
        method,
        "fs/read_text_file"
            | "fs/write_text_file"
            | "terminal/create"
            | "terminal/output"
            | "terminal/wait_for_exit"
            | "terminal/kill"
            | "terminal/release"
    )
}

/// An explicit refusal used when no mediator is attached.
pub struct NoMediation;

impl ClientMediation for NoMediation {
    fn call(&self, method: &str, _params: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err(format!(
            "mediated mode unavailable: no capability mediator attached for `{method}` \
             (self-contained mode — service it through the agent's own executor or Channel B)"
        ))
    }
}

/// Ownership state for notifications captured during `session/load`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplayOwner {
    /// The session owns the replay buffer until a successful prompt or an
    /// explicit caller-side take.
    Session(String),
    /// No replay buffer is owned by the session.
    None,
}

/// An ACP session: one agent subprocess + the JSON-RPC request/response state.
pub struct AcpSession<T: AcpTransport> {
    transport: T,
    next_id: u64,
    initialized: bool,
    session_id: Option<String>,
    agent_info: Option<AgentInfo>,
    auth_methods: Vec<AuthMethod>,
    agent_capabilities: Option<AgentCapabilities>,
    config_options: Vec<ConfigOption>,
    authenticated: bool,
    /// The host's mediation seam (P69.C2). `None` ⇒ mediated requests are
    /// refused explicitly rather than answered `-32601`.
    mediator: Option<std::sync::Arc<dyn ClientMediation>>,
    /// Inbound messages that arrived **before** the response we were waiting
    /// for. ACP agents interleave freely — `codex-acp` sends `session/update`
    /// notifications between our `session/new` request and its reply — so a
    /// response reader that assumes the next frame is its own answer breaks on
    /// a real agent. Non-matching frames are parked here and drained by
    /// [`AcpSession::prompt_with_content`].
    pending: std::collections::VecDeque<Value>,
    /// Valid response envelopes for other in-flight request ids. They are
    /// never consumed by the request that merely read them first.
    pending_responses: Vec<PendingResponse>,
    /// Bounded raw frames retained for diagnostics after a parse failure. The
    /// session is quarantined at the same time, so no later operation can
    /// silently continue past the unparseable boundary.
    pending_raw: std::collections::VecDeque<String>,
    /// Set when both bounded preservation queues overflow. The overflow is
    /// surfaced as a hard failure rather than silently discarded.
    pending_overflow: bool,
    /// A protocol/transport failure makes the framing boundary untrustworthy.
    /// The session remains inspectable but every request path fails closed until
    /// the owner rotates to a fresh session/transport handle.
    quarantined: Option<&'static str>,
    /// Notifications replayed from the most recent successful `session/load`.
    /// They remain available until the next prompt successfully completes (or
    /// an explicit take), so a failed/cancelled prompt cannot silently lose
    /// load-time state.
    replayed_updates: Vec<SessionUpdate>,
    /// Explicit owner of the replay buffer.  `Session` means the ACP session
    /// still owns the updates and may replay them; `None` means ownership was
    /// transferred to the caller or the updates were cleared.  This prevents a
    /// failed prompt from dropping a buffer that the next prompt must retry.
    replay_owner: ReplayOwner,
    /// A lock-free cancellation hook owned by the transport, when available.
    /// It is independent of the mutable session borrow so a host can cancel a
    /// blocked prompt without taking the global handle-map lock.
    cancel_handle: AcpCancelHandle,
}

impl<T: AcpTransport> AcpSession<T> {
    pub fn new(transport: T) -> Self {
        let cancel_handle = transport.cancellation_handle().unwrap_or_else(|| {
            AcpCancelHandle::new(Arc::new(std::sync::atomic::AtomicBool::new(false)), None)
        });
        Self {
            transport,
            next_id: 1,
            initialized: false,
            session_id: None,
            agent_info: None,
            auth_methods: Vec::new(),
            agent_capabilities: None,
            config_options: Vec::new(),
            authenticated: false,
            mediator: None,
            pending: std::collections::VecDeque::new(),
            pending_responses: Vec::new(),
            pending_raw: std::collections::VecDeque::new(),
            pending_overflow: false,
            quarantined: None,
            replayed_updates: Vec::new(),
            replay_owner: ReplayOwner::None,
            cancel_handle,
        }
    }

    /// Clone the cancellation hook for a host that cannot borrow the session
    /// while a prompt is blocked.
    pub fn cancellation_handle(&self) -> AcpCancelHandle {
        self.cancel_handle.clone()
    }

    /// Clear a previous cancellation request before starting the next turn on
    /// this provider session.
    pub fn reset_cancellation(&self) {
        self.cancel_handle.reset();
    }

    /// Whether this session handle has been quarantined after a framing or
    /// protocol failure. A quarantined handle must be replaced, not retried.
    pub fn is_quarantined(&self) -> bool {
        self.quarantined.is_some()
            || self
                .cancel_handle
                .faulted
                .load(std::sync::atomic::Ordering::Acquire)
    }

    fn ensure_usable(&self) -> Result<(), AcpError> {
        if let Some(reason) = self.quarantined {
            return Err(AcpError::Quarantined(reason));
        }
        if self
            .cancel_handle
            .faulted
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(AcpError::Quarantined("cancellation writer failed"));
        }
        if self.pending_overflow {
            return Err(AcpError::Quarantined("pending-frame queue overflow"));
        }
        if !self.pending_raw.is_empty() {
            return Err(AcpError::Quarantined("unparsed ACP frame"));
        }
        Ok(())
    }

    fn mark_quarantined(&mut self, reason: &'static str) {
        if self.quarantined.is_none() {
            self.quarantined = Some(reason);
        }
    }

    fn send_frame(&mut self, json: &str) -> Result<(), AcpError> {
        self.ensure_usable()?;
        if let Err(error) = try_encode_message(json) {
            self.mark_quarantined("outbound ACP frame was invalid or oversized");
            return if error.kind() == io::ErrorKind::InvalidData {
                Err(AcpError::FrameTooLarge)
            } else {
                Err(AcpError::Malformed(
                    "outbound ACP frame was not a single valid frame".into(),
                ))
            };
        }
        self.transport.send(json).map_err(|error| {
            self.mark_quarantined("transport write failed");
            AcpError::Io(error)
        })
    }

    /// Attach the host's mediation seam (P69.C2). Attach it **before**
    /// `initialize_with_caps` so the advertised client capabilities match the
    /// mode the host can actually service (P69.C3).
    pub fn set_mediator(&mut self, mediator: std::sync::Arc<dyn ClientMediation>) {
        self.mediator = Some(mediator);
    }

    /// Whether a mediation seam is attached (drives the mediated/self-contained
    /// advertisement).
    pub fn has_mediator(&self) -> bool {
        self.mediator.is_some()
    }

    pub fn agent_info(&self) -> Option<&AgentInfo> {
        self.agent_info.as_ref()
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// The authentication methods the agent advertised in `initialize`
    /// (`authMethods`). Empty ⇒ the agent needs no auth.
    pub fn auth_methods(&self) -> &[AuthMethod] {
        &self.auth_methods
    }

    /// The agent capability set advertised during `initialize`.
    pub fn agent_capabilities(&self) -> Option<&AgentCapabilities> {
        self.agent_capabilities.as_ref()
    }

    /// Whether `authenticate` has succeeded on this connection.
    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }

    /// The latest complete agent-owned session configuration.
    pub fn config_options(&self) -> &[ConfigOption] {
        &self.config_options
    }

    /// ACP handshake: `initialize` → version/capability negotiation, with the
    /// **withhold** client capability set (fs/terminal: false) — the
    /// Self-contained governance path. Withholding never forces MCP Channel B
    /// (spec §4.2.5a §3, corrected v3.46); use
    /// [`Self::initialize_with_caps`] to advertise the mediated surface.
    pub fn initialize(&mut self, client_info: ClientInfo) -> Result<InitializeResult, AcpError> {
        self.initialize_with_caps(client_info, ClientCapabilities::default())
    }

    /// ACP handshake with an explicit client capability set (P38
    /// GovernedSession): pass a capability set with `fs.readTextFile` /
    /// `writeTextFile` / `terminal` = true to advertise the **Mediated**
    /// surface (sandbox-aware agents then delegate their file/shell ops to
    /// us); pass the default (all false) to withhold.
    pub fn initialize_with_caps(
        &mut self,
        client_info: ClientInfo,
        client_capabilities: ClientCapabilities,
    ) -> Result<InitializeResult, AcpError> {
        let id = self.next_id;
        self.next_id += 1;
        let caps = serde_json::to_value(&client_capabilities)
            .map_err(|e| AcpError::Malformed(e.to_string()))?;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": caps,
                "clientInfo": client_info,
            }
        });
        self.send_frame(&req.to_string())?;
        let resp = self.read_response(id)?;
        let result: InitializeResult = serde_json::from_value(resp.clone()).map_err(|e| {
            // A bare "malformed agent message" costs a debugging cycle: the
            // `pi-acp` authMethods shape was only identifiable from the bytes.
            // The initialize reply carries capabilities + auth methods and no
            // secrets, so the snippet is safe to surface here.
            let raw = resp.to_string();
            let mut snip: String = raw.chars().take(600).collect();
            if raw.chars().count() > 600 {
                snip.push('\u{2026}');
            }
            AcpError::Malformed(format!("initialize result: {e} — reply was {snip}"))
        })?;
        if result.protocol_version != PROTOCOL_VERSION {
            return Err(AcpError::ProtocolMismatch(result.protocol_version));
        }
        self.agent_info = Some(result.agent_info.clone());
        self.agent_capabilities = Some(result.agent_capabilities.clone());
        self.auth_methods = result.auth_methods.clone();
        // An agent advertising no auth methods needs no auth: the session is
        // authenticated by construction (see `auth_methods`). Anything
        // advertised must still complete `authenticate` explicitly.
        self.authenticated = self.auth_methods.is_empty();
        self.initialized = true;
        Ok(result)
    }

    /// Authenticate with one of the methods advertised in `initialize`
    /// (`authenticate` request). `method_id` must match an advertised id.
    ///
    /// Agent-type methods return an empty result (the agent drives its own
    /// login flow — prints a URL / opens its own browser). URL-type methods
    /// return a `url` the client opens in the system browser; the caller
    /// should surface it, then call `authenticate` again once the user has
    /// completed login.
    pub fn authenticate(&mut self, method_id: &str) -> Result<AuthenticateResult, AcpError> {
        self.ensure_initialized()?;
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "authenticate",
            "params": { "methodId": method_id }
        });
        self.send_frame(&req.to_string())?;
        let resp = self.read_response(id)?;
        let result: AuthenticateResult =
            serde_json::from_value(resp).map_err(|e| AcpError::Malformed(e.to_string()))?;
        // A `url` means the user must complete login in the browser first;
        // an empty result means the flow already succeeded.
        if result.url.is_none() {
            self.authenticated = true;
        }
        Ok(result)
    }

    /// End the authenticated state (`logout` request). The agent must have
    /// advertised `agentCapabilities.auth.logout` in `initialize`; this is a
    /// best-effort call (the caller checks the capability first).
    pub fn logout(&mut self) -> Result<(), AcpError> {
        self.ensure_initialized()?;
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "logout",
            "params": {}
        });
        self.send_frame(&req.to_string())?;
        self.read_response(id)?;
        self.authenticated = false;
        Ok(())
    }

    /// Create a session in the agent's workspace (`session/new`).
    ///
    /// ACP requires `cwd` to be an **absolute** path, and agents enforce it
    /// (`pi-acp` answers `cwd must be an absolute path: .` with -32602). The
    /// path is therefore canonicalized here, in the one place every driver
    /// goes through, rather than trusting each caller to remember — a relative
    /// path is resolved against the process cwd, which is the same directory
    /// the agent was spawned in.
    pub fn session_new(
        &mut self,
        cwd: &str,
        mcp_servers: Vec<McpServer>,
    ) -> Result<String, AcpError> {
        self.ensure_ready()?;
        validate_mcp_servers(&mcp_servers)?;
        self.validate_mcp_transport_support(&mcp_servers)?;
        let cwd_abs = absolute_cwd(cwd)?;
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/new",
            "params": { "cwd": cwd_abs, "mcpServers": mcp_servers }
        });
        self.send_frame(&req.to_string())?;
        let resp = self.read_response(id)?;
        let result: SessionNewResult =
            serde_json::from_value(resp).map_err(|e| AcpError::Malformed(e.to_string()))?;
        if !valid_session_id(&result.session_id) {
            return Err(AcpError::InvalidSessionId);
        }
        self.session_id = Some(result.session_id.clone());
        self.config_options = result.config_options.clone();
        self.clear_replay();
        Ok(result.session_id)
    }

    /// Load an existing ACP v1 provider session (`session/load`).
    ///
    /// This is a real provider resume operation, not a `session/new` fallback.
    /// The agent must have explicitly negotiated `agentCapabilities.loadSession`
    /// during `initialize`; omission is refusal. The requested provider id,
    /// absolute workspace, and MCP descriptor set are sent together. ACP v1
    /// permits an empty/options-only result, so the requested id remains the
    /// active identity unless the agent returns a bounded extension id.
    /// Notifications that arrive before the response are retained for replay.
    ///
    /// **Consumer status (P71.12, 2026-09-25): no production call site.** The
    /// only caller is this crate's handshake acceptance suite. The shell seam
    /// `acp_cmds::acp_session_load` deliberately refuses rather than guessing:
    /// `ADR-0007` §3 makes transport reconnect the precondition for attempting
    /// provider resume, and v1 has no reconnect seam, while §4 keeps ACP v2 (and
    /// therefore its `session/resume` method) out of scope. An added caller must
    /// re-attach the *same* `(SessionId, WorkId, RunId, AgentBindingId)` and
    /// must take its provider session id from the canonical binding record —
    /// never from a handle, a filename, or a fresh `session/new`.
    pub fn session_load(
        &mut self,
        session_id: &str,
        cwd: &str,
        mcp_servers: Vec<McpServer>,
    ) -> Result<String, AcpError> {
        Ok(self
            .session_load_with_updates(session_id, cwd, mcp_servers)?
            .session_id)
    }

    /// Alias for callers that use the protocol method name in prose.
    pub fn load_session(
        &mut self,
        session_id: &str,
        cwd: &str,
        mcp_servers: Vec<McpServer>,
    ) -> Result<String, AcpError> {
        self.session_load(session_id, cwd, mcp_servers)
    }

    /// Load a provider session and return the load-time update notifications
    /// alongside the validated active session id.
    pub fn session_load_with_updates(
        &mut self,
        session_id: &str,
        cwd: &str,
        mcp_servers: Vec<McpServer>,
    ) -> Result<SessionLoadOutcome, AcpError> {
        self.ensure_ready()?;
        let supports_load = self
            .agent_capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.load_session);
        if !supports_load {
            return Err(AcpError::SessionLoadUnsupported);
        }
        if !valid_session_id(session_id) {
            return Err(AcpError::InvalidSessionId);
        }
        validate_mcp_servers(&mcp_servers)?;
        self.validate_mcp_transport_support(&mcp_servers)?;
        let cwd_abs = absolute_cwd(cwd)?;
        let id = self.next_id;
        self.next_id += 1;
        let params = SessionLoadParams {
            session_id: session_id.to_string(),
            cwd: cwd_abs,
            mcp_servers,
        };
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/load",
            "params": params,
        });
        self.send_frame(&req.to_string())?;

        // Capture interleaved frames locally. Nothing below mutates the
        // active session/config/replay state until the response, every inline
        // update, and every captured frame has been validated and staged.
        // Frames retained for the next turn are queued only at the same commit
        // point, so a failed load is a true rollback rather than a half-applied
        // provider state.
        let mut captured = Vec::new();
        let response = match self.read_response_capture(JsonRpcId::number(id), &mut captured) {
            Ok(response) => response,
            Err(error) => {
                self.preserve_values(captured);
                return Err(error);
            }
        };
        let result: SessionLoadResult = if response.is_null() {
            // `result: null` is a valid JSON-RPC result.  A response with no
            // `result` member never reaches this branch: the envelope decoder
            // rejects it before the load operation is allowed to proceed.
            SessionLoadResult::default()
        } else {
            match serde_json::from_value(response) {
                Ok(result) => result,
                Err(error) => {
                    self.preserve_values(captured);
                    return Err(AcpError::Malformed(error.to_string()));
                }
            }
        };
        let active_session_id = match result.session_id {
            Some(returned) => {
                if !valid_session_id(&returned) {
                    self.preserve_values(captured);
                    return Err(AcpError::InvalidSessionId);
                }
                returned
            }
            None => session_id.to_string(),
        };
        // Once the response is decoded, the active identity is the returned
        // extension id when present, otherwise the requested provider id. Do
        // not accept frames for the other value: that would make a load
        // response a cross-session replay primitive.
        let allowed_ids = vec![active_session_id.clone()];
        let mut staged_config = result.config_options;
        let mut updates = result.updates;
        for update in &updates {
            if let Err(error) = validate_load_update(update, &allowed_ids) {
                self.preserve_values(captured);
                return Err(error);
            }
        }
        for update in &updates {
            if update.is_config_option_update() && !update.config_options.is_empty() {
                staged_config = update.config_options.clone();
            }
        }
        let (captured_updates, retained, final_config) =
            match self.classify_load_frames(&captured, &allowed_ids, &staged_config) {
                Ok(parts) => parts,
                Err(error) => {
                    self.preserve_values(captured);
                    return Err(error);
                }
            };
        updates.extend(captured_updates);

        // Check queue capacity before committing any state. On overflow the
        // captured frames are preserved and the old active state remains
        // untouched; the overflow flag makes subsequent operations fail closed.
        if !self.has_pending_capacity(retained.len()) {
            self.preserve_values(captured);
            self.mark_quarantined("pending-frame queue overflow");
            return Err(AcpError::PendingQueueFull);
        }

        // Commit point: all validation and staging has succeeded. Captured
        // frames preceded any frames still waiting in the queue, so restore
        // that arrival order at the front.
        for value in retained.into_iter().rev() {
            self.pending.push_front(value);
        }
        self.session_id = Some(active_session_id.clone());
        self.config_options = final_config;
        self.replayed_updates = updates.clone();
        self.replay_owner = ReplayOwner::Session(active_session_id.clone());
        Ok(SessionLoadOutcome {
            session_id: active_session_id,
            updates,
        })
    }

    /// Alias for [`Self::session_load_with_updates`].
    pub fn load_session_with_updates(
        &mut self,
        session_id: &str,
        cwd: &str,
        mcp_servers: Vec<McpServer>,
    ) -> Result<SessionLoadOutcome, AcpError> {
        self.session_load_with_updates(session_id, cwd, mcp_servers)
    }

    /// Updates captured by the most recent successful `session/load`.
    pub fn replayed_session_updates(&self) -> &[SessionUpdate] {
        &self.replayed_updates
    }

    pub fn replayed_updates(&self) -> &[SessionUpdate] {
        self.replayed_session_updates()
    }

    /// Take the load-time updates, transferring ownership to the caller.
    /// Calling this is optional; a subsequent prompt also claims the same
    /// updates, but only after that prompt reaches a provider response.
    pub fn take_replayed_session_updates(&mut self) -> Vec<SessionUpdate> {
        if !self.replay_owned_by_current_session() {
            self.clear_replay();
            return Vec::new();
        }
        self.replay_owner = ReplayOwner::None;
        std::mem::take(&mut self.replayed_updates)
    }

    fn clear_replay(&mut self) {
        self.replayed_updates.clear();
        self.replay_owner = ReplayOwner::None;
    }

    fn replay_owned_by_current_session(&self) -> bool {
        matches!(
            &self.replay_owner,
            ReplayOwner::Session(owner) if self.session_id.as_deref() == Some(owner.as_str())
        )
    }

    fn replay_for_prompt(&self, session_id: &str) -> Vec<SessionUpdate> {
        if matches!(&self.replay_owner, ReplayOwner::Session(owner) if owner == session_id) {
            self.replayed_updates.clone()
        } else {
            Vec::new()
        }
    }

    fn commit_replay_consumed(&mut self, session_id: &str) {
        if matches!(&self.replay_owner, ReplayOwner::Session(owner) if owner == session_id) {
            self.clear_replay();
        }
    }

    /// Change one agent-owned session configuration value. The agent returns
    /// the complete configuration list so dependent model/mode options remain
    /// coherent.
    pub fn set_config_option(
        &mut self,
        config_id: &str,
        value: serde_json::Value,
    ) -> Result<Vec<ConfigOption>, AcpError> {
        self.ensure_ready()?;
        let session_id = self.session_id.clone().ok_or(AcpError::NotReady)?;
        let id = self.next_id;
        self.next_id += 1;
        let params = SetConfigOptionParams {
            session_id,
            config_id: config_id.to_string(),
            value,
        };
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/set_config_option",
            "params": params,
        });
        self.send_frame(&req.to_string())?;
        let resp = self.read_response(id)?;
        let result: SetConfigOptionResult =
            serde_json::from_value(resp).map_err(|e| AcpError::Malformed(e.to_string()))?;
        self.config_options = result.config_options.clone();
        Ok(result.config_options)
    }

    /// Drive one prompt turn. Sends `session/prompt`, then reads inbound
    /// messages until the prompt response: `session/update` notifications are
    /// collected, and `session/request_permission` requests are answered via
    /// `on_permission` (the Guard-2 seam). Unsupported client methods get a
    /// clean method-not-found error.
    pub fn prompt(
        &mut self,
        text: &str,
        on_permission: impl FnMut(&PermissionRequestParams) -> PermissionDecision,
    ) -> Result<PromptOutcome, AcpError> {
        self.prompt_with_content(vec![PromptContent::text(text)], on_permission)
    }

    /// Drive one prompt with capability-gated content blocks. The caller is
    /// responsible for checking the agent's advertised capabilities before
    /// adding resource blocks; plain text remains the safe fallback.
    pub fn prompt_with_content(
        &mut self,
        prompt: Vec<PromptContent>,
        mut on_permission: impl FnMut(&PermissionRequestParams) -> PermissionDecision,
    ) -> Result<PromptOutcome, AcpError> {
        self.ensure_ready()?;
        self.ensure_usable()?;
        let session_id = self.session_id.clone().ok_or(AcpError::NotReady)?;
        // Opening a turn clears a stale local flag and installs a generation
        // fence. A cancellation racing this point is handled by the new turn;
        // a cancellation arriving after the provider response is fenced out by
        // `TurnFence::complete` below.
        let mut turn = self.cancel_handle.begin_turn();
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": prompt,
            }
        });
        self.send_frame(&req.to_string())?;

        // Clone rather than take: the session remains the owner until the
        // provider returns a terminal response. Failed and locally cancelled
        // turns therefore leave the same replay available for a retry.
        let replayed = self.replay_for_prompt(&session_id);
        let mut outcome = PromptOutcome {
            // Load-time notifications are replayed into the next turn when the
            // host did not explicitly take them first.
            updates: replayed,
            ..PromptOutcome::default()
        };
        loop {
            self.ensure_usable()?;
            // Anything parked while waiting on a handshake response is handled
            // first, in arrival order — a notification that arrives before a
            // reply must not be lost.
            let value = if let Some(parked) = self.pending.pop_front() {
                parked
            } else {
                let raw = match self.transport.recv() {
                    Ok(Some(raw)) => raw,
                    Ok(None) => {
                        self.mark_quarantined("transport ended before prompt completion");
                        return Err(if turn.requested() {
                            AcpError::CancellationOutcomeUnknown
                        } else {
                            AcpError::Eof
                        });
                    }
                    Err(error) => {
                        self.mark_quarantined("transport read failed");
                        return Err(if turn.requested() {
                            AcpError::CancellationOutcomeUnknown
                        } else {
                            AcpError::Io(error)
                        });
                    }
                };
                self.decode_raw_frame(raw)?
            };

            let frame = match decode_inbound_frame(&value) {
                Ok(frame) => frame,
                Err(error) => {
                    if self.pending_raw.len() < MAX_PENDING_FRAMES {
                        self.pending_raw.push_back(value.to_string());
                    } else {
                        self.pending_overflow = true;
                    }
                    self.mark_quarantined("invalid JSON-RPC envelope");
                    return Err(error);
                }
            };

            match frame {
                RpcFrame::Response {
                    id: response_id,
                    payload,
                } => {
                    // JSON-RPC id equality is type-sensitive. A response for a
                    // different in-flight request remains available to its
                    // owner; it must not strand or complete this request.
                    if response_id != JsonRpcId::number(id) {
                        self.defer_response(response_id, payload)?;
                        continue;
                    }
                    // Fence the turn as soon as the expected response is
                    // visible, before decoding its payload.
                    turn.complete();
                    match payload {
                        ResponsePayload::Error(error) => return Err(map_error(&error)),
                        ResponsePayload::Result(result) => {
                            let result: SessionPromptResult = serde_json::from_value(result)
                                .map_err(|error| {
                                    self.mark_quarantined("prompt response payload was malformed");
                                    AcpError::Malformed(error.to_string())
                                })?;
                            outcome.stop_reason = result.stop_reason;
                            // P71.4 — carry the agent's usage report through untouched.
                            outcome.usage = result.usage;
                            self.commit_replay_consumed(&session_id);
                            return Ok(outcome);
                        }
                    }
                }
                RpcFrame::Request { method, id: rid } => {
                    match method.as_str() {
                        "session/request_permission" => {
                            let params: PermissionRequestParams = serde_json::from_value(
                                value.get("params").cloned().unwrap_or_default(),
                            )
                            .map_err(|error| {
                                self.mark_quarantined("permission request payload was malformed");
                                AcpError::Malformed(error.to_string())
                            })?;
                            if !valid_session_id(&params.session_id)
                                || params.session_id != session_id
                            {
                                self.mark_quarantined("cross-session frame arrived during prompt");
                                self.preserve_pending_value(value)?;
                                return Err(AcpError::CrossSessionFrame);
                            }
                            let decision = on_permission(&params);
                            // FIX-03: the reply must name an option the agent
                            // actually offered. When the bridge cannot express
                            // the decision, the request is refused with a typed
                            // JSON-RPC error and the turn fails closed — never a
                            // synthesized option id and never a silent allow.
                            let option_id = match resolve_option(&params, &decision) {
                                Ok(option_id) => option_id,
                                Err(error) => {
                                    outcome.permissions.push(params);
                                    outcome.permission_decisions.push(decision);
                                    let reply = json!({
                                        "jsonrpc": "2.0", "id": rid,
                                        "error": {
                                            "code": ERROR_PERMISSION_UNANSWERABLE,
                                            "message": error.to_string()
                                        }
                                    });
                                    self.send_frame(&reply.to_string())?;
                                    self.mark_quarantined(
                                        "permission request could not be answered fail-closed",
                                    );
                                    return Err(error);
                                }
                            };
                            outcome.permissions.push(params);
                            outcome.permission_decisions.push(decision);
                            let result = PermissionResult {
                                outcome: PermissionOutcome { option_id },
                            };
                            let reply = json!({ "jsonrpc": "2.0", "id": rid, "result": result });
                            self.send_frame(&reply.to_string())?;
                        }
                        // P69.C2 — mediated fs/terminal requests go through the
                        // host's capability seam; never `-32601` when mediated.
                        other if is_mediated_client_method(other) => {
                            let params = value.get("params").cloned().unwrap_or(Value::Null);
                            let mediator = self.mediator.clone();
                            let reply = match mediator {
                                Some(m) => match m.call(other, &params) {
                                    Ok(result) => {
                                        outcome.mediated.push(MediatedCall {
                                            method: other.to_string(),
                                            ok: true,
                                            error: None,
                                        });
                                        json!({"jsonrpc": "2.0", "id": rid, "result": result})
                                    }
                                    Err(message) => {
                                        outcome.mediated.push(MediatedCall {
                                            method: other.to_string(),
                                            ok: false,
                                            error: Some(message.clone()),
                                        });
                                        json!({
                                            "jsonrpc": "2.0", "id": rid,
                                            "error": { "code": -32603, "message": message }
                                        })
                                    }
                                },
                                None => {
                                    let message = format!(
                                        "mediated mode unavailable: no capability mediator attached \
                                         for `{other}` (self-contained mode)"
                                    );
                                    outcome.mediated.push(MediatedCall {
                                        method: other.to_string(),
                                        ok: false,
                                        error: Some(message.clone()),
                                    });
                                    json!({
                                        "jsonrpc": "2.0", "id": rid,
                                        "error": { "code": -32603, "message": message }
                                    })
                                }
                            };
                            self.send_frame(&reply.to_string())?;
                        }
                        other => {
                            let reply = json!({
                                "jsonrpc": "2.0", "id": rid,
                                "error": {
                                    "code": -32601,
                                    "message": format!("method not found: {other}")
                                }
                            });
                            self.send_frame(&reply.to_string())?;
                        }
                    }
                }
                RpcFrame::Notification { method } => {
                    if method == "session/update" {
                        let update: SessionUpdate = serde_json::from_value(update_params(
                            value.get("params"),
                        ))
                        .map_err(|error| {
                            self.mark_quarantined("session/update payload was malformed");
                            AcpError::Malformed(error.to_string())
                        })?;
                        if !valid_session_id(&update.session_id) || update.session_id != session_id
                        {
                            self.mark_quarantined("cross-session frame arrived during prompt");
                            self.preserve_pending_value(value)?;
                            return Err(AcpError::CrossSessionFrame);
                        }
                        if update.is_config_option_update() && !update.config_options.is_empty() {
                            self.config_options = update.config_options.clone();
                        }
                        outcome.updates.push(update);
                    }
                }
            }
        }
    }

    /// Interrupt the ongoing turn (`session/cancel` notification).
    pub fn cancel(&mut self) -> Result<(), AcpError> {
        self.ensure_ready()?;
        self.ensure_usable()?;
        let session_id = self.session_id.clone().ok_or(AcpError::NotReady)?;
        if self.cancel_handle.sender.is_some() {
            self.cancel_handle.request(&session_id)?;
        } else if self.cancel_handle.has_active_turn() {
            self.cancel_handle.mark_requested()?;
            let msg = json!({
                "jsonrpc": "2.0",
                "method": "session/cancel",
                "params": { "sessionId": session_id }
            });
            self.send_frame(&msg.to_string())?;
        } else {
            // Record the local request for the next fenced turn, but do not
            // send a stale cancellation notification while no turn is active.
            self.cancel_handle.mark_requested()?;
        }
        Ok(())
    }

    /// Request cancellation without mutably borrowing the session. This is the
    /// path used by a host-side stop command while a prompt owns the session
    /// mutex.  Process transports also expose a bounded shutdown signal; a
    /// custom transport without that hook fails closed as an unsupported
    /// out-of-band operation rather than pretending the provider stopped.
    pub fn request_cancel(&self) -> Result<(), AcpError> {
        self.ensure_ready()?;
        self.ensure_usable()?;
        let session_id = self.session_id.as_deref().ok_or(AcpError::NotReady)?;
        self.cancel_handle.request(session_id).map_err(AcpError::Io)
    }

    /// Is the underlying agent process still alive?
    pub fn is_alive(&mut self) -> bool {
        self.transport.is_alive()
    }

    /// Tear the agent down (kill + reap).
    pub fn shutdown(&mut self) {
        // Wake a blocked reader through the shared hook first.  The concrete
        // transport still owns final kill/reap, which keeps this method
        // bounded even when the agent is unresponsive.
        let _ = self.cancel_handle.request_shutdown();
        self.transport.shutdown();
    }

    fn ensure_ready(&self) -> Result<(), AcpError> {
        self.ensure_initialized()?;
        Ok(())
    }

    fn ensure_initialized(&self) -> Result<(), AcpError> {
        if !self.initialized {
            return Err(AcpError::NotReady);
        }
        Ok(())
    }

    /// Gate every HTTP/SSE descriptor on the capability negotiated by the
    /// agent. This runs before request serialization and before transport I/O.
    fn validate_mcp_transport_support(&self, servers: &[McpServer]) -> Result<(), AcpError> {
        let capabilities = self
            .agent_capabilities
            .as_ref()
            .map(|capabilities| &capabilities.mcp_capabilities);
        if servers.iter().any(McpServer::is_http)
            && !capabilities.is_some_and(|capabilities| capabilities.http)
        {
            return Err(AcpError::McpHttpUnsupported);
        }
        if servers.iter().any(McpServer::is_sse)
            && !capabilities.is_some_and(|capabilities| capabilities.sse)
        {
            return Err(AcpError::McpSseUnsupported);
        }
        Ok(())
    }

    /// Decode one bounded raw frame. Any framing/JSON failure poisons the
    /// handle because later bytes can no longer be proven to share a boundary
    /// with the request being awaited.
    fn decode_raw_frame(&mut self, raw: String) -> Result<Value, AcpError> {
        if raw.len() > MAX_ACP_FRAME_BYTES {
            self.mark_quarantined("inbound frame exceeded the byte limit");
            return Err(AcpError::FrameTooLarge);
        }
        match serde_json::from_str(&raw) {
            Ok(value) => Ok(value),
            Err(error) => {
                if self.pending_raw.len() < MAX_PENDING_FRAMES {
                    self.pending_raw.push_back(raw);
                } else {
                    self.pending_overflow = true;
                }
                self.mark_quarantined("inbound frame was not valid JSON");
                Err(AcpError::Malformed(error.to_string()))
            }
        }
    }

    fn defer_response(&mut self, id: JsonRpcId, payload: ResponsePayload) -> Result<(), AcpError> {
        if !self.has_pending_capacity(1) {
            self.mark_quarantined("pending-response queue overflow");
            return Err(AcpError::PendingQueueFull);
        }
        self.pending_responses.push(PendingResponse { id, payload });
        Ok(())
    }

    fn preserve_pending_value(&mut self, value: Value) -> Result<(), AcpError> {
        if self.pending.len() >= MAX_PENDING_FRAMES {
            self.mark_quarantined("pending-frame queue overflow");
            return Err(AcpError::PendingQueueFull);
        }
        self.pending.push_front(value);
        Ok(())
    }

    fn capture_value(&mut self, captured: &mut Vec<Value>, value: Value) -> Result<(), AcpError> {
        if captured.len() >= MAX_PENDING_FRAMES {
            self.mark_quarantined("pending-frame queue overflow");
            return Err(AcpError::PendingQueueFull);
        }
        captured.push(value);
        Ok(())
    }

    /// Preserve valid interleaved frames captured during a failed operation.
    /// The aggregate preservation budget is bounded; overflow quarantines the
    /// handle rather than silently discarding protocol data.
    fn preserve_values(&mut self, values: Vec<Value>) {
        let retained = self
            .pending
            .len()
            .saturating_add(self.pending_responses.len())
            .saturating_add(self.pending_raw.len());
        if retained.saturating_add(values.len()) > MAX_PENDING_FRAMES.saturating_mul(2) {
            self.pending_overflow = true;
            self.mark_quarantined("pending-frame queue overflow");
            return;
        }
        // `captured` frames arrived before any frames left in the queue. Put
        // them back at the front in their original order; appending would
        // silently reorder replay and response ownership after a failed wait.
        for value in values.into_iter().rev() {
            if self.pending.len() < MAX_PENDING_FRAMES {
                self.pending.push_front(value);
            } else if self.pending_raw.len() < MAX_PENDING_FRAMES {
                self.pending_raw.push_front(value.to_string());
            } else {
                self.pending_overflow = true;
                self.mark_quarantined("pending-frame queue overflow");
            }
        }
    }

    fn has_pending_capacity(&self, additional: usize) -> bool {
        self.pending
            .len()
            .saturating_add(self.pending_responses.len())
            .saturating_add(self.pending_raw.len())
            .saturating_add(additional)
            <= MAX_PENDING_FRAMES.saturating_mul(2)
    }

    /// Validate and partition frames captured while `session/load` waited for
    /// its response without touching active session state. Matching updates
    /// are consumed for replay; requests and unrelated frames remain queued for
    /// the next prompt. The returned config vector is the fully staged result
    /// of applying config updates in arrival order.
    fn classify_load_frames(
        &mut self,
        frames: &[Value],
        allowed_ids: &[String],
        initial_config: &[ConfigOption],
    ) -> Result<(Vec<SessionUpdate>, Vec<Value>, Vec<ConfigOption>), AcpError> {
        let mut updates = Vec::new();
        let mut retained = Vec::new();
        let mut staged_config = initial_config.to_vec();
        for frame in frames {
            match decode_inbound_frame(frame) {
                Ok(RpcFrame::Notification { method }) if method == "session/update" => {
                    let update: SessionUpdate =
                        serde_json::from_value(update_params(frame.get("params")))
                            .map_err(|error| AcpError::Malformed(error.to_string()))?;
                    validate_load_update(&update, allowed_ids)?;
                    if update.is_config_option_update() && !update.config_options.is_empty() {
                        staged_config = update.config_options.clone();
                    }
                    updates.push(update);
                }
                Ok(RpcFrame::Request { method, .. }) if method == "session/request_permission" => {
                    let params: PermissionRequestParams =
                        serde_json::from_value(frame.get("params").cloned().unwrap_or_default())
                            .map_err(|error| AcpError::Malformed(error.to_string()))?;
                    if !allowed_ids.iter().any(|id| id == &params.session_id) {
                        self.mark_quarantined("cross-session frame arrived during load");
                        return Err(AcpError::CrossSessionFrame);
                    }
                    retained.push(frame.clone());
                }
                Ok(_) => retained.push(frame.clone()),
                Err(error) => {
                    self.mark_quarantined("invalid JSON-RPC envelope during load");
                    return Err(error);
                }
            }
        }
        Ok((updates, retained, staged_config))
    }

    /// Read until the response to `expected_id` arrives, collecting every
    /// valid interleaved frame in `captured`. The caller decides whether those
    /// frames belong to the current operation; on any error it can preserve
    /// them without guessing from a queue-length snapshot.
    ///
    /// A response for a different request is valid protocol data, not a reason
    /// to discard the frame. Identifier type is part of the comparison, so a
    /// string `"7"` is preserved for its own owner and never completes numeric
    /// request `7` (and the inverse holds as well).
    fn read_response_capture(
        &mut self,
        expected_id: JsonRpcId,
        captured: &mut Vec<Value>,
    ) -> Result<Value, AcpError> {
        self.ensure_usable()?;
        if let Some(index) = self
            .pending_responses
            .iter()
            .position(|response| response.id == expected_id)
        {
            let response = self.pending_responses.remove(index);
            return match response.payload {
                ResponsePayload::Result(result) => Ok(result),
                ResponsePayload::Error(error) => Err(map_error(&error)),
            };
        }
        loop {
            // A previous operation may already have read frames belonging to a
            // different request. Drain those before asking the transport for a
            // new frame, otherwise the response can be stranded behind a queue
            // that this operation never examines.
            let value = if let Some(value) = self.pending.pop_front() {
                value
            } else {
                let raw = match self.transport.recv() {
                    Ok(Some(raw)) => raw,
                    Ok(None) => {
                        self.mark_quarantined("transport ended before a response arrived");
                        return Err(AcpError::Eof);
                    }
                    Err(error) => {
                        self.mark_quarantined("transport read failed");
                        return Err(AcpError::Io(error));
                    }
                };
                self.decode_raw_frame(raw)?
            };

            match decode_inbound_frame(&value) {
                Ok(RpcFrame::Request { .. } | RpcFrame::Notification { .. }) => {
                    self.capture_value(captured, value)?;
                }
                Ok(RpcFrame::Response { id, payload }) if id == expected_id => {
                    return match payload {
                        ResponsePayload::Result(result) => Ok(result),
                        ResponsePayload::Error(error) => Err(map_error(&error)),
                    };
                }
                Ok(RpcFrame::Response { id, payload }) => {
                    self.defer_response(id, payload)?;
                }
                Err(error) => {
                    // A value already admitted to `pending` must have passed
                    // this same validation previously. Treat any discrepancy as
                    // desynchronization rather than attempting recovery.
                    if self.pending_raw.len() < MAX_PENDING_FRAMES {
                        self.pending_raw.push_back(value.to_string());
                    } else {
                        self.pending_overflow = true;
                    }
                    self.mark_quarantined("invalid JSON-RPC envelope");
                    return Err(error);
                }
            }
        }
    }

    /// Read until the response to `expected_id` arrives, parking all valid
    /// interleaved frames for the next operation. Unlike the old load-specific
    /// discard path, a result parse error never removes these frames.
    fn read_response(&mut self, expected_id: u64) -> Result<Value, AcpError> {
        let mut captured = Vec::new();
        let result = self.read_response_capture(JsonRpcId::number(expected_id), &mut captured);
        self.preserve_values(captured);
        result
    }
}

fn validate_load_update(update: &SessionUpdate, allowed_ids: &[String]) -> Result<(), AcpError> {
    if !valid_session_id(&update.session_id)
        || !allowed_ids.iter().any(|id| id == &update.session_id)
    {
        return Err(AcpError::CrossSessionFrame);
    }
    Ok(())
}

fn validate_mcp_servers(servers: &[McpServer]) -> Result<(), AcpError> {
    McpServer::validate_list(servers).map_err(|error| AcpError::InvalidMcpServer(error.to_string()))
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ACP_SESSION_ID_BYTES
        && id.trim() == id
        && !id.chars().any(char::is_control)
}

fn absolute_cwd(cwd: &str) -> Result<String, AcpError> {
    if cwd.trim().is_empty() {
        return Err(AcpError::InvalidCwd("cwd must not be empty".into()));
    }
    let path = Path::new(cwd);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| AcpError::InvalidCwd(error.to_string()))?
            .join(path)
    };
    let normalized = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    let value = normalized.to_string_lossy().into_owned();
    if !Path::new(&value).is_absolute() {
        return Err(AcpError::InvalidCwd(
            "cwd could not be made absolute".into(),
        ));
    }
    Ok(value)
}

/// Map a JSON-RPC error object to an [`AcpError`]. The ACP schema's
/// protocol-specific codes: `-32000` auth_required, `-32002`
/// resource_not_found. Unknown codes surface as [`AcpError::ServerError`].
/// Normalize a `session/update` notification's `params` into the flat shape
/// [`SessionUpdate`] deserializes.
///
/// ACP nests the payload: `params.sessionId` + `params.update.{…}`. Older
/// harnesses (and this crate's fixtures) put the update fields directly on
/// `params`. Accept **both**, promoting `sessionId` into the update object, so
/// a spec-shaped agent can never silently produce an empty update — which is
/// exactly how a nested payload used to look: every field defaulted and the
/// update was dropped without an error.
fn update_params(params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return Value::Null;
    };
    let Some(update) = params.get("update").filter(|u| u.is_object()) else {
        return params.clone();
    };
    let mut merged = update.clone();
    if let (Some(map), Some(session_id)) = (merged.as_object_mut(), params.get("sessionId")) {
        map.entry("sessionId").or_insert_with(|| session_id.clone());
    }
    merged
}

/// A JSON-RPC identifier with its wire type preserved. JSON does not coerce
/// `"7"` to `7`; treating those as equal could correlate a response with the
/// wrong in-flight request.
#[derive(Debug, Clone, PartialEq)]
enum JsonRpcId {
    String(String),
    Number(serde_json::Number),
    Null,
}

impl JsonRpcId {
    fn parse(value: &Value) -> Result<Self, AcpError> {
        match value {
            Value::String(value) => Ok(Self::String(value.clone())),
            Value::Number(value) => Ok(Self::Number(value.clone())),
            Value::Null => Ok(Self::Null),
            _ => Err(AcpError::Malformed(
                "JSON-RPC id must be a string, number, or null".into(),
            )),
        }
    }

    fn number(value: u64) -> Self {
        Self::Number(serde_json::Number::from(value))
    }
}

/// The payload carried by a valid JSON-RPC response.
#[derive(Debug)]
enum ResponsePayload {
    Result(Value),
    Error(Value),
}

/// A fully validated inbound JSON-RPC envelope.
#[derive(Debug)]
enum RpcFrame {
    Response {
        id: JsonRpcId,
        payload: ResponsePayload,
    },
    Request {
        method: String,
        id: Value,
    },
    Notification {
        method: String,
    },
}

/// A valid response retained for a different in-flight request. Keeping these
/// separate from interleaved requests/notifications prevents a prompt from
/// repeatedly dequeuing and re-queueing the same non-matching response.
struct PendingResponse {
    id: JsonRpcId,
    payload: ResponsePayload,
}

/// Validate the complete envelope before any method-specific classification.
/// In particular, `method` plus `id` is a request and cannot bypass validation
/// merely because a caller is waiting for a response.
fn decode_inbound_frame(value: &Value) -> Result<RpcFrame, AcpError> {
    let object = value
        .as_object()
        .ok_or_else(|| AcpError::Malformed("JSON-RPC frame must be an object".into()))?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(AcpError::Malformed(
            "JSON-RPC frame is missing jsonrpc=2.0".into(),
        ));
    }
    if let Some(params) = object.get("params") {
        if !params.is_object() && !params.is_array() {
            return Err(AcpError::Malformed(
                "JSON-RPC params must be an object or array".into(),
            ));
        }
    }

    if let Some(method_value) = object.get("method") {
        let method = method_value
            .as_str()
            .filter(|method| !method.is_empty())
            .ok_or_else(|| {
                AcpError::Malformed("JSON-RPC method must be a non-empty string".into())
            })?;
        if object.contains_key("result") || object.contains_key("error") {
            return Err(AcpError::Malformed(
                "JSON-RPC request cannot contain result or error".into(),
            ));
        }
        return if let Some(id) = object.get("id") {
            JsonRpcId::parse(id)?;
            Ok(RpcFrame::Request {
                method: method.to_string(),
                id: id.clone(),
            })
        } else {
            Ok(RpcFrame::Notification {
                method: method.to_string(),
            })
        };
    }

    let id = object
        .get("id")
        .ok_or_else(|| AcpError::Malformed("JSON-RPC response is missing id".into()))?;
    let id = JsonRpcId::parse(id)?;
    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");
    if has_error {
        let error = object.get("error").expect("checked above");
        if !error.is_object()
            || !error
                .get("code")
                .is_some_and(|code| code.is_i64() || code.is_u64())
            || !error.get("message").is_some_and(Value::is_string)
        {
            return Err(AcpError::Malformed(
                "JSON-RPC response error must contain numeric code and string message".into(),
            ));
        }
    }
    let payload = match (has_result, has_error) {
        (true, false) => {
            ResponsePayload::Result(object.get("result").cloned().unwrap_or(Value::Null))
        }
        (false, true) => {
            ResponsePayload::Error(object.get("error").cloned().unwrap_or(Value::Null))
        }
        (false, false) => {
            return Err(AcpError::Malformed(
                "JSON-RPC response must contain exactly one of result or error".into(),
            ));
        }
        (true, true) => {
            return Err(AcpError::Malformed(
                "JSON-RPC response cannot contain both result and error".into(),
            ));
        }
    };
    Ok(RpcFrame::Response { id, payload })
}

fn map_error(err: &Value) -> AcpError {
    let code = err.get("code").and_then(Value::as_i64);
    if code == Some(ERROR_AUTH_REQUIRED) {
        return AcpError::AuthRequired;
    }
    // Some agents use the older -32001 or message-based auth_required signal;
    // treat a message containing "auth_required" as the same condition.
    if let Some(msg) = err.get("message").and_then(Value::as_str) {
        if msg.contains("auth_required") {
            return AcpError::AuthRequired;
        }
    }
    // A provider error carrying an explicit cancellation signal is observed
    // cancellation, unlike a local `AcpCancelHandle::request` call.  ACP does
    // not standardize one cancellation error code, so keep this narrow and
    // fail closed for all other provider errors.
    if let Some(message) = err.get("message").and_then(Value::as_str) {
        let normalized = message.to_ascii_lowercase();
        if normalized.contains("canceled") || normalized.contains("cancelled") {
            return AcpError::Cancelled;
        }
    }
    AcpError::ServerError(err.to_string())
}

/// Choose the option id that realizes a [`PermissionDecision`], refusing when
/// the decision cannot be expressed (FIX-03 / `TASK-CHAN-001`).
///
/// The strict resolver lives in [`crate::permission_bridge`] so the bridge and
/// the wire agree by construction: an unpinned allow is a `once` (never an
/// `allow_always` the user never chose), an explicit id must be one the agent
/// offered, and an unanswerable decision is an error instead of a synthesized
/// `allow_once` / `reject_once`.
fn resolve_option(
    params: &PermissionRequestParams,
    decision: &PermissionDecision,
) -> Result<String, AcpError> {
    crate::permission_bridge::PermissionBridge::new()
        .resolve(params, decision)
        .map_err(|error| AcpError::PermissionUnanswerable(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

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

    impl AcpTransport for MockTransport {
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

    #[derive(Debug, Clone, Copy)]
    enum PromptRaceMode {
        CancelBeforeResponse,
        CancelAfterResponse,
        EofAfterCancel,
    }

    /// A transport that can place a local cancellation immediately before or
    /// after the provider's terminal frame. It lets the unit tests exercise the
    /// race without a real process or wall-clock sleeps.
    struct PromptRaceTransport {
        responses: VecDeque<String>,
        prompt_response: Option<String>,
        sent: Vec<String>,
        handle: AcpCancelHandle,
        mode: PromptRaceMode,
        prompt_seen: bool,
        prompt_session: Option<String>,
        cancel_after_sent: bool,
        alive: bool,
    }

    impl PromptRaceTransport {
        fn new(
            responses: Vec<String>,
            prompt_response: Option<String>,
            mode: PromptRaceMode,
        ) -> Self {
            let requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let notification_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let sender: AcpCancelSender = Arc::new(move |_message| {
                notification_seen.store(true, std::sync::atomic::Ordering::Release);
                Ok(())
            });
            Self {
                responses: responses.into(),
                prompt_response,
                sent: Vec::new(),
                handle: AcpCancelHandle::new(requested, Some(sender)),
                mode,
                prompt_seen: false,
                prompt_session: None,
                cancel_after_sent: false,
                alive: true,
            }
        }
    }

    impl AcpTransport for PromptRaceTransport {
        fn send(&mut self, json: &str) -> io::Result<()> {
            self.sent.push(json.to_string());
            let value: Value = serde_json::from_str(json)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
            if value.get("method").and_then(Value::as_str) == Some("session/prompt") {
                self.prompt_seen = true;
                self.prompt_session = value
                    .get("params")
                    .and_then(|params| params.get("sessionId"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let session_id = self.prompt_session.clone().unwrap_or_default();
                match self.mode {
                    PromptRaceMode::CancelBeforeResponse | PromptRaceMode::EofAfterCancel => {
                        self.handle.request(&session_id)?;
                    }
                    PromptRaceMode::CancelAfterResponse => {
                        self.cancel_after_sent = true;
                    }
                }
            }
            Ok(())
        }

        fn recv(&mut self) -> io::Result<Option<String>> {
            if let Some(response) = self.responses.pop_front() {
                return Ok(Some(response));
            }
            if !self.prompt_seen {
                return Ok(None);
            }
            if self.cancel_after_sent {
                self.cancel_after_sent = false;
                let session_id = self.prompt_session.clone().unwrap_or_default();
                self.handle.request(&session_id)?;
            }
            if matches!(self.mode, PromptRaceMode::EofAfterCancel) {
                return Ok(None);
            }
            Ok(self.prompt_response.take())
        }

        fn is_alive(&mut self) -> bool {
            self.alive
        }

        fn shutdown(&mut self) {
            self.alive = false;
        }

        fn cancellation_handle(&self) -> Option<AcpCancelHandle> {
            Some(self.handle.clone())
        }
    }

    fn result_response(id: u64, result: Value) -> String {
        json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
    }

    fn client_info() -> ClientInfo {
        ClientInfo {
            name: "agentcowork".into(),
            title: "AgentCowork".into(),
            version: "0.1.0".into(),
        }
    }

    fn init_result() -> Value {
        json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "mcpCapabilities": { "http": true, "sse": false }
            },
            "agentInfo": { "name": "claude-acp", "title": "Claude", "version": "0.66.0" },
            "authMethods": []
        })
    }

    fn init_result_with_methods() -> Value {
        json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "mcpCapabilities": { "http": true, "sse": false }
            },
            "agentInfo": { "name": "claude-acp", "title": "Claude", "version": "0.66.0" },
            "authMethods": [
                { "id": "agent-login", "name": "Agent login", "description": "Sign in with your account" }
            ]
        })
    }

    #[test]
    fn initialize_with_no_auth_methods_marks_authenticated() {
        // The documented contract (`auth_methods`): empty ⇒ the agent needs
        // no auth, so the session is authenticated by construction. This is
        // what the mock-CLI E2E (`live_spawn`) relies on.
        let mut t = MockTransport::new(vec![&result_response(1, init_result())]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        assert!(s.auth_methods().is_empty());
        assert!(s.is_authenticated());
    }

    #[test]
    fn initialize_with_advertised_methods_stays_unauthenticated() {
        let mut t = MockTransport::new(vec![&result_response(1, init_result_with_methods())]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        assert_eq!(s.auth_methods().len(), 1);
        assert!(!s.is_authenticated());
    }

    #[test]
    fn initialize_negotiates_version_and_capabilities() {
        let mut t = MockTransport::new(vec![&result_response(1, init_result())]);
        let mut s = AcpSession::new(&mut t);
        let r = s.initialize(client_info()).unwrap();
        assert_eq!(r.protocol_version, 1);
        assert_eq!(s.agent_info().unwrap().name, "claude-acp");
        let first: Value = serde_json::from_str(&t.sent[0]).unwrap();
        assert_eq!(first["method"], "initialize");
        assert_eq!(first["params"]["protocolVersion"], 1);
    }

    #[test]
    fn initialize_withhold_payload_has_fs_terminal_false() {
        // P38 GovernedSession: the default (withhold) path sends fs/terminal
        // false — the Self-contained path, never a Channel-B force.
        let mut t = MockTransport::new(vec![&result_response(1, init_result())]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        let first: Value = serde_json::from_str(&t.sent[0]).unwrap();
        let caps = &first["params"]["clientCapabilities"];
        assert_eq!(caps["fs"]["readTextFile"], false);
        assert_eq!(caps["fs"]["writeTextFile"], false);
        assert_eq!(caps["terminal"], false);
    }

    #[test]
    fn initialize_mediated_payload_advertises_fs_terminal() {
        // P38 GovernedSession Mediated: advertising fs/terminal true makes
        // sandbox-aware agents delegate their file/shell ops to us.
        let mut t = MockTransport::new(vec![&result_response(1, init_result())]);
        let mut s = AcpSession::new(&mut t);
        let caps = ClientCapabilities {
            fs: FsCapabilities {
                read_text_file: true,
                write_text_file: true,
            },
            terminal: true,
            session: Some(SessionCapabilities::config_options_with_boolean()),
        };
        s.initialize_with_caps(client_info(), caps).unwrap();
        let first: Value = serde_json::from_str(&t.sent[0]).unwrap();
        let caps = &first["params"]["clientCapabilities"];
        assert_eq!(caps["fs"]["readTextFile"], true);
        assert_eq!(caps["fs"]["writeTextFile"], true);
        assert_eq!(caps["terminal"], true);
    }

    #[test]
    fn session_new_sets_session_id() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "sess-1" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        let sid = s.session_new("/workspace", vec![]).unwrap();
        assert_eq!(sid, "sess-1");
        assert_eq!(s.session_id(), Some("sess-1"));
    }

    #[test]
    fn session_new_emits_official_stdio_mcp_shape() {
        let server = McpServer::stdio(
            "local",
            "/usr/local/bin/agent-mcp",
            vec!["--stdio".into()],
            vec!["LANG=C".into()],
        );
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "session-new-1" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        assert_eq!(
            s.session_new("/workspace", vec![server]).unwrap(),
            "session-new-1"
        );
        let request: Value = serde_json::from_str(&t.sent[1]).unwrap();
        let descriptor = &request["params"]["mcpServers"][0];
        assert!(descriptor.get("type").is_none());
        assert_eq!(descriptor["command"], "/usr/local/bin/agent-mcp");
        assert_eq!(descriptor["env"][0]["name"], "LANG");
        assert_eq!(descriptor["env"][0]["value"], "C");
        assert!(descriptor.get("url").is_none());
    }

    #[test]
    fn http_mcp_is_refused_before_transport_io_without_capability() {
        let init_values = [
            json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": true,
                    "mcpCapabilities": { "http": false, "sse": false }
                },
                "agentInfo": { "name": "no-http", "title": "No HTTP", "version": "1" },
                "authMethods": []
            }),
            json!({
                "protocolVersion": 1,
                "agentCapabilities": { "loadSession": true },
                "agentInfo": { "name": "no-mcp-capabilities", "title": "No MCP", "version": "1" },
                "authMethods": []
            }),
        ];
        for init in init_values {
            let server = McpServer::http_with_lease_values(
                "shared",
                "http://127.0.0.1:43123/mcp",
                "fixture-token",
            )
            .unwrap();
            let mut t = MockTransport::new(vec![&result_response(1, init)]);
            let mut s = AcpSession::new(&mut t);
            s.initialize(client_info()).unwrap();
            let error = s.session_new("/workspace", vec![server]).unwrap_err();
            assert!(matches!(error, AcpError::McpHttpUnsupported));
            assert_eq!(
                t.sent.len(),
                1,
                "capability refusal must precede session/new"
            );
        }
    }

    #[test]
    fn session_load_empty_and_options_only_results_retain_requested_id() {
        let option = json!({
            "id": "mode",
            "name": "Mode",
            "type": "select",
            "currentValue": "safe",
            "options": [{ "value": "safe", "name": "Safe" }]
        });
        for (result, expected_options) in [
            (Value::Null, 0),
            (json!({}), 0),
            (json!({ "configOptions": [] }), 0),
            (json!({ "configOptions": [option] }), 1),
        ] {
            let mut t = MockTransport::new(vec![
                &result_response(1, init_result()),
                &result_response(2, result),
            ]);
            let mut s = AcpSession::new(&mut t);
            s.initialize(client_info()).unwrap();
            let outcome = s
                .session_load_with_updates("provider-empty", "/workspace", vec![])
                .unwrap();
            assert_eq!(outcome.session_id, "provider-empty");
            assert_eq!(s.session_id(), Some("provider-empty"));
            assert_eq!(s.config_options().len(), expected_options);
        }
    }

    #[test]
    fn session_load_rejects_response_without_result_or_error() {
        let malformed = json!({ "jsonrpc": "2.0", "id": 2 }).to_string();
        let mut t = MockTransport::new(vec![&result_response(1, init_result()), &malformed]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        let error = s
            .session_load("provider-7", "/workspace", vec![])
            .unwrap_err();
        assert!(matches!(error, AcpError::Malformed(_)));
        assert!(s.session_id().is_none());
        assert!(
            s.pending.is_empty(),
            "malformed data is not a valid pending frame"
        );
        assert_eq!(
            s.pending_raw.len(),
            1,
            "the malformed response is retained raw"
        );
        assert!(s.is_quarantined());
    }

    #[test]
    fn load_preserves_a_response_for_another_in_flight_request() {
        let mismatched = result_response(99, json!({ "otherRequest": true }));
        let expected = result_response(2, json!({}));
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &mismatched,
            &expected,
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        let loaded = s.session_load("provider-7", "/workspace", vec![]).unwrap();
        assert_eq!(loaded, "provider-7");
        assert!(s.pending.is_empty());
        assert_eq!(s.pending_responses.len(), 1);
        assert_eq!(s.pending_responses[0].id, JsonRpcId::number(99));
        assert!(matches!(
            &s.pending_responses[0].payload,
            ResponsePayload::Result(result) if result["otherRequest"] == true
        ));
    }

    #[test]
    fn handshake_preserves_a_mismatched_response_for_its_owner() {
        let mismatched = result_response(99, json!({ "otherRequest": true }));
        let init = result_response(1, init_result());
        let mut t = MockTransport::new(vec![&mismatched, &init]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        assert!(s.pending.is_empty());
        assert_eq!(s.pending_responses.len(), 1);
        assert_eq!(s.pending_responses[0].id, JsonRpcId::number(99));
    }

    #[test]
    fn response_correlation_preserves_json_id_type_in_both_directions() {
        let numeric_text = r#"{"jsonrpc":"2.0","id":"7","result":{"owner":"string"}}"#;
        let numeric = r#"{"jsonrpc":"2.0","id":7,"result":{"owner":"number"}}"#;
        let mut t = MockTransport::new(vec![numeric_text, numeric]);
        let mut s = AcpSession::new(&mut t);
        let mut captured = Vec::new();
        let result = s
            .read_response_capture(JsonRpcId::number(7), &mut captured)
            .unwrap();
        assert_eq!(result["owner"], "number");
        assert!(captured.is_empty());
        assert_eq!(s.pending_responses.len(), 1);
        assert_eq!(s.pending_responses[0].id, JsonRpcId::String("7".into()));

        let numeric = r#"{"jsonrpc":"2.0","id":9,"result":{"owner":"number"}}"#;
        let string = r#"{"jsonrpc":"2.0","id":"req-9","result":{"owner":"string"}}"#;
        let mut t = MockTransport::new(vec![numeric, string]);
        let mut s = AcpSession::new(&mut t);
        let mut captured = Vec::new();
        let result = s
            .read_response_capture(JsonRpcId::String("req-9".into()), &mut captured)
            .unwrap();
        assert_eq!(result["owner"], "string");
        assert!(captured.is_empty());
        assert_eq!(s.pending_responses.len(), 1);
        assert_eq!(s.pending_responses[0].id, JsonRpcId::number(9));
    }

    #[test]
    fn wrong_type_response_id_is_rejected_and_never_preserved_as_valid() {
        let malformed = r#"{"jsonrpc":"2.0","id":true,"result":{}}"#;
        let mut t = MockTransport::new(vec![malformed]);
        let mut s = AcpSession::new(&mut t);
        let mut captured = Vec::new();
        let error = s
            .read_response_capture(JsonRpcId::number(7), &mut captured)
            .unwrap_err();
        assert!(matches!(error, AcpError::Malformed(_)));
        assert!(captured.is_empty());
        assert_eq!(s.pending_raw.len(), 1);
        assert!(s.is_quarantined());
        assert!(matches!(
            s.send_frame(r#"{"jsonrpc":"2.0"}"#),
            Err(AcpError::Quarantined(_))
        ));
        assert!(t.sent.is_empty());
    }

    #[test]
    fn oversized_custom_transport_frame_quarantines_the_handle() {
        let oversized = "x".repeat(MAX_ACP_FRAME_BYTES + 1);
        let mut t = MockTransport::new(vec![&oversized]);
        let mut s = AcpSession::new(&mut t);
        assert!(matches!(
            s.initialize(client_info()),
            Err(AcpError::FrameTooLarge)
        ));
        assert!(s.is_quarantined());
        assert!(matches!(
            s.initialize(client_info()),
            Err(AcpError::Quarantined(_))
        ));
        assert_eq!(t.sent.len(), 1, "a poisoned handle emits no retry");
    }

    #[test]
    fn pending_raw_overflow_quarantines_instead_of_retrying() {
        let mut t = MockTransport::new(vec![]);
        let mut s = AcpSession::new(&mut t);
        s.pending = (0..MAX_PENDING_FRAMES)
            .map(|id| json!({ "jsonrpc": "2.0", "id": id, "result": {} }))
            .collect();
        s.pending_raw = std::iter::repeat_n(
            "{\"jsonrpc\":\"2.0\",\"result\":{}}".to_string(),
            MAX_PENDING_FRAMES,
        )
        .collect();
        s.preserve_values(vec![json!({ "jsonrpc": "2.0", "id": 999, "result": {} })]);
        assert!(s.pending_overflow);
        assert!(s.is_quarantined());
        assert!(matches!(
            s.send_frame(r#"{"jsonrpc":"2.0"}"#),
            Err(AcpError::Quarantined(_))
        ));
        assert!(t.sent.is_empty());
    }

    #[test]
    fn method_and_id_frame_must_pass_the_complete_envelope_before_interleaving() {
        let malformed = json!({
            "jsonrpc": "1.0",
            "id": 99,
            "method": "session/update",
            "params": {
                "sessionId": "s1",
                "sessionUpdate": "agent_message_chunk",
                "content": [{ "type": "text", "text": "must-not-bypass" }]
            }
        })
        .to_string();
        let valid = result_response(1, init_result());
        let mut t = MockTransport::new(vec![&malformed, &valid]);
        let mut s = AcpSession::new(&mut t);
        let error = s.initialize(client_info()).unwrap_err();
        assert!(matches!(error, AcpError::Malformed(_)));
        assert!(s.pending.is_empty());
        assert_eq!(s.pending_raw.len(), 1);
        assert!(s.is_quarantined());

        // The poisoned handle cannot silently send another request.
        assert!(matches!(
            s.initialize(client_info()),
            Err(AcpError::Quarantined(_))
        ));
        assert_eq!(t.sent.len(), 1);
    }

    #[test]
    fn prompt_defers_other_responses_without_requeue_loop() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "s1" })),
            &r#"{"jsonrpc":"2.0","id":"other-request","result":{"owner":"other"}}"#.to_string(),
            &result_response(3, json!({ "stopReason": "end_turn" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/workspace", vec![]).unwrap();
        let outcome = s.prompt("go", |_| PermissionDecision::allow()).unwrap();
        assert_eq!(outcome.stop_reason, StopReason::EndTurn);
        assert_eq!(s.pending_responses.len(), 1);
        assert_eq!(
            s.pending_responses[0].id,
            JsonRpcId::String("other-request".into())
        );
    }

    #[test]
    fn valid_method_and_id_session_update_is_a_request_not_a_notification() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "s1" })),
            &json!({
                "jsonrpc": "2.0",
                "id": "agent-request-1",
                "method": "session/update",
                "params": {
                    "sessionId": "s1",
                    "sessionUpdate": "agent_message_chunk",
                    "content": [{ "type": "text", "text": "request-shaped" }]
                }
            })
            .to_string(),
            &result_response(3, json!({ "stopReason": "end_turn" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/workspace", vec![]).unwrap();
        let outcome = s.prompt("go", |_| PermissionDecision::allow()).unwrap();
        assert!(outcome.updates.is_empty());
        let reply: Value = serde_json::from_str(&t.sent[3]).unwrap();
        assert_eq!(reply["id"], "agent-request-1");
        assert_eq!(reply["error"]["code"], -32601);
    }

    #[test]
    fn session_load_rejects_unbounded_or_control_returned_ids() {
        let oversized = "x".repeat(MAX_ACP_SESSION_ID_BYTES + 1);
        for returned in [oversized, "bad\nid".to_string()] {
            let mut t = MockTransport::new(vec![
                &result_response(1, init_result()),
                &result_response(2, json!({ "sessionId": returned })),
            ]);
            let mut s = AcpSession::new(&mut t);
            s.initialize(client_info()).unwrap();
            let error = s
                .session_load("provider-7", "/workspace", vec![])
                .unwrap_err();
            assert!(matches!(error, AcpError::InvalidSessionId));
            assert!(s.session_id().is_none());
        }
    }

    #[test]
    fn cross_session_load_update_is_rejected_and_kept_pending() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "provider-7" })),
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "other-session",
                    "sessionUpdate": "agent_message_chunk",
                    "content": [{ "type": "text", "text": "must-not-replay" }]
                }
            })
            .to_string(),
            &result_response(3, json!({})),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/workspace", vec![]).unwrap();
        let error = s
            .session_load("provider-7", "/workspace", vec![])
            .unwrap_err();
        assert!(matches!(error, AcpError::CrossSessionFrame));
        assert_eq!(s.pending.len(), 1, "rejected frame remains non-lossy");
        assert_eq!(
            s.pending.front().unwrap()["params"]["sessionId"],
            "other-session"
        );
    }

    #[test]
    fn load_config_updates_roll_back_when_a_later_frame_is_invalid() {
        let old_config = json!([{
            "id": "model",
            "name": "Model",
            "type": "select",
            "currentValue": "old-model",
            "options": [{ "value": "old-model", "name": "Old" }]
        }]);
        let staged_config = json!([{
            "id": "model",
            "name": "Model",
            "type": "select",
            "currentValue": "staged-model",
            "options": [{ "value": "staged-model", "name": "Staged" }]
        }]);
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(
                2,
                json!({ "sessionId": "old-session", "configOptions": old_config }),
            ),
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "new-session",
                    "update": {
                        "sessionUpdate": "config_option_update",
                        "configOptions": staged_config
                    }
                }
            })
            .to_string(),
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "other-session",
                    "sessionUpdate": "agent_message_chunk",
                    "content": [{ "type": "text", "text": "invalid-tail" }]
                }
            })
            .to_string(),
            &result_response(3, json!({})),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/workspace", vec![]).unwrap();
        let error = s
            .session_load("new-session", "/workspace", vec![])
            .unwrap_err();
        assert!(matches!(error, AcpError::CrossSessionFrame));
        assert_eq!(s.session_id(), Some("old-session"));
        assert_eq!(s.config_options()[0].current_value, json!("old-model"));
        assert_eq!(s.pending.len(), 2, "both captured frames remain non-lossy");
    }

    #[test]
    fn load_parse_failure_preserves_update_for_the_next_turn() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "provider-7" })),
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "provider-7",
                    "sessionUpdate": "agent_message_chunk",
                    "content": [{ "type": "text", "text": "queued-before-error" }]
                }
            })
            .to_string(),
            // Invalid options shape makes the result decode fail after the
            // interleaved update has already been captured.
            &result_response(3, json!({ "configOptions": "not-an-array" })),
            &result_response(4, json!({ "stopReason": "end_turn" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/workspace", vec![]).unwrap();
        assert!(s.session_load("provider-7", "/workspace", vec![]).is_err());
        let outcome = s
            .prompt("continue", |_| PermissionDecision::allow())
            .unwrap();
        assert!(outcome.updates.iter().any(|update| {
            update
                .content
                .iter()
                .any(|block| block.text == "queued-before-error")
        }));
    }

    #[test]
    fn session_load_refuses_without_negotiated_capability() {
        let init_without_load = json!({
            "protocolVersion": 1,
            "agentCapabilities": { "loadSession": false },
            "agentInfo": { "name": "no-load", "title": "No load", "version": "1" },
            "authMethods": []
        });
        let mut t = MockTransport::new(vec![&result_response(1, init_without_load)]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        let error = s
            .session_load("provider-1", "/workspace", vec![])
            .unwrap_err();
        assert!(matches!(error, AcpError::SessionLoadUnsupported));
        assert!(s.session_id().is_none());
        drop(s);
        assert_eq!(
            t.sent.len(),
            1,
            "refusal must not put session/load on the wire"
        );
    }

    #[test]
    fn session_load_sends_absolute_cwd_replays_updates_and_keeps_requested_id() {
        let server = McpServer::http_with_lease_values(
            "shared",
            "http://127.0.0.1:43123/mcp",
            "fixture-token",
        )
        .unwrap();
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "provider-7",
                    "sessionUpdate": "agent_message_chunk",
                    "content": [{ "type": "text", "text": "restored" }]
                }
            })
            .to_string(),
            &result_response(2, json!({ "sessionId": "provider-7" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        let outcome = s
            .session_load_with_updates("provider-7", ".", vec![server.clone()])
            .unwrap();
        assert_eq!(outcome.session_id, "provider-7");
        assert_eq!(outcome.updates.len(), 1);
        assert_eq!(outcome.updates[0].content[0].text, "restored");
        assert_eq!(s.session_id(), Some("provider-7"));
        drop(s);

        let request: Value = serde_json::from_str(&t.sent[1]).unwrap();
        assert_eq!(request["method"], "session/load");
        assert_eq!(request["params"]["sessionId"], "provider-7");
        assert!(std::path::Path::new(request["params"]["cwd"].as_str().unwrap()).is_absolute());
        assert_eq!(request["params"]["mcpServers"][0]["name"], "shared");
        assert_eq!(
            request["params"]["mcpServers"][0]["headers"][0]["name"],
            "Authorization"
        );
        assert_eq!(
            request["params"]["mcpServers"][0]["headers"][0]["value"],
            "Bearer fixture-token"
        );
        assert!(request["params"]["mcpServers"][0].get("env").is_none());
    }

    #[test]
    fn session_load_accepts_a_bounded_returned_extension_id() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "different-provider" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        let returned = s.session_load("provider-7", "/workspace", vec![]).unwrap();
        assert_eq!(returned, "different-provider");
        assert_eq!(s.session_id(), Some("different-provider"));
        drop(s);
        let sent: Vec<Value> = t
            .sent
            .iter()
            .map(|message| serde_json::from_str(message).unwrap())
            .collect();
        assert_eq!(
            sent.iter()
                .filter(|message| message["method"] == "session/new")
                .count(),
            0
        );
        assert_eq!(sent[1]["method"], "session/load");
        assert_eq!(sent[1]["params"]["sessionId"], "provider-7");
    }

    #[test]
    fn load_notifications_are_replayed_into_the_next_prompt_outcome() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "provider-8",
                    "sessionUpdate": "agent_message_chunk",
                    "content": [{ "type": "text", "text": "load-update" }]
                }
            })
            .to_string(),
            &result_response(2, json!({ "sessionId": "provider-8" })),
            &result_response(3, json!({ "stopReason": "end_turn" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_load("provider-8", "/workspace", vec![]).unwrap();
        let outcome = s
            .prompt("continue", |_| PermissionDecision::allow())
            .unwrap();
        assert!(outcome.updates.iter().any(|update| {
            update
                .content
                .iter()
                .any(|block| block.text == "load-update")
        }));
    }

    #[test]
    fn replay_updates_survive_prompt_failure_and_are_consumed_once_on_retry() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "provider-replay",
                    "sessionUpdate": "agent_message_chunk",
                    "content": [{ "type": "text", "text": "load-replay" }]
                }
            })
            .to_string(),
            &result_response(2, json!({})),
            &json!({
                "jsonrpc": "2.0", "id": 3,
                "error": { "code": -32010, "message": "temporary provider failure" }
            })
            .to_string(),
            &result_response(4, json!({ "stopReason": "end_turn" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_load("provider-replay", "/workspace", vec![])
            .unwrap();
        let error = s
            .prompt("first", |_| PermissionDecision::allow())
            .unwrap_err();
        assert!(matches!(error, AcpError::ServerError(_)));
        assert_eq!(s.replayed_session_updates().len(), 1);

        let outcome = s.prompt("retry", |_| PermissionDecision::allow()).unwrap();
        assert!(outcome.updates.iter().any(|update| {
            update
                .content
                .iter()
                .any(|block| block.text == "load-replay")
        }));
        assert!(s.replayed_session_updates().is_empty());
    }

    #[test]
    fn replay_updates_survive_local_cancellation_with_unknown_provider_outcome() {
        let transport = PromptRaceTransport::new(
            vec![
                result_response(1, init_result()),
                json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": "provider-race",
                        "sessionUpdate": "agent_message_chunk",
                        "content": [{ "type": "text", "text": "load-replay" }]
                    }
                })
                .to_string(),
                result_response(2, json!({})),
            ],
            None,
            PromptRaceMode::EofAfterCancel,
        );
        let mut s = AcpSession::new(transport);
        s.initialize(client_info()).unwrap();
        s.session_load("provider-race", "/workspace", vec![])
            .unwrap();
        let error = s
            .prompt("cancel me", |_| PermissionDecision::allow())
            .unwrap_err();
        assert!(matches!(error, AcpError::CancellationOutcomeUnknown));
        assert_eq!(s.replayed_session_updates().len(), 1);
    }

    #[test]
    fn late_local_cancellation_cannot_rewrite_a_provider_completed_turn() {
        let transport = PromptRaceTransport::new(
            vec![
                result_response(1, init_result()),
                result_response(2, json!({})),
            ],
            Some(result_response(3, json!({ "stopReason": "end_turn" }))),
            PromptRaceMode::CancelAfterResponse,
        );
        let mut s = AcpSession::new(transport);
        s.initialize(client_info()).unwrap();
        s.session_load("provider-race", "/workspace", vec![])
            .unwrap();
        let outcome = s
            .prompt("complete", |_| PermissionDecision::allow())
            .unwrap();
        assert_eq!(outcome.stop_reason, StopReason::EndTurn);
        assert!(!s.cancellation_handle().is_requested());
    }

    #[test]
    fn local_cancellation_before_provider_completion_does_not_force_cancelled() {
        let transport = PromptRaceTransport::new(
            vec![
                result_response(1, init_result()),
                result_response(2, json!({})),
            ],
            Some(result_response(3, json!({ "stopReason": "end_turn" }))),
            PromptRaceMode::CancelBeforeResponse,
        );
        let mut s = AcpSession::new(transport);
        s.initialize(client_info()).unwrap();
        s.session_load("provider-race", "/workspace", vec![])
            .unwrap();
        let outcome = s
            .prompt("cancel then complete", |_| PermissionDecision::allow())
            .unwrap();
        assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn provider_observed_cancellation_is_the_only_durable_cancelled_result() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "provider-cancel" })),
            &result_response(3, json!({ "stopReason": "cancelled" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_load("provider-cancel", "/workspace", vec![])
            .unwrap();
        let outcome = s.prompt("cancel", |_| PermissionDecision::allow()).unwrap();
        assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    }

    #[test]
    fn session_new_captures_the_agents_own_config_options() {
        // P60 — the agent owns this vocabulary (model/mode/reasoning). We keep
        // the complete list verbatim; the native provider catalog never enters
        // here. An agent that omits `configOptions` is still a valid session
        // (covered by `session_new_sets_session_id`).
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(
                2,
                json!({
                    "sessionId": "sess-1",
                    "configOptions": [{
                        "id": "model",
                        "name": "Model",
                        "category": "model",
                        "type": "select",
                        "currentValue": "model-1",
                        "options": [
                            { "value": "model-1", "name": "Model 1" },
                            { "value": "model-2", "name": "Model 2" },
                        ]
                    }]
                }),
            ),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/workspace", vec![]).unwrap();

        let options = s.config_options();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].id, "model");
        assert_eq!(options[0].category.as_deref(), Some("model"));
        assert_eq!(options[0].current_value, json!("model-1"));
        assert_eq!(options[0].options.len(), 2);
    }

    #[test]
    fn set_config_option_sends_the_documented_shape_and_stores_the_return() {
        let updated = json!([{
            "id": "model",
            "name": "Model",
            "type": "select",
            "currentValue": "model-2",
            "options": [
                { "value": "model-1", "name": "Model 1" },
                { "value": "model-2", "name": "Model 2" },
            ]
        }]);
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "sess-1" })),
            &result_response(3, json!({ "configOptions": updated })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/w", vec![]).unwrap();

        let options = s.set_config_option("model", json!("model-2")).unwrap();
        assert_eq!(options[0].current_value, json!("model-2"));
        // The response is the complete configuration state, so the session's
        // view must be replaced rather than patched.
        assert_eq!(s.config_options(), options.as_slice());

        let sent: Value = serde_json::from_str(&t.sent[2]).unwrap();
        assert_eq!(sent["method"], "session/set_config_option");
        assert_eq!(sent["params"]["sessionId"], "sess-1");
        assert_eq!(sent["params"]["configId"], "model");
        assert_eq!(sent["params"]["value"], "model-2");
    }

    #[test]
    fn config_option_update_notification_replaces_the_stored_list() {
        // The agent may re-select on its own (e.g. a model fallback mid-turn).
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "s1" })),
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "s1",
                    "update": {
                        "sessionUpdate": "config_option_update",
                        "configOptions": [{
                            "id": "model", "name": "Model", "type": "select",
                            "currentValue": "fallback-model", "options": []
                        }]
                    }
                }
            })
            .to_string(),
            &result_response(3, json!({ "stopReason": "end_turn" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/w", vec![]).unwrap();
        s.prompt("hi", |_p| PermissionDecision::allow()).unwrap();

        let options = s.config_options();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].current_value, json!("fallback-model"));
    }

    #[test]
    fn spec_nested_session_update_is_parsed_not_dropped() {
        // The protocol nests the payload under `params.update`. Before the
        // normalization fix this deserialized into an all-defaults
        // SessionUpdate, so a real agent's tool calls/commands vanished.
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "s1" })),
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "s1",
                    "update": {
                        "sessionUpdate": "available_commands_update",
                        "availableCommands": [
                            { "name": "review", "description": "Review the diff" }
                        ]
                    }
                }
            })
            .to_string(),
            &result_response(3, json!({ "stopReason": "end_turn" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/w", vec![]).unwrap();
        let outcome = s.prompt("hi", |_p| PermissionDecision::allow()).unwrap();

        assert_eq!(outcome.updates.len(), 1);
        let u = &outcome.updates[0];
        assert!(u.is_available_commands_update());
        // `sessionId` is promoted from the envelope so the update is keyed.
        assert_eq!(u.session_id, "s1");
        assert_eq!(u.available_commands.len(), 1);
        assert_eq!(u.available_commands[0].name, "review");
    }

    #[test]
    fn prompt_drives_turn_and_answers_permission() {
        // Sequence after initialize+session/new (ids 1,2): prompt = id 3.
        // Inbound: a session/update notification, a request_permission (id 99),
        // then the prompt response (id 3).
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "s1" })),
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": { "sessionId": "s1", "sessionUpdate": "tool_call", "toolCallId": "tc1", "title": "Edit", "kind": "edit" }
            })
            .to_string(),
            &json!({
                "jsonrpc": "2.0", "id": 99, "method": "session/request_permission",
                "params": {
                    "sessionId": "s1",
                    "toolCall": { "toolCallId": "tc1", "title": "Edit a.rs", "kind": "edit" },
                    "options": [ { "optionId": "allow-once", "kind": "allow_once", "label": "Allow once" } ]
                }
            })
            .to_string(),
            &result_response(3, json!({ "stopReason": "end_turn" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/w", vec![]).unwrap();

        let outcome = s
            .prompt("fix the bug", |_p| PermissionDecision::allow())
            .unwrap();
        assert_eq!(outcome.stop_reason, StopReason::EndTurn);
        assert_eq!(outcome.updates.len(), 1);
        assert!(outcome.updates[0].is_tool_call());
        assert_eq!(outcome.permissions.len(), 1);
        assert_eq!(outcome.permission_decisions[0], PermissionDecision::allow());

        // The permission reply selected the offered allow-once option.
        let replies: Vec<Value> = t
            .sent
            .iter()
            .map(|s| serde_json::from_str(s).unwrap())
            .filter(|v: &Value| {
                v.get("method").and_then(Value::as_str).is_none()
                    && v.get("id").is_some()
                    && v.get("result").is_some()
            })
            .collect();
        let perm_reply = replies
            .iter()
            .find(|v| v["result"]["outcome"].is_object())
            .expect("permission reply present");
        assert_eq!(perm_reply["result"]["outcome"]["optionId"], "allow-once");
    }

    #[test]
    fn cross_session_permission_request_is_rejected_before_callback() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "s1" })),
            &json!({
                "jsonrpc": "2.0", "id": 99, "method": "session/request_permission",
                "params": {
                    "sessionId": "other-session",
                    "toolCall": { "toolCallId": "tc1", "title": "cross-session" },
                    "options": []
                }
            })
            .to_string(),
            &result_response(3, json!({ "stopReason": "end_turn" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/workspace", vec![]).unwrap();
        let mut callback_count = 0;
        let error = s
            .prompt("should not authorize", |_| {
                callback_count += 1;
                PermissionDecision::allow()
            })
            .unwrap_err();
        assert!(matches!(error, AcpError::CrossSessionFrame));
        assert_eq!(callback_count, 0);
        assert_eq!(s.pending.len(), 1);
    }

    #[test]
    fn prompt_deny_uses_reject_option() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "s1" })),
            &json!({
                "jsonrpc": "2.0", "id": 99, "method": "session/request_permission",
                "params": {
                    "sessionId": "s1",
                    "toolCall": { "toolCallId": "tc1", "title": "rm", "kind": "delete" },
                    "options": [
                        { "optionId": "allow-once", "kind": "allow_once", "label": "Allow" },
                        { "optionId": "reject-once", "kind": "reject_once", "label": "Reject" }
                    ]
                }
            })
            .to_string(),
            &result_response(3, json!({ "stopReason": "refusal" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/w", vec![]).unwrap();
        let outcome = s
            .prompt("delete it", |_p| PermissionDecision::deny())
            .unwrap();
        assert_eq!(outcome.stop_reason, StopReason::Refusal);
        let reply: Value = serde_json::from_str(&t.sent[t.sent.len() - 1]).unwrap();
        assert_eq!(reply["result"]["outcome"]["optionId"], "reject-once");
    }

    // ---- FIX-03: the permission bridge never invents an option ------------

    /// The inbound frames for one driven turn whose permission request carries
    /// `options` (the verbatim JSON array the agent offers).
    fn permission_turn(options: &str) -> Vec<String> {
        vec![
            result_response(1, init_result()),
            result_response(2, json!({ "sessionId": "s1" })),
            json!({
                "jsonrpc": "2.0", "id": 99, "method": "session/request_permission",
                "params": {
                    "sessionId": "s1",
                    "toolCall": { "toolCallId": "tc1", "title": "Edit a.rs", "kind": "edit" },
                    "options": serde_json::from_str::<Value>(options).expect("options json")
                }
            })
            .to_string(),
            result_response(3, json!({ "stopReason": "end_turn" })),
        ]
    }

    /// A handshaken session over a scripted permission turn.
    fn permission_session(options: &str) -> AcpSession<MockTransport> {
        let frames = permission_turn(options);
        let borrowed: Vec<&str> = frames.iter().map(String::as_str).collect();
        let mut s = AcpSession::new(MockTransport::new(borrowed));
        s.initialize(client_info()).unwrap();
        s.session_new("/w", vec![]).unwrap();
        s
    }

    /// The last frame the session sent — the permission reply.
    fn last_reply(s: &AcpSession<MockTransport>) -> Value {
        serde_json::from_str(s.transport.sent.last().expect("a reply")).unwrap()
    }

    #[test]
    fn an_unanswerable_permission_is_refused_with_a_typed_error_not_a_synthesized_option() {
        // The agent offers no reject option, so a denial cannot be expressed.
        // The turn must fail closed: no `result` with an invented id, and the
        // agent gets a typed JSON-RPC error instead of a silent allow.
        let mut s = permission_session(
            r#"[ { "optionId": "allow-once", "kind": "allow_once", "label": "Allow" } ]"#,
        );
        let error = s
            .prompt("deny it", |_p| PermissionDecision::deny())
            .expect_err("must fail closed");
        assert!(
            matches!(&error, AcpError::PermissionUnanswerable(message) if message.contains("reject")),
            "unexpected error: {error}"
        );
        let reply = last_reply(&s);
        assert_eq!(reply["id"], 99);
        assert_eq!(reply["error"]["code"], ERROR_PERMISSION_UNANSWERABLE);
        assert!(
            reply.get("result").is_none(),
            "a fail-closed refusal must not carry an outcome: {reply}"
        );
        // The transport is quarantined: the session cannot keep running after a
        // protocol-level refusal.
        assert!(s.is_quarantined());
    }

    #[test]
    fn an_unpinned_allow_selects_allow_once_even_when_allow_always_is_listed_first() {
        let mut s = permission_session(
            r#"[
                { "optionId": "allow-always", "kind": "allow_always", "label": "Always" },
                { "optionId": "allow-once", "kind": "allow_once", "label": "Once" },
                { "optionId": "reject-once", "kind": "reject_once", "label": "No" }
            ]"#,
        );
        s.prompt("do it", |_p| PermissionDecision::allow())
            .expect("answered");
        assert_eq!(
            last_reply(&s)["result"]["outcome"]["optionId"],
            "allow-once"
        );
    }

    #[test]
    fn a_foreign_option_id_is_refused_rather_than_echoed() {
        let mut s = permission_session(
            r#"[ { "optionId": "allow-once", "kind": "allow_once", "label": "Allow" } ]"#,
        );
        let error = s
            .prompt("sneak one in", |_p| PermissionDecision::Allow {
                option_id: Some("approve-everything".into()),
            })
            .expect_err("must fail closed");
        assert!(
            matches!(&error, AcpError::PermissionUnanswerable(message) if message.contains("approve-everything")),
            "unexpected error: {error}"
        );
        let reply = last_reply(&s);
        assert!(
            reply.get("result").is_none(),
            "a foreign option id must never reach the agent: {reply}"
        );
    }

    #[test]
    fn authenticate_agent_method_succeeds() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result_with_methods()),
            &result_response(2, json!({})),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        assert!(!s.is_authenticated());

        let r = s.authenticate("agent-login").unwrap();
        assert!(r.url.is_none());
        assert!(s.is_authenticated());

        // The request carried the advertised method id.
        let req: Value = serde_json::from_str(&t.sent[1]).unwrap();
        assert_eq!(req["method"], "authenticate");
        assert_eq!(req["params"]["methodId"], "agent-login");
    }

    #[test]
    fn authenticate_url_method_returns_url_and_waits() {
        let mut t = MockTransport::new(vec![
            &result_response(
                1,
                json!({
                    "protocolVersion": 1,
                    "agentCapabilities": { "loadSession": true },
                    "agentInfo": { "name": "claude-acp", "title": "Claude", "version": "0.66.0" },
                    "authMethods": [
                        { "id": "agent-login", "name": "Agent login", "type": "url", "description": "Open a browser" }
                    ]
                }),
            ),
            // url-type: first call returns the browser URL, not yet authed.
            &result_response(
                2,
                json!({ "url": "https://agent.example.com/login?code=abc" }),
            ),
            // after the user completes login, the second call returns {}.
            &result_response(3, json!({})),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();

        let r = s.authenticate("agent-login").unwrap();
        assert_eq!(
            r.url.as_deref(),
            Some("https://agent.example.com/login?code=abc")
        );
        assert!(!s.is_authenticated(), "url flow not complete until re-auth");

        let r = s.authenticate("agent-login").unwrap();
        assert!(r.url.is_none());
        assert!(s.is_authenticated());
    }

    #[test]
    fn auth_required_error_is_detected_on_session_new() {
        // initialize ok; session/new fails with the auth_required code.
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &json!({
                "jsonrpc": "2.0", "id": 2,
                "error": { "code": -32000, "message": "Authentication required" }
            })
            .to_string(),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        assert!(matches!(
            s.session_new("/w", vec![]),
            Err(AcpError::AuthRequired)
        ));
    }

    #[test]
    fn auth_required_message_fallback_detected() {
        // Older agents may use -32001 + a message mentioning auth_required.
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &json!({
                "jsonrpc": "2.0", "id": 2,
                "error": { "code": -32001, "message": "auth_required: sign in first" }
            })
            .to_string(),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        assert!(matches!(
            s.session_new("/w", vec![]),
            Err(AcpError::AuthRequired)
        ));
    }

    #[test]
    fn logout_sends_request_and_clears_auth() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result_with_methods()),
            &result_response(2, json!({})),
            &result_response(3, json!({})),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.authenticate("agent-login").unwrap();
        assert!(s.is_authenticated());

        s.logout().unwrap();
        assert!(!s.is_authenticated());
        let req: Value = serde_json::from_str(&t.sent[2]).unwrap();
        assert_eq!(req["method"], "logout");
    }

    #[test]
    fn initialize_exposes_advertised_auth_methods() {
        let mut t = MockTransport::new(vec![&result_response(
            1,
            json!({
                "protocolVersion": 1,
                "agentCapabilities": { "auth": { "logout": {} } },
                "agentInfo": { "name": "claude-acp", "title": "Claude", "version": "1" },
                "authMethods": [
                    { "id": "agent-login", "name": "Agent login", "description": "Sign in with your account" },
                    { "id": "browser", "name": "Browser login", "type": "url", "description": "Open a browser" }
                ]
            }),
        )]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        let methods = s.auth_methods();
        assert_eq!(methods.len(), 2);
        assert_eq!(methods[0].id, "agent-login");
        assert_eq!(methods[1].r#type, Some(AuthMethodType::Url));
    }

    #[test]
    fn protocol_mismatch_is_surfaced() {
        let mut t = MockTransport::new(vec![&result_response(1, json!({ "protocolVersion": 2 }))]);
        let mut s = AcpSession::new(&mut t);
        assert!(matches!(
            s.initialize(client_info()),
            Err(AcpError::ProtocolMismatch(2))
        ));
    }

    #[test]
    fn custom_transport_without_shutdown_hook_fails_closed() {
        let requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = AcpCancelHandle::new(requested, None);
        let error = handle.request_shutdown().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn prompt_before_session_fails() {
        let mut t = MockTransport::new(vec![&result_response(1, init_result())]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        assert!(matches!(
            s.prompt("x", |_| PermissionDecision::allow()),
            Err(AcpError::NotReady)
        ));
    }

    #[test]
    fn idle_cancel_is_fenced_and_does_not_emit_a_stale_notification() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "s1" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/w", vec![]).unwrap();
        s.cancel().unwrap();
        assert!(s.cancellation_handle().is_requested());
        drop(s);
        assert!(!t.sent.iter().any(
            |message| serde_json::from_str::<Value>(message).unwrap()["method"] == "session/cancel"
        ));
    }

    #[test]
    fn host_cancellation_is_scoped_and_resettable_per_provider_session() {
        let mut t = MockTransport::new(vec![
            &result_response(1, init_result()),
            &result_response(2, json!({ "sessionId": "provider-1" })),
            &result_response(3, json!({ "stopReason": "end_turn" })),
            &result_response(4, json!({ "stopReason": "end_turn" })),
        ]);
        let mut s = AcpSession::new(&mut t);
        s.initialize(client_info()).unwrap();
        s.session_new("/w", vec![]).unwrap();
        let cancel = s.cancellation_handle();
        cancel.request("provider-1").unwrap();
        assert!(cancel.is_requested());

        // A local request is not provider evidence. The new turn fence drops
        // this stale request before the prompt is sent, and the provider's
        // completed response remains successful.
        let outcome = s
            .prompt("blocked", |_| PermissionDecision::allow())
            .unwrap();
        assert_eq!(outcome.stop_reason, StopReason::EndTurn);
        assert!(!cancel.is_requested());

        s.reset_cancellation();
        assert!(s.prompt("next", |_| PermissionDecision::allow()).is_ok());
        assert!(!cancel.is_requested());
    }

    #[test]
    fn delayed_old_cancel_write_cannot_overlap_a_new_turn() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sender_calls = Arc::clone(&calls);
        let closure_release = Arc::clone(&release_rx);
        let sender: AcpCancelSender = Arc::new(move |_message| {
            sender_calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            entered_tx.send(()).unwrap();
            closure_release.lock().unwrap().recv().unwrap();
            Ok(())
        });
        let requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = AcpCancelHandle::new(requested, Some(sender));
        let old_turn = handle.begin_turn();

        let request_handle = handle.clone();
        let request_thread = thread::spawn(move || request_handle.request("old-session"));
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let transition_handle = handle.clone();
        let (completed_tx, completed_rx) = mpsc::channel();
        let (new_ready_tx, new_ready_rx) = mpsc::channel();
        let transition_thread = thread::spawn(move || {
            let mut old_turn = old_turn;
            old_turn.complete();
            completed_tx.send(()).unwrap();
            let new_turn = transition_handle.begin_turn();
            new_ready_tx.send(()).unwrap();
            new_turn
        });

        completed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the old turn must not wait for a blocked cancellation pipe write");
        assert!(
            new_ready_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "a later turn must wait until the old cancellation write is ordered"
        );
        release_tx.send(()).unwrap();
        request_thread.join().unwrap().unwrap();
        let new_turn = transition_thread.join().unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 1);
        assert!(!handle.is_requested());
        drop(new_turn);
    }

    #[test]
    fn expired_transport_deadline_is_not_reused_by_a_new_turn() {
        let requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = AcpCancelHandle::new(requested, None);
        let mut old_turn = handle.begin_turn();
        handle.mark_requested().unwrap();
        let old_generation = handle.cancellation_generation().unwrap();
        let now = Instant::now();
        let mut deadline = CancellationDeadline {
            active: Some((
                old_generation,
                now.checked_sub(Duration::from_secs(1)).unwrap(),
            )),
        };
        assert!(deadline.observe(Some(old_generation), now));
        old_turn.complete();

        let mut new_turn = handle.begin_turn();
        handle.mark_requested().unwrap();
        let new_generation = handle.cancellation_generation().unwrap();
        assert_ne!(old_generation, new_generation);
        assert!(
            !deadline.observe(Some(new_generation), now),
            "a fresh cancellation receives a fresh grace window"
        );
        new_turn.complete();
        assert!(!deadline.observe(handle.cancellation_generation(), now));
    }

    #[test]
    fn hermetic_parent_policy_allows_only_launch_runtime_variables() {
        let vars = vec![
            (OsString::from("PATH"), OsString::from("/safe/bin")),
            (OsString::from("LANG"), OsString::from("C.UTF-8")),
            (OsString::from("LD_PRELOAD"), OsString::from("/tmp/evil.so")),
            (
                OsString::from("DYLD_INSERT_LIBRARIES"),
                OsString::from("evil"),
            ),
            (OsString::from("BASH_ENV"), OsString::from("evil")),
            (
                OsString::from("HTTP_PROXY"),
                OsString::from("http://proxy.invalid"),
            ),
            (
                OsString::from("EVERYAIOS_HOST_TOKEN"),
                OsString::from("hidden"),
            ),
            // The current spelling is blocked by the same rule; the retired
            // spelling stays in this test so the legacy token never leaks
            // back into a child environment (DEC-053).
            (
                OsString::from("AGENTCOWORK_HOST_TOKEN"),
                OsString::from("hidden"),
            ),
            (OsString::from("UNKNOWN_FLAG"), OsString::from("hidden")),
        ];

        let selected = hermetic_parent_environment(vars);
        let names: Vec<String> = selected
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["LANG".to_string(), "PATH".to_string()]);
    }

    #[test]
    fn explicit_child_overrides_use_only_reviewed_non_secret_names() {
        let prepared = prepare_child_environment(&[
            ("ANTHROPIC_BASE_URL", "https://api.example.test/v1"),
            ("ANTHROPIC_MODEL", "test-model"),
        ])
        .unwrap();
        let selected: BTreeMap<_, _> = prepared
            .into_iter()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert_eq!(
            selected.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("https://api.example.test/v1")
        );
        assert_eq!(
            selected.get("ANTHROPIC_MODEL").map(String::as_str),
            Some("test-model")
        );

        for name in [
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "BASH_ENV",
            "NODE_OPTIONS",
            "HTTP_PROXY",
            "NO_PROXY",
            "DATABASE_URL",
            "KUBECONFIG",
            "EVERYAIOS_HOST_TOKEN",
            "AGENTCOWORK_HOST_TOKEN",
            "ANTHROPIC_API_KEY",
            "PATH",
            "UNKNOWN_AGENT_FLAG",
        ] {
            assert!(matches!(
                prepare_child_environment(&[(name, "must-not-launch")]),
                Err(ProcessTransportError::DisallowedEnvVar { .. })
            ));
        }
    }

    #[test]
    fn explicit_values_reject_control_characters_and_unsafe_urls() {
        for (name, value) in [
            ("ANTHROPIC_MODEL", "model\nINJECTED=1"),
            ("ANTHROPIC_MODEL", "https://attacker.invalid/"),
            ("ANTHROPIC_MODEL", "sk-live-value"),
            (
                "ANTHROPIC_BASE_URL",
                "https://user:password@example.invalid/v1",
            ),
            (
                "ANTHROPIC_BASE_URL",
                "https://example.invalid/v1?token=exfiltrate",
            ),
            (
                "ANTHROPIC_BASE_URL",
                "http://169.254.169.254/latest/meta-data/",
            ),
            ("ANTHROPIC_BASE_URL", "http://192.168.1.10/v1"),
        ] {
            let error = prepare_child_environment(&[(name, value)])
                .expect_err("unsafe reviewed value must be refused");
            assert!(matches!(
                error,
                ProcessTransportError::InvalidEnvValue { .. }
            ));
            assert!(!error.to_string().contains("password"));
            assert!(!error.to_string().contains("exfiltrate"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_parent_names_are_deduplicated_case_insensitively() {
        let selected = hermetic_parent_environment(vec![
            (OsString::from("Path"), OsString::from("first")),
            (OsString::from("PATH"), OsString::from("second")),
        ]);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0, OsString::from("PATH"));
        assert_eq!(selected[0].1, OsString::from("first"));
    }

    struct ScopedEnv {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl ScopedEnv {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(name);
            // SAFETY: the test lock below serializes this test's mutations, and
            // the sentinel names are unique to this test binary.
            unsafe { std::env::set_var(name, value) };
            Self { name, previous }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: paired with `set`; the lock remains held by the test.
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(self.name, value),
                    None => std::env::remove_var(self.name),
                }
            }
        }
    }

    /// Live proof that `env_clear` is structural without serializing the full
    /// environment: the child reports only selected allow/deny observations.
    #[cfg(unix)]
    #[test]
    fn spawned_child_receives_allowlisted_parent_env_and_no_parent_secrets() {
        static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let _lang = ScopedEnv::set("LANG", "agentcowork-acp-hermetic-test");
        let _secret = ScopedEnv::set("AGENTCOWORK_ACP_PARENT_API_KEY", "secret-sentinel");
        let _control = ScopedEnv::set("AGENTCOWORK_ACP_NON_ALLOWED_CONTROL", "control-sentinel");
        let dir = std::env::temp_dir().join(format!("acp-env-clear-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let out = dir.join("selected-observations.txt");
        let script = format!(
            "printf 'lang=%s\\npath=%s\\nsecret=%s\\ncontrol=%s\\n' \\\n             \"${{LANG-}}\" \\\n             \"$([ -n \"${{PATH-}}\" ] && echo present || echo absent)\" \\\n             \"$([ -n \"${{AGENTCOWORK_ACP_PARENT_API_KEY+x}}\" ] && echo present || echo absent)\" \\\n             \"$([ -n \"${{AGENTCOWORK_ACP_NON_ALLOWED_CONTROL+x}}\" ] && echo present || echo absent)\" > {}",
            out.display()
        );
        let mut transport = ProcessTransport::spawn("sh", &["-c", script.as_str()], &[]).unwrap();
        let mut selected = String::new();
        for _ in 0..200 {
            if let Ok(value) = std::fs::read_to_string(&out) {
                if !value.is_empty() {
                    selected = value;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        transport.shutdown();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(selected.contains("lang=agentcowork-acp-hermetic-test"));
        assert!(selected.contains("path=present"));
        assert!(selected.contains("secret=absent"));
        assert!(selected.contains("control=absent"));
        assert!(!selected.contains("secret-sentinel"));
        assert!(!selected.contains("control-sentinel"));
    }

    /// Real process smoke test: spawn `cat` (echoes stdin) over the newline
    /// transport and verify one frame round-trips.
    #[cfg(unix)]
    #[test]
    fn process_transport_roundtrips_through_stdio() {
        let mut t = ProcessTransport::spawn("cat", &[], &[]).unwrap();
        assert!(t.is_alive());
        t.send(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
            .unwrap();
        let echoed = t.recv().unwrap().expect("cat echoes");
        assert_eq!(echoed, r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
        t.shutdown();
        assert!(!t.is_alive());
    }

    #[cfg(unix)]
    #[test]
    fn process_transport_partial_frame_is_an_error_and_poisons_the_handle() {
        let mut t = ProcessTransport::spawn(
            "sh",
            &["-c", "printf '%s' '{\"jsonrpc\":\"2.0\",\"id\":1'"],
            &[],
        )
        .unwrap();
        let error = t.recv().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(
            t.recv().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe,
            "the same process handle must not be retried"
        );
        assert_eq!(
            t.send(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        t.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn unresponsive_agent_shutdown_signal_is_bounded_and_fail_closed() {
        let transport = Arc::new(Mutex::new(
            ProcessTransport::spawn("sh", &["-c", "exec sleep 30"], &[]).unwrap(),
        ));
        let cancel = transport.lock().unwrap().cancellation_handle().unwrap();
        let reader_transport = Arc::clone(&transport);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let reader = thread::spawn(move || {
            let mut transport = reader_transport.lock().unwrap();
            let result = transport.recv();
            let _ = done_tx.send(result);
        });
        thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        cancel.request_shutdown().unwrap();
        let result = done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("shutdown signal must interrupt an unresponsive read");
        assert!(
            result.is_err(),
            "shutdown is an unknown/fail-closed outcome"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        reader.join().unwrap();
        let mut transport = transport.lock().unwrap();
        transport.shutdown();
        assert!(!transport.is_alive());
    }

    #[test]
    fn spawn_missing_binary_errors() {
        assert!(ProcessTransport::spawn("definitely-not-a-real-agent", &[], &[]).is_err());
    }

    /// Sandboxed-process smoke test (Linux only): round-trip one frame over
    /// a bwrap-launched `/bin/cat`, with the monitor owning the child. The
    /// test skips (honest no-op) when bubblewrap or user namespaces are
    /// unavailable on the host; it never passes without real containment.
    #[cfg(all(unix, target_os = "linux"))]
    #[test]
    fn sandboxed_transport_roundtrips_through_bwrap() {
        use agentcowork_guard::sandbox::linux_bwrap_available;
        use agentcowork_guard::sandbox::{SandboxRole, SandboxSpec, profiles};
        if !linux_bwrap_available() {
            eprintln!("bwrap not available — skipping sandboxed transport test");
            return;
        }
        // The backend refuses to bind a nonexistent host path (fail-closed);
        // the worker profile's scratch dir must exist before spawning.
        let scratch = "/tmp/agentcowork-acp-sandbox-test";
        let _ = std::fs::create_dir_all(scratch);
        let spec = SandboxSpec {
            role: SandboxRole::ChildExecutionSandbox,
            profile: profiles::worker(scratch),
            network: "deny".into(),
            credentials: "none".into(),
            resource_limit_bytes: 1 << 20,
        };
        let Ok(mut t) = ProcessTransport::spawn_sandboxed(&spec, &["/bin/cat".into()]) else {
            // bwrap present but unusable (e.g. no user namespaces in this
            // container) — fail-closed today, not a transport regression.
            eprintln!("sandboxed spawn unavailable — skipping sandboxed transport test");
            return;
        };
        assert!(t.is_alive());
        t.send(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
            .unwrap();
        let echoed = t.recv().unwrap().expect("cat echoes through bwrap");
        assert_eq!(echoed, r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
        t.shutdown();
        assert!(!t.is_alive());
    }

    /// Regression: two frames arriving in a single read chunk must BOTH be
    /// delivered. The old `recv` kept only the first message and dropped the
    /// rest, which could hang the client waiting for a response that was
    /// already received and discarded.
    #[cfg(unix)]
    #[test]
    fn recv_queues_multiple_frames_from_one_chunk() {
        let mut t =
            ProcessTransport::spawn("sh", &["-c", "printf '{\"a\":1}\\n{\"b\":2}\\n'"], &[])
                .unwrap();
        let first = t.recv().unwrap().expect("first frame");
        let second = t.recv().unwrap().expect("second frame");
        assert_eq!(first, "{\"a\":1}");
        assert_eq!(second, "{\"b\":2}");
        // Stream is now at EOF.
        assert_eq!(t.recv().unwrap(), None);
        t.shutdown();
    }
}
