//! Markdown blueprint format (P6.1 — B2 §P2, doc 03): a whole [`Blueprint`]
//! round-trips through one `.md` file so the main agent, sub-agents, and a
//! rebooted session all share the same durable source of truth.
//!
//! A file may carry an optional agent-frontmatter block (`---` delimited,
//! qwen-code/Claude-Code shape) above the blueprint body, so a single `.md`
//! doubles as an agent config + its task plan.
//!
//! Body shape (status/verify/deps are first-class, not prose):
//!
//! ```text
//! # Blueprint: bp-1
//! **Goal:** ship the report
//!
//! ## Tasks
//! ### a
//! **Goal:** write the summary
//! **Depends on:** (none)
//! **Status:** pending
//! ## Context
//! - src/report.md is the draft
//! ## Acceptance
//! - [ ] summary.md exists
//! ## Verify
//! exists summary.md
//! contains summary.md shipped
//! ## Policy
//! - never touch raw.xlsx
//! ```

use crate::blueprint::{Blueprint, BlueprintTask, TaskStatus, VerifyBlock};
use crate::frontmatter::{
    AgentConfig, FrontmatterError, Isolation, PermissionMode, parse_frontmatter,
};
use crate::spec::TaskSpec;
use crate::verify::{HashAlgorithm, OutcomeCheck};
use thiserror::Error;

/// A parsed blueprint file: an optional agent-frontmatter block + the plan.
#[derive(Debug, Clone, PartialEq)]
pub struct BlueprintDoc {
    pub agent_config: Option<AgentConfig>,
    pub blueprint: Blueprint,
}

#[derive(Debug, Error)]
pub enum MdError {
    #[error("missing # Blueprint: id header")]
    MissingId,
    #[error("missing top-level **Goal:**")]
    MissingGoal,
    #[error("missing task id header (### …)")]
    MissingTaskId,
    #[error("missing task goal (**Goal:**)")]
    MissingTaskGoal,
    #[error("unknown task status {0:?}")]
    InvalidStatus(String),
    #[error("malformed verify line {0:?}")]
    MalformedVerifyLine(String),
    #[error(transparent)]
    Frontmatter(#[from] FrontmatterError),
    #[error("unbalanced frontmatter delimiters")]
    UnbalancedFrontmatter,
}

impl Blueprint {
    /// Serialize the plan (without any agent frontmatter) to markdown.
    pub fn to_markdown(&self) -> String {
        let mut out = format!("# Blueprint: {}\n\n**Goal:** {}\n", self.id, self.goal);
        out.push_str("\n## Tasks\n");
        for task in &self.tasks {
            out.push_str("\n### ");
            out.push_str(&task.spec.id);
            out.push('\n');
            out.push_str(&format!("**Goal:** {}\n", task.spec.goal));
            if !task.depends_on.is_empty() {
                out.push_str(&format!("**Depends on:** {}\n", task.depends_on.join(", ")));
            }
            out.push_str(&format!("**Status:** {}\n", task.status.as_str()));
            if !task.spec.context.is_empty() {
                out.push_str("\n## Context\n");
                for c in &task.spec.context {
                    out.push_str(&format!("- {c}\n"));
                }
            }
            if !task.spec.acceptance.is_empty() {
                out.push_str("\n## Acceptance\n");
                for a in &task.spec.acceptance {
                    out.push_str(&format!("- [ ] {a}\n"));
                }
            }
            if !task.verify.checks.is_empty() {
                out.push_str("\n## Verify\n");
                for c in &task.verify.checks {
                    out.push_str(&check_line(c));
                    out.push('\n');
                }
            }
            if !task.verify.policy.is_empty() {
                out.push_str("\n## Policy\n");
                for p in &task.verify.policy {
                    out.push_str(&format!("- {p}\n"));
                }
            }
        }
        out
    }

