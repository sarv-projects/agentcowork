//! P44.7 — Discovery surface ("AgentCowork is ready" boot).
//!
//! One inventory surface across every resource class the product manages —
//! Agents / Models / Providers / MCP / Skills / Browsers — with per-resource
//! cards (id · version · source · auth-shape · capabilities · governance ·
//! lifecycle status). The router reads the same inventory (P44.8) so a newly
//! discovered/enabled resource is usable immediately.
//!
//! ## The core distinction: discovery ≠ installation ≠ activation
//! A resource moves through the [`ManagedResource`] lifecycle:
//! `Discovered → Validated → Installed → Inventoried → Enabled → Started →
//! Healthy → InUse → (Updating / RollingBack / Removed)`. Discovery only
//! *finds and describes*; it never installs, enables, or activates, and it
//! **never harvests secrets** — an auth *shape* (env-var name, oauth) is
//! metadata; the credential itself only ever enters the vault with explicit
//! user authorization.
//!
//! Pure + testable: this module aggregates already-resolved records + counts;
//! the live per-class collectors (installed MCP dirs, agent bundles, local
//! models) are supplied by the caller (the Tauri layer) and fed in as
//! [`ResourceCard`]s. Providers are derived here directly from the registry.

use serde::{Deserialize, Serialize};

use crate::provider::{DiscoverySource, ProviderRegistry};

/// The resource classes the Discover surface enumerates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Agent,
    Model,
    Provider,
    Mcp,
    Skill,
    Browser,
}

impl ResourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResourceKind::Agent => "agent",
            ResourceKind::Model => "model",
            ResourceKind::Provider => "provider",
            ResourceKind::Mcp => "mcp",
            ResourceKind::Skill => "skill",
            ResourceKind::Browser => "browser",
        }
    }
}

/// The ManagedResource lifecycle (discovery ≠ install ≠ activation). Ordered
/// so `>=` comparisons express "at least this far along".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedResource {
    /// Found + described (default from a bare discovery pass).
    #[default]
    Discovered,
    /// Passed validation (schema/signature/allow-list).
    Validated,
    /// Bits are on disk.
    Installed,
    /// Capabilities inventoried (advertised — not yet verified).
    Inventoried,
    /// User enabled it (opt-in; still not running).
    Enabled,
    /// Process/session started.
    Started,
    /// Health check passed.
    Healthy,
    /// Actively used by a run.
    InUse,
    /// A version update is in progress.
    Updating,
    /// Rolling back a failed update.
    RollingBack,
    /// Removed/uninstalled.
    Removed,
}

impl ManagedResource {
    pub fn as_str(&self) -> &'static str {
        match self {
            ManagedResource::Discovered => "discovered",
            ManagedResource::Validated => "validated",
            ManagedResource::Installed => "installed",
            ManagedResource::Inventoried => "inventoried",
            ManagedResource::Enabled => "enabled",
            ManagedResource::Started => "started",
            ManagedResource::Healthy => "healthy",
            ManagedResource::InUse => "in_use",
            ManagedResource::Updating => "updating",
            ManagedResource::RollingBack => "rolling_back",
            ManagedResource::Removed => "removed",
        }
    }

    /// Is the resource at least installed (available to enable/use)?
    pub fn is_installed(&self) -> bool {
        matches!(
            self,
            ManagedResource::Installed
                | ManagedResource::Inventoried
                | ManagedResource::Enabled
                | ManagedResource::Started
                | ManagedResource::Healthy
                | ManagedResource::InUse
                | ManagedResource::Updating
                | ManagedResource::RollingBack
        )
    }

    /// Is it enabled (user opted in)?
    pub fn is_enabled(&self) -> bool {
        matches!(
            self,
            ManagedResource::Enabled
                | ManagedResource::Started
                | ManagedResource::Healthy
                | ManagedResource::InUse
        )
    }
}

