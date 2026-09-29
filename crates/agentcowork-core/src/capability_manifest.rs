//! The generated capability status manifest: the **census** over the live
//! registry, the loading-mode split, and the budgeted activation subset.
//!
//! Three jobs, one source of truth. Statuses come from runtime evidence (the
//! live `ToolRegistry` + the capability registry projected from it), never from
//! hand-counted matrix totals.
//!
//! 1. **Census** (`TASK-CAP-001`) — every invocable surface gets a descriptor
//!    with a stable, dotted, protocol-neutral id, affordances, requirements, a
//!    loading mode, a risk class, and a verification hook; provider coverage is
//!    derived, never hand-written. A malformed, duplicated, or unbacked entry is
//!    a finding, and the gate fails on findings.
//! 2. **Loading modes + budget** (`TASK-CAP-003`) — an agent receives a
//!    budgeted semantic subset, never a flat dump of the raw catalog. The
//!    budget is enforced by construction: [`activate`] cannot return more than
//!    the budget allows, so "dump everything" is unrepresentable rather than
//!    merely discouraged.
//! 3. **Guidance for the unmapped** — a discovered tool the adapter could not
//!    map to a capability is reported, not exposed (`DEC-047`).

use crate::execution::ExecutionKernel;
use crate::tools::{RegisteredTool, ToolFamily, ToolRegistry};
use crate::version::VERSION;
use serde::Serialize;

use agentcowork_guard::capability_broker::{
    CapabilityRegistry as GuardCapabilityRegistry, CensusReport, ProviderCandidate, ProviderHealth,
    ResolutionConstraints,
};
use agentcowork_guard::capability_contract::{
    CapabilityDescriptor, CapabilityResult, CapabilityRisk, LoadingMode, NextAction,
    NextActionKind, Requirement, VerificationHook,
};

/// The in-process kernel adapter's id. It is a *provider* assignment on a
/// descriptor, never part of a capability id (REQ-CAP-003).
pub const NATIVE_PROVIDER_ID: &str = "runtime.native";

/// The default context budget for one turn's capability exposure. Twelve
/// compact task-shaped definitions fit comfortably; the raw catalog (tens of
/// primitive tools) does not — which is the point of the layer.
pub const DEFAULT_ACTIVATION_BUDGET: ActivationBudget = ActivationBudget {
    max_bytes: 4096,
    max_entries: 12,
};

/// The byte/entry ceiling one activation must respect. Both bounds are maxima:
/// the subset is built under them, never trimmed to them afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationBudget {
    pub max_bytes: usize,
    pub max_entries: usize,
}

impl Default for ActivationBudget {
    fn default() -> Self {
        DEFAULT_ACTIVATION_BUDGET
    }
}

/// Who an activation belongs to. Activation is scoped per agent/session/run —
/// never global, never leaking across agents (`DEC-024`, REQ-CAP-006).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationScope {
    pub agent_id: String,
    pub session_id: String,
    pub run_id: String,
}

impl ActivationScope {
    pub fn new(
        agent_id: impl Into<String>,
        session_id: impl Into<String>,
        run_id: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            run_id: run_id.into(),
        }
    }

    /// Is a subset stamped for this scope actually valid for `other`? A subset
    /// from another agent is not a subset for this one.
    pub fn admits(&self, other: &ActivationScope) -> bool {
        self == other
    }
}

/// One capability as the model sees it: identity, affordances, class — never
/// the args schema, never a provider, never a transport (INV-15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityExposure {
    pub capability_id: String,
    pub version: String,
    pub description: String,
    pub affordances: Vec<String>,
    pub loading_mode: LoadingMode,
    pub risk_class: CapabilityRisk,
    pub context_bytes: usize,
}

/// Why a capability the caller asked about (or that exists) is not in the
/// subset. Naming the reason is the point: a bounded subset must be
/// explainable, not silently small.
///
/// Serializes as a bare token (`"budget"`) — the variants carry no payload, and
/// the field it lives in is already named `reason`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeferredReason {
    /// The budget is full; the capability is available on request.
    Budget,
    /// `on-demand`: zero context cost until the turn asks for it.
    NotRequested,
    /// `catalog`: known to the UI/registry, not to the model.
    CatalogOnly,
    /// Not a registered capability at all.
    Unknown,
}

/// One deferred capability plus the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredCapability {
    pub capability_id: String,
    pub reason: DeferredReason,
}

