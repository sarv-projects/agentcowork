//! P6.4 (B7) / P71.3d — scheduled-task commands over the **trigger plane**.
//! Thin wrappers over the shared `agentcowork-core::SchedulerService` (job
//! registry, cron/interval/event/webhook/window triggers, battery/misfire/
//! admission policy, nudge sentinels). The shell exposes the job list,
//! create/delete/enable/pause/resume/run-now, battery state, event fires and
//! nudge suggestions to the UI; execution is the Work kernel's business
//! (`ARCH/AUTOMATION.md` §9) — no leases, retries or run ledger live here.

use agentcowork_core::SchedulerService;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

use crate::AppState;

/// P65.4 — share the one scheduler handle with the Settings envelope.
/// Same source as every command above (the relay-owned service); a second
/// scheduler is never constructed here.
pub(crate) fn scheduler_handle(state: &AppState) -> Result<Arc<Mutex<SchedulerService>>, String> {
    let relay = state.chat_relay.lock().map_err(|e| e.to_string())?;
    let relay = relay
        .as_ref()
        .ok_or_else(|| "sidecar not connected — scheduler service not ready".to_string())?;
    Ok(relay.scheduler())
}

/// Clone the shared scheduler service handle through the relay (single source
/// of truth — the coordinator drives the same instance over `scheduler/*`).
/// The returned `Arc` is independent of the relay guard, so commands can lock
/// it without lifetime gymnastics.
fn svc(
    state: &State<'_, AppState>,
) -> Result<std::sync::Arc<std::sync::Mutex<SchedulerService>>, String> {
    let relay = state.chat_relay.lock().map_err(|e| e.to_string())?;
    let relay = relay
        .as_ref()
        .ok_or_else(|| "sidecar not connected — scheduler service not ready".to_string())?;
    Ok(relay.scheduler())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Validate and preserve the caller-owned delivery identity for an ingress
/// request. The scheduler service treats this as an explicit input; a missing
/// key must never fall back to a timestamp or a payload digest at this host
/// boundary.
fn require_idempotency_key(kind: &str, value: &str) -> Result<String, String> {
    let key = value.trim();
    if key.is_empty() {
        return Err(format!("{kind} ingress requires a caller idempotency key"));
    }
    if key.len() > 256 || key.chars().any(char::is_control) {
        return Err(format!("{kind} ingress idempotency key is invalid"));
    }
    Ok(key.to_string())
}

fn manual_ingress_params(id: &str, idempotency_key: &str, now: u64) -> Result<Value, String> {
    let idempotency_key = require_idempotency_key("manual", idempotency_key)?;
    Ok(serde_json::json!({
        "id": id,
        "now": now,
        "idempotencyKey": idempotency_key,
    }))
}

fn event_ingress_params(
    kind: &str,
    payload: Value,
    idempotency_key: &str,
    now: u64,
) -> Result<Value, String> {
    let idempotency_key = require_idempotency_key("event", idempotency_key)?;
    Ok(serde_json::json!({
        "kind": kind,
        "payload": payload,
        "now": now,
        "idempotencyKey": idempotency_key,
    }))
}

fn webhook_ingress_params(
    path: &str,
    body: Value,
    token: Option<&str>,
    idempotency_key: &str,
    now: u64,
) -> Result<Value, String> {
    let idempotency_key = require_idempotency_key("webhook", idempotency_key)?;
    Ok(serde_json::json!({
        "path": path,
        "body": body,
        "token": token,
        "now": now,
        "idempotencyKey": idempotency_key,
    }))
}

/// The full job list + battery state (the H14 scheduled-tasks surface).
#[tauri::command]
pub fn scheduler_list(state: State<'_, AppState>) -> Result<Value, String> {
    let handle = svc(&state)?;
    let svc = handle.lock().map_err(|e| e.to_string())?;
    svc.ensure_public_health()?;
    let jobs: Vec<Value> = svc
        .list()
        .iter()
        .map(|j| serde_json::to_value(j).unwrap_or(Value::Null))
        .collect();
    let occurrences: Vec<Value> = svc
        .occurrences()
        .into_iter()
        .map(|occurrence| serde_json::to_value(occurrence).unwrap_or(Value::Null))
        .collect();
    Ok(serde_json::json!({
        "jobs": jobs,
        "onBattery": svc.on_battery(),
        "occurrences": occurrences,
    }))
}

/// Create (or replace) a scheduled job. `trigger` is the `TriggerSpec` serde
/// shape (`{"type":"cron","expr":"0 8 * * *"}`, `interval`, `event`, or
/// `webhook`); `steps` are `AutomationStep`s; `policy` is optional.
#[tauri::command]
pub fn scheduler_create(
    state: State<'_, AppState>,
    id: String,
    name: String,
    session_id: String,
    trigger: Value,
    steps: Value,
    policy: Option<Value>,
) -> Result<bool, String> {
    let handle = svc(&state)?;
    let mut svc = handle.lock().map_err(|e| e.to_string())?;
    let trigger = serde_json::from_value(trigger).map_err(|e| format!("bad trigger: {e}"))?;
    let steps = serde_json::from_value(steps).map_err(|e| format!("bad steps: {e}"))?;
    let policy = policy
        .map(|p| serde_json::from_value(p).map_err(|e| format!("bad policy: {e}")))
        .transpose()?;
    svc.upsert_checked(id, name, session_id, trigger, steps, policy, now_secs())?;
    Ok(true)
}

/// Delete a job.
#[tauri::command]
pub fn scheduler_delete(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let handle = svc(&state)?;
    let mut svc = handle.lock().map_err(|e| e.to_string())?;
    svc.delete_checked(&id)
}

/// Enable/disable a job.
#[tauri::command]
pub fn scheduler_enable(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<bool, String> {
    let handle = svc(&state)?;
    let mut svc = handle.lock().map_err(|e| e.to_string())?;
    svc.set_enabled(&id, enabled, now_secs())
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// Pause every job bound to a chat session (delete-chat cascade).
#[tauri::command]
pub fn scheduler_pause_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<u32, String> {
    let handle = svc(&state)?;
    let mut svc = handle.lock().map_err(|e| e.to_string())?;
    svc.pause_session_checked(&session_id)
        .map(|count| count as u32)
        .map_err(|e| e.to_string())
}

/// HITL pause (trigger-plane flag: stop firing; Work owns execution waits).
#[tauri::command]
pub fn scheduler_pause(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let handle = svc(&state)?;
    let mut svc = handle.lock().map_err(|e| e.to_string())?;
    svc.pause(&id).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Resume a paused job.
#[tauri::command]
pub fn scheduler_resume(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let handle = svc(&state)?;
    let mut svc = handle.lock().map_err(|e| e.to_string())?;
    svc.resume(&id, now_secs()).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Force a job into the next due pass (the UI's "Run now" button).
///
/// `idempotency_key` is supplied by the UI action owner. It is required so a
/// retry of the same gesture returns the existing occurrence rather than
/// minting a new identity from the wall clock.
#[tauri::command]
pub fn scheduler_run_now(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
    idempotency_key: String,
) -> Result<bool, String> {
    let handle = svc(&state)?;
    let now = now_secs();
    let params = manual_ingress_params(&id, &idempotency_key, now)?;
    {
        let mut svc = handle.lock().map_err(|e| e.to_string())?;
        svc.handle("scheduler/run_now", &params)
            .map_err(|e| e.to_string())?;
    }
    // Manual admission is only metadata; the host dispatcher below is the
    // single Work/Run creation path shared with schedules and ingress.
    crate::scheduler_fire::fire_due_checked(&app)?;
    Ok(true)
}

/// Report the device battery state (battery-aware scheduling suppresses jobs
/// with `suppress_on_battery` while on battery).
#[tauri::command]
pub fn scheduler_battery(state: State<'_, AppState>, on_battery: bool) -> Result<bool, String> {
    let handle = svc(&state)?;
    let mut svc = handle.lock().map_err(|e| e.to_string())?;
    svc.set_battery(on_battery);
    // J16 — mirror to the shared AppState flag so the storage commands (heavy
    // scans) defer from the same OS power event.
    state
        .battery
        .store(on_battery, std::sync::atomic::Ordering::Relaxed);
    Ok(true)
}

/// Fire an event trigger (CI build-fail / test-regression / repo-change /
/// ticket-assign / telemetry-threshold) with scope+frequency policy.
///
/// The event producer owns the stable delivery key and must reuse it when
/// replaying a delivery. The command refuses a missing key instead of asking
/// the service to derive one from the event payload.
#[tauri::command]
pub fn scheduler_fire_event(
    state: State<'_, AppState>,
    app: AppHandle,
    kind: String,
    payload: Value,
    idempotency_key: String,
) -> Result<Vec<String>, String> {
    let handle = svc(&state)?;
    let params = event_ingress_params(&kind, payload, &idempotency_key, now_secs())?;
    let ids: Vec<String> = {
        let mut svc = handle.lock().map_err(|e| e.to_string())?;
        let response = svc.handle("scheduler/fire_event", &params)?;
        serde_json::from_value(
            response
                .get("fired")
                .cloned()
                .ok_or_else(|| "scheduler event admission returned no fired ids".to_string())?,
        )
        .map_err(|e| format!("decode admitted event ids: {e}"))?
    };
    crate::scheduler_fire::fire_due_checked(&app)?;
    Ok(ids)
}

/// Webhook ingress (F11 loopback): validate path + required keys, queue jobs.
///
/// The webhook caller supplies the delivery key (normally the HTTP
/// `Idempotency-Key` header). It is carried explicitly to the shared service;
/// no host-side timestamp or body digest is used as a replacement.
#[tauri::command]
pub fn scheduler_fire_webhook(
    state: State<'_, AppState>,
    app: AppHandle,
    path: String,
    body: Value,
    token: Option<String>,
    idempotency_key: String,
) -> Result<Vec<String>, String> {
    let handle = svc(&state)?;
    let params =
        webhook_ingress_params(&path, body, token.as_deref(), &idempotency_key, now_secs())?;
    let ids: Vec<String> = {
        let mut svc = handle.lock().map_err(|e| e.to_string())?;
        let response = svc.handle("scheduler/fire_webhook", &params)?;
        serde_json::from_value(
            response
                .get("fired")
                .cloned()
                .ok_or_else(|| "scheduler webhook admission returned no fired ids".to_string())?,
        )
        .map_err(|e| format!("decode admitted webhook ids: {e}"))?
    };
    crate::scheduler_fire::fire_due_checked(&app)?;
    Ok(ids)
}

/// Nudge sentinels: repeating-pattern schedule suggestions (H14 nudge cards).
#[tauri::command]
pub fn scheduler_nudges(state: State<'_, AppState>) -> Result<Value, String> {
    let handle = svc(&state)?;
    let svc = handle.lock().map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(svc.nudges()).unwrap_or(Value::Null))
}

/// Record a goal observation (feeds the nudge sentinels — from chat/session).
#[tauri::command]
pub fn scheduler_nudge(
    state: State<'_, AppState>,
    goal: String,
    ts: Option<u64>,
) -> Result<bool, String> {
    let handle = svc(&state)?;
    let mut svc = handle.lock().map_err(|e| e.to_string())?;
    svc.record_nudge(&goal, ts.unwrap_or_else(now_secs));
    Ok(true)
}

/// P51.32a — a job's durable notepad (the only continuity the trigger plane
/// keeps; run results live in the Event Log, `I3`).
#[tauri::command]
pub fn scheduler_notepad_get(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    let handle = svc(&state)?;
    let svc = handle.lock().map_err(|e| e.to_string())?;
    Ok(svc
        .notepad(&id)
        .map(|n| serde_json::json!({ "notepad": n }))
        .unwrap_or(Value::Null))
}

/// P51.32a — append one line to a job's durable notepad.
#[tauri::command]
pub fn scheduler_notepad_append(
    state: State<'_, AppState>,
    id: String,
    line: String,
) -> Result<bool, String> {
    let handle = svc(&state)?;
    let mut svc = handle.lock().map_err(|e| e.to_string())?;
    Ok(svc.append_notepad(&id, &line))
}

/// P51.32e — open (unacked-first) incidents ledger.
#[tauri::command]
pub fn scheduler_incidents(state: State<'_, AppState>) -> Result<Value, String> {
    let handle = svc(&state)?;
    let svc = handle.lock().map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(svc.list_incidents()).unwrap_or(Value::Null))
}

/// P51.32e — acknowledge one incident (explicit only, no auto-clear).
#[tauri::command]
pub fn scheduler_incident_ack(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let handle = svc(&state)?;
    let mut svc = handle.lock().map_err(|e| e.to_string())?;
    Ok(svc.ack_incident(&id))
}

/// P51.32f — read-only trigger-plane health (missed schedule fires, registry
/// depth). Run-level health belongs to the Work kernel / Event Log.
///
/// P71.3f — the doctor also reads **agent readiness** (`ARCH/AUTOMATION.md` §9:
/// a run-level failure is readiness + binding policy, never a global "the
/// model"): a firing whose engine is not ready is the first thing a support
/// pass needs to see, so it is a check like any other, with the states counted
/// rather than guessed.
#[tauri::command]
pub fn scheduler_doctor(state: State<'_, AppState>) -> Result<Value, String> {
    let handle = svc(&state)?;
    let svc = handle.lock().map_err(|e| e.to_string())?;
    svc.ensure_public_health()?;
    let mut checks = svc.cron_doctor(now_secs());
    drop(svc);
    checks.push(crate::acp_cmds::agents_doctor_check(Arc::clone(
        &state.acp_sessions,
    )));
    Ok(serde_json::to_value(checks).unwrap_or(Value::Null))
}

// ---- P71.9e — the §11 run surface (runs · duplicate · export) --------------

/// The bound agent for a session, resolved exactly as `scheduler_fire` does
/// (the sidecar's session pin → the config's primary agent). A retired
/// built-in spelling resolves to nothing (`ADR-0005`) — the UI shows the
/// truth ("no agent bound"), never a substitute engine.
fn bound_agent_for(state: &AppState, session_id: &str) -> Option<String> {
    let resolved = {
        let relay = state.chat_relay.lock().ok()?;
        let relay = relay.as_ref()?;
        relay
            .link()
            .request(
                "chief/resolve_session",
                serde_json::json!({ "sessionId": session_id }),
            )
            .ok()
    };
    if let Some(id) = resolved
        .as_ref()
        .and_then(|out| out.get("chiefId"))
        .and_then(Value::as_str)
    {
        let id = id.trim();
        // NOTE (DEC-053 Step 4): `everyaios` stays excluded here on purpose —
        // the retired agent id, recognized so already-retired rows are never
        // resurrected.
        if !id.is_empty() && id != "inbuilt" && id != "everyaios" && id != "agentcowork-native" {
            return Some(id.to_string());
        }
    }
    let cfg = agentcowork_core::Config::load().ok()?;
    let pinned = cfg.primary_chief.trim().to_string();
    // NOTE (DEC-053 Step 4): same retired-id exclusion as above.
    if pinned.is_empty()
        || pinned == "inbuilt"
        || pinned == "everyaios"
        || pinned == "agentcowork-native"
    {
        None
    } else {
        Some(pinned)
    }
}

/// Recent runs for one automation (or all, when `job_id` is empty): the
/// ExecutionLedger rows whose trigger is `scheduler` and whose context
/// snapshot carries this job's `automationId` — the record the firing path
/// already writes (`scheduler_fire.rs`), read back as honest status. A run's
/// `waitingApproval` phase is the §11 "waiting for approval" state; there is
/// no fabricated success/failure beyond the ledger's own phases.
#[tauri::command]
pub fn scheduler_runs(state: State<'_, AppState>, job_id: String) -> Result<Value, String> {
    let relay = state.chat_relay.lock().map_err(|e| e.to_string())?;
    let relay = relay
        .as_ref()
        .ok_or_else(|| "sidecar not connected — run history not ready".to_string())?;
    let kernel = relay.executions();
    let (automation_id, automation_generation) = if job_id.is_empty() {
        (None, None)
    } else {
        let scheduler = relay.scheduler();
        let scheduler = scheduler.lock().map_err(|e| e.to_string())?;
        let job = scheduler
            .get(&job_id)
            .ok_or_else(|| format!("unknown automation {job_id}"))?;
        (Some(job.automation_id.clone()), Some(job.generation))
    };
    let k = kernel.lock().map_err(|e| e.to_string())?;
    let mut runs: Vec<Value> = k
        .all()
        .filter(|ex| {
            if ex.trigger != agentcowork_core::execution::ExecutionTrigger::Scheduler {
                return false;
            }
            if let Some(automation_id) = automation_id.as_deref() {
                let context = match serde_json::from_str::<Value>(&ex.context_snapshot) {
                    Ok(context) => context,
                    Err(_) => return false,
                };
                if context.get("automationId").and_then(Value::as_str) != Some(automation_id) {
                    return false;
                }
                if let Some(generation) = automation_generation {
                    if context.get("automationGeneration").and_then(Value::as_u64)
                        != Some(generation)
                    {
                        return false;
                    }
                }
            }
            true
        })
        .map(|ex| {
            let context: Value = serde_json::from_str(&ex.context_snapshot).unwrap_or(Value::Null);
            serde_json::json!({
                "id": ex.id,
                "sessionId": ex.session_id,
                "objective": ex.objective,
                "phase": ex.state,
                "waitingApproval": matches!(ex.state, agentcowork_core::execution::ExecutionPhase::WaitingApproval),
                "createdAtMs": ex.created_at_ms,
                "context": ex.context_snapshot,
                "automationId": context.get("automationId").cloned().unwrap_or(Value::Null),
                "revisionId": context.get("revisionId").cloned().unwrap_or(Value::Null),
                "automationGeneration": context.get("automationGeneration").cloned().unwrap_or(Value::Null),
                "triggerOccurrenceId": context.get("triggerOccurrenceId").cloned().unwrap_or(Value::Null),
                "workId": context.get("workId").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    // Newest first (the §11 "at a glance" ordering).
    runs.sort_by(|a, b| {
        let ka = a.get("createdAtMs").and_then(Value::as_u64).unwrap_or(0);
        let kb = b.get("createdAtMs").and_then(Value::as_u64).unwrap_or(0);
        kb.cmp(&ka)
    });
    runs.truncate(50);
    Ok(serde_json::json!({ "runs": runs, "count": runs.len() }))
}

/// Duplicate an automation: a new id, the same definition, **disabled** (the
/// §11 duplicate affordance must never silently arm a second trigger).
#[tauri::command]
pub fn scheduler_duplicate(state: State<'_, AppState>, id: String) -> Result<String, String> {
    let handle = svc(&state)?;
    let new_id = {
        let mut svc = handle.lock().map_err(|e| e.to_string())?;
        let src = svc
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("unknown automation {id}"))?;
        let base_id = format!("{id}-copy-{}", now_secs());
        let mut new_id = base_id.clone();
        let mut suffix = 1usize;
        while svc.get(&new_id).is_some() {
            new_id = format!("{base_id}-{suffix}");
            suffix = suffix.saturating_add(1);
        }
        svc.upsert_checked_with_enabled(
            new_id.clone(),
            format!("{} (copy)", src.name),
            src.session_id,
            src.trigger,
            src.steps,
            Some(src.policy),
            Some(false),
            now_secs(),
        )?;
        new_id
    };
    Ok(new_id)
}

/// Export an automation as its `*.automation.json` definition — name, trigger,
/// steps, policy and the **session binding only**. Secrets never ride this
/// file (`AUTOMATION.md` §11): the vault holds credentials, and only a
/// `credential_ref`-shaped empty placeholder could ever appear here (steps
/// carry capability *requests*, never granted credentials).
#[tauri::command]
pub fn scheduler_export(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    let handle = svc(&state)?;
    let svc = handle.lock().map_err(|e| e.to_string())?;
    let jobs = svc.list();
    let src = jobs
        .iter()
        .find(|j| j.id == id)
        .ok_or_else(|| format!("unknown automation {id}"))?;
    let bound = bound_agent_for(&state, &src.session_id);
    Ok(serde_json::json!({
        // NOTE (DEC-053 Step 4): `"everyaios.automation"` is a wire value the
        // sidecar parses — its spelling stays so existing automations keep
        // matching.
        "kind": "everyaios.automation",
        "version": 1,
        "automation": {
            "id": src.id,
            "automationId": src.automation_id,
            "revisionId": src.revision_id,
            "automationGeneration": src.generation,
            "name": src.name,
            "sessionId": src.session_id,
            "boundAgent": bound,
            "trigger": serde_json::to_value(&src.trigger).unwrap_or(Value::Null),
            "steps": serde_json::to_value(&src.steps).unwrap_or(Value::Null),
            "policy": serde_json::to_value(&src.policy).unwrap_or(Value::Null),
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentcowork_core::scheduler_service::{EventKind, TriggerSpec};
    use serde_json::json;

    fn add_job(service: &mut SchedulerService, id: &str, trigger: TriggerSpec) {
        service.upsert(id, id, "source", trigger, Vec::new(), None, 1);
    }

    fn occurrence_id(response: &Value) -> String {
        response["occurrence"]["triggerOccurrenceId"]
            .as_str()
            .expect("manual response carries an occurrence id")
            .to_string()
    }

    fn first_occurrence_id(response: &Value) -> String {
        response["occurrences"][0]["triggerOccurrenceId"]
            .as_str()
            .expect("ingress response carries an occurrence id")
            .to_string()
    }

    #[test]
    fn ingress_params_preserve_the_caller_key_and_reject_missing_keys() {
        let manual = manual_ingress_params("job", "manual-request-1", 42).unwrap();
        assert_eq!(manual["idempotencyKey"], json!("manual-request-1"));
        assert_eq!(manual["now"], json!(42));

        let event =
            event_ingress_params("repo_change", json!({ "repo": "r" }), "event-request-1", 43)
                .unwrap();
        assert_eq!(event["idempotencyKey"], json!("event-request-1"));

        let webhook = webhook_ingress_params(
            "/hooks/ci",
            json!({ "ref": "main" }),
            None,
            "webhook-request-1",
            44,
        )
        .unwrap();
        assert_eq!(webhook["idempotencyKey"], json!("webhook-request-1"));
        assert!(manual_ingress_params("job", "  ", 45).is_err());
        assert!(event_ingress_params("repo_change", json!({}), "", 45).is_err());
        assert!(webhook_ingress_params("/hooks/ci", json!({}), None, "", 45).is_err());
    }

    #[test]
    fn manual_retry_with_the_same_key_reuses_one_occurrence() {
        let mut service = SchedulerService::new();
        add_job(&mut service, "manual", TriggerSpec::Manual);

        let first = service
            .handle(
                "scheduler/run_now",
                &manual_ingress_params("manual", "manual-request-1", 10).unwrap(),
            )
            .unwrap();
        let replay = service
            .handle(
                "scheduler/run_now",
                &manual_ingress_params("manual", "manual-request-1", 11).unwrap(),
            )
            .unwrap();

        assert_eq!(occurrence_id(&first), occurrence_id(&replay));
        assert_eq!(service.occurrences().len(), 1);
    }

    #[test]
    fn event_retry_conflict_and_distinct_keys_follow_scheduler_semantics() {
        let mut service = SchedulerService::new();
        add_job(
            &mut service,
            "event",
            TriggerSpec::Event {
                kind: EventKind::RepoChange,
                filter: String::new(),
            },
        );
        let payload = json!({ "repo": "r", "sha": "one" });

        let first = service
            .handle(
                "scheduler/fire_event",
                &event_ingress_params("repo_change", payload.clone(), "event-1", 20).unwrap(),
            )
            .unwrap();
        let replay = service
            .handle(
                "scheduler/fire_event",
                &event_ingress_params("repo_change", payload.clone(), "event-1", 21).unwrap(),
            )
            .unwrap();
        assert_eq!(first_occurrence_id(&first), first_occurrence_id(&replay));

        assert!(service
            .handle(
                "scheduler/fire_event",
                &event_ingress_params(
                    "repo_change",
                    json!({ "repo": "r", "sha": "two" }),
                    "event-1",
                    22,
                )
                .unwrap(),
            )
            .is_err());

        let distinct = service
            .handle(
                "scheduler/fire_event",
                &event_ingress_params("repo_change", payload, "event-2", 23).unwrap(),
            )
            .unwrap();
        assert_ne!(first_occurrence_id(&first), first_occurrence_id(&distinct));
        assert_eq!(service.occurrences().len(), 2);
    }

    #[test]
    fn webhook_retry_conflict_and_distinct_keys_follow_scheduler_semantics() {
        let mut service = SchedulerService::new();
        add_job(
            &mut service,
            "webhook",
            TriggerSpec::Webhook {
                path: "/hooks/ci".into(),
                schema: vec!["ref".into(), "sha".into()],
            },
        );
        let body = json!({ "ref": "main", "sha": "one" });

        let first = service
            .handle(
                "scheduler/fire_webhook",
                &webhook_ingress_params("/hooks/ci", body.clone(), None, "webhook-1", 30).unwrap(),
            )
            .unwrap();
        let replay = service
            .handle(
                "scheduler/fire_webhook",
                &webhook_ingress_params("/hooks/ci", body.clone(), None, "webhook-1", 31).unwrap(),
            )
            .unwrap();
        assert_eq!(first_occurrence_id(&first), first_occurrence_id(&replay));

        assert!(service
            .handle(
                "scheduler/fire_webhook",
                &webhook_ingress_params(
                    "/hooks/ci",
                    json!({ "ref": "main", "sha": "two" }),
                    None,
                    "webhook-1",
                    32,
                )
                .unwrap(),
            )
            .is_err());

        let distinct = service
            .handle(
                "scheduler/fire_webhook",
                &webhook_ingress_params("/hooks/ci", body, None, "webhook-2", 33).unwrap(),
            )
            .unwrap();
        assert_ne!(first_occurrence_id(&first), first_occurrence_id(&distinct));
        assert_eq!(service.occurrences().len(), 2);
    }
}
