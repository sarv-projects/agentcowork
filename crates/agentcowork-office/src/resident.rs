//! FIX-14 / REQ-OFFICE-003 — **resident document contexts and exclusive
//! writer leases** (`ARCH/22` §1.3 + §4, `ARCH/25` §6, EDGE-050).
//!
//! Before this module the crate had the *primitives* — `write_atomic`,
//! `Snapshot` rollback, surgical patchers — but no **owner**. Two writers
//! could hold the same document open and the second save would silently win,
//! which `ARCH/22` §4 forbids ("second writer gets an explicit `in use`
//! result"). This module is that owner:
//!
//! * [`ResidentRegistry`] holds **one table per format** (`ARCH/22` §1.1 — one
//!   registry per format, shared by every surface).
//! * Each table holds **one resident context per open document**. Identity is
//!   the canonicalized path, so two spellings of one file are one document.
//! * A writable context holds an **exclusive writer lease** bound to the
//!   `work_id` that opened it (`ARCH/25` §6 — "lease ownership ties to
//!   `work_id`/worker"), never to a process or a socket.
//! * A lease **expires** on wall-clock time. Nothing has to release it, so a
//!   crashed writer's lease is reclaimed by the next acquisition attempt
//!   (crash-safe expiry, `ARCH/25` §6 / `REQ-FILES-008`).
//! * A second writer gets a typed [`ResidentError::InUse`] carrying the
//!   holder, the expiry, and the only two options v1 supports — `read_only`
//!   or `wait`. **There is no merge** (`ARCH/22` §4).
//! * Flush is **interval + dirty-marker driven**, explicit on session end,
//!   and idle contexts are evicted under a memory bound (`ARCH/22` §4). Every
//!   flush commits through [`crate::atomic::commit_bytes`] — stage → fsync →
//!   atomic swap (`REQ-OFFICE-004`).
//!
//! # Known gap — op-log replay
//!
//! `ARCH/22` §4 and REQ-OFFICE-004 also design an operation log whose replay
//! reconstructs a commit that crashed after the swap. **No op log and no replay
//! engine exist in this crate.** A crash before the swap is fully covered (the
//! target keeps its pre-edit bytes and the orphan staging package is
//! discoverable through [`crate::atomic::recover_orphans`]); a crash *after*
//! the swap needs no replay because the swap already landed. The real
//! remaining gap is a crash *during a multi-step batch*, which is `REQ-OFFICE-005`
//! territory and is not implemented here.
//!
//! # Scratch confinement
//!
//! Commit staging is a **sibling of the target** because `rename` is only
//! atomic within a filesystem. Document *roots* are declared out of band
//! ([`DocRoots`], the `GENOFFICE_ALLOWED_ROOTS` pattern named in `ARCH/22` §4);
//! when roots are declared, opening a document outside them is refused.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::atomic::{CommitError, CommitTrace, commit_bytes, verify_readback};
use crate::rollback::Snapshot;

/// Environment variable declaring the document roots the office runtime may
/// open and stage inside (the `GENOFFICE_ALLOWED_ROOTS` pattern,
/// `ARCH/22` §4 "scratch isolation"). Multiple roots use the platform path
/// separator (`:` on Unix, `;` on Windows).
pub const ENV_DOC_ROOTS: &str = "AGENTCOWORK_OFFICE_DOC_ROOTS";
/// Retired spelling of [`ENV_DOC_ROOTS`]: still honored as a read-only
/// fallback (DEC-053).
pub const LEGACY_ENV_DOC_ROOTS: &str = "EVERYAIOS_OFFICE_DOC_ROOTS";

/// The document formats this runtime owns. One resident table per format.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum DocFormat {
    /// Word document (`.docx`).
    Docx,
    /// Workbook (`.xlsx`).
    Xlsx,
    /// Presentation (`.pptx`).
    Pptx,
    /// Portable document (`.pdf`).
    Pdf,
}

impl DocFormat {
    /// Stable identifier for receipts and audit payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            DocFormat::Docx => "docx",
            DocFormat::Xlsx => "xlsx",
            DocFormat::Pptx => "pptx",
            DocFormat::Pdf => "pdf",
        }
    }

    /// Every format, in a stable order.
    pub fn all() -> [DocFormat; 4] {
        [
            DocFormat::Docx,
            DocFormat::Xlsx,
            DocFormat::Pptx,
            DocFormat::Pdf,
        ]
    }

    /// The format implied by a file extension (`None` when unknown).
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "docx" => Some(DocFormat::Docx),
            "xlsx" | "xlsm" => Some(DocFormat::Xlsx),
            "pptx" => Some(DocFormat::Pptx),
            "pdf" => Some(DocFormat::Pdf),
            _ => None,
        }
    }

    /// The format implied by a path's extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(DocFormat::from_extension)
    }
}

impl std::fmt::Display for DocFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who owns a writer lease. A lease is bound to a **work item**, not a
/// process: a crashed writer's lease is reclaimed by expiry, so there is no
/// durable lock file to go stale.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LeaseHolder {
    /// The work item the lease belongs to (`ARCH/25` §6).
    pub work_id: String,
    /// The session that opened the document.
    pub session_id: String,
}

/// An exclusive writer lease on one document.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WriterLease {
    /// The work item allowed to write.
    pub holder: LeaseHolder,
    /// Wall-clock ms when the lease was taken or last renewed.
    pub issued_at_ms: u64,
    /// Wall-clock ms after which the lease is reclaimable.
    pub expires_at_ms: u64,
}

impl WriterLease {
    /// Whether the lease has lapsed at `now_ms`.
    pub fn is_expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_at_ms
    }

    /// Milliseconds left, or `0` once lapsed.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.expires_at_ms.saturating_sub(now_ms)
    }

    /// Whether `work_id` may write under this lease at `now_ms`.
    pub fn admits(&self, work_id: &str, now_ms: u64) -> bool {
        !self.is_expired(now_ms) && self.holder.work_id == work_id
    }
}

/// A live lease on the document a second writer collided with. The message
/// is the "in use" result `REQ-OFFICE-003` requires, and `options` are the
/// only two resolutions v1 supports — **no merge**.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LeaseConflict {
    /// The document under lease.
    pub path: String,
    /// Its format.
    pub format: DocFormat,
    /// The current owner.
    pub holder: LeaseHolder,
    /// When the lease lapses and the document becomes acquirable.
    pub expires_at_ms: u64,
    /// `read_only` · `wait` — never a silent overwrite.
    pub options: Vec<String>,
}

impl std::fmt::Display for LeaseConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::fmt::Display for EvictionReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} evicted, {} flushed, {} retained, {} resident bytes",
            self.evicted.len(),
            self.flushed.len(),
            self.retained.len(),
            self.resident_bytes
        )
    }
}

impl std::fmt::Display for SessionCloseReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} receipts, {} retained",
            self.receipts.len(),
            self.retained.len()
        )
    }
}

impl LeaseConflict {
    /// The user-facing "in use" message.
    pub fn message(&self) -> String {
        format!(
            "{} is in use by work {} (session {}) until {} — open read-only or wait; \
             merge is not available in v1",
            self.path, self.holder.work_id, self.holder.session_id, self.expires_at_ms
        )
    }
}

