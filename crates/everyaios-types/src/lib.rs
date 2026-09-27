//! P47.3 — **everyaios-types**: the shared contract crate (spec §4.0 item 20,
//! the "one structural change" the v3.59 architecture finalization chose).
//!
//! This crate exists to **kill contract drift between crates**. Every ID and
//! every status/risk/governance enum has exactly one canonical home here; the
//! other EveryAIOS crates name the same thing the same way instead of each
//! minting a slightly-different copy. Deliberately **pure**: newtypes + enums
//! only — no business logic, no DB, no networking, no IO.
//!
//! Rules for contributors:
//! - An ID that shows up on a wire boundary / across two crates lives here.
//! - A status/risk/category enum that two crates would otherwise re-declare
//!   lives here.
//! - Nothing with side effects. If you need behavior, add it as an `impl`
//!   on a type that already exists, or put it in the crate that owns the
//!   behavior (never here).

use serde::{Deserialize, Serialize};

pub mod canonical;
pub mod config;
pub mod delegation;
pub mod error;
pub mod id;
pub mod plane;
pub mod store;
pub mod time;
pub mod turn_snapshot;
pub mod workbench;

pub use config::{CONFIG_SCHEMA_VERSION, ConfigRefusal, ConfigSnapshot};
pub use error::{
    BoundaryError, ErrorCode, KernelError, MAX_BOUNDARY_CAUSE, MAX_BOUNDARY_MESSAGE, NextStep,
    RetryClass,
};
pub use id::{EntityId, IdMinter, ShortId, UuidV7Error};
pub use time::{Deadline, EpochMillis, MonotonicClock, TimezonePolicy};

// ────────────────────────────────────────────────────────────────────────
// ID newtypes — opaque, serializable, displayable, comparable.
// Use these on any cross-crate wire boundary so a `WorkId` can never be
// silently passed where a `TicketId` is expected (the pain point this crate
// exists to remove).
// ────────────────────────────────────────────────────────────────────────

macro_rules! id_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(v: impl Into<String>) -> Self {
                Self(v.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<String> for $name {
            fn from(v: String) -> Self {
                Self(v)
            }
        }

        impl From<&str> for $name {
            fn from(v: &str) -> Self {
                Self(v.to_string())
            }
        }
    };
}

id_newtype!(
    /// The durable unit of work (the product name for the `Execution` hub).
    WorkId
);
id_newtype!(
    /// P71.10 (ADR-0008) — a Work/Run-owned resource lease. Opaque and
    /// non-secret: safe to correlate in a projection or audit row. The
    /// capability/bearer proving possession stays in the Rust-private owner
    /// and never appears beside this id on any wire boundary.
    LeaseId
);
id_newtype!(
    /// A workspace / project root identity.
    ProjectId
);
id_newtype!(
    /// A chat / agent session.
    SessionId
);
id_newtype!(
    /// A single run of a work item.
    RunId
);
id_newtype!(
    /// A recorded execution step inside a work item.
    ExecutionId
);
id_newtype!(
    /// A Guard-2 authorization ticket.
    TicketId
);
id_newtype!(
    /// A (work- or effect-level) audit receipt.
    ReceiptId
);
id_newtype!(
    /// An artifact produced during work.
    ArtifactId
);
id_newtype!(
    /// A resource identity (file, sheet+cell, URL, window …) a ticket binds.
    ResourceId
);
id_newtype!(
    /// The acting agent.
    AgentId
);
id_newtype!(
    /// An extension capability id.
    CapabilityId
);
id_newtype!(
    /// A model/provider identity.
    ProviderId
);
id_newtype!(
    /// An installed/registered skill slug.
    SkillId
);
id_newtype!(
    /// A distributed-trace correlation id.
    TraceId
);
id_newtype!(
    /// P71.3g — a durable Work checkpoint (`ARCH/RECOVERY.md` §7). Identified so
    /// a resume, a restore fence and a receipt can name the same snapshot
    /// instead of re-deriving it from `(work, step)` string concatenation.
    CheckpointId
);

impl CheckpointId {
    /// The deterministic id for one step checkpoint: `ckpt:<work>/<step>`.
    /// Deterministic on purpose — a re-derivation of the same
    /// `(work, step)` names the same snapshot, no mapping table needed.
    pub fn for_step(work_id: &str, step: u32) -> Self {
        Self(format!("ckpt:{work_id}/{step}"))
    }

    /// No id assigned (a legacy snapshot written before ids existed, or a
    /// fresh in-memory checkpoint not yet persisted).
    pub fn unassigned() -> Self {
        Self(String::new())
    }

    /// Whether an id is actually attached.
    pub fn is_assigned(&self) -> bool {
        !self.0.is_empty()
    }
}

impl Default for CheckpointId {
    fn default() -> Self {
        Self::unassigned()
    }
}

// ────────────────────────────────────────────────────────────────────────
// Hash newtypes
// ────────────────────────────────────────────────────────────────────────

id_newtype!(
    /// A content-addressable config/runtime-manifest hash.
    ConfigHash
);
id_newtype!(
    /// A canonical-args SHA-256 hash bound into a ticket.
    ArgsHash
);

// ────────────────────────────────────────────────────────────────────────
// Shared enum vocabulary
// ────────────────────────────────────────────────────────────────────────

/// The lifecycle of a durable unit of work (`ARCH/WORK.md` §4).
///
/// Event-derived: the state is a projection of the Work's own timeline, never a
/// mutable blob the system trusts. [`Self::Recoverable`] is a **first-class
/// outcome, not a failure** — *the effect outcome is unknown* is a different fact
/// from *the effect failed* (`ARCH/RECOVERY.md` §3), and collapsing the two is
/// how a resumable Work gets reported as a lost one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkState {
    /// Minted; nothing has been planned yet.
    Created,
    /// A plan is being produced.
    Planning,
    /// Planned and admissible; no run has started.
    Ready,
    /// A run is actively progressing (no wait outstanding).
    Running,
    /// Running, blocked on a tool call in flight.
    WaitingTool,
    /// Running, blocked on an approval decision (Guard ticket / HITL).
    WaitingApproval,
    /// Running, blocked on the user (a question only they can answer).
    WaitingUser,
    /// Running, paused at a durable checkpoint. Resumable from the snapshot.
    Checkpointed,
    /// Execution finished; verification has not yet returned a verdict.
    Verifying,
    /// Verified success.
    Completed,
    /// Attempted and failed (the failure is known and is not an unknown effect).
    Failed,
    /// Stopped by the user or by policy.
    Cancelled,
    /// Deliberately parked; resumable without a checkpoint verdict.
    Paused,
    /// Interrupted with an **unknown** in-flight effect: resumable, and the
    /// resume must present what is known and what is unknown.
    Recoverable,
}

