//! P48.3 — E9 desktop computer-use effect-funnel seam.
//!
//! `agentcowork-desktop` is a *library* engine (see/read/act/verify); this
//! module is the host wiring that the crate's own docs require: "the desktop
//! host wires `policy::PermissionGate` to the ticket store and `AuditSink` to
//! the Merkle audit chain, exactly like every other effect in the product."
//!
//! **Both paths are wired**: the human-gesture commands (`desktop_act` /
//! see / read / the escalation pair) and the inbuilt agent's loop tool
//! (`desktop.windows` / `desktop.read` / `desktop.act` in `ToolService`,
//! P48.3), which reaches the same engine through [`DesktopEngineBackend`] — the
//! `agentcowork_core::DesktopBackend` seam `attach_desktop` binds.
//!
//! There is **one** engine, one live policy and one Guard-2 preflight; the two
//! paths differ only in the provenance they declare. The human commands use
//! `Engine::act` (human gesture); the agent backend uses
//! `Engine::act_with(.., ActProvenance::Agent)`, so an agent-initiated action
//! is filed under the agent authority class instead of being recorded as the
//! user's own click. Risky classes still reach this host's `PermissionGate`,
//! which is deny-by-default — so an agent's risky act fails closed until the
//! Guard-2 card surface lands (P57/P59).
//!
//! Gating model (fail-closed, per the spec's dual-guard + honesty invariant):
//! each human `act` is routed through the engine's own Guard-2 preflight. The
//! preflight lets **routine/navigational** acts through (Allow on an
//! allow-listed app); **risky classes** (Delete / Money / Install / Captcha /
//! Transmit) reach the human `PermissionGate` seam, which this host backs
//! with a Deny-by-default gate so nothing risky silently executes. Every
//! decision and every executed act is audited via `control::record_mutation`
//! with `human_gesture` provenance (same Merkle chain as every other effect).

use std::sync::Arc;
use tauri::{Manager, State};

use crate::AppState;

/// The lazy desktop engine held in `AppState`. `None` until a successful
/// platform backend attach; on headless / no-display it honest-fails (matches
/// the "honest-fail → live" browser posture). Also caches an `AppHandle` so
/// the engine's Guard-2 audit sink can reach `record_mutation`.
#[derive(Default)]
pub struct DesktopSlot {
    engine: Option<Arc<agentcowork_computeruse::DesktopEngine>>,
    /// Why the engine is unavailable when `None` (empty until first attempt).
    last_error: Option<String>,
    /// The audit sink bridge (holds the `AppHandle` to feed the Merkle chain).
    sink: Option<Arc<AuditSinkToChain>>,
    /// P57.8 — the installed-app inventory. The disk scan happens once per
    /// session (installed apps do not appear mid-session); Settings filtering
    /// reads this cache, so typing in the picker costs no filesystem work.
    apps: Option<Vec<agentcowork_computeruse::InstalledApp>>,
}

/// P57.8 — the persisted desktop policy (`<data_dir>/desktop.json`).
///
/// One file is the source of truth for the allow-list paths, the legacy name
/// allow-list, strict mode, safe zones and the interaction default, read at
/// attach time and written by every Settings change, so the live Guard-2
/// preflight and the surface can never disagree about what is in use. A
/// malformed file degrades to the local default (no allow-list), never to a
/// partially trusted one.
pub fn policy_path() -> std::path::PathBuf {
    agentcowork_core::default_data_dir().join("desktop.json")
}