/// Failures from the resident runtime. Every variant is typed and names the
/// document or the lease, so a surface can render an action instead of prose.
#[derive(Debug, thiserror::Error)]
pub enum ResidentError {
    #[error("{0}")]
    InUse(LeaseConflict),
    #[error("writer lease on {path} lapsed at {expires_at_ms}; renew it before writing")]
    LeaseExpired { path: String, expires_at_ms: u64 },
    #[error("writer lease on {path} is held by {found}, not {expected}")]
    NotLeaseHolder {
        path: String,
        expected: String,
        found: String,
    },
    #[error("no resident context for {path} ({format})")]
    NotResident { path: String, format: DocFormat },
    #[error("resident context for {path} is read-only")]
    ReadOnly { path: String },
    #[error("cannot infer the document format of {path}")]
    UnknownFormat { path: String },
    #[error("document {path} is outside every declared office document root")]
    OutsideDocRoots { path: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("commit failed: {0}")]
    Commit(#[from] CommitError),
}

impl ResidentError {
    /// The machine-readable conflict, when this is a lease conflict.
    pub fn conflict(&self) -> Option<&LeaseConflict> {
        match self {
            ResidentError::InUse(c) => Some(c),
            _ => None,
        }
    }

    /// Whether the caller can recover by waiting for the lease to lapse.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ResidentError::InUse(_) | ResidentError::LeaseExpired { .. }
        )
    }
}

impl From<LeaseConflict> for ResidentError {
    fn from(c: LeaseConflict) -> Self {
        ResidentError::InUse(c)
    }
}

/// Build the "in use" result for a document whose lease is live and held by
/// another work item. `options` are the only two resolutions v1 supports.
fn conflict_of(format: DocFormat, id: &str, lease: &WriterLease) -> LeaseConflict {
    LeaseConflict {
        path: id.to_string(),
        format,
        holder: lease.holder.clone(),
        expires_at_ms: lease.expires_at_ms,
        options: vec!["read_only".to_string(), "wait".to_string()],
    }
}

/// The declared document roots (`AGENTCOWORK_OFFICE_DOC_ROOTS`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocRoots {
    roots: Vec<PathBuf>,
}

impl DocRoots {
    /// The roots declared in the environment (empty when unset). The new
    /// `AGENTCOWORK_OFFICE_DOC_ROOTS` spelling wins; the retired
    /// `EVERYAIOS_OFFICE_DOC_ROOTS` is honored as a fallback.
    pub fn from_env() -> Self {
        let raw = agentcowork_types::env_compat::get("OFFICE_DOC_ROOTS")
            .or_else(|| std::env::var(LEGACY_ENV_DOC_ROOTS).ok());
        match raw {
            Some(raw) => Self::from_list(std::env::split_paths(&raw)),
            None => Self::undeclared(),
        }
    }

    /// Explicitly declared roots. An empty list means *undeclared*.
    pub fn from_list<I: IntoIterator<Item = PathBuf>>(roots: I) -> Self {
        let roots: Vec<PathBuf> = roots
            .into_iter()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| normalize(&p))
            .collect();
        Self { roots }
    }

    /// No roots declared: the caller's own path check is the boundary. This
    /// is the default so the module is usable without deployment config; a
    /// deployed shell declares roots and gets confinement.
    pub fn undeclared() -> Self {
        Self { roots: Vec::new() }
    }

    /// Whether any root is declared.
    pub fn is_declared(&self) -> bool {
        !self.roots.is_empty()
    }

    /// The declared roots.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Whether `path` is inside a declared root. Always `true` when no root is
    /// declared.
    pub fn contains(&self, path: &Path) -> bool {
        if self.roots.is_empty() {
            return true;
        }
        let target = normalize(path);
        self.roots.iter().any(|root| target.starts_with(root))
    }

    /// Refuse a document outside every declared root.
    pub fn require(&self, path: &Path) -> Result<(), ResidentError> {
        if self.contains(path) {
            Ok(())
        } else {
            Err(ResidentError::OutsideDocRoots {
                path: path.display().to_string(),
            })
        }
    }
}

/// Lexically normalize a path (resolve `.`/`..` without touching the
/// filesystem), then canonicalize it when it exists. Lease identity must be
/// stable whether or not the file is on disk yet.
fn normalize(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Wall-clock milliseconds since the Unix epoch (saturating at 0 before it).
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The flush/eviction policy (`ARCH/22` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FlushPolicy {
    /// Interval-driven flush: a dirty context whose last flush is older than
    /// this is a flush candidate.
    pub interval_ms: u64,
    /// Idle eviction: a context untouched for this long is a candidate.
    pub idle_ms: u64,
    /// Memory bound over the resident working set.
    pub max_resident_bytes: u64,
    /// Re-read the target after a commit and refuse the receipt on a
    /// mismatch (`ARCH/22` §5 — validate before a receipt).
    pub verify_readback: bool,
}

impl Default for FlushPolicy {
    fn default() -> Self {
        Self {
            interval_ms: 30_000,
            idle_ms: 300_000,
            max_resident_bytes: 256 * 1024 * 1024,
            verify_readback: true,
        }
    }
}

/// What a post-commit check found (the verification half of a receipt).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommitVerification {
    /// The target re-read after the swap matched the committed length.
    pub readback_checked: bool,
    /// The commit reached a durable directory entry.
    pub durable: bool,
    /// The number of commit stages executed, in order.
    pub stages: Vec<crate::atomic::CommitStage>,
}

/// A receipt for one committed document mutation (`ARCH/29` §3, the subset a
/// document runtime can honestly produce). Verification precedes the receipt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommitReceipt {
    /// The document's format.
    pub format: DocFormat,
    /// The committed target.
    pub path: String,
    /// The work item the commit was made under.
    pub work_id: String,
    /// The session the context belonged to.
    pub session_id: String,
    /// Committed byte length.
    pub bytes: usize,
    /// The pre-commit byte length the target held.
    pub previous_bytes: usize,
    /// Monotonic per-context commit counter.
    pub commit_seq: u64,
    /// Wall-clock ms of the commit.
    pub at_ms: u64,
    /// The commit's ordering evidence.
    pub trace: CommitTrace,
    /// The post-commit structural check.
    pub verification: CommitVerification,
}

impl CommitReceipt {
    /// The receipt's stable identity (`<path>#<seq>`), suitable for an
    /// effect reference.
    pub fn effect_ref(&self) -> String {
        format!("{}#{}", self.path, self.commit_seq)
    }
}

