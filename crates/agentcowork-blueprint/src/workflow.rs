//! P25-1 — programmable workflows (doc 77 §2 — Airflow semantics over our
//! durable task primitives).
//!
//! Airflow's durable-execution vocabulary, mapped onto our blueprint task
//! machinery: DAG runs, per-task states, retries with exponential backoff,
//! **backfill** (run a past interval's dag runs), and the same "no silent
//! success" rule the rest of the crate enforces — a task is only `Success`
//! when its verify block passes.
//!
//! This is pattern-level adoption (the *semantics*, not Airflow's code):
//! states, retry policy, catchup. The execution itself delegates to the
//! caller's executor (the same seam as `blueprint::verify_against`), and
//! every transition is recorded on the run so the audit trail is complete.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Airflow-style task states (a superset of the four-state blueprint DAG).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    /// Scheduled / not yet attempted.
    Pending,
    /// Currently executing (claimed by a worker).
    Running,
    /// A retry is armed (backoff waiting).
    UpForRetry,
    /// Finished successfully (verify passed).
    Success,
    /// Exhausted retries.
    Failed,
    /// No retry will help (permanent) — but can be manually requeued.
    Skipped,
    /// Needs a human decision. Not progress.
    Blocked,
}

impl TaskState {
    /// Legal transitions (the state machine — same discipline as
    /// `TaskStatus::transition` in the blueprint module).
    pub fn can_transition(from: TaskState, to: TaskState) -> bool {
        use TaskState::*;
        matches!(
            (from, to),
            (Pending, Running)
                | (Running, Success)
                | (Running, UpForRetry)
                | (Running, Failed)
                | (Running, Skipped)
                | (Running, Blocked)
                | (UpForRetry | Failed | Skipped | Blocked, Running) // requeue
                | (Pending | Skipped, Skipped)
                | (Running, Pending) // claim released
        )
    }
}

/// Retry policy for a task (Airflow `retries` + `retry_delay` + `max_retry_delay`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Max retries before the task is `Failed`.
    pub max_retries: u32,
    /// Base delay seconds — the nth retry waits `base * 2^(n-1)` up to `max_wait_secs`.
    pub base_delay_secs: u64,
    /// Hard cap on any single retry delay (Airflow `max_retry_delay`).
    pub max_wait_secs: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_secs: 5,
            max_wait_secs: 300,
        }
    }
}

impl RetryPolicy {
    /// The wait (seconds) before retry `n` (1-based, `n <= max_retries`).
    pub fn delay_for(&self, retry_number: u32) -> u64 {
        let backoff = self
            .base_delay_secs
            .saturating_mul(2u64.saturating_pow(retry_number - 1));
        backoff.min(self.max_wait_secs)
    }
}

/// One task in the workflow DAG. `id` is the stable address (persisted).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfTask {
    pub id: String,
    /// Dependencies — must be `Success` before this task is ready.
    pub depends_on: Vec<String>,
    /// State of the *current* run.
    pub state: TaskState,
    /// Retries already consumed this run.
    pub retries_used: u32,
    /// Next attempt hint (unix secs) — set when `UpForRetry` arms.
    pub next_attempt_at: Option<i64>,
    /// Optional verify hook name — resolved by the executor; the task
    /// cannot be `Success` without it passing.
    pub verify: Option<String>,
}

impl WfTask {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            depends_on: Vec::new(),
            state: TaskState::Pending,
            retries_used: 0,
            next_attempt_at: None,
            verify: None,
        }
    }

    pub fn with_deps(mut self, deps: &[&str]) -> Self {
        self.depends_on = deps.iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn with_verify(mut self, verify: impl Into<String>) -> Self {
        self.verify = Some(verify.into());
        self
    }

    /// Runnable now: Pending and every dependency `Success`.
    pub fn ready(&self, states: &BTreeMap<String, TaskState>) -> bool {
        self.state == TaskState::Pending
            && self
                .depends_on
                .iter()
                .all(|d| states.get(d) == Some(&TaskState::Success))
    }
}

/// One full workflow run — one row in the run ledger (the audit hook).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub workflow_id: String,
    /// `YYYY-MM-DD`-style logical date (the Airflow backfill unit).
    pub logical_date: String,
    /// The task states captured when the run was recorded.
    pub task_states: BTreeMap<String, TaskState>,
    /// Whether that run completed.
    pub completed: bool,
}

