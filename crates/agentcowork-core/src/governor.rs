//! P67 Fleet Concurrency Governor — Adaptive subagent scheduling & queue management
//!
//! Controls the execution of multi-agent swarms (e.g. Primary Chief coordinating
//! 20-30 subagents). Ensures that subagent requests are admitted, queued, and
//! dispatched within system CPU, memory, provider rate-limit, and budget boundaries.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetTaskKind {
    ReadOnly,
    Mutation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetTaskStatus {
    Admitted,
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTask {
    pub task_id: String,
    pub parent_run_id: String,
    pub role: String,
    pub runtime: String,
    pub kind: FleetTaskKind,
    pub status: FleetTaskStatus,
    pub priority: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernorConfig {
    pub max_concurrent_mutation: usize,
    pub max_concurrent_readonly: usize,
    pub max_queue_capacity: usize,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            max_concurrent_mutation: 4,
            max_concurrent_readonly: 8,
            max_queue_capacity: 50,
        }
    }
}

pub struct ConcurrencyGovernor {
    config: GovernorConfig,
    active_mutation: usize,
    active_readonly: usize,
    queue: VecDeque<SubagentTask>,
}

#[derive(Debug, thiserror::Error)]
pub enum GovernorError {
    #[error("Governor queue capacity exceeded ({0})")]
    QueueFull(usize),
    #[error("subagent depth {depth} exceeds max_depth {max_depth} (no recursive spawn)")]
    DepthExceeded { depth: u32, max_depth: u32 },
    #[error("subagent concurrent limit exceeded ({active} active of {max_concurrent})")]
    ConcurrentLimitExceeded {
        active: usize,
        max_concurrent: usize,
    },
    #[error("subagent total-per-run limit exceeded ({total} of {max_total})")]
    TotalLimitExceeded { total: u32, max_total: u32 },
}

// ---------------------------------------------------------------------------
// P64.4 — Subagent admission (ARCH/17 §17.6.4, SPEC B3/I14 boundary)
// ---------------------------------------------------------------------------
//
// The canonical limits live in `agentcowork-blueprint::subagent::SubAgentLimits`
// (`max_depth 2 · max_concurrent 3 · max_total 6`); the constants below mirror
// them for the fleet-governor seam so `admit_task` wiring enforces the same
// ceiling without a second policy. `DELEGATE_BLOCKED_TOOLS` mirrors
// `agentcowork-blueprint::subagent::DELEGATE_BLOCKED_TOOLS` for the same reason:
// sub-agents inherit denies, never escalated grants.

/// P64.4 — max sub-agent depth (root parent = 0; child = 1; grandchild = 2).
pub const P64_MAX_DEPTH: u32 = 2;
/// P64.4 — max simultaneously-running sub-agents.
pub const P64_MAX_CONCURRENT: usize = 3;
/// P64.4 — max total sub-agent spawns per run.
pub const P64_MAX_TOTAL: u32 = 6;

/// P64.4 — tools a parent always withholds from sub-agents. Mirrors
/// `agentcowork-blueprint::subagent::DELEGATE_BLOCKED_TOOLS`.
pub const DELEGATE_BLOCKED_TOOLS: [&str; 5] =
    ["delegate", "clarify", "memory", "send_message", "cronjob"];

/// P64.4 — task/todo bookkeeping is default-deny for children unless the
/// parent explicitly grants it. Mirrors
/// `agentcowork-blueprint::subagent::DEFAULT_DENY_TASK_TOOLS`.
pub const DEFAULT_DENY_TASK_TOOLS: [&str; 2] = ["task", "todo"];

