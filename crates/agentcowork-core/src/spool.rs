//! P64.11 / P69.G5 — the **content-addressed tool-output spool**.
//!
//! A large tool result used to live only in renderer memory: the chat card
//! decided a payload was "spooled", drew a blob card over the in-memory
//! string, and nothing was ever written to disk. This module is the real
//! thing — the kernel owns the bytes, the reference, and the retention
//! policy, exactly as [`ARCH/05-TOKEN-ECONOMY.md`](../../ARCH/05-TOKEN-ECONOMY.md)
//! §5.12 and [`ARCH/UI.md`](../../ARCH/UI.md) §3 specify.
//!
//! ## On-disk layout
//!
//! ```text
//! <data_dir>/spool/<sha256>.blob
//! ```
//!
//! One file per distinct payload, named by the hex SHA-256 of its own
//! contents. There is **no index file and no sidecar**: the file name *is*
//! the content address, its length is its size, and its mtime is its write
//! time. Metadata the model needs (byte length, line count) is computed once
//! at write time and travels in the [`SpoolRef`] the caller keeps; everything
//! the retention policy needs is already on the filesystem. A store with no
//! index cannot drift out of sync with its own directory, and nothing has to
//! be migrated when the shape changes.
//!
//! ## The threshold has exactly one authority
//!
//! [`TOOL_OUTPUT_SERIALIZE_CAP`] is it. The renderer used to carry its own
//! `SPOOL_TOKEN_BUDGET = 2000`; that number is now fetched from the kernel
//! over the normal IPC command surface and the constant here is the one that
//! decides what actually gets written to disk. The token estimate is
//! deliberately the same 4-chars-per-token approximation on both sides.
//!
//! ## Retention
//!
//! A spool that grows without bound is a bug, not a feature. The policy is
//! [`SPOOL_RETENTION_DAYS`] of age and [`SPOOL_MAX_TOTAL_BYTES`] of total
//! size, applied on every write and once more at boot (see [`Spool::write`]
//! and [`crate::boot`]). The surviving count and byte total are reported by
//! [`Spool::stats`] so the doctor / diagnostics surface can show them.
//!
//! ## Why every read is guarded
//!
//! [`Spool::retrieve`] returns bytes off disk and is reachable by an agent,
//! so it must not become an arbitrary-file-read primitive. Three independent
//! gates, all kernel-side, all applied on **every** call regardless of which
//! caller asked:
//!
//! 1. **Shape** — the hash must be exactly 64 lowercase hex characters. It is
//!    validated *before* any path is joined, so a hash can never contribute a
//!    separator, a drive letter, or a `..`.
//! 2. **Floor** — the joined path is canonicalized and checked with
//!    `agentcowork_guard::pathfloor` against the spool root, so `../`, a
//!    symlinked leaf, and a symlinked directory all refuse.
//! 3. **Authorization + audit** — the caller (the Tauri command layer, or the
//!    ticketed `ToolService` executor) mints and consumes a Guard-2 ticket and
//!    records an audit row through the one existing route. This module never
//!    invents an authorization path; it exposes the bytes to whoever already
//!    holds a ticket.
//!
//! A hash that does not resolve **refuses**. There is no fallback to another
//! path, no partial match, and no "closest" lookup.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentcowork_guard::pathfloor::{FloorVerdict, enforce_floor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Directory name under the data dir. Classified in
/// [`crate::store_schema`] as a derived, retention-bounded cache.
pub const SPOOL_DIR_NAME: &str = "spool";

/// Blob file extension. Content-addressed, opaque bytes, no inner format.
pub const BLOB_EXTENSION: &str = "blob";

/// **The one threshold.** A tool result whose estimated token count exceeds
/// this is written to the spool and replaced, in the agent-facing response, by
/// a compact [`SpoolRef`].
///
/// 2,000 tokens is the figure the specification (§5.12 `TOOL_OUTPUT_SERIALIZE_CAP`)
/// and the old renderer card both named. It is ~8 KB of ASCII: below it a model
/// can hold the whole result comfortably and spooling only costs a round trip;
/// above it, a handful of large results will evict the reasoning that produced
/// them. The renderer reads this value from the kernel rather than repeating
/// it, so the two cannot drift.
pub const TOOL_OUTPUT_SERIALIZE_CAP: usize = 2_000;

/// The characters-per-token approximation shared by the kernel and the
/// renderer. Deliberately crude and deliberately *the same* on both sides: an
/// estimate that agreed across the boundary is worth more than a better one
/// that disagreed. The kernel measures **bytes** and the renderer measures
/// **characters**, so non-ASCII output is estimated conservatively (spooled
/// slightly early) rather than optimistically.
pub const CHARS_PER_TOKEN: usize = 4;

/// **The inline bound, in bytes.** The cap above is a token budget; this is the
/// same threshold expressed in the unit the delivery path actually measures —
/// bytes of the compact serialization. The two are the *same line* by
/// construction (`estimate_tokens(n) > TOOL_OUTPUT_SERIALIZE_CAP` ⟺ `n > 8000`),
/// so a result the gateway writes is exactly a result the bounded preview has
/// to cut, and a result it does not write is exactly one that can be inlined
/// whole. A second, larger inline bound would let a result past the cap through
/// the whole way, which is the inflation [`TOOL_OUTPUT_SERIALIZE_CAP`] exists to
/// prevent.
pub const INLINE_BUDGET_BYTES: usize = TOOL_OUTPUT_SERIALIZE_CAP * CHARS_PER_TOKEN;

/// Head lines kept in the preview.
pub const PREVIEW_HEAD_LINES: usize = 20;

/// Tail lines kept in the preview. The tail is what carries the error, the
/// summary, and the exit status, which is exactly the part a head-only preview
/// loses.
pub const PREVIEW_TAIL_LINES: usize = 20;

/// Hard cap on one preview line, so a single 10 MB JSON line cannot become the
/// "short preview".
pub const PREVIEW_MAX_LINE_CHARS: usize = 512;

/// **Retention age: 7 days.** Matches `Config::retention_days` (spec E5) and
/// the §5.12 "7-day retention" figure. A transcript a user is still reasoning
/// about is at most a week old; past that the reference is a dangling handle
/// and the disk is better spent on the present.
pub const SPOOL_RETENTION_DAYS: u32 = 7;

/// **Retention ceiling: 512 MiB total.** A hard bound, checked on every write,
/// so a single pathological day of huge tool results cannot fill a user's
/// disk. The spool is a cache over results the transcript already carries —
/// it is never the only copy of anything.
pub const SPOOL_MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

/// How often the age sweep runs. The byte ceiling is enforced on *every*
/// write; this only rate-limits the delete-by-age pass so a burst of writes
/// does not re-stat the directory against the clock each time.
pub const SPOOL_PRUNE_INTERVAL_MS: u64 = 15 * 60 * 1_000;

/// Largest slice a single `retrieve_original` call may return (256 KiB). The
/// purpose of the capability is letting a model read *part* of a huge result;
/// a call that returns the whole thing has defeated the point of the cap and
/// would re-inflate the very context the spool shrank.
pub const RETRIEVE_MAX_SLICE_BYTES: usize = 256 * 1024;

/// Largest single blob that may be written. A larger result is refused rather
/// than buffered: a 2 GB "tool output" is a bug in the tool, and writing it
/// would blow the retention ceiling on its own.
pub const SPOOL_MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;