/// The four `Running` sub-states of [`WorkState`] (`ARCH/WORK.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkRunPhase {
    /// Actively progressing.
    Active,
    /// Blocked on a tool call in flight.
    WaitingTool,
    /// Blocked on an approval decision.
    WaitingApproval,
    /// Blocked on the user.
    WaitingUser,
    /// Parked at a durable checkpoint.
    Checkpointed,
}

impl WorkRunPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::WaitingTool => "waiting_tool",
            Self::WaitingApproval => "waiting_approval",
            Self::WaitingUser => "waiting_user",
            Self::Checkpointed => "checkpointed",
        }
    }

    /// The Work state for this phase (one vocabulary, one direction).
    pub fn work_state(self) -> WorkState {
        match self {
            Self::Active => WorkState::Running,
            Self::WaitingTool => WorkState::WaitingTool,
            Self::WaitingApproval => WorkState::WaitingApproval,
            Self::WaitingUser => WorkState::WaitingUser,
            Self::Checkpointed => WorkState::Checkpointed,
        }
    }
}

impl WorkState {
    /// Stable wire spelling (the UI projection reads this verbatim).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Planning => "planning",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::WaitingTool => "waiting_tool",
            Self::WaitingApproval => "waiting_approval",
            Self::WaitingUser => "waiting_user",
            Self::Checkpointed => "checkpointed",
            Self::Verifying => "verifying",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Paused => "paused",
            Self::Recoverable => "recoverable",
        }
    }

    /// Parse the canonical wire spelling, or `None` when it is not one. Every
    /// wire boundary (the transition RPC, a replayed event) uses this and
    /// refuses what it does not understand, so a state is never invented.
    pub fn try_parse(s: &str) -> Option<Self> {
        match s {
            "created" => Some(Self::Created),
            "planning" => Some(Self::Planning),
            "ready" => Some(Self::Ready),
            "running" => Some(Self::Running),
            "waiting_tool" | "waiting-tool" => Some(Self::WaitingTool),
            "waiting_approval" | "waiting-approval" => Some(Self::WaitingApproval),
            "waiting_user" | "waiting-user" => Some(Self::WaitingUser),
            "checkpointed" => Some(Self::Checkpointed),
            "verifying" => Some(Self::Verifying),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" | "canceled" => Some(Self::Cancelled),
            "paused" => Some(Self::Paused),
            "recoverable" => Some(Self::Recoverable),
            _ => None,
        }
    }

    /// Parse the canonical wire spelling. Unknown spellings read as
    /// [`Self::Created`] — "not started" is the only safe default, and it is
    /// never reported as success.
    pub fn parse(s: &str) -> Self {
        Self::try_parse(s).unwrap_or(Self::Created)
    }

    /// The Work is progressing, including its `Running` sub-states.
    pub fn is_running(self) -> bool {
        matches!(
            self,
            Self::Running
                | Self::WaitingTool
                | Self::WaitingApproval
                | Self::WaitingUser
                | Self::Checkpointed
        )
    }

    /// The `Running` sub-state, when this is one.
    pub fn run_phase(self) -> Option<WorkRunPhase> {
        match self {
            Self::Running => Some(WorkRunPhase::Active),
            Self::WaitingTool => Some(WorkRunPhase::WaitingTool),
            Self::WaitingApproval => Some(WorkRunPhase::WaitingApproval),
            Self::WaitingUser => Some(WorkRunPhase::WaitingUser),
            Self::Checkpointed => Some(WorkRunPhase::Checkpointed),
            _ => None,
        }
    }

    /// A state from which the Work can be resumed.
    pub fn is_resumable(self) -> bool {
        matches!(
            self,
            Self::Paused | Self::Checkpointed | Self::Recoverable | Self::WaitingUser
        )
    }

    /// The Work will not progress further on its own.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Whether a lifecycle transition is legal for a durable Work projection.
    ///
    /// Terminal outcomes are one-way. A repeated terminal state is idempotent
    /// (replay can observe the same fact more than once), but no terminal
    /// outcome may be replaced by `Ready`/`Running` or by another terminal
    /// outcome. `Recoverable` is intentionally resumable; callers that allow
    /// its `Running` edge must first reconcile any uncertain effect at the
    /// Work/effect owner rather than infer success from this vocabulary alone.
    pub fn can_transition(self, next: Self) -> bool {
        use WorkState::*;
        if self == next {
            return true;
        }
        if self.is_terminal() {
            return false;
        }
        match self {
            Created => matches!(next, Planning | Ready | Cancelled),
            Planning => matches!(next, Ready | Failed | Cancelled),
            Ready => matches!(
                next,
                Running | WaitingTool | WaitingApproval | WaitingUser | Cancelled
            ),
            Running => matches!(
                next,
                WaitingTool
                    | WaitingApproval
                    | WaitingUser
                    | Checkpointed
                    | Verifying
                    | Completed
                    | Failed
                    | Cancelled
                    | Paused
                    | Recoverable
            ),
            WaitingTool | WaitingApproval | WaitingUser => matches!(
                next,
                Running
                    | Failed
                    | Cancelled
                    | Paused
                    | Recoverable
                    | WaitingTool
                    | WaitingApproval
                    | WaitingUser
            ),
            Checkpointed => matches!(next, Running | Failed | Cancelled | Paused | Recoverable),
            Verifying => matches!(next, Completed | Failed | Cancelled | Recoverable),
            Paused => matches!(next, Running | Failed | Cancelled | Recoverable),
            // The owner must perform effect reconciliation before selecting the
            // resumable edge; the vocabulary itself does not grant permission.
            Recoverable => matches!(next, Running | Failed | Cancelled),
            Completed | Failed | Cancelled => false,
        }
    }
}

/// Why a Work is not progressing (`ARCH/AUTOMATION.md` §8).
///
/// Durable: a wait survives a restart with the reason intact, so a resumed Work
/// presents *what it waited for* instead of a confident summary. The four
/// `WorkState::Waiting*` states cover tool/approval/user/checkpoint; the
/// remaining reasons are waits with no `Waiting*` state of their own and park
/// the Work as [`WorkState::Paused`] until their condition is met.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    /// A Guard ticket / HITL approval is pending.
    Approval,
    /// The user must answer a question.
    UserInput,
    /// A deadline (`deadline_ms`) or a scheduled re-attempt.
    Timer,
    /// An external event (webhook, connector push, file watch).
    ExternalEvent,
    /// A resource is unavailable or rate-limited (disk, quota, lock).
    Resource,
    /// Another agent's turn or delegated child must finish.
    Agent,
    /// A retry backoff is in progress (`ARCH/RECOVERY.md`).
    Retry,
}

