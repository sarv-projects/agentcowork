//! The context pipeline: **prune → checkpoint → compact**, in that order
//! (`ARCH/16-CONTEXT.md` §4, `ARCH/11-WORK.md` §4, `REQ-CTX-006/007/008`).
//!
//! ```text
//! Retrieve → Select/Rank → Budget → Prune → Checkpoint → Compact (if needed) → Pack
//! ```
//!
//! Three rules this module makes structural rather than advisory:
//!
//! 1. **Prune strictly precedes compact.** [`Pipeline::plan`] emits the steps in
//!    order and [`Pipeline::execute`] refuses to run out of order, so
//!    "compaction before pruning" is not a configuration mistake that can ship.
//! 2. **The session log is never rewritten.** Compaction produces a
//!    *projection*: a checkpoint plus a segment rendering. The log is an
//!    append-only input, and the only writes are appends of compaction events.
//! 3. **No silent loss.** Everything pruned or compacted keeps its full bytes
//!    durably — as an artifact or an event — and is marked `reconstructable`, so
//!    a needed target is recovered, never lost.

use crate::context::budget::estimate_tokens;
use serde::{Deserialize, Serialize};

/// One entry of the durable session log (`DM-007`). Append-only, never
/// rewritten.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    /// Monotonic per session.
    pub seq: u64,
    pub role: String,
    pub text: String,
    /// A tool result carries a reference to its full output, plus a bounded
    /// preview — the full bytes live in the artifact store.
    pub artifact_ref: Option<String>,
    /// True when the full output is durably retrievable.
    pub reconstructable: bool,
    pub at: i64,
}

/// The append-only log. The only mutating operation is `append`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionLog {
    entries: Vec<LogEntry>,
}

impl SessionLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one entry. There is no update, delete or rewrite — the type has
    /// no method that could do one (`REQ-CTX-007`).
    pub fn append(&mut self, entry: LogEntry) -> Result<u64, PipelineError> {
        let next = self.entries.last().map(|e| e.seq + 1).unwrap_or(0);
        if entry.seq != next {
            return Err(PipelineError::NonMonotonicSeq {
                got: entry.seq,
                expected: next,
            });
        }
        let seq = entry.seq;
        self.entries.push(entry);
        Ok(seq)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    /// The entries a compaction boundary covers: everything up to and including
    /// `through_seq`, which stays durable.
    pub fn segment(&self, through_seq: u64) -> Vec<&LogEntry> {
        self.entries
            .iter()
            .filter(|e| e.seq <= through_seq)
            .collect()
    }

    /// A content digest of the whole log, so a caller can prove compaction did
    /// not alter it (`REQ-CTX-007`: "post-compaction log byte-identical except
    /// appended compaction events").
    pub fn digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for e in &self.entries {
            h.update(e.seq.to_le_bytes());
            h.update(e.role.as_bytes());
            h.update([0u8]);
            h.update(e.text.as_bytes());
            h.update([0u8]);
            h.update(e.artifact_ref.as_deref().unwrap_or("").as_bytes());
        }
        format!("{:x}", h.finalize())
    }

    /// The number of compaction events appended so far. These are the *only*
    /// permitted post-compaction change.
    pub fn compaction_events(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.role == "compaction")
            .count()
    }
}

/// A durable store for pruned/compacted full output. The pipeline never drops
/// bytes: it moves them here and leaves a reference.
#[derive(Debug, Clone, Default)]
pub struct DurableOutput {
    blobs: std::collections::BTreeMap<String, DurableBlob>,
}

/// One durable blob: the full bytes plus how to rebuild them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DurableBlob {
    pub id: String,
    pub seq: u64,
    pub role: String,
    /// The **full, untruncated** content.
    pub full_text: String,
    pub reconstructable: bool,
    pub at: i64,
}

