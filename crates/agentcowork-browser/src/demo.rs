//! K3 half-1 — demonstration recording (doc 81 §3.1): capture a
//! demonstration as a sequence of steps, each anchored to the a11y tree
//! (role + name + stable ref) with the input performed and the outcome
//! evidence observed. Starts early (feeds E2/E5/E9 + ADD-1); the compile
//! half (K3 half-2, Gate D) replays these steps deterministically with zero
//! model tokens.
//!
//! This module owns the *recording contract* — anchors, inputs, outcome
//! evidence — and the deterministic anchors-from-tree extraction. The live
//! recorder script (which captures DOM events) lives in [`crate::replay`].
//!
//! The second half (P21-1, OpenAdapt STEAL): a **demonstration compiler** —
//! record → compile to a deterministic [`ReplayProgram`] (action list +
//! element selectors + verify-assertions) → zero-model healthy replay →
//! governed repair (model invoked only on interface drift) →
//! **halt-instead-of-guess**.

use crate::A11yNode;
use serde::{Deserialize, Serialize};

/// A stable anchor: what the step targeted, in accessible terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoAnchor {
    pub role: String,
    pub name: String,
    /// The stable a11y ref (`eN`) captured at record time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,
    /// A fallback CSS/XPath (G8) so replay survives minor DOM drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_selector: Option<String>,
}

/// The input performed on the anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoInput {
    /// `click` / `type` / `select` / `key`.
    pub action: String,
    /// The typed value / selected option (empty for clicks).
    #[serde(default)]
    pub value: String,
}

/// The outcome evidence captured after the input — what proved the step
/// worked (the compiled replay asserts these, not the model's claim).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeEvidence {
    /// e.g. `a11y_snapshot`, `url`, `element_present`, `content_contains`.
    pub kind: String,
    /// The observed value (URL fragment, element name, text snippet).
    pub observed: String,
}

/// One recorded demonstration step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoStep {
    pub index: u32,
    pub anchor: DemoAnchor,
    pub input: DemoInput,
    /// Evidence observed after the input — the replay's assertion target.
    pub outcome: OutcomeEvidence,
}

/// A full demonstration recording.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoRecording {
    pub name: String,
    /// The URL the demo started on (the replay's entry point).
    pub start_url: String,
    pub steps: Vec<DemoStep>,
}

impl DemoRecording {
    pub fn new(name: impl Into<String>, start_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            start_url: start_url.into(),
            steps: Vec::new(),
        }
    }

    /// Append a step (indices auto-assigned, deterministic).
    pub fn push(&mut self, anchor: DemoAnchor, input: DemoInput, outcome: OutcomeEvidence) {
        let index = self.steps.len() as u32;
        self.steps.push(DemoStep {
            index,
            anchor,
            input,
            outcome,
        });
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Extract a deterministic anchor from an a11y node (the recording side of
/// the G8 selector resolver — same vocabulary, different direction).
pub fn anchor_from_node(node: &A11yNode) -> DemoAnchor {
    DemoAnchor {
        role: node.role.clone(),
        name: node.name.clone(),
        ref_id: node.ref_id.clone(),
        fallback_selector: None,
    }
}

/// Whether a recorded step's outcome evidence still holds against a fresh
/// snapshot — the deterministic replay assertion (a node with the same
/// role+name exists, a URL contains the fragment, text is present).
pub fn outcome_holds(evidence: &OutcomeEvidence, root: &A11yNode, current_url: &str) -> bool {
    match evidence.kind.as_str() {
        "url" => current_url.contains(&evidence.observed),
        "element_present" => find_by_name(root, &evidence.observed).is_some(),
        "content_contains" => tree_text(root).contains(&evidence.observed),
        _ => false, // unknown evidence kinds never silently pass
    }
}

fn find_by_name<'a>(node: &'a A11yNode, name: &str) -> Option<&'a A11yNode> {
    if node.name == name {
        return Some(node);
    }
    node.children.iter().find_map(|c| find_by_name(c, name))
}

fn tree_text(node: &A11yNode) -> String {
    let mut out = node.name.clone();
    for c in &node.children {
        out.push(' ');
        out.push_str(&tree_text(c));
    }
    out
}

// ---------------------------------------------------------------------------
// P21-1 — the demonstration compiler (OpenAdapt STEAL, doc 73 §1): record →
// compile to a deterministic replay program (action list + element selectors
// + verify-assertions) → zero-model healthy path → governed repair → halt-
// instead-of-guess. This is the compile half (K3 half-2) of the
// record/compile split; the recording contract lives above.
// ---------------------------------------------------------------------------

