# AgentCowork — Architecture v1 — Index & Blueprint

> **Doc set:** AgentCowork architecture **v1** — rebuilt from scratch on the shoulders of v0.
> **Status:** **v1 FROZEN (2026-09-26)** — docs 00–44 + `AGENTCOWORK-SPEC.md` + `AGENTCOWORK-UI.md` frozen after the P7 SDD wave (`DEC-038…045` promoted to `Locked`; independent review reconciled). Further changes require a superseding `DEC` (`ARCH/04-DECISIONS.md`). **Re-frozen (2026-09-26, after the owner-directed P9 verification pass):** every doc read line-by-line, understood and fixed (architect-style); clashes reconciled with back-propagation; `DEC-046`/`DEC-047` added and promoted to `Locked`; changes again require a superseding `DEC`.
> **Date:** 2026-09-26 · **Repo:** `desktop_app` · **Branch:** `main`
> **Code:** the 2026-09-26 v1 freeze is a baseline record; implementation follows the amended requirements and `TODO.md` W0–W6 with acceptance in `ARCH/42-EVIDENCE-MAP.md` and scenario oracles in `ARCH/49-TEST-CASES.md`.
> **v0 archive:** `ARCHIVE/v0/` (local, git-ignored) — see `ARCHIVE/v0/MANIFEST.md`. Nothing in the archive is a contract.
> **Delivery tracker:** `TODO.md` is not an architecture authority, but this amendment includes it in the traceability pass: active `TASK-*` rows point to their exact requirement, owner section, current/new code path, reviewed source file where applicable, and pending test slot.
>
> **Current target (2026-09-29):** the frozen 2026-09-26 baseline is amended by DEC-052/054/055/056/057/058/059/060/061/062/063. DEC-063 clarifies that universal governance and receipt rules in DEC-002/010/022 cover Core-mediated effects; native external-agent effects retain their own policy and distinct provenance. `ARCH/08-REQUIREMENTS.md` and `ARCH/09-FEATURE-MATRIX.md` each contain 343 active requirements/rows across 31 domains; `TODO.md` tracks W0–W6. Historical P7/P9 completion records below describe their dated baseline, not proof that the amended set has passed a new line-by-line semantic audit.
>
> **Re-frozen after verification (2026-09-26, owner-directed):** full architect-style pass across `AGENTCOWORK-SPEC.md` + `AGENTCOWORK-UI.md` + `ARCH/00–44` — read → understand → fix completed; schema/architecture/LLD clashes reconciled with back-propagation; missing pieces added; independent review applied; v1 re-frozen.
>
> **Absorption wave 2 (2026-09-26, before freeze):** provider layer (`DEC-034/035`), async subagents (`DEC-036`), web search (`DEC-037`) — absorbed and registered.
>
> **P7 (SDD layer, 2026-09-26):** module passes `10`–`34` seeded; registry + matrix + agent-kit protocol (`.agents/docs/spec-driven-development.md`) + `AGENTS.md` §16.

---

## 1. Why a rebuild

v0 grew by accretion: 33 ARCH files, 55 research files, a 355 KB spec, a 606 KB changelog — with drift between docs and code (stale modules, phantom traces, duplicate ownership of responsibilities). v1 is written from scratch as a small set of documents with:

- **one owner per topic**,
- **an explicit authority chain** (§2),
- **evidence rules** — no claim about an external system is accepted without a primary citation (§6, §7),
- **a pass protocol** that checks architecture viability after every pass (§4, §5),
- **module-by-module development** — each module doc must state what it owns, what it depends on, what it exposes, and how it fails (§3).

**Kept from v0 (direction that survived verification):** Work-first runtime; one governed execution path for every Core-mediated effect; Capability ≠ Provider; custody invariants (sidecar proposes / Core disposes for Core-mediated calls; Core-held keys never leave the vault); agent-native tools retain their agent's ownership under DEC-054.
**Rebuilt or added in v1:** naming layer (AgentCowork / Core); memory architecture (new); world model; workflow engine as Core infrastructure; context split (infrastructure vs control); UI architecture incl. chat rendering; module-by-module interop checks; canonical data-model + contracts as separate shared docs.

---

## 2. Authority chain

