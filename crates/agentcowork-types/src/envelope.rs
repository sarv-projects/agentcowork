//! `ARCH/10-KERNEL.md` §7 + `ARCH/07-CONTRACTS.md` §5 — **the base envelope**
//! every `CTR-*` invocation carries.
//!
//! Four things, and each of them exists because a specific rule above it is
//! otherwise easy to miss on one of twenty-six contracts:
//!
//! 1. **Actor context** — who is calling (`user · agent · workflow`), the scope
//!    they act in, and a *reference* to the permission snapshot. The reference
//!    and not the values: an envelope crosses a trust boundary, and `INV-11`
//!    says an external caller receives projections only. The Trust owner
//!    resolves the reference; nothing on the wire is a capability.
//! 2. **Cancellation with deadline propagation** — a cooperative token and an
//!    absolute deadline. `Deadline::tighten` is what makes "deadlines propagate"
//!    a property rather than a habit: a sub-call takes the tighter of the two,
//!    so no call outlives its caller.
//! 3. **Idempotency for effects** — `work_id` + `ticket` derive the key
//!    (`ARCH/07-CONTRACTS.md` §5.4), so a retry after a transport failure sends
//!    the *same* key and a provider that supports dedupe can collapse it.
//! 4. **A versioned result envelope** — `{ ok, value } | { error }`, with a
//!    schema version. The union is decoded by hand, not by `#[serde(untagged)]`:
//!    a document that is both, or neither, is a **decode error**, not a silently
//!    coerced success. An envelope that can be read two ways is an envelope
//!    whose success has no meaning.
//!
//! # `effect_bearing` is a property of the *contract*, not of the caller
//!
//! [`ContractRule`] is the per-contract declaration ([`CONTRACT_RULES`] lists the
//! registered `CTR-*`s) and [`RequestEnvelope::conforms`] is the check. An
//! effect-bearing contract without a ticket and an idempotency key is refused
//! *before* the call is dispatched, which is the only moment a refusal is still
//! cheap.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{BoundaryError, ErrorCode, KernelError};
use crate::time::{CancellationToken, Deadline, EpochMillis, SystemClock, now_epoch_millis};
use crate::{CANONICAL_SCHEMA_VERSION, EntityId, TicketId, WorkId};

/// The current instant as an [`EpochMillis`]. The one place the kernel reads
/// the wall clock for a boundary decision.
fn now() -> EpochMillis {
    EpochMillis::from_unix_millis(now_epoch_millis())
}

/// The schema version stamped into every envelope. It is the existing canonical
/// constant, not a second counter: two version numbers on one wire is how a peer
/// cannot tell which one is authoritative.
pub const ENVELOPE_VERSION: u32 = CANONICAL_SCHEMA_VERSION;

/// The default budget for a call that declares no deadline. Generous enough for
/// a local tool call, short enough that a hung call is a bounded wait rather
/// than a stuck run.
pub const DEFAULT_DEADLINE_MS: u64 = 120_000;

/// Who is calling (`ARCH/10-KERNEL.md` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    /// A person, through a surface.
    User,
    /// An agent runtime, native or external.
    Agent,
    /// A workflow run, acting on its own run's authority.
    Workflow,
}

impl ActorKind {
    /// The stable wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
            Self::Workflow => "workflow",
        }
    }

    /// Parse the wire spelling; unknown text is refused rather than guessed, and
    /// `User` is never a default — "we do not know who is calling" is exactly
    /// the state in which a call must not be trusted.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "user" => Some(Self::User),
            "agent" => Some(Self::Agent),
            "workflow" => Some(Self::Workflow),
            _ => None,
        }
    }
}

impl fmt::Display for ActorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who is calling, in what scope, under which permissions.
///
/// A missing field is not a default: [`ActorContext::new`] takes every field, so
/// "a call with no actor context" is unrepresentable rather than a struct with an
/// empty `id` (`ARCH/10-KERNEL.md` §7: "missing actor context → rejected").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorContext {
    /// Which kind of principal this is.
    pub kind: ActorKind,
    /// The principal's id — a user id, an agent id, a workflow run id. Opaque.
    pub id: String,
    /// What the call may act on: paths, targets, resource patterns. A scope
    /// *expression* owned by Trust (`12`), not a capability.
    pub scope: String,
    /// A reference to the permission snapshot this call was admitted under.
    ///
    /// A reference and not the values: the envelope crosses a trust boundary,
    /// and `INV-11` gives an external caller projections only. The Trust owner
    /// resolves it; a snapshot that travelled *inside* the envelope would be a
    /// second, unaudited copy of an authorization decision.
    pub permissions_ref: String,
    /// When the call was admitted, in epoch milliseconds UTC.
    pub at: EpochMillis,
}

impl ActorContext {
    /// An actor context, stamped with the current instant.
    pub fn new(
        kind: ActorKind,
        id: impl Into<String>,
        scope: impl Into<String>,
        permissions_ref: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            id: id.into(),
            scope: scope.into(),
            permissions_ref: permissions_ref.into(),
            at: now(),
        }
    }

    /// Whether the context is complete enough to be admitted. An empty id or an
    /// empty permissions reference is an absent context, however well-formed the
    /// struct looks.
    pub fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
            && !self.scope.trim().is_empty()
            && !self.permissions_ref.trim().is_empty()
    }
}

