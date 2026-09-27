//! The capability descriptor contract (DM-011 · DM-012 · CTR-009).
//!
//! A capability describes **what** can be done, never **who** does it: no
//! protocol, vendor, or tool name appears in a capability id or descriptor
//! (`ARCH/13-CAPABILITY.md` §1 rule 1). Provider assignment is a registry fact
//! derived at registration, never a copy-edited part of the contract
//! (REQ-CAP-003).
//!
//! This module holds the *contract* — the value types plus the id/naming rules
//! the census gate enforces. It holds no registry, no provider state, and no
//! credential material: it is safe to carry across the coordinator/core
//! boundary.
//!
//! Three shapes live here:
//!
//! - [`CapabilityDescriptor`] — what the caller can express, what it requires,
//!   how it loads, how risky it is, and what must run before a receipt.
//! - [`CapabilityResult`] — `completed` / `guidance` / `requires_user_action` /
//!   `failed`, where guidance and user-action are **results**, not failures
//!   (`ARCH/13` §3).
//! - [`CapabilityHandle`] — the epoch-checked binding handed back by the
//!   resolver; a provider restart invalidates it (`ARCH/13` §4).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The contract revision this module implements. A descriptor that changes
/// shape carries the version it was minted under, so a run keeps serving the
/// descriptor it started with (`ARCH/13` §8).
pub const CAPABILITY_CONTRACT_VERSION: u32 = 1;

/// Default handle lifetime when a caller names no TTL (`OQ-CAP-3` default).
/// Deliberately short: a handle is a hot-path binding, not a lease.
pub const DEFAULT_HANDLE_TTL_MS: u64 = 120_000;

// ---------------------------------------------------------------------------
// Invocation (secret-free)
// ---------------------------------------------------------------------------

/// The secret-free invocation metadata the broker checks a grant against.
///
/// This contract deliberately contains no credential material. It is safe to
/// carry across the coordinator/core boundary; the vault remains the only
/// component that resolves and injects secrets (`CTR-013`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityInvocation {
    pub grant_id: String,
    pub run_id: String,
    pub capability: String,
    pub operation: String,
}

impl CapabilityInvocation {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.grant_id.is_empty() {
            return Err("capability grant id is required");
        }
        if self.run_id.is_empty() {
            return Err("capability run id is required");
        }
        if self.capability.is_empty() {
            return Err("capability scope is required");
        }
        if self.operation.is_empty() {
            return Err("capability operation is required");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Risk class (INV-19 · `ARCH/34` §3)
// ---------------------------------------------------------------------------

/// The declared risk of a capability. Maps to policy defaults
/// (`ARCH/12` §3) and to the **minimum** verification depth that must run
/// before a receipt is written (INV-19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRisk {
    /// Deterministic validation where meaningful; otherwise trivially
    /// observable effect (metadata reads, scaffold creation).
    Safe,
    /// Validation plus a re-read/render check (file write → re-open + hash).
    Sensitive,
    /// Validation plus render/inspection **and** reconciliation; confirmation
    /// surfaces where required (delete/wipe → count + audit).
    // The `Default`: an absent risk class must be the strictest floor, so a
    // descriptor that forgets to declare one can never under-verify.
    #[default]
    Dangerous,
}

impl CapabilityRisk {
    /// The wire token (census-stable, human-readable in a manifest).
    pub const fn as_str(self) -> &'static str {
        match self {
            CapabilityRisk::Safe => "safe",
            CapabilityRisk::Sensitive => "sensitive",
            CapabilityRisk::Dangerous => "dangerous",
        }
    }

    /// Parse the wire token; `None` for anything else (a malformed descriptor
    /// is a census finding, never a silent default to `safe`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "safe" => Some(CapabilityRisk::Safe),
            "sensitive" => Some(CapabilityRisk::Sensitive),
            "dangerous" => Some(CapabilityRisk::Dangerous),
            _ => None,
        }
    }

    /// The **floor** of verification depth this class demands. The matrix is a
    /// floor, never a ceiling (`REQ-VERIFY-001`); a domain may declare more.
    pub const fn verification_depth(self) -> VerificationDepth {
        match self {
            CapabilityRisk::Safe => VerificationDepth::Validate,
            CapabilityRisk::Sensitive => VerificationDepth::ValidateAndReread,
            CapabilityRisk::Dangerous => VerificationDepth::ValidateAndReconcile,
        }
    }
}

/// How much verification a capability's effects must survive before a receipt
/// is written (`ARCH/34` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationDepth {
    /// Deterministic validation of the produced effect.
    Validate,
    /// Validation plus an independent re-read/render of the effect.
    ValidateAndReread,
    /// Validation plus re-read plus reconciliation against the intent.
    ValidateAndReconcile,
}

impl VerificationDepth {
    pub const fn as_str(self) -> &'static str {
        match self {
            VerificationDepth::Validate => "validate",
            VerificationDepth::ValidateAndReread => "validate_reread",
            VerificationDepth::ValidateAndReconcile => "validate_reconcile",
        }
    }

    /// Is this depth at least as strong as the `floor`? (The matrix is a
    /// floor, so a descriptor may exceed it and may never fall below it.)
    pub const fn satisfies(self, floor: VerificationDepth) -> bool {
        (self as u8) >= (floor as u8)
    }
}

// ---------------------------------------------------------------------------
// Loading modes (`ARCH/13` §6)
// ---------------------------------------------------------------------------