| Layer | Document | Authority over |
|---|---|---|
| Product | `AGENTCOWORK-SPEC.md` + accepted `DEC-054/055/056/057/058/059/060/061/062` amendments | **WHAT** the product must be: behavior, contracts, acceptance. Frozen baseline is superseded on named boundaries by the later decisions. |
| Requirements | `ARCH/08-REQUIREMENTS.md` | **WHAT must be verified**: testable behaviors (`REQ-*`) derived from the SPEC, each with acceptance + failure cases. |
| Traceability | `ARCH/09-FEATURE-MATRIX.md` | The `REQ → design → task → test` map. Owns links only, never content. |
| Architecture | `ARCH/03-HLD.md` | **HOW** the system is structured. Module docs derive from it. |
| Module LLD | `ARCH/10..39`, `ARCH/46`, `ARCH/48`, `ARCH/51` | Their module only. MUST NOT contradict HLD/SPEC/contracts. |
| Shared | `ARCH/06-DATA-MODEL.md`, `ARCH/07-CONTRACTS.md` | Canonical shared entities (DM-*) and interfaces (CTR-*). Module docs own local details only. |
| UI | `ARCH/48-EXPERIENCE-SURFACES.md`; `AGENTCOWORK-UI.md` baseline | Final interaction HLD/LLD is `48`; root UI is source-path/baseline evidence where it conflicts with DEC-055. |
| Meta | `ARCH/00..05, 40..44` | Navigation, naming, thesis, decisions, invariants, flows, edge cases, evidence, glossary. |
| Delivery | `TODO.md` | Implementation status only — never a design authority. |
| Evidence | `ARCHIVE/v0/**`, `ARCHIVE/v1-research/**`, `REPO-COMPARE/**` | Historical/working evidence — never authority. |

**Conflict resolution:** module doc vs HLD → HLD wins. HLD vs SPEC → SPEC wins (product intent), unless a recorded decision (`DEC-*`) says otherwise. Any change to an authority doc requires a new/updated `DEC` entry in `04-DECISIONS.md`.

---

## 3. Document map

> **DEC-054/055/056/057/058/059/060/061/062/063 amendment (2026-09-29):** The 2026-09-26 frozen baseline is historical status, superseded on named boundaries by these decisions. Target HLD is `03` + `35` + `46` + `50`; Mission LLD is `35`–`37`; Experience HLD/LLD is `48`; Machine Observer LLD is `51`; model-agnostic scenario oracles are `49`; research ledger is `45`, market comparison `47`; requirements/matrix are `08`/`09` and implementation work is `TODO.md` W0–W6. DEC-060 reconciles the capability-selection and computer-use ladders; DEC-061 uses the official Rust MCP SDK behind the Guard transport while retaining Core-owned policy; DEC-062 resolves queued chat items against the latest valid conversation selection at dequeue; DEC-063 clarifies that the universal governance/receipt rules in DEC-002/010/022 cover Core-mediated operations and preserves distinct native-agent provenance. Historical count statements below refer to earlier freezes, not the amended registry.

> **Amendment reconciliation:** The older module headings “Not in v1” record a 2026-09-26 implementation sequence, not exclusions from the final product. For final capability conflicts, accepted DEC-054/055/056/057 and the amended requirements prevail. `TODO.md` W0–W6 is the current delivery tracker; its History section and the older UI baseline are provenance only. Remote/cloud continuation, cross-device projections, artifact knowledge retrieval, scoped extensions and advanced Workbench editing require explicit implementation and qualification even where an older module lists them as deferred. No checked P9 historical box certifies a fresh semantic audit of later amendments.

