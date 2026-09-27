//! P7.3 — Extension/Plugin ABI (I6, doc 44 §5 modularity; Zed WIT +
//! Hermes `allowed_*` pattern). The feature half of the plugin ABI; the
//! security half lives in `agentcowork-guard::granter`.
//!
//! A plugin is a directory under `~/.everyaios/plugins/<name>/` containing
//! a `manifest.toml` (schema below) plus its payload. [`PluginRegistry`]
//! scans the directory at boot and **registers** every valid plugin without
//! loading it — activation is lazy, on first use. Every plugin is bound to
//! explicit agents (never global) and its capabilities are the intersection
//! of its manifest allow-list and the host grant, computed by the
//! [`CapabilityGranter`].
//!
//! [`HostFacades`] are the host-owned `ctx` surface handed to an activated
//! plugin: `ctx.llm`, `ctx.files` (capability-scoped) and `ctx.approval()`.
//! Each facade refuses any operation outside the granted capabilities.
//!
//! [`dogfood_rule`] enforces the first-party rule: office/connector/search
//! ship as plugins, and `author = "everyaios"` can only be claimed by the
//! bundled catalog (no spoofing).

use agentcowork_guard::granter::{
    CapabilityGranter, GrantRequest, GrantedCapabilities, TrustFlags,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// The manifest ABI this host understands. Bumped only on breaking changes
/// to the schema; the validator rejects anything else.
pub const ABI_VERSION: u32 = 1;

/// The plugin slot taxonomy (where a plugin can contribute execution).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Slot {
    /// Agent loop hooks (before/after each model step).
    Loop,
    /// Scheduled-task providers / triggers.
    Scheduler,
    /// Sandboxed execution backends.
    Sandbox,
    /// Session-store providers.
    SessionStore,
}

/// Fail-closed trust flags (mirrors `agentcowork_guard::granter::TrustFlags`;
/// every flag defaults to false — nothing is allowed unless declared).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustFlagsDecl {
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub shell: bool,
    #[serde(default)]
    pub files_write: bool,
    #[serde(default)]
    pub approval_required: bool,
    #[serde(default)]
    pub sandboxed: bool,
}

impl TrustFlagsDecl {
    fn to_granter(&self) -> TrustFlags {
        TrustFlags {
            network: self.network,
            shell: self.shell,
            files_write: self.files_write,
            approval_required: self.approval_required,
            sandboxed: self.sandboxed,
        }
    }
}

/// What the plugin contributes to the host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contributes {
    /// Execution slots this plugin hooks into.
    #[serde(default)]
    pub slots: Vec<Slot>,
    /// Tool ids this plugin registers (e.g. `office.convert`).
    #[serde(default)]
    pub tools: Vec<String>,
}

/// The manifest capability declaration: allow-list ∧ host grant, with
/// explicit denies always winning.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityList {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

/// The plugin manifest's explicit agent-binding declaration — capabilities
/// are never global. Empty `bind` means the plugin is bound to nothing (and
/// is refused by the granter). Deliberately *not* named `AgentBinding`: the
/// canonical `agentcowork_types::AgentBinding` is the durable agent-attachment
/// primitive (`ARCH/AGENT.md` §3), and a manifest declaration must never
/// shadow it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAgentBinding {
    #[serde(default)]
    pub bind: Vec<String>,
}

/// The `manifest.toml` schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub abi_version: u32,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    #[serde(default)]
    pub trust: TrustFlagsDecl,
    #[serde(default)]
    pub contributes: Contributes,
    #[serde(default)]
    pub capabilities: CapabilityList,
    #[serde(default)]
    pub agents: PluginAgentBinding,
}

/// Errors from the plugin ABI.
#[derive(Debug, Error)]
pub enum PluginError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed manifest.toml in {path}: {msg}")]
    Malformed { path: String, msg: String },
    #[error("invalid plugin name `{0}` (must be [a-z0-9-]+)")]
    InvalidName(String),
    #[error("plugin `{0}` not found")]
    NotFound(String),
    #[error("plugin `{0}` already registered")]
    Exists(String),
    #[error("grant refused for plugin `{plugin}`: {msg}")]
    Grant { plugin: String, msg: String },
}

impl PluginManifest {
    fn valid_name(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 64
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }

    fn valid_version(v: &str) -> bool {
        let parts: Vec<&str> = v.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    }

