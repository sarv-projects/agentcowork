//! F8 — the **official ACP agent registry** (doc 57 §2 / doc 69 §1):
//! `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`.
//!
//! This module parses the registry's published schema (agents with `npx` /
//! `uvx` / per-platform `binary` distributions — archive URL + sha256 + cmd),
//! resolves the current platform, produces a concrete [`InstallSpec`] for an
//! agent (the F8 "plan" half of plan-before-touch), merges the registry into
//! the curated [`crate::registry::LaunchRegistry`] seed, and enforces a
//! curated allow-list [`RegistryPolicy`].
//!
//! The download-and-extract executor (the "touch" half) is a separate
//! Guard-2-ticketed step; this module is the deterministic resolution + trust
//! gate that produces exactly what that step will do.

use crate::registry::{AuthMode, Distribution, HarnessManifest, HarnessProtocol, LaunchRegistry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// The registry index root (mirrors `registry.json` top level).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryIndex {
    pub version: String,
    #[serde(default)]
    pub agents: Vec<RegistryAgent>,
    #[serde(default)]
    pub extensions: Vec<serde_json::Value>,
}

/// One agent entry (the registry's `agents[]` schema).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: String,
    /// P69.C10 — the registry publishes it; the parser must not drop it.
    /// `camelCase` on the wire (`licenseUrl`).
    #[serde(default, rename = "licenseUrl")]
    pub license_url: Option<String>,
    pub distribution: RegistryDistribution,
    #[serde(default)]
    pub icon: Option<String>,
}

/// A package distribution (`npx` / `uvx` blocks share this shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PkgSpec {
    pub package: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Distribution types the registry supports. The registry JSON uses a
/// **field name** (`npx` / `uvx` / `binary`), not a `type` tag, so this is
/// untagged (exactly one key is present per agent).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RegistryDistribution {
    Npx {
        npx: PkgSpec,
    },
    Uvx {
        uvx: PkgSpec,
    },
    Binary {
        binary: HashMap<String, BinaryTarget>,
    },
}

/// A platform binary target (`binary.<platform>` in the registry).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinaryTarget {
    pub archive: String,
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// The platform keys the registry uses (match its `binary` object keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    DarwinAarch64,
    DarwinX86_64,
    LinuxAarch64,
    LinuxX86_64,
    WindowsAarch64,
    WindowsX86_64,
}

impl Platform {
    /// The current host platform (cfg-based, no runtime I/O).
    pub fn current() -> Platform {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            return Platform::DarwinAarch64;
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            return Platform::DarwinX86_64;
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            return Platform::LinuxAarch64;
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            return Platform::LinuxX86_64;
        }
        #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
        {
            return Platform::WindowsAarch64;
        }
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            return Platform::WindowsX86_64;
        }
        #[allow(unreachable_code)]
        Platform::LinuxX86_64
    }

    pub fn key(&self) -> &'static str {
        match self {
            Platform::DarwinAarch64 => "darwin-aarch64",
            Platform::DarwinX86_64 => "darwin-x86_64",
            Platform::LinuxAarch64 => "linux-aarch64",
            Platform::LinuxX86_64 => "linux-x86_64",
            Platform::WindowsAarch64 => "windows-aarch64",
            Platform::WindowsX86_64 => "windows-x86_64",
        }
    }

    pub fn is_windows(&self) -> bool {
        matches!(self, Platform::WindowsAarch64 | Platform::WindowsX86_64)
    }
}

/// The concrete "what would be installed" plan (F8 plan-before-touch).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallKind {
    /// Self-installing: `npx -y <package> <args>` at spawn; no download step.
    Npx {
        package: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    /// Self-installing: `uvx <package> <args>` at spawn; no download step.
    Uvx {
        package: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    /// Download `archive` → verify sha256 → extract → run `cmd`.
    Binary {
        archive: String,
        cmd: String,
        args: Vec<String>,
        sha256: String,
        env: Vec<(String, String)>,
    },
}

/// The F8 install plan for one agent (the ticketed executor consumes this).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallSpec {
    pub agent_id: String,
    pub name: String,
    pub version: String,
    pub license: String,
    /// P69.C10 — the registry's published license URL (carried through the
    /// plan so the consent surface can show where the license came from).
    #[serde(default)]
    pub license_url: Option<String>,
    pub kind: InstallKind,
    /// For `Binary`: the extract destination (`<data_dir>/agents/<id>/<version>`).
    #[serde(default)]
    pub install_dir: Option<PathBuf>,
}

/// Policy decision for a registry entry (doc 57 §3 — trust + ToS gate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyVerdict {
    /// Curated allow-list, open license → auto-install ok.
    Allow,
    /// Needs human approval (proprietary/ToS or off-list).
    Ask,
    /// Explicitly blocked (never install).
    Block,
}

