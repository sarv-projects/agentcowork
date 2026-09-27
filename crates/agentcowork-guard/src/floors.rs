//! P51.29 — human floors, MCP risk floor, run grants, unlocated writes.
//!
//! Hard floors that no autonomy preset may bypass: writes under
//! in-project protected prefixes and persistent-authority ops always
//! require a human; MCP/external effects never grade below
//! [`RiskClass::External`]; writes with no located grant are blocked.

use crate::autonomy::{AutonomyVerdict, RiskClass};

/// Ops and in-project paths that always require a human, even on the
/// maximum-autonomy preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanFloor {
    /// In-project path prefixes (e.g. `.git/hooks/`, `.github/workflows/`,
    /// `.vscode/tasks.json`, `.coworker/`). Any touched path containing one
    /// of these prefixes requires a human.
    pub protected_in_project: Vec<String>,
    /// Op names that confer persistent authority (e.g. `save_skill`,
    /// `create_scheduled_task`). Matching is case-insensitive exact.
    pub persistent_authority: Vec<String>,
}

impl HumanFloor {
    pub fn new(protected_in_project: Vec<String>, persistent_authority: Vec<String>) -> Self {
        Self {
            protected_in_project,
            persistent_authority,
        }
    }

    /// The documented default floor.
    pub fn defaults() -> Self {
        Self {
            protected_in_project: vec![
                ".git/hooks/".to_string(),
                ".github/workflows/".to_string(),
                ".vscode/tasks.json".to_string(),
                ".coworker/".to_string(),
            ],
            persistent_authority: vec![
                "save_skill".to_string(),
                "create_scheduled_task".to_string(),
            ],
        }
    }

    /// Does this `(op, path)` require a human? True when the op names a
    /// persistent authority, or when the path touches a protected
    /// in-project prefix (substring match so absolute project paths like
    /// `/proj/.git/hooks/x` still trip the floor — fail-closed).
    pub fn requires_human(&self, op: &str, path: Option<&str>) -> bool {
        let op_lower = op.to_lowercase();
        if self
            .persistent_authority
            .iter()
            .any(|a| a.to_lowercase() == op_lower)
        {
            return true;
        }
        if let Some(p) = path {
            let normalized = p.replace('\\', "/");
            for prefix in &self.protected_in_project {
                let norm_prefix = prefix.replace('\\', "/");
                if normalized.contains(norm_prefix.as_str()) {
                    return true;
                }
            }
        }
        false
    }
}

impl Default for HumanFloor {
    fn default() -> Self {
        Self::defaults()
    }
}

/// MCP/external ops never grade below [`RiskClass::External`].
pub fn mcp_floor_risk() -> RiskClass {
    RiskClass::External
}

/// A run-scoped grant: an id plus the scope it covers and a unix-ms expiry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunGrant {
    pub id: String,
    pub scope: String,
    pub expires_at_ms: u64,
}

impl RunGrant {
    pub fn new(id: &str, scope: &str, expires_at_ms: u64) -> Self {
        Self {
            id: id.to_string(),
            scope: scope.to_string(),
            expires_at_ms,
        }
    }

    /// Is this grant live at `now_ms`? `expires_at_ms == 0` means no expiry
    /// (follows the ticket/broker convention); otherwise live while
    /// `now_ms < expires_at_ms`.
    pub fn is_live(&self, now_ms: u64) -> bool {
        if self.expires_at_ms == 0 {
            return true;
        }
        now_ms < self.expires_at_ms
    }
}

/// Writes with no located grant are blocked outright.
pub fn unlocated_write_verdict() -> AutonomyVerdict {
    AutonomyVerdict::Block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_in_project_requires_human() {
        let floor = HumanFloor::defaults();
        assert!(floor.requires_human("write", Some(".git/hooks/pre-commit")));
        assert!(floor.requires_human("write", Some("/proj/.github/workflows/ci.yml")));
        assert!(!floor.requires_human("write", Some("src/main.rs")));
        assert!(!floor.requires_human("write", None));
    }

    #[test]
    fn persistent_authority_requires_human() {
        let floor = HumanFloor::defaults();
        assert!(floor.requires_human("save_skill", None));
        assert!(floor.requires_human("create_scheduled_task", Some("src/main.rs")));
        // Case-insensitive op match.
        assert!(floor.requires_human("SAVE_SKILL", None));
        assert!(!floor.requires_human("write", None));
    }

    #[test]
    fn unlocated_write_blocked() {
        assert_eq!(unlocated_write_verdict(), AutonomyVerdict::Block);
    }

    #[test]
    fn mcp_op_never_below_external() {
        assert_eq!(mcp_floor_risk(), RiskClass::External);
    }

    #[test]
    fn run_grant_expired_denied() {
        let expired = RunGrant::new("g1", "fs.write:/tmp", 1000);
        assert!(!expired.is_live(1000));
        assert!(!expired.is_live(2000));
        let live = RunGrant::new("g2", "fs.write:/tmp", 2000);
        assert!(live.is_live(1000));
    }
}
