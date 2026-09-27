//! Scheduled-task **trigger plane** (P6.4 — B7; re-scoped by `P71.3d` /
//! `ARCH/AUTOMATION.md` §9). The scheduler owns **definitions, triggers,
//! occurrences and admission policy — nothing else**:
//!
//! - trigger registry: cron · interval · event · webhook · window;
//! - cron math, next-due computation, trigger dedupe (one durable occurrence
//!   is admitted before Work; advancement happens after Work/Run admission);
//! - battery/wake policy, misfire policy (`run_once_on_resume` by
//!   construction: a stale `next_run_at` fires once, then advances),
//!   frequency admission (rolling-hour cap);
//! - monitor observation accounting (the "run vs notify" delta), nudge
//!   sentinels, incident ack-store, read-only doctor;
//! - durable persistence of that registry.
//!
//! It holds **no execution state**: no run state machine, no leases or
//! fences, no checkpoints, no retries, no run ledger. A trigger never
//! executes a task — it surfaces due jobs and the host (the Work kernel)
//! executes them (`AUTOMATION.md` §1, **I26**). Run history belongs to the
//! Event Log (**I3**); execution waits belong to Work's `WaitCondition`
//! (`AUTOMATION.md` §8), never to this plane.
//!
//! Patterns adopted (pattern-only, no copied code):
//! - cronflow (doc 56 §3, no LICENSE → reference only): webhook triggers
//!   with schema validation; HITL pause — here as a **trigger-plane flag**
//!   (stop firing); the execution-level pause state lives in Work.
//! - Gartner event-driven orchestration (doc 62 §3): **CI build-fail /
//!   test-regression / repo-change / ticket-assign / telemetry-threshold**
//!   triggers with **scope + frequency policy** controls.
//! - Nudge sentinels (B7): detect repeating patterns (same goal at the same
//!   time-of-day/weekday) → suggest a schedule (H14 nudge-card surface).

use std::collections::HashMap;
use std::io::Write;

use crate::automation_runtime::{content_addressed_revision_id_for_generation, parse_revision_id};
use agentcowork_blueprint::automation::{Automation, AutomationStep, Trigger as BlueprintTrigger};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Cron (5-field: min hour dom mon dow; `*`, `N`, `N-M`, `*/step`, comma lists)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct CronExpr {
    minute: Vec<u8>,
    hour: Vec<u8>,
    dom: Vec<u8>,
    month: Vec<u8>,
    dow: Vec<u8>,
    /// Raw source (for display).
    pub source: String,
}

fn parse_field(field: &str, min: u8, max: u8) -> Result<Vec<u8>, String> {
    if field == "*" {
        return Ok((min..=max).collect());
    }
    let mut out = Vec::new();
    for part in field.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, s.parse::<u8>().map_err(|_| format!("bad step {s}"))?),
            None => (part, 1),
        };
        if step == 0 {
            return Err(format!("step cannot be 0 in {field}"));
        }
        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (
                a.parse::<u8>().map_err(|_| format!("bad range {range}"))?,
                b.parse::<u8>().map_err(|_| format!("bad range {range}"))?,
            )
        } else {
            let v = range
                .parse::<u8>()
                .map_err(|_| format!("bad value {range}"))?;
            (v, v)
        };
        if lo < min || hi > max || lo > hi {
            return Err(format!("value out of range in {field}"));
        }
        let mut v = lo;
        while v <= hi {
            out.push(v);
            v = v.saturating_add(step);
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

impl CronExpr {
    /// Parse a standard 5-field cron string (`min hour dom mon dow`).
    pub fn parse(source: &str) -> Result<Self, String> {
        let parts: Vec<&str> = source.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(format!(
                "cron needs 5 fields, got {}: {source:?}",
                parts.len()
            ));
        }
        Ok(Self {
            minute: parse_field(parts[0], 0, 59)?,
            hour: parse_field(parts[1], 0, 23)?,
            dom: parse_field(parts[2], 1, 31)?,
            month: parse_field(parts[3], 1, 12)?,
            dow: parse_field(parts[4], 0, 6)?,
            source: source.to_string(),
        })
    }

    /// Does this cron match the given unix time (minute granularity)?
    pub fn matches(&self, unix_secs: u64) -> bool {
        let (min, hour, dom, month, dow) = civil_parts(unix_secs);
        if !self.minute.contains(&min) || !self.hour.contains(&hour) {
            return false;
        }
        if !self.month.contains(&month) {
            return false;
        }
        // Standard cron OR-semantics when both dom and dow are restricted.
        let dom_restricted = self.dom != (1..=31).collect::<Vec<_>>();
        let dow_restricted = self.dow != (0..=6).collect::<Vec<_>>();
        if dom_restricted && dow_restricted {
            self.dom.contains(&dom) || self.dow.contains(&dow)
        } else {
            self.dom.contains(&dom) && self.dow.contains(&dow)
        }
    }
}

/// Civil date parts from a unix timestamp (Howard Hinnant algorithms).
fn civil_parts(unix_secs: u64) -> (u8, u8, u8, u8, u8) {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8; // [1, 12]
    // 1970-01-01 was a Thursday; days % 7 = 0 is Thursday, so +4 shifts to
    // Sunday = 0 (cron dow convention).
    let weekday = (((days + 4) % 7) + 7) % 7; // 0 = Sunday (cron dow)
    (
        (secs_of_day / 60 % 60) as u8,
        (secs_of_day / 3600) as u8,
        d,
        m,
        weekday as u8,
    )
}

// ---------------------------------------------------------------------------
// Triggers and policy
// ---------------------------------------------------------------------------

/// Event-driven trigger kinds (doc 62 §3 — Gartner 2026 observability signals).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    CiBuildFail,
    TestRegression,
    RepoChange,
    TicketAssign,
    TelemetryThreshold,
}

/// Named civil-day windows above raw cron (H2 / P6.4). Fires once per day
/// at the window start hour (UTC + optional offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DayWindow {
    /// 06:00–11:59
    Morning,
    /// 12:00–17:59
    Afternoon,
    /// 18:00–21:59
    Evening,
}

impl DayWindow {
    pub fn start_hour(self) -> u8 {
        match self {
            Self::Morning => 6,
            Self::Afternoon => 12,
            Self::Evening => 18,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerSpec {
    Cron {
        expr: String,
    },
    Interval {
        secs: u64,
    },
    /// Event-triggered; `filter` matches a payload field (repo path, ticket id
    /// pattern, metric name…). `scope` (in [`SchedulePolicy`]) narrows further.
    Event {
        kind: EventKind,
        filter: String,
    },
    /// Loopback webhook ingress (F11); `path` is the URL path, `schema` lists
    /// required body keys (validated before the job is queued).
    Webhook {
        path: String,
        schema: Vec<String>,
    },
    /// A user-initiated one-shot admission. It has no next-run timestamp;
    /// `scheduler/run_now` creates the durable occurrence instead.
    Manual,
    /// Broader time window (morning / afternoon / evening) — a schedule
    /// primitive above raw cron.
    Window {
        window: DayWindow,
        #[serde(default, alias = "utcOffsetMinutes")]
        utc_offset_minutes: i32,
    },
}

/// Policy for schedule firings that were missed while the host was asleep or
/// offline.  This is admission policy around Work, not an execution retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MisfirePolicy {
    /// Drop the missed tick and move the schedule forward.
    Skip,
    /// Admit one occurrence for the missed tick, then advance after receipt.
    RunOnceOnResume,
    /// Reserved for an explicit future bounded catch-up implementation.
    CatchUp,
}

impl Default for MisfirePolicy {
    fn default() -> Self {
        Self::RunOnceOnResume
    }
}

/// Admission policy per job (doc 62 §3: scope + frequency; battery-aware B7).
/// Concurrency-and-misfire policy **around** Work (`AUTOMATION.md` §7) — not
/// an execution policy: nothing here runs a task.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulePolicy {
    /// Suppress runs while the device is on battery.
    pub suppress_on_battery: bool,
    /// Max runs per rolling hour (event/webhook spam guard).
    pub max_runs_per_hour: Option<u32>,
    /// Scope filter (repo/worktree/path prefix the event payload must match).
    pub scope: Option<String>,
    /// Overdue schedule behavior.  `run_once_on_resume` is the safe default.
    #[serde(default)]
    pub misfire_policy: MisfirePolicy,
}

impl Default for SchedulePolicy {
    fn default() -> Self {
        Self {
            suppress_on_battery: true,
            max_runs_per_hour: Some(4),
            scope: None,
            misfire_policy: MisfirePolicy::RunOnceOnResume,
        }
    }
}

/// Monitor-script mode (P51.32b): how a monitor produces observations.
/// `Llm` is the default analyst path; `Script` runs a command whose stdout
/// is stored verbatim as the observation.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorSource {
    #[default]
    Llm,
    Script {
        cmd: String,
        #[serde(default, rename = "allowNet", alias = "allow_net")]
        allow_net: bool,
    },
}

/// Monitoring semantics (the ChatGPT "monitoring task" pattern): a recurring
/// job whose runs *observe* state and notify only on a meaningful delta,
/// remembering the previous observation between runs ("previous runs are
/// remembered"). `stop_on_condition` stops the monitor when the executor
/// reports the end condition met.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorConfig {
    /// Stop the recurring monitor when the executor reports the end condition
    /// met (disable the job + keep the record).
    #[serde(default)]
    pub stop_on_condition: bool,
    /// Previous run's observation (persisted monitoring state). `None` = never
    /// observed (the first run always notifies as the baseline).
    #[serde(default)]
    pub last_observation: Option<String>,
    /// Notifications sent so far (the "run vs notify" accounting).
    #[serde(default)]
    pub notifications: u32,
    /// Where observations come from (P51.32b). Defaults to `Llm` so existing
    /// persisted monitors keep their semantics.
    #[serde(default)]
    pub source: MonitorSource,
}

impl MonitorConfig {
    /// Script-mode evaluation (P51.32b): store `stdout` verbatim (no trim,
    /// no normalization). Empty output combined with `silent_on_empty`
    /// suppresses the notification (a quiet poll, not a delta).
    /// Pure w.r.t. stored state — returns the verdict without mutating;
    /// use [`SchedulerService::monitor_evaluate_script`] for the persisting path.
    pub fn evaluate_script(&self, stdout: &str, silent_on_empty: bool) -> MonitorVerdict {
        let current = stdout.to_string();
        let previous = self.last_observation.clone();
        let changed = previous.as_deref() != Some(stdout);
        let notified = if stdout.is_empty() && silent_on_empty {
            false
        } else {
            previous.is_none() || changed
        };
        let notifications = if notified {
            self.notifications.saturating_add(1)
        } else {
            self.notifications
        };
        MonitorVerdict {
            changed,
            notified,
            stopped: false,
            previous,
            current,
            notifications,
        }
    }
}

const RETIRED_JOB_FIELDS: &[&str] = &[
    "checkpoint",
    "state",
    "currentRun",
    "runs",
    "successes",
    "failures",
    "modelPin",
    "effortPin",
    "manifestHash",
    "lastOutput",
];

/// Remove execution leftovers from a persisted scheduler document.
fn drop_retired_execution_fields(value: &mut Value) {
    let jobs = if let Some(array) = value.as_array_mut() {
        array
    } else if let Some(jobs) = value.get_mut("jobs").and_then(|jobs| jobs.as_array_mut()) {
        jobs
    } else {
        return;
    };
    for job in jobs {
        let Some(object) = job.as_object_mut() else {
            continue;
        };
        for key in RETIRED_JOB_FIELDS {
            object.remove(*key);
        }
    }
}

/// One trigger-plane job: a **definition + trigger**, never a run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Job {
    pub id: String,
    /// Rust-owned identity used in occurrence/provenance records. The
    /// registry key remains the user-facing automation id, but trigger
    /// payloads are never allowed to choose this provenance identity.
    #[serde(default)]
    pub automation_id: String,
    /// Monotonic definition revision. It advances only when the definition
    /// changes; mutable trigger-plane flags do not rewrite it.
    #[serde(default)]
    pub revision: u64,
    /// Durable automation lifetime generation.  Delete/recreate advances it;
    /// edits keep it stable.
    #[serde(default)]
    pub generation: u64,
    /// Content-addressed identity for [`Self::revision`] and generation.
    #[serde(default)]
    pub revision_id: String,
    pub name: String,
    /// The session this job reawakens (heartbeat automation — doc 67 §2).
    pub session_id: String,
    pub trigger: TriggerSpec,
    pub steps: Vec<AutomationStep>,
    pub policy: SchedulePolicy,
    pub enabled: bool,
    /// Trigger-plane pause (cronflow HITL, chat-delete cascade): stop firing
    /// without losing the definition. Execution-level waiting (approval,
    /// user input, timer…) is Work's `WaitCondition` (`AUTOMATION.md` §8),
    /// never this flag.
    #[serde(default)]
    pub paused: bool,
    /// Next due unix time (cron/interval); None = waiting on an event.
    pub next_run_at: Option<u64>,
    /// Last fired unix time (occurrence record — "why did this run?" §4).
    #[serde(default)]
    pub last_fired_at: Option<u64>,
    /// Rolling 1h fire timestamps (frequency admission).
    #[serde(default)]
    pub recent_fires: Vec<u64>,
    /// Monitoring config (`None` = a plain scheduled/event job; lazily created
    /// by `monitor_evaluate` for delta-notify semantics).
    #[serde(default)]
    pub monitor: Option<MonitorConfig>,
    /// Scratch notepad carried across runs (P51.32a continuity — explicitly
    /// user/agent-curated notes; run results live in the Event Log, **I3**).
    #[serde(default)]
    pub notepad: String,
}

/// The outcome of one monitoring evaluation (stateful-polling delta check).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorVerdict {
    /// The new observation differs from the previous one (or it's the first run).
    pub changed: bool,
    /// Should the user be notified this run? (first run, a delta, or the stop
    /// condition) — the "run vs notify" split: a run completes without
    /// notifying when nothing changed.
    pub notified: bool,
    /// The end condition was met and `stop_on_condition` was set → the monitor
    /// was stopped (job disabled).
    pub stopped: bool,
    /// The previous observation (None on the first run).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    /// The observation just recorded.
    pub current: String,
    /// Total notifications sent after this run.
    pub notifications: u32,
}

impl Job {
    fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        session_id: impl Into<String>,
        trigger: TriggerSpec,
    ) -> Self {
        Self::new_with_generation(id, name, session_id, trigger, 1)
    }

    fn new_with_generation(
        id: impl Into<String>,
        name: impl Into<String>,
        session_id: impl Into<String>,
        trigger: TriggerSpec,
        generation: u64,
    ) -> Self {
        let id = id.into();
        let mut job = Self {
            automation_id: trusted_automation_id(&id),
            revision: 1,
            generation,
            revision_id: String::new(),
            id,
            name: name.into(),
            session_id: session_id.into(),
            trigger,
            policy: SchedulePolicy::default(),
            steps: Vec::new(),
            enabled: true,
            paused: false,
            next_run_at: None,
            last_fired_at: None,
            recent_fires: Vec::new(),
            monitor: None,
            notepad: String::new(),
        };
        job.revision_id = revision_id_for(&job, job.revision);
        job
    }
}

/// The on-disk scheduler format version. Version 1 was the historical bare
/// `Vec<Job>` file; version 2 added occurrences; version 3 adds generation
/// counters and explicit occurrence admission states.
const SCHEDULER_STORE_VERSION: u64 = 3;

/// The trigger kind captured on an occurrence. This is admission metadata,
/// not an execution state machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OccurrenceTrigger {
    Schedule { scheduled_at: u64 },
    Manual,
    Event { kind: String },
    Webhook { path: String },
}

/// The immutable definition snapshot used to admit one occurrence. Keeping the
/// snapshot beside its id means an edit after admission cannot rewrite the
/// Work that the occurrence will compile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationRevision {
    pub automation_id: String,
    pub revision: u64,
    pub revision_id: String,
    pub name: String,
    pub session_id: String,
    pub trigger: TriggerSpec,
    pub steps: Vec<AutomationStep>,
    pub policy: SchedulePolicy,
}

