//! P51.10 — multi-run fan-out: launch several **Runs of the same Work**
//! (each bound to an agent binding in its own worktree), reduce their
//! outcomes, and render ordered diff walkthroughs.
//!
//! I9 — this is a strategy over Runs, never an execution kernel. There is no
//! model list here: under ADR-0005 an external agent owns its own model, so a
//! member's identity is the **agent binding + Run**, not a model id.
//!
//! Pure and deterministic: construction validates the fan-out budget, [`collect`]
//! reduces per-run outcomes, and [`walkthrough`] parses a unified diff into
//! ordered narrative steps. No execution happens here.

use serde::{Deserialize, Serialize};

/// How a [`MultiRun`]'s run outcomes are reduced.
///
/// The strategy vocabulary is unchanged (`keep_best` / `fuse`), but its basis
/// is agent/run-centric: attribution names the agent binding and Run, never a
/// model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FuseMode {
    /// Take the single highest-scoring Run's output.
    KeepBest,
    /// Concatenate every Run's output with per-agent attribution, best first.
    Fuse,
}

/// One fan-out: the same Work attempted by several Runs, each bound to an
/// agent binding in its own worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiRun {
    pub id: String,
    pub work_id: String,
    /// Agent bindings — one per Run. Never a model list.
    pub agent_ids: Vec<String>,
    /// The member Runs of `work_id`, 1:1 with `agent_ids`. **Derived here, one
    /// owner (I4)** — callers never supply Run ids, so no sidecar invents a
    /// kernel id.
    pub run_ids: Vec<String>,
    /// Isolated worktree per Run (1:1 with `agent_ids` when supplied).
    pub worktree_ids: Vec<String>,
    pub mode: FuseMode,
}

impl MultiRun {
    /// Construct a fan-out over one Work, validating the member budget
    /// (1..=5 Runs, each in its own worktree when supplied) and deriving the
    /// member Run ids.
    pub fn new(
        id: impl Into<String>,
        work_id: impl Into<String>,
        agent_ids: Vec<String>,
        worktree_ids: Vec<String>,
        mode: FuseMode,
    ) -> Result<Self, String> {
        if agent_ids.is_empty() {
            return Err("multirun requires at least one Run member — fail-closed".to_string());
        }
        if agent_ids.len() > 5 {
            return Err(format!(
                "multirun supports at most 5 Runs, got {}",
                agent_ids.len()
            ));
        }
        if !worktree_ids.is_empty() && worktree_ids.len() != agent_ids.len() {
            return Err(format!(
                "multirun requires one worktree per member (agents {}, worktrees {})",
                agent_ids.len(),
                worktree_ids.len()
            ));
        }
        let work_id = work_id.into();
        if work_id.is_empty() {
            return Err(
                "multirun requires the Work it fans out — a strategy groups Runs of one Work \
                 (fail-closed)"
                    .to_string(),
            );
        }
        let run_ids = (0..agent_ids.len())
            .map(|i| format!("{work_id}/run-{i}"))
            .collect();
        Ok(Self {
            id: id.into(),
            work_id,
            agent_ids,
            run_ids,
            worktree_ids,
            mode,
        })
    }

    /// Alias for [`MultiRun::new`].
    pub fn try_new(
        id: impl Into<String>,
        work_id: impl Into<String>,
        agent_ids: Vec<String>,
        worktree_ids: Vec<String>,
        mode: FuseMode,
    ) -> Result<Self, String> {
        Self::new(id, work_id, agent_ids, worktree_ids, mode)
    }
}

/// One Run's attempt: which agent binding produced it, and how it scored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunOutcome {
    pub run_id: String,
    pub agent_id: String,
    pub output: String,
    pub score: f64,
}

/// The reduced result of [`collect`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectedRun {
    pub output: String,
    pub best_run_id: Option<String>,
    pub best_agent_id: Option<String>,
    pub score: f64,
    pub mode: FuseMode,
}

/// Reduce `outcomes` per `mode`:
/// - [`FuseMode::KeepBest`] → the highest-score Run's output.
/// - [`FuseMode::Fuse`] → every Run concatenated with per-agent attribution
///   headers (`## <agent_id> (run <run_id>, score …)`), best score first.
pub fn collect(outcomes: Vec<RunOutcome>, mode: FuseMode) -> CollectedRun {
    if outcomes.is_empty() {
        return CollectedRun {
            output: String::new(),
            best_run_id: None,
            best_agent_id: None,
            score: 0.0,
            mode,
        };
    }
    let mut ranked = outcomes;
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let best = &ranked[0];
    match mode {
        FuseMode::KeepBest => CollectedRun {
            output: best.output.clone(),
            best_run_id: Some(best.run_id.clone()),
            best_agent_id: Some(best.agent_id.clone()),
            score: best.score,
            mode,
        },
        FuseMode::Fuse => {
            let parts: Vec<String> = ranked
                .iter()
                .map(|o| {
                    format!(
                        "## {} (run {}, score {:.2})\n{}",
                        o.agent_id, o.run_id, o.score, o.output
                    )
                })
                .collect();
            CollectedRun {
                output: parts.join("\n\n"),
                best_run_id: Some(best.run_id.clone()),
                best_agent_id: Some(best.agent_id.clone()),
                score: best.score,
                mode,
            }
        }
    }
}

/// One ordered step of a diff walkthrough.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderedDiffStep {
    pub seq: usize,
    pub path: String,
    pub hunk: String,
    pub narrative: String,
}

