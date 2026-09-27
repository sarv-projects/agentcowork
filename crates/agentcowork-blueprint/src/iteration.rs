//! Iteration budgets + loop detection + circuit-break (P6.3 — doc 16 Hermes
//! `iteration_budget.py` 500/50 + DeerFlow `subagent_limit_middleware`).
//!
//! - [`IterationBudget`] — parent 500 / sub-agent 50 turn caps, with
//!   **execute-code refund** (deterministic code never counts against the
//!   budget).
//! - [`LoopDetector`] — hashes the last `N` tool calls; the same sequence
//!   repeating `threshold` (default 3) times trips an interrupt.
//! - [`TimeoutPolicy`] — sub-agent timeouts (900s custom / 1800s global).
//! - [`CircuitBreak`] — the MCQ interrupt card the UI renders (Skip / Retry /
//!   Escalate / TakeOver), driven by [`CircuitBreaker`].
//!
//! Depth (2) + concurrency (3) + total (6) caps already live in
//! [`crate::subagent::SubAgentLimits`] — this module adds the *turn* budget
//! and the *repeat* detector on top.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use thiserror::Error;

/// Hermes defaults: parent 500 turns, sub-agents 50.
pub const PARENT_MAX_ITERATIONS: u32 = 500;
pub const SUBAGENT_MAX_ITERATIONS: u32 = 50;
/// Sub-agent timeout defaults (DeerFlow): 900s custom / 1800s global.
pub const SUBAGENT_TIMEOUT_CUSTOM_SECS: u64 = 900;
pub const SUBAGENT_TIMEOUT_GLOBAL_SECS: u64 = 1800;

/// What a budgeted step is, for accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// An LLM turn (counts against the budget).
    LlmTurn,
    /// A tool call (counts against the budget).
    ToolCall,
    /// Deterministic code execution — refunded, never counts (Hermes
    /// `execute_code` refund).
    ExecuteCode,
}

/// The agent whose budget was exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Parent,
    SubAgent,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BudgetError {
    #[error("parent iteration budget exhausted ({max} turns)")]
    ParentExhausted { max: u32 },
    #[error("sub-agent iteration budget exhausted ({max} turns)")]
    SubAgentExhausted { max: u32 },
}

/// Parent/sub-agent turn budgets with the execute-code refund.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IterationBudget {
    pub parent_max: u32,
    pub subagent_max: u32,
    parent_used: u32,
    subagent_used: u32,
}

impl Default for IterationBudget {
    fn default() -> Self {
        Self::new(PARENT_MAX_ITERATIONS, SUBAGENT_MAX_ITERATIONS)
    }
}

impl IterationBudget {
    pub fn new(parent_max: u32, subagent_max: u32) -> Self {
        Self {
            parent_max,
            subagent_max,
            parent_used: 0,
            subagent_used: 0,
        }
    }

    pub fn parent_used(&self) -> u32 {
        self.parent_used
    }

    pub fn subagent_used(&self) -> u32 {
        self.subagent_used
    }

    pub fn remaining_parent(&self) -> u32 {
        self.parent_max.saturating_sub(self.parent_used)
    }

    pub fn remaining_subagent(&self) -> u32 {
        self.subagent_max.saturating_sub(self.subagent_used)
    }

    /// Count a parent step. `ExecuteCode` is refunded (deterministic code
    /// shouldn't consume the LLM-turn budget).
    pub fn parent_step(&mut self, kind: StepKind) -> Result<(), BudgetError> {
        if kind == StepKind::ExecuteCode {
            return Ok(()); // refund: no charge
        }
        self.parent_used += 1;
        if self.parent_used > self.parent_max {
            return Err(BudgetError::ParentExhausted {
                max: self.parent_max,
            });
        }
        Ok(())
    }

    /// Count a sub-agent step (sub-agents get their own, smaller cap).
    pub fn subagent_step(&mut self) -> Result<(), BudgetError> {
        self.subagent_used += 1;
        if self.subagent_used > self.subagent_max {
            return Err(BudgetError::SubAgentExhausted {
                max: self.subagent_max,
            });
        }
        Ok(())
    }
}

/// A rolling repeat detector: hash the last `window` tool calls; the same
/// sequence repeating `threshold` times consecutively → interrupt.
#[derive(Debug, Clone)]
pub struct LoopDetector {
    window: usize,
    threshold: usize,
    hashes: VecDeque<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopVerdict {
    Normal,
    /// The sequence repeated, but below the interrupt threshold.
    Repeat {
        repeats: usize,
    },
    /// The sequence repeated `threshold` times → interrupt.
    Interrupted {
        repeats: usize,
    },
}

impl Default for LoopDetector {
    fn default() -> Self {
        Self::new(4, 3)
    }
}

impl LoopDetector {
    pub fn new(window: usize, threshold: usize) -> Self {
        assert!(window > 0, "window must be positive");
        assert!(threshold > 0, "threshold must be positive");
        Self {
            window,
            threshold,
            hashes: VecDeque::new(),
        }
    }

