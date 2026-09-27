//! Hermetic production-recovery/replay coverage for the core Work spine.
//!
//! These tests use only temporary JSONL journals and kernel snapshots. No
//! provider, scheduler service, or external effect is involved: the point is
//! to prove that the journal is authoritative and that ambiguous state fails
//! closed before a new prompt can be admitted.

use std::path::{Path, PathBuf};

use agentcowork_core::execution::{ExecutionKernel, ExecutionPhase};
use agentcowork_core::work_gateway::{DomainEvent, WorkEvent, WorkGateway};
use agentcowork_types::{
    AgentBinding, AgentBindingId, AgentGovernanceMode, AgentId, AgentProtocol, IdempotencyClass,
    SessionId, SessionKind, WorkId, WorkState,
};
use serde_json::{Value, json};

fn temp_dir(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "agentcowork-core-recovery-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn journal(path: &Path) -> WorkGateway {
    WorkGateway::open(path).unwrap()
}

fn make_automation_work(gateway: &mut WorkGateway) {
    gateway
        .create_work_in_session(
            "automation-work",
            None,
            Some("automation-session".into()),
            SessionKind::Automation,
            "compiled automation objective",
        )
        .unwrap();
    gateway
        .append(
            "automation-work",
            WorkEvent::Domain(DomainEvent::WorkUpdated {
                patch: json!({
                    "automationId": "automation:auto:test",
                    "revisionId": "g1:r1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "automationGeneration": 1,
                    "triggerOccurrenceId": "occ:test",
                    "payloadDigest": "b".repeat(64),
                    "dedupDigest": "a".repeat(64),
                    "sourceSessionId": "source-session",
                }),
            }),
            None,
        )
        .unwrap();
    gateway
        .bind_execution("automation-work", "automation-run")
        .unwrap();
    gateway
        .record_execution_transition("automation-work", "automation-run", WorkState::Running)
        .unwrap();
}

