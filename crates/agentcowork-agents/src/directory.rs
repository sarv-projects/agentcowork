//! The canonical agent directory (P69.D1).
//!
//! Before this module the same question — "which agents exist and what are
//! they?" — had four answers: the ACP launch registry
//! (`agentcowork-acp::registry`), the bundled registry index
//! (`agentcowork-acp::registry_index`), the local `agent.toml` bundle store
//! ([`crate::registry::AgentRegistry`]) and the TypeScript/UI agent maps.
//!
//! `AgentDirectory` is the single composition point. It does not *own* those
//! sources — the shell feeds it rows as they are discovered — but it owns the
//! canonical record ([`agentcowork_types::AgentDefinition`]), the provenance of
//! each row, and the derived view the UI renders. Every other surface (ACP
//! registry rows, bundle store, picker) is therefore a projection, not a
//! parallel truth.

use std::collections::BTreeMap;

use agentcowork_types::{AgentDefinition, AgentReadiness};

use crate::bundle::AgentBundle;

/// Where a directory entry came from. Provenance is a first-class field: an
/// entry discovered from the ACP registry is not equivalent to one the user
/// authored locally, and the UI must be able to say which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentSource {
    /// A curated ACP registry row (`registry.json`).
    AcpRegistry,
    /// A runtime discovered on this machine (installed CLI/server).
    Discovered,
    /// A user-authored `agent.toml` bundle.
    LocalBundle,
    /// An external MCP server's agent surface.
    Mcp,
}

impl AgentSource {
    /// Stable wire spelling (the UI reads this verbatim).
    pub fn as_str(self) -> &'static str {
        match self {
            AgentSource::AcpRegistry => "acp_registry",
            AgentSource::Discovered => "discovered",
            AgentSource::LocalBundle => "local_bundle",
            AgentSource::Mcp => "mcp",
        }
    }

    /// Whether an entry from this source can go away again (curated catalog
    /// rows cannot be uninstalled).
    pub fn is_removable(self) -> bool {
        matches!(self, AgentSource::Discovered | AgentSource::LocalBundle)
    }

    /// The readiness an entry of this provenance starts at — a *floor*, not a
    /// claim: the host replaces it with the probed fact. A local bundle's
    /// runtime is assumed present, a discovered CLI/server is present, and a
    /// catalog row without a runtime is only discovered. No source starts at
    /// `Ready` any more: nothing ships with the app (ADR-0005).
    pub fn default_readiness(self) -> AgentReadiness {
        match self {
            AgentSource::LocalBundle | AgentSource::Discovered => AgentReadiness::Installed,
            AgentSource::AcpRegistry | AgentSource::Mcp => AgentReadiness::Discovered,
        }
    }
}

/// One directory entry: the canonical definition plus its provenance and the
/// facts the picker needs but the definition itself should not carry.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentDirectoryEntry {
    pub definition: AgentDefinition,
    pub source: AgentSource,
    /// The one readiness state (P71.3f — `agentcowork_types::AgentReadiness`).
    /// Replaces the `installed` boolean: an entry whose runtime is present but
    /// which is not authenticated, not protocol-compatible, or not usable in
    /// this environment says so instead of collapsing to one bit.
    pub readiness: AgentReadiness,
    /// Install/launch hint (registry id, executable, or bundle path). Opaque
    /// to the kernel; shown to the user when they ask "why can't I run this?".
    pub locator: Option<String>,
}

impl AgentDirectoryEntry {
    pub fn new(definition: AgentDefinition, source: AgentSource) -> Self {
        Self {
            definition,
            source,
            readiness: source.default_readiness(),
            locator: None,
        }
    }

    /// Replace the readiness state with a fact the host probed.
    pub fn with_readiness(mut self, readiness: AgentReadiness) -> Self {
        self.readiness = readiness;
        self
    }

    /// Whether a runtime is present — **derived** from the one state, never a
    /// second field to keep in sync.
    pub fn installed(&self) -> bool {
        self.readiness.is_installed()
    }

    /// Whether the agent may serve a turn right now.
    pub fn ready(&self) -> bool {
        self.readiness.is_ready()
    }

    pub fn with_locator(mut self, locator: impl Into<String>) -> Self {
        self.locator = Some(locator.into());
        self
    }
}

/// The one agent directory.
///
/// Ordered by insertion-independent id so two builds of the same inputs
/// produce byte-identical output (the picker must not reshuffle).
#[derive(Debug, Default, Clone)]
pub struct AgentDirectory {
    entries: BTreeMap<String, AgentDirectoryEntry>,
}

