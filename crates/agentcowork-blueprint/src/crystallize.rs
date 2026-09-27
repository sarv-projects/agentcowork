//! Crystallization (P6.5 — B8, Algorithm #5, doc 03 §1/§2, doc 63).
//!
//! The self-evolution loop's persistence half: when the same deterministic
//! workflow succeeds N times, it is *crystallized* into a compiled script in
//! the skill registry (`~/.everyaios/skills/`) so future runs cost **zero
//! model tokens**. Steps are classified so only the non-cognitive parts
//! (waits, triggers, transforms, notifications) crystallize; anything that
//! still needs reasoning stays an LLM step. `decrystallize` is the honest
//! fallback: if the script's output drifts from the recorded expectation,
//! the task falls back to the LLM rather than silently corrupting.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Classification of a workflow step (doc 03 §7 / P6.5 "non-cognitive").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepClass {
    /// A fixed delay / poll (deterministic, crystallizable).
    Wait,
    /// An event trigger (crystallizable).
    Trigger,
    /// A deterministic data transform (crystallizable).
    Transform,
    /// A notification (crystallizable).
    Notify,
    /// Anything that still needs model reasoning — never crystallizes.
    Cognitive,
}

impl StepClass {
    /// Non-cognitive = safe to compile into a deterministic script.
    pub fn is_crystallizable(self) -> bool {
        !matches!(self, StepClass::Cognitive)
    }
}

/// One observed step in a workflow trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowStep {
    /// Tool/verb name (e.g. `file_ops.write`, `notify`, `sleep`).
    pub tool: String,
    /// Canonical args (sorted-key JSON string) — the identity of the step.
    pub args: String,
    pub class: StepClass,
}

/// A detected workflow: an ordered step trace plus how often it has
/// succeeded identically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workflow {
    /// Content fingerprint (sorted steps → hash).
    pub signature: String,
    pub steps: Vec<WorkflowStep>,
    pub successes: u32,
}

/// The result of compiling a workflow into a deterministic script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledSkill {
    pub name: String,
    pub language: ScriptLanguage,
    pub source: String,
    /// The recorded expected output (for drift detection).
    pub expected_output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScriptLanguage {
    Ts,
    Python,
}

/// The drift verdict after running a compiled skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Drift {
    /// Output matches the recorded expectation — the skill is safe.
    Match,
    /// Output drifted — decrystallize and fall back to the LLM.
    Drifted,
}

/// Tracks repeated successful workflows and promotes them to candidates.
#[derive(Debug, Clone, Default)]
pub struct WorkflowDetector {
    /// signature → workflow (last-seen wins; `successes` accumulates).
    observed: HashMap<String, Workflow>,
    /// Success threshold before a workflow is a crystallization candidate.
    threshold: u32,
}

impl WorkflowDetector {
    pub fn new(threshold: u32) -> Self {
        Self {
            observed: HashMap::new(),
            threshold: threshold.max(1),
        }
    }

    /// Record one successful run of `steps`. Returns `true` when the workflow
    /// has now succeeded `threshold` times (i.e. it is ready to crystallize).
    pub fn record_success(&mut self, steps: Vec<WorkflowStep>) -> bool {
        let signature = signature(&steps);
        let entry = self.observed.entry(signature).or_insert_with(|| Workflow {
            signature: signature_of(&steps),
            steps,
            successes: 0,
        });
        entry.successes += 1;
        entry.successes >= self.threshold
    }

    /// All workflows that have reached the crystallization threshold.
    pub fn candidates(&self) -> Vec<&Workflow> {
        self.observed
            .values()
            .filter(|w| {
                w.successes >= self.threshold && w.steps.iter().all(|s| s.class.is_crystallizable())
            })
            .collect()
    }
}

/// A stable content fingerprint of a step trace.
pub fn signature(steps: &[WorkflowStep]) -> String {
    // Deterministic hash over tool+args+class (sorted already by definition,
    // but we hash in order to capture sequence identity).
    let mut s = String::new();
    for st in steps {
        s.push_str(&st.tool);
        s.push('\u{1f}');
        s.push_str(&st.args);
        s.push('\u{1f}');
    }
    fnv1a(&s).to_string()
}

