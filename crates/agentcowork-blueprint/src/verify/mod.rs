//! The runtime verification contract (P69.D32).
//!
//! The task-manifest format, the completion-status taxonomy, the deterministic
//! verifier SDK and the surface checks are **runtime contracts**: the kernel
//! verifies a work item's claimed completion (`eval_service` in
//! `agentcowork-core`) and a blueprint task's `verify:` block
//! ([`crate::blueprint`]) without ever linking the evaluation harness.
//!
//! They therefore live here, upstream of the harness, and
//! `agentcowork-eval` depends on *them* — never the other way round. The harness
//! (runner, adversarial suite, corpus, evidence store, batch reports) stays in
//! `agentcowork-eval`, which is a tool, not a runtime dependency.
//!
//! The core principle is unchanged: **the agent's own "finished" is an
//! untrusted claim.** Completion is proven by an independent, deterministic
//! verifier that checks the requested final state exists — files, hashes,
//! content, test-pass — and that no forbidden side effect occurred.

pub mod manifest;
pub mod status;
pub mod surface;
pub mod verifier;

pub use manifest::{
    Budgets, Constraint, EvidenceRequirement, HashAlgorithm, OutcomeCheck, TaskManifest,
};
pub use status::{CompletionStatus, Score};
pub use surface::{Surface, SurfaceCheck, SurfaceContext, SurfaceVerdict, verify_surface};
pub use verifier::{
    OutcomeCheckResult, VerificationReport, VerificationScore, run_outcome_check, verify,
    verify_with_policy,
};
