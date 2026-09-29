# 31 — Skills & Plugins

> **DEC-054/055/064 target amendment:** The host registry owns only host extensions. A discovered agent's native MCPs, skills and plugins remain in its namespace and are at most inventoried read-only. Catalogued, installed, activated and executing are distinct states; effective grant is resolved for each agent binding/session/workspace/Mission and cannot exceed Trust policy. A plugin's bundled components require separate grants. Bundled host skills are available across compatible bindings but are loaded only when relevant/selected; project pins and per-Work activation remain explicit. Workflow-to-skill conversion and publication live in `37`, with user controls in `48`; no global auto-mount or silent native-config rewrite is allowed.

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P3).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-SKILL-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** **skills** are reusable know-how (instructions + requirements); **plugins** are the declared extension surfaces. Skills teach; capabilities enable; workflows repeat; agents reason.
> **Dependencies:** `13-CAPABILITY` (requirements resolution) · `11-WORK` (activation scope) · `12-TRUST` (permissions/review) · `19-RUNTIME-ENVIRONMENTS` (sandboxed plugin code) · `29-ARTIFACTS` (library promotion). **Consumers:** `15` (agent skill use), UI (skill/plugin management).
> **Evidence:** product-owner brief (skills loaded only when relevant; “skills define how; agents define why/what/when”; selective everything-is-a-plugin) · `agent-harness-verification.md` §D1 (scope-tagged registrations; plugin architecture verified), §E (plugin surfaces) · `ARCH/13-CAPABILITY.md` §7 (`described_by`) · `ARCH/06-DATA-MODEL.md` DM-027 · repo conventions (`.agents/skills/*/SKILL.md`).

## 1. Purpose & rules

**Owns:** the skill registry · loader · resolver · bindings · the skill package format · activation logic · plugin **surfaces** (capabilities · skills · providers · agents · model adapters · channels · UI contributions · workflow templates) · plugin lifecycle (install/enable/update/disable/uninstall) · the review gate.
**Never owns:** capability semantics (`13`) · agent behavior (`15`) · provider protocols (`14`).

1. **Skill ≠ capability.** A skill is instructions + declared requirements; it resolves through the capability graph to providers. It never bypasses `13`/`12`.
2. **Activation is scoped and relevant** — the four-state model applies (Installed → Available → Activated → Executing); skills are **never** bulk-dumped into context.
3. **Plugins extend at declared surfaces only** — no privileged core patching; every plugin carries a manifest and passes a review gate.
4. **Provenance matters** — skills/plugins are content with provenance (source, version, digest); prompt-injection hygiene applies to their instructions like any external content.

## 2. Skill model (DM-027)

| Field | Meaning |
|---|---|
| `id` · `version` | stable identity; content-addressed digest |
| `metadata` | name · description · tags · provenance (author/source) |
| `instructions_ref` | the skill body (markdown; progressive disclosure: metadata → instructions → resources) |
| `capability_requirements[]` | what must exist/be available (resolved via `13`; missing ⇒ `guidance`) |
| `input/output contracts` | declared shapes for invocation |
| `examples_refs[]` | worked examples (loaded on demand) |

**Package format:** a directory with a manifest + instructions + optional scripts/resources (mirrors the repo’s `SKILL.md` convention). Loading order: scan catalog → match relevance → activate → inject bounded instructions; heavy resources load on demand.

### 2.1 Bundled host skill catalog

The app ships versioned, read-only catalog entries under the `cowork/` namespace. They are available in every project and compatible agent picker, but their full instructions are **not** inserted into every session. Relevance resolution may suggest activation; the user can invoke a skill explicitly from `/` or a contextual skill chip, pin it for a project, disable a project default, or select it for the current Work. Activation is recorded with its exact skill digest on that Work.

Initial catalog families:

| Skill ID | Teaches the agent to | Capability requirements (resolved separately) |
|---|---|---|
| `cowork/research` | Plan source-backed research, compare claims, preserve citations and disclose gaps | Web search/fetch or browser |
| `cowork/rich-artifact` | Turn explanations/data into diagrams, charts, visual explainers and interactive previews; link the deliverable in the response | Artifact create/version/open + an available safe renderer; image generation is optional |
| `cowork/document-authoring` | Produce and revise polished reports and documents; verify structure and rendered output | File/artifact write + compatible Office/PDF provider |
| `cowork/spreadsheet-analysis` | Inspect data, clean/date-analyze, write formulas/charts and verify recalculation | Scoped file read/write + spreadsheet provider/runtime |
| `cowork/presentation-authoring` | Build a coherent slide story, apply a template and inspect rendered slides | Presentation provider/runtime + artifact output |
| `cowork/image-creation` | Create/edit visual assets, report provenance and return the saved asset as an artifact | Connected image-generation/edit provider; otherwise offer a truthful fallback |
| `cowork/browser-workflow` | Use a shared browser for navigation, extraction and verified multi-step web tasks | Compatible host browser capability |
| `cowork/computer-workflow` | Plan, perform and verify visible desktop interactions with takeover boundaries | Compatible OS accessibility/computer-use capability and effective permission |
| `cowork/workflow-capture` | Convert a successful repeatable procedure into a reviewed, versioned workflow or skill with secrets removed | Work/event history or explicit user demonstration; skill/workflow lifecycle |