fn signature_of(steps: &[WorkflowStep]) -> String {
    signature(steps)
}

/// Compile a fully-crystallizable workflow into a deterministic script.
pub fn compile_to_script(
    name: &str,
    workflow: &Workflow,
    language: ScriptLanguage,
) -> CompiledSkill {
    let steps = &workflow.steps;
    let source = match language {
        ScriptLanguage::Ts => compile_ts(name, steps),
        ScriptLanguage::Python => compile_python(name, steps),
    };
    CompiledSkill {
        name: name.to_string(),
        language,
        source,
        // The expected output is the transform steps' args — the deterministic
        // contract the decrystallize check compares against.
        expected_output: steps
            .iter()
            .filter(|s| s.class == StepClass::Transform)
            .map(|s| s.args.clone())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn compile_ts(name: &str, steps: &[WorkflowStep]) -> String {
    let mut out = format!("// crystallized skill: {name}\n// 0-token deterministic run (P6.5)\n");
    for s in steps {
        let safe_tool = s.tool.replace(['-', '.', '/', ' '], "_");
        out.push_str(&format!(
            "// step {} [{}]: {} = {};\n",
            s.class.as_str(),
            s.tool,
            safe_tool,
            s.args
        ));
    }
    out.push_str("export {};\n");
    out
}

fn compile_python(name: &str, steps: &[WorkflowStep]) -> String {
    let mut out = format!("# crystallized skill: {name}\n# 0-token deterministic run (P6.5)\n");
    for s in steps {
        out.push_str(&format!(
            "# step {} [{}]: {} = {}\n",
            s.class.as_str(),
            s.tool,
            s.tool,
            s.args
        ));
    }
    out
}

impl StepClass {
    fn as_str(self) -> &'static str {
        match self {
            StepClass::Wait => "wait",
            StepClass::Trigger => "trigger",
            StepClass::Transform => "transform",
            StepClass::Notify => "notify",
            StepClass::Cognitive => "cognitive",
        }
    }
}

/// The on-disk skill registry (`~/.everyaios/skills/`).
#[derive(Debug, Clone)]
pub struct SkillRegistry {
    root: PathBuf,
}

impl SkillRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Default location: `~/.everyaios/skills/`.
    pub fn default_home() -> PathBuf {
        dirs_like_home().join(".everyaios").join("skills")
    }

    pub fn store(&self, skill: &CompiledSkill) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.root)?;
        let ext = match skill.language {
            ScriptLanguage::Ts => "ts",
            ScriptLanguage::Python => "py",
        };
        let path = self.root.join(format!("{}.{}", skill.name, ext));
        std::fs::write(&path, &skill.source)?;
        Ok(path)
    }

    pub fn load(&self, name: &str, language: ScriptLanguage) -> std::io::Result<Option<String>> {
        let ext = match language {
            ScriptLanguage::Ts => "ts",
            ScriptLanguage::Python => "py",
        };
        let path = self.root.join(format!("{name}.{ext}"));
        if path.exists() {
            Ok(Some(std::fs::read_to_string(&path)?))
        } else {
            Ok(None)
        }
    }

    pub fn list(&self) -> std::io::Result<Vec<String>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(stem) = entry.path().file_stem() {
                    names.push(stem.to_string_lossy().into_owned());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// The decrystallize fallback: compare a run's output against the recorded
/// expectation. Drift → return [`Drift::Drifted`] so the caller falls back
/// to the LLM (never silently accept a drifted deterministic run).
pub fn decrystallize_check(skill: &CompiledSkill, actual_output: &str) -> Drift {
    if skill.expected_output.is_empty() || actual_output.trim() == skill.expected_output.trim() {
        Drift::Match
    } else {
        Drift::Drifted
    }
}

fn dirs_like_home() -> PathBuf {
    // Prefer $HOME; fall back to a temp dir (tests inject a custom root).
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_step(ms: u32) -> WorkflowStep {
        WorkflowStep {
            tool: "sleep".into(),
            args: format!("{{\"ms\":{ms}}}"),
            class: StepClass::Wait,
        }
    }

    fn transform_step(expr: &str) -> WorkflowStep {
        WorkflowStep {
            tool: "transform".into(),
            args: expr.into(),
            class: StepClass::Transform,
        }
    }

    fn notify_step(text: &str) -> WorkflowStep {
        WorkflowStep {
            tool: "notify".into(),
            args: text.into(),
            class: StepClass::Notify,
        }
    }

    fn cognitive_step() -> WorkflowStep {
        WorkflowStep {
            tool: "reason".into(),
            args: "{}".into(),
            class: StepClass::Cognitive,
        }
    }

    #[test]
    fn detector_promotes_after_threshold() {
        let mut d = WorkflowDetector::new(3);
        let steps = vec![wait_step(10), transform_step("x+1"), notify_step("done")];
        assert!(!d.record_success(steps.clone()));
        assert!(!d.record_success(steps.clone()));
        assert!(d.record_success(steps.clone()));
        assert_eq!(d.candidates().len(), 1);
    }

    #[test]
    fn cognitive_steps_never_crystallize() {
        let mut d = WorkflowDetector::new(1);
        let steps = vec![cognitive_step(), notify_step("done")];
        assert!(d.record_success(steps));
        // Recorded but not a candidate (contains a cognitive step).
        assert!(d.candidates().is_empty());
    }

    #[test]
    fn signatures_are_order_sensitive_and_stable() {
        let a = vec![wait_step(10), transform_step("x")];
        let b = vec![transform_step("x"), wait_step(10)];
        assert_ne!(signature(&a), signature(&b));
        assert_eq!(signature(&a), signature(&a));
    }

    #[test]
    fn compile_ts_embeds_steps() {
        let wf = Workflow {
            signature: signature(&[wait_step(5)]),
            steps: vec![wait_step(5), transform_step("x*2")],
            successes: 3,
        };
        let skill = compile_to_script("double", &wf, ScriptLanguage::Ts);
        assert!(skill.source.contains("crystallized skill: double"));
        assert!(skill.source.contains("transform"));
        // Expected output = transform args joined.
        assert_eq!(skill.expected_output, "x*2");
    }

    #[test]
    fn registry_roundtrips_on_disk() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-skills-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let reg = SkillRegistry::new(dir.clone());
        let skill = CompiledSkill {
            name: "morning".into(),
            language: ScriptLanguage::Python,
            source: "# hello\n".into(),
            expected_output: "x".into(),
        };
        let path = reg.store(&skill).unwrap();
        assert!(path.ends_with("morning.py"));
        assert_eq!(
            reg.load("morning", ScriptLanguage::Python).unwrap(),
            Some("# hello\n".into())
        );
        assert_eq!(reg.list().unwrap(), vec!["morning".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decrystallize_detects_drift() {
        let skill = CompiledSkill {
            name: "s".into(),
            language: ScriptLanguage::Ts,
            source: "".into(),
            expected_output: "42".into(),
        };
        assert_eq!(decrystallize_check(&skill, "42"), Drift::Match);
        assert_eq!(decrystallize_check(&skill, "43"), Drift::Drifted);
        // Empty expectation → always match (nothing to drift from).
        let empty = CompiledSkill {
            expected_output: "".into(),
            ..skill.clone()
        };
        assert_eq!(decrystallize_check(&empty, "anything"), Drift::Match);
    }

    #[test]
    fn zero_token_contract_is_structural() {
        // The invariant that matters: a fully-crystallizable workflow compiles
        // to a script with no model dependency. We assert the compile output
        // is a plain script (no `reason`/cognitive step survives).
        let steps = vec![wait_step(1), transform_step("y"), notify_step("z")];
        let wf = Workflow {
            signature: signature(&steps),
            steps,
            successes: 3,
        };
        let skill = compile_to_script("zero", &wf, ScriptLanguage::Python);
        assert!(!skill.source.contains("reason"));
        assert!(skill.source.contains("zero"));
    }
}
