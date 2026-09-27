//! P71.10 (ADR-0008) — canonical workbench-projection and resource-lease schema.
//!
//! This module owns the **shape** of `SessionWorkbenchProjection`, `LensState`,
//! `ResourceLease` and the safe `LeaseAttachment` view. It performs no effect,
//! holds no bearer material, and persists nothing: runtimes that execute or
//! store these records live in their owning crates (the fencing coordinator is
//! `agentcowork-core::workbench`; the event/receipt spine stays the truth).
//!
//! Privacy rule (ADR-0008 §1.3, §2.1): no type in this module has a field for
//! bearer material — no bearer, token, cookie, credential value, or raw lease
//! secret. [`LeaseAttachment`] is the only lease shape that may cross IPC, be
//! rendered, or be written to an event payload or log: it carries the opaque
//! non-secret id, generation, fence, and status. [`FORBIDDEN_PROJECTION_KEYS`]
//! plus [`projection_json_has_no_secrets`] lock that boundary in tests.

use serde::{Deserialize, Serialize};

use crate::{
    AgentBindingId, BindingLifecycle, CANONICAL_SCHEMA_VERSION, ReceiptId, RunId, SessionId,
    SessionKind, WorkId, WorkState,
};

/// Schema version stamped into every [`SessionWorkbenchProjection`].
/// Additive fields keep the number; breaking changes bump it alongside
/// [`CANONICAL_SCHEMA_VERSION`].
pub const WORKBENCH_SCHEMA_VERSION: u32 = CANONICAL_SCHEMA_VERSION;

/// Canonical lease-lifecycle event kinds. These ride the existing
/// `EventEnvelope` stream (`ARCH/WORK.md` §5) — a lease never gets its own
/// event log (ADR-0008 §5, decision 5).
pub const LEASE_EVENT_ACQUIRED: &str = "lease.acquired";
pub const LEASE_EVENT_RENEWED: &str = "lease.renewed";
pub const LEASE_EVENT_RELEASED: &str = "lease.released";
pub const LEASE_EVENT_REVOKED: &str = "lease.revoked";
pub const LEASE_EVENT_EXPIRED: &str = "lease.expired";
pub const LEASE_EVENT_RECLAIM_PENDING: &str = "lease.reclaim_pending";
pub const LEASE_EVENT_RECLAIMED: &str = "lease.reclaimed";
pub const LEASE_EVENT_UNCERTAIN: &str = "lease.uncertain";
pub const LEASE_EVENT_ACTOR_REVALIDATED: &str = "lease.actor_revalidated";

/// Every lease-lifecycle fact kind the coordinator may emit or replay.
pub const LEASE_EVENT_KINDS: &[&str] = &[
    LEASE_EVENT_ACQUIRED,
    LEASE_EVENT_RENEWED,
    LEASE_EVENT_RELEASED,
    LEASE_EVENT_REVOKED,
    LEASE_EVENT_EXPIRED,
    LEASE_EVENT_RECLAIM_PENDING,
    LEASE_EVENT_RECLAIMED,
    LEASE_EVENT_UNCERTAIN,
    LEASE_EVENT_ACTOR_REVALIDATED,
];

/// Substrings that must never appear (case-insensitively) in a serialized
/// projection, attachment, lease-lifecycle payload, error, or log line.
/// Possession material stays in the Rust-private owner; these keys are the
/// tripwire that proves it.
pub const FORBIDDEN_PROJECTION_KEYS: &[&str] = &[
    "bearer",
    "token",
    "cookie",
    "secret",
    "credential",
    "password",
    "api_key",
    "apikey",
    "private_key",
    "session_key",
];

/// Scan a serialized surface for secret-shaped keys. Case-insensitive.
/// Used by schema, coordinator, and adversarial IPC/log-capture tests.
pub fn projection_json_has_no_secrets(rendered: &str) -> bool {
    let lower = rendered.to_ascii_lowercase();
    FORBIDDEN_PROJECTION_KEYS
        .iter()
        .all(|key| !lower.contains(key))
}

// ────────────────────────────────────────────────────────────────────────
// Access classes (ADR-0008 §2.1, §2.3)
// ────────────────────────────────────────────────────────────────────────

/// What a lease holder may do with the resource generation it observed.
/// Observation/view leases may coexist; edit/action/control leases are
/// exclusive against every incompatible lease on the same resource key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseAccess {
    Observe,
    View,
    Edit,
    Action,
    Control,
}

impl LeaseAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::View => "view",
            Self::Edit => "edit",
            Self::Action => "action",
            Self::Control => "control",
        }
    }

    /// Parse the canonical wire spelling. Unknown spellings fail closed to
    /// `None` — an access class is never guessed.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "observe" => Some(Self::Observe),
            "view" => Some(Self::View),
            "edit" => Some(Self::Edit),
            "action" => Some(Self::Action),
            "control" => Some(Self::Control),
            _ => None,
        }
    }

    /// Observation classes may share a resource generation.
    pub fn is_observation(self) -> bool {
        matches!(self, Self::Observe | Self::View)
    }

    /// Edit/action/control leases are exclusive.
    pub fn is_exclusive(self) -> bool {
        !self.is_observation()
    }

    /// Two leases may coexist only when both are observations.
    pub fn compatible_with(self, other: Self) -> bool {
        self.is_observation() && other.is_observation()
    }
}

// ────────────────────────────────────────────────────────────────────────
// Lease lifecycle (ADR-0008 §2.1, §2.3 rules 5–6; RECOVERY.md §6.1)
// ────────────────────────────────────────────────────────────────────────

/// Work/Run concurrency state of one lease. Terminal states are one-way;
/// `Uncertain` and `ReclaimPending` are durable facts, never silent frees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    Active,
    Released,
    Expired,
    Revoked,
    ReclaimPending,
    Reclaimed,
    Uncertain,
}

