//! agentcowork-memory — memory fusion + token economy (P5, C1–C10).
//!
//! The pure, testable algorithm cores of the memory pillar. Retrieval signals
//! (FTS5, vectors, graph) are supplied by callers; this crate owns the fusion
//! math, the ACT-R decay/recall model, the taste profile, and the compaction
//! pipeline.
//!
//! - `fusion` — weighted RRF multi-signal fusion (Alg #18), dedupe, smart
//!   snippets, per-type budget caps, chunk-min-size merging (Alg #29).
//! - `actr` — ACT-R retention decay + importance floor + associative recall +
//!   spontaneous-recall query derivation (Alg #32).
//! - `taste` — confidence-scored taste profile + stable-prefix injection +
//!   shareable markdown (Alg #31).
//! - `compaction` — snip/soft/force ratios, safe split points, sliding window,
//!   summarize-fail-open, prefix-dirty flag, PRUNE_PROTECT (Alg #21).
//! - `graph` — graph store (entity/episodic + typed edges + temporal
//!   edge-versioning + spreading activation + depth-cap query) (Alg #6/#30).
//! - `paging` — Letta-style 3-surface paging (core/archival/recall) with
//!   queued writes + overflow eviction (Alg #20).
//! - `ghost` — ghost-context prevention index (atomic tombstone + re-path).
//! - `reference` — pass-by-reference handles + bounded previews (C10).
//! - `fsrs` — FSRS-6 spaced-repetition scheduler (C13): memory-state
//!   prediction, next-interval/next-states, and a workload simulator for the
//!   "reinforce what I learned" review queue.
//! - `classify` — intent classifier (Vane pattern): memory/fact/event/
//!   document class + (needs_research, needs_tools, needs_widgets,
//!   rewrite_query) routing signals.
//! - `summary` — hierarchical repo summarization (deepwiki-open pattern):
//!   summarize-file → directory → index → answer over summaries (no
//!   embeddings).
//! - `reinforce` — FSRS-backed review queue: ingest post-session candidates
//!   and surface due review prompts at retention-target intervals.
//! - `bm25` — BM25 keyword retrieval signal (P5/C7): pure Okapi BM25 over
//!   in-memory docs, used as one of the fused retrieval signals.
//! - `planner` — context planner (C7): decides what goes in the prompt from
//!   the retrieved signals (memory, search, tools, widgets) with token
//!   budget + precedence.
//! - `janus` — Janus structural passes (doc 63 §2.1): dedup (exact +
//!   near-dup), regex collapse, and AST prune — the context-reduction
//!   pipeline before injection.
//! - `cognee` — Cognee-style entity/knowledge-graph API (memory ontology
//!   CRUD + query surface, doc 63 §2.1) — graph as a first-class memory
//!   shape alongside snippets.
//! - `rtk` — RTK-style per-command tool-output compression (P5.7): ls/ps/
//!   git/du parsers that keep only action-relevant fields, measured
//!   60–90% reduction.
//! - `usage` — usage accounting (P8): per-key/per-session token + cache
//!   hit/miss ledger with cost at configured prices (the per-key cost
//!   display + cache-hit-rate queries).
//!
//! The v1 durable surface (`ARCH/17-MEMORY.md`) and the context controller
//! (`ARCH/16-CONTEXT.md`) are the modules below `scope`/`store`/`recall`/
//! `write_path`/`injection`/`lifecycle`/`ops`/`context`. They implement the
//! frozen v1 contract: one SQLite + FTS5 store with a keyed-digest suppression
//! set, actor-derived scope sets, ADD-only extraction, budget-as-maximum
//! injection, and a read-only context seam. The older algorithm modules above
//! (graph, ACT-R, FSRS, paging, taste) are the pre-v1 surface; they are retained
//! because callers still use them, and v1 does not require them to function.

