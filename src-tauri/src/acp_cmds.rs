//! F12 / J17 — the **ACP harness bridge** commands (doc 45 §1, doc 57 §2).
//!
//! Thin wrappers over `agentcowork-acp`:
//! - [`acp_agents`] — the launch registry (the `ollama launch` pattern): one
//!   manifest per agent with its auth-mode badge, distribution and protocol,
//!   so the picker shows "same chat bar, agent differs, default = inbuilt".
//! - [`acp_launch`] — resolve the spawn plan, spawn the agent CLI, run the
//!   `initialize` handshake + `session/new`, and store the live session. The
//!   agent's advertised `authMethods` are surfaced: if `session/new` returns
//!   `auth_required`, the launch still succeeds but reports `authRequired:
//!   true` so the UI can render "Sign in with <agent>" before prompting.
//! - [`acp_authenticate`] — drive the ACP `authenticate` flow (agent-type:
//!   the agent handles login; url-type: return the browser URL, re-call after
//!   the user completes), then retry `session/new`.
//! - [`acp_prompt`] — drive one turn; the agent's `session/request_permission`
//!   requests are answered by the shared [`agentcowork_core::GuardService`]
//!   (estop → policy → profile), so an ACP agent obeys the *same* Guard-2
//!   ticket card as the inbuilt engine.
//! - [`acp_install_request`] / [`acp_install_commit`] — the F8 one-click
//!   install split into the Guard-2 halves: the request resolves the plan and
//!   mints a ticket (or auto-allows allow-listed agents); the commit consumes
//!   the ticket (`use_ticket`) and executes the download, so the download is
//!   a **renderable approval card**, not a silent write.
//! - [`acp_cancel`] / [`acp_shutdown`] / [`acp_sessions`] — turn interrupt,
//!   teardown, and live-handle listing.
//!
//! The spawn/handshake/framing logic is tested in `agentcowork-acp`; this
//! module is the app-level state holder + policy seam.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agentcowork_acp::{
    AcpCancelHandle, AcpError, AcpSession, AuthMethod, AvailableCommand, ClientInfo, ConfigOption,
    Distribution, Installer, LaunchRegistry, PermissionDecision, Platform, PolicyVerdict,
    ProcessTransport, PromptContent, PromptOutcome, RegistryClient, RegistryPolicy, ToolCall,
    ToolKind,
};
use agentcowork_core::config::Config;
use agentcowork_core::{ExecutionPhase, ExecutionTrigger, GuardDecision};
use agentcowork_guard::{DecisionPackage, Operation, RiskLevel};
use agentcowork_types::RuntimeControl;
use serde::Serialize;
use tauri::State;

use crate::AppState;

/// Monotonic ACP handle-id source (never reuses an id within a process).
static ACP_COUNTER: AtomicU64 = AtomicU64::new(1);

/// P38 — read the `primary_chief` default (an installed ACP agent id; empty
/// means none is chosen). ADR-0005: there is no built-in fallback, so an
/// unset default resolves to *no agent* rather than to an engine.
/// P53.3 — `known` is the live launch-registry id set, never a hardcoded trio.
#[tauri::command]
pub fn chief_default_get() -> Result<serde_json::Value, String> {
    let cfg = Config::load().map_err(|e| e.to_string())?;
    let known: Vec<String> = launch_registry()
        .agents
        .iter()
        .map(|m| m.id.clone())
        .collect();
    Ok(serde_json::json!({
        "primaryChief": cfg.primary_chief,
        "known": known
    }))
}

/// P53.3 — set the `primary_chief` default. Occupancy is **any installed**
/// agent: the id must be in the launch registry **and** installed (an AgentCowork
/// install record or a PATH-discovered binary). Unknown or not-installed ids
/// are refused fail-closed, and no id is privileged — a typo or a missing
/// binary must never silently fall back to a built-in engine, because none
/// exists (ADR-0005).
#[tauri::command]
pub fn chief_default_set(primary_chief: String) -> Result<String, String> {
    if launch_registry().get(&primary_chief).is_none() {
        return Err(format!(
            "unknown primary_chief {primary_chief:?} — no registered launch path (fail-closed, no silent fallback)"
        ));
    }
    if !agent_installed(&primary_chief) {
        return Err(format!(
            "primary_chief {primary_chief:?} is not installed — install it (F8) or put it on PATH first (fail-closed)"
        ));
    }
    let path = Config::config_path().map_err(|e| e.to_string())?;
    let mut cfg = Config::load().map_err(|e| e.to_string())?;
    cfg.primary_chief = primary_chief.clone();
    cfg.save(&path).map_err(|e| e.to_string())?;
    Ok(primary_chief)
}

/// P53.3 — installed-ness for Chief occupancy: an AgentCowork install record
/// **or** a PATH-discovered binary (the same two legs `acp_install_status`
/// reports — one predicate, no second definition). No id is installed by
/// construction (ADR-0005).
fn install_outcome_usable(outcome: &agentcowork_acp::InstallOutcome) -> bool {
    match outcome.kind.as_str() {
        // A stale pointer is not occupancy. The executable must still be
        // present before Settings or Chief can call this agent installed.
        "binary" | "path" => outcome
            .binary_path
            .as_deref()
            .map(std::path::Path::is_file)
            .unwrap_or(false),
        // npx/uvx are self-installing at launch; readiness means their
        // package manager is available, not that AgentCowork downloaded the
        // package into its own tree.
        "npx" => resolve_on_path("npx").is_some(),
        "uvx" => resolve_on_path("uvx").is_some(),
        _ => false,
    }
}

fn resolve_native_binary(command: &str) -> Option<std::path::PathBuf> {
    resolve_on_path(command).or_else(|| discover_windows_app_path(command))
}

/// P71.3f — the **live** facts readiness needs, read off one launched handle:
/// `(auth_required, has_session)`. A handle only exists when the `initialize`
/// handshake succeeded, which is what makes `ProtocolCompatible` observable.
fn live_facts(handle: &AcpHandle) -> (bool, bool) {
    (handle.auth_required, handle.provider_session_id.is_some())
}

/// P71.3f — the cold readiness derivation (no live session): registry presence,
/// install record, PATH/App-Paths/WSL discovery, package-manager readiness.
/// The derived booleans the older surfaces read (`installed`, `launchable`,
/// `discovered`) are projections of this one state — never a second truth.
///
/// `Failed`/`Unavailable` are reachable only from attempt records the shell
/// does not keep yet (a failed launch is reported to the caller today), so the
/// cold path never claims them.
pub(crate) fn agent_readiness(agent_id: &str) -> agentcowork_types::AgentReadiness {
    use agentcowork_types::AgentReadiness;
    // ADR-0005 — no built-in row ships, so nothing is `Ready` by construction;
    // every id is probed like any other external agent.
    let registry = launch_registry();
    let Some(manifest) = registry.get(agent_id) else {
        // An id no registry knows is not "discovered": nothing was found.
        return AgentReadiness::Unknown;
    };
    if installer()
        .installed(agent_id)
        .is_some_and(|o| install_outcome_usable(&o))
    {
        return AgentReadiness::Installed;
    }
    // Path/App-Paths/WSL discovery or a self-installing package manager means
    // the runtime can actually be started here.
    let launchable = match &manifest.distribution {
        Distribution::Binary { command, .. } => {
            !command.is_empty() && resolve_native_binary(command).is_some()
        }
        Distribution::Npx { .. } => resolve_on_path("npx").is_some(),
        Distribution::Uvx { .. } => resolve_on_path("uvx").is_some(),
    };
    if launchable {
        return AgentReadiness::Launchable;
    }
    AgentReadiness::Discovered
}

/// P71.3f — cold facts plus the live handshake state. A live handle outranks
/// install facts: the process negotiated, so readiness is at least
/// `ProtocolCompatible`; `auth_required` on the handle is `AuthRequired`; a
/// negotiated session is `Ready`.
pub(crate) fn agent_readiness_with_live(
    agent_id: &str,
    live: Option<(bool, bool)>,
) -> agentcowork_types::AgentReadiness {
    use agentcowork_types::AgentReadiness;
    match live {
        Some((true, _)) => AgentReadiness::AuthRequired,
        Some((false, true)) => AgentReadiness::Ready,
        Some((false, false)) => AgentReadiness::ProtocolCompatible,
        None => agent_readiness(agent_id),
    }
}

/// P71.3f — the shell's mounted [`agentcowork_core::tools::AgentReadinessSource`]:
/// install/discovery facts plus the live ACP handshake state from
/// `AppState::acp_sessions`. This is the one place the picker, the delegation
/// gate and the trigger plane's doctor read agent readiness from.
pub(crate) struct ShellAgentReadiness {
    pub(crate) sessions: Arc<std::sync::Mutex<std::collections::HashMap<String, AcpHandle>>>,
}

/// P71.3f — the trigger plane's doctor reads readiness through this check
/// (`ARCH/AUTOMATION.md` §9). `ok` means at least one agent is `Ready`; the
/// detail counts each rung so a support pass sees *which* rung is missing
/// rather than a single opaque boolean.
pub(crate) fn agents_doctor_check(
    sessions: Arc<std::sync::Mutex<std::collections::HashMap<String, AcpHandle>>>,
) -> agentcowork_core::CronCheck {
    use agentcowork_core::tools::AgentReadinessSource;
    use agentcowork_types::AgentReadiness;
    let source = ShellAgentReadiness { sessions };
    let registry = launch_registry();
    let (mut ready, mut launchable, mut needs_auth, mut discovered) = (0usize, 0, 0, 0);
    for manifest in &registry.agents {
        match source.readiness(&manifest.id) {
            state if state.is_ready() => ready += 1,
            state if state.needs_auth() => needs_auth += 1,
            state if state.is_launchable() => launchable += 1,
            _ => discovered += 1,
        }
    }
    let unavailable: Vec<&str> = registry
        .agents
        .iter()
        .filter(|m| source.readiness(&m.id) == AgentReadiness::Unknown)
        .map(|m| m.id.as_str())
        .collect();
    agentcowork_core::CronCheck {
        name: "agents".to_string(),
        ok: ready > 0,
        detail: format!(
            "{ready} ready · {launchable} launchable · {needs_auth} auth-required · \
             {discovered} discovered{} — an unready agent blocks its firings with the state \
             as the reason, never a silent model swap",
            if unavailable.is_empty() {
                String::new()
            } else {
                format!(
                    " · {} unprobed ({})",
                    unavailable.len(),
                    unavailable.join(", ")
                )
            }
        ),
    }
}

impl agentcowork_core::tools::AgentReadinessSource for ShellAgentReadiness {
    fn readiness(&self, agent_id: &str) -> agentcowork_types::AgentReadiness {
        let live = self.sessions.lock().ok().and_then(|sessions| {
            sessions
                .values()
                .find(|h| h.agent_id == agent_id)
                .map(live_facts)
        });
        agent_readiness_with_live(agent_id, live)
    }
}

/// P71.3f — occupancy is now a **projection** of the readiness state, so the
/// answer to "is this agent installed?" and "is this agent ready?" cannot
/// disagree (the old pair could: `installed` was true for agents that could
/// not run).
pub(crate) fn agent_installed(agent_id: &str) -> bool {
    agent_readiness(agent_id).is_installed()
}

/// The launch registry the runtime actually resolves against: the curated
/// builtin seed **merged with the cached official ACP registry**
/// (`registry.json`, cached under `<data_dir>/agents` by [`registry_client`]).
///
/// This is the one place the dynamic catalog enters the runtime. The two
/// facts stay separate: the merged registry is the *catalog* (which agents
/// exist, and how to spawn them), while occupancy is [`agent_installed`]
/// (an AgentCowork install record or a PATH-discovered binary). A registry
/// entry therefore never becomes a selectable/usable agent by itself.
///
/// No cache (never fetched, or offline before the first fetch) ⇒ the curated
/// seed, so the shell degrades to the builtin list instead of failing.
///
/// The merge is memoised on the cache file's mtime: this helper is called in
/// loops (once per agent row in `acp_install_status` / `chief_subagents`), and
/// re-parsing the registry JSON per row would be a real cost. A refresh that
/// rewrites `registry.json` changes the mtime and invalidates the memo, so a
/// newly fetched catalog is picked up on the next read without a restart.
pub(crate) fn launch_registry() -> LaunchRegistry {
    let client = registry_client();
    let stamp = std::fs::metadata(client.cache_dir().join("registry.json"))
        .and_then(|m| m.modified())
        .ok();
    if let Ok(memo) = LAUNCH_REGISTRY_MEMO.lock() {
        if let Some((cached_stamp, reg)) = memo.as_ref() {
            if *cached_stamp == stamp {
                return reg.clone();
            }
        }
    }
    let mut reg = LaunchRegistry::builtin();
    if let Some(snap) = client.load_cached() {
        snap.index.merge_into(&mut reg, Platform::current());
    }
    if let Ok(mut memo) = LAUNCH_REGISTRY_MEMO.lock() {
        *memo = Some((stamp, reg.clone()));
    }
    reg
}

/// Memo for [`launch_registry`]: `(cache-file mtime, merged registry)`.
static LAUNCH_REGISTRY_MEMO: std::sync::Mutex<
    Option<(Option<std::time::SystemTime>, LaunchRegistry)>,
> = std::sync::Mutex::new(None);

/// The canonical owner of one ACP turn path.
///
/// `session_id` is the AgentCowork Session identity supplied by the caller. The
/// provider's own ACP session id is deliberately not part of this value; it
/// lives on [`AcpHandle::provider_session_id`] and in the durable binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AcpCanonicalOwner {
    pub session_id: String,
    pub work_id: String,
    pub binding_id: String,
}

/// The typed identity/lifecycle failures that must stop a turn before provider
/// I/O. Keeping these distinct from ACP protocol errors prevents a missing
/// Work/AgentBinding bridge from silently degrading into an unowned prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AcpIdentityError {
    MissingApplicationSession,
    MissingProviderSession,
    WorkGatewayUnavailable,
    Work(String),
    Execution(String),
    Binding(String),
    OwnerMismatch {
        handle: String,
        expected_session: String,
        actual_session: String,
    },
    WorkOwnerMismatch {
        work_id: String,
        expected_session: String,
        actual_session: Option<String>,
    },
    BindingOwnerMismatch {
        binding_id: String,
        expected_session: String,
        actual_session: String,
    },
    BindingMismatch {
        handle: String,
        expected_binding: String,
        actual_binding: String,
    },
    ProviderSessionConflict {
        binding_id: String,
        expected: String,
        actual: Option<String>,
    },
    NoCanonicalOwner {
        handle: String,
    },
}

impl std::fmt::Display for AcpIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingApplicationSession => {
                write!(f, "ACP turn requires an application Session id")
            }
            Self::MissingProviderSession => {
                write!(f, "ACP handle has no provider session id")
            }
            Self::WorkGatewayUnavailable => {
                write!(f, "sidecar Work Gateway unavailable; refusing an unowned ACP turn")
            }
            Self::Work(message) => write!(f, "canonical Work unavailable: {message}"),
            Self::Execution(message) => write!(f, "canonical Run unavailable: {message}"),
            Self::Binding(message) => write!(f, "AgentBinding unavailable: {message}"),
            Self::OwnerMismatch {
                handle,
                expected_session,
                actual_session,
            } => write!(
                f,
                "ACP handle {handle} belongs to Session {actual_session}, not requested Session {expected_session}"
            ),
            Self::WorkOwnerMismatch {
                work_id,
                expected_session,
                actual_session,
            } => write!(
                f,
                "Work {work_id} is owned by {:?}, not requested Session {expected_session}",
                actual_session.as_deref().unwrap_or("<none>")
            ),
            Self::BindingOwnerMismatch {
                binding_id,
                expected_session,
                actual_session,
            } => write!(
                f,
                "AgentBinding {binding_id} belongs to Session {actual_session}, not requested Session {expected_session}"
            ),
            Self::BindingMismatch {
                handle,
                expected_binding,
                actual_binding,
            } => write!(
                f,
                "ACP handle {handle} is bound to AgentBinding {actual_binding}, not requested {expected_binding}"
            ),
            Self::ProviderSessionConflict {
                binding_id,
                expected,
                actual,
            } => write!(
                f,
                "AgentBinding {binding_id} is attached to provider session {:?}, not {expected}",
                actual.as_deref().unwrap_or("<none>")
            ),
            Self::NoCanonicalOwner { handle } => {
                write!(f, "ACP handle {handle} has no canonical Session/Work/Binding owner")
            }
        }
    }
}

impl std::error::Error for AcpIdentityError {}

/// Per-handle session storage. The small wrapper keeps the provider session
/// mutex separate from the global handle map and exposes the independent
/// cancellation writer used by stop/cancel paths.
pub(crate) struct AcpSessionSlot {
    inner: Arc<Mutex<AcpSession<ProcessTransport>>>,
    cancel: AcpCancelHandle,
}

impl AcpSessionSlot {
    fn new(session: AcpSession<ProcessTransport>) -> Self {
        let cancel = session.cancellation_handle();
        Self {
            inner: Arc::new(Mutex::new(session)),
            cancel,
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, AcpSession<ProcessTransport>>, String> {
        self.inner.lock().map_err(|e| e.to_string())
    }

    fn cancellation_handle(&self) -> AcpCancelHandle {
        self.cancel.clone()
    }
}

impl Clone for AcpSessionSlot {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            cancel: self.cancel.clone(),
        }
    }
}

/// A live ACP agent session + the id it was launched under.
pub(crate) struct AcpHandle {
    pub agent_id: String,
    /// The workspace dir the agent session was created in (retained so
    /// [`acp_authenticate`] can retry `session/new` after login).
    pub cwd: String,
    /// True when `session/new` answered `auth_required` — the user must sign
    /// in before the handle can drive prompts.
    pub auth_required: bool,
    /// The methods the agent advertised in `initialize` (`authMethods`).
    pub auth_methods: Vec<AuthMethod>,
    /// P53.8 — capabilities advertised by the agent at initialize.
    pub embedded_context: bool,
    /// P53.1 — the agent's last advertised slash vocabulary (from the most
    /// recent `available_commands_update` on this handle; empty until the
    /// agent sends one). Served to the composer via `acp_session_commands`.
    pub available_commands: Vec<AvailableCommand>,
    /// Complete agent-owned config option state. This is intentionally
    /// separate from AgentCowork Native provider/model state.
    pub config_options: Vec<ConfigOption>,
    /// P69.E9 — the I16 prefix-stability guard: fingerprints the shell-owned
    /// stable prefix each turn and classifies turn-to-turn changes (stable /
    /// declared / undeclared mutation) into the per-session observability log
    /// (`ARCH/CONTEXT.md` §4: prefix mutations must be intentional AND
    /// observable).
    pub prefix_guard: agentcowork_acp::PrefixGuard,
    /// The provider-native ACP session identity. It is never used as the
    /// AgentCowork Session id and is persisted only on the AgentBinding.
    pub provider_session_id: Option<String>,
    /// The canonical owner claimed by the first prompt. A handle is valid only
    /// for this Session/Work/Binding tuple once claimed.
    pub owner: Option<AcpCanonicalOwner>,
    /// The current durable Run id, replaced for each prompt on this Work.
    pub run_id: Option<String>,
    /// The provider session is per-handle state. The Arc lets a prompt hold
    /// only this handle's mutex across blocking provider I/O; the global map
    /// mutex is released before prompt/approval work begins.
    pub session: AcpSessionSlot,
    /// A lock-free cancellation hook for the provider session.
    pub cancel: AcpCancelHandle,
}

