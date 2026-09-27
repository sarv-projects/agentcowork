//! P1.8 — local model picker (LM Studio-style fit badges + context floor).

use agentcowork_core::{detect_hardware, LocalManager};
use tauri::State;

use crate::AppState;

#[tauri::command]
pub fn local_models() -> Result<serde_json::Value, String> {
    let cfg = agentcowork_core::Config::load().unwrap_or_default();
    let mgr = LocalManager::from_config(&cfg);
    let hw = detect_hardware();
    Ok(serde_json::json!({
        "hardware": hw,
        "models": mgr.list_for_picker(),
        "ctxFloor": 15_000,
        "ctxSoft": 20_000,
    }))
}

#[tauri::command]
pub fn local_ensure(
    state: State<'_, AppState>,
    runtime: String,
    model: Option<String>,
) -> Result<serde_json::Value, String> {
    let result = crate::runtime_cmds::runtime_start(state, None, Some(runtime.clone()), model)?;
    Ok(serde_json::json!({
        "ok": true,
        "runtime": runtime,
        "result": result,
    }))
}

#[tauri::command]
pub fn local_hardware(_state: State<'_, AppState>) -> serde_json::Value {
    serde_json::to_value(detect_hardware()).unwrap_or(serde_json::json!({}))
}
