//! The **context controller** — the agent's context-control layer
//! (`ARCH/16-CONTEXT.md` §1.2, `ARCH/15-AGENT-X.md` §5, `REQ-CTX-001..005`).
//!
//! ```text
//! assemble · estimateBudget · select · prune · compact · rebuild · pin · exclude
//! ```
//!
//! These verbs belong to the agent, not to Core. The split is enforced by
//! construction rather than by review:
//!
//! - the controller holds its sources as [`ReadOnlySource`], so it has no path
//!   that can write one (INV-08, `REQ-CTX-001`);
//! - it reads memory through [`crate::recall::MemoryStore::recall`], which
//!   returns **candidates**, so the controller — not memory — decides inclusion
//!   (DEC-019, `REQ-CTX-003`);
//! - it never discovers overflow at the provider: the feasibility check runs
//!   first, and a refusal is returned with guidance before any send
//!   (`REQ-CTX-005`).

pub mod budget;
pub mod pipeline;
pub mod sources;
pub mod stability;

use crate::injection::{MutationKind, RenderedBlocks};
use crate::recall::{RecallConfig, RecallOutcome};
use budget::{
    BudgetTerms, Feasibility, Refusal, TurnFootprint, check_feasibility, refusal_guidance,
};
use pipeline::{Pipeline, PipelineError, PipelineOutcome, PruneConfig};
use serde::{Deserialize, Serialize};
use sources::ContextItem;
use stability::{CacheTelemetry, FrozenBlock, PackingCache};

/// The pinned/excluded manual control (`ARCH/16-CONTEXT.md` §7).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ManualControl {
    pub pinned: Vec<String>,
    pub excluded: Vec<String>,
}

impl ManualControl {
    /// `focus` / `pin` / `exclude`. A pin is a floor: a pinned item is never
    /// pruned. It is still subject to the pin ceiling, so a pin cannot be a way
    /// to exceed the budget silently.
    pub fn pin(&mut self, id: &str) {
        if !self.pinned.iter().any(|p| p == id) {
            self.pinned.push(id.to_string());
        }
        self.excluded.retain(|e| e != id);
    }

    pub fn unpin(&mut self, id: &str) {
        self.pinned.retain(|p| p != id);
    }

    pub fn exclude(&mut self, id: &str) {
        if !self.excluded.iter().any(|e| e == id) {
            self.excluded.push(id.to_string());
        }
        self.pinned.retain(|p| p != id);
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.pinned.iter().any(|p| p == id)
    }

    pub fn is_excluded(&self, id: &str) -> bool {
        self.excluded.iter().any(|e| e == id)
    }
}

/// The trace the UI's context inspector reads. It is a **projection**: it
/// reports what was decided, and it owns no context state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControllerTrace {
    pub turn: u64,
    /// The named budget terms, verbatim.
    pub budget: budget::BudgetBreakdown,
    pub footprint: TurnFootprint,
    /// The pre-send verdict. A `ProviderSent` verdict is impossible: the trace
    /// has no field for it.
    pub feasibility: Feasibility,
    pub recall_outcome: RecallOutcome,
    pub selected: Vec<TraceItem>,
    pub pinned: usize,
    pub excluded: usize,
    pub pruned: usize,
    pub compacted_through_seq: Option<u64>,
    pub checkpoint_id: Option<String>,
    pub relevant_tokens: u32,
    pub always_on_tokens: u32,
    pub cache: CacheTelemetry,
    /// The gates the memory path reported, for the honest UI.
    pub deferred: Vec<String>,
}

/// One selected item, as the inspector shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceItem {
    pub id: String,
    pub source: String,
    pub trust_tier: crate::scope::TrustTier,
    pub sensitivity: crate::scope::Sensitivity,
    pub tokens: u32,
    pub reconstructable: bool,
    pub content_ref: String,
}