impl WaitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::UserInput => "user_input",
            Self::Timer => "timer",
            Self::ExternalEvent => "external_event",
            Self::Resource => "resource",
            Self::Agent => "agent",
            Self::Retry => "retry",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "approval" => Some(Self::Approval),
            "user_input" | "user-input" => Some(Self::UserInput),
            "timer" => Some(Self::Timer),
            "external_event" | "external-event" => Some(Self::ExternalEvent),
            "resource" => Some(Self::Resource),
            "agent" => Some(Self::Agent),
            "retry" => Some(Self::Retry),
            _ => None,
        }
    }

    /// The Work state this reason parks the Work in. Approval and user input
    /// have named `Waiting*` states; the rest park as `Paused` (a durable wait
    /// with no dedicated state — never a silent `Running`).
    pub fn work_state(self) -> WorkState {
        match self {
            Self::Approval => WorkState::WaitingApproval,
            Self::UserInput => WorkState::WaitingUser,
            Self::Timer | Self::ExternalEvent | Self::Resource | Self::Agent | Self::Retry => {
                WorkState::Paused
            }
        }
    }
}

/// One durable wait: the reason plus what would resume it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitCondition {
    pub reason: WaitReason,
    /// Free-text detail for the UI (never a secret; never a substitute for the
    /// reason).
    #[serde(default)]
    pub detail: Option<String>,
    /// Wall-clock ms after which the wait is reconsidered (`Timer`). `None` ⇒
    /// no deadline. Integer ms — a wire boundary never carries floats.
    #[serde(default)]
    pub deadline_ms: Option<u64>,
    /// What resumes it: an event kind, a ticket id, an agent id, a resource id.
    #[serde(default)]
    pub resume_on: Option<String>,
}

impl WaitCondition {
    pub fn new(reason: WaitReason) -> Self {
        Self {
            reason,
            detail: None,
            deadline_ms: None,
            resume_on: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_deadline_ms(mut self, at_ms: u64) -> Self {
        self.deadline_ms = Some(at_ms);
        self
    }

    pub fn with_resume_on(mut self, what: impl Into<String>) -> Self {
        self.resume_on = Some(what.into());
        self
    }

    /// The Work state this condition parks the Work in.
    pub fn work_state(&self) -> WorkState {
        self.reason.work_state()
    }
}

/// The phase of an execution step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Ready,
    Running,
    Completed,
    Failed,
    Rejected,
}

/// The guard risk band. Canonical so every crate grades an action on the
/// same scale (the `RiskLevel` re-declarations in `everyaios-guard` are
/// aliased to this).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

/// The H34 autonomy level (canonical — the UI/native/Rust all agree).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    Sandbox,
    Ask,
    Auto,
    Maximum,
}

/// Who governs an effect: policy-auto, a human gesture, a ticket, an
/// automation task. The single source of truth for audit `authorization`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceMode {
    AgentTicket,
    AutomationTicket,
    HumanGesture,
    Policy,
    Coordinator,
}

/// The honesty status of evidence for a claim/effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Verified,
    PartiallyComplete,
    Degraded,
    Unverifiable,
    NotVerified,
}

/// Ownership/lifecycle of a resource a ticket or audit binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceState {
    Free,
    Owned,
    Locked,
    Released,
    Tombstone,
}

/// The retry class of an effect (doc 53 §4 — safe-retry / unsafe /
/// same-key / confirm-after-uncertain).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdempotencyClass {
    /// Read-only / deterministic — retry freely.
    SafeRetry,
    /// Mutates (write, send, execute) — never auto-retry.
    UnsafeRetry,
    /// Retry only with an identical idempotency key; broker dedupes.
    SameKey,
    /// Outcome unknown (network drop mid-mutation) — confirm before retry.
    ConfirmAfterUncertain,
}

impl IdempotencyClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SafeRetry => "safe_retry",
            Self::UnsafeRetry => "unsafe_retry",
            Self::SameKey => "same_key",
            Self::ConfirmAfterUncertain => "confirm_after_uncertain",
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// P69.D25 — canonical id additions (Space → Project → Workspace; the
// Work → Run → Step → Effect spine; the binding/passport/event identities).
// ────────────────────────────────────────────────────────────────────────

id_newtype!(
    /// The top-level ownership/memory scope (a Space holds Projects).
    SpaceId
);
id_newtype!(
    /// A workspace root inside a project (the filesystem identity).
    WorkspaceId
);
id_newtype!(
    /// A step inside a run — the `Work → Run → Step` execution spine.
    StepId
);
id_newtype!(
    /// A single requested/performed effect.
    EffectId
);
id_newtype!(
    /// A canonical `WorkEvent` envelope identity.
    EventId
);
id_newtype!(
    /// A durable agent binding (an agent attached to a Work).
    AgentBindingId
);
id_newtype!(
    /// A context snapshot identity.
    SnapshotId
);
id_newtype!(
    /// A context passport identity (semantic hand-off on switch/spawn).
    PassportId
);

// ────────────────────────────────────────────────────────────────────────
// P69.D25 — the canonical schema vocabulary (contracts named by
// ARCH/CORE.md, ARCH/AGENT.md, ARCH/SESSION.md). The owning runtime lives in
// its subsystem crate; the *shape* lives here so no two crates drift.
// ────────────────────────────────────────────────────────────────────────

/// Version stamped into every canonical schema (envelopes, receipts). Bump
/// only on breaking changes; additive fields keep the number.
pub const CANONICAL_SCHEMA_VERSION: u32 = 1;

/// The canonical auth mode for an agent/harness row (P69.C11 — one spelling
/// for the whole stack; the TS union is a projection of this enum).
///
/// Per `ARCH/03-BYOK-KEYRINGS.md` §3.0, [`AuthMode::Local`] means **local
/// inference on this machine** (Ollama / llamafile / on-device) — it is *not*
/// "open source" (a license property orthogonal to authentication) and not
/// code for "not a subscription vendor". Anything needing user credentials
/// is [`AuthMode::ApiKey`]; anything the source is silent about is
/// [`AuthMode::Unknown`] and must render as unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// The agent uses its own subscription login (e.g. Claude via the official
    /// ACP wrapper — Anthropic co-authored; allowed, never token-harvested).
    Subscription,
    /// The agent uses the user's API keys (BYOK).
    ApiKey,
    /// Local inference on this machine (Ollama / llamafile / on-device).
    Local,
    /// No credential is required on every path we know of (e.g. an open
    /// keyless local endpoint). Distinct from `Local`.
    Keyless,
    /// The authoritative source (the ACP handshake, a manifest, or the user)
    /// has not stated one. Must render as unknown, never guessed.
    Unknown,
}