impl AcpHandle {
    pub(crate) fn application_session_id(&self) -> Option<&str> {
        self.owner.as_ref().map(|owner| owner.session_id.as_str())
    }

    pub(crate) fn work_id(&self) -> Option<&str> {
        self.owner.as_ref().map(|owner| owner.work_id.as_str())
    }

    pub(crate) fn binding_id(&self) -> Option<&str> {
        self.owner.as_ref().map(|owner| owner.binding_id.as_str())
    }
}

/// One launched-session summary for the picker/harness list.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpHandleInfo {
    pub(crate) handle: String,
    agent_id: String,
    agent_name: String,
    /// The provider-native ACP session id. Retained under the historical field
    /// name for existing callers; it is not the application Session id.
    session_id: String,
    #[serde(default)]
    provider_session_id: String,
    #[serde(default)]
    application_session_id: String,
    #[serde(default)]
    work_id: String,
    #[serde(default)]
    binding_id: String,
    #[serde(default)]
    run_id: String,
    protocol: String,
    /// True when the agent needs authentication before it will accept a
    /// session (the UI renders the "Sign in" surface from `authMethods`).
    #[serde(default)]
    auth_required: bool,
    #[serde(default)]
    auth_methods: Vec<AuthMethod>,
    #[serde(default)]
    embedded_context: bool,
    #[serde(default)]
    config_options: Vec<ConfigOption>,
}

impl From<(&AcpHandle, &str)> for AcpHandleInfo {
    fn from((h, handle): (&AcpHandle, &str)) -> Self {
        AcpHandleInfo {
            handle: handle.to_string(),
            agent_id: h.agent_id.clone(),
            agent_name: h.agent_id.clone(),
            session_id: h.provider_session_id.clone().unwrap_or_default(),
            provider_session_id: h.provider_session_id.clone().unwrap_or_default(),
            application_session_id: h.application_session_id().unwrap_or_default().to_string(),
            work_id: h.work_id().unwrap_or_default().to_string(),
            binding_id: h.binding_id().unwrap_or_default().to_string(),
            run_id: h.run_id.clone().unwrap_or_default(),
            protocol: "acp".to_string(),
            auth_required: h.auth_required,
            auth_methods: h.auth_methods.clone(),
            embedded_context: h.embedded_context,
            config_options: h.config_options.clone(),
        }
    }
}

/// The launch registry (the agent picker). Default = inbuilt AgentCowork.
///
/// P50.3.9 — governance truth: every agent row carries an explicit
/// `governance` classification so the picker and the work transcript never
/// imply AgentCowork audit coverage for effects an external agent performs
/// inside its own process.
/// - `GovernedMediated` — every effect flows through the AgentCowork executor
///   (Guard-2 ticket → receipt on the one audit trail). No registry row is
///   this today: it belonged to the retired built-in engine, and an external
///   agent's own tools never cross Guard — the class stays in the vocabulary
///   for the post-v1 governed baseline (ADR-0005 §3 has the honest split).
/// - `SelfContained` — the agent's `session/request_permission` requests are
///   answered by the shared GuardService (mediated at the ACP boundary), but
///   effects the agent performs internally (its own shell, files, network)
///   are **outside** the AgentCowork audit trail. Honest label for ACP
///   harnesses like Claude Code / Codex.
/// - `NotGoverned` — neither of the above; no AgentCowork coverage. (Registry
///   agents that neither mediate permissions nor route effects; the picker
///   must render the row as un-audited.)
#[tauri::command]
pub fn acp_agents() -> Vec<serde_json::Value> {
    launch_registry()
        .agents
        .iter()
        .map(|m| {
            // ADR-0005: every registry row is an external ACP harness, so every
            // row is `SelfContained` — nothing is claimed as fully governed
            // (I14/I15: authority does not leak across the seam).
            let (class, audited_effects, note) = (
                "SelfContained",
                false,
                "Permission requests are mediated by Guard-2, but effects performed inside the agent's own process (shell, files, network) are outside the AgentCowork audit trail.",
            );
            let mut v = serde_json::to_value(m).unwrap_or(serde_json::Value::Null);
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "governance".into(),
                    serde_json::json!({
                        "class": class,
                        "auditedEffects": audited_effects,
                        "note": note,
                    }),
                );
            }
            v
        })
        .collect()
}

/// F8 — refresh the official ACP registry cache (`registry.json` from the
/// CDN). Returns the catalog status; the app stays on the builtin seed if the
/// network fails.
#[tauri::command]
pub fn acp_registry_refresh() -> Result<serde_json::Value, String> {
    let client = registry_client();
    let snap = client.refresh().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "version": snap.index.version,
        "agentCount": snap.index.agents.len(),
        "fetchedAtMs": snap.fetched_at_ms,
        "fromCache": snap.from_cache,
        "cacheDir": client.cache_dir(),
    }))
}

/// F8 — the cached registry status (no network). `null` if never cached.
#[tauri::command]
pub fn acp_registry_status() -> Result<Option<serde_json::Value>, String> {
    let client = registry_client();
    Ok(client.load_cached().map(|s| {
        serde_json::json!({
            "version": s.index.version,
            "agentCount": s.index.agents.len(),
            "fetchedAtMs": s.fetched_at_ms,
        })
    }))
}

/// F8 — the exact install plan for a registry agent on this platform, plus
/// the trust/ToS policy verdict (plan-before-touch: this is what the
/// Guard-2-ticketed installer would do).
#[tauri::command]
pub fn acp_registry_install_plan(agent_id: String) -> Result<serde_json::Value, String> {
    let client = registry_client();
    let snap = client
        .load_or_refresh()
        .ok_or_else(|| "no registry catalog available (offline and not cached)".to_string())?;
    let spec = snap
        .index
        .install_plan(&agent_id, Platform::current())
        .ok_or_else(|| format!("no install plan for this platform on {agent_id}"))?;
    let verdict = RegistryPolicy::builtin().evaluate(&agent_id, &spec.license);
    Ok(serde_json::json!({ "spec": spec, "policy": verdict.as_str() }))
}

/// The F8 registry cache dir: `<data_dir>/agents`.
fn registry_client() -> RegistryClient {
    RegistryClient::new(agentcowork_core::default_data_dir().join("agents"))
}

/// How old a cached registry must be before the boot job refetches it. The
/// upstream job publishes hourly; 6h keeps us current for a desktop app
/// without polling someone else's CDN every hour.
const REGISTRY_STALE_SECS: u64 = 6 * 60 * 60;
/// How often the job re-checks staleness (no network unless stale).
const REGISTRY_RECHECK_SECS: u64 = 60 * 60;

