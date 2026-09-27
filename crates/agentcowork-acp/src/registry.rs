//! The **agent launch registry** (F12/J17 + doc 57 §2 — the Ollama
//! `ollama launch <agent>` pattern). Ollama ships a catalog of agent CLIs and
//! one command that configures + spawns them on its model backend. We adopt
//! the same shape: one [`HarnessManifest`] per agent (id, name, auth-mode
//! badge, distribution type, and *how* we drive it), plus a
//! [`LaunchRegistry`] whose default entry is our own inbuilt engine — the
//! "same chat bar, agent differs, default = AgentCowork" model.
//!
//! Data only: the actual spawn happens in `agentcowork-core` (which feeds the
//! manifest's command/args/env to a process transport and, for ACP agents,
//! drives it via [`crate::client::AcpSession`]).
//!
//! # Entrypoint provenance (verified 2026-08)
//!
//! The catalog is seeded from the **official ACP registry** —
//! `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`
//! (44 agents, Apache-2.0 registry, per-agent licenses), plus the `ollama
//! launch` catalog and the Zed `/acp` ecosystem. Spawn commands (npx/uvx
//! package + args, or binary cmd + args) are transcribed verbatim from the
//! registry's `distribution` blocks. This is the **curated seed**, not the
//! ceiling: the F8 installer + registry-fed discovery (still TODO) re-pin
//! versions + platform archives at install time.

use serde::{Deserialize, Serialize};

/// The canonical auth-mode spelling (P69.C11) — one enum for the whole stack.
///
/// Home: `agentcowork_types::AuthMode` (five variants: `subscription` /
/// `api_key` / `local` / `keyless` / `unknown`). The legacy `"local_cli"`
/// spelling emitted by `settings_cmds` is deleted, and `Local` means **local
/// inference on this machine** (`ARCH/03-BYOK-KEYRINGS.md` §3.0) — never
/// "open source" (a license property) and never "not a subscription vendor".
/// Anything the source is silent about is `Unknown` and must render as
/// unknown; the authoritative refinement is the ACP handshake.
pub use agentcowork_types::AuthMode;

/// How the agent binary is distributed (doc 57 §2: `binary`/`npx`/`uvx`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Distribution {
    /// A platform binary (`command` + optional `args`). The binary path is
    /// pinned by the F8 installer / registry-fed discovery.
    Binary {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// An npm package run via `npx` (+ optional `args`).
    Npx {
        package: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// A Python package run via `uvx` (+ optional `args`).
    Uvx {
        package: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

/// How our app drives the agent.
///
/// ADR-0005: external agents are the only v1 engines, so `Acp` is the whole
/// vocabulary. The built-in engine's `Inbuilt` and `ModelBackend` ("point
/// this CLI at my models") paths are **deferred to post-v1** and are
/// deliberately absent — nothing may depend on them (P71.2a/b).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HarnessProtocol {
    /// Drive via ACP stdio (the agent speaks ACP; spawn its ACP entrypoint).
    Acp,
}

/// One agent in the launch registry (serializable → the agent picker).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub auth_mode: AuthMode,
    pub distribution: Distribution,
    pub protocol: HarnessProtocol,
    /// Fixed env vars the agent needs (e.g. auto-update disables) — merged
    /// into the spawn env by [`LaunchRegistry::launch_plan`].
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// The concrete spawn spec `ollama launch <agent> --model X` would produce.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPlan {
    pub agent_id: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub protocol: HarnessProtocol,
}

/// The catalog of launchable agents (ADR-0005 §D1: external agents are the
/// only first-class v1 engines — there is no default/built-in selection).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRegistry {
    pub agents: Vec<HarnessManifest>,
}

