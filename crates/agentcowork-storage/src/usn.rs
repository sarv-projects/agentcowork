//! W1 — USN journal (Windows) incremental index that scales.
//!
//! Windows NTFS exposes a change journal (`fsutil usn`) that lists file
//! deltas since a cursor *without rescans*. This module defines the typed
//! contract (what a real journal read yields + how the index consumes it),
//! the persisted cursor/epoch row, and the delta driver that turns a bounded
//! journal read into exactly one of: *records applied*, *no changes*, or *a
//! gap that forces a scoped rescan*.
//!
//! `ARCH/21-WORLD-MODEL.md` §4 and `ARCH/25-FILES.md` §3–§4 are the contract:
//! - **Cursor row** `(source, scope, epoch, cursor, observed_at)`; the epoch is
//!   the source generation (Windows = `JournalID`).
//! - **Epoch reset** discards the cursor and rescans the scope; records are
//!   never applied across epochs (`REQ-FILES-005`, `REQ-WORLD-004`).
//! - **Gaps** (journal deleted/truncated, requested USN below the retained
//!   history, replay/rewind) abort incremental application and force the
//!   smallest known scope to be rescanned, with a freshness anomaly recorded
//!   (`REQ-FILES-004`, `REQ-WORLD-005`). A gap is *never* reported as "no
//!   changes".
//!
//! The OS reader is behind the [`JournalReader`] trait so the cursor/epoch/gap
//! logic is testable on any host; `usn_winapi` (Windows-only) is the NTFS
//! implementation. The cross-platform fallback for non-Windows is the
//! notify-debounced walker in `events.rs`.
//!
//! **Honest ceiling:** the *runtime* behaviour of the NTFS path (journal open,
//! ioctl round-trips, gap detection against a live journal) is only provable
//! on Windows with an acceptance record; the driver logic around it is
//! unit-tested everywhere.

use std::path::Path;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Why a file changed (the USN reason vocabulary, mapped to our events).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsnReason {
    Create,
    Delete,
    RenameOld,
    RenameNew,
    DataOverwrite,
    DataExtend,
    DataTruncate,
}

/// One journal record (the minimal slice the index needs).
#[derive(Debug, Clone, PartialEq)]
pub struct UsnRecord {
    /// Monotonic journal cursor (the upper bound of the read).
    pub usn: u64,
    pub reason: UsnReason,
    pub path: PathBuf,
    /// The journal's file reference number — the event-side identity evidence
    /// (`ARCH/25-FILES.md` §2: "USN FRN for event correlation"). On NTFS this
    /// 64-bit value is `sequence:16 | mft_index:48`, so it is already
    /// incarnation-safe and can be matched against
    /// [`crate::identity::FileIdentity`]'s NTFS id. `None` when the source did
    /// not supply one.
    pub file_ref: Option<u64>,
}

impl UsnRecord {
    /// Build a record from the parts a reader has.
    pub fn new(usn: u64, reason: UsnReason, path: PathBuf, file_ref: Option<u64>) -> Self {
        Self {
            usn,
            reason,
            path,
            file_ref,
        }
    }
}

/// Who consumes journal deltas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalSource {
    /// Real NTFS journal (Windows; platform-gated reader).
    Ntfs,
    /// Cross-platform debounced walker fallback (notify events).
    DebouncedWatch,
}

impl JournalSource {
    /// The stable `source` label stored in the cursor row.
    pub const fn as_str(self) -> &'static str {
        match self {
            JournalSource::Ntfs => "ntfs-usn",
            JournalSource::DebouncedWatch => "debounced-watch",
        }
    }
}

#[allow(clippy::derivable_impls)]
impl Default for JournalSource {
    fn default() -> Self {
        JournalSource::DebouncedWatch
    }
}

/// Who consumes journal deltas.
pub trait JournalSink {
    /// Apply a journal delta batch. Returns the next cursor.
    fn apply(&mut self, batch: &[UsnRecord]) -> Result<u64, String>;
}

/// The in-memory cursor holder + ingest buffer. The index owns one of these;
/// the backend (NTFS reader or debounced walker) feeds it.
#[derive(Debug, Default)]
pub struct UsnCursor {
    pub source: JournalSource,
    pub next_usn: u64,
    pub applied: u64,
}

impl UsnCursor {
    pub fn new(source: JournalSource) -> Self {
        Self {
            source,
            next_usn: 0,
            applied: 0,
        }
    }

    /// Feed a batch whose first `usn` is > `next_usn` (journal ordering).
    /// Records are consumed in order; any gap aborts the batch (honest: a
    /// missing journal range must never be silently applied).
    pub fn ingest(&mut self, batch: &[UsnRecord]) -> Result<u64, String> {
        if batch.iter().any(|r| r.usn <= self.next_usn) {
            return Err("journal record at/below cursor skipped (duplicate or rewind)".into());
        }
        let Some(last) = batch.last() else {
            return Ok(self.next_usn);
        };
        self.next_usn = last.usn;
        self.applied += batch.len() as u64;
        Ok(self.next_usn)
    }
}

