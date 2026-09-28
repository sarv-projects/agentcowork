# 36 — Outcome evaluation, evidence and long-horizon recovery

> Status: accepted target architecture 2026-09-28 under DEC-054; implementation pending. `34-EFFECT-VERIFICATION.md` retains ownership of individual Core-mediated effects. This document owns mission-level interpretation and recovery policy.

## Two verification levels

Effect verification asks whether one Core-mediated operation produced its expected observable state; it uses `34`'s observe → validate → render → verify → reconcile path. Mission outcome evaluation asks whether the **user's goal** and versioned requirements are satisfied. It consumes effect receipts, artifact validations, deterministic tests, independent reviewer findings and external observations. Agent-native actions may contribute observed evidence, but a self-report is identified as `agent_reported` and cannot be elevated to Core-verified. A requirement result records `evidence_type`, `verifier_identity`, `independence`, `environment_fingerprint`, `observed_at`, and `valid_until` or invalidation condition.

The evaluator first applies deterministic acceptance predicates. Where semantic judgment is unavoidable, a separate evaluator receives the contract, deliverables and bounded evidence, not the worker's private reasoning. Contradictions produce a `conflict` event and `partial`/`not_tested` result until resolved. Completion requires each required criterion to pass or a user-approved exception; optional criteria are shown separately. A pretty artifact, passed unit test or worker claim cannot substitute for missing user-intent evidence. Final bundle lists requested outcome, artifacts, changed resources, requirement results, checks, side effects, uncertainty, costs and remaining work.

## Evidence graph and artifact freshness

Link `Requirement → PlanNode → Work attempt → Receipt/Artifact → VerificationRecord`, plus artifact-input edges. Each artifact keeps creator, mission/node, inputs, versions, validation and downstream dependents (`29-ARTIFACTS`). When a source file, connected record, assumption, contract or environment version changes, only affected evidence and downstream artifacts become `stale` pending revalidation. Artifact states are draft, current, stale, invalid, superseded, verified, published, archived; `verified` is a separate validation flag if the lifecycle state is retained elsewhere. No automatic regeneration mutates user artifacts without a new Work item and applicable policy.

## Recovery ladder

0. Reconcile an ambiguous side effect before any retry; keyless unknown outcomes enter `needs_attention`.
1. Retry a transient operation within its fixed bound and idempotency key.
2. Resume a Step/Work from the last durable checkpoint.
3. Replace a Session with a fresh context packet; preserve Work and Mission.
4. Retry a PlanNode as new Work, with the prior attempt and failure evidence.
5. Apply a versioned PlanPatch or invalidate/reopen a milestone.
6. Pause the affected branch and ask the user if permission, ambiguity or irreversible conflict requires it.

Every level records reason, attempt count, budget and observed state. Recovery uses the existing Work/Workflow event store and scheduler. A process heartbeat proves liveness only; progress requires changed verified outputs, resolved blockers, accepted findings or satisfied requirements. A bounded no-progress detector compares attempt signatures and escalates instead of repeating the same approach. Remote ownership later uses leases and expiry; a single-host runtime should not pay distributed coordination cost until it actually supports multiple hosts.

## Resume after dormancy

Load latest committed contract and plan; rebuild projections; inspect worktree/file hashes and connector/resource epochs; compare with checkpoint fingerprint; mark suspect evidence; create a reconciliation report; invalidate the affected dependency closure; then ask an agent to replan only that portion. Reuse unchanged outcomes. Do not replay a native agent's old tool calls or assume its native session can resume. If supported, native resume is an optimization after reconciliation, not the truth model. Auth expiry, missing browser profile, changed APIs and changed user files are explicit blockers.