impl std::fmt::Display for AgentReadiness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AuthMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::ApiKey => "api_key",
            Self::Local => "local",
            Self::Keyless => "keyless",
            Self::Unknown => "unknown",
        }
    }

    /// Parse the canonical wire spelling (short form, snake_case).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "subscription" => Some(Self::Subscription),
            "api_key" | "apikey" | "api-key" => Some(Self::ApiKey),
            "local" => Some(Self::Local),
            "keyless" => Some(Self::Keyless),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// The one readiness state of an agent (P71.3f).
///
/// Before this type the same question — *"can I use this agent right now?"* —
/// was answered by scattered booleans (`installed`, `has_api_key`, a Chief
/// default flag), each of which was true for agents that could not actually
/// run. The distinction this type exists to enforce is stated once, here:
/// **installed ≠ launchable ≠ protocol-compatible ≠ authenticated ≠ ready**,
/// and *allowed as a subagent* is a further, separate policy step
/// ([`Self::can_delegate`] is the readiness half of it; budget and workspace
/// policy belong to the delegation policy).
///
/// It is a **runtime fact**, never user policy (disabled/paused) and never
/// install activity (installing/updating): those are projections on top.
/// Every reader — the picker, the resolver, the trigger plane, the delegation
/// gate — reads this one value, and an unprobed agent is [`Self::Unknown`]
/// rather than a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentReadiness {
    /// Nothing has probed this agent yet. Must render as unknown, never as
    /// available.
    Unknown,
    /// The catalog/registry knows the agent exists; no runtime is present.
    Discovered,
    /// A runtime is present on this machine (managed install or PATH/App-Paths
    /// discovery). It has not been proven launchable here.
    Installed,
    /// The runtime can be started on this platform (its package manager or
    /// executable resolves; WSL is a distinct launch path and counts).
    Launchable,
    /// A launched process negotiated a compatible protocol version
    /// (`initialize` succeeded).
    ProtocolCompatible,
    /// The agent requires authentication in **its own** store before it can
    /// serve (`authMethods` advertised and no credential evidence yet).
    AuthRequired,
    /// An authentication flow is in progress. Not usable yet.
    Authenticating,
    /// Launchable, protocol-compatible and authenticated: the only state in
    /// which an agent may serve a turn or receive a delegated Work.
    Ready,
    /// Usable with a stated reduction (a capability or provider is unavailable,
    /// a partial negotiation). The reduction must be shown, not hidden.
    Degraded,
    /// Present but not usable in this environment (platform unsupported,
    /// missing dependency). Distinct from [`Self::Discovered`]: something was
    /// found and cannot run.
    Unavailable,
    /// The last launch/negotiation/auth attempt failed. Terminal for this
    /// attempt; a retry is a new attempt, never a silent state reset.
    Failed,
}

impl AgentReadiness {
    /// Stable wire spelling (the UI reads this verbatim; TS is a projection).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Discovered => "discovered",
            Self::Installed => "installed",
            Self::Launchable => "launchable",
            Self::ProtocolCompatible => "protocol_compatible",
            Self::AuthRequired => "auth_required",
            Self::Authenticating => "authenticating",
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
        }
    }

    /// Parse the canonical wire spelling. Unknown spellings fail closed to
    /// [`Self::Unknown`] — a wire value we do not understand is not "ready".
    pub fn parse(s: &str) -> Self {
        match s {
            "unknown" => Self::Unknown,
            "discovered" => Self::Discovered,
            "installed" => Self::Installed,
            "launchable" => Self::Launchable,
            "protocol_compatible" | "protocol-compatible" => Self::ProtocolCompatible,
            "auth_required" | "auth-required" => Self::AuthRequired,
            "authenticating" => Self::Authenticating,
            "ready" => Self::Ready,
            "degraded" => Self::Degraded,
            "unavailable" => Self::Unavailable,
            "failed" => Self::Failed,
            _ => Self::Unknown,
        }
    }

    /// A runtime is present (managed install or discovery).
    pub fn is_installed(self) -> bool {
        matches!(
            self,
            Self::Installed
                | Self::Launchable
                | Self::ProtocolCompatible
                | Self::AuthRequired
                | Self::Authenticating
                | Self::Ready
                | Self::Degraded
        )
    }

    /// The runtime can be started here.
    pub fn is_launchable(self) -> bool {
        matches!(
            self,
            Self::Launchable
                | Self::ProtocolCompatible
                | Self::AuthRequired
                | Self::Authenticating
                | Self::Ready
                | Self::Degraded
        )
    }

    /// A launched process proved protocol compatibility.
    pub fn is_negotiated(self) -> bool {
        matches!(
            self,
            Self::ProtocolCompatible
                | Self::AuthRequired
                | Self::Authenticating
                | Self::Ready
                | Self::Degraded
        )
    }

    /// The agent may serve a turn (the top rung, or the top rung with a stated
    /// reduction).
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready | Self::Degraded)
    }

    /// The agent still needs the user (or itself) to authenticate.
    pub fn needs_auth(self) -> bool {
        matches!(self, Self::AuthRequired | Self::Authenticating)
    }

    /// The readiness half of "allowed as a subagent": only a ready agent may
    /// receive a delegated child Work. `installed` is **not** enough, which is
    /// exactly the conflation this type removes.
    pub fn can_delegate(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// A terminal failure state (as opposed to not-yet-probed).
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Unavailable | Self::Failed)
    }

    /// The wire spelling is also the display spelling: one vocabulary, no
    /// second human-facing name to drift from it.
    pub fn display(self) -> &'static str {
        self.as_str()
    }

    /// Short human phrase for a refusal message ("why can't I run this?").
    pub fn summary(self) -> &'static str {
        match self {
            Self::Unknown => "readiness unknown — not probed",
            Self::Discovered => "known agent, no runtime present",
            Self::Installed => "runtime present but not proven launchable here",
            Self::Launchable => "launchable, protocol not negotiated yet",
            Self::ProtocolCompatible => "protocol negotiated, authentication not confirmed",
            Self::AuthRequired => "authentication required (the agent's own store)",
            Self::Authenticating => "authentication in progress",
            Self::Ready => "ready",
            Self::Degraded => "ready with a stated reduction",
            Self::Unavailable => "present but not usable in this environment",
            Self::Failed => "the last attempt failed",
        }
    }
}

