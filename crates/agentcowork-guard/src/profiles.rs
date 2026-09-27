//! P7.7 — profile-gated hooks (doc 46 ECC pattern). The agent runs under
//! one of three guard profiles; each hook (per tool class) is gated by the
//! profile, so the same action pipeline enforces different strictness without
//! changing code paths. `minimal` = fastest, most trusting; `strict` =
//! maximum checks before any side effect.

use crate::ticket::RiskLevel;

/// Guard profile (doc 46: minimal/standard/strict).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Profile {
    Minimal,
    Standard,
    Strict,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Minimal => "minimal",
            Profile::Standard => "standard",
            Profile::Strict => "strict",
        }
    }

    /// Minimum risk level that requires a human-approved ticket.
    pub fn human_approval_threshold(self) -> RiskLevel {
        match self {
            Profile::Minimal => RiskLevel::Critical,
            Profile::Standard => RiskLevel::High,
            Profile::Strict => RiskLevel::Medium,
        }
    }

    /// Do we verify path floors on every file op?
    pub fn enforce_path_floor(self) -> bool {
        matches!(self, Profile::Standard | Profile::Strict)
    }

    /// Do we scan shell output for injection markers before feeding back?
    pub fn scan_tool_output(self) -> bool {
        matches!(self, Profile::Standard | Profile::Strict)
    }

    /// Do we require the red-team gate before the session can run?
    pub fn require_red_team_gate(self) -> bool {
        matches!(self, Profile::Strict)
    }

    /// Does a network call to a new domain need approval?
    pub fn ask_on_new_domain(self) -> bool {
        matches!(self, Profile::Standard | Profile::Strict)
    }
}

/// What a hook gate asks the caller to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateAction {
    Allow,
    /// Ask the user (Guard-2 card / permissions.toml).
    Ask,
    /// Refuse outright.
    Block,
}

/// Evaluate one hook under a profile. Deterministic.
pub fn gate(profile: Profile, hook: Hook) -> GateAction {
    use Hook::*;
    match hook {
        PathFloor => {
            // The floor is enforced inside, not asked — always allow through
            // the hook; violations are refused by the floor itself.
            let _ = profile.enforce_path_floor();
            GateAction::Allow
        }
        NewDomain => {
            if profile.ask_on_new_domain() {
                GateAction::Ask
            } else {
                GateAction::Allow
            }
        }
        ToolOutputScan => {
            // The scan runs (flagged lines are neutralized), never asked.
            let _ = profile.scan_tool_output();
            GateAction::Allow
        }
        SensitiveFile => match profile {
            Profile::Minimal => GateAction::Allow,
            Profile::Standard => GateAction::Ask,
            Profile::Strict => GateAction::Block,
        },
        KeyMaterial => match profile {
            Profile::Minimal => GateAction::Ask,
            Profile::Standard => GateAction::Block,
            Profile::Strict => GateAction::Block,
        },
        ExternalNetwork => match profile {
            Profile::Minimal => GateAction::Allow,
            Profile::Standard => GateAction::Ask,
            Profile::Strict => GateAction::Ask,
        },
        DestructiveCommand => match profile {
            Profile::Minimal => GateAction::Ask,
            Profile::Standard => GateAction::Block,
            Profile::Strict => GateAction::Block,
        },
    }
}

/// The hooks the guard pipeline evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    /// Path is inside the granted floor.
    PathFloor,
    /// Network request to a domain not seen before.
    NewDomain,
    /// Scan tool output for injection markers.
    ToolOutputScan,
    /// Reading/writing sensitive files (.env, id_rsa, credentials).
    SensitiveFile,
    /// Key material exfiltration attempts.
    KeyMaterial,
    /// Any external network call.
    ExternalNetwork,
    /// Destructive shell command.
    DestructiveCommand,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_blocks_destructive_without_ask() {
        assert_eq!(
            gate(Profile::Strict, Hook::DestructiveCommand),
            GateAction::Block
        );
        assert_eq!(
            gate(Profile::Standard, Hook::DestructiveCommand),
            GateAction::Block
        );
        assert_eq!(
            gate(Profile::Minimal, Hook::DestructiveCommand),
            GateAction::Ask
        );
    }

    #[test]
    fn sensitive_file_escalates() {
        assert_eq!(
            gate(Profile::Minimal, Hook::SensitiveFile),
            GateAction::Allow
        );
        assert_eq!(
            gate(Profile::Standard, Hook::SensitiveFile),
            GateAction::Ask
        );
        assert_eq!(
            gate(Profile::Strict, Hook::SensitiveFile),
            GateAction::Block
        );
    }

    #[test]
    fn approval_threshold_orders() {
        // Strict demands human approval from Medium upward; Minimal only for
        // Critical. So threshold: Strict < Standard < Minimal.
        assert!(
            Profile::Strict.human_approval_threshold()
                < Profile::Standard.human_approval_threshold()
        );
        assert!(
            Profile::Standard.human_approval_threshold()
                < Profile::Minimal.human_approval_threshold()
        );
    }

    #[test]
    fn key_material_always_gated() {
        assert_eq!(gate(Profile::Minimal, Hook::KeyMaterial), GateAction::Ask);
        assert_eq!(gate(Profile::Strict, Hook::KeyMaterial), GateAction::Block);
    }
}
