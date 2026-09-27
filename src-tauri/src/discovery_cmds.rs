//! P44.7/44.8 — Discovery surface + routing-feed Tauri wiring.
//!
//! `discovery_inventory` aggregates every managed-resource class into one
//! surface: providers come from the catalog registry; MCP servers, skills,
//! local models, browsers and agents are collected from their live on-disk /
//! runtime sources. Auth is reported as a *shape*, never a secret — discovery
//! never harvests credentials.
//!
//! `routing_feed_decide` exposes the P44.8 provider-level route decision
//! (verified capabilities + health → ranked providers) so the UI can show
//! *why* a provider is or isn't a route candidate, and so the value is
//! test-observable end to end.

use std::path::{Path, PathBuf};

use agentcowork_catalog::{
    apply_observation_health, DiscoveryInventory, Health, ManagedResource, ResourceCard,
    ResourceKind, RouteRequirements, RoutingFeed,
};
use agentcowork_vault::KeyRing;
use tauri::State;

use crate::AppState;

/// Build the full Discover inventory. Providers from the registry; the other
/// classes from their live collectors (best-effort, honest empties).
#[tauri::command]
pub fn discovery_inventory() -> Result<serde_json::Value, String> {
    // The observed registry, so a card's `capabilities_verified` reflects what
    // a live probe actually confirmed rather than catalog metadata alone.
    let reg = crate::catalog_cmds::observed_registry();
    let mut inv = DiscoveryInventory::from_registry(&reg, 1);

    let data_dir = agentcowork_core::default_data_dir();
    inv.extend(collect_local_models());
    inv.extend(collect_installed_mcp(&data_dir));
    inv.extend(collect_installed_skills(&data_dir));
    inv.extend(collect_agents(&data_dir));
    inv.extend(collect_browsers());

    let counts = inv.counts();
    let cards: Vec<serde_json::Value> = inv
        .cards
        .into_iter()
        .map(|card| {
            let local_model = card.kind == ResourceKind::Model && card.source == "local_runtime";
            let mut value = serde_json::to_value(card).unwrap_or_else(|_| serde_json::json!({}));
            if local_model {
                value["lifecycle"] = serde_json::json!("observed");
            }
            value
        })
        .collect();
    Ok(serde_json::json!({
        "counts": counts,
        "cards": cards,
        "generation": inv.generation,
    }))
}

/// P44.8 — the provider-level route decision for a set of requirements. Loads
/// the observed registry into a fresh feed and ranks; health comes from the
/// recorded P44.4 observations, so a provider whose last probe never connected
/// is `Down` (and a provider that answered with a rejection is `Degraded`, not
/// `Down` — it is reachable). A provider with no observation stays `Unknown`,
/// never a guessed value.
///
/// P50.3.6 — the decision is **vault-credential gated**: only providers the
/// vault currently holds a key for may rank. A provider whose auth needs a
/// vault key but has none is excluded with an explicit "add a key" reason,
/// so the catalog alone can never make an unkeyed provider look usable.
#[tauri::command]
pub fn routing_feed_decide(
    state: State<'_, AppState>,
    requires_tools: Option<bool>,
    requires_structured_output: Option<bool>,
    requires_codex: Option<bool>,
) -> Result<serde_json::Value, String> {
    let reg = crate::catalog_cmds::observed_registry();
    let mut feed = RoutingFeed::new();
    feed.load_registry(&reg);
    // Unknown-by-default so the decision is meaningful in the absence of live
    // pings, then overridden per-id by whatever was actually observed.
    for p in reg.all() {
        feed.set_health(&p.id, Health::Unknown);
    }
    apply_observation_health(&mut feed, &crate::catalog_cmds::observation_file());
    // P50.3.6 — the live vault key set drives routability. Locked/empty vault
    // ⇒ empty set ⇒ every keyed provider excluded with the reason below;
    // keyless/local providers (no vault key needed) still rank.
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let ring = KeyRing::new(&vault);
    let credentialed: Vec<String> = ring.providers_with_keys().unwrap_or_default();
    feed.set_credentialed(credentialed);
    let decision = feed.decide(&RouteRequirements {
        requires_tools: requires_tools.unwrap_or(false),
        requires_structured_output: requires_structured_output.unwrap_or(false),
        requires_codex: requires_codex.unwrap_or(false),
    });
    serde_json::to_value(&decision).map_err(|e| e.to_string())
}

// --- live collectors (best-effort, never harvest secrets) ------------------

fn collect_local_models() -> Vec<ResourceCard> {
    // Wire `status` is this catalog projection; process `lifecycle` uses core `ManagedResource<R>` and never occupies this field.
    let cfg = agentcowork_core::Config::load().unwrap_or_default();
    let mgr = agentcowork_core::LocalManager::from_config(&cfg);
    mgr.list_ollama_models()
        .into_iter()
        .map(|m| ResourceCard {
            kind: ResourceKind::Model,
            id: format!("ollama/{}", m.name),
            name: m.name,
            version: String::new(),
            source: "local_runtime".into(),
            auth: "keyless".into(),
            capabilities: vec![format!("ctx:{}", m.context_window)],
            capabilities_verified: false,
            governance: "local".into(),
            base_url: String::new(),
            doc_url: String::new(),
            status: ManagedResource::Inventoried,
        })
        .collect()
}

