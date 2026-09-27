//! `ARCH/10-KERNEL.md` §7 — the kernel-owned base envelope **on a real
//! transport crossing**, end to end.
//!
//! The envelope itself (`agentcowork_types::envelope`) is unit-tested and
//! acceptance-tested in the kernel crate. This file proves the one thing that
//! belongs to the transport: an envelope crosses a length-prefixed frame and
//! comes back decodable, with nothing about the internal process crossing with
//! it. That is the legitimate reason this crate dev-depends on
//! `agentcowork-types` (production code stays dependency-free per PURITY-1).

use agentcowork_ipc::frame;
use agentcowork_types::envelope::{
    ActorContext, ActorKind, RequestEnvelope, ResultEnvelope, TicketBinding, TypedEnvelope,
    decode_result, rule_for,
};
use agentcowork_types::error::{ErrorCode, KernelError};
use agentcowork_types::time::{EpochMillis, now_epoch_millis};
use agentcowork_types::{TicketId, WorkId};

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

#[test]
fn a_refusal_crosses_the_frame_as_a_typed_code_and_leaks_nothing() {
    let envelope = RequestEnvelope::new(
        "provider/call",
        actor(),
        serde_json::json!({"op": "send"}),
        Some(5_000),
    );
    // The same call, refused, answered as a typed failure.
    let rule = rule_for("CTR-010").expect("CTR-010 is registered");
    envelope.boundary_conforms(rule, 3).expect_err("no ticket");
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
}

#[test]
fn a_reply_crosses_the_frame_and_stays_attributable() {
    let envelope = RequestEnvelope::new(
        "work.step.execute",
        actor(),
        serde_json::json!({"step": 3}),
        Some(30_000),
    )
    .with_ticket(ticket(3), &work());
    let reply = envelope.to_result(Ok(serde_json::json!({"receipt": "r-9"})));
    let framed = frame::encode(&serde_json::to_vec(&reply).expect("serializable"));
    let payload = frame::decode_all(&framed).expect("decodes").remove(0);
    let back: TypedEnvelope<serde_json::Value> =
        serde_json::from_slice(&payload).expect("the reply decodes");
    // Against the envelope's *own* actor: `ActorContext::new` stamps `at` from
    // the clock, so a freshly built one is a millisecond later and the
    // comparison would be testing the clock, not the attribution.
    assert_eq!(back.actor, envelope.actor);
    assert_eq!(back.method, "work.step.execute");
    assert!(back.result.is_ok());
    assert_eq!(back.result.value.expect("a value")["receipt"], "r-9");
}
