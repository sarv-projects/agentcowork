# 30 — Events

> **DEC-054/058 amendment:** The one append-only store adds `mission.*`, `contract.versioned`, `requirement.*`, `plan.versioned`, `node.*`, `assumption.*`, `decision.*`, `evidence.*`, `outcome.*`, `extension.grant.*` and low-rate `observer.*` consent/configuration/health/alert-transition events. Mission projections rebuild from this store; no second Mission event bus. Event payloads carry refs/redacted metadata, not secrets, consent disclosure bodies, sample series or entire native transcripts (`35`, `36`, `51`).

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P3).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-EVENTS-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** **one event store + one bus**. UI projections, workflow triggers, world updates, low-rate lifecycle/usage telemetry and audit feeds derive from it — no hidden side channels (INV-23). High-rate Machine Observer samples remain in its bounded local telemetry store; only consent, configuration, health and configured alert transitions are published here (DEC-058).
> **Boundary:** `SessionEvent` (DM-007, owned by `11`) is the session-local append-only log; `Event` (DM-008, owned here) is the **published system stream**. Everything material emits ≥1 published event; session logs remain the session’s truth.
> **Dependencies:** `10-KERNEL` · `11-WORK` (producers) · all modules (producers/consumers). **Consumers:** UI (`32`), `20` (triggers), `21` (world updates), `12` (audit feed), telemetry.
> **Evidence:** INV-23 · DEC-027 (log + projections) · DEC-033 (workflow events) · `agent-harness-verification.md` §E3 (typed stream vocabulary) §E4 (log + projections) · `ARCH/15-AGENT-PLANE.md` §4 (typed stream), `ARCH/20-WORKFLOW.md` §4, `ARCH/21-WORLD-MODEL.md` §4.

## 1. Purpose & rules

**Owns:** the append-only event store · the bus (publish/subscribe) · replay · subscription filters · the stream vocabulary · usage/cost telemetry events · observer lifecycle/configuration/health/alert transition events · the external-agent event projection.
**Never owns:** session logs (`11`) · the audit chain (`12` appends its own tamper-evident entries and also consumes events) · payload data (events carry **refs**, not documents).

1. **One store** — every projection (UI Runs, pending work, world state, workflow triggers) is derived; never a second source of truth.
2. **Typed** — namespaced event types with declared payload schemas; no opaque “output chunk” events.
3. **Refs over payloads** — large data lives in artifacts/stores; events carry ids + bounded metadata (no secrets, INV-02).
4. **At-least-once delivery; idempotent consumers** — consumers dedupe by event id; ordering is guaranteed per work/session, not globally.
5. **No high-rate sample mirroring** — observer sample rows are queried through its bounded store and Core capability projection; only meaningful observer state transitions enter this stream. The event store must not become a duplicate time-series database.

## 2. Event model (DM-008)

| Field | Meaning |
|---|---|
| `id` | uuidv7 (dedupe key for consumers) |
| `type` | namespaced dotted (`run.started`, `tool.completed`, `artifact.created`, `memory.item.added`, `world.file.changed`, …) |
| `actor` | `system` · `user` · `agent:<id>` · `workflow:<id>` |
| refs | `work_id?` · `session_id?` · `run_id?` · `artifact_id?` · provider/step refs |
| `payload` | bounded metadata or refs (schemas per type; no credentials/PII) |
| `occurred_at` | epoch ms UTC |

**Producers** append; **consumers** subscribe with filters. Storage is SQLite-class append-only with sequence for range reads; hot delivery is in-memory fan-out.

## 3. Stream vocabulary (typed — the UI’s only progress channel)

| Family | Types (representative) |
|---|---|
| Run/step | `run.started` · `plan.created` · `step.started` · `step.completed` · `run.completed` · `run.failed` |
| Model | `model.started` · `model.delta` · `usage.recorded` |
| Tools | `tool.proposed` · `tool.started` · `tool.progress` · `tool.completed` |
| Subagents | `subagent.spawned` · `subagent.progress` · `subagent.finished` |
| Approvals | `approval.requested` · `approval.granted` · `approval.expired` |
| Context | `context.compacting` · `context.compacted` |
| Verification | `verification.started` · `verification.completed` |
| Artifacts | `artifact.created` · `artifact.updated` · `receipt.recorded` |
| Memory | `memory.item.added` · `memory.item.superseded` · `memory.item.forgotten` · `memory.extraction.run` · `memory.recall.outcome` |
| Workflow | `wf.occurrence.materialized` · `wf.run.claimed` · `wf.node.settled` · `wf.wait.armed` · `wf.lease.reaped` |
| World | `world.file.changed` · `world.tab.navigated` · `world.window.focused` · `world.rescan` |
| Comms | `email.arrived` · `message.received` · `calendar.event.upcoming` |
| Provider | `provider.health.changed` · `provider.epoch.bumped` |
| Channel | `channel.health.changed` (`connecting` · `live` · `reconnecting` · `auth_blocked` · `stale` · `closed`; source/reason/last-confirmed time) · `channel.approval.observed` (Core approval ref or native-reported provenance) |
| Session | `session.forked` (parent session, origin message/checkpoint ref, new session; no Work implied) |
| Machine Observer | `observer.consent.changed` · `observer.config.changed` · `observer.health.changed` · `observer.alert.raised` · `observer.alert.cleared` (scope/config refs, source, timestamp; never a sample value or user-content payload) |