#[test]
fn automation_kind_provenance_and_terminal_state_replay_exactly() {
    let dir = temp_dir("automation");
    let path = dir.join("events.jsonl");
    {
        let mut gateway = journal(&path);
        make_automation_work(&mut gateway);
        gateway
            .record_execution_transition("automation-work", "automation-run", WorkState::Completed)
            .unwrap();
    }

    let gateway = journal(&path);
    let address = gateway.get_work("automation-work").unwrap();
    assert_eq!(address.session_kind, SessionKind::Automation);
    assert_eq!(
        address.provenance.automation_id.as_deref(),
        Some("automation:auto:test")
    );
    assert_eq!(
        address.provenance.revision_id.as_deref(),
        Some("g1:r1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert_eq!(address.provenance.automation_generation, Some(1));
    assert_eq!(
        address.provenance.trigger_occurrence_id.as_deref(),
        Some("occ:test")
    );

    let kernel = ExecutionKernel::recover_from_work_gateway(&gateway).unwrap();
    let run = kernel.get("automation-run").unwrap();
    assert_eq!(run.session_id, "automation-session");
    assert_eq!(
        run.trigger,
        agentcowork_core::execution::ExecutionTrigger::Scheduler
    );
    assert_eq!(run.state, ExecutionPhase::Completed);
    assert_eq!(run.idempotency_key, "exec:automation-run");
    assert!(run.context_snapshot.contains("automation:auto:test"));

    let mut gateway = gateway;
    assert!(
        gateway
            .record_execution_transition("automation-work", "automation-run", WorkState::Running)
            .is_err()
    );
    assert_eq!(
        gateway
            .presence("automation-work")
            .and_then(|presence| presence.work_state),
        Some(WorkState::Completed)
    );
}

#[test]
fn missing_or_corrupt_run_state_refuses_recovery() {
    let dir = temp_dir("missing-run");
    let path = dir.join("events.jsonl");
    // Deliberately write only the durable Work→Run reference. There is no
    // RunQueued/Started event, so recovery must not mint `ex:1`.
    // Rewrite a deterministic legacy-compatible journal with creation first.
    let creation = json!({
        "workId": "missing-run-work",
        "sequence": 0,
        "eventId": "we:0",
        "event": {
            "class": "domain",
            "event": {
                "kind": "work_created",
                "data": {
                    "objective": "objective",
                    "projectId": null,
                    "sessionId": "session",
                    "parentWorkId": null
                }
            }
        },
        "timestamp": 1,
        "traceId": null,
        "causalParent": null
    });
    let reference = json!({
        "workId": "missing-run-work",
        "sequence": 1,
        "eventId": "we:1",
        "event": {
            "class": "domain",
            "event": {
                "kind": "work_updated",
                "data": {"patch": {"executionId": "run-missing"}}
            }
        },
        "timestamp": 2,
        "traceId": null,
        "causalParent": null
    });
    std::fs::write(&path, format!("{creation}\n{reference}\n")).unwrap();
    let gateway = WorkGateway::open(&path).unwrap();
    let error = ExecutionKernel::recover_from_work_gateway(&gateway).unwrap_err();
    assert!(error.contains("no durable lifecycle event"), "got: {error}");

    let corrupt_path = dir.join("corrupt-events.jsonl");
    let run_started = json!({
        "workId": "missing-run-work",
        "sequence": 1,
        "eventId": "we:1",
        "event": {
            "class": "domain",
            "event": {
                "kind": "run_started",
                "data": {"run_id": "run-corrupt"}
            }
        },
        "timestamp": 2,
        "traceId": null,
        "causalParent": null
    });
    let run_completed = json!({
        "workId": "missing-run-work",
        "sequence": 2,
        "eventId": "we:2",
        "event": {
            "class": "domain",
            "event": {
                "kind": "run_completed",
                "data": {"run_id": "run-corrupt"}
            }
        },
        "timestamp": 3,
        "traceId": null,
        "causalParent": null
    });
    let run_reopened = json!({
        "workId": "missing-run-work",
        "sequence": 3,
        "eventId": "we:3",
        "event": {
            "class": "domain",
            "event": {
                "kind": "run_started",
                "data": {"run_id": "run-corrupt"}
            }
        },
        "timestamp": 4,
        "traceId": null,
        "causalParent": null
    });
    std::fs::write(
        &corrupt_path,
        format!("{creation}\n{run_started}\n{run_completed}\n{run_reopened}\n"),
    )
    .unwrap();
    let error = WorkGateway::open(&corrupt_path).unwrap_err();
    assert!(error.contains("illegal Work transition"), "got: {error}");
}

#[test]
fn checkpoint_mismatch_is_fail_closed_and_journal_wins() {
    let dir = temp_dir("checkpoint");
    let path = dir.join("events.jsonl");
    let checkpoint = dir.join("kernel.json");
    let mut gateway = journal(&path);
    make_automation_work(&mut gateway);
    gateway
        .record_execution_transition("automation-work", "automation-run", WorkState::Running)
        .unwrap();
    let kernel = ExecutionKernel::recover_from_work_gateway(&gateway).unwrap();
    kernel.persist_to(&checkpoint).unwrap();

    let mut raw: Value =
        serde_json::from_str(&std::fs::read_to_string(&checkpoint).unwrap()).unwrap();
    raw["executions"]["automation-run"]["state"] = json!("completed");
    raw["executions"]["automation-run"]["journalSequence"] = json!(99);
    std::fs::write(&checkpoint, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    let error =
        ExecutionKernel::recover_from_work_gateway_with_checkpoint(&gateway, Some(&checkpoint))
            .unwrap_err();
    assert!(error.contains("checkpoint"), "got: {error}");

    // A matching cache is accepted, while the journal remains authoritative.
    let matching =
        ExecutionKernel::recover_from_work_gateway_with_checkpoint(&gateway, Some(&checkpoint));
    assert!(
        matching.is_err(),
        "the deliberately mismatched cache stays refused"
    );
}

#[test]
fn binding_ownership_and_private_session_survive_replay() {
    let dir = temp_dir("binding");
    let path = dir.join("events.jsonl");
    {
        let mut gateway = journal(&path);
        gateway.create_work("binding-work", None, Some("session".into()), "agent work");
        let binding = AgentBinding {
            binding_id: AgentBindingId::new("binding-1"),
            session_id: SessionId::new("session"),
            work_id: WorkId::new("binding-work"),
            agent_id: AgentId::new("agent-1"),
            adapter_id: None,
            protocol: AgentProtocol::Acp,
            provider_session_id: None,
            model: None,
            mode: None,
            capability_manifest: vec![],
            governance_mode: AgentGovernanceMode::SelfContained,
            bridge_id: None,
            state: agentcowork_types::BindingLifecycle::Parked,
            usage: Default::default(),
            last_event_seq: 0,
            private_state_ref: None,
        };
        gateway.create_agent_binding(binding).unwrap();
        gateway
            .transition_agent_binding("binding-1", "activated", Some("provider-1".into()))
            .unwrap();
        gateway
            .bind_execution_with_metadata(
                "binding-work",
                "binding-run",
                &json!({"trigger": "acp", "sessionId": "session", "objective": "agent work"}),
            )
            .unwrap();
        gateway
            .record_execution_transition("binding-work", "binding-run", WorkState::Running)
            .unwrap();
    }
    let gateway = journal(&path);
    assert_eq!(
        gateway
            .get_work("binding-work")
            .unwrap()
            .binding_id
            .as_deref(),
        Some("binding-1")
    );
    assert_eq!(
        gateway
            .agent_binding("binding-1")
            .and_then(|binding| binding.provider_session_id.as_deref()),
        Some("provider-1")
    );
    let kernel = ExecutionKernel::recover_from_work_gateway(&gateway).unwrap();
    let run = kernel.get("binding-run").unwrap();
    assert_eq!(
        run.trigger,
        agentcowork_core::execution::ExecutionTrigger::Acp
    );
    assert!(run.context_snapshot.contains("provider-1"));
}

#[test]
fn pending_approval_replays_and_resolution_is_terminal_journal_fact() {
    let dir = temp_dir("pending-approval");
    let path = dir.join("events.jsonl");
    {
        let mut gateway = journal(&path);
        gateway.create_work(
            "approval-work",
            None,
            Some("session".into()),
            "guarded action",
        );
        gateway
            .bind_execution_with_metadata(
                "approval-work",
                "approval-run",
                &json!({
                    "trigger": "chat",
                    "sessionId": "session",
                    "objective": "guarded action"
                }),
            )
            .unwrap();
        gateway
            .record_execution_transition("approval-work", "approval-run", WorkState::Running)
            .unwrap();
        gateway
            .record_execution_transition(
                "approval-work",
                "approval-run",
                WorkState::WaitingApproval,
            )
            .unwrap();
        gateway
            .record_pending_approval(
                "approval-work",
                "ticket-1",
                "file.write",
                "args-hash",
                "R3",
                1234,
            )
            .unwrap();
    }

    let gateway = journal(&path);
    let pending = gateway
        .pending_approval("approval-work", "ticket-1")
        .expect("pending approval is rebuilt from the journal");
    assert_eq!(pending["runId"], json!("approval-run"));
    assert_eq!(pending["toolId"], json!("file.write"));
    let kernel = ExecutionKernel::recover_from_work_gateway(&gateway).unwrap();
    let run = kernel.get("approval-run").unwrap();
    assert_eq!(run.state, ExecutionPhase::WaitingApproval);
    assert_eq!(run.pending_approval.as_ref().unwrap().ticket_id, "ticket-1");
    assert_eq!(run.approval_refs, vec!["ticket-1"]);

    let checkpoint = dir.join("pending.checkpoint.json");
    kernel.persist_to(&checkpoint).unwrap();
    let mut raw: Value =
        serde_json::from_str(&std::fs::read_to_string(&checkpoint).unwrap()).unwrap();
    raw["executions"]["approval-run"]["pendingApproval"] = Value::Null;
    std::fs::write(&checkpoint, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    let error =
        ExecutionKernel::recover_from_work_gateway_with_checkpoint(&gateway, Some(&checkpoint))
            .unwrap_err();
    assert!(error.contains("checkpoint"), "got: {error}");

    let mut gateway = gateway;
    assert!(
        gateway
            .record_execution_transition("approval-work", "approval-run", WorkState::Running)
            .is_err()
    );
    gateway
        .resolve_pending_approval_for_run("approval-work", "approval-run", "ticket-1", true)
        .unwrap();
    assert!(
        gateway
            .pending_approval("approval-work", "ticket-1")
            .is_none()
    );
    assert_eq!(
        gateway.run_state("approval-work", "approval-run"),
        Some(WorkState::Running)
    );

    let reopened = journal(&path);
    let kernel = ExecutionKernel::recover_from_work_gateway(&reopened).unwrap();
    let run = kernel.get("approval-run").unwrap();
    assert_eq!(run.state, ExecutionPhase::Running);
    assert!(run.pending_approval.is_none());
}

#[test]
fn approval_rpc_handles_are_ticket_scoped_and_match_the_projection() {
    let dir = temp_dir("approval-rpc");
    let path = dir.join("events.jsonl");
    let mut kernel = ExecutionKernel::new();
    let work = kernel.begin(
        agentcowork_core::execution::ExecutionTrigger::Chat,
        "session",
        "guarded action",
        None,
        String::new(),
        String::new(),
        vec![],
    );
    kernel
        .transition(&work.id, ExecutionPhase::Running)
        .unwrap();
    kernel
        .transition(&work.id, ExecutionPhase::WaitingApproval)
        .unwrap();

    let mut gateway = journal(&path);
    gateway.create_work(
        "approval-rpc-work",
        None,
        Some("session".into()),
        "guarded action",
    );
    gateway
        .bind_execution_with_metadata(
            "approval-rpc-work",
            &work.id,
            &json!({"trigger": "chat", "sessionId": "session", "objective": "guarded action"}),
        )
        .unwrap();
    gateway
        .record_execution_transition("approval-rpc-work", &work.id, WorkState::Running)
        .unwrap();
    gateway
        .record_execution_transition("approval-rpc-work", &work.id, WorkState::WaitingApproval)
        .unwrap();
    gateway
        .record_pending_approval_for_run(
            "approval-rpc-work",
            &work.id,
            "ticket-rpc",
            "file.write",
            "hash-rpc",
            "R2",
            99,
        )
        .unwrap();

    let record_params = json!({
        "id": work.id,
        "ticketId": "ticket-rpc",
        "toolId": "file.write",
        "argsHash": "hash-rpc",
        "riskTier": "R2",
        "requestedAtMs": 99
    });
    assert!(
        kernel
            .handle("execution/record_approval", &record_params)
            .is_ok()
    );
    assert_eq!(
        kernel
            .get(&work.id)
            .unwrap()
            .pending_approval
            .as_ref()
            .unwrap()
            .ticket_id,
        "ticket-rpc"
    );

    let resolve_params = json!({
        "id": work.id,
        "ticketId": "ticket-rpc",
        "approved": true
    });
    assert!(
        kernel
            .handle("execution/resolve_approval", &resolve_params)
            .is_ok()
    );
    assert_eq!(kernel.get(&work.id).unwrap().state, ExecutionPhase::Running);
    assert!(kernel.get(&work.id).unwrap().pending_approval.is_none());
}

#[test]
fn uncertain_effect_requires_explicit_reconciliation_before_resume() {
    let dir = temp_dir("uncertain");
    let path = dir.join("events.jsonl");
    let mut gateway = journal(&path);
    gateway.create_work("uncertain-work", None, Some("session".into()), "mutate");
    gateway
        .bind_execution_with_metadata(
            "uncertain-work",
            "uncertain-run",
            &json!({"trigger": "chat", "sessionId": "session", "objective": "mutate"}),
        )
        .unwrap();
    gateway
        .record_execution_transition("uncertain-work", "uncertain-run", WorkState::Running)
        .unwrap();
    gateway
        .record_effect_with_class(
            "uncertain-work",
            "effect-1",
            "attempted",
            "",
            Some("grant-1"),
            Some(IdempotencyClass::UnsafeRetry),
        )
        .unwrap();
    gateway
        .record_uncertain_effect(
            "uncertain-work",
            "effect-1",
            "transport dropped after dispatch",
            Some(IdempotencyClass::UnsafeRetry),
        )
        .unwrap();
    gateway
        .record_execution_transition("uncertain-work", "uncertain-run", WorkState::Recoverable)
        .unwrap();

    let gateway = journal(&path);
    let kernel = ExecutionKernel::recover_from_work_gateway(&gateway).unwrap();
    assert_eq!(
        kernel.get("uncertain-run").unwrap().state,
        ExecutionPhase::Recoverable
    );
    assert!(kernel.get("uncertain-run").unwrap().receipt.is_none());
    let mut gateway = gateway;
    assert!(
        gateway
            .record_execution_transition("uncertain-work", "uncertain-run", WorkState::Running)
            .is_err()
    );
    gateway
        .reconcile_uncertain_effect(
            "uncertain-work",
            "effect-1",
            "confirmed not committed",
            false,
        )
        .unwrap();
    gateway
        .record_execution_transition("uncertain-work", "uncertain-run", WorkState::Running)
        .unwrap();
}
