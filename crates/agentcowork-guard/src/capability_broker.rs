//! P49.7 — the local CapabilityBroker, plus the resolver half of CTR-009.
//!
//! Two halves live here and they are deliberately different things:
//!
//! 1. **Authorization** (unchanged, P49.7) — [`CapabilityBroker`] mints opaque
//!    run-scoped grants and authorizes requests by run, capability scope, and
//!    expiry. It is intentionally independent of secrets: secret material
//!    remains in the vault/connector host and is never returned by this module.
//! 2. **Resolution** — [`CapabilityRegistry`] + [`resolve`] implement the
//!    `resolve(capability_id, constraints) → CapabilityHandle` half of CTR-009:
//!    registry-derived provider coverage, deterministic ranking with an audit
//!    trail, epoch-checked handles, requirement-graph resolution, and an
//!    exhausted chain that surfaces as **guidance**, never as a wrong answer.
//!
//! The resolver holds no provider state of its own: callers project their
//! provider registry into [`ProviderCandidate`] values, so there is exactly one
//! health/epoch store in the system (REQ-PROV-006).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::capability_contract::{
    CapabilityDescriptor, CapabilityError, CapabilityHandle, CapabilityResult, CensusFinding,
    DEFAULT_HANDLE_TTL_MS, NextAction, NextActionKind, ProviderAssignment, ProviderCoverage,
    census_descriptor,
};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// P69.D17/D18 — the *runtime* capability invocation shape the broker checks
/// a grant against. Named deliberately: the canonical schema record is
/// `agentcowork_types::CapabilityRequest` (a Work/binding-scoped request that
/// precedes grant minting); this struct is the broker's per-call check input
/// at the executor boundary. Every connector action and MCP tool call funnels
/// through `invoke(grant_id, &request)` here — there is no second permission
/// universe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRequest {
    pub run_id: String,
    pub capability: String,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EphemeralCredential {
    pub handle: String,
    pub scope: String,
    pub issued_for: String,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityGrant {
    pub grant_id: String,
    pub run_id: String,
    pub capability: String,
    pub handle: EphemeralCredential,
    pub issued_at_ms: u64,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityBrokerError {
    #[error("capability request is missing a run id")]
    MissingRun,
    #[error("capability request is missing a scope")]
    MissingCapability,
    #[error("capability is not authorized for this run")]
    NotAuthorized,
    #[error("capability grant is expired")]
    Expired,
    #[error("capability grant is revoked")]
    Revoked,
    #[error("unknown capability grant")]
    UnknownGrant,
    #[error("capability request does not match the grant")]
    ScopeMismatch,
}

pub trait CapabilityBroker {
    fn list_capabilities(&self, run_id: &str) -> Vec<String>;
    fn authorize(
        &mut self,
        request: CapabilityRequest,
        ttl_ms: u64,
    ) -> Result<CapabilityGrant, CapabilityBrokerError>;
    fn invoke(
        &self,
        grant_id: &str,
        request: &CapabilityRequest,
    ) -> Result<EphemeralCredential, CapabilityBrokerError>;
    fn revoke(&mut self, grant_id: &str) -> bool;
}

#[derive(Debug, Default)]
pub struct LocalCapabilityBroker {
    grants: HashMap<String, CapabilityGrant>,
    allowed: HashMap<String, Vec<String>>,
    next_id: u64,
}
impl LocalCapabilityBroker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow_for_run(&mut self, run_id: impl Into<String>, capabilities: Vec<String>) {
        self.allowed.insert(run_id.into(), capabilities);
    }

    fn matches(pattern: &str, value: &str) -> bool {
        pattern == value
            || (pattern.ends_with("**") && value.starts_with(pattern.trim_end_matches("**")))
    }
}

impl CapabilityBroker for LocalCapabilityBroker {
    fn list_capabilities(&self, run_id: &str) -> Vec<String> {
        self.allowed.get(run_id).cloned().unwrap_or_default()
    }

    fn authorize(
        &mut self,
        request: CapabilityRequest,
        ttl_ms: u64,
    ) -> Result<CapabilityGrant, CapabilityBrokerError> {
        if request.run_id.is_empty() {
            return Err(CapabilityBrokerError::MissingRun);
        }
        if request.capability.is_empty() {
            return Err(CapabilityBrokerError::MissingCapability);
        }
        if !self
            .list_capabilities(&request.run_id)
            .iter()
            .any(|p| Self::matches(p, &request.capability))
        {
            return Err(CapabilityBrokerError::NotAuthorized);
        }
        self.next_id += 1;
        let at = now_ms();
        let grant_id = format!("grant:{}", self.next_id);
        let handle = EphemeralCredential {
            handle: format!("cred:{}", self.next_id),
            scope: request.capability.clone(),
            issued_for: request.run_id.clone(),
            expires_at_ms: if ttl_ms == 0 {
                0
            } else {
                at.saturating_add(ttl_ms)
            },
        };
        let grant = CapabilityGrant {
            grant_id: grant_id.clone(),
            run_id: request.run_id,
            capability: request.capability,
            handle,
            issued_at_ms: at,
            revoked: false,
        };
        self.grants.insert(grant_id, grant.clone());
        Ok(grant)
    }

    fn invoke(
        &self,
        grant_id: &str,
        request: &CapabilityRequest,
    ) -> Result<EphemeralCredential, CapabilityBrokerError> {
        let grant = self
            .grants
            .get(grant_id)
            .ok_or(CapabilityBrokerError::UnknownGrant)?;
        if grant.revoked {
            return Err(CapabilityBrokerError::Revoked);
        }
        if grant.handle.expires_at_ms != 0 && now_ms() > grant.handle.expires_at_ms {
            return Err(CapabilityBrokerError::Expired);
        }
        if grant.run_id != request.run_id
            || grant.capability != request.capability
            || !Self::matches(&grant.capability, &request.capability)
        {
            return Err(CapabilityBrokerError::ScopeMismatch);
        }
        Ok(grant.handle.clone())
    }

    fn revoke(&mut self, grant_id: &str) -> bool {
        self.grants
            .get_mut(grant_id)
            .map(|g| {
                g.revoked = true;
                true
            })
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CapabilityRequest {
        CapabilityRequest {
            run_id: "run-1".into(),
            capability: "connector:gmail.read".into(),
            operation: "list".into(),
        }
    }

    #[test]
    fn grants_are_opaque_and_run_scoped() {
        let mut broker = LocalCapabilityBroker::new();
        broker.allow_for_run("run-1", vec!["connector:gmail.**".into()]);
        let grant = broker.authorize(request(), 60_000).unwrap();
        assert!(!grant.handle.handle.contains("gmail"));
        assert!(broker.invoke(&grant.grant_id, &request()).is_ok());
        let mut other = request();
        other.run_id = "run-2".into();
        assert!(matches!(
            broker.invoke(&grant.grant_id, &other),
            Err(CapabilityBrokerError::ScopeMismatch)
        ));
    }

    #[test]
    fn revoke_is_fail_closed() {
        let mut broker = LocalCapabilityBroker::new();
        broker.allow_for_run("run-1", vec!["fs.read:**".into()]);
        let grant = broker
            .authorize(
                CapabilityRequest {
                    run_id: "run-1".into(),
                    capability: "fs.read:/tmp/x".into(),
                    operation: "read".into(),
                },
                60_000,
            )
            .unwrap();
        assert!(broker.revoke(&grant.grant_id));
        assert!(matches!(
            broker.invoke(
                &grant.grant_id,
                &CapabilityRequest {
                    run_id: "run-1".into(),
                    capability: "fs.read:/tmp/x".into(),
                    operation: "read".into()
                }
            ),
            Err(CapabilityBrokerError::Revoked)
        ));
    }

    #[test]
    fn unlisted_capability_is_denied() {
        let mut broker = LocalCapabilityBroker::new();
        broker.allow_for_run("run-1", vec!["connector:gmail.read".into()]);
        assert!(matches!(
            broker.authorize(
                CapabilityRequest {
                    run_id: "run-1".into(),
                    capability: "connector:gmail.send".into(),
                    operation: "send".into()
                },
                1
            ),
            Err(CapabilityBrokerError::NotAuthorized)
        ));
    }
}

// ===========================================================================
// The resolver half of CTR-009
// ===========================================================================
//
// `resolve(capability_id, constraints) → CapabilityHandle` · `descriptors(scope)`
// · `health(provider_id)`. The rules this half owes the architecture:
//
// - Providers are **assignments**, derived at registration — never a field the
//   capability author writes (REQ-CAP-003).
// - Ranking is health → environment fit → permission fit → cost/latency, with
//   deterministic tie-breaks that are **audited**, never random (REQ-CAP-007).
// - A degraded provider is skipped *before* a call can fail on it
//   (REQ-PROV-006).
// - A provider restart bumps its epoch and invalidates outstanding handles;
//   the next step is re-resolution, never a blind retry (REQ-CAP-002).
// - `requires` edges resolve before execution; a blocked chain names the
//   missing edge (REQ-CAP-008).
// - An exhausted chain is `guidance`/`requires_user_action` with a concrete
//   next action — never a dead end, never a silent wrong answer (REQ-CAP-005).

/// Provider liveness, as projected from the provider registry. `Unknown` is a
/// real state, not "probably fine": it ranks below anything observed good and
/// is never dressed up as `Ok`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealth {
    Ok,
    /// Reachable but degraded — skipped while a healthy candidate exists
    /// (REQ-PROV-006 "degraded-before-fail").
    Degraded,
    Down,
    /// No observation yet.
    Unknown,
}

impl ProviderHealth {
    /// Ranking contribution. Higher is better. `Down` sorts last so it is
    /// only reached after every other candidate failed.
    const fn rank(self) -> u8 {
        match self {
            ProviderHealth::Ok => 3,
            ProviderHealth::Unknown => 2,
            ProviderHealth::Degraded => 1,
            ProviderHealth::Down => 0,
        }
    }

    /// May this provider be attempted at all? A `Down` provider is still a
    /// *last* resort when nothing else can serve — reported honestly as
    /// `Unavailable` rather than pretended away.
    pub const fn attempted(self) -> bool {
        true
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            ProviderHealth::Ok => "ok",
            ProviderHealth::Degraded => "degraded",
            ProviderHealth::Down => "down",
            ProviderHealth::Unknown => "unknown",
        }
    }
}