/// What the model, the catalog, and the resolver each see. The three modes are
/// the semantic-compression layer: an agent holds a budgeted subset, never a
/// flat dump of raw tools (REQ-CAP-001 · REQ-CAP-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoadingMode {
    /// Always known for the active agent/task — compact definitions in model
    /// context.
    Eager,
    /// Known to the UI/registry (catalog, capability browser, loadout editor),
    /// **not** to the model.
    Catalog,
    /// Resolved only when invoked — zero context cost until requested.
    OnDemand,
}

impl LoadingMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            LoadingMode::Eager => "eager",
            LoadingMode::Catalog => "catalog",
            LoadingMode::OnDemand => "on-demand",
        }
    }

    /// Parse the wire token; `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "eager" => Some(LoadingMode::Eager),
            "catalog" => Some(LoadingMode::Catalog),
            "on-demand" => Some(LoadingMode::OnDemand),
            _ => None,
        }
    }

    /// May this mode appear in a model-facing exposure? Only `eager` may
    /// (unprompted), and `on-demand` only when the turn asked for it. A
    /// `catalog` entry never enters model context unrequested.
    pub const fn may_expose_to_model(self, requested: bool) -> bool {
        match self {
            LoadingMode::Eager => true,
            LoadingMode::Catalog => requested,
            LoadingMode::OnDemand => requested,
        }
    }
}

// ---------------------------------------------------------------------------
// Requirements, auth, verification
// ---------------------------------------------------------------------------

/// What kind of thing a capability requires before it can be invoked. These are
/// the `requires →` edges of the capability graph (`ARCH/13` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequirementKind {
    /// Another capability must resolve first.
    Capability,
    /// A connected account/app must exist (connector id).
    Connection,
    /// A runtime environment must exist and be selectable (environment id).
    Environment,
}

/// One `requires` edge. A blocked chain names the missing edge rather than
/// failing opaquely (REQ-CAP-008).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Requirement {
    pub kind: RequirementKind,
    /// The capability id, connection id, or environment id required.
    pub target: String,
    /// Why it is required, in the caller's language (a guidance result quotes
    /// this).
    pub rationale: String,
}

impl Requirement {
    pub fn capability(target: impl Into<String>, rationale: impl Into<String>) -> Self {
        Self {
            kind: RequirementKind::Capability,
            target: target.into(),
            rationale: rationale.into(),
        }
    }

    pub fn connection(target: impl Into<String>, rationale: impl Into<String>) -> Self {
        Self {
            kind: RequirementKind::Connection,
            target: target.into(),
            rationale: rationale.into(),
        }
    }

    pub fn environment(target: impl Into<String>, rationale: impl Into<String>) -> Self {
        Self {
            kind: RequirementKind::Environment,
            target: target.into(),
            rationale: rationale.into(),
        }
    }
}

/// The auth **shapes** a capability can require. Modeled as a method enum
/// only: never a value, never a token, never a header (`ARCH/14` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMethod {
    /// A key held in the vault, referenced by handle.
    Api,
    /// An authorize/callback/refresh/expiry flow whose tokens live in the vault.
    Oauth,
    /// A locally discoverable / OS-mediated identity (no stored secret).
    WellKnown,
}

impl AuthMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            AuthMethod::Api => "api",
            AuthMethod::Oauth => "oauth",
            AuthMethod::WellKnown => "well-known",
        }
    }
}

/// An auth requirement carried by a descriptor: the **method** plus a reference
/// the vault can resolve (`use`-style, `CTR-013`). `secret_ref` is a handle,
/// never a credential — the struct cannot express a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRequirement {
    pub method: AuthMethod,
    /// Vault handle / connection id. Empty when the method needs no stored
    /// secret (`well-known`).
    pub secret_ref: String,
    /// Optional prompt shown when the requirement is unmet (guidance text).
    pub prompt: String,
}

impl AuthRequirement {
    pub fn api(secret_ref: impl Into<String>) -> Self {
        Self {
            method: AuthMethod::Api,
            secret_ref: secret_ref.into(),
            prompt: String::new(),
        }
    }

    pub fn oauth(secret_ref: impl Into<String>) -> Self {
        Self {
            method: AuthMethod::Oauth,
            secret_ref: secret_ref.into(),
            prompt: String::new(),
        }
    }

    pub fn well_known() -> Self {
        Self {
            method: AuthMethod::WellKnown,
            secret_ref: String::new(),
            prompt: String::new(),
        }
    }
}

/// The verification hook: what must run before a receipt is written, and the
/// depth it is declared at (`ARCH/34`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationHook {
    /// Hook reference (a verifier id), never an inline implementation.
    pub hook: String,
    /// Depth actually declared — must be at least the risk-class floor.
    pub depth: VerificationDepth,
}

impl VerificationHook {
    pub fn new(hook: impl Into<String>, depth: VerificationDepth) -> Self {
        Self {
            hook: hook.into(),
            depth,
        }
    }
}

/// A deprecation record. A deprecated capability keeps resolving until the
/// window closes (REQ-CAP-009 · EDGE-012).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deprecation {
    /// Version in which the deprecation was declared.
    pub since_version: String,
    /// Wall-clock ms at which the window closes; `0` means "not yet dated".
    pub window_ends_ms: u64,
}

impl Deprecation {
    /// Is the window still open at `now_ms`? A capability inside its window
    /// still resolves; one past it does not.
    pub const fn serves_at(&self, now_ms: u64) -> bool {
        self.window_ends_ms == 0 || now_ms < self.window_ends_ms
    }
}

// ---------------------------------------------------------------------------
// Providers (assignment, never identity)
// ---------------------------------------------------------------------------

