//! Calendar & Calendar Event IPC commands (Open WebUI parity & scheduling).
//!
//! Exposes CRUD operations for persistent calendars and AI-scheduled events
//! stored securely in the SQLCipher vault (`ui_calendars`, `ui_calendar_events`).

use agentcowork_vault::{CalendarEventRow, CalendarRow};
use serde_json::Value;
use tauri::State;

use crate::AppState;

#[tauri::command]
pub fn calendar_list(state: State<'_, AppState>) -> Result<Value, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let calendars = vault.list_ui_calendars().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "calendars": calendars }))
}

#[tauri::command]
pub fn calendar_put(state: State<'_, AppState>, calendar: Value) -> Result<bool, String> {
    let row: CalendarRow =
        serde_json::from_value(calendar).map_err(|e| format!("bad calendar: {e}"))?;
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault.put_ui_calendar(&row).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
pub fn calendar_delete(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault.delete_ui_calendar(&id).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
pub fn calendar_event_list(
    state: State<'_, AppState>,
    calendar_id: Option<String>,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
) -> Result<Value, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let start = start_ts.unwrap_or(0);
    let end = end_ts.unwrap_or(i64::MAX);
    let events = vault
        .list_ui_calendar_events(calendar_id.as_deref(), start, end)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "events": events }))
}

#[tauri::command]
pub fn calendar_event_put(state: State<'_, AppState>, event: Value) -> Result<bool, String> {
    let row: CalendarEventRow =
        serde_json::from_value(event).map_err(|e| format!("bad calendar event: {e}"))?;
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault
        .put_ui_calendar_event(&row)
        .map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
pub fn calendar_event_delete(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault
        .delete_ui_calendar_event(&id)
        .map_err(|e| e.to_string())?;
    Ok(true)
}