impl PolicyVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            PolicyVerdict::Allow => "allow",
            PolicyVerdict::Ask => "ask",
            PolicyVerdict::Block => "block",
        }
    }
}

/// The curated allow-list + license gate (F8 trust + ToS).
#[derive(Debug, Clone, Default)]
pub struct RegistryPolicy {
    /// Curated ids that may auto-install without a prompt.
    pub allowlist: Vec<String>,
    /// Ids explicitly blocked (never install, even manually).
    pub denylist: Vec<String>,
}

impl RegistryPolicy {
    pub fn builtin() -> Self {
        Self {
            allowlist: vec![
                "opencode".into(),
                "goose".into(),
                "aider".into(),
                "cline".into(),
                "kimi".into(),
                "kilo".into(),
                "qwen-code".into(),
                "mistral-vibe".into(),
                "stakpak".into(),
                "harn".into(),
                "dirac".into(),
                "crow-cli".into(),
                "vtcode".into(),
                "sigit".into(),
                "minion-code".into(),
                "fast-agent".into(),
                "deepagents".into(),
            ],
            denylist: vec![],
        }
    }

    /// Evaluate an agent: denylist → Block; allowlist (or open license) →
    /// Allow; proprietary/off-list → Ask.
    pub fn evaluate(&self, id: &str, license: &str) -> PolicyVerdict {
        if self.denylist.iter().any(|d| d == id) {
            return PolicyVerdict::Block;
        }
        let open = is_open_license(license);
        if self.allowlist.iter().any(|a| a == id) || open {
            PolicyVerdict::Allow
        } else {
            PolicyVerdict::Ask
        }
    }
}

/// The exact identifier set [`is_open_license`] accepts (normalized lowercase).
/// P69.C10 — a curated list, not substring sniffing: a license string merely
/// *containing* `mit`/`bsd` is **not** treated as open.
pub const OPEN_LICENSE_IDS: &[&str] = &[
    "0bsd",
    "agpl-3.0",
    "agpl-3.0-only",
    "agpl-3.0-or-later",
    "apache-2.0",
    "bsd-2-clause",
    "bsd-3-clause",
    "cc0-1.0",
    "eupl-1.2",
    "gpl-2.0",
    "gpl-2.0-only",
    "gpl-2.0-or-later",
    "gpl-3.0",
    "gpl-3.0-only",
    "gpl-3.0-or-later",
    "isc",
    "lgpl-2.1",
    "lgpl-3.0",
    "mit",
    "mpl-2.0",
    "ms-pl",
    "unlicense",
    "wtfpl",
    "zlib",
];

/// Normalize one license token for exact matching.
fn normalize_license_token(raw: &str) -> Option<String> {
    let t = raw.trim().to_ascii_lowercase();
    // Tolerate presentation suffixes ("MIT License", "Apache-2.0 license").
    let t = t.strip_suffix(" license").unwrap_or(&t).trim().to_string();
    if t.is_empty() { None } else { Some(t) }
}

/// P69.C10 — **exact (normalized) license matching**, never substring sniffing.
/// An `A OR B` / `A/B` expression is open when at least one alternative parses
/// to a known open identifier; anything unrecognized is not open.
pub fn is_open_license(license: &str) -> bool {
    let lower = license.to_ascii_lowercase();
    lower
        .split(" or ")
        .flat_map(|p| p.split('/'))
        .filter_map(normalize_license_token)
        .any(|p| OPEN_LICENSE_IDS.contains(&p.as_str()))
}

impl RegistryIndex {
    /// Parse the official `registry.json` text.
    pub fn parse(text: &str) -> Result<RegistryIndex, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn get(&self, id: &str) -> Option<&RegistryAgent> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// Resolve an agent to its install plan for the given platform.
    pub fn install_plan(&self, id: &str, platform: Platform) -> Option<InstallSpec> {
        let a = self.get(id)?;
        let kind = match &a.distribution {
            RegistryDistribution::Npx { npx } => InstallKind::Npx {
                package: npx.package.clone(),
                args: npx.args.clone(),
                env: env_to_vec(&npx.env),
            },
            RegistryDistribution::Uvx { uvx } => InstallKind::Uvx {
                package: uvx.package.clone(),
                args: uvx.args.clone(),
                env: env_to_vec(&uvx.env),
            },
            RegistryDistribution::Binary { binary } => {
                let t = binary.get(platform.key())?;
                InstallKind::Binary {
                    archive: t.archive.clone(),
                    cmd: t.cmd.clone(),
                    args: t.args.clone(),
                    sha256: t.sha256.clone(),
                    env: env_to_vec(&t.env),
                }
            }
        };
        Some(InstallSpec {
            agent_id: a.id.clone(),
            name: a.name.clone(),
            version: a.version.clone(),
            license: a.license.clone(),
            license_url: a.license_url.clone(),
            kind,
            install_dir: None,
        })
    }

