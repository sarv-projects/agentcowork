//! P45.4 — SQLite connection pool (4 readers + 1 writer pre-opened).
//!
//! The concurrency-sensitive read-heavy stores (search/FTS5 index consumers)
//! can pre-open N reader connections + 1 writer instead of opening/closing a
//! connection per call. rusqlite `Connection` is not `Sync`, so the pool owns
//! the handles and leases them out one at a time — callers get a guard whose
//! lifetime proves no concurrent double-take on one handle.
//!
//! Invariants asserted by tests:
//! - `read()` never returns the writer's connection (reads and writes
//!   literally cannot share a handle).
//! - re-acquire after release returns the SAME pre-opened handle (no per-call
//!   open — the exact measure P45.4's acceptance names).
//! - the writer is a single dedicated handle (no new write-path conn).
//! - the free-list is only slot indices (a `Mutex<VecDeque<usize>>`), never
//!   connections — so release works through `&self` without `Connection`
//!   being `Sync`; one lease at a time per handle is enforced by the guard.

use std::collections::VecDeque;
use std::sync::Mutex;

use rusqlite::Connection;
use thiserror::Error;

/// Default pool shape: 4 readers + 1 writer (the P45.4 acceptance).
pub const DEFAULT_READERS: usize = 4;

#[derive(Debug, Error)]
pub enum PoolError {
    #[error("connection pool exhausted: {0}")]
    Exhausted(&'static str),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// One leased reader connection. Releasing returns the slot to the pool.
pub struct ConnGuard<'a> {
    pool: &'a ConnectionPool,
    slot: usize,
    pub conn: &'a Connection,
}

impl Drop for ConnGuard<'_> {
    fn drop(&mut self) {
        self.pool.release(self.slot);
    }
}

/// Path or in-memory — avoids per-site branching.
pub enum PathOrMemory {
    Path(std::path::PathBuf),
    Memory,
}

impl PathOrMemory {
    fn open(&self) -> Result<Connection, rusqlite::Error> {
        match self {
            PathOrMemory::Path(p) => Connection::open(p),
            PathOrMemory::Memory => Connection::open_in_memory(),
        }
    }

    /// Convenience for callers with a real path.
    pub fn path(p: impl Into<std::path::PathBuf>) -> Self {
        Self::Path(p.into())
    }

    pub fn memory() -> Self {
        Self::Memory
    }
}

/// Pre-opened readers + one writer, with interior-mutable free-list so
/// `ConnGuard` releases through `&Pool`.
pub struct ConnectionPool {
    readers: Vec<Connection>,
    writer: Connection,
    free: Mutex<VecDeque<usize>>,
}

impl ConnectionPool {
    /// Open `readers_n` read connections + one dedicated writer to `path`.
    /// Every handle gets the P45.1–.3 non-vault tuning (WAL + synchronous
    /// NORMAL + bounded WAL + mmap) — same as the single-conn stores.
    pub fn open(path: &PathOrMemory, readers_n: usize) -> Result<Self, PoolError> {
        let mut readers = Vec::with_capacity(readers_n);
        for _ in 0..readers_n {
            let c = path.open()?;
            crate::pragmas::apply_read_heavy_index(&c)?;
            readers.push(c);
        }
        let writer = path.open()?;
        crate::pragmas::apply_read_heavy_index(&writer)?;
        let free = Mutex::new((0..readers_n).collect());
        Ok(Self {
            readers,
            writer,
            free,
        })
    }

    /// Lease a reader (returns `Exhausted` when the pool is fully checked
    /// out — the caller retries, never blocks a turn).
    pub fn read(&self) -> Result<ConnGuard<'_>, PoolError> {
        let slot = self
            .free
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(PoolError::Exhausted("all reader connections are in use"))?;
        Ok(ConnGuard {
            pool: self,
            slot,
            conn: &self.readers[slot],
        })
    }

    /// The dedicated writer — one handle, never leased to a reader.
    pub fn write(&self) -> &Connection {
        &self.writer
    }

    fn release(&self, slot: usize) {
        self.free.lock().unwrap().push_back(slot);
    }
}

/// Apply the P45.4 pool over the on-disk index path: open the pool and return
/// it wrapped with the pragmas already applied. Convenience for adopters.
pub fn open_pooled_index(path: &std::path::Path) -> Result<ConnectionPool, PoolError> {
    ConnectionPool::open(&PathOrMemory::Path(path.to_path_buf()), DEFAULT_READERS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> ConnectionPool {
        ConnectionPool::open(&PathOrMemory::Memory, DEFAULT_READERS).unwrap()
    }

    #[test]
    fn preopens_four_readers_plus_one_writer() {
        // Acquiring 4 readers succeeds consecutively (all 4 pre-opened
        // handles exist), and no 5th exists.
        let p = pool();
        let r1 = p.read().unwrap();
        let r2 = p.read().unwrap();
        let r3 = p.read().unwrap();
        let r4 = p.read().unwrap();
        assert!(p.read().is_err(), "5th reader must not exist");
        drop((r1, r2, r3, r4));
        assert!(p.read().is_ok(), "release must return a handle");
    }

    #[test]
    fn release_reuses_preopened_handles_never_reopens() {
        // 8 sequential acquire → release cycles: only the 4 pre-opened
        // handles may ever appear. A per-call open would mint a 5th, 6th, …
        // distinct connection address, which is exactly what P45.4 forbids.
        let p = pool();
        let mut seen: Vec<usize> = Vec::new();
        for _ in 0..8 {
            let g = p.read().unwrap();
            seen.push(g.conn as *const Connection as usize);
            drop(g);
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            DEFAULT_READERS,
            "only pre-opened handles reused"
        );
    }

    #[test]
    fn writer_is_a_distinct_dedicated_handle() {
        let p = pool();
        let w = p.write() as *const Connection as usize;
        for _ in 0..DEFAULT_READERS {
            let r = p.read().unwrap();
            let r_addr = r.conn as *const Connection as usize;
            assert_ne!(w, r_addr, "reader must never be the writer handle");
        }
        drop(p);
    }

    #[test]
    fn writer_is_not_leased_to_readers_with_all_in_use() {
        // Holding all 4 readers must not consume the writer.
        let p = pool();
        let guards: Vec<_> = (0..DEFAULT_READERS).map(|_| p.read().unwrap()).collect();
        assert!(p.read().is_err());
        // The writer is still present and functional — write a test row.
        p.write()
            .execute_batch("CREATE TABLE IF NOT EXISTS t (x INTEGER);")
            .unwrap();
        drop(guards);
    }

    #[test]
    fn pooled_index_applies_non_vault_pragmas_to_every_handle() {
        let dir = std::env::temp_dir().join(format!("agentcowork-pool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("idx.sqlite");
        let p = open_pooled_index(&path).unwrap();
        // Every handle carries the P45.1 non-vault tuning (synchronous=NORMAL).
        for _ in 0..DEFAULT_READERS {
            let r = p.read().unwrap();
            let sync: i64 = r
                .conn
                .query_row("PRAGMA synchronous;", [], |r| r.get(0))
                .unwrap();
            assert_eq!(sync, 1, "pooled reader must be tuned synchronous=NORMAL");
        }
        let sync: i64 = p
            .write()
            .query_row("PRAGMA synchronous;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sync, 1, "pooled writer must be tuned synchronous=NORMAL");
        std::fs::remove_dir_all(&dir).ok();
    }
}