Namespacing rules: `<domain>.<noun>.<verb>`; additive evolution preferred; deprecations are declared with a window. `model.delta` (streaming tokens) is **ephemeral delivery only** — deltas are not persisted as individual events (the settled message is). Subagent events follow DEC-036: `subagent.spawned` is emitted before the first prompt dispatch, and `subagent.finished` carries status · error · tool calls · turns · duration · tokens · output · `will_wake`.

## 4. Bus, subscriptions, replay

- **Publish:** single API with backpressure; slow consumers get lag markers, never unbounded queues.
- **Subscribe:** filters by type/refs/scope + auth via `12`; delivery at-least-once; consumers idempotent by id.
- **Replay:** `read(range)` reconstructs projections (Runs, pending work, world state) after restarts; projections must be rebuildable from the store (+ checkpoints).
- **Retention:** durable events pruned by age policy per family; **receipts/audit are separate stores and not pruned with events** (`29`/`12`).
- **Poison events:** quarantine + reconciliation entry; never block the stream.

## 5. Telemetry (usage & cost — where budgets live)

Usage events (`usage.recorded`) carry: model tokens in/out · estimated cost · latency · provider/model id · work/session refs. Aggregations power UI analytics and per-work budget checks (`11` §5). **No prompt or completion content** — counts and refs only.

## 6. External-agent projection (DEC-009)

External agents receive a **filtered stream** for their own work only: session/run/tool/artifact/approval/context events with sensitivity filtering (`12`); never the internal bus, never other agents’ events. The exposed vocabulary is a declared **stable subset** of §3.

## 7. Failure modes

| Failure | Behavior |
|---|---|
| Store growth | Retention prunes by age policy; projections/critical evidence live elsewhere (receipts/audit). |
| Event storm | Bounded subscriber queues + lag markers; producers backpressure; memory never grows unbounded (EDGE-076). |
| Consumer lag | Lag marker event; pull-based catch-up from the store. |
| Duplicate delivery | Consumers dedupe by event id (idempotent by contract). |
| Bus restart | Subscribers re-attach + replay from last ack. |
| Poison event | Quarantine + reconciliation; stream continues. |
| Retention gap | Critical evidence lives in receipts/audit (not pruned with events); projections rebuild from store + checkpoints; a real gap is reported, never silently answered (EDGE-106). |
| Projection drift | Rebuild projections from store (they are derivable — INV-23). |

## 8. Interop

**Depends on:** `10` · storage.
**Exposes to:** UI (`32`), `20` (triggers), `21` (world), `12` (audit feed), analytics, external agents (via `32`) — through `CTR-019` (`EventBus`/`EventStore`; `07` §1).
**DAG check:** the event store never calls into producers; it records what they publish.

## 9. Not in v1

Distributed log/federation · cross-device streaming · external schema registry service (schemas documented in `06`/module docs) · persisted token-level deltas.

## 10. Open questions (`OQ-EVT-*`)

1. Retention windows per family (proposal: operational events 30–90 d; telemetry aggregates longer).
2. Sequence strategy (global vs per-partition) for range reads.
3. Subscription limits/quotas for external agents.
4. Frozen subset vocabulary for the external-agent projection.
5. Whether `model.delta` needs a coalesced persistence mode for replay UX (proposal: no).

## 11. Evidence

INV-23 (one log) · DEC-027 (log + projections) · DEC-033 (workflow event set) · `agent-harness-verification.md` §E3 (typed stream union as wire vocabulary), §E4 (log + projections pattern) · `ARCH/15-AGENT-PLANE.md` §4 · `ARCH/20-WORKFLOW.md` §4 · `ARCH/21-WORLD-MODEL.md` §4 · `ARCH/17-MEMORY.md` §4 (memory events) · `ARCH/29-ARTIFACTS.md` §3 (receipt emission).

## 12. Requirements (`REQ-EVENTS-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-EVENTS-001` | One event store + one bus; every projection derived — no hidden side channels (INV-23, DEC-027) |
| `REQ-EVENTS-002` | Namespaced typed vocabulary with declared payload schemas; no opaque output-chunk events |
| `REQ-EVENTS-003` | Refs over payloads: bounded metadata, no credentials/PII (INV-02) |
| `REQ-EVENTS-004` | At-least-once delivery; consumers dedupe by event id; per-work/session ordering only |
| `REQ-EVENTS-005` | Publish backpressure; bounded queues + lag markers under storm — memory never unbounded (EDGE-076) |
| `REQ-EVENTS-006` | Subscriptions are filtered by type/refs/scope and authorized; no out-of-scope delivery |
| `REQ-EVENTS-007` | Replay (`read(range)`) rebuilds projections after restart from store + checkpoints |
| `REQ-EVENTS-008` | Retention prunes events per family; receipts/audit are separate stores; a real gap is reported (EDGE-106) |
| `REQ-EVENTS-009` | Poison events quarantine + reconcile; the stream continues |
| `REQ-EVENTS-010` | Usage/cost telemetry is counts + refs only — no prompt/completion content; powers budget checks (`11` §5) |
| `REQ-EVENTS-011` | External agents get a filtered, own-work-only stable subset projection (DEC-009, INV-11) |
| `REQ-EVENTS-012` | `model.delta` is ephemeral delivery only; settled messages are what persist |