    /// Parse a plan from markdown (ignoring any agent frontmatter).
    pub fn from_markdown(md: &str) -> Result<Self, MdError> {
        Ok(BlueprintDoc::from_markdown(md)?.blueprint)
    }
}

impl BlueprintDoc {
    /// Serialize with an optional agent-frontmatter block.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        if let Some(cfg) = &self.agent_config {
            out.push_str(&frontmatter_to_markdown(cfg));
            out.push('\n');
        }
        out.push_str(&self.blueprint.to_markdown());
        out
    }

    /// Parse a blueprint file (with or without a leading frontmatter block).
    pub fn from_markdown(md: &str) -> Result<Self, MdError> {
        let (fm, body) = split_frontmatter(md)?;
        let agent_config = fm
            .map(|f| parse_frontmatter(&format!("---\n{f}\n---")))
            .transpose()?;
        let blueprint = parse_blueprint_body(body)?;
        Ok(Self {
            agent_config,
            blueprint,
        })
    }
}

fn split_frontmatter(md: &str) -> Result<(Option<&str>, &str), MdError> {
    let trimmed = md.trim_start();
    let Some(rest) = trimmed.strip_prefix("---\n") else {
        return Ok((None, md));
    };
    let Some(end) = rest.find("\n---") else {
        return Err(MdError::UnbalancedFrontmatter);
    };
    let (fm, body) = rest.split_at(end);
    // body starts at "\n---" → skip past the closing delimiter.
    let body = body.trim_start_matches("\n---").trim_start_matches('\n');
    Ok((Some(fm.trim()), body))
}

