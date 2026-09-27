# AgentCowork — Product Specification (SPEC)

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P5). **Authority:** root for **WHAT** the product must be (`ARCH/00-INDEX.md` §2). HOW lives in `ARCH/03-HLD.md` and the module docs; schemas in `ARCH/06-DATA-MODEL.md`/`07-CONTRACTS.md`; flows in `ARCH/40-FLOWS.md`.
> **P7 pass (2026-09-26):** line-checked; requirements registry (`ARCH/08-REQUIREMENTS.md`) cross-referenced.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Names:** product **AgentCowork** (working), runtime **Core**; the agent engine is external in v1 (`ARCH/01-NAMING.md`).
> **v1.0 scope:** Windows-first desktop, local-first, single-user. No scope cuts carried from here — deferrals are explicit (§15).
> **SDD:** testable behaviors derived from this SPEC are registered as `REQ-*` in `ARCH/08-REQUIREMENTS.md`; traceability accrues in `ARCH/09-FEATURE-MATRIX.md`; process: `.agents/docs/spec-driven-development.md`.

## 1. Definition

**AgentCowork is a local-first AI work environment that composes interchangeable agents, models, capabilities and execution environments behind one governed execution model, on top of a continuously updated model of the user's digital world.**

Working positioning: *one workspace, every model, every tool.* It is an AI-native execution layer on the user's existing computer — **not** an OS/kernel replacement.

## 2. Normative principles

| # | Principle |
|---|---|
| P-01 | Core is the brain; every surface is a projection. |
| P-02 | Work is the universal execution abstraction. |
| P-03 | Agent ≠ Model ≠ Provider. |
| P-04 | Capability describes *what*; provider describes *who*. |
| P-05 | Protocols (MCP/ACP/CLI/HTTP/plugins) are adapters, never the center. |
| P-06 | One governed path for every externally visible effect; control path bounded, effect path asynchronous. |
| P-07 | Artifacts and receipts are first-class, durable, versioned. |
| P-08 | Domain runtimes own specialized complexity; the kernel stays small. |
| P-09 | Context: infrastructure in Core, control in the agent. |
| P-10 | Memory ≠ context. |
| P-11 | Workflow is deterministic; agent is adaptive; both compose. |
| P-12 | The computer explains itself; vision is the fallback rung. |
| P-13 | Enforcement lives in Core; prompts never enforce. |
| P-14 | Token discipline: deterministic operations never touch an LLM. |

(Teaching narrative: `ARCH/02-THESIS.md`.)

## 3. Surfaces contract

Desktop app (primary), CLI (`agentcowork`, placeholder), IDE via ACP, Work API, mobile-later. All surfaces are projections of the same Core and the same `AgentEngine` contract; none is a second brain. External agents connect only through the Agent Gateway and receive the 7-item projection (identity · capabilities · context · workspace · tools · artifacts · events). → `ARCH/32-CHANNELS.md`.

## 4. Governed execution contract

Every externally visible effect: `Work → Capability → Provider → Handle → Guard → Ticket → Execute → Effect → Verify → Receipt → Event`. Control path: p50 < 2 ms · p95 < 10 ms · p99 < 25 ms (bounded work only); effect path: asynchronous and observable. No bypasses — not for domains, adapters, UI, or any engine. Verification depth scales with risk class; receipts are mandatory for visible effects. → `ARCH/03-HLD.md` §5, `12`, `13`, `34`, `29`.

## 5. Capability contract

- Capabilities are **semantic operations** (`office.spreadsheet.edit`); providers implement them; transports are invisible above the Capability Plane.
- Descriptor / result / handle shapes are canonical (`DM-011/012`; `13` §2–§4). `guidance` and `requires_user_action` are first-class results with concrete `next_action`.
- Models see task-relevant capability subsets under budget — **never** a flat dump of raw tools (semantic compression; loading modes eager/catalog/on-demand).
- Handles are epoch-checked; provider restarts invalidate them (`DM-012`, `ARCH/13-CAPABILITY.md` §4).

## 6. Trust contract

- **One decider** (Guard): `ALLOW / ASK / DENY` composed of three layers — platform confinement × approval policy × declarative exec rules (`DEC-028`).
- **Vault custody:** credentials exist only in the vault; `use`-style API; never in prompts/logs/events (`INV-02`).
- **Egress:** one guarded path; fail closed (`INV-05`).
- **Permission defaults:** everyday allow · dangerous ask · Full Access with an irreducible catastrophic gate (`12` §3).
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

- Composer controls are **capability-negotiated** per selected agent (no fake controls): agent · model · reasoning dial · context indicator; `@` opens a structured reference picker; `/eaios:*` is the reserved host namespace and the selected agent keeps its native commands untouched; `+` attaches/creates; `Run ▾` offers now/background/workflow/schedule.
- Chat rendering: markdown pipeline; mermaid auto-conversion (policy-gated, isolated); tool calls with state model; plan bar; reasoning dial — **never raw chain-of-thought**.
- Universal DocumentSurface: files open as tabs regardless of type; Office/Browser are runtimes under it.
- Token discipline: opening/rendering/navigating/previewing never call a model (`AGENTCOWORK-UI.md` owns detail; `DEC-015`).

## 10. Workflow & automation contract

- Typed IR; published vs draft; runs pinned; approvals first-class; workflows-as-tools; agent-authored workflows with validation.
- Durability: append-only journal; occurrences; leases; exactly-once claims; resume matrix; `needs_attention` for keyless side effects; misfire policy.
- v1 trigger set: manual · schedule · agent-call · sub-workflow · workflow failure · Core/World events. Browser-event and completion-chained triggers are declared product inventions if ever shipped. → `20`.

## 11. Multi-agent contract

- Every agent engine is a peer: same `AgentEngine`, same Guard, no privileged path — including a first-party engine bound later (`DEC-010`, `DEC-052`).
- **No first-party engine ships in v1.** The reasoning engine is developed outside this repository and is bound here afterwards as an ordinary engine binding: same contract, same Guard, same projections, no privileged path (`DEC-052`).
- Delegation: child session per subagent; full escaped project rules; per-spawn worktree option; **receipts, not transcripts**; outer bounds enforced by Core (`DEC-029`, `DEC-031`).
- Scheduler lanes: foreground · background · detached, with interactive priority.

## 12. Extension contract

Skills teach (activation-scoped, relevance-loaded); plugins extend at declared surfaces only, through a review gate, sandboxed, no Core patching; licensing rules from `44` apply to any reuse. → `31`.

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
