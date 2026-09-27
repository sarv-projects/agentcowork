//! P36 (G7) — FTS5 trigram index in SQLite for **substring** filename search.
//!
//! The existing FTS5 default tokenizer is prefix-friendly but weak at
//! mid-string matches ("report-2024" won't hit a query for "2024"). The
//! `trigram` tokenizer makes every 3-char subsequence indexable → substring
//! queries that scale (the USN journal piece of G7 is the companion
//! incremental scan; see `usn.rs`).

use rusqlite::{Connection, Result, params};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct TrigramHit {
    pub path: PathBuf,
    pub score: f64,
}

/// Trigram FTS index over filenames (basenames only — the substring surface).
pub struct TrigramIndex {
    backing: TrigramBacking,
}

enum TrigramBacking {
    Memory(Connection),
    /// P45.4 — file indexes keep four readers and one writer open.
    Pooled(crate::pool::ConnectionPool),
}

impl TrigramIndex {
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE VIRTUAL TABLE filenames USING fts5(name, tokenize = 'trigram');",
        )?;
        Ok(Self {
            backing: TrigramBacking::Memory(conn),
        })
    }

    pub fn open(path: &Path) -> Result<Self> {
        let pool = crate::pool::ConnectionPool::open(
            &crate::pool::PathOrMemory::Path(path.to_path_buf()),
            crate::pool::DEFAULT_READERS,
        )
        .map_err(|err| rusqlite::Error::InvalidParameterName(err.to_string()))?;
        // The pool already applied the read-heavy pragmas. Create the table
        // on the writer; readers see it through WAL.
        pool.write().execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS filenames USING fts5(name, tokenize = 'trigram');",
        )?;
        Ok(Self {
            backing: TrigramBacking::Pooled(pool),
        })
    }

    fn writer(&self) -> &Connection {
        match &self.backing {
            TrigramBacking::Memory(conn) => conn,
            TrigramBacking::Pooled(pool) => pool.write(),
        }
    }

    /// P45.3 — checkpoint when the user is idle or the host is on battery
    /// and the WAL has at least one page.
    pub fn maintain(&self, user_idle: bool) -> Result<bool> {
        crate::pragmas::checkpoint_connection(self.writer(), user_idle)
    }

    pub fn insert(&mut self, path: &Path) -> Result<()> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.len() >= 3 {
            self.writer().execute(
                "INSERT OR REPLACE INTO filenames(name) VALUES (?1)",
                params![name],
            )?;
        }
        Ok(())
    }

    pub fn insert_batch(&mut self, paths: &[&Path]) -> Result<()> {
        for p in paths {
            self.insert(p)?;
        }
        Ok(())
    }

    pub fn search(&self, needle: &str, limit: usize) -> Result<Vec<TrigramHit>> {
        let needle = needle.trim();
        if needle.len() < 3 {
            // Substring under 3 chars: fall back to a LIKE scan of the table
            // (FTS5 trigram needs >= 3 chars) — still bounded + honest.
            return self.like_search(needle, limit);
        }
        let guard;
        let conn = match &self.backing {
            TrigramBacking::Memory(conn) => conn,
            TrigramBacking::Pooled(pool) => {
                guard = pool
                    .read()
                    .map_err(|err| rusqlite::Error::InvalidParameterName(err.to_string()))?;
                guard.conn
            }
        };
        let mut stmt = conn
            .prepare("SELECT name, bm25(filenames) AS b FROM filenames WHERE filenames MATCH ?1 ORDER BY b LIMIT ?2")?;
        // The trigram tokenizer accepts the literal substring quoted.
        let q = format!("\"{needle}\"");
        let rows = stmt.query_map(params![q, limit as i64], |r| {
            Ok(TrigramHit {
                path: PathBuf::from(r.get::<_, String>(0)?),
                score: r.get(1)?,
            })
        })?;
        rows.collect()
    }

    fn like_search(&self, needle: &str, limit: usize) -> Result<Vec<TrigramHit>> {
        let guard;
        let conn = match &self.backing {
            TrigramBacking::Memory(conn) => conn,
            TrigramBacking::Pooled(pool) => {
                guard = pool
                    .read()
                    .map_err(|err| rusqlite::Error::InvalidParameterName(err.to_string()))?;
                guard.conn
            }
        };
        let mut stmt = conn.prepare("SELECT name FROM filenames WHERE name LIKE ?1 LIMIT ?2")?;
        let pat = format!("%{}%", needle.replace('%', "\\%"));
        let rows = stmt.query_map(params![pat, limit as i64], |r| {
            Ok(TrigramHit {
                path: PathBuf::from(r.get::<_, String>(0)?),
                score: 1.0,
            })
        })?;
        rows.collect()
    }

    pub fn len(&self) -> usize {
        self.writer()
            .query_row("SELECT COUNT(*) FROM filenames", [], |r| r.get(0))
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp() -> std::path::PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("agentcowork-trigram-{}-{n}", std::process::id()))
    }

    #[test]
    fn substring_search_hits_mid_string() {
        let mut idx = TrigramIndex::open_in_memory().unwrap();
        idx.insert(Path::new("/tmp/2024-report-final.pdf")).unwrap();
        idx.insert(Path::new("/tmp/notes.txt")).unwrap();
        idx.insert(Path::new("/tmp/budget.xlsx")).unwrap();
        let hits = idx.search("report", 10).unwrap();
        assert!(
            hits.iter()
                .any(|h| h.path.ends_with("2024-report-final.pdf"))
        );
        let hits = idx.search("2024", 10).unwrap();
        assert!(
            hits.iter()
                .any(|h| h.path.ends_with("2024-report-final.pdf")),
            "mid-string 2024 must match"
        );
        let _ = fs::remove_dir_all(tmp());
    }

    #[test]
    fn short_query_falls_back_to_like() {
        let mut idx = TrigramIndex::open_in_memory().unwrap();
        idx.insert(Path::new("/tmp/a12.txt")).unwrap();
        let hits = idx.search("a1", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("a12.txt"));
    }

    #[test]
    fn no_match_returns_empty() {
        let mut idx = TrigramIndex::open_in_memory().unwrap();
        idx.insert(Path::new("/tmp/only.txt")).unwrap();
        assert!(idx.search("zzz", 10).unwrap().is_empty());
    }

    #[test]
    fn file_index_uses_the_pool_and_can_checkpoint_when_idle() {
        let dir = tmp();
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("names.sqlite");
        let mut idx = TrigramIndex::open(&path).unwrap();
        idx.insert(Path::new("/work/quarterly-report.pdf")).unwrap();
        let hits = idx.search("report", 5).unwrap();
        assert!(
            hits.iter()
                .any(|hit| hit.path.ends_with("quarterly-report.pdf"))
        );
        let _ = idx.maintain(true).unwrap();
        fs::remove_dir_all(&dir).ok();
    }
}
