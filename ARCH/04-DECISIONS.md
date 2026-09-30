# 04 — Decision Register

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P1). Every decision that shapes v1 is recorded here with its evidence. Module docs cite `DEC-*` instead of repeating rationale.
> **Statuses:** `Locked` — agreed for v1; changing it requires a new DEC superseding this one. `Provisional` — directionally fixed; detail pending. `Pending` — not yet decided (§3). `Deferred` — out of v1 with an explicit trigger.
> **Change rule:** any change to an authority doc (`AGENTCOWORK-SPEC.md`, `ARCH/03-HLD.md`, module docs, contracts) that alters behavior requires a DEC entry here.
> **SDD:** requirements cite decisions in their `Source` field; REQ ↔ DEC links accrue in `ARCH/09-FEATURE-MATRIX.md`. A Locked decision changes only by a superseding DEC — its text is never silently edited.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).

## 1. Register

| ID | Decision | Status | Affects |
|---|---|---|---|
| DEC-001 | Product frame: Work + Capability runtime layered on the user's computer — not an OS replacement | Locked | 02, SPEC |
| DEC-002 | One governed execution path; control path bounded, effect path asynchronous | Locked | 03, 12, 13, 14 |
| DEC-003 | Work is the universal execution abstraction | Locked | 11 |
| DEC-004 | Agent ≠ Model ≠ Provider; Capability ≠ Provider | Locked | 13, 14, 15, 18 |
| DEC-005 | Protocols (MCP/ACP/CLI/HTTP/plugin) are provider adapters only | Locked | 14, 32 |
| DEC-006 | Domain runtimes own specialized complexity; the kernel stays small | Locked | 10, 22–28 |
| DEC-007 | Context: infrastructure in Core, control in the agent, projection for external agents | Locked | 16 |
| DEC-008 | Workflow engine is Core infrastructure; agent ⇄ workflow composition in both directions | Locked | 20 |
| DEC-009 | External agents get projections only (identity/capabilities/context/workspace/tools/artifacts/events) | Locked | 12, 16, 32 |
| DEC-010 | ~~Agent X is an architectural peer~~ → **generalized by DEC-052:** *every* agent engine is a peer — one `AgentEngine` contract, no privileged path | Locked (rule unchanged in force; the first-party agent is retired, DEC-052) | 15, 32 |
| DEC-011 | World Model is first-class; computer-use ladder; vision is fallback | Locked | 21, 24 |
| DEC-012 | Browser: managed Chromium default; Chrome/Edge/Firefox/Opera as adapters | Locked | 23 |
| DEC-013 | Office: runtime under the universal document surface; L1/L2/L3; resident contexts | Locked | 22 |
| DEC-014 | Artifacts (work products) ≠ Library (reusable inventory); promotion explicit | Locked | 29 |
| DEC-015 | Token discipline: deterministic operations never touch an LLM | Locked | 02, 16, 34 |
| DEC-016 | No captcha-evasion / anti-bot / residential-proxy tooling | Locked | 23, 24 |
| DEC-017 | Absorb strategy: study → redesign → implement; licensing ledger gates code reuse | Locked | 44 |
| DEC-018 | Memory v1 store: SQLite + FTS5; ADD-only extraction + `superseded_by`; suppression-based forget; no vectors/graph/decay | Locked | 17 |
| DEC-019 | Memory ≠ context; recall returns candidates; non-touching read; budget is a maximum | Locked | 16, 17 |
| DEC-020 | Working names: ~~AgentCowork / Core / Agent X~~ → **AgentCowork / Core**; engines are external in v1 (DEC-052) · code identifiers unfrozen and renamed by DEC-053 | Provisional | 01 |
| DEC-021 | Approval is a first-class primitive for agents and workflows | Locked | 12, 20 |
| DEC-022 | Receipts mandatory for externally visible effects; verification proportional to risk | Locked | 29, 34 |
| DEC-023 | Effect-verification plane (validate / render / reconcile) | Locked | 34 |
| DEC-024 | Four-state scoping (Installed/Available/Activated/Executing) + five scopes | Locked | 03, 13, 31 |
| DEC-025 | Native-first capability resolution for agents; Core enforces | Locked | 13, 15 |
| DEC-026 | Docs rebuild process: v1 from scratch, v0 archived, pass protocol + viability checklist, evidence-first, TODO exempt | Locked | 00 |
| DEC-027 | Context budget & compaction discipline: named terms (keep/buffer/reserve) · pre-turn feasibility check · overflow recovery · checkpoint as projection boundary over a durable log · pruning separate; no copied "native" path | Locked | 16, 11, 15 |
| DEC-028 | Guard composes three layers: platform confinement × approval policy × declarative exec rules | Locked | 12, 19 |
| DEC-029 | Subagent model: child session per subagent · per-spawn worktree option · full escaped rules · receipts not transcripts · Core-enforced bounds; review queue is our own build | Locked | 15, 11, 20 |
| DEC-030 | MCP dual-era policy: modern `2026-07-28` first (stateless, `_meta`, `server/discover`), legacy `2025-11-25` fallback; era cached per process/origin; force-legacy escape hatch; façade stateless modern + initialize compat; HTTP+SSE and sessions/sampling/roots/logging are non-goals | Locked | 14, 32 |
| DEC-031 | Work scheduler: three lanes (foreground/background/detached) + Core-enforced outer limits (agents/workers/depth/tokens/spend); interactive priority; pause-and-surface on budget exhaustion | Locked | 11, 15, 20 |
| DEC-032 | Artifact storage & retention: managed per-workspace content-addressed store · immutable versions · receipt-pinned versions never GC'd · explicit promotion | Locked | 29, 25 |
| DEC-033 | Workflow durability: append-only journal · occurrence rows + leases · exactly-once claims · `wake_at` + misfire policy · pinned versions · `needs_attention` for keyless side effects | Locked | 20, 11, 30 |
| DEC-034 | Model-plane normalization contract: one typed stream union above the mapper · usage invariant (inclusive totals + non-overlapping breakdown, never subtract) · cache-class-aware cost with provider-actual override · single-owner layered retries · typed provider-error taxonomy · protocol-scoped prompt-cache hints · catalog-as-data refresh discipline; runtime npm install of provider SDKs rejected | Locked | 18, 14, 16, 30 |
| DEC-035 | Provider client identity & session affinity (gateway class): own User-Agent (never impersonation) + stable per-conversation session header (e.g. `x-opencode-session`) mapped from the logical session id; stable across turns/compaction/restarts; provider traffic policies are conditions of enablement | Locked | 18, 14, 11 |
| DEC-036 | Async subagent lifecycle (completes DEC-029): spawn-returns-immediately · two completion modes (bounded wait vs turn-boundary queue-only wake) · wake-suppression gate (cancelled never wakes) · typed child stream (`spawned/progress/finished` + `will_wake`) · bounded waits auto-background · held-until-closed slots · token-based cancellation (no completion rebuffer) · child reports untrusted + scanned | Locked | 15, 11, 12, 30 |
| DEC-037 | Web search & fetch capabilities: `web.search`/`web.fetch` with provider variants (native · MCP · guarded fetch; browser fallback) · vault keys never in URLs · Guard egress with operator policy overriding model · caps + TTL cache with freshness · citations/provenance first-class · fetched content untrusted (no instruction authority) | Locked | 28, 27, 14, SPEC |
| DEC-038 | Memory sensitivity: one vocabulary (`public`/`personal`/`confidential`), default `personal`; assignment by user action or deterministic monotone floor from source scope/surface; recall ceilings derived from the actor binding per surface, never caller-supplied | Locked | 17, 16, 06, 12, 05 |
| DEC-039 | Memory at-rest protection & erasure semantics (closes PEND-06): whole-DB SQLCipher (vault-held key; WAL/temp covered) · `secure_delete` + WAL checkpoint/TRUNCATE + FTS delete-trigger/rebuild after forget · keyed suppression digest · audit carries no item body · stated threat-model boundary | Locked | 17, 12, 05 |
| DEC-040 | Memory project identity: bind the `project` scope to `DM-024 project_identity`, not raw paths; canonicalization + re-key rules for move/clone/rename/worktree/path-reuse; Windows verification pending | Locked | 17, 25, 21, 06 |
| DEC-041 | Summary ownership: the checkpoint (`DM-006`) is authoritative for work state; memory `summary` items reference checkpoints/sessions via `source_ref` and are never served as work state; no second timeline | Locked | 17, 16, 11 |
| DEC-042 | Memory mutation classification: in-store writes are local persistent mutations (policy-gated + audited, no per-write tickets); export/import/sharing follow the guarded effect path; permitted scopes and ceilings are actor-derived, never caller-supplied | Locked | 17, 07, 12, 05 |
| DEC-043 | External-agent memory boundary: v1 recall-only projection (bound project + own session/task + user preferences); Core never writes an engine's own stores; provider-session transcripts not harvested; subagent child sessions harvested only when Core-owned with parent linkage; An engine's private notes are session-scope items + session log | Locked | 17, 15, 32 |
| DEC-044 | Extraction model & disclosure: session provider default; `confidential` scopes local-only or off until enabled; global extraction budget + global/per-scope kill switches + per-run metering; concurrent sessions bounded | Locked | 17, 18, 11 |
| DEC-045 | Provider-native compaction adoption policy: amends DEC-027's evidence clause only (a verified shipping reference exists — Codex remote compaction v2) and freezes the adoption rules (provider capability event · Guard egress + audit + per-provider off switch · usage via `18` · deterministic checkpoint stays primary); DEC-027 stays Locked with its rules untouched | Locked | 16, 18 |
| DEC-046 | Verification plane stage completion: completes DEC-023's stage list to five — observe → validate → render → verify → reconcile (DEC-023's rules otherwise unchanged) | Locked | 34, 12 |
| DEC-047 | Capability id mapping for adapter-native tools: capability ids stay protocol-neutral; provider adapters own the mapping table (`transport_ref` + native tool name ↔ capability id) at discovery; unmapped native tools are not invocable (guidance, never silent exposure); mapping changes are registry data (epoch-checked), never contract changes | Locked | 13, 14 |
| DEC-048 | MCP dual-era behavior and façade clarifications: persisted force-legacy · read-only era/source projection · HTTP-origin / stdio-command-fingerprint cache · typed stdio probe timeout · lease-less, method-restricted, session-less `initialize` compatibility. Its hand-rolled-core choice is superseded by DEC-061 | Locked; ownership superseded by DEC-061 | 14, 32, 08 |
| DEC-049 | ACP governance class: no ACP-launched agent is ever `Mediated` (all are external processes) — withhold the `fs`/`terminal` client capabilities at `initialize`, answer `session/request_permission` through the one Trust decider, and claim only `SelfContained` (mediated at the ACP boundary; effects inside the agent's own process are outside our audit trail) · `Mediated` stays in the vocabulary only for the post-v1 governed baseline; `NotGoverned` remains the honest answer when an agent neither mediates permissions nor routes effects · the Channel-B availability flag is **derived from the mounted server list, never asserted** | Locked | 32, 12, 15 |
| DEC-050 | Base-envelope actor context carries a permissions **reference**, not a snapshot: `permissions_ref` is resolved by the Trust owner at admission (INV-11), because the envelope crosses a trust boundary and an in-envelope snapshot would be a second, unaudited copy of an authorization decision — clarifies the `10-KERNEL` §7 and `REQ-KERNEL-007` prose to the canonical shape already stated in that section; no envelope field, contract or trust rule changes | Locked | 10, 08, 07, 05 |
| DEC-052 | The first-party native agent is **not** an AgentCowork component: the engine is developed outside this repository and bound afterwards as an ordinary binding — same `AgentEngine`, same Guard, no privileged path (DEC-010's parity rule generalized to every engine and unchanged in force) · `ARCH/15-AGENT-X.md` becomes `ARCH/15-AGENT-PLANE.md` (the agent plane) · `REQ-AGX-001…013` and `TASK-AGX-001…014` are retired (never reused) and the engine-agnostic subset is re-seeded as `REQ-AGENT-001…004` · W4 is retired · INV-12 becomes engine parity | Locked | 01, 15, 05, 08, 09, 03, 02 |
| DEC-053 | Code identifiers are unfrozen and renamed: `everyaios-*` → `agentcowork-*` (crates, packages, scopes, imports, strings) · the data home `~/.everyaios` → `~/.agentcowork` and env vars `EVERYAIOS_*` → `AGENTCOWORK_*` **with a legacy fallback and a one-time migration owned solely by Core**, so existing local data is never orphaned · supersedes DEC-020's identifier freeze | Locked | 01, 10, 12 |
| DEC-054 | Durable Mission above Work; external agents retain native architecture and config; Core governs its own shared effects with truthful provenance; scoped extension activation; heterogeneous teams; independent outcome evaluation; workflow/skill lifecycle | Accepted target, implementation pending | 03, 05–09, 11–17, 20, 29–32, 34–46, AGENTS |
| DEC-055 | Outcome-first, progressively disclosed desktop experience: simple chat and task entry, one contextual Workbench, agent-owned configuration, approachable permissions, explicit proof and recovery; advanced controls remain reachable | Accepted target, implementation pending | 00, 03, 08, 09, 29, 32, 38, 48–50, UI, TODO |
| DEC-056 | Reconcile final Experience with the older UI backlog: typed settings scope and profiles, Core-owned channel health/mediated approvals, explicit session-fork lineage, and a multi-tab Workbench in place of a fixed six-slot launcher | Accepted target, implementation pending | 06–09, 11, 30, 32, 48, TODO |
| DEC-057 | Clarify DEC-003/031/033 for remote continuity: one logical host execution scheduler and one fenced trigger owner per definition; occurrence claims admit one logical run, while external effects still require idempotency or reconciliation | Accepted target, implementation pending | 03, 05–11, 19–20, 40, 43, TODO |
| DEC-058 | Local Machine Observer: separate structural identity from bounded telemetry; standalone service; scoped disclosure/consent; optional one-shot least-privilege helper | Accepted target, implementation pending | 00, 03–09, 12–13, 19, 21, 30, 39, 42–45, 48–51, AGENTS, TODO |
| DEC-059 | Explicit in-app System consent dialog; no install-time monitoring elevation; add bounded same-task candidate comparison as a strategy over Mission/Work with isolated writes, shared criteria, budget and human/evidence-based selection | Accepted target, implementation pending | 00, 04–09, 12, 19, 35–36, 40–42, 45–51, AGENTS, TODO |
| DEC-060 | One capability-selection preference: suitable authorized API/connector/MCP/CLI first, target-specific browser or OS accessibility next, vision after structured paths, raw input last; native agent tools remain agent-owned | Accepted target, implementation pending | 03, 08–09, 13–15, 23–24, 40, 46, 50 |
| DEC-061 | Use the official Rust MCP SDK for protocol semantics behind Core-owned provider, policy, credential and Guard boundaries; inject a Guard-backed HTTP transport; qualify pinned SDK behavior before removing compatibility code | Accepted target, implementation pending | 04, 08–09, 14, 32, 44–45, TODO |
| DEC-062 | Queued chat items resolve the latest conversation binding at dequeue time (agent, advertised model or agent-managed state, mode and access); queue content/references remain durable, but a stale binding snapshot is never silently reused; unavailable or unsupported latest selection pauses with an explicit resolution state | Locked | 08, 11, 15, 18, 30, 48–50, TODO |
| DEC-063 | Clarify universal governance wording in DEC-002/010/022: the Work → Capability → Guard/Ticket/Receipt path governs Core-mediated effects; self-contained agents' native effects remain under their policy with distinct reported/observed provenance | Locked clarification under DEC-054 | 02–05, 07, 12, 15, 22, 29, 34, 46, SPEC |
| DEC-064 | Built-in host skill catalog is agent-neutral and lazy-loaded; resolve user/project/session/Work scopes without changing native agent stores; normalize artifact/file outputs into safe clickable, exact-version Workbench references and render agent activity in approachable host UI | Accepted target, implementation pending | 06–09, 15–16, 29–32, 37–38, 45–50, SPEC, TODO |
| DEC-065 | Active-work visualization is a read-only projection over Mission/Work/events/artifacts; semantic activity is evidence-backed; domain anchors are ephemeral; continuous surface streaming is a separate, explicitly authorized capability and deferred pending design/qualification | Accepted target; stream deferred | 03–07, 11, 13–15, 19, 21–24, 29–30, 34–36, 38, 40–42, 48–50, TODO |
| DEC-066 | Live Desk is a welcoming, animated, evidence-backed view: purposeful state transitions explain real work without implying unobserved actions; provide static/reduced-motion controls and measure nontechnical user comprehension and appeal | Accepted target, implementation pending | 00, 03–05, 08–09, 38, 40–42, 47–50, TODO |
| DEC-067 | One product-neutral Shared Extension Market publishes a read-only, versioned metadata catalog; HorizonCode consumes it first and AgentCowork later, while installs, accounts, credentials, grants and runtimes remain product-local | Proposed | 00, 03–05, 08–09, 12–14, 31, 42, 45–50, SPEC, TODO |

## 2. Details

### DEC-001 — Product frame
AgentCowork is an AI-native execution environment layered on the user's existing computer: a universal Work + Capability runtime composing interchangeable agents, models, providers and environments behind one governed execution model. It is not an OS/kernel/bootloader replacement and never markets itself as one.
**Evidence:** product-owner brief (2026-09-26).

### DEC-002 — One governed execution path
Every externally visible effect follows exactly one path: `Work → Capability → Provider → Handle → Guard → Ticket → Execute → Effect → Verify → Receipt → Event`. Control path: bounded, synchronous (targets p50 < 2 ms · p95 < 10 ms · p99 < 25 ms). Effect path: asynchronous, observable, unbounded. No bypasses — not for domains, adapters, UI, or the native agent.
**Evidence:** owner brief; `ARCH/03-HLD.md §5`.

### DEC-003 — Work as the universal abstraction
Chat turns, workflow runs, background jobs and subagent tasks all materialize as `Work` items with one lifecycle, checkpoints, cancellation and receipts.
**Affects:** `ARCH/11-WORK.md`.

### DEC-004 — Three-way independence
`Agent ≠ Model ≠ Provider` and `Capability ≠ Provider`: an agent asks the router for a model; a capability resolves to whatever provider implements it. Nothing is hard-coded to a vendor.
**Affects:** `13`, `14`, `15`, `18`.

### DEC-005 — Protocols are adapters
MCP, ACP, CLI, HTTP and plugins live only in provider/channel adapters. Nothing above the Capability Plane knows which transport executed an operation.
**Affects:** `14`, `32`.

### DEC-006 — Domain runtimes own complexity
Office/Browser/Computer/Files/Code/Search/Comms own their specialized execution; the kernel never reimplements domain logic and domains never govern themselves.
**Affects:** `10`, `22`–`28`.

### DEC-007 — Context split
Context infrastructure (store/query/snapshot/checkpoint/projection) is Core's; context control (selection/ranking/budget/prune/compact/rebuild) is the agent's; external agents receive a scoped context projection. "Core answers what exists; the agent answers what the model sees now."
**Affects:** `ARCH/16-CONTEXT.md`.

### DEC-008 — Workflow engine placement
The Workflow Engine is Core infrastructure, a peer of the Agent Runtime — not a feature inside any engine. Workflows call agents; agents author and invoke workflows (workflows-as-tools). Deterministic vs adaptive is an explicit distinction.
**Affects:** `ARCH/20-WORKFLOW.md`.

### DEC-009 — External agents get projections
External agents (ACP/A2A/API/CLI) connect through the Agent Gateway and receive exactly: identity contract, capability projection, context projection, workspace projection (allowed/read-only paths), filtered tool set, artifact gateway, filtered event stream. No Core internals, no engine internals.
**Affects:** `12`, `16`, `32`.

### DEC-010 — Native agent parity
Every engine implements the same `AgentEngine` contract and passes the same Guard. Any proposal to give one a shortcut (direct store access, bypassed tickets, internal hooks) is rejected unless a superseding DEC records the full trade-off.
**Affects:** `15`, `32`.

### DEC-011 — World Model + ladder
A continuously-updated World Model (apps, windows, browser, files, processes, devices, relationships) is first-class; computer use follows the deterministic ladder (native API → structured UI → DOM/AX → CLI/MCP → vision → raw input). Vision is the fallback, not the default.
**Affects:** `21`, `24`.

### DEC-012 — Browser runtime
Default is AgentCowork-managed Chromium (isolated profile, predictable automation). Google Chrome / Edge / Firefox / Opera / system browser are selectable adapters, not parallel embedded runtimes.
**Affects:** `ARCH/23-BROWSER.md`.

### DEC-013 — Office runtime
Office is a capability runtime underneath a universal document surface — not a sidebar mode. Progressive L1 (semantic read) → L2 (structured mutation) → L3 (raw escape hatch); documents stay resident for active editing sessions.
**Affects:** `ARCH/22-OFFICE.md`.

### DEC-014 — Artifacts vs Library
Artifacts are work products scoped to session/run/workflow/project; Library is global reusable inventory (agents, skills, workflows, connectors, plugins, templates, prompts, saved artifacts). Promotion is explicit ("Save to Library"), never automatic.
**Affects:** `ARCH/29-ARTIFACTS.md`.

### DEC-015 — Token discipline
Rendering, navigation, listing, opening, previewing, index search and deterministic user-triggered operations never consume model tokens. The OS renders; the model reasons.
**Affects:** `02`, `16`, `34`.

### DEC-016 — No evasion tooling
No CAPTCHA solving, anti-bot evasion, fingerprint spoofing, or residential-proxy infrastructure in browser/computer-use capabilities. Capability success must not depend on evasion.
**Affects:** `23`, `24`.

### DEC-017 — Absorb strategy
Three levels: integrate directly (only with cleared licensing), reimplement the primitive (study → redesign around our contracts), use as external provider. Default posture: study → model → redesign → implement independently. The licensing ledger (`44`) must record each reuse decision.
**Affects:** `ARCH/44-ABSORB-REGISTER.md`.

### DEC-018 — Memory v1 store
One SQLite file + FTS5 (`memory_items`, `memory_fts`, `memory_suppressions`, `memory_jobs`); one bounded extractor with verbs `ADD | SUPERSEDE | NONE` (single LLM call, off the hot path); explicit `superseded_by` pointer for contradiction; permanent forget via content-hash suppression. Vectors, graph, decay/activation math and autonomous consolidation are deferred behind measured triggers (U0–U11).
**Evidence:** `ARCHIVE/v1-research/memory.md` §0, §2.7, §4; anchors: mem0 ADD-only (`clone2/mem0/mem0/memory/main.py:879-1195`), TEPA revocation (`https://arxiv.org/abs/2608.07429`), STALE (`https://arxiv.org/abs/2605.06527`).

### DEC-019 — Memory ≠ context
Memory is the durable scoped store; context is a per-turn selection under budget. The interface is `recall(query, scopes, budget) → candidates`; the Context Controller decides inclusion. Injection is non-touching (never bumps counters/salience) and budgets are maxima (zero relevant hits ⇒ zero injected tokens).
**Evidence:** `ARCHIVE/v1-research/memory.md` §4.1; NOOA non-touching read (`clone2/nooa/packages/nooa-memory/src/nooa_memory/schema.py:315-331`); claude-mem whole-item budget degradation (`clone2/claude-mem/src/services/context/ContextBudget.ts:4-40`).

### DEC-020 — Working names
**Superseded in two parts (2026-09-27):** DEC-052 retires the third working name, DEC-053 lifts the identifier freeze recorded below. The reasoning this entry records — names are provisional and live in one layer, the layer is centralized so a rename is mechanical — is unchanged and is what made both changes cheap.

AgentCowork (product) and Core (runtime) are the working names for v1 (DEC-052 retires the third name); the rename map and rules live in `ARCH/01-NAMING.md`. Code identifiers (`everyaios-*`, `EveryAIOS` strings) stay frozen until a post-freeze code-phase rename.
**Status note:** Provisional — branding may change; the architecture must not depend on the names.

### DEC-021 — Approval primitive
Human-in-the-loop decisions are first-class: agents can request approval; workflows have approval nodes (`approve/reject/edit/provide-data`). One primitive, recorded in events and receipts, routed through Trust.
**Affects:** `12`, `20`.

### DEC-022 — Receipts + risk-proportional verification
Every externally visible effect produces a durable receipt; before a receipt is issued, verification runs at a depth determined by the capability's risk class. "Implemented but unverified" cannot masquerade as complete.
**Affects:** `29`, `34`.

### DEC-023 — Verification plane
Validate → render → verify → reconcile is a plane, not an afterthought: deterministic validators per domain (Office, files, browser state), optional visual/render inspection, and reconciliation against the intended effect.
**Affects:** `ARCH/34-EFFECT-VERIFICATION.md`.

### DEC-024 — Scoping model
Installed / Available / Activated / Executing, with scopes Global → Workspace → Agent → Session → Run. Resources are never duplicated per agent; only activation/execution are narrow.
**Affects:** `03`, `13`, `31`.

### DEC-025 — Native-first resolution
When an agent has a native way to do something (its own tools, shell, editor), the agent uses it; AgentCowork augments when the native capability is absent or worse on quality/cost/permission/latency. The platform never removes or duplicates an external agent's native tools.
**Affects:** `13`, `15`.

### DEC-026 — Rebuild process
v1 is written from scratch; v0 is archived locally (`ARCHIVE/v0/`) and is reference-only; docs are built in passes (P0–P6) gated by the viability checklist in `00-INDEX`; external claims require primary evidence; `TODO.md` is exempt.
**Note (2026-09-26):** the pass series later extended to P7 (SDD layer), P8 (freeze) and P9 (verification pass) — “P0–P6” above records the original gating series; see `ARCH/00-INDEX.md` §4.
**Affects:** `ARCH/00-INDEX.md`.

### DEC-027 — Context budget & compaction discipline
Named budget vocabulary (`keep` ≈ 8k retained recent tokens; `buffer`/`reserve` ≈ 20k safety margin; summary output reserve), a pre-turn feasibility check (resolved window × effective percent), mandatory overflow recovery (compact-after-overflow → retry the **same step**), and compaction as a **projection boundary over a durable log**: the full session event log is never rewritten; a checkpoint segment is rendered as historical context. Tool-output pruning is a separate, opt-in transform that never touches log truth. A provider-native compaction path is ours to design — the verified shipping set has none (OpenCode summarizes with the model in both generations).
**Evidence:** `agent-harness-verification.md` §C1–C2, §E2; anchors `clone2/opencode/packages/core/src/session/compaction.ts:12-15, 178, 232-243` · `to-llm-message.ts:152-162` · `history.ts:13-80` · `clone2/codex/codex-rs/core/src/session/mod.rs:4560-4587`.
**Affects:** `16-CONTEXT`, `11-WORK`, `15-AGENT-PLANE`.

### DEC-028 — Guard as three layers
Permission enforcement composes three distinct layers and never collapses them into one enum: (1) **platform confinement** (sandbox policy — OS-level bounds per platform); (2) **approval policy** (when a human is asked; policy enum + granular per-category config); (3) **declarative exec rules** (pre-authorized command/prefix/network patterns). Guard turns the composition into ALLOW/ASK/DENY; tickets encode the outcome; protected subpaths (e.g. VCS hooks) stay read-only inside writable roots.
**Evidence:** `agent-harness-verification.md` §A4; anchors `clone2/codex/codex-rs/protocol/src/protocol.rs:969-1125` · `sandbox.rs:10-16` · `execpolicy/src/`.
**Affects:** `12-TRUST`, `19-RUNTIME-ENVIRONMENTS`.

### DEC-029 — Subagent model
One child session per subagent (own context/toolset/persona), full escaped project rules delivered to children; `fork_context` is a per-spawn option (default fresh + bounded snapshot); worktree isolation is a per-spawn option, with write leases for overlapping files; parents receive **worker receipts**, never transcripts; the platform enforces outer bounds (parallel/total/depth/tokens/spend) while the running agent decides within them. A "review queue" is our own product-layer feature — not borrowed (Codex source contains no queue).
**Evidence:** `agent-harness-verification.md` §A3, §B3, §E7; anchors `clone2/grok-build/crates/codegen/xai-grok-shell/src/agent/subagent/spawn.rs:1-33` · `host_service.rs:500-503, 566-578` · `prompt/context.rs:152,196` · `clone2/codex/codex-rs/core/src/tools/handlers/multi_agents_spec.rs:14-16, 726-737`.
**Affects:** `15-AGENT-PLANE`, `11-WORK`, `20-WORKFLOW`.

### DEC-030 — MCP dual-era policy
Client side: detect and negotiate per transport — **stdio** probes `server/discover` (10 s cap) then falls back to legacy `initialize`; **HTTP** classifies the `400` body; the negotiated era is cached per process/origin; a per-server force-legacy escape hatch exists. Implement on `rmcp` 3.4.x (verified to carry both `2026-07-28` and `2025-11-25`). Server façade (our own MCP surface): stateless modern + `initialize` compatibility, with the mandatory `server/discover` method and `Mcp-Method`/`Mcp-Name` validation. **Non-goals:** HTTP+SSE transport, sessions/resumability, sampling, roots, logging.
**Corrections on record:** HTTP+SSE has been deprecated since `2025-03-26` (~18 months; removal clock = SEP-2596, Final 2026-05-18 + 3 months ⇒ eligible ≈2026-08-18, not yet removed) — the earlier “≥12 months” framing was wrong. Code-phase fixes identified: the existing in-repo remote client sends no `_meta`/modern headers; `server/discover` is absent from the current façade.
**Evidence at original decision:** `ARCHIVE/v1-research/mcp-provider-verification.md` (641 lines) — spec changelog `2026-07-28`; versioning/transports/deprecated pages; `clone2/grok-build/crates/codegen/xai-grok-mcp/src/servers.rs:3782-3910`; `clone2/codex/codex-rs/rmcp-client/src/protocol_mode.rs:9-51`; the then-reviewed `rmcp` source; SEP-2596. The SDK decision is superseded by DEC-061, which records the exact current upstream pin and transport seam.
**Affects:** `14-PROVIDERS`, `32-CHANNELS`.

### DEC-031 — Work scheduler lanes + outer limits
Three lanes — **foreground** (the active interactive turn; 1/session) · **background** (jobs/workers admitted without blocking the UI) · **detached** (long work that may outlive the app session; rehydrated on start) — with Core-enforced outer bounds the running agent cannot exceed: max simultaneous agents · max total workers per work tree · max depth · max worker tokens · max session spend · per-lane concurrency. Interactive > background priority; starvation guard; queue-depth backpressure; parent→child cancellation; budget exhaustion pauses and surfaces (no silent overrun).
**Evidence:** product-owner brief (lanes, limits; “background work is essential”); `ARCH/11-WORK.md` §3; `agent-harness-verification.md` §A3 (background guidance), §E7 (bounds as per-spawn policy).
**Affects:** `11-WORK`, `15-AGENT-PLANE`, `20-WORKFLOW`.

### DEC-032 — Artifact storage & retention
Artifacts live in a **managed per-workspace store** with content-addressed immutable versions; workspace-file artifacts are referenced by identity (`25`) plus a managed copy when they must survive edits. **Receipt-pinned versions are never garbage-collected** — chain integrity is never traded for storage. Unreferenced versions are pruned by age/count policy; all deletions are audited. External-agent exchange uses artifact refs through the gateway (working scheme token `eaios://artifact/<id>`; the final scheme renames with the brand — OQ-003 tie).
**Evidence:** product-owner brief (`Artifact`/`LibraryItem` schemas, “Save to Library”); `ARCH/06-DATA-MODEL.md` DM-019/020/023; `ARCH/29-ARTIFACTS.md` §6.
**Affects:** `29-ARTIFACTS`, `25-FILES`, `32-CHANNELS`.

### DEC-033 — Workflow durability model
State lives in a single append-only journal (SQLite WAL): pinned workflow version + digest · trigger rows (`next_due_at`, misfire policy) · materialized **occurrence rows** with unique idempotency keys · run rows with leases/heartbeats/`cancel_requested` · step-attempt rows with idempotency keys · effect intents/receipts · wait rows (`wake_at`) · approval rows · event log. One scheduler loop: reconcile leases → materialize occurrences → claim exactly-once → execute step-by-step → compute the nearest wake. Resume: settled steps reuse results; unsettled idempotent steps retry with the **same** key; **keyless side effects land in `needs_attention`** — completion is never fabricated. In-flight runs keep their pinned version; new triggers take the latest published. Desktop misfire default: Skip + record (bounded grace).
**Evidence:** `ARCHIVE/v1-research/workflow-engine-verification.md` §0/§4 — n8n durable scheduler; Temporal timers/versioning; OpenWork `types/src/automations.ts:346-371`; Grok Build `occurrence_journal.rs:1-12`; DeepSeek README:128 (un-journaled falsifier).
**Affects:** `20-WORKFLOW`, `11-WORK`, `30-EVENTS`.

### DEC-034 — Model-plane normalization contract
Provider dialects are confined to adapters/mappers; everything above speaks **one typed stream-event union** (block start/delta/end for text/reasoning/tool input; explicit ids; step-finish vs turn-finish distinct; provider-error event; raw-payload escape hatch). Usage: **inclusive totals + non-overlapping breakdown with a written invariant and clamping — consumers never subtract** (underflow-class fix). Cost: cache-class-aware; provider-reported actual overrides estimates; included plans are exactly 0. Retries: exactly one owner per failure class (request-start transport · pre-content buffer-until-proven with usage aggregation · post-content turn-level); user aborts never retried; overflow terminal; watchdogs (header/chunk/read) with explicit abort reasons. Errors: typed taxonomy with `retryable` derived. Prompt cache: protocol-scoped hint policy (auto breakpoints only for inline-marker protocols). Catalog: data, not code — TTL + atomic write + lock + vendored snapshot + scheduled refresh.
**Rejected:** runtime npm installation of provider SDKs; vendored date-gated heuristic blobs as code; prompt capture to disk.
**Evidence:** `ARCHIVE/v1-research/provider-layer-absorption.md` (OpenCode `fe3f3a4` MIT · Cline `254f40c` Apache-2.0; absorb A1–A14, gaps G1–G16, rejects R1–R5).
**Affects:** `18-MODEL-ROUTING`, `14-PROVIDERS`, `16-CONTEXT`, `30-EVENTS`.

### DEC-035 — Provider client identity & session affinity
When a provider requires client identification and session affinity (the OpenCode Go class), adapters MUST: (1) send our **own** identifying User-Agent (`agentcowork-agentx/<version>`-style) — never impersonate another client or a generic SDK/HTTP-library name; (2) send a **stable per-conversation session id** in the provider's session header (e.g. `x-opencode-session`), mapped from the logical session id (`DM-004`). Stability rules: one value per conversation — unchanged across turns, compaction and app restarts; a new conversation ⇒ a new value; each subagent session is its own conversation. (3) comply with provider traffic policies (typical coding-agent traffic; monitored for abuse) — accepting a provider's terms is a condition of enabling that provider (`12`/`44`), and evasion/impersonation is never a design goal.
**Evidence:** `opencode.ai/docs/go` §“Where can I use it?” (fetched 2026-09-26 — own user agent + `x-opencode-session` + monitored traffic; validated/problematic client lists) · clone `clone2/opencode` — per-provider UA pattern (`packages/opencode/src/provider/provider.ts:631,764,834`), `opencode-go` provider id (`packages/opencode/src/tool/registry.ts:61`), Go docs (`packages/web/src/content/docs/*/go.mdx`).
**Affects:** `18-MODEL-ROUTING`, `14-PROVIDERS`, `11-WORK`.

### DEC-036 — Async subagent lifecycle (completes DEC-029)
Completion delivery has exactly two modes: a **bounded foreground wait** (declared tiers, used sparingly) or a **turn-boundary queue-only wake** (the result is admitted at `next-turn`/`next-step`, never mid-step). A **wake-suppression gate** decides relevance (`backgrounded && !cancelled && wake_enabled && !block_waited && !explicitly_killed && !goal_loop_active && parent_channel_open`); a cancelled child never wakes the parent and never re-buffers a completion after teardown. The child stream is typed: `spawned` (before the first prompt dispatch) · `progress` (≈2 s) · `finished {will_wake}`. Waits that exceed budget **auto-background** instead of freezing the parent. Concurrency slots are **held until closed**; admission is queue-on-limit with a `fail` opt-in; defaults/depth are declared and enforced by `11` (DEC-031). Cancellation is cooperative and token-based with parent-prompt/teardown/close cascades. **Child receipts are untrusted data** — scanned for instruction-shaped patterns and delivered under a no-authority header; background completion notices are automated events.
**Evidence:** `ARCHIVE/v1-research/async-subagents-websearch-absorption.md` §0–§2 — Claude Code subagent docs; Codex `trigger_turn:false` (`completion.rs:98-129`); Grok wake gate (`spawn.rs:456-472`) + typed notifications (`notification.rs:723-830`); Cline continuation gate; OpenCode `<task>` inject; Aider verdict (no upstream subagents).
**Affects:** `15-AGENT-PLANE`, `11-WORK`, `12-TRUST`, `30-EVENTS`.

### DEC-037 — Web search & fetch capabilities
`web.search` and `web.fetch` are capabilities (never the local search plane). Provider variants: native/server-side search · MCP search server · guarded local fetch; browser (`23`) as the fallback for rendered pages. Rules: credentials only via the vault and **never in URLs** (INV-02); egress via Guard with domain allow/block policy that **overrides model requests** (INV-05); SSRF floor (no localhost/no-dot/private/link-local/metadata; resolve-then-check); caps (fetch 5 MB · search response 256 KiB · default 8 results/hard max 20 · synthesis ≤10k chars · per-session search budget counted across subagents); fetch TTL cache (default 15 min) keyed `(normalized URL, format)` with an explicit `fresh` bypass; citations/provenance first-class (`{ref, url, title?, retrieved_at, sha256?}`) and never merged into prose without the source ref; **fetched content is untrusted input** — no instruction authority, URL-provenance option, cross-host redirects surfaced, robots/ToS honored, no evasion tooling (DEC-016).
**Evidence:** `ARCHIVE/v1-research/async-subagents-websearch-absorption.md` §3–§4 — Claude Code WebSearch/WebFetch docs + Anthropic server-tool docs; OpenCode `mcp-websearch.ts`/`webfetch.ts`; Cline `web-fetch.ts`; Grok Build `resolve_filters`.
**Affects:** `28-COMMS`, `27-SEARCH`, `14-PROVIDERS`, SPEC §8.

### DEC-038 — Memory sensitivity assignment, vocabulary and ceilings
One canonical vocabulary for memory and context — `public | personal | confidential`, default `personal` (`06` §0 wins on naming). Assignment is deterministic where it must be: user actions may raise a class; the extractor may propose one; an item is **never less sensitive than its source scope/surface** (monotone floor). Recall ceilings are derived from the actor binding per surface — desktop user (owner, all classes) · external-agent projection (`personal` and below; `confidential` only with a recorded loadout; v1 default none) · UI inspect (owner-visible) · exports (classes marked). Caller-supplied scope or ceiling widening is rejected by construction. This makes INV-10 testable and removes the second vocabulary (`normal | sensitive`) from memory/context paths.
**Evidence:** `ARCHIVE/v1-research/v1-sdd/memory-agent-deep-dive.md` §1.4/§6.1 (finding F-01; contradiction C-01); `ARCH/17-MEMORY.md` §5.3/§7/§9.
**Status note:** Locked — vocabulary and rules fixed; per-surface ceiling defaults are product knobs.
**Affects:** `17-MEMORY`, `16-CONTEXT`, `06-DATA-MODEL`, `12-TRUST`, `05-INVARIANTS` (INV-10).

### DEC-039 — Memory at-rest protection & erasure semantics
Closes PEND-06. The memory store is **whole-DB encrypted at rest** (SQLCipher via the same bundled stack as the vault; the key never rests outside the vault; WAL/journal/temp files inherit the encryption). Forget remains a hard delete, and the erasure claim carries an explicit policy and threat model: `secure_delete` on; WAL checkpoint/TRUNCATE after forget; FTS rows removed through the external-content delete trigger (rebuild for repair); **suppression digests are keyed** (store-local key), not plain content hashes, so low-entropy content is not dictionary-correlatable; audit records carry no item body. The claim explicitly does **not** cover OS caches, backups/snapshots, or flash wear-leveling — “permanent” means no path inside Core can resurrect the item, not physical media sanitization.
**Evidence:** `ARCHIVE/v1-research/v1-sdd/memory-agent-deep-dive.md` §3.4/§6.1 (findings F-04/F-23; probes §7.1); `ARCH/04-DECISIONS.md` §3 PEND-06; `ARCH/17-MEMORY.md` §7/§9.
**Status note:** Locked — policy fixed; encryption and forensic verification are code-phase.
**Affects:** `17-MEMORY`, `12-TRUST`, `05-INVARIANTS`.

### DEC-040 — Memory project identity & re-keying
The memory `project` scope is keyed by the stable project identity (`DM-024 project_identity`), never a raw path: canonical root + git remote as identity attributes; Windows canonicalization follows the file-identity model (`21` §3) — case-insensitive comparison, junction/short-name/long-path/UNC normalization; POSIX symlinks resolved once and recorded. Re-key rules: move/rename keeps identity when the identity attributes survive (remote match), otherwise the item set is surfaced with explicit re-point guidance; a clone gets a new identity unless the user explicitly adopts the source mapping (recorded); worktrees share the parent identity; a new project at a reused path never inherits memory automatically. Recall can only return identities in the actor's permitted set.
**Evidence:** `ARCHIVE/v1-research/v1-sdd/memory-agent-deep-dive.md` §6.1 (findings F-09/E-06/E-12); `ARCH/25-FILES.md` §5 (shared with OQ-FILES-01); `ARCH/21-WORLD-MODEL.md` §3.
**Status note:** Locked — Windows canonicalization matrix pending a Windows acceptance record.
**Affects:** `17-MEMORY`, `25-FILES`, `21-WORLD-MODEL`, `06-DATA-MODEL`.

### DEC-041 — Summary ownership (checkpoint vs memory)
The checkpoint (`DM-006`) is the authoritative work-state projection for long-horizon steering; memory `summary` items are durable *knowledge* roll-ups that reference their checkpoint/session through `source_ref` and are never served as work state. A live checkpoint supersedes a stale memory summary in assembly; deleting a checkpoint annotates its summaries, never promotes them. No second timeline exists (`16` §4, `11` §2).
**Evidence:** `ARCHIVE/v1-research/v1-sdd/memory-agent-deep-dive.md` §2.3/§6.2 (finding F-10; contradiction C-05); `ARCH/17-MEMORY.md` §2.1/§2.3; `ARCH/16-CONTEXT.md` §4.
**Status note:** Locked.
**Affects:** `17-MEMORY`, `16-CONTEXT`, `11-WORK`.

### DEC-042 — Memory mutation classification & actor-derived scopes
In-store memory writes (extraction, `remember`, forget/supersede/pin/edit, scope wipe) are **local persistent mutations**: policy-gated per scope and audited, with **no per-write ticket** — a background extractor must be able to run without holding effect tickets. Boundary-crossing operations (export/import to disk, sharing, anything leaving the machine) follow the guarded effect path (pathfloor/egress/tickets as applicable). The permitted scope set and sensitivity ceiling are derived by the service from the actor binding; caller parameters may only narrow — a caller cannot select another project's scope (confused-deputy prevention). Every mutation is audited with no item body in the record.
**Evidence:** `ARCHIVE/v1-research/v1-sdd/memory-agent-deep-dive.md` §6.1 (finding F-27; contradictions C-04/E-18); `ARCH/07-CONTRACTS.md` §0/§5; `ARCH/12-TRUST.md` §8/§9; `ARCH/05-INVARIANTS.md` INV-01/04/24.
**Status note:** Locked.
**Affects:** `17-MEMORY`, `07-CONTRACTS`, `12-TRUST`, `05-INVARIANTS`.

### DEC-043 — External-agent memory boundary
v1 memory exposure to external agents is **read-only filtered recall** — bound project + own session/task + user preferences; no org, no other projects, `confidential` only with a recorded loadout (v1 default: none). Core never writes or mutates an external agent's native memory/config/session stores, and provider-session transcripts we do not own are never harvested; imports from native stores stay deferred (U8), explicit, read-only to the source, and audited. Subagent child sessions are Core-owned: they may be harvested at their own settle boundaries with parent linkage (`source_ref`), never auto-promoted, and receipts enter extraction only as untrusted data. An engine's “private working notes” are session-scope memory items + the session log — there is no second durable store.
**Evidence:** `ARCHIVE/v1-research/v1-sdd/memory-agent-deep-dive.md` §4/§6.1 (findings F-13/F-14; contradiction C-08); `ARCH/15-AGENT-PLANE.md` §5/§7; `ARCH/32-CHANNELS.md` §3; `ARCH/41-EDGE-CASES.md` EDGE-150/151.
**Status note:** Locked.
**Affects:** `17-MEMORY`, `15-AGENT-PLANE`, `32-CHANNELS`.

### DEC-044 — Extraction model & disclosure policy
Background extraction defaults to the session's active provider (no *new* disclosure). `confidential` scopes extract **local-only or not at all** until explicitly enabled by the user. Extraction runs against a declared **global budget** (calls/tokens per period) with global and per-scope **kill switches** and per-run metering (`memory.extraction.run`); concurrent sessions cannot exceed the budget. This closes the extraction-disclosure item (`OQ-MEM-03`) and bounds cost/DoS exposure.
**Evidence:** `ARCHIVE/v1-research/v1-sdd/memory-agent-deep-dive.md` §6.1 (finding F-15); `ARCH/17-MEMORY.md` §5.1/§9; `ARCH/04-DECISIONS.md` DEC-031 (outer limits).
**Status note:** Locked — default model policy fixed; budget constants are product knobs.
**Affects:** `17-MEMORY`, `18-MODEL-ROUTING`, `11-WORK`.

### DEC-045 — Provider-native compaction adoption policy
**Correction on record:** DEC-027's evidence clause ("the verified shipping set has none") is factually stale. A verified shipping reference exists: Codex remote compaction v2 — `run_remote_compaction_request_v2` (`codex-rs/core/src/compact_remote_v2.rs:387-412`), a `ContextCompactionItem` protocol request item (`codex-rs/protocol/src/items.rs:509-517`), a `compaction_output` response (`codex-rs/core/src/compact_remote_v2_attempt.rs:23-24, 104-129`), with analytics distinguishing `CompactionImplementation::{Responses, ResponsesCompactionV2}` (`codex-rs/analytics/src/facts.rs:460-463`). **DEC-027's rules are untouched** — named terms, pre-turn feasibility, one overflow recovery, the projection boundary over a durable log, and pruning-as-a-separate-transform all stand; this decision corrects the evidence and freezes the policy if a provider-native path is adopted.
**Supersession scope (explicit):** this decision amends DEC-027's **evidence clause only**. DEC-027 remains Locked; its behavioural rules — named budget terms, pre-turn feasibility, one overflow recovery, the projection boundary over a durable log, and pruning as a separate transform — are not superseded or amended, and this decision is the correction on record wherever that evidence clause is read.

**Adoption rules (frozen):** a provider-native compaction path is a **provider capability event**, never an implementation detail — conversation egress goes through Guard with provider allowlisting, audit and a per-provider off switch (INV-02/INV-05); its usage/cost rolls into `18` telemetry as a second inference call; the deterministic checkpoint remains the primary reconstructable state and provider output is never the sole checkpoint. Adoption is decided per provider behind capability detection (OQ-CTX-01).
**Status note:** Locked — the correction and the adoption rules are fixed; which providers ship the path remains open (`OQ-CTX-01`). Proposed 2026-09-26 (P7 pass 15); DEC-027's text is not amended.
**Evidence:** `ARCHIVE/v1-research/v1-sdd/agentx-opencode-harness-notes.md` §5 (correction 1 — re-verified at the pinned Codex HEAD) · `ARCHIVE/v1-research/v1-sdd/agentx-finalization-draft.md` §4.2.
**Affects:** `16-CONTEXT`, `18-MODEL-ROUTING`.

### DEC-046 — Verification plane stage completion
**Completion on record:** DEC-023 summarizes the verification plane as "validate → render → verify → reconcile"; `ARCH/34-EFFECT-VERIFICATION.md` §2 and `REQ-VERIFY-003` define the pipeline as five stages — **observe → validate → render → verify → reconcile** — where observation (capturing the actual effect: render, screenshot, state read) precedes validation. This decision completes the stage list; DEC-023's rules are otherwise unchanged and remain Locked. The DEC-023 register row's three-stage short form (validate / render / reconcile) is superseded by this five-stage list.
**Status note:** Locked (promoted at the re-freeze, 2026-09-26).
**Evidence:** `ARCH/34-EFFECT-VERIFICATION.md` §2/§7 · `ARCH/08-REQUIREMENTS.md` REQ-VERIFY-003 · P9 verification pass (2026-09-26).
**Affects:** `34-EFFECT-VERIFICATION`, `12-TRUST`.

### DEC-047 — Capability id mapping for adapter-native tools

**Completion on record (P9, 2026-09-26):** MCP and adapter tool names are protocol-shaped while capability ids must stay protocol-neutral (`ARCH/13-CAPABILITY.md` §1 rule 1; `ARCH/14-PROVIDERS.md` §2 rule 2). `13` §8 and `14` §5 left the mapping owner unstated; this names it: the provider adapter owns the mapping table (`transport_ref` + native tool name ↔ capability id) built at discovery. Unmapped native tools are not invocable — they surface as guidance, never silent exposure. Mapping changes are registry data (epoch-checked — `DM-012`, `13` §4), never contract changes.

**Status note:** Locked (promoted at the re-freeze, 2026-09-26).

**Evidence:** `ARCH/13-CAPABILITY.md` §1/§8 · `ARCH/14-PROVIDERS.md` §2/§5/§7 · `ARCH/08-REQUIREMENTS.md` REQ-PROV-007 · P9 verification pass (2026-09-26).

**Affects:** `13-CAPABILITY`, `14-PROVIDERS`.

### DEC-048 — MCP client core ownership (decision 1 superseded by DEC-061) + three DEC-030 clarifications

**Decision 1 (historical; superseded by DEC-061) — the client core stays hand-rolled, dual-era and patch-owned; `rmcp` is not adopted.** At this decision's date, the in-crate client (`crates/agentcowork-mcp/src/remote.rs`; formerly `everyaios-mcp` before DEC-053) was treated as the protocol core, and no Guard-compatible SDK transport seam had been established. DEC-061 records the later official-source finding that `rmcp-v3.4.0` provides a custom HTTP client seam and replaces this ownership decision. This paragraph remains as decision history only; it is not current implementation guidance. The two reasons and the “transport-seam milestone” are therefore closed by that superseding decision, not active objections.

**Clarification A — the force-legacy hatch is persisted state, and the era is a read-only projection.** The hatch is a field on the **stored server record** (`StoreEntry.force_legacy`), never a per-call argument: it changes the wire contract that server is spoken in, so a runtime-only flag would be lost on restart and silently re-pin the origin to a wrong era. The **effective era plus its source** (`forced` · `cached` · `probed` · `default`) is a **read-only projection** surfaced to status surfaces; reading it **never probes the network** (a read that issued a probe would turn inspection into a network round trip).

**Clarification B — era caching is keyed per origin on HTTP and per command fingerprint on stdio; a stdio probe timeout fails the attach.** DEC-030 says "per process/origin"; an *origin* (`scheme://authority`) is meaningless for a spawned child, so stdio verdicts are cached under a **command fingerprint** (the resolved command line) and re-probed when the command is edited. The era probe carries a **10 s budget on both transports**, defined once so the two cannot drift. A stdio probe that **times out fails the attach with a typed error** rather than falling through to the legacy path: an unanswered probe leaves the ND-JSON stream desynchronised, so a late `server/discover` reply would be read as the `initialize` reply and the reconciled tool names would belong to the wrong request. A `-32601` refusal is a *reply*, not a timeout, so the legacy fallback still runs.

**Clarification C — the façade's `initialize` compatibility is a separate lease-less, method-restricted path.** Modern methods require the strict lease that pins the negotiated revision. The compatibility path accepts only `initialize`; it is read-only, creates no session, grants no capability handle, and dispatches nothing. It is a separate admission function, not an exception inside the modern lease gate, so it cannot weaken that gate (INV-03). **If `initialize` ever becomes session-bearing or gains effectful behavior, remove this lease-less path or revisit its authorization contract** — that condition is the whole safety argument.

**Amendment scope (explicit):** DEC-030 stays **Locked** and its text is not amended; clarifications A–C are the correction on record wherever DEC-030's force-legacy hatch, era-caching and `initialize`-compatibility clauses are read. DEC-030's implementation clause ("implement on `rmcp` 3.4.x") is reaffirmed and qualified by DEC-061's exact initial pin and Guard-backed custom transport. DEC-030's non-goals (HTTP+SSE · sessions/resumability · sampling · roots · logging) and its two code-phase corrections are unchanged (see `14` §4).

**Status note:** Locked as historical decision record. Clarifications A–C remain in force. Decision 1 and its no-SDK ownership choice are superseded by DEC-061; `OQ-PRV-1` was reopened and resolved by that accepted target.

**Evidence:** `ARCHIVE/v1-research/v1-sdd/mcp-engine-opencode.md` §10.1–§10.4 (historical decisions 1–4; §7.3 the in-tree-patch tell; §11 U5 the stdio `server/discover` uncertainty) · `ARCHIVE/v1-research/v1-sdd/mcp-engine-apps.md` §5.1–§5.4 (historical prior-art studies) · implementation evidence under `crates/agentcowork-mcp/` as renamed by DEC-053; exact current source paths are maintained in `TODO.md` with pending requalification under DEC-061 · `ARCH/05-INVARIANTS.md` INV-05 · `ARCH/14-PROVIDERS.md` §4/§7 · `ARCH/08-REQUIREMENTS.md` REQ-PROV-004/005, REQ-CHAN-012.

**Affects:** `14-PROVIDERS`, `32-CHANNELS`, `08-REQUIREMENTS`.

### DEC-049 — ACP governance class (`SelfContained`, and a derived Channel-B flag)

**Decision 1 — no ACP-launched session is classified `Mediated`.** Every agent reached over ACP is an **external process** — we are the protocol **client**, the agent is a child we spawn — so its own shell, file and network effects never cross our capability plane, our Guard or our audit trail. `Mediated` (we advertise the `fs`/`terminal` surface and service those calls ourselves, every effect ticketed and receipted) is therefore **unreachable for ACP in v1**; claiming it would be a claim about audit coverage we do not have. Concretely, the boundary is enforced, not asserted:

1. **Withhold the `fs`/`terminal` client capabilities at `initialize`.** The default handshake advertises the empty capability set; the mediated surface is only reachable through an explicit `initialize_with_caps` call, and the v1 launch path never makes it. Their presence in the protocol's method set is therefore **not** a claim that we drive them.
2. **Answer `session/request_permission` through the one Trust decider.** The bridge is a *projection, never a decider*: `once` is a single-use ticket bound to that request, `always` requires a durable user-made policy change and is otherwise narrowed back to `once`, `reject` is a first-class answer, an option id the agent never offered is a typed error rather than a synthesized string, and the approval card carries a bounded, secret-redacted diff. It routes the one approval primitive (`DEC-021`, `CTR-012`) and mints through Guard (`CTR-011`) — it does not decide anything of its own, and it fails closed.
3. **Class the session `SelfContained` and claim only that:** permissions are mediated **at the ACP boundary**, and effects performed inside the agent's own process are **outside the EveryAIOS audit trail**. The class is an architectural claim with a matching refusal, which is what makes it honest.

`Mediated` stays in the vocabulary (the enum carries it, and the picker vocabulary names it) for the **post-v1 governed baseline binding only** — a path where a governed in-process engine is the one first-class engine, so its effects do cross the capability plane. That baseline is **out of v1**. `NotGoverned` remains the correct answer, and the only other reachable class: an agent that neither mediates permissions nor routes effects gets no coverage claim at all, and every row the picker renders says so.

**Decision 2 — the Channel-B availability flag is derived from the mounted server list, never asserted.** A badge is read by the agent as permission to use a catalog; a hardcoded flag could claim a mounted catalog that is not mounted. The flag is now computed as "is the Channel-B server list non-empty", so a **failed lease leaves no servers and the flag reads `false`** — an unavailable lease can never produce a claim about a catalog the agent cannot reach. The previous shape (a constant `true` beside an optional lease) is what made a failed lease a false claim rather than a missing one; that is corrected here and the corrected shape is the invariant.

**Completion on record.** This decision **completes** DEC-009 — the external-agent boundary (`12` §8 projection enforcement, the decision this class implements) — and the archived v0 decision it descends from, ADR-0005 *“External agents are the v1 engines; the built-in engine defers to post-v1”* (decision item 2 is the deferred governed baseline binding, and its Consequences carry the honest split: an external agent's own tools are governed by that agent's permissions plus the OS sandbox). Both are **completed, not amended**: DEC-009 stays `Locked` with its projection list untouched, and the archived ADR stays `accepted` with its decision text intact. What was missing was the class the boundary implies, which is what this entry records.

**Status note:** Locked — both parts describe current, implemented, tested behavior (the permission bridge and the derived flag are merged and green). The post-v1 governed baseline that would make `Mediated` reachable is **out of v1**; returning it requires a superseding DEC.

**Evidence:** `ARCH/12-TRUST.md` §8 (external-agent boundary — the projection model whose governance half this class is) · `ARCH/32-CHANNELS.md` §3/§4 (the 7-item projection; ACP as client over a spawned child) · `ARCH/42-EVIDENCE-MAP.md` §4 FIX-07 (the governance-badge register entry, annotated close pending exactly this DEC/evidence note) · implementation `crates/everyaios-acp/src/client.rs:1422-1427` (the withhold-by-default handshake) · `crates/everyaios-acp/src/permission_bridge.rs:1-28` (projection-not-decider, the three mapping rules) · `crates/everyaios-acp/src/chief.rs:228-249` (`GovernedSession` + badge), `:255-272` (`governance_mode`) · `src-tauri/src/acp_cmds.rs:1667-1681` (the `SelfContained` classification and the derived `channel_b`) · `src-tauri/src/acp_cmds.rs:1426-1439` (a failed lease yields an empty server list) · `src-tauri/src/acp_cmds.rs:567-605` (every registry row classified, with the explicit note that internal effects are un-audited) · `crates/everyaios-acp/tests/acceptance_permission_bridge.rs:110,146,171,209,239,301` (the six bridge cases: single-use ticket · reject · `always` narrowed to `once` without a recorded policy change · fail-closed · replay refusal · never invent an option id) · `crates/everyaios-acp/src/chief.rs:767-791,876-878,986-1012` (the class mapping and the Channel-B bit, unit-tested) · `ARCHIVE/v0/ARCH/ADR/0005-external-agents-are-the-v1-engines.md` (archived, reference-only) · `ARCH/05-INVARIANTS.md` INV-11 (projections only) / INV-15 (transport isolation) · verified 2026-09-26: `cargo test -p everyaios-acp --lib --test acceptance_permission_bridge` ⇒ 176 + 6 passed, 0 failed (the derivation *at the call site* — `acp_cmds.rs:1680` — is exercised by the crate's `governance_mode` cases, not by a dedicated shell-side test).

**Affects:** `32-CHANNELS`, `12-TRUST`, `15-AGENT-PLANE`.

### DEC-050 — Base envelope carries a permissions reference, not a snapshot

**Correction on record (W1 kernel implementation, 2026-09-27):** two prose statements described the base envelope's actor context as carrying a permissions **snapshot** — `ARCH/10-KERNEL.md` §7 (*"who is calling (user · agent · workflow), with scope + permissions snapshot"*) and `REQ-KERNEL-007` in `ARCH/08-REQUIREMENTS.md` (*"actor context (user/agent/workflow with scope + permissions snapshot)"*). The canonical shape stated twelve lines below the first one already disagreed: the envelope JSON carries `"permissions_ref": "…"`. The implementation follows the shape, not the prose (`crates/everyaios-types/src/envelope.rs` — `ActorContext::permissions_ref`, a reference rather than values, resolved by the Trust owner).

**Why the reference is the correct reading:** the envelope crosses a trust boundary. INV-11 gives external callers projections only, and a permission snapshot travelling *inside* the envelope would be a second copy of an authorization decision that no ticket mints and no audit entry covers — exactly the "second, unaudited copy" failure the one-decider rule (`INV-01`, `ARCH/12-TRUST.md`) exists to prevent. The admission decision stays in Trust; the envelope only names it.

**Supersession scope (explicit):** this decision corrects **prose only**. The envelope's fields, `ARCH/07-CONTRACTS.md` CTR signatures, the Trust spine, INV-01/INV-11 and every other rule are unchanged; `ARCH/10-KERNEL.md` §7 is now internally consistent, and `REQ-KERNEL-007`'s statement matches the shape it derives from.

**Status note:** Locked (2026-09-27).

**Evidence:** `ARCH/10-KERNEL.md` §7 (JSON canonical shape) · `ARCH/05-INVARIANTS.md` INV-11 · `crates/everyaios-types/src/envelope.rs` `ActorContext` · `crates/everyaios-types/tests/envelope.rs`.

**Affects:** `10-KERNEL`, `08-REQUIREMENTS`, `07-CONTRACTS`, `05-INVARIANTS`.

### DEC-052 — The first-party agent is external; the agent plane is what we specify

**Owner decision (2026-09-27):** AgentCowork ships **no first-party reasoning engine**. The engine is developed outside this repository and is bound here afterwards as an ordinary engine binding. This removes the `Agent X` design from the v1 set.

**What this supersedes, and what survives.** DEC-010 is **not** weakened — its rule ("one `AgentEngine` contract, no privileged path") is *generalized*: it previously read as a first-party agent held to parity with external ones; it now reads as **every** engine held to parity with every other, which is a strictly larger surface with the same force. DEC-010 stays Locked. DEC-020's third working name is retired (the naming layer keeps AgentCowork and Core).

**Doc surgery (all of it, not a partial sweep).** `ARCH/15-AGENT-X.md` → `ARCH/15-AGENT-PLANE.md`, carrying the engine contract (`CTR-001/002/007`), session model, delegation + subagent lifecycle, isolation modes, receipts, interop and the Core/engine boundaries — and losing the loop, the planner, the recovery design, the eager-hot-set tool prescription and the proposed implementation crate. The first-party design is removed, not reworded. `REQ-AGX-001…013` and `TASK-AGX-001…014` are retired and never reused; the four genuinely v1 behaviors among them (delegation contract, subagent spawn/completion, isolation modes, receipts-not-transcripts) are re-seeded as `REQ-AGENT-001…004` so the coverage is not lost with the IDs. W4 is retired. INV-12 is renamed to engine parity and generalized. Every incidental reference across the spec, HLD, flows, context, memory, workflow, channels, skills, code, glossary, evidence map and the agent contract is reworded to the engine-agnostic form.

**The single architectural mention.** `AGENTCOWORK-SPEC.md` and `ARCH/03-HLD.md` state it once: no first-party engine ships in v1; a first-party engine bound later is a peer, not a privileged component. `ARCH/01-NAMING.md` records that the engine is external and why.

**Status note:** Locked (2026-09-27).

**Affects:** `01-NAMING`, `15-AGENT-PLANE`, `05-INVARIANTS`, `08-REQUIREMENTS`, `09-FEATURE-MATRIX`, `03-HLD`, `02-THESIS`, `16-CONTEXT`, `17-MEMORY`, `20-WORKFLOW`, `32-CHANNELS`, `40-FLOWS`, SPEC.

### DEC-053 — Identifier rename and the data-home migration owner

**Owner decision (2026-09-27):** code identifiers are unfrozen and renamed to the product name. `everyaios-*` → `agentcowork-*` across the 21 crates, the 10 TypeScript packages (`@everyaios/*` → `@agentcowork/*`), import paths, the workspace manifests, the Tauri identifiers and every user-facing string. `EveryAIOS` → `AgentCowork`. This supersedes DEC-020's identifier freeze; OQ-003 closes here.

**Data home and environment (the part that can lose data).** `~/.everyaios` → `~/.agentcowork` and `EVERYAIOS_*` → `AGENTCOWORK_*`, **with a legacy fallback**: resolution tries the new location and falls back to the legacy one, and a **single owner** — Core, at startup — performs the one-time migration. No lower-level crate migrates, so there is no migration race and no second migrator. Split constructors rather than a nullable key/argument where a store is encrypted (DEC-039, INV-02): the product path never opens a plaintext store and never silently falls back to a well-known default key.

**Status note:** Locked (2026-09-27).

**Affects:** `01-NAMING`, `10-KERNEL`, `12-TRUST`, `29-ARTIFACTS`, `17-MEMORY`.

### DEC-054 — Durable Mission, native-agent autonomy and scoped shared ecosystem

**Owner direction (2026-09-28):** capability, quality and user appeal govern the target architecture. External agents retain their own agentic architecture, model, native tools, MCPs, skills, plugins and private state. AgentCowork adds shared capabilities, durable missions, heterogeneous teams, workflows, evidence and human control. Horizon Code is a future external engine binding. No first-party reasoning engine is added here.

**Decision 1 — Mission above Work.** Introduce a first-class durable `Mission` with versioned GoalContract, individually addressable requirements, milestone/PlanNode graph, decisions, assumptions, evidence, outcome evaluation and stop/recovery control (`35`, `36`). PlanNode is semantic intent; Work is an attempt. Mission proposes Work through the existing scheduler and uses the one event store. It does not own a model loop, session transcript, separate queue or credential system. Mission graph is adaptive; `20` Workflow IR remains version-pinned executable process. This extends DEC-003/008/027 without replacing them.

**Decision 2 — truthful trust boundary.** DEC-049 `SelfContained` remains the correct default for an external agent with native effects. Core Trust governs **Core-mediated effects**; it cannot attest to authorization/audit of an agent's own file/shell/network/MCP calls. For native activity, record `agent_reported` or `native_observed` evidence with its provenance, never a Core receipt. A managed/mediated binding is opt-in and requires a verified enforcement boundary before it can claim stronger coverage. This supersedes absolute wording in INV-01/24, `03`, `12`, `13`, `15`, `32` and the product spec wherever it appears; it does not weaken the Core-mediated ticket path or vault rule. DEC-052 stays in force.

**Decision 3 — non-invasive discovery and scoped extensions.** Discovering an installed agent is read-only. Preserve its native config and extensions. Host-installed engines may use a reviewed private profile. Shared MCP/skill/plugin catalog visibility never implies activation or grant. Resolve per agent binding, workspace and Work, attenuate child grants, namespace collisions, start servers on demand and carry actual session/work/agent identity through Channel-B. An optional ephemeral overlay requires protocol support; no silent global configuration edit. `46` owns the HLD; `31`/`32` own LLD.

**Decision 4 — reuse and verification.** Retain one embedded Work/Workflow substrate; optional n8n/Activepieces adapters invoke their own engines and connectors. Workflow-to-skill promotion is versioned and evaluated (`37`). Mission outcome verification is separate from effect verification (`36`). One progressive UI exposes actual agent/native/shared ownership and evidence (`38`). Build sequence is `39`; source ledger is `45`. Cloud/mobile/team surfaces are target capabilities, with truthful availability and explicit executors, not deferred by an arbitrary release label.

**Supersession detail for older agent/context/model/memory wording.** DEC-043's “no second durable store” applies to **Core-owned** memory, not to an external engine's independently owned private memory. DEC-027/044's pre-turn budget, compaction and active-provider extraction rules apply to Core-owned inference or adapter-supported controls; an opaque agent's private model call, prompt, budget, retry, compaction and memory cannot be redirected or attested by Core. An external-agent turn has no presumed Core model provider: automatic extraction from its private transcript is disabled; extraction of Core-owned, user-approved material uses an explicitly configured Core provider or local model under the stated disclosure policy. Host session/child logs contain Core-observed events, not a copy of the agent's private transcript. This clarification governs `15`–`18` and their requirements without relaxing Core's own contracts.

**Status:** Accepted architecture direction; implementation not claimed. **Affects:** `00`, `03`, `05`–`09`, `11`–`17`, `20`, `29`–`32`, `34`–`42`, `AGENTS.md` and the product/UI specs. The owner subsequently expanded this pass under DEC-055; root product/UI specs now carry explicit amendments and `TODO.md` W6 tracks implementation.

### DEC-055 — Outcome-first experience for every user

**Owner direction (2026-09-28):** The finished product must serve nontechnical and technical users in one workspace. The default surface is a clear composer and finished work, not an agent or infrastructure dashboard. Agent/model selection, budgets, autonomy, teams, traces, MCPs and provider details remain available in context or Settings. No separate product mode is required to ask a question, delegate a task, or inspect a mission.

**Decision:** `48-EXPERIENCE-SURFACES.md` is the final Experience HLD/LLD over the frozen `AGENTCOWORK-UI.md` baseline. It owns the exact left rail, chat bar (`+`, `@`, `/`, agent/model/autonomy), answer rendering, agent/settings surfaces, attention inbox, right Workbench, universal document/editor surface and accessible copy. The root UI document remains a source-path inventory and predecessor design; where it conflicts with `48`, DEC-055 wins. In particular, its first-party Native picker, obligatory separate Work Mode, always-visible model/reasoning/context controls and raw `/eaios:*` host namespace are superseded. The internal Work/Mission/agent vocabulary remains in architecture and expert detail, not primary navigation. User-facing labels and onboarding are evaluated with real nontechnical users before claiming ease of use.

**Boundary:** An agent picker lists real bound engines only. It does not manufacture model/provider options for a self-contained engine. Authentication is performed in the engine's supported flow or in a Core-owned provider connection with distinct custody; a discovered agent's native configuration is never rewritten. Shared MCP/skills/plugins are catalogued centrally but activated by explicit scoped grants per binding/Work. A plugin cannot silently grant its whole package. Browser and document surfaces show actual supported edit fidelity and fall back explicitly to the native app when needed; they never promise perfect handling of every file type.

**Decision on teams:** Keep one Mission controller, Work scheduler and event store. A lead external agent can request heterogeneous child Work; each child has its own binding, context, grant, budget and output contract. Native subagents stay engine-owned and are observed only if exposed. “Swarm” is a task-shaped strategy, not a second framework: independent map tasks may fan out; dependent/integration work stays ordered; optional best-of-N has a comparison contract, isolated outputs and a judge or human review. No automatic agent count target.

**Evidence and delivery:** `49-TEST-CASES.md` defines scenario acceptance from basic chat to long-horizon heterogeneous work; `50-SYSTEM-BLUEPRINT.md` maps the final ownership and data flow. `47` retains market comparisons. The earlier architecture-only file restriction is superseded by the owner's explicit request in this pass to reconcile the UI spec and TODO as well; unrelated code and `CURRENT_RUN.md` remain untouched.

### DEC-056 — Resolve the final Experience's planning gaps

**Decision (2026-09-28):** A Core-owned settings registry declares one key, typed parser, owner and scope per setting. Cosmetic device preferences may persist locally, but authorization, connector grants and other policy state are Core-owned, revisioned and never accepted from a stale UI copy. Named profiles contain only versioned, non-secret preference overrides; secret values remain vault references and switching a profile previews its effective changes. Unsaved settings use Save/Discard/Cancel. The Settings UI is searchable and grouped; the Workbench is multi-tab and dockable. Neither a fixed six-slot rail nor a fixed count of Settings sections is an architectural requirement.

The event store owns `channel.health.changed` with source, phase, reason and last-confirmed time; the UI derives no confident running state from a silent channel. Every Core-mediated approval is decided by the one Trust owner, even when requested or answered through another channel, and the same decision record is visible locally. A native agent's private approval remains native and is only observed/reported with that provenance. Forking a conversation creates a new Session with a parent Session and origin reference plus a lineage event; Work is created only when that fork executes a turn. This avoids inventing a running Work merely to display a branch. `TASK-AGX-015` is retired without reuse; the engine-neutral work belongs to `TASK-AGENT-002`.

**Agent-profile schema clarification (2026-09-28):** `installed`, `discovered`, `launchable`, `available` and `disabled` are not mutually exclusive lifecycle states. DM-014 stores origin/location evidence, launch probe result, current health and user enablement separately; Experience derives its Ready/Needs setup/Discovered/Disabled groups from those facts. This satisfies REQ-UI-011 and prevents a discovered WSL agent or catalog listing from becoming selectable merely because one `status` string was set.

**Reason:** W5's frozen-baseline UI backlog contains five decision blocks and one retired agent task that otherwise permit mutually incompatible implementations. DEC-055 already selected the final interaction shape; this decision supplies the missing state and authority contracts. The resulting behavior is testable without adding a second scheduler, event store, permission decider or native-agent configuration path.

### DEC-057 — Fenced trigger ownership and truthful occurrence semantics

**Decision (2026-09-28):** DEC-003/031/033's “one scheduler” and “exactly-once claim” language denotes one **Core Work admission owner** and one **logical run per persisted occurrence identity**, not control over an external agent's private scheduler or exactly-once execution of real-world effects. The workflow trigger/wake loop is the single logical **trigger owner** for each definition; it materializes due time and authenticated connector/event/webhook occurrences, then asks Work to admit the run. Moving that owner to a local service or cloud executor requires an accepted handoff of definition/version, occurrence journal, source cursor and a fencing epoch. The old owner stops materializing before the successor claims. An uncertain handoff reconciles the journal and external effects before another claim. An external workflow provider retains trigger and node ownership for its own definition; the host tracks one invocation Work and never duplicates the provider trigger. A closed desktop without an accepted owner records a misfire or paused state; a detached row is not a live scheduler.

**Effect boundary:** unique occurrence keys and atomic claims prevent duplicate logical runs for one trigger identity. A step may still be attempted at least once after failure; provider idempotency, observed-state reconciliation and `needs_attention` for unknown keyless effects prevent unreviewed duplicate side effects. No exactly-once external-effect promise is made. This amendment narrows the older DEC-033 wording while preserving its journal, lease, version pinning and safe retry design. It also narrows INV-06 to **host-owned responsibilities**; agent-native and external provider schedulers remain in their own policy domains and are represented honestly.

### DEC-058 — Local machine observation as an extractable service

**Owner direction (2026-09-28):** Add a local, read-only Machine Observer for understandable system health, hardware/GPU, storage health, bounded history, safe OS queries, on-demand least-rights process/service diagnostics, local-model runtime telemetry and explicit WSL inspection. It must work as an AgentCowork capability and be independently buildable from its own source subtree as a laptop-monitor product.

**Ownership:** World Model (21) keeps stable process/device/volume identities and relationships; Machine Observer (51) owns time-varying metrics and bounded local history. It uses the existing Runtime supervisor (19), Trust decision path (12) and Capability Broker (13). The existing Files/Storage scanner remains the sole directory walker and treemap producer. Core's single event store gets observer lifecycle, consent, provider-health and alert transitions; high-rate sample rows remain in the Observer's bounded local telemetry store. No second Work scheduler, event log, policy decider or file scanner is introduced.

**Permission boundary (refined by DEC-059):** on the user's first explicit System Workbench visit, before any observation including basic read-only status, show an in-app scope/purpose/retention/recipient consent dialog; never at installation, app launch or background startup. The user explicitly enables it; process summary, advanced process diagnostics, network, history and WSL scopes have separate grants. Trust stores this local-user category consent until revocation so explicitly enabled history can continue when the panel closes; this is not an agent grant. Installer elevation is never telemetry consent. The default consumer route is NSIS `currentUser` and does not require admin. This repo pins Tauri CLI `2.11.4`; its generated WiX template sets `InstallScope="perMachine"` (`pnpm-lock.yaml`; exact template in `45`), and the repo does not override that template. Treat the MSI as an explicitly labeled administrator-managed, machine-wide install; its UAC is solely for installation. If MSI is offered as the ordinary consumer path in future, first build and qualify a per-user package; do not imply that the current MSI is per-user. Do not make the Observer require elevation during installation, add an elevated service/driver, or treat any install approval as permission to collect. OS elevation is requested only when a user requests an exact read and the selected provider actually needs it. Windows desktop/Core stays asInvoker; a narrow typed helper handles one read and exits behind the real UAC prompt. After in-app consent is declined, keep the scope off and show one inline explanation of what the System view can do if enabled; after UAC is denied, explain once which fields remain unavailable and offer one user-initiated retry or standard access. Do not nag or retry in the background. No hidden privileged service or global “admin access to everything” switch ships by default.

**Implementation choice:** one portable service host with a versioned local IPC protocol, platform/vendor adapters, bounded history and independently buildable manifest under `services/machine-observer/`. No network listener by default and no dependency on Core business logic, Trust, app storage or UI. Its only permitted AgentCowork crate dependency is the existing pure `agentcowork-types` contract crate for canonical wire DTOs; keep those DTOs free of service behavior so the standalone monitor can carry the same contract. Reuse patterns from pinned projects in 45; do not copy source in this architecture change. eBPF/kernel tracing, process control, packet payload capture, and an embedded Prometheus/Grafana stack are outside this service baseline; deeper capabilities require a separately reviewed decision.

**Status:** Accepted architecture direction; implementation and device qualification pending. **Affects:** 00, 03, 05, 06, 07, 08, 09, 12, 13, 19, 21, 30, 39, 42, 43, 44, 45, 48, 49, 50, 51, AGENTS.md, TODO.md.

### DEC-059 — Explicit data-consent dialog and bounded candidate comparison

**Owner direction (2026-09-28):** The System Workbench must ask before collecting even basic read-only metrics, explain any optional administrator-only reading, and keep the product useful if permission is declined. The architecture must also expose a user-facing way to compare independent agent/model approaches without turning every team task into an unbounded swarm.

**System permission UX:** On the user's first explicit open/request of the System Workbench, or before fulfilling a user- or agent-originated machine-data request, show an in-app consent dialog before the first sample. Core returns typed `authorization_required` with the complete `missing_grants` set: `consent_required(category, purpose, scope)` for local observation and `work_grant_required(work, capability, fields)` for sharing with that Work. If both are missing, report both; Experience may present one dialog, but each choice is independent and the request remains held until all required grants are valid. The agent cannot grant either permission, and the request itself does not imply consent. Explain the selected category, purpose, sampling cadence/active condition, local retention and agent-sharing boundary; offer **Enable local overview** and **Not now**. The overview samples on demand or while its page is open; collection after the page closes requires a separate history/background grant with an explicit cadence and retention. This product-consent gate governs only the Core Machine Observer capability: it neither grants nor blocks an external agent's independent native shell/OS tools, whose actual permissions remain separately visible under that agent's policy. Do not imply that declining Observer consent constrains a native agent; for enforced isolation, use a binding that actually provides it. This dialog is product consent, not an OS permission. If declined, collect nothing and show one contextual inline explanation with **Enable** / **Keep off**; only a new user action or reopening the permission control may ask again. Never show this dialog at app launch or install. Do not request admin rights during installation to enable observation. Keep NSIS per-user as the consumer default; the current machine-scope MSI UAC is installation-only. If an exact, provider-proven read needs elevation, explain that one field and the standard-access fallback next to a shield-marked action; that action opens Windows UAC directly. Denial leaves standard readings usable and yields at most one non-modal follow-up with a user-initiated retry and **Keep standard access**. No app-wide elevation, forced admin service, background prompt or repeated nag.

**Candidate comparison:** Add an explicit **Compare approaches** strategy distinct from heterogeneous team delegation. It creates one Mission comparison PlanNode and a user/budget-bounded set of Work attempts that share the same immutable contract, input versions and acceptance criteria. Record each candidate's actual harness/model, native and Core capability loadout and effective policy; if these differ, show that this compares worker systems rather than models alone. Isolate writes only through a binding that can actually enforce a separate worktree/artifact version; otherwise the candidate is read-only/ineligible or its native execution scope is explicitly reviewed before launch. Core-mediated external effects remain preview/dry-run until a result is selected and then use normal Trust tickets/approvals. Native effects remain under the agent's own policy; Core cannot promise to block them, so candidates with unbounded native effects are ineligible for the default safe comparison unless the user separately accepts that policy before dispatch. The evaluator reports criterion-level evidence, failure gaps, provenance, latency and cost. The user or a declared comparison policy selects a viable candidate; integration/fusion is a separate Work on fresh isolated state. Never auto-merge code, send messages, publish artifacts or commit Core-mediated effects merely because a candidate scored highest. Model/harness identity is provenance, not the ranking rule. This reuses Mission, Work, Agent adapters, Trust and verification; it adds no scheduler or agent framework.

**Evidence basis:** OpenChamber's pinned Multi-run shows same-prompt lanes, separate sessions/worktrees, per-lane outcomes, selectable fusion and source-membership checks. Its source also makes write isolation optional in the composer and has no durable Mission-level outcome contract; AgentCowork adopts the comparison surface and source identity while making isolation, budget and outcome proof explicit. See `45` and `49` TC-052.

**Status:** Accepted architecture target; implementation pending. **Affects:** 00, 03, 05, 06, 07, 08, 09, 12, 19, 35, 36, 40, 41, 42, 45, 46, 48, 49, 50, 51, AGENTS.md, TODO.md.

### DEC-060 — Prefer suitable structured capabilities before computer control

**Decision (2026-09-28):** When AgentCowork selects or recommends a host-shared capability path, it first considers a suitable, authorized typed API/connector, site-native MCP, or CLI. For web tasks it next considers browser DOM/accessibility operations; for native desktop tasks it next considers the platform accessibility interface. These are target-specific structured paths, not a universal ordering between browser DOM and OS accessibility. Use visual browser/desktop interaction only when a structured path cannot meet the task or is needed for verification. Raw input remains the final, separately gated rung. Selection also considers observed reliability, required effect coverage, freshness, permissions, latency, user preference and verified capability; an unavailable, unauthorized or unsuitable API is not a forced choice. This amends only the path-selection wording in DEC-011 and does not change World Model ownership, computer-use safety gates or DEC-054's external-agent boundary: a discovered agent may keep using its native tools, and Core records the actual path and provenance.

**Reason:** The old global ladder placed UIA/AX before browser structure and put API/MCP/CLI after UIA, while later capability/ecosystem docs preferred structured app interfaces first. A single preference now avoids unnecessary fragile GUI automation while keeping the best available path task-specific.

**Status:** Accepted target; implementation and comparative reliability evidence pending. **Affects:** `03`, `08`, `09`, `13`–`15`, `23`, `24`, `40`, `46`, `50`.

### DEC-061 — Adopt the official MCP protocol SDK behind Core-owned policy

**Decision — use the official Rust MCP SDK (`rmcp`, initially pinned to release tag `rmcp-v3.4.0`) for MCP protocol encoding, decoding, negotiation and transport state machines where the SDK represents the required behavior.** Keep provider-specific policy in `agentcowork-mcp`: per-server force-legacy state, origin/command-fingerprint cache identity, capability mapping, status projections, credential custody, lifecycle and Core-mediated authorization. Do not maintain a second hand-written implementation of protocol semantics merely to keep those provider policies.

**Guard boundary:** Streamable HTTP must use the SDK's custom `StreamableHttpClient` seam and a Core adapter that sends each request through Guard-2/netfloor, with redirect, DNS-rebinding, origin, body-size, timeout, and cancellation behavior tested against the existing policy. Do not enable the SDK's default `reqwest` transport for guarded external MCP calls. Stdio remains a bounded child-process transport with the same attach timeout, framing, cancellation, and child cleanup guarantees. Audit SDK default features and prefer an explicit feature set (`default-features = false`) so an unreviewed transport cannot enter the dependency graph; `cargo tree -e features` must prove no default HTTP client bypasses Guard. The official 2026-07-28 SDK table lists Rust as Tier 1; upstream tiering/conformance is evidence, not a substitute for AgentCowork's security and compatibility tests.

Use the SDK for the server façade only where its API can preserve DEC-048 clarification C: `initialize` is a separate lease-less, method-restricted, session-less compatibility path, while all modern calls remain pinned and authorized. Preserve DEC-048 clarifications A–C, DEC-030's protocol behavior, and its explicit non-goals. If a required behavior is absent from the pinned SDK, isolate the smallest protocol extension behind the adapter and record the gap; do not fork the SDK or silently replace its negotiation with a second engine. Remove old protocol code only after parity tests prove the migration.

**Basis:** the official `rmcp-v3.4.0` source defines the `StreamableHttpClient` injection interface and includes both `2026-07-28` and `2025-11-25` protocol constants; its `LATEST` default remains `2025-11-25`, so AgentCowork must explicitly negotiate/advertise versions rather than assume the newest date is selected. The official 2026-07-28 SDK table lists Rust as Tier 1. Exact source links and limitations are recorded in `ARCH/45-REFERENCE-RESEARCH.md` §MCP SDK.

**Status note:** Accepted architecture target; implementation, dependency/security review and conformance qualification remain pending. This decision supersedes **only DEC-048 decision 1** (hand-rolled client core / SDK rejection). DEC-048 clarifications A–C remain Locked. The closed `OQ-PRV-1` is replaced by implementation task `TASK-PROV-003` and acceptance test `TEST-PROV-003`.

**Affects:** `14-PROVIDERS`, `32-CHANNELS`, `08-REQUIREMENTS`, `09-FEATURE-MATRIX`, `44-ABSORB-REGISTER`, `45-REFERENCE-RESEARCH`.

### DEC-062 — Queued chat uses the latest selection at dequeue

**Decision:** A queued chat item stores its prompt, structured references, attachments, draft revision and conversation identity, but does not freeze the agent/model/access selection. When the item becomes eligible to start, Core resolves the latest valid selection on that logical conversation and records the actual `AgentBinding`, provider-qualified model (or `agent-managed`), mode and effective grants on the resulting Work. A user changing the picker while an item is queued therefore changes what the queued item will use.

The latest selection is revalidated at dequeue. If the selected agent is offline, unauthenticated, revoked, does not support a requested model or cannot accept an attachment/reference, the item remains queued with a typed `needs_resolution` state and an actionable explanation. It must not silently fall back to `Auto`, another model, another agent or a broader grant. The active Work that caused the queue remains owned by its original binding; this rule applies only when the later queued item is admitted.

**Reason:** The user expects a picker change to govern the next turn. Freezing a binding at queue time makes the visible picker misleading and can run a later action under stale permissions or stale availability. Content and references remain durable independently of the binding so a failed revalidation does not lose user work.

**Status:** Locked. **Affects:** `08`, `09`, `11`, `15`, `18`, `30`, `48`, `49`, `50`, `TODO`.

### DEC-063 — Clarify Core-mediated governance and receipt scope

**Decision:** The universal effect-governance and receipt language in DEC-002, DEC-010 and DEC-022 applies to effects mediated by AgentCowork Core. A self-contained external agent's native tools and effects do not pass through Core's Work/Capability/Guard/Ticket/Receipt path unless that agent explicitly invokes a shared Core capability. Native effects remain under the agent's own policy and environment; AgentCowork may record them only as `agent_reported` or `native_observed` evidence, never as a Core ticket, receipt, or verified Core-mediated effect. There are no bypasses **within the Core-mediated path**. For a shared capability invocation, the full Core path and its receipt rules remain mandatory regardless of which agent requested it.

**Reason:** DEC-054 established the native-agent boundary, and the current product specification, invariants, HLD, Trust, agent-plane and ecosystem docs already apply the Core path only to Core-mediated calls. Unqualified language in DEC-002 (“or the native agent”), DEC-010 (“every engine … passes the same Guard”) and DEC-022 (“every externally visible effect produces a durable receipt”) could imply that Core can intercept an external process's private tools or attest to effects it did not mediate. This clarification preserves the full Guard/Ticket/verification/receipt invariant for every Core call while making no claim that Core governs native calls.

**Supersession scope:** This decision clarifies the scope of universal governance/receipt statements in DEC-002, DEC-010 and DEC-022; it does not otherwise alter them. The Core-mediated execution sequence, latency targets, verification, receipt requirements and vault custody remain unchanged. DEC-054 remains the authority for the broader external-agent boundary.

**Status:** Locked clarification (2026-09-29). **Affects:** `02-THESIS`, `03-HLD`, `04-DECISIONS`, `05-INVARIANTS`, `07-CONTRACTS`, `12-TRUST`, `15-AGENT-PLANE`, `22-OFFICE`, `29-ARTIFACTS`, `34-EFFECT-VERIFICATION`, `46-ECOSYSTEM-ARCHITECTURE`, `AGENTCOWORK-SPEC.md`.

### DEC-064 — Cross-agent built-in skills and artifact-first conversation

**Decision:** AgentCowork ships a versioned, read-only host skill catalog available to every compatible agent binding and project. Catalog availability does not mean injection into every prompt: only relevant, selected skills are loaded for a session/Work, and a project may pin a version or disable a host default. Personal/global, project, session/Work and agent-native skills keep separate owner/scope namespaces; precedence and collisions are explicit. Host skills reach an external agent only through a supported, bounded session/context overlay or scoped host capability. If that binding cannot accept the overlay, show the skill as unavailable to that binding and offer supported host-side execution/manual guidance; never rewrite native skill stores or claim parity the adapter cannot provide. Skill activation never grants a capability or permission.

The initial built-in procedural catalog covers source-backed research; rich artifact authoring and visualization; document/report creation; spreadsheet analysis; presentation creation; image creation/editing when an image provider is connected; browser workflows; computer-use workflows; and reviewed workflow-to-skill capture. Each entry declares required host capabilities and output contract. Missing providers produce explicit guidance/fallback, not fabricated ability. The catalog is intentionally extensible and versioned; it is not a bundle of privileged plugins or global MCP servers.

Generated files and interactive artifacts are first-class conversation results. Prefer structured artifact refs from the host artifact API or binding result events. A filesystem watcher may attach an observed file ref only when a write is correlated to the active Work and resolves inside the authorized workspace. Chat turns that output into an artifact card and clickable exact-version ref; clicking focuses/opens the corresponding Workbench tab. A plain-text filename/path becomes a link only when it uniquely matches such a Work-scoped ref; ambiguous, stale, outside-scope or missing paths remain plain text or offer an explicit choice. Never interpret arbitrary path-like text as permission to read/open external content or execute it. Observed native-agent output keeps native provenance and never receives a Core effect receipt by inference.

The host UI normalizes supported agent stream/activity events into approachable progress labels and typed result cards. Raw TUI/ANSI output, tool names, provider details and diagnostic traces stay in expandable Advanced details; a dedicated Terminal tab is the only surface that intentionally shows terminal output. The host does not rewrite an agent's reasoning loop, hide permission boundaries, expose private chain-of-thought, or invent progress events unavailable from the binding.

**Reason:** A single conversation/workspace should remain useful while the user switches among capable agents. A host-owned procedural layer, scoped artifact bridge and consistent result renderer add common UX without taking ownership of an external agent's architecture. Google Antigravity's official artifact docs and Generative UI article demonstrate the product value of structured deliverables, review surfaces, in-context rich previews and export. Adopt those interaction ideas, not its private `agent-embed` syntax, tool names, CSP allowlists or internal skill files; use AgentCowork's artifact and security contracts instead.

**Status:** Accepted architecture direction (2026-09-29); implementation pending. **Affects:** `06-DATA-MODEL`, `07-CONTRACTS`, `08-REQUIREMENTS`, `09-FEATURE-MATRIX`, `15-AGENT-PLANE`, `16-CONTEXT`, `29-ARTIFACTS`, `31-SKILLS-PLUGINS`, `37-WORKFLOW-SKILL-LIFECYCLE`, `38-EXPERIENCE-QUALITY`, `46-ECOSYSTEM-ARCHITECTURE`, `48-EXPERIENCE-SURFACES`, `49-TEST-CASES`, `AGENTCOWORK-SPEC.md`, `TODO.md`.

### DEC-065 — Truthful active-work visualization

**Decision:** An optional Live Desk/active-work view is a read-only Experience projection over existing Mission, PlanNode, Work, typed events, artifacts, world observations and verification evidence. It is not another scheduler, event store, World Model, computer-use engine, or default mode. Display mission workstreams by outcome; worker identities and technical traces are secondary details. Replay reconstructs only recorded facts and never executes or invents intermediate actions.

Where existing event payloads cannot support a useful, truthful label, Work/domain results may add a bounded, versioned activity descriptor to the existing event/result contract: semantic verb, actual execution mechanism, correlation reference, existing resource references, measured quantities only, sensitivity, timestamp and provenance (`core_mediated`, `core_observed`, `native_observed`, or `agent_reported`). Do not infer meaning from tool-name prefixes. Reuse canonical Work/Artifact/World identities; do not create a competing `ResourceRef` identity registry. Domain-owned browser/desktop anchors may be passed to the renderer as ephemeral observations only. They are neither durable identities nor authorization; discard them when their owning snapshot/document epoch is stale or correspondence to the displayed surface is uncertain.

Opening a task/Workbench panel does not initiate screen capture. Existing browser and desktop paths provide snapshots, not a general live stream. A future live surface stream is a separate optional, read-only observation capability with exact target/session scope, explicit user authorization and visible active state, bounded latest-frame-wins delivery, cancellation on hide/takeover/revoke, no frame persistence by default, and no Work backpressure. It cannot be assigned `CTR-033`, which is already MachineObserverService. Add a new stable contract only after a platform/browser proof establishes need, consent behavior, lifecycle, resource bounds and failure semantics. No stream is promised for an opaque native agent or unsupported remote/platform binding.

**Reason:** The current architecture already gives Mission/Work/event projection, artifact and verification owners, privacy boundaries and takeover fencing. The proposal's visual principle is sound, but its claims of existing activity projections and streaming capture are overstated: `NowDoingStrip` is partly derived from active-session state; `CompanionChip` is selected-session-only; Cockpit snapshots poll; DesktopView is user-operated; WGC and browser capture are one-shot. Build semantic visibility first and add pixels only where evidence and user value justify another governed capability.

**Status:** Accepted target architecture (2026-09-29); implementation pending. Continuous surface streaming is deferred. **Affects:** `03-HLD`, `06-DATA-MODEL`, `07-CONTRACTS`, `08-REQUIREMENTS`, `09-FEATURE-MATRIX`, `11-WORK`, `13-CAPABILITY`, `14-PROVIDERS`, `19-RUNTIME-ENVIRONMENTS`, `21-WORLD-MODEL`, `23-BROWSER`, `24-COMPUTER-USE`, `29-ARTIFACTS`, `30-EVENTS`, `34-EFFECT-VERIFICATION`, `35-MISSION`, `36-OUTCOME-AND-RECOVERY`, `38-EXPERIENCE-QUALITY`, `40-FLOWS`, `41-EDGE-CASES`, `42-EVIDENCE-MAP`, `48-EXPERIENCE-SURFACES`, `49-TEST-CASES`, `50-SYSTEM-BLUEPRINT`, `TODO.md`.

### DEC-066 — Expressive, truthful Live Desk motion

**Decision:** Live Desk is an optional, user-friendly visual explanation of active work, not a technical topology dashboard. Use composed workstreams, readable resource/artifact cards, and purposeful, brief transitions to help people understand how work moves from inputs through activity to checks and outputs. Motion is a rendering treatment of known state transitions, not an additional fact: it may show an indeterminate “working” treatment only while an authoritative source says the Work is running, and may animate a resource between conceptual UI regions only when the corresponding event/state transition is recorded. It must never imply a physical click, cursor path, hidden app access, measured quantity, percentage, success, or verification that the system did not observe. Actual physical input remains visually distinct and is shown only when raw-input ownership is real. A static view communicates the same state, sources, blockers, and evidence.

The default Live Desk is calm, warm, clear, and approachable to nontechnical users, students, and expert users; ordinary chat stays visually quiet. Complexity is progressive: plain-language status and outcomes first, technical details available on demand. Status copy describes the user's task in everyday language (for example, “Looking through your selected files,” “Checking the totals,” or “Ready for you”); counts appear only when measured, and opaque native activity falls back to a general truthful status rather than invented detail. Each workstream's status and motion follows its own Work/evidence. A branch waiting on the user becomes still and actionable while independent running workstreams remain visibly active; the mission summary describes mixed activity instead of implying that everything stopped or finished. Avoid constant decorative motion, flashing, sustained oscillation, motion that blocks interaction, and visual-only status. Honor operating-system reduced-motion preferences and offer an in-product Static/Reduced motion choice and a clearly named Pause motion control. Pausing animation never pauses the Work; hiding Live Desk stops optional presentation updates, not execution. Screen readers announce meaningful semantic state changes rather than frames, timers, or repeated progress ticks. Measure rendering cost and conduct usability studies that include nontechnical users; an aesthetic claim alone is not acceptance evidence.

The motion system is local Experience rendering over DEC-065 projections. It adds no event family, model call, execution, capture permission, animation-specific backend state, or continuous surface stream. Incomplete event coverage falls back to a clear static/generic state. Use interaction feedback and transition principles consistent with platform accessibility guidance; users can cancel motion without waiting for it to finish. See `48`, `38`, `08` REQ-UXQ-013, `49` TC-070, and `TODO.md` TASK-UXQ-012.

**Reason:** “Evidence-backed” must not be misread as “static” or “technically dry.” Motion can make state changes easier to follow and more enjoyable when it clarifies relationships. Conversely, theatrical animations can misrepresent agent-native work or distract users. This decision makes the visual value explicit while retaining the evidence and privacy boundary in DEC-065. W3C guidance requires a way to pause/stop/hide qualifying automatically moving or updating content and supports reduced-motion alternatives; Apple’s motion guidance recommends purposeful, optional, brief feedback that users can cancel. These are accessibility/design references, not proof that AgentCowork’s design is good.

**Status:** Accepted target architecture (2026-09-29); implementation and user evidence pending. **Affects:** `00-INDEX`, `03-HLD`, `05-INVARIANTS`, `08-REQUIREMENTS`, `09-FEATURE-MATRIX`, `30-EVENTS` (presentation-only clarification), `38-EXPERIENCE-QUALITY`, `40-FLOWS`, `41-EDGE-CASES`, `42-EVIDENCE-MAP`, `47-MARKET-AND-BENCHMARKS`, `48-EXPERIENCE-SURFACES`, `49-TEST-CASES`, `50-SYSTEM-BLUEPRINT`, `TODO.md`.

**References:** [W3C WCAG 2.2.2: Pause, Stop, Hide](https://www.w3.org/WAI/WCAG22/Understanding/pause-stop-hide); [W3C WCAG 2.3.3: Animation from Interactions](https://www.w3.org/WAI/WCAG21/Understanding/animation-from-interactions); [Apple Human Interface Guidelines: Motion](https://developer.apple.com/design/human-interface-guidelines/motion).

### DEC-067 — Shared read-only extension market (proposal)

**Status:** Proposed target architecture; implementation and adoption are pending.

**Decision:** Use one product-neutral Shared Extension Market catalog for both HorizonCode and AgentCowork. HorizonCode is the first consumer; AgentCowork later consumes the same published catalog revision instead of building a second public index. The initial market is a controlled ingestion/review pipeline that publishes versioned JSON snapshots through a public read-only HTTPS endpoint/CDN. It indexes metadata and upstream package pointers; it does not host executable payloads, accept public submissions, keep marketplace accounts or ratings, or perform installation. Product clients may cache and filter the shared revision and later add explicitly scoped private sources.

The shared catalog has four listing families: Connector/service, standalone MCP server, Agent Skills package, and plugin bundle. The broad-release goal is at least 500 unique source-resolvable, type-qualified listings, with 1,000 as the expansion target. The planning mix is 200 Connector/service records, 150 standalone MCP servers, 100 skills, and 50 plugin bundles. Mirrors, versions, alternate provider offers, and components nested inside a bundle do not inflate those counts. Service identity, provider offer, account Connection, capability, and permission grant remain different objects. Compatibility, source resolution, publisher verification, security review, official status, and installation remain separately evidenced claims.

The existing local-first package review, confinement, Vault, Guard, scoped grants, and active-Work pinning remain authoritative. A market listing or Connector card cannot install a package, authorize an account, expose credentials, or grant a tool. The Core Capability Catalog (`13`) and model/provider catalog (`14`/model plane) are not merged with the Shared Extension Market.

**Reason:** Official product sources describe different distribution surfaces rather than one common marketplace protocol. Codex documents portable plugin packages and local/repository catalogs; Claude Code documents Git-backed marketplace manifests; Grok Build documents plugin/skill/MCP marketplace compatibility; the MCP Registry provides MCP-server metadata; Agent Skills standardizes a package layout, not a universal marketplace service. A small shared read catalog can aggregate these source formats while preserving their identities and trust limits.

**Status rule:** This remains a proposal. No shared service, endpoint, catalog snapshot, or 500-entry evidence exists. No code or skill-store admission schema changes follow from this proposal. Adoption requires the repository's documented review process.

**Affects:** `00`, `03`, `08`, `09`, `31`, `42`, `45`, `46`, `48`, `49`, `50`, `SPEC`, `TODO.md`.

## 3. Pending decisions

| ID | Decision needed | Inform by | Affects |
|---|---|---|---|
| PEND-01 | ~~MCP era policy~~ → resolved as DEC-030 | ✅ `lib-4` (2026-09-26) | 14 |
| PEND-02 | ~~Compaction strategy priority + cache-stability rules~~ → resolved as DEC-027 | ✅ `gen-21` (2026-09-26) | 16 |
| PEND-03 | ~~Scheduler lanes + global limits~~ → resolved as DEC-031 | ✅ `11-WORK` (2026-09-26) | 11, 15 |
| PEND-04 | ~~First-release surfaces (desktop + CLI minimum? ACP timing)~~ → superseded by DEC-055's capability/quality target and `32`'s channel requirements | ✅ DEC-055 (2026-09-28) | 32 |
| PEND-05 | Agent profile / "assistant" composition model naming | 15, UI doc | 15 |
| PEND-06 | ~~Memory encryption at rest (SQLCipher vs plaintext; item-level for confidential)~~ → resolved as DEC-039 | ✅ `17-MEMORY` (2026-09-26) | 17 |
| PEND-07 | ~~Artifact storage layout + retention policy~~ → resolved as DEC-032 | ✅ `29-ARTIFACTS` (2026-09-26) | 29 |