// ---------------------------------------------------------------------------
// Cursor/epoch row + gap policy (ARCH/21-WORLD-MODEL.md §4)
// ---------------------------------------------------------------------------

/// Why a collector must stop applying deltas and rescan instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GapReason {
    /// The requested USN is below the journal's `FirstUsn`: the history we
    /// needed is gone (the journal wrapped). This is the NTFS
    /// "starting point too old" condition (`ERROR_INVALID_PARAMETER` on
    /// `FSCTL_READ_USN_JOURNAL`).
    StartingPointTooOld,
    /// The journal was deleted or truncated while we were reading it
    /// (`ERROR_JOURNAL_DELETE_IN_PROGRESS`, `ERROR_HANDLE_EOF` mid-stream).
    JournalDeleted,
    /// The source generation changed: the journal id is not the one our cursor
    /// was written under. The cursor is meaningless under a new epoch.
    EpochChanged,
    /// A batch contained a record at or below the cursor (duplicate/replay) or
    /// a rewind, so the batch is refused rather than partially applied.
    Replay,
    /// The OS refused the read (access denied, journal inactive, device
    /// error). Under `REQ-WORLD-010` this is the non-admin fallback signal —
    /// recorded, surfaced, never silently elevated.
    SourceUnavailable,
}

impl GapReason {
    /// Stable machine-readable label for the anomaly event.
    pub const fn as_str(self) -> &'static str {
        match self {
            GapReason::StartingPointTooOld => "starting_point_too_old",
            GapReason::JournalDeleted => "journal_deleted",
            GapReason::EpochChanged => "epoch_changed",
            GapReason::Replay => "replay_or_rewind",
            GapReason::SourceUnavailable => "source_unavailable",
        }
    }
}

/// The smallest scope a gap forces us to rescan (`REQ-FILES-004`: a full
/// volume when a smaller scope was known is itself a defect).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RescanScope {
    /// A whole volume — the granularity a USN journal reader actually knows.
    Volume { volume: String },
    /// One directory (inotify / FSEvents parity lanes).
    Directory { path: PathBuf },
    /// One path (a write lease that must be re-validated).
    Path { path: PathBuf },
}

/// A gap signal: the reason, the bounded rescan it forces, and the detail a
/// consumer needs to log or surface it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GapSignal {
    pub reason: GapReason,
    pub rescan: RescanScope,
    pub detail: String,
    /// The cursor that was discarded (0 = none yet).
    pub discarded_cursor: u64,
}

/// The observable freshness-anomaly event emitted for **every** gap
/// (`ARCH/21-WORLD-MODEL.md` §4: "record a freshness anomaly event"; a silent
/// gap is a defect).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GapAnomaly {
    pub source: String,
    pub scope: String,
    pub reason: GapReason,
    pub rescan: RescanScope,
    pub discarded_cursor: u64,
    pub observed_at: u64,
    pub detail: String,
}

impl GapAnomaly {
    /// The projection a `30-EVENTS` consumer can publish.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": "freshness_anomaly",
            "class": "collector_gap",
            "source": self.source,
            "scope": self.scope,
            "reason": self.reason.as_str(),
            "rescan": match &self.rescan {
                RescanScope::Volume { volume } => serde_json::json!({ "volume": volume }),
                RescanScope::Directory { path } => serde_json::json!({ "directory": path }),
                RescanScope::Path { path } => serde_json::json!({ "path": path }),
            },
            "discardedCursor": self.discarded_cursor,
            "observedAt": self.observed_at,
            "detail": self.detail,
        })
    }
}

/// A reader-side failure, typed so the driver can turn it into either a gap or
/// a clean "no changes".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    /// `from` is below the oldest USN the journal still holds.
    #[error("journal history dropped: usn {from} is below the retained first {first}")]
    StartingPointTooOld { from: u64, first: u64 },
    /// The journal was deleted or truncated under us.
    #[error("journal deleted or truncated")]
    JournalDeleted,
    /// The journal id moved: our cursor belongs to a previous epoch.
    #[error("journal epoch changed: expected {expected}, found {found}")]
    EpochChanged { expected: u64, found: u64 },
    /// The read returned no data and no progress (nothing to apply).
    #[error("journal caught up")]
    CaughtUp,
    /// The OS refused the read; non-admin fallback territory.
    #[error("journal unavailable: {0}")]
    Unavailable(String),
}

/// One bounded read of the journal.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JournalChunk {
    /// The records read, strictly increasing in `usn` and all `> from`.
    pub records: Vec<UsnRecord>,
    /// Where to resume: the journal's own cursor watermark after this read.
    pub next_usn: u64,
    /// `false` when the reader stopped because `max_records` was reached (more
    /// deltas are waiting); `true` when it drained the journal.
    pub exhausted: bool,
}

