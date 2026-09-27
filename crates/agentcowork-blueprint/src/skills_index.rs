//! F8 skills_index.json manifest (doc 65 §6 — agentic-awesome-skills steal):
//! a machine-readable discovery index for the skill registry, plus
//! `compose_stack` — a **read-only** validation that a proposed stack of
//! skills is resolvable against the index, emitting `selection_evidence`
//! (why each skill was chosen) and rejecting unknowns or conflicts. No side
//! effects: composing never writes, installs, or mutates the store.

use crate::skill_store::{Skill, SkillManifest};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One row of the discovery index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct IndexEntry {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub version: String,
    /// P51.28 — Crush/Zed: listed as a user slash command.
    #[serde(default)]
    pub user_invocable: bool,
    /// P51.28 — model must not auto-select; user `/name` still works.
    #[serde(default)]
    pub disable_model_invocation: bool,
}

impl From<&SkillManifest> for IndexEntry {
    fn from(m: &SkillManifest) -> Self {
        Self {
            name: m.name.clone(),
            description: m.description.clone(),
            tags: m.triggers.clone(),
            author: m.author.clone(),
            version: m.version.clone(),
            user_invocable: m.user_invocable,
            disable_model_invocation: m.disable_model_invocation,
        }
    }
}

/// The on-disk `skills_index.json` shape (versioned for forward drift).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsIndexFile {
    /// Format version — bump on breaking shape changes; readers tolerate a
    /// newer minor by ignoring unknown fields (serde default).
    pub version: u32,
    #[serde(default)]
    pub skills: Vec<IndexEntry>,
}

impl SkillsIndexFile {
    pub fn new(skills: Vec<IndexEntry>) -> Self {
        Self { version: 1, skills }
    }

    /// Build the index from a store scan (pure — only reads the store).
    pub fn from_skills(skills: &[Skill]) -> Self {
        Self::new(
            skills
                .iter()
                .map(|s| IndexEntry::from(&s.manifest))
                .collect(),
        )
    }

    /// Write the index file. Sorted by name for deterministic diffs.
    pub fn write_to(&self, path: &Path) -> Result<(), String> {
        let mut entries = self.skills.clone();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let json = serde_json::to_string_pretty(&SkillsIndexFile {
            skills: entries,
            ..self.clone()
        })
        .map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    /// Read the index file (missing file → empty index, never an error).
    pub fn read_from(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path).unwrap_or_else(|_| "{\"version\":1}".into());
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    }

    /// Look up a skill by name.
    pub fn get(&self, name: &str) -> Option<&IndexEntry> {
        self.skills.iter().find(|e| e.name == name)
    }
}

/// Why one skill was selected into the stack — the evidence trail the
/// planner can show (F8 `selection_evidence`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectionEvidence {
    pub name: String,
    /// The signal that matched (trigger/tool/description/name).
    pub matched_on: String,
    /// Deterministic relevance score in `[0, 1]`.
    pub score: f64,
    /// Whether the skill is already active (idempotence check).
    pub already_active: bool,
}

/// Why a skill was rejected from the stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionReason {
    /// Not present in the index (typo or not installed).
    Unknown,
    /// Listed in the stack more than once.
    Duplicate,
    /// Conflicts with another selected skill (declared tags collide).
    Conflict { with: String },
    /// P51.28 — `disable-model-invocation: true`; only an explicit user invoke.
    ModelInvocationDisabled,
}

/// The result of a compose-stack validation. Read-only — the caller decides
/// what to do with the evidence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ComposeOutcome {
    #[serde(default)]
    pub selected: Vec<SelectionEvidence>,
    #[serde(default)]
    pub rejected: Vec<(String, RejectionReason)>,
}

impl ComposeOutcome {
    pub fn is_valid(&self) -> bool {
        self.rejected.is_empty()
    }
}

/// Validate a proposed stack of skill names against the index, emitting
/// selection evidence. Pure: no store writes, no installs. `active` is the
/// set of skills already loaded this session (for the idempotence flag).
/// Who is asking for the skill this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvokeKind {
    /// Model auto-select from the catalog.
    ModelAuto,
    /// User typed `/skill-name` or an equivalent explicit invoke.
    UserExplicit,
}

pub fn compose_stack(
    index: &SkillsIndexFile,
    stack: &[String],
    active: &[String],
    query: &str,
) -> ComposeOutcome {
    compose_stack_for(index, stack, active, query, InvokeKind::ModelAuto)
}