/// P60.14 — the ACP registry boot job.
///
/// The contract is a **dynamically consumed** catalog with a local
/// last-known-good cache: discovery must not depend on the user remembering to
/// press “Discover more”. So the shell refreshes the official registry when the
/// cache is missing or stale (mirroring the P56.1 models.dev job, whose cadence
/// is configurable in Settings), then re-checks hourly so an app left running
/// picks up new agents without a restart.
///
/// Failure is silent and non-fatal: a failed fetch keeps the cached catalog,
/// and with no cache at all the resolver degrades to the curated seed. Never
/// blocks the UI thread; writes only the registry cache (`registry.json` +
/// `registry.meta.json`) and never touches the vault.
pub fn spawn_registry_refresh_job() {
    std::thread::spawn(|| loop {
        let client = registry_client();
        let stale = client
            .load_cached()
            .map(|s| {
                let age_ms = now_ms().saturating_sub(s.fetched_at_ms);
                age_ms > REGISTRY_STALE_SECS * 1000
            })
            .unwrap_or(true);
        if stale {
            // Best-effort: an offline boot keeps whatever is cached.
            let _ = client.refresh();
        }
        std::thread::sleep(std::time::Duration::from_secs(REGISTRY_RECHECK_SECS));
    });
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The F8 install root: `<data_dir>/agents` (registry cache + installed
/// binaries + install-state pointers share the directory).
fn installer() -> Installer {
    Installer::new(agentcowork_core::default_data_dir().join("agents"))
}

/// Resolve the current install plan for a registry agent (shared by the
/// install request/commit halves so the args-hash is deterministic).
fn resolve_spec(agent_id: &str) -> Result<agentcowork_acp::InstallSpec, String> {
    let client = registry_client();
    let snap = client
        .load_or_refresh()
        .ok_or_else(|| "no registry catalog available (offline and not cached)".to_string())?;
    snap.index
        .get(agent_id)
        .ok_or_else(|| format!("unknown registry agent: {agent_id}"))?;
    snap.index
        .install_plan(agent_id, Platform::current())
        .ok_or_else(|| format!("no install plan for this platform on {agent_id}"))
        .map(|mut spec| {
            // Pin the extract destination so the decision card shows exactly
            // where the bytes land (`<data_dir>/agents/<id>/<version>`).
            spec.install_dir = Some(
                agentcowork_core::default_data_dir()
                    .join("agents")
                    .join(agent_id)
                    .join(&spec.version),
            );
            spec
        })
}

/// Probe PATH for an executable name. This is the auto-discovery half: an
/// agent CLI the user installed themselves (Claude Code via npm, Codex, …)
/// shows up as installed without AgentCowork ever downloading it. On Windows,
/// npm-global CLIs are `.cmd`/`.bat` shims and native tools are `.exe`, so
/// all three are probed there.
fn resolve_on_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat"] {
            let mut p = cand.clone();
            p.set_extension(ext);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Read a Windows App Paths registration without invoking a shell. App Paths
/// is a discovery source only: its result is never treated as a managed
/// AgentCowork install and is never persisted as an install record.
#[cfg(windows)]
fn discover_windows_app_path(command: &str) -> Option<std::path::PathBuf> {
    use std::process::Command;

    let exe = command
        .strip_suffix(".exe")
        .or_else(|| command.strip_suffix(".EXE"))
        .unwrap_or(command);
    let exe = format!("{exe}.exe");
    for hive in ["HKCU", "HKLM"] {
        let key =
            format!(r"{hive}\\Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{exe}");
        let output = Command::new("reg")
            .args(["query", &key, "/ve"])
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            if let Some((_, value)) = line.split_once("REG_SZ") {
                let value = value.trim().trim_matches('"');
                let path = std::path::PathBuf::from(value);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn discover_windows_app_path(_command: &str) -> Option<std::path::PathBuf> {
    None
}

/// Discover a CLI installed only inside WSL. This is intentionally separate
/// from Windows PATH/App Paths: the returned Linux path must be launched via
/// `wsl.exe -d <distro> -- <command>`, never as a Windows executable.
#[cfg(windows)]
fn discover_wsl_path(command: &str) -> Option<(String, String)> {
    use std::process::Command;

    let output = Command::new("wsl.exe").args(["-l", "-q"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let distros = String::from_utf8_lossy(&output.stdout);
    for distro in distros.lines().map(str::trim).filter(|d| !d.is_empty()) {
        let output = Command::new("wsl.exe")
            .args(["-d", distro, "--", "which", command])
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        let linux_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !linux_path.is_empty() && !linux_path.contains('\\') {
            return Some((distro.to_string(), linux_path));
        }
    }
    None
}

#[cfg(not(windows))]
fn discover_wsl_path(_command: &str) -> Option<(String, String)> {
    None
}

/// P66.1 — Dedicated WSL spawn adapter: Translates a WSL-discovered command into
/// a `wsl.exe -d <distro> -- <linux_path>` execution tuple.
pub fn resolve_wsl_spawn(command: &str) -> Option<(String, Vec<String>)> {
    discover_wsl_path(command).map(|(distro, linux_path)| {
        (
            "wsl.exe".to_string(),
            vec!["-d".to_string(), distro, "--".to_string(), linux_path],
        )
    })
}

/// Build the public, non-secret runtime location record consumed by Settings
/// and the picker. Catalog membership is never used as occupancy evidence.
pub(crate) fn runtime_location_json(
    manifest: &agentcowork_acp::HarnessManifest,
    install: Option<&agentcowork_acp::InstallOutcome>,
) -> serde_json::Value {
    if let Some(o) = install {
        let kind = match o.kind.as_str() {
            "path" => {
                if cfg!(windows) {
                    "windows_path"
                } else {
                    "path"
                }
            }
            "binary" => "managed",
            "npx" => "package_manager",
            "uvx" => "package_manager",
            _ => "unavailable",
        };
        return serde_json::json!({
            "kind": kind,
            "source": if o.kind == "path" { "path_probe" } else { "agentcowork_install" },
            "executable": o.binary_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "version": if o.version == "path" { serde_json::Value::Null } else { serde_json::json!(o.version) },
            "verifiedAt": serde_json::Value::Null,
        });
    }

    match &manifest.distribution {
        Distribution::Binary { command, .. } if !command.is_empty() => {
            if let Some(path) = resolve_on_path(command) {
                return serde_json::json!({
                    "kind": if cfg!(windows) { "windows_path" } else { "path" },
                    "source": "path_probe",
                    "executable": path.to_string_lossy(),
                    "version": serde_json::Value::Null,
                    "verifiedAt": serde_json::Value::Null,
                });
            }
            if let Some(path) = discover_windows_app_path(command) {
                return serde_json::json!({
                    "kind": "windows_path",
                    "source": "app_paths",
                    "executable": path.to_string_lossy(),
                    "version": serde_json::Value::Null,
                    "verifiedAt": serde_json::Value::Null,
                });
            }
            if let Some((distro, linux_path)) = discover_wsl_path(command) {
                return serde_json::json!({
                    "kind": "wsl",
                    "source": "wsl_probe",
                    "distro": distro,
                    "linuxPath": linux_path,
                    "windowsLauncher": "wsl.exe",
                    "version": serde_json::Value::Null,
                    "verifiedAt": serde_json::Value::Null,
                });
            }
            serde_json::json!({ "kind": "unavailable", "source": "path_probe", "reason": "executable not found on PATH, Windows App Paths, or WSL" })
        }
        Distribution::Npx { package, .. } => {
            if let Some(path) = resolve_on_path("npx") {
                serde_json::json!({ "kind": "package_manager", "source": "path_probe", "manager": "npx", "command": path.to_string_lossy(), "package": package })
            } else {
                serde_json::json!({ "kind": "unavailable", "source": "path_probe", "reason": "npx is not available on the effective PATH" })
            }
        }
        Distribution::Uvx { package, .. } => {
            if let Some(path) = resolve_on_path("uvx") {
                serde_json::json!({ "kind": "package_manager", "source": "path_probe", "manager": "uvx", "command": path.to_string_lossy(), "package": package })
            } else {
                serde_json::json!({ "kind": "unavailable", "source": "path_probe", "reason": "uvx is not available on the effective PATH" })
            }
        }
        Distribution::Binary { .. } => {
            serde_json::json!({ "kind": "unavailable", "source": "registry_catalog", "reason": "manifest has no executable command" })
        }
    }
}

/// P65.2 — the one occupancy-provenance builder for Settings: install record
/// or PATH/App-Paths/WSL probe for one agent id, mapped to the location JSON.
/// A row whose distribution has no executable reports `unavailable` here; no
/// agent ships with the app in v1 (ADR-0005).
pub(crate) fn runtime_location_for(agent_id: &str) -> serde_json::Value {
    let registry = launch_registry();
    let Some(manifest) = registry.get(agent_id) else {
        return serde_json::json!({ "kind": "unavailable", "reason": "unknown agent id" });
    };
    let installed = installer()
        .installed(agent_id)
        .filter(install_outcome_usable);
    runtime_location_json(manifest, installed.as_ref())
}

/// F8/P66 — install state plus exact non-secret runtime provenance for every
/// registry agent. Managed installs, user PATH/App Paths discoveries, and
/// npx/uvx readiness are distinct. WSL is reported as a separate discovery
/// location and is never treated as a native Windows executable.
#[tauri::command]
pub fn acp_install_status(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let registry = launch_registry();
    let inst = installer();
    let mut out = serde_json::Map::new();
    for m in &registry.agents {
        let mut installed = inst.installed(&m.id).filter(install_outcome_usable);
        if installed.is_none() {
            if let Distribution::Binary { command, .. } = &m.distribution {
                if !command.is_empty() {
                    if let Some(path) = resolve_native_binary(command) {
                        // Persist the exact user-owned path so later launches
                        // do not depend on a mutable child PATH.
                        let _ = inst.record_path(&m.id, &path);
                        installed = inst.installed(&m.id);
                    }
                }
            }
        }
        let location = runtime_location_json(m, installed.as_ref());
        let location_kind = location
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unavailable");
        let package_manager_ready = matches!(location_kind, "package_manager");
        let discovered = location_kind != "unavailable";
        // P71.3f — one readiness state; the three booleans below are its
        // projections (kept because older surfaces read them), not parallel
        // truths. A live handle outranks install facts.
        let readiness = {
            use agentcowork_types::AgentReadiness;
            let live = state.acp_sessions.lock().ok().and_then(|sessions| {
                sessions
                    .values()
                    .find(|h| h.agent_id == m.id)
                    .map(live_facts)
            });
            match live {
                Some((true, _)) => AgentReadiness::AuthRequired,
                Some((false, true)) => AgentReadiness::Ready,
                Some((false, false)) => AgentReadiness::ProtocolCompatible,
                None if installed.is_some() => AgentReadiness::Installed,
                None if package_manager_ready
                    || matches!(location_kind, "path" | "windows_path" | "wsl") =>
                {
                    AgentReadiness::Launchable
                }
                None => AgentReadiness::Discovered,
            }
        };
        // App Paths, PATH, and WSL (via dedicated WSL spawn adapter) are launchable.
        let launchable = readiness.is_launchable();
        out.insert(
            m.id.clone(),
            serde_json::json!({
                "readiness": readiness.as_str(),
                "installed": readiness.is_installed(),
                "discovered": discovered,
                "launchable": launchable,
                "version": installed.as_ref().and_then(|o| if o.version == "path" { None } else { Some(o.version.clone()) }),
                "kind": installed.as_ref().map(|o| o.kind.clone()).or_else(|| package_manager_ready.then(|| "package_manager".to_string())),
                "binaryPath": installed.as_ref().and_then(|o| o.binary_path.as_ref().map(|p| p.to_string_lossy().into_owned())),
                "location": location,
            }),
        );
    }
    Ok(serde_json::Value::Object(out))
}

/// F8 — the **install request** half (plan-before-touch). Resolves the
/// platform install plan, applies the trust gate (denylist refuses outright),
/// then routes through the shared Guard-2 [`GuardService`]: allow-listed /
/// open-license agents auto-allow, everything else mints a ticket whose card
/// renders the full decision package (goal, paths, download URL, sha256).
/// **Nothing is downloaded here** — [`acp_install_commit`] is the executor.
#[tauri::command]
pub fn acp_install_request(
    state: State<'_, AppState>,
    agent_id: String,
) -> Result<serde_json::Value, String> {
    let spec = resolve_spec(&agent_id)?;
    let verdict = RegistryPolicy::builtin().evaluate(&agent_id, &spec.license);
    if verdict == PolicyVerdict::Block {
        return Err(format!("agent {agent_id} is blocked by policy"));
    }

    // The decision card: exactly what the download will do.
    let mut decision = DecisionPackage::new(format!(
        "Install {} v{} (F8 registry)",
        spec.name, spec.version
    ))
    .with_risk(RiskLevel::Medium)
    .with_paths(vec![spec
        .install_dir
        .clone()
        .unwrap_or_else(|| {
            agentcowork_core::default_data_dir()
                .join("agents")
                .join(&agent_id)
        })
        .to_string_lossy()
        .into_owned()]);
    let exact_command: Vec<String> = match &spec.kind {
        agentcowork_acp::InstallKind::Npx { package, .. } => {
            vec!["npx".into(), "-y".into(), package.clone()]
        }
        agentcowork_acp::InstallKind::Uvx { package, .. } => {
            vec!["uvx".into(), package.clone()]
        }
        agentcowork_acp::InstallKind::Binary {
            archive, sha256, ..
        } => {
            vec![
                "agentcowork-installer".into(),
                "download".into(),
                archive.clone(),
                format!("sha256:{sha256}"),
            ]
        }
    };
    agentcowork_core::exact_command_consent(&exact_command).map_err(|e| e.to_string())?;

    decision = match &spec.kind {
        agentcowork_acp::InstallKind::Npx { package, .. } => decision
            .with_script(vec![format!("npx -y {package}")], "npx")
            .with_network(vec!["registry.npmjs.org".into()]),
        agentcowork_acp::InstallKind::Uvx { package, .. } => decision
            .with_script(vec![format!("uvx {package}")], "uvx")
            .with_network(vec!["pypi.org".into()]),
        agentcowork_acp::InstallKind::Binary {
            archive, sha256, ..
        } => {
            let host = url_host(archive);
            decision
                .with_script(
                    vec![
                        format!("download {archive}"),
                        format!("sha256 verify {sha256}"),
                        format!(
                            "extract → {}",
                            spec.install_dir
                                .as_ref()
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        ),
                    ],
                    "agentcowork-installer",
                )
                .with_network(vec![host])
        }
    };

    let args_hash = install_args_hash(&agent_id, &spec.version);
    let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
    match guard.evaluate(
        "install",
        &agent_id,
        "acp.install",
        Operation::GenericWrite,
        decision,
        &args_hash,
        0,
    ) {
        // P69.C12 — both verdicts carry the evidence the consent surface must
        // show: the license (and its registry-published URL), the verdict, the
        // source, and the reason. `ask` is the common path for proprietary
        // registry agents; it must never be silent.
        GuardDecision::Allow { ticket_id } => Ok(serde_json::json!({
            "action": "allow",
            "agentId": agent_id,
            "version": spec.version,
            // Auto-allowed still carries a (pre-approved) single-use ticket —
            // the executor consumes it in `acp_install_commit` either way.
            "ticketId": ticket_id,
            "exactCommand": exact_command,
            "consentRequired": true,
            "preferNative": matches!(spec.kind, agentcowork_acp::InstallKind::Binary { .. }),
            "license": spec.license,
            "licenseUrl": spec.license_url,
            "verdict": "allow",
            "reason": "allow-listed curated agent or an open license",
            "source": "acp-registry",
        })),
        GuardDecision::Ask { ticket_id } => Ok(serde_json::json!({
            "action": "ask",
            "agentId": agent_id,
            "version": spec.version,
            "ticketId": ticket_id,
            "exactCommand": exact_command,
            "consentRequired": true,
            "preferNative": matches!(spec.kind, agentcowork_acp::InstallKind::Binary { .. }),
            "license": spec.license,
            "licenseUrl": spec.license_url,
            "verdict": "ask",
            "reason": "no allow-list entry for this agent — one explicit consent is required",
            "source": "acp-registry",
        })),
        GuardDecision::Block { reason } => Err(format!("install blocked: {reason}")),
    }
}

/// P69.C12 — the **consent wait** half: block until the user answers the
/// Guard-2 card for an `ask` install (approve in the dedicated guard window),
/// then report honestly. The caller commits with the same single-use ticket
/// when — and only when — the answer was approval; a rejection or a timeout
/// returns `approved: false` (never a fabricated success), so the picker can
/// surface the visible reason instead of stalling.
#[tauri::command]
pub fn acp_install_await(
    state: State<'_, AppState>,
    ticket_id: String,
    timeout_ms: Option<u64>,
) -> Result<serde_json::Value, String> {
    let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
    let approved = guard.wait_ticket(
        &ticket_id,
        std::time::Duration::from_millis(timeout_ms.unwrap_or(120_000)),
    );
    Ok(serde_json::json!({
        "approved": approved,
        "reason": if approved {
            "approved in the Guard window"
        } else {
            "not approved — declined or timed out; nothing was installed"
        },
    }))
}

/// F8 — the **install executor** (the "touch" half). Consumes the Guard-2
/// ticket (**mandatory** — `use_ticket` enforces approval + single-use +
/// args-hash), then executes the plan: binary agents download → sha256-verify
/// → extract; npx/uvx agents record the pin. The user's explicit click
/// satisfied an `Ask` verdict by approving the card; an auto-allowed (`allow`)
/// request carries a pre-approved ticket that is still consumed here.
#[tauri::command]
pub fn acp_install_commit(
    state: State<'_, AppState>,
    agent_id: String,
    ticket_id: String,
) -> Result<serde_json::Value, String> {
    let spec = resolve_spec(&agent_id)?;
    let args_hash = install_args_hash(&agent_id, &spec.version);
    let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
    guard
        .use_ticket(&ticket_id, &args_hash)
        .map_err(|e| format!("install ticket not consumable: {e}"))?;
    drop(guard);

    let outcome = installer().install(&spec).map_err(|e| e.to_string())?;
    let audit_seq = crate::control::record_turn(
        &state,
        crate::control::AuthKind::AgentTicket,
        &[(
            "acp.install",
            serde_json::json!({
                "agentId": outcome.agent_id,
                "version": outcome.version,
                "ticketId": ticket_id,
            }),
        )],
    )
    .first()
    .copied()
    .unwrap_or(0);
    Ok(serde_json::json!({
        "agentId": outcome.agent_id,
        "version": outcome.version,
        "kind": outcome.kind,
        "binaryPath": outcome.binary_path.map(|p| p.to_string_lossy().into_owned()),
        "env": outcome.env,
        "auditSeq": audit_seq,
        // The agent's own auth (subscription OAuth / API key) is surfaced from
        // the ACP `initialize` handshake's `authMethods` on first launch.
        "auth": "surfaced at launch via ACP authMethods",
    }))
}

/// Legacy one-shot install kept for callers that already resolved the ticket
/// (or for allow-listed agents): resolves the plan, mints the ticket if
/// policy asks, and returns `{action, ticketId?}` without touching the disk —
/// the caller then invokes [`acp_install_commit`]. Mirrors
/// [`acp_install_request`] exactly.
#[tauri::command]
pub fn acp_install(
    state: State<'_, AppState>,
    agent_id: String,
) -> Result<serde_json::Value, String> {
    acp_install_request(state, agent_id)
}

/// P66.2 — import a user-specified binary path for an agent. Validates that
/// the file exists, canonicalizes the path, records it in the installer state
/// with kind: "path", and records an audit mutation.
#[tauri::command]
pub fn acp_agent_import(
    state: State<'_, AppState>,
    agent_id: String,
    binary_path: String,
) -> Result<serde_json::Value, String> {
    let registry = launch_registry();
    let manifest = registry
        .get(&agent_id)
        .cloned()
        .ok_or_else(|| format!("unknown agent id: {agent_id}"))?;

    let trimmed = binary_path.trim();
    if trimmed.is_empty() {
        return Err("binary path cannot be empty".to_string());
    }
    let p = std::path::PathBuf::from(trimmed);
    if !p.is_file() {
        return Err(format!("path is not a valid executable file: {trimmed}"));
    }
    let canonical = std::fs::canonicalize(&p).unwrap_or(p);

    let inst = installer();
    inst.record_path(&agent_id, &canonical)
        .map_err(|e| e.to_string())?;

    let audit_seq = crate::control::record_mutation(
        &state,
        // A path the user supplied themselves is the user's own gesture; it
        // must not be recorded as an agent/ticket action.
        crate::control::AuthKind::HumanGesture,
        "acp.agent_import",
        serde_json::json!({
            "agentId": agent_id,
            "path": canonical.to_string_lossy(),
        }),
    );

    let installed = inst.installed(&agent_id);
    let location = runtime_location_json(&manifest, installed.as_ref());

    Ok(serde_json::json!({
        "agentId": agent_id,
        "status": "ready",
        "binaryPath": canonical.to_string_lossy(),
        "location": location,
        "auditSeq": audit_seq,
    }))
}

/// P66.2 — probe and verify an agent's executable readiness and version string.
/// Runs `<executable> --version` or probes launch readiness. Returns verifiedAt
/// timestamp, detected version, and readiness status.
#[tauri::command]
pub fn acp_agent_verify(agent_id: String) -> Result<serde_json::Value, String> {
    let registry = launch_registry();
    let manifest = registry
        .get(&agent_id)
        .cloned()
        .ok_or_else(|| format!("unknown agent id: {agent_id}"))?;

    let inst = installer();
    let installed = inst.installed(&agent_id).filter(install_outcome_usable);

    let (exec_cmd, verify_args, is_wsl, display_exec): (String, Vec<String>, bool, String) =
        if let Some(ref o) = installed {
            if let Some(ref p) = o.binary_path {
                let s = p.to_string_lossy().into_owned();
                (s.clone(), vec!["--version".to_string()], false, s)
            } else {
                (
                    "npx".to_string(),
                    vec!["--version".to_string()],
                    false,
                    "npx".to_string(),
                )
            }
        } else {
            match &manifest.distribution {
                Distribution::Binary { command, .. } if !command.is_empty() => {
                    if let Some(p) = resolve_native_binary(command) {
                        let s = p.to_string_lossy().into_owned();
                        (s.clone(), vec!["--version".to_string()], false, s)
                    } else if let Some((wsl_bin, wsl_prefix)) = resolve_wsl_spawn(command) {
                        let mut args = wsl_prefix;
                        let linux_exec = args.last().cloned().unwrap_or_else(|| command.clone());
                        args.push("--version".to_string());
                        (wsl_bin, args, true, format!("wsl://{}", linux_exec))
                    } else {
                        return Ok(serde_json::json!({
                            "agentId": agent_id,
                            "status": "unavailable",
                            "reason": "executable not found on PATH, Windows App Paths, or WSL",
                            "verifiedAt": now_ms(),
                        }));
                    }
                }
                Distribution::Npx { package, .. } => {
                    if let Some(p) = resolve_on_path("npx") {
                        let s = p.to_string_lossy().into_owned();
                        (s.clone(), vec!["--version".to_string()], false, s)
                    } else {
                        return Ok(serde_json::json!({
                            "agentId": agent_id,
                            "status": "unavailable",
                            "reason": "npx not found on PATH",
                            "package": package,
                            "verifiedAt": now_ms(),
                        }));
                    }
                }
                Distribution::Uvx { package, .. } => {
                    if let Some(p) = resolve_on_path("uvx") {
                        let s = p.to_string_lossy().into_owned();
                        (s.clone(), vec!["--version".to_string()], false, s)
                    } else {
                        return Ok(serde_json::json!({
                            "agentId": agent_id,
                            "status": "unavailable",
                            "reason": "uvx not found on PATH",
                            "package": package,
                            "verifiedAt": now_ms(),
                        }));
                    }
                }
                _ => {
                    return Ok(serde_json::json!({
                        "agentId": agent_id,
                        "status": "unavailable",
                        "reason": "manifest has no executable target",
                        "verifiedAt": now_ms(),
                    }));
                }
            }
        };

    if !is_wsl && !std::path::Path::new(&exec_cmd).is_file() && resolve_on_path(&exec_cmd).is_none()
    {
        return Ok(serde_json::json!({
            "agentId": agent_id,
            "status": "unavailable",
            "reason": "resolved binary file does not exist",
            "executable": display_exec,
            "verifiedAt": now_ms(),
        }));
    }

    let version_output = std::process::Command::new(&exec_cmd)
        .args(&verify_args)
        .output()
        .ok();

    let mut detected_version = None;
    let mut exit_ok = false;
    if let Some(out) = version_output {
        if out.status.success() {
            exit_ok = true;
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let v_str = if !stdout.is_empty() { stdout } else { stderr };
            if let Some(line) = v_str.lines().next() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    detected_version = Some(trimmed.to_string());
                }
            }
        }
    }

    Ok(serde_json::json!({
        "agentId": agent_id,
        "status": if exit_ok || (!is_wsl && std::path::Path::new(&exec_cmd).is_file()) { "ready" } else { "degraded" },
        "executable": display_exec,
        "version": detected_version.or_else(|| installed.and_then(|o| if o.version == "path" { None } else { Some(o.version) })),
        "verifiedAt": now_ms(),
        "isWsl": is_wsl,
    }))
}

/// P63.11 — the shared-plane servers for this launch. A bind failure leaves
/// the list empty and is logged; the launch itself still proceeds.
fn channel_b_servers(state: &AppState) -> Vec<agentcowork_acp::McpServer> {
    if let Ok(relay) = state.chat_relay.lock() {
        if let Some(relay) = relay.as_ref() {
            crate::channel_b::publish_tools(&state.channel_b_tools, relay.tools());
        }
    }
    match crate::channel_b::ensure_servers(&state.channel_b, &state.channel_b_tools) {
        Ok(servers) => servers,
        Err(err) => {
            eprintln!("agentcowork: channel B lease unavailable: {err}");
            Vec::new()
        }
    }
}

/// The prompt's Channel-B claim must reflect the host server list actually
/// mounted for this binding. An empty list (including lease failure) is never
/// described as available to the external agent.
fn governance_for_channel_b_servers(
    servers: &[agentcowork_acp::McpServer],
) -> agentcowork_acp::GovernedSession {
    agentcowork_acp::GovernedSession::SelfContained {
        channel_b: !servers.is_empty(),
    }
}

/// Launch an agent by id: resolve its spawn plan, spawn the process, run the
/// ACP handshake (`initialize` → `session/new`), and keep the session alive.
///
/// Every launchable agent is an external subprocess (ADR-0005); an id no
/// registry knows fails closed with `unknown agent id` rather than resolving
/// to a built-in engine that no longer exists.
///
/// **Auth surfacing:** when `session/new` answers `auth_required`, the launch
/// still succeeds and reports `authRequired: true` with the agent's
/// `authMethods` — the UI renders "Sign in with <agent>" instead of failing.
#[tauri::command]
pub fn acp_launch(
    state: State<'_, AppState>,
    agent_id: String,
    cwd: String,
) -> Result<AcpHandleInfo, String> {
    let registry = launch_registry();
    let manifest = registry
        .get(&agent_id)
        .cloned()
        .ok_or_else(|| format!("unknown agent id: {agent_id}"))?;

    let plan = registry
        .launch_plan(&agent_id)
        .ok_or_else(|| format!("no launch plan for {agent_id}"))?;

    // F8: if a binary agent is installed, launch the extracted binary path
    // (not the seed's PATH command), merging the installed env. P53.7: with
    // no install record, resolve the seed command on PATH so a user-installed
    // CLI launches by its discovered absolute path — never a bare-name guess
    // that depends on the child's inherited PATH.
    let installed = installer().installed(&agent_id);
    let path_resolved = match &plan {
        p if matches!(
            registry.get(&agent_id).map(|m| &m.distribution),
            Some(Distribution::Binary { .. })
        ) =>
        {
            resolve_native_binary(&p.command).map(|p| p.to_string_lossy().into_owned())
        }
        _ => None,
    };
    let (command, extra_args): (String, Vec<String>) = if let Some(p) =
        installed.as_ref().and_then(|o| o.binary_path.as_ref())
    {
        (p.to_string_lossy().into_owned(), vec![])
    } else if let Some(path) = path_resolved {
        let _ = installer().record_path(&agent_id, std::path::Path::new(&path));
        (path, vec![])
    } else if let Some((wsl_bin, wsl_prefix)) = match &manifest.distribution {
        Distribution::Binary { command, .. } if !command.is_empty() => resolve_wsl_spawn(command),
        _ => None,
    } {
        (wsl_bin, wsl_prefix)
    } else if matches!(
        registry.get(&agent_id).map(|m| &m.distribution),
        Some(Distribution::Binary { .. })
    ) {
        return Err(format!(
            "agent {agent_id} has no installed, PATH-resolved, or WSL launch path"
        ));
    } else {
        (plan.command.clone(), vec![])
    };

    // P63 — the user's per-agent provider binding (chosen in Agent runtimes)
    // is injected as environment. The key is read from the vault here, in
    // Rust, and never travels through IPC or the renderer; the binding comes
    // last so an explicit user choice wins over a manifest default. A refusal
    // or a missing binding yields nothing and never blocks the launch.
    let backend_env: Vec<(String, String)> =
        crate::agent_backend_cmds::spawn_env_for(&state, &agent_id);
    let mut env: Vec<(&str, &str)> = plan
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    if let Some(o) = &installed {
        for (k, v) in &o.env {
            env.push((k.as_str(), v.as_str()));
        }
    }
    for (k, v) in &backend_env {
        env.push((k.as_str(), v.as_str()));
    }
    let mut args: Vec<&str> = extra_args.iter().map(String::as_str).collect();
    for a in &plan.args {
        args.push(a.as_str());
    }
    let transport = ProcessTransport::spawn(&command, &args, &env)
        .map_err(|e| format!("failed to spawn {command}: {e}"))?;

    let mut session = AcpSession::new(transport);
    session
        .initialize(ClientInfo {
            name: "agentcowork".to_string(),
            title: "AgentCowork".to_string(),
            version: "0.1.0".to_string(),
        })
        .map_err(|e| format!("acp initialize failed: {e}"))?;
    let init = session.agent_capabilities().cloned().unwrap_or_default();
    let embedded_context = init.prompt_capabilities.embedded_context;
    let auth_methods = session.auth_methods().to_vec();

    // Try to create the session. `auth_required` is not a failure — it is a
    // signal to surface the sign-in surface (the handle stays alive so
    // `acp_authenticate` can retry after login).
    let mcp_servers = channel_b_servers(&state);
    let (session_id, auth_required) = match session.session_new(&cwd, mcp_servers) {
        Ok(sid) => (sid, false),
        Err(agentcowork_acp::AcpError::AuthRequired) => (String::new(), true),
        Err(e) => return Err(format!("acp session/new failed: {e}")),
    };

    let handle = format!("acp-{}", ACP_COUNTER.fetch_add(1, Ordering::Relaxed));
    let agent_name = manifest.name.clone();
    // Snapshot the negotiated session config options *before* the session is
    // moved into the handle map, so both the handle and the launch response
    // report the same list.
    let config_options = session.config_options().to_vec();
    let provider_session_id = (!session_id.is_empty()).then(|| session_id.clone());
    let session = AcpSessionSlot::new(session);
    let cancel = session.cancellation_handle();
    state
        .acp_sessions
        .lock()
        .map_err(|e| e.to_string())?
        .insert(
            handle.clone(),
            AcpHandle {
                agent_id: agent_id.clone(),
                cwd,
                auth_required,
                auth_methods: auth_methods.clone(),
                embedded_context,
                available_commands: Vec::new(),
                config_options: config_options.clone(),
                prefix_guard: agentcowork_acp::PrefixGuard::new(),
                provider_session_id,
                owner: None,
                run_id: None,
                session,
                cancel,
            },
        );

    Ok(AcpHandleInfo {
        handle,
        agent_id,
        agent_name,
        provider_session_id: session_id.clone(),
        session_id: session_id.clone(),
        application_session_id: String::new(),
        work_id: String::new(),
        binding_id: String::new(),
        run_id: String::new(),
        protocol: "acp".to_string(),
        auth_required,
        auth_methods,
        embedded_context,
        config_options,
    })
}

/// Drive the ACP `authenticate` flow on a live handle, then retry
/// `session/new`. Agent-type methods return `{}` (the agent drives its own
/// login flow — prints a URL / opens its own browser). URL-type methods
/// return a `url`: the UI opens it in the system browser, the user completes
/// login, then the UI calls `acp_authenticate` again (which now succeeds and
/// creates the session). This is the "already signed in?" check — a launch
/// with `authRequired: false` means no login was needed.
#[tauri::command]
pub fn acp_authenticate(
    state: State<'_, AppState>,
    handle: String,
    method_id: String,
) -> Result<serde_json::Value, String> {
    // Copy only the per-handle session handle out of the registry. The global
    // map lock is never held across authenticate/session-new I/O.
    let (session, cwd) = {
        let sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
        let entry = sessions
            .get(&handle)
            .ok_or_else(|| format!("unknown ACP handle: {handle}"))?;
        (entry.session.clone(), entry.cwd.clone())
    };

    let result = {
        let mut session = session.lock().map_err(|e| e.to_string())?;
        session
            .authenticate(&method_id)
            .map_err(|e| format!("acp authenticate failed: {e}"))?
    };

    // url-type: hand the URL back — the user must complete login first.
    if let Some(url) = result.url {
        return Ok(serde_json::json!({ "ok": false, "url": url, "pending": true }));
    }

    // agent-type (or completed url-type): the connection is authenticated;
    // retry the session the launch couldn't create.
    let (session_id, config_options) = {
        let mut session = session.lock().map_err(|e| e.to_string())?;
        let mcp_servers = channel_b_servers(&state);
        let session_id = match session.session_new(&cwd, mcp_servers) {
            Ok(sid) => sid,
            Err(agentcowork_acp::AcpError::AuthRequired) => {
                return Err("still auth_required after authenticate".to_string());
            }
            Err(e) => return Err(format!("acp session/new after auth failed: {e}")),
        };
        (session_id, session.config_options().to_vec())
    };
    {
        let mut sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
        let entry = sessions
            .get_mut(&handle)
            .ok_or_else(|| format!("unknown ACP handle: {handle}"))?;
        entry.auth_required = false;
        entry.provider_session_id = Some(session_id.clone());
        entry.config_options = config_options;
    }
    Ok(serde_json::json!({ "ok": true, "sessionId": session_id }))
}

/// P38 (spec §4.2.5a §2) — build the prompt for an external Chief with the
/// memory passport (C10) + governance block injected, mirroring the inbuilt
/// path's `<memory_warm_set>` injection. Best-effort: a missing/unavailable
/// memory handler never blocks the turn (same contract as `memory/plan`).
fn build_acp_prompt_with_passport(state: &State<'_, AppState>, text: &str) -> (String, u64) {
    // Every agent we launch is external and self-contained: permission
    // requests are mediated by Guard-2 at the ACP boundary, but effects
    // performed inside the agent's own process are outside the AgentCowork audit
    // trail. No id gets a fully-mediated session any more — that was the
    // retired built-in engine's privilege (ADR-0005 §3). The class is an
    // architectural claim and is recorded as a decision; the Channel-B flag is
    // **observed**, not assumed: a failed lease leaves no servers, and a badge
    // that claims a mounted catalog when none is mounted is a lie the agent
    // would read as permission to use it (FIX-07).
    let mounted_servers = channel_b_servers(state);
    let governance = governance_for_channel_b_servers(&mounted_servers);
    let core_facts = {
        let relay = state.chat_relay.lock().ok();
        relay
            .as_ref()
            .and_then(|r| r.as_ref())
            .map(|r| {
                let mem = r.memory();
                let m = mem.lock().unwrap_or_else(|e| e.into_inner());
                m.core_facts()
            })
            .unwrap_or_default()
    };
    // P53.6 — expose the persisted installed-CLI delegation mix at the
    // moment the Chief receives a turn. This is advisory context only; every
    // child launch remains subject to the B3 limits and Guard-2 policy.
    let mut mix = String::new();
    if let Ok(cfg) = Config::load() {
        let rows: Vec<String> = launch_registry()
            .agents
            .iter()
            .filter(|m| agent_installed(&m.id))
            .filter(|m| cfg.subagent_enabled.get(&m.id).copied().unwrap_or(true))
            .map(|m| {
                let note = cfg.subagent_notes.get(&m.id).cloned().unwrap_or_default();
                format!(
                    "- {}: {}",
                    m.name,
                    if note.is_empty() {
                        m.description.clone()
                    } else {
                        note
                    }
                )
            })
            .collect();
        if !rows.is_empty() {
            mix.push_str(&rows.join("\n"));
            mix.push_str("\nUse only within the declared B3 depth/concurrency limits.");
        }
    }
    // P71.9i — the passport assembly itself lives in `agentcowork-acp` so the
    // documented block order (passport → governance → tool-affinity steering →
    // delegation mix → user turn) is one implementation, not a call-site
    // convention. (This block previously appended the mix *after* the user
    // turn and emitted literal `\\n` escapes instead of newlines.)
    let prompt = agentcowork_acp::build_chief_prompt_with_steering(
        text,
        &core_facts,
        &governance,
        Some(agentcowork_acp::COWORK_AFFINITY_STEERING),
        if mix.is_empty() {
            None
        } else {
            Some(mix.as_str())
        },
    );
    // P69.E9 — fingerprint the shell-owned stable prefix with the exact
    // inputs this builder assembled (`ARCH/CONTEXT.md` §4: the warm-memory
    // set is dynamic-tail content and is deliberately not fingerprinted).
    let fingerprint = agentcowork_acp::fingerprint_stable_prefix(
        governance.badge(),
        Some(agentcowork_acp::COWORK_AFFINITY_STEERING),
        if mix.is_empty() {
            None
        } else {
            Some(mix.as_str())
        },
    );
    (prompt, fingerprint)
}

/// P53.5 — per-session tool observability file. Each ACP turn appends one
/// JSON line with the visible prompt prefix + the turn's tool-call rows +
/// stop reason to `<data_dir>/acp_sessions/<session>/tool_log.jsonl`. This is
/// metrics the user can open ("what did it run?") — it is never imported into
/// chat context (the return path folds only visible assistant text). Best
/// effort: a logging failure never fails the turn.
fn append_acp_tool_log(
    application_session_id: &str,
    provider_session_id: &str,
    handle: &str,
    agent_id: &str,
    text: &str,
    outcome: &PromptOutcome,
    prefix_event: agentcowork_acp::PrefixEvent,
) {
    let safe: String = application_session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dir = agentcowork_core::default_data_dir()
        .join("acp_sessions")
        .join(if safe.is_empty() { "unknown" } else { &safe });
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let tools: Vec<serde_json::Value> = outcome
        .updates
        .iter()
        .filter(|u| u.session_update.starts_with("tool_call"))
        .map(|u| {
            serde_json::json!({
                "toolCallId": u.tool_call_id,
                "title": u.title,
                "kind": u.kind,
                "status": u.status,
            })
        })
        .collect();
    let mut prompt_prefix = text.chars().take(240).collect::<String>();
    if text.chars().count() > 240 {
        prompt_prefix.push('…');
    }
    let line = serde_json::json!({
        "tsMs": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        "handle": handle,
        "applicationSessionId": application_session_id,
        "providerSessionId": provider_session_id,
        "agentId": agent_id,
        "promptPrefix": prompt_prefix,
        "stopReason": outcome.stop_reason.as_str(),
        // P69.E9 — I16 observability: the guard's classification of this
        // turn's stable-prefix state (first_turn / stable /
        // declared_mutation / undeclared_mutation).
        "prefixEvent": prefix_event.as_str(),
        "toolCalls": tools,
    });
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("tool_log.jsonl"))
    {
        use std::io::Write;
        let _ = writeln!(f, "{}", line);
    }
}

/// P53.6 + P60 — Settings → Subagents rows: **installed CLIs only** (an
/// AgentCowork install record or a PATH-discovered binary — the same
/// `agent_installed` predicate Chief occupancy uses), plus AgentCowork Native,
/// which is always present as the default candidate. Each row carries the
/// shipped default when-to-use text (the registry manifest description) plus
/// the user's override from `agentcowork.toml` (`subagent_notes`; empty =
/// default). The Chief reads these at delegate time (ACP prompt injection +
/// handoff bundle).
#[tauri::command]
pub fn chief_subagents() -> Result<Vec<serde_json::Value>, String> {
    let cfg = Config::load().map_err(|e| e.to_string())?;
    let registry = launch_registry();
    let mut rows = Vec::new();
    for m in &registry.agents {
        // External CLIs appear only when `agent_installed` verifies them, so
        // the Subagents surface is occupancy, never the raw catalog. There is
        // no built-in delegation candidate (ADR-0005 §D1) — delegation goes
        // through the `delegate.*` façade on the shared plane (P71.1).
        if !agent_installed(&m.id) {
            continue;
        }
        let note = cfg.subagent_notes.get(&m.id).cloned().unwrap_or_default();
        let enabled = cfg.subagent_enabled.get(&m.id).copied().unwrap_or(true);
        rows.push(serde_json::json!({
            "agentId": m.id,
            "name": m.name.as_str(),
            "defaultWhenToUse": m.description,
            "whenToUse": if note.is_empty() { m.description.clone() } else { note.clone() },
            "customized": !note.is_empty(),
            "enabled": enabled,
        }));
    }
    Ok(rows)
}

/// P53.6 — set (or clear, with an empty note) the user's when-to-use override
/// for one installed subagent CLI. Refuses unknown/uninstalled ids — notes
/// attach only to real occupancy candidates.
#[tauri::command]
pub fn chief_subagent_set_note(agent_id: String, note: String) -> Result<String, String> {
    if launch_registry().get(&agent_id).is_none() {
        return Err(format!("unknown agent id: {agent_id}"));
    }
    if !agent_installed(&agent_id) {
        return Err(format!("agent {agent_id} is not installed"));
    }
    let path = Config::config_path().map_err(|e| e.to_string())?;
    let mut cfg = Config::load().map_err(|e| e.to_string())?;
    let trimmed = note.trim().to_string();
    if trimmed.is_empty() {
        cfg.subagent_notes.remove(&agent_id);
    } else {
        cfg.subagent_notes.insert(agent_id.clone(), trimmed);
    }
    cfg.save(&path).map_err(|e| e.to_string())?;
    Ok(agent_id)
}

/// P53.6 — enable or disable an installed CLI in the Chief's delegation mix.
#[tauri::command]
pub fn chief_subagent_set_enabled(agent_id: String, enabled: bool) -> Result<bool, String> {
    if launch_registry().get(&agent_id).is_none() {
        return Err(format!("unknown agent id: {agent_id}"));
    }
    if !agent_installed(&agent_id) {
        return Err(format!("agent {agent_id} is not installed"));
    }
    let path = Config::config_path().map_err(|e| e.to_string())?;
    let mut cfg = Config::load().map_err(|e| e.to_string())?;
    cfg.subagent_enabled.insert(agent_id, enabled);
    cfg.save(&path).map_err(|e| e.to_string())?;
    Ok(enabled)
}

/// P71.9d — persist one installed agent's delegation profile
/// (Settings → Subagents). Refuses unknown/uninstalled ids; fields absent from
/// the payload keep their spec defaults, and the gateway's live
/// `delegation_gauge` remains the admission authority.
/// (The nine optional fields are the profile's own shape; grouping them into
/// a struct would ripple through the UI call site for no behavioral gain.)
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn chief_subagent_set_policy(
    agent_id: String,
    model_policy: Option<String>,
    role: Option<String>,
    may_spawn: Option<bool>,
    max_children: Option<u32>,
    max_depth: Option<u32>,
    max_concurrency: Option<u32>,
    workspace: Option<String>,
    budget: Option<u64>,
    allow_as_primary: Option<bool>,
    enable_as_subagent: Option<bool>,
    domains: Option<Vec<String>>,
    max_cents_per_turn: Option<u32>,
    max_tokens_per_turn: Option<u64>,
) -> Result<serde_json::Value, String> {
    if launch_registry().get(&agent_id).is_none() {
        return Err(format!("unknown agent id: {agent_id}"));
    }
    if !agent_installed(&agent_id) {
        return Err(format!("agent {agent_id} is not installed"));
    }
    let path = Config::config_path().map_err(|e| e.to_string())?;
    let mut cfg = Config::load().map_err(|e| e.to_string())?;
    let policy = cfg
        .subagent_policy
        .entry(agent_id.clone())
        .or_insert_with(agentcowork_core::SubagentPolicy::default);
    if let Some(v) = model_policy {
        policy.model_policy = v;
    }
    if let Some(v) = role {
        policy.role = v;
    }
    if let Some(v) = may_spawn {
        policy.may_spawn = v;
    }
    if let Some(v) = max_children {
        policy.max_children = v;
    }
    if let Some(v) = max_depth {
        policy.max_depth = v;
    }
    if let Some(v) = max_concurrency {
        policy.max_concurrency = v;
    }
    if let Some(v) = workspace {
        if v != "shared" && v != "isolated" {
            return Err(format!(
                "workspace must be \"shared\" or \"isolated\", got {v:?}"
            ));
        }
        policy.workspace = v;
    }
    if let Some(v) = budget {
        policy.budget = v;
    }
    if let Some(v) = allow_as_primary {
        policy.allow_as_primary = v;
    }
    if let Some(v) = enable_as_subagent {
        policy.enable_as_subagent = v;
    }
    if let Some(v) = domains {
        const ALLOWED: &[&str] = &["coding", "architecture", "research", "scraping", "office"];
        for tag in &v {
            if !ALLOWED.contains(&tag.as_str()) {
                return Err(format!("unknown domain tag {tag:?}"));
            }
        }
        policy.domains = v;
    }
    if let Some(v) = max_cents_per_turn {
        policy.max_cents_per_turn = v;
    }
    if let Some(v) = max_tokens_per_turn {
        policy.max_tokens_per_turn = v;
    }
    let saved = policy.clone();
    cfg.save(&path).map_err(|e| e.to_string())?;
    serde_json::to_value(saved).map_err(|e| e.to_string())
}

/// P53.6 — the current enabled delegation mix, consumed by Chief handoff.
#[tauri::command]
pub fn chief_subagent_mix() -> Result<Vec<serde_json::Value>, String> {
    Ok(chief_subagents()?
        .into_iter()
        .filter(|row| {
            row.get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true)
        })
        .collect())
}

#[tauri::command]
pub fn acp_session_commands(
    state: State<'_, AppState>,
    handle: String,
) -> Result<Vec<AvailableCommand>, String> {
    let sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
    let entry = sessions
        .get(&handle)
        .ok_or_else(|| format!("unknown ACP handle: {handle}"))?;
    Ok(entry.available_commands.clone())
}

const NATIVE_ONLY_MODEL_REASON: &str =
    "This agent manages its own model — configure it inside the agent.";

fn runtime_control_wire(control: RuntimeControl) -> &'static str {
    match control {
        RuntimeControl::NativeOnly => "NativeOnly",
        RuntimeControl::LaunchOverride => "LaunchOverride",
        RuntimeControl::SessionConfig => "SessionConfig",
        RuntimeControl::Unknown => "Unknown",
    }
}

#[derive(Clone)]
struct AcpConfigTarget {
    handle: String,
    agent_id: String,
    binding_id: Option<String>,
    session: AcpSessionSlot,
}

fn resolve_acp_config_target(
    state: &AppState,
    agent_id: Option<&str>,
    binding_id: Option<&str>,
    handle: Option<&str>,
) -> Result<AcpConfigTarget, String> {
    let agent_id = agent_id.map(str::trim).filter(|value| !value.is_empty());
    let binding_id = binding_id.map(str::trim).filter(|value| !value.is_empty());
    let handle = handle.map(str::trim).filter(|value| !value.is_empty());
    if agent_id.is_none() && binding_id.is_none() && handle.is_none() {
        return Err("agentId, bindingId, or a live ACP handle is required".to_string());
    }

    let sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
    let selected = if let Some(handle) = handle {
        sessions
            .get_key_value(handle)
            .map(|(key, entry)| (key, entry))
    } else if let Some(binding_id) = binding_id {
        sessions.iter().find(|(_, entry)| {
            entry.binding_id() == Some(binding_id)
                && agent_id.map_or(true, |agent_id| entry.agent_id == agent_id)
        })
    } else {
        let agent_id = agent_id.expect("agentId checked above");
        let matches: Vec<_> = sessions
            .iter()
            .filter(|(_, entry)| entry.agent_id == agent_id)
            .collect();
        match matches.as_slice() {
            [only] => Some(*only),
            [] => None,
            _ => {
                return Err(format!(
                    "agent {agent_id} has multiple live ACP sessions; bindingId is required"
                ));
            }
        }
    };
    let (handle, entry) = selected.ok_or_else(|| {
        let subject = handle
            .map(|id| format!("ACP handle {id}"))
            .or_else(|| binding_id.map(|id| format!("binding {id}")))
            .unwrap_or_else(|| format!("agent {}", agent_id.unwrap_or("<unknown>")));
        format!("no live ACP session is bound to {subject}")
    })?;
    if let Some(agent_id) = agent_id {
        if entry.agent_id != agent_id {
            return Err(format!(
                "ACP handle {handle} belongs to agent {}, not {agent_id}",
                entry.agent_id
            ));
        }
    }
    if let Some(binding_id) = binding_id {
        if entry.binding_id() != Some(binding_id) {
            return Err(format!(
                "ACP handle {handle} is not owned by binding {binding_id}"
            ));
        }
    }
    Ok(AcpConfigTarget {
        handle: handle.to_string(),
        agent_id: entry.agent_id.clone(),
        binding_id: entry.binding_id().map(str::to_string),
        session: entry.session.clone(),
    })
}

fn live_config_options(target: &AcpConfigTarget) -> Result<Vec<ConfigOption>, String> {
    let session = target.session.lock()?;
    Ok(session.config_options().to_vec())
}

fn config_options_projection(
    target: &AcpConfigTarget,
    options: Vec<ConfigOption>,
) -> serde_json::Value {
    let control = if options.is_empty() {
        RuntimeControl::NativeOnly
    } else {
        RuntimeControl::SessionConfig
    };
    serde_json::json!({
        "handle": target.handle,
        "agentId": target.agent_id,
        "bindingId": target.binding_id,
        "control": runtime_control_wire(control),
        "reason": (options.is_empty()).then_some(NATIVE_ONLY_MODEL_REASON),
        "options": options,
    })
}

fn native_only_config_request_projection(
    agent_id: &str,
    binding_id: Option<&str>,
    handle: &str,
) -> serde_json::Value {
    serde_json::json!({
        "state": "requested",
        "detail": "No configuration change was sent because the agent advertises no session options",
        "control": runtime_control_wire(RuntimeControl::NativeOnly),
        "reason": NATIVE_ONLY_MODEL_REASON,
        "agentId": agent_id,
        "bindingId": binding_id,
        "handle": handle,
        "options": [],
        "agentConfirmed": false,
    })
}

fn config_option_request_projection<F>(
    agent_id: &str,
    binding_id: Option<&str>,
    handle: &str,
    config_id: &str,
    value: serde_json::Value,
    options: &[ConfigOption],
    apply: F,
) -> Result<serde_json::Value, String>
where
    F: FnOnce(&str, serde_json::Value) -> Result<Vec<ConfigOption>, String>,
{
    if options.is_empty() {
        return Ok(native_only_config_request_projection(
            agent_id, binding_id, handle,
        ));
    }
    if !options.iter().any(|option| option.id == config_id) {
        return Err(format!(
            "agent does not advertise config option {config_id}; AgentCowork will not fabricate one"
        ));
    }
    let updated = apply(config_id, value)?;
    Ok(serde_json::json!({
        "state": "requested",
        "detail": "agent confirmed session/set_config_option; the agent remains the owner of model activation",
        "control": runtime_control_wire(RuntimeControl::SessionConfig),
        "agentId": agent_id,
        "bindingId": binding_id,
        "handle": handle,
        "configId": config_id,
        "options": updated,
        "agentConfirmed": true,
    }))
}

/// Return the bound agent's advertised ACP config vocabulary, never a host catalog.
#[tauri::command]
pub fn acp_config_options(
    state: State<'_, AppState>,
    agent_id: Option<String>,
    binding_id: Option<String>,
    handle: Option<String>,
) -> Result<serde_json::Value, String> {
    let target = resolve_acp_config_target(
        &state,
        agent_id.as_deref(),
        binding_id.as_deref(),
        handle.as_deref(),
    )?;
    let options = live_config_options(&target)?;
    Ok(config_options_projection(&target, options))
}

/// Apply one advertised ACP session config option through the existing mediated
/// ACP session. The command reports a request state even after confirmation.
#[tauri::command]
pub fn acp_set_session_config_option(
    state: State<'_, AppState>,
    agent_id: Option<String>,
    binding_id: Option<String>,
    handle: Option<String>,
    config_id: String,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let target = resolve_acp_config_target(
        &state,
        agent_id.as_deref(),
        binding_id.as_deref(),
        handle.as_deref(),
    )?;
    let options = live_config_options(&target)?;
    let confirmed_handle = target.handle.clone();
    let result = config_option_request_projection(
        &target.agent_id,
        target.binding_id.as_deref(),
        &target.handle,
        &config_id,
        value,
        &options,
        |config_id, value| {
            let mut session = target.session.lock()?;
            session
                .set_config_option(config_id, value)
                .map_err(|e| e.to_string())
        },
    )?;
    if result
        .get("agentConfirmed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        let updated: Vec<ConfigOption> = serde_json::from_value(
            result
                .get("options")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(|error| error.to_string())?;
        if let Ok(mut sessions) = state.acp_sessions.lock() {
            if let Some(entry) = sessions.get_mut(&confirmed_handle) {
                entry.config_options = updated;
            }
        }
    }
    Ok(result)
}

/// Return the complete model/config vocabulary owned by one external ACP
/// session. No native provider keys or models are returned here.
#[tauri::command]
pub fn acp_session_config_options(
    state: State<'_, AppState>,
    handle: String,
) -> Result<Vec<ConfigOption>, String> {
    let sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
    let entry = sessions
        .get(&handle)
        .ok_or_else(|| format!("unknown ACP handle: {handle}"))?;
    Ok(entry.config_options.clone())
}

/// Set one agent-owned session configuration value. The ACP response replaces
/// the complete option list so dependent model/reasoning options stay honest.
#[tauri::command]
pub fn acp_session_set_config_option(
    state: State<'_, AppState>,
    handle: String,
    config_id: String,
    value: serde_json::Value,
) -> Result<Vec<ConfigOption>, String> {
    let (session, advertised) = {
        let sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
        let entry = sessions
            .get(&handle)
            .ok_or_else(|| format!("unknown ACP handle: {handle}"))?;
        (entry.session.clone(), entry.config_options.clone())
    };
    if advertised.is_empty() {
        return Ok(Vec::new());
    }
    if !advertised.iter().any(|option| option.id == config_id) {
        return Err(format!(
            "agent does not advertise config option {config_id}; AgentCowork will not fabricate one"
        ));
    }
    let options = {
        let mut session = session.lock().map_err(|e| e.to_string())?;
        session
            .set_config_option(&config_id, value)
            .map_err(|e| e.to_string())?
    };
    let mut sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
    let entry = sessions
        .get_mut(&handle)
        .ok_or_else(|| format!("unknown ACP handle: {handle}"))?;
    entry.config_options = options.clone();
    Ok(options)
}

/// Why the `session/load` provider-resume seam refuses, as a typed reason.
///
/// **v1 ships without provider resume, and that is a recorded decision, not an
/// omission.** [`ARCH/ADR/0007`](ARCH/ADR/0007-windows-first-v1-qualification.md)
/// §4 sets this release's ACP policy to *narrow unsupported v2*, which puts the
/// ACP **v2** `session/resume` method out of scope, and §3 makes **transport
/// reconnect** — replay the Work event stream from the last acknowledged
/// sequence and re-attach the existing binding — the precondition for
/// attempting provider resume at all. v1 has no such reconnect seam, so
/// [`AcpSession::session_load`](agentcowork_acp::AcpSession::session_load) stays
/// adapter-internal and its only call site is the crate's own handshake
/// acceptance suite. A shell caller gets one of these reasons instead of a
/// fabricated resume.
///
/// The continuation a user actually gets is a **new** provider session driven
/// from the durable checkpoint/`ContextPassport`, which
/// [`ARCH/AGENT.md`](ARCH/AGENT.md) §3.2 and ADR-0007 §3 require be labelled
/// *provider session restarted* — never presented as native resume. Nothing here
/// ever reads or writes a provider session id as a canonical Session/Work id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpSessionLoadRefusal {
    /// The agent never advertised `agentCapabilities.loadSession` during
    /// `initialize`. ACP v1 forbids inferring a capability from omission
    /// (ADR-0007 §4), so this is refusal rather than a probe.
    CapabilityNotNegotiated,
    /// The canonical `Session → Work → Run → AgentBinding` chain records no
    /// provider session id for this agent. AgentCowork never fabricates one, and
    /// an unavailable Work Gateway is treated exactly like a missing record
    /// (fail-closed).
    NoRecordedProviderSession,
    /// Both preconditions hold, but v1 has not qualified provider resume.
    ProviderResumeOutOfScope,
}

impl AcpSessionLoadRefusal {
    /// Classify a resume attempt from the only two facts it may legitimately
    /// rest on: the capability the agent actually negotiated, and the provider
    /// session id the canonical chain actually recorded.
    fn decide(load_session_negotiated: bool, recorded_provider_session_id: Option<&str>) -> Self {
        if !load_session_negotiated {
            return Self::CapabilityNotNegotiated;
        }
        if recorded_provider_session_id
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .is_none()
        {
            return Self::NoRecordedProviderSession;
        }
        Self::ProviderResumeOutOfScope
    }
}

impl std::fmt::Display for AcpSessionLoadRefusal {
    /// Plain language, because this text is what reaches the user: Tauri
    /// commands report through `Err(String)`, and the shell's `nativeCall`
    /// wrapper surfaces the message verbatim. Never a debug-formatted enum.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CapabilityNotNegotiated => write!(
                f,
                "This agent never advertised the ACP \"loadSession\" capability, so it cannot \
                 re-open an existing provider conversation, and AgentCowork does not guess from \
                 silence. The conversation continues in a new provider session instead."
            ),
            Self::NoRecordedProviderSession => write!(
                f,
                "This conversation has no provider session id recorded on its agent binding, so \
                 there is nothing to re-open. AgentCowork never invents a provider session id, and a \
                 provider session id is never a Session id."
            ),
            Self::ProviderResumeOutOfScope => write!(
                f,
                "Resuming an external agent's own provider conversation is not available in this \
                 release. ACP v2 — which owns the \"session/resume\" method — is deliberately \
                 unsupported (ARCH/ADR/0007 section 4), and attempting provider resume first \
                 requires a transport reconnect that re-attaches the existing binding, which v1 \
                 has not qualified. The conversation continues from the durable checkpoint and \
                 context passport in a new provider session, labelled \"provider session \
                 restarted\" — a restart, not a native resume."
            ),
        }
    }
}