/// The OS boundary: a bounded, read-only view of a change journal.
///
/// Implemented for real by `usn_winapi::NtfsJournalReader` (Windows) and faked
/// in tests so the cursor/epoch/gap logic is provable on any host.
pub trait JournalReader {
    /// Which source this reader is bound to (stored in the cursor row).
    fn source(&self) -> JournalSource;
    /// The scope this reader covers (a volume root, a directory).
    fn scope(&self) -> String;
    /// The source generation — the epoch (Windows = `UsnJournalID`). A change
    /// invalidates any stored cursor.
    fn epoch(&self) -> u64;
    /// The next USN the journal can serve (the read watermark).
    fn next_usn(&mut self) -> Result<u64, JournalError>;
    /// The oldest USN still retained; anything below it is a gap.
    fn first_usn(&mut self) -> Result<u64, JournalError>;
    /// Read deltas with `usn > from`, stopping at `max_records`.
    ///
    /// `from == 0` means "no cursor yet": start at the oldest retained USN, so
    /// a first inventory can never be mistaken for lost history.
    ///
    /// Implementations must return [`JournalError::StartingPointTooOld`] when a
    /// real `from` fell off the retained history and
    /// [`JournalError::CaughtUp`] when there is nothing to read — never an
    /// empty success that would look like "no changes" for a lost history.
    fn read_from(&mut self, from: u64, max_records: usize) -> Result<JournalChunk, JournalError>;
}

/// The persisted cursor row (`ARCH/25-FILES.md` §4,
/// `ARCH/21-WORLD-MODEL.md` §4): `(source, scope, epoch, cursor, observed_at)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorRow {
    /// Stable source label (`"ntfs-usn"` / `"debounced-watch"`).
    pub source: String,
    /// The covered scope (a volume root or a directory).
    pub scope: String,
    /// The source generation the cursor belongs to. `0` = not established yet
    /// (a first scan, no deltas to resume from).
    pub epoch: u64,
    /// Resume point: the next USN to read.
    pub cursor: u64,
    /// Unix seconds of the last successful observation.
    pub observed_at: u64,
    /// The journal's read watermark at the last observation (`0` = unknown).
    pub journal_next: u64,
    /// The oldest USN still retained at the last observation (`0` = unknown).
    pub journal_first: u64,
    /// `true` while a gap is unresolved. While open, completeness must not be
    /// claimed and no delta may be applied.
    pub gap_open: bool,
    /// Why the open gap happened (the last one seen).
    pub last_gap: Option<GapReason>,
    /// Cumulative gap count, so a gap is observable even after it is closed.
    pub anomalies: u64,
}

impl CursorRow {
    /// A fresh row with no established epoch.
    pub fn new(source: JournalSource, scope: impl Into<String>) -> Self {
        Self {
            source: source.as_str().to_string(),
            scope: scope.into(),
            epoch: 0,
            cursor: 0,
            observed_at: 0,
            journal_next: 0,
            journal_first: 0,
            gap_open: false,
            last_gap: None,
            anomalies: 0,
        }
    }

    /// The row as loaded from disk for `source`/`scope` (defaults when absent).
    pub fn load_from(
        path: &Path,
        source: JournalSource,
        scope: &str,
    ) -> Result<Self, crate::StorageError> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::new(source, scope)),
            Err(e) => Err(e.into()),
        }
    }

    /// Persist the row (caller-chosen path; the crate picks no location).
    pub fn save_to(&self, path: &Path) -> Result<(), crate::StorageError> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

/// What one [`UsnDeltaSource::poll`] produced.
#[derive(Debug, Clone, PartialEq)]
pub enum DeltaOutcome {
    /// The cursor is current: no deltas, no gap.
    Idle,
    /// Records applied in order; the cursor advanced to `cursor`.
    Applied {
        records: Vec<UsnRecord>,
        cursor: u64,
    },
    /// The stream is incomplete. No record was applied; the smallest known
    /// scope must be rescanned before the cursor may be trusted again.
    Gap(GapSignal),
}

/// The W1 delta source: drives a [`JournalReader`] into an
/// applied / idle / gap verdict while maintaining the persisted cursor row.
///
/// Guarantees, all of which are testable without an OS:
/// * a record is applied only while no gap is open and only when every
///   `usn > cursor` and strictly increasing;
/// * an epoch change, a dropped history, a deleted journal or a rewind all
///   produce a [`DeltaOutcome::Gap`] — never `Idle`;
/// * every gap increments [`CursorRow::anomalies`], sets `gap_open` and records
///   `last_gap`, so it stays observable after it is closed;
/// * while `gap_open`, `poll` keeps returning the gap and applies nothing.
pub struct UsnDeltaSource<R: JournalReader> {
    reader: R,
    state: CursorRow,
    cursor: UsnCursor,
    max_batch: usize,
    now: u64,
}