/// One per-resource card for the Discover surface. Auth is a *shape*, never a
/// secret; `capabilities` are advertised unless `capabilities_verified` is set
/// (P44.4 probe write-back).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceCard {
    pub kind: ResourceKind,
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// Provenance (catalog / plugin / user config / local runtime …).
    #[serde(default)]
    pub source: String,
    /// Auth *shape* only (e.g. `api_key_env:OPENAI_API_KEY`, `oauth`, `none`).
    /// NEVER a credential value.
    #[serde(default)]
    pub auth: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Whether `capabilities` are P44.4-verified (else advertised only).
    #[serde(default)]
    pub capabilities_verified: bool,
    /// Governance badge (e.g. `mediated` / `self_contained` / `not_governed`
    /// for agents; `inbuilt` for first-party).
    #[serde(default)]
    pub governance: String,
    /// Explicit endpoint when the catalog knows one (else empty — the SDK
    /// default or a user override applies). Never a secret.
    #[serde(default)]
    pub base_url: String,
    /// models.dev provider page — the full model list, pricing, and the
    /// provider's own docs link. Derived from the id, never fetched.
    #[serde(default)]
    pub doc_url: String,
    pub status: ManagedResource,
}

impl ResourceCard {
    /// Build a provider card from a registry record — auth is reported as a
    /// shape string, never a value; capabilities are marked verified only when
    /// a P44.4 report confirmed the hard caps.
    pub fn from_provider(rec: &crate::provider::ProviderRecord) -> Self {
        let auth = match &rec.auth {
            crate::provider::Auth::ApiKey => "api_key".to_string(),
            crate::provider::Auth::ApiKeyEnv => rec
                .api_key_env
                .first()
                .map(|e| format!("api_key_env:{e}"))
                .unwrap_or_else(|| "api_key_env".to_string()),
            crate::provider::Auth::OAuthDeviceCode => "oauth_device_code".to_string(),
            crate::provider::Auth::OAuthExternal => "oauth_external".to_string(),
            crate::provider::Auth::ExternalProcess => "external_process".to_string(),
            crate::provider::Auth::AwsSdk => "aws_sdk".to_string(),
            crate::provider::Auth::Vertex => "vertex".to_string(),
            crate::provider::Auth::Keyless => "keyless".to_string(),
        };
        let verified = rec
            .verified_report
            .as_ref()
            .map(|r| r.hard_caps_verified)
            .unwrap_or(false);
        ResourceCard {
            kind: ResourceKind::Provider,
            id: rec.id.clone(),
            name: if rec.name.is_empty() {
                rec.id.clone()
            } else {
                rec.name.clone()
            },
            version: rec.source_version.clone(),
            source: source_label(rec.source),
            auth,
            capabilities: rec.capabilities.clone(),
            capabilities_verified: verified,
            governance: String::new(),
            base_url: rec.base_url.clone().unwrap_or_default(),
            doc_url: format!("https://models.dev/providers/{}", rec.id),
            status: if verified {
                ManagedResource::Healthy
            } else {
                ManagedResource::Inventoried
            },
        }
    }
}

fn source_label(s: DiscoverySource) -> String {
    match s {
        DiscoverySource::ModelsDev => "models.dev",
        DiscoverySource::Overlay => "overlay",
        DiscoverySource::UserConfig => "user_config",
        DiscoverySource::PluginProfile => "plugin",
    }
    .to_string()
}

/// The full Discover inventory: counts per class + the cards. Providers are
/// derived from the registry; the other classes are supplied by the caller
/// (their live collectors live in the shell — installed MCP dirs, agent
/// bundles, local models, browsers).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryInventory {
    pub cards: Vec<ResourceCard>,
    /// A monotonically increasing generation stamp — bumped whenever the
    /// inventory is rebuilt so the router can invalidate its cache (P44.8).
    pub generation: u64,
}

impl DiscoveryInventory {
    /// Build the provider slice of the inventory from a registry (pure).
    pub fn from_registry(reg: &ProviderRegistry, generation: u64) -> Self {
        let cards = reg.all().map(ResourceCard::from_provider).collect();
        Self { cards, generation }
    }

    /// Add caller-collected cards for the non-provider classes.
    pub fn extend(&mut self, cards: impl IntoIterator<Item = ResourceCard>) {
        self.cards.extend(cards);
    }