impl LaunchRegistry {
    pub fn builtin() -> Self {
        // Compact constructors keep the 46-entry catalog readable.
        fn acp(
            id: &str,
            name: &str,
            desc: &str,
            auth: AuthMode,
            dist: Distribution,
        ) -> HarnessManifest {
            HarnessManifest {
                id: id.into(),
                name: name.into(),
                description: desc.into(),
                auth_mode: auth,
                distribution: dist,
                protocol: HarnessProtocol::Acp,
                env: vec![],
            }
        }
        fn npx(pkg: &str, args: &[&str]) -> Distribution {
            Distribution::Npx {
                package: pkg.into(),
                args: args.iter().map(|s| s.to_string()).collect(),
            }
        }
        fn uvx(pkg: &str, args: &[&str]) -> Distribution {
            Distribution::Uvx {
                package: pkg.into(),
                args: args.iter().map(|s| s.to_string()).collect(),
            }
        }
        fn bin(cmd: &str, args: &[&str]) -> Distribution {
            Distribution::Binary {
                command: cmd.into(),
                args: args.iter().map(|s| s.to_string()).collect(),
            }
        }

        Self {
            agents: vec![
                // ---- Frontier labs (subscription-backed official wrappers) ----
                acp(
                    "claude",
                    "Claude Code",
                    "Anthropic's coding tool with subagents (official ACP wrapper).",
                    AuthMode::Subscription,
                    npx("@agentclientprotocol/claude-agent-acp", &[]),
                ),
                acp(
                    "codex",
                    "Codex",
                    "OpenAI's coding agent (stdio ACP adapter for the Codex app server).",
                    AuthMode::Subscription,
                    npx("@agentclientprotocol/codex-acp", &[]),
                ),
                acp(
                    "gemini",
                    "Gemini CLI",
                    "Google's CLI for Gemini.",
                    AuthMode::Subscription,
                    npx("@google/gemini-cli", &["--acp"]),
                ),
                acp(
                    "copilot",
                    "GitHub Copilot",
                    "GitHub's AI pair programmer (ACP public preview).",
                    AuthMode::Subscription,
                    npx("@github/copilot", &["--acp"]),
                ),
                acp(
                    "chatgpt",
                    "ChatGPT",
                    "Complete work with ChatGPT (Codex engine, ollama launch alias).",
                    AuthMode::Subscription,
                    bin("chatgpt", &[]),
                ),
                acp(
                    "grok",
                    "Grok Build",
                    "xAI's coding agent and CLI.",
                    AuthMode::Subscription,
                    npx("@xai-official/grok", &["agent", "stdio"]),
                ),
                acp(
                    "cursor",
                    "Cursor",
                    "Cursor's coding agent.",
                    AuthMode::Subscription,
                    bin("cursor-agent", &["acp"]),
                ),
                acp(
                    "devin",
                    "Devin",
                    "Devin CLI coding agent by Cognition.",
                    AuthMode::Subscription,
                    bin("devin", &["acp"]),
                ),
                acp(
                    "junie",
                    "Junie",
                    "AI coding agent by JetBrains.",
                    AuthMode::Subscription,
                    bin("junie", &["--acp=true"]),
                ),
                acp(
                    "kiro",
                    "Kiro CLI",
                    "AWS's Kiro coding agent (kiro-cli acp).",
                    AuthMode::Subscription,
                    bin("kiro-cli", &["acp"]),
                ),
                acp(
                    "auggie",
                    "Auggie CLI",
                    "Augment Code's software agent.",
                    AuthMode::Subscription,
                    npx("@augmentcode/auggie", &["--acp"]),
                ),
                acp(
                    "codebuddy-code",
                    "Codebuddy Code",
                    "Tencent Cloud's intelligent coding tool.",
                    AuthMode::Subscription,
                    npx("@tencent-ai/codebuddy-code", &["--acp"]),
                ),
                acp(
                    "qoder",
                    "Qoder CLI",
                    "AI coding assistant with agentic capabilities.",
                    AuthMode::Subscription,
                    npx("@qoder-ai/qodercli", &["--acp"]),
                ),
                acp(
                    "poolside",
                    "Poolside",
                    "Poolside's coding agent.",
                    AuthMode::Subscription,
                    bin("pool", &["acp"]),
                ),
                acp(
                    "cortex-code",
                    "Cortex Code",
                    "Snowflake's Cortex Code agent.",
                    AuthMode::Subscription,
                    bin("cortex", &["acp", "serve"]),
                ),
                acp(
                    "nova",
                    "Nova",
                    "Compass AI's software engineer agent.",
                    AuthMode::Subscription,
                    npx("@compass-ai/nova", &["acp"]),
                ),
                acp(
                    "dimcode",
                    "DimCode",
                    "A coding agent for leading models.",
                    AuthMode::Subscription,
                    npx("dimcode", &["acp"]),
                ),
                acp(
                    "factory-droid",
                    "Factory Droid",
                    "Factory AI's coding agent.",
                    AuthMode::Subscription,
                    npx("droid", &["exec", "--output-format", "acp-daemon"]),
                ),
                // ---- Open / BYOK (a user-supplied credential on every path) ----
                // P69.C7 — `Local` means *local inference on this machine*
                // (`ARCH/03` §3.0), never "open source". These agents are BYOK
                // (they ship no model access; the user supplies an API key or
                // points them at a local endpoint), so the audited value is
                // `ApiKey`; the ACP handshake refines it at runtime when the
                // agent advertises `authMethods`.
                acp(
                    "cline",
                    "Cline",
                    "Cline CLI — autonomous coding agent (cline --acp).",
                    AuthMode::ApiKey,
                    npx("cline", &["--acp"]),
                ),
                acp(
                    "opencode",
                    "OpenCode",
                    "Anomaly's open-source coding agent (opencode acp).",
                    AuthMode::ApiKey,
                    bin("opencode", &["acp"]),
                ),
                acp(
                    "hermes",
                    "Hermes Agent",
                    "Nous Research's self-improving agent (hermes acp).",
                    AuthMode::ApiKey,
                    bin("hermes", &["acp"]),
                ),
                acp(
                    "openclaw",
                    "OpenClaw",
                    "Personal AI with 100+ skills (openclaw client acp).",
                    AuthMode::ApiKey,
                    bin("openclaw", &["client", "acp"]),
                ),
                acp(
                    "qwen-code",
                    "Qwen Code",
                    "Alibaba's Qwen coding assistant.",
                    AuthMode::ApiKey,
                    npx("@qwen-code/qwen-code", &["--acp", "--experimental-skills"]),
                ),
                acp(
                    "goose",
                    "goose",
                    "Block's local, extensible, open source agent.",
                    AuthMode::ApiKey,
                    bin("goose", &["acp"]),
                ),
                acp(
                    "aider",
                    "Aider",
                    "AI pair programming in the terminal.",
                    AuthMode::ApiKey,
                    uvx("aider-chat", &[]),
                ),
                acp(
                    "kimi",
                    "Kimi CLI",
                    "Moonshot AI's coding assistant.",
                    AuthMode::ApiKey,
                    bin("kimi", &["acp"]),
                ),
                acp(
                    "kilo",
                    "Kilo",
                    "The open source coding agent (Kilo Code).",
                    AuthMode::ApiKey,
                    npx("@kilocode/cli", &["acp"]),
                ),
                // Zhipu's agent authenticates through the GLM Coding Plan — a
                // subscription, not local inference and not BYOK.
                acp(
                    "glm-agent",
                    "GLM Agent",
                    "Zhipu AI's GLM Coding Plan agent.",
                    AuthMode::Subscription,
                    npx("glm-acp-agent", &[]),
                ),
                acp(
                    "deepagents",
                    "DeepAgents",
                    "LangChain's batteries-included agent.",
                    AuthMode::ApiKey,
                    npx("deepagents-acp", &[]),
                ),
                acp(
                    "fast-agent",
                    "fast-agent",
                    "Multi-provider agent builder.",
                    AuthMode::ApiKey,
                    uvx("fast-agent-acp", &["-x"]),
                ),
                acp(
                    "minion-code",
                    "Minion Code",
                    "AI code assistant on the Minion framework.",
                    AuthMode::ApiKey,
                    uvx("minion-code", &["acp"]),
                ),
                acp(
                    "mistral-vibe",
                    "Mistral Vibe",
                    "Mistral's open-source coding assistant.",
                    AuthMode::ApiKey,
                    bin("vibe-acp", &[]),
                ),
                acp(
                    "dirac",
                    "Dirac",
                    "Cost-optimizing, fully open-source coding agent.",
                    AuthMode::ApiKey,
                    npx("dirac-cli", &["--acp"]),
                ),
                acp(
                    "stakpak",
                    "Stakpak",
                    "Open-source DevOps agent in Rust.",
                    AuthMode::ApiKey,
                    bin("stakpak", &["acp"]),
                ),
                acp(
                    "autohand",
                    "Autohand Code",
                    "Autohand AI's coding agent.",
                    AuthMode::ApiKey,
                    npx("@autohandai/autohand-acp", &[]),
                ),
                // siGit Code is the one open agent documented as running
                // on-device inference → `Local` is accurate here.
                acp(
                    "sigit",
                    "siGit Code",
                    "Local-first agent with on-device inference.",
                    AuthMode::Local,
                    npx("@smbcloud/sigit", &[]),
                ),
                // Amp is a commercial frontier agent with its own login.
                acp(
                    "amp",
                    "Amp",
                    "ACP wrapper for Amp, the frontier coding agent.",
                    AuthMode::Subscription,
                    bin("amp-acp", &[]),
                ),
                // The source is silent on authentication for these — the honest
                // value is `Unknown`, refined by the ACP handshake (never
                // guessed from a license).
                acp(
                    "harn",
                    "Harn",
                    "Harn runs .harn agent pipelines as an ACP agent.",
                    AuthMode::Unknown,
                    bin("harn", &["serve", "acp"]),
                ),
                acp(
                    "crow-cli",
                    "crow-cli",
                    "Minimal ACP-native coding agent.",
                    AuthMode::Unknown,
                    bin("crow-cli", &["acp"]),
                ),
                acp(
                    "vtcode",
                    "VT Code",
                    "Open-source agent with LLM-native understanding.",
                    AuthMode::Unknown,
                    bin("vtcode", &["acp"]),
                ),
                acp(
                    "corust-agent",
                    "Corust Agent",
                    "Co-building with a seasoned Rust partner.",
                    AuthMode::Unknown,
                    bin("corust-agent-acp", &[]),
                ),
                acp(
                    "agoragentic",
                    "Agoragentic",
                    "Agent marketplace with 174+ AI capabilities.",
                    AuthMode::Unknown,
                    npx("agoragentic-mcp", &["--acp"]),
                ),
                acp(
                    "commandcode",
                    "Command Code",
                    "Frontier coding agent that learns your taste (candidate — verify ACP flag via registry).",
                    AuthMode::Unknown,
                    bin("commandcode", &["acp"]),
                ),
                // ---- API-key harness (DeepSeek) ----
                acp(
                    "codewhale",
                    "CodeWhale",
                    "Rust TUI coding agent (Hmbown/CodeWhale, the DeepSeek-TUI project renamed — doc 58 §6).",
                    AuthMode::ApiKey,
                    bin("codewhale", &[]),
                ),
                acp(
                    "dsh",
                    "DeepSeek Harness",
                    "DeepSeek's open-source agent harness.",
                    AuthMode::ApiKey,
                    bin("dsh", &[]),
                ),
                acp(
                    "pi",
                    "Pi",
                    "Minimal AI agent toolkit (via the pi-acp adapter).",
                    AuthMode::ApiKey,
                    npx("pi-acp", &[]),
                ),
            ],
        }
    }