impl<R: JournalReader> UsnDeltaSource<R> {
    /// Wrap a reader, resuming from a persisted `row`.
    pub fn new(reader: R, row: CursorRow) -> Self {
        let source = if row.source == JournalSource::Ntfs.as_str() {
            JournalSource::Ntfs
        } else {
            JournalSource::DebouncedWatch
        };
        let cursor = UsnCursor {
            source,
            next_usn: row.cursor,
            applied: 0,
        };
        Self {
            reader,
            state: row,
            cursor,
            max_batch: 4096,
            now: 0,
        }
    }

    /// Wrap a reader with a fresh (no cursor) row for its own scope.
    pub fn fresh(reader: R) -> Self {
        let row = CursorRow::new(reader.source(), reader.scope());
        Self::new(reader, row)
    }

    /// Override the per-read record budget (bounded by construction).
    pub fn with_max_batch(mut self, max_batch: usize) -> Self {
        self.max_batch = max_batch.max(1);
        self
    }

    /// Override the clock (so tests get deterministic `observed_at`).
    pub fn with_clock(mut self, now: u64) -> Self {
        self.now = now;
        self
    }

    /// The live cursor row (persist this to resume after a restart).
    pub fn state(&self) -> &CursorRow {
        &self.state
    }

    /// The reader (for diagnostics/consent records).
    pub fn reader(&self) -> &R {
        &self.reader
    }

