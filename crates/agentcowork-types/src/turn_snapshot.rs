//! P69.G2 — turn-atomic multi-file change sets (`ARCH/RECOVERY.md` §10).
//!
//! This module owns the **shape** of one turn-scoped change set: the set of
//! files a mutating step was about to change, the pre-mutation identity of each
//! (digest + how the pre-image was captured), the `ResourceLease` that covered
//! it (or `"none"`), and the outcome verdict.
//!
//! What it deliberately does **not** own:
//!
//! * **bytes.** A [`FileSnapshotEntry`] carries a digest and, at most, a
//!   reference to a stored pre-image. Pre-mutation bytes are captured and
//!   restored by the kernel owner (`agentcowork-core::turn_snapshot`) and are
//!   never carried across a sidecar or IPC boundary — a snapshot type that
//!   could hold file contents would make that accidental.
//! * **durability.** The set is recorded as one `file_change_set` on the
//!   existing Work event journal; the file store is registered in the existing
//!   durable-store registry. There is no second event log here.
//! * **the outcome vocabulary beyond honesty.** [`ChangeSetOutcome::RolledBackDueToFailure`]
//!   is the `RECOVERY.md` §10 verdict spelling; a partial or unverified restore
//!   is [`ChangeSetOutcome::Uncertain`], never `rolled_back`.
//!
//! Privacy (same rule as [`crate::workbench`]): no field here can hold bearer,
//! token, cookie, credential, or any other possession material. The lease a
//! file was captured under is a [`LeaseAttachment`] — id, generation, fence,
//! state — and [`FORBIDDEN_PROJECTION_KEYS`] proves the rendered JSON carries
//! nothing else.

use serde::{Deserialize, Serialize};

use crate::workbench::{
    FORBIDDEN_PROJECTION_KEYS, LeaseAttachment, projection_json_has_no_secrets,
};

/// The `RECOVERY.md` §10 rollback verdict, spelled exactly as the contract
/// names it. It appears verbatim in the audit row's `outcome` so an audit
/// reader (and a grep) finds the same word the document uses.
pub const ROLLED_BACK_DUE_TO_FAILURE: &str = "RolledBackDueToFailure";

/// How the pre-mutation identity of a captured file was obtained.
///
/// Both arms are durable and restorable; they differ only in whether a
/// pre-image copy was stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileCaptureKind {
    /// The pre-image bytes were copied into the kernel snapshot store. Undo
    /// restores them.
    Bytes,
    /// The file did not exist before the mutation: a `path` + digest-of-absent
    /// marker. Undo deletes the created file.
    AbsentMarker,
}

impl FileCaptureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bytes => "bytes",
            Self::AbsentMarker => "absent_marker",
        }
    }

    /// Strict parse — an unknown spelling is `None`, never a guess.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "bytes" => Some(Self::Bytes),
            "absent_marker" => Some(Self::AbsentMarker),
            _ => None,
        }
    }
}

/// The terminal verdict of a change set.
///
/// `RolledBackDueToFailure` is only reachable when **every** captured file was
/// restored and the restore was verified. Anything else that moved the
/// filesystem is `Uncertain` — never success, never failure by inference
/// (`RECOVERY.md` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSetOutcome {
    /// Captured; no rollback attempted yet. This is the crash-window state.
    Open,
    /// Every captured file was restored to its captured pre-mutation state.
    RolledBackDueToFailure,
    /// Some files were restored and some were not (or a restore could not be
    /// verified). The world is partially restored and **unknown**: reconcile
    /// before retrying anything.
    Uncertain,
}

impl ChangeSetOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::RolledBackDueToFailure => ROLLED_BACK_DUE_TO_FAILURE,
            Self::Uncertain => "uncertain",
        }
    }

    /// Strict parse. The documented `RolledBackDueToFailure` spelling is
    /// accepted along with its snake_case form; anything else is `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            ROLLED_BACK_DUE_TO_FAILURE | "rolled_back_due_to_failure" | "rolled_back" => {
                Some(Self::RolledBackDueToFailure)
            }
            "uncertain" | "unknown" => Some(Self::Uncertain),
            _ => None,
        }
    }

    /// Whether the filesystem is provably back at the pre-mutation state.
    /// Only this state may be reported as a completed rollback.
    pub fn is_fully_restored(self) -> bool {
        matches!(self, Self::RolledBackDueToFailure)
    }
}

