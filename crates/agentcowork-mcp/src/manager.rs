//! P22 — Built-in MCP server **manager** (doc 74): the runtime half of the
//! directory (P18 = catalog surface, P22 = install → spawn → serve →
//! reconcile).
//!
//! Mirrors the proven `agentcowork-acp` registry/installer/transport machinery
//! for *consuming* third-party MCP servers, fed by the official MCP Registry
//! API shape (`registry.modelcontextprotocol.io/v0/servers`): per-server
//! `packages[]` give `registryType`/`identifier`/`runtimeHint`/`transport`/
//! `packageArguments` — the same install shape as the F8 ACP registry.
//!
//! Trust posture (K6): the registry is community-curated **discovery, never
//! a trust boundary** — a curated allow-list gates installs, distributions
//! must be npx/uvx/binary (never floating/arbitrary), binaries are sha256-
//! pinned, and spawned servers are managed children the host can kill.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One server as the official MCP Registry API describes it (the fields the
/// manager consumes; extra API fields are ignored).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryServer {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// `npx` | `uvx` | `binary` | `remote`.
    #[serde(rename = "registryType", default)]
    pub registry_type: String,
    /// The package/binary identifier (e.g. `@scope/server-name`).
    #[serde(default)]
    pub identifier: String,
    #[serde(rename = "runtimeHint", default)]
    pub runtime_hint: String,
    #[serde(default)]
    pub transport: String,
    /// Extra argv for the distribution (e.g. `--config foo.json`).
    #[serde(rename = "packageArguments", default)]
    pub package_arguments: Vec<String>,
    /// Published sha256 for binary installs (empty = no binary pin).
    #[serde(default)]
    pub sha256: String,
}

impl RegistryServer {
    /// The install command shape: `npx <identifier> [args...]` etc.
    pub fn command(&self) -> Vec<String> {
        let mut cmd = vec![self.registry_type.clone(), self.identifier.clone()];
        cmd.extend(self.package_arguments.iter().cloned());
        cmd
    }
}

/// Parsed registry index (official API array shape) + search + pagination.
/// `persist`/`load` are the offline-cache contract (paths are runtime wiring;
/// this module owns the shape, not the disk I/O).
#[derive(Debug, Clone, Default)]
pub struct RegistryIndex {
    by_id: BTreeMap<String, RegistryServer>,
}

impl RegistryIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parse(servers: &[RegistryServer]) -> Self {
        let mut by_id = BTreeMap::new();
        for s in servers {
            if !s.id.is_empty() {
                by_id.insert(s.id.clone(), s.clone());
            }
        }
        Self { by_id }
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&RegistryServer> {
        self.by_id.get(id)
    }

    /// Case-insensitive substring search over id + name + description.
    pub fn search(&self, term: &str, limit: usize) -> Vec<&RegistryServer> {
        let t = term.to_ascii_lowercase();
        self.by_id
            .values()
            .filter(|s| {
                s.id.to_ascii_lowercase().contains(&t)
                    || s.name.to_ascii_lowercase().contains(&t)
                    || s.description.to_ascii_lowercase().contains(&t)
            })
            .take(limit)
            .collect()
    }

    /// Offset/limit pagination over the full set.
    pub fn page(&self, offset: usize, limit: usize) -> Vec<&RegistryServer> {
        self.by_id.values().skip(offset).take(limit).collect()
    }
}

/// The curated allow-list. Fail-closed: an id not on this list is refused
/// (K6 — community registry is discovery, not trust).
pub const ALLOW_LIST: &[&str] = &[
    "filesystem", // modelcontextprotocol/server-filesystem
    "github",
    "gitlab",
    "slack",
    "notion",
    "linear",
    "figma",
    "stripe",
    "sentry",
    "postgres",
    "sqlite",
    "memory",
    "fetch",
    "sequential-thinking",
    "everart",
];

pub fn is_allowed(id: &str) -> bool {
    ALLOW_LIST.contains(&id)
}

/// Why an install plan was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("`{0}` is not on the curated allow-list")]
    NotAllowed(String),
    #[error("unrecognized registryType `{0}` (expected npx/uvx/binary)")]
    UnsupportedType(String),
    #[error("missing package identifier")]
    MissingIdentifier,
    #[error("floating unpinned package `{0}` — refusing (K6 version pinning)")]
    Floating(String),
    #[error("remote server needs an https (or loopback) URL, got `{0}`")]
    RemoteUrl(String),
}

/// The validated install plan (what the executor runs). For npx/uvx the
/// package manager self-installs at first spawn — the plan records the pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub id: String,
    /// `npx <pkg>@<pin> [args...]`
    pub command: String,
    pub args: Vec<String>,
    /// Published sha256 for binary installs ("" = n/a).
    pub sha256_pin: String,
}