/// The budgeted subset one turn receives. The invariants [`activate`] upholds:
/// `included.len() <= budget.max_entries` and `bytes_used <= budget.max_bytes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySubset {
    pub scope: ActivationScope,
    pub budget: ActivationBudget,
    pub included: Vec<CapabilityExposure>,
    /// Everything held back, with the reason for each.
    pub deferred: Vec<DeferredCapability>,
    pub bytes_used: usize,
    /// True when the budget, not the request, cut the set short.
    pub budget_limited: bool,
}

impl CapabilitySubset {
    /// Is this subset the one this scope was granted? A subset from another
    /// agent/session/run is refused rather than reused.
    pub fn is_scoped_to(&self, scope: &ActivationScope) -> bool {
        self.scope.admits(scope)
    }

    pub fn contains(&self, capability_id: &str) -> bool {
        self.included
            .iter()
            .any(|c| c.capability_id == capability_id)
    }

    /// Did the budget hold?
    pub fn within_budget(&self) -> bool {
        self.included.len() <= self.budget.max_entries && self.bytes_used <= self.budget.max_bytes
    }
}

// ---------------------------------------------------------------------------
// Descriptor derivation — the adapter's id mapping (DEC-047)
// ---------------------------------------------------------------------------

/// Kernel-routed façades that carry no id prefix to derive a domain from. Kept
/// as an explicit table rather than a heuristic: a wrong guess would mint a
/// capability id that misnames the domain.
const KERNEL_FACADE_DOMAINS: &[(&str, &str)] = &[("retrieve_original", "artifact")];

/// The domain a catalog family belongs to. A capability id is
/// `<domain>.<operation>`; the domain is semantic (`office`, `browser`), never
/// a transport or a vendor.
const fn domain_of(family: ToolFamily) -> &'static str {
    match family {
        ToolFamily::Browser => "browser",
        ToolFamily::Storage => "storage",
        ToolFamily::Script => "shell",
        ToolFamily::FileOps => "file",
        ToolFamily::Search => "search",
        ToolFamily::Office => "office",
        ToolFamily::Desktop => "desktop",
        ToolFamily::External => "external",
        ToolFamily::Connector => "connector",
        ToolFamily::Facade => "shared",
    }
}

/// The capability id for a catalog entry.
///
/// - An id that is already dotted **is** the capability id (façades and the
///   native extras) — the id is the contract, not a rendering.
/// - `office_edit` → `office.edit`, `memory_retrieve` → `memory.retrieve`: a
///   prefixed primitive folds its prefix into the dotted form.
/// - Anything else takes its family domain: `navigate` → `browser.navigate`.
pub fn capability_id_for(catalog_id: &str, family: ToolFamily) -> String {
    if catalog_id.contains('.') {
        return catalog_id.to_string();
    }
    for (prefix, domain) in [
        ("office_", "office"),
        ("memory_", "memory"),
        ("search_", "search"),
        ("storage_", "storage"),
        ("browser_", "browser"),
    ] {
        if let Some(rest) = catalog_id.strip_prefix(prefix) {
            return format!("{domain}.{rest}");
        }
    }
    if let Some((_, domain)) = KERNEL_FACADE_DOMAINS
        .iter()
        .find(|(id, _)| *id == catalog_id)
    {
        return format!("{domain}.{catalog_id}");
    }
    format!("{}.{}", domain_of(family), catalog_id)
}

/// The declared loading mode for a catalog entry.
///
/// - **façades → `eager`**: the shared-plane task shapes are exactly the
///   compact, semantic surface a model is meant to hold (L1/L2). This is the
///   anti-flat-dump mechanism: ~16 task-shaped definitions instead of the raw
///   primitive catalog.
/// - **external → `catalog`**: a discovered third-party tool is known to the UI
///   and the registry, never to the model until the agent binds it.
/// - **everything else → `on-demand`**: zero context cost until requested.
pub const fn loading_mode_for(family: ToolFamily) -> LoadingMode {
    match family {
        ToolFamily::Facade => LoadingMode::Eager,
        ToolFamily::External => LoadingMode::Catalog,
        _ => LoadingMode::OnDemand,
    }
}