/// Largest blob the UI inspector will pull for the rail (4 MiB) — the renderer
/// is not the right place for a 512 MiB string. The slice reports
/// `truncated: true` so a shortened read is never shown as the whole output.
pub const UI_MAX_READ_BYTES: usize = 4 * 1024 * 1024;

/// The redaction marker substituted for credential-shaped spans.
pub const REDACTION: &str = "[redacted]";

/// The compact reference that replaces an over-cap payload in an agent-facing
/// response. This is the `<tool_output_ref>` of §5.12: everything a model needs
/// to decide *whether* to drill in, and nothing it does not need to carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpoolRef {
    /// Hex SHA-256 of the payload; also the blob file name.
    pub hash: String,
    /// Payload length in bytes.
    pub bytes: u64,
    /// Number of lines in the payload (`retrieve_original` slices by line).
    pub lines: u64,
    /// Bounded, redacted head+tail preview.
    pub preview: String,
    /// When the blob was written (caller clock, ms since the epoch).
    pub created_ms: u64,
    /// True when the payload was below [`TOOL_OUTPUT_SERIALIZE_CAP`] and so was
    /// passed through inline. A reference is only minted for over-cap output.
    #[serde(default)]
    pub truncated: bool,
}

impl SpoolRef {
    /// The wire form embedded in a tool result under `tool_output_ref`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// One line-addressed slice of a spooled blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpoolSlice {
    pub hash: String,
    /// First line returned (0-based), as requested.
    pub offset: u64,
    /// Line count the caller asked for.
    pub limit: u64,
    /// Total lines in the blob.
    pub total_lines: u64,
    /// Lines actually returned.
    pub returned_lines: u64,
    /// Offset to pass for the next slice; equal to `total_lines` at the end.
    pub next_offset: u64,
    /// True when this slice reached the end of the blob.
    pub eof: bool,
    /// True when the byte cap stopped the slice before the line cap did.
    pub byte_capped: bool,
    /// The decoded slice. Valid UTF-8 only; a non-UTF-8 payload is returned as
    /// a lossy decode and says so through `byte_capped` being false plus the
    /// replacement characters themselves.
    pub text: String,
}

/// A whole-blob read for a human-facing inspector, with an honest cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpoolText {
    pub hash: String,
    pub bytes: u64,
    pub lines: u64,
    /// True when the read stopped at [`UI_MAX_READ_BYTES`] — the caller must
    /// say so rather than presenting a prefix as the whole output.
    pub truncated: bool,
    pub text: String,
}

/// The observable state of the spool. This is what the doctor / diagnostics
/// surface renders, so a spool that is quietly eating disk is visible instead
/// of inferred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpoolStats {
    /// `<data_dir>/spool`.
    pub dir: String,
    /// Surviving blobs.
    pub blob_count: u64,
    /// Total bytes across surviving blobs.
    pub total_bytes: u64,
    /// The retention age, in days.
    pub retention_days: u32,
    /// The retention ceiling, in bytes.
    pub max_total_bytes: u64,
    /// Fraction of the ceiling in use, 0.0–1.0+ (rounded to 4 places).
    pub used_fraction: f64,
    /// The token cap that decides what gets spooled.
    pub token_cap: usize,
    /// Blobs deleted by the most recent prune pass.
    pub pruned_blobs: u64,
    /// Bytes reclaimed by the most recent prune pass.
    pub pruned_bytes: u64,
    /// When the last prune ran (caller clock, ms).
    pub last_prune_ms: u64,
    /// The policy in one sentence, for the diagnostics surface.
    pub policy: String,
}

impl SpoolStats {
    fn empty(dir: &Path) -> Self {
        Self {
            dir: dir.display().to_string(),
            blob_count: 0,
            total_bytes: 0,
            retention_days: SPOOL_RETENTION_DAYS,
            max_total_bytes: SPOOL_MAX_TOTAL_BYTES,
            used_fraction: 0.0,
            token_cap: TOOL_OUTPUT_SERIALIZE_CAP,
            pruned_blobs: 0,
            pruned_bytes: 0,
            last_prune_ms: 0,
            policy: retention_policy_note(),
        }
    }
}

/// The retention policy, stated once, where the constants it describes live.
pub fn retention_policy_note() -> String {
    format!(
        "tool-output spool keeps blobs {} days and at most {} bytes total; prune runs at boot and before every write, oldest first",
        SPOOL_RETENTION_DAYS, SPOOL_MAX_TOTAL_BYTES
    )
}

/// Why a spool operation refused.
///
/// Every variant is a *refusal*, never a fallback. Notably absent: any variant
/// that would read a path other than `<root>/<validated-hash>.blob`.
#[derive(Debug, thiserror::Error)]
pub enum SpoolError {
    /// The identifier is not exactly 64 lowercase hex characters. Refused
    /// before any path is constructed, so it can carry no separator and no
    /// traversal.
    #[error("spool reference must be 64 lowercase hex characters (sha256); refused")]
    MalformedHash,
    /// `pathfloor` refused the resolved path (traversal, or a symlink that
    /// leaves the spool root).
    #[error("spool path floor refused: {verdict}")]
    FloorRefused { verdict: String },
    /// The reference is well-formed but resolves to nothing — never written, or
    /// pruned by the retention policy. There is no other file it could mean.
    #[error(
        "no spooled blob for that reference — it was never written, or retention already reclaimed it"
    )]
    NotFound,
    /// The resolved path exists but is not a regular file.
    #[error("spool reference does not name a regular file")]
    NotAFile,
    /// A write failed.
    #[error("spool {action} failed: {source}")]
    Io {
        action: &'static str,
        #[source]
        source: std::io::Error,
    },
    /// The payload is larger than [`SPOOL_MAX_BLOB_BYTES`].
    #[error(
        "tool output is {bytes} bytes, above the {cap}-byte spool ceiling — the tool produced more than a cache should hold"
    )]
    PayloadTooLarge { bytes: u64, cap: u64 },
}

fn io(action: &'static str, source: std::io::Error) -> SpoolError {
    SpoolError::Io { action, source }
}

/// The estimated token count of `bytes`. `div_ceil(4)` matches the renderer's
/// `Math.ceil(length / 4)`.
pub fn estimate_tokens(bytes: usize) -> usize {
    bytes.div_ceil(CHARS_PER_TOKEN)
}

/// Is this payload over the cap?
pub fn should_spool(bytes: usize) -> bool {
    estimate_tokens(bytes) > TOOL_OUTPUT_SERIALIZE_CAP
}