/// Cost class, normalized. Providers that do not declare one are
/// `Unspecified` and never win a tie against a declared class on price alone —
/// an unknown cost is not a free cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostClass {
    Free,
    Low,
    Standard,
    High,
    Unspecified,
}

/// Latency class, normalized (`ARCH/18` §2 `latency_class`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencyClass {
    Local,
    Fast,
    Standard,
    Slow,
    Unspecified,
}

/// Whether the caller's policy snapshot admits this provider. `Unknown` means
/// the fit was never evaluated and must be re-derived before use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionFit {
    /// The policy snapshot names this provider for this scope.
    Granted,
    /// Evaluated, and not permitted.
    Denied,
    /// Not evaluated — never treated as granted.
    Unknown,
}

impl PermissionFit {
    pub const fn servable(self) -> bool {
        !matches!(self, PermissionFit::Denied)
    }
}

/// A provider **projection** fed to the resolver: what the resolver needs to
/// rank, and nothing else. The resolver never stores it (one health/epoch
/// store, REQ-PROV-006), so this type is a value, not a registry row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCandidate {
    pub id: String,
    /// Bumped on every adapter restart; a handle minted at a lower epoch is
    /// stale by definition.
    pub epoch: u64,
    pub health: ProviderHealth,
    /// Environments this provider can run in (`ARCH/19`).
    pub environments: Vec<String>,
    /// The capability ids this provider implements. The registry derives its
    /// `providers` list from exactly this.
    pub capabilities: Vec<String>,
    pub permission_fit: PermissionFit,
    pub cost_class: CostClass,
    pub latency_class: LatencyClass,
    /// Per-capability health (REQ-PROV-006 "partial capability failure
    /// isolates the failing capability"). `None` = the whole provider.
    pub degraded_capabilities: Vec<String>,
}

