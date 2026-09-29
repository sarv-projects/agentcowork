# 11 — Work

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P2).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-WORK-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** the **universal host execution abstraction** (DEC-003). Every execution scheduled by Core — a chat turn, a workflow run, a background job, a host-assigned subagent task, an automation — is a `Work` item with one lifecycle, one scheduler and one Runs surface. Native agent-internal actions and subagents remain under that agent and are reported/observed only when exposed (DEC-054).
> **Dependencies:** `10-KERNEL` · `12-TRUST` (tickets for effects) · `16-CONTEXT` (checkpoints) · `30-EVENTS` (stream). **Consumers:** `15-AGENT-PLANE` · `20-WORKFLOW` · `32-CHANNELS` · UI.
> **Evidence:** product-owner brief (lanes, limits, background work, “Work is universal”) · `ARCHIVE/v1-research/agent-harness-verification.md` §A3 (background guidance), §D1/§E4 (durable log + projections, `next-turn`/`next-step`), §C2 (durable history) · `ARCH/06-DATA-MODEL.md` (DM-001…008) · DEC-003 / DEC-027 / INV-16 / INV-23.

## 1. Purpose & responsibilities

**Owns:** `Work` · `Step` · `Session` · `Run` · `SessionEvent` records (DM-001…005, 007; `Task` DM-003 is the projection in §2) · `Checkpoint` storage (DM-006 — produce/rebuild semantics owned by `16`, `CTR-005`) · the scheduler (lanes, limits, priorities) · durability (checkpoints, resume, cancellation) · budgets (tokens/cost/time) · the Runs projection for the UI (`32`).
**Never owns:** reasoning (`15`) · execution (`13`/`14`) · workflow control-flow semantics (`20` — workflow runs *appear as* work, but the IR and node execution belong to 20) · UI rendering.

Rules:
1. **One lifecycle for host work** — no side Core schedulers or second Core job system (DEC-003, INV-06). A native agent's own child processes are not fabricated as host Work.
2. **The session log is append-only; every view is a projection** — UI history, prompt history, pending-work (inbox), runs list and active-work/Live Desk views (DEC-027, DEC-065, INV-23). Existing Work events may carry optional bounded `ActivityDescriptor` metadata (`06`, `30`) when the execution owner can support a truthful semantic summary; missing metadata stays unknown/generic.
3. **Work state is durable** — crash/restart first reconciles the executor and effects; it resumes a compatible live session or creates a bounded new attempt where safe. No unsupported external-agent resume is claimed. Cancellation is recorded, not implied (INV-16, DEC-054).
4. **Budgets are maxima** — the scheduler enforces outer bounds; agents decide within them (DEC-029/031).

## 2. Entity model (detail for DM-001…008)

**`Work` (DM-001)** — `kind`: `session_turn` · `job` · `workflow_run` · `subagent_task` · `automation`; `status`: `queued → running → waiting | paused | awaiting_approval → completed | failed | cancelled | expired`; plus `parent_work_id` (work trees), nullable `mission_id`/`plan_node_id`/`attempt_number` (semantic correlation), `session_id`, `agent_id`, `workspace_id`, `objective`, `priority`, `completion_contract_ref`, `budget {tokens, cost, time}`, `checkpoint_ref`, timestamps.

**`Step` (DM-002)** — one unit of progress inside a run: `pending → active → done | failed | skipped`; inputs/output refs; tool-call refs; timestamps. Steps are checkpoint boundaries.

**`Task` (DM-003)** — **decision recorded here (OQ-DM-01 resolved for v1):** `Task` is a **projection** over `Work` + `Step` + assignment metadata, not a separate durable entity. Rationale: avoids a second hierarchy beside work/step; delegation already models “task” as the unit passed to workers (`15` §7). Revisit only with evidence (e.g. cross-work task graphs).

**`Session` (DM-004)** — durable container: `active → hibernated → archived`; `agent_binding`, `workspace_id`, `title`, `parent_session_id?`, `fork_origin_ref?`, `log_range` (SessionEvent span), `retention_class`, `last_active`.