/// The idempotency key for an effect invocation (`ARCH/07-CONTRACTS.md` §5.4).
///
/// Derived, not random: `work_id` + `ticket` means a retry of *the same* effect
/// produces *the same* key, which is the whole point — a random key per attempt
/// would defeat dedupe on every retry. A random key is what
/// [`IdempotencyKey::mint_detached`] is for, and it is only for a call that
/// genuinely has no ticket.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// The key for one effect: `<work_id>:<ticket_id>`.
    pub fn for_effect(work_id: &WorkId, ticket_id: &TicketId) -> Self {
        Self(format!("{}:{}", work_id.as_str(), ticket_id.as_str()))
    }

    /// A minted key for an effect with no ticket to derive from. Recorded as
    /// `detached:` so a reader can tell it apart from a ticket-derived one — a
    /// bare random key would look like a work id and hide the missing ticket.
    pub fn mint_detached() -> Self {
        let (id, _) = EntityId::mint();
        Self(format!("detached:{id}"))
    }

    /// The key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this key was derived from a ticket.
    pub fn is_ticket_derived(&self) -> bool {
        !self.0.starts_with("detached:")
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The ticket an effect call is authorized by, plus the epoch binding that
/// makes it checkable (`ARCH/07-CONTRACTS.md` §5.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketBinding {
    /// The ticket id.
    pub ticket_id: TicketId,
    /// The provider epoch the ticket was minted against. A handle or ticket from
    /// an earlier epoch is stale, and a stale epoch is `InvalidState`.
    pub provider_epoch: u64,
    /// The environment the ticket is bound to.
    pub environment_id: String,
    /// When the ticket expires, in epoch milliseconds UTC.
    pub expires_at: EpochMillis,
}

impl TicketBinding {
    /// Whether the ticket is still usable against a provider at `epoch` right
    /// now. Both halves matter: a live ticket against a restarted provider is
    /// exactly the stale case `ARCH/13-CAPABILITY.md` §4 describes.
    pub fn is_usable(&self, epoch: u64, now: EpochMillis) -> bool {
        self.provider_epoch == epoch && now.is_at_or_before(self.expires_at)
    }
}

/// A request envelope: everything a `CTR-*` call carries besides its parameters.
#[derive(Debug, Clone)]
pub struct RequestEnvelope<P> {
    /// The schema version of this envelope.
    pub version: u32,
    /// The contract's method name, e.g. `work.step.execute`.
    pub method: String,
    /// Who is calling.
    pub actor: ActorContext,
    /// The contract's parameters.
    pub params: P,
    /// The cooperative cancellation token, shared with every sub-call.
    pub cancel: CancellationToken,
    /// The id this token is registered under on the wire, so a peer can map a
    /// `cancel` message back to the local token.
    pub cancel_token_id: String,
    /// When the call must stop, in epoch milliseconds UTC. `None` means the
    /// contract declared no deadline; [`Self::effective_deadline`] then applies
    /// [`DEFAULT_DEADLINE_MS`], so "no deadline" never means "no bound".
    pub deadline: Option<EpochMillis>,
    /// The idempotency key, for an effect-bearing call.
    pub idempotency: Option<IdempotencyKey>,
    /// The ticket authorizing an effect-bearing call.
    pub ticket: Option<TicketBinding>,
}

impl<P> RequestEnvelope<P> {
    /// Build an envelope. Every field is explicit: there is no `Default`, because
    /// a default envelope is an envelope with no actor and no deadline.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        method: impl Into<String>,
        actor: ActorContext,
        params: P,
        deadline_ms: Option<u64>,
    ) -> Self {
        let (cancel_token_id, _) = EntityId::mint();
        Self {
            version: ENVELOPE_VERSION,
            method: method.into(),
            actor,
            params,
            cancel: CancellationToken::new(),
            cancel_token_id: cancel_token_id.to_string(),
            deadline: deadline_ms.map(|ms| now().saturating_add_millis(ms as i64)),
            idempotency: None,
            ticket: None,
        }
    }

    /// Mark this call as effect-bearing, with the ticket and the key it derives.
    #[must_use]
    pub fn with_ticket(mut self, ticket: TicketBinding, work_id: &WorkId) -> Self {
        self.idempotency = Some(IdempotencyKey::for_effect(work_id, &ticket.ticket_id));
        self.ticket = Some(ticket);
        self
    }

    /// Replace the cancellation token — a resumed call, or one whose token was
    /// reconstructed from a wire `cancel_token`. The token id is kept, so the
    /// wire form is unchanged.
    #[must_use]
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// The deadline, defaulted. Every call has a bound.
    pub fn effective_deadline(&self) -> EpochMillis {
        self.deadline
            .unwrap_or_else(|| now().saturating_add_millis(DEFAULT_DEADLINE_MS as i64))
    }

    /// The monotonic deadline to hand a sub-call. The tighter of this call's
    /// remaining budget and the sub-call's own; a sub-call may not outlive its
    /// caller (`ARCH/07-CONTRACTS.md` §5.3).
    pub fn sub_deadline(&self, budget_ms: u64, label: &'static str) -> Deadline {
        let own = Deadline::after(
            &SystemClock::new(),
            std::time::Duration::from_millis(budget_ms),
            label,
        );
        let inherited = Deadline::after(
            &SystemClock::new(),
            // `now().signed_millis_until(deadline)` is the budget *left* on this
            // call. A call whose deadline has already passed has none left, and
            // its sub-calls inherit that — which is the whole point of
            // propagating a deadline rather than restating it.
            std::time::Duration::from_millis(
                u64::try_from(now().signed_millis_until(self.effective_deadline())).unwrap_or(0),
            ),
            "inherited",
        );
        own.tighten(inherited)
    }

    /// The typed envelope, versioned and actor-bound, for a call whose result is
    /// [`ResultEnvelope`].
    pub fn to_result(&self, result: std::result::Result<P, KernelError>) -> TypedEnvelope<P> {
        TypedEnvelope {
            version: ENVELOPE_VERSION,
            method: self.method.clone(),
            actor: self.actor.clone(),
            result: result.map_or_else(ResultEnvelope::from_kernel, ResultEnvelope::ok),
        }
    }
}

