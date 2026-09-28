# 43 — Glossary

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P3). Terms are defined **once here**; module docs may add domain-specific vocabulary but must link back. Where a term has a canonical schema, the entity id is given.
> **P7 pass (2026-09-26):** line-checked; cross-references verified.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).

| Term | Definition |
|---|---|
| **Core** | The runtime/kernel — supervisor of the work plane, capability plane, trust plane, domains and stores. Surfaces are projections of it. |
| **Mission** | Durable user outcome with versioned intent, requirements, adaptive plan and evidence; survives Work/session/agent replacement (DM-028, `35`). |
| **Goal Contract** | Versioned objective, constraints, acceptance criteria, required evidence, approvals and budget for a Mission (DM-029). |
| **PlanNode** | Semantic unit of a Mission plan; one node may have several Work attempts (DM-038). Distinct from the `Task` projection over Work/Step. |
| **Outcome Evaluation** | Requirement-by-requirement assessment of the user's goal against current evidence; separate from individual effect verification (DM-036, `36`). |
| **Extension Grant** | Explicit, attenuable activation of a host-owned MCP/skill/plugin component for a binding, workspace and Work (DM-037, `46`). Catalog visibility alone is not a grant. |
| **Agent** | A reasoning runtime that plans and acts. Any bound engine — external (ACP/A2A/CLI/remote) or in-process. |
| **`AgentEngine`** | The peer adapter contract for external engines; lifecycle features are negotiated and unsupported methods return a typed result. Cross-engine delegation is host Work (CTR-001/030, DEC-054). |
| **Agent Gateway** | The brokered entry point external agents use; produces the 7-item projection (enforcement lives in Trust — `12` §8, `32` §3) (CTR-022, DEC-009). |
| **Agent Profile** | Discoverable agent metadata and install provenance: runtime, version/fingerprint, supported models, declared capabilities and composer abilities. Discovery does not modify native config (DM-014, DEC-054). |
| **Agent Binding** | A profile attached through a particular launch/protocol with negotiated capability evidence, governance class and scoped host grants (DM-039). |
| **Approval** | A recorded human decision (approve/reject/edit/provide-data) requested by an agent question or a workflow node (DM-010, DEC-021). |
| **Artifact** | A versioned, provenance-carrying work product (document, spreadsheet, patch, dataset, capture…) (DM-019). |
| **Artifact Gateway** | The ref-based exchange surface for artifacts with external agents (`artifact_id`, mime, uri) (`29` §5). |
| **`BLOCKED`** | The spec-conflict stop: when code and spec disagree, the change stops and a `DEC` / spec change is proposed; implementation resumes only after resolution (`.agents/docs/spec-driven-development.md`). |
| **Capability** | A semantic operation — *what* can be done (`office.spreadsheet.edit`) — independent of *who* implements it (DEC-004). |
| **Capability Descriptor** | The capability’s declaration: id, version, affordances, requirements, providers, loading mode, risk class, auth, verification hook (DM-011). |
| **Capability Handle** | A resolved, epoch-checked binding to a provider for a capability (DM-012). |
| **Capability Plane** | The registry/resolver/handles/affordances layer that turns capabilities into provider bindings (`13`). |
| **Catalog** | The browseable list of installed/available capabilities, skills, agents, workflows — the UI/agent discovery surface (`13` §6). |
| **Channel** | A user or protocol surface (desktop, CLI, ACP, A2A, API, mobile) (`32`); each advertises its actual availability and capabilities. |
| **Checkpoint** | A durable state-reconstruction record (work · context · workflow · session) (DM-006, DEC-027). |
| **Completion Contract** | The declared success conditions + verification for a piece of work; the loop may not stop before it is satisfied (`15` §4). |
| **Connector** | A provider integration for external services (mail, calendar, messaging, SaaS) (`28`). |
| **Context (Control vs Infrastructure)** | Core owns context **infrastructure** (what exists); the agent owns context **control** (what the model sees now) (DEC-007). |
| **Context Item** | One context fragment record: source, type, content ref, token cost, scope, pins, reconstructable flag, sensitivity (DM-017). |
| **Context Projection** | The scoped slice given to subagents and external agents — never the substrate (`16` §1.3). |
| **Decision (`DEC-*`)** | A recorded design choice with its evidence (`04-DECISIONS.md`); a `Locked` decision changes only through a new `DEC` that supersedes the old — its text is never silently edited. |
| **Domain Runtime** | A specialized execution domain — Office, Browser, Computer, Files, Code, Search, Comms (`22`–`28`). |
| **Effect** | The externally visible consequence of an executed capability — the thing verification and receipts attach to. |
| **Event** | A published system fact (append-only) — the single stream every projection derives from (DM-008, `30`). |
| **GIVEN/WHEN/THEN** | The statement form of a testable requirement — GIVEN the context · WHEN the trigger · THEN the observable outcome (`08-REQUIREMENTS.md`). |
| **Guard** | The single policy decider (`ALLOW`/`ASK`/`DENY`) composed of three layers: platform confinement × approval policy × declarative exec rules (DEC-028). |
| **Handles** | See Capability Handle. Handles are cached bindings; the hot path looks one up instead of re-negotiating. |
| **Installed / Available / Activated / Executing** | The four-state scoping model for resources (MCP servers, skills, plugins, models) (DEC-024). |
| **Lane** | Core Work lanes: **foreground** (active turn) · **background** (non-blocking work) · **detached** (durable record; execution after app close requires an accepted local service or remote owner) (DEC-031/057). |
| **Library** | User-facing composite view of permitted generated/uploaded/imported/linked artifacts and explicitly saved reusable items; catalog visibility never grants access (DEC-014/055). |
| **Library Item** | One promoted inventory entry (DM-023); promotion is explicit (“Save to Library”). |
| **Memory** | Durable, scoped, provenance-carrying knowledge with write/read/forget lifecycle — distinct from context (DEC-019). |
| **Memory Item** | One atomic memory record; ADD-only with a single `superseded_by` pointer; suppression-based forget (DM-018). |
| **MCP** | Model Context Protocol — one provider transport; dual-era policy (modern 2026-07-28 + legacy fallback) (DEC-030). |
| **Model Descriptor** | A model’s capabilities and limits: window, tools, reasoning modes, vision, costs, locality (DM-025). |
| **Model Router** | Core-owned model consumers and compatible host-controlled bindings use CTR-014; discovered agents may retain their native model/provider configuration (`18`, DEC-054). |
| **Occurrence** | A persisted trigger identity with a dedupe key; one logical run is admitted by atomic claim, while external effects still require idempotency or reconciliation (`20` §4, DEC-057). |
| **Open question (`OQ-*`)** | An unresolved design point. Cross-cutting questions are `OQ-###` in `ARCH/00-INDEX.md` §9; module-scoped questions are `OQ-<MNEMONIC>-<n>` in the owning module doc's Open questions section (e.g. `OQ-CTX-01`). Ids are stable — never renumbered or reused. |
| **Provider** | An implementation of capabilities (native runtime, MCP server, ACP agent, HTTP/CLI/plugin/remote) (DM-013). |
| **Provider Epoch** | A counter bumped on provider restart; stale handles/tickets bound to old epochs are invalid (DM-012, `13` §4). |
| **Receipt** | Durable evidence of an effect: ticket, capability/provider, inputs digest, outputs, verification performed (DM-020). |
| **Reconciliation** | The verification step comparing intended vs actual effect outcomes (`34` §6). |
| **RepoGraph / RepoMap** | The repository intelligence index and its ranked, token-budgeted projection (`26`). |
| **Requirement (`REQ-<DOMAIN>-<NNN>`)** | One testable behavior with acceptance and failure cases, owned by a module and derived from the SPEC/invariants/decisions (`08-REQUIREMENTS.md`); IDs are stable — never renumbered or reused. |
| **Run** | One concrete execution of an agent or workflow node (DM-005). |
| **Scheduler** | The Core component admitting work into lanes under outer limits (DEC-031, CTR-026). |
| **Session** | A conversation/agent-context container with durable host event references; the native model context may be disposable or unsupported on resume. Mission does not depend on its transcript (DM-004/007, DEC-054). |
| **Sensitivity** | Data classification (`public`/`personal`/`confidential`) enforced at recall/projection/injection (INV-10). |
| **Skill** | Reusable know-how (instructions + capability requirements) — teaches; doesn’t execute (DM-027, `31`). |
| **Spec gate** | The pre-implementation check: read the applicable specs → extract the `REQ-*` to satisfy → inspect the implementation → plan → obtain a decision for any architecture change → smallest change → test every failure case → verify acceptance (`.agents/docs/spec-driven-development.md`). |
| **Step** | A unit of progress inside a run; checkpoint boundary (DM-002). |
| **Subagent** | A delegated child Work/session, possibly backed by a different external engine; returns a receipt, never a required transcript (DEC-029/054). |
| **Task (`TASK-<DOMAIN>-<NNN>`)** | A plan unit in `TODO.md` that references the `REQ-*` it implements and its touched paths/tests; the matrix (`09`) carries the link. |
| **Test (`TEST-<DOMAIN>-<NNN>`)** | The executable verification attached to a requirement; `verified` status requires an acceptance record (`42`) — for risky classes, never unit tests alone (`09`). |
| **Ticket** | Scoped, time-boxed Core authorization required before a Core-mediated effect. External-agent native effects do not carry Core tickets (DM-009, INV-03, DEC-054). |
| **Tool vs Capability** | “Tool” is provider-side vocabulary; AgentCowork exposes capabilities. Protocols/tool names stay below the Capability Plane (INV-15). |
| **Vault** | The credential store; `use`-style API only — values never leave it (INV-02). |
| **Verification** | The read-only checks before a receipt; depth scales with risk class (`34`, INV-19). |
| **Work** | The universal execution abstraction — one lifecycle for turns, jobs, workflow runs, subagent tasks, automations (DM-001, DEC-003). |
| **Workflow** | A deterministic process definition (typed IR) that can include agent nodes; runs are pinned to a version (DM-021/022, `20`). |
| **Machine Observer** | A local read-only service that exposes consented host/hardware/process/runtime observations and bounded local time-series history through scoped Core capabilities (`51`, DEC-058). |
| **Observation descriptor** | A typed metric/query result including stable id, unit/schema, source/provider, observed time, freshness and per-item availability status (`51`, DM-041). |
| **Product consent vs OS elevation** | In-app consent discloses and enables a data category; Windows UAC (or a platform-specific privilege prompt) grants an operating-system privilege for an exact operation. Neither substitutes for the other (`12`, `19`, `51`, DEC-058). |
| **One-shot elevated helper** | A separately elevated, allowlisted process that performs one typed read after explicit user intent and exits; it does not elevate the app or run background sampling (`19`, `51`, DEC-058). |
| **Worktree** | A git-isolated checkout provisioned per-spawn for concurrent writers (DEC-029). |
| **World Model** | The continuously updated structural map of the machine + change stream; consumers query it instead of screenshotting (`21`). |
| **W1…W7** | World Model collector set — W1 file inventory + deltas · W2 process/window registry · W3 UI tree on demand · W4 window capture on demand · W5 browser world · W6 devices/registry/shares (deferred) · W7 content index/OCR (deferred); defined in `ARCH/21-WORLD-MODEL.md` §2. |

**Naming note:** v0 product names (AgentCowork-era) appear only in `ARCH/01-NAMING.md` and archive references; frozen code identifiers (`agentcowork-*`) keep their historical prefix until the post-freeze code-phase rename (OQ-003).