/// The validated dtype for a **remote** server (`registryType: "remote"`):
/// the app connects over HTTP/SSE with OAuth 2.1 authorization instead of
/// spawning a child. No executable bytes cross the trust boundary — the
/// server is a reviewed URL, not a downloaded binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePlan {
    pub id: String,
    /// HTTPS server URL (the OAuth 2.1 protected-resource endpoint).
    pub url: String,
    /// Optional vault OAuth `provider` key (which ProviderSettings to route
    /// the authorization through). Empty for public servers.
    pub oauth_provider: String,
}

/// Validate a remote server: allow-listed AND an https (or loopback-dev)
/// URL. `install_plan` has no URL field (stdio children), so remote targets
/// go through `remote_plan` — the Connect-Store path.
pub fn remote_plan(server: &RegistryServer, consent_url: &str) -> Result<RemotePlan, PlanError> {
    if !is_allowed(&server.id) {
        return Err(PlanError::NotAllowed(server.id.clone()));
    }
    if server.registry_type != "remote" {
        return Err(PlanError::UnsupportedType(server.registry_type.clone()));
    }
    let url = consent_url.trim();
    let is_https = url.starts_with("https://");
    let is_loopback = url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost");
    if url.is_empty() || !(is_https || is_loopback) {
        return Err(PlanError::RemoteUrl(url.to_string()));
    }
    Ok(RemotePlan {
        id: server.id.clone(),
        url: url.to_string(),
        oauth_provider: server.identifier.clone(),
    })
}

/// Validate an install: allow-listed, recognized distribution, and for
/// npx/uvx the package must carry an explicit version pin (never float).
pub fn install_plan(server: &RegistryServer) -> Result<InstallPlan, PlanError> {
    if !is_allowed(&server.id) {
        return Err(PlanError::NotAllowed(server.id.clone()));
    }
    let identifier = &server.identifier;
    if identifier.is_empty() {
        return Err(PlanError::MissingIdentifier);
    }
    match server.registry_type.as_str() {
        "npx" | "uvx" => {
            // pin required: `@scope/name@1.2.3` or `name@1.2.3`
            let rest = identifier
                .strip_prefix('@')
                .map(|s| s.split_once('/').map(|(_, p)| p).unwrap_or(s))
                .unwrap_or(identifier.as_str());
            if !rest.contains('@') {
                return Err(PlanError::Floating(identifier.clone()));
            }
            Ok(InstallPlan {
                id: server.id.clone(),
                command: server.registry_type.clone(),
                args: vec![identifier.clone()]
                    .into_iter()
                    .chain(server.package_arguments.iter().cloned())
                    .collect(),
                sha256_pin: server.sha256.clone(),
            })
        }
        "binary" => {
            if server.sha256.is_empty() {
                // a binary without a published pin is as good as floating
                return Err(PlanError::Floating(identifier.clone()));
            }
            Ok(InstallPlan {
                id: server.id.clone(),
                command: identifier.clone(),
                args: server.package_arguments.clone(),
                sha256_pin: server.sha256.clone(),
            })
        }
        "remote" => Err(PlanError::UnsupportedType("remote".into())),
        other => Err(PlanError::UnsupportedType(other.into())),
    }
}

/// Verify a downloaded archive against the published sha256 pin (the K6
/// trust boundary for binary installs). Pure + testable; the download itself
/// is the documented runtime seam.
pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> bool {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let got = format!("{:x}", hasher.finalize());
    got.eq_ignore_ascii_case(expected_hex.trim())
}

/// The managed-child seam: the host spawns a server through this trait so
/// lifecycle logic is testable without exec'ing real processes. The default
/// `ProcessSpawner` wraps `std::process::Command` (runtime wiring).
pub trait ServerSpawner {
    fn spawn(&mut self, command: &str, args: &[String]) -> Result<ChildHandle, SpawnError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("spawn failed: {0}")]
    Msg(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// A handle to a managed child (killable by the host).
#[derive(Debug, Clone)]
pub struct ChildHandle {
    pub pid: u32,
    pub alive: bool,
}

/// The runtime spawner: a real stdio child. stdio framing/reuse of the
/// existing `AttachedServer::spawn` path is the documented integration point.
#[derive(Debug, Default)]
pub struct ProcessSpawner;

impl ServerSpawner for ProcessSpawner {
    fn spawn(&mut self, command: &str, args: &[String]) -> Result<ChildHandle, SpawnError> {
        let child = std::process::Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        Ok(ChildHandle {
            pid: child.id(),
            alive: true,
        })
    }
}

/// One installed + (optionally) running server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedServer {
    pub id: String,
    pub plan: InstallPlan,
    pub state: ServerState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerState {
    Installed,
    Running,
    Stopped,
}

/// The tool surface a server's `tools/list` contributes to the agent
/// registry — kind/readOnly/openWorld/profile, merged with provenance so the
/// registry always knows where a tool came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSurface {
    /// The tool name (`tools/list` entry name).
    pub name: String,
    /// `read` | `write` | `both` (from the readOnlyHint).
    pub kind: String,
    pub read_only: bool,
    pub open_world: bool,
    /// e.g. `default` | `sandboxed` — set by the host policy.
    pub profile: String,
    /// Which server contributed this tool.
    pub origin: String,
}

