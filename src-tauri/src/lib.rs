//! AgentCowork desktop shell — Tauri v2 backend (tasks P0.2).
//!
//! The shell is deliberately thin: every capability lives in the
//! `agentcowork-*` crates (core, vault, guard, audit, ipc). This crate wires
//! them to the UI as Tauri commands + events, and owns the system tray.

use std::path::PathBuf;
use std::process::{ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod acp_cmds;
mod agent_backend_cmds;
mod agent_cmds;
mod artifact_cmds;
mod boot;
mod browser_cmds;
mod calendar_cmds;
mod catalog_cmds;
mod channel_b;
mod cockpit_cmds;
mod codeintel_cmds;
mod commands;
mod control;
mod desktop_cmds;
mod diagnostics_cmds;
mod discovery_cmds;
mod doctor_cmds;
mod feedback_cmds;
mod fs_cmds;
mod git_cmds;
mod guard_cmds;
mod guard_window;
mod local_cmds;
mod lsp_cmds;
mod maintenance_cmds;
mod mcp_cmds;
mod memory_cmds;
mod model_cmds;
mod oauth_cmds;
mod office_cmds;
mod openai_cmds;
mod replay_cmds;
mod runtime_cmds;
// P55.8 — the SearXNG endpoint config + searx.space instance feed surface.
mod scheduler_cmds;
mod scheduler_fire;
mod search_cmds;
// P65 — Settings Control Center read-models + the one mutation funnel
// (ARCH/17 §17.12). Declared here like every other command family; the
// registration itself lives in `commands.rs` and is checked by
// `tests/registration_sync.rs`.
mod settings_cmds;

mod skills_cmds;
mod state;
mod storage_cmds;
mod sync_cmds;
mod tasks_cmds;
mod terminal_cmds;
mod trajectory_cmds;
mod updater_cmds;
mod vault_cmds;
mod voice_cmds;
mod work_cmds;

pub use state::AppState;

use agentcowork_core::GuardService;
use agentcowork_guard::prescan::guard as compiled_guard;
use agentcowork_vault::Vault;

pub mod xlsx_cmds;
use tauri::{AppHandle, Emitter, Manager, State};

// P71.2c — the stream-id counter that `chat_stream`/`plan_execute` minted is
// gone with them; the ACP channel owns its own turn identity.

/// Event name the UI listens to for chat stream updates.
pub const CHAT_EVENT: &str = "chat-event";

/// FIX-02 / `TASK-TRUST-001` — the process-wide control-plane limiter.
///
/// One instance guards the whole `nativeCall`/Tauri IPC surface
/// (`agentcowork-guard::ratelimit`). It is a `OnceLock` singleton rather than
/// managed state because the gate has to be callable *before* a command's state
/// is resolved, and because a limiter that could be swapped out from under the
/// gate would be a limiter that can be turned off.
fn control_plane_limiter() -> &'static agentcowork_guard::RateLimiter {
    static LIMITER: std::sync::OnceLock<agentcowork_guard::RateLimiter> =
        std::sync::OnceLock::new();
    LIMITER.get_or_init(agentcowork_guard::RateLimiter::with_defaults)
}

/// Wrap the IPC handler with the control-plane admission gate.
///
/// Trust infrastructure fails closed (`ARCH/12-TRUST.md` §11,
/// `REQ-TRUST-009`): a throttled caller is **refused with a typed error**,
/// never queued, never silently allowed, and never served by a second path.
/// The refusal is the canonical taxonomy's `Unavailable` code (retryable, with
/// `retry_after_ms`) — `ARCH/10-KERNEL.md` §3 makes the taxonomy canonical and
/// requires a `DEC` to extend it, so no `RateLimited` code is invented here.
///
/// The gate is deliberately *thin*: it reads the invoke's command name and
/// answers with an error reply, so a throttled call never reaches a command
/// body, a state lock, or the disk. `inner` is the single `generate_handler!`
/// closure from [`commands`]; wrapping (rather than replacing) it keeps
/// `tests/registration_sync.rs` authoritative for the command list.
fn control_plane_gate<F>(inner: F) -> impl Fn(tauri::ipc::Invoke) -> bool
where
    F: Fn(tauri::ipc::Invoke) -> bool,
{
    move |invoke: tauri::ipc::Invoke| {
        let command = invoke.message.command();
        let caller = invoke.message.headers().get("x-agentcowork-caller");
        // The header is an attribution hint for the audit row only. It is never
        // trusted for authorization (a caller can set it), it is bounded, and a
        // missing one falls back to the single renderer identity — so the
        // per-caller tier degrades to the per-command tier instead of
        // inventing a bucket per spoofed name.
        let caller = caller
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && s.len() <= 64)
            .unwrap_or("renderer");
        match control_plane_limiter().check(caller, command) {
            Ok(()) => inner(invoke),
            Err(denied) => {
                // A typed, structured refusal: the canonical `Unavailable` code,
                // the stable reason token, and the backoff the caller needs.
                // No command body, no state lock and no disk are reached.
                let payload = serde_json::json!({
                    "code": denied.code(),
                    "reason": denied.reason,
                    "scope": denied.scope.as_str(),
                    "retryable": denied.retryable(),
                    "retryAfterMs": denied.retry_after_ms,
                    "message": denied.to_string(),
                });
                invoke
                    .resolver
                    .respond::<()>(Err(tauri::ipc::InvokeError(payload)));
                // `true` = the invoke is handled, so Tauri does not fall through
                // to a "command not found" that would hide the real reason.
                true
            }
        }
    }
}