    fn timestamp(&self) -> u64 {
        if self.now != 0 {
            return self.now;
        }
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Record a gap: discard the cursor, flag the row, count the anomaly.
    fn open_gap(&mut self, reason: GapReason, detail: String) -> DeltaOutcome {
        let discarded = self.state.cursor;
        self.state.gap_open = true;
        self.state.last_gap = Some(reason);
        self.state.anomalies += 1;
        self.state.cursor = 0;
        self.cursor.next_usn = 0;
        self.state.observed_at = self.timestamp();
        DeltaOutcome::Gap(GapSignal {
            reason,
            rescan: RescanScope::Volume {
                volume: self.state.scope.clone(),
            },
            detail,
            discarded_cursor: discarded,
        })
    }

    /// The anomaly event for an open gap, as a `30-EVENTS` projection.
    pub fn anomaly(&self, signal: &GapSignal) -> GapAnomaly {
        GapAnomaly {
            source: self.state.source.clone(),
            scope: self.state.scope.clone(),
            reason: signal.reason,
            rescan: signal.rescan.clone(),
            discarded_cursor: signal.discarded_cursor,
            observed_at: self.state.observed_at,
            detail: signal.detail.clone(),
        }
    }

    /// Read once. Applies at most `max_batch` records and never applies across
    /// a gap.
    pub fn poll(&mut self) -> Result<DeltaOutcome, JournalError> {
        // 1. An open gap outranks everything: nothing may be applied until the
        //    scope has been rescanned (`acknowledge_rescan`).
        if self.state.gap_open {
            let reason = self.state.last_gap.unwrap_or(GapReason::SourceUnavailable);
            return Ok(self.open_gap(
                reason,
                "gap still open: incremental application aborted pending rescan".into(),
            ));
        }

        // 2. Epoch check. A journal id we did not write the cursor under
        //    invalidates the cursor outright (REQ-WORLD-004).
        let epoch = self.reader.epoch();
        if self.state.epoch != 0 && epoch != self.state.epoch {
            let (expected, found) = (self.state.epoch, epoch);
            return Ok(self.open_gap(
                GapReason::EpochChanged,
                format!(
                    "epoch {expected:#x} → {found:#x}: cursor discarded, scope rescan required"
                ),
            ));
        }
        if self.state.epoch == 0 {
            // First contact: adopt the epoch but keep the cursor at 0 so the
            // caller performs the initial inventory before applying deltas.
            self.state.epoch = epoch;
        }

        // 3. Refresh the journal watermarks, and detect a dropped history.
        let next = self.reader.next_usn()?;
        let first = self.reader.first_usn()?;
        self.state.journal_next = next;
        self.state.journal_first = first;
        if first > 0 && self.state.cursor > 0 && self.state.cursor < first {
            let (from, f) = (self.state.cursor, first);
            return Ok(self.open_gap(
                GapReason::StartingPointTooOld,
                format!("cursor {from} is below the retained first usn {f}: history dropped"),
            ));
        }

        // 4. Read a bounded batch.
        let from = self.state.cursor;
        let chunk = match self.reader.read_from(from, self.max_batch) {
            Ok(c) => c,
            // Caught up is a legitimate "no changes", never a gap.
            Err(JournalError::CaughtUp) => {
                self.state.observed_at = self.timestamp();
                return Ok(DeltaOutcome::Idle);
            }
            Err(JournalError::StartingPointTooOld { from, first }) => {
                return Ok(self.open_gap(
                    GapReason::StartingPointTooOld,
                    format!("usn {from} is below the retained first {first}: history dropped"),
                ));
            }
            Err(JournalError::JournalDeleted) => {
                return Ok(self.open_gap(
                    GapReason::JournalDeleted,
                    "journal deleted or truncated mid-read".into(),
                ));
            }
            Err(JournalError::EpochChanged { expected, found }) => {
                return Ok(self.open_gap(
                    GapReason::EpochChanged,
                    format!(
                        "epoch {expected:#x} → {found:#x}: cursor discarded, scope rescan required"
                    ),
                ));
            }
            Err(JournalError::Unavailable(e)) => {
                // Recorded and surfaced, never silently elevated
                // (REQ-WORLD-010); the caller falls back to a bounded walk.
                return Ok(self.open_gap(
                    GapReason::SourceUnavailable,
                    format!("journal read refused: {e}"),
                ));
            }
        };

        if chunk.records.is_empty() {
            self.state.observed_at = self.timestamp();
            return Ok(DeltaOutcome::Idle);
        }

        // 5. Validate before applying. Records at/below the cursor, or not
        //    strictly increasing, are a replay/rewind: refuse the whole batch
        //    rather than apply part of it (REQ-FILES-005, usn.rs:77-90).
        let mut prev = from;
        for r in &chunk.records {
            if r.usn <= prev {
                return Ok(self.open_gap(
                    GapReason::Replay,
                    format!(
                        "record usn {} is at/below the expected sequence (> {prev}): batch refused",
                        r.usn
                    ),
                ));
            }
            prev = r.usn;
        }

        // 6. Apply. The batch is ordered and strictly above the cursor, so the
        //    existing gap-abort ingest cannot fail; a failure would still be a
        //    replay, which is a gap, not a silent drop.
        let records = chunk.records.clone();
        match self.cursor.ingest(&records) {
            Ok(_) => {}
            Err(e) => {
                return Ok(self.open_gap(GapReason::Replay, e));
            }
        }
        let applied_to = if chunk.next_usn > self.cursor.next_usn {
            chunk.next_usn
        } else {
            self.cursor.next_usn
        };
        self.cursor.next_usn = applied_to;
        self.state.cursor = applied_to;
        self.state.observed_at = self.timestamp();
        Ok(DeltaOutcome::Applied {
            records,
            cursor: applied_to,
        })
    }

    /// Close an open gap after the caller has rescanned the scope up to
    /// `rescanned_to`. The epoch is kept (the journal did not change; we
    /// re-learned its position), the anomaly stays on the row, and deltas may
    /// be applied again.
    pub fn acknowledge_rescan(&mut self, rescanned_to: u64) {
        self.state.cursor = rescanned_to;
        self.cursor.next_usn = rescanned_to;
        self.state.gap_open = false;
        self.state.observed_at = self.timestamp();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(usn: u64) -> UsnRecord {
        UsnRecord {
            usn,
            reason: UsnReason::DataOverwrite,
            path: PathBuf::from("/tmp/f.txt"),
            file_ref: Some(usn),
        }
    }

    // --- legacy UsnCursor contract (unchanged) ------------------------------

    #[test]
    fn cursor_advances_in_order() {
        let mut c = UsnCursor::new(JournalSource::DebouncedWatch);
        let batch = vec![rec(1), rec(2), rec(3)];
        assert_eq!(c.ingest(&batch).unwrap(), 3);
        assert_eq!(c.applied, 3);
        assert_eq!(c.ingest(&[rec(5), rec(7)]).unwrap(), 7);
        assert_eq!(c.applied, 5);
    }

    #[test]
    fn gap_aborts() {
        let mut c = UsnCursor::new(JournalSource::DebouncedWatch);
        c.ingest(&[rec(10)]).unwrap();
        let err = c.ingest(&[rec(5)]).unwrap_err();
        assert!(err.contains("at/below"), "{err}");
        // Cursor unchanged after the failed batch.
        assert_eq!(c.next_usn, 10);
    }

    #[test]
    fn empty_batch_is_noop() {
        let mut c = UsnCursor::new(JournalSource::DebouncedWatch);
        assert_eq!(c.ingest(&[]).unwrap(), 0);
    }

    // --- the fake journal the driver tests run against ---------------------

    /// A scriptable, in-memory journal. `records` is the full retained history;
    /// `epoch` is the journal id; `first_usn` is the retained floor.
    struct FakeJournal {
        epoch: u64,
        first: u64,
        next: u64,
        records: Vec<UsnRecord>,
        source: JournalSource,
        scope: String,
        /// One-shot scripted failure for the next read.
        fail: Option<JournalError>,
        /// One-shot scripted batch, served verbatim (to inject a replay/rewind).
        forced: Option<Vec<UsnRecord>>,
    }

    impl FakeJournal {
        fn new() -> Self {
            Self {
                epoch: 0xABCD_0001,
                first: 0,
                next: 0,
                records: Vec::new(),
                source: JournalSource::Ntfs,
                scope: "C:\\".to_string(),
                fail: None,
                forced: None,
            }
        }

        fn with_history(first: u64, usns: &[u64]) -> Self {
            let mut j = Self::new();
            j.first = first;
            j.records = usns.iter().map(|&u| rec(u)).collect();
            j.next = usns.last().copied().unwrap_or(first);
            j
        }

        fn fail_next(&mut self, e: JournalError) {
            self.fail = Some(e);
        }

        /// Serve this exact batch on the next read, however it is ordered.
        fn serve_next(&mut self, records: Vec<UsnRecord>) {
            self.forced = Some(records);
        }
    }

    impl JournalReader for FakeJournal {
        fn source(&self) -> JournalSource {
            self.source
        }
        fn scope(&self) -> String {
            self.scope.clone()
        }
        fn epoch(&self) -> u64 {
            self.epoch
        }
        fn next_usn(&mut self) -> Result<u64, JournalError> {
            Ok(self.next)
        }
        fn first_usn(&mut self) -> Result<u64, JournalError> {
            Ok(self.first)
        }
        fn read_from(
            &mut self,
            from: u64,
            max_records: usize,
        ) -> Result<JournalChunk, JournalError> {
            if let Some(e) = self.fail.take() {
                return Err(e);
            }
            if let Some(forced) = self.forced.take() {
                let next_usn = forced.last().map(|r| r.usn).unwrap_or(from);
                return Ok(JournalChunk {
                    records: forced,
                    next_usn,
                    exhausted: true,
                });
            }
            // `from == 0` = "no cursor yet": serve the oldest retained USN and
            // onwards. Only a real cursor that fell off the history is a gap.
            let floor = if from == 0 {
                self.first
            } else {
                if self.first > 0 && from < self.first {
                    return Err(JournalError::StartingPointTooOld {
                        from,
                        first: self.first,
                    });
                }
                from + 1
            };
            let mut out = Vec::new();
            let mut prev = from;
            for r in self.records.iter() {
                if r.usn >= floor {
                    out.push(r.clone());
                    prev = r.usn;
                    if out.len() >= max_records {
                        return Ok(JournalChunk {
                            next_usn: prev,
                            records: out,
                            exhausted: false,
                        });
                    }
                }
            }
            if out.is_empty() {
                return Err(JournalError::CaughtUp);
            }
            Ok(JournalChunk {
                next_usn: self.next.max(prev),
                records: out,
                exhausted: true,
            })
        }
    }

    fn applied(out: DeltaOutcome) -> (Vec<UsnRecord>, u64) {
        match out {
            DeltaOutcome::Applied { records, cursor } => (records, cursor),
            other => panic!("expected Applied, got {other:?}"),
        }
    }

    fn gap(out: DeltaOutcome) -> GapSignal {
        match out {
            DeltaOutcome::Gap(g) => g,
            other => panic!("expected Gap, got {other:?}"),
        }
    }

    // --- driver: the happy path ---------------------------------------------

    #[test]
    fn applies_deltas_and_advances_the_persisted_cursor() {
        let j = FakeJournal::with_history(0, &[10, 11, 12]);
        let mut s = UsnDeltaSource::fresh(j).with_clock(1_700_000_000);
        let (recs, cursor) = applied(s.poll().unwrap());
        assert_eq!(recs.len(), 3);
        assert_eq!(cursor, 12);

        let row = s.state();
        assert_eq!(row.source, "ntfs-usn");
        assert_eq!(row.scope, "C:\\");
        assert_eq!(row.epoch, 0xABCD_0001);
        assert_eq!(row.cursor, 12);
        assert_eq!(row.observed_at, 1_700_000_000);
        assert!(!row.gap_open);
        assert_eq!(row.anomalies, 0);
        assert_eq!(row.journal_next, 12);
    }

    #[test]
    fn caught_up_is_idle_not_a_gap() {
        let mut s = UsnDeltaSource::fresh(FakeJournal::with_history(0, &[10]));
        applied(s.poll().unwrap());
        match s.poll().unwrap() {
            DeltaOutcome::Idle => {}
            other => panic!("expected Idle, got {other:?}"),
        }
        assert_eq!(s.state().anomalies, 0);
        assert!(!s.state().gap_open);
    }

    #[test]
    fn batch_budget_bounds_one_read() {
        let j = FakeJournal::with_history(0, &[1, 2, 3, 4, 5]);
        let mut s = UsnDeltaSource::fresh(j).with_max_batch(2);
        let (recs, cursor) = applied(s.poll().unwrap());
        assert_eq!(recs.len(), 2);
        assert_eq!(cursor, 2);
        let (recs, cursor) = applied(s.poll().unwrap());
        assert_eq!(recs.len(), 2);
        assert_eq!(cursor, 4);
    }

    // --- driver: restart resumes from the persisted row ---------------------

    #[test]
    fn restart_resumes_from_the_persisted_row() {
        let mut row = CursorRow::new(JournalSource::Ntfs, "C:\\");
        row.epoch = 0xABCD_0001;
        row.cursor = 12;
        row.observed_at = 1_699_999_999;

        let j = FakeJournal::with_history(0, &[10, 11, 12, 13, 14]);
        let mut s = UsnDeltaSource::new(j, row).with_clock(1_700_000_500);
        let (recs, cursor) = applied(s.poll().unwrap());
        // Only what happened after the stored cursor.
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].usn, 13);
        assert_eq!(cursor, 14);
        assert_eq!(s.state().observed_at, 1_700_000_500);
    }

