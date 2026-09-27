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
| DEC-010 | Agent X is an architectural peer — one `AgentEngine` contract, no privileged path | Locked | 15, 32 |
| DEC-011 | World Model is first-class; computer-use ladder; vision is fallback | Locked | 21, 24 |
| DEC-012 | Browser: managed Chromium default; Chrome/Edge/Firefox/Opera as adapters | Locked | 23 |
| DEC-013 | Office: runtime under the universal document surface; L1/L2/L3; resident contexts | Locked | 22 |
| DEC-014 | Artifacts (work products) ≠ Library (reusable inventory); promotion explicit | Locked | 29 |
| DEC-015 | Token discipline: deterministic operations never touch an LLM | Locked | 02, 16, 34 |
| DEC-016 | No captcha-evasion / anti-bot / residential-proxy tooling | Locked | 23, 24 |
| DEC-017 | Absorb strategy: study → redesign → implement; licensing ledger gates code reuse | Locked | 44 |
| DEC-018 | Memory v1 store: SQLite + FTS5; ADD-only extraction + `superseded_by`; suppression-based forget; no vectors/graph/decay | Locked | 17 |
| DEC-019 | Memory ≠ context; recall returns candidates; non-touching read; budget is a maximum | Locked | 16, 17 |
| DEC-020 | Working names: AgentCowork / Core / Agent X; code identifiers frozen until post-freeze | Provisional | 01 |
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
| DEC-043 | External-agent memory boundary: v1 recall-only projection (bound project + own session/task + user preferences); Core never writes native agent stores; provider-session transcripts not harvested; subagent child sessions harvested only when Core-owned with parent linkage; Agent X private notes are session-scope items + session log | Locked | 17, 15, 32 |
| DEC-044 | Extraction model & disclosure: session provider default; `confidential` scopes local-only or off until enabled; global extraction budget + global/per-scope kill switches + per-run metering; concurrent sessions bounded | Locked | 17, 18, 11 |
| DEC-045 | Provider-native compaction adoption policy: amends DEC-027's evidence clause only (a verified shipping reference exists — Codex remote compaction v2) and freezes the adoption rules (provider capability event · Guard egress + audit + per-provider off switch · usage via `18` · deterministic checkpoint stays primary); DEC-027 stays Locked with its rules untouched | Locked | 16, 18 |
| DEC-046 | Verification plane stage completion: completes DEC-023's stage list to five — observe → validate → render → verify → reconcile (DEC-023's rules otherwise unchanged) | Locked | 34, 12 |
| DEC-047 | Capability id mapping for adapter-native tools: capability ids stay protocol-neutral; provider adapters own the mapping table (`transport_ref` + native tool name ↔ capability id) at discovery; unmapped native tools are not invocable (guidance, never silent exposure); mapping changes are registry data (epoch-checked), never contract changes | Locked | 13, 14 |
| DEC-048 | MCP client core ownership + DEC-030 clarifications: the client core stays hand-rolled, dual-era and patch-owned (`rmcp` not adopted; OQ-PRV-1 closed) · force-legacy is a persisted per-server record field and era+source is a read-only projection · era caching per origin (HTTP) / per command fingerprint (stdio) and a stdio probe timeout fails the attach typed · the façade's `initialize` compatibility is lease-less, method-restricted and session-less | Locked | 14, 32, 08 |
| DEC-049 | ACP governance class: no ACP-launched agent is ever `Mediated` (all are external processes) — withhold the `fs`/`terminal` client capabilities at `initialize`, answer `session/request_permission` through the one Trust decider, and claim only `SelfContained` (mediated at the ACP boundary; effects inside the agent's own process are outside our audit trail) · `Mediated` stays in the vocabulary only for the post-v1 governed baseline; `NotGoverned` remains the honest answer when an agent neither mediates permissions nor routes effects · the Channel-B availability flag is **derived from the mounted server list, never asserted** | Locked | 32, 12, 15 |
| DEC-050 | Base-envelope actor context carries a permissions **reference**, not a snapshot: `permissions_ref` is resolved by the Trust owner at admission (INV-11), because the envelope crosses a trust boundary and an in-envelope snapshot would be a second, unaudited copy of an authorization decision — clarifies the `10-KERNEL` §7 and `REQ-KERNEL-007` prose to the canonical shape already stated in that section; no envelope field, contract or trust rule changes | Locked | 10, 08, 07, 05 |

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
The Workflow Engine is Core infrastructure, a peer of the Agent Runtime — not a feature inside Agent X. Workflows call agents; agents author and invoke workflows (workflows-as-tools). Deterministic vs adaptive is an explicit distinction.
**Affects:** `ARCH/20-WORKFLOW.md`.

### DEC-009 — External agents get projections
External agents (ACP/A2A/API/CLI) connect through the Agent Gateway and receive exactly: identity contract, capability projection, context projection, workspace projection (allowed/read-only paths), filtered tool set, artifact gateway, filtered event stream. No Core internals, no Agent X internals.
**Affects:** `12`, `16`, `32`.

### DEC-010 — Native agent parity
Agent X implements the same `AgentEngine` contract as every external agent and passes the same Guard. Any proposal to give it a shortcut (direct store access, bypassed tickets, internal hooks) is rejected unless a superseding DEC records the full trade-off.
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
AgentCowork (product), Core (runtime), Agent X (native agent) are working names for v1; the rename map and rules live in `ARCH/01-NAMING.md`. Code identifiers (`everyaios-*`, `EveryAIOS` strings) stay frozen until a post-freeze code-phase rename.
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
**Affects:** `16-CONTEXT`, `11-WORK`, `15-AGENT-X`.

### DEC-028 — Guard as three layers
Permission enforcement composes three distinct layers and never collapses them into one enum: (1) **platform confinement** (sandbox policy — OS-level bounds per platform); (2) **approval policy** (when a human is asked; policy enum + granular per-category config); (3) **declarative exec rules** (pre-authorized command/prefix/network patterns). Guard turns the composition into ALLOW/ASK/DENY; tickets encode the outcome; protected subpaths (e.g. VCS hooks) stay read-only inside writable roots.
**Evidence:** `agent-harness-verification.md` §A4; anchors `clone2/codex/codex-rs/protocol/src/protocol.rs:969-1125` · `sandbox.rs:10-16` · `execpolicy/src/`.
**Affects:** `12-TRUST`, `19-RUNTIME-ENVIRONMENTS`.

### DEC-029 — Subagent model
One child session per subagent (own context/toolset/persona), full escaped project rules delivered to children; `fork_context` is a per-spawn option (default fresh + bounded snapshot); worktree isolation is a per-spawn option, with write leases for overlapping files; parents receive **worker receipts**, never transcripts; the platform enforces outer bounds (parallel/total/depth/tokens/spend) while the running agent decides within them. A "review queue" is our own product-layer feature — not borrowed (Codex source contains no queue).
**Evidence:** `agent-harness-verification.md` §A3, §B3, §E7; anchors `clone2/grok-build/crates/codegen/xai-grok-shell/src/agent/subagent/spawn.rs:1-33` · `host_service.rs:500-503, 566-578` · `prompt/context.rs:152,196` · `clone2/codex/codex-rs/core/src/tools/handlers/multi_agents_spec.rs:14-16, 726-737`.
**Affects:** `15-AGENT-X`, `11-WORK`, `20-WORKFLOW`.

### DEC-030 — MCP dual-era policy
Client side: detect and negotiate per transport — **stdio** probes `server/discover` (10 s cap) then falls back to legacy `initialize`; **HTTP** classifies the `400` body; the negotiated era is cached per process/origin; a per-server force-legacy escape hatch exists. Implement on `rmcp` 3.4.x (verified to carry both `2026-07-28` and `2025-11-25`). Server façade (our own MCP surface): stateless modern + `initialize` compatibility, with the mandatory `server/discover` method and `Mcp-Method`/`Mcp-Name` validation. **Non-goals:** HTTP+SSE transport, sessions/resumability, sampling, roots, logging.
**Corrections on record:** HTTP+SSE has been deprecated since `2025-03-26` (~18 months; removal clock = SEP-2596, Final 2026-05-18 + 3 months ⇒ eligible ≈2026-08-18, not yet removed) — the earlier “≥12 months” framing was wrong. Code-phase fixes identified: the existing in-repo remote client sends no `_meta`/modern headers; `server/discover` is absent from the current façade.
**Evidence:** `ARCHIVE/v1-research/mcp-provider-verification.md` (641 lines) — spec changelog `2026-07-28`; versioning/transports/deprecated pages; `clone2/grok-build/crates/codegen/xai-grok-mcp/src/servers.rs:3782-3910`; `clone2/codex/codex-rs/rmcp-client/src/protocol_mode.rs:9-51`; `rmcp@3.4.1`; SEP-2596.
**Affects:** `14-PROVIDERS`, `32-CHANNELS`.

### DEC-031 — Work scheduler lanes + outer limits
Three lanes — **foreground** (the active interactive turn; 1/session) · **background** (jobs/workers admitted without blocking the UI) · **detached** (long work that may outlive the app session; rehydrated on start) — with Core-enforced outer bounds the running agent cannot exceed: max simultaneous agents · max total workers per work tree · max depth · max worker tokens · max session spend · per-lane concurrency. Interactive > background priority; starvation guard; queue-depth backpressure; parent→child cancellation; budget exhaustion pauses and surfaces (no silent overrun).
**Evidence:** product-owner brief (lanes, limits; “background work is essential”); `ARCH/11-WORK.md` §3; `agent-harness-verification.md` §A3 (background guidance), §E7 (bounds as per-spawn policy).
**Affects:** `11-WORK`, `15-AGENT-X`, `20-WORKFLOW`.

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
**Affects:** `15-AGENT-X`, `11-WORK`, `12-TRUST`, `30-EVENTS`.

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
v1 memory exposure to external agents is **read-only filtered recall** — bound project + own session/task + user preferences; no org, no other projects, `confidential` only with a recorded loadout (v1 default: none). Core never writes or mutates an external agent's native memory/config/session stores, and provider-session transcripts we do not own are never harvested; imports from native stores stay deferred (U8), explicit, read-only to the source, and audited. Subagent child sessions are Core-owned: they may be harvested at their own settle boundaries with parent linkage (`source_ref`), never auto-promoted, and receipts enter extraction only as untrusted data. Agent X's “private working notes” are session-scope memory items + the session log — there is no second durable store.
**Evidence:** `ARCHIVE/v1-research/v1-sdd/memory-agent-deep-dive.md` §4/§6.1 (findings F-13/F-14; contradiction C-08); `ARCH/15-AGENT-X.md` §5/§7; `ARCH/32-CHANNELS.md` §3; `ARCH/41-EDGE-CASES.md` EDGE-150/151.
**Status note:** Locked.
**Affects:** `17-MEMORY`, `15-AGENT-X`, `32-CHANNELS`.

### DEC-044 — Extraction model & disclosure policy
Background extraction defaults to the session's active provider (no *new* disclosure). `confidential` scopes extract **local-only or not at all** until explicitly enabled by the user. Extraction runs against a declared **global budget** (calls/tokens per period) with global and per-scope **kill switches** and per-run metering (`memory.extraction.run`); concurrent sessions cannot exceed the budget. This closes the extraction-disclosure item (`OQ-MEM-03`) and bounds cost/DoS exposure.
**Evidence:** `ARCHIVE/v1-research/v1-sdd/memory-agent-deep-dive.md` §6.1 (finding F-15); `ARCH/17-MEMORY.md` §5.1/§9; `ARCH/04-DECISIONS.md` DEC-031 (outer limits).
**Status note:** Locked — default model policy fixed; budget constants are product knobs.
**Affects:** `17-MEMORY`, `18-MODEL-ROUTING`, `11-WORK`.

### DEC-045 — Provider-native compaction adoption policy
**Correction on record:** DEC-027's evidence clause ("the verified shipping set has none") is factually stale. A verified shipping reference exists: Codex remote compaction v2 — `run_remote_compaction_request_v2` (`codex-rs/core/src/compact_remote_v2.rs:387-412`), a `ContextCompactionItem` protocol request item (`codex-rs/protocol/src/items.rs:509-517`), a `compaction_output` response (`codex-rs/core/src/compact_remote_v2_attempt.rs:23-24, 104-129`), with analytics distinguishing `CompactionImplementation::{Responses, ResponsesCompactionV2}` (`codex-rs/analytics/src/facts.rs:460-463`). **DEC-027's rules are untouched** — named terms, pre-turn feasibility, one overflow recovery, the projection boundary over a durable log, and pruning-as-a-separate-transform all stand; this decision corrects the evidence and freezes the policy if a provider-native path is adopted.
**Supersession scope (explicit):** this decision amends DEC-027's **evidence clause only**. DEC-027 remains Locked; its behavioural rules — named budget terms, pre-turn feasibility, one overflow recovery, the projection boundary over a durable log, and pruning as a separate transform — are not superseded or amended, and this decision is the correction on record wherever that evidence clause is read.

**Adoption rules (frozen):** a provider-native compaction path is a **provider capability event**, never an implementation detail — conversation egress goes through Guard with provider allowlisting, audit and a per-provider off switch (INV-02/INV-05); its usage/cost rolls into `18` telemetry as a second inference call; the deterministic checkpoint remains the primary reconstructable state and provider output is never the sole checkpoint. Adoption is decided per provider behind capability detection (OQ-CTX-01).
**Status note:** Locked — the correction and the adoption rules are fixed; which providers ship the path remains open (`OQ-CTX-01`). Proposed 2026-09-26 (Agent X finalisation, P7 pass 15); DEC-027's text is not amended.
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

### DEC-048 — MCP client core ownership + three DEC-030 clarifications

**Decision 1 — the client core stays hand-rolled, dual-era and patch-owned; `rmcp` is not adopted in the Rust kernel.** The dual-era client we own (`crates/everyaios-mcp/src/remote.rs`) is the Rust kernel's MCP protocol core: era detection, negotiation, the modern `_meta` envelope and the legacy `initialize` path are one implementation, transport-agnostic behind a small HTTP seam, and we own and patch it in-tree. `rmcp` is **not** adopted, for two reasons. (a) Guard egress (INV-05) requires every outbound request to be routed through our policy; a transport that owns its own HTTP client cannot be routed without an SDK-provided `fetch`/connector seam or a fork — so adoption would mean forking the SDK for the same reason the hand-rolled client already exists. (b) A Rust SDK would *add* a second protocol core next to the existing TypeScript one rather than remove it, which is the "no second engine" rule (`DEC-006`) inverted. This closes `OQ-PRV-1`. **Revisit trigger:** the transport-seam milestone — and only with a written answer to all three questions: does `rmcp` expose a `fetch`/connector seam for Guard egress · does it carry both revisions (`2026-07-28` and `2025-11-25`) under a stable API · does adopting it delete more code than it adds. A partial answer does not reopen it.

**Clarification A — the force-legacy hatch is persisted state, and the era is a read-only projection.** The hatch is a field on the **stored server record** (`StoreEntry.force_legacy`), never a per-call argument: it changes the wire contract that server is spoken in, so a runtime-only flag would be lost on restart and silently re-pin the origin to a wrong era. The **effective era plus its source** (`forced` · `cached` · `probed` · `default`) is a **read-only projection** surfaced to status surfaces; reading it **never probes the network** (a read that issued a probe would turn inspection into a network round trip).

**Clarification B — era caching is keyed per origin on HTTP and per command fingerprint on stdio; a stdio probe timeout fails the attach.** DEC-030 says "per process/origin"; an *origin* (`scheme://authority`) is meaningless for a spawned child, so stdio verdicts are cached under a **command fingerprint** (the resolved command line) and re-probed when the command is edited. The era probe carries a **10 s budget on both transports**, defined once so the two cannot drift. A stdio probe that **times out fails the attach with a typed error** rather than falling through to the legacy path: an unanswered probe leaves the ND-JSON stream desynchronised, so a late `server/discover` reply would be read as the `initialize` reply and the reconciled tool names would belong to the wrong request. A `-32601` refusal is a *reply*, not a timeout, so the legacy fallback still runs.

**Clarification C — the façade's `initialize` compatibility is served lease-less and method-restricted.** The strict lease pins the modern revision, and **`initialize` alone is exempt** from that pin: it is read-only, creates no session, grants no capability handle, dispatches nothing, and every other method stays pinned. The exemption is a separate admission function, not a branch inside the modern gate, so the weakening is structural and auditable (INV-03). **If `initialize` ever becomes session-bearing, this exemption must be revisited** — that condition is the whole safety argument.

**Amendment scope (explicit):** DEC-030 stays **Locked** and its text is not amended; clarifications A–C are the correction on record wherever DEC-030's force-legacy hatch, era-caching and `initialize`-compatibility clauses are read. DEC-030's implementation clause ("implement on `rmcp` 3.4.x") is the one clause **superseded** by decision 1 — the client core is ours, not an SDK. DEC-030's non-goals (HTTP+SSE · sessions/resumability · sampling · roots · logging) and its two code-phase corrections are unchanged (both corrections are now implemented — see `14` §4).

**Status note:** Locked — the ownership decision, the three clarifications and the revisit trigger are fixed. `OQ-PRV-1` is closed as of this entry (`14` §10 item 1).

**Evidence:** `ARCHIVE/v1-research/v1-sdd/mcp-engine-opencode.md` §10.1–§10.4 (decisions 1–4; §7.3 the in-tree-patch tell; §11 U5 the stdio `server/discover` uncertainty) · `ARCHIVE/v1-research/v1-sdd/mcp-engine-apps.md` §5.1–§5.4 (decisions 1–4 across the three studied products) · implementation `crates/everyaios-mcp/src/remote.rs:61` (probe budget), `:185-205` (`EraSource`), `:427-447` (origin + stdio fingerprint keys) · `crates/everyaios-mcp/src/store.rs:79-90` (`force_legacy`) · `crates/everyaios-mcp/src/server.rs:79-88`, `:2639-2658` (the `initialize` exemption) · `src-tauri/src/mcp_cmds.rs:876-902` (read-only `effective_era`) · `crates/everyaios-mcp/tests/acceptance_mcp_dual_era.rs:671`, `:716`, `:780`, `:875` (`cargo test -p everyaios-mcp`: 179 passed / 0 failed / 4 ignored — the ignored set is the opt-in external inspector CLI, not this surface) · `ARCH/05-INVARIANTS.md` INV-05 · `ARCH/14-PROVIDERS.md` §4/§7 · `ARCH/08-REQUIREMENTS.md` REQ-PROV-004/005, REQ-CHAN-012.

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

**Affects:** `32-CHANNELS`, `12-TRUST`, `15-AGENT-X`.

### DEC-050 — Base envelope carries a permissions reference, not a snapshot

**Correction on record (W1 kernel implementation, 2026-09-27):** two prose statements described the base envelope's actor context as carrying a permissions **snapshot** — `ARCH/10-KERNEL.md` §7 (*"who is calling (user · agent · workflow), with scope + permissions snapshot"*) and `REQ-KERNEL-007` in `ARCH/08-REQUIREMENTS.md` (*"actor context (user/agent/workflow with scope + permissions snapshot)"*). The canonical shape stated twelve lines below the first one already disagreed: the envelope JSON carries `"permissions_ref": "…"`. The implementation follows the shape, not the prose (`crates/everyaios-types/src/envelope.rs` — `ActorContext::permissions_ref`, a reference rather than values, resolved by the Trust owner).

**Why the reference is the correct reading:** the envelope crosses a trust boundary. INV-11 gives external callers projections only, and a permission snapshot travelling *inside* the envelope would be a second copy of an authorization decision that no ticket mints and no audit entry covers — exactly the "second, unaudited copy" failure the one-decider rule (`INV-01`, `ARCH/12-TRUST.md`) exists to prevent. The admission decision stays in Trust; the envelope only names it.

**Supersession scope (explicit):** this decision corrects **prose only**. The envelope's fields, `ARCH/07-CONTRACTS.md` CTR signatures, the Trust spine, INV-01/INV-11 and every other rule are unchanged; `ARCH/10-KERNEL.md` §7 is now internally consistent, and `REQ-KERNEL-007`'s statement matches the shape it derives from.

**Status note:** Locked (2026-09-27).

**Evidence:** `ARCH/10-KERNEL.md` §7 (JSON canonical shape) · `ARCH/05-INVARIANTS.md` INV-11 · `crates/everyaios-types/src/envelope.rs` `ActorContext` · `crates/everyaios-types/tests/envelope.rs`.

**Affects:** `10-KERNEL`, `08-REQUIREMENTS`, `07-CONTRACTS`, `05-INVARIANTS`.

## 3. Pending decisions

| ID | Decision needed | Inform by | Affects |
|---|---|---|---|
| PEND-01 | ~~MCP era policy~~ → resolved as DEC-030 | ✅ `lib-4` (2026-09-26) | 14 |
| PEND-02 | ~~Compaction strategy priority + cache-stability rules~~ → resolved as DEC-027 | ✅ `gen-21` (2026-09-26) | 16 |
| PEND-03 | ~~Scheduler lanes + global limits~~ → resolved as DEC-031 | ✅ `11-WORK` (2026-09-26) | 11, 15 |
| PEND-04 | First-release surfaces (desktop + CLI minimum? ACP timing) | 32, SPEC | 32 |
| PEND-05 | Agent profile / "assistant" composition model naming | 15, UI doc | 15 |
| PEND-06 | ~~Memory encryption at rest (SQLCipher vs plaintext; item-level for confidential)~~ → resolved as DEC-039 | ✅ `17-MEMORY` (2026-09-26) | 17 |
| PEND-07 | ~~Artifact storage layout + retention policy~~ → resolved as DEC-032 | ✅ `29-ARTIFACTS` (2026-09-26) | 29 |
