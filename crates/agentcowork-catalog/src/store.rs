//! P56.1 — durable storage for the live models.dev snapshot.
//!
//! Two files under `<data_dir>/catalog/`:
//!
//! * `models.dev.json` — the parsed [`CatalogSnapshot`] (providers + models).
//!   This is what the Settings → Providers surface and the broker's endpoint
//!   resolution read.
//! * `models.dev.meta.json` — `etag` · `fetched_at` · counts · the last
//!   refresh verdict. Cheap to read, so `catalog_status` and the 4h
//!   staleness check never parse the 4.6 MB snapshot.
//!
//! Writes are atomic (`.tmp` + rename) and the snapshot is written **before**
//! the meta, so a crash between the two leaves a valid snapshot with a stale
//! meta — which costs one extra full fetch, never a corrupt catalog.
//!
//! The store takes an explicit directory: it has no opinion about where the
//! app keeps its data (`agentcowork_core::default_data_dir()` is the shell's
//! business), which also makes every path here testable without a socket or
//! a real install.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::live::{CatalogSnapshot, RefreshDecision};

/// Snapshot file name inside the catalog directory.
pub const SNAPSHOT_FILE: &str = "models.dev.json";
/// Meta file name inside the catalog directory.
pub const META_FILE: &str = "models.dev.meta.json";
/// User-tunable catalog settings (P56.1 refresh cadence).
pub const SETTINGS_FILE: &str = "settings.json";

/// Catalog settings the Settings UI can change (currently the cadence).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CatalogSettings {
    /// Requested refresh cadence in hours; clamped to 1–24 on use.
    pub refresh_hours: Option<u64>,
}

/// Cheap status half of the store (never requires parsing the snapshot).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CatalogMeta {
    pub source: String,
    /// The ETag to send as `If-None-Match` on the next refresh.
    pub etag: String,
    /// Unix ms of the last successful (200 **or** 304) refresh.
    pub fetched_at: i64,
    pub providers: usize,
    pub models: usize,
    /// Human-readable verdict of the last attempt (honest, including
    /// rejections/failures — never only successes).
    pub last_decision: Option<String>,
    /// True when the last attempt did not produce a new snapshot.
    pub last_failed: bool,
}

impl CatalogMeta {
    pub fn from_snapshot(snapshot: &CatalogSnapshot, decision: &RefreshDecision) -> Self {
        Self {
            source: snapshot.source.clone(),
            etag: snapshot.etag.clone(),
            fetched_at: snapshot.fetched_at,
            providers: snapshot.provider_count(),
            models: snapshot.model_count(),
            last_decision: Some(decision.summary()),
            last_failed: !decision.accepted(),
        }
    }

    /// `If-None-Match` value, or `None` when nothing is stored yet.
    pub fn if_none_match(&self) -> Option<&str> {
        let e = self.etag.trim();
        if e.is_empty() { None } else { Some(e) }
    }
}

/// The catalog directory handle.
#[derive(Debug, Clone)]
pub struct CatalogStore {
    dir: PathBuf,
}

impl CatalogStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn snapshot_path(&self) -> PathBuf {
        self.dir.join(SNAPSHOT_FILE)
    }

    pub fn meta_path(&self) -> PathBuf {
        self.dir.join(META_FILE)
    }

    pub fn settings_path(&self) -> PathBuf {
        self.dir.join(SETTINGS_FILE)
    }

    /// The persisted settings (empty when absent/unreadable → defaults).
    pub fn load_settings(&self) -> CatalogSettings {
        std::fs::read_to_string(self.settings_path())
            .ok()
            .and_then(|b| serde_json::from_str(&b).ok())
            .unwrap_or_default()
    }

    /// The effective refresh cadence in seconds (clamped 1–24h).
    pub fn refresh_interval_secs(&self) -> u64 {
        crate::live::refresh_interval_secs(self.load_settings().refresh_hours)
    }

    pub fn save_settings(&self, settings: &CatalogSettings) -> Result<(), String> {
        let body = serde_json::to_string_pretty(settings)
            .map_err(|e| format!("catalog store: serialize settings: {e}"))?;
        write_atomic(&self.settings_path(), &body)
    }

    /// Load the parsed snapshot, or `None` when absent/unreadable. An
    /// unreadable snapshot is treated as "no snapshot" (the next refresh
    /// refetches) — never a partial parse.
    pub fn load(&self) -> Option<CatalogSnapshot> {
        let body = std::fs::read_to_string(self.snapshot_path()).ok()?;
        serde_json::from_str(&body).ok()
    }

    /// Load the cheap meta, or `None` when absent/unreadable.
    pub fn load_meta(&self) -> Option<CatalogMeta> {
        let body = std::fs::read_to_string(self.meta_path()).ok()?;
        serde_json::from_str(&body).ok()
    }

    /// Persist a snapshot + its meta. Snapshot first, meta second (see the
    /// module note on crash ordering).
    pub fn save(
        &self,
        snapshot: &CatalogSnapshot,
        decision: &RefreshDecision,
    ) -> Result<CatalogMeta, String> {
        let meta = CatalogMeta::from_snapshot(snapshot, decision);
        let body = serde_json::to_string(snapshot)
            .map_err(|e| format!("catalog store: serialize snapshot: {e}"))?;
        write_atomic(&self.snapshot_path(), &body)?;
        self.save_meta(&meta)?;
        Ok(meta)
    }

    /// Persist only the meta (used by the 304 path, where the snapshot bytes
    /// are unchanged but `fetched_at`/verdict must move forward).
    pub fn save_meta(&self, meta: &CatalogMeta) -> Result<(), String> {
        let body = serde_json::to_string_pretty(meta)
            .map_err(|e| format!("catalog store: serialize meta: {e}"))?;
        write_atomic(&self.meta_path(), &body)
    }

    /// Remove the cached snapshot + meta (Settings → clear cached catalog).
    /// Settings are deliberately kept — the user's cadence is not cache.
    pub fn clear(&self) -> Result<(), String> {
        for p in [self.snapshot_path(), self.meta_path()] {
            match std::fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("catalog store: remove {}: {e}", p.display())),
            }
        }
        Ok(())
    }
}