/// Read the provider session id the canonical chain recorded for one binding.
///
/// The lookup is keyed by the **application** Session (whose id is this path's
/// Work id) plus the agent id, so a provider transcript id can never be used as
/// a Session/Work key. A missing Work, a missing or foreign binding, and a
/// binding with no recorded provider session all yield `None`: AgentCowork never
/// fabricates a provider session id, and a restarted provider session is not
/// evidence of a resume.
fn recorded_binding_provider_session_id(
    gateway: &agentcowork_core::WorkGateway,
    application_session_id: &str,
    agent_id: &str,
) -> Option<String> {
    let work_id = canonical_work_id(application_session_id);
    gateway
        .bindings_for(&work_id)
        .into_iter()
        .find(|binding| {
            binding.session_id.as_str() == application_session_id
                && binding.agent_id.as_str() == agent_id
        })
        .and_then(|binding| binding.provider_session_id.clone())
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

/// Re-attach an existing AgentBinding to a live provider session
/// (`session/load`), under the Work-gateway identity rules.
///
/// **This command refuses, in v1, by policy — the refusal is the contract.**
/// `session/load` is implemented and capability-gated in the adapter
/// (`crates/agentcowork-acp/src/client.rs`), but ADR-0007 §3 makes transport
/// reconnect the precondition for attempting provider resume, and v1 has no
/// reconnect seam; ADR-0007 §4 keeps ACP v2 (and therefore its `session/resume`
/// method) out of scope. Rather than ship a command that would need a second
/// lifecycle owner to be honest, this seam reports the precise reason it cannot
/// re-attach, so the shell never has to guess.
///
/// Guarantees this command keeps even while refusing:
/// - it never fabricates a provider session id, and an unreadable Work Gateway
///   is treated as "nothing recorded" (fail-closed);
/// - it never performs provider I/O, never mutates Work/Run/Binding state, and
///   never touches the Guard/receipt path, because it has no effect to guard;
/// - the requested identity is the **application** Session, and a handle already
///   claimed by another Session is refused with the existing owner-mismatch
///   error before the binding is read.
///
/// The refusal is returned as plain-language text on the command's error
/// channel, which is the surface the shell's `nativeCall` already renders.
#[tauri::command]
pub fn acp_session_load(
    state: State<'_, AppState>,
    handle: String,
    session_id: String,
) -> Result<(), String> {
    let application_session_id = session_id.trim().to_string();
    if application_session_id.is_empty() {
        return Err(AcpIdentityError::MissingApplicationSession.to_string());
    }
    // Copy the per-handle slot out of the registry first: the global map lock is
    // never held while the provider session mutex is taken (the prompt path
    // takes them in the opposite order).
    let (agent_id, slot, owner) = {
        let sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
        let entry = sessions
            .get(&handle)
            .ok_or_else(|| format!("unknown ACP handle: {handle}"))?;
        (
            entry.agent_id.clone(),
            entry.session.clone(),
            entry.owner.clone(),
        )
    };
    if let Some(owner) = owner.as_ref() {
        if owner.session_id != application_session_id {
            return Err(AcpIdentityError::OwnerMismatch {
                handle: handle.clone(),
                expected_session: application_session_id,
                actual_session: owner.session_id.clone(),
            }
            .to_string());
        }
    }
    // The negotiated capability set is a cached field from `initialize`; reading
    // it is not provider I/O.
    let load_session_negotiated = {
        let session = slot.lock().map_err(|e| e.to_string())?;
        session
            .agent_capabilities()
            .is_some_and(|capabilities| capabilities.load_session)
    };
    // Fail closed when the Work Gateway cannot be read: "unknown" is never
    // promoted to "there is a recorded provider session".
    let recorded = match relay_planes(&state) {
        Ok((gateway, _)) => match gateway.lock() {
            Ok(gateway) => {
                recorded_binding_provider_session_id(&gateway, &application_session_id, &agent_id)
            }
            Err(_) => None,
        },
        Err(_) => None,
    };
    let refusal = AcpSessionLoadRefusal::decide(load_session_negotiated, recorded.as_deref());
    Err(format!(
        "{refusal} (agent {agent_id}, Session {application_session_id})"
    ))
}

/// P53.5 — read the per-session tool observability file (newest last).
/// Empty until the first ACP turn lands for that application Session. A
/// missing file is honest emptiness, not an error. The application Session id
/// is sanitized exactly like the writer (`append_acp_tool_log`) so reads cannot
/// escape the dir; the provider session id is never used as a path.
#[tauri::command]
pub fn acp_tool_log(session_id: String) -> Result<Vec<serde_json::Value>, String> {
    let safe: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = agentcowork_core::default_data_dir()
        .join("acp_sessions")
        .join(if safe.is_empty() { "unknown" } else { &safe })
        .join("tool_log.jsonl");
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            out.push(v);
        }
    }
    Ok(out)
}
/// The deterministic canonical Work id for an application Session. The
/// scheduler already uses the Session id as its Work id, so reusing it here
/// resolves that existing Work instead of creating a second owner.
fn canonical_work_id(session_id: &str) -> String {
    session_id.to_string()
}

