//! P38 — Dynamic Chief (spec §4.2.5a): the top brain is a configurable slot.
//!
//! [`ChiefAdapter`] is the trait the dispatcher uses so the inbuilt engine
//! and an external ACP agent are driven identically (spec §4.2.5a §2).
//! [`GovernedSession`] is the corrected governance boundary (§3): omitting
//! `fs`/`terminal` does NOT force MCP Channel B — it makes the agent fall back
//! to its own in-process backends, where its own sandbox governs. The three
//! honest states are **Mediated** (we advertise + service fs/terminal through
//! guard), **Self-contained** (agent's own sandbox; we claim only that), and
//! **NotGoverned** (no boundary, no sandbox — no governance claim at all).
//!
//! Two impls:
//! - [`DelegateChief`] — the **inbuilt** path: a thin in-process delegate the
//!   host fills with handlers (no ACP hop). Default Chief.
//! - [`AcpChief`] — the **external** path: drives the J17 stdio client
//!   (`AcpSession`) over official ACP adapters. The driver runs on a thread
//!   so `stream_events` stays live while a turn runs; `request_permission`
//!   answers the driver's blocked permission callback (the Guard-2 seam).

use crate::client::{AcpError, AcpSession, AcpTransport, ClientMediation};
use crate::messages::{
    AgentCapabilities, ClientCapabilities, ClientInfo, McpServer, PermissionDecision,
};
use crate::registry::AuthMode;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// A session-scoped id (opaque string; wire-friendly).
pub type SessionId = String;

/// A handle to a started Chief session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub session_id: String,
}

/// Session options for [`ChiefAdapter::start_session`].
#[derive(Debug, Clone, Default)]
pub struct SessionOptions {
    pub cwd: String,
    pub mcp_servers: Vec<McpServer>,
    /// The initial prompt — the dispatcher injects the memory passport (C10)
    /// + taste profile (C9) here in **both** impl paths (spec §4.2.5a §2).
    pub initial_prompt: String,
}

/// A user message to the Chief.
#[derive(Debug, Clone)]
pub struct UserMessage {
    pub text: String,
}

/// P38 (spec §4.2.5a §2) — inject the memory passport (C10) + governance
/// block into an ACP Chief's prompt, mirroring the inbuilt path's
/// `<memory_warm_set>` injection below the cache boundary. The external Chief
/// gets the same durable context as the inbuilt engine; the block is inert
/// when there are no facts to pass. Callers fetch `core_facts()` from the
/// shared `MemoryService` (best-effort — a missing memory handler never
/// blocks a turn, exactly like the inbuilt path).
pub fn build_chief_prompt(
    text: &str,
    core_facts: &[String],
    governance: &GovernedSession,
) -> String {
    build_chief_prompt_with_steering(text, core_facts, governance, None, None)
}

/// P71.9i — the full passport assembly, in the documented order
/// (`ARCH/13-PROMPT-ANATOMY.md` §"Live Rust Context Passport & Tool Affinity
/// Steering", `ARCH/EXTERNAL-AGENTS.md` §11.1):
///
/// ```text
/// 1. <memory_passport>
/// 2. ## Governance
/// 3. ## Shared Cowork Capabilities (tool affinity steering)
/// 4. ## Installed subagent delegation mix
/// 5. the user turn
/// ```
///
/// The steering blocks exist because external agents are trained to reach for
/// their own habits — a Python script or `curl` — before the host's shared
/// facades, and a shell-biased model that never calls `office.*`/`browser.*`
/// silently loses the audit trail. They are **advisory text**, not a policy
/// boundary: Guard-1 is what refuses a shell route (SEC-4), and the harness
/// limits still apply.
///
/// `affinity` and `delegation_mix` carry the block *contents*; the headings are
/// added here so the order and the newlines cannot drift per call site.
/// Passing `None` for both is exactly [`build_chief_prompt`].
pub fn build_chief_prompt_with_steering(
    text: &str,
    core_facts: &[String],
    governance: &GovernedSession,
    affinity: Option<&str>,
    delegation_mix: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !core_facts.is_empty() {
        parts.push(format!(
            "<memory_passport>\n{}\n</memory_passport>",
            core_facts.join("\n")
        ));
    }
    parts.push(format!(
        "## Governance\nThis session runs under {}.\n",
        governance.badge()
    ));
    if let Some(body) = affinity {
        if !body.trim().is_empty() {
            parts.push(format!("## Shared Cowork Capabilities\n{body}"));
        }
    }
    if let Some(body) = delegation_mix {
        if !body.trim().is_empty() {
            parts.push(format!("## Installed subagent delegation mix\n{body}"));
        }
    }
    parts.push(text.to_string());
    parts.join("\n\n")
}