/// P52.x (guard-UX wave) — push-style Guard-2 lifecycle event
/// (`{ kind: minted|approved|rejected|expired, ticketId, batch }`).
/// Emitted from the shared `GuardService` lifecycle hook so both the Guard
/// panel and the guard window re-render instantly; polling remains only as
/// a fallback for missed emits.
pub const GUARD_EVENT: &str = "guard-event";

/// P11.5.11 — AG-UI live transport event (raw encoded envelope line in
/// `{ "line": … }`). Emitted for every `agui/event` notification the
/// coordinator pushes.
pub const AGUI_EVENT: &str = "agui-event";

/// P50.4.2 — local-model download/serve progress events
/// (`{ kind, id, repo, filename, phase, doneBytes, totalBytes, … }`).
pub const MODEL_DOWNLOAD_EVENT: &str = "model-download";

/// P11.5.11 — send one AG-UI event from the UI into the coordinator (e.g.
/// `interrupt_resolved` answering an outstanding AG-UI interrupt). The line
/// is forwarded as an `agui/event` notification over the sidecar link.
#[tauri::command]
fn agui_send(state: State<'_, AppState>, line: String) -> Result<(), String> {
    ensure_sidecar(&state);
    let relay = state.chat_relay.lock().map_err(|e| e.to_string())?;
    let relay = relay
        .as_ref()
        .ok_or_else(|| "sidecar not connected — coordinator link not established".to_string())?;
    relay
        .send_agui(&line)
        .map_err(|e| format!("agui_send failed: {e}"))
}

/// P11.5.11 — ack that the UI is listening for `agui-event`. The sink is
/// attached at boot (`connect_chat_relay`); this returns once the relay is
/// live so the UI knows the AG-UI transport is ready end-to-end.
#[tauri::command]
fn agui_listen(state: State<'_, AppState>) -> Result<(), String> {
    let relay = state.chat_relay.lock().map_err(|e| e.to_string())?;
    match relay.as_ref() {
        Some(r) if r.agui().is_attached() => Ok(()),
        Some(_) => Err("agui sink not attached".to_string()),
        None => Err("sidecar not connected".to_string()),
    }
}

