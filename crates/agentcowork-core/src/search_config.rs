//! P55.8 — the search-engine configuration owner.
//!
//! The G8 cascade is **local-first**: your own SearXNG on the default ports,
//! then the DDG fallback. Public `searx.space` instances are never used unless
//! the user turns them on here, because routing a query through a stranger's
//! server is a privacy decision we refuse to make on their behalf.
//!
//! One file is the source of truth (`<data_dir>/search.json`), read both at
//! `ToolService` construction and by the Settings toggle, so the live cascade
//! and the surface can never disagree about what is in use.
//!
//! Every failure is honest: a missing/malformed config is the local-only
//! default, an unreachable feed is an error (or the last known good list marked
//! `stale`), and nothing here invents an endpoint.

use std::path::PathBuf;
use std::sync::OnceLock;

use agentcowork_search::searx_space::{FeedError, InstanceFeedTransport, SearxSpaceFeed};

// Re-exported so the shell's command layer names one path for the feed types.
pub use agentcowork_search::searx_space::{
    FeedError as InstanceFeedError, FeedSource, SearxInstance,
};

/// The local-first endpoint list (one owner: `agentcowork-search`).
pub const LOCAL_ENDPOINTS: &[&str] = agentcowork_search::DEFAULT_SEARX_ENDPOINTS;

/// The persisted search configuration.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchConfig {
    /// Off by default. On = the resolved list also carries the public
    /// instances below (local SearXNG still wins first).
    #[serde(default)]
    pub use_public_instances: bool,
    /// The public instance URLs last written when the user opted in. Kept even
    /// while switched off, so re-enabling works offline from the cache.
    #[serde(default)]
    pub public_endpoints: Vec<String>,
}

/// `<data_dir>/search.json`.
pub fn config_path() -> PathBuf {
    crate::default_data_dir().join("search.json")
}

/// Load the config. Missing or malformed ⇒ the local-only default (never a
/// silent opt-in to public egress).
pub fn load() -> SearchConfig {
    let Ok(bytes) = std::fs::read(config_path()) else {
        return SearchConfig::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Persist the config atomically.
pub fn save(config: &SearchConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create data dir: {e}"))?;
    }
    let json = serde_json::to_vec_pretty(config).map_err(|e| format!("encode: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(|e| format!("write: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename: {e}"))
}

/// The endpoint list the cascade should use: the local endpoints first, then
/// the opted-in public instances (deduped, local wins a collision).
pub fn resolved_endpoints() -> Vec<String> {
    resolved_endpoints_for(&load())
}

pub fn resolved_endpoints_for(config: &SearchConfig) -> Vec<String> {
    let mut endpoints: Vec<String> = LOCAL_ENDPOINTS.iter().map(|s| s.to_string()).collect();
    if config.use_public_instances {
        for url in &config.public_endpoints {
            let url = url.trim_end_matches('/').to_string();
            if !url.is_empty() && !endpoints.iter().any(|e| e.trim_end_matches('/') == url) {
                endpoints.push(url);
            }
        }
    }
    endpoints
}

/// Shorthand used by `ToolService` construction.
pub fn search_endpoints_from_config() -> Vec<String> {
    resolved_endpoints()
}

/// The live feed transport: one `ureq` GET of the `searx.space` JSON document.
///
/// FIX-09: the feed destination is pre-flighted through
/// [`agentcowork_guard::netfloor::preflight_url`] immediately before the socket,
/// so a Settings refresh can never be steered at link-local/cloud-metadata or
/// LAN space. The feed URL is a shipped constant, but the seam is
/// caller-supplied, so the check lives at the client rather than at the one
/// call site.
pub struct UreqInstanceFeed;

impl InstanceFeedTransport for UreqInstanceFeed {
    fn get_json(&self, url: &str) -> Result<serde_json::Value, String> {
        agentcowork_guard::netfloor::preflight_url(url, agentcowork_guard::NetPolicy::default())
            .map_err(|e| e.to_string())?;
        let body = ureq::get(url)
            .timeout(std::time::Duration::from_secs(15))
            .call()
            .map_err(|e| e.to_string())?
            .into_string()
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&body).map_err(|e| e.to_string())
    }
}

/// The process-wide feed cache (6 h) — a Settings visit cannot trigger a
/// network fetch on every render.
pub fn feed() -> &'static SearxSpaceFeed {
    static FEED: OnceLock<SearxSpaceFeed> = OnceLock::new();
    FEED.get_or_init(SearxSpaceFeed::default)
}

/// Discover public instances. `force` bypasses the freshness window (the
/// explicit Refresh button); otherwise a fresh cache costs no network call.
pub fn discover_instances(force: bool) -> Result<(Vec<SearxInstance>, FeedSource), FeedError> {
    let feed = feed();
    let transport = UreqInstanceFeed;
    if force {
        feed.refresh(&transport)
    } else {
        feed.instances(&transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(use_public: bool, urls: &[&str]) -> SearchConfig {
        SearchConfig {
            use_public_instances: use_public,
            public_endpoints: urls.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// The default is local-only: no public egress without an explicit opt-in.
    #[test]
    fn default_is_local_only() {
        let resolved = resolved_endpoints_for(&SearchConfig::default());
        assert_eq!(resolved, LOCAL_ENDPOINTS);
        assert!(!SearchConfig::default().use_public_instances);
    }

    /// Opted in ⇒ local endpoints still win, the public ones follow, and a
    /// duplicate of a local endpoint is not repeated.
    #[test]
    fn opt_in_appends_public_after_local_and_dedupes() {
        let resolved =
            resolved_endpoints_for(&cfg(true, &["https://a.example/", "http://localhost:8080"]));
        assert_eq!(resolved[0], LOCAL_ENDPOINTS[0]);
        assert_eq!(resolved[1], LOCAL_ENDPOINTS[1]);
        assert_eq!(resolved[2], "https://a.example");
        assert_eq!(resolved.len(), 3, "the localhost duplicate is not repeated");
    }

    /// Switching off keeps the stored list for later but stops using it.
    #[test]
    fn off_ignores_stored_public_endpoints() {
        let stored = cfg(false, &["https://a.example"]);
        assert_eq!(resolved_endpoints_for(&stored), LOCAL_ENDPOINTS);
        assert_eq!(stored.public_endpoints.len(), 1);
    }

    /// A malformed persisted file degrades to local-only, never to a partially
    /// trusted public list.
    #[test]
    fn malformed_config_is_local_only() {
        let parsed: Option<SearchConfig> = serde_json::from_str("{ not json").ok();
        assert!(parsed.is_none());
        assert_eq!(
            resolved_endpoints_for(&SearchConfig::default()),
            LOCAL_ENDPOINTS
        );
    }
}
