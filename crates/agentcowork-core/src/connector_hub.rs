//! Connector Hub (P6.6 — F1–F5, doc 13).
//!
//! The unified connection layer's *routing spine*: one hub, four engines, one
//! registry, one permission system. This is the deterministic routing logic —
//! it never holds credentials (OAuth tokens live in `agentcowork-vault`; the
//! actual connector adapters are the TS core-connectors + the Rust MCP/ACP
//! bridges). The two invariants this module enforces:
//!
//! 1. **One `(provider, account)` → one engine** (no double-connect).
//! 2. **External-write / destructive connectors are permission-classed** so
//!    the Guard-2 card + trust-ladder gate is mechanical, not advisory.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The four engines the hub routes to (doc 13 §1). The aggregator chain
/// (Composio → Zapier → Nango) was removed per the Connector-platform
/// decision (2026-08-16) — MCP is the platform, no cloud SaaS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// First-party native adapter (zero-auth or BYO OAuth/API-key, tokens in
    /// the SQLCipher vault).
    Native,
    /// A user-supplied or official MCP server (stdio/npx or user-hosted HTTP).
    Mcp,
    /// Local Auth Bridge (F4) — PKCE OAuth with vault-stored tokens.
    AuthBridge,
    /// Browser-session connector (F3) — drive the web app via CDP + vault.
    BrowserSession,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Native => "native",
            Engine::Mcp => "mcp",
            Engine::AuthBridge => "auth_bridge",
            Engine::BrowserSession => "browser_session",
        }
    }
}

/// Permission class (doc 13 §2) — every connector tool carries one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionClass {
    ReadOnly,
    LocalWrite,
    ExternalWrite,
    Destructive,
}

impl PermissionClass {
    /// Does this class require a Guard-2 confirmation card?
    pub fn requires_card(self) -> bool {
        matches!(
            self,
            PermissionClass::ExternalWrite | PermissionClass::Destructive
        )
    }
}

/// One connected account (doc 13 §2 `Connection`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub provider: String,
    pub account: String,
    pub engine: Engine,
    pub engine_ref: String,
    pub state: ConnectionState,
    pub permission_class: PermissionClass,
    pub usage: UsageMeter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error,
    Dead,
}

/// Per-connector usage metering (doc 13 §2 `usage`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageMeter {
    pub calls: u64,
    pub budget: Option<u64>,
}

impl UsageMeter {
    pub fn remaining(&self) -> Option<u64> {
        self.budget.map(|b| b.saturating_sub(self.calls))
    }
    /// Is this connector over its budget (or exactly at it)?
    pub fn exhausted(&self) -> bool {
        self.budget.map(|b| self.calls >= b).unwrap_or(false)
    }
    pub fn record_call(&mut self) {
        self.calls = self.calls.saturating_add(1);
    }
}

/// The hub: routing preference + the no-double-connect registry.
#[derive(Debug, Clone, Default)]
pub struct ConnectorHub {
    /// `(provider, account)` → connection id (the dedupe index).
    index: BTreeMap<(String, String), String>,
    /// connection id → connection.
    connections: BTreeMap<String, Connection>,
    /// Engine routing preference (default: native → auth_bridge → mcp →
    /// browser_session; overridable per provider).
    preference: Vec<Engine>,
    /// Per-provider engine override (Settings).
    provider_preference: BTreeMap<String, Engine>,
}

impl ConnectorHub {
    pub fn new() -> Self {
        Self {
            index: BTreeMap::new(),
            connections: BTreeMap::new(),
            preference: vec![
                Engine::Native,
                Engine::AuthBridge,
                Engine::Mcp,
                Engine::BrowserSession,
            ],
            provider_preference: BTreeMap::new(),
        }
    }

    /// Set a per-provider engine override.
    pub fn prefer(&mut self, provider: &str, engine: Engine) {
        self.provider_preference
            .insert(provider.to_string(), engine);
    }

    /// Resolve the engine for a provider (override → global preference).
    pub fn engine_for(&self, provider: &str) -> Engine {
        if let Some(e) = self.provider_preference.get(provider) {
            return *e;
        }
        *self.preference.first().unwrap_or(&Engine::Native)
    }

    /// Register a connection. **Returns `Err` if `(provider, account)` is
    /// already connected** — the no-double-connect guarantee (doc 13 §6).
    pub fn connect(
        &mut self,
        provider: &str,
        account: &str,
        engine: Engine,
    ) -> Result<String, HubError> {
        let key = (provider.to_string(), account.to_string());
        if self.index.contains_key(&key) {
            return Err(HubError::AlreadyConnected(
                provider.to_string(),
                account.to_string(),
            ));
        }
        let id = format!("{provider}:{account}");
        let conn = Connection {
            provider: provider.to_string(),
            account: account.to_string(),
            engine,
            engine_ref: id.clone(),
            state: ConnectionState::Connecting,
            permission_class: PermissionClass::ReadOnly,
            usage: UsageMeter::default(),
        };
        self.index.insert(key, id.clone());
        self.connections.insert(id.clone(), conn);
        Ok(id)
    }

    /// Is `(provider, account)` already connected?
    pub fn is_connected(&self, provider: &str, account: &str) -> bool {
        self.index
            .contains_key(&(provider.to_string(), account.to_string()))
    }

