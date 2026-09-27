//! A7 MoA presets (doc 69 §3 — `hermes moa` steal): named Mixture-of-Agents
//! presets the planner can select — multi-brain routing beyond the current
//! single tier pick. A preset declares an agent lineup, a routing strategy,
//! and the fusion rule that turns N drafts into one answer.
//!
//! Pure data + selection: the preset is the contract; the coordinator runs
//! the lineup and fuses the drafts.

use serde::{Deserialize, Serialize};

/// How the lineup is run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Routing {
    /// All agents draft in parallel, then fuse.
    Parallel,
    /// Agents draft in order; each sees the previous drafts (progressive).
    Sequential,
    /// One delegator assigns sub-prompts per agent, then fuses.
    Delegated,
}

/// How the drafts become one answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fusion {
    /// The orchestrator picks the best draft (ranked, not blended).
    Select,
    /// Synthesize a new answer from all drafts (blend).
    Synthesize,
}

/// One named MoA preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoAPreset {
    pub id: String,
    pub description: String,
    /// The agent lineup, in invocation order.
    pub agents: Vec<String>,
    pub routing: Routing,
    pub fusion: Fusion,
    /// Max total turns the whole MoA run may consume (the budget gate).
    pub max_turns: u32,
}

/// The preset catalog — the planner picks by id, never hard-codes a lineup.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MoACatalog {
    pub presets: Vec<MoAPreset>,
}

impl MoACatalog {
    /// The built-in presets (defaults the planner can offer out of the box).
    pub fn builtin() -> Self {
        Self {
            presets: vec![
                MoAPreset {
                    id: "bounded".into(),
                    description:
                        "Two agents, parallel, cheapest first — the default multi-brain route."
                            .into(),
                    agents: vec!["agentcowork-inbuilt".into(), "claude".into()],
                    routing: Routing::Parallel,
                    fusion: Fusion::Select,
                    max_turns: 60,
                },
                MoAPreset {
                    id: "diverse".into(),
                    description:
                        "Three different-model agents for divergent drafts, then synthesize.".into(),
                    agents: vec!["claude".into(), "grok".into(), "qwen-code".into()],
                    routing: Routing::Parallel,
                    fusion: Fusion::Synthesize,
                    max_turns: 90,
                },
                MoAPreset {
                    id: "depth".into(),
                    description: "Sequential deepening — each agent builds on the previous draft."
                        .into(),
                    agents: vec![
                        "agentcowork-inbuilt".into(),
                        "claude".into(),
                        "codex".into(),
                    ],
                    routing: Routing::Sequential,
                    fusion: Fusion::Synthesize,
                    max_turns: 120,
                },
                MoAPreset {
                    id: "review".into(),
                    description: "One drafter, one critic, one finalizer (delegated).".into(),
                    agents: vec![
                        "claude".into(),
                        "opencode".into(),
                        "agentcowork-inbuilt".into(),
                    ],
                    routing: Routing::Delegated,
                    fusion: Fusion::Select,
                    max_turns: 80,
                },
            ],
        }
    }

    pub fn find(&self, id: &str) -> Option<&MoAPreset> {
        self.presets.iter().find(|p| p.id == id)
    }

    pub fn ids(&self) -> Vec<&str> {
        self.presets.iter().map(|p| p.id.as_str()).collect()
    }

    /// Validate a lineup against the registry of known agent ids: unknown
    /// agents are reported, never silently dropped.
    pub fn validate(&self, preset_id: &str, known_agents: &[String]) -> Result<(), Vec<String>> {
        let preset = self
            .find(preset_id)
            .ok_or_else(|| vec![preset_id.to_string()])?;
        let unknown: Vec<String> = preset
            .agents
            .iter()
            .filter(|a| !known_agents.iter().any(|k| k == *a))
            .cloned()
            .collect();
        if unknown.is_empty() {
            Ok(())
        } else {
            Err(unknown)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_offers_named_presets() {
        let c = MoACatalog::builtin();
        assert_eq!(c.ids(), vec!["bounded", "diverse", "depth", "review"]);
        let d = c.find("depth").unwrap();
        assert_eq!(d.routing, Routing::Sequential);
        assert_eq!(d.fusion, Fusion::Synthesize);
        assert!(c.find("nope").is_none());
    }

    #[test]
    fn validate_reports_unknown_agents() {
        let c = MoACatalog::builtin();
        assert!(
            c.validate("bounded", &["agentcowork-inbuilt".into(), "claude".into()])
                .is_ok()
        );
        let err = c
            .validate("bounded", &["agentcowork-inbuilt".into()])
            .unwrap_err();
        assert_eq!(err, vec!["claude".to_string()]);
        assert!(c.validate("ghost", &[]).is_err());
    }

    #[test]
    fn presets_are_deterministic_data() {
        let a = MoACatalog::builtin();
        let b = MoACatalog::builtin();
        assert_eq!(a, b);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }
}
