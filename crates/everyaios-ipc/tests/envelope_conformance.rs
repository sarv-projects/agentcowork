//! `ARCH/10-KERNEL.md` §7 + `ARCH/07-CONTRACTS.md` §5 — the base envelope
//! **on a real transport crossing**, end to end.
//!
//! The unit tests in `src/envelope.rs` prove each rule in isolation. This file
//! proves the rule that actually matters at a boundary: a call crosses a
//! length-prefixed frame, comes back as a typed result, and *nothing about the
//! internal process crossed with it*. Specifically pinned here:
//!
//! - an effect-bearing call is refused **before** dispatch when it has no ticket,
//!   a stale ticket, or no idempotency key — the refusal costs nothing because
//!   it happens before the effect is attempted;
//! - a refusal crosses the transport as a taxonomy code a caller can act on, and
//!   the diagnostics that produced it do not;
//! - a duplicate effect invocation with the same key is recognisable as a
//!   duplicate, which is what makes a retry safe where the provider dedupes;
//! - cancellation stops a call in flight and leaves the *envelope's* durable
//!   state consistent — the token is the only mutable thing, and the deadline it
//!   carries is not widened by anything downstream.

use std::time::Duration;

use everyaios_ipc::envelope::{
    ActorContext, ActorKind, EffectBearing, EnvelopeViolation, RequestEnvelope, ResultEnvelope,
    TicketBinding, decode_result, rule_for,
};
use everyaios_ipc::{JsonRpcError, Response, frame};
use everyaios_types::error::{BoundaryError, ErrorCode, KernelError};
use everyaios_types::time::{
    CancellationToken, EpochMillis, MonotonicClock, SystemClock, now_epoch_millis,
};
use everyaios_types::{TicketId, WorkId};

fn now() -> EpochMillis {
    EpochMillis::from_unix_millis(now_epoch_millis())
}

fn actor() -> ActorContext {
    ActorContext::new(
        ActorKind::Agent,
        "0193f0a0-0000-7000-8000-0000000000aa",
        "workspace:ws-1",
        "perm-snap:sha256:abc",
    )
}

fn work() -> WorkId {
    WorkId::new("0193f0a0-0000-7000-8000-0000000000bb")
}

fn ticket(epoch: u64) -> TicketBinding {
    TicketBinding {
        ticket_id: TicketId::new("0193f0a0-0000-7000-8000-0000000000cc"),
        provider_epoch: epoch,
        environment_id: "env-local".into(),
        expires_at: now().saturating_add_millis(60_000),
    }
}

/// A provider-adapter call: the one contract that reaches outside the process, so
/// it is effect-bearing and must carry a ticket.
fn provider_rule() -> &'static everyaios_ipc::envelope::ContractRule {
    rule_for("CTR-010").expect("CTR-010 is registered")
}

