//! Stage-0 plan executor support (P6.3 seam) — the per-plan [`CircuitBreaker`]
//! owner the coordinator drives over `plan/*` JSON-RPC.
//!
//! The `CircuitBreak`/`McqOption` model lives in `agentcowork-blueprint::iteration`
//! with no runtime producer: it fires from `CircuitBreaker::step` inside
//! blueprint plan execution, and no plan executor ran in the coordinator. This
//! service gives the coordinator a per-plan breaker to step between LLM turns
//! and tool calls, and the trip it returns becomes the `chat/interrupt`
//! notification the UI's MCQ card renders (H2 cockpit).
//!
//! Rust owns the breaker state ("Rust disposes"); the coordinator proposes
//! each step. A dead/replaying sidecar can't double-charge a budget or hide a
//! loop — the same breaker instance lives here for the plan's lifetime.

use std::collections::HashMap;

use agentcowork_blueprint::iteration::{CircuitBreaker, IterationBudget, LoopDetector};
use serde_json::{Value, json};

/// The executor-facing pre-flight state: one breaker per active plan.
#[derive(Debug, Default)]
pub struct PlanService {
    breakers: HashMap<String, CircuitBreaker>,
}

impl PlanService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a plan with a fresh breaker (Hermes defaults 500/50 +
    /// loop detector 4/3 unless overridden). A duplicate planId resets it.
    pub fn begin(
        &mut self,
        plan_id: &str,
        parent_max: Option<u32>,
        subagent_max: Option<u32>,
        loop_window: Option<usize>,
        loop_threshold: Option<usize>,
    ) {
        let budget = IterationBudget::new(
            parent_max.unwrap_or(agentcowork_blueprint::PARENT_MAX_ITERATIONS),
            subagent_max.unwrap_or(agentcowork_blueprint::SUBAGENT_MAX_ITERATIONS),
        );
        let detector = LoopDetector::new(loop_window.unwrap_or(4), loop_threshold.unwrap_or(3));
        self.breakers
            .insert(plan_id.to_string(), CircuitBreaker::new(budget, detector));
    }

    /// Step the plan's breaker. `Ok(Value)` is either `{"ok": true}` or
    /// `{"ok": false, "interrupt": <CircuitBreak>}` — the trip the coordinator
    /// turns into a `chat/interrupt` notification. `Err` when the plan is
    /// unknown (the coordinator must `plan/begin` first).
    pub fn step(
        &mut self,
        plan_id: &str,
        kind: agentcowork_blueprint::StepKind,
        tool_call: &str,
    ) -> Result<Value, String> {
        let breaker = self
            .breakers
            .get_mut(plan_id)
            .ok_or_else(|| format!("unknown plan {plan_id:?} — call plan/begin first"))?;
        match breaker.step(kind, tool_call) {
            Ok(()) => Ok(json!({ "ok": true })),
            Err(break_) => Ok(json!({
                "ok": false,
                "interrupt": serde_json::to_value(&break_)
                    .map_err(|e| format!("interrupt serialize: {e}"))?,
            })),
        }
    }

    /// Drop the plan's breaker (end of execution). No-op for unknown plans.
    pub fn end(&mut self, plan_id: &str) {
        self.breakers.remove(plan_id);
    }

    /// Active plan ids (diagnostics / tests).
    pub fn active_plans(&self) -> Vec<String> {
        self.breakers.keys().cloned().collect()
    }

    /// JSON-RPC dispatch (`plan/*`) — the coordinator drives the same breaker
    /// the whole plan run steps, so there is one source of truth.
    pub fn handle(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        match method {
            "plan/begin" => {
                let plan_id = str_param(params, "planId").ok_or("plan/begin requires planId")?;
                let parent_max = params
                    .get("parentMax")
                    .and_then(Value::as_u64)
                    .map(|v| v as u32);
                let subagent_max = params
                    .get("subagentMax")
                    .and_then(Value::as_u64)
                    .map(|v| v as u32);
                let loop_window = params
                    .get("loopWindow")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize);
                let loop_threshold = params
                    .get("loopThreshold")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize);
                self.begin(
                    plan_id,
                    parent_max,
                    subagent_max,
                    loop_window,
                    loop_threshold,
                );
                Ok(json!({ "started": true, "planId": plan_id }))
            }
            "plan/step" => {
                let plan_id = str_param(params, "planId").ok_or("plan/step requires planId")?;
                let kind = parse_step_kind(params)?;
                let tool_call = str_param(params, "toolCall").unwrap_or("");
                self.step(plan_id, kind, tool_call)
            }
            "plan/end" => {
                let plan_id = str_param(params, "planId").ok_or("plan/end requires planId")?;
                self.end(plan_id);
                Ok(json!({ "ended": true, "planId": plan_id }))
            }
            "plan/list" => Ok(json!({ "plans": self.active_plans() })),
            _ => Err(format!("method not found: {method}")),
        }
    }
}

fn str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