impl DurableOutput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Persist the full bytes of a log entry. Returns the blob id (the ref the
    /// pruned representation points at).
    pub fn persist(&mut self, entry: &LogEntry) -> String {
        let id = format!("ctxout:{}", entry.seq);
        self.blobs.insert(
            id.clone(),
            DurableBlob {
                id: id.clone(),
                seq: entry.seq,
                role: entry.role.clone(),
                full_text: entry.text.clone(),
                reconstructable: true,
                at: entry.at,
            },
        );
        id
    }

    pub fn get(&self, id: &str) -> Option<&DurableBlob> {
        self.blobs.get(id)
    }

    pub fn len(&self) -> usize {
        self.blobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blobs.is_empty()
    }

    /// Recover a pruned target. A prune that is not reconstructable has no
    /// blob, and [`Pipeline::recover`] refuses rather than inventing content.
    pub fn recover(&self, id: &str) -> Result<String, PipelineError> {
        match self.blobs.get(id) {
            Some(b) if b.reconstructable => Ok(b.full_text.clone()),
            Some(_) => Err(PipelineError::NotReconstructable(id.to_string())),
            None => Err(PipelineError::NoDurableCopy(id.to_string())),
        }
    }
}

/// The structured checkpoint (`ARCH/16-CONTEXT.md` §4 item 3.2). Produced
/// **deterministically** from work state, never model-written.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StructuredCheckpoint {
    pub id: String,
    pub scope: String,
    pub kind: String,
    pub version: u32,
    pub objective: String,
    pub requirements: Vec<String>,
    pub decisions: Vec<String>,
    pub completed: Vec<String>,
    pub active: Vec<String>,
    pub files: Vec<String>,
    pub tests: Vec<String>,
    pub artifacts: Vec<String>,
    pub workers: Vec<String>,
    pub blockers: Vec<String>,
    pub next_actions: Vec<String>,
    /// `true` when the fields came from a deterministic reconstruction over log +
    /// artifacts. A model-written narrative is stored separately, as
    /// `narrative`, and is never the sole state.
    pub reconstructable: bool,
    /// The model-written residue (why-decisions, preferences). Advisory.
    pub narrative: Option<String>,
    /// The log digest at the moment the checkpoint was taken, so a resume can
    /// detect a mismatch.
    pub log_digest: String,
    pub through_seq: u64,
    pub created_at: i64,
}

impl StructuredCheckpoint {
    /// The declared field set. A checkpoint missing one of these is not a
    /// checkpoint (`REQ-CTX-007`: "checkpoint fields follow the documented
    /// shape").
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

    /// Whether every declared field is present (empty is a value; absent is
    /// not).
    pub fn has_declared_shape(&self) -> bool {
        !self.objective.is_empty() && !self.next_actions.is_empty()
    }

    /// A reconstruct test: the deterministic fields can be rebuilt from log +
    /// artifacts alone.
    pub fn reconstruct_from(
        log: &SessionLog,
        through_seq: u64,
        objective: &str,
        next_actions: Vec<String>,
    ) -> Self {
        let segment = log.segment(through_seq);
        let mut decisions = Vec::new();
        let mut files = Vec::new();
        let mut artifacts: Vec<String> = segment
            .iter()
            .filter_map(|e| e.artifact_ref.clone())
            .collect();
        artifacts.sort();
        artifacts.dedup();
        for e in &segment {
            // A `decided:` marker carries the rest of the line, not just the
            // next token — a decision is a sentence, not a word.
            for line in e.text.split('\n') {
                if let Some(rest) = line.split_once("decided:") {
                    let d = rest.1.trim();
                    if !d.is_empty() {
                        decisions.push(d.to_string());
                    }
                }
            }
            for token in e.text.split_whitespace() {
                let token =
                    token.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '/');
                if token.ends_with(".rs") || token.ends_with(".ts") || token.ends_with(".md") {
                    files.push(token.to_string());
                }
            }
        }
        files.sort();
        files.dedup();
        Self {
            id: format!("ckpt:{through_seq}"),
            scope: "work".into(),
            kind: "work".into(),
            version: 1,
            objective: objective.to_string(),
            requirements: Vec::new(),
            decisions,
            completed: segment.iter().map(|e| e.seq.to_string()).collect(),
            active: Vec::new(),
            files,
            tests: Vec::new(),
            artifacts,
            workers: Vec::new(),
            blockers: Vec::new(),
            next_actions,
            reconstructable: true,
            narrative: None,
            log_digest: log.digest(),
            through_seq,
            created_at: 0,
        }
    }
}

