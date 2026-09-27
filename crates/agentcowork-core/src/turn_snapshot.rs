//! P69.G2 — the turn-atomic file snapshot store (`ARCH/RECOVERY.md` §10).
//!
//! One mutating step can change several files. If the third of five writes
//! fails, the first two must not be left behind: §10 requires the whole change
//! set to come back. What existed before this module was
//! `agentcowork_office::rollback::Snapshot` — one document's bytes, in memory,
//! with `undo()` for a single file. That is a one-file undo, not a turn
//! transaction, and it is the only thing in the tree called "rollback".
//!
//! This module is the turn-scoped transaction, and it is deliberately narrow:
//!
//! * **The bytes never leave Rust.** Capture reads a file and writes the
//!   pre-image into this store; restore writes it back. Nothing here is
//!   serializable into an event, a log, an error, or an IPC payload — the
//!   journaled [`TurnChangeSet`] carries digests and store *references* only.
//!   A sidecar or a TypeScript process can therefore never hold a pre-image.
//! * **One record per turn step.** The set is appended to the **existing** Work
//!   journal by [`WorkGateway::record_file_change_set`], keyed by
//!   `(WorkId, StepId)`. There is no second event log, and the store directory
//!   is a row in the existing durable-store registry.
//! * **All-or-nothing, or `uncertain`.** A restore that cannot be *verified* is
//!   recorded [`ChangeSetOutcome::Uncertain`] with a `has_gap` receipt. The
//!   documented `RolledBackDueToFailure` verdict is reachable only when every
//!   captured file was written back and its digest re-checked.
//! * **Leases are recorded, not assumed.** Each entry carries the
//!   [`LeaseAttachment`] that covered the write, or the literal `"none"` — a
//!   `ResourceLease`/fence (ADR-0008) fact beside the bytes, never inside them.
//! * **Pathfloor everywhere.** A capture/restore target is canonicalized and
//!   floored against the workspace root through the same
//!   `agentcowork_guard::pathfloor` check the write tools use, so a snapshot can
//!   never become a way to read or write outside it.

use std::io::Write;
use std::path::{Path, PathBuf};

use agentcowork_audit::AuditEvent;
use agentcowork_audit::receipt::EffectReceipt;
use agentcowork_guard::pathfloor::{FloorVerdict, enforce_floor};
use agentcowork_types::turn_snapshot::{
    ChangeSetOutcome, FileCaptureKind, FileSnapshotEntry, TurnChangeSet,
};
use agentcowork_types::workbench::LeaseAttachment;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::work_gateway::WorkGateway;

/// The store directory, relative to the data dir. Named here so the
/// durable-store registry row and this module cannot drift.
pub const SNAPSHOT_DIR: &str = "snapshots";

/// Refusals the capture/restore boundary can produce. Every one of them leaves
/// the filesystem as it was, except [`TurnSnapshotError::RestoreUnverified`],
/// which is returned *after* an attempt and carries the partial result.
///
/// `std::io::Error` is neither `Eq` nor `Clone`, so tests compare with
/// `matches!` rather than `==` — the same discipline `store_schema` uses.
#[derive(Debug, Error)]
pub enum TurnSnapshotError {
    /// The path is not inside the workspace floor. The verdict is named so a
    /// reader can tell a lexical escape from a symlink escape.
    #[error("path floor refused {path}: {verdict:?} (snapshots never leave the workspace)")]
    PathFloor { path: String, verdict: FloorVerdict },
    /// A target exists but is not a regular file (a directory, socket, …).
    #[error("snapshot target is not a regular file: {0}")]
    NotAFile(String),
    #[error("read snapshot target {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("write snapshot pre-image for {path}: {source}")]
    WritePreImage {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("restore {path} to its pre-mutation state: {source}")]
    Restore {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// The set is recorded but its verdict has already been settled.
    #[error("change set `{0}` is already settled")]
    AlreadySettled(String),
    /// The set is not on the Work journal — there is nothing to roll back from.
    #[error("no change set recorded for work `{work_id}` step `{step_id}`")]
    SetNotRecorded { work_id: String, step_id: String },
    /// A stored pre-image is missing or does not match its recorded digest, so
    /// the restore cannot be *verified*. The world is left untouched and the
    /// outcome is `uncertain`, never "rolled back".
    #[error("stored pre-image for {path} is missing or does not match the captured digest")]
    PreImageUnusable { path: String },
    #[error(
        "change set `{change_set_id}` could not be fully restored; it is uncertain: {failed:?}"
    )]
    RestoreUnverified {
        change_set_id: String,
        failed: Vec<String>,
    },
    #[error("record the change set on the Work journal: {0}")]
    Journal(String),
}