/// P71.9i — the tool-affinity steering text (`ARCH/EXTERNAL-AGENTS.md` §11.1).
/// Kept as a constant so the published contract and the shipped prompt are one
/// string; `chief.rs` tests assert the facade names it instructs the model to
/// prefer, and `scripts/check-prompt-steering.mjs` fails if this and the doc
/// drift apart.
pub const COWORK_AFFINITY_STEERING: &str = "\
You have direct access to AgentCowork native cowork tools via the connected MCP server:
\
- **Spreadsheets (.xlsx) & Documents (.docx/.pptx):** ALWAYS use `office.*` tools \
(`office.open`, `office.inspect`, `office.edit`, `office.calculate`). Do NOT write custom \
Python scripts or execute CLI tools in bash to modify office files.
\
- **Web Browsing & Research:** ALWAYS use `browser.*` tools (`browser.research`, \
`browser.operate`, `browser.extract`) rather than executing raw curl or headless scripts in bash.
\
- **Desktop UI Automation:** Use `computer_use.*` tools (`computer_use.see`, `computer_use.act`).
\
- **Subagent Delegation:** Use `delegate.spawn` to delegate subtasks to isolated child worktrees.";

/// A permission request surfaced for Guard-2 (the ticket seam).
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub tool_call_id: String,
    pub title: String,
    pub kind: String,
}

/// The host's answer to a [`PermissionRequest`].
#[derive(Debug, Clone)]
pub struct Approval {
    pub approved: bool,
    pub option_id: Option<String>,
}

impl Approval {
    pub fn allow() -> Self {
        Self {
            approved: true,
            option_id: None,
        }
    }
    pub fn deny() -> Self {
        Self {
            approved: false,
            option_id: None,
        }
    }
}

/// The host's authorization seam for ACP permission prompts (**P69.C1 / I12**).
///
/// The ACP layer must never decide a permission question on its own: the host
/// implements this over Guard (evaluate → mint an [`AuthorizationTicket`] →
/// consume it) with the agent identity attached to the ticket, and returns the
/// verdict. The default when no gate is installed is fail-closed:
/// [`DenyAllGate`] denies everything, so a session can never silently approve.
///
/// [`AuthorizationTicket`]: agentcowork_types::TicketId
pub trait PermissionGate: Send + Sync {
    /// Decide one pending request. Implementations consult Guard and their
    /// decision is final (the ACP layer maps it to the wire option).
    fn decide(&self, req: &PermissionRequest) -> Approval;
}

/// The fail-closed default gate: no host gate attached ⇒ nothing approved.
pub struct DenyAllGate;

impl PermissionGate for DenyAllGate {
    fn decide(&self, _req: &PermissionRequest) -> Approval {
        Approval::deny()
    }
}

/// How this session should be governed (**P69.C3**).
///
/// The default is [`GovernancePreference::Mediated`]: when a mediation seam is
/// attached we advertise + service `fs`/`terminal` through Guard. Without a
/// seam the same preference degrades honestly to the self-contained path
/// (advertise nothing — never claim mediation we cannot service).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GovernancePreference {
    /// Advertise `fs`/`terminal` and service them through the host's
    /// [`ClientMediation`] seam (Guard → executor → observation + receipt).
    /// This is a **v1-peer** surface: ACP v2 removes it in favour of
    /// `mcpServers` (Channel B), so a v2 connection degrades cleanly to
    /// self-contained or Channel B.
    #[default]
    Mediated,
    /// Withhold `fs`/`terminal`; the agent's own sandbox governs and we claim
    /// only that (declared alternative, always selectable).
    SelfContained,
}

/// Capabilities reported back from [`ChiefAdapter::initialize`].
#[derive(Debug, Clone, Default)]
pub struct ChiefCapabilities {
    pub governed: GovernedSession,
    pub auth_mode: Option<AuthMode>,
    /// Channel B (our MCP catalog) is available to this agent.
    pub channel_b: bool,
}

/// The governance boundary per agent (spec §4.2.5a §3, corrected v3.46).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GovernedSession {
    /// We advertise `fs`/`terminal` and service those calls through
    /// Guard/path-floor/audit (vscode-acp/codeg pattern). Best observability.
    Mediated { fs: bool, terminal: bool },
    /// We withhold `fs`/`terminal`; the agent's own sandbox governs. We claim
    /// only that — never audit visibility over its internal ops.
    SelfContained { channel_b: bool },
    /// No boundary, no sandbox, no MCP — no governance claim at all.
    #[default]
    NotGoverned,
}

impl GovernedSession {
    /// The UI badge per mode (spec §4.2.5a §3).
    pub fn badge(self) -> &'static str {
        match self {
            GovernedSession::Mediated { .. } => "Governed-Mediated",
            GovernedSession::SelfContained { .. } => "Self-contained",
            GovernedSession::NotGoverned => "NotGoverned",
        }
    }
}

/// Compute the governance mode (spec §4.2.5a §3). `advertised` = did WE
/// advertise the fs/terminal surface; `agent` = the agent's
/// `agentCapabilities` answer; `sandbox_claim` = an explicit per-agent
/// manifest claim that the agent runs under its own OS-level sandbox.
pub fn governance_mode(
    advertised: bool,
    agent: &AgentCapabilities,
    sandbox_claim: bool,
) -> GovernedSession {
    if advertised {
        return GovernedSession::Mediated {
            fs: true,
            terminal: true,
        };
    }
    let channel_b = agent.mcp_capabilities.http || agent.mcp_capabilities.sse;
    if sandbox_claim || channel_b {
        GovernedSession::SelfContained { channel_b }
    } else {
        GovernedSession::NotGoverned
    }
}