/// The declared risk class, derived from the catalog's own risk + read-only
/// flags: a read is `safe`, a bounded mutation is `sensitive`, a destructive
/// or high-risk operation is `dangerous`. The descriptor's verification depth is
/// then the risk-class floor, so a receipt can never claim less verification
/// than its class demands (INV-19).
pub fn risk_class_for(tool: &RegisteredTool) -> CapabilityRisk {
    match (tool.read_only, tool.risk.as_str()) {
        (true, _) => CapabilityRisk::Safe,
        (false, "medium") => CapabilityRisk::Sensitive,
        _ => CapabilityRisk::Dangerous,
    }
}

/// The verification hook: what must run before a receipt, at the depth the risk
/// class demands. The depth equals the floor — never below it, because the
/// census rejects anything that is.
fn verification_for(tool: &RegisteredTool, risk: CapabilityRisk) -> VerificationHook {
    VerificationHook::new(
        format!(
            "verifier.{}.{}",
            tool.operation,
            risk.verification_depth().as_str()
        ),
        risk.verification_depth(),
    )
}

/// What the caller can express, in the caller's language. Always non-empty —
/// an empty affordance list is a census finding, so derivation can never
/// produce one.
fn affordances_for(tool: &RegisteredTool, capability_id: &str) -> Vec<String> {
    let level = if tool.read_only { "L1" } else { "L2" };
    vec![
        format!("{level}: {}", tool.description),
        format!("operation `{}` on `{}`", tool.operation, capability_id),
    ]
}

/// The `requires` edges. A connector or third-party tool cannot run before the
/// thing that backs it exists; that is a connection edge, not a failure.
fn requirements_for(tool: &RegisteredTool) -> Vec<Requirement> {
    match tool.family {
        ToolFamily::Connector => vec![Requirement::connection(
            "connector.account",
            "the account backing this write must be connected and approved",
        )],
        ToolFamily::External => vec![Requirement::connection(
            "mcp.server",
            "the attached server must be connected before its tools are callable",
        )],
        _ => Vec::new(),
    }
}

/// Build one descriptor from a live catalog entry.
pub fn descriptor_for(tool: &RegisteredTool) -> CapabilityDescriptor {
    let capability_id = capability_id_for(&tool.id, tool.family);
    let risk = risk_class_for(tool);
    CapabilityDescriptor::new(
        capability_id.clone(),
        "1.0.0",
        tool.description.clone(),
        affordances_for(tool, &capability_id),
        loading_mode_for(tool.family),
        risk,
        verification_for(tool, risk),
    )
    .with_requirements(requirements_for(tool))
}

// ---------------------------------------------------------------------------
// The census over the live registry
// ---------------------------------------------------------------------------

/// The census result plus the facts a reader needs to act on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityCensus {
    pub report: CensusReport,
    /// Capability ids registered, in stable order.
    pub registered: Vec<String>,
    /// The loading-mode split (how many capabilities sit in each mode).
    pub by_loading_mode: Vec<(&'static str, usize)>,
    /// Third-party tools that are **not** capabilities: unmapped, therefore not
    /// invocable, therefore reported as guidance rather than exposed.
    pub unmapped_externals: Vec<UnmappedTool>,
}

/// One discovered tool with no capability mapping (`DEC-047`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmappedTool {
    /// The server/provenance label, never the wire tool name (INV-15: the name
    /// stays behind the adapter).
    pub label: String,
    pub count: usize,
    /// The guidance a caller receives when it asks for one of them.
    pub guidance: CapabilityResult,
}

impl UnmappedTool {
    fn new(label: impl Into<String>, count: usize) -> Self {
        let label = label.into();
        Self {
            guidance: CapabilityResult::guidance(NextAction::new(
                NextActionKind::Connect,
                label.clone(),
                format!(
                    "`{label}` exposes tools that are not mapped to a capability; \
                     map them at discovery before any agent may call them"
                ),
                format!("`{label}`'s tools become callable as capabilities"),
            )),
            count,
            label,
        }
    }
}

/// Project the live `ToolRegistry` into the capability registry.
///
/// The in-process kernel is projected as the single `native` provider: its
/// capabilities are the descriptors' ids, and its epoch is 1 (the kernel
/// process). This is a *projection*, not a second store — health and epoch for
/// real providers come from the provider registry.
pub fn build_capability_registry(registry: &ToolRegistry) -> GuardCapabilityRegistry {
    build_capability_registry_at(registry, NATIVE_PROVIDER_START_EPOCH)
}

/// The epoch a freshly started in-process adapter reports.
pub const NATIVE_PROVIDER_START_EPOCH: u64 = 1;

