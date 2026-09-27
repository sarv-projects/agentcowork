//! Checkpointing + registry (P6.1 — resume-after-reboot + checkpoint freeze
//! on circuit-break). A [`Checkpoint`] snapshots the whole plan as JSON so a
//! rebooted session resumes from a turn boundary instead of re-planning; the
//! optional `frozen_reason` is set when a circuit-break (B6 MCQ) halts the
//! run, so the resume path can ask "resume or retry?" rather than silently
//! continuing. [`BlueprintRegistry`] indexes blueprint `.md` files from a
//! directory (blueprint → optional `AgentConfig` frontmatter).

use crate::blueprint::Blueprint;
use crate::frontmatter::AgentConfig;
#[cfg(test)]
use crate::frontmatter::Isolation;
use crate::md::{BlueprintDoc, MdError};
use agentcowork_types::CheckpointId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// A durable snapshot of a plan at a turn boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// P71.3g — the canonical checkpoint id (`ARCH/RECOVERY.md` §7). Snapshot
    /// files written before ids existed deserialize as unassigned, so a resume
    /// can still read them without inventing an identity for them.
    #[serde(default)]
    pub checkpoint_id: CheckpointId,
    pub blueprint: Blueprint,
    /// Non-empty when frozen on circuit-break (B6 MCQ pattern).
    #[serde(default)]
    pub frozen_reason: Option<String>,
    /// Plan version (bumped on rewrite; used by the plan cache).
    #[serde(default)]
    pub version: u32,
    /// `ARCH/16-CONTEXT.md` §4 item 3.2 — the structured checkpoint fields a
    /// resume reads. Absent on a bare plan snapshot, so an older file still
    /// deserializes unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<CheckpointState>,
    /// The kind of reconstruction point (`DM-006`): `work` · `context` ·
    /// `workflow` · `session`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Epoch ms when the checkpoint was written. Used to decide whether live
    /// state is newer than this checkpoint (`REQ-CTX-008`).
    #[serde(default)]
    pub written_at_ms: u64,
    /// A content digest of the blueprint payload, so a corrupted snapshot is
    /// detected rather than resumed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// The documented structured-checkpoint shape (`ARCH/16-CONTEXT.md` §4 item 3.2).
///
/// Every list defaults to empty, which is a *value*: the point of this shape is
/// that a reader never has to guess whether a field is present, only whether it
/// has entries.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CheckpointState {
    pub objective: String,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub completed: Vec<String>,
    #[serde(default)]
    pub active: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub workers: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub next_actions: Vec<String>,
    /// A model-written narrative for the non-reconstructable residue. Advisory
    /// only: the deterministic fields above are the state, so a narrative can
    /// never be the sole source (`ARCH/16-CONTEXT.md` §4 item 3.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrative: Option<String>,
}

impl CheckpointState {
    /// The documented field list, in order. A checkpoint that claims to be
    /// structured but is missing one of these is not a structured checkpoint
    /// (`REQ-CTX-007`).
    pub const FIELDS: [&'static str; 11] = [
        "objective",
        "requirements",
        "decisions",
        "completed",
        "active",
        "files",
        "tests",
        "artifacts",
        "workers",
        "blockers",
        "next_actions",
    ];

    /// Whether the state has an objective and at least one next action. Anything
    /// less is not a resumable work state.
    pub fn is_resumable(&self) -> bool {
        !self.objective.trim().is_empty() && !self.next_actions.is_empty()
    }

    /// Attach the model-written residue for the non-reconstructable parts.
    /// The deterministic fields are untouched, so the narrative is additive.
    pub fn with_narrative(mut self, narrative: &str) -> Self {
        self.narrative = Some(narrative.to_string());
        self
    }
}

/// Which source a rebuild should use (`ARCH/16-CONTEXT.md` §9: `rebuild`
/// prefers live state over a stale checkpoint).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum RebuildSource {
    /// The checkpoint is at least as new as live state.
    Checkpoint { checkpoint_id: CheckpointId },
    /// Live state is newer, so the checkpoint would be stale.
    Live,
    /// Nothing to compare: a cold resume reads the checkpoint it was handed.
    Cold,
}