pub mod abort;
pub mod actr;
pub mod avoid;
#[cfg(test)]
mod bench;
pub mod bm25;
pub mod branch;
pub mod cache;
pub mod classify;
pub mod cognee;
pub mod compaction;
pub mod context;
pub mod embedding;
pub mod fsrs;
pub mod fusion;
pub mod ghost;
pub mod graph;
pub mod injection;
pub mod janus;
pub mod journey;
pub mod lazy_graph;
pub mod lifecycle;
pub mod maintain;
pub mod ops;
pub mod paging;
pub mod passport;
pub mod planner;
pub mod recall;
pub mod reference;
pub mod reinforce;
pub mod repair;
pub mod rerank;
pub mod rtk;
pub mod saved;
pub mod scope;
pub mod seek;
pub mod store;
pub mod summary;
pub mod taste;
pub mod usage;
pub mod write_path;

pub use actr::{
    DEFAULT_IMPORTANCE_FLOOR, Memory, RecallWeights, activation, derive_queries, forget_sweep,
    is_protected, keyword_hits, recall_score, recency,
};
pub use avoid::{AvoidRule, AvoidanceStore};
pub use bm25::{
    Bm25Doc, Bm25Index, Hit, SignalKind, SignalRank, SignalSource, fuse_signals,
    run_signals_parallel, tokenize,
};
pub use cache::{ResultCache, SemanticCache};
pub use classify::{ExecutionPlan, Intent, IntentKind, classify, parallel_groups, plan_execution};
pub use cognee::{CogneeMemory, RecallResult};
pub use compaction::{
    CacheBreak, CompactionConfig, CompactionCoordinator, CompactionEvent, ContextAction,
    FallbackStep, PersistDecision, PrefixCache, Summarizer, compact_with_fallback,
    decide_context_action, find_safe_split, persist_decision, prune_protect,
    run_compaction_lifecycle, should_snip, sliding_window, snip_anchor, summarize_or_passthrough,
    truncate_with_marker,
};
pub use embedding::{
    BinaryVector, Embedder, EmbeddingIndex, Int8Vector, cosine, dot, hamming, l2, quantize_binary,
    quantize_int8,
};
pub use fsrs::{
    DEFAULT_PARAMETERS, FSRS5_DEFAULT_DECAY, FSRS6_DEFAULT_DECAY, Fsrs, FsrsError, ItemState,
    MemoryState, NextStates, Rating, SimulationConfig, SimulationReport, simulate,
};
pub use fusion::{
    ContentType, Signal, approx_tokens, budget_tokens, cap_text, dedupe, merge_small_chunks,
    rrf_fuse, smart_snippets,
};
pub use ghost::{FsEvent, GhostIndex};
pub use graph::OPEN;
pub use graph::{
    DEFAULT_MAX_DEPTH, DEFAULT_TOP_K, Edge, EdgeType, GraphBackend, GraphStore, Node, NodeKind,
};
pub use janus::{PassResult, ast_prune, dedup, regex_collapse, run_janus};
pub use journey::{Journey, JourneyEvent, JourneyKind};
pub use lazy_graph::{
    LazyConceptGraph, LazyGraphRag, RelevanceAssessor, RetrievalReport, RetrieveOptions,
    RetrievedChunk, SimilarityScorer, extract_concepts, lexical_similarity,
};