/// A provider **assignment** on a descriptor. The id is registry-derived; the
/// presence of this type on a descriptor is what makes "providers are derived,
/// never copy-edited" checkable (REQ-CAP-003).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAssignment {
    pub provider_id: String,
    /// The provider epoch this assignment was observed at. A bump re-resolves;
    /// it never silently rebinds a live handle (`ARCH/14` §1 rule 4).
    pub provider_epoch: u64,
}

// ---------------------------------------------------------------------------
// The descriptor
// ---------------------------------------------------------------------------

/// The versioned description of one capability (DM-011).
///
/// Field order follows the architecture doc so a reader can diff the two.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptor {
    /// Stable public API. Dotted, protocol- and vendor-neutral
    /// (`office.spreadsheet.edit`). Never renumbered, never reused.
    pub id: String,
    /// Content version, `major.minor.patch`. Additive change ⇒ minor bump;
    /// breaking change ⇒ a decision + a deprecation window (REQ-CAP-009).
    pub version: String,
    pub description: String,
    /// What the caller can express / what is supported. Never empty (census).
    pub affordances: Vec<String>,
    /// `requires →` edges. May name other capabilities (graph edges).
    pub requirements: Vec<Requirement>,
    /// Registry-derived provider assignments. The registry fills this in; a
    /// hand-authored list that disagrees is a census finding.
    pub providers: Vec<ProviderAssignment>,
    pub loading_mode: LoadingMode,
    pub risk_class: CapabilityRisk,
    /// Vault refs / connection ids — methods and handles, never values.
    pub auth_requirements: Vec<AuthRequirement>,
    /// What must run before a receipt.
    pub verification: VerificationHook,
    /// Set when the capability is deprecated; `None` while current.
    pub deprecated: Option<Deprecation>,
}

impl CapabilityDescriptor {
    /// A current (non-deprecated) descriptor. Providers start empty: the
    /// registry derives them.
    pub fn new(
        id: impl Into<String>,
        version: impl Into<String>,
        description: impl Into<String>,
        affordances: Vec<String>,
        loading_mode: LoadingMode,
        risk_class: CapabilityRisk,
        verification: VerificationHook,
    ) -> Self {
        Self {
            id: id.into(),
            version: version.into(),
            description: description.into(),
            affordances,
            requirements: Vec::new(),
            providers: Vec::new(),
            loading_mode,
            risk_class,
            auth_requirements: Vec::new(),
            verification,
            deprecated: None,
        }
    }

    pub fn with_requirements(mut self, requirements: Vec<Requirement>) -> Self {
        self.requirements = requirements;
        self
    }

    pub fn with_auth(mut self, auth: Vec<AuthRequirement>) -> Self {
        self.auth_requirements = auth;
        self
    }

    pub fn with_deprecation(mut self, deprecation: Deprecation) -> Self {
        self.deprecated = Some(deprecation);
        self
    }

    /// Requirements of one kind, in declaration order.
    pub fn requirements_of(&self, kind: RequirementKind) -> Vec<&Requirement> {
        self.requirements
            .iter()
            .filter(|r| r.kind == kind)
            .collect()
    }

    /// Is this capability still served at `now_ms`? A descriptor outside its
    /// deprecation window does not resolve (REQ-CAP-009).
    pub fn serves_at(&self, now_ms: u64) -> bool {
        self.deprecated
            .as_ref()
            .map(|d| d.serves_at(now_ms))
            .unwrap_or(true)
    }

    /// The context cost of exposing this descriptor, in bytes, as the
    /// activation planner accounts it (id + description + affordances). A
    /// budgeted subset is a byte budget, not an entry count alone.
    pub fn context_bytes(&self) -> usize {
        self.id.len()
            + self.description.len()
            + self.affordances.iter().map(|a| a.len() + 1).sum::<usize>()
    }

    /// Is the declared version a well-formed content version?
    pub fn version_is_well_formed(&self) -> bool {
        is_well_formed_version(&self.version)
    }

    /// The versions this descriptor may be resolved against without a decision.
    ///
    /// The same major is additive (`minor` bumps only). A *different* major is
    /// served only while the capability is inside a deprecation window — the run
    /// that pinned the old descriptor keeps working until it completes
    /// (`EDGE-012`), and nothing else does.
    pub fn serves_version(&self, pinned: &str) -> bool {
        if pinned == self.version {
            return true;
        }
        match (major_of(&self.version), major_of(pinned)) {
            (Some(cur), Some(want)) if cur == want => true,
            _ => self.deprecated.is_some(),
        }
    }
}