impl LeaseState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Released => "released",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
            Self::ReclaimPending => "reclaim_pending",
            Self::Reclaimed => "reclaimed",
            Self::Uncertain => "uncertain",
        }
    }

    /// Strict parse: unknown spellings are `None`, never a live state.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "released" => Some(Self::Released),
            "expired" => Some(Self::Expired),
            "revoked" => Some(Self::Revoked),
            "reclaim_pending" => Some(Self::ReclaimPending),
            "reclaimed" => Some(Self::Reclaimed),
            "uncertain" => Some(Self::Uncertain),
            _ => None,
        }
    }

    /// Only [`Self::Active`] may exercise a resource. `ReclaimPending` is
    /// held-but-unverified: the holder must not act until reconciliation.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Active)
    }

    /// One-way terminal facts: release, expiry, revocation, reclaim.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Released | Self::Expired | Self::Revoked | Self::Reclaimed
        )
    }

    /// Whether a lifecycle transition is legal. Terminal facts are one-way
    /// (a repeated terminal fact replays idempotently). `Uncertain` is
    /// reconcilable: it may be reclaimed once the in-flight effect is
    /// settled, revalidated back to `Active`, or closed by revocation/expiry.
    /// Normal release, expiry, revocation, and reclaim stay distinct facts:
    /// no transition collapses one into another.
    pub fn can_transition(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        if self.is_terminal() {
            return false;
        }
        match self {
            Self::Active => matches!(
                next,
                Self::Released
                    | Self::Expired
                    | Self::Revoked
                    | Self::ReclaimPending
                    | Self::Uncertain
            ),
            Self::ReclaimPending => {
                matches!(next, Self::Reclaimed | Self::Uncertain | Self::Revoked)
            }
            Self::Uncertain => matches!(
                next,
                Self::Reclaimed | Self::Active | Self::Revoked | Self::Expired
            ),
            Self::Released | Self::Expired | Self::Revoked | Self::Reclaimed => false,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Typed resource identity (ADR-0008 §2.2)
// ────────────────────────────────────────────────────────────────────────

/// The owning engine's resource family. The key below carries the smallest
/// identity that prevents a false match; the kind selects which fields the
/// engine canonicalized into `stable_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// Canonical real path + file identity + revision/content digest.
    File,
    /// File viewed/edited through the shared Office engine.
    OfficeDocument,
    /// Profile identity + target/tab identity + document/navigation generation.
    BrowserTab,
    /// Isolated or explicitly shared browser profile identity.
    BrowserProfile,
    /// Host + window/app/target identity + process birth identity + generation.
    DesktopTarget,
    /// Binding identity + provider-handle generation, never a Session id.
    ProviderHandle,
    /// A typed resource the coordinator tracks generically.
    Other,
}

impl ResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::OfficeDocument => "office_document",
            Self::BrowserTab => "browser_tab",
            Self::BrowserProfile => "browser_profile",
            Self::DesktopTarget => "desktop_target",
            Self::ProviderHandle => "provider_handle",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "file" => Some(Self::File),
            "office_document" => Some(Self::OfficeDocument),
            "browser_tab" => Some(Self::BrowserTab),
            "browser_profile" => Some(Self::BrowserProfile),
            "desktop_target" => Some(Self::DesktopTarget),
            "provider_handle" => Some(Self::ProviderHandle),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// Stable typed identity plus the physical version observed at issue.
/// `generation` changes when the physical version changes (file revision,
/// tab navigation, profile recreation, target replacement, provider-handle
/// rotation). It is not interchangeable with the lease `fence`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedResourceKey {
    pub kind: ResourceKind,
    /// Engine-canonicalized stable identity (never a URL, account record, or
    /// caller argument standing in for one — ADR-0008 §6 isolation rows).
    pub stable_id: String,
    /// Physical version observed at issue.
    pub generation: u64,
}

impl TypedResourceKey {
    pub fn new(kind: ResourceKind, stable_id: impl Into<String>, generation: u64) -> Self {
        Self {
            kind,
            stable_id: stable_id.into(),
            generation,
        }
    }

    /// The contention key: kind + stable identity, without the generation.
    /// Leases on the same canonical key contend; the generation decides
    /// whether a contender is current or stale.
    pub fn canonical_key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.stable_id)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.stable_id.is_empty() {
            return Err("resource stable_id must not be empty".into());
        }
        Ok(())
    }
}

/// How a lens/projection reports a referenced resource. Safe reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAvailability {
    Available,
    Stale,
    Unavailable,
    Conflicting,
    Pending,
}

impl ResourceAvailability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
            Self::Conflicting => "conflicting",
            Self::Pending => "pending",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "available" => Some(Self::Available),
            "stale" => Some(Self::Stale),
            "unavailable" => Some(Self::Unavailable),
            "conflicting" => Some(Self::Conflicting),
            "pending" => Some(Self::Pending),
            _ => None,
        }
    }
}

/// A Session-scoped reference to a physical resource. Identity, generation,
/// revision, and availability — never a bearer, permission, or content copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkbenchResourceRef {
    /// Lookup scope only. The Session does not own the physical resource.
    pub session_id: SessionId,
    pub key: TypedResourceKey,
    /// Revision/content digest when the engine provides one (files, Office).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_digest: Option<String>,
    pub availability: ResourceAvailability,
}

impl WorkbenchResourceRef {
    pub fn validate(&self) -> Result<(), String> {
        if self.session_id.as_str().is_empty() {
            return Err("resource ref session_id must not be empty".into());
        }
        self.key.validate()
    }
}

// ────────────────────────────────────────────────────────────────────────
// Owner context (ADR-0008 §1.1)
// ────────────────────────────────────────────────────────────────────────

/// The active owner tuple. `(SessionId, WorkId, RunId, AgentBindingId)`.
/// A provider session id is never a canonical owner id and has no field here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerContext {
    pub session_id: SessionId,
    pub work_id: WorkId,
    pub run_id: RunId,
    pub binding_id: AgentBindingId,
}

