//! Blueprint + verify-gated tasks (P6.1 — doc 63 §2.3, openspec verify-gate).
//!
//! Each blueprint task carries a `verify` block — deterministic checks that
//! must pass before the task is marked done. **We never accept the agent's own
//! "finished" claim**; the verifier ([`crate::verify`]) proves the state.

use crate::spec::TaskSpec;
use crate::verify::{OutcomeCheck, TaskManifest, VerificationReport, verify};
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// The deterministic verify block attached to a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyBlock {
    /// Deterministic outcome checks (exists/hash/contains).
    pub checks: Vec<OutcomeCheck>,
    /// Approvals / forbidden-side-effect notes the harness enforces.
    pub policy: Vec<String>,
}

impl VerifyBlock {
    pub fn new(checks: Vec<OutcomeCheck>) -> Self {
        Self {
            checks,
            policy: Vec::new(),
        }
    }
}

/// Task lifecycle status (the verifier decides `done`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    /// All verify checks pass.
    Done,
    /// Verify checks failed — the task is not done.
    Failed,
    /// Blocked on a missing capability/approval.
    Blocked,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::InProgress => "in_progress",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
            TaskStatus::Blocked => "blocked",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(TaskStatus::Pending),
            "in_progress" | "in-progress" | "running" => Some(TaskStatus::InProgress),
            "done" | "complete" | "completed" => Some(TaskStatus::Done),
            "failed" => Some(TaskStatus::Failed),
            "blocked" => Some(TaskStatus::Blocked),
            _ => None,
        }
    }

    /// The DAG state machine — the only legal transitions. The verifier
    /// moves a task `InProgress → Done/Failed`; a blocked task can be
    /// unblocked (`Blocked → InProgress`) or abandoned (`Blocked → Failed`);
    /// a failed task can be retried (`Failed → InProgress`). `Done` and
    /// `Failed` are terminal (retry starts a new task).
    pub fn transition(self, to: TaskStatus) -> Result<TaskStatus, String> {
        use TaskStatus::*;
        let legal = match (self, to) {
            (a, b) if a == b => true,
            (Pending, InProgress) => true,
            (InProgress, Done) | (InProgress, Failed) | (InProgress, Blocked) => true,
            (Blocked, InProgress) | (Blocked, Failed) => true,
            (Failed, InProgress) => true,
            _ => false,
        };
        if legal {
            Ok(to)
        } else {
            Err(format!("illegal transition {self:?} -> {to:?}"))
        }
    }
}

/// One task in a blueprint: a spec + its verify gate + dependencies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlueprintTask {
    pub spec: TaskSpec,
    pub verify: VerifyBlock,
    /// Task ids that must be `Done` before this one starts.
    pub depends_on: Vec<String>,
    pub status: TaskStatus,
}

impl BlueprintTask {
    pub fn new(spec: TaskSpec, verify: VerifyBlock) -> Self {
        Self {
            spec,
            verify,
            depends_on: Vec::new(),
            status: TaskStatus::Pending,
        }
    }

    /// Convert the verify block into an eval manifest for verification.
    pub fn to_manifest(&self) -> TaskManifest {
        TaskManifest {
            task_id: self.spec.id.clone(),
            goal: self.spec.goal.clone(),
            required_outcomes: self.verify.checks.clone(),
            constraints: vec![],
            budgets: Default::default(),
            evidence: vec![],
        }
    }

    /// Run the verifier against the workspace `base_dir`. Never trusts the
    /// agent's own claim — the checks decide.
    pub fn verify_against(&self, base_dir: &Path) -> VerificationReport {
        verify(&self.to_manifest(), base_dir)
    }
}

/// A blueprint: an ordered, dependency-aware set of tasks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blueprint {
    pub id: String,
    pub goal: String,
    pub tasks: Vec<BlueprintTask>,
}

#[derive(Debug, Error)]
pub enum BlueprintError {
    #[error("task {0} depends on unknown task {1}")]
    UnknownDependency(String, String),
    #[error("cycle detected involving task {0}")]
    Cycle(String),
}

