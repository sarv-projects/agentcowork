//! P11.5.9 (follow-on) — the RepoMap SQLite cache.
//!
//! `repomap_build` is deterministic: the same (file set, content) always
//! produces the same ranked rows. This module caches that result in SQLite,
//! keyed by a content-hash of every file that fed the map, so a second build
//! of an unchanged tree is a single indexed lookup (no tag extraction, no
//! PageRank). The cache is a pure accelerator — a miss recomputes and
//! re-stores; corruption or schema drift falls back to a full rebuild.

use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::Path;

/// One cached repo-map row (mirrors the serializable command row).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedRow {
    pub symbol: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub rank: f64,
}

/// Content-hash the (file, content) pairs that feed a map — the cache key.
/// A single byte change anywhere invalidates the entry (merkle-style: each
/// file hashes with its path, files hash in sorted order).
pub fn map_hash(files: &[(String, String)]) -> String {
    let mut h = Sha256::new();
    let mut sorted: Vec<&(String, String)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (file, content) in sorted {
        h.update(file.as_bytes());
        h.update([0u8]);
        h.update(content.as_bytes());
        h.update([0u8]);
    }
    format!("{:x}", h.finalize())
}

/// SQLite-backed repo-map cache. Not thread-safe by design (the command is
/// synchronous); open one per build.
pub struct RepoMapCache {
    conn: Connection,
}

impl RepoMapCache {
    /// Open (or create) the cache at `path`; missing parent dirs are created.
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        // P45.1/.3 — WAL + synchronous=NORMAL + bounded WAL (non-crypto;
        // the vault keeps its safer FULL setting).
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA journal_size_limit=67108864;
             PRAGMA wal_autocheckpoint=4000;",
        )
        .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS repo_maps (
                hash TEXT PRIMARY KEY,
                rows_json TEXT NOT NULL,
                built_ms INTEGER NOT NULL
            );",
        )
        .map_err(|e| e.to_string())?;
        Ok(Self { conn })
    }

    /// Look up a cached map by content hash; `None` on miss (or corruption).
    pub fn get(&self, hash: &str) -> Option<Vec<CachedRow>> {
        let row: Option<String> = self
            .conn
            .query_row(
                "SELECT rows_json FROM repo_maps WHERE hash = ?1",
                [hash],
                |r| r.get(0),
            )
            .optional()
            .ok()?;
        row.and_then(|json| serde_json::from_str(&json).ok())
    }

    /// Store a map under its content hash (INSERT OR REPLACE).
    pub fn put(&self, hash: &str, rows: &[CachedRow]) -> Result<(), String> {
        let json = serde_json::to_string(rows).map_err(|e| e.to_string())?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO repo_maps (hash, rows_json, built_ms)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![hash, json, chrono_now_ms()],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Purge entries older than `max_age_ms` (maintenance; default retention
    /// keeps the cache bounded). Returns the number of rows removed.
    pub fn prune(&self, max_age_ms: i64) -> Result<usize, String> {
        let now = chrono_now_ms();
        let cutoff = now - max_age_ms;
        self.conn
            .execute("DELETE FROM repo_maps WHERE built_ms < ?1", [cutoff])
            .map_err(|e| e.to_string())
    }
}

fn chrono_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentcowork-repocache-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("repo_cache.db")
    }

    #[test]
    fn round_trip_hit_and_miss() {
        let db = temp_db();
        let cache = RepoMapCache::open(&db).unwrap();
        assert!(cache.get("nope").is_none());
        let rows = vec![CachedRow {
            symbol: "main".into(),
            kind: "fn".into(),
            file: "src/main.rs".into(),
            line: 1,
            rank: 0.9,
        }];
        cache.put("abc", &rows).unwrap();
        let got = cache.get("abc").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].symbol, "main");
        assert_eq!(got[0].rank, 0.9);
    }

    #[test]
    fn content_hash_changes_with_any_file_edit() {
        let a = map_hash(&[("a.rs".into(), "fn a() {}".into())]);
        let b = map_hash(&[("a.rs".into(), "fn a() { x }".into())]);
        assert_ne!(a, b);
        // Same content, different order → same hash (deterministic).
        let c = map_hash(&[
            ("b.rs".into(), "fn b() {}".into()),
            ("a.rs".into(), "fn a() {}".into()),
        ]);
        let d = map_hash(&[
            ("a.rs".into(), "fn a() {}".into()),
            ("b.rs".into(), "fn b() {}".into()),
        ]);
        assert_eq!(c, d);
    }

    #[test]
    fn prune_removes_only_expired() {
        let db = temp_db();
        let cache = RepoMapCache::open(&db).unwrap();
        cache.put("fresh", &[]).unwrap();
        // A negative age makes every row "expired".
        let removed = cache.prune(-1).unwrap();
        assert_eq!(removed, 1);
        assert!(cache.get("fresh").is_none());
    }
}