/// One resident context: the working bytes, the exclusive writer lease, the
/// pre-edit snapshot, and the flush state.
#[derive(Debug, Clone)]
pub struct ResidentContext {
    /// The document's format.
    pub format: DocFormat,
    /// The path as the caller supplied it (the receipt echoes it).
    pub path: PathBuf,
    /// Canonical identity the lease is bound to.
    pub identity: String,
    /// The exclusive writer lease; `None` for a read-only context.
    pub lease: Option<WriterLease>,
    /// The pre-edit snapshot (one-click undo target, kept in memory).
    pub snapshot: Option<Snapshot>,
    /// Working-set bytes currently held for this document.
    pub resident_bytes: usize,
    /// When the context was opened.
    pub opened_at_ms: u64,
    /// When the context was last read or written.
    pub last_access_ms: u64,
    /// When the context last committed to disk.
    pub last_flush_ms: u64,
    /// Monotonic commit counter.
    pub commit_seq: u64,
    /// The session that opened the context.
    pub session_id: String,
    /// The work item that opened the context.
    pub work_id: String,
    working: Vec<u8>,
    dirty: bool,
    read_only: bool,
}

impl ResidentContext {
    /// A fresh context holding `bytes` as the working set, with the pre-edit
    /// snapshot captured. The caller assigns the lease.
    fn new(
        format: DocFormat,
        path: PathBuf,
        identity: String,
        bytes: Vec<u8>,
        session_id: &str,
        now_ms: u64,
    ) -> Self {
        Self {
            format,
            path,
            identity,
            lease: None,
            snapshot: Some(Snapshot::capture(bytes.clone())),
            resident_bytes: bytes.len(),
            opened_at_ms: now_ms,
            last_access_ms: now_ms,
            last_flush_ms: now_ms,
            commit_seq: 0,
            session_id: session_id.to_string(),
            work_id: String::new(),
            working: bytes,
            dirty: false,
            read_only: true,
        }
    }

    /// Whether the context holds a live writer lease for `work_id`.
    pub fn is_writer(&self, work_id: &str, now_ms: u64) -> bool {
        self.lease
            .as_ref()
            .is_some_and(|l| l.admits(work_id, now_ms))
    }

    /// Whether uncommitted changes are held.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether the context is read-only.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// The working bytes.
    pub fn working(&self) -> &[u8] {
        &self.working
    }

    /// Replace the working bytes and mark the context dirty (the
    /// dirty-marker half of the flush policy).
    pub fn set_working(&mut self, bytes: Vec<u8>) {
        self.resident_bytes = bytes.len();
        self.working = bytes;
        self.dirty = true;
    }

    /// Apply a mutation to the working bytes in place.
    pub fn edit<F>(&mut self, f: F) -> Result<(), ResidentError>
    where
        F: FnOnce(&mut Vec<u8>) -> Result<(), String>,
    {
        if self.read_only {
            return Err(ResidentError::ReadOnly {
                path: self.path.display().to_string(),
            });
        }
        f(&mut self.working).map_err(|why| ResidentError::Io(std::io::Error::other(why)))?;
        self.dirty = true;
        Ok(())
    }

    /// Whether the interval half of the flush policy says "flush now".
    pub fn flush_due(&self, now_ms: u64, policy: &FlushPolicy) -> bool {
        self.dirty
            && !self.read_only
            && now_ms.saturating_sub(self.last_flush_ms) >= policy.interval_ms
    }

    /// Whether the context is idle past the policy's bound.
    pub fn is_idle(&self, now_ms: u64, policy: &FlushPolicy) -> bool {
        now_ms.saturating_sub(self.last_access_ms) >= policy.idle_ms
    }
}

/// An eviction report: what left the resident set and why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EvictionReport {
    /// Contexts removed.
    pub evicted: Vec<PathBuf>,
    /// Receipts produced by the flush that preceded each eviction.
    pub flushed: Vec<CommitReceipt>,
    /// Contexts retained because their flush failed or was refused.
    pub retained: Vec<PathBuf>,
    /// The resident working-set total after eviction.
    pub resident_bytes: u64,
}

/// What closing a session produced.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionCloseReport {
    /// Receipts for every document that committed.
    pub receipts: Vec<CommitReceipt>,
    /// Documents whose flush could not commit. They stay **resident with their
    /// edits intact** and their leases still expire — losing an edit is worse
    /// than holding memory.
    pub retained: Vec<PathBuf>,
}

/// One format's resident table — the exclusivity boundary for that format.
#[derive(Debug)]
pub struct ResidentTable {
    format: DocFormat,
    roots: DocRoots,
    policy: FlushPolicy,
    contexts: HashMap<String, ResidentContext>,
}

impl ResidentTable {
    /// An empty table for `format` under `roots`.
    pub fn new(format: DocFormat, roots: DocRoots) -> Self {
        Self {
            format,
            roots,
            policy: FlushPolicy::default(),
            contexts: HashMap::new(),
        }
    }

    /// Replace the flush/eviction policy.
    pub fn with_policy(mut self, policy: FlushPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// The format this table owns.
    pub fn format(&self) -> DocFormat {
        self.format
    }

    /// The declared document roots.
    pub fn roots(&self) -> &DocRoots {
        &self.roots
    }

    /// The active flush policy.
    pub fn policy(&self) -> &FlushPolicy {
        &self.policy
    }

    /// The number of resident contexts.
    pub fn len(&self) -> usize {
        self.contexts.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.contexts.is_empty()
    }

    /// The resident working-set total.
    pub fn resident_bytes(&self) -> u64 {
        self.contexts
            .values()
            .map(|c| c.resident_bytes as u64)
            .sum()
    }

    /// Snapshot of the resident documents (for a UI list / eviction preview).
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.contexts.values().map(|c| c.path.clone()).collect();
        out.sort();
        out
    }

    /// Borrow a resident context by path.
    pub fn get(&self, path: &Path) -> Option<&ResidentContext> {
        self.contexts.get(&identity_of(path))
    }

    /// Mutably borrow a resident context by path.
    pub fn get_mut(&mut self, path: &Path) -> Option<&mut ResidentContext> {
        self.contexts.get_mut(&identity_of(path))
    }

    /// Open `path` for writing under an exclusive lease bound to `work_id`.
    ///
    /// A live lease held by a *different* work item is a typed `InUse` with
    /// the holder and the options; a lapsed lease is reclaimed (crash-safe
    /// expiry); the same `work_id` re-opening renews rather than conflicting.
    pub fn open(
        &mut self,
        path: &Path,
        work_id: &str,
        session_id: &str,
        lease_ttl_ms: u64,
        now_ms: u64,
    ) -> Result<&mut ResidentContext, ResidentError> {
        self.roots.require(path)?;
        let id = identity_of(path);

        // The decision is computed from an immutable borrow first, then
        // applied — so the conflict is reported without holding a mutable
        // borrow of the table.
        if let Some(existing) = self.contexts.get(&id) {
            // A live lease held by another work item is the explicit "in use"
            // result. There is no merge in v1 (`ARCH/22` §4).
            if let Some(lease) = existing.lease.as_ref()
                && !lease.is_expired(now_ms)
                && lease.holder.work_id != work_id
            {
                return Err(ResidentError::InUse(conflict_of(self.format, &id, lease)));
            }
            let ctx = self.contexts.get_mut(&id).expect("checked above");
            ctx.last_access_ms = now_ms;
            // Live and ours: renew. Lapsed: reclaim — nothing has to release a
            // dead lease, which is what makes expiry crash-safe.
            let holder = match ctx.lease.as_ref() {
                Some(l) if l.holder.work_id == work_id => l.holder.clone(),
                _ => LeaseHolder {
                    work_id: work_id.to_string(),
                    session_id: session_id.to_string(),
                },
            };
            ctx.lease = Some(WriterLease {
                holder,
                issued_at_ms: now_ms,
                expires_at_ms: now_ms.saturating_add(lease_ttl_ms),
            });
            ctx.read_only = false;
            ctx.work_id = work_id.to_string();
            ctx.session_id = session_id.to_string();
            return Ok(ctx);
        }

        let mut ctx = ResidentContext::new(
            self.format,
            path.to_path_buf(),
            id.clone(),
            std::fs::read(path)?,
            session_id,
            now_ms,
        );
        ctx.lease = Some(WriterLease {
            holder: LeaseHolder {
                work_id: work_id.to_string(),
                session_id: session_id.to_string(),
            },
            issued_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(lease_ttl_ms),
        });
        ctx.read_only = false;
        ctx.work_id = work_id.to_string();
        Ok(self.contexts.entry(id).or_insert(ctx))
    }