| ID | File | Scope | Purpose | Status |
|---|---|---|---|---|
| 00 | `ARCH/00-INDEX.md` | Meta | This file: authority, doc map, passes, conventions, evidence | Frozen v1 |
| 01 | `ARCH/01-NAMING.md` | Meta | Working names + v0→v1 rename map | Frozen v1 |
| 02 | `ARCH/02-THESIS.md` | Meta | Positioning, locked principles, non-goals, success statements | Frozen v1 |
| 03 | `ARCH/03-HLD.md` | Meta | Planes, module map, dependency rules, governed path, scoping model | Frozen v1 |
| 04 | `ARCH/04-DECISIONS.md` | Meta | Decision register (`DEC-*`) with evidence | Frozen v1 |
| 05 | `ARCH/05-INVARIANTS.md` | Meta | Invariants (`INV-*`) + enforcement points + verification | Frozen v1 |
| 06 | `ARCH/06-DATA-MODEL.md` | Shared | Canonical entities and schemas (`DM-*`) | Frozen v1 |
| 07 | `ARCH/07-CONTRACTS.md` | Shared | Canonical cross-module interfaces (`CTR-*`) | Frozen v1 |
| 08 | `ARCH/08-REQUIREMENTS.md` | Requirements | Behavioral registry (`REQ-*`): statements, acceptance, failure cases | Frozen v1 |
| 09 | `ARCH/09-FEATURE-MATRIX.md` | Requirements | Traceability: `REQ` → `DEC/DM/CTR` → `TASK` → `TEST` | Frozen v1 |
| 10 | `ARCH/10-KERNEL.md` | Core kernel | ids, errors, config, time, serialization; minimal-kernel rule | Frozen v1 |
| 11 | `ARCH/11-WORK.md` | Work plane | Work · Step · Task · Session · Run · Checkpoint · Scheduler | Frozen v1 |
| 12 | `ARCH/12-TRUST.md` | Trust/Control | Policy · Guard · approvals · tickets · vault · egress · audit · external-agent projections | Frozen v1 |
| 13 | `ARCH/13-CAPABILITY.md` | Capability | Registry · catalog · resolver · handles · affordances · guidance · capability graph | Frozen v1 |
| 14 | `ARCH/14-PROVIDERS.md` | Capability/Execution | Provider adapter contract + native/MCP/ACP/HTTP/CLI/plugin/remote + MCP era policy | Frozen v1 |
| 15 | `ARCH/15-AGENT-PLANE.md` | Agent plane | Negotiated host adapter contract for external agents, host delegation/child Work, isolation modes, receipts and native-path limits | Frozen v1; amended DEC-054 |
| 16 | `ARCH/16-CONTEXT.md` | Context | Context infrastructure (Core) + context control (the bound engine) + projections | Frozen v1 |
| 17 | `ARCH/17-MEMORY.md` | Memory | Durable memory: layers, write/read paths, minimal algorithm set, upgrade path | Frozen v1 |
| 18 | `ARCH/18-MODEL-ROUTING.md` | Model plane | Model registry · router · adapters; local discovery; reasoning-effort mapping | Frozen v1 |
| 19 | `ARCH/19-RUNTIME-ENVIRONMENTS.md` | Execution | Process manager · environments · sandbox · lifecycle · health | Frozen v1 |
| 20 | `ARCH/20-WORKFLOW.md` | Orchestration | Workflow IR · triggers · durability · versioning · approvals | Frozen v1 |
| 21 | `ARCH/21-WORLD-MODEL.md` | World | Scanner · registries · world graph · event stream · incremental updates | Frozen v1 |
| 22 | `ARCH/22-OFFICE.md` | Domain | Office runtime: L1/L2/L3 · resident contexts · render/validate · format providers | Frozen v1 |
| 23 | `ARCH/23-BROWSER.md` | Domain | Browser runtime: managed Chromium + adapters · browser world · ladder | Frozen v1 |
| 24 | `ARCH/24-COMPUTER-USE.md` | Domain | Computer-use ladder · UI automation · vision fallback · input safety | Frozen v1 |
| 25 | `ARCH/25-FILES.md` | Domain | File identity · watchers · leases · indexing | Frozen v1 |
| 26 | `ARCH/26-CODE.md` | Domain | RepoGraph/RepoMap · LSP · worktrees · code execution | Frozen v1 |
| 27 | `ARCH/27-SEARCH.md` | Domain | Search plane | Frozen v1 |
| 28 | `ARCH/28-COMMS.md` | Domain | Connectors; email/calendar/messaging as a capability layer | Frozen v1 |
| 29 | `ARCH/29-ARTIFACTS.md` | Artifacts | Artifact + Receipt models · versions · provenance · library promotion | Frozen v1 |
| 30 | `ARCH/30-EVENTS.md` | Events | Event store · bus · replay · subscriptions; usage & cost telemetry | Frozen v1 |
| 31 | `ARCH/31-SKILLS-PLUGINS.md` | Extensibility | Skill registry/loader/resolver; plugin surfaces | Frozen v1 |
| 32 | `ARCH/32-CHANNELS.md` | Surfaces | Desktop/CLI/ACP/A2A/API/mobile projections; agent gateway; owns the ACP crate (`agentcowork-acp`) | Frozen v1 |
| 34 | `ARCH/34-EFFECT-VERIFICATION.md` | Verification | Validate · render · verify · reconcile; receipt policy | Frozen v1 |
| 35 | `ARCH/35-MISSION.md` | Mission | Durable goal, contract, requirements, adaptive plan, controller | DEC-054 target |
| 36 | `ARCH/36-OUTCOME-AND-RECOVERY.md` | Mission | Outcome evaluation, evidence validity, long-horizon recovery | DEC-054 target |
| 37 | `ARCH/37-WORKFLOW-SKILL-LIFECYCLE.md` | Extensibility | Capture, workflow-to-skill promotion, external workflow adapters | DEC-054 target |
| 38 | `ARCH/38-EXPERIENCE-QUALITY.md` | Experience | Progressive Mission Control and measurable quality | DEC-054 target |
| 39 | `ARCH/39-ARCHITECTURE-DELIVERY.md` | Delivery design | Dependency sequence and acceptance gates | DEC-054 target |
| 40 | `ARCH/40-FLOWS.md` | Cross | End-to-end sequences (`FLOW-*`) | Frozen v1 |
| 41 | `ARCH/41-EDGE-CASES.md` | Cross | Edge-case catalog (`EDGE-*`) + resolutions | Frozen v1 |
| 42 | `ARCH/42-EVIDENCE-MAP.md` | Cross | Evidence map + acceptance mapping for implementation | Frozen v1 |
| 43 | `ARCH/43-GLOSSARY.md` | Meta | Terms | Frozen v1 |
| 44 | `ARCH/44-ABSORB-REGISTER.md` | Meta | Competitor absorb register + licensing ledger | Frozen v1 |
| 45 | `ARCH/45-REFERENCE-RESEARCH.md` | Evidence | Pinned source observations and limits | DEC-054 evidence |
| 46 | `ARCH/46-ECOSYSTEM-ARCHITECTURE.md` | HLD | External agent ownership, scoped ecosystem, heterogeneous teams | DEC-054 target |
| 47 | `ARCH/47-MARKET-AND-BENCHMARKS.md` | Evidence | Official competitor capability snapshot and comparison protocol | DEC-054 evidence |
| 48 | `ARCH/48-EXPERIENCE-SURFACES.md` | Experience | Final composer, navigation, Workbench, Library, settings and team interaction HLD/LLD | DEC-055 target |
| 49 | `ARCH/49-TEST-CASES.md` | Quality | Model-agnostic product scenario ladder and pass oracles | DEC-055 target |
| 50 | `ARCH/50-SYSTEM-BLUEPRINT.md` | Cross | Mermaid ownership, lifecycle, effect, artifact and team maps | DEC-055 target |
| 51 | `ARCH/51-MACHINE-OBSERVABILITY.md` | Local service | Read-only system/hardware/GPU/storage/history/query capability and extractable service boundary | DEC-058/059 target |
| — | `AGENTCOWORK-SPEC.md` | Product | Product contract (WHAT) — root authority | Frozen v1 |
| — | `AGENTCOWORK-UI.md` | UI | Frozen baseline and source-path inventory; `48` supersedes conflicting target interactions | Frozen v1 baseline |
| — | `README.md` (root) | Product | Repo landing page — v1 sync | Frozen v1 |
| — | `AGENTS.md` (root) | Process | Agent operating instructions — v1 synced 2026-09-26 | Done |