impl OwnerContext {
    pub fn new(
        session_id: SessionId,
        work_id: WorkId,
        run_id: RunId,
        binding_id: AgentBindingId,
    ) -> Self {
        Self {
            session_id,
            work_id,
            run_id,
            binding_id,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.session_id.as_str().is_empty() {
            return Err("owner session_id must not be empty".into());
        }
        if self.work_id.as_str().is_empty() {
            return Err("owner work_id must not be empty".into());
        }
        if self.run_id.as_str().is_empty() {
            return Err("owner run_id must not be empty".into());
        }
        if self.binding_id.as_str().is_empty() {
            return Err("owner binding_id must not be empty".into());
        }
        Ok(())
    }
}

// ────────────────────────────────────────────────────────────────────────
// ResourceLease + safe attachment (ADR-0008 §2.1)
// ────────────────────────────────────────────────────────────────────────

/// A Work/Run-owned concurrency and fencing record. Says which Work/Run may
/// observe, view, edit, or act on a resource generation. Not a permission,
/// capability grant, or Guard ticket — Guard still authorizes every effect.
///
/// There is deliberately no field for possession material: the bearer stays
/// in the Rust-private owner (`agentcowork-core::workbench`) and is never
/// serialized here, so a `ResourceLease` can be logged or audited safely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLease {
    /// Opaque, non-secret correlation id. Safe in projections and audit rows.
    pub lease_id: crate::LeaseId,
    pub resource: TypedResourceKey,
    /// The authoritative Work/Run owners of this lease record.
    pub owner_work_id: WorkId,
    pub owner_run_id: RunId,
    /// The binding that may exercise the lease (actor context, not owner).
    pub actor_binding_id: AgentBindingId,
    /// Canonical Session for lookup/audit only. Never the lease owner.
    pub session_scope: SessionId,
    pub access: LeaseAccess,
    /// Physical version observed at issue.
    pub resource_generation: u64,
    /// Monotonic lease-authority value for this resource. Advances on
    /// revocation, reclaim, and replacement — never reused.
    pub fence: u64,
    /// Policy/config scope used at issue (logout/scope change invalidates it).
    pub scope_fingerprint: String,
    pub state: LeaseState,
    pub issued_at_ms: u64,
    /// Bounded lifetime: always later than issue. No immortal leases.
    pub expires_at_ms: u64,
    /// Liveness evidence, not authority.
    pub last_heartbeat_ms: u64,
}

impl ResourceLease {
    pub fn validate(&self) -> Result<(), String> {
        if self.lease_id.as_str().is_empty() {
            return Err("lease_id must not be empty".into());
        }
        self.resource.validate()?;
        if self.owner_work_id.as_str().is_empty() || self.owner_run_id.as_str().is_empty() {
            return Err("lease Work/Run owner must not be empty".into());
        }
        if self.actor_binding_id.as_str().is_empty() {
            return Err("lease actor binding must not be empty".into());
        }
        if self.session_scope.as_str().is_empty() {
            return Err("lease session_scope must not be empty".into());
        }
        if self.fence == 0 {
            return Err("lease fence must be a positive monotonic value".into());
        }
        if self.expires_at_ms <= self.issued_at_ms {
            return Err("lease lifetime must be bounded: expires_at must exceed issued_at".into());
        }
        if self.last_heartbeat_ms < self.issued_at_ms {
            return Err("lease heartbeat must not predate issue".into());
        }
        Ok(())
    }

    /// Whether the lease record is live at `now_ms`. Wall-clock expiry is a
    /// backstop only: revocation and fencing decide authority.
    pub fn is_live(&self, now_ms: u64) -> bool {
        self.state.is_live() && now_ms <= self.expires_at_ms
    }

    /// The safe projection of this lease: id, generation, fence, status.
    /// This is the only lease shape that may cross IPC or be rendered.
    pub fn attachment(&self) -> LeaseAttachment {
        LeaseAttachment {
            lease_id: self.lease_id.clone(),
            session_scope: self.session_scope.clone(),
            resource_key: self.resource.canonical_key(),
            generation: self.resource_generation,
            fence: self.fence,
            state: self.state,
        }
    }
}

/// The non-secret lease view for projections, IPC, events, and logs.
/// No bearer, no handle, no capability material — by construction there is
/// no field that could carry any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseAttachment {
    pub lease_id: crate::LeaseId,
    pub session_scope: SessionId,
    pub resource_key: String,
    pub generation: u64,
    pub fence: u64,
    pub state: LeaseState,
}

// ────────────────────────────────────────────────────────────────────────
// LensState (ADR-0008 §1.3)
// ────────────────────────────────────────────────────────────────────────

/// Presentation surface family. A lens is a reference to a surface, not
/// ownership of the surface's physical resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LensKind {
    Browser,
    Office,
    Desktop,
    Code,
    Shell,
    Progress,
    Other,
}

impl LensKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Office => "office",
            Self::Desktop => "desktop",
            Self::Code => "code",
            Self::Shell => "shell",
            Self::Progress => "progress",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "browser" => Some(Self::Browser),
            "office" => Some(Self::Office),
            "desktop" => Some(Self::Desktop),
            "code" => Some(Self::Code),
            "shell" => Some(Self::Shell),
            "progress" => Some(Self::Progress),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// Per-resource availability as seen by one lens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LensAvailability {
    Available,
    Stale,
    Unavailable,
    Conflicting,
    Pending,
}

impl LensAvailability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
            Self::Conflicting => "conflicting",
            Self::Pending => "pending",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "available" => Some(Self::Available),
            "stale" => Some(Self::Stale),
            "unavailable" => Some(Self::Unavailable),
            "conflicting" => Some(Self::Conflicting),
            "pending" => Some(Self::Pending),
            _ => None,
        }
    }
}

/// How the lens may interact. Opening a lens records a reference; it never
/// acquires authority by itself. Takeover is requested, then Guard decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    Observe,
    ReadOnly,
    TakeoverRequested,
    UserControlled,
}

impl InteractionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::ReadOnly => "read_only",
            Self::TakeoverRequested => "takeover_requested",
            Self::UserControlled => "user_controlled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "observe" => Some(Self::Observe),
            "read_only" => Some(Self::ReadOnly),
            "takeover_requested" => Some(Self::TakeoverRequested),
            "user_controlled" => Some(Self::UserControlled),
            _ => None,
        }
    }
}

