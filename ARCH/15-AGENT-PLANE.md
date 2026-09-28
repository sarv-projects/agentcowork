# 15 — Agent plane (engine contract, delegation)

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P2). **Amended 2026-09-27 (DEC-052):** the first-party native agent is removed; this document now specifies the agent plane only.
> **P7 pass (2026-09-26):** line-checked; requirements seeded (now `REQ-AGENT-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** the agent plane — the contract **every** agent engine implements and the obligations AgentCowork holds over it. No first-party engine ships in v1 (DEC-052): engines are external, and a first-party engine developed outside this repository is bound afterwards as an ordinary binding — same `AgentEngine` contract, same Guard, no privileged path (DEC-010, INV-12).
> **Dependencies:** `11-WORK`, `16-CONTEXT`, `17-MEMORY`, `13-CAPABILITY`, `14-PROVIDERS`, `18-MODEL-ROUTING`, `19-RUNTIME-ENVIRONMENTS`, `12-TRUST`.
> **Evidence:** `ARCHIVE/v1-research/agent-harness-verification.md` — 8 VERIFIED / 3 PARTIAL / 0 WRONG (claims, corrections and pinned clone HEADs recorded there) + product-owner brief (2026-09-26). Anchors cited inline.

## 1. Purpose & responsibilities

An **agent engine** is a reasoning runtime that plans and acts. This document specifies what AgentCowork requires of one, and — just as load-bearing — what it refuses to do on an engine's behalf.

**The engine owns:** its own loop and step admission · its own planning · context **control** for its own turn (selection/ranking/budget/prune/compact/rebuild, DEC-007) · its own tool selection and batching · its own continuation and recovery · how it thinks and what it puts in its prompt. These are the agent-native plane (`ARCH/02-THESIS.md`) and are **not** ours to specify.

**Core owns, always:** capability implementation (`13`/`14`) · permissions and every authorization decision (`12`) · work scheduling, budgets and deadlines (`11`) · the durable memory store (`17`) · context data services (`16` infra) · deterministic multi-step processes (`20`) · the CLI/ACP surfaces it is reachable through · the UI.

**The line (DEC-054):** an engine is a *peer*, not a privileged Core component. For shared Core capabilities it proposes and Core disposes (INV-01); native agent tools remain under the engine's own policy and environment. It receives Core projections, never another agent's state (INV-11). Every binding takes the same Guard/ticket path **when invoking a shared Core capability** (DEC-010, INV-12).

**Bounded tools, never a flat dump (DEC-028, REQ-CAP-001):** an engine sees a bounded, Guard-mapped set of task-shaped tool façades — never a flattened dump of every MCP, plugin or native tool. Every effect-bearing call resolves through the capability/Guard/ticket path, read-only tools carry path scopes, and tool outputs are bounded (a preview plus a durable artifact ref; lossy success is forbidden — DEC-032). **How** an engine loads, searches and batches its tools is the engine's own design; the long tail is reachable, the wire is never flooded.

## 2. Peer contract (`AgentEngine`)

```
createSession(options) → AgentSession
resumeSession(id) → AgentSession
run(session, input) → RunHandle
steer(session, input) → void        interrupt(session) → void
cancel(run) → void
spawnSubagent(options) → SubagentRef   // immediate spawn (DEC-036); full contract: CTR-021 (§7)
dispose(session) → void
```

Adapter-split discipline (verified reference: DeepSeek Harness, §D1):

- Core's registry holds **factories**; the driver registers itself (`setFactory` pattern). The owner receives a capability-only `AgentHandle` (`dispose()`), never direct state.
- Input arrives through a **durable inbox projection** over session events — not by direct mutation; a crashed process loses nothing accepted.
- Tools / prompt sections / listeners are **scope-tagged per agent** (per-binding variance without global registries).

**Evidence:** `agent-harness-verification.md §D1`; anchors `clone2/deepseek-harness/packages/core/agent/src/index.ts:160-203, 245-256` · `agent-loop/src/inbox.ts:27-65` · `scope/src/index.ts:1-40` · `system-prompt/src/index.ts:370-387`.

## 3. Session model

`AgentSession` (internal shape): handle · session state · inbox (durable projection) · context (control side) · tool scope · capability scope · event stream · memory scope · work scope · loop driver.

- **Admission boundaries:** `next-turn` (user messages) vs `next-step` (injected context / tool results). Injected inputs wait for a wake; they never interleave mid-step.
- **Durable log:** session events are append-only; UI history, prompt history and pending work are **projections** (§E4). Compaction is a projection boundary (DEC-027), never a rewrite.
- Session records are Core-owned (`11-WORK`); an engine reads/writes through contracts only.
- **Child sessions:** a subagent child is a normal durable session with `parent_session_id?` linkage; its log projections are independent of — and survive — parent compaction, and its state is a facet of its `Work` item (`kind: subagent_task`), never a second state machine (§7).

## 5. Context control

The engine owns context **control**; Core owns context **data** (DEC-007; `16-CONTEXT`).

- Budget discipline per DEC-027: named terms (`keep` ≈ 8k retained recent tokens · `buffer`/`reserve` ≈ 20k safety margin · summary output reserve), pre-turn feasibility check, stable-prefix/dynamic-suffix assembly, bounded fragments with persisted baseline + deltas (§A2 / §E1).
- Pipeline: retrieve → select/rank → budget → prune → checkpoint → compact-if-needed → pack.
- Manual control: focus / pin / exclude / inspect (surfaced in UI; `AGENTCOWORK-UI.md`).
- **Subagent context isolation:** each child assembles its own context; `fork_context` is an explicit per-spawn option (default: fresh + bounded inherited snapshot) — never the parent's full transcript (§B3).
- **Memory boundary:** recall via `memory.recall()` from `17` (scopes and ceilings actor-derived, never caller-supplied); an engine's private working notes live as **session-scope memory items + the session log** — there is **no second durable memory store**, and Core never writes or mutates another agent's native memory/config/session files (DEC-043).

## 7. Delegation & subagents

Per DEC-029 (evidence §A3 / §B3 / §E7):

| Aspect | Rule |
|---|---|
| Child sessions | One child session per subagent, own context, own toolset/persona — never a forked prompt inside the parent. |
| Project rules | Delivered **in full** to children, escaped so repository content cannot forge harness framing (`prompt/context.rs:152,196`; `agents_md.rs:382`). |
| Isolation | `inprocess` (default), `worktree`, and `acp` are **per-spawn options**; cheap read-only children don't pay worktree cost; concurrent writers get isolated checkouts + write leases. ACP children run as external provider-executed agents through the `14` acp adapter and gateway (`32` §4) under a scoped capability projection (Core tickets never cross the boundary), with permission prompts routed through the parent's approval channel and the same concurrency bound (DEC-029/031); receipts are schema-validated with at most one bounded correction retry before a raw-text fallback with a typed note (REQ-AGENT-003). |
| Context | `fork_context` explicit; default fresh + bounded inherited snapshot. |
| Return value | **Worker receipt** (status · scope · summary · findings · changed files · tests · artifacts · blockers · confidence · usage · `will_wake` · `partial`) — never the transcript. |
| Bounds | Platform enforces outer limits (max parallel · total · depth · tokens · spend); the running agent decides actual usage within them. |

### 7.1 Capability-affinity steering (superseded by DEC-054)

**Target contract:** A discovered external engine retains native tool choice. The four mandatory rankings below describe the currently shipped prompt block and its current assertion, **not** the target behavior. Replace it with a bounded, task-specific description of available shared capabilities and their actual grants. The engine may choose native tools. Show the selected path and its governance class. Do not claim `office.*`, `browser.*`, `computer_use.*` or `delegate.*` is always superior to an engine-native operation. The prompt-steering assertion must be revised with the implementation; until then this is a documented code/spec gap.

A bound engine that can reach the shared cowork plane is told, in its own prompt, that the native capability façades outrank the generic shell. This is the published form of `COWORK_AFFINITY_STEERING` (`crates/agentcowork-acp/src/chief.rs`, asserted by `scripts/check-prompt-steering.mjs`), in the same order:

1. **Spreadsheets (`.xlsx`) & documents (`.docx`/`.pptx`)** — always `office.*` (`office.open`, `office.inspect`, `office.edit`, `office.calculate`); never hand-written Python or CLI tools in a shell to modify office files.
2. **Web browsing & research** — always `browser.*` (`browser.research`, `browser.operate`, `browser.extract`) rather than raw `curl` or a headless script.
3. **Desktop UI automation** — `computer_use.*` (`computer_use.see`, `computer_use.act`).
4. **Subagent delegation** — `delegate.spawn`, for subtasks that belong in an isolated child.

The order is a contract, not formatting: the ranking is what the block exists to express, so reordering it is a behavior change, and `check-prompt-steering.mjs` fails when the document and the shipped constant disagree. The block is **steering, not authority** — it ranks capabilities inside a projection the engine already holds, so an engine without the Office façade is never told to use one. Tool exposure itself is `13` (capability resolution, DEC-025 native-first) and `12` (Trust); nothing here grants a capability.
| Modes | `automatic` · `preferred` · `manual` · `disabled`, plus routing rules (task type / language / capability / cost). |
| Main context | Sees only a compact worker catalog (role, skills, model, relative cost) — never each worker's full prompt. |
| Review queue | **Not borrowed from Codex** (absent from its source, §A3). If wanted, it is our own product-layer build (OQ-AGENT-01). |

Overlapping writes go through workspace leases (queue / rebase / ask) — never silent overwrite.

**Async lifecycle (absorbed wave 2 — `DEC-036`):**
- **Spawn returns immediately** — `{agent_id, nickname?, session_ref, work_id, status, parent_turn_id}`; spawn is never coupled to child completion unless a bounded `await` is requested.
- **Two completion modes:** a **bounded foreground wait** (declared tiers; used sparingly) or a **queue-only wake at a turn boundary** (`next-turn`/`next-step`) — completion is admitted as a typed `subagent.finished` event plus a queued prompt only if the parent is live and the child was not cancelled.
- **Wake-suppression gate:** `backgrounded && !cancelled && wake_enabled && !block_waited && !explicitly_killed && !goal_loop_active && parent_channel_open`; **a cancelled child never wakes the parent**; `will_wake` is explicit so clients never promise a wake that will not happen.
- **Typed child stream:** `subagent.spawned` (emitted before the first prompt dispatch) · `subagent.progress` (≈2 s) · `subagent.finished` (status · error · tool calls · turns · duration · tokens · output · `will_wake`).
- **Bounded waits auto-background** — a wait that exceeds its budget moves the child to the background lane instead of freezing the parent turn.
- **Concurrency:** slots are **held until closed** (not just until finished); admission is queue-on-limit by default with a `fail` opt-in; per-lane defaults + depth are declared and enforced via `11` (DEC-031).
- **Cancellation:** cooperative and token-based — parent cancel ⇒ child cancel; session teardown ⇒ cancel with **no completion rebuffer**; explicit close cascades to descendants; cancelled runs are terminal and never wake; queued spawns are swept within a bounded interval.
- **Report trust:** child receipts are **untrusted data** — scanned for instruction-shaped patterns and delivered under a no-authority header; background completion notices are framed as automated events, never as messages. (The studied harnesses consume child output as trusted; this divergence is deliberate — DEC-036.)
- **Child sessions are durable:** child transcripts live in their own log projections, survive parent compaction, and are addressable by `agent_id` for resume/steer; receipt delivery is at-most-once per parent incarnation, size-capped with a full-log artifact ref; `usage` rolls up to the parent.

**Child lifecycle mapping (no second enum — INV-06).** Child work items are `Work` of `kind: subagent_task` (`11` §2); subagent state is a facet of that state machine, not a parallel machine:

| Child facet | Derived from | Notes |
|---|---|---|
| `pending` | Work `queued` (admitted, not started) | spawn returned; slot held (DEC-036) |
| `running` | Work `running` | `subagent.spawned` emitted before the first prompt dispatch |
| `waiting` | Work `waiting`/`paused`/`awaiting_approval` | Guard ASK / question / bounded wait parked |
| `interrupted` | active Step settled `failed{reason:"interrupted"}`, Work returns admissible | `interrupt` only; session survives |
| `completed` / `failed` | Work terminal | receipt emitted once; wake per gate |
| `cancelled` / `expired` | Work terminal | never wakes; teardown not rebuffered |
| `closed` | explicit close after terminal | releases the concurrency slot (held-until-closed) |

**Spawn options (`SubagentOptions`, CTR-021 extension; DEC-029/036):**

```
SubagentOptions {
  worker: AgentProfileRef | role            // required
  task: { objective, prompt?, success_conditions? }
  context: { fork: none | bounded | full    // default per role (OQ-AGENT-02)
             refs[] }                        // ≤ declared ref budget; never the transcript
  tools?: loadout_ref                        // child tool scope = parent ceiling ∩ loadout ∩ agent rules
  model?: ModelRef | inherit                 // default inherit → 18 router
  isolation: inprocess | worktree | acp      // default inprocess
  limits?: { max_steps?, max_tokens?, max_spend?, wall_time_ms? }   // may narrow, never widen Core bounds
  delivery: { await?: bounded(ms) | none, wake: bool = true, surface: parent | ui }
}
→ SubagentRef { agent_id, nickname?, session_ref, work_id, status, parent_turn_id }
```

Depth/budget fields (`max_parallel` · `max_total_per_tree` · `max_depth` · `max_worker_tokens` · `max_session_spend` · per-lane concurrency) are **Core-owned** (`11` §3; DEC-031): a spawn may request **narrower** limits only.

## 9. Model interaction

- Asks `18-MODEL-ROUTING`; never hard-codes a vendor. Reasoning effort is a normalized dial mapped to provider capabilities.
- Cache discipline: stable prefix, dynamic suffix; persisted baseline + deltas (§A2).
- Extractor/vision/embedding calls also route through `18` — no side-channel SDK usage anywhere in the agent.

## 10. Permissions

Defaults (owner brief): **everyday allow** — workspace read/write/edit, normal commands and tests, normal deps, local git read/write, browser navigation, office editing, MCP reads · **dangerous ask** — permanent deletion, destructive shell, credential access, OS/security changes, disk operations, mass external writes, destructive git · **full access** — user-activated, with an irreducible catastrophic-operation gate. Enforcement lives in Guard (`12-TRUST`) across **three layers** (DEC-028); the agent requests, never decides.

## 11. Surfaces

| Surface | Shape |
|---|---|
| UI | Session conversation + plan/status + composer (via `32-CHANNELS`; UI doc owns rendering). |
| CLI | `agentcowork` (placeholder) — `-p "prompt"`, `--workspace`, `serve --acp`. |
| ACP server | Exposes an engine like any other agent: session manager, tool registry, typed updates (§B1/§B2 basis). |
| Interop | Agents reach each other through Core's gateway (`32`); a given engine is reached through its `14` adapter. |

## 12. Failure modes

| Failure | Behavior |
|---|---|
| Model call fails/timeouts | Bounded retry → block with surfaced reason + retry path. |
| Tool failure | Recovery pipeline (retry / alternate provider / replan). |
| Context overflow | Compact-after-overflow → retry same step; never "start a new conversation". |
| Subagent fails/blocks | Receipt with blockers; parent re-plans or escalates. |
| Stuck loop | Stuck detector → escalation. |
| Memory/extractor failure | Turn unaffected (`17`). |
| Guard returns ASK | Pause via approval primitive; state durable across the wait. |

## 13. Interop

**Depends on:** `11` (work/sessions) · `16` (context infra) · `17` (recall) · `13`/`14` (capabilities/providers) · `18` (models) · `19` (environments) · `12` (guard/approvals).
**Exposes to:** `11` (agent engine) · `20` (agent nodes) · `32` (CLI/ACP/UI projections) · `13` (capability requests).
**DAG check:** an engine calls services; services never call an engine except through work orchestration (`20`) or the delegation contract.

## 14. Open questions (`OQ-AGENT-*`)

1. Review-queue scope for v1 (product-layer build; ties to the UI Runs surface).
2. `fork_context` default per worker role (researcher / coder / reviewer).
3. Persona/assistant composition model (PEND-05).
4. CLI binary name + command surface (with `32`).
5. Engine-private notes vs Core-only memory — **resolved (DEC-043):** the Core store is the only durable memory; private notes are session-scope items + the session log, not a second store and not importable.
6. Which model-specific compaction hooks ship in v1 (with `16`, `18`).

## 15. Evidence

`ARCHIVE/v1-research/agent-harness-verification.md` — §A1–A4 (Codex) · §B1–B3 (Grok Build) · §C1–C3 (OpenCode) · §D1 (DeepSeek Harness) · §E (cross-cutting patterns) · §F (unverified: Codex review queue, OpenCode native compaction, OpenCode V2 pruning execution).
Corrections recorded: Codex review queue **absent**; OpenCode **no native compaction path** (both generations summarize with the model); OpenCode pruning is **V1-only**; Codex worktrees are session-bound in the desktop layer, not automatic per subagent.
Pinned clone HEADs: codex `13a966fc` · grok-build `4247f661` · opencode `fe3f3a41` · deepseek-harness `c36a83ff`.

## 16. Requirements (`REQ-AGENT-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-AGENT-001` | Delegation contract: the delegated item is first-class `Work`, with named engine, scopes and deadline |
| `REQ-AGENT-002` | Subagent spawn/completion: observable lifecycle, receipt-bearing terminal record, scheduler-tracked when async |
| `REQ-AGENT-003` | Isolation modes are explicit per spawn and recorded; an unavailable mode is refused, never downgraded |
| `REQ-AGENT-004` | Results are receipts, not transcripts; an unverified engine report is labelled as a report |
