# 07 — Contracts (canonical cross-module interfaces)

> **Status:** Frozen v1 baseline (2026-09-26), amended by DEC-054/055/056/057/058/059/060/061/062/063 (2026-09-29) — the interface registry. Every named contract that crosses a module boundary lives here. **Owner** = the module that implements/stabilizes it; **consumers** = modules that call it. Module docs carry serialization/transport detail; this doc owns names, semantic signatures, and guarantees. `Draft`/`Provisional`/`Proposed` describe design maturity, not shipped implementation; implementation evidence lives in `09`/`42`. Queued chat payloads are binding-independent and resolve the latest valid selection at dequeue under DEC-062. Universal governance and receipt rules apply to Core-mediated calls; native agent actions have separate provenance (DEC-063).
> **Rules:** signatures are transport-free (adapters map transports); each Core contract takes an `actor` context (user / agent / workflow). Effect-bearing Core contracts are subject to Trust; the external agent's native operations are outside Core contracts (DEC-054).
> **SDD:** this registry carries the L3 interface layer for behaviors in `ARCH/08-REQUIREMENTS.md`; REQ ↔ CTR links accrue in `ARCH/09-FEATURE-MATRIX.md`.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).

## 0. Conventions

- **IDs:** `CTR-###`, stable.
- **Async by default**; every long-running call accepts a cancellation handle and declares a default timeout + retryability.
- **Typed errors** only: `AuthorizationDenied · NotFound · Conflict · Unavailable · Unsupported · Timeout · InvalidState · Internal`. `guidance`/`requires_user_action` are **results**, not errors (a capability may answer “connect Google Drive first”).
- **Effects:** any Core contract that can cause an externally visible effect MUST require a `Ticket` (INV-03) and return/append a `Receipt` ref (INV-07). Native agent tools are outside this contract and produce separately labelled evidence.
- **No store exposure:** contracts return projections, handles and refs — never internal stores, vault values, or other modules' mutable state (INV-11).
- **Versioning:** breaking signature changes require a `DEC`; additive changes are minor.

## 1. Registry

| CTR | Contract | Owner | Primary consumers | Status |
|---|---|---|---|---|
| CTR-001 | `AgentEngine` | 15 | 11, 20, 32 | Draft (15) |
| CTR-002 | `AgentSession` + `AgentHandle` + `Inbox` | 15 | 11, 32 | Draft (15) |
| CTR-003 | `WorkService` | 11 | 15, 20, 32, UI | Provisional |
| CTR-004 | `SessionLog` + projections (ui · prompt · pending) | 11 | 15, 16, UI | Provisional |
| CTR-005 | `CheckpointService` (produce · rebuild) | 16 | 11, 15, 20 | Draft (16) |
| CTR-006 | `ContextProvider` | 16 | 15, 20, 32 | Draft (16) |
| CTR-007 | `ContextController` | 15 | the bound engine; reference for external agents | Draft (15/16) |
| CTR-008 | `MemoryService` | 17 | 15, 16, 32, UI | Draft (17) |
| CTR-009 | `CapabilityBroker` | 13 | 15, 20, 32 | Provisional |
| CTR-010 | `ProviderAdapter` | 14 | 13 | Provisional |
| CTR-011 | `Guard` (check · tickets · revoke) | 12 | 13, 14, 32, UI | Provisional |
| CTR-012 | `ApprovalService` | 12 | 15, 20, UI | Provisional |
| CTR-013 | `Vault` (use-without-expose) | 12 | 18, 14, 28 | Provisional |
| CTR-014 | `ModelRouter` / `ModelAdapter` | 18 | 15, 24 | Provisional |
| CTR-015 | `EnvironmentService` | 19 | 14, 22–26 | Provisional |
| CTR-016 | `WorkflowEngine` | 20 | 15, 32, UI | Provisional |
| CTR-017 | `WorldService` (query · subscribe) | 21 | 16, 20, 22–24, UI | Provisional |
| CTR-018 | `ArtifactService` + `ReceiptService` | 29 | 15, 20, 32, UI | Provisional |
| CTR-019 | `EventBus` / `EventStore` | 30 | all | Provisional |
| CTR-020 | `SkillResolver` | 31 | 13, 15 | Provisional |
| CTR-021 | `DelegationService` (SubagentManager) | 15 | 15-internal, 11, 20 | Draft (15) |
| CTR-022 | `AgentGateway` | 32 | external agents | Provisional |
| CTR-023 | `EffectVerifier` | 34 | 13, 29 | Provisional |
| CTR-024 | `FileIdentity` + `WorkspaceWatcher` + `WriteLeases` | 25 | 16, 21, 26 | Provisional |
| CTR-025 | `RepoIntelligence` (graph · map · lsp · git) | 26 | 16, 15 | Provisional |
| CTR-026 | `Scheduler` (lanes · limits) | 11 | 15, 20, 32 | Provisional |
| CTR-027 | `MissionService` (contract/plan/dispatch/reconcile) | 35 | 11, 15, 20, 32, UI | Proposed (DEC-054) |
| CTR-028 | `OutcomeEvaluator` (requirement/mission evaluation) | 36 | 35, 29, UI | Proposed (DEC-054) |
| CTR-029 | `MissionRecovery` (fingerprint/impact/reopen) | 36 | 35, 11, 19, 29 | Proposed (DEC-054) |
| CTR-030 | `AgentBindingAdapter` (negotiate/launch/steer/status/receipt) | 15 | 35, 11, 32 | Proposed (DEC-054) |
| CTR-031 | `ExternalWorkflowAdapter` (invoke/status/cancel/callback) | 20 | 35, 11, 37 | Proposed (DEC-054) |
| CTR-032 | `PreferenceService` (registry/effective/snapshot/compare-and-swap/profile) | 48 with Core storage | UI, 12, 15, 32 | Proposed (DEC-056) |
| CTR-033 | MachineObserverService (snapshot/query/history/sampling/health) | 51 service; Core adapter owns trust projection | 13, 19, 21, 48 | Proposed (DEC-058) |