// --- the v1 durable memory surface + the context controller -----------------
pub use context::budget::{
    BudgetBreakdown, BudgetError, BudgetTerms, Feasibility, RefusalReason, TurnFootprint,
    check_feasibility, estimate_tokens, refusal_guidance,
};
pub use context::pipeline::{
    DurableBlob, DurableOutput, LivePreference, LogEntry, Pipeline, PipelineError, PipelineOutcome,
    PruneConfig, PruneDecision, SegmentProjection, SessionLog, Step, StructuredCheckpoint,
    bound_preview, prefer_live_state,
};
pub use context::sources::{
    Admission, ContentRef, ContextCandidate, ContextCheckpointRef, ContextItem, ContextScope,
    ContextService, ContextSnapshot, ItemSensitivity, ProjectionPolicy, ReadOnlySource,
    ScopedSlice, SourceError, admit, bound_text,
};
pub use context::stability::{CacheTelemetry, FrozenBlock, PackedTurn, PackingCache, StablePrefix};
pub use context::{
    ContextController, ControllerTrace, InspectorView, ManualControl, SelectedItem, TraceItem,
    TurnInput, TurnPlan, inspector_view,
};
pub use injection::{
    ALWAYS_ON_TOKEN_CEILING, Block, FrozenBlockCache, InjectedItem, Injection, Invalidation,
    MEMORY_FRAMING, MutationKind, RELEVANT_TOKEN_CEILING, RenderedBlocks, always_on_signature,
    estimate_tokens as estimate_block_tokens, fit_block, mutation_invalidates, render_always_on,
    render_relevant,
};
pub use lifecycle::{ForgetAudit, ScopeBound, SessionAnchor, SweepReport};
// `ItemState` here is the memory-ops one (Current/Superseded/Expired); the FSRS
// `ItemState` keeps the unqualified re-export above, so the ops type is
// re-exported under its own name to avoid two meanings for one identifier.
pub use ops::{
    AuditClass, ClaimOutcome, Classification, ContentionOutcome, InspectRow,
    ItemState as MemoryItemState, MemoryHealth, Quarantine, RepairOutcome, RepairPlan, WriterLease,
    check_and_quarantine, claim_writer, export_readable, heartbeat, quarantine_path,
    repair_by_reimport, with_bounded_retry,
};
pub use paging::{CORE_BUDGET_TOKENS, MemoryEntry, PagedMemory, Surface};
pub use passport::{ContextPassport, PassportEntry, PassportScope};
pub use planner::{BudgetResult, ContextPlanner, PlannerConfig, PlannerDecision};
pub use recall::{
    AllSourcesAvailable, FTS_CANDIDATE_LIMIT, Provenance, PrunedSources, Ranked, RecallConfig,
    RecallOutcome, RecallQuery, RecallResponse, SourceAvailability, sanitize_fts_query,
};
pub use reference::{
    PREVIEW_BUDGET_TOKENS, RefHandle, RefKind, bounded_preview, make_ref_handle, query_ref,
};
pub use reinforce::{
    ReviewCandidate, ReviewCard, ReviewQueue, extract_candidates, split_sentences,
};
pub use repair::{Repair, repair_tool_json};
pub use rerank::{Candidate, LexicalReranker, RankedHit, Reranker, rerank};
pub use rtk::{CommandKind, CompressedOutput, compress, kind_for};
pub use saved::{MemoryObservation, ObservationSource, SavedVsDiscovered};
pub use scope::{
    AccessSet, ActorBinding, ActorKind, Kind, Scope, ScopeKey, Sensitivity, SourceSurface,
    TrustTier, VocabularyError,
};
pub use store::{
    ContentKey, ExportBundle, ForgetOutcome, GcOutcome, HASH_VERSION, ImportRemap, ImportReport,
    IntegrityReport, JobRow, MemoryItem, MemoryStore, MonotoneClock, NewItem, SCHEMA_VERSION,
    StoreConfig, StoreError, SuppressionRow, WipeOutcome, normalize, wall_clock_ms,
};
pub use summary::{
    FileSummary, answer_over_summaries, index_summaries, summarize_directory, summarize_file,
};
pub use taste::{TasteRule, TasteStore};
pub use usage::{AgentSessionMetrics, UsageLedger, UsageObservations, UsageRecord, UsageSource};
pub use write_path::{
    AddCandidate, CandidateOutcome, Disclosure, ExtractionBudget, ExtractionResult, ExtractorModel,
    ExtractorPolicy, ExtractorVerb, GateDecision, GateReason, Harvest, HarvestedTurn,
    RejectionReason, RunAudit, RunOutcome, Signals, SupersedeCandidate, WriteConfig, WritePath,
    looks_like_secret,
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