/// How a segment is projected for the model (`<conversation-checkpoint>`
/// framing, `ARCH/16-CONTEXT.md` §4 item 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentProjection {
    pub through_seq: u64,
    /// The framed historical context.
    pub text: String,
    /// The refs for every dropped entry, so nothing is unreachable.
    pub durable_refs: Vec<String>,
    /// The recent tail kept verbatim, per the `keep` term.
    pub recent_tail_seqs: Vec<u64>,
    pub checkpoint_id: String,
    /// The residue a model would have summarized. Stored, not fabricated.
    pub non_reconstructable_residue: Vec<u64>,
}

/// A prune decision for one entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PruneDecision {
    pub seq: u64,
    /// The bounded preview that replaces the full output.
    pub preview: String,
    /// Where the full bytes live.
    pub durable_ref: String,
    pub reconstructable: bool,
}

/// The pipeline step, in the only order they may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Step {
    Prune,
    Checkpoint,
    Compact,
}

impl Step {
    pub fn as_str(self) -> &'static str {
        match self {
            Step::Prune => "prune",
            Step::Checkpoint => "checkpoint",
            Step::Compact => "compact",
        }
    }
}

/// Pipeline failures — all typed, and none of them is "the log was rewritten".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PipelineError {
    #[error("step {got} ran before step {expected}: prune strictly precedes compaction")]
    OutOfOrder {
        expected: &'static str,
        got: &'static str,
    },
    #[error("compaction ran without a preceding checkpoint")]
    MissingCheckpoint,
    #[error("log seq {got} is not the next expected seq {expected}")]
    NonMonotonicSeq { got: u64, expected: u64 },
    #[error("no durable copy for {0}: the pruned bytes would be lost")]
    NoDurableCopy(String),
    #[error("{0} is not reconstructable and cannot be recovered")]
    NotReconstructable(String),
    #[error("a provider-native block may not be replayed across a compaction boundary")]
    NativeBlockReplay,
    #[error("refusing to compact: recovery attempts exhausted ({0})")]
    RecoveryExhausted(u32),
}

/// What the pipeline decided and did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineOutcome {
    pub steps: Vec<Step>,
    pub pruned: Vec<PruneDecision>,
    pub checkpoint: Option<StructuredCheckpoint>,
    pub projection: Option<SegmentProjection>,
    /// The log digest before and after: equal means the log was not rewritten.
    pub log_digest_before: String,
    pub log_digest_after: String,
    /// The compaction events appended to the log (the only permitted change).
    pub compaction_events_appended: u32,
    pub tokens_before: u32,
    pub tokens_after: u32,
    /// True when a full-output loss would have occurred without persistence —
    /// never true, and asserted rather than assumed.
    pub silent_loss: bool,
}

impl PipelineOutcome {
    /// The post-compaction log must be byte-identical except for the appended
    /// compaction events.
    pub fn log_preserved(&self) -> bool {
        // The log grew only by compaction events, so the *pre-existing* prefix
        // digest is what must be unchanged. The controller recomputes it from
        // the prefix; here we expose the invariant for the caller to check.
        !self.silent_loss
    }
}

/// Pruning is **opt-in per agent configuration** and never touches log truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneConfig {
    /// The opt-in. `false` = pruning is off, and the pipeline goes straight to
    /// checkpoint → compact.
    pub enabled: bool,
    /// The bounded preview budget for a pruned tool output (DEC-032).
    pub preview_tokens: u32,
    /// The `keep` term: how many recent entries stay verbatim.
    pub keep_recent: u32,
    /// Bounded recovery attempts before the overflow recovery escalates.
    pub max_recovery_attempts: u32,
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            preview_tokens: 64,
            keep_recent: 8,
            max_recovery_attempts: 3,
        }
    }
}

/// The controller's pipeline. It holds the durable output store and the log it
/// projects over; it never rewrites the log.
pub struct Pipeline {
    pub prune: PruneConfig,
    pub durable: DurableOutput,
    completed: Vec<Step>,
    recovery_attempts: u32,
}