    /// Open `path` for reading only, returning a **snapshot** of the document
    /// rather than the writer's live context.
    ///
    /// `read_only` is one of the two options the `InUse` result offers, so it
    /// must succeed while another work item holds the lease. The returned
    /// context carries no lease, is not registered in the table, and can
    /// therefore never mutate or commit.
    pub fn open_read_only(
        &mut self,
        path: &Path,
        session_id: &str,
        now_ms: u64,
    ) -> Result<ResidentContext, ResidentError> {
        self.roots.require(path)?;
        let id = identity_of(path);
        let mut ctx = match self.contexts.get(&id) {
            // A resident writer: the reader gets its own copy of the working
            // bytes, so the writer's context stays private to the writer.
            Some(existing) => existing.clone(),
            None => {
                let bytes = std::fs::read(path)?;
                let mut fresh = ResidentContext::new(
                    self.format,
                    path.to_path_buf(),
                    id.clone(),
                    bytes,
                    session_id,
                    now_ms,
                );
                fresh.work_id = String::new();
                fresh
            }
        };
        ctx.lease = None;
        ctx.read_only = true;
        ctx.last_access_ms = now_ms;
        Ok(ctx)
    }

    /// Extend a live lease. Fails when the lease has lapsed — a lapsed lease
    /// must be re-acquired, never silently resurrected.
    pub fn renew(
        &mut self,
        path: &Path,
        work_id: &str,
        lease_ttl_ms: u64,
        now_ms: u64,
    ) -> Result<(), ResidentError> {
        let id = identity_of(path);
        let ctx = self
            .contexts
            .get_mut(&id)
            .ok_or_else(|| ResidentError::NotResident {
                path: path.display().to_string(),
                format: self.format,
            })?;
        let lease = ctx
            .lease
            .as_mut()
            .ok_or_else(|| ResidentError::ReadOnly { path: id.clone() })?;
        if lease.is_expired(now_ms) {
            return Err(ResidentError::LeaseExpired {
                path: id,
                expires_at_ms: lease.expires_at_ms,
            });
        }
        if lease.holder.work_id != work_id {
            return Err(ResidentError::NotLeaseHolder {
                path: id,
                expected: work_id.to_string(),
                found: lease.holder.work_id.clone(),
            });
        }
        lease.issued_at_ms = now_ms;
        lease.expires_at_ms = now_ms.saturating_add(lease_ttl_ms);
        ctx.last_access_ms = now_ms;
        Ok(())
    }

    /// Commit a context's working bytes through the crash-safe path and
    /// return the receipt. Refuses without a live lease for `work_id`.
    pub fn flush(
        &mut self,
        path: &Path,
        work_id: &str,
        now_ms: u64,
    ) -> Result<Option<CommitReceipt>, ResidentError> {
        let id = identity_of(path);
        let (target, bytes, previous_bytes, session_id, seq) = {
            let ctx = self
                .contexts
                .get_mut(&id)
                .ok_or_else(|| ResidentError::NotResident {
                    path: path.display().to_string(),
                    format: self.format,
                })?;
            if ctx.read_only {
                return Err(ResidentError::ReadOnly {
                    path: path.display().to_string(),
                });
            }
            let lease = ctx.lease.as_ref().ok_or_else(|| ResidentError::ReadOnly {
                path: path.display().to_string(),
            })?;
            if lease.is_expired(now_ms) {
                return Err(ResidentError::LeaseExpired {
                    path: path.display().to_string(),
                    expires_at_ms: lease.expires_at_ms,
                });
            }
            if lease.holder.work_id != work_id {
                return Err(ResidentError::NotLeaseHolder {
                    path: path.display().to_string(),
                    expected: work_id.to_string(),
                    found: lease.holder.work_id.clone(),
                });
            }
            if !ctx.dirty {
                return Ok(None);
            }
            let previous_bytes = ctx.resident_bytes;
            let bytes = std::mem::take(&mut ctx.working);
            (
                ctx.path.clone(),
                bytes,
                previous_bytes,
                ctx.session_id.clone(),
                ctx.commit_seq + 1,
            )
        };

        // Commit outside the borrow: stage → fsync → swap, then the
        // post-commit read-back check.
        let trace = match commit_bytes(&target, &bytes) {
            Ok(t) => t,
            Err(e) => {
                // Commit failed: put the working bytes back so no edit is lost
                // and the target is still the pre-edit file.
                if let Some(ctx) = self.contexts.get_mut(&id) {
                    ctx.working = bytes;
                    ctx.resident_bytes = ctx.working.len();
                    ctx.dirty = true;
                }
                return Err(ResidentError::Commit(e));
            }
        };
        if self.policy.verify_readback {
            verify_readback(&target, bytes.len()).map_err(ResidentError::Commit)?;
        }

        let committed = bytes.len();
        let ctx = self.contexts.get_mut(&id).expect("context exists");
        ctx.resident_bytes = committed;
        ctx.last_flush_ms = now_ms;
        ctx.last_access_ms = now_ms;
        ctx.commit_seq = seq;
        ctx.dirty = false;
        if let Some(snap) = ctx.snapshot.as_mut() {
            snap.record_save(bytes.clone());
        }
        Ok(Some(CommitReceipt {
            format: self.format,
            path: target.display().to_string(),
            work_id: work_id.to_string(),
            session_id,
            bytes: committed,
            previous_bytes,
            commit_seq: seq,
            at_ms: now_ms,
            verification: CommitVerification {
                readback_checked: self.policy.verify_readback,
                durable: trace.is_durable(),
                stages: trace.stages.clone(),
            },
            trace,
        }))
    }

    /// Flush every context the interval policy says is due.
    pub fn flush_due(&mut self, now_ms: u64) -> Result<Vec<CommitReceipt>, ResidentError> {
        let due: Vec<PathBuf> = self
            .contexts
            .values()
            .filter(|c| c.flush_due(now_ms, &self.policy))
            .map(|c| c.path.clone())
            .collect();
        let mut out = Vec::with_capacity(due.len());
        for path in due {
            let work_id = self
                .contexts
                .get(&identity_of(&path))
                .map(|c| c.work_id.clone())
                .unwrap_or_default();
            if let Some(receipt) = self.flush(&path, &work_id, now_ms)? {
                out.push(receipt);
            }
        }
        Ok(out)
    }

