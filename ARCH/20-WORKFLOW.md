# 20 — Workflow Engine

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P2).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-WF-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** Core infrastructure, a **peer of the agent runtime** (DEC-008). Deterministic processes that can include agent nodes; agents author workflows and invoke them as tools (P-11).
> **Dependencies:** `11-WORK` (lifecycle/checkpoints/scheduler) · `13`/`14` (action nodes) · `15` (agent nodes) · `12` (approvals/tickets) · `30-EVENTS` · `21-WORLD-MODEL` (trigger sources).
> **Evidence:** `ARCHIVE/v1-research/workflow-engine-verification.md` (690 lines; Temporal/n8n/Copilot Studio verified high, Agent Builder deprecation verified with one PARTIAL) · `agent-harness-verification.md` §E8 (Grok scheduler) · DEC-008/021/033 · INV-16/23.

## 1. Purpose & rules

**Owns:** the workflow IR + registry + version store · `WorkflowRun` state machine · the wake/scheduler loop · occurrence journaling · trigger resolution · approvals integration · run-level receipts.
**Never owns:** agent reasoning (`15`) · capability execution (`13`/`14`) · the Work lifecycle primitives (`11` — workflow runs *materialize as* Work) · UI rendering.

1. **Deterministic ≠ adaptive** — workflows execute known processes; agents figure out unknown ones; both compose in both directions.
2. **Durability is a state-machine problem, not a sleep problem** (verified): every wait/every occurrence exists as a persisted row *before* it is due.
3. **In-flight runs keep their pinned version**; new triggers resolve the then-current published version (INV-16).
4. **Never fabricate completion** — unsettled keyless side effects go to `needs_attention` for a human decision (DEC-022 discipline, mechanized).

## 2. Definition IR (DM-021)

| Node kind | Meaning |
|---|---|
| `action` | capability invocation (`capability_id` + input) |
| `agent` | any `AgentEngine` node: task + `context_refs` |
| `workflow` | sub-workflow call (nested run, parent-close policy) |
| `if` · `switch` · `for_each` · `while` · `parallel` · `join` | control flow |
| `approval` | human decision (`approve · reject · edit · provide-data`) |
| `wait` · `delay` · `timeout` | durable timers |