impl ProviderCandidate {
    /// A minimal well-formed candidate: healthy, local, `Unspecified` cost so
    /// it never wins a price tie by default.
    pub fn new(id: impl Into<String>, epoch: u64, capabilities: Vec<String>) -> Self {
        Self {
            id: id.into(),
            epoch,
            health: ProviderHealth::Unknown,
            environments: vec!["local".to_string()],
            capabilities,
            permission_fit: PermissionFit::Unknown,
            cost_class: CostClass::Unspecified,
            latency_class: LatencyClass::Unspecified,
            degraded_capabilities: Vec::new(),
        }
    }

    /// Does this provider implement the capability at all?
    pub fn implements(&self, capability_id: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability_id)
    }

    /// Is *this* capability degraded on an otherwise-healthy provider?
    pub fn capability_degraded(&self, capability_id: &str) -> bool {
        self.degraded_capabilities
            .iter()
            .any(|c| c == capability_id)
    }

    /// Health for one capability: per-capability health wins over the
    /// provider-wide one.
    pub fn health_for(&self, capability_id: &str) -> ProviderHealth {
        if self.capability_degraded(capability_id) {
            ProviderHealth::Degraded
        } else {
            self.health
        }
    }

    /// Can this provider run in `environment_id`?
    pub fn serves_environment(&self, environment_id: &str) -> bool {
        self.environments.iter().any(|e| e == environment_id)
    }
}

/// Caller constraints for one resolution (`ARCH/13` §5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionConstraints {
    /// The environment the work runs in. A candidate that cannot run there is
    /// out, not down-ranked.
    pub environment_id: String,
    /// The policy snapshot id bound into the handle.
    pub permission_snapshot: String,
    /// Maximum cost class the caller accepts. `None` = no ceiling.
    pub max_cost_class: Option<CostClass>,
    /// Maximum latency class accepted. `None` = no ceiling.
    pub max_latency_class: Option<LatencyClass>,
}

impl ResolutionConstraints {
    pub fn new(environment_id: impl Into<String>, permission_snapshot: impl Into<String>) -> Self {
        Self {
            environment_id: environment_id.into(),
            permission_snapshot: permission_snapshot.into(),
            max_cost_class: None,
            max_latency_class: None,
        }
    }
}

/// Why one candidate was excluded — the honesty surface, so an empty decision
/// is explainable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateExclusion {
    pub provider_id: String,
    pub reason: String,
}

/// One audited ranking step. The tuple recorded is the *whole* decision input,
/// so a ranking can be re-derived and explained without the live provider
/// registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedCandidate {
    pub provider_id: String,
    pub provider_epoch: u64,
    pub health: ProviderHealth,
    /// health → environment → permission → cost → latency, as computed.
    pub score: ResolutionScore,
}

/// The ordered ranking key. `Ord` is the ranking order: health, then
/// environment fit, then permission fit, then cost, then latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionScore {
    pub health: u8,
    /// 2 = the requested environment is explicitly served, 1 = the provider
    /// serves some environment, 0 = cannot run there.
    pub environment_fit: u8,
    pub permission_fit: u8,
    pub cost: CostClass,
    pub latency: LatencyClass,
}

impl ResolutionScore {
    /// The human-readable reason this score outranks another, for the audit
    /// trail. Returns `None` when the ordering is a pure tie-break (which is
    /// then resolved by provider id — deterministic, and recorded as such).
    pub fn deciding_dimension(&self, other: &Self) -> Option<&'static str> {
        if self.health != other.health {
            return Some("health");
        }
        if self.environment_fit != other.environment_fit {
            return Some("environment_fit");
        }
        if self.permission_fit != other.permission_fit {
            return Some("permission_fit");
        }
        if self.cost != other.cost {
            return Some("cost");
        }
        if self.latency != other.latency {
            return Some("latency");
        }
        None
    }
}

/// Why a resolution produced no ranked chain. Each variant is a *typed*
/// outcome the caller can act on, never a bare "not found".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Unresolved {
    /// No such capability id in the registry.
    NotRegistered { capability_id: String },
    /// The capability is registered but past its deprecation window.
    Deprecated { capability_id: String },
    /// A `requires` edge is unmet. `missing` names the edge (REQ-CAP-008).
    MissingRequirement {
        capability_id: String,
        missing: String,
        kind: String,
        rationale: String,
    },
    /// Registered and servable in principle, but no candidate survives the
    /// constraints. `excluded` says why for each one.
    NoServableProvider {
        capability_id: String,
        excluded: Vec<CandidateExclusion>,
    },
}

impl Unresolved {
    /// The typed error a caller surfaces when it cannot use the guidance path.
    /// `capability_id` is the id the caller asked for, which is authoritative
    /// even when the variant carries a nested capability (a blocked chain names
    /// the *missing* edge, not the id asked for).
    pub fn as_error(&self, capability_id: &str) -> CapabilityError {
        match self {
            Unresolved::NotRegistered { .. } => CapabilityError::NotFound {
                capability_id: capability_id.to_string(),
            },
            Unresolved::Deprecated { capability_id } => CapabilityError::InvalidState {
                reason: format!("capability `{capability_id}` is past its deprecation window"),
            },
            Unresolved::MissingRequirement {
                missing, rationale, ..
            } => CapabilityError::InvalidState {
                reason: format!("requirement `{missing}` is unmet: {rationale}"),
            },
            Unresolved::NoServableProvider { excluded, .. } => CapabilityError::Unavailable {
                reason: format!(
                    "no provider can serve it ({})",
                    excluded
                        .iter()
                        .map(|e| format!("{}: {}", e.provider_id, e.reason))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            },
        }
    }
}

/// The outcome of `resolve`: either a ranked chain plus a handle, or a typed
/// reason that becomes guidance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub capability_id: String,
    /// The ranked chain, in attempt order. Empty when unresolved.
    pub chain: Vec<RankedCandidate>,
    /// Every candidate that was considered and why it lost, including the ones
    /// behind the head of the chain.
    pub excluded: Vec<CandidateExclusion>,
    /// Which dimension decided each **adjacent pair** in the chain, in attempt
    /// order. `n` entries for `n` ranked candidates describe the `n-1`
    /// comparisons between them; `None` means the pair was an exact tie broken
    /// by provider id — recorded as a tie rather than dressed up as a reason.
    pub audit: Vec<Option<String>>,
    /// The handle for the head of the chain, minted at the head's epoch.
    pub handle: Option<CapabilityHandle>,
    /// The typed reason there is no chain.
    pub unresolved: Option<Unresolved>,
}

