//! P43 — BackgroundTaskRecord detached-work ledger (spec B7 v3.53; OpenClaw
//! `tasks` pattern, pattern-only — docs.openclaw.ai/automation/tasks).
//!
//! Every detached run — automation job, subagent spawn, ACP spawn, CLI-initiated
//! run, scheduled task — raises a [`TaskRecord`] with lifecycle
//! `queued → running → terminal {succeeded, failed, timed_out, cancelled, lost}`.
//!
//! Contract (v3.53):
//! - **Completion is push-driven**: terminal transitions fire the registered
//!   watchers — the requester session/heartbeat is *woken*, never polled.
//! - **Execution ≠ delivery**: a run can be `succeeded` while its completion is
//!   still being delivered; a blocked delivery retries on a capped, fenced
//!   generation and reports `Blocked`, never `failed`.
//! - **`lost`** = no live authority and no durable run evidence after the
//!   per-runtime grace window (5-min class). Conservative offline rules never
//!   reclaim a live turn — a Running record with a fresh heartbeat is never
//!   marked lost.
//! - **Retention**: terminal records are pruned after 7 days.
//!
//! The ledger is storage-agnostic over [`TaskStore`] (in-memory for tests,
//! JSON-file for the desktop). Persistence is best-effort: a failed save is an
//! error, never a silent drop.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Push-completion hook fired on terminal transitions.
type TaskWatcher = Box<dyn Fn(&TaskRecord) + Send>;

/// Terminal records are pruned after this age (7 days).
pub const RETENTION_MS: i64 = 7 * 24 * 3600 * 1000;
/// Default lost-state grace window (5-minute class, B7).
pub const DEFAULT_LOST_GRACE_MS: i64 = 5 * 60 * 1000;

/// What kind of detached work this record describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Automation,
    Subagent,
    Acp,
    Cli,
    Scheduled,
}

/// The task lifecycle (B7 v3.53): `queued → running → terminal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    Lost,
}

impl TaskStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Succeeded
                | TaskStatus::Failed
                | TaskStatus::TimedOut
                | TaskStatus::Cancelled
                | TaskStatus::Lost
        )
    }
}

/// Delivery of the completion result to the requester. `Blocked` is a delivery
/// concern — the run itself may already be `succeeded`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Pending,
    Delivered,
    Blocked {
        /// Capped, fenced retry generation — each retry increments.
        retries: u32,
        /// Absolute deadline (ms) after which the delivery is dismissed.
        deadline_ms: i64,
    },
    Dismissed,
}

/// One detached-work task record.
///
/// P51.12 cost join: `tokens_in`/`tokens_out`/`cost_usd` accumulate the
/// vault-side spend for this task (`Vault::task_cost` → `attach_cost`). Each
/// carries `#[serde(default)]` so pre-P51.12 JSON rows (file store, coordinator
/// mirrors) still parse — missing cost fields read back as zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub kind: TaskKind,
    pub title: String,
    pub status: TaskStatus,
    /// The session/heartbeat that spawned the run (woken on completion).
    pub requester: Option<String>,
    pub created_ms: i64,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    /// Last heartbeat (ms). The lost-state reaper keys off this.
    pub last_heartbeat_ms: Option<i64>,
    pub error: Option<String>,
    /// Fence: each retry spawns a fresh record at generation+1.
    pub retry_generation: u32,
    pub delivery: DeliveryState,
    /// Accumulated input tokens charged to this task (vault join).
    #[serde(default)]
    pub tokens_in: u64,
    /// Accumulated output tokens charged to this task (vault join).
    #[serde(default)]
    pub tokens_out: u64,
    /// Accumulated $ cost charged to this task (vault join).
    #[serde(default)]
    pub cost_usd: f64,
}

impl TaskRecord {
    /// Accumulate one vault-side cost observation into the record (the
    /// task-side half of the P51.12 join — the caller feeds
    /// `Vault::task_cost(task_id)` sums here). Token counters saturate;
    /// cost adds (a task's spend is a running total, never negative).
    pub fn attach_cost(&mut self, tokens_in: u64, tokens_out: u64, cost_usd: f64) {
        self.tokens_in = self.tokens_in.saturating_add(tokens_in);
        self.tokens_out = self.tokens_out.saturating_add(tokens_out);
        self.cost_usd += cost_usd;
    }
}

