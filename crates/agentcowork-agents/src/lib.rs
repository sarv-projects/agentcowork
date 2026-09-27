//! P31 — custom agent bundles (B9, user directive 2026-08-17).
//!
//! `agent.toml` bundles: persona + engine binding + optional model pin +
//! scoped MCP/connectors/skills/tools + workflows. Composes existing rows
//! (F8/F12/J17 ACP, A2/A6 models, P22 MCP, F7, F-connectors, I6 guard,
//! B2/B7 workflows); adds no engines — this crate is the bundle schema,
//! the registry, the templates, and the scope computation.

pub mod bundle;
pub mod directory;
pub mod moa;
pub mod registry;
pub mod scope;
pub mod templates;
pub mod workflows;

pub use bundle::{AgentBundle, EngineBinding, ModelPin, ToolScope};
pub use directory::{AgentDirectory, AgentDirectoryEntry, AgentSource};
pub use moa::{Fusion, MoACatalog, MoAPreset, Routing};
pub use registry::{AgentMeta, AgentRegistry};
pub use scope::AgentScopes;
pub use templates::AgentTemplate;
pub use workflows::{AgentRun, AgentRuns, RunKind, RunStatus};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_to_end_bundle_to_scope() {
        // General → Coder template → save → load → scopes.
        let root =
            std::env::temp_dir().join(format!("agentcowork-agents-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let reg = AgentRegistry::new(root.clone());
        let coder = AgentTemplate::Coder.bundle();
        reg.save(&coder).unwrap();
        let loaded = reg.load("coder").unwrap();
        let scopes = AgentScopes::from_bundle(&loaded);
        assert!(scopes.can_use_mcp("filesystem"));
        assert!(scopes.tool_allowed("fs.write"));
        assert!(!scopes.tool_allowed("fs.remove"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
