//! P2.10 — session replay ingest + storage (E5, ARCH/08 §8.5, doc 33 §9).
//!
//! BrowserOS contract (doc 33 §9.2/§9.3): the injected recorder streams
//! NDJSON event batches to the ingest with `x-recording-tab-id` /
//! `x-recording-document-id` / `x-recording-batch-id` + gap header. We
//! validate chrome document ids, make malformed/dropped lines **sticky
//! `has_gap`** (no fake-complete replays), and commit stream metadata +
//! NDJSON payload + durable dedupe identity in one transaction.
//!
//! Storage: `<data_dir>/replays/` NDJSON files (one per document) +
//! `<data_dir>/screenshots/` JPEGs (one per step) + a SQLite index
//! (per-tab → per-document segments with event counts + timestamps).
//! Retention: 7 days default, configurable; wipe = delete files.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One recorded event (one NDJSON line). `kind` mirrors the injected
/// recorder's capture set: `dom_mutation`, `scroll`, `click`, `input`,
/// `navigate`, `screenshot`, `meta`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplayEvent {
    /// Monotonic sequence within the document stream (1-based).
    pub seq: u64,
    /// UNIX milliseconds.
    pub ts_ms: u64,
    pub kind: String,
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub document_id: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl ReplayEvent {
    pub fn new(kind: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            seq: 0,
            ts_ms: now_ms(),
            kind: kind.into(),
            tab_id: String::new(),
            document_id: String::new(),
            data,
        }
    }
}

/// The ingest contract (doc 33 §9.2): one NDJSON batch from the recorder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplayBatch {
    pub batch_id: String,
    pub tab_id: String,
    pub document_id: String,
    /// True when the recorder knows it missed events (sticky).
    #[serde(default)]
    pub gap: bool,
    pub events: Vec<ReplayEvent>,
}

/// Receipt returned for every accepted batch (dedupe-stable).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestReceipt {
    pub batch_id: String,
    pub document_id: String,
    pub accepted: u64,
    pub dropped: u64,
    /// True when this batch_id was already ingested for this document.
    pub duplicate: bool,
    pub has_gap: bool,
}

/// Per-document segment metadata served to the replay/scrubber UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub document_id: String,
    pub tab_id: String,
    pub first_ts_ms: u64,
    pub last_ts_ms: u64,
    pub event_count: u64,
    pub size_bytes: u64,
    pub has_gap: bool,
    pub created_ms: u64,
}

/// One document's replay timeline for the scrubber UI (P3.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Timeline {
    pub segment: Option<Segment>,
    pub events: Vec<ReplayEvent>,
    /// Steps (1-based) that have a persisted screenshot JPEG.
    pub screenshot_steps: Vec<u64>,
}

/// Result of a retention sweep.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SweepStats {
    pub documents_removed: u64,
    pub replay_files_removed: u64,
    pub screenshot_files_removed: u64,
    pub bytes_freed: u64,
}

/// NDJSON replay files + SQLite index on disk under a base dir.
pub struct ReplayStore {
    base_dir: PathBuf,
}

