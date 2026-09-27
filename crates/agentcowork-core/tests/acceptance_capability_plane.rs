//! `TASK-CAP-001`/`002`/`003` + `TASK-PROV-004`/`005` — the capability and
//! provider acceptance lines, driven through the **live** kernel surface rather
//! than through types in isolation.
//!
//! What this pins, from `ARCH/08-REQUIREMENTS.md`:
//!
//! - `REQ-CAP-001` / `REQ-CAP-006`: the model-facing plane is a budgeted,
//!   loading-mode-honouring subset; asking for the whole catalog still returns a
//!   subset, and a flat dump is not representable.
//! - `REQ-CAP-002`: a handle minted before a provider restart is refused by
//!   `tool/exec`, which answers "re-resolve" instead of retrying.
//! - `REQ-CAP-004` / `REQ-CAP-010`: the census over the live catalog passes, and
//!   every invocable non-third-party surface resolves to a capability id.
//! - `REQ-CAP-005`: an unresolvable capability yields guidance with a next
//!   action, not a dead end.
//! - `REQ-PROV-001` / `REQ-PROV-003` / `REQ-PROV-007`: a configured provider
//!   registers in the declared entry shape (one class, distinct catalog/transport
//!   refs, a typed auth method holding only a reference), and the derived
//!   capability ids carry no provider identity.
//! - `REQ-PROV-009`: the identity the transport injects matches the policy the
//!   provider registry declares.

use std::sync::{Arc, Mutex};

use agentcowork_core::capability_manifest::{
    DEFAULT_ACTIVATION_BUDGET, DeferredReason, activate, build_capability_registry,
    capability_id_for, census, descriptor_for, loading_mode_for, risk_class_for,
};
use agentcowork_core::guard_service::GuardService;
use agentcowork_core::providers::{
    ProvidersFile, effective_base_url, gateway_identity_conforms, provider_registrations,
    wire_client_identity,
};
use agentcowork_core::tools::{ToolFamily, ToolRegistry, ToolService};
use agentcowork_guard::capability_broker::ResolutionConstraints;
use agentcowork_guard::capability_contract::{CapabilityRisk, LoadingMode, is_well_formed_id};
use serde_json::json;