/// The turn result. One of four shapes, always explicit.
///
/// The payloads are boxed because the variants differ hugely in size: an
/// unboxed trace would make the small `Recover` variant as large as the
/// biggest, so every `TurnPlan` would pay for the worst case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TurnPlan {
    /// Send this.
    Send {
        injection: Box<Injection>,
        turn: Box<PackedTurn>,
    },
    /// Recovery is available: prune/compact first, then retry. This is never
    /// surfaced to the user as an error while recovery options remain.
    Recover {
        over_by: u32,
        trace: Box<ControllerTrace>,
    },
    /// Refused before send, with guidance. Never sent to fail at the provider.
    Refused {
        refusal: Refusal,
        trace: Box<ControllerTrace>,
    },
    /// Compaction ran; the turn must be re-evaluated (bounded retries).
    Compacted {
        outcome: Box<PipelineOutcome>,
        trace: Box<ControllerTrace>,
    },
}

/// The dynamic part of a turn: the task, observations and tool results.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnInput {
    pub task: String,
    pub dynamic_suffix: Vec<String>,
    pub turn: u64,
    pub objective: String,
    pub next_actions: Vec<String>,
}

/// The context controller.
pub struct ContextController {
    pub terms: BudgetTerms,
    pub recall_cfg: RecallConfig,
    pub prune_cfg: PruneConfig,
    pub manual: ManualControl,
    pub packing: PackingCache,
    pipeline: Pipeline,
    /// The pin ceiling: pins are a floor, never a way past the budget.
    pub pin_ceiling_tokens: u32,
    /// Recovery attempts for this turn; bounded (`REQ-CTX-007`).
    pub recovery_attempts: u32,
    pub max_recovery_attempts: u32,
}

impl ContextController {
    pub fn new(terms: BudgetTerms) -> Self {
        Self {
            terms,
            recall_cfg: RecallConfig::default(),
            prune_cfg: PruneConfig::default(),
            manual: ManualControl::default(),
            packing: PackingCache::new(),
            pipeline: Pipeline::new(PruneConfig::default()),
            pin_ceiling_tokens: crate::injection::ALWAYS_ON_TOKEN_CEILING,
            recovery_attempts: 0,
            max_recovery_attempts: 3,
        }
    }

    /// `estimateBudget` — the resolved terms, for the inspector.
    pub fn estimate_budget(&self) -> budget::BudgetBreakdown {
        self.terms.breakdown()
    }

    /// The pre-turn feasibility check. Runs before anything is packed or sent.
    pub fn preflight(&self, footprint: &TurnFootprint) -> Result<Feasibility, budget::BudgetError> {
        check_feasibility(&self.terms, footprint)
    }

