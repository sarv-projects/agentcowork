//! Automation tool shapes (P6.4 — doc 63 §4.12, khoj pattern).
//!
//! First-class automation steps: `run_code` (sandboxed exec via
//! `agentcowork-script`), `online_search` (the G8 cascade), plus email/calendar
//! triggers where the connectors exist (F14/F15). These are the *shapes* —
//! the Work factory (`agentcowork-core::automation_runtime::compile_work`,
//! P71.3c) compiles them into Work + capability requests; the Work kernel
//! executes. The automation layer never runs them.

use serde::{Deserialize, Serialize};

/// One step in an automation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum AutomationStep {
    /// Run code in a sandbox (agentcowork-script).
    RunCode {
        /// The language/runtime hint (js, ts, py, sh).
        language: String,
        code: String,
    },
    /// Search the web via the G8 cascade (local index → cached web → live).
    OnlineSearch { query: String },
    /// Send an email (F14 connector) — approval-gated at runtime.
    Email {
        to: Vec<String>,
        subject: String,
        body: String,
    },
    /// Create a calendar event (F15 connector).
    Calendar { title: String, when: String },
}

/// A trigger condition for an automation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "trigger", rename_all = "snake_case")]
pub enum Trigger {
    /// On a schedule (cron-style string).
    Schedule { cron: String },
    /// When a file/dir event fires (created/modified/deleted).
    FileEvent { path: String, event: String },
    /// When an email arrives matching a filter (F14).
    IncomingEmail { filter: String },
    /// Manual/one-shot.
    Manual,
}

/// A complete automation: a trigger + steps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Automation {
    pub id: String,
    pub name: String,
    pub trigger: Trigger,
    pub steps: Vec<AutomationStep>,
}

impl Automation {
    pub fn new(id: impl Into<String>, name: impl Into<String>, trigger: Trigger) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            trigger,
            steps: Vec::new(),
        }
    }

    pub fn step(mut self, step: AutomationStep) -> Self {
        self.steps.push(step);
        self
    }

    /// Email/calendar steps are privileged — the executor must approval-gate
    /// them. Surface them here so the gate can be enforced mechanically.
    pub fn privileged_steps(&self) -> Vec<&AutomationStep> {
        self.steps
            .iter()
            .filter(|s| {
                matches!(
                    s,
                    AutomationStep::Email { .. } | AutomationStep::Calendar { .. }
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_roundtrips_serde() {
        let a = Automation::new(
            "a1",
            "Morning brief",
            Trigger::Schedule {
                cron: "0 8 * * *".into(),
            },
        )
        .step(AutomationStep::OnlineSearch {
            query: "latest AI news".into(),
        })
        .step(AutomationStep::RunCode {
            language: "js".into(),
            code: "return 42".into(),
        });
        let json = serde_json::to_string(&a).unwrap();
        let back: Automation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn privileged_steps_are_surfaced() {
        let a = Automation::new("a2", "Send report", Trigger::Manual)
            .step(AutomationStep::RunCode {
                language: "js".into(),
                code: "x".into(),
            })
            .step(AutomationStep::Email {
                to: vec!["bob@x.test".into()],
                subject: "s".into(),
                body: "b".into(),
            });
        assert_eq!(a.privileged_steps().len(), 1);
    }

    #[test]
    fn run_code_is_not_privileged() {
        let a = Automation::new("a3", "Calc", Trigger::Manual).step(AutomationStep::RunCode {
            language: "js".into(),
            code: "x".into(),
        });
        assert!(a.privileged_steps().is_empty());
    }
}
