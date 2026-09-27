//! P5.9 (J5) — Trajectory Tauri commands. Thin wrappers over the
//! `agentcowork-audit` session log; the logic + tests live in the crate, the
//! shell just exposes the inspect-by-source context-injection view to the UI.

use agentcowork_audit::session_log::{list_session_ids, ContextInjectionRecord, SessionLog};
use tauri::State;

use crate::AppState;

/// The session ids that have a context-injection log (Trajectory source list).
#[tauri::command]
pub fn trajectory_sessions(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    list_session_ids(&state.replay_dir).map_err(|e| e.to_string())
}

/// P5.9/J5 — one session's context-injection records, newest-last (the view
/// groups them by source: persona / user_document / memory / tool_result /
/// blueprint / other).
#[tauri::command]
pub fn trajectory_snapshot(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<ContextInjectionRecord>, String> {
    SessionLog::open(&state.replay_dir, &session_id)
        .map_err(|e| e.to_string())?
        .context_injections()
        .map_err(|e| e.to_string())
}