    /// Parse `manifest.toml` source.
    pub fn parse(source: &str, path: &str) -> Result<PluginManifest, PluginError> {
        let m: PluginManifest = toml::from_str(source).map_err(|e| PluginError::Malformed {
            path: path.into(),
            msg: e.to_string(),
        })?;
        m.validate(path)?;
        Ok(m)
    }

    /// Schema validation at load — reject invalid manifests outright.
    /// A bad bundle never reaches the registry.
    pub fn validate(&self, path: &str) -> Result<(), PluginError> {
        if self.abi_version != ABI_VERSION {
            return Err(PluginError::Malformed {
                path: path.into(),
                msg: format!(
                    "abi_version {} unsupported (host speaks {ABI_VERSION})",
                    self.abi_version
                ),
            });
        }
        if !Self::valid_name(&self.name) {
            return Err(PluginError::InvalidName(self.name.clone()));
        }
        if !Self::valid_version(&self.version) {
            return Err(PluginError::Malformed {
                path: path.into(),
                msg: format!("version `{}` is not semver x.y.z", self.version),
            });
        }
        if self.description.is_empty() {
            return Err(PluginError::Malformed {
                path: path.into(),
                msg: "missing required `description`".into(),
            });
        }
        if self.author.is_empty() {
            return Err(PluginError::Malformed {
                path: path.into(),
                msg: "missing required `author`".into(),
            });
        }
        for cap in self
            .capabilities
            .allow
            .iter()
            .chain(self.capabilities.deny.iter())
        {
            if GrantRequest::class(cap).is_none() {
                return Err(PluginError::Malformed {
                    path: path.into(),
                    msg: format!("capability `{cap}` has no `<class>:` prefix"),
                });
            }
        }
        Ok(())
    }

    /// Build the granter request the host will evaluate this manifest with.
    pub fn grant_request(&self) -> GrantRequest {
        GrantRequest {
            name: self.name.clone(),
            agent_bindings: self.agents.bind.clone(),
            trust: self.trust.to_granter(),
            capabilities_allow: self.capabilities.allow.clone(),
            capabilities_deny: self.capabilities.deny.clone(),
        }
    }
}

/// Lazy-activation state: `scan()` only registers; `activate()` loads on
/// first use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    Registered,
    Activated,
}

/// A registered plugin: validated manifest + on-disk location + state.
#[derive(Debug, Clone)]
pub struct PluginEntry {
    pub manifest: PluginManifest,
    pub dir: PathBuf,
    pub state: PluginState,
}

/// The on-disk registry: `<root>/<name>/manifest.toml` per plugin.
#[derive(Debug, Clone)]
pub struct PluginRegistry {
    root: PathBuf,
    entries: HashMap<String, PluginEntry>,
}

impl PluginRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            entries: HashMap::new(),
        }
    }

    /// `~/.everyaios/plugins` (the documented default location).
    pub fn default_home() -> PathBuf {
        dirs_home()
            .map(|h| h.join(".everyaios").join("plugins"))
            .unwrap_or_else(|| PathBuf::from(".everyaios/plugins"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Scan `<root>/*/manifest.toml` at boot and **register** every valid
    /// plugin. Malformed bundles are skipped and reported (a bad plugin
    /// must not hide the rest). Registration never loads the plugin — that
    /// is [`PluginRegistry::activate`], on first use.
    pub fn scan(&mut self) -> Result<Vec<String>, PluginError> {
        let mut names = Vec::new();
        if !self.root.exists() {
            return Ok(names);
        }
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let manifest_path = entry.path().join("manifest.toml");
            if !manifest_path.exists() {
                continue;
            }
            let source = match std::fs::read_to_string(&manifest_path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let manifest =
                match PluginManifest::parse(&source, &manifest_path.display().to_string()) {
                    Ok(m) => m,
                    Err(_) => continue, // malformed — skip, keep the rest
                };
            let name = manifest.name.clone();
            self.entries.insert(
                name.clone(),
                PluginEntry {
                    manifest,
                    dir: entry.path(),
                    state: PluginState::Registered,
                },
            );
            names.push(name);
        }
        names.sort();
        Ok(names)
    }

    /// Lazy activation: mark the plugin loaded on first use and return it.
    /// Registration (scan) never loads; only an explicit first use does.
    pub fn activate(&mut self, name: &str) -> Result<PluginEntry, PluginError> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| PluginError::NotFound(name.into()))?;
        entry.state = PluginState::Activated;
        Ok(entry.clone())
    }

    pub fn get(&self, name: &str) -> Option<&PluginEntry> {
        self.entries.get(name)
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.entries.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Grant a registered plugin its capabilities (manifest ∧ host grant).
    /// Refusal is explicit — the plugin stays registered but unusable.
    pub fn grant(
        &self,
        name: &str,
        granter: &CapabilityGranter,
    ) -> Result<GrantedCapabilities, PluginError> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| PluginError::NotFound(name.into()))?;
        granter
            .grant(&entry.manifest.grant_request())
            .map_err(|e| PluginError::Grant {
                plugin: name.into(),
                msg: e.to_string(),
            })
    }
}