/// Parse a unified diff into ordered narrative steps.
///
/// A new step starts at every `@@` hunk header; the path tracks the most
/// recent `diff --git a/… b/…` (or `+++ b/…`) line. Steps are numbered in
/// document order (`seq` from 0).
pub fn walkthrough(diff_unified: &str) -> Vec<OrderedDiffStep> {
    let mut steps: Vec<OrderedDiffStep> = Vec::new();
    let mut path = "unknown".to_string();
    for line in diff_unified.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let mut parts = rest.split_whitespace();
            let a = parts.next().unwrap_or("");
            let b = parts.next().unwrap_or(a);
            let p = b.strip_prefix("b/").unwrap_or(b);
            if !p.is_empty() && p != "/dev/null" {
                path = p.to_string();
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            let p = rest.trim().strip_prefix("b/").unwrap_or(rest.trim());
            if !p.is_empty() && p != "/dev/null" {
                path = p.to_string();
            }
            continue;
        }
        if line.starts_with("@@") {
            let seq = steps.len();
            steps.push(OrderedDiffStep {
                seq,
                path: path.clone(),
                hunk: line.to_string(),
                narrative: format!("Step {}: review {} ({})", seq + 1, path, line),
            });
            continue;
        }
        if let Some(step) = steps.last_mut() {
            step.hunk.push('\n');
            step.hunk.push_str(line);
        }
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agents(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("agent-{i}")).collect()
    }

    fn run(agent: &str, run_id: &str, output: &str, score: f64) -> RunOutcome {
        RunOutcome {
            run_id: run_id.into(),
            agent_id: agent.into(),
            output: output.into(),
            score,
        }
    }

    #[test]
    fn multirun_rejects_six_runs() {
        let err = MultiRun::new(
            "run-1",
            "work-1",
            agents(6),
            vec!["wt-1".to_string()],
            FuseMode::KeepBest,
        )
        .expect_err("six runs must be rejected");
        assert!(err.contains('6') || err.contains('5'), "got: {err}");
        // Five is the budget edge and still fits.
        assert!(
            MultiRun::new(
                "run-1",
                "work-1",
                agents(5),
                vec!["wt-1".to_string(); 5],
                FuseMode::KeepBest,
            )
            .is_ok()
        );
    }

    #[test]
    fn multirun_rejects_empty_and_mismatched_members() {
        let empty = MultiRun::new("run-1", "work-1", vec![], vec![], FuseMode::KeepBest)
            .expect_err("no members must fail closed");
        assert!(empty.contains("at least one"), "got: {empty}");

        let no_work = MultiRun::new("run-1", "", agents(2), vec![], FuseMode::KeepBest)
            .expect_err("the Work is required — Runs belong to it");
        assert!(no_work.contains("requires the Work"), "got: {no_work}");

        let mismatched = MultiRun::new(
            "run-1",
            "work-1",
            agents(2),
            vec!["wt-1".to_string()],
            FuseMode::KeepBest,
        )
        .expect_err("one worktree per member is required");
        assert!(
            mismatched.contains("one worktree per member"),
            "got: {mismatched}"
        );
    }

    #[test]
    fn multirun_derives_member_runs_from_the_work() {
        let run = MultiRun::new(
            "mr-1",
            "w/subagent/t",
            vec!["a".into(), "b".into()],
            vec!["wt-a".into(), "wt-b".into()],
            FuseMode::Fuse,
        )
        .expect("fan-out of two fits the budget");
        assert_eq!(
            run.run_ids,
            vec!["w/subagent/t/run-0", "w/subagent/t/run-1"]
        );
        assert_eq!(run.work_id, "w/subagent/t");
    }

    #[test]
    fn keep_best_picks_highest() {
        let outcomes = vec![
            run("a", "r-a", "meh", 0.2),
            run("b", "r-b", "best", 0.9),
            run("c", "r-c", "mid", 0.5),
        ];
        let got = collect(outcomes, FuseMode::KeepBest);
        assert_eq!(got.output, "best");
        assert_eq!(got.best_agent_id.as_deref(), Some("b"));
        assert_eq!(got.best_run_id.as_deref(), Some("r-b"));
        assert!((got.score - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn fuse_attributes_agent_and_run() {
        let outcomes = vec![run("a", "r-a", "alpha", 0.7), run("b", "r-b", "beta", 0.4)];
        let got = collect(outcomes, FuseMode::Fuse);
        assert!(got.output.contains("## a"), "missing attribution for a");
        assert!(got.output.contains("## b"), "missing attribution for b");
        assert!(got.output.contains("run r-a"));
        assert!(got.output.contains("run r-b"));
        assert!(got.output.contains("alpha"));
        assert!(got.output.contains("beta"));
        // Best-first: `a` (0.7) precedes `b` (0.4).
        assert!(got.output.find("## a").unwrap() < got.output.find("## b").unwrap());
    }

    #[test]
    fn walkthrough_orders_hunks() {
        let diff = "\ndiff --git a/src/a.rs b/src/a.rs\n@@ -1,2 +1,3 @@\n ctx\n+one\ndiff --git a/src/b.rs b/src/b.rs\n@@ -10,2 +10,3 @@ fn b()\n ctx\n+two\n";
        let steps = walkthrough(diff);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].seq, 0);
        assert_eq!(steps[1].seq, 1);
        assert!(steps[0].seq < steps[1].seq);
        assert_eq!(steps[0].path, "src/a.rs");
        assert_eq!(steps[1].path, "src/b.rs");
        assert!(steps[0].hunk.contains("@@"));
        assert!(!steps[0].narrative.is_empty());
    }
}
