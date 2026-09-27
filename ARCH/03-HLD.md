# 03 — High-Level Architecture (HLD)

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P0) — architecture root for **HOW** (see `ARCH/00-INDEX.md` §2). Module docs derive from this file; conflicts escalate to a `DEC` entry.
> **Companion docs:** `ARCH/02-THESIS.md` (identity, principles) · `ARCH/06-DATA-MODEL.md` (entities) · `ARCH/07-CONTRACTS.md` (interfaces).
> **SDD:** this doc is the L2 architecture layer — it satisfies behaviors registered in `ARCH/08-REQUIREMENTS.md` and must not contradict them; module → REQ traceability accrues in `ARCH/09-FEATURE-MATRIX.md`.
> **Fleshed:** P7 (2026-09-26) — contract index (§3.1), failure model (§11), non-functional envelope (§12).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).

## 1. Shape

```mermaid
flowchart TB
  subgraph XP["Experience Plane"]
    UID["Desktop UI"]
    CLI["CLI"]
    IDE["IDE / ACP"]
    CH["Channels · API · Mobile"]
  end

  subgraph CORE["Core"]
    WORK["Work Plane<br/>Work · Step · Task · Session · Run · Checkpoint · Scheduler"]
    ORCH["Orchestration<br/>Delegation · Agent Graph"]
    AGX["Agent Runtime<br/>bound engine + external adapters"]
    CTX["Context Infrastructure + Memory"]
    MODELS["Model Plane<br/>registry · router · adapters"]
    CAP["Capability Plane<br/>Registry · Resolver · Handles · Affordances · Guidance"]
    TRUST["Trust / Control<br/>Policy · Guard · Approvals · Tickets · Vault"]
    EXEC["Execution Plane<br/>Providers · Environments · Sandbox"]
    DOM["Domain Runtimes<br/>Office · Browser · Computer · Files · Code · Search · Comms"]
    WF["Workflow Engine"]
    WORLD["World Model<br/>scanner · registries · graph · events"]
    VER["Effect Verification<br/>validate · render · reconcile"]
    ART["Artifacts + Receipts"]
    EVT["Events"]
  end

  XP --> WORK
  WORK --> ORCH
  ORCH --> AGX
  ORCH --> WF
  AGX --> CTX
  AGX --> MODELS
  AGX --> CAP
  WF --> CAP
  CAP --> TRUST
  TRUST --> EXEC
  EXEC --> DOM
  DOM --> VER
  VER --> ART
  ART --> EVT
  WORLD --> CTX
  WORLD --> WF
  EVT --> WORLD
```

*The diagram shows primary flow, not every edge. The authoritative edge list is the module map (§3) plus each module doc's contract section.*

## 2. Planes — ownership boundaries

| Plane | Owns | Never owns |
|---|---|---|
| Experience | Rendering, input, presentation, view state | Domain logic, execution, policy |
| Work | Lifecycle, checkpoints, scheduling, budgets, cancellation | Reasoning, execution |
| Orchestration | Delegation, agent graph, workflow control flow | Raw execution |
| Agent Runtime | Reasoning, context control, planning, model calls | Capability implementation, policy |
| Capability | Semantic operation catalog, resolution, handles, affordances, guidance | Model reasoning |
| Trust / Control | Policy, permissions, approvals, tickets, egress, custody (vault), audit | Domain logic |
| Execution | Running provider calls, environments, sandbox, process lifecycle | Deciding *whether* to run |
| Domain Runtimes | Office / Browser / Computer / Files / Code specialized execution | Agent reasoning, governance |
| Effect Verification | Validate / render / verify / reconcile effects | Deciding what to build |
| Receipts & Events | Durable evidence, replay, audit, projections | Live execution |
| World Model *(cross-cutting)* | Structural state of the machine + change stream | Acting on the world |
| Model Plane | Model catalog, routing, adapter normalization; credential **use** via vault | Holding credentials (vault owns custody) |

