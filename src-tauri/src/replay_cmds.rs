//! P3.1 — replay/audit Tauri commands. Thin wrappers over the
//! `agentcowork-audit` replay store + the unix control channel; all logic and
//! tests live in the crates, the shell just exposes them to the UI.

use agentcowork_audit::replay::{ReplayEvent, ReplayStore, Segment, Timeline};
use tauri::State;

use crate::AppState;

/// The replay store rooted at the app's data dir.
pub fn replay_store(state: &AppState) -> ReplayStore {
    ReplayStore::new(state.replay_dir.clone())
}

/// Searchable sessions list (P3.1): segments filtered by document/tab id.
#[tauri::command]
pub fn replay_sessions(
    state: State<'_, AppState>,
    query: Option<String>,
) -> Result<Vec<Segment>, String> {
    replay_store(&state)
        .search_sessions(query.as_deref().unwrap_or(""))
        .map_err(|e| e.to_string())
}

/// Full scrubber data for one document: segment + events + screenshot steps.
#[tauri::command]
pub fn replay_timeline(
    state: State<'_, AppState>,
    document_id: String,
) -> Result<Timeline, String> {
    replay_store(&state)
        .timeline(&document_id)
        .map_err(|e| e.to_string())
}

/// A step's screenshot as a base64 `data:image/jpeg;base64,` URL (or null).
#[tauri::command]
pub fn replay_screenshot(
    state: State<'_, AppState>,
    document_id: String,
    step: u64,
) -> Result<Option<String>, String> {
    let store = replay_store(&state);
    let path = match store.screenshot_path(&document_id, step) {
        Some(p) => p,
        None => return Ok(None),
    };
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(Some(format!(
        "data:image/jpeg;base64,{}",
        base64_encode(&bytes)
    )))
}

/// Watch mode (P3.1): live tail of a document's stream since `since_seq`.
#[tauri::command]
pub fn watch_events(
    state: State<'_, AppState>,
    document_id: String,
    since_seq: u64,
) -> Result<Vec<ReplayEvent>, String> {
    replay_store(&state)
        .events_since(&document_id, since_seq)
        .map_err(|e| e.to_string())
}

/// Stop button (P3.1): cancel in-flight streams for the session.
#[tauri::command]
pub fn agent_stop(app: tauri::AppHandle, session_id: String) -> Result<serde_json::Value, String> {
    crate::control::stop_session(&app, &session_id)
        .map(|ids| serde_json::json!({ "cancelled": ids }))
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