/// The compiled action the replay performs (a stable subset of the live
/// [`crate::actions::ActKind`] vocabulary — clicks, typing, key presses,
/// selects).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum ReplayAct {
    Click,
    Type { value: String },
    PressKey { key: String },
    Select { value: String },
}

impl ReplayAct {
    /// Lower a replay action onto the live browser `ActKind`, resolving the
    /// anchor against the fresh snapshot (ref-id first, then role+name).
    /// Returns `None` when the anchor cannot be found — the runner treats
    /// that as drift, never a guess.
    pub fn to_act_kind(
        &self,
        anchor: &DemoAnchor,
        root: &A11yNode,
    ) -> Option<crate::actions::ActKind> {
        let node = find_ref(root, anchor.ref_id.as_deref()?).or_else(|| {
            let located = crate::locator::find_first(
                root,
                &crate::locator::SemanticQuery {
                    role: Some(anchor.role.clone()),
                    name: Some(anchor.name.clone()),
                },
            )?;
            find_ref(root, located.ref_id.as_deref()?)
        });
        let ref_id = node.and_then(|n| n.ref_id.clone())?;
        Some(match self {
            ReplayAct::Click => crate::actions::ActKind::Click { ref_id },
            ReplayAct::Type { value } => crate::actions::ActKind::Type {
                ref_id,
                text: value.clone(),
            },
            ReplayAct::PressKey { key } => crate::actions::ActKind::Press { key: key.clone() },
            ReplayAct::Select { value } => crate::actions::ActKind::Select {
                ref_id,
                value: value.clone(),
            },
        })
    }
}

fn find_ref<'a>(node: &'a A11yNode, ref_id: &str) -> Option<&'a A11yNode> {
    if node.ref_id.as_deref() == Some(ref_id) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find_ref(c, ref_id))
}

/// One compiled replay step: anchor (selector) + deterministic act + the
/// verify-assertions (from the recorded outcome evidence).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayStep {
    pub index: u32,
    pub anchor: DemoAnchor,
    pub act: ReplayAct,
    pub verify: Vec<OutcomeEvidence>,
}

/// The compiled program — the deterministic artifact crystallization
/// produces (zero model calls on the healthy path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayProgram {
    pub id: String,
    pub name: String,
    pub start_url: String,
    pub steps: Vec<ReplayStep>,
}

/// Compile a recording into a replay program. Refuses: empty recordings,
/// unknown actions (the deterministic surface is fixed), and steps without
/// anchors or evidence. Pure — no browser, no model.
pub fn compile(recording: &DemoRecording) -> Result<ReplayProgram, CompileError> {
    if recording.is_empty() {
        return Err(CompileError::Empty);
    }
    if recording.start_url.trim().is_empty() {
        return Err(CompileError::NoStartUrl);
    }
    let mut steps = Vec::with_capacity(recording.steps.len());
    for step in &recording.steps {
        if step.anchor.role.is_empty() && step.anchor.name.is_empty() {
            return Err(CompileError::UnanchoredStep(step.index));
        }
        let act = match step.input.action.as_str() {
            "click" => ReplayAct::Click,
            "type" => ReplayAct::Type {
                value: step.input.value.clone(),
            },
            "key" | "press" => ReplayAct::PressKey {
                key: step.input.value.clone(),
            },
            "select" => ReplayAct::Select {
                value: step.input.value.clone(),
            },
            other => return Err(CompileError::UnknownAction(other.to_string())),
        };
        if step.outcome.kind.is_empty() {
            return Err(CompileError::NoEvidence(step.index));
        }
        steps.push(ReplayStep {
            index: step.index,
            anchor: step.anchor.clone(),
            act,
            verify: vec![step.outcome.clone()],
        });
    }
    Ok(ReplayProgram {
        id: format!("demo-{}", slug(&recording.name)),
        name: recording.name.clone(),
        start_url: recording.start_url.clone(),
        steps,
    })
}

fn slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Compilation errors (fail-closed: a recording that can't become a
/// deterministic program is refused, not approximated).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompileError {
    #[error("recording is empty")]
    Empty,
    #[error("recording has no start url")]
    NoStartUrl,
    #[error("step {0} has no anchor")]
    UnanchoredStep(u32),
    #[error("unknown replay action `{0}`")]
    UnknownAction(String),
    #[error("step {0} has no outcome evidence")]
    NoEvidence(u32),
}

/// Runner state (the halt-instead-of-guess discipline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplayState {
    /// Replaying step index.
    Running,
    /// A step drifted; a governed repair is offered (once per step).
    Repairing { step: usize },
    /// Repair failed (or refused) and the result cannot be verified —
    /// terminal. Never a fabricated completion.
    Halted { step: usize, reason: String },
    /// Every step verified.
    Finished,
}

