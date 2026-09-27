//! The provider adapter contract (CTR-010 · `ARCH/14-PROVIDERS.md` §2/§3/§5).
//!
//! One contract, seven declared classes, one registry entry shape. Three rules
//! this module exists to make **unrepresentable**:
//!
//! 1. **A registry entry declares exactly one class.** An entry without a class
//!    is not registered; there is no "default" transport (REQ-PROV-003).
//! 2. **The canonical provider id is distinct from its catalog key and from its
//!    runtime transports.** `catalog_ref` is the catalog key, `transport_refs`
//!    are runtime transport ids, and one gateway may carry several of them
//!    (`ARCH/14` §5, A10). A canonical id is never aliased to a transport id.
//! 3. **A registry entry holds no credential material.** `auth` is a *method*
//!    plus a vault reference; the value never exists on this struct, so it
//!    cannot be serialized, logged, or exported (REQ-PROV-007 · INV-02).
//!
//! Protocol knowledge stops here. Nothing above this module may name a wire
//! protocol, a native tool, or a transport (INV-15 · REQ-PROV-001).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::routing_feed::Health;

// ---------------------------------------------------------------------------
// Adapter classes (`ARCH/14` §3)
// ---------------------------------------------------------------------------

/// The declared class of a provider adapter. Exactly one per registry entry.
///
/// `variant_count` exists so a test can assert the enum has not silently grown a
/// catch-all variant — a new class is a decision, not a patch.
pub const ADAPTER_CLASS_COUNT: usize = 7;

/// The transport class a provider adapter wraps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterClass {
    /// In-process domain runtimes. Highest trust; no process boundary.
    Native,
    /// An external server spoken to over a JSON-RPC protocol.
    Mcp,
    /// An external agent as a capability executor.
    Acp,
    /// A REST/GraphQL endpoint.
    Http,
    /// A declaratively wrapped binary (command allowlist + arg schema).
    Cli,
    /// An in-process extension, reviewed under the native trust rules.
    Plugin,
    /// Another instance of this product, or a hosted provider.
    Remote,
}

impl AdapterClass {
    /// Every declared class, in architecture order.
    pub const ALL: [AdapterClass; ADAPTER_CLASS_COUNT] = [
        AdapterClass::Native,
        AdapterClass::Mcp,
        AdapterClass::Acp,
        AdapterClass::Http,
        AdapterClass::Cli,
        AdapterClass::Plugin,
        AdapterClass::Remote,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            AdapterClass::Native => "native",
            AdapterClass::Mcp => "mcp",
            AdapterClass::Acp => "acp",
            AdapterClass::Http => "http",
            AdapterClass::Cli => "cli",
            AdapterClass::Plugin => "plugin",
            AdapterClass::Remote => "remote",
        }
    }

    /// Does this class require a process or network boundary? Native and plugin
    /// adapters run in-process and are governed by the same review as the kernel
    /// after review; every other class is an egress adapter.
    pub const fn is_egress(self) -> bool {
        !matches!(self, AdapterClass::Native | AdapterClass::Plugin)
    }

    /// Must an adapter of this class carry a floor-checked egress declaration
    /// (host + policy) at registration? Yes for every egress class.
    pub const fn requires_egress_declaration(self) -> bool {
        self.is_egress()
    }

    /// Does this class preserve the peer's own native tool surface as-is
    /// (REQ-PROV-003: an `acp` adapter must not flatten native tools)?
    pub const fn preserves_native_tools(self) -> bool {
        matches!(self, AdapterClass::Acp)
    }
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

/// Adapter health, as `health()` reports it (`ARCH/14` §2). There is no
/// "probably fine": an adapter that has not reported is `Unknown`, and the
/// resolver ranks `Unknown` below anything observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealth {
    Ok,
    Degraded,
    Down,
    Unknown,
}

impl ProviderHealth {
    pub const fn as_str(self) -> &'static str {
        match self {
            ProviderHealth::Ok => "ok",
            ProviderHealth::Degraded => "degraded",
            ProviderHealth::Down => "down",
            ProviderHealth::Unknown => "unknown",
        }
    }

    /// The catalog's observation vocabulary folds onto this one. `Healthy` and
    /// `Unknown` both map onto their own states rather than being merged, so a
    /// "never observed" provider never reads as healthy.
    pub const fn from_observed(h: Health) -> Self {
        match h {
            Health::Healthy => ProviderHealth::Ok,
            Health::Degraded => ProviderHealth::Degraded,
            Health::Down => ProviderHealth::Down,
            Health::Unknown => ProviderHealth::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// Auth — a method, never a value (`ARCH/14` §5, G4)
// ---------------------------------------------------------------------------

/// How a provider authenticates. Modeled as a *method*: the credential lives in
/// the vault and is only ever `use`d (`CTR-013`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMethod {
    /// A key held in the vault, referenced by handle.
    Api,
    /// An authorize/callback/refresh/expiry flow whose tokens live in the vault.
    Oauth,
    /// A locally discoverable / OS-mediated identity; nothing stored.
    WellKnown,
}

impl AuthMethod {
    pub const ALL: [AuthMethod; 3] = [AuthMethod::Api, AuthMethod::Oauth, AuthMethod::WellKnown];

    pub const fn as_str(self) -> &'static str {
        match self {
            AuthMethod::Api => "api",
            AuthMethod::Oauth => "oauth",
            AuthMethod::WellKnown => "well-known",
        }
    }

    /// Does this method need a vault-held secret? `well-known` does not — it
    /// resolves against the local OS/runtime, so carrying a reference for it
    /// would be a contradiction.
    pub const fn needs_secret_ref(self) -> bool {
        !matches!(self, AuthMethod::WellKnown)
    }
}

/// The auth surface of a registry entry: a method, a reference, and optional
/// prompt/validation metadata. **No field on this struct can hold a secret.**
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSpec {
    pub method: AuthMethod,
    /// Vault handle / connection id. Never a value (INV-02).
    pub secret_ref: String,
    /// Optional prompt shown when the method is declared but unmet.
    pub prompt: String,
    /// Optional client-side validation hint (e.g. a minimum key shape), never a
    /// sample credential.
    pub validation: String,
}