/// A capability-scoped file backend the host provides (the host performs
/// the real IO; the facade only checks the grant and delegates).
pub trait FileBackend {
    fn read(&self, path: &Path) -> Result<Vec<u8>, String>;
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String>;
    fn stat(&self, path: &Path) -> Result<u64, String>;
}

/// A host-provided LLM backend (the host owns the key and the call).
pub trait LlmBackend {
    fn call(&self, model: &str, prompt: &str) -> Result<String, String>;
}

/// Host-owned facades — the `ctx` surface a plugin sees. Every method first
/// checks the granted capabilities and refuses anything outside them.
#[derive(Debug, Clone)]
pub struct HostFacades {
    granted: GrantedCapabilities,
}

impl HostFacades {
    pub fn new(granted: GrantedCapabilities) -> Self {
        Self { granted }
    }

    pub fn granted(&self) -> &GrantedCapabilities {
        &self.granted
    }

    fn has(&self, need: &str) -> bool {
        CapabilityGranter::granted_has(&self.granted, need)
    }

    /// `ctx.llm.call(model, prompt)` — requires `llm:*` granted.
    pub fn llm_call(
        &self,
        backend: &dyn LlmBackend,
        model: &str,
        prompt: &str,
    ) -> Result<String, String> {
        let need = format!("llm:{model}");
        if !self.has("llm:**") && !self.has(&need) {
            return Err(format!(
                "ctx.llm: capability `{need}` not granted to plugin `{}`",
                self.granted.plugin
            ));
        }
        backend.call(model, prompt)
    }

    /// `ctx.files.read(path)` — requires `fs.read:<path>` granted
    /// (wildcard-aware). Paths are checked as-given; the host still applies
    /// the path-floor before any real IO.
    pub fn files_read(&self, backend: &dyn FileBackend, path: &Path) -> Result<Vec<u8>, String> {
        let need = format!("fs.read:{}", path.display());
        if !self.has("fs.read:**") && !self.has(&need) {
            return Err(format!(
                "ctx.files: capability `{need}` not granted to plugin `{}`",
                self.granted.plugin
            ));
        }
        backend.read(path)
    }

    /// `ctx.files.write(path, bytes)` — requires `fs.write:<path>` granted.
    pub fn files_write(
        &self,
        backend: &dyn FileBackend,
        path: &Path,
        bytes: &[u8],
    ) -> Result<(), String> {
        let need = format!("fs.write:{}", path.display());
        if !self.has("fs.write:**") && !self.has(&need) {
            return Err(format!(
                "ctx.files: capability `{need}` not granted to plugin `{}`",
                self.granted.plugin
            ));
        }
        backend.write(path, bytes)
    }

    /// `ctx.approval.request(operation, details)` — requires
    /// `approval:*` granted. Produces the approval record the host wires to
    /// the Guard-2 ticket card; the plugin never approves itself.
    pub fn approval_request(
        &self,
        operation: &str,
        details: &str,
    ) -> Result<ApprovalRequest, String> {
        // Approval is granted as a class (`approval:*` / `approval:**`); the
        // operation name is the *detail* of the request, not the grant key.
        if !self.has("approval:*") && !self.has("approval:**") {
            return Err(format!(
                "ctx.approval: approval capability not granted to plugin `{}`",
                self.granted.plugin
            ));
        }
        Ok(ApprovalRequest {
            plugin: self.granted.plugin.clone(),
            agent: self.granted.agent.clone(),
            operation: operation.into(),
            details: details.into(),
        })
    }
}

/// A human-approval request produced by [`HostFacades::approval_request`].
/// The host binds it to a Guard-2 ticket; the plugin cannot approve it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub plugin: String,
    pub agent: String,
    pub operation: String,
    pub details: String,
}

