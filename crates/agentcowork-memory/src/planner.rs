//! Context planner (C7 — doc 63 §2.1; the "warm-set injection, scope-leakage
//! floors, 0ms TTFT" enforcement). Decides what enters the context window
//! each turn: the core warm set always fits, retrieval output respects the
//! budget, and a scope-leakage floor rejects signals that would blow the
//! window. Deterministic — the coordinator feeds it per-turn token counts.

use crate::paging::{CORE_BUDGET_TOKENS, PagedMemory};

/// Planner budget knobs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlannerConfig {
    /// Total context budget (tokens) the planner must never exceed.
    pub context_budget_tokens: usize,
    /// Max tokens for retrieval/evidence output per turn.
    pub retrieval_budget_tokens: usize,
    /// Max tokens for tool results per turn.
    pub tool_result_budget_tokens: usize,
    /// The headroom floor (fraction of budget) that must stay free for the
    /// model's reply — crossing it is a scope leak.
    pub scope_leakage_floor: f64,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            context_budget_tokens: 32_000,
            retrieval_budget_tokens: 6_000,
            tool_result_budget_tokens: 12_000,
            scope_leakage_floor: 0.15,
        }
    }
}

/// Why the planner cut or admitted something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlannerDecision {
    /// Admitted as-is.
    Admitted,
    /// Fits only after truncation (the caller truncates to the given tokens).
    Truncate,
    /// Rejected — admitting it would blow the budget (scope leak).
    Reject,
}

/// One planning outcome for a candidate chunk of context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetResult {
    pub decision: PlannerDecision,
    /// Tokens the caller may inject (0 when rejected).
    pub allowed_tokens: usize,
}

/// The turn-level context planner.
#[derive(Debug, Clone, Copy)]
pub struct ContextPlanner {
    pub config: PlannerConfig,
    /// Tokens already committed this turn (core warm set + admitted content).
    committed: usize,
}

impl ContextPlanner {
    pub fn new(config: PlannerConfig) -> Self {
        Self {
            config,
            committed: 0,
        }
    }

    /// Commit the core warm set (memory + persona) at the start of the turn.
    /// The warm set is capped at `CORE_BUDGET_TOKENS`; returns its size.
    pub fn inject_warm_set(&mut self, memory: &PagedMemory, persona_tokens: usize) -> usize {
        let core = memory.core_tokens().min(CORE_BUDGET_TOKENS);
        let total = (core + persona_tokens).min(self.config.context_budget_tokens);
        self.committed = total;
        total
    }

    /// Remaining budget after the warm set.
    pub fn remaining(&self) -> usize {
        self.config
            .context_budget_tokens
            .saturating_sub(self.committed)
    }

    /// Plan admission of a `candidate_tokens` chunk (retrieval evidence or
    /// tool result). Enforces the per-category cap AND the overall budget with
    /// the scope-leakage floor. `category_cap` is the per-kind limit.
    pub fn plan(&mut self, candidate_tokens: usize, category_cap: usize) -> BudgetResult {
        let floor_reserve =
            (self.config.context_budget_tokens as f64 * self.config.scope_leakage_floor) as usize;
        let available = self
            .config
            .context_budget_tokens
            .saturating_sub(self.committed)
            .saturating_sub(floor_reserve);
        let allowed = available.min(category_cap);
        if candidate_tokens == 0 || allowed == 0 {
            return BudgetResult {
                decision: PlannerDecision::Reject,
                allowed_tokens: 0,
            };
        }
        if candidate_tokens <= allowed {
            self.committed += candidate_tokens;
            BudgetResult {
                decision: PlannerDecision::Admitted,
                allowed_tokens: candidate_tokens,
            }
        } else {
            // Partial admission: caller truncates to `allowed` tokens.
            self.committed += allowed;
            BudgetResult {
                decision: PlannerDecision::Truncate,
                allowed_tokens: allowed,
            }
        }
    }

