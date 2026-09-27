//! Host-owned automation admission and Work dispatch.
//!
//! A trigger never owns execution. The host admits one durable occurrence,
//! compiles the immutable automation revision through the Work factory, and
//! creates ordinary Work/Run records through the existing gateway/kernel
//! owners. Agent execution, waits, retries, effects, and completion remain
//! outside this module.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use agentcowork_core::automation_runtime::{compile_work, WorkSpec};
use agentcowork_core::execution::{ExecutionPhase, ExecutionTrigger};
use agentcowork_core::scheduler_service::{
    AutomationOccurrence, SchedulerService, WorkRunAdmissionReceipt,
};
use agentcowork_core::work_gateway::{DomainEvent, WorkEvent};
use agentcowork_types::{SessionKind, WorkState};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

use crate::AppState;

/// Stop-condition marker retained for monitor compatibility. The scheduler no
/// longer executes a monitor turn itself; a bound Work executor owns that
/// interpretation when it is attached.
#[allow(dead_code)]
pub const MONITOR_STOP_MARKER: &str = "[MONITOR_DONE]";

/// Seconds between due checks (the cadence used by the host loop).
pub const TICK_SECS: u64 = 5;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn service(state: &AppState) -> Result<Arc<Mutex<SchedulerService>>, String> {
    crate::scheduler_cmds::scheduler_handle(state)
}

/// The deterministic owner identity for one automation. It is deliberately
/// not the Work id: a Work can be retried/recovered independently of the
/// Session that owns it.
fn automation_session_id(automation_id: &str, generation: u64) -> String {
    if generation <= 1 {
        format!("automation-session:{automation_id}")
    } else {
        format!("automation-session:{automation_id}:g{generation}")
    }
}

fn automation_work_id(automation_id: &str, occurrence_id: &str) -> String {
    format!("automation-work:{automation_id}:{occurrence_id}")
}

fn automation_run_id(automation_id: &str, occurrence_id: &str) -> String {
    format!("automation-run:{automation_id}:{occurrence_id}")
}

fn job_id_for_occurrence(
    service: &SchedulerService,
    occurrence: &AutomationOccurrence,
) -> Option<String> {
    service
        .list()
        .into_iter()
        .find(|job| job.automation_id == occurrence.automation_id)
        .map(|job| job.id.clone())
}

/// A headless Session record is a real owner, not a Chat fabricated per run.
/// The vault is the existing Session persistence seam; the WorkGateway still
/// receives the explicit owner and kind on Work creation.
fn ensure_automation_session(
    state: &State<'_, AppState>,
    session_id: &str,
    automation_id: &str,
    revision_id: &str,
    automation_generation: u64,
) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|error| error.to_string())?;
    if let Some(raw) = vault
        .get_ui_session(session_id)
        .map_err(|error| format!("automation Session lookup failed: {error}"))?
    {
        let value: Value = serde_json::from_str(&raw)
            .map_err(|error| format!("automation Session `{session_id}` is corrupt: {error}"))?;
        if value.get("id").and_then(Value::as_str) != Some(session_id)
            || value.get("kind").and_then(Value::as_str) != Some("automation")
            || value.get("automationId").and_then(Value::as_str) != Some(automation_id)
            || value
                .get("automationGeneration")
                .and_then(Value::as_u64)
                .is_some_and(|generation| generation != automation_generation)
        {
            return Err(format!(
                "automation Session `{session_id}` is not the expected headless owner"
            ));
        }
        return Ok(());
    }
    let payload = serde_json::to_string(&json!({
        "id": session_id,
        "kind": "automation",
        "automationId": automation_id,
        "revisionId": revision_id,
        "automationGeneration": automation_generation,
        "headless": true,
    }))
    .map_err(|error| format!("encode automation Session: {error}"))?;
    vault
        .put_ui_session(session_id, &payload)
        .map_err(|error| format!("persist automation Session: {error}"))
}

/// Compile an occurrence with the production factory. Keeping this as a small
/// named boundary makes it straightforward to test that no trigger path can
/// silently bypass `compile_work`.
fn compile_occurrence(occurrence: &AutomationOccurrence) -> Result<WorkSpec, String> {
    let automation = occurrence.revision.automation();
    let spec = compile_work(
        &automation,
        &occurrence.revision_id,
        &occurrence.trigger_occurrence_id,
    )
    .map_err(|error| format!("automation compile refused: {error}"))?;
    if spec.provenance.automation_id != occurrence.automation_id
        || spec.provenance.revision_id != occurrence.revision_id
        || spec.provenance.automation_generation != occurrence.revision.generation()
        || spec.provenance.trigger_occurrence_id != occurrence.trigger_occurrence_id
    {
        return Err("compiled Work provenance does not match its occurrence".into());
    }
    Ok(spec)
}

