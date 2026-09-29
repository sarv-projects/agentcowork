# AgentCowork — Product Specification (SPEC)

> **DEC-054/055/060/061/062 target amendment (2026-09-29):** The frozen v1 text below records its historical baseline. The final product contract now includes durable Missions, agent-native ownership, scoped ecosystem, the outcome-first Experience, the official Rust MCP SDK behind Core policy, and one shared capability-selection preference in `ARCH/35-MISSION.md`, `ARCH/46-ECOSYSTEM-ARCHITECTURE.md`, `ARCH/48-EXPERIENCE-SURFACES.md` and `ARCH/50-SYSTEM-BLUEPRINT.md`. Queued chat items persist their structured payload but resolve the latest valid conversation binding at dequeue; unavailable selections remain typed `needs_resolution` and never silently fall back. Those accepted decisions supersede contrary baseline wording, including absolute governance of self-contained external-agent native effects, a first-party Native engine, scope-based deferrals, and conflicting action-ladder ordering. The target is a local/cloud capable architecture; actual cloud execution and every adapter require implementation evidence. `ARCH/08-REQUIREMENTS.md` and `TODO.md` carry the amended work. No current superiority claim is made.

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P5). **Authority:** root for **WHAT** the product must be (`ARCH/00-INDEX.md` §2). HOW lives in `ARCH/03-HLD.md` and the module docs; schemas in `ARCH/06-DATA-MODEL.md`/`07-CONTRACTS.md`; flows in `ARCH/40-FLOWS.md`.
> **P7 pass (2026-09-26):** line-checked; requirements registry (`ARCH/08-REQUIREMENTS.md`) cross-referenced.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Names:** product **AgentCowork** (working), runtime **Core**; the agent engine is external in v1 (`ARCH/01-NAMING.md`).
> **Release baseline:** Windows-first desktop, local-first, single-user. This frozen release baseline is not the final capability ceiling: accepted DEC-054/055 architecture includes configured local and remote execution, heterogeneous teams, durable goals and broader Workbench capabilities; each remains unimplemented until its TODO task and acceptance evidence land. Deferrals below describe delivery sequencing, not product-value cuts.
> **SDD:** testable behaviors derived from this SPEC are registered as `REQ-*` in `ARCH/08-REQUIREMENTS.md`; traceability accrues in `ARCH/09-FEATURE-MATRIX.md`; process: `.agents/docs/spec-driven-development.md`.

## 1. Definition

**AgentCowork is a local-first AI work environment that accepts durable goals and composes interchangeable external agents, models, capabilities, workflows and execution environments around one shared mission and outcome record. Core governs the effects that pass through its own capabilities; external agents retain their reasoning loop, native tools and effective policy, with distinct provenance for native activity.**

Working positioning: *one workspace, every model, every tool.* It is an AI-native execution layer on the user's existing computer — **not** an OS/kernel replacement.

## 2. Normative principles

| # | Principle |
|---|---|
| P-01 | Core owns durable Mission, Work and shared-plane truth; each external agent owns its reasoning loop; every UI surface is a projection. |
| P-02 | Work is the universal execution abstraction for execution admitted by AgentCowork; an external agent's native subtasks remain agent-owned unless Core explicitly creates host Work for them. |
| P-03 | Agent ≠ Model ≠ Provider. |
| P-04 | Capability describes *what*; provider describes *who*. |
| P-05 | Protocols (MCP/ACP/CLI/HTTP/plugins) are adapters, never the center. |
| P-06 | One governed path for every Core-mediated externally visible effect; a self-contained external agent's native effects have distinct observed/reported provenance and remain under its own policy. |
| P-07 | Artifacts and receipts are first-class, durable, versioned. |
| P-08 | Domain runtimes own specialized complexity; the kernel stays small. |
| P-09 | Context: infrastructure in Core, control in the agent. |
| P-10 | Memory ≠ context. |
| P-11 | Workflow is deterministic; agent is adaptive; both compose. |
| P-12 | The computer explains itself; vision is the fallback rung. |
| P-13 | Enforcement for Core-mediated capabilities lives in Core; prompts never enforce. Native external-agent policy remains owned by that agent. |
| P-14 | Token discipline: deterministic operations never touch an LLM. |

(Teaching narrative: `ARCH/02-THESIS.md`.)

## 3. Surfaces contract

