//! P51.22 — connector approval matrix: per-(connector, op) rules with
//! per-task overrides, a delete-always-asks floor, and review-cost hints.
//!
//! Pure and deterministic: the matrix never executes effects, it only resolves
//! the standing rule for a `(connector, op)` pair (optionally scoped to one
//! task) so the Guard/executor call-site can decide Allow vs Ask vs Block.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The standing rule for one `(connector, op)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorRule {
    /// Auto-approved — no human prompt.
    Always,
    /// Requires a human approval ticket before running.
    NeedsApproval,
    /// Refused outright — no ticket can lift it.
    Blocked,
}

/// The approval matrix: global `(connector, op)` rules plus per-task
/// overrides. A task override wins over the global rule for that task only.
#[derive(Debug, Clone, Default)]
pub struct ConnectorMatrix {
    pub rules: HashMap<(String, String), ConnectorRule>,
    pub task_overrides: HashMap<String, HashMap<(String, String), ConnectorRule>>,
}

impl ConnectorMatrix {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert (or replace) a global rule.
    pub fn set(&mut self, connector: &str, op: &str, rule: ConnectorRule) {
        self.rules
            .insert((connector.to_string(), op.to_string()), rule);
    }

    /// Insert (or replace) a per-task override scoped to `task` only.
    pub fn set_task_override(
        &mut self,
        task: &str,
        connector: &str,
        op: &str,
        rule: ConnectorRule,
    ) {
        self.task_overrides
            .entry(task.to_string())
            .or_default()
            .insert((connector.to_string(), op.to_string()), rule);
    }

    /// Resolve the rule for `(connector, op)`, optionally scoped to `task`.
    ///
    /// Order: task override (exact task only) wins → global rule → default
    /// [`ConnectorRule::NeedsApproval`]. The delete floor then applies: any
    /// op whose name contains `delete` (case-insensitive) is at minimum
    /// [`ConnectorRule::NeedsApproval`] — an `Always` on a delete op still
    /// asks. [`ConnectorRule::Blocked`] is never lowered by the floor (it is
    /// already stricter).
    pub fn resolve(&self, connector: &str, op: &str, task: Option<&str>) -> ConnectorRule {
        let key = (connector.to_string(), op.to_string());
        let base = task
            .and_then(|t| self.task_overrides.get(t))
            .and_then(|overrides| overrides.get(&key))
            .copied()
            .or_else(|| self.rules.get(&key).copied())
            .unwrap_or(ConnectorRule::NeedsApproval);
        if op.to_ascii_lowercase().contains("delete") && base == ConnectorRule::Always {
            return ConnectorRule::NeedsApproval;
        }
        base
    }

    /// Human-readable review-cost hint for an approval mode.
    ///
    /// `Manual` = a human reviews each call; `Auto` = runs without a prompt
    /// but incurs extra reviewer cost; `Skip` = the connector step is skipped
    /// (no review, no result). Matching is case-insensitive; unknown modes
    /// fall back to the manual hint.
    pub fn cost_hint(mode: &str) -> &'static str {
        match mode.to_ascii_lowercase().as_str() {
            "auto" => "auto approval: no per-call prompt, but incurs extra reviewer cost",
            "skip" => "skipped: connector step is skipped entirely, no review and no result",
            _ => "manual approval: a human reviews each call, no extra reviewer cost",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix() -> ConnectorMatrix {
        ConnectorMatrix::new()
    }

    #[test]
    fn connector_blocked_wins_over_auto() {
        let mut m = matrix();
        m.set("github", "read", ConnectorRule::Always);
        // A task-scoped Blocked beats the global Always for that task.
        m.set_task_override("task-1", "github", "read", ConnectorRule::Blocked);
        assert_eq!(
            m.resolve("github", "read", Some("task-1")),
            ConnectorRule::Blocked
        );
        // A standing Blocked is never softened.
        let mut m2 = matrix();
        m2.set("github", "read", ConnectorRule::Blocked);
        assert_eq!(m2.resolve("github", "read", None), ConnectorRule::Blocked);
        assert_eq!(
            m2.resolve("github", "read", Some("other-task")),
            ConnectorRule::Blocked
        );
    }

    #[test]
    fn per_task_override_scoped_to_task() {
        let mut m = matrix();
        m.set("calendar", "write", ConnectorRule::NeedsApproval);
        m.set_task_override("task-a", "calendar", "write", ConnectorRule::Always);
        assert_eq!(
            m.resolve("calendar", "write", Some("task-a")),
            ConnectorRule::Always
        );
        // Other tasks and the unscoped lookup still see the global rule.
        assert_eq!(
            m.resolve("calendar", "write", Some("task-b")),
            ConnectorRule::NeedsApproval
        );
        assert_eq!(
            m.resolve("calendar", "write", None),
            ConnectorRule::NeedsApproval
        );
    }

    #[test]
    fn delete_always_asks_despite_always() {
        let mut m = matrix();
        m.set("drive", "delete_file", ConnectorRule::Always);
        m.set("drive", "read", ConnectorRule::Always);
        // Delete-like ops floor to at minimum NeedsApproval.
        assert_eq!(
            m.resolve("drive", "delete_file", None),
            ConnectorRule::NeedsApproval
        );
        assert_eq!(
            m.resolve("drive", "delete", None),
            ConnectorRule::NeedsApproval
        );
        assert_eq!(
            m.resolve("drive", "bulkDeleteSharingLink", None),
            ConnectorRule::NeedsApproval
        );
        // Non-delete ops keep their Always.
        assert_eq!(m.resolve("drive", "read", None), ConnectorRule::Always);
        // Unknown pairs default to NeedsApproval.
        assert_eq!(
            m.resolve("drive", "unknown_op", None),
            ConnectorRule::NeedsApproval
        );
    }

    #[test]
    fn cost_hint_differs_per_mode() {
        let manual = ConnectorMatrix::cost_hint("Manual");
        let auto = ConnectorMatrix::cost_hint("Auto");
        let skip = ConnectorMatrix::cost_hint("Skip");
        assert_ne!(manual, auto);
        assert_ne!(manual, skip);
        assert_ne!(auto, skip);
        assert!(
            auto.contains("extra reviewer cost"),
            "auto hint must note extra reviewer cost, got: {auto}"
        );
    }
}
