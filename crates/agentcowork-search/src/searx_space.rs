//! P55.8 — the `searx.space` instance feed client (G1/G8).
//!
//! The G8 cascade ships with a **localhost-first** endpoint list: your own
//! SearXNG, then the DDG fallback. Neither discovers anything. This module is
//! the missing half — a client for the public instance feed that searx.space
//! publishes and refreshes continuously:
//!
//! ```text
//! GET https://searx.space/data/instances.json
//! { "metadata": { … },
//!   "instances": {
//!     "https://host/": { "network_type": "normal",
//!                        "version": "2026.9.11+61d660276",
//!                        "http": { "status_code": 200, "error": null },
//!                        "timing": { "initial": { "all": { "value": 0.378 } },
//!                                    "search": { "success_percentage": 100 } },
//!                        "error": null } } }
//! ```
//!
//! Everything here is pure logic over an injected [`InstanceFeedTransport`]:
//! parsing, eligibility filtering, latency ordering and the cache are fully
//! testable offline. The live HTTP client lives with the other `ureq`
//! transports (`agentcowork-core`), so this crate keeps its no-runtime-dependency
//! posture.
//!
//! **Privacy is explicit.** Nothing in this module adds a public instance to
//! the cascade; it only reports what is available. Public egress stays an
//! opt-in decision made by the caller (Settings → Browser & Network), because
//! searching the public web through a stranger's server is exactly the kind of
//! default this product refuses to pick for the user.
//!
//! **Honesty rules.** A feed entry is never invented and never softened: an
//! instance with a failing HTTP probe, a Tor-only network type, a parity-less
//! or zero-success rate, or no measured latency is dropped rather than shown as
//! healthy. A failed refresh falls back to the last good cache and says so; a
//! cold failure is an error, never an empty "success".

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The upstream feed endpoint (verified live 2026-09-12).
pub const INSTANCES_FEED_URL: &str = "https://searx.space/data/instances.json";

/// Default freshness window for the cached instance list.
pub const FEED_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// The injected JSON transport — the live implementation is `ureq`.
pub trait InstanceFeedTransport: Send + Sync {
    fn get_json(&self, url: &str) -> Result<serde_json::Value, String>;
}

/// One eligible public SearXNG instance.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SearxInstance {
    /// Instance base URL, exactly as the feed keys it (trailing slash kept).
    pub url: String,
    /// Advertised version string (proves it is SearXNG, not a dead host).
    pub version: String,
    /// Median initial-response seconds from the feed's own probe.
    pub median_seconds: Option<f64>,
    /// Search success percentage from the feed's own probe.
    pub search_success_percentage: Option<f64>,
}

/// Why the feed could not be used.
#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    #[error("instance feed unavailable: {0}")]
    Transport(String),
    #[error("instance feed malformed: {0}")]
    Malformed(String),
    #[error("no eligible instance in the feed (all offline, Tor-only, or failing)")]
    Empty,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    instances: Vec<SearxInstance>,
    at: Instant,
}

/// The cached `searx.space` instance feed.
#[derive(Debug)]
pub struct SearxSpaceFeed {
    cache: Mutex<Option<CacheEntry>>,
    ttl: Duration,
    endpoint: String,
}

impl Default for SearxSpaceFeed {
    fn default() -> Self {
        Self::new(FEED_TTL)
    }
}

/// Where a returned instance list came from — never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedSource {
    /// Fetched from the network in this call.
    Live,
    /// Served from the cache, still inside the freshness window.
    FreshCache,
    /// The network failed; this is the last known good list.
    StaleCache,
}