/// Why an envelope was refused before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeViolation {
    /// The envelope's schema version is not one this build speaks.
    UnsupportedVersion {
        /// What the envelope said.
        got: u32,
        /// What this build speaks.
        expected: u32,
    },
    /// No actor context, or an incomplete one.
    MissingActorContext,
    /// An effect-bearing call with no ticket (`ARCH/07-CONTRACTS.md` §5.1).
    MissingTicket,
    /// An effect-bearing call with no idempotency key, so a retry could
    /// double-apply the effect.
    MissingIdempotency,
    /// The ticket is expired, or bound to a provider epoch that has moved on.
    StaleTicket {
        /// The epoch the ticket was minted against.
        ticket_epoch: u64,
        /// The current epoch.
        current_epoch: u64,
    },
    /// A read-only contract carrying a ticket: the call claims authority it does
    /// not need, which is how a read starts looking like an effect.
    UnexpectedTicket,
}

impl fmt::Display for EnvelopeViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion { got, expected } => write!(
                f,
                "the envelope speaks schema v{got}; this build speaks v{expected}"
            ),
            Self::MissingActorContext => {
                f.write_str("a call must carry a complete actor context (kind, id, scope, permissions_ref)")
            }
            Self::MissingTicket => {
                f.write_str("an effect-bearing contract requires a ticket (ARCH/07-CONTRACTS.md §5.1)")
            }
            Self::MissingIdempotency => f.write_str(
                "an effect-bearing contract requires an idempotency key derived from work_id + ticket",
            ),
            Self::StaleTicket {
                ticket_epoch,
                current_epoch,
            } => write!(
                f,
                "the ticket is bound to provider epoch {ticket_epoch} and the provider is at \
                 {current_epoch}: the ticket is stale, and a stale epoch is not retryable"
            ),
            Self::UnexpectedTicket => {
                f.write_str("a read-only contract must not carry a ticket")
            }
        }
    }
}

impl std::error::Error for EnvelopeViolation {}

impl From<EnvelopeViolation> for BoundaryError {
    fn from(violation: EnvelopeViolation) -> Self {
        // A refused envelope is `AuthorizationDenied` for the ticket cases (the
        // call was not authorized) and `InvalidState` for the shape cases (what
        // arrived is not a call this contract accepts). Both are non-retryable:
        // re-sending the same envelope produces the same refusal.
        let code = match violation {
            EnvelopeViolation::MissingTicket
            | EnvelopeViolation::MissingIdempotency
            | EnvelopeViolation::StaleTicket { .. } => ErrorCode::AuthorizationDenied,
            EnvelopeViolation::UnexpectedTicket => ErrorCode::InvalidState,
            EnvelopeViolation::UnsupportedVersion { .. }
            | EnvelopeViolation::MissingActorContext => ErrorCode::InvalidState,
        };
        BoundaryError::new(code, violation.to_string())
    }
}

/// The result envelope (`ARCH/10-KERNEL.md` §7): `{ ok, value } | { error }`.
///
/// Hand-decoded so the two arms cannot be confused. `#[serde(untagged)]` would
/// pick whichever arm happened to match and report a `NotFound` as a success
/// with a missing value; here a document that sets `ok` and `value` and `error`
/// together, or none of them, is a **decode error** — the failure mode this type
/// exists to remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultEnvelope<T> {
    /// The schema version, so an additive field is distinguishable from a
    /// breaking one.
    pub version: u32,
    /// `true` for a value, `false` for an error. Checked against the arm on
    /// decode, so it cannot contradict what is beside it.
    pub ok: bool,
    /// The value, present exactly when `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<T>,
    /// The typed error, present exactly when `!ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<BoundaryError>,
}

impl<T> ResultEnvelope<T> {
    /// A success envelope.
    pub fn ok(value: T) -> Self {
        Self {
            version: ENVELOPE_VERSION,
            ok: true,
            value: Some(value),
            error: None,
        }
    }

    /// A failure envelope.
    pub fn err(error: BoundaryError) -> Self {
        Self {
            version: ENVELOPE_VERSION,
            ok: false,
            value: None,
            error: Some(error),
        }
    }

    /// A failure envelope from an in-process error, via the one conversion that
    /// decides what may leave the process.
    pub fn from_kernel(error: KernelError) -> Self {
        Self::err(error.into_boundary())
    }

    /// The value, if this is a success. `Err` carries the typed error.
    pub fn into_result(self) -> std::result::Result<T, BoundaryError> {
        match (self.ok, self.value, self.error) {
            (true, Some(value), None) => Ok(value),
            (false, None, Some(error)) => Err(error),
            _ => Err(BoundaryError::new(
                ErrorCode::Internal,
                "the result envelope contradicts itself",
            )),
        }
    }

    /// Whether this is a success.
    pub fn is_ok(&self) -> bool {
        self.ok
    }
}

/// The whole reply: the result envelope plus the actor and method it answers, so
/// a response can be attributed and correlated without a second message.
///
/// The wire form is **flat** — `{"version","method","actor","ok","value"|"error"}` —
/// and both directions are hand-written rather than `#[serde(flatten)]`. Flatten
/// would deserialize the result arm through serde's own struct visitor, which
/// would accept the contradictory documents [`decode_result`] exists to refuse:
/// the derive would silently pick an arm where the hand-written decoder errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedEnvelope<T> {
    /// The schema version.
    pub version: u32,
    /// The method this answers.
    pub method: String,
    /// The actor the call was made as.
    pub actor: ActorContext,
    /// The result.
    pub result: ResultEnvelope<T>,
}