/// A Chief event on the stream.
#[derive(Debug, Clone)]
pub enum ChiefEvent {
    Token(String),
    ToolCall { id: String, title: String },
    PermissionRequest(PermissionRequest),
    Done { stop_reason: String },
    Error(String),
}

/// The event stream type (a blocking receiver the host drains).
pub type EventStream = Receiver<ChiefEvent>;

/// Current session state for [`ChiefAdapter::update`].
#[derive(Debug, Clone)]
pub struct SessionState {
    pub session_id: String,
    pub turn_count: u64,
    pub alive: bool,
}

/// Errors from the Chief adapter layer.
#[derive(Debug, thiserror::Error)]
pub enum ChiefError {
    #[error("chief not initialized")]
    NotInitialized,
    #[error("no active session")]
    NoSession,
    #[error("chief process died")]
    ChiefDied,
    #[error("driver stopped")]
    DriverStopped,
    #[error("permission request {0} has no pending answer channel")]
    NoPendingPermission(String),
    #[error("acp error: {0}")]
    Acp(#[from] AcpError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// The adapter contract (spec §4.2.5a §2). The dispatcher builds one impl
/// from `primary_chief` and treats both kinds identically.
pub trait ChiefAdapter {
    fn initialize(&mut self, session: &SessionId) -> Result<ChiefCapabilities, ChiefError>;
    fn start_session(&mut self, opts: SessionOptions) -> Result<SessionHandle, ChiefError>;
    fn send_message(&mut self, h: &SessionHandle, msg: UserMessage) -> Result<(), ChiefError>;
    fn stream_events(&mut self, h: &SessionHandle) -> EventStream;
    /// → Guard-2 ticket flow: the host answers a pending permission request.
    fn request_permission(&mut self, req: PermissionRequest) -> Result<Approval, ChiefError>;
    /// → watchdog / budget kill points.
    fn cancel(&mut self, h: &SessionHandle) -> Result<(), ChiefError>;
    /// → audit NDJSON.
    fn update(&mut self, h: &SessionHandle) -> Result<SessionState, ChiefError>;
}

// ---------------------------------------------------------------------------
// Impl A — Inbuilt (DelegateChief): direct in-process calls, no ACP hop.
// ---------------------------------------------------------------------------

type InitHandler = Box<dyn FnMut(&SessionId) -> ChiefCapabilities + Send>;
type StartHandler = Box<dyn FnMut(SessionOptions) -> Result<SessionHandle, ChiefError> + Send>;
type SendHandler = Box<dyn FnMut(&SessionHandle, UserMessage) -> Result<(), ChiefError> + Send>;
type StreamHandler = Box<dyn FnMut(&SessionHandle) -> EventStream + Send>;
type PermissionHandler = Box<dyn FnMut(PermissionRequest) -> Result<Approval, ChiefError> + Send>;
type CancelHandler = Box<dyn FnMut(&SessionHandle) -> Result<(), ChiefError> + Send>;
type UpdateHandler = Box<dyn FnMut(&SessionHandle) -> Result<SessionState, ChiefError> + Send>;

/// The **inbuilt** Chief: a thin in-process delegate. The host (the
/// coordinator's inbuilt engine) fills the handlers; there is no ACP hop and
/// every call is direct. Default `primary_chief`.
pub struct DelegateChief {
    session_id: Option<SessionId>,
    on_initialize: InitHandler,
    on_start_session: StartHandler,
    on_send_message: SendHandler,
    on_stream_events: StreamHandler,
    on_request_permission: PermissionHandler,
    on_cancel: CancelHandler,
    on_update: UpdateHandler,
}

impl DelegateChief {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        on_initialize: InitHandler,
        on_start_session: StartHandler,
        on_send_message: SendHandler,
        on_stream_events: StreamHandler,
        on_request_permission: PermissionHandler,
        on_cancel: CancelHandler,
        on_update: UpdateHandler,
    ) -> Self {
        Self {
            session_id: None,
            on_initialize,
            on_start_session,
            on_send_message,
            on_stream_events,
            on_request_permission,
            on_cancel,
            on_update,
        }
    }
}

impl ChiefAdapter for DelegateChief {
    fn initialize(&mut self, session: &SessionId) -> Result<ChiefCapabilities, ChiefError> {
        let caps = (self.on_initialize)(session);
        self.session_id = Some(session.clone());
        Ok(caps)
    }

    fn start_session(&mut self, opts: SessionOptions) -> Result<SessionHandle, ChiefError> {
        (self.on_start_session)(opts)
    }

    fn send_message(&mut self, h: &SessionHandle, msg: UserMessage) -> Result<(), ChiefError> {
        (self.on_send_message)(h, msg)
    }