fn canonical_binding_id(session_id: &str, work_id: &str, agent_id: &str) -> String {
    format!("acp-binding:{session_id}:{work_id}:{agent_id}")
}

type AcpRelayPlanes = (
    Arc<Mutex<agentcowork_core::WorkGateway>>,
    Arc<Mutex<agentcowork_core::ExecutionKernel>>,
);

fn relay_planes(state: &State<'_, AppState>) -> Result<AcpRelayPlanes, AcpIdentityError> {
    let relay = state
        .chat_relay
        .lock()
        .map_err(|_| AcpIdentityError::WorkGatewayUnavailable)?;
    let relay = relay
        .as_ref()
        .ok_or(AcpIdentityError::WorkGatewayUnavailable)?;
    Ok((relay.work_gateway(), relay.executions()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcpTurnIdentity {
    owner: AcpCanonicalOwner,
    run_id: String,
}

/// Resolve the one durable Work/Run/Binding tuple before any provider prompt.
///
/// The Work id is the application Session id for the interactive path. If a
/// Work already exists under that id, its owner is checked rather than being
/// overwritten. A provider session is only ever stored on the binding's
/// private `provider_session_id` field.
fn prepare_acp_turn(
    gateway: &mut agentcowork_core::WorkGateway,
    kernel: &mut agentcowork_core::ExecutionKernel,
    application_session_id: &str,
    agent_id: &str,
    provider_session_id: &str,
    objective: &str,
) -> Result<AcpTurnIdentity, AcpIdentityError> {
    let application_session_id = application_session_id.trim();
    let agent_id = agent_id.trim();
    let provider_session_id = provider_session_id.trim();
    if application_session_id.is_empty() {
        return Err(AcpIdentityError::MissingApplicationSession);
    }
    if agent_id.is_empty() {
        return Err(AcpIdentityError::Binding("agent id is empty".into()));
    }
    if provider_session_id.is_empty() {
        return Err(AcpIdentityError::MissingProviderSession);
    }

    let work_id = canonical_work_id(application_session_id);
    // `ADR-0006` §1 — the Session kind is stated by the owner of the record,
    // never invented by the caller. `canonical_work_id` deliberately reuses the
    // host automation Work's id, and that Work carries `SessionKind::Automation`;
    // re-asserting `Interactive` unconditionally made `create_work_in_session`
    // refuse the very Work this function exists to resolve, so the documented
    // "reuse the existing Work" path failed closed on the automation seam. An
    // existing Work therefore re-states its own recorded kind (the owner check
    // below is unchanged and still fail-closed); only a new Work is stated
    // `Interactive`.
    let session_kind = gateway
        .get_work(&work_id)
        .map_or(agentcowork_types::SessionKind::Interactive, |existing| {
            existing.session_kind
        });
    let address = gateway
        .create_work_in_session(
            work_id.clone(),
            None,
            Some(application_session_id.to_string()),
            session_kind,
            objective,
        )
        .map_err(AcpIdentityError::Work)?;
    if address.session_id.as_deref() != Some(application_session_id) {
        return Err(AcpIdentityError::WorkOwnerMismatch {
            work_id,
            expected_session: application_session_id.to_string(),
            actual_session: address.session_id,
        });
    }

    let deterministic_binding_id = canonical_binding_id(application_session_id, &work_id, agent_id);
    let existing = gateway
        .bindings_for(&work_id)
        .into_iter()
        .find(|binding| {
            binding.session_id.as_str() == application_session_id
                && binding.agent_id.as_str() == agent_id
        })
        .cloned();
    let binding_id = existing
        .as_ref()
        .map(|binding| binding.binding_id.as_str().to_string())
        .unwrap_or(deterministic_binding_id);
    if binding_id.is_empty() {
        return Err(AcpIdentityError::Binding(
            "matching AgentBinding has an empty id".into(),
        ));
    }
    let mut activated_here = false;
    if let Some(binding) = existing {
        if binding.session_id.as_str() != application_session_id {
            return Err(AcpIdentityError::BindingOwnerMismatch {
                binding_id: binding.binding_id.as_str().to_string(),
                expected_session: application_session_id.to_string(),
                actual_session: binding.session_id.as_str().to_string(),
            });
        }
        if binding.work_id.as_str() != work_id || binding.agent_id.as_str() != agent_id {
            return Err(AcpIdentityError::Binding(format!(
                "binding {} is not owned by Work {work_id} and agent {agent_id}",
                binding.binding_id
            )));
        }
        match binding.state {
            agentcowork_types::BindingLifecycle::Active => {
                if binding
                    .provider_session_id
                    .as_deref()
                    .is_some_and(|id| id != provider_session_id)
                {
                    return Err(AcpIdentityError::ProviderSessionConflict {
                        binding_id,
                        expected: provider_session_id.to_string(),
                        actual: binding.provider_session_id,
                    });
                }
                if binding.provider_session_id.is_none() {
                    gateway
                        .transition_agent_binding(
                            &binding_id,
                            "activated",
                            Some(provider_session_id.to_string()),
                        )
                        .map_err(AcpIdentityError::Binding)?;
                }
            }
            agentcowork_types::BindingLifecycle::Parked => {
                gateway
                    .transition_agent_binding(
                        &binding_id,
                        "activated",
                        Some(provider_session_id.to_string()),
                    )
                    .map_err(AcpIdentityError::Binding)?;
                activated_here = true;
            }
            agentcowork_types::BindingLifecycle::Resuming => {
                gateway
                    .transition_agent_binding(
                        &binding_id,
                        "resumed",
                        Some(provider_session_id.to_string()),
                    )
                    .map_err(AcpIdentityError::Binding)?;
                activated_here = true;
            }
            agentcowork_types::BindingLifecycle::Dead
            | agentcowork_types::BindingLifecycle::Unavailable => {
                return Err(AcpIdentityError::Binding(format!(
                    "binding {binding_id} is {:?}",
                    binding.state
                )));
            }
        }
    } else {
        let binding = agentcowork_types::AgentBinding {
            binding_id: agentcowork_types::AgentBindingId::new(binding_id.clone()),
            session_id: agentcowork_types::SessionId::new(application_session_id),
            work_id: agentcowork_types::WorkId::new(work_id.clone()),
            agent_id: agentcowork_types::AgentId::new(agent_id),
            adapter_id: None,
            protocol: agentcowork_types::AgentProtocol::Acp,
            provider_session_id: None,
            model: None,
            mode: None,
            capability_manifest: Vec::new(),
            governance_mode: agentcowork_types::AgentGovernanceMode::SelfContained,
            bridge_id: None,
            state: agentcowork_types::BindingLifecycle::Parked,
            usage: Default::default(),
            last_event_seq: 0,
            private_state_ref: None,
        };
        gateway
            .create_agent_binding(binding)
            .map_err(AcpIdentityError::Binding)?;
        gateway
            .transition_agent_binding(
                &binding_id,
                "activated",
                Some(provider_session_id.to_string()),
            )
            .map_err(AcpIdentityError::Binding)?;
        activated_here = true;
    }

    // A caller may already have opened the canonical Run (the host automation
    // path does this before it enters ACP). Reuse an active Run for this
    // Session; otherwise start a new Run for this prompt. Never overwrite a
    // missing kernel record with a fresh id: that would make recovery
    // ambiguous.
    let existing_execution = gateway.execution_id(&work_id).map(str::to_string);
    let (execution, reused_execution) = if let Some(existing_id) = existing_execution {
        let Some(existing) = kernel.get(&existing_id) else {
            return Err(AcpIdentityError::Execution(format!(
                "Work {work_id} points at missing Run {existing_id}"
            )));
        };
        if existing.session_id != application_session_id {
            return Err(AcpIdentityError::Execution(format!(
                "Run {existing_id} belongs to Session {}, not {application_session_id}",
                existing.session_id
            )));
        }
        if let Ok(context) = serde_json::from_str::<serde_json::Value>(&existing.context_snapshot) {
            if let Some(bound_agent) = context.get("agentId").and_then(serde_json::Value::as_str) {
                if bound_agent != agent_id {
                    return Err(AcpIdentityError::Execution(format!(
                        "Run {existing_id} is owned by agent {bound_agent}, not {agent_id}"
                    )));
                }
            }
            if let Some(bound_binding) =
                context.get("bindingId").and_then(serde_json::Value::as_str)
            {
                if bound_binding != binding_id {
                    return Err(AcpIdentityError::Binding(format!(
                        "Run {existing_id} is bound to {bound_binding}, not {binding_id}"
                    )));
                }
            }
        }
        let resumable = matches!(
            existing.state,
            ExecutionPhase::Ready
                | ExecutionPhase::Running
                | ExecutionPhase::WaitingTool
                | ExecutionPhase::WaitingApproval
                | ExecutionPhase::WaitingUser
                | ExecutionPhase::Checkpointed
                | ExecutionPhase::Paused
                | ExecutionPhase::Recoverable
        );
        if resumable {
            if existing.state != ExecutionPhase::Running {
                if let Err(error) = kernel.transition(&existing_id, ExecutionPhase::Running) {
                    if activated_here {
                        let _ = gateway.transition_agent_binding(&binding_id, "suspended", None);
                    }
                    return Err(AcpIdentityError::Execution(error));
                }
                if let Err(error) = gateway.record_execution_transition(
                    &work_id,
                    &existing_id,
                    agentcowork_types::WorkState::Running,
                ) {
                    let _ = kernel.transition(&existing_id, ExecutionPhase::Failed);
                    if activated_here {
                        let _ = gateway.transition_agent_binding(&binding_id, "suspended", None);
                    }
                    return Err(AcpIdentityError::Work(error));
                }
            }
            (existing_id, true)
        } else {
            let execution = kernel
                .begin(
                    ExecutionTrigger::Acp,
                    application_session_id,
                    objective,
                    None,
                    String::new(),
                    serde_json::json!({
                        "sessionId": application_session_id,
                        "workId": work_id,
                        "bindingId": binding_id,
                        "agentId": agent_id,
                        "providerSessionId": provider_session_id,
                    })
                    .to_string(),
                    vec![],
                )
                .id;
            (execution, false)
        }
    } else {
        let execution = kernel
            .begin(
                ExecutionTrigger::Acp,
                application_session_id,
                objective,
                None,
                String::new(),
                serde_json::json!({
                    "sessionId": application_session_id,
                    "workId": work_id,
                    "bindingId": binding_id,
                    "agentId": agent_id,
                    "providerSessionId": provider_session_id,
                })
                .to_string(),
                vec![],
            )
            .id;
        (execution, false)
    };

    if !reused_execution {
        if let Err(error) = kernel.transition(&execution, ExecutionPhase::Running) {
            if activated_here {
                let _ = gateway.transition_agent_binding(&binding_id, "suspended", None);
            }
            return Err(AcpIdentityError::Execution(error));
        }
        if let Err(error) = gateway.bind_execution(&work_id, &execution) {
            let _ = kernel.transition(&execution, ExecutionPhase::Failed);
            if activated_here {
                let _ = gateway.transition_agent_binding(&binding_id, "suspended", None);
            }
            return Err(AcpIdentityError::Work(error));
        }
        if let Err(error) = gateway.record_execution_transition(
            &work_id,
            &execution,
            agentcowork_types::WorkState::Running,
        ) {
            let _ = kernel.transition(&execution, ExecutionPhase::Failed);
            if activated_here {
                let _ = gateway.transition_agent_binding(&binding_id, "suspended", None);
            }
            return Err(AcpIdentityError::Work(error));
        }
    }

    Ok(AcpTurnIdentity {
        owner: AcpCanonicalOwner {
            session_id: application_session_id.to_string(),
            work_id,
            binding_id,
        },
        run_id: execution,
    })
}

fn transition_acp_run(
    state: &State<'_, AppState>,
    work_id: &str,
    execution_id: &str,
    work_state: agentcowork_types::WorkState,
) -> Result<(), String> {
    let (gateway, kernel) = relay_planes(state).map_err(|e| e.to_string())?;
    let mut gateway = gateway.lock().map_err(|e| e.to_string())?;
    gateway
        .record_execution_transition(work_id, execution_id, work_state)
        .map_err(|e| e.to_string())?;
    let phase = match work_state {
        agentcowork_types::WorkState::WaitingApproval => ExecutionPhase::WaitingApproval,
        agentcowork_types::WorkState::WaitingTool => ExecutionPhase::WaitingTool,
        agentcowork_types::WorkState::WaitingUser => ExecutionPhase::WaitingUser,
        agentcowork_types::WorkState::Checkpointed => ExecutionPhase::Checkpointed,
        agentcowork_types::WorkState::Paused => ExecutionPhase::Paused,
        agentcowork_types::WorkState::Recoverable => ExecutionPhase::Recoverable,
        agentcowork_types::WorkState::Completed => ExecutionPhase::Completed,
        agentcowork_types::WorkState::Failed => ExecutionPhase::Failed,
        agentcowork_types::WorkState::Cancelled => ExecutionPhase::Cancelled,
        agentcowork_types::WorkState::Verifying => ExecutionPhase::Verifying,
        agentcowork_types::WorkState::Created
        | agentcowork_types::WorkState::Planning
        | agentcowork_types::WorkState::Ready
        | agentcowork_types::WorkState::Running => ExecutionPhase::Running,
    };
    let mut kernel = kernel.lock().map_err(|e| e.to_string())?;
    kernel
        .transition(execution_id, phase)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn record_acp_binding_usage(
    state: &State<'_, AppState>,
    owner: &AcpCanonicalOwner,
    usage: Option<&agentcowork_acp::PromptUsage>,
) -> Result<(), String> {
    let Some(usage) = usage.filter(|usage| usage.reported()) else {
        return Ok(());
    };
    let (gateway, _) = relay_planes(state).map_err(|e| e.to_string())?;
    let mut gateway = gateway.lock().map_err(|e| e.to_string())?;
    gateway
        .record_binding_usage(
            &owner.binding_id,
            agentcowork_types::BindingUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cost_micros: usage
                    .cost_usd
                    .map(|cost| (cost * 1_000_000.0).round().max(0.0) as u64)
                    .unwrap_or(0),
            },
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Drive one ACP prompt turn. The agent's `session/request_permission`
/// requests route through the shared Guard-2 service: `Allow` auto-allows,
/// `Block` denies, and `Ask` denies the current turn while minting a ticket
/// the user can approve (then re-prompt). Never auto-allows an `Ask`.
#[tauri::command]
pub fn acp_prompt(
    state: State<'_, AppState>,
    handle: String,
    text: String,
    handoff: Option<String>,
    refs: Option<Vec<String>>,
    session_id: String,
    binding_id: Option<String>,
) -> Result<serde_json::Value, String> {
    crate::ensure_sidecar(&state);
    // The caller's Session id is the canonical AgentCowork identity. Never
    // replace it with the provider's ACP session id.
    let application_session_id = session_id.trim().to_string();
    if application_session_id.is_empty() {
        return Err(AcpIdentityError::MissingApplicationSession.to_string());
    }
    let requested_binding_id = match binding_id {
        Some(binding_id) => {
            let binding_id = binding_id.trim();
            if binding_id.is_empty() {
                return Err(AcpIdentityError::Binding("binding id is empty".into()).to_string());
            }
            Some(binding_id.to_string())
        }
        None => None,
    };
    let handoff_declared = handoff
        .as_ref()
        .map(|h| !h.trim().is_empty())
        .unwrap_or(false);

    // Copy per-handle state out of the global registry and release the global
    // lock before any provider I/O. The per-handle session mutex serializes
    // turns for this handle; the cancellation hook remains independent.
    let (agent_id, cwd, embedded_context, provider_session_id, session, cancel, existing_owner) = {
        let sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
        let entry = sessions
            .get(&handle)
            .ok_or_else(|| format!("unknown ACP handle: {handle}"))?;
        if entry.auth_required {
            return Err("agent requires sign-in — run acp_authenticate first".to_string());
        }
        let readiness = agent_readiness_with_live(&entry.agent_id, Some(live_facts(entry)));
        if !readiness.is_ready() {
            return Err(format!(
                "agent '{}' is not ready: {readiness} — {}",
                entry.agent_id,
                readiness.summary()
            ));
        }
        let (agent_id, cwd, embedded_context, provider_session_id, session, cancel) = (
            entry.agent_id.clone(),
            entry.cwd.clone(),
            entry.embedded_context,
            entry.provider_session_id.clone(),
            entry.session.clone(),
            entry.cancel.clone(),
        );
        (
            agent_id,
            cwd,
            embedded_context,
            provider_session_id,
            session,
            cancel,
            entry.owner.clone(),
        )
    };

    let provider_session_id =
        provider_session_id.ok_or_else(|| AcpIdentityError::MissingProviderSession.to_string())?;
    if provider_session_id.trim().is_empty() {
        return Err(AcpIdentityError::MissingProviderSession.to_string());
    }
    if let Some(owner) = existing_owner.as_ref() {
        if owner.session_id != application_session_id {
            return Err(AcpIdentityError::OwnerMismatch {
                handle: handle.clone(),
                expected_session: application_session_id,
                actual_session: owner.session_id.clone(),
            }
            .to_string());
        }
        if owner.work_id != canonical_work_id(&application_session_id) {
            return Err(AcpIdentityError::WorkOwnerMismatch {
                work_id: owner.work_id.clone(),
                expected_session: application_session_id,
                actual_session: Some(owner.session_id.clone()),
            }
            .to_string());
        }
        if let Some(requested_binding_id) = requested_binding_id.as_deref() {
            if owner.binding_id != requested_binding_id {
                return Err(AcpIdentityError::BindingMismatch {
                    handle: handle.clone(),
                    expected_binding: requested_binding_id.to_string(),
                    actual_binding: owner.binding_id.clone(),
                }
                .to_string());
            }
        }
    }

    if existing_owner.is_none() {
        if let Some(requested_binding_id) = requested_binding_id.as_deref() {
            let expected_binding_id = canonical_binding_id(
                &application_session_id,
                &canonical_work_id(&application_session_id),
                &agent_id,
            );
            if requested_binding_id != expected_binding_id {
                return Err(AcpIdentityError::BindingMismatch {
                    handle: handle.clone(),
                    expected_binding: expected_binding_id,
                    actual_binding: requested_binding_id.to_string(),
                }
                .to_string());
            }
        }
    }

    // J11 session budget is checked against the application Session, never a
    // provider transcript id.
    if let Ok(relay) = state.chat_relay.lock() {
        if let Some(relay) = relay.as_ref() {
            relay
                .preflight_session_budget(&application_session_id)
                .map_err(|e| e.to_string())?;
        }
    }

    // Serialize this handle's lifecycle and prompt. No global handle-map lock
    // is held while WorkGateway journaling or ACP I/O runs.
    let mut provider_session = session.lock().map_err(|e| e.to_string())?;
    {
        let sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
        let entry = sessions
            .get(&handle)
            .ok_or_else(|| format!("unknown ACP handle: {handle}"))?;
        if let Some(owner) = entry.owner.as_ref() {
            if owner.session_id != application_session_id {
                return Err(AcpIdentityError::OwnerMismatch {
                    handle: handle.clone(),
                    expected_session: application_session_id,
                    actual_session: owner.session_id.clone(),
                }
                .to_string());
            }
        }
    }
    provider_session.reset_cancellation();

    let (gateway, kernel) = relay_planes(&state).map_err(|e| e.to_string())?;
    let identity = {
        let mut gateway = gateway.lock().map_err(|e| e.to_string())?;
        let mut kernel = kernel.lock().map_err(|e| e.to_string())?;
        prepare_acp_turn(
            &mut gateway,
            &mut kernel,
            &application_session_id,
            &agent_id,
            &provider_session_id,
            &text,
        )
        .map_err(|e| e.to_string())?
    };

    if let Some(requested_binding_id) = requested_binding_id.as_deref() {
        if identity.owner.binding_id != requested_binding_id {
            return Err(AcpIdentityError::BindingMismatch {
                handle: handle.clone(),
                expected_binding: identity.owner.binding_id.clone(),
                actual_binding: requested_binding_id.to_string(),
            }
            .to_string());
        }
    }

    // Publish the canonical owner only after Work/Run/Binding durability has
    // succeeded. A handle that was concurrently claimed by another Session is
    // rejected before provider I/O.
    {
        let mut sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
        let entry = sessions
            .get_mut(&handle)
            .ok_or_else(|| format!("unknown ACP handle: {handle}"))?;
        if let Some(owner) = entry.owner.as_ref() {
            if owner.session_id != application_session_id {
                return Err(AcpIdentityError::OwnerMismatch {
                    handle: handle.clone(),
                    expected_session: application_session_id,
                    actual_session: owner.session_id.clone(),
                }
                .to_string());
            }
            if owner != &identity.owner {
                return Err(AcpIdentityError::BindingMismatch {
                    handle: handle.clone(),
                    expected_binding: identity.owner.binding_id.clone(),
                    actual_binding: owner.binding_id.clone(),
                }
                .to_string());
            }
        }
        entry.owner = Some(identity.owner.clone());
        entry.run_id = Some(identity.run_id.clone());
    }

    let (mut prompt_text, prefix_fingerprint) = build_acp_prompt_with_passport(&state, &text);
    let prefix_event = {
        let mut sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
        let entry = sessions
            .get_mut(&handle)
            .ok_or_else(|| format!("unknown ACP handle: {handle}"))?;
        let event = entry
            .prefix_guard
            .observe(prefix_fingerprint, handoff_declared);
        if event == agentcowork_acp::PrefixEvent::UndeclaredMutation {
            eprintln!(
                "ACP stable-prefix mutation without a declared cache-boundary event on \
                 application Session {application_session_id}, provider session {provider_session_id} \
                 (handle {handle})"
            );
        }
        event
    };
    if let Some(bundle) = handoff.as_ref().map(|h| h.trim()).filter(|h| !h.is_empty()) {
        let capped: String = bundle.chars().take(6000).collect();
        prompt_text = format!("<chief_handoff>\n{capped}\n</chief_handoff>\n\n{prompt_text}");
    }

    let content = if embedded_context {
        let mut blocks = vec![PromptContent::text(prompt_text.clone())];
        for reference in refs.as_deref().unwrap_or_default() {
            if let Some(resource) = read_workspace_resource(&cwd, reference) {
                blocks.push(PromptContent::resource(resource.0, resource.1, resource.2));
            }
        }
        blocks
    } else {
        vec![PromptContent::text(prompt_text.clone())]
    };

    let guard = Arc::clone(&state.guard_service);
    let mut pending_tickets: Vec<String> = Vec::new();
    // FIX-03 — the ACP permission bridge. Trust (GuardService) is the only
    // decider; the bridge maps its verdict onto an option the agent actually
    // offered, requires a live single-use bound ticket for any allow, and
    // fails closed when it cannot answer (`ARCH/12-TRUST.md` §5, §11).
    let bridge = agentcowork_acp::PermissionBridge::new();
    let prompt_result = provider_session.prompt_with_content(content, |req| {
        if cancel.is_requested() {
            return PermissionDecision::deny();
        }
        let Ok(mut g) = guard.lock() else {
            return PermissionDecision::deny();
        };
        let (op, risk) = map_tool_call(&req.tool_call);
        // The approval card must show *what* is being approved: the preview is
        // the bounded, secret-redacted projection of the agent's own request
        // (diff, script lines, destinations), so the human approves against the
        // change rather than a title. The preview is the only source of that
        // content — this call site just maps it onto the canonical Guard-2
        // decision package.
        let preview = agentcowork_acp::PermissionPreview::build(req, op.name(), risk);
        let decision = DecisionPackage::new(preview.title.clone())
            .with_diff(preview.diff.clone())
            .with_risk(risk)
            .with_paths(preview.paths.clone())
            .with_script(
                preview.script_lines.clone(),
                preview.execution_target.clone(),
            )
            .with_network(preview.network_destinations.clone());
        let args_hash = hash_tool_args(&req.tool_call);
        let binding = agentcowork_acp::TicketBinding {
            agent_id: agent_id.clone(),
            session_id: application_session_id.clone(),
            args_hash: args_hash.clone(),
        };
        let tool_call_id = req.tool_call.tool_call_id.clone();
        // The one decider runs first; only its own verdict decides anything.
        let verdict = g.evaluate(
            &application_session_id,
            &agent_id,
            &tool_call_id,
            op,
            decision,
            &args_hash,
            0,
        );
        // Spend the ticket exactly as Trust issued it: `use_ticket` enforces
        // approval → validity → args → single-use, and its result is never
        // discarded (a stale, expired or already-used ticket is a denial, not
        // an allow).
        let spend = |g: &mut agentcowork_core::GuardService, ticket_id: &str| -> bool {
            match g.use_ticket(ticket_id, &args_hash) {
                Ok(()) => !cancel.is_requested(),
                Err(error) => {
                    eprintln!(
                        "agentcowork: ACP permission `{tool_call_id}` refused — ticket {ticket_id} \
                         could not be spent: {error}"
                    );
                    false
                }
            }
        };
        let trust = match verdict {
            GuardDecision::Allow { ticket_id } => {
                if spend(&mut g, &ticket_id) {
                    agentcowork_acp::TrustOutcome::once(
                        acp_ticket_facts(&ticket_id, &binding),
                        "guard allow (policy or standing rule)",
                    )
                } else {
                    agentcowork_acp::TrustOutcome::reject("ticket spend refused")
                }
            }
            GuardDecision::Block { reason } => {
                agentcowork_acp::TrustOutcome::reject(format!("guard deny: {reason}"))
            }
            GuardDecision::Ask { ticket_id } => {
                pending_tickets.push(ticket_id.clone());
                let rx = g.watch_ticket(&ticket_id);
                drop(g);
                // The approval waits on the channel that owns the binding
                // (`ARCH/32-CHANNELS.md` §7): a disconnect, a cancel, or a
                // lost channel all resolve to a denial, never an implicit allow.
                let approved = loop {
                    if cancel.is_requested() {
                        break false;
                    }
                    match rx.recv_timeout(std::time::Duration::from_millis(250)) {
                        Ok(value) => break value,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break false,
                    }
                };
                let Ok(mut g) = guard.lock() else {
                    return PermissionDecision::deny();
                };
                if approved && spend(&mut g, &ticket_id) {
                    agentcowork_acp::TrustOutcome::once(
                        acp_ticket_facts(&ticket_id, &binding),
                        "human approved on the owning channel",
                    )
                } else if approved {
                    agentcowork_acp::TrustOutcome::reject("ticket spend refused after approval")
                } else {
                    agentcowork_acp::TrustOutcome::reject("human rejected or wait ended")
                }
            }
        };
        match bridge.answer(req, &binding, &trust) {
            Ok(answer) => {
                if answer.narrowed {
                    eprintln!(
                        "agentcowork: ACP permission `{tool_call_id}` answered as `{}` — no durable \
                         policy change was recorded, so the standing grant was not created",
                        answer.choice.card_vocabulary()
                    );
                }
                answer.decision
            }
            Err(error) => {
                // Fail closed: the bridge could not express the decision, so no
                // option is granted. `resolve_option` will refuse to name an
                // option the agent never offered and the turn ends.
                eprintln!("agentcowork: ACP permission `{tool_call_id}` failed closed: {error}");
                PermissionDecision::deny()
            }
        }
    });
    drop(provider_session);

    let outcome = match prompt_result {
        Ok(outcome) => outcome,
        Err(AcpError::Cancelled) => {
            let _ = transition_acp_run(
                &state,
                &identity.owner.work_id,
                &identity.run_id,
                agentcowork_types::WorkState::Cancelled,
            );
            return Err("ACP turn cancelled".to_string());
        }
        Err(error) => {
            let _ = transition_acp_run(
                &state,
                &identity.owner.work_id,
                &identity.run_id,
                agentcowork_types::WorkState::Failed,
            );
            return Err(format!("ACP prompt failed: {error}"));
        }
    };

    let latest_config = {
        let session = session.lock().map_err(|e| e.to_string())?;
        session.config_options().to_vec()
    };
    {
        let mut sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
        if let Some(entry) = sessions.get_mut(&handle) {
            entry.config_options = latest_config;
            for update in &outcome.updates {
                if update.is_available_commands_update() && !update.available_commands.is_empty() {
                    entry.available_commands = update.available_commands.clone();
                }
                if update.is_config_option_update() && !update.config_options.is_empty() {
                    entry.config_options = update.config_options.clone();
                }
            }
        }
    }
    append_acp_tool_log(
        &application_session_id,
        &provider_session_id,
        &handle,
        &agent_id,
        &text,
        &outcome,
        prefix_event,
    );

    // The durable binding receives the same agent-reported usage observation
    // as the memory ledger; no provider credential or model value crosses this
    // boundary.
    let binding_usage_error =
        record_acp_binding_usage(&state, &identity.owner, outcome.usage.as_ref()).err();

    let memory_arc = state
        .chat_relay
        .lock()
        .ok()
        .and_then(|relay| relay.as_ref().map(|r| r.memory()));
    if let Some(mem) = memory_arc {
        if let Ok(mut m) = mem.lock() {
            m.set_primary_agent(&agent_id);
            match outcome.usage {
                Some(u) if u.reported() => m.record_usage_from(
                    agentcowork_core::UsageSource::AgentReport,
                    &agent_id,
                    &agent_id,
                    &application_session_id,
                    u.input_tokens,
                    u.output_tokens,
                    u.cached_read_tokens > 0,
                    u.cached_read_tokens,
                    u.cached_write_tokens,
                    u.cost_usd.unwrap_or(0.0),
                ),
                _ => m.record_usage_unreported(&agent_id),
            }
        }
    }

    let transition_error = if outcome.stop_reason == agentcowork_acp::StopReason::Cancelled {
        transition_acp_run(
            &state,
            &identity.owner.work_id,
            &identity.run_id,
            agentcowork_types::WorkState::Cancelled,
        )
        .err()
    } else if pending_tickets.is_empty() {
        transition_acp_run(
            &state,
            &identity.owner.work_id,
            &identity.run_id,
            agentcowork_types::WorkState::Verifying,
        )
        .and_then(|_| {
            transition_acp_run(
                &state,
                &identity.owner.work_id,
                &identity.run_id,
                agentcowork_types::WorkState::Completed,
            )
        })
        .err()
    } else {
        transition_acp_run(
            &state,
            &identity.owner.work_id,
            &identity.run_id,
            agentcowork_types::WorkState::WaitingApproval,
        )
        .err()
    };
    let lifecycle_error = match (transition_error, binding_usage_error) {
        (Some(transition), Some(usage)) => Some(format!("{transition}; binding usage: {usage}")),
        (Some(transition), None) => Some(transition),
        (None, Some(usage)) => Some(format!("binding usage: {usage}")),
        (None, None) => None,
    };

    let final_text = outcome
        .updates
        .iter()
        .flat_map(|u| u.content.iter())
        .filter_map(|block| {
            if block.r#type == "text" || !block.text.is_empty() {
                Some(block.text.as_str())
            } else {
                None
            }
        })
        .collect::<String>();

    Ok(serde_json::json!({
        "handle": handle,
        "applicationSessionId": identity.owner.session_id,
        "workId": identity.owner.work_id,
        "bindingId": identity.owner.binding_id,
        "runId": identity.run_id,
        "providerSessionId": provider_session_id,
        "stopReason": outcome.stop_reason.as_str(),
        "updateCount": outcome.updates.len(),
        "permissionCount": outcome.permissions.len(),
        "pendingTickets": pending_tickets,
        "finalText": final_text,
        "updates": outcome.updates,
        "executionId": identity.run_id,
        "lifecycleError": lifecycle_error,
    }))
}

fn validate_cancel_owner(
    handle: &str,
    owner: Option<&AcpCanonicalOwner>,
    requested_session: Option<&str>,
    requested_binding: Option<&str>,
) -> Result<(), AcpIdentityError> {
    let Some(owner) = owner else {
        if requested_session.is_some() || requested_binding.is_some() {
            return Err(AcpIdentityError::NoCanonicalOwner {
                handle: handle.to_string(),
            });
        }
        return Err(AcpIdentityError::NoCanonicalOwner {
            handle: handle.to_string(),
        });
    };
    if let Some(session_id) = requested_session {
        if owner.session_id != session_id {
            return Err(AcpIdentityError::OwnerMismatch {
                handle: handle.to_string(),
                expected_session: session_id.to_string(),
                actual_session: owner.session_id.clone(),
            });
        }
    }
    if let Some(binding_id) = requested_binding {
        if owner.binding_id != binding_id {
            return Err(AcpIdentityError::BindingMismatch {
                handle: handle.to_string(),
                expected_binding: binding_id.to_string(),
                actual_binding: owner.binding_id.clone(),
            });
        }
    }
    Ok(())
}

#[derive(Clone)]
struct AcpCancellationTarget {
    handle: String,
    provider_session_id: String,
    owner: AcpCanonicalOwner,
    cancel: AcpCancelHandle,
}

fn request_acp_cancellation(target: &AcpCancellationTarget) -> Result<(), String> {
    if target.provider_session_id.trim().is_empty() {
        return Err(format!(
            "{}: {}",
            target.handle,
            AcpIdentityError::MissingProviderSession
        ));
    }
    target
        .cancel
        .request(&target.provider_session_id)
        .map_err(|e| format!("ACP cancellation request failed for {}: {e}", target.handle))
}

/// Request cancellation only for targets whose canonical owner is the requested
/// application Session. This helper deliberately has no Work/Run transition:
/// sending `session/cancel` is only a provider request. The prompt's observed
/// cancellation result is the evidence that may settle the durable Run.
fn cancel_acp_targets_for_session(
    targets: impl IntoIterator<Item = AcpCancellationTarget>,
    session_id: &str,
) -> Result<Vec<String>, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Ok(Vec::new());
    }
    let mut cancelled = Vec::new();
    let mut errors = Vec::new();
    for target in targets {
        if target.owner.session_id != session_id {
            continue;
        }
        match request_acp_cancellation(&target) {
            Ok(()) => cancelled.push(target.handle),
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        Ok(cancelled)
    } else {
        Err(errors.join("; "))
    }
}

/// Interrupt one canonical ACP turn. Both owner fields are mandatory: a stale
/// handle must not become a global cancellation primitive when a renderer or
/// control caller omits identity.
#[tauri::command]
pub fn acp_cancel(
    state: State<'_, AppState>,
    handle: String,
    session_id: String,
    binding_id: String,
) -> Result<(), String> {
    let session_id = session_id.trim().to_string();
    let binding_id = binding_id.trim().to_string();
    if session_id.is_empty() {
        return Err(AcpIdentityError::MissingApplicationSession.to_string());
    }
    if binding_id.is_empty() {
        return Err(AcpIdentityError::Binding("binding id is empty".into()).to_string());
    }
    let (cancel, provider_session_id, owner) = {
        let sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
        let entry = sessions
            .get(&handle)
            .ok_or_else(|| format!("unknown ACP handle: {handle}"))?;
        validate_cancel_owner(
            &handle,
            entry.owner.as_ref(),
            Some(&session_id),
            Some(&binding_id),
        )
        .map_err(|e| e.to_string())?;
        let provider_session_id = entry
            .provider_session_id
            .clone()
            .ok_or_else(|| AcpIdentityError::MissingProviderSession.to_string())?;
        (
            entry.cancel.clone(),
            provider_session_id,
            entry.owner.clone(),
        )
    };
    let target = AcpCancellationTarget {
        handle: handle.clone(),
        provider_session_id,
        owner: owner
            .ok_or_else(|| AcpIdentityError::NoCanonicalOwner { handle })
            .map_err(|e| e.to_string())?,
        cancel,
    };
    // The global map lock is released before writing to the provider pipe.
    request_acp_cancellation(&target)
}

/// Cancel every live ACP handle owned by one application Session. This is the
/// scoped stop seam used by the control-channel `agent_stop` path; callers must
/// use it instead of enumerating agent ids.
pub(crate) fn cancel_acp_for_session(
    state: &State<'_, AppState>,
    session_id: &str,
) -> Result<Vec<String>, String> {
    let session_id = session_id.trim();
    let targets: Vec<AcpCancellationTarget> = {
        let sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
        sessions
            .iter()
            .filter_map(|(handle, entry)| {
                let owner = entry.owner.as_ref()?.clone();
                if owner.session_id != session_id {
                    return None;
                }
                Some(AcpCancellationTarget {
                    handle: handle.clone(),
                    provider_session_id: entry.provider_session_id.clone().unwrap_or_default(),
                    owner,
                    cancel: entry.cancel.clone(),
                })
            })
            .collect()
    };
    cancel_acp_targets_for_session(targets, session_id)
}

/// Tear an ACP session down (kill + reap) and drop its handle. The provider
/// process is shut down outside the global registry lock, then the durable
/// binding is parked when a WorkGateway is available.
#[tauri::command]
pub fn acp_shutdown(state: State<'_, AppState>, handle: String) -> Result<bool, String> {
    let (session, owner) = {
        let mut sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
        let Some(entry) = sessions.remove(&handle) else {
            return Ok(false);
        };
        (entry.session.clone(), entry.owner)
    };
    {
        let mut session = session.lock().map_err(|e| e.to_string())?;
        session.shutdown();
    }
    if let Some(owner) = owner {
        if let Ok((gateway, _)) = relay_planes(&state) {
            let mut gateway = gateway.lock().map_err(|e| e.to_string())?;
            let _ = gateway.transition_agent_binding(&owner.binding_id, "suspended", None);
        }
    }
    Ok(true)
}

/// Live ACP handles (the harness list in the cockpit).
#[tauri::command]
pub fn acp_sessions(state: State<'_, AppState>) -> Result<Vec<AcpHandleInfo>, String> {
    let sessions = state.acp_sessions.lock().map_err(|e| e.to_string())?;
    Ok(sessions
        .iter()
        .map(|(handle, entry)| AcpHandleInfo::from((entry, handle.as_str())))
        .collect())
}

/// Read one user-selected workspace file for an ACP resource block. The
/// canonical path check prevents `@` refs from escaping the session folder;
/// oversized files are truncated before crossing the ACP boundary.
fn read_workspace_resource(cwd: &str, reference: &str) -> Option<(String, String, String)> {
    let raw = reference.trim().trim_start_matches('@');
    if raw.is_empty() || raw.contains('\0') {
        return None;
    }
    let base = std::fs::canonicalize(cwd).ok()?;
    let path = std::path::Path::new(raw);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let canonical = std::fs::canonicalize(candidate).ok()?;
    if !canonical.starts_with(&base) || !canonical.is_file() {
        return None;
    }
    let bytes = std::fs::read(&canonical).ok()?;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(128 * 1024)]).into_owned();
    let mime = match canonical.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "json" => "application/json",
        "md" | "markdown" => "text/markdown",
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" => "text/plain",
        _ => "text/plain",
    };
    Some((
        format!("file://{}", canonical.to_string_lossy()),
        mime.to_string(),
        text,
    ))
}