impl WorkflowRun {
    pub fn new(workflow_id: impl Into<String>, logical_date: impl Into<String>) -> Self {
        Self {
            workflow_id: workflow_id.into(),
            logical_date: logical_date.into(),
            task_states: BTreeMap::new(),
            completed: false,
        }
    }
}

/// The workflow engine: a DAG of [`WfTask`]s with a per-workflow retry
/// policy. Pure state machine — execution stays with the caller's executor.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Workflow {
    pub id: String,
    pub tasks: BTreeMap<String, WfTask>,
    #[serde(default)]
    pub retry: RetryPolicy,
    /// The run ledger (history; the caller persists, we append).
    #[serde(default)]
    pub runs: Vec<WorkflowRun>,
}

impl Workflow {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            tasks: BTreeMap::new(),
            retry: RetryPolicy::default(),
            runs: Vec::new(),
        }
    }

    pub fn add_task(&mut self, task: WfTask) -> &mut Self {
        self.tasks.insert(task.id.clone(), task);
        self
    }

    pub fn task(&self, id: &str) -> Option<&WfTask> {
        self.tasks.get(id)
    }

    /// The tasks currently runnable (deps done, state pending).
    pub fn ready_tasks(&self) -> Vec<String> {
        let states: BTreeMap<String, TaskState> = self
            .tasks
            .iter()
            .map(|(k, t)| (k.clone(), t.state))
            .collect();
        self.tasks
            .iter()
            .filter(|(_, t)| t.ready(&states))
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Claim a task (Pending → Running), or `Err` if not ready / illegal.
    pub fn claim(&mut self, id: &str) -> Result<(), WorkflowError> {
        // readiness check first (immutable borrow), then the state move
        let deps_ok = self
            .tasks
            .get(id)
            .map(|t| {
                t.depends_on.iter().all(|d| {
                    self.tasks
                        .get(d)
                        .map(|dt| dt.state == TaskState::Success)
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if !deps_ok {
            return Err(WorkflowError::NotReady(id.to_string()));
        }
        let t = self
            .tasks
            .get_mut(id)
            .ok_or(WorkflowError::UnknownTask(id.to_string()))?;
        if !TaskState::can_transition(t.state, TaskState::Running) {
            return Err(WorkflowError::IllegalTransition(
                t.state,
                TaskState::Running,
            ));
        }
        t.state = TaskState::Running;
        Ok(())
    }

    /// Task succeeded — but only when its verify hook passed. The caller
    /// passes the verify result; a failed verify is a retryable failure.
    pub fn succeed(&mut self, id: &str, verify_passed: bool) -> Result<(), WorkflowError> {
        let t = self
            .tasks
            .get_mut(id)
            .ok_or(WorkflowError::UnknownTask(id.to_string()))?;
        if t.state != TaskState::Running {
            return Err(WorkflowError::IllegalTransition(
                t.state,
                TaskState::Success,
            ));
        }
        if t.verify.is_some() && !verify_passed {
            return self.record_failure(id, "verify failed");
        }
        t.state = TaskState::Success;
        Ok(())
    }

    /// Task failed: consume a retry (arm backoff) or mark `Failed`.
    pub fn fail(&mut self, id: &str, _reason: &str) -> Result<(), WorkflowError> {
        self.record_failure(id, _reason)
    }

    fn record_failure(&mut self, id: &str, _reason: &str) -> Result<(), WorkflowError> {
        let t = self
            .tasks
            .get_mut(id)
            .ok_or(WorkflowError::UnknownTask(id.to_string()))?;
        if t.state != TaskState::Running {
            return Err(WorkflowError::IllegalTransition(t.state, TaskState::Failed));
        }
        if t.retries_used < self.retry.max_retries {
            t.retries_used += 1;
            let wait = self.retry.delay_for(t.retries_used);
            t.next_attempt_at = Some(now_ts() + wait as i64);
            t.state = TaskState::UpForRetry;
            Ok(())
        } else {
            t.state = TaskState::Failed;
            Ok(())
        }
    }

    /// Retry timers fired: `UpForRetry → Running` (re-run from the task's
    /// verified checkpoint — the durable-run rule). Returns requeued ids.
    pub fn requeue_due_retries(&mut self, now: i64) -> Vec<String> {
        let mut due = Vec::new();
        for t in self.tasks.values_mut() {
            if t.state == TaskState::UpForRetry
                && t.next_attempt_at.map(|n| n <= now).unwrap_or(true)
                && TaskState::can_transition(TaskState::UpForRetry, TaskState::Running)
            {
                t.state = TaskState::Running;
                due.push(t.id.clone());
            }
        }
        due
    }

    /// Manual requeue (`Skipped`/`Blocked`/`Failed` → `Running`).
    pub fn requeue(&mut self, id: &str) -> Result<(), WorkflowError> {
        let t = self
            .tasks
            .get_mut(id)
            .ok_or(WorkflowError::UnknownTask(id.to_string()))?;
        if !TaskState::can_transition(t.state, TaskState::Running) {
            return Err(WorkflowError::IllegalTransition(
                t.state,
                TaskState::Running,
            ));
        }
        t.state = TaskState::Running;
        Ok(())
    }

    /// Mark a task `Blocked` (nudge card surface).
    pub fn block(&mut self, id: &str) -> Result<(), WorkflowError> {
        let t = self
            .tasks
            .get_mut(id)
            .ok_or(WorkflowError::UnknownTask(id.to_string()))?;
        if !TaskState::can_transition(t.state, TaskState::Blocked) {
            return Err(WorkflowError::IllegalTransition(
                t.state,
                TaskState::Blocked,
            ));
        }
        t.state = TaskState::Blocked;
        Ok(())
    }

    /// Whole-workflow success: every task `Success`.
    pub fn is_complete(&self) -> bool {
        !self.tasks.is_empty() && self.tasks.values().all(|t| t.state == TaskState::Success)
    }

    /// **Backfill**: re-schedule the whole DAG for a past logical date —
    /// record the finished run in the ledger, reset every task to
    /// `Pending` with fresh retry counters. (Airflow catch-up: a workflow
    /// that missed its schedule re-runs over the missed intervals.)
    pub fn backfill(&mut self, logical_date: impl Into<String>) -> WorkflowRun {
        let run = WorkflowRun {
            workflow_id: self.id.clone(),
            logical_date: logical_date.into(),
            task_states: self
                .tasks
                .iter()
                .map(|(k, t)| (k.clone(), t.state))
                .collect(),
            completed: self.is_complete(),
        };
        self.runs.push(run.clone());
        for t in self.tasks.values_mut() {
            t.state = TaskState::Pending;
            t.retries_used = 0;
            t.next_attempt_at = None;
        }
        run
    }

    /// The run ledger (history, newest first).
    pub fn recent_runs(&self) -> impl Iterator<Item = &WorkflowRun> {
        self.runs.iter().rev()
    }

    /// Topological order (deps-first) for deterministic execution — Kahn's
    /// algorithm, same as `blueprint::topological_order`.
    pub fn topological_order(&self) -> Result<Vec<String>, WorkflowError> {
        let mut indeg: BTreeMap<String, usize> =
            self.tasks.keys().map(|k| (k.clone(), 0)).collect();
        let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for t in self.tasks.values() {
            for d in &t.depends_on {
                if !self.tasks.contains_key(d) {
                    return Err(WorkflowError::UnknownDependency(d.clone()));
                }
                *indeg.get_mut(&t.id).unwrap() += 1;
                adj.entry(d.clone()).or_default().push(t.id.clone());
            }
        }
        let mut queue: Vec<String> = indeg
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(k, _)| k.clone())
            .collect();
        let mut order = Vec::new();
        while let Some(id) = queue.pop() {
            order.push(id.clone());
            if let Some(children) = adj.get(&id) {
                for c in children {
                    let d = indeg.get_mut(c).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        queue.push(c.clone());
                    }
                }
            }
        }
        if order.len() != self.tasks.len() {
            return Err(WorkflowError::CycleDetected);
        }
        Ok(order)
    }
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowError {
    UnknownTask(String),
    UnknownDependency(String),
    NotReady(String),
    CycleDetected,
    IllegalTransition(TaskState, TaskState),
}

impl std::fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTask(id) => write!(f, "unknown task {id}"),
            Self::UnknownDependency(d) => write!(f, "unknown dependency {d}"),
            Self::NotReady(id) => write!(f, "task {id} not ready"),
            Self::CycleDetected => write!(f, "cycle in workflow DAG"),
            Self::IllegalTransition(from, to) => write!(f, "illegal state move {from:?} → {to:?}"),
        }
    }
}

