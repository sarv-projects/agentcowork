# 15 — Agent plane (engine contract, delegation)

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P2). **Amended 2026-09-27 (DEC-052):** the first-party native agent is removed; this document now specifies the agent plane only. **Scope clarification (DEC-063, 2026-09-29):** shared Core calls use the governed path; native external-agent effects remain under native policy and distinct provenance.
> **P7 pass (2026-09-26):** line-checked; requirements seeded (now `REQ-AGENT-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** the agent plane — the adapter contract each compatible external engine implements and the obligations AgentCowork holds over shared calls. No first-party reasoning engine is owned here (DEC-052). Engines keep native tools and policy; the same Core Guard applies to their shared Core calls (DEC-054, INV-12).
> **Dependencies:** `11-WORK`, `16-CONTEXT`, `17-MEMORY`, `13-CAPABILITY`, `14-PROVIDERS`, `18-MODEL-ROUTING`, `19-RUNTIME-ENVIRONMENTS`, `12-TRUST`.
> **Evidence:** `ARCHIVE/v1-research/agent-harness-verification.md` — 8 VERIFIED / 3 PARTIAL / 0 WRONG (claims, corrections and pinned clone HEADs recorded there) + product-owner brief (2026-09-26). Anchors cited inline.

## 1. Purpose & responsibilities

An **agent engine** is a reasoning runtime that plans and acts. This document specifies what AgentCowork requires of one, and — just as load-bearing — what it refuses to do on an engine's behalf.

**The engine owns:** its own loop and native step admission · its own planning · context **control** for its own turn (selection/ranking/budget/prune/compact/rebuild, DEC-007) · its own tool selection and batching · its own continuation and recovery · how it thinks and what it puts in its prompt. Core can offer bounded context and request a task, but cannot guarantee the private context strategy of an opaque engine.

**Core owns:** implementation and authorization of its shared capabilities (`12`–`14`) · host Work scheduling, budgets and deadlines (`11`) · Core memory (`17`) · context data services (`16` infra) · host workflows (`20`) · its surfaces and UI. The engine retains its native permission decisions, tools, stores and turn loop (DEC-054).

**The line (DEC-054):** an engine is a *peer*, not a privileged Core component. For shared Core capabilities it proposes and Core disposes (INV-01); native agent tools remain under the engine's own policy and environment. It receives Core projections, never another agent's state (INV-11). Every binding takes the same Guard/ticket path **when invoking a shared Core capability** (DEC-010, INV-12).

**Bounded shared loadout, never a flat dump (DEC-028/054, REQ-CAP-001):** Core offers a task-relevant, granted set of shared tool façades. Every Core-mediated effect call resolves through Capability/Guard/Ticket; Core tool outputs are bounded by a preview plus durable artifact ref. Native tools remain the engine's own catalog and choice. Core does not inject or flatten the engine's private MCP/skill/plugin set.

## 2. Peer contract (`AgentEngine`)

This is a **host adapter contract**, not a claim that every external engine implements every method. `46` §2 owns negotiated support: `unsupported` is typed and visible; `resume`, `steer`, `interrupt`, `cancel`, checkpoint and usage are available only when the binding proves them. Host delegation creates another Work through the scheduler, regardless of whether an engine exposes native subagents.

```
createSession(options) → AgentSession
resumeSession(id) → AgentSession | unsupported
run(session, input) → RunHandle
steer(session, input) → void | unsupported    interrupt(session) → void | unsupported
cancel(run) → void | unsupported
spawnSubagent(options) → SubagentRef   // host Work delegation via CTR-021 (§7); not an assumed native engine method
dispose(session) → void
```

Adapter-split discipline (verified reference: DeepSeek Harness, §D1):

- Core's registry holds **factories**; the driver registers itself (`setFactory` pattern). The owner receives a capability-only `AgentHandle` (`dispose()`), never direct state.
- Host-accepted input arrives through a **durable inbox projection** over session events. Its delivery and external engine acknowledgement are separate; a crashed opaque process may require replay/reconciliation and may not support native resume.
- Host-offered tools / context overlays / listeners are **scope-tagged per binding** where the adapter supports attachment. The host does not rewrite private prompt sections or native registries.

**Evidence:** `agent-harness-verification.md §D1`; anchors `clone2/deepseek-harness/packages/core/agent/src/index.ts:160-203, 245-256` · `agent-loop/src/inbox.ts:27-65` · `scope/src/index.ts:1-40` · `system-prompt/src/index.ts:370-387`.

## 3. Session model

`AgentSession` is a host binding record: handle · negotiated support · durable input/outcome projection · shared capability scope · event stream · Work reference · optional external session reference. Private context, native memory, native loop driver and native tool scope remain inside the engine.

- **Admission boundaries:** Core delivers accepted input at the adapter's negotiated safe point. `next-turn`/`next-step` semantics are promised only if that engine/protocol supports them; otherwise the input queues for the next supported turn.
- **Durable log:** Core session events and mediated effects are append-only projections; an external engine's private prompt history and compaction remain opaque. Core never requires that transcript to reconstruct a Mission.
- Core owns its binding/session record (`11-WORK`), while the engine owns its native session state. An opaque engine's unsupported resume is never represented as successful recovery.
- **Host-created child Work:** carries `parent_session_id?` and its own host log projection. Agent-native children stay native; they are not fabricated as Core child sessions or Work (§7).

## 5. Context control

The engine owns context **control**; Core owns context **data** offered to it (DEC-007; `16-CONTEXT`). The following budget/compaction techniques are adapter capabilities or guidance for engines exposing control, never mandatory rewrites of an opaque engine's private context:

- Budget discipline per DEC-027: named terms (`keep` ≈ 8k retained recent tokens · `buffer`/`reserve` ≈ 20k safety margin · summary output reserve), pre-turn feasibility check, stable-prefix/dynamic-suffix assembly, bounded fragments with persisted baseline + deltas (§A2 / §E1).
- Pipeline: retrieve → select/rank → budget → prune → checkpoint → compact-if-needed → pack.
- Manual control: focus / pin / exclude / inspect appear only when the binding advertises and proves support (`48` §3; `16` §4.1).
- **Subagent context isolation:** each child assembles its own context; `fork_context` is an explicit per-spawn option (default: fresh + bounded inherited snapshot) — never the parent's full transcript (§B3).
- **Memory boundary:** Core's `memory.recall()` uses actor-derived scopes and ceilings. A discovered engine may have its own private memory/notes; these are outside Core's store, cannot be silently imported or governed by Core, and are disclosed as a separate custody boundary. Core never writes or mutates another agent's native memory/config/session files (DEC-043/054).

## 7. Delegation & subagents

Per DEC-029 (evidence §A3 / §B3 / §E7), the table and lifecycle below specify **host-created delegated Work**. A discovered agent's native children remain agent-owned and may expose only reported/observed telemetry (DEC-054):

| Aspect | Rule |
|---|---|
| Child sessions | One child session per subagent, own context, own toolset/persona — never a forked prompt inside the parent. |
| Project rules | Delivered to host-created children as a bounded, escaped task packet; the child engine decides how to use them. Do not promise full-rule injection when an adapter lacks it. |
| Isolation | `shared-read`, `worktree` and adapter-specific sandbox modes are requested per host spawn and accepted only when available; parallel writers require isolated checkouts or serialized write leases. An ACP child is another bound external agent through `32`; Core tickets apply only to its shared calls. Native permission prompts remain native unless a proven adapter routes them; host mediated approvals use Trust. Receipts are schema-validated; an unavailable output schema yields a labelled report, never invented evidence. |
| Context | `fork_context` explicit; default fresh + bounded inherited snapshot. |
| Return value | **Worker receipt** (status · scope · summary · findings · changed files · tests · artifacts · blockers · confidence · usage · `will_wake` · `partial`) — never the transcript. |
| Bounds | Core enforces host spawn concurrency, Work admission and its own mediated spend. Token/step limits inside an opaque external engine are only enforceable when adapter support or environment containment is proven; otherwise they are estimates/warnings. |

### 7.1 Optional shared capability guidance (DEC-054)

An external engine retains its native tool choice. Core describes only available, task-relevant shared capabilities and their grants through a supported session overlay. The agent may prefer a native tool; Core records the path actually used and the associated assurance class. The currently shipped mandatory `COWORK_AFFINITY_STEERING` block and its assertion in `scripts/check-prompt-steering.mjs` conflict with this target and must be retired together during implementation. Neither prompt text nor capability ranking grants access.

Host delegation modes are `automatic`, `preferred`, `manual` and `disabled`, with task/capability/budget routing rules. The lead sees a compact worker catalog, not every worker prompt. A review queue is a host product feature if built, not an assumed external-agent capability. Agent-native child sessions remain owned by that agent; the host lifecycle and receipt contract below apply only to **host-created** child Work. Native child progress is displayed as reported/observed if exposed, with its provenance and missing controls shown honestly.

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
- **Host child records are durable:** Core-observed events and receipts survive parent compaction. A private child transcript remains with its engine and is available for resume/steer only if negotiated. Receipt delivery is deduplicated per parent incarnation; reported usage is marked estimated when the adapter cannot provide actuals.

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
  context: { fork: none | bounded           // host packet only; never native transcript
             refs[] }                        // ≤ declared ref budget; never the transcript
  tools?: loadout_ref                        // child tool scope = parent ceiling ∩ loadout ∩ agent rules
  model?: ModelRef | inherit                 // only when bound engine accepts model selection
  isolation: shared_read | worktree | adapter_sandbox
  limits?: { max_steps?, max_tokens?, max_spend?, wall_time_ms? }   // may narrow, never widen Core bounds
  delivery: { await?: bounded(ms) | none, wake: bool = true, surface: parent | ui }
}
→ SubagentRef { agent_id, nickname?, session_ref, work_id, status, parent_turn_id }
```

Host admission/delegation fields (`max_parallel` · `max_total_per_tree` · `max_depth` · per-lane concurrency) are Core-owned (`11` §3; DEC-031). Token/spend caps for an opaque engine are enforceable only if its adapter reports and honors them or the execution environment supplies a hard boundary. A spawn may request narrower limits only.

### 7.2 User-configured host subagent roster and model pin

`DelegationPolicyEntry` is the user's allowlist for host-created child Work, not a prompt suggestion. Settings → **Agents & models → [agent] → Subagents** contains one off-by-default `Available as a subagent` switch per binding. On enable, configure role(s), a provider-qualified model pin (or explicit `agent-managed` mode), compatible task types, scoped skill/MCP loadout ceiling, workspace/isolation, max parallel/depth and budget ceiling. Saving previews the effective policy; turning it off prevents new child admission and lets the user choose whether existing children finish or cancel.

At spawn, the selected roster entry provides the worker binding and fixed model policy. A request may narrow role/resources/budget, but cannot choose a different model from the pinned one, activate a disabled worker, or widen skills/MCP/permissions. If the adapter accepts a per-session model override, bind that exact model before creating the child and record requested + observed model in the Run/receipt. If it cannot set the model, a pinned-model profile is ineligible; it must not silently fall back. `agent-managed` is a distinct mode, allowed only when enabled in the profile and surfaced to the user as unknown/agent-selected model.

The same profile may be used as both lead and child. This creates a **new child session and context**; it does not fork the lead's private transcript or reuse the lead session. A child model may differ from the lead model only when that engine advertises and verifies model selection per session. Otherwise the child uses the profile's supported fixed/default model, with that constraint shown before enablement. Native subagents spawned inside the agent's own loop remain the engine's responsibility and are not created, configured or suppressed by this host roster.

The UI's fast **Switch agent** action changes only which binding receives the next turn in the same logical host Session (`46` §2.1). It does not change a host child's worker selection or mutate a running Work. Model and extension grants are captured per turn/Work, so switching the lead does not retroactively alter active or completed children.

## 9. Model interaction

- An external engine retains its own provider, model and reasoning configuration. Core's model picker calls a proven adapter configuration option when available; otherwise the engine opens its own supported settings/auth flow (`46` §2, `48` §3).
- Core-owned utility inference (memory extraction, vision, embeddings) routes through `18`; that router does not intercept the external engine's private model calls.
- Stable-prefix caching, baseline/deltas and compaction apply to Core-owned inference/context packets or adapter-supported controls; private cache behavior is not claimed.

## 10. Permissions

Defaults for **Core-mediated calls** (owner brief): **everyday allow** — workspace read/write/edit, normal commands and tests, normal deps, local git read/write, browser navigation, office editing, MCP reads · **dangerous ask** — permanent deletion, destructive shell, credential access, OS/security changes, disk operations, mass external writes, destructive git · **full access** — user-activated, with an irreducible catastrophic-operation gate. Core enforcement lives in Guard (`12-TRUST`) across **three layers** (DEC-028); the external agent's native policy remains separate and the UI labels it.

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
| Model call fails/timeouts | Host receives adapter status where available; native retry remains engine-owned. Work records a typed reported/observed failure or uncertainty and offers the supported retry path. |
| Tool failure | Recovery pipeline (retry / alternate provider / replan). |
| Context overflow | If adapter supports recovery, request it and reconcile; otherwise surface the engine's failure and preserve Mission/Work state for a fresh supported session. |
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
5. Engine-private notes vs Core memory — **resolved (DEC-043/054):** Core has one owned memory store and never imports or rewrites an external engine's separately owned private memory/config/session files.
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
