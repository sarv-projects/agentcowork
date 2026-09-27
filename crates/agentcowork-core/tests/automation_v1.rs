//! Hermetic automation-v1 admission, provenance, and recovery tests.

use std::path::PathBuf;

use agentcowork_blueprint::AutomationStep;
use agentcowork_core::automation_runtime::{compile_work, content_addressed_revision_id};
use agentcowork_core::execution::{ExecutionKernel, ExecutionTrigger};
use agentcowork_core::scheduler_service::{
    AutomationOccurrence, EventKind, OccurrenceState, SchedulerService, TriggerSpec,
    WorkRunAdmissionReceipt,
};
use agentcowork_core::work_gateway::WorkGateway;
use agentcowork_types::{SessionKind, WorkState};
use serde_json::json;

fn temp_path(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "agentcowork-automation-v1-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path.join("scheduler.json")
}

fn job_steps() -> Vec<AutomationStep> {
    vec![AutomationStep::OnlineSearch {
        query: "release notes".into(),
    }]
}

fn add_job(service: &mut SchedulerService, id: &str, trigger: TriggerSpec) {
    service.upsert(id, id, "source", trigger, job_steps(), None, 10);
}

fn receipt(
    service: &SchedulerService,
    occurrence: &AutomationOccurrence,
) -> WorkRunAdmissionReceipt {
    service
        .receipt_for_occurrence(&occurrence.trigger_occurrence_id)
        .unwrap()
}

