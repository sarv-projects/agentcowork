//! P7.5 / J21 — the `<data_dir>/permissions.toml` escalation-rules policy
//! layer (doc 52 §2). Guard-1 blocks *known-bad* strings deterministically;
//! this layer answers the next question — *does this action need a human?* —
//! from user policy rather than the model's own judgment.
//!
//! The file is a small TOML mapping high-level operations to a rule:
//!
//! ```toml
//! [permissions]
//! delete_files      = "always_ask"            # always_ask | always_allow | block
//! multi_file_edit   = "ask_if_gt_5"           # ask when > N files
//! external_network  = "ask_if_new_domain"     # ask on unseen domains
//! terminal_shell    = "ask_if_destructive"    # ask when Guard-1 flags the cmd
//! web_action        = "always_ask"            # checkout/payment/sensitive submit
//! min_confidence_for_auto = 0.85              # below this → ask (auto path)
//! user_feedback_learning = true               # approvals feed taste profile
//! ```
//!
//! Everything is pure + deterministic: `load` parses (unknown keys ignored,
//! malformed → default), `evaluate` maps an operation + context to
//! [`PolicyAction`]::{Allow,Ask,Block}.

use serde::{Deserialize, Serialize};

/// What the policy decides for one action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    /// Run without a human ticket (may still be subject to Guard-1).
    Allow,
    /// Mint a Guard-2 ticket and wait for approval.
    Ask,
    /// Refuse outright.
    Block,
}

/// The high-level operation classes the policy distinguishes. These are the
/// canonical names the executor tags an action with before calling
/// [`PermissionsPolicy::evaluate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Delete file(s)/dir(s).
    DeleteFiles,
    /// Edit more than one file in a single step.
    MultiFileEdit { files: usize },
    /// Network request to a (possibly new) domain.
    ExternalNetwork { new_domain: bool },
    /// Shell command (Guard-1 flag carried in `destructive`).
    TerminalShell { destructive: bool },
    /// Sensitive web action (checkout/payment/account change).
    WebAction,
    /// Any other privileged mutation (write, exec, …).
    GenericWrite,
    /// P51.20 — a read of a *sensitive* location (vault/credential dirs,
    /// dotfile secrets, browser session data). Plain reads stay executor-
    /// auto-approved (there is no policy gate for them); this variant is the
    /// approve-then-read surface the agent path emits when the target path
    /// resolves inside a sensitive scope. `sensitive: false` is the identity
    /// of a plain read (always Allow).
    Read { sensitive: bool },
}

impl Operation {
    /// The canonical name (used by the TOML key + the ticket `operation`).
    pub fn name(&self) -> &'static str {
        match self {
            Operation::DeleteFiles => "delete",
            Operation::MultiFileEdit { .. } => "multi_file_edit",
            Operation::ExternalNetwork { .. } => "external_network",
            Operation::TerminalShell { .. } => "terminal_shell",
            Operation::WebAction => "web_action",
            Operation::GenericWrite => "write",
            Operation::Read { .. } => "read",
        }
    }
}

/// A parsed rule (the value of a `permissions` key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    AlwaysAsk,
    AlwaysAllow,
    Block,
    /// `ask_if_gt_N`: ask only when the count exceeds N.
    AskIfGt(u32),
    /// `ask_if_new_domain`: ask only when the domain is unseen.
    AskIfNewDomain,
    /// `ask_if_destructive`: ask only when the command is destructive.
    AskIfDestructive,
}

/// Canonical TOML string for a rule (the inverse of [`Rule::parse`]).
fn rule_str(r: Rule) -> String {
    match r {
        Rule::AlwaysAsk => "always_ask".into(),
        Rule::AlwaysAllow => "always_allow".into(),
        Rule::Block => "block".into(),
        Rule::AskIfGt(n) => format!("ask_if_gt_{n}"),
        Rule::AskIfNewDomain => "ask_if_new_domain".into(),
        Rule::AskIfDestructive => "ask_if_destructive".into(),
    }
}

impl Rule {
    fn parse(s: &str) -> Option<Rule> {
        let t = s.trim();
        Some(match t {
            "always_ask" | "ask" => Rule::AlwaysAsk,
            "always_allow" | "allow" => Rule::AlwaysAllow,
            "block" | "deny" => Rule::Block,
            "ask_if_new_domain" => Rule::AskIfNewDomain,
            "ask_if_destructive" => Rule::AskIfDestructive,
            _ => {
                // ask_if_gt_N
                let n = t.strip_prefix("ask_if_gt_")?;
                return Some(Rule::AskIfGt(n.parse().ok()?));
            }
        })
    }

