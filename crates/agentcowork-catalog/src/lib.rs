//! P14 — Model catalog (doc 66, models.dev steal).
//!
//! A vendorable model-capability index: one `ModelEntry` per provider/model
//! (compiled shape), the two-tier lab/provider schema, a cache-aware cost
//! engine, and the routing filter matrix. Parsed once at startup; the router
//! and cost display read from it — nothing here mutates.

pub mod catalog;
pub mod discovery;
pub mod fetch;
pub mod gallery;
pub mod gateway;
pub mod live;
pub mod model;
pub mod observations;
pub mod policy;
pub mod pricing;
pub mod probe;
pub mod profiles;
pub mod provider;
pub mod provider_adapter;
pub mod provider_seed;
pub mod routing;
pub mod routing_feed;
pub mod store;
pub mod sync;
pub mod tier;

pub use catalog::ModelCatalog;
pub use discovery::{
    DiscoveryInventory, ManagedResource, ResourceCard, ResourceCounts, ResourceKind,
};
pub use fetch::{
    EndpointProbe, HttpFetch, RefreshOutcome, count_models, endpoint_probe_result,
    probe_models_endpoint, refresh_now,
};
pub use gateway::{GatewayError, GatewayRouter, RouteResult, TaskHint};
pub use live::{
    CatalogSnapshot, DEFAULT_REFRESH_SECS, FetchOutcome, LOGO_URL_PREFIX, LiveModel, LiveProvider,
    MAX_REFRESH_SECS, MIN_REFRESH_SECS, MODELS_DEV_API_URL, Modalities, ModelCost, ModelLimit,
    ModelProviderOverride, RefreshDecision, apply_refresh, free_model_ids, is_free_model_id,
    is_stale, logo_url, refresh_interval_secs, transport_from_npm,
};
pub use model::ModelEntry;
pub use observations::{
    ObservationStore, ProbePolicy, ProviderObservation, ProviderObservationsFile, Reachability,
    apply_observation_health, apply_observations, health_of,
};
pub use pricing::{CostBreakdown, cost_for, split_input};
pub use probe::{
    AdvertisedHardCaps, Capability, CapabilityVerdict, ProbeResult, Verdict, VerificationReport,
    trusted_capabilities,
};
pub use profiles::{
    ProfileFormat, ProfileModel, ProfileSource, ProfileStore, ProviderProfile,
    ProviderProfilesFile, nvidia_nim_profile, opencode_overlay_profiles,
};
pub use provider::{
    ALIASES, AggregatorKind, Auth, DiscoverySource, OPENAI_COMPATIBLE_PROFILES, ProviderRecord,
    ProviderRegistry, Transport, base_registry, normalize,
};
pub use provider_adapter::{
    ADAPTER_CLASS_COUNT, AdapterClass, AdapterError, AdapterRegistry, AuthMethod, AuthSpec,
    CapabilityInvocation, CustodyFinding, EgressDeclaration, GatewayIdentityPolicy,
    InvocationRefusal, ProviderEvent, ProviderHealth, ProviderInfo, ProviderRegistration,
    RegistrationError, TransportToolMapping, custody_findings, validate_registration,
};
pub use routing::{RouteFilters, rejection_reasons};
pub use routing_feed::{
    ExcludedProvider, Health, RankedProvider, RouteDecision, RouteRequirements, RoutingFeed,
};
pub use store::{
    CatalogMeta, CatalogSettings, CatalogStore, META_FILE, SETTINGS_FILE, SNAPSHOT_FILE,
};
pub use sync::{
    GateFinding, RefreshReport, SYNC_MODULES, Severity, SyncSpec, gate_passes, merge_refresh,
    refresh_plan, validate_vendored,
};
pub use tier::{ProviderOverride, ResolvedModel, validate_tiers};
