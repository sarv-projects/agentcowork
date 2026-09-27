//! P36 (D10) — persistent hash cache + delta re-scan (fclones `--cache`
//! pattern). File hashes survive between scans; only files whose
//! (path, size, mtime) changed since the last pass get re-hashed — the
//! scanned delta, not the whole tree. SQLCipher-backed via rusqlite.

use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct CachedHash {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: u64,
    pub hash: String,
}

/// An open persistent hash cache for one scan root.
#[derive(Debug)]
pub struct HashCache {
    conn: Connection,
}

impl HashCache {
    /// Open (or create) the cache for `root`.
    pub fn open(root: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(root.join(".agentcowork-fclones.sqlite"))?;
        // P45.1/.3 — WAL + synchronous=NORMAL + bounded WAL (write-heavy
        // cache; no mmap — not read-mostly). Vault never goes through here.
        crate::pragmas::apply_non_vault(&conn)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS hash_cache (
                path TEXT PRIMARY KEY,
                size INTEGER NOT NULL,
                mtime_ms INTEGER NOT NULL,
                hash TEXT NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE hash_cache (
                path TEXT PRIMARY KEY,
                size INTEGER NOT NULL,
                mtime_ms INTEGER NOT NULL,
                hash TEXT NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    pub fn path_key(&self, p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    pub fn lookup(&self, p: &Path, size: u64, mtime_ms: u64) -> Option<String> {
        let key = self.path_key(p);
        self.conn
            .query_row(
                "SELECT hash FROM hash_cache WHERE path = ?1 AND size = ?2 AND mtime_ms = ?3",
                params![key, size, mtime_ms],
                |row| row.get(0),
            )
            .ok()
    }

    pub fn put(&mut self, p: &Path, size: u64, mtime_ms: u64, hash: &str) -> rusqlite::Result<()> {
        let key = self.path_key(p);
        self.conn.execute(
            "INSERT OR REPLACE INTO hash_cache (path, size, mtime_ms, hash) VALUES (?1, ?2, ?3, ?4)",
            params![key, size, mtime_ms, hash],
        )?;
        Ok(())
    }

    pub fn remove(&mut self, p: &Path) -> rusqlite::Result<()> {
        let key = self.path_key(p);
        self.conn
            .execute("DELETE FROM hash_cache WHERE path = ?1", params![key])?;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The size/mtime the cache knows for a path (for delta detection).
    pub fn known(&self, p: &Path) -> Option<(u64, u64)> {
        let key = self.path_key(p);
        self.conn
            .query_row(
                "SELECT size, mtime_ms FROM hash_cache WHERE path = ?1",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()
    }

    pub fn len(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM hash_cache", [], |r| r.get(0))
            .unwrap_or(0)
    }

    pub fn cleanup_removed(&mut self, existing: &[PathBuf]) -> rusqlite::Result<()> {
        let have: std::collections::HashSet<String> =
            existing.iter().map(|p| self.path_key(p)).collect();
        let stale: Vec<String> = {
            let mut stmt = self.conn.prepare("SELECT path FROM hash_cache")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok())
                .filter(|p| !have.contains(p))
                .collect()
        };
        for p in stale {
            self.conn
                .execute("DELETE FROM hash_cache WHERE path = ?1", params![p])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A directory unique **per test**. Keying on the process id alone made
    /// every test in this module share one path, so one test's
    /// `remove_dir_all` deleted another's file and the suite only passed when
    /// it happened to run single-threaded. The tag is that missing dimension
    /// (the same shape the other storage test helpers already use).
    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "agentcowork-hashcache-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn unchanged_file_is_cache_hit() {
        let root = tmp("unchanged");
        let f = root.join("a.txt");
        fs::write(&f, "hello").unwrap();
        let meta = fs::metadata(&f).unwrap();
        let mtime = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let mut cache = HashCache::open_in_memory().unwrap();
        assert!(cache.lookup(&f, meta.len(), mtime).is_none());
        cache.put(&f, meta.len(), mtime, "deadbeef").unwrap();
        assert_eq!(
            cache.lookup(&f, meta.len(), mtime).as_deref(),
            Some("deadbeef")
        );
        // Same size+mtime → same hash without re-hash.
        assert_eq!(
            cache.lookup(&f, meta.len(), mtime).as_deref(),
            Some("deadbeef")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn changed_file_misses() {
        let root = tmp("changed");
        let f = root.join("b.txt");
        fs::write(&f, "v1").unwrap();
        let mut cache = HashCache::open_in_memory().unwrap();
        cache.put(&f, 2, 100, "hash-v1").unwrap();
        assert!(cache.lookup(&f, 3, 200).is_none()); // size/mtime changed
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn removed_paths_cleaned() {
        let root = tmp("removed");
        let f = root.join("gone.txt");
        let mut cache = HashCache::open_in_memory().unwrap();
        cache.put(&f, 1, 1, "h").unwrap();
        cache.cleanup_removed(&[]).unwrap();
        assert_eq!(cache.len(), 0);
        let _ = fs::remove_dir_all(&root);
    }
}