fn provenance_patch(occurrence: &AutomationOccurrence, spec: &WorkSpec) -> Value {
    json!({
        "automationId": occurrence.automation_id,
        "revisionId": occurrence.revision_id,
        "automationGeneration": spec.provenance.automation_generation,
        "triggerOccurrenceId": occurrence.trigger_occurrence_id,
        "payloadDigest": occurrence.payload_digest,
        "dedupDigest": occurrence.dedup_digest,
        "compiledWork": spec,
        "capabilityRequests": spec.capability_requests,
    })
}

fn has_provenance_event(
    gateway: &agentcowork_core::WorkGateway,
    work_id: &str,
    occurrence: &AutomationOccurrence,
    spec: &WorkSpec,
) -> bool {
    let expected_spec = serde_json::to_value(spec).ok();
    gateway.events(work_id).iter().any(|envelope| {
        matches!(
            &envelope.event,
            WorkEvent::Domain(DomainEvent::WorkUpdated { patch })
                if patch.get("automationId").and_then(Value::as_str)
                    == Some(occurrence.automation_id.as_str())
                    && patch.get("revisionId").and_then(Value::as_str)
                        == Some(occurrence.revision_id.as_str())
                    && patch.get("automationGeneration").and_then(Value::as_u64)
                        == Some(occurrence.revision.generation())
                    && patch.get("triggerOccurrenceId").and_then(Value::as_str)
                        == Some(occurrence.trigger_occurrence_id.as_str())
                    && patch.get("payloadDigest").and_then(Value::as_str)
                        == Some(occurrence.payload_digest.as_str())
                    && patch.get("dedupDigest").and_then(Value::as_str)
                        == Some(occurrence.dedup_digest.as_str())
                    && expected_spec
                        .as_ref()
                        .is_some_and(|expected| patch.get("compiledWork") == Some(expected))
        )
    })
}

fn bound_agent(state: &State<'_, AppState>, session_id: &str) -> Option<String> {
    let resolved = {
        let relay = state.chat_relay.lock().ok()?;
        let relay = relay.as_ref()?;
        relay
            .link()
            .request("chief/resolve_session", json!({ "sessionId": session_id }))
            .ok()
    };
    resolved
        .as_ref()
        .and_then(|value| value.get("chiefId"))
        .and_then(Value::as_str)
        .map(str::trim)
        // NOTE (DEC-053 Step 4): `everyaios` stays filtered here on purpose —
        // it is the retired agent id; rows already carrying it are recognized
        // and refused, never resurrected.
        .filter(|id| {
            !id.is_empty() && *id != "inbuilt" && *id != "everyaios" && *id != "agentcowork-native"
        })
        .map(str::to_string)
        .or_else(|| {
            let config = agentcowork_core::Config::load().ok()?;
            let id = config.primary_chief.trim();
            (!id.is_empty() && id != "inbuilt" && id != "everyaios" && id != "agentcowork-native")
                .then(|| id.to_string())
        })
}

struct AdmittedWork {
    admission_receipt: WorkRunAdmissionReceipt,
    blocked: Option<String>,
}