    pub fn window(&self) -> usize {
        self.window
    }

    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// Observe one tool call (hashed by name + normalized args) and report
    /// whether it forms a repeat loop.
    pub fn observe(&mut self, tool_call: &str) -> LoopVerdict {
        self.hashes.push_back(fnv1a(tool_call.as_bytes()));
        // Keep just enough history to detect `threshold` repeats of a
        // `window`-sequence (bounded memory, no unbounded growth).
        while self.hashes.len() > self.window * (self.threshold + 1) {
            self.hashes.pop_front();
        }
        let v: Vec<u64> = self.hashes.iter().copied().collect();
        let n = v.len();
        if n < self.window {
            return LoopVerdict::Normal;
        }
        // The trailing `window`-sequence.
        let tail = &v[n - self.window..];
        // Count consecutive trailing windows equal to `tail`.
        let mut repeats = 1usize;
        let mut end = n - self.window;
        while end >= self.window {
            let start = end - self.window;
            if &v[start..end] == tail {
                repeats += 1;
                end = start;
            } else {
                break;
            }
        }
        if repeats >= self.threshold {
            LoopVerdict::Interrupted { repeats }
        } else if repeats > 1 {
            LoopVerdict::Repeat { repeats }
        } else {
            LoopVerdict::Normal
        }
    }

    pub fn reset(&mut self) {
        self.hashes.clear();
    }
}

/// FNV-1a 64-bit (deterministic, session-scoped — no crypto needed).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Sub-agent timeout policy (DeerFlow).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeoutPolicy {
    pub custom_secs: u64,
    pub global_secs: u64,
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        Self {
            custom_secs: SUBAGENT_TIMEOUT_CUSTOM_SECS,
            global_secs: SUBAGENT_TIMEOUT_GLOBAL_SECS,
        }
    }
}

/// Why the circuit broke (the MCQ card's headline).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum InterruptReason {
    BudgetExhausted { scope: Scope },
    LoopDetected { repeats: usize },
    Timeout { secs: u64 },
}

/// The choices the user gets on the interrupt card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McqOption {
    Skip,
    Retry,
    Escalate,
    TakeOver,
}

/// The MCQ interrupt card payload the UI renders (H2 cockpit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CircuitBreak {
    pub reason: InterruptReason,
    pub options: Vec<McqOption>,
}

impl CircuitBreak {
    pub fn budget(scope: Scope) -> Self {
        Self {
            reason: InterruptReason::BudgetExhausted { scope },
            options: vec![McqOption::Skip, McqOption::Retry, McqOption::Escalate],
        }
    }

    pub fn loop_detected(repeats: usize) -> Self {
        Self {
            reason: InterruptReason::LoopDetected { repeats },
            options: vec![
                McqOption::Skip,
                McqOption::Retry,
                McqOption::Escalate,
                McqOption::TakeOver,
            ],
        }
    }

    pub fn timeout(secs: u64) -> Self {
        Self {
            reason: InterruptReason::Timeout { secs },
            options: vec![McqOption::Skip, McqOption::Retry, McqOption::Escalate],
        }
    }
}

/// The per-turn circuit-breaker seam: budget first, then loop detection.
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    pub budget: IterationBudget,
    pub detector: LoopDetector,
}

impl CircuitBreaker {
    pub fn new(budget: IterationBudget, detector: LoopDetector) -> Self {
        Self { budget, detector }
    }