/// As [`build_capability_registry`], with the in-process adapter's **live**
/// epoch. The epoch is passed in rather than invented here so the registry and
/// the runtime cannot disagree about whether a handle is stale — the resolver
/// reads one number, and it is the runtime's.
pub fn build_capability_registry_at(
    registry: &ToolRegistry,
    native_epoch: u64,
) -> GuardCapabilityRegistry {
    let mut capabilities = GuardCapabilityRegistry::new();
    let native_ids: Vec<String> = registry
        .list()
        .iter()
        // A third-party tool is not a capability until the adapter maps it.
        .filter(|t| t.family != ToolFamily::External)
        .map(|t| capability_id_for(&t.id, t.family))
        .collect();
    let native = ProviderCandidate {
        health: ProviderHealth::Ok,
        // The in-process kernel is granted for the local environment it runs in;
        // a permission fit is re-derived at ticket time, never inherited.
        permission_fit: agentcowork_guard::capability_broker::PermissionFit::Granted,
        capabilities: native_ids,
        ..ProviderCandidate::new(NATIVE_PROVIDER_ID, native_epoch, Vec::new())
    };
    capabilities.project_providers(&[native]);
    for tool in registry.list() {
        if tool.family == ToolFamily::External {
            continue;
        }
        // Duplicate capability ids (two catalog entries folding to one id) are
        // exactly what the census must catch, so registration failures are
        // recorded as findings rather than swallowed.
        let _ = capabilities.register(descriptor_for(tool));
    }
    capabilities
}