/// Admit one occurrence into the canonical Work/Run spine. No provider I/O is
/// performed here. A required-but-unready agent leaves Work/Run in `Ready` and
/// reports a blocker; it is never reported as a completed execution.
fn dispatch_occurrence(
    state: &State<'_, AppState>,
    occurrence: &AutomationOccurrence,
) -> Result<AdmittedWork, String> {
    let spec = compile_occurrence(occurrence)?;
    let session_id =
        automation_session_id(&occurrence.automation_id, occurrence.revision.generation());
    let work_id = automation_work_id(&occurrence.automation_id, &occurrence.trigger_occurrence_id);
    let run_id = automation_run_id(&occurrence.automation_id, &occurrence.trigger_occurrence_id);

    // The Session must exist before Work. A failed vault write is a failed
    // admission, not a reason to create an ownerless Work.
    ensure_automation_session(
        state,
        &session_id,
        &occurrence.automation_id,
        &occurrence.revision_id,
        occurrence.revision.generation(),
    )?;

    let (relay, kernel) = {
        let relay_guard = state.chat_relay.lock().map_err(|error| error.to_string())?;
        let relay = relay_guard
            .as_ref()
            .ok_or_else(|| "sidecar not connected — Work gateway unavailable".to_string())?;
        (relay.work_gateway(), relay.executions())
    };

    let mut gateway = relay.lock().map_err(|error| error.to_string())?;
    if let Some(existing) = gateway.get_work(&work_id) {
        let provenance_replayed = has_provenance_event(&gateway, &work_id, occurrence, &spec);
        let kind_matches = existing.session_kind == SessionKind::Automation
            || (existing.session_kind == SessionKind::Interactive && provenance_replayed);
        if existing.session_id.as_deref() != Some(session_id.as_str()) || !kind_matches {
            return Err(format!("Work `{work_id}` exists with a different owner"));
        }
        let objective_matches = gateway.events(&work_id).iter().any(|envelope| {
            matches!(
                &envelope.event,
                WorkEvent::Domain(DomainEvent::WorkCreated { objective, .. })
                    if objective == &spec.objective
            )
        });
        if !objective_matches {
            return Err(format!("Work `{work_id}` has a different objective"));
        }
    } else {
        gateway.create_work_in_session(
            work_id.clone(),
            None,
            Some(session_id.clone()),
            SessionKind::Automation,
            spec.objective.clone(),
        )?;
    }

    // Provenance is a durable Work event, not an in-memory side channel. The
    // deterministic occurrence makes this append idempotent across retries.
    if !has_provenance_event(&gateway, &work_id, occurrence, &spec) {
        gateway.append(
            &work_id,
            WorkEvent::Domain(DomainEvent::WorkUpdated {
                patch: provenance_patch(occurrence, &spec),
            }),
            None,
        )?;
    }

    let context = serde_json::to_string(&json!({
        "automationId": occurrence.automation_id,
        "revisionId": occurrence.revision_id,
        "automationGeneration": spec.provenance.automation_generation,
        "triggerOccurrenceId": occurrence.trigger_occurrence_id,
        "payloadDigest": occurrence.payload_digest,
        "dedupDigest": occurrence.dedup_digest,
        "sourceSessionId": occurrence.revision.session_id,
        "workId": work_id,
        "runId": run_id,
        "agentRequired": spec.agent_required,
        "compiledWork": spec,
    }))
    .map_err(|error| format!("encode Work context: {error}"))?;
    let capability_scope: Vec<String> = spec
        .capability_requests
        .iter()
        .map(|request| request.capability_id.clone())
        .collect();

    let run_phase = {
        let mut executions = kernel.lock().map_err(|error| error.to_string())?;
        match executions.get(&run_id) {
            Some(existing) => {
                if existing.session_id != session_id {
                    return Err(format!("Run `{run_id}` belongs to another Session"));
                }
                let existing_context: Value = serde_json::from_str(&existing.context_snapshot)
                    .map_err(|error| {
                        format!("Run `{run_id}` has corrupt provenance context: {error}")
                    })?;
                if existing_context.get("automationId").and_then(Value::as_str)
                    != Some(occurrence.automation_id.as_str())
                    || existing_context.get("revisionId").and_then(Value::as_str)
                        != Some(occurrence.revision_id.as_str())
                    || existing_context
                        .get("automationGeneration")
                        .and_then(Value::as_u64)
                        != Some(occurrence.revision.generation())
                    || existing_context
                        .get("triggerOccurrenceId")
                        .and_then(Value::as_str)
                        != Some(occurrence.trigger_occurrence_id.as_str())
                {
                    return Err(format!(
                        "Run `{run_id}` has a different occurrence provenance"
                    ));
                }
                Some(existing.state)
            }
            None => {
                executions.begin_named(
                    run_id.clone(),
                    ExecutionTrigger::Scheduler,
                    &session_id,
                    &spec.objective,
                    None,
                    serde_json::to_string(&occurrence.revision.policy)
                        .map_err(|error| format!("encode Work policy: {error}"))?,
                    context,
                    capability_scope,
                );
                Some(ExecutionPhase::Ready)
            }
        }
    };

    if gateway.execution_id(&work_id).is_some()
        && gateway.execution_id(&work_id) != Some(run_id.as_str())
    {
        return Err(format!("Work `{work_id}` is already bound to another Run"));
    }
    if gateway.execution_id(&work_id).is_none() {
        gateway.bind_execution(&work_id, &run_id)?;
    }
    let work_state = work_state_for_phase(run_phase);
    if gateway
        .presence(&work_id)
        .and_then(|presence| presence.work_state)
        != Some(work_state)
    {
        gateway.record_execution_transition(&work_id, &run_id, work_state)?;
    }

    let blocked = if spec.agent_required {
        match bound_agent(state, &occurrence.revision.session_id) {
            Some(agent_id) => {
                let readiness = crate::acp_cmds::agent_readiness(&agent_id);
                (!readiness.is_ready()).then(|| {
                    format!(
                        "bound agent `{agent_id}` is not ready: {}",
                        readiness.summary()
                    )
                })
            }
            None => Some(
                "no external agent is bound to this automation's source Session; Work remains pending"
                    .to_string(),
            ),
        }
    } else {
        None
    };

    let admission_receipt = WorkRunAdmissionReceipt::new(
        &work_id,
        &run_id,
        &occurrence.automation_id,
        &occurrence.revision_id,
        &occurrence.trigger_occurrence_id,
        spec.provenance.automation_generation,
    );

    Ok(AdmittedWork {
        admission_receipt,
        blocked,
    })
}