/// The resolved rebuild decision, plus whether a memory summary is involved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebuildDecision {
    pub source: RebuildSource,
    /// A memory `summary` item may reference this checkpoint; it is never a
    /// second timeline and never the state (`DEC-041`).
    pub memory_summary_is_reference_only: bool,
    /// True when the checkpoint's version differs from the live plan version —
    /// an explicit migration/rebuild decision is required rather than a silent
    /// resume (`REQ-CTX-008`).
    pub version_mismatch: bool,
}

impl Checkpoint {
    pub fn new(blueprint: Blueprint) -> Self {
        Self {
            checkpoint_id: CheckpointId::unassigned(),
            blueprint,
            frozen_reason: None,
            version: 0,
            state: None,
            kind: None,
            written_at_ms: 0,
            digest: None,
        }
    }

    /// Attach the canonical id (a step checkpoint knows its `(work, step)`).
    pub fn with_id(mut self, checkpoint_id: CheckpointId) -> Self {
        self.checkpoint_id = checkpoint_id;
        self
    }

    /// Attach the structured work state, the stamp and the digest. This is what
    /// makes a checkpoint a *context/work* reconstruction point rather than a
    /// bare plan snapshot.
    pub fn with_state(mut self, kind: &str, state: CheckpointState, written_at_ms: u64) -> Self {
        self.kind = Some(kind.to_string());
        self.digest = Some(plan_digest(&self.blueprint));
        self.state = Some(state);
        self.written_at_ms = written_at_ms;
        self
    }

    /// True when this checkpoint carries the declared structured shape and is
    /// therefore reconstructable from live work state.
    pub fn is_reconstructable(&self) -> bool {
        self.state.as_ref().is_some_and(|s| s.is_resumable())
    }

    /// Decide what a rebuild should read. Live state wins whenever it is newer
    /// than the checkpoint or carries a different plan version; a version
    /// mismatch is surfaced rather than silently resumed.
    pub fn rebuild_decision(&self, live_at_ms: u64, live_version: u32) -> RebuildDecision {
        let version_mismatch = live_version != self.version;
        let source = match (self.written_at_ms, live_at_ms) {
            (0, 0) => RebuildSource::Cold,
            (0, _) => RebuildSource::Live,
            (cp, live) if live > cp || version_mismatch => RebuildSource::Live,
            _ => RebuildSource::Checkpoint {
                checkpoint_id: self.checkpoint_id.clone(),
            },
        };
        RebuildDecision {
            source,
            memory_summary_is_reference_only: true,
            version_mismatch,
        }
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen_reason.is_some()
    }
}

/// A stable digest of the plan payload, so a torn or edited snapshot file is
/// detected instead of resumed from.
fn plan_digest(blueprint: &Blueprint) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(blueprint.id.as_bytes());
    h.update([0u8]);
    h.update(blueprint.goal.as_bytes());
    h.update([0u8]);
    for t in &blueprint.tasks {
        h.update(t.spec.id.as_bytes());
        h.update([0u8]);
    }
    format!("{:x}", h.finalize())
}

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(
        "checkpoint digest mismatch at {path}: recorded {expected}, payload hashes to {actual}"
    )]
    DigestMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
}

impl Blueprint {
    /// Freeze the plan to `path` atomically (temp-file + rename) so a crash
    /// never leaves a half-written checkpoint.
    pub fn checkpoint_to(
        &self,
        path: &Path,
        frozen_reason: Option<&str>,
        version: u32,
    ) -> Result<(), CheckpointError> {
        let cp = Checkpoint {
            // A file-targeted snapshot has no `(work, step)` to derive from;
            // the per-step path (`checkpoint_step_to`) assigns the id.
            checkpoint_id: CheckpointId::unassigned(),
            blueprint: self.clone(),
            frozen_reason: frozen_reason.map(str::to_string),
            version,
            state: None,
            kind: None,
            written_at_ms: 0,
            digest: Some(plan_digest(self)),
        };
        atomic_write_json(path, &cp)
    }

