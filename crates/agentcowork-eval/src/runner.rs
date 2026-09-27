//! Sandbox runner (P8.0 runtime half — doc 63 §2.3 pipeline: provision
//! identical snapshot → agent → fault injector → independent verifier →
//! immutable result bundle → reset snapshot). The agent's own "finished" is
//! never trusted: `RunOutcome` carries the verifier's `VerificationReport`,
//! not the agent's final text.
//!
//! This is the *executor* half of the eval subsystem. `suite`/`verifier`/
//! `evidence` define and check; `runner` actually runs a task end-to-end in
//! a disposable workspace.

use crate::evidence::{ApprovalEvent, ArtifactHash, EvidenceBundle};
use crate::manifest::{HashAlgorithm, OutcomeCheck, TaskManifest};
use crate::suite::{AdversarialTask, FaultInjection};
use crate::verifier::{VerificationReport, verify_with_policy};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::io;
use std::path::{Path, PathBuf};

/// The seam the desktop harness implements: run the agent against a workspace
/// and return the permission-trace violations it detected (an empty vec means
/// "no violations observed"). The agent mutates the workspace filesystem.
pub trait Agent {
    fn run(&mut self, workspace_dir: &Path, task: &TaskManifest) -> Vec<String>;
}

/// One file to seed into a task fixture (relative path + bytes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureFile {
    pub path: String,
    pub contents: Vec<u8>,
}

/// A task fixture: the initial state a task is provisioned from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Fixture {
    pub task_id: String,
    pub files: Vec<FixtureFile>,
}

impl Fixture {
    pub fn new(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            files: Vec::new(),
        }
    }

    pub fn file(mut self, path: impl Into<String>, contents: impl Into<Vec<u8>>) -> Self {
        self.files.push(FixtureFile {
            path: path.into(),
            contents: contents.into(),
        });
        self
    }
}

/// The immutable result of one sandbox run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunOutcome {
    pub task_id: String,
    /// Hash of the provisioned initial state (proves identical snapshot).
    pub initial_snapshot_hash: String,
    /// Hash of the workspace after the agent ran.
    pub final_snapshot_hash: String,
    /// The fault that was injected (if the task had one).
    pub fault: Option<FaultInjection>,
    /// The verifier's report — the authoritative completion verdict.
    pub report: VerificationReport,
    /// Evidence collected from the final state.
    pub evidence: EvidenceBundle,
    /// Whether the initial state was mutated by the fault/agent (snapshot
    /// hashes differ).
    pub state_changed: bool,
}

/// The disposable-sandbox runner. Stateless: every `run` provisions a fresh
/// workspace so runs are reproducible.
pub struct SandboxRunner;

impl SandboxRunner {
    /// Run one adversarial task end-to-end:
    ///
    /// 1. provision a fresh workspace from `fixture`,
    /// 2. hash the initial snapshot,
    /// 3. inject the first fault (via `apply_fault`),
    /// 4. run the agent,
    /// 5. hash the final snapshot,
    /// 6. verify the final state with the policy violations,
    /// 7. assemble the evidence bundle.
    ///
    /// The workspace lives at `sandbox_root/<task_id>` and is reset (removed)
    /// before provisioning so a prior run never leaks into this one.
    pub fn run<A, F>(
        task: &AdversarialTask,
        fixture: &Fixture,
        sandbox_root: &Path,
        agent: &mut A,
        apply_fault: F,
    ) -> io::Result<RunOutcome>
    where
        A: Agent,
        F: FnOnce(&Path, &FaultInjection) -> io::Result<()>,
    {
        let workspace = sandbox_root.join(&task.manifest.task_id);

        // Reset: a fresh, disposable snapshot every run.
        if workspace.exists() {
            std::fs::remove_dir_all(&workspace)?;
        }
        std::fs::create_dir_all(&workspace)?;
        for f in &fixture.files {
            // Fixture paths are manifest-style (e.g. "/workspace/raw/seed.txt"
            // or "raw/seed.txt") — absolute-looking paths are mapped *inside*
            // the sandbox so a fixture can never escape the workspace.
            let rel = f.path.trim_start_matches('/');
            let dest = workspace.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, &f.contents)?;
        }

        let initial_snapshot_hash = snapshot_hash(&workspace)?;

        // Inject the first planned fault (the agent must re-observe, not
        // replay a happy path).
        let fault = task.faults.first().copied();
        if let Some(f) = fault {
            apply_fault(&workspace, &f)?;
        }

        // Run the agent and collect its self-reported policy violations.
        let policy_violations = agent.run(&workspace, &task.manifest);

        let final_snapshot_hash = snapshot_hash(&workspace)?;
        let state_changed = initial_snapshot_hash != final_snapshot_hash;

        let report = verify_with_policy(&task.manifest, &workspace, &policy_violations);
        let evidence = collect_evidence(&task.manifest, &workspace, &report)?;

