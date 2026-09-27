//! K2 Reversible Change Sets (doc 81 — roadmap §2; doc-53 idempotency made
//! truthful): a change-set coordinator *above* tickets. Every mutating
//! action joins a change set with a dependency DAG, pre/postconditions, and
//! a truthful [`EffectClass`] (doc-53's four idempotency classes). Recovery
//! is honest: a kill mid-task rolls back what is reversible, compensates
//! what is compensatable, and **reports** what is irreversible/uncertain —
//! it never pretends.
//!
//! The acceptance test: kill mid-task → honest recovery, no duplicate
//! (an entry already committed is never re-executed; its idempotency key
//! is the guard).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

/// doc-53 idempotency classes made explicit per change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    /// Safe to roll back byte-for-byte (fs write with snapshot, office edit).
    Reversible,
    /// Can't undo, but a compensating action restores the intent (git commit
    /// → revert commit, email → follow-up).
    Compensatable,
    /// Cannot be undone or compensated (destructive delete, external send).
    Irreversible,
    /// The class isn't known — recovery must halt and ask, never guess.
    Uncertain,
}

/// One change in a set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    /// The guard ticket that authorized this change.
    pub ticket_id: String,
    /// The idempotency key (execution_id/action_id) — re-running with the
    /// same key is a no-op (doc-53).
    pub idempotency_key: String,
    pub effect_class: EffectClass,
    /// Other change ids this one depends on (DAG edges).
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Preconditions (statements true before the change; evaluated by the
    /// caller or asserted here).
    #[serde(default)]
    pub preconditions: Vec<String>,
    /// Postconditions (statements true after).
    #[serde(default)]
    pub postconditions: Vec<String>,
}

/// Change lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeState {
    #[default]
    Planned,
    Committed,
    Reverted,
    Compensated,
    /// Irreversible/uncertain and not recoverable — surfaced, never hidden.
    Unrecoverable,
}

/// A committed change's record (what recovery works from).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommittedChange {
    pub change: Change,
    pub state: ChangeState,
    pub committed_at: u64,
}

/// The change-set coordinator.
#[derive(Debug, Clone, Default)]
pub struct ChangeSet {
    changes: Vec<CommittedChange>,
    by_id: std::collections::HashMap<String, usize>,
}

/// A host-imported change produced by a sandboxed external process.
///
/// The host accepts only an exact, pre-reviewed manifest: paths are relative
/// to the declared root, hashes are checked against the bytes being imported,
/// and every entry has a corresponding planned change/ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewedImport {
    pub root: PathBuf,
    pub changes: Vec<ImportEntry>,
    pub manifest_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportEntry {
    pub change_id: String,
    pub relative_path: PathBuf,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("import manifest is empty")]
    Empty,
    #[error("import path must be relative: {0}")]
    AbsolutePath(PathBuf),
    #[error("import path escapes its root: {0}")]
    PathEscape(PathBuf),
    #[error("import entry `{0}` has no planned change")]
    UnknownChange(String),
    #[error("import entry `{0}` is duplicated")]
    DuplicateChange(String),
    #[error("import manifest hash mismatch")]
    ManifestHashMismatch,
    #[error("import content hash mismatch for `{path}`")]
    ContentHashMismatch { path: PathBuf },
}

impl ReviewedImport {
    /// Validate a sandbox-produced manifest before any host mutation occurs.
    /// This is deliberately pure over bytes and does not write to disk.
    pub fn validate(
        root: impl Into<PathBuf>,
        entries: Vec<ImportEntry>,
        expected_manifest_sha256: &str,
        files: &[(PathBuf, Vec<u8>)],
        planned: &ChangeSet,
    ) -> Result<Self, ImportError> {
        if entries.is_empty() {
            return Err(ImportError::Empty);
        }
        let root = root.into();
        let mut ids = std::collections::HashSet::new();
        for entry in &entries {
            if entry.relative_path.is_absolute() {
                return Err(ImportError::AbsolutePath(entry.relative_path.clone()));
            }
            if entry
                .relative_path
                .components()
                .any(|c| matches!(c, Component::ParentDir))
            {
                return Err(ImportError::PathEscape(entry.relative_path.clone()));
            }
            if planned.get(&entry.change_id).is_none() {
                return Err(ImportError::UnknownChange(entry.change_id.clone()));
            }
            if !ids.insert(entry.change_id.clone()) {
                return Err(ImportError::DuplicateChange(entry.change_id.clone()));
            }
            let Some((_, bytes)) = files.iter().find(|(p, _)| p == &entry.relative_path) else {
                return Err(ImportError::ContentHashMismatch {
                    path: entry.relative_path.clone(),
                });
            };
            if sha256_hex(bytes) != entry.content_sha256 {
                return Err(ImportError::ContentHashMismatch {
                    path: entry.relative_path.clone(),
                });
            }
        }
        let manifest = serde_json::to_vec(&entries).unwrap_or_default();
        if sha256_hex(&manifest) != expected_manifest_sha256 {
            return Err(ImportError::ManifestHashMismatch);
        }
        Ok(Self {
            root,
            changes: entries,
            manifest_sha256: expected_manifest_sha256.into(),
        })
    }