impl AuthSpec {
    pub fn api(secret_ref: impl Into<String>) -> Self {
        Self {
            method: AuthMethod::Api,
            secret_ref: secret_ref.into(),
            prompt: String::new(),
            validation: String::new(),
        }
    }

    pub fn oauth(secret_ref: impl Into<String>) -> Self {
        Self {
            method: AuthMethod::Oauth,
            secret_ref: secret_ref.into(),
            prompt: String::new(),
            validation: String::new(),
        }
    }

    pub fn well_known() -> Self {
        Self {
            method: AuthMethod::WellKnown,
            secret_ref: String::new(),
            prompt: String::new(),
            validation: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Egress declaration (INV-05 · REQ-PROV-008)
// ---------------------------------------------------------------------------

/// What an egress adapter declares about its network reach: the host it talks
/// to and the netfloor policy class it needs. Registration refuses an egress
/// adapter that declares neither — an adapter with no floor check is an
/// unregistered egress path (REQ-PROV-008 "static checks find no direct egress
/// clients above the adapter layer").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressDeclaration {
    /// The host this adapter is allowed to reach. A provider entry serving
    /// several hosts declares several adapters (or several hosts here).
    pub hosts: Vec<String>,
    /// Whether loopback destinations are permitted (local runtimes / local
    /// proxies). Mirrors the netfloor policy field of the same name, so the
    /// declaration and the floor check cannot drift into two vocabularies.
    pub allow_loopback: bool,
    /// Whether private/local-network destinations are permitted. Off by
    /// default: that is the actual SSRF prize.
    pub allow_private: bool,
}

impl EgressDeclaration {
    pub fn host(host: impl Into<String>) -> Self {
        Self {
            hosts: vec![host.into()],
            ..Default::default()
        }
    }

    /// The declared host list, normalized for comparison (case-insensitive,
    /// trailing dot stripped).
    pub fn normalized_hosts(&self) -> BTreeSet<String> {
        self.hosts
            .iter()
            .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
            .filter(|h| !h.is_empty())
            .collect()
    }

    /// Is the declaration complete enough to be floor-checked? An egress
    /// adapter with no host has nothing to check, which is the defect this
    /// function exists to catch.
    pub fn is_checkable(&self) -> bool {
        !self.normalized_hosts().is_empty()
    }
}

// ---------------------------------------------------------------------------
// The registry entry (DM-013 + A10)
// ---------------------------------------------------------------------------

/// One provider registry entry. This is the whole shape — id · kind · version ·
/// health · capabilities ref · environments · epoch, plus the distinct
/// `catalog_ref` / `transport_ref`s and the typed auth method (`ARCH/14` §5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRegistration {
    /// Canonical provider id. Stable; never an alias for a transport.
    pub id: String,
    /// The catalog key this entry's identity came from. Distinct from `id`.
    pub catalog_ref: String,
    /// Runtime transport ids. One entry may carry several (a gateway hosting
    /// more than one wire protocol).
    pub transport_refs: Vec<String>,
    /// Exactly one declared class.
    pub class: AdapterClass,
    pub version: String,
    pub health: ProviderHealth,
    /// Capability ids this provider implements. Registry-derived per capability,
    /// declared here as the ref the capability registry projects.
    pub capabilities: Vec<String>,
    /// Environments this provider can run in.
    pub environments: Vec<String>,
    /// Adapter instance epoch. A restart bumps it and invalidates handles.
    pub epoch: u64,
    pub auth: AuthSpec,
    /// Required for every egress class; `None` only for in-process classes.
    pub egress: Option<EgressDeclaration>,
}

impl ProviderRegistration {
    /// An in-process provider (no egress declaration, well-known identity).
    pub fn native(id: impl Into<String>, capabilities: Vec<String>) -> Self {
        let id = id.into();
        Self {
            catalog_ref: format!("catalog/{id}"),
            transport_refs: vec![format!("inproc:{id}")],
            class: AdapterClass::Native,
            version: "1.0.0".into(),
            health: ProviderHealth::Unknown,
            capabilities,
            environments: vec!["local".into()],
            epoch: 1,
            auth: AuthSpec::well_known(),
            egress: None,
            id,
        }
    }

    /// An egress provider. The caller supplies the host list; the class decides
    /// whether the declaration is mandatory.
    pub fn egress(
        id: impl Into<String>,
        class: AdapterClass,
        catalog_ref: impl Into<String>,
        transport_refs: Vec<String>,
        hosts: Vec<String>,
    ) -> Self {
        let id = id.into();
        Self {
            catalog_ref: catalog_ref.into(),
            transport_refs,
            class,
            version: "1.0.0".into(),
            health: ProviderHealth::Unknown,
            capabilities: Vec::new(),
            environments: vec!["local".into()],
            epoch: 1,
            auth: AuthSpec::api(format!("vault://provider/{id}")),
            egress: Some(EgressDeclaration {
                hosts,
                allow_loopback: false,
                allow_private: false,
            }),
            id,
        }
    }

    /// Attach another transport to an entry — the multi-protocol gateway case
    /// (one gateway, several wire protocols; `ARCH/14` §5).
    pub fn with_transport(mut self, transport_ref: impl Into<String>) -> Self {
        self.transport_refs.push(transport_ref.into());
        self
    }

    /// Does this entry route the given transport ref?
    pub fn serves_transport(&self, transport_ref: &str) -> bool {
        self.transport_refs.iter().any(|t| t == transport_ref)
    }

    /// The floor policy a netfloor check must be run with for this entry.
    pub fn floor_policy(&self) -> Option<(bool, bool)> {
        self.egress
            .as_ref()
            .map(|e| (e.allow_loopback, e.allow_private))
    }
}

/// What `discover()` returns (CTR-010): the entry plus the protocol revisions
/// the adapter can actually speak. A caller above the adapter never reads the
/// revisions — it reads capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub registration: ProviderRegistration,
    /// Wire revisions this adapter instance negotiated, newest first. Adapter
    /// internal; surfaced for diagnostics only.
    pub negotiated_revisions: Vec<String>,
    /// Protocol-mismatch detail, set when the adapter is permanently
    /// incompatible (`ARCH/14` §7). A provider marked incompatible carries the
    /// reason, never a silent fallback.
    pub incompatible_reason: Option<String>,
}

