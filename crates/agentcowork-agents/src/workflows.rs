//! P31.9 — workflows + automations attachment: blueprints (B2) and
//! scheduled automations (B7) live in the same bundle; agent-owned runs
//! land in the audit timeline. This module is the pure bookkeeping for what
//! a run is; the coordinator executes against the blueprint/scheduler
//! engines.

use serde::{Deserialize, Serialize};

/// A workflow run detached from chat, owned by an agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRun {
    /// Which agent owns the run.
    pub agent_id: String,
    /// Which workflow/automation (blueprint id or automation id).
    pub workflow_id: String,
    pub kind: RunKind,
    /// The audit timeline lands the receipt here (J5 append-only family).
    pub timeline_id: Option<String>,
    pub started_at_ms: u64,
    #[serde(default)]
    pub finished_at_ms: Option<u64>,
    #[serde(default)]
    pub status: RunStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunKind {
    Blueprint,
    Automation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStatus {
    #[default]
    Pending,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

/// The ledger of agent-owned runs (in-memory; the audit log is the durable
/// copy).
#[derive(Debug, Clone, Default)]
pub struct AgentRuns {
    runs: Vec<AgentRun>,
}

impl AgentRuns {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(
        &mut self,
        agent_id: &str,
        workflow_id: &str,
        kind: RunKind,
        at_ms: u64,
        timeline_id: Option<String>,
    ) -> String {
        let id = format!("run-{}-{at_ms}", self.runs.len() + 1);
        self.runs.push(AgentRun {
            agent_id: agent_id.to_string(),
            workflow_id: workflow_id.to_string(),
            kind,
            timeline_id,
            started_at_ms: at_ms,
            finished_at_ms: None,
            status: RunStatus::Running,
        });
        id
    }

    pub fn finish(&mut self, index: usize, status: RunStatus, at_ms: u64) -> Result<(), String> {
        let run = self.runs.get_mut(index).ok_or("no such run")?;
        run.status = status;
        run.finished_at_ms = Some(at_ms);
        Ok(())
    }

    pub fn runs_for(&self, agent_id: &str) -> Vec<&AgentRun> {
        self.runs
            .iter()
            .filter(|r| r.agent_id == agent_id)
            .collect()
    }

    pub fn timeline(&self) -> &[AgentRun] {
        &self.runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_lifecycle() {
        let mut runs = AgentRuns::new();
        runs.begin(
            "analyst",
            "pivot-sheet",
            RunKind::Blueprint,
            100,
            Some("tl-1".to_string()),
        );
        let agent_runs = runs.runs_for("analyst");
        assert_eq!(agent_runs.len(), 1);
        assert_eq!(agent_runs[0].status, RunStatus::Running);
        runs.finish(0, RunStatus::Succeeded, 500).unwrap();
        assert_eq!(runs.timeline()[0].finished_at_ms, Some(500));
    }

    #[test]
    fn runs_are_per_agent() {
        let mut runs = AgentRuns::new();
        runs.begin("a", "w1", RunKind::Automation, 1, None);
        runs.begin("b", "w2", RunKind::Blueprint, 2, None);
        assert_eq!(runs.runs_for("a").len(), 1);
        assert!(runs.runs_for("a")[0].workflow_id == "w1");
    }
}