/// The deterministic replay runner. The caller supplies fresh snapshots for
/// each step; the runner verifies, advances, and gates repair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayRunner {
    pub program: ReplayProgram,
    pub state: ReplayState,
    /// Model calls consumed by governed repairs (healthy runs stay 0).
    pub model_calls: u64,
    /// The next step to check (monotone cursor).
    cursor: usize,
    repair_budget: u32,
    repairs_used: u32,
}

impl ReplayRunner {
    pub fn new(program: ReplayProgram, repair_budget: u32) -> Self {
        Self {
            program,
            state: ReplayState::Running,
            model_calls: 0,
            cursor: 0,
            repair_budget: repair_budget.max(1),
            repairs_used: 0,
        }
    }

    pub fn current_step(&self) -> Option<usize> {
        match self.state {
            ReplayState::Running | ReplayState::Repairing { .. } => Some(self.cursor),
            ReplayState::Halted { step, .. } => Some(step),
            ReplayState::Finished => None,
        }
    }

    /// Advance to the next step to verify (the caller re-snapshots on it).
    pub fn next_pending(&self) -> Option<usize> {
        match self.state {
            ReplayState::Running if self.cursor < self.program.steps.len() => Some(self.cursor),
            _ => None,
        }
    }

    /// Verify the current step's evidence against the fresh snapshot and
    /// advance. Healthy path: zero model calls. Drift → Repairing.
    pub fn verify_and_advance(&mut self, root: &A11yNode, url: &str) -> bool {
        match &self.state {
            ReplayState::Running | ReplayState::Repairing { .. } => {}
            _ => return false,
        }
        let idx = self.cursor;
        let step = match self.program.steps.get(idx) {
            Some(s) => s,
            None => {
                self.state = ReplayState::Finished;
                return true;
            }
        };
        let holds = step.verify.iter().all(|ev| outcome_holds(ev, root, url));
        if holds {
            self.cursor += 1;
            if self.cursor >= self.program.steps.len() {
                self.state = ReplayState::Finished;
            }
            true
        } else {
            self.state = ReplayState::Repairing { step: idx };
            false
        }
    }

    /// Governed repair gate: called after a drift. The repair is offered
    /// only inside the repair budget; exhaustion → Halt. Returns the current
    /// step index when repair is allowed.
    pub fn request_repair(&mut self, drift_evidence: &str) -> Option<usize> {
        if let ReplayState::Repairing { step } = self.state {
            if self.repairs_used >= self.repair_budget {
                self.state = ReplayState::Halted {
                    step,
                    reason: format!("drift remains after repair budget: {drift_evidence}"),
                };
                return None;
            }
            self.repairs_used += 1;
            self.model_calls += 1; // governed repair = one model invocation
            Some(step)
        } else {
            None
        }
    }