/// How our app drives an agent (canonical home; `everyaios-acp`'s
/// `HarnessProtocol` folds into this in P69.D1).
///
/// ADR-0005: v1 engines are external agents, so `Acp` and the model-only
/// binding are the whole vocabulary. The retired built-in identities —
/// `Inbuilt` (our native engine) and `ModelBackend` (configuring a CLI's
/// model endpoint through us) — are gone; they return post-v1 with the
/// governed baseline binding and must then obey I23/I24 like any agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProtocol {
    /// Driven via ACP stdio.
    Acp,
    /// No subprocess and no loop of ours: the binding is a model-only brain
    /// (a v1 bundle with a model pin, chat-only, no tools). The inference
    /// path is deferred with the built-in engine, so a row with this protocol
    /// must never render as runnable while that is true.
    ModelOnly,
}

/// The kind of a Session (`ARCH/ADR/0006`).
///
/// A **property of the Session record**, never inferred from whether a Chat
/// happens to exist. `Interactive` keeps the Chat↔Session 1:1 rule;
/// `Automation` is a trigger-created Session with **no Chat** (the same way
/// `project_id = null` is normal — not a degraded state); `Delegated` is
/// reserved for an out-of-session delegation the user explicitly starts, not
/// for ordinary child Work (**I8** — a child lives in its parent's Session).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// A user's chat Session (the existing rows; the default).
    #[default]
    Interactive,
    /// Created by a trigger (scheduler / workflow); owns headless Work.
    Automation,
    /// An out-of-session delegation the user explicitly started.
    Delegated,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Automation => "automation",
            Self::Delegated => "delegated",
        }
    }

    /// Parse the canonical wire spelling; unknown spellings read as
    /// `Interactive` — existing rows predate the field and are interactive.
    pub fn parse(s: &str) -> Self {
        match s {
            "automation" => Self::Automation,
            "delegated" => Self::Delegated,
            _ => Self::Interactive,
        }
    }

    /// Whether a Chat may exist for this Session. Non-interactive Sessions
    /// normally have none; a Chat **may be created from** one later (ADR-0006
    /// §7 — the user opens a run to inspect or continue it), after which the
    /// 1:1 rule holds again. This predicate is about *creation*, not existence:
    /// nothing may **infer** the kind from a Chat's presence.
    pub fn chat_is_normal(self) -> bool {
        matches!(self, Self::Interactive)
    }
}

impl std::fmt::Display for SessionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The audit/governance mode of an external agent connection (`ARCH/
/// EXTERNAL-AGENTS.md` §5): every capability surface states which of these it
/// is, and audit coverage is stated per mode — never implied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentGovernanceMode {
    /// Every effect is mediated by EveryAIOS (Guard → ticket → executor).
    GovernedMediated,
    /// Permission callbacks are mediated, but effects performed inside the
    /// agent's own process are outside the EveryAIOS audit trail.
    SelfContained,
    /// No mediation and no audit coverage; must render as such.
    NotGoverned,
}

/// The `state` of a durable [`AgentBinding`] (`ARCH/AGENT.md` §3).
///
/// `Active` · `Parked` · `Resuming` move through the named lifecycle events
/// (`AgentBindingCreated` · `Activated` · `Suspended` · `Resumed`,
/// `ARCH/WORK.md`). `Dead` and `Unavailable` are consequence states on the
/// same record — `ARCH/AGENT.md` §3 names no event for them, so their
/// derivation belongs to the AgentBridge event bridge (`P69.B4`). `Dead`
/// never resumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingLifecycle {
    Active,
    Parked,
    Resuming,
    Dead,
    Unavailable,
}

/// Token/cost accounting carried per binding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    /// Cost in micro-currency units (1e-6 of the ledger's unit) — integers,
    /// never floats on a wire boundary.
    #[serde(default)]
    pub cost_micros: u64,
}

/// The canonical definition of an agent (P69.D1: `everyaios-agents` owns the
/// registry; this is the record shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub id: AgentId,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub protocol: AgentProtocol,
    pub auth_mode: AuthMode,
    #[serde(default)]
    pub capabilities: Vec<CapabilityId>,
    /// Extension mechanisms the adapter negotiated (hook names etc.) — data,
    /// never kernel branches (`ARCH/AGENT.md` §5.3).
    #[serde(default)]
    pub extension_mechanisms: Vec<String>,
}

/// A durable agent binding — an agent attached to a Work. The binding, not
/// the process, is the unit that survives a restart (`ARCH/AGENT.md` §3);
/// `provider_session_id` is deliberately distinct from the EveryAIOS
/// [`SessionId`] (`ARCH/SESSION.md`). A Session owns bindings; exactly one is
/// `active` (`ARCH/CORE.md` §7.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBinding {
    pub binding_id: AgentBindingId,
    pub session_id: SessionId,
    pub work_id: WorkId,
    pub agent_id: AgentId,
    /// Reference into `everyaios-agents`' adapter registry (`P69.B3`); the
    /// binding names the adapter, it never embeds one.
    #[serde(default)]
    pub adapter_id: Option<String>,
    /// How the runtime drives the agent — the canonical vocabulary from
    /// `AgentDefinition` (`P69.D1`; `ARCH/AGENT.md` §3's `acp | stdio |
    /// in-process` is the illustrative form of this enum).
    pub protocol: AgentProtocol,
    /// The agent's own session identity (resume handle). Never the EveryAIOS
    /// session id — conflating them is how resume breaks.
    #[serde(default)]
    pub provider_session_id: Option<String>,
    /// Provider model + mode in force for this binding, when the adapter
    /// supports switching them (`ARCH/AGENT.md` §5 — negotiated, never
    /// assumed).
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    /// The resolved effective capability set (`ARCH/AGENT.md` §3).
    #[serde(default)]
    pub capability_manifest: Vec<CapabilityId>,
    pub governance_mode: AgentGovernanceMode,
    /// Reference to the short-lived, work/binding-scoped bridge credential
    /// once `AgentBridge` lands (`P69.B4`); the bridge owns the credential,
    /// the binding only names it.
    #[serde(default)]
    pub bridge_id: Option<String>,
    pub state: BindingLifecycle,
    #[serde(default)]
    pub usage: BindingUsage,
    #[serde(default)]
    pub last_event_seq: u64,
    /// Handle into agent-private state owned by the adapter (opaque to us).
    #[serde(default)]
    pub private_state_ref: Option<String>,
}