fn parse_step_kind(params: &Value) -> Result<agentcowork_blueprint::StepKind, String> {
    use agentcowork_blueprint::StepKind;
    let name = str_param(params, "kind").ok_or("plan/step requires kind")?;
    match name {
        "llm_turn" => Ok(StepKind::LlmTurn),
        "tool_call" => Ok(StepKind::ToolCall),
        "execute_code" => Ok(StepKind::ExecuteCode),
        other => Err(format!("unknown step kind {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentcowork_blueprint::{
        CircuitBreak, InterruptReason, McqOption, SUBAGENT_MAX_ITERATIONS, Scope,
    };

    fn params(value: serde_json::Value) -> Value {
        value
    }

    #[test]
    fn begin_step_end_lifecycle() {
        let mut svc = PlanService::new();
        svc.handle(
            "plan/begin",
            &params(json!({ "planId": "p1", "parentMax": 2 })),
        )
        .unwrap();
        assert_eq!(svc.active_plans(), vec!["p1".to_string()]);

        // Two LLM turns fit the parent budget of 2.
        let s1 = svc
            .handle(
                "plan/step",
                &params(json!({ "planId": "p1", "kind": "llm_turn", "toolCall": "t1" })),
            )
            .unwrap();
        assert_eq!(s1, json!({ "ok": true }));

        // Third turn trips the budget → the interrupt payload the coordinator
        // turns into a chat/interrupt notification.
        svc.handle(
            "plan/step",
            &params(json!({ "planId": "p1", "kind": "llm_turn", "toolCall": "t2" })),
        )
        .unwrap();
        let trip = svc
            .handle(
                "plan/step",
                &params(json!({ "planId": "p1", "kind": "llm_turn", "toolCall": "t3" })),
            )
            .unwrap();
        assert_eq!(trip["ok"], json!(false));
        let break_: CircuitBreak = serde_json::from_value(trip["interrupt"].clone()).unwrap();
        assert!(matches!(
            break_.reason,
            InterruptReason::BudgetExhausted {
                scope: Scope::Parent
            }
        ));
        assert!(break_.options.contains(&McqOption::Skip));
        assert!(break_.options.contains(&McqOption::Escalate));

        svc.handle("plan/end", &params(json!({ "planId": "p1" })))
            .unwrap();
        assert!(svc.active_plans().is_empty());
    }

    #[test]
    fn execute_code_is_refunded_by_the_breaker() {
        let mut svc = PlanService::new();
        svc.handle(
            "plan/begin",
            &params(json!({ "planId": "p1", "parentMax": 1 })),
        )
        .unwrap();
        // Deterministic code never charges (Hermes refund).
        for i in 0..50 {
            let out = svc
                .handle(
                    "plan/step",
                    &params(json!({ "planId": "p1", "kind": "execute_code", "toolCall": format!("code{i}") })),
                )
                .unwrap();
            assert_eq!(out, json!({ "ok": true }));
        }
        // One LLM turn still exhausts the 1-turn budget.
        svc.handle(
            "plan/step",
            &params(json!({ "planId": "p1", "kind": "llm_turn", "toolCall": "x" })),
        )
        .unwrap();
        let trip = svc
            .handle(
                "plan/step",
                &params(json!({ "planId": "p1", "kind": "llm_turn", "toolCall": "y" })),
            )
            .unwrap();
        assert_eq!(trip["ok"], json!(false));
    }

    #[test]
    fn loop_detector_trips_after_three_repeats() {
        let mut svc = PlanService::new();
        // window=1 threshold=3 → the same tool call 3× trips.
        svc.handle(
            "plan/begin",
            &params(json!({ "planId": "p1", "loopWindow": 1, "loopThreshold": 3 })),
        )
        .unwrap();
        for _ in 0..2 {
            let out = svc
                .handle(
                    "plan/step",
                    &params(json!({ "planId": "p1", "kind": "tool_call", "toolCall": "read(a)" })),
                )
                .unwrap();
            assert_eq!(out, json!({ "ok": true }));
        }
        let trip = svc
            .handle(
                "plan/step",
                &params(json!({ "planId": "p1", "kind": "tool_call", "toolCall": "read(a)" })),
            )
            .unwrap();
        assert_eq!(trip["ok"], json!(false));
        let break_: CircuitBreak = serde_json::from_value(trip["interrupt"].clone()).unwrap();
        assert!(matches!(
            break_.reason,
            InterruptReason::LoopDetected { repeats: 3 }
        ));
        assert!(break_.options.contains(&McqOption::TakeOver));
    }

    #[test]
    fn step_unknown_plan_is_an_error() {
        let mut svc = PlanService::new();
        let err = svc
            .handle(
                "plan/step",
                &params(json!({ "planId": "ghost", "kind": "llm_turn", "toolCall": "x" })),
            )
            .unwrap_err();
        assert!(err.contains("plan/begin first"), "err: {err}");
    }

    #[test]
    fn subagent_defaults_are_50() {
        // Sub-agent budget is separate from the parent — begin with parent=500
        // and verify the sub-agent cap default is 50 via a dedicated breaker.
        let mut svc = PlanService::new();
        svc.handle("plan/begin", &params(json!({ "planId": "p1" })))
            .unwrap();
        let breaker = svc.breakers.get("p1").unwrap();
        assert_eq!(breaker.budget.subagent_max, SUBAGENT_MAX_ITERATIONS);
        assert_eq!(
            breaker.budget.parent_max,
            agentcowork_blueprint::PARENT_MAX_ITERATIONS
        );
    }

    #[test]
    fn step_kind_parse_rejects_unknown() {
        let mut svc = PlanService::new();
        svc.handle("plan/begin", &params(json!({ "planId": "p1" })))
            .unwrap();
        let err = svc
            .handle(
                "plan/step",
                &params(json!({ "planId": "p1", "kind": "teleport", "toolCall": "x" })),
            )
            .unwrap_err();
        assert!(err.contains("unknown step kind"), "err: {err}");
    }

    #[test]
    fn handles_only_plan_methods() {
        let mut svc = PlanService::new();
        assert!(svc.handle("guard/evaluate", &json!({})).is_err());
        assert!(svc.handle("plan/list", &json!({})).is_ok());
    }
}
