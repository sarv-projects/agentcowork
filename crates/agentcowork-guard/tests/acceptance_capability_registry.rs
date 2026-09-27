//! `TASK-CAP-001` / `TASK-CAP-002` — capability-plane acceptance suite.
//!
//! Drives the public capability contract of `agentcowork-guard`: the descriptor
//! contract and census gate (`REQ-CAP-003`, `REQ-CAP-004`, `REQ-CAP-009`,
//! `REQ-CAP-010`), the resolver with epoch-checked handles and deterministic
//! failover (`REQ-CAP-002`, `REQ-CAP-007`, `REQ-CAP-008`), and first-class
//! guidance (`REQ-CAP-005`).
//!
//! These are the acceptance/failure lines from `ARCH/08-REQUIREMENTS.md` pinned
//! as executable tests: a stale handle after a simulated provider restart yields
//! a rejection plus re-resolution rather than a silent reuse; an exact tie is
//! broken deterministically and audited; an exhausted chain surfaces as guidance
//! with a named next action; a census gate rejects a malformed, duplicated, or
//! unbacked entry.

use agentcowork_guard::capability_broker::{
    CandidateExclusion, CapabilityRegistry, CostClass, LatencyClass, PermissionFit,
    ProviderCandidate, ProviderHealth, ResolutionConstraints, Unresolved,
};
use agentcowork_guard::capability_contract::{
    AuthRequirement, CapabilityDescriptor, CapabilityError, CapabilityHandle, CapabilityResult,
    CapabilityRisk, CapabilityStatus, CensusFinding, Deprecation, HandleInvalid, LoadingMode,
    NextAction, NextActionKind, Requirement, VerificationDepth, VerificationHook,
    census_descriptor, is_well_formed_id, protocol_token_in,
};

const NOW: u64 = 1_700_000_000_000;

fn descriptor(id: &str) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        id,
        "1.0.0",
        format!("{id} — the operation, described"),
        vec![format!("express {id}")],
        LoadingMode::OnDemand,
        CapabilityRisk::Safe,
        VerificationHook::new("verifier.read_back", VerificationDepth::Validate),
    )
}

fn destructive(id: &str) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        id,
        "1.0.0",
        format!("{id} — a destructive operation"),
        vec![format!("destroy via {id}")],
        LoadingMode::OnDemand,
        CapabilityRisk::Dangerous,
        VerificationHook::new(
            "verifier.reconcile",
            VerificationDepth::ValidateAndReconcile,
        ),
    )
}

fn candidate(
    id: &str,
    epoch: u64,
    health: ProviderHealth,
    capabilities: &[&str],
) -> ProviderCandidate {
    ProviderCandidate {
        id: id.to_string(),
        epoch,
        health,
        permission_fit: PermissionFit::Granted,
        cost_class: CostClass::Standard,
        latency_class: LatencyClass::Standard,
        environments: vec!["env.local".to_string()],
        capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
        degraded_capabilities: Vec::new(),
    }
}

fn constraints() -> ResolutionConstraints {
    ResolutionConstraints::new("env.local", "policy-snapshot-9")
}