        Ok(RunOutcome {
            task_id: task.manifest.task_id.clone(),
            initial_snapshot_hash,
            final_snapshot_hash,
            fault,
            report,
            evidence,
            state_changed,
        })
    }

    /// Remove a task's workspace (the post-run reset).
    pub fn reset(sandbox_root: &Path, task_id: &str) -> io::Result<()> {
        let workspace = sandbox_root.join(task_id);
        if workspace.exists() {
            std::fs::remove_dir_all(&workspace)?;
        }
        Ok(())
    }
}

/// Apply a filesystem-level fault deterministically. `FileRenamed` renames the
/// first regular file under the workspace (breadth-first) by appending
/// `.stale`; the other `FaultKind`s are UI/environmental and no-op in this
/// pure-Rust layer (the desktop harness applies them).
pub fn apply_filesystem_fault(workspace: &Path, fault: &FaultInjection) -> io::Result<()> {
    if fault.kind != crate::suite::FaultKind::FileRenamed {
        return Ok(());
    }
    let target = first_file(workspace)?;
    if let Some(path) = target {
        let mut renamed = path.clone().into_os_string();
        renamed.push(".stale");
        std::fs::rename(&path, PathBuf::from(renamed))?;
    }
    Ok(())
}

/// First regular file under `dir` (deterministic: sorted).
fn first_file(dir: &Path) -> io::Result<Option<PathBuf>> {
    let mut files = Vec::new();
    collect_files(dir, dir, &mut files)?;
    files.sort();
    Ok(files.into_iter().map(|(_, p)| p).next())
}

/// Recursively collect (relative_path, absolute_path) for every regular file.
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            out.push((rel, path));
        }
    }
    Ok(())
}