    /// Convenience: plan a retrieval-evidence chunk (uses the retrieval cap).
    pub fn plan_retrieval(&mut self, tokens: usize) -> BudgetResult {
        self.plan(tokens, self.config.retrieval_budget_tokens)
    }

    /// Convenience: plan a tool-result chunk (uses the tool cap).
    pub fn plan_tool_result(&mut self, tokens: usize) -> BudgetResult {
        self.plan(tokens, self.config.tool_result_budget_tokens)
    }

    /// End the turn: release the committed budget for the next turn. Returns
    /// the tokens used this turn (for TTFT accounting).
    pub fn end_turn(&mut self) -> usize {
        let used = self.committed;
        self.committed = 0;
        used
    }

    /// Is the current commit crossing the scope-leakage floor (context would
    /// leave too little room for the reply)? The coordinator warns before
    /// admitting more.
    pub fn leaking(&self) -> bool {
        self.committed as f64 / self.config.context_budget_tokens as f64
            >= 1.0 - self.config.scope_leakage_floor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planner() -> ContextPlanner {
        ContextPlanner::new(PlannerConfig {
            context_budget_tokens: 10_000,
            retrieval_budget_tokens: 2_000,
            tool_result_budget_tokens: 3_000,
            scope_leakage_floor: 0.1,
        })
    }

    #[test]
    fn warm_set_commits_and_leaves_room() {
        let mut p = planner();
        let memory = PagedMemory::new();
        let used = p.inject_warm_set(&memory, 500);
        assert_eq!(used, 500);
        assert!(!p.leaking());
        assert_eq!(p.remaining(), 9_500);
    }

    #[test]
    fn warm_set_is_capped_at_core_budget() {
        let mut p = planner();
        let mut memory = PagedMemory::new();
        memory.write(crate::paging::MemoryEntry {
            id: "big".into(),
            content: "x".repeat(CORE_BUDGET_TOKENS * 4 * 4),
            importance: 9,
        });
        memory.flush_writes();
        let used = p.inject_warm_set(&memory, 100);
        assert!(used <= CORE_BUDGET_TOKENS + 100);
    }

    #[test]
    fn plan_admits_within_caps() {
        let mut p = planner();
        p.inject_warm_set(&PagedMemory::new(), 500);
        let r = p.plan_retrieval(1_000);
        assert_eq!(r.decision, PlannerDecision::Admitted);
        assert_eq!(r.allowed_tokens, 1_000);
    }

    #[test]
    fn plan_truncates_oversized_chunks() {
        let mut p = planner();
        p.inject_warm_set(&PagedMemory::new(), 500);
        let r = p.plan_retrieval(5_000); // over the 2k retrieval cap
        assert_eq!(r.decision, PlannerDecision::Truncate);
        assert_eq!(r.allowed_tokens, 2_000);
    }

    #[test]
    fn plan_rejects_when_budget_exhausted() {
        let mut p = planner();
        p.inject_warm_set(&PagedMemory::new(), 9_500); // nearly full
        let r = p.plan_retrieval(1_000);
        // 9500 committed + floor reserve 1000 = 10_500 > 10_000 → reject.
        assert_eq!(r.decision, PlannerDecision::Reject);
        assert_eq!(r.allowed_tokens, 0);
    }

    #[test]
    fn scope_leakage_flagged_when_floor_crossed() {
        let mut p = planner();
        p.inject_warm_set(&PagedMemory::new(), 9_000); // floor reserve = 1k
        assert!(p.leaking()); // 9_000 ≥ 9_000
    }

    #[test]
    fn end_turn_releases_budget() {
        let mut p = planner();
        p.inject_warm_set(&PagedMemory::new(), 500);
        p.plan_retrieval(1_000);
        let used = p.end_turn();
        assert_eq!(used, 1_500);
        assert_eq!(p.remaining(), 10_000);
    }
}
