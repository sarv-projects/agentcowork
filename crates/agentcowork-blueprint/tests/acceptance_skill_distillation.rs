//! Track 3 / P64.8 — Validated skill distillation acceptance suite.
//!
//! Evidence-gated readiness (§17.12.6): this drives the *public*
//! crystallization and `/learn` APIs, proving the self-evolution loop's
//! persistence half end-to-end:
//!
//! 1. **Classification** — a repeated non-cognitive trace (wait/trigger/
//!    transform/notify) becomes a crystallization candidate only after the
//!    threshold; a trace containing a cognitive step never does.
//! 2. **Compilation + drift** — a candidate compiles to a deterministic script
//!    with a recorded expected output; the decrystallize check returns Match on
//!    agreement and Drifted on divergence (the honest LLM fallback).
//! 3. **Validated storage** — a learned skill is written as a versioned
//!    `SKILL.md` bundle *only* after the sandbox gate passes; re-learning bumps
//!    the patch version, and a refusing gate blocks the save entirely.
//!
//! Cross-platform: runs on every host. The live-model DAG-trace soak remains
//! open — see TODO P64.8.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use agentcowork_blueprint::{
    Drift, LearnGate, LearnRequest, ScriptLanguage, Skill, SkillStore, StepClass, WorkflowDetector,
    WorkflowStep, compile_to_script, decrystallize_check, derive_name, learn_and_save,
};

fn step(tool: &str, args: &str, class: StepClass) -> WorkflowStep {
    WorkflowStep {
        tool: tool.into(),
        args: args.into(),
        class,
    }
}

/// A deterministic non-cognitive workflow trace.
fn noncognitive_trace() -> Vec<WorkflowStep> {
    vec![
        step("sleep", "{\"secs\":5}", StepClass::Wait),
        step("on_event", "{\"topic\":\"build.done\"}", StepClass::Trigger),
        step("transform", "{\"rows\":120}", StepClass::Transform),
        step("notify", "{\"channel\":\"#ops\"}", StepClass::Notify),
    ]
}

fn temp_store(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "agentcowork-skills-acceptance-{tag}-{}-{nanos}",
        std::process::id()
    ))
}

struct AllowGate;
impl LearnGate for AllowGate {
    fn verify(&self, _skill: &Skill, _evidence_sha256: &str) -> Result<(), String> {
        Ok(())
    }
}

struct DenyGate;
impl LearnGate for DenyGate {
    fn verify(&self, _skill: &Skill, _evidence_sha256: &str) -> Result<(), String> {
        Err("sandbox refused the blueprint".into())
    }
}

#[test]
fn detector_crystallizes_only_repeated_noncognitive_dependencies() {
    let mut detector = WorkflowDetector::new(3);

    // First two successes are below the threshold; the third crosses it.
    assert!(!detector.record_success(noncognitive_trace()));
    assert!(!detector.record_success(noncognitive_trace()));
    assert!(detector.record_success(noncognitive_trace()));
    assert_eq!(detector.candidates().len(), 1);

    // A trace with a cognitive step must never become a candidate, no matter
    // how often it repeats.
    let mut cognitive = WorkflowDetector::new(2);
    let mut trace = noncognitive_trace();
    trace.push(step("reason", "{\"prompt\":\"why\"}", StepClass::Cognitive));
    assert!(!cognitive.record_success(trace.clone()));
    assert!(cognitive.record_success(trace));
    assert!(
        cognitive.candidates().is_empty(),
        "a cognitive step disqualifies the workflow"
    );
}

#[test]
fn compilation_records_a_contract_and_drift_is_detected() {
    let mut detector = WorkflowDetector::new(1);
    assert!(detector.record_success(noncognitive_trace()));
    let workflow = detector.candidates()[0];

    let compiled = compile_to_script("deploy-check", workflow, ScriptLanguage::Ts);
    assert_eq!(compiled.name, "deploy-check");
    assert!(
        !compiled.source.is_empty(),
        "a deterministic script is emitted"
    );
    assert!(compiled.source.contains("crystallized skill: deploy-check"));
    // The expected output is the transform step's args — the drift contract.
    assert!(
        compiled.expected_output.contains("rows"),
        "expected_output records the transform: {:?}",
        compiled.expected_output
    );

    // Agreement ⇒ safe; divergence ⇒ decrystallize (LLM fallback).
    assert_eq!(
        decrystallize_check(&compiled, &compiled.expected_output),
        Drift::Match
    );
    assert_eq!(
        decrystallize_check(&compiled, "something else"),
        Drift::Drifted
    );
}

#[test]
fn learn_and_save_writes_a_versioned_skill_bundle_behind_the_gate() {
    let root = temp_store("bundle");
    let store = SkillStore::new(&root);

    let req = LearnRequest {
        evidence: "# Deploy Checklist\n\nRun the migration, then restart the worker pool, then verify health.\n"
            .into(),
        title: Some("Deploy Checklist".into()),
        author: "acceptance".into(),
        name: None,
        tools: vec!["script.run".into()],
    };
    let slug = derive_name(&req);
    assert_eq!(slug, "deploy-checklist", "deterministic slug");

    // Gate passes → the bundle is written.
    let path = learn_and_save(&store, &req, &AllowGate).expect("learn_and_save");
    assert_eq!(
        path.file_name().unwrap().to_string_lossy(),
        "SKILL.md",
        "path: {}",
        path.display()
    );
    assert_eq!(
        path.parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy(),
        slug,
        "the skill lives in a directory named after its slug"
    );
    assert!(path.is_file(), "SKILL.md must exist on disk");

    // It parses back into a well-formed, valid skill manifest.
    let saved = store.load(&slug).expect("load the saved skill");
    assert_eq!(saved.manifest.name, slug);
    assert_eq!(saved.manifest.version, "0.1.0");
    assert_eq!(saved.manifest.author, "acceptance");
    assert!(saved.manifest.description.len() >= 12);
    assert_eq!(saved.manifest.tools, vec!["script.run".to_string()]);
    // Round-trips through the canonical SKILL.md text.
    let md = saved.to_skill_md();
    assert!(md.starts_with("---\n"), "frontmatter present");
    assert!(md.contains("name: deploy-checklist"));
    let reparsed = Skill::from_skill_md(&md, "mem").expect("reparse SKILL.md");
    assert_eq!(reparsed.manifest.name, saved.manifest.name);
    assert_eq!(reparsed.manifest.version, saved.manifest.version);

    // Re-learning the same evidence bumps the patch version (never a silent
    // overwrite) and stays discoverable via a store scan.
    let _ = learn_and_save(&store, &req, &AllowGate).expect("re-learn");
    let bumped = store.load(&slug).expect("load after re-learn");
    assert_eq!(bumped.manifest.version, "0.1.1", "re-learn bumps the patch");
    assert_eq!(store.scan().expect("scan").len(), 1);

    // The test/sandbox gate is never bypassed: a refusing gate blocks the save
    // and leaves the existing version untouched.
    let denied = learn_and_save(&store, &req, &DenyGate);
    assert!(denied.is_err(), "a refusing gate must block the save");
    assert_eq!(
        store.load(&slug).unwrap().manifest.version,
        "0.1.1",
        "a blocked learn must not write a new version"
    );

    let _ = std::fs::remove_dir_all(&root);
}
