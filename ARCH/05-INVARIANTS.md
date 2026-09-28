# 05 — Invariants

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P1). Invariants are rules the system must never violate. Each declares its enforcement point and how it is verified. Changing an invariant requires a `DEC` that supersedes it.
> **Enforcement points** name the owning module; **verification** names the acceptance evidence path (detailed in `ARCH/42-EVIDENCE-MAP.md`).
> **SDD:** requirements cite the invariants they enforce (`ARCH/08-REQUIREMENTS.md`); a violated invariant is a spec deviation under `.agents/docs/spec-driven-development.md`.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).

---

### INV-01 — Core disposes
**Invariant:** Every **Core-mediated externally visible** mutating effect requires authorization minted in Core; surfaces, domains and adapters only propose to that path. A self-contained external agent may execute native effects under its own policy and environment; Core does not attest to those effects (DEC-049/054). Local persistent Core mutations that never leave the machine (e.g. in-store memory writes) are not ticket-bearing: they are policy-gated per scope and audited (DEC-042, INV-24).
**Enforcement:** `12-TRUST` (Guard → Ticket) on the governed path; `13`, `14`; `17` (local-mutation class, DEC-042).
**Verification:** no Core-mediated externally visible effect path exists without a ticket; every Core effect receipt references its ticket; the local-mutation audit census is complete (INV-24); native effects are labelled separately.

### INV-02 — Vault custody
**Invariant:** Provider credentials in Core custody exist only in the vault. They never appear in Core prompts, context, events, logs, receipts, or code. Discovered external agents retain custody of their native credentials; Core neither imports nor claims to protect those stores (DEC-054).
**Enforcement:** `12-TRUST` (vault), `18-MODEL-ROUTING` (use-without-exposure).
**Verification:** secret-corpus scans; prompt/log audits; vault isolation tests.

### INV-03 — One governed path
**Invariant:** Every Core-mediated externally visible effect follows DEC-002's single path. Domains, adapters and UI are not exempt; an external agent's native path is outside this guarantee (DEC-054).
**Enforcement:** `13-CAPABILITY`, `14-PROVIDERS`; review gate at every module doc.
**Verification:** architecture sweep (P6; re-verified in the P9 pass, 2026-09-26); capability tests demonstrating no bypass.

### INV-04 — One authorization decider
**Invariant:** Exactly one Core component decides ALLOW / ASK / DENY for Core-mediated calls. No second Core permission system — including ones inside domains, connectors, or agent adapters. External agents retain their native permission systems (DEC-054).
**Enforcement:** `12-TRUST`.
**Verification:** grep-level check for policy evaluations outside Trust; interop review.

### INV-05 — One egress path
**Invariant:** All Core-mediated outbound network traffic leaves through the guarded egress. Higher Core layers never open side-door connections. A self-contained external agent's native network is outside this path (DEC-054).
**Enforcement:** `12-TRUST` (egress), `14-PROVIDERS`, `19-RUNTIME-ENVIRONMENTS`.
**Verification:** egress tests; static check for direct network clients above the adapter layer.

### INV-06 — One owner per state
**Invariant:** Each Core durable store has one writer (work store, event store, memory store, artifact store, world state). Core has one Work admission scheduler and one logical trigger owner per workflow definition; ownership may transfer only through a fenced handoff (DEC-057). Agent-native and external workflow-provider schedulers are separate domains, never shadow copies of Core state. No second Core registry, queue or policy decider duplicates an existing responsibility.
**Enforcement:** module boundaries (`10`–`37`), Work admission (`11`), Workflow trigger ownership (`20`) and remote fencing (`19`).
**Verification:** interop matrix and ownership table in `ARCH/03-HLD.md` §3; `TC-023/039` exercises duplicate events, crash and owner handoff. The P6/P9 checks covered only the 2026-09-26 baseline.

### INV-07 — Receipts and events
**Invariant:** Every Core-mediated externally visible effect produces a receipt and at least one event. Native agent effects may be observed or reported, with explicit provenance; Mission completion still requires suitable evidence (DEC-054).
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
**Invariant:** External agents receive only projected **Core** views (capability/context/workspace/artifacts/events). Internal topology, stores, policy engines and other agents' state are never exposed. Core-mediated workspace operations enforce boundaries by interception; a self-contained agent's native operations follow its own environment and are not falsely described as intercepted (DEC-054).
**Enforcement:** `12-TRUST`, `16-CONTEXT`, `32-CHANNELS`.
**Verification:** projection tests (7-item contract); deny-outside-path tests.

### INV-12 — Engine parity
**Invariant:** Every agent engine receives the same Core-mediated governance for the same shared capability and grant, whichever codebase it comes from. Native agent tools remain under native policy; no engine receives a privileged shortcut through Core (DEC-054).
**Enforcement:** `15-AGENT-PLANE`, `32-CHANNELS`.
**Verification:** the same shared call through each binding runs through the same Guard/ticket path; binding type alone grants no exception.

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
**Invariant:** Work state is checkpointed; after crash/restart it is reconciled and resumed or replaced according to the adapter's actual capabilities. Native agent sessions need not resume; Mission/Work truth persists independently (DEC-054). In-flight workflow runs execute against their recorded version and never mutate underneath themselves.
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
**Invariant:** Every Core-mediated mutating operation — including forget/delete/wipe — is audited; audit entries are append-only. Agent-native effects outside Core do not receive a Core audit claim (DEC-054).
**Enforcement:** `12-TRUST` (audit), all writers.
**Verification:** mutation census vs audit entries (coverage = 100%).

## DEC-054 boundary and Mission invariants (2026-09-28)

INV-01 and INV-24 quantify over **Core-mediated operations**. DEC-049's external agent native effects are outside Core's ticket/audit path; those effects require honest `native_observed` or `agent_reported` provenance and no Core-governed badge. This is an explicit amendment, not a claim that Core can intercept a discovered agent's own tools. The vault rule remains absolute for credentials in Core custody.

| ID | Invariant | Enforcer |
|---|---|---|
| INV-25 | Mission truth survives agent session/model/provider replacement; no transcript is required to reconstruct it. | `35-MISSION`, `30-EVENTS` |
| INV-26 | A failed Work attempt does not itself fail its PlanNode or Mission. | `35-MISSION`, `11-WORK` |
| INV-27 | GoalContract and PlanVersion commits are immutable; agents propose patches, Mission validates and commits against a base version. | `35-MISSION` |
| INV-28 | Mission completion requires evidence against every required current criterion, or an explicit user-approved exception. | `36-OUTCOME-AND-RECOVERY` |
| INV-29 | Requirement/input/environment changes trigger impact analysis; affected completed nodes/evidence may become invalid. Resume reconciles external state before mutation. | `35`, `36`, `29` |
| INV-30 | Effect verification and mission outcome evaluation are separate; agent self-report is never upgraded to independent proof. | `34`, `36` |
| INV-31 | Repeated no-progress attempts are bounded by budget and escalation policy. | `35`, `36`, `11` |
| INV-32 | Discovered agent configuration and native extensions are read-only to discovery/attachment; host grants are explicit, scoped and reversible. | `46`, `31`, `32` |
| INV-33 | Catalog visibility is not authorization; shared calls use the actual Work/session/agent/grant identity. | `46`, `12`, `13`, `32` |
| INV-34 | Native agent effects and Core-mediated effects carry distinct provenance and assurance claims. | `12`, `34`, `36`, UI |
| INV-35 | Learned skills/workflows/policies are versioned proposals and cannot silently change trusted active behavior. | `37`, `31`, `20` |