/// The audit seam. The store never owns a log: it emits [`AuditEvent`]s and the
/// caller hands them to the writer it already owns — the kernel's
/// [`agentcowork_audit::merkle::MerkleChain`], the shell's `AuditWriter`, or a
/// test double. That is what keeps §10 from introducing a second audit trail.
pub trait ChangeSetAudit {
    /// Record one audit event on the caller's existing writer.
    fn record(&mut self, event: AuditEvent);
}

impl ChangeSetAudit for agentcowork_audit::merkle::MerkleChain {
    fn record(&mut self, event: AuditEvent) {
        self.push(event);
    }
}

/// One file a mutating step is about to change, with the lease covering it.
///
/// There is no content field by construction: the store reads the bytes itself,
/// so a caller cannot hand it (or a sidecar) a file body.
#[derive(Debug, Clone)]
pub struct CaptureTarget {
    /// Path as the tool call named it; floored against the workspace.
    pub path: String,
    /// The `ResourceLease` (ADR-0008) covering the write, or `None` for a write
    /// that is not under a lease. Recorded either way.
    pub lease: Option<LeaseAttachment>,
}

impl CaptureTarget {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            lease: None,
        }
    }

    pub fn under_lease(mut self, lease: LeaseAttachment) -> Self {
        self.lease = Some(lease);
        self
    }
}

/// Everything one capture needs. Time is supplied by the caller so a test is
/// deterministic and the durable timestamp is the caller's, not a hidden clock.
#[derive(Debug, Clone)]
pub struct CaptureRequest {
    pub work_id: String,
    pub step_id: String,
    pub session_id: Option<agentcowork_types::SessionId>,
    pub files: Vec<CaptureTarget>,
    pub now_ms: u64,
}

/// What a rollback attempt actually did. The outcome is the honest verdict;
/// `receipt` is the `has_gap`-carrying per-effect proof, and `audit_event` is
/// the single row the caller records.
#[derive(Debug, Clone)]
pub struct RollbackOutcome {
    pub change_set_id: String,
    pub outcome: ChangeSetOutcome,
    /// Files whose bytes were written back, in set order.
    pub restored: Vec<String>,
    /// Files deleted because the step had created them.
    pub deleted: Vec<String>,
    /// Paths that could not be written back or verified.
    pub failed: Vec<String>,
    pub receipt: EffectReceipt,
    pub audit_event: AuditEvent,
}

impl RollbackOutcome {
    /// Whether the filesystem is provably back at the pre-mutation state.
    pub fn is_fully_rolled_back(&self) -> bool {
        self.outcome.is_fully_restored()
    }
}

/// The durable, turn-scoped snapshot store.
///
/// Rooted at `<data_dir>/snapshots`, floored against the workspace the mutating
/// tools operate on. One instance per data dir; the pre-images are content
/// keyed inside a per-`(work, step)` directory so a replay of the same capture
/// is idempotent.
#[derive(Debug, Clone)]
pub struct TurnSnapshotStore {
    root: PathBuf,
    workspace: PathBuf,
}

impl TurnSnapshotStore {
    /// `data_dir` is the app data directory (`~/.agentcowork`), `workspace` the
    /// root the mutating tools are floored against.
    pub fn new(data_dir: &Path, workspace: &Path) -> Self {
        Self {
            root: data_dir.join(SNAPSHOT_DIR),
            workspace: workspace.to_path_buf(),
        }
    }