/// How much model control an agent binding exposes for a runtime.
///
/// This is capability evidence, not authority to run inference. A runtime may
/// list a model without the bound agent being able to select or use it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeControl {
    /// The agent owns provider, model, authentication, and fallback.
    NativeOnly,
    /// A verified launch environment may override provider/model settings.
    LaunchOverride,
    /// The agent advertises ACP `ConfigOption`s for per-session model control.
    SessionConfig,
    /// The runtime has not been probed, so no model control may be offered.
    #[default]
    Unknown,
}

/// Who owns a runtime's lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOwnership {
    /// EveryAIOS owns the process handle and its stop/restart lifecycle.
    Managed,
    /// The user already runs the process; discovery may observe but never stop it.
    External,
    /// The runtime runs on another machine and is not locally manageable.
    Remote,
}

/// Evidence-backed health for a runtime endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHealthState {
    /// No health evidence has been collected.
    #[default]
    Unknown,
    /// The endpoint is reachable, but no full health probe has completed.
    Observed,
    /// A full probe verified the runtime is ready for use.
    Healthy,
    /// The runtime responds with reduced capability or an explicit warning.
    Degraded,
    /// A previously observed runtime is no longer reachable.
    Down,
    /// The endpoint or required runtime protocol is not supported here.
    Unsupported,
}

/// The five-level model-control ladder exposed as UI data.
///
/// Higher tiers add control but do not imply that an external agent will use
/// a listed model. Tier four is reserved and refused in v1 because the host
/// must not become a managed reasoning engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ModelControlTier {
    /// Tier 0: inventory/listing only.
    Inventory = 0,
    /// Tier 1: the agent owns provider and model selection.
    NativeOnly = 1,
    /// Tier 2: a verified launch environment can override provider/model settings.
    LaunchOverride = 2,
    /// Tier 3: the binding advertises ACP `ConfigOption`s for session control.
    SessionConfig = 3,
    /// Tier 4: fully managed inference; reserved and refused in v1.
    FullyManagedInference = 4,
}

impl ModelControlTier {
    /// Returns the stable numeric rung used by UI projections.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Agent-specific evidence about whether a runtime can be used.
///
/// `usable = None` and `control = RuntimeControl::Unknown` are the safe
/// defaults before a binding-specific probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRuntimeCompatibility {
    /// Canonical identity of the agent binding this evidence concerns.
    pub agent_id: AgentId,
    /// Maximum verified model-control surface exposed by that binding.
    pub control: RuntimeControl,
    /// Whether the agent can use this runtime, or `None` when not probed.
    pub usable: Option<bool>,
    /// Probe evidence or the explicit reason usability is unavailable.
    #[serde(default)]
    pub reason: Option<String>,
}

/// One observed model-runtime endpoint in the canonical inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeInventoryEntry {
    /// Stable identity for this observed endpoint.
    pub id: String,
    /// Runtime kind proven by a handshake, or `generic_openai_compatible` as fallback.
    pub kind: String,
    /// Base endpoint suitable for runtime-specific client construction.
    pub endpoint: String,
    /// Runtime-reported version, when a handshake exposes one.
    pub version: Option<String>,
    /// Wire protocol proven by the handshake, or `unknown` when unproven.
    pub protocol: String,
    /// Lifecycle owner; inventory discovery never infers managed ownership.
    pub ownership: RuntimeOwnership,
    /// Current health evidence without agent-usability inference.
    pub health: RuntimeHealthState,
    /// Unix epoch milliseconds of the last completed probe, when one occurred.
    pub last_probe_ms: Option<u64>,
    /// Model identifiers listed by the runtime; never proof of agent usability.
    #[serde(default)]
    pub models: Vec<String>,
    /// Binding-specific usability evidence, empty until explicitly probed.
    #[serde(default)]
    pub agent_compatibility: Vec<AgentRuntimeCompatibility>,
}

/// The canonical effect request: every mutating operation reduces to this
/// shape before it reaches Guard and the executor (`ARCH/CORE.md` §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectRequest {
    pub effect_id: EffectId,
    #[serde(default)]
    pub work_id: Option<WorkId>,
    #[serde(default)]
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub step_id: Option<StepId>,
    #[serde(default)]
    pub binding_id: Option<AgentBindingId>,
    pub capability_id: CapabilityId,
    pub operation: String,
    pub args_hash: ArgsHash,
    #[serde(default)]
    pub resources: Vec<ResourceId>,
    pub idempotency: IdempotencyClass,
    /// Authorization provenance (agent ticket vs human gesture vs policy) —
    /// the corrected form, never "everything is ticketed".
    pub requested_by: GovernanceMode,
    pub risk: RiskLevel,
    pub created_at_ms: u64,
    #[serde(default)]
    pub audit_seq: Option<u64>,
}

/// A capability request (scoped, bounded) — the input to the capability
/// resolver/broker; a grant is materialized as an opaque handle, never a
/// secret (`ARCH/CAPABILITIES.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityRequest {
    pub capability_id: CapabilityId,
    #[serde(default)]
    pub binding_id: Option<AgentBindingId>,
    #[serde(default)]
    pub work_id: Option<WorkId>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub expires_at_ms: Option<u64>,
}

/// One node of a context snapshot (a source event the model can see, an
/// injected system block, a replacement, or a by-reference pointer).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextNode {
    pub node_id: String,
    /// Where the node came from (`user`, `assistant`, `tool`, `system`, …).
    pub source: String,
    /// Visible to the model vs retained-but-not-injected.
    #[serde(default)]
    pub visible: bool,
    #[serde(default)]
    pub tokens: u64,
    /// If this node replaces earlier content, the node it replaced.
    #[serde(default)]
    pub replacement: Option<String>,
    /// If the node is a by-reference pointer (pass-by-reference), its handle.
    #[serde(default)]
    pub reference: Option<String>,
}

/// The durable projection of what was in front of a binding at a point in
/// time (`ARCH/CONTEXT.md`): source events, visible/injected nodes,
/// replacements, references, token estimate, and the cache boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub snapshot_id: SnapshotId,
    pub work_id: WorkId,
    #[serde(default)]
    pub binding_id: Option<AgentBindingId>,
    /// The WorkEvent sequence this snapshot reflects.
    pub event_seq: u64,
    #[serde(default)]
    pub nodes: Vec<ContextNode>,
    #[serde(default)]
    pub token_estimate: u64,
    /// Everything at/ below this sequence is cache-stable prefix.
    pub cache_boundary_seq: u64,
    #[serde(default)]
    pub compacted: bool,
}

