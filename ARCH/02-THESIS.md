# 02 — Thesis

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P0). This doc sets identity and principles; decisions that need evidence land in `ARCH/04-DECISIONS.md`, invariants in `ARCH/05-INVARIANTS.md`.
> **SDD:** the success statements (S-01…S-10) are the falsifiable seeds of the requirement registry; REQ traceability accrues in `ARCH/09-FEATURE-MATRIX.md`.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).

## 1. Definition (one sentence)

**AgentCowork is a local-first work environment that keeps human goals durable across interchangeable external agents and offers them scoped shared capabilities over the user's digital world.** A configured cloud executor is an additional location, not a different product or source of truth (DEC-054).

## 2. Positioning

- **Not an OS replacement.** It is an AI-native execution layer on the user's existing computer(s) — not a kernel, bootloader, or desktop-environment substitute.
- **Not an MCP application.** MCP is one interoperability door among several; it is never the architectural center.
- **Not a wrapper around one agent.** External agents are peers under one binding contract. Their native effects retain their native policy; the same Core governance applies whenever they call a shared Core capability.
- **The outward promise:** one approachable workspace that can use the agents and tools the user chooses. **The internal promise:** durable Mission truth, one governed path for Core-mediated effects, and honest provenance for native effects.

## 3. Locked principles

| ID | Principle | One-line meaning |
|---|---|---|
| P-01 | **Core owns durable work truth.** | Desktop / web / CLI / IDE / mobile are projections; each external agent owns its own reasoning loop. |
| P-02 | **Work is the universal host execution abstraction.** | A Core-scheduled chat turn, workflow run, background job or host-assigned subagent task is `Work`; an external agent's private child activity stays native and is observed with honest provenance (DEC-054). |
| P-03 | **Agent ≠ Model ≠ Provider.** | None is hard-coded to another; an external engine owns its model choice unless it explicitly delegates selection to the Core model router. |
| P-04 | **Capability ≠ Provider.** | A capability is a semantic operation (`office.spreadsheet.edit`); a provider is who implements it (native, MCP, ACP, CLI, plugin, remote). |
| P-05 | **Protocols are adapters, never the center.** | MCP / ACP / CLI / HTTP / plugins are interoperability doors. |
| P-06 | **One governed Core path.** | Every Core-mediated externally visible effect follows `Work → Capability → Provider → Handle → Guard → Ticket → Execute → Effect → Verify → Receipt → Event`. Agent-native effects retain separate provenance. |
| P-07 | **Artifacts and receipts are first-class.** | Durable, versioned, provenance-carrying. Never “just tool output”. |
| P-08 | **Domain runtimes own complexity.** | Office / Browser / Computer / Files / Code own their specialized execution; the kernel stays small and never reimplements domain logic. |
| P-09 | **Context is a platform capability; context control is an agent capability.** | Core answers “what context exists?”; the agent answers “what should the model see right now?” |
| P-10 | **Memory ≠ context.** | Memory is durable knowledge with its own write/read/forget lifecycle; context is a per-call selection. |
| P-11 | **Workflow is deterministic; agent is adaptive.** | Both compose in both directions: agent nodes inside workflows; workflows exposed as agent tools. |
| P-12 | **The computer explains itself.** | The World Model gives agents structural awareness; vision / screenshot computer-use is a fallback rung, not the default. |
| P-13 | **Core enforces its own path.** | Guard decides ALLOW / ASK / DENY for Core-mediated calls; prompts never enforce; external agents receive Core projections, never internals; Core-held secrets never leave the vault. |
| P-14 | **Token discipline.** | Deterministic operations (render, browse, list, open, search index, navigate) never touch an LLM. |
| P-15 | **Mission survives the worker.** | The user's versioned goal, evidence and plan outlive agent sessions, model changes and execution attempts. |
| P-16 | **Outcome first for people.** | Chat and finished work lead the interface; internal agents, plans, traces and extension scopes appear progressively when useful. |

## 4. Non-goals

- No OS / kernel / bootloader replacement.
- No forced single browser, model provider, coding agent, or document format.
- No vendoring of third-party projects as hard runtime dependencies because their *idea* was good — absorb architecture, redesign, implement independently; reuse code only where licensing is explicitly cleared (`ARCH/44-ABSORB-REGISTER.md`).
- No anti-bot / CAPTCHA-evasion / residential-proxy tooling in browser or computer-use capabilities.
- No universal chat syntax imposed on agents — agents keep their native grammars. DEC-055 adopts a user-facing `/cowork:*` namespace for host commands with collision preview and preserves native agent commands; the historical `/eaios:*` spelling in the frozen UI baseline is superseded (`48` §3).
- No dumping whole tool/skill/MCP catalogs or whole documents into model context by default.

## 5. The differentiator (the claim we must earn)

> **A compatible bound agent can use a granted shared capability through a suitable provider and environment while keeping its own native architecture. Mission can coordinate and verify its work across workers.**

The claim is earned only when a complete user task succeeds with low friction and reviewable evidence. Core governance applies identically to every shared call with the same grant; native tool paths remain visibly distinct (DEC-054).

## 6. Success statements (falsifiable; verified in `ARCH/42-EVIDENCE-MAP.md`)

| ID | Statement |
|---|---|
| S-01 | Every Core-scheduled chat turn, workflow run, host-assigned subagent task and background job materializes as `Work` in Runs; private agent children remain native activity, labelled by provenance. |
| S-02 | Every Core-mediated externally visible effect produces a `Receipt` replayable to its inputs; native activity is labelled observed or reported. |
| S-03 | The same capability (`office.presentation.edit`) resolves to different providers without the caller changing. |
| S-04 | An external agent onboards through the Agent Gateway and receives only its projection: identity, capability set, scoped context, workspace paths, tools, artifacts, filtered events. |
| S-05 | Context compaction never loses reconstructable facts — they are rebuilt deterministically from Work / Events / Git / Artifacts, not re-invented by the model. |
| S-06 | A workflow with an 8-hour wait persists its node and wake state across app close, logout and reboot. It resumes at the correct node when an accepted local service or remote executor owns the trigger; without a live owner it records a misfire or paused state for reconciliation on return (DEC-057). |
| S-07 | Opening and rendering a document (PDF/DOCX/XLSX/PPTX/code) consumes zero model tokens. |
| S-08 | No Core-mediated externally visible effect executes without Guard and a valid, scoped, time-boxed ticket; native paths are never shown with that assurance. |
| S-09 | A capability call can return `guidance` or `requires_user_action` (“connect Google Drive first”), not only success/failure. |
| S-10 | Durable memory survives sessions; run context dies with the run unless explicitly promoted (artifact, memory, or library item). |

## 7. Language discipline

- The product never markets itself as “an AI OS that replaces your OS”.
- “Agent” always means a reasoning runtime (the bound engine, of any kind); “capability” always means a semantic operation; “provider” always means an implementation of capabilities. Docs MUST NOT blur these.