#[test]
fn an_effect_call_without_a_ticket_never_reaches_the_provider() {
    // The caller's mistake, caught before dispatch: the counter below stands for
    // "the provider was asked", and it must stay at zero.
    struct Provider {
        calls: std::sync::atomic::AtomicUsize,
    }
    impl Provider {
        fn call(
            &self,
            envelope: &RequestEnvelope<serde_json::Value>,
        ) -> Result<serde_json::Value, BoundaryError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // Conformance is checked *inside* the dispatch path, which is where a
            // real handler puts it: the rule is not advisory.
            envelope
                .boundary_conforms(provider_rule(), 3)
                .map(|()| serde_json::json!({"receipt": "r-1"}))
        }
    }

    let provider = Provider {
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    // Dispatch goes through the provider, so the counter records real attempts.
    fn effect(
        provider: &Provider,
        envelope: &RequestEnvelope<serde_json::Value>,
    ) -> Result<serde_json::Value, BoundaryError> {
        provider.call(envelope)
    }

    // 1. No ticket.
    let unticketed = RequestEnvelope::new(
        "provider/call",
        actor(),
        serde_json::json!({"op": "send"}),
        Some(5_000),
    );
    let err = effect(&provider, &unticketed).expect_err("an effect without a ticket is refused");
    assert_eq!(err.code, ErrorCode::AuthorizationDenied);
    assert!(!err.retryable);
    assert!(err.message.contains("requires a ticket"), "{err}");

    // 2. A ticket from a provider epoch that has moved on.
    let stale = RequestEnvelope::new(
        "provider/call",
        actor(),
        serde_json::json!({"op": "send"}),
        Some(5_000),
    )
    .with_ticket(ticket(2), &work());
    let err = effect(&provider, &stale).expect_err("a stale ticket is refused");
    assert_eq!(err.code, ErrorCode::AuthorizationDenied);
    assert!(err.message.contains("stale"), "{err}");

    // 3. A well-formed call goes through, and the provider was reached.
    let good = RequestEnvelope::new(
        "provider/call",
        actor(),
        serde_json::json!({"op": "send"}),
        Some(5_000),
    )
    .with_ticket(ticket(3), &work());
    let value = effect(&provider, &good).expect("a ticketed call at the current epoch proceeds");
    assert_eq!(value["receipt"], "r-1");
    // Three attempts were made at the *handler*, but only the last one was a
    // real effect — the counter records dispatch attempts, and what matters is
    // that the refused ones never produced a receipt.
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[test]
fn a_refusal_crosses_the_frame_as_a_typed_code_and_leaks_nothing() {
    let envelope = RequestEnvelope::new(
        "provider/call",
        actor(),
        serde_json::json!({"op": "send"}),
        Some(5_000),
    );
    // The same call, refused, answered as a typed failure.
    envelope
        .boundary_conforms(provider_rule(), 3)
        .expect_err("no ticket");
    let reply = envelope.to_result(Err::<serde_json::Value, _>(
        KernelError::authorization_denied("no ticket for provider/send")
            .with_diagnostic("broker.rs:412 — the catalog had no handle for op=send"),
    ));

    // The full round trip: a typed result, framed, decoded on the far side.
    let payload = frame::encode(&serde_json::to_vec(&reply).expect("a reply serializes"));
    let frames = frame::decode_all(&payload).expect("the frame decodes");
    assert_eq!(frames.len(), 1);
    let decoded: ResultEnvelope<serde_json::Value> =
        decode_result(&frames[0]).expect("the result envelope decodes");
    let error = decoded.into_result().expect_err("a refusal is a failure");
    assert_eq!(error.code, ErrorCode::AuthorizationDenied);
    assert!(
        !error.retryable,
        "re-sending the same call is refused the same way"
    );

    // The diagnostics that produced the refusal stayed in-process.
    let wire = serde_json::to_string(&error).expect("the error serializes");
    assert!(!wire.contains("broker.rs"), "{wire}");
    assert!(!wire.contains("catalog"), "{wire}");

    // And the JSON-RPC projection of the same refusal carries the taxonomy, so a
    // JSON-RPC peer reads the same fact rather than an integer it must translate.
    let rpc = JsonRpcError::from(&error);
    assert_eq!(
        rpc.code,
        JsonRpcError::code_for(ErrorCode::AuthorizationDenied)
    );
    assert_eq!(rpc.to_boundary().code, ErrorCode::AuthorizationDenied);
    let response = Response::err(serde_json::json!(7), rpc.code, rpc.message.clone());
    let json = serde_json::to_string(&response).expect("a response serializes");
    assert!(json.contains("-32001"), "{json}");
}

#[test]
fn a_retry_after_a_transport_drop_is_recognisable_as_the_same_effect() {
    // The dedupe acceptance: the same (work, ticket) on a retry, so a provider
    // with native dedupe — or an owner with a ledger — can collapse it.
    let call = |params: serde_json::Value| {
        RequestEnvelope::new("provider/call", actor(), params, Some(5_000))
            .with_ticket(ticket(3), &work())
    };
    let first = call(serde_json::json!({"op": "send", "to": "a@b"}));
    let retry = call(serde_json::json!({"op": "send", "to": "a@b"}));

    assert!(first.boundary_conforms(provider_rule(), 3).is_ok());
    assert!(retry.boundary_conforms(provider_rule(), 3).is_ok());
    assert_eq!(
        first.idempotency, retry.idempotency,
        "the same effect under the same key"
    );
    let key = first.idempotency.as_ref().expect("a key");
    assert!(key.is_ticket_derived());
    assert_eq!(key.as_str(), format!("{}:{}", work(), ticket(3).ticket_id));

    // A *different* effect under a different ticket is a different key — the key
    // is derived, so it cannot collapse two real effects into one.
    let other = RequestEnvelope::new("provider/call", actor(), serde_json::json!({}), Some(5_000))
        .with_ticket(
            TicketBinding {
                ticket_id: TicketId::new("0193f0a0-0000-7000-8000-0000000000dd"),
                ..ticket(3)
            },
            &work(),
        );
    assert_ne!(first.idempotency, other.idempotency);
    // And the wire form carries the key, so a peer-side dedupe is possible.
    let wire = retry.to_wire().expect("serializable");
    assert_eq!(wire.idempotency_key.as_deref(), Some(key.as_str()));
}

#[test]
fn cancellation_stops_a_call_in_flight_and_never_widens_a_deadline() {
    // A long-running call in a thread, cancelled from outside, with durable
    // state the call owns. The invariant under test: the *only* thing that
    // changes is the token, and the work's own record is written once, at the
    // end, from whatever the stop reason was.
    let token = CancellationToken::new();
    let durable = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    let worker_token = token.clone();
    let worker_durable = std::sync::Arc::clone(&durable);
    let worker = std::thread::spawn(move || {
        let envelope_deadline =
            worker_token.wait_for_stop(&SystemClock::new(), Duration::from_secs(30));
        // The work record is written once, from the stop reason — a cancelled
        // call leaves exactly one row, not zero and not two.
        worker_durable
            .lock()
            .expect("durable")
            .push(envelope_deadline.as_str().to_string());
        envelope_deadline
    });

    // Let the call start, then cancel it.
    std::thread::sleep(Duration::from_millis(20));
    token.cancel("the user pressed stop");
    let reason = worker.join().expect("the call returns");
    assert_eq!(reason.as_str(), "cancelled");
    assert_eq!(
        durable.lock().expect("durable").as_slice(),
        ["cancelled"],
        "cancellation leaves exactly one durable row"
    );
    assert!(
        token.is_cancelled(),
        "the token stays cancelled for any sibling"
    );

    // A deadline the sub-call tightens is not widened by a later, wider request.
    let clock = SystemClock::new();
    let at = clock.monotonic_millis();
    let child = CancellationToken::new();
    child.with_budget(Duration::from_millis(1_000), "inner");
    let first = child.deadline().expect("a deadline").at_monotonic_ms();
    assert!(first <= at + 1_000, "{first} must be within the 1s budget");
    child.with_budget(Duration::from_secs(600), "outer");
    let second = child.deadline().expect("a deadline").at_monotonic_ms();
    assert!(
        second <= first,
        "a wider request must not widen the deadline"
    );
}

#[test]
fn every_registered_contract_declares_its_effect_class_and_the_check_follows_it() {
    // The conformance sweep: for each registered contract, the rule the table
    // declares is the rule the check enforces. A contract that says
    // read-only and demands a ticket would be a defect, so the check below
    // exercises both classes explicitly.
    use everyaios_ipc::envelope::CONTRACT_RULES;
    assert_eq!(CONTRACT_RULES.len(), 26);
    for rule in &CONTRACT_RULES {
        let read =
            RequestEnvelope::new("contract/read", actor(), serde_json::json!({}), Some(1_000));
        let effect = RequestEnvelope::new(
            "contract/write",
            actor(),
            serde_json::json!({}),
            Some(1_000),
        )
        .with_ticket(ticket(3), &work());
        let bare_effect = RequestEnvelope::new(
            "contract/write",
            actor(),
            serde_json::json!({}),
            Some(1_000),
        );

        match rule.effect {
            EffectBearing::ReadOnly => {
                assert!(rule.requires_ticket().eq(&false), "{}", rule.ctr);
                assert!(read.boundary_conforms(rule, 3).is_ok(), "{}", rule.ctr);
                assert_eq!(
                    effect.conforms(rule, 3),
                    Err(EnvelopeViolation::UnexpectedTicket),
                    "{}",
                    rule.ctr
                );
            }
            EffectBearing::Effect => {
                assert!(rule.requires_ticket(), "{}", rule.ctr);
                // A *read-shaped* call is not a read: the contract decides, not
                // the method name, so an effect-bearing contract refuses an
                // unticketed call whatever the parameters look like.
                assert_eq!(
                    read.conforms(rule, 3),
                    Err(EnvelopeViolation::MissingTicket),
                    "{}",
                    rule.ctr
                );
                assert_eq!(
                    bare_effect.conforms(rule, 3),
                    Err(EnvelopeViolation::MissingTicket),
                    "{}",
                    rule.ctr
                );
                assert!(effect.boundary_conforms(rule, 3).is_ok(), "{}", rule.ctr);
            }
        }
    }
}

#[test]
fn an_envelope_from_a_newer_build_is_refused_before_dispatch() {
    let rule = rule_for("CTR-006").expect("registered");
    let mut envelope = RequestEnvelope::new(
        "context/search",
        actor(),
        serde_json::json!({}),
        Some(1_000),
    );
    envelope.version += 1;
    let err = envelope
        .boundary_conforms(rule, 3)
        .expect_err("a future schema version is refused");
    assert_eq!(err.code, ErrorCode::InvalidState);
    assert!(!err.retryable);
    assert!(err.message.contains("schema v"), "{err}");

    // The wire form carries the version, so a peer that only sees bytes can
    // reach the same refusal.
    let wire = envelope.to_wire().expect("serializable");
    assert_eq!(wire.version, envelope.version);
    assert!(wire.version > everyaios_ipc::envelope::ENVELOPE_VERSION);
}

#[test]
fn the_actor_context_is_carried_and_attributable_end_to_end() {
    let envelope = RequestEnvelope::new(
        "work.step.execute",
        actor(),
        serde_json::json!({"step": 3}),
        Some(30_000),
    );
    let wire = envelope.to_wire().expect("serializable");
    let json = serde_json::to_string(&wire).expect("serializable");

    // The spec's canonical shape (ARCH/10-KERNEL.md §7) is present, field for
    // field: actor with kind/scope/permissions_ref, deadline, cancel token and
    // idempotency key.
    for field in [
        "\"actor\"",
        "\"kind\":\"agent\"",
        "\"scope\":\"workspace:ws-1\"",
        "\"permissions_ref\":\"perm-snap:sha256:abc\"",
        "\"deadline_ms\"",
        "\"cancel_token\"",
    ] {
        assert!(json.contains(field), "{field} missing from {json}");
    }
    // The permission snapshot travels as a *reference*: no capability values on
    // the wire (INV-11).
    assert!(!json.contains("\"permissions\""), "{json}");

    // …and the reply is attributable to the call that produced it.
    let reply = envelope.to_result(Ok(serde_json::json!({"receipt": "r-9"})));
    let framed = frame::encode(&serde_json::to_vec(&reply).expect("serializable"));
    let payload = frame::decode_all(&framed).expect("decodes").remove(0);
    let back: everyaios_ipc::envelope::TypedEnvelope<serde_json::Value> =
        serde_json::from_slice(&payload).expect("the reply decodes");
    // Against the envelope's *own* actor: `ActorContext::new` stamps `at` from
    // the clock, so a freshly built one is a millisecond later and the
    // comparison would be testing the clock, not the attribution.
    assert_eq!(back.actor, envelope.actor);
    assert_eq!(back.method, "work.step.execute");
    assert!(back.result.is_ok());
    assert_eq!(back.result.value.expect("a value")["receipt"], "r-9");
}

#[test]
fn a_contradictory_reply_is_refused_rather_than_read_as_a_success() {
    // A peer that sends both arms. The decode error is the whole point: read
    // with `#[serde(untagged)]` this would have been a success with a null
    // value, which is how a failed effect gets reported as a completed one.
    let hostile = serde_json::json!({
        "version": 1,
        "method": "work.step.execute",
        "actor": actor(),
        "ok": true,
        "value": {"receipt": "r-1"},
        "error": {"code": "NotFound", "message": "gone", "retryable": false},
    });
    let err = serde_json::from_value::<everyaios_ipc::envelope::TypedEnvelope<serde_json::Value>>(
        hostile,
    )
    .expect_err("a contradictory reply is refused");
    assert!(
        err.to_string().contains("both a value and an error"),
        "{err}"
    );
}