/// Per-Session presentation state. Scoped by exactly one owner tuple;
/// switching Sessions restores only the newly selected Session's lenses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensState {
    pub lens_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_id: Option<String>,
    pub kind: LensKind,
    pub open_order: u32,
    pub active: bool,
    /// Typed references shown by this lens (ids into the projection's
    /// `resource_refs`, never physical handles).
    #[serde(default)]
    pub resource_ref_ids: Vec<String>,
    pub resource_generation: u64,
    pub availability: LensAvailability,
    pub interaction_mode: InteractionMode,
    pub owner: OwnerContext,
}

impl LensState {
    pub fn validate(&self) -> Result<(), String> {
        if self.lens_id.is_empty() {
            return Err("lens_id must not be empty".into());
        }
        self.owner.validate()
    }
}

// ────────────────────────────────────────────────────────────────────────
// Post-effect outcome: unknown stays uncertain (RECOVERY.md §3)
// ────────────────────────────────────────────────────────────────────────

/// First-class result of a post-effect classification. `unknown`/`uncertain`
/// is a result, never a synonym for success, failure, or cancellation: no
/// path maps it to one of those without an observation/reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostEffectOutcome {
    Success,
    Failed,
    Cancelled,
    Uncertain,
}

impl PostEffectOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Uncertain => "uncertain",
        }
    }

    /// Strict parse. `unknown` and `uncertain` both read as [`Self::Uncertain`];
    /// anything unrecognized is `None` — never a fabricated verdict.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "success" | "succeeded" => Some(Self::Success),
            "failed" | "failure" => Some(Self::Failed),
            "cancelled" | "canceled" => Some(Self::Cancelled),
            "uncertain" | "unknown" => Some(Self::Uncertain),
            _ => None,
        }
    }

    /// Classify an observation: `None` (no observation — crash, transport
    /// drop, lost attempt) is [`Self::Uncertain`], never success or failure.
    /// Cancellation is explicit evidence only and never inferred here.
    pub fn from_observation(observed_success: Option<bool>) -> Self {
        match observed_success {
            Some(true) => Self::Success,
            Some(false) => Self::Failed,
            None => Self::Uncertain,
        }
    }

    pub fn is_settled(self) -> bool {
        matches!(self, Self::Success | Self::Failed | Self::Cancelled)
    }
}

// ────────────────────────────────────────────────────────────────────────
// Interaction-record references (ADR-0008 §4). The projection renders these;
// the named owner stays authoritative.
// ────────────────────────────────────────────────────────────────────────

/// A Work summary with a durable queue reference. Owned by Work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkRef {
    pub work_id: WorkId,
    pub session_id: SessionId,
    pub state: WorkState,
}

/// One execution attempt. Owned by Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Active,
    Waiting,
    Completed,
    Failed,
    Paused,
    Uncertain,
}

impl RunPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Paused => "paused",
            Self::Uncertain => "uncertain",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "waiting" => Some(Self::Waiting),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "paused" => Some(Self::Paused),
            "uncertain" | "unknown" => Some(Self::Uncertain),
            _ => None,
        }
    }
}

/// A Run summary with waits and active lease refs. Owned by Run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRef {
    pub run_id: RunId,
    pub work_id: WorkId,
    pub session_id: SessionId,
    pub phase: RunPhase,
    #[serde(default)]
    pub active_lease_ids: Vec<crate::LeaseId>,
}

/// A safe binding/readiness summary. Provider-private state is never here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingRef {
    pub binding_id: AgentBindingId,
    pub session_id: SessionId,
    pub work_id: WorkId,
    pub lifecycle: BindingLifecycle,
    /// A provider restart created a new provider-handle generation; old
    /// handles are stale and continuation is labelled restarted.
    #[serde(default)]
    pub provider_restarted: bool,
}

/// A durable pre-submit draft reference. Owned by the Session's logical
/// input record; never an effect, queue item, or provider prompt until
/// submitted. Unpromoted drafts follow the Session retention policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftRef {
    pub draft_id: String,
    pub session_id: SessionId,
}

/// An accepted user prompt for a specific Work/Run. Owned by Work/Run:
/// durable, ordered, cancellable, replayable from the event cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItemRef {
    pub queue_id: String,
    pub work_id: WorkId,
    pub session_id: SessionId,
    #[serde(default)]
    pub cancelled: bool,
}

/// A question bound to the exact Work/Run/generation. Owned by Work/Run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionRef {
    pub question_id: String,
    pub work_id: WorkId,
    pub run_id: RunId,
    pub session_id: SessionId,
    pub generation: u64,
    #[serde(default)]
    pub answered: bool,
}

/// A request to inspect a plan, diff, or pending effect. Grants no authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRef {
    pub review_id: String,
    pub work_id: WorkId,
    pub session_id: SessionId,
}

/// Audit-owned evidence reference. The projection displays the reference and
/// its uncertainty; it cannot manufacture a success receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptRef {
    pub receipt_id: ReceiptId,
    pub work_id: WorkId,
    pub session_id: SessionId,
    #[serde(default)]
    pub uncertain: bool,
}

// ────────────────────────────────────────────────────────────────────────
// Projection freshness (ADR-0008 §1.2)
// ────────────────────────────────────────────────────────────────────────

/// A projection is a cache/read model. When its sources are incomplete it
/// reports `rebuilding` or `unavailable` — never a fabricated clean state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionFreshness {
    Fresh,
    Rebuilding,
    Stale,
    Unavailable,
}