    /// Step the parent: charge the budget (refunded for `ExecuteCode`), then
    /// observe the tool call for a loop. Returns the first break that trips.
    pub fn step(&mut self, kind: StepKind, tool_call: &str) -> Result<(), CircuitBreak> {
        if let Err(e) = self.budget.parent_step(kind) {
            let scope = match e {
                BudgetError::ParentExhausted { .. } => Scope::Parent,
                BudgetError::SubAgentExhausted { .. } => Scope::SubAgent,
            };
            return Err(CircuitBreak::budget(scope));
        }
        match self.detector.observe(tool_call) {
            LoopVerdict::Interrupted { repeats } => Err(CircuitBreak::loop_detected(repeats)),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_defaults_are_500_50() {
        let b = IterationBudget::default();
        assert_eq!(b.parent_max, 500);
        assert_eq!(b.subagent_max, 50);
    }

    #[test]
    fn parent_budget_exhausts() {
        let mut b = IterationBudget::new(3, 50);
        assert!(b.parent_step(StepKind::LlmTurn).is_ok());
        assert!(b.parent_step(StepKind::ToolCall).is_ok());
        assert!(b.parent_step(StepKind::LlmTurn).is_ok());
        assert!(matches!(
            b.parent_step(StepKind::LlmTurn),
            Err(BudgetError::ParentExhausted { max: 3 })
        ));
    }

    #[test]
    fn execute_code_is_refunded() {
        let mut b = IterationBudget::new(1, 50);
        // Deterministic code never charges.
        for _ in 0..100 {
            assert!(b.parent_step(StepKind::ExecuteCode).is_ok());
        }
        assert_eq!(b.parent_used(), 0);
        assert_eq!(b.remaining_parent(), 1);
        // One LLM turn exhausts it.
        b.parent_step(StepKind::LlmTurn).unwrap();
        assert_eq!(b.parent_used(), 1);
        assert_eq!(b.remaining_parent(), 0);
    }

    #[test]
    fn subagent_budget_exhausts() {
        let mut b = IterationBudget::new(500, 2);
        assert!(b.subagent_step().is_ok());
        assert!(b.subagent_step().is_ok());
        assert!(matches!(
            b.subagent_step(),
            Err(BudgetError::SubAgentExhausted { max: 2 })
        ));
    }

    #[test]
    fn loop_detector_interrupts_on_3x_repeat() {
        let mut d = LoopDetector::new(1, 3);
        assert_eq!(d.observe("read_file(foo)"), LoopVerdict::Normal);
        assert_eq!(
            d.observe("read_file(foo)"),
            LoopVerdict::Repeat { repeats: 2 }
        );
        assert_eq!(
            d.observe("read_file(foo)"),
            LoopVerdict::Interrupted { repeats: 3 }
        );
    }

    #[test]
    fn loop_detector_resets_on_change() {
        let mut d = LoopDetector::new(1, 3);
        d.observe("a");
        d.observe("a");
        d.observe("b"); // breaks the streak
        assert_eq!(d.observe("a"), LoopVerdict::Normal);
    }

    #[test]
    fn loop_detector_detects_sequence_repeat() {
        // window=2: the pair (a,b) repeating 3x → interrupt.
        let mut d = LoopDetector::new(2, 3);
        let seq = ["tool_a", "tool_b", "tool_a", "tool_b", "tool_a", "tool_b"];
        let verdicts: Vec<LoopVerdict> = seq.iter().map(|t| d.observe(t)).collect();
        assert_eq!(
            verdicts.last(),
            Some(&LoopVerdict::Interrupted { repeats: 3 })
        );
    }

    #[test]
    fn timeout_policy_defaults() {
        let t = TimeoutPolicy::default();
        assert_eq!(t.custom_secs, 900);
        assert_eq!(t.global_secs, 1800);
    }

    #[test]
    fn circuit_break_cards_carry_mcq_options() {
        let budget = CircuitBreak::budget(Scope::Parent);
        assert_eq!(
            budget.reason,
            InterruptReason::BudgetExhausted {
                scope: Scope::Parent
            }
        );
        assert_eq!(budget.options.len(), 3);

        let looped = CircuitBreak::loop_detected(3);
        assert_eq!(looped.reason, InterruptReason::LoopDetected { repeats: 3 });
        assert!(looped.options.contains(&McqOption::TakeOver));

        let timeout = CircuitBreak::timeout(1800);
        assert_eq!(timeout.reason, InterruptReason::Timeout { secs: 1800 });
    }

    #[test]
    fn circuit_breaker_trips_on_budget() {
        let mut cb = CircuitBreaker::new(IterationBudget::new(2, 50), LoopDetector::default());
        assert!(cb.step(StepKind::LlmTurn, "a").is_ok());
        assert!(cb.step(StepKind::LlmTurn, "b").is_ok());
        let err = cb.step(StepKind::LlmTurn, "c").unwrap_err();
        assert!(matches!(
            err.reason,
            InterruptReason::BudgetExhausted {
                scope: Scope::Parent
            }
        ));
    }

    #[test]
    fn circuit_breaker_trips_on_loop() {
        let mut cb = CircuitBreaker::new(IterationBudget::default(), LoopDetector::new(1, 3));
        cb.step(StepKind::LlmTurn, "x").unwrap();
        cb.step(StepKind::LlmTurn, "x").unwrap();
        let err = cb.step(StepKind::LlmTurn, "x").unwrap_err();
        assert!(matches!(
            err.reason,
            InterruptReason::LoopDetected { repeats: 3 }
        ));
    }
}