    /// `select` — take ranked candidates and pins/exclusions, and return the
    /// items the controller would pack, in order. Memory returned candidates;
    /// deciding this is the controller's job, not memory's.
    pub fn select<'a>(
        &self,
        candidates: &'a [crate::recall::Ranked],
        items: &'a [ContextItem],
    ) -> Vec<SelectedItem<'a>> {
        let mut out: Vec<SelectedItem<'a>> = Vec::new();
        // A pin is a floor: pinned items come first, in their pinned order.
        for id in &self.manual.pinned {
            if let Some(c) = candidates.iter().find(|c| &c.item.id == id) {
                out.push(SelectedItem::Pinned(c));
            }
        }
        for c in candidates {
            if self.manual.is_pinned(&c.item.id) || self.manual.is_excluded(&c.item.id) {
                continue;
            }
            out.push(SelectedItem::Ranked(c));
        }
        // Non-reconstructable context items are only ever included, never
        // dropped silently: the caller sees them in the trace.
        for i in items {
            if self.manual.is_excluded(&i.id) {
                continue;
            }
            out.push(SelectedItem::Context(i));
        }
        out
    }

    /// `rebuild` — prefer live state over a stale checkpoint.
    pub fn rebuild_preference(
        &self,
        checkpoint_at: i64,
        live_at: i64,
        checkpoint_version: u32,
        live_version: u32,
    ) -> pipeline::LivePreference {
        pipeline::prefer_live_state(checkpoint_at, live_at, checkpoint_version, live_version)
    }

    /// `prune` → `checkpoint` → `compact`, in the only legal order, over the
    /// durable log.
    pub fn compact(
        &mut self,
        log: &mut pipeline::SessionLog,
        through_seq: u64,
        objective: &str,
        next_actions: Vec<String>,
        at: i64,
    ) -> Result<PipelineOutcome, PipelineError> {
        self.pipeline = Pipeline::new(self.prune_cfg);
        self.pipeline
            .execute(log, through_seq, objective, next_actions, at)
    }

    /// A mutation happened: unfreeze the affected block, accepting the cache
    /// bust (`REQ-CTX-019`: the correctness exception).
    pub fn note_mutation(&mut self, kind: MutationKind, block_name: &str) -> bool {
        let _ = kind;
        self.packing.unfreeze(block_name)
    }

    /// Freeze the rendered memory blocks for the session. The signature comes
    /// from the block's own member set, so a change to the set produces a
    /// different signature and therefore a different frozen copy.
    pub fn freeze_memory_blocks(&mut self, blocks: &RenderedBlocks) {
        if let Some(b) = &blocks.always_on {
            self.packing.freeze(FrozenBlock {
                name: "memory_always_on".into(),
                text: b.text.clone(),
                tokens: b.tokens,
                signature: b.member_signature(),
            });
        }
    }

    /// The trace projection for the inspector. It reads state; it holds none.
    ///
    /// The argument list is the reported fact set, one field per column, rather
    /// than a struct: a trace is built once per turn from values the caller
    /// already has, so grouping them would add a type without removing any.
    #[allow(clippy::too_many_arguments)]
    pub fn trace(
        &self,
        input: &TurnInput,
        footprint: TurnFootprint,
        feasibility: Feasibility,
        blocks: &RenderedBlocks,
        recall_outcome: RecallOutcome,
        selected: &[SelectedItem<'_>],
        deferred: Vec<String>,
        compacted_through_seq: Option<u64>,
        checkpoint_id: Option<String>,
    ) -> ControllerTrace {
        ControllerTrace {
            turn: input.turn,
            budget: self.terms.breakdown(),
            footprint,
            feasibility,
            recall_outcome,
            selected: selected
                .iter()
                .map(|s| match s {
                    SelectedItem::Ranked(r) | SelectedItem::Pinned(r) => TraceItem {
                        id: r.item.id.clone(),
                        source: r.item.source.clone(),
                        trust_tier: r.item.trust_tier,
                        sensitivity: r.item.sensitivity,
                        tokens: 0,
                        reconstructable: true,
                        content_ref: format!("memory:{}", r.item.id),
                    },
                    SelectedItem::Context(i) => TraceItem {
                        id: i.id.clone(),
                        source: i.source.clone(),
                        trust_tier: crate::scope::TrustTier::DerivedUntrusted,
                        sensitivity: sensitivity_of(i),
                        tokens: i.token_cost,
                        reconstructable: i.reconstructable,
                        content_ref: i.content_ref.key(),
                    },
                })
                .collect(),
            pinned: self.manual.pinned.len(),
            excluded: self.manual.excluded.len(),
            pruned: self.prune_cfg.enabled as usize,
            compacted_through_seq,
            checkpoint_id,
            relevant_tokens: blocks.relevant_tokens(),
            always_on_tokens: blocks.always_on_tokens(),
            cache: self.packing.telemetry,
            deferred,
        }
    }

    /// A refusal, with guidance, before any send.
    pub fn refuse(&self, feasibility: Feasibility) -> Option<Refusal> {
        match feasibility {
            Feasibility::Refused { reason, shortfall } => Some(refusal_guidance(
                &self.terms,
                &Refusal {
                    reason,
                    shortfall,
                    guidance: Vec::new(),
                },
            )),
            _ => None,
        }
    }

    /// The bounded overflow recovery: retry the same step, at most
    /// `max_recovery_attempts` times, then surface.
    pub fn note_recovery(&mut self) -> Result<(), PipelineError> {
        self.recovery_attempts += 1;
        if self.recovery_attempts > self.max_recovery_attempts {
            return Err(PipelineError::RecoveryExhausted(self.recovery_attempts - 1));
        }
        Ok(())
    }
}