## 3. Module map

> `Depends on` lists primary dependencies. Every edge MUST have a named contract in `ARCH/07-CONTRACTS.md` (P1) or the owning module doc.

| Doc | Module | Responsibility | Depends on |
|---|---|---|---|
| 10 | Kernel | ids, errors, config, time, serialization, contracts | — |
| 11 | Work | Work/Step/Task/Session/Run/Checkpoint/Scheduler; lanes (foreground/background/detached) | kernel |
| 12 | Trust | Policy, Guard, approvals, tickets, vault, egress, audit hooks, external-agent projections | kernel, work |
| 13 | Capability | Registry, catalog, resolver, handles (epoch-checked), affordances, guidance | kernel, work, providers, trust |
| 14 | Providers | `ProviderAdapter` contract + native/MCP/ACP/HTTP/CLI/plugin/remote adapters; MCP era policy | kernel, trust, runtime-environments |
| 15 | Agent plane | The `AgentEngine` contract every agent implements, plus delegation + subagent lifecycle, isolation modes and receipts | work, context, memory, capability, models, runtime-environments |
| 16 | Context | Context infrastructure (Core: store/query/snapshot/checkpoint) + context control (the bound engine) + projections | kernel, files, world-model, memory, artifacts |
| 17 | Memory | Durable memory: layers, write/read paths, minimal algorithm set, lifecycle | kernel, events |
| 18 | Models | Model registry, router, adapters, local discovery (Ollama/LM Studio/vLLM/llama.cpp), reasoning mapping | kernel, trust (vault) |
| 19 | Runtime & Environments | Process manager, environments, sandbox, lifecycle, health | kernel, trust |
| 20 | Workflow | Workflow IR, registry, scheduler, state/version store, approval nodes | work, capability, agent runtime, events, world-model |
| 21 | World Model | Scanner, app/window/process/device/BrowserWorld/FileWorld registries, world graph, event stream, incremental updates | kernel, events, domain collectors |
| 22 | Office | L1/L2/L3 semantics, resident contexts, batch, render/validate, format providers | capability, providers, runtime-environments, trust |
| 23 | Browser | Managed Chromium + adapters, BrowserWorld, capability ladder rungs | capability, providers, world-model |
| 24 | Computer Use | UI automation, accessibility trees, vision fallback, input safety | world-model, capability, models (vision) |
| 25 | Files | File identity, watchers, deltas, leases (write conflicts) | kernel, events |
| 26 | Code | RepoGraph, RepoMap, LSP bridge, worktrees, code execution | files, capability, runtime-environments |
| 27 | Search | Search plane (files, memory, artifacts, world objects) | files, memory, artifacts, world-model |
| 28 | Comms | Connectors: email/calendar/messaging as capability layer | capability, providers, trust |
| 29 | Artifacts | Artifact + Receipt models, versions, provenance, previews, library promotion | kernel, files, events |
| 30 | Events | Event store, bus, replay, subscriptions; usage & cost telemetry | kernel |
| 31 | Skills & Plugins | Skill registry/loader/resolver; plugin surfaces (capabilities, providers, agents, models, channels, UI) | capability, work, trust |
| 32 | Channels | Surfaces & protocols: desktop, CLI, ACP, A2A, API, mobile; agent gateway | everything above (thin) |
| 34 | Effect Verification | Validate/render/verify/reconcile pipeline; receipt policy per risk | domains, artifacts, events |
| 40–42 | Cross | Flows, edge cases, evidence map | all |
| 43 | Glossary | Canonical terms — defined once, linked back (meta) | — |
| 44 | Absorb Register | Competitor absorb matrix + licensing ledger | archive/REPO-COMPARE evidence |

### 3.1 Contract index (module → owned contracts)

Every cross-module edge is named in `ARCH/07-CONTRACTS.md`. A module with no owned contract exposes only capability descriptors through CTR-009 until its module pass registers one.