impl ProjectionFreshness {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Rebuilding => "rebuilding",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fresh" => Some(Self::Fresh),
            "rebuilding" => Some(Self::Rebuilding),
            "stale" => Some(Self::Stale),
            "unavailable" => Some(Self::Unavailable),
            _ => None,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// SessionWorkbenchProjection (ADR-0008 §1.2)
// ────────────────────────────────────────────────────────────────────────

/// The derived, non-authoritative read model keyed by canonical `SessionId`.
/// Composed from the `Session → Work → Run → AgentBinding` owner chain and
/// typed resource references. It contains references and safe summaries —
/// never physical content, credentials, bearer material, or an independent
/// execution state machine.
///
/// A persisted copy may make a cockpit fast, but it is never evidence that
/// an effect happened. Rebuilt from canonical records and the last
/// acknowledged event cursor; see `agentcowork-core::workbench`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWorkbenchProjection {
    pub schema_version: u32,
    /// Canonical, unique projection key.
    pub session_id: SessionId,
    pub session_kind: SessionKind,
    /// The active owner tuple, when a Work is active in this Session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_owner: Option<OwnerContext>,
    /// Last acknowledged canonical sequence this projection reflects.
    pub event_cursor: u64,
    pub freshness: ProjectionFreshness,
    #[serde(default)]
    pub works: Vec<WorkRef>,
    #[serde(default)]
    pub runs: Vec<RunRef>,
    #[serde(default)]
    pub bindings: Vec<BindingRef>,
    #[serde(default)]
    pub lenses: Vec<LensState>,
    #[serde(default)]
    pub resource_refs: Vec<WorkbenchResourceRef>,
    /// Non-secret lease id/generation/fence/status only.
    #[serde(default)]
    pub lease_attachments: Vec<LeaseAttachment>,
    #[serde(default)]
    pub drafts: Vec<DraftRef>,
    #[serde(default)]
    pub prompt_queue: Vec<QueueItemRef>,
    #[serde(default)]
    pub pending_questions: Vec<QuestionRef>,
    #[serde(default)]
    pub pending_reviews: Vec<ReviewRef>,
    #[serde(default)]
    pub receipt_refs: Vec<ReceiptRef>,
}

impl SessionWorkbenchProjection {
    /// A fresh, empty projection for one Session. Empty is honest:
    /// `rebuilding` until the coordinator fills it from the event cursor.
    pub fn rebuilding(session_id: SessionId, session_kind: SessionKind) -> Self {
        Self {
            schema_version: WORKBENCH_SCHEMA_VERSION,
            session_id,
            session_kind,
            active_owner: None,
            event_cursor: 0,
            freshness: ProjectionFreshness::Rebuilding,
            works: Vec::new(),
            runs: Vec::new(),
            bindings: Vec::new(),
            lenses: Vec::new(),
            resource_refs: Vec::new(),
            lease_attachments: Vec::new(),
            drafts: Vec::new(),
            prompt_queue: Vec::new(),
            pending_questions: Vec::new(),
            pending_reviews: Vec::new(),
            receipt_refs: Vec::new(),
        }
    }

    /// Validate the projection boundary: the schema version is current, the
    /// key is non-empty, and **every** Work, Run, Binding, lens, resource
    /// reference, lease attachment, draft, queue item, question, review, and
    /// receipt points back to this projection's `SessionId`. A projection may
    /// be absent, rebuilding, stale, or unavailable; it may never borrow
    /// another Session's records to fill the gap.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != WORKBENCH_SCHEMA_VERSION {
            return Err(format!(
                "unsupported workbench schema_version {}",
                self.schema_version
            ));
        }
        if self.session_id.as_str().is_empty() {
            return Err("projection session_id must not be empty".into());
        }
        let key = self.session_id.as_str();
        let scope = |what: &str, id: &str| -> Result<(), String> {
            if id != key {
                return Err(format!(
                    "{what} belongs to session `{id}`, not to projection `{key}`"
                ));
            }
            Ok(())
        };
        if let Some(owner) = &self.active_owner {
            owner.validate()?;
            scope("active_owner", owner.session_id.as_str())?;
        }
        for w in &self.works {
            scope("work", w.session_id.as_str())?;
            if w.work_id.as_str().is_empty() {
                return Err("work ref id must not be empty".into());
            }
        }
        for r in &self.runs {
            scope("run", r.session_id.as_str())?;
            if r.run_id.as_str().is_empty() || r.work_id.as_str().is_empty() {
                return Err("run ref ids must not be empty".into());
            }
        }
        for b in &self.bindings {
            scope("binding", b.session_id.as_str())?;
            if b.binding_id.as_str().is_empty() || b.work_id.as_str().is_empty() {
                return Err("binding ref ids must not be empty".into());
            }
        }
        for lens in &self.lenses {
            lens.validate()?;
            scope("lens", lens.owner.session_id.as_str())?;
        }
        for r in &self.resource_refs {
            r.validate()?;
            scope("resource_ref", r.session_id.as_str())?;
        }
        for a in &self.lease_attachments {
            if a.lease_id.as_str().is_empty() || a.resource_key.is_empty() {
                return Err("lease attachment ids must not be empty".into());
            }
            scope("lease_attachment", a.session_scope.as_str())?;
        }
        for d in &self.drafts {
            if d.draft_id.is_empty() {
                return Err("draft id must not be empty".into());
            }
            scope("draft", d.session_id.as_str())?;
        }
        for q in &self.prompt_queue {
            if q.queue_id.is_empty() || q.work_id.as_str().is_empty() {
                return Err("queue item ids must not be empty".into());
            }
            scope("prompt_queue", q.session_id.as_str())?;
        }
        for q in &self.pending_questions {
            if q.question_id.is_empty()
                || q.work_id.as_str().is_empty()
                || q.run_id.as_str().is_empty()
            {
                return Err("question ids must not be empty".into());
            }
            scope("question", q.session_id.as_str())?;
        }
        for r in &self.pending_reviews {
            if r.review_id.is_empty() || r.work_id.as_str().is_empty() {
                return Err("review ids must not be empty".into());
            }
            scope("review", r.session_id.as_str())?;
        }
        for r in &self.receipt_refs {
            if r.receipt_id.as_str().is_empty() || r.work_id.as_str().is_empty() {
                return Err("receipt ref ids must not be empty".into());
            }
            scope("receipt", r.session_id.as_str())?;
        }
        Ok(())
    }
}