Desktop app (primary), CLI (`agentcowork`, placeholder), IDE via ACP, Work API, mobile-later. All surfaces are projections of the same Core and the same `AgentEngine` contract; none is a second brain. External agents connect only through the Agent Gateway and receive the 7-item projection (identity · capabilities · context · workspace · tools · artifacts · events). → `ARCH/32-CHANNELS.md`.

## 4. Governed execution contract

Every **Core-mediated** externally visible effect follows `Work → Capability → Provider → Handle → Guard → Ticket → Execute → Effect → Verify → Receipt → Event`. Within that path there are no domain, adapter or UI bypasses. The pipeline does not claim to intercept a self-contained external agent's native tools or effects; those remain under that agent's policy and are labelled as reported or observed when the binding exposes evidence. Control path: p50 < 2 ms · p95 < 10 ms · p99 < 25 ms (bounded work only); effect path: asynchronous and observable. Verification depth scales with risk class; Core-mediated visible effects require receipts. → `ARCH/03-HLD.md` §5, `12`, `13`, `34`, `29`, `46`.

## 5. Capability contract

- Capabilities are **semantic operations** (`office.spreadsheet.edit`); providers implement them; transports are invisible above the Capability Plane.
- Descriptor / result / handle shapes are canonical (`DM-011/012`; `13` §2–§4). `guidance` and `requires_user_action` are first-class results with concrete `next_action`.
- Models see task-relevant capability subsets under budget — **never** a flat dump of raw tools (semantic compression; loading modes eager/catalog/on-demand).
- Handles are epoch-checked; provider restarts invalidate them (`DM-012`, `ARCH/13-CAPABILITY.md` §4).

## 6. Trust contract

- **One decider** (Guard): `ALLOW / ASK / DENY` composed of three layers — platform confinement × approval policy × declarative exec rules (`DEC-028`).
- **Vault custody:** credentials exist only in the vault; `use`-style API; never in prompts/logs/events (`INV-02`).
- **Egress:** one guarded path for Core-mediated outbound calls; fail closed (`INV-05`). External agents' native network calls remain within their own effective policy and are not represented as Core-guarded.
- **Core-mediated permission defaults:** everyday allow · dangerous ask · Full Access with an irreducible catastrophic gate (`12` §3). These defaults do not describe a discovered external agent's native permission policy.
- **Projections only** for external agents; workspace boundaries enforced by interception, not un-discovery (`12` §8).
- Approvals are one primitive for agents and workflows (`DEC-021`).

## 7. Data & state contract

- Canonical entities `DM-001…027` (`06`); interfaces `CTR-001…026` (`07`).
- One writer per store; logs are append-only; every view is a projection (`INV-06`, `INV-23`).
- Checkpoints are reconstructable records (`DEC-027`, `DM-006`); workflow runs pin their version and step attempts carry idempotency keys (`DEC-033`).
- Memory v1: SQLite + FTS5; ADD-only extraction with `superseded_by`; suppression-based forget; **no vectors/graph/decay in v1** (`DEC-018/019`).
- Artifacts: immutable versions, mandatory provenance; receipt-pinned versions never GC'd (`DEC-032`).

## 8. Domain runtimes contract

| Runtime | Headline guarantees | Doc |
|---|---|---|
| Office | L1/L2/L3; resident + lease; staging→fsync→atomic swap; validate before commit; typed ops (not one string tool) | `22` |
| Browser | Managed Chromium default + adapters; CDP snapshot/refs/trusted input; no evasion tooling | `23` |
| Computer Use | Deterministic ladder first; per-platform capability matrix; vision fallback with caps; consent-gated input | `24` |
| Files | Identity `(volume,fileId,incarnation)` / `(dev,ino,nlink)`; watchers with overflow rescan; write leases | `25` |
| Code | RepoGraph → RepoMap under budget; LSP bridge; per-spawn worktrees; governed execution | `26` |
| Search | One kernel implementation; deterministic; scoped; abstention allowed | `27` |
| Comms | Capability layer over connectors; sends approval-gated; no bulk ingest | `28` |
| Web | `web.search`/`web.fetch` capabilities — provider variants (native · MCP · guarded fetch; browser fallback); citations/provenance; fetched content untrusted; caps + TTL cache | `28` (`DEC-037`) |

## 9. Experience contract