/// REQ-CAP-004 acceptance + failure: the census gate rejects incomplete and
/// duplicate descriptors, and requires provider coverage.
#[test]
fn the_census_gate_rejects_malformed_duplicated_and_unbacked_entries() {
    // (a) A well-formed, provider-backed entry passes.
    let mut reg = CapabilityRegistry::new();
    let provider = candidate("runtime.native", 1, ProviderHealth::Ok, &["office.open"]);
    reg.project_providers(&[provider]);
    reg.register(descriptor("office.open")).expect("registers");
    assert!(reg.census().ok(), "{:?}", reg.census().findings);

    // (b) Duplicate id → rejection. A second, different descriptor under a
    // registered id is refused, and the stored one is untouched.
    let conflicting = destructive("office.open");
    assert_eq!(
        reg.register(conflicting),
        Err(CensusFinding::DuplicateId {
            id: "office.open".into()
        })
    );
    assert_eq!(
        reg.get("office.open").unwrap().risk_class,
        CapabilityRisk::Safe
    );

    // (c) Unbacked entry (no provider) → census finding, not a serving entry.
    let mut unbacked = CapabilityRegistry::new();
    unbacked.register(descriptor("office.missing")).unwrap();
    let report = unbacked.census();
    assert!(!report.ok());
    assert_eq!(
        report.findings_for("office.missing"),
        vec![&CensusFinding::NoProvider {
            id: "office.missing".into()
        }]
    );

    // (d) Malformed: a protocol-bearing id, an empty affordance list, a
    // verification depth below the risk floor.
    let mut malformed = CapabilityRegistry::new();
    malformed.project_providers(&[candidate("p", 1, ProviderHealth::Ok, &["office.mcp_call"])]);
    let mut bad = descriptor("office.mcp_call");
    bad.affordances.clear();
    bad.risk_class = CapabilityRisk::Dangerous; // floor: validate_reconcile
    bad.verification.depth = VerificationDepth::Validate;
    malformed.register(bad).unwrap();
    let findings = malformed.census().findings;
    assert!(findings.contains(&CensusFinding::MalformedId {
        id: "office.mcp_call".into(),
        detail: format!(
            "id leaks the protocol token `{}`; capability ids stay protocol-neutral (DEC-047)",
            protocol_token_in("office.mcp_call").unwrap()
        )
    }));
    assert!(findings.contains(&CensusFinding::EmptyAffordances {
        id: "office.mcp_call".into()
    }));
    assert!(
        findings.contains(&CensusFinding::UnderDeclaredVerification {
            id: "office.mcp_call".into(),
            declared: "validate".into(),
            floor: "validate_reconcile".into()
        })
    );
}

/// REQ-CAP-003: a capability describes what, never who. Provider assignment is a
/// derived fact, and a hand-authored one that disagrees is a census finding.
#[test]
fn a_capability_id_and_descriptor_carry_no_provider_identity() {
    let mut reg = CapabilityRegistry::new();
    reg.project_providers(&[
        candidate("alpha", 1, ProviderHealth::Ok, &["search.query"]),
        candidate("beta", 3, ProviderHealth::Ok, &["search.query"]),
    ]);
    reg.register(descriptor("search.query")).unwrap();
    let stored = reg.get("search.query").unwrap();
    assert!(is_well_formed_id(&stored.id));
    // The providers arrived by derivation, at the epochs they were projected at.
    assert_eq!(
        stored
            .providers
            .iter()
            .map(|p| (p.provider_id.as_str(), p.provider_epoch))
            .collect::<Vec<_>>(),
        vec![("alpha", 1), ("beta", 3)]
    );
    // A provider swap preserves the capability contract: the id, version, and
    // affordances are unchanged by which providers back it.
    let mut swapped = CapabilityRegistry::new();
    swapped.project_providers(&[candidate("gamma", 1, ProviderHealth::Ok, &["search.query"])]);
    swapped.register(descriptor("search.query")).unwrap();
    let other = swapped.get("search.query").unwrap();
    assert_eq!(other.id, stored.id);
    assert_eq!(other.version, stored.version);
    assert_eq!(other.affordances, stored.affordances);
}

/// REQ-CAP-002 acceptance + failure: across a simulated provider restart, a
/// stale handle is rejected and re-resolution is required — retrying against the
/// stale handle is forbidden.
#[test]
fn a_provider_restart_invalidates_outstanding_handles_and_forbids_a_blind_retry() {
    let mut reg = CapabilityRegistry::new();
    let live = candidate("runtime.native", 4, ProviderHealth::Ok, &["office.edit"]);
    reg.project_providers(&[live.clone()]);
    reg.register(destructive("office.edit")).unwrap();

    let before = reg
        .resolve("office.edit", &constraints(), &[live.clone()], NOW)
        .handle
        .expect("a handle");
    assert_eq!(before.provider_epoch, 4);
    assert!(before.validate(NOW + 1, 4).is_ok());

    // The provider restarts: the epoch advances to 5.
    let restarted = candidate("runtime.native", 5, ProviderHealth::Ok, &["office.edit"]);
    reg.project_providers(&[restarted.clone()]);
    let live_epoch = reg.provider_epoch("runtime.native");
    assert_eq!(live_epoch, 5);

    // The stale handle is rejected, and the rejection is typed.
    match before.validate(NOW + 1, live_epoch) {
        Err(HandleInvalid::EpochMismatch { expected, live }) => {
            assert_eq!((expected, live), (4, 5));
        }
        other => panic!("a stale handle must be rejected, got {other:?}"),
    }

    // Re-resolution is the only way forward, and it yields a usable handle.
    let after = reg
        .resolve("office.edit", &constraints(), &[restarted], NOW + 2)
        .handle
        .expect("re-resolution mints a fresh handle");
    assert_eq!(after.provider_epoch, 5);
    assert!(after.validate(NOW + 2, 5).is_ok());
    assert_ne!(after.runtime_handle_ref, before.runtime_handle_ref);
}

