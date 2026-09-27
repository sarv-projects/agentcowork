//! Shell managed state (Fix 1b).
//!
//! Extracted verbatim — field-for-field and type-for-type — from the old
//! `lib.rs` `AppState` so this is pure motion: every existing `use
//! crate::AppState` (26 files) still resolves through the re-export in
//! `lib.rs`. Keeping the fields in one struct is a deliberate, conservative
//! first step; grouping them into domain sub-structs is a follow-up and
//! should land behind the same safety test as Fix 1a.

use std::path::PathBuf;
use std::process::{ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

use agentcowork_core::GuardService;
use agentcowork_guard::prescan::Guard;
use agentcowork_vault::Vault;

use crate::acp_cmds::AcpHandle;
use crate::browser_cmds::LiveBrowser;
use crate::catalog_cmds::CatalogState;
use crate::control::FileUndo;
use crate::desktop_cmds::DesktopSlot;
use crate::mcp_cmds::McpServerRow;

use agentcowork_core::terminal::PtyHost;

/// Shared state handed to every Tauri command via `State<'_, AppState>`.
pub struct AppState {
    /// P0.2: the boot report line from `agentcowork-core::boot`.
    pub boot_report: Mutex<String>,
    /// P0.2: an initialized Guard-1 scanner (stub blocklist until P7.4).
    pub guard: Guard,
    /// The encrypted vault (opened at boot; shared with the chat relay).
    pub vault: Arc<Mutex<Vault>>,
    /// Whether the currently managed vault was successfully opened with an
    /// authoritative key. Commands update this after setup/unlock so the
    /// renderer receives the actual lifecycle state, not a keyfile guess.
    pub vault_unlocked: AtomicBool,
    /// Monotonic activity clock for the currently managed coordinator. This
    /// lets the readiness probe detect a dead/stale relay instead of treating
    /// `Option<ChatRelay>` as proof that the sidecar is alive.
    pub sidecar_activity_ms: Mutex<Option<Arc<AtomicU64>>>,
    /// P45.8 — the shell sets this when a turn needs a parked sidecar.
    pub sidecar_resume: Arc<AtomicBool>,
    /// True while the coordinator is down after an idle exit.
    pub sidecar_parked: Arc<AtomicBool>,
    /// P1.4: the chat relay over the coordinator link. `None` until the
    /// supervisor hands the sidecar's stdio pipes to a `SidecarLink` (the
    /// integration seam — the relay + protocol are fully built + tested).
    pub chat_relay: Mutex<Option<agentcowork_core::ChatRelay<ChildStdin, ChildStdout>>>,
    /// P3.1: the replay store base dir (replays/ + screenshots/ + index).
    pub replay_dir: PathBuf,
    /// P3.2: the cockpit / ambient flight-deck live state (agent cards,
    /// interrupts, quiet flag) — fed by the coordinator via the feed seams,
    /// polled by the UI.
    pub cockpit: Arc<Mutex<agentcowork_audit::cockpit::CockpitState>>,
    /// P7.5/J21 (Guard-2): the shared pre-flight service (tickets + policy +
    /// estop + profile) — minted by the coordinator over `guard/*`, rendered
    /// + approved/rejected by the cards here, consumed by the executor.
    pub guard_service: Arc<Mutex<GuardService>>,
    /// F12/J17 (ACP harness bridge): live ACP agent sessions keyed by handle
    /// id — spawned via `acp_launch`, driven via `acp_prompt`/`acp_cancel`.
    /// The handle value carries its canonical Session/Work/Binding owner; the
    /// map is not an agent-id registry. P71.3f — shared with the mounted
    /// readiness source, so the live handshake state readiness reports is the
    /// same map the launch path writes.
    pub(crate) acp_sessions: Arc<Mutex<std::collections::HashMap<String, AcpHandle>>>,
    /// H4: Merkle chain of mutations (Excel / ACP-install / undo).
    pub audit: Mutex<agentcowork_audit::merkle::MerkleChain>,
    /// Durable NDJSON audit log (best-effort; None if the file couldn't open).
    pub audit_log: Mutex<Option<agentcowork_audit::AuditWriter>>,
    /// File snapshots for agent undo (xlsx + other shell mutations).
    pub file_undos: Mutex<Vec<FileUndo>>,
    /// J16: whether the device is on battery (heavy storage scans defer).
    pub battery: Arc<AtomicBool>,
    /// P11.5.3: the live CDP browser session for the browse view (None until
    /// `browser_start`). Dropping it kills the Chrome child.
    pub browser: Mutex<Option<LiveBrowser>>,
    /// P11.5.3: live shell processes keyed by session id (shell view).

    /// H36 (P54): the integrated-terminal PTY host — profile-backed unix pty /
    /// ConPTY sessions keyed by `pty_id`. Sessions are session-scoped and
    /// survive a Shell-view unmount; dropping the host kills + reaps them.
    ///
    /// `Arc` (P68.9) so the `script.run` executor can hold the one plane and
    /// block on a command's completion without re-entering the managed-state
    /// lock on the tool-dispatch path.
    pub terminal: Arc<PtyHost>,
    /// P11.5.8: attached user-supplied MCP servers (rows for the Connectors
    /// panel) + the live child handles (dropping the map kills the child).
    pub mcp_servers: Arc<Mutex<std::collections::HashMap<String, McpServerRow>>>,
    /// Arc-shared so the agent loop's `ExternalToolBackend` (P55.11) can reach
    /// the same child that answered `tools/list` — one server per row, never a
    /// second spawn. Dropping the last reference kills the children.
    pub mcp_live:
        Arc<Mutex<std::collections::HashMap<String, agentcowork_mcp::attach::AttachedServer>>>,
    /// Remote-MCP OAuth 2.1: in-flight PKCE flows (store id → flow) and
    /// connected tokens (store id → bearer). Live in the shell, not the
    /// renderer — the coordinator never sees them.
    pub mcp_remote_flows:
        Arc<Mutex<std::collections::HashMap<String, crate::mcp_cmds::RemoteFlowState>>>,
    pub mcp_remote_tokens: Arc<Mutex<std::collections::HashMap<String, String>>>,
    /// P50.3.4 — remote MCP `tools/call` requests waiting on their Guard-2
    /// ticket (the request half minted the card; the commit half consumes the
    /// ticket and executes). Keyed by ticket id — direct remote calls must not
    /// bypass the same approval path native tools go through.
    pub mcp_pending_calls:
        Mutex<std::collections::HashMap<String, crate::mcp_cmds::PendingRemoteCall>>,
    /// P48.3 (E9): the lazily-attached native desktop engine (None until first
    /// use; honest-fail on headless / no display). Engine + audit bridge live
    /// here — never in the renderer or the coordinator.
    pub desktop: Mutex<DesktopSlot>,
    /// P15-H29: live artifact preview servers keyed by loopback port
    /// (`agentcowork_script::artifact::serve`). Dropping a handle stops its
    /// server thread; the map is the shell's registry of running previews so
    /// `artifact_stop` can tear a specific one down. Guard-2-ticketed at the
    /// command layer — the server is loopback-only + path-floored by
    /// construction.
    pub artifacts: Mutex<std::collections::HashMap<u16, agentcowork_script::artifact::ServerHandle>>,
    /// P9.5: the local OpenAI-compatible server (loopback + bearer token).
    /// `None` until `openai_server_start`; dropping it closes the listener.
    pub openai_server: Mutex<crate::openai_cmds::OpenAiServerSlot>,
    /// P50.4.2 — in-flight local-model downloads (Hugging Face GGUF/MLX).
    /// Each slot carries the cooperative cancel flag + shared status; the
    /// worker thread removes the slot at terminal state and leaves the
    /// `.part` staging file in place so a later start resumes via `Range`.
    pub model_downloads:
        Mutex<std::collections::HashMap<String, crate::model_cmds::ModelDownloadSlot>>,
    /// AgentCowork-owned model-runtime process handles keyed by stable serve id.
    /// Dropping AppState drops this registry and therefore kills/reaps every
    /// retained managed child through `ManagedServeHandle`'s RAII contract.
    pub(crate) model_serves: Mutex<crate::model_cmds::ManagedServeRegistry>,
    /// Last runtime/model observations used to answer a failed live probe with
    /// the prior fact explicitly marked Down, never an invented empty list.
    pub(crate) runtime_observations:
        Mutex<std::collections::HashMap<String, crate::runtime_cmds::CachedRuntimeObservation>>,
    /// P56.1 — the live models.dev catalog: the durable snapshot store, its
    /// refresh cadence, and the serialized refresh gate. The background job,
    /// Settings → Providers, the model table and the chat relay's endpoint
    /// resolution all read this one owner.
    pub catalog: Arc<CatalogState>,
    /// P70.C3/C4 — metadata for the pending update (channel + version) while
    /// the background download runs. The artifact bytes live in the managed
    /// `updater_cmds::PendingUpdateSlot` (different lifetime); this slot only
    /// tracks *that a download is in flight* so a second one can be refused.
    pub pending_update: Mutex<Option<crate::updater_cmds::PendingUpdate>>,
    /// P63.11 — the loopback shared-plane lease handed to ACP `session/new`.
    pub channel_b: Mutex<Option<crate::channel_b::ChannelBSlot>>,
    /// Tool service published by the chat relay for that lease's handler.
    pub channel_b_tools: crate::channel_b::SharedTools,
}

impl AppState {
    /// Snapshot the handles whose canonical owner belongs to `session_id`.
    ///
    /// This is intentionally a projection over the live handle values, not a
    /// second binding registry. Callers use the returned ids only after the
    /// map lock is released; provider I/O must never run under this lock.
    #[allow(dead_code)] // the control-channel writer owns the final call site
    pub(crate) fn acp_handles_for_session(&self, session_id: &str) -> Result<Vec<String>, String> {
        let sessions = self.acp_sessions.lock().map_err(|e| e.to_string())?;
        Ok(sessions
            .iter()
            .filter_map(|(handle, entry)| {
                let owner = entry.owner.as_ref()?;
                (owner.session_id == session_id).then(|| handle.clone())
            })
            .collect())
    }
}