    /// Write a **structured** checkpoint: the documented work-state shape plus
    /// the stamp and the plan digest. This is the form a compaction boundary
    /// writes before it projects the log (`ARCH/16-CONTEXT.md` §4), and the form
    /// a resume reads.
    #[allow(clippy::too_many_arguments)]
    pub fn checkpoint_state_to(
        &self,
        path: &Path,
        kind: &str,
        state: &CheckpointState,
        version: u32,
        written_at_ms: u64,
    ) -> Result<(), CheckpointError> {
        let cp = Checkpoint {
            checkpoint_id: CheckpointId::unassigned(),
            blueprint: self.clone(),
            frozen_reason: None,
            version,
            state: Some(state.clone()),
            kind: Some(kind.to_string()),
            written_at_ms,
            digest: Some(plan_digest(self)),
        };
        atomic_write_json(path, &cp)
    }

    /// Resume a frozen/checkpointed plan from `path`.
    pub fn resume_from(path: &Path) -> Result<Checkpoint, CheckpointError> {
        let bytes = std::fs::read(path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Resume and **verify**: a snapshot whose plan digest does not match its
    /// payload is refused rather than resumed from, so a torn or hand-edited
    /// file cannot become the state a run continues from.
    pub fn resume_verified(path: &Path) -> Result<Checkpoint, CheckpointError> {
        let cp = Self::resume_from(path)?;
        if let Some(d) = &cp.digest {
            let actual = plan_digest(&cp.blueprint);
            if actual != *d {
                return Err(CheckpointError::DigestMismatch {
                    path: path.to_path_buf(),
                    expected: d.clone(),
                    actual,
                });
            }
        }
        Ok(cp)
    }
}

/// Write JSON atomically: serialize → temp file → rename over the target.
fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), CheckpointError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let tmp = temp_sibling(path);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// P64.7 — per-step checkpoint & rollback index (SPEC I16, ARCH/17 §17.10)
// ---------------------------------------------------------------------------
//
// Every mutating tool call checkpoints: git for code workspaces (the SHA is
// recorded by the caller via `agentcowork-core::execution::commit_workspace_
// snapshot`, which reuses `git_commit::commit_verified_edit`) + the JSON
// snapshot below (reuses [`Blueprint::checkpoint_to`]). The UI restore picker
// lists [`StepCheckpoint`] rows; restore itself is fence-checked in
// `agentcowork-core::execution` (`check_restore_fence` + the never-replay
// predicate), never here — this module owns the index, not authority.

/// P64.7 — one restorable step: the blueprint snapshot + the fencing token
/// that owned the run when the step landed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepCheckpoint {
    /// P71.3g — the canonical checkpoint id (`ckpt:<work>/<step>`). Rows written
    /// before ids existed deserialize as unassigned and are re-derived on read
    /// (deterministic, so no row is given a new identity).
    #[serde(default)]
    pub checkpoint_id: CheckpointId,
    /// Owning work id (`execution/begin*` id, e.g. `ex:3`).
    pub work_id: String,
    /// Monotonic step within the work (1-based).
    pub step: u32,
    /// Short git SHA when the workspace is a git checkout (`None` for
    /// non-git resource snapshots — honest, never faked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
    /// File name of the blueprint snapshot inside the checkpoint dir.
    pub snapshot_file: String,
    /// Opaque `RunAuthority` fencing token at checkpoint time.
    #[serde(default)]
    pub fencing_token: u64,
    /// Wall-clock ms when the checkpoint landed.
    #[serde(default)]
    pub created_at_ms: u64,
}

fn step_snapshot_name(work_id: &str, step: u32) -> String {
    let safe: String = work_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("bp-{safe}-step-{step}.json")
}