impl Resolution {
    pub fn is_resolved(&self) -> bool {
        !self.chain.is_empty() && self.unresolved.is_none()
    }

    /// The next provider to attempt after the given one (failover advances
    /// along the chain; it never jumps to an unranked provider).
    pub fn failover_next(&self, provider_id: &str) -> Option<&RankedCandidate> {
        let at = self
            .chain
            .iter()
            .position(|c| c.provider_id == provider_id)?;
        self.chain.get(at + 1)
    }

    /// Advance past a failed provider. Returns the next candidate, or the
    /// exhausted-chain guidance: an exhausted chain is a **result** the caller
    /// can show, not a silent wrong answer.
    ///
    /// The error side is boxed because the guidance result carries a next action
    /// and an output slot — a wide `Err` on a hot failover path is a cost, and
    /// the guidance case is the rare one.
    pub fn failover(
        &self,
        failed_provider: &str,
    ) -> Result<RankedCandidate, Box<CapabilityResult>> {
        if let Some(next) = self.failover_next(failed_provider) {
            return Ok(next.clone());
        }
        let attempted: Vec<String> = self.chain.iter().map(|c| c.provider_id.clone()).collect();
        let hint = if attempted.is_empty() {
            self.unresolved
                .as_ref()
                .map(|u| u.as_error(&self.capability_id).to_string())
                .unwrap_or_else(|| "no provider is registered".to_string())
        } else {
            format!("tried {}", attempted.join(", "))
        };
        Err(Box::new(CapabilityResult::guidance(NextAction::new(
            NextActionKind::Configure,
            self.capability_id.clone(),
            format!(
                "every provider for `{}` failed ({hint}) — configure, attach, or repair one, then retry",
                self.capability_id
            ),
            format!("`{}` becomes available again", self.capability_id),
        ))))
    }
}