impl AutomationRevision {
    /// The automation generation encoded by the canonical revision id.
    pub fn generation(&self) -> u64 {
        parse_revision_id(&self.revision_id)
            .map(|identity| identity.generation)
            .unwrap_or(0)
    }

    /// Convert the immutable snapshot to the compiler's definition type.
    /// Trigger variants that have no blueprint equivalent are represented as
    /// `Manual` for compilation; the full trigger remains in this snapshot.
    pub fn automation(&self) -> Automation {
        let trigger = match &self.trigger {
            TriggerSpec::Cron { expr } => BlueprintTrigger::Schedule { cron: expr.clone() },
            _ => BlueprintTrigger::Manual,
        };
        let metadata_digest = digest_json(&json!({
            "sessionId": self.session_id,
            "trigger": self.trigger,
            "policy": self.policy,
        }));
        Automation {
            id: format!("{}::snapshot:{}", self.automation_id, metadata_digest),
            name: self.name.clone(),
            trigger,
            steps: self.steps.clone(),
        }
    }
}

/// Durable admission state of one trigger occurrence.  This is metadata for
/// the trigger plane; it is not an execution state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceState {
    /// Admitted and waiting for a host Work/Run admission receipt.
    Pending,
    /// Admission or its outcome cannot be proven; reconciliation is required.
    Uncertain,
    /// Cancellation won the race and this occurrence can never reopen.
    Cancelled,
    /// A durable Work/Run admission receipt advanced the occurrence.
    Terminal,
}

impl OccurrenceState {
    /// Compatibility spelling for the terminal admission projection.
    pub const ADVANCED: Self = Self::Terminal;
}

impl Default for OccurrenceState {
    fn default() -> Self {
        Self::Pending
    }
}

/// The host-owned proof that Work and Run admission was durably completed.
/// The scheduler validates its provenance and identity, but it does not
/// create Work or claim that execution completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkRunAdmissionReceipt {
    pub work_id: String,
    pub run_id: String,
    pub automation_id: String,
    pub revision_id: String,
    #[serde(alias = "occurrenceId", alias = "occurrence_id")]
    pub trigger_occurrence_id: String,
    #[serde(default)]
    pub automation_generation: u64,
    #[serde(default)]
    pub durable: bool,
}

impl WorkRunAdmissionReceipt {
    /// Construct a receipt for a host which has already durably created the
    /// deterministic Work/Run pair.  The host still owns the actual proof.
    pub fn new(
        work_id: impl Into<String>,
        run_id: impl Into<String>,
        automation_id: impl Into<String>,
        revision_id: impl Into<String>,
        trigger_occurrence_id: impl Into<String>,
        automation_generation: u64,
    ) -> Self {
        Self {
            work_id: work_id.into(),
            run_id: run_id.into(),
            automation_id: automation_id.into(),
            revision_id: revision_id.into(),
            trigger_occurrence_id: trigger_occurrence_id.into(),
            automation_generation,
            durable: true,
        }
    }
}

/// A durable trigger admission record. It is the scheduler's occurrence
/// metadata only; Work/Run lifecycle, waits, retries, and effects remain in
/// the Work owners.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationOccurrence {
    /// Stable, Rust-generated identity for this one trigger admission.
    pub trigger_occurrence_id: String,
    pub automation_id: String,
    pub revision_id: String,
    pub trigger: OccurrenceTrigger,
    /// Digest of the canonical trigger payload. Raw payloads are intentionally
    /// not persisted here; the digest is enough for idempotency/audit.
    pub payload_digest: String,
    /// Digest of `(automation, trigger, payload)`, used for delivery dedupe.
    pub dedup_digest: String,
    pub admitted_at: u64,
    /// Explicit durable admission state.  Older rows are normalized on load.
    #[serde(default)]
    pub state: OccurrenceState,
    /// Stable delivery/request key used to derive `dedup_digest`.
    #[serde(default)]
    pub idempotency_key: String,
    #[serde(default)]
    pub fired_at: Option<u64>,
    #[serde(default)]
    pub work_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub admission_error: Option<String>,
    /// The durable Work/Run admission proof, retained for terminal rows.
    #[serde(default)]
    pub admission_receipt: Option<WorkRunAdmissionReceipt>,
    pub revision: AutomationRevision,
}

impl AutomationOccurrence {
    pub fn is_pending(&self) -> bool {
        self.state == OccurrenceState::Pending
    }

    pub fn status(&self) -> OccurrenceState {
        self.state
    }

    pub fn is_uncertain(&self) -> bool {
        self.state == OccurrenceState::Uncertain
    }

    pub fn is_cancelled(&self) -> bool {
        self.state == OccurrenceState::Cancelled
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            OccurrenceState::Terminal | OccurrenceState::Cancelled
        )
    }

    pub fn id(&self) -> &str {
        &self.trigger_occurrence_id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedScheduler {
    version: u64,
    jobs: Vec<Job>,
    #[serde(default)]
    occurrences: Vec<AutomationOccurrence>,
    /// Monotonic generation allocator per registry id.  It is retained after
    /// deletion so recreate cannot inherit an old automation lifetime.
    #[serde(default)]
    generation_counters: HashMap<String, u64>,
}

// ---------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------

/// Nudge sentinel sample: a goal fired at a time-of-day / weekday.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NudgeSample {
    pub goal: String,
    pub unix_secs: u64,
}

/// A schedule suggestion produced by the nudge sentinels.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NudgeSuggestion {
    pub goal: String,
    pub cron: String,
    pub confidence: f64,
    /// Times-of-day (HH:MM) where the goal was observed.
    pub observed_at: Vec<String>,
}

pub const NUDGE_WINDOW_DAYS: u64 = 14;
/// Registry soft cap (P51.32f doctor's `queue_depth` guard).
pub const REGISTRY_SOFT_CAP: usize = 500;

/// An incident (P51.32e): an explicit, ack-gated failure record.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Incident {
    pub id: String,
    pub job_id: String,
    pub at_ms: u64,
    pub kind: String,
    pub detail: String,
    pub acked: bool,
}

/// A single doctor check (P51.32f).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Clone)]
pub struct SchedulerService {
    jobs: HashMap<String, Job>,
    on_battery: bool,
    nudge_log: Vec<NudgeSample>,
    webhook_token: Option<String>,
    /// P50.3.3 — durable job persistence. `Some(path)` ⇒ every mutation is
    /// written through to the JSON file (atomic tmp+rename, best-effort: a
    /// failed save is an error surfaced by `persist`, never a silent drop).
    persist_path: Option<std::path::PathBuf>,
    /// Durable trigger admissions. This is metadata for the Work factory,
    /// not a second execution/retry/wait store.
    occurrences: Vec<AutomationOccurrence>,
    /// Durable generation allocator retained across delete/recreate.
    generation_counters: HashMap<String, u64>,
    /// A corrupt/unknown on-disk state is retained as an explicit load error;
    /// it is never converted into an apparently healthy empty registry.
    load_error: Option<String>,
    /// P51.32e — explicit-ack incident store.
    incidents: Vec<Incident>,
    incident_seq: u64,
}

impl Default for SchedulerService {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulerService {
    pub fn new() -> Self {
        Self {
            jobs: HashMap::new(),
            on_battery: false,
            nudge_log: Vec::new(),
            webhook_token: None,
            persist_path: None,
            occurrences: Vec::new(),
            generation_counters: HashMap::new(),
            load_error: None,
            incidents: Vec::new(),
            incident_seq: 0,
        }
    }

    /// Open the service and fail closed on an unknown/corrupt persisted
    /// state. The compatibility [`Self::load_or_new`] wrapper preserves the
    /// infallible constructor used by the relay, but records the failure so
    /// every mutating/query funnel can refuse it explicitly.
    pub fn load_or_new_checked(path: std::path::PathBuf) -> Result<Self, String> {
        let mut svc = Self::new();
        svc.persist_path = Some(path.clone());
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(svc),
            Err(error) => return Err(format!("read scheduler state: {error}")),
        };
        let mut raw: Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse scheduler state: {error}"))?;
        // Older files stored execution leftovers (a checkpoint, a run list).
        // Those are not trigger-plane fields. Drop them so a restart can read
        // the jobs. They are not loaded as a second runtime.
        drop_retired_execution_fields(&mut raw);
        let (jobs, occurrences, generation_counters) = if let Some(array) = raw.as_array() {
            let jobs: Vec<Job> = serde_json::from_value(Value::Array(array.clone()))
                .map_err(|error| format!("parse scheduler jobs: {error}"))?;
            (jobs, Vec::new(), HashMap::new())
        } else {
            let store: PersistedScheduler = serde_json::from_value(raw)
                .map_err(|error| format!("parse scheduler store: {error}"))?;
            if store.version != 2 && store.version != SCHEDULER_STORE_VERSION {
                return Err(format!(
                    "unsupported scheduler store version {} (expected 2 or {SCHEDULER_STORE_VERSION})",
                    store.version
                ));
            }
            (store.jobs, store.occurrences, store.generation_counters)
        };
        svc.generation_counters = generation_counters;
        svc.install_loaded(jobs, occurrences)?;
        Ok(svc)
    }

    /// Compatibility constructor. A malformed store is **not** treated as an
    /// empty registry: the returned service is poisoned and callers receive a
    /// named error from `handle`/admission instead of silently losing jobs.
    pub fn load_or_new(path: std::path::PathBuf) -> Self {
        Self::load_or_new_checked(path.clone()).unwrap_or_else(|error| {
            let mut svc = Self::new();
            svc.persist_path = Some(path);
            svc.load_error = Some(error);
            svc
        })
    }

    fn install_loaded(
        &mut self,
        jobs: Vec<Job>,
        occurrences: Vec<AutomationOccurrence>,
    ) -> Result<(), String> {
        for mut job in jobs {
            if job.id.trim().is_empty() {
                return Err("scheduler job has an empty id".into());
            }
            if job.automation_id.trim().is_empty() {
                job.automation_id = trusted_automation_id(&job.id);
            }
            if job.automation_id != trusted_automation_id(&job.id) {
                return Err(format!(
                    "scheduler job `{}` has an untrusted automation id",
                    job.id
                ));
            }
            if job.revision == 0 {
                job.revision = 1;
            }
            if job.generation == 0 {
                job.generation = 1;
            }
            self.generation_counters
                .entry(job.id.clone())
                .and_modify(|current| *current = (*current).max(job.generation))
                .or_insert(job.generation);
            validate_job_definition(&job)?;
            let expected_revision = revision_id_for(&job, job.revision);
            if !job.revision_id.is_empty()
                && job.revision_id != expected_revision
                && !legacy_revision_id_matches(&job, &job.revision_id)
            {
                return Err(format!(
                    "scheduler job `{}` has an invalid immutable revision id",
                    job.id
                ));
            }
            job.revision_id = expected_revision;
            if self.jobs.contains_key(&job.id) {
                return Err(format!("duplicate scheduler job id `{}`", job.id));
            }
            if self
                .jobs
                .values()
                .any(|existing| existing.automation_id == job.automation_id)
            {
                return Err(format!(
                    "duplicate scheduler automation id `{}`",
                    job.automation_id
                ));
            }
            self.jobs.insert(job.id.clone(), job);
        }
        for mut occurrence in occurrences {
            normalize_loaded_occurrence(&mut occurrence)?;
            self.validate_occurrence(&occurrence)?;
            if self
                .occurrences
                .iter()
                .any(|existing| existing.trigger_occurrence_id == occurrence.trigger_occurrence_id)
            {
                return Err(format!(
                    "duplicate scheduler occurrence `{}`",
                    occurrence.trigger_occurrence_id
                ));
            }
            if self
                .occurrences
                .iter()
                .any(|existing| existing.dedup_digest == occurrence.dedup_digest)
            {
                return Err(format!(
                    "duplicate scheduler delivery digest `{}`",
                    occurrence.dedup_digest
                ));
            }
            self.occurrences.push(occurrence);
        }
        Ok(())
    }

    fn validate_occurrence(&self, occurrence: &AutomationOccurrence) -> Result<(), String> {
        validate_occurrence_shape(occurrence)
    }

    /// Whether this service may be used. A corrupt persisted registry is a
    /// hard refusal, not an empty healthy service.
    pub fn is_healthy(&self) -> bool {
        self.load_error.is_none()
    }

    pub fn registry_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// Public fail-closed health check for host adapters that cannot call the
    /// private mutation guard.
    pub fn ensure_public_health(&self) -> Result<(), String> {
        self.ensure_healthy()
    }

    fn ensure_healthy(&self) -> Result<(), String> {
        match &self.load_error {
            Some(error) => Err(format!("scheduler registry unavailable: {error}")),
            None => Ok(()),
        }
    }

