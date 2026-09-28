# 08 — Requirements (L1 behavioral registry)

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P7). **Authority:** the behavioral layer between `AGENTCOWORK-SPEC.md` (WHAT and why) and `ARCH/03-HLD.md` (HOW). Each `REQ-*` is one testable behavior with acceptance and failure cases.
> **Rules:** IDs are stable once accepted; module docs cite `REQ-*` IDs instead of restating behavior; `ARCH/09-FEATURE-MATRIX.md` maps every REQ to its design, task and test. Changing an accepted requirement requires a `DEC` (or the `SPEC` change it derives from).
> **Seed note:** the seed entries below were extracted from `AGENTCOWORK-SPEC.md`, `ARCH/05-INVARIANTS.md` and accepted `DEC-*` entries; each module pass verifies, splits or extends its domain's set. Seeds are pre-freeze drafts — numbering stabilises when a release accepts them.

---

## 1. Purpose and position

This registry answers one question per entry: **what behavior must this system exhibit, and how do we prove it?**

- It does **not** re-explain the product (that is `AGENTCOWORK-SPEC.md`) and does not design (that is `ARCH/03-HLD.md` + module docs). It states *observable behavior* — the contract code must satisfy and tests must verify.
- Requirements are derived from the SPEC, invariants, decisions and flows. When a source and a requirement disagree, the source wins until a `DEC` says otherwise.
- Work units (`TASK-*` in `TODO.md`) and tests (`TEST-*`) reference these IDs; `ARCH/09-FEATURE-MATRIX.md` carries the full chain.

## 2. Identity and domains

**Format:** `REQ-<DOMAIN>-<NNN>` — domain mnemonics, three-digit sequence, never reused.

| Domain | Scope | Owner doc |
|---|---|---|
| `PROD` | Product-wide behaviors that span modules | `AGENTCOWORK-SPEC.md` |
| `KERNEL` | ids, errors, config, time, serialization, minimal-kernel rule | `ARCH/10-KERNEL.md` |
| `WORK` | Work · Step · Task · Session · Run · Checkpoint · Scheduler | `ARCH/11-WORK.md` |
| `TRUST` | Policy · Guard · approvals · tickets · vault · egress · audit | `ARCH/12-TRUST.md` |
| `CAP` | Registry · catalog · resolver · handles · affordances · guidance | `ARCH/13-CAPABILITY.md` |
| `PROV` | Provider adapter contract + native/MCP/ACP/HTTP/CLI/plugin/remote | `ARCH/14-PROVIDERS.md` |
| `AGENT` | The engine contract every agent implements: delegation + subagent lifecycle, isolation modes, receipts | `ARCH/15-AGENT-PLANE.md` |
| `CTX` | Context infrastructure + context control + projections | `ARCH/16-CONTEXT.md` |
| `MEM` | Durable memory: layers, write/read paths, minimal algorithm set | `ARCH/17-MEMORY.md` |
| `MODEL` | Model registry · router · adapters; local discovery; effort mapping | `ARCH/18-MODEL-ROUTING.md` |
| `RTENV` | Process manager · environments · sandbox · lifecycle · health | `ARCH/19-RUNTIME-ENVIRONMENTS.md` |
| `WF` | Workflow IR · triggers · durability · versioning · approvals | `ARCH/20-WORKFLOW.md` |
| `WORLD` | Scanner · registries · world graph · event stream · incremental updates | `ARCH/21-WORLD-MODEL.md` |
| `OFFICE` | Office runtime L1/L2/L3 · resident contexts · render/validate | `ARCH/22-OFFICE.md` |
| `BROWSER` | Managed Chromium + adapters · browser world · ladder | `ARCH/23-BROWSER.md` |
| `CUA` | Computer-use ladder · UI automation · vision fallback · input safety | `ARCH/24-COMPUTER-USE.md` |
| `FILES` | File identity · watchers · leases · indexing | `ARCH/25-FILES.md` |
| `CODE` | RepoGraph/RepoMap · LSP · worktrees · code execution | `ARCH/26-CODE.md` |
| `SEARCH` | Search plane | `ARCH/27-SEARCH.md` |
| `COMMS` | Connectors; email/calendar/messaging; `web.search`/`web.fetch` | `ARCH/28-COMMS.md` |
| `ART` | Artifact + Receipt models · versions · provenance · Library promotion | `ARCH/29-ARTIFACTS.md` |
| `EVENTS` | Event store · bus · replay · subscriptions; cost telemetry | `ARCH/30-EVENTS.md` |
| `SKILL` | Skill registry/loader/resolver; plugin surfaces | `ARCH/31-SKILLS-PLUGINS.md` |
| `CHAN` | Desktop/CLI/ACP/A2A/API/mobile projections; agent gateway | `ARCH/32-CHANNELS.md` |
| `VERIFY` | Validate · render · verify · reconcile; receipt policy | `ARCH/34-EFFECT-VERIFICATION.md` |
| `UI` | Chat rendering, surface behavior, interaction model | `AGENTCOWORK-UI.md` |
| `MISSION` | Durable goal, plan, evidence, resume and outcome | `ARCH/35-MISSION.md`, `ARCH/36-OUTCOME-AND-RECOVERY.md` |
| `ECO` | External-agent coexistence and scoped shared ecosystem | `ARCH/46-ECOSYSTEM-ARCHITECTURE.md` |
| `LEARN` | Workflow/skill capture and controlled promotion | `ARCH/37-WORKFLOW-SKILL-LIFECYCLE.md` |
| `UXQ` | Progressive control and measured quality | `ARCH/38-EXPERIENCE-QUALITY.md` |

| `OBS` | Machine Observer service · sensors · system telemetry · safe OS queries · bounded history | `ARCH/51-MACHINE-OBSERVABILITY.md` |

**Entry format (machine-parseable — fixed heading + field lines):**

```md
#### REQ-<DOMAIN>-<NNN> — <one-line title>
- **Statement:** GIVEN <context>, WHEN <trigger>, THEN <observable outcome>.
- **Priority:** must | should | may
- **Source:** <SPEC section | INV-nn | DEC-nnn | FLOW-nn>
- **Acceptance:** <the observable condition that proves it>
- **Failure cases:** <enumerated failure behaviors>
- **Tests:** TEST-<DOMAIN>-<NNN> | pending
- **Status:** seeded | accepted | implemented | verified | deprecated
```

## 3. Requirement quality rules

1. One behavior per requirement; split anything with two behaviors.
2. If no test could fail on it, it is not a requirement.
3. Failure cases are enumerated before implementation.
4. Non-functional requirements carry numbers (latency, budget, limits, compatibility).
5. No implementation detail — behavior only; design lives in the module docs.
6. `Status: verified` requires evidence (`ARCH/42-EVIDENCE-MAP.md`); unit tests alone do not mark risky classes verified.

## 4. Seed set (P7 draft — verify/extend in the owning module pass)

### Product-wide (`PROD`)

#### REQ-PROD-001 — One governed path for each Core-mediated effect
- **Statement:** GIVEN any **Core-mediated** externally visible effect requested from a surface, domain or adapter, WHEN the effect is proposed, THEN it executes only through the governed path (`Work → Capability → Provider → Handle → Guard → Ticket → Execute → Effect → Verify → Receipt → Event`) and its receipt references its ticket. A self-contained external agent's native effects are outside this guarantee (DEC-054).
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §4 · `ARCH/05-INVARIANTS.md` INV-01/INV-03 · `DEC-002`
- **Acceptance:** a Core-mediated attempted effect without a ticket fails closed and is audited; every Core receipt cites a ticket; native external activity never receives a Core-governed badge without proof of mediation.
- **Failure cases:** bypass attempt via Core domain/adapter/UI → denied; missing ticket → denied + audit entry; Core receipt without ticket → verification failure; native effect labelled Core-mediated → provenance failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-PROD-002 — Credential custody
- **Statement:** GIVEN a Core-owned provider credential is stored, WHEN any Core prompt, context, event, log, receipt or code path is produced, THEN the credential value never appears; Core consumers receive vault-mediated use, never the secret. Discovered agents retain custody of their native credentials (DEC-054).
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §6 · `ARCH/05-INVARIANTS.md` INV-02
- **Acceptance:** secret-corpus scans over prompts/logs/events/receipts find zero credential values; vault isolation test proves no non-vault consumer can read a secret.
- **Failure cases:** leakage via prompt/log/event/receipt → verification failure; TypeScript/sidecar custody attempt → blocked (CRED-2).
- **Tests:** pending
- **Status:** seeded

#### REQ-PROD-003 — One authorization decider
- **Statement:** GIVEN a Core-mediated mutating action requires a decision, WHEN a Core component evaluates it, THEN exactly one Trust component decides ALLOW / ASK / DENY; no second Core permission system exists. An external agent retains its native permission system (DEC-054).
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §6 · `ARCH/05-INVARIANTS.md` INV-04 · `DEC-028`
- **Acceptance:** static inspection finds policy evaluation only in Trust; every domain routes decisions to it; composed layers (confinement × approval policy × exec rules) produce one verdict.
- **Failure cases:** domain-local allow-list → detected by review; conflicting verdicts → impossible by construction (one decider).
- **Tests:** pending
- **Status:** seeded

#### REQ-PROD-004 — Engine parity
- **Statement:** GIVEN any agent engine invokes a shared Core effect capability, WHEN the call mutates state or crosses a boundary, THEN it traverses the same Guard/ticket path as any other binding; native agent tools remain under that agent's policy (DEC-054).
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §11 · `ARCH/05-INVARIANTS.md` INV-12 · `DEC-010`
- **Acceptance:** equivalent shared calls from different bindings receive equivalent Guard/ticket decisions; no binding kind is exempt from that parity test.
- **Failure cases:** binding-specific bypass of a shared Core call → forbidden; native call shown as Core-governed → provenance failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-PROD-005 — Token discipline for deterministic operations
- **Statement:** GIVEN a deterministic operation (render, browse, list, open, preview, navigate, index search, deterministic user-triggered domain ops), WHEN it runs, THEN zero LLM calls are made.
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §9 · `ARCH/05-INVARIANTS.md` INV-13 · `DEC-015`
- **Acceptance:** traces for deterministic flows show zero model calls; token accounting attributes no spend to them.
- **Failure cases:** accidental model call on open/preview/render → traced and fixed; "helpful" summaries on deterministic paths → forbidden.
- **Tests:** pending
- **Status:** seeded

#### REQ-PROD-006 — Evidence-gated readiness
- **Statement:** GIVEN any readiness or capability status shown to a user, WHEN the status is produced, THEN it reflects real acceptance records; mock, preview, catalog or unit-only results are never presented as verified.
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §13 · `ARCH/42-EVIDENCE-MAP.md`
- **Acceptance:** statuses derive from acceptance records; Windows readiness requires a real Windows acceptance record.
- **Failure cases:** status without record → blocked from `verified`; mock data leaking into live status → treated as a defect.
- **Tests:** pending
- **Status:** seeded

### Kernel (`KERNEL`)

#### REQ-KERNEL-001 — Minimal kernel
- **Statement:** GIVEN the kernel surface, WHEN any change adds behavior, THEN domain semantics (Office, browser, files, agents) are rejected — the kernel holds identity, errors, config, time, serialization and the base envelope only.
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-14 · `ARCH/10-KERNEL.md` §8
- **Acceptance:** dependency-direction check shows everything depends on the kernel and the kernel depends on nothing in `10`–`34`; a domain special-case in kernel code fails review.
- **Failure cases:** domain logic placed in the kernel → rejected; kernel surface change without a `DEC` → rejected.
- **Tests:** pending
- **Status:** seeded

#### REQ-KERNEL-002 — Single-writer identity
- **Statement:** GIVEN any durable entity id, WHEN it is minted, THEN it is a uuidv7 minted by the owning service (never by callers or UI), and ids stay opaque (no state, version or meaning encoded).
- **Priority:** must
- **Source:** `ARCH/10-KERNEL.md` §2 · `ARCH/05-INVARIANTS.md` INV-06
- **Acceptance:** no id-minting path outside owning services; derived short ids never used for lookup without resolving through the owner.
- **Failure cases:** caller-minted id → rejected; id collision (uuidv7) → `Internal` error (a bug, not a case).
- **Tests:** pending
- **Status:** seeded

#### REQ-KERNEL-003 — Typed, safe error taxonomy
- **Statement:** GIVEN any boundary error, WHEN it surfaces, THEN it uses the canonical taxonomy codes with correct retryability, carries no secrets or user content, preserves cause chains for diagnostics, and never leaks internals across boundaries.
- **Priority:** must
- **Source:** `ARCH/10-KERNEL.md` §3 · `ARCH/05-INVARIANTS.md` INV-11
- **Acceptance:** every boundary error maps to a taxonomy code; secret-corpus scan of error surfaces is clean; `GuidanceRequired`/`RequiresUserAction` arrive as results with next steps, not failures.
- **Failure cases:** untyped error crossing a boundary → review failure; retryability misclassified → defect; internals leaked → verification failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-KERNEL-004 — Configuration layering and validation
- **Statement:** GIVEN a setting, WHEN it is loaded, THEN the typed Core registry applies only that key's permitted scopes and precedence with source/revision recorded; device cosmetics may persist locally, while policy and connector grants remain Core-owned and cannot be overridden by later UI preferences; unknown/invalid keys are rejected with actionable migration/error information; secrets appear only as vault references (DEC-056).
- **Priority:** must
- **Source:** `ARCH/10-KERNEL.md` §4 · `ARCH/05-INVARIANTS.md` INV-02
- **Acceptance:** per-key permitted-scope precedence tests; unknown/invalid-key rejection; policy/grant override rejection; no secret value in config stores; effective configuration affecting running work is versioned into that work's record.
- **Failure cases:** invalid layer/key → typed error and valid fallback only if schema permits; stale UI copy or run override weakens a grant/policy → rejected; silent acceptance of unknown key → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-KERNEL-005 — Time discipline
- **Statement:** GIVEN timestamps, durations and schedules, WHEN they are stored or measured, THEN times are integer epoch milliseconds UTC, durations/timeouts use the monotonic clock (never wall-clock deltas), and schedules carry an explicit timezone policy resolved at the boundary.
- **Priority:** must
- **Source:** `ARCH/10-KERNEL.md` §5
- **Acceptance:** stored-time format tests; timers unaffected by simulated wall-clock jumps; DST-boundary schedule test.
- **Failure cases:** wall-clock delta used for a timeout → defect; naive local-time storage → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-KERNEL-006 — Canonical serialization and store conventions
- **Statement:** GIVEN any boundary serialization or durable store, WHEN data crosses or persists, THEN canonical JSON (stable field order, integer-safe numbers) is used at boundaries, SQLite WAL with one writer per store is the durable convention, migrations are forward-only, idempotent and tested, and content is referenced rather than copied.
- **Priority:** must
- **Source:** `ARCH/10-KERNEL.md` §6 · `ARCH/05-INVARIANTS.md` INV-06
- **Acceptance:** cross-language round-trip tests on canonical JSON; no cross-module direct DB access; migration suite idempotent; a half-migrated store can never serve traffic (blocked cleanly).
- **Failure cases:** float-precision id/size corruption → defect; migration failure → dependent feature blocked with exact migration + error reported.
- **Tests:** pending
- **Status:** seeded

#### REQ-KERNEL-007 — Base envelope on every contract
- **Statement:** GIVEN any `CTR-*` invocation, WHEN it is called, THEN it carries actor context (user/agent/workflow with scope + a `permissions_ref` naming the admission decision, resolved by Trust — never a permission snapshot in the envelope; DEC-050), cooperative cancellation with deadline propagation, idempotency keys for effects (`work_id` + ticket), and a versioned `{ ok, value } | { error }` result envelope.
- **Priority:** must
- **Source:** `ARCH/10-KERNEL.md` §7 · `ARCH/07-CONTRACTS.md` · `ARCH/05-INVARIANTS.md` INV-16
- **Acceptance:** contract conformance tests show the envelope on every boundary; cancellation leaves durable state consistent; duplicate effect invocation with the same key dedupes where the provider supports it.
- **Failure cases:** missing actor context → rejected; cancellation corrupting durable state → violation; retry double-applying an effect → treated as verification failure.
- **Tests:** pending
- **Status:** seeded

### Work (`WORK`)

#### REQ-WORK-001 — One host lifecycle and Work admission scheduler
- **Statement:** GIVEN any Core-scheduled chat turn, job, workflow run, host-assigned subagent task or automation, WHEN it is created, THEN it is a `Work` item on the single host lifecycle and admitted by the one Work scheduler. Agent-native children and external workflow-provider internals are not fabricated as host Work (DEC-054/057).
- **Priority:** must
- **Source:** `ARCH/04-DECISIONS.md` DEC-003 · `ARCH/11-WORK.md` §1 · `ARCH/05-INVARIANTS.md` INV-06
- **Acceptance:** every Core-scheduled execution kind appears as Work with the same status machine; static check finds no duplicate Core admission scheduler; native child telemetry is labeled by provenance.
- **Failure cases:** Core-scheduled kind running outside Work, fabricated native child Work or duplicate Core queue → architecture violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORK-002 — Append-only log, projections only
- **Statement:** GIVEN any session/history view (ui-history, prompt-history, inbox, runs), WHEN it is produced, THEN it is folded from the append-only session log; no mutable session state is authoritative.
- **Priority:** must
- **Source:** `ARCH/04-DECISIONS.md` DEC-027 · `ARCH/05-INVARIANTS.md` INV-23 · `ARCH/11-WORK.md` §2/§4
- **Acceptance:** projections rebuild from the log after a crash; no writer mutates a projection store directly.
- **Failure cases:** authoritative mutable state → defect; log gap → stale projections surfaced, not hidden.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORK-003 — Durable work and resume
- **Statement:** GIVEN a crash or restart, WHEN Work is reconstructed, THEN `running` work first reconciles executor and effect state, then resumes only through a supported live adapter or is safely requeued as a new attempt; `waiting`/`awaiting_approval` remain pending, `cancelled` stays cancelled, and cancellation is recorded — never implied.
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-16 · `ARCH/11-WORK.md` §4
- **Acceptance:** crash/restart test matrix per status and adapter resume capability; unsupported native session resume is never claimed; interrupted effects are verified before retry and unknown keyless effects enter `needs_attention`.
- **Failure cases:** duplicate effect after resume → verification failure; silently dropped queue → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORK-004 — Scheduler lanes and enforced outer bounds
- **Statement:** GIVEN concurrent demand, WHEN work is admitted, THEN lanes (foreground 1/session · background bounded globally and per tree · detached rehydrated and bounded) and outer limits (max agents, workers/tree, depth, worker tokens, session spend, per-lane concurrency) are enforced at admission, with the decision recorded.
- **Priority:** must
- **Source:** `ARCH/04-DECISIONS.md` DEC-031 · `ARCH/11-WORK.md` §3
- **Acceptance:** admission tests reject over-limit work (rejected, never trimmed silently); interactive work preempts background; a paused tree releases slots.
- **Failure cases:** worker-tree explosion → rejected at admission; starvation → starvation guard engages.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORK-005 — Budgets are maxima; overrun pauses and surfaces
- **Statement:** GIVEN per-work budgets (tokens/cost/wall-time) aggregated per tree, WHEN a soft threshold is crossed, THEN a warning event is emitted; WHEN a hard ceiling is reached, THEN work pauses and surfaces — never silently overruns; kill only by explicit policy.
- **Priority:** must
- **Source:** `ARCH/11-WORK.md` §5 · `ARCH/05-INVARIANTS.md` INV-22 · `ARCH/04-DECISIONS.md` DEC-031
- **Acceptance:** budget tests show pause-at-ceiling; usage attributed into runs/receipts/telemetry.
- **Failure cases:** silent overrun → defect; background work silently exceeding session budget → paused + surfaced.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORK-006 — Cancellation semantics
- **Statement:** GIVEN an interrupt request on active work, WHEN it applies, THEN one of three verbs runs — interrupt (stop current step, keep session) · cancel (terminate work) · dispose (release environment/resources) — cooperatively, propagating parent→child, always recording state + reason, with partial effects receipted or verified, never hidden.
- **Priority:** must
- **Source:** `ARCH/11-WORK.md` §6
- **Acceptance:** cancellation-propagation tests; environment release delegated to `ARCH/19-RUNTIME-ENVIRONMENTS.md`; partial effects appear in receipts.
- **Failure cases:** cancellation hiding partial effects → verification failure; orphaned child after parent cancel → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORK-007 — Checkpoint cadence and side-effect safety
- **Statement:** GIVEN work in progress, WHEN steps complete, waits/approvals start, compaction is about to run, or a worker is handed off, THEN a checkpoint is produced (step boundaries as the base cadence), versioned and reconstructable.
- **Priority:** must
- **Source:** `ARCH/11-WORK.md` §4 · `ARCH/16-CONTEXT.md`
- **Acceptance:** checkpoint tests at each trigger; a resume from checkpoint needs no in-memory state.
- **Failure cases:** missing checkpoint before compaction → resume loss; non-reconstructable checkpoint mislabeled → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORK-008 — Runs projection and terminal reasons
- **Statement:** GIVEN a work tree in the UI, WHEN Runs renders, THEN it shows main agent + workers + statuses + budget from typed events; every terminal state carries a reason (receipts + typed error + blockers).
- **Priority:** should
- **Source:** `ARCH/11-WORK.md` §8 · `ARCH/32-CHANNELS.md`
- **Acceptance:** Runs rebuilt solely from events; terminal-state-reason test matrix.
- **Failure cases:** terminal state without reason → defect; UI reading store directly → architecture violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORK-009 — Fork lineage is durable and truthful
- **Statement:** GIVEN a conversation or checkpoint fork, WHEN a branch is created, THEN the new Session records its parent and origin and emits a lineage event; no Work is created until the fork executes a turn; the UI can navigate both directions.
- **Priority:** should
- **Source:** DEC-056 · `ARCH/11-WORK.md` §Sessions · `ARCH/48-EXPERIENCE-SURFACES.md` §Navigation
- **Acceptance:** lineage survives restart and archive/restore; the parent and child are reachable in the rail; a fork without a turn has no phantom Work.
- **Failure cases:** missing parent/origin → typed failure; source deleted/hidden → lineage remains with an unavailable marker; cross-scope fork denied.
- **Tests:** pending
- **Status:** accepted

### Trust (`TRUST`)

#### REQ-TRUST-001 — Egress fail-closed
- **Statement:** GIVEN Core-mediated outbound network traffic, WHEN the guarded egress is unavailable or denies, THEN the connection does not leave by any Core side door — it fails closed (DEC-054).
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §6 · `ARCH/05-INVARIANTS.md` INV-05
- **Acceptance:** egress tests show denied/unavailable egress → no connection; static check finds no direct network clients above the adapter layer.
- **Failure cases:** adapter opening its own socket → architecture violation; timeout falling back to direct → forbidden.
- **Tests:** pending
- **Status:** seeded

#### REQ-TRUST-002 — One approval primitive
- **Statement:** GIVEN a human-in-the-loop decision (agent question, workflow approval node), WHEN it is requested and resolved, THEN it routes through one approval primitive, recorded in events and receipts.
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §6 · `ARCH/05-INVARIANTS.md` INV-17 · `DEC-021`
- **Acceptance:** agent-path and workflow-path approvals use the same primitive; audit trail present.
- **Failure cases:** bespoke approval dialogs → blocked at review; unrecorded approvals → receipt verification failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-TRUST-003 — One authorization decider
- **Statement:** GIVEN any Core-mediated mutating effect, WHEN it executes, THEN the decision is made by the single Trust decider (policy evaluation + ticket mint) — no Core module, prompt, or surface holds a second permission path (DEC-054).
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-04 · `ARCH/12-TRUST.md` §1/§3 · `ARCH/04-DECISIONS.md` DEC-028
- **Acceptance:** static and runtime checks find exactly one Core decider entry point; every Core-mediated effect path resolves through it; a bypass attempt fails closed.
- **Failure cases:** Core-mediated effect executed without a Trust decision → architecture violation; second Core decider introduced → review failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-TRUST-004 — Vault custody, use-only
- **Statement:** GIVEN a Core-owned provider credential, WHEN it is used by Core, THEN it lives only in the vault; Core callers receive scoped use, never the value; secrets never appear in Core prompts, context, events, logs, receipts or UI. Agent-native credentials remain agent-owned (DEC-054).
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-02 · `ARCH/12-TRUST.md` §6 · `ARCH/07-CONTRACTS.md` CTR-013
- **Acceptance:** secret-corpus scans of prompts/logs/receipts/events are clean; no read-value API exists outside the vault; rotation audited.
- **Failure cases:** credential surfaced to a model or log → verification failure; plaintext fallback when vault unavailable → rejected (typed Unavailable).
- **Tests:** pending
- **Status:** seeded

#### REQ-TRUST-005 — Tickets bind and validate
- **Statement:** GIVEN an authorization ticket, WHEN an effect uses it, THEN the ticket is bound (capability, provider, environment, scope, uses, expiry, provider epoch, optional approval) and validated at execution; stale epoch, expiry or revocation ⇒ InvalidState.
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-03 · `ARCH/12-TRUST.md` §4 · `ARCH/06-DATA-MODEL.md` DM-009
- **Acceptance:** ticket-validation tests per binding dimension; replay beyond use-bound fails; epoch-bump invalidation test.
- **Failure cases:** ticket reuse beyond declared uses → rejection + audit; unbounded ticket → review failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-TRUST-006 — Three policy layers stay distinct
- **Statement:** GIVEN policy configuration, WHEN confinement, approval policy, and declarative exec rules are evaluated, THEN the three layers remain distinct and none is collapsed into another.
- **Priority:** must
- **Source:** `ARCH/04-DECISIONS.md` DEC-028 · `ARCH/12-TRUST.md` §2
- **Acceptance:** layer-distinction tests (each layer evaluated independently, decisions composed); no single knob replaces another.
- **Failure cases:** confinement expressed as approval prompt → violation; approval bypassed by rule → rejection.
- **Tests:** pending
- **Status:** seeded

#### REQ-TRUST-007 — Audit completeness
- **Statement:** GIVEN any Trust decision, denial, or forget/delete operation, WHEN it occurs, THEN it is recorded append-only and tamper-evident, and reads of the audit trail are access-controlled.
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-24 · `ARCH/12-TRUST.md` §9
- **Acceptance:** audit-completeness tests over decision classes; tamper-evidence verification; read access control test.
- **Failure cases:** unaudited decision → violation; audit readable by an unauthorized surface → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-TRUST-008 — Projection-only external agents
- **Statement:** GIVEN an external agent boundary, WHEN services are exposed, THEN only the permitted projection (declared capabilities, scoped context, workspace, mediated tools/MCP, artifacts, events) is visible — never topology, stores, queues, scheduler, vault or policy internals, router internals, or other agents.
- **Priority:** must
- **Source:** `ARCH/04-DECISIONS.md` DEC-009 · `ARCH/05-INVARIANTS.md` INV-10/11 · `ARCH/12-TRUST.md` §8
- **Acceptance:** boundary-enumeration test shows no internal surface reachable; projection is least-privilege per binding.
- **Failure cases:** internal endpoint reachable → security violation; cross-agent visibility → rejection.
- **Tests:** pending
- **Status:** seeded

#### REQ-TRUST-009 — Trust infrastructure fails closed
- **Statement:** GIVEN an unavailable or failing Trust dependency (policy engine, vault, egress mediation), WHEN an effect requests approval/credential/egress, THEN the effect is denied with a typed error — never allowed by fallback.
- **Priority:** must
- **Source:** `ARCH/12-TRUST.md` §11 · `ARCH/03-HLD.md` §11
- **Acceptance:** failure-injection tests per dependency; denial surfaced with typed error + audit.
- **Failure cases:** open fallback → catastrophic violation; silent allow → verification failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-TRUST-010 — Catastrophic gate is irreducible
- **Statement:** GIVEN a catastrophic-class action, WHEN it is requested, THEN it is gated by explicit approval even under Full Access; no setting, rule or agent capability removes the gate.
- **Priority:** must
- **Source:** `ARCH/12-TRUST.md` §3 · `AGENTCOWORK-SPEC.md` §6
- **Acceptance:** catastrophic-corpus tests under every policy mode show approval required; no bypass path.
- **Failure cases:** gate bypassed under Full Access → catastrophic violation.
- **Tests:** pending
- **Status:** seeded

### Capability (`CAP`)

#### REQ-CAP-001 — No flat tool dump; budgeted subsets
- **Statement:** GIVEN a compatible bound agent session with shared-capability needs, WHEN Core presents its shared surface, THEN only a task-relevant semantic subset under the configured budget is shown (loading modes eager / catalog / on-demand); Core never dumps its raw catalog or alters the agent's private native catalog (DEC-054).
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §5
- **Acceptance:** Core shared-capability payloads stay within budget; host dump-all behavior is unrepresentable; loading mode is honoured for supported overlays; unsupported attachment is shown as unavailable.
- **Failure cases:** context overflow from tool lists → prevented by budget; missing capability at need → resolver serves it on demand.
- **Tests:** pending
- **Status:** seeded

#### REQ-CAP-002 — Epoch-checked handles
- **Statement:** GIVEN an issued capability handle, WHEN the provider restarts or its epoch advances, THEN the stale handle is rejected and re-resolution is required — never blind retry.
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §5 · `ARCH/06-DATA-MODEL.md` DM-012 · `ARCH/13-CAPABILITY.md` §4
- **Acceptance:** stale-handle test across a simulated provider restart yields a rejection + re-resolution, not a silent reuse.
- **Failure cases:** stale handle used after restart → error surfaced; retry without re-resolution → forbidden.
- **Tests:** pending
- **Status:** seeded

#### REQ-CAP-003 — Capabilities describe what, never who
- **Statement:** GIVEN a capability descriptor, WHEN providers are attached, THEN the capability describes the operation and its risk; provider identity is an assignment, never part of the capability's contract.
- **Priority:** must
- **Source:** `ARCH/04-DECISIONS.md` DEC-004 · `ARCH/13-CAPABILITY.md` §1/§2
- **Acceptance:** capability ids and descriptors carry no provider identity; provider swap preserves the capability contract.
- **Failure cases:** provider baked into capability id or descriptor → review failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-CAP-004 — Descriptor contract and census gate
- **Statement:** GIVEN any capability, WHEN it is registered, THEN it has a versioned descriptor (id, description, affordances, requirements, providers, loading mode, risk class, verification) and passes the census gate (unique id, non-empty affordances and verification).
- **Priority:** must
- **Source:** `ARCH/13-CAPABILITY.md` §2/§8 · `ARCH/06-DATA-MODEL.md` DM-011 · `ARCH/05-INVARIANTS.md` INV-19
- **Acceptance:** census gate rejects incomplete/duplicate descriptors; catalog generation derives providers from the registry.
- **Failure cases:** invocable capability without descriptor → rejection; duplicate id → rejection.
- **Tests:** pending
- **Status:** seeded

#### REQ-CAP-005 — Guidance and requires_user_action are first-class
- **Statement:** GIVEN a capability invocation that cannot complete alone, WHEN it returns, THEN it yields `guidance` or `requires_user_action` with a next action — a result, not a failure; failures are typed errors with retryability.
- **Priority:** must
- **Source:** `ARCH/13-CAPABILITY.md` §3
- **Acceptance:** result-model tests; UI renders guidance as workable next steps (no dead ends).
- **Failure cases:** guidance surfaced as failure → defect; completed result without receipt → violation (INV-07).
- **Tests:** pending
- **Status:** seeded

#### REQ-CAP-006 — Loading modes and semantic compression
- **Statement:** GIVEN a supported shared-capability overlay, WHEN Core capabilities are activated, THEN the declared loading mode applies (eager/catalog/on-demand) within the context budget; activation is scoped per binding/session/Work; Core raw catalogs are never dumped (L1 semantic → L2 structured → L3 raw on demand), while private agent tools remain agent-owned (DEC-054).
- **Priority:** must
- **Source:** `ARCH/04-DECISIONS.md` DEC-005/024 · `ARCH/05-INVARIANTS.md` INV-13 · `ARCH/13-CAPABILITY.md` §6 · `ARCH/16-CONTEXT.md` §3
- **Acceptance:** host budget tests; binding/session/Work activation-scope tests; no flat host dump in host prompt assembly; unsupported overlay does not silently mutate native configuration.
- **Failure cases:** catalog dump → budget violation; unscoped activation leaking across agents → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CAP-007 — Deterministic resolution and failover
- **Statement:** GIVEN a Core capability request with constraints, WHEN providers are selected, THEN candidates lacking grant/policy or environment eligibility are filtered first; eligible candidates are ranked by health → environment fit → cost/latency, ties are broken deterministically and audited, and provider failure fails over only to another eligible candidate. Guard still authorizes each call.
- **Priority:** must
- **Source:** `ARCH/13-CAPABILITY.md` §5/§9
- **Acceptance:** ineligible-provider filtering tests; resolution-order and deterministic-tie tests; failover test with an unhealthy provider; per-invocation Guard decision test.
- **Failure cases:** nondeterministic selection → defect; no-provider → guidance path, never a dead end.
- **Tests:** pending
- **Status:** seeded