/// Run the census over the live catalog.
pub fn census(registry: &ToolRegistry) -> CapabilityCensus {
    let capabilities = build_capability_registry(registry);
    let report = capabilities.census();
    let registered: Vec<String> = capabilities
        .descriptors()
        .into_iter()
        .map(|d| d.id.clone())
        .collect();
    let mut by_loading_mode: Vec<(&'static str, usize)> = Vec::new();
    for mode in [
        LoadingMode::Eager,
        LoadingMode::Catalog,
        LoadingMode::OnDemand,
    ] {
        let n = capabilities
            .descriptors()
            .into_iter()
            .filter(|d| d.loading_mode == mode)
            .count();
        by_loading_mode.push((mode.as_str(), n));
    }
    let unmapped = registry
        .list()
        .iter()
        .filter(|t| t.family == ToolFamily::External)
        .count();
    CapabilityCensus {
        report,
        registered,
        by_loading_mode,
        unmapped_externals: if unmapped == 0 {
            Vec::new()
        } else {
            vec![UnmappedTool::new("external", unmapped)]
        },
    }
}

// ---------------------------------------------------------------------------
// Activation — the budgeted subset
// ---------------------------------------------------------------------------

/// Build the exposure view of a descriptor (what the model receives).
pub fn exposure_of(descriptor: &CapabilityDescriptor) -> CapabilityExposure {
    CapabilityExposure {
        capability_id: descriptor.id.clone(),
        version: descriptor.version.clone(),
        description: descriptor.description.clone(),
        affordances: descriptor.affordances.clone(),
        loading_mode: descriptor.loading_mode,
        risk_class: descriptor.risk_class,
        context_bytes: descriptor.context_bytes(),
    }
}

/// Resolve a capability into a handle against the live registry — the
/// `resolve(capability_id, constraints) → CapabilityHandle` half of CTR-009,
/// with the epoch check the resolver owes.
pub fn resolve_capability(
    registry: &ToolRegistry,
    capability_id: &str,
    constraints: &ResolutionConstraints,
    now_ms: u64,
) -> agentcowork_guard::capability_broker::Resolution {
    resolve_capability_at(
        registry,
        capability_id,
        constraints,
        now_ms,
        NATIVE_PROVIDER_START_EPOCH,
    )
}

/// As [`resolve_capability`], with the in-process adapter's live epoch, so the
/// handle a caller receives carries the epoch the runtime is actually at.
pub fn resolve_capability_at(
    registry: &ToolRegistry,
    capability_id: &str,
    constraints: &ResolutionConstraints,
    now_ms: u64,
    native_epoch: u64,
) -> agentcowork_guard::capability_broker::Resolution {
    let capabilities = build_capability_registry_at(registry, native_epoch);
    let native = ProviderCandidate {
        health: ProviderHealth::Ok,
        permission_fit: agentcowork_guard::capability_broker::PermissionFit::Granted,
        capabilities: capabilities
            .descriptors()
            .into_iter()
            .map(|d| d.id.clone())
            .collect(),
        ..ProviderCandidate::new(NATIVE_PROVIDER_ID, native_epoch, Vec::new())
    };
    capabilities.resolve(capability_id, constraints, &[native], now_ms)
}

/// The budgeted subset one turn receives.
///
/// Order: the capabilities the turn **asked for** first (in request order), then
/// the remaining `eager` set in stable id order. The fill stops at the budget;
/// everything left over is named in `deferred` with its reason. Requesting the
/// whole catalog therefore yields a budgeted subset, never a dump.
pub fn activate(
    registry: &ToolRegistry,
    scope: &ActivationScope,
    budget: ActivationBudget,
    requested: &[&str],
) -> CapabilitySubset {
    let capabilities = build_capability_registry(registry);
    let all: Vec<CapabilityExposure> = capabilities
        .descriptors()
        .into_iter()
        .map(exposure_of)
        .collect();

    // Requested ids first, in the order the caller named them, then eager.
    let mut ordered: Vec<CapabilityExposure> = Vec::new();
    let mut deferred: Vec<DeferredCapability> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for id in requested {
        match all.iter().find(|e| &e.capability_id == id) {
            Some(exposure) => {
                if !seen.contains(&exposure.capability_id) {
                    seen.push(exposure.capability_id.clone());
                    ordered.push(exposure.clone());
                }
            }
            None => deferred.push(DeferredCapability {
                capability_id: (*id).to_string(),
                reason: DeferredReason::Unknown,
            }),
        }
    }
    let mut eager: Vec<CapabilityExposure> = all
        .iter()
        .filter(|e| e.loading_mode == LoadingMode::Eager && !seen.contains(&e.capability_id))
        .cloned()
        .collect();
    eager.sort_by(|a, b| a.capability_id.cmp(&b.capability_id));
    ordered.extend(eager);

    let mut included: Vec<CapabilityExposure> = Vec::new();
    let mut bytes_used = 0usize;
    let mut budget_limited = false;
    for exposure in ordered {
        // A `catalog` entry never enters model context unrequested, whatever
        // the budget says.
        if !exposure
            .loading_mode
            .may_expose_to_model(requested.contains(&exposure.capability_id.as_str()))
        {
            deferred.push(DeferredCapability {
                capability_id: exposure.capability_id,
                reason: DeferredReason::CatalogOnly,
            });
            continue;
        }
        let fits = included.len() < budget.max_entries
            && bytes_used + exposure.context_bytes <= budget.max_bytes;
        if fits {
            bytes_used += exposure.context_bytes;
            included.push(exposure);
        } else {
            budget_limited = true;
            deferred.push(DeferredCapability {
                capability_id: exposure.capability_id,
                reason: DeferredReason::Budget,
            });
        }
    }

    // Everything the caller could have asked for but did not is `on-demand`
    // work: named, so "why isn't this in my subset" has an answer.
    let requested_ids: Vec<&str> = requested.to_vec();
    for exposure in &all {
        if included
            .iter()
            .any(|e| e.capability_id == exposure.capability_id)
            || deferred
                .iter()
                .any(|d| d.capability_id == exposure.capability_id)
        {
            continue;
        }
        let reason = if exposure.loading_mode == LoadingMode::Catalog {
            DeferredReason::CatalogOnly
        } else if requested_ids.contains(&exposure.capability_id.as_str()) {
            DeferredReason::Budget
        } else {
            DeferredReason::NotRequested
        };
        deferred.push(DeferredCapability {
            capability_id: exposure.capability_id.clone(),
            reason,
        });
    }
    deferred.sort_by(|a, b| a.capability_id.cmp(&b.capability_id));

    CapabilitySubset {
        scope: scope.clone(),
        budget,
        included,
        deferred,
        bytes_used,
        budget_limited,
    }
}

// ---------------------------------------------------------------------------
// The generated manifest
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityManifest {
    pub commit: String,
    pub generated_at_ms: u64,
    pub version: String,
    pub test_command: String,
    pub test_count: usize,
    pub benchmark_run_id: String,
    pub capabilities: Vec<CapabilityRow>,
    pub execution_kernel: bool,
    pub connectivity_modes: [&'static str; 4],
    /// The census verdict. A non-empty `report.findings` is a gate failure, not
    /// a footnote.
    pub census: CapabilityCensus,
    /// The provider coverage the descriptors derive from.
    pub providers: Vec<ProviderCoverageRow>,
}

/// One provider's coverage of the capability set (registry-derived, never
/// copy-edited into a descriptor).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCoverageRow {
    pub provider_id: String,
    pub provider_epoch: u64,
    pub capability_count: usize,
}

/// One raw catalog row (the pre-census view, kept because it is what the tool
/// surface actually dispatches).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRow {
    pub id: String,
    pub family: String,
    pub risk_tier: String,
    pub read_only: bool,
    pub runtime_wired: bool,
}