impl<T: Serialize> Serialize for TypedEnvelope<T> {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = ser.serialize_map(Some(5))?;
        map.serialize_entry("version", &self.result.version)?;
        map.serialize_entry("method", &self.method)?;
        map.serialize_entry("actor", &self.actor)?;
        map.serialize_entry("ok", &self.result.ok)?;
        match &self.result.error {
            Some(error) => map.serialize_entry("error", error)?,
            None => {
                if let Some(value) = &self.result.value {
                    map.serialize_entry("value", value)?;
                }
            }
        }
        map.end()
    }
}

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for TypedEnvelope<T> {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = serde_json::Value::deserialize(de)?;
        let object = raw
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("a typed envelope is not an object"))?;
        let bad = |what: &str| serde::de::Error::custom(what.to_string());
        let version = object
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| bad("a typed envelope carries no schema version"))?
            as u32;
        let method = object
            .get("method")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| bad("a typed envelope names no method"))?
            .to_string();
        let actor: ActorContext = serde_json::from_value(
            object
                .get("actor")
                .cloned()
                .ok_or_else(|| bad("a typed envelope carries no actor context"))?,
        )
        .map_err(|err| bad(&format!("the actor context is unreadable: {err}")))?;

        // Lift the result arm out and hand it to the single-arm decoder, so both
        // entry points enforce the same rule.
        let mut arm = serde_json::Map::new();
        for key in ["version", "ok", "value", "error"] {
            if let Some(value) = object.get(key) {
                arm.insert(key.to_string(), value.clone());
            }
        }
        let arm = serde_json::Value::Object(arm);
        let result = ResultEnvelope::<T>::decode_value(arm).map_err(|err| bad(&err.message))?;
        // The arm's own version is the authoritative one for the result; the
        // outer field must agree with it, or the document is describing two
        // different schema versions at once.
        if result.version != version {
            return Err(bad(&format!(
                "a typed envelope claims v{version} and its result claims v{}",
                result.version
            )));
        }
        Ok(Self {
            version,
            method,
            actor,
            result,
        })
    }
}

/// Whether a contract can cause an externally visible effect
/// (`ARCH/07-CONTRACTS.md` §0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectBearing {
    /// No externally visible effect: a read or a projection. No ticket, and a
    /// ticket on a read is itself a violation.
    ReadOnly,
    /// An externally visible effect: a ticket is required, plus an idempotency
    /// key so a retry cannot double-apply.
    Effect,
}

/// Whether a provider honours an idempotency key for dedupe
/// (`ARCH/10-KERNEL.md` §7: "where providers support dedupe").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupeSupport {
    /// The provider dedupes on the key: a repeat returns the first result.
    Native,
    /// The provider has no dedupe, so the *owner* must. The key is still sent:
    /// recording it is what lets the owner recognise its own retry.
    OwnerMustDedupe,
}

/// One contract's envelope requirements, as declared in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContractRule {
    /// The contract's registry id, `CTR-###`.
    pub ctr: &'static str,
    /// The contract's name.
    pub name: &'static str,
    /// The owning module's `ARCH/NN` number.
    pub owner: &'static str,
    /// Whether the contract can cause an effect.
    pub effect: EffectBearing,
    /// Whether the owner dedupes on the idempotency key.
    pub dedupe: DedupeSupport,
}

impl ContractRule {
    /// Whether this contract requires a ticket.
    pub const fn requires_ticket(&self) -> bool {
        matches!(self.effect, EffectBearing::Effect)
    }
}

/// The registered contracts, transcribed from `ARCH/07-CONTRACTS.md` §1.
///
/// This is data, not logic: the rules a call must satisfy come from the two
/// columns above, and the table only says *which* rule applies to *which*
/// contract. A contract added to the registry with no row here is a contract
/// nobody checked, which is what `contracts_are_all_registered` in the tests
/// looks for.
pub const CONTRACT_RULES: [ContractRule; 26] = [
    rule("CTR-001", "AgentEngine", "15", EffectBearing::Effect),
    rule("CTR-002", "AgentSession", "15", EffectBearing::Effect),
    rule("CTR-003", "WorkService", "11", EffectBearing::Effect),
    rule("CTR-004", "SessionLog", "11", EffectBearing::Effect),
    rule("CTR-005", "CheckpointService", "16", EffectBearing::Effect),
    rule("CTR-006", "ContextProvider", "16", EffectBearing::ReadOnly),
    rule("CTR-007", "ContextController", "15", EffectBearing::Effect),
    rule("CTR-008", "MemoryService", "17", EffectBearing::Effect),
    rule("CTR-009", "CapabilityBroker", "13", EffectBearing::ReadOnly),
    rule("CTR-010", "ProviderAdapter", "14", EffectBearing::Effect),
    rule("CTR-011", "Guard", "12", EffectBearing::ReadOnly),
    rule("CTR-012", "ApprovalService", "12", EffectBearing::Effect),
    rule("CTR-013", "Vault", "12", EffectBearing::ReadOnly),
    rule("CTR-014", "ModelRouter", "18", EffectBearing::ReadOnly),
    rule("CTR-015", "EnvironmentService", "19", EffectBearing::Effect),
    rule("CTR-016", "WorkflowEngine", "20", EffectBearing::Effect),
    rule("CTR-017", "WorldService", "21", EffectBearing::ReadOnly),
    rule("CTR-018", "ArtifactService", "29", EffectBearing::Effect),
    rule("CTR-019", "EventStore", "30", EffectBearing::Effect),
    rule("CTR-020", "SkillResolver", "31", EffectBearing::ReadOnly),
    rule("CTR-021", "DelegationService", "15", EffectBearing::Effect),
    rule("CTR-022", "AgentGateway", "32", EffectBearing::ReadOnly),
    rule("CTR-023", "EffectVerifier", "34", EffectBearing::Effect),
    rule("CTR-024", "FileIdentity", "25", EffectBearing::ReadOnly),
    rule("CTR-025", "RepoIntelligence", "26", EffectBearing::ReadOnly),
    rule("CTR-026", "Scheduler", "11", EffectBearing::Effect),
];

const fn rule(
    ctr: &'static str,
    name: &'static str,
    owner: &'static str,
    effect: EffectBearing,
) -> ContractRule {
    ContractRule {
        ctr,
        name,
        owner,
        effect,
        // Default: where the provider cannot dedupe, the owner must. A contract
        // that *does* get native dedupe says so explicitly.
        dedupe: DedupeSupport::OwnerMustDedupe,
    }
}

/// Look up a contract's rule by its registry id.
pub fn rule_for(ctr: &str) -> Option<&'static ContractRule> {
    CONTRACT_RULES.iter().find(|rule| rule.ctr == ctr)
}