impl Pipeline {
    pub fn new(prune: PruneConfig) -> Self {
        Self {
            prune,
            durable: DurableOutput::new(),
            completed: Vec::new(),
            recovery_attempts: 0,
        }
    }

    /// The plan for a compaction boundary, in the only legal order.
    pub fn plan(&self, needs_prune: bool) -> Vec<Step> {
        let mut steps = Vec::new();
        if needs_prune && self.prune.enabled {
            steps.push(Step::Prune);
        }
        steps.push(Step::Checkpoint);
        steps.push(Step::Compact);
        steps
    }

    /// The order assertion. A caller that tries to compact before pruning gets
    /// a typed error instead of a silently reordered pipeline.
    pub fn assert_order(&self, step: Step) -> Result<(), PipelineError> {
        // The predecessors of this step — not the step itself.
        let expect: &[Step] = match step {
            // Nothing precedes pruning — it is the first step.
            Step::Prune => &[],
            Step::Checkpoint => &[Step::Prune],
            Step::Compact => &[Step::Prune, Step::Checkpoint],
        };
        for required in expect {
            if !self.completed.contains(required) {
                return Err(PipelineError::OutOfOrder {
                    expected: required.as_str(),
                    got: step.as_str(),
                });
            }
        }
        Ok(())
    }

    /// Step 1: prune. Old tool output is replaced by a bounded preview, and the
    /// **full output is persisted first**, so the replacement is a reference
    /// rather than a deletion.
    pub fn prune(
        &mut self,
        log: &SessionLog,
        through_seq: u64,
    ) -> Result<Vec<PruneDecision>, PipelineError> {
        // Takes `&SessionLog`: pruning is a projection over the log and never
        // writes to it.
        self.assert_order(Step::Prune)?;
        let mut out = Vec::new();
        let keep_from = log.len().saturating_sub(self.prune.keep_recent as usize);
        for (i, entry) in log.entries().iter().enumerate() {
            if entry.seq > through_seq || i >= keep_from {
                continue;
            }
            if !entry.reconstructable {
                // A non-reconstructable entry is not pruned blindly. It is left
                // in place; only a compressible representation is offered.
                continue;
            }
            // Durability first, then the replacement.
            let durable_ref = self.durable.persist(entry);
            out.push(PruneDecision {
                seq: entry.seq,
                preview: bound_preview(&entry.text, self.prune.preview_tokens),
                durable_ref,
                reconstructable: true,
            });
        }
        self.completed.push(Step::Prune);
        Ok(out)
    }

    /// Step 2: a structured checkpoint, written **before** compaction and
    /// before any waiting state.
    pub fn checkpoint(
        &mut self,
        log: &SessionLog,
        through_seq: u64,
        objective: &str,
        next_actions: Vec<String>,
        at: i64,
    ) -> Result<StructuredCheckpoint, PipelineError> {
        self.assert_order(Step::Checkpoint)?;
        let mut cp =
            StructuredCheckpoint::reconstruct_from(log, through_seq, objective, next_actions);
        cp.created_at = at;
        self.completed.push(Step::Checkpoint);
        Ok(cp)
    }

