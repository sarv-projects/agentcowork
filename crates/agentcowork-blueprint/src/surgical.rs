//! Surgical-hierarchy routing (P6.8 — doc 52 surgical hierarchy).
//!
//! The **brain → core → surgeon** decomposition routes a task to the harness
//! tier that should own it. This is *routing policy, not a pipeline*: a
//! simple edit goes straight to the surgeon (cheap, precise); only broad
//! refactors escalate through brain (plan) → core (coordinate) → surgeon
//! (edit). The routing is deterministic over a task class + a `surgical`
//! weight in `[0,1]` (how much the task needs decomposition vs. direct
//! editing), mirroring the asymmetric-tiering policy in
//! `agentcowork-vault::tier` but at the *harness* level (which agent runs it),
//! not the model level.
//!
//! Live Aider SEARCH/REPLACE validation remains an external-binary test; this
//! module is the routing decision the harness consumes.

use serde::{Deserialize, Serialize};

/// The three surgical-hierarchy roles (doc 52).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurgeonRole {
    /// Brain — plans / decomposes / judges (frontier agent).
    Brain,
    /// Core — coordinates sub-agents and the edit session.
    Core,
    /// Surgeon — the precise, cheap editor (Aider-style SEARCH/REPLACE).
    Surgeon,
}

/// Task classes the surgical router understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurgicalTask {
    /// A single-file, well-scoped edit.
    SimpleEdit,
    /// A broad, multi-file refactor.
    BroadRefactor,
    /// A code question (no edit required).
    CodeQuestion,
    /// Research / browser work.
    Research,
    /// Spreadsheet/data cleanup.
    DataCleanup,
}

/// The routing outcome: which role owns the task, and the escalation chain
/// the harness should use if the task grows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurgicalRoute {
    /// The owner role for this task.
    pub owner: SurgeonRole,
    /// The full escalation chain (owner first) for mid-task growth.
    pub chain: Vec<SurgeonRole>,
}

/// Route a task through the surgical hierarchy. `surgical` in `[0,1]` is the
/// task's need for decomposition/judgment (like the planner weight in the
/// Switchyard policy): low = direct edit, high = plan first.
pub fn surgical_route(task: SurgicalTask, surgical: f64) -> SurgicalRoute {
    let s = surgical.clamp(0.0, 1.0);
    use SurgeonRole::*;
    use SurgicalTask::*;
    match task {
        // Simple edits never leave the surgeon — the floor is the surgeon.
        SimpleEdit => SurgicalRoute {
            owner: Surgeon,
            chain: vec![Surgeon],
        },
        // Code questions are read-only brain work unless they turn into edits.
        CodeQuestion => SurgicalRoute {
            owner: Brain,
            chain: vec![Brain, Surgeon],
        },
        Research => SurgicalRoute {
            owner: Core,
            chain: vec![Core, Brain, Surgeon],
        },
        // Data cleanup is surgeon work with core oversight.
        DataCleanup => SurgicalRoute {
            owner: Surgeon,
            chain: vec![Surgeon, Core],
        },
        // Broad refactors escalate by surgical weight: low-surgical refactors
        // can still start on the surgeon; high-surgical ones plan first.
        BroadRefactor if s >= 0.5 => SurgicalRoute {
            owner: Brain,
            chain: vec![Brain, Core, Surgeon],
        },
        BroadRefactor => SurgicalRoute {
            owner: Core,
            chain: vec![Core, Surgeon],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_edit_stays_on_the_surgeon() {
        let r = surgical_route(SurgicalTask::SimpleEdit, 0.9);
        assert_eq!(r.owner, SurgeonRole::Surgeon);
        assert_eq!(r.chain, vec![SurgeonRole::Surgeon]);
    }

    #[test]
    fn broad_refactor_escalates_by_weight() {
        // High decomposition need → brain owns, full chain available.
        let planned = surgical_route(SurgicalTask::BroadRefactor, 0.8);
        assert_eq!(planned.owner, SurgeonRole::Brain);
        assert_eq!(
            planned.chain,
            vec![SurgeonRole::Brain, SurgeonRole::Core, SurgeonRole::Surgeon]
        );
        // Low need → core coordinates, surgeon does the edits.
        let direct = surgical_route(SurgicalTask::BroadRefactor, 0.2);
        assert_eq!(direct.owner, SurgeonRole::Core);
        assert_eq!(direct.chain, vec![SurgeonRole::Core, SurgeonRole::Surgeon]);
    }

    #[test]
    fn code_question_is_brain_work() {
        let r = surgical_route(SurgicalTask::CodeQuestion, 0.0);
        assert_eq!(r.owner, SurgeonRole::Brain);
    }

    #[test]
    fn data_cleanup_is_surgeon_with_core_oversight() {
        let r = surgical_route(SurgicalTask::DataCleanup, 0.0);
        assert_eq!(r.owner, SurgeonRole::Surgeon);
        assert_eq!(r.chain, vec![SurgeonRole::Surgeon, SurgeonRole::Core]);
    }

    #[test]
    fn surgical_weight_is_clamped() {
        let r = surgical_route(SurgicalTask::BroadRefactor, 2.0);
        assert_eq!(r.owner, SurgeonRole::Brain);
    }
}