/// A tool service over a throwaway workspace. The guard is the real one, so the
/// capability surface is exercised against the same policy engine production
/// uses.
fn service(name: &str) -> ToolService {
    let dir = std::env::temp_dir().join(format!("agentcowork-cap-plane-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    ToolService::new(Arc::new(Mutex::new(GuardService::new())), dir)
}

/// REQ-CAP-001 + REQ-CAP-006 — through the kernel's own `tool/list` plane, the
/// budget holds and a request for everything still returns a subset.
#[test]
fn the_capability_plane_is_budgeted_wherever_it_is_read_from() {
    let mut service = service("t1");
    let every_id: Vec<String> = build_capability_registry(&ToolRegistry::new())
        .descriptors()
        .into_iter()
        .map(|d| d.id.clone())
        .collect();
    assert!(
        every_id.len() > DEFAULT_ACTIVATION_BUDGET.max_entries,
        "the live catalog must be larger than one turn's budget, or the layer is untested"
    );

    let response = service
        .handle(
            "tool/list",
            &json!({
                "plane": "capabilities",
                "sessionId": "s1",
                "agentId": "agent-a",
                "runId": "run-1",
                "requested": every_id,
            }),
        )
        .expect("the capability plane answers");
    let included = response["included"].as_array().expect("included");
    assert!(!included.is_empty());
    assert!(included.len() <= DEFAULT_ACTIVATION_BUDGET.max_entries);
    assert!(response["bytesUsedMs"].is_null() || response["bytesUsedMs"].is_number());
    let bytes_used = response["bytesUsed"].as_u64().expect("bytes used");
    assert!(bytes_used <= DEFAULT_ACTIVATION_BUDGET.max_bytes as u64);
    assert_eq!(
        response["budgetLimited"],
        json!(true),
        "asking for the whole catalog hits the budget"
    );
    // The raw catalog is not in the response — no `tools` array, no args schema.
    assert!(
        response.get("tools").is_none(),
        "a flat dump is not representable"
    );
    for entry in included {
        assert!(entry["capabilityId"].is_string());
        assert!(!entry["affordances"].as_array().unwrap().is_empty());
        // A provider id never appears on an exposure: the caller speaks
        // capability ids only (INV-15 · REQ-PROV-001).
        assert!(entry.get("providerId").is_none());
        assert!(entry.get("transportRef").is_none());
    }
    // The held-back capabilities are named with a reason.
    let deferred = response["deferred"].as_array().expect("deferred");
    assert!(deferred.iter().any(|d| d["reason"] == json!("budget")));
    assert!(
        deferred
            .iter()
            .any(|d| d["capabilityId"] == json!("file_ops.read"))
    );
}

/// REQ-CAP-006 — activation is scoped: a subset minted for one agent/session/run
/// is not another one's, and the kernel echoes the scope it was given.
#[test]
fn activation_is_scoped_to_the_turn_that_requested_it() {
    let mut service = service("t2");
    let read = service
        .handle(
            "tool/list",
            &json!({
                "plane": "capabilities",
                "sessionId": "s1",
                "agentId": "agent-a",
                "runId": "run-1",
            }),
        )
        .expect("subset");
    assert_eq!(read["scope"]["agentId"], json!("agent-a"));
    assert_eq!(read["scope"]["sessionId"], json!("s1"));
    assert_eq!(read["scope"]["runId"], json!("run-1"));

    // The same turn, asked for a specific on-demand capability, gets it; a
    // different turn's subset is a different object.
    let on_demand = service
        .handle(
            "tool/list",
            &json!({
                "plane": "capabilities",
                "sessionId": "s1",
                "agentId": "agent-a",
                "runId": "run-1",
                "requested": ["file_ops.read"],
            }),
        )
        .expect("subset");
    let ids: Vec<String> = on_demand["included"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["capabilityId"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.first().map(String::as_str), Some("file_ops.read"));
    assert_ne!(on_demand, read, "a different request is a different subset");
}

/// REQ-CAP-002 — a handle minted at epoch N is refused once the provider has
/// restarted, and the refusal asks for re-resolution rather than a retry.
#[test]
fn a_stale_handle_is_refused_and_re_resolution_is_required() {
    let mut service = service("t3");
    let epoch = service.provider_epoch();
    let resolved = service
        .handle(
            "capability/resolve",
            &json!({
                "capabilityId": "office.open",
                "environmentId": "local",
                "policySnapshot": "policy-1",
            }),
        )
        .expect("resolve");
    assert_eq!(resolved["resolved"], json!(true));
    assert!(resolved["handle"]["runtimeHandleRef"].is_string());
    // The chain is returned to a caller that asked for it, and the handle is
    // bound to the live epoch.
    assert!(!resolved["chain"].as_array().unwrap().is_empty());

    // A fresh provider epoch: the old handle is stale.
    let bumped = service.restart_provider();
    assert_eq!(bumped, epoch + 1);
    let stale = service
        .handle(
            "tool/exec",
            &json!({
                "toolId": "file_ops.read",
                "args": { "path": "x" },
                "sessionId": "s1",
                "providerEpoch": epoch,
            }),
        )
        .expect("the executor answers");
    assert_eq!(stale["action"], json!("re-resolve"));
    assert_eq!(stale["providerEpoch"], json!(bumped));
    assert_eq!(stale["retryable"], json!(false));
    assert!(
        stale["reason"].as_str().unwrap().contains("re-resolve"),
        "the refusal names the next step: {}",
        stale["reason"]
    );

    // Re-resolving against the live epoch is accepted by the same check.
    let fresh = service
        .handle(
            "capability/resolve",
            &json!({
                "capabilityId": "office.open",
                "environmentId": "local",
                "policySnapshot": "policy-1",
            }),
        )
        .expect("resolve");
    let live_epoch = fresh["handle"]["providerEpoch"].as_u64().unwrap();
    let ok = service
        .handle(
            "tool/exec",
            &json!({
                "toolId": "file_ops.read",
                "args": { "path": "x" },
                "sessionId": "s1",
                "providerEpoch": live_epoch,
            }),
        )
        .expect("the executor answers");
    assert!(
        ok["action"] != json!("re-resolve"),
        "a handle at the live epoch passes the check: {ok}"
    );
}

/// REQ-CAP-004 + REQ-CAP-010 — the census over the live catalog, and coverage of
/// every invocable surface by a capability id.
#[test]
fn the_live_catalog_passes_its_census_and_covers_every_invocable_surface() {
    let registry = ToolRegistry::new();
    let report = census(&registry);
    assert!(
        report.report.ok(),
        "the live registry must pass its own census: {:?}",
        report.report.findings
    );
    assert!(report.registered.len() > 10);
    for id in &report.registered {
        assert!(is_well_formed_id(id), "{id} is not a dotted, neutral id");
    }
    // Every non-third-party catalog entry — everything `tool/exec` can dispatch —
    // has a registered capability behind it.
    let capabilities = build_capability_registry(&registry);
    for tool in registry.list() {
        if tool.family == ToolFamily::External {
            continue;
        }
        let id = capability_id_for(&tool.id, tool.family);
        let descriptor = capabilities
            .get(&id)
            .unwrap_or_else(|| panic!("{} has no capability descriptor", tool.id));
        assert!(!descriptor.affordances.is_empty());
        assert!(!descriptor.verification.hook.is_empty());
        assert!(
            descriptor
                .verification
                .depth
                .satisfies(descriptor.risk_class.verification_depth()),
            "{} under-declares verification",
            tool.id
        );
    }
    // Provider coverage is derived, and it is the in-process kernel.
    assert_eq!(
        capabilities
            .providers_of("office.open")
            .into_iter()
            .collect::<Vec<_>>(),
        vec!["runtime.native".to_string()]
    );
}

/// REQ-CAP-005 — an unresolvable capability is guidance with a next action, and
/// the guidance is reachable through the kernel surface.
#[test]
fn an_unresolvable_capability_is_guidance_not_a_dead_end() {
    let mut service = service("t4");
    let unknown = service
        .handle(
            "capability/resolve",
            &json!({ "capabilityId": "office.definitely_not_registered" }),
        )
        .expect("resolve answers");
    assert_eq!(unknown["resolved"], json!(false));
    assert!(unknown["handle"].is_null(), "no handle is invented");
    assert!(
        unknown["status"].is_string(),
        "a status, not an error string"
    );
    assert_eq!(unknown["error"]["code"], json!("not_found"));
    let action = &unknown["nextAction"];
    assert!(
        action["instruction"]
            .as_str()
            .unwrap()
            .contains("not a registered capability")
    );
    assert!(action["unlocks"].as_str().is_some());
}

/// REQ-CAP-010 failure — dispatch rejects an unregistered id. Pinned here so the
/// census coverage above and the dispatch guard below cannot drift apart.
#[test]
fn dispatch_rejects_an_unregistered_capability_id() {
    let mut service = service("t5");
    let err = service
        .handle(
            "tool/exec",
            &json!({ "toolId": "not_a_real_tool", "args": {} }),
        )
        .expect_err("an unregistered id cannot be dispatched");
    assert!(err.contains("unknown tool"), "{err}");
}

/// `REQ-PROV-003` / `REQ-PROV-007` — a configured provider registers in the
/// declared shape, with the auth method holding a reference and never a value.
#[test]
fn a_configured_provider_registers_in_the_declared_entry_shape() {
    let file: ProvidersFile = toml::from_str(
        r#"
[[providers]]
name = "openai"
base_url = "https://api.openai.com/v1"

[[providers.keys]]
id = "prod-1"
value = "sk-super-secret-value"
"#,
    )
    .expect("parse");
    let entries = provider_registrations(&file).expect("registers");
    let entry = &entries[0];
    assert_eq!(entry.id, "openai");
    assert_eq!(
        entry.class,
        agentcowork_catalog::provider_adapter::AdapterClass::Http
    );
    assert_eq!(entry.catalog_ref, "catalog/openai");
    assert_ne!(entry.id, entry.catalog_ref);
    assert_eq!(
        entry.health,
        agentcowork_catalog::provider_adapter::ProviderHealth::Unknown
    );
    assert_eq!(entry.epoch, 1);
    assert_eq!(
        entry.auth.method,
        agentcowork_catalog::provider_adapter::AuthMethod::Api
    );
    assert_eq!(entry.auth.secret_ref, "vault://provider/openai/prod-1");
    // The key's value never reaches the registry — custody, checked on the
    // serialized form.
    let json = serde_json::to_string(&entries).unwrap();
    assert!(!json.contains("sk-super-secret-value"));
    assert!(
        agentcowork_catalog::custody_findings(&serde_json::to_value(&entries).unwrap()).is_empty()
    );
    // And the host the floor check will run against is the configured one.
    assert_eq!(
        entry
            .egress
            .as_ref()
            .unwrap()
            .normalized_hosts()
            .into_iter()
            .collect::<Vec<_>>(),
        vec!["api.openai.com".to_string()]
    );
    // The base URL the entry declares is the one the transport will dial.
    assert_eq!(
        effective_base_url(&file.providers[0]).as_deref(),
        Some("https://api.openai.com/v1")
    );
}

/// `REQ-PROV-009` — the client identity on the wire matches the identity policy
/// the provider registry declares, and affinity is one value per conversation.
#[test]
fn the_gateway_identity_matches_the_declared_policy() {
    assert!(
        gateway_identity_conforms(),
        "the injected headers must match the declared GatewayIdentityPolicy"
    );
    let policy = agentcowork_catalog::GatewayIdentityPolicy::default();
    let value = |headers: &[(String, String)], name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let one = wire_client_identity("conv-1");
    let two = wire_client_identity("conv-2");
    assert_eq!(value(&one, &policy.session_header), "conv-1");
    assert_eq!(value(&two, &policy.session_header), "conv-2");
    // Identity is stable for the process and is our own name.
    assert_eq!(value(&one, "user-agent"), value(&two, "user-agent"));
    assert!(value(&one, "user-agent").starts_with("AgentCowork/"));
}

/// `REQ-CAP-003` — the derived capability ids name the operation, not the
/// provider, and the risk class tracks the catalog's own flags.
#[test]
fn capability_ids_describe_what_and_never_who() {
    let registry = ToolRegistry::new();
    for tool in registry.list() {
        let id = capability_id_for(&tool.id, tool.family);
        assert!(is_well_formed_id(&id), "{id} (from {})", tool.id);
        let descriptor = descriptor_for(tool);
        assert_eq!(descriptor.id, id);
        // No provider identity anywhere on the descriptor.
        assert!(
            descriptor.providers.is_empty(),
            "providers are derived later"
        );
        let serialized = serde_json::to_string(&descriptor).unwrap().to_lowercase();
        // The `providers` field exists (it is derived, not absent) — what must
        // not appear is a provider *id* or a transport name.
        for leak in ["runtime.native", "mcp.", "http://", "wire:", "vault://"] {
            assert!(
                !serialized.contains(leak),
                "{id} leaks `{leak}`: {serialized}"
            );
        }
    }
    // Risk classes follow the catalog flags, and loading modes follow the family.
    let read = registry.get("file_ops.read").expect("read");
    let write = registry.get("file_ops.write").expect("write");
    let delete = registry.get("file_ops.delete").expect("delete");
    assert_eq!(risk_class_for(read), CapabilityRisk::Safe);
    assert_eq!(risk_class_for(write), CapabilityRisk::Sensitive);
    assert_eq!(risk_class_for(delete), CapabilityRisk::Dangerous);
    assert_eq!(loading_mode_for(ToolFamily::Facade), LoadingMode::Eager);
    assert_eq!(loading_mode_for(ToolFamily::External), LoadingMode::Catalog);
    assert_eq!(loading_mode_for(ToolFamily::Browser), LoadingMode::OnDemand);
}

/// `REQ-PROV-001` — the resolver is the only place a provider id appears, and a
/// caller that does not ask for the chain never sees one.
#[test]
fn provider_identity_stays_below_the_capability_layer() {
    let registry = ToolRegistry::new();
    let constraints = ResolutionConstraints::new("local", "policy-1");
    let resolution = agentcowork_core::capability_manifest::resolve_capability(
        &registry,
        "office.open",
        &constraints,
        1_000,
    );
    assert!(resolution.is_resolved());
    // The handle names the provider because it *is* the binding to it — that is
    // the adapter-side reference, not something the model is shown.
    let handle = resolution.handle.expect("handle");
    assert!(!handle.runtime_handle_ref.is_empty());
    assert!(
        !handle.runtime_handle_ref.to_lowercase().contains("sk-"),
        "a handle never carries credential material"
    );
    // The subset a model receives is built from descriptors only.
    let subset = activate(
        &registry,
        &agentcowork_core::capability_manifest::ActivationScope::new("a", "s", "r"),
        DEFAULT_ACTIVATION_BUDGET,
        &["office.open"],
    );
    let serialized = serde_json::to_string(&subset.included).unwrap();
    assert!(!serialized.contains("runtime.native"), "{serialized}");
    assert!(subset.contains("office.open"));
    assert!(subset.within_budget());
}

/// The census is a gate, not a report: a budgeted subset is still budgeted when
/// the caller passes a budget of one entry.
#[test]
fn a_tiny_budget_yields_a_tiny_subset() {
    let registry = ToolRegistry::new();
    let budget = agentcowork_core::capability_manifest::ActivationBudget {
        max_bytes: 200,
        max_entries: 1,
    };
    let subset = activate(
        &registry,
        &agentcowork_core::capability_manifest::ActivationScope::new("a", "s", "r"),
        budget,
        &[],
    );
    assert_eq!(subset.included.len(), 1);
    assert!(subset.within_budget());
    assert!(subset.budget_limited);
    assert!(
        subset
            .deferred
            .iter()
            .any(|d| d.reason == DeferredReason::Budget)
    );
}

/// An attached third-party server's tools are reported as unmapped guidance and
/// never enter the model-facing subset.
#[test]
fn third_party_tools_are_reported_as_unmapped_and_never_exposed() {
    let mut registry = ToolRegistry::new();
    let names = registry.register_external(
        "third-party",
        &[agentcowork_mcp::ExternalTool {
            name: "stranger_tool".into(),
            description: "a stranger's tool".into(),
            input_schema: json!({ "type": "object" }),
            read_only: false,
            open_world: true,
            source: "third-party".into(),
        }],
    );
    assert_eq!(names, vec!["stranger_tool".to_string()]);
    let report = census(&registry);
    assert!(!report.unmapped_externals.is_empty());
    let unmapped = &report.unmapped_externals[0];
    assert!(unmapped.guidance.is_actionable());
    // The census itself still passes — an unmapped tool is a *report*, not a
    // malformed capability.
    assert!(report.report.ok(), "{:?}", report.report.findings);
    let subset = activate(
        &registry,
        &agentcowork_core::capability_manifest::ActivationScope::new("a", "s", "r"),
        DEFAULT_ACTIVATION_BUDGET,
        &["stranger_tool"],
    );
    assert!(!subset.contains("stranger_tool"));
    assert!(
        subset
            .deferred
            .iter()
            .any(|d| d.capability_id == "stranger_tool" && d.reason == DeferredReason::Unknown)
    );
}

/// The attach path advances the provider epoch, because attaching a provider is
/// that provider starting.
#[test]
fn attaching_a_provider_advances_the_epoch() {
    let mut service = service("t6");
    let before = service.provider_epoch();
    let names = service.attach_external_server(
        "third-party",
        &[agentcowork_mcp::ExternalTool {
            name: "stranger_tool".into(),
            description: "a stranger's tool".into(),
            input_schema: json!({ "type": "object" }),
            read_only: true,
            open_world: false,
            source: "third-party".into(),
        }],
        Arc::new(NoopExternal),
    );
    assert_eq!(names, vec!["stranger_tool".to_string()]);
    assert_eq!(
        service.provider_epoch(),
        before + 1,
        "a provider starting is an epoch bump"
    );
}

/// A no-op external backend: this test is about the catalog/epoch effect, not
/// about reaching the peer.
struct NoopExternal;

impl agentcowork_core::tools::ExternalToolBackend for NoopExternal {
    fn call(&self, _tool_id: &str, _args: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err("not attached".into())
    }
}