/// Split a `major.minor.patch` version into its major number.
pub fn major_of(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

/// Is this a well-formed content version (`major.minor.patch`, no pre-release
/// suffix)? Descriptors carry one; a free-form tag is a census finding.
pub fn is_well_formed_version(version: &str) -> bool {
    let mut parts = version.split('.');
    let parsed: Vec<&str> = parts.by_ref().take(3).collect();
    if parts.next().is_some() || parsed.len() != 3 {
        return false;
    }
    parsed
        .iter()
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

// ---------------------------------------------------------------------------
// Id rules — dotted, protocol-neutral (DEC-047)
// ---------------------------------------------------------------------------

/// The **closed** protocol/transport vocabulary that may not appear in a
/// capability id segment (DEC-047; `ARCH/13` §1 rule 1). Protocol awareness
/// lives only inside adapters (INV-15), so an id that names one is a review
/// failure rather than a naming preference.
pub const PROTOCOL_ID_TOKENS: &[&str] = &[
    "acp", "api", "cli", "grpc", "http", "https", "jsonrpc", "mcp", "rest", "rpc", "sdk", "socket",
    "sse", "stdio", "ws", "wss",
];

/// Is a capability id well formed? Two dots-separated rules:
/// lowercase `[a-z0-9_]` segments, two or more of them (dotted naming), and no
/// protocol token in any segment.
pub fn is_well_formed_id(id: &str) -> bool {
    if !id_is_dotted(id) {
        return false;
    }
    protocol_token_in(id).is_none()
}

/// Does the id carry dotted (`domain.verb`) naming? At least two segments.
pub fn id_is_dotted(id: &str) -> bool {
    let segments: Vec<&str> = id.split('.').collect();
    segments.len() >= 2
        && segments.iter().all(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
}

/// The protocol token this id leaks, if any. `None` means protocol-neutral.
pub fn protocol_token_in(id: &str) -> Option<&'static str> {
    id.split('.')
        .filter_map(|seg| {
            seg.split('_')
                .find_map(|word| PROTOCOL_ID_TOKENS.iter().copied().find(|t| *t == word))
        })
        .next()
}

// ---------------------------------------------------------------------------
// The result model (`ARCH/13` §3)
// ---------------------------------------------------------------------------

/// The status of a capability invocation. `Guidance` and `RequiresUserAction`
/// are **results**: a capability may answer "connect the account first" with a
/// next action instead of failing the caller's plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Completed,
    Guidance,
    RequiresUserAction,
    Failed,
}

/// The next action a guidance / user-action result carries (`OQ-CAP-4`
/// taxonomy: connect · install · configure · grant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextActionKind {
    /// Attach a connection (the account/app is missing).
    Connect,
    /// Install what is missing.
    Install,
    /// Configure something that exists but is unset.
    Configure,
    /// Grant a permission/credential the capability needs.
    Grant,
}

/// A concrete, workable next step. Never a dead end: a result that is guidance
/// always names what to do and what it would unlock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextAction {
    pub kind: NextActionKind,
    /// The thing acted on (connection id, package id, setting, permission).
    pub target: String,
    /// Human-readable instruction shown in the UI and to the model.
    pub instruction: String,
    /// What becomes possible once it is done.
    pub unlocks: String,
}

impl NextAction {
    pub fn new(
        kind: NextActionKind,
        target: impl Into<String>,
        instruction: impl Into<String>,
        unlocks: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            target: target.into(),
            instruction: instruction.into(),
            unlocks: unlocks.into(),
        }
    }
}

/// Typed capability errors (`ARCH/10` §3). Every failure carries a type and a
/// retryability; there is no untyped "something went wrong".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum CapabilityError {
    /// No such capability id in the registry.
    NotFound { capability_id: String },
    /// The handle is stale, the environment is gone, or the runtime moved
    /// underneath the caller. Re-resolution is required — never a blind retry.
    InvalidState { reason: String },
    /// A ticket was absent, expired, or did not bind this action (INV-03).
    AuthorizationDenied { reason: String },
    /// Credentials absent/expired at the adapter.
    Authentication { reason: String },
    /// Throttled upstream; `retry_after_ms` is the provider's hint when it sent
    /// one.
    RateLimit { retry_after_ms: Option<u64> },
    /// Every provider for this capability is unusable right now.
    Unavailable { reason: String },
    /// The provider's result violated the descriptor/contract schema; nothing
    /// partial was applied (EDGE-018).
    SchemaViolation { reason: String },
    /// A bounded timeout; the watchdog aborted with a typed reason.
    Timeout { reason: String },
    /// Verification did not satisfy the declared depth — the effect is marked
    /// unverified rather than reported complete.
    VerificationFailed { reason: String },
}

impl CapabilityError {
    /// Derived retryability (`ARCH/18` §4 rule: typed first).
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            CapabilityError::RateLimit { .. } | CapabilityError::Unavailable { .. }
        )
    }

    /// The stable wire code for this error type. Callers switch on the code, not
    /// on prose.
    pub const fn code(&self) -> &'static str {
        match self {
            CapabilityError::NotFound { .. } => "not_found",
            CapabilityError::InvalidState { .. } => "invalid_state",
            CapabilityError::AuthorizationDenied { .. } => "authorization_denied",
            CapabilityError::Authentication { .. } => "authentication",
            CapabilityError::RateLimit { .. } => "rate_limit",
            CapabilityError::Unavailable { .. } => "unavailable",
            CapabilityError::SchemaViolation { .. } => "schema_violation",
            CapabilityError::Timeout { .. } => "timeout",
            CapabilityError::VerificationFailed { .. } => "verification_failed",
        }
    }
}

impl std::fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapabilityError::NotFound { capability_id } => {
                write!(f, "no such capability: {capability_id}")
            }
            CapabilityError::InvalidState { reason } => write!(f, "invalid state: {reason}"),
            CapabilityError::AuthorizationDenied { reason } => {
                write!(f, "authorization denied: {reason}")
            }
            CapabilityError::Authentication { reason } => write!(f, "authentication: {reason}"),
            CapabilityError::RateLimit { retry_after_ms } => match retry_after_ms {
                Some(ms) => write!(f, "rate limited (retry after {ms}ms)"),
                None => write!(f, "rate limited"),
            },
            CapabilityError::Unavailable { reason } => write!(f, "unavailable: {reason}"),
            CapabilityError::SchemaViolation { reason } => write!(f, "schema violation: {reason}"),
            CapabilityError::Timeout { reason } => write!(f, "timeout: {reason}"),
            CapabilityError::VerificationFailed { reason } => {
                write!(f, "verification failed: {reason}")
            }
        }
    }
}