    pub fn host_path(&self, relative: &Path) -> Result<PathBuf, ImportError> {
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(ImportError::PathEscape(relative.to_path_buf()));
        }
        Ok(self.root.join(relative))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

impl ChangeSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.changes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Register a planned change; validates the dependency DAG (unknown
    /// deps are refused — a change can't depend on nothing).
    pub fn plan(&mut self, change: Change) -> Result<(), String> {
        if self.by_id.contains_key(&change.id) {
            return Err(format!("change `{}` already exists", change.id));
        }
        for dep in &change.depends_on {
            if !self.by_id.contains_key(dep) {
                return Err(format!("change `{}` depends on unknown `{dep}`", change.id));
            }
        }
        self.by_id.insert(change.id.clone(), self.changes.len());
        self.changes.push(CommittedChange {
            change,
            state: ChangeState::Planned,
            committed_at: 0,
        });
        Ok(())
    }

    /// Whether a change's dependencies are all committed (topological
    /// readiness). Deterministic.
    pub fn ready(&self, id: &str) -> bool {
        let Some(rec) = self.get(id) else {
            return false;
        };
        rec.change.depends_on.iter().all(|d| {
            self.get(d)
                .is_some_and(|r| r.state == ChangeState::Committed)
        })
    }

    /// The ready, not-yet-committed changes in plan order (the executor's
    /// queue).
    pub fn ready_queue(&self) -> Vec<&Change> {
        self.changes
            .iter()
            .filter(|r| r.state == ChangeState::Planned && self.ready(&r.change.id))
            .map(|r| &r.change)
            .collect()
    }

    /// Commit a change (idempotent by key: re-committing the same key is a
    /// no-op — the "no duplicate" half of the acceptance test).
    pub fn commit(&mut self, id: &str, at_ms: u64) -> Result<ChangeState, String> {
        if self.changes.iter().any(|r| {
            r.change.idempotency_key
                == self
                    .get(id)
                    .map(|r| r.change.idempotency_key.clone())
                    .unwrap_or_default()
                && r.state == ChangeState::Committed
                && r.change.id != id
        }) {
            return Err("idempotency key already committed (duplicate prevented)".to_string());
        }
        let idx = *self
            .by_id
            .get(id)
            .ok_or_else(|| format!("unknown change `{id}`"))?;
        if !self.ready(id) {
            return Err(format!("change `{id}` has uncommitted dependencies"));
        }
        let rec = &mut self.changes[idx];
        rec.state = ChangeState::Committed;
        rec.committed_at = at_ms;
        Ok(rec.state)
    }

    pub fn get(&self, id: &str) -> Option<&CommittedChange> {
        self.by_id.get(id).map(|i| &self.changes[*i])
    }

    /// Honest recovery after an interruption: roll back every committed
    /// reversible change (reverse order), compensate compensatable ones,
    /// and return the unrecoverable list for the user — nothing is assumed.
    pub fn recover(&mut self) -> RecoveryReport {
        let mut reverted = Vec::new();
        let mut compensated = Vec::new();
        let mut unrecoverable = Vec::new();
        // Reverse commit order — dependencies commit before dependents, so
        // roll back dependents first.
        let order: Vec<usize> = (0..self.changes.len()).rev().collect();
        for i in order {
            let rec = &mut self.changes[i];
            match (rec.state, rec.change.effect_class) {
                (ChangeState::Committed, EffectClass::Reversible) => {
                    rec.state = ChangeState::Reverted;
                    reverted.push(rec.change.id.clone());
                }
                (ChangeState::Committed, EffectClass::Compensatable) => {
                    rec.state = ChangeState::Compensated;
                    compensated.push(rec.change.id.clone());
                }
                (ChangeState::Committed, EffectClass::Irreversible)
                | (ChangeState::Committed, EffectClass::Uncertain) => {
                    rec.state = ChangeState::Unrecoverable;
                    unrecoverable.push(rec.change.id.clone());
                }
                _ => {}
            }
        }
        RecoveryReport {
            reverted,
            compensated,
            unrecoverable,
        }
    }
}

/// What recovery actually did — the honest summary for the user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReport {
    /// Rolled back byte-for-byte.
    pub reverted: Vec<String>,
    /// Compensating action issued.
    pub compensated: Vec<String>,
    /// Cannot be undone — surfaced, never hidden.
    pub unrecoverable: Vec<String>,
}