Edges are typed; definitions carry `variables`, `secrets[]` (vault refs only), `retry/timeout/concurrency` policies, and `outputs`. **The typed IR is the truth** — the visual graph (later) is one authoring surface among JSON/YAML, SDK, and agent authorship (the OpenAI Agent Builder lesson: keep the typed representation, don't depend on the product).

**Publish model:** draft vs published; only **published** versions trigger or execute; versions are content-addressed (definition digest + input/output schema digests).

## 3. Run state machine (DM-022)

`queued → running → waiting | paused | awaiting_approval | retrying → completed | failed | cancelled | expired` — plus `cancel_requested` as a flag on running rows (cooperative cancellation, grace period, force-kill explicit and audited).

Run fields: pinned `workflow_version` + digest · occurrence/trigger ref · inputs ref · attempt count · lease owner/expiry/heartbeat · step ledger ref · approvals · artifacts · errors · `retry_state`. **Workflow state ≠ agent context** (workflow state is deterministic; agent nodes assemble their own context, `16`).

## 4. Durability model (DEC-033 — verified synthesis)

**Persisted state (the minimum set, convergent across n8n/Temporal/OpenWork/Grok/DeepSeek):**

| # | State |
|---|---|
| 1 | Definition + immutable version + **digest** |
| 2 | Trigger row: kind · timezone · enabled · `next_due_at` · **misfire policy** |
| 3 | **Occurrence row** (materialized ahead): id · due time · idempotency key · claim/lease |
| 4 | Run row: pinned version · status · lease/heartbeat · `cancel_requested` |
| 5 | Step attempt rows: node · attempt · status · **idempotency key** · timestamps · result ref |
| 6 | Effect intent / receipt: what was about to happen · ticket/capability ref · outcome |
| 7 | **Wait row**: `wake_at` · kind (`timer/approval/signal`) · cancelled |
| 8 | Approval row: request · payload · decider · decision · edits |
| 9 | Append-only event log (audit/UI/replay — one store, INV-23) |

**Storage:** SQLite (WAL, single writer) with rows + transactions — not whole-file JSON snapshots (volume + partial-write requirements).

**The wake loop (single-writer actor):**

```
1. reconcile(): expired leases → requeue step/run (lease_reaped event);
                cancel_requested → cancel at next step boundary
2. materialize(): enabled schedules with next_due_at ≤ now+window → insert occurrence rows
                  (idempotency key = workflow+occurrence time; unique index ⇒ exactly once)
3. claim(): due occurrence → run pinned to the version resolved at claim time, in one txn
4. execute step-by-step: txn{step started + ticket req} → commit → capability via Guard →
                        txn{step settled + next step/wait rows + event}
5. compute nearest wake (occurrence due · wait wake_at · approval deadline · lease expiry); sleep
```

**Resume matrix (crash between steps):**

| Persisted state | Action |
|---|---|
| `pending` | Execute |
| `settled` | Reuse result; continue |
| `started`, read-only/idempotent | Retry with the **same** idempotency key |
| `started`, side-effecting, retryable | Retry with the same key; provider must dedupe |
| `started`, side-effecting, **keyless** | **`needs_attention`** — human decides (repair/retry/skip); never a blind re-run |
| `failed`, attempts exhausted | Terminal + repair path |

**Idempotency key shape:** `wf:<workflowId>:<versionDigest>:occ:<occurrenceId>:node:<nodeId>:a<attempt>`.

**Misfire policy (desktop):** default **Skip + record** (visible missed row); optional *Run latest missed* (never the whole backlog); grace bounded (≤ 24 h desktop policy).

**Sleep & clocks:** persisted `wake_at` is “not before” (never wall-clock precision); every boot and wake re-checks persisted times; calendar schedules resolve in the stored IANA zone; an OS-level nudge for a *closed* app is a product decision (OQ-WF-04), not a v1 primitive.

**Deliberately not built:** server timer queues · multi-worker distribution · unlimited retries · history compaction / continue-as-new (retention + terminal pruning suffice) · Temporal Nexus/cross-namespace and child-workflow `ABANDON` bookkeeping (the declared parent-close policy itself is kept, §2/§6).

## 5. Triggers (verified taxonomy — real vs product-invention)

| Trigger | Status |
|---|---|
| **Manual** | REAL — universal |
| **Schedule** (interval/calendar; full cron only Temporal/n8n) | REAL — we adopt cron-capable schedule + IANA timezone |
| **Agent-call / workflow-as-tool** · **sub-workflow call/return** · **workflow-failure hook** | REAL — invocation surfaces |
| Webhook | REAL as an *ingress pattern* (HTTP handler → signal); not a runtime primitive |
| File · email · calendar · git | REAL only via connectors (n8n evidence); v1 = **event-store-fed** triggers |
| **Browser events** · **generic agent-event bus** · **completion-starts-another-workflow** | **NOT EVIDENCED** — if shipped, declared as our own design (World Model/CDP sourcing), never “ported” |

**v1 defensible set:** manual · schedule · agent-call · sub-workflow call · workflow failure · Core/World-Model events.

## 6. Versioning

Pinned per run (Temporal Pinned / n8n published snapshot / OpenWork `revisionId` — all agree). New triggers pick the current published version at claim time. Explicit run-upgrade is exceptional and audited. Nested runs inherit the parent’s pinned version per declared parent-close/inheritance rules (Temporal discipline; cron-specifics skipped). Draft ≠ published; an authoring review gate can reuse the approval primitive (later).

## 7. Approvals (DEC-021) — with the verified carve-out

- **approve / reject:** proven (Copilot Studio stages). v1: single-approver default; quorum later (OQ-WF-02).
- **provide-data:** proven (typed RFI payloads) — reuse the approval primitive with a typed form payload.
- **edit:** **design gap** — no surveyed product implements it. Design: an approval carrying an **editable draft payload** while the original stays immutable; both recorded in the receipt.
- **timeout/escalation:** explicit field; default timeout → reject/escalate.
- **routing:** condition edges at the IR level, not approval-specific config.

## 8. Retry / timeout / concurrency defaults (desktop-conservative)

| Knob | Default (evidence-backed) |
|---|---|
| Node retry | **2** attempts by default, opt-in per node (OpenWork 2; n8n 5 — we take the conservative end); exponential backoff |
| Step timeout | 5 min default for tool/binary nodes (Codex background-terminal default), overridable; workflow `maximumRuntime` bounded |
| Lease/reaper | lease **60 s**, reaper **30 s** (n8n durable scheduler) |
| Concurrency | global via `11` scheduler; per-workflow **Overlap Policy default Skip** (Temporal semantics the industry shares), queue option for production runs |
| Trigger storms | backpressure via queue depth (`11`), never unbounded fan-out |

## 9. Composition

- **workflow → agent node:** node passes task + bounded context refs; agent returns a result/receipt; the run never sees the agent's transcript.
- **agent → workflow:** the workflow registry is exposed through the capability catalog (`generate_weekly_report()`, `prepare_release(version)`) — the LLM reasons, the workflow executes deterministically.
- **Agent authors workflows:** the bound engine emits a `WorkflowDefinition` → validation (IR + policy + capability census) → “Save as Workflow” → publish gate. **The engine owns the journal, not the authoring script** (DeepSeek’s un-journaled scripts are the falsifier: they cannot resume).

## 10. Observability

Run receipts + per-node receipts + typed events; Runs UI surface (`32`); audit via `12`. A run’s evidence is replayable (evidence replay, `29` §3).

## 11. Failure modes

| Failure | Behavior |
|---|---|
| Missed wake (OS sleep/reboot) | Persisted `wake_at` + misfire policy (skip+record / latest) |
| Crash mid-node | Resume matrix (§4) |
| Keyless side effect interrupted | `needs_attention` — never fabricated completion |
| Approval expired | Per-class policy (default reject/escalate) |
| Version upgrade mid-flight | Impossible — runs are pinned |
| Trigger storm | Concurrency policy + backpressure |
| Clock change / DST | IANA zone resolution; “not before” semantics |

## 12. Interop

**Depends on:** `10` kernel · `11` (Work lifecycle/checkpoints/scheduler) · `13`/`14` (action nodes) · `15` (agent nodes) · `12` (approvals/tickets) · `21` (trigger sources) · `30` (events).
**Exposes to:** `15` (workflows-as-tools), `32`/UI (runs/approvals projections), `40` FLOW-04/05/18.
**DAG check:** the engine claims occurrences and dispatches capabilities/agents; it never executes effects itself and never owns agent reasoning.

## 13. Not in v1

Visual DAG editor (typed IR + JSON/YAML + agent authoring first; graph later) · cloud/distributed scheduling · quorum approvals · compensation transactions beyond declared policy · cron-specific version inheritance.

## 14. Open questions (`OQ-WF-*`)

1. Misfire default per trigger class (proposal: skip+record).
2. Single-approver vs quorum for v1 (proposal: single).
3. Compensation/rollback scope (define explicit nodes vs defer entirely).
4. OS-level nudge for closed-app schedules (product decision; Windows Task Scheduler-class).
5. Visual editor timing (post-v1).
6. Local webhook ingress surface + its security model (`12`).

## 15. Evidence

`ARCHIVE/v1-research/workflow-engine-verification.md` — §0 (design inputs), §1 (claims A–D verdicts + corrections), §2 (clone evidence: Grok `occurrence_journal.rs:1-12`/`types.rs:7-40`, OpenWork `types/src/automations.ts:346-371`, Open Cowork, NextCoWork, DeepSeek README:128 falsifier, Codex background), §3 (trigger taxonomy), §4 (durability design + resume matrix), §5 (versioning/approvals/retry defaults) · `agent-harness-verification.md` §E8 · DEC-008/021/033 · INV-16/23.

## 16. Requirements (`REQ-WF-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-WF-001` | In-flight runs execute their pinned version + digest; resume uses it; only an explicit audited upgrade changes it (INV-16) |
| `REQ-WF-002` | Typed IR (DM-021) is the definition truth; only published, content-addressed versions trigger or execute |
| `REQ-WF-003` | Occurrence rows persist before due with unique idempotency keys; claims admit exactly one run (DEC-033) |
| `REQ-WF-004` | One wake loop: reconcile leases → materialize → claim → execute step-by-step → nearest wake; no second scheduler (CTR-016) |
| `REQ-WF-005` | Crash resume follows the persisted step matrix; keyless side effects go to `needs_attention`; completion is never fabricated |
| `REQ-WF-006` | `wake_at` is "not before" and re-checked at boot/wake; calendar schedules resolve in stored IANA zones (DST-safe) |
| `REQ-WF-007` | Missed occurrences: default Skip + record, optional "latest missed" only, grace ≤ 24 h |
| `REQ-WF-008` | Approval nodes use the one approval primitive (`approve`/`reject`/`edit`/`provide-data`), timeout per class (DEC-021, INV-17) |
| `REQ-WF-009` | Node retry 2 · step timeout 5 min · lease 60 s/reaper 30 s · overlap Skip default · storm backpressure (DEC-031, DEC-033) |
| `REQ-WF-010` | Agent nodes return receipts (never transcripts); workflows-as-tools resolve via the catalog; authored definitions pass validation + publish gate |
| `REQ-WF-011` | Run/node receipts + typed events in the one event store; terminal reasons recorded; evidence replayable (INV-07/23) |