impl std::error::Error for CapabilityError {}

/// The result of an invocation. Guidance and user-action are first-class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityResult {
    pub status: CapabilityStatus,
    pub output: Option<serde_json::Value>,
    /// Receipt reference. Mandatory for `completed` results that carried an
    /// externally visible effect (INV-07).
    pub receipt: Option<String>,
    pub next_action: Option<NextAction>,
    /// The typed error, present exactly when `status == failed`.
    pub error: Option<CapabilityError>,
    pub retryable: bool,
}

impl CapabilityResult {
    /// A completed result for an effect that produced a receipt. The receipt
    /// is required by the constructor, so "completed without evidence" is not
    /// representable (INV-07).
    pub fn completed(output: Option<serde_json::Value>, receipt: impl Into<String>) -> Self {
        Self {
            status: CapabilityStatus::Completed,
            output,
            receipt: Some(receipt.into()),
            next_action: None,
            error: None,
            retryable: false,
        }
    }

    /// A completed result for a read: no externally visible effect, so no
    /// receipt is owed. Used deliberately, never as a shortcut around
    /// [`CapabilityResult::completed`].
    pub fn read_completed(output: Option<serde_json::Value>) -> Self {
        Self {
            status: CapabilityStatus::Completed,
            output,
            receipt: None,
            next_action: None,
            error: None,
            retryable: false,
        }
    }

    /// "You need this first" — a result, with a next action.
    pub fn guidance(next_action: NextAction) -> Self {
        Self {
            status: CapabilityStatus::Guidance,
            output: None,
            receipt: None,
            next_action: Some(next_action),
            error: None,
            retryable: false,
        }
    }

    /// "A human has to do this" — a result, with a next action.
    pub fn requires_user_action(next_action: NextAction) -> Self {
        Self {
            status: CapabilityStatus::RequiresUserAction,
            output: None,
            receipt: None,
            next_action: Some(next_action),
            error: None,
            retryable: false,
        }
    }

    /// A typed failure. Retryability is derived from the error type, never set
    /// by hand at the call site.
    pub fn failed(error: CapabilityError) -> Self {
        let retryable = error.retryable();
        Self {
            status: CapabilityStatus::Failed,
            output: None,
            receipt: None,
            next_action: None,
            error: Some(error),
            retryable,
        }
    }

    pub fn is_completed(&self) -> bool {
        self.status == CapabilityStatus::Completed
    }

    /// Is this a non-failure, non-effect result the caller can act on?
    pub fn is_actionable(&self) -> bool {
        matches!(
            self.status,
            CapabilityStatus::Guidance | CapabilityStatus::RequiresUserAction
        )
    }
}

// ---------------------------------------------------------------------------
// Handles (`ARCH/13` §4 · DM-012)
// ---------------------------------------------------------------------------

/// Why a handle may not be used. Every variant demands re-resolution: there is
/// no retry path against a runtime that moved (`ARCH/13` §9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum HandleInvalid {
    /// The handle's lifetime is over.
    Expired { at_ms: u64 },
    /// The provider restarted: `expected` (minted) vs `live` (now).
    EpochMismatch { expected: u64, live: u64 },
    /// The pinned descriptor version is no longer served.
    DescriptorGone { pinned_version: String },
}

/// The reusable, epoch-checked binding a resolve hands back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityHandle {
    pub capability_id: String,
    pub provider_id: String,
    /// The provider epoch at mint time. A bump invalidates this handle.
    pub provider_epoch: u64,
    pub environment_id: String,
    /// Policy snapshot bound at mint time, so a mid-session policy change
    /// re-validates at ticket time rather than riding the old decision.
    pub permission_snapshot: String,
    /// Adapter-side handle (opaque; never a credential).
    pub runtime_handle_ref: String,
    /// Absolute expiry in ms; `0` means "expires with the run".
    pub expires_at_ms: u64,
    /// The descriptor version the run pinned (`ARCH/13` §8).
    pub descriptor_version: String,
}

impl CapabilityHandle {
    /// Validate against the live provider epoch and the clock.
    ///
    /// Order matters: an expired handle is reported as expired, and a stale
    /// epoch is reported as an epoch mismatch — never the reverse, and never
    /// "retry anyway".
    pub fn validate(&self, now_ms: u64, live_epoch: u64) -> Result<(), HandleInvalid> {
        if self.expires_at_ms != 0 && now_ms > self.expires_at_ms {
            return Err(HandleInvalid::Expired {
                at_ms: self.expires_at_ms,
            });
        }
        if self.provider_epoch != live_epoch {
            return Err(HandleInvalid::EpochMismatch {
                expected: self.provider_epoch,
                live: live_epoch,
            });
        }
        Ok(())
    }

    /// Expiry for a handle minted at `now_ms` with `ttl_ms`
    /// ([`DEFAULT_HANDLE_TTL_MS`] when `ttl_ms == 0`).
    pub fn expiry_from(now_ms: u64, ttl_ms: u64) -> u64 {
        let ttl = if ttl_ms == 0 {
            DEFAULT_HANDLE_TTL_MS
        } else {
            ttl_ms
        };
        now_ms.saturating_add(ttl)
    }
}

// ---------------------------------------------------------------------------
// Census rules (the parts that are pure)
// ---------------------------------------------------------------------------