/// P45.8 — if the coordinator exited because nobody was chatting, ask the
/// supervisor to spawn it again and wait until that spawn clears the park
/// flag. A turn that arrives while the process is still up does nothing.
pub(crate) fn ensure_sidecar(state: &AppState) {
    if !state.sidecar_parked.load(Ordering::Acquire) {
        return;
    }
    state.sidecar_resume.store(true, Ordering::Release);
    let start = std::time::Instant::now();
    while state.sidecar_parked.load(Ordering::Acquire)
        && start.elapsed() < std::time::Duration::from_millis(2_000)
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Wire a live coordinator link into the chat relay and store it in state.
/// The relay's consumer loop forwards every `chat/*` notification from the
/// sidecar to the UI as a `chat-event` (P1.4: token deltas → core → Tauri
/// events → UI). Called once per (re)spawn from the link-drain thread.
fn connect_chat_relay(
    app: &AppHandle,
    stdin: ChildStdin,
    stdout: ChildStdout,
    activity: Arc<AtomicU64>,
) {
    let handle = app.clone();
    let state = app.state::<AppState>();
    *state
        .sidecar_activity_ms
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&activity));
    // The SidecarLink reader re-arms the supervisor's idle-watchdog clock on
    // every decoded frame (session/ready + session/heartbeat).
    let link = agentcowork_core::SidecarLink::new_with_activity(stdin, stdout, Some(activity));
    // J21: the relay shares the app's Guard-2 service, and loads the user's
    // `permissions.toml` escalation policy at boot.
    let relay = agentcowork_core::ChatRelay::new_with_guard(
        link,
        Arc::clone(&state.vault),
        Arc::clone(&state.guard_service),
        move |ev| {
            // Fire-and-forget: never let a UI emit failure break the relay.
            let _ = handle.emit(CHAT_EVENT, ev);
        },
    );
    // P71.3f — mount the one readiness source: install/discovery facts plus the
    // live ACP handshake state. The picker façade, the delegation gate and the
    // turn gate all read this; mounted before the relay is published so no
    // turn can race boot and see an unprobed (fail-closed) state.
    relay.mount_readiness(Arc::new(acp_cmds::ShellAgentReadiness {
        sessions: Arc::clone(&state.acp_sessions),
    }));
    // P11.5.11 — AG-UI live transport: `agui/event` notifications from the
    // coordinator reach the UI as `agui-event` emits (raw encoded line).
    {
        let h = app.clone();
        relay.with_agui(move |line| {
            let _ = h.emit(AGUI_EVENT, serde_json::json!({ "line": line }));
        });
    }
    // P68.9 — the agent's shell is the *one* PTY plane: `script.run` runs on the
    // automation profile with agent provenance, so it is audited as
    // `terminal.agent_run` and shows up in the Shell view as a labelled
    // read-only tab. Attached here, before the relay is published to state, so
    // no tool call can race ahead of the executor.
    relay.attach_terminal(Arc::new(terminal_cmds::TerminalPlaneExecutor::new(
        Arc::clone(&state.terminal),
        app.clone(),
    )));
    // P54.5 — the read-only half of the same seam, over the *same* host object,
    // so `terminal/status` and the Shell view can never describe different
    // shells. Attaching it here (before the relay is published) means no turn
    // can observe "no plane" because it raced boot.
    // Coerce the value (not the `Arc::clone` argument) so this is an unsize
    // coercion of the shared handle rather than a second host.
    let plane_host: Arc<agentcowork_core::terminal::PtyHost> = Arc::clone(&state.terminal);
    let terminal_plane: Arc<dyn agentcowork_core::terminal::TerminalPlaneObserver> = plane_host;
    relay.attach_terminal_plane(terminal_plane);
    // P48.3 — the inbuilt agent's computer-use path (E9). Attached here, before
    // the relay is published, so no agent turn can race ahead of the executor.
    // Best-effort: a headless / no-display host has no platform backend, so
    // nothing is attached and `desktop.*` keeps fail-closing honestly with
    // `desktop session not attached` instead of pretending to drive the GUI.
    match desktop_cmds::publish_desktop_backend(&state, app) {
        Ok(()) => eprintln!("agentcowork-desktop: agent desktop backend attached"),
        Err(e) => eprintln!("agentcowork-desktop: agent desktop backend unavailable ({e})"),
    }
    let policy_path = agentcowork_core::default_data_dir().join("permissions.toml");
    relay.with_policy(&policy_path);
    eprintln!("agentcowork-desktop: relay stage policy ok");
    // P71.2c — the relay no longer carries a provider dial plan (profiles,
    // resolved endpoints, keyless local endpoints): the broker that consumed it
    // is deleted with the built-in engine (ADR-0005 §2), and an external agent
    // resolves its own transport. What remains below is the **observation**
    // half — the capability probe sweep whose results every registry replays
    // (A11, `ARCH/ROUTING.md` §6).
    //
    // P44.4 — observe the connected set once, off this thread: a probe is
    // network I/O (bounded per provider) and must never delay the relay coming
    // up. Unlocking the vault sweeps again, because that is when the keyed
    // providers join the connected set.
    catalog_cmds::spawn_boot_observation_sweep(app.clone());
    eprintln!("agentcowork-desktop: relay stage observation sweep ok");
    // P43 (B7 v3.53) — push completion: every terminal transition of the
    // task ledger wakes the UI via a `task-update` event (never polling).
    {
        let h = app.clone();
        relay
            .tasks()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .watch(Box::new(move |record: &agentcowork_core::TaskRecord| {
                let _ = h.emit(
                    "task-update",
                    serde_json::to_value(record).unwrap_or_else(|_| serde_json::json!({})),
                );
            }));
    }
    relay.spawn();
    eprintln!("agentcowork-desktop: relay stage spawn ok");
    *state.chat_relay.lock().expect("chat_relay poisoned") = Some(relay);
    if let Some(relay) = state
        .chat_relay
        .lock()
        .expect("chat_relay poisoned")
        .as_ref()
    {
        crate::channel_b::publish_tools(&state.channel_b_tools, relay.tools());
    }
    // Boot diagnostic: `runtime_status.sidecar` reads this slot, so "coordinator
    // offline" in the UI is exactly "this line never printed".
    eprintln!("agentcowork-desktop: chat relay live — coordinator connected");
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    vault: &'static str,
    sidecar: bool,
    persistence: &'static str,
}