impl SearxSpaceFeed {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Mutex::new(None),
            ttl,
            endpoint: INSTANCES_FEED_URL.to_string(),
        }
    }

    /// Override the feed URL (tests / mirrors).
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Whether the cache is present and still fresh.
    pub fn is_fresh(&self) -> bool {
        match self.cache.lock() {
            Ok(guard) => match guard.as_ref() {
                Some(entry) => entry.at.elapsed() < self.ttl,
                None => false,
            },
            Err(_) => false,
        }
    }

    /// The last known good list, regardless of age (no network).
    pub fn cached(&self) -> Option<Vec<SearxInstance>> {
        self.cache
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|e| e.instances.clone()))
    }

    /// The instance list plus where it came from.
    ///
    /// Fresh cache ⇒ no network. Otherwise fetch: success refreshes the cache;
    /// failure serves the last good list as [`FeedSource::StaleCache`], or
    /// surfaces the transport/malformed error when there is nothing cached.
    pub fn instances(
        &self,
        transport: &dyn InstanceFeedTransport,
    ) -> Result<(Vec<SearxInstance>, FeedSource), FeedError> {
        if self.is_fresh() {
            if let Some(instances) = self.cached() {
                return Ok((instances, FeedSource::FreshCache));
            }
        }
        match transport.get_json(&self.endpoint) {
            Ok(json) => {
                let instances = parse_instances(&json)?;
                if let Ok(mut guard) = self.cache.lock() {
                    *guard = Some(CacheEntry {
                        instances: instances.clone(),
                        at: Instant::now(),
                    });
                }
                Ok((instances, FeedSource::Live))
            }
            Err(e) => match self.cached() {
                Some(instances) if !instances.is_empty() => Ok((instances, FeedSource::StaleCache)),
                _ => Err(FeedError::Transport(e)),
            },
        }
    }

    /// Force a fetch regardless of the freshness window (the explicit Refresh
    /// action). A successful fetch replaces the cache; a failure returns the
    /// last good list as stale, or the transport error when nothing is cached.
    pub fn refresh(
        &self,
        transport: &dyn InstanceFeedTransport,
    ) -> Result<(Vec<SearxInstance>, FeedSource), FeedError> {
        // Keep the previous entry (with its original timestamp) so a failed
        // forced refresh restores it instead of destroying the fallback.
        let previous: Option<CacheEntry> = self.cache.lock().ok().and_then(|g| g.clone());
        if let Ok(mut guard) = self.cache.lock() {
            *guard = None;
        }
        match self.instances(transport) {
            Ok(v) => Ok(v),
            Err(e) => {
                if let Some(entry) = previous {
                    let instances = entry.instances.clone();
                    if let Ok(mut guard) = self.cache.lock() {
                        *guard = Some(entry);
                    }
                    if !instances.is_empty() {
                        return Ok((instances, FeedSource::StaleCache));
                    }
                }
                Err(e)
            }
        }
    }

    /// Just the URLs, cheapest-first — the shape `G8Cascade` consumes.
    pub fn endpoint_list(
        &self,
        transport: &dyn InstanceFeedTransport,
    ) -> Result<(Vec<String>, FeedSource), FeedError> {
        let (instances, source) = self.instances(transport)?;
        let urls = instances
            .into_iter()
            .map(|i| i.url.trim_end_matches('/').to_string())
            .collect();
        Ok((urls, source))
    }
}