| Module (doc) | Owned contracts |
|---|---|
| Kernel (10) | — (types, ids, errors only) |
| Work (11) | CTR-003 `WorkService` · CTR-004 `SessionLog` · CTR-026 `Scheduler` |
| Trust (12) | CTR-011 `Guard` · CTR-012 `ApprovalService` · CTR-013 `Vault` |
| Capability (13) | CTR-009 `CapabilityBroker` |
| Providers (14) | CTR-010 `ProviderAdapter` |
| Agent plane (15) | CTR-001 `AgentEngine` · CTR-002 `AgentSession` · CTR-007 `ContextController` · CTR-021 `DelegationService` |
| Context (16) | CTR-005 `CheckpointService` · CTR-006 `ContextProvider` |
| Memory (17) | CTR-008 `MemoryService` |
| Models (18) | CTR-014 `ModelRouter` / `ModelAdapter` |
| Runtime & Environments (19) | CTR-015 `EnvironmentService` |
| Workflow (20) | CTR-016 `WorkflowEngine` |
| World Model (21) | CTR-017 `WorldService` |
| Office (22) · Browser (23) · Computer Use (24) | — (capability descriptors through CTR-009; a named contract is registered only for a non-capability edge) |
| Files (25) | CTR-024 `FileIdentity` / `WorkspaceWatcher` / `WriteLeases` |
| Code (26) | CTR-025 `RepoIntelligence` |
| Search (27) | — (implements the Core search service behind `CTR-006 context.search`; capability descriptors through CTR-009) |
| Comms (28) | — (capability descriptors through CTR-009) |
| Artifacts (29) | CTR-018 `ArtifactService` / `ReceiptService` |
| Events (30) | CTR-019 `EventBus` / `EventStore` |
| Skills & Plugins (31) | CTR-020 `SkillResolver` |
| Channels (32) | CTR-022 `AgentGateway` |
| Effect Verification (34) | CTR-023 `EffectVerifier` |
| Cross (40–42) · Register (44) | — |

## 4. Dependency rules

1. **Direction is downward only:** Experience → Work/Orchestration → Agent/Capability → Trust → Execution → Domains; cross-cutting services (events, artifacts, memory, world) are leaves others may depend on, never the reverse.
2. **Kernel stays small.** No domain logic, no orchestration, no policy in the kernel.
3. **One implementation per responsibility.** No second orchestrator, registry, scheduler, provider system, or permission system — including “temporary” ones.
4. **Adapters at the edge.** MCP/ACP/CLI/HTTP/remote live only in Providers/Channels; nothing above Capability knows the transport.
5. **Domains never govern themselves.** Domain runtimes execute; Trust decides; the kernel never special-cases a domain's permission path.
6. **External agents are clients of the public contract** — they MUST NOT be given internal module access to make integration easier.

## 5. The governed execution path

```
USER INTENT
   → WORK (create/resume)                     ◄── control path: bounded, synchronous
   → CAPABILITY (resolve semantic operation)
   → PROVIDER (resolver picks implementation)
   → HANDLE (cached, epoch-checked)                control path target:
   → GUARD (ALLOW | ASK | DENY)                    p50 < 2 ms · p95 < 10 ms · p99 < 25 ms
   → TICKET (scoped, time-boxed authorization)
   → EXECUTE (enqueued)                        ◄── effect path: async, observable
   → EFFECT
   → VERIFY (validate / render / reconcile)
   → RECEIPT (durable evidence)
   → EVENT (published)
```

- **No shortcuts.** Not `agent → raw MCP tool → side effect`; not `UI → special-cased Office backend`; not `browser feature → its own permission system`; not `external agent adapter → its own capability semantics`.
- **Verification depth scales with risk class** (`safe` / `sensitive` / `dangerous`) — defined in `ARCH/34-EFFECT-VERIFICATION.md`.
- **Receipts are mandatory for externally visible effects.**

## 6. Scoping model