    /// Merge the registry into the curated seed. For each registry agent:
    /// - resolve the canonical id (strip `-acp`, fix the known aliases);
    /// - npx/uvx agents **pin their version** (the package string already
    ///   carries `@version`) and become spawnable immediately;
    /// - binary agents keep the seed's PATH command (or fall back to the
    ///   platform `cmd` basename) — their download spec lives in
    ///   [`RegistryIndex::install_plan`], executed by the F8 installer.
    pub fn merge_into(&self, reg: &mut LaunchRegistry, platform: Platform) {
        for a in &self.agents {
            let canon = canonical_id(&a.id);
            // P69.C7 — the registry publishes **no auth field**, so it cannot
            // be the authority. If a curated row already exists (audited),
            // keep its value; otherwise the honest default is `Unknown` — the
            // ACP handshake (`initialize` / `authenticate`) refines it. Never
            // infer auth from the software license.
            let auth = reg
                .get(&canon)
                .map(|m| m.auth_mode)
                .unwrap_or(AuthMode::Unknown);
            let (dist, env) = match &a.distribution {
                RegistryDistribution::Npx { npx } => (
                    Distribution::Npx {
                        package: npx.package.clone(),
                        args: npx.args.clone(),
                    },
                    // P69.C8 — carry `npx.env` into the manifest env, not into
                    // a description note.
                    env_to_vec(&npx.env),
                ),
                RegistryDistribution::Uvx { uvx } => (
                    Distribution::Uvx {
                        package: uvx.package.clone(),
                        args: uvx.args.clone(),
                    },
                    env_to_vec(&uvx.env),
                ),
                RegistryDistribution::Binary { binary } => {
                    // P69.C9 — ACP binary args are load-bearing (`["acp"]`,
                    // `["--acp=true"]`, …). A missing platform target must
                    // **not** start the agent's normal CLI looking broken;
                    // skip the merge so no bare launch is ever derived (the
                    // install plan already reports `None` for this platform).
                    let Some(t) = binary.get(platform.key()) else {
                        continue;
                    };
                    (
                        Distribution::Binary {
                            command: basename_cmd(&t.cmd),
                            args: t.args.clone(),
                        },
                        env_to_vec(&t.env),
                    )
                }
            };
            let manifest = HarnessManifest {
                id: canon.clone(),
                name: a.name.clone(),
                description: a.description.clone(),
                auth_mode: auth,
                distribution: dist,
                protocol: HarnessProtocol::Acp,
                env,
            };
            reg.upsert(manifest);
        }
    }
}

/// Strip a platform path to a launchable basename (`./bin/devin` → `devin`,
/// `./dist-package/cursor-agent` → `cursor-agent`, `*.exe` → name).
fn basename_cmd(cmd: &str) -> String {
    let base = cmd.rsplit('/').next().unwrap_or(cmd);
    let base = base.rsplit('\\').next().unwrap_or(base);
    base.trim_end_matches(".exe").to_string()
}

/// Map the registry id to our seed id (fix the `-acp`/alias mismatches).
fn canonical_id(id: &str) -> String {
    match id {
        "github-copilot-cli" => "copilot".to_string(),
        "grok-build" => "grok".to_string(),
        "glm-acp-agent" => "glm-agent".to_string(),
        "factory-droid" => "factory-droid".to_string(),
        other => other.trim_end_matches("-acp").to_string(),
    }
}