// ────────────────────────────────────────────────────────────────────────
// Lease-lifecycle facts: the durable, replayable lease vocabulary.
// Facts ride `EventEnvelope`s; replaying them rebuilds coordinator state
// from the event cursor without a second log.
// ────────────────────────────────────────────────────────────────────────

/// One durable lease-lifecycle fact. Emitted by the coordinator for every
/// mutation; the owner persists it on the Work event stream and recovery
/// replays it. No bearer material — safe in payloads and logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseLifecycleFact {
    pub lease_id: crate::LeaseId,
    /// One of [`LEASE_EVENT_KINDS`].
    pub kind: String,
    pub fence: u64,
    pub generation: u64,
    pub at_ms: u64,
    /// Safe detail only (state names, owner ids, conflict class) — never
    /// possession material. Validated by [`Self::validate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Self-describing acquire context: present on `lease.acquired` facts so
    /// a rebuild from the event cursor can reconstruct the lease without a
    /// second store. Ids and access class only — never possession material.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<LeaseAccess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<OwnerContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<TypedResourceKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_fingerprint: Option<String>,
}

impl LeaseLifecycleFact {
    pub fn new(
        lease_id: crate::LeaseId,
        kind: impl Into<String>,
        fence: u64,
        generation: u64,
        at_ms: u64,
    ) -> Self {
        Self {
            lease_id,
            kind: kind.into(),
            fence,
            generation,
            at_ms,
            detail: None,
            access: None,
            owner: None,
            resource: None,
            scope_fingerprint: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Attach the acquire context for a `lease.acquired` fact.
    pub fn with_acquire_context(
        mut self,
        access: LeaseAccess,
        owner: OwnerContext,
        resource: TypedResourceKey,
        scope_fingerprint: impl Into<String>,
    ) -> Self {
        self.access = Some(access);
        self.owner = Some(owner);
        self.resource = Some(resource);
        self.scope_fingerprint = Some(scope_fingerprint.into());
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.lease_id.as_str().is_empty() {
            return Err("lease fact id must not be empty".into());
        }
        if !LEASE_EVENT_KINDS.contains(&self.kind.as_str()) {
            return Err(format!("unknown lease fact kind `{}`", self.kind));
        }
        if self.fence == 0 {
            return Err("lease fact fence must be positive".into());
        }
        if let Some(detail) = &self.detail {
            let rendered = detail.to_ascii_lowercase();
            for key in FORBIDDEN_PROJECTION_KEYS {
                if rendered.contains(key) {
                    return Err(format!(
                        "lease fact detail must not carry secret-shaped key `{key}`"
                    ));
                }
            }
        }
        if !projection_json_has_no_secrets(&serde_json::to_string(self).unwrap_or_default()) {
            return Err("lease fact must not carry secret-shaped keys".into());
        }
        Ok(())
    }

    /// Read a fact back from a canonical envelope payload. Returns `None`
    /// when the envelope is not a lease fact (unknown kinds are skipped by
    /// replay, never fabricated into lease state).
    pub fn from_envelope(kind: &str, payload: &serde_json::Value) -> Option<Self> {
        if !LEASE_EVENT_KINDS.contains(&kind) {
            return None;
        }
        let lease_id = payload.get("lease_id")?.as_str()?;
        let parse_owner = || -> Option<OwnerContext> {
            let o = payload.get("owner")?;
            Some(OwnerContext {
                session_id: crate::SessionId::new(o.get("session_id")?.as_str()?),
                work_id: crate::WorkId::new(o.get("work_id")?.as_str()?),
                run_id: crate::RunId::new(o.get("run_id")?.as_str()?),
                binding_id: crate::AgentBindingId::new(o.get("binding_id")?.as_str()?),
            })
        };
        let parse_resource = || -> Option<TypedResourceKey> {
            let r = payload.get("resource")?;
            Some(TypedResourceKey {
                kind: ResourceKind::parse(r.get("kind")?.as_str()?)?,
                stable_id: r.get("stable_id")?.as_str()?.to_string(),
                generation: r.get("generation").and_then(|g| g.as_u64()).unwrap_or(0),
            })
        };
        Some(Self {
            lease_id: crate::LeaseId::new(lease_id),
            kind: kind.to_string(),
            fence: payload.get("fence")?.as_u64()?,
            generation: payload
                .get("generation")
                .and_then(|g| g.as_u64())
                .unwrap_or(0),
            at_ms: payload.get("at_ms").and_then(|t| t.as_u64()).unwrap_or(0),
            detail: payload
                .get("detail")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string()),
            access: payload
                .get("access")
                .and_then(|a| a.as_str())
                .and_then(LeaseAccess::parse),
            owner: parse_owner(),
            resource: parse_resource(),
            scope_fingerprint: payload
                .get("scope_fingerprint")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string()),
        })
    }

    /// The envelope payload for this fact. The owner stamps identity, order,
    /// and schema version; the fact contributes data only.
    pub fn envelope_payload(&self) -> serde_json::Value {
        let mut payload = serde_json::json!({
            "lease_id": self.lease_id.as_str(),
            "fence": self.fence,
            "generation": self.generation,
            "at_ms": self.at_ms,
        });
        if let Some(detail) = &self.detail {
            payload["detail"] = serde_json::Value::String(detail.clone());
        }
        if let Some(access) = &self.access {
            payload["access"] = serde_json::Value::String(access.as_str().to_string());
        }
        if let Some(owner) = &self.owner {
            payload["owner"] = serde_json::json!({
                "session_id": owner.session_id.as_str(),
                "work_id": owner.work_id.as_str(),
                "run_id": owner.run_id.as_str(),
                "binding_id": owner.binding_id.as_str(),
            });
        }
        if let Some(resource) = &self.resource {
            payload["resource"] = serde_json::json!({
                "kind": resource.kind.as_str(),
                "stable_id": resource.stable_id,
                "generation": resource.generation,
            });
        }
        if let Some(scope) = &self.scope_fingerprint {
            payload["scope_fingerprint"] = serde_json::Value::String(scope.clone());
        }
        payload
    }