---

## 4. Build passes

| Pass | Content | Exit condition |
|---|---|---|
| **P0** ✅ | Archive v0; 00-INDEX, 01-NAMING, 02-THESIS, 03-HLD | Blueprint readable end-to-end (2026-09-26) |
| **P1** ✅ | 04-DECISIONS, 05-INVARIANTS, 06-DATA-MODEL, 07-CONTRACTS | Drafted 2026-09-26 — `DEC-001…033`, `INV-01…024`, `DM-001…027`, `CTR-001…026` |
| **P2** ✅ | Core modules: 10 → 11 → 12 → 13 → 14 → 15 → 16 → 17 → 18 → 19 → 20 → 21 → 29 | All drafted 2026-09-26 (22 carried with P3 label) |
| **P3** ✅ | Remaining modules: 22–28, 30, 31, 32, 34, 43, 44 | All drafted 2026-09-26 |
| **P4** ✅ | Cross-cutting: 40-FLOWS, 41-EDGE-CASES, 42-EVIDENCE-MAP | Drafted 2026-09-26 — 24 flows · 138 edge cases · FIX register |
| **P5** ✅ | `AGENTCOWORK-SPEC.md`, `AGENTCOWORK-UI.md` | SPEC ✅ + UI ✅ drafted 2026-09-26 (UI: 14 sections) |
| **P6** ✅ | Viability + evidence sweep; consistency pass; freeze v1; README/AGENTS sync | Sweep run 2026-09-26 (cross-refs/sections/statuses/names clean; `20-WORKFLOW` interop gap fixed); freeze declared for review; README + AGENTS synced |
| **P7** ✅ | SDD layer: `08-REQUIREMENTS` + `09-FEATURE-MATRIX`; module Requirements/Acceptance sections; `AGENTS.md` §16 + kit protocol | `REQ-*` registry seeded per domain (307); matrix traceable (307 rows); `TODO.md` reworked into `TASK-*` units (W0–W4) |
| **P8** ✅ | **v1 freeze** — owner decision (2026-09-26): `DEC-038…045` promoted to `Locked`; doc statuses flipped | **Frozen v1 (2026-09-26)** |
| **P9** ✅ | Owner-directed verification pass (2026-09-26): every doc read line-by-line, understood, fixed; clashes back-propagated; missing pieces added; independent review applied | **Re-frozen v1 (2026-09-26)** |