impl<P> RequestEnvelope<P> {
    /// Whether this envelope satisfies `rule`, checked against the provider
    /// epoch currently in force.
    ///
    /// Run **before** dispatch: after it, the effect is already attempted and a
    /// refusal is a partial completion rather than a clean rejection.
    pub fn conforms(
        &self,
        rule: &ContractRule,
        current_provider_epoch: u64,
    ) -> Result<(), EnvelopeViolation> {
        if self.version != ENVELOPE_VERSION {
            return Err(EnvelopeViolation::UnsupportedVersion {
                got: self.version,
                expected: ENVELOPE_VERSION,
            });
        }
        if !self.actor.is_complete() {
            return Err(EnvelopeViolation::MissingActorContext);
        }
        match rule.effect {
            EffectBearing::ReadOnly => {
                if self.ticket.is_some() {
                    return Err(EnvelopeViolation::UnexpectedTicket);
                }
            }
            EffectBearing::Effect => {
                let Some(ticket) = &self.ticket else {
                    return Err(EnvelopeViolation::MissingTicket);
                };
                if !ticket.is_usable(current_provider_epoch, now()) {
                    return Err(EnvelopeViolation::StaleTicket {
                        ticket_epoch: ticket.provider_epoch,
                        current_epoch: current_provider_epoch,
                    });
                }
                if self.idempotency.is_none() {
                    return Err(EnvelopeViolation::MissingIdempotency);
                }
            }
        }
        Ok(())
    }

    /// Conform, as a typed boundary error — the shape a contract actually
    /// returns.
    pub fn boundary_conforms(
        &self,
        rule: &ContractRule,
        current_provider_epoch: u64,
    ) -> Result<(), BoundaryError> {
        self.conforms(rule, current_provider_epoch)
            .map_err(BoundaryError::from)
    }
}

impl<P> RequestEnvelope<P>
where
    P: Serialize,
{
    /// The wire projection: the envelope without the live cancellation token,
    /// which is a local object. `cancel_token` carries the id the peer uses to
    /// send `cancel` back, and `deadline_ms` is the absolute instant rather than
    /// a remaining budget — a remaining budget is meaningless to a peer whose
    /// clock started later.
    pub fn to_wire(&self) -> Result<WireEnvelope, KernelError> {
        Ok(WireEnvelope {
            version: self.version,
            method: self.method.clone(),
            actor: self.actor.clone(),
            params: serde_json::to_value(&self.params).map_err(KernelError::from)?,
            deadline_ms: self.deadline.map(EpochMillis::unix_millis),
            cancel_token: Some(self.cancel_token_id.clone()),
            idempotency_key: self.idempotency.as_ref().map(|key| key.to_string()),
            ticket: self.ticket.clone(),
        })
    }
}

/// The wire form of a request envelope. Serializable, and free of anything
/// local.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireEnvelope {
    /// The schema version.
    pub version: u32,
    /// The method.
    pub method: String,
    /// The actor.
    pub actor: ActorContext,
    /// The parameters, as a canonical JSON value.
    pub params: serde_json::Value,
    /// The absolute deadline in epoch milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<i64>,
    /// The id the peer's `cancel` message refers to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_token: Option<String>,
    /// The idempotency key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// The ticket binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<TicketBinding>,
}

impl WireEnvelope {
    /// Rehydrate into a local envelope, adopting the peer's cancellation token id
    /// and a live token this process controls.
    pub fn into_local<P: serde::de::DeserializeOwned>(
        self,
    ) -> Result<RequestEnvelope<P>, BoundaryError> {
        let params: P = serde_json::from_value(self.params).map_err(|err| {
            BoundaryError::new(
                ErrorCode::InvalidState,
                format!("the envelope's params do not match the contract: {err}"),
            )
        })?;
        let cancel_token_id = self.cancel_token.unwrap_or_else(|| {
            let (id, _) = EntityId::mint();
            id.to_string()
        });
        Ok(RequestEnvelope {
            version: self.version,
            method: self.method,
            actor: self.actor,
            params,
            cancel: CancellationToken::new(),
            cancel_token_id,
            deadline: self.deadline_ms.map(EpochMillis::from_unix_millis),
            idempotency: self.idempotency_key.map(IdempotencyKey),
            ticket: self.ticket,
        })
    }
}

/// Decode a result envelope, refusing a document that contradicts itself.
///
/// This is the whole point of the hand-written decode: a peer that sends
/// `{ "ok": true, "value": null, "error": {...} }` gets a *decode error*, not a
/// success with a null value.
pub fn decode_result<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
) -> std::result::Result<ResultEnvelope<T>, BoundaryError> {
    let raw: serde_json::Value = serde_json::from_slice(bytes).map_err(|err| {
        BoundaryError::new(
            ErrorCode::InvalidState,
            format!("a result envelope is not valid json: {err}"),
        )
    })?;
    ResultEnvelope::<T>::decode_value(raw)
}