    /// Count cards per resource class (the Discover header counters).
    pub fn counts(&self) -> ResourceCounts {
        let mut c = ResourceCounts::default();
        for card in &self.cards {
            match card.kind {
                ResourceKind::Agent => c.agents += 1,
                ResourceKind::Model => c.models += 1,
                ResourceKind::Provider => c.providers += 1,
                ResourceKind::Mcp => c.mcp += 1,
                ResourceKind::Skill => c.skills += 1,
                ResourceKind::Browser => c.browsers += 1,
            }
        }
        c
    }

    /// Cards of one kind.
    pub fn of_kind(&self, kind: ResourceKind) -> Vec<&ResourceCard> {
        self.cards.iter().filter(|c| c.kind == kind).collect()
    }
}

/// The per-class counters the Discover header shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceCounts {
    pub agents: usize,
    pub models: usize,
    pub providers: usize,
    pub mcp: usize,
    pub skills: usize,
    pub browsers: usize,
}

impl ResourceCounts {
    pub fn total(&self) -> usize {
        self.agents + self.models + self.providers + self.mcp + self.skills + self.browsers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::base_registry;

    #[test]
    fn lifecycle_ordering_and_predicates() {
        assert!(ManagedResource::Discovered < ManagedResource::Installed);
        assert!(ManagedResource::Enabled < ManagedResource::Healthy);
        assert!(!ManagedResource::Discovered.is_installed());
        assert!(ManagedResource::Installed.is_installed());
        assert!(!ManagedResource::Installed.is_enabled());
        assert!(ManagedResource::Enabled.is_enabled());
        assert!(ManagedResource::default() == ManagedResource::Discovered);
    }

    #[test]
    fn provider_cards_never_contain_a_secret_value() {
        let reg = base_registry();
        let inv = DiscoveryInventory::from_registry(&reg, 1);
        assert!(!inv.cards.is_empty());
        for card in &inv.cards {
            assert_eq!(card.kind, ResourceKind::Provider);
            // Auth is a shape — env-var NAME or oauth/keyless/etc., never a value.
            assert!(
                card.auth.starts_with("api_key")
                    || card.auth.starts_with("oauth")
                    || card.auth == "external_process"
                    || card.auth == "aws_sdk"
                    || card.auth == "vertex"
                    || card.auth == "keyless",
                "auth must be a shape, got: {}",
                card.auth
            );
            // No card field may look like a live secret token.
            assert!(!card.auth.contains("sk-"));
            // Detail-panel fields: models.dev page derives from the id, the
            // endpoint mirrors the registry (empty = SDK default).
            assert_eq!(
                card.doc_url,
                format!("https://models.dev/providers/{}", card.id)
            );
        }
    }

    #[test]
    fn counts_aggregate_across_kinds() {
        let reg = base_registry();
        let mut inv = DiscoveryInventory::from_registry(&reg, 2);
        let providers = inv.counts().providers;
        assert!(providers > 0);
        inv.extend([
            ResourceCard {
                kind: ResourceKind::Mcp,
                id: "filesystem".into(),
                name: "filesystem".into(),
                version: "1.0".into(),
                source: "installed".into(),
                auth: "none".into(),
                capabilities: vec!["read".into()],
                capabilities_verified: false,
                governance: String::new(),
                base_url: String::new(),
                doc_url: String::new(),
                status: ManagedResource::Enabled,
            },
            ResourceCard {
                kind: ResourceKind::Skill,
                id: "q3-digest".into(),
                name: "q3 digest".into(),
                version: "0.2".into(),
                source: "store".into(),
                auth: "none".into(),
                capabilities: vec![],
                capabilities_verified: false,
                governance: String::new(),
                base_url: String::new(),
                doc_url: String::new(),
                status: ManagedResource::Installed,
            },
        ]);
        let counts = inv.counts();
        assert_eq!(counts.providers, providers);
        assert_eq!(counts.mcp, 1);
        assert_eq!(counts.skills, 1);
        assert_eq!(counts.total(), providers + 2);
        assert_eq!(inv.of_kind(ResourceKind::Mcp).len(), 1);
    }

    #[test]
    fn unverified_provider_card_is_inventoried_not_healthy() {
        // base_registry providers have no probe report → advertised only.
        let reg = base_registry();
        let inv = DiscoveryInventory::from_registry(&reg, 3);
        assert!(
            inv.cards
                .iter()
                .all(|c| !c.capabilities_verified && c.status == ManagedResource::Inventoried)
        );
    }
}