fn parse_blueprint_body(md: &str) -> Result<Blueprint, MdError> {
    let mut id = None;
    let mut goal = None;
    let mut tasks: Vec<BlueprintTask> = Vec::new();
    let mut current: Option<TaskBuilder> = None;

    for raw in md.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("# Blueprint:") {
            id = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("**Goal:**") {
            match current.as_mut() {
                None => goal = Some(rest.trim().to_string()),
                Some(b) => b.goal = Some(rest.trim().to_string()),
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("### ") {
            if let Some(b) = current.take() {
                tasks.push(b.finish()?);
            }
            current = Some(TaskBuilder::new(rest.trim().to_string()));
            continue;
        }
        if let Some(rest) = line.strip_prefix("**Depends on:**") {
            let b = current.as_mut().ok_or(MdError::MissingTaskId)?;
            b.depends_on = rest
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            continue;
        }
        if let Some(rest) = line.strip_prefix("**Status:**") {
            let b = current.as_mut().ok_or(MdError::MissingTaskId)?;
            b.status = Some(
                TaskStatus::parse(rest.trim())
                    .ok_or_else(|| MdError::InvalidStatus(rest.trim().to_string()))?,
            );
            continue;
        }
        if line.starts_with("## ") {
            let section = match line {
                "## Context" => Section::Context,
                "## Acceptance" => Section::Acceptance,
                "## Verify" => Section::Verify,
                "## Policy" => Section::Policy,
                _ => Section::Other,
            };
            if let Some(b) = current.as_mut() {
                b.section = section;
            }
            continue;
        }
        // Content line inside a task section.
        let Some(b) = current.as_mut() else {
            continue;
        };
        match b.section {
            Section::Context => {
                if let Some(item) = line.strip_prefix('-') {
                    b.context.push(item.trim().to_string());
                }
            }
            Section::Acceptance => {
                if let Some(item) = line
                    .strip_prefix("- [ ]")
                    .or_else(|| line.strip_prefix('-'))
                {
                    b.acceptance.push(item.trim().to_string());
                }
            }
            Section::Verify => {
                if !line.starts_with('-') {
                    b.checks.push(parse_check(line)?);
                }
            }
            Section::Policy => {
                if let Some(item) = line.strip_prefix('-') {
                    b.policy.push(item.trim().to_string());
                }
            }
            Section::None | Section::Other => {}
        }
    }
    if let Some(b) = current {
        tasks.push(b.finish()?);
    }

    Ok(Blueprint {
        id: id.ok_or(MdError::MissingId)?,
        goal: goal.ok_or(MdError::MissingGoal)?,
        tasks,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    Context,
    Acceptance,
    Verify,
    Policy,
    Other,
}

/// A task being assembled line-by-line.
struct TaskBuilder {
    id: String,
    goal: Option<String>,
    context: Vec<String>,
    acceptance: Vec<String>,
    checks: Vec<OutcomeCheck>,
    policy: Vec<String>,
    depends_on: Vec<String>,
    status: Option<TaskStatus>,
    section: Section,
}

impl TaskBuilder {
    fn new(id: String) -> Self {
        Self {
            id,
            goal: None,
            context: Vec::new(),
            acceptance: Vec::new(),
            checks: Vec::new(),
            policy: Vec::new(),
            depends_on: Vec::new(),
            status: None,
            section: Section::None,
        }
    }

    fn finish(self) -> Result<BlueprintTask, MdError> {
        let goal = self.goal.ok_or(MdError::MissingTaskGoal)?;
        Ok(BlueprintTask {
            spec: TaskSpec {
                id: self.id,
                goal,
                context: self.context,
                acceptance: self.acceptance,
            },
            verify: VerifyBlock {
                checks: self.checks,
                policy: self.policy,
            },
            depends_on: self.depends_on,
            status: self.status.unwrap_or(TaskStatus::Pending),
        })
    }
}

fn check_line(c: &OutcomeCheck) -> String {
    match c {
        OutcomeCheck::FileExists { path } => format!("exists {path}"),
        OutcomeCheck::FileContains { path, substring } => format!("contains {path} {substring}"),
        OutcomeCheck::FileHash {
            path,
            algorithm,
            expected,
        } => format!("hash {path} {} {expected}", algorithm_str(*algorithm)),
    }
}

fn parse_check(line: &str) -> Result<OutcomeCheck, MdError> {
    if let Some(path) = line.strip_prefix("exists ") {
        let path = path.trim();
        if path.is_empty() {
            return Err(MdError::MalformedVerifyLine(line.to_string()));
        }
        return Ok(OutcomeCheck::FileExists { path: path.into() });
    }
    if let Some(rest) = line.strip_prefix("contains ") {
        let (path, substring) = rest
            .split_once(' ')
            .ok_or_else(|| MdError::MalformedVerifyLine(line.to_string()))?;
        let path = path.trim();
        let substring = substring.trim();
        if path.is_empty() || substring.is_empty() {
            return Err(MdError::MalformedVerifyLine(line.to_string()));
        }
        return Ok(OutcomeCheck::FileContains {
            path: path.into(),
            substring: substring.into(),
        });
    }
    if let Some(rest) = line.strip_prefix("hash ") {
        let mut parts = rest.splitn(3, ' ');
        let path = parts.next().unwrap_or("").trim();
        let alg = parts.next().unwrap_or("");
        let expected = parts.next().unwrap_or("").trim();
        let algorithm =
            parse_algorithm(alg).ok_or_else(|| MdError::MalformedVerifyLine(line.to_string()))?;
        if path.is_empty() || expected.is_empty() {
            return Err(MdError::MalformedVerifyLine(line.to_string()));
        }
        return Ok(OutcomeCheck::FileHash {
            path: path.into(),
            algorithm,
            expected: expected.into(),
        });
    }
    Err(MdError::MalformedVerifyLine(line.to_string()))
}

fn algorithm_str(a: HashAlgorithm) -> &'static str {
    match a {
        HashAlgorithm::Sha256 => "sha256",
        HashAlgorithm::Sha1 => "sha1",
    }
}

fn parse_algorithm(s: &str) -> Option<HashAlgorithm> {
    match s.to_ascii_lowercase().as_str() {
        "sha256" => Some(HashAlgorithm::Sha256),
        "sha1" => Some(HashAlgorithm::Sha1),
        _ => None,
    }
}

fn frontmatter_to_markdown(cfg: &AgentConfig) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!(
        "permissionMode: {}\n",
        permission_mode_str(cfg.permission_mode)
    ));
    if let Some(color) = &cfg.color {
        out.push_str(&format!("color: {color}\n"));
    }
    if let Some(max_turns) = cfg.max_turns {
        out.push_str(&format!("maxTurns: {max_turns}\n"));
    }
    // P19-3 — the doc-75 fields (effort / background / isolation) ride the
    // same frontmatter so agent files round-trip losslessly.
    if let Some(effort) = cfg.effort {
        out.push_str(&format!("effort: {effort}\n"));
    }
    if let Some(background) = &cfg.background {
        out.push_str(&format!("background: {background}\n"));
    }
    if cfg.isolation != Isolation::None {
        out.push_str("isolation: worktree\n");
    }
    if !cfg.hooks.is_empty() {
        out.push_str("hooks:\n");
        for h in &cfg.hooks {
            out.push_str(&format!("  - {h}\n"));
        }
    }
    if !cfg.mcp_servers.is_empty() {
        out.push_str("mcpServers:\n");
        for m in &cfg.mcp_servers {
            out.push_str(&format!("  - {m}\n"));
        }
    }
    out.push_str("---\n");
    out
}

fn permission_mode_str(m: PermissionMode) -> &'static str {
    match m {
        PermissionMode::Default => "default",
        PermissionMode::Plan => "plan",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::Auto => "auto",
        PermissionMode::BypassPermissions => "bypassPermissions",
        PermissionMode::DontAsk => "dontAsk",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, goal: &str) -> BlueprintTask {
        BlueprintTask::new(TaskSpec::new(id, goal), VerifyBlock::new(vec![]))
    }

    #[test]
    fn blueprint_markdown_roundtrips_losslessly() {
        let mut bp = Blueprint::new("bp-1", "ship the report");
        let a = task("a", "write the summary");
        let mut b = task("b", "attach the chart");
        b.depends_on.push("a".into());
        b.status = TaskStatus::Done;
        b.verify = VerifyBlock {
            checks: vec![
                OutcomeCheck::FileExists {
                    path: "summary.md".into(),
                },
                OutcomeCheck::FileContains {
                    path: "summary.md".into(),
                    substring: "shipped".into(),
                },
                OutcomeCheck::FileHash {
                    path: "out.bin".into(),
                    algorithm: HashAlgorithm::Sha256,
                    expected: "abc123".into(),
                },
            ],
            policy: vec!["never touch raw.xlsx".into()],
        };
        b.spec.context = vec!["src/report.md is the draft".into()];
        b.spec.acceptance = vec!["summary.md exists".into()];
        bp.push(a);
        bp.push(b);

        let md = bp.to_markdown();
        let back = Blueprint::from_markdown(&md).unwrap();
        assert_eq!(back, bp);
    }

    #[test]
    fn doc_roundtrips_with_frontmatter() {
        let mut bp = Blueprint::new("bp-1", "goal");
        bp.push(task("a", "do a"));
        let doc = BlueprintDoc {
            agent_config: Some(AgentConfig {
                permission_mode: PermissionMode::Plan,
                color: Some("red".into()),
                hooks: vec!["npm test".into()],
                mcp_servers: vec!["browser".into()],
                max_turns: Some(50),
                effort: Some(5),
                background: None,
                isolation: Isolation::Worktree,
            }),
            blueprint: bp,
        };
        let md = doc.to_markdown();
        let back = BlueprintDoc::from_markdown(&md).unwrap();
        assert_eq!(back, doc);
    }

    #[test]
    fn body_without_frontmatter_has_no_agent_config() {
        let mut bp = Blueprint::new("bp-1", "goal");
        bp.push(task("a", "do a"));
        let doc = BlueprintDoc::from_markdown(&bp.to_markdown()).unwrap();
        assert!(doc.agent_config.is_none());
    }

    #[test]
    fn parse_requires_id_and_goal() {
        assert!(matches!(
            Blueprint::from_markdown("**Goal:** g\n## Tasks\n"),
            Err(MdError::MissingId)
        ));
        assert!(matches!(
            Blueprint::from_markdown("# Blueprint: b\n## Tasks\n"),
            Err(MdError::MissingGoal)
        ));
    }

    #[test]
    fn parse_requires_task_goal() {
        let md = "# Blueprint: b\n**Goal:** g\n## Tasks\n### a\n**Status:** done\n";
        assert!(matches!(
            Blueprint::from_markdown(md),
            Err(MdError::MissingTaskGoal)
        ));
    }

    #[test]
    fn malformed_verify_line_errors() {
        let md =
            "# Blueprint: b\n**Goal:** g\n## Tasks\n### a\n**Goal:** ga\n## Verify\nbogus line\n";
        assert!(matches!(
            Blueprint::from_markdown(md),
            Err(MdError::MalformedVerifyLine(_))
        ));
    }

    #[test]
    fn unbalanced_frontmatter_errors() {
        let md = "---\npermissionMode: plan\n# Blueprint: b\n";
        assert!(matches!(
            BlueprintDoc::from_markdown(md),
            Err(MdError::UnbalancedFrontmatter)
        ));
    }
}