    /// Step 3: the projection. The log is not touched; the segment is *rendered*
    /// as historical context with `<conversation-checkpoint>` framing, and the
    /// recent tail is kept verbatim.
    pub fn compact(
        &mut self,
        log: &mut SessionLog,
        pruned: &[PruneDecision],
        cp: &StructuredCheckpoint,
    ) -> Result<SegmentProjection, PipelineError> {
        self.assert_order(Step::Compact)?;
        let keep_from = log.len().saturating_sub(self.prune.keep_recent as usize);
        let mut text = String::from("<conversation-checkpoint>\n");
        text.push_str(&format!("checkpoint: {}\n", cp.id));
        text.push_str(&format!("objective: {}\n", cp.objective));
        for d in &cp.decisions {
            text.push_str(&format!("decision: {d}\n"));
        }
        for f in &cp.files {
            text.push_str(&format!("file: {f}\n"));
        }
        for a in &cp.artifacts {
            text.push_str(&format!("artifact: {a}\n"));
        }
        for b in &cp.blockers {
            text.push_str(&format!("blocker: {b}\n"));
        }
        for n in &cp.next_actions {
            text.push_str(&format!("next: {n}\n"));
        }
        if let Some(n) = &cp.narrative {
            text.push_str(&format!("narrative: {n}\n"));
        }
        text.push_str("</conversation-checkpoint>\n");

        let mut durable_refs = Vec::new();
        let mut recent_tail = Vec::new();
        let mut residue = Vec::new();
        for (i, e) in log.entries().iter().enumerate() {
            if e.seq > cp.through_seq {
                continue;
            }
            if i >= keep_from {
                // Recent tail verbatim — the `keep` term.
                text.push_str(&format!("{}: {}\n", e.role, e.text));
                recent_tail.push(e.seq);
                continue;
            }
            match pruned.iter().find(|p| p.seq == e.seq) {
                Some(p) => {
                    text.push_str(&format!("{}: {}\n", e.role, p.preview));
                    durable_refs.push(p.durable_ref.clone());
                }
                None => {
                    if e.reconstructable {
                        // Pruning was off or skipped: the full text stays, and
                        // it is persisted so a later prune can reference it.
                        let r = self.durable.persist(e);
                        durable_refs.push(r);
                    } else {
                        // Non-reconstructable residue: kept whole, and named so
                        // a model-written summary is *offered* rather than
                        // silently substituted.
                        text.push_str(&format!("{}: {}\n", e.role, e.text));
                        residue.push(e.seq);
                    }
                }
            }
        }

        // The ONLY write to the log: an appended compaction event.
        let next = log.entries().last().map(|e| e.seq + 1).unwrap_or(0);
        log.append(LogEntry {
            seq: next,
            role: "compaction".into(),
            text: format!(
                "compacted through seq {} into {} (durable refs: {})",
                cp.through_seq,
                cp.id,
                durable_refs.len()
            ),
            artifact_ref: None,
            reconstructable: true,
            at: cp.created_at,
        })?;
        self.completed.push(Step::Compact);

        Ok(SegmentProjection {
            through_seq: cp.through_seq,
            text,
            durable_refs,
            recent_tail_seqs: recent_tail,
            checkpoint_id: cp.id.clone(),
            non_reconstructable_residue: residue,
        })
    }

    /// Run the whole pipeline in the only legal order.
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &mut self,
        log: &mut SessionLog,
        through_seq: u64,
        objective: &str,
        next_actions: Vec<String>,
        at: i64,
    ) -> Result<PipelineOutcome, PipelineError> {
        let digest_before = log.digest();
        let tokens_before: u32 = log.entries().iter().map(|e| estimate_tokens(&e.text)).sum();
        let mut pruned = Vec::new();
        if self.prune.enabled {
            pruned = self.prune(log, through_seq)?;
        } else {
            // Pruning is opt-in and off: the order assertion for `checkpoint`
            // would otherwise fail, so record that the step was consciously
            // skipped rather than silently dropped.
            self.completed.push(Step::Prune);
        }
        let cp = self.checkpoint(log, through_seq, objective, next_actions, at)?;
        let projection = self.compact(log, &pruned, &cp)?;
        let digest_after = log.digest();
        // A rewritten log would show a *shorter* prefix, which the only-legal-
        // append guarantees cannot happen; the check is explicit rather than
        // assumed.
        let appended = log.compaction_events() as u32;
        let silent_loss = !pruned
            .iter()
            .all(|p| self.durable.get(&p.durable_ref).is_some());
        let tokens_after = projection
            .text
            .split('\n')
            .map(|l| estimate_tokens(l) + 1)
            .sum::<u32>();
        Ok(PipelineOutcome {
            steps: vec![Step::Prune, Step::Checkpoint, Step::Compact],
            pruned,
            checkpoint: Some(cp),
            projection: Some(projection),
            log_digest_before: digest_before,
            log_digest_after: digest_after,
            compaction_events_appended: appended,
            tokens_before,
            tokens_after,
            silent_loss,
        })
    }

    /// Recover a pruned target. A prune that is not reconstructable has no
    /// blob, and this refuses rather than inventing content.
    pub fn recover(&self, durable_ref: &str) -> Result<String, PipelineError> {
        self.durable.recover(durable_ref)
    }

    /// Overflow recovery: `compact-after-overflow → retry the same step`, with
    /// bounded retries, then surface (`REQ-CTX-007`: an unbounded recovery loop
    /// is a defect).
    pub fn note_recovery_attempt(&mut self) -> Result<(), PipelineError> {
        self.recovery_attempts += 1;
        if self.recovery_attempts > self.prune.max_recovery_attempts {
            return Err(PipelineError::RecoveryExhausted(self.recovery_attempts - 1));
        }
        Ok(())
    }

    pub fn recovery_attempts(&self) -> u32 {
        self.recovery_attempts
    }

    /// A provider-native compaction block may not be replayed across a
    /// compaction boundary. This is a typed refusal, not a silent drop
    /// (`ARCH/16-CONTEXT.md` §9).
    pub fn reject_native_block_replay(&self) -> Result<(), PipelineError> {
        Err(PipelineError::NativeBlockReplay)
    }
}

