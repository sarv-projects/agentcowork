//! P50.5.5 (remainder) + P50.5.6 — failure + crash/restart recovery gates.
//!
//! The binary-level legs (SIGKILL sidecar, locked vault, no provider, corrupt
//! persistence, guard suites) run in `scripts/e2e/failure-injection.mjs`
//! against the real core binary. This file covers the legs that belong at the
//! crate boundary, where construing them through the binary would hide the
//! assertion:
//!
//! - expired tickets are refused at consume time (approval cannot resurrect);
//! - corrupt scheduler JSON fails closed while task JSON recovery remains
//!   explicitly non-authoritative;
//! - a wrong vault key fails closed (no plaintext, no silent recreate);
//! - scheduler leases held by a dead worker reconcile to Idle on reload;
//! - a task ledger killed mid-run reopens Running (never terminal — recovery
//!   classifies uncertainty, never fabricates completion);
//! - an execution kernel killed mid-run recovers Running with its counter;
//! - the audit session log reopens with exact seq/order continuity;
//! - (env-gated) a real search round-trip lands as a tool receipt in the
//!   audit chain, proving search → receipt → audit composition.
//!
//! Crash discipline everywhere: `drop` the owner WITHOUT finishing, reload
//! from disk, and assert the recovered state is honest.

use std::time::Duration;

use agentcowork_audit::session_log::{EventInput, EventType, SessionLog};
use agentcowork_core::execution::{ExecutionKernel, ExecutionPhase, ExecutionTrigger};
use agentcowork_core::scheduler_service::{SchedulerService, TriggerSpec};
use agentcowork_core::task_ledger::{FileStore, TaskKind, TaskLedger, TaskStatus};
use agentcowork_guard::RiskLevel;
use agentcowork_guard::ticket::{
    ApprovalSource, AuthorizationTicket, TicketError, TicketState, TicketStore,
};
use agentcowork_vault::Vault;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "agentcowork-p50-gates-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn expired_ticket() -> AuthorizationTicket {
    AuthorizationTicket {
        ticket_id: "t-expired".into(),
        agent_id: "a1".into(),
        session_id: "s1".into(),
        tool_id: "fs.write".into(),
        operation: "write".into(),
        args_hash: "abc123".into(),
        paths: vec!["report.txt".into()],
        // Epoch: unambiguously in the past on every runner clock.
        expires_at_ms: 1,
        single_use: true,
        approval_source: ApprovalSource::Policy,
        approval_nonce: "n".into(),
        risk: RiskLevel::Medium,
        audit_seq: 0,
        state: TicketState::Pending,
        bindings: Vec::new(),
        execution_id: String::new(),
        action_id: String::new(),
        idempotency_key: String::new(),
    }
}

// ---------------------------------------------------------------------------
// P50.5.5 — expire a ticket: approval cannot resurrect an expired ticket.
// ---------------------------------------------------------------------------

#[test]
fn expired_ticket_is_refused_at_consume() {
    let mut store = TicketStore::new();
    let id = store.mint(expired_ticket());
    // Even a human approval lands (the card was answered)…
    assert!(store.approve(&id), "approve must accept the pending ticket");
    // …but consuming an expired ticket fails with Expired, never executes.
    let err = store
        .use_ticket(&id, "abc123")
        .expect_err("P50.5.5: an expired ticket was consumed");
    assert!(
        matches!(err, TicketError::Expired),
        "P50.5.5: expected Expired, got {err:?}"
    );
    // And the ticket is marked expired for the audit trail.
    assert!(
        store
            .pending()
            .iter()
            .all(|t| t.ticket_id != id || matches!(t.state, TicketState::Expired)),
        "P50.5.5: expired ticket must leave Pending"
    );
    eprintln!("P50.5.5: expired ticket refused at consume (approval did not resurrect)");
}

// ---------------------------------------------------------------------------
// P50.5.5 — corrupt persistence recovers fresh (never crash-loops).
// ---------------------------------------------------------------------------