/// P64.4 — the effective toolset for a sub-agent: parent grants minus
/// explicit denies minus [`DELEGATE_BLOCKED_TOOLS`]. Sorted + deduped so the
/// prompt-cache body stays byte-stable.
pub fn effective_subagent_tools(granted: &[String], denied: &[String]) -> Vec<String> {
    let mut out: Vec<String> = granted
        .iter()
        .filter(|t| !denied.iter().any(|d| d == *t))
        .filter(|t| !DELEGATE_BLOCKED_TOOLS.contains(&t.as_str()))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// P64.4 — pure admission check for the Subagent trigger path. Fails closed:
/// depth, concurrency, and total are all enforced before any worktree is
/// provisioned. Returns the effective (deny-stripped) toolset on success.
pub fn check_subagent_admission(
    depth: u32,
    active: usize,
    total: u32,
    granted_tools: &[String],
    denied_tools: &[String],
) -> Result<Vec<String>, GovernorError> {
    if depth > P64_MAX_DEPTH {
        return Err(GovernorError::DepthExceeded {
            depth,
            max_depth: P64_MAX_DEPTH,
        });
    }
    if active >= P64_MAX_CONCURRENT {
        return Err(GovernorError::ConcurrentLimitExceeded {
            active,
            max_concurrent: P64_MAX_CONCURRENT,
        });
    }
    if total >= P64_MAX_TOTAL {
        return Err(GovernorError::TotalLimitExceeded {
            total,
            max_total: P64_MAX_TOTAL,
        });
    }
    Ok(effective_subagent_tools(granted_tools, denied_tools))
}

impl ConcurrencyGovernor {
    pub fn new(config: GovernorConfig) -> Self {
        Self {
            config,
            active_mutation: 0,
            active_readonly: 0,
            queue: VecDeque::new(),
        }
    }

    /// Calculate dynamic concurrency bounds based on host resources.
    pub fn calculate_dynamic_limits(cpu_count: usize, memory_gib: usize) -> GovernorConfig {
        let max_mut = (cpu_count / 2).clamp(2, 6);
        let max_ro = (cpu_count).clamp(4, 12).min(memory_gib / 2);
        GovernorConfig {
            max_concurrent_mutation: max_mut,
            max_concurrent_readonly: max_ro,
            max_queue_capacity: 50,
        }
    }

    /// Admit a subagent task into the fleet. If slots are available, immediately returns `Running`.
    /// Otherwise queues the task as `Queued`.
    pub fn admit_task(&mut self, mut task: SubagentTask) -> Result<FleetTaskStatus, GovernorError> {
        match task.kind {
            FleetTaskKind::Mutation => {
                if self.active_mutation < self.config.max_concurrent_mutation {
                    self.active_mutation += 1;
                    task.status = FleetTaskStatus::Running;
                    Ok(FleetTaskStatus::Running)
                } else {
                    if self.queue.len() >= self.config.max_queue_capacity {
                        return Err(GovernorError::QueueFull(self.config.max_queue_capacity));
                    }
                    task.status = FleetTaskStatus::Queued;
                    self.queue.push_back(task);
                    Ok(FleetTaskStatus::Queued)
                }
            }
            FleetTaskKind::ReadOnly => {
                if self.active_readonly < self.config.max_concurrent_readonly {
                    self.active_readonly += 1;
                    task.status = FleetTaskStatus::Running;
                    Ok(FleetTaskStatus::Running)
                } else {
                    if self.queue.len() >= self.config.max_queue_capacity {
                        return Err(GovernorError::QueueFull(self.config.max_queue_capacity));
                    }
                    task.status = FleetTaskStatus::Queued;
                    self.queue.push_back(task);
                    Ok(FleetTaskStatus::Queued)
                }
            }
        }
    }

    /// Complete a running task and pop the next runnable task from the queue.
    pub fn complete_task(&mut self, kind: FleetTaskKind) -> Option<SubagentTask> {
        match kind {
            FleetTaskKind::Mutation => {
                self.active_mutation = self.active_mutation.saturating_sub(1);
            }
            FleetTaskKind::ReadOnly => {
                self.active_readonly = self.active_readonly.saturating_sub(1);
            }
        }

        // Try to pop next eligible task
        if let Some(pos) = self.queue.iter().position(|t| match t.kind {
            FleetTaskKind::Mutation => self.active_mutation < self.config.max_concurrent_mutation,
            FleetTaskKind::ReadOnly => self.active_readonly < self.config.max_concurrent_readonly,
        }) {
            let mut next = self.queue.remove(pos).unwrap();
            match next.kind {
                FleetTaskKind::Mutation => self.active_mutation += 1,
                FleetTaskKind::ReadOnly => self.active_readonly += 1,
            }
            next.status = FleetTaskStatus::Running;
            Some(next)
        } else {
            None
        }
    }

    pub fn active_counts(&self) -> (usize, usize, usize) {
        (self.active_mutation, self.active_readonly, self.queue.len())
    }

    /// P64.4 — admit a sub-agent task on the Subagent trigger path.
    ///
    /// Wiring only: enforces the P64 depth/concurrency/total ceiling via
    /// [`check_subagent_admission`] first, then delegates to [`Self::admit_task`]
    /// for the normal fleet slot accounting. The returned toolset is the
    /// deny-stripped effective set the child may hold.
    pub fn admit_subagent_task(
        &mut self,
        task: SubagentTask,
        depth: u32,
        total_spawned: u32,
        granted_tools: &[String],
        denied_tools: &[String],
    ) -> Result<(FleetTaskStatus, Vec<String>), GovernorError> {
        let active_subagents = self.active_mutation + self.active_readonly;
        let effective = check_subagent_admission(
            depth,
            active_subagents,
            total_spawned,
            granted_tools,
            denied_tools,
        )?;
        let status = self.admit_task(task)?;
        Ok((status, effective))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_concurrency_governor_queueing() {
        let config = GovernorConfig {
            max_concurrent_mutation: 2,
            max_concurrent_readonly: 2,
            max_queue_capacity: 10,
        };
        let mut gov = ConcurrencyGovernor::new(config);

        let t1 = SubagentTask {
            task_id: "1".into(),
            parent_run_id: "p1".into(),
            role: "coder".into(),
            runtime: "opencode".into(),
            kind: FleetTaskKind::Mutation,
            status: FleetTaskStatus::Admitted,
            priority: 1,
        };
        let t2 = t1.clone();
        let t3 = t1.clone();

        assert_eq!(gov.admit_task(t1).unwrap(), FleetTaskStatus::Running);
        assert_eq!(gov.admit_task(t2).unwrap(), FleetTaskStatus::Running);
        assert_eq!(gov.admit_task(t3).unwrap(), FleetTaskStatus::Queued);

        assert_eq!(gov.active_counts(), (2, 0, 1));

        let next = gov.complete_task(FleetTaskKind::Mutation).unwrap();
        assert_eq!(next.status, FleetTaskStatus::Running);
        assert_eq!(gov.active_counts(), (2, 0, 0));
    }

    #[test]
    fn p64_subagent_limits_mirror_blueprint() {
        // Canonical contract: max_depth 2 / max_concurrent 3 / max_total 6.
        assert_eq!(P64_MAX_DEPTH, 2);
        assert_eq!(P64_MAX_CONCURRENT, 3);
        assert_eq!(P64_MAX_TOTAL, 6);
        assert_eq!(
            DELEGATE_BLOCKED_TOOLS,
            ["delegate", "clarify", "memory", "send_message", "cronjob"]
        );
        // agentcowork-blueprint is the canonical owner — the mirror must match.
        assert_eq!(
            P64_MAX_DEPTH,
            agentcowork_blueprint::subagent::SubAgentLimits::default().max_depth
        );
        assert_eq!(
            P64_MAX_CONCURRENT as u32,
            agentcowork_blueprint::subagent::SubAgentLimits::default().max_concurrent
        );
        assert_eq!(
            P64_MAX_TOTAL,
            agentcowork_blueprint::subagent::SubAgentLimits::default().max_total
        );
    }

    #[test]
    fn p64_effective_tools_strip_blocked_and_denies() {
        let granted = vec![
            "read".to_string(),
            "delegate".to_string(),
            "memory".to_string(),
            "write".to_string(),
        ];
        let denied = vec!["write".to_string()];
        let eff = effective_subagent_tools(&granted, &denied);
        assert_eq!(eff, vec!["read".to_string()]);
    }

    #[test]
    fn p64_admission_fails_closed_on_depth_concurrency_total() {
        // Depth exceeded.
        assert!(matches!(
            check_subagent_admission(3, 0, 0, &[], &[]),
            Err(GovernorError::DepthExceeded { .. })
        ));
        // Concurrency exceeded (3 active of 3).
        assert!(matches!(
            check_subagent_admission(1, 3, 0, &[], &[]),
            Err(GovernorError::ConcurrentLimitExceeded { .. })
        ));
        // Total exceeded (6 of 6).
        assert!(matches!(
            check_subagent_admission(1, 0, 6, &[], &[]),
            Err(GovernorError::TotalLimitExceeded { .. })
        ));
        // Happy path at the ceiling edge.
        assert!(check_subagent_admission(2, 2, 5, &[], &[]).is_ok());
    }

    #[test]
    fn p64_admit_subagent_task_wires_through_admit_task() {
        let mut gov = ConcurrencyGovernor::new(GovernorConfig {
            max_concurrent_mutation: 8,
            max_concurrent_readonly: 8,
            max_queue_capacity: 10,
        });
        let mk = |id: &str| SubagentTask {
            task_id: id.into(),
            parent_run_id: "p".into(),
            role: "coder".into(),
            runtime: "native".into(),
            kind: FleetTaskKind::Mutation,
            status: FleetTaskStatus::Admitted,
            priority: 1,
        };
        let granted = vec!["read".to_string(), "delegate".to_string()];
        let (status, eff) = gov
            .admit_subagent_task(mk("s1"), 1, 0, &granted, &[])
            .unwrap();
        assert_eq!(status, FleetTaskStatus::Running);
        assert_eq!(eff, vec!["read".to_string()]);
        // Depth 3 is refused before any fleet slot is consumed.
        let before = gov.active_counts();
        assert!(
            gov.admit_subagent_task(mk("deep"), 3, 1, &granted, &[])
                .is_err()
        );
        assert_eq!(gov.active_counts(), before);
    }
}