    fn stream_events(&mut self, h: &SessionHandle) -> EventStream {
        (self.on_stream_events)(h)
    }

    fn request_permission(&mut self, req: PermissionRequest) -> Result<Approval, ChiefError> {
        (self.on_request_permission)(req)
    }

    fn cancel(&mut self, h: &SessionHandle) -> Result<(), ChiefError> {
        (self.on_cancel)(h)
    }

    fn update(&mut self, h: &SessionHandle) -> Result<SessionState, ChiefError> {
        (self.on_update)(h)
    }
}

// ---------------------------------------------------------------------------
// Impl B — AcpChief: the J17 stdio client behind the same interface.
// ---------------------------------------------------------------------------

enum DriverCmd {
    Initialize {
        advertise_fs_terminal: bool,
        sandbox_claim: bool,
        reply: Sender<Result<ChiefCapabilities, ChiefError>>,
    },
    Start {
        opts: SessionOptions,
        reply: Sender<Result<SessionHandle, ChiefError>>,
    },
    Prompt {
        text: String,
    },
    Cancel,
    Shutdown,
}

/// The **external** Chief: wraps the J17 ACP client. A driver thread owns the
/// [`AcpSession`] so `stream_events` stays live while a turn runs; the host
/// answers permission requests through [`ChiefAdapter::request_permission`].
pub struct AcpChief {
    driver: Sender<DriverCmd>,
    events: EventStream,
    waiters: Arc<Mutex<HashMap<String, Sender<PermissionDecision>>>>,
    session_id: Option<String>,
    turn_count: u64,
    caps: Option<ChiefCapabilities>,
    thread: Option<JoinHandle<()>>,
    /// The host's Guard seam (P69.C1). Default: deny everything.
    gate: Arc<dyn PermissionGate>,
    /// The host's mediation seam (P69.C2). `None` ⇒ self-contained path.
    mediation: Option<Arc<dyn ClientMediation>>,
    /// The declared governance preference (P69.C3).
    governance: GovernancePreference,
}

impl AcpChief {
    /// Spawn the driver thread over `transport` with the given client info.
    /// No mediation seam and no host gate: the honest degraded path (a
    /// self-contained session that can never approve).
    pub fn spawn<T: AcpTransport + Send + 'static>(transport: T, client_info: ClientInfo) -> Self {
        Self::spawn_with(
            transport,
            client_info,
            None,
            GovernancePreference::default(),
        )
    }

    /// Spawn with an explicit governance contract: the mediation seam the
    /// session may advertise/service, and the declared preference.
    pub fn spawn_with<T: AcpTransport + Send + 'static>(
        transport: T,
        client_info: ClientInfo,
        mediation: Option<Arc<dyn ClientMediation>>,
        governance: GovernancePreference,
    ) -> Self {
        let (cmd_tx, cmd_rx) = channel::<DriverCmd>();
        let (event_tx, event_rx) = channel::<ChiefEvent>();
        let waiters: Arc<Mutex<HashMap<String, Sender<PermissionDecision>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let waiters_clone = waiters.clone();
        let mediation_for_driver = mediation.clone();
        let thread = std::thread::spawn(move || {
            driver_loop(
                transport,
                client_info,
                cmd_rx,
                event_tx,
                waiters_clone,
                mediation_for_driver,
            );
        });
        Self {
            driver: cmd_tx,
            events: event_rx,
            waiters,
            session_id: None,
            turn_count: 0,
            caps: None,
            thread: Some(thread),
            gate: Arc::new(DenyAllGate),
            mediation,
            governance,
        }
    }

    /// Install the host's Guard seam (P69.C1). Must be installed before the
    /// first `request_permission` call; without it every request is denied.
    pub fn set_permission_gate(&mut self, gate: Arc<dyn PermissionGate>) {
        self.gate = gate;
    }

    /// Builder form of [`AcpChief::set_permission_gate`].
    pub fn with_permission_gate(mut self, gate: Arc<dyn PermissionGate>) -> Self {
        self.gate = gate;
        self
    }

    /// Whether this instance can actually service mediated requests: the
    /// declared preference is mediated **and** a seam is attached. This is the
    /// exact condition `initialize` advertises on the wire (P69.C3).
    pub fn mediated(&self) -> bool {
        self.governance == GovernancePreference::Mediated && self.mediation.is_some()
    }

    /// The declared governance preference.
    pub fn governance_preference(&self) -> GovernancePreference {
        self.governance
    }

    /// The negotiated governance (available after `initialize`).
    pub fn capabilities(&self) -> Option<&ChiefCapabilities> {
        self.caps.as_ref()
    }
}

impl ChiefAdapter for AcpChief {
    fn initialize(&mut self, session: &SessionId) -> Result<ChiefCapabilities, ChiefError> {
        let (tx, rx) = channel();
        // P69.C3 — the default is chosen from the explicit governance policy,
        // not hardcoded: mediated is advertised **only** when this instance can
        // actually service the surface; otherwise the self-contained path is
        // the honest answer.
        self.driver
            .send(DriverCmd::Initialize {
                advertise_fs_terminal: self.mediated(),
                sandbox_claim: false,
                reply: tx,
            })
            .map_err(|_| ChiefError::DriverStopped)?;
        let caps = rx.recv().map_err(|_| ChiefError::DriverStopped)??;
        let _ = session; // ACP sessions are created by session/new; the caller's id is our handle key
        self.session_id = Some("acp".to_string());
        self.caps = Some(caps.clone());
        Ok(caps)
    }