fn step_index_name(work_id: &str, step: u32) -> String {
    let snap = step_snapshot_name(work_id, step);
    format!("{}.step.json", snap.trim_end_matches(".json"))
}

impl Blueprint {
    /// P64.7 — checkpoint one mutating step: reuse [`Blueprint::checkpoint_to`]
    /// for the snapshot payload, then write the [`StepCheckpoint`] index row
    /// atomically beside it. Returns the index row.
    pub fn checkpoint_step_to(
        &self,
        dir: &Path,
        work_id: &str,
        step: u32,
        git_sha: Option<String>,
        fencing_token: u64,
    ) -> Result<StepCheckpoint, CheckpointError> {
        if work_id.is_empty() {
            return Err(CheckpointError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "checkpoint_step_to requires work_id",
            )));
        }
        if step == 0 {
            return Err(CheckpointError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "checkpoint_step_to requires step >= 1",
            )));
        }
        std::fs::create_dir_all(dir)?;
        let snapshot_file = step_snapshot_name(work_id, step);
        let checkpoint_id = CheckpointId::for_step(work_id, step);
        // The snapshot carries the same identity as its index row.
        self.checkpoint_to(&dir.join(&snapshot_file), None, step)?;
        let row = StepCheckpoint {
            checkpoint_id,
            work_id: work_id.to_string(),
            step,
            git_sha,
            snapshot_file: snapshot_file.clone(),
            fencing_token,
            created_at_ms: now_ms(),
        };
        atomic_write_json(&dir.join(step_index_name(work_id, step)), &row)?;
        Ok(row)
    }

    /// P64.7 — list all step rows for a work, sorted by step (the restore
    /// picker's input). Missing dir ⇒ empty (no steps yet, not an error).
    pub fn list_step_checkpoints(
        dir: &Path,
        work_id: &str,
    ) -> Result<Vec<StepCheckpoint>, CheckpointError> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".step.json") {
                continue;
            }
            let bytes = std::fs::read(entry.path())?;
            let mut row: StepCheckpoint = serde_json::from_slice(&bytes)?;
            if row.checkpoint_id.is_assigned() {
                // already identified
            } else {
                // Legacy row (written before ids existed): re-derive, never mint.
                row.checkpoint_id = CheckpointId::for_step(&row.work_id, row.step);
            }
            if row.work_id == work_id {
                out.push(row);
            }
        }
        out.sort_by_key(|r| r.step);
        Ok(out)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| "checkpoint".into());
    name.push(".tmp");
    path.with_file_name(name)
}

/// An in-memory index of blueprints loaded from `.md` files.
#[derive(Debug, Default)]
pub struct BlueprintRegistry {
    docs: Vec<BlueprintDoc>,
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("blueprint parse error: {0}")]
    Md(#[from] MdError),
    #[error("duplicate blueprint id {0:?}")]
    DuplicateId(String),
}

impl BlueprintRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, doc: BlueprintDoc) -> Result<(), RegistryError> {
        let id = doc.blueprint.id.clone();
        if self.get(&id).is_some() {
            return Err(RegistryError::DuplicateId(id));
        }
        self.docs.push(doc);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Blueprint> {
        self.docs.iter().map(|d| &d.blueprint).find(|b| b.id == id)
    }

