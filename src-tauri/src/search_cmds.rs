//! P55.8 — the search-engine surface: which SearXNG endpoints the G8 cascade
//! will actually use, and the `searx.space` instance feed behind the opt-in.
//!
//! The privacy default is **local-first**: your own SearXNG on the default
//! ports, then the DDG fallback. Public instances are only appended when the
//! user turns them on, and the decision is persisted by
//! [`agentcowork_core::search_config`] so the live cascade and this surface can
//! never disagree.
//!
//! Failure honesty: a feed that cannot be fetched is an error here (never an
//! empty list presented as success), and a cached list is labelled as such.

use tauri::State;

use crate::AppState;

/// Read the resolved search configuration without any network access.
#[tauri::command]
pub fn search_config() -> Result<serde_json::Value, String> {
    let config = agentcowork_core::search_config::load();
    Ok(serde_json::json!({
        "usePublic": config.use_public_instances,
        "endpoints": agentcowork_core::search_config::resolved_endpoints_for(&config),
        "localEndpoints": agentcowork_core::search_config::LOCAL_ENDPOINTS,
        "publicEndpoints": config.public_endpoints,
    }))
}

/// Discover public SearXNG instances from the `searx.space` feed. `refresh`
/// bypasses the 6h freshness window (the explicit Refresh action); otherwise a
/// fresh cache costs no network call. `source` says where the list came from.
#[tauri::command]
pub fn search_instances(refresh: bool) -> Result<serde_json::Value, String> {
    let (instances, source) = agentcowork_core::search_config::discover_instances(refresh)
        .map_err(|e| format!("instance feed: {e}"))?;
    Ok(serde_json::json!({
        "source": source,
        "count": instances.len(),
        "instances": instances,
    }))
}

/// Turn public instances on/off. Enabling requires a non-empty eligible list —
/// we never write an opt-in that points at nothing — and applies to the live
/// relay immediately when one is attached (the persisted config covers the
/// next boot either way).
#[tauri::command]
pub fn search_instances_apply(
    state: State<'_, AppState>,
    use_public: bool,
) -> Result<serde_json::Value, String> {
    let mut config = agentcowork_core::search_config::load();
    let mut discovered = 0usize;
    let mut source: Option<agentcowork_core::search_config::FeedSource> = None;
    if use_public {
        let (instances, src) = agentcowork_core::search_config::discover_instances(false)
            .map_err(|e| format!("cannot enable public instances: {e}"))?;
        let urls: Vec<String> = instances
            .iter()
            .map(|i| i.url.trim_end_matches('/').to_string())
            .collect();
        if urls.is_empty() {
            return Err("no eligible public instance in the feed — nothing enabled".into());
        }
        discovered = urls.len();
        source = Some(src);
        config.public_endpoints = urls;
    }
    config.use_public_instances = use_public;
    agentcowork_core::search_config::save(&config)?;
    let endpoints = agentcowork_core::search_config::resolved_endpoints_for(&config);

    // Apply to the live executor when the sidecar relay exists. When it does
    // not, the persisted config is what the next `ToolService` construction
    // reads — the response says which of the two happened.
    let mut applied_live = false;
    if let Ok(relay) = state.chat_relay.lock() {
        if let Some(r) = relay.as_ref() {
            if let Ok(mut tools) = r.tools().lock() {
                tools.set_search_endpoints(endpoints.clone());
                applied_live = true;
            }
        }
    }
    Ok(serde_json::json!({
        "usePublic": use_public,
        "endpoints": endpoints,
        "discovered": discovered,
        "source": source,
        "appliedLive": applied_live,
    }))
}