#[tauri::command]
fn runtime_status(state: State<'_, AppState>) -> RuntimeStatus {
    let vault = if state.vault_unlocked.load(Ordering::Acquire) {
        "ready"
    } else {
        match agentcowork_core::gate_mode(&agentcowork_core::default_data_dir()) {
            "setup" | "wrap" => "setup",
            "unlock" => "locked",
            _ => "unknown",
        }
    };
    let persistence = state
        .boot_report
        .lock()
        .ok()
        .map(|r| {
            if r.contains("EPHEMERAL VAULT") {
                "ephemeral"
            } else {
                "durable"
            }
        })
        .unwrap_or("unknown");
    let sidecar = state
        .chat_relay
        .lock()
        .map(|relay| relay.is_some())
        .unwrap_or(false)
        && state
            .sidecar_activity_ms
            .lock()
            .ok()
            .and_then(|clock| clock.as_ref().cloned())
            .map(|clock| {
                let last = clock.load(Ordering::Relaxed);
                last > 0 && now_ms().saturating_sub(last) <= 30_000
            })
            .unwrap_or(false);
    RuntimeStatus {
        vault,
        sidecar,
        persistence,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[tauri::command]
fn version() -> String {
    agentcowork_core::version::banner()
}

#[tauri::command]
fn core_boot_report(state: State<'_, AppState>) -> Result<String, String> {
    state
        .boot_report
        .lock()
        .map(|r| r.clone())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn scan_text(state: State<'_, AppState>, text: String) -> Result<bool, String> {
    // Guard-1: deterministic pre-exec scan. `true` = blocked.
    Ok(state.guard.is_blocked(&text))
}

/// P71.2c — usage is an **observation** ledger (ADR-0005 §2,
/// `ARCH/ROUTING.md` §5): the durable `token_usage` rows plus the in-process
/// memory ledger. It no longer reflects an AgentCowork-owned call, because
/// AgentCowork makes none; the surfaces read what was observed and say so when
/// nothing was (`I15`).
#[tauri::command]
fn usage_snapshot(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    // P5.9: the token/cost dashboard data source — per-key/per-session usage,
    // cache-hit rate, and cost from the relay's in-process `MemoryService`
    // ledger. Empty (zeros) until the sidecar connects and records calls.
    let relay = state.chat_relay.lock().map_err(|e| e.to_string())?;
    let relay = relay
        .as_ref()
        .ok_or_else(|| "sidecar not connected — usage ledger not ready".to_string())?;
    let memory = relay.memory();
    let mem = memory.lock().map_err(|e| e.to_string())?;
    Ok(mem.usage_snapshot())
}

/// P5.9 — per-session cost/token breakdown (the durable `token_usage` ledger,
/// grouped by session). Feeds the analytics cost table.
#[tauri::command]
fn session_totals(
    state: State<'_, AppState>,
) -> Result<Vec<agentcowork_vault::SessionTotal>, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault.session_totals().map_err(|e| e.to_string())
}

#[tauri::command]
fn probe_vault() -> Result<String, String> {
    // Security (P0.2 stub): the path is NOT webview-controlled — it is pinned
    // to the data dir. Arbitrary path handling arrives with P1.1 key
    // management (vault path comes from config, never from the frontend).
    let path = agentcowork_core::default_data_dir().join("vault.db");
    let resolved = agentcowork_core::resolve_vault_key(&agentcowork_core::default_data_dir())
        .map_err(|e| e.to_string())?;
    let vault = agentcowork_vault::Vault::open(&path, &resolved.key).map_err(|e| e.to_string())?;
    Ok(vault.status())
}

#[tauri::command]
fn vault_key_status() -> Result<serde_json::Value, String> {
    let dir = agentcowork_core::default_data_dir();
    let mode = agentcowork_core::gate_mode(&dir);
    let gate = agentcowork_core::needs_passphrase_gate(&dir);
    match agentcowork_core::resolve_vault_key(&dir) {
        Ok(r) => Ok(serde_json::json!({
            "ok": true,
            "origin": r.origin,
            "path": r.path,
            "needsSetup": gate,
            "mode": if r.origin == agentcowork_core::VaultKeyOrigin::Generated { "wrap" } else { mode },
            "locked": false,
        })),
        Err(agentcowork_core::VaultKeyError::NeedsSetup) => Ok(serde_json::json!({
            "ok": false,
            "needsSetup": true,
            "mode": mode,
            "locked": mode == "unlock",
        })),
        Err(e) => Err(e.to_string()),
    }
}

fn reopen_disk_vault(state: &AppState, key: &str) -> Result<String, String> {
    let path = agentcowork_core::default_data_dir().join("vault.db");
    let vault = Vault::open(&path, key).map_err(|e| e.to_string())?;
    let status = vault.status();
    *state.vault.lock().map_err(|e| e.to_string())? = vault;
    Ok(status)
}

#[tauri::command]
fn vault_setup(
    state: State<'_, AppState>,
    app: AppHandle,
    passphrase: String,
) -> Result<serde_json::Value, String> {
    let r =
        agentcowork_core::setup_vault_passphrase(&agentcowork_core::default_data_dir(), &passphrase)
            .map_err(|e| e.to_string())?;
    let status = reopen_disk_vault(&state, &r.key)?;
    state.vault_unlocked.store(true, Ordering::Release);
    // P44.4 — a fresh vault is a fresh connected set: observe it now rather
    // than waiting for the next boot.
    catalog_cmds::spawn_observation_sweep(app);
    Ok(serde_json::json!({
        "ok": true,
        "origin": r.origin,
        "path": r.path,
        "needsSetup": false,
        "status": status,
    }))
}

#[tauri::command]
fn vault_unlock(
    state: State<'_, AppState>,
    app: AppHandle,
    passphrase: String,
) -> Result<serde_json::Value, String> {
    let r =
        agentcowork_core::unlock_vault_passphrase(&agentcowork_core::default_data_dir(), &passphrase)
            .map_err(|e| e.to_string())?;
    let status = reopen_disk_vault(&state, &r.key)?;
    state.vault_unlocked.store(true, Ordering::Release);
    // P44.4 — unlocking is what makes the keyed providers part of the connected
    // set, so this is the pass that can observe them. Off-thread and bounded;
    // the unlock response never waits on a network call.
    catalog_cmds::spawn_observation_sweep(app);
    Ok(serde_json::json!({
        "ok": true,
        "origin": r.origin,
        "needsSetup": false,
        "status": status,
    }))
}

#[tauri::command]
fn session_list(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let rows = vault.list_ui_sessions().map_err(|e| e.to_string())?;
    let sessions: Vec<serde_json::Value> = rows
        .into_iter()
        .filter_map(|(_, payload)| serde_json::from_str(&payload).ok())
        .collect();
    Ok(serde_json::json!({ "sessions": sessions }))
}

#[tauri::command]
fn session_put(state: State<'_, AppState>, session: serde_json::Value) -> Result<(), String> {
    let id = session
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("session id required")?
        .to_string();
    let payload = serde_json::to_string(&session).map_err(|e| e.to_string())?;
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault
        .put_ui_session(&id, &payload)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn session_delete(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault
        .delete_ui_session(&session_id)
        .map_err(|e| e.to_string())
}

/// Locate the coordinator sidecar binary. `AGENTCOWORK_COORDINATOR_BIN` wins;
/// otherwise the packaged resource dir (`bin/coordinator` — P8.8 installers
/// ship the sidecar as a bundle resource) is probed, then the standard
/// workspace build output paths.
fn locate_coordinator_bin(app: &AppHandle) -> Option<PathBuf> {
    if let Some(p) = agentcowork_types::env_compat::get("COORDINATOR_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    // Packaged app: the sidecar is a bundle resource (`bin/coordinator`).
    if let Ok(res) = app.path().resource_dir() {
        for rel in [
            "bin/coordinator",
            "bin/coordinator.exe",
            "coordinator",
            "coordinator.exe",
        ] {
            let p = res.join(rel);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let cwd = std::env::current_dir().ok()?;
    for rel in [
        "packages/coordinator/dist/coordinator",
        "../packages/coordinator/dist/coordinator",
        "packages/coordinator/dist/coordinator.exe",
        "../packages/coordinator/dist/coordinator.exe",
    ] {
        let p = cwd.join(rel);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// P70.A2 — why is the sidecar not connected? Distinguishes the two very
/// different cases the UI must not conflate:
///
/// - `connecting` — the binary exists; the supervisor is still warming up or
///   restarting (transient; the normal boot path).
/// - `missing` — the **bundled binary itself is absent**. In a packaged app
///   this is unrecoverable (a broken install): the error names the remedy
///   (reinstall) instead of leaving the user with a generic "offline" chip.
///   In a dev checkout it points at building the sidecar first.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SidecarProbe {
    connected: bool,
    /// `connected` · `connecting` · `missing`
    state: &'static str,
    detail: String,
}

#[tauri::command]
fn sidecar_probe(app: AppHandle, state: State<'_, AppState>) -> SidecarProbe {
    let relay_live = state
        .chat_relay
        .lock()
        .map(|relay| relay.is_some())
        .unwrap_or(false)
        && state
            .sidecar_activity_ms
            .lock()
            .ok()
            .and_then(|clock| clock.as_ref().cloned())
            .map(|clock| {
                let last = clock.load(Ordering::Relaxed);
                last > 0 && now_ms().saturating_sub(last) <= 30_000
            })
            .unwrap_or(false);
    if relay_live {
        return SidecarProbe {
            connected: true,
            state: "connected",
            detail: "coordinator link is live".into(),
        };
    }
    match locate_coordinator_bin(&app) {
        Some(bin) => SidecarProbe {
            connected: false,
            state: "connecting",
            detail: format!(
                "sidecar binary found at {} — waiting for the supervisor to connect",
                bin.display()
            ),
        },
        None => {
            let packaged = app.path().resource_dir().is_ok();
            SidecarProbe {
                connected: false,
                state: "missing",
                detail: if packaged {
                    "The coordinator sidecar was not found in this installation — the install is broken or incomplete. Reinstall AgentCowork to restore live agent work.".into()
                } else {
                    "The coordinator sidecar is not built. Run `pnpm --filter @agentcowork/coordinator build` (or set AGENTCOWORK_COORDINATOR_BIN) and restart.".into()
                },
            }
        }
    }
}

/// J16: pre-spawn the coordinator sidecar at boot (hidden — the app window
/// and the sidecar warm up in parallel, so the first chat request never waits
/// on a process spawn: ~200ms perceived cold start). Two threads: one runs the
/// process lifecycle (spawn/watchdog/restart), the other drains the link
/// handoffs and (re)builds the `ChatRelay` on every (re)spawn. Non-fatal: the
/// app runs fine without the sidecar; chat simply reports "sidecar not
/// connected".
fn pre_spawn_coordinator(app: AppHandle) {
    let Some(bin) = locate_coordinator_bin(&app) else {
        eprintln!("agentcowork-desktop: coordinator binary not found — pre-spawn skipped");
        return;
    };
    let (mut supervisor, link_rx) = agentcowork_core::start_supervisor_with_link(bin);
    let activity = Arc::clone(&supervisor.last_activity_ms);
    // Share the park/resume flags with commands so the next turn can wake
    // a sidecar that exited because the user went idle.
    if let Some(state) = app.try_state::<AppState>() {
        supervisor.resume = Arc::clone(&state.sidecar_resume);
        supervisor.parked = Arc::clone(&state.sidecar_parked);
    }
    // Lifecycle thread: spawn, watchdog, restart. Blocks until circuit open.
    std::thread::spawn(move || {
        if let Err(e) = supervisor.wait_or_restart() {
            eprintln!("agentcowork-desktop: supervisor ended: {e}");
        }
    });
    // Link thread: rebuild the chat relay on every (re)spawn handoff.
    std::thread::spawn(move || {
        while let Ok((stdin, stdout)) = link_rx.recv() {
            eprintln!("agentcowork-desktop: sidecar link acquired — building chat relay");
            connect_chat_relay(&app, stdin, stdout, Arc::clone(&activity));
        }
    });
}

/// J16: bind the UNIX-domain socket control channel and dispatch
/// `agent/stop` / `agent/undo` / `agent/interrupt-response`.
#[cfg(unix)]
fn serve_unix_control_channel(app: AppHandle) {
    let cfg = agentcowork_core::Config::load().unwrap_or_default();
    let sock = cfg.resolved_socket_path();
    let server = match agentcowork_ipc::UnixFrameServer::bind(&sock) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("agentcowork-desktop: unix socket bind failed (continuing): {e}");
            return;
        }
    };
    std::thread::spawn(move || loop {
        match server.accept() {
            Ok(stream) => {
                let app = app.clone();
                let _ = server.serve_connection(stream, move |payload| {
                    let parsed: serde_json::Value =
                        serde_json::from_slice(&payload).unwrap_or_default();
                    let method = parsed.get("method").and_then(|m| m.as_str()).unwrap_or("");
                    let params = parsed
                        .get("params")
                        .cloned()
                        .unwrap_or(serde_json::json!({}));
                    let result = control::dispatch(&app, method, &params);
                    let reply = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": parsed.get("id"),
                        "result": result,
                    });
                    Some(serde_json::to_vec(&reply).unwrap_or_default())
                });
            }
            Err(e) => {
                eprintln!("agentcowork-desktop: unix socket accept: {e}");
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Build the initial state exactly like the headless binary would.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut boot_report =
        agentcowork_core::boot(&args).unwrap_or_else(|e| format!("boot failed: {e}"));
    let guard = compiled_guard().clone();
    let data_dir = agentcowork_core::default_data_dir();
    let resolved = agentcowork_core::resolve_vault_key(&data_dir);
    // Bugfix 14 — persistence must never fail *silently* open. If the vault
    // cannot be opened on disk we fall back to an in-memory vault (chat keeps
    // working) but flag it loudly so the UI can warn that nothing will persist,
    // instead of letting the user believe their chat is durable.
    let mut vault_ephemeral = false;
    let mut vault_unlocked = false;
    let vault = match resolved {
        Ok(r) => match Vault::open(&data_dir.join("vault.db"), &r.key) {
            Ok(vault) => {
                vault_unlocked = true;
                vault
            }
            Err(e) => {
                // Keep a disposable vault only as a type-safe boot container;
                // it is locked from the UI and cannot be treated as durable.
                eprintln!("agentcowork-desktop: vault open failed (persistence unavailable): {e}");
                vault_ephemeral = true;
                Vault::open_in_memory(&r.key).unwrap_or_else(|_| {
                    Vault::open_in_memory(&agentcowork_core::default_vault_key())
                        .expect("in-memory vault")
                })
            }
        },
        Err(e) => {
            eprintln!("agentcowork-desktop: vault key resolve failed (persistence unavailable): {e}");
            vault_ephemeral = true;
            Vault::open_in_memory(&agentcowork_core::default_vault_key()).expect("in-memory vault")
        }
    };
    if vault_ephemeral {
        boot_report.push_str(" | EPHEMERAL VAULT: persistence disabled (fix vault lock)");
    }
    let vault = Arc::new(Mutex::new(vault));
    let audit_log = agentcowork_audit::AuditWriter::open(&data_dir.join("audit.ndjson")).ok();

    tauri::Builder::default()
        .manage(AppState {
            boot_report: Mutex::new(boot_report),
            guard,
            vault: Arc::clone(&vault),
            vault_unlocked: std::sync::atomic::AtomicBool::new(vault_unlocked),
            sidecar_activity_ms: Mutex::new(None),
            sidecar_resume: Arc::new(AtomicBool::new(false)),
            sidecar_parked: Arc::new(AtomicBool::new(false)),
            chat_relay: Mutex::new(None),
            replay_dir: agentcowork_core::default_data_dir(),
            cockpit: Arc::new(Mutex::new(Default::default())),
            guard_service: Arc::new(Mutex::new(GuardService::new())),
            acp_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            audit: Mutex::new(agentcowork_audit::merkle::MerkleChain::new()),
            audit_log: Mutex::new(audit_log),
            file_undos: Mutex::new(Vec::new()),
            battery: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            browser: Mutex::new(None),

            mcp_servers: Arc::new(Mutex::new(mcp_cmds::load_attached_servers())),
            mcp_live: Arc::new(Mutex::new(std::collections::HashMap::new())),
            mcp_remote_flows: Arc::new(Mutex::new(std::collections::HashMap::new())),
            mcp_remote_tokens: Arc::new(Mutex::new(std::collections::HashMap::new())),
            mcp_pending_calls: Mutex::new(std::collections::HashMap::new()),
            model_downloads: Mutex::new(std::collections::HashMap::new()),
            model_serves: Mutex::new(Default::default()),
            runtime_observations: Mutex::new(std::collections::HashMap::new()),
            desktop: Mutex::new(desktop_cmds::DesktopSlot::default()),
            artifacts: Mutex::new(std::collections::HashMap::new()),
            openai_server: Mutex::new(Default::default()),
            // H36 (P54) — the PTY host owns live terminal sessions; they
            // persist independently of any Shell-view mount.
            terminal: {
                let host = agentcowork_core::terminal::PtyHost::new();
                // P68.8 — the replay ring's capacity comes from
                // `terminal.scrollbackBytes` (clamped inside the setter). Read
                // once, here: sessions snapshot the capacity at spawn, so a
                // later config change never resizes a live scrollback under
                // the user's cursor.
                if let Ok(cfg) = agentcowork_core::Config::load() {
                    host.set_scrollback_bytes(cfg.terminal.scrollback_bytes());
                }
                std::sync::Arc::new(host)
            },
            // P56.1 — the live models.dev catalog (snapshot + cadence).
            catalog: std::sync::Arc::new(catalog_cmds::CatalogState::new(
                agentcowork_core::default_data_dir().join("catalog"),
            )),
            pending_update: Mutex::new(None),
            channel_b: Mutex::new(None),
            channel_b_tools: crate::channel_b::SharedTools::default(),
        })
        // P70.C3 — the slot holding a downloaded-but-not-installed update
        // artifact between the background download and the explicit restart.
        .manage(updater_cmds::PendingUpdateSlot(Default::default()))
        .invoke_handler(control_plane_gate(commands::handler()))
        // P8.8: auto-updater (checks + downloads against the configured
        // endpoints; signing key is the release secret).
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Office/folder views: native file-open dialog (Browse buttons).
        // Capability-gated (`dialog:allow-open`); the UI falls back to typed
        // paths where the shell is absent (plain-browser preview).
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // P52.x — bridge the shared GuardService lifecycle hook to
            // `guard-event` emits (same fire-and-forget pattern as
            // CHAT_EVENT). Narrow lock: subscribe once, then move only the
            // receiver + handle into the forwarder thread.
            {
                let rx = app
                    .state::<AppState>()
                    .guard_service
                    .lock()
                    .map(|mut svc| svc.subscribe_lifecycle())
                    .unwrap_or_else(|e| e.into_inner().subscribe_lifecycle());
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    for ev in rx {
                        let payload = match &ev {
                            agentcowork_core::GuardLifecycle::Minted { ticket_id, batch } => {
                                serde_json::json!({ "kind": "minted", "ticketId": ticket_id, "batch": batch })
                            }
                            agentcowork_core::GuardLifecycle::Approved { ticket_id, batch } => {
                                serde_json::json!({ "kind": "approved", "ticketId": ticket_id, "batch": batch })
                            }
                            agentcowork_core::GuardLifecycle::Rejected { ticket_id, batch } => {
                                serde_json::json!({ "kind": "rejected", "ticketId": ticket_id, "batch": batch })
                            }
                            agentcowork_core::GuardLifecycle::Expired { ticket_id, batch } => {
                                serde_json::json!({ "kind": "expired", "ticketId": ticket_id, "batch": batch })
                            }
                        };
                        let _ = handle.emit(GUARD_EVENT, payload);
                    }
                });
            }
            // Tray must be non-fatal: on systems without appindicator/tray
            // support the app should still start (just without a tray icon).
            if let Err(e) = boot::setup_tray(app.handle()) {
                eprintln!("agentcowork-desktop: tray setup failed (continuing): {e}");
            }
            // P71.2c — the trigger plane's firing loop. The host owns a
            // firing (Work + Run + the bound agent's ACP turn); the sidecar
            // no longer executes one (ARCH/AUTOMATION.md §9).
            scheduler_fire::spawn_loop(app.handle());
            // J16: pre-spawn the coordinator + bind the unix control socket.
            pre_spawn_coordinator(app.handle().clone());
            #[cfg(unix)]
            serve_unix_control_channel(app.handle().clone());
            // P2.11 (E16): serve the WebMCP tool catalog over loopback HTTP.
            boot::spawn_webmcp_server();
            // Ledger-growth fault line: enforce ARCH/06's configurable audit
            // retention — compact the NDJSON log at most once per day
            // (writer-quiescent window; marker-gated; non-fatal).
            if let Err(e) = maintenance_cmds::run_audit_sweep_if_due(&app.state::<AppState>()) {
                eprintln!("agentcowork-desktop: audit sweep failed (continuing): {e}");
            }
            // P43.4 — task-ledger maintenance at boot: reap grace-expired
            // running tasks + prune terminal records past 7-day retention.
            // (Marker-free: the ledger itself is idempotent — reap/prune
            // only touch records that match their predicates.)
            if let Err(e) = tasks_cmds::tasks_sweep(app.state::<AppState>()) {
                eprintln!("agentcowork-desktop: task sweep failed (continuing): {e}");
            }
            // P56.1 — the live catalog job: fetch models.dev/api.json when the
            // stored snapshot is stale (4h default, 1–24h configurable), then
            // re-check every minute so a Settings change applies without a
            // restart. A failed fetch keeps the last good snapshot and only
            // records the honest verdict (offline = cached).
            {
                let catalog = std::sync::Arc::clone(&app.state::<AppState>().catalog);
                catalog_cmds::spawn_refresh_job(catalog);
            }
            // P60.14 — the ACP registry job: the external-agent catalog is
            // consumed dynamically (Zed/ACP model), so discovery refreshes the
            // official `registry.json` when the local cache is missing or
            // stale, then re-checks hourly. Offline keeps the cached catalog;
            // with no cache the resolver degrades to the curated seed. The
            // merge into `launch_registry()` is memoised on the cache file's
            // mtime, so a refresh lands without a restart.
            acp_cmds::spawn_registry_refresh_job();
            // P70.C4 — auto-update check: one shortly after boot, then every
            // 4h. Emits `updater-status`; never degrades the running app.
            updater_cmds::spawn_periodic_check(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running AgentCowork");
}

#[cfg(test)]
mod tests {
    //! FIX-02 — the control-plane gate's decision contract.
    //!
    //! The gate wraps a real `tauri::ipc::Invoke`, which cannot be constructed
    //! in a unit test, so what is pinned here is the pure part: the caller
    //! attribution rule and the failure mode of the limiter itself. The
    //! limiter's token-bucket behaviour is covered in
    //! `agentcowork_guard::ratelimit`'s own tests.

    use super::control_plane_limiter;
    use agentcowork_guard::{RateLimitConfig, RateLimiter};

    /// A missing / oversized / blank caller header collapses to the single
    /// renderer identity, so a spoofed header cannot mint a fresh bucket per
    /// request (which would defeat the per-caller tier entirely).
    #[test]
    fn caller_attribution_is_bounded_and_never_trusted_for_authorization() {
        // The rule the gate applies, mirrored here so it is pinned.
        let attribute = |raw: Option<&str>| -> &str {
            raw.map(str::trim)
                .filter(|s| !s.is_empty() && s.len() <= 64)
                .unwrap_or("renderer")
        };
        assert_eq!(attribute(None), "renderer");
        assert_eq!(attribute(Some("   ")), "renderer");
        assert_eq!(attribute(Some("")), "renderer");
        assert_eq!(attribute(Some(&"x".repeat(65))), "renderer");
        assert_eq!(attribute(Some("agent-a")), "agent-a");
    }

    /// Fail closed: with the budget spent, the gate's limiter refuses rather
    /// than allowing, and the refusal carries the canonical code.
    #[test]
    fn the_gate_limiter_fails_closed_with_the_canonical_code() {
        let rl = RateLimiter::new(RateLimitConfig {
            global: agentcowork_guard::Limit::new(2, 0.0),
            per_caller_command: agentcowork_guard::Limit::new(2, 0.0),
            ttl_ms: 60_000,
            max_entries: 8,
        });
        assert!(rl.check("renderer", "fs_read_file").is_ok());
        assert!(rl.check("renderer", "fs_read_file").is_ok());
        let err = rl
            .check("renderer", "fs_read_file")
            .expect_err("fail closed");
        assert_eq!(err.code(), "Unavailable");
        assert!(err.retryable());
    }

    /// The process-wide limiter is a singleton, so two command paths share one
    /// budget instead of each getting its own.
    #[test]
    fn the_limiter_is_one_shared_instance() {
        let a = control_plane_limiter() as *const RateLimiter;
        let b = control_plane_limiter() as *const RateLimiter;
        assert_eq!(a, b);
    }
}