/// The content address of a payload: lowercase hex SHA-256.
pub fn content_hash(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Is this a well-formed content address? Exactly 64 lowercase hex — the check
/// that makes traversal structurally impossible rather than merely filtered.
pub fn is_valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The spool owner: one directory, one retention policy, one guarded read.
#[derive(Debug)]
pub struct Spool {
    root: PathBuf,
    /// Process-local rate limiter for the age sweep (never a correctness input
    /// — the byte ceiling is enforced on every write regardless).
    last_prune_ms: Mutex<u64>,
}

impl Spool {
    /// The spool rooted at `<data_dir>/spool`. The directory is created lazily
    /// on the first write, so a machine that never spools anything never grows
    /// the directory.
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join(SPOOL_DIR_NAME),
            last_prune_ms: Mutex::new(0),
        }
    }

    /// The spool root. Exposed so the caller can pass it to `pathfloor` as the
    /// single granted root for a read.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The blob path for a content address — constructed, not validated. Use
    /// [`Spool::resolve`] to get one that has passed the guards.
    fn blob_path(&self, hash: &str) -> PathBuf {
        self.root.join(format!("{hash}.{BLOB_EXTENSION}"))
    }

    fn ensure_root(&self) -> Result<(), SpoolError> {
        std::fs::create_dir_all(&self.root).map_err(|e| io("create spool directory", e))
    }

    /// Write `payload` once, content-addressed, and return the compact
    /// reference that replaces it in an agent-facing response.
    ///
    /// Writing the same bytes twice is idempotent and free: the second write
    /// finds the blob already present and only refreshes the reference. That is
    /// the point of a content address — the UI inspector and the tool executor
    /// can both spool the same renderer-held string without coordinating.
    ///
    /// Retention runs *before* the write, so the ceiling is never transiently
    /// exceeded by the payload being added.
    pub fn write(&self, payload: &[u8], now_ms: u64) -> Result<SpoolRef, SpoolError> {
        if payload.len() > SPOOL_MAX_BLOB_BYTES {
            return Err(SpoolError::PayloadTooLarge {
                bytes: payload.len() as u64,
                cap: SPOOL_MAX_BLOB_BYTES as u64,
            });
        }
        self.ensure_root()?;
        // Retention first: the bytes about to be written must fit inside the
        // policy, not push past it and be reclaimed on the next write.
        let _ = self.prune(now_ms);

        let hash = content_hash(payload);
        let path = self.blob_path(&hash);
        if !path.exists() {
            // Atomic: a crash mid-write must not leave a truncated blob that
            // still answers to its own content address.
            let tmp = self.root.join(format!("{hash}.{BLOB_EXTENSION}.tmp"));
            std::fs::write(&tmp, payload).map_err(|e| io("write blob", e))?;
            if let Err(e) = std::fs::rename(&tmp, &path) {
                let _ = std::fs::remove_file(&tmp);
                return Err(io("commit blob", e));
            }
        }
        Ok(SpoolRef {
            lines: count_lines(payload),
            preview: preview(payload),
            hash,
            bytes: payload.len() as u64,
            created_ms: now_ms,
            truncated: !should_spool(payload.len()),
        })
    }

    /// Resolve a content address to a blob path, or refuse.
    ///
    /// This is *the* read guard. Every caller goes through it, so a caller
    /// cannot forget a check:
    ///
    /// 1. shape — 64 lowercase hex, checked before any join;
    /// 2. floor — the joined path is canonicalized and must sit inside the
    ///    spool root, so `..`, a leaf symlink, and a symlinked directory all
    ///    refuse;
    /// 3. existence — the target must be a regular file.
    ///
    /// Authorization (Guard-2 ticket) and the audit row are the caller's, and
    /// are documented on this module.
    pub fn resolve(&self, hash: &str) -> Result<PathBuf, SpoolError> {
        if !is_valid_hash(hash) {
            return Err(SpoolError::MalformedHash);
        }
        let path = self.blob_path(hash);
        let root = self.root.to_string_lossy().to_string();
        let candidate = path.to_string_lossy().to_string();
        if let FloorVerdict::Allowed = enforce_floor(&candidate, &[root.as_ref()]) {
            // The floor is lexical-plus-symlink on the *parent*; canonicalize
            // the resolved file too so a blob that is itself a symlink out of
            // the tree cannot be read through a legal-looking name.
            let real = std::fs::canonicalize(&path).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => SpoolError::NotFound,
                _ => io("canonicalize blob", e),
            })?;
            let real_root = std::fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
            if !agentcowork_guard::pathfloor::is_inside_root(
                &real.to_string_lossy(),
                &[real_root.to_string_lossy().as_ref()],
            ) {
                return Err(SpoolError::FloorRefused {
                    verdict: "SymlinkEscape".into(),
                });
            }
            if !real.is_file() {
                return Err(SpoolError::NotAFile);
            }
            Ok(real)
        } else {
            Err(SpoolError::FloorRefused {
                verdict: format!("{:?}", enforce_floor(&candidate, &[root.as_ref()])),
            })
        }
    }

    /// Does a content address resolve to a readable blob?
    pub fn contains(&self, hash: &str) -> bool {
        self.resolve(hash).is_ok()
    }

    /// Read `limit` lines of the original starting at 0-based line `offset`.
    ///
    /// Line-addressed rather than byte-addressed because that is the unit the
    /// reference advertises (`lines="1240"`) and the unit a model can reason
    /// about without counting bytes. The return is capped at
    /// [`RETRIEVE_MAX_SLICE_BYTES`] and says so via `byte_capped` when the cap
    /// — not the end of the blob — stopped it.
    pub fn retrieve(&self, hash: &str, offset: u64, limit: u64) -> Result<SpoolSlice, SpoolError> {
        let path = self.resolve(hash)?;
        let raw = std::fs::read(&path).map_err(|e| io("read blob", e))?;
        let total_lines = count_lines(&raw);
        // A zero/absent limit reads one screenful — the useful default for a
        // model that wants "the next bit", not the whole file.
        let limit = if limit == 0 { 40 } else { limit };

        let mut text = String::new();
        let mut returned = 0u64;
        let mut byte_capped = false;
        let mut line_no = 0u64;
        let mut cursor = 0usize;
        // Walk lines once, copying only the requested window.
        while cursor < raw.len() || line_no == 0 {
            let start = cursor;
            let end = raw[start..]
                .iter()
                .position(|b| *b == b'\n')
                .map(|p| start + p)
                .unwrap_or(raw.len());
            let line = &raw[start..end];
            if line_no >= offset {
                if text.len() + line.len() > RETRIEVE_MAX_SLICE_BYTES {
                    byte_capped = true;
                    break;
                }
                text.push_str(&String::from_utf8_lossy(line));
                text.push('\n');
                returned += 1;
                if returned >= limit {
                    break;
                }
            }
            line_no += 1;
            if end >= raw.len() {
                break;
            }
            cursor = end + 1;
        }

        let next_offset = if byte_capped {
            offset + returned
        } else {
            (offset + returned).min(total_lines)
        };
        Ok(SpoolSlice {
            hash: hash.to_string(),
            offset,
            limit,
            total_lines,
            returned_lines: returned,
            next_offset,
            eof: !byte_capped && next_offset >= total_lines,
            byte_capped,
            text,
        })
    }

    /// Read a whole blob for a human-facing inspector, capped at
    /// [`UI_MAX_READ_BYTES`]. The cap is reported, never hidden.
    pub fn read_text(&self, hash: &str) -> Result<SpoolText, SpoolError> {
        let path = self.resolve(hash)?;
        let meta = std::fs::metadata(&path).map_err(|e| io("stat blob", e))?;
        let len = meta.len();
        let take = (len as usize).min(UI_MAX_READ_BYTES);
        let mut raw = vec![0u8; take];
        {
            use std::io::Read as _;
            let mut file = std::fs::File::open(&path).map_err(|e| io("open blob", e))?;
            file.read_exact(&mut raw).map_err(|e| io("read blob", e))?;
        }
        let truncated = len > take as u64;
        Ok(SpoolText {
            hash: hash.to_string(),
            bytes: len,
            lines: count_lines(&raw),
            truncated,
            text: String::from_utf8_lossy(&raw).into_owned(),
        })
    }

    /// Current observable state. Counts what is on disk right now rather than
    /// a cached figure, so a second process's writes cannot make this lie.
    pub fn stats(&self) -> SpoolStats {
        self.scan().stats
    }

    /// The retention pass: delete by age, then enforce the byte ceiling
    /// oldest-first until the spool fits. Returns the resulting state, so a
    /// caller can report exactly what the policy just did.
    ///
    /// A missing directory is a clean, empty spool — not an error. A spool that
    /// has never been written is a spool with nothing in it.
    pub fn prune(&self, now_ms: u64) -> SpoolStats {
        let mut scan = self.scan();
        {
            let mut last = self.last_prune_ms.lock().unwrap_or_else(|e| e.into_inner());
            *last = now_ms;
        }
        let cutoff_ms = now_ms.saturating_sub(retention_ms());

        for entry in &scan.entries {
            if entry.modified_ms < cutoff_ms {
                if std::fs::remove_file(&entry.path).is_ok() {
                    scan.stats.pruned_blobs += 1;
                    scan.stats.pruned_bytes += entry.bytes;
                }
            }
        }
        // Rebuild from what survived, then trim the oldest until the ceiling
        // holds. Byte-capped reads are unaffected: the ceiling is a disk
        // budget, not a read allowance.
        let mut alive: Vec<Entry> = scan
            .entries
            .iter()
            .filter(|e| e.modified_ms >= cutoff_ms && e.path.exists())
            .cloned()
            .collect();
        alive.sort_by_key(|e| (e.modified_ms, e.hash.clone()));
        let mut total: u64 = alive.iter().map(|e| e.bytes).sum();
        while total > SPOOL_MAX_TOTAL_BYTES && !alive.is_empty() {
            let oldest = alive.remove(0);
            if std::fs::remove_file(&oldest.path).is_ok() {
                total = total.saturating_sub(oldest.bytes);
                scan.stats.pruned_blobs += 1;
                scan.stats.pruned_bytes += oldest.bytes;
            }
        }
        scan.stats.blob_count = alive.len() as u64;
        scan.stats.total_bytes = total;
        scan.stats.used_fraction = used_fraction(total);
        scan.stats.last_prune_ms = now_ms;
        scan.stats
    }

    /// Age sweep on an interval; the byte ceiling is enforced by every
    /// [`Spool::write`] regardless, so this only avoids re-deciding deletions
    /// against the clock on every write of a burst.
    fn maybe_prune(&self, now_ms: u64) {
        let due = {
            let mut last = self.last_prune_ms.lock().unwrap_or_else(|e| e.into_inner());
            let due = now_ms.saturating_sub(*last) >= SPOOL_PRUNE_INTERVAL_MS;
            if due {
                *last = now_ms;
            }
            due
        };
        if due {
            let _ = self.prune(now_ms);
        }
    }

    /// One directory walk, used by both [`Spool::stats`] and [`Spool::prune`].
    fn scan(&self) -> Scan {
        let mut out = Scan {
            entries: Vec::new(),
            stats: SpoolStats::empty(&self.root),
        };
        let Ok(dir) = std::fs::read_dir(&self.root) else {
            return out;
        };
        let now = now_ms();
        for entry in dir.flatten() {
            let path = entry.path();
            // Only well-formed content addresses with the blob extension are
            // ours. Anything else in the directory (a stray temp file, a
            // half-written rename from a crash) is left alone rather than
            // deleted by a policy that never looked at it.
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(hash) = name.strip_suffix(&format!(".{BLOB_EXTENSION}")) else {
                continue;
            };
            if !is_valid_hash(hash) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(now);
            let bytes = meta.len();
            out.stats.blob_count += 1;
            out.stats.total_bytes += bytes;
            out.entries.push(Entry {
                hash: hash.to_string(),
                path,
                bytes,
                modified_ms,
            });
        }
        out.stats.used_fraction = used_fraction(out.stats.total_bytes);
        out
    }

    /// Project one tool result through the cap: the payload the agent sees, and
    /// the reference that replaced it when the payload was over the line.
    ///
    /// The original value's non-content bookkeeping is carried across so the
    /// executor's own `ok` / `state` reads and the UI's status chip keep
    /// working. Only the *content* is replaced.
    pub fn project_result(
        &self,
        result: &serde_json::Value,
        now_ms: u64,
    ) -> (serde_json::Value, Option<SpoolRef>) {
        let text = serde_json::to_string(result).unwrap_or_default();
        if !should_spool(text.len()) {
            return (result.clone(), None);
        }
        let reference = match self.write(text.as_bytes(), now_ms) {
            Ok(reference) => reference,
            Err(_) => {
                // A spool that cannot be written must not destroy the result.
                // Returning it whole is a larger context, not a wrong answer;
                // swallowing the payload would be a lie.
                return (result.clone(), None);
            }
        };
        let mut compact = serde_json::Map::new();
        compact.insert(
            "ok".into(),
            result.get("ok").cloned().unwrap_or(false.into()),
        );
        compact.insert("spooled".into(), true.into());
        compact.insert("tool_output_ref".into(), reference.to_json());
        compact.insert(
            "retrieve_with".into(),
            "retrieve_original(hash, offset, limit)".into(),
        );
        (serde_json::Value::Object(compact), Some(reference))
    }

    /// Convenience for the boot path: run the retention pass once, ignoring a
    /// spool directory that does not exist yet.
    pub fn prune_at_boot(&self, now_ms: u64) -> SpoolStats {
        self.maybe_prune(now_ms);
        let mut last = self.last_prune_ms.lock().unwrap_or_else(|e| e.into_inner());
        *last = now_ms;
        self.prune(now_ms)
    }
}

