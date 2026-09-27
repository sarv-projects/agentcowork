//! Guard-2-ticketed cleanup proposals (D9 cleanup, ARCH/06).
//!
//! This crate **never deletes**. It only *proposes* `CleanupAction`s, each of
//! which must be converted to a Guard-2 decision package and approved before
//! the core executes the actual move (recycle-bin-aware). The invariant is
//! "sidecar proposes, Rust disposes": cleanup never bypasses the dual-guard.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::dedup::DupGroup;
use crate::finder::{FinderOptions, SortBy, find_large_files};
use crate::walk::Arena;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CleanupKind {
    /// Move to the OS recycle bin (default, reversible).
    MoveToTrash,
    /// Replace a copy with a hardlink to the kept file (same inode).
    ReplaceWithHardlink,
    /// Replace a copy with a reflink (CoW clone on btrfs/xfs/apfs).
    ReplaceWithReflink,
    /// Permanent unlink — highest risk, never proposed automatically.
    DeletePermanently,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CleanupAction {
    pub path: PathBuf,
    pub kind: CleanupKind,
    pub freed_bytes: u64,
    pub rationale: String,
    /// Always true: cleanup never bypasses the dual-guard.
    pub requires_ticket: bool,
    /// `false` when the physical-copy test behind `freed_bytes` could not be
    /// confirmed for this candidate (unknown file identity, or a group whose
    /// copy count is unprovable). The number is then an upper bound, not a
    /// measurement, and must be presented as such.
    pub identity_backed: bool,
}

impl CleanupAction {
    /// The structured decision package rendered as a Guard-2 card.
    pub fn decision_package(&self) -> serde_json::Value {
        serde_json::json!({
            "tool_id": "disk_cleanup",
            "operation": match self.kind {
                CleanupKind::MoveToTrash => "move_to_trash",
                CleanupKind::ReplaceWithHardlink => "replace_with_hardlink",
                CleanupKind::ReplaceWithReflink => "replace_with_reflink",
                CleanupKind::DeletePermanently => "delete_permanently",
            },
            "path": self.path.to_string_lossy(),
            "freed_bytes": self.freed_bytes,
            "rationale": self.rationale,
            "requires_ticket": self.requires_ticket,
            "identity_backed": self.identity_backed,
            "risk": "destructive",
        })
    }
}

/// For each duplicate group, keep one copy (lexicographically first path) and
/// propose moving the rest to trash. Redundant hardlink names (provably the
/// same physical file) free 0 bytes but are still proposed for tidiness.
///
/// **FIX-10 / `REQ-FILES-001`:** the physical-copy test used to be "has this
/// `(dev, ino)` pair been seen?". Every pre-fix Windows candidate answered
/// `(0, 0)`, so the *first* file claimed the only key and every remaining
/// duplicate was reported as a redundant hardlink with `freed_bytes: 0`. The
/// test is now `DupCandidate::hardlink_key()`: a candidate whose identity is
/// unknown (or which has a single link) frees its full size and marks the
/// action `identity_backed: false`, so the estimate is visibly an upper bound
/// rather than a measured zero.
pub fn propose_duplicate_cleanup(groups: &[DupGroup]) -> Vec<CleanupAction> {
    let mut out = Vec::new();
    for g in groups {
        let mut order: Vec<usize> = (0..g.files.len()).collect();
        order.sort_by(|&a, &b| g.files[a].path.cmp(&g.files[b].path));
        if order.is_empty() {
            continue;
        }

        let mut seen: HashSet<crate::identity::FileKey> = HashSet::new();
        // First (kept) file claims its physical-copy key, if it has one.
        if let Some(k) = order.first().and_then(|&i| g.files[i].hardlink_key()) {
            seen.insert(k);
        }
        let kept = g.files[order[0]].path.clone();
        for &i in &order[1..] {
            let f = &g.files[i];
            let is_new_copy = match f.hardlink_key() {
                Some(k) => seen.insert(k),
                None => true,
            };
            let freed = if is_new_copy { g.size } else { 0 };
            out.push(CleanupAction {
                path: f.path.clone(),
                kind: CleanupKind::MoveToTrash,
                freed_bytes: freed,
                rationale: format!("duplicate of {}", kept.display()),
                requires_ticket: true,
                // The group's copy count is provable, so this action's number
                // is provable too — including a hard-provable `0`.
                identity_backed: g.identity_backed,
            });
        }
    }
    out
}

/// Propose moving the top-N largest files to trash.
pub fn propose_large_files_cleanup(arena: &Arena, top_n: usize) -> Vec<CleanupAction> {
    let files = find_large_files(
        arena,
        &FinderOptions {
            top_n,
            ..Default::default()
        },
        SortBy::SizeDesc,
        u64::MAX,
    );
    files
        .into_iter()
        .map(|f| CleanupAction {
            path: PathBuf::from(&f.path),
            kind: CleanupKind::MoveToTrash,
            freed_bytes: f.size,
            rationale: format!("large file ({:.1} MiB)", f.size as f64 / (1024.0 * 1024.0)),
            requires_ticket: true,
            identity_backed: true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dedup::DupCandidate;

    #[test]
    fn keeps_one_and_tickets_the_rest() {
        let group = DupGroup {
            size: 10,
            files: vec![
                DupCandidate {
                    path: "b".into(),
                    size: 10,
                    dev: 0,
                    ino: 2,
                    nlink: 1,
                },
                DupCandidate {
                    path: "a".into(),
                    size: 10,
                    dev: 0,
                    ino: 1,
                    nlink: 1,
                },
                DupCandidate {
                    path: "c".into(),
                    size: 10,
                    dev: 0,
                    ino: 3,
                    nlink: 1,
                },
            ],
            wasted_bytes: 20,
            hardlink_groups: 3,
            reflink_eligible: true,
            identity_backed: true,
        };

        let actions = propose_duplicate_cleanup(&[group]);
        assert_eq!(actions.len(), 2); // keep "a", trash "b" and "c"
        assert_eq!(actions[0].path.to_string_lossy(), "b");
        assert_eq!(actions[1].path.to_string_lossy(), "c");
        assert!(actions.iter().all(|a| a.requires_ticket));
        assert!(actions.iter().all(|a| a.kind == CleanupKind::MoveToTrash));
        assert!(actions.iter().all(|a| a.freed_bytes == 10));
        assert!(actions.iter().all(|a| a.identity_backed));

        // Decision package shape.
        let pkg = actions[0].decision_package();
        assert_eq!(pkg["tool_id"], "disk_cleanup");
        assert_eq!(pkg["requires_ticket"], true);
        assert_eq!(pkg["identity_backed"], true);
    }

    #[test]
    fn redundant_hardlink_frees_zero() {
        let group = DupGroup {
            size: 10,
            files: vec![
                DupCandidate {
                    path: "a".into(),
                    size: 10,
                    dev: 0,
                    ino: 7,
                    nlink: 2,
                },
                DupCandidate {
                    path: "b".into(),
                    size: 10,
                    dev: 0,
                    ino: 7,
                    nlink: 2,
                },
            ],
            wasted_bytes: 0,
            hardlink_groups: 1,
            reflink_eligible: true,
            identity_backed: true,
        };
        let actions = propose_duplicate_cleanup(&[group]);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].freed_bytes, 0);
        assert!(actions[0].identity_backed);
    }

    // --- FIX-10: unknown identity must not read as "same inode" -------------

    #[test]
    fn zeroed_identity_frees_the_full_size_not_zero() {
        // The pre-fix Windows record (`dev = 0, ino = 0`) made the kept file
        // claim the only key, so every remaining duplicate was reported as a
        // redundant hardlink with `freed_bytes: 0`.
        let group = DupGroup {
            size: 10,
            files: vec![
                DupCandidate {
                    path: "a".into(),
                    size: 10,
                    dev: 0,
                    ino: 0,
                    nlink: 1,
                },
                DupCandidate {
                    path: "b".into(),
                    size: 10,
                    dev: 0,
                    ino: 0,
                    nlink: 1,
                },
            ],
            wasted_bytes: 10,
            hardlink_groups: 2,
            reflink_eligible: false,
            identity_backed: false,
        };
        let actions = propose_duplicate_cleanup(&[group]);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].freed_bytes, 10, "not a fabricated zero");
        // ...but the estimate is flagged as unprovable.
        assert!(!actions[0].identity_backed);
        assert_eq!(actions[0].decision_package()["identity_backed"], false);
    }

    #[test]
    fn single_link_candidate_never_reads_as_a_redundant_hardlink() {
        // Even with a *matching* id, `nlink == 1` proves there is no twin.
        let group = DupGroup {
            size: 10,
            files: vec![
                DupCandidate {
                    path: "a".into(),
                    size: 10,
                    dev: 9,
                    ino: 42,
                    nlink: 1,
                },
                DupCandidate {
                    path: "b".into(),
                    size: 10,
                    dev: 9,
                    ino: 42,
                    nlink: 1,
                },
            ],
            wasted_bytes: 10,
            hardlink_groups: 2,
            reflink_eligible: true,
            identity_backed: true,
        };
        let actions = propose_duplicate_cleanup(&[group]);
        assert_eq!(actions[0].freed_bytes, 10);
    }

    #[test]
    fn empty_group_produces_no_actions() {
        let group = DupGroup {
            size: 10,
            files: vec![],
            wasted_bytes: 0,
            hardlink_groups: 0,
            reflink_eligible: false,
            identity_backed: true,
        };
        assert!(propose_duplicate_cleanup(&[group]).is_empty());
    }
}