impl ProviderInfo {
    pub fn new(registration: ProviderRegistration) -> Self {
        Self {
            registration,
            negotiated_revisions: Vec::new(),
            incompatible_reason: None,
        }
    }

    /// Is this adapter permanently unusable?
    pub fn is_incompatible(&self) -> bool {
        self.incompatible_reason.is_some()
    }
}

// ---------------------------------------------------------------------------
// Native-tool ↔ capability-id mapping (DEC-047)
// ---------------------------------------------------------------------------

/// The mapping the **adapter** owns between one of its native tool names and a
/// capability id, built at discovery. Epoch-checked registry data, never a
/// contract change (`ARCH/14` §5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportToolMapping {
    pub provider_id: String,
    pub transport_ref: String,
    /// The native tool name on the wire. Adapter-internal; never exposed above
    /// the provider layer.
    pub native_tool: String,
    /// The capability id this tool implements. `None` means **unmapped**: the
    /// tool is not invocable, and asking for it is a guidance result, never a
    /// silent exposure.
    pub capability_id: Option<String>,
    /// The provider epoch the mapping was built at. A mapping from an older
    /// epoch is stale and must be re-discovered.
    pub epoch: u64,
}

impl TransportToolMapping {
    pub fn mapped(
        provider_id: impl Into<String>,
        transport_ref: impl Into<String>,
        native_tool: impl Into<String>,
        capability_id: impl Into<String>,
        epoch: u64,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            transport_ref: transport_ref.into(),
            native_tool: native_tool.into(),
            capability_id: Some(capability_id.into()),
            epoch,
        }
    }

    /// A discovered tool the adapter could not map to a capability.
    pub fn unmapped(
        provider_id: impl Into<String>,
        transport_ref: impl Into<String>,
        native_tool: impl Into<String>,
        epoch: u64,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            transport_ref: transport_ref.into(),
            native_tool: native_tool.into(),
            capability_id: None,
            epoch,
        }
    }

    pub fn is_mapped(&self) -> bool {
        self.capability_id.is_some()
    }

    /// Is this mapping still current for `live_epoch`?
    pub fn is_current(&self, live_epoch: u64) -> bool {
        self.epoch == live_epoch
    }
}

/// Why an invocation could not be served, in the vocabulary the provider plane
/// hands upward. Every case is a typed value; `Silent` does not exist (the whole
/// point of DEC-047).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvocationRefusal {
    /// The native tool has no capability mapping — guidance, never exposure.
    UnmappedTool {
        native_tool: String,
        provider_id: String,
    },
    /// The mapping was built at an older provider epoch; re-discover.
    StaleMapping {
        native_tool: String,
        mapped_epoch: u64,
        live_epoch: u64,
    },
    /// No such tool on this transport at all.
    UnknownTool { native_tool: String },
}

impl InvocationRefusal {
    /// The instruction a caller gets. Deliberately **not** naming the native
    /// tool in the user-facing part: the mapping owner decides exposure, and a
    /// caller above the provider layer must not learn the wire name.
    pub fn instruction(&self) -> String {
        match self {
            InvocationRefusal::UnmappedTool { provider_id, .. } => format!(
                "`{provider_id}` exposes a tool that is not mapped to a capability; \
                 map it at discovery or request the capability that covers the job"
            ),
            InvocationRefusal::StaleMapping {
                mapped_epoch,
                live_epoch,
                ..
            } => format!(
                "the capability mapping is stale (built at epoch {mapped_epoch}, runtime is at {live_epoch}); \
                 re-discover before invoking"
            ),
            InvocationRefusal::UnknownTool { .. } => {
                "no such capability is exposed by the resolved provider".to_string()
            }
        }
    }

    pub fn retryable(&self) -> bool {
        // A stale mapping is worth retrying *after* re-discovery; an unmapped
        // tool is a governance decision, not a transient condition.
        matches!(self, InvocationRefusal::StaleMapping { .. })
    }
}

// ---------------------------------------------------------------------------
// The adapter contract (CTR-010)
// ---------------------------------------------------------------------------

/// Something an adapter observed and published (health changes, capability
/// diffs, restarts). One log, projected upward (`ARCH/30`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderEvent {
    /// Health transition.
    HealthChanged {
        provider_id: String,
        from: ProviderHealth,
        to: ProviderHealth,
    },
    /// The adapter restarted: the epoch bumped and outstanding handles are
    /// invalid (`ARCH/14` §1 rule 4).
    Restarted { provider_id: String, epoch: u64 },
    /// A capability was added at re-discovery.
    CapabilityAdded {
        provider_id: String,
        capability_id: String,
    },
    /// A capability disappeared at re-discovery; calls to it now fail typed
    /// (schema drift, `ARCH/14` §7).
    CapabilityRemoved {
        provider_id: String,
        capability_id: String,
    },
    /// Permanent protocol mismatch, with the reason.
    Incompatible { provider_id: String, reason: String },
}

/// One invocation, already authorized: the caller walked Guard → Ticket before
/// getting here (INV-03). `handle` is the epoch-checked binding; an adapter
/// that receives an invocation without one is being called off-contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityInvocation {
    /// The capability id being invoked (protocol-neutral, INV-15).
    pub capability_id: String,
    /// The handle ref from the resolve step; empty means "no handle".
    pub handle: String,
    /// The provider epoch the caller believes it holds.
    pub provider_epoch: u64,
    /// The authorization ticket id. Empty means "no ticket".
    pub ticket_id: String,
    /// Structured arguments.
    pub args: serde_json::Value,
}

impl CapabilityInvocation {
    /// Contract conformance: an execution is only legal with a handle **and** a
    /// ticket. Both absent is the bypass the invariant forbids.
    pub fn conformance_error(&self) -> Option<&'static str> {
        if self.handle.trim().is_empty() {
            return Some("execute requires a validated capability handle");
        }
        if self.ticket_id.trim().is_empty() {
            return Some("execute requires an authorization ticket");
        }
        if self.capability_id.trim().is_empty() {
            return Some("execute requires a capability id");
        }
        None
    }
}

/// The adapter contract (`ARCH/14` §2). Lifecycle: register (discover) →
/// connect → serve → shutdown.
///
/// `execute` receives an operation bound to a validated handle and is never
/// called without a ticket. Adapters are the only place protocol knowledge
/// lives (INV-15).
pub trait ProviderAdapter {
    /// The declared class of this adapter.
    fn class(&self) -> AdapterClass;