pub fn compose_stack_for(
    index: &SkillsIndexFile,
    stack: &[String],
    active: &[String],
    query: &str,
    invoke: InvokeKind,
) -> ComposeOutcome {
    let mut outcome = ComposeOutcome::default();
    let mut seen: Vec<&str> = Vec::new();
    for name in stack {
        if seen.contains(&name.as_str()) {
            outcome
                .rejected
                .push((name.clone(), RejectionReason::Duplicate));
            continue;
        }
        seen.push(name);
        let Some(entry) = index.get(name) else {
            outcome
                .rejected
                .push((name.clone(), RejectionReason::Unknown));
            continue;
        };
        if invoke == InvokeKind::ModelAuto && entry.disable_model_invocation {
            outcome
                .rejected
                .push((name.clone(), RejectionReason::ModelInvocationDisabled));
            continue;
        }
        // Conflict check: a selected skill whose tags collide with an
        // already-selected one is rejected (deterministic, first-wins).
        if let Some(other) = entry.tags.iter().find_map(|t| {
            outcome
                .selected
                .iter()
                .find(|s| s.name != *name && index.get(&s.name).is_some_and(|e| e.tags.contains(t)))
                .map(|s| s.name.clone())
        }) {
            outcome
                .rejected
                .push((name.clone(), RejectionReason::Conflict { with: other }));
            continue;
        }
        let score = relevance(entry, query);
        outcome.selected.push(SelectionEvidence {
            name: name.clone(),
            matched_on: matched_signal(entry, query),
            score,
            already_active: active.contains(name),
        });
    }
    outcome
}

/// Deterministic relevance in `[0,1]` — mirrors the SkillIndex scoring
/// weights (trigger > tool > name/description).
fn relevance(entry: &IndexEntry, query: &str) -> f64 {
    let q = query.to_lowercase();
    if q.is_empty() {
        return 0.0;
    }
    let terms: Vec<&str> = q.split_whitespace().collect();
    let mut hits: f64 = 0.0;
    let mut weight: f64 = 0.0;
    for term in terms {
        if entry.tags.iter().any(|t| t.to_lowercase().contains(term)) {
            hits += 1.0;
            weight += 1.0;
        } else if entry.description.to_lowercase().contains(term) {
            hits += 0.5;
            weight += 0.5;
        } else if entry.name.contains(term) {
            hits += 0.4;
            weight += 0.4;
        }
    }
    if weight == 0.0 {
        0.0
    } else {
        (hits / weight).clamp(0.0, 1.0)
    }
}

fn matched_signal(entry: &IndexEntry, query: &str) -> String {
    let q = query.to_lowercase();
    for term in q.split_whitespace() {
        if entry.tags.iter().any(|t| t.to_lowercase().contains(term)) {
            return format!("trigger `{term}`");
        }
    }
    for term in q.split_whitespace() {
        if entry.description.to_lowercase().contains(term) {
            return format!("description `{term}`");
        }
    }
    for term in q.split_whitespace() {
        if entry.name.contains(term) {
            return format!("name `{term}`");
        }
    }
    "explicit request".into()
}

/// P51.28 — model auto-catalog omits `disable-model-invocation` skills.
pub fn may_model_auto_invoke(m: &SkillManifest) -> bool {
    !m.disable_model_invocation
}

/// P51.28 — slash/picker listing.
/// Crush: `user-invocable: true`. Zed: `disable-model-invocation` skills
/// remain slash commands (the only way to run them).
pub fn may_user_slash_invoke(m: &SkillManifest) -> bool {
    m.user_invocable || m.disable_model_invocation
}

/// Warm-set lines for the model prompt. Disabled-model skills stay out.
pub fn model_warm_set(skills: &[Skill]) -> Vec<String> {
    skills
        .iter()
        .filter(|s| may_model_auto_invoke(&s.manifest))
        .map(|s| format!("{}: {}", s.manifest.name, s.manifest.description))
        .collect()
}