impl AgentDirectory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace an entry, keyed by the canonical agent id.
    pub fn upsert(&mut self, entry: AgentDirectoryEntry) -> Option<AgentDirectoryEntry> {
        let key = entry.definition.id.as_str().to_string();
        self.entries.insert(key.clone(), entry);
        // The stored row, not the replaced one: `None` means the entry was
        // rejected upstream, never "this was a fresh key" — a caller cannot
        // tell those apart from the old value.
        self.entries.get(&key).cloned()
    }

    /// Insert a definition with default facts for its source.
    pub fn insert(
        &mut self,
        definition: AgentDefinition,
        source: AgentSource,
    ) -> Option<AgentDirectoryEntry> {
        self.upsert(AgentDirectoryEntry::new(definition, source))
    }

    pub fn get(&self, id: &str) -> Option<&AgentDirectoryEntry> {
        self.entries.get(id)
    }

    pub fn remove(&mut self, id: &str) -> Option<AgentDirectoryEntry> {
        self.entries.remove(id)
    }

    /// All entries, id-ordered.
    pub fn list(&self) -> Vec<&AgentDirectoryEntry> {
        self.entries.values().collect()
    }

    /// Entries from one source, id-ordered.
    pub fn by_source(&self, source: AgentSource) -> Vec<&AgentDirectoryEntry> {
        self.entries
            .values()
            .filter(|e| e.source == source)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add a user bundle's definition under `LocalBundle` provenance. An
    /// unbound bundle (no engine chosen) is a **draft**, not an agent —
    /// nothing is inserted and `None` is returned (ADR-0005: no row may
    /// present a brain that does not exist).
    pub fn upsert_bundle(&mut self, bundle: &AgentBundle) -> Option<AgentDirectoryEntry> {
        let definition = bundle.definition()?;
        self.insert(definition, AgentSource::LocalBundle)
    }

    /// Add every bundle in a registry, returning how many entries exist after
    /// the merge (the bundle store is a source, never a second directory).
    pub fn upsert_bundles(&mut self, bundles: &[AgentBundle]) -> usize {
        for bundle in bundles {
            self.upsert_bundle(bundle);
        }
        self.entries.len()
    }

    /// The picker's default: the first **ready** entry, else nothing — never
    /// an arbitrary row, never an assumed built-in. Selection is user-owned
    /// (the shell returns `defaultAgentId: null`); this only answers "what
    /// could run right now" when a surface must preselect something.
    pub fn default_entry(&self) -> Option<&AgentDirectoryEntry> {
        self.entries.values().find(|e| e.ready())
    }

    /// The picker's selectable rows: readiness `Ready` or `Degraded`. A row
    /// that is only installed/launchable stays visible with its state ("why
    /// can't I run this?") but is not offered as a choice.
    pub fn selectable(&self) -> Vec<&AgentDirectoryEntry> {
        self.entries.values().filter(|e| e.ready()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentcowork_types::{AgentId, AgentProtocol, AuthMode};

    fn def(id: &str) -> AgentDefinition {
        AgentDefinition {
            id: AgentId::new(id),
            name: id.to_string(),
            description: String::new(),
            protocol: AgentProtocol::Acp,
            auth_mode: AuthMode::Subscription,
            capabilities: Vec::new(),
            extension_mechanisms: Vec::new(),
        }
    }

    #[test]
    fn list_is_id_ordered_and_default_is_the_first_ready_row() {
        let mut dir = AgentDirectory::new();
        dir.insert(def("zeta"), AgentSource::AcpRegistry);
        dir.insert(def("alpha"), AgentSource::Discovered);
        dir.upsert(
            AgentDirectoryEntry::new(def("beta"), AgentSource::Discovered)
                .with_readiness(AgentReadiness::Ready),
        );

        let ids: Vec<&str> = dir
            .list()
            .iter()
            .map(|e| e.definition.id.as_str())
            .collect();
        assert_eq!(ids, vec!["alpha", "beta", "zeta"]);
        // Only `beta` is ready — nothing is assumed to be the default.
        assert_eq!(dir.default_entry().unwrap().definition.id.as_str(), "beta");
    }

    #[test]
    fn unbound_bundles_never_join_the_directory() {
        // ADR-0005: a draft bundle (no engine bound) is not an agent.
        let mut dir = AgentDirectory::new();
        let draft = AgentBundle::new("Draft");
        assert!(dir.upsert_bundle(&draft).is_none());
        assert!(dir.is_empty());

        let mut bound = AgentBundle::new("Bound");
        bound.engine = Some(crate::bundle::EngineBinding::Acp("claude-code".into()));
        assert!(dir.upsert_bundle(&bound).is_some());
        assert_eq!(dir.len(), 1);
    }

    #[test]
    fn discovery_never_claims_installed_and_removal_is_source_scoped() {
        let mut dir = AgentDirectory::new();
        dir.insert(def("cursor"), AgentSource::AcpRegistry);
        assert!(!dir.get("cursor").unwrap().installed());
        assert_eq!(
            dir.get("cursor").unwrap().readiness,
            AgentReadiness::Discovered
        );
        assert!(!AgentSource::AcpRegistry.is_removable());

        dir.upsert(
            AgentDirectoryEntry::new(def("local"), AgentSource::LocalBundle)
                .with_readiness(AgentReadiness::Installed)
                .with_locator("/tmp/agent.toml"),
        );
        assert!(dir.get("local").unwrap().installed());
        // Installed is not ready: the picker must not offer it.
        assert!(!dir.get("local").unwrap().ready());
        assert!(AgentSource::LocalBundle.is_removable());
    }

    #[test]
    fn selectable_rows_are_the_ready_ones_only() {
        let mut dir = AgentDirectory::new();
        dir.upsert(
            AgentDirectoryEntry::new(def("claude"), AgentSource::AcpRegistry)
                .with_readiness(AgentReadiness::AuthRequired),
        );
        dir.upsert(
            AgentDirectoryEntry::new(def("codex"), AgentSource::Discovered)
                .with_readiness(AgentReadiness::Degraded),
        );
        let ids: Vec<&str> = dir
            .selectable()
            .iter()
            .map(|e| e.definition.id.as_str())
            .collect();
        // `claude` is installed but auth-required → visible, not selectable.
        assert_eq!(ids, vec!["codex"]);
    }
}