    /// `discover() → ProviderInfo`
    fn discover(&mut self) -> Result<ProviderInfo, AdapterError>;

    /// `connect() → void`
    fn connect(&mut self) -> Result<(), AdapterError>;

    /// `health() → HealthStatus`
    fn health(&self) -> ProviderHealth;

    /// `capabilities() → CapabilityDescriptor[]` — capability ids only; the
    /// wire names stay behind the adapter.
    fn capabilities(&self) -> Vec<String>;

    /// `execute(op) → CapabilityResult` — guarded by handle + ticket.
    fn execute(&mut self, op: &CapabilityInvocation) -> Result<serde_json::Value, AdapterError>;

    /// `shutdown() → void`
    fn shutdown(&mut self) -> Result<(), AdapterError>;

    /// `events() → AsyncIterable<ProviderEvent>` — the buffered projection of
    /// what the adapter has published so far.
    fn events(&self) -> Vec<ProviderEvent>;
}

/// Typed adapter failures (`ARCH/14` §7). Bounded, never silent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum AdapterError {
    /// The adapter could not be reached / started.
    #[error("connect failed: {0}")]
    Connect(String),
    /// A typed refusal from the mapping/authorization layer.
    #[error("{0:?}")]
    Refused(InvocationRefusal),
    /// No ticket, or the ticket did not bind this action.
    #[error("authorization denied: {0}")]
    AuthorizationDenied(String),
    /// Credentials absent or expired at the adapter.
    #[error("authentication: {0}")]
    Authentication(String),
    /// Throttled; the provider's own hint when it sent one.
    #[error("rate limited{}", match .retry_after_ms { Some(ms) => format!(" (retry after {ms}ms)"), None => String::new() })]
    RateLimited { retry_after_ms: Option<u64> },
    /// Permanent protocol mismatch — the provider is marked incompatible with
    /// a reason, never silently skipped.
    #[error("protocol mismatch: {0}")]
    ProtocolMismatch(String),
    /// The provider's result violated the descriptor/contract schema; nothing
    /// partial is applied and the provider is marked degraded.
    #[error("schema violation: {0}")]
    SchemaViolation(String),
    /// The outbound connection is not permitted by the declared floor. There is
    /// no fallback route after a denial.
    #[error("egress denied for {host}: {reason}")]
    EgressDenied { host: String, reason: String },
    /// A bounded timeout with an explicit watchdog reason.
    #[error("timeout: {0}")]
    Timeout(String),
    /// Anything else, still typed.
    #[error("adapter error: {0}")]
    Other(String),
}

impl AdapterError {
    /// Derived retryability — the adapter never decides "retry" by hand.
    pub fn retryable(&self) -> bool {
        match self {
            AdapterError::RateLimited { .. }
            | AdapterError::Timeout(_)
            | AdapterError::Connect(_) => true,
            // A refusal inherits the refusal's own retryability: a stale mapping
            // is worth re-discovering, an unmapped tool is not.
            AdapterError::Refused(r) => r.retryable(),
            AdapterError::AuthorizationDenied(_)
            | AdapterError::Authentication(_)
            | AdapterError::ProtocolMismatch(_)
            | AdapterError::SchemaViolation(_)
            | AdapterError::EgressDenied { .. }
            | AdapterError::Other(_) => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Registry validation
// ---------------------------------------------------------------------------

/// Why a registration was refused. Each variant is a governance failure that
/// must not reach the serving set.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RegistrationError {
    #[error("provider id is required")]
    MissingId,
    #[error("provider id `{0}` is blank or contains a separator")]
    MalformedId(String),
    #[error("provider `{id}` declares no transport (transport_refs is empty)")]
    NoTransport { id: String },
    #[error("provider `{id}` aliases its transport id `{transport}` as its canonical id")]
    TransportAliasedAsId { id: String, transport: String },
    #[error("provider `{id}` has a duplicate transport ref `{transport}`")]
    DuplicateTransport { id: String, transport: String },
    #[error("provider `{id}` is an egress adapter with no floor-checked host declaration")]
    UndeclaredEgress { id: String },
    #[error("provider `{id}` declares a class that requires egress but has none")]
    MissingEgress { id: String },
    #[error("provider `{id}` declares egress hosts for an in-process class")]
    UnexpectedEgress { id: String },
    #[error("provider `{id}` declares auth method `{method}` without a vault reference")]
    AuthWithoutReference { id: String, method: String },
    #[error(
        "provider `{id}` carries a reference for `{method}`, which resolves against the runtime and stores nothing"
    )]
    UnnecessaryReference { id: String, method: String },
    #[error("provider `{id}` declares {count} environments; at least one is required")]
    NoEnvironment { id: String, count: usize },
    #[error("provider `{id}` epoch {epoch} is older than the registered epoch {registered}")]
    EpochWentBackwards {
        id: String,
        epoch: u64,
        registered: u64,
    },
    #[error("provider `{id}` changed class from `{from}` to `{to}`; a class change is a new entry")]
    ClassChanged {
        id: String,
        from: String,
        to: String,
    },
}