/// Map an ACP tool call onto a Guard-2 operation + risk tier so it routes
/// through the same policy engine as native tools (F9 shared taxonomy).
///
/// One mapping, and it is the only one: the ACP wire's tool kind is the
/// translation boundary, and the operation + risk it yields are what Guard
/// decides on. Note that a non-mutating kind still maps to a *write* operation
/// at low risk — the classification is deliberately conservative, and Guard's
/// own layers (human floor, protected paths, risk tier) decide what happens
/// next.
fn map_tool_call(tc: &ToolCall) -> (Operation, RiskLevel) {
    match tc.kind {
        Some(ToolKind::Delete) => (Operation::DeleteFiles, RiskLevel::High),
        Some(ToolKind::Execute) => (
            Operation::TerminalShell { destructive: false },
            RiskLevel::High,
        ),
        Some(ToolKind::Edit) | Some(ToolKind::Move) => (Operation::GenericWrite, RiskLevel::Medium),
        // read / search / think / fetch / unknown → non-mutating, auto-allow.
        _ => (Operation::GenericWrite, RiskLevel::Low),
    }
}

/// The ticket facts the ACP permission bridge validates before it will express
/// an allow (FIX-03 / `TASK-CHAN-001`).
///
/// `GuardService` deliberately exposes no ticket getter (a ticket is not a
/// read API), so the facts are reconstructed from exactly what this call site
/// asked Guard to mint plus the `use_ticket` result that already succeeded: the
/// same `agent_id` / `session_id` / `args_hash` it passed to `evaluate`, and the
/// single-use declaration `evaluate` always sets (pinned end-to-end by
/// `an_acp_permission_ticket_is_spent_exactly_once` below). A mismatch in any
/// of them — or a spend Guard refused — makes the bridge refuse the allow, so a
/// fabricated or stale approval can never reach the agent.
fn acp_ticket_facts(
    ticket_id: &str,
    binding: &agentcowork_acp::TicketBinding,
) -> agentcowork_acp::TicketFacts {
    agentcowork_acp::TicketFacts {
        ticket_id: ticket_id.to_string(),
        agent_id: binding.agent_id.clone(),
        session_id: binding.session_id.clone(),
        args_hash: binding.args_hash.clone(),
        single_use: true,
        // True only because the caller reached here *after* `use_ticket`
        // returned `Ok` for this exact ticket and args hash.
        validated_by_guard: true,
    }
}