fn collect_installed_mcp(data_dir: &Path) -> Vec<ResourceCard> {
    let dir = data_dir.join("mcp");
    std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| {
                    let id = e.file_name().to_string_lossy().into_owned();
                    ResourceCard {
                        kind: ResourceKind::Mcp,
                        id: id.clone(),
                        name: id,
                        version: String::new(),
                        source: "installed".into(),
                        auth: "none".into(),
                        capabilities: vec![],
                        capabilities_verified: false,
                        governance: String::new(),
                        base_url: String::new(),
                        doc_url: String::new(),
                        status: ManagedResource::Installed,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn collect_installed_skills(data_dir: &Path) -> Vec<ResourceCard> {
    let dir = data_dir.join("skills");
    std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| {
                    let id = e.file_name().to_string_lossy().into_owned();
                    ResourceCard {
                        kind: ResourceKind::Skill,
                        id: id.clone(),
                        name: id,
                        version: String::new(),
                        source: "store".into(),
                        auth: "none".into(),
                        capabilities: vec![],
                        capabilities_verified: false,
                        governance: String::new(),
                        base_url: String::new(),
                        doc_url: String::new(),
                        status: ManagedResource::Installed,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn collect_agents(data_dir: &Path) -> Vec<ResourceCard> {
    // The inbuilt agent is always present; on-disk bundles live under agents/.
    let mut cards = vec![ResourceCard {
        kind: ResourceKind::Agent,
        id: "inbuilt".into(),
        name: "AgentCowork".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        source: "builtin".into(),
        auth: "none".into(),
        capabilities: vec!["chat".into(), "tools".into(), "plan".into()],
        capabilities_verified: false,
        governance: "inbuilt".into(),
        base_url: String::new(),
        doc_url: String::new(),
        status: ManagedResource::Discovered,
    }];
    let dir = data_dir.join("agents");
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten().filter(|e| e.path().is_dir()) {
            let id = e.file_name().to_string_lossy().into_owned();
            cards.push(ResourceCard {
                kind: ResourceKind::Agent,
                id: id.clone(),
                name: id,
                version: String::new(),
                source: "bundle".into(),
                auth: "none".into(),
                capabilities: vec![],
                capabilities_verified: false,
                governance: "custom".into(),
                base_url: String::new(),
                doc_url: String::new(),
                status: ManagedResource::Installed,
            });
        }
    }
    cards
}

fn collect_browsers() -> Vec<ResourceCard> {
    // Dependency-light discovery of installed browsers (same probe order as
    // doctor). We only report presence — never a profile or a cookie.
    // PATH names first (Linux/macOS), then well-known absolute install paths
    // — Windows Chrome/Edge/Brave and macOS apps are NOT on PATH, so the
    // PATH probe alone would report zero browsers there.
    const CANDS: &[(&str, &[&str])] = &[
        (
            "chrome",
            &["google-chrome", "google-chrome-stable", "chrome"],
        ),
        ("chromium", &["chromium", "chromium-browser"]),
        ("edge", &["msedge", "microsoft-edge"]),
        ("brave", &["brave-browser"]),
    ];
    #[cfg(target_os = "windows")]
    const KNOWN_PATHS: &[(&str, &str)] = &[
        (
            "chrome",
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        ),
        (
            "chrome",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ),
        (
            "edge",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        ),
        (
            "edge",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        ),
        (
            "brave",
            r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
        ),
        (
            "brave",
            r"C:\Program Files (x86)\BraveSoftware\Brave-Browser\Application\brave.exe",
        ),
    ];
    #[cfg(target_os = "macos")]
    const KNOWN_PATHS: &[(&str, &str)] = &[
        (
            "chrome",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ),
        (
            "edge",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ),
        (
            "chromium",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ),
        (
            "brave",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        ),
    ];
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    const KNOWN_PATHS: &[(&str, &str)] = &[];

    let path = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };
    let dirs: Vec<&str> = path.split(sep).collect();
    let mut found_ids: Vec<&str> = Vec::new();
    for (id, names) in CANDS {
        let on_path = names.iter().any(|n| {
            dirs.iter().any(|d| {
                let mut p = PathBuf::from(d);
                p.push(n);
                p.is_file() || {
                    p.set_extension("exe");
                    p.is_file()
                }
            })
        });
        if on_path {
            found_ids.push(id);
        }
    }
    for (id, p) in KNOWN_PATHS {
        if PathBuf::from(p).is_file() && !found_ids.contains(id) {
            found_ids.push(id);
        }
    }
    found_ids
        .into_iter()
        .map(|id| ResourceCard {
            kind: ResourceKind::Browser,
            id: id.to_string(),
            name: id.to_string(),
            version: String::new(),
            source: "system".into(),
            auth: "none".into(),
            capabilities: vec!["cdp".into()],
            capabilities_verified: false,
            governance: String::new(),
            base_url: String::new(),
            doc_url: String::new(),
            status: ManagedResource::Discovered,
        })
        .collect()
}

/// P51.1 — live health ping for one provider base URL (`GET {base}/v1/models`,
/// 2s timeout). Returns `{ reachable }` — the drawer health-check trigger.
/// No credentials are sent; loopback and https only (fail-closed otherwise).
#[tauri::command]
pub fn provider_health_probe(url: String) -> Result<serde_json::Value, String> {
    let base = url.trim_end_matches('/');
    if !(base.starts_with("http://127.0.0.1")
        || base.starts_with("http://localhost")
        || base.starts_with("https://"))
    {
        return Err("probe refused: loopback or https only".to_string());
    }
    let reachable = agentcowork_core::models::probe::probe_openai_endpoint(base);
    Ok(serde_json::json!({ "url": base, "reachable": reachable }))
}