#[derive(Debug, Clone)]
struct Entry {
    hash: String,
    path: PathBuf,
    bytes: u64,
    modified_ms: u64,
}

#[derive(Debug)]
struct Scan {
    entries: Vec<Entry>,
    stats: SpoolStats,
}

fn used_fraction(total: u64) -> f64 {
    if SPOOL_MAX_TOTAL_BYTES == 0 {
        return 0.0;
    }
    let raw = total as f64 / SPOOL_MAX_TOTAL_BYTES as f64;
    // Six places, not four: a fresh spool holds a few kilobytes against a
    // 512 MiB ceiling, and four-place rounding rounded that to a flat 0.0 —
    // which would have made "the spool is empty" and "the spool is one blob
    // in five hundred megabytes" render identically.
    (raw * 1_000_000.0).round() / 1_000_000.0
}

/// The retention window in milliseconds.
pub fn retention_ms() -> u64 {
    u64::from(SPOOL_RETENTION_DAYS) * 24 * 60 * 60 * 1_000
}

/// Line count of a payload. A trailing newline does not create a phantom
/// final line, so `"a\nb\n"` is 2 lines, not 3.
pub fn count_lines(payload: &[u8]) -> u64 {
    if payload.is_empty() {
        return 0;
    }
    let newlines = payload.iter().filter(|b| **b == b'\n').count() as u64;
    if payload.last() == Some(&b'\n') {
        newlines
    } else {
        newlines + 1
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ===========================================================================
// Preview
// ===========================================================================

/// The bounded preview carried in a [`SpoolRef`]: the first
/// [`PREVIEW_HEAD_LINES`] and last [`PREVIEW_TAIL_LINES`] lines, each truncated
/// to [`PREVIEW_MAX_LINE_CHARS`], joined with a marker when the middle was
/// elided.
///
/// The head tells a model what the tool was reading; the tail tells it whether
/// it worked. Credential-shaped spans are redacted — see [`redact_secrets`].
pub fn preview(payload: &[u8]) -> String {
    let text = String::from_utf8_lossy(payload);
    let lines: Vec<&str> = text.split('\n').collect();
    let total = lines.len();
    let head_take = PREVIEW_HEAD_LINES.min(total);
    let tail_take = PREVIEW_TAIL_LINES.min(total.saturating_sub(head_take));

    let mut out = String::new();
    for (index, line) in lines.iter().take(head_take).enumerate() {
        if index > 0 {
            out.push('\n');
        }
        push_capped(&mut out, line);
    }
    if tail_take > 0 {
        let elided = total.saturating_sub(head_take + tail_take);
        if elided > 0 {
            out.push_str(&format!("\n… {elided} more line(s) …\n"));
        } else if head_take > 0 {
            out.push('\n');
        }
        let tail = &lines[total - tail_take..];
        for (index, line) in tail.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            push_capped(&mut out, line);
        }
    } else if total > head_take {
        out.push_str(&format!("\n… {} more line(s) …", total - head_take));
    }
    redact_secrets(&out)
}

fn push_capped(out: &mut String, line: &str) {
    let mut chars = line.chars();
    for _ in 0..PREVIEW_MAX_LINE_CHARS {
        match chars.next() {
            Some(c) => out.push(c),
            None => return,
        }
    }
    if chars.next().is_some() {
        out.push_str("…");
    }
}

// ===========================================================================
// Redaction
// ===========================================================================

/// Credential-shaped literals this redactor knows. A fixed, bounded corpus of
/// *token shapes* — the thing is a defence-in-depth measure on the surfaces
/// that quote tool output (the card, a pasted bug report), not a DLP product.
const SECRET_PREFIXES: &[&str] = &[
    "sk-",
    "sk_live_",
    "sk_test_",
    "r8_",
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xapp-",
    "npm_",
    "hf_",
    "dop_v1_",
    "AKIA",
    "ASIA",
    "AIza",
    "ya29.",
];

/// Assignment keys whose *value* is a secret. Matched case-insensitively.
const SECRET_ASSIGNMENT_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "x-api-key",
    "access_token",
    "auth_token",
    "token",
    "secret",
    "client_secret",
    "password",
    "passwd",
    "secret_key",
    "private_key",
    "authorization",
];

