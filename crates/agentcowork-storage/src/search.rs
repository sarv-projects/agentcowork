//! G7 — SQLite FTS5 instant filename index + debounced watcher (doc 49 §5).
//!
//! The index is a plain FTS5 virtual table (`name`, `dir`, unindexed `path`).
//! Queries are prefix matches so typing "repo" finds "repo_map.rs" instantly.
//! The watcher uses `notify` + a pure debouncer (flushed after a quiet period)
//! for incremental updates; OS-native hooks (Everything/MFT, mdfind, Baloo)
//! remain optional accelerators, not requirements.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use notify::Watcher;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::StorageError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    pub path: String,
    pub name: String,
    pub dir: String,
}

pub struct SearchIndex {
    conn: Connection,
}

impl SearchIndex {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StorageError> {
        // P45.1–.3 — WAL + synchronous=NORMAL + bounded WAL + mmap on the
        // read-heavy FTS5 index. Vault never goes through here.
        crate::pragmas::apply_read_heavy_index(&conn)?;
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS files \
             USING fts5(path UNINDEXED, name, dir, tokenize='unicode61');",
        )?;
        Ok(Self { conn })
    }

    pub fn insert(&mut self, path: &Path) -> Result<(), StorageError> {
        let (name, dir) = split(path);
        self.conn.execute(
            "INSERT INTO files(path, name, dir) VALUES (?1, ?2, ?3)",
            params![path.to_string_lossy().to_string(), name, dir],
        )?;
        Ok(())
    }

    pub fn insert_batch(
        &mut self,
        paths: impl Iterator<Item = PathBuf>,
    ) -> Result<usize, StorageError> {
        let tx = self.conn.transaction()?;
        let mut n = 0usize;
        {
            let mut stmt = tx.prepare("INSERT INTO files(path, name, dir) VALUES (?1, ?2, ?3)")?;
            for p in paths {
                let (name, dir) = split(&p);
                stmt.execute(params![p.to_string_lossy().to_string(), name, dir])?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn remove(&mut self, path: &Path) -> Result<(), StorageError> {
        self.conn.execute(
            "DELETE FROM files WHERE path = ?1",
            params![path.to_string_lossy().to_string()],
        )?;
        Ok(())
    }

    /// Prefix search over name + dir; results ordered by FTS5 relevance.
    pub fn query(&self, term: &str, limit: usize) -> Result<Vec<SearchHit>, StorageError> {
        let term = term.trim();
        if term.is_empty() {
            return Ok(Vec::new());
        }
        let q = format!("\"{}\"*", term.replace('"', "\"\""));
        let mut stmt = self.conn.prepare(
            "SELECT path, name, dir FROM files WHERE files MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![q, limit as i64], |r| {
            Ok(SearchHit {
                path: r.get(0)?,
                name: r.get(1)?,
                dir: r.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }
}

fn split(path: &Path) -> (String, String) {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = path
        .parent()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    (name, dir)
}

/// Pure debounce: accumulate paths, flush after a quiet period. Testable
/// without a filesystem watcher.
pub struct Debouncer {
    pending: HashSet<PathBuf>,
    last_event: Option<Instant>,
    quiet: Duration,
}

impl Debouncer {
    pub fn new(quiet: Duration) -> Self {
        Debouncer {
            pending: HashSet::new(),
            last_event: None,
            quiet,
        }
    }

    /// Feed events; returns the flushed batch when the stream has been quiet
    /// for `quiet` since the previous event.
    pub fn push(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
        now: Instant,
    ) -> Option<Vec<PathBuf>> {
        let should_flush = match self.last_event {
            Some(last) => now.duration_since(last) >= self.quiet,
            None => false,
        };
        self.pending.extend(paths);
        if should_flush {
            return self.flush();
        }
        self.last_event = Some(now);
        None
    }

    pub fn flush(&mut self) -> Option<Vec<PathBuf>> {
        if self.pending.is_empty() {
            return None;
        }
        let mut v: Vec<PathBuf> = self.pending.drain().collect();
        v.sort();
        Some(v)
    }
}

/// Handle for a live watcher thread; `stop()` (or drop) joins it.
pub struct WatchHandle {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl WatchHandle {
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }

    /// Construct a handle from an already-running watcher thread (used by
    /// `events::watch_events`, which shares this crate's stop/join contract).
    pub(crate) fn from_parts(stop: Arc<AtomicBool>, join: std::thread::JoinHandle<()>) -> Self {
        Self {
            stop,
            join: Some(join),
        }
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Watch `roots` recursively and call `on_change` with debounced path batches.
pub fn watch<F>(
    roots: Vec<PathBuf>,
    quiet: Duration,
    mut on_change: F,
) -> Result<WatchHandle, StorageError>
where
    F: FnMut(Vec<PathBuf>) + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| StorageError::Notify(e.to_string()))?;

    for r in &roots {
        watcher
            .watch(r.as_path(), notify::RecursiveMode::Recursive)
            .map_err(|e| StorageError::Notify(e.to_string()))?;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let join = std::thread::spawn(move || {
        let mut debouncer = Debouncer::new(quiet);
        loop {
            if stop2.load(Ordering::SeqCst) {
                break;
            }
            match rx.recv_timeout(quiet) {
                Ok(Ok(event)) => {
                    let now = Instant::now();
                    if let Some(batch) = debouncer.push(event.paths, now) {
                        on_change(batch);
                    }
                }
                Ok(Err(_)) => continue,
                Err(_timeout) => {
                    // Quiet period elapsed — flush whatever is pending.
                    let now = Instant::now();
                    if let Some(batch) = debouncer.push(std::iter::empty(), now) {
                        on_change(batch);
                    }
                }
            }
        }
    });

    Ok(WatchHandle {
        stop,
        join: Some(join),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts5_index_prefix_query() {
        let mut idx = SearchIndex::open_in_memory().unwrap();
        idx.insert(Path::new("/home/u/Documents/report.md"))
            .unwrap();
        idx.insert(Path::new("/home/u/src/main.rs")).unwrap();
        idx.insert(Path::new("/home/u/src/lib.rs")).unwrap();
        assert_eq!(idx.count(), 3);

        let hits = idx.query("main", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].name.contains("main"));

        let rs = idx.query("rs", 10).unwrap();
        assert_eq!(rs.len(), 2);

        idx.remove(Path::new("/home/u/src/lib.rs")).unwrap();
        assert_eq!(idx.count(), 2);
        assert_eq!(idx.query("rs", 10).unwrap().len(), 1);
    }

    #[test]
    fn many_inserts_query_is_fast_enough() {
        let mut idx = SearchIndex::open_in_memory().unwrap();
        let paths: Vec<PathBuf> = (0..5000)
            .map(|i| PathBuf::from(format!("/data/project/file_{i}.rs")))
            .collect();
        let start = Instant::now();
        idx.insert_batch(paths.into_iter()).unwrap();
        let hits = idx.query("file_2601", 10).unwrap();
        let elapsed = start.elapsed();
        assert_eq!(hits.len(), 1);
        // Generous bound: proves FTS5 (not a linear scan) is in play.
        assert!(elapsed < Duration::from_secs(5), "elapsed {elapsed:?}");
    }

    #[test]
    fn debouncer_flushes_after_quiet_period() {
        let mut d = Debouncer::new(Duration::from_millis(100));
        let t0 = Instant::now();
        assert_eq!(d.push(vec![PathBuf::from("a")], t0), None);
        assert_eq!(
            d.push(vec![PathBuf::from("b")], t0 + Duration::from_millis(50)),
            None
        );
        // 150ms after first event → quiet period elapsed → flush both.
        let batch = d
            .push(vec![], t0 + Duration::from_millis(150))
            .expect("should flush");
        assert_eq!(batch, vec![PathBuf::from("a"), PathBuf::from("b")]);
    }
}
