# 05 — Invariants

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P1). Invariants are rules the system must never violate. Each declares its enforcement point and how it is verified. Changing an invariant requires a `DEC` that supersedes it.
> **Enforcement points** name the owning module; **verification** names the acceptance evidence path (detailed in `ARCH/42-EVIDENCE-MAP.md`).
> **SDD:** requirements cite the invariants they enforce (`ARCH/08-REQUIREMENTS.md`); a violated invariant is a spec deviation under `.agents/docs/spec-driven-development.md`.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).

---

### INV-01 — Core disposes
**Invariant:** Every **externally visible** mutating effect requires authorization minted in Core; agents, surfaces and adapters only propose. No surface, domain, adapter or agent may execute an externally visible effect on its own authority. Local persistent mutations that never leave the machine (e.g. in-store memory writes) are not ticket-bearing: they are policy-gated per scope and audited (DEC-042, INV-24).
**Enforcement:** `12-TRUST` (Guard → Ticket) on the governed path; `13`, `14`; `17` (local-mutation class, DEC-042).
**Verification:** no externally visible effect path exists without a ticket; every receipt references its ticket; the local-mutation audit census is complete (INV-24).

### INV-02 — Vault custody
**Invariant:** Provider credentials exist only in the vault. They never appear in prompts, context, events, logs, receipts, or code.
**Enforcement:** `12-TRUST` (vault), `18-MODEL-ROUTING` (use-without-exposure).
**Verification:** secret-corpus scans; prompt/log audits; vault isolation tests.

### INV-03 — One governed path
**Invariant:** Every externally visible effect follows DEC-002's single path. Domains, adapters, UI and the native agent are not exempt.
**Enforcement:** `13-CAPABILITY`, `14-PROVIDERS`; review gate at every module doc.
**Verification:** architecture sweep (P6; re-verified in the P9 pass, 2026-09-26); capability tests demonstrating no bypass.

### INV-04 — One authorization decider
**Invariant:** Exactly one component decides ALLOW / ASK / DENY. No second permission system — including "small" ones inside domains, connectors, or agent adapters.
**Enforcement:** `12-TRUST`.
**Verification:** grep-level check for policy evaluations outside Trust; interop review.

### INV-05 — One egress path
**Invariant:** All outbound network traffic leaves through the guarded egress. Higher layers never open side-door connections.
**Enforcement:** `12-TRUST` (egress), `14-PROVIDERS`, `19-RUNTIME-ENVIRONMENTS`.
**Verification:** egress tests; static check for direct network clients above the adapter layer.

### INV-06 — One owner per state
**Invariant:** Each durable store has exactly one writer (work store, event store, memory store, artifact store, world state). No second registry, scheduler, or queue exists anywhere.
**Enforcement:** module boundaries (`10`–`34`).
**Verification:** interop matrix + ownership table in `ARCH/03-HLD.md §3`; P6 sweep (re-verified in P9, 2026-09-26).

### INV-07 — Receipts and events
**Invariant:** Every externally visible effect produces a receipt and at least one event. No silent effects; no "completed" without evidence.
**Enforcement:** `29-ARTIFACTS` (receipts), `30-EVENTS`, `34-EFFECT-VERIFICATION`.
**Verification:** effect-path tests; receipt replay tests.

### INV-08 — Context assembly is read-only
**Invariant:** Assembling context (repo maps, memory recall, file excerpts, world queries) never mutates durable state. Recall is a non-touching read.
**Enforcement:** `16-CONTEXT`, `17-MEMORY` (recall semantics).
**Verification:** mutation-free recall tests; counters bump only on explicit use.

### INV-09 — Memory write discipline
**Invariant:** Memory writes derive from settled history, run off the hot path, and never fail a turn. Secrets are rejected at write. User forget is permanent — suppression blocks re-extraction **and import**, and erasure follows the declared policy/threat model (DEC-039).
**Enforcement:** `17-MEMORY` (pipeline), `12-TRUST` (authorization/audit).
**Verification:** extractor-failure isolation test; secret corpus never persisted; forget→re-extract = 0; forget→import = 0.

### INV-10 — Sensitivity ceilings
**Invariant:** Sensitivity uses one canonical vocabulary (`public | personal | confidential`); every item's class is assigned at write (default `personal`; user actions may raise; monotone floor from source scope/surface — DEC-038). Recall enforces sensitivity ≤ the ceiling derived from the actor binding (never caller parameters); `confidential` items never leave their owning project scope.
**Enforcement:** `17-MEMORY`, `12-TRUST` (projection), `32-CHANNELS`.
**Verification:** write-assignment and ceiling-matrix tests; cross-project leakage = 0; external-agent view tests.