    /// The durable store root. Exposed so a retention/inspection surface can
    /// see the directory without re-deriving the name.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Canonicalize + floor a target path. This is the same floor the write
    /// tools apply (`ToolService::floor_path`), so a snapshot is incapable of
    /// being a way around it.
    pub fn floor(&self, path: &str) -> Result<PathBuf, TurnSnapshotError> {
        let joined = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.workspace.join(path)
        };
        let target = joined.to_string_lossy();
        let root = self.workspace.to_string_lossy();
        match enforce_floor(&target, &[root.as_ref()]) {
            FloorVerdict::Allowed => Ok(joined),
            other => Err(TurnSnapshotError::PathFloor {
                path: path.to_string(),
                verdict: other,
            }),
        }
    }

    fn step_dir(&self, work_id: &str, step_id: &str) -> PathBuf {
        self.root.join(work_id).join(step_id)
    }

    /// Capture the pre-mutation state of every file a mutating step is about to
    /// change, then record the set on the Work journal as **one** event.
    ///
    /// Ordering matters and is the whole point: every pre-image is written and
    /// `sync_all`ed to disk **before** the journal event is acknowledged, so a
    /// crash in the middle leaves either "no set recorded, nothing changed" or
    /// "a complete set recorded, nothing changed yet" — never a set that claims
    /// a pre-image the store does not have.
    pub fn capture(
        &self,
        gateway: &mut WorkGateway,
        request: CaptureRequest,
    ) -> Result<TurnChangeSet, TurnSnapshotError> {
        if request.files.is_empty() {
            return Err(TurnSnapshotError::Journal(
                "a change set must name at least one file".into(),
            ));
        }
        let mut seen: Vec<String> = Vec::new();
        for target in &request.files {
            let floored = self.floor(&target.path)?;
            let canonical = floored.to_string_lossy().to_string();
            if seen.contains(&canonical) {
                return Err(TurnSnapshotError::Journal(format!(
                    "change set names `{canonical}` twice"
                )));
            }
            seen.push(canonical);
        }

        let dir = self.step_dir(&request.work_id, &request.step_id);
        std::fs::create_dir_all(&dir).map_err(|source| TurnSnapshotError::WritePreImage {
            path: dir.display().to_string(),
            source,
        })?;

        let mut entries: Vec<FileSnapshotEntry> = Vec::with_capacity(request.files.len());
        for (index, target) in request.files.iter().enumerate() {
            let floored = self.floor(&target.path)?;
            let canonical = floored.to_string_lossy().to_string();
            let entry = match std::fs::symlink_metadata(&floored) {
                Ok(meta) if meta.is_dir() => {
                    return Err(TurnSnapshotError::NotAFile(canonical));
                }
                Ok(_) => {
                    let bytes =
                        std::fs::read(&floored).map_err(|source| TurnSnapshotError::Read {
                            path: canonical.clone(),
                            source,
                        })?;
                    let name = format!("{index:04}.blob");
                    let blob_path = dir.join(&name);
                    // Create + write + fsync: the pre-image must be durable
                    // before anything can mutate the original.
                    let mut file = std::fs::File::create(&blob_path).map_err(|source| {
                        TurnSnapshotError::WritePreImage {
                            path: blob_path.display().to_string(),
                            source,
                        }
                    })?;
                    file.write_all(&bytes)
                        .map_err(|source| TurnSnapshotError::WritePreImage {
                            path: blob_path.display().to_string(),
                            source,
                        })?;
                    file.sync_all()
                        .map_err(|source| TurnSnapshotError::WritePreImage {
                            path: blob_path.display().to_string(),
                            source,
                        })?;
                    FileSnapshotEntry {
                        path: canonical,
                        capture: FileCaptureKind::Bytes,
                        pre_digest: sha256_hex(&bytes),
                        pre_len: bytes.len() as u64,
                        blob: Some(name),
                        lease: target.lease.clone(),
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileSnapshotEntry {
                    // A creation. The recorded fact is "it did not exist", not
                    // a digest of nothing.
                    path: canonical,
                    capture: FileCaptureKind::AbsentMarker,
                    pre_digest: String::new(),
                    pre_len: 0,
                    blob: None,
                    lease: target.lease.clone(),
                },
                Err(e) => {
                    return Err(TurnSnapshotError::Read {
                        path: canonical,
                        source: e,
                    });
                }
            };
            entries.push(entry);
        }

        let change_set = TurnChangeSet {
            change_set_id: TurnChangeSet::id_for(&request.work_id, &request.step_id),
            work_id: request.work_id.clone(),
            step_id: request.step_id.clone(),
            session_id: request.session_id.as_ref().map(|s| s.as_str().to_string()),
            captured_at_ms: request.now_ms,
            entries,
            outcome: None,
            settled_at_ms: None,
            reason: None,
        };
        change_set.validate().map_err(TurnSnapshotError::Journal)?;
        gateway
            .record_file_change_set(&change_set)
            .map_err(TurnSnapshotError::Journal)?;
        Ok(change_set)
    }

    /// The stored pre-image for one entry, digest-checked against what the set
    /// recorded. A mismatch is [`TurnSnapshotError::PreImageUnusable`]: the
    /// restore cannot be verified, so it must not be attempted.
    pub fn pre_image(
        &self,
        change_set: &TurnChangeSet,
        entry: &FileSnapshotEntry,
    ) -> Result<Vec<u8>, TurnSnapshotError> {
        let name = entry
            .blob
            .as_deref()
            .ok_or_else(|| TurnSnapshotError::PreImageUnusable {
                path: entry.path.clone(),
            })?;
        let path = self
            .step_dir(&change_set.work_id, &change_set.step_id)
            .join(name);
        let bytes = std::fs::read(&path).map_err(|_| TurnSnapshotError::PreImageUnusable {
            path: entry.path.clone(),
        })?;
        if sha256_hex(&bytes) != entry.pre_digest {
            return Err(TurnSnapshotError::PreImageUnusable {
                path: entry.path.clone(),
            });
        }
        Ok(bytes)
    }

    /// Roll a captured change set back to its pre-mutation state, all-or-nothing.
    ///
    /// Three phases, in this order:
    ///
    /// 1. **pre-flight** — every restorable entry's stored pre-image is present
    ///    and digest-matches. If any is not, the filesystem is *not touched* and
    ///    the outcome is `uncertain`: an unverified rollback is worse than none.
    /// 2. **apply** — each entry is written back (or, for a creation, deleted).
    /// 3. **verify** — each file is re-read and its digest compared with the
    ///    captured pre-image (a creation must now be absent).
    ///
    /// The verdict is [`ChangeSetOutcome::RolledBackDueToFailure`] only when
    /// apply *and* verify both succeeded for every entry. Otherwise the set
    /// settles as `uncertain` and the receipt carries a `has_gap`.
    pub fn rollback(
        &self,
        gateway: &mut WorkGateway,
        work_id: &str,
        step_id: &str,
        reason: &str,
        now_ms: u64,
        audit: &mut dyn ChangeSetAudit,
    ) -> Result<RollbackOutcome, TurnSnapshotError> {
        let recorded = gateway
            .file_change_set(work_id, step_id)
            .cloned()
            .ok_or_else(|| TurnSnapshotError::SetNotRecorded {
                work_id: work_id.to_string(),
                step_id: step_id.to_string(),
            })?;
        if !recorded.is_open() {
            return Err(TurnSnapshotError::AlreadySettled(
                recorded.change_set_id.clone(),
            ));
        }

        // Phase 1 — pre-flight. Nothing is written until every pre-image is
        // known good.
        let mut pre_images: Vec<(FileSnapshotEntry, Option<Vec<u8>>)> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for entry in &recorded.entries {
            if entry.undo_deletes() {
                pre_images.push((entry.clone(), None));
                continue;
            }
            match self.pre_image(&recorded, entry) {
                Ok(bytes) => pre_images.push((entry.clone(), Some(bytes))),
                Err(_) => failed.push(entry.path.clone()),
            }
        }

        let mut restored: Vec<String> = Vec::new();
        let mut deleted: Vec<String> = Vec::new();
        let mut applied = Vec::new();

        if failed.is_empty() {
            // Phase 2 — apply.
            for (entry, bytes) in &pre_images {
                let target = self.floor(&entry.path)?;
                let result = match bytes {
                    Some(bytes) => crate::file_undo::restore_file_to_bytes(&target, Some(bytes)),
                    None => crate::file_undo::restore_file_to_bytes(&target, None),
                };
                match result {
                    Ok(()) => applied.push(entry.clone()),
                    Err(e) => {
                        failed.push(entry.path.clone());
                        let _ = e;
                    }
                }
            }
            // Phase 3 — verify. Re-read every applied file and compare.
            for entry in &applied {
                let target = self.floor(&entry.path)?;
                if entry.undo_deletes() {
                    if target.exists() {
                        failed.push(entry.path.clone());
                        continue;
                    }
                    deleted.push(entry.path.clone());
                    continue;
                }
                match std::fs::read(&target) {
                    Ok(bytes) if sha256_hex(&bytes) == entry.pre_digest => {
                        restored.push(entry.path.clone());
                    }
                    _ => failed.push(entry.path.clone()),
                }
            }
        }

        let outcome = if failed.is_empty() {
            ChangeSetOutcome::RolledBackDueToFailure
        } else {
            ChangeSetOutcome::Uncertain
        };

        let mut settled = recorded.clone();
        settled.outcome = Some(outcome);
        settled.settled_at_ms = Some(now_ms);
        if let Err(e) = gateway.settle_file_change_set(&settled, Some(reason)) {
            // The journal refused the verdict. That is a real failure, not a
            // cosmetic one: the world moved and the record did not.
            return Err(TurnSnapshotError::Journal(e));
        }

        let outcome_receipt = self.build_receipt(&settled, reason, &failed);
        let event = AuditEvent::new(
            "work.file_change_set.rolled_back",
            serde_json::json!({
                "outcome": outcome.as_str(),
                "changeSetId": settled.change_set_id,
                "workId": settled.work_id,
                "stepId": settled.step_id,
                "files": settled.entries.len(),
                "restored": restored.len(),
                "deleted": deleted.len(),
                "failed": failed,
                "leaseCovered": settled
                    .entries
                    .iter()
                    .filter(|e| e.lease.is_some())
                    .count(),
                "leaseNone": settled
                    .entries
                    .iter()
                    .filter(|e| e.lease.is_none())
                    .count(),
                "atMs": now_ms,
            }),
        );
        audit.record(event.clone());

        if outcome == ChangeSetOutcome::Uncertain {
            return Err(TurnSnapshotError::RestoreUnverified {
                change_set_id: settled.change_set_id.clone(),
                failed,
            });
        }
        Ok(RollbackOutcome {
            change_set_id: settled.change_set_id,
            outcome,
            restored,
            deleted,
            failed,
            receipt: outcome_receipt,
            audit_event: event,
        })
    }

    fn build_receipt(
        &self,
        settled: &TurnChangeSet,
        reason: &str,
        failed: &[String],
    ) -> EffectReceipt {
        let receipt = EffectReceipt::new(
            format!("fcs-receipt:{}", settled.change_set_id),
            "turn.change_set.rollback",
            "n/a",
            &settled
                .entries
                .iter()
                .map(|e| e.pre_digest.as_str())
                .collect::<Vec<_>>()
                .join(","),
            format!(
                "restore {} file(s) to their pre-mutation state",
                settled.entries.len()
            ),
            format!(
                "restore {} file(s) to their pre-mutation state",
                settled.entries.len()
            ),
        )
        .with_resource(settled.change_set_id.clone())
        .with_rollback(settled.change_set_id.clone());
        if failed.is_empty() {
            receipt
        } else {
            // Honesty invariant: a rollback that could not be verified is a
            // receipt with a gap, so no reader mistakes it for a completed
            // restore.
            receipt.gap(format!(
                "restore unverified for {failed:?} (reason: {reason})"
            ))
        }
    }

    /// Drop a settled set's stored pre-images. Only ever called for a set whose
    /// verdict is already on the journal — the store refuses to discard the only
    /// copy of a pre-image while the set is still open.
    pub fn prune(
        &self,
        gateway: &WorkGateway,
        work_id: &str,
        step_id: &str,
    ) -> Result<usize, TurnSnapshotError> {
        let recorded = gateway.file_change_set(work_id, step_id).ok_or_else(|| {
            TurnSnapshotError::SetNotRecorded {
                work_id: work_id.to_string(),
                step_id: step_id.to_string(),
            }
        })?;
        if recorded.is_open() {
            return Err(TurnSnapshotError::Journal(format!(
                "change set `{}` is still open; refusing to drop its pre-images",
                recorded.change_set_id
            )));
        }
        let dir = self.step_dir(work_id, step_id);
        let mut removed = 0usize;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if std::fs::remove_file(entry.path()).is_ok() {
                    removed += 1;
                }
            }
        }
        let _ = std::fs::remove_dir(&dir);
        Ok(removed)
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentcowork_types::workbench::{LeaseState, ResourceKind, TypedResourceKey};
    use agentcowork_types::{LeaseId, SessionId, SessionKind};

    struct Recorder(Vec<AuditEvent>);

    impl ChangeSetAudit for Recorder {
        fn record(&mut self, event: AuditEvent) {
            self.0.push(event);
        }
    }

    struct Fixture {
        _base: PathBuf,
        work: PathBuf,
        store: TurnSnapshotStore,
        gateway: WorkGateway,
        audit: Recorder,
    }

    fn fixture(name: &str) -> Fixture {
        let base = std::env::temp_dir().join(format!(
            "ea-turn-snap-{}-{}-{}",
            name,
            std::process::id(),
            name.len()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let work = base.join("workspace");
        std::fs::create_dir_all(&work).unwrap();
        let mut gateway = WorkGateway::new();
        gateway
            .create_work_in_session(
                "w-1",
                None,
                Some("s-1".into()),
                SessionKind::Interactive,
                "refactor",
            )
            .unwrap();
        Fixture {
            store: TurnSnapshotStore::new(&base.join("data"), &work),
            work,
            gateway,
            audit: Recorder(Vec::new()),
            _base: base,
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self._base);
        }
    }

    fn write(f: &Fixture, rel: &str, body: &str) -> PathBuf {
        let p = f.work.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    }

    fn lease() -> LeaseAttachment {
        LeaseAttachment {
            lease_id: LeaseId::new("lease-1"),
            session_scope: SessionId::new("s-1"),
            resource_key: TypedResourceKey::new(ResourceKind::File, "a.rs", 3).canonical_key(),
            generation: 3,
            fence: 1,
            state: LeaseState::Active,
        }
    }

    fn request(files: Vec<CaptureTarget>) -> CaptureRequest {
        CaptureRequest {
            work_id: "w-1".into(),
            step_id: "step-1".into(),
            session_id: Some(SessionId::new("s-1")),
            files,
            now_ms: 1_000,
        }
    }

    #[test]
    fn capture_records_pre_mutation_identity_without_ever_carrying_bytes() {
        let mut f = fixture("capture");
        let a = write(&f, "src/a.rs", "original a\n");
        let b = write(&f, "src/b.rs", "original b\n");
        let set = f
            .store
            .capture(
                &mut f.gateway,
                request(vec![
                    CaptureTarget::new("src/a.rs").under_lease(lease()),
                    CaptureTarget::new(b.to_string_lossy().to_string()),
                ]),
            )
            .unwrap();
        assert!(set.validate().is_ok());
        assert_eq!(set.change_set_id, "fcs:w-1/step-1");
        assert_eq!(set.entries.len(), 2);
        // Pre-mutation digests, not content.
        for entry in &set.entries {
            assert_eq!(entry.pre_digest.len(), 64);
            assert!(entry.is_restorable());
            assert!(!serde_json::to_string(entry).unwrap().contains("original"));
        }
        // The lease that covered the write is recorded; the unleased one says so.
        assert_eq!(set.entries[0].lease_label(), "lease-1");
        assert_eq!(set.entries[1].lease_label(), "none");
        // The bytes are on disk in the kernel store, not in the set.
        let blob = f
            .store
            .root()
            .join("w-1")
            .join("step-1")
            .join(set.entries[0].blob.as_deref().unwrap());
        assert_eq!(std::fs::read(blob).unwrap(), b"original a\n");
        // And the set is on the Work journal as ONE event.
        assert_eq!(f.gateway.file_change_sets_for("w-1").len(), 1);
        assert!(f.gateway.open_file_change_sets("w-1").len() == 1);
        let _ = (a, b);
    }

    #[test]
    fn a_creation_is_captured_as_absent_and_undo_deletes_it() {
        let mut f = fixture("creation");
        let set = f
            .store
            .capture(
                &mut f.gateway,
                request(vec![CaptureTarget::new("src/new.rs")]),
            )
            .unwrap();
        assert_eq!(set.entries[0].capture, FileCaptureKind::AbsentMarker);
        assert!(set.entries[0].blob.is_none());
        assert!(set.entries[0].pre_digest.is_empty());
        // The step then creates the file.
        write(&f, "src/new.rs", "created by the step\n");
        let out = f
            .store
            .rollback(
                &mut f.gateway,
                "w-1",
                "step-1",
                "step 3 failed",
                2_000,
                &mut f.audit,
            )
            .unwrap();
        assert!(out.is_fully_rolled_back());
        // The recorded path is the floored absolute one, never the caller's
        // relative spelling: that is what the path floor decided to act on.
        assert_eq!(out.deleted.len(), 1);
        assert!(out.deleted[0].ends_with("src/new.rs"));
        assert!(!f.work.join("src/new.rs").exists());
    }

    #[test]
    fn a_partial_failure_rolls_everything_back_and_reports_the_documented_verdict() {
        let mut f = fixture("rollback");
        write(&f, "a.rs", "a before\n");
        write(&f, "b.rs", "b before\n");
        f.store
            .capture(
                &mut f.gateway,
                request(vec![CaptureTarget::new("a.rs"), CaptureTarget::new("b.rs")]),
            )
            .unwrap();
        // Two writes land, then the step fails.
        write(&f, "a.rs", "a after\n");
        write(&f, "b.rs", "b after\n");

        let out = f
            .store
            .rollback(
                &mut f.gateway,
                "w-1",
                "step-1",
                "step 2 failed: syntax error",
                2_000,
                &mut f.audit,
            )
            .unwrap();
        assert!(out.is_fully_rolled_back());
        assert_eq!(out.outcome, ChangeSetOutcome::RolledBackDueToFailure);
        assert_eq!(out.failed.len(), 0);
        assert_eq!(
            std::fs::read_to_string(f.work.join("a.rs")).unwrap(),
            "a before\n"
        );
        assert_eq!(
            std::fs::read_to_string(f.work.join("b.rs")).unwrap(),
            "b before\n"
        );
        // The verdict is on the journal with the documented spelling.
        let settled = f.gateway.file_change_set("w-1", "step-1").unwrap();
        assert_eq!(
            settled.outcome,
            Some(ChangeSetOutcome::RolledBackDueToFailure)
        );
        assert_eq!(
            settled.reason.as_deref(),
            Some("step 2 failed: syntax error")
        );
        assert!(f.gateway.open_file_change_sets("w-1").is_empty());
        // The audit row carries the verdict, and the receipt names the rollback.
        assert_eq!(f.audit.0.len(), 1);
        assert_eq!(f.audit.0[0].kind, "work.file_change_set.rolled_back");
        assert_eq!(
            f.audit.0[0].payload["outcome"],
            serde_json::json!("RolledBackDueToFailure")
        );
        assert_eq!(f.audit.0[0].payload["leaseCovered"], 0);
        assert_eq!(f.audit.0[0].payload["leaseNone"], 2);
        assert_eq!(out.receipt.rollback_ref.as_deref(), Some("fcs:w-1/step-1"));
        assert!(!out.receipt.has_gap);
    }

    #[test]
    fn an_unusable_pre_image_never_touches_the_filesystem_and_settles_uncertain() {
        let mut f = fixture("unusable");
        write(&f, "a.rs", "a before\n");
        let set = f
            .store
            .capture(&mut f.gateway, request(vec![CaptureTarget::new("a.rs")]))
            .unwrap();
        write(&f, "a.rs", "a after\n");
        // The pre-image is lost (a wiped store, a truncated disk).
        let blob = f
            .store
            .root()
            .join("w-1")
            .join("step-1")
            .join(set.entries[0].blob.as_deref().unwrap());
        std::fs::remove_file(&blob).unwrap();

        let err = f
            .store
            .rollback(
                &mut f.gateway,
                "w-1",
                "step-1",
                "step 2 failed",
                2_000,
                &mut f.audit,
            )
            .unwrap_err();
        assert!(matches!(err, TurnSnapshotError::RestoreUnverified { .. }));
        // The mutated file was left alone — the store refuses a partial promise.
        assert_eq!(
            std::fs::read_to_string(f.work.join("a.rs")).unwrap(),
            "a after\n"
        );
        // And the recorded verdict is `uncertain`, never `RolledBackDueToFailure`.
        let settled = f.gateway.file_change_set("w-1", "step-1").unwrap();
        assert_eq!(settled.outcome, Some(ChangeSetOutcome::Uncertain));
        assert_eq!(
            f.audit.0[0].payload["outcome"],
            serde_json::json!("uncertain")
        );
    }

    #[test]
    fn a_tampered_pre_image_is_detected_rather_than_restored() {
        let mut f = fixture("tampered");
        write(&f, "a.rs", "a before\n");
        let set = f
            .store
            .capture(&mut f.gateway, request(vec![CaptureTarget::new("a.rs")]))
            .unwrap();
        write(&f, "a.rs", "a after\n");
        let blob = f
            .store
            .root()
            .join("w-1")
            .join("step-1")
            .join(set.entries[0].blob.as_deref().unwrap());
        std::fs::write(&blob, b"a DIFFERENT before\n").unwrap();
        let err = f
            .store
            .rollback(
                &mut f.gateway,
                "w-1",
                "step-1",
                "step 2 failed",
                2_000,
                &mut f.audit,
            )
            .unwrap_err();
        assert!(matches!(err, TurnSnapshotError::RestoreUnverified { .. }));
        assert_eq!(
            std::fs::read_to_string(f.work.join("a.rs")).unwrap(),
            "a after\n"
        );
    }

    #[test]
    fn a_capture_outside_the_workspace_floor_is_refused_before_any_byte_is_read() {
        let mut f = fixture("floor");
        let outside = std::env::temp_dir().join("ea-turn-snap-outside.txt");
        std::fs::write(&outside, b"secret\n").unwrap();
        let err = f
            .store
            .capture(
                &mut f.gateway,
                request(vec![CaptureTarget::new(
                    outside.to_string_lossy().to_string(),
                )]),
            )
            .unwrap_err();
        assert!(
            matches!(err, TurnSnapshotError::PathFloor { .. }),
            "unexpected error: {err}"
        );
        assert!(f.gateway.file_change_sets_for("w-1").is_empty());
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn a_relative_escape_is_refused_by_the_floor() {
        let mut f = fixture("escape");
        let err = f
            .store
            .capture(
                &mut f.gateway,
                request(vec![CaptureTarget::new("../../etc/passwd")]),
            )
            .unwrap_err();
        assert!(matches!(err, TurnSnapshotError::PathFloor { .. }));
    }

    #[test]
    fn a_settled_set_cannot_be_rolled_back_or_rewritten() {
        let mut f = fixture("settled");
        write(&f, "a.rs", "a before\n");
        f.store
            .capture(&mut f.gateway, request(vec![CaptureTarget::new("a.rs")]))
            .unwrap();
        write(&f, "a.rs", "a after\n");
        f.store
            .rollback(
                &mut f.gateway,
                "w-1",
                "step-1",
                "first",
                2_000,
                &mut f.audit,
            )
            .unwrap();
        write(&f, "a.rs", "a mutated again\n");
        let err = f
            .store
            .rollback(
                &mut f.gateway,
                "w-1",
                "step-1",
                "second",
                3_000,
                &mut f.audit,
            )
            .unwrap_err();
        assert!(matches!(err, TurnSnapshotError::AlreadySettled(_)));
        // The second attempt changed nothing and wrote no second audit row.
        assert_eq!(f.audit.0.len(), 1);
        assert_eq!(
            std::fs::read_to_string(f.work.join("a.rs")).unwrap(),
            "a mutated again\n"
        );
    }

    #[test]
    fn an_unrecorded_set_has_nothing_to_roll_back() {
        let mut f = fixture("unrecorded");
        let err = f
            .store
            .rollback(
                &mut f.gateway,
                "w-1",
                "step-9",
                "no set",
                1_000,
                &mut f.audit,
            )
            .unwrap_err();
        assert!(matches!(err, TurnSnapshotError::SetNotRecorded { .. }));
        assert!(f.audit.0.is_empty());
    }

    #[test]
    fn a_captured_set_survives_reopening_the_journal() {
        let dir = std::env::temp_dir().join(format!("ea-turn-snap-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let journal = dir.join("events.jsonl");
        let work = dir.join("workspace");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("a.rs"), b"a before\n").unwrap();

        {
            let mut gateway = WorkGateway::open(&journal).unwrap();
            gateway
                .create_work_in_session(
                    "w-1",
                    None,
                    Some("s-1".into()),
                    SessionKind::Interactive,
                    "refactor",
                )
                .unwrap();
            let store = TurnSnapshotStore::new(&dir, &work);
            store
                .capture(
                    &mut gateway,
                    CaptureRequest {
                        work_id: "w-1".into(),
                        step_id: "step-1".into(),
                        session_id: Some(SessionId::new("s-1")),
                        files: vec![CaptureTarget::new("a.rs")],
                        now_ms: 1_000,
                    },
                )
                .unwrap();
            // The step mutates and the process dies before any rollback.
            std::fs::write(work.join("a.rs"), b"a half-applied\n").unwrap();
        }

        // A restart rebuilds the open set from the journal alone: the crash
        // window is recoverable, not lost.
        let mut reopened = WorkGateway::open(&journal).unwrap();
        let open = reopened.open_file_change_sets("w-1");
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].change_set_id, "fcs:w-1/step-1");
        let store = TurnSnapshotStore::new(&dir, &work);
        let out = store
            .rollback(
                &mut reopened,
                "w-1",
                "step-1",
                "process died mid-turn",
                9_000,
                &mut Recorder(Vec::new()),
            )
            .unwrap();
        assert!(out.is_fully_rolled_back());
        assert_eq!(
            std::fs::read_to_string(work.join("a.rs")).unwrap(),
            "a before\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_drops_pre_images_only_for_a_settled_set() {
        let mut f = fixture("prune");
        write(&f, "a.rs", "a before\n");
        f.store
            .capture(&mut f.gateway, request(vec![CaptureTarget::new("a.rs")]))
            .unwrap();
        let err = f.store.prune(&f.gateway, "w-1", "step-1").unwrap_err();
        assert!(matches!(err, TurnSnapshotError::Journal(_)));
        f.store
            .rollback(
                &mut f.gateway,
                "w-1",
                "step-1",
                "settled",
                2_000,
                &mut f.audit,
            )
            .unwrap();
        assert_eq!(f.store.prune(&f.gateway, "w-1", "step-1").unwrap(), 1);
    }

    #[test]
    fn a_directory_target_is_refused_rather_than_captured_as_bytes() {
        let mut f = fixture("dir");
        std::fs::create_dir_all(f.work.join("src")).unwrap();
        let err = f
            .store
            .capture(&mut f.gateway, request(vec![CaptureTarget::new("src")]))
            .unwrap_err();
        assert!(matches!(err, TurnSnapshotError::NotAFile(_)));
        assert!(f.gateway.file_change_sets_for("w-1").is_empty());
    }
}