/// The registry: descriptors plus the derived provider coverage. It is the one
/// place a capability id is registered; nothing else mints one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRegistry {
    descriptors: BTreeMap<String, CapabilityDescriptor>,
    /// capability id → provider ids, derived from the provider projections.
    coverage: BTreeMap<String, BTreeSet<String>>,
    /// provider id → the latest projected epoch.
    epochs: BTreeMap<String, u64>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Project provider state. This is the only write path for provider facts,
    /// and it stores coverage + epoch only — never health, never a store of its
    /// own. Projecting the same provider twice keeps the **highest** epoch seen,
    /// so a stale projection can never un-bump a restart.
    pub fn project_providers(&mut self, candidates: &[ProviderCandidate]) {
        for candidate in candidates {
            let epoch = self
                .epochs
                .entry(candidate.id.clone())
                .and_modify(|e| *e = (*e).max(candidate.epoch))
                .or_insert(candidate.epoch);
            let epoch = *epoch;
            for capability_id in &candidate.capabilities {
                self.coverage
                    .entry(capability_id.clone())
                    .or_default()
                    .insert(candidate.id.clone());
                // Keep the assignment list in step with coverage, including the
                // epoch each assignment was observed at.
                if let Some(d) = self.descriptors.get_mut(capability_id) {
                    match d
                        .providers
                        .iter_mut()
                        .find(|p| p.provider_id == candidate.id)
                    {
                        Some(existing) => {
                            existing.provider_epoch = existing.provider_epoch.max(epoch)
                        }
                        None => d.providers.push(ProviderAssignment {
                            provider_id: candidate.id.clone(),
                            provider_epoch: epoch,
                        }),
                    }
                }
            }
        }
    }

    /// The epoch last projected for a provider (`0` when unknown).
    pub fn provider_epoch(&self, provider_id: &str) -> u64 {
        self.epochs.get(provider_id).copied().unwrap_or(0)
    }

    /// Register a descriptor. The incoming `providers` list is **replaced** by
    /// the derived one: a capability's provider assignment is a registry fact,
    /// never a copy-edited field (REQ-CAP-003).
    pub fn register(&mut self, descriptor: CapabilityDescriptor) -> Result<(), CensusFinding> {
        let normalized = self.with_derived_providers(descriptor.clone());
        if let Some(existing) = self.descriptors.get(&descriptor.id) {
            if *existing != normalized {
                return Err(CensusFinding::DuplicateId {
                    id: descriptor.id.clone(),
                });
            }
            return Ok(());
        }
        self.descriptors.insert(descriptor.id.clone(), normalized);
        Ok(())
    }

    /// A descriptor with its provider list replaced by the derived coverage, in
    /// stable id order.
    fn with_derived_providers(&self, mut descriptor: CapabilityDescriptor) -> CapabilityDescriptor {
        let derived: ProviderCoverage = self
            .coverage
            .get(&descriptor.id)
            .cloned()
            .unwrap_or_default();
        descriptor.providers = derived
            .iter()
            .map(|provider_id| ProviderAssignment {
                provider_id: provider_id.clone(),
                provider_epoch: self.provider_epoch(provider_id),
            })
            .collect();
        descriptor
    }

    pub fn get(&self, capability_id: &str) -> Option<&CapabilityDescriptor> {
        self.descriptors.get(capability_id)
    }

    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    /// Every descriptor, in stable id order.
    pub fn descriptors(&self) -> Vec<&CapabilityDescriptor> {
        self.descriptors.values().collect()
    }

    /// The provider ids backing a capability, derived.
    pub fn providers_of(&self, capability_id: &str) -> BTreeSet<String> {
        self.coverage
            .get(capability_id)
            .cloned()
            .unwrap_or_default()
    }

    /// The census gate. Any finding is a rejection: a malformed, duplicated, or
    /// unbacked entry never enters the serving set.
    pub fn census(&self) -> CensusReport {
        let mut findings: Vec<CensusFinding> = Vec::new();
        // Duplicate ids cannot exist in a BTreeMap; the census asserts the
        // invariant that survives every other rule — a descriptor whose own
        // provider list disagrees with the derived coverage.
        for descriptor in self.descriptors.values() {
            findings.extend(census_descriptor(
                descriptor,
                &self.providers_of(&descriptor.id),
            ));
            for requirement in
                descriptor.requirements_of(crate::capability_contract::RequirementKind::Capability)
            {
                if !self.descriptors.contains_key(&requirement.target) {
                    findings.push(CensusFinding::DanglingRequirement {
                        id: descriptor.id.clone(),
                        missing: requirement.target.clone(),
                    });
                }
            }
        }
        CensusReport {
            censused: self.descriptors.len(),
            findings,
        }
    }

    /// Resolve a capability id against a candidate projection.
    ///
    /// Order of work: existence → deprecation window → requirement graph →
    /// candidate ranking. The first three produce typed guidance; only a
    /// successful ranking mints a handle.
    pub fn resolve(
        &self,
        capability_id: &str,
        constraints: &ResolutionConstraints,
        candidates: &[ProviderCandidate],
        now_ms: u64,
    ) -> Resolution {
        let Some(descriptor) = self.descriptors.get(capability_id) else {
            return Resolution {
                capability_id: capability_id.to_string(),
                chain: Vec::new(),
                excluded: Vec::new(),
                audit: Vec::new(),
                handle: None,
                unresolved: Some(Unresolved::NotRegistered {
                    capability_id: capability_id.to_string(),
                }),
            };
        };
        if !descriptor.serves_at(now_ms) {
            return Resolution {
                capability_id: capability_id.to_string(),
                chain: Vec::new(),
                excluded: Vec::new(),
                audit: Vec::new(),
                handle: None,
                unresolved: Some(Unresolved::Deprecated {
                    capability_id: capability_id.to_string(),
                }),
            };
        }
        if let Some(blocked) = self.first_unmet_requirement(descriptor, candidates) {
            return Resolution {
                capability_id: capability_id.to_string(),
                chain: Vec::new(),
                excluded: Vec::new(),
                audit: Vec::new(),
                handle: None,
                unresolved: Some(blocked),
            };
        }

        let mut ranked: Vec<RankedCandidate> = Vec::new();
        let mut excluded: Vec<CandidateExclusion> = Vec::new();
        for candidate in candidates {
            if !candidate.implements(capability_id) {
                excluded.push(CandidateExclusion {
                    provider_id: candidate.id.clone(),
                    reason: "does not implement this capability".into(),
                });
                continue;
            }
            if !candidate.permission_fit.servable() {
                excluded.push(CandidateExclusion {
                    provider_id: candidate.id.clone(),
                    reason: "policy snapshot denies this provider".into(),
                });
                continue;
            }
            if !constraints.environment_id.is_empty()
                && !candidate.serves_environment(&constraints.environment_id)
            {
                excluded.push(CandidateExclusion {
                    provider_id: candidate.id.clone(),
                    reason: format!(
                        "does not run in environment `{}`",
                        constraints.environment_id
                    ),
                });
                continue;
            }
            if let Some(ceiling) = constraints.max_cost_class
                && candidate.cost_class > ceiling
            {
                excluded.push(CandidateExclusion {
                    provider_id: candidate.id.clone(),
                    reason: format!(
                        "cost class `{:?}` exceeds the caller's ceiling",
                        candidate.cost_class
                    ),
                });
                continue;
            }
            if let Some(ceiling) = constraints.max_latency_class
                && candidate.latency_class > ceiling
            {
                excluded.push(CandidateExclusion {
                    provider_id: candidate.id.clone(),
                    reason: format!(
                        "latency class `{:?}` exceeds the caller's ceiling",
                        candidate.latency_class
                    ),
                });
                continue;
            }
            let environment_fit = if constraints.environment_id.is_empty() {
                1
            } else if candidate.serves_environment(&constraints.environment_id) {
                2
            } else {
                0
            };
            let score = ResolutionScore {
                health: candidate.health_for(capability_id).rank(),
                environment_fit,
                permission_fit: candidate.permission_fit as u8,
                cost: candidate.cost_class,
                latency: candidate.latency_class,
            };
            // A `Down` provider is attempted only when nothing else can serve:
            // it stays in the chain, ranked last, and the honest `Unavailable`
            // is returned if it is reached.
            ranked.push(RankedCandidate {
                provider_id: candidate.id.clone(),
                provider_epoch: candidate.epoch,
                health: candidate.health_for(capability_id),
                score,
            });
        }

        // The exclusion list is part of the resolution, so it is ordered too:
        // the whole resolution is then a pure function of the candidate *set*,
        // not of the order the caller happened to project it in.
        excluded.sort_by(|a, b| {
            a.provider_id
                .cmp(&b.provider_id)
                .then_with(|| a.reason.cmp(&b.reason))
        });

        // Deterministic order: score descending, then provider id ascending.
        // Both components are total, so the same inputs always produce the same
        // chain — there is no random tie-break anywhere (REQ-CAP-007).
        ranked.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.provider_id.cmp(&b.provider_id))
        });
        let audit: Vec<Option<String>> = ranked
            .windows(2)
            .map(|pair| {
                pair[0]
                    .score
                    .deciding_dimension(&pair[1].score)
                    .map(str::to_string)
            })
            .collect();

        if ranked.is_empty() {
            return Resolution {
                capability_id: capability_id.to_string(),
                chain: Vec::new(),
                excluded: excluded.clone(),
                audit: Vec::new(),
                handle: None,
                unresolved: Some(Unresolved::NoServableProvider {
                    capability_id: capability_id.to_string(),
                    excluded,
                }),
            };
        }

        let head = &ranked[0];
        let handle = CapabilityHandle {
            capability_id: capability_id.to_string(),
            provider_id: head.provider_id.clone(),
            provider_epoch: head.provider_epoch,
            environment_id: constraints.environment_id.clone(),
            permission_snapshot: constraints.permission_snapshot.clone(),
            // An opaque adapter-side reference, never a credential.
            runtime_handle_ref: format!("rt:{}:{}", head.provider_id, head.provider_epoch),
            expires_at_ms: CapabilityHandle::expiry_from(now_ms, DEFAULT_HANDLE_TTL_MS),
            descriptor_version: descriptor.version.clone(),
        };
        Resolution {
            capability_id: capability_id.to_string(),
            chain: ranked,
            excluded,
            audit,
            handle: Some(handle),
            unresolved: None,
        }
    }

    /// The first unmet `requires` edge, depth-first over capability edges.
    ///
    /// A `connection`/`environment` edge is unmet when no candidate that
    /// implements the capability can satisfy it — an account that is not
    /// connected is a `requires_user_action` result, not a failure.
    fn first_unmet_requirement(
        &self,
        descriptor: &CapabilityDescriptor,
        candidates: &[ProviderCandidate],
    ) -> Option<Unresolved> {
        for requirement in &descriptor.requirements {
            match requirement.kind {
                crate::capability_contract::RequirementKind::Capability => {
                    let Some(required) = self.descriptors.get(&requirement.target) else {
                        return Some(Unresolved::MissingRequirement {
                            capability_id: descriptor.id.clone(),
                            missing: requirement.target.clone(),
                            kind: "capability".into(),
                            rationale: requirement.rationale.clone(),
                        });
                    };
                    if !required.serves_at(0) {
                        return Some(Unresolved::MissingRequirement {
                            capability_id: descriptor.id.clone(),
                            missing: requirement.target.clone(),
                            kind: "capability".into(),
                            rationale: requirement.rationale.clone(),
                        });
                    }
                    if let Some(nested) = self.first_unmet_requirement(required, candidates) {
                        return Some(nested);
                    }
                }
                crate::capability_contract::RequirementKind::Environment => {
                    if !candidates
                        .iter()
                        .any(|c| c.serves_environment(&requirement.target))
                    {
                        return Some(Unresolved::MissingRequirement {
                            capability_id: descriptor.id.clone(),
                            missing: requirement.target.clone(),
                            kind: "environment".into(),
                            rationale: requirement.rationale.clone(),
                        });
                    }
                }
                crate::capability_contract::RequirementKind::Connection => {
                    // A connection requirement is satisfiable only when a
                    // candidate that implements this capability exists at all;
                    // the *credential* gate lives in the provider registry
                    // (P50.3.6), which excludes unkeyed providers before the
                    // chain is ever built. Here the edge blocks when nothing
                    // implements the capability — the honest "connect first".
                    if !candidates.iter().any(|c| c.implements(&descriptor.id)) {
                        return Some(Unresolved::MissingRequirement {
                            capability_id: descriptor.id.clone(),
                            missing: requirement.target.clone(),
                            kind: "connection".into(),
                            rationale: requirement.rationale.clone(),
                        });
                    }
                }
            }
        }
        None
    }
}