    #[test]
    fn cursor_row_round_trips_through_disk() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-usn-cursor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cursor.json");

        // A missing file is a fresh row, not an error.
        let fresh = CursorRow::load_from(&path, JournalSource::Ntfs, "C:\\").unwrap();
        assert_eq!(fresh.cursor, 0);
        assert!(!fresh.gap_open);

        let mut row = fresh;
        row.epoch = 42;
        row.cursor = 7;
        row.gap_open = true;
        row.last_gap = Some(GapReason::JournalDeleted);
        row.anomalies = 3;
        row.journal_next = 9;
        row.journal_first = 4;
        row.observed_at = 123;
        row.save_to(&path).unwrap();

        let back = CursorRow::load_from(&path, JournalSource::Ntfs, "C:\\").unwrap();
        assert_eq!(back, row, "a restart must resume the same epoch + cursor");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- driver: gaps never look like "no changes" -------------------------

    #[test]
    fn epoch_change_discards_the_cursor_and_records_a_gap() {
        let mut row = CursorRow::new(JournalSource::Ntfs, "C:\\");
        row.epoch = 0x1111;
        row.cursor = 50;

        let mut j = FakeJournal::with_history(0, &[50, 51]);
        j.epoch = 0x2222; // the journal was recreated
        let mut s = UsnDeltaSource::new(j, row).with_clock(7);

        let g = gap(s.poll().unwrap());
        assert_eq!(g.reason, GapReason::EpochChanged);
        assert_eq!(g.discarded_cursor, 50);
        assert_eq!(
            g.rescan,
            RescanScope::Volume {
                volume: "C:\\".into()
            }
        );
        assert!(
            g.detail.contains("0x1111") && g.detail.contains("0x2222"),
            "{g:?}"
        );

        let st = s.state();
        assert!(st.gap_open);
        assert_eq!(st.cursor, 0, "stale cursor must not survive the reset");
        assert_eq!(st.last_gap, Some(GapReason::EpochChanged));
        assert_eq!(st.anomalies, 1);
    }