/// Validate a registration against the registry-entry rules. This is the
/// admission gate: a failing entry is not registered.
pub fn validate_registration(
    reg: &ProviderRegistration,
    registered_epoch: Option<u64>,
) -> Result<(), RegistrationError> {
    let id = reg.id.trim();
    if id.is_empty() {
        return Err(RegistrationError::MissingId);
    }
    if id.contains("://") || id.contains('/') || id.contains(' ') {
        return Err(RegistrationError::MalformedId(id.to_string()));
    }
    if reg.transport_refs.is_empty() || reg.transport_refs.iter().any(|t| t.trim().is_empty()) {
        return Err(RegistrationError::NoTransport { id: id.to_string() });
    }
    let mut seen = BTreeSet::new();
    for transport in &reg.transport_refs {
        if !seen.insert(transport) {
            return Err(RegistrationError::DuplicateTransport {
                id: id.to_string(),
                transport: transport.clone(),
            });
        }
        // The canonical id must never be an alias for a transport id: a caller
        // bound to one transport id would then be unable to route the same
        // capability through the other transports of the same entry.
        if transport == id {
            return Err(RegistrationError::TransportAliasedAsId {
                id: id.to_string(),
                transport: transport.clone(),
            });
        }
    }
    if reg.class.requires_egress_declaration() {
        match &reg.egress {
            Some(e) if e.is_checkable() => {}
            _ => return Err(RegistrationError::UndeclaredEgress { id: id.to_string() }),
        }
    } else if reg.egress.is_some() {
        return Err(RegistrationError::UnexpectedEgress { id: id.to_string() });
    }
    if reg.auth.method.needs_secret_ref() && reg.auth.secret_ref.trim().is_empty() {
        return Err(RegistrationError::AuthWithoutReference {
            id: id.to_string(),
            method: reg.auth.method.as_str().to_string(),
        });
    }
    if !reg.auth.method.needs_secret_ref() && !reg.auth.secret_ref.trim().is_empty() {
        return Err(RegistrationError::UnnecessaryReference {
            id: id.to_string(),
            method: reg.auth.method.as_str().to_string(),
        });
    }
    if reg.environments.is_empty() {
        return Err(RegistrationError::NoEnvironment {
            id: id.to_string(),
            count: 0,
        });
    }
    if let Some(registered) = registered_epoch
        && reg.epoch < registered
    {
        return Err(RegistrationError::EpochWentBackwards {
            id: id.to_string(),
            epoch: reg.epoch,
            registered,
        });
    }
    Ok(())
}

/// Field/key names that would indicate credential material in a serialized
/// registry entry. The check is a **scan**, not a type guarantee: a future field
/// added to [`ProviderRegistration`] that carries a value must be caught here.
const CREDENTIAL_KEY_TOKENS: &[&str] = &[
    "api_key",
    "apikey",
    "secret",
    "token",
    "password",
    "passwd",
    "credential",
    "bearer",
    "private_key",
    "client_secret",
    "refresh_token",
    "access_key",
];

/// Value prefixes that indicate a credential even under an innocent key name.
const CREDENTIAL_VALUE_PREFIXES: &[&str] = &[
    "sk-", "sk_", "pk-", "bearer ", "ghp_", "gho_", "xoxb-", "xoxp-", "AKIA", "ASIA",
];

/// Custody findings over a serialized registry entry (REQ-PROV-007: "auth-method
/// metadata carries no secret material").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustodyFinding {
    /// Dotted path of the offending field, e.g. `auth.secretRef`.
    pub path: String,
    pub detail: String,
}

/// Scan a serialized registry entry for credential material. Returns every
/// finding; an empty result is the proof the acceptance line asks for.
pub fn custody_findings(json: &serde_json::Value) -> Vec<CustodyFinding> {
    let mut findings = Vec::new();
    walk(json, &mut String::new(), &mut findings);
    findings
}

fn walk(node: &serde_json::Value, path: &mut String, out: &mut Vec<CustodyFinding>) {
    match node {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let lowered = key.to_ascii_lowercase();
                let before = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(key);
                // `secretRef` / `secret_ref` are references, not secrets; the
                // token match is on the whole key with a ref suffix excluded.
                let is_ref_key = lowered.ends_with("ref") || lowered.ends_with("_ref");
                if !is_ref_key && CREDENTIAL_KEY_TOKENS.iter().any(|t| lowered.contains(t)) {
                    out.push(CustodyFinding {
                        path: path.clone(),
                        detail: format!("field `{key}` looks like credential material"),
                    });
                }
                walk(value, path, out);
                path.truncate(before);
            }
        }
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if CREDENTIAL_VALUE_PREFIXES
                .iter()
                .any(|p| trimmed.starts_with(p) && trimmed.len() > p.len() + 4)
            {
                out.push(CustodyFinding {
                    path: path.clone(),
                    detail: "value looks like a credential".to_string(),
                });
            }
        }
        serde_json::Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                let before = path.len();
                path.push_str(&format!("[{i}]"));
                walk(item, path, out);
                path.truncate(before);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// The provider registry
// ---------------------------------------------------------------------------