#[test]
fn corrupt_scheduler_json_fails_closed() {
    let dir = temp_dir("sched-corrupt");
    let path = dir.join("scheduler.json");
    std::fs::write(&path, "not-json{{").unwrap();
    // A corrupt registry is not an empty healthy registry. The compatibility
    // constructor remains infallible for the relay, but the service carries a
    // named load error and every host-facing door refuses it.
    let mut svc = SchedulerService::load_or_new(path.clone());
    assert!(!svc.is_healthy());
    assert!(svc.registry_error().is_some());
    assert!(
        svc.handle("scheduler/list", &serde_json::json!({}))
            .is_err()
    );
    assert!(
        svc.upsert_checked(
            "j1",
            "probe",
            "s1",
            TriggerSpec::Interval { secs: 60 },
            Vec::new(),
            None,
            0,
        )
        .is_err()
    );
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("P50.5.5: corrupt scheduler.json failed closed");
}

#[test]
fn corrupt_tasks_json_recovers_empty() {
    let dir = temp_dir("tasks-corrupt");
    let path = dir.join("tasks.json");
    std::fs::write(&path, "[broken").unwrap();
    // FileStore::load is lenient (missing/corrupt ⇒ empty vec, never Err).
    let mut ledger = TaskLedger::new(Box::new(FileStore::new(path.clone())));
    let id = ledger.enqueue(TaskKind::Automation, "after corruption", None::<String>);
    ledger
        .persist()
        .expect("P50.5.5: persist after corrupt recovery");
    let reloaded = TaskLedger::new(Box::new(FileStore::new(path)));
    let rec = reloaded
        .get(&id)
        .expect("P50.5.5: task enqueued after corrupt recovery must survive");
    assert_eq!(rec.status, TaskStatus::Queued);
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("P50.5.5: corrupt tasks.json recovered empty and writable");
}