/// Names the user can invoke as `/skill-name`.
pub fn user_slash_catalog(skills: &[Skill]) -> Vec<String> {
    skills
        .iter()
        .filter(|s| may_user_slash_invoke(&s.manifest))
        .map(|s| s.manifest.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn index() -> SkillsIndexFile {
        SkillsIndexFile::new(vec![
            IndexEntry {
                name: "refactor-helper".into(),
                description: "Safe multi-file refactor discipline".into(),
                tags: vec!["refactor".into(), "rename".into()],
                author: "tester".into(),
                version: "1.0.0".into(),
                ..Default::default()
            },
            IndexEntry {
                name: "data-cleanup".into(),
                description: "spreadsheet cleanup".into(),
                tags: vec!["cleanup".into()],
                author: "a".into(),
                version: "1.0.0".into(),
                ..Default::default()
            },
        ])
    }

    #[test]
    fn compose_stack_validates_and_emits_evidence() {
        let idx = index();
        let out = compose_stack(
            &idx,
            &["refactor-helper".to_string(), "data-cleanup".to_string()],
            &[],
            "refactor this file",
        );
        assert!(out.is_valid());
        assert_eq!(out.selected.len(), 2);
        assert_eq!(out.selected[0].name, "refactor-helper");
        assert_eq!(out.selected[0].matched_on, "trigger `refactor`");
        assert!(out.selected[0].score > 0.5);
        assert!(!out.selected[0].already_active);
    }

    #[test]
    fn compose_rejects_unknown_duplicate_and_conflict() {
        let idx = index();
        // Unknown + duplicate.
        let out = compose_stack(
            &idx,
            &[
                "refactor-helper".into(),
                "nope".into(),
                "refactor-helper".into(),
            ],
            &[],
            "x",
        );
        assert!(!out.is_valid());
        assert!(
            out.rejected
                .contains(&("nope".into(), RejectionReason::Unknown))
        );
        assert!(
            out.rejected
                .contains(&("refactor-helper".into(), RejectionReason::Duplicate))
        );
        // Conflict: a skill sharing the `refactor` tag collides with the
        // already-selected one.
        let clash = SkillsIndexFile::new(vec![
            IndexEntry {
                name: "a".into(),
                description: "x".into(),
                tags: vec!["refactor".into()],
                author: "a".into(),
                version: "1".into(),
                ..Default::default()
            },
            IndexEntry {
                name: "b".into(),
                description: "y".into(),
                tags: vec!["refactor".into()],
                author: "a".into(),
                version: "1".into(),
                ..Default::default()
            },
        ]);
        let out = compose_stack(&clash, &["a".into(), "b".into()], &[], "refactor");
        assert!(
            out.rejected
                .contains(&("b".into(), RejectionReason::Conflict { with: "a".into() }))
        );
    }

    #[test]
    fn p51_disable_model_invocation_is_user_only() {
        let idx = SkillsIndexFile::new(vec![IndexEntry {
            name: "deploy".into(),
            description: "Deploy the branch".into(),
            disable_model_invocation: true,
            user_invocable: true,
            ..Default::default()
        }]);
        let auto = compose_stack(&idx, &["deploy".into()], &[], "deploy");
        assert!(
            auto.rejected
                .iter()
                .any(|(_, r)| { matches!(r, RejectionReason::ModelInvocationDisabled) })
        );
        let user = compose_stack_for(
            &idx,
            &["deploy".into()],
            &[],
            "deploy",
            InvokeKind::UserExplicit,
        );
        assert!(user.is_valid());
        assert_eq!(user.selected[0].name, "deploy");
        let m = SkillManifest {
            name: "deploy".into(),
            description: "d".into(),
            disable_model_invocation: true,
            user_invocable: true,
            author: "t".into(),
            created: "2026-09-18".into(),
            version: "1".into(),
            ..Default::default()
        };
        assert!(!may_model_auto_invoke(&m));
        assert!(may_user_slash_invoke(&m));
        // Zed: a deploy skill with only disable-model-invocation is still `/deploy`.
        let deploy_only = SkillManifest {
            name: "deploy".into(),
            disable_model_invocation: true,
            user_invocable: false,
            ..Default::default()
        };
        assert!(may_user_slash_invoke(&deploy_only));
        let auto_skill = Skill {
            manifest: SkillManifest {
                name: "notes".into(),
                description: "Take notes".into(),
                ..Default::default()
            },
            body: String::new(),
        };
        let hidden = Skill {
            manifest: deploy_only.clone(),
            body: String::new(),
        };
        let warm = model_warm_set(&[auto_skill.clone(), hidden.clone()]);
        assert_eq!(warm, vec!["notes: Take notes".to_string()]);
        assert_eq!(
            user_slash_catalog(&[auto_skill, hidden]),
            vec!["deploy".to_string()]
        );
    }

    #[test]
    fn index_file_write_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!("agentcowork-idx-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path: PathBuf = dir.join("skills_index.json");
        let idx = index();
        idx.write_to(&path).unwrap();
        let back = SkillsIndexFile::read_from(&path).unwrap();
        assert_eq!(back.skills.len(), 2);
        assert_eq!(back.version, 1);
        // Missing file → empty index, not an error.
        let missing = SkillsIndexFile::read_from(&dir.join("nope.json")).unwrap();
        assert!(missing.skills.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_skills_derives_index_rows() {
        let skills: Vec<Skill> = vec![Skill {
            manifest: crate::skill_store::SkillManifest {
                name: "refactor-helper".into(),
                description: "Safe multi-file refactor discipline".into(),
                tools: vec![],
                triggers: vec!["refactor".into()],
                when_to_use: vec![],
                scripts: vec![],
                references: vec![],
                assets: vec![],
                author: "tester".into(),
                created: "2026-08-20".into(),
                version: "1.0.0".into(),
                user_invocable: false,
                disable_model_invocation: false,
            },
            body: String::new(),
        }];
        let idx = SkillsIndexFile::from_skills(&skills);
        assert_eq!(idx.skills[0].name, "refactor-helper");
        assert!(idx.skills[0].tags.contains(&"refactor".to_string()));
    }
}