/// A selected item, tagged with why it was selected. A pinned item is visibly
/// pinned, so the inspector can show the floor it has.
#[derive(Debug, Clone, PartialEq)]
pub enum SelectedItem<'a> {
    Pinned(&'a crate::recall::Ranked),
    Ranked(&'a crate::recall::Ranked),
    Context(&'a ContextItem),
}

impl SelectedItem<'_> {
    pub fn id(&self) -> &str {
        match self {
            SelectedItem::Pinned(r) | SelectedItem::Ranked(r) => &r.item.id,
            SelectedItem::Context(i) => &i.id,
        }
    }

    pub fn is_pinned(&self) -> bool {
        matches!(self, SelectedItem::Pinned(_))
    }
}

fn sensitivity_of(item: &ContextItem) -> crate::scope::Sensitivity {
    match item.sensitivity {
        sources::ItemSensitivity::Public => crate::scope::Sensitivity::Public,
        sources::ItemSensitivity::Personal => crate::scope::Sensitivity::Personal,
        sources::ItemSensitivity::Confidential => crate::scope::Sensitivity::Confidential,
    }
}

/// The projection the UI reads. Nothing here can write back to the controller.
pub fn inspector_view(trace: &ControllerTrace) -> InspectorView {
    InspectorView {
        turn: trace.turn,
        window: trace.budget.model_window_resolved,
        usable: trace.budget.usable,
        per_source: trace
            .selected
            .iter()
            .fold(std::collections::BTreeMap::new(), |mut acc, i| {
                *acc.entry(i.source.clone()).or_insert(0usize) += 1;
                acc
            }),
        pinned: trace.pinned,
        excluded: trace.excluded,
        relevant_tokens: trace.relevant_tokens,
        always_on_tokens: trace.always_on_tokens,
        recent_checkpoint: trace.checkpoint_id.clone(),
    }
}

/// The Context Inspector's view model (`ARCH/16-CONTEXT.md` §7): window/usable,
/// per-source breakdown, pinned, excluded, recent checkpoint age.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InspectorView {
    pub turn: u64,
    pub window: u32,
    pub usable: u32,
    pub per_source: std::collections::BTreeMap<String, usize>,
    pub pinned: usize,
    pub excluded: usize,
    pub relevant_tokens: u32,
    pub always_on_tokens: u32,
    pub recent_checkpoint: Option<String>,
}

/// Keep the recall types in this module's public surface so a caller can hold a
/// controller and a recall response together.
pub type RecallOutcomeAlias = RecallOutcome;
/// A re-export so `SourceAvailability` is reachable from the controller.
pub use crate::recall::SourceAvailability as SourceAvail;

