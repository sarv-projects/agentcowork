//! agentcowork-eval — Verified-Completion Eval Subsystem (P8.0, EV1 —
//! doc 63 §2.3; better-harness evidence-first + skyvern verification loop +
//! codex attestation + openspec verify-gate).
//!
//! The core principle: **the agent's own "finished" is an untrusted claim.**
//! Completion is only proven by an independent, deterministic verifier that
//! checks the *requested final state exists* — files, hashes, content,
//! test-pass — and that no forbidden side effect occurred.
//!
//! The runtime verification contract — the completion-status taxonomy, the
//! task-manifest format, the deterministic verifier SDK and the surface checks
//! — lives in `agentcowork_blueprint::verify` (P69.D32) and is re-exported here
//! so harness callers keep one import site. The harness must never be a runtime
//! dependency: the kernel verifies through the contract, not through this crate.
//! - `evidence` — the evidence bundle (artifact hashes, validator reports,
//!   screenshots, approval events) with explicit missing-evidence reporting.
//! - `report` — the evidence-first loop report (impact / expected-output /
//!   scoped-repair / acceptance-checks; missing evidence stays explicit).
//! - `suite` — the built-in 30-task adversarial desktop suite + fault
//!   injection kinds.
//! - `retrieval` — the retrieval-eval corpus scoring: 7 metrics (evidence
//!   recall/precision, grounding, span fidelity, multi-hop, permission
//!   compliance, injection resistance).
//! - `runner` — the sandbox runner: provision a fixture into a fresh
//!   workspace, inject a fault, run the agent, verify, assemble the result
//!   bundle, and reset.
//! - `corpus` — the built-in corpus data: retrieval corpus + questions +
//!   cases, and deterministic per-task fixtures.
//! - `store` — the evidence-bundle persistent store (JSON on disk, keyed by
//!   task id).
//! - `batch` — batch runs: the full adversarial suite through the sandbox
//!   runner, and retrieval cases through an answering function, both with
//!   per-case distributions + aggregates.

pub mod batch;
pub mod corpus;
pub mod evidence;
pub mod report;
pub mod retrieval;
pub mod runner;
pub mod simulator;
pub mod store;
pub mod suite;
pub mod usage;

// The runtime verification contract is owned by `agentcowork-blueprint::verify`
// (P69.D32) and re-exported here so existing harness call paths
// (`crate::manifest::…`) and external callers keep resolving unchanged.
pub use agentcowork_blueprint::verify::{manifest, status, surface, verifier};

pub use agentcowork_blueprint::verify::manifest::{
    Budgets, Constraint, EvidenceRequirement, HashAlgorithm, OutcomeCheck, TaskManifest,
};
pub use agentcowork_blueprint::verify::status::{CompletionStatus, Score};
pub use agentcowork_blueprint::verify::surface::{
    Surface, SurfaceCheck, SurfaceContext, SurfaceVerdict, verify_surface,
};
pub use agentcowork_blueprint::verify::verifier::{
    OutcomeCheckResult, VerificationReport, VerificationScore, run_outcome_check, verify,
    verify_with_policy,
};
pub use batch::{RetrievalBatchReport, SuiteReport, run_retrieval_batch, run_suite};
pub use corpus::{
    RetrievalCase, builtin_fixtures, builtin_retrieval_cases, builtin_retrieval_corpus,
    builtin_retrieval_questions,
};
pub use evidence::{ApprovalEvent, ArtifactHash, EvidenceBundle};
pub use report::{Finding, LoopReport};
pub use retrieval::{
    EvidenceSpan, ExpectedAnswer, RetrievalDocument, RetrievalQuestion, RetrievalResult,
    RetrievalScores, score_retrieval,
};
pub use runner::{Agent, Fixture, FixtureFile, RunOutcome, SandboxRunner, apply_filesystem_fault};
pub use simulator::{
    CompiledDemo, CompiledStep, SimulationFixture, SimulationReport, Simulator, StepVerdict,
    compile as compile_demo,
};
pub use store::EvidenceStore;
pub use suite::{AdversarialTask, FaultInjection, FaultKind, TaskCategory, builtin_suite};
pub use usage::{
    EfficiencyMetrics, GenericUsageParser, TurnClass, TurnKind, TurnStat, Usage, UsageParser,
    UsageParserRegistry,
};