/// A bounded semantic continuation state for a binding switch or child spawn
/// (`ARCH/AGENT.md` §5.2) — execution provenance plus continuation, **never a
/// transcript**.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPassport {
    pub passport_id: PassportId,
    pub work_id: WorkId,
    #[serde(default)]
    pub from_binding: Option<AgentBindingId>,
    #[serde(default)]
    pub to_binding: Option<AgentBindingId>,
    /// Why the passport was generated (switch / child spawn / resume).
    pub reason: String,
    /// Bounded semantic continuation text.
    #[serde(default)]
    pub continuation: String,
    /// Size guard — passports are bounded, full stop.
    #[serde(default)]
    pub byte_len: u32,
    pub created_at_ms: u64,
}

/// The versioned canonical event envelope (`ARCH/WORK.md` §canonical event
/// vocabulary). `kind` is one of the 30 canonical event names; the envelope
/// carries identity, ordering, and schema version so every writer and every
/// projection agree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event_id: EventId,
    /// Monotonic per-Work sequence.
    pub seq: u64,
    pub work_id: WorkId,
    #[serde(default)]
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub step_id: Option<StepId>,
    /// Canonical event name (e.g. `work.started`, `effect.committed`).
    pub kind: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    pub at_ms: u64,
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
}

fn default_schema_version() -> u32 {
    CANONICAL_SCHEMA_VERSION
}

/// Uncertainty classification of an attempted effect (`ARCH/RECOVERY.md`):
/// approved-but-unattempted / outcome-unknown must never read as complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectUncertainty {
    /// The outcome is known.
    None,
    /// The attempt may or may not have landed (e.g. transport dropped).
    UnknownOutcome,
    /// The effect landed with a gap (partial write, skipped step).
    Partial,
}

/// The canonical receipt view for one effect (P47.5's `EffectReceipt` shape,
/// schema-home here so every receipt writer emits the same view).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectReceiptView {
    pub receipt_id: ReceiptId,
    pub effect_id: EffectId,
    #[serde(default)]
    pub work_id: Option<WorkId>,
    /// Whether the effect was authorized (ticket or trusted gesture).
    pub authorized: bool,
    #[serde(default)]
    pub resource: Option<ResourceId>,
    #[serde(default)]
    pub before_ref: Option<String>,
    #[serde(default)]
    pub after_ref: Option<String>,
    #[serde(default)]
    pub diff_hash: Option<String>,
    #[serde(default)]
    pub rollback_ref: Option<String>,
    #[serde(default)]
    pub has_gap: bool,
    #[serde(default = "default_uncertainty")]
    pub uncertainty: EffectUncertainty,
}