    #[test]
    fn starting_point_too_old_is_a_gap_not_idle() {
        let mut row = CursorRow::new(JournalSource::Ntfs, "C:\\");
        row.epoch = 0xABCD_0001;
        row.cursor = 5; // long since overwritten

        // The journal only retains from 500 now.
        let j = FakeJournal::with_history(500, &[500, 501]);
        let mut s = UsnDeltaSource::new(j, row);
        let g = gap(s.poll().unwrap());
        assert_eq!(g.reason, GapReason::StartingPointTooOld);
        assert!(s.state().gap_open);
        assert_eq!(s.state().anomalies, 1);
    }

    #[test]
    fn reader_side_starting_point_too_old_also_opens_a_gap() {
        // The same condition reported by the read itself (the NTFS
        // `ERROR_INVALID_PARAMETER` path) must not look like a clean read.
        let mut j = FakeJournal::with_history(0, &[10]);
        j.fail_next(JournalError::StartingPointTooOld {
            from: 3,
            first: 900,
        });
        let mut s = UsnDeltaSource::fresh(j);
        let g = gap(s.poll().unwrap());
        assert_eq!(g.reason, GapReason::StartingPointTooOld);
        assert!(g.detail.contains("900"), "{g:?}");
    }

    #[test]
    fn journal_deleted_is_a_gap() {
        let mut j = FakeJournal::with_history(0, &[10]);
        j.fail_next(JournalError::JournalDeleted);
        let mut s = UsnDeltaSource::fresh(j);
        assert_eq!(gap(s.poll().unwrap()).reason, GapReason::JournalDeleted);
        assert!(s.state().gap_open);
    }

    #[test]
    fn source_unavailable_is_recorded_not_silently_elevated() {
        let mut j = FakeJournal::with_history(0, &[10]);
        j.fail_next(JournalError::Unavailable("ERROR_ACCESS_DENIED".into()));
        let mut s = UsnDeltaSource::fresh(j);
        let g = gap(s.poll().unwrap());
        assert_eq!(g.reason, GapReason::SourceUnavailable);
        assert!(g.detail.contains("ERROR_ACCESS_DENIED"), "{g:?}");
        assert_eq!(s.state().anomalies, 1);
    }