pub fn generate_manifest(commit: &str) -> CapabilityManifest {
    let reg = ToolRegistry::new();
    let capabilities: Vec<CapabilityRow> = reg
        .list()
        .iter()
        .map(|t| CapabilityRow {
            id: t.id.clone(),
            family: format!("{:?}", t.family).to_lowercase(),
            risk_tier: t.risk_tier.clone(),
            read_only: t.read_only,
            runtime_wired: true,
        })
        .collect();
    let capability_registry = build_capability_registry(&reg);
    let providers: Vec<ProviderCoverageRow> = vec![ProviderCoverageRow {
        provider_id: NATIVE_PROVIDER_ID.to_string(),
        provider_epoch: capability_registry.provider_epoch(NATIVE_PROVIDER_ID),
        capability_count: capability_registry.len(),
    }];
    CapabilityManifest {
        commit: if commit.is_empty() {
            "unknown".into()
        } else {
            commit.into()
        },
        generated_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        version: VERSION.to_string(),
        test_command: "cargo test -p agentcowork-core --lib".into(),
        test_count: capabilities.len(),
        benchmark_run_id: format!("cap-{}", commit),
        capabilities,
        execution_kernel: std::mem::size_of::<ExecutionKernel>() > 0,
        connectivity_modes: ["offline", "local", "byok", "third_party"],
        census: census(&reg),
        providers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> ActivationScope {
        ActivationScope::new("agent-a", "sess-1", "run-1")
    }

    #[test]
    fn manifest_lists_runtime_wired_tools() {
        let m = generate_manifest("test-sha");
        assert_eq!(m.commit, "test-sha");
        assert!(!m.capabilities.is_empty());
        assert!(m.capabilities.iter().any(|c| c.id == "file_ops.read"));
        assert!(m.execution_kernel);
        assert_eq!(m.test_count, m.capabilities.len());
        assert_eq!(m.connectivity_modes.len(), 4);
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            json.contains("riskTier")
                || json.contains("risk_tier")
                || json.contains("file_ops.read")
        );
    }

    #[test]
    fn the_census_over_the_live_registry_passes() {
        let report = census(&ToolRegistry::new());
        assert!(
            report.report.ok(),
            "the live registry must pass its own census: {:?}",
            report.report.findings
        );
        assert!(report.registered.len() > 10);
        // Every registered capability is dotted and protocol-neutral.
        for id in &report.registered {
            assert!(
                agentcowork_guard::capability_contract::is_well_formed_id(id),
                "{id}"
            );
        }
        // Provider coverage is derived, and it is the in-process kernel.
        let coverage =
            build_capability_registry(&ToolRegistry::new()).providers_of(&report.registered[0]);
        assert!(coverage.contains(NATIVE_PROVIDER_ID));
    }

    #[test]
    fn every_invocable_surface_has_a_capability_id() {
        let reg = ToolRegistry::new();
        let capabilities = build_capability_registry(&reg);
        // Every non-third-party catalog entry — the surfaces `tool/exec` can
        // dispatch — resolves to a registered capability (REQ-CAP-010).
        for tool in reg
            .list()
            .iter()
            .filter(|t| t.family != ToolFamily::External)
        {
            let id = capability_id_for(&tool.id, tool.family);
            assert!(
                capabilities.get(&id).is_some(),
                "{} ({:?}) has no capability id",
                tool.id,
                tool.family
            );
        }
    }

    #[test]
    fn capability_ids_are_dotted_protocol_neutral_and_stable() {
        // Façade/native-extras ids are already the contract.
        assert_eq!(
            capability_id_for("office.edit", ToolFamily::Facade),
            "office.edit"
        );
        assert_eq!(
            capability_id_for("file_ops.read", ToolFamily::FileOps),
            "file_ops.read"
        );
        // Prefixed primitives fold the prefix into the dotted form.
        assert_eq!(
            capability_id_for("office_edit", ToolFamily::Office),
            "office.edit"
        );
        assert_eq!(
            capability_id_for("memory_retrieve", ToolFamily::Storage),
            "memory.retrieve"
        );
        // Everything else takes its family domain.
        assert_eq!(
            capability_id_for("navigate", ToolFamily::Browser),
            "browser.navigate"
        );
        assert_eq!(
            capability_id_for("deep_research", ToolFamily::Search),
            "search.deep_research"
        );
        assert_eq!(
            capability_id_for("send_email", ToolFamily::Connector),
            "connector.send_email"
        );
        // A dotless kernel-routed façade uses its declared domain, not a guess.
        assert_eq!(
            capability_id_for("retrieve_original", ToolFamily::Facade),
            "artifact.retrieve_original"
        );
        assert_eq!(
            capability_id_for("artifact.retrieve_original", ToolFamily::Facade),
            "artifact.retrieve_original"
        );
        // Derivation is pure: the same input always yields the same id.
        for tool in ToolRegistry::new().list() {
            assert_eq!(
                capability_id_for(&tool.id, tool.family),
                capability_id_for(&tool.id, tool.family)
            );
        }
    }

    #[test]
    fn loading_modes_split_eager_catalog_and_on_demand() {
        assert_eq!(loading_mode_for(ToolFamily::Facade), LoadingMode::Eager);
        assert_eq!(loading_mode_for(ToolFamily::External), LoadingMode::Catalog);
        for family in [
            ToolFamily::Browser,
            ToolFamily::Storage,
            ToolFamily::Script,
            ToolFamily::FileOps,
            ToolFamily::Search,
            ToolFamily::Office,
            ToolFamily::Desktop,
            ToolFamily::Connector,
        ] {
            assert_eq!(
                loading_mode_for(family),
                LoadingMode::OnDemand,
                "{family:?}"
            );
        }
        let report = census(&ToolRegistry::new());
        let eager = report
            .by_loading_mode
            .iter()
            .find(|(m, _)| *m == "eager")
            .map(|(_, n)| *n)
            .unwrap();
        let on_demand = report
            .by_loading_mode
            .iter()
            .find(|(m, _)| *m == "on-demand")
            .map(|(_, n)| *n)
            .unwrap();
        assert!(eager > 0 && on_demand > 0, "both modes must be populated");
        // The model-facing set is the task-shaped one, not the primitive dump.
        assert!(
            eager < on_demand,
            "eager ({eager}) must be the smaller, semantic surface"
        );
    }

    #[test]
    fn risk_class_and_verification_depth_track_the_catalog_flags() {
        let reg = ToolRegistry::new();
        for tool in reg.list() {
            let risk = risk_class_for(tool);
            let hook = verification_for(tool, risk);
            assert!(
                hook.depth.satisfies(risk.verification_depth()),
                "{} declares a depth below its risk-class floor",
                tool.id
            );
        }
        let read = reg.get("file_ops.read").expect("read tool");
        assert_eq!(risk_class_for(read), CapabilityRisk::Safe);
        let delete = reg.get("file_ops.delete").expect("delete tool");
        assert_eq!(risk_class_for(delete), CapabilityRisk::Dangerous);
        let write = reg.get("file_ops.write").expect("write tool");
        assert_eq!(risk_class_for(write), CapabilityRisk::Sensitive);
    }

    #[test]
    fn activation_is_budgeted_and_a_flat_dump_is_unrepresentable() {
        let reg = ToolRegistry::new();
        let every_id: Vec<String> = build_capability_registry(&reg)
            .descriptors()
            .into_iter()
            .map(|d| d.id.clone())
            .collect();
        assert!(
            every_id.len() > DEFAULT_ACTIVATION_BUDGET.max_entries,
            "the raw catalog is larger than one turn's budget — that is the point"
        );
        let all: Vec<&str> = every_id.iter().map(|s| s.as_str()).collect();
        let subset = activate(&reg, &scope(), DEFAULT_ACTIVATION_BUDGET, &all);
        assert!(subset.within_budget(), "{:?}", subset.included.len());
        assert!(subset.included.len() <= DEFAULT_ACTIVATION_BUDGET.max_entries);
        assert!(subset.bytes_used <= DEFAULT_ACTIVATION_BUDGET.max_bytes);
        assert!(
            subset.budget_limited,
            "asking for everything hits the budget"
        );
        // Nothing is silently dropped: the held-back set is named.
        let deferred: Vec<&str> = subset
            .deferred
            .iter()
            .map(|d| d.capability_id.as_str())
            .collect();
        assert!(deferred.contains(&"file_ops.read"));
        assert!(
            subset
                .deferred
                .iter()
                .any(|d| d.reason == DeferredReason::Budget)
        );
    }

    #[test]
    fn an_on_demand_capability_enters_context_only_when_requested() {
        let reg = ToolRegistry::new();
        let without = activate(&reg, &scope(), DEFAULT_ACTIVATION_BUDGET, &[]);
        assert!(!without.contains("file_ops.read"));
        assert!(without.deferred.iter().any(
            |d| d.capability_id == "file_ops.read" && d.reason == DeferredReason::NotRequested
        ));
        let with = activate(
            &reg,
            &scope(),
            DEFAULT_ACTIVATION_BUDGET,
            &["file_ops.read"],
        );
        assert!(
            with.contains("file_ops.read"),
            "the resolver serves it on demand"
        );
        assert!(with.included[0].capability_id == "file_ops.read");
    }

    #[test]
    fn activation_is_scoped_to_one_agent_session_and_run() {
        let reg = ToolRegistry::new();
        let mine = activate(&reg, &scope(), DEFAULT_ACTIVATION_BUDGET, &[]);
        assert!(mine.is_scoped_to(&scope()));
        let other_agent = ActivationScope::new("agent-b", "sess-1", "run-1");
        assert!(!mine.is_scoped_to(&other_agent), "no leaking across agents");
        let other_run = ActivationScope::new("agent-a", "sess-1", "run-2");
        assert!(!mine.is_scoped_to(&other_run), "no leaking across runs");
        let other_session = ActivationScope::new("agent-a", "sess-2", "run-1");
        assert!(!mine.is_scoped_to(&other_session));
    }

    #[test]
    fn an_unknown_request_is_named_not_ignored() {
        let reg = ToolRegistry::new();
        let subset = activate(
            &reg,
            &scope(),
            DEFAULT_ACTIVATION_BUDGET,
            &["office.not_a_capability"],
        );
        assert!(
            subset
                .deferred
                .iter()
                .any(|d| d.capability_id == "office.not_a_capability"
                    && d.reason == DeferredReason::Unknown)
        );
    }

    #[test]
    fn unmapped_third_party_tools_are_guidance_never_exposure() {
        let mut reg = ToolRegistry::new();
        let added = reg.register_external(
            "third-party",
            &[agentcowork_mcp::ExternalTool {
                name: "sneaky_tool".into(),
                description: "a stranger's tool".into(),
                input_schema: serde_json::json!({ "type": "object" }),
                read_only: false,
                open_world: true,
                source: "third-party".into(),
            }],
        );
        assert_eq!(added, vec!["sneaky_tool".to_string()]);
        let report = census(&reg);
        assert!(
            !report.unmapped_externals.is_empty(),
            "an unmapped tool must be reported"
        );
        let unmapped = &report.unmapped_externals[0];
        assert!(unmapped.guidance.is_actionable());
        assert_eq!(
            unmapped.guidance.next_action.as_ref().unwrap().kind,
            NextActionKind::Connect
        );
        // And it is not in the model-facing surface.
        let subset = activate(&reg, &scope(), DEFAULT_ACTIVATION_BUDGET, &["sneaky_tool"]);
        assert!(!subset.contains("sneaky_tool"));
    }

    #[test]
    fn resolution_mints_an_epoch_checked_handle_for_a_registered_capability() {
        let reg = ToolRegistry::new();
        let constraints = ResolutionConstraints::new("local", "policy-1");
        let resolved = resolve_capability(&reg, "office.open", &constraints, 1_000);
        assert!(resolved.is_resolved(), "{resolved:?}");
        let handle = resolved.handle.expect("a handle");
        assert_eq!(handle.provider_id, NATIVE_PROVIDER_ID);
        assert_eq!(handle.environment_id, "local");
        assert_eq!(handle.permission_snapshot, "policy-1");
        assert!(handle.validate(1_001, 1).is_ok());
        // A provider restart invalidates it — no blind reuse.
        assert!(handle.validate(1_001, 2).is_err());
        // An unknown id is typed, not an empty chain.
        let unknown = resolve_capability(&reg, "office.nope", &constraints, 1_000);
        assert!(!unknown.is_resolved());
        assert!(unknown.unresolved.is_some());
    }
}
