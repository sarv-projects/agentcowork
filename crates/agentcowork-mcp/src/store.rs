//! Connect Store — the curated "click → sign in → use" surface (F6/F7).
//!
//! The ChatGPT-connector equivalent for a local-first app: instead of a
//! settings form where the user pastes OAuth client IDs (the n8n self-host
//! wall), the store is a short curated index of **remote MCP servers** and
//! **flat connector providers** the app knows how to connect with one click.
//!
//! How this relates to the existing surfaces:
//! - `manager::RegistryServer` + `ALLOW_LIST` gate **local** stdio/npx/uvx
//!   installs. `store` covers the **remote** (HTTP/Sse) + OAuth half, which
//!   `install_plan` currently rejects (`UnsupportedType("remote")`).
//! - The vault's `agentcowork-vault::oauth` already implements PKCE-loopback
//!   and device-code flows behind an extensible provider map. `store` names
//!   the **connector** providers (GitHub/Google/Microsoft/Slack) and which
//!   flow each uses, so the shell can route them into that manager.
//! - Guard-2 (the always-on-top consent webview) is the approval gate: every
//!   entry carries the tool/scope list a `connect` must render before the
//!   OAuth flow proceeds. Compromised renderer ≠ fake consent.
//!
//! Everything here is pure data + small typed helpers — no HTTP, no OAuth
//! call. The actual authorization/token plumbing lives in the vault; the
//! runtime fetch/spawn seams are documented per entry.

use serde::{Deserialize, Serialize};

/// The OAuth flow the app runs to connect a store entry (drawn from the
/// vault's `agentcowork-vault::oauth` `FlowKind` — kept in sync).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectFlow {
    /// PKCE (S256) auth-code with a local loopback redirect. Best for
    /// Google/Microsoft-family and OAuth 2.1 MCP resource servers.
    Pkce,
    /// RFC 8628 device authorization — user sees a code, authorizes on any
    /// device. Best for GitHub (no redirect URI needed) and terminals.
    DeviceCode,
    /// No OAuth: an API key the user pastes (test-connection → save in vault).
    ApiKey,
}

/// What a `connect` must show to the user before it runs (the Guard-2
/// consent payload). Plain-language scopes, not opaque OAuth scope strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectConsent {
    /// A human sentence, e.g. "Read your GitHub repositories and issues."
    pub scopes_plain: Vec<String>,
    /// Mutations this connection can make on the user's behalf.
    #[serde(default)]
    pub can_mutate: bool,
    /// Whether the connected data is ingested into local memory/search.
    #[serde(default)]
    pub indexes_into_memory: bool,
}

/// One server provider kind in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StoreKind {
    /// A remote MCP server reachable over HTTP/SSE (OAuth 2.1 per the MCP
    /// authorization spec). Connectors for GitHub/Drive/Atlassian/etc.
    RemoteMcp,
    /// A flat connector backed by the vault OAuth manager (no MCP server).
    Connector,
}

/// One curated store entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreEntry {
    /// Stable id (used for the per-consumer enable + consent key).
    pub id: String,
    pub kind: StoreKind,
    /// Display name shown as the connector's label.
    pub name: String,
    pub description: String,
    /// For `RemoteMcp`: the HTTPS server URL (the OAuth 2.1 resource server).
    /// Empty/None for flat `Connector`s (provider routing lives in the vault).
    #[serde(default)]
    pub url: Option<String>,
    /// For `RemoteMcp`: the per-server **force-legacy** escape hatch
    /// (DEC-030 / ARCH/14 §4).
    ///
    /// Persisted on the record, not passed per call: it changes the *wire
    /// contract* this server is spoken in, so a runtime-only flag would be lost
    /// on restart and silently re-pin the origin to a wrong era. It is
    /// operator-only — there is no user-facing toggle in v1 — and it is the one
    /// lever for a server that misreports its era.
    #[serde(default)]
    pub force_legacy: bool,
    /// Which flow this entry uses.
    pub flow: ConnectFlow,
    /// Vault `provider` key (matches `agentcowork-vault::oauth` ProviderSettings).
    #[serde(default)]
    pub vault_provider: String,
    /// The consent payload Guard-2 must render before authorizing.
    pub consent: ConnectConsent,
    /// Tool count the connected server exposes (informational).
    #[serde(default)]
    pub tool_hint: u32,
}

/// The curated store, indexable by id. `entries()` returns them in display
/// order (high-demand first).
#[derive(Debug, Clone, Default)]
pub struct StoreIndex {
    by_id: std::collections::BTreeMap<String, StoreEntry>,
}