/// The set of provider ids that back the registry, for coverage checks.
pub type ProviderCoverage = BTreeSet<String>;

/// One census violation. The gate fails on any of these: a malformed,
/// duplicated, or unbacked entry is a rejection, not a warning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CensusFinding {
    /// The same id registered twice — ids are stable public API and are never
    /// reused (REQ-CAP-009).
    DuplicateId {
        id: String,
    },
    /// The id is not dotted, or carries a protocol/vendor token.
    MalformedId {
        id: String,
        detail: String,
    },
    /// The version is not `major.minor.patch`.
    MalformedVersion {
        id: String,
        version: String,
    },
    /// No description, or empty affordances — an empty affordance list is the
    /// classic malformed descriptor.
    EmptyAffordances {
        id: String,
    },
    EmptyDescription {
        id: String,
    },
    /// No verification hook, or a depth below the risk-class floor (INV-19).
    MissingVerification {
        id: String,
    },
    UnderDeclaredVerification {
        id: String,
        declared: String,
        floor: String,
    },
    /// Nothing implements it: an invocable thing with no provider is an
    /// unbacked capability.
    NoProvider {
        id: String,
    },
    /// A hand-authored provider list that disagrees with the registry-derived
    /// one — `providers` is derived, never copy-edited.
    HandAuthoredProviders {
        id: String,
    },
    /// An auth requirement that names no reference for a method that needs one
    /// (or, worse, tries to carry a value where a handle belongs).
    MalformedAuth {
        id: String,
        detail: String,
    },
    /// A `requires` edge pointing at a capability that is not registered.
    DanglingRequirement {
        id: String,
        missing: String,
    },
}

impl CensusFinding {
    /// The capability id the finding is about (`""` for a whole-registry one).
    pub fn capability_id(&self) -> &str {
        match self {
            CensusFinding::DuplicateId { id }
            | CensusFinding::MalformedId { id, .. }
            | CensusFinding::MalformedVersion { id, .. }
            | CensusFinding::EmptyAffordances { id }
            | CensusFinding::EmptyDescription { id }
            | CensusFinding::MissingVerification { id }
            | CensusFinding::UnderDeclaredVerification { id, .. }
            | CensusFinding::NoProvider { id }
            | CensusFinding::HandAuthoredProviders { id }
            | CensusFinding::MalformedAuth { id, .. }
            | CensusFinding::DanglingRequirement { id, .. } => id,
        }
    }
}

