//! P37 worktree disk cap (P20 / P7.8): the enforcer that keeps a fleet of
//! sub-agent git worktrees from eating the disk. The cap is checked before a
//! worktree is created: `would_exceed` is the deterministic gate; the
//! coordinator calls it with the measured used bytes before `git worktree
//! add`.

use serde::{Deserialize, Serialize};

/// The worktree disk budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeCap {
    /// Hard ceiling in GiB across all worktrees.
    pub max_gib: u64,
    /// GiB already consumed by worktrees.
    pub used_gib: u64,
    /// A per-worktree reservation floor — creating a worktree always
    /// reserves at least this much (so tiny repos can't mask runaway growth).
    pub min_reserve_gib: u64,
}

impl Default for WorktreeCap {
    fn default() -> Self {
        Self {
            max_gib: 8,
            used_gib: 0,
            min_reserve_gib: 1,
        }
    }
}

/// The cap verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapVerdict {
    /// The worktree may be created (updates the used budget).
    Allowed,
    /// Refused — creating it would exceed the cap.
    Refused,
}

impl WorktreeCap {
    /// The gate: `new_worktree_gib` (the repo's checkout size) is charged at
    /// least `min_reserve_gib`. Deterministic.
    pub fn would_exceed(&self, new_worktree_gib: u64) -> bool {
        let charge = new_worktree_gib.max(self.min_reserve_gib);
        self.used_gib.saturating_add(charge) > self.max_gib
    }

    /// Evaluate + reserve: `Allowed` charges the budget; `Refused` leaves it
    /// untouched.
    pub fn reserve(&mut self, new_worktree_gib: u64) -> CapVerdict {
        if self.would_exceed(new_worktree_gib) {
            CapVerdict::Refused
        } else {
            self.used_gib = self
                .used_gib
                .saturating_add(new_worktree_gib.max(self.min_reserve_gib));
            CapVerdict::Allowed
        }
    }

    /// Release a worktree's charge (on merge/revert).
    pub fn release(&mut self, gib: u64) {
        self.used_gib = self.used_gib.saturating_sub(gib);
    }

    pub fn remaining_gib(&self) -> u64 {
        self.max_gib.saturating_sub(self.used_gib)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_refuses_over_cap() {
        let mut cap = WorktreeCap {
            max_gib: 8,
            used_gib: 7,
            min_reserve_gib: 1,
        };
        assert!(cap.would_exceed(2));
        assert_eq!(cap.reserve(2), CapVerdict::Refused);
        assert_eq!(cap.used_gib, 7); // untouched on refusal
    }

    #[test]
    fn min_reserve_floors_tiny_repos() {
        let mut cap = WorktreeCap {
            max_gib: 2,
            used_gib: 1,
            min_reserve_gib: 1,
        };
        // A 0.1GiB repo still charges 1GiB — 3 tiny worktrees can't hide.
        assert_eq!(cap.reserve(0), CapVerdict::Allowed);
        assert_eq!(cap.used_gib, 2);
        assert_eq!(cap.reserve(0), CapVerdict::Refused);
    }

    #[test]
    fn release_frees_budget() {
        let mut cap = WorktreeCap::default();
        assert_eq!(cap.reserve(3), CapVerdict::Allowed);
        cap.release(3);
        assert_eq!(cap.used_gib, 0);
        assert_eq!(cap.remaining_gib(), 8);
    }
}
