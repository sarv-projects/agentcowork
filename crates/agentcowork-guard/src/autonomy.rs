//! P30.1/P30.3 — the **RiskClass × Mode autonomy gradient** (openworker
//! pattern, doc 83 §1, reimplemented on our stack). This is the user-facing
//! layer *over* `permissions.toml` (P7.5): `RiskClass` classifies the effect
//! of an action (`READ` / `WRITE_LOCAL` / `EXEC` / `EXTERNAL`), `Mode` selects
//! how autonomous a run is allowed to be (`DISCUSS` / `PLAN` / `INTERACTIVE` /
//! `AUTO` / `CUSTOM`), and the combination resolves to an
//! [`AutonomyVerdict`]. The numeric Trust Ladder (J21/RiskTier) stays the
//! underlying score; this layer is the knob the user turns.
//!
//! P30.3 — background/unattended runs park their `EXTERNAL`-risk asks in an
//! inbox instead of acting: [`AutonomyPolicy::unattended_verdict`] returns
//! [`AutonomyVerdict::ParkInInbox`] for off-machine effects when no human is
//! watching, powering the messaging + automation proactivity layer.

use crate::permissions::Operation;
use crate::ticket::RiskTier;
use serde::{Deserialize, Serialize};

/// The effect class of an action (openworker `RiskClass`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiskClass {
    /// Pure read — no state change, on- or off-machine.
    Read,
    /// Local, reversible write (file edit, folder create).
    WriteLocal,
    /// Local execution (shell, script, spawn).
    Exec,
    /// Off-machine side effect (network send, web submit, email, publish).
    External,
}

impl RiskClass {
    pub fn as_str(self) -> &'static str {
        match self {
            RiskClass::Read => "READ",
            RiskClass::WriteLocal => "WRITE_LOCAL",
            RiskClass::Exec => "EXEC",
            RiskClass::External => "EXTERNAL",
        }
    }

    /// Map a policy operation to its effect class.
    pub fn from_operation(op: &Operation) -> RiskClass {
        match op {
            Operation::ExternalNetwork { .. } | Operation::WebAction => RiskClass::External,
            Operation::TerminalShell { .. } | Operation::GenericWrite => RiskClass::Exec,
            Operation::DeleteFiles | Operation::MultiFileEdit { .. } => RiskClass::WriteLocal,
            Operation::Read { .. } => RiskClass::Read,
        }
    }

    /// The underlying numeric tier (J21/RiskTier) for this class.
    pub fn risk_tier(self, destructive: bool) -> RiskTier {
        match self {
            RiskClass::Read => RiskTier::R0,
            RiskClass::WriteLocal => {
                if destructive {
                    RiskTier::R3
                } else {
                    RiskTier::R1
                }
            }
            RiskClass::Exec => RiskTier::R2,
            RiskClass::External => RiskTier::R2,
        }
    }
}

/// The autonomy mode of a run (openworker `Mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Mode {
    /// Talk only — never act; every action resolves to Ask.
    Discuss,
    /// Read + propose; mutations resolve to Ask.
    Plan,
    /// Ask before each non-read action (default for casual users).
    Interactive,
    /// Run reads + reversible local writes; ask before exec/external.
    Auto,
    /// Follow the `permissions.toml` rules verbatim (power users).
    Custom,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Discuss => "DISCUSS",
            Mode::Plan => "PLAN",
            Mode::Interactive => "INTERACTIVE",
            Mode::Auto => "AUTO",
            Mode::Custom => "CUSTOM",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s.to_uppercase().as_str() {
            "DISCUSS" => Mode::Discuss,
            "PLAN" => Mode::Plan,
            "INTERACTIVE" => Mode::Interactive,
            "AUTO" => Mode::Auto,
            "CUSTOM" => Mode::Custom,
            _ => return None,
        })
    }
}

/// What the autonomy layer says about an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyVerdict {
    /// Proceed (Guard-1/tickets still apply as usual).
    Act,
    /// Mint a Guard-2 ticket and wait for a human.
    Ask,
    /// Unattended run — park in the inbox, never act silently.
    ParkInInbox,
    /// Refuse outright.
    Block,
}

impl AutonomyVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            AutonomyVerdict::Act => "act",
            AutonomyVerdict::Ask => "ask",
            AutonomyVerdict::ParkInInbox => "park_in_inbox",
            AutonomyVerdict::Block => "block",
        }
    }
}

/// A resolved policy: the effect class of the action + the run's mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutonomyPolicy {
    pub class: RiskClass,
    pub mode: Mode,
}

impl AutonomyPolicy {
    pub fn new(class: RiskClass, mode: Mode) -> Self {
        Self { class, mode }
    }

    /// Resolve the gradient for an action. `destructive` sharpens the tier.
    pub fn resolve(&self, op: &Operation, destructive: bool) -> AutonomyVerdict {
        match self.mode {
            Mode::Discuss => match self.class {
                RiskClass::Read => AutonomyVerdict::Act,
                _ => AutonomyVerdict::Ask,
            },
            Mode::Plan => match self.class {
                RiskClass::Read => AutonomyVerdict::Act,
                _ => AutonomyVerdict::Ask,
            },
            Mode::Interactive => match self.class {
                RiskClass::Read => AutonomyVerdict::Act,
                _ => AutonomyVerdict::Ask,
            },
            Mode::Auto => match self.class {
                RiskClass::Read => AutonomyVerdict::Act,
                RiskClass::WriteLocal if !destructive => AutonomyVerdict::Act,
                RiskClass::WriteLocal => AutonomyVerdict::Ask,
                RiskClass::Exec => AutonomyVerdict::Ask,
                RiskClass::External => AutonomyVerdict::Ask,
            },
            Mode::Custom => {
                // The permissions.toml rule decides; the gradient only
                // classifies. `op` is the same object the policy evaluates.
                let _ = op;
                AutonomyVerdict::Act
            }
        }
    }