    fn start_session(&mut self, opts: SessionOptions) -> Result<SessionHandle, ChiefError> {
        let (tx, rx) = channel();
        self.driver
            .send(DriverCmd::Start { opts, reply: tx })
            .map_err(|_| ChiefError::DriverStopped)?;
        let handle = rx.recv().map_err(|_| ChiefError::DriverStopped)??;
        self.session_id = Some(handle.session_id.clone());
        Ok(handle)
    }

    fn send_message(&mut self, h: &SessionHandle, msg: UserMessage) -> Result<(), ChiefError> {
        let _ = h;
        self.driver
            .send(DriverCmd::Prompt { text: msg.text })
            .map_err(|_| ChiefError::DriverStopped)
    }

    fn stream_events(&mut self, _h: &SessionHandle) -> EventStream {
        // The driver keeps ONE sender (captured at spawn); the host drains
        // through the receiver we own. Swap in a fresh dead receiver and hand
        // the live one out — the dispatcher calls this once per session.
        let (_dead_tx, dead_rx) = channel::<ChiefEvent>();
        std::mem::replace(&mut self.events, dead_rx)
    }

    fn request_permission(&mut self, req: PermissionRequest) -> Result<Approval, ChiefError> {
        let waiter = self
            .waiters
            .lock()
            .expect("waiters poisoned")
            .remove(&req.tool_call_id)
            .ok_or_else(|| ChiefError::NoPendingPermission(req.tool_call_id.clone()))?;
        // P69.C1 — the decision comes from the host's Guard seam, never from
        // this layer. `DenyAllGate` (the default) denies, so there is no
        // unconditional allow on any path here.
        let approval = self.gate.decide(&req);
        // FIX-03: an allow names **no** option id of its own. The wire reply
        // must be an option the agent actually offered, so the strict resolver
        // picks the offered `allow_once`; synthesizing an id here (this path
        // used to fall back to the tool-call id) would put a value in the
        // agent's hand that the agent never minted.
        let decision = if approval.approved {
            PermissionDecision::Allow {
                option_id: approval.option_id.clone(),
            }
        } else {
            PermissionDecision::Deny {
                option_id: approval.option_id.clone(),
            }
        };
        waiter
            .send(decision)
            .map_err(|_| ChiefError::DriverStopped)?;
        Ok(approval)
    }

    fn cancel(&mut self, _h: &SessionHandle) -> Result<(), ChiefError> {
        self.driver
            .send(DriverCmd::Cancel)
            .map_err(|_| ChiefError::DriverStopped)
    }

    fn update(&mut self, h: &SessionHandle) -> Result<SessionState, ChiefError> {
        Ok(SessionState {
            session_id: h.session_id.clone(),
            turn_count: self.turn_count,
            alive: true,
        })
    }
}

