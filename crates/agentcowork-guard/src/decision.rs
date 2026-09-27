//! P7.5 / J21 — the structured **decision package** (doc 52 §2): the bundle a
//! lower tier passes up when it escalates, rendered as the Guard-2 card. It
//! carries everything the user needs to judge an action at a glance —
//! goal, the proposed diff, risk, affected paths, and (for shell/web actions)
//! the exact lines/env/network destinations — so approval is informed, never
//! a blind "yes".

use crate::ticket::RiskLevel;
use serde::{Deserialize, Serialize};

/// Sensitive web-action classes that must render a confirm dialog (J3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebActionKind {
    /// Cart / purchase / subscription.
    Checkout,
    /// Card / wallet / transfer.
    Payment,
    /// Password / email / account settings change.
    AccountChange,
    /// Any other sensitive form submit (address, legal, delete-account).
    SensitiveSubmit,
}

impl WebActionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            WebActionKind::Checkout => "checkout",
            WebActionKind::Payment => "payment",
            WebActionKind::AccountChange => "account_change",
            WebActionKind::SensitiveSubmit => "sensitive_submit",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "checkout" => WebActionKind::Checkout,
            "payment" => WebActionKind::Payment,
            "account_change" => WebActionKind::AccountChange,
            "sensitive_submit" => WebActionKind::SensitiveSubmit,
            _ => return None,
        })
    }
}

/// The escalation bundle (doc 52 §2 + J3 "show exactly what"). Every field is
/// optional so a caller can attach only what it knows; the card renders what
/// is present.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionPackage {
    /// One-sentence goal the agent is trying to achieve.
    #[serde(default)]
    pub goal: String,
    /// The proposed change (unified diff text / patch / summary).
    #[serde(default)]
    pub proposed_diff: String,
    /// Risk tier for the card's chip.
    #[serde(default)]
    pub risk: RiskLevel,
    /// Exact files the action touches.
    #[serde(default)]
    pub affected_paths: Vec<String>,
    /// Exact shell script lines (the command / script body).
    #[serde(default)]
    pub script_lines: Vec<String>,
    /// Execution target (binary / interpreter + args).
    #[serde(default)]
    pub execution_target: String,
    /// Environment variables that will be set/visible.
    #[serde(default)]
    pub env_vars: Vec<String>,
    /// Network destinations (hosts / URLs) the action contacts.
    #[serde(default)]
    pub network_destinations: Vec<String>,
    /// Sensitive web action, when this is a browser mutation.
    #[serde(default)]
    pub web_action: Option<WebActionKind>,
    /// Model's reported confidence (0..1) for the auto path.
    #[serde(default)]
    pub confidence: Option<f64>,
}

impl DecisionPackage {
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            goal: goal.into(),
            ..Self::default()
        }
    }

    /// Builder helpers — chained, so call-sites stay readable.
    pub fn with_diff(mut self, diff: impl Into<String>) -> Self {
        self.proposed_diff = diff.into();
        self
    }

    pub fn with_risk(mut self, risk: RiskLevel) -> Self {
        self.risk = risk;
        self
    }

    pub fn with_paths(mut self, paths: Vec<String>) -> Self {
        self.affected_paths = paths;
        self
    }

    pub fn with_script(mut self, lines: Vec<String>, target: impl Into<String>) -> Self {
        self.script_lines = lines;
        self.execution_target = target.into();
        self
    }

    pub fn with_env(mut self, env: Vec<String>) -> Self {
        self.env_vars = env;
        self
    }

    pub fn with_network(mut self, dests: Vec<String>) -> Self {
        self.network_destinations = dests;
        self
    }

    pub fn with_web_action(mut self, kind: WebActionKind) -> Self {
        self.web_action = Some(kind);
        self
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = Some(confidence);
        self
    }

    /// Is this a sensitive web action (must render a confirm dialog, J3)?
    pub fn is_web_action(&self) -> bool {
        self.web_action.is_some()
    }

    /// One-line human summary for the card header.
    pub fn summary(&self) -> String {
        if self.goal.is_empty() {
            return "(no goal)".to_string();
        }
        self.goal.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_compose() {
        let p = DecisionPackage::new("Fix the Q3 budget")
            .with_diff("- cell C2 4500\n+ cell C2 4200")
            .with_risk(RiskLevel::High)
            .with_paths(vec!["/workspace/Q3-Budget.xlsx".into()])
            .with_network(vec!["api.example.com".into()])
            .with_web_action(WebActionKind::Payment)
            .with_confidence(0.92);

        assert_eq!(p.summary(), "Fix the Q3 budget");
        assert_eq!(p.risk, RiskLevel::High);
        assert_eq!(p.affected_paths.len(), 1);
        assert!(p.is_web_action());
        assert_eq!(p.web_action, Some(WebActionKind::Payment));
        assert_eq!(p.confidence, Some(0.92));
    }

    #[test]
    fn web_action_kind_roundtrips() {
        assert_eq!(
            WebActionKind::parse("checkout"),
            Some(WebActionKind::Checkout)
        );
        assert_eq!(WebActionKind::parse("nope"), None);
        assert_eq!(WebActionKind::Payment.as_str(), "payment");
    }

    #[test]
    fn decision_package_serializes() {
        let p = DecisionPackage::new("g").with_script(vec!["rm -rf x".into()], "/bin/sh");
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["goal"], "g");
        assert_eq!(v["scriptLines"][0], "rm -rf x");
    }
}