/// The per-descriptor half of the census: everything checkable without the
/// registry. `coverage` supplies the derived provider set.
pub fn census_descriptor(
    descriptor: &CapabilityDescriptor,
    derived_providers: &ProviderCoverage,
) -> Vec<CensusFinding> {
    let mut findings = Vec::new();
    let id = descriptor.id.as_str();

    if !is_well_formed_id(id) {
        let detail = if !id_is_dotted(id) {
            "ids are dotted (`domain.verb`), lowercase [a-z0-9_]".to_string()
        } else {
            format!(
                "id leaks the protocol token `{}`; capability ids stay protocol-neutral (DEC-047)",
                protocol_token_in(id).unwrap_or("?")
            )
        };
        findings.push(CensusFinding::MalformedId {
            id: id.to_string(),
            detail,
        });
    }
    if !descriptor.version_is_well_formed() {
        findings.push(CensusFinding::MalformedVersion {
            id: id.to_string(),
            version: descriptor.version.clone(),
        });
    }
    if descriptor.description.trim().is_empty() {
        findings.push(CensusFinding::EmptyDescription { id: id.to_string() });
    }
    if descriptor.affordances.iter().all(|a| a.trim().is_empty()) {
        findings.push(CensusFinding::EmptyAffordances { id: id.to_string() });
    }
    if descriptor.verification.hook.trim().is_empty() {
        findings.push(CensusFinding::MissingVerification { id: id.to_string() });
    } else if !descriptor
        .verification
        .depth
        .satisfies(descriptor.risk_class.verification_depth())
    {
        findings.push(CensusFinding::UnderDeclaredVerification {
            id: id.to_string(),
            declared: descriptor.verification.depth.as_str().to_string(),
            floor: descriptor
                .risk_class
                .verification_depth()
                .as_str()
                .to_string(),
        });
    }
    if derived_providers.is_empty() {
        findings.push(CensusFinding::NoProvider { id: id.to_string() });
    }
    if !descriptor.providers.is_empty() {
        let declared: BTreeSet<String> = descriptor
            .providers
            .iter()
            .map(|p| p.provider_id.clone())
            .collect();
        if &declared != derived_providers {
            findings.push(CensusFinding::HandAuthoredProviders { id: id.to_string() });
        }
    }
    for auth in &descriptor.auth_requirements {
        let needs_ref = auth.method != AuthMethod::WellKnown;
        if needs_ref && auth.secret_ref.trim().is_empty() {
            findings.push(CensusFinding::MalformedAuth {
                id: id.to_string(),
                detail: format!(
                    "auth method `{}` needs a vault reference, not an empty one",
                    auth.method.as_str()
                ),
            });
        }
        if !needs_ref && !auth.secret_ref.trim().is_empty() {
            findings.push(CensusFinding::MalformedAuth {
                id: id.to_string(),
                detail: format!(
                    "auth method `{}` must not carry a reference (nothing to resolve)",
                    auth.method.as_str()
                ),
            });
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(id: &str) -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            id,
            "1.0.0",
            "Read a UTF-8 file inside the workspace floor",
            vec!["read a file".into()],
            LoadingMode::OnDemand,
            CapabilityRisk::Safe,
            VerificationHook::new("file.reopen", VerificationDepth::Validate),
        )
    }

    #[test]
    fn ids_are_dotted_and_protocol_neutral() {
        assert!(is_well_formed_id("office.spreadsheet.edit"));
        assert!(is_well_formed_id("file_ops.read"));
        assert!(!is_well_formed_id("office_edit"), "not dotted");
        assert!(!is_well_formed_id("Office.Edit"), "not lowercase");
        assert!(!is_well_formed_id("office..edit"), "empty segment");
        assert!(!is_well_formed_id("office.mcp_call"), "protocol token");
        assert!(!is_well_formed_id("browser.https_read"));
        assert!(!is_well_formed_id("mcp.navigate"));
        assert_eq!(protocol_token_in("office.mcp_call"), Some("mcp"));
        assert_eq!(protocol_token_in("office.spreadsheet.edit"), None);
    }

    #[test]
    fn versions_must_be_content_versions() {
        assert!(is_well_formed_version("1.0.0"));
        assert!(is_well_formed_version("2.13.4"));
        assert!(!is_well_formed_version("1.0"));
        assert!(!is_well_formed_version("1.0.0-rc1"));
        assert!(!is_well_formed_version("v1.0.0"));
        assert!(!is_well_formed_version("1.0.0.1"));
        assert_eq!(major_of("3.4.5"), Some(3));
    }

    #[test]
    fn risk_class_drives_the_verification_floor() {
        assert_eq!(
            CapabilityRisk::Safe.verification_depth(),
            VerificationDepth::Validate
        );
        assert_eq!(
            CapabilityRisk::Sensitive.verification_depth(),
            VerificationDepth::ValidateAndReread
        );
        assert_eq!(
            CapabilityRisk::Dangerous.verification_depth(),
            VerificationDepth::ValidateAndReconcile
        );
        // The matrix is a floor, never a ceiling.
        assert!(VerificationDepth::ValidateAndReconcile.satisfies(VerificationDepth::Validate));
        assert!(!VerificationDepth::Validate.satisfies(VerificationDepth::ValidateAndReread));
        // An absent class is the strictest floor, never a convenient one.
        assert_eq!(CapabilityRisk::default(), CapabilityRisk::Dangerous);
    }

    #[test]
    fn census_rejects_malformed_descriptors() {
        let good = descriptor("office.open");
        assert!(census_descriptor(&good, &["runtime.native".into()].into()).is_empty());

        let mut no_affordances = descriptor("office.open");
        no_affordances.affordances.clear();
        assert!(
            census_descriptor(&no_affordances, &["runtime.native".into()].into()).contains(
                &CensusFinding::EmptyAffordances {
                    id: "office.open".into()
                }
            )
        );

        let bad_id = descriptor("office.mcp_open");
        assert!(census_descriptor(&bad_id, &["runtime.native".into()].into())
            .contains(&CensusFinding::MalformedId {
                id: "office.mcp_open".into(),
                detail: protocol_token_in("office.mcp_open")
                    .map(|t| format!("id leaks the protocol token `{t}`; capability ids stay protocol-neutral (DEC-047)"))
                    .unwrap(),
            }));

        let mut under = descriptor("file_ops.delete");
        under.risk_class = CapabilityRisk::Dangerous;
        under.verification.depth = VerificationDepth::Validate;
        assert!(
            census_descriptor(&under, &["runtime.native".into()].into()).contains(
                &CensusFinding::UnderDeclaredVerification {
                    id: "file_ops.delete".into(),
                    declared: "validate".into(),
                    floor: "validate_reconcile".into(),
                }
            )
        );
    }

    #[test]
    fn census_requires_provider_coverage() {
        let d = descriptor("search.query");
        let findings = census_descriptor(&d, &ProviderCoverage::new());
        assert_eq!(
            findings,
            vec![CensusFinding::NoProvider {
                id: "search.query".into()
            }]
        );
    }

    #[test]
    fn census_rejects_hand_authored_provider_lists() {
        let mut d = descriptor("search.query");
        d.providers.push(ProviderAssignment {
            provider_id: "runtime.native".into(),
            provider_epoch: 1,
        });
        // Matches the derived set → fine.
        assert!(
            census_descriptor(&d, &["runtime.native".into()].into()).is_empty(),
            "a list equal to the derived set is not hand-authoring"
        );
        d.providers[0].provider_id = "some.other.provider".into();
        assert_eq!(
            census_descriptor(&d, &["runtime.native".into()].into()),
            vec![CensusFinding::HandAuthoredProviders {
                id: "search.query".into()
            }]
        );
    }

    #[test]
    fn census_rejects_auth_that_misses_or_fakes_a_reference() {
        let mut d = descriptor("connector.email_send");
        d.risk_class = CapabilityRisk::Sensitive;
        d.verification.depth = VerificationDepth::ValidateAndReread;
        d.auth_requirements = vec![AuthRequirement::oauth("")];
        assert!(
            census_descriptor(&d, &["runtime.native".into()].into()).contains(
                &CensusFinding::MalformedAuth {
                    id: "connector.email_send".into(),
                    detail: "auth method `oauth` needs a vault reference, not an empty one".into(),
                }
            )
        );
        d.auth_requirements = vec![AuthRequirement {
            method: AuthMethod::WellKnown,
            secret_ref: "vault://x".into(),
            prompt: String::new(),
        }];
        assert!(
            census_descriptor(&d, &["runtime.native".into()].into()).contains(
                &CensusFinding::MalformedAuth {
                    id: "connector.email_send".into(),
                    detail:
                        "auth method `well-known` must not carry a reference (nothing to resolve)"
                            .into(),
                }
            )
        );
    }

    #[test]
    fn guidance_and_user_action_are_results_not_failures() {
        let g = CapabilityResult::guidance(NextAction::new(
            NextActionKind::Connect,
            "drive",
            "Connect the drive account",
            "read and write documents there",
        ));
        assert_eq!(g.status, CapabilityStatus::Guidance);
        assert!(g.is_actionable());
        assert!(!g.is_completed());
        assert!(g.error.is_none());
        assert_eq!(g.next_action.unwrap().kind, NextActionKind::Connect);

        let u = CapabilityResult::requires_user_action(NextAction::new(
            NextActionKind::Grant,
            "files.write",
            "Approve the write permission",
            "the edit can be applied",
        ));
        assert_eq!(u.status, CapabilityStatus::RequiresUserAction);
        assert!(u.is_actionable());
    }

    #[test]
    fn completed_effects_carry_a_receipt_and_reads_do_not_need_one() {
        let effect = CapabilityResult::completed(Some(serde_json::json!({"bytes": 4})), "rcpt-1");
        assert!(effect.is_completed());
        assert_eq!(effect.receipt.as_deref(), Some("rcpt-1"));
        let read = CapabilityResult::read_completed(Some(serde_json::json!({"bytes": 4})));
        assert!(read.is_completed());
        assert!(read.receipt.is_none());
    }

    #[test]
    fn failure_retryability_is_derived_from_the_error_type() {
        let throttled = CapabilityResult::failed(CapabilityError::RateLimit {
            retry_after_ms: Some(250),
        });
        assert_eq!(throttled.status, CapabilityStatus::Failed);
        assert!(throttled.retryable);
        let denied = CapabilityResult::failed(CapabilityError::AuthorizationDenied {
            reason: "no ticket".into(),
        });
        assert!(!denied.retryable, "a denial is never retryable in place");
        let schema = CapabilityResult::failed(CapabilityError::SchemaViolation {
            reason: "malformed".into(),
        });
        assert!(!schema.retryable);
        // The code is the switch surface, and it renders for a human.
        assert_eq!(
            denied.error.as_ref().unwrap().code(),
            "authorization_denied"
        );
        assert_eq!(
            throttled.error.as_ref().unwrap().to_string(),
            "rate limited (retry after 250ms)"
        );
        assert_eq!(
            schema.error.as_ref().unwrap().to_string(),
            "schema violation: malformed"
        );
    }

    #[test]
    fn handles_are_epoch_checked_and_never_blindly_retried() {
        let handle = CapabilityHandle {
            capability_id: "office.edit".into(),
            provider_id: "runtime.native".into(),
            provider_epoch: 3,
            environment_id: "env.local".into(),
            permission_snapshot: "policy-7".into(),
            runtime_handle_ref: "rt:1".into(),
            expires_at_ms: 1_000,
            descriptor_version: "1.0.0".into(),
        };
        assert!(handle.validate(500, 3).is_ok());
        assert_eq!(
            handle.validate(500, 4),
            Err(HandleInvalid::EpochMismatch {
                expected: 3,
                live: 4
            })
        );
        assert_eq!(
            handle.validate(2_000, 3),
            Err(HandleInvalid::Expired { at_ms: 1_000 }),
            "expiry is reported before the epoch"
        );
    }

    #[test]
    fn handle_expiry_uses_the_documented_default() {
        assert_eq!(
            CapabilityHandle::expiry_from(1_000, 0),
            1_000 + DEFAULT_HANDLE_TTL_MS
        );
        assert_eq!(CapabilityHandle::expiry_from(1_000, 50), 1_050);
    }

    #[test]
    fn deprecation_window_governs_resolvability() {
        let mut d = descriptor("legacy.export");
        d.deprecated = Some(Deprecation {
            since_version: "1.2.0".into(),
            window_ends_ms: 2_000,
        });
        assert!(d.serves_at(1_999));
        assert!(!d.serves_at(2_000));
        // A pinned older major still serves inside its window.
        assert!(d.serves_version("1.0.0"));
        let current = descriptor("office.open");
        assert!(!current.serves_version("0.9.0"), "no window, no old major");
        assert!(current.serves_version("1.4.0"), "same major is additive");
    }

    #[test]
    fn loading_modes_gate_model_exposure() {
        assert!(LoadingMode::Eager.may_expose_to_model(false));
        assert!(!LoadingMode::Catalog.may_expose_to_model(false));
        assert!(!LoadingMode::OnDemand.may_expose_to_model(false));
        assert!(LoadingMode::OnDemand.may_expose_to_model(true));
        assert_eq!(LoadingMode::parse("on-demand"), Some(LoadingMode::OnDemand));
        assert_eq!(LoadingMode::parse("sometimes"), None);
    }

    #[test]
    fn invocation_metadata_stays_secret_free_and_validated() {
        let inv = CapabilityInvocation {
            grant_id: "grant:1".into(),
            run_id: "run-1".into(),
            capability: "file_ops.read".into(),
            operation: "read".into(),
        };
        assert!(inv.validate().is_ok());
        let json = serde_json::to_string(&inv).unwrap();
        assert!(!json.to_lowercase().contains("secret"));
        assert!(!json.to_lowercase().contains("token"));
        let mut bad = inv.clone();
        bad.capability = String::new();
        assert_eq!(bad.validate(), Err("capability scope is required"));
    }
}