/// Merge a server's tool list into a catalog surface. `readOnlyHint` maps to
/// `read`/`write`/`both`; the profile is host policy input, not server data.
pub fn merge_into_catalog(
    server_id: &str,
    tools: &[crate::server::ToolListEntry],
    default_profile: &str,
) -> Vec<ToolSurface> {
    tools
        .iter()
        .map(|t| ToolSurface {
            name: t.name.clone(),
            kind: if t.read_only {
                "read".into()
            } else {
                "write".into()
            },
            read_only: t.read_only,
            open_world: t.open_world,
            profile: default_profile.to_string(),
            origin: server_id.to_string(),
        })
        .collect()
}

/// The one-click lifecycle: allow-list → plan → (download+verify for
/// binary) → managed child. Pure orchestration over the seams above.
#[derive(Debug, Default)]
pub struct McpServerManager<S: ServerSpawner = ProcessSpawner> {
    index: RegistryIndex,
    spawner: S,
    installed: BTreeMap<String, ManagedServer>,
    /// The never-allowed list (K6 quarantine).
    quarantined: Vec<String>,
}

impl<S: ServerSpawner> McpServerManager<S> {
    pub fn new(index: RegistryIndex, spawner: S) -> Self {
        Self {
            index,
            spawner,
            installed: BTreeMap::new(),
            quarantined: Vec::new(),
        }
    }

    pub fn index(&self) -> &RegistryIndex {
        &self.index
    }

    pub fn installed(&self) -> Vec<&ManagedServer> {
        self.installed.values().collect()
    }

    pub fn quarantine(&mut self, id: &str) {
        if !self.quarantined.contains(&id.to_string()) {
            self.quarantined.push(id.to_string());
        }
        self.installed.remove(id);
    }

    pub fn is_quarantined(&self, id: &str) -> bool {
        self.quarantined.iter().any(|q| q == id)
    }

    /// Plan + record an install (no bytes touched here — the caller runs the
    /// plan through the download/verify seam for binaries).
    pub fn install(&mut self, id: &str) -> Result<InstallPlan, PlanError> {
        if self.is_quarantined(id) {
            return Err(PlanError::NotAllowed(id.to_string()));
        }
        let server = self
            .index
            .get(id)
            .ok_or_else(|| PlanError::NotAllowed(id.to_string()))?;
        let plan = install_plan(server)?;
        self.installed.insert(
            id.to_string(),
            ManagedServer {
                id: id.to_string(),
                plan: plan.clone(),
                state: ServerState::Installed,
            },
        );
        Ok(plan)
    }

    /// Spawn an installed server as a managed child.
    pub fn run(&mut self, id: &str) -> Result<ChildHandle, SpawnError> {
        let plan = self
            .installed
            .get(id)
            .map(|m| m.plan.clone())
            .ok_or_else(|| SpawnError::Msg(format!("`{id}` is not installed")))?;
        let handle = self.spawner.spawn(&plan.command, &plan.args)?;
        if let Some(m) = self.installed.get_mut(id) {
            m.state = ServerState::Running;
        }
        Ok(handle)
    }