**Domain contracts.** Office (22), Browser (23), Computer Use (24) and Comms (28) deliberately own no named contract yet: their operations resolve through CTR-009 as capability descriptors, and a module pass registers a contract here only if a non-capability edge appears (see `ARCH/03-HLD.md` §3.1). Search (27) implements the one Core search service behind the **CTR-006 `context.search`** façade (owned by `16`) and resolves its other operations through CTR-009 — no separate Search contract is minted.

## 2. The six core contracts (owner brief)

### CTR-001 `AgentEngine` — host adapter surface for compatible bound runtimes (15)
```
createSession(options: SessionOptions) → AgentSession
resumeSession(id) → AgentSession | Unsupported
run(session, input: RunInput) → RunHandle
steer(session, input: SteerInput) → void | Unsupported
interrupt(session) → void | Unsupported
cancel(run) → void | Unsupported   // Core Work cancels; native termination depends on adapter
spawnSubagent(options: SubagentOptions) → SubagentRef   // host Work delegation through CTR-021
dispose(session) → void
```
**Guarantees:** host input is durable and delivered at the adapter's negotiated safe boundary; steering never claims mid-tool delivery without support. Core `cancel` is terminal for its Work and cascades through host-owned child Work; termination of an external process is separately acknowledged or left uncertain. `dispose` releases host-owned environment resources. DEC-054: unsupported controls return typed `Unsupported`; cross-engine children use CTR-021/Work. Core can replace a session with new Work and reconstructed context; interchangeability does not imply identical native APIs.

### CTR-002 `AgentSession` + `AgentHandle` + `Inbox` (15)
- `AgentHandle` is a **capability** (`dispose()` only) returned to the owner; the registry keeps factories, not live internals.
- `Inbox` is a **durable Core projection** over host session events. `next-turn` and `next-step` delivery are available only when the adapter supports those boundaries; otherwise accepted input queues for the next supported turn.
- Core session events are append-only; private engine session/transcript state remains native and is not reconstructed from the host log.

### CTR-006 `ContextProvider` + CTR-007 `ContextController` (16 / 15)
```
// Core — what context exists (read-only, INV-08)
search(query, scopes) → ContextCandidate[]
snapshot(scope) → ContextSnapshot
get(id) → ContextItem
checkpoint(scope) → ContextCheckpoint
projection(target, policy) → ScopedSlice

// Agent — what the model sees now (control)
assemble(request) → ModelContext
estimateBudget(request) → Budget
select(items) → ContextSelection
prune(ctx) → ctx          compact(ctx) → ctx          rebuild(checkpoint) → ModelContext
pin(item) · exclude(item)
```
**Guarantees:** recall-style reads never mutate state; budgets are maxima (zero hits ⇒ zero injected tokens); projections are sensitivity-filtered.