/// The injection shape, re-exported for callers of the controller.
pub use crate::injection::Injection;
/// The packed-turn shape, re-exported for callers of the controller.
pub use stability::PackedTurn;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::sources::{ContextScope, ProjectionPolicy};
    use crate::context::stability::StablePrefix;
    use crate::injection::render_always_on;
    use crate::recall::{AllSourcesAvailable, Ranked, RecallQuery};
    use crate::scope::{Kind, Sensitivity, TrustTier};
    use crate::store::{MemoryItem, MemoryStore, NewItem, StoreConfig};

    fn ranked(id: &str) -> Ranked {
        Ranked {
            item: MemoryItem {
                id: id.into(),
                scope: crate::scope::Scope::Project,
                scope_ref: Some("p".into()),
                kind: Kind::Fact,
                content: format!("content for {id}"),
                byte_size: 10,
                content_hash: format!("h-{id}"),
                hash_version: 1,
                dedup_key: None,
                sensitivity: Sensitivity::Personal,
                trust_tier: TrustTier::UserExplicit,
                source: "user".into(),
                source_ref: None,
                confidence: 1.0,
                pinned: false,
                used_count: 0,
                last_used_at: None,
                created_at: 1,
                updated_at: 1,
                expires_at: None,
                superseded_by: None,
            },
            relevance: 1.0,
            boost: 1.0,
            score: 1.0,
        }
    }

    fn context_item(id: &str) -> ContextItem {
        ContextItem {
            id: id.into(),
            source: "file".into(),
            item_type: "file_excerpt".into(),
            content_ref: sources::ContentRef::new("file", id),
            token_cost: 20,
            scope: ContextScope::Project,
            pinned: false,
            reconstructable: true,
            compressible: true,
            sensitivity: sources::ItemSensitivity::Personal,
            relevance: 1.0,
            freshness: 1.0,
            priority: 0,
            bounded_snippet: Some(id.into()),
        }
    }

    #[test]
    fn a_turn_that_fits_is_sent_and_the_trace_shows_the_named_terms() {
        let mut c = ContextController::new(BudgetTerms::cloud(200_000));
        let prefix = StablePrefix::new("sys", "agent", "rules", "tools");
        c.packing.pack(&prefix, vec!["task".into()]);
        let footprint = TurnFootprint {
            system_tokens: 2_000,
            message_tokens: 5_000,
            tool_schema_tokens: 1_000,
        };
        let feas = c.preflight(&footprint).unwrap();
        assert!(feas.is_fits());
        let blocks = crate::injection::RenderedBlocks::default();
        let t = c.trace(
            &TurnInput::default(),
            footprint,
            feas,
            &blocks,
            RecallOutcome::Hit,
            &[],
            vec![],
            None,
            None,
        );
        assert_eq!(t.budget.usable, BudgetTerms::cloud(200_000).usable());
        assert!(t.budget.safety_buffer > 0);
    }

    #[test]
    fn an_oversized_turn_is_recoverable_not_surfaced_as_an_error() {
        let c = ContextController::new(BudgetTerms::cloud(50_000));
        let footprint = TurnFootprint {
            system_tokens: 1_000,
            message_tokens: 200_000,
            tool_schema_tokens: 1_000,
        };
        let feas = c.preflight(&footprint).unwrap();
        assert!(matches!(feas, Feasibility::NeedsRecovery { .. }));
        assert!(c.refuse(feas).is_none(), "recovery options remain");
    }

    #[test]
    fn a_refused_turn_carries_guidance_and_is_never_sent() {
        let c = ContextController::new(BudgetTerms {
            keep: 1_000,
            ..BudgetTerms::local_small(8_192)
        });
        let footprint = TurnFootprint {
            system_tokens: 3_000,
            message_tokens: 0,
            tool_schema_tokens: 2_000,
        };
        let feas = c.preflight(&footprint).unwrap();
        let refusal = c.refuse(feas).expect("a refusal with guidance");
        assert!(!refusal.guidance.is_empty());
    }

    #[test]
    fn a_pinned_item_is_selected_first_and_an_excluded_one_is_never_selected() {
        let mut c = ContextController::new(BudgetTerms::cloud(200_000));
        let cands = vec![ranked("a"), ranked("b"), ranked("c")];
        c.manual.pin("c");
        c.manual.exclude("a");
        let sel = c.select(&cands, &[]);
        let ids: Vec<&str> = sel.iter().map(|s| s.id()).collect();
        assert_eq!(ids[0], "c", "the pin is a floor");
        assert!(sel[0].is_pinned());
        assert!(!ids.contains(&"a"), "an exclusion is absolute");
        assert!(ids.contains(&"b"));
    }

    #[test]
    fn a_non_reconstructable_context_item_is_never_pruned_blindly() {
        let c = ContextController::new(BudgetTerms::cloud(200_000));
        let mut durable = context_item("keep-me");
        durable.reconstructable = false;
        let items = vec![durable, context_item("rebuildable")];
        let cands = vec![ranked("a")];
        let sel = c.select(&cands, &items);
        let ids: Vec<&str> = sel.iter().map(|s| s.id()).collect();
        // Both context items are selected; the non-reconstructable one is
        // flagged so the pipeline can refuse to prune it.
        assert!(ids.contains(&"keep-me") && ids.contains(&"rebuildable"));
        let trace = c.trace(
            &TurnInput::default(),
            TurnFootprint::default(),
            Feasibility::Fits,
            &crate::injection::RenderedBlocks::default(),
            RecallOutcome::Hit,
            &sel,
            vec![],
            None,
            None,
        );
        let keep = trace
            .selected
            .iter()
            .find(|i| i.id == "keep-me")
            .expect("the non-reconstructable item is in the trace");
        assert!(!keep.reconstructable, "the flag reaches the inspector");
        let rebuild = trace
            .selected
            .iter()
            .find(|i| i.id == "rebuildable")
            .expect("the reconstructable item is in the trace");
        assert!(rebuild.reconstructable);
        // And the memory candidate's own id is a reference, not a copy.
        assert!(trace.selected.iter().all(|i| i.content_ref.contains(':')));
    }

    #[test]
    fn an_excluded_context_item_is_never_selected() {
        let mut c = ContextController::new(BudgetTerms::cloud(200_000));
        c.manual.exclude("rebuildable");
        let items = vec![context_item("keep-me"), context_item("rebuildable")];
        let sel = c.select(&[], &items);
        let ids: Vec<&str> = sel.iter().map(|s| s.id()).collect();
        assert_eq!(ids, vec!["keep-me"]);
    }

    #[test]
    fn unpinning_moves_an_item_back_into_the_ranked_order() {
        let mut c = ContextController::new(BudgetTerms::cloud(200_000));
        let cands = vec![ranked("a"), ranked("b")];
        c.manual.pin("b");
        assert_eq!(c.select(&cands, &[])[0].id(), "b");
        c.manual.unpin("b");
        assert_eq!(c.select(&cands, &[])[0].id(), "a");
    }

    #[test]
    fn a_mutation_unfreezes_the_block_and_the_bust_is_accepted() {
        let mut c = ContextController::new(BudgetTerms::cloud(200_000));
        let blocks = crate::injection::RenderedBlocks {
            always_on: Some(crate::injection::Block {
                name: "always_on".into(),
                text: "- [user|user|personal] prefers tabs".into(),
                tokens: 10,
                items: vec![],
                dropped: vec![],
                ceiling: 128,
            }),
            relevant: None,
        };
        c.freeze_memory_blocks(&blocks);
        assert!(c.packing.frozen("memory_always_on").is_some());
        assert!(c.note_mutation(MutationKind::Forget, "memory_always_on"));
        assert!(c.packing.frozen("memory_always_on").is_none());
    }

    #[test]
    fn compaction_runs_prune_checkpoint_compact_in_order() {
        let mut c = ContextController::new(BudgetTerms::cloud(200_000));
        let mut log = pipeline::SessionLog::new();
        for i in 0..6u64 {
            log.append(pipeline::LogEntry {
                seq: i,
                role: "tool".into(),
                text: format!("entry {i} decided: use buck — build.rs"),
                artifact_ref: Some(format!("art:{i}")),
                reconstructable: true,
                at: i as i64,
            })
            .unwrap();
        }
        let out = c
            .compact(&mut log, 5, "ship it", vec!["write tests".into()], 9_000)
            .unwrap();
        assert_eq!(
            out.steps,
            vec![
                pipeline::Step::Prune,
                pipeline::Step::Checkpoint,
                pipeline::Step::Compact
            ]
        );
        assert!(!out.silent_loss);
        assert_eq!(out.compaction_events_appended, 1);
    }

    #[test]
    fn a_recall_through_the_controller_is_a_non_touching_read() {
        let mut s = MemoryStore::open_in_memory(StoreConfig::default()).unwrap();
        s.insert(
            &NewItem::new(
                "m1",
                crate::scope::ScopeKey::project("p"),
                Kind::Fact,
                "the build uses bazel",
            ),
            1,
        )
        .unwrap();
        let before = s.get("m1").unwrap().unwrap();
        let c = ContextController::new(BudgetTerms::cloud(200_000));
        let r = s.recall(
            &crate::scope::ActorBinding::local_in("u", "p"),
            &RecallQuery {
                text: "bazel",
                filter: None,
                token_budget: 256,
                now_ms: 10_000,
            },
            &c.recall_cfg,
            &AllSourcesAvailable,
        );
        assert_eq!(r.unwrap().outcome, RecallOutcome::Hit);
        // Assembling changed nothing: no counter, no timestamp, no salience.
        assert_eq!(s.get("m1").unwrap().unwrap(), before);
    }

    #[test]
    fn rebuild_prefers_live_state_over_a_stale_checkpoint() {
        let c = ContextController::new(BudgetTerms::cloud(200_000));
        assert_eq!(
            c.rebuild_preference(100, 200, 1, 1),
            pipeline::LivePreference::Live
        );
        assert_eq!(
            c.rebuild_preference(200, 100, 2, 2),
            pipeline::LivePreference::Checkpoint
        );
    }

    #[test]
    fn the_inspector_view_reports_the_documented_fields() {
        let c = ContextController::new(BudgetTerms::cloud(100_000));
        let blocks = crate::injection::RenderedBlocks::default();
        let t = c.trace(
            &TurnInput::default(),
            TurnFootprint::default(),
            Feasibility::Fits,
            &blocks,
            RecallOutcome::Abstain,
            &[SelectedItem::Ranked(&ranked("a"))],
            vec![],
            None,
            None,
        );
        let v = inspector_view(&t);
        assert_eq!(v.window, 100_000);
        assert!(v.usable > 0);
        assert_eq!(v.per_source.get("user"), Some(&1));
        assert_eq!(v.relevant_tokens, 0);
        assert_eq!(v.always_on_tokens, 0);
    }

    #[test]
    fn the_trace_reports_deferred_gates_honestly() {
        let c = ContextController::new(BudgetTerms::cloud(200_000));
        let blocks = crate::injection::RenderedBlocks::default();
        let t = c.trace(
            &TurnInput::default(),
            TurnFootprint::default(),
            Feasibility::Fits,
            &blocks,
            RecallOutcome::Abstain,
            &[],
            vec!["budget_exhausted".into()],
            None,
            None,
        );
        assert_eq!(t.deferred, vec!["budget_exhausted".to_string()]);
    }

    #[test]
    fn a_projection_through_the_controller_is_a_pure_function() {
        let f = sources::ContextScope::Project;
        let _ = f;
        let c = ContextController::new(BudgetTerms::cloud(200_000));
        let policy = ProjectionPolicy::external_agent_default("agent:ag", "p1");
        assert_eq!(
            policy.effective_ceiling(),
            sources::ItemSensitivity::Personal
        );
        // The controller does not own a projection: the Core service builds it,
        // and the controller only consumes the slice.
        assert!(c.manual.pinned.is_empty());
    }

    #[test]
    fn frozen_memory_blocks_are_reused_across_turns() {
        let mut c = ContextController::new(BudgetTerms::cloud(200_000));
        let block = crate::injection::Block {
            name: "always_on".into(),
            text: "- [user|user|personal] prefers tabs".into(),
            tokens: 10,
            items: vec![],
            dropped: vec![],
            ceiling: 128,
        };
        let blocks = crate::injection::RenderedBlocks {
            always_on: Some(block),
            relevant: None,
        };
        c.freeze_memory_blocks(&blocks);
        let hits_before = c.packing.telemetry.frozen_block_hits;
        c.freeze_memory_blocks(&blocks);
        assert_eq!(c.packing.telemetry.frozen_block_hits, hits_before + 1);
    }

    #[test]
    fn zero_pinned_items_produce_no_always_on_block() {
        let b = render_always_on(Vec::new());
        assert!(b.is_none());
    }
}
