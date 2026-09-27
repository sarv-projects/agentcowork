//! P3.2 — cockpit / ambient flight-deck Tauri commands (H2, doc 33 §9.5).
//!
//! Thin wrappers over `agentcowork_audit::cockpit`: the live in-memory state
//! lives in `AppState`, the coordinator/sidecar feeds it (the `cockpit_*`
//! feed commands are the seam), and the UI polls `cockpit_snapshot`. The
//! control-channel writes (`agent/undo`, `agent/interrupt-response`) mirror
//! the existing `agent/stop` pattern.
//!
//! The cockpit lock recovers from poison (`.unwrap_or_else(PoisonError::
//! into_inner)`) rather than panicking. It is plain display state — agent
//! cards, open interrupts, a quiet flag — with no invariant to protect, so a
//! thread that panicked while holding it must not permanently wedge the flight
//! deck for every later poll. (The guard-service lock is the opposite case: it
//! holds ticket/policy authority, so it fails **closed**.)

use agentcowork_audit::cockpit::{AgentCard, CockpitState};
use tauri::{AppHandle, Manager, State};

use crate::AppState;

/// Snapshot of the whole cockpit (agent cards + open interrupts + quiet
/// flag) — the UI polls this for the flight deck.
#[tauri::command]
pub fn cockpit_snapshot(state: State<'_, AppState>) -> CockpitState {
    state
        .cockpit
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Feed seam: record a live agent action (the coordinator's tool calls land
/// here via the control channel; also used by tests/demo feeds).
#[tauri::command]
pub fn cockpit_activity(
    state: State<'_, AppState>,
    agent_id: String,
    tool: String,
    summary: String,
) -> Result<(), String> {
    let ts = now_ms();
    state
        .cockpit
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .agent_action(ts, agent_id, tool, summary);
    Ok(())
}

/// Feed seam: update an agent's token counters.
#[tauri::command]
pub fn cockpit_tokens(
    state: State<'_, AppState>,
    agent_id: String,
    tokens_in: u64,
    tokens_out: u64,
) -> Result<(), String> {
    let ok = state
        .cockpit
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .agent_tokens(&agent_id, tokens_in, tokens_out);
    if ok {
        Ok(())
    } else {
        Err(format!("unknown agent: {agent_id}"))
    }
}

/// Quiet mode: collapse the cockpit to a single-sentence tray status. Turns
/// the tray tooltip into the status line and hides the window (the tray's
/// Show item restores it). `status` overrides the auto-generated line.
#[tauri::command]
pub fn cockpit_quiet(
    app: AppHandle,
    state: State<'_, AppState>,
    quiet: bool,
    status: Option<String>,
) -> Result<(), String> {
    {
        let mut s = state
            .cockpit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        s.quiet = quiet;
    }
    let line = match status {
        Some(s) => s,
        None => state
            .cockpit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .quiet_status(now_ms()),
    };
    if let Some(tray) = app.tray_by_id("main-tray") {
        let tooltip = if quiet { Some(line.clone()) } else { None };
        tray.set_tooltip(tooltip).map_err(|e| e.to_string())?;
    }
    if quiet {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.hide();
        }
    }
    Ok(())
}

/// UNDO: revert last action (cockpit + audit). Unix clients hit the same
/// helper via `agent/undo` on the control channel.
#[tauri::command]
pub fn agent_undo(app: AppHandle, session_id: String) -> Result<(), String> {
    crate::control::undo_session(&app, &session_id)
}

/// Answer a circuit-break MCQ interrupt card and forward it to the plan
/// executor (`agent/interrupt-response`).
#[tauri::command]
pub fn interrupt_respond(
    app: AppHandle,
    interrupt_id: String,
    choice: usize,
) -> Result<(), String> {
    crate::control::interrupt_response(&app, &interrupt_id, &choice.to_string())
}

/// Feed seam: upsert a full agent card (coordinator registration).
#[tauri::command]
pub fn cockpit_upsert_agent(
    state: State<'_, AppState>,
    agent_id: String,
    label: String,
    model: String,
    provider: String,
) -> Result<(), String> {
    let card = AgentCard::new(agent_id, label, model, provider, now_ms());
    state
        .cockpit
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .upsert_agent(card);
    Ok(())
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