    pub fn get(&self, id: &str) -> Option<&Connection> {
        self.connections.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Connection> {
        self.connections.get_mut(id)
    }

    /// Transition a connection's state + set its permission class.
    pub fn set_state(&mut self, id: &str, state: ConnectionState) -> Result<(), HubError> {
        match self.connections.get_mut(id) {
            Some(c) => {
                c.state = state;
                Ok(())
            }
            None => Err(HubError::NotFound(id.to_string())),
        }
    }

    /// Set a connection's permission class (an external-write/destructive
    /// connector is gated at runtime).
    pub fn set_permission(&mut self, id: &str, class: PermissionClass) -> Result<(), HubError> {
        match self.connections.get_mut(id) {
            Some(c) => {
                c.permission_class = class;
                Ok(())
            }
            None => Err(HubError::NotFound(id.to_string())),
        }
    }

    /// Record one metered call (returns `false` if the budget is exhausted —
    /// the caller must refuse).
    pub fn meter(&mut self, id: &str) -> Result<bool, HubError> {
        match self.connections.get_mut(id) {
            Some(c) => {
                if c.usage.exhausted() {
                    return Ok(false);
                }
                c.usage.record_call();
                Ok(true)
            }
            None => Err(HubError::NotFound(id.to_string())),
        }
    }

    pub fn list(&self) -> Vec<&Connection> {
        let mut v: Vec<&Connection> = self.connections.values().collect();
        v.sort_by(|a, b| {
            (a.provider.as_str(), a.account.as_str())
                .cmp(&(b.provider.as_str(), b.account.as_str()))
        });
        v
    }

    pub fn len(&self) -> usize {
        self.connections.len()
    }

    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum HubError {
    #[error("provider '{0}' account '{1}' is already connected (no double-connect)")]
    AlreadyConnected(String, String),
    #[error("connection '{0}' not found")]
    NotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_double_connect_rejects_second_register() {
        let mut hub = ConnectorHub::new();
        let id = hub
            .connect("gmail", "me@gmail.com", Engine::AuthBridge)
            .unwrap();
        assert!(hub.is_connected("gmail", "me@gmail.com"));
        // Same (provider, account) again → error.
        assert_eq!(
            hub.connect("gmail", "me@gmail.com", Engine::Mcp),
            Err(HubError::AlreadyConnected(
                "gmail".into(),
                "me@gmail.com".into()
            ))
        );
        // A different account is fine.
        assert!(hub.connect("gmail", "other@gmail.com", Engine::Mcp).is_ok());
        assert_eq!(hub.len(), 2);
        assert_eq!(id, "gmail:me@gmail.com");
    }

    #[test]
    fn engine_preference_native_first_with_override() {
        let hub = ConnectorHub::new();
        assert_eq!(hub.engine_for("github"), Engine::Native);
        let mut hub = ConnectorHub::new();
        hub.prefer("github", Engine::Mcp);
        assert_eq!(hub.engine_for("github"), Engine::Mcp);
        assert_eq!(hub.engine_for("notion"), Engine::Native);
    }

    #[test]
    fn permission_classes_gate_external_writes() {
        assert!(!PermissionClass::ReadOnly.requires_card());
        assert!(!PermissionClass::LocalWrite.requires_card());
        assert!(PermissionClass::ExternalWrite.requires_card());
        assert!(PermissionClass::Destructive.requires_card());
    }

    #[test]
    fn usage_metering_exhausts_budget() {
        let mut hub = ConnectorHub::new();
        let id = hub.connect("slack", "team", Engine::Mcp).unwrap();
        hub.get_mut(&id).unwrap().usage.budget = Some(2);
        assert!(hub.meter(&id).unwrap());
        assert!(hub.meter(&id).unwrap());
        // Third call is refused (budget exhausted).
        assert!(!hub.meter(&id).unwrap());
        assert_eq!(hub.get(&id).unwrap().usage.calls, 2);
    }

    #[test]
    fn state_and_permission_transitions() {
        let mut hub = ConnectorHub::new();
        let id = hub.connect("notion", "me", Engine::BrowserSession).unwrap();
        hub.set_state(&id, ConnectionState::Connected).unwrap();
        hub.set_permission(&id, PermissionClass::ExternalWrite)
            .unwrap();
        let c = hub.get(&id).unwrap();
        assert_eq!(c.state, ConnectionState::Connected);
        assert!(c.permission_class.requires_card());
    }

    #[test]
    fn unknown_connection_is_an_error() {
        let mut hub = ConnectorHub::new();
        assert_eq!(hub.meter("nope"), Err(HubError::NotFound("nope".into())));
    }

    /// FIX-01 / INV-02 — the hub is routing metadata, not custody. A
    /// `Connection` names the engine and the account; it holds no credential,
    /// and the whole registry serializes without one. This test pins that so a
    /// future field cannot quietly become a token holder.
    #[test]
    fn a_connection_record_carries_no_credential() {
        let mut hub = ConnectorHub::new();
        let id = hub
            .connect("gmail", "me@example.com", Engine::AuthBridge)
            .unwrap();
        hub.set_state(&id, ConnectionState::Connected).unwrap();
        let json = serde_json::to_string(&hub.list()).unwrap();
        for forbidden in [
            "access_token",
            "accessToken",
            "refresh_token",
            "refreshToken",
            "token",
            "secret",
            "password",
        ] {
            assert!(
                !json.to_lowercase().contains(&forbidden.to_lowercase()),
                "the hub record leaked a credential-shaped field: {json}"
            );
        }
        // What it *does* carry is the engine reference the executor resolves
        // through the vault — a reference, not bytes.
        let conn = hub.get(&id).unwrap();
        assert_eq!(conn.engine_ref, "gmail:me@example.com");
        assert_eq!(conn.engine, Engine::AuthBridge);
    }
}