/// The one **adapter** registry: entries, their mappings, and the epoch each
/// entry is at. Distinct from [`crate::provider::ProviderRegistry`], which
/// holds *model* provider identity (aliases, base URLs, verified reports) — this
/// one holds the runtime adapter surface. Health lives here, so the resolver
/// reads one store rather than keeping its own (REQ-PROV-006 "no second store
/// for provider health").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterRegistry {
    entries: BTreeMap<String, ProviderRegistration>,
    mappings: BTreeMap<(String, String), TransportToolMapping>,
    incompatible: BTreeMap<String, String>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register or re-register an entry. Re-registering with a higher epoch is a
    /// restart; a lower epoch is refused.
    pub fn register(&mut self, reg: ProviderRegistration) -> Result<(), RegistrationError> {
        // A class is part of the entry's identity: the same id cannot be
        // re-declared as a different kind of adapter under it.
        if let Some(existing) = self.entries.get(&reg.id)
            && existing.class != reg.class
        {
            return Err(RegistrationError::ClassChanged {
                id: reg.id.clone(),
                from: existing.class.as_str().to_string(),
                to: reg.class.as_str().to_string(),
            });
        }
        validate_registration(&reg, self.entries.get(&reg.id).map(|e| e.epoch))?;
        self.entries.insert(reg.id.clone(), reg);
        Ok(())
    }

    pub fn get(&self, provider_id: &str) -> Option<&ProviderRegistration> {
        self.entries.get(provider_id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry, in stable id order.
    pub fn entries(&self) -> Vec<&ProviderRegistration> {
        self.entries.values().collect()
    }

    /// The live epoch of an entry (`0` when unknown).
    pub fn epoch(&self, provider_id: &str) -> u64 {
        self.entries.get(provider_id).map(|e| e.epoch).unwrap_or(0)
    }

    /// Record a restart: the epoch bumps and outstanding handles minted at the
    /// old epoch become stale by definition.
    pub fn restart(&mut self, provider_id: &str) -> Result<u64, RegistrationError> {
        let entry = self
            .entries
            .get_mut(provider_id)
            .ok_or_else(|| RegistrationError::MissingId)?;
        entry.epoch += 1;
        let live = entry.epoch;
        // Mappings are epoch-checked registry data, so a restart invalidates them
        // too: re-discovery rebuilds them.
        self.mappings
            .retain(|(provider, _), _| provider != provider_id);
        Ok(live)
    }

    /// Record health. One store; the UI and the resolver read it.
    pub fn set_health(&mut self, provider_id: &str, health: ProviderHealth) -> bool {
        match self.entries.get_mut(provider_id) {
            Some(entry) if entry.health != health => {
                entry.health = health;
                true
            }
            _ => false,
        }
    }

    /// Mark an entry permanently incompatible, with a reason.
    pub fn mark_incompatible(&mut self, provider_id: &str, reason: impl Into<String>) {
        self.incompatible
            .insert(provider_id.to_string(), reason.into());
    }

    /// Why an entry is incompatible, if it is.
    pub fn incompatible_reason(&self, provider_id: &str) -> Option<&str> {
        self.incompatible.get(provider_id).map(|s| s.as_str())
    }

    /// Install a discovery-built mapping.
    pub fn map_tool(&mut self, mapping: TransportToolMapping) {
        self.mappings.insert(
            (mapping.provider_id.clone(), mapping.native_tool.clone()),
            mapping,
        );
    }

    /// Look up a native tool on a provider and decide whether it is invocable.
    ///
    /// The only path from a wire name to a capability id. An unmapped tool is a
    /// typed refusal, never a silent exposure (DEC-047).
    pub fn resolve_tool(
        &self,
        provider_id: &str,
        native_tool: &str,
    ) -> Result<String, InvocationRefusal> {
        let Some(entry) = self.entries.get(provider_id) else {
            return Err(InvocationRefusal::UnknownTool {
                native_tool: native_tool.to_string(),
            });
        };
        let Some(mapping) = self
            .mappings
            .get(&(provider_id.to_string(), native_tool.to_string()))
        else {
            return Err(InvocationRefusal::UnknownTool {
                native_tool: native_tool.to_string(),
            });
        };
        if !mapping.is_current(entry.epoch) {
            return Err(InvocationRefusal::StaleMapping {
                native_tool: native_tool.to_string(),
                mapped_epoch: mapping.epoch,
                live_epoch: entry.epoch,
            });
        }
        match &mapping.capability_id {
            Some(id) => Ok(id.clone()),
            None => Err(InvocationRefusal::UnmappedTool {
                native_tool: native_tool.to_string(),
                provider_id: provider_id.to_string(),
            }),
        }
    }

    /// The capability ids this provider actually implements *right now* — the
    /// mapped ones. An unmapped tool contributes nothing.
    pub fn implemented_capabilities(&self, provider_id: &str) -> BTreeSet<String> {
        self.mappings
            .iter()
            .filter(|((provider, _), _)| provider == provider_id)
            .filter_map(|(_, m)| m.capability_id.clone())
            .collect()
    }

    /// Tools discovered on a provider that map to no capability — the set the
    /// census surfaces, so an unmapped tool is visible rather than invisible.
    pub fn unmapped_tools(&self, provider_id: &str) -> Vec<String> {
        self.mappings
            .iter()
            .filter(|((provider, _), mapping)| provider == provider_id && !mapping.is_mapped())
            .map(|((_, tool), _)| tool.clone())
            .collect()
    }

    /// Diff two capability sets discovered at two points in time
    /// (schema drift, `ARCH/14` §7): added and removed, both named.
    pub fn diff_capabilities(
        &self,
        provider_id: &str,
        before: &BTreeSet<String>,
        after: &BTreeSet<String>,
    ) -> Vec<ProviderEvent> {
        let mut events = Vec::new();
        for added in after.difference(before) {
            events.push(ProviderEvent::CapabilityAdded {
                provider_id: provider_id.to_string(),
                capability_id: added.clone(),
            });
        }
        for removed in before.difference(after) {
            events.push(ProviderEvent::CapabilityRemoved {
                provider_id: provider_id.to_string(),
                capability_id: removed.clone(),
            });
        }
        events
    }
}

// ---------------------------------------------------------------------------
// Gateway client identity + session affinity (DEC-035 · REQ-PROV-009)
// ---------------------------------------------------------------------------

/// The client-identity and session-affinity policy a gateway-class provider may
/// require. Declared as data so the policy is reviewable and testable, and so a
/// host can prove its injected headers match what the registry declares.
///
/// Identity is **ours**: never an impersonated user agent and never a generic
/// SDK name (`ARCH/18` §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayIdentityPolicy {
    /// The client name that identifies this product on the wire.
    pub client_name: String,
    /// The User-Agent template. `{version}` is substituted at send time.
    pub user_agent_template: String,
    /// The session-affinity header name (one stable value per conversation).
    pub session_header: String,
    /// The secondary session header some deployments read instead.
    pub session_header_alt: String,
    /// The per-request correlation header (fresh value per request).
    pub request_header: String,
    /// The client-kind header.
    pub client_header: String,
    /// The placeholder used when a conversation id is empty — a real value, not
    /// an empty header, because a missing session is a hard error upstream.
    pub anonymous_session: String,
}

/// The client name this product identifies as on the wire. Explicit rather than
/// derived from the crate name: the header is a product identity, and a
/// transport that sent its own crate name would be leaking build topology to
/// every provider it talks to.
pub const CLIENT_NAME: &str = "AgentCowork";

impl Default for GatewayIdentityPolicy {
    fn default() -> Self {
        Self {
            client_name: CLIENT_NAME.to_string(),
            user_agent_template: "{client}/{version}".to_string(),
            session_header: "x-opencode-session".to_string(),
            session_header_alt: "X-Session-Id".to_string(),
            request_header: "x-opencode-request".to_string(),
            client_header: "x-opencode-client".to_string(),
            anonymous_session: "agentcowork-anon".to_string(),
        }
    }
}

impl GatewayIdentityPolicy {
    /// The user agent for a version.
    pub fn user_agent(&self, version: &str) -> String {
        self.user_agent_template
            .replace("{client}", &self.client_name)
            .replace("{version}", version)
    }