fn env_to_vec(env: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = env
        .iter()
        .map(|(k, val)| (k.clone(), val.clone()))
        .collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "version": "1.0.0",
      "agents": [
        {
          "id": "claude-acp",
          "name": "Claude Agent",
          "version": "0.69.0",
          "description": "ACP wrapper for Anthropic's Claude",
          "authors": ["Anthropic"],
          "license": "proprietary",
          "distribution": { "npx": { "package": "@agentclientprotocol/claude-agent-acp@0.69.0" } }
        },
        {
          "id": "cline",
          "name": "Cline",
          "version": "3.0.55",
          "description": "Autonomous coding agent CLI",
          "license": "Apache-2.0",
          "distribution": { "npx": { "package": "cline@3.0.55", "args": ["--acp"] } }
        },
        {
          "id": "devin",
          "name": "Devin",
          "version": "3000.4.25",
          "description": "Devin CLI coding agent by Cognition",
          "license": "proprietary",
          "distribution": { "binary": {
            "linux-x86_64": { "archive": "https://static.devin.ai/cli/3000.4.25/devin-linux.tar.gz", "cmd": "./bin/devin", "args": ["acp"], "sha256": "abc" },
            "darwin-aarch64": { "archive": "https://static.devin.ai/cli/3000.4.25/devin-darwin.tar.gz", "cmd": "./bin/devin", "args": ["acp"], "sha256": "def" }
          } }
        }
      ]
    }"#;

    #[test]
    fn parse_registry_and_resolve_platform() {
        let idx: RegistryIndex = RegistryIndex::parse(FIXTURE).unwrap();
        assert_eq!(idx.version, "1.0.0");
        assert_eq!(idx.agents.len(), 3);
        assert!(idx.get("claude-acp").is_some());

        // Npx: version-pinned package resolves to a self-installing plan.
        let plan = idx
            .install_plan("claude-acp", Platform::LinuxX86_64)
            .unwrap();
        match plan.kind {
            InstallKind::Npx { package, .. } => {
                assert_eq!(package, "@agentclientprotocol/claude-agent-acp@0.69.0");
            }
            _ => panic!("expected npx"),
        }

        // Binary: platform-specific archive + cmd + sha256.
        let plan = idx.install_plan("devin", Platform::LinuxX86_64).unwrap();
        match plan.kind {
            InstallKind::Binary {
                archive,
                cmd,
                sha256,
                args,
                ..
            } => {
                assert_eq!(
                    archive,
                    "https://static.devin.ai/cli/3000.4.25/devin-linux.tar.gz"
                );
                assert_eq!(cmd, "./bin/devin");
                assert_eq!(args, vec!["acp"]);
                assert_eq!(sha256, "abc");
            }
            _ => panic!("expected binary"),
        }

        // Missing platform target → no plan.
        assert!(idx.install_plan("devin", Platform::WindowsX86_64).is_none());
    }

    #[test]
    fn merge_pins_versions_and_adds_new_agents() {
        let idx: RegistryIndex = RegistryIndex::parse(FIXTURE).unwrap();
        let mut reg = LaunchRegistry::builtin();
        let before = reg.get("claude").cloned().unwrap();
        assert!(
            matches!(&before.distribution, Distribution::Npx { package, .. } if package == "@agentclientprotocol/claude-agent-acp")
        );

        idx.merge_into(&mut reg, Platform::LinuxX86_64);

        // Claude is now version-pinned from the registry.
        let claude = reg.get("claude").unwrap();
        assert!(matches!(
            &claude.distribution,
            Distribution::Npx { package, .. } if package == "@agentclientprotocol/claude-agent-acp@0.69.0"
        ));

        // Cline's args survived the merge.
        let cline = reg.get("cline").unwrap();
        assert!(
            matches!(&cline.distribution, Distribution::Npx { args, .. } if args == &vec!["--acp".to_string()])
        );

        // Devin (binary) keeps a PATH command; no built-in default exists (ADR-0005 §D1).
        let devin = reg.get("devin").unwrap();
        assert!(
            matches!(&devin.distribution, Distribution::Binary { command, .. } if command == "devin")
        );
    }

    #[test]
    fn policy_allowlist_and_license_gate() {
        let p = RegistryPolicy::builtin();
        assert_eq!(p.evaluate("opencode", "MIT"), PolicyVerdict::Allow);
        // Open license not on the allow-list still auto-allows.
        assert_eq!(
            p.evaluate("some-new-open", "Apache-2.0"),
            PolicyVerdict::Allow
        );
        // Proprietary off-list → ask.
        assert_eq!(p.evaluate("claude-acp", "proprietary"), PolicyVerdict::Ask);
        // Denylist wins.
        let mut p = RegistryPolicy::builtin();
        p.denylist.push("opencode".into());
        assert_eq!(p.evaluate("opencode", "MIT"), PolicyVerdict::Block);
    }

    #[test]
    fn license_matching_is_exact() {
        // P69.C10 — substring sniffing rejected. `"mitochondria"` contains
        // `mit`; it must not read as an open license.
        assert!(!is_open_license("mitochondria"));
        assert!(!is_open_license("BSD-like"));
        assert!(!is_open_license("proprietary"));
        assert!(is_open_license("MIT"));
        assert!(is_open_license("Apache-2.0"));
        assert!(is_open_license("MIT License"));
        assert!(is_open_license("MIT OR Apache-2.0"));
        assert!(is_open_license("Apache-2.0/MIT"));
    }

    #[test]
    fn basename_and_canonical_mapping() {
        assert_eq!(basename_cmd("./bin/devin"), "devin");
        assert_eq!(basename_cmd("./dist-package/cursor-agent"), "cursor-agent");
        assert_eq!(basename_cmd("./kilo.exe"), "kilo");
        assert_eq!(canonical_id("claude-acp"), "claude");
        assert_eq!(canonical_id("codex-acp"), "codex");
        assert_eq!(canonical_id("github-copilot-cli"), "copilot");
        assert_eq!(canonical_id("grok-build"), "grok");
        assert_eq!(canonical_id("opencode"), "opencode");
    }
}