    /// Apply the repaired anchor (the model's ONLY output in the loop).
    pub fn apply_repair(&mut self, step: usize, anchor: DemoAnchor) -> bool {
        match self.state {
            ReplayState::Repairing { step: s } if s == step => {
                if let Some(target) = self.program.steps.get_mut(step) {
                    target.anchor = anchor;
                    self.state = ReplayState::Running;
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// Halt at the current step (circuit-break / user stop).
    pub fn halt(&mut self, reason: &str) {
        let step = self.cursor;
        self.state = ReplayState::Halted {
            step,
            reason: reason.to_string(),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording() -> DemoRecording {
        let mut rec = DemoRecording::new("login", "https://example.com/login");
        rec.push(
            DemoAnchor {
                role: "textbox".into(),
                name: "Username".into(),
                ref_id: Some("e2".into()),
                fallback_selector: None,
            },
            DemoInput {
                action: "type".into(),
                value: "alice".into(),
            },
            OutcomeEvidence {
                kind: "content_contains".into(),
                observed: "alice".into(),
            },
        );
        rec.push(
            DemoAnchor {
                role: "button".into(),
                name: "Sign in".into(),
                ref_id: Some("e7".into()),
                fallback_selector: None,
            },
            DemoInput {
                action: "click".into(),
                value: String::new(),
            },
            OutcomeEvidence {
                kind: "url".into(),
                observed: "/dashboard".into(),
            },
        );
        rec
    }

    fn tree(after_type: bool) -> A11yNode {
        let mut root = A11yNode::new("document", "Page");
        let boxed = A11yNode::new("textbox", if after_type { "alice" } else { "Username" })
            .with_ref("e2")
            .with_actionable();
        root.push(boxed);
        root.push(
            A11yNode::new("button", "Sign in")
                .with_ref("e7")
                .with_actionable(),
        );
        root
    }

    #[test]
    fn compile_rejects_invalid_recordings() {
        let empty = DemoRecording::new("x", "https://e.com");
        assert_eq!(compile(&empty), Err(CompileError::Empty));
        let mut no_url = recording();
        no_url.start_url = "".into();
        assert_eq!(compile(&no_url), Err(CompileError::NoStartUrl));
        let mut bad_act = recording();
        bad_act.steps[0].input.action = "drag".into();
        assert_eq!(
            compile(&bad_act),
            Err(CompileError::UnknownAction("drag".into()))
        );
        let mut no_anchor = recording();
        no_anchor.steps[0].anchor = DemoAnchor {
            role: "".into(),
            name: "".into(),
            ref_id: None,
            fallback_selector: None,
        };
        assert_eq!(compile(&no_anchor), Err(CompileError::UnanchoredStep(0)));
    }

    #[test]
    fn compile_produces_deterministic_program() {
        let program = compile(&recording()).unwrap();
        assert_eq!(program.steps.len(), 2);
        assert_eq!(
            program.steps[0].act,
            ReplayAct::Type {
                value: "alice".into()
            }
        );
        assert_eq!(program.steps[1].verify.len(), 1);
        assert_eq!(program.steps[1].verify[0].kind, "url");
        assert!(program.id.starts_with("demo-"));
    }

    #[test]
    fn healthy_replay_is_zero_model() {
        let program = compile(&recording()).unwrap();
        let mut runner = ReplayRunner::new(program, 1);
        // step 0: type into username → content_contains "alice"
        assert!(runner.verify_and_advance(&tree(true), "https://example.com/login"));
        assert_eq!(runner.model_calls, 0);
        // step 1: click sign in → url contains /dashboard
        assert!(runner.verify_and_advance(&tree(true), "https://example.com/dashboard"));
        assert_eq!(runner.state, ReplayState::Finished);
        assert_eq!(runner.model_calls, 0);
    }

    #[test]
    fn drift_gates_repair_and_recovery() {
        let program = compile(&recording()).unwrap();
        let mut runner = ReplayRunner::new(program, 3);
        // drift: "alice" not visible yet
        assert!(!runner.verify_and_advance(&tree(false), "https://example.com/login"));
        assert_eq!(runner.state, ReplayState::Repairing { step: 0 });
        let step = runner.request_repair("textbox value absent").unwrap();
        assert_eq!(step, 0);
        assert_eq!(runner.model_calls, 1);
        // repaired anchor (same role, fresh ref)
        assert!(runner.apply_repair(
            0,
            DemoAnchor {
                role: "textbox".into(),
                name: "Username".into(),
                ref_id: Some("e2".into()),
                fallback_selector: None
            }
        ));
        // now verify again with the text present
        assert!(runner.verify_and_advance(&tree(true), "https://example.com/login"));
        assert!(runner.verify_and_advance(&tree(true), "https://example.com/dashboard"));
        assert_eq!(runner.state, ReplayState::Finished);
        assert_eq!(runner.model_calls, 1);
    }

    #[test]
    fn second_drift_halts_instead_of_guessing() {
        let program = compile(&recording()).unwrap();
        let mut runner = ReplayRunner::new(program, 1);
        assert!(!runner.verify_and_advance(&tree(false), "https://example.com/login"));
        assert!(runner.request_repair("text absent").is_some());
        // second drift on the same step (repair didn't fix it)
        assert!(!runner.verify_and_advance(&tree(false), "https://example.com/login"));
        assert_eq!(runner.state, ReplayState::Repairing { step: 0 });
        assert!(runner.request_repair("still absent").is_none());
        assert!(matches!(runner.state, ReplayState::Halted { .. }));
        assert_eq!(runner.model_calls, 1); // budget 1 → no third call
    }

    #[test]
    fn replay_act_lowers_onto_live_act_kind() {
        let program = compile(&recording()).unwrap();
        let tree = tree(true);
        let kind = program.steps[0]
            .act
            .to_act_kind(&program.steps[0].anchor, &tree);
        assert!(
            matches!(kind, Some(crate::actions::ActKind::Type { ref_id, text }) if ref_id == "e2" && text == "alice")
        );
        let click = program.steps[1]
            .act
            .to_act_kind(&program.steps[1].anchor, &tree);
        assert!(matches!(click, Some(crate::actions::ActKind::Click { ref_id }) if ref_id == "e7"));
        // unresolved anchor → None (drift, never a guess)
        let missing = DemoAnchor {
            role: "button".into(),
            name: "Logout".into(),
            ref_id: None,
            fallback_selector: None,
        };
        assert!(program.steps[1].act.to_act_kind(&missing, &tree).is_none());
    }
}