**Four states** — applied to MCP servers, skills, plugins, providers, models alike:

| State | Question | Example |
|---|---|---|
| Installed | Does AgentCowork have this at all? | GitHub MCP server installed |
| Available | Can this agent/workspace use it? | Workspace policy allows it |
| Activated | Is it loaded for this task? | Only relevant skills load into context |
| Executing | Is it running right now? | An actual tool call / session / worker |

**Five scopes** (outer → inner): **Global/User → Workspace/Project → Agent → Session → Run/Task.**
Resources are installed/available at Global or Workspace and never duplicated per agent; only activation and execution are scoped narrowly.

## 7. Cross-cutting subsystems

- **Context** (`16`): Core owns infrastructure (store/query/snapshot/checkpoint/projection); the bound engine owns control (selection/ranking/budget/prune/compact/rebuild). External agents get a **context projection**, never the substrate.
- **Memory** (`17`): durable knowledge; written via explicit lifecycle (not an LLM dumping everything); read through the Context Controller under budget. Memory ≠ context.
- **World Model** (`21`): the machine explains itself — registries + graph + event stream; consumers query, they do not screenshot by default.
- **Events** (`30`): one event store; UI projections, workflow triggers, world updates, audit, and usage/cost telemetry all derive from it.
- **Artifacts & Receipts** (`29`): outputs of work vs reusable inventory (Library); promotion is explicit.
- **Model Plane** (`18`): one catalog, one router; every engine and every internal consumer asks the router, never a vendor SDK directly.

## 8. Module interop matrix (first cut — expanded per module in P2/P3, verified in P6; re-verified in P9)

| Module | Exposes (primary) | Consumed by |
|---|---|---|
| Kernel | types, ids, errors, config | all |
| Work | Work service, lanes, checkpoints | agents, workflows, UI, scheduler consumers |
| Trust | Guard decisions, tickets, approvals, projections | capability, providers, channels, domains |
| Capability | resolve/invoke, handles, descriptors | agents, workflows, UI (deterministic ops) |
| Providers | adapter registry, health, events | capability |
| Agent plane (15) | `AgentEngine` contract + `DelegationService`, CLI and ACP surfaces | work, channels, delegation |
| Context | query/snapshot/checkpoint + projection | agents, workflow nodes |
| Memory | memory service (write/read/forget) | agents, context |
| Models | registry, router, adapters | agents, vision, embeddings |
| Runtime & Environments | environment handles, process/sandbox lifecycle | providers, domains |
| Workflow | engine, registry, run state | agent tool surface, schedulers |
| World Model | world query/subscribe | context, workflows, domains, UI |
| Domains | capability descriptors + execution | capability |
| Artifacts | artifact/receipt service | agents, workflows, UI, library |
| Events | store/bus/replay/subscriptions | everyone (read/subscribe) |
| Skills & Plugins | skill resolver, plugin surfaces | capability, work, agents |
| Channels | Agent Gateway, surface mappings | external agents, UI/CLI/IDE |
| Verification | validated effects + render/verify results | capability (pre-receipt) |

## 9. Implementation order (after docs freeze; not current work)

1. **Six core contracts first:** `AgentEngine` (CTR-001), `AgentSession` (CTR-002), `ContextController` + `ContextProvider` (CTR-007/006), `CapabilityBroker` (CTR-009), `DelegationService` (CTR-021), `ModelAdapter` (CTR-014).
2. **Minimal native runtime:** model streaming, tool loop, project rules, RepoGraph/RepoMap, filesystem, shell, git, parallel workers, background execution, structured-checkpoint compaction, Core capability access.
3. **Bolt on domains:** browser, Office, computer-use, MCP provider adapter, plugins/skills, ACP server.
4. **Workflow Engine** wired to Capability Plane and World Model events.
5. **World Model** — scanner, registries, graph, incremental updates.
6. **Experience Plane** — shell, Workbench, universal document surface, composer with the namespace protocol.
7. **Multi-surface** — CLI, IDE/ACP, API, mobile, cloud/remote handoff.