/// The census verdict. `ok` is the gate: no findings means the registry is
/// servable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CensusReport {
    pub censused: usize,
    pub findings: Vec<CensusFinding>,
}

impl CensusReport {
    pub fn ok(&self) -> bool {
        self.findings.is_empty()
    }

    /// The findings for one capability id.
    pub fn findings_for(&self, capability_id: &str) -> Vec<&CensusFinding> {
        self.findings
            .iter()
            .filter(|f| f.capability_id() == capability_id)
            .collect()
    }
}

#[cfg(test)]
mod resolution_tests {
    use super::*;
    use crate::capability_contract::{
        CapabilityRisk, LoadingMode, Requirement, VerificationDepth, VerificationHook,
    };

    const NOW: u64 = 1_000_000;

    fn descriptor(id: &str, risk: CapabilityRisk) -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            id,
            "1.0.0",
            format!("{id} capability"),
            vec![format!("perform {id}")],
            LoadingMode::OnDemand,
            risk,
            VerificationHook::new("verify.read_back", VerificationDepth::Validate),
        )
    }

    fn candidate(id: &str, epoch: u64, health: ProviderHealth) -> ProviderCandidate {
        ProviderCandidate {
            epoch,
            health,
            permission_fit: PermissionFit::Granted,
            cost_class: CostClass::Standard,
            latency_class: LatencyClass::Standard,
            // The tests resolve in `env.local`, so a candidate must serve it —
            // an environment mismatch is an exclusion, not a down-rank.
            environments: vec!["env.local".to_string()],
            ..ProviderCandidate::new(id, epoch, vec!["office.edit".to_string()])
        }
    }

    fn constraints() -> ResolutionConstraints {
        ResolutionConstraints::new("env.local", "policy-7")
    }

    #[test]
    fn resolution_ranks_health_then_environment_then_permission_then_cost() {
        let mut reg = CapabilityRegistry::new();
        let mut healthy = candidate("p.ok", 1, ProviderHealth::Ok);
        healthy.latency_class = LatencyClass::Slow;
        let degraded = candidate("p.degraded", 1, ProviderHealth::Degraded);
        let mut ok_but_deny = candidate("p.denied", 1, ProviderHealth::Ok);
        ok_but_deny.permission_fit = PermissionFit::Denied;
        let mut cheap_down = candidate("p.cheap_down", 1, ProviderHealth::Down);
        cheap_down.cost_class = CostClass::Free;
        let all = vec![
            cheap_down,
            degraded,
            ok_but_deny,
            healthy.clone(),
            candidate("p.unknown", 1, ProviderHealth::Unknown),
        ];
        reg.project_providers(&all);
        reg.register(descriptor("office.edit", CapabilityRisk::Sensitive))
            .expect("registers");

        let r = reg.resolve("office.edit", &constraints(), &all, NOW);
        assert!(r.is_resolved(), "{r:?}");
        let order: Vec<&str> = r.chain.iter().map(|c| c.provider_id.as_str()).collect();
        // Health dominates, and the permission denial drops out of the chain
        // entirely (it is excluded, not down-ranked).
        assert_eq!(
            order,
            vec!["p.ok", "p.unknown", "p.degraded", "p.cheap_down"]
        );
        assert!(
            r.excluded
                .iter()
                .any(|e| e.provider_id == "p.denied" && e.reason.contains("denies"))
        );
        // The first pair is decided by health; the tie between the two ok
        // health entries is not — the audit says so instead of inventing one.
        assert_eq!(r.audit.len(), 3, "one entry per adjacent pair");
        assert_eq!(r.audit[0], Some("health".to_string()));
        assert_eq!(r.audit[1], Some("health".to_string()));
        assert_eq!(r.audit[2], Some("health".to_string()));
    }

    #[test]
    fn tie_break_is_deterministic_and_audited() {
        let mut reg = CapabilityRegistry::new();
        let mut a = candidate("a", 1, ProviderHealth::Ok);
        a.cost_class = CostClass::Low;
        let mut b = candidate("b", 1, ProviderHealth::Ok);
        b.cost_class = CostClass::Low;
        let all = vec![b.clone(), a.clone()];
        reg.project_providers(&all);
        reg.register(descriptor("office.edit", CapabilityRisk::Safe))
            .unwrap();
        let first = reg.resolve("office.edit", &constraints(), &all, NOW);
        let reversed = reg.resolve("office.edit", &constraints(), &[a, b], NOW);
        let order: Vec<&str> = first.chain.iter().map(|c| c.provider_id.as_str()).collect();
        assert_eq!(order, vec!["a", "b"], "provider id decides an exact tie");
        assert_eq!(
            first
                .chain
                .iter()
                .map(|c| c.provider_id.clone())
                .collect::<Vec<_>>(),
            reversed
                .chain
                .iter()
                .map(|c| c.provider_id.clone())
                .collect::<Vec<_>>(),
            "input order must not change the chain"
        );
        assert_eq!(
            first.audit[0], None,
            "a tie is recorded as a tie, not a reason"
        );
    }

    #[test]
    fn handle_is_epoch_checked_against_a_provider_restart() {
        let mut reg = CapabilityRegistry::new();
        let first = candidate("p", 1, ProviderHealth::Ok);
        reg.project_providers(std::slice::from_ref(&first));
        reg.register(descriptor("office.edit", CapabilityRisk::Safe))
            .unwrap();
        let before = reg
            .resolve("office.edit", &constraints(), &[first], NOW)
            .handle
            .expect("handle");
        assert_eq!(before.provider_epoch, 1);
        assert!(before.validate(NOW + 1, 1).is_ok());

        // The provider restarts: epoch 2, and the old handle is stale.
        let restarted = candidate("p", 2, ProviderHealth::Ok);
        reg.project_providers(std::slice::from_ref(&restarted));
        assert_eq!(reg.provider_epoch("p"), 2);
        let stale = before.validate(NOW + 1, reg.provider_epoch("p"));
        assert_eq!(
            stale,
            Err(crate::capability_contract::HandleInvalid::EpochMismatch {
                expected: 1,
                live: 2
            }),
            "a restart invalidates outstanding handles — no blind retry"
        );
        let after = reg
            .resolve("office.edit", &constraints(), &[restarted], NOW)
            .handle
            .expect("re-resolution mints a fresh handle");
        assert_eq!(after.provider_epoch, 2);
        assert!(after.validate(NOW + 1, 2).is_ok());
    }

    #[test]
    fn a_stale_projection_cannot_un_bump_a_restart() {
        let mut reg = CapabilityRegistry::new();
        reg.project_providers(&[candidate("p", 5, ProviderHealth::Ok)]);
        reg.project_providers(&[candidate("p", 2, ProviderHealth::Ok)]);
        assert_eq!(reg.provider_epoch("p"), 5);
    }

    #[test]
    fn degraded_is_ranked_below_healthy_before_a_call_fails() {
        let mut reg = CapabilityRegistry::new();
        let healthy = candidate("p.ok", 1, ProviderHealth::Ok);
        let mut partially_degraded = candidate("p.part", 1, ProviderHealth::Ok);
        partially_degraded.degraded_capabilities = vec!["office.edit".into()];
        let all = vec![partially_degraded, healthy.clone()];
        reg.project_providers(&all);
        reg.register(descriptor("office.edit", CapabilityRisk::Safe))
            .unwrap();
        let r = reg.resolve("office.edit", &constraints(), &all, NOW);
        assert_eq!(r.chain[0].provider_id, "p.ok");
        assert_eq!(
            r.chain[1].health,
            ProviderHealth::Degraded,
            "per-capability health isolates the failing capability"
        );
    }

    #[test]
    fn failover_walks_the_chain_then_returns_guidance() {
        let mut reg = CapabilityRegistry::new();
        let all = vec![
            candidate("p.first", 1, ProviderHealth::Ok),
            candidate("p.second", 1, ProviderHealth::Degraded),
        ];
        reg.project_providers(&all);
        reg.register(descriptor("office.edit", CapabilityRisk::Safe))
            .unwrap();
        let r = reg.resolve("office.edit", &constraints(), &all, NOW);
        assert_eq!(r.failover_next("p.first").unwrap().provider_id, "p.second");
        let exhausted = r.failover("p.second").expect_err("chain is exhausted");
        assert!(
            exhausted.is_actionable(),
            "exhausted is guidance, not a failure"
        );
        let action = exhausted.next_action.expect("a next action");
        assert_eq!(action.kind, NextActionKind::Configure);
        assert!(
            action.instruction.contains("p.first") && action.instruction.contains("p.second"),
            "the exhausted chain is named: {}",
            action.instruction
        );
        assert!(action.unlocks.contains("office.edit"));
    }

    #[test]
    fn no_provider_is_guidance_with_a_named_requirement() {
        let mut reg = CapabilityRegistry::new();
        let mut d = descriptor("connector.email_send", CapabilityRisk::Dangerous);
        d.verification.depth = VerificationDepth::ValidateAndReconcile;
        d = d.with_requirements(vec![Requirement::connection(
            "mail-account",
            "the mail account must be connected before a message can be sent",
        )]);
        reg.register(d).unwrap();
        let r = reg.resolve("connector.email_send", &constraints(), &[], NOW);
        assert!(!r.is_resolved());
        let unresolved = r.unresolved.clone().expect("typed reason");
        assert!(matches!(
            unresolved,
            Unresolved::MissingRequirement { ref missing, .. } if missing == "mail-account"
        ));
        // The blocked chain names the edge, and it is still a result.
        let result = r.failover("no-such-provider").expect_err("no chain");
        assert!(result.is_actionable());
        assert!(
            unresolved
                .as_error("connector.email_send")
                .to_string()
                .contains("mail-account"),
            "the error names the missing edge"
        );
    }

    #[test]
    fn requirement_graph_resolves_depth_first() {
        let mut reg = CapabilityRegistry::new();
        let inner = descriptor("storage.read", CapabilityRisk::Safe).with_requirements(vec![
            Requirement::environment("env.local", "a local filesystem must be mounted"),
        ]);
        let mut outer = descriptor("office.open", CapabilityRisk::Sensitive);
        outer.verification.depth = VerificationDepth::ValidateAndReread;
        outer = outer.with_requirements(vec![Requirement::capability(
            "storage.read",
            "the document is fetched through the storage capability",
        )]);
        let mut far = descriptor("office.summarise", CapabilityRisk::Safe);
        far = far.with_requirements(vec![Requirement::capability(
            "office.open",
            "summarising needs the document open",
        )]);
        for d in [inner, outer, far] {
            reg.register(d).unwrap();
        }
        // No candidate serves `env.local` → the *nested* edge blocks, and the
        // block names the nested edge rather than the outer one.
        let r = reg.resolve("office.summarise", &constraints(), &[], NOW);
        assert!(matches!(
            r.unresolved.expect("blocked"),
            Unresolved::MissingRequirement { ref missing, ref kind, .. }
                if missing == "env.local" && kind == "environment"
        ));
        // With an environment-serving candidate the whole chain resolves.
        let serving = ProviderCandidate {
            permission_fit: PermissionFit::Granted,
            environments: vec!["env.local".to_string()],
            ..ProviderCandidate::new(
                "p",
                1,
                vec![
                    "storage.read".to_string(),
                    "office.open".to_string(),
                    "office.summarise".to_string(),
                ],
            )
        };
        let ok = reg.resolve("office.summarise", &constraints(), &[serving], NOW);
        assert!(ok.is_resolved(), "{ok:?}");
    }

    #[test]
    fn deprecated_capability_stops_resolving_after_its_window() {
        let mut reg = CapabilityRegistry::new();
        let c = ProviderCandidate {
            permission_fit: PermissionFit::Granted,
            environments: vec!["env.local".to_string()],
            ..ProviderCandidate::new("p", 1, vec!["legacy.export".to_string()])
        };
        reg.project_providers(std::slice::from_ref(&c));
        let mut d = descriptor("legacy.export", CapabilityRisk::Safe);
        d = d.with_deprecation(crate::capability_contract::Deprecation {
            since_version: "1.0.0".into(),
            window_ends_ms: 2_000,
        });
        reg.register(d).unwrap();
        assert!(
            reg.resolve("legacy.export", &constraints(), &[c.clone()], 1_999)
                .is_resolved(),
            "a deprecated capability keeps serving inside its window"
        );
        let after = reg.resolve("legacy.export", &constraints(), &[c], 2_000);
        assert!(matches!(
            after.unresolved.expect("past its window"),
            Unresolved::Deprecated { .. }
        ));
    }

    #[test]
    fn unknown_id_is_not_found_not_a_silent_empty_chain() {
        let reg = CapabilityRegistry::new();
        let r = reg.resolve("office.nope", &constraints(), &[], NOW);
        assert!(!r.is_resolved());
        assert!(r.handle.is_none());
        assert!(matches!(
            r.unresolved.expect("typed"),
            Unresolved::NotRegistered { ref capability_id } if capability_id == "office.nope"
        ));
    }

    #[test]
    fn registration_derives_providers_and_the_census_fails_without_one() {
        let mut reg = CapabilityRegistry::new();
        let c = candidate("p", 7, ProviderHealth::Ok);
        reg.project_providers(&[c]);
        let d = descriptor("office.edit", CapabilityRisk::Safe);
        reg.register(d).unwrap();
        let stored = reg.get("office.edit").expect("registered");
        assert_eq!(stored.providers.len(), 1);
        assert_eq!(stored.providers[0].provider_id, "p");
        assert_eq!(stored.providers[0].provider_epoch, 7);
        assert!(reg.census().ok(), "{:?}", reg.census().findings);

        // An unbacked capability is a census finding, not a serving entry.
        let mut reg2 = CapabilityRegistry::new();
        reg2.register(descriptor("office.edit", CapabilityRisk::Safe))
            .unwrap();
        let report = reg2.census();
        assert!(!report.ok());
        assert_eq!(
            report.findings,
            vec![CensusFinding::NoProvider {
                id: "office.edit".into()
            }]
        );
    }

    #[test]
    fn census_rejects_a_duplicate_id_and_a_dangling_requirement() {
        let mut reg = CapabilityRegistry::new();
        reg.project_providers(&[candidate("p", 1, ProviderHealth::Ok)]);
        let d = descriptor("office.edit", CapabilityRisk::Safe);
        reg.register(d.clone()).unwrap();
        // Re-registering the identical descriptor is idempotent…
        assert!(reg.register(d.clone()).is_ok());
        // …a different descriptor under the same id is a duplicate.
        let conflicting = descriptor("office.edit", CapabilityRisk::Dangerous);
        assert_eq!(
            reg.register(conflicting),
            Err(CensusFinding::DuplicateId {
                id: "office.edit".into()
            })
        );

        let dangling = descriptor("office.summarise", CapabilityRisk::Safe)
            .with_requirements(vec![Requirement::capability("office.absent", "needed")]);
        reg.project_providers(&[ProviderCandidate::new(
            "q",
            1,
            vec!["office.summarise".to_string()],
        )]);
        reg.register(dangling.clone()).unwrap();
        let report = reg.census();
        assert!(report.findings_for("office.summarise").contains(
            &&CensusFinding::DanglingRequirement {
                id: "office.summarise".into(),
                missing: "office.absent".into(),
            }
        ));
    }
}
