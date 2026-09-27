//! agentcowork-storage — storage intelligence (P4.8, D9–D12 + G7, doc 49).
//!
//! The crate behind the "what is actually eating my disk?" cockpit. It is
//! intentionally **read + propose only**: scanning, indexing, hashing and
//! reporting happen here; every destructive action is emitted as a
//! `CleanupAction` proposal that the guard layer must ticket and execute
//! (recycle-bin-aware) — "sidecar proposes, Rust disposes" (ARCH/06).
//!
//! - `walk` — parallel work-stealing disk walker (`crossbeam-deque`) +
//!   u32-indexed tree arena with bottom-up size aggregation.
//! - `identity` — incarnation-aware platform file identity
//!   (`(volume, fileId, incarnation)` / `(dev, ino, nlink)`); explicit
//!   "unknown" instead of fabricated zeros (`ARCH/25-FILES.md` §2).
//! - `snapshot` — immutable snapshots behind `arc_swap`, zstd persistence.
//! - `treemap` — squarified treemap + per-dir aggregation + stable colors.
//! - `dedup` — 7-stage hash duplicate detection (size → xxHash3 prefix/suffix
//!   → BLAKE3, hardlink-aware, reflink-eligibility).
//! - `finder` — large-file finder (top-N by size/age + filters).
//! - `cleanup` — Guard-2-ticketed cleanup proposals (never executes).
//! - `search` — SQLite FTS5 filename index + debounced `notify` watcher.
//! - `health` — D12 drive-threshold monitoring (90% full flag).
//! - `usn` / `usn_reader` / `usn_winapi` — the W1 delta source: cursor + epoch
//!   discipline over the NTFS change journal, with gaps that always force a
//!   scoped rescan (`ARCH/21-WORLD-MODEL.md` §4).

pub mod checkpoint;
pub mod cleanup;
pub mod content;
pub mod dedup;
pub mod events;
pub mod finder;
pub mod hash_cache;
pub mod health;
pub mod identity;
pub mod pool;
pub mod pragmas;
pub mod search;
pub mod snapshot;
pub mod treemap;
pub mod trigram;
pub mod usn;
pub mod usn_reader;
#[cfg(windows)]
pub mod usn_winapi;
pub mod walk;

pub use checkpoint::{ChangeKind, CheckpointedFile, FsCheckpoint};
pub use cleanup::{
    CleanupAction, CleanupKind, propose_duplicate_cleanup, propose_large_files_cleanup,
};
pub use content::{
    ContentHit, ContentIndex, NoOcr, OcrEngine, OcrError, TesseractCli, extract_text, strip_html,
};
pub use dedup::{DedupOptions, DupCandidate, DupGroup, find_duplicates};
pub use events::{FileEvent, watch_events};
pub use finder::{FinderOptions, SortBy, find_large_files};
pub use health::{
    DriveStats, HealthStatus, check_health, drive_stats, health_from_stats, over_threshold,
};
pub use identity::{
    Discriminator, FileIdentity, FileKey, IdentityPlatform, IdentityPolicy, IdentityVerdict,
    Incarnation, identity_from_metadata,
};
pub use search::{Debouncer, SearchHit, SearchIndex, WatchHandle, watch};
pub use snapshot::{Snapshot, SnapshotStore};
pub use treemap::{TreemapRect, color_for, squarify, treemap_for_dir};
pub use usn::{
    CursorRow, DeltaOutcome, GapAnomaly, GapReason, GapSignal, JournalChunk, JournalError,
    JournalReader, JournalSink, JournalSource, RescanScope, UsnCursor, UsnDeltaSource, UsnReason,
    UsnRecord,
};
pub use walk::{
    Arena, FileNode, FileRecord, ROOT_ID, ScanOptions, build_arena, scan, scan_with_policy,
};

use thiserror::Error;

/// xxHash3-64 (twox-hash — xxhash-rust is BSL-1.0, doc 54 §1.2).
pub(crate) fn xxh3(data: &[u8]) -> u64 {
    twox_hash::xxhash3_64::Hasher::oneshot(data)
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("notify error: {0}")]
    Notify(String),
    #[error("{0}")]
    Other(String),
}