#[test]
fn occurrence_identity_revision_and_work_factory_are_bound() {
    let path = temp_path("identity");
    let mut service = SchedulerService::load_or_new(path.clone());
    add_job(&mut service, "brief", TriggerSpec::Manual);
    let revision = service.get("brief").unwrap().revision_id.clone();
    let occurrence = service
        .admit_manual("brief", &json!({ "requestId": "request-1" }), 11)
        .unwrap();
    assert!(occurrence.automation_id.starts_with("automation:auto:"));
    assert_eq!(occurrence.revision_id, revision);
    assert!(occurrence.trigger_occurrence_id.starts_with("occ:"));
    assert_eq!(occurrence.payload_digest.len(), 64);
    assert_eq!(occurrence.dedup_digest.len(), 64);
    assert_eq!(occurrence.state, OccurrenceState::Pending);

    let spec = compile_work(
        &occurrence.revision.automation(),
        &occurrence.revision_id,
        &occurrence.trigger_occurrence_id,
    )
    .unwrap();
    assert_eq!(spec.provenance.automation_id, occurrence.automation_id);
    assert_eq!(spec.provenance.revision_id, occurrence.revision_id);
    assert_eq!(
        spec.provenance.automation_generation,
        occurrence.revision.generation()
    );
    assert_eq!(
        spec.provenance.trigger_occurrence_id,
        occurrence.trigger_occurrence_id
    );
    assert_eq!(spec.capability_requests[0].capability_id, "net.search");

    let replay = service
        .admit_manual("brief", &json!({ "requestId": "request-1" }), 11)
        .unwrap();
    assert_eq!(
        replay.trigger_occurrence_id,
        occurrence.trigger_occurrence_id
    );
    assert_eq!(service.occurrences().len(), 1);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn uncertain_occurrence_is_not_retried_until_explicit_reconciliation() {
    let mut service = SchedulerService::new();
    add_job(
        &mut service,
        "scheduled",
        TriggerSpec::Interval { secs: 60 },
    );
    let first = service.admit_due(71).unwrap().pop().unwrap();
    service
        .mark_occurrence_uncertain(&first.trigger_occurrence_id, "gateway unavailable")
        .unwrap();

    assert_eq!(
        service.occurrence_state(&first.trigger_occurrence_id),
        Some(OccurrenceState::Uncertain)
    );
    assert!(service.pending_occurrences().is_empty());
    let replay = service.admit_due(71).unwrap();
    assert!(
        replay.is_empty(),
        "uncertain occurrences must not enter the host queue"
    );
    assert_eq!(service.occurrences().len(), 1);

    let admission = receipt(&service, &first);
    service
        .mark_occurrence_fired_with_receipt(&first.trigger_occurrence_id, 71, &admission)
        .unwrap();
    assert_eq!(
        service.occurrence_state(&first.trigger_occurrence_id),
        Some(OccurrenceState::Terminal)
    );
}

#[test]
fn cancellation_is_monotonic_and_never_reopens_the_occurrence() {
    let mut service = SchedulerService::new();
    add_job(&mut service, "manual", TriggerSpec::Manual);
    let first = service
        .admit_manual("manual", &json!({ "requestId": "cancel-me" }), 11)
        .unwrap();
    service
        .cancel_occurrence(&first.trigger_occurrence_id, "user cancelled")
        .unwrap();
    assert_eq!(
        service.occurrence_state(&first.trigger_occurrence_id),
        Some(OccurrenceState::Cancelled)
    );
    assert!(service.pending_occurrences().is_empty());

    let replay = service
        .admit_manual("manual", &json!({ "requestId": "cancel-me" }), 12)
        .unwrap();
    assert_eq!(replay.trigger_occurrence_id, first.trigger_occurrence_id);
    assert!(
        service
            .mark_occurrence_fired_with_receipt(
                &first.trigger_occurrence_id,
                12,
                &receipt(&service, &first),
            )
            .is_err()
    );

    let later = service
        .admit_manual("manual", &json!({ "requestId": "after-cancel" }), 13)
        .unwrap();
    assert_ne!(later.trigger_occurrence_id, first.trigger_occurrence_id);
}

#[test]
fn receipt_and_provenance_are_required_for_advancement() {
    let mut service = SchedulerService::new();
    add_job(&mut service, "manual", TriggerSpec::Manual);
    let occurrence = service
        .admit_manual("manual", &json!({ "requestId": "receipt-1" }), 11)
        .unwrap();
    let mut wrong = receipt(&service, &occurrence);
    wrong.durable = false;
    assert!(
        service
            .mark_occurrence_fired_with_receipt(&occurrence.trigger_occurrence_id, 11, &wrong)
            .is_err()
    );

    let mut wrong_provenance = receipt(&service, &occurrence);
    wrong_provenance.revision_id = content_addressed_revision_id(
        &occurrence.revision.automation(),
        occurrence.revision.revision + 1,
    );
    assert!(
        service
            .mark_occurrence_fired_with_receipt(
                &occurrence.trigger_occurrence_id,
                11,
                &wrong_provenance,
            )
            .is_err()
    );

    let mut wrong_ids = receipt(&service, &occurrence);
    wrong_ids.work_id = "work-forged".into();
    assert!(
        service
            .mark_occurrence_fired_with_receipt(&occurrence.trigger_occurrence_id, 11, &wrong_ids)
            .is_err()
    );
    assert_eq!(
        service.occurrence_state(&occurrence.trigger_occurrence_id),
        Some(OccurrenceState::Pending)
    );

    let admission = receipt(&service, &occurrence);
    service
        .mark_occurrence_fired_with_receipt(&occurrence.trigger_occurrence_id, 11, &admission)
        .unwrap();
    assert_eq!(
        service.occurrence_state(&occurrence.trigger_occurrence_id),
        Some(OccurrenceState::Terminal)
    );
}

#[test]
fn delete_recreate_advances_generation_and_cannot_reuse_occurrence_identity() {
    let path = temp_path("generation");
    let mut service = SchedulerService::load_or_new(path.clone());
    add_job(&mut service, "brief", TriggerSpec::Manual);
    let old_generation = service.get("brief").unwrap().generation;
    let old_revision = service.get("brief").unwrap().revision_id.clone();
    let old = service
        .admit_manual("brief", &json!({ "requestId": "same-key" }), 11)
        .unwrap();
    let admission = receipt(&service, &old);
    service
        .mark_occurrence_fired_with_receipt(&old.trigger_occurrence_id, 11, &admission)
        .unwrap();
    service.delete_checked("brief").unwrap();

    add_job(&mut service, "brief", TriggerSpec::Manual);
    let new_job = service.get("brief").unwrap();
    assert!(new_job.generation > old_generation);
    assert_ne!(new_job.revision_id, old_revision);
    let new = service
        .admit_manual("brief", &json!({ "requestId": "same-key" }), 12)
        .unwrap();
    assert_ne!(new.trigger_occurrence_id, old.trigger_occurrence_id);
    assert_ne!(new.revision_id, old.revision_id);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn delivery_keys_distinguish_identical_payloads_and_replay_is_idempotent() {
    let mut service = SchedulerService::new();
    add_job(
        &mut service,
        "event",
        TriggerSpec::Event {
            kind: EventKind::RepoChange,
            filter: String::new(),
        },
    );
    let payload = json!({ "path": "same" });
    let first = service
        .admit_event_with_key(EventKind::RepoChange, &payload, "delivery-1", 21)
        .unwrap()
        .pop()
        .unwrap();
    let second = service
        .admit_event_with_key(EventKind::RepoChange, &payload, "delivery-2", 21)
        .unwrap()
        .pop()
        .unwrap();
    assert_ne!(first.trigger_occurrence_id, second.trigger_occurrence_id);
    assert_eq!(first.payload_digest, second.payload_digest);

    let replay = service
        .admit_event_with_key(EventKind::RepoChange, &payload, "delivery-1", 22)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(replay.trigger_occurrence_id, first.trigger_occurrence_id);
    assert!(
        service
            .admit_event_with_key(
                EventKind::RepoChange,
                &json!({ "path": "changed" }),
                "delivery-1",
                22,
            )
            .is_err()
    );
    assert_eq!(service.occurrences().len(), 2);
}

#[test]
fn revision_digest_is_checked_by_compile_work() {
    let mut service = SchedulerService::new();
    add_job(&mut service, "brief", TriggerSpec::Manual);
    let occurrence = service
        .admit_manual("brief", &json!({ "requestId": "digest-1" }), 11)
        .unwrap();
    let mut changed = occurrence.revision.automation();
    changed.name.push_str(" edited");
    assert!(
        compile_work(
            &changed,
            &occurrence.revision_id,
            &occurrence.trigger_occurrence_id
        )
        .is_err()
    );
    assert!(
        compile_work(
            &occurrence.revision.automation(),
            &occurrence.revision_id,
            &occurrence.trigger_occurrence_id,
        )
        .is_ok()
    );
    let mut policy_changed = occurrence.revision.clone();
    policy_changed.policy.max_runs_per_hour = Some(99);
    assert!(
        compile_work(
            &policy_changed.automation(),
            &occurrence.revision_id,
            &occurrence.trigger_occurrence_id,
        )
        .is_err()
    );
}

#[test]
fn persistence_failure_rolls_back_definition_and_admission() {
    let path = temp_path("persist-failure");
    let mut service = SchedulerService::load_or_new(path.clone());
    add_job(&mut service, "brief", TriggerSpec::Manual);
    let before = service.get("brief").unwrap().clone();

    let parent = path.parent().unwrap();
    std::fs::remove_dir_all(parent).unwrap();
    std::fs::write(parent, b"not a directory").unwrap();
    let definition_error = service.upsert_checked(
        "brief",
        "changed",
        "source",
        TriggerSpec::Manual,
        job_steps(),
        None,
        11,
    );
    assert!(definition_error.is_err());
    assert_eq!(service.get("brief").unwrap().name, before.name);
    assert!(service.occurrences().is_empty());
    let _ = std::fs::remove_file(parent);
}

#[test]
fn overdue_cron_admits_one_run_once_on_resume() {
    let mut service = SchedulerService::new();
    service.upsert(
        "cron",
        "cron",
        "source",
        TriggerSpec::Cron {
            expr: "0 9 * * *".into(),
        },
        job_steps(),
        None,
        1_750_000_000,
    );
    let scheduled_at = service.get("cron").unwrap().next_run_at.unwrap();
    let resumed_at = scheduled_at + 17 * 60;
    let admitted = service.admit_due(resumed_at).unwrap();
    assert_eq!(admitted.len(), 1);
    assert_eq!(
        admitted[0].trigger,
        agentcowork_core::scheduler_service::OccurrenceTrigger::Schedule { scheduled_at }
    );
    assert_eq!(service.admit_due(resumed_at).unwrap().len(), 1);
    let occurrence = admitted[0].clone();
    let admission = receipt(&service, &occurrence);
    service
        .mark_occurrence_fired_with_receipt(
            &occurrence.trigger_occurrence_id,
            resumed_at,
            &admission,
        )
        .unwrap();
    assert!(service.due(resumed_at).is_empty());
    assert!(service.get("cron").unwrap().next_run_at.unwrap() > resumed_at);
}

#[test]
fn checked_definition_write_applies_enabled_and_rolls_back_on_persist_failure() {
    let path = temp_path("enabled");
    let mut service = SchedulerService::load_or_new(path.clone());
    service
        .handle(
            "scheduler/upsert",
            &json!({
                "id": "disabled",
                "name": "Disabled",
                "sessionId": "source",
                "trigger": { "type": "interval", "secs": 60 },
                "steps": [],
                "enabled": false,
                "now": 10,
            }),
        )
        .unwrap();
    assert!(!service.get("disabled").unwrap().enabled);
    assert!(service.due(1_000).is_empty());

    let parent = path.parent().unwrap();
    std::fs::remove_dir_all(parent).unwrap();
    std::fs::write(parent, b"blocked").unwrap();
    assert!(
        service
            .upsert_checked(
                "disabled",
                "Changed",
                "source",
                TriggerSpec::Interval { secs: 60 },
                job_steps(),
                None,
                11,
            )
            .is_err()
    );
    assert_eq!(service.get("disabled").unwrap().name, "Disabled");
    let _ = std::fs::remove_file(parent);
}

#[test]
fn manual_rpc_without_a_request_key_fails_closed() {
    let mut service = SchedulerService::new();
    add_job(&mut service, "manual", TriggerSpec::Manual);
    assert!(
        service
            .handle("scheduler/run_now", &json!({ "id": "manual", "now": 11 }))
            .is_err()
    );
    assert!(service.occurrences().is_empty());
}

#[test]
fn work_state_projection_does_not_invent_completion() {
    let mut gateway = WorkGateway::new();
    gateway
        .create_work_in_session(
            "work-state",
            None,
            Some("automation-session".into()),
            SessionKind::Automation,
            "compiled objective",
        )
        .unwrap();
    let mut kernel = ExecutionKernel::new();
    kernel.begin_named(
        "run-state".to_string(),
        ExecutionTrigger::Scheduler,
        "automation-session",
        "compiled objective",
        None,
        String::new(),
        r#"{"triggerOccurrenceId":"occ-state"}"#.into(),
        vec![],
    );
    gateway.bind_execution("work-state", "run-state").unwrap();
    gateway
        .record_execution_transition("work-state", "run-state", WorkState::Ready)
        .unwrap();
    assert_eq!(
        gateway.presence("work-state").unwrap().work_state,
        Some(WorkState::Ready)
    );
    for state in [
        WorkState::WaitingApproval,
        WorkState::Recoverable,
        WorkState::Cancelled,
    ] {
        gateway
            .record_execution_transition("work-state", "run-state", state)
            .unwrap();
        assert_eq!(
            gateway.presence("work-state").unwrap().work_state,
            Some(state)
        );
        assert_ne!(state, WorkState::Completed);
    }
}