#[test]
fn wrong_vault_key_fails_closed() {
    let dir = temp_dir("vault-locked");
    let path = dir.join("vault.db");
    {
        let _vault = Vault::open(&path, "correct-key").expect("create vault");
    }
    // A wrong key must fail — never open plaintext, never silently recreate
    // a vault the user may hold key material in. (`Vault` is not `Debug`, so
    // `expect_err` cannot be used here — match instead.)
    let err = match Vault::open(&path, "wrong-key") {
        Ok(_) => panic!("P50.5.5: wrong key opened the vault"),
        Err(e) => e,
    };
    assert!(
        !format!("{err:?}").is_empty(),
        "P50.5.5: wrong-key failure must carry a reason"
    );
    eprintln!("P50.5.5: wrong vault key fails closed: {err:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vault_open_in_unwritable_location_fails_honestly() {
    // P50.2.1 — persistence failure at the vault boundary: when the data dir
    // cannot even be created (a file blocks the path), open must Err with a
    // reason — never a half-initialized vault, never a silent in-memory swap
    // the caller did not ask for.
    let dir = temp_dir("vault-unwritable");
    let blocker = dir.join("blocked");
    std::fs::write(&blocker, "a file, not a dir").unwrap();
    let err = match Vault::open(&blocker.join("vault.db"), "k") {
        Ok(_) => panic!("P50.2.1: vault open through a file must fail"),
        Err(e) => e,
    };
    assert!(
        !format!("{err:?}").is_empty(),
        "P50.2.1: vault open failure must carry a reason"
    );
    eprintln!("P50.2.1: unwritable vault location fails honestly: {err:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// P50.5.6 — crash/restart recovery at each effect boundary.
// ---------------------------------------------------------------------------

#[test]
fn scheduler_trigger_registry_survives_crash() {
    let dir = temp_dir("sched-lease");
    let path = dir.join("scheduler.json");
    let job_id = {
        let mut svc = SchedulerService::load_or_new(path.clone());
        svc.upsert(
            "nightly",
            "digest",
            "s1",
            TriggerSpec::Interval { secs: 3600 },
            Vec::new(),
            None,
            0,
        );
        // A durable occurrence is admitted, then Work/Run admission is recorded
        // before the process dies (drop = SIGKILL). Trigger-only advancement is
        // intentionally not a valid persistence boundary.
        let occurrence = svc
            .admit_due(3_601)
            .expect("due occurrence admitted")
            .pop()
            .expect("one due occurrence");
        let receipt = svc
            .receipt_for_occurrence(&occurrence.trigger_occurrence_id)
            .expect("durable Work/Run receipt");
        svc.mark_occurrence_fired_with_receipt(&occurrence.trigger_occurrence_id, 3_601, &receipt)
            .expect("firing recorded");
        svc.persist().expect("persist fired state");
        "nightly".to_string()
    };
    // Reload: the trigger registry (definitions + occurrences) survives; the
    // slipped schedule fires once on resume (`run_once_on_resume`), then
    // `mark_fired` advances it — no replay of missed occurrences.
    let reloaded = SchedulerService::load_or_new(path);
    let job = reloaded.get(&job_id).expect("job survives the crash");
    assert_eq!(
        job.last_fired_at,
        Some(3_601),
        "P50.5.6: the occurrence record survives the crash"
    );
    assert!(
        !job.paused,
        "no stuck execution state to reconcile: a trigger plane holds no lease"
    );
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("P50.5.6: crashed scheduler trigger registry survived, occurrence recorded");
}

#[test]
fn ledger_crash_midrun_recovers_running_never_terminal() {
    let dir = temp_dir("ledger-crash");
    let path = dir.join("tasks.json");
    let id = {
        let mut ledger = TaskLedger::new(Box::new(FileStore::new(path.clone())));
        let id = ledger.enqueue(TaskKind::Subagent, "crashed work", None::<String>);
        ledger.start(&id).expect("start");
        ledger.persist().expect("persist running state");
        id // drop = the process dies mid-run, before complete().
    };
    let mut ledger = TaskLedger::new(Box::new(FileStore::new(path)));
    let rec_status = ledger
        .get(&id)
        .map(|r| r.status.clone())
        .expect("P50.5.6: crashed task must survive reload");
    // Uncertain, never completed: recovery classifies Running, not a
    // fabricated Succeeded. The follow-up (sweep/retry/complete) decides.
    assert_eq!(
        rec_status,
        TaskStatus::Running,
        "P50.5.6: crashed task must reopen Running, got {rec_status:?}"
    );
    // And the honest follow-up still works: complete lands terminal.
    ledger
        .complete(&id, true, None)
        .expect("honest completion after recovery");
    assert_eq!(ledger.get(&id).unwrap().status, TaskStatus::Succeeded);
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("P50.5.6: crashed ledger task reopened Running, completed honestly after");
}

#[test]
fn kernel_crash_midrun_recovers_running_with_counter() {
    let dir = temp_dir("kernel-crash");
    let path = dir.join("checkpoint.json");
    let exec_id = {
        let mut k = ExecutionKernel::new();
        let ex = k.begin(
            ExecutionTrigger::Scheduler,
            "s1",
            "sync the digest",
            None,
            "pol".into(),
            "ctx".into(),
            vec!["storage.read".to_string()],
        );
        k.transition(&ex.id, ExecutionPhase::Running).unwrap();
        k.persist_to(&path).unwrap();
        ex.id.clone() // drop = SIGKILL mid-run.
    };
    let recovered = ExecutionKernel::recover_from(&path).expect("P50.5.6: kernel must recover");
    let ex = recovered
        .get(&exec_id)
        .expect("P50.5.6: crashed execution must survive");
    assert_eq!(
        ex.state,
        ExecutionPhase::Running,
        "P50.5.6: crashed execution must recover Running (never Completed), got {:?}",
        ex.state
    );
    assert_eq!(recovered.counter(), 1, "P50.5.6: id counter must survive");
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("P50.5.6: crashed kernel recovered Running with its counter");
}

#[test]
fn audit_chain_survives_reopen_with_exact_order() {
    let dir = temp_dir("audit-reopen");
    {
        let mut log = SessionLog::open(&dir, "s-crash").expect("open log");
        log.append(EventInput::new(EventType::TaskStarted, "s-crash", "a1"))
            .expect("append 1");
        log.append(
            EventInput::new(EventType::ToolCompleted, "s-crash", "a1").with_tool("fs.write", "h1"),
        )
        .expect("append 2");
        assert_eq!(log.seq(), 2);
        // drop = crash before any further event.
    }
    let mut log = SessionLog::open(&dir, "s-crash").expect("reopen log");
    assert_eq!(log.seq(), 2, "P50.5.6: seq must resume, not reset");
    let events = log.events().expect("read back");
    assert_eq!(
        events.len(),
        2,
        "P50.5.6: both pre-crash events must replay"
    );
    let seq3 = log
        .append(EventInput::new(
            EventType::CheckpointCommitted,
            "s-crash",
            "a1",
        ))
        .expect("append after recovery");
    assert_eq!(seq3, 3, "P50.5.6: post-crash append continues the chain");
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("P50.5.6: audit chain reopened with exact seq/order continuity");
}

// ---------------------------------------------------------------------------
// P50.5.2 (composition) — a real search round-trip lands as a tool receipt in
// the audit chain. Env-gated like the search-crate leg; the receipt shape is
// what the UI trajectory renders.
// ---------------------------------------------------------------------------

struct LocalSearxng;

impl agentcowork_search::SearchTransport for LocalSearxng {
    fn search(
        &self,
        endpoint: &str,
        query: &str,
    ) -> Result<Vec<agentcowork_search::SearchResult>, String> {
        if endpoint == "ddg" {
            return Err("p50 gates transport has no DDG scraper".into());
        }
        let url = format!("{}/search", endpoint.trim_end_matches('/'));
        let body = ureq::get(&url)
            .query("q", query)
            .query("format", "json")
            .timeout(Duration::from_secs(20))
            .call()
            .map_err(|e| format!("searxng request failed: {e}"))?
            .into_string()
            .map_err(|e| format!("searxng read failed: {e}"))?;
        let v: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("searxng bad json: {e}"))?;
        let mut out = Vec::new();
        if let Some(arr) = v.get("results").and_then(|r| r.as_array()) {
            for r in arr {
                let url = r
                    .get("url")
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string();
                if url.is_empty() {
                    continue;
                }
                out.push(agentcowork_search::SearchResult {
                    url,
                    title: r
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string(),
                    snippet: String::new(),
                    source: endpoint.to_string(),
                });
            }
        }
        Ok(out)
    }

    fn fetch(&self, _tier: &str, _url: &str) -> Result<String, String> {
        Err("p50 gates leg does not fetch pages".into())
    }
}

#[test]
fn real_search_round_trip_lands_receipt_in_audit_chain() {
    let Some(base) = agentcowork_types::env_compat::get("E2E_SEARXNG_URL") else {
        eprintln!("SKIP P50.5.2 composition leg: set AGENTCOWORK_E2E_SEARXNG_URL");
        return;
    };
    let dir = temp_dir("search-receipt");
    // 1) live search → bounded citable results.
    let cascade = agentcowork_search::G8Cascade::new(
        Duration::from_secs(300),
        vec![base],
        3,
        Duration::from_secs(60),
    );
    let hits = cascade
        .query(&LocalSearxng, "rust programming language")
        .expect("P50.5.2: live search failed");
    assert!(!hits.is_empty(), "P50.5.2: live search returned nothing");

    // 2) tool receipt: the completed search as the trajectory renders it.
    let first = &hits[0];
    let receipt = serde_json::json!({
        "tool": "search.query",
        "query": "rust programming language",
        "hits": hits.len(),
        "top_url": first.url,
        "top_title": first.title,
    });
    let args_hash = format!("{:x}", {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        receipt.to_string().hash(&mut h);
        h.finish()
    });

    // 3) audit: Started → Completed(tool, args_hash) → reopen replays both.
    let mut log = SessionLog::open(&dir, "s-search").expect("open log");
    log.append(EventInput::new(
        EventType::ToolStarted,
        "s-search",
        "researcher",
    ))
    .expect("started");
    log.append(
        EventInput::new(EventType::ToolCompleted, "s-search", "researcher")
            .with_tool("search.query", &args_hash),
    )
    .expect("completed");
    drop(log);
    let log = SessionLog::open(&dir, "s-search").expect("reopen log");
    assert_eq!(log.seq(), 2, "P50.5.2: receipt pair must survive reopen");
    assert_eq!(log.events().expect("events").len(), 2);
    eprintln!(
        "P50.5.2: live search ({} hits) → receipt {} → audit seq 2",
        hits.len(),
        &args_hash[..8.min(args_hash.len())]
    );
    let _ = std::fs::remove_dir_all(&dir);
}