    fn evaluate(self, op: &Operation) -> PolicyAction {
        match (self, op) {
            (Rule::AlwaysAsk, _) => PolicyAction::Ask,
            (Rule::AlwaysAllow, _) => PolicyAction::Allow,
            (Rule::Block, _) => PolicyAction::Block,
            (Rule::AskIfGt(n), Operation::MultiFileEdit { files }) => {
                if *files > n as usize {
                    PolicyAction::Ask
                } else {
                    PolicyAction::Allow
                }
            }
            (Rule::AskIfGt(_), _) => PolicyAction::Allow,
            (Rule::AskIfNewDomain, Operation::ExternalNetwork { new_domain }) => {
                if *new_domain {
                    PolicyAction::Ask
                } else {
                    PolicyAction::Allow
                }
            }
            (Rule::AskIfNewDomain, _) => PolicyAction::Allow,
            (Rule::AskIfDestructive, Operation::TerminalShell { destructive }) => {
                if *destructive {
                    PolicyAction::Ask
                } else {
                    PolicyAction::Allow
                }
            }
            (Rule::AskIfDestructive, _) => PolicyAction::Allow,
        }
    }
}

/// Raw TOML shape (serde deserializes the `permissions` table).
#[derive(Debug, Clone, Default, Deserialize)]
struct RawPermissions {
    #[serde(default)]
    permissions: RawTable,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawTable {
    delete_files: Option<String>,
    multi_file_edit: Option<String>,
    external_network: Option<String>,
    terminal_shell: Option<String>,
    web_action: Option<String>,
    write: Option<String>,
    /// P51.20 — approve-then-read rule for reads of sensitive scopes
    /// (vault/credential dirs, dotfile secrets, browser session data).
    read_sensitive: Option<String>,
    min_confidence_for_auto: Option<f64>,
    #[serde(default)]
    user_feedback_learning: Option<bool>,
}

/// The evaluated, ready-to-consult policy.
#[derive(Debug, Clone)]
pub struct PermissionsPolicy {
    delete_files: Rule,
    multi_file_edit: Rule,
    external_network: Rule,
    terminal_shell: Rule,
    web_action: Rule,
    write: Rule,
    /// P51.20 — approve-then-read: sensitive-scope reads (vault/credential
    /// dirs, dotfile secrets, browser session data) resolve against this
    /// rule. Default AlwaysAsk — the approve-then-read contract. Plain
    /// reads never consult it (they are executor-auto-approved).
    read_sensitive: Rule,
    /// Below this confidence the auto path must ask.
    pub min_confidence_for_auto: f64,
    /// Approvals/denials feed the taste profile.
    pub user_feedback_learning: bool,
}

impl Default for PermissionsPolicy {
    fn default() -> Self {
        Self {
            delete_files: Rule::AlwaysAsk,
            multi_file_edit: Rule::AskIfGt(5),
            external_network: Rule::AskIfNewDomain,
            terminal_shell: Rule::AskIfDestructive,
            web_action: Rule::AlwaysAsk,
            write: Rule::AlwaysAsk,
            read_sensitive: Rule::AlwaysAsk,
            min_confidence_for_auto: 0.85,
            user_feedback_learning: true,
        }
    }
}

/// P44.5 — the H34 autonomy levels as `permissions.toml` presets. Each level
/// is a fixed rule map over the landed `PermissionsPolicy` (plus a
/// `min_confidence_for_auto`); the *hard floors* (destructive, secret/
/// credential access, financial, security changes, cross-workspace writes,
/// irreversible external effects) stay Ask/Block in every preset — a preset
/// is never a bypass around Guard-1/Guard-2, only a knob over who must
/// approve which class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyPreset {
    /// 🛡 plan + read-only — every mutation denied.
    Sandbox,
    /// 👀 default — safe reads auto; meaningful mutations + external ask;
    /// destructive always ask.
    Ask,
    /// ⚡ low-risk mutations auto (workspace edits, dir create/rename, local
    /// tests, format, generated artifacts); external sends/money/destructive/
    /// new domains/credentials/high-risk shell/scope expansion still ask.
    Auto,
    /// 🚀 maximum autonomy within the hard floors — never bypasses
    /// destructive, secrets/credentials, financial, security changes,
    /// cross-workspace writes, irreversible external effects.
    Maximum,
}