This is an initial built-in catalog, not a claim that every operation works without a provider. A skill never supplies the tool, authentication, permission or renderer it names. If a selected binding accepts bounded context overlays, the adapter supplies the activated host skill by digest; if it does not, show `not available to this agent` and offer direct host execution where supported or manual guidance. Do not install or synchronize these entries into native agent directories. Keep this inventory small and composable; domain capability itself belongs in the owner module, not duplicated in prompts.

## 3. Resolution & activation

- **Resolver inputs:** task/intent, agent profile, workspace, available capabilities.
- **Activation sets** are per session/run; users can invoke a skill explicitly; agents may request activation.
- **Scopes:** built-ins are app-catalogued; user skills may be personal; project skills/pins are workspace-bound; a turn's activation is session/Work-bound; native skills remain agent-owned. Resolution uses namespaced refs and an explicit compatibility result. The effective set is attenuated by project, binding and Work policy; no scope widens Trust grants. Project defaults and a user's disable choice are persisted separately from per-Work activation.
- **Cross-binding delivery:** a logical conversation may switch agents, but the host skill is re-resolved for the binding used by each new Work. Only selected, compatible instructions are handed off; record `delivered`, `unsupported`, `disabled` or `missing-requirement` rather than assuming injection. Switching agents never copies native skills or changes native config.
- **Context discipline:** only activated skill instructions enter context, bounded by `16`; skill bodies never live in the stable prefix unless pinned.
- **Requirements check:** a skill whose capabilities are unavailable activates in `guidance` mode (“needs X”) rather than failing silently — and activation never grants permissions; policy still decides (`12`).

## 4. Plugin surfaces

| Surface | What a plugin can contribute | Notes |
|---|---|---|
| Capability | new capabilities + executors | Descriptors from `13`; review gate |
| Skill pack | bundles of skills | Same format as native skills |
| Provider | adapters (`14` classes) | Protocol code lives here only |
| Agent runtime | an `AgentEngine` implementation | Same contract as every other engine (DEC-010, INV-12) |
| Model adapter | model/endpoint shapes (`18`) | Auth via vault refs |
| Channel | surfaces (`32`) | Gateway-mediated |
| UI contribution | panels/commands/questions | Declared slots only; no arbitrary renderer code in v1 |
| Workflow template | Library `template`/`workflow` kinds | Promotion explicit (`29` §4) |

Manifest fields: id · version · surfaces[] · permissions requested · hooks · compat window (Core contracts `07`).

## 5. Lifecycle & trust

- **Install:** from local file/folder (v1, local-first) → **review gate** (declared surfaces, requested permissions, provenance) → explicit enable per scope.
- **Run:** code-bearing host plugins execute only in a qualified confined Runtime profile (`19`) under exec policy (`12`); if the selected executor cannot provide the required confinement, Core refuses code execution with a typed reason. Static skill/template content may still load under untrusted-content handling but gains no execution authority. Grants are explicit, scope-bound and recorded.
- **Update:** versioned; compatibility checked against contract versions (`07` §0); breaking mismatches are rejected with a typed error.
- **Disable/failure:** repeated crashes auto-disable + audit; failure isolation (a bad plugin never breaks Core).
- **Uninstall:** removes code + owned data; Library entries and receipts referencing it are preserved.

## 6. Distribution (v1)

### 6.1 Catalogs, installation and marketplaces

The v1 host has a local catalog with built-in entries plus explicit local folder/file and bundle import/export. It may display links to the native agent's own catalog/marketplace, but that store remains owned by that agent and is never installed or activated by Core behind the user's back. A Core marketplace is a **catalog source**, not an execution scope: listing/browsing has no install, authentication or runtime side effect.

Remote community marketplace feeds and automatic updates are deferred until package signing, publisher identity, immutable version/digest pinning, revocation, dependency review, malware/content scanning, rollback and a maintained catalog service are designed and qualified. Until then, do not present a remote package URL as verified marketplace content. A downloaded package enters a review queue, is stored locally by immutable digest, and requires explicit install followed by separate activation. Updating creates a new version; running Works stay pinned to the old version until their next activation. Rollback is explicit and does not restore revoked credentials or grants.

Recommended lifecycle and vocabulary:

```text
catalogued → inspected → installed locally → enabled in a profile
           → activated for this binding/Work → executing → revoked/removed
```

The UI shows these independently. **Install** stores package content; **Connect** authorizes an external account; **Enable for this agent** chooses compatibility/default availability; **Use in this Work** creates the scoped grant. None implies the next. Third-party connector catalogs such as Composio or an MCP directory can supply discoverability and protocol packages, but connector credentials still belong to the connector owner/Core vault and require separate per-action authorization.