pub fn load_policy() -> agentcowork_computeruse::AppPolicy {
    std::fs::read(policy_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_policy(policy: &agentcowork_computeruse::AppPolicy) -> Result<(), String> {
    let path = policy_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create data dir: {e}"))?;
    }
    let json = serde_json::to_vec_pretty(policy).map_err(|e| format!("encode: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(|e| format!("write: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename: {e}"))
}

fn policy_json(policy: &agentcowork_computeruse::AppPolicy) -> serde_json::Value {
    serde_json::json!({
        "allowList": policy.allow_list,
        "allowPaths": policy.allow_paths,
        "strict": policy.strict,
        "interactionDefault": policy.interaction_mode.as_str(),
        "allowsRaisingWindows": policy.allows_raising_windows(),
    })
}

/// P57.8 — write the policy through: persist, then apply to the live engine
/// when one is attached. `appliedLive=false` means the file is what the next
/// attach will read (the response says which, so the UI never implies a live
/// change it did not make).
fn commit_policy(
    state: &State<'_, AppState>,
    policy: &agentcowork_computeruse::AppPolicy,
) -> Result<bool, String> {
    save_policy(policy)?;
    let mut applied = false;
    let slot = state.desktop.lock().map_err(|e| e.to_string())?;
    if let Some(engine) = slot.engine.as_ref() {
        engine.set_policy(policy.clone());
        applied = true;
    }
    Ok(applied)
}

/// Fail-closed human gate: the engine's preflight already lets routine acts
/// through on allow-listed apps; this gate only fires when the policy needs a
/// human confirmation for a risky class (Delete / Money / Install / Captcha /
/// Transmit). Auto-approving those from a Tauri command is never acceptable,
/// so we Deny. (A future Guard-2 card surface may render these; until then
/// they fail closed — never silent.)
struct FailClosedGate;

impl agentcowork_computeruse::policy::PermissionGate for FailClosedGate {
    fn request(
        &self,
        _act: &agentcowork_computeruse::types::ActKind,
        _class: agentcowork_computeruse::policy::ConfirmClass,
    ) -> agentcowork_computeruse::policy::GateDecision {
        agentcowork_computeruse::policy::GateDecision::Deny
    }
}

/// The authority class one desktop act is recorded under.
///
/// Pure, so the mapping is pinned by a test: the whole point of threading
/// provenance through the engine is that an **agent** act never lands on the
/// Merkle chain as a human gesture.
fn auth_kind_for(provenance: agentcowork_computeruse::ActProvenance) -> crate::control::AuthKind {
    match provenance {
        agentcowork_computeruse::ActProvenance::HumanGesture => {
            crate::control::AuthKind::HumanGesture
        }
        agentcowork_computeruse::ActProvenance::Agent => crate::control::AuthKind::AgentTicket,
        agentcowork_computeruse::ActProvenance::Automation => {
            crate::control::AuthKind::AutomationTicket
        }
    }
}

/// Bridges the engine's Guard-2 `AuditSink` to the same Merkle chain every
/// other effect uses (`control::record_mutation`), choosing the authority class
/// from the provenance the engine reports. `record_mutation` itself injects
/// `authorization`, so the value is Rust-derived and never payload-derived. The
/// shared `Arc` inner lets the single engine instance (and a clone passed to the
/// engine) both see the app handle installed at attach time.
#[derive(Clone)]
struct AuditSinkToChain {
    app: std::sync::Arc<std::sync::Mutex<Option<tauri::AppHandle>>>,
}

impl Default for AuditSinkToChain {
    fn default() -> Self {
        Self {
            app: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

impl agentcowork_computeruse::policy::AuditSink for AuditSinkToChain {
    fn write(
        &self,
        kind: &str,
        payload: serde_json::Value,
        provenance: agentcowork_computeruse::ActProvenance,
    ) {
        let app = self.app.lock().ok().and_then(|a| a.clone());
        if let Some(app) = app {
            let state = app.state::<AppState>();
            // The engine tells us who acted; the authority class is derived
            // here in Rust and never read from a payload, so nothing a machine
            // produces can manufacture a human gesture (the anti-impersonation
            // invariant in `control::AuthKind`).
            let authorization = auth_kind_for(provenance);
            crate::control::record_mutation(&state, authorization, kind, payload);
        }
    }
}

/// Get-or-lazily-attach the engine, caching a shared handle to the audit sink
/// so the Guard-2 bridge can reach `record_mutation` on later calls.
fn get_or_attach(
    state: &AppState,
    app: &tauri::AppHandle,
) -> Result<Arc<agentcowork_computeruse::DesktopEngine>, String> {
    {
        let mut slot = state.desktop.lock().map_err(|e| e.to_string())?;
        if let Some(engine) = slot.engine.as_ref() {
            return Ok(Arc::clone(engine));
        }
        let sink = AuditSinkToChain::default();
        let slot_sink = Arc::new(sink.clone());
        // P57.8 — the attach reads the persisted policy, so an allow-list row
        // added in a previous session is in force before the first action.
        match agentcowork_computeruse::DesktopEngine::new(
            load_policy(),
            Box::new(FailClosedGate),
            Box::new(sink),
        ) {
            Ok(engine) => {
                let engine = Arc::new(engine);
                // Install the app handle ONCE (both the engine's copy and our
                // retained copy share the same Arc inner), so the engine's
                // Guard-2 audits reach `record_mutation` on the live chain.
                *slot_sink.app.lock().map_err(|e| e.to_string())? = Some(app.clone());
                slot.engine = Some(Arc::clone(&engine));
                slot.sink = Some(slot_sink);
                slot.last_error = None;
                Ok(engine)
            }
            Err(e) => {
                let msg = format!("desktop engine unavailable: {e}");
                slot.last_error = Some(msg.clone());
                Err(msg)
            }
        }
    }
}

/// The engine's **own** `WindowInfo` for a window id.
///
/// The previous helper fabricated an empty `WindowInfo` (`app: ""`), and that
/// silently broke every non-launch act: `AppPolicy::evaluate` gates on the
/// **subject app**, `""` is never allow-listed, so a routine act on an
/// allow-listed app fell through to `Confirm(Routine)` and the host's
/// fail-closed gate denied it. Human Computer-use clicks were dead on arrival,
/// and an agent act had no chance either. Resolving the id against the engine's
/// live window list restores the real app name (and bounds) the policy has to
/// match.
///
/// If the window no longer exists we fall back to an id-only `WindowInfo`, which
/// then fails the allow-list honestly instead of acting on a stale guess.
fn resolve_window(
    engine: &agentcowork_computeruse::DesktopEngine,
    id: u64,
) -> agentcowork_computeruse::WindowInfo {
    engine
        .list_windows()
        .ok()
        .and_then(|windows| windows.into_iter().find(|w| w.id == id))
        .unwrap_or_else(|| window_by_id_only(id))
}

fn window_by_id_only(id: u64) -> agentcowork_computeruse::WindowInfo {
    agentcowork_computeruse::WindowInfo {
        id,
        title: String::new(),
        app: String::new(),
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        has_a11y_tree: false,
    }
}

/// Serialize the engine's window list into the `DesktopBackend` wire shape the
/// tool registry serves (`{id, title, app}` — see `ToolFamily::Desktop`).
fn windows_json(windows: &[agentcowork_computeruse::WindowInfo]) -> serde_json::Value {
    serde_json::json!(windows
        .iter()
        .map(|w| serde_json::json!({
            "id": w.id, "title": w.title, "app": w.app,
            "x": w.x, "y": w.y, "width": w.width, "height": w.height,
        }))
        .collect::<Vec<_>>())
}

/// Serialize an a11y read into the `DesktopBackend` snapshot shape
/// (`{windowId, tree:[{role, name, indexPath}], hasTree}`).
///
/// `FIX-17`: the snapshot now carries the read's **status**, epoch, guidance and
/// freshness anomalies. The agent-facing point: a window this process is not
/// allowed to read used to reach the model as `hasTree: false`, which reads as
/// "the app has no controls" — and the model's next move is a blind click. An
/// `unknown` status with its guidance is the difference between "nothing there"
/// and "I could not look" (`REQ-CUA-006`).
fn snapshot_json(window_id: u64, read: &agentcowork_computeruse::ReadResult) -> serde_json::Value {
    let tree = read
        .tree
        .as_ref()
        .map(|root| {
            root.flatten()
                .into_iter()
                .map(|n| {
                    serde_json::json!({
                        "indexPath": n.index_path,
                        "role": n.role,
                        "name": n.name,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::json!({
        "windowId": window_id,
        "tree": tree,
        "hasTree": read.tree.is_some(),
        "dpiScale": read.dpi_scale,
        "status": read.status.as_str(),
        "detail": read.status_detail(),
        // `false` means "this read may not conclude anything is absent" — the
        // agent must not treat a missing control as a fact on such a read.
        "mayInferAbsence": read.may_infer_absence(),
        "epoch": read.epoch.0,
        "observedAtMs": read.observed_at_ms,
        "guidance": read.guidance,
        "anomalies": read
            .anomalies
            .iter()
            .map(|a| serde_json::json!({ "kind": a.kind, "scope": a.scope, "detail": a.detail }))
            .collect::<Vec<_>>(),
    })
}

/// Flatten a11y tree → `[index] role: name` lines (the "text read" of desktop).
fn render_tree(root: &agentcowork_computeruse::ReadNode) -> String {
    root.flatten()
        .into_iter()
        .map(|n| {
            let label = if n.name.is_empty() {
                n.role.clone()
            } else {
                format!("{} {}", n.role, n.name)
            };
            format!("[{}] {}", n.index_path, label)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Capability surface + attach state + the derived H4 readiness (the honest
/// "what can this machine do, is it allowed to, and is the engine live" probe
/// the UI rail dot and the Settings chip read).
#[tauri::command]
pub fn desktop_status(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let slot = state.desktop.lock().map_err(|e| e.to_string())?;
    match slot.engine.as_ref() {
        Some(engine) => {
            let c = engine.capabilities();
            let policy = engine.policy();
            let read = agentcowork_computeruse::derive_readiness(
                Some(&c),
                &policy,
                engine.guard().kill.is_stopped(),
                None,
            );
            Ok(serde_json::json!({
                "attached": true,
                "interactionDefault": policy.interaction_mode.as_str(),
                "readiness": { "state": read.state.as_str(), "detail": read.detail, "usable": read.usable },
                "capabilities": {
                    "see": format!("{:?}", c.see),
                    "see_occluded": c.see_occluded,
                    "uia_tree": c.uia_tree,
                    "invoke_set_value": c.invoke_set_value,
                    "send_input": c.send_input,
                    "background_input": c.background_input,
                    "ocr": c.ocr,
                    "window_list": c.window_list,
                    "launch_app": c.launch_app,
                    // `FIX-18` — the capture chip is a real readiness probe, so
                    // the surface can say "graphics capture unavailable —
                    // PrintWindow fallback" before anyone attempts a capture.
                    "capture_readiness": {
                        "state": c.capture_readiness.state,
                        "pipeline": c.capture_readiness.pipeline,
                        "detail": c.capture_readiness.detail,
                    },
                },
            }))
        }
        None => {
            // H4 — an unattached driver is `driver_missing` with the attach
            // error verbatim, never a green dot over a dead engine.
            let policy = load_policy();
            let read = agentcowork_computeruse::derive_readiness(
                None,
                &policy,
                false,
                slot.last_error.as_deref(),
            );
            Ok(serde_json::json!({
                "attached": false,
                "reason": slot.last_error,
                "interactionDefault": policy.interaction_mode.as_str(),
                "readiness": { "state": read.state.as_str(), "detail": read.detail, "usable": read.usable },
            }))
        }
    }
}

/// Explicit user-triggered attach/probe. Passive `desktop_status` intentionally
/// does not open a platform handle or request desktop permissions; Settings and
/// the Computer-use view call this command when the user asks to measure the
/// live capability surface. Attach failure is returned as an honest status so
/// the UI can render the derived H4 state instead of a generic IPC error.
#[tauri::command]
pub fn desktop_attach(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let _ = get_or_attach(&state, &app);
    // P48.3 — a user-triggered attach on a host that was headless at boot (or
    // whose platform backend only became available later) must reach the agent
    // loop too, not just the Settings surfaces.
    let _ = publish_desktop_backend(&state, &app);
    desktop_status(state)
}

/// List native windows (read-only; estop-guarded, not a mutation).
#[tauri::command]
pub fn desktop_windows(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let engine = get_or_attach(&state, &app)?;
    let windows = engine
        .list_windows()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|w| {
            serde_json::json!({
                "id": w.id, "title": w.title, "app": w.app,
                "x": w.x, "y": w.y, "width": w.width, "height": w.height,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({ "windows": windows }))
}

/// Text read of a window via the a11y tree. Read-only (estop-guarded, not a
/// mutation; no Merkle row is expected for a read).
///
/// `FIX-17`: the response now says **how** the read went, not just whether a tree
/// came back. `has_tree: false` used to be ambiguous between "this app exposes no
/// accessibility tree" (the vision rung) and "this process is not allowed to look"
/// (elevation), and the second is the one a caller must not treat as empty
/// (`REQ-CUA-006`).
#[tauri::command]
pub fn desktop_read(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    window_id: u64,
) -> Result<serde_json::Value, String> {
    let engine = get_or_attach(&state, &app)?;
    let window = resolve_window(&engine, window_id);
    let read = engine.read(&window).map_err(|e| e.to_string())?;
    let text = read.tree.as_ref().map(render_tree).unwrap_or_default();
    Ok(serde_json::json!({
        "tree": text,
        "has_tree": read.tree.is_some(),
        "dpi_scale": read.dpi_scale,
        "status": read.status.as_str(),
        "detail": read.status_detail(),
        "may_infer_absence": read.may_infer_absence(),
        "epoch": read.epoch.0,
        "observed_at_ms": read.observed_at_ms,
        "guidance": read.guidance,
        "anomalies": read
            .anomalies
            .iter()
            .map(|a| serde_json::json!({ "kind": a.kind, "scope": a.scope, "detail": a.detail }))
            .collect::<Vec<_>>(),
    }))
}

/// Capture a window (`see`), returning PNG bytes as base64 for the UI.
/// Read-only (estop-guarded, not a mutation).
///
/// `FIX-18`: the capture path is verified **before** it runs, and the response
/// carries the verdict. The v0 shape (png + dimensions) could not distinguish
/// "captured with graphics capture" from "graphics capture was unavailable, so
/// PrintWindow was used, and here is why" — which is the whole of the readiness
/// requirement (`REQ-CUA-008`, `ARCH/24` §7).
#[tauri::command]
pub fn desktop_see(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    window_id: u64,
) -> Result<serde_json::Value, String> {
    let engine = get_or_attach(&state, &app)?;
    let result = engine
        .see(&resolve_window(&engine, window_id))
        .map_err(|e| e.to_string())?;
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&result.png);
    Ok(serde_json::json!({
        "png": b64,
        "width": result.width,
        "height": result.height,
        "method": format!("{:?}", result.method),
        // `FIX-18` — the readiness verdict the capture was made under, including
        // the pipeline that was skipped and the fault that skipped it.
        "readiness": result.readiness.to_json(),
        "degraded": result.readiness.is_degraded(),
        "budget": result
            .budget
            .as_ref()
            .map(|b| serde_json::json!({ "describe": b.describe() })),
        "describe": result.describe(),
    }))
}

/// Parse the wire act-kind vocabulary into an [`agentcowork_computeruse::ActKind`].
/// Shared by `desktop_act` and the P57.4 escalation pair so both see the exact
/// same act.
#[allow(clippy::too_many_arguments)]
fn parse_act(
    kind: &str,
    x: Option<i32>,
    y: Option<i32>,
    name: Option<String>,
    text: Option<String>,
) -> Result<agentcowork_computeruse::ActKind, String> {
    Ok(match kind {
        "click" => agentcowork_computeruse::ActKind::Click {
            x: x.unwrap_or(0),
            y: y.unwrap_or(0),
        },
        "clickByName" => agentcowork_computeruse::ActKind::ClickByName {
            name: name.ok_or("name required for clickByName")?,
        },
        "type" => agentcowork_computeruse::ActKind::Type {
            text: text.ok_or("text required for type")?,
        },
        "setValue" => agentcowork_computeruse::ActKind::SetValue {
            name: name.ok_or("name required for setValue")?,
            value: text.ok_or("value required for setValue")?,
        },
        "scroll" => agentcowork_computeruse::ActKind::Scroll {
            x: x.unwrap_or(0),
            y: y.unwrap_or(0),
            delta: y.unwrap_or(0),
        },
        // P57.1 — launch the allow-listed program by its canonical path. A
        // launch with no path falls back to the name form, which resolves
        // through PATH in the backend. Guard-2 evaluates the **launch target**
        // as the subject, so an app has to be allow-listed in Settings →
        // Computer use before this can proceed.
        "launch" => match text.filter(|t| !t.trim().is_empty()) {
            Some(path) => agentcowork_computeruse::ActKind::launch_path(path),
            None => agentcowork_computeruse::ActKind::launch_by_name(
                name.ok_or("launch requires a path (text) or an app name")?,
            ),
        },
        other => return Err(format!("unsupported desktop act kind: {other}")),
    })
}

/// Execute ONE human-initiated desktop act, through the engine's Guard-2 gate
/// and audited on the same Merkle chain as every other effect. Fail-closed:
/// risky classes are Denied by `FailClosedGate`; hard-denied apps never run.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command signature — fixed arity by contract
pub fn desktop_act(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    window_id: u64,
    kind: String,
    x: Option<i32>,
    y: Option<i32>,
    name: Option<String>,
    text: Option<String>,
) -> Result<serde_json::Value, String> {
    let engine = get_or_attach(&state, &app)?;
    let act = parse_act(&kind, x, y, name, text)?;
    let outcome = engine
        .act(&resolve_window(&engine, window_id), &act, None)
        .map_err(|e| e.to_string())?;

    // Every executed-or-declined act is audited with human_gesture provenance.
    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "desktop.act",
        serde_json::json!({
            "act": act.describe(),
            "window_id": window_id,
            "executed": outcome.ok && outcome.error.is_none(),
            // `FIX-17` — the ladder verdict carries *why* an act did not land,
            // including the `unknown_region` stop: an elevated target is refused
            // there rather than acted on blind.
            "ladder": outcome.click.as_ref().map(|v| v.to_json()),
            "error": outcome.error,
        }),
    );

    if let Some(err) = outcome.error {
        return Err(format!("desktop.act declined: {err}"));
    }
    Ok(serde_json::json!({
        "ok": true,
        "act": act.describe(),
        "ladder": outcome.click.as_ref().map(|v| v.to_json()),
    }))
}

/// P57.4 — does this act need a foreground escalation under the current
/// interaction default? Pure read: the UI renders the Guard-2 card from this
/// and nothing moves.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn desktop_escalation(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    window_id: u64,
    kind: String,
    x: Option<i32>,
    y: Option<i32>,
    name: Option<String>,
    text: Option<String>,
) -> Result<serde_json::Value, String> {
    let engine = get_or_attach(&state, &app)?;
    let act = parse_act(&kind, x, y, name, text)?;
    serde_json::to_value(engine.escalation_for(&resolve_window(&engine, window_id), &act))
        .map_err(|e| e.to_string())
}

/// P57.4 — run an act that needs a foreground escalation, **only** with an
/// explicit human gesture. Without one the escalation is returned as a refusal
/// (nothing is raised). With one, the previous foreground window is snapshotted,
/// the default is switched to Foreground for this single act, and both are
/// restored afterwards; the outcome (including an honest restore failure) is
/// audited with human_gesture provenance.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn desktop_act_escalating(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    window_id: u64,
    kind: String,
    gesture_approved: bool,
    x: Option<i32>,
    y: Option<i32>,
    name: Option<String>,
    text: Option<String>,
) -> Result<serde_json::Value, String> {
    let engine = get_or_attach(&state, &app)?;
    let act = parse_act(&kind, x, y, name, text)?;
    let snapshot = engine.foreground_snapshot();
    let outcome = engine
        .act_escalating(
            &resolve_window(&engine, window_id),
            &act,
            None,
            gesture_approved,
        )
        .map_err(|e| e.to_string())?;
    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "desktop.act.escalated",
        serde_json::json!({
            "act": act.describe(),
            "window_id": window_id,
            "gestureApproved": gesture_approved,
            "previousForeground": snapshot.window_id,
            "executed": outcome.ok && outcome.error.is_none(),
            "error": outcome.error,
        }),
    );
    if let Some(err) = outcome.error {
        return Err(format!("desktop.act.escalated declined: {err}"));
    }
    Ok(serde_json::json!({
        "ok": true,
        "act": act.describe(),
        "escalated": gesture_approved,
        "restored": snapshot.window_id,
    }))
}

/// Emergency stop — trips the engine's kill switch so every further op fails
/// closed.
#[tauri::command]
pub fn desktop_stop(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let slot = state.desktop.lock().map_err(|e| e.to_string())?;
    if let Some(engine) = slot.engine.as_ref() {
        engine.emergency_stop();
    }
    Ok(serde_json::json!({ "stopped": true }))
}

// ==== P57.8 — Settings → Computer use ========================================

/// The current policy + derived readiness (the section's first read).
#[tauri::command]
pub fn desktop_policy_get(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let slot = state.desktop.lock().map_err(|e| e.to_string())?;
    let (policy, caps, killed) = match slot.engine.as_ref() {
        Some(engine) => (
            engine.policy(),
            Some(engine.capabilities()),
            engine.guard().kill.is_stopped(),
        ),
        None => (load_policy(), None, false),
    };
    let read = agentcowork_computeruse::derive_readiness(
        caps.as_ref(),
        &policy,
        killed,
        slot.last_error.as_deref(),
    );
    Ok(serde_json::json!({
        "policy": policy_json(&policy),
        "attached": slot.engine.is_some(),
        "readiness": { "state": read.state.as_str(), "detail": read.detail, "usable": read.usable },
    }))
}

/// The installed-app inventory, filtered by `query` (all-character match over
/// name + path). Cached per session; `refresh: true` re-scans the disk.
#[tauri::command]
pub fn desktop_apps(
    state: State<'_, AppState>,
    query: Option<String>,
    refresh: Option<bool>,
) -> Result<serde_json::Value, String> {
    let policy = load_policy();
    let mut slot = state.desktop.lock().map_err(|e| e.to_string())?;
    let rescan = refresh.unwrap_or(false) || slot.apps.is_none();
    if rescan {
        // The filesystem scan is session-cached; policy annotations are
        // refreshed below on every read so an Add/Remove write is visible
        // immediately without rescanning application directories.
        slot.apps = Some(agentcowork_computeruse::installed_apps(&policy));
    }
    let mut cached = slot.apps.clone().unwrap_or_default();
    // Keep the disk scan cached, but never keep policy annotations cached: an
    // Add/Remove operation may have changed the persisted policy since the
    // inventory was collected.
    agentcowork_computeruse::annotate_inventory(&mut cached, &policy);
    drop(slot);
    let rows: Vec<serde_json::Value> =
        agentcowork_computeruse::search_apps(&cached, query.as_deref().unwrap_or(""))
            .into_iter()
            .map(|a| {
                serde_json::json!({
                    "name": a.name,
                    "path": a.path,
                    "source": a.source.as_str(),
                    "hardDenied": a.hard_denied,
                    "allowListed": a.allow_listed,
                })
            })
            .collect();
    Ok(serde_json::json!({
        "apps": rows,
        "total": cached.len(),
        "scanned": rescan,
        "platformRoots": agentcowork_computeruse::apps::platform_app_roots()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
    }))
}

/// Allow-list one path (the picker's Add, or **Add by path** after a file
/// picker). The path is canonicalized in Rust and a hard-denied program is
/// refused here too — the UI hiding the button is not the enforcement.
#[tauri::command]
pub fn desktop_policy_allow_path(
    state: State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let mut policy = load_policy();
    let added = policy.add_path(&path)?;
    let applied = commit_policy(&state, &policy)?;
    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "desktop.policy.allow_path",
        serde_json::json!({
            "path": added,
            "requested": path,
            "appliedLive": applied,
            "allowPaths": policy.allow_paths.len(),
        }),
    );
    Ok(serde_json::json!({
        "added": added,
        "appliedLive": applied,
        "policy": policy_json(&policy),
    }))
}

/// Remove an allow-listed path (the Settings Remove action).
#[tauri::command]
pub fn desktop_policy_remove_path(
    state: State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let mut policy = load_policy();
    let removed = policy.remove_path(&path);
    let applied = commit_policy(&state, &policy)?;
    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "desktop.policy.remove_path",
        serde_json::json!({
            "path": path,
            "removed": removed,
            "appliedLive": applied,
        }),
    );
    Ok(serde_json::json!({
        "removed": removed,
        "appliedLive": applied,
        "policy": policy_json(&policy),
    }))
}

/// Set the interaction default (`background` | `foreground`). Background is the
/// contract; foreground is the escalation the engine enforces for window
/// raising (P57.4).
#[tauri::command]
pub fn desktop_policy_set_interaction(
    state: State<'_, AppState>,
    mode: String,
) -> Result<serde_json::Value, String> {
    let parsed = agentcowork_computeruse::InteractionMode::parse(&mode)
        .ok_or_else(|| format!("unknown interaction mode: {mode}"))?;
    let mut policy = load_policy();
    policy.interaction_mode = parsed;
    let applied = commit_policy(&state, &policy)?;
    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "desktop.policy.set_interaction",
        serde_json::json!({ "mode": parsed.as_str(), "appliedLive": applied }),
    );
    Ok(serde_json::json!({
        "appliedLive": applied,
        "policy": policy_json(&policy),
    }))
}

// ==== P48.3 — the inbuilt agent's computer-use path =========================

/// The `agentcowork_core::tools::DesktopBackend` seam the loop's `desktop.*`
/// tools dispatch to.
///
/// It shares the **one** engine and the **same** Guard-2 preflight as the human
/// commands — allow-list, safe zones, hard denies, kill switch, rate limit,
/// foreground/background contract, audit — so there is no second policy and no
/// bypass. The single difference is the provenance it declares: an
/// agent-initiated act is recorded under the agent authority class rather than
/// as the user's own gesture.
///
/// Risky classes still reach this host's `PermissionGate`, which is
/// deny-by-default while the Guard-2 card surface is unbuilt — so an agent's
/// risky act fails closed instead of executing.
struct DesktopEngineBackend {
    engine: Arc<agentcowork_computeruse::DesktopEngine>,
}

/// Parse a wire act for the **agent** path, refusing what this tool's schema
/// cannot express.
///
/// `desktop.act`'s schema carries no raw x/y (`additionalProperties: false`), so
/// a coordinate act has nowhere to come from — and defaulting a missing point to
/// the origin would blind-click (0,0) on the user's desktop. Refuse instead, and
/// point the model at the name-addressed form.
///
/// Pure: no engine, no policy, no side effect — callers do the work.
fn agent_act_kind(
    kind: &str,
    target: Option<&str>,
    text: Option<&str>,
) -> Result<agentcowork_computeruse::ActKind, String> {
    if matches!(kind, "click" | "scroll") {
        return Err(format!(
            "desktop.act '{kind}' needs a coordinate, which this tool does not carry — \
             use 'clickByName' with the control's accessible name"
        ));
    }
    parse_act(
        kind,
        None,
        None,
        target.map(str::to_string),
        text.map(str::to_string),
    )
}

impl agentcowork_core::tools::DesktopBackend for DesktopEngineBackend {
    fn list_windows(&self) -> Result<serde_json::Value, String> {
        let windows = self.engine.list_windows().map_err(|e| e.to_string())?;
        Ok(windows_json(&windows))
    }

    fn read(&self, window_id: u64) -> Result<serde_json::Value, String> {
        let window = resolve_window(&self.engine, window_id);
        let read = self.engine.read(&window).map_err(|e| e.to_string())?;
        Ok(snapshot_json(window_id, &read))
    }

    fn act(
        &self,
        kind: &str,
        window_id: Option<u64>,
        target: Option<&str>,
        text: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let act = agent_act_kind(kind, target, text)?;
        let id = window_id.unwrap_or(0);
        let window = resolve_window(&self.engine, id);
        let outcome = self
            .engine
            .act_with(
                &window,
                &act,
                None,
                agentcowork_computeruse::ActProvenance::Agent,
            )
            .map_err(|e| e.to_string())?;
        if let Some(err) = outcome.error {
            return Err(format!("desktop.act declined: {err}"));
        }
        if !outcome.ok {
            return Err(format!("desktop.act did not complete: {}", act.describe()));
        }
        // `FIX-17` — the verdict travels to the model too: an `unknown_region`
        // stop and an ambiguity are both answers the agent must see, and both
        // name the remedy in the same sentence the UI card shows.
        Ok(serde_json::json!({
            "kind": kind,
            "windowId": id,
            "target": target,
            "text": text,
            "ok": true,
            "ladder": outcome.click.as_ref().map(|v| v.to_json()),
        }))
    }
}

/// Publish the live engine as the agent's `DesktopBackend` on the chat relay.
///
/// Best-effort by design: on a headless / no-display host the engine never
/// attaches and this returns the honest reason — nothing is published, and the
/// `desktop.*` tools keep fail-closing with `desktop session not attached`
/// rather than pretending. An `Err` is **not** a boot failure; a headless host
/// is a legitimate state.
///
/// Called at boot (before the relay is published to `AppState`, so no agent
/// turn can race ahead of the executor) and again from `desktop_attach` so a
/// later attach reaches the agent loop too.
pub fn publish_desktop_backend(state: &AppState, app: &tauri::AppHandle) -> Result<(), String> {
    let engine = get_or_attach(state, app)?;
    let relay = state.chat_relay.lock().map_err(|e| e.to_string())?;
    match relay.as_ref() {
        Some(relay) => {
            relay.attach_desktop(Arc::new(DesktopEngineBackend { engine }));
            Ok(())
        }
        // The relay is not connected yet (early boot ordering). Let the caller
        // know rather than reporting a success it did not achieve.
        None => Err("chat relay not connected yet".to_string()),
    }
}

fn cua_dir(state: &AppState, work_id: &str) -> std::path::PathBuf {
    let base = state
        .replay_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| state.replay_dir.clone());
    let id = if work_id.trim().is_empty() {
        "default"
    } else {
        work_id.trim()
    };
    base.join("cua").join(id)
}

/// P59.8 — load the persisted Computer-use DAG for Progress. Missing file is
/// an empty graph, never a fabricated plan.
#[tauri::command]
pub fn cua_dag_get(state: State<'_, AppState>, work_id: String) -> serde_json::Value {
    let dir = cua_dir(&state, &work_id);
    match agentcowork_core::load_dag(&dir) {
        Ok(dag) => serde_json::json!({ "ok": true, "dag": dag }),
        Err(_) => serde_json::json!({ "ok": true, "dag": null, "reason": "no graph yet" }),
    }
}

/// P59.8 — user edit of a *remaining* node (triggers a replan seq bump).
/// Verified nodes are refused.
#[tauri::command]
pub fn cua_dag_edit_remaining(
    state: State<'_, AppState>,
    work_id: String,
    node_id: String,
    name: Option<String>,
    info: Option<String>,
) -> Result<serde_json::Value, String> {
    let dir = cua_dir(&state, &work_id);
    let mut dag = agentcowork_core::load_dag(&dir)?;
    dag.apply_remaining_edit(&node_id, name, info)?;
    agentcowork_core::append_replan_log(&dir, dag.replan_seq, "user-edit-remaining")?;
    agentcowork_core::persist_dag(&dir, &dag)?;
    Ok(serde_json::json!({ "ok": true, "replanSeq": dag.replan_seq, "dag": dag }))
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    /// The persisted shape is the contract the UI reads; a field rename here
    /// would silently blank the Settings section.
    #[test]
    fn policy_json_exposes_the_contract_fields() {
        let p = agentcowork_computeruse::AppPolicy {
            allow_paths: vec!["/usr/bin/gedit".into()],
            interaction_mode: agentcowork_computeruse::InteractionMode::Foreground,
            ..Default::default()
        };
        let j = policy_json(&p);
        assert_eq!(j["interactionDefault"], "foreground");
        assert_eq!(j["allowsRaisingWindows"], true);
        assert_eq!(j["allowPaths"][0], "/usr/bin/gedit");
        assert_eq!(j["strict"], false);
        let bg = agentcowork_computeruse::AppPolicy {
            strict: true,
            ..Default::default()
        };
        assert_eq!(policy_json(&bg)["allowsRaisingWindows"], false);
    }

    /// The whole point of threading provenance through the engine: an agent act
    /// must never be recorded as the user's own gesture.
    #[test]
    fn provenance_maps_to_the_right_authority_class() {
        use agentcowork_computeruse::ActProvenance;
        assert_eq!(
            auth_kind_for(ActProvenance::HumanGesture),
            crate::control::AuthKind::HumanGesture
        );
        assert_eq!(
            auth_kind_for(ActProvenance::Agent),
            crate::control::AuthKind::AgentTicket
        );
        assert_eq!(
            auth_kind_for(ActProvenance::Automation),
            crate::control::AuthKind::AutomationTicket
        );
        // The vocabulary is distinct — a rename that collapsed two of these
        // would silently re-label agent actions as human ones.
        assert_ne!(
            auth_kind_for(ActProvenance::Agent).as_str(),
            auth_kind_for(ActProvenance::HumanGesture).as_str()
        );
    }

    /// A coordinate act cannot be expressed by `desktop.act`'s schema, and
    /// guessing the origin would blind-click (0,0) on the user's desktop.
    #[test]
    fn agent_act_refuses_coordinate_acts_it_cannot_express() {
        let err = agent_act_kind("click", Some("Save"), None).unwrap_err();
        assert!(err.contains("needs a coordinate"), "got: {err}");
        assert!(err.contains("clickByName"), "got: {err}");
        let err = agent_act_kind("scroll", None, None).unwrap_err();
        assert!(err.contains("needs a coordinate"), "got: {err}");
        // …and the name-addressed forms still parse.
        assert!(matches!(
            agent_act_kind("clickByName", Some("Save"), None).unwrap(),
            agentcowork_computeruse::ActKind::ClickByName { .. }
        ));
        assert!(matches!(
            agent_act_kind("type", None, Some("hello")).unwrap(),
            agentcowork_computeruse::ActKind::Type { .. }
        ));
        // A missing required name is an honest error, not a default.
        assert!(agent_act_kind("clickByName", None, None).is_err());
        assert!(agent_act_kind("teleport", None, None).is_err());
    }

    /// The tool registry serves `desktop.windows` this exact shape; a field
    /// rename here would silently blank the agent's window list.
    #[test]
    fn window_json_carries_the_contract_fields() {
        let w = agentcowork_computeruse::WindowInfo {
            id: 7,
            title: "Notes".into(),
            app: "notepad".into(),
            x: 1,
            y: 2,
            width: 300,
            height: 400,
            has_a11y_tree: true,
        };
        let v = windows_json(&[w]);
        assert_eq!(v[0]["id"], 7);
        assert_eq!(v[0]["title"], "Notes");
        assert_eq!(v[0]["app"], "notepad");
        assert_eq!(v[0]["width"], 300);
    }

    /// A window that no longer exists must resolve to an id-only info (which
    /// then fails the allow-list), never to some other window's app name.
    #[test]
    fn missing_window_falls_back_to_id_only_never_a_borrowed_app() {
        let w = window_by_id_only(42);
        assert_eq!(w.id, 42);
        assert!(w.app.is_empty());
        assert!(!w.has_a11y_tree);
    }

    /// `FIX-17` — the agent-facing snapshot must distinguish "this app exposes no
    /// accessibility tree" from "this process could not look". A field rename or a
    /// dropped status would turn an elevation block back into "no controls", and
    /// the model's next move would be a blind click.
    #[test]
    fn the_agent_snapshot_carries_the_read_status_epoch_and_guidance() {
        let base = agentcowork_computeruse::ReadResult::absent(7, 1.0, vec![]);
        let json = snapshot_json(7, &base);
        assert_eq!(json["windowId"], 7);
        assert_eq!(json["hasTree"], false);
        // An `absent` read is a positive fact: the vision rung may follow.
        assert_eq!(json["status"], "absent");
        assert_eq!(json["mayInferAbsence"], true);
        assert!(json["guidance"].as_str().unwrap().contains("vision rung"));

        // A read that could not look must not report absence as inferable.
        let blocked = agentcowork_computeruse::ReadResult {
            status: agentcowork_computeruse::UiaReadStatus::Unknown {
                detail: "this window runs elevated and this process is not UIAccess-enabled"
                    .into(),
            },
            guidance: Some(
                "this window runs elevated — no input is synthesized into it".into(),
            ),
            epoch: agentcowork_computeruse::SnapshotEpoch(3),
            ..base
        };
        let json = snapshot_json(7, &blocked);
        assert_eq!(json["status"], "unknown");
        assert_eq!(json["mayInferAbsence"], false);
        assert_eq!(json["epoch"], 3);
        assert!(json["detail"]
            .as_str()
            .unwrap()
            .contains("UIAccess"));
        assert!(json["guidance"].as_str().unwrap().contains("no input"));
    }

    #[test]
    fn a_malformed_policy_file_is_never_a_partial_allow_list() {
        // `load_policy` reads the real data dir, so assert the decode rule the
        // same way it is applied: a bad document yields the default.
        let bad: Result<agentcowork_computeruse::AppPolicy, _> = serde_json::from_slice(b"{oops");
        assert!(bad.is_err());
        let fallback = bad.unwrap_or_default();
        assert!(fallback.allow_paths.is_empty());
        assert_eq!(
            fallback.interaction_mode,
            agentcowork_computeruse::InteractionMode::Background
        );
    }
}