impl StoreIndex {
    /// The built-in curated set. Deliberately small and audit-trail-visible:
    /// each entry is a reviewed connector, not an open registry.
    pub fn bundled() -> Self {
        let mut s = Self::default();
        for e in bundled_entries() {
            s.by_id.insert(e.id.clone(), e);
        }
        s
    }

    pub fn with(add: impl IntoIterator<Item = StoreEntry>) -> Self {
        let mut s = Self::bundled();
        for e in add {
            s.by_id.insert(e.id.clone(), e);
        }
        s
    }

    pub fn get(&self, id: &str) -> Option<&StoreEntry> {
        self.by_id.get(id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// All entries in deterministic (display) order.
    pub fn entries(&self) -> Vec<&StoreEntry> {
        self.by_id.values().collect()
    }

    /// Search by name/description/id substring.
    pub fn search(&self, term: &str, limit: usize) -> Vec<&StoreEntry> {
        let t = term.to_lowercase();
        self.by_id
            .values()
            .filter(|e| {
                e.id.to_lowercase().contains(&t)
                    || e.name.to_lowercase().contains(&t)
                    || e.description.to_lowercase().contains(&t)
            })
            .take(limit)
            .collect()
    }

    /// Filter to a single kind (MCP remotes vs flat connectors).
    pub fn of_kind(&self, kind: StoreKind) -> Vec<&StoreEntry> {
        self.by_id.values().filter(|e| e.kind == kind).collect()
    }
}

/// The v1 curated set. Extreme care: every entry is (a) a well-known official
/// service, (b) reachable via a documented remote-MCP/HTTP or flat-OAuth
/// surface, and (c) scoped to the least privilege that is useful.
///
/// Remote-MCP OAuth 2.1 authorization is the 2026 standard path (the MCP
/// authorization spec, `<server>/.well-known/oauth-protected-resource` +
/// authorization-server discovery + PKCE). `url` is the server to discover.
fn bundled_entries() -> Vec<StoreEntry> {
    vec![
        StoreEntry {
            id: "github".into(),
            kind: StoreKind::RemoteMcp,
            name: "GitHub".into(),
            description: "Repositories, issues, PRs, code search, Actions state from the official GitHub MCP server.".into(),
            url: Some("https://api.githubcopilot.com/mcp/".into()),
            force_legacy: false,
            flow: ConnectFlow::DeviceCode,
            vault_provider: "copilot".into(),
            consent: ConnectConsent {
                scopes_plain: vec![
                    "Read your repositories, issues, and pull requests".into(),
                    "Run code searches against your repos".into(),
                ],
                can_mutate: true,
                indexes_into_memory: false,
            },
            tool_hint: 30,
        },
        StoreEntry {
            id: "google-drive".into(),
            kind: StoreKind::RemoteMcp,
            name: "Google Drive".into(),
            description: "Read and write files in your Google Drive via the official Drive connector.".into(),
            url: Some("https://mcp.googleapis.com/mcp/".into()),
            force_legacy: false,
            flow: ConnectFlow::Pkce,
            vault_provider: "google".into(),
            consent: ConnectConsent {
                scopes_plain: vec!["View your Google Drive file list and metadata".into()],
                can_mutate: true,
                indexes_into_memory: false,
            },
            tool_hint: 12,
        },
        StoreEntry {
            id: "microsoft-graph".into(),
            kind: StoreKind::RemoteMcp,
            name: "Microsoft Graph".into(),
            description: "Outlook mail, OneDrive, and Calendar via the Microsoft Graph connector.".into(),
            url: Some("https://mcp.microsoft.com/mcp/".into()),
            force_legacy: false,
            flow: ConnectFlow::Pkce,
            vault_provider: "microsoft".into(),
            consent: ConnectConsent {
                scopes_plain: vec![
                    "Read your Outlook mail headers".into(),
                    "Read your OneDrive file list".into(),
                    "Read your calendar events".into(),
                ],
                can_mutate: true,
                indexes_into_memory: false,
            },
            tool_hint: 20,
        },
        StoreEntry {
            id: "notion".into(),
            kind: StoreKind::RemoteMcp,
            name: "Notion".into(),
            description: "Notion pages, databases, and search from the official Notion connector.".into(),
            url: Some("https://mcp.notion.com/mcp".into()),
            force_legacy: false,
            flow: ConnectFlow::Pkce,
            vault_provider: "notion".into(),
            consent: ConnectConsent {
                scopes_plain: vec!["Read pages and databases you can access".into()],
                can_mutate: true,
                indexes_into_memory: false,
            },
            tool_hint: 10,
        },
        StoreEntry {
            id: "slack".into(),
            kind: StoreKind::RemoteMcp,
            name: "Slack".into(),
            description: "Channels, messages, and files from the official Slack MCP connector.".into(),
            url: Some("https://api.slack.com/mcp/http".into()),
            force_legacy: false,
            flow: ConnectFlow::Pkce,
            vault_provider: "slack".into(),
            consent: ConnectConsent {
                scopes_plain: vec![
                    "Read messages from channels you belong to".into(),
                    "Post messages to channels".into(),
                ],
                can_mutate: true,
                indexes_into_memory: false,
            },
            tool_hint: 15,
        },
        // ---- Flat connectors (no MCP server) --------------------------------
        StoreEntry {
            id: "github-raw".into(),
            kind: StoreKind::Connector,
            name: "GitHub (device)".into(),
            description: "Flat GitHub connector via device flow — repos/issues/PRs as first-party tools.".into(),
            url: None,
            force_legacy: false,
            flow: ConnectFlow::DeviceCode,
            vault_provider: "github".into(),
            consent: ConnectConsent {
                scopes_plain: vec![
                    "Read your repositories, issues, and pull requests".into(),
                    "Create issues and open PRs".into(),
                ],
                can_mutate: true,
                indexes_into_memory: false,
            },
            tool_hint: 0,
        },
        StoreEntry {
            id: "gmail".into(),
            kind: StoreKind::Connector,
            name: "Gmail".into(),
            description: "Read and send email through the local Gmail connector (read-first, approve-before-send).".into(),
            url: None,
            force_legacy: false,
            flow: ConnectFlow::Pkce,
            vault_provider: "google".into(),
            consent: ConnectConsent {
                scopes_plain: vec![
                    "Read your email messages".into(),
                    "Draft and send email (always reviewed first)".into(),
                ],
                can_mutate: true,
                indexes_into_memory: false,
            },
            tool_hint: 0,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_store_has_reviewed_entries() {
        let s = StoreIndex::bundled();
        assert!(!s.is_empty());
        assert!(s.by_id.len() >= 5, "store should be non-empty");
        // Every entry carries an id + consent + a valid flow.
        for e in s.entries() {
            assert!(!e.id.is_empty());
            assert!(!e.consent.scopes_plain.is_empty());
            assert!(matches!(
                e.flow,
                ConnectFlow::Pkce | ConnectFlow::DeviceCode | ConnectFlow::ApiKey
            ));
        }
    }

    #[test]
    fn get_lookup_by_id() {
        let s = StoreIndex::bundled();
        assert!(s.get("github").is_some());
        assert!(s.get("nope").is_none());
    }

    #[test]
    fn search_matches_name_and_description() {
        let s = StoreIndex::bundled();
        assert!(s.search("git", 10).iter().any(|e| e.id == "github"));
        assert!(s.search("email", 10).iter().any(|e| e.id == "gmail"));
        assert!(s.search("zzz-none", 10).is_empty());
    }

    #[test]
    fn of_kind_splits_remote_and_flat() {
        let s = StoreIndex::bundled();
        let remotes = s.of_kind(StoreKind::RemoteMcp);
        let flat = s.of_kind(StoreKind::Connector);
        assert!(!remotes.is_empty());
        assert!(!flat.is_empty());
        // A remote entry has a server URL; a flat connector does not.
        assert!(remotes.iter().all(|e| e.url.is_some()));
        assert!(flat.iter().all(|e| e.url.is_none()));
    }

    #[test]
    fn overrides_are_possible() {
        // `with` replaces bundled entries and adds custom ones.
        let overrides = StoreIndex::with([StoreEntry {
            id: "github".into(),
            kind: StoreKind::RemoteMcp,
            name: "GitHub (custom client)".into(),
            description: "override".into(),
            url: Some("https://custom.example.com/mcp".into()),
            force_legacy: false,
            flow: ConnectFlow::DeviceCode,
            vault_provider: "copilot".into(),
            consent: ConnectConsent {
                scopes_plain: vec!["x".into()],
                can_mutate: true,
                indexes_into_memory: false,
            },
            tool_hint: 1,
        }]);
        let e = overrides.get("github").unwrap();
        assert_eq!(e.name, "GitHub (custom client)");
        assert_eq!(e.url.as_deref(), Some("https://custom.example.com/mcp"));
    }
}