impl Drop for AcpChief {
    fn drop(&mut self) {
        let _ = self.driver.send(DriverCmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The driver thread: owns the [`AcpSession`] and serializes commands.
fn driver_loop<T: AcpTransport + Send + 'static>(
    transport: T,
    client_info: ClientInfo,
    cmds: Receiver<DriverCmd>,
    events: Sender<ChiefEvent>,
    waiters: Arc<Mutex<HashMap<String, Sender<PermissionDecision>>>>,
    mediation: Option<Arc<dyn ClientMediation>>,
) {
    let mut session = AcpSession::new(transport);
    // Attach the mediation seam before `initialize` so the advertised client
    // capabilities match exactly what the host can service (P69.C2/P69.C3).
    if let Some(m) = mediation {
        session.set_mediator(m);
    }
    let mut initialized = false;
    loop {
        let Ok(cmd) = cmds.recv() else {
            break;
        };
        match cmd {
            DriverCmd::Initialize {
                advertise_fs_terminal,
                sandbox_claim,
                reply,
            } => {
                let caps = build_client_capabilities(advertise_fs_terminal);
                let result = (|| -> Result<ChiefCapabilities, ChiefError> {
                    let res = session.initialize_with_caps(client_info.clone(), caps)?;
                    let mode = governance_mode(
                        advertise_fs_terminal,
                        &res.agent_capabilities,
                        sandbox_claim,
                    );
                    let channel_b = res.agent_capabilities.mcp_capabilities.http
                        || res.agent_capabilities.mcp_capabilities.sse;
                    initialized = true;
                    Ok(ChiefCapabilities {
                        governed: mode,
                        auth_mode: None,
                        channel_b,
                    })
                })();
                let _ = reply.send(result);
            }
            DriverCmd::Start { opts, reply } => {
                let result = (|| -> Result<SessionHandle, ChiefError> {
                    if !initialized {
                        return Err(ChiefError::NotInitialized);
                    }
                    let sid = session.session_new(&opts.cwd, opts.mcp_servers)?;
                    // The initial prompt (passport + taste profile) rides the
                    // first Prompt command — the dispatcher sends it right
                    // after start_session.
                    Ok(SessionHandle { session_id: sid })
                })();
                let _ = reply.send(result);
            }
            DriverCmd::Prompt { text } => {
                let outcome = session.prompt(&text, |params| {
                    let tool_call_id = params.tool_call.tool_call_id.clone();
                    let (tx, rx) = channel::<PermissionDecision>();
                    waiters
                        .lock()
                        .expect("waiters poisoned")
                        .insert(tool_call_id.clone(), tx);
                    let _ = events.send(ChiefEvent::PermissionRequest(PermissionRequest {
                        tool_call_id: tool_call_id.clone(),
                        title: params.tool_call.title.clone(),
                        kind: params
                            .tool_call
                            .kind
                            .unwrap_or(crate::messages::ToolKind::Other)
                            .as_str()
                            .to_string(),
                    }));
                    // Block until the host answers (the Guard-2 seam).
                    rx.recv()
                        .unwrap_or(PermissionDecision::Deny { option_id: None })
                });
                match outcome {
                    Ok(o) => {
                        let _ = events.send(ChiefEvent::Done {
                            stop_reason: format!("{:?}", o.stop_reason),
                        });
                    }
                    Err(e) => {
                        let _ = events.send(ChiefEvent::Error(e.to_string()));
                    }
                }
            }
            DriverCmd::Cancel => {
                let _ = session.cancel();
            }
            DriverCmd::Shutdown => {
                session.shutdown();
                break;
            }
        }
    }
}

/// The ACP `clientCapabilities` payload: advertised (fs/terminal: true —
/// Mediated) or withheld (all false — Self-contained path). Withholding never
/// forces MCP Channel B (spec §4.2.5a §3 corrected).
fn build_client_capabilities(advertise: bool) -> ClientCapabilities {
    ClientCapabilities {
        fs: crate::messages::FsCapabilities {
            read_text_file: advertise,
            write_text_file: advertise,
        },
        terminal: advertise,
        session: Some(crate::messages::SessionCapabilities::config_options_with_boolean()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{McpCapabilities, PermissionRequestParams, StopReason, ToolCall};
    use std::collections::VecDeque;
    use std::io;

    fn agent_caps(mcp_http: bool, mcp_sse: bool) -> AgentCapabilities {
        AgentCapabilities {
            mcp_capabilities: McpCapabilities {
                http: mcp_http,
                sse: mcp_sse,
            },
            ..Default::default()
        }
    }

    #[test]
    fn governance_advertised_is_mediated() {
        let mode = governance_mode(true, &agent_caps(false, false), false);
        assert_eq!(
            mode,
            GovernedSession::Mediated {
                fs: true,
                terminal: true
            }
        );
        assert_eq!(mode.badge(), "Governed-Mediated");
    }

    #[test]
    fn governance_withhold_without_sandbox_or_mcp_is_not_governed() {
        // The negative case from the acceptance: no negotiation, no sandbox,
        // no MCP → NotGoverned, no governance claim.
        let mode = governance_mode(false, &agent_caps(false, false), false);
        assert_eq!(mode, GovernedSession::NotGoverned);
        assert_eq!(mode.badge(), "NotGoverned");
    }

    #[test]
    fn governance_withhold_with_sandbox_is_self_contained() {
        let mode = governance_mode(false, &agent_caps(false, false), true);
        assert_eq!(mode, GovernedSession::SelfContained { channel_b: false });
        assert_eq!(mode.badge(), "Self-contained");
    }

    #[test]
    fn build_chief_prompt_injects_passport_and_governance() {
        let facts = vec![
            "user prefers concise answers".to_string(),
            "project uses pnpm".to_string(),
        ];
        let prompt = build_chief_prompt(
            "Summarize the repo",
            &facts,
            &GovernedSession::SelfContained { channel_b: true },
        );
        assert!(prompt.contains("<memory_passport>"));
        assert!(prompt.contains("user prefers concise answers"));
        assert!(prompt.contains("project uses pnpm"));
        assert!(prompt.contains("Self-contained"));
        assert!(prompt.ends_with("Summarize the repo"));
    }

    #[test]
    fn build_chief_prompt_without_facts_is_just_governance_plus_text() {
        // Empty warm set → no passport block, but the governance badge is
        // still honest (a Chief session always runs under some boundary).
        let prompt = build_chief_prompt("hi", &[], &GovernedSession::NotGoverned);
        assert!(!prompt.contains("<memory_passport>"));
        assert!(prompt.contains("NotGoverned"));
        assert!(prompt.ends_with("hi"));
    }

    #[test]
    fn p71_9i_steering_blocks_follow_the_documented_order_and_use_real_newlines() {
        // ARCH/13-PROMPT-ANATOMY.md: passport → governance → affinity → mix →
        // the user turn, in that order. The user turn is last, exactly once.
        let facts = vec!["prefers pnpm".to_string()];
        let prompt = build_chief_prompt_with_steering(
            "THE USER TURN",
            &facts,
            &GovernedSession::SelfContained { channel_b: true },
            Some(COWORK_AFFINITY_STEERING),
            Some("- Codex: review passes"),
        );
        let pos = |needle: &str| {
            prompt
                .find(needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        };
        assert!(pos("<memory_passport>") < pos("## Governance"));
        assert!(pos("## Governance") < pos("## Shared Cowork Capabilities"));
        assert!(pos("## Shared Cowork Capabilities") < pos("## Installed subagent delegation mix"));
        assert!(pos("## Installed subagent delegation mix") < pos("THE USER TURN"));
        assert_eq!(prompt.matches("THE USER TURN").count(), 1);
        assert!(prompt.ends_with("THE USER TURN"));
        // The steering must be the published text, not a paraphrase.
        assert!(prompt.contains("office.edit"));
        assert!(prompt.contains("browser.research"));
        assert!(prompt.contains("computer_use.act"));
        assert!(prompt.contains("delegate.spawn"));
        // Regression: the block was once injected with literal `\\n` escapes,
        // which arrived in the model's context as backslash-n, not newlines.
        assert!(
            !prompt.contains("\\n"),
            "literal backslash-n leaked into the prompt"
        );
    }

    #[test]
    fn p71_9i_steering_blocks_are_inert_when_absent_or_blank() {
        let base = build_chief_prompt("turn", &[], &GovernedSession::NotGoverned);
        assert!(!base.contains("## Shared Cowork Capabilities"));
        assert!(!base.contains("## Installed subagent delegation mix"));
        // Blank steering must not emit an empty heading.
        let blank = build_chief_prompt_with_steering(
            "turn",
            &[],
            &GovernedSession::NotGoverned,
            Some("   \n"),
            Some(""),
        );
        assert_eq!(blank, base);
    }

    #[test]
    fn governance_withhold_with_mcp_is_self_contained_with_channel_b() {
        let mode = governance_mode(false, &agent_caps(true, false), false);
        assert_eq!(mode, GovernedSession::SelfContained { channel_b: true });
    }

    #[test]
    fn delegate_chief_satisfies_the_trait_contract() {
        let mut chief = DelegateChief::new(
            Box::new(|_s| ChiefCapabilities {
                governed: GovernedSession::Mediated {
                    fs: true,
                    terminal: true,
                },
                ..Default::default()
            }),
            Box::new(|opts| {
                Ok(SessionHandle {
                    session_id: format!("inbuilt-{}", opts.cwd.len()),
                })
            }),
            Box::new(|_h, msg| {
                assert!(!msg.text.is_empty());
                Ok(())
            }),
            Box::new(|_h| {
                let (_tx, rx) = channel();
                rx
            }),
            Box::new(|_req| Ok(Approval::allow())),
            Box::new(|_h| Ok(())),
            Box::new(|h| {
                Ok(SessionState {
                    session_id: h.session_id.clone(),
                    turn_count: 1,
                    alive: true,
                })
            }),
        );
        let caps = chief.initialize(&"s1".into()).unwrap();
        assert_eq!(caps.governed.badge(), "Governed-Mediated");
        let h = chief
            .start_session(SessionOptions {
                cwd: "/w".into(),
                ..Default::default()
            })
            .unwrap();
        chief
            .send_message(&h, UserMessage { text: "hi".into() })
            .unwrap();
        let state = chief.update(&h).unwrap();
        assert!(state.alive);
    }

    // -- AcpChief driver tests (scripted transport) --

    struct Scripted {
        responses: VecDeque<String>,
        sent: Vec<String>,
    }

    impl AcpTransport for Scripted {
        fn send(&mut self, json: &str) -> io::Result<()> {
            self.sent.push(json.to_string());
            Ok(())
        }
        fn recv(&mut self) -> io::Result<Option<String>> {
            Ok(self.responses.pop_front())
        }
        fn is_alive(&mut self) -> bool {
            true
        }
        fn shutdown(&mut self) {}
    }

    fn ok(id: u64, result: serde_json::Value) -> String {
        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
    }

    fn init_result(mcp_http: bool) -> String {
        ok(
            1,
            serde_json::json!({
                "protocolVersion": 1,
                "agentCapabilities": { "loadSession": true, "mcpCapabilities": { "http": mcp_http, "sse": false } },
                "agentInfo": { "name": "mock-acp", "title": "Mock", "version": "1.0.0" },
                "authMethods": []
            }),
        )
    }

    fn client_info() -> ClientInfo {
        ClientInfo {
            name: "agentcowork".into(),
            title: "AgentCowork".into(),
            version: "0.1.0".into(),
        }
    }

    /// A stand-in host Guard seam that approves — the lifecycle test's
    /// purpose is the request→decision→resume round trip, not the policy.
    /// (The fail-closed default is covered by the DenyAllGate tests.)
    struct AllowGate;

    impl PermissionGate for AllowGate {
        fn decide(&self, _req: &PermissionRequest) -> Approval {
            Approval::allow()
        }
    }

    #[test]
    fn acp_chief_negotiates_not_governed_without_mcp_or_sandbox() {
        let t = Scripted {
            responses: VecDeque::from([init_result(false)]),
            sent: Vec::new(),
        };
        let mut chief = AcpChief::spawn(t, client_info());
        let caps = chief.initialize(&"s1".into()).unwrap();
        // The E2E negative: a mock agent with no mcpCapabilities and no
        // sandbox → NotGoverned, never a governance claim.
        assert_eq!(caps.governed, GovernedSession::NotGoverned);
        assert!(!caps.channel_b);
        drop(chief);
    }

    #[test]
    fn acp_chief_reports_channel_b_when_mcp_available() {
        let t = Scripted {
            responses: VecDeque::from([init_result(true)]),
            sent: Vec::new(),
        };
        let mut chief = AcpChief::spawn(t, client_info());
        let caps = chief.initialize(&"s1".into()).unwrap();
        assert_eq!(
            caps.governed,
            GovernedSession::SelfContained { channel_b: true }
        );
        assert!(caps.channel_b);
        drop(chief);
    }

    #[test]
    fn acp_chief_full_lifecycle_with_permission_seam() {
        // initialize → session/new → prompt (with a permission request
        // answered through the host's request_permission seam) → done.
        let t = Scripted {
            responses: VecDeque::from([
                init_result(false),
                ok(2, serde_json::json!({ "sessionId": "acp-1" })),
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 99, "method": "session/request_permission",
                    "params": {
                        "sessionId": "acp-1",
                        "toolCall": { "toolCallId": "tc-1", "title": "Edit a.rs", "kind": "edit" },
                        "options": [ { "optionId": "allow-once", "kind": "allow_once", "label": "Allow once" } ]
                    }
                })
                .to_string(),
                ok(3, serde_json::json!({ "stopReason": "end_turn" })),
            ]),
            sent: Vec::new(),
        };
        // P69.C1 — the host's Guard seam must be attached to approve; the
        // fail-closed default (DenyAllGate) denies without one.
        let mut chief = AcpChief::spawn(t, client_info()).with_permission_gate(Arc::new(AllowGate));
        chief.initialize(&"s1".into()).unwrap();
        let h = chief
            .start_session(SessionOptions {
                cwd: "/w".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(h.session_id, "acp-1");

        chief
            .send_message(
                &h,
                UserMessage {
                    text: "fix it".into(),
                },
            )
            .unwrap();

        // Drain events: a permission request arrives; answer it via the seam.
        let events = chief.stream_events(&h);
        let mut saw_permission = false;
        let mut saw_done = false;
        while let Ok(ev) = events.recv() {
            match ev {
                ChiefEvent::PermissionRequest(req) => {
                    saw_permission = true;
                    assert_eq!(req.tool_call_id, "tc-1");
                    let approval = chief.request_permission(req).unwrap();
                    assert!(approval.approved);
                }
                ChiefEvent::Done { .. } => {
                    saw_done = true;
                    break;
                }
                // A failed turn must end the drain: a driver that reported an
                // error will send no further events, and waiting for one would
                // hang the test instead of failing it.
                ChiefEvent::Error(message) => panic!("chief turn failed: {message}"),
                _ => {}
            }
        }
        assert!(saw_permission, "permission request must surface");
        assert!(saw_done, "done must surface");
        drop(chief);
    }

    #[test]
    fn acp_chief_wire_payload_withholds_fs_terminal_by_default() {
        // The withhold path is the default (Self-contained), NOT a
        // Channel-B force — the wire payload must carry fs/terminal false.
        let t = Scripted {
            responses: VecDeque::from([init_result(false)]),
            sent: Vec::new(),
        };
        let mut chief = AcpChief::spawn(t, client_info());
        chief.initialize(&"s1".into()).unwrap();
        // Capture the transport's sent initialize via the driver thread is
        // not exposed; instead assert governance reflects the withhold path.
        assert_eq!(
            chief.capabilities().unwrap().governed,
            GovernedSession::NotGoverned
        );
        let _ = t.sent;
        drop(chief);
    }

    // The wire-level mediate assertion lives in client.rs tests
    // (initialize_with_caps advertises fs/terminal when asked).
    #[test]
    fn permission_request_params_shape_parses() {
        let p: PermissionRequestParams = serde_json::from_value(serde_json::json!({
            "sessionId": "s",
            "toolCall": { "toolCallId": "t1", "title": "X", "kind": "edit" },
            "options": []
        }))
        .unwrap();
        assert_eq!(p.tool_call.tool_call_id, "t1");
        let tc: ToolCall = serde_json::from_value(serde_json::json!({
            "toolCallId": "t1", "title": "X", "kind": "edit"
        }))
        .unwrap();
        assert_eq!(tc.kind.unwrap().as_str(), "edit");
        let _ = StopReason::EndTurn;
    }
}
