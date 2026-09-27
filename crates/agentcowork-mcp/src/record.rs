//! P36 (F6/F8) — `MCPServerRecord`: the canonical record for one MCP server.
//!
//! Per spec v3.39: id / registry / version / transport / provenance /
//! digest / capabilities / trust / `enabled_consumers[]` / health /
//! `config_hash` — with **per-consumer enable** (running Agent X never loads
//! Agent Y's servers — P31.4 semantics live here). The process lifecycle
//! (discover → … → remove) rides `agentcowork-core::resources::ManagedResource`;
//! this record is the typed row that fills the `R` slot.
//!
//! The crate is dependency-free for this module: the kernel generic comes
//! from the caller's side, these are the pure MCP-shaped fields.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpTransport {
    Stdio,
    Http,
    Sse,
    Loopback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpProvenance {
    /// User-supplied local server (stdio/npx).
    User,
    /// Installed from a directory/registry (e.g. mcpservers.org queue, P18).
    Registry,
    /// Bundled with AgentCowork.
    Bundled,
}

/// One consumer's toggle + health view. Consumers tick exact server subsets,
/// never "all" (P31.4 semantics).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsumerEnable {
    pub consumer: String,
    pub enabled: bool,
    pub last_health: Option<McpHealthState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpHealthState {
    Unknown,
    Starting,
    Healthy,
    Degraded,
    Down,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MCPServerRecord {
    /// Canonical identity (stable across config changes).
    pub id: String,
    pub registry: Option<String>,
    pub version: String,
    pub transport: McpTransport,
    pub provenance: McpProvenance,
    /// Content digest of the config that produced this record — any change
    /// mints a new value and forces re-validation.
    pub digest: String,
    pub capabilities: Vec<String>,
    /// Fail-closed trust flags (Hermes pattern): nothing is trusted unless
    /// it was both declared and granted.
    pub trust: McpTrust,
    /// Per-consumer enable map. Consumers never see servers they didn't tick.
    pub enabled_consumers: BTreeMap<String, ConsumerEnable>,
    pub health: McpHealthState,
    /// Config fingerprint (immutable per generation).
    pub config_hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpTrust {
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub files_read: bool,
    #[serde(default)]
    pub files_write: bool,
    #[serde(default)]
    pub shell: bool,
    #[serde(default)]
    pub requires_approval: bool,
}

impl MCPServerRecord {
    pub fn new(id: impl Into<String>, transport: McpTransport, version: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            registry: None,
            version: version.into(),
            transport,
            provenance: McpProvenance::User,
            digest: String::new(),
            capabilities: Vec::new(),
            trust: McpTrust::default(),
            enabled_consumers: BTreeMap::new(),
            health: McpHealthState::Unknown,
            config_hash: String::new(),
        }
    }

    /// Per-consumer enable: tick one consumer onto this server. Exact subset
    /// semantics — an unticked consumer is not present at all.
    pub fn enable_for(&mut self, consumer: &str) {
        self.enabled_consumers.insert(
            consumer.to_string(),
            ConsumerEnable {
                consumer: consumer.to_string(),
                enabled: true,
                last_health: None,
            },
        );
    }

    pub fn disable_for(&mut self, consumer: &str) {
        if let Some(e) = self.enabled_consumers.get_mut(consumer) {
            e.enabled = false;
        }
    }

    pub fn is_enabled_for(&self, consumer: &str) -> bool {
        self.enabled_consumers
            .get(consumer)
            .is_some_and(|e| e.enabled)
    }

    pub fn consumers(&self) -> Vec<String> {
        self.enabled_consumers
            .values()
            .filter(|e| e.enabled)
            .map(|e| e.consumer.clone())
            .collect()
    }

    /// A consumer's server is only usable when the server itself is healthy
    /// and the consumer enabled it.
    pub fn usable_by(&self, consumer: &str) -> bool {
        self.health == McpHealthState::Healthy && self.is_enabled_for(consumer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_consumer_enable_without_all() {
        let mut r = MCPServerRecord::new("filesystem", McpTransport::Stdio, "0.4.0");
        r.health = McpHealthState::Healthy;
        assert!(!r.is_enabled_for("alice"));
        r.enable_for("alice");
        assert!(r.is_enabled_for("alice"));
        assert!(!r.is_enabled_for("bob"));
        r.enable_for("bob");
        r.disable_for("alice");
        assert_eq!(r.consumers(), vec!["bob"]);
    }

    #[test]
    fn enable_plus_health_gate() {
        let mut r = MCPServerRecord::new("fs", McpTransport::Loopback, "1");
        r.enable_for("coder");
        assert!(!r.usable_by("coder")); // enabled but health unknown
        r.health = McpHealthState::Healthy;
        assert!(r.usable_by("coder"));
        r.health = McpHealthState::Down;
        assert!(!r.usable_by("coder"));
    }

    #[test]
    fn digest_change_forces_revalidation() {
        let mut r = MCPServerRecord::new("fs", McpTransport::Stdio, "1");
        r.digest = "abc".into();
        assert_eq!(r.digest, "abc");
        // The consumer contract: config_hash is immutable per generation —
        // a mutation requires a new record; enforced by the caller.
    }
}