impl ReplayStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    pub fn replay_dir(&self) -> PathBuf {
        self.base_dir.join("replays")
    }

    pub fn screenshot_dir(&self) -> PathBuf {
        self.base_dir.join("screenshots")
    }

    pub fn index_path(&self) -> PathBuf {
        self.replay_dir().join("index.sqlite")
    }

    fn replay_file(&self, document_id: &str) -> PathBuf {
        self.replay_dir().join(format!("{document_id}.ndjson"))
    }

    fn ensure_dirs(&self) -> io::Result<()> {
        fs::create_dir_all(self.replay_dir())?;
        fs::create_dir_all(self.screenshot_dir())?;
        Ok(())
    }

    /// Open (or create) the SQLite index with the replay schema.
    pub fn open_index(&self) -> Result<Connection, ReplayError> {
        self.ensure_dirs()?;
        let conn = Connection::open(self.index_path())?;
        // P45.1/.3 — WAL + synchronous=NORMAL + bounded WAL on the replay
        // index (non-crypto; the vault keeps its safer FULL setting).
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA journal_size_limit=67108864;
             PRAGMA wal_autocheckpoint=4000;",
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS replay_segments (
                document_id TEXT PRIMARY KEY,
                tab_id       TEXT NOT NULL,
                first_ts_ms  INTEGER NOT NULL,
                last_ts_ms   INTEGER NOT NULL,
                event_count  INTEGER NOT NULL DEFAULT 0,
                size_bytes   INTEGER NOT NULL DEFAULT 0,
                has_gap      INTEGER NOT NULL DEFAULT 0,
                created_ms   INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS replay_batches (
                batch_id   TEXT NOT NULL,
                document_id TEXT NOT NULL,
                seq_start  INTEGER NOT NULL,
                seq_end    INTEGER NOT NULL,
                PRIMARY KEY (batch_id, document_id)
            ) WITHOUT ROWID;",
        )?;
        Ok(conn)
    }

    /// Persist one JPEG screenshot for a step of a document.
    pub fn write_screenshot(
        &self,
        document_id: &str,
        step: u64,
        jpeg: &[u8],
    ) -> Result<PathBuf, ReplayError> {
        self.ensure_dirs()?;
        let path = self
            .screenshot_dir()
            .join(format!("{document_id}-{step:06}.jpg"));
        fs::write(&path, jpeg)?;
        Ok(path)
    }

    /// Read a document's full event stream back (playback).
    pub fn read_document(&self, document_id: &str) -> Result<Vec<ReplayEvent>, ReplayError> {
        let path = self.replay_file(document_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let body = fs::read_to_string(&path)?;
        let mut events = Vec::new();
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(ev) = serde_json::from_str::<ReplayEvent>(line) {
                events.push(ev);
            }
        }
        Ok(events)
    }

    /// All per-document segments, newest-created first.
    pub fn segments(&self) -> Result<Vec<Segment>, ReplayError> {
        let conn = self.open_index()?;
        let mut stmt = conn.prepare(
            "SELECT document_id, tab_id, first_ts_ms, last_ts_ms, event_count,
                    size_bytes, has_gap, created_ms
             FROM replay_segments ORDER BY created_ms DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Segment {
                document_id: r.get(0)?,
                tab_id: r.get(1)?,
                first_ts_ms: r.get(2)?,
                last_ts_ms: r.get(3)?,
                event_count: r.get(4)?,
                size_bytes: r.get(5)?,
                has_gap: r.get::<_, i64>(6)? != 0,
                created_ms: r.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Delete replays + screenshots older than `max_age` (default 7 days).
    /// A document's files are removed only when its *segment* is expired.
    pub fn retention_sweep(&self, max_age: Duration) -> Result<SweepStats, ReplayError> {
        let cutoff = now_ms().saturating_sub(max_age.as_millis() as u64);
        let conn = self.open_index()?;
        let expired: Vec<Segment> = {
            let mut stmt = conn.prepare(
                "SELECT document_id, tab_id, first_ts_ms, last_ts_ms, event_count,
                        size_bytes, has_gap, created_ms
                 FROM replay_segments WHERE last_ts_ms < ?1",
            )?;
            let rows = stmt.query_map([cutoff as i64], |r| {
                Ok(Segment {
                    document_id: r.get(0)?,
                    tab_id: r.get(1)?,
                    first_ts_ms: r.get(2)?,
                    last_ts_ms: r.get(3)?,
                    event_count: r.get(4)?,
                    size_bytes: r.get(5)?,
                    has_gap: r.get::<_, i64>(6)? != 0,
                    created_ms: r.get(7)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut stats = SweepStats::default();
        for seg in expired {
            // Replay NDJSON + its screenshots.
            if let Ok(md) = fs::metadata(self.replay_file(&seg.document_id)) {
                stats.bytes_freed += md.len();
            }
            if fs::remove_file(self.replay_file(&seg.document_id)).is_ok() {
                stats.replay_files_removed += 1;
            }
            let shots = self.screenshot_files(&seg.document_id)?;
            for s in shots {
                if let Ok(md) = fs::metadata(&s) {
                    stats.bytes_freed += md.len();
                }
                if fs::remove_file(&s).is_ok() {
                    stats.screenshot_files_removed += 1;
                }
            }
            conn.execute(
                "DELETE FROM replay_segments WHERE document_id = ?1",
                params![seg.document_id],
            )?;
            conn.execute(
                "DELETE FROM replay_batches WHERE document_id = ?1",
                params![seg.document_id],
            )?;
            stats.documents_removed += 1;
        }
        Ok(stats)
    }

    /// Wipe everything: delete all replay files, screenshots, and index rows.
    pub fn wipe(&self) -> Result<SweepStats, ReplayError> {
        let mut stats = SweepStats::default();
        if let Ok(conn) = self.open_index() {
            conn.execute("DELETE FROM replay_batches", [])?;
            conn.execute("DELETE FROM replay_segments", [])?;
        }
        if self.replay_dir().exists() {
            for entry in fs::read_dir(self.replay_dir())? {
                let p = entry?.path();
                if p.is_file() && p.extension().map(|e| e == "ndjson").unwrap_or(false) {
                    if let Ok(md) = fs::metadata(&p) {
                        stats.bytes_freed += md.len();
                    }
                    if fs::remove_file(&p).is_ok() {
                        stats.replay_files_removed += 1;
                    }
                }
            }
        }
        if self.screenshot_dir().exists() {
            for entry in fs::read_dir(self.screenshot_dir())? {
                let p = entry?.path();
                if p.is_file() {
                    if let Ok(md) = fs::metadata(&p) {
                        stats.bytes_freed += md.len();
                    }
                    if fs::remove_file(&p).is_ok() {
                        stats.screenshot_files_removed += 1;
                    }
                }
            }
        }
        Ok(stats)
    }

    /// Searchable sessions list (P3.1) — segments filtered by document_id /
    /// tab_id substring (case-insensitive), newest first; empty = all.
    pub fn search_sessions(&self, query: &str) -> Result<Vec<Segment>, ReplayError> {
        let conn = self.open_index()?;
        let q = query.trim().to_lowercase();
        let mut stmt = conn.prepare(
            "SELECT document_id, tab_id, first_ts_ms, last_ts_ms, event_count,
                    size_bytes, has_gap, created_ms
             FROM replay_segments ORDER BY created_ms DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Segment {
                document_id: r.get(0)?,
                tab_id: r.get(1)?,
                first_ts_ms: r.get(2)?,
                last_ts_ms: r.get(3)?,
                event_count: r.get(4)?,
                size_bytes: r.get(5)?,
                has_gap: r.get::<_, i64>(6)? != 0,
                created_ms: r.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            let seg = row?;
            if q.is_empty()
                || seg.document_id.to_lowercase().contains(&q)
                || seg.tab_id.to_lowercase().contains(&q)
            {
                out.push(seg);
            }
        }
        Ok(out)
    }

    /// Full timeline for one document (P3.1): segment + events + screenshot
    /// steps — everything the scrubber renders.
    pub fn timeline(&self, document_id: &str) -> Result<Timeline, ReplayError> {
        let segment = self
            .search_sessions(document_id)?
            .into_iter()
            .find(|s| s.document_id == document_id);
        Ok(Timeline {
            segment,
            events: self.read_document(document_id)?,
            screenshot_steps: self.screenshot_steps(document_id)?,
        })
    }

    /// Steps (1-based) that have a persisted screenshot JPEG.
    pub fn screenshot_steps(&self, document_id: &str) -> Result<Vec<u64>, ReplayError> {
        let prefix = format!("{document_id}-");
        let mut steps = Vec::new();
        if !self.screenshot_dir().exists() {
            return Ok(steps);
        }
        for entry in fs::read_dir(self.screenshot_dir())? {
            let p = entry?.path();
            let name = match p.file_name().map(|n| n.to_string_lossy().to_string()) {
                Some(n) => n,
                None => continue,
            };
            if !name.starts_with(&prefix) || !name.ends_with(".jpg") {
                continue;
            }
            let mid = &name[prefix.len()..name.len() - 4];
            if let Ok(step) = mid.parse::<u64>() {
                steps.push(step);
            }
        }
        steps.sort_unstable();
        Ok(steps)
    }

    /// Absolute path of a step screenshot, if it exists.
    pub fn screenshot_path(&self, document_id: &str, step: u64) -> Option<PathBuf> {
        let p = self
            .screenshot_dir()
            .join(format!("{document_id}-{step:06}.jpg"));
        p.exists().then_some(p)
    }

    /// Events after `since_seq` (P3.1 Watch — live tail of a document).
    pub fn events_since(
        &self,
        document_id: &str,
        since_seq: u64,
    ) -> Result<Vec<ReplayEvent>, ReplayError> {
        Ok(self
            .read_document(document_id)?
            .into_iter()
            .filter(|e| e.seq > since_seq)
            .collect())
    }

    fn screenshot_files(&self, document_id: &str) -> Result<Vec<PathBuf>, ReplayError> {
        let prefix = format!("{document_id}-");
        let mut out = Vec::new();
        if !self.screenshot_dir().exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(self.screenshot_dir())? {
            let p = entry?.path();
            if p.is_file()
                && p.file_name()
                    .map(|n| n.to_string_lossy().starts_with(&prefix))
                    .unwrap_or(false)
            {
                out.push(p);
            }
        }
        Ok(out)
    }
}

/// The ingest: validate → parse → dedupe → one-transaction commit.
pub struct ReplayIngest {
    store: ReplayStore,
}

impl ReplayIngest {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            store: ReplayStore::new(base_dir),
        }
    }

    /// In-process path: a structured batch.
    pub fn ingest_batch(&self, batch: ReplayBatch) -> Result<IngestReceipt, ReplayError> {
        self.ingest(
            &batch.batch_id,
            &batch.tab_id,
            &batch.document_id,
            batch.gap,
            batch.events,
        )
    }

    /// HTTP path (doc 33 §9.2): raw NDJSON body + recorder headers. Malformed
    /// lines are dropped and flip the stream's sticky `has_gap`.
    pub fn ingest_ndjson(
        &self,
        tab_id: &str,
        document_id: &str,
        batch_id: &str,
        gap: bool,
        body: &str,
    ) -> Result<IngestReceipt, ReplayError> {
        let mut events = Vec::new();
        let mut dropped = 0u64;
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<ReplayEvent>(line) {
                Ok(ev) => events.push(ev),
                Err(_) => dropped += 1,
            }
        }
        self.ingest(batch_id, tab_id, document_id, gap || dropped > 0, events)
            .map(|mut r| {
                r.dropped = dropped;
                r
            })
    }

    /// Core path — shared by both ingest forms.
    fn ingest(
        &self,
        batch_id: &str,
        tab_id: &str,
        document_id: &str,
        gap: bool,
        events: Vec<ReplayEvent>,
    ) -> Result<IngestReceipt, ReplayError> {
        validate_document_id(document_id)?;
        if batch_id.trim().is_empty() {
            return Err(ReplayError::InvalidBatchId);
        }
        let mut conn = self.store.open_index()?;

        // Dedupe identity: (batch_id, document_id) is unique. A re-send must
        // return the same receipt without appending duplicate lines.
        let existing = conn
            .query_row(
                "SELECT has_gap, event_count FROM replay_segments WHERE document_id = ?1",
                params![document_id],
                |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, u64>(1)?)),
            )
            .optional()?;
        let dup = conn
            .query_row(
                "SELECT 1 FROM replay_batches WHERE batch_id = ?1 AND document_id = ?2",
                params![batch_id, document_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();

        if dup {
            let (has_gap, _count) = existing.unwrap_or((false, 0));
            return Ok(IngestReceipt {
                batch_id: batch_id.into(),
                document_id: document_id.into(),
                accepted: 0,
                dropped: 0,
                duplicate: true,
                has_gap,
            });
        }

        if events.is_empty() {
            // Nothing to append, but a gap may still need to go sticky.
            if gap {
                conn.execute(
                    "INSERT INTO replay_segments
                        (document_id, tab_id, first_ts_ms, last_ts_ms, event_count,
                         size_bytes, has_gap, created_ms)
                     VALUES (?1, ?2, ?3, ?3, 0, 0, 1, ?3)
                     ON CONFLICT(document_id) DO UPDATE SET has_gap = 1",
                    params![document_id, tab_id, now_ms() as i64],
                )?;
            }
            return Ok(IngestReceipt {
                batch_id: batch_id.into(),
                document_id: document_id.into(),
                accepted: 0,
                dropped: 0,
                duplicate: false,
                has_gap: gap,
            });
        }

        // Assign monotonic seq within the document stream.
        let start = existing.map(|(_, c)| c).unwrap_or(0);
        let mut evs = Vec::with_capacity(events.len());
        let mut first_ts = u64::MAX;
        let mut last_ts = 0u64;
        for (i, mut ev) in events.into_iter().enumerate() {
            ev.seq = start + i as u64 + 1;
            ev.tab_id = tab_id.to_string();
            ev.document_id = document_id.to_string();
            first_ts = first_ts.min(ev.ts_ms);
            last_ts = last_ts.max(ev.ts_ms);
            evs.push(ev);
        }
        if first_ts == u64::MAX {
            first_ts = now_ms();
        }
        if last_ts == 0 {
            last_ts = first_ts;
        }

        // Append NDJSON payload, then commit metadata + dedupe identity in
        // one SQLite transaction. If the tx fails we truncate the file back
        // to its pre-append length so retries don't double-write.
        let file = self.store.replay_file(document_id);
        let before_len = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        {
            let mut f = OpenOptions::new().create(true).append(true).open(&file)?;
            for ev in &evs {
                let mut line = serde_json::to_vec(ev)?;
                line.push(b'\n');
                f.write_all(&line)?;
            }
            f.flush()?;
        }
        let after_len = fs::metadata(&file).map(|m| m.len()).unwrap_or(before_len);
        let has_gap = gap;

        let tx = conn.transaction()?;
        let res = (|| -> Result<(), ReplayError> {
            tx.execute(
                "INSERT INTO replay_batches (batch_id, document_id, seq_start, seq_end)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    batch_id,
                    document_id,
                    start as i64 + 1,
                    start as i64 + evs.len() as i64
                ],
            )?;
            tx.execute(
                "INSERT INTO replay_segments
                    (document_id, tab_id, first_ts_ms, last_ts_ms, event_count,
                     size_bytes, has_gap, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(document_id) DO UPDATE SET
                    tab_id      = excluded.tab_id,
                    first_ts_ms = MIN(first_ts_ms, excluded.first_ts_ms),
                    last_ts_ms  = MAX(last_ts_ms, excluded.last_ts_ms),
                    event_count = event_count + excluded.event_count,
                    size_bytes  = excluded.size_bytes,
                    has_gap     = MAX(has_gap, excluded.has_gap)",
                params![
                    document_id,
                    tab_id,
                    first_ts as i64,
                    last_ts as i64,
                    evs.len() as i64,
                    after_len as i64,
                    has_gap as i64,
                    now_ms() as i64,
                ],
            )?;
            Ok(())
        })();
        match res {
            Ok(()) => {
                tx.commit()?;
                Ok(IngestReceipt {
                    batch_id: batch_id.into(),
                    document_id: document_id.into(),
                    accepted: evs.len() as u64,
                    dropped: 0,
                    duplicate: false,
                    has_gap,
                })
            }
            Err(e) => {
                // Roll back the file append so a retry stays clean.
                if let Ok(f) = OpenOptions::new().write(true).open(&file) {
                    let _ = f.set_len(before_len);
                }
                Err(e)
            }
        }
    }

    pub fn store(&self) -> &ReplayStore {
        &self.store
    }
}

/// Chrome document ids are short opaque ASCII ids. We validate the shape so
/// path traversal can never reach outside the replays dir.
pub fn validate_document_id(id: &str) -> Result<(), ReplayError> {
    if id.is_empty() || id.len() > 64 {
        return Err(ReplayError::InvalidDocumentId(id.to_string()));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        return Err(ReplayError::InvalidDocumentId(id.to_string()));
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid chrome document id: {0}")]
    InvalidDocumentId(String),
    #[error("empty batch id")]
    InvalidBatchId,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("agentcowork-replay-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn ev(kind: &str, ts: u64) -> ReplayEvent {
        ReplayEvent {
            seq: 0,
            ts_ms: ts,
            kind: kind.into(),
            tab_id: String::new(),
            document_id: String::new(),
            data: serde_json::json!({}),
        }
    }

    #[test]
    fn ingest_batch_appends_and_segments_accumulate() {
        let dir = tmp_dir("batch");
        let ingest = ReplayIngest::new(&dir);
        let r1 = ingest
            .ingest_batch(ReplayBatch {
                batch_id: "b1".into(),
                tab_id: "tab-1".into(),
                document_id: "docABC123".into(),
                gap: false,
                events: vec![ev("click", 1000), ev("scroll", 1100)],
            })
            .unwrap();
        assert_eq!(r1.accepted, 2);
        assert!(!r1.duplicate);

        let r2 = ingest
            .ingest_batch(ReplayBatch {
                batch_id: "b2".into(),
                tab_id: "tab-1".into(),
                document_id: "docABC123".into(),
                gap: false,
                events: vec![ev("input", 1200)],
            })
            .unwrap();
        assert_eq!(r2.accepted, 1);

        let segs = ingest.store().segments().unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].event_count, 3);
        assert_eq!(segs[0].first_ts_ms, 1000);
        assert_eq!(segs[0].last_ts_ms, 1200);
        assert!(!segs[0].has_gap);

        // Playback returns all events in order with document-scoped seq.
        let all = ingest.store().read_document("docABC123").unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].seq, 1);
        assert_eq!(all[2].seq, 3);
        assert_eq!(all[0].document_id, "docABC123");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_batch_is_receipt_stable_no_double_append() {
        let dir = tmp_dir("dup");
        let ingest = ReplayIngest::new(&dir);
        let batch = ReplayBatch {
            batch_id: "b1".into(),
            tab_id: "tab-1".into(),
            document_id: "docABC123".into(),
            gap: false,
            events: vec![ev("click", 1000)],
        };
        let first = ingest.ingest_batch(batch.clone()).unwrap();
        assert_eq!(first.accepted, 1);
        let again = ingest.ingest_batch(batch).unwrap();
        assert!(again.duplicate);
        assert_eq!(again.accepted, 0);
        assert_eq!(ingest.store().read_document("docABC123").unwrap().len(), 1);
        assert_eq!(ingest.store().segments().unwrap()[0].event_count, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_ndjson_lines_drop_and_flip_sticky_gap() {
        let dir = tmp_dir("gap");
        let ingest = ReplayIngest::new(&dir);
        let body = "{\"seq\":0,\"ts_ms\":1,\"kind\":\"click\",\"data\":{}}\nNOT-JSON\n{\"seq\":0,\"ts_ms\":2,\"kind\":\"scroll\",\"data\":{}}\n";
        let r = ingest
            .ingest_ndjson("tab-1", "docABC123", "b1", false, body)
            .unwrap();
        assert_eq!(r.accepted, 2);
        assert_eq!(r.dropped, 1);
        assert!(r.has_gap);
        let segs = ingest.store().segments().unwrap();
        assert!(segs[0].has_gap, "malformed line must stick has_gap");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recorder_declared_gap_is_sticky_even_with_events() {
        let dir = tmp_dir("gapped");
        let ingest = ReplayIngest::new(&dir);
        ingest
            .ingest_batch(ReplayBatch {
                batch_id: "b1".into(),
                tab_id: "tab-1".into(),
                document_id: "docABC123".into(),
                gap: true,
                events: vec![ev("click", 1000)],
            })
            .unwrap();
        assert!(ingest.store().segments().unwrap()[0].has_gap);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_document_id_rejected() {
        assert!(validate_document_id("").is_err());
        assert!(validate_document_id("../../etc/passwd").is_err());
        assert!(validate_document_id(&"x".repeat(65)).is_err());
        assert!(validate_document_id("docABC123").is_ok());
        let dir = tmp_dir("badid");
        let ingest = ReplayIngest::new(&dir);
        let err = ingest
            .ingest_batch(ReplayBatch {
                batch_id: "b1".into(),
                tab_id: "t".into(),
                document_id: "../escape".into(),
                gap: false,
                events: vec![],
            })
            .unwrap_err();
        assert!(matches!(err, ReplayError::InvalidDocumentId(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn screenshots_roundtrip_and_wipe() {
        let dir = tmp_dir("shots");
        let store = ReplayStore::new(&dir);
        let p = store
            .write_screenshot("docABC123", 1, b"\xff\xd8fakejpeg")
            .unwrap();
        assert!(p.exists());
        let p2 = store
            .write_screenshot("docABC123", 2, b"\xff\xd8fakejpeg")
            .unwrap();
        assert!(p2.exists());

        let ingest = ReplayIngest::new(&dir);
        ingest
            .ingest_batch(ReplayBatch {
                batch_id: "b1".into(),
                tab_id: "t".into(),
                document_id: "docABC123".into(),
                gap: false,
                events: vec![ev("click", 1000)],
            })
            .unwrap();
        let stats = store.wipe().unwrap();
        assert!(stats.replay_files_removed >= 1);
        assert!(stats.screenshot_files_removed >= 2);
        assert!(!p.exists());
        assert!(!p2.exists());
        assert!(store.segments().unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_sessions_filters_and_timeline_assembles() {
        let dir = tmp_dir("query");
        let ingest = ReplayIngest::new(&dir);
        ingest
            .ingest_batch(ReplayBatch {
                batch_id: "b1".into(),
                tab_id: "tab-alpha".into(),
                document_id: "docAlpha1".into(),
                gap: false,
                events: vec![ev("click", 1000), ev("scroll", 1100)],
            })
            .unwrap();
        ingest
            .ingest_batch(ReplayBatch {
                batch_id: "b2".into(),
                tab_id: "tab-beta".into(),
                document_id: "docBeta1".into(),
                gap: false,
                events: vec![ev("input", 2000)],
            })
            .unwrap();
        let store = ingest.store();
        // Search by document id substring.
        let hits = store.search_sessions("alpha").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].document_id, "docAlpha1");
        // Search by tab id substring.
        let hits = store.search_sessions("BETA").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].tab_id, "tab-beta");
        // Empty query returns all, newest first.
        let all = store.search_sessions("").unwrap();
        assert_eq!(all.len(), 2);
        // Timeline: segment + events + (no) screenshot steps.
        let tl = store.timeline("docAlpha1").unwrap();
        assert_eq!(tl.segment.as_ref().unwrap().event_count, 2);
        assert_eq!(tl.events.len(), 2);
        assert!(tl.screenshot_steps.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn screenshot_steps_and_path_resolve() {
        let dir = tmp_dir("shots2");
        let store = ReplayStore::new(&dir);
        store.write_screenshot("docA", 1, b"jpeg1").unwrap();
        store.write_screenshot("docA", 3, b"jpeg3").unwrap();
        store.write_screenshot("docB", 2, b"jpeg2").unwrap();
        let steps = store.screenshot_steps("docA").unwrap();
        assert_eq!(steps, vec![1, 3]);
        assert!(store.screenshot_path("docA", 3).unwrap().exists());
        assert!(store.screenshot_path("docA", 2).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn events_since_returns_live_tail() {
        let dir = tmp_dir("tail");
        let ingest = ReplayIngest::new(&dir);
        ingest
            .ingest_batch(ReplayBatch {
                batch_id: "b1".into(),
                tab_id: "t".into(),
                document_id: "docTail1".into(),
                gap: false,
                events: vec![ev("click", 1), ev("scroll", 2), ev("input", 3)],
            })
            .unwrap();
        let tail = ingest.store().events_since("docTail1", 1).unwrap();
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].seq, 2);
        assert_eq!(tail[1].seq, 3);
        assert!(
            ingest
                .store()
                .events_since("docTail1", 99)
                .unwrap()
                .is_empty()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_sweep_removes_only_expired() {
        let dir = tmp_dir("retention");
        let ingest = ReplayIngest::new(&dir);
        // Fresh doc (now).
        ingest
            .ingest_batch(ReplayBatch {
                batch_id: "fresh".into(),
                tab_id: "t".into(),
                document_id: "docFresh1".into(),
                gap: false,
                events: vec![ev("click", now_ms())],
            })
            .unwrap();
        // Stale doc (10 days ago).
        let old_ts = now_ms() - 10 * 24 * 3600 * 1000;
        ingest
            .ingest_batch(ReplayBatch {
                batch_id: "old".into(),
                tab_id: "t".into(),
                document_id: "docOld1".into(),
                gap: false,
                events: vec![ev("click", old_ts)],
            })
            .unwrap();
        let stats = ingest
            .store()
            .retention_sweep(Duration::from_secs(7 * 24 * 3600))
            .unwrap();
        assert_eq!(stats.documents_removed, 1);
        let ids: Vec<String> = ingest
            .store()
            .segments()
            .unwrap()
            .iter()
            .map(|s| s.document_id.clone())
            .collect();
        assert_eq!(ids, vec!["docFresh1".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }
}