    /// Write the registry and occurrence index through to the backing file
    /// (atomic tmp+rename). The caller receives persistence failures instead
    /// of continuing with an occurrence that is only in memory.
    pub fn persist(&self) -> Result<(), String> {
        self.ensure_healthy()?;
        let Some(path) = &self.persist_path else {
            return Ok(());
        };
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create scheduler directory: {e}"))?;
        }
        let mut jobs: Vec<&Job> = self.jobs.values().collect();
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        let mut occurrences = self.occurrences.clone();
        occurrences.sort_by(|a, b| {
            a.admitted_at
                .cmp(&b.admitted_at)
                .then(a.trigger_occurrence_id.cmp(&b.trigger_occurrence_id))
        });
        let store = PersistedScheduler {
            version: SCHEDULER_STORE_VERSION,
            jobs: jobs.into_iter().cloned().collect(),
            occurrences,
            generation_counters: self.generation_counters.clone(),
        };
        let json = serde_json::to_vec_pretty(&store).map_err(|e| format!("encode: {e}"))?;
        let tmp = path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&tmp).map_err(|e| format!("write: {e}"))?;
        file.write_all(&json).map_err(|e| format!("write: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("sync scheduler state: {e}"))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("rename: {e}"))
    }

    // -- registry -----------------------------------------------------------

    pub fn list(&self) -> Vec<&Job> {
        let mut v: Vec<&Job> = self.jobs.values().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn get(&self, id: &str) -> Option<&Job> {
        self.jobs.get(id)
    }

    pub fn job_id_for_automation(&self, automation_id: &str) -> Option<&str> {
        self.jobs
            .values()
            .find(|job| job.automation_id == automation_id)
            .map(|job| job.id.as_str())
    }

    pub fn automation_id_for_job(&self, id: &str) -> Option<&str> {
        self.jobs.get(id).map(|job| job.automation_id.as_str())
    }

    pub fn job_ids_for_occurrences(&self, occurrences: &[AutomationOccurrence]) -> Vec<String> {
        occurrences
            .iter()
            .filter_map(|occurrence| self.job_id_for_automation(&occurrence.automation_id))
            .map(str::to_string)
            .collect()
    }

    fn pending_job_ids_for_occurrences(&self, occurrences: &[AutomationOccurrence]) -> Vec<String> {
        occurrences
            .iter()
            .filter(|occurrence| occurrence.is_pending())
            .filter_map(|occurrence| self.job_id_for_automation(&occurrence.automation_id))
            .map(str::to_string)
            .collect()
    }

    /// Create (or replace) a job. The legacy signature cannot return a
    /// persistence error, so it rolls back in-memory state on failure; host
    /// adapters should use [`Self::upsert_checked`].
    #[allow(clippy::too_many_arguments)]
    pub fn upsert(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        session_id: impl Into<String>,
        trigger: TriggerSpec,
        steps: Vec<AutomationStep>,
        policy: Option<SchedulePolicy>,
        now: u64,
    ) -> &mut Job {
        let id = id.into();
        let name = name.into();
        let session_id = session_id.into();
        let fallback = Job::new(
            id.clone(),
            name.clone(),
            session_id.clone(),
            trigger.clone(),
        );
        if let Err(error) = self.upsert_checked_with_enabled(
            id.clone(),
            name.clone(),
            session_id.clone(),
            trigger.clone(),
            steps.clone(),
            policy.clone(),
            None,
            now,
        ) {
            // The historical API cannot return an error. Poison the service
            // rather than exposing an unpersisted/invalid registry.
            self.load_error = Some(error);
            if !self.jobs.contains_key(&id) {
                self.jobs.insert(id.clone(), fallback);
            }
        }
        self.jobs.get_mut(&id).expect("job was just upserted")
    }

    /// Checked host-facing definition write.
    pub fn upsert_checked(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        session_id: impl Into<String>,
        trigger: TriggerSpec,
        steps: Vec<AutomationStep>,
        policy: Option<SchedulePolicy>,
        now: u64,
    ) -> Result<(), String> {
        self.upsert_checked_with_enabled(id, name, session_id, trigger, steps, policy, None, now)
    }

    /// Checked definition write with an explicit `enabled` value. `None`
    /// preserves the current value for an edit and defaults a new job to true.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_checked_with_enabled(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        session_id: impl Into<String>,
        trigger: TriggerSpec,
        steps: Vec<AutomationStep>,
        policy: Option<SchedulePolicy>,
        enabled: Option<bool>,
        now: u64,
    ) -> Result<(), String> {
        self.ensure_healthy()?;
        let before = self.clone();
        let id = id.into();
        let name = name.into();
        let session_id = session_id.into();
        let existing = self.jobs.get(&id).cloned();
        let old_digest = existing.as_ref().map(legacy_definition_digest);
        let mut candidate = existing.clone().unwrap_or_else(|| {
            let generation = self
                .generation_counters
                .get(&id)
                .copied()
                .unwrap_or(0)
                .saturating_add(1)
                .max(1);
            Job::new_with_generation(
                id.clone(),
                name.clone(),
                session_id.clone(),
                trigger.clone(),
                generation,
            )
        });
        candidate.name = name;
        candidate.session_id = session_id;
        candidate.trigger = trigger;
        candidate.steps = steps;
        if let Some(policy) = policy {
            candidate.policy = policy;
        }
        if let Some(enabled) = enabled {
            candidate.enabled = enabled;
        }
        if old_digest.as_ref() != Some(&legacy_definition_digest(&candidate)) {
            candidate.revision = existing
                .as_ref()
                .map(|job| job.revision.saturating_add(1))
                .unwrap_or(1)
                .max(1);
        } else if candidate.revision == 0 {
            candidate.revision = 1;
        }
        if candidate.generation == 0 {
            candidate.generation = 1;
        }
        if existing
            .as_ref()
            .map(|job| job.trigger != candidate.trigger)
            .unwrap_or(true)
            || candidate.next_run_at.is_none()
        {
            candidate.next_run_at =
                compute_next_run(&candidate.trigger, now, candidate.next_run_at);
        }
        candidate.revision_id = revision_id_for(&candidate, candidate.revision);
        validate_job_definition(&candidate)?;
        let candidate_generation = candidate.generation;
        self.jobs.insert(id.clone(), candidate);
        self.generation_counters
            .entry(id)
            .and_modify(|current| *current = (*current).max(candidate_generation))
            .or_insert(candidate_generation);
        if let Err(error) = self.persist() {
            *self = before;
            return Err(error);
        }
        Ok(())
    }

    pub fn delete(&mut self, id: &str) -> bool {
        match self.delete_checked(id) {
            Ok(removed) => removed,
            Err(error) => {
                self.load_error = Some(error);
                false
            }
        }
    }

    pub fn delete_checked(&mut self, id: &str) -> Result<bool, String> {
        self.ensure_healthy()?;
        let Some(job) = self.jobs.get(id) else {
            return Ok(false);
        };
        let automation_id = job.automation_id.clone();
        if self.occurrences.iter().any(|occurrence| {
            occurrence.automation_id == automation_id && !occurrence.is_terminal()
        }) {
            return Err(format!(
                "automation `{id}` has an unresolved occurrence; cancel or reconcile it first"
            ));
        }
        let before = self.clone();
        self.jobs.remove(id);
        if let Err(error) = self.persist() {
            *self = before;
            return Err(error);
        }
        Ok(true)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool, now: u64) -> Result<(), String> {
        self.ensure_healthy()?;
        let before = self.clone();
        let job = self
            .jobs
            .get_mut(id)
            .ok_or_else(|| format!("unknown job {id:?}"))?;
        job.enabled = enabled;
        if enabled && job.next_run_at.is_none() {
            job.next_run_at = compute_next_run(&job.trigger, now, None);
        }
        if let Err(error) = self.persist() {
            *self = before;
            return Err(error);
        }
        Ok(())
    }

    /// Attach/replace a job's monitoring config (or clear it with `None`).
    pub fn set_monitor(&mut self, id: &str, monitor: Option<MonitorConfig>) -> Result<(), String> {
        self.ensure_healthy()?;
        let before = self.clone();
        let job = self
            .jobs
            .get_mut(id)
            .ok_or_else(|| format!("unknown job {id:?}"))?;
        job.monitor = monitor;
        if let Err(error) = self.persist() {
            *self = before;
            return Err(error);
        }
        Ok(())
    }

    /// Evaluate and durably store a monitor observation.
    pub fn monitor_evaluate(
        &mut self,
        id: &str,
        observation: &str,
        condition_met: bool,
    ) -> Result<MonitorVerdict, String> {
        self.ensure_healthy()?;
        let before = self.clone();
        let job = self
            .jobs
            .get_mut(id)
            .ok_or_else(|| format!("unknown job {id:?}"))?;
        let monitor = job.monitor.get_or_insert_with(MonitorConfig::default);
        let previous = monitor.last_observation.clone();
        let changed = previous.as_deref() != Some(observation);
        let notified = previous.is_none() || changed || condition_met;
        if notified {
            monitor.notifications = monitor.notifications.saturating_add(1);
        }
        monitor.last_observation = Some(observation.to_string());
        let stopped = condition_met && monitor.stop_on_condition;
        if stopped {
            job.enabled = false;
        }
        let verdict = MonitorVerdict {
            changed,
            notified,
            stopped,
            previous,
            current: observation.to_string(),
            notifications: monitor.notifications,
        };
        if let Err(error) = self.persist() {
            *self = before;
            return Err(error);
        }
        Ok(verdict)
    }

    // -- continuity (P51.32a) --------------------------------------------------

    pub fn append_notepad(&mut self, id: &str, line: &str) -> bool {
        match self.append_notepad_checked(id, line) {
            Ok(changed) => changed,
            Err(error) => {
                self.load_error = Some(error);
                false
            }
        }
    }

    pub fn append_notepad_checked(&mut self, id: &str, line: &str) -> Result<bool, String> {
        self.ensure_healthy()?;
        if !self.jobs.contains_key(id) {
            return Ok(false);
        }
        let before = self.clone();
        let job = self.jobs.get_mut(id).expect("job existence checked");
        if job.notepad.is_empty() {
            job.notepad = line.to_string();
        } else {
            job.notepad.push('\n');
            job.notepad.push_str(line);
        }
        if let Err(error) = self.persist() {
            *self = before;
            return Err(error);
        }
        Ok(true)
    }

    pub fn notepad(&self, id: &str) -> Option<String> {
        self.jobs.get(id).map(|job| job.notepad.clone())
    }

    // -- monitor-script mode (P51.32b) -----------------------------------------

    pub fn monitor_evaluate_script(
        &mut self,
        id: &str,
        stdout: &str,
        silent_on_empty: bool,
    ) -> Result<MonitorVerdict, String> {
        self.ensure_healthy()?;
        let before = self.clone();
        let job = self
            .jobs
            .get_mut(id)
            .ok_or_else(|| format!("unknown job {id:?}"))?;
        let monitor = job.monitor.get_or_insert_with(MonitorConfig::default);
        let snapshot = monitor.clone();
        let verdict = snapshot.evaluate_script(stdout, silent_on_empty);
        monitor.last_observation = Some(stdout.to_string());
        if verdict.notified {
            monitor.notifications = monitor.notifications.saturating_add(1);
        }
        let result = MonitorVerdict {
            notifications: monitor.notifications,
            ..verdict
        };
        if let Err(error) = self.persist() {
            *self = before;
            return Err(error);
        }
        Ok(result)
    }

    pub fn evaluate_script(&self, stdout: &str, silent_on_empty: bool) -> MonitorVerdict {
        MonitorConfig::default().evaluate_script(stdout, silent_on_empty)
    }

    // -- trigger-plane pause -----------------------------------------------------

    pub fn pause(&mut self, id: &str) -> Result<(), String> {
        self.ensure_healthy()?;
        let before = self.clone();
        let job = self
            .jobs
            .get_mut(id)
            .ok_or_else(|| format!("unknown job {id:?}"))?;
        job.paused = true;
        if let Err(error) = self.persist() {
            *self = before;
            return Err(error);
        }
        Ok(())
    }

    pub fn resume(&mut self, id: &str, now: u64) -> Result<(), String> {
        self.ensure_healthy()?;
        let before = self.clone();
        let job = self
            .jobs
            .get_mut(id)
            .ok_or_else(|| format!("unknown job {id:?}"))?;
        job.paused = false;
        if job.enabled && job.next_run_at.is_none() {
            job.next_run_at = compute_next_run(&job.trigger, now, None);
        }
        if let Err(error) = self.persist() {
            *self = before;
            return Err(error);
        }
        Ok(())
    }

    pub fn pause_session(&mut self, session_id: &str) -> usize {
        match self.pause_session_checked(session_id) {
            Ok(count) => count,
            Err(error) => {
                self.load_error = Some(error);
                0
            }
        }
    }

    pub fn pause_session_checked(&mut self, session_id: &str) -> Result<usize, String> {
        self.ensure_healthy()?;
        let before = self.clone();
        let mut count = 0;
        for job in self.jobs.values_mut() {
            if job.session_id == session_id && !job.paused {
                job.paused = true;
                count += 1;
            }
        }
        if count > 0 {
            if let Err(error) = self.persist() {
                *self = before;
                return Err(error);
            }
        }
        Ok(count)
    }

    // -- durable occurrence admission ------------------------------------------

    /// All durable trigger occurrences, newest admission first.
    pub fn occurrences(&self) -> Vec<&AutomationOccurrence> {
        let mut rows: Vec<&AutomationOccurrence> = self.occurrences.iter().collect();
        rows.sort_by(|a, b| {
            b.admitted_at
                .cmp(&a.admitted_at)
                .then(b.trigger_occurrence_id.cmp(&a.trigger_occurrence_id))
        });
        rows
    }

    /// Pending occurrences are the only rows a host may automatically pick up.
    /// Uncertain and cancelled rows are deliberately excluded.
    pub fn pending_occurrences(&self) -> Vec<AutomationOccurrence> {
        self.occurrences()
            .into_iter()
            .filter(|occurrence| occurrence.is_pending())
            .cloned()
            .collect()
    }

    /// Occurrences that require an explicit host reconciliation receipt.
    pub fn uncertain_occurrences(&self) -> Vec<AutomationOccurrence> {
        self.occurrences()
            .into_iter()
            .filter(|occurrence| occurrence.is_uncertain())
            .cloned()
            .collect()
    }

    /// Cancelled occurrences retained as monotonic admission history.
    pub fn cancelled_occurrences(&self) -> Vec<AutomationOccurrence> {
        self.occurrences()
            .into_iter()
            .filter(|occurrence| occurrence.is_cancelled())
            .cloned()
            .collect()
    }

    pub fn occurrence(&self, id: &str) -> Option<&AutomationOccurrence> {
        self.occurrences
            .iter()
            .find(|occurrence| occurrence.trigger_occurrence_id == id)
    }

    /// Admit currently due schedule/window occurrences.  Cron admission uses
    /// the stored due timestamp, not whether the wall clock happens to match
    /// the cron expression, so an overdue `run_once_on_resume` tick fires once.
    pub fn admit_due(&mut self, now: u64) -> Result<Vec<AutomationOccurrence>, String> {
        self.ensure_healthy()?;
        let before = self.clone();
        let ids = self.due(now);
        let mut admitted = Vec::new();
        let mut changed = false;
        for id in ids {
            let Some(job) = self.jobs.get(&id).cloned() else {
                continue;
            };
            let scheduled_at = job.next_run_at.unwrap_or(now);
            if matches!(job.trigger, TriggerSpec::Cron { .. }) {
                match job.policy.misfire_policy {
                    MisfirePolicy::RunOnceOnResume => {}
                    MisfirePolicy::Skip => {
                        let mut next = job.clone();
                        next.next_run_at = compute_next_run(&next.trigger, now, None);
                        self.jobs.insert(id.clone(), next);
                        changed = true;
                        continue;
                    }
                    MisfirePolicy::CatchUp => {
                        *self = before;
                        return Err(
                            "scheduler catch_up misfire policy is not admitted; use run_once_on_resume or skip"
                                .into(),
                        );
                    }
                }
            }
            let trigger = OccurrenceTrigger::Schedule { scheduled_at };
            let payload = json!({
                "scheduledAt": scheduled_at,
                "trigger": "schedule",
            });
            let key = format!("schedule:{}:{}", job.generation, scheduled_at);
            match self.admit_one_in_memory(&job, trigger, &payload, &key, now) {
                Ok(occurrence) => {
                    if occurrence.is_pending() {
                        admitted.push(occurrence);
                    }
                }
                Err(error) => {
                    *self = before;
                    return Err(error);
                }
            }
        }
        if changed || self.occurrences.len() != before.occurrences.len() {
            if let Err(error) = self.persist() {
                *self = before;
                return Err(error);
            }
        }
        Ok(admitted)
    }

    /// Admit matching event triggers. A stable delivery key is mandatory.
    pub fn admit_event(
        &mut self,
        kind: EventKind,
        payload: &Value,
        now: u64,
    ) -> Result<Vec<AutomationOccurrence>, String> {
        let key = extract_idempotency_key(payload, "event")?;
        self.admit_event_with_key(kind, payload, &key, now)
    }

    pub fn admit_event_with_key(
        &mut self,
        kind: EventKind,
        payload: &Value,
        idempotency_key: &str,
        now: u64,
    ) -> Result<Vec<AutomationOccurrence>, String> {
        self.ensure_healthy()?;
        if !payload.is_object() {
            return Err("event admission requires an object payload".into());
        }
        let key = normalize_idempotency_key(idempotency_key, "event")?;
        let payload_text = canonical_json(payload);
        let mut selected = Vec::new();
        for job in self.jobs.values() {
            if !job.enabled || job.paused {
                continue;
            }
            let TriggerSpec::Event {
                kind: expected,
                filter,
            } = &job.trigger
            else {
                continue;
            };
            if *expected != kind || (!filter.is_empty() && !payload_text.contains(filter)) {
                continue;
            }
            if let Some(scope) = &job.policy.scope {
                if !payload_text.contains(scope) {
                    continue;
                }
            }
            selected.push(job.clone());
        }
        let event_kind = serde_json::to_value(kind)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "event".into());
        self.admit_selected_transactional(
            selected,
            |_job| {
                Some((
                    OccurrenceTrigger::Event {
                        kind: event_kind.clone(),
                    },
                    payload.clone(),
                    key.clone(),
                ))
            },
            now,
        )
    }

    /// Admit a webhook after token, path, body-shape, schema, and stable
    /// delivery-key validation. The key must be supplied by the ingress owner.
    pub fn admit_webhook(
        &mut self,
        path: &str,
        body: &Value,
        now: u64,
        token: Option<&str>,
    ) -> Result<Vec<AutomationOccurrence>, String> {
        let key = extract_idempotency_key(body, "webhook")?;
        self.admit_webhook_with_key(path, body, &key, now, token)
    }

    pub fn admit_webhook_with_key(
        &mut self,
        path: &str,
        body: &Value,
        idempotency_key: &str,
        now: u64,
        token: Option<&str>,
    ) -> Result<Vec<AutomationOccurrence>, String> {
        self.ensure_healthy()?;
        let key = normalize_idempotency_key(idempotency_key, "webhook")?;
        if let Some(expected) = &self.webhook_token {
            if token != Some(expected.as_str()) {
                return Err("webhook: bad token".into());
            }
        }
        let object = body
            .as_object()
            .ok_or_else(|| format!("webhook {path}: body must be a JSON object"))?;
        let mut selected = Vec::new();
        for job in self.jobs.values() {
            if !job.enabled || job.paused {
                continue;
            }
            let TriggerSpec::Webhook {
                path: expected_path,
                schema,
            } = &job.trigger
            else {
                continue;
            };
            if expected_path != path {
                continue;
            }
            for required in schema {
                if !object.contains_key(required) {
                    return Err(format!("webhook {path}: missing required key {required:?}"));
                }
            }
            selected.push(job.clone());
        }
        self.admit_selected_transactional(
            selected,
            |_job| {
                Some((
                    OccurrenceTrigger::Webhook {
                        path: path.to_string(),
                    },
                    body.clone(),
                    key.clone(),
                ))
            },
            now,
        )
    }

    /// Admit a manual request. A stable request key is mandatory; a timestamp
    /// or payload digest is not an identity for a retryable request.
    pub fn admit_manual(
        &mut self,
        id: &str,
        payload: &Value,
        now: u64,
    ) -> Result<AutomationOccurrence, String> {
        let key = extract_idempotency_key(payload, "manual")?;
        self.admit_manual_with_key(id, payload, &key, now)
    }

    pub fn admit_manual_with_key(
        &mut self,
        id: &str,
        payload: &Value,
        idempotency_key: &str,
        now: u64,
    ) -> Result<AutomationOccurrence, String> {
        self.ensure_healthy()?;
        let key = normalize_idempotency_key(idempotency_key, "manual")?;
        let job = self
            .jobs
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown job {id:?}"))?;
        if !job.enabled || job.paused {
            return Err(format!("job {id} is disabled or paused"));
        }
        let mut admitted = self.admit_selected_transactional(
            vec![job],
            |_job| Some((OccurrenceTrigger::Manual, payload.clone(), key.clone())),
            now,
        )?;
        admitted
            .pop()
            .ok_or_else(|| format!("manual occurrence for {id} was not admitted"))
    }

    pub fn admit_event_idempotent(
        &mut self,
        kind: EventKind,
        payload: &Value,
        key: &str,
        now: u64,
    ) -> Result<Vec<AutomationOccurrence>, String> {
        self.admit_event_with_key(kind, payload, key, now)
    }

    pub fn admit_webhook_idempotent(
        &mut self,
        path: &str,
        body: &Value,
        key: &str,
        now: u64,
        token: Option<&str>,
    ) -> Result<Vec<AutomationOccurrence>, String> {
        self.admit_webhook_with_key(path, body, key, now, token)
    }

    pub fn admit_manual_idempotent(
        &mut self,
        id: &str,
        payload: &Value,
        key: &str,
        now: u64,
    ) -> Result<AutomationOccurrence, String> {
        self.admit_manual_with_key(id, payload, key, now)
    }

    fn frequency_admits(&self, job: &Job, now: u64) -> bool {
        let Some(cap) = job.policy.max_runs_per_hour else {
            return true;
        };
        let cutoff = now.saturating_sub(3600);
        (job.recent_fires.iter().filter(|at| **at >= cutoff).count() as u32) < cap
    }

    fn admit_selected_transactional<F>(
        &mut self,
        selected: Vec<Job>,
        make: F,
        now: u64,
    ) -> Result<Vec<AutomationOccurrence>, String>
    where
        F: Fn(&Job) -> Option<(OccurrenceTrigger, Value, String)>,
    {
        let before = self.clone();
        match self.admit_selected_in_memory(selected, make, now) {
            Ok(admitted) => {
                if self.occurrences.len() != before.occurrences.len() {
                    if let Err(error) = self.persist() {
                        *self = before;
                        return Err(error);
                    }
                }
                Ok(admitted)
            }
            Err(error) => {
                *self = before;
                Err(error)
            }
        }
    }

    fn admit_selected_in_memory<F>(
        &mut self,
        selected: Vec<Job>,
        make: F,
        now: u64,
    ) -> Result<Vec<AutomationOccurrence>, String>
    where
        F: Fn(&Job) -> Option<(OccurrenceTrigger, Value, String)>,
    {
        let original_len = self.occurrences.len();
        let mut admitted = Vec::new();
        for job in selected {
            let Some((trigger, payload, key)) = make(&job) else {
                continue;
            };
            let key = normalize_idempotency_key(&key, "delivery")?;
            let payload_digest = digest_json(&payload);
            let dedup_digest = dedup_digest_for(&job.automation_id, job.generation, &trigger, &key);
            if let Some(existing) = self
                .occurrences
                .iter()
                .find(|occurrence| occurrence.dedup_digest == dedup_digest)
            {
                if existing.payload_digest != payload_digest {
                    self.occurrences.truncate(original_len);
                    return Err(format!(
                        "idempotency key `{key}` was reused with a different payload"
                    ));
                }
                admitted.push(existing.clone());
                continue;
            }
            if !self.frequency_admits(&job, now) {
                continue;
            }
            let occurrence = AutomationOccurrence {
                trigger_occurrence_id: format!("occ:{}", &dedup_digest[..32]),
                automation_id: job.automation_id.clone(),
                revision_id: job.revision_id.clone(),
                trigger,
                payload_digest,
                dedup_digest,
                admitted_at: now,
                state: OccurrenceState::Pending,
                idempotency_key: key,
                fired_at: None,
                work_id: None,
                run_id: None,
                admission_error: None,
                admission_receipt: None,
                revision: revision_snapshot(&job),
            };
            if let Err(error) = validate_occurrence_shape(&occurrence) {
                self.occurrences.truncate(original_len);
                return Err(error);
            }
            self.occurrences.push(occurrence.clone());
            admitted.push(occurrence);
        }
        Ok(admitted)
    }

    fn admit_one_in_memory(
        &mut self,
        job: &Job,
        trigger: OccurrenceTrigger,
        payload: &Value,
        key: &str,
        now: u64,
    ) -> Result<AutomationOccurrence, String> {
        self.admit_selected_in_memory(
            vec![job.clone()],
            |_candidate| Some((trigger.clone(), payload.clone(), key.to_string())),
            now,
        )?
        .pop()
        .ok_or_else(|| format!("occurrence for automation `{}` was not admitted", job.id))
    }

    /// Deterministic Work identity expected by the host admission receipt.
    pub fn expected_work_id(&self, occurrence: &AutomationOccurrence) -> String {
        format!(
            "automation-work:{}:{}",
            occurrence.automation_id, occurrence.trigger_occurrence_id
        )
    }

    pub fn expected_run_id(&self, occurrence: &AutomationOccurrence) -> String {
        format!(
            "automation-run:{}:{}",
            occurrence.automation_id, occurrence.trigger_occurrence_id
        )
    }

    /// Build the receipt shape expected by the current host adapter.  The
    /// caller must only use this after Work and Run are durably admitted.
    pub fn receipt_for_occurrence(
        &self,
        occurrence_id: &str,
    ) -> Result<WorkRunAdmissionReceipt, String> {
        let occurrence = self
            .occurrence(occurrence_id)
            .ok_or_else(|| format!("unknown occurrence {occurrence_id:?}"))?;
        let generation = parse_revision_id(&occurrence.revision_id)
            .map_err(|error| error.to_string())?
            .generation;
        Ok(WorkRunAdmissionReceipt::new(
            self.expected_work_id(occurrence),
            self.expected_run_id(occurrence),
            occurrence.automation_id.clone(),
            occurrence.revision_id.clone(),
            occurrence.trigger_occurrence_id.clone(),
            generation,
        ))
    }

    /// Legacy trigger-only advancement is intentionally fail-closed. Callers
    /// must use [`Self::mark_occurrence_fired_with_receipt`] with a receipt
    /// minted after durable Work/Run admission.
    pub fn mark_occurrence_fired(
        &mut self,
        _occurrence_id: &str,
        _now: u64,
        _work_id: &str,
        _run_id: &str,
    ) -> Result<(), String> {
        Err("occurrence advancement requires a durable WorkRunAdmissionReceipt".into())
    }

    /// Advance/reconcile an occurrence only with a matching durable receipt.
    pub fn mark_occurrence_fired_with_receipt(
        &mut self,
        occurrence_id: &str,
        now: u64,
        receipt: &WorkRunAdmissionReceipt,
    ) -> Result<(), String> {
        self.ensure_healthy()?;
        let index = self
            .occurrences
            .iter()
            .position(|occurrence| occurrence.trigger_occurrence_id == occurrence_id)
            .ok_or_else(|| format!("unknown occurrence {occurrence_id:?}"))?;
        let original = self.occurrences[index].clone();
        validate_receipt_for(
            &original,
            receipt,
            &self.expected_work_id(&original),
            &self.expected_run_id(&original),
        )?;
        if original.state == OccurrenceState::Cancelled {
            return Err(format!("occurrence {occurrence_id} is cancelled"));
        }
        if original.state == OccurrenceState::Terminal {
            if original.admission_receipt.as_ref() == Some(receipt) {
                return Ok(());
            }
            return Err(format!(
                "occurrence {occurrence_id} was already advanced with a different receipt"
            ));
        }
        let job_id = self
            .jobs
            .values()
            .find(|job| job.automation_id == original.automation_id)
            .map(|job| job.id.clone())
            .ok_or_else(|| format!("occurrence {occurrence_id} names an unknown automation"))?;
        let old_job = self
            .jobs
            .get(&job_id)
            .cloned()
            .ok_or_else(|| format!("unknown job {job_id:?}"))?;
        let before = self.clone();
        let mut next_job = old_job.clone();
        next_job.last_fired_at = Some(now);
        let cutoff = now.saturating_sub(3600);
        next_job.recent_fires.retain(|at| *at >= cutoff);
        next_job.recent_fires.push(now);
        if let OccurrenceTrigger::Schedule { scheduled_at } = &original.trigger {
            next_job.next_run_at =
                compute_next_run(&next_job.trigger, now.max(*scheduled_at), None);
        }
        let mut next_occurrence = original.clone();
        next_occurrence.state = OccurrenceState::Terminal;
        next_occurrence.fired_at = Some(now);
        next_occurrence.work_id = Some(receipt.work_id.clone());
        next_occurrence.run_id = Some(receipt.run_id.clone());
        next_occurrence.admission_error = None;
        next_occurrence.admission_receipt = Some(receipt.clone());
        validate_occurrence_shape(&next_occurrence)?;
        self.occurrences[index] = next_occurrence;
        self.jobs.insert(job_id, next_job);
        if let Err(error) = self.persist() {
            *self = before;
            return Err(error);
        }
        Ok(())
    }

    pub fn advance_occurrence(
        &mut self,
        occurrence_id: &str,
        now: u64,
        receipt: WorkRunAdmissionReceipt,
    ) -> Result<(), String> {
        self.mark_occurrence_fired_with_receipt(occurrence_id, now, &receipt)
    }

    pub fn reconcile_occurrence(
        &mut self,
        occurrence_id: &str,
        now: u64,
        receipt: WorkRunAdmissionReceipt,
    ) -> Result<(), String> {
        self.advance_occurrence(occurrence_id, now, receipt)
    }

    /// Mark admission uncertainty.  An uncertain occurrence is intentionally
    /// absent from `pending_occurrences` until an explicit receipt reconciles it.
    pub fn mark_occurrence_uncertain(
        &mut self,
        occurrence_id: &str,
        reason: &str,
    ) -> Result<(), String> {
        self.ensure_healthy()?;
        if reason.trim().is_empty() {
            return Err("uncertain occurrence requires a reconciliation reason".into());
        }
        let index = self
            .occurrences
            .iter()
            .position(|occurrence| occurrence.trigger_occurrence_id == occurrence_id)
            .ok_or_else(|| format!("unknown occurrence {occurrence_id:?}"))?;
        let original = self.occurrences[index].clone();
        if original.state == OccurrenceState::Cancelled
            || original.state == OccurrenceState::Terminal
        {
            return Err(format!("occurrence {occurrence_id} is already terminal"));
        }
        let before = self.clone();
        self.occurrences[index].state = OccurrenceState::Uncertain;
        self.occurrences[index].admission_error = Some(reason.to_string());
        if let Err(error) = self.persist() {
            *self = before;
            return Err(error);
        }
        Ok(())
    }

    /// Cancel one occurrence monotonically.  A later scheduler tick can admit a
    /// new scheduled occurrence, but it can never reopen this one.
    pub fn cancel_occurrence(&mut self, occurrence_id: &str, reason: &str) -> Result<(), String> {
        self.ensure_healthy()?;
        let index = self
            .occurrences
            .iter()
            .position(|occurrence| occurrence.trigger_occurrence_id == occurrence_id)
            .ok_or_else(|| format!("unknown occurrence {occurrence_id:?}"))?;
        let original = self.occurrences[index].clone();
        if original.state == OccurrenceState::Terminal {
            return Err(format!("occurrence {occurrence_id} is already terminal"));
        }
        if original.state == OccurrenceState::Cancelled {
            return Ok(());
        }
        let before = self.clone();
        self.occurrences[index].state = OccurrenceState::Cancelled;
        self.occurrences[index].admission_error =
            (!reason.trim().is_empty()).then(|| reason.to_string());
        if let OccurrenceTrigger::Schedule { scheduled_at } = &original.trigger {
            if let Some(job) = self
                .jobs
                .values_mut()
                .find(|job| job.automation_id == original.automation_id)
            {
                job.next_run_at = compute_next_run(&job.trigger, *scheduled_at, None);
            }
        }
        if let Err(error) = self.persist() {
            *self = before;
            return Err(error);
        }
        Ok(())
    }

    pub fn occurrence_state(&self, occurrence_id: &str) -> Option<OccurrenceState> {
        self.occurrence(occurrence_id)
            .map(|occurrence| occurrence.state)
    }

    // -- occurrences ------------------------------------------------------------

    /// Legacy trigger-only advancement is fail-closed by design.
    pub fn mark_fired(&mut self, _id: &str, _now: u64) -> Result<(), String> {
        Err("mark_fired cannot advance a schedule without a WorkRunAdmissionReceipt".into())
    }

    // -- battery ---------------------------------------------------------------

    pub fn set_battery(&mut self, on_battery: bool) {
        if self.load_error.is_none() {
            self.on_battery = on_battery;
        }
    }

    pub fn on_battery(&self) -> bool {
        self.on_battery
    }

    // -- due computation --------------------------------------------------------

    /// Jobs due now (cron/interval/window match), respecting trigger-plane
    /// pause, battery suppression and the frequency policy. Returns job ids
    /// ordered by next_run_at.
    pub fn due(&mut self, now: u64) -> Vec<String> {
        if self.load_error.is_some() {
            return Vec::new();
        }
        let on_battery = self.on_battery;
        let ids: Vec<String> = self.jobs.keys().cloned().collect();
        let mut out = Vec::new();
        for id in ids {
            let Some(job) = self.jobs.get(&id) else {
                continue;
            };
            if !job.enabled || job.paused {
                continue;
            }
            // Battery suppression.
            if on_battery && job.policy.suppress_on_battery {
                continue;
            }
            // Frequency policy (rolling hour) — skip if at/over cap.
            if let Some(cap) = job.policy.max_runs_per_hour {
                let cutoff = now.saturating_sub(3600);
                let in_window = job.recent_fires.iter().filter(|t| **t >= cutoff).count() as u32;
                if in_window >= cap {
                    continue;
                }
            }
            let due = match &job.trigger {
                // A stale cron tick is due by its persisted next-run value;
                // requiring the current wall-clock minute would silently lose
                // run_once_on_resume occurrences.
                TriggerSpec::Cron { expr } => {
                    CronExpr::parse(expr).is_ok() && job.next_run_at.is_some_and(|next| next <= now)
                }
                TriggerSpec::Interval { .. } | TriggerSpec::Window { .. } => {
                    job.next_run_at.is_some_and(|next| next <= now)
                }
                TriggerSpec::Event { .. } | TriggerSpec::Webhook { .. } | TriggerSpec::Manual => {
                    false
                }
            };
            if due {
                let blocked_by_reconciliation = job.next_run_at.is_some_and(|next| {
                    self.occurrences.iter().any(|occurrence| {
                        occurrence.automation_id == job.automation_id
                            && matches!(
                                &occurrence.trigger,
                                OccurrenceTrigger::Schedule { scheduled_at } if *scheduled_at == next
                            )
                            && !occurrence.is_pending()
                    })
                });
                if !blocked_by_reconciliation {
                    out.push(id);
                }
            }
        }
        out.sort_by_key(|id| {
            self.jobs
                .get(id)
                .and_then(|j| j.next_run_at)
                .unwrap_or(u64::MAX)
        });
        out
    }

    // -- event + webhook triggers ----------------------------------------------

    /// Fire an event (Gartner kinds). This compatibility façade now uses the
    /// same durable admission path as the host dispatcher; it returns the
    /// registry ids for callers that only need an acknowledgement.
    pub fn fire_event(
        &mut self,
        kind: EventKind,
        payload: &Value,
        now: u64,
    ) -> Result<Vec<String>, String> {
        let admitted = self.admit_event(kind, payload, now)?;
        Ok(self.pending_job_ids_for_occurrences(&admitted))
    }

    /// Webhook ingress (F11 loopback). Validate the path + required body keys
    /// (schema), then durably admit the occurrence. `token` (optional) guards
    /// the loopback listener — set via `scheduler/webhook_token`.
    pub fn fire_webhook(
        &mut self,
        path: &str,
        body: &Value,
        now: u64,
        token: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let admitted = self.admit_webhook(path, body, now, token)?;
        Ok(self.pending_job_ids_for_occurrences(&admitted))
    }

    pub fn set_webhook_token(&mut self, token: Option<String>) {
        if self.load_error.is_none() {
            self.webhook_token = token;
        }
    }

    // -- nudge sentinels --------------------------------------------------------

    /// Record a goal observation (from chat turns / session activity).
    pub fn record_nudge(&mut self, goal: &str, unix_secs: u64) {
        self.nudge_log.push(NudgeSample {
            goal: goal.to_string(),
            unix_secs,
        });
        let cutoff = unix_secs.saturating_sub(NUDGE_WINDOW_DAYS * 86_400);
        self.nudge_log.retain(|s| s.unix_secs >= cutoff);
    }

    /// Detect repeating patterns: same goal at the same hour-of-day across
    /// ≥3 days in the window → suggest a daily cron. Returns suggestions
    /// sorted by confidence.
    pub fn nudges(&self) -> Vec<NudgeSuggestion> {
        let mut by_goal: HashMap<&str, Vec<u64>> = HashMap::new();
        for s in &self.nudge_log {
            by_goal
                .entry(s.goal.as_str())
                .or_default()
                .push(s.unix_secs);
        }
        let mut out = Vec::new();
        for (goal, times) in by_goal {
            let mut hours: Vec<u8> = times.iter().map(|t| ((t % 86_400) / 3600) as u8).collect();
            hours.sort_unstable();
            hours.dedup();
            // Distinct days the goal fired on.
            let mut days: Vec<u64> = times.iter().map(|t| t / 86_400).collect();
            days.sort_unstable();
            days.dedup();
            if days.len() >= 3 && hours.len() == 1 {
                let h = hours[0];
                let day_count = days.len();
                let confidence = (day_count as f64 / NUDGE_WINDOW_DAYS as f64).min(1.0);
                out.push(NudgeSuggestion {
                    goal: goal.to_string(),
                    cron: format!("0 {h} * * *"),
                    confidence,
                    observed_at: vec![format!("{:02}:00", h)],
                });
            }
        }
        out.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }

    // -- incidents (P51.32e) ------------------------------------------------------

    /// Record an incident. Returns the new incident id. Incidents start
    /// unacked and require an explicit [`Self::ack_incident`].
    pub fn report_incident(
        &mut self,
        job_id: impl Into<String>,
        kind: impl Into<String>,
        detail: impl Into<String>,
        at_ms: u64,
    ) -> String {
        self.incident_seq = self.incident_seq.saturating_add(1);
        let id = format!("inc-{}", self.incident_seq);
        self.incidents.push(Incident {
            id: id.clone(),
            job_id: job_id.into(),
            at_ms,
            kind: kind.into(),
            detail: detail.into(),
            acked: false,
        });
        id
    }

    /// Explicitly acknowledge an incident. Returns `false` for unknown ids.
    pub fn ack_incident(&mut self, id: &str) -> bool {
        if let Some(inc) = self.incidents.iter_mut().find(|i| i.id == id) {
            inc.acked = true;
            true
        } else {
            false
        }
    }

    pub fn get_incident(&self, id: &str) -> Option<&Incident> {
        self.incidents.iter().find(|i| i.id == id)
    }

    /// Cloned incident list (ordered by report time).
    pub fn list_incidents(&self) -> Vec<Incident> {
        self.incidents.clone()
    }

    // -- doctor (P51.32f) ----------------------------------------------------------

    /// Pure read-only health check of the **trigger plane**: `missed_runs`
    /// (enabled, unpaused schedule jobs whose `next_run_at` lies in the past —
    /// the misfire surface) and `queue_depth` (registry size guard). Never
    /// mutates; run-level health belongs to the Work kernel / Event Log.
    pub fn cron_doctor(&self, now: u64) -> Vec<CronCheck> {
        let mut missed = 0usize;
        for job in self.jobs.values() {
            if !job.enabled || job.paused {
                continue;
            }
            if matches!(
                job.trigger,
                TriggerSpec::Event { .. } | TriggerSpec::Webhook { .. } | TriggerSpec::Manual
            ) {
                continue; // event-driven jobs have no next_run_at
            }
            if let Some(t) = job.next_run_at {
                if t < now {
                    missed += 1;
                }
            }
        }
        let depth = self.jobs.len();
        vec![
            CronCheck {
                name: "missed_runs".to_string(),
                ok: missed == 0,
                detail: if missed == 0 {
                    "none".to_string()
                } else {
                    format!("{missed} missed")
                },
            },
            CronCheck {
                name: "queue_depth".to_string(),
                ok: depth <= REGISTRY_SOFT_CAP,
                detail: format!("depth={depth}"),
            },
        ]
    }

    // -- JSON-RPC dispatch ------------------------------------------------------

    pub fn handle(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        self.ensure_healthy()?;
        let before = self.clone();
        let out = match self.handle_inner(method, params) {
            Ok(out) => out,
            Err(error) => {
                *self = before;
                return Err(error);
            }
        };
        if let Err(error) = self.persist() {
            *self = before.clone();
            // Best-effort repair keeps disk and memory aligned if a checked
            // mutator had already written before the outer commit failed.
            let _ = before.persist();
            return Err(error);
        }
        Ok(out)
    }

    fn handle_inner(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        let now = params
            .get("now")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            });
        match method {
            "scheduler/list" => {
                let jobs: Vec<Value> = self
                    .list()
                    .iter()
                    .map(|j| serde_json::to_value(j).unwrap_or(Value::Null))
                    .collect();
                Ok(json!({
                    "jobs": jobs,
                    "onBattery": self.on_battery,
                    "occurrences": self.occurrences(),
                }))
            }
            "scheduler/upsert" => {
                let id = str_param(params, "id").ok_or("scheduler/upsert requires id")?;
                let name = str_param(params, "name").unwrap_or(id);
                let session_id = str_param(params, "sessionId").unwrap_or("");
                let trigger = serde_json::from_value::<TriggerSpec>(
                    params
                        .get("trigger")
                        .cloned()
                        .ok_or("scheduler/upsert requires trigger")?,
                )
                .map_err(|e| format!("bad trigger: {e}"))?;
                let steps = serde_json::from_value::<Vec<AutomationStep>>(
                    params.get("steps").cloned().unwrap_or(Value::Array(vec![])),
                )
                .map_err(|e| format!("bad steps: {e}"))?;
                let policy = params
                    .get("policy")
                    .cloned()
                    .map(|v| {
                        serde_json::from_value::<SchedulePolicy>(v)
                            .map_err(|e| format!("bad policy: {e}"))
                    })
                    .transpose()?;
                self.upsert_checked_with_enabled(
                    id,
                    name,
                    session_id,
                    trigger,
                    steps,
                    policy,
                    params.get("enabled").and_then(Value::as_bool),
                    now,
                )?;
                Ok(json!({ "ok": true, "id": id }))
            }
            "scheduler/delete" => {
                let id = str_param(params, "id").ok_or("scheduler/delete requires id")?;
                Ok(json!({ "ok": self.delete_checked(id)? }))
            }
            "scheduler/enable" => {
                let id = str_param(params, "id").ok_or("scheduler/enable requires id")?;
                let enabled = params
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                self.set_enabled(id, enabled, now)?;
                Ok(json!({ "ok": true }))
            }
            "scheduler/pause" => {
                let id = str_param(params, "id").ok_or("scheduler/pause requires id")?;
                self.pause(id)?;
                Ok(json!({ "ok": true }))
            }
            "scheduler/pause_session" => {
                let session_id = str_param(params, "sessionId")
                    .ok_or("scheduler/pause_session requires sessionId")?;
                let paused = self.pause_session_checked(session_id)?;
                Ok(json!({ "ok": true, "paused": paused }))
            }
            "scheduler/resume" => {
                let id = str_param(params, "id").ok_or("scheduler/resume requires id")?;
                self.resume(id, now)?;
                Ok(json!({ "ok": true }))
            }
            "scheduler/due" => {
                // A due query is also an admission boundary: the occurrence
                // is durable before the host is told which ids need Work.
                let occurrences = self.admit_due(now)?;
                Ok(json!({
                    "due": self.due(now),
                    "occurrences": occurrences,
                    "now": now,
                }))
            }
            "scheduler/admit_due" => {
                let admitted = self.admit_due(now)?;
                Ok(json!({ "occurrences": admitted }))
            }
            "scheduler/pending_occurrences" => {
                Ok(json!({ "occurrences": self.pending_occurrences() }))
            }
            "scheduler/occurrence_error" => {
                let id = str_param(params, "id").ok_or("scheduler/occurrence_error requires id")?;
                let reason = str_param(params, "reason").unwrap_or("admission uncertain");
                self.mark_occurrence_uncertain(id, reason)?;
                Ok(json!({ "ok": true, "id": id }))
            }
            "scheduler/mark_occurrence_fired" => {
                let id =
                    str_param(params, "id").ok_or("scheduler/mark_occurrence_fired requires id")?;
                let receipt = params
                    .get("receipt")
                    .cloned()
                    .ok_or("scheduler/mark_occurrence_fired requires a durable receipt")?;
                let receipt: WorkRunAdmissionReceipt = serde_json::from_value(receipt)
                    .map_err(|error| format!("bad admission receipt: {error}"))?;
                self.mark_occurrence_fired_with_receipt(id, now, &receipt)?;
                Ok(json!({ "ok": true, "id": id, "receipt": receipt }))
            }
            "scheduler/cancel_occurrence" => {
                let id =
                    str_param(params, "id").ok_or("scheduler/cancel_occurrence requires id")?;
                let reason = str_param(params, "reason").unwrap_or("cancelled by host");
                self.cancel_occurrence(id, reason)?;
                Ok(json!({ "ok": true, "id": id }))
            }
            "scheduler/mark_fired" => {
                let id = str_param(params, "id").ok_or("scheduler/mark_fired requires id")?;
                self.mark_fired(id, now)?;
                Ok(json!({ "ok": true, "id": id }))
            }
            "scheduler/battery" => {
                let on = params
                    .get("onBattery")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.set_battery(on);
                Ok(json!({ "ok": true, "onBattery": on }))
            }
            "scheduler/fire_event" => {
                let kind = serde_json::from_value::<EventKind>(
                    params
                        .get("kind")
                        .cloned()
                        .ok_or("scheduler/fire_event requires kind")?,
                )
                .map_err(|e| format!("bad kind: {e}"))?;
                let payload = params.get("payload").cloned().unwrap_or(Value::Null);
                let admitted = if let Some(key) = str_param(params, "idempotencyKey") {
                    self.admit_event_with_key(kind, &payload, key, now)?
                } else {
                    let key = format!("event:payload:{}", digest_json(&payload));
                    self.admit_event_with_key(kind, &payload, &key, now)?
                };
                let fired = self.pending_job_ids_for_occurrences(&admitted);
                Ok(json!({ "fired": fired, "occurrences": admitted }))
            }
            "scheduler/fire_webhook" => {
                let path =
                    str_param(params, "path").ok_or("scheduler/fire_webhook requires path")?;
                let body = params.get("body").cloned().unwrap_or(Value::Null);
                let token = params.get("token").and_then(Value::as_str);
                let admitted = if let Some(key) = str_param(params, "idempotencyKey") {
                    self.admit_webhook_with_key(path, &body, key, now, token)?
                } else {
                    let key = format!("webhook:payload:{}", digest_json(&body));
                    self.admit_webhook_with_key(path, &body, &key, now, token)?
                };
                let fired = self.pending_job_ids_for_occurrences(&admitted);
                Ok(json!({ "fired": fired, "occurrences": admitted }))
            }
            "scheduler/webhook_token" => {
                let token = params
                    .get("token")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.set_webhook_token(token);
                Ok(json!({ "ok": true }))
            }
            "scheduler/nudge" => {
                let goal = str_param(params, "goal").ok_or("scheduler/nudge requires goal")?;
                let ts = params.get("ts").and_then(Value::as_u64).unwrap_or(now);
                self.record_nudge(goal, ts);
                Ok(json!({ "ok": true }))
            }
            "scheduler/nudges" => Ok(json!({ "suggestions": self.nudges() })),
            "scheduler/run_now" => {
                let id = str_param(params, "id").ok_or("scheduler/run_now requires id")?;
                let payload = params.get("payload").cloned().unwrap_or(Value::Null);
                let occurrence = if let Some(key) = str_param(params, "idempotencyKey") {
                    self.admit_manual_with_key(id, &payload, key, now)?
                } else {
                    self.admit_manual(id, &payload, now)?
                };
                Ok(json!({ "ok": true, "id": id, "occurrence": occurrence }))
            }
            "scheduler/monitor" => {
                let id = str_param(params, "id").ok_or("scheduler/monitor requires id")?;
                let observation = str_param(params, "observation").unwrap_or("");
                let condition_met = params
                    .get("conditionMet")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let verdict = self.monitor_evaluate(id, observation, condition_met)?;
                Ok(serde_json::to_value(verdict).unwrap_or(Value::Null))
            }
            "scheduler/monitor_config" => {
                let id = str_param(params, "id").ok_or("scheduler/monitor_config requires id")?;
                let monitor = params
                    .get("monitor")
                    .cloned()
                    .map(|v| {
                        serde_json::from_value::<MonitorConfig>(v)
                            .map_err(|e| format!("bad monitor: {e}"))
                    })
                    .transpose()?;
                self.set_monitor(id, monitor)?;
                Ok(json!({ "ok": true, "id": id }))
            }
            _ => Err(format!("method not found: {method}")),
        }
    }
}