    /// Close a context (session end): flush its dirty bytes, then evict. A
    /// flush that cannot commit keeps the context resident so no edit is lost.
    pub fn close(
        &mut self,
        path: &Path,
        work_id: &str,
        now_ms: u64,
    ) -> Result<Option<CommitReceipt>, ResidentError> {
        let id = identity_of(path);
        let ctx = self
            .contexts
            .get(&id)
            .ok_or_else(|| ResidentError::NotResident {
                path: path.display().to_string(),
                format: self.format,
            })?;
        if !ctx.lease.as_ref().is_none_or(|l| l.admits(work_id, now_ms)) {
            let found = ctx
                .lease
                .as_ref()
                .map(|l| l.holder.work_id.clone())
                .unwrap_or_default();
            return Err(ResidentError::NotLeaseHolder {
                path: path.display().to_string(),
                expected: work_id.to_string(),
                found,
            });
        }
        let receipt = self.flush(path, work_id, now_ms)?;
        self.contexts.remove(&id);
        Ok(receipt)
    }

    /// Evict idle contexts under the memory bound (`ARCH/22` §4). A dirty
    /// context is flushed first; if the flush cannot commit, the context is
    /// **retained** rather than dropped — losing edits is worse than memory.
    pub fn evict_idle(&mut self, now_ms: u64) -> Result<EvictionReport, ResidentError> {
        let mut report = EvictionReport {
            evicted: Vec::new(),
            flushed: Vec::new(),
            retained: Vec::new(),
            resident_bytes: 0,
        };
        let candidates: Vec<PathBuf> = self
            .contexts
            .values()
            .filter(|c| {
                c.is_idle(now_ms, &self.policy)
                    || self.resident_bytes() > self.policy.max_resident_bytes
            })
            .map(|c| c.path.clone())
            .collect();
        // Evict oldest first so the memory bound is reached in a stable order.
        let mut ordered = candidates;
        ordered.sort_by_key(|p| {
            self.contexts
                .get(&identity_of(p))
                .map(|c| c.last_access_ms)
                .unwrap_or(0)
        });
        for path in ordered {
            let id = identity_of(&path);
            let (work_id, dirty) = {
                let ctx = self.contexts.get(&id).expect("context exists");
                (ctx.work_id.clone(), ctx.dirty)
            };
            if dirty {
                match self.flush(&path, &work_id, now_ms) {
                    Ok(Some(receipt)) => report.flushed.push(receipt),
                    Ok(None) => {}
                    Err(_) => {
                        report.retained.push(path.clone());
                        continue;
                    }
                }
            }
            self.contexts.remove(&id);
            report.evicted.push(path);
        }
        report.resident_bytes = self.resident_bytes();
        Ok(report)
    }
}

/// The process-wide resident runtime: **one table per format**, shared by
/// every surface (CLI/MCP/GUI/agent/API resolve the same document identity and
/// therefore the same lease).
#[derive(Debug)]
pub struct ResidentRegistry {
    tables: HashMap<DocFormat, ResidentTable>,
}

impl Default for ResidentRegistry {
    fn default() -> Self {
        Self::new(DocRoots::from_env())
    }
}

impl ResidentRegistry {
    /// A registry over `roots`.
    pub fn new(roots: DocRoots) -> Self {
        let mut tables = HashMap::new();
        for format in DocFormat::all() {
            tables.insert(format, ResidentTable::new(format, roots.clone()));
        }
        Self { tables }
    }

    /// Replace every table's flush policy.
    pub fn with_policy(mut self, policy: FlushPolicy) -> Self {
        for format in DocFormat::all() {
            let table = self.tables.remove(&format).expect("one table per format");
            self.tables.insert(format, table.with_policy(policy));
        }
        self
    }

    /// The table for `format`.
    pub fn table(&mut self, format: DocFormat) -> &mut ResidentTable {
        self.tables
            .entry(format)
            .or_insert_with(|| ResidentTable::new(format, DocRoots::undeclared()))
    }

    /// The table for the format implied by `path`'s extension.
    pub fn table_for_path(&mut self, path: &Path) -> Result<&mut ResidentTable, ResidentError> {
        let format = DocFormat::from_path(path).ok_or_else(|| ResidentError::UnknownFormat {
            path: path.display().to_string(),
        })?;
        Ok(self.table(format))
    }

    /// Open a document for writing under an exclusive lease.
    pub fn open(
        &mut self,
        path: &Path,
        work_id: &str,
        session_id: &str,
        lease_ttl_ms: u64,
        now_ms: u64,
    ) -> Result<&mut ResidentContext, ResidentError> {
        self.table_for_path(path)?
            .open(path, work_id, session_id, lease_ttl_ms, now_ms)
    }

    /// Open a document read-only (a snapshot with no lease; allowed while a
    /// writer holds one).
    pub fn open_read_only(
        &mut self,
        path: &Path,
        session_id: &str,
        now_ms: u64,
    ) -> Result<ResidentContext, ResidentError> {
        self.table_for_path(path)?
            .open_read_only(path, session_id, now_ms)
    }

    /// The total resident context count across every format.
    pub fn contexts(&self) -> usize {
        self.tables.values().map(|t| t.len()).sum()
    }

    /// The total resident working set across every format.
    pub fn resident_bytes(&self) -> u64 {
        self.tables.values().map(|t| t.resident_bytes()).sum()
    }

    /// Every resident document path, sorted.
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.tables.values().flat_map(|t| t.paths()).collect();
        out.sort();
        out
    }

    /// Flush every context due under its table's interval policy.
    pub fn flush_due(&mut self, now_ms: u64) -> Result<Vec<CommitReceipt>, ResidentError> {
        let mut out = Vec::new();
        for format in DocFormat::all() {
            if let Some(table) = self.tables.get_mut(&format) {
                out.extend(table.flush_due(now_ms)?);
            }
        }
        Ok(out)
    }

    /// Evict idle contexts under the memory bound, across every format.
    pub fn evict_idle(&mut self, now_ms: u64) -> Result<Vec<EvictionReport>, ResidentError> {
        let mut out = Vec::new();
        for format in DocFormat::all() {
            if let Some(table) = self.tables.get_mut(&format) {
                let report = table.evict_idle(now_ms)?;
                if !report.evicted.is_empty() || !report.retained.is_empty() {
                    out.push(report);
                }
            }
        }
        Ok(out)
    }

    /// Close every context a session opened (explicit flush on session end).
    pub fn close_session(
        &mut self,
        session_id: &str,
        now_ms: u64,
    ) -> Result<SessionCloseReport, ResidentError> {
        let mut receipts = Vec::new();
        let mut retained = Vec::new();
        for format in DocFormat::all() {
            let Some(table) = self.tables.get_mut(&format) else {
                continue;
            };
            let owned: Vec<PathBuf> = table
                .paths()
                .into_iter()
                .filter(|p| table.get(p).is_some_and(|c| c.session_id == session_id))
                .collect();
            for path in owned {
                let work_id = table
                    .get(&path)
                    .map(|c| c.work_id.clone())
                    .unwrap_or_default();
                match table.close(&path, &work_id, now_ms) {
                    Ok(receipt) => {
                        if let Some(r) = receipt {
                            receipts.push(r);
                        }
                    }
                    // The flush failed: the context is untouched and still
                    // resident. Report it instead of dropping the edits.
                    Err(_) => retained.push(path),
                }
            }
        }
        Ok(SessionCloseReport { receipts, retained })
    }
}

