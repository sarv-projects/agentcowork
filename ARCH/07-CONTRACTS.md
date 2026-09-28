# 07 — Contracts (canonical cross-module interfaces)

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P1) — the interface registry. Every named contract that crosses a module boundary lives here. **Owner** = the module that implements/stabilizes it; **consumers** = modules that call it. Module docs carry serialization/transport detail; this doc owns names, semantic signatures, and guarantees. Contract status reflects implementation maturity — every owner doc is complete as of the v1 freeze.
> **Rules:** signatures are transport-free (adapters map transports); each Core contract takes an `actor` context (user / agent / workflow). Effect-bearing Core contracts are subject to Trust; the external agent's native operations are outside Core contracts (DEC-054).
> **SDD:** this registry carries the L3 interface layer for behaviors in `ARCH/08-REQUIREMENTS.md`; REQ ↔ CTR links accrue in `ARCH/09-FEATURE-MATRIX.md`.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).

## 0. Conventions

- **IDs:** `CTR-###`, stable.
- **Async by default**; every long-running call accepts a cancellation handle and declares a default timeout + retryability.
- **Typed errors** only: `AuthorizationDenied · NotFound · Conflict · Unavailable · Unsupported · Timeout · InvalidState · GuidanceRequired · RequiresUserAction · Internal`. `guidance`/`requires_user_action` are **results**, not errors (a capability may answer “connect Google Drive first”).
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

**Domain contracts.** Office (22), Browser (23), Computer Use (24) and Comms (28) deliberately own no named contract yet: their operations resolve through CTR-009 as capability descriptors, and a module pass registers a contract here only if a non-capability edge appears (see `ARCH/03-HLD.md` §3.1). Search (27) implements the one Core search service behind the **CTR-006 `context.search`** façade (owned by `16`) and resolves its other operations through CTR-009 — no separate Search contract is minted.

## 2. The six core contracts (owner brief)

### CTR-001 `AgentEngine` — every agent runtime implements this (15)
```
createSession(options: SessionOptions) → AgentSession
resumeSession(id) → AgentSession
run(session, input: RunInput) → RunHandle
steer(session, input: SteerInput) → void
interrupt(session) → void
cancel(run) → void                  // terminate the work; children cascade
spawnSubagent(options: SubagentOptions) → SubagentRef   // immediate spawn (DEC-036); full delegation contract: CTR-021
dispose(session) → void
```
**Guarantees:** input is admitted only at turn/step boundaries (inbox); `steer` never lands mid-tool; `interrupt` is cooperative and bounded; `cancel` is terminal for the Work, cascades through host-owned child Work, and never rebuffers a completion (DEC-036); `dispose` releases host-owned environment resources. DEC-054: adapter capabilities are negotiated per binding; unsupported `resumeSession`, `steer` or native `spawnSubagent` returns a typed `Unsupported` result, while cross-engine child creation uses CTR-021/Work. Core can replace a session by starting new Work with a reconstructed packet; interchangeability does not imply identical native APIs.

### CTR-002 `AgentSession` + `AgentHandle` + `Inbox` (15)
- `AgentHandle` is a **capability** (`dispose()` only) returned to the owner; the registry keeps factories, not live internals.
- `Inbox` is a **durable projection** over session events with two admission boundaries: `next-turn` (user messages) and `next-step` (injected context / tool results).
- Session events are append-only; nothing reads mutable session internals directly.

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
**Guarantees:** `invoke` always walks Guard → Ticket → Execute → Verify → Receipt (DEC-002); handles are epoch-checked (DM-012, `13` §4; stale ⇒ `InvalidState`); callers never see providers or transports (INV-15).

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
**Guarantees:** credentials are used via `CTR-013` (never read); context windows/tokenization feed the Context budget (16); no module hard-codes a vendor.

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
