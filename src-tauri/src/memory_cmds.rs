//! P11.5.6 — memory browser backend. Exposes the in-process `MemoryService`
//! (the same store the coordinator's `memory/*` JSON-RPC dispatches to) over
//! Tauri commands so the memory panel lists real knowledge instead of
//! `mockMemory`. `memory_request` is the generic JSON-RPC passthrough;
//! `memory_read` is the convenient read-and-rank shortcut used by the
//! panel's search box.
//!
//! Honest ceiling: MemoryService is the in-process store (the durable disk
//! persistence is the P5.1 ⚠️ follow-up); writes still arrive from the
//! coordinator's `extractMemory` turn path. The panel is read-mostly.

use std::sync::Arc;

use tauri::State;

use crate::AppState;

/// Clone the shared `MemoryService` Arc out of the live relay. The Arc keeps
/// the store alive independently of the relay lock, so callers can lock it
/// without a borrow-of-temporary.
fn memory_arc(
    state: &State<'_, AppState>,
) -> Result<Arc<std::sync::Mutex<agentcowork_core::MemoryService>>, String> {
    crate::ensure_sidecar(state);
    let relay = state.chat_relay.lock().map_err(|e| e.to_string())?;
    let relay = relay
        .as_ref()
        .ok_or_else(|| "sidecar not connected — coordinator link not established".to_string())?;
    Ok(relay.memory())
}

/// Generic `memory/*` JSON-RPC passthrough (write/read/plan/forget/ghost/
/// usage/snapshot/status/consolidate). Mutations are written through to the
/// durable `memory.json` store exactly like the sidecar dispatch path, so the
/// panel's Add/Delete edits survive restarts (P50.2.2).
#[tauri::command]
pub fn memory_request(
    state: State<'_, AppState>,
    method: String,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mem = memory_arc(&state)?;
    let mut mem = mem.lock().map_err(|e| e.to_string())?;
    let is_mutation = matches!(
        method.as_str(),
        "memory/write"
            | "memory/forget"
            | "memory/ghost"
            | "memory/ghost_batch"
            | "memory/consolidate"
            | "memory/tick"
            | "memory/scope"
            | "memory/assess"
            | "memory/load"
    );
    let out = mem.handle(&method, &params).map_err(|e| e.to_string())?;
    if is_mutation {
        let dir = agentcowork_core::default_data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = mem.save_to(&dir.join("memory.json"));
    }
    Ok(out)
}

/// Read-and-rank shortcut: query the memory store for the top-k relevant
/// facts (used by the memory browser's search box).
#[tauri::command]
pub fn memory_read(
    state: State<'_, AppState>,
    query: String,
    k: Option<usize>,
) -> Result<serde_json::Value, String> {
    let mem = memory_arc(&state)?;
    let mem = mem.lock().map_err(|e| e.to_string())?;
    let results = mem.read(&query, k.unwrap_or(8));
    Ok(serde_json::json!({ "query": query, "results": results }))
}