/// Storage seam — the ledger never writes files itself.
pub trait TaskStore {
    fn load(&self) -> Vec<TaskRecord>;
    fn save(&mut self, records: &[TaskRecord]) -> Result<(), String>;
}

/// In-memory store (tests, single-process).
#[derive(Debug, Default)]
pub struct InMemoryStore {
    records: Vec<TaskRecord>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TaskStore for InMemoryStore {
    fn load(&self) -> Vec<TaskRecord> {
        self.records.clone()
    }
    fn save(&mut self, records: &[TaskRecord]) -> Result<(), String> {
        self.records = records.to_vec();
        Ok(())
    }
}

/// JSON-file store (desktop): atomic tmp+rename, best-effort save.
#[derive(Debug, Clone)]
pub struct FileStore {
    pub path: PathBuf,
}

impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl TaskStore for FileStore {
    fn load(&self) -> Vec<TaskRecord> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }
    fn save(&mut self, records: &[TaskRecord]) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        let json = serde_json::to_vec_pretty(records).map_err(|e| format!("encode: {e}"))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| format!("write: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("rename: {e}"))
    }
}

/// The ledger. Not `Clone` — ownership is the point.
pub struct TaskLedger {
    records: Vec<TaskRecord>,
    store: Box<dyn TaskStore + Send>,
    /// Push-completion hooks: fired on every terminal transition.
    watchers: Vec<TaskWatcher>,
    grace_ms: i64,
    next_seq: u64,
    clock: std::sync::Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl TaskLedger {
    pub fn new(store: Box<dyn TaskStore + Send>) -> Self {
        let records = store.load();
        Self {
            records,
            store,
            watchers: Vec::new(),
            grace_ms: DEFAULT_LOST_GRACE_MS,
            next_seq: 1,
            clock: std::sync::Arc::new(now_ms_default),
        }
    }

    #[cfg(test)]
    pub fn with_clock(
        store: Box<dyn TaskStore + Send>,
        now: std::sync::Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        let records = store.load();
        Self {
            records,
            store,
            watchers: Vec::new(),
            grace_ms: DEFAULT_LOST_GRACE_MS,
            next_seq: 1,
            clock: now,
        }
    }

    #[cfg(test)]
    pub fn set_grace(&mut self, grace_ms: i64) {
        self.grace_ms = grace_ms;
    }

    pub fn grace_ms(&self) -> i64 {
        self.grace_ms
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn next_id(&mut self) -> String {
        let id = format!("task-{:06}", self.next_seq);
        self.next_seq += 1;
        id
    }

    /// Register a push-completion hook. Fired synchronously on every terminal
    /// transition (the Tauri layer turns this into a `task-update` event so
    /// the UI is woken, never polling).
    pub fn watch(&mut self, cb: Box<dyn Fn(&TaskRecord) + Send>) {
        self.watchers.push(cb);
    }

    fn push_terminal(&self, record: &TaskRecord) {
        for w in &self.watchers {
            w(record);
        }
    }

    fn find_mut(&mut self, id: &str) -> Result<&mut TaskRecord, String> {
        self.records
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(|| format!("no such task: {id}"))
    }

    /// Raise a new task record (`queued`).
    pub fn enqueue(
        &mut self,
        kind: TaskKind,
        title: impl Into<String>,
        requester: Option<impl Into<String>>,
    ) -> String {
        let id = self.next_id();
        self.records.push(TaskRecord {
            id: id.clone(),
            kind,
            title: title.into(),
            status: TaskStatus::Queued,
            requester: requester.map(Into::into),
            created_ms: self.now(),
            started_ms: None,
            finished_ms: None,
            last_heartbeat_ms: None,
            error: None,
            retry_generation: 0,
            delivery: DeliveryState::Pending,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
        });
        let _ = self.persist();
        id
    }

    /// `queued → running`.
    pub fn start(&mut self, id: &str) -> Result<(), String> {
        let now = self.now();
        let r = self.find_mut(id)?;
        if r.status != TaskStatus::Queued {
            return Err(format!("cannot start {} from {:?}", id, r.status));
        }
        r.status = TaskStatus::Running;
        r.started_ms = Some(now);
        r.last_heartbeat_ms = Some(now);
        let _ = self.persist();
        Ok(())
    }

    /// Refresh the liveness heartbeat of a running task.
    pub fn heartbeat(&mut self, id: &str) -> Result<(), String> {
        let now = self.now();
        let r = self.find_mut(id)?;
        if r.status != TaskStatus::Running {
            return Err(format!("heartbeat on non-running task {id}"));
        }
        r.last_heartbeat_ms = Some(now);
        Ok(())
    }

    /// `running → terminal {succeeded, failed}`. Push-completion fires here.
    pub fn complete(&mut self, id: &str, ok: bool, error: Option<String>) -> Result<(), String> {
        let now = self.now();
        let r = self.find_mut(id)?;
        if !matches!(r.status, TaskStatus::Running | TaskStatus::Queued) {
            return Err(format!("cannot complete {} from {:?}", id, r.status));
        }
        r.status = if ok {
            TaskStatus::Succeeded
        } else {
            TaskStatus::Failed
        };
        r.finished_ms = Some(now);
        r.error = error;
        let snapshot = r.clone();
        let _ = self.persist();
        self.push_terminal(&snapshot);
        Ok(())
    }

    /// Cancel a queued or running task (`→ cancelled`).
    pub fn cancel(&mut self, id: &str) -> Result<(), String> {
        let now = self.now();
        let r = self.find_mut(id)?;
        if !matches!(r.status, TaskStatus::Queued | TaskStatus::Running) {
            return Err(format!("cannot cancel terminal task {id}"));
        }
        r.status = TaskStatus::Cancelled;
        r.finished_ms = Some(now);
        let snapshot = r.clone();
        let _ = self.persist();
        self.push_terminal(&snapshot);
        Ok(())
    }

    /// `running → timed_out` (deadline exceeded).
    pub fn timeout(&mut self, id: &str) -> Result<(), String> {
        let now = self.now();
        let r = self.find_mut(id)?;
        if r.status != TaskStatus::Running {
            return Err(format!("cannot timeout non-running task {id}"));
        }
        r.status = TaskStatus::TimedOut;
        r.finished_ms = Some(now);
        let snapshot = r.clone();
        let _ = self.persist();
        self.push_terminal(&snapshot);
        Ok(())
    }

    /// Mark a running task `lost` — only if its heartbeat is older than the
    /// grace window. A fresh heartbeat is a live turn and is never reclaimed.
    pub fn mark_lost(&mut self, id: &str, now: i64) -> Result<(), String> {
        let grace_ms = self.grace_ms;
        let r = self.find_mut(id)?;
        if r.status != TaskStatus::Running {
            return Err(format!("cannot lose non-running task {id}"));
        }
        let last = r.last_heartbeat_ms.unwrap_or(r.started_ms.unwrap_or(now));
        if now - last < grace_ms {
            return Err("heartbeat fresh — live turn, not lost".into());
        }
        r.status = TaskStatus::Lost;
        r.finished_ms = Some(now);
        let snapshot = r.clone();
        let _ = self.persist();
        self.push_terminal(&snapshot);
        Ok(())
    }

    /// Lost-state reaper: mark every running task past its grace window as
    /// `lost`. Returns the ids that transitioned.
    pub fn reap_lost(&mut self, now: i64) -> Vec<String> {
        let stale: Vec<String> = self
            .records
            .iter()
            .filter(|r| r.status == TaskStatus::Running)
            .filter(|r| {
                let last = r.last_heartbeat_ms.unwrap_or(r.started_ms.unwrap_or(now));
                now - last >= self.grace_ms
            })
            .map(|r| r.id.clone())
            .collect();
        let mut lost = Vec::new();
        for id in stale {
            if self.mark_lost(&id, now).is_ok() {
                lost.push(id);
            }
        }
        lost
    }

    // ---- delivery (execution ≠ delivery) ----------------------------------

    /// Mark a terminal run's completion delivered.
    pub fn deliver(&mut self, id: &str) -> Result<(), String> {
        let r = self.find_mut(id)?;
        if !r.status.is_terminal() {
            return Err(format!("delivery only applies to terminal tasks: {id}"));
        }
        r.delivery = DeliveryState::Delivered;
        let _ = self.persist();
        Ok(())
    }

    /// A blocked completion retries on a capped, fenced generation. The run is
    /// NOT re-reported as failed — only the delivery is re-queued.
    pub fn retry_delivery(&mut self, id: &str, deadline_ms: i64, cap: u32) -> Result<(), String> {
        let r = self.find_mut(id)?;
        if !r.status.is_terminal() {
            return Err(format!("delivery only applies to terminal tasks: {id}"));
        }
        let retries = match r.delivery {
            DeliveryState::Blocked { retries, .. } => retries + 1,
            _ => 1,
        };
        if retries > cap {
            r.delivery = DeliveryState::Dismissed;
        } else {
            r.delivery = DeliveryState::Blocked {
                retries,
                deadline_ms,
            };
        }
        let _ = self.persist();
        Ok(())
    }

    // ---- cost join (P51.12: vault `task_cost` → record) ---------------------

    /// Fold vault-side spend into a task record (the ledger-level half of the
    /// P51.12 join — the caller passes `Vault::task_cost(task_id)` sums).
    /// Applies to any lifecycle state: cost accrues while running and stays
    /// readable on terminal records.
    pub fn attach_cost(
        &mut self,
        id: &str,
        tokens_in: u64,
        tokens_out: u64,
        cost_usd: f64,
    ) -> Result<(), String> {
        let r = self.find_mut(id)?;
        r.attach_cost(tokens_in, tokens_out, cost_usd);
        let _ = self.persist();
        Ok(())
    }

    // ---- retry (fenced generation) ----------------------------------------

    /// Re-run a terminal task: raises a fresh `queued` record at the next
    /// fenced generation. The old record stays for audit.
    pub fn retry(&mut self, id: &str) -> Result<String, String> {
        let old = self
            .records
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| format!("no such task: {id}"))?;
        let new_id = self.next_id();
        self.records.push(TaskRecord {
            id: new_id.clone(),
            kind: old.kind,
            title: old.title,
            status: TaskStatus::Queued,
            requester: old.requester.clone(),
            created_ms: self.now(),
            started_ms: None,
            finished_ms: None,
            last_heartbeat_ms: None,
            error: None,
            retry_generation: old.retry_generation + 1,
            delivery: DeliveryState::Pending,
            // A fenced retry is a fresh spend scope — cost restarts at zero
            // (the old record keeps its accumulated cost for audit).
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
        });
        let _ = self.persist();
        Ok(new_id)
    }

    // ---- read + retention -------------------------------------------------

    pub fn get(&self, id: &str) -> Option<&TaskRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    pub fn list(&self) -> Vec<TaskRecord> {
        self.records.clone()
    }

    pub fn list_status(&self, status: TaskStatus) -> Vec<TaskRecord> {
        self.records
            .iter()
            .filter(|r| r.status == status)
            .cloned()
            .collect()
    }

    /// Prune terminal records older than `retention_ms` (7-day class).
    /// Returns the ids pruned.
    pub fn prune(&mut self, now: i64, retention_ms: i64) -> Vec<String> {
        let cutoff = now - retention_ms;
        let before = self.records.len();
        let pruned: Vec<String> = self
            .records
            .iter()
            .filter(|r| r.status.is_terminal())
            .filter(|r| r.finished_ms.unwrap_or(r.created_ms) < cutoff)
            .map(|r| r.id.clone())
            .collect();
        self.records.retain(|r| {
            !(r.status.is_terminal() && r.finished_ms.unwrap_or(r.created_ms) < cutoff)
        });
        if self.records.len() != before {
            let _ = self.persist();
        }
        pruned
    }

    /// Persist best-effort: a failed save is surfaced as an error, never a
    /// silent drop.
    pub fn persist(&mut self) -> Result<(), String> {
        self.store.save(&self.records)
    }

    /// JSON-RPC dispatch (`tasks/*` — the coordinator + Tauri shell drive the
    /// same ledger through this surface; Rust owns the state machine).
    pub fn handle(
        &mut self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        use serde_json::Value;
        let id = params.get("id").and_then(Value::as_str).unwrap_or_default();
        let now = self.now();
        match method {
            "tasks/list" => {
                let status = params.get("status").and_then(Value::as_str);
                let records = match status {
                    Some("queued") => self.list_status(TaskStatus::Queued),
                    Some("running") => self.list_status(TaskStatus::Running),
                    Some("terminal") => self
                        .records
                        .iter()
                        .filter(|r| r.status.is_terminal())
                        .cloned()
                        .collect(),
                    _ => self.list(),
                };
                Ok(serde_json::to_value(records).map_err(|e| e.to_string())?)
            }
            "tasks/show" => match self.get(id) {
                Some(r) => Ok(serde_json::to_value(r).map_err(|e| e.to_string())?),
                None => Err(format!("no such task: {id}")),
            },
            "tasks/enqueue" => {
                let kind: TaskKind =
                    serde_json::from_value(params.get("kind").cloned().unwrap_or(Value::Null))
                        .map_err(|e| format!("bad kind: {e}"))?;
                let title = params
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("task");
                let requester = params.get("requester").and_then(Value::as_str);
                let id = self.enqueue(kind, title, requester);
                Ok(serde_json::json!({ "id": id }))
            }
            "tasks/start" => {
                self.start(id)?;
                Ok(serde_json::json!({ "ok": true }))
            }
            "tasks/heartbeat" => {
                self.heartbeat(id)?;
                Ok(serde_json::json!({ "ok": true }))
            }
            "tasks/complete" => {
                let ok = params.get("ok").and_then(Value::as_bool).unwrap_or(false);
                let error = params
                    .get("error")
                    .and_then(Value::as_str)
                    .map(String::from);
                self.complete(id, ok, error)?;
                Ok(serde_json::json!({ "ok": true }))
            }
            "tasks/cancel" => {
                self.cancel(id)?;
                Ok(serde_json::json!({ "ok": true }))
            }
            "tasks/retry" => {
                let new = self.retry(id)?;
                Ok(serde_json::json!({ "id": new }))
            }
            "tasks/deliver" => {
                self.deliver(id)?;
                Ok(serde_json::json!({ "ok": true }))
            }
            "tasks/reap" => {
                let lost = self.reap_lost(now);
                Ok(serde_json::json!({ "lost": lost }))
            }
            "tasks/prune" => {
                let retention = params
                    .get("retentionMs")
                    .and_then(Value::as_i64)
                    .unwrap_or(RETENTION_MS);
                let pruned = self.prune(now, retention);
                Ok(serde_json::json!({ "pruned": pruned }))
            }
            _ => Err(format!("unknown tasks method: {method}")),
        }
    }
}

fn now_ms_default() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Clock {
        now: std::sync::Arc<std::sync::atomic::AtomicI64>,
    }
    impl Clock {
        fn new() -> Self {
            Self {
                now: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(1_000_000)),
            }
        }
        fn advance(&self, ms: i64) {
            self.now.fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
        }
        fn set(&self, ms: i64) {
            self.now.store(ms, std::sync::atomic::Ordering::SeqCst);
        }
        fn cur(&self) -> i64 {
            self.now.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    fn ledger() -> (TaskLedger, Clock) {
        let c = Clock::new();
        let now = c.now.clone();
        let l = TaskLedger::with_clock(
            Box::new(InMemoryStore::new()),
            std::sync::Arc::new(move || now.load(std::sync::atomic::Ordering::SeqCst)),
        );
        (l, c)
    }

    #[test]
    fn lifecycle_queued_running_succeeded() {
        let (mut l, c) = ledger();
        let id = l.enqueue(TaskKind::Automation, "digest", Some("sess-1"));
        let r = l.get(&id).unwrap();
        assert_eq!(r.status, TaskStatus::Queued);
        assert_eq!(r.retry_generation, 0);
        l.start(&id).unwrap();
        assert_eq!(l.get(&id).unwrap().status, TaskStatus::Running);
        l.complete(&id, true, None).unwrap();
        let r = l.get(&id).unwrap();
        assert_eq!(r.status, TaskStatus::Succeeded);
        assert!(r.finished_ms.is_some());
        assert_eq!(r.delivery, DeliveryState::Pending);
        // terminal transitions are delivered on the audit/requester side
        l.deliver(&id).unwrap();
        assert_eq!(l.get(&id).unwrap().delivery, DeliveryState::Delivered);
        // completing twice fails (state machine, not idempotent-silent)
        assert!(l.complete(&id, true, None).is_err());
        c.advance(1_000_000);
    }

    #[test]
    fn failed_records_error_honestly() {
        let (mut l, _) = ledger();
        let id = l.enqueue(TaskKind::Acp, "harness run", None::<String>);
        l.start(&id).unwrap();
        l.complete(&id, false, Some("timeout after 900s".into()))
            .unwrap();
        let r = l.get(&id).unwrap();
        assert_eq!(r.status, TaskStatus::Failed);
        assert_eq!(r.error.as_deref(), Some("timeout after 900s"));
    }

    #[test]
    fn cancel_works_from_queued_and_running() {
        let (mut l, _) = ledger();
        let q = l.enqueue(TaskKind::Cli, "queued job", None::<String>);
        l.cancel(&q).unwrap();
        assert_eq!(l.get(&q).unwrap().status, TaskStatus::Cancelled);

        let r = l.enqueue(TaskKind::Cli, "running job", None::<String>);
        l.start(&r).unwrap();
        l.cancel(&r).unwrap();
        assert_eq!(l.get(&r).unwrap().status, TaskStatus::Cancelled);
        // cancelling a terminal record fails
        assert!(l.cancel(&q).is_err());
    }

    #[test]
    fn timeout_is_terminal_and_distinct_from_failed() {
        let (mut l, _) = ledger();
        let id = l.enqueue(TaskKind::Subagent, "research", None::<String>);
        l.start(&id).unwrap();
        l.timeout(&id).unwrap();
        let r = l.get(&id).unwrap();
        assert_eq!(r.status, TaskStatus::TimedOut);
        assert!(r.error.is_none(), "timeout is not a failure with an error");
    }

    #[test]
    fn lost_state_respects_grace_and_never_reclaims_live_turns() {
        let (mut l, c) = ledger();
        l.set_grace(5 * 60_000);
        let live = l.enqueue(TaskKind::Scheduled, "heartbeating", None::<String>);
        l.start(&live).unwrap();
        // heartbeat at +1min — still fresh
        c.set(1_061_000);
        l.heartbeat(&live).unwrap();
        // reaper at +3min: fresh heartbeat → NOT lost
        c.set(1_180_000);
        assert!(l.reap_lost(c.cur()).is_empty());
        assert_eq!(l.get(&live).unwrap().status, TaskStatus::Running);

        // a stale run (no heartbeat for > grace) IS lost
        let stale = l.enqueue(TaskKind::Cli, "orphan", None::<String>);
        l.start(&stale).unwrap();
        c.set(1_180_000 + 6 * 60_000); // 6 min after the stale start
        // the live task heartbeats right before the reap — fresh turn, kept
        l.heartbeat(&live).unwrap();
        let lost = l.reap_lost(c.cur());
        assert!(lost.contains(&stale));
        assert_eq!(l.get(&stale).unwrap().status, TaskStatus::Lost);
        // the live one heartbeated moments ago — never reclaimed
        assert_eq!(l.get(&live).unwrap().status, TaskStatus::Running);
    }

    #[test]
    fn blocked_delivery_retries_on_fenced_generation_and_caps() {
        let (mut l, _) = ledger();
        let id = l.enqueue(TaskKind::Automation, "send digest", Some("sess-9"));
        l.start(&id).unwrap();
        l.complete(&id, true, None).unwrap();
        // execution succeeded; delivery blocked — run is NOT failed
        l.retry_delivery(&id, 1_300_000, 3).unwrap();
        let r = l.get(&id).unwrap();
        assert_eq!(r.status, TaskStatus::Succeeded);
        assert_eq!(
            r.delivery,
            DeliveryState::Blocked {
                retries: 1,
                deadline_ms: 1_300_000
            }
        );
        l.retry_delivery(&id, 1_600_000, 3).unwrap();
        l.retry_delivery(&id, 1_900_000, 3).unwrap();
        // cap 3 → fourth attempt dismisses, never misreports as failed
        l.retry_delivery(&id, 2_200_000, 3).unwrap();
        assert_eq!(l.get(&id).unwrap().delivery, DeliveryState::Dismissed);
        assert_eq!(l.get(&id).unwrap().status, TaskStatus::Succeeded);
    }

    #[test]
    fn retry_raises_a_fenced_fresh_record_keeps_audit_old() {
        let (mut l, _) = ledger();
        let id = l.enqueue(TaskKind::Subagent, "plan", Some("s-1"));
        l.start(&id).unwrap();
        l.complete(&id, false, Some("provider 5xx".into())).unwrap();
        let new = l.retry(&id).unwrap();
        assert_ne!(new, id);
        let fresh = l.get(&new).unwrap();
        assert_eq!(fresh.status, TaskStatus::Queued);
        assert_eq!(fresh.retry_generation, 1);
        // old record stays for audit
        assert_eq!(l.get(&id).unwrap().status, TaskStatus::Failed);
    }

    #[test]
    fn push_completion_fires_watchers_on_terminal_transitions() {
        use std::sync::{Arc, Mutex};
        let (mut l, _) = ledger();
        let fired: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let f2 = fired.clone();
        l.watch(Box::new(move |r: &TaskRecord| {
            f2.lock().unwrap().push(format!("{}:{:?}", r.id, r.status));
        }));
        let id = l.enqueue(TaskKind::Cli, "x", None::<String>);
        l.start(&id).unwrap();
        assert!(fired.lock().unwrap().is_empty(), "start is not terminal");
        l.complete(&id, true, None).unwrap();
        assert_eq!(
            fired.lock().unwrap().as_slice(),
            &[format!("{id}:Succeeded")]
        );
        // one terminal transition → exactly one wake
        assert_eq!(fired.lock().unwrap().len(), 1);
    }

    #[test]
    fn prune_keeps_young_and_removes_old_terminal_after_7_days() {
        let (mut l, c) = ledger();
        let young = l.enqueue(TaskKind::Scheduled, "today", None::<String>);
        l.start(&young).unwrap();
        l.complete(&young, true, None).unwrap();

        c.advance(RETENTION_MS + 60_000); // 7 days + 1 min later
        let old = l.enqueue(TaskKind::Scheduled, "old", None::<String>);
        l.start(&old).unwrap();
        l.complete(&old, true, None).unwrap();
        // a queued (non-terminal) record is never pruned
        let queued = l.enqueue(TaskKind::Cli, "pending", None::<String>);

        c.advance(60_000);
        let pruned = l.prune(c.cur(), RETENTION_MS);
        assert!(pruned.contains(&young), "young terminal pruned after 7d");
        assert!(!pruned.contains(&old), "old (just finished) kept");
        assert!(l.get(&young).is_none());
        assert!(l.get(&old).is_some());
        assert!(l.get(&queued).is_some(), "queued never pruned");
    }

    #[test]
    fn file_store_roundtrips_records() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-taskledger-{}", std::process::id()));
        let path = dir.join("tasks.json");
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut l = TaskLedger::new(Box::new(FileStore::new(&path)));
            let id = l.enqueue(TaskKind::Automation, "persisted", Some("s"));
            l.start(&id).unwrap();
            l.complete(&id, true, None).unwrap();
        }
        {
            let l = TaskLedger::new(Box::new(FileStore::new(&path)));
            let records = l.list();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].status, TaskStatus::Succeeded);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P50.3.2 — the wire contract between the Rust ledger and the TS bridge
    /// (`ui/src/lib/tasks.ts` mirrors these exact serde shapes). If this test
    /// fails, a field was renamed/added in Rust without updating the TS
    /// mirror (or vice versa) — the `tasks/*` responses would desync from the
    /// activity rail. Checked fields: names, casing (snake_case), the
    /// blocked-delivery payload shape, and the enum value spellings.
    ///
    /// P51.12 note: `tokens_in`/`tokens_out`/`cost_usd` extend this shape —
    /// the coordinator mirror (`ui/src/lib/tasks.ts` `TaskRecord` +
    /// `ui/src/lib/tasks-contract.test.ts`) must gain the same three fields.
    #[test]
    fn serialized_record_matches_ts_bridge_contract() {
        let record = TaskRecord {
            id: "task-000001".into(),
            kind: TaskKind::Scheduled,
            title: "contract".into(),
            status: TaskStatus::TimedOut,
            requester: Some("s-1".into()),
            created_ms: 1,
            started_ms: Some(2),
            finished_ms: Some(3),
            last_heartbeat_ms: Some(4),
            error: Some("boom".into()),
            retry_generation: 7,
            delivery: DeliveryState::Blocked {
                retries: 2,
                deadline_ms: 99,
            },
            tokens_in: 111,
            tokens_out: 22,
            cost_usd: 0.004,
        };
        let v = serde_json::to_value(&record).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "cost_usd",
                "created_ms",
                "delivery",
                "error",
                "finished_ms",
                "id",
                "kind",
                "last_heartbeat_ms",
                "requester",
                "retry_generation",
                "started_ms",
                "status",
                "title",
                "tokens_in",
                "tokens_out",
            ]
        );
        // Enum spellings (snake_case in both languages).
        assert_eq!(v["kind"], "scheduled");
        assert_eq!(v["status"], "timed_out");
        // Struct variant: externally-tagged → `{ blocked: { retries, deadline_ms } }`.
        assert_eq!(v["delivery"]["blocked"]["retries"], 2);
        assert_eq!(v["delivery"]["blocked"]["deadline_ms"], 99);
        // P51.12 cost join fields ride the same wire shape (snake_case).
        assert_eq!(v["tokens_in"], 111);
        assert_eq!(v["tokens_out"], 22);
        assert!((v["cost_usd"].as_f64().unwrap() - 0.004).abs() < 1e-12);

        // Unit variants of the externally-tagged enum serialize as plain
        // strings ("pending"), not {"pending": null} — the TS mirror type
        // (`ui/src/lib/tasks.ts`) matches this exactly.
        let empty = TaskRecord {
            id: "task-000002".into(),
            kind: TaskKind::Cli,
            title: "contract".into(),
            status: TaskStatus::Queued,
            requester: None,
            created_ms: 1,
            started_ms: None,
            finished_ms: None,
            last_heartbeat_ms: None,
            error: None,
            retry_generation: 0,
            delivery: DeliveryState::Pending,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
        };
        let v = serde_json::to_value(&empty).unwrap();
        // The optionals are present as null (TS: `field: T | null`), not absent.
        assert!(v["requester"].is_null());
        assert!(v["started_ms"].is_null());
        assert_eq!(v["kind"], "cli");
        assert_eq!(v["status"], "queued");
        assert_eq!(v["delivery"], "pending");
    }

    /// P51.12 — cost fields default to zero (fresh records, accumulator
    /// behavior), and pre-P51.12 JSON without the cost fields still parses
    /// (serde defaults — the file store never fails to load old rows).
    #[test]
    fn task_record_cost_defaults_zero() {
        let (mut l, _) = ledger();
        let id = l.enqueue(TaskKind::Subagent, "research", Some("s-1"));
        let r = l.get(&id).unwrap();
        assert_eq!((r.tokens_in, r.tokens_out, r.cost_usd), (0, 0, 0.0));

        // Accumulator: two observations sum.
        l.attach_cost(&id, 100, 40, 0.001).unwrap();
        l.attach_cost(&id, 50, 10, 0.0005).unwrap();
        let r = l.get(&id).unwrap();
        assert_eq!(r.tokens_in, 150);
        assert_eq!(r.tokens_out, 50);
        assert!((r.cost_usd - 0.0015).abs() < 1e-12);
        // Unknown ids fail loudly (never a silent drop).
        assert!(l.attach_cost("task-999999", 1, 1, 0.1).is_err());

        // Backward compat: old JSON (no cost fields) parses to zeros.
        let old: TaskRecord = serde_json::from_value(serde_json::json!({
            "id": "task-000007",
            "kind": "cli",
            "title": "old",
            "status": "succeeded",
            "requester": null,
            "created_ms": 1,
            "started_ms": 1,
            "finished_ms": 2,
            "last_heartbeat_ms": 2,
            "error": null,
            "retry_generation": 0,
            "delivery": "delivered",
        }))
        .unwrap();
        assert_eq!((old.tokens_in, old.tokens_out, old.cost_usd), (0, 0, 0.0));

        // A fenced retry starts a fresh spend scope; the audit row keeps its own.
        l.start(&id).unwrap();
        l.complete(&id, true, None).unwrap();
        let fresh = l.retry(&id).unwrap();
        let f = l.get(&fresh).unwrap();
        assert_eq!((f.tokens_in, f.tokens_out, f.cost_usd), (0, 0, 0.0));
        assert_eq!(l.get(&id).unwrap().tokens_in, 150);
    }
}