/// Write `body` to `path` atomically (`path.tmp` + rename), creating the
/// parent directory first.
fn write_atomic(path: &Path, body: &str) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("catalog store: {} has no parent", path.display()))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("catalog store: create {}: {e}", dir.display()))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body)
        .map_err(|e| format!("catalog store: write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("catalog store: rename {}: {e}", tmp.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::FetchOutcome;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "agentcowork-catalog-store-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn snapshot() -> CatalogSnapshot {
        let body = r#"{"anthropic":{"id":"anthropic","name":"Anthropic","npm":"@ai-sdk/anthropic","env":["ANTHROPIC_API_KEY"],"models":{"claude-x":{"id":"claude-x","name":"Claude X","limit":{"context":200000,"output":128000},"cost":{"input":3,"output":15}}}}}"#;
        CatalogSnapshot::parse("https://models.dev/api.json", body, 1234, "\"e1\"").unwrap()
    }

    #[test]
    fn save_then_load_round_trips_snapshot_and_meta() {
        let d = dir("roundtrip");
        let store = CatalogStore::new(&d);
        let snap = snapshot();
        let decision = crate::live::RefreshDecision::Updated {
            providers: 1,
            models: 1,
            fetched_at: 1234,
        };
        let meta = store.save(&snap, &decision).unwrap();
        assert_eq!(meta.etag, "e1");
        assert_eq!(meta.fetched_at, 1234);
        assert_eq!(meta.providers, 1);
        assert_eq!(meta.models, 1);
        assert!(!meta.last_failed);
        assert_eq!(meta.if_none_match(), Some("e1"));
        assert_eq!(store.load().unwrap(), snap);
        assert_eq!(store.load_meta().unwrap(), meta);
        // tidy
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_files_are_none_not_an_error() {
        let d = dir("missing");
        let store = CatalogStore::new(&d);
        assert!(store.load().is_none());
        assert!(store.load_meta().is_none());
        assert_eq!(store.load_meta().unwrap_or_default().if_none_match(), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_snapshot_reads_as_absent() {
        let d = dir("corrupt");
        let store = CatalogStore::new(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(store.snapshot_path(), "not json").unwrap();
        assert!(store.load().is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn meta_records_failed_attempts_honestly() {
        let d = dir("failed");
        let store = CatalogStore::new(&d);
        let snap = snapshot();
        let (_, decision) =
            crate::live::apply_refresh(Some(&snap), FetchOutcome::Failed("offline".into()), 999);
        let meta = CatalogMeta::from_snapshot(&snap, &decision);
        assert!(meta.last_failed);
        assert!(meta.last_decision.unwrap().contains("offline"));
        // the snapshot itself is untouched by a failed attempt
        store.save(&snap, &decision).unwrap();
        assert_eq!(store.load().unwrap(), snap);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn clear_removes_both_files_and_is_idempotent() {
        let d = dir("clear");
        let store = CatalogStore::new(&d);
        let decision = crate::live::RefreshDecision::Updated {
            providers: 1,
            models: 1,
            fetched_at: 1,
        };
        store.save(&snapshot(), &decision).unwrap();
        assert!(store.snapshot_path().exists());
        store.clear().unwrap();
        assert!(!store.snapshot_path().exists());
        assert!(!store.meta_path().exists());
        store.clear().unwrap(); // idempotent
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn settings_default_to_four_hours_and_clamp() {
        let d = dir("settings");
        let store = CatalogStore::new(&d);
        assert_eq!(store.refresh_interval_secs(), 4 * 3600);
        store
            .save_settings(&CatalogSettings {
                refresh_hours: Some(99),
            })
            .unwrap();
        assert_eq!(store.refresh_interval_secs(), 24 * 3600);
        store
            .save_settings(&CatalogSettings {
                refresh_hours: Some(0),
            })
            .unwrap();
        assert_eq!(store.refresh_interval_secs(), 3600);
        assert_eq!(store.load_settings().refresh_hours, Some(0));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn clear_keeps_settings() {
        let d = dir("clear-settings");
        let store = CatalogStore::new(&d);
        store
            .save_settings(&CatalogSettings {
                refresh_hours: Some(6),
            })
            .unwrap();
        let decision = crate::live::RefreshDecision::Updated {
            providers: 1,
            models: 1,
            fetched_at: 1,
        };
        store.save(&snapshot(), &decision).unwrap();
        store.clear().unwrap();
        assert!(store.settings_path().exists());
        assert_eq!(store.refresh_interval_secs(), 6 * 3600);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn atomic_write_leaves_no_tmp_behind() {
        let d = dir("atomic");
        let store = CatalogStore::new(&d);
        let decision = crate::live::RefreshDecision::Updated {
            providers: 1,
            models: 1,
            fetched_at: 1,
        };
        store.save(&snapshot(), &decision).unwrap();
        assert!(!store.snapshot_path().with_extension("tmp").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
