//! P6 loop-pattern registry (doc 65 §9 — loop-engineering steal): named
//! loop patterns — `budget-guard`, `run-log`, `early-exit` — each with
//! `triggers` / `guards` / `exit_conditions` expressed as concrete signals.
//! The coordinator loop loads the registry, and each turn evaluates the
//! current [`LoopSnapshot`] against every pattern; an engaged pattern's
//! guards are enforced by the J11 efficiency metrics + B6 iteration budgets.
//!
//! Pure and deterministic — the loop feeds a snapshot, the registry answers
//! which patterns are engaged and what each one demands.

use serde::{Deserialize, Serialize};

/// The numeric facts of the current loop the patterns evaluate against.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LoopSnapshot {
    /// Turns used out of the iteration budget (fraction 0..=1).
    pub budget_used: f64,
    /// Times the same tool-call sequence has repeated (0 = none yet).
    pub repeat_count: u32,
    /// Est. USD spent per successful edit this run (J11).
    pub cost_per_edit_usd: f64,
    /// J11 one-shot rate (0..=1).
    pub one_shot_rate: f64,
    /// Turns since the last verified progress (diff/artifact/commit).
    pub turns_since_progress: u32,
    /// Whether verification reported the task complete.
    pub verified_complete: bool,
}

/// A condition a pattern evaluates. All thresholds are data — the registry
/// is declarative, not code.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// The turn budget has crossed this fraction.
    BudgetUsedAbove { fraction: f64 },
    /// The same sequence repeated ≥ this many times.
    RepeatsAbove { count: u32 },
    /// Cost per successful edit exceeded this (USD).
    CostPerEditAbove { usd: f64 },
    /// One-shot rate fell below this threshold.
    OneShotRateBelow { threshold: f64 },
    /// No verified progress for ≥ this many turns.
    NoProgressFor { turns: u32 },
    /// The verifier reported completion.
    VerifiedComplete,
}

impl Condition {
    fn holds(&self, s: &LoopSnapshot) -> bool {
        match *self {
            Condition::BudgetUsedAbove { fraction } => s.budget_used >= fraction,
            Condition::RepeatsAbove { count } => s.repeat_count >= count,
            Condition::CostPerEditAbove { usd } => s.cost_per_edit_usd > usd,
            Condition::OneShotRateBelow { threshold } => {
                s.one_shot_rate > 0.0 && s.one_shot_rate < threshold
            }
            Condition::NoProgressFor { turns } => s.turns_since_progress >= turns,
            Condition::VerifiedComplete => s.verified_complete,
        }
    }
}

/// One named loop pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopPattern {
    pub id: String,
    pub description: String,
    /// Any trigger engaging → the pattern is active.
    #[serde(default)]
    pub triggers: Vec<Condition>,
    /// All guards must hold while the pattern is active; a violated guard is
    /// a hard interrupt (the loop must change course).
    #[serde(default)]
    pub guards: Vec<Condition>,
    /// Any exit condition → the loop may end cleanly.
    #[serde(default)]
    pub exit_conditions: Vec<Condition>,
}

/// The registry of named patterns, loaded by the coordinator loop.
#[derive(Debug, Clone, Default)]
pub struct LoopPatternRegistry {
    patterns: Vec<LoopPattern>,
}

impl LoopPatternRegistry {
    pub fn new(patterns: Vec<LoopPattern>) -> Self {
        Self { patterns }
    }

    /// The built-in registry (budget-guard / run-log / early-exit).
    pub fn builtin() -> Self {
        Self::new(vec![
            LoopPattern {
                id: "budget-guard".into(),
                description: "Slow the loop before the budget runs out; interrupt on runaway cost."
                    .into(),
                triggers: vec![Condition::BudgetUsedAbove { fraction: 0.8 }],
                guards: vec![
                    Condition::CostPerEditAbove { usd: 0.02 },
                    Condition::OneShotRateBelow { threshold: 0.2 },
                ],
                exit_conditions: vec![Condition::VerifiedComplete],
            },
            LoopPattern {
                id: "run-log".into(),
                description: "Detect a repeating sequence and force divergence or escalation."
                    .into(),
                triggers: vec![Condition::RepeatsAbove { count: 3 }],
                guards: vec![Condition::NoProgressFor { turns: 4 }],
                exit_conditions: vec![
                    Condition::VerifiedComplete,
                    Condition::BudgetUsedAbove { fraction: 1.0 },
                ],
            },
            LoopPattern {
                id: "early-exit".into(),
                description: "Stop as soon as verification passes — never burn turns after done."
                    .into(),
                triggers: vec![Condition::VerifiedComplete],
                guards: vec![],
                exit_conditions: vec![Condition::VerifiedComplete],
            },
        ])
    }

    pub fn all(&self) -> &[LoopPattern] {
        &self.patterns
    }

    /// Patterns engaged by the snapshot, in registry order.
    pub fn engaged(&self, s: &LoopSnapshot) -> Vec<&LoopPattern> {
        self.patterns
            .iter()
            .filter(|p| p.triggers.iter().any(|c| c.holds(s)))
            .collect()
    }

    /// The first violated guard across engaged patterns (the hard interrupt
    /// the loop must act on), if any.
    pub fn violated_guard(&self, s: &LoopSnapshot) -> Option<(&LoopPattern, Condition)> {
        self.engaged(s)
            .into_iter()
            .find_map(|p| p.guards.iter().find(|c| c.holds(s)).map(|c| (p, *c)))
    }

    /// Whether any engaged pattern says the loop may exit cleanly.
    pub fn may_exit(&self, s: &LoopSnapshot) -> bool {
        self.engaged(s)
            .iter()
            .any(|p| p.exit_conditions.iter().any(|c| c.holds(s)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> LoopSnapshot {
        LoopSnapshot::default()
    }

    #[test]
    fn budget_guard_engages_at_threshold() {
        let reg = LoopPatternRegistry::builtin();
        assert!(reg.engaged(&snapshot()).is_empty());
        let s = LoopSnapshot {
            budget_used: 0.85,
            ..snapshot()
        };
        let engaged = reg.engaged(&s);
        assert!(engaged.iter().any(|p| p.id == "budget-guard"));
    }

    #[test]
    fn run_log_trips_on_repeats_without_progress() {
        let reg = LoopPatternRegistry::builtin();
        let s = LoopSnapshot {
            repeat_count: 4,
            turns_since_progress: 6,
            ..snapshot()
        };
        let (p, _c) = reg.violated_guard(&s).unwrap();
        assert_eq!(p.id, "run-log");
    }

    #[test]
    fn early_exit_allows_clean_stop() {
        let reg = LoopPatternRegistry::builtin();
        let s = LoopSnapshot {
            verified_complete: true,
            ..snapshot()
        };
        assert!(reg.may_exit(&s));
        assert!(!reg.may_exit(&snapshot()));
    }

    #[test]
    fn guards_do_not_fire_before_trigger() {
        let reg = LoopPatternRegistry::builtin();
        // Cost over cap but budget untouched — no pattern engaged, no interrupt.
        let s = LoopSnapshot {
            cost_per_edit_usd: 0.5,
            ..snapshot()
        };
        assert!(reg.engaged(&s).is_empty());
        assert!(reg.violated_guard(&s).is_none());
    }
}
