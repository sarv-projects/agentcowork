//! F8 — fetch + cache the **official ACP agent registry** (doc 57 §2).
//!
//! [`RegistryClient`] fetches `registry.json` from the CDN (pluggable
//! [`Fetch`] transport — `ureq` in production, mock in tests), parses it into
//! a [`RegistryIndex`], and writes a **local cache** (the JSON + a meta file
//! with the fetch time) so an offline app still has the last-known catalog.
//! The cache is version-pinned by the registry's own `version` field.

use crate::registry_index::RegistryIndex;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// The canonical CDN endpoint (doc 57 §2, doc 69 §1).
pub const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("http error: {0}")]
    Http(String),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("registry JSON parse error: {0}")]
    Parse(String),
}

/// A minimal HTTP GET transport (mocked in tests).
pub trait Fetch {
    fn get(&self, url: &str) -> Result<String, FetchError>;
}

/// The production transport (`ureq` — already a workspace dep).
pub struct UreqFetch;

impl Fetch for UreqFetch {
    fn get(&self, url: &str) -> Result<String, FetchError> {
        let resp = ureq::get(url)
            .timeout(std::time::Duration::from_secs(30))
            .call()
            .map_err(|e| FetchError::Http(e.to_string()))?;
        resp.into_string()
            .map_err(|e| FetchError::Http(e.to_string()))
    }
}

/// A fetched-and-cached registry snapshot.
#[derive(Debug, Clone)]
pub struct RegistrySnapshot {
    pub index: RegistryIndex,
    pub fetched_at_ms: u64,
    pub from_cache: bool,
}

/// Fetches, caches and loads the official registry into a cache directory
/// (default: `<data_dir>/agents`).
pub struct RegistryClient {
    fetch: Box<dyn Fetch + Send + Sync>,
    cache_dir: PathBuf,
}

impl RegistryClient {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            fetch: Box::new(UreqFetch),
            cache_dir,
        }
    }

    /// Test seam: inject a transport.
    pub fn with_fetch(mut self, fetch: impl Fetch + Send + Sync + 'static) -> Self {
        self.fetch = Box::new(fetch);
        self
    }

    fn json_path(&self) -> PathBuf {
        self.cache_dir.join("registry.json")
    }

    fn meta_path(&self) -> PathBuf {
        self.cache_dir.join("registry.meta.json")
    }

    /// Fetch the live registry, parse it, and write the cache. Returns the
    /// fresh snapshot (from_cache = false).
    pub fn refresh(&self) -> Result<RegistrySnapshot, FetchError> {
        let text = self.fetch.get(REGISTRY_URL)?;
        let index = RegistryIndex::parse(&text).map_err(|e| FetchError::Parse(e.to_string()))?;
        std::fs::create_dir_all(&self.cache_dir)?;
        std::fs::write(self.json_path(), &text)?;
        let fetched_at_ms = now_ms();
        let meta = serde_json::json!({ "fetchedAtMs": fetched_at_ms, "version": index.version });
        std::fs::write(
            self.meta_path(),
            serde_json::to_string(&meta).unwrap_or_default(),
        )?;
        Ok(RegistrySnapshot {
            index,
            fetched_at_ms,
            from_cache: false,
        })
    }

    /// Load the cached registry (no network). Returns `None` if never cached.
    pub fn load_cached(&self) -> Option<RegistrySnapshot> {
        let text = std::fs::read_to_string(self.json_path()).ok()?;
        let index = RegistryIndex::parse(&text).ok()?;
        let fetched_at_ms = std::fs::read_to_string(self.meta_path())
            .ok()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok())
            .and_then(|v| v.get("fetchedAtMs").and_then(|t| t.as_u64()))
            .unwrap_or(0);
        Some(RegistrySnapshot {
            index,
            fetched_at_ms,
            from_cache: true,
        })
    }

    /// Best-effort: cached if present, else fetch. Never fails the caller on
    /// network loss — returns `None` only if there is no catalog at all.
    pub fn load_or_refresh(&self) -> Option<RegistrySnapshot> {
        match self.load_cached() {
            Some(s) => Some(s),
            None => self.refresh().ok(),
        }
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockFetch {
        body: String,
        calls: AtomicUsize,
    }

    impl Fetch for MockFetch {
        fn get(&self, _url: &str) -> Result<String, FetchError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.body.clone())
        }
    }

    const FIXTURE: &str = r#"{"version":"1.0.0","agents":[]}"#;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("agentcowork-acp-reg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn refresh_fetches_parses_and_caches() {
        let dir = tmp_dir("refresh");
        let client = RegistryClient::new(dir.clone()).with_fetch(MockFetch {
            body: FIXTURE.into(),
            calls: AtomicUsize::new(0),
        });
        let snap = client.refresh().unwrap();
        assert!(!snap.from_cache);
        assert_eq!(snap.index.version, "1.0.0");
        assert!(dir.join("registry.json").exists());
        assert!(dir.join("registry.meta.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_cached_roundtrips_without_network() {
        let dir = tmp_dir("cached");
        let client = RegistryClient::new(dir.clone()).with_fetch(MockFetch {
            body: FIXTURE.into(),
            calls: AtomicUsize::new(0),
        });
        client.refresh().unwrap();
        let snap = client.load_cached().unwrap();
        assert!(snap.from_cache);
        assert_eq!(snap.index.version, "1.0.0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_refresh_falls_back_to_network() {
        let dir = tmp_dir("fallback");
        let client = RegistryClient::new(dir.clone()).with_fetch(MockFetch {
            body: FIXTURE.into(),
            calls: AtomicUsize::new(0),
        });
        // No cache → falls back to refresh (one fetch).
        let snap = client.load_or_refresh().unwrap();
        assert!(!snap.from_cache);
        // Now cached → served from cache (no second fetch).
        let snap2 = client.load_or_refresh().unwrap();
        assert!(snap2.from_cache);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_error_surfaces() {
        let client = RegistryClient::new(tmp_dir("bad")).with_fetch(MockFetch {
            body: "not json".into(),
            calls: AtomicUsize::new(0),
        });
        assert!(matches!(client.refresh(), Err(FetchError::Parse(_))));
    }
}