**Marketplace evidence and limits:** OpenHands Agent Canvas at the pin recorded in `45` has a catalog/custom MCP install path with connection testing, OAuth, tool listing, health and profile references. OpenWork's inspected source merges project/global/runtime MCP sources and resolves skills from multiple locations; that is a managed-engine configuration pattern, not authority to rewrite discovered agents. Libre WebUI's inspected catalog is an admin/user/grant-scoped first-party tool server directory with per-user credentials and turn-level narrowing; it does not prove safe injection into arbitrary ACP agents. OpenCowork's enabled MCP manager and skill-location precedence are host-local patterns, but its global tool set and child inheritance are too broad for this contract. Treat these as lifecycle/UI references, not as a reason to globally activate packages. Exact pins and files: `ARCH/45-REFERENCE-RESEARCH.md` §“Extension ownership and connected-app setup”; `TODO.md` implementation-source table rows for `TASK-UXQ-006`/`TASK-UXQ-007`.

## 7. Failure modes

| Failure | Behavior |
|---|---|
| Bad/missing manifest | Rejected at review; typed reason. |
| Capability requirements unmet | `guidance` with the missing requirement; no silent degradation. |
| Plugin crash loop | Auto-disable + audit; Core unaffected. |
| Version skew vs Core contracts | Rejected with compat window explanation. |
| Malicious/injected skill instructions | Treated as untrusted content: review gate + provenance + the same injection hygiene as other external content. |
| Hostile skill/plugin package | Extraction bounded and confined to declared roots (pathfloor); manifest + provenance pass the review gate; rejection is typed — nothing lands outside the package (EDGE-094, `25` §7). |
| Plugin update while executing | In-flight execution finishes on its loaded version; the new version applies at next activation; breaking skew is rejected with the compat window — no hot-swap mid-call (EDGE-096). |

## 8. Interop

**Depends on:** `10` · `12` (review/permissions) · `13` (requirements) · `14`/`18`/`19` (surface hosts) · `29` (library).
**Exposes to:** `15` (skills), UI (management), `32` (channel contributions) — through `CTR-020` (`SkillResolver`; `07` §1).
**DAG check:** skills resolve capabilities; they never execute effects directly, and plugins never patch Core.

## 9. Not in v1

Marketplace/remote registries · auto-update · code-signing infrastructure · arbitrary UI code execution · cross-user sharing.

## 10. Open questions (`OQ-SKP-*`)

1. Skill package format finalization (align with existing `SKILL.md` convention + manifest schema).
2. Review-gate depth for v1 (manual enable vs checks only).
3. Plugin permission model shape (capability grants vs surface grants).
4. UI contribution slot list for v1 (or defer entirely).
5. Whether skill activation feedback (which skills ran) surfaces in receipts.

## 11. Evidence

Product-owner brief (skills model, relevance-only loading, agent-vs-skill distinction, selective plugin surfaces) · `agent-harness-verification.md` §D1 (verified plugin registry + scope-tagged registrations), §E (surface patterns) · `ARCH/13-CAPABILITY.md` §7 · `ARCH/06-DATA-MODEL.md` DM-027 · repo `.agents/skills` convention.

## 12. Requirements (`REQ-SKILL-*`)

> **DEC-054 amendment:** Host-installed, discovered agent-native and third-party extensions have distinct custodians and namespaces. Installed/catalog-visible does not mean activated/authorized; activation is per binding and Work with child attenuation. Workflow-to-skill capture and versioned promotion are specified in `37-WORKFLOW-SKILL-LIFECYCLE.md`. Discovery never edits a native extension directory.

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-SKILL-001` | Skill ≠ capability: requirements resolve through the capability graph under policy — no bypass of `13`/`12` |
| `REQ-SKILL-002` | Four-state activation; relevance-loaded and context-bounded — skills are never bulk-dumped |
| `REQ-SKILL-003` | DM-027 skill model + package format with content-addressed provenance |
| `REQ-SKILL-004` | Missing requirements → `guidance` naming the gap; activation never grants permissions |
| `REQ-SKILL-005` | Plugins contribute at declared surfaces only, via manifest — no Core patching |
| `REQ-SKILL-006` | Install → review gate → explicit per-scope enable (local-first v1) |
| `REQ-SKILL-007` | Plugin code runs sandboxed under exec policy; grants explicit and recorded |
| `REQ-SKILL-008` | Versioned updates checked against `07` contract versions; breaking skew rejected typed |
| `REQ-SKILL-009` | Crash loops auto-disable + audit; a bad plugin never breaks Core |
| `REQ-SKILL-010` | Uninstall removes code + owned data; Library entries and receipts are preserved |
| `REQ-SKILL-011` | Package extraction is bounded and confined (pathfloor); hostile packages are rejected typed (EDGE-094) |
| `REQ-SKILL-012` | Skill/plugin instructions are untrusted content — provenance + injection hygiene (EDGE-093) |
| `REQ-SKILL-013` | Updates never hot-swap mid-call; new versions apply at next activation (EDGE-096) |
| `REQ-SKILL-014` | Bundled host skills are cross-binding and lazy-loaded by digest only through supported overlays; native stores and Trust grants are unaffected |
