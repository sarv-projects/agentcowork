//! K3 half-2 — teach → compile → deterministic replay (doc 81 §4; Gate D
//! simulator/fixtures first). This module is the **compile contract + the
//! simulator seed**: a [`CompiledDemo`] is a deterministic step list (each
//! step carries its K2 effect class), and the [`Simulator`] runs it against
//! a [`SimulationFixture`] — the Gate D "world" — asserting outcome
//! evidence step by step. **Halt-over-guess** (OpenAdapt pattern, P21): when
//! a step's outcome cannot be decided from the fixture, the simulator halts
//! and reports the step — it never guesses and never lets a model re-run.
//!
//! The recording half (K3 half-1, [`agentcowork-browser::demo`]) produces the
//! anchors/outcomes; the simulator compiles them into a fixture-checkable
//! form. Zero model tokens on healthy runs — the compiled path asserts
//! against the fixture, not the model's claim.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One compiled step (the replay's unit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledStep {
    pub index: u32,
    /// Anchor: role + name (the replay locates by these).
    pub role: String,
    pub name: String,
    pub action: String,
    #[serde(default)]
    pub value: String,
    /// The outcome evidence this step must produce (fixture-checkable).
    pub expected_kind: String,
    pub expected: String,
    /// K2 effect class — recovery knowledge carried into the replay.
    pub effect_class: String,
}

/// The compiled demonstration (deterministic; same fixture → same result).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledDemo {
    pub name: String,
    pub start_url: String,
    pub steps: Vec<CompiledStep>,
}

/// The Gate D fixture: a deterministic "world" the simulator checks against.
/// Element presence + URL + visible text — enough to decide every step's
/// outcome without a model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulationFixture {
    /// element-name → present.
    pub elements: BTreeMap<String, bool>,
    /// The URL the simulator reports (steps can assert on it).
    pub url: String,
    /// Visible text the simulator reports.
    pub text: String,
}

/// The verdict of one compiled step under a fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepVerdict {
    /// Anchor found + outcome evidence holds.
    Pass,
    /// Anchor or outcome evidence missing — the step failed.
    Fail,
    /// The fixture cannot decide (unknown evidence kind) — halt.
    Uncertain,
}

/// The simulator's deterministic run report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulationReport {
    pub passed: usize,
    pub failed: Vec<(usize, String)>,
    /// The step index where the run halted over a guess, if any.
    pub halted_at: Option<usize>,
}

impl SimulationReport {
    /// Healthy run: every step passed, no halt.
    pub fn is_healthy(&self) -> bool {
        self.halted_at.is_none() && self.failed.is_empty()
    }
}

/// The simulator: runs a compiled demo against a fixture, deterministic.
#[derive(Debug, Clone, Default)]
pub struct Simulator;

impl Simulator {
    pub fn run(&self, demo: &CompiledDemo, fixture: &SimulationFixture) -> SimulationReport {
        let mut report = SimulationReport::default();
        for step in &demo.steps {
            let verdict = self.evaluate(step, fixture);
            match verdict {
                StepVerdict::Pass => report.passed += 1,
                StepVerdict::Fail => report.failed.push((
                    step.index as usize,
                    format!("{} '{}'", step.expected_kind, step.expected),
                )),
                StepVerdict::Uncertain => {
                    report.halted_at = Some(step.index as usize);
                    return report; // halt-over-guess: stop, never guess
                }
            }
        }
        report
    }

    fn evaluate(&self, step: &CompiledStep, fixture: &SimulationFixture) -> StepVerdict {
        // Anchor must be present (element named `step.name`).
        if fixture.elements.get(&step.name) != Some(&true) {
            return StepVerdict::Fail;
        }
        match step.expected_kind.as_str() {
            "element_present" => {
                if fixture.elements.get(&step.expected) == Some(&true) {
                    StepVerdict::Pass
                } else {
                    StepVerdict::Fail
                }
            }
            "url_contains" => {
                if fixture.url.contains(&step.expected) {
                    StepVerdict::Pass
                } else {
                    StepVerdict::Fail
                }
            }
            "text_contains" => {
                if fixture.text.contains(&step.expected) {
                    StepVerdict::Pass
                } else {
                    StepVerdict::Fail
                }
            }
            // Unknown evidence kind: the fixture cannot decide → halt.
            _ => StepVerdict::Uncertain,
        }
    }
}

/// The compile function: turn a recorded demo (browser crate anchors) into a
/// [`CompiledDemo`] — the teach→compile boundary. Deterministic.
pub fn compile(name: &str, start_url: &str, steps: &[CompiledStep]) -> CompiledDemo {
    CompiledDemo {
        name: name.into(),
        start_url: start_url.into(),
        steps: steps.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> CompiledDemo {
        compile(
            "search-then-save",
            "https://example.com",
            &[
                CompiledStep {
                    index: 0,
                    role: "textbox".into(),
                    name: "Search".into(),
                    action: "type".into(),
                    value: "x".into(),
                    expected_kind: "element_present".into(),
                    expected: "Results".into(),
                    effect_class: "reversible".into(),
                },
                CompiledStep {
                    index: 1,
                    role: "button".into(),
                    name: "Save".into(),
                    action: "click".into(),
                    value: String::new(),
                    expected_kind: "url_contains".into(),
                    expected: "saved".into(),
                    effect_class: "reversible".into(),
                },
            ],
        )
    }

    #[test]
    fn healthy_run_is_zero_guess() {
        let fixture = SimulationFixture {
            elements: BTreeMap::from([
                ("Search".into(), true),
                ("Save".into(), true),
                ("Results".into(), true),
            ]),
            url: "https://example.com/saved".into(),
            text: String::new(),
        };
        let report = Simulator.run(&demo(), &fixture);
        assert!(report.is_healthy());
        assert_eq!(report.passed, 2);
        assert_eq!(report.halted_at, None);
    }

    #[test]
    fn failing_evidence_is_reported() {
        let fixture = SimulationFixture {
            // Results present (step 0 passes); url diverges → step 1 fails.
            elements: BTreeMap::from([
                ("Search".into(), true),
                ("Save".into(), true),
                ("Results".into(), true),
            ]),
            url: "https://example.com/other".into(),
            text: String::new(),
        };
        let report = Simulator.run(&demo(), &fixture);
        assert!(!report.is_healthy());
        assert_eq!(report.failed.len(), 1); // url_contains 'saved' fails
        assert_eq!(report.failed[0].0, 1);
    }

    #[test]
    fn missing_anchor_fails_the_step() {
        let fixture = SimulationFixture {
            elements: BTreeMap::from([("Search".into(), true), ("Results".into(), true)]), // Save missing
            url: "https://example.com/saved".into(),
            text: String::new(),
        };
        let report = Simulator.run(&demo(), &fixture);
        assert_eq!(report.failed[0].0, 1); // step 1's anchor 'Save' is absent
    }

    #[test]
    fn halt_over_guess_never_guesses() {
        let demo = compile(
            "uncertain",
            "u",
            &[CompiledStep {
                index: 0,
                role: "x".into(),
                name: "A".into(),
                action: "click".into(),
                value: String::new(),
                expected_kind: "model_says_ok".into(),
                expected: "anything".into(),
                effect_class: "uncertain".into(),
            }],
        );
        let fixture = SimulationFixture {
            elements: BTreeMap::from([("A".into(), true)]),
            url: "u".into(),
            text: String::new(),
        };
        let report = Simulator.run(&demo, &fixture);
        assert_eq!(report.halted_at, Some(0));
        assert!(!report.is_healthy());
    }
}