### CTR-009 `CapabilityBroker` (13)
```
resolve(capability_id, constraints) → CapabilityHandle
invoke(handle, op) → CapabilityResult        // status: completed | guidance | requires_user_action | failed
descriptors(scope) → CapabilityDescriptor[]
health(provider_id) → HealthStatus
```
**Guarantees:** every Core-mediated `invoke` walks Guard → Ticket → Execute → Verify → Receipt (DEC-002/063); handles are epoch-checked (DM-012, `13` §4; stale ⇒ `InvalidState`); callers never see providers or transports (INV-15).

### CTR-010 `ProviderAdapter` (14)
```
discover() → ProviderInfo        connect() → void        health() → HealthStatus
capabilities() → CapabilityDescriptor[]
execute(op: CapabilityInvocation) → CapabilityResult
shutdown() → void                events() → AsyncIterable<ProviderEvent>
```
**Guarantees:** adapters are the only place protocol/transport knowledge lives; egress goes through Guard (INV-05); health/events feed the provider registry.

### CTR-014 `ModelRouter` / `ModelAdapter` (18)
```
resolve(preferences, constraints) → ModelSelection
stream(request) → AsyncIterable<ModelChunk>
capabilities(model) → ModelDescriptor
mapReasoning(level) → provider-params
```
**Guarantees:** Core-owned model calls use credentials via `CTR-013` (never read); context windows/tokenization feed the Context budget (16); no Core model consumer hard-codes a vendor. An external agent may retain its native model/provider configuration; CTR-014 cannot silently override it (DEC-054).

### CTR-021 `DelegationService` (SubagentManager) (15)
```
delegate(spec: {worker, task, context_refs, limits}) → RunHandle
collect(run) → WorkerReceipt
policies() → DelegationPolicyEntry[]
```
**Guarantees:** one child session per subagent (never a prompt fork); receipts, not transcripts; outer bounds enforced here (parallel/total/depth/tokens/spend); worktree isolation is a per-spawn option (DEC-029).

## 3. Trust spine

**CTR-011 `Guard`** — `check(action_context) → ALLOW | ASK | DENY` · `issueTicket(authorization) → Ticket` · `validate(ticket, action) → ok | denied` · `revoke(ticket)`. Composes the three layers (DEC-028): platform confinement × approval policy × declarative exec rules.
**CTR-012 `ApprovalService`** — `request(prompt, options) → Approval` · `decide(id, decision)` · `list(scope)`. Used by agents (questions) and workflows (approval nodes) — one primitive (DEC-021).
**CTR-013 `Vault`** — `use(secret_ref, action)` style APIs only; **no read**. No contract returns a credential value (INV-02).

## 4. Evidence spine

**CTR-018 `ArtifactService` + `ReceiptService`** — `create/version/get/link/export`, `resolveSelection(artifact_id,version,typed_location)`, `list(filters,scope)` and `dependencyStatus(version)` for artifacts; `record/get/replay` for Core-effect receipts. Immutable versions; provenance mandatory; index state and exact-version location accompany retrieval; Core-effect receipts reference tickets and verification; native observations/reports use distinct evidence types (INV-07/INV-18, DEC-054/055). Rendering/editing remains in domain providers and the Experience surface, never in this store contract.
**CTR-019 `EventBus`/`EventStore`** — `publish(event)` · `subscribe(filter) → Stream` · `read(range)` · `replay(from)`. One log; all projections derive from it (INV-23).
**CTR-023 `EffectVerifier`** — `verify(effect, risk_class) → VerificationRecord`; depth scales with risk (INV-19).

**CTR-032 `PreferenceService`** — `registry()` returns stable key, owner, parser, scope, default and sensitivity; `effective(context)` returns source and revision; `update(key,value,expected_revision)` validates scope and compare-and-swap semantics; `profile_create/preview/apply/archive` operate on non-secret overrides only. Device-local cosmetic state may use the same schema in the UI, but Trust policy and extension grants resolve from Core authority, never from a renderer copy (DEC-056).