**`Run` (DM-005)** — one concrete execution of an agent (or workflow node): `session_id`, `work_id`, `agent_id`, `model`, `reasoning_level`, `usage`, `receipt_refs[]`.

**`Checkpoint` (DM-006)** — `kind`: `work` · `context` · `workflow` · `session`; `content_ref`; `reconstructable` (deterministically produced vs model-written); versioned. Produced at step boundaries, before waits/approvals, and before compaction.

**`SessionEvent` (DM-007)** — append-only log entry; `seq` monotonic per session. **Projections:** `ui-history`, `prompt-history`, `inbox` (`next-turn` | `next-step`), `runs`. The published `Event` (DM-008, `30`) is a *separate* stream derived from the same facts.

## 3. Scheduler

**Lanes:**

| Lane | What runs | Concurrency rule |
|---|---|---|
| Foreground | The active interactive turn | 1 per session (user-visible) |
| Background | Jobs/workers admitted without blocking the UI | Global bound; per-tree bound |
| Detached | Long work whose record outlives the app UI (workflow runs, scheduled automations) | A separate healthy local service or accepted remote executor is required to **continue executing** after app close; otherwise pause and rehydrate on restart (`19` §7) |

**Admission:** work is created with `kind` + priority + budget + completion contract; the scheduler admits under the limits and records the decision (event).

**Outer limits enforced here** (agents decide within them): max simultaneous agents · max total workers per work tree · max depth · max worker tokens · max session spend · per-lane concurrency.

**Policies:** interactive > background priority; starvation guard; queue-depth backpressure; cancellation propagates parent→child; a paused tree releases concurrency slots.

**Durability:** the queue is a **projection over the log** — after a crash the scheduler rebuilds pending/running state from events + checkpoints rather than trusting an in-memory queue.

## 4. Durability & resume

- **Checkpoint cadence:** every step boundary (cheap, incremental); before compaction (`16`); before waits/approvals; before handing off to a worker.
- **Resume semantics on crash/restart:** `running` → reconcile executor/effect liveness, then resume only if the adapter supports it or requeue/new-attempt when safe; an unknown keyless effect enters `needs_attention`. `waiting`/`awaiting_approval` remain; `cancelled` stays cancelled.
- **Resume target check:** resume re-resolves workspace identity before any write (`25` §5); an unresolvable root yields a typed `NotFound` + re-point guidance — queued work is never replayed against a guessed path (EDGE-009).
- **Side-effect safety:** tickets + idempotency keys (`07` §5) make retries safe where providers support dedupe; where they do not, the step is marked *interrupted* and verification (`34`) runs before any retry — a keyless effect that cannot be verified lands in `needs_attention`, never a blind re-fire (EDGE-017).
- **Log + projections:** no mutable session state is authoritative — every view folds the log (harness §D1/§E4 pattern).

## 5. Budgets & accounting

- Per-work budget: tokens · cost · wall-time; aggregated per work tree (background memory extraction is metered under its own declared budget and kill switches — DEC-044).
- Hard ceilings enforced by the scheduler; soft thresholds emit warning events (UI surfacing).
- Usage flows to `30` (telemetry) and summarizes into receipts (`29`) and runs.
- Background work never silently exceeds session budget — exceeding **pauses and surfaces** (kill only by explicit policy).

## 6. Cancellation & interruption

- Cooperative and bounded; propagates parent→child; always records state + reason.
- Three verbs (aligned with `15`): **interrupt** (stop current step, keep session) · **cancel** (terminate work) · **dispose** (release environment/resources).
- Cleanup delegates to `19-RUNTIME-ENVIRONMENTS`; partial effects are receipted/verified, never hidden.

## 7. Sessions & hibernation

- **Retention classes** (provisional naming): `interactive` (long) · `job` (medium) · `ephemeral` (short).
- Hibernation folds volatile state into the log + a checkpoint; archives keep the log + artifact refs (no content duplication).
- Session ownership: Core owns records; agents read/write via CTR-002/004 (never direct store access).
- A user fork creates a new Session with `parent_session_id`, origin message/checkpoint reference and `session.forked` event (DEC-056). It remains navigable even before a turn is run. Work is created only for actual execution; a missing or out-of-scope parent remains a typed unavailable lineage reference, not a fabricated Work.