fn work_state_for_phase(phase: Option<ExecutionPhase>) -> WorkState {
    match phase {
        Some(ExecutionPhase::WaitingTool) => WorkState::WaitingTool,
        Some(ExecutionPhase::WaitingApproval) => WorkState::WaitingApproval,
        Some(ExecutionPhase::WaitingUser) => WorkState::WaitingUser,
        Some(ExecutionPhase::Checkpointed) => WorkState::Checkpointed,
        Some(ExecutionPhase::Verifying) => WorkState::Verifying,
        Some(ExecutionPhase::Completed) => WorkState::Completed,
        Some(ExecutionPhase::Failed) => WorkState::Failed,
        Some(ExecutionPhase::Cancelled) => WorkState::Cancelled,
        Some(ExecutionPhase::Paused) => WorkState::Paused,
        Some(ExecutionPhase::Recoverable) => WorkState::Recoverable,
        Some(ExecutionPhase::Running) => WorkState::Running,
        Some(ExecutionPhase::Created | ExecutionPhase::Planning | ExecutionPhase::Ready) | None => {
            WorkState::Ready
        }
    }
}

fn mark_occurrence_uncertain(state: &State<'_, AppState>, occurrence_id: &str, reason: &str) {
    if let Ok(service) = service(state) {
        if let Ok(mut service) = service.lock() {
            let _ = service.mark_occurrence_uncertain(occurrence_id, reason);
        }
    }
}

/// Fire all pending occurrences plus all currently due schedules. Every path
/// converges here; event/webhook/manual ingress only admits metadata and never
/// creates Work itself.
pub fn fire_due_checked(app: &AppHandle) -> Result<Vec<String>, String> {
    let state = app.state::<AppState>();
    let service_handle = service(&state)?;
    let now = now_secs();
    let occurrences = {
        let mut service = service_handle
            .lock()
            .map_err(|error| format!("scheduler lock unavailable: {error}"))?;
        service.ensure_public_health()?;
        let mut pending = service.pending_occurrences();
        service.admit_due(now)?;
        pending.extend(service.pending_occurrences());
        let mut unique = HashMap::new();
        for occurrence in pending {
            unique
                .entry(occurrence.trigger_occurrence_id.clone())
                .or_insert(occurrence);
        }
        unique.into_values().collect::<Vec<_>>()
    };

    let mut fired = Vec::new();
    let mut first_error: Option<String> = None;
    for occurrence in occurrences {
        // Admission and dispatch are deliberately separate. Re-check the
        // durable state after releasing the scheduler lock so a cancellation or
        // uncertainty decision wins before any Work is created.
        let still_pending = match service_handle.lock() {
            Ok(service) => service
                .occurrence(&occurrence.trigger_occurrence_id)
                .is_some_and(|current| current.is_pending()),
            Err(error) => {
                first_error.get_or_insert_with(|| {
                    format!("scheduler occurrence state unavailable: {error}")
                });
                continue;
            }
        };
        if !still_pending {
            continue;
        }
        match dispatch_occurrence(&state, &occurrence) {
            Ok(admitted) => {
                let result = service_handle.lock().ok().map(|mut service| {
                    service.mark_occurrence_fired_with_receipt(
                        &occurrence.trigger_occurrence_id,
                        now,
                        &admitted.admission_receipt,
                    )
                });
                match result {
                    Some(Ok(())) => {
                        if let Some(job_id) = service_handle
                            .lock()
                            .ok()
                            .and_then(|service| job_id_for_occurrence(&service, &occurrence))
                        {
                            fired.push(job_id);
                        }
                    }
                    Some(Err(error)) => {
                        first_error.get_or_insert_with(|| error.clone());
                        mark_occurrence_uncertain(
                            &state,
                            &occurrence.trigger_occurrence_id,
                            &error,
                        );
                    }
                    None => {
                        let error = "scheduler occurrence advancement was unavailable";
                        first_error.get_or_insert_with(|| error.to_string());
                        mark_occurrence_uncertain(&state, &occurrence.trigger_occurrence_id, error);
                    }
                }
                if let Some(reason) = admitted.blocked {
                    // The Work/Run remain durable and non-terminal. This is an
                    // explicit pending state, never a fabricated completion.
                    eprintln!(
                        "automation occurrence {} pending: {reason}",
                        occurrence.id()
                    );
                }
            }
            Err(reason) => {
                first_error.get_or_insert_with(|| reason.clone());
                mark_occurrence_uncertain(&state, &occurrence.trigger_occurrence_id, &reason);
            }
        }
    }
    if let Some(error) = first_error {
        Err(error)
    } else {
        Ok(fired)
    }
}