#### REQ-CAP-008 — Capability graph resolves requirements
- **Statement:** GIVEN a capability with `requires` edges, WHEN it is invoked, THEN requirement chains resolve before execution and a blocked chain names the missing edge.
- **Priority:** should
- **Source:** `ARCH/13-CAPABILITY.md` §7
- **Acceptance:** graph-resolution tests; blocked-chain error names the unmet requirement.
- **Failure cases:** silent resolution failure → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CAP-009 — Registry governance and versioning
- **Statement:** GIVEN registry evolution, WHEN descriptors change, THEN changes are additive for minor versions; breaking changes require a decision and a deprecation window; deprecated capabilities stay resolvable until the window closes.
- **Priority:** should
- **Source:** `ARCH/13-CAPABILITY.md` §8
- **Acceptance:** versioning-policy tests; deprecation-window enforcement; breaking change without decision → rejected.
- **Failure cases:** silent breaking change → review failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-CAP-010 — Invocable implies governed
- **Statement:** GIVEN anything invocable through Core's shared surface (mapped native domain tool, MCP tool, connector or domain operation), WHEN it is exposed, THEN it is a registered capability with descriptor, risk class and verification hook; a bound external agent's private native tools remain outside that registry and are labelled with native provenance (DEC-054).
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-03/19 · `ARCH/13-CAPABILITY.md` §1
- **Acceptance:** Core shared-surface census shows every Core-invocable tool has a capability id; Core dispatch rejects unregistered ids; private agent activity is never misrepresented as Core-governed.
- **Failure cases:** unregistered tool invocable through Core or private tool falsely labelled as ticketed → architecture violation.
- **Tests:** pending
- **Status:** seeded

### Providers (`PROV`)

#### REQ-PROV-001 — No protocol vocabulary above the provider layer
- **Statement:** GIVEN a caller of Core's shared capability path above the provider layer, WHEN it invokes a capability, THEN it speaks capability ids and does not depend on MCP tool names, ACP methods, HTTP paths or transport details; adapters are interchangeable behind one capability. This does not constrain an external agent's private/native invocations (DEC-054).
- **Priority:** must
- **Source:** `ARCH/14-PROVIDERS.md` §1 · `ARCH/05-INVARIANTS.md` INV-15 · `ARCH/04-DECISIONS.md` DEC-004
- **Acceptance:** Core shared-path interface review finds no provider protocol vocabulary above the adapter boundary; one capability resolves to different providers without caller changes; native agent telemetry stays separately identified.
- **Failure cases:** transport name leaking into capability contracts or prompts → defect; caller bound to one provider → rejected.
- **Tests:** pending
- **Status:** seeded

#### REQ-PROV-002 — Adapter contract and lifecycle
- **Statement:** GIVEN a provider adapter, WHEN it is used, THEN it implements the `ProviderAdapter` contract (discover · connect · health · capabilities · execute · shutdown · events) with the register → connect → serve → shutdown lifecycle, and `execute` runs only with a validated handle and a ticket.
- **Priority:** must
- **Source:** `ARCH/14-PROVIDERS.md` §2 · `ARCH/07-CONTRACTS.md` CTR-010 · `ARCH/05-INVARIANTS.md` INV-03
- **Acceptance:** contract-conformance tests per adapter class; an `execute` call without handle or ticket is rejected.
- **Failure cases:** missing lifecycle call → adapter rejected at review; execute without ticket → `AuthorizationDenied`.
- **Tests:** pending
- **Status:** seeded

#### REQ-PROV-003 — Declared adapter classes
- **Statement:** GIVEN any provider, WHEN it registers, THEN it declares its adapter class — native · mcp · acp · http · cli · plugin · remote — and for `acp` the capability mapping preserves the agent's native tools.
- **Priority:** must
- **Source:** `ARCH/14-PROVIDERS.md` §3 · `ARCH/04-DECISIONS.md` DEC-025
- **Acceptance:** registry entries carry exactly one declared class; `acp` adapters expose tool mappings without flattening native tools.
- **Failure cases:** undeclared class → registration rejected; flattening native tools in an `acp` adapter → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-PROV-004 — MCP client dual-era policy
- **Statement:** GIVEN an MCP server connection, WHEN the era is negotiated, THEN the client tries the modern revision `2026-07-28` (stateless, context in `_meta`, `server/discover`) first and falls back to legacy `2025-11-25` (`initialize`), with per-transport detection (stdio probe with 10 s cap; HTTP 400-body classification), era caching per process/origin, and a per-server force-legacy escape hatch.
- **Priority:** must
- **Source:** `ARCH/14-PROVIDERS.md` §4 · `ARCH/04-DECISIONS.md` DEC-030, DEC-048 · `ARCHIVE/v1-research/mcp-provider-verification.md`
- **Acceptance:** dual-era tests against both revisions; detection and cache tests; force-legacy honored; the client core is the hand-rolled, patch-owned in-crate implementation carrying both revisions (DEC-048 — no SDK core), with the era verdict cached per origin (HTTP) and per command fingerprint (stdio) and the era probe bounded at 10 s on both transports.
- **Failure cases:** permanent mismatch → provider marked incompatible with reason; detection that loses capabilities → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-PROV-005 — MCP server façade compliance
- **Statement:** GIVEN our MCP façade, WHEN an external client connects, THEN it serves stateless modern behavior with `initialize` compatibility, MUST implement `server/discover`, and MUST validate `Mcp-Method` and `Mcp-Name` headers.
- **Priority:** must
- **Source:** `ARCH/14-PROVIDERS.md` §4 · `ARCH/04-DECISIONS.md` DEC-030, DEC-048
- **Acceptance:** façade tests cover modern and legacy clients; `server/discover` present; header validation rejects mismatches; and `initialize` compatibility is reachable **on the strict lease**, method-restricted (only `initialize` is exempt from the revision pin) and session-less (no lease, no session, no capability handle) — `crates/agentcowork-mcp/tests/acceptance_mcp_dual_era.rs::acceptance_a_legacy_initialize_completes_on_the_strict_lease`.
- **Failure cases:** missing `server/discover` → client cannot negotiate; unvalidated headers → request rejected.
- **Tests:** pending
- **Status:** seeded

#### REQ-PROV-006 — Epoch discipline and health-first resolution
- **Statement:** GIVEN adapter instances and provider health, WHEN an adapter restarts or degrades, THEN its `provider_epoch` bumps and outstanding handles are invalidated, and the resolver skips degraded providers before they fail a call; health events publish on the event plane and the UI reads the registry.
- **Priority:** must
- **Source:** `ARCH/14-PROVIDERS.md` §5 · `ARCH/06-DATA-MODEL.md` DM-012 · `ARCH/13-CAPABILITY.md` §4 · `ARCH/30-EVENTS.md`
- **Acceptance:** restart invalidates handles; degraded-before-fail ordering test; per-capability health isolates partial failure; no second store for provider health.
- **Failure cases:** stale handle accepted after epoch bump → `InvalidState`; degraded provider attempted first → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-PROV-007 — Registry entry shape and id mapping
- **Statement:** GIVEN the Core provider registry, WHEN entries are stored, THEN each follows `DM-013` and carries distinct `catalog_ref` and `transport_ref`, with auth modeled as a typed method enum that never holds values. The adapter owns mappings for tools offered through Core; an unmapped tool cannot be invoked **through Core** and yields guidance. External agent-private tools remain native and separate (DEC-047/054).
- **Priority:** must
- **Source:** `ARCH/14-PROVIDERS.md` §5 · `ARCH/06-DATA-MODEL.md` DM-013 · `ARCH/12-TRUST.md` §6 · `ARCH/04-DECISIONS.md` DEC-047
- **Acceptance:** schema tests; several transports may share one catalog entry; auth metadata carries no secret; an unmapped host-exposed tool returns guidance and is never silently exposed by Core; agent-private tools are labelled native.
- **Failure cases:** credential value in registry → custody violation; canonical id aliased with transport id → defect; unmapped tool invocable through Core or private tool labelled Core-governed → exposure violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-PROV-008 — Adapter egress and custody compliance
- **Statement:** GIVEN any adapter network or secret access, WHEN it connects or executes, THEN egress passes Guard (allowlists), secrets are `use`-style vault references only, environment scoping (local · sandbox · remote) is part of the handle, and denial is a typed error with no silent fallback.
- **Priority:** must
- **Source:** `ARCH/14-PROVIDERS.md` §6 · `ARCH/05-INVARIANTS.md` INV-02/05 · `ARCH/07-CONTRACTS.md` CTR-013
- **Acceptance:** static checks find no direct egress clients above the adapter layer; unauthorized egress test returns typed deny; secret scan clean.
- **Failure cases:** silent fallback route after DENY → violation; plaintext secret in adapter config → custody violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-PROV-009 — Gateway client identity and session affinity
- **Statement:** GIVEN a gateway-class provider, WHEN a conversation runs, THEN adapters inject the client-identity User-Agent and the session-affinity header (`x-opencode-session` class) from session identity, and a provider entry carries multiple `transport_ref`s when one gateway hosts several wire protocols.
- **Priority:** must
- **Source:** `ARCH/14-PROVIDERS.md` §5 · `ARCH/04-DECISIONS.md` DEC-035 · `ARCH/18-MODEL-ROUTING.md` §4
- **Acceptance:** header-injection tests per conversation; multi-protocol gateway resolves per transport; identity stable across a conversation.
- **Failure cases:** missing affinity header → provider-side session split (defect); client identity spoofed or missing → rejected.
- **Tests:** pending
- **Status:** seeded

#### REQ-PROV-010 — Typed, bounded provider failures
- **Statement:** GIVEN provider failures (crash · protocol mismatch · schema drift · connect timeout · stream idle · auth expiry · cost mismatch), WHEN they occur, THEN behavior is typed and bounded: epoch-bump failover, at most one era retry, descriptor diff + typed failures for removed capabilities, watchdog aborts with reason, refresh-or-guidance on auth, actual-cost override with audit event — never silent.
- **Priority:** must
- **Source:** `ARCH/14-PROVIDERS.md` §7 · `ARCH/30-EVENTS.md` · `ARCH/34-EFFECT-VERIFICATION.md`
- **Acceptance:** failure-mode matrix tests (one per row); each failure surfaces a typed error or event; audit event on cost mismatch.
- **Failure cases:** unbounded retry loop → defect; silent capability removal → defect.
- **Tests:** pending
- **Status:** seeded

### Context (`CTX`)

#### REQ-CTX-001 — Context assembly is a non-touching read
- **Statement:** GIVEN context assembly (repo maps, memory recall, excerpts, world queries), WHEN it runs, THEN it never mutates durable state; recall counters bump only on explicit use.
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-08
- **Acceptance:** mutation-free recall tests; counters unchanged by assembly alone.
- **Failure cases:** assembly writing back "helpful" state → forbidden; usage counting as a side effect of read → forbidden.
- **Tests:** pending
- **Status:** seeded

#### REQ-CTX-002 — Budget honesty
- **Statement:** GIVEN context or memory budgets, WHEN content is injected, THEN budgets are maxima: zero relevant hits inject zero tokens; degradation drops whole items and never truncates an item.
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-22
- **Acceptance:** budget tests (p50/p95 injected tokens vs ceiling); zero-hit = zero-token test; no partial item in rendered output.
- **Failure cases:** idle tokens injected → budget test fails; truncated item → invalid injection.
- **Tests:** pending
- **Status:** seeded

#### REQ-CTX-003 — Two-layer split: infrastructure vs control
- **Statement:** GIVEN context handling, WHEN responsibilities are assigned, THEN Core provides context infrastructure (search/snapshot/get/checkpoint/projection — “what context exists”) and the bound agent owns context control (assemble/select/prune/compact/pin/exclude — “what the model sees”), and neither layer does the other’s job.
- **Priority:** must
- **Source:** `ARCH/04-DECISIONS.md` DEC-007 · `ARCH/16-CONTEXT.md` §1
- **Acceptance:** Core exposes no policy deciding what the model sees; agent control ops call Core services instead of reaching into sources; for external agents their native context control is preserved (INV-12) with only the projection seam of `12`/`32`.
- **Failure cases:** Core silently selecting or truncating model context → architecture violation; an agent mutating source stores to build context → denied (read-only, INV-08).
- **Tests:** pending
- **Status:** seeded

#### REQ-CTX-004 — References over copies; read-only sources
- **Statement:** GIVEN a context item, WHEN it is assembled or retrieved, THEN items reference their source (`content_ref` + metadata) rather than duplicating content, and every read passes the context service without mutating sources.
- **Priority:** must
- **Source:** `ARCH/16-CONTEXT.md` §1.1/§2 · `ARCH/05-INVARIANTS.md` INV-08
- **Acceptance:** no source-store write path from context assembly; injected content byte-identical to the referenced artifact/record version; changed sources are re-read, never silently stale-copied.
- **Failure cases:** copy divergence → defect (stale context); write through assembly → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CTX-005 — Pre-turn feasibility, named budget terms
- **Statement:** GIVEN a Core-owned model call or an adapter-supported external call with a reported window, WHEN its turn is prepared, THEN the usable window is computed from the named reserves and feasibility is checked before send. For opaque external engines, Core only budgets its offered ContextPacket, labels private window/usage unknown and does not claim to prevent native overflow (DEC-054).
- **Priority:** must
- **Source:** `ARCH/16-CONTEXT.md` §3 · `ARCH/04-DECISIONS.md` DEC-027, DEC-045
- **Acceptance:** Core budget math tests per model class; oversized Core call caught pre-send; telemetry shows named terms; opaque engine state is labelled unknown and supported recovery is negotiated.
- **Failure cases:** preventable Core-side provider overflow, missing reserve term or silent rounding → defect; claiming an opaque native overflow was prechecked → false assurance.
- **Tests:** pending
- **Status:** seeded

#### REQ-CTX-006 — Prune before compact; durable full output
- **Statement:** GIVEN pressure on a Core-owned or adapter-supported context window, WHEN its controllable pipeline reacts, THEN pruning runs before compaction, pruned-away Core output stays durable (artifact/event) and is marked reconstructable, and pruning is opt-in — never applied to log truth or an opaque engine's private context (DEC-054).
- **Priority:** must
- **Source:** `ARCH/16-CONTEXT.md` §4
- **Acceptance:** pruned output retrievable from durable storage; `reconstructable` flag honored (a needed pruned target is recovered, never lost); ordering test (prune strictly precedes compact).
- **Failure cases:** pruned content unrecoverable → verification failure; compaction before pruning → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CTX-007 — Compaction is a projection; log never rewritten
- **Statement:** GIVEN a Core-owned/adapter-supported compaction trigger, WHEN it runs, THEN it produces a checkpoint projection over the Core durable session log without rewriting it; the controllable chain is deterministic pruning → structured checkpoint → optional model-written summary for non-reconstructable residue → optional proven provider-native path. Opaque engine compaction remains native and its private log is not claimed as Core truth (DEC-054).
- **Priority:** must
- **Source:** `ARCH/16-CONTEXT.md` §4 · `ARCH/11-WORK.md` §4
- **Acceptance:** post-compaction log byte-identical except appended compaction events; resume after compaction uses log + checkpoint only; checkpoint fields follow the documented shape (objective/requirements/decisions/completed/active/files/tests/artifacts/workers/blockers/next_actions).
- **Failure cases:** log rewritten in place → architecture violation; compaction losing durable entries → verification failure; unbounded recovery loop → bounded retries, harder compact, then surfaced.
- **Tests:** pending
- **Status:** seeded

#### REQ-CTX-008 — Checkpoint reconstructability; rebuild prefers live state
- **Statement:** GIVEN any checkpoint, WHEN it is used for resume or rebuild, THEN it is versioned and reconstructable from log + artifacts, and `rebuild` prefers live state over a stale checkpoint.
- **Priority:** must
- **Source:** `ARCH/16-CONTEXT.md` §4/§9
- **Acceptance:** reconstruct test from log + artifacts alone; staleness test shows rebuild choosing live sources; version mismatch handled by an explicit migration/rebuild decision.
- **Failure cases:** non-reconstructable checkpoint mislabeled → defect; stale checkpoint silently used → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CTX-009 — Cache stability
- **Statement:** GIVEN repeated Core-owned or adapter-supported model calls, WHEN Core controls packing, THEN its stable prefix remains stable, dynamic content lands in a suffix, injection blocks are frozen until their declared invalidation, and supported turns ship baseline + deltas. An opaque engine's private prefix/cache is not attested by Core (DEC-054).
- **Priority:** must
- **Source:** `ARCH/16-CONTEXT.md` §5
- **Acceptance:** prefix-stability test across turns (byte-stable until a real change); a frozen injection block is not recomputed mid-session **except on the declared memory mutation-invalidation triggers (`REQ-MEM-019`)**; cache-hit telemetry.
- **Failure cases:** recomputing the always-on block per turn → defect (cache churn); dynamic content placed in the prefix → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CTX-010 — Projection scoping, deny-by-default
- **Statement:** GIVEN an external agent or any projection consumer, WHEN a context projection is requested, THEN the slice is scoped (project/workspace/task/step/artifact/user), sensitivity-filtered at Core and re-enforced at Trust, and deny-by-default for anything not explicitly in scope.
- **Priority:** must
- **Source:** `ARCH/16-CONTEXT.md` §1.3 · `ARCH/04-DECISIONS.md` DEC-009 · `ARCH/05-INVARIANTS.md` INV-11
- **Acceptance:** out-of-scope request denied with typed error; confidential items never enter a broader assembly; the `32` contract is enforced together with `12`.
- **Failure cases:** unscoped projection leak → verification failure; sensitivity-filter bypass → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CTX-011 — Context inspector reflects actual bound-engine capabilities
- **Statement:** GIVEN a user opens context detail from the composer or Workbench, WHEN an agent supports context inspection or pin/exclude/focus, THEN one scoped inspector shows those actions and their effective result; unsupported native-agent controls are disabled with a reason and cannot imply that Core controls private context.
- **Priority:** should
- **Source:** DEC-054/056 · `ARCH/16-CONTEXT.md` §Context control · `ARCH/48-EXPERIENCE-SURFACES.md` §Composer
- **Acceptance:** composer and Workbench open the same inspected snapshot; pin/exclude/focus use negotiated support; optimize/compact is an explicit request, not an automatic side effect of opening the panel.
- **Failure cases:** stale snapshot after agent switch → refresh; native context unavailable → truthful limitation; out-of-scope item → denied.
- **Tests:** pending
- **Status:** accepted

### Memory (`MEM`)