---

## 5. Viability checklist (per doc, and for the whole set at freeze)

- [x] **One governed Core path:** every Core-mediated externally visible effect flows `Work → Capability → Provider → Handle → Guard → Ticket → Execute → Effect → Verify → Receipt → Event`; external agents' native effects carry separate provenance (DEC-054).
- [x] **One owner per responsibility:** no duplicated engines/registries/schedulers/provider systems.
- [x] **Acyclic dependencies:** every module edge has a named contract (`CTR-*`).
- [x] **Module interop:** doc states depends-on, exposes-to, and failure behavior.
- [x] **Flows complete:** start, success, failure, cancel, crash-recovery.
- [x] **Edge cases:** enumerated and resolved, or explicitly deferred with a DEC.
- [x] **Evidence:** external claims cited (`path:line` / URL); no silent UNVERIFIED claims.
- [x] **Traceability:** every `REQ-*` has acceptance + failure cases, an owning module, and a `TEST-*` (or an explicit deferral).
- [x] **Token discipline:** deterministic operations never require an LLM.
- [x] **Security:** enforcement in Core (Guard) not prompts; vault custody preserved; external agents get projections only.
- [x] **No stale v0 terms:** v1 names only (except `01-NAMING` map and history notes).

> **Freeze record (2026-09-26):** checklist run across the set during P6 + P7 (module passes, line-checks, independent review; findings reconciled; re-run in the P9 verification pass before the re-freeze). `TEST-*` minting and Windows acceptance records are code-phase (`TODO.md` W0–W4, `ARCH/42-EVIDENCE-MAP.md`).

---

## 6. Conventions

- **Keywords:** MUST / SHOULD / MAY (RFC-2119 semantics).
- **IDs:** `DEC-###` decisions · `REQ-<DOMAIN>-<NNN>` requirements · `TASK-<DOMAIN>-<NNN>` tasks · `TEST-<DOMAIN>-<NNN>` tests · `INV-NN` invariants · `DM-###` data-model entities · `CTR-###` contracts · `FLOW-###` flows · `EDGE-###` edge cases · `RISK-###` risks · `FIX-##` code-phase fix register · `PEND-###` pending decisions.
- **Open questions:** cross-cutting `OQ-###` are registered in §9; module-scoped questions use `OQ-<MNEMONIC>-<n>` (e.g. `OQ-CTX-01`, `OQ-MEM-03`) and are defined by the owning module doc's Open questions section. Both forms are referenced inline; ids are never renumbered or reused.
- **World-model collectors:** `W1…W7` as defined in `ARCH/21-WORLD-MODEL.md` §2.
- **Evidence format:** `ev: path:line` (repo) · `ev: URL` (web) · confidence `H/M/L` · `UNVERIFIED` must be temporary and carry what would verify it.
- **Status labels:** `Planned` → `Draft Pn` → `Review Pn` → `Frozen v1 (frozen <date>; drafted Pn)`.
- **Cross-references:** use file paths (`ARCH/17-MEMORY.md`), not section numbers, so docs can evolve.