/// The first-party dogfood catalog: office, connector, and search ship as
/// plugins (the dogfood rule — the product eats its own ABI).
pub fn first_party_catalog() -> Vec<PluginManifest> {
    [
        (
            "office-tools",
            "office.convert",
            "office.render",
            vec!["office-worker"],
        ),
        (
            "connector-hub",
            "connector.sync",
            "connector.status",
            vec!["primary", "office-worker"],
        ),
        (
            "search-cascade",
            "search.query",
            "search.snippet",
            vec!["primary"],
        ),
    ]
    .iter()
    .map(|(name, t1, t2, agents)| PluginManifest {
        abi_version: ABI_VERSION,
        name: (*name).into(),
        version: "1.0.0".into(),
        description: format!("First-party {name} (dogfood)"),
        author: "everyaios".into(),
        trust: TrustFlagsDecl {
            approval_required: true,
            sandboxed: true,
            ..Default::default()
        },
        contributes: Contributes {
            slots: vec![Slot::Loop, Slot::Sandbox],
            tools: vec![(*t1).into(), (*t2).into()],
        },
        capabilities: CapabilityList {
            allow: vec![
                "fs.read:**".into(),
                "approval:request".into(),
                format!("{t1}:*"),
                format!("{t2}:*"),
            ],
            deny: vec![],
        },
        agents: PluginAgentBinding {
            bind: agents.iter().map(|s| s.to_string()).collect(),
        },
    })
    .collect()
}