fn default_uncertainty() -> EffectUncertainty {
    EffectUncertainty::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newtype_ids_are_opaque_and_round_trip() {
        let w = WorkId::new("w-1");
        assert_eq!(w.as_str(), "w-1");
        assert_eq!(w.to_string(), "w-1");
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(json, "\"w-1\""); // transparent: serializes as the string
        let back: WorkId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, w);
        // A WorkId is not comparable to a TicketId (compile-time safety).
        // (No runtime assertion needed — this is a type-level guarantee.)
    }

    #[test]
    fn every_id_serializes_transparent() {
        for s in [
            serde_json::to_string(&WorkId::new("x")).unwrap(),
            serde_json::to_string(&TicketId::new("t")).unwrap(),
            serde_json::to_string(&ReceiptId::new("r")).unwrap(),
            serde_json::to_string(&ConfigHash::new("c")).unwrap(),
        ] {
            assert!(s.starts_with('"') && s.ends_with('"'));
        }
    }

    #[test]
    fn canonical_auth_mode_has_one_spelling() {
        for (mode, s) in [
            (AuthMode::Subscription, "subscription"),
            (AuthMode::ApiKey, "api_key"),
            (AuthMode::Local, "local"),
            (AuthMode::Keyless, "keyless"),
            (AuthMode::Unknown, "unknown"),
        ] {
            assert_eq!(mode.as_str(), s);
            assert_eq!(AuthMode::parse(s), Some(mode));
        }
        // The legacy TS `_cli` spelling is tolerated on read, never emitted.
        assert_eq!(AuthMode::parse("api_key_cli"), None);
    }

    #[test]
    fn spine_contracts_round_trip() {
        let req = EffectRequest {
            effect_id: EffectId::new("e-1"),
            work_id: Some(WorkId::new("w-1")),
            run_id: Some(RunId::new("r-1")),
            step_id: Some(StepId::new("s-1")),
            binding_id: None,
            capability_id: CapabilityId::new("office.docx_patch"),
            operation: "patch".into(),
            args_hash: ArgsHash::new("abc"),
            resources: vec![ResourceId::new("file/doc.docx")],
            idempotency: IdempotencyClass::UnsafeRetry,
            requested_by: GovernanceMode::AgentTicket,
            risk: RiskLevel::High,
            created_at_ms: 1,
            audit_seq: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"agent_ticket\""));
        let back: EffectRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);

        let env = EventEnvelope {
            event_id: EventId::new("ev-1"),
            seq: 7,
            work_id: WorkId::new("w-1"),
            run_id: None,
            step_id: None,
            kind: "effect.committed".into(),
            payload: serde_json::json!({ "capability": "ok" }),
            at_ms: 42,
            schema_version: CANONICAL_SCHEMA_VERSION,
        };
        let back: EventEnvelope =
            serde_json::from_str(&serde_json::to_string(&env).unwrap()).unwrap();
        assert_eq!(back, env);

        let binding = AgentBinding {
            binding_id: AgentBindingId::new("b-1"),
            session_id: SessionId::new("s-1"),
            work_id: WorkId::new("w-1"),
            agent_id: AgentId::new("opencode"),
            adapter_id: None,
            protocol: AgentProtocol::Acp,
            provider_session_id: Some("agent-side-42".into()),
            model: Some("claude-sonnet-4-5".into()),
            mode: Some("default".into()),
            capability_manifest: vec![],
            governance_mode: AgentGovernanceMode::SelfContained,
            bridge_id: None,
            state: BindingLifecycle::Parked,
            usage: BindingUsage::default(),
            last_event_seq: 3,
            private_state_ref: None,
        };
        assert_ne!(
            binding.session_id.as_str(),
            binding.provider_session_id.clone().unwrap()
        );
        let _ = serde_json::to_string(&binding).unwrap();
    }

    #[test]
    fn readiness_wire_spelling_is_total_and_fails_closed() {
        let all = [
            AgentReadiness::Unknown,
            AgentReadiness::Discovered,
            AgentReadiness::Installed,
            AgentReadiness::Launchable,
            AgentReadiness::ProtocolCompatible,
            AgentReadiness::AuthRequired,
            AgentReadiness::Authenticating,
            AgentReadiness::Ready,
            AgentReadiness::Degraded,
            AgentReadiness::Unavailable,
            AgentReadiness::Failed,
        ];
        for state in all {
            assert_eq!(AgentReadiness::parse(state.as_str()), state);
            let wire = serde_json::to_string(&state).unwrap();
            assert_eq!(wire, format!("\"{}\"", state.as_str()));
            assert_eq!(
                serde_json::from_str::<AgentReadiness>(&wire).unwrap(),
                state
            );
        }
        // A spelling we do not understand is unknown — never ready.
        assert_eq!(
            AgentReadiness::parse("probably_fine"),
            AgentReadiness::Unknown
        );
    }

    #[test]
    fn readiness_distinguishes_installed_from_ready_and_delegable() {
        assert!(AgentReadiness::Installed.is_installed());
        assert!(!AgentReadiness::Installed.is_launchable());
        assert!(!AgentReadiness::Installed.is_ready());
        assert!(!AgentReadiness::Installed.can_delegate());

        assert!(AgentReadiness::Launchable.is_launchable());
        assert!(!AgentReadiness::Launchable.is_negotiated());

        assert!(AgentReadiness::AuthRequired.is_negotiated());
        assert!(AgentReadiness::AuthRequired.needs_auth());
        assert!(!AgentReadiness::AuthRequired.can_delegate());

        // Degraded serves a turn but is not admissible as a subagent.
        assert!(AgentReadiness::Degraded.is_ready());
        assert!(!AgentReadiness::Degraded.can_delegate());
        assert!(AgentReadiness::Ready.can_delegate());

        assert!(AgentReadiness::Failed.is_failure());
        assert!(AgentReadiness::Unavailable.is_failure());
        assert!(!AgentReadiness::Discovered.is_failure());
        assert!(!AgentReadiness::Unknown.is_installed());
    }

    #[test]
    fn work_state_covers_the_work_contract_and_keeps_recoverable_distinct() {
        // WORK.md §4 — the full set, with the Running sub-states grouped.
        let all = [
            WorkState::Created,
            WorkState::Planning,
            WorkState::Ready,
            WorkState::Running,
            WorkState::WaitingTool,
            WorkState::WaitingApproval,
            WorkState::WaitingUser,
            WorkState::Checkpointed,
            WorkState::Verifying,
            WorkState::Completed,
            WorkState::Failed,
            WorkState::Cancelled,
            WorkState::Paused,
            WorkState::Recoverable,
        ];
        for state in all {
            assert_eq!(WorkState::parse(state.as_str()), state);
            let wire = serde_json::to_string(&state).unwrap();
            assert_eq!(wire, format!("\"{}\"", state.as_str()));
        }
        // A spelling we do not understand is never reported as success.
        assert_eq!(WorkState::parse("done_maybe"), WorkState::Created);

        assert!(WorkState::Running.is_running());
        assert!(WorkState::WaitingApproval.is_running());
        assert!(WorkState::Checkpointed.is_running());
        assert!(!WorkState::Paused.is_running());
        assert_eq!(WorkState::Running.run_phase(), Some(WorkRunPhase::Active));
        assert_eq!(
            WorkState::WaitingUser.run_phase(),
            Some(WorkRunPhase::WaitingUser)
        );
        assert_eq!(WorkState::Verifying.run_phase(), None);
        assert_eq!(
            WorkRunPhase::WaitingTool.work_state(),
            WorkState::WaitingTool
        );

        // The distinction the contract exists to keep.
        assert!(WorkState::Completed.is_terminal());
        assert!(WorkState::Failed.is_terminal());
        assert!(WorkState::Cancelled.is_terminal());
        assert!(!WorkState::Recoverable.is_terminal());
        assert!(WorkState::Recoverable.is_resumable());
        assert!(!WorkState::Failed.is_resumable());
    }

    #[test]
    fn wait_conditions_name_their_state_and_survive_the_wire() {
        assert_eq!(
            WaitReason::Approval.work_state(),
            WorkState::WaitingApproval
        );
        assert_eq!(WaitReason::UserInput.work_state(), WorkState::WaitingUser);
        // The reasons with no dedicated `Waiting*` state park as Paused.
        for reason in [
            WaitReason::Timer,
            WaitReason::ExternalEvent,
            WaitReason::Resource,
            WaitReason::Agent,
            WaitReason::Retry,
        ] {
            assert_eq!(reason.work_state(), WorkState::Paused);
            assert_eq!(WaitReason::parse(reason.as_str()), Some(reason));
        }
        assert_eq!(WaitReason::parse("whenever"), None);

        let wait = WaitCondition::new(WaitReason::Approval)
            .with_detail("ticket tkt:42 pending")
            .with_resume_on("tkt:42");
        assert_eq!(wait.work_state(), WorkState::WaitingApproval);
        let round: WaitCondition =
            serde_json::from_str(&serde_json::to_string(&wait).unwrap()).unwrap();
        assert_eq!(round, wait);
    }

    #[test]
    fn checkpoint_ids_are_deterministic_and_optional_on_legacy_rows() {
        let id = CheckpointId::for_step("w-1", 3);
        assert_eq!(id.as_str(), "ckpt:w-1/3");
        assert_eq!(CheckpointId::for_step("w-1", 3), id);
        assert!(id.is_assigned());
        assert!(!CheckpointId::unassigned().is_assigned());
        assert!(!CheckpointId::default().is_assigned());
    }

    #[test]
    fn session_kind_is_a_record_property_not_a_chat_inference() {
        // Wire round-trip + the default for existing rows.
        for kind in [
            SessionKind::Interactive,
            SessionKind::Automation,
            SessionKind::Delegated,
        ] {
            let wire = serde_json::to_string(&kind).unwrap();
            assert_eq!(wire, format!("\"{}\"", kind.as_str()));
            assert_eq!(serde_json::from_str::<SessionKind>(&wire).unwrap(), kind);
            assert_eq!(SessionKind::parse(kind.as_str()), kind);
        }
        // Unknown spellings are existing rows → interactive, never automation.
        assert_eq!(SessionKind::parse("whatever"), SessionKind::Interactive);
        assert_eq!(SessionKind::default(), SessionKind::Interactive);
        assert!(SessionKind::Interactive.chat_is_normal());
    }

    #[test]
    fn enums_round_trip_and_are_versioned_names() {
        let lvl: RiskLevel = serde_json::from_str("\"critical\"").unwrap();
        assert_eq!(lvl, RiskLevel::Critical);
        let act: AutonomyLevel = serde_json::from_str("\"maximum\"").unwrap();
        assert_eq!(act, AutonomyLevel::Maximum);
        assert_eq!(IdempotencyClass::UnsafeRetry.as_str(), "unsafe_retry");
    }
}
