//! Local-runtime inventory and managed lifecycle commands.
//!
//! This module is the Tauri projection for `ARCH/16-LOCAL-RUNTIME-INTEROP.md`.
//! External runtimes are observations only. Managed runtimes are present only
//! while the shell retains their process custody; lifecycle effects use the
//! existing GuardService evaluate → consume path and are audited afterwards.

use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use agentcowork_core::models::discover_runtime_inventory;
use agentcowork_core::GuardDecision;
use agentcowork_guard::{DecisionPackage, Operation, RiskLevel};
use agentcowork_types::{RuntimeHealthState, RuntimeInventoryEntry, RuntimeOwnership};
use serde::Serialize;
use tauri::State;
use url::Url;

use crate::model_cmds::{
    start_managed_serve, ManagedServeProcess, ManagedServeSnapshot, ModelServeStart,
};
use crate::AppState;

const MAX_RUNTIME_RESPONSE_BYTES: usize = 1024 * 1024;
const RUNTIME_HTTP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub(crate) struct CachedRuntimeObservation {
    entry: RuntimeInventoryEntry,
    models_observed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeModelsObservation {
    runtime_id: String,
    kind: String,
    endpoint: String,
    health: RuntimeHealthState,
    last_probe_ms: Option<u64>,
    models: Vec<String>,
    models_observed: bool,
    detail: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Build the deterministic argument identity consumed by Guard's ticket.
pub(crate) fn runtime_effect_args_hash(action: &str, target: &str) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    action.hash(&mut hash);
    target.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

/// Evaluate and consume the one Guard ticket for a local-runtime lifecycle effect.
///
/// This is the same direct UI-command pattern used by artifact serving: the
/// Guard lock is never held across process start/stop or endpoint I/O.
pub(crate) fn authorize_runtime_effect(
    state: &AppState,
    action: &str,
    detail: &str,
    port: u16,
    args_hash: &str,
) -> Result<(), String> {
    let decision = DecisionPackage::new(detail.to_string())
        .with_risk(RiskLevel::Medium)
        .with_network(vec![format!("127.0.0.1:{port}")]);
    let mut guard = state
        .guard_service
        .lock()
        .map_err(|error| error.to_string())?;
    match guard.evaluate(
        "local-runtime",
        "everyaios",
        action,
        Operation::GenericWrite,
        decision,
        args_hash,
        0,
    ) {
        GuardDecision::Allow { ticket_id } => guard
            .use_ticket(&ticket_id, args_hash)
            .map_err(|error| format!("{action} ticket not consumable: {error}")),
        GuardDecision::Ask { ticket_id } => Err(format!(
            "{action} requires approval in the Guard window (ticket {ticket_id}); approve it and retry"
        )),
        GuardDecision::Block { reason } => Err(format!("{action} blocked: {reason}")),
    }
}

fn managed_inventory_entry(row: &ManagedServeSnapshot) -> RuntimeInventoryEntry {
    RuntimeInventoryEntry {
        id: row.id.clone(),
        kind: row.kind.clone(),
        endpoint: row.base_url.clone(),
        version: None,
        protocol: "openai_compatible".to_string(),
        ownership: RuntimeOwnership::Managed,
        health: row.health,
        last_probe_ms: (row.health != RuntimeHealthState::Unknown).then(now_ms),
        models: vec![row.model_id.clone()],
        agent_compatibility: Vec::new(),
    }
}

fn managed_inventory_from_start(start: &ModelServeStart) -> RuntimeInventoryEntry {
    RuntimeInventoryEntry {
        id: start.serve_id.clone(),
        kind: if start.model_id.starts_with("mlx-community/") {
            "mlx".to_string()
        } else {
            "gguf".to_string()
        },
        endpoint: start.base_url.clone(),
        version: None,
        protocol: "openai_compatible".to_string(),
        ownership: RuntimeOwnership::Managed,
        health: start.health,
        last_probe_ms: Some(now_ms()),
        models: vec![start.model_id.clone()],
        agent_compatibility: Vec::new(),
    }
}

/// Merge external observations with retained managed entries, preferring the
/// managed identity when both probes see the same loopback endpoint.
fn merge_runtime_inventory(
    external: Vec<RuntimeInventoryEntry>,
    managed: Vec<RuntimeInventoryEntry>,
) -> Vec<RuntimeInventoryEntry> {
    let managed_endpoints: std::collections::HashSet<String> = managed
        .iter()
        .map(|entry| entry.endpoint.trim_end_matches('/').to_string())
        .collect();
    let mut merged = managed;
    merged.extend(
        external
            .into_iter()
            .filter(|entry| !managed_endpoints.contains(entry.endpoint.trim_end_matches('/'))),
    );
    merged.sort_by(|left, right| left.id.cmp(&right.id));
    merged
}

fn cache_observations(
    state: &AppState,
    entries: &[RuntimeInventoryEntry],
    models_observed: impl Fn(&RuntimeInventoryEntry) -> bool,
) -> Result<(), String> {
    let mut cache = state
        .runtime_observations
        .lock()
        .map_err(|error| error.to_string())?;
    for entry in entries {
        cache.insert(
            entry.id.clone(),
            CachedRuntimeObservation {
                entry: entry.clone(),
                models_observed: models_observed(entry),
            },
        );
    }
    Ok(())
}

/// Return external runtime observations plus every retained managed runtime.
#[tauri::command]
pub fn runtime_inventory_list(
    state: State<'_, AppState>,
) -> Result<Vec<RuntimeInventoryEntry>, String> {
    let external = discover_runtime_inventory();
    let managed = {
        let mut registry = state
            .model_serves
            .lock()
            .map_err(|error| error.to_string())?;
        registry
            .rows()
            .iter()
            .map(managed_inventory_entry)
            .collect()
    };
    let merged = merge_runtime_inventory(external, managed);
    cache_observations(&state, &merged, |entry| {
        entry.health == RuntimeHealthState::Observed || !entry.models.is_empty()
    })?;
    Ok(merged)
}

fn cached_observation(
    state: &AppState,
    runtime_id: &str,
) -> Result<Option<CachedRuntimeObservation>, String> {
    Ok(state
        .runtime_observations
        .lock()
        .map_err(|error| error.to_string())?
        .get(runtime_id)
        .cloned())
}

fn parse_loopback_url(endpoint: &str) -> Result<(Url, SocketAddr), String> {
    let url = Url::parse(endpoint).map_err(|error| format!("invalid runtime endpoint: {error}"))?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("runtime endpoint is not an unauthenticated loopback HTTP URL".to_string());
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "runtime endpoint has no port".to_string())?;
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    Ok((url, address))
}

fn decode_chunked(body: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::new();
    let mut cursor = 0usize;
    loop {
        let line_end = body[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|position| cursor + position)?;
        let size_text = std::str::from_utf8(&body[cursor..line_end]).ok()?;
        let size_text = size_text.split(';').next()?.trim();
        let size = usize::from_str_radix(size_text, 16).ok()?;
        cursor = line_end + 2;
        if size == 0 {
            return Some(decoded);
        }
        let end = cursor.checked_add(size)?;
        if end + 2 > body.len() || &body[end..end + 2] != b"\r\n" {
            return None;
        }
        if decoded.len().saturating_add(size) > MAX_RUNTIME_RESPONSE_BYTES {
            return None;
        }
        decoded.extend_from_slice(&body[cursor..end]);
        cursor = end + 2;
    }
}

fn read_http_json(address: SocketAddr, path: &str) -> Option<serde_json::Value> {
    let mut stream = TcpStream::connect_timeout(&address, RUNTIME_HTTP_TIMEOUT).ok()?;
    stream.set_read_timeout(Some(RUNTIME_HTTP_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(RUNTIME_HTTP_TIMEOUT)).ok()?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).ok()?;

    let mut raw = Vec::new();
    let mut buffer = [0_u8; 4096];
    while raw.len() < MAX_RUNTIME_RESPONSE_BYTES {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => raw.extend_from_slice(&buffer[..count]),
            Err(_) if !raw.is_empty() => break,
            Err(_) => return None,
        }
    }
    if raw.len() >= MAX_RUNTIME_RESPONSE_BYTES {
        return None;
    }
    let header_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)?;
    let headers = std::str::from_utf8(&raw[..header_end]).ok()?;
    let status = headers
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse::<u16>()
        .ok()?;
    if !(200..300).contains(&status) {
        return None;
    }
    let body = &raw[header_end..];
    let body = if headers
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_chunked(body)?
    } else if let Some(length) = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())?
    }) {
        body.get(..length.min(body.len()))?.to_vec()
    } else {
        body.to_vec()
    };
    serde_json::from_slice(&body).ok()
}