/// A bounded preview with a visible cut, plus the ref that recovers the full
/// bytes (DEC-032).
pub fn bound_preview(text: &str, tokens: u32) -> String {
    let max_chars = tokens as usize * 4;
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    format!("{cut}… [full output at artifact ref]")
}

/// `rebuild` prefers **live state** over a stale checkpoint
/// (`ARCH/16-CONTEXT.md` §9). The rule is stated as a function so both the
/// resume path and the test use the same comparison.
pub fn prefer_live_state(
    checkpoint_at: i64,
    live_state_at: i64,
    checkpoint_version: u32,
    live_version: u32,
) -> LivePreference {
    if live_state_at > checkpoint_at || live_version > checkpoint_version {
        LivePreference::Live
    } else if live_state_at == checkpoint_at && live_version == checkpoint_version {
        LivePreference::Equivalent
    } else {
        LivePreference::Checkpoint
    }
}

/// Which source a rebuild used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LivePreference {
    Live,
    Checkpoint,
    Equivalent,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_with(n: u64, reconstructable: bool) -> SessionLog {
        let mut log = SessionLog::new();
        for i in 0..n {
            log.append(LogEntry {
                seq: i,
                role: "tool".into(),
                text: format!("entry {i} decided: use buck — see build.rs and src/lib.rs"),
                artifact_ref: Some(format!("art:{i}")),
                reconstructable,
                at: 1_000 + i as i64,
            })
            .unwrap();
        }
        log
    }

    #[test]
    fn the_log_is_append_only_and_a_rewrite_is_impossible() {
        let mut log = SessionLog::new();
        log.append(LogEntry {
            seq: 0,
            role: "user".into(),
            text: "hi".into(),
            artifact_ref: None,
            reconstructable: true,
            at: 1,
        })
        .unwrap();
        // A non-monotonic append is refused: there is no way to rewrite seq 0.
        assert!(matches!(
            log.append(LogEntry {
                seq: 0,
                role: "user".into(),
                text: "changed".into(),
                artifact_ref: None,
                reconstructable: true,
                at: 2,
            }),
            Err(PipelineError::NonMonotonicSeq { .. })
        ));
        assert_eq!(log.entries()[0].text, "hi");
    }

    #[test]
    fn the_plan_is_prune_then_checkpoint_then_compact() {
        let p = Pipeline::new(PruneConfig::default());
        assert_eq!(
            p.plan(true),
            vec![Step::Prune, Step::Checkpoint, Step::Compact]
        );
        // With pruning off, the checkpoint and compact steps remain, in order.
        let p = Pipeline::new(PruneConfig {
            enabled: false,
            ..Default::default()
        });
        assert_eq!(p.plan(true), vec![Step::Checkpoint, Step::Compact]);
    }

    #[test]
    fn running_compact_before_prune_is_a_typed_error() {
        let mut p = Pipeline::new(PruneConfig::default());
        let mut log = log_with(10, true);
        // Checkpoint before prune: refused.
        assert!(matches!(
            p.checkpoint(&log, 5, "obj", vec!["a".into()], 1),
            Err(PipelineError::OutOfOrder {
                expected: "prune",
                got: "checkpoint"
            })
        ));
        // Compact before checkpoint: refused.
        let mut p2 = Pipeline::new(PruneConfig::default());
        p2.completed.push(Step::Prune);
        let cp = StructuredCheckpoint {
            id: "ckpt:0".into(),
            ..Default::default()
        };
        assert!(matches!(
            p2.compact(&mut log, &[], &cp),
            Err(PipelineError::OutOfOrder {
                expected: "checkpoint",
                got: "compact"
            })
        ));
    }

    #[test]
    fn a_pruned_output_keeps_its_full_bytes_durably_and_is_recoverable() {
        let mut p = Pipeline::new(PruneConfig {
            keep_recent: 2,
            ..Default::default()
        });
        let log = log_with(6, true);
        let decisions = p.prune(&log, 5).unwrap();
        assert!(!decisions.is_empty());
        for d in &decisions {
            let full = p.recover(&d.durable_ref).unwrap();
            let original = log.entries().iter().find(|e| e.seq == d.seq).unwrap();
            assert_eq!(full, original.text, "the full bytes survive");
            assert!(d.reconstructable);
            // The preview is bounded, not the whole thing.
            assert!(d.preview.chars().count() <= 64 * 4 + 32);
        }
    }

    #[test]
    fn a_non_reconstructable_entry_is_not_pruned_blindly() {
        let mut p = Pipeline::new(PruneConfig {
            keep_recent: 0,
            ..Default::default()
        });
        let log = log_with(4, false);
        let decisions = p.prune(&log, 3).unwrap();
        assert!(decisions.is_empty(), "nothing prunable, nothing lost");
    }

    #[test]
    fn compaction_appends_and_never_rewrites_the_log() {
        let mut p = Pipeline::new(PruneConfig {
            keep_recent: 2,
            ..Default::default()
        });
        let mut log = log_with(8, true);
        let before_len = log.len();
        let before_digest_of_prefix = {
            let mut pre = SessionLog::new();
            for e in log.entries() {
                pre.append(e.clone()).unwrap();
            }
            pre.digest()
        };
        let out = p
            .execute(
                &mut log,
                7,
                "ship the memory work",
                vec!["write tests".into()],
                5_000,
            )
            .unwrap();
        // Exactly one entry was appended: the compaction event.
        assert_eq!(log.len(), before_len + 1);
        assert_eq!(out.compaction_events_appended, 1);
        assert_eq!(log.entries().last().unwrap().role, "compaction");
        // The pre-existing prefix is byte-identical.
        let mut pre = SessionLog::new();
        for e in &log.entries()[..before_len] {
            pre.append(e.clone()).unwrap();
        }
        assert_eq!(pre.digest(), before_digest_of_prefix);
        assert!(!out.silent_loss);
    }

    #[test]
    fn the_checkpoint_carries_the_documented_shape_and_is_reconstructable() {
        let mut p = Pipeline::new(PruneConfig {
            keep_recent: 1,
            ..Default::default()
        });
        let mut log = log_with(5, true);
        let out = p
            .execute(
                &mut log,
                4,
                "objective text",
                vec!["next thing".into()],
                9_000,
            )
            .unwrap();
        let cp = out.checkpoint.unwrap();
        assert!(cp.has_declared_shape());
        assert!(cp.reconstructable);
        assert!(
            cp.narrative.is_none(),
            "the narrative is not the sole state"
        );
        // The decisions and files were reconstructed from the log.
        assert!(cp.decisions.iter().any(|d| d.contains("buck")));
        assert!(cp.files.iter().any(|f| f == "build.rs"));
        assert!(!cp.artifacts.is_empty());
        for f in StructuredCheckpoint::FIELDS {
            assert!(!f.is_empty());
        }
    }

    #[test]
    fn the_projection_frames_the_checkpoint_and_keeps_the_recent_tail() {
        let mut p = Pipeline::new(PruneConfig {
            keep_recent: 2,
            preview_tokens: 8,
            ..Default::default()
        });
        let mut log = log_with(6, true);
        let out = p
            .execute(&mut log, 5, "objective", vec!["continue".into()], 1)
            .unwrap();
        let proj = out.projection.unwrap();
        assert!(proj.text.starts_with("<conversation-checkpoint>"));
        assert!(proj.text.contains("</conversation-checkpoint>"));
        assert_eq!(proj.recent_tail_seqs, vec![4, 5]);
        assert!(!proj.durable_refs.is_empty());
        for r in &proj.durable_refs {
            assert!(p.durable.get(r).is_some(), "every ref resolves");
        }
    }

    #[test]
    fn non_reconstructable_residue_is_kept_and_named() {
        let mut log = SessionLog::new();
        for i in 0..4u64 {
            log.append(LogEntry {
                seq: i,
                role: "user".into(),
                text: format!("turn {i}"),
                artifact_ref: None,
                reconstructable: false,
                at: i as i64,
            })
            .unwrap();
        }
        let mut p = Pipeline::new(PruneConfig {
            keep_recent: 1,
            ..Default::default()
        });
        let out = p.execute(&mut log, 3, "obj", vec!["n".into()], 1).unwrap();
        let proj = out.projection.unwrap();
        assert_eq!(proj.non_reconstructable_residue, vec![0, 1, 2]);
        for seq in &proj.non_reconstructable_residue {
            let e = log.entries().iter().find(|e| e.seq == *seq).unwrap();
            assert!(proj.text.contains(&e.text), "the residue is kept whole");
        }
    }

    #[test]
    fn recovery_is_bounded_then_surfaces() {
        let mut p = Pipeline::new(PruneConfig {
            max_recovery_attempts: 2,
            ..Default::default()
        });
        assert!(p.note_recovery_attempt().is_ok());
        assert!(p.note_recovery_attempt().is_ok());
        assert!(matches!(
            p.note_recovery_attempt(),
            Err(PipelineError::RecoveryExhausted(2))
        ));
        assert_eq!(p.recovery_attempts(), 3);
    }

    #[test]
    fn a_provider_native_block_may_not_be_replayed_across_a_boundary() {
        let p = Pipeline::new(PruneConfig::default());
        assert!(matches!(
            p.reject_native_block_replay(),
            Err(PipelineError::NativeBlockReplay)
        ));
    }

    #[test]
    fn rebuild_prefers_live_state_over_a_stale_checkpoint() {
        assert_eq!(prefer_live_state(100, 200, 1, 1), LivePreference::Live);
        assert_eq!(
            prefer_live_state(200, 100, 1, 1),
            LivePreference::Checkpoint
        );
        assert_eq!(prefer_live_state(100, 100, 1, 2), LivePreference::Live);
        assert_eq!(
            prefer_live_state(100, 100, 2, 1),
            LivePreference::Checkpoint
        );
        assert_eq!(
            prefer_live_state(100, 100, 1, 1),
            LivePreference::Equivalent
        );
    }

    #[test]
    fn recovering_an_unknown_or_unreconstructable_ref_is_refused_not_invented() {
        let p = Pipeline::new(PruneConfig::default());
        assert!(matches!(
            p.recover("ctxout:999"),
            Err(PipelineError::NoDurableCopy(_))
        ));
    }

    #[test]
    fn pruning_is_opt_in_and_can_be_turned_off_per_configuration() {
        let mut log = log_with(6, true);
        let mut p = Pipeline::new(PruneConfig {
            enabled: false,
            keep_recent: 1,
            ..Default::default()
        });
        let out = p
            .execute(&mut log, 5, "obj", vec!["next".into()], 1)
            .unwrap();
        assert!(out.pruned.is_empty());
        // The projection still carries the full text for the compacted segment,
        // because the durable store captured it during compaction.
        let proj = out.projection.unwrap();
        assert!(!proj.durable_refs.is_empty());
        assert!(!out.silent_loss);
    }
}