/// The lease identity of a path: the canonicalized (or lexically normalized)
/// path string. Two spellings of one document are one lease.
pub fn identity_of(path: &Path) -> String {
    normalize(path).to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Process-wide resident runtime.
//
// The registry is per-process by nature (one desktop process owns the open
// documents), so the shell's commit paths reach it without threading new state
// through the Tauri `AppState`. `office_cmds`/`xlsx_cmds` take a short-lived
// lease around each commit; a long-lived open/close surface belongs to the
// document UI, which is out of this crate's scope.
// ---------------------------------------------------------------------------

static PROCESS_REGISTRY: std::sync::OnceLock<std::sync::Mutex<ResidentRegistry>> =
    std::sync::OnceLock::new();

/// The process-wide resident registry.
pub fn process_registry() -> &'static std::sync::Mutex<ResidentRegistry> {
    PROCESS_REGISTRY
        .get_or_init(|| std::sync::Mutex::new(ResidentRegistry::new(DocRoots::from_env())))
}

/// Commit `bytes` to `path` under a short-lived exclusive lease.
///
/// This is the shape the Tauri commit paths use: a document is not held
/// resident between user actions, so the lease is taken for the commit and
/// released after it. Two concurrent writers still collide — the second gets
/// [`ResidentError::InUse`] with the holder and its expiry — which is the
/// exclusivity REQ-OFFICE-003 asks for, and the commit itself always runs
/// through [`crate::atomic::commit_bytes`] (stage → fsync → atomic swap).
pub fn commit_under_lease(
    path: &Path,
    bytes: &[u8],
    work_id: &str,
    session_id: &str,
    now_ms: u64,
) -> Result<CommitReceipt, ResidentError> {
    let mut registry = process_registry()
        .lock()
        .map_err(|_| std::io::Error::other("resident registry poisoned"))?;
    let format = DocFormat::from_path(path).ok_or_else(|| ResidentError::UnknownFormat {
        path: path.display().to_string(),
    })?;
    let table = registry.table(format);
    table
        .open(path, work_id, session_id, DEFAULT_LEASE_TTL_MS, now_ms)?
        .set_working(bytes.to_vec());
    let receipt = table.flush(path, work_id, now_ms)?;
    // The document is not held resident between UI actions: close the context
    // so the next surface re-reads the committed bytes from disk.
    let _ = table.close(path, work_id, now_ms);
    receipt.ok_or(ResidentError::NotResident {
        path: path.display().to_string(),
        format,
    })
}

/// Default writer-lease time-to-live: long enough for a long edit, short
/// enough that a crashed writer is reclaimed promptly.
pub const DEFAULT_LEASE_TTL_MS: u64 = 5 * 60_000;

/// Monotonic counter for synthetic work ids (used by the commit path when the
/// caller has no work item, so the lease is still work-bound).
static SYNTHETIC_WORK: AtomicU64 = AtomicU64::new(0);