/// One file inside a change set: what it was before the mutation, and which
/// lease (if any) covered the write.
///
/// There is no bytes field. `blob` is a store-relative *reference* to the
/// pre-image the kernel already wrote durably; it is not content, and it is
/// only meaningful to the kernel snapshot store that wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSnapshotEntry {
    /// Canonical, pathfloor-checked path the write was aimed at.
    pub path: String,
    pub capture: FileCaptureKind,
    /// SHA-256 of the pre-mutation bytes, or the empty string for a file that
    /// did not exist (an absent file has no content digest; `capture` records
    /// that fact instead of inventing one).
    pub pre_digest: String,
    /// Byte length of the pre-image. `0` for a creation.
    pub pre_len: u64,
    /// Store-relative reference to the captured pre-image. `None` for a
    /// creation (nothing to restore — undo deletes the file instead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    /// The `ResourceLease` (ADR-0008) that covered this file when it was
    /// captured, or `None` when the write was **not** under a lease. The
    /// absence is a first-class recorded fact, not a gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseAttachment>,
}

impl FileSnapshotEntry {
    /// The recorded lease label: the lease id, or the literal `"none"`.
    ///
    /// The contract requires the *fact* of no coverage to be readable, not just
    /// inferable from an absent field, so the label is total.
    pub fn lease_label(&self) -> &str {
        match &self.lease {
            Some(lease) => lease.lease_id.as_str(),
            None => "none",
        }
    }

    /// Whether the entry holds a stored pre-image (as opposed to a creation
    /// marker, whose undo is a delete).
    pub fn is_restorable(&self) -> bool {
        matches!(self.capture, FileCaptureKind::Bytes) && self.blob.is_some()
    }

    /// Whether undo can return this file to its captured state at all. Both
    /// capture kinds can: a pre-image is written back, a creation is deleted.
    /// A `Bytes` entry with no stored pre-image cannot — and
    /// [`Self::validate`] refuses to build one.
    pub fn undo_handles(&self) -> bool {
        match self.capture {
            FileCaptureKind::Bytes => self.blob.is_some(),
            FileCaptureKind::AbsentMarker => true,
        }
    }

    /// The work this file belongs to is a creation when undo must *delete* it.
    pub fn undo_deletes(&self) -> bool {
        matches!(self.capture, FileCaptureKind::AbsentMarker)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.path.trim().is_empty() {
            return Err("change-set entry requires a path".into());
        }
        if matches!(self.capture, FileCaptureKind::Bytes) {
            if self.pre_digest.is_empty() {
                return Err("a bytes capture requires a pre-mutation digest".into());
            }
            if self.blob.is_none() {
                return Err("a bytes capture requires a stored pre-image reference".into());
            }
        }
        if self.undo_deletes() && !self.pre_digest.is_empty() {
            return Err("an absent-marker capture must not carry a content digest".into());
        }
        Ok(())
    }
}