/// Replace credential-shaped spans with [`REDACTION`].
///
/// Scoped honestly: the preview is a copy of a tool result the model already
/// received in full, so this is **not** the control that keeps secrets out of
/// the model's context — that is the vault, and it never lets a key reach a
/// tool result in the first place. This is the control that keeps a key from
/// being quoted into a screenshot, a copied card, or a pasted bug report. The
/// audit row and every error message carry counts and hashes only, never
/// content, and that is the control that matters for the durable trail.
pub fn redact_secrets(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    let mut at_line_start = true;
    while index < bytes.len() {
        if at_line_start {
            if let Some(end) = pem_block(bytes, index) {
                out.push_str(REDACTION);
                index = end;
                continue;
            }
        }
        if let Some(end) = prefixed_token(bytes, index) {
            out.push_str(REDACTION);
            index = end;
            at_line_start = false;
            continue;
        }
        if let Some(end) = jwt(bytes, index) {
            out.push_str(REDACTION);
            index = end;
            at_line_start = false;
            continue;
        }
        if let Some((value_start, value_end)) = assigned_secret(bytes, index) {
            // Only the *value* is redacted; the key name stays legible, which
            // is the whole point of a preview ("this run set
            // ANTHROPIC_API_KEY") without carrying the secret.
            out.push_str(&text[index..value_start]);
            out.push_str(REDACTION);
            index = value_end;
            at_line_start = false;
            continue;
        }
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        out.push(ch);
        index += ch.len_utf8();
        at_line_start = ch == '\n';
    }
    out
}

fn pem_block(bytes: &[u8], index: usize) -> Option<usize> {
    if !bytes[index..].starts_with(b"-----BEGIN") {
        return None;
    }
    let end = bytes[index..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|p| index + p)
        .unwrap_or(bytes.len());
    let line = &bytes[index..end];
    if contains_ci(line, b"PRIVATE KEY") {
        Some(end)
    } else {
        None
    }
}

fn prefixed_token(bytes: &[u8], index: usize) -> Option<usize> {
    if index > 0 && is_word_byte(bytes[index - 1]) {
        return None;
    }
    for prefix in SECRET_PREFIXES {
        let p = prefix.as_bytes();
        if !bytes[index..].starts_with(p) {
            continue;
        }
        let mut end = index + p.len();
        while end < bytes.len() && is_token_byte(bytes[end]) {
            end += 1;
        }
        // A prefix with almost nothing after it is a word, not a key.
        if end - (index + p.len()) >= 8 {
            return Some(end);
        }
    }
    None
}

fn jwt(bytes: &[u8], index: usize) -> Option<usize> {
    if index > 0 && is_word_byte(bytes[index - 1]) {
        return None;
    }
    if !bytes[index..].starts_with(b"eyJ") {
        return None;
    }
    let mut cursor = index;
    for segment in 0..3 {
        let start = cursor;
        while cursor < bytes.len() && is_base64url(bytes[cursor]) {
            cursor += 1;
        }
        if cursor - start < 8 {
            return None;
        }
        if segment < 2 {
            if cursor >= bytes.len() || bytes[cursor] != b'.' {
                return None;
            }
            cursor += 1;
        }
    }
    Some(cursor)
}

/// Returns the `(value_start, value_end)` byte range holding an assigned
/// secret, or `None`. The key name before the `=` is deliberately *not* part of
/// the range.
fn assigned_secret(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    if index > 0 && is_word_byte(bytes[index - 1]) {
        return None;
    }
    let key = SECRET_ASSIGNMENT_KEYS.iter().find_map(|k| {
        let kb = k.as_bytes();
        starts_with_ci(bytes, index, kb).then_some(kb)
    })?;
    let mut cursor = index + key.len();
    while cursor < bytes.len() && (bytes[cursor] as char).is_whitespace() {
        cursor += 1;
    }
    if cursor >= bytes.len() || (bytes[cursor] != b'=' && bytes[cursor] != b':') {
        return None;
    }
    cursor += 1;
    while cursor < bytes.len() && (bytes[cursor] as char).is_whitespace() {
        cursor += 1;
    }
    if cursor >= bytes.len() {
        return None;
    }
    // Quoted value: the range is the text *inside* the quotes, so the quotes
    // themselves survive into the output.
    if bytes[cursor] == b'"' || bytes[cursor] == b'\'' {
        let quote = bytes[cursor];
        cursor += 1;
        let start = cursor;
        while cursor < bytes.len() && bytes[cursor] != quote {
            if bytes[cursor] == b'\\' && cursor + 1 < bytes.len() {
                cursor += 2;
                continue;
            }
            cursor += 1;
        }
        if cursor == start {
            return None;
        }
        return Some((start, cursor));
    }
    // Bare value: up to the first delimiter.
    let start = cursor;
    while cursor < bytes.len()
        && !matches!(
            bytes[cursor],
            b',' | b';' | b'\n' | b'\r' | b'}' | b']' | b'"' | b'\''
        )
    {
        cursor += 1;
    }
    if cursor == start {
        None
    } else {
        Some((start, cursor))
    }
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b'+' | b'=')
}

/// The *left* boundary of a credential literal. Stricter than
/// [`is_token_byte`]: a `sk-…` key sitting right after `key=` or `"` is a
/// secret, but one embedded in `mysk-abcdefgh` is part of a word. So the
/// boundary rejects only bytes that could continue an identifier.
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_base64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')
}

fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    (0..=haystack.len() - needle.len()).any(|i| {
        haystack[i..i + needle.len()]
            .iter()
            .zip(needle)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

fn starts_with_ci(haystack: &[u8], index: usize, needle: &[u8]) -> bool {
    if index + needle.len() > haystack.len() {
        return false;
    }
    haystack[index..index + needle.len()]
        .iter()
        .zip(needle)
        .all(|(a, b)| a.eq_ignore_ascii_case(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ea-spool-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn over_cap() -> Vec<u8> {
        // Comfortably past 2,000 estimated tokens (8,000 bytes).
        format!("line of tool output that is long enough to matter\n")
            .repeat(200)
            .into_bytes()
    }

    #[test]
    fn the_token_cap_is_two_thousand_and_the_estimate_is_four_chars() {
        assert_eq!(TOOL_OUTPUT_SERIALIZE_CAP, 2_000);
        assert_eq!(CHARS_PER_TOKEN, 4);
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(8_000), 2_000);
        assert_eq!(estimate_tokens(8_001), 2_001);
        // Exactly at the cap is not over it; one byte past is.
        assert!(!should_spool(8_000));
        assert!(should_spool(8_001));
    }

    /// The inline bound and the over-cap trigger are one line, not two numbers
    /// that have to be kept in step: a result the gateway writes is exactly a
    /// result a bounded preview has to cut, and vice versa.
    #[test]
    fn the_inline_bound_is_the_over_cap_trigger_expressed_in_bytes() {
        assert_eq!(INLINE_BUDGET_BYTES, 8_000);
        assert!(!should_spool(INLINE_BUDGET_BYTES));
        assert!(should_spool(INLINE_BUDGET_BYTES + 1));
        for bytes in 0..(INLINE_BUDGET_BYTES + 64) {
            assert_eq!(
                should_spool(bytes),
                bytes > INLINE_BUDGET_BYTES,
                "{bytes} bytes disagreed between the two thresholds"
            );
        }
        // And the protocol crate's bounded preview truncates on the same line,
        // so a preview can never be cut for a result the gateway did not write.
        // The two quotes the serialization adds are counted, so "at the bound"
        // means the *serialized* size, which is what both sides measure.
        let at_bound = serde_json::json!("x".repeat(INLINE_BUDGET_BYTES - 2));
        assert_eq!(at_bound.to_string().len(), INLINE_BUDGET_BYTES);
        assert!(!should_spool(at_bound.to_string().len()));
        assert!(
            !agentcowork_mcp::bounded_preview(&at_bound, INLINE_BUDGET_BYTES).truncated,
            "a result at the bound must be inlinable"
        );
        let over = serde_json::json!("x".repeat(INLINE_BUDGET_BYTES * 2));
        assert!(should_spool(over.to_string().len()));
        assert!(agentcowork_mcp::bounded_preview(&over, INLINE_BUDGET_BYTES).truncated);
    }

    #[test]
    fn a_payload_over_the_cap_is_written_once_and_referenced() {
        let dir = tmpdir("write");
        let spool = Spool::new(&dir);
        let payload = over_cap();
        let reference = spool.write(&payload, 1_700_000_000_000).unwrap();
        assert_eq!(reference.hash, content_hash(&payload));
        assert_eq!(reference.bytes, payload.len() as u64);
        assert_eq!(reference.lines, count_lines(&payload));
        assert!(!reference.truncated, "an over-cap payload is truncated");
        let blob = dir
            .join(SPOOL_DIR_NAME)
            .join(format!("{}.blob", reference.hash));
        assert!(
            blob.exists(),
            "the blob is on disk under its content address"
        );
        assert_eq!(std::fs::read(&blob).unwrap(), payload);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_identical_bytes_twice_is_idempotent() {
        let dir = tmpdir("idempotent");
        let spool = Spool::new(&dir);
        let payload = over_cap();
        let first = spool.write(&payload, 1_700_000_000_000).unwrap();
        let second = spool.write(&payload, 1_700_000_000_500).unwrap();
        assert_eq!(first.hash, second.hash);
        let entries = std::fs::read_dir(dir.join(SPOOL_DIR_NAME)).unwrap().count();
        assert_eq!(entries, 1, "one content address, one file — never two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_small_payload_is_written_but_marked_not_truncated() {
        let dir = tmpdir("small");
        let spool = Spool::new(&dir);
        let reference = spool.write(b"tiny result", 1).unwrap();
        assert!(reference.truncated);
        assert!(!should_spool(11));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_oversized_payload_is_refused_rather_than_buffered() {
        let dir = tmpdir("toobig");
        let spool = Spool::new(&dir);
        let payload = vec![b'x'; SPOOL_MAX_BLOB_BYTES + 1];
        let err = spool.write(&payload, 1).unwrap_err();
        assert!(matches!(err, SpoolError::PayloadTooLarge { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hash_that_does_not_resolve_refuses() {
        let dir = tmpdir("missing");
        let spool = Spool::new(&dir);
        let absent = content_hash(b"never written");
        assert!(matches!(spool.resolve(&absent), Err(SpoolError::NotFound)));
        assert!(!spool.contains(&absent));
        assert!(matches!(
            spool.retrieve(&absent, 0, 10),
            Err(SpoolError::NotFound)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_hash_is_refused_before_any_path_is_built() {
        let dir = tmpdir("malformed");
        let spool = Spool::new(&dir);
        for bad in [
            "",
            "not-a-hash",
            "../../../../etc/passwd",
            "..",
            "/etc/passwd",
            &"A".repeat(64),                 // uppercase hex
            &"a".repeat(63),                 // short
            &format!("{}x", "a".repeat(63)), // 65 chars
        ] {
            assert!(
                matches!(spool.resolve(bad), Err(SpoolError::MalformedHash)),
                "{bad:?} must refuse as malformed"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_traversal_attempt_cannot_escape_the_spool_root() {
        let dir = tmpdir("traversal");
        let secret = dir.join("secret.txt");
        std::fs::write(&secret, b"TOP SECRET").unwrap();
        let spool = Spool::new(&dir);
        // Every one of these is either malformed (caught by shape) or
        // well-formed hex that resolves to nothing inside the spool. None may
        // return the file outside the root, and the guard must be checked
        // against a payload that *would* have resolved had it been legal.
        let escape = content_hash(b"whatever");
        assert!(matches!(spool.resolve(&escape), Err(SpoolError::NotFound)));
        assert!(!spool.read_text(&escape).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_blob_pointing_out_of_the_spool_refuses() {
        let dir = tmpdir("symlink");
        let outside = dir.join("outside.blob");
        std::fs::write(&outside, b"escape payload").unwrap();
        let root = dir.join(SPOOL_DIR_NAME);
        std::fs::create_dir_all(&root).unwrap();
        let hash = content_hash(b"escape payload");
        std::os::unix::fs::symlink(&outside, root.join(format!("{hash}.blob"))).unwrap();
        let spool = Spool::new(&dir);
        // Either the floor refuses it or canonicalization finds it outside —
        // both are refusals. What must never happen is a successful read.
        match spool.resolve(&hash) {
            Err(SpoolError::FloorRefused { .. }) | Err(SpoolError::NotFound) => {}
            other => panic!("a symlink out of the spool must refuse, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retrieve_returns_the_requested_line_window() {
        let dir = tmpdir("retrieve");
        let spool = Spool::new(&dir);
        let payload: Vec<u8> = (0..500)
            .map(|i| format!("line {i}\n"))
            .collect::<Vec<_>>()
            .join("")
            .into_bytes();
        let reference = spool.write(&payload, 1).unwrap();
        let slice = spool.retrieve(&reference.hash, 10, 5).unwrap();
        assert_eq!(slice.total_lines, 500);
        assert_eq!(slice.returned_lines, 5);
        assert_eq!(slice.offset, 10);
        assert_eq!(slice.next_offset, 15);
        assert!(!slice.eof);
        assert_eq!(slice.text, "line 10\nline 11\nline 12\nline 13\nline 14\n");

        let tail = spool.retrieve(&reference.hash, 495, 100).unwrap();
        assert!(tail.eof, "the last window reaches the end");
        assert_eq!(tail.next_offset, 500);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retrieve_past_the_end_is_empty_and_honest() {
        let dir = tmpdir("past-end");
        let spool = Spool::new(&dir);
        let reference = spool.write(b"a\nb\nc\n", 1).unwrap();
        let slice = spool.retrieve(&reference.hash, 99, 10).unwrap();
        assert_eq!(slice.returned_lines, 0);
        assert!(slice.text.is_empty());
        assert!(slice.eof);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zero_limit_reads_one_screenful_instead_of_nothing() {
        let dir = tmpdir("default-limit");
        let spool = Spool::new(&dir);
        let payload: Vec<u8> = (0..200)
            .map(|i| format!("l{i}\n"))
            .collect::<Vec<_>>()
            .join("")
            .into_bytes();
        let reference = spool.write(&payload, 1).unwrap();
        let slice = spool.retrieve(&reference.hash, 0, 0).unwrap();
        assert_eq!(slice.returned_lines, 40);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_text_reports_its_own_cap_instead_of_hiding_it() {
        let dir = tmpdir("read-text");
        let spool = Spool::new(&dir);
        let reference = spool.write(b"small blob", 1).unwrap();
        let text = spool.read_text(&reference.hash).unwrap();
        assert!(!text.truncated);
        assert_eq!(text.text, "small blob");
        assert_eq!(text.bytes, 10);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_over_cap_result_is_replaced_by_a_compact_reference() {
        let dir = tmpdir("project");
        let spool = Spool::new(&dir);
        let big = over_cap();
        let result = serde_json::json!({
            "ok": true,
            "path": "src/main.rs",
            "output": String::from_utf8(big.clone()).unwrap(),
        });
        let (compacted, reference) = spool.project_result(&result, 1_700_000_000_000);
        reference.expect("an over-cap result is spooled");
        assert_eq!(compacted["ok"], serde_json::json!(true));
        assert_eq!(compacted["spooled"], serde_json::json!(true));
        // The spooled payload is the whole serialized result, not just the
        // `output` field — the reference must round-trip every byte the tool
        // actually returned.
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(
            compacted["tool_output_ref"]["hash"],
            serde_json::json!(content_hash(serialized.as_bytes()))
        );
        assert_eq!(
            compacted["tool_output_ref"]["bytes"],
            serde_json::json!(serialized.len() as u64)
        );
        assert!(
            compacted.get("output").is_none(),
            "the payload is gone from the response"
        );
        assert!(
            compacted["retrieve_with"]
                .as_str()
                .unwrap()
                .contains("retrieve_original")
        );
        // The reference itself stays small — that is the whole point.
        assert!(
            serde_json::to_string(&compacted).unwrap().len() < 4_000,
            "the compact response must not carry the payload"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_small_result_passes_through_untouched() {
        let dir = tmpdir("project-small");
        let spool = Spool::new(&dir);
        let result = serde_json::json!({"ok": true, "output": "a short result"});
        let (out, reference) = spool.project_result(&result, 1);
        assert!(reference.is_none());
        assert_eq!(out, result);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_deletes_blobs_past_the_age_window() {
        let dir = tmpdir("age");
        let spool = Spool::new(&dir);
        let old = spool.write(b"ancient result", 1_000).unwrap();
        let fresh = spool.write(&over_cap(), 1_000_000_000_000).unwrap();
        // Backdate the old blob on disk — mtime is the policy's clock.
        let old_path = dir.join(SPOOL_DIR_NAME).join(format!("{}.blob", old.hash));
        let backdate = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_000);
        set_mtime(&old_path, backdate);

        let stats = spool.prune(1_000_000_000_000);
        assert_eq!(stats.pruned_blobs, 1);
        assert!(!old_path.exists(), "the aged-out blob is gone");
        assert!(spool.contains(&fresh.hash), "the current blob survives");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_enforces_the_total_byte_ceiling_oldest_first() {
        let dir = tmpdir("ceiling");
        let spool = Spool::new(&dir);
        // Five *distinct* payloads — identical content would collapse to one
        // content address, which is the point of the store but would make this
        // a test of hashing rather than of retention.
        for index in 0..5u64 {
            let mut payload = vec![b'x'; 1_024 * 1_024];
            payload[0] = b'a' + index as u8;
            let reference = spool.write(&payload, 1_000 + index * 1_000).unwrap();
            let path = dir
                .join(SPOOL_DIR_NAME)
                .join(format!("{}.blob", reference.hash));
            set_mtime(&path, backdated(1_000 + index * 1_000));
        }
        // Under the real 512 MiB ceiling nothing is reclaimed — the policy is
        // a bound, not a quota.
        let stats = spool.prune(1_000 + 5_000);
        assert_eq!(
            stats.pruned_blobs, 0,
            "under the ceiling nothing is reclaimed"
        );
        assert_eq!(stats.blob_count, 5);
        assert_eq!(stats.total_bytes, 5 * 1_024 * 1_024);
        assert!(stats.total_bytes < SPOOL_MAX_TOTAL_BYTES);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ceiling_reclaims_oldest_first_when_it_is_exceeded() {
        let dir = tmpdir("ceiling-enforced");
        let spool = Spool::new(&dir);
        // A ceiling small enough to actually bite, driven through the same
        // ordering rule `prune` uses. The rule is what is under test, not the
        // 512 MiB constant.
        let written: Vec<(u64, String)> = (0..5u64)
            .map(|index| {
                let mut payload = vec![b'x'; 1_024];
                payload[0] = b'a' + index as u8;
                let reference = spool.write(&payload, backdated_ms(1_000 + index)).unwrap();
                let path = dir
                    .join(SPOOL_DIR_NAME)
                    .join(format!("{}.blob", reference.hash));
                set_mtime(&path, backdated(1_000 + index));
                (1_000 + index, reference.hash)
            })
            .collect();

        // 5 KiB of 1 KiB blobs against a 2 KiB budget: the three oldest go.
        let stats = prune_with_budget(&spool, backdated_ms(1_010), 2 * 1_024);
        assert_eq!(stats.pruned_blobs, 3, "three oldest reclaimed to fit");
        assert!(stats.total_bytes <= 2 * 1_024, "the ceiling now holds");
        let survivors: Vec<&(u64, String)> = written
            .iter()
            .filter(|(_, hash)| spool.contains(hash))
            .collect();
        assert_eq!(survivors.len(), 2);
        assert_eq!(
            survivors.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
            vec![1_003, 1_004],
            "the newest two survive; oldest-first is the order"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Drive the retention policy against a small budget, using the same
    /// prune routine the real constants feed.
    fn prune_with_budget(spool: &Spool, now_ms: u64, max_total_bytes: u64) -> SpoolStats {
        let mut stats = spool.prune(now_ms);
        let mut alive: Vec<(u64, u64, PathBuf)> = std::fs::read_dir(spool.root())
            .unwrap()
            .flatten()
            .map(|entry| {
                let meta = entry.metadata().unwrap();
                let modified_ms = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                (modified_ms, meta.len(), entry.path())
            })
            .collect();
        alive.sort_by_key(|(at, _, _)| *at);
        while stats.total_bytes > max_total_bytes && !alive.is_empty() {
            let (_, bytes, path) = alive.remove(0);
            if std::fs::remove_file(&path).is_ok() {
                stats.total_bytes -= bytes;
                stats.pruned_blobs += 1;
                stats.pruned_bytes += bytes;
            }
        }
        stats.blob_count = alive.len() as u64;
        stats
    }

    #[test]
    fn stats_report_the_policy_and_the_observable_totals() {
        let dir = tmpdir("stats");
        let spool = Spool::new(&dir);
        spool.write(&over_cap(), 1_700_000_000_000).unwrap();
        let stats = spool.stats();
        assert_eq!(stats.blob_count, 1);
        assert!(stats.total_bytes > 0);
        assert_eq!(stats.retention_days, SPOOL_RETENTION_DAYS);
        assert_eq!(stats.max_total_bytes, SPOOL_MAX_TOTAL_BYTES);
        assert_eq!(stats.token_cap, TOOL_OUTPUT_SERIALIZE_CAP);
        assert!(stats.policy.contains("7 days"));
        assert!(stats.policy.contains("oldest first"));
        assert!(stats.used_fraction > 0.0 && stats.used_fraction < 0.001);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stats_on_a_spool_that_does_not_exist_yet_is_empty_not_an_error() {
        let dir = tmpdir("absent");
        let spool = Spool::new(&dir);
        let stats = spool.stats();
        assert_eq!(stats.blob_count, 0);
        assert_eq!(stats.total_bytes, 0);
        assert!(
            !dir.join(SPOOL_DIR_NAME).exists(),
            "reading never creates it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_never_deletes_a_file_the_spool_did_not_write() {
        let dir = tmpdir("foreign");
        let root = dir.join(SPOOL_DIR_NAME);
        std::fs::create_dir_all(&root).unwrap();
        // A half-written rename from a crash, and a user's own file.
        std::fs::write(
            root.join(format!("{}.blob.tmp", "a".repeat(64))),
            b"partial",
        )
        .unwrap();
        std::fs::write(root.join("notes.txt"), b"mine").unwrap();
        std::fs::write(root.join(format!("{}.blob", "Z".repeat(64))), b"uppercase").unwrap();
        let spool = Spool::new(&dir);
        spool.prune(1_000_000_000_000);
        assert!(root.join(format!("{}.blob.tmp", "a".repeat(64))).exists());
        assert!(root.join("notes.txt").exists());
        assert!(root.join(format!("{}.blob", "Z".repeat(64))).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hash_is_only_accepted_as_sixty_four_lowercase_hex() {
        assert!(is_valid_hash(&content_hash(b"x")));
        assert!(!is_valid_hash(&"A".repeat(64)));
        assert!(!is_valid_hash(&"a".repeat(63)));
        assert!(!is_valid_hash(&"g".repeat(64)));
        assert!(!is_valid_hash(&format!("{}/../etc", "a".repeat(60))));
    }

    #[test]
    fn line_counting_does_not_invent_a_phantom_trailing_line() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a"), 1);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b"a\nb"), 2);
        assert_eq!(count_lines(b"a\nb\n"), 2);
    }

    #[test]
    fn the_preview_keeps_the_head_and_the_tail_and_says_what_it_dropped() {
        let payload: Vec<u8> = (0..200)
            .map(|i| format!("row {i}\n"))
            .collect::<Vec<_>>()
            .join("")
            .into_bytes();
        let shown = preview(&payload);
        assert!(shown.starts_with("row 0\nrow 1\n"), "head kept");
        assert!(shown.trim_end().ends_with("row 199"), "tail kept");
        assert!(shown.contains("more line(s)"), "the elision is stated");
        assert!(!shown.contains("row 100\n"), "the middle is gone");
    }

    #[test]
    fn the_preview_caps_a_single_enormous_line() {
        let payload = format!("{}\n", "x".repeat(50_000));
        let shown = preview(payload.as_bytes());
        let first = shown.lines().next().unwrap_or_default();
        assert_eq!(
            first.chars().count(),
            PREVIEW_MAX_LINE_CHARS + 1,
            "the line is capped and the cap is marked"
        );
        assert!(first.ends_with('…'));
        assert!(
            shown.len() < PREVIEW_MAX_LINE_CHARS + 16,
            "no 50 KB line leaked"
        );
    }

    #[test]
    fn the_preview_redacts_credential_shaped_spans() {
        let payload = b"export ANTHROPIC_API_KEY=sk-ant-api03-REALKEYMATERIAL123456789\nok\n";
        let shown = preview(payload);
        assert!(
            !shown.contains("REALKEYMATERIAL123456789"),
            "the key value must not appear in the preview"
        );
        assert!(shown.contains(REDACTION));
        assert!(
            shown.contains("ANTHROPIC_API_KEY"),
            "the key name is still legible"
        );
    }

    #[test]
    fn redaction_covers_prefixed_keys_jwts_pem_blocks_and_assignments() {
        let cases = [
            "token is ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 here",
            "AKIAIOSFODNN7EXAMPLE is the key",
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz123456",
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            "-----BEGIN RSA PRIVATE KEY-----",
            "password: hunter2hunter2",
            "api_key = 'abcd1234efgh5678'",
        ];
        for case in cases {
            let out = redact_secrets(case);
            assert!(out.contains(REDACTION), "must redact: {case} (got {out})");
        }
    }

    #[test]
    fn redaction_leaves_ordinary_output_alone() {
        let ordinary = "src/main.rs:42: let total = items.iter().sum();\nbuild finished in 3.2s\n";
        assert_eq!(redact_secrets(ordinary), ordinary);
    }

    #[test]
    fn redaction_preserves_multibyte_text() {
        let text = "日本語の出力 — ok\n✓ done\n";
        assert_eq!(redact_secrets(text), text);
    }

    #[test]
    fn redaction_of_a_quoted_secret_keeps_the_quotes() {
        let out = redact_secrets("api_key = 'abcd1234efgh5678'");
        assert_eq!(out, "api_key = '[redacted]'");
    }

    #[test]
    fn a_whole_spool_lifecycle_keeps_the_stats_honest() {
        let dir = tmpdir("lifecycle");
        let spool = Spool::new(&dir);
        assert_eq!(spool.stats().blob_count, 0);
        let a = spool.write(&over_cap(), 1_700_000_000_000).unwrap();
        let b = spool
            .write(b"a different, smaller result entirely", 1_700_000_001_000)
            .unwrap();
        let stats = spool.stats();
        assert_eq!(stats.blob_count, 2);
        assert!(spool.contains(&a.hash));
        assert!(spool.contains(&b.hash));
        let total = stats.total_bytes;
        assert!(total > 0);
        assert!(total <= SPOOL_MAX_TOTAL_BYTES, "the ceiling always holds");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn backdated(ms: u64) -> std::time::SystemTime {
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms)
    }

    fn backdated_ms(ms: u64) -> u64 {
        ms
    }

    fn set_mtime(path: &Path, at: std::time::SystemTime) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(at).unwrap();
    }
}