/// The dogfood rule: `author = "everyaios"` can only be claimed by the
/// bundled first-party catalog — any other bundle claiming the first-party
/// author is rejected (spoofing prevention).
pub fn dogfood_rule(manifest: &PluginManifest) -> bool {
    if manifest.author != "everyaios" {
        return true; // third-party: normal path
    }
    first_party_catalog()
        .iter()
        .any(|fp| fp.name == manifest.name && fp.version == manifest.version)
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentcowork_guard::granter::{CapabilityGranter, HostGrant};

    const GOOD: &str = r#"
abi_version = 1
name = "office-tools"
version = "0.1.0"
description = "Office conversion + rendering"
author = "acme"

[trust]
files_write = true
approval_required = true
sandboxed = true

[contributes]
slots = ["loop", "sandbox"]
tools = ["office.convert", "office.render"]

[capabilities]
allow = ["fs.read:/tmp/office/**", "fs.write:/tmp/office/**", "approval:request"]
deny = ["fs.write:/tmp/office/secret/**"]

[agents]
bind = ["office-worker"]
"#;

    fn host() -> HostGrant {
        HostGrant {
            trusted_agents: vec!["primary".into(), "office-worker".into()],
            capabilities: vec![
                "fs.read:**".into(),
                "fs.write:/tmp/office/**".into(),
                "approval:request".into(),
            ],
        }
    }

    fn good_manifest() -> PluginManifest {
        PluginManifest::parse(GOOD, "test").unwrap()
    }

    #[test]
    fn manifest_parses_and_round_trips() {
        let m = good_manifest();
        assert_eq!(m.abi_version, ABI_VERSION);
        assert_eq!(m.name, "office-tools");
        assert_eq!(m.contributes.slots, vec![Slot::Loop, Slot::Sandbox]);
        assert!(
            m.capabilities
                .deny
                .contains(&"fs.write:/tmp/office/secret/**".into())
        );
        // validate() is idempotent on a good manifest.
        m.validate("test").unwrap();
    }

    #[test]
    fn manifest_rejects_bad_bundles() {
        // Wrong ABI version.
        let bad = GOOD.replace("abi_version = 1", "abi_version = 2");
        assert!(PluginManifest::parse(&bad, "t").is_err());
        // No capability class prefix.
        let bad = GOOD.replace("fs.read:/tmp/office/**", "bare-capability");
        assert!(PluginManifest::parse(&bad, "t").is_err());
        // Bad version.
        let bad = GOOD.replace("version = \"0.1.0\"", "version = \"1.0\"");
        assert!(PluginManifest::parse(&bad, "t").is_err());
        // Bad name.
        let bad = GOOD.replace("name = \"office-tools\"", "name = \"Office Tools!\"");
        assert!(PluginManifest::parse(&bad, "t").is_err());
        // Not even TOML.
        assert!(PluginManifest::parse("not toml [[[", "t").is_err());
    }

    #[test]
    fn registry_scans_registers_but_does_not_load() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-plugin-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("office-tools")).unwrap();
        std::fs::write(dir.join("office-tools/manifest.toml"), GOOD).unwrap();
        std::fs::create_dir_all(dir.join("broken")).unwrap();
        std::fs::write(dir.join("broken/manifest.toml"), "abi_version = 99").unwrap();

        let mut reg = PluginRegistry::new(&dir);
        let names = reg.scan().unwrap();
        assert_eq!(names, vec!["office-tools".to_string()]); // broken skipped
        assert_eq!(reg.len(), 1);
        // Lazy: registered, not loaded.
        assert_eq!(
            reg.get("office-tools").unwrap().state,
            PluginState::Registered
        );
        // Explicit first use activates.
        reg.activate("office-tools").unwrap();
        assert_eq!(
            reg.get("office-tools").unwrap().state,
            PluginState::Activated
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grant_blocks_unlisted_exec_and_explicit_deny() {
        let mut reg = PluginRegistry::new("unused");
        reg.entries.insert(
            "office-tools".into(),
            PluginEntry {
                manifest: good_manifest(),
                dir: PathBuf::from("unused"),
                state: PluginState::Registered,
            },
        );
        let granter = CapabilityGranter::new(host());
        let granted = reg.grant("office-tools", &granter).unwrap();
        assert_eq!(granted.agent, "office-worker");
        assert!(CapabilityGranter::granted_has(
            &granted,
            "fs.read:/tmp/office/x.pdf"
        ));
        assert!(!CapabilityGranter::granted_has(
            &granted,
            "fs.write:/tmp/office/secret/x.pdf"
        ));
        assert!(!CapabilityGranter::granted_has(&granted, "shell:any"));

        // An unlisted exec capability refuses the whole plugin.
        let mut m = good_manifest();
        m.capabilities.allow.push("shell:any".into());
        m.trust.shell = true;
        reg.entries.insert(
            "evil".into(),
            PluginEntry {
                manifest: m,
                dir: PathBuf::from("unused"),
                state: PluginState::Registered,
            },
        );
        assert!(reg.grant("evil", &granter).is_err());
    }

    #[test]
    fn facades_are_capability_scoped() {
        struct Mem(#[allow(dead_code)] HashMap<String, Vec<u8>>);
        impl FileBackend for Mem {
            fn read(&self, path: &Path) -> Result<Vec<u8>, String> {
                self.0
                    .get(&path.display().to_string())
                    .cloned()
                    .ok_or("no such file".into())
            }
            fn write(&self, _path: &Path, _b: &[u8]) -> Result<(), String> {
                Ok(())
            }
            fn stat(&self, _path: &Path) -> Result<u64, String> {
                Ok(0)
            }
        }
        struct Llm;
        impl LlmBackend for Llm {
            fn call(&self, _m: &str, p: &str) -> Result<String, String> {
                Ok(format!("echo:{p}"))
            }
        }

        let mut reg = PluginRegistry::new("unused");
        reg.entries.insert(
            "office-tools".into(),
            PluginEntry {
                manifest: good_manifest(),
                dir: PathBuf::from("unused"),
                state: PluginState::Registered,
            },
        );
        let granted = reg
            .grant("office-tools", &CapabilityGranter::new(host()))
            .unwrap();
        let f = HostFacades::new(granted);
        let mut map = HashMap::new();
        map.insert("/tmp/office/x/y.pdf".to_string(), b"pdf".to_vec());
        let mem = Mem(map);

        // Allowed by fs.read:/tmp/office/**.
        assert_eq!(
            f.files_read(&mem, Path::new("/tmp/office/x/y.pdf"))
                .unwrap(),
            b"pdf"
        );
        // Outside the grant → refused (even though the backend exists).
        assert!(f.files_read(&mem, Path::new("/home/secret.txt")).is_err());
        // llm not granted → refused.
        assert!(f.llm_call(&Llm, "gpt", "hi").is_err());
        // approval granted → record produced.
        let a = f
            .approval_request("office.convert", "convert report.docx → pdf")
            .unwrap();
        assert_eq!(a.plugin, "office-tools");
        assert_eq!(a.agent, "office-worker");
    }

    #[test]
    fn dogfood_rule_blocks_spoofed_first_party() {
        for fp in first_party_catalog() {
            assert!(dogfood_rule(&fp));
        }
        let mut spoofed = good_manifest();
        spoofed.author = "everyaios".into(); // third-party bundle claiming first-party author
        assert!(!dogfood_rule(&spoofed));
    }
}