## 10. Architecture risks to resolve in module passes

| ID | Risk | Resolved in |
|---|---|---|
| RISK-001 | Control-path latency targets depend on handle caching + ticket reuse; needs a design that keeps Guard cheap without weakening it. | 12, 13 |
| RISK-002 | World Model scope creep (scanning everything) vs value; incremental updates must be provably bounded. | 21 |
| RISK-003 | Workflow durability parity with established runtimes (timers, versioning, cancellation) without adopting one wholesale. | 20 |
| RISK-004 | Office resident contexts — memory/lifecycle bounds and crash safety. | 22 |
| RISK-005 | External-agent projection fidelity: enough context to work, not enough to leak. | 12, 16, 32 |
| RISK-006 | Memory minimalism: retrieval quality with a small algorithm set. | 17 |
| RISK-007 | Provider adapter counting: capability × provider matrix stays declarative, not hand-maintained. | 13, 14 |

## 11. Failure, recovery, and degradation

The architecture fails **closed** at trust boundaries and **honestly** everywhere else: no component fabricates state to hide a failure, and no failure path bypasses Guard (INV-01, INV-05).

| Failure | Architectural behaviour | Owner |
|---|---|---|
| Guard unavailable / cannot decide | DENY — effects never execute on a missing decision; no fallback path exists | 12 |
| Vault unavailable | Credential *use* fails closed; no plaintext fallback, no cached secret | 12 |
| Provider down / degraded | Health flips; the resolver may select another provider, otherwise a typed `Unavailable`; executed effects are not silently retried | 13, 14 |
| Execution crashes mid-effect | Effect state is reconciled at next start; where the outcome cannot be determined, the receipt records uncertainty rather than claiming success | 19, 34 |
| Domain runtime crash (Office/Browser/CUA) | Host survives; the provider epoch bumps, invalidating stale handles (DM-012, `13` §4); resident contexts are bounded and released | 22–24 |
| Agent run crashes | Work is marked failed with its last checkpoint retained; host and other sessions are unaffected | 11, 15 |
| Workflow runner restarts | Runs resume from checkpoints against their pinned definition version (INV-16) | 20 |
| World scanner lags | Consumers see `freshness` stamps and must tolerate staleness — never fabricate live state | 21 |
| Event store temporarily unavailable | Producers/consumers surface the gap and mark projections stale; no shadow state is created (INV-23) | 30 |
| Memory unavailable | Runs degrade to stateless context — memory is an addition, never a blocker for execution | 17 |
| UI disconnects | The cockpit re-derives its view from projections on reconnect; no UI-held state is treated as truth | 32 |

## 12. Non-functional envelope

| Axis | Envelope |
|---|---|
| Control-path latency | p50 < 2 ms · p95 < 10 ms · p99 < 25 ms for the guarded resolution path (SPEC §4); the effect path is asynchronous and observable |
| Token discipline | Deterministic operations never call a model (P-14, INV-13); context assembly is budgeted and honest about spend (INV-22) |
| Platform | Windows is the first release target; macOS/Linux are hosted targets; no design may assume a POSIX-only primitive (`ARCH/19-RUNTIME-ENVIRONMENTS.md`) |
| Resource isolation | Kernel stays minimal (INV-14); domain runtimes and sandboxed execution carry their own bounds; a failing domain cannot take down the host |
| Durability | Work, checkpoints, receipts and events survive restart; runs are pinned to versions (INV-16, INV-18) |
| Security | One authorization decider, one egress path, vault custody (INV-02/04/05); every externally visible effect is auditable (INV-24) |
| Observability | One event log; usage/cost telemetry derives from it; every effect has a receipt (INV-07, INV-23) |
| Verification | Depth scales with risk class (`ARCH/34-EFFECT-VERIFICATION.md`); "implemented" ≠ "verified" (SPEC §13) |