/// A work id for a commit that arrives without one (a plain UI gesture). The
/// lease is still work-bound — the id names the synthetic work item.
pub fn synthetic_work_id(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        SYNTHETIC_WORK.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "agentcowork-resident-{tag}-{}-{}",
                std::process::id(),
                SYNTHETIC_WORK.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        fn docx(&self, name: &str, body: &[u8]) -> PathBuf {
            let p = self.dir.join(name);
            std::fs::write(&p, body).unwrap();
            p
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn registry() -> ResidentRegistry {
        ResidentRegistry::new(DocRoots::undeclared())
    }

    // -----------------------------------------------------------------
    // REQ-OFFICE-003 — one resident context + exclusive writer lease.
    // -----------------------------------------------------------------

    #[test]
    fn second_writer_is_refused_with_a_typed_in_use_result() {
        let fx = Fixture::new("exclusive");
        let doc = fx.docx("a.docx", b"v1");
        let mut reg = registry();

        let t0 = 1_000;
        reg.open(&doc, "work-1", "session-1", 60_000, t0).unwrap();
        assert_eq!(reg.contexts(), 1);

        let err = reg
            .open(&doc, "work-2", "session-2", 60_000, t0 + 1_000)
            .unwrap_err();
        let conflict = err.conflict().expect("InUse carries a conflict");
        assert_eq!(conflict.holder.work_id, "work-1");
        assert_eq!(conflict.holder.session_id, "session-1");
        assert_eq!(conflict.format, DocFormat::Docx);
        assert_eq!(conflict.expires_at_ms, t0 + 60_000);
        // Exactly the two options v1 supports — never a silent overwrite.
        assert_eq!(conflict.options, vec!["read_only", "wait"]);
        assert!(err.is_retryable());
        assert!(conflict.message().contains("in use by work work-1"));

        // The document is still one context, owned by the first writer.
        assert_eq!(reg.contexts(), 1);
        assert_eq!(std::fs::read(&doc).unwrap(), b"v1");
    }

    #[test]
    fn same_work_reopening_renews_instead_of_conflicting() {
        let fx = Fixture::new("renew");
        let doc = fx.docx("a.docx", b"v1");
        let mut reg = registry();
        reg.open(&doc, "work-1", "session-1", 10_000, 0).unwrap();
        reg.open(&doc, "work-1", "session-1", 10_000, 5_000)
            .unwrap();
        let ctx = reg.table(DocFormat::Docx).get(&doc).expect("resident");
        let lease = ctx.lease.as_ref().unwrap();
        assert_eq!(lease.issued_at_ms, 5_000);
        assert_eq!(lease.expires_at_ms, 15_000);
        assert_eq!(reg.contexts(), 1);
    }

    #[test]
    fn an_expired_lease_is_reclaimed_crash_safely() {
        let fx = Fixture::new("expiry");
        let doc = fx.docx("a.docx", b"v1");
        let mut reg = registry();
        // work-1 takes the lease and then "crashes" — nothing is ever released.
        reg.open(&doc, "work-1", "session-1", 10_000, 0).unwrap();

        // Before expiry the second writer is still refused.
        assert!(
            reg.open(&doc, "work-2", "s2", 10_000, 9_999)
                .unwrap_err()
                .conflict()
                .is_some()
        );

        // At expiry the lease lapses and the document is acquirable without
        // any recovery action by the dead writer.
        let ctx = reg
            .table(DocFormat::Docx)
            .open(&doc, "work-2", "s2", 10_000, 10_000)
            .unwrap();
        assert_eq!(ctx.lease.as_ref().unwrap().holder.work_id, "work-2");
        assert_eq!(reg.contexts(), 1);
    }

    #[test]
    fn renew_refuses_a_lapsed_lease() {
        let fx = Fixture::new("renew-expired");
        let doc = fx.docx("a.docx", b"v1");
        let mut reg = registry();
        reg.open(&doc, "work-1", "s1", 5_000, 0).unwrap();
        let table = reg.table(DocFormat::Docx);
        assert!(matches!(
            table.renew(&doc, "work-1", 5_000, 6_000),
            Err(ResidentError::LeaseExpired { .. })
        ));
        // Renewing a lease held by another work item is refused by identity.
        table.open(&doc, "work-1", "s1", 5_000, 0).unwrap();
        assert!(matches!(
            table.renew(&doc, "work-9", 5_000, 1_000),
            Err(ResidentError::NotLeaseHolder { .. })
        ));
    }

    #[test]
    fn read_only_open_is_one_of_the_offered_options() {
        let fx = Fixture::new("readonly");
        let doc = fx.docx("a.docx", b"v1");
        let mut reg = registry();
        reg.open(&doc, "work-1", "s1", 60_000, 0).unwrap();
        // While a writer holds the lease, `read_only` still works...
        let mut ro = reg.open_read_only(&doc, "s2", 1_000).unwrap();
        assert!(ro.is_read_only());
        assert!(ro.lease.is_none());
        // ...but it can never mutate or commit.
        assert!(matches!(
            ro.edit(|_| Ok(())),
            Err(ResidentError::ReadOnly { .. })
        ));
        // The writer's own context is untouched by the reader.
        let writer = reg.table(DocFormat::Docx).get(&doc).unwrap();
        assert!(!writer.is_read_only());
        assert!(writer.lease.as_ref().unwrap().admits("work-1", 1_000));
        // A document nobody holds opens read-only straight from disk.
        let cold = fx.docx("cold.docx", b"cold");
        let ro2 = reg.open_read_only(&cold, "s3", 2_000).unwrap();
        assert!(ro2.is_read_only());
        assert_eq!(ro2.working(), b"cold");
        // A read-only open does not register a second context.
        assert_eq!(reg.contexts(), 1);
    }

    #[test]
    fn flush_requires_a_live_lease_and_holder() {
        let fx = Fixture::new("flush-lease");
        let doc = fx.docx("a.docx", b"v1");
        let mut reg = registry();
        {
            let ctx = reg.open(&doc, "work-1", "s1", 5_000, 0).unwrap();
            ctx.set_working(b"v2".to_vec());
        }
        // Another work item cannot commit this document.
        assert!(matches!(
            reg.table(DocFormat::Docx).flush(&doc, "work-2", 1_000),
            Err(ResidentError::NotLeaseHolder { .. })
        ));
        // An expired lease cannot commit either.
        assert!(matches!(
            reg.table(DocFormat::Docx).flush(&doc, "work-1", 9_999),
            Err(ResidentError::LeaseExpired { .. })
        ));
        // The owner can, and the target now holds the new bytes.
        let receipt = reg
            .table(DocFormat::Docx)
            .flush(&doc, "work-1", 1_000)
            .unwrap()
            .expect("dirty context commits");
        assert_eq!(std::fs::read(&doc).unwrap(), b"v2");
        assert_eq!(receipt.bytes, 2);
        assert!(receipt.verification.durable);
        assert!(receipt.verification.readback_checked);
        assert_eq!(
            receipt.verification.stages[1],
            crate::atomic::CommitStage::Fsynced
        );
    }

    #[test]
    fn a_clean_context_flush_is_a_no_op() {
        let fx = Fixture::new("clean");
        let doc = fx.docx("a.docx", b"v1");
        let mut reg = registry();
        reg.open(&doc, "work-1", "s1", 60_000, 0).unwrap();
        assert!(
            reg.table(DocFormat::Docx)
                .flush(&doc, "work-1", 1_000)
                .unwrap()
                .is_none()
        );
    }

    // -----------------------------------------------------------------
    // ARCH/22 §4 — flush policy.
    // -----------------------------------------------------------------

    #[test]
    fn interval_flush_fires_only_when_dirty_and_due() {
        let fx = Fixture::new("interval");
        let doc = fx.docx("a.docx", b"v1");
        let policy = FlushPolicy {
            interval_ms: 10_000,
            idle_ms: 1_000_000,
            max_resident_bytes: u64::MAX,
            verify_readback: true,
        };
        let mut reg = registry().with_policy(policy);
        {
            let ctx = reg.open(&doc, "work-1", "s1", 600_000, 0).unwrap();
            assert!(!ctx.is_dirty());
        }
        // Not dirty → nothing is due, however old the context is.
        assert!(
            reg.flush_due(999_999).unwrap().is_empty(),
            "a clean context must not be committed"
        );
        {
            let ctx = reg.table(DocFormat::Docx).get_mut(&doc).unwrap();
            ctx.set_working(b"v2".to_vec());
        }
        // Dirty but not yet due.
        assert!(reg.flush_due(5_000).unwrap().is_empty());
        // Dirty and due → committed.
        let receipts = reg.flush_due(10_000).unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(std::fs::read(&doc).unwrap(), b"v2");
    }

    #[test]
    fn session_end_flushes_then_releases() {
        let fx = Fixture::new("session-end");
        let a = fx.docx("a.docx", b"a1");
        let b = fx.docx("b.pdf", b"b1");
        let c = fx.docx("c.xlsx", b"c1");
        let mut reg = registry();
        {
            let ctx = reg.open(&a, "work-1", "s1", 600_000, 0).unwrap();
            ctx.set_working(b"a2".to_vec());
        }
        {
            let ctx = reg.open(&b, "work-2", "s1", 600_000, 0).unwrap();
            ctx.set_working(b"b2".to_vec());
        }
        {
            let ctx = reg.open(&c, "work-3", "s2", 600_000, 0).unwrap();
            let _ = ctx;
        }

        let report = reg.close_session("s1", 1_000).unwrap();
        assert_eq!(report.receipts.len(), 2);
        assert!(report.retained.is_empty());
        assert_eq!(std::fs::read(&a).unwrap(), b"a2");
        assert_eq!(std::fs::read(&b).unwrap(), b"b2");
        // The other session's document is untouched and still resident.
        assert_eq!(reg.contexts(), 1);
        assert_eq!(std::fs::read(&c).unwrap(), b"c1");
        // A later writer can take the released documents immediately.
        assert!(reg.open(&a, "work-4", "s4", 1_000, 2_000).is_ok());
    }

    #[test]
    fn session_end_reports_a_document_it_could_not_commit() {
        let fx = Fixture::new("session-retain");
        let doc = fx.docx("a.docx", b"v1");
        let mut reg = registry();
        {
            let ctx = reg.open(&doc, "work-1", "s1", 600_000, 0).unwrap();
            ctx.set_working(b"v2".to_vec());
        }
        // Make the commit impossible after the open: the target path becomes a
        // non-empty directory, so the staging file cannot be swapped over it.
        std::fs::remove_file(&doc).unwrap();
        std::fs::create_dir(&doc).unwrap();
        std::fs::write(doc.join("blocker"), b"x").unwrap();

        let report = reg.close_session("s1", 1_000).unwrap();
        assert!(report.receipts.is_empty());
        assert_eq!(report.retained, vec![doc.clone()]);
        // The pending edit survives, still resident and still dirty.
        let ctx = reg.table(DocFormat::Docx).get(&doc).unwrap();
        assert!(ctx.is_dirty());
        assert_eq!(ctx.working(), b"v2");
    }

    #[test]
    fn idle_eviction_flushes_dirty_contexts_and_respects_the_memory_bound() {
        let fx = Fixture::new("evict");
        let small = fx.docx("small.docx", b"12345");
        let big = fx.docx("big.docx", &[b'x'; 4_096]);
        let policy = FlushPolicy {
            interval_ms: 1_000_000,
            idle_ms: 5_000,
            max_resident_bytes: 100,
            verify_readback: true,
        };
        let mut reg = registry().with_policy(policy);
        {
            let ctx = reg.open(&small, "work-1", "s1", 600_000, 0).unwrap();
            ctx.set_working(b"edited".to_vec());
        }
        {
            let ctx = reg.open(&big, "work-2", "s2", 600_000, 0).unwrap();
            let _ = ctx;
        }
        assert!(reg.resident_bytes() > 100);

        // Oldest access first: the small (edited) document is flushed, not lost.
        let reports = reg.evict_idle(6_000).unwrap();
        assert_eq!(reports.len(), 1);
        let report = &reports[0];
        assert_eq!(report.evicted.len(), 2);
        assert!(report.retained.is_empty());
        assert_eq!(
            report.flushed.len(),
            1,
            "the dirty document is flushed first"
        );
        assert_eq!(report.flushed[0].path, small.display().to_string());
        assert_eq!(std::fs::read(&small).unwrap(), b"edited");
        // The clean document keeps its original bytes.
        assert_eq!(std::fs::read(&big).unwrap(), vec![b'x'; 4_096]);
        assert_eq!(reg.contexts(), 0);
        assert_eq!(reg.resident_bytes(), 0);
    }

    #[test]
    fn eviction_retains_a_document_whose_flush_cannot_commit() {
        let fx = Fixture::new("evict-retain");
        let doc = fx.docx("a.docx", b"v1");
        let policy = FlushPolicy {
            idle_ms: 1,
            ..FlushPolicy::default()
        };
        let mut reg = registry().with_policy(policy);
        {
            let ctx = reg.open(&doc, "work-1", "s1", 600_000, 0).unwrap();
            ctx.set_working(b"edited".to_vec());
        }
        // After the open, the target path becomes a non-empty directory, so the
        // commit cannot swap the staging package over it.
        std::fs::remove_file(&doc).unwrap();
        std::fs::create_dir(&doc).unwrap();
        std::fs::write(doc.join("blocker"), b"x").unwrap();

        let reports = reg.evict_idle(10).unwrap();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].evicted.is_empty());
        assert_eq!(reports[0].retained, vec![doc.clone()]);
        assert_eq!(reg.contexts(), 1);
        let ctx = reg.table(DocFormat::Docx).get(&doc).unwrap();
        assert!(ctx.is_dirty(), "pending edits survive a failed flush");
        assert_eq!(ctx.working(), b"edited");
    }

    // -----------------------------------------------------------------
    // One table per format + document-root confinement.
    // -----------------------------------------------------------------

    #[test]
    fn each_format_has_its_own_table_and_exclusivity_boundary() {
        let fx = Fixture::new("per-format");
        let docx = fx.docx("same.docx", b"1");
        let pptx = fx.docx("same.pptx", b"1");
        let mut reg = registry();
        // Identical file *name*, different formats: different documents.
        reg.open(&docx, "work-1", "s1", 60_000, 0).unwrap();
        reg.open(&pptx, "work-1", "s1", 60_000, 0).unwrap();
        assert_eq!(reg.contexts(), 2);
        assert_eq!(reg.table(DocFormat::Docx).len(), 1);
        assert_eq!(reg.table(DocFormat::Pptx).len(), 1);
        assert_eq!(reg.table(DocFormat::Pdf).len(), 0);
        // Unknown extensions are refused with a typed error.
        let err = reg
            .open(&fx.dir.join("notes.txt"), "work-1", "s1", 60_000, 0)
            .unwrap_err();
        assert!(matches!(err, ResidentError::UnknownFormat { .. }));
    }

    #[test]
    fn two_spellings_of_one_path_are_one_lease() {
        let fx = Fixture::new("identity");
        let doc = fx.docx("a.docx", b"v1");
        let roundabout = fx.dir.join("sub").join("..").join("a.docx");
        std::fs::create_dir_all(fx.dir.join("sub")).unwrap();
        let mut reg = registry();
        reg.open(&doc, "work-1", "s1", 60_000, 0).unwrap();
        // `..` cannot smuggle in a second writer.
        let err = reg
            .open(&roundabout, "work-2", "s2", 60_000, 1)
            .unwrap_err();
        assert!(err.conflict().is_some());
        assert_eq!(reg.contexts(), 1);
    }

    #[test]
    fn declared_document_roots_confine_opens() {
        let fx = Fixture::new("roots");
        let inside = fx.docx("a.docx", b"v1");
        let outside_dir = Fixture::new("roots-outside");
        let outside = outside_dir.docx("b.docx", b"v1");

        let mut reg = ResidentRegistry::new(DocRoots::from_list([fx.dir.clone()]));
        reg.open(&inside, "work-1", "s1", 60_000, 0).unwrap();
        let err = reg.open(&outside, "work-1", "s1", 60_000, 0).unwrap_err();
        assert!(matches!(err, ResidentError::OutsideDocRoots { .. }));
        assert!(err.conflict().is_none());
        // Undeclared roots defer to the caller's own path check.
        assert!(DocRoots::undeclared().contains(&outside));
    }

    #[test]
    fn doc_roots_from_env_is_empty_when_unset() {
        // Neither spelling is set in the test process, so the default policy is
        // "undeclared" — documented, not a silent confinement claim.
        if std::env::var(ENV_DOC_ROOTS).is_err() && std::env::var(LEGACY_ENV_DOC_ROOTS).is_err() {
            assert!(!DocRoots::from_env().is_declared());
        }
    }

    #[test]
    fn snapshot_gives_a_one_click_undo_per_resident_context() {
        let fx = Fixture::new("undo");
        let doc = fx.docx("a.docx", b"v1");
        let mut reg = registry();
        let ctx = reg.open(&doc, "work-1", "s1", 60_000, 0).unwrap();
        assert_eq!(ctx.snapshot.as_ref().unwrap().original(), b"v1");
        ctx.set_working(b"v2".to_vec());
        reg.table(DocFormat::Docx)
            .flush(&doc, "work-1", 1)
            .unwrap()
            .unwrap();
        let ctx = reg.table(DocFormat::Docx).get(&doc).unwrap();
        assert_eq!(ctx.snapshot.as_ref().unwrap().original(), b"v1");
        assert!(ctx.snapshot.as_ref().unwrap().dirty());
    }
}