    /// P30.3 — the unattended variant: a background/headless/scheduled run
    /// with no human watching. Off-machine (and local-exec) effects park in
    /// the inbox instead of acting; reversible local writes may proceed.
    pub fn unattended_verdict(&self, _op: &Operation, destructive: bool) -> AutonomyVerdict {
        match self.class {
            RiskClass::Read => AutonomyVerdict::Act,
            RiskClass::WriteLocal if !destructive => AutonomyVerdict::Act,
            RiskClass::WriteLocal => AutonomyVerdict::ParkInInbox,
            RiskClass::Exec => AutonomyVerdict::ParkInInbox,
            RiskClass::External => AutonomyVerdict::ParkInInbox,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(name: &str) -> Operation {
        match name {
            "delete" => Operation::DeleteFiles,
            "multi" => Operation::MultiFileEdit { files: 3 },
            "net" => Operation::ExternalNetwork { new_domain: true },
            "shell" => Operation::TerminalShell { destructive: false },
            "web" => Operation::WebAction,
            _ => Operation::GenericWrite,
        }
    }

    #[test]
    fn class_mapping() {
        assert_eq!(RiskClass::from_operation(&op("net")), RiskClass::External);
        assert_eq!(RiskClass::from_operation(&op("web")), RiskClass::External);
        assert_eq!(RiskClass::from_operation(&op("shell")), RiskClass::Exec);
        assert_eq!(
            RiskClass::from_operation(&op("delete")),
            RiskClass::WriteLocal
        );
        assert_eq!(RiskClass::risk_tier(RiskClass::Read, false), RiskTier::R0);
        assert_eq!(
            RiskClass::risk_tier(RiskClass::WriteLocal, true),
            RiskTier::R3
        );
    }

    #[test]
    fn discuss_and_plan_never_act() {
        for mode in [Mode::Discuss, Mode::Plan] {
            let p = AutonomyPolicy::new(RiskClass::External, mode);
            assert_eq!(p.resolve(&op("net"), false), AutonomyVerdict::Ask);
            let p = AutonomyPolicy::new(RiskClass::WriteLocal, mode);
            assert_eq!(p.resolve(&op("multi"), false), AutonomyVerdict::Ask);
            let p = AutonomyPolicy::new(RiskClass::Read, mode);
            assert_eq!(p.resolve(&op("write"), false), AutonomyVerdict::Act);
        }
    }

    #[test]
    fn interactive_asks_for_all_mutations() {
        let p = AutonomyPolicy::new(RiskClass::WriteLocal, Mode::Interactive);
        assert_eq!(p.resolve(&op("multi"), false), AutonomyVerdict::Ask);
        let p = AutonomyPolicy::new(RiskClass::Read, Mode::Interactive);
        assert_eq!(p.resolve(&op("write"), false), AutonomyVerdict::Act);
    }

    #[test]
    fn auto_acts_on_reversible_local_only() {
        let p = AutonomyPolicy::new(RiskClass::WriteLocal, Mode::Auto);
        assert_eq!(p.resolve(&op("multi"), false), AutonomyVerdict::Act);
        let p = AutonomyPolicy::new(RiskClass::WriteLocal, Mode::Auto);
        assert_eq!(p.resolve(&op("delete"), true), AutonomyVerdict::Ask);
        let p = AutonomyPolicy::new(RiskClass::Exec, Mode::Auto);
        assert_eq!(p.resolve(&op("shell"), false), AutonomyVerdict::Ask);
    }

    #[test]
    fn unattended_parks_external_and_exec() {
        let p = AutonomyPolicy::new(RiskClass::External, Mode::Auto);
        assert_eq!(
            p.unattended_verdict(&op("net"), false),
            AutonomyVerdict::ParkInInbox
        );
        let p = AutonomyPolicy::new(RiskClass::Exec, Mode::Auto);
        assert_eq!(
            p.unattended_verdict(&op("shell"), false),
            AutonomyVerdict::ParkInInbox
        );
        let p = AutonomyPolicy::new(RiskClass::WriteLocal, Mode::Auto);
        assert_eq!(
            p.unattended_verdict(&op("multi"), false),
            AutonomyVerdict::Act
        );
        let p = AutonomyPolicy::new(RiskClass::Read, Mode::Auto);
        assert_eq!(
            p.unattended_verdict(&op("write"), false),
            AutonomyVerdict::Act
        );
    }

    #[test]
    fn mode_parse_roundtrip() {
        for m in [
            Mode::Discuss,
            Mode::Plan,
            Mode::Interactive,
            Mode::Auto,
            Mode::Custom,
        ] {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
        }
        assert_eq!(Mode::parse("nope"), None);
    }
}