## 8. Runs projection & observability

- **Runs** = work-tree view (main agent + workers + statuses + budget) built from events, consumed by the UI (`32`).
- Failures keep receipts + typed errors + blockers; every terminal state has a reason.
- Typed streaming (run/step/tool/subagent/approval/context/artifact events) is the UI’s only progress channel.

## 9. Failure modes

| Failure | Behavior |
|---|---|
| Crash mid-step | Resume/requeue per idempotency; interrupted effects verified before retry. |
| Queue inconsistency | Rebuild from log + checkpoints (queue is not truth). |
| Hung run | Scheduler watchdog + stuck detector (`15`) → interrupt/cancel with reason. |
| Budget exhaustion | Pause + surface; no silent overrun. |
| Approval never answered | Expiry policy per `12` (approval records have TTLs). |
| Worker tree explosion | Depth/parallel/total limits enforced at admission; violations rejected, not trimmed silently. |

## 10. Interop

**Depends on:** `10` kernel · `12` trust (tickets/approvals) · `16` context (checkpoints/compaction) · `30` events (stream).
**Exposes to:** `15` (AgentEngine sessions/runs) · `20` (workflow runs as work) · `32` (projections) · UI (Runs).
**DAG check:** Work never calls agents or workflows into existence on its own; admission comes from `15`/`20`/UI/automations.

## 11. Open questions (`OQ-WORK-*`)

1. Detached-lane semantics across app close (platform support; ties `19` and helper/service decisions).
2. Queue/state store: same SQLite instance as sessions or a dedicated store (with `19`/`30`).
3. Hibernation triggers + TTLs per retention class.
4. Fairness policy details under sustained background load.
5. **Resolved by DEC-054:** cross-work semantic graphs use `PlanNode` (DM-038); the Work/Step `Task` projection remains a view, not a second durable task table.

## 12. Evidence

Product-owner brief (universal Work; lanes; global limits; background work) · `agent-harness-verification.md` §A3 (background guidance, bounded delegation), §D1 (inbox `next-turn`/`next-step`, handle/factory), §E4 (log + projections), §C2 (durable history over DB rows) · `ARCH/04-DECISIONS.md` DEC-003/DEC-027 · `ARCH/06-DATA-MODEL.md` DM-001…008 · `ARCH/15-AGENT-PLANE.md` §3–§4 (session model, loop).

## 13. Requirements (`REQ-WORK-*`)

> **DEC-054 amendment:** A Work item may carry `mission_id`, `plan_node_id` and `attempt_number` as nullable foreign references. These are correlation and budget references, not Work ownership of Mission semantics. A failed Work leaves the PlanNode intact. The existing scheduler remains the only execution admission/wake owner; Mission computes semantic readiness and submits Work through it. A checkpoint records context baseline and environment reference for Mission reconciliation (`35`, `36`).

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-WORK-001` | One lifecycle, one scheduler — every execution kind is Work; no second job system (DEC-003). |
| `REQ-WORK-002` | Append-only log, projections only — no mutable session state is authoritative (DEC-027). |
| `REQ-WORK-003` | Durable work and resume — status-correct recovery; cancellation recorded, not implied (INV-16). |
| `REQ-WORK-004` | Scheduler lanes and enforced outer bounds — admission rejects over-limit work, never trims silently. |
| `REQ-WORK-005` | Budgets are maxima — soft warning, hard pause + surface, no silent overrun (DEC-031). |
| `REQ-WORK-006` | Cancellation semantics — interrupt / cancel / dispose, cooperative, parent→child, reason recorded. |
| `REQ-WORK-007` | Checkpoint cadence and side-effect safety — step boundaries; before waits/compaction/handoff. |
| `REQ-WORK-008` | Runs projection — work-tree view from typed events; every terminal state has a reason. |
| `REQ-WORK-009` | Forking a session preserves parent/child lineage; a new Work is created only when the fork executes. |