fn query_runtime_models(endpoint: &str, kind: &str) -> Option<Vec<String>> {
    let (url, address) = parse_loopback_url(endpoint).ok()?;
    let path = if kind.eq_ignore_ascii_case("ollama") {
        format!(
            "{}:{}/api/tags",
            url.host_str().unwrap_or("127.0.0.1"),
            url.port().unwrap_or(80)
        )
    } else {
        let path = url.path().trim_end_matches('/');
        if path.ends_with("/v1") {
            format!("{path}/models")
        } else {
            format!("{path}/v1/models")
        }
    };
    let value = read_http_json(address, &path)?;
    let rows = if kind.eq_ignore_ascii_case("ollama") {
        value.get("models")?.as_array()?
    } else {
        value.get("data")?.as_array()?
    };
    Some(
        rows.iter()
            .filter_map(|model| {
                model
                    .get("id")
                    .or_else(|| model.get("name"))
                    .or_else(|| model.get("model"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .collect(),
    )
}

fn runtime_models_view(
    runtime_id: String,
    entry: &RuntimeInventoryEntry,
    models: Vec<String>,
    models_observed: bool,
    detail: impl Into<String>,
) -> RuntimeModelsObservation {
    RuntimeModelsObservation {
        runtime_id,
        kind: entry.kind.clone(),
        endpoint: entry.endpoint.clone(),
        health: entry.health,
        last_probe_ms: entry.last_probe_ms,
        models,
        models_observed,
        detail: detail.into(),
    }
}

/// Return the model observation for one runtime, refreshing a live endpoint
/// when possible and preserving the last non-empty observation on failure.
#[tauri::command]
pub fn runtime_models(
    state: State<'_, AppState>,
    runtime_id: String,
) -> Result<RuntimeModelsObservation, String> {
    let runtime_id = runtime_id.trim().to_string();
    if runtime_id.is_empty() {
        return Err("runtimeId is required".to_string());
    }

    let managed = {
        let mut registry = state
            .model_serves
            .lock()
            .map_err(|error| error.to_string())?;
        registry.row(&runtime_id)
    };
    if let Some(row) = managed {
        let mut entry = managed_inventory_entry(&row);
        if row.health != RuntimeHealthState::Down {
            if let Some(models) = query_runtime_models(&row.base_url, &row.kind) {
                entry.health = RuntimeHealthState::Healthy;
                entry.last_probe_ms = Some(now_ms());
                entry.models = models.clone();
                cache_observations(&state, &[entry.clone()], |_| true)?;
                return Ok(runtime_models_view(
                    runtime_id,
                    &entry,
                    models,
                    true,
                    "models refreshed from the managed runtime",
                ));
            }
        }
        entry.health = RuntimeHealthState::Down;
        entry.last_probe_ms = Some(now_ms());
        let models = entry.models.clone();
        let had_observation = !models.is_empty();
        cache_observations(&state, &[entry.clone()], |_| had_observation)?;
        return Ok(runtime_models_view(
            runtime_id,
            &entry,
            models,
            had_observation,
            "managed runtime did not answer; returning its last retained model observation as down",
        ));
    }

    let external = discover_runtime_inventory()
        .into_iter()
        .find(|entry| entry.id == runtime_id);
    if let Some(mut entry) = external {
        if entry.health == RuntimeHealthState::Observed {
            let models = entry.models.clone();
            let observed = true;
            cache_observations(&state, &[entry.clone()], |_| observed)?;
            return Ok(runtime_models_view(
                runtime_id,
                &entry,
                models,
                true,
                "models observed from the external runtime; no agent compatibility is implied",
            ));
        }
        entry.health = RuntimeHealthState::Down;
        entry.last_probe_ms = Some(now_ms());
        let models = entry.models.clone();
        cache_observations(&state, &[entry.clone()], |_| false)?;
        return Ok(runtime_models_view(
            runtime_id,
            &entry,
            models,
            false,
            "runtime did not answer; the empty list is not a verified absence",
        ));
    }

    if let Some(cached) = cached_observation(&state, &runtime_id)? {
        let mut entry = cached.entry;
        entry.health = RuntimeHealthState::Down;
        entry.last_probe_ms = Some(now_ms());
        let models = entry.models.clone();
        return Ok(runtime_models_view(
            runtime_id,
            &entry,
            models,
            cached.models_observed,
            "runtime did not answer; returning the last observation as down",
        ));
    }

    Err(format!(
        "runtime {runtime_id} was not found and has no prior model observation; an empty model list would not be truthful"
    ))
}

fn normalize_runtime_kind(kind: &str) -> String {
    match kind.trim().to_ascii_lowercase().as_str() {
        "llamafile" | "llama.cpp" | "llama_cpp" | "gguf" => "gguf".to_string(),
        "ollama" => "ollama".to_string(),
        "mlx" => "mlx".to_string(),
        other => other.to_string(),
    }
}

fn runtime_start_response(start: &ModelServeStart) -> serde_json::Value {
    let runtime = managed_inventory_from_start(start);
    serde_json::json!({
        "ok": true,
        "started": true,
        "health": start.health,
        "ownership": start.ownership,
        "runtime": runtime,
    })
}

/// Start a runtime AgentCowork can retain and stop. External inventory rows are
/// always refused before any effect.
#[tauri::command]
pub fn runtime_start(
    state: State<'_, AppState>,
    runtime_id: Option<String>,
    runtime_kind: Option<String>,
    model_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let runtime_id = runtime_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    if let Some(id) = runtime_id {
        let existing = {
            let mut registry = state
                .model_serves
                .lock()
                .map_err(|error| error.to_string())?;
            registry.row(id)
        };
        if let Some(row) = existing {
            return Ok(serde_json::json!({
                "ok": true,
                "started": false,
                "alreadyManaged": true,
                "health": row.health,
                "runtime": managed_inventory_entry(&row),
            }));
        }
        let current_external = discover_runtime_inventory()
            .iter()
            .any(|entry| entry.id == id && entry.ownership == RuntimeOwnership::External);
        let cached_external = cached_observation(&state, id)?
            .is_some_and(|cached| cached.entry.ownership == RuntimeOwnership::External);
        if current_external || cached_external {
            return Err(format!(
                "runtime {id} is user-owned (External) and must be started or stopped by the user"
            ));
        }
    }

    let kind = match runtime_kind.as_deref().map(normalize_runtime_kind) {
        Some(kind) => kind,
        None => {
            let cached = runtime_id
                .and_then(|id| cached_observation(&state, id).ok().flatten())
                .map(|cached| cached.entry.kind);
            cached.ok_or_else(|| {
                "runtimeKind is required when runtimeId has no current observation".to_string()
            })?
        }
    };

    match kind.as_str() {
        "gguf" | "llamafile" => {
            let model_id = model_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "modelId is required for a managed GGUF runtime".to_string())?;
            let start = start_managed_serve(&state, model_id, None)?;
            Ok(runtime_start_response(&start))
        }
        "ollama" => {
            if discover_runtime_inventory()
                .iter()
                .any(|entry| entry.kind == "ollama")
            {
                return Err(
                    "Ollama is user-owned (External) and must be started or stopped by the user"
                        .to_string(),
                );
            }
            Err(
                "Ollama managed start is not wired: the local manager does not retain the child \
                 process; attach to a user-run Ollama instead"
                    .to_string(),
            )
        }
        other => Err(format!(
            "runtime kind {other} has no retained managed-start implementation"
        )),
    }
}

fn unregistered_managed_serve_error(runtime_id: &str) -> String {
    format!(
        "runtime {runtime_id} is not in the managed serve registry; it is user-owned (or unknown) and must be stopped by the user"
    )
}

/// Stop one registered managed serve after consuming a Guard ticket.
pub(crate) fn stop_managed_serve(
    state: &AppState,
    runtime_id: &str,
) -> Result<serde_json::Value, String> {
    let runtime_id = runtime_id.trim();
    if runtime_id.is_empty() {
        return Err("runtimeId is required".to_string());
    }
    let port = {
        let mut registry = state
            .model_serves
            .lock()
            .map_err(|error| error.to_string())?;
        if !registry.contains(runtime_id) {
            return Err(unregistered_managed_serve_error(runtime_id));
        }
        let Some(row) = registry.row(runtime_id) else {
            return Err(unregistered_managed_serve_error(runtime_id));
        };
        row.port
    };
    let args_hash = runtime_effect_args_hash("runtime.stop", runtime_id);
    authorize_runtime_effect(
        state,
        "runtime.stop",
        &format!("stop AgentCowork-managed runtime {runtime_id}"),
        port,
        &args_hash,
    )?;

    let handle = state
        .model_serves
        .lock()
        .map_err(|error| error.to_string())?
        .remove(runtime_id)
        .ok_or_else(|| unregistered_managed_serve_error(runtime_id))?;
    ManagedServeProcess::stop_owned(handle)?;
    crate::control::record_mutation(
        state,
        crate::control::AuthKind::AgentTicket,
        "runtime.stop",
        serde_json::json!({
            "runtimeId": runtime_id,
            "ownership": "managed",
        }),
    );
    Ok(serde_json::json!({
        "ok": true,
        "stopped": true,
        "runtimeId": runtime_id,
    }))
}

/// Stop a runtime only when its exact id is retained in the managed registry.
#[tauri::command]
pub fn runtime_stop(
    state: State<'_, AppState>,
    runtime_id: String,
) -> Result<serde_json::Value, String> {
    stop_managed_serve(&state, &runtime_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_cmds::ManagedServeRegistry;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[derive(Debug)]
    struct FakeManagedServe {
        stopped: Arc<AtomicBool>,
    }

    impl ManagedServeProcess for FakeManagedServe {
        fn port(&self) -> u16 {
            41_235
        }

        fn base_url(&self) -> &str {
            "http://127.0.0.1:41235"
        }

        fn model_id(&self) -> &str {
            "fixture/model"
        }

        fn config_hash(&self) -> &str {
            "fixture-config"
        }

        fn health(&mut self) -> RuntimeHealthState {
            RuntimeHealthState::Healthy
        }

        fn stop_owned(handle: Self) -> Result<(), String> {
            handle.stopped.store(true, Ordering::Release);
            Ok(())
        }
    }

    fn external_entry() -> RuntimeInventoryEntry {
        RuntimeInventoryEntry {
            id: "ollama@127.0.0.1:11434".to_string(),
            kind: "ollama".to_string(),
            endpoint: "http://127.0.0.1:11434".to_string(),
            version: None,
            protocol: "ollama".to_string(),
            ownership: RuntimeOwnership::External,
            health: RuntimeHealthState::Observed,
            last_probe_ms: Some(42),
            models: vec!["fixture-model".to_string()],
            agent_compatibility: Vec::new(),
        }
    }

    #[test]
    fn registry_retains_and_stops_an_injected_managed_serve() {
        let stopped = Arc::new(AtomicBool::new(false));
        let mut registry = ManagedServeRegistry::<FakeManagedServe>::default();
        registry
            .insert(
                "serve-fixture".to_string(),
                "gguf".to_string(),
                7,
                FakeManagedServe {
                    stopped: Arc::clone(&stopped),
                },
            )
            .unwrap();
        let row = registry.row("serve-fixture").unwrap();
        assert_eq!(row.model_id, "fixture/model");
        assert_eq!(row.health, RuntimeHealthState::Healthy);

        let handle = registry.remove("serve-fixture").unwrap();
        FakeManagedServe::stop_owned(handle).unwrap();
        assert!(stopped.load(Ordering::Acquire));
        assert!(!registry.contains("serve-fixture"));
    }

    #[test]
    fn stopping_a_non_registered_runtime_is_an_explicit_user_owned_refusal() {
        let error = unregistered_managed_serve_error("external-runtime");
        assert!(error.contains("user-owned"));
        assert!(error.contains("must be stopped by the user"));
    }

    #[test]
    fn inventory_keeps_external_runtime_as_external_and_observed() {
        let merged = merge_runtime_inventory(vec![external_entry()], Vec::new());
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].ownership, RuntimeOwnership::External);
        assert_eq!(merged[0].health, RuntimeHealthState::Observed);
        assert!(merged[0].agent_compatibility.is_empty());
    }
}