/// Parse the feed into eligible instances, cheapest measured latency first.
///
/// Eligibility (every clause is a *reason to drop*, never a benefit of the
/// doubt): `network_type == "normal"` (Tor-only instances cannot serve an
/// ordinary search), HTTP probe `status_code == 200` with no `http.error` and
/// no top-level `error`, a non-empty `version`, and a search probe that
/// actually returned results (`success_percentage > 0`).
pub fn parse_instances(json: &serde_json::Value) -> Result<Vec<SearxInstance>, FeedError> {
    let map = json
        .get("instances")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| FeedError::Malformed("no `instances` object".into()))?;

    let mut out: Vec<SearxInstance> = Vec::new();
    for (url, entry) in map {
        if !url.starts_with("http") {
            continue;
        }
        if entry.get("error").map(|e| !e.is_null()).unwrap_or(false) {
            continue;
        }
        let network_type = entry
            .get("network_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if network_type != "normal" {
            continue;
        }
        let http = entry.get("http");
        let status = http
            .and_then(|h| h.get("status_code"))
            .and_then(serde_json::Value::as_u64);
        if status != Some(200) {
            continue;
        }
        if http
            .and_then(|h| h.get("error"))
            .map(|e| !e.is_null())
            .unwrap_or(false)
        {
            continue;
        }
        let version = entry
            .get("version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if version.is_empty() {
            continue;
        }
        let timing = entry.get("timing");
        let search_success = timing
            .and_then(|t| t.get("search"))
            .and_then(|s| s.get("success_percentage"))
            .and_then(serde_json::Value::as_f64);
        if !matches!(search_success, Some(p) if p > 0.0) {
            continue;
        }
        let median_seconds = timing
            .and_then(|t| t.get("initial"))
            .and_then(|i| i.get("all"))
            .and_then(|a| a.get("value"))
            .and_then(serde_json::Value::as_f64);
        out.push(SearxInstance {
            url: url.clone(),
            version: version.to_string(),
            median_seconds,
            search_success_percentage: search_success,
        });
    }

    if out.is_empty() {
        return Err(FeedError::Empty);
    }
    // Cheapest measured latency first; an unmeasured instance sorts last rather
    // than being dropped (it may still be the only healthy one).
    out.sort_by(|a, b| {
        let key = |i: &SearxInstance| i.median_seconds.unwrap_or(f64::MAX);
        key(a)
            .partial_cmp(&key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.url.cmp(&b.url))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn instance(
        status: u64,
        network: &str,
        success: f64,
        median: Option<f64>,
    ) -> serde_json::Value {
        json!({
            "network_type": network,
            "version": "2026.9.11+61d660276",
            "http": { "status_code": status, "error": null },
            "timing": {
                "initial": { "all": median.map(|m| json!({ "value": m })) },
                "search": { "success_percentage": success },
            },
            "error": null,
        })
    }

    fn feed_json(entries: Vec<(&str, serde_json::Value)>) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (url, e) in entries {
            map.insert(url.to_string(), e);
        }
        json!({ "metadata": { "timestamp": 1 }, "instances": map })
    }

    /// The live feed's own field names drive parsing — a renamed field must
    /// break loudly rather than yield a silently empty instance list.
    #[test]
    fn parses_the_live_feed_shape() {
        let parsed = parse_instances(&feed_json(vec![(
            "https://anonsearch.win/",
            instance(200, "normal", 100.0, Some(0.378)),
        )]))
        .expect("one healthy instance");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].url, "https://anonsearch.win/");
        assert_eq!(parsed[0].median_seconds, Some(0.378));
        assert_eq!(parsed[0].search_success_percentage, Some(100.0));
    }

    #[test]
    fn drops_offline_tor_and_failing_instances() {
        let parsed = parse_instances(&feed_json(vec![
            ("https://ok/", instance(200, "normal", 100.0, Some(0.5))),
            ("https://down/", instance(503, "normal", 100.0, Some(0.1))),
            ("https://tor/", instance(200, "tor", 100.0, Some(0.1))),
            ("https://nohits/", instance(200, "normal", 0.0, Some(0.1))),
            (
                "https://olderrored/",
                json!({
                    "network_type": "normal",
                    "version": "2026.9.11",
                    "http": { "status_code": 200, "error": "timeout" },
                    "timing": { "initial": { "all": { "value": 0.2 } }, "search": { "success_percentage": 100 } },
                    "error": null,
                }),
            ),
            (
                "https://noversion/",
                json!({
                    "network_type": "normal",
                    "version": "",
                    "http": { "status_code": 200, "error": null },
                    "timing": { "initial": { "all": { "value": 0.2 } }, "search": { "success_percentage": 100 } },
                    "error": null,
                }),
            ),
        ]))
        .expect("one instance survives");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].url, "https://ok/");
    }

    #[test]
    fn orders_by_measured_latency_and_keeps_unmeasured_last() {
        let parsed = parse_instances(&feed_json(vec![
            ("https://slow/", instance(200, "normal", 100.0, Some(2.0))),
            ("https://fast/", instance(200, "normal", 100.0, Some(0.2))),
            ("https://unmeasured/", instance(200, "normal", 100.0, None)),
        ]))
        .unwrap();
        let urls: Vec<&str> = parsed.iter().map(|i| i.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://fast/", "https://slow/", "https://unmeasured/"]
        );
    }

    #[test]
    fn an_all_bad_feed_is_an_error_never_an_empty_success() {
        let err = parse_instances(&feed_json(vec![(
            "https://down/",
            instance(500, "normal", 0.0, None),
        )]))
        .unwrap_err();
        assert!(matches!(err, FeedError::Empty));

        let malformed = parse_instances(&json!({ "metadata": {} })).unwrap_err();
        assert!(matches!(malformed, FeedError::Malformed(_)));
    }

    struct Scripted {
        calls: Mutex<u32>,
        reply: Result<serde_json::Value, String>,
    }

    impl Scripted {
        fn ok(v: serde_json::Value) -> Self {
            Self {
                calls: Mutex::new(0),
                reply: Ok(v),
            }
        }
        fn err(msg: &str) -> Self {
            Self {
                calls: Mutex::new(0),
                reply: Err(msg.to_string()),
            }
        }
        fn calls(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    impl InstanceFeedTransport for Scripted {
        fn get_json(&self, _url: &str) -> Result<serde_json::Value, String> {
            *self.calls.lock().unwrap() += 1;
            self.reply.clone()
        }
    }

    #[test]
    fn a_fresh_cache_costs_no_network_call() {
        let feed = SearxSpaceFeed::default();
        let t = Scripted::ok(feed_json(vec![(
            "https://ok/",
            instance(200, "normal", 100.0, Some(0.3)),
        )]));
        let (first, src) = feed.instances(&t).unwrap();
        assert_eq!(src, FeedSource::Live);
        assert_eq!(first.len(), 1);

        let (second, src2) = feed.instances(&t).unwrap();
        assert_eq!(src2, FeedSource::FreshCache);
        assert_eq!(second, first);
        assert_eq!(t.calls(), 1, "the second read must come from the cache");
    }

    #[test]
    fn a_failed_refresh_serves_the_last_good_list_as_stale() {
        // Seed the cache live, then read past the TTL with a dead network.
        let feed = SearxSpaceFeed::new(Duration::ZERO);
        let good = Scripted::ok(feed_json(vec![(
            "https://ok/",
            instance(200, "normal", 100.0, Some(0.3)),
        )]));
        feed.instances(&good).unwrap();

        let dead = Scripted::err("connection refused");
        let (instances, src) = feed.instances(&dead).unwrap();
        assert_eq!(src, FeedSource::StaleCache);
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].url, "https://ok/");
    }

    #[test]
    fn a_cold_failure_is_an_error_not_an_empty_list() {
        let feed = SearxSpaceFeed::default();
        let dead = Scripted::err("dns failure");
        let err = feed.instances(&dead).unwrap_err();
        assert!(matches!(err, FeedError::Transport(_)));
        assert!(feed.cached().is_none());
    }

    #[test]
    fn a_failed_forced_refresh_restores_the_previous_list() {
        let feed = SearxSpaceFeed::default();
        let good = Scripted::ok(feed_json(vec![(
            "https://ok/",
            instance(200, "normal", 100.0, Some(0.3)),
        )]));
        feed.instances(&good).unwrap();

        let dead = Scripted::err("offline");
        let (instances, src) = feed.refresh(&dead).unwrap();
        assert_eq!(src, FeedSource::StaleCache);
        assert_eq!(instances.len(), 1);
        // …and the restored entry is readable again afterwards.
        assert_eq!(feed.cached().unwrap().len(), 1);
    }

    #[test]
    fn endpoint_list_strips_trailing_slashes_for_the_cascade() {
        let feed = SearxSpaceFeed::default();
        let t = Scripted::ok(feed_json(vec![
            ("https://a/", instance(200, "normal", 100.0, Some(0.2))),
            ("https://b/", instance(200, "normal", 100.0, Some(0.4))),
        ]));
        let (urls, _) = feed.endpoint_list(&t).unwrap();
        assert_eq!(urls, vec!["https://a", "https://b"]);
    }
}
