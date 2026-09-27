//! Multi-agent topologies (P6.2 — doc 63 §4.13, agent-framework orchestration
//! vocab): group-chat (shared turn loop, roles) and handoff (an agent passes
//! control + context via message). Sequential/concurrent compose from batch
//! mode. **Evaluate per-dollar/per-minute vs a single agent before shipping**
//! — multi-agent only enters where the eval data proves it (user directive).

use serde::{Deserialize, Serialize};

/// A role in a multi-agent setup (least-privilege by construction).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRole {
    pub name: String,
    pub system_prompt: String,
    /// The tool capabilities this role is granted (names).
    pub tools: Vec<String>,
    /// Read-only roles cannot request privileged actions.
    pub read_only: bool,
}

impl AgentRole {
    pub fn new(name: impl Into<String>, system_prompt: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            system_prompt: system_prompt.into(),
            tools: Vec::new(),
            read_only: false,
        }
    }

    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools = tools;
        self
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }
}

/// The topology of a multi-agent run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "topology", rename_all = "snake_case")]
pub enum Topology {
    /// A shared turn loop with a moderator; every role sees the shared
    /// transcript and speaks in turn.
    GroupChat { moderator: String, max_rounds: u32 },
    /// A chain: each agent passes control + context to the next via message.
    Handoff { chain: Vec<String> },
    /// Sequential workers over a batch (composes from batch mode).
    Sequential { workers: Vec<String> },
    /// Concurrent workers over a batch (fan-out, then reduce).
    Concurrent { workers: Vec<String> },
}

/// A multi-agent run: roles + the topology wiring them together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiAgentPlan {
    pub roles: Vec<AgentRole>,
    pub topology: Topology,
}

impl MultiAgentPlan {
    pub fn role(&self, name: &str) -> Option<&AgentRole> {
        self.roles.iter().find(|r| r.name == name)
    }

    /// Validate that every topology participant has a defined role.
    pub fn validate(&self) -> Result<(), String> {
        let participants: Vec<&str> = match &self.topology {
            Topology::GroupChat { moderator, .. } => vec![moderator],
            Topology::Handoff { chain } => chain.iter().map(String::as_str).collect(),
            Topology::Sequential { workers } | Topology::Concurrent { workers } => {
                workers.iter().map(String::as_str).collect()
            }
        };
        for p in participants {
            if self.role(p).is_none() {
                return Err(format!("topology references undefined role {p:?}"));
            }
        }
        Ok(())
    }

    /// Only the coordinator may request privileged actions; workers are
    /// read-only unless explicitly granted. A worker role that is not
    /// read-only is a policy smell — surfaced here.
    pub fn privileged_workers(&self) -> Vec<&AgentRole> {
        self.roles
            .iter()
            .filter(|r| !r.read_only)
            .filter(|r| match &self.topology {
                Topology::GroupChat { moderator, .. } => r.name != *moderator,
                Topology::Handoff { .. }
                | Topology::Sequential { .. }
                | Topology::Concurrent { .. } => true,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> MultiAgentPlan {
        MultiAgentPlan {
            roles: vec![
                AgentRole::new("coordinator", "plan and delegate"),
                AgentRole::new("researcher", "read-only research").read_only(),
                AgentRole::new("coder", "write code"),
            ],
            topology: Topology::GroupChat {
                moderator: "coordinator".into(),
                max_rounds: 5,
            },
        }
    }

    #[test]
    fn validates_defined_roles() {
        assert!(plan().validate().is_ok());
    }

    #[test]
    fn rejects_undefined_role() {
        let mut p = plan();
        p.topology = Topology::Handoff {
            chain: vec!["ghost".into()],
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn coordinator_is_not_a_privileged_worker() {
        // The moderator is not counted as a worker; the coder (not read-only)
        // is the privileged worker.
        let binding = plan();
        let privileged = binding.privileged_workers();
        assert_eq!(privileged.len(), 1);
        assert_eq!(privileged[0].name, "coder");
    }

    #[test]
    fn sequential_and_concurrent_roundtrip() {
        let mut p = plan();
        p.topology = Topology::Concurrent {
            workers: vec!["researcher".into(), "coder".into()],
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: MultiAgentPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
}