---

## 7. Evidence base (surviving)

- v0 docs: `ARCHIVE/v0/ARCH/` (33 entries incl. ADR/ and the archived coordinator loop).
- v0 research: `ARCHIVE/v0/RESEARCH/` — `2026-ai-landscape/` (10 files), `desktop_app/` (45 files).
- Comparator work: `~/business_Dev/REPO-COMPARE/` — `MASTER-COMPARISON.md` (190 items — its §1 “186” line is stale; see `DISPOSITION.md` §a), `DISPOSITION.md`, `LICENSE-LEDGER.md`, `BRIEFS/` (20), `clone2/` (55 full clones), `clone3/` (2).
- v1 lane research: `ARCHIVE/v1-research/` (nine evidence deliverables) + `ARCHIVE/v1-research/v1-sdd/` (code-state inventory, OpenCode/harness notes, engine-loop draft, memory deep-dive, UI proposals, final-review findings).
- **Lost:** `/tmp/opencode/recon/` reports 01–29 (temp cleanup, 2026-09-26). Do not cite them; re-verify from surviving sources.

---

## 8. Working names

Working product name: **AgentCowork** · Runtime: **Core** · Agent engine: external in v1. Full map and rename table: `ARCH/01-NAMING.md`.

---

## 9. Open questions

| ID | Question | Resolve by |
|---|---|---|
| OQ-001 | Product shorthand for UI copy (“AC”? “Cowork”? none) | Before UI copy freeze (P5) |
| OQ-002 | Windows-first World Model collectors; cross-platform parity is separately qualified per environment | DEC-058/059 and `21`/`51` define the current target; platform acceptance remains pending |
| OQ-003 | Code identifier rename | Resolved by DEC-053 (2026-09-27); legacy spellings remain only in history/evidence |
| OQ-004 | `docs/` folder v1 review; README/AGENTS sync ✅ done (2026-09-26) | Review remaining `docs/` content against the amended target before release |
| OQ-005 | CLI final binary name + command surface (`32-CHANNELS.md` §3) | Product owner (branding, `DEC-020`) |
| OQ-006 | Whether v0 doc removals are committed now or when v1 freezes | ✅ resolved — committed `573fff0` (2026-09-26) |

---

## 10. P0 archive record

**Archived to `ARCHIVE/v0/`** (git-ignored; see `ARCHIVE/v0/MANIFEST.md`): `ARCH/`, `RESEARCH/`, `DESKTOP-APP-SPEC.md`, `SPEC-CHANGELOG.md`, `COMPETITIVE-POSITIONING.md`, `PACKAGING.md`, `SUPPORT-MATRIX.md`, `TEST-CASES.md`, `testcases.md`, `UI-DESIGN-PROMPT.md`, `UX-TESTING-PLAN.md`, `multiagent.txt`.
**Kept live:** `TODO.md` (exempt), `README.md`, `AGENTS.md`, `CURRENT_RUN.md`, `CONTRIBUTING.md`, `PRIVACY.md`, `SECURITY.md`, `LICENSE*`, `THIRD-PARTY-NOTICES.md`, `ui/DESIGN-SYSTEM.md` (current theme source), `docs/` (operational; later pass), `.agents/` (agent kit; later pass).
**Interpretation:** “docs” = the product/architecture/research corpus. Operational, legal, generated and agent-kit files stay in place until their scheduled v1 sync so the repo keeps working during the rebuild.

**Repo cleanup (2026-09-26):** `CODEBASE-MAP.md`, `docs/codebase/` (10 md + `freshness.json`), `docs/release/post-v1.md`, `docs/release/retrospective-pack.md`, `docs/download.md` and `.agents/docs/research.md` were moved to `ARCHIVE/v0/repo-cleanup-2026-09-26/` (git-ignored; see `ARCHIVE/v0/MANIFEST.md`). The gates and references that depended on them were re-homed the same day (see the manifest's "Gate re-homing" note).