### INV-11 — Projections only
**Invariant:** External agents receive projected views only (capability/context/workspace/artifacts/events). Internal topology, stores, policy engines and other agents' state are never exposed. Workspace boundaries are enforced by interception, not by discovery.
**Enforcement:** `12-TRUST`, `16-CONTEXT`, `32-CHANNELS`.
**Verification:** projection tests (7-item contract); deny-outside-path tests.

### INV-12 — Engine parity
**Invariant:** every agent engine is governed identically, whichever codebase it comes from and however it is bound. No engine gets a privileged shortcut, even temporarily.
**Enforcement:** `15-AGENT-PLANE`, `32-CHANNELS`.
**Verification:** an in-process engine runs through the same guard/ticket path in tests as an external adapter; no binding kind is exempt from the parity test.

### INV-13 — Token discipline
**Invariant:** Deterministic operations (render, browse, list, open, preview, navigate, index search, deterministic user-triggered domain ops) never require an LLM call.
**Enforcement:** `16-CONTEXT`, `22`–`28`, UI.
**Verification:** token-accounting tests for deterministic flows; traces show zero model calls.

### INV-14 — Kernel minimality
**Invariant:** The kernel contains no domain logic. Domains execute; Trust governs; the kernel never special-cases a domain's path.
**Enforcement:** `10-KERNEL` rule; module reviews.
**Verification:** dependency-direction check; P6 sweep (re-verified in P9, 2026-09-26).

### INV-15 — Transport isolation
**Invariant:** Protocol awareness (MCP/ACP/CLI/HTTP) exists only inside provider/channel adapters. Nothing above Capability knows which transport executed an operation.
**Enforcement:** `14-PROVIDERS`, `32-CHANNELS`.
**Verification:** capability tests run identically against different transports.

### INV-16 — Durable work
**Invariant:** Work state is checkpointed; runs resume after crash/restart. In-flight workflow runs execute against their recorded version and never mutate underneath themselves.
**Enforcement:** `11-WORK`, `20-WORKFLOW`.
**Verification:** kill/restart tests; long-wait resume tests.

### INV-17 — Approval primitive
**Invariant:** Human-in-the-loop decisions (agent questions, workflow approval nodes) route through one approval primitive, recorded in events and receipts.
**Enforcement:** `12-TRUST` (approvals), `20-WORKFLOW` (approval node).
**Verification:** approval tests for both agent and workflow paths; audit trail.

### INV-18 — Artifact discipline
**Invariant:** Artifact versions are immutable once written; provenance is always recorded; Library promotion is explicit; deletion is audited.
**Enforcement:** `29-ARTIFACTS`.
**Verification:** version-immutability tests; provenance coverage; promotion audit.

### INV-19 — Risk-proportional verification
**Invariant:** Before a receipt, verification runs at a depth determined by the capability's risk class (`safe` / `sensitive` / `dangerous`).
**Enforcement:** `34-EFFECT-VERIFICATION`, capability descriptors (`13`).
**Verification:** risk-class → verification matrix tests; receipts record verification performed.

### INV-20 — World consent and bounds
**Invariant:** World Model collectors are consent-scoped, bounded and incremental. Raw-volume or system-level reads are gated and metadata-first. No stealth collection.
**Enforcement:** `21-WORLD-MODEL` (+ per-collector consent in `12-TRUST`).
**Verification:** collector consent tests; incremental-update bounds; no full rescan per query.

### INV-21 — No evasion tooling
**Invariant:** Browser and computer-use never implement CAPTCHA solving, anti-bot evasion, fingerprint spoofing, or proxy rotation.
**Enforcement:** `23-BROWSER`, `24-COMPUTER-USE`.
**Verification:** capability catalogue review; no dependency on evasion services.

### INV-22 — Budget honesty
**Invariant:** Context and memory budgets are maxima: zero **query-relevant** hits ⇒ zero tokens in the relevant injection block; the always-on block is separately budgeted and present only when pinned items exist. Injection is measured on rendered output and degraded by dropping whole items — never by truncating an item.
**Enforcement:** `16-CONTEXT`, `17-MEMORY`.
**Verification:** budget tests (p50/p95 injected tokens vs ceiling); zero-query-hit = zero relevant-block tokens test, with the always-on block measured separately.

### INV-23 — Single event log
**Invariant:** UI projections, workflow triggers, world updates, telemetry and audit derive from one event store. No hidden side channels for state propagation.
**Enforcement:** `30-EVENTS`.
**Verification:** event coverage tests; no direct cross-module state writes without events.

### INV-24 — Audit completeness
**Invariant:** Every mutating operation — including forget/delete/wipe — is audited; audit entries are append-only.
**Enforcement:** `12-TRUST` (audit), all writers.
**Verification:** mutation census vs audit entries (coverage = 100%).