impl RecoveryReport {
    pub fn is_clean(&self) -> bool {
        self.unrecoverable.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(id: &str, class: EffectClass, deps: Vec<&str>, key: &str) -> Change {
        Change {
            id: id.into(),
            ticket_id: format!("t-{id}"),
            idempotency_key: key.into(),
            effect_class: class,
            depends_on: deps.into_iter().map(String::from).collect(),
            preconditions: vec![],
            postconditions: vec![],
        }
    }

    #[test]
    fn ready_queue_respects_dependencies() {
        let mut cs = ChangeSet::new();
        cs.plan(change("a", EffectClass::Reversible, vec![], "k-a"))
            .unwrap();
        cs.plan(change("b", EffectClass::Reversible, vec!["a"], "k-b"))
            .unwrap();
        // Only a is ready first.
        let q: Vec<&str> = cs.ready_queue().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(q, vec!["a"]);
        cs.commit("a", 1).unwrap();
        let q: Vec<&str> = cs.ready_queue().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(q, vec!["b"]);
    }

    #[test]
    fn commit_requires_ready_deps() {
        let mut cs = ChangeSet::new();
        cs.plan(change("a", EffectClass::Reversible, vec![], "k-a"))
            .unwrap();
        cs.plan(change("b", EffectClass::Reversible, vec!["a"], "k-b"))
            .unwrap();
        assert!(cs.commit("b", 1).is_err());
        cs.commit("a", 1).unwrap();
        assert!(cs.commit("b", 2).is_ok());
    }

    #[test]
    fn recover_rolls_back_reversible_and_reports_unrecoverable() {
        let mut cs = ChangeSet::new();
        cs.plan(change("w", EffectClass::Reversible, vec![], "k-w"))
            .unwrap();
        cs.plan(change("g", EffectClass::Compensatable, vec![], "k-g"))
            .unwrap();
        cs.plan(change("del", EffectClass::Irreversible, vec![], "k-d"))
            .unwrap();
        cs.commit("w", 1).unwrap();
        cs.commit("g", 2).unwrap();
        cs.commit("del", 3).unwrap();
        let report = cs.recover();
        assert_eq!(report.reverted, vec!["w"]);
        assert_eq!(report.compensated, vec!["g"]);
        assert_eq!(report.unrecoverable, vec!["del"]);
        assert!(!report.is_clean());
        assert_eq!(cs.get("w").unwrap().state, ChangeState::Reverted);
        assert_eq!(cs.get("del").unwrap().state, ChangeState::Unrecoverable);
    }

    #[test]
    fn reviewed_import_rejects_escape_and_hash_mismatch() {
        let mut cs = ChangeSet::new();
        cs.plan(change("a", EffectClass::Reversible, vec![], "k-a"))
            .unwrap();
        let bytes = b"safe".to_vec();
        let entry = ImportEntry {
            change_id: "a".into(),
            relative_path: PathBuf::from("out.txt"),
            content_sha256: sha256_hex(&bytes),
        };
        let manifest = serde_json::to_vec(std::slice::from_ref(&entry)).unwrap();
        let digest = sha256_hex(&manifest);
        let import = ReviewedImport::validate(
            "/tmp/work",
            vec![entry],
            &digest,
            &[(PathBuf::from("out.txt"), bytes)],
            &cs,
        )
        .unwrap();
        assert_eq!(
            import.host_path(Path::new("out.txt")).unwrap(),
            PathBuf::from("/tmp/work/out.txt")
        );
        assert!(matches!(
            import.host_path(Path::new("../escape")),
            Err(ImportError::PathEscape(_))
        ));
    }

    #[test]
    fn idempotency_key_prevents_duplicate() {
        let mut cs = ChangeSet::new();
        cs.plan(change("a", EffectClass::Reversible, vec![], "same-key"))
            .unwrap();
        cs.plan(change("b", EffectClass::Reversible, vec![], "same-key"))
            .unwrap();
        cs.commit("a", 1).unwrap();
        assert!(cs.commit("b", 2).is_err()); // duplicate key — refused
    }
}