    /// The header set one conversation must carry, as (name, value) pairs. The
    /// two session headers carry the **same** conversation id; the request
    /// header is supplied by the caller because it is fresh per request.
    pub fn conversation_headers(
        &self,
        conversation_id: &str,
        request_id: &str,
    ) -> Vec<(String, String)> {
        let session = match conversation_id.trim() {
            "" => self.anonymous_session.clone(),
            other => other.to_string(),
        };
        vec![
            (self.session_header_alt.clone(), session.clone()),
            (self.session_header.clone(), session),
            (self.request_header.clone(), request_id.to_string()),
            (self.client_header.clone(), "cli".to_string()),
        ]
    }

    /// The header names a host must not let a provider or endpoint override: the
    /// client identity and the affinity header are ours to set (REQ-PROV-009
    /// "client identity spoofed → rejected").
    pub fn reserved_headers(&self) -> BTreeSet<String> {
        BTreeSet::from([
            "user-agent".to_string(),
            self.session_header.to_ascii_lowercase(),
            self.session_header_alt.to_ascii_lowercase(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn egress_entry() -> ProviderRegistration {
        ProviderRegistration::egress(
            "gateway",
            AdapterClass::Http,
            "catalog/gateway",
            vec!["wire:a".into(), "wire:b".into()],
            vec!["api.example.test".into()],
        )
    }

    #[test]
    fn adapter_classes_are_exactly_the_seven_declared() {
        assert_eq!(ADAPTER_CLASS_COUNT, 7);
        assert_eq!(AdapterClass::ALL.len(), 7);
        let names: Vec<&str> = AdapterClass::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            names,
            vec!["native", "mcp", "acp", "http", "cli", "plugin", "remote"]
        );
        // An `acp` adapter must not flatten the peer's native tools; the
        // in-process classes need no egress declaration.
        assert!(AdapterClass::Acp.preserves_native_tools());
        assert!(!AdapterClass::Mcp.preserves_native_tools());
        assert!(AdapterClass::Mcp.requires_egress_declaration());
        assert!(!AdapterClass::Native.requires_egress_declaration());
        assert!(!AdapterClass::Plugin.requires_egress_declaration());
    }

    #[test]
    fn registration_shape_keeps_catalog_and_transport_refs_distinct() {
        let reg = egress_entry();
        assert_eq!(reg.id, "gateway");
        assert_eq!(reg.catalog_ref, "catalog/gateway");
        assert_eq!(reg.transport_refs, vec!["wire:a", "wire:b"]);
        assert_ne!(reg.id, reg.catalog_ref);
        assert!(reg.serves_transport("wire:a"));
        assert!(!reg.serves_transport("wire:z"));
        assert_eq!(reg.floor_policy(), Some((false, false)));
        assert!(validate_registration(&reg, None).is_ok());
    }

    #[test]
    fn registration_refuses_an_id_aliased_to_a_transport() {
        let mut reg = egress_entry();
        reg.transport_refs.push("gateway".into());
        assert_eq!(
            validate_registration(&reg, None),
            Err(RegistrationError::TransportAliasedAsId {
                id: "gateway".into(),
                transport: "gateway".into()
            })
        );
    }

    #[test]
    fn registration_refuses_undeclared_or_impossible_egress() {
        let mut reg = egress_entry();
        reg.egress = Some(EgressDeclaration::default());
        assert_eq!(
            validate_registration(&reg, None),
            Err(RegistrationError::UndeclaredEgress {
                id: "gateway".into()
            })
        );
        let mut native = ProviderRegistration::native("kernel", vec!["office.open".into()]);
        native.egress = Some(EgressDeclaration::host("api.example.test"));
        assert_eq!(
            validate_registration(&native, None),
            Err(RegistrationError::UnexpectedEgress {
                id: "kernel".into()
            })
        );
    }

    #[test]
    fn registration_refuses_auth_without_a_reference_and_the_reverse() {
        let mut reg = egress_entry();
        reg.auth.secret_ref = String::new();
        assert!(matches!(
            validate_registration(&reg, None),
            Err(RegistrationError::AuthWithoutReference { .. })
        ));
        let mut keyless = ProviderRegistration::native("kernel", vec![]);
        keyless.auth.secret_ref = "vault://x".into();
        assert!(matches!(
            validate_registration(&keyless, None),
            Err(RegistrationError::UnnecessaryReference { .. })
        ));
    }

    #[test]
    fn epoch_only_moves_forward_and_a_class_cannot_change_under_an_id() {
        let mut reg = AdapterRegistry::new();
        reg.register(egress_entry()).unwrap();
        assert_eq!(reg.epoch("gateway"), 1);
        let mut restarted = egress_entry();
        restarted.epoch = 5;
        reg.register(restarted).unwrap();
        assert_eq!(reg.epoch("gateway"), 5);
        let mut stale = egress_entry();
        stale.epoch = 2;
        assert!(matches!(
            reg.register(stale),
            Err(RegistrationError::EpochWentBackwards { .. })
        ));
        let mut reclassed = egress_entry();
        reclassed.class = AdapterClass::Cli;
        assert!(matches!(
            reg.register(reclassed),
            Err(RegistrationError::ClassChanged { .. })
        ));
    }

    #[test]
    fn a_restart_bumps_the_epoch_and_invalidates_mappings() {
        let mut reg = AdapterRegistry::new();
        reg.register(egress_entry()).unwrap();
        reg.map_tool(TransportToolMapping::mapped(
            "gateway",
            "wire:a",
            "do_thing",
            "office.open",
            1,
        ));
        assert_eq!(
            reg.resolve_tool("gateway", "do_thing").unwrap(),
            "office.open"
        );
        assert_eq!(reg.restart("gateway").unwrap(), 2);
        assert_eq!(reg.epoch("gateway"), 2);
        assert_eq!(
            reg.resolve_tool("gateway", "do_thing"),
            Err(InvocationRefusal::UnknownTool {
                native_tool: "do_thing".into()
            }),
            "a restart invalidates the mapping — re-discover, never re-invoke"
        );
    }

    #[test]
    fn an_unmapped_tool_is_a_typed_refusal_never_a_silent_exposure() {
        let mut reg = AdapterRegistry::new();
        reg.register(egress_entry()).unwrap();
        reg.map_tool(TransportToolMapping::unmapped(
            "gateway", "wire:a", "sneaky", 1,
        ));
        let refusal = reg.resolve_tool("gateway", "sneaky").unwrap_err();
        assert_eq!(
            refusal,
            InvocationRefusal::UnmappedTool {
                native_tool: "sneaky".into(),
                provider_id: "gateway".into()
            }
        );
        assert!(refusal.instruction().contains("not mapped to a capability"));
        assert!(
            !refusal.retryable(),
            "an unmapped tool is a governance decision"
        );
        assert_eq!(reg.unmapped_tools("gateway"), vec!["sneaky".to_string()]);
        assert!(reg.implemented_capabilities("gateway").is_empty());
    }

    #[test]
    fn a_stale_mapping_reports_the_epoch_gap_and_is_retryable() {
        let mut reg = AdapterRegistry::new();
        let mut entry = egress_entry();
        entry.epoch = 4;
        reg.register(entry).unwrap();
        reg.map_tool(TransportToolMapping::mapped(
            "gateway",
            "wire:a",
            "do_thing",
            "office.open",
            2,
        ));
        let refusal = reg.resolve_tool("gateway", "do_thing").unwrap_err();
        assert_eq!(
            refusal,
            InvocationRefusal::StaleMapping {
                native_tool: "do_thing".into(),
                mapped_epoch: 2,
                live_epoch: 4
            }
        );
        assert!(refusal.retryable(), "re-discovery is worth retrying");
        assert!(AdapterError::Refused(refusal.clone()).retryable());
    }

    #[test]
    fn schema_drift_names_added_and_removed_capabilities() {
        let reg = AdapterRegistry::new();
        let before: BTreeSet<String> = BTreeSet::from(["a.one".into(), "a.two".into()]);
        let after: BTreeSet<String> = BTreeSet::from(["a.two".into(), "a.three".into()]);
        let events = reg.diff_capabilities("p", &before, &after);
        assert_eq!(
            events,
            vec![
                ProviderEvent::CapabilityAdded {
                    provider_id: "p".into(),
                    capability_id: "a.three".into()
                },
                ProviderEvent::CapabilityRemoved {
                    provider_id: "p".into(),
                    capability_id: "a.one".into()
                }
            ]
        );
    }

    #[test]
    fn custody_scan_finds_no_credential_material_in_a_serialized_entry() {
        let reg = egress_entry();
        let json = serde_json::to_value(&reg).unwrap();
        assert!(
            custody_findings(&json).is_empty(),
            "{:?}",
            custody_findings(&json)
        );
        // A reference is not a credential — the scan must not flag `secretRef`.
        assert!(json.get("auth").unwrap().get("secretRef").is_some());
        // But a value under any key is caught — and a smuggled value is caught
        // twice over (the key name *and* the value shape), which is deliberate:
        // the two checks are independent and either one is a custody violation.
        let smuggled = serde_json::json!({
            "id": "gateway",
            "auth": { "method": "api", "apiKey": "sk-live-0123456789" }
        });
        let findings = custody_findings(&smuggled);
        let paths: Vec<&str> = findings.iter().map(|f| f.path.as_str()).collect();
        assert!(
            paths.contains(&"auth.apiKey"),
            "the key name is flagged: {paths:?}"
        );
        assert!(
            findings
                .iter()
                .any(|f| f.detail == "value looks like a credential")
        );
    }

    #[test]
    fn invocation_conformance_demands_a_handle_and_a_ticket() {
        let bare = CapabilityInvocation {
            capability_id: "office.open".into(),
            handle: String::new(),
            provider_epoch: 1,
            ticket_id: "t-1".into(),
            args: serde_json::json!({}),
        };
        assert_eq!(
            bare.conformance_error(),
            Some("execute requires a validated capability handle")
        );
        let unticketed = CapabilityInvocation {
            handle: "rt:p:1".into(),
            ticket_id: String::new(),
            ..bare.clone()
        };
        assert_eq!(
            unticketed.conformance_error(),
            Some("execute requires an authorization ticket")
        );
        let good = CapabilityInvocation {
            handle: "rt:p:1".into(),
            ticket_id: "t-1".into(),
            ..bare
        };
        assert_eq!(good.conformance_error(), None);
    }

    #[test]
    fn gateway_identity_is_stable_per_conversation_and_ours() {
        let policy = GatewayIdentityPolicy::default();
        let first = policy.conversation_headers("conv-7", "req-1");
        let second = policy.conversation_headers("conv-7", "req-2");
        let session: Vec<String> = first
            .iter()
            .filter(|(name, _)| *name == policy.session_header)
            .map(|(_, v)| v.clone())
            .collect();
        let again: Vec<String> = second
            .iter()
            .filter(|(name, _)| *name == policy.session_header)
            .map(|(_, v)| v.clone())
            .collect();
        assert_eq!(session, again, "affinity is one value per conversation");
        assert_eq!(session, vec!["conv-7".to_string()]);
        // The request header is fresh per request; the session header is not.
        assert_ne!(first[2].1, second[2].1);
        // A new conversation is a new value.
        let other = policy.conversation_headers("conv-8", "req-3");
        assert_ne!(other[1].1, first[1].1);
        // An empty conversation id still yields a real header value.
        let anon = policy.conversation_headers("", "req-4");
        assert_eq!(anon[1].1, policy.anonymous_session);
        // Identity is ours and reserved from override.
        let ua = policy.user_agent("0.1.0");
        assert_eq!(ua, "AgentCowork/0.1.0");
        assert!(ua.starts_with(&policy.client_name) && ua.contains("0.1.0"));
        assert!(policy.reserved_headers().contains("user-agent"));
        assert!(
            policy
                .reserved_headers()
                .contains(&policy.session_header.to_ascii_lowercase())
        );
    }

    #[test]
    fn adapter_error_retryability_is_derived() {
        assert!(
            AdapterError::RateLimited {
                retry_after_ms: Some(500)
            }
            .retryable()
        );
        assert!(AdapterError::Timeout("idle".into()).retryable());
        assert!(!AdapterError::AuthorizationDenied("no ticket".into()).retryable());
        assert!(!AdapterError::ProtocolMismatch("no common era".into()).retryable());
        assert!(!AdapterError::SchemaViolation("bad shape".into()).retryable());
        assert!(!AdapterError::Authentication("expired".into()).retryable());
        assert!(
            !AdapterError::EgressDenied {
                host: "h".into(),
                reason: "blocked".into()
            }
            .retryable()
        );
    }
}