    /// The [`LeaseState`] this fact records, when it records one.
    pub fn records_state(&self) -> Option<LeaseState> {
        match self.kind.as_str() {
            LEASE_EVENT_ACQUIRED | LEASE_EVENT_RENEWED | LEASE_EVENT_ACTOR_REVALIDATED => {
                Some(LeaseState::Active)
            }
            LEASE_EVENT_RELEASED => Some(LeaseState::Released),
            LEASE_EVENT_REVOKED => Some(LeaseState::Revoked),
            LEASE_EVENT_EXPIRED => Some(LeaseState::Expired),
            LEASE_EVENT_RECLAIM_PENDING => Some(LeaseState::ReclaimPending),
            LEASE_EVENT_RECLAIMED => Some(LeaseState::Reclaimed),
            LEASE_EVENT_UNCERTAIN => Some(LeaseState::Uncertain),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(session: &str) -> OwnerContext {
        OwnerContext::new(
            SessionId::new(session),
            WorkId::new("w-1"),
            RunId::new("r-1"),
            AgentBindingId::new("b-1"),
        )
    }

    fn lease(session: &str) -> ResourceLease {
        ResourceLease {
            lease_id: crate::LeaseId::new("lease-1"),
            resource: TypedResourceKey::new(ResourceKind::File, "/work/plan.md", 7),
            owner_work_id: WorkId::new("w-1"),
            owner_run_id: RunId::new("r-1"),
            actor_binding_id: AgentBindingId::new("b-1"),
            session_scope: SessionId::new(session),
            access: LeaseAccess::Edit,
            resource_generation: 7,
            fence: 1,
            scope_fingerprint: "policy:v3".into(),
            state: LeaseState::Active,
            issued_at_ms: 100,
            expires_at_ms: 200,
            last_heartbeat_ms: 120,
        }
    }

    #[test]
    fn projection_key_never_borrows_another_sessions_records() {
        let mut projection =
            SessionWorkbenchProjection::rebuilding(SessionId::new("s-1"), SessionKind::Interactive);
        projection.active_owner = Some(owner("s-1"));
        projection.works.push(WorkRef {
            work_id: WorkId::new("w-1"),
            session_id: SessionId::new("s-1"),
            state: WorkState::Running,
        });
        projection.lenses.push(LensState {
            lens_id: "lens-1".into(),
            view_id: None,
            kind: LensKind::Office,
            open_order: 0,
            active: true,
            resource_ref_ids: vec![],
            resource_generation: 7,
            availability: LensAvailability::Available,
            interaction_mode: InteractionMode::ReadOnly,
            owner: owner("s-1"),
        });
        projection.resource_refs.push(WorkbenchResourceRef {
            session_id: SessionId::new("s-1"),
            key: TypedResourceKey::new(ResourceKind::File, "/work/plan.md", 7),
            revision_digest: Some("sha256:abc".into()),
            availability: ResourceAvailability::Available,
        });
        projection.lease_attachments.push(lease("s-1").attachment());
        assert!(projection.validate().is_ok());

        // A lens from another Session must not fill this projection's gap.
        projection.lenses[0].owner = owner("s-2");
        let err = projection
            .validate()
            .expect_err("cross-session lens must fail");
        assert!(err.contains("s-2"), "unexpected error: {err}");

        // Same for a borrowed lease attachment.
        projection.lenses[0].owner = owner("s-1");
        projection.lease_attachments[0].session_scope = SessionId::new("s-2");
        let err = projection
            .validate()
            .expect_err("cross-session lease must fail");
        assert!(err.contains("s-2"), "unexpected error: {err}");
    }

    #[test]
    fn unknown_post_effect_outcomes_stay_uncertain() {
        assert_eq!(
            PostEffectOutcome::from_observation(None),
            PostEffectOutcome::Uncertain
        );
        assert_eq!(
            PostEffectOutcome::from_observation(Some(true)),
            PostEffectOutcome::Success
        );
        assert_eq!(
            PostEffectOutcome::from_observation(Some(false)),
            PostEffectOutcome::Failed
        );
        // The unknown spellings are first-class uncertainty, never verdicts.
        assert_eq!(
            PostEffectOutcome::parse("unknown"),
            Some(PostEffectOutcome::Uncertain)
        );
        assert_eq!(
            PostEffectOutcome::parse("uncertain"),
            Some(PostEffectOutcome::Uncertain)
        );
        assert_eq!(PostEffectOutcome::parse("probably_fine"), None);
        assert!(!PostEffectOutcome::Uncertain.is_settled());
        assert!(PostEffectOutcome::Cancelled.is_settled());
    }

    #[test]
    fn observation_leases_share_but_edit_is_exclusive() {
        assert!(LeaseAccess::Observe.compatible_with(LeaseAccess::View));
        assert!(LeaseAccess::View.compatible_with(LeaseAccess::Observe));
        assert!(!LeaseAccess::Edit.compatible_with(LeaseAccess::View));
        assert!(!LeaseAccess::View.compatible_with(LeaseAccess::Edit));
        assert!(!LeaseAccess::Action.compatible_with(LeaseAccess::Action));
        assert!(!LeaseAccess::Control.compatible_with(LeaseAccess::Observe));
        assert!(LeaseAccess::Edit.is_exclusive());
        assert!(!LeaseAccess::View.is_observation() == false);
        assert_eq!(LeaseAccess::parse("nope"), None);
    }

    #[test]
    fn lease_lifecycle_keeps_release_expiry_revocation_reclaim_distinct() {
        // Terminal facts are one-way; replay of the same fact is idempotent.
        for terminal in [
            LeaseState::Released,
            LeaseState::Expired,
            LeaseState::Revoked,
            LeaseState::Reclaimed,
        ] {
            assert!(terminal.is_terminal());
            assert!(terminal.can_transition(terminal));
            assert!(!terminal.can_transition(LeaseState::Active));
            assert!(!terminal.can_transition(LeaseState::Uncertain));
        }
        // Active branches to every durable fact, but never straight to reclaimed.
        assert!(LeaseState::Active.can_transition(LeaseState::Uncertain));
        assert!(LeaseState::Active.can_transition(LeaseState::ReclaimPending));
        assert!(!LeaseState::Active.can_transition(LeaseState::Reclaimed));
        // Uncertain reconciles; it is never silently freed. Revocation stays
        // available from ReclaimPending: it is an explicit authority decision
        // (takeover, logout, Guard), distinct from reclaim, and it fences
        // immediately so a racing holder goes stale.
        assert!(LeaseState::Uncertain.can_transition(LeaseState::Reclaimed));
        assert!(LeaseState::Uncertain.can_transition(LeaseState::Active));
        assert!(!LeaseState::Uncertain.can_transition(LeaseState::Released));
        assert!(LeaseState::ReclaimPending.can_transition(LeaseState::Revoked));
        assert!(!LeaseState::Active.is_terminal());
        assert!(LeaseState::Active.is_live());
        assert!(!LeaseState::ReclaimPending.is_live());
    }

    #[test]
    fn lease_schema_bounds_lifetime_and_fence() {
        let good = lease("s-1");
        assert!(good.validate().is_ok());
        assert!(good.is_live(150));
        assert!(!good.is_live(201));

        let mut immortal = good.clone();
        immortal.expires_at_ms = immortal.issued_at_ms;
        assert!(immortal.validate().is_err());

        let mut unfenced = good.clone();
        unfenced.fence = 0;
        assert!(unfenced.validate().is_err());

        let mut early_heartbeat = good.clone();
        early_heartbeat.last_heartbeat_ms = 1;
        assert!(early_heartbeat.validate().is_err());
    }

    #[test]
    fn projection_and_facts_carry_no_secret_shaped_keys() {
        let mut projection =
            SessionWorkbenchProjection::rebuilding(SessionId::new("s-1"), SessionKind::Automation);
        projection.active_owner = Some(owner("s-1"));
        projection.lease_attachments.push(lease("s-1").attachment());
        let rendered = serde_json::to_string(&projection).unwrap();
        assert!(
            projection_json_has_no_secrets(&rendered),
            "projection leaks secret-shaped keys: {rendered}"
        );

        let fact = LeaseLifecycleFact::new(
            crate::LeaseId::new("lease-1"),
            LEASE_EVENT_ACQUIRED,
            1,
            7,
            100,
        )
        .with_detail("revoked holder lease-0 conflict=edit");
        assert!(fact.validate().is_ok());
        let rendered = serde_json::to_string(&fact.envelope_payload()).unwrap();
        assert!(projection_json_has_no_secrets(&rendered));

        // A detail smuggling possession material is refused at validation.
        let smuggled = LeaseLifecycleFact::new(
            crate::LeaseId::new("lease-1"),
            LEASE_EVENT_ACQUIRED,
            1,
            7,
            100,
        )
        .with_detail("bearer=deadbeef");
        assert!(smuggled.validate().is_err());

        // Unknown fact kinds never enter the vocabulary.
        let unknown = LeaseLifecycleFact::new(crate::LeaseId::new("x"), "lease.minted", 1, 1, 1);
        assert!(unknown.validate().is_err());
        assert!(LeaseLifecycleFact::from_envelope("chat.sent", &serde_json::json!({})).is_none());
    }

    #[test]
    fn facts_round_trip_through_envelope_payloads() {
        let fact = LeaseLifecycleFact::new(
            crate::LeaseId::new("lease-9"),
            LEASE_EVENT_REVOKED,
            4,
            7,
            512,
        );
        let payload = fact.envelope_payload();
        let back = LeaseLifecycleFact::from_envelope(LEASE_EVENT_REVOKED, &payload).unwrap();
        assert_eq!(back, fact);
        assert_eq!(fact.records_state(), Some(LeaseState::Revoked));
        // Acquire facts carry their context so a cursor rebuild needs no
        // second store — and the context stays secret-free.
        let acquired = LeaseLifecycleFact::new(
            crate::LeaseId::new("lease-1"),
            LEASE_EVENT_ACQUIRED,
            1,
            7,
            100,
        )
        .with_acquire_context(
            LeaseAccess::Edit,
            owner("s-1"),
            TypedResourceKey::new(ResourceKind::File, "/work/plan.md", 7),
            "policy:v3",
        );
        assert!(acquired.validate().is_ok());
        let payload = acquired.envelope_payload();
        assert!(projection_json_has_no_secrets(
            &serde_json::to_string(&payload).unwrap()
        ));
        let back = LeaseLifecycleFact::from_envelope(LEASE_EVENT_ACQUIRED, &payload).unwrap();
        assert_eq!(back, acquired);
        assert_eq!(
            LeaseLifecycleFact::new(
                crate::LeaseId::new("l"),
                LEASE_EVENT_ACTOR_REVALIDATED,
                2,
                1,
                1
            )
            .records_state(),
            Some(LeaseState::Active)
        );
        for kind in LEASE_EVENT_KINDS {
            assert!(LeaseState::parse("active").is_some());
            let _ = kind;
        }
    }

    #[test]
    fn canonical_key_separates_identity_from_generation() {
        let a = TypedResourceKey::new(ResourceKind::BrowserTab, "profile:p/tab:t", 3);
        let b = TypedResourceKey::new(ResourceKind::BrowserTab, "profile:p/tab:t", 4);
        assert_eq!(a.canonical_key(), b.canonical_key());
        assert_ne!(a.generation, b.generation);
        assert!(
            TypedResourceKey::new(ResourceKind::File, "", 1)
                .validate()
                .is_err()
        );
        // A provider handle never carries a Session id as its identity.
        let handle = TypedResourceKey::new(ResourceKind::ProviderHandle, "binding:b/gen:2", 2);
        assert!(!handle.stable_id.contains("session"));
    }
}