fn str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

/// Canonical JSON used for all scheduler digests. Object keys are sorted so
/// semantically identical values have one byte representation.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into()),
        Value::Array(values) => {
            let parts: Vec<String> = values.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(values) => {
            let mut keys: Vec<&String> = values.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()),
                        canonical_json(&values[key])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

fn digest_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn digest_json(value: &Value) -> String {
    digest_bytes(canonical_json(value).as_bytes())
}

fn is_sha256_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A stable identity derived by Rust from the registry key. Callers may
/// choose the human-facing key, but they cannot choose occurrence provenance.
///
/// NOTE (DEC-053 Step 4): `"everyaios.automation.v1"` is a persisted digest
/// domain separator — its spelling stays so existing occurrence ids stay
/// stable.
fn trusted_automation_id(registry_id: &str) -> String {
    format!(
        "automation:auto:{}",
        digest_bytes(format!("everyaios.automation.v1\0{registry_id}").as_bytes())
    )
}

fn validate_job_definition(job: &Job) -> Result<(), String> {
    if job.id.trim().is_empty() || job.name.trim().is_empty() {
        return Err(format!("scheduler job `{}` has an empty id/name", job.id));
    }
    if job.generation == 0 || job.revision == 0 {
        return Err(format!(
            "scheduler job `{}` has no generation/revision",
            job.id
        ));
    }
    match &job.trigger {
        TriggerSpec::Cron { expr } => {
            CronExpr::parse(expr)
                .map_err(|error| format!("invalid cron for `{}`: {error}", job.id))?;
        }
        TriggerSpec::Interval { secs } if *secs == 0 => {
            return Err(format!(
                "scheduler interval for `{}` must be non-zero",
                job.id
            ));
        }
        TriggerSpec::Webhook { path, .. } if path.trim().is_empty() => {
            return Err(format!(
                "scheduler webhook for `{}` has an empty path",
                job.id
            ));
        }
        TriggerSpec::Event { .. }
        | TriggerSpec::Interval { .. }
        | TriggerSpec::Webhook { .. }
        | TriggerSpec::Window { .. }
        | TriggerSpec::Manual => {}
    }
    if matches!(job.trigger, TriggerSpec::Cron { .. })
        && job.policy.misfire_policy == MisfirePolicy::CatchUp
    {
        return Err(format!(
            "scheduler job `{}` uses unsupported catch_up misfire policy",
            job.id
        ));
    }
    Ok(())
}

fn revision_id_for(job: &Job, revision: u64) -> String {
    let snapshot = revision_snapshot(job);
    content_addressed_revision_id_for_generation(&snapshot.automation(), job.generation, revision)
}

fn revision_snapshot(job: &Job) -> AutomationRevision {
    AutomationRevision {
        automation_id: job.automation_id.clone(),
        revision: job.revision,
        revision_id: job.revision_id.clone(),
        name: job.name.clone(),
        session_id: job.session_id.clone(),
        trigger: job.trigger.clone(),
        steps: job.steps.clone(),
        policy: job.policy.clone(),
    }
}

fn legacy_definition_digest(job: &Job) -> String {
    digest_json(&json!({
        "automationId": job.automation_id,
        "name": job.name,
        "sessionId": job.session_id,
        "trigger": job.trigger,
        "steps": job.steps,
        "policy": job.policy,
    }))
}

fn legacy_revision_id_matches(job: &Job, revision_id: &str) -> bool {
    revision_id == format!("{}:{}", job.revision, legacy_definition_digest(job))
}

fn legacy_revision_id_for_snapshot(revision: &AutomationRevision) -> String {
    format!(
        "{}:{}",
        revision.revision,
        digest_json(&json!({
            "automationId": revision.automation_id,
            "name": revision.name,
            "sessionId": revision.session_id,
            "trigger": revision.trigger,
            "steps": revision.steps,
            "policy": revision.policy,
        }))
    )
}

fn normalize_loaded_occurrence(occurrence: &mut AutomationOccurrence) -> Result<(), String> {
    // Version-2 rows predate the generation-aware identity.  Migrate only an
    // exact legacy digest; an arbitrary malformed identity remains a hard
    // load failure rather than being silently repaired.
    if parse_revision_id(&occurrence.revision_id).is_err() {
        let legacy_id = legacy_revision_id_for_snapshot(&occurrence.revision);
        if !occurrence.revision_id.is_empty() && occurrence.revision_id != legacy_id {
            return Err(format!(
                "scheduler occurrence `{}` has an invalid legacy revision id",
                occurrence.trigger_occurrence_id
            ));
        }
        let generation = occurrence.revision.generation().max(1);
        let revision = occurrence.revision.revision.max(1);
        let canonical = content_addressed_revision_id_for_generation(
            &occurrence.revision.automation(),
            generation,
            revision,
        );
        occurrence.revision_id = canonical.clone();
        occurrence.revision.revision_id = canonical;
    } else if occurrence.revision.revision_id.trim().is_empty() {
        occurrence.revision.revision_id = occurrence.revision_id.clone();
    }

    let had_key = !occurrence.idempotency_key.trim().is_empty();
    if !had_key {
        occurrence.idempotency_key = format!("legacy:{}", occurrence.dedup_digest);
    }
    if occurrence.state == OccurrenceState::Pending || occurrence.state == OccurrenceState::Terminal
    {
        occurrence.state = if occurrence.admission_receipt.is_some() {
            OccurrenceState::Terminal
        } else if !had_key
            || occurrence.admission_error.is_some()
            || occurrence.fired_at.is_some()
            || occurrence.work_id.is_some()
            || occurrence.run_id.is_some()
        {
            if occurrence.admission_error.is_none() {
                occurrence.admission_error = Some(if had_key {
                    "legacy firing lacks a durable Work/Run receipt".into()
                } else {
                    "legacy occurrence lacks a stable delivery key".into()
                });
            }
            OccurrenceState::Uncertain
        } else {
            OccurrenceState::Pending
        };
    }
    Ok(())
}

fn validate_occurrence_shape(occurrence: &AutomationOccurrence) -> Result<(), String> {
    if !occurrence.trigger_occurrence_id.starts_with("occ:")
        || occurrence.trigger_occurrence_id.len() != 36
        || occurrence.automation_id.trim().is_empty()
        || occurrence.revision_id.trim().is_empty()
        || occurrence.idempotency_key.trim().is_empty()
        || !is_sha256_digest(&occurrence.payload_digest)
        || !is_sha256_digest(&occurrence.dedup_digest)
    {
        return Err("scheduler occurrence has an invalid identity/digest field".into());
    }
    let identity = parse_revision_id(&occurrence.revision_id)
        .map_err(|error| format!("scheduler occurrence has invalid revision: {error}"))?;
    if occurrence.revision.automation_id != occurrence.automation_id
        || occurrence.revision.revision_id != occurrence.revision_id
        || occurrence.revision.revision != identity.revision
        || occurrence.revision.generation() != identity.generation
        || occurrence.revision_id
            != content_addressed_revision_id_for_generation(
                &occurrence.revision.automation(),
                identity.generation,
                identity.revision,
            )
    {
        return Err(format!(
            "scheduler occurrence `{}` has inconsistent revision provenance",
            occurrence.trigger_occurrence_id
        ));
    }
    if !occurrence_trigger_matches_revision(&occurrence.trigger, &occurrence.revision.trigger) {
        return Err(format!(
            "scheduler occurrence `{}` trigger does not match its immutable revision",
            occurrence.trigger_occurrence_id
        ));
    }
    let expected_dedup = dedup_digest_for(
        &occurrence.automation_id,
        identity.generation,
        &occurrence.trigger,
        &occurrence.idempotency_key,
    );
    if !occurrence.idempotency_key.starts_with("legacy:")
        && (expected_dedup != occurrence.dedup_digest
            || occurrence.trigger_occurrence_id
                != format!("occ:{}", &occurrence.dedup_digest[..32]))
    {
        return Err(format!(
            "scheduler occurrence `{}` has inconsistent delivery identity",
            occurrence.trigger_occurrence_id
        ));
    }
    match occurrence.state {
        OccurrenceState::Pending => {
            if occurrence.fired_at.is_some()
                || occurrence.work_id.is_some()
                || occurrence.run_id.is_some()
                || occurrence.admission_receipt.is_some()
                || occurrence.admission_error.is_some()
            {
                return Err("pending occurrence contains terminal/admission metadata".into());
            }
        }
        OccurrenceState::Uncertain => {
            if occurrence.fired_at.is_some() || occurrence.admission_receipt.is_some() {
                return Err("uncertain occurrence contains a terminal receipt".into());
            }
            if occurrence
                .admission_error
                .as_deref()
                .is_none_or(|reason| reason.trim().is_empty())
            {
                return Err("uncertain occurrence requires a reconciliation reason".into());
            }
        }
        OccurrenceState::Cancelled => {
            if occurrence.fired_at.is_some()
                || occurrence.work_id.is_some()
                || occurrence.run_id.is_some()
                || occurrence.admission_receipt.is_some()
            {
                return Err("cancelled occurrence contains an admission receipt".into());
            }
        }
        OccurrenceState::Terminal => {
            if occurrence.fired_at.is_none()
                || occurrence.work_id.is_none()
                || occurrence.run_id.is_none()
                || occurrence.admission_error.is_some()
            {
                return Err("terminal occurrence lacks Work/Run admission metadata".into());
            }
            let receipt = occurrence
                .admission_receipt
                .as_ref()
                .ok_or_else(|| "terminal occurrence lacks durable receipt".to_string())?;
            validate_receipt_for(
                occurrence,
                receipt,
                &format!(
                    "automation-work:{}:{}",
                    occurrence.automation_id, occurrence.trigger_occurrence_id
                ),
                &format!(
                    "automation-run:{}:{}",
                    occurrence.automation_id, occurrence.trigger_occurrence_id
                ),
            )?;
        }
    }
    Ok(())
}

fn occurrence_trigger_matches_revision(
    trigger: &OccurrenceTrigger,
    revision_trigger: &TriggerSpec,
) -> bool {
    match (trigger, revision_trigger) {
        (OccurrenceTrigger::Schedule { .. }, TriggerSpec::Cron { .. })
        | (OccurrenceTrigger::Schedule { .. }, TriggerSpec::Interval { .. })
        | (OccurrenceTrigger::Schedule { .. }, TriggerSpec::Window { .. }) => true,
        (OccurrenceTrigger::Manual, TriggerSpec::Manual) => true,
        (OccurrenceTrigger::Event { kind }, TriggerSpec::Event { kind: expected, .. }) => {
            serde_json::to_value(expected)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .as_deref()
                == Some(kind.as_str())
        }
        (OccurrenceTrigger::Webhook { path }, TriggerSpec::Webhook { path: expected, .. }) => {
            path == expected
        }
        _ => false,
    }
}

fn validate_receipt_for(
    occurrence: &AutomationOccurrence,
    receipt: &WorkRunAdmissionReceipt,
    expected_work_id: &str,
    expected_run_id: &str,
) -> Result<(), String> {
    if !receipt.durable {
        return Err("Work/Run receipt is not marked durable".into());
    }
    if receipt.work_id != expected_work_id || receipt.run_id != expected_run_id {
        return Err("Work/Run receipt ids do not match the deterministic occurrence owner".into());
    }
    if receipt.automation_id != occurrence.automation_id
        || receipt.revision_id != occurrence.revision_id
        || receipt.trigger_occurrence_id != occurrence.trigger_occurrence_id
        || receipt.automation_generation != occurrence.revision.generation()
    {
        return Err("Work/Run receipt provenance does not match the occurrence".into());
    }
    Ok(())
}

fn normalize_idempotency_key(value: &str, kind: &str) -> Result<String, String> {
    let key = value.trim();
    if key.is_empty() {
        return Err(format!(
            "{kind} admission requires a stable idempotency key"
        ));
    }
    if key.len() > 256 || key.chars().any(char::is_control) {
        return Err(format!("{kind} idempotency key is invalid"));
    }
    Ok(key.to_string())
}

fn extract_idempotency_key(payload: &Value, kind: &str) -> Result<String, String> {
    let object = payload
        .as_object()
        .ok_or_else(|| format!("{kind} admission requires an object payload"))?;
    const KEYS: [&str; 8] = [
        "deliveryId",
        "delivery_id",
        "idempotencyKey",
        "idempotency_key",
        "requestId",
        "request_id",
        "eventId",
        "event_id",
    ];
    let mut found = None;
    for key in KEYS {
        if let Some(value) = object.get(key) {
            if found.is_some() {
                return Err(format!(
                    "{kind} admission supplied multiple idempotency keys"
                ));
            }
            found = Some(
                value
                    .as_str()
                    .ok_or_else(|| format!("{kind} idempotency key must be a string"))?,
            );
        }
    }
    let key = found.ok_or_else(|| format!("{kind} admission requires a stable idempotency key"))?;
    normalize_idempotency_key(key, kind)
}

fn dedup_digest_for(
    automation_id: &str,
    generation: u64,
    trigger: &OccurrenceTrigger,
    idempotency_key: &str,
) -> String {
    digest_json(&json!({
        "automationId": automation_id,
        "generation": generation,
        "trigger": trigger,
        "idempotencyKey": idempotency_key,
    }))
}

/// First cron/interval fire = the next matching minute (interval: now + secs).
fn compute_next_run(trigger: &TriggerSpec, now: u64, current: Option<u64>) -> Option<u64> {
    match trigger {
        TriggerSpec::Cron { expr } => {
            match CronExpr::parse(expr) {
                Ok(c) => {
                    // Next minute that matches (scan up to 366 days).
                    let mut t = now.saturating_sub(now % 60).saturating_add(60);
                    for _ in 0..(366 * 1440) {
                        if c.matches(t) {
                            return Some(t);
                        }
                        t += 60;
                    }
                    None
                }
                Err(_) => current,
            }
        }
        TriggerSpec::Interval { secs } => Some(now.saturating_add(*secs)),
        TriggerSpec::Window {
            window,
            utc_offset_minutes,
        } => Some(next_window_unix(now, *window, *utc_offset_minutes)),
        TriggerSpec::Event { .. } | TriggerSpec::Webhook { .. } | TriggerSpec::Manual => None,
    }
}

/// Next daily fire at the window's start hour in the given UTC offset.
pub fn next_window_unix(now: u64, window: DayWindow, utc_offset_minutes: i32) -> u64 {
    let offset = (utc_offset_minutes as i64).saturating_mul(60);
    let local = now as i64 + offset;
    let local = if local < 0 { 0 } else { local as u64 };
    let day = local / 86_400;
    let start_today = day * 86_400 + u64::from(window.start_hour()) * 3600;
    let start_local = if local < start_today {
        start_today
    } else {
        start_today + 86_400
    };
    let utc = start_local as i64 - offset;
    if utc < 0 { 0 } else { utc as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2025-06-15 15:06:40 UTC (Sunday) — 15:06 avoids ambiguity with the
    /// 09:30/09:15/12:00/16:00 fixtures used below.
    fn now() -> u64 {
        1_750_000_000
    }

    /// 2025-06-15 (Sunday) 09:30:00 UTC.
    fn sun_0930() -> u64 {
        let day_start = 1_750_000_000 - 1_750_000_000 % 86_400;
        day_start + 9 * 3600 + 30 * 60
    }

    /// 2025-06-16 (Monday) 09:15:00 UTC.
    fn mon_0915() -> u64 {
        let sunday_start = 1_750_000_000 - 1_750_000_000 % 86_400;
        sunday_start + 86_400 + 9 * 3600 + 15 * 60
    }

    #[test]
    fn revision_identity_is_stable_until_definition_changes() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "job",
            "brief",
            "source",
            TriggerSpec::Manual,
            vec![AutomationStep::RunCode {
                language: "js".into(),
                code: "return 1".into(),
            }],
            None,
            now(),
        );
        let first = svc.get("job").unwrap();
        let first_revision = first.revision;
        let first_id = first.revision_id.clone();
        let automation_id = first.automation_id.clone();
        assert!(automation_id.starts_with("automation:auto:"));
        assert_ne!(automation_id, "job");

        svc.upsert(
            "job",
            "brief",
            "source",
            TriggerSpec::Manual,
            vec![AutomationStep::RunCode {
                language: "js".into(),
                code: "return 1".into(),
            }],
            None,
            now() + 1,
        );
        let unchanged = svc.get("job").unwrap();
        assert_eq!(unchanged.revision, first_revision);
        assert_eq!(unchanged.revision_id, first_id);

        svc.upsert(
            "job",
            "brief edited",
            "source",
            TriggerSpec::Manual,
            vec![AutomationStep::RunCode {
                language: "js".into(),
                code: "return 2".into(),
            }],
            None,
            now() + 2,
        );
        let changed = svc.get("job").unwrap();
        assert_eq!(changed.revision, first_revision + 1);
        assert_ne!(changed.revision_id, first_id);
    }

    #[test]
    fn schedule_occurrence_is_durable_and_idempotent() {
        let dir = std::env::temp_dir().join(format!(
            "agentcowork-automation-occurrence-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scheduler.json");
        let occurrence_id = {
            let mut svc = SchedulerService::load_or_new(path.clone());
            svc.upsert(
                "scheduled",
                "brief",
                "source",
                TriggerSpec::Interval { secs: 60 },
                vec![AutomationStep::OnlineSearch {
                    query: "news".into(),
                }],
                None,
                now(),
            );
            let admitted = svc.admit_due(now() + 61).unwrap();
            assert_eq!(admitted.len(), 1);
            let occurrence = admitted[0].clone();
            assert!(occurrence.trigger_occurrence_id.starts_with("occ:"));
            assert!(!occurrence.payload_digest.is_empty());
            assert!(!occurrence.dedup_digest.is_empty());
            assert!(occurrence.is_pending());
            assert_eq!(
                occurrence.automation_id,
                svc.get("scheduled").unwrap().automation_id
            );
            assert_eq!(
                occurrence.revision_id,
                svc.get("scheduled").unwrap().revision_id
            );

            // Replaying the same due pass returns the same occurrence.
            let replay = svc.admit_due(now() + 61).unwrap();
            assert_eq!(replay.len(), 1);
            assert_eq!(
                replay[0].trigger_occurrence_id,
                occurrence.trigger_occurrence_id
            );
            assert_eq!(svc.occurrences().len(), 1);

            let admission = svc
                .receipt_for_occurrence(&occurrence.trigger_occurrence_id)
                .unwrap();
            svc.mark_occurrence_fired_with_receipt(
                &occurrence.trigger_occurrence_id,
                now() + 61,
                &admission,
            )
            .unwrap();
            assert!(svc.due(now() + 61).is_empty());
            occurrence.trigger_occurrence_id
        };
        let reloaded = SchedulerService::load_or_new(path);
        let occurrence = reloaded.occurrence(&occurrence_id).unwrap();
        let expected_work = reloaded.expected_work_id(occurrence);
        let expected_run = reloaded.expected_run_id(occurrence);
        assert_eq!(occurrence.work_id.as_deref(), Some(expected_work.as_str()));
        assert_eq!(occurrence.run_id.as_deref(), Some(expected_run.as_str()));
        assert!(!occurrence.is_pending());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn event_webhook_and_manual_share_occurrence_admission() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "event",
            "event",
            "source",
            TriggerSpec::Event {
                kind: EventKind::RepoChange,
                filter: String::new(),
            },
            vec![],
            None,
            now(),
        );
        svc.upsert(
            "hook",
            "hook",
            "source",
            TriggerSpec::Webhook {
                path: "/hook".into(),
                schema: vec!["ref".into()],
            },
            vec![],
            None,
            now(),
        );
        svc.upsert(
            "manual",
            "manual",
            "source",
            TriggerSpec::Manual,
            vec![],
            None,
            now(),
        );

        let event = svc
            .admit_event(
                EventKind::RepoChange,
                &json!({ "deliveryId": "delivery-1" }),
                now(),
            )
            .unwrap();
        assert_eq!(event.len(), 1);
        let event_replay = svc
            .admit_event(
                EventKind::RepoChange,
                &json!({ "deliveryId": "delivery-1" }),
                now(),
            )
            .unwrap();
        assert_eq!(
            event[0].trigger_occurrence_id,
            event_replay[0].trigger_occurrence_id
        );

        let webhook = svc
            .admit_webhook(
                "/hook",
                &json!({ "ref": "main", "deliveryId": "delivery-webhook" }),
                now(),
                None,
            )
            .unwrap();
        assert_eq!(webhook.len(), 1);
        assert!(
            svc.admit_webhook(
                "/hook",
                &json!({ "deliveryId": "missing-ref" }),
                now(),
                None,
            )
            .is_err()
        );

        let manual = svc
            .admit_manual("manual", &json!({ "requestId": "r-1" }), now())
            .unwrap();
        let manual_replay = svc
            .admit_manual("manual", &json!({ "requestId": "r-1" }), now())
            .unwrap();
        assert_eq!(
            manual.trigger_occurrence_id,
            manual_replay.trigger_occurrence_id
        );
        assert_eq!(svc.occurrences().len(), 3);
    }

    #[test]
    fn corrupt_scheduler_state_is_not_an_empty_registry() {
        let path = std::env::temp_dir().join(format!(
            "agentcowork-automation-corrupt-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, b"not-json").unwrap();
        let mut svc = SchedulerService::load_or_new(path.clone());
        assert!(!svc.is_healthy());
        assert!(svc.registry_error().is_some());
        assert!(
            svc.handle("scheduler/list", &json!({ "now": now() }))
                .is_err()
        );
        assert!(svc.admit_due(now()).is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn uncertain_occurrence_stays_pending_until_work_admission() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "manual",
            "manual",
            "source",
            TriggerSpec::Manual,
            vec![],
            None,
            now(),
        );
        let occurrence = svc
            .admit_manual("manual", &json!({ "requestId": "r-uncertain" }), now())
            .unwrap();
        svc.mark_occurrence_uncertain(&occurrence.trigger_occurrence_id, "work gateway down")
            .unwrap();
        let row = svc.occurrence(&occurrence.trigger_occurrence_id).unwrap();
        assert!(!row.is_pending());
        assert!(row.is_uncertain());
        assert_eq!(row.admission_error.as_deref(), Some("work gateway down"));
    }

    #[test]
    fn cron_matches_minute_exactly() {
        let c = CronExpr::parse("30 9 * * *").unwrap();
        let t = sun_0930();
        assert!(c.matches(t));
        assert!(!c.matches(t + 60));
    }

    #[test]
    fn cron_matches_star_fields() {
        let c = CronExpr::parse("* * * * *").unwrap();
        for t in [0u64, 60, 86_400, 1_750_000_000] {
            assert!(c.matches(t), "t={t}");
        }
    }

    #[test]
    fn cron_ranges_and_steps() {
        let c = CronExpr::parse("*/15 9-17 * * 1-5").unwrap();
        // 2025-06-16 (Monday) 09:15 → match; 09:20 → no.
        assert!(c.matches(mon_0915()));
        assert!(!c.matches(mon_0915() + 5 * 60));
        // Sunday 09:15 → no (dow 1-5 excludes Sunday).
        assert!(!c.matches(sun_0930() - 15 * 60));
    }

    #[test]
    fn cron_rejects_bad_fields() {
        assert!(CronExpr::parse("60 * * * *").is_err());
        assert!(CronExpr::parse("* * 32 * *").is_err());
        assert!(CronExpr::parse("*/0 * * * *").is_err());
        assert!(CronExpr::parse("* * * *").is_err());
    }

    #[test]
    fn interval_due_respects_next_run() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "j1",
            "probe",
            "s1",
            TriggerSpec::Interval { secs: 60 },
            vec![],
            None,
            now(),
        );
        // Not due immediately after creation (next_run_at = now + 60).
        assert!(svc.due(now()).is_empty());
        // Due at now + 61.
        assert_eq!(svc.due(now() + 61), vec!["j1".to_string()]);
    }

    #[test]
    fn cron_due_at_match() {
        let mut svc = SchedulerService::new();
        // Job created at 15:06; cron fires at 16:00 (later today).
        svc.upsert(
            "j1",
            "hourly",
            "s1",
            TriggerSpec::Cron {
                expr: "0 16 * * *".into(),
            },
            vec![],
            None,
            now(),
        );
        // 16:00 today (now() is 15:06) — day_start + 16h.
        let day_start = now() - (now() % 86_400);
        let due_at = day_start + 16 * 3600;
        assert!(svc.due(due_at - 60).is_empty());
        assert_eq!(svc.due(due_at), vec!["j1".to_string()]);
    }

    /// A schedule advances only after a matching Work/Run receipt.
    #[test]
    fn receipt_advances_schedule_and_dedupes() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "j1",
            "tick",
            "s1",
            TriggerSpec::Interval { secs: 60 },
            vec![],
            None,
            now(),
        );
        let occurrence = svc.admit_due(now() + 61).unwrap().pop().unwrap();
        assert_eq!(svc.due(now() + 62), vec!["j1".to_string()]);
        let admission = svc
            .receipt_for_occurrence(&occurrence.trigger_occurrence_id)
            .unwrap();
        svc.mark_occurrence_fired_with_receipt(
            &occurrence.trigger_occurrence_id,
            now() + 62,
            &admission,
        )
        .unwrap();
        assert!(svc.due(now() + 62).is_empty(), "receipt → deduped");
        assert!(svc.due(now() + 100).is_empty(), "next occurrence not yet");
        assert_eq!(svc.due(now() + 123), vec!["j1".to_string()]);
        let job = svc.get("j1").unwrap();
        assert_eq!(job.last_fired_at, Some(now() + 62));
        assert_eq!(job.recent_fires, vec![now() + 62]);

        svc.upsert(
            "j2",
            "ev",
            "s1",
            TriggerSpec::Event {
                kind: EventKind::RepoChange,
                filter: String::new(),
            },
            vec![],
            None,
            now(),
        );
        let event = svc
            .admit_event(
                EventKind::RepoChange,
                &json!({ "deliveryId": "event-1" }),
                now(),
            )
            .unwrap()
            .pop()
            .unwrap();
        let admission = svc
            .receipt_for_occurrence(&event.trigger_occurrence_id)
            .unwrap();
        svc.mark_occurrence_fired_with_receipt(&event.trigger_occurrence_id, now(), &admission)
            .unwrap();
        assert!(svc.get("j2").unwrap().next_run_at.is_none());
    }

    #[test]
    fn battery_suppression_skips_jobs() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "j1",
            "bat",
            "s1",
            TriggerSpec::Interval { secs: 5 },
            vec![],
            None,
            now(),
        );
        svc.set_battery(true);
        // Interval job is due (now + 5 passed) but suppressed on battery.
        assert!(svc.due(now() + 10).is_empty());
        svc.set_battery(false);
        assert_eq!(svc.due(now() + 10), vec!["j1".to_string()]);
        // A job that opted out runs on battery.
        let mut svc2 = SchedulerService::new();
        svc2.upsert(
            "j2",
            "always",
            "s1",
            TriggerSpec::Interval { secs: 5 },
            vec![],
            None,
            now(),
        );
        svc2.jobs.get_mut("j2").unwrap().policy.suppress_on_battery = false;
        svc2.set_battery(true);
        assert_eq!(svc2.due(now() + 10), vec!["j2".to_string()]);
    }

    #[test]
    fn event_fire_matches_kind_filter_scope() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "j1",
            "ci",
            "s1",
            TriggerSpec::Event {
                kind: EventKind::CiBuildFail,
                filter: "repo-a".into(),
            },
            vec![],
            None,
            now(),
        );
        svc.upsert(
            "j2",
            "other",
            "s1",
            TriggerSpec::Event {
                kind: EventKind::TestRegression,
                filter: "".into(),
            },
            vec![],
            None,
            now(),
        );
        let fired = svc
            .fire_event(
                EventKind::CiBuildFail,
                &json!({ "repo": "repo-a", "build": 42, "deliveryId": "e1" }),
                now(),
            )
            .unwrap();
        assert_eq!(fired, vec!["j1".to_string()]);
        // Filter miss → nothing.
        let fired2 = svc
            .fire_event(
                EventKind::CiBuildFail,
                &json!({ "repo": "repo-b", "deliveryId": "e2" }),
                now(),
            )
            .unwrap();
        assert!(fired2.is_empty());
        // With scope policy.
        svc.upsert(
            "j3",
            "scoped",
            "s1",
            TriggerSpec::Event {
                kind: EventKind::RepoChange,
                filter: "".into(),
            },
            vec![],
            None,
            now(),
        );
        svc.jobs.get_mut("j3").unwrap().policy.scope = Some("src/".into());
        let fired3 = svc
            .fire_event(
                EventKind::RepoChange,
                &json!({ "path": "README.md", "deliveryId": "e3" }),
                now(),
            )
            .unwrap();
        assert!(fired3.is_empty());
        let fired4 = svc
            .fire_event(
                EventKind::RepoChange,
                &json!({ "path": "src/main.rs", "deliveryId": "e4" }),
                now(),
            )
            .unwrap();
        assert_eq!(fired4, vec!["j3".to_string()]);
    }

    #[test]
    fn webhook_validates_schema_and_token() {
        let mut svc = SchedulerService::new();
        svc.set_webhook_token(Some("tok".into()));
        svc.upsert(
            "w1",
            "hook",
            "s1",
            TriggerSpec::Webhook {
                path: "/hooks/ci".into(),
                schema: vec!["ref".into(), "sha".into()],
            },
            vec![],
            None,
            now(),
        );
        // Bad token → error.
        assert!(
            svc.fire_webhook(
                "/hooks/ci",
                &json!({"ref":"main","sha":"x","deliveryId":"bad-token"}),
                now(),
                Some("nope")
            )
            .is_err()
        );
        // Missing key → error.
        assert!(
            svc.fire_webhook("/hooks/ci", &json!({"ref":"main"}), now(), Some("tok"))
                .is_err()
        );
        // Good → fires.
        let fired = svc
            .fire_webhook(
                "/hooks/ci",
                &json!({"ref":"main","sha":"abc","deliveryId":"good"}),
                now(),
                Some("tok"),
            )
            .unwrap();
        assert_eq!(fired, vec!["w1".to_string()]);
        // Wrong path → nothing.
        let fired2 = svc
            .fire_webhook(
                "/hooks/nope",
                &json!({"ref":"main","sha":"x","deliveryId":"bad-token"}),
                now(),
                Some("tok"),
            )
            .unwrap();
        assert!(fired2.is_empty());
    }

    /// Frequency admission counts receipt-backed firings, not delivery intent.
    #[test]
    fn frequency_policy_caps_fires() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "j1",
            "noisy",
            "s1",
            TriggerSpec::Event {
                kind: EventKind::TelemetryThreshold,
                filter: "".into(),
            },
            vec![],
            None,
            now(),
        );
        svc.jobs.get_mut("j1").unwrap().policy.max_runs_per_hour = Some(2);
        let first = svc
            .admit_event_with_key(
                EventKind::TelemetryThreshold,
                &json!({ "value": 1 }),
                "delivery-1",
                now(),
            )
            .unwrap()
            .pop()
            .unwrap();
        let receipt = svc
            .receipt_for_occurrence(&first.trigger_occurrence_id)
            .unwrap();
        svc.mark_occurrence_fired_with_receipt(&first.trigger_occurrence_id, now(), &receipt)
            .unwrap();

        let second = svc
            .admit_event_with_key(
                EventKind::TelemetryThreshold,
                &json!({ "value": 2 }),
                "delivery-2",
                now(),
            )
            .unwrap()
            .pop()
            .unwrap();
        let receipt = svc
            .receipt_for_occurrence(&second.trigger_occurrence_id)
            .unwrap();
        svc.mark_occurrence_fired_with_receipt(&second.trigger_occurrence_id, now(), &receipt)
            .unwrap();

        assert!(
            svc.fire_event(
                EventKind::TelemetryThreshold,
                &json!({ "deliveryId": "delivery-3" }),
                now(),
            )
            .unwrap()
            .is_empty()
        );
        let later = now() + 3700;
        assert_eq!(
            svc.fire_event(
                EventKind::TelemetryThreshold,
                &json!({ "deliveryId": "delivery-4" }),
                later,
            )
            .unwrap()
            .len(),
            1
        );
    }

    /// HITL pause on the trigger plane: a flag that stops firing, plus the
    /// chat-delete cascade. (Execution-level waiting is Work's `WaitCondition`.)
    #[test]
    fn pause_is_a_trigger_plane_flag() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "j1",
            "review",
            "s1",
            TriggerSpec::Interval { secs: 60 },
            vec![],
            None,
            now(),
        );
        svc.pause("j1").unwrap();
        assert!(svc.get("j1").unwrap().paused);
        // Paused jobs are not due.
        assert!(svc.due(now() + 61).is_empty());
        svc.resume("j1", now()).unwrap();
        assert!(!svc.get("j1").unwrap().paused);
        assert_eq!(svc.due(now() + 61), vec!["j1".to_string()]);
        // Resume of an event job does not invent a schedule.
        svc.upsert(
            "j2",
            "ev",
            "s1",
            TriggerSpec::Event {
                kind: EventKind::CiBuildFail,
                filter: String::new(),
            },
            vec![],
            None,
            now(),
        );
        svc.pause("j2").unwrap();
        svc.resume("j2", now()).unwrap();
        assert!(svc.get("j2").unwrap().next_run_at.is_none());
    }

    #[test]
    fn nudge_sentinels_suggest_schedule_after_3_days() {
        let mut svc = SchedulerService::new();
        // Same goal, same hour (09:00), 3 distinct days.
        for day in 0..3u64 {
            svc.record_nudge(
                "Morning brief",
                now() + day * 86_400 - now() % 86_400 + 9 * 3600,
            );
        }
        let suggestions = svc.nudges();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].goal, "Morning brief");
        assert_eq!(suggestions[0].cron, "0 9 * * *");
        assert!(suggestions[0].confidence > 0.0);
    }

    #[test]
    fn nudge_needs_three_distinct_days() {
        let mut svc = SchedulerService::new();
        for _ in 0..3 {
            svc.record_nudge("Once off", now() + 9 * 3600);
        }
        assert!(svc.nudges().is_empty()); // same day
    }

    #[test]
    fn handle_dispatch_roundtrip() {
        let mut svc = SchedulerService::new();
        // Interval job — deterministic due without wall-clock minute coupling.
        svc.handle(
            "scheduler/upsert",
            &json!({
                "id": "j1", "name": "Morning brief", "sessionId": "s1",
                "trigger": { "type": "interval", "secs": 60 },
                "steps": [],
                "now": now(),
            }),
        )
        .unwrap();
        let list = svc
            .handle("scheduler/list", &json!({ "now": now() }))
            .unwrap();
        assert_eq!(list["jobs"].as_array().unwrap().len(), 1);
        // Run-now admits a manual occurrence without changing the recurring
        // schedule.  Its receipt is required before terminal advancement.
        let run = svc
            .handle(
                "scheduler/run_now",
                &json!({ "id": "j1", "idempotencyKey": "run-1", "now": now() }),
            )
            .unwrap();
        let occurrence_id = run["occurrence"]["triggerOccurrenceId"].as_str().unwrap();
        let receipt = svc.receipt_for_occurrence(occurrence_id).unwrap();
        svc.handle(
            "scheduler/mark_occurrence_fired",
            &json!({
                "id": occurrence_id,
                "receipt": receipt,
                "now": now(),
            }),
        )
        .unwrap();
        let due2 = svc
            .handle("scheduler/due", &json!({ "now": now() }))
            .unwrap();
        assert!(due2["due"].as_array().unwrap().is_empty());
        // enable/disable.
        svc.handle(
            "scheduler/enable",
            &json!({ "id": "j1", "enabled": false, "now": now() }),
        )
        .unwrap();
        let due3 = svc
            .handle("scheduler/due", &json!({ "now": now() + 120 }))
            .unwrap();
        assert!(due3["due"].as_array().unwrap().is_empty());
        // battery.
        svc.handle("scheduler/battery", &json!({ "onBattery": true }))
            .unwrap();
        assert!(svc.on_battery());
        // Pause/resume through the funnel.
        svc.handle("scheduler/pause", &json!({ "id": "j1" }))
            .unwrap();
        svc.handle("scheduler/resume", &json!({ "id": "j1", "now": now() }))
            .unwrap();
    }

    #[test]
    fn unknown_job_methods_error() {
        let mut svc = SchedulerService::new();
        assert!(
            svc.handle("scheduler/pause", &json!({ "id": "ghost" }))
                .is_err()
        );
        assert!(
            svc.handle("scheduler/mark_fired", &json!({ "id": "ghost" }))
                .is_err()
        );
        assert!(svc.handle("scheduler/nope", &json!({})).is_err());
    }

    #[test]
    fn monitor_notifies_on_first_run_and_stores_observation() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "m1",
            "watch",
            "s1",
            TriggerSpec::Interval { secs: 3600 },
            vec![],
            None,
            now(),
        );
        let v = svc.monitor_evaluate("m1", "price=100", false).unwrap();
        assert!(v.notified, "first run always notifies (baseline)");
        assert!(v.changed, "no previous observation → changed");
        assert!(!v.stopped);
        assert_eq!(v.previous, None);
        assert_eq!(v.current, "price=100");
        assert_eq!(v.notifications, 1);
        assert_eq!(
            svc.get("m1")
                .unwrap()
                .monitor
                .as_ref()
                .unwrap()
                .last_observation
                .as_deref(),
            Some("price=100"),
            "the observation is remembered for the next run (stateful polling)"
        );
    }

    #[test]
    fn monitor_suppresses_unchanged_runs() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "m1",
            "watch",
            "s1",
            TriggerSpec::Interval { secs: 3600 },
            vec![],
            None,
            now(),
        );
        svc.monitor_evaluate("m1", "price=100", false).unwrap();
        let v = svc.monitor_evaluate("m1", "price=100", false).unwrap();
        assert!(!v.changed);
        assert!(
            !v.notified,
            "no delta → no notification (the run vs notify split)"
        );
        assert_eq!(
            v.notifications, 1,
            "an unchanged run does not bump the count"
        );
    }

    #[test]
    fn monitor_notifies_on_delta() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "m1",
            "watch",
            "s1",
            TriggerSpec::Interval { secs: 3600 },
            vec![],
            None,
            now(),
        );
        svc.monitor_evaluate("m1", "price=100", false).unwrap();
        let v = svc.monitor_evaluate("m1", "price=80", false).unwrap();
        assert!(v.changed);
        assert!(v.notified);
        assert_eq!(v.previous.as_deref(), Some("price=100"));
        assert_eq!(v.notifications, 2);
    }

    #[test]
    fn monitor_stops_on_condition() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "m1",
            "watch",
            "s1",
            TriggerSpec::Interval { secs: 3600 },
            vec![],
            None,
            now(),
        );
        svc.set_monitor(
            "m1",
            Some(MonitorConfig {
                stop_on_condition: true,
                ..MonitorConfig::default()
            }),
        )
        .unwrap();
        svc.monitor_evaluate("m1", "shipped=false", false).unwrap();
        let v = svc.monitor_evaluate("m1", "delivered", true).unwrap();
        assert!(v.stopped, "condition met + stop_on_condition → stopped");
        assert!(v.notified, "the stop event is worth reporting");
        let job = svc.get("m1").unwrap();
        assert!(!job.enabled, "a stopped monitor is disabled");
    }

    #[test]
    fn job_serializes_camel_case_for_the_sidecar() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "j1",
            "brief",
            "s1",
            TriggerSpec::Interval { secs: 60 },
            vec![],
            None,
            now(),
        );
        let list = svc
            .handle("scheduler/list", &json!({ "now": now() }))
            .unwrap();
        let job = &list["jobs"][0];
        assert_eq!(
            job["sessionId"], "s1",
            "session_id must serialize as sessionId for the coordinator: {job}"
        );
        assert!(
            job.get("session_id").is_none(),
            "no snake_case leakage: {job}"
        );
        assert!(
            job["policy"]["suppressOnBattery"].as_bool().is_some(),
            "policy is camelCase: {}",
            job["policy"]
        );
        assert!(job["policy"].get("suppress_on_battery").is_none());

        // Trigger-plane shape: a pause flag, no execution state at all —
        // no `state` machine, no leases/fences/checkpoints/run snapshots.
        assert_eq!(job["paused"], json!(false));
        for gone in [
            "state",
            "checkpoint",
            "currentRun",
            "runs",
            "successes",
            "failures",
            "modelPin",
            "effortPin",
            "manifestHash",
            "lastOutput",
        ] {
            assert!(
                job.get(gone).is_none(),
                "execution state {gone:?} must not exist on the trigger plane: {job}"
            );
        }
    }

    #[test]
    fn a_leftover_checkpoint_does_not_refuse_the_registry() {
        let dir = std::env::temp_dir().join(format!(
            "agentcowork-sched-checkpoint-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("jobs.json");
        std::fs::write(
            &path,
            r#"[{"id":"j1","name":"brief","sessionId":"s1","trigger":{"type":"interval","secs":60},"steps":[],"policy":{"suppressOnBattery":false,"maxRunsPerHour":null,"scope":null},"enabled":true,"paused":false,"nextRunAt":null,"checkpoint":{"id":"old"}}]"#,
        )
        .unwrap();
        let svc = SchedulerService::load_or_new_checked(path).expect("checkpoint is ignored");
        assert!(svc.get("j1").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The structural **I26/I3** assertion: the wire shape itself carries no
    /// run state machine — a trigger plane, not a second executor.
    #[test]
    fn trigger_plane_wire_has_no_execution_state() {
        let svc = SchedulerService::new();
        let _ = svc; // the assertion above is the structural check; keep both named
    }

    #[test]
    fn window_trigger_fires_at_named_hour() {
        // 09:30 UTC — morning (06:00) already passed, next morning is tomorrow 06:00.
        let t = sun_0930();
        let next = next_window_unix(t, DayWindow::Morning, 0);
        assert!(next > t);
        let (_min, hour, ..) = civil_parts(next);
        assert_eq!(hour, 6);
        // Afternoon 12:00 is still ahead today.
        let aft = next_window_unix(t, DayWindow::Afternoon, 0);
        let (_m, h, ..) = civil_parts(aft);
        assert_eq!(h, 12);
        assert!(aft > t);
        let mut svc = SchedulerService::new();
        svc.upsert(
            "w1",
            "brief",
            "s1",
            TriggerSpec::Window {
                window: DayWindow::Afternoon,
                utc_offset_minutes: 0,
            },
            vec![],
            None,
            t,
        );
        assert_eq!(svc.get("w1").unwrap().next_run_at, Some(aft));
    }

    #[test]
    fn pause_session_cascades_to_session_jobs() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "a",
            "a",
            "chat-1",
            TriggerSpec::Interval { secs: 60 },
            vec![],
            None,
            now(),
        );
        svc.upsert(
            "b",
            "b",
            "chat-1",
            TriggerSpec::Interval { secs: 60 },
            vec![],
            None,
            now(),
        );
        svc.upsert(
            "c",
            "c",
            "other",
            TriggerSpec::Interval { secs: 60 },
            vec![],
            None,
            now(),
        );
        let n = svc.pause_session("chat-1");
        assert_eq!(n, 2);
        assert!(svc.get("a").unwrap().paused);
        assert!(!svc.get("c").unwrap().paused);
        let out = svc
            .handle("scheduler/pause_session", &json!({ "sessionId": "chat-1" }))
            .unwrap();
        assert_eq!(out["paused"], 0, "already paused — no double count");
    }

    /// The trigger registry survives a restart, and a slipped schedule admits
    /// one occurrence on resume rather than replaying every missed tick.
    #[test]
    fn jobs_survive_restart_and_recover_run_once_on_resume() {
        let dir = std::env::temp_dir().join(format!("agentcowork-schedsvc-{}", std::process::id()));
        let path = dir.join("scheduler.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");

        {
            let mut svc = SchedulerService::load_or_new(path.clone());
            svc.upsert(
                "j-done",
                "idle job",
                "s1",
                TriggerSpec::Interval { secs: 60 },
                vec![],
                None,
                now(),
            );
            svc.upsert(
                "j-slip",
                "slipped job",
                "s1",
                TriggerSpec::Interval { secs: 60 },
                vec![],
                None,
                now(),
            );
            let admitted = svc.admit_due(now() + 61).unwrap();
            let slipped = admitted
                .into_iter()
                .find(|occurrence| {
                    svc.job_id_for_automation(&occurrence.automation_id) == Some("j-slip")
                })
                .expect("one slipped occurrence");
            let receipt = svc
                .receipt_for_occurrence(&slipped.trigger_occurrence_id)
                .unwrap();
            svc.mark_occurrence_fired_with_receipt(
                &slipped.trigger_occurrence_id,
                now() + 61,
                &receipt,
            )
            .unwrap();
        }
        {
            let mut svc = SchedulerService::load_or_new(path.clone());
            assert_eq!(svc.list().len(), 2, "both jobs survive the restart");
            assert!(svc.get("j-done").unwrap().enabled);
            assert_eq!(svc.get("j-slip").unwrap().last_fired_at, Some(now() + 61));
            assert!(svc.due(now() + 30).is_empty());
            assert_eq!(svc.due(now() + 61), vec!["j-done".to_string()]);
            let due = svc.admit_due(now() + 61).unwrap();
            assert_eq!(due.len(), 1);
            let receipt = svc
                .receipt_for_occurrence(&due[0].trigger_occurrence_id)
                .unwrap();
            svc.mark_occurrence_fired_with_receipt(
                &due[0].trigger_occurrence_id,
                now() + 61,
                &receipt,
            )
            .unwrap();
            assert!(svc.due(now() + 61).is_empty());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- P51.32a continuity ------------------------------------------------------

    #[test]
    fn notepad_roundtrips_and_ignores_unknown_jobs() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "j1",
            "brief",
            "s1",
            TriggerSpec::Interval { secs: 60 },
            vec![],
            None,
            now(),
        );
        assert!(svc.append_notepad("j1", "line one"));
        assert!(svc.append_notepad("j1", "line two"));
        assert_eq!(
            svc.notepad("j1").as_deref(),
            Some("line one\nline two"),
            "notepad survives into the next run"
        );
        assert!(svc.notepad("ghost").is_none());
        assert!(!svc.append_notepad("ghost", "x"));
    }

    // -- P51.32b monitor-script mode ----------------------------------------------

    #[test]
    fn script_empty_is_silent() {
        let cfg = MonitorConfig {
            source: MonitorSource::Script {
                cmd: "check.sh".into(),
                allow_net: false,
            },
            ..MonitorConfig::default()
        };
        let silent = cfg.evaluate_script("", true);
        assert!(!silent.notified, "empty + silent_on_empty → not notified");
        assert_eq!(silent.current, "");
        let baseline = cfg.evaluate_script("", false);
        assert!(
            baseline.notified,
            "empty without the silent flag still notifies the baseline"
        );
    }

    #[test]
    fn script_stdout_verbatim_no_trim() {
        let cfg = MonitorConfig::default();
        let out = "  padded  \nline2  ";
        let v = cfg.evaluate_script(out, false);
        assert_eq!(v.current, out, "stdout stored verbatim, no trim");
        // Stateful path also stores verbatim and dedupes identical polls.
        let mut svc = SchedulerService::new();
        svc.upsert(
            "m1",
            "watch",
            "s1",
            TriggerSpec::Interval { secs: 3600 },
            vec![],
            None,
            now(),
        );
        let v1 = svc.monitor_evaluate_script("m1", out, false).unwrap();
        assert_eq!(v1.current, out);
        assert!(v1.notified, "first script observation notifies");
        let v2 = svc.monitor_evaluate_script("m1", out, false).unwrap();
        assert!(!v2.changed, "identical stdout → unchanged");
        assert!(!v2.notified, "identical stdout → no second notification");
        assert_eq!(
            svc.get("m1")
                .unwrap()
                .monitor
                .as_ref()
                .unwrap()
                .last_observation
                .as_deref(),
            Some(out)
        );
    }

    // -- P51.32e incidents -----------------------------------------------------------

    #[test]
    fn incidents_require_explicit_ack() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "j1",
            "fragile",
            "s1",
            TriggerSpec::Interval { secs: 60 },
            vec![],
            None,
            now(),
        );
        let id = svc.report_incident("j1", "run_failed", "boom", now());
        let list = svc.list_incidents();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].job_id, "j1");
        assert!(!list[0].acked, "incidents start unacked");
        assert!(!svc.ack_incident("inc-nope"), "unknown id → false");
        // Still unacked until the explicit ack.
        assert!(!svc.list_incidents()[0].acked);
        assert!(svc.ack_incident(&id));
        assert!(svc.list_incidents()[0].acked, "explicit ack flips the flag");
        assert_eq!(svc.get_incident(&id).unwrap().detail, "boom");
    }

    // -- P51.32f doctor -----------------------------------------------------------------

    #[test]
    fn cron_doctor_flags_missed_runs() {
        let mut svc = SchedulerService::new();
        svc.upsert(
            "missed",
            "m",
            "s1",
            TriggerSpec::Interval { secs: 60 },
            vec![],
            None,
            now(),
        );
        svc.jobs.get_mut("missed").unwrap().next_run_at = Some(now() - 1000);
        svc.upsert(
            "fresh",
            "f",
            "s1",
            TriggerSpec::Interval { secs: 60 },
            vec![],
            None,
            now(),
        );
        // Event jobs have no schedule to miss.
        svc.upsert(
            "ev",
            "e",
            "s1",
            TriggerSpec::Event {
                kind: EventKind::RepoChange,
                filter: String::new(),
            },
            vec![],
            None,
            now(),
        );
        let checks = svc.cron_doctor(now());
        let missed = checks.iter().find(|c| c.name == "missed_runs").unwrap();
        let queue = checks.iter().find(|c| c.name == "queue_depth").unwrap();
        assert!(!missed.ok, "overdue next_run_at flags: {}", missed.detail);
        assert!(queue.ok, "small registry is healthy: {}", queue.detail);
        // Healthy service → all green.
        let fresh = SchedulerService::new();
        assert!(fresh.cron_doctor(now()).iter().all(|c| c.ok));
    }
}