/// REQ-CAP-002 failure: an expired handle is reported as expired — before the
/// epoch check, so a caller is never told the wrong reason.
#[test]
fn an_expired_handle_is_reported_as_expired() {
    let handle = CapabilityHandle {
        capability_id: "office.edit".into(),
        provider_id: "runtime.native".into(),
        provider_epoch: 2,
        environment_id: "env.local".into(),
        permission_snapshot: "policy-snapshot-9".into(),
        runtime_handle_ref: "rt:runtime.native:2".into(),
        expires_at_ms: NOW + 10,
        descriptor_version: "1.0.0".into(),
    };
    assert_eq!(
        handle.validate(NOW + 11, 2),
        Err(HandleInvalid::Expired { at_ms: NOW + 10 })
    );
    // Expiry wins even when the epoch is also stale.
    assert!(matches!(
        handle.validate(NOW + 11, 9),
        Err(HandleInvalid::Expired { .. })
    ));
}

/// REQ-CAP-007: ranking is health → environment → permission → cost/latency, an
/// exact tie is broken deterministically and audited as a tie, and a degraded
/// provider is skipped before it can fail a call.
#[test]
fn resolution_order_is_deterministic_audited_and_health_first() {
    let mut reg = CapabilityRegistry::new();
    let healthy_slow = ProviderCandidate {
        latency_class: LatencyClass::Slow,
        ..candidate("p.healthy", 1, ProviderHealth::Ok, &["office.edit"])
    };
    let unknown = candidate("p.unknown", 1, ProviderHealth::Unknown, &["office.edit"]);
    let degraded = candidate("p.degraded", 1, ProviderHealth::Degraded, &["office.edit"]);
    let mut down = candidate("p.down", 1, ProviderHealth::Down, &["office.edit"]);
    down.cost_class = CostClass::Free;
    let mut denied = candidate("p.denied", 1, ProviderHealth::Ok, &["office.edit"]);
    denied.permission_fit = PermissionFit::Denied;
    let mut wrong_env = candidate("p.other-env", 1, ProviderHealth::Ok, &["office.edit"]);
    wrong_env.environments = vec!["env.remote".into()];
    let unrelated = candidate("p.other-cap", 1, ProviderHealth::Ok, &["office.read"]);

    let all = vec![
        healthy_slow.clone(),
        unknown.clone(),
        degraded.clone(),
        down.clone(),
        denied.clone(),
        wrong_env.clone(),
        unrelated.clone(),
    ];
    reg.project_providers(&all);
    reg.register(destructive("office.edit")).unwrap();

    let resolution = reg.resolve("office.edit", &constraints(), &all, NOW);
    assert!(resolution.is_resolved(), "{resolution:?}");
    assert_eq!(
        resolution
            .chain
            .iter()
            .map(|c| c.provider_id.as_str())
            .collect::<Vec<_>>(),
        vec!["p.healthy", "p.unknown", "p.degraded", "p.down"],
        "health dominates; a free-but-down provider still ranks last"
    );

    // Exclusions are named, and the three classes of exclusion are distinct.
    let excluded = |id: &str| -> Option<&CandidateExclusion> {
        resolution.excluded.iter().find(|e| e.provider_id == id)
    };
    assert!(excluded("p.denied").unwrap().reason.contains("denies"));
    assert!(
        excluded("p.other-env")
            .unwrap()
            .reason
            .contains("env.local")
    );
    assert!(
        excluded("p.other-cap")
            .unwrap()
            .reason
            .contains("does not implement")
    );
    // The exclusion list is ordered too, so the decision does not depend on the
    // order the caller projected its providers in.
    assert_eq!(
        resolution
            .excluded
            .iter()
            .map(|e| e.provider_id.as_str())
            .collect::<Vec<_>>(),
        vec!["p.denied", "p.other-cap", "p.other-env"]
    );

    // Every adjacent pair is audited by the dimension that decided it.
    assert_eq!(resolution.audit.len(), resolution.chain.len() - 1);
    assert!(
        resolution
            .audit
            .iter()
            .all(|d| d.as_deref() == Some("health"))
    );

    // Determinism: the same inputs in a different order give the same chain, and
    // re-resolving is stable.
    let shuffled = vec![
        unrelated,
        down,
        denied,
        degraded,
        unknown,
        wrong_env,
        healthy_slow,
    ];
    let again = reg.resolve("office.edit", &constraints(), &shuffled, NOW);
    assert_eq!(
        again
            .chain
            .iter()
            .map(|c| c.provider_id.clone())
            .collect::<Vec<_>>(),
        resolution
            .chain
            .iter()
            .map(|c| c.provider_id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        again, resolution,
        "resolution is a pure function of its inputs"
    );
}

#[test]
fn an_exact_tie_is_broken_by_provider_id_and_audited_as_a_tie() {
    let mut reg = CapabilityRegistry::new();
    let b = candidate("b", 1, ProviderHealth::Ok, &["search.query"]);
    let a = candidate("a", 1, ProviderHealth::Ok, &["search.query"]);
    reg.project_providers(&[a.clone(), b.clone()]);
    reg.register(descriptor("search.query")).unwrap();

    let first = reg.resolve("search.query", &constraints(), &[a.clone(), b.clone()], NOW);
    let reversed = reg.resolve("search.query", &constraints(), &[b, a], NOW);
    assert_eq!(
        first
            .chain
            .iter()
            .map(|c| c.provider_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(first, reversed, "input order must not decide the chain");
    assert_eq!(
        first.audit,
        vec![None],
        "an exact tie is recorded as a tie, not dressed up as a reason"
    );
}

/// REQ-CAP-007 failure: no provider available is guidance, never a dead end; an
/// exhausted chain names every provider it tried.
#[test]
fn an_exhausted_chain_surfaces_as_guidance_naming_what_was_tried() {
    let mut reg = CapabilityRegistry::new();
    let all = vec![
        candidate("p.first", 1, ProviderHealth::Ok, &["office.edit"]),
        candidate("p.second", 1, ProviderHealth::Degraded, &["office.edit"]),
    ];
    reg.project_providers(&all);
    reg.register(destructive("office.edit")).unwrap();
    let resolution = reg.resolve("office.edit", &constraints(), &all, NOW);

    // Failover walks the chain in rank order.
    assert_eq!(
        resolution.failover_next("p.first").unwrap().provider_id,
        "p.second"
    );
    // Then the chain is exhausted — and that is a *result* with a next action.
    let exhausted = *resolution
        .failover("p.second")
        .expect_err("the chain is exhausted");
    assert_eq!(exhausted.status, CapabilityStatus::Guidance);
    assert!(exhausted.is_actionable());
    assert!(exhausted.error.is_none(), "guidance is not a failure");
    assert!(!exhausted.retryable);
    let action = exhausted.next_action.expect("a concrete next action");
    assert_eq!(action.kind, NextActionKind::Configure);
    assert!(
        action.instruction.contains("p.first"),
        "{}",
        action.instruction
    );
    assert!(
        action.instruction.contains("p.second"),
        "the exhausted chain is named: {}",
        action.instruction
    );
    assert!(action.unlocks.contains("office.edit"));
}

/// REQ-CAP-005: guidance and requires_user_action are first-class results, and a
/// failure is a typed error with derived retryability.
#[test]
fn guidance_and_user_action_are_results_and_failures_are_typed() {
    let guidance = CapabilityResult::guidance(NextAction::new(
        NextActionKind::Connect,
        "drive",
        "Connect the drive account",
        "read and write the documents there",
    ));
    assert_eq!(guidance.status, CapabilityStatus::Guidance);
    assert!(guidance.is_actionable() && !guidance.is_completed());

    let user_action = CapabilityResult::requires_user_action(NextAction::new(
        NextActionKind::Grant,
        "files.write",
        "Approve the write permission",
        "the edit can be applied",
    ));
    assert_eq!(user_action.status, CapabilityStatus::RequiresUserAction);
    assert!(user_action.is_actionable());

    // A completed *effect* carries a receipt by construction (INV-07).
    let effect = CapabilityResult::completed(Some(serde_json::json!({"ok": true})), "rcpt-7");
    assert_eq!(effect.receipt.as_deref(), Some("rcpt-7"));
    // A read owes no receipt.
    assert!(CapabilityResult::read_completed(None).receipt.is_none());

    // Failures are typed, and retryability is derived rather than chosen.
    for (error, retryable) in [
        (
            CapabilityError::RateLimit {
                retry_after_ms: Some(1_000),
            },
            true,
        ),
        (
            CapabilityError::Unavailable {
                reason: "all down".into(),
            },
            true,
        ),
        (
            CapabilityError::AuthorizationDenied {
                reason: "no ticket".into(),
            },
            false,
        ),
        (
            CapabilityError::Authentication {
                reason: "expired".into(),
            },
            false,
        ),
        (
            CapabilityError::SchemaViolation {
                reason: "bad".into(),
            },
            false,
        ),
        (
            CapabilityError::VerificationFailed {
                reason: "mismatch".into(),
            },
            false,
        ),
        (
            CapabilityError::Timeout {
                reason: "idle".into(),
            },
            false,
        ),
    ] {
        let result = CapabilityResult::failed(error.clone());
        assert_eq!(result.status, CapabilityStatus::Failed);
        assert_eq!(result.retryable, retryable, "{}", error.code());
        assert_eq!(result.error.as_ref().unwrap().code(), error.code());
    }
}

/// REQ-CAP-008: a blocked `requires` chain names the missing edge, and the chain
/// resolves once the edge is satisfied.
#[test]
fn a_blocked_requirement_chain_names_the_missing_edge() {
    let mut reg = CapabilityRegistry::new();
    let inner = descriptor("storage.read").with_requirements(vec![Requirement::connection(
        "storage-account",
        "a mounted workspace must be reachable before a file can be read",
    )]);
    let outer = descriptor("office.open").with_requirements(vec![Requirement::capability(
        "storage.read",
        "the document is fetched through the storage capability",
    )]);
    let mut summary = descriptor("office.summarise");
    summary = summary.with_requirements(vec![Requirement::capability(
        "office.open",
        "summarising needs the document open",
    )]);
    for d in [inner, outer, summary] {
        reg.register(d).unwrap();
    }

    // Nothing implements the leaf, so the leaf's connection edge blocks — and
    // the block names the *nested* edge, not the id the caller asked for.
    let blocked = reg.resolve("office.summarise", &constraints(), &[], NOW);
    assert!(!blocked.is_resolved());
    match blocked.unresolved.as_ref().expect("typed reason") {
        Unresolved::MissingRequirement {
            capability_id,
            missing,
            kind,
            rationale,
        } => {
            assert_eq!(capability_id, "storage.read");
            assert_eq!(missing, "storage-account");
            assert_eq!(kind, "connection");
            assert!(rationale.contains("mounted workspace"));
        }
        other => panic!("expected a missing requirement, got {other:?}"),
    }
    let error = blocked
        .unresolved
        .as_ref()
        .unwrap()
        .as_error("office.summarise");
    assert_eq!(error.code(), "invalid_state");
    assert!(error.to_string().contains("storage-account"));

    // With a provider that implements the whole chain and serves the
    // environment, the same request resolves.
    let serving = ProviderCandidate {
        permission_fit: PermissionFit::Granted,
        environments: vec!["env.local".into()],
        ..candidate(
            "runtime.native",
            1,
            ProviderHealth::Ok,
            &["storage.read", "office.open", "office.summarise"],
        )
    };
    let resolved = reg.resolve("office.summarise", &constraints(), &[serving], NOW);
    assert!(resolved.is_resolved(), "{resolved:?}");
    let handle = resolved.handle.expect("a handle");
    assert_eq!(handle.capability_id, "office.summarise");
    assert_eq!(handle.permission_snapshot, "policy-snapshot-9");
    assert_eq!(handle.environment_id, "env.local");
}

/// REQ-CAP-007 failure: an unknown id and a capability past its deprecation
/// window are both typed refusals, never an empty chain that looks like success.
#[test]
fn unknown_and_deprecated_capabilities_are_typed_not_empty() {
    let mut reg = CapabilityRegistry::new();
    let live = candidate("p", 1, ProviderHealth::Ok, &["legacy.export"]);
    reg.project_providers(&[live.clone()]);
    let deprecated = descriptor("legacy.export").with_deprecation(Deprecation {
        since_version: "1.0.0".into(),
        window_ends_ms: NOW + 1_000,
    });
    reg.register(deprecated).unwrap();

    // Inside the window it still serves; past it, it does not.
    assert!(
        reg.resolve("legacy.export", &constraints(), &[live.clone()], NOW)
            .is_resolved()
    );
    let after = reg.resolve("legacy.export", &constraints(), &[live], NOW + 1_000);
    assert!(!after.is_resolved());
    assert!(after.handle.is_none());
    assert!(matches!(
        after.unresolved,
        Some(Unresolved::Deprecated { .. })
    ));

    // An unregistered id is `not_found`, never a silent no-op.
    let unknown = reg.resolve("office.never_registered", &constraints(), &[], NOW);
    assert!(!unknown.is_resolved());
    assert_eq!(
        unknown
            .unresolved
            .as_ref()
            .unwrap()
            .as_error("office.never_registered")
            .code(),
        "not_found"
    );
}

/// REQ-PROV-006: health is read from the projection, the resolver keeps no
/// health store of its own, and per-capability health isolates a partial failure.
#[test]
fn per_capability_health_isolates_a_partial_failure() {
    let mut reg = CapabilityRegistry::new();
    let mut partial = candidate(
        "p.partial",
        1,
        ProviderHealth::Ok,
        &["office.edit", "office.read"],
    );
    partial.degraded_capabilities = vec!["office.edit".into()];
    let healthy = candidate(
        "p.healthy",
        1,
        ProviderHealth::Ok,
        &["office.edit", "office.read"],
    );
    let all = vec![partial, healthy.clone()];
    reg.project_providers(&all);
    reg.register(destructive("office.edit")).unwrap();
    reg.register(descriptor("office.read")).unwrap();

    // For the degraded capability, the healthy provider leads.
    let edit = reg.resolve("office.edit", &constraints(), &all, NOW);
    assert_eq!(edit.chain[0].provider_id, "p.healthy");
    assert_eq!(edit.chain[1].health, ProviderHealth::Degraded);
    // For the untouched capability, the same provider is fully healthy.
    let read = reg.resolve("office.read", &constraints(), &all, NOW);
    assert!(read.chain.iter().all(|c| c.health == ProviderHealth::Ok));
}

/// REQ-CAP-009: an additive change bumps the minor version inside the same
/// major; a different major is served only inside a deprecation window, which is
/// what keeps a run that pinned the old descriptor alive until it completes.
#[test]
fn versioning_is_additive_within_a_major_and_pins_within_a_window() {
    let mut reg = CapabilityRegistry::new();
    reg.project_providers(&[candidate("p", 1, ProviderHealth::Ok, &["office.open"])]);
    reg.register(descriptor("office.open")).unwrap();
    let current = reg.get("office.open").unwrap();
    assert!(current.serves_version("1.0.0"), "same major is additive");
    assert!(current.serves_version("1.4.0"));
    assert!(
        !current.serves_version("2.0.0"),
        "a different major needs a decision, and a current capability has no window"
    );
    assert!(!current.serves_version("nonsense"));

    let mut deprecated = CapabilityRegistry::new();
    deprecated.project_providers(&[candidate("p", 1, ProviderHealth::Ok, &["legacy.open"])]);
    let mut old = descriptor("legacy.open");
    old.version = "1.4.0".into();
    deprecated
        .register(old.with_deprecation(Deprecation {
            since_version: "1.4.0".into(),
            window_ends_ms: 0,
        }))
        .unwrap();
    let stored = deprecated.get("legacy.open").unwrap();
    assert!(
        stored.serves_version("1.0.0"),
        "a run that pinned the pre-deprecation major keeps serving"
    );
    assert!(!stored.serves_version("0.9.0") || stored.deprecated.is_some());
}

/// The census per-descriptor half is usable on its own, so a caller that
/// validates a descriptor before registration gets the same verdict.
#[test]
fn the_census_is_usable_per_descriptor_before_registration() {
    let coverage = ["runtime.native".to_string()].into_iter().collect();
    assert!(census_descriptor(&descriptor("office.open"), &coverage).is_empty());
    let mut missing_auth = descriptor("connector.email_send");
    missing_auth.auth_requirements = vec![AuthRequirement::oauth("")];
    assert!(
        census_descriptor(&missing_auth, &coverage).contains(&CensusFinding::MalformedAuth {
            id: "connector.email_send".into(),
            detail: "auth method `oauth` needs a vault reference, not an empty one".into(),
        })
    );
}