impl<T: serde::de::DeserializeOwned> ResultEnvelope<T> {
    /// The decoding half of the union, over an already-parsed value.
    ///
    /// The schema version is read and carried, not enforced: a version this build
    /// does not speak is the *caller's* policy (`RequestEnvelope::conforms` on the
    /// way in, a peer's compatibility rule on the way out). What this function
    /// does enforce, unconditionally, is that the document names **one** arm.
    pub fn decode_value(raw: serde_json::Value) -> std::result::Result<Self, BoundaryError> {
        let object = raw.as_object().ok_or_else(|| {
            BoundaryError::new(
                ErrorCode::InvalidState,
                "a result envelope is not an object",
            )
        })?;
        let bad = |what: &str| {
            BoundaryError::new(
                ErrorCode::InvalidState,
                format!("a result envelope cannot be {what}"),
            )
        };
        let version = object
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| bad("read: no schema version"))? as u32;
        let ok = object
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| bad("read: no ok flag"))?;
        // A `value` key that is present but null is an *absent* value: a success
        // whose value is legitimately `null` says so with a value type that can
        // hold it, not by leaving the key null.
        let has_value = object.get("value").is_some_and(|value| !value.is_null());
        let error = match object.get("error") {
            None | Some(serde_json::Value::Null) => None,
            Some(raw) => Some(
                serde_json::from_value::<BoundaryError>(raw.clone())
                    .map_err(|err| bad(&format!("read: {err}")))?,
            ),
        };
        match (ok, has_value, error) {
            (true, true, None) => {
                let value = serde_json::from_value(object["value"].clone())
                    .map_err(|err| bad(&format!("read: {err}")))?;
                Ok(Self {
                    version,
                    ..Self::ok(value)
                })
            }
            (false, false, Some(error)) => Ok(Self {
                version,
                ..Self::err(error)
            }),
            (true, _, Some(_)) => Err(bad(
                "both a value and an error: an envelope that can be read two ways has a \
                 success with no meaning",
            )),
            (true, false, None) => Err(bad("a success with no value")),
            (false, true, _) => Err(bad("a failure with a value")),
            (false, false, None) => Err(bad("neither a value nor an error")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MonotonicClock;
    use std::time::Duration;

    fn actor() -> ActorContext {
        ActorContext::new(ActorKind::User, "u-1", "workspace:ws-1", "perm-snap:abc")
    }

    fn ticket() -> TicketBinding {
        TicketBinding {
            ticket_id: TicketId::new("0193f0a0-0000-7000-8000-000000000001"),
            provider_epoch: 3,
            environment_id: "env-local".into(),
            expires_at: now().saturating_add_millis(60_000),
        }
    }

    fn work() -> WorkId {
        WorkId::new("0193f0a0-0000-7000-8000-0000000000ff")
    }

    fn effect_envelope() -> RequestEnvelope<serde_json::Value> {
        RequestEnvelope::new(
            "work.step.execute",
            actor(),
            serde_json::json!({"step": 1}),
            None,
        )
        .with_ticket(ticket(), &work())
    }

    #[test]
    fn a_result_envelope_has_exactly_one_arm() {
        let ok: ResultEnvelope<u32> = ResultEnvelope::ok(7);
        assert!(ok.is_ok());
        assert_eq!(ok.clone().into_result().unwrap(), 7);
        let json = serde_json::to_string(&ok).unwrap();
        assert!(json.contains("\"ok\":true"), "{json}");
        assert!(json.contains("\"value\":7"), "{json}");
        assert!(!json.contains("error"), "{json}");

        let err: ResultEnvelope<u32> = ResultEnvelope::from_kernel(
            KernelError::not_found("work").with_diagnostic("work_gateway.rs:412"),
        );
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"ok\":false"), "{json}");
        assert!(json.contains("\"code\":\"NotFound\""), "{json}");
        assert!(
            !json.contains("work_gateway"),
            "the diagnostic stays put: {json}"
        );
        let back: ResultEnvelope<u32> = decode_result(json.as_bytes()).expect("decodes");
        assert!(!back.is_ok());
    }

    #[test]
    fn a_contradictory_result_envelope_is_a_decode_error_not_a_coercion() {
        // Both arms present. `untagged` would have picked the value; here it is
        // refused, because a success that also carries an error is not a fact.
        let raw = serde_json::json!({
            "version": ENVELOPE_VERSION,
            "ok": true,
            "value": 1,
            "error": {"code": "NotFound", "message": "gone", "retryable": false}
        });
        let err = ResultEnvelope::<u32>::decode_value(raw).expect_err("refused");
        assert!(err.message.contains("both a value and an error"), "{err}");

        // Neither arm.
        let raw = serde_json::json!({"version": ENVELOPE_VERSION, "ok": true});
        assert!(ResultEnvelope::<u32>::decode_value(raw).is_err());
        let raw = serde_json::json!({"version": ENVELOPE_VERSION, "ok": false});
        assert!(ResultEnvelope::<u32>::decode_value(raw).is_err());

        // A success flag with no value, and a failure with a value.
        let raw = serde_json::json!({"version": ENVELOPE_VERSION, "ok": true, "value": null});
        assert!(ResultEnvelope::<u32>::decode_value(raw).is_err());
        let raw = serde_json::json!({"version": ENVELOPE_VERSION, "ok": false, "value": 3});
        assert!(ResultEnvelope::<u32>::decode_value(raw).is_err());

        // No `ok` at all: an envelope without an arm selector is not an envelope.
        let raw = serde_json::json!({"version": ENVELOPE_VERSION, "value": 1});
        assert!(ResultEnvelope::<u32>::decode_value(raw).is_err());
        // Not an object.
        assert!(ResultEnvelope::<u32>::decode_value(serde_json::json!([])).is_err());
        // Not JSON.
        assert!(decode_result::<u32>(b"{oops").is_err());
    }

    #[test]
    fn a_guidance_result_arrives_as_a_result_with_next_steps() {
        // The spec's line: guidance and a needed decision are *results*, not
        // failures. The envelope carries them as a typed error arm with steps,
        // and the steps survive the wire.
        let guidance = ResultEnvelope::<u32>::from_kernel(KernelError::guidance(
            "the drive connector is not connected",
            vec![crate::error::NextStep::at(
                "Connect Google Drive",
                "settings/connectors/drive",
            )],
        ));
        let json = serde_json::to_string(&guidance).unwrap();
        assert!(json.contains("GuidanceRequired"), "{json}");
        assert!(json.contains("Connect Google Drive"), "{json}");
        let back: ResultEnvelope<u32> = decode_result(json.as_bytes()).expect("decodes");
        match back.into_result() {
            Err(error) => {
                assert_eq!(error.code, ErrorCode::GuidanceRequired);
                assert_eq!(error.next_steps.len(), 1);
                assert!(!error.retryable, "guidance is not a retry");
            }
            Ok(value) => panic!("guidance must not decode as a value: {value}"),
        }
    }

    #[test]
    fn an_effect_call_without_a_ticket_is_refused_before_dispatch() {
        let rule = rule_for("CTR-010").expect("registered");
        assert!(rule.requires_ticket(), "ProviderAdapter is effect-bearing");
        let envelope = RequestEnvelope::new("provider/call", actor(), serde_json::json!({}), None);
        assert_eq!(
            envelope.conforms(rule, 3),
            Err(EnvelopeViolation::MissingTicket)
        );
        // …as a typed boundary error, with the spec's code.
        let err = envelope.boundary_conforms(rule, 3).expect_err("refused");
        assert_eq!(err.code, ErrorCode::AuthorizationDenied);
        assert!(!err.retryable, "the same envelope is refused the same way");
        assert!(err.message.contains("§5.1"), "{err}");

        // With a ticket it passes, and the key is derived, not random.
        let envelope = effect_envelope();
        assert!(envelope.conforms(rule, 3).is_ok());
        let key = envelope.idempotency.as_ref().expect("key");
        assert!(key.is_ticket_derived());
        assert_eq!(key.as_str(), format!("{}:{}", work(), ticket().ticket_id));
    }

    #[test]
    fn a_ticket_from_a_previous_epoch_is_stale_and_not_retryable() {
        let rule = rule_for("CTR-010").expect("registered");
        let envelope = effect_envelope();
        // The provider restarted: the epoch moved, the ticket did not.
        let err = envelope
            .boundary_conforms(rule, 4)
            .expect_err("a stale epoch is refused");
        assert_eq!(err.code, ErrorCode::AuthorizationDenied);
        assert!(
            !err.retryable,
            "a stale ticket does not become valid on retry"
        );
        assert!(err.message.contains("stale"), "{err}");

        // An expired ticket is the same refusal.
        let mut expired = effect_envelope();
        expired.ticket.as_mut().expect("ticket").expires_at = now().saturating_add_millis(-1);
        assert!(matches!(
            expired.conforms(rule, 3),
            Err(EnvelopeViolation::StaleTicket { .. })
        ));
    }

    #[test]
    fn a_read_only_contract_must_not_carry_a_ticket() {
        let rule = rule_for("CTR-014").expect("registered");
        assert!(!rule.requires_ticket(), "ModelRouter is a read");
        // Plain read: fine.
        let read = RequestEnvelope::new("model/route", actor(), serde_json::json!({}), None);
        assert!(read.conforms(rule, 3).is_ok());
        // The same read, claiming authority it does not need.
        let over = read.clone().with_ticket(ticket(), &work());
        assert_eq!(
            over.conforms(rule, 3),
            Err(EnvelopeViolation::UnexpectedTicket)
        );
    }

    #[test]
    fn a_call_without_actor_context_is_rejected() {
        let rule = rule_for("CTR-006").expect("registered");
        let mut envelope =
            RequestEnvelope::new("context/search", actor(), serde_json::json!({}), None);
        assert!(envelope.conforms(rule, 3).is_ok());
        // An empty id is an absent context, however well-formed the struct is.
        envelope.actor.id = String::new();
        assert_eq!(
            envelope.conforms(rule, 3),
            Err(EnvelopeViolation::MissingActorContext)
        );
        envelope.actor.id = "u-1".into();
        envelope.actor.permissions_ref = "  ".into();
        assert_eq!(
            envelope.conforms(rule, 3),
            Err(EnvelopeViolation::MissingActorContext)
        );
    }

    #[test]
    fn an_unknown_schema_version_is_refused() {
        let rule = rule_for("CTR-006").expect("registered");
        let mut envelope =
            RequestEnvelope::new("context/search", actor(), serde_json::json!({}), None);
        envelope.version = ENVELOPE_VERSION + 1;
        assert_eq!(
            envelope.conforms(rule, 3),
            Err(EnvelopeViolation::UnsupportedVersion {
                got: ENVELOPE_VERSION + 1,
                expected: ENVELOPE_VERSION,
            })
        );
    }

    #[test]
    fn a_deadline_propagates_into_a_sub_call_and_never_widens() {
        // The caller's own budget is 5s. Every comparison is against the clock
        // read *now*: an absolute millisecond would make this a test of how
        // loaded the machine is.
        let clock = SystemClock::new();
        let envelope = RequestEnvelope::new(
            "work.step.execute",
            actor(),
            serde_json::json!({}),
            Some(5_000),
        );

        // A sub-call that asks for less than the caller has left keeps its own.
        let at = clock.monotonic_millis();
        let tight = envelope.sub_deadline(1_000, "inner");
        assert!(
            tight.at_monotonic_ms() <= at + 1_000,
            "a tighter request is granted as asked"
        );

        // A sub-call that asks for more than the caller has left is tightened to
        // the caller's remaining budget — not granted.
        let at = clock.monotonic_millis();
        let loose = envelope.sub_deadline(600_000, "inner");
        assert!(
            loose.at_monotonic_ms() <= at + 5_000,
            "a wider request is tightened to the caller's budget, not granted"
        );
        assert!(
            loose.at_monotonic_ms() < at + 600_000,
            "and it is genuinely shorter than what was asked for"
        );

        // Every call has a bound, even one that declared no deadline.
        let at = clock.monotonic_millis();
        let unbounded =
            RequestEnvelope::new("work.step.execute", actor(), serde_json::json!({}), None);
        assert!(
            unbounded.sub_deadline(10_000, "inner").at_monotonic_ms()
                <= at + DEFAULT_DEADLINE_MS as i64
        );
        // …and a sub-request under that default keeps its own budget: the
        // default is a ceiling, not a floor.
        assert!(unbounded.sub_deadline(10_000, "inner").at_monotonic_ms() <= at + 10_000);
    }

    #[test]
    fn cancellation_is_cooperative_and_shared_with_sub_calls() {
        let envelope = effect_envelope();
        let child = envelope.cancel.clone();
        assert!(!child.is_cancelled());
        envelope.cancel.cancel("the user pressed stop");
        assert!(child.is_cancelled(), "a sub-call sees the same token");
        // A sub-call that tightens the deadline cannot widen it afterwards. The
        // comparison is against the clock *now*, not an absolute millisecond: an
        // absolute value would make the test fail on a loaded machine rather
        // than on a real regression.
        let clock = SystemClock::new();
        let before = clock.monotonic_millis();
        child.with_deadline(envelope.sub_deadline(1_000, "inner"));
        let wider = envelope.sub_deadline(600_000, "inner");
        child.with_deadline(wider);
        let final_deadline = child.deadline().expect("a deadline");
        let after = clock.monotonic_millis();
        assert!(
            final_deadline.at_monotonic_ms() <= after + 1_000,
            "a wider request must not widen the deadline"
        );
        assert!(final_deadline.at_monotonic_ms() > before);
        // …and the wait ends on the cancellation, not on the deadline.
        let reason = child.wait_for_stop(&clock, Duration::from_millis(20));
        assert_eq!(reason.as_str(), "cancelled");
    }

    #[test]
    fn the_wire_form_round_trips_and_carries_no_live_token() {
        let envelope = effect_envelope();
        let wire = envelope.to_wire().expect("serializable");
        let json = serde_json::to_string(&wire).unwrap();
        // The canonical shape of ARCH/10-KERNEL.md §7 is present.
        for field in [
            "\"version\"",
            "\"actor\"",
            "\"kind\":\"user\"",
            "\"scope\"",
            "\"permissions_ref\"",
            "\"idempotency_key\"",
            "\"cancel_token\"",
            "\"ticket\"",
        ] {
            assert!(json.contains(field), "{field} missing from {json}");
        }
        let back: WireEnvelope = serde_json::from_str(&json).expect("decodes");
        assert_eq!(back, wire);
        let local: RequestEnvelope<serde_json::Value> = back.into_local().expect("rehydrates");
        // The peer's token id is adopted, so a `cancel` message routes back to
        // this process's token; the token itself is local and is not on the wire.
        assert_eq!(local.cancel_token_id, wire.cancel_token.unwrap());
        assert!(!local.cancel.is_cancelled());
        assert_eq!(local.idempotency, envelope.idempotency);
    }

    #[test]
    fn every_registered_contract_declares_its_envelope_rules() {
        // The audit: 26 contracts, unique ids, every one classified, and the
        // effect-bearing ones are exactly the ones that demand a ticket.
        assert_eq!(CONTRACT_RULES.len(), 26);
        let mut ids: Vec<&str> = CONTRACT_RULES.iter().map(|rule| rule.ctr).collect();
        ids.sort_unstable();
        let unique = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), unique, "a duplicate CTR id is a fork");
        for rule in &CONTRACT_RULES {
            assert!(rule.ctr.starts_with("CTR-"), "{}", rule.ctr);
            assert!(!rule.name.is_empty(), "{}", rule.ctr);
            assert!(
                rule.owner.chars().all(|c| c.is_ascii_digit()),
                "{} names an ARCH module",
                rule.ctr
            );
            assert!(rule_for(rule.ctr).is_some());
            match rule.effect {
                EffectBearing::Effect => {
                    // An effect-bearing contract with a read-only classification
                    // is the failure this pins: a ticket-less effect.
                    assert!(rule.requires_ticket());
                }
                EffectBearing::ReadOnly => assert!(!rule.requires_ticket()),
            }
        }
        // The three work-owned contracts the kernel envelope names explicitly.
        for ctr in ["CTR-003", "CTR-004", "CTR-026"] {
            assert!(
                rule_for(ctr).is_some(),
                "{ctr} is named in ARCH/10-KERNEL.md §7"
            );
        }
    }

    #[test]
    fn a_duplicate_effect_call_with_the_same_key_is_recognisable() {
        // The dedupe acceptance: the same (work, ticket) produces the same key on
        // every attempt, so a provider with native dedupe — or an owner keeping
        // its own ledger — can collapse the retry.
        let rule = rule_for("CTR-010").expect("registered");
        let first = effect_envelope();
        let retry = effect_envelope();
        assert_eq!(first.idempotency, retry.idempotency);
        assert!(first.conforms(rule, 3).is_ok());
        assert!(retry.conforms(rule, 3).is_ok());
        // A detached key is visibly different, so a missing ticket cannot hide.
        assert!(!IdempotencyKey::mint_detached().is_ticket_derived());
        assert_ne!(
            IdempotencyKey::for_effect(&work(), &ticket().ticket_id),
            IdempotencyKey::for_effect(
                &WorkId::new("0193f0a0-0000-7000-8000-0000000000fe"),
                &ticket().ticket_id
            )
        );
    }

    #[test]
    fn a_typed_envelope_attributes_the_reply_to_its_call() {
        let envelope = effect_envelope();
        let ok = envelope.to_result(Ok(serde_json::json!({"receipt": "r-1"})));
        let json = serde_json::to_string(&ok).unwrap();
        assert!(json.contains("\"method\":\"work.step.execute\""), "{json}");
        assert!(json.contains("\"actor\""), "{json}");
        assert!(json.contains("\"receipt\":\"r-1\""), "{json}");
        let back: TypedEnvelope<serde_json::Value> = serde_json::from_str(&json).unwrap();
        // Compared against the envelope's *own* actor, not a freshly built one:
        // `ActorContext::new` stamps `at` with the clock, so a second call lands
        // a millisecond later and the comparison would be a clock test, not an
        // attribution test.
        assert_eq!(back.actor, envelope.actor);
        assert_eq!(back.method, envelope.method);
        assert!(back.result.is_ok());
    }

    #[test]
    fn actor_kinds_round_trip_and_an_unknown_kind_is_refused() {
        for kind in [ActorKind::User, ActorKind::Agent, ActorKind::Workflow] {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            assert_eq!(ActorKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(ActorKind::parse("system"), None);
    }
}