**CTR-033 MachineObserverService** (51) provides hello, capabilities, snapshot, typed query, history, opt-in sampling start/stop, revocation, health and shutdown operations. Requests carry Core-resolved consent/grant references, the requested scope and hard result/time bounds; a caller cannot supply consent as authority. Before starting the service or invoking a provider, Core resolves Trust/Capability and returns a typed `authorization_required` response with the complete `missing_grants` set and invariant `sampled=false`. Entries distinguish local `consent_required(category, purpose, scope)` from `work_grant_required(work, capability, fields)`; if both are missing, return both. Experience may show one dialog with independent choices, but selecting one never satisfies the other; the request remains held until every required grant is valid. Neither an agent nor the Observer may grant consent. The service independently validates its scoped startup lease and typed schema. It returns sample/source/unit/freshness/status projections, never raw database access or credentials. It exposes no TCP/HTTP listener by default and no mutating OS operation. An optional helper is a separate process/protocol for one specific read operation and exits after returning one result; installation or UAC consent is never reused as data consent. Directory tree scans are not in this contract; Core delegates to the existing Files/Storage owner.

### Trigger and remote execution ownership

**CTR-016 `WorkflowEngine`** — `publish(definition)`, `register_trigger(definition_version, owner_ref)`, `materialize(source_event_or_due_time, dedupe_key)`, `claim(occurrence_id, owner_epoch)`, `status(run_id)`, `cancel(run_id)` and `handoff_trigger_owner(expected_epoch, target)` operate on the one persisted occurrence journal. Work's CTR-026 admits resulting execution. A claim is one logical run per occurrence identity, not an exactly-once claim for external effects; those still require idempotency or reconciliation (`20` §4, REQ-WF-003/004). A source event/webhook must be authenticated and deduped before materialization. A closed UI does not keep a trigger alive without a healthy local service or accepted remote owner (`19` §7).

**CTR-027/028/029 Mission seams** — MissionService owns compare-and-swap contract/plan versions and semantic dispatch; a comparison PlanNode pins eligible worker bindings, candidate bound, shared input/contract versions, criteria, concurrency and budget policy, then dispatches ordinary Work attempts; OutcomeEvaluator consumes each candidate's evidence with provenance and returns criterion-level comparison and per-requirement verdicts; MissionRecovery reconciles environment/effect uncertainty before reopening nodes. Selection/integration is a separate user/policy decision and Work. These contracts do not own the Work queue or agent reasoning (`35`, `36`, `46`). **CTR-030/031 adapters** expose negotiated agent lifecycle and external workflow invoke/status/cancel/callback with typed `Unsupported`, stable external run identity, idempotency and authenticated callbacks; neither imports provider-internal state as Core truth (`15`, `20`, `37`).

## 5. Cross-contract rules

1. **Ticket first:** no effect-bearing Core capability contract executes without a valid ticket; contracts never accept raw provider handles to bypass this. Native agent tools are not Core capability contracts.
2. **Receipts:** every Core-mediated externally visible mutation returns a receipt ref or a typed error; native agent observations and self-reports are distinct evidence, not forged receipts.
3. **Cancellation:** every long-running call accepts a cancellation handle; cancellation is cooperative, bounded, and leaves durable state consistent (INV-16).
4. **Idempotency:** effect invocations carry idempotency keys derived from `work_id + ticket` so retries cannot double-apply where providers support dedupe.
5. **Epoch binding:** tickets and handles are bound to `provider_epoch` + `environment_id`.
6. **Version pinning:** workflow runs pin their definition version; agent and adapter versions are always reported.
7. **Projection rule:** external callers (agents, channels) receive projections only — never stores, never vault, never other agents' state.

## 6. Open questions (`OQ-CTR-*`)

1. **Resolved (P7 passes 11–14):** signatures firmed with their owner docs; churn now tracked per contract.
2. **Resolved:** one service, two facets — `WorkService` (CTR-003) + `Scheduler` (CTR-026).
3. **Resolved:** separate contract — `ApprovalService` (CTR-012), composed by Guard.
4. **Resolved (§5.4):** idempotency keys derive from `work_id + ticket`.
5. External-agent contract surface mapping to ACP (32) — which CTRs are exposed and how.
6. **Resolved (§0):** additive = minor; breaking requires a `DEC` (+ deprecation window per `13` §8).

## 7. Evidence

Product-owner brief (“six core contracts”, `AgentEngine`, `ContextProvider`/`ContextController`, `ProviderAdapter`, `CapabilityDescriptor/Result/Handle`) · `ARCH/15-AGENT-PLANE.md` §2 · `ARCH/16-CONTEXT.md` §1 · `ARCH/17-MEMORY.md` §4 · `ARCHIVE/v1-research/agent-harness-verification.md` §D1 (handle/factory/inbox), §A4 (guard layers), §C1–C2 (log/projection).