impl AutonomyPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Ask => "ask",
            Self::Auto => "auto",
            Self::Maximum => "maximum",
        }
    }

    pub fn parse(s: &str) -> Option<AutonomyPreset> {
        Some(match s.to_lowercase().as_str() {
            "sandbox" | "plan" | "read_only" => AutonomyPreset::Sandbox,
            "ask" | "interactive" | "default" => AutonomyPreset::Ask,
            "auto" => AutonomyPreset::Auto,
            "maximum" | "run_everything" | "max" | "custom" => AutonomyPreset::Maximum,
            _ => return None,
        })
    }

    /// The autonomy-gradient mode this preset drives (H34 rides the landed
    /// `agentcowork-guard::autonomy` gradient, so the calculator stays one
    /// knob).
    pub fn to_mode(self) -> crate::autonomy::Mode {
        match self {
            Self::Sandbox => crate::autonomy::Mode::Plan,
            Self::Ask => crate::autonomy::Mode::Interactive,
            Self::Auto => crate::autonomy::Mode::Auto,
            Self::Maximum => crate::autonomy::Mode::Custom,
        }
    }
}

impl PermissionsPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// P44.5 — build the `permissions.toml` preset for an H34 level. The map
    /// is explicit per rule (not a delta), so a preset is a complete,
    /// inspectable policy. The floors are hard: destructive/delete, web
    /// (financial/account), new-domain external, and destructive shell stay
    /// Ask-or-worse in every level.
    pub fn preset(level: AutonomyPreset) -> Self {
        match level {
            AutonomyPreset::Sandbox => Self {
                delete_files: Rule::Block,
                multi_file_edit: Rule::Block,
                external_network: Rule::Block,
                terminal_shell: Rule::Block,
                web_action: Rule::Block,
                write: Rule::Block,
                read_sensitive: Rule::Block,
                min_confidence_for_auto: 1.0,
                user_feedback_learning: false,
            },
            AutonomyPreset::Ask => Self {
                delete_files: Rule::AlwaysAsk,
                multi_file_edit: Rule::AskIfGt(5),
                external_network: Rule::AskIfNewDomain,
                terminal_shell: Rule::AskIfDestructive,
                web_action: Rule::AlwaysAsk,
                write: Rule::AlwaysAsk,
                read_sensitive: Rule::AlwaysAsk,
                min_confidence_for_auto: 0.85,
                user_feedback_learning: true,
            },
            AutonomyPreset::Auto => Self {
                // Low-risk local mutations run; anything with a floor asks.
                delete_files: Rule::AlwaysAsk,
                multi_file_edit: Rule::AlwaysAllow,
                external_network: Rule::AskIfNewDomain,
                terminal_shell: Rule::AskIfDestructive,
                web_action: Rule::AlwaysAsk,
                write: Rule::AlwaysAllow,
                read_sensitive: Rule::AlwaysAsk,
                min_confidence_for_auto: 0.75,
                user_feedback_learning: true,
            },
            AutonomyPreset::Maximum => Self {
                // Everything local runs; the floors never move.
                delete_files: Rule::AlwaysAsk,
                multi_file_edit: Rule::AlwaysAllow,
                external_network: Rule::AskIfNewDomain,
                terminal_shell: Rule::AskIfDestructive,
                web_action: Rule::AlwaysAsk,
                write: Rule::AlwaysAllow,
                read_sensitive: Rule::AlwaysAsk,
                min_confidence_for_auto: 0.6,
                user_feedback_learning: true,
            },
        }
    }

    /// Render the preset as an actual `[permissions]` TOML block — the file
    /// the user would write, guaranteed to round-trip through [`Self::parse`].
    pub fn preset_toml(level: AutonomyPreset) -> String {
        let p = Self::preset(level);
        format!(
            "# AgentCowork autonomy preset: {name}\n\
             [permissions]\n\
             delete_files = \"{delete}\"\n\
             multi_file_edit = \"{multi}\"\n\
             external_network = \"{ext}\"\n\
             terminal_shell = \"{shell}\"\n\
             web_action = \"{web}\"\n\
             write = \"{write}\"\n\
             read_sensitive = \"{read_sensitive}\"\n\
             min_confidence_for_auto = {conf}\n\
             user_feedback_learning = {feedback}\n",
            name = level.as_str(),
            delete = rule_str(p.delete_files),
            multi = rule_str(p.multi_file_edit),
            ext = rule_str(p.external_network),
            shell = rule_str(p.terminal_shell),
            web = rule_str(p.web_action),
            write = rule_str(p.write),
            read_sensitive = rule_str(p.read_sensitive),
            conf = p.min_confidence_for_auto,
            feedback = p.user_feedback_learning,
        )
    }

    /// Parse a `permissions.toml` document. Unknown keys and malformed rule
    /// strings fall back to the default (never panic, never over-grant).
    pub fn parse(doc: &str) -> Self {
        let Ok(raw) = toml::from_str::<RawPermissions>(doc) else {
            return Self::default();
        };
        let mut p = Self::default();
        let t = &raw.permissions;
        if let Some(r) = t.delete_files.as_deref().and_then(Rule::parse) {
            p.delete_files = r;
        }
        if let Some(r) = t.multi_file_edit.as_deref().and_then(Rule::parse) {
            p.multi_file_edit = r;
        }
        if let Some(r) = t.external_network.as_deref().and_then(Rule::parse) {
            p.external_network = r;
        }
        if let Some(r) = t.terminal_shell.as_deref().and_then(Rule::parse) {
            p.terminal_shell = r;
        }
        if let Some(r) = t.web_action.as_deref().and_then(Rule::parse) {
            p.web_action = r;
        }
        if let Some(r) = t.write.as_deref().and_then(Rule::parse) {
            p.write = r;
        }
        if let Some(r) = t.read_sensitive.as_deref().and_then(Rule::parse) {
            p.read_sensitive = r;
        }
        if let Some(m) = t.min_confidence_for_auto {
            p.min_confidence_for_auto = m.clamp(0.0, 1.0);
        }
        if let Some(u) = t.user_feedback_learning {
            p.user_feedback_learning = u;
        }
        p
    }

    /// Is this policy exactly one of the H34 presets? (Used by the autonomy
    /// indicator to name the level currently applied.)
    pub fn is_preset(&self, level: AutonomyPreset) -> bool {
        let p = Self::preset(level);
        p.delete_files == self.delete_files
            && p.multi_file_edit == self.multi_file_edit
            && p.external_network == self.external_network
            && p.terminal_shell == self.terminal_shell
            && p.web_action == self.web_action
            && p.write == self.write
            && p.read_sensitive == self.read_sensitive
            && (p.min_confidence_for_auto - self.min_confidence_for_auto).abs() < f64::EPSILON
    }

    /// P51.20 — apply a named rule bundle (a one-click combo). Keys are the
    /// canonical permission keys; unknown keys and unparseable rules are
    /// refused (never silently over-grant). Returns the number of rules
    /// applied.
    pub fn apply_rules(&mut self, rules: &[(&str, &str)]) -> Result<usize, String> {
        let mut applied = 0usize;
        for (key, rule) in rules {
            let parsed = Rule::parse(rule).ok_or_else(|| format!("unknown rule: {rule}"))?;
            match *key {
                "delete_files" => self.delete_files = parsed,
                "multi_file_edit" => self.multi_file_edit = parsed,
                "external_network" => self.external_network = parsed,
                "terminal_shell" => self.terminal_shell = parsed,
                "web_action" => self.web_action = parsed,
                "write" => self.write = parsed,
                "read_sensitive" => self.read_sensitive = parsed,
                other => return Err(format!("unknown permission key: {other}")),
            }
            applied += 1;
        }
        Ok(applied)
    }

    /// Evaluate one operation under the policy. This is the executor's
    /// pre-flight: `Ask` means mint a ticket; `Block` means refuse; `Allow`
    /// means run (still subject to Guard-1).
    pub fn evaluate(&self, op: &Operation) -> PolicyAction {
        match op {
            Operation::DeleteFiles => self.delete_files.evaluate(op),
            Operation::MultiFileEdit { .. } => self.multi_file_edit.evaluate(op),
            Operation::ExternalNetwork { .. } => self.external_network.evaluate(op),
            Operation::TerminalShell { .. } => self.terminal_shell.evaluate(op),
            Operation::WebAction => self.web_action.evaluate(op),
            Operation::GenericWrite => self.write.evaluate(op),
            // P51.20 — plain reads are executor-auto-approved (no policy
            // gate); only sensitive-scope reads resolve against the
            // approve-then-read rule.
            Operation::Read { sensitive: false } => PolicyAction::Allow,
            Operation::Read { sensitive: true } => self.read_sensitive.evaluate(op),
        }
    }

    /// The auto path must ask when a model's reported confidence is below
    /// this threshold.
    pub fn auto_confidence_ok(&self, confidence: f64) -> bool {
        confidence >= self.min_confidence_for_auto
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let p = PermissionsPolicy::default();
        assert_eq!(p.evaluate(&Operation::DeleteFiles), PolicyAction::Ask);
        assert_eq!(
            p.evaluate(&Operation::MultiFileEdit { files: 6 }),
            PolicyAction::Ask
        );
        assert_eq!(
            p.evaluate(&Operation::MultiFileEdit { files: 5 }),
            PolicyAction::Allow
        );
        assert_eq!(
            p.evaluate(&Operation::ExternalNetwork { new_domain: true }),
            PolicyAction::Ask
        );
        assert_eq!(
            p.evaluate(&Operation::ExternalNetwork { new_domain: false }),
            PolicyAction::Allow
        );
        assert_eq!(
            p.evaluate(&Operation::TerminalShell { destructive: true }),
            PolicyAction::Ask
        );
        assert_eq!(
            p.evaluate(&Operation::TerminalShell { destructive: false }),
            PolicyAction::Allow
        );
        assert_eq!(p.evaluate(&Operation::WebAction), PolicyAction::Ask);
        assert_eq!(p.min_confidence_for_auto, 0.85);
        assert!(p.user_feedback_learning);
    }

    #[test]
    fn parses_full_toml() {
        let doc = r#"
[permissions]
delete_files = "always_ask"
multi_file_edit = "ask_if_gt_3"
external_network = "allow"
terminal_shell = "block"
web_action = "always_ask"
min_confidence_for_auto = 0.90
user_feedback_learning = true
"#;
        let p = PermissionsPolicy::parse(doc);
        assert_eq!(p.evaluate(&Operation::DeleteFiles), PolicyAction::Ask);
        assert_eq!(
            p.evaluate(&Operation::MultiFileEdit { files: 4 }),
            PolicyAction::Ask
        );
        assert_eq!(
            p.evaluate(&Operation::MultiFileEdit { files: 3 }),
            PolicyAction::Allow
        );
        assert_eq!(
            p.evaluate(&Operation::ExternalNetwork { new_domain: true }),
            PolicyAction::Allow
        );
        assert_eq!(
            p.evaluate(&Operation::TerminalShell { destructive: true }),
            PolicyAction::Block
        );
        assert_eq!(p.min_confidence_for_auto, 0.90);
    }

    #[test]
    fn malformed_doc_falls_back_to_defaults() {
        let p = PermissionsPolicy::parse("not valid [ toml");
        assert_eq!(p.evaluate(&Operation::DeleteFiles), PolicyAction::Ask);
    }

    #[test]
    fn unknown_rule_string_falls_back_to_default() {
        let p = PermissionsPolicy::parse("[permissions]\ndelete_files = \"maybe_sometimes\"\n");
        // Unknown → keeps default always_ask.
        assert_eq!(p.evaluate(&Operation::DeleteFiles), PolicyAction::Ask);
    }

    #[test]
    fn confidence_threshold() {
        let p = PermissionsPolicy::default();
        assert!(p.auto_confidence_ok(0.86));
        assert!(!p.auto_confidence_ok(0.84));
    }

    #[test]
    fn sandbox_preset_denies_every_mutation() {
        let p = PermissionsPolicy::preset(AutonomyPreset::Sandbox);
        assert_eq!(p.evaluate(&Operation::DeleteFiles), PolicyAction::Block);
        assert_eq!(
            p.evaluate(&Operation::MultiFileEdit { files: 1 }),
            PolicyAction::Block
        );
        assert_eq!(
            p.evaluate(&Operation::ExternalNetwork { new_domain: false }),
            PolicyAction::Block
        );
        assert_eq!(
            p.evaluate(&Operation::TerminalShell { destructive: false }),
            PolicyAction::Block
        );
        assert_eq!(p.evaluate(&Operation::WebAction), PolicyAction::Block);
        assert_eq!(p.evaluate(&Operation::GenericWrite), PolicyAction::Block);
        assert_eq!(p.min_confidence_for_auto, 1.0);
    }

    #[test]
    fn ask_preset_matches_default_policy() {
        let p = PermissionsPolicy::preset(AutonomyPreset::Ask);
        assert_eq!(p.evaluate(&Operation::DeleteFiles), PolicyAction::Ask);
        assert_eq!(
            p.evaluate(&Operation::ExternalNetwork { new_domain: true }),
            PolicyAction::Ask
        );
        assert_eq!(
            p.evaluate(&Operation::TerminalShell { destructive: false }),
            PolicyAction::Allow
        );
        assert_eq!(p.min_confidence_for_auto, 0.85);
    }

    #[test]
    fn auto_preset_runs_low_risk_keeps_floors() {
        let p = PermissionsPolicy::preset(AutonomyPreset::Auto);
        // Low-risk local mutations auto.
        assert_eq!(p.evaluate(&Operation::GenericWrite), PolicyAction::Allow);
        assert_eq!(
            p.evaluate(&Operation::MultiFileEdit { files: 9 }),
            PolicyAction::Allow
        );
        // Floors stay.
        assert_eq!(p.evaluate(&Operation::DeleteFiles), PolicyAction::Ask);
        assert_eq!(p.evaluate(&Operation::WebAction), PolicyAction::Ask);
        assert_eq!(
            p.evaluate(&Operation::ExternalNetwork { new_domain: true }),
            PolicyAction::Ask
        );
        assert_eq!(
            p.evaluate(&Operation::TerminalShell { destructive: true }),
            PolicyAction::Ask
        );
        assert_eq!(p.min_confidence_for_auto, 0.75);
    }

    #[test]
    fn maximum_preset_is_auto_within_hard_floors() {
        let p = PermissionsPolicy::preset(AutonomyPreset::Maximum);
        assert_eq!(p.evaluate(&Operation::GenericWrite), PolicyAction::Allow);
        assert_eq!(
            p.evaluate(&Operation::MultiFileEdit { files: 99 }),
            PolicyAction::Allow
        );
        // Hard floors never move: destructive / financial / new-domain / high
        // -risk shell still ask.
        assert_eq!(p.evaluate(&Operation::DeleteFiles), PolicyAction::Ask);
        assert_eq!(p.evaluate(&Operation::WebAction), PolicyAction::Ask);
        assert_eq!(
            p.evaluate(&Operation::ExternalNetwork { new_domain: true }),
            PolicyAction::Ask
        );
        assert_eq!(
            p.evaluate(&Operation::TerminalShell { destructive: true }),
            PolicyAction::Ask
        );
        assert_eq!(p.min_confidence_for_auto, 0.6);
    }

    #[test]
    fn preset_toml_round_trips_through_parse() {
        for level in [
            AutonomyPreset::Sandbox,
            AutonomyPreset::Ask,
            AutonomyPreset::Auto,
            AutonomyPreset::Maximum,
        ] {
            let toml = PermissionsPolicy::preset_toml(level);
            let reparsed = PermissionsPolicy::parse(&toml);
            let original = PermissionsPolicy::preset(level);
            for op in [
                Operation::DeleteFiles,
                Operation::MultiFileEdit { files: 1 },
                Operation::MultiFileEdit { files: 9 },
                Operation::ExternalNetwork { new_domain: true },
                Operation::ExternalNetwork { new_domain: false },
                Operation::TerminalShell { destructive: true },
                Operation::TerminalShell { destructive: false },
                Operation::WebAction,
                Operation::GenericWrite,
            ] {
                assert_eq!(
                    reparsed.evaluate(&op),
                    original.evaluate(&op),
                    "preset {level:?} diverged on {op:?}"
                );
            }
            assert_eq!(
                reparsed.min_confidence_for_auto,
                original.min_confidence_for_auto
            );
        }
    }

    #[test]
    fn preset_maps_to_autonomy_gradient_modes() {
        assert_eq!(
            AutonomyPreset::Sandbox.to_mode(),
            crate::autonomy::Mode::Plan
        );
        assert_eq!(
            AutonomyPreset::Ask.to_mode(),
            crate::autonomy::Mode::Interactive
        );
        assert_eq!(AutonomyPreset::Auto.to_mode(), crate::autonomy::Mode::Auto);
        assert_eq!(
            AutonomyPreset::Maximum.to_mode(),
            crate::autonomy::Mode::Custom
        );
    }
}