#### REQ-MEM-001 — Memory v1 algorithm set
- **Statement:** GIVEN v1 memory, WHEN storage and algorithms are exercised, THEN storage is one Core-owned SQLite file in WAL mode with FTS5 kept in sync by explicit triggers (trigger DDL is part of the store definition), extraction is ADD-only with a `superseded_by` pointer, forget is suppression-based, no vectors, graph, decay or consolidation loops are required for v1 to function, and recall scoring follows `REQ-MEM-015`.
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §7 · `ARCH/04-DECISIONS.md` DEC-018 · `ARCH/06-DATA-MODEL.md` DM-018 · `ARCH/17-MEMORY.md` §3
- **Acceptance:** the v1 acceptance suite passes with no vector/graph/decay component; supersede and suppression-forget semantics verified; a named integrity-check (`integrity-check`/row-count parity) plus `rebuild` procedure with post-rebuild verification restores FTS from the item store; `content_hash` normalization is a versioned function.
- **Failure cases:** any v1 behaviour requiring vectors/graph/decay → defect; FTS drift after mutations → integrity check + rebuild restores it (with post-rebuild verification), and the test fails if it does not.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-002 — Memory write discipline and non-blocking degradation
- **Statement:** GIVEN any memory activity, WHEN extraction, storage or recall fails or a scope is disabled, THEN the turn is never blocked or failed: extraction derives from settled history and runs off the hot path, secrets are rejected at write, user forget is permanent, a storage failure disables memory for the session with a surfaced warning, and a disabled memory performs no injection, retrieval, writes or background extraction.
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-09 · `ARCH/17-MEMORY.md` §1/§8
- **Acceptance:** extractor-failure isolation test (turn unaffected); DB-locked test degrades memory only; secret corpus never persisted; disabled-scope test shows zero activity; forget → re-extract = 0.
- **Failure cases:** memory failure breaking a turn → forbidden; secret persisted → verification failure; disabled memory still writing or injecting → defect; forgotten item re-appearing → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-003 — Memory ≠ context: recall returns candidates
- **Statement:** GIVEN a recall request, WHEN it is served, THEN memory returns ranked candidates with provenance and never decides inclusion — the Context Controller decides what enters the model's context, and memory exposes no injection policy of its own.
- **Priority:** must
- **Source:** `ARCH/04-DECISIONS.md` DEC-019 · `AGENTCOWORK-SPEC.md` §2 (P-10) · `ARCH/16-CONTEXT.md` §1
- **Acceptance:** a recall response contains candidates and provenance only; injection-selection tests live in the context layer; memory exposes no API that writes into a prompt.
- **Failure cases:** memory injecting directly into a turn → architecture violation; recall returning a pre-truncated injection block → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-004 — Non-touching read
- **Statement:** GIVEN context assembly, recall, or inspection, WHEN memory is read, THEN no memory state mutates — no counter, salience or timestamp changes; `used_count`/`last_used_at` bump only on the explicit-use path (or stay deferred), never as a side effect of reading. Mutation-driven invalidation of a frozen injection block (`REQ-MEM-019`) is not a read-side write: “non-touching” governs recall and inspection only.
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-08 · `ARCH/17-MEMORY.md` §6
- **Acceptance:** mutation-free recall test (store contents byte-identical before and after assembly); counters unchanged unless explicit use runs; deferred counters leave no hidden writes; block invalidation occurs only as an explicit response to a mutation, never as a recall side effect.
- **Failure cases:** a read causing a write → violation; implicit usage counting → defect; recall itself recomputing a frozen block → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-005 — Scope model and lifecycle
- **Statement:** GIVEN any memory item, WHEN it is stored or read, THEN it carries exactly one scope — `session` (TTL 7 days after session end) · `task` · `project` · `user` · `org` (schema-ready, disabled in v1) — with lifetime rules enforced: session items expire by default, summaries are pruned by count/age, and pinned items are never auto-pruned.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §2.1/§7 · `ARCH/06-DATA-MODEL.md` DM-018
- **Acceptance:** scope-validation tests (unknown scope rejected); TTL-expiry test; org-scope writes rejected in v1; pinned item survives pruning.
- **Failure cases:** unscoped item persisted → defect; org items written in v1 → rejected; pinned item auto-pruned → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-006 — Isolation, sensitivity and projection
- **Statement:** GIVEN a recall or projection request, WHEN items are selected, THEN recall filters current/unexpired items with sensitivity ≤ the ceiling **derived from the actor binding** (never caller-supplied) over the canonical vocabulary (`public | personal | confidential`, assigned at write with a monotone floor — `REQ-MEM-013`), `confidential` items never leave their owning project scope, the project scope is bound to the stable project identity (`REQ-MEM-020`), and an external agent receives only its permitted projection — owning project scope plus its own session/task plus user preferences; no org, no other projects, no `confidential` without an explicit loadout (v1 default: project + user).
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-10 · `ARCH/04-DECISIONS.md` DEC-009/038 · `ARCH/12-TRUST.md` §8 · `ARCH/17-MEMORY.md` §4/§9 · `REQ-MEM-013/020`
- **Acceptance:** cross-project leakage = 0; external-agent view test shows no org, other-project or confidential items; sensitivity-ceiling matrix per surface rejects above-ceiling items; a caller-supplied scope/ceiling cannot widen the actor's permitted set; superseded items are never served as current.
- **Failure cases:** confidential item in a broader assembly → verification failure; projection leaking another project → security violation; caller-supplied scope honored → security failure; superseded item served as current → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-007 — Extraction trigger discipline
- **Statement:** GIVEN a session or task, WHEN extraction is considered, THEN it runs only at settled boundaries (session idle ≥5 min or task completion) behind a cheap signal gate — no signal means no model call and no write — with debounce bounded to ≈1 run per 10 turns per scope and a bounded harvest (≤20 turns, per-message truncation, top-k existing items of the same scope for link/supersede).
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §5 · `ARCH/04-DECISIONS.md` DEC-018
- **Acceptance:** no-signal test shows zero model calls and zero writes; debounce test; harvest-bound test (harvest stays capped); settled-boundary test (extraction never runs mid-turn).
- **Failure cases:** extraction on the hot path → forbidden; unbounded harvest → defect; signal-gate bypass outside the explicit `memory.remember` path → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-008 — Extractor contract and deterministic validation
- **Statement:** GIVEN a scheduled extraction, WHEN the extractor runs, THEN it is one model call with the closed verb set `ADD | SUPERSEDE (target id required) | NONE`, at most 3 items per run, no scope widening without an explicit user statement, relative dates resolved to absolute, followed by deterministic validation — normalize + hash, drop batch/store duplicates and suppressed hashes, reject secrets with a log (never persist), verify each `SUPERSEDE` target exists, is current and is same-or-narrower scope — all persisted in a single transaction with FTS sync and one audit event.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §5.3–5.5 · `ARCH/04-DECISIONS.md` DEC-018
- **Acceptance:** verb-set test (unknown verb rejected); cap test (over-cap items dropped); scope-widening test rejected without a user statement; secret rejection logged; supersede-target validation matrix; single-transaction rollback test with FTS consistency.
- **Failure cases:** model-rewritten stored text → impossible by construction (verbs only); invalid supersede target → rejected; partial write on failure → transaction rollback and job marked error.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-009 — Supersede semantics and staleness
- **Statement:** GIVEN an item superseded by the extractor or a user edit, WHEN the store is read or written, THEN the original body is never rewritten — a single `superseded_by` pointer marks the reversal, superseded rows are retained for audit but filtered from read paths, and after an explicit supersede the stale original is never served again.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §7 · `ARCH/04-DECISIONS.md` DEC-018
- **Acceptance:** stale-return on explicit supersede = 0 across the conflict suite; superseded rows remain in the store but absent from recall/inspect current views; pointer integrity (target current) test.
- **Failure cases:** stored text rewritten by a model → violation; superseded row served as current → defect; dangling `superseded_by` → integrity failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-010 — Forget and scope wipe are permanent
- **Statement:** GIVEN a user forget or scope wipe, WHEN it executes, THEN forget hard-deletes the item, writes a content-hash suppression (blocking re-extraction **and import** of identical content) and an audit entry; a scope wipe removes that scope's items and superseded rows but retains suppressions; deletion of a superseding item never fails and never resurrects an older item (no dangling `superseded_by`); erasure follows the declared policy/threat model (DEC-039); both are permanent, and replaying the same history cannot resurrect the item (`REQ-MEM-016`).
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-09/INV-24 · `ARCH/17-MEMORY.md` §4/§7 · `ARCH/04-DECISIONS.md` DEC-039 · `REQ-MEM-016`
- **Acceptance:** re-extraction after forget = 0; import after forget = 0; delete-of-superseding-item leaves no resurrection and no FK error; pointer-integrity check passes; wipe leaves no FTS orphans and no lost suppressions; every forget/wipe audited append-only.
- **Failure cases:** forgotten item re-appearing via extraction/import/replay → security failure; delete failing on a pointer reference → defect; scope wipe re-opening re-extraction of a suppressed hash → failure; unaudited delete → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-011 — Inspect, export and import
- **Statement:** GIVEN the Memory UI or tooling, WHEN a user inspects, exports or imports memory, THEN every item shows scope/source/created-at and supports per-item delete and per-scope wipe; `memory.export` / `memory.import` support `json | md`; imports re-run hash and secret checks and land as `source='import'`; an export → import round-trip is byte-identical for unchanged identity, and remapped imports follow the declared id/scope rules (`REQ-MEM-027`).
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §4/§13 · `AGENTCOWORK-SPEC.md` §7 · `REQ-MEM-027`
- **Acceptance:** provenance visible for every item; delete/wipe reachable from the UI; round-trip byte-identical for unchanged identity; imported duplicates dedupe by hash; secret scan runs on import; remapped imports are explicit and audited.
- **Failure cases:** missing provenance → defect; import bypassing validation → rejected; import silently widening scope → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-012 — Recall performance and injection budget honesty
- **Statement:** GIVEN recall and injection under the memory budget, WHEN queries run at 10k items, THEN recall p95 is ≤ 50 ms, zero **query-relevant** hits inject zero tokens **in the relevant block** (the always-on block is separately budgeted and present only when pinned items exist), degradation drops whole items — never truncating an item — with injection measured on rendered output, extraction cost stays visible through `memory.extraction.run` events, and ranking correctness follows `REQ-MEM-015`.
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-22 · `ARCH/17-MEMORY.md` §6/§10 · `REQ-MEM-015/019`
- **Acceptance:** p95 ≤ 50 ms at 10k measured in the eval suite; zero-query-hit test shows zero relevant-block tokens with the always-on block measured separately; whole-item drop only; extraction cost events present per run.
- **Failure cases:** idle tokens in the relevant block → budget test fails; truncating an item → invalid injection; extraction spend without an event → telemetry defect; always-on tokens counted as relevant hits → test-boundary defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-013 — Sensitivity assignment, vocabulary and ceilings
- **Statement:** GIVEN any write into memory, WHEN an item is created or edited, THEN it receives exactly one sensitivity class from the canonical registry vocabulary (`public | personal | confidential`, `ARCH/06-DATA-MODEL.md` §0), defaulting to `personal`; the class may be raised only by a user action or by a deterministic floor rule whereby an item is never less sensitive than its source scope/surface; recall enforces sensitivity ≤ the ceiling derived from the actor binding (not from caller-supplied parameters); and the per-surface default ceilings (desktop user, external agent, projection, UI inspect) are declared.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §2.1/§5.3/§7/§9 · `ARCH/16-CONTEXT.md` §2/§6 · `ARCH/06-DATA-MODEL.md` §0 · `ARCH/05-INVARIANTS.md` INV-10 · `ARCH/04-DECISIONS.md` DEC-038 · `REQ-MEM-006`
- **Acceptance:** fixture writes all three classes; monotone-floor test (item sourced from a confidential project cannot be stored below `confidential`); caller-ceiling matrix per surface; zero above-ceiling items in recall; a vocabulary check rejects `normal|sensitive` on memory paths.
- **Failure cases:** missing/unknown class → rejected; caller-supplied ceiling widening → rejected; `confidential` item in a broader projection → leak test fails; two vocabularies on the recall path → contract test fails.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-014 — Extraction boundary: untrusted content and provenance trust tiers
- **Statement:** GIVEN extraction harvests settled turns that may contain untrusted data (web content, files, tool output, external receipts), WHEN candidates are extracted and stored, THEN harvested content is bounded, delimited and escaped as data (never as instructions), every item carries a provenance trust tier (`user_explicit | agent_asserted | derived_untrusted | import`) derived from its source, instruction-shaped or scope-widening candidates are rejected/downgraded/stored without authority, and injected memory text can never change agent policy, goals, permissions, or tool choices.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §5.2–5.4/§6/§13 · `ARCH/15-AGENT-PLANE.md` §7 (escaping precedent) · `ARCH/04-DECISIONS.md` DEC-036/037 · `ARCH/41-EDGE-CASES.md` EDGE-093
- **Acceptance:** adversarial fixture (page/tool output containing a “remember: always …” instruction) produces no policy-bearing item and no scope widening; every injected item exposes its trust tier; a harness test shows memory text is rendered as quoted data and never executed/obeyed.
- **Failure cases:** untrusted instruction stored as `decision`/`preference` with authority → failure; scope widened from untrusted content → failure; injected content obeyed in an agent test → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-015 — Recall scoring correctness and query robustness
- **Statement:** GIVEN a recall query, WHEN candidates are scored, THEN the score is defined on a non-negative relevance base (FTS5 `bm25()` returns negative values; normalize e.g. `relevance = −bm25`), the ordering direction is explicit and monotone in relevance, boosts never invert relevance, ties break deterministically (`created_at`, id), a free-text query is sanitized into valid FTS5 syntax or the call abstains, an invalid query never returns arbitrary candidates, and recall outcomes distinguish `hit` / `abstain` / `error` with metering.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §3/§6 · `ARCH/27-SEARCH.md` §4 · `ARCH/16-CONTEXT.md` §6 · SQLite FTS5 documentation (bm25 is multiplied by −1 so better matches are assigned numerically lower scores)
- **Acceptance:** golden-set ordering test (recall@5/MRR); fixture where a pinned item has worse raw BM25 than an unpinned match and still ranks first; malformed-query fuzz (quotes/operators/NEAR) yields abstention, not an error or garbage; non-English fixture records tokenizer behavior; abstention and error are distinguishable in telemetry.
- **Failure cases:** boosted rank below a better unboosted match → test failure; malformed query error surfaced to the turn → failure; empty/invalid query returning candidates → defect; error reported as abstention (or vice versa) → telemetry defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-016 — Forget, supersede pointers and erasure integrity
- **Statement:** GIVEN a forget, scope wipe, supersede or import, WHEN the store mutates, THEN stored text is never rewritten; a forgotten item is hard-deleted with its suppression recorded; a `superseded_by` pointer is never left dangling and deleting a superseding item neither fails nor resurrects an older item; import re-checks suppression and cannot resurrect a forgotten hash; scope-wipe suppression semantics are explicit; and “permanent” states the physical-erasure policy (secure delete/checkpoint/VACUUM/FTS rebuild) or an explicit threat-model boundary.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §3/§4/§7 · `ARCH/05-INVARIANTS.md` INV-09/INV-24 · `ARCH/04-DECISIONS.md` DEC-039 · `REQ-MEM-009/010` · SQLite pragma documentation
- **Acceptance:** forget→re-extract = 0; forget→import = 0; delete-of-superseding-item leaves no resurrection and no FK error; wipe leaves no FTS orphans; pointer-integrity check passes; suppression digest uses the decided keyed scheme; audit payload contains no item body.
- **Failure cases:** forgotten item returns via any path (extraction, import, replay) → security failure; delete fails due to a pointer reference → defect; old item resurrects after deleting the new one unless explicitly decided → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-017 — Growth bounds, item caps, retention and expiry
- **Statement:** GIVEN long-running use, WHEN memory grows, THEN every writable scope has a declared bound (max items and/or bytes), a single item is capped at the declared byte size with oversize rejected at validate, session TTLs have a defined anchor (task-scoped items live only as long as the task) plus a sweeper that deletes, syncs FTS and audits per policy, `memory_jobs` is garbage-collected, and the durable scopes' bound (or declared intentional unboundedness) is a named product knob measured by the drift simulation.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §7/§10/§13 · `ARCH/11-WORK.md` §7
- **Acceptance:** oversize item rejected with no partial write; per-scope cap test; TTL-anchor test; sweeper leaves no FTS orphans; jobs-GC test; 1k/10k/50k-item recall benchmarks; drift simulation reports store size and top-k waste.
- **Failure cases:** unbounded growth with no declared bound → freeze-checklist failure; oversize item stored → defect; expired item recalled → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-018 — Concurrency, single-writer and corruption semantics
- **Statement:** GIVEN multiple surfaces and processes (desktop, CLI, detached work, extractor, UI), WHEN memory is written or read, THEN exactly one writer owns the store through a named mechanism, lock contention is bounded (busy timeout + backoff + per-call degradation) and never disables memory for a whole session or fails a turn, and corruption is detected, quarantined, surfaced and repaired via export/rebuild without blocking chat.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §3/§8 · `ARCH/05-INVARIANTS.md` INV-06 · `ARCH/11-WORK.md` §3 · `ARCH/32-CHANNELS.md` §6
- **Acceptance:** two-process write test (contention retries; no lost writes; no turn failure); forced-lock test shows bounded degradation only; corrupt-file test shows quarantine + surfaced repair + chat unaffected; detached/CLI write test.
- **Failure cases:** silent lost write → defect; corruption disabling chat → violation; `BUSY` propagated as a turn failure → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-019 — Mutation invalidation and frozen-injection budget semantics
- **Statement:** GIVEN injection blocks computed under budget, WHEN a memory mutation occurs (forget, edit, pin/unpin, supersede, scope wipe, enable/disable), THEN the next turn reflects it — frozen blocks are invalidated for forget/edit/disable with cache-bust accepted — zero query-relevant hits inject zero tokens in the relevant block, the always-on block is separately budgeted and present only when pinned items exist, and degradation drops whole items.
- **Priority:** must
- **Source:** `ARCH/16-CONTEXT.md` §5/§6/§7 · `ARCH/17-MEMORY.md` §6 · `ARCH/05-INVARIANTS.md` INV-22 · `REQ-MEM-012`
- **Acceptance:** delete-mid-session test (deleted item absent from the immediate next turn's rendered context); pin/edit/disable invalidation test; zero-hit rendering shows zero relevant-block tokens and no empty headers; render test with always-on present/absent.
- **Failure cases:** forgotten item injected after a turn boundary → privacy failure; zero-hit injection > 0 relevant tokens → INV-22 failure; truncated item → failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-020 — Project identity, re-keying and cross-project isolation
- **Statement:** GIVEN a project scope, WHEN it is bound or re-bound, THEN the scope key is a stable project identity (`DM-024 project_identity`), not a raw path; canonicalization handles Windows case/junction/short-name/long-path and POSIX symlink cases; clone/move/rename/worktree behavior is explicit; and no recall path can return an item whose project identity is not in the actor's permitted set.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §2.1 · `ARCH/25-FILES.md` §5/§7 · `ARCH/21-WORLD-MODEL.md` §3 · `ARCH/06-DATA-MODEL.md` DM-024 · `ARCH/04-DECISIONS.md` DEC-040 · INV-10
- **Acceptance:** two-project leakage test = 0 under move/clone/re-key; non-git folder test; worktree sharing test; Windows canonicalization matrix with recorded results.
- **Failure cases:** path-keyed scope attached to a new project at the same path → leak; worktree/clone unexpectedly sharing or losing memory without a recorded decision → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-021 — Checkpoint is authoritative; memory summaries reference, never duplicate
- **Statement:** GIVEN long-horizon work, WHEN summaries exist in both the checkpoint path and memory, THEN the checkpoint (`DM-006`, DEC-027 projection) is authoritative for work state, a memory `summary` item carries a `source_ref` to its checkpoint/session and is never served as work state, a live checkpoint in the assembly supersedes a stale memory summary, and no second timeline exists.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §2.3 · `ARCH/16-CONTEXT.md` §4 · `ARCH/11-WORK.md` §2 · `ARCH/04-DECISIONS.md` DEC-027/041 · INV-16/INV-23
- **Acceptance:** fixture where checkpoint and memory summary disagree → assembled context uses the checkpoint; summary provenance resolves; deleting the checkpoint leaves the summary annotated, not silently authoritative; no recall path serves `summary` as `checkpoint` state.
- **Failure cases:** two divergent timelines injected → defect; memory summary overriding a live checkpoint → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-022 — Multi-agent boundary: external agents and subagents
- **Statement:** GIVEN external agents and subagents, WHEN they interact with memory, THEN v1 exposure is read-only filtered recall (bound project + own session/task + user preferences; no org, no other projects, no `confidential` without a recorded loadout); Core never writes an external agent's native memory/config/session stores; external provider-session transcripts are never harvested; child-session harvesting, if enabled, is explicit with parent linkage; receipts enter extraction only as untrusted data; and imports are user-initiated, read-only to the source, and audited.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §4/§5/§9 · `ARCH/15-AGENT-PLANE.md` §5/§7 · `ARCH/04-DECISIONS.md` DEC-009/025/029/036/043 · `ARCH/32-CHANNELS.md` §3
- **Acceptance:** external-agent view matrix test; test that no write occurs to a fixture external memory directory; receipt-only parent context test; child-session extraction policy test; import audit test.
- **Failure cases:** an external agent's native files written → violation; receipt transcript in parent context → violation; external transcript harvested → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-023 — Mutation classification, authorization and actor-derived scope sets
- **Statement:** GIVEN any memory call, WHEN authorization is evaluated, THEN memory writes are classified as local persistent mutations (policy-gated per scope, audited, not per-write tickets) while boundary-crossing operations (export/import to disk, sharing) follow the guarded path (pathfloor/egress and tickets as applicable); the permitted scope set is derived by the service from the actor context and never trusted from caller parameters; and every mutation — including forget/wipe/import and policy-driven deletes — is audited with no item body in the audit record.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §4/§9 · `ARCH/07-CONTRACTS.md` §0/§5 · `ARCH/05-INVARIANTS.md` INV-01/INV-04/INV-24 · `ARCH/12-TRUST.md` §7/§8/§9 · `ARCH/04-DECISIONS.md` DEC-042
- **Acceptance:** classification-matrix test (local write vs export → effect path); confused-deputy test (caller passes another project's scope → denied by construction); audit coverage census = 100% of memory mutations; audit payload contains no content body.
- **Failure cases:** caller-chosen scope honored → security failure; export bypassing Guard → violation; unaudited mutation → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-024 — Extractor budget, model policy and kill switch
- **Statement:** GIVEN background extraction of eligible Core-owned material, WHEN it runs, THEN it is metered against a declared global budget with global and per-scope kill switches. A Core-owned inference session may use its already active Core provider; an opaque external-agent turn has no presumed Core provider and requires an explicitly selected Core extractor or local model. Native private transcripts are not harvested; `confidential` scopes follow the local/no-extraction policy; cost/outcome is reported per run (DEC-044/054).
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §5.1/§9 · `ARCH/04-DECISIONS.md` DEC-031/044
- **Acceptance:** budget-cap test (N+1st extraction deferred); kill-switch test (zero model calls/writes); opaque-agent turn has no automatic extractor call and no native transcript harvest; explicit Core provider selection is recorded; confidential-scope/concurrency tests; metering event per run.
- **Failure cases:** unbounded extraction calls → cost/security defect; confidential content sent to a disallowed model → violation; kill switch ignored → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-025 — Time correctness and clock-skew resilience
- **Statement:** GIVEN wall-clock timestamps used for ranking, TTL and leases, WHEN the clock moves backwards or jumps, THEN invariants hold: recency ordering never promotes an older item over a newer one because of a backwards clock; TTL uses the declared anchor and neither expires fresh items nor resurrects expired ones; job leases fail safe; and non-monotonic observations are clamped and recorded.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §3/§6/§7 · `ARCH/06-DATA-MODEL.md` §0 · `ARCH/41-EDGE-CASES.md` EDGE-112/EDGE-177 (alignment)
- **Acceptance:** simulated backwards-clock test (ordering + TTL + lease); DST/timezone irrelevance test (epoch ms only); skew event recorded.
- **Failure cases:** a clock jump changes the ranking order of fixed items → defect; TTL reversal → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-026 — Provenance integrity under deletion and edit
- **Statement:** GIVEN provenance references (turn/event/artifact ids), WHEN referenced sources are deleted or pruned, THEN recall never dereferences them, rendering tolerates missing refs with a “source unavailable” annotation, user edits create a new item with `source='user'` while the prior item is superseded (history preserved), pinned-item delete/wipe confirmation is defined, and no item becomes unusable solely because its source is gone.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §3/§4/§7 · `ARCH/04-DECISIONS.md` DEC-032 · `ARCH/06-DATA-MODEL.md` DM-018 · `ARCH/29-ARTIFACTS.md`
- **Acceptance:** deleted-artifact test (item still recalls, annotated); user-edit test (new item + superseded old + provenance); pinned-item wipe confirmation test; provenance rendering with missing refs.
- **Failure cases:** dangling ref breaks injection → defect; user edit rewrites the body in place → violation; lost edit provenance → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MEM-027 — Schema versioning, migration and import/export fidelity
- **Statement:** GIVEN store evolution (U0–U11) and cross-machine import/export, WHEN the schema changes or a file is imported, THEN the database carries a schema version with forward migrations tested from every shipped version, export/import defines id/scope remapping rules (project identity is not assumed portable) and suppression handling, and “byte-identical round-trip” is defined against those remapping rules.
- **Priority:** must
- **Source:** `ARCH/17-MEMORY.md` §4/§12/§13 · `ARCH/06-DATA-MODEL.md` §0
- **Acceptance:** migration test from each version; import into a different project identity with explicit mapping/abstention; suppression-hash check on import; round-trip comparison per the defined rule.
- **Failure cases:** silent schema drift → defect; import attaching items to a wrong project → leak; import resurrecting suppressed content → failure.
- **Tests:** pending
- **Status:** seeded

### Models (`MODEL`)

#### REQ-MODEL-001 — One registry, one router, no hard-coded vendor
- **Statement:** GIVEN a Core-owned inference model — cloud or local — WHEN it is registered or selected, THEN it lives behind exactly one Core model registry/router, no Core module hard-codes a vendor, and Core callers never address a provider endpoint directly. External engines retain their own native model registries and configuration (DEC-054).
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §2 (P-03) · `ARCH/04-DECISIONS.md` DEC-004 · `ARCH/18-MODEL-ROUTING.md` §1
- **Acceptance:** Core registry census shows every Core-selectable model has one entry; static check finds no Core vendor selection outside the router; swapping a Core provider changes no Core caller; an external engine's list is labelled agent-owned.
- **Failure cases:** a module calling a vendor endpoint outside the router → architecture violation; a second registry or router → review failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-MODEL-002 — ModelDescriptor contract
- **Statement:** GIVEN a Core-registered model, WHEN its descriptor is stored, THEN it follows `DM-025` — id, provider, resolved context window and max output, tool-calling support, reasoning modes, vision, streaming, structured output, cost, latency class, locality and tokenizer ref, plus declared lifecycle/visibility, family/release, variants/options, transport and catalog refs, prompt-cache and privacy additions. External engine models are shown from adapter-advertised options, not manufactured Core descriptors (DEC-054).
- **Priority:** must
- **Source:** `ARCH/18-MODEL-ROUTING.md` §2 · `ARCH/06-DATA-MODEL.md` DM-025
- **Acceptance:** schema validation rejects incomplete descriptors; router and UI read declared capabilities only (composer negotiation shows supported options); a privacy-flagged model is excluded from disallowed scopes.
- **Failure cases:** missing required field → registration rejected; undeclared capability offered to a caller → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MODEL-003 — Catalog is data, never code
- **Statement:** GIVEN catalog data and local discovery, WHEN the catalog refreshes, THEN it is vendored/compiled as data with a short TTL (≈5 min), atomic tmp+rename writes, a cross-process lock and an offline vendored snapshot, a scheduled refresh (≈60 min) emits a refresh event, refresh failures are logged and never block the UI, and provider SDKs are never installed at runtime.
- **Priority:** must
- **Source:** `ARCH/18-MODEL-ROUTING.md` §2 · `ARCH/04-DECISIONS.md` DEC-034
- **Acceptance:** refresh tests (TTL honored, atomicity under simulated crash, lock contention); offline start works from the snapshot; a failed refresh does not block the UI; no runtime package-install path exists.
- **Failure cases:** torn catalog write → defect; refresh failure blocking the UI → defect; runtime SDK installation → rejected (DEC-034).
- **Tests:** pending
- **Status:** seeded

#### REQ-MODEL-004 — Deterministic routing and no silent downgrade
- **Statement:** GIVEN a Core-owned model selection or a binding that explicitly delegates model choice, WHEN the Core router resolves, THEN ranking is deterministic and audited from permitted preferences, task requirements, policy, availability and weights; the selection carries a declared fallback chain, and unmet requirements yield guidance, never a silent downgrade. An opaque agent's model is engine-managed and cannot be silently changed by Core (DEC-054).
- **Priority:** must
- **Source:** `ARCH/18-MODEL-ROUTING.md` §3 · `ARCH/07-CONTRACTS.md` CTR-014
- **Acceptance:** determinism test on identical inputs (stable order, audited tie-break); unmet-requirement test returns guidance; failover test with an unavailable provider; a policy-denied model is never selected.
- **Failure cases:** nondeterministic selection → defect; silent downgrade or tool emulation → defect; provider down without failover or typed `Unavailable` → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MODEL-005 — Vault-only credentials in the model plane
- **Statement:** GIVEN adapter auth and endpoint configuration, WHEN a model call is made, THEN credentials are vault `use`-style references only — never values — and keys never appear in logs, prompts, telemetry or stored config.
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-02 · `ARCH/07-CONTRACTS.md` CTR-013 · `ARCH/18-MODEL-ROUTING.md` §4
- **Acceptance:** secret-corpus scan over logs/prompts/telemetry is clean; endpoint config carries references only; the model plane exposes no read-value API.
- **Failure cases:** key in a log or prompt → verification failure; plaintext fallback when the vault is unavailable → typed `Unavailable`, never a stored copy.
- **Tests:** pending
- **Status:** seeded

#### REQ-MODEL-006 — One typed stream union above adapters
- **Statement:** GIVEN any streaming completion, WHEN events reach callers, THEN they use one typed stream-event union (step-start · text/reasoning/tool-input deltas · tool-call/result/error · step-finish · finish · provider-error) with explicit block ids synthesized when absent, step-finish and turn-finish distinct, and cancellation and backpressure mandatory — no consumer branches on provider id.
- **Priority:** must
- **Source:** `ARCH/18-MODEL-ROUTING.md` §4 · `ARCH/04-DECISIONS.md` DEC-034 · `ARCH/15-AGENT-PLANE.md` §4
- **Acceptance:** union-conformance tests per adapter; block-id synthesis test; provider-id branching absent above the adapter; cancellation and backpressure tests.
- **Failure cases:** provider-specific event shape leaking upward → defect; missing step/turn distinction → defect; silent stream end treated as success → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MODEL-007 — Usage and cost accounting invariant
- **Statement:** GIVEN a completed Core-owned model call, WHEN usage and cost are reported, THEN totals are inclusive with a non-overlapping breakdown (`non_cached_input` · `cache_read` · `cache_write` · `reasoning`), values are clamped and consumers never subtract; cost is cache-class-aware, provider-reported actuals override catalog estimates, included/free plans are exactly 0, mismatch is logged, and telemetry carries no prompt/completion content. External-engine usage is `reported`, `estimated` or `unknown` per adapter evidence (DEC-054).
- **Priority:** must
- **Source:** `ARCH/18-MODEL-ROUTING.md` §7 · `ARCH/04-DECISIONS.md` DEC-034 · `ARCH/30-EVENTS.md` §5
- **Acceptance:** usage-invariant tests (breakdown stays within the inclusive total; clamping test); actual-overrides-estimate test; mismatch event test; telemetry content scan clean.
- **Failure cases:** consumer subtracting fields → forbidden by the written invariant; negative/underflow value → clamped; prompt/completion content in telemetry → verification failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-MODEL-008 — Single-owner retry discipline
- **Statement:** GIVEN a failed Core-owned model request, WHEN retries are considered, THEN exactly one Core layer owns each failure class — request-start transport retries (exponential + jitter, honoring `retry-after`) · pre-content stream interruptions via buffer-until-proven with discarded-attempt usage summed · post-content failures at the Core turn level; a user abort vetoes retry and context overflow is terminal at the transport. An external engine's native retries remain native and are not duplicated by Core (DEC-054).
- **Priority:** must
- **Source:** `ARCH/18-MODEL-ROUTING.md` §4 · `ARCH/04-DECISIONS.md` DEC-034
- **Acceptance:** retry-ownership matrix test (one owner per class); user-abort veto test; usage aggregation across discarded attempts; overflow ends without retry.
- **Failure cases:** two layers retrying one failure → defect; adapter retrying a post-content failure → defect; discarded-attempt usage lost → accounting defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MODEL-009 — Watchdogs and typed provider errors
- **Statement:** GIVEN a stalled or failing provider stream, WHEN a watchdog fires or the provider errors, THEN header/chunk-idle/read timeouts abort with an explicit reason, a network-error finish fails the step rather than silently ending it, and every failure maps to the typed taxonomy (`InvalidRequest` · `Authentication` · `RateLimit{retryAfterMs}` · `QuotaExceeded` · `ContentPolicy` · `ProviderInternal` · `Transport` · `ContextOverflow`) with `retryable` derived from the type — rate limits back off and surface, and auth expiry gets one refresh attempt where supported, else re-auth guidance.
- **Priority:** must
- **Source:** `ARCH/18-MODEL-ROUTING.md` §4/§8 · `ARCH/04-DECISIONS.md` DEC-034
- **Acceptance:** watchdog matrix tests (each timeout → typed abort reason); taxonomy mapping test; retryability-derivation test; rate-limit backoff/queue surfaced; auth-expiry path test.
- **Failure cases:** silent hang → defect; silent stream end on network error → defect; untyped error crossing a boundary → defect; unbounded retry on auth failure → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MODEL-010 — Local discovery and locality guarantee
- **Statement:** GIVEN local model servers (Ollama · LM Studio · vLLM · llama.cpp · any OpenAI-compatible endpoint), WHEN Core discovery runs, THEN it probes endpoints, lists and health-checks Core-selectable models with no manual configuration on the common path; manual entry remains available. A Core-owned call declared `locality: local` has zero external egress; an external engine using a local model requires separate containment evidence for the same claim (DEC-054).
- **Priority:** must
- **Source:** `ARCH/18-MODEL-ROUTING.md` §6 · `ARCH/05-INVARIANTS.md` INV-05
- **Acceptance:** discovery test against a local server; unconfigured common-path test; egress-observation test shows zero external traffic for local models; manual entry works.
- **Failure cases:** a local model routed through cloud egress → violation; local server version drift → health re-probe + descriptor refresh + degraded marking; discovery failure → typed `Unavailable` with guidance.
- **Tests:** pending
- **Status:** seeded

#### REQ-MODEL-011 — Reasoning-effort mapping
- **Statement:** GIVEN a Core-owned reasoning model, WHEN effort is chosen, THEN the normalized dial maps only to supported provider parameters. For an external engine, show only adapter-advertised native reasoning controls and label unsupported controls unavailable; raw chain-of-thought never enters the Core transcript (DEC-054).
- **Priority:** must
- **Source:** `ARCH/18-MODEL-ROUTING.md` §5 · `ARCH/04-DECISIONS.md` DEC-034
- **Acceptance:** mapping tests per declared level; unsupported level not offered (composer negotiation); transcript scan finds no raw chain-of-thought.
- **Failure cases:** unsupported level silently ignored → defect; raw chain-of-thought rendered → UI defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-MODEL-012 — Resolved window feeds context feasibility
- **Statement:** GIVEN a resolved Core-owned model selection, WHEN a turn is prepared, THEN its resolved window and reserves feed the pre-turn feasibility check from shared constants; an estimate mismatch is resolved before Core send. An opaque external engine's private window is not asserted by Core; only its offered ContextPacket is budgeted (DEC-054).
- **Priority:** must
- **Source:** `ARCH/18-MODEL-ROUTING.md` §3 · `ARCH/04-DECISIONS.md` DEC-027 · `ARCH/16-CONTEXT.md` §3
- **Acceptance:** shared-constant test (router and context compute identical numbers); mismatch test triggers pre-send recovery; no provider-side overflow while recovery options remain.
- **Failure cases:** router and context diverging on window arithmetic → defect; overflow surfaced by the provider after send → defect.
- **Tests:** pending
- **Status:** seeded

### Runtime environments (`RTENV`)

#### REQ-RTENV-001 — Runtime executes confinement, never decides it
- **Statement:** GIVEN any process or environment action, WHEN the runtime acts, THEN it executes the policy decided by the single Trust decider and never evaluates its own permission policy; no second permission path exists inside the runtime.
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-04 · `ARCH/04-DECISIONS.md` DEC-028 · `ARCH/19-RUNTIME-ENVIRONMENTS.md` §1
- **Acceptance:** static check finds no policy evaluation in the runtime; spawn inputs arrive pre-validated by the exec-policy layer; a runtime-local allow-list fails review.
- **Failure cases:** runtime allowing an action Trust denied → violation; policy logic duplicated in the runtime → review failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-RTENV-002 — Sandbox backends fail closed
- **Statement:** GIVEN a confined action, WHEN the requested sandbox backend is unavailable, THEN the runtime walks the declared ladder (next backend → deny with reason); protected subpaths (e.g. VCS hooks) stay read-only inside writable roots; unconfined execution happens only under an explicit policy flag that is audited and surfaced — never by default.
- **Priority:** must
- **Source:** `ARCH/04-DECISIONS.md` DEC-028 · `ARCH/19-RUNTIME-ENVIRONMENTS.md` §1/§4 · `ARCH/42-EVIDENCE-MAP.md` §3
- **Acceptance:** backend-unavailable test denies after the ladder (or uses the explicitly allowed, audited mode); protected-subpath write test fails; containment/escape results recorded in the evidence map.
- **Failure cases:** silently unconfined spawn → violation; missing backend treated as success → violation; protected subpath writable → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-RTENV-003 — Every process belongs to an environment
- **Statement:** GIVEN any spawned process, WHEN it starts, THEN it belongs to exactly one environment whose identity appears in capability handles and whose declared fields are enforced — kind (`local` · `sandbox` · `worktree`; `remote`/`cloud` later), platform, the confinement profile actually in use, resource limits, workspace roots under pathfloor with scope `shared`/`isolated-worktree`/`sandbox`, network policy ref, and lifetime tied to session/work/detached scope.
- **Priority:** must
- **Source:** `ARCH/19-RUNTIME-ENVIRONMENTS.md` §2 · `ARCH/07-CONTRACTS.md` CTR-015 · `ARCH/06-DATA-MODEL.md` DM-015
- **Acceptance:** environment-identity test (a process without an environment is rejected); limit-enforcement tests; pathfloor test (writes outside workspace roots denied); lifetime test (session end releases session-scoped environments).
- **Failure cases:** process outside any environment → defect; declared limit not enforced → defect; workspace escape → security violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-RTENV-004 — Process trees, orphans and detached work
- **Statement:** GIVEN a running process tree, WHEN a parent exits or cancellation propagates, THEN children are tracked parent→child, cancellation follows the `ARCH/11-WORK.md` §6 semantics, orphans are reaped on parent death, and detached processes are registered for rehydration — never left as unregistered orphans.
- **Priority:** must
- **Source:** `ARCH/19-RUNTIME-ENVIRONMENTS.md` §1/§3 · `ARCH/11-WORK.md` §6 · `ARCH/04-DECISIONS.md` DEC-031
- **Acceptance:** tree-cancellation test; orphan-reaping test; detached registration + rehydration test; no orphan survives without a registry record.
- **Failure cases:** orphaned child after parent death → defect; detached process unregistered → defect; cancellation not propagated to children → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-RTENV-005 — Two PTY classes
- **Statement:** GIVEN a terminal surface, WHEN a PTY is created, THEN it is one of two classes — agent terminal (programmatic, policy-scoped) or user terminal (interactive, user-owned) — served by the same manager under different policies, with class-appropriate authorization.
- **Priority:** must
- **Source:** `ARCH/19-RUNTIME-ENVIRONMENTS.md` §3
- **Acceptance:** class-policy tests (agent PTY actions mediated; user PTY user-owned); same-manager test (one lifecycle/registry path); wrong-class authorization denied.
- **Failure cases:** agent PTY bypassing policy → violation; user terminal silently scripted by an agent → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-RTENV-006 — Output is durable evidence, bounded to context
- **Statement:** GIVEN process output, WHEN it is produced, THEN the full output persists bounded to an artifact/event while the model-facing view is a compact representation plus a reference — unbounded output never streams into context, and no evidence is discarded.
- **Priority:** must
- **Source:** `ARCH/05-INVARIANTS.md` INV-07 · `ARCH/19-RUNTIME-ENVIRONMENTS.md` §3 · `ARCH/16-CONTEXT.md` §4
- **Acceptance:** output-persistence test (full output retrievable); bounded-context test (model view size capped; the reference resolves); no lost-output case.
- **Failure cases:** unbounded output in context → defect; output discarded without artifact/event → evidence loss; compact view without a resolvable reference → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-RTENV-007 — MCP server lifecycle and epoch discipline
- **Statement:** GIVEN an MCP server requested by an adapter, WHEN the runtime hosts it, THEN it spawns (stdio) or connects (HTTP) with an epoch recorded at start and bumped on restart — invalidating outstanding handles — applies the per-server health/restart policy, shuts down on scope end (global/workspace-scoped servers persist; session-scoped servers end with the session, DEC-024 four-state scoping), and takes config from the provider registry with secrets as vault references only.
- **Priority:** must
- **Source:** `ARCH/19-RUNTIME-ENVIRONMENTS.md` §5 · `ARCH/04-DECISIONS.md` DEC-024 · `ARCH/05-INVARIANTS.md` INV-02
- **Acceptance:** restart test bumps the epoch and rejects stale handles; scope-end shutdown test per scope; server crash routes to the provider health/failover path; no secret value in server config.
- **Failure cases:** stale handle accepted after restart → defect; session-scoped server outliving its session → leak; secret in server config → custody violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-RTENV-008 — Elevated helper is opt-in and degrades loudly
- **Statement:** GIVEN a capability that needs privileged reads (e.g. the file-index helper), WHEN the helper is used, THEN it requires explicit install and consent, runs with no service/autostart by default, communicates only over a guarded channel where every request is audited, and denial or absence degrades the capability to non-admin modes with a surfaced note — never silent elevation.
- **Priority:** must
- **Source:** `ARCH/19-RUNTIME-ENVIRONMENTS.md` §6 · `ARCH/21-WORLD-MODEL.md` §5 · `ARCH/05-INVARIANTS.md` INV-20/INV-24
- **Acceptance:** install/consent test; no-autostart test; guarded-channel and per-request-audit test; denied-helper test shows degraded mode with a visible note.
- **Failure cases:** silent elevation → security violation; helper IPC outside the guarded channel → violation; unaudited privileged request → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-RTENV-009 — App lifecycle and crash recovery rebuild from work state
- **Statement:** GIVEN app start, close or crash, WHEN the runtime reconciles, THEN start rehydrates the process registry from work/event state and adopts, monitors or reconciles strays, close applies and records the per-work-kind policy (keep · suspend · stop), and crash recovery rebuilds environments from work state plus checkpoints — no environment state is authoritative.
- **Priority:** must
- **Source:** `ARCH/19-RUNTIME-ENVIRONMENTS.md` §7 · `ARCH/05-INVARIANTS.md` INV-16 · `ARCH/11-WORK.md` §4
- **Acceptance:** restart test rehydrates detached work; stray reconciliation is audited (killed or re-attached per policy); close decision recorded per work kind; crash rebuild test (no state loss from in-memory environment state).
- **Failure cases:** environment state authoritative after restart → defect; close silently stopping kept detached work → defect; stray process untracked → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-RTENV-010 — Health and bounded telemetry
- **Statement:** GIVEN environments, processes and servers, WHEN health changes, THEN each reports `ok` · `degraded` · `down` with a reason, changes publish as events, resource telemetry stays bounded metadata (never payload capture), the UI surfaces environment health for diagnostics, and work items carry their environment ids.
- **Priority:** must
- **Source:** `ARCH/19-RUNTIME-ENVIRONMENTS.md` §8 · `ARCH/30-EVENTS.md` §1
- **Acceptance:** health-state and reason test; event-publication test; telemetry scan shows no payload content; work item carries its environment id.
- **Failure cases:** health change without an event → defect; payload capture in telemetry → privacy violation; down environment without a reason → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-RTENV-011 — Typed spawn failures and hang handling
- **Statement:** GIVEN a spawn request or a hung process, WHEN the failure surfaces, THEN spawn failure distinguishes policy denial from OS failure with no retry loop on policy denial, and a hang is handled by watchdogs plus the `ARCH/11-WORK.md` timeouts, ending in interrupt/cancel with a recorded reason.
- **Priority:** must
- **Source:** `ARCH/19-RUNTIME-ENVIRONMENTS.md` §9 · `ARCH/11-WORK.md` §6
- **Acceptance:** typed spawn-error test (denial vs OS failure distinguishable); no-retry-on-denial test; hang-watchdog test ends with a reason; timeout and cancel are distinguished.
- **Failure cases:** retry loop on policy denial → defect; hang without an abort reason → defect; OS failure reported as policy denial (or vice versa) → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-RTENV-012 — Remote/cloud continuation has real ownership
- **Statement:** GIVEN local Work is moved to a configured remote/cloud executor, WHEN the local app closes or the remote lease changes, THEN only an authenticated accepted owner may run it; input/credential scopes and unavailable local dependencies are explicit; heartbeat loss triggers effect reconciliation before reassignment; cross-device clients observe and steer the same Work and Mission.
- **Priority:** must
- **Source:** DEC-054/055 · `ARCH/19-RUNTIME-ENVIRONMENTS.md` §7 · `ARCH/35-MISSION.md` §Recovery · `ARCH/32-CHANNELS.md` §5
- **Acceptance:** offline handoff continues only after acceptance; remote result attaches to the same Work/Mission; late lease result cannot overwrite the winner; local-only browser/file dependency blocks or takes an explicit transfer path; mobile/web steering reaches the current owner.
- **Failure cases:** app closes before acceptance → Work pauses; heartbeat loss with unknown side effect → reconcile before retry; revoked credential or missing input → blocked with reason; disconnected client does not silently cancel remote execution.
- **Tests:** pending
- **Status:** accepted

### Workflow (`WF`)

#### REQ-WF-001 — Runs pinned to their version
- **Statement:** GIVEN an in-flight workflow run, WHEN the workflow definition is edited or a new version is published, THEN the run continues against its recorded `workflow_version` + digest, resume after restart uses the same pinned version, nested runs inherit per the declared inheritance rules, and the only exception is an explicit, audited run-upgrade — a run never mutates underneath itself.
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §10 · `ARCH/05-INVARIANTS.md` INV-16 · `ARCH/20-WORKFLOW.md` §6 · `ARCH/04-DECISIONS.md` DEC-033
- **Acceptance:** edit-during-run test shows the run executes the pinned version; resume after restart still uses it; a silent upgrade attempt is rejected; an authorized upgrade is recorded and audited.
- **Failure cases:** run picking up edited definition → defect; partial state from mixed versions → forbidden; unrecorded upgrade → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-WF-002 — Typed IR is the single definition truth
- **Statement:** GIVEN a workflow definition, WHEN it is authored or published, THEN it is the typed IR (`DM-021`: typed nodes/edges, variables, vault-ref-only secrets, retry/timeout/concurrency policies, outputs) and only **published**, content-addressed versions (definition digest + input/output schema digests) can trigger or execute; every authoring surface (JSON/YAML · SDK · future visual graph · agent) lands on this same IR.
- **Priority:** must
- **Source:** `ARCH/20-WORKFLOW.md` §2 · `ARCH/06-DATA-MODEL.md` DM-021 · `ARCH/04-DECISIONS.md` DEC-008
- **Acceptance:** IR/schema validation rejects malformed definitions; any declared-content change changes the digest; drafts cannot trigger or execute; all authoring surfaces produce equivalent IR.
- **Failure cases:** draft execution → defect; non-content-addressed version → defect; secret value in a definition → custody violation (INV-02).
- **Tests:** pending
- **Status:** seeded

#### REQ-WF-003 — Occurrences persist ahead and admit one logical run
- **Statement:** GIVEN an enabled trigger, WHEN it becomes due, THEN its occurrence row (due time + unique idempotency key) exists before it is due, and an atomic claim admits each occurrence identity to one logical run. This does not guarantee exactly-once external effects: a retried step uses provider idempotency or reconciliation before another action.
- **Priority:** must
- **Source:** `ARCH/20-WORKFLOW.md` §4 · `ARCH/04-DECISIONS.md` DEC-033
- **Acceptance:** no execution without a persisted occurrence row; duplicate-key insertion/claim yields one logical run; a crash between materialize and claim leaves a reclaimable occurrence; a lost effect acknowledgement does not trigger a blind duplicate.
- **Failure cases:** execution from an unpersisted occurrence, double logical run or unreconciled duplicate side effect → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-WF-004 — One logical trigger owner reconciles, claims and dispatches
- **Statement:** GIVEN the workflow trigger owner, WHEN it runs locally or after an accepted remote/cloud handoff, THEN one fenced logical owner reconciles expired leases (requeue + `lease_reaped` event), applies `cancel_requested` at the next step boundary, materializes time or authenticated connector/event occurrences with stable dedupe keys, atomically claims them, dispatches steps through Work and sleeps until the nearest wake (occurrence due · wait `wake_at` · approval deadline · lease expiry). A local service, cloud executor or external workflow provider may own a trigger, but never two at once for one definition.
- **Priority:** must
- **Source:** `ARCH/20-WORKFLOW.md` §4 · `ARCH/04-DECISIONS.md` DEC-033/057 · `ARCH/07-CONTRACTS.md` CTR-016 · `ARCH/05-INVARIANTS.md` INV-06 · `ARCH/40-FLOWS.md` FLOW-47 · `ARCH/41-EDGE-CASES.md` EDGE-206…209
- **Acceptance:** one fenced trigger owner per definition; duplicate SaaS/webhook event creates one logical occurrence; local-to-cloud owner transfer survives local shutdown; lease-expiry requeue emits the event; cancellation lands only at step boundaries; nearest-wake computation test.
- **Failure cases:** simultaneous trigger owners, unverified webhook, cancelled run executing another step or expired lease blocking a claim → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WF-005 — Crash resume follows the step matrix; completion is never fabricated
- **Statement:** GIVEN a crash between steps, WHEN the run resumes, THEN persisted step state decides the action — `pending` → execute · `settled` → reuse result · `started` idempotent/retryable → retry with the **same** idempotency key · `started` side-effecting **keyless** → `needs_attention` (a human decides repair/retry/skip) · attempts exhausted → terminal + repair path — and completion is never fabricated or blindly re-run.
- **Priority:** must
- **Source:** `ARCH/20-WORKFLOW.md` §4 · `ARCH/04-DECISIONS.md` DEC-022/033
- **Acceptance:** resume-matrix test per row; a keyless interrupted side effect never re-runs without a human decision; retries reuse the recorded key.
- **Failure cases:** fabricated completion → violation; blind re-run of a keyless side effect → verification failure; lost settled result → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WF-006 — Wake times are "not before"; schedules are timezone-faithful
- **Statement:** GIVEN persisted timers and calendar schedules, WHEN the app boots or wakes, THEN persisted `wake_at` is treated as "not before" — never as wall-clock precision — and re-checked at every boot and wake, calendar schedules resolve in the stored IANA zone, and clock changes or DST transitions can neither skip nor duplicate a due occurrence.
- **Priority:** must
- **Source:** `ARCH/20-WORKFLOW.md` §4 · `ARCH/04-DECISIONS.md` DEC-033
- **Acceptance:** boot/wake re-check test; DST-boundary and simulated clock-jump tests show no skip and no duplicate.
- **Failure cases:** wall-clock precision assumed → defect; naive local-time schedule resolution → defect; skip or duplicate on DST → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WF-007 — Misfire policy is explicit, recorded and bounded
- **Statement:** GIVEN a missed due time (OS sleep or reboot), WHEN reconcile runs, THEN the default policy is **Skip + record** (a visible missed row, no execution), the optional *run latest missed* never executes the whole backlog, and the grace window is bounded (≤ 24 h desktop policy).
- **Priority:** should
- **Source:** `ARCH/20-WORKFLOW.md` §4/§11 · `ARCH/04-DECISIONS.md` DEC-033
- **Acceptance:** policy tests per trigger class; missed rows visible; backlog test executes at most the latest missed occurrence; grace-bound enforcement.
- **Failure cases:** silent skip → defect; backlog replay → defect; unbounded grace → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WF-008 — Approval nodes use the one approval primitive
- **Statement:** GIVEN a workflow approval node, WHEN it is requested and resolved, THEN it routes through the single approval primitive with `approve · reject · edit · provide-data` (typed form payload for provide-data), an edit records both the editable draft and the immutable original in the receipt, timeout resolves per class (default reject/escalate), and routing conditions live at the IR level.
- **Priority:** must
- **Source:** `ARCH/20-WORKFLOW.md` §7 · `ARCH/04-DECISIONS.md` DEC-021 · `ARCH/05-INVARIANTS.md` INV-17 · `ARCH/07-CONTRACTS.md` CTR-012
- **Acceptance:** node tests per decision kind; timeout default test; edit records both versions; approval audited and evented.
- **Failure cases:** bespoke approval dialog/path → review failure; timeout leaving a run pending forever → defect; unrecorded approval → receipt verification failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-WF-009 — Retry, timeout and concurrency bounds are declared
- **Statement:** GIVEN node execution and trigger concurrency, WHEN defaults apply, THEN node retry is 2 attempts with exponential backoff (opt-in change), step timeout is 5 min default (overridable; workflow `maximumRuntime` bounded), lease/reaper run at 60 s/30 s, per-workflow overlap defaults to Skip with a declared queue option, and trigger storms are absorbed by backpressure — never unbounded fan-out.
- **Priority:** should
- **Source:** `ARCH/20-WORKFLOW.md` §8 · `ARCH/04-DECISIONS.md` DEC-031/033
- **Acceptance:** defaults test; per-node override test; overlap Skip/queue tests; storm/backpressure test.
- **Failure cases:** retry beyond declared attempts → defect; unbounded fan-out → defect; step running past `maximumRuntime` → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WF-010 — Composition: agent nodes, workflows-as-tools, authored definitions
- **Statement:** GIVEN composition in both directions, WHEN a workflow invokes an agent node, THEN it passes a task + bounded context refs and receives a result/receipt — never the agent's transcript; WHEN an agent invokes a workflow, THEN it resolves through the capability catalog like any other capability; WHEN an agent authors a workflow, THEN the emitted definition passes IR, policy and capability-census validation plus a publish gate before any trigger can run it.
- **Priority:** must
- **Source:** `ARCH/20-WORKFLOW.md` §9 · `ARCH/04-DECISIONS.md` DEC-008 · `ARCH/07-CONTRACTS.md` CTR-001/CTR-009
- **Acceptance:** agent-node handoff carries receipts only; workflow-as-tool resolves through the broker; an unvalidated or unpublished authored definition cannot execute.
- **Failure cases:** transcript leakage into the run → violation; authored definition executing without validation/publish → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WF-011 — Run evidence is receipted, evented and replayable
- **Statement:** GIVEN a workflow run, WHEN it progresses or reaches a terminal state, THEN run-level and per-node receipts plus typed events are recorded in the single event store, every terminal state carries a reason, and the run's evidence is replayable.
- **Priority:** must
- **Source:** `ARCH/20-WORKFLOW.md` §10 · `ARCH/05-INVARIANTS.md` INV-07/INV-23 · `ARCH/04-DECISIONS.md` DEC-022 · `ARCH/29-ARTIFACTS.md` §3
- **Acceptance:** receipt coverage per run and node; event coverage; replay reproduces the run's step ledger; every terminal state has a reason.
- **Failure cases:** run without receipts → violation; silent terminal state → defect; non-replayable evidence → defect.
- **Tests:** pending
- **Status:** seeded

### World model (`WORLD`)

#### REQ-WORLD-001 — Structural state first; queries, not screenshots
- **Statement:** GIVEN a consumer needs machine state (apps, windows, processes, files, browser, devices), WHEN it asks the World Model, THEN it receives structural world objects/edges with identity and freshness — observation is a query over indexed state, not a screenshot — and window capture happens only on demand (explicit view, action verification, or a structured-tree miss).
- **Priority:** must
- **Source:** `ARCH/04-DECISIONS.md` DEC-011 · `ARCH/21-WORLD-MODEL.md` §1 · `ARCH/24-COMPUTER-USE.md` §7
- **Acceptance:** canonical use cases ("open the spreadsheet from yesterday", "go back to that tab", "what is the active document?") resolve from the index; routine queries capture no screenshots; captures are recorded only for the allowed on-demand cases.
- **Failure cases:** screenshot-first default → design violation; routine query triggering a capture → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORLD-002 — Collector set with independent enable/disable and health
- **Statement:** GIVEN the v1 collector set — W1 file inventory + deltas · W2 process/window registry · W3 UI tree on demand · W4 window capture on demand · W5 browser world (W6 devices/registry/shares and W7 content index/OCR deferred) — WHEN collectors run, THEN each is independently enable-able/disable-able and health-reported, and the browser collector shares the `23-BROWSER` CDP machinery.
- **Priority:** must
- **Source:** `ARCH/21-WORLD-MODEL.md` §2
- **Acceptance:** registry shows per-collector state + health; deferred collectors are absent in v1; the browser collector uses the same CDP path as `23`.
- **Failure cases:** collector without health/state → defect; a deferred collector silently active → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORLD-003 — Per-kind identity model
- **Statement:** GIVEN any world object, WHEN it is keyed, THEN identity follows the kind's model (`DM-026`): file `(volume, fileId, incarnation)` on Windows (`(st_dev, st_ino)` + guards on POSIX, FAT caveat declared), process `PID + start time`, window `HWND + PID + class + title + launch time`, browser tab `targetId` session-scoped and never persisted across launches, UI element handles epoch-scoped (valid for one observation/action, not persistent identity), and content hash kept separate from file identity.
- **Priority:** must
- **Source:** `ARCH/21-WORLD-MODEL.md` §3 · `ARCH/06-DATA-MODEL.md` DM-026
- **Acceptance:** identity tests per kind; delete→recreate yields a new file incarnation; PID/handle reuse is rejected as a mismatch; tab identity never survives a launch; ambiguous window/process matches are rejected, not guessed.
- **Failure cases:** reused identity accepted as the same object → defect; element ref treated as durable identity → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORLD-004 — Cursor and epoch discipline per collector
- **Statement:** GIVEN any collector instance, WHEN it tracks incremental updates, THEN it stores `(source, scope, epoch, cursor, observed_at)`; WHEN its source epoch resets (journal rollover, device change, mount/watch re-generation, new document/session), THEN the old cursor is discarded and the scope rescanned — records are never applied across epochs.
- **Priority:** must
- **Source:** `ARCH/21-WORLD-MODEL.md` §4 · `ARCH/05-INVARIANTS.md` INV-20
- **Acceptance:** epoch-reset test discards the cursor and rescans; cursor/epoch persisted per instance; records at or below the cursor are treated as errors, never silently applied.
- **Failure cases:** stale cursor applied after an epoch reset → defect; cross-scope cursor reuse → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORLD-005 — Gaps force a scoped rescan, never a silent gap
- **Statement:** GIVEN a lossy watcher condition (inotify/fanotify overflow, FSEvents drop, RDCW zero-length buffer, dead or truncated USN journal), WHEN a gap is detected, THEN the collector aborts incremental application and forces a rescan of the smallest known scope, recording a freshness anomaly event — a silent gap is never allowed.
- **Priority:** must
- **Source:** `ARCH/21-WORLD-MODEL.md` §4 · `ARCH/05-INVARIANTS.md` INV-20
- **Acceptance:** one test per gap source shows rescan + anomaly event; no incremental record is applied across the gap; the rescan scope is bounded to the smallest known scope.
- **Failure cases:** silent gap → verification failure; unbounded full-volume rescan when a smaller scope is known → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORLD-006 — Freshness contract on every object
- **Statement:** GIVEN any world object, WHEN it is produced or read, THEN it carries `observed_at` + `source` + `epoch`; consumers receive explicit staleness, per-collector TTLs can mark objects `unknown` (files: minutes; process/window: seconds), and write paths re-validate stale objects before acting.
- **Priority:** must
- **Source:** `ARCH/21-WORLD-MODEL.md` §4 · `ARCH/07-CONTRACTS.md` CTR-017
- **Acceptance:** object-freshness tests; a stale object is marked `unknown` after its TTL; write-path re-validation test; no stale object is silently reported fresh.
- **Failure cases:** missing freshness stamps → defect; a write path acting on a stale object without re-validation → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORLD-007 — Queries read the index; no rescan per query
- **Statement:** GIVEN a query or subscription, WHEN it is served, THEN it reads indexed world state (and the event stream), never walks the filesystem and never triggers a full rescan, and event delivery never triggers unbounded work.
- **Priority:** must
- **Source:** `ARCH/21-WORLD-MODEL.md` §2/§4/§6 · `ARCH/05-INVARIANTS.md` INV-20
- **Acceptance:** query-path inspection shows index reads only; a query against a stale index returns freshness-marked results instead of walking; event-delivery bound test.
- **Failure cases:** full rescan per query → INV-20 violation; unbounded work per event → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORLD-008 — Consent records and deny-by-default scoping
- **Statement:** GIVEN a collector instance, WHEN it is enabled, THEN a consent record names the collector id + version · scope · required capability (standard/elevated/OS-permission) · permission actually granted and how · event source + epoch/cursor · data classes · start/stop + retention · revocation path; collection is deny-by-default, scope never expands silently, and there is no persistent "always allow" in v1.
- **Priority:** must
- **Source:** `ARCH/21-WORLD-MODEL.md` §5 · `ARCH/05-INVARIANTS.md` INV-20
- **Acceptance:** consent-record completeness test; a new scope requires new consent; revocation stops the collector and its events; no persistent allow record exists.
- **Failure cases:** collection without consent → violation; silent scope expansion → violation; revocation leaving the collector active → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORLD-009 — Metadata-first, capture-gated, local-first
- **Statement:** GIVEN collectors, WHEN they observe, THEN they read metadata only (never file content); screenshots occur only for explicit view, action verification, or a W3 miss; a visible indicator shows while any capture collector is active; `IsPassword`/protected fields are excluded or masked; and no upload path exists — world-model data stays local.
- **Priority:** must
- **Source:** `ARCH/21-WORLD-MODEL.md` §5 · `ARCH/05-INVARIANTS.md` INV-20 · `ARCH/04-DECISIONS.md` DEC-011
- **Acceptance:** content-read scan is clean; indicator test; masked protected-fields test; egress observation shows no world-model upload path.
- **Failure cases:** collector reading file content → violation; capture without the indicator → defect; world data leaving the machine → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORLD-010 — Elevated collectors degrade loudly
- **Statement:** GIVEN an elevated collector mode (USN/MFT file index), WHEN the helper or elevation is absent or denied, THEN the collector falls back to non-admin modes (walk/RDCW), the mode actually granted is recorded per instance, and the degradation is surfaced — never silent elevation and never a silent capability loss.
- **Priority:** must
- **Source:** `ARCH/21-WORLD-MODEL.md` §5 · `ARCH/19-RUNTIME-ENVIRONMENTS.md` §6 · `ARCH/05-INVARIANTS.md` INV-24
- **Acceptance:** denied-helper test yields non-admin mode + visible note + recorded mode; no USN-derived record is applied without the elevation that produced it.
- **Failure cases:** silent elevation → security violation; degraded mode without a surfaced note → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-WORLD-011 — Query/subscribe surface with filtered projections
- **Statement:** GIVEN the `WorldService` contract (`CTR-017`), WHEN consumers query or subscribe, THEN `query(filter)` and `subscribe(filter) → Stream` are the only surfaces, external-agent projections are sensitivity-filtered per `12`/`32`, and internals (raw stores, cursors, collector internals) are never exposed.
- **Priority:** must
- **Source:** `ARCH/21-WORLD-MODEL.md` §6 · `ARCH/07-CONTRACTS.md` CTR-017 · `ARCH/05-INVARIANTS.md` INV-11
- **Acceptance:** contract-conformance test; external projection test shows filtered fields only; no consumer reads collector stores directly.
- **Failure cases:** unfiltered projection → security violation; consumer reading collector stores directly → architecture violation.
- **Tests:** pending
- **Status:** seeded

### Office (`OFFICE`)

#### REQ-OFFICE-001 — One registry per format, shared by every surface
- **Statement:** GIVEN a document operation, WHEN any surface (CLI/MCP/GUI/agent/API) invokes it, THEN it resolves through the single per-format operation registry where each op is `{id · input/output schema · risk · executor · verification hook}`, exposed as typed capability descriptors (never a command-string tool), and a docs-sync test gates registry↔docs parity with no divergent implementations.
- **Priority:** must
- **Source:** `ARCH/22-OFFICE.md` §1/§3 · `ARCH/04-DECISIONS.md` DEC-013 · `ARCH/07-CONTRACTS.md` CTR-009
- **Acceptance:** all surfaces resolve the same op ids; the docs-sync test is present and enforced; no per-surface op implementation; the model receives semantic ops, not a shell.
- **Failure cases:** divergent op implementations → rejection; command-string tool surface → design violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-OFFICE-002 — Progressive L1 → L2 → L3 access
- **Statement:** GIVEN a document operation, WHEN it runs, THEN it is classified L1 (semantic read) · L2 (structured mutation) · or L3 (raw part-level escape hatch, gated), ordinary agent work uses L1/L2, and L3 requires explicit policy gating — it is never the default path.
- **Priority:** must
- **Source:** `ARCH/22-OFFICE.md` §1/§3 · `ARCH/04-DECISIONS.md` DEC-013
- **Acceptance:** the op set is classified per format; L3 ops require explicit gating (policy/approval); default flows emit no raw part edits.
- **Failure cases:** ungated raw part write → violation; L3 used to bypass registry ops → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-OFFICE-003 — Resident document contexts with exclusive writer leases
- **Statement:** GIVEN an open document, WHEN it is edited, THEN exactly one resident context per document holds an exclusive writer lease; a second writer receives an explicit "in use" result with options (read-only render vs wait); flush is interval + dirty-marker driven with explicit flush on session end and idle eviction under memory bounds; no merge exists in v1.
- **Priority:** must
- **Source:** `ARCH/22-OFFICE.md` §1/§4 · `ARCH/04-DECISIONS.md` DEC-013
- **Acceptance:** concurrent-open test yields the lease message and no silent overwrite; flush-policy tests; session-end flush; idle eviction respects the memory bound.
- **Failure cases:** two writers mutating one document → violation; edits lost without an explicit "in use" result → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-OFFICE-004 — Crash-safe commit: staging, fsync, atomic swap, op log
- **Statement:** GIVEN a commit of document mutations, WHEN it persists, THEN it writes a staging package, `fsync`s it, atomically swaps it into place, and supports op-log replay on recovery; a crash mid-write never leaves a torn file, and scratch/work areas stay confined to declared roots (pathfloor).
- **Priority:** must
- **Source:** `ARCH/22-OFFICE.md` §4 · `ARCH/12-TRUST.md` §2 · `ARCH/04-DECISIONS.md` DEC-013
- **Acceptance:** kill-during-commit test recovers via op-log replay with no torn file; scratch-write test denies paths outside declared roots.
- **Failure cases:** torn file after crash → violation; scratch escaping declared roots → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-OFFICE-005 — Batch atomicity with replayable op log
- **Statement:** GIVEN a batch of document operations, WHEN it is applied, THEN it is all-or-nothing with an op log for replay and audit; a pre-commit validation failure aborts the whole batch and the receipt records the failed check.
- **Priority:** must
- **Source:** `ARCH/22-OFFICE.md` §4/§5/§7 · `ARCH/04-DECISIONS.md` DEC-023
- **Acceptance:** a failing batch leaves the document unchanged; op-log replay reproduces the batch; the receipt names the failing check.
- **Failure cases:** partially applied batch → violation; failed check unrecorded → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-OFFICE-006 — No silent lossy path; engine limits are declared
- **Statement:** GIVEN a format engine and its fidelity limits, WHEN an operation exceeds them (charts/pivots/SmartArt/OLE or unavoidable re-serialization), THEN the limitation is declared per engine, the operation returns typed `guidance` naming the limitation, and re-serialization is disclosed whenever it is unavoidable — never a silent lossy transformation.
- **Priority:** must
- **Source:** `ARCH/22-OFFICE.md` §1/§2/§7 · `ARCH/07-CONTRACTS.md` CTR-009
- **Acceptance:** an engine-limit registry exists; an unsupported-op test yields typed guidance naming the limitation; no lossy path executes undeclared.
- **Failure cases:** silent shape/chart loss → defect; undeclared re-serialization → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-OFFICE-007 — Deterministic render and validation before commit
- **Statement:** GIVEN a preview or a pending commit, WHEN it runs, THEN previews are projections produced without model tokens (browser shell-out or native renderer), and per-format structural validation (DOCX structure/text round-trip · XLSX recalc + formula presence · PPTX slide/shape audit · PDF page/object counts, plus render-diff where useful) runs before commit.
- **Priority:** must
- **Source:** `ARCH/22-OFFICE.md` §5 · `ARCH/04-DECISIONS.md` DEC-015 · `ARCH/05-INVARIANTS.md` INV-13
- **Acceptance:** the preview flow shows zero model calls; validation hooks run per format pre-commit; a failing structural check blocks the commit.
- **Failure cases:** preview consuming model tokens → INV-13 violation; commit without required validation → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-OFFICE-008 — Risk-scaled verification hooks
- **Statement:** GIVEN a committed office effect, WHEN verification runs, THEN its depth scales with the capability's risk class; PDF redact requires a post-op text-extraction check proving removal, and externally visible sends carry a receipt recording the validation result.
- **Priority:** must
- **Source:** `ARCH/22-OFFICE.md` §5 · `ARCH/05-INVARIANTS.md` INV-19 · `ARCH/04-DECISIONS.md` DEC-022 · `ARCH/34-EFFECT-VERIFICATION.md` §3
- **Acceptance:** the redact post-op extraction test proves removal; the receipt carries the validation result; verification depth matches the risk class.
- **Failure cases:** redact leaving extractable content → security failure; externally visible send without a receipt → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-OFFICE-009 — XLSX formula integrity: supported recalculate before commit
- **Statement:** GIVEN an XLSX mutation, WHEN the workbook is committed, THEN a probed provider that supports its formulas/features recalculates and verifies cached values before commit. If no provider can do so, the app blocks a verified save and offers an explicit native-app handoff or reviewed derivative; it never corrupts/overwrites unsupported formula caches.
- **Priority:** must
- **Source:** `ARCH/22-OFFICE.md` §2/§3 · `ARCH/04-DECISIONS.md` DEC-023
- **Acceptance:** recalc-before-commit test for supported formulas; an edited formula yields the updated cached value; unsupported functions and recalc failure block verified commit with typed reason and fallback.
- **Failure cases:** commit without a valid recalc → defect; unsupported formula silently recalculated incorrectly → defect; stale cached value after a formula edit → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-OFFICE-010 — Templates and staged construction validate at each stage
- **Statement:** GIVEN a template merge or a staged deck build (`deck_start → deck_page → deck_build → deck_replace`), WHEN stages execute, THEN each stage is validated check-before-write and the merged/built artifact is validated after merge — a failed stage stops the build without leaving a partial artifact.
- **Priority:** should
- **Source:** `ARCH/22-OFFICE.md` §6 · `ARCH/04-DECISIONS.md` DEC-023
- **Acceptance:** stage-validation tests; a failed stage leaves no partial artifact; the merged result is validated.
- **Failure cases:** partial artifact after a failed stage → defect; unvalidated merge → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-OFFICE-011 — Corrupt inputs quarantine; large inputs stay bounded
- **Statement:** GIVEN a corrupt/unreadable document or a huge workbook/document, WHEN it is opened, THEN the corrupt document is quarantined with a typed error and its original left untouched, and huge inputs use bounded loads/streaming reads under declared limits.
- **Priority:** must
- **Source:** `ARCH/22-OFFICE.md` §7
- **Acceptance:** corrupt-input test; quarantine keeps the original byte-identical; huge-file test respects the declared memory/load limits.
- **Failure cases:** original modified on failed open → violation; unbounded load → defect.
- **Tests:** pending
- **Status:** seeded

### Browser (`BROWSER`)

#### REQ-BROWSER-001 — Managed Chromium default; adapters are integrations
- **Statement:** GIVEN a browser task, WHEN the runtime selects an engine, THEN AgentCowork-managed Chromium (predictable version, isolated profile, headless/background operation) is the default, and Chrome/Edge/Firefox/Opera/system browsers are selectable adapters — never parallel embedded runtimes — with capability differences declared per adapter.
- **Priority:** must
- **Source:** `ARCH/23-BROWSER.md` §1 · `ARCH/04-DECISIONS.md` DEC-012
- **Acceptance:** default-selection test; the adapter registry declares per-adapter capabilities; no second embedded engine ships.
- **Failure cases:** a second embedded runtime → architecture violation; undeclared adapter capability difference → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-BROWSER-002 — Browser instances are environments with lifecycle and recovery
- **Statement:** GIVEN a browser instance, WHEN it runs, THEN it is an environment (`19`) scoped per workspace/task with isolated cookies/storage by default; its lifecycle is launch → ready → operate → park (hibernate) → close; crash recovery re-launches and re-establishes targets; tabs survive where the profile allows.
- **Priority:** must
- **Source:** `ARCH/23-BROWSER.md` §2 · `ARCH/19-RUNTIME-ENVIRONMENTS.md` §2 · `ARCH/04-DECISIONS.md` DEC-012
- **Acceptance:** the instance appears as an environment; isolation test shows no cookie/storage bleed between workspaces; crash-recovery test re-establishes targets; park/close releases resources.
- **Failure cases:** cross-workspace cookie bleed → security failure; unregistered browser process → defect; crash without target recovery → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-BROWSER-003 — User-browser attach and takeover
- **Statement:** GIVEN the user's own browser (Chrome/Edge), WHEN attach is requested, THEN the explicit "user browser" mode uses its profile under consent, an attach failure falls back to managed Chromium with a surfaced note, the agent's operation is visibly indicated, and user takeover is supported (the agent yields input).
- **Priority:** must
- **Source:** `ARCH/23-BROWSER.md` §2/§7
- **Acceptance:** attach-mode test; fallback-with-note test; visible-indicator test; takeover test shows the agent yields control.
- **Failure cases:** silent profile use without consent → violation; attach failure leaving the run stalled → defect; agent ignoring takeover → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-BROWSER-004 — BrowserWorld tab/frame model
- **Statement:** GIVEN the browser world (`21` W5), WHEN tabs and frames are tracked, THEN tab identity is CDP `targetId` — session-scoped, never persisted across launches, stable ordering — frames carry CDP session ids with cross-origin iframes bounded (expand one level, skip blocked, depth ≈5), and page state includes URL · title · forms · downloads · auth state · freshness.
- **Priority:** must
- **Source:** `ARCH/23-BROWSER.md` §3 · `ARCH/06-DATA-MODEL.md` DM-026 · `ARCH/21-WORLD-MODEL.md` §3
- **Acceptance:** tab-identity test (no persistence across launches); frame-bounded test; page-state fields present.
- **Failure cases:** persisted tab identity reused across launches → defect; unbounded frame expansion → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-BROWSER-005 — Compact, bounded snapshots with maskable fields
- **Statement:** GIVEN a page observation, WHEN a snapshot is produced, THEN it is a compact role/name/value graph (≈200–400 tokens for a page, never raw HTML by default), bounded by nodes · depth · text length, with password/protected fields masked.
- **Priority:** must
- **Source:** `ARCH/23-BROWSER.md` §3/§4
- **Acceptance:** snapshot size stays within declared bounds; raw HTML is not the default output; masked protected-field test.
- **Failure cases:** raw DOM dump by default → defect; unmasked protected field → security failure; unbounded snapshot → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-BROWSER-006 — Ephemeral refs re-resolve after events
- **Statement:** GIVEN an element ref `@eN`, WHEN the page navigates or any event invalidates it, THEN the ref is invalidated and never recycled within a session, and the next action re-resolves by role+name (or a fresh snapshot) — a stale ref is never clicked, and refs are never a security boundary.
- **Priority:** must
- **Source:** `ARCH/23-BROWSER.md` §1/§3/§4/§7
- **Acceptance:** stale-ref test yields re-resolution, never a click on a stale ref; ref-recycling test fails as designed.
- **Failure cases:** click on a stale ref → defect; ref treated as authorization → security violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-BROWSER-007 — Trusted input and condition-based waits
- **Statement:** GIVEN a browser action, WHEN input is dispatched, THEN it uses CDP trusted input events (`Input.dispatchMouseEvent`-class), never gestureless `element.click()`-style calls, and waits are condition-based (network/DOM), not sleeps.
- **Priority:** must
- **Source:** `ARCH/23-BROWSER.md` §4
- **Acceptance:** action-path inspection shows trusted input only; wait-policy test shows no fixed sleeps on navigation.
- **Failure cases:** gestureless click → defect; fixed sleep as the wait strategy → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-BROWSER-008 — Structured-first, vision last inside the browser
- **Statement:** GIVEN a browser task, WHEN connectors/APIs (`28`), DOM/AX structure, or CDP actions can do the work, THEN screenshot vision is not used; vision runs only when structure fails (canvas/WebGL) or for verification/diff, with size-capped captures and highlight-before-capture preferred.
- **Priority:** must
- **Source:** `ARCH/23-BROWSER.md` §1/§4/§6 · `ARCH/04-DECISIONS.md` DEC-011
- **Acceptance:** capability-routing test; vision-fallback test; capture cost caps honored; diff runs without model cost.
- **Failure cases:** screenshot-first browser automation → design violation; uncapped image sent → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-BROWSER-009 — Credentials are user-controlled; auth state is surfaced
- **Statement:** GIVEN logins and sessions, WHEN authentication is needed, THEN the agent never harvests passwords (user-assisted login or connector OAuth via `28`), cookies/storage stay in the managed profile with explicit export/import, and auth state is surfaced in BrowserWorld rather than extracted into context.
- **Priority:** must
- **Source:** `ARCH/23-BROWSER.md` §5 · `ARCH/05-INVARIANTS.md` INV-02
- **Acceptance:** credential-capture scan clean; auth-state projection test (surfaced, not extracted); explicit profile export/import test.
- **Failure cases:** password harvested into context or logs → security violation; implicit profile export → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-BROWSER-010 — Per-origin policy and session consent records
- **Statement:** GIVEN browsing, WHEN origins are visited, THEN allowed/blocked/read-only origins and download policy are enforced by `12-TRUST`, and browser-session consent records name the browser instance · profile · origins · granted capabilities.
- **Priority:** must
- **Source:** `ARCH/23-BROWSER.md` §5 · `ARCH/12-TRUST.md` §10 · `ARCH/05-INVARIANTS.md` INV-05/INV-20
- **Acceptance:** per-origin enforcement tests (allow/block/read-only/download); consent-record completeness test.
- **Failure cases:** blocked origin reachable → security violation; browsing without a consent record → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-BROWSER-011 — Downloads and uploads route through artifacts
- **Statement:** GIVEN a download or upload, WHEN it occurs, THEN downloads land in a managed staging area and become artifacts (`29`) with provenance, and uploads are user-gated or policy-gated.
- **Priority:** must
- **Source:** `ARCH/23-BROWSER.md` §2 · `ARCH/07-CONTRACTS.md` CTR-018
- **Acceptance:** download → artifact test with provenance; upload gate test (user/policy); no file written outside staging.
- **Failure cases:** download bypassing artifacts → defect; ungated upload → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-BROWSER-012 — No evasion; CAPTCHA and bot blocks are surfaced
- **Statement:** GIVEN CAPTCHA/bot checks, site automation blocks, or anti-bot friction, WHEN encountered, THEN the runtime surfaces the situation to the user or returns typed `guidance` — it never solves CAPTCHAs, rotates identities/proxies, or spoofs fingerprints.
- **Priority:** must
- **Source:** `ARCH/23-BROWSER.md` §1/§7 · `ARCH/04-DECISIONS.md` DEC-016 · `ARCH/05-INVARIANTS.md` INV-21
- **Acceptance:** evasion-capability catalogue review finds none; CAPTCHA test surfaces to the user; blocked-site test returns guidance without identity rotation.
- **Failure cases:** evasion tooling present → violation; automated CAPTCHA solving → catastrophic violation.
- **Tests:** pending
- **Status:** seeded

### Computer use (`CUA`)

#### REQ-CUA-001 — Highest deterministic rung first
- **Statement:** GIVEN a desktop interaction need, WHEN a rung is chosen, THEN the highest deterministic rung runs first — native API → structured UI (UIA/AX/AT-SPI) → browser DOM/AX → CLI/app API/MCP → vision → raw input — and a screenshot is never taken for something an API or a tree can answer.
- **Priority:** must
- **Source:** `ARCH/24-COMPUTER-USE.md` §1/§2 · `ARCH/04-DECISIONS.md` DEC-011
- **Acceptance:** rung-selection tests per scenario; vision/raw paths are reached only after higher rungs fail or are unavailable.
- **Failure cases:** screenshot used where structure answers → design violation; raw input chosen first → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CUA-002 — Per-platform capability matrix is declared and honest
- **Statement:** GIVEN a platform, WHEN computer use is offered, THEN the actually available rungs are declared in a per-platform matrix (e.g. no by-point AX on macOS in the current build; no AT-SPI client on bare X11), the matrix is tested and surfaced, and unimplemented rungs are absent rather than abstractly promised.
- **Priority:** must
- **Source:** `ARCH/24-COMPUTER-USE.md` §1/§2
- **Acceptance:** matrix-conformance tests per platform; a missing rung produces guidance, never a silent failure.
- **Failure cases:** promising a rung the platform lacks → defect; unavailable rung reported as success → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CUA-003 — Epoch-scoped observations; ambiguity is rejected
- **Statement:** GIVEN an element resolution, WHEN a handle is produced, THEN it is `(runtime_id | role+name+automationId+bounds)` valid for one observation/action, re-read per step and never persisted as identity; ambiguous matches are rejected and re-read or escalated rather than guessed.
- **Priority:** must
- **Source:** `ARCH/24-COMPUTER-USE.md` §1/§3/§8 · `ARCH/06-DATA-MODEL.md` DM-026 · `ARCH/21-WORLD-MODEL.md` §3
- **Acceptance:** handle-lifetime test; ambiguous-match test rejects; a stale observation is re-validated before acting.
- **Failure cases:** cached structure used as identity → defect; a guessed element selected → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CUA-004 — Bounded structured reads with timeout isolation
- **Statement:** GIVEN a structured UI read, WHEN it runs, THEN it is bounded (nodes · depth · text limits), the tree is treated as lazy and changing (re-read per action, never cached as identity), and a per-call budget plus worker isolation prevent a hung provider from stalling the agent.
- **Priority:** must
- **Source:** `ARCH/24-COMPUTER-USE.md` §3 · `ARCH/21-WORLD-MODEL.md` §7
- **Acceptance:** bounds test; hung-provider test times out to a partial tree and the ladder falls through.
- **Failure cases:** hung provider stalling the agent → defect; unbounded tree read → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CUA-005 — Pattern-first actuation
- **Statement:** GIVEN a target element, WHEN it is actuated, THEN supported patterns are re-queried and invoked first (`Invoke` · `Value` · `Toggle` · `Scroll` · `Selection` · `Text` · `Window`), then synthetic events, and raw input only as the last, gated rung.
- **Priority:** must
- **Source:** `ARCH/24-COMPUTER-USE.md` §3/§5
- **Acceptance:** pattern-preference tests per control type; patterns are re-queried per action; raw input is never chosen while a supported pattern works.
- **Failure cases:** synthetic or raw input where a pattern applies → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CUA-006 — Elevation limits degrade to guidance
- **Statement:** GIVEN an elevated region unreachable without UIAccess/consent, WHEN the agent acts, THEN the region is marked unknown, no partial-input attempt is made, and the run surfaces typed guidance instead.
- **Priority:** must
- **Source:** `ARCH/24-COMPUTER-USE.md` §3/§8
- **Acceptance:** elevated-region test yields unknown + guidance; no input synthesis into the unreachable region.
- **Failure cases:** blind input at an elevated region → violation; unreachable region silently reported empty → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CUA-007 — Vision rung discipline
- **Statement:** GIVEN the vision rung, WHEN it runs, THEN it executes only for canvas/WebGL/custom render, poor semantics, verification, or a structured miss; captures are size-capped before send (model limits honored; no reliance on provider downscaling); the loop is screenshot → model → action → observe, with a zoom-class action available for legibility.
- **Priority:** must
- **Source:** `ARCH/24-COMPUTER-USE.md` §4 · `ARCH/04-DECISIONS.md` DEC-011
- **Acceptance:** capture-size test; vision-trigger tests per allowed class; zoom path test.
- **Failure cases:** vision used as the default → design violation; oversized capture sent → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CUA-008 — Capture preference and context discipline
- **Statement:** GIVEN a capture, WHEN vision is needed, THEN DOM/AX-informed capture with highlights is preferred over a plain screenshot, local OCR word-boxes are preferred when the tree is empty and text suffices (zero model cost), snapshots entering context are bounded, and screenshots enter context only when the vision rung ran.
- **Priority:** must
- **Source:** `ARCH/24-COMPUTER-USE.md` §4/§7 · `ARCH/16-CONTEXT.md` §3 · `ARCH/04-DECISIONS.md` DEC-015 · `ARCH/05-INVARIANTS.md` INV-22
- **Acceptance:** preference-order tests; the OCR path runs without model calls; snapshot-bound test; a context scan shows no screenshots outside the vision rung.
- **Failure cases:** a plain screenshot preferred over highlight capture → defect; screenshot in context without the vision rung → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CUA-009 — On-screen content is untrusted input
- **Statement:** GIVEN on-screen content, WHEN the agent processes it, THEN the content is untrusted input (a prompt-injection surface), consequential actions it proposes require the approval primitive, and capture is consent-gated with protected fields masked.
- **Priority:** must
- **Source:** `ARCH/24-COMPUTER-USE.md` §4/§6 · `ARCH/04-DECISIONS.md` DEC-021 · `ARCH/12-TRUST.md` §5
- **Acceptance:** injection-corpus test (screen instructions never auto-execute); consequential-action approval test; masked-field test.
- **Failure cases:** screen text treated as instructions → catastrophic violation; unapproved consequential action → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CUA-010 — Raw input is gated, indicated and rate-limited
- **Statement:** GIVEN the raw-input rung, WHEN it is used, THEN it requires a `HumanAuthorization`-class gate, shows a visible indicator, is rate-limited, is never the first choice, and is never used for evasion.
- **Priority:** must
- **Source:** `ARCH/24-COMPUTER-USE.md` §5 · `ARCH/04-DECISIONS.md` DEC-016 · `ARCH/05-INVARIANTS.md` INV-20/INV-21
- **Acceptance:** gate test (raw input without authorization is denied); indicator test; rate-limit test.
- **Failure cases:** ungated synthetic input → violation; raw input used for evasion → catastrophic violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CUA-011 — Confirmation thresholds and no bulk storms
- **Statement:** GIVEN a consequential action class (sends · purchases · deletes · permission changes), WHEN it is about to execute, THEN the approval primitive is required; destructive/persistent patterns are denied by policy defaults and the ladder never overrides policy; bulk-input storms are prevented by a bounded action rate per target.
- **Priority:** must
- **Source:** `ARCH/24-COMPUTER-USE.md` §6 · `ARCH/04-DECISIONS.md` DEC-021 · `ARCH/12-TRUST.md` §3
- **Acceptance:** consequential-class tests require approval; policy-default denial test; rate-bound test.
- **Failure cases:** consequential action without approval → violation; ladder bypassing a policy denial → violation; bulk storm → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CUA-012 — Post-action verification with bounded recovery
- **Statement:** GIVEN an action whose outcome is uncertain (vision-based location, a pattern call that may not have applied), WHEN it completes, THEN the outcome is verified (structured re-read or a second observation), retries are bounded, and repeated failure lands in `needs_attention` — never an unbounded retry loop.
- **Priority:** must
- **Source:** `ARCH/24-COMPUTER-USE.md` §4/§8 · `ARCH/04-DECISIONS.md` DEC-022
- **Acceptance:** post-action verification tests; bounded-retry test; repeated failure yields `needs_attention`.
- **Failure cases:** unbounded retry loop → defect; unverified success claimed → violation.
- **Tests:** pending
- **Status:** seeded

### Agent engine (`AGENT`)

> The engine-agnostic obligations AgentCowork holds over **any** agent engine — an
> external CLI or protocol agent, and any engine bound natively later. The engine's
> own loop, prompt construction, native tools and internal recovery are the engine's
> business (`ARCH/02-THESIS.md` agent-native plane); what is specified here is only what
> we require of it and what we refuse to do on its behalf. The first-party engine
> requirement set (`REQ-AGX-001…013`) is retired by DEC-052; these IDs are new and are
> never renumbered or reused.

#### REQ-AGENT-001 — Delegation contract
- **Statement:** GIVEN a **host-created** delegation request, WHEN it is dispatched, THEN it names the bound engine, Work item, permitted scopes and deadline, and the delegated work appears as first-class `Work` with its own host journal. An external engine's privately spawned children remain native activity with reported/observed provenance (DEC-054).
- **Priority:** must
- **Source:** `ARCH/15-AGENT-PLANE.md` §7 · `ARCH/11-WORK.md` · `ARCH/04-DECISIONS.md` DEC-029, DEC-031, DEC-036
- **Acceptance:** a delegated item is queryable as `Work` while running and after completion; a cancelled parent leaves the child's terminal state honest.
- **Failure cases:** a delegation that cannot be represented as `Work` → refused; a child outliving its deadline → reported as expired, never as completed.

#### REQ-AGENT-002 — Subagent spawn and completion
- **Statement:** GIVEN a **host-created** child Work request, WHEN it is admitted, THEN its host lifecycle is observable, its terminal result is a receipt or labelled worker report with evidence refs, and the async child is tracked by Work rather than an in-memory promise. An engine-native child remains outside the host lifecycle (DEC-054).
- **Priority:** must
- **Source:** `ARCH/15-AGENT-PLANE.md` §7 · `ARCH/20-WORKFLOW.md` · `ARCH/04-DECISIONS.md` DEC-029, DEC-036 · `ARCH/30-EVENTS.md`
- **Acceptance:** spawn → completion is reconstructable from the journal alone after a restart; a child that dies without a terminal record settles as uncertain, never as completed.
- **Failure cases:** a lost child record → `uncertain`; a completion claimed without evidence → rejected.

#### REQ-AGENT-003 — Subagent isolation modes
- **Statement:** GIVEN a host delegation request, WHEN isolation is chosen, THEN the requested and enforced modes are explicit and recorded per Work. Shared read, worktree or adapter-specific sandbox are available only where proven; a bound external engine is never described as in-process or stronger-isolated without evidence (DEC-054).
- **Priority:** must
- **Source:** `ARCH/15-AGENT-PLANE.md` §7 · `ARCH/04-DECISIONS.md` DEC-025, DEC-029 · `ARCH/19-RUNTIME-ENVIRONMENTS.md`
- **Acceptance:** the recorded mode is the mode actually enforced; a request for a mode the runtime cannot provide is refused rather than downgraded.
- **Failure cases:** silent downgrade to a weaker isolation mode → violation; unavailable worktree/ACP mode → refused with the reason.

#### REQ-AGENT-004 — Receipts, not transcripts
- **Statement:** GIVEN a completed delegated item, WHEN its result is projected to the user or a parent engine, THEN it is a receipt (what was intended, what changed, what it cost, what remains uncertain) and never a raw transcript presented as a result.
- **Priority:** must
- **Source:** `ARCH/15-AGENT-PLANE.md` §7 · `ARCH/29-ARTIFACTS.md` · `ARCH/34-EFFECT-VERIFICATION.md` · `ARCH/04-DECISIONS.md` DEC-022
- **Acceptance:** a delegated result is renderable as a receipt with no transcript access; an engine's self-report of success is labelled as a report until verified.
- **Failure cases:** an unverified report rendered as fact → violation; a transcript surfaced as a user-facing result → violation.

### UI (`UI`)

#### REQ-UI-001 — Reasoning is summarized, never raw chain-of-thought
- **Statement:** GIVEN a model produces reasoning, WHEN it is rendered in chat, THEN the user sees a summarized, structured progress view — never raw chain-of-thought.
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §9 · `AGENTCOWORK-UI.md` §4.4
- **Acceptance:** no raw CoT in stored or displayed transcripts; reasoning renders only through the `reasoning` projection.
- **Failure cases:** raw CoT rendered → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-002 — Deterministic-first UI operations
- **Statement:** GIVEN a UI surface renders, navigates, converts a diagram or discovers references, WHEN the operation runs, THEN it is a pure function of already-fetched state and consumes zero model tokens; only explicit opt-in actions (title generation, summarisation, "what next") may call a model, with visible cost.
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §9 · `DEC-015` · `INV-13` · `AGENTCOWORK-UI.md` §8 (UI-02)
- **Acceptance:** a trace of rendering/navigation/diagram/reference paths shows zero provider calls; opt-in actions show a cost affordance.
- **Failure cases:** a render/navigation path calling a model → defect; a preview silently spending budget → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-003 — Truthful state, never inferred as measured
- **Statement:** GIVEN a value, state or progress affordance, WHEN the value is not measured, THEN the UI renders `—` or nothing and never a plausible guess, fabricated percentage, fake spinner or progress bar standing in for an unknown.
- **Priority:** must
- **Source:** `AGENTCOWORK-UI.md` §1.2 (UI-03, UI-18), §11; `ev: ui/src/components/views/run-projection.tsx:33-56`; `ev: ui/src/lib/store.ts:191-194`
- **Acceptance:** unknown figures render `—`/absent; no determinate progress without a measured value; no fake spinner on a Core-unavailable banner.
- **Failure cases:** inferred value shown as measured → defect; fabricated progress → defect; spinner with no work in flight → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-004 — Streaming-safe markdown completeness
- **Statement:** GIVEN an assistant answer containing headings, tables, blockquotes, `hr`, ordered lists or images, WHEN it renders (streaming or committed), THEN every element renders from the token palette, a GFM table scrolls inside a keyboard-focusable `role="region"`, raw HTML is gated off, math is constrained, in-flight code fences are not re-highlighted, and stream writes coalesce to at most one per animation frame.
- **Priority:** must
- **Source:** `AGENTCOWORK-UI.md` §4.1 (R1–R8); evidence §4.1
- **Acceptance:** a heading + table + 400-line fence renders correctly; a render trace shows no per-delta re-parse; the table is focusable without breaking the bubble.
- **Failure cases:** borderless/unconstrained table → defect; re-highlight of an open fence → defect; raw HTML rendered → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-005 — Mermaid policy-gated isolated auto-conversion
- **Statement:** GIVEN a fenced `mermaid` block in the transcript, WHEN the fence closes, THEN it auto-converts with no user action through the policy gate and a `securityLevel: 'strict'` config locked by a `secure:` array, the theme is read from live tokens, output is a blob-`<img>` (never inline SVG/HTML), size is reserved (CLS 0), renders are serialized and bounded, and a rejection/failure leaves copyable source with a status line.
- **Priority:** must
- **Source:** `AGENTCOWORK-SPEC.md` §9 · `AGENTCOWORK-UI.md` §4.2 (R10–R18)
- **Acceptance:** a `mermaid` fence converts automatically; `img:`/`%%{init}%%` source renders as copyable source with a status line; a theme/accent flip re-renders; a failed render leaves the previous image visible (CLS 0).
- **Failure cases:** render while the fence is open → defect; source bypassing the gate → blocked; inline SVG/HTML injection → forbidden.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-006 — Tool-call states and grouping
- **Statement:** GIVEN tool calls in a turn, WHEN they stream, run, succeed, fail or are cancelled, THEN the UI renders exactly five states (`proposed · running · succeeded · failed · cancelled`) with streaming mapped to `running` + `tool.progress`, a group containing an error never auto-collapses, a settled turn collapses to one rail line, group identity is the first item's identity, and a Guard-denied call is neutral-with-lock (not red).
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §3 · `AGENTCOWORK-UI.md` §4.3 (R21–R27; UI-04, UI-06)
- **Acceptance:** a failed call never auto-collapses; a settled turn collapses to one line; a resumed session replays the same grouping; denied ≠ failed styling.
- **Failure cases:** failure auto-collapsed → defect; a sixth `streaming` state → defect; blocked rendered as error → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-007 — Plan bar bound to the running turn
- **Statement:** GIVEN a plan for the active turn, WHEN the turn runs, THEN a plan bar sits above the composer bound to `turn_id === activeTurnId`, is keyboard-operable (`role="button"`, `aria-expanded`), caps open height at `min(22vh, 180px)`, shows `completed/total`, reports a version delta on a new plan version (never "no change" from an unknown previous list), and renders/reserves nothing when no plan belongs to the turn.
- **Priority:** must
- **Source:** `AGENTCOWORK-UI.md` §4.4 (R31); `ev: ui/src/lib/store.ts:197-205,211`
- **Acceptance:** the bar appears only while its turn runs and disappears with it; a second version reports a delta; the step vocabulary is the existing `ProgressStep` union.
- **Failure cases:** plan hanging over the next turn → defect; false "no change" → defect; a new status vocabulary → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-008 — Reasoning dial capability-negotiated
- **Statement:** GIVEN the bound agent's model descriptor, WHEN the reasoning dial is shown, THEN it offers only the levels the model supports (`auto · minimal · low · medium · high · extra_high`), clamps the value on model switch, keeps `auto` off the track on its own row, keeps the pill visible at `auto`, tracks the label live while dragging, states what the current level means, and says the model has no reasoning surface rather than showing a dead control.
- **Priority:** must
- **Source:** `ARCH/18-MODEL-ROUTING.md` §5 · `AGENTCOWORK-UI.md` §5.4 (R51); `ev: ui/src/lib/acp.ts:93-101`
- **Acceptance:** a reasoning-less model shows an honest statement, not a dead slider; an unsupported level is never offered; the value clamps on switch.
- **Failure cases:** dial offers a level the model rejects → defect; `auto` on the track → defect; silent stale value after model switch → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-009 — Universal DocumentSurface
- **Statement:** GIVEN a reference, artifact, run, URL or session, WHEN it is opened, THEN one DocumentSurface opens exactly one tab per document identity (a second open focuses the existing tab) with N documents per kind, re-derives content from a persisted light ref, pre-checks existence in the host (a missing file renders an inert *not found* row with a re-link), and opens/renders with zero model tokens.
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §7 · `AGENTCOWORK-UI.md` §3 (R45), §2.3
- **Acceptance:** two spreadsheets and a PDF open together; a per-session reload restores the tab set; a missing file renders inert with a reason; two opens of one identity share one tab.
- **Failure cases:** a second document of a kind replacing the first → defect; a tab opening onto "file not found" → defect; tab content persisted instead of a light ref → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-010 — Agent picker / agent-owner model / two-pane runtime surface
- **Statement:** GIVEN an agent binding, WHEN the composer agent control is used or agent configuration is opened, THEN the compact control paints the agent-owned model or an explicit em dash, selection is installed-only (catalog-only rows render a not-installed state with an install affordance), and configuration expands to a two-pane surface (runtimes left; the selected runtime's own model/auth/native capabilities + shared grants right) where a Native model is never offered for an external runtime and an agent with no model surface says "managed by \<agent\>".
- **Priority:** must
- **Source:** `AGENTCOWORK-UI.md` §5.3, §5.13; `ev: ui/DESIGN-SYSTEM.md:11,58`; `ev: ui/src/components/chat/agent-model-picker.tsx:474-489,526-549`
- **Acceptance:** a governance badge and honest hover note per agent; installed-only selectable; catalog-only not selectable; no Native model offered for an external runtime; a switch during a live stream applies to the next turn and says so.
- **Failure cases:** a catalog row becoming selectable → defect; a Native model offered for an external runtime → defect; the platform inventing a model list → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-011 — Windows-first provenance and truthful readiness
- **Statement:** GIVEN an agent/runtime row, WHEN it renders, THEN it shows a discriminated location (`managed · windows_path · app_paths · user_path · package_manager · wsl · unavailable`), keeps `installed`/`discovered`/`launchable` distinct (a WSL row is discovered-not-launchable until its spawn adapter exists and is non-selectable with a reason), never treats a catalog/registry row as occupancy, and shows readiness as evidence-gated (`unverified`/`available` until a real Windows acceptance record) rather than fabricated from a mock, preview, catalog entry or unit-only result.
- **Priority:** must
- **Source:** `AGENTCOWORK-UI.md` §5.13 (UI-17); workspace UI/UX skill section 8; `ARCH/06-DATA-MODEL.md` DM-014; `ARCH/13-CAPABILITY.md` §6
- **Acceptance:** a WSL-only row is non-selectable with an honest reason; a catalog row is not occupancy; no verified readiness without a Windows acceptance record; a Linux path is never handed to `CreateProcess`.
- **Failure cases:** path string instead of a discriminated location → defect; `discovered` treated as `launchable` → defect; fabricated readiness → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-012 — Keyboard-complete accessibility
- **Statement:** GIVEN any interactive surface, WHEN the user navigates by keyboard, THEN every action (workbench slot, tab, disclosure, approval choice, composer control) is reachable with a visible `focus-visible` ring, the Workbench is a real `role="tab"`/`tabpanel` tablist, composer `@`/`/` use combobox semantics with IME-safe Enter, streaming text is not announced token-by-token while status/approval changes are announced appropriately, and an automated accessibility gate passes.
- **Priority:** must
- **Source:** `AGENTCOWORK-UI.md` §9.1 (UI-12); `ev: ui/src/globals.css:791-795`
- **Acceptance:** keyboard-only traversal of every workflow, tab, disclosure, approval choice and composer control with a visible ring; the a11y gate passes; live-region announcements follow the discipline.
- **Failure cases:** an interactive with no focus ring → defect; token-by-token live announcement → defect; a11y gate absent → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-013 — Progressive disclosure and non-intrusive product invariants
- **Statement:** GIVEN advanced detail, a policy-blocked control, or a finishing tool, WHEN the surface renders, THEN advanced detail sits behind a labelled collapsed row (open-by-default only when the content is the answer), a blocked control stays visible marked blocked with its reason and who can change it, and nothing auto-navigates — a tool finishing never steals focus, opens a pane or moves the page.
- **Priority:** must
- **Source:** `AGENTCOWORK-UI.md` §1.2 (UI-04, UI-05, UI-07); evidence §3.8 (openwork P3/P4, T1, S5)
- **Acceptance:** a blocked action is visible with a reason; a completed tool does not move focus or open a pane; advanced detail is behind a labelled disclosure.
- **Failure cases:** blocked control hidden → defect; auto-navigation on completion → defect; blocked rendered red as a failure → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-014 — Zero layout shift on live regions
- **Statement:** GIVEN a live region whose content ticks or changes (reasoning timer, tool rail, telemetry readout, streaming diagram, transcript), WHEN its state changes, THEN its space is reserved (`min-h`, fixed readout slots, intrinsic-size hints, viewBox-sized diagram) so CLS is zero and no control moves when a neighbouring state changes.
- **Priority:** must
- **Source:** `AGENTCOWORK-UI.md` §1.2 (UI-08), §4.2 (R16); `ev: ui/src/components/chat/message-bubble.tsx:197`; `ev: ui/src/components/chat/tool-chip.tsx:533,541`; `ev: ui/src/components/chat/chat-composer.tsx:929-965`
- **Acceptance:** CLS measured 0 while a turn streams and a diagram re-renders; telemetry readouts do not move when web-search toggles; the reasoning trigger reserves its height.
- **Failure cases:** content jumping as a timer ticks → defect; a readout slot resizing on state change → defect; a diagram collapsing to a placeholder on re-render → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-UI-015 — Typed settings, profiles and safe editing
- **Statement:** GIVEN any user-visible setting, WHEN it is read or changed, THEN one registry defines its key, parser, owner, scope, effective source and revision; policy state is Core-owned; named profiles hold only non-secret overrides; dirty navigation offers Save/Discard/Cancel.
- **Priority:** must
- **Source:** DEC-056 · DM-040 · CTR-032 · `ARCH/48-EXPERIENCE-SURFACES.md` §Settings
- **Acceptance:** unknown/invalid keys reject; stale policy write rejects; a device-only cosmetic preference does not change another device; profile preview names every effective change; no secret value is copied into a profile.
- **Failure cases:** unsaved change lost silently, stale overwrite, profile credential copy or permission widening → defect.
- **Tests:** pending
- **Status:** accepted

#### REQ-UI-016 — Render and boot failure recovery
- **Statement:** GIVEN a renderer exception or slow/failed boot, WHEN the app cannot present the normal workspace, THEN it shows a bounded recovery surface with Reload and copyable redacted diagnostics rather than a blank window.
- **Priority:** must
- **Source:** DEC-056 · `ARCH/38-EXPERIENCE-QUALITY.md` §Progressive interaction · `ARCH/48-EXPERIENCE-SURFACES.md` §Interaction and quality acceptance
- **Acceptance:** simulated render exception and boot timeout each expose a keyboard-reachable recovery action; draft state is restored after reload where durable.
- **Failure cases:** blank window, infinite spinner, secret in copied diagnostic or unrecoverable focus trap → defect.
- **Tests:** pending
- **Status:** accepted

#### REQ-UI-017 — Searchable settings and one shortcut dispatch registry
- **Statement:** GIVEN a settings search or keyboard shortcut, WHEN the user invokes it, THEN search resolves labels/synonyms to the owning control and the shortcut catalogue is generated from the same collision-checked dispatch registry as the handlers.
- **Priority:** should
- **Source:** DEC-056 · `ARCH/48-EXPERIENCE-SURFACES.md` §Settings
- **Acceptance:** matched field opens and highlights; a protected/conflicting chord is rejected; every displayed shortcut has a handler and every user shortcut appears in the catalogue.
- **Failure cases:** dead shortcut entry, hidden handler, unresolved synonym or unsafe collision → defect.
- **Tests:** pending
- **Status:** accepted

### Files (`FILES`)

#### REQ-FILES-001 — Platform file identity is incarnation-aware
- **Statement:** GIVEN a file on a supported platform, WHEN its identity is recorded, THEN it is `(volume, fileId)` on Windows or `(st_dev, st_ino)` on POSIX extended with incarnation evidence — `(volume, fileId, incarnation)` / `(dev, ino, nlink)` — so a reused id after delete never resumes the old identity, and Windows records never carry zeroed `dev`/`ino`.
- **Priority:** must
- **Source:** `ARCH/25-FILES.md` §1/§2 · `ARCH/21-WORLD-MODEL.md` §3 · `ARCH/06-DATA-MODEL.md` DM-026 · `ARCH/05-INVARIANTS.md` INV-20
- **Acceptance:** rename/move within a volume preserves identity; delete + recreate with a reused id yields a new identity; hardlink sets are distinguished by link count; a Windows scan record contains a real file id (no zeros).
- **Failure cases:** path used as identity → defect; reused id accepted as the same file → defect; zeroed Windows `dev`/`ino` reaching dedup/lease keys → defect (recorded code-phase fix `walk.rs:131-157`).
- **Tests:** pending
- **Status:** seeded

#### REQ-FILES-002 — File replacement re-keys dependents explicitly
- **Statement:** GIVEN a file whose incarnation changes (replace/restore/move across volumes), WHEN dedup, leases or index entries still refer to the old identity, THEN they are re-keyed or invalidated explicitly — stale identity is never carried forward.
- **Priority:** must
- **Source:** `ARCH/25-FILES.md` §2/§8 · `ARCH/21-WORLD-MODEL.md` §3
- **Acceptance:** replacement test shows a new identity with old lease/dedup entries re-keyed or dropped; the index row for the old incarnation is marked stale; dependent capabilities see the new identity.
- **Failure cases:** stale identity silently reused after replacement → defect; dedup grouping across two incarnations → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-FILES-003 — The metadata index is the query surface
- **Statement:** GIVEN file queries (names/paths/ids/size/times/attrs/links), WHEN they run, THEN they read the metadata index instead of walking the filesystem, and the index is updated from watcher deltas (bounded rescans on gaps) with per-row freshness.
- **Priority:** must
- **Source:** `ARCH/25-FILES.md` §1/§3/§4 · `ARCH/05-INVARIANTS.md` INV-20
- **Acceptance:** query-time I/O trace shows no directory walk; a delta-updated file appears in results without a full rescan; index rows expose `observed_at` freshness.
- **Failure cases:** query walking the filesystem → architecture violation; stale index row served without a freshness flag → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-FILES-004 — Watcher gaps abort to a bounded rescan, never a silent gap
- **Statement:** GIVEN a native watcher gap (dead/truncated USN journal, `IN_Q_OVERFLOW`, `FAN_Q_OVERFLOW`, FSEvents `MustScanSubDirs`, RDCW zero-length buffer), WHEN it is detected, THEN the cursor is discarded, the smallest known scope is rescanned, and a freshness anomaly is recorded — completeness is never claimed while a gap is open.
- **Priority:** must
- **Source:** `ARCH/25-FILES.md` §1/§3/§8 · `ARCH/21-WORLD-MODEL.md` §4 · `ARCH/05-INVARIANTS.md` INV-20
- **Acceptance:** overflow-injection test produces a scoped rescan plus anomaly event; no result set is served as complete while the gap is unresolved.
- **Failure cases:** silent gap → defect; full-volume rescan when a smaller known scope was available → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-FILES-005 — Cursor and epoch discipline survives restart and journal reset
- **Statement:** GIVEN a collector instance, WHEN deltas are consumed, THEN its cursor row is `(source, scope, epoch, cursor, observed_at)`; a journal-id/epoch change or a deleted/truncated journal discards the cursor and requests a rescan; records at or below the cursor are rejected, never silently applied.
- **Priority:** must
- **Source:** `ARCH/25-FILES.md` §3/§4 · `ARCH/21-WORLD-MODEL.md` §4
- **Acceptance:** restart resumes from the stored cursor; epoch-change test discards and rescans with the reset recorded; a duplicate/replayed batch is refused.
- **Failure cases:** stale cursor applied after journal reset → defect; replayed records silently applied → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-FILES-006 — Writes re-validate freshness before mutating
- **Statement:** GIVEN a write targeting a known file, WHEN the index entry is stale (TTL passed, identity/size/mtime guard mismatch), THEN the object is re-validated against live state and marked `unknown` on failure before any mutation — never a blind write against a stale snapshot.
- **Priority:** must
- **Source:** `ARCH/25-FILES.md` §4 · `ARCH/21-WORLD-MODEL.md` §4
- **Acceptance:** stale-entry write test triggers re-validation; an identity/size/mtime mismatch blocks or re-keys the write and surfaces; an `unknown` object is not mutated without re-validation.
- **Failure cases:** blind write on a stale/unknown object → defect; silent overwrite of a newer version → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-FILES-007 — Overlapping writes are serialized by leases
- **Statement:** GIVEN two writers whose path scopes overlap, WHEN a write lease is requested, THEN exactly one is granted and the other receives a conflict result offering queue · rebase (VCS-aware) · ask — never a silent overwrite.
- **Priority:** must
- **Source:** `ARCH/25-FILES.md` §1/§6 · `ARCH/04-DECISIONS.md` DEC-029
- **Acceptance:** concurrent-writer test yields grant + conflict; the conflict resolves through the declared policy; no interleaved write reaches disk.
- **Failure cases:** silent overwrite → violation; both writers granted the same scope → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-FILES-008 — Leases expire crash-safe, are audited and bound to work
- **Statement:** GIVEN a granted lease, WHEN the holder crashes or the work ends, THEN the lease expires without manual cleanup, ownership is tied to `work_id`/worker, grant/conflict/expiry are audited, and worktree-isolated writers hold leases on their checkout with merges as explicit steps.
- **Priority:** must
- **Source:** `ARCH/25-FILES.md` §6 · `ARCH/04-DECISIONS.md` DEC-029 · `ARCH/05-INVARIANTS.md` INV-24
- **Acceptance:** crash-expiry test releases the lease; audit entries exist for grant and expiry; a merge into the parent without its lease is rejected.
- **Failure cases:** permanent lease after crash → defect; unaudited grant/expiry → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-FILES-009 — Canonicalization precedes policy checks and is re-checked at use
- **Statement:** GIVEN a path used in an operation, WHEN policy resolves it, THEN symlinks/junctions are canonicalized first, platform case-sensitivity and Windows long-path rules are declared, and a canonical mismatch between check and use denies the operation (TOCTOU-aware).
- **Priority:** must
- **Source:** `ARCH/25-FILES.md` §7 · `ARCH/12-TRUST.md` §2/§8
- **Acceptance:** symlink-swap test denies at use time; case/long-path declaration tests per platform; no policy decision is made on a non-canonical path.
- **Failure cases:** policy decision on a non-canonical path → security failure; check/use race exploited → TOCTOU violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-FILES-010 — Path scopes intercept; protected subpaths stay read-only
- **Statement:** GIVEN workspace path scopes (`allowed_paths` / `read_only_paths`), WHEN out-of-scope access occurs, THEN it is denied and logged (interception, not un-discovery), and protected subpaths (VCS hooks, system dirs) remain read-only inside writable roots.
- **Priority:** must
- **Source:** `ARCH/25-FILES.md` §7 · `ARCH/12-TRUST.md` §2/§8 · `ARCH/05-INVARIANTS.md` INV-10
- **Acceptance:** out-of-scope access denied and logged; protected subpath write denied inside an otherwise writable root; listing suppression is not used as a security mechanism.
- **Failure cases:** out-of-scope write allowed → security failure; protected subpath mutated → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-FILES-011 — Workspace/project identity anchors scopes and survives worktrees
- **Statement:** GIVEN a workspace (`DM-024`: folder · repo · multi-root with roots/trust/policy refs), WHEN project identity is assigned, THEN it is keyed by canonical repo root with git remote as an attribute, re-keying on move/clone is explicit, and worktrees share the parent project identity.
- **Priority:** must
- **Source:** `ARCH/25-FILES.md` §5 · `ARCH/06-DATA-MODEL.md` DM-024 · `ARCH/17-MEMORY.md` §2.1
- **Acceptance:** worktree membership maps to the parent identity; move/clone without an explicit re-key keeps the old key; memory and policy scopes resolve through the same key.
- **Failure cases:** implicit re-key → defect; worktree treated as a new project → memory fragmentation or cross-scope leakage → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-FILES-012 — Unreadable scopes are skipped honestly and surfaced
- **Statement:** GIVEN a permission-denied or unreadable path during collection, WHEN the scan proceeds, THEN the path is skipped without aborting the scan, the skip is surfaced with a count (metadata-mode honesty), and the instance's consent/elevation mode (helper/per-scan/non-admin) is recorded.
- **Priority:** should
- **Source:** `ARCH/25-FILES.md` §3/§8 · `ARCH/21-WORLD-MODEL.md` §5 · `ARCH/05-INVARIANTS.md` INV-20
- **Acceptance:** a permission-denied fixture yields a partial result plus a surfaced skipped count; the collector mode is recorded on the instance; elevated modes remain consent-gated.
- **Failure cases:** one denied path aborting the whole scan → defect; skipped paths hidden → defect; silent elevation → violation.
- **Tests:** pending
- **Status:** seeded

### Code (`CODE`)

#### REQ-CODE-001 — RepoGraph builds incrementally into a per-workspace store
- **Statement:** GIVEN a workspace, WHEN the RepoGraph index is built or updated, THEN it stores the declared node kinds (File · Symbol · Import · Reference · Call · Test · Config · Document · Command) and typed edges (`imports` · `calls` · `extends/implements` · `references` · `tested_by` · `configured_by` · `generated_by` · `depends_on`), re-parses only hash-changed files, and keeps one store per workspace.
- **Priority:** must
- **Source:** `ARCH/26-CODE.md` §1/§2/§8 · `ARCH/07-CONTRACTS.md` CTR-025
- **Acceptance:** change-one-file test re-parses only that file; a clean rebuild equals the incremental result; a corrupted store rebuilds from source within bounded work; stores do not leak across workspaces.
- **Failure cases:** full re-parse on every change → defect (bounded-work violation); cross-workspace store leakage → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CODE-002 — Edge precision is labeled; inference is never presented as certainty
- **Statement:** GIVEN graph edges and query results, WHEN they are produced, THEN compiler/LSP-grade edges and heuristic (tree-sitter/regex-level) edges are distinct in the data, and consumers can tell which is which — an inferred relationship is never rendered as certain.
- **Priority:** must
- **Source:** `ARCH/26-CODE.md` §1/§2/§12
- **Acceptance:** every edge/query result exposes a precision class; a heuristic-only edge is labeled inferred in agent and UI projections; precision survives projections.
- **Failure cases:** heuristic edge shown as compiler-grade → defect; precision field lost in a projection → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CODE-003 — RepoMap is a bounded, deterministic signature projection
- **Statement:** GIVEN the RepoGraph, a token budget and task hints, WHEN a RepoMap is produced, THEN it is a deterministic signature-level projection (declarations + key refs, not bodies) within the declared token allowance — zero budget yields zero map — with stable ordering for the cache prefix.
- **Priority:** must
- **Source:** `ARCH/26-CODE.md` §1/§3 · `ARCH/16-CONTEXT.md` §3/§5 · `ARCH/04-DECISIONS.md` DEC-027
- **Acceptance:** identical (graph, budget, hints) yields byte-identical output; an oversize case drops whole entries to fit; zero-budget emits nothing; no file bodies appear in the map.
- **Failure cases:** nondeterministic ordering → defect; budget overrun → defect; body dump → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CODE-004 — LSP bridge enriches precisely and degrades cleanly
- **Statement:** GIVEN a language server for the workspace language set, WHEN it is available, THEN definitions/references/hover/diagnostics/symbols resolve through it (rename policy-gated, lifecycle under `19`); WHEN it is absent or crashed, THEN queries degrade to RepoGraph + ripgrep with diagnostics marked unavailable — never a silent empty answer.
- **Priority:** must
- **Source:** `ARCH/26-CODE.md` §4 · `ARCH/19-RUNTIME-ENVIRONMENTS.md` §3 · `ARCH/16-CONTEXT.md` §2
- **Acceptance:** LSP-down test returns graph/search results with an explicit unavailable marker; rename without policy approval is denied; diagnostics surface as context items.
- **Failure cases:** silent empty result on LSP crash → defect; ungated rename → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CODE-005 — Retrieval returns refs and bounded excerpts only
- **Statement:** GIVEN a lexical (ripgrep), structural (graph) or combined query, WHEN results return, THEN each result is a `file:range` ref with a bounded excerpt, path-scoped by policy, and whole-file dumps are never the default.
- **Priority:** must
- **Source:** `ARCH/26-CODE.md` §5 · `ARCH/12-TRUST.md` §2 · `ARCH/16-CONTEXT.md` §2
- **Acceptance:** excerpt-size bound test; out-of-scope search path denied; resolving a result's content is a separate permission-checked read.
- **Failure cases:** whole-file dump by default → defect; unscoped search → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-CODE-006 — Worktrees are per-spawn options with explicit merge and cleanup
- **Statement:** GIVEN isolated coding work, WHEN a worktree is provisioned, THEN isolation is requested per spawn (cheap read-only work does not pay it), the branch strategy is declared per task, worktree-isolated writers hold write leases on their checkout, merging is an explicit reviewed step with receipts, and abandoned worktrees are cleaned with a receipt.
- **Priority:** must
- **Source:** `ARCH/26-CODE.md` §1/§6 · `ARCH/04-DECISIONS.md` DEC-029 · `ARCH/25-FILES.md` §6
- **Acceptance:** isolation-request test creates a worktree; merge requires diff review + tests and emits a receipt; abandoned-worktree cleanup is receipted.
- **Failure cases:** silent merge into the parent workspace → violation; worktree leak without a cleanup receipt → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CODE-007 — Destructive git operations are policy-gated
- **Statement:** GIVEN a destructive history or remote operation (force-push, reset, rebase rewrite), WHEN it is requested, THEN it is policy-gated and requires explicit approval — no silent history rewrite occurs.
- **Priority:** must
- **Source:** `ARCH/26-CODE.md` §6 · `ARCH/12-TRUST.md` §3
- **Acceptance:** force-push denied without approval; the decision is recorded; a rejected operation leaves the repository unchanged.
- **Failure cases:** silent rebase/reset → violation; force-push outside policy → catastrophic-class violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CODE-008 — Code execution walks the governed path
- **Statement:** GIVEN a **Core-mediated** code execution request (`code.run` · `code.test` · `code.build` · `code.lint`), WHEN it runs, THEN it executes inside a declared environment under Core exec policy with a ticket. A self-contained external agent may run native shell/tools under its own policy; Core labels and reconciles that path separately (DEC-054).
- **Priority:** must
- **Source:** `ARCH/26-CODE.md` §1/§7 · `AGENTCOWORK-SPEC.md` §4 · `ARCH/05-INVARIANTS.md` INV-01/INV-03 · `ARCH/03-HLD.md` §5
- **Acceptance:** no direct subprocess bypass exists inside the Core wrapper; the mediated wrapper shows handle + ticket + receipt; native shell activity is not mislabelled as Core-governed.
- **Failure cases:** Core wrapper bypasses ticket → violation; native effect falsely given a Core receipt → false assurance.
- **Tests:** pending
- **Status:** seeded

#### REQ-CODE-009 — Execution output is bounded with a durable full log
- **Statement:** GIVEN a build/test/run execution, WHEN it completes, THEN output capture is bounded (full log → artifact ref, compact view → context) and long jobs run on the background lane with observable status.
- **Priority:** must
- **Source:** `ARCH/26-CODE.md` §7 · `ARCH/16-CONTEXT.md` §4 · `ARCH/11-WORK.md` §3 · `ARCH/05-INVARIANTS.md` INV-22
- **Acceptance:** output-bound test keeps the compact view within budget while the full log stays retrievable as an artifact; a long job lands on the background lane; no unbounded output enters context.
- **Failure cases:** full output into context → INV-22 violation; lost log → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CODE-010 — Index freshness follows file deltas; stale edges are visible
- **Statement:** GIVEN file changes reported by `25` watcher deltas, WHEN the index updates, THEN changed files are reindexed, stale edges are flagged rather than served as fresh, and queries prefer fresh subgraphs.
- **Priority:** must
- **Source:** `ARCH/26-CODE.md` §2 · `ARCH/25-FILES.md` §3
- **Acceptance:** watcher-delta test updates only affected files; a stale-edge query is flagged; queries prefer the fresh subgraph.
- **Failure cases:** stale edge served as current → defect; a delta ignored → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CODE-011 — Index bounds and the v1 language set are declared
- **Statement:** GIVEN a large or mixed repository, WHEN indexing runs, THEN ignore rules (node_modules/vendor/build), size caps and the v1 tree-sitter/LSP language set are declared and enforced, and unsupported languages degrade to lexical results instead of failing the index.
- **Priority:** should
- **Source:** `ARCH/26-CODE.md` §2/§10
- **Acceptance:** ignore-rule test excludes declared paths; an oversized file is skipped with a flag; an unsupported-language file remains lexically searchable.
- **Failure cases:** unbounded index over vendor trees → defect; unsupported language aborting the whole index → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CODE-012 — Generated files carry provenance; direct edits are flagged
- **Statement:** GIVEN a generated file, WHEN it is indexed or edited, THEN `generated_by` provenance is recorded and a direct edit to a generated file is flagged rather than silently accepted.
- **Priority:** should
- **Source:** `ARCH/26-CODE.md` §2/§8
- **Acceptance:** provenance resolves to the producing step; an edit-to-generated test raises a flag; regeneration updates the edge.
- **Failure cases:** generated file edited without a flag → defect; provenance lost → defect.
- **Tests:** pending
- **Status:** seeded

### Search (`SEARCH`)

#### REQ-SEARCH-001 — One search implementation
- **Statement:** GIVEN any search need (UI, agent, service, workflow), WHEN it is served, THEN it resolves through the single Core search service (`ARCH/27-SEARCH.md`, consumed through contracts; `context.search` — `CTR-006` — is the assembly-facing façade) — no second search path, index or parallel implementation exists.
- **Priority:** must
- **Source:** `ARCH/27-SEARCH.md` §1 · `ARCH/03-HLD.md` §4 · `ARCH/16-CONTEXT.md` §1.1 · `AGENTCOWORK-SPEC.md` §8
- **Acceptance:** static check finds one search implementation; every consumer calls through the service; no module-local ad-hoc search over another owner's store.
- **Failure cases:** a second search path → architecture violation; a consumer querying a source store directly → review failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-SEARCH-002 — Deterministic, model-free queries
- **Statement:** GIVEN any search query, WHEN it executes, THEN it performs zero model calls and no network activity — lexical and structured retrieval only.
- **Priority:** must
- **Source:** `ARCH/27-SEARCH.md` §1/§6 · `AGENTCOWORK-SPEC.md` §9 · `ARCH/05-INVARIANTS.md` INV-13 · `ARCH/04-DECISIONS.md` DEC-015
- **Acceptance:** traces show zero model calls and zero network I/O for search; token accounting attributes no spend; results are reproducible for identical inputs.
- **Failure cases:** a model call on a search path → INV-13 violation; network fetch inside local search → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-SEARCH-003 — Scope and sensitivity are enforced before querying
- **Statement:** GIVEN a query, WHEN it runs, THEN it carries scopes and a sensitivity ceiling from the caller's policy snapshot, and out-of-scope sources are never queried for that caller — filtering is not an after-the-fact step.
- **Priority:** must
- **Source:** `ARCH/27-SEARCH.md` §1/§5 · `ARCH/05-INVARIANTS.md` INV-10 · `ARCH/12-TRUST.md` §8
- **Acceptance:** an out-of-scope source is untouched in query traces; a sensitive hit for lower clearance is filtered before scoring; cross-project leakage is zero.
- **Failure cases:** post-hoc filtering after a source was read → violation; unscoped query → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-SEARCH-004 — External agents receive a filtered search projection
- **Statement:** GIVEN an external agent, WHEN it queries search, THEN it sees only its own project plus granted scopes, and out-of-scope refs resolve to `NotFound` — deny-by-default.
- **Priority:** must
- **Source:** `ARCH/27-SEARCH.md` §5/§8 · `ARCH/04-DECISIONS.md` DEC-009 · `ARCH/05-INVARIANTS.md` INV-11
- **Acceptance:** external-agent query test returns only granted scope; an out-of-scope id is not found; no internal index handles leak.
- **Failure cases:** a cross-project result → security violation; an internal handle exposed → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-SEARCH-005 — Source adapters declare powers and never bypass owners
- **Statement:** GIVEN a source (files metadata, memory, artifacts, world objects, repo symbols, events), WHEN its adapter is registered, THEN it declares supported query forms, freshness semantics and cost class; search composes adapters and never writes or bypasses the owner's store, and structural code queries delegate to `26`.
- **Priority:** must
- **Source:** `ARCH/27-SEARCH.md` §2 · `ARCH/07-CONTRACTS.md` §1
- **Acceptance:** adapter registry shows declared powers; search performs no source-store writes; a code-symbol query routes to RepoGraph.
- **Failure cases:** an adapter accessing a store it does not own → architecture violation; search re-indexing code → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-SEARCH-006 — Query forms compose with a deterministic v1 merge
- **Statement:** GIVEN a query mixing lexical (BM25), structured filters, exact lookups and delegated structural forms, WHEN results are merged, THEN v1 merges by source priority + recency with deterministic tie-breaks (fusion/RRF deferred until a measured trigger), and zero results is a valid outcome.
- **Priority:** must
- **Source:** `ARCH/27-SEARCH.md` §3 · `ARCH/17-MEMORY.md` §12 · `ARCH/16-CONTEXT.md` §11
- **Acceptance:** mixed-form query test; identical inputs produce identical order; empty result sets are returned without padding.
- **Failure cases:** nondeterministic merge order → defect; a fusion layer shipping without the measured trigger → review failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-SEARCH-007 — Canonical result shape; abstention is correct
- **Statement:** GIVEN any result, WHEN it is returned, THEN it carries `ref` · `source` · `score` · `freshness` · bounded `snippet` · `sensitivity`; resolving content is a separate permission-checked read; when nothing matches, nothing is returned.
- **Priority:** must
- **Source:** `ARCH/27-SEARCH.md` §1/§3/§4 · `ARCH/17-MEMORY.md` §6
- **Acceptance:** schema test on every result; a zero-hit query returns an empty set (no placeholder); content resolution performs its own permission check.
- **Failure cases:** missing result fields → defect; padding or guessed results → defect; content embedded without a permission check → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-SEARCH-008 — Ranking is deterministic and explained by declared factors
- **Statement:** GIVEN two comparable results, WHEN they are ranked, THEN order derives from a non-negative relevance base (`ARCH/27-SEARCH.md` §4: `relevance = −bm25` for FTS5) × recency × pin/priority boosts with deterministic tie-breaks, and no personalization beyond the declared factors exists in v1.
- **Priority:** must
- **Source:** `ARCH/27-SEARCH.md` §4/§9
- **Acceptance:** tie-break determinism test; ranking factors observable per result; no personalized-ranking code path.
- **Failure cases:** nondeterministic ranking → defect; hidden personalization → review failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-SEARCH-009 — Result sets are bounded; over-broad queries get guidance
- **Statement:** GIVEN an over-broad query, WHEN it executes, THEN result count and snippet size are capped end-to-end and the caller receives narrowing guidance instead of an unbounded result set.
- **Priority:** must
- **Source:** `ARCH/27-SEARCH.md` §3/§7
- **Acceptance:** caps test enforced at the service (not only in UI); over-broad fixture returns guidance; downstream consumers cannot lift caps.
- **Failure cases:** unbounded result set → defect; caps enforced only in the UI → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-SEARCH-010 — Local metadata search meets the declared latency target
- **Statement:** GIVEN 100k indexed files on the reference profile, WHEN metadata search runs, THEN p95 ≤ 50 ms with local indexes only (no network in the search path).
- **Priority:** should
- **Source:** `ARCH/27-SEARCH.md` §6 · `ARCH/42-EVIDENCE-MAP.md`
- **Acceptance:** the 100k-file benchmark records p95 ≤ 50 ms; the evidence record is attached to the requirement.
- **Failure cases:** target missed without a recorded deviation → defect; a network dependency in the path → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-SEARCH-011 — Staleness and partial failure are surfaced, never silent
- **Statement:** GIVEN a missing or stale source index, or a failing adapter, WHEN results return, THEN freshness is flagged, partial results come with a typed error path, and other sources remain unaffected.
- **Priority:** must
- **Source:** `ARCH/27-SEARCH.md` §7 · `ARCH/21-WORLD-MODEL.md` §4
- **Acceptance:** stale-index fixture flags freshness; adapter-error test returns partial results plus a typed error; healthy sources are unaffected.
- **Failure cases:** stale results presented as fresh → defect; one adapter failure failing the whole query → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-SEARCH-012 — Search returns references, not answers
- **Statement:** GIVEN a search response, WHEN the caller needs content, THEN the response contains refs and bounded snippets only, and answer synthesis happens outside the search service (agent/consumer) — search never synthesizes prose or bypasses a permission check.
- **Priority:** must
- **Source:** `ARCH/27-SEARCH.md` §1/§4
- **Acceptance:** response shapes contain no synthesized answer text; every content read triggered from a result passes a permission check.
- **Failure cases:** search emitting synthesized answers → design violation; result content embedded while bypassing permission → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-SEARCH-013 — Indexed content remains exact-version and permission checked
- **Statement:** GIVEN a Library or workspace item is indexed, WHEN it changes, is re-indexed, or access is revoked, THEN extracted content and retrieval references remain bound to the exact item version and scope, and revocation removes future retrieval access.
- **Priority:** must
- **Source:** DEC-055; `ARCH/27-SEARCH.md` §2.1/§5; `ARCH/29-ARTIFACTS.md` §4; `ARCH/48-EXPERIENCE-SURFACES.md` §6
- **Acceptance:** deterministic fixtures prove version-specific source anchors, bounded extraction status, atomic re-index, permission-checked dereference, and exclusion after revocation/deletion.
- **Failure cases:** stale chunks returned as current; path-only identity; partial re-index mixed with old version; revoked content still recalled; extraction error hidden.
- **Tests:** pending
- **Status:** accepted
### Communication (`COMMS`)

#### REQ-COMMS-001 — Capabilities, never a client
- **Statement:** GIVEN communication functionality, WHEN it is exposed, THEN it is capability verbs (`mail.*` · `calendar.*` · `messaging.*` · `web.*`) over connectors — the product never ships a second inbox/messaging client or a mirrored mailbox.
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §1/§3/§9 · `AGENTCOWORK-SPEC.md` §8
- **Acceptance:** capability-surface test; no mailbox mirror store exists; UI surfaces act through capability calls.
- **Failure cases:** a mirror inbox → architecture violation; a second send path outside capabilities → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-COMMS-002 — Connectors are providers behind the adapter contract
- **Statement:** GIVEN a connector (native HTTP · MCP server · plugin), WHEN it registers, THEN it implements the `14` provider adapter contract and nothing above Capability knows the transport; a transport swap preserves capability contracts.
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §1/§2 · `ARCH/14-PROVIDERS.md` §2/§3 · `ARCH/04-DECISIONS.md` DEC-005
- **Acceptance:** connector-swap test with no capability change; descriptors carry no transport vocabulary; `14` conformance per connector.
- **Failure cases:** transport details leaking into capability contracts → defect; a connector outside the adapter contract → review failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-COMMS-003 — Connector descriptors, consent and vault-only credentials
- **Statement:** GIVEN a connector instance, WHEN it is configured, THEN its descriptor declares id · provider · auth type · scopes · capabilities · sync model · rate limits · data classes, auth flows run locally with tokens landing only in the vault, and a per-instance consent record exists.
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §2 · `ARCH/12-TRUST.md` §6 · `ARCH/21-WORLD-MODEL.md` §5 · `ARCH/05-INVARIANTS.md` INV-02
- **Acceptance:** descriptor-completeness test; secret scan finds no token outside the vault; consent record present per instance; a scope change requires new consent.
- **Failure cases:** token in config/log/event → custody violation; connector active without consent → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-COMMS-004 — On-demand sync; no bulk ingestion
- **Statement:** GIVEN connector data, WHEN it is accessed, THEN queries run on demand plus subscriptions where the provider offers them, caches are bounded (headers only where a capability requires it), and nothing (mailbox/calendar/messages) is mirrored by default.
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §1/§2/§5 · `AGENTCOWORK-SPEC.md` §8
- **Acceptance:** mirror-store absence test; cache-bound test; an offline query reports coverage rather than stale data.
- **Failure cases:** silent bulk mirror → violation; unbounded cache growth → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-COMMS-005 — Sends are approval-gated and receipted
- **Statement:** GIVEN a send-class action (`mail.send` · `messaging.send` · `calendar.create` with invites), WHEN it executes, THEN it follows draft → approval primitive with `edit` semantics → send → receipt; an uncertain outcome lands in `needs_attention` with no silent retry.
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §1/§3/§6 · `ARCH/04-DECISIONS.md` DEC-021 · `ARCH/05-INVARIANTS.md` INV-07 · `ARCH/20-WORKFLOW.md` §7
- **Acceptance:** send without approval is denied; the receipt cites the exact content ref; timeout-after-submit yields `needs_attention` and no duplicate send.
- **Failure cases:** ungated send → violation; duplicate send after an uncertain outcome → defect; receiptless send → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-COMMS-006 — Read/write risk classes are declared and honored
- **Statement:** GIVEN a communication capability, WHEN its risk class is set, THEN reads are `sensitive` and externally visible sends are `dangerous`, default decisions follow the policy model, and content sensitivity defaults apply (message bodies `confidential`, profile metadata `personal`).
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §3/§5 · `ARCH/12-TRUST.md` §3 · `ARCH/06-DATA-MODEL.md` DM-011
- **Acceptance:** descriptor risk-class test; default-decision tests for read/send; sensitivity defaults asserted on stored refs.
- **Failure cases:** send classified `safe` → violation; read exposed without policy evaluation → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-COMMS-007 — Content is untrusted; excerpts bounded; attachments gated
- **Statement:** GIVEN message/calendar/web content, WHEN it enters the agent, THEN it is untrusted input (no instruction authority), enters context only as bounded excerpts on demand, and attachments move through the artifact gateway with the connector's permissions recorded.
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §3/§5 · `ARCH/29-ARTIFACTS.md` §5 · `ARCH/16-CONTEXT.md` §2 · `ARCH/04-DECISIONS.md` DEC-037
- **Acceptance:** injection-corpus test (content never auto-executes); excerpt-bound test; attachment permission record present.
- **Failure cases:** message content treated as instructions → violation; bulk bodies imported into memory → violation; attachment bypassing the gateway → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-COMMS-008 — Arrival events carry refs, not bodies, with dedupe
- **Statement:** GIVEN `email.arrived` · `message.received` · `calendar.event.upcoming`, WHEN published, THEN payloads carry refs + metadata only (never full bodies) with provenance and dedupe keys, and workflows may subscribe; content fetches remain separate permission-checked capability calls.
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §4 · `ARCH/30-EVENTS.md` · `ARCH/20-WORKFLOW.md` §5
- **Acceptance:** payload-shape test (no bodies); duplicate delivery deduped by key; a subscribed workflow triggers once per distinct event.
- **Failure cases:** body content in an event payload → violation; duplicate event causing a duplicate side effect → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-COMMS-009 — Accounts and scopes are isolated
- **Statement:** GIVEN multiple accounts, WHEN a query or send runs, THEN scopes isolate accounts and cross-account access is explicit — never implicit.
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §5
- **Acceptance:** cross-account query denied without an explicit scope; per-account token/scope isolation; an account switch does not merge results.
- **Failure cases:** implicit account mixing → violation; cross-account send → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-COMMS-010 — Connector failures are typed, bounded and honest
- **Statement:** GIVEN connector failures (expired/revoked token · rate limit · partial sync · uncertain send · abuse guard), WHEN they occur, THEN behavior is bounded and typed: re-auth guidance with no cached-credential fallback, backoff + queue, flagged coverage, `needs_attention`, and per-connector rate caps.
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §7 · `ARCH/12-TRUST.md` §6 · `ARCH/13-CAPABILITY.md` §3
- **Acceptance:** failure-mode matrix tests (one per row); no fallback to cached credentials; persistent limits surfaced to the user.
- **Failure cases:** silent fallback to stale credentials → violation; unbounded retry → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-COMMS-011 — `web.search` is a bounded, cited capability
- **Statement:** GIVEN a `web.search` call, WHEN it executes, THEN it honors declared affordances (query · count ≤20 with default 8 · freshness · type · domain allow/block · locale), returns results carrying `{ref, url, title?, retrieved_at, …}`, and consumes the per-session search budget counted across subagents.
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §3 (Web search & fetch) · `ARCH/04-DECISIONS.md` DEC-037 · `AGENTCOWORK-SPEC.md` §3
- **Acceptance:** cap test (hard max 20, default 8); budget test across subagents; citation-fields test; domain filters honored.
- **Failure cases:** uncited result → defect; budget bypass via a subagent → violation; count over 20 → rejected.
- **Tests:** pending
- **Status:** seeded

#### REQ-COMMS-012 — `web.fetch` content is size-capped, cached transparently, and cited
- **Statement:** GIVEN a `web.fetch` call, WHEN it executes, THEN it enforces caps (5 MB body · timeout ≤120 s · declared max chars/tokens), caches per session by `(normalized URL, format)` with a default 15-minute TTL and an explicit `fresh` bypass, and returns `retrieved_at` (+ `sha256`) with content kept as an artifact ref.
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §3 (Web search & fetch) · `ARCH/04-DECISIONS.md` DEC-037
- **Acceptance:** cap tests; cache key/TTL tests; `fresh` bypass test; a cached result always surfaces its retrieval time; no silent page substitution.
- **Failure cases:** uncapped fetch → defect; cache key collision → defect; silently serving a different page → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-COMMS-013 — Web access obeys custody, egress and no-evasion rules
- **Statement:** GIVEN any web search/fetch, WHEN it leaves the machine, THEN provider credentials travel only via vault refs (never in URLs), egress passes Guard with a domain allow/block policy that overrides model requests, the SSRF floor rejects localhost/no-dot/private/link-local/metadata targets after resolution, cross-host redirects are surfaced, and fetched content is untrusted — robots/ToS honored, no evasion.
- **Priority:** must
- **Source:** `ARCH/28-COMMS.md` §3 (Web search & fetch) · `ARCH/05-INVARIANTS.md` INV-02/INV-05 · `ARCH/04-DECISIONS.md` DEC-016/DEC-037
- **Acceptance:** SSRF corpus test (resolve-then-check); key-in-URL scan clean; a model-requested domain denied when policy blocks it; redirect host changes surfaced; no CAPTCHA/anti-bot code path exists.
- **Failure cases:** credential in a URL/query → custody violation; SSRF target reached → security failure; evasion behavior → catastrophic-class violation.
- **Tests:** pending
- **Status:** seeded

### Artifacts & Receipts (`ART`)

#### REQ-ART-001 — Artifact versions are immutable; edits create new versions
- **Statement:** GIVEN an artifact, WHEN a new edit is written, THEN a new immutable version is created (prior versions unchanged), and derived work records lineage via `parent_artifact?`.
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §2 · `ARCH/06-DATA-MODEL.md` DM-019 · `ARCH/05-INVARIANTS.md` INV-18
- **Acceptance:** version-write test (previous version bytes unchanged); an edit produces version+1; a derived artifact carries its lineage link.
- **Failure cases:** in-place mutation of a written version → violation; silent overwrite → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-ART-002 — Provenance is mandatory on every artifact
- **Statement:** GIVEN any artifact created by agent/workflow/user/worker, WHEN it is recorded, THEN it carries a provenance chain (`created_by_agent → run → worker → workflow`) plus an inputs digest, and provenance-less artifacts are rejected.
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §1/§2 · `ARCH/06-DATA-MODEL.md` DM-019 · `ARCH/05-INVARIANTS.md` INV-18
- **Acceptance:** provenance-completeness test; creation without provenance fails typed; the chain resolves to session/run refs.
- **Failure cases:** missing provenance → rejected; fabricated provenance → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-ART-003 — Receipts are mandatory for externally visible effects
- **Statement:** GIVEN an externally visible effect, WHEN it completes, THEN a receipt is emitted inside the governed path (`12` → `13` → `34` → receipt), and the effect path cannot commit without it.
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §3/§8 · `ARCH/05-INVARIANTS.md` INV-07 · `ARCH/04-DECISIONS.md` DEC-022 · `AGENTCOWORK-SPEC.md` §4
- **Acceptance:** an effect-without-receipt attempt is blocked before commit; every committed effect has a receipt citing effect/ticket/verification.
- **Failure cases:** visible effect without receipt → violation; receipt written outside the governed path → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-ART-004 — Receipts are immutable; replay is evidence replay
- **Statement:** GIVEN a recorded receipt, WHEN `replay(receipt)` runs, THEN it reconstructs inputs and shows what verification ran and what changed; re-doing the action is a new work item, never a silent re-fire.
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §3 · `ARCH/07-CONTRACTS.md` CTR-018 · `ARCH/05-INVARIANTS.md` INV-07
- **Acceptance:** receipt-mutation attempt fails; replay performs no provider call or side effect; replay output matches the recorded verification.
- **Failure cases:** replay re-executing the effect → violation; mutated receipt → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-ART-005 — Receipt emission yields three views of one fact
- **Statement:** GIVEN a receipt emission, WHEN it occurs, THEN it also writes an event (`30`) and an audit entry (`12`) — three views of one fact with no duplicated state.
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §3 · `ARCH/30-EVENTS.md` §3 (`receipt.recorded`) · `ARCH/12-TRUST.md` §9 · `ARCH/05-INVARIANTS.md` INV-23/INV-24
- **Acceptance:** emission produces exactly one event and one audit entry referencing the receipt; no store duplicates the receipt payload.
- **Failure cases:** receipt without an event or audit entry → defect; duplicated receipt state across stores → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-ART-006 — Library promotion is explicit and versioned
- **Statement:** GIVEN a reusable inventory item, WHEN it is promoted from work ("Save to Library" / "Save as template"), THEN promotion is explicit (never automatic), the Library item is global and durable with a `DM-023` kind, and versioning/deprecation are explicit operations.
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §4 · `ARCH/04-DECISIONS.md` DEC-014 · `ARCH/06-DATA-MODEL.md` DM-023
- **Acceptance:** no automatic library entries after work completion; promotion requires an explicit action; artifact scope ≠ library scope test.
- **Failure cases:** auto-promotion → violation; library item without a promotion origin where applicable → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-ART-007 — Artifact gateway exchanges refs only under permissions
- **Statement:** GIVEN an external agent, WHEN it reads/writes/attaches/transforms/publishes an artifact, THEN exchange is by refs (`artifact_id` · `mime_type` · `uri`) under per-verb permission checks; raw storage paths are never exposed, and v1 is workspace-scoped — a cross-workspace request is denied typed (explicit export only).
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §5 · `ARCH/41-EDGE-CASES.md` EDGE-105 · `ARCH/05-INVARIANTS.md` INV-11
- **Acceptance:** gateway verbs are gated per permission; no path leakage in responses or errors; a cross-workspace request is denied typed.
- **Failure cases:** raw path in a projection → security failure; unpermissioned verb → violation; implicit cross-workspace access → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-ART-008 — Receipt-pinned versions are never GC'd
- **Statement:** GIVEN garbage collection or retention pruning, WHEN a version is pinned by a receipt, THEN the pin check runs in the GC transaction and the version is skipped — chain integrity outranks storage savings.
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §6 · `ARCH/41-EDGE-CASES.md` EDGE-100 · `ARCH/04-DECISIONS.md` DEC-032 · `AGENTCOWORK-SPEC.md` §7
- **Acceptance:** GC-vs-receipt race test (pinned version survives); an unpinned version is pruned per policy; the GC outcome is recorded.
- **Failure cases:** receipt-pinned version collected → violation; pin/GC race → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-ART-009 — Retention pruning and explicit delete are audited
- **Statement:** GIVEN retention policy or an explicit delete, WHEN versions are pruned/deleted, THEN pruning follows the per-workspace age/count policy, delete is a user/authorized operation, and both are audited.
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §6 · `ARCH/05-INVARIANTS.md` INV-24 · `ARCH/04-DECISIONS.md` DEC-032
- **Acceptance:** a deletion audit entry is present; an unauthorized delete is denied; retained versions match policy; media follows the same rules.
- **Failure cases:** silent deletion → violation; delete outside authorization → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-ART-010 — Previews are projections; rendering consumes zero model tokens
- **Statement:** GIVEN an artifact preview, WHEN the UI opens/renders it, THEN artifacts store render refs (thumbnail/render refs produced by domains, not pixels), and opening/rendering consumes zero model tokens.
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §7 · `ARCH/04-DECISIONS.md` DEC-015 · `AGENTCOWORK-SPEC.md` §9
- **Acceptance:** opening a preview makes no model call; the artifact record stores refs; the render ref resolves through the domain renderer.
- **Failure cases:** preview generating a model call → violation; artifact storing raw pixels → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-ART-011 — Unresolved artifact locations keep digest and re-link
- **Statement:** GIVEN a moved/deleted backing location, WHEN the identity check (`25`) detects it, THEN the artifact is marked `unresolved`, receipts referencing it keep the digest, and the item is surfaced for re-link — never silently re-pointed.
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §8 · `ARCH/41-EDGE-CASES.md` EDGE-101 · `ARCH/25-FILES.md` §2
- **Acceptance:** a moved-location test shows `unresolved` + re-link guidance; the receipt digest is unchanged; no path is guessed.
- **Failure cases:** artifact silently re-pointed to a different file → violation; receipt digest lost → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-ART-012 — Write failures are atomic; effects cannot complete without a receipt
- **Statement:** GIVEN a write failure mid-version or disk full during an artifact/receipt write, WHEN it occurs, THEN no partial version or receipt becomes visible (retry or discard), and a mandatory-receipt effect cannot complete — it pauses with reason.
- **Priority:** must
- **Source:** `ARCH/29-ARTIFACTS.md` §8 · `ARCH/41-EDGE-CASES.md` EDGE-104 · `ARCH/05-INVARIANTS.md` INV-07
- **Acceptance:** crash/disk-full injection leaves no partial records; the effect pauses instead of committing; the failure is audited.
- **Failure cases:** partial version visible → defect; effect committed with a missing receipt → violation.
- **Tests:** pending
- **Status:** seeded

### Events (`EVENTS`)

#### REQ-EVENTS-001 — One event store and one bus; projections are derived
- **Statement:** GIVEN any UI projection, workflow trigger, world update, telemetry or audit feed, WHEN it needs system state, THEN it derives from the single event store + bus — no second source of truth and no hidden side channels.
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §1/§4 · `ARCH/05-INVARIANTS.md` INV-23 · `ARCH/04-DECISIONS.md` DEC-027
- **Acceptance:** projections rebuild from the store alone; no parallel state store exists for runs/pending work/world.
- **Failure cases:** a second source of truth → violation; a projection that cannot be rebuilt → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-EVENTS-002 — The stream vocabulary is typed and declared
- **Statement:** GIVEN an event, WHEN it is published, THEN its type is a namespaced dotted name with a declared payload schema; opaque "output chunk" events do not exist; deprecations are declared with a window.
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §2/§3 · `ARCH/06-DATA-MODEL.md` DM-008
- **Acceptance:** schema-validation test per type; an unregistered type is rejected; no opaque chunk event classes exist.
- **Failure cases:** payload without a schema → defect; silent type rename/removal → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-EVENTS-003 — Events carry refs, never payload data or secrets
- **Statement:** GIVEN a material event, WHEN it is published, THEN its payload is bounded metadata/refs — large data lives in artifacts/stores, and credentials/PII never appear.
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §1/§2 · `ARCH/05-INVARIANTS.md` INV-02 · `ARCH/06-DATA-MODEL.md` DM-008
- **Acceptance:** payload-shape test; secret scan over emitted events is clean; large blobs are referenced, not embedded.
- **Failure cases:** secret/PII in a payload → custody violation; unbounded payload → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-EVENTS-004 — At-least-once delivery with idempotent consumers
- **Statement:** GIVEN event delivery, WHEN an event is delivered (possibly more than once), THEN consumers dedupe by event id and ordering is guaranteed per work/session — never globally.
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §1/§4 · `ARCH/06-DATA-MODEL.md` DM-008
- **Acceptance:** duplicate-delivery test produces one side effect; per-work ordering test passes.
- **Failure cases:** duplicate side effect → defect; a consumer relying on global ordering → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-EVENTS-005 — Publish applies backpressure; slow consumers get lag markers
- **Statement:** GIVEN a slow consumer or an event storm, WHEN queues fill, THEN bounded queues apply producer backpressure and emit lag markers with pull-based catch-up — memory never grows unbounded.
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §4/§7 · `ARCH/41-EDGE-CASES.md` EDGE-076
- **Acceptance:** storm test shows bounded memory plus lag markers; catch-up reads from the store; no silent event loss.
- **Failure cases:** unbounded queue growth → defect; event loss without a marker → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-EVENTS-006 — Subscriptions are filtered and authorized
- **Statement:** GIVEN a subscriber, WHEN it subscribes, THEN delivery is filtered by type/refs/scope and authorized; a subscriber never receives out-of-scope events.
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §4 · `ARCH/05-INVARIANTS.md` INV-11 · `ARCH/04-DECISIONS.md` DEC-009
- **Acceptance:** an out-of-scope subscription is denied; filter tests pass; authorization decisions are auditable.
- **Failure cases:** unauthorized subscription → violation; scope leakage → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-EVENTS-007 — Replay rebuilds projections after restart
- **Statement:** GIVEN a restart or detected drift, WHEN `read(range)` replays the store, THEN projections (Runs, pending work, world state) are reconstructable from the store + checkpoints.
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §4 · `ARCH/04-DECISIONS.md` DEC-027/DEC-033
- **Acceptance:** kill/restart test rebuilds projections consistently; drift triggers a rebuild; replay never calls producers.
- **Failure cases:** projection not rebuildable → violation; replay invoking producers → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-EVENTS-008 — Retention prunes events, never receipts/audit; gaps are reported
- **Statement:** GIVEN retention policy, WHEN durable events are pruned by age per family, THEN receipts/audit are separate stores and are not pruned with events; a pruned range a projection genuinely still needs is reported, never silently answered.
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §4/§7 · `ARCH/41-EDGE-CASES.md` EDGE-106 · `ARCH/29-ARTIFACTS.md` §6 · `ARCH/12-TRUST.md` §9
- **Acceptance:** retention test (events pruned, receipts/audit intact); a gap produces a surfaced report; projections rebuild from checkpoints.
- **Failure cases:** receipts/audit pruned with events → violation; silent gap → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-EVENTS-009 — Poison events quarantine without stalling the stream
- **Statement:** GIVEN an unprocessable event, WHEN it is encountered, THEN it is quarantined with a reconciliation entry and the stream continues.
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §4/§7
- **Acceptance:** poison-injection test shows quarantine + continued delivery; a reconciliation entry exists.
- **Failure cases:** one event blocks the stream → defect; a dropped event without quarantine → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-EVENTS-010 — Usage/cost telemetry is counts and refs only
- **Statement:** GIVEN a model call, WHEN `usage.recorded` is published, THEN it carries model tokens in/out, estimated cost, latency, provider/model id and work/session refs — no prompt or completion content — and powers per-work budget checks.
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §5 · `ARCH/11-WORK.md` §5
- **Acceptance:** telemetry-shape test (no content fields); budget checks and analytics derive from the same aggregations.
- **Failure cases:** prompt/completion content in telemetry → privacy violation; budget bypass via telemetry → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-EVENTS-011 — External agents receive a filtered, stable projection
- **Statement:** GIVEN an external agent, WHEN it consumes events, THEN it receives a filtered stream for its own work only (session/run/tool/artifact/approval/context events), sensitivity-filtered, as a declared stable subset with deprecation windows — never the internal bus and never other agents' events.
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §6 · `ARCH/04-DECISIONS.md` DEC-009 · `ARCH/05-INVARIANTS.md` INV-11
- **Acceptance:** cross-agent isolation test; delivered vocabulary matches the declared subset; a version change returns a typed error naming the supported window.
- **Failure cases:** internal bus exposure → violation; another agent's events delivered → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-EVENTS-012 — `model.delta` is ephemeral delivery only
- **Statement:** GIVEN streaming tokens, WHEN `model.delta` events are emitted, THEN they are ephemeral delivery only — individual deltas are not persisted; the settled message is.
- **Priority:** must
- **Source:** `ARCH/30-EVENTS.md` §3
- **Acceptance:** persistence test shows no per-delta rows; replay shows settled messages; losing ephemeral deltas loses nothing.
- **Failure cases:** per-delta persistence → defect; settled message missing → defect.
- **Tests:** pending
- **Status:** seeded

### Skills & Plugins (`SKILL`)

#### REQ-SKILL-001 — A skill is not a capability; requirements resolve through the graph
- **Statement:** GIVEN a skill's declared requirements, WHEN it runs, THEN it resolves through the capability graph (`13`) under policy (`12`) and never bypasses them to execute effects directly.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §1/§3 · `ARCH/13-CAPABILITY.md` §7 · `ARCH/12-TRUST.md` §3
- **Acceptance:** skill invocation produces capability calls only; no direct-executor path exists; requirement resolution uses the graph.
- **Failure cases:** a skill bypassing `13`/`12` → violation; direct effect execution from a skill → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-SKILL-002 — Activation is scoped and relevance-loaded
- **Statement:** GIVEN a session/run, WHEN skills activate, THEN the four-state model applies (Installed → Available → Activated → Executing), activation sets are per session/run, and skill bodies are never bulk-dumped into context — only activated instructions enter, bounded by `16`.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §1/§3 · `ARCH/16-CONTEXT.md` §3
- **Acceptance:** context contains no inactive skill instructions; activation sets are per session/run; bounded-injection test passes.
- **Failure cases:** bulk skill dump → violation; skill body in the stable prefix unless pinned → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-SKILL-003 — The skill model and package format are declared with provenance
- **Statement:** GIVEN a skill, WHEN it is registered, THEN it conforms to `DM-027` (id/version, metadata, instructions ref, capability requirements, I/O contracts, example refs) with a content-addressed digest, and its package is a directory with a manifest + instructions + optional scripts/resources.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §2 · `ARCH/06-DATA-MODEL.md` DM-027
- **Acceptance:** schema-conformance test; digest present and verified; package scan matches the declared format.
- **Failure cases:** skill without provenance/digest → rejected; package format drift → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-SKILL-004 — Missing requirements become guidance; activation never grants permissions
- **Statement:** GIVEN a skill whose capability requirements are unavailable, WHEN it attempts activation, THEN it activates in `guidance` mode naming the missing requirement, and activation never grants permissions — policy still decides.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §3 · `ARCH/13-CAPABILITY.md` §3/§9 · `ARCH/12-TRUST.md` §3
- **Acceptance:** the guidance result names the missing requirement; a permission-requiring invocation still hits policy; no silent-degradation path exists.
- **Failure cases:** silently skipping requirements → defect; activation widening permissions → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-SKILL-005 — Plugins extend at declared surfaces only
- **Statement:** GIVEN a plugin, WHEN it is installed, THEN it contributes only through the declared surfaces (capability · skill pack · provider · agent runtime · model adapter · channel · UI contribution · workflow template), carries a manifest (id · version · surfaces[] · permissions · hooks · compat window), and never patches Core.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §1/§4
- **Acceptance:** surface-declaration test; an undeclared surface is rejected at review; no privileged Core-patching path exists.
- **Failure cases:** Core patching → violation; contribution outside declared surfaces → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-SKILL-006 — Install passes a review gate before explicit enable
- **Statement:** GIVEN a local file/folder install (v1 local-first), WHEN it is installed, THEN it passes the review gate (declared surfaces · requested permissions · provenance) before any explicit per-scope enable.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §5 · `AGENTCOWORK-SPEC.md` §12
- **Acceptance:** install without review is impossible; enable is explicit and scoped; review findings are recorded.
- **Failure cases:** auto-enable after install → violation; review bypass → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-SKILL-007 — Plugin code executes sandboxed without ambient authority
- **Statement:** GIVEN enabled plugin code, WHEN it runs, THEN it executes sandboxed (`19`) under the exec policy (`12`) with explicit, recorded grants and no ambient authority.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §5 · `ARCH/19-RUNTIME-ENVIRONMENTS.md` §4 · `ARCH/04-DECISIONS.md` DEC-028
- **Acceptance:** sandbox-confinement test; an undeclared privileged action is denied + audited; grants are recorded per plugin.
- **Failure cases:** ambient authority → security failure; sandbox escape → catastrophic-class violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-SKILL-008 — Updates are versioned and compatibility-checked
- **Statement:** GIVEN a plugin/skill update, WHEN it lands, THEN it is versioned and checked against Core contract versions (`07` §0); a breaking mismatch is rejected with a typed error and a compat-window explanation.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §5 · `ARCH/07-CONTRACTS.md` §0
- **Acceptance:** a skew test rejects with the compat explanation; a compatible update applies; the active version is reported.
- **Failure cases:** silently applying a breaking version → violation; untyped rejection → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-SKILL-009 — Crash loops auto-disable with audit; failures stay isolated
- **Statement:** GIVEN repeated plugin crashes, WHEN the threshold is hit, THEN the plugin auto-disables with an audit entry and Core remains unaffected.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §5/§7
- **Acceptance:** crash-loop test disables once + audits; Core health is unaffected; no crash propagation.
- **Failure cases:** crash loop left running → defect; Core instability caused by a plugin crash → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-SKILL-010 — Uninstall removes code + owned data, preserving evidence
- **Statement:** GIVEN an uninstall, WHEN it completes, THEN code and plugin-owned data are removed, while Library entries and receipts referencing it are preserved.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §5 · `ARCH/29-ARTIFACTS.md` §6
- **Acceptance:** post-uninstall scan finds no plugin code/data; library entries + receipts intact; deletion is audited.
- **Failure cases:** orphaned plugin code/data → defect; receipts deleted with the plugin → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-SKILL-011 — Packages are extracted under bounds and confinement
- **Statement:** GIVEN a skill/plugin package, WHEN it is extracted, THEN extraction is bounded and confined to declared roots (pathfloor), the manifest + provenance pass the review gate, and rejection is typed — nothing lands outside the package.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §5 · `ARCH/41-EDGE-CASES.md` EDGE-094 · `ARCH/25-FILES.md` §7
- **Acceptance:** traversal/oversize corpus test (rejected typed); nothing written outside the declared root; the review gate is invoked.
- **Failure cases:** path traversal writing outside roots → security failure; unbounded extraction → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-SKILL-012 — Skill/plugin instructions are untrusted content
- **Statement:** GIVEN skill/plugin instructions, WHEN they enter context or execution, THEN they are treated as untrusted content with provenance and the same injection hygiene as other external content — never as instruction authority.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §1/§7 · `ARCH/41-EDGE-CASES.md` EDGE-093
- **Acceptance:** injection-corpus test (embedded instructions do not expand privileges); provenance is shown; the review gate flags suspicious content.
- **Failure cases:** skill instructions treated as system authority → security failure; an unreviewed package activated → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-SKILL-013 — In-flight executions finish on their loaded version
- **Statement:** GIVEN a plugin update landing while its code executes, WHEN the update arrives, THEN in-flight execution finishes on the loaded version and the new version applies at the next activation — never a hot-swap mid-call; breaking skew is rejected.
- **Priority:** must
- **Source:** `ARCH/31-SKILLS-PLUGINS.md` §5 · `ARCH/41-EDGE-CASES.md` EDGE-096
- **Acceptance:** update-during-execution test (the call completes on the old version; the next activation uses the new); skew rejected.
- **Failure cases:** hot-swap mid-call → defect; two versions serving concurrently without declaration → defect.
- **Tests:** pending
- **Status:** seeded

### Channels (`CHAN`)

#### REQ-CHAN-001 — Surfaces are projections; Core is the only brain
- **Statement:** GIVEN any surface (desktop · CLI · ACP · A2A · API · web/mobile), WHEN it renders, requests or subscribes, THEN it never owns Mission/Work truth — all surfaces use authenticated scoped Core projections and the same `AgentEngine` contract.
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §1 · `AGENTCOWORK-SPEC.md` §2 (P-01)/§3
- **Acceptance:** no surface-local durable state; a surface restart loses nothing; two surfaces observe identical state.
- **Failure cases:** a surface storing authoritative session state → violation; two surfaces forking state → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CHAN-002 — One contract, many protocol mappings
- **Statement:** GIVEN ACP/A2A/API/CLI, WHEN they map onto the internal runtime, THEN all map onto the same contracts (`07`) and no protocol-specific semantics leak inward.
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §1/§4 · `ARCH/05-INVARIANTS.md` INV-15 · `ARCH/07-CONTRACTS.md` §2
- **Acceptance:** a contract-mapping test per protocol; Core code contains no protocol-vocabulary branching; adapters carry the mapping.
- **Failure cases:** protocol semantics in Core → violation; a parallel contract per protocol → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CHAN-003 — The Agent Gateway exposes exactly the 7-item projection
- **Statement:** GIVEN an external agent, WHEN it connects, THEN it receives only the 7-item projection (identity/agent contract · capability projection · context projection · workspace projection · tool/MCP subset · artifacts · events/task state) and never Core internals, stores, queues, vault, policy internals or other agents' state.
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §3 · `ARCH/07-CONTRACTS.md` CTR-022 · `ARCH/04-DECISIONS.md` DEC-009
- **Acceptance:** projection-surface test enumerates exactly 7 items; a request for a non-exposed resource is denied; hidden-property scan clean.
- **Failure cases:** exposing a Core internal → security failure; an implicit 8th surface → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CHAN-004 — The workspace projection intercepts, never un-discovers
- **Statement:** GIVEN workspace boundaries, WHEN an external agent targets a path/tool outside its projection, THEN access is intercepted and denied + audited (`allowed_paths`/`read_only_paths` via pathfloor) — never hidden by un-discovery.
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §3 · `ARCH/12-TRUST.md` §8 · `ARCH/41-EDGE-CASES.md` EDGE-035/EDGE-155
- **Acceptance:** an out-of-scope path is denied + audited; in-scope operations are unaffected; the session is flagged per policy.
- **Failure cases:** silent omission (un-discovery) → violation; an out-of-scope write allowed → security failure.
- **Tests:** pending
- **Status:** seeded

#### REQ-CHAN-005 — Gateway identity is issued, audited and scoped
- **Statement:** GIVEN any gateway connection, WHEN identity is established, THEN it is gateway-issued and audited; local stdio subprocesses use the local trust model, and remote/API binds use gateway-issued tokens.
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §3/§4 · `ARCH/12-TRUST.md` §3
- **Acceptance:** identity issuance is audited; a token bind without valid identity is rejected; local-vs-remote trust paths are distinguishable.
- **Failure cases:** unauthenticated remote bind → security failure; identity not auditable → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CHAN-006 — Approvals route to the owning channel and stay durable
- **Statement:** GIVEN an approval request, WHEN it is raised, THEN it routes to the channel bound to the session/work; with no interactive channel attached it waits durably and re-surfaces on the next attach — notification ≠ receipt.
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §7 · `ARCH/04-DECISIONS.md` DEC-021 · `ARCH/20-WORKFLOW.md` §7 · `ARCH/41-EDGE-CASES.md` EDGE-073
- **Acceptance:** the approval is delivered to the bound channel; a disconnect test keeps it pending; re-attach re-surfaces; no duplicate grant.
- **Failure cases:** approval lost on disconnect → violation; approval delivered to a non-owning channel → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CHAN-007 — ACP maps the typed stream in both directions
- **Statement:** GIVEN ACP, WHEN it operates as a server, THEN it exposes session management + tool registry + typed streaming updates mapping the internal typed stream (`30` §3) onto ACP update classes; as a client, external agents arrive in-process or as stdio ND-JSON subprocesses whose adapters register a factory (`15` §2).
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §4 · `ARCH/30-EVENTS.md` §3 · `ARCH/15-AGENT-PLANE.md` §2
- **Acceptance:** ACP server update-class mapping test; subprocess factory-registration test; no opaque update channel.
- **Failure cases:** ACP-specific semantics leaking into Core → violation; an unregistered adapter path → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CHAN-008 — A2A keeps remote agents opaque
- **Statement:** GIVEN a configured remote agent, WHEN it exchanges work, THEN tasks/messages/artifacts are exchanged through an authenticated transport while its internals stay private; remote runs materialize as Work items like everything else. An unavailable transport is reported as unavailable, never simulated by a registry entry.
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §2/§5 · `AGENTCOWORK-SPEC.md` §15
- **Acceptance:** no remote internal state is imported; a remote run appears as regular Work; a configured authenticated A2A transport can dispatch, observe, cancel and reconcile the run, while an unavailable transport reports that limitation explicitly (DEC-055 target).
- **Failure cases:** importing remote internals → violation; a second execution model for remote runs → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CHAN-009 — The CLI is a thin projection independent of the desktop UI
- **Statement:** GIVEN the CLI, WHEN it runs a prompt/workspace/serve/status command, THEN it uses the same Core state and gateway rules without a CLI-local store; it works with the desktop UI closed when a local service or remote executor is actually available, and otherwise reports paused/unavailable Work truthfully.
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §2/§6 · `ARCH/11-WORK.md` §3/§7
- **Acceptance:** CLI-driven work continues with the UI closed only under a healthy accepted executor; without one it pauses with a reason; no CLI-local state store; the same policy decisions as the desktop surface.
- **Failure cases:** CLI privileged shortcut → violation; CLI-local session state → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-CHAN-010 — Surfaces declare supported capabilities
- **Statement:** GIVEN a surface, WHEN UX behavior is decided (e.g. approval prompts), THEN each surface declares what it supports and the UI/composer follows those declarations.
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §1 · `AGENTCOWORK-UI.md` · `AGENTCOWORK-SPEC.md` §9
- **Acceptance:** an unsupported interaction is never offered; declaration changes are reflected at runtime; no fake controls.
- **Failure cases:** offering an unsupported interaction → defect; surface behavior diverging from declarations → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CHAN-011 — Surface crashes are isolated
- **Statement:** GIVEN a surface crash, WHEN it occurs, THEN Core and other surfaces are unaffected and work continues asynchronously.
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §8 · `ARCH/41-EDGE-CASES.md` EDGE-075
- **Acceptance:** kill-surface test shows continued work; other surfaces are unaffected; re-attach restores the projection.
- **Failure cases:** a surface crash affecting Core → violation; work stopped by a surface crash → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CHAN-012 — Protocol version mismatch is typed with the supported window
- **Statement:** GIVEN a protocol client with a mismatched version, WHEN it connects, THEN it receives a typed error naming the supported window — never a silently missing stream.
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §8 · `ARCH/41-EDGE-CASES.md` EDGE-079 · `ARCH/30-EVENTS.md` §6 · `ARCH/04-DECISIONS.md` DEC-048
- **Acceptance:** a mismatch test returns a typed error + window; compatible clients connect; no silent partial stream; the refusal carries the window as data, not only as prose — `error.data.supportedProtocolVersions` on the MCP façade (DEC-048).
- **Failure cases:** silent vocabulary drop → violation; untyped rejection → defect.
- **Tests:** `crates/agentcowork-mcp/tests/acceptance_mcp_dual_era.rs::{acceptance_the_revision_pin_holds_for_every_method_except_initialize, acceptance_a_comma_duplicated_version_header_is_normalized, acceptance_an_unknown_initialize_version_is_answered_not_echoed}`
- **Status:** implemented

#### REQ-CHAN-013 — External-agent disconnects leave no orphaned state
- **Statement:** GIVEN an ACP/API drop mid-run, WHEN the client re-attaches, THEN Work identity and recorded progress remain durable and the filtered stream replays from the last acknowledgement. Execution continues only if its independently tracked executor remains alive; unknown liveness is reconciled before retry or completion.
- **Priority:** must
- **Source:** `ARCH/32-CHANNELS.md` §3/§7 · `ARCH/41-EDGE-CASES.md` EDGE-077 · `ARCH/11-WORK.md` §4 · `ARCH/30-EVENTS.md` §4
- **Acceptance:** a drop/re-attach test replays from the last ack; a live executor may complete without the client; a dead or unconfirmed executor settles to a typed uncertain/recovery state; no state is orphaned by the drop.
- **Failure cases:** a client disconnect silently discards Work → violation; replay gap or false claim of continued execution → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-CHAN-014 — Authenticated cross-device Mission access
- **Statement:** GIVEN an online Core endpoint and a configured Work executor, WHEN a user switches between desktop, web and mobile clients, THEN each authenticated surface reads the same scoped Mission/Work state, can steer or answer an eligible wait, and receives a completion/needs-input notification without creating a second session truth.
- **Priority:** must
- **Source:** DEC-055 · `ARCH/32-CHANNELS.md` §2/§7 · `ARCH/19-RUNTIME-ENVIRONMENTS.md` §Remote/cloud execution handoff
- **Acceptance:** device switch preserves draft/attachment scope and task identity; a remote steer reaches current Work owner; denied/revoked device cannot read or approve; notification links to the exact item.
- **Failure cases:** offline Core or no executor → honest unavailable/paused state; stale/replayed approval → rejected; native-agent private state never copied into a client projection.
- **Tests:** pending
- **Status:** accepted

### Effect Verification (`VERIFY`)

#### REQ-VERIFY-001 — Verification depth scales with risk; the matrix is a floor
- **Statement:** GIVEN a capability's risk class (`safe`/`sensitive`/`dangerous`), WHEN an effect completes, THEN the declared minimum verification for that class runs, and per-capability descriptor overrides may raise but never lower the floor.
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §1/§3 · `ARCH/06-DATA-MODEL.md` DM-011 · `ARCH/04-DECISIONS.md` DEC-022 · `ARCH/05-INVARIANTS.md` INV-19
- **Acceptance:** depth-matrix test per class; an override below the floor is rejected; the receipt cites the depth used.
- **Failure cases:** a dangerous effect with only `safe`-depth verification → violation; an override lowering the floor → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-VERIFY-002 — Verification is read-only; repairs are new operations
- **Statement:** GIVEN a verification run, WHEN it observes/validates/renders, THEN it never mutates the effect; a repair is a new work item/operation with its own ticket and receipt — never a silent re-execution.
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §1/§6 · `ARCH/29-ARTIFACTS.md` §3 · `ARCH/15-AGENT-PLANE.md` §8
- **Acceptance:** verification makes no write; a repair has a distinct ticket + receipt; no silent re-fire path exists.
- **Failure cases:** verification mutating state → violation; silent retry → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-VERIFY-003 — The pipeline order precedes every receipt
- **Statement:** GIVEN any effect entering verification, WHEN it runs, THEN the pipeline order holds (observe context/ticket · deterministic validation · render where applicable · intended-vs-actual postconditions · reconcile outcome) before any receipt.
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §2 · `ARCH/04-DECISIONS.md` DEC-046
- **Acceptance:** pipeline-order test; a receipt cannot be emitted before reconcile; each stage's evidence refs are recorded.
- **Failure cases:** receipt emitted before verification → violation; a skipped stage without a recorded reason → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-VERIFY-004 — Receipts state what ran, was skipped, and why
- **Statement:** GIVEN a receipt, WHEN it is recorded, THEN it records the verification performed (what passed, what was skipped, why) — including the unavailability of a deep hook.
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §1/§4 · `ARCH/04-DECISIONS.md` DEC-022
- **Acceptance:** receipt field-completeness test; skipped checks carry reasons; a receipt cannot omit its verification section.
- **Failure cases:** a receipt claiming verification never run → violation; an omitted skip reason → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-VERIFY-005 — Verification records are stored once and referenced
- **Statement:** GIVEN a verification result, WHEN it is stored, THEN one verification record exists (`{id · effect_ref · capability/provider · checks[] · render refs · outcome · duration}`) and receipts reference it rather than duplicating it.
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §5 · `ARCH/29-ARTIFACTS.md` §3 · `ARCH/07-CONTRACTS.md` CTR-018
- **Acceptance:** single-record test; receipts contain refs not copies; the record id is stable across reads.
- **Failure cases:** duplicated verification payloads → defect; a receipt without a record ref → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-VERIFY-006 — The hook contract is idempotent, bounded and read-only
- **Statement:** GIVEN a verification hook, WHEN it executes, THEN `(effect record) → verification record` is idempotent, bounded and read-only, with typed failures.
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §4 · `ARCH/10-KERNEL.md` §3
- **Acceptance:** a repeat-run test yields identical records; time/scope limits are enforced; a hook with side effects is rejected.
- **Failure cases:** a hook with side effects → violation; an unbounded hook → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-VERIFY-007 — Missing hooks degrade to the risk default with a recorded gap
- **Statement:** GIVEN a capability without a deep hook, WHEN verification runs, THEN it falls back to the risk-class default (re-read/observe) and the receipt/record notes that the deep hook was unavailable.
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §4 · `ARCH/41-EDGE-CASES.md` EDGE-055/EDGE-160
- **Acceptance:** fallback test; the gap is recorded; sensitive/dangerous classes may require human confirmation instead.
- **Failure cases:** claiming deep verification without a hook → violation; silent degradation without a recorded gap → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-VERIFY-008 — Deterministic validators lead; model/vision is consent-gated
- **Statement:** GIVEN verification depth, WHEN checks are selected, THEN deterministic validators, re-reads, structural checks and diffs lead; model/vision inspection runs only where necessary and consent-gated.
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §1/§3 · `ARCH/04-DECISIONS.md` DEC-015
- **Acceptance:** selection test prefers deterministic checks; vision runs only with consent; token use for vision checks is recorded.
- **Failure cases:** a routine deterministic operation routed through vision → violation; vision without consent → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-VERIFY-009 — Reconciliation outcomes are explicit; failures route
- **Statement:** GIVEN an intended-vs-actual comparison, WHEN it completes, THEN the outcome is recorded `pass | fail | partial`; failures route to repair, escalation or `needs_attention` — never a silent retry; partial outcomes are explicit.
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §2/§6 · `ARCH/41-EDGE-CASES.md` EDGE-162
- **Acceptance:** an outcome is recorded for every run; a mismatch produces a routed outcome; no silent re-execution.
- **Failure cases:** a mismatch auto-overwriting state → violation; a silent retry → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-VERIFY-010 — Verify–repair cycles are bounded
- **Statement:** GIVEN a verify–repair cycle making no progress, WHEN the bound is reached, THEN the system escalates with evidence — never an infinite loop.
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §7 · `ARCH/41-EDGE-CASES.md` EDGE-163
- **Acceptance:** a no-progress test terminates with escalation/`needs_attention`; attempts are counted.
- **Failure cases:** an unbounded cycle → defect; escalation without evidence → defect.
- **Tests:** pending
- **Status:** seeded

#### REQ-VERIFY-011 — Postconditions are declared per operation
- **Statement:** GIVEN an operation, WHEN verification runs, THEN postconditions are declared ("cell B2 = 42", "file exists with hash H", "tab URL = X", "message accepted by provider") and compared; batch atomicity applies where declared.
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §6 · `ARCH/22-OFFICE.md` §3 · `ARCH/26-CODE.md` §7
- **Acceptance:** a postcondition-declaration test per capability family; comparison against actual state; partial batch outcomes recorded explicitly.
- **Failure cases:** verification without declared postconditions → defect; a hidden partial batch → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-VERIFY-012 — Verification runs are observable on the event stream
- **Statement:** GIVEN a verification run, WHEN it starts/completes, THEN it emits `verification.started` / `verification.completed` events so progress is observable.
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §5 · `ARCH/30-EVENTS.md` §3 · `ARCH/05-INVARIANTS.md` INV-23
- **Acceptance:** an event pair is emitted per run; events carry refs not payloads; UI visibility derives from the stream.
- **Failure cases:** verification invisible to the event stream → defect; payload data in the event → violation.
- **Tests:** pending
- **Status:** seeded

#### REQ-VERIFY-013 — Verifier failure degrades honestly
- **Statement:** GIVEN a crash/absent validator or an unavailable vision pass, WHEN verification cannot complete, THEN the effect is marked `unverified` with the gap recorded, and policy may block dangerous classes entirely (human confirmation instead).
- **Priority:** must
- **Source:** `ARCH/34-EFFECT-VERIFICATION.md` §7 · `ARCH/41-EDGE-CASES.md` EDGE-160 · `ARCH/05-INVARIANTS.md` INV-19
- **Acceptance:** a validator-crash test marks the effect unverified; a dangerous class blocks or asks for confirmation; no silent pass.
- **Failure cases:** a crash defaulting to pass → violation; an unverified effect delivered as verified → violation.
- **Tests:** pending
- **Status:** seeded

### DEC-054 amendment — Mission, ecosystem, learning and experience

#### REQ-MISSION-001 — Mission survives worker replacement
- **Statement:** GIVEN a Mission with a failed or replaced agent session, WHEN Work restarts, THEN the current contract, plan, requirements and evidence remain reconstructable without the old transcript.
- **Priority:** must
- **Source:** DEC-054 · INV-25/26
- **Acceptance:** restart from event/checkpoint state with a different adapter retains the same Mission and PlanNode identity.
- **Failure cases:** missing native session → no Mission loss; failed Work → node remains retryable.
- **Tests:** pending
- **Status:** accepted

#### REQ-MISSION-002 — Versioned intent and plan
- **Statement:** GIVEN a contract or plan change, WHEN committed, THEN it creates an immutable version after impact, graph and stale-base validation.
- **Priority:** must
- **Source:** DEC-054 · INV-27
- **Acceptance:** concurrent stale PlanPatch rejects; prior versions remain readable; affected nodes/evidence identified.
- **Failure cases:** cycle, stale base, scope widening or budget violation → typed refusal.
- **Tests:** pending
- **Status:** accepted

#### REQ-MISSION-003 — Semantic node and execution attempt separation
- **Statement:** GIVEN a PlanNode fails an attempt, WHEN retried, THEN a new Work attempt is linked to the unchanged semantic node.
- **Priority:** must
- **Source:** DEC-054 · INV-26
- **Acceptance:** history shows two attempts for one node and no duplicate requirement.
- **Failure cases:** failed attempt marks Mission failed or overwrites prior evidence → reject.
- **Tests:** pending
- **Status:** accepted

#### REQ-MISSION-004 — Evidence-backed completion
- **Statement:** GIVEN Mission completion is proposed, WHEN evaluated, THEN each required current criterion has valid independent evidence or an explicit user-approved exception.
- **Priority:** must
- **Source:** DEC-054 · INV-28/30
- **Acceptance:** missing required evidence yields partial/not_tested and blocks completed status.
- **Failure cases:** worker self-report promoted to verified → reject; stale evidence → invalidate.
- **Tests:** pending
- **Status:** accepted

#### REQ-MISSION-005 — Drift-aware resume
- **Statement:** GIVEN a dormant Mission, WHEN resumed, THEN environment and referenced inputs are compared before further mutation and affected nodes/evidence are reconciled.
- **Priority:** must
- **Source:** DEC-054 · INV-29
- **Acceptance:** changed file/dependency invalidates only its dependent closure and produces a reviewable report.
- **Failure cases:** expired auth, missing profile or changed repo → pause affected branch rather than replay blindly.
- **Tests:** pending
- **Status:** accepted

#### REQ-MISSION-006 — Bounded no-progress recovery
- **Statement:** GIVEN repeated attempts with the same failure and no meaningful state delta, WHEN the bound is reached, THEN the controller changes strategy or stops that branch with a reason.
- **Priority:** must
- **Source:** DEC-054 · INV-31
- **Acceptance:** repeat signature count, budget and escalation visible; no infinite retry.
- **Failure cases:** alive heartbeat without progress → not treated as success.
- **Tests:** pending
- **Status:** accepted

#### REQ-MISSION-007 — Branch-local human wait
- **Statement:** GIVEN one node needs user input, WHEN independent nodes are ready, THEN those nodes continue within budget while the dependent branch waits durably.
- **Priority:** must
- **Source:** DEC-054
- **Acceptance:** approval/question inbox shows blocker and unaffected work advances.
- **Failure cases:** no interactive channel → durable wait, no assumed approval.
- **Tests:** pending
- **Status:** accepted

#### REQ-ECO-001 — Non-invasive installed-agent discovery
- **Statement:** GIVEN an existing local agent, WHEN discovered and attached, THEN native config/extensions are not modified and unsupported capabilities are shown as unknown/unavailable.
- **Priority:** must
- **Source:** DEC-054 · INV-32
- **Acceptance:** config hashes unchanged after discovery/attach; probe timeouts do not block startup.
- **Failure cases:** probe failure → unverified; unsupported overlay → explicit guidance.
- **Tests:** pending
- **Status:** accepted

#### REQ-ECO-002 — Scoped MCP and extension grants
- **Statement:** GIVEN a catalog MCP/skill/plugin, WHEN a Work starts, THEN only explicitly activated, policy-permitted components for its binding/workspace are exposed.
- **Priority:** must
- **Source:** DEC-054 · INV-32/33
- **Acceptance:** global catalog entry grants zero access by itself; child grants are attenuated; revocation blocks next call.
- **Failure cases:** name collision → namespaced choice; unauthorized server → never starts for that Work.
- **Tests:** pending
- **Status:** accepted

#### REQ-ECO-003 — Actual identity on shared calls
- **Statement:** GIVEN a shared tool call, WHEN admitted, THEN its policy decision and receipt use the real session, binding, Work, grant and trace identity.
- **Priority:** must
- **Source:** DEC-054 · INV-33
- **Acceptance:** concurrent agent sessions receive distinct scopes and audit links.
- **Failure cases:** placeholder identity or missing grant → reject typed.
- **Tests:** pending
- **Status:** accepted

#### REQ-ECO-004 — Honest native/mediated provenance
- **Statement:** GIVEN an external agent uses native or host tools, WHEN progress and evidence are shown, THEN governance class and proof level match the actual path.
- **Priority:** must
- **Source:** DEC-049/054 · INV-34
- **Acceptance:** native reports never display a Core ticket/verified badge; mediated calls cite a ticket and receipt.
- **Failure cases:** unknown path → unknown/unverified, not Core-governed.
- **Tests:** pending
- **Status:** accepted

#### REQ-ECO-005 — Heterogeneous child delegation
- **Statement:** GIVEN a lead requests a child with a compatible adapter, WHEN admitted, THEN child Work has its own context, scope, budget, lifecycle and typed receipt; integration is separately checked.
- **Priority:** must
- **Source:** DEC-054
- **Acceptance:** two different agent implementations complete isolated nodes and parent receives evidence refs without child transcripts.
- **Failure cases:** unsupported capability or isolation → typed refusal, no silent downgrade.
- **Tests:** pending
- **Status:** accepted

#### REQ-ECO-006 — Shared browser/desktop/connector choice
- **Statement:** GIVEN an agent has native and host options, WHEN a task needs an external resource, THEN host options are discoverable but the selected path and owner are visible and no native tool is silently suppressed.
- **Priority:** must
- **Source:** DEC-054
- **Acceptance:** same scenario can select a suitable connector/browser/desktop path under grants; actual path is recorded.
- **Failure cases:** connector unavailable → explicit fallback; login required → user handoff.
- **Tests:** pending
- **Status:** accepted

#### REQ-LEARN-001 — Reviewable workflow-to-skill promotion
- **Statement:** GIVEN a demonstration or successful workflow, WHEN proposed as a skill, THEN deterministic steps, adaptive guidance, permissions, inputs, provenance and version are reviewable before publication.
- **Priority:** must
- **Source:** DEC-054 · INV-35
- **Acceptance:** replay/evaluation evidence and rollback target attached to published version.
- **Failure cases:** secret capture or unscoped side effects → proposal rejected.
- **Tests:** pending
- **Status:** accepted

#### REQ-LEARN-002 — External workflow adapter preserves ownership
- **Statement:** GIVEN an n8n/Activepieces run invoked by AgentCowork, WHEN paused/resumed/completed, THEN one host Work tracks the provider run while trigger and internal node execution retain a single declared owner.
- **Priority:** must
- **Source:** DEC-054
- **Acceptance:** callback replay and resume-before-pause settle once with evidence.
- **Failure cases:** provider unavailable → waiting_dependency; duplicate trigger → deduplicated.
- **Tests:** pending
- **Status:** accepted

#### REQ-UXQ-001 — Progressive Mission Control and truthful controls
- **Statement:** GIVEN a simple request or a substantial Mission, WHEN displayed, THEN simple chat remains simple and Mission controls/evidence appear as complexity grows.
- **Priority:** must
- **Source:** DEC-054
- **Acceptance:** user can inspect/steer/pause/replace/take over relevant Work and distinguish native versus mediated proof.
- **Failure cases:** unavailable control → disabled with reason; notification never masquerades as receipt.
- **Tests:** pending
- **Status:** accepted

#### REQ-UXQ-002 — Measured quality before superiority claims
- **Statement:** GIVEN a release claims comparative task quality, WHEN published, THEN scenario outcomes, latency/cost, recovery, accessibility and competitor version/date are supported by reproducible evidence.
- **Priority:** must
- **Source:** DEC-054
- **Acceptance:** benchmark record includes task inputs, hardware, product versions, human rubric and failures.
- **Failure cases:** no comparable result → no superiority claim.
- **Tests:** pending
- **Status:** accepted

#### REQ-UXQ-003 — Composer controls and durable draft
- **Statement:** GIVEN a conversation, WHEN the user composes or work is active, THEN `+`, `@`, `/`, agent, conditional model, access, voice where available, Send/Stop, Queue/Steer, attachments and draft persistence follow `48` §3.
- **Priority:** must
- **Source:** DEC-055; `48` §3
- **Acceptance:** `TC-005…008` pass with keyboard and crash/navigation restore; unsupported model/access controls explain why.
- **Failure cases:** native command collision, lost draft, invented model choice, silent worker switch.
- **Tests:** pending
- **Status:** accepted

#### REQ-UXQ-004 — Approachable navigation and agent setup
- **Statement:** GIVEN a new user, WHEN opening the app or configuring an agent, THEN primary navigation is outcome-based and agent sign-in/provider/extension choices reflect probed support and custody.
- **Priority:** must
- **Source:** DEC-055; `48` §§2, 7
- **Acceptance:** `TC-007`, `TC-009` and `TC-038` pass; discovery does not block first prompt.
- **Failure cases:** catalog entry shown ready, unsupported API key prompt, jargon required for first task.
- **Tests:** pending
- **Status:** accepted

#### REQ-UXQ-005 — Universal Workbench with honest format support
- **Statement:** GIVEN a file, URL, artifact or agent, WHEN opened beside chat, THEN a stable tab, typed selection and inspect/edit/takeover control are offered only at probed capability, with reader or native fallback otherwise.
- **Priority:** must
- **Source:** DEC-055; `48` §5
- **Acceptance:** `TC-010…018` pass including conflict and unsupported-format paths.
- **Failure cases:** silent file flattening, wrong selection target, browser control without resnapshot.
- **Tests:** pending
- **Status:** accepted

#### REQ-UXQ-006 — Complete and safe answer rendering
- **Statement:** GIVEN text, math, tables, images, citations, diagrams or an interactive artifact, WHEN rendered or streamed, THEN it is responsive, accessible, stable and isolated according to `48` §4.
- **Priority:** must
- **Source:** DEC-055; `48` §4
- **Acceptance:** `TC-003`, `TC-004`, `TC-014`, `TC-038` pass.
- **Failure cases:** raw HTML privileges, broken image layout, math without accessible fallback.
- **Tests:** pending
- **Status:** accepted

#### REQ-UXQ-007 — Library metadata and exact-version retrieval
- **Statement:** GIVEN generated, uploaded, imported or linked material, WHEN filtered or retrieved, THEN origin, version, permissions, index status and provenance remain distinct; citations identify exact source location.
- **Priority:** must
- **Source:** DEC-055; `29` §4; `48` §6
- **Acceptance:** `TC-015`, `TC-033` pass under version change, parser failure and revocation.
- **Failure cases:** stale retrieval presented current, unindexed content invented, revoked item exposed.
- **Tests:** pending
- **Status:** accepted

#### REQ-UXQ-008 — Scoped extensions and truthful native inventory
- **Statement:** GIVEN a discovered or installed agent and host MCP/skill/plugin, WHEN configured, THEN catalog/install/active states and effective agent/session/workspace/Mission scope are visible; native config remains owned by its agent.
- **Priority:** must
- **Source:** DEC-054/055; `46`; `48` §7
- **Acceptance:** extension collision, revoke and unsupported overlay cases pass `TC-007`, `TC-008`, `TC-026`.
- **Failure cases:** global auto-mount, native config mutation, Core receipt claimed for native effect.
- **Tests:** pending
- **Status:** accepted

#### REQ-UXQ-009 — Team visibility and bounded collaboration
- **Statement:** GIVEN host-delegated or native child agents, WHEN work runs, THEN the UI distinguishes their provenance and supports bounded host delegation, independent integration and branch-local blocking.
- **Priority:** must
- **Source:** DEC-054/055; `15`; `35`; `48` §8
- **Acceptance:** `TC-024…026`, `TC-034`, `TC-035` pass.
- **Failure cases:** native child shown as controlled Work, unbounded swarm, completion without integration.
- **Tests:** pending
- **Status:** accepted

#### REQ-UXQ-010 — Workflow and skill conversion is reviewable
- **Statement:** GIVEN a successful repeated procedure or a workflow, WHEN converted, THEN the candidate skill/workflow is versioned, scoped, sanitized and evaluated before publish; running execution keeps its declared owner.
- **Priority:** must
- **Source:** DEC-054/055; `37`; `48` §8
- **Acceptance:** `TC-021…023` pass including secret removal and selector drift.
- **Failure cases:** silent publish, permission widening, second scheduler.
- **Tests:** pending
- **Status:** accepted

#### REQ-UXQ-011 — System insights are understandable and explicitly shared
- **Statement:** GIVEN a user opens the System Workbench or an agent requests machine data, WHEN information is displayed or shared, THEN plain-language health summaries explain freshness and limitations, sensitive data scope is disclosed before access, and opening the panel alone never adds machine data to chat context.
- **Priority:** must
- **Source:** DEC-058; ARCH/48-EXPERIENCE-SURFACES.md §5/§7; ARCH/51-MACHINE-OBSERVABILITY.md §6/§8
- **Acceptance:** new-user tasks identify what is collected, why, retention and agent visibility; advanced query/process controls are discoverable but hidden behind progressive detail; screen-reader and keyboard journeys pass.
- **Failure cases:** technical jargon required for Overview; hidden data collection; dashboard opening silently injects telemetry; consent implied by OS permission; no reason shown for unsupported/denied readings.
- **Tests:** pending
- **Status:** accepted

### Machine Observer (OBS)

#### REQ-OBS-001 — Current machine snapshot is typed and fresh
- **Statement:** GIVEN an authorized local request names supported metric ids, WHEN the Observer returns a snapshot, THEN each returned value includes a stable metric id, unit, source, observed time and freshness status.
- **Priority:** must
- **Source:** DEC-058; ARCH/51-MACHINE-OBSERVABILITY.md §2/§10
- **Acceptance:** typed fixtures verify CPU, memory, volume, process summary and network rates; snapshot age and unavailable states are explicit.
- **Failure cases:** units omitted or confused; stale values presented as current; failed sampling converted to zero; unknown fields accepted as authoritative.
- **Tests:** pending
- **Status:** accepted

#### REQ-OBS-002 — Provider gaps are honest per metric
- **Statement:** GIVEN a host lacks a sensor, driver, vendor API or permission, WHEN one provider fails, THEN only affected metrics report their typed status and reason while supported readings remain available.
- **Priority:** must
- **Source:** DEC-058; ARCH/51-MACHINE-OBSERVABILITY.md §4/§9
- **Acceptance:** mixed supported/unsupported fixtures preserve provenance, provider version, observation time and independent status.
- **Failure cases:** fabricated sensor values; one provider failure hides unrelated readings; unsupported represented as healthy zero.
- **Tests:** pending
- **Status:** accepted

#### REQ-OBS-003 — Machine observation has explicit notice and least-privilege elevation
- **Statement:** GIVEN any person or agent requests Machine Observer data, WHEN sampling or disclosure begins, THEN the user first receives a plain-language in-app notice describing the category, purpose, local recipient, cadence and retention, and explicitly grants revocable category consent; separate sensitive scopes and any agent disclosure are authorized independently, and OS elevation appears only for an exact operation whose provider requires it.
- **Priority:** must
- **Source:** DEC-058; INV-03/36/37; ARCH/06-DATA-MODEL.md DM-043; ARCH/12-TRUST.md; ARCH/19-RUNTIME-ENVIRONMENTS.md §6; ARCH/51-MACHINE-OBSERVABILITY.md §6
- **Acceptance:** basic standard-user metrics work without elevation after in-app consent; Trust persists only the user's revocable local collection choice by category/scope/purpose/retention; the Observer adds no installation-time privilege/service/driver requirement; app/Core remain asInvoker; any installer UAC is attributable only to the chosen installation scope and is never treated as data consent; helper requests are typed, one-shot and expire on exit; declining product consent keeps collection off and produces at most one contextual inline explanation; denying/canceling OS elevation keeps standard readings available, records no OS grant and produces at most one scope-specific follow-up; agent disclosure always needs a separate Work-scoped capability grant.
- **Failure cases:** UAC at startup/global polling; whole-app elevation; collection before in-app consent; installer approval treated as telemetry consent; silent read access to process/network/history metadata; caller-supplied scope treated as authority; helper accepts shell/path/query; repeated prompt/follow-up without new user intent.
- **Tests:** pending
- **Status:** accepted

#### REQ-OBS-004 — Local history is bounded, consented and separate from Core events
- **Statement:** GIVEN history categories and retention are enabled, WHEN samples are collected, THEN the service stores bounded local time-series data, exposes gaps and retention state, and publishes only lifecycle/configuration/health/alert transitions to Core Events.
- **Priority:** must
- **Source:** DEC-058; INV-23/36; ARCH/30-EVENTS.md §5; ARCH/51-MACHINE-OBSERVABILITY.md §5
- **Acceptance:** configured quotas and retention prune/downsample deterministically; high-rate samples do not create event rows; process-history consent is independent and revocable.
- **Failure cases:** unbounded growth; sampling while disabled; samples leak to event log/support bundle; retention cleanup corrupts concurrent reads; collection gap displayed as zero.
- **Tests:** pending
- **Status:** accepted

#### REQ-OBS-005 — Machine queries are typed, read-only and bounded
- **Statement:** GIVEN a user or authorized agent requests OS facts, WHEN the query runs, THEN it resolves through an allowlisted typed plan with time, row and scope limits and returns provenance plus omitted-field/status information.
- **Priority:** must
- **Source:** DEC-058; INV-03/36; ARCH/51-MACHINE-OBSERVABILITY.md §7/§10
- **Acceptance:** supported process, device and connection-metadata query fixtures return only authorized fields; query limits are enforced by service, not just UI. File metadata remains owned by Files/Search.
- **Failure cases:** write/multi-statement/PRAGMA/extension/shell execution; unbounded table scan; query bypasses Trust; file walking duplicated; command lines, secrets or payloads included by default.
- **Tests:** pending
- **Status:** accepted

#### REQ-OBS-006 — Storage health reuses the sole file scanner
- **Statement:** GIVEN storage overview or disk-health information is requested, WHEN the Observer reports volumes/SMART and the user requests a treemap, THEN volume/health observations stay read-only and directory traversal uses the existing Files/Storage scanner.
- **Priority:** must
- **Source:** DEC-058; ARCH/25-FILES.md; ARCH/51-MACHINE-OBSERVABILITY.md §2/§4/§11
- **Acceptance:** volume capacity and per-device SMART support are labeled with source/permissions; treemap progress/cancel/scope come from the existing scanner with no duplicate walker.
- **Failure cases:** unsupported drive shown healthy; disk self-test/repair/write is invoked; overlapping scanner implementation; cancellation ignored or scope widened.
- **Tests:** pending
- **Status:** accepted

#### REQ-OBS-007 — GPU and local-model telemetry preserve source and attribution limits
- **Statement:** GIVEN a supported GPU or configured local inference runtime is observed, WHEN metrics are displayed or shared, THEN device/provider/runtime measurements retain source and interval, per-process attribution is marked approximate when inferred, and prompts/completions are excluded.
- **Priority:** must
- **Source:** DEC-058; ARCH/18-MODEL-ROUTING.md; ARCH/51-MACHINE-OBSERVABILITY.md §4/§5
- **Acceptance:** NVIDIA/AMD/Intel/platform test fixtures expose only supported metrics; runtime token/rate fields are accepted only from a configured metrics interface or run-scoped producer.
- **Failure cases:** GPU load presented as tokens/sec; utilization attributed to wrong process; missing WSL metric fabricated; prompts, completions or credentials recorded.
- **Tests:** pending
- **Status:** accepted

#### REQ-OBS-008 — WSL observation is explicit and never starts a distro
- **Statement:** GIVEN WSL detail is requested, WHEN the user selects an installed distro, THEN only that already-running distro is queried under a visible scope; AgentCowork never starts, installs, or enters another distro to gather telemetry.
- **Priority:** must
- **Source:** DEC-058; ARCH/19-RUNTIME-ENVIRONMENTS.md; ARCH/21-WORLD-MODEL.md; ARCH/51-MACHINE-OBSERVABILITY.md §2/§4
- **Acceptance:** stopped and unselected distributions receive no process/filesystem call; unsupported GPU/query fields are shown as unavailable with provider explanation.
- **Failure cases:** automatic distro launch/install; implicit filesystem traversal; host GPU figures represented as per-distro GPU; distro identity ambiguity.
- **Tests:** pending
- **Status:** accepted

#### REQ-OBS-009 — Observer is independently buildable and integrated through scoped Core IPC
- **Statement:** GIVEN the Observer runs standalone or under AgentCowork, WHEN a client connects, THEN a versioned authenticated local protocol exposes only typed observation capabilities, with Core applying Trust scope and Runtime supervising the service.
- **Priority:** must
- **Source:** DEC-058; CTR-033; INV-36/37/38; ARCH/19-RUNTIME-ENVIRONMENTS.md §6.1; ARCH/51-MACHINE-OBSERVABILITY.md §3/§10
- **Acceptance:** service builds without AgentCowork internal crates/UI; mismatched protocol, unauthorized pipe peer, malformed request and helper/service crash fail closed; no network listener exists by default.
- **Failure cases:** direct agent access to telemetry store; shared secret in caller-controlled args; unbounded messages; hidden network endpoint; stale Core agent/Work grant silently restored after service restart.
- **Tests:** pending
- **Status:** accepted

#### REQ-OBS-010 — Advanced process diagnostics are on-demand and least-rights
- **Statement:** GIVEN the user requests an advanced process/service diagnostic, WHEN the Observer reads it, THEN it uses a separate explicit category grant and an allowlisted read-only field plan, requests only the minimum operating-system access rights, and reports protected or inaccessible targets without bypassing their security descriptors.
- **Priority:** should
- **Source:** DEC-058; INV-03/36/37/38; ARCH/12-TRUST.md §10; ARCH/51-MACHINE-OBSERVABILITY.md §2/§6/§7; [Microsoft process security and access rights](https://learn.microsoft.com/en-us/windows/win32/procthread/process-security-and-access-rights)
- **Acceptance:** opted-in on-demand queries can return supported process owner/image/parent, thread summary, handle count, service state and module metadata; fixtures prove per-field unsupported/access-denied states, no default history, no `PROCESS_ALL_ACCESS`/handle duplication/process-memory reads, and no data leaves the device unless a separate Work-scoped share is approved. Any field needing elevation follows REQ-OBS-003's one-operation UAC flow.
- **Failure cases:** broad access mask; protected-process bypass; command line, environment, open-file paths, memory, stack traces or kernel data collected under this requirement; diagnostic history collected without its own grant; denied field hidden as zero or success.
- **Tests:** pending
- **Status:** accepted

## 5. Seeding status

| Domain | Seeds | Next pass |
|---|---|---|
| `PROD` (6) | drafted above | verified during the P9 verification pass ✅ (2026-09-26) |
| `CTX` (11) | 10 baseline + REQ-CTX-011 (DEC-056 inspector) | baseline verified during pass `16` ✅ (2026-09-26); amendment implementation pending |
| `TRUST` (10), `CAP` (10) | drafted above + expanded in passes `12`/`13` | verified during passes `12` ✅ / `13` ✅ (2026-09-26) |
| `PROV` (10) | drafted above + expanded in pass `14` | verified during pass `14` ✅ (2026-09-26) |
| `AGENT` (4) | drafted 2026-09-27 from the retired `AGX` set (DEC-052) | pending — no engine binding exists yet |
| `UI` (17) | 14 baseline + REQ-UI-015…017 (DEC-056) | baseline verified during the P7 UI merge ✅ (2026-09-26); amendment implementation pending |
| `KERNEL` (7), `WORK` (9) | baseline + REQ-WORK-009 lineage (DEC-056) | baseline verified during passes `10` ✅ / `11` ✅ (2026-09-26); amendment implementation pending |
| `MEM` (27) | drafted above + expanded in pass `17` and the P7 memory merge (`REQ-MEM-013…027`) | verified during pass `17` ✅ / P7 memory merge ✅ (2026-09-26) |
| `MODEL` (12) | drafted above + expanded in pass `18` | verified during pass `18` ✅ (2026-09-26) |
| `RTENV` (12) | 11 baseline + REQ-RTENV-012 remote/cloud ownership (DEC-055) | baseline verified during pass `19` ✅ (2026-09-26); amendment implementation pending |
| `WF` (11) | drafted above + expanded in pass `20` | verified during pass `20` ✅ (2026-09-26) |
| `WORLD` (11) | drafted above + expanded in pass `21` | verified during pass `21` ✅ (2026-09-26) |
| `OFFICE` (11) | drafted above + expanded in pass `22` | verified during pass `22` ✅ (2026-09-26) |
| `BROWSER` (12) | drafted above + expanded in pass `23` | verified during pass `23` ✅ (2026-09-26) |
| `CUA` (12) | drafted above + expanded in pass `24` | verified during pass `24` ✅ (2026-09-26) |
| `FILES` (12) | drafted above + expanded in pass `25` | verified during pass `25` ✅ (2026-09-26) |
| `CODE` (12) | drafted above + expanded in pass `26` | verified during pass `26` ✅ (2026-09-26) |
| `SEARCH` (13) | 12 baseline + REQ-SEARCH-013 exact-version content indexing (DEC-055) | baseline verified during pass `27` ✅ (2026-09-26); amendment implementation pending |
| `COMMS` (13) | drafted above + expanded in pass `28` | verified during pass `28` ✅ (2026-09-26) |
| `ART` (12) | drafted above + expanded in pass `29` | verified during pass `29` ✅ (2026-09-26) |
| `EVENTS` (12) | drafted above + expanded in pass `30` | verified during pass `30` ✅ (2026-09-26) |
| `SKILL` (13) | drafted above + expanded in pass `31` | verified during pass `31` ✅ (2026-09-26) |
| `CHAN` (14) | 13 baseline + REQ-CHAN-014 cross-device access (DEC-055) | baseline verified during pass `32` ✅ (2026-09-26); amendment implementation pending |
| `VERIFY` (13) | drafted above + expanded in pass `34` | verified during pass `34` ✅ (2026-09-26) |
| `UXQ` (11) | 10 baseline + REQ-UXQ-011 System Workbench (DEC-058) | implementation pending |
| `OBS` (10) | new domain for local machine observation and on-demand process diagnostics (DEC-058) | implementation and device qualification pending |

> **P9 verification (2026-09-26, historical):** registry integrity was recorded as 307 entries ↔ 307 matrix rows before later retirements. **DEC-054/055/056/057/058 amendment (2026-09-28):** current registry and matrix each contain 342 unique active `REQ-*` entries/rows across 31 domains (330 prior active + REQ-SEARCH-013 + REQ-UXQ-011 + REQ-OBS-001…010). W0–W6 in `TODO.md` name implementation tasks; all newly associated tests/evidence remain pending. This count is an inventory, not verification of implementation.

## 6. Related

- `AGENTCOWORK-SPEC.md` (WHAT) · `ARCH/03-HLD.md` (HOW) · `ARCH/05-INVARIANTS.md` (INV-*) · `ARCH/04-DECISIONS.md` (DEC-*)
- `ARCH/09-FEATURE-MATRIX.md` (traceability) · `TODO.md` (`TASK-*`) · `ARCH/42-EVIDENCE-MAP.md` (acceptance evidence)
- Process: `.agents/docs/spec-driven-development.md` · Template: `.agents/templates/SPEC.template.md`
