//! Spec-per-task files (P6.1 — doc 63 §4.18, codger/openspec pattern).
//!
//! The main agent writes one `spec.md` per task: goal + acceptance checks +
//! context. A sub-agent receives its spec as its starting context and returns
//! a written status block — **the specs are the persistent memory**; the main
//! agent never holds the full history.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One task's spec — the exact content a sub-agent starts from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub id: String,
    pub goal: String,
    /// Context snippets the sub-agent needs (and nothing more).
    pub context: Vec<String>,
    /// Human-readable acceptance criteria (the typed checks live in the
    /// verify block on the blueprint task).
    pub acceptance: Vec<String>,
}

#[derive(Debug, Error)]
pub enum SpecError {
    #[error("missing task id header (# Task: …)")]
    MissingId,
    #[error("missing goal (**Goal:** …)")]
    MissingGoal,
}

impl TaskSpec {
    pub fn new(id: impl Into<String>, goal: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            goal: goal.into(),
            context: Vec::new(),
            acceptance: Vec::new(),
        }
    }

    pub fn with_context(mut self, context: Vec<String>) -> Self {
        self.context = context;
        self
    }

    pub fn with_acceptance(mut self, acceptance: Vec<String>) -> Self {
        self.acceptance = acceptance;
        self
    }

    /// Serialize to the `spec.md` a sub-agent receives.
    pub fn to_markdown(&self) -> String {
        let mut out = format!("# Task: {}\n\n**Goal:** {}\n", self.id, self.goal);
        if !self.context.is_empty() {
            out.push_str("\n## Context\n");
            for c in &self.context {
                out.push_str(&format!("- {c}\n"));
            }
        }
        if !self.acceptance.is_empty() {
            out.push_str("\n## Acceptance\n");
            for a in &self.acceptance {
                out.push_str(&format!("- [ ] {a}\n"));
            }
        }
        out
    }

    /// Parse a `spec.md` back (tolerant — unknown sections are ignored).
    pub fn from_markdown(md: &str) -> Result<Self, SpecError> {
        let mut id = None;
        let mut goal = None;
        let mut context = Vec::new();
        let mut acceptance = Vec::new();
        let mut section = Section::None;

        for raw in md.lines() {
            let line = raw.trim();
            if let Some(rest) = line.strip_prefix("# Task:") {
                id = Some(rest.trim().to_string());
                continue;
            }
            if let Some(rest) = line.strip_prefix("**Goal:**") {
                goal = Some(rest.trim().to_string());
                continue;
            }
            match line {
                "## Context" => section = Section::Context,
                "## Acceptance" => section = Section::Acceptance,
                "## Verify" | "## Status" | "## Dependencies" => section = Section::Other,
                _ => {}
            }
            match section {
                Section::Context => {
                    if let Some(item) = line.strip_prefix('-') {
                        context.push(item.trim().to_string());
                    }
                }
                Section::Acceptance => {
                    if let Some(item) = line
                        .strip_prefix("- [ ]")
                        .or_else(|| line.strip_prefix("-"))
                    {
                        acceptance.push(item.trim().to_string());
                    }
                }
                Section::None | Section::Other => {}
            }
        }

        Ok(Self {
            id: id.ok_or(SpecError::MissingId)?,
            goal: goal.ok_or(SpecError::MissingGoal)?,
            context,
            acceptance,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    Context,
    Acceptance,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_roundtrips() {
        let spec = TaskSpec::new("task-1", "Add the /health route")
            .with_context(vec!["src/server.rs is the entry".into()])
            .with_acceptance(vec!["GET /health returns 200".into()]);
        let md = spec.to_markdown();
        assert!(md.contains("# Task: task-1"));
        assert!(md.contains("**Goal:** Add the /health route"));
        let back = TaskSpec::from_markdown(&md).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn parse_requires_id_and_goal() {
        assert!(matches!(
            TaskSpec::from_markdown("**Goal:** g"),
            Err(SpecError::MissingId)
        ));
        assert!(matches!(
            TaskSpec::from_markdown("# Task: t"),
            Err(SpecError::MissingGoal)
        ));
    }

    #[test]
    fn parse_tolerates_unknown_sections() {
        let md = "# Task: t\n**Goal:** g\n## Verify\n- exists(x)\n## Acceptance\n- [ ] a\n";
        let spec = TaskSpec::from_markdown(md).unwrap();
        assert_eq!(spec.acceptance, vec!["a".to_string()]);
        assert!(spec.context.is_empty());
    }
}