/// A stable args fingerprint so the minted ticket is single-use on the exact
/// request (the executor compares this at `guard/use`).
fn hash_tool_args(tc: &ToolCall) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    tc.tool_call_id.hash(&mut h);
    tc.title.hash(&mut h);
    if let Some(raw) = &tc.raw_input {
        serde_json::to_string(raw).unwrap_or_default().hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// The install ticket's args-hash — deterministic from (agent, version) so
/// the request and commit halves always agree (single-use enforcement).
fn install_args_hash(agent_id: &str, version: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    "acp.install".hash(&mut h);
    agent_id.hash(&mut h);
    version.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The host of an archive URL (for the decision card's network scope).
fn url_host(url: &str) -> String {
    url.split('/').nth(2).unwrap_or(url).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_b_governance_claim_matches_the_mounted_server_list() {
        assert_eq!(
            governance_for_channel_b_servers(&[]),
            agentcowork_acp::GovernedSession::SelfContained { channel_b: false }
        );

        let mounted = [agentcowork_acp::McpServer::stdio(
            "core-tools",
            "/usr/bin/core-tools",
            Vec::new(),
            Vec::new(),
        )];
        assert_eq!(
            governance_for_channel_b_servers(&mounted),
            agentcowork_acp::GovernedSession::SelfContained { channel_b: true }
        );
    }

    /// P71.12 — an agent that never negotiated `loadSession` is refused on the
    /// capability itself, before any binding record is consulted.
    #[test]
    fn session_load_refuses_an_agent_that_did_not_negotiate_load_session() {
        assert_eq!(
            AcpSessionLoadRefusal::decide(false, Some("provider-1")),
            AcpSessionLoadRefusal::CapabilityNotNegotiated
        );
        // Omission is refusal even when a provider session id *is* recorded:
        // capability is checked first, so no recorded id can imply consent.
        let message = AcpSessionLoadRefusal::CapabilityNotNegotiated.to_string();
        assert!(message.contains("loadSession"));
        assert!(message.contains("does not guess from silence"));
        assert!(!message.contains('{'), "no debug enum text: {message}");
    }

    /// A blank/absent recorded provider session id is refused, never invented.
    #[test]
    fn session_load_refuses_rather_than_inventing_a_provider_session_id() {
        for recorded in [None, Some(""), Some("   ")] {
            assert_eq!(
                AcpSessionLoadRefusal::decide(true, recorded),
                AcpSessionLoadRefusal::NoRecordedProviderSession
            );
        }
        let message = AcpSessionLoadRefusal::NoRecordedProviderSession.to_string();
        assert!(message.contains("never invents a provider session id"));
    }

    /// With both preconditions satisfied the refusal is still the v1 policy,
    /// and it names that policy in words a user can act on.
    #[test]
    fn session_load_refusal_names_the_adr_0007_v1_policy() {
        assert_eq!(
            AcpSessionLoadRefusal::decide(true, Some("provider-1")),
            AcpSessionLoadRefusal::ProviderResumeOutOfScope
        );
        let message = AcpSessionLoadRefusal::ProviderResumeOutOfScope.to_string();
        assert!(message.contains("ARCH/ADR/0007 section 4"));
        assert!(message.contains("session/resume"));
        assert!(message.contains("not available in this release"));
        assert!(message.contains("provider session restarted"));
        assert!(message.contains("not a native resume"));
    }

    /// Every refusal reaches the user as prose, never as a raw enum/path.
    #[test]
    fn every_session_load_refusal_is_plain_language() {
        for refusal in [
            AcpSessionLoadRefusal::CapabilityNotNegotiated,
            AcpSessionLoadRefusal::NoRecordedProviderSession,
            AcpSessionLoadRefusal::ProviderResumeOutOfScope,
        ] {
            let message = refusal.to_string();
            assert!(
                !message.contains("AcpSessionLoadRefusal"),
                "leaked the enum type: {message}"
            );
            assert!(!message.contains("::"), "leaked a variant path: {message}");
            assert!(!message.contains('{') && !message.contains('}'));
            assert!(message.ends_with('.'), "not a sentence: {message}");
        }
    }

    /// The recorded provider session id is read from the canonical binding, and
    /// only under the application Session + agent that own it.
    #[test]
    fn recorded_provider_session_id_comes_from_the_canonical_binding() {
        let mut gateway = agentcowork_core::WorkGateway::new();
        let mut kernel = agentcowork_core::ExecutionKernel::new();
        let identity = prepare_acp_turn(
            &mut gateway,
            &mut kernel,
            "application-a",
            "shared-agent",
            "provider-a",
            "objective",
        )
        .unwrap();

        assert_eq!(
            recorded_binding_provider_session_id(&gateway, "application-a", "shared-agent"),
            Some("provider-a".to_string())
        );
        // A different agent on the same Work has no binding of its own, so there
        // is no provider session to read — and none is fabricated.
        assert_eq!(
            recorded_binding_provider_session_id(&gateway, "application-a", "other-agent"),
            None
        );
        // An unknown Session has no Work, so there is no record to read.
        assert_eq!(
            recorded_binding_provider_session_id(&gateway, "application-b", "shared-agent"),
            None
        );
        // The provider id is never a Session/Work key: looking it up as one
        // finds nothing.
        assert_eq!(
            recorded_binding_provider_session_id(&gateway, "provider-a", "shared-agent"),
            None
        );
        assert_eq!(identity.owner.work_id, "application-a");
    }

    #[test]
    fn agent_without_advertised_config_options_is_native_only_and_unchanged() {
        let mut apply_count = 0_u32;
        let response = config_option_request_projection(
            "native-agent",
            Some("binding-native"),
            "acp-fixture",
            "model",
            serde_json::json!("fixture-model"),
            &[],
            |_, _| {
                apply_count += 1;
                Ok(Vec::new())
            },
        )
        .unwrap();
        assert_eq!(apply_count, 0);
        assert_eq!(response["control"], "NativeOnly");
        assert_eq!(response["reason"], NATIVE_ONLY_MODEL_REASON);
        assert_eq!(response["state"], "requested");
        assert_eq!(response["agentConfirmed"], false);
        assert_eq!(response["options"], serde_json::json!([]));
    }

    #[test]
    fn missing_application_or_provider_identity_fails_closed() {
        let mut gateway = agentcowork_core::WorkGateway::new();
        let mut kernel = agentcowork_core::ExecutionKernel::new();
        assert!(matches!(
            prepare_acp_turn(
                &mut gateway,
                &mut kernel,
                "",
                "agent",
                "provider",
                "objective",
            ),
            Err(AcpIdentityError::MissingApplicationSession)
        ));
        assert!(matches!(
            prepare_acp_turn(
                &mut gateway,
                &mut kernel,
                "application",
                "agent",
                "",
                "objective",
            ),
            Err(AcpIdentityError::MissingProviderSession)
        ));
        assert!(gateway.list_work().is_empty());
    }

    #[test]
    fn canonical_owner_keeps_provider_ids_separate_for_shared_agent() {
        let mut gateway = agentcowork_core::WorkGateway::new();
        let mut kernel = agentcowork_core::ExecutionKernel::new();
        let first = prepare_acp_turn(
            &mut gateway,
            &mut kernel,
            "application-a",
            "shared-agent",
            "provider-a",
            "first",
        )
        .unwrap();
        let second = prepare_acp_turn(
            &mut gateway,
            &mut kernel,
            "application-b",
            "shared-agent",
            "provider-b",
            "second",
        )
        .unwrap();

        assert_eq!(first.owner.session_id, "application-a");
        assert_eq!(second.owner.session_id, "application-b");
        assert_ne!(first.owner.binding_id, second.owner.binding_id);
        assert_eq!(
            gateway
                .agent_binding(&first.owner.binding_id)
                .and_then(|b| b.provider_session_id.as_deref()),
            Some("provider-a")
        );
        assert_eq!(
            gateway
                .agent_binding(&second.owner.binding_id)
                .and_then(|b| b.provider_session_id.as_deref()),
            Some("provider-b")
        );
        assert_eq!(
            kernel.get(&first.run_id).unwrap().session_id,
            "application-a"
        );
        assert_eq!(
            kernel.get(&second.run_id).unwrap().session_id,
            "application-b"
        );
    }

    #[test]
    fn existing_active_run_is_resolved_instead_of_replaced() {
        let mut gateway = agentcowork_core::WorkGateway::new();
        let mut kernel = agentcowork_core::ExecutionKernel::new();
        gateway
            .create_work_in_session(
                "automation-session",
                None,
                Some("automation-session".into()),
                agentcowork_types::SessionKind::Automation,
                "scheduled objective",
            )
            .unwrap();
        let existing = kernel
            .begin(
                ExecutionTrigger::Scheduler,
                "automation-session",
                "scheduled objective",
                None,
                String::new(),
                r#"{"trigger":"scheduler"}"#.into(),
                vec![],
            )
            .id;
        kernel
            .transition(&existing, ExecutionPhase::Running)
            .unwrap();
        gateway
            .bind_execution("automation-session", &existing)
            .unwrap();
        gateway
            .record_execution_transition(
                "automation-session",
                &existing,
                agentcowork_types::WorkState::Running,
            )
            .unwrap();

        let identity = prepare_acp_turn(
            &mut gateway,
            &mut kernel,
            "automation-session",
            "scheduled-agent",
            "provider-1",
            "scheduled objective",
        )
        .unwrap();
        assert_eq!(identity.run_id, existing);
        assert_eq!(
            gateway.execution_id("automation-session"),
            Some(existing.as_str())
        );
    }

    #[test]
    fn binding_and_run_replay_from_the_work_journal() {
        let path = std::env::temp_dir().join(format!(
            "agentcowork-acp-identity-{}-{}.jsonl",
            std::process::id(),
            ACP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut gateway = agentcowork_core::WorkGateway::open(&path).unwrap();
        let mut kernel = agentcowork_core::ExecutionKernel::new();
        let identity = prepare_acp_turn(
            &mut gateway,
            &mut kernel,
            "replay-session",
            "replay-agent",
            "provider-replay",
            "replay",
        )
        .unwrap();
        gateway
            .record_binding_usage(
                &identity.owner.binding_id,
                agentcowork_types::BindingUsage {
                    input_tokens: 7,
                    output_tokens: 3,
                    cost_micros: 11,
                },
            )
            .unwrap();
        drop(gateway);
        drop(kernel);

        let replay = agentcowork_core::WorkGateway::open(&path).unwrap();
        let binding = replay.agent_binding(&identity.owner.binding_id).unwrap();
        assert_eq!(binding.session_id.as_str(), "replay-session");
        assert_eq!(
            binding.provider_session_id.as_deref(),
            Some("provider-replay")
        );
        assert_eq!(binding.usage.input_tokens, 7);
        assert_eq!(binding.usage.output_tokens, 3);
        assert_eq!(binding.usage.cost_micros, 11);
        assert_eq!(
            replay.execution_id("replay-session"),
            Some(identity.run_id.as_str())
        );
        assert_eq!(
            replay
                .get_work("replay-session")
                .unwrap()
                .session_id
                .as_deref(),
            Some("replay-session")
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn owner_validation_rejects_a_handle_from_another_session() {
        let owner = AcpCanonicalOwner {
            session_id: "session-a".into(),
            work_id: "session-a".into(),
            binding_id: "binding-a".into(),
        };
        assert!(validate_cancel_owner("h1", Some(&owner), Some("session-a"), None).is_ok());
        let other = AcpCanonicalOwner {
            session_id: "session-b".into(),
            work_id: "session-b".into(),
            binding_id: "binding-b".into(),
        };
        assert!(validate_cancel_owner("h2", Some(&other), Some("session-b"), None).is_ok());
        let error = validate_cancel_owner("h1", Some(&owner), Some("session-b"), None)
            .expect_err("a handle cannot be reused by another Session");
        assert!(matches!(error, AcpIdentityError::OwnerMismatch { .. }));
        let binding_error =
            validate_cancel_owner("h1", Some(&owner), Some("session-a"), Some("binding-b"))
                .expect_err("a handle cannot be reused by another binding");
        assert!(matches!(
            binding_error,
            AcpIdentityError::BindingMismatch { .. }
        ));
    }

    #[test]
    fn session_scoped_cancellation_leaves_other_session_handles_alone() {
        let owner_a = AcpCanonicalOwner {
            session_id: "session-a".into(),
            work_id: "work-a".into(),
            binding_id: "binding-a".into(),
        };
        let owner_b = AcpCanonicalOwner {
            session_id: "session-b".into(),
            work_id: "work-b".into(),
            binding_id: "binding-b".into(),
        };
        let flag_a = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag_b = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_a = AcpCancelHandle::new(Arc::clone(&flag_a), None);
        let cancel_b = AcpCancelHandle::new(Arc::clone(&flag_b), None);
        let cancelled = cancel_acp_targets_for_session(
            [
                AcpCancellationTarget {
                    handle: "handle-a".into(),
                    provider_session_id: "provider-a".into(),
                    owner: owner_a,
                    cancel: cancel_a,
                },
                AcpCancellationTarget {
                    handle: "handle-b".into(),
                    provider_session_id: "provider-b".into(),
                    owner: owner_b,
                    cancel: cancel_b,
                },
            ],
            "session-a",
        )
        .unwrap();

        assert_eq!(cancelled, vec!["handle-a".to_string()]);
        assert!(flag_a.load(Ordering::Acquire));
        assert!(!flag_b.load(Ordering::Acquire));
    }

    #[test]
    fn delete_tool_maps_to_high_risk_delete() {
        let tc = ToolCall {
            tool_call_id: "t1".into(),
            title: "rm -rf".into(),
            kind: Some(ToolKind::Delete),
            ..Default::default()
        };
        let (op, risk) = map_tool_call(&tc);
        assert!(matches!(op, Operation::DeleteFiles));
        assert_eq!(risk, RiskLevel::High);
    }

    #[test]
    fn read_tool_maps_to_low_risk_write() {
        let tc = ToolCall {
            tool_call_id: "t2".into(),
            title: "read file".into(),
            kind: Some(ToolKind::Read),
            ..Default::default()
        };
        let (_, risk) = map_tool_call(&tc);
        assert_eq!(risk, RiskLevel::Low);
        let (op, _) = map_tool_call(&tc);
        assert!(matches!(op, Operation::GenericWrite));
    }

    /// FIX-03: the ticket the ACP permission bridge relies on is genuinely
    /// single-use, end to end through the real `GuardService` — the fact the
    /// bridge asserts is not an assumption.
    #[test]
    fn an_acp_permission_ticket_is_spent_exactly_once() {
        let mut g = agentcowork_core::GuardService::new();
        let verdict = g.evaluate(
            "session-1",
            "claude-code",
            "acp.tc-1",
            Operation::GenericWrite,
            DecisionPackage::new("edit a.rs"),
            "args-1",
            0,
        );
        let ticket_id = match verdict {
            GuardDecision::Allow { ticket_id } => ticket_id,
            GuardDecision::Ask { ticket_id } => {
                assert!(g.approve(&ticket_id), "a pending ticket needs a decision");
                ticket_id
            }
            GuardDecision::Block { reason } => panic!("unexpected block: {reason}"),
        };
        assert!(
            g.use_ticket(&ticket_id, "args-1").is_ok(),
            "the first spend consumes the ticket"
        );
        assert!(
            g.use_ticket(&ticket_id, "args-1").is_err(),
            "a second spend must be refused: an ACP allow is never replayable"
        );
        // A different argument set is refused too — the ticket is bound to the
        // exact request (`DM-009`).
        let mut other = agentcowork_core::GuardService::new();
        let verdict = other.evaluate(
            "session-1",
            "claude-code",
            "acp.tc-2",
            Operation::GenericWrite,
            DecisionPackage::new("edit a.rs"),
            "args-1",
            0,
        );
        if let GuardDecision::Allow { ticket_id } | GuardDecision::Ask { ticket_id } = verdict {
            other.approve(&ticket_id);
            assert!(other.use_ticket(&ticket_id, "args-2").is_err());
        }
    }

    #[test]
    fn args_hash_is_stable_for_same_input() {
        let a = ToolCall {
            tool_call_id: "t1".into(),
            title: "x".into(),
            ..Default::default()
        };
        let b = a.clone();
        assert_eq!(hash_tool_args(&a), hash_tool_args(&b));
    }

    #[test]
    fn install_args_hash_is_deterministic_and_scoped() {
        assert_eq!(
            install_args_hash("devin", "3000.4.25"),
            install_args_hash("devin", "3000.4.25")
        );
        assert_ne!(
            install_args_hash("devin", "3000.4.25"),
            install_args_hash("devin", "3000.4.26")
        );
        assert_ne!(
            install_args_hash("devin", "3000.4.25"),
            install_args_hash("kiro", "3000.4.25")
        );
    }

    #[test]
    fn url_host_extracts_authority() {
        assert_eq!(
            url_host("https://static.devin.ai/cli/1.0/devin.tar.gz"),
            "static.devin.ai"
        );
        assert_eq!(url_host("https://x.ai"), "x.ai");
    }

    #[test]
    fn registry_has_no_builtin_and_lists_launch_agents() {
        let reg = LaunchRegistry::builtin();
        // ADR-0005 §D1: no built-in/default identity ships in the catalog.
        // NOTE (DEC-053 Step 4): `everyaios` stays asserted-absent — the
        // retired agent id, recognized so it is never resurrected.
        assert!(reg.get("everyaios").is_none());
        assert!(reg.get("claude").is_some());
        assert!(reg.get("codex").is_some());
    }

    #[test]
    fn resolve_on_path_finds_real_binaries_and_misses_absences() {
        // Every build machine has a shell-ish binary on PATH; on Windows the
        // probe also covers .exe/.cmd/.bat shims via the same helper.
        let probe = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(
            resolve_on_path(probe).is_some(),
            "{probe} must resolve on PATH"
        );
        assert!(
            resolve_on_path("definitely-not-a-real-agentcowork-binary-xyz").is_none(),
            "unknown names must not resolve"
        );
    }

    #[test]
    fn stale_managed_install_is_not_occupancy() {
        let root = std::env::temp_dir().join(format!(
            "agentcowork-stale-agent-{}-{}",
            std::process::id(),
            ACP_COUNTER.load(Ordering::Relaxed)
        ));
        let outcome = agentcowork_acp::InstallOutcome {
            agent_id: "test-agent".into(),
            version: "1.0.0".into(),
            kind: "binary".into(),
            binary_path: Some(root.join("missing.exe")),
            env: vec![],
        };
        assert!(!install_outcome_usable(&outcome));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("agent.exe");
        std::fs::write(&binary, b"test").unwrap();
        let usable = agentcowork_acp::InstallOutcome {
            binary_path: Some(binary),
            ..outcome
        };
        assert!(install_outcome_usable(&usable));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn package_manager_install_requires_manager_readiness() {
        let outcome = agentcowork_acp::InstallOutcome {
            agent_id: "test-agent".into(),
            version: "1.0.0".into(),
            kind: "npx".into(),
            binary_path: None,
            env: vec![],
        };
        assert_eq!(
            install_outcome_usable(&outcome),
            resolve_on_path("npx").is_some()
        );
    }

    #[test]
    fn path_discovery_reports_installed_with_kind_path() {
        // Simulate the discovery leg by pointing PATH at a real binary and
        // resolving one of the registry's Binary-distribution commands.
        let reg = LaunchRegistry::builtin();
        let bin = reg
            .agents
            .iter()
            .find(|a| matches!(a.distribution, Distribution::Binary { ref command, .. } if !command.is_empty()));
        let Some(manifest) = bin else {
            panic!("registry must contain a Binary-distribution agent");
        };
        let Distribution::Binary { command, .. } = &manifest.distribution else {
            unreachable!()
        };
        // The real binary may or may not be on this machine's PATH; the point
        // is the probe never fabricates a path for a name that does not exist.
        let found = resolve_on_path(command);
        if let Some(p) = &found {
            assert!(p.is_file(), "resolved path must exist: {}", p.display());
        }
    }

    /// P60.14 — the desktop shell's live registry leg, end to end.
    ///
    /// `#[ignore]` (network). Run explicitly, single-threaded because it
    /// repoints `AGENTCOWORK_HOME` for the duration:
    ///
    /// ```text
    /// cargo test -p agentcowork-desktop acp_registry_refresh -- --ignored --nocapture --test-threads=1
    /// ```
    ///
    /// It exercises the *shell's own* composition rather than a copy of it:
    /// `acp_registry_refresh()` — the command the Settings “Discover more”
    /// button invokes — fetches the real CDN with the production `ureq`
    /// transport and writes the cache, and then `launch_registry()`, the
    /// resolver behind `acp_agents` / `acp_install_status` / `chief_subagents`
    /// (memoised on the cache file's mtime), must serve the merged catalog
    /// without a restart. Occupancy is untouched by this: a merged row is a
    /// catalog fact, and `agent_installed` still decides what is usable.
    #[test]
    #[ignore = "network: fetches the live ACP registry CDN"]
    fn live_acp_registry_refresh_drives_the_shell_launch_registry() {
        let home =
            std::env::temp_dir().join(format!("agentcowork-shell-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let previous_home = std::env::var("AGENTCOWORK_HOME").ok();
        std::env::set_var("AGENTCOWORK_HOME", &home);

        // No cache yet ⇒ the resolver degrades to the curated seed.
        let seed = LaunchRegistry::builtin();
        assert_eq!(
            launch_registry().agents.len(),
            seed.agents.len(),
            "an empty cache must resolve to the curated seed alone"
        );
        assert!(
            registry_client().load_cached().is_none(),
            "a fresh data dir must not report a cached catalog"
        );

        // The command the Settings button calls: live fetch + cache write.
        let snap = acp_registry_refresh().expect("live registry refresh");
        let count = snap["agentCount"].as_u64().unwrap_or(0);
        assert!(count > 0, "the live registry reported no agents: {snap}");
        assert_eq!(snap["fromCache"], serde_json::json!(false));
        eprintln!("shell registry refresh: {snap}");
        assert_eq!(
            registry_client()
                .load_cached()
                .map(|s| s.index.agents.len()),
            Some(count as usize),
            "the cache must hold exactly what the refresh fetched"
        );

        // The memoised resolver (cache mtime key) now serves the merged catalog.
        let merged = launch_registry();
        let added: Vec<String> = merged
            .agents
            .iter()
            .map(|m| m.id.clone())
            .filter(|id| seed.get(id).is_none())
            .collect();
        eprintln!(
            "merged rows: {} (seed {}), added {added:?}",
            merged.agents.len(),
            seed.agents.len()
        );
        assert!(
            merged.get("everyaios").is_none(),
            "no built-in agent identity exists (ADR-0005)"
        );
        assert!(
            !added.is_empty(),
            "a live refresh must add at least one catalog row the seed lacks"
        );

        // A second read hits the memo (unchanged file stamp) with the same rows.
        assert_eq!(launch_registry().agents.len(), merged.agents.len());

        match previous_home {
            Some(prev) => std::env::set_var("AGENTCOWORK_HOME", prev),
            None => std::env::remove_var("AGENTCOWORK_HOME"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn test_user_path_import_and_verification() {
        let tmp =
            std::env::temp_dir().join(format!("agentcowork-import-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let dummy_bin = tmp.join("dummy_agent");
        std::fs::write(&dummy_bin, b"#!/bin/sh\necho 'dummy-agent v1.2.3'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&dummy_bin).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&dummy_bin, perms).unwrap();
        }

        let home = std::env::temp_dir().join(format!("agentcowork-home-test-{}", std::process::id()));
        let previous_home = std::env::var("AGENTCOWORK_HOME").ok();
        std::env::set_var("AGENTCOWORK_HOME", &home);

        let inst = installer();
        inst.record_path("opencode", &dummy_bin).unwrap();
        let outcome = inst.installed("opencode").expect("installed outcome");
        assert_eq!(outcome.kind, "path");
        assert_eq!(outcome.binary_path, Some(dummy_bin.clone()));

        let verify = acp_agent_verify("opencode".to_string()).unwrap();
        assert_eq!(verify["status"], "ready");
        assert_eq!(verify["executable"], dummy_bin.to_string_lossy().as_ref());

        match previous_home {
            Some(prev) => std::env::set_var("AGENTCOWORK_HOME", prev),
            None => std::env::remove_var("AGENTCOWORK_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn test_wsl_spawn_resolution() {
        // On non-windows platforms, resolve_wsl_spawn degrades to None.
        // On windows platforms with WSL distros installed, it returns wsl.exe with args.
        let wsl_res = resolve_wsl_spawn("nonexistent-command-xyz-123");
        assert!(wsl_res.is_none());
    }
}