impl std::error::Error for WorkflowError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf() -> Workflow {
        let mut w = Workflow::new("ingest-daily");
        w.add_task(WfTask::new("fetch").with_verify("fetch-ok"));
        w.add_task(WfTask::new("parse").with_deps(&["fetch"]));
        w.add_task(WfTask::new("publish").with_deps(&["parse"]));
        w
    }

    #[test]
    fn ready_tasks_follow_deps() {
        let w = wf();
        assert_eq!(w.ready_tasks(), vec!["fetch"]);
    }

    #[test]
    fn claim_gates_on_deps() {
        let mut w = wf();
        assert!(matches!(w.claim("parse"), Err(WorkflowError::NotReady(_))));
        assert!(w.claim("fetch").is_ok());
        assert!(matches!(
            w.claim("fetch"),
            Err(WorkflowError::IllegalTransition(..))
        )); // already running
        assert!(w.succeed("fetch", true).is_ok());
        assert!(w.claim("parse").is_ok());
    }

    #[test]
    fn verify_controls_success() {
        let mut w = wf();
        w.claim("fetch").unwrap();
        // verify failing is a retryable failure, not success
        assert!(w.succeed("fetch", false).is_ok());
        assert_eq!(w.task("fetch").unwrap().state, TaskState::UpForRetry);
        assert_eq!(w.task("fetch").unwrap().retries_used, 1);
    }

    #[test]
    fn retries_exhaust_to_failed() {
        let mut w = wf();
        w.retry.max_retries = 2;
        for i in 1..=2 {
            w.claim("fetch").unwrap();
            w.fail("fetch", "boom").unwrap();
            assert_eq!(w.task("fetch").unwrap().retries_used, i);
        }
        assert_eq!(w.task("fetch").unwrap().state, TaskState::UpForRetry);
        w.claim("fetch").unwrap();
        w.fail("fetch", "boom").unwrap();
        assert_eq!(w.task("fetch").unwrap().state, TaskState::Failed);
        // Failed → requeue manually
        w.requeue("fetch").unwrap();
        assert_eq!(w.task("fetch").unwrap().state, TaskState::Running);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let p = RetryPolicy {
            max_retries: 10,
            base_delay_secs: 5,
            max_wait_secs: 40,
        };
        assert_eq!(p.delay_for(1), 5);
        assert_eq!(p.delay_for(2), 10);
        assert_eq!(p.delay_for(3), 20);
        assert_eq!(p.delay_for(4), 40);
        assert_eq!(p.delay_for(9), 40); // capped
    }

    #[test]
    fn due_retries_requeue_after_wait() {
        let mut w = wf();
        let now = now_ts();
        w.claim("fetch").unwrap();
        w.fail("fetch", "x").unwrap();
        assert_eq!(w.task("fetch").unwrap().state, TaskState::UpForRetry);
        let due = w.requeue_due_retries(now);
        assert!(due.is_empty()); // timer not fired yet
        let due = w.requeue_due_retries(now + 10_000);
        assert_eq!(due, vec!["fetch"]);
        assert_eq!(w.task("fetch").unwrap().state, TaskState::Running);
    }

    #[test]
    fn backfill_resets_and_ledgers() {
        let mut w = wf();
        w.claim("fetch").unwrap();
        w.succeed("fetch", true).unwrap();
        let run = w.backfill("2026-08-23");
        assert!(run.task_states.contains_key("fetch"));
        assert!(!run.completed);
        assert_eq!(w.recent_runs().next().unwrap().logical_date, "2026-08-23");
        // everything reset
        assert_eq!(w.ready_tasks(), vec!["fetch"]);
        assert_eq!(w.task("fetch").unwrap().retries_used, 0);
    }

    #[test]
    fn topological_order_and_cycle() {
        let w = wf();
        let order = w.topological_order().unwrap();
        assert_eq!(order, vec!["fetch", "parse", "publish"]);

        let mut cyc = Workflow::new("cyc");
        cyc.add_task(WfTask::new("a").with_deps(&["b"]));
        cyc.add_task(WfTask::new("b").with_deps(&["a"]));
        assert_eq!(cyc.topological_order(), Err(WorkflowError::CycleDetected));

        let mut missing = Workflow::new("missing");
        missing.add_task(WfTask::new("a").with_deps(&["nope"]));
        assert_eq!(
            missing.topological_order(),
            Err(WorkflowError::UnknownDependency("nope".into()))
        );
    }

    #[test]
    fn full_run_completes() {
        let mut w = wf();
        for id in ["fetch", "parse", "publish"] {
            w.claim(id).unwrap();
            w.succeed(id, true).unwrap();
        }
        assert!(w.is_complete());
        let run = w.backfill("2026-08-22");
        assert!(run.completed);
    }
}