impl Blueprint {
    pub fn new(id: impl Into<String>, goal: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            goal: goal.into(),
            tasks: Vec::new(),
        }
    }

    pub fn push(&mut self, task: BlueprintTask) {
        self.tasks.push(task);
    }

    pub fn find(&self, id: &str) -> Option<&BlueprintTask> {
        self.tasks.iter().find(|t| t.spec.id == id)
    }

    /// Tasks whose dependencies are all `Done` (the ready set).
    pub fn ready(&self) -> Vec<&BlueprintTask> {
        self.tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Done && t.status != TaskStatus::Failed)
            .filter(|t| {
                t.depends_on.iter().all(|d| {
                    self.find(d)
                        .map(|d| d.status == TaskStatus::Done)
                        .unwrap_or(false)
                })
            })
            .collect()
    }

    /// Validate dependency references and detect cycles.
    pub fn validate(&self) -> Result<(), BlueprintError> {
        for t in &self.tasks {
            for d in &t.depends_on {
                if self.find(d).is_none() {
                    return Err(BlueprintError::UnknownDependency(
                        t.spec.id.clone(),
                        d.clone(),
                    ));
                }
            }
        }
        // DFS cycle detection.
        let mut visiting = std::collections::HashSet::new();
        let mut visited = std::collections::HashSet::new();
        for t in &self.tasks {
            if self.detect_cycle(&t.spec.id, &mut visiting, &mut visited)? {
                return Err(BlueprintError::Cycle(t.spec.id.clone()));
            }
        }
        Ok(())
    }

    /// A deterministic execution order (Kahn's algorithm) — every task is
    /// listed after its dependencies. Errors on unknown deps or a cycle.
    pub fn topological_order(&self) -> Result<Vec<String>, BlueprintError> {
        self.validate()?;
        let mut indegree: Vec<usize> = self.tasks.iter().map(|t| t.depends_on.len()).collect();
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); self.tasks.len()];
        for (i, t) in self.tasks.iter().enumerate() {
            for d in &t.depends_on {
                if let Some(j) = self.tasks.iter().position(|x| x.spec.id == *d) {
                    dependents[j].push(i);
                }
            }
        }
        let mut queue: std::collections::VecDeque<usize> = (0..self.tasks.len())
            .filter(|&i| indegree[i] == 0)
            .collect();
        let mut order = Vec::with_capacity(self.tasks.len());
        while let Some(i) = queue.pop_front() {
            order.push(self.tasks[i].spec.id.clone());
            for &dep in &dependents[i] {
                indegree[dep] -= 1;
                if indegree[dep] == 0 {
                    queue.push_back(dep);
                }
            }
        }
        if order.len() != self.tasks.len() {
            return Err(BlueprintError::Cycle(self.id.clone()));
        }
        Ok(order)
    }

    /// Apply a status transition by task id (the DAG state machine).
    pub fn set_status(&mut self, id: &str, to: TaskStatus) -> Result<(), String> {
        let t = self
            .tasks
            .iter_mut()
            .find(|t| t.spec.id == id)
            .ok_or_else(|| format!("unknown task {id:?}"))?;
        t.status = t.status.transition(to)?;
        Ok(())
    }

    /// `true` when every task is `Done` (the plan is complete).
    pub fn is_complete(&self) -> bool {
        !self.tasks.is_empty() && self.tasks.iter().all(|t| t.status == TaskStatus::Done)
    }

    fn detect_cycle(
        &self,
        id: &str,
        visiting: &mut std::collections::HashSet<String>,
        visited: &mut std::collections::HashSet<String>,
    ) -> Result<bool, BlueprintError> {
        if visiting.contains(id) {
            return Ok(true);
        }
        if visited.contains(id) {
            return Ok(false);
        }
        visiting.insert(id.to_string());
        if let Some(t) = self.find(id) {
            for d in &t.depends_on {
                if self.detect_cycle(d, visiting, visited)? {
                    return Ok(true);
                }
            }
        }
        visiting.remove(id);
        visited.insert(id.to_string());
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::OutcomeCheck;

    fn spec(id: &str) -> TaskSpec {
        TaskSpec::new(id, format!("goal {id}"))
    }

    fn check(path: &str) -> OutcomeCheck {
        OutcomeCheck::FileExists { path: path.into() }
    }

    #[test]
    fn manifest_conversion_carries_checks() {
        let t = BlueprintTask::new(spec("a"), VerifyBlock::new(vec![check("out.txt")]));
        let m = t.to_manifest();
        assert_eq!(m.task_id, "a");
        assert_eq!(m.required_outcomes.len(), 1);
    }

    #[test]
    fn verify_against_runs_the_verifier() {
        let dir = std::env::temp_dir().join("bp-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("out.txt"), "x").unwrap();

        let t = BlueprintTask::new(spec("a"), VerifyBlock::new(vec![check("out.txt")]));
        let report = t.verify_against(&dir);
        assert!(report.status.is_complete());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ready_respects_dependencies() {
        let mut bp = Blueprint::new("bp", "g");
        let a = BlueprintTask::new(spec("a"), VerifyBlock::new(vec![]));
        let mut b = BlueprintTask::new(spec("b"), VerifyBlock::new(vec![]));
        b.depends_on.push("a".into());
        bp.push(a);
        bp.push(b);

        assert_eq!(bp.ready().len(), 1); // only "a" is ready (no deps)
        assert_eq!(bp.ready()[0].spec.id, "a");

        // Mark "a" done → "b" becomes ready.
        bp.tasks[0].status = TaskStatus::Done;
        assert_eq!(bp.ready().len(), 1);
        assert_eq!(bp.ready()[0].spec.id, "b");
    }

    #[test]
    fn validate_detects_unknown_dependency_and_cycle() {
        let mut bp = Blueprint::new("bp", "g");
        let mut a = BlueprintTask::new(spec("a"), VerifyBlock::new(vec![]));
        a.depends_on.push("ghost".into());
        bp.push(a);
        assert!(matches!(
            bp.validate(),
            Err(BlueprintError::UnknownDependency(..))
        ));

        let mut bp2 = Blueprint::new("bp2", "g");
        let mut a = BlueprintTask::new(spec("a"), VerifyBlock::new(vec![]));
        a.depends_on.push("b".into());
        let mut b = BlueprintTask::new(spec("b"), VerifyBlock::new(vec![]));
        b.depends_on.push("a".into());
        bp2.push(a);
        bp2.push(b);
        assert!(matches!(bp2.validate(), Err(BlueprintError::Cycle(..))));
    }

    #[test]
    fn status_machine_enforces_legal_transitions() {
        assert_eq!(
            TaskStatus::Pending
                .transition(TaskStatus::InProgress)
                .unwrap(),
            TaskStatus::InProgress
        );
        assert_eq!(
            TaskStatus::InProgress.transition(TaskStatus::Done).unwrap(),
            TaskStatus::Done
        );
        assert_eq!(
            TaskStatus::Blocked
                .transition(TaskStatus::InProgress)
                .unwrap(),
            TaskStatus::InProgress
        );
        assert_eq!(
            TaskStatus::Failed
                .transition(TaskStatus::InProgress)
                .unwrap(),
            TaskStatus::InProgress
        );
        // Done is terminal.
        assert!(TaskStatus::Done.transition(TaskStatus::InProgress).is_err());
        // Pending cannot jump straight to Done without running.
        assert!(TaskStatus::Pending.transition(TaskStatus::Done).is_err());
    }

    #[test]
    fn status_parse_roundtrips() {
        for s in [
            TaskStatus::Pending,
            TaskStatus::InProgress,
            TaskStatus::Done,
            TaskStatus::Failed,
            TaskStatus::Blocked,
        ] {
            assert_eq!(TaskStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(TaskStatus::parse("completed"), Some(TaskStatus::Done));
        assert_eq!(TaskStatus::parse("bogus"), None);
    }

    #[test]
    fn topological_order_respects_dependencies() {
        let mut bp = Blueprint::new("bp", "g");
        let a = BlueprintTask::new(spec("a"), VerifyBlock::new(vec![]));
        let mut b = BlueprintTask::new(spec("b"), VerifyBlock::new(vec![]));
        b.depends_on.push("a".into());
        let mut c = BlueprintTask::new(spec("c"), VerifyBlock::new(vec![]));
        c.depends_on.push("b".into());
        bp.push(c); // pushed out of order on purpose
        bp.push(b);
        bp.push(a);

        let order = bp.topological_order().unwrap();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn topological_order_rejects_cycles() {
        let mut bp = Blueprint::new("bp", "g");
        let mut a = BlueprintTask::new(spec("a"), VerifyBlock::new(vec![]));
        a.depends_on.push("b".into());
        let mut b = BlueprintTask::new(spec("b"), VerifyBlock::new(vec![]));
        b.depends_on.push("a".into());
        bp.push(a);
        bp.push(b);
        assert!(matches!(
            bp.topological_order(),
            Err(BlueprintError::Cycle(_))
        ));
    }

    #[test]
    fn set_status_and_is_complete() {
        let mut bp = Blueprint::new("bp", "g");
        bp.push(BlueprintTask::new(spec("a"), VerifyBlock::new(vec![])));
        assert!(!bp.is_complete());
        bp.set_status("a", TaskStatus::InProgress).unwrap();
        bp.set_status("a", TaskStatus::Done).unwrap();
        assert!(bp.is_complete());
        assert!(bp.set_status("ghost", TaskStatus::Done).is_err());
    }
}