    #[test]
    fn replayed_batch_is_refused_whole() {
        // The reader hands back a batch that goes backwards relative to the
        // stored cursor — a duplicate/replay, which must never be applied.
        let mut j = FakeJournal::with_history(0, &[10, 11, 12]);
        let mut row = CursorRow::new(JournalSource::Ntfs, "C:\\");
        row.epoch = j.epoch;
        row.cursor = 11;
        j.serve_next(vec![rec(9), rec(11)]);
        let mut s = UsnDeltaSource::new(j, row);
        let g = gap(s.poll().unwrap());
        assert_eq!(g.reason, GapReason::Replay);
        assert!(s.state().gap_open);
        assert_eq!(s.state().cursor, 0);
    }

    #[test]
    fn out_of_order_batch_is_refused_whole() {
        let mut j = FakeJournal::with_history(0, &[10, 11, 12]);
        j.serve_next(vec![rec(20), rec(19)]);
        let mut s = UsnDeltaSource::fresh(j);
        assert_eq!(gap(s.poll().unwrap()).reason, GapReason::Replay);
        assert_eq!(s.state().cursor, 0);
    }

    #[test]
    fn a_gap_blocks_incremental_application_until_rescanned() {
        let mut j = FakeJournal::with_history(0, &[10, 11, 12, 13]);
        j.fail_next(JournalError::JournalDeleted);
        let mut s = UsnDeltaSource::fresh(j).with_clock(5);
        assert_eq!(gap(s.poll().unwrap()).reason, GapReason::JournalDeleted);

        // The journal is healthy again, but nothing may be applied while the
        // gap is open — and it must keep reporting the gap, not "no changes".
        let again = s.poll().unwrap();
        assert_eq!(gap(again).reason, GapReason::JournalDeleted);
        assert_eq!(s.state().anomalies, 2);
        assert_eq!(s.state().cursor, 0);

        // After a scoped rescan the delta stream resumes at the rescan point.
        s.acknowledge_rescan(11);
        let (recs, cursor) = applied(s.poll().unwrap());
        assert_eq!(recs.len(), 2);
        assert_eq!(cursor, 13);
        assert!(!s.state().gap_open);
        // The anomaly stays on the row: a gap is observable after it closes.
        assert_eq!(s.state().anomalies, 2);
        assert_eq!(s.state().last_gap, Some(GapReason::JournalDeleted));
    }

    #[test]
    fn first_contact_adopts_the_epoch_without_applying_old_deltas() {
        // No stored row: the epoch is learned, the cursor stays at 0, and the
        // initial inventory is the caller's job.
        let j = FakeJournal::with_history(0, &[10, 11]);
        let mut s = UsnDeltaSource::fresh(j);
        let (recs, cursor) = applied(s.poll().unwrap());
        assert_eq!(cursor, 11);
        assert_eq!(s.state().epoch, 0xABCD_0001);
        assert_eq!(recs.len(), 2);
    }

    // --- observability -------------------------------------------------------

    #[test]
    fn gap_anomaly_renders_an_event_projection() {
        let mut j = FakeJournal::with_history(0, &[10]);
        j.fail_next(JournalError::JournalDeleted);
        let mut s = UsnDeltaSource::fresh(j).with_clock(1_700_000_000);
        let outcome = s.poll().unwrap();
        let signal = gap(outcome);
        let event = s.anomaly(&signal);
        assert_eq!(event.reason, GapReason::JournalDeleted);
        assert_eq!(event.observed_at, 1_700_000_000);

        let json = event.to_json();
        assert_eq!(json["kind"], "freshness_anomaly");
        assert_eq!(json["source"], "ntfs-usn");
        assert_eq!(json["reason"], "journal_deleted");
        assert_eq!(json["rescan"]["volume"], "C:\\");
    }

    #[test]
    fn gap_reasons_have_stable_labels() {
        let all = [
            GapReason::StartingPointTooOld,
            GapReason::JournalDeleted,
            GapReason::EpochChanged,
            GapReason::Replay,
            GapReason::SourceUnavailable,
        ];
        let labels: Vec<&str> = all.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "starting_point_too_old",
                "journal_deleted",
                "epoch_changed",
                "replay_or_rewind",
                "source_unavailable"
            ]
        );
    }

    #[test]
    fn journal_error_messages_name_the_boundary() {
        assert!(
            JournalError::StartingPointTooOld { from: 1, first: 9 }
                .to_string()
                .contains("below the retained first")
        );
        assert!(
            JournalError::JournalDeleted
                .to_string()
                .contains("truncated")
        );
        assert!(JournalError::CaughtUp.to_string().contains("caught up"));
    }

    #[test]
    fn records_carry_the_usn_file_reference_number() {
        let r = UsnRecord::new(
            5,
            UsnReason::Create,
            PathBuf::from("C:/a.txt"),
            Some(0x0002_0000),
        );
        assert_eq!(r.file_ref, Some(0x0002_0000));
    }
}
