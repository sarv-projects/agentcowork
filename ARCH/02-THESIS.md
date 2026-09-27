# 02 — Thesis

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P0). This doc sets identity and principles; decisions that need evidence land in `ARCH/04-DECISIONS.md`, invariants in `ARCH/05-INVARIANTS.md`.
> **SDD:** the success statements (S-01…S-10) are the falsifiable seeds of the requirement registry; REQ traceability accrues in `ARCH/09-FEATURE-MATRIX.md`.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).

## 1. Definition (one sentence)

**AgentCowork is a local-first AI work environment that composes interchangeable agents, models, capabilities and execution environments behind one governed execution model, on top of a continuously updated model of the user's digital world.**

## 2. Positioning

- **Not an OS replacement.** It is an AI-native execution layer on the user's existing computer(s) — not a kernel, bootloader, or desktop-environment substitute.
- **Not an MCP application.** MCP is one interoperability door among several; it is never the architectural center.
- **Not a wrapper around one agent.** External agents (OpenCode, Codex, Claude Code, ACP/A2A agents, remote agents) are peers — same engine contract, same governance.
- **The outward promise:** one workspace, every model, every tool. **The internal promise:** one governed path for every effect.

## 3. Locked principles

| ID | Principle | One-line meaning |
|---|---|---|
| P-01 | **Core is the brain.** | Desktop / web / CLI / IDE / mobile are projections; none is a second source of truth. |
| P-02 | **Work is the universal execution abstraction.** | A chat turn, workflow run, background job, subagent task — all are `Work` with a lifecycle. |
| P-03 | **Agent ≠ Model ≠ Provider.** | None is hard-coded to another; the engine asks the model router. |
| P-04 | **Capability ≠ Provider.** | A capability is a semantic operation (`office.spreadsheet.edit`); a provider is who implements it (native, MCP, ACP, CLI, plugin, remote). |
| P-05 | **Protocols are adapters, never the center.** | MCP / ACP / CLI / HTTP / plugins are interoperability doors. |
| P-06 | **One governed path.** | Every externally visible effect: `Work → Capability → Provider → Handle → Guard → Ticket → Execute → Effect → Verify → Receipt → Event`. Control path is bounded (ms); effect path is asynchronous and observable. |
| P-07 | **Artifacts and receipts are first-class.** | Durable, versioned, provenance-carrying. Never “just tool output”. |
| P-08 | **Domain runtimes own complexity.** | Office / Browser / Computer / Files / Code own their specialized execution; the kernel stays small and never reimplements domain logic. |
| P-09 | **Context is a platform capability; context control is an agent capability.** | Core answers “what context exists?”; the agent answers “what should the model see right now?” |
| P-10 | **Memory ≠ context.** | Memory is durable knowledge with its own write/read/forget lifecycle; context is a per-call selection. |
| P-11 | **Workflow is deterministic; agent is adaptive.** | Both compose in both directions: agent nodes inside workflows; workflows exposed as agent tools. |
| P-12 | **The computer explains itself.** | The World Model gives agents structural awareness; vision / screenshot computer-use is a fallback rung, not the default. |
| P-13 | **Enforcement lives in Core.** | Guard decides ALLOW / ASK / DENY; prompts never enforce; external agents receive projections, never internals; secrets never leave the vault. |
| P-14 | **Token discipline.** | Deterministic operations (render, browse, list, open, search index, navigate) never touch an LLM. |

## 4. Non-goals

- No OS / kernel / bootloader replacement.
- No forced single browser, model provider, coding agent, or document format.
- No vendoring of third-party projects as hard runtime dependencies because their *idea* was good — absorb architecture, redesign, implement independently; reuse code only where licensing is explicitly cleared (`ARCH/44-ABSORB-REGISTER.md`).
- No anti-bot / CAPTCHA-evasion / residential-proxy tooling in browser or computer-use capabilities.
- No universal chat syntax imposed on agents — agents keep their native grammars; AgentCowork reserves one small, collision-free namespace — **`/eaios:*`** for v1 (working spelling; the final token renames with the brand — `OQ-003`/`OQ-001`; normative rule in `AGENTCOWORK-UI.md` §5.7).
- No dumping whole tool/skill/MCP catalogs or whole documents into model context by default.

## 5. The differentiator (the claim we must earn)

> **Any agent can perform any available capability through any suitable provider in any suitable environment, under one Work / Trust / Execution model.**

Competitors ship pieces of this: agents, capability catalogs, browser/computer control, durable workflows, office runtimes. The composition — with governance that applies identically to the native agent and every external one — is the product.

## 6. Success statements (falsifiable; verified in `ARCH/42-EVIDENCE-MAP.md`)

| ID | Statement |
|---|---|
| S-01 | A chat turn, workflow run, subagent task, and background job all materialize as `Work` items visible in one Runs surface. |
| S-02 | Every externally visible effect produces a `Receipt`; every receipt is replayable to its inputs. |
| S-03 | The same capability (`office.presentation.edit`) resolves to different providers without the caller changing. |
| S-04 | An external agent onboards through the Agent Gateway and receives only its projection: identity, capability set, scoped context, workspace paths, tools, artifacts, filtered events. |
| S-05 | Context compaction never loses reconstructable facts — they are rebuilt deterministically from Work / Events / Git / Artifacts, not re-invented by the model. |
| S-06 | A workflow with an 8-hour wait survives app close, logout, and reboot, then resumes at the correct node. |
| S-07 | Opening and rendering a document (PDF/DOCX/XLSX/PPTX/code) consumes zero model tokens. |
| S-08 | No externally visible effect can execute without passing Guard and holding a valid, scoped, time-boxed ticket. |
| S-09 | A capability call can return `guidance` or `requires_user_action` (“connect Google Drive first”), not only success/failure. |
| S-10 | Durable memory survives sessions; run context dies with the run unless explicitly promoted (artifact, memory, or library item). |

## 7. Language discipline

- The product never markets itself as “an AI OS that replaces your OS”.
- “Agent” always means a reasoning runtime (the bound engine, of any kind); “capability” always means a semantic operation; “provider” always means an implementation of capabilities. Docs MUST NOT blur these.