/// One turn-scoped, atomic change set, keyed by `(WorkId, StepId)`.
///
/// Recorded as a **single** event on the existing Work journal, so a crash
/// mid-turn leaves exactly one durable record naming every file that was about
/// to change and what it looked like before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnChangeSet {
    /// Deterministic id `fcs:<work>/<step>` — re-derivable, so a replay after a
    /// crash names the same set without a lookup table.
    pub change_set_id: String,
    pub work_id: String,
    pub step_id: String,
    /// Canonical Session scope when the turn had one. Lookup/audit only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub captured_at_ms: u64,
    pub entries: Vec<FileSnapshotEntry>,
    /// `None` until a rollback attempt settles the set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ChangeSetOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at_ms: Option<u64>,
    /// Why the set was rolled back (the failing step's reason), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl TurnChangeSet {
    /// The deterministic id for one `(work, step)` pair.
    pub fn id_for(work_id: &str, step_id: &str) -> String {
        format!("fcs:{work_id}/{step_id}")
    }

    pub fn is_open(&self) -> bool {
        self.outcome.is_none()
    }

    /// The entries undo cannot return to their captured state. A validated set
    /// has none; a set whose stored pre-image went missing has one, and that is
    /// exactly the case that must settle as `uncertain`.
    pub fn unrestored(&self) -> Vec<&FileSnapshotEntry> {
        self.entries.iter().filter(|e| !e.undo_handles()).collect()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.work_id.trim().is_empty() {
            return Err("change set requires a work id".into());
        }
        if self.step_id.trim().is_empty() {
            return Err("change set requires a step id".into());
        }
        if self.change_set_id != Self::id_for(&self.work_id, &self.step_id) {
            return Err(format!(
                "change set id `{}` does not match the deterministic id for ({}, {})",
                self.change_set_id, self.work_id, self.step_id
            ));
        }
        if self.entries.is_empty() {
            return Err("a change set must name at least one file".into());
        }
        let mut seen: Vec<&str> = Vec::new();
        for entry in &self.entries {
            entry.validate()?;
            if seen.contains(&entry.path.as_str()) {
                return Err(format!(
                    "change set names `{}` twice — one file may appear once per set",
                    entry.path
                ));
            }
            seen.push(entry.path.as_str());
        }
        if let Some(outcome) = self.outcome {
            if matches!(outcome, ChangeSetOutcome::RolledBackDueToFailure)
                && !self.entries.iter().all(|e| e.undo_handles())
            {
                return Err(
                    "a fully-rolled-back change set cannot contain an entry undo cannot handle"
                        .into(),
                );
            }
            if matches!(outcome, ChangeSetOutcome::Open) {
                return Err(
                    "`open` is not a settling outcome; leave `outcome` empty instead".into(),
                );
            }
        }
        Ok(())
    }

    /// Rendered form checked against the shared secret-key tripwire. Called by
    /// every surface that serializes a change set (event payload, audit row,
    /// IPC projection).
    pub fn is_secret_free(&self) -> bool {
        if let Some(reason) = &self.reason {
            let lower = reason.to_ascii_lowercase();
            if FORBIDDEN_PROJECTION_KEYS
                .iter()
                .any(|key| lower.contains(key))
            {
                return false;
            }
        }
        projection_json_has_no_secrets(&serde_json::to_string(self).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LeaseId;
    use crate::SessionId;
    use crate::workbench::{LeaseState, ResourceKind, TypedResourceKey};

    fn lease(id: &str) -> LeaseAttachment {
        LeaseAttachment {
            lease_id: LeaseId::new(id),
            session_scope: SessionId::new("s-1"),
            resource_key: TypedResourceKey::new(ResourceKind::File, "/work/a.rs", 7)
                .canonical_key(),
            generation: 7,
            fence: 3,
            state: LeaseState::Active,
        }
    }

    fn entry(path: &str) -> FileSnapshotEntry {
        FileSnapshotEntry {
            path: path.into(),
            capture: FileCaptureKind::Bytes,
            pre_digest: "ab".repeat(32),
            pre_len: 12,
            blob: Some(format!("{}.bin", path.replace('/', "_"))),
            lease: Some(lease("lease-1")),
        }
    }

    fn set(entries: Vec<FileSnapshotEntry>) -> TurnChangeSet {
        TurnChangeSet {
            change_set_id: TurnChangeSet::id_for("w-1", "step-1"),
            work_id: "w-1".into(),
            step_id: "step-1".into(),
            session_id: Some("s-1".into()),
            captured_at_ms: 100,
            entries,
            outcome: None,
            settled_at_ms: None,
            reason: None,
        }
    }

    #[test]
    fn ids_are_deterministic_and_must_match_their_key() {
        assert_eq!(TurnChangeSet::id_for("w-1", "step-1"), "fcs:w-1/step-1");
        let mut s = set(vec![entry("/work/a.rs")]);
        assert!(s.validate().is_ok());
        s.step_id = "step-2".into();
        assert!(s.validate().unwrap_err().contains("deterministic id"));
    }

    #[test]
    fn a_creation_is_an_absent_marker_not_a_fake_digest() {
        let s = set(vec![FileSnapshotEntry {
            path: "/work/new.rs".into(),
            capture: FileCaptureKind::AbsentMarker,
            pre_digest: String::new(),
            pre_len: 0,
            blob: None,
            lease: None,
        }]);
        assert!(s.validate().is_ok());
        assert_eq!(s.entries[0].lease_label(), "none");
        assert!(s.entries[0].undo_deletes());
        assert!(!s.entries[0].is_restorable());
        // A creation is still fully undoable: undo deletes the created file.
        assert!(s.entries[0].undo_handles());

        let mut lying = s.clone();
        lying.entries[0].pre_digest = "deadbeef".into();
        assert!(lying.validate().is_err());
    }

    #[test]
    fn the_captured_lease_is_recorded_and_its_absence_is_readable() {
        let s = set(vec![entry("/work/a.rs")]);
        assert_eq!(s.entries[0].lease_label(), "lease-1");
        assert_eq!(s.entries[0].lease.as_ref().unwrap().fence, 3);
        let mut mutator = s.clone();
        mutator.entries[0].lease = None;
        assert_eq!(mutator.entries[0].lease_label(), "none");
        assert!(mutator.validate().is_ok());
    }

    #[test]
    fn only_a_fully_restorable_set_may_claim_the_documented_rollback_verdict() {
        let mut s = set(vec![entry("/work/a.rs")]);
        s.outcome = Some(ChangeSetOutcome::RolledBackDueToFailure);
        s.settled_at_ms = Some(200);
        assert!(s.validate().is_ok());
        assert_eq!(
            s.outcome.unwrap().as_str(),
            "RolledBackDueToFailure",
            "the audit verdict must use the documented spelling"
        );
        assert!(ChangeSetOutcome::parse("RolledBackDueToFailure").is_some());
        assert!(ChangeSetOutcome::parse("definitely_done").is_none());

        // A creation in the set is still a full restore: undo deletes it.
        s.entries.push(FileSnapshotEntry {
            path: "/work/new.rs".into(),
            capture: FileCaptureKind::AbsentMarker,
            pre_digest: String::new(),
            pre_len: 0,
            blob: None,
            lease: None,
        });
        assert!(s.validate().is_ok());
        assert!(s.unrestored().is_empty());

        // What *is* refused: a `bytes` capture with no stored pre-image. Undo
        // could not return that file, so such a set is not constructible at
        // all — the store detects the on-disk case (a pre-image deleted after
        // capture) separately and settles `uncertain` there.
        let mut unverifiable = set(vec![entry("/work/a.rs")]);
        unverifiable.entries[0].blob = None;
        assert!(unverifiable.validate().is_err());
        assert!(set(vec![entry("/work/a.rs")]).unrestored().is_empty());

        // A set is never left half-settled: `open` is the absence of a verdict,
        // not one.
        let mut half = set(vec![entry("/work/a.rs")]);
        half.outcome = Some(ChangeSetOutcome::Open);
        assert!(half.validate().is_err());
    }

    #[test]
    fn a_change_set_names_each_file_once_and_carries_no_secrets() {
        assert!(
            set(vec![entry("/work/a.rs"), entry("/work/b.rs")])
                .validate()
                .is_ok()
        );
        assert!(
            set(vec![entry("/work/a.rs"), entry("/work/a.rs")])
                .validate()
                .is_err()
        );

        let mut s = set(vec![entry("/work/a.rs")]);
        assert!(s.is_secret_free());
        s.reason = Some("step 3 failed: bearer=deadbeef".into());
        assert!(!s.is_secret_free());
    }
}