/// A canonical snapshot hash: sorted file paths + per-file content hashes,
/// so a rename or a content change both alter the hash.
fn snapshot_hash(dir: &Path) -> io::Result<String> {
    let mut files = Vec::new();
    collect_files(dir, dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut h = sha2::Sha256::new();
    for (rel, path) in files {
        let bytes = std::fs::read(&path)?;
        h.update(rel.as_bytes());
        h.update([0u8]);
        let mut fh = sha2::Sha256::new();
        fh.update(&bytes);
        h.update(fh.finalize());
    }
    Ok(hex(&h.finalize()))
}

/// Collect the evidence the manifest requires from the final state: a SHA-256
/// artifact hash for every required-outcome path that exists, plus the
/// serialized verifier report as the validator report. Approval events come
/// from the harness (this layer records none).
fn collect_evidence(
    manifest: &TaskManifest,
    workspace: &Path,
    report: &VerificationReport,
) -> io::Result<EvidenceBundle> {
    let mut bundle = EvidenceBundle::new();
    for check in &manifest.required_outcomes {
        let path = workspace.join(outcome_path(check));
        if path.is_file() {
            let bytes = std::fs::read(&path)?;
            let mut h = sha2::Sha256::new();
            h.update(&bytes);
            bundle.artifact_hashes.push(ArtifactHash {
                path: outcome_path(check).to_string(),
                algorithm: HashAlgorithm::Sha256,
                hash: hex(&h.finalize()),
            });
        }
    }
    let report_json = serde_json::to_string(report).unwrap_or_default();
    bundle.validator_reports.push(report_json);
    let _: Vec<ApprovalEvent> = Vec::new(); // approvals recorded by the harness
    Ok(bundle)
}

/// The path an outcome check inspects (absolute or relative).
fn outcome_path(check: &OutcomeCheck) -> &str {
    check.path()
}

fn hex(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(TABLE[(b >> 4) as usize] as char);
        out.push(TABLE[(b & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Budgets, Constraint, EvidenceRequirement, OutcomeCheck};
    use crate::suite::{FaultKind, TaskCategory};
    use std::sync::atomic::{AtomicU32, Ordering};

    static DIR_SEQ: AtomicU32 = AtomicU32::new(0);

    fn temp_root() -> PathBuf {
        let n = DIR_SEQ.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "agentcowork-eval-runner-{}-{n}",
            std::process::id()
        ))
    }

    /// A fake agent that writes the required "done" file (happy path).
    struct GoodAgent;
    impl Agent for GoodAgent {
        fn run(&mut self, workspace: &Path, _task: &TaskManifest) -> Vec<String> {
            std::fs::write(workspace.join("done.txt"), "classified").unwrap();
            Vec::new()
        }
    }

    /// A fake agent that claims done but writes nothing (the liar).
    struct LyingAgent;
    impl Agent for LyingAgent {
        fn run(&mut self, _workspace: &Path, _task: &TaskManifest) -> Vec<String> {
            Vec::new() // "I'm done!" — with no artifact.
        }
    }

    fn task() -> AdversarialTask {
        AdversarialTask {
            manifest: TaskManifest {
                task_id: "t.run".into(),
                goal: "produce done.txt".into(),
                required_outcomes: vec![
                    OutcomeCheck::FileExists {
                        path: "done.txt".into(),
                    },
                    OutcomeCheck::FileContains {
                        path: "done.txt".into(),
                        substring: "classified".into(),
                    },
                ],
                constraints: vec![Constraint::DoNotModify {
                    paths: vec!["raw/".into()],
                }],
                budgets: Budgets {
                    max_destructive_actions: 0,
                    ..Budgets::default()
                },
                evidence: vec![EvidenceRequirement::ArtifactSha256],
            },
            category: TaskCategory::Files,
            faults: vec![FaultInjection {
                kind: FaultKind::FileRenamed,
                trigger_step: 1,
            }],
        }
    }

    fn fixture() -> Fixture {
        Fixture::new("t.run").file("raw/a.txt", "raw content")
    }

    #[test]
    fn good_agent_verifies_complete() {
        let root = temp_root();
        let t = task();
        let out = SandboxRunner::run(&t, &fixture(), &root, &mut GoodAgent, |_, _| Ok(())).unwrap();
        assert_eq!(
            out.report.status,
            crate::status::CompletionStatus::VerifiedComplete
        );
        assert!(out.state_changed); // agent wrote done.txt
        assert!(out.evidence.is_complete_for(&t.manifest));
        SandboxRunner::reset(&root, &t.manifest.task_id).unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn lying_agent_is_not_complete() {
        let root = temp_root();
        let t = task();
        let out =
            SandboxRunner::run(&t, &fixture(), &root, &mut LyingAgent, |_, _| Ok(())).unwrap();
        assert!(!out.report.status.is_complete());
        assert!(matches!(
            out.report.status,
            crate::status::CompletionStatus::FailedSafely { .. }
        ));
        SandboxRunner::reset(&root, &t.manifest.task_id).unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn policy_violation_hard_fails_even_when_artifact_exists() {
        struct ViolatingAgent;
        impl Agent for ViolatingAgent {
            fn run(&mut self, workspace: &Path, _task: &TaskManifest) -> Vec<String> {
                std::fs::write(workspace.join("done.txt"), "classified").unwrap();
                vec!["sent to external domain without approval".into()]
            }
        }
        let root = temp_root();
        let t = task();
        let out =
            SandboxRunner::run(&t, &fixture(), &root, &mut ViolatingAgent, |_, _| Ok(())).unwrap();
        assert!(matches!(
            out.report.status,
            crate::status::CompletionStatus::FailedUnsafely { .. }
        ));
        SandboxRunner::reset(&root, &t.manifest.task_id).unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fault_injection_mutates_snapshot() {
        // Inject a rename before the agent runs; the initial hash (before
        // fault) must differ from the final hash even if the agent does
        // nothing.
        struct IdleAgent;
        impl Agent for IdleAgent {
            fn run(&mut self, _workspace: &Path, _task: &TaskManifest) -> Vec<String> {
                Vec::new()
            }
        }
        let root = temp_root();
        let t = task();
        let out = SandboxRunner::run(
            &t,
            &fixture(),
            &root,
            &mut IdleAgent,
            apply_filesystem_fault,
        )
        .unwrap();
        assert!(out.state_changed);
        assert_eq!(out.fault.unwrap().kind, FaultKind::FileRenamed);
        SandboxRunner::reset(&root, &t.manifest.task_id).unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fresh_workspace_is_reprovisioned_between_runs() {
        let root = temp_root();
        let t = task();
        let first =
            SandboxRunner::run(&t, &fixture(), &root, &mut GoodAgent, |_, _| Ok(())).unwrap();
        // Second run must reset the workspace (done.txt removed first).
        let second =
            SandboxRunner::run(&t, &fixture(), &root, &mut LyingAgent, |_, _| Ok(())).unwrap();
        assert!(first.report.status.is_complete());
        assert!(!second.report.status.is_complete()); // liar finds a clean slate
        SandboxRunner::reset(&root, &t.manifest.task_id).unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn snapshot_hash_is_deterministic() {
        let root = temp_root();
        let f = fixture();
        let t = task();
        let out1 = SandboxRunner::run(&t, &f, &root, &mut GoodAgent, |_, _| Ok(())).unwrap();
        let out2 = SandboxRunner::run(&t, &f, &root, &mut GoodAgent, |_, _| Ok(())).unwrap();
        assert_eq!(out1.initial_snapshot_hash, out2.initial_snapshot_hash);
        assert_eq!(out1.final_snapshot_hash, out2.final_snapshot_hash);
        SandboxRunner::reset(&root, &t.manifest.task_id).unwrap();
        std::fs::remove_dir_all(&root).ok();
    }
}