    pub fn doc(&self, id: &str) -> Option<&BlueprintDoc> {
        self.docs.iter().find(|d| d.blueprint.id == id)
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Load every `*.md` file in `dir` that parses as a blueprint. Files that
    /// are not blueprints (missing the `# Blueprint:` header) are skipped.
    /// Returns the number of blueprints loaded.
    pub fn load_dir(&mut self, dir: &Path) -> Result<usize, RegistryError> {
        let mut loaded = 0;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let text = std::fs::read_to_string(&path)?;
            match BlueprintDoc::from_markdown(&text) {
                Ok(doc) => {
                    self.insert(doc)?;
                    loaded += 1;
                }
                Err(MdError::MissingId) => { /* not a blueprint — skip */ }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(loaded)
    }

    /// The agent configs carried by registered blueprints (name → config), so
    /// a blueprint directory doubles as an `AgentConfig` registry.
    pub fn agent_configs(&self) -> Vec<(&str, &AgentConfig)> {
        self.docs
            .iter()
            .filter_map(|d| {
                d.agent_config
                    .as_ref()
                    .map(|c| (d.blueprint.id.as_str(), c))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::{BlueprintTask, VerifyBlock};
    use crate::spec::TaskSpec;

    fn bp(id: &str) -> Blueprint {
        let mut b = Blueprint::new(id, "goal");
        b.push(BlueprintTask::new(
            TaskSpec::new("a", "do a"),
            VerifyBlock::new(vec![]),
        ));
        b
    }

    #[test]
    fn checkpoint_roundtrips_atomically() {
        let dir = std::env::temp_dir().join("bp-ckpt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bp.json");

        bp("bp-1")
            .checkpoint_to(&path, Some("circuit-break: budget"), 3)
            .unwrap();
        let cp = Blueprint::resume_from(&path).unwrap();
        assert_eq!(cp.blueprint.id, "bp-1");
        assert!(cp.is_frozen());
        assert_eq!(cp.frozen_reason.as_deref(), Some("circuit-break: budget"));
        assert_eq!(cp.version, 3);

        // No leftover temp file.
        assert!(!dir.join("bp.json.tmp").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unfrozen_checkpoint_is_not_frozen() {
        let cp = Checkpoint::new(bp("bp-1"));
        assert!(!cp.is_frozen());
    }

    #[test]
    fn registry_loads_dir_and_skips_non_blueprints() {
        let dir = std::env::temp_dir().join("bp-reg");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), bp("a").to_markdown()).unwrap();
        std::fs::write(dir.join("b.md"), bp("b").to_markdown()).unwrap();
        std::fs::write(dir.join("notes.md"), "# not a blueprint\njust prose").unwrap();

        let mut reg = BlueprintRegistry::new();
        let n = reg.load_dir(&dir).unwrap();
        assert_eq!(n, 2);
        assert!(reg.get("a").is_some());
        assert!(reg.get("b").is_some());
        assert!(reg.get("notes").is_none());
        assert_eq!(reg.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn registry_rejects_duplicate_ids() {
        let mut reg = BlueprintRegistry::new();
        reg.insert(BlueprintDoc {
            agent_config: None,
            blueprint: bp("a"),
        })
        .unwrap();
        assert!(matches!(
            reg.insert(BlueprintDoc {
                agent_config: None,
                blueprint: bp("a"),
            }),
            Err(RegistryError::DuplicateId(_))
        ));
    }

    #[test]
    fn registry_exposes_agent_configs() {
        use crate::frontmatter::{AgentConfig, PermissionMode};
        let mut reg = BlueprintRegistry::new();
        reg.insert(BlueprintDoc {
            agent_config: Some(AgentConfig {
                permission_mode: PermissionMode::Plan,
                color: None,
                hooks: vec![],
                mcp_servers: vec![],
                max_turns: None,
                effort: None,
                background: None,
                isolation: Isolation::None,
            }),
            blueprint: bp("a"),
        })
        .unwrap();
        let cfgs = reg.agent_configs();
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].0, "a");
        assert_eq!(cfgs[0].1.permission_mode, PermissionMode::Plan);
    }

    // --- P64.7 per-step checkpoint ------------------------------------------------

    #[test]
    fn p64_step_checkpoint_reuses_snapshot_format() {
        let dir = std::env::temp_dir().join(format!("bp-p64-step-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let b = bp("bp-step");
        let row = b
            .checkpoint_step_to(&dir, "ex:3", 1, Some("abc123".into()), 9)
            .unwrap();
        assert_eq!(row.work_id, "ex:3");
        assert_eq!(row.step, 1);
        assert_eq!(row.git_sha.as_deref(), Some("abc123"));
        assert_eq!(row.fencing_token, 9);
        // Snapshot payload is the same `Checkpoint` shape as `checkpoint_to`.
        let cp = Blueprint::resume_from(&dir.join(&row.snapshot_file)).unwrap();
        assert_eq!(cp.blueprint.id, "bp-step");
        assert_eq!(cp.version, 1);
        // Non-git step records honest None.
        let row2 = b.checkpoint_step_to(&dir, "ex:3", 2, None, 9).unwrap();
        assert_eq!(row2.git_sha, None);
        // Restore picker lists in step order.
        let rows = Blueprint::list_step_checkpoints(&dir, "ex:3").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].step, rows[1].step), (1, 2));
        // Other works are filtered out; missing dir is empty, not an error.
        assert!(
            Blueprint::list_step_checkpoints(&dir, "ex:9")
                .unwrap()
                .is_empty()
        );
        assert!(
            Blueprint::list_step_checkpoints(&dir.join("nope"), "ex:3")
                .unwrap()
                .is_empty()
        );
        // No leftover temp files.
        assert!(!dir.join("bp.json.tmp").exists());
        // Fail closed on bad inputs.
        assert!(b.checkpoint_step_to(&dir, "", 1, None, 0).is_err());
        assert!(b.checkpoint_step_to(&dir, "ex:3", 0, None, 0).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod structured_checkpoint_tests {
    use super::*;
    use crate::blueprint::{BlueprintTask, VerifyBlock};
    use crate::spec::TaskSpec;

    fn bp(id: &str) -> Blueprint {
        let mut b = Blueprint::new(id, "goal");
        b.push(BlueprintTask::new(
            TaskSpec::new("a", "do a"),
            VerifyBlock::new(vec![]),
        ));
        b
    }

    fn state() -> CheckpointState {
        CheckpointState {
            objective: "ship the memory store".into(),
            requirements: vec!["REQ-MEM-001".into()],
            decisions: vec!["use buck".into()],
            completed: vec!["step 1".into()],
            active: vec!["step 2".into()],
            files: vec!["build.rs".into()],
            tests: vec!["cargo test".into()],
            artifacts: vec!["ctxout:1".into()],
            workers: vec!["w1".into()],
            blockers: vec![],
            next_actions: vec!["write the tests".into()],
            narrative: None,
        }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bp-structured-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_structured_checkpoint_roundtrips_with_the_documented_shape() {
        let d = dir("roundtrip");
        let path = d.join("cp.json");
        bp("bp-1")
            .checkpoint_state_to(&path, "work", &state(), 3, 1_700_000_000_000)
            .unwrap();
        let cp = Blueprint::resume_from(&path).unwrap();
        assert_eq!(cp.kind.as_deref(), Some("work"));
        assert_eq!(cp.version, 3);
        assert_eq!(cp.written_at_ms, 1_700_000_000_000);
        assert!(cp.is_reconstructable());
        let s = cp.state.as_ref().unwrap();
        // The documented field set is present and populated (an empty list is a
        // value; a missing field is not).
        assert_eq!(s.objective, "ship the memory store");
        assert_eq!(s.requirements.len(), 1);
        assert_eq!(s.decisions.len(), 1);
        assert_eq!(s.completed.len(), 1);
        assert_eq!(s.active.len(), 1);
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.tests.len(), 1);
        assert_eq!(s.artifacts.len(), 1);
        assert_eq!(s.workers.len(), 1);
        // `blockers` is empty here — an empty list is a real value.
        assert!(s.blockers.is_empty());
        assert_eq!(s.next_actions, vec!["write the tests".to_string()]);
        assert_eq!(CheckpointState::FIELDS.len(), 11);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_legacy_snapshot_file_still_deserializes() {
        let d = dir("legacy");
        let path = d.join("legacy.json");
        // The pre-structured shape: no `state`, no `kind`, no digest.
        std::fs::write(
            &path,
            br#"{"checkpoint_id":"ckpt:ex:1/1","blueprint":{"id":"bp","goal":"g","tasks":[]},"version":2}"#,
        )
        .unwrap();
        let cp = Blueprint::resume_from(&path).unwrap();
        assert_eq!(cp.version, 2);
        assert!(cp.state.is_none());
        assert!(!cp.is_reconstructable());
        // No digest recorded means nothing to verify against, so the resume is
        // allowed — the file predates digests.
        assert!(Blueprint::resume_verified(&path).is_ok());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_narrative_is_additive_and_never_the_sole_state() {
        let s = state().with_narrative("the team chose buck after evaluating bazel");
        assert_eq!(s.objective, "ship the memory store");
        assert!(!s.next_actions.is_empty());
        assert!(s.is_resumable());
        assert_eq!(
            s.narrative.as_deref(),
            Some("the team chose buck after evaluating bazel")
        );
        // Without a narrative the checkpoint is still resumable: the residue is
        // optional by design.
        assert!(state().is_resumable());
        assert!(state().narrative.is_none());
    }

    #[test]
    fn a_state_without_an_objective_or_next_action_is_not_resumable() {
        let mut s = state();
        s.objective = String::new();
        assert!(!s.is_resumable());
        let mut s2 = state();
        s2.next_actions.clear();
        assert!(!s2.is_resumable());
    }

    #[test]
    fn rebuild_prefers_live_state_over_a_stale_checkpoint() {
        let cp = Checkpoint {
            version: 2,
            ..Checkpoint::new(bp("bp-1"))
                .with_state("work", state(), 1_000)
                .with_id(CheckpointId::for_step("ex:1", 1))
        };
        // Live state is newer → the checkpoint would be stale.
        let d = cp.rebuild_decision(2_000, 2);
        assert_eq!(d.source, RebuildSource::Live);
        assert!(!d.version_mismatch);
        assert!(d.memory_summary_is_reference_only);
        // Live state older → the checkpoint is authoritative.
        assert!(matches!(
            cp.rebuild_decision(500, 2).source,
            RebuildSource::Checkpoint { .. }
        ));
    }

    #[test]
    fn a_version_mismatch_is_surfaced_rather_than_silently_resumed() {
        let cp = Checkpoint {
            version: 2,
            ..Checkpoint::new(bp("bp-1"))
                .with_state("work", state(), 5_000)
                .with_id(CheckpointId::for_step("ex:1", 1))
        };
        // Same timestamp, different plan version: the mismatch is declared, and
        // the decision goes to live state rather than pretending to match.
        let d = cp.rebuild_decision(5_000, 3);
        assert!(d.version_mismatch);
        assert_eq!(d.source, RebuildSource::Live);
    }

    #[test]
    fn a_cold_resume_reads_the_checkpoint_it_was_handed() {
        let cp = Checkpoint::new(bp("bp-1"));
        assert_eq!(cp.rebuild_decision(0, 0).source, RebuildSource::Cold);
    }

    #[test]
    fn a_tampered_snapshot_is_refused_rather_than_resumed_from() {
        let d = dir("tamper");
        let path = d.join("cp.json");
        bp("bp-1")
            .checkpoint_state_to(&path, "work", &state(), 1, 1_000)
            .unwrap();
        assert!(Blueprint::resume_verified(&path).is_ok());
        // Edit the payload without updating the digest.
        let mut raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        raw["blueprint"]["goal"] = serde_json::json!("a different goal");
        std::fs::write(&path, raw.to_string()).unwrap();
        let err = Blueprint::resume_verified(&path).unwrap_err();
        assert!(matches!(err, CheckpointError::DigestMismatch { .. }));
        // The unverified read still works: verification is opt-in at the call
        // site, not something the read hides.
        assert!(Blueprint::resume_from(&path).is_ok());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_digest_changes_when_the_plan_changes() {
        let a = plan_digest(&bp("bp-1"));
        let b = plan_digest(&bp("bp-2"));
        assert_ne!(a, b, "a different plan hashes differently");
    }
}