/// Fire the host dispatcher without making the background loop panic on a
/// transiently unavailable owner.
pub fn fire_due(app: &AppHandle) -> Vec<String> {
    match fire_due_checked(app) {
        Ok(ids) => ids,
        Err(error) => {
            eprintln!("automation firing paused: {error}");
            Vec::new()
        }
    }
}

/// Start the host trigger loop. The sidecar only admits webhook requests; it
/// never owns this dispatch loop.
pub fn spawn_loop(app: &AppHandle) {
    let app = app.clone();
    std::thread::Builder::new()
        .name("agentcowork-automation-firing".to_string())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(TICK_SECS));
            let _ = fire_due(&app);
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentcowork_blueprint::AutomationStep;
    use agentcowork_core::automation_runtime::content_addressed_revision_id;
    use agentcowork_core::scheduler_service::{
        AutomationRevision, OccurrenceState, OccurrenceTrigger,
    };

    fn occurrence() -> AutomationOccurrence {
        let mut revision = AutomationRevision {
            automation_id: "automation:auto:test".into(),
            revision: 1,
            revision_id: "1:revision".into(),
            name: "test".into(),
            session_id: "source".into(),
            trigger: agentcowork_core::scheduler_service::TriggerSpec::Manual,
            steps: vec![AutomationStep::RunCode {
                language: "js".into(),
                code: "return 1".into(),
            }],
            policy: Default::default(),
        };
        revision.revision_id =
            content_addressed_revision_id(&revision.automation(), revision.revision);
        let payload_digest = "b".repeat(64);
        let dedup_digest = "a".repeat(64);
        AutomationOccurrence {
            trigger_occurrence_id: format!("occ:{}", &dedup_digest[..32]),
            automation_id: revision.automation_id.clone(),
            revision_id: revision.revision_id.clone(),
            trigger: OccurrenceTrigger::Manual,
            payload_digest,
            dedup_digest,
            admitted_at: 1,
            state: OccurrenceState::Pending,
            idempotency_key: "test-delivery".into(),
            fired_at: None,
            work_id: None,
            run_id: None,
            admission_error: None,
            admission_receipt: None,
            revision,
        }
    }

    #[test]
    fn production_compile_preserves_occurrence_provenance() {
        let occurrence = occurrence();
        let spec = compile_occurrence(&occurrence).unwrap();
        assert_eq!(spec.provenance.automation_id, occurrence.automation_id);
        assert_eq!(spec.provenance.revision_id, occurrence.revision_id);
        assert_eq!(spec.provenance.automation_generation, 1);
        assert_eq!(
            spec.provenance.trigger_occurrence_id,
            occurrence.trigger_occurrence_id
        );
        assert_eq!(spec.capability_requests.len(), 1);
    }

    #[test]
    fn deterministic_work_does_not_require_an_agent() {
        let spec = compile_occurrence(&occurrence()).unwrap();
        assert_eq!(MONITOR_STOP_MARKER, "[MONITOR_DONE]");
        assert!(!spec.agent_required);
        assert!(spec.steps.iter().all(|step| step.deterministic));
    }
}