    pub fn get(&self, id: &str) -> Option<&HarnessManifest> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// Insert or replace an agent by id (the registry-fed merge seam — a
    /// registry entry supersedes the seed's command/version for the same id).
    pub fn upsert(&mut self, manifest: HarnessManifest) {
        match self.agents.iter_mut().find(|a| a.id == manifest.id) {
            Some(slot) => *slot = manifest,
            None => self.agents.push(manifest),
        }
    }

    /// Resolve the spawn spec for an agent: the command + args its
    /// distribution resolves to, merged with the manifest's fixed `env`.
    /// There is no backend injection — an external agent owns its own
    /// auth/model/routing (`ARCH/CORE.md` §11, ADR-0005 §5), so we never
    /// rewrite its model endpoint through the environment.
    pub fn launch_plan(&self, id: &str) -> Option<LaunchPlan> {
        let m = self.get(id)?;
        let (command, args) = match &m.distribution {
            Distribution::Binary { command, args } => (command.clone(), args.clone()),
            Distribution::Npx { package, args } => {
                let mut a = vec!["-y".to_string(), package.clone()];
                a.extend(args.iter().cloned());
                ("npx".into(), a)
            }
            Distribution::Uvx { package, args } => {
                let mut a = vec![package.clone()];
                a.extend(args.iter().cloned());
                ("uvx".into(), a)
            }
        };
        let env = m.env.clone();
        Some(LaunchPlan {
            agent_id: m.id.clone(),
            command,
            args,
            env,
            protocol: m.protocol.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_default_agent_exists() {
        // ADR-0005 §D1: external agents are the only first-class v1 engines.
        // There is no built-in/default identity in the launch catalog.
        // NOTE (DEC-053 Step 4): `everyaios` is asserted *absent* here on
        // purpose — it is the retired agent id, recognized so already-retired
        // rows are never resurrected.
        let reg = LaunchRegistry::builtin();
        assert!(reg.get("everyaios").is_none());
        assert!(reg.get("inbuilt").is_none());
    }

    #[test]
    fn catalog_has_the_full_ecosystem() {
        let reg = LaunchRegistry::builtin();
        for id in [
            "claude",
            "codex",
            "gemini",
            "copilot",
            "chatgpt",
            "grok",
            "cursor",
            "devin",
            "junie",
            "kiro",
            "auggie",
            "codebuddy-code",
            "qoder",
            "poolside",
            "cortex-code",
            "nova",
            "dimcode",
            "factory-droid",
            "cline",
            "opencode",
            "hermes",
            "openclaw",
            "qwen-code",
            "goose",
            "aider",
            "kimi",
            "kilo",
            "glm-agent",
            "deepagents",
            "fast-agent",
            "minion-code",
            "mistral-vibe",
            "harn",
            "dirac",
            "crow-cli",
            "stakpak",
            "vtcode",
            "sigit",
            "corust-agent",
            "autohand",
            "amp",
            "agoragentic",
            "commandcode",
            "dsh",
            "pi",
        ] {
            assert!(reg.get(id).is_some(), "missing {id}");
        }
        // Claude is subscription-backed via the official ACP wrapper.
        let claude = reg.get("claude").unwrap();
        assert_eq!(claude.auth_mode, AuthMode::Subscription);
        assert_eq!(claude.protocol, HarnessProtocol::Acp);
        assert!(matches!(
            claude.distribution,
            Distribution::Npx { ref package, .. } if package == "@agentclientprotocol/claude-agent-acp"
        ));
    }

    #[test]
    fn launch_plan_resolves_npx_binary_and_uvx_with_args() {
        let reg = LaunchRegistry::builtin();
        let claude = reg.launch_plan("claude").unwrap();
        assert_eq!(claude.command, "npx");
        assert_eq!(
            claude.args,
            vec!["-y", "@agentclientprotocol/claude-agent-acp"]
        );

        // Codex goes through the stdio ACP adapter.
        assert_eq!(
            reg.launch_plan("codex").unwrap().args,
            vec!["-y", "@agentclientprotocol/codex-acp"]
        );

        // Npx + args: `npx cline --acp`.
        assert_eq!(
            reg.launch_plan("cline").unwrap().args,
            vec!["-y", "cline", "--acp"]
        );

        // Binary + args: opencode/hermes/devin/kiro use subcommand/flag.
        assert_eq!(reg.launch_plan("opencode").unwrap().args, vec!["acp"]);
        assert_eq!(reg.launch_plan("hermes").unwrap().args, vec!["acp"]);
        assert_eq!(reg.launch_plan("devin").unwrap().args, vec!["acp"]);
        assert_eq!(reg.launch_plan("kiro").unwrap().args, vec!["acp"]);
        assert_eq!(
            reg.launch_plan("copilot").unwrap().args,
            vec!["-y", "@github/copilot", "--acp"]
        );
        assert_eq!(
            reg.launch_plan("grok").unwrap().args,
            vec!["-y", "@xai-official/grok", "agent", "stdio"]
        );

        // Uvx + args: `uvx minion-code acp`.
        assert_eq!(
            reg.launch_plan("minion-code").unwrap().args,
            vec!["minion-code", "acp"]
        );
        // Uvx plain: `uvx aider-chat`.
        assert_eq!(reg.launch_plan("aider").unwrap().args, vec!["aider-chat"]);

        assert!(reg.launch_plan("nope").is_none());
    }

    #[test]
    fn launch_plan_never_injects_a_model_backend() {
        // ADR-0005 §5: provider transport is not ours. A manifest's fixed env
        // is merged; nothing is added to point the agent at a model endpoint.
        let reg = LaunchRegistry {
            agents: vec![HarnessManifest {
                id: "fixed-env-agent".into(),
                name: "Fixed Env".into(),
                description: "test".into(),
                auth_mode: AuthMode::Local,
                distribution: Distribution::Binary {
                    command: "claude".into(),
                    args: vec![],
                },
                protocol: HarnessProtocol::Acp,
                env: vec![("AGENT_FLAG".into(), "1".into())],
            }],
        };
        let plan = reg.launch_plan("fixed-env-agent").unwrap();
        assert_eq!(plan.env, vec![("AGENT_FLAG".to_string(), "1".to_string())]);
    }
}