- Composer controls are **capability-negotiated** per selected agent (no fake controls): agent · model · reasoning dial · context indicator; `@` opens a structured reference picker; `/cowork:*` is the reserved host namespace and the selected agent keeps its native commands untouched; `+` attaches/creates; `Run ▾` offers now/background/workflow/schedule.
- Chat rendering: markdown pipeline; mermaid auto-conversion (policy-gated, isolated); tool calls with state model; plan bar; reasoning dial — **never raw chain-of-thought**.
- Universal DocumentSurface: files open as tabs regardless of type; Office/Browser are runtimes under it.
- Token discipline: opening/rendering/navigating/previewing never call a model (`ARCH/48-EXPERIENCE-SURFACES.md` owns final interaction detail; `AGENTCOWORK-UI.md` is retained as baseline/source-path evidence; `DEC-015`).

## 10. Workflow & automation contract

- Typed IR; published vs draft; runs pinned; approvals first-class; workflows-as-tools; agent-authored workflows with validation.
- Durability: append-only journal; persisted trigger occurrences with unique logical-run claims; leases; resume matrix; at-least-once step attempts with provider idempotency or observed-state reconciliation; `needs_attention` for uncertain keyless side effects; misfire policy. No exactly-once claim is made for external effects (`DEC-057`).
- v1 trigger set: manual · schedule · agent-call · sub-workflow · workflow failure · Core/World events. Browser-event and completion-chained triggers are declared product inventions if ever shipped. → `20`.

## 11. Multi-agent contract

- Every agent binding uses the negotiated `AgentEngine` contract. Core-mediated capability calls follow the same Guard path for every binding; no binding gets a Core bypass. A self-contained external agent's native loop and effects remain under its own policy and carry separate provenance (`DEC-010`, `DEC-052`, `DEC-054`).
- **No first-party reasoning engine ships in v1.** Horizon Code is a future external binding developed outside this repository and attached as an ordinary agent; it does not replace or own AgentCowork's Mission, Work, Trust or evidence records (`DEC-052`, `DEC-054`).
- Host delegation creates a bounded child session/Work with escaped project rules, optional per-spawn worktree isolation and **receipts, not transcripts**; Core enforces outer bounds on that host-created work (`DEC-029`, `DEC-031`, `DEC-054`). Native subagents remain owned by their agent and are reported only when the binding exposes them.
- Scheduler lanes: foreground · background · detached, with interactive priority.

## 12. Extension contract

Skills teach (activation-scoped, relevance-loaded); plugins extend at declared surfaces only, through a review gate, with no Core patching. Code-bearing host plugins run only under a qualified confinement backend; they are unavailable where required confinement is absent. Content-only skills/templates follow untrusted-content rules and do not gain execution rights. Licensing rules from `44` apply to any reuse. → `31`.

## 13. Evidence & acceptance

- Evidence rules and the acceptance map: `ARCH/42-EVIDENCE-MAP.md`. “Implemented” ≠ verified; Windows readiness requires real acceptance records.
- Code-phase fix register (FIX-01…18) is owned by the code phase (`TODO.md` W0; `ARCH/42-EVIDENCE-MAP.md` §4).

## 14. Non-goals (normative)

No OS replacement · no forced single browser/model/agent/format · no vendoring of third-party projects as dependencies without cleared licensing · no CAPTCHA/anti-bot evasion tooling · no universal chat syntax imposed on agents · no bulk context dumps. The 16 explicit rejections: `44` §2.

## 15. Deferrals (explicit)

A2A transport implementation · remote/cloud environments · mobile surfaces · content indexing (W7) · vectors/consolidation in memory · cross-device sync · marketplace/auto-update for plugins · visual workflow editor · service-backed background nudge for closed apps. Each has a trigger in its module doc.

## 16. Open questions

`OQ-001`–`003` and `OQ-005` remain open; `OQ-004` is done and `OQ-006` resolved (`00-INDEX` §9). `PEND-04…05` (`04-DECISIONS` §3) remain open; PEND-06 is closed by DEC-039 and PEND-07 by DEC-032; product-owner decisions where flagged.

## 17. Change control

Changes to this SPEC or any authority doc require a `DEC` entry; module docs must not contradict the SPEC; conflicts escalate per `00-INDEX` §2. v1 froze at P8 (2026-09-26); post-freeze changes are v1.x via the same process.