    pub fn stop(&mut self, id: &str) {
        if let Some(m) = self.installed.get_mut(id) {
            m.state = ServerState::Stopped;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(id: &str, ty: &str, identifier: &str) -> RegistryServer {
        RegistryServer {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            registry_type: ty.into(),
            identifier: identifier.into(),
            runtime_hint: String::new(),
            transport: "stdio".into(),
            package_arguments: vec![],
            sha256: String::new(),
        }
    }

    #[test]
    fn index_searches_and_paginates() {
        let idx = RegistryIndex::parse(&[
            server(
                "filesystem",
                "npx",
                "@modelcontextprotocol/server-filesystem@0.6.2",
            ),
            server("github", "remote", "github"),
            server("postgres", "binary", "/usr/local/bin/mcp-postgres"),
        ]);
        assert_eq!(idx.len(), 3);
        assert_eq!(idx.search("FILE", 5).len(), 1);
        assert_eq!(idx.page(1, 1)[0].id, "github");
        assert!(idx.get("postgres").is_some());
    }

    #[test]
    fn allow_list_is_fail_closed() {
        assert!(is_allowed("filesystem"));
        assert!(!is_allowed("some-suspicious-server"));
        let mut m = McpServerManager::new(
            RegistryIndex::parse(&[server("bad", "npx", "bad@1.0.0")]),
            ProcessSpawner,
        );
        assert!(matches!(m.install("bad"), Err(PlanError::NotAllowed(_))));
    }

    #[test]
    fn plan_refuses_floating_and_unknown_types() {
        let floating = server(
            "filesystem",
            "npx",
            "@modelcontextprotocol/server-filesystem",
        );
        assert!(matches!(
            install_plan(&floating),
            Err(PlanError::Floating(_))
        ));
        let binary_without_pin = server("sqlite", "binary", "/usr/bin/mcp-sqlite");
        assert!(matches!(
            install_plan(&binary_without_pin),
            Err(PlanError::Floating(_))
        ));
        let remote = server("github", "remote", "github");
        // A remote goes through `remote_plan`, not `install_plan`. `install_plan`
        // still refuses it (no child executable to run).
        assert!(matches!(
            install_plan(&remote),
            Err(PlanError::UnsupportedType(_))
        ));
        // ...but `remote_plan` accepts an https remote on the allow-list.
        let github_https = server("github", "remote", "github");
        let plan = remote_plan(&github_https, "https://api.githubcopilot.com/mcp/").unwrap();
        assert_eq!(plan.url, "https://api.githubcopilot.com/mcp/");
        assert_eq!(plan.oauth_provider, "github");
        // Non-https remote is rejected.
        assert!(matches!(
            remote_plan(&github_https, "http://insecure.example.com/mcp"),
            Err(PlanError::RemoteUrl(_))
        ));
    }

    #[test]
    fn remote_plan_requires_allow_list_and_https() {
        let bad = server("some-suspicious", "remote", "x");
        assert!(matches!(
            remote_plan(&bad, "https://evil.com/mcp"),
            Err(PlanError::NotAllowed(_))
        ));
        let ok = server("github", "remote", "github");
        assert!(remote_plan(&ok, "https://api.githubcopilot.com/mcp/").is_ok());
        // loopback dev is allowed too
        assert!(remote_plan(&ok, "http://127.0.0.1:8080/mcp").is_ok());
    }

    #[test]
    fn pinned_npx_plan_is_ok() {
        let s = server(
            "filesystem",
            "npx",
            "@modelcontextprotocol/server-filesystem@0.6.2",
        );
        let plan = install_plan(&s).unwrap();
        assert_eq!(plan.command, "npx");
        assert_eq!(
            plan.args[0],
            "@modelcontextprotocol/server-filesystem@0.6.2"
        );
    }

    #[test]
    fn sha256_verify_matches() {
        use sha2::Digest;
        let bytes = b"hello mcp";
        let mut hasher = sha2::Sha256::new();
        hasher.update(bytes);
        let hex = format!("{:x}", hasher.finalize());
        assert!(verify_sha256(bytes, &hex));
        assert!(!verify_sha256(bytes, "0000"));
    }

    #[test]
    fn lifecycle_install_run_stop_quarantine() {
        struct FakeSpawner;
        impl ServerSpawner for FakeSpawner {
            fn spawn(&mut self, _c: &str, _a: &[String]) -> Result<ChildHandle, SpawnError> {
                Ok(ChildHandle {
                    pid: 4242,
                    alive: true,
                })
            }
        }
        let idx = RegistryIndex::parse(&[server(
            "filesystem",
            "npx",
            "@modelcontextprotocol/server-filesystem@0.6.2",
        )]);
        let mut m = McpServerManager::new(idx, FakeSpawner);
        m.install("filesystem").unwrap();
        let handle = m.run("filesystem").unwrap();
        assert_eq!(handle.pid, 4242);
        assert_eq!(m.installed()[0].state, ServerState::Running);
        m.stop("filesystem");
        assert_eq!(m.installed()[0].state, ServerState::Stopped);
        m.quarantine("filesystem");
        assert!(m.is_quarantined("filesystem"));
        assert!(matches!(
            m.install("filesystem"),
            Err(PlanError::NotAllowed(_))
        ));
    }

    #[test]
    fn tool_surface_merge_carries_provenance() {
        use crate::server::ToolListEntry;
        let tools = vec![
            ToolListEntry {
                name: "read_file".into(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                read_only: true,
                open_world: false,
            },
            ToolListEntry {
                name: "write_file".into(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                read_only: false,
                open_world: true,
            },
        ];
        let surface = merge_into_catalog("filesystem", &tools, "sandboxed");
        assert_eq!(surface.len(), 2);
        assert_eq!(surface[0].kind, "read");
        assert_eq!(surface[0].origin, "filesystem");
        assert_eq!(surface[1].kind, "write");
        assert!(surface[1].open_world);
        assert_eq!(surface[0].profile, "sandboxed");
    }
}
