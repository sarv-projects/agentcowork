# AgentCowork — implementation plan

> **Status:** active implementation plan (2026-09-28). The 2026-09-26 v1 freeze is amended by DEC-052/054/055; W0–W6 are active and the History section is archival. **Code:** the plan is not a claim that its target capabilities are implemented; work proceeds spec-driven per `AGENTS.md` §16.
> **Authority / read order:** `AGENTCOWORK-SPEC.md` (WHAT) + accepted decisions → `ARCH/08-REQUIREMENTS.md` (testable behaviors) → `ARCH/03-HLD.md` (HOW) → owning module docs (`ARCH/10…38`, `46`, `48`, `50`). **This file owns `TASK-*` ids, status and sequencing, never design.** Section links in task rows point to the design owner; the requirement heading owns acceptance/failure cases. Where this plan disagrees with an authority doc, record the conflict and its decision before implementing.
> **Revision:** reworked 2026-09-26 (P7) from the v0-era ledger. Still-true open rows are condensed into the waves; the full pre-v1 text (1,304 done / 375 open / 4 partial rows at commit `3eeb8f0`) is kept in the History section and preserved verbatim in git (`git show 3eeb8f0:TODO.md`).

## How to read this plan

**SDD chain:** `REQ → TASK → TEST → evidence`. Requirements are registered in `ARCH/08-REQUIREMENTS.md` (330 active `REQ-*` across 30 domains, 2026-09-28); this file turns them into `TASK-*` units; `ARCH/09-FEATURE-MATRIX.md` links the chain; `ARCH/42-EVIDENCE-MAP.md` holds the acceptance map and the `FIX-01…18` register.

**Task format:** `TASK-<DOMAIN>-<NNN>` — active domains are:
`PROD · KERNEL · WORK · TRUST · CAP · PROV · AGENT · CTX · MEM · MODEL · RTENV · WF · WORLD · OFFICE · BROWSER · CUA · FILES · CODE · SEARCH · COMMS · ART · EVENTS · SKILL · CHAN · VERIFY · UI · MISSION · ECO · LEARN · UXQ`. `AGX` is retired and its IDs are never reused.
Every task row carries: the `REQ-*` it implements (and `FIX-*` where it closes the register), the touched paths (current tree, or `new` + intended module), the test slot (`TEST-<DOMAIN>-<NNN>`), and a status.

**Statuses:** `[ ]` todo · `[~]` in progress · `[x]` done · `[!]` blocked.
A task flips to `[x]` only with passing tests **and** the acceptance evidence `ARCH/42` requires; unit tests alone never close a risky class (`ARCH/08` §3, `ARCH/42` §1).

**Rules:**

1. IDs are never renumbered or reused (`ARCH/00-INDEX.md` §6). A split task keeps its id and the split-off unit takes the next free number in the same domain.
2. Tests are named `TEST-<DOMAIN>-<NNN> (pending)` until written; `pending` is visible debt, not failure (`ARCH/09` §2).
3. If code and spec disagree, stop and emit `BLOCKED` with a proposed `DEC`/spec change — never implement around the conflict (`.agents/docs/spec-driven-development.md`; `AGENTS.md` §16).
4. Task rows never restate a requirement: statement, acceptance and failure cases live in `ARCH/08`; the design lives in the owning module doc.
5. The matrix `Task` cell is filled in the same change as the task row. `pending` is allowed only with an explicit planning gap, not when an active task already covers the requirement.
6. The baseline below is a snapshot (2026-09-26); each task re-verifies its own paths against the tree before editing.

**Wave evidence.** `W5` — UI completeness (from the UX coverage audit, P10) — `ARCHIVE/v1-research/v1-sdd/ux-completeness-ledger.md`. Its `AGENTCOWORK-UI.md` citations are historical baseline locations; DEC-055's final interaction owner is `ARCH/48-EXPERIENCE-SURFACES.md`. A W5 row whose old wording conflicts with `48` must be revised or superseded before implementation. W6 adds the resulting product-wide experience work.

## Implementation source pointers (pinned prior art)

Use this table with each task's `REQ-*`, owning `ARCH` section and current-tree path. **These are exact starting files for design study, not assertions that code was copied or that the linked project implements our contract.** `ARCH/45-REFERENCE-RESEARCH.md` records the observed behavior and evidence limit; `ARCH/44-ABSORB-REGISTER.md` records reuse disposition. Before importing code, confirm the exact upstream commit and its license in the intended dependency context. If a task is absent here, it is driven by our own contract/current code, or no exact external source has yet been verified; do not invent an upstream implementation. Recheck upstream paths before coding because the pins are historical.

| Applies to active tasks | Exact repository and file at reviewed pin | What to inspect and how to use it |
|---|---|---|
| `TASK-ECO-001`, `TASK-UXQ-002` | [AionUi `6744099`: `binaryResolver.ts`](https://github.com/iOfficeAI/AionUi/blob/6744099/packages/desktop/src/process/backend/binaryResolver.ts); [AionCore `ea24f50`: `registry.rs`](https://github.com/iOfficeAI/AionCore/blob/ea24f50/crates/aionui-ai-agent/src/registry.rs) | Staged local-agent binary discovery and registry hydration; adapt to our read-only discovery/negotiation contract. |
| `TASK-ECO-001`, `TASK-ECO-002`, `TASK-CHAN-003` | [AionCore `ea24f50`: `sync_service.rs`](https://github.com/iOfficeAI/AionCore/blob/ea24f50/crates/aionui-mcp/src/sync_service.rs), [`session_injection.rs`](https://github.com/iOfficeAI/AionCore/blob/ea24f50/crates/aionui-mcp/src/session_injection.rs); [OpenWork `8e52796`: `mcp.ts`](https://github.com/different-ai/openwork/blob/8e52796/apps/server/src/mcp.ts), [`connect-state.ts`](https://github.com/different-ai/openwork/blob/8e52796/apps/server/src/connect-state.ts) | Read-only native MCP inventory, negotiated session injection and host/project/global catalog merging; preserve custody and require explicit grants. |
| `TASK-ECO-002`, `TASK-SKILL-002`, `TASK-SKILL-003` | [OpenWork `8e52796`: `skills.ts`](https://github.com/different-ai/openwork/blob/8e52796/apps/server/src/skills.ts); [OpenCowork `a1d0e4a`: `skills-manager.ts`](https://github.com/OpenCoworkAI/open-cowork/blob/a1d0e4a/src/main/skills/skills-manager.ts), [`mcp-manager.ts`](https://github.com/OpenCoworkAI/open-cowork/blob/a1d0e4a/src/main/mcp/mcp-manager.ts) | Discovery locations, override precedence and manager lifecycle; adapt into separate host catalog, activation and grant scopes. |
| `TASK-AGENT-001`, `TASK-ECO-002`, `TASK-MISSION-001` | [OpenCowork `a1d0e4a`: `subagent-extension.ts`](https://github.com/OpenCoworkAI/open-cowork/blob/a1d0e4a/src/main/agent/subagent-extension.ts); [Orkas `75468ab`: `task_board.ts`](https://github.com/Orkas-AI/Orkas/blob/75468ab/src/main/features/group_chat/task_board.ts), [`plan_executor.ts`](https://github.com/Orkas-AI/Orkas/blob/75468ab/src/main/features/group_chat/plan_executor.ts), [`bus.ts`](https://github.com/Orkas-AI/Orkas/blob/75468ab/src/main/features/group_chat/bus.ts) | Child-session bounds, team mailbox/board and commander handoff. AionCore's team tree is additionally inventoried in `45`; an exact file must be pinned before using it as an implementation recipe. Work/Mission semantics are ours; an external agent's native child is never fabricated as host Work. |
| `TASK-MISSION-002`, `TASK-MISSION-003`, `TASK-VERIFY-001` | [Orkas `75468ab`: `reflection-orchestrator.ts`](https://github.com/Orkas-AI/Orkas/blob/75468ab/src/main/features/reflection-orchestrator.ts), [`task_board.ts`](https://github.com/Orkas-AI/Orkas/blob/75468ab/src/main/features/group_chat/task_board.ts) | Bounded reflection and the distinction between turn termination and quality; design reference for separate outcome evaluation, not reusable completion logic. |
| `TASK-WF-001`, `TASK-WF-002`, `TASK-LEARN-001` | [n8n `53dc2515`: `workflow-execute.ts`](https://github.com/n8n-io/n8n/blob/53dc2515/packages/core/src/execution-engine/workflow-execute.ts), [`active-workflow-manager.ts`](https://github.com/n8n-io/n8n/blob/53dc2515/packages/cli/src/active-workflow-manager.ts); [Activepieces `ba93a937`: `flow.operation.ts`](https://github.com/activepieces/activepieces/blob/ba93a937/packages/server/engine/src/lib/operations/flow.operation.ts), [`waitpoint-client.ts`](https://github.com/activepieces/activepieces/blob/ba93a937/packages/server/engine/src/lib/piece-context/waitpoint-client.ts) | Versioned graph execution, trigger activation and durable wait/resume. Use external provider adapters where installed; do not embed either server or create a second Core scheduler. |
| `TASK-COMMS-002` | [cowork-os `45de987`: `google-workspace-api.ts`](https://github.com/cowork-os/cowork-os/blob/45de987/src/electron/utils/google-workspace-api.ts), [`gmail-api.ts`](https://github.com/cowork-os/cowork-os/blob/45de987/src/electron/utils/gmail-api.ts), [`google-workspace.ts`](https://github.com/cowork-os/cowork-os/blob/45de987/src/shared/google-workspace.ts); [Google Workspace CLI `a3768d0`: `auth.rs`](https://github.com/googleworkspace/cli/blob/a3768d0/crates/google-workspace-cli/src/auth.rs), [`executor.rs`](https://github.com/googleworkspace/cli/blob/a3768d0/crates/google-workspace-cli/src/executor.rs) | Action-scoped Gmail/Drive/Sheets auth, refresh and typed command execution. A CLI adapter retains its own credential boundary; Core connectors use the vault. |
| `TASK-LEARN-001`, `TASK-UXQ-008` | [Google Workspace CLI `a3768d0`: `generate_skills.rs`](https://github.com/googleworkspace/cli/blob/a3768d0/crates/google-workspace-cli/src/generate_skills.rs); [OpenWork `8e52796`: `skills.ts`](https://github.com/different-ai/openwork/blob/8e52796/apps/server/src/skills.ts) | Generated/reusable skill patterns; our recorded-procedure promotion still requires review, versioning and evaluation. |
| `TASK-UXQ-001`, `TASK-UXQ-002` | [OpenChamber `8c70e98`: `ComposerFooter.tsx`](https://github.com/openchamber/openchamber/blob/8c70e98/packages/ui/src/components/chat/composer/ui/ComposerFooter.tsx), [`SessionGoalDialog.tsx`](https://github.com/openchamber/openchamber/blob/8c70e98/packages/ui/src/components/chat/SessionGoalDialog.tsx) | Concrete composer and continuing-goal interactions. Adapt under `48`'s progressive, nontechnical UX; source inspection is not a usability verdict. |
| `TASK-UXQ-003`, `TASK-UI-003`, `TASK-BROWSER-001` | [OpenChamber `8c70e98`: `BrowserPane.tsx`](https://github.com/openchamber/openchamber/blob/8c70e98/packages/ui/src/components/browser/BrowserPane.tsx), [`SidebarFilesTree.tsx`](https://github.com/openchamber/openchamber/blob/8c70e98/packages/ui/src/components/layout/SidebarFilesTree.tsx) | Browser/file panel state, selection and takeover surfaces for the Workbench; verify our own editor/document fidelity. |
| `TASK-MISSION-002`, `TASK-ECO-002` | [OpenChamber `8c70e98`: `fusion.ts`](https://github.com/openchamber/openchamber/blob/8c70e98/packages/ui/src/lib/multirun/fusion.ts) | Parallel result source-membership check; our integration and independent evidence contract remains separate. |
| `TASK-BROWSER-001`, `TASK-BROWSER-002`, `TASK-CUA-003` | [UI-TARS Desktop `c2ad42e`: `base-browser.ts`](https://github.com/bytedance/UI-TARS-desktop/blob/c2ad42e/packages/agent-infra/browser/src/base-browser.ts), [`operator.ts`](https://github.com/bytedance/UI-TARS-desktop/blob/c2ad42e/apps/ui-tars/src/main/agent/operator.ts) | Browser abstraction and desktop input seam as reference for structured-to-visual fallback. Verify platform behavior with our own acceptance matrix. |
| `TASK-FILES-003`, `TASK-WORLD-002`, `TASK-WORLD-003` | [SiYuan `60a4387`: `index_queue.go`](https://github.com/siyuan-note/siyuan/blob/60a4387/kernel/sql/index_queue.go) | Incremental local indexing and persisted queue handling; do not copy the note application's entire knowledge model. |
| `TASK-MODEL-004`, `TASK-ECO-002` | [llama.cpp server documentation](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md); [vLLM serving documentation](https://github.com/vllm-project/vllm/blob/main/docs/serving/online_serving/README.md) | Local inference interfaces only. A model endpoint needs a qualified external harness before it can be a delegated cowork worker. |
| `TASK-OFFICE-004`, `TASK-OFFICE-005` | [OfficeCLI `3442550`: README](https://github.com/officecli/officecli/blob/3442550/README.md) | Optional black-box office adapter/benchmark. Its public pin does **not** expose the execution engine; no source-level office algorithm is available to take from it. |

Other reviewed repositories are inventoried in `ARCH/45-REFERENCE-RESEARCH.md` §Additional pattern survey (Dify, Hermes Agent, nanobot, learn-claude-code, oh-my-claudecode, awesome-claude-skills, Cherry Studio, CowAgent, MindsHub, QwenPaw). Those entries do not yet establish an exact implementation file for an active task and therefore are not silently treated as a coding recipe. A task using one must first add a pinned file-level source and a verified observation here/`45`.

## Architecture owner index

Each active task row links to the exact heading of its first `REQ-*` in `ARCH/08-REQUIREMENTS.md`. The requirement's Source line names the precise design section; the map below gives the owning module entry point. W5 and W6 additionally name final interaction/implementation sections in their rows. Line numbers are a navigation snapshot for this revision and must be refreshed after changing `08` or an owner section.

| REQ domain | Owning design entry |
|---|---|
| `PROD` | `ARCH/03-HLD.md:9` (1. Shape) |
| `KERNEL` | `ARCH/10-KERNEL.md:9` (1. Scope) |
| `WORK` | `ARCH/11-WORK.md:10` (1. Purpose & responsibilities) |
| `TRUST` | `ARCH/12-TRUST.md:12` (1. Purpose & rules) |
| `CAP` | `ARCH/13-CAPABILITY.md:12` (1. Purpose & responsibilities) |
| `PROV` | `ARCH/14-PROVIDERS.md:12` (1. Purpose & rules) |
| `AGENT` | `ARCH/15-AGENT-PLANE.md:10` (1. Purpose & responsibilities) |
| `CTX` | `ARCH/16-CONTEXT.md:12` (1. The two-layer split) |
| `MEM` | `ARCH/17-MEMORY.md:12` (1. Purpose & responsibilities) |
| `MODEL` | `ARCH/18-MODEL-ROUTING.md:12` (1. Purpose & responsibilities) |
| `RTENV` | `ARCH/19-RUNTIME-ENVIRONMENTS.md:12` (1. Purpose & responsibilities) |
| `WF` | `ARCH/20-WORKFLOW.md:10` (1. Purpose & rules) |
| `WORLD` | `ARCH/21-WORLD-MODEL.md:14` (1. Purpose & honest framing) |
| `OFFICE` | `ARCH/22-OFFICE.md:12` (1. Purpose & rules) |
| `BROWSER` | `ARCH/23-BROWSER.md:12` (1. Purpose & rules) |
| `CUA` | `ARCH/24-COMPUTER-USE.md:13` (1. Purpose & rules) |
| `FILES` | `ARCH/25-FILES.md:12` (1. Purpose & rules) |
| `CODE` | `ARCH/26-CODE.md:10` (1. Purpose & rules) |
| `SEARCH` | `ARCH/27-SEARCH.md:10` (1. Purpose & rules) |
| `COMMS` | `ARCH/28-COMMS.md:12` (1. Purpose & rules) |
| `ART` | `ARCH/29-ARTIFACTS.md:12` (1. Purpose & responsibilities) |
| `EVENTS` | `ARCH/30-EVENTS.md:13` (1. Purpose & rules) |
| `SKILL` | `ARCH/31-SKILLS-PLUGINS.md:12` (1. Purpose & rules) |
| `CHAN` | `ARCH/32-CHANNELS.md:12` (1. Purpose & rules) |
| `VERIFY` | `ARCH/34-EFFECT-VERIFICATION.md:12` (1. Purpose & rules) |
| `UI` | `ARCH/48-EXPERIENCE-SURFACES.md:5` (1. Experience rule) |
| `MISSION` | `ARCH/35-MISSION.md:5` (Purpose and boundaries) |
| `ECO` | `ARCH/46-ECOSYSTEM-ARCHITECTURE.md:5` (1. Product contract and ownership) |
| `LEARN` | `ARCH/37-WORKFLOW-SKILL-LIFECYCLE.md:5` (Three distinct objects) |
| `UXQ` | `ARCH/48-EXPERIENCE-SURFACES.md:5` (1. Experience rule) |

## Current code state (baseline)

> Condensed from the frozen-tree inventory `ARCHIVE/v1-research/v1-sdd/code-state-inventory.md` (static inspection, 2026-09-26, HEAD `573fff0`; no builds or tests executed). State labels: `implemented` · `partial` · `stub` · `absent`.

**Shape.** 21 Rust crates (`crates/Cargo.toml`; 21/21 on disk; edition 2024; Rust 1.98) · 10 TypeScript packages (`packages/`) · Tauri shell with 42 `*_cmds.rs` + 16 `lib.rs` commands = **367 registered** (362 definitions counted; 306 UI invocations; `ipc-parity` 0 broken / 61 ghosts / 49 dead) · UI 309 TS/TSX files with **12 center screens** and **22 right-rail viewports — all wired**. Tests in-tree: 406 Rust `#[cfg(test)]` modules + 33 Rust integration files + 87 package test files + 68 UI test files. The Rust tree has almost no placeholder bodies (4 non-load-bearing `todo!()`-class hits); the gaps are missing capabilities and dormant superseded paths.

**Gates.**

| Gate | State (2026-09-26) |
|---|---|
| `scripts/check-arch-invariants.mjs` | pass (exit 0) |
| `scripts/ipc-parity.mjs` | pass — 367 registered / 362 counted / 306 UI-invoked / 0 broken / 61 ghosts |
| `scripts/check-doc-refs.mjs` | pass (0 backlog) |
| `scripts/check-doc-sync.mjs` | **red on main** — ENOENT on the archived `DESKTOP-APP-SPEC.md`; queued for re-homing (`AGENTS.md` §11). This plan does not fix it without a re-homing task |

**Per-module baseline** (`ARCH` module → current owner → state → first gap):

| Module | Owning code today | State | First gap (baseline) |
|---|---|---|---|
| `10-KERNEL` | `crates/agentcowork-types` (delegation/plane/turn_snapshot/workbench only), `agentcowork-ipc`, `core/src/config.rs` | partial | no unified id/error/time module; no `10-KERNEL` crate |
| `11-WORK` | `core/src/{execution,work_gateway,task_ledger}.rs`, `blueprint/{jobs,kanban,checkpoint}.rs`, 43 `work_*` + 8 `tasks_*` commands | implemented | v1 lanes/outer bounds and exact resume matrix not fully present |
| `12-TRUST` | `agentcowork-guard` (37 modules), `-vault` (15), `-audit` (8) | implemented | rate limiting absent (FIX-02); egress bypasses (FIX-09) |
| `13-CAPABILITY` | `guard/src/{capability_broker,capability_contract}.rs`, `core/src/capability_manifest.rs` | partial | no unified descriptor/resolver/census gate per `13` |
| `14-PROVIDERS` | `agentcowork-catalog` (19 mods), `mcp` (10), `acp` (12), `core/src/{providers,tools}.rs` | partial | MCP dual-era (FIX-12, FIX-13); direct `ureq` at `core/src/tools.rs:2414` |
| `15-AGENT-PLANE` | external-agent plumbing only (`acp` + `acp_cmds.rs` + coordinator) | partial (external bindings) | adapter capability negotiation, native-vs-Core provenance and durable child handoff remain incomplete; no first-party loop is required (DEC-052/054) |
| `16-CONTEXT` | `memory/src/{passport,compaction,summary}.rs`, `core/src/distill.rs`, `blueprint/plan_cache.rs`, `coordinator/src/context-trace.ts` | partial | no `ContextItem`/cache-stability infrastructure matching `16` |
| `17-MEMORY` | `agentcowork-memory` (34 modules), `core/src/memory_service.rs`, `packages/core-memory` | implemented (TS half dormant) | `core-memory` consumed only by `core-ai` + tests |
| `18-MODEL-ROUTING` | `catalog/{model,routing,routing_feed,pricing}.rs`, `core/src/routing.rs`, `model_cmds.rs` | partial | `coordinator/src/router.ts` self-declares no production consumer |
| `19-RUNTIME-ENVIRONMENTS` | `core/src/{supervisor,worker_pool,spool,terminal}.rs`, `guard/src/{sandbox,seccomp}.rs`, `runtime_cmds.rs`, `terminal_cmds.rs` | implemented | macOS/Windows sandbox backends release-gated |
| `20-WORKFLOW` | `blueprint/src/{workflow,automation,jobs}.rs`, `core/src/automation_runtime.rs`, 21 `scheduler_*` | partial | v1 durability model (occurrences/claims/resume) incomplete |
| `21-WORLD-MODEL` | no owner — fragments (`catalog/discovery.rs`, `storage/{usn,usn_winapi,walk}.rs`, `desktop/{apps,readiness}.rs`) | **absent** | FIX-10 (Windows identity) + FIX-11 (USN unwired) blocking |
| `22-OFFICE` | `agentcowork-office` (10 src files), 9 `office_*` + 7 `xlsx_*` | implemented (gaps) | FIX-14 resident/lease, FIX-15 PDF redact, FIX-16 fsync |
| `23-BROWSER` | `agentcowork-browser` (27), `-cdp` (6), 12 `browser_*`, `browse-view.tsx` | implemented (tiers partial) | tier/adapter conformance not exercised |
| `24-COMPUTER-USE` | `agentcowork-desktop` (17), 16 `desktop_*`, `desktop-view.tsx` | implemented | Windows acceptance missing; FIX-17, FIX-18 open |
| `25-FILES` | `agentcowork-storage` (19), 9 `fs_*` | partial | FIX-10 live; write leases absent |
| `26-CODE` | `agentcowork-codeintel` (17), 10 `git_*` + 1 `lsp_*`, worktrees in `core`+`blueprint`, `ide-workbench.tsx` | partial | IDE search/run/extensions are honest placeholders |
| `27-SEARCH` | `agentcowork-search` (3), `packages/core-search`, 3 `search_*` | implemented (egress partial) | governance of egress paths (with FIX-09) |
| `28-COMMS` | `core/src/{connectors,connector_hub,messaging,email}.rs`, `core-connectors` (39 files), 6 `calendar_*`, `connectors-panel.tsx` | partial | FIX-01 custody unverified; calendar parity audit missing |
| `29-ARTIFACTS` | 2 `artifact_*` commands, `audit/src/receipt.rs`, `blueprint/src/crystallize.rs` | partial | library/gateway/GC per `29` absent |
| `30-EVENTS` | `core/src/work_gateway.rs` journal, `spool.rs`, audit merkle/session_log, 5 `replay_*` + 2 `trajectory_*` | partial | no bus/replay/subscription surface per `30` |
| `31-SKILLS-PLUGINS` | `blueprint/src/{skill_store,plugin,plugin_manifest,marketplace}.rs`, `guard/src/skillstore.rs`, 4 `skills_*` | implemented (gaps) | FIX-05 live (`delete` unchecked) |
| `32-CHANNELS` | `channel_b.rs`, `coordinator/src/{agui,scheduler}.ts`, `core/src/openai_server.rs`, `openai_cmds.rs`, `acp/src/a2a.rs` | partial | Agent-gateway 7-item projection absent |
| `34-EFFECT-VERIFICATION` | `agentcowork-eval` (10 mods), `desktop/src/verify.rs`, `browser/src/diff.rs`, `office/src/conformance.rs`, `audit/src/receipt.rs` | partial | no unified verify/reconcile pipeline per `34` |

**Live security candidates (re-verify in W0 before fixing):**

| Item | Evidence (baseline) |
|---|---|
| FIX-05 | `blueprint/src/skill_store.rs:442-449` — `delete` joins a caller-supplied name to root and `remove_dir_all`s; `skills_cmds.rs:274-279` passes caller input straight through |
| FIX-06 | `fs_cmds.rs:129-137` — `fs_write_file` writes after `floor_user_file` only, no ticket; UI still calls it (`ui/src/lib/fs.ts:68`); `terminal_cmds.rs:300-305` records `AgentTicket` provenance with no visible mint |
| FIX-07 | `acp_cmds.rs:1676` — hardcoded `GovernedSession::SelfContained { channel_b: true }`; per the code comment this may be by design post-ADR-0005 — needs a decision, not a patch |
| FIX-09 | direct `ureq`: `core/src/tools.rs:2414` (comment claims pre-checked), `core/src/messaging.rs:35`, `vault/src/oauth.rs:1022-1048`; pre-flight unclear from static read |
| FIX-10 | `storage/src/walk.rs:146-157` — non-unix `dev_of`/`ino_of` → 0, `nlink_of` → 1; `dedup.rs:106-118` then reports 0 wasted bytes on Windows |

**Dormant / superseded code (decide deliberately in the owning wave — re-home or delete; never build on it):**

- `core/src/chat.rs` streaming relay (4,208 LOC; `start_stream` has no caller) — its header still cites the archived coordinator loop; superseded by DEC-010/DEC-027.
- 12 `coordinator` modules with no non-test consumer (`stream-session`, `resumable`, `waterfall`, `channel-a`, `fleet`, `goal`, `budget`, `guard`, `surfaces`, `h32`, `fabric`, `router`) + stale package description (“agent-loop engine”).
- `packages/core-ai` inference machinery — no production importer; owner unclear under `16`/`18`.
- `storage/src/usn*` unwired (FIX-11); `core-domain` `ProviderConfig.apiKey` legacy type (no writer found).

**Verification tooling to reuse (do not reinvent):** `scripts/e2e/security-gate.mjs` (S1/S2 suites) · `scripts/e2e/failure-injection.mjs` · `scripts/e2e/vertical-chat.mjs` · `scripts/clean-profile-boot-check.mjs` · `scripts/windows-acceptance.mjs` (always exits 2 — presence is not acceptance) · `scripts/release-qualify.mjs` (E1–E12).

## Waves

> The earlier `ARCH/42` §5 sequence has been amended by DEC-052/054/055/056: P0 fixes → shared foundations → external-agent binding and scoped ecosystem → Mission → domain and Experience journeys → remote/cloud and measured qualification. W0 security candidates still gate dependent work. Each wave updates `ARCH/09` traceability in the same change as its task rows; W6 is the current delivery integration plan.

### W0 — Security & correctness (fix register + P0 set)

> Every item is **re-verified against the current tree before it is fixed** (`ARCH/42` §4). This is the P0 wave of the register: SEC-1 vault custody, SEC-2 control-plane rate limiting, SEC-3 ACP permission bridge, SEC-21 UI-blob scan — plus the inventory's live candidates.

| ID | Task | Closes / REQ | Touched paths (current tree) | Test | Status |
|---|---|---|---|---|---|
| `TASK-COMMS-001` | Connector OAuth token custody → vault (never sidecar/UI-held) | `FIX-01` · `REQ-COMMS-003` · `REQ-TRUST-004` · `REQ-PROD-002`; `ARCH/08-REQUIREMENTS.md:2190` | `core/src/{connectors,connector_hub}.rs`, `vault/src/oauth.rs`, `oauth_cmds.rs`, `packages/core-connectors` | `TEST-COMMS-001` (pending) | `[ ]` |
| `TASK-TRUST-001` | Control-plane rate limiting for `nativeCall`/Guard | `FIX-02` · `REQ-TRUST-009`; `ARCH/08-REQUIREMENTS.md:356` | `guard/src/*`, `core/src/tools.rs`, `src-tauri/src/lib.rs` | `TEST-TRUST-001` (pending) | `[ ]` |
| `TASK-CHAN-001` | ACP permission bridge: once/always/reject + diff previews, fail-closed | `FIX-03` · `REQ-CHAN-006` · `REQ-TRUST-002`; `ARCH/08-REQUIREMENTS.md:2675` | `agentcowork-acp` (`client.rs`, `messages.rs`), `acp_cmds.rs`, guard window | `TEST-CHAN-001` (pending) | `[ ]` |
| `TASK-UI-001` | UI-blob + secret-corpus scan over prompts/logs/events/receipts | `FIX-04` · `REQ-PROD-002`; `ARCH/08-REQUIREMENTS.md:89` | `ui/src/**`, new scan script under `scripts/`, `scripts/e2e/security-gate.mjs` | `TEST-UI-001` (pending) | `[ ]` |
| `TASK-SKILL-001` | `SkillStore::delete` path handling + uninstall validation | `FIX-05` · `REQ-SKILL-010` · `REQ-TRUST-003`; `ARCH/08-REQUIREMENTS.md:2592` | `blueprint/src/skill_store.rs:442-449`, `skills_cmds.rs:274-279` | `TEST-SKILL-001` (pending) | `[ ]` |
| `TASK-FILES-001` | Unticketed `fs_*`/`terminal_run` authority audit (full path, not a spot fix) | `FIX-06` · `REQ-PROD-001` · `REQ-TRUST-003, REQ-TRUST-005` · `REQ-FILES-006`; `ARCH/08-REQUIREMENTS.md:80` | `fs_cmds.rs:129-137`, `terminal_cmds.rs:300-305`, `ui/src/lib/fs.ts:68`, `core/src/tools.rs` | `TEST-FILES-001` (pending) | `[ ]` |
| `TASK-CHAN-002` | Governance-badge truth (`SelfContained { channel_b: true }`) — decision first | `FIX-07` · `REQ-CHAN-003` · `REQ-TRUST-008`; `ARCH/08-REQUIREMENTS.md:2648` | `acp_cmds.rs:1676`, `ARCH/12` (decision), picker UI | `TEST-CHAN-002` (pending) | `[ ]` |
| `TASK-PROV-001` | `tool/commit` live driver | `FIX-08` · `REQ-PROV-002` · `REQ-ART-003, REQ-ART-012`; `ARCH/08-REQUIREMENTS.md:477` | `core/src/tools.rs`, `guard/src/ticket.rs`, `mcp/src/remote.rs` | `TEST-PROV-001` (pending) | `[ ]` |
| `TASK-PROV-002` | Egress outside Guard: direct `ureq` audit and repair | `FIX-09` · `REQ-TRUST-001` · `REQ-PROV-008` · `REQ-COMMS-013`; `ARCH/08-REQUIREMENTS.md:284` | `core/src/tools.rs:2414`, `core/src/messaging.rs:35`, `vault/src/oauth.rs:1022-1048`, `guard/src/netfloor.rs` | `TEST-PROV-002` (pending) | `[ ]` |
| `TASK-FILES-002` | Windows file identity — stop zeroing dev/ino/nlink on non-unix | `FIX-10` · `REQ-FILES-001, REQ-FILES-002` · `REQ-WORLD-003`; `ARCH/08-REQUIREMENTS.md:1842` | `storage/src/walk.rs:146-157`, `storage/src/dedup.rs:106-118`, `storage/src/cleanup.rs:69-76` | `TEST-FILES-002` (pending) | `[ ]` |
| `TASK-WORLD-001` | USN wired as the Windows delta source | `FIX-11` · `REQ-WORLD-002, REQ-WORLD-004, REQ-WORLD-005` · `REQ-FILES-004, REQ-FILES-005`; `ARCH/08-REQUIREMENTS.md:1236` | `storage/src/{usn,usn_winapi}.rs`, `storage/src/walk.rs`, `storage_cmds.rs` | `TEST-WORLD-001` (pending) | `[ ]` |
| `TASK-PROV-003` | MCP dual-era: client `_meta`/modern headers + façade `server/discover` + header validation | `FIX-12, FIX-13` · `REQ-PROV-004, REQ-PROV-005`; `ARCH/08-REQUIREMENTS.md:495` | `mcp/src/{remote,server,manager}.rs` | `TEST-PROV-003` (pending) | `[ ]` |
| `TASK-OFFICE-001` | Office resident document contexts + exclusive writer leases | `FIX-14` · `REQ-OFFICE-003`; `ARCH/08-REQUIREMENTS.md:1346` | `agentcowork-office` (new resident module), `office_cmds.rs`, `xlsx_cmds.rs` | `TEST-OFFICE-001` (pending) | `[ ]` |
| `TASK-OFFICE-002` | PDF redact removes content (not an annotation) | `FIX-15` · `REQ-OFFICE-006, REQ-OFFICE-007`; `ARCH/08-REQUIREMENTS.md:1373` | `agentcowork-office` PDF path, `office_cmds.rs` (`pdf_page_op`) | `TEST-OFFICE-002` (pending) | `[ ]` |
| `TASK-OFFICE-003` | fsync before atomic swap in the Office commit path | `FIX-16` · `REQ-OFFICE-004`; `ARCH/08-REQUIREMENTS.md:1355` | `agentcowork-office` (`commit`/`rollback`), `xlsx_cmds.rs` | `TEST-OFFICE-003` (pending) | `[ ]` |
| `TASK-CUA-001` | UIA collector hardening (AutomationId-as-hint, UIAccess limits) | `FIX-17` · `REQ-CUA-003, REQ-CUA-009, REQ-CUA-010` · `REQ-WORLD-007`; `ARCH/08-REQUIREMENTS.md:1557` | `desktop/src/platform/*` (Windows), `desktop/src/{ladder,verify}.rs` | `TEST-CUA-001` (pending) | `[ ]` |
| `TASK-CUA-002` | WGC readiness verification for window capture | `FIX-18` · `REQ-CUA-008` · `REQ-WORLD-010`; `ARCH/08-REQUIREMENTS.md:1602` | `desktop/src/platform/` (WGC path), `desktop_cmds.rs` | `TEST-CUA-002` (pending) | `[ ]` |
| `TASK-ART-003` | Bounded tool-result preview wired into the work/capability result path: write the artifact, then attach the ref | follow-up to `TASK-PROV-001` · `REQ-ART-007, REQ-ART-010` (fail-closed half: `REQ-ART-012`); `ARCH/08-REQUIREMENTS.md:2345` | `mcp/src/preview.rs:24-34` (`bounded_preview` has no production caller), `core/src/tools.rs:1605-1679` + `:1749-1754` (`compact_result`), `core/src/work_gateway.rs:5180-5198` (`record_artifact`), `artifact_cmds.rs` | `TEST-ART-003` (pending) | `[ ]` |
| `TASK-FILES-005` | Renderer-chosen read path resolved against the session's path scopes (typed denial + audit row — never per-read approval) | follow-up to `TASK-FILES-001` · `REQ-FILES-010, REQ-FILES-009`; `ARCH/08-REQUIREMENTS.md:1923` | `fs_cmds.rs:91-124` (`fs_read_file` takes a renderer path with no floor and no scope resolve; `:33-38` `fs_list_dir` likewise), `control.rs:135-147` (`floor_user_file` — write-side only), `guard/src/pathfloor.rs` | `TEST-FILES-005` (pending) | `[ ]` |
| `TASK-TRUST-011` | Rate-limit numbers become a named configuration entry (120 burst / 20 per second per key; 600 / 100 global) | follow-up to `TASK-TRUST-001` · `REQ-TRUST-009` (config ownership: `REQ-KERNEL-004`); `ARCH/08-REQUIREMENTS.md:356` | `guard/src/ratelimit.rs:143-160` (`RateLimitConfig::default`), `src-tauri/src/lib.rs:85-92` (`control_plane_limiter`), `core/src/tools.rs:1176-1191` | `TEST-TRUST-011` (pending) | `[ ]` |
| `TASK-PROV-006` | Sidecar MCP SDK pinned to an exact version (a caret range today, so a minor bump moves the wire surface) | follow-up to `TASK-PROV-003` · `REQ-PROV-004, REQ-PROV-005`; `ARCH/08-REQUIREMENTS.md:495` | `packages/core-search/package.json:29` (`^1.29.0`), `pnpm-lock.yaml:130`, `packages/core-search/src/mcp-client.ts:1-2` (sole importer) | `TEST-PROV-006` (pending) | `[ ]` |
| `TASK-PROV-007` | Retire the sidecar's own MCP client — the Rust `agentcowork-mcp` is the one implementation, so the TS client is a second one | follow-up to `TASK-PROV-003` · `REQ-PROV-004, REQ-PROV-005`; `ARCH/08-REQUIREMENTS.md:495` | `packages/core-search/src/mcp-client.ts:1-2`, `packages/core-search/package.json:29`, `packages/core-search/src/index.ts` (re-export, if any) | `TEST-PROV-007` (pending) | `[ ]` |
| `TASK-PROD-004` | Pre-existing red tests registered as known failures with owners, so the W0 gate reports an honest baseline | follow-up to `TASK-PROD-001` · `REQ-PROD-006` (release-qualification input with `TASK-PROD-003`, W1); `ARCH/08-REQUIREMENTS.md:125` | `core/src/{doctor,execution,rss_measure,scheduler_service,tools,work_gateway}.rs`, `core/src/lib.rs:389-440` | `TEST-PROD-004` (pending) | `[ ]` |

**W0 follow-up notes** (appended 2026-09-26; the register rows above stay at `[ ]` because unit tests never close a risky class — `ARCH/42` §1, and a Linux host is not a Windows acceptance record).

- **`TASK-ART-003`** — the helper's own module doc names the owner: the work/capability result path, the same place a receipt is emitted (`ARCH/29-ARTIFACTS.md` §3). The rule that makes this a correctness row and not a nicety is the doc's third property: the helper **never invents an artifact reference**, so a truncated result with no writer behind it is unreachable-by-design until this row lands. A second writer inside the protocol crate would be a second artifact store (`DEC-002`, `DEC-005`) — the write belongs to the artifact gateway, and the ref is set only after that write succeeds. Sequence: bounded preview → write the full bytes through the artifact gateway → `with_artifact_ref`.
- **`TASK-FILES-005`** — the scope-boundary denial is already the owner ruling (`ARCH/12-TRUST.md` "Read interception"; `ARCH/25-FILES.md` §7.1; `REQ-FILES-010`); what is missing is the resolution step. `fs_write_file` floors its path, `fs_read_file` and `fs_list_dir` do not, and neither resolves the caller's path against the session's `allowed_paths` / `read_only_paths`. Canonicalization precedes the check and is re-checked at use (`REQ-FILES-009`).
- **`TASK-TRUST-011`** — the numbers are a **product knob, not a spec constant**: the module already documents the sizing argument, which is a configuration argument. Default the entry, keep the shipped values as its defaults, and give it a home with `TASK-KERNEL-003` (config layering, W1) so the shell and the kernel read one shape. The user-visible home arrives with the settings registry (`TASK-UI-023`, W5); DEC-056 resolves its scope decision, while implementation remains open.
- **`TASK-PROV-006` / `TASK-PROV-007`** — one dependency, two obligations. Pin first (`packages/core-search/package.json:29` declares `^1.29.0`; the lock resolves `1.30.0`), then decide the retirement of the single TS MCP client — the crate has one importer (`mcp-client.ts:1-2`) and the Rust `agentcowork-mcp` owns the MCP plane, so a live TS client is a second engine. The licensing-ledger entry for non-adoption is **not** this row; another lane owns it.
- **`TASK-PROD-004`** — the baseline is measured, not assumed. The untouched pre-W0 tree, built in a worktree, reproduces the **identical** failure set, so none of these are regressions from the register work. Two `agentcowork-core` unit tests **cannot pass as written** (the defect is in the test, not the code): `tools::tests::p69_g2_deflection_is_not_downgradable_by_a_ticket_or_args_hash` (`core/src/tools.rs:4664`) — the `args-hash drift` guard at `:1556` returns first, so the deflection the test is meant to prove is never reached and `out["refused"]` is `null` at `:4684`; and `tools::tests::p64_facades_are_flat_unique_and_annotated` (`:5635`) — it asserts every façade id contains a dot, and `retrieve_original` (`:3849`) does not, so the invariant it states does not hold. **Twelve `agentcowork-core` lib failures** (the two above plus `doctor::tests::all_ok_when_everything_healthy`, `execution::tests::p64_auto_checkpoint_persists_and_fence_holds`, `rss_measure::tests::measure_tree_includes_self`, `scheduler_service::tests::{event_fire_matches_kind_filter_scope, frequency_policy_caps_fires, handle_dispatch_roundtrip}`, `work_gateway::tests::{execution_binding_and_lifecycle_events_are_explicit, journal_reopens_and_replays_without_duplicate_creation, p71_automation_work_requires_its_owning_session_and_children_inherit_it, p71_durable_wait_carries_its_condition_and_projects_honestly}`) and a **boot test that hangs** — in fact both boot tests never complete (`core/src/lib.rs:393` `boot_reports_ready` and `:421` `boot_does_not_initialize_heavy_subsystems`, each still running past 60 s, so the suite never returns a summary and must be bounded per-test to get a report at all). Each row gets an owner and a reason; a known failure is a claim with a name attached, never a silent skip, and a *new* red in this set is a regression until proven otherwise.

### W1 — Plane foundations

| ID | Task | REQ | Touched paths (current tree) | Test | Status |
|---|---|---|---|---|---|
| `TASK-PROD-001` | Governed-path conformance harness — no bypass path in any domain, adapter, UI or agent | `REQ-PROD-001, REQ-PROD-003, REQ-PROD-004`; `ARCH/08-REQUIREMENTS.md:80` | `core/src/tools.rs`, `guard/*`, `scripts/e2e/security-gate.mjs` | `TEST-PROD-001` (pending) | `[ ]` |
| `TASK-PROD-002` | Token discipline — zero model calls on deterministic operations | `REQ-PROD-005`; `ARCH/08-REQUIREMENTS.md:116` | office/browser/UI open-render paths, trace assertions | `TEST-PROD-002` (pending) | `[ ]` |
| `TASK-PROD-003` | Evidence-gated readiness + Windows acceptance matrix + release qualification | `REQ-PROD-006`; `ARCH/08-REQUIREMENTS.md:125` | `scripts/windows-acceptance.mjs`, `scripts/release-qualify.mjs`, `ARCH/42` records | `TEST-PROD-003` (pending) | `[ ]` |
| `TASK-KERNEL-001` | Minimal kernel: ids, time, canonical serialization + store conventions | `REQ-KERNEL-001, REQ-KERNEL-002, REQ-KERNEL-005, REQ-KERNEL-006`; `ARCH/08-REQUIREMENTS.md:136` | `agentcowork-types`, `agentcowork-ipc`, `core/src/config.rs` | `TEST-KERNEL-001` (pending) | `[ ]` |
| `TASK-KERNEL-002` | Typed error taxonomy + base envelope on every contract | `REQ-KERNEL-003, REQ-KERNEL-007`; `ARCH/08-REQUIREMENTS.md:154` | `agentcowork-types` (new error module), `agentcowork-ipc/src/handle.rs`, contract audit | `TEST-KERNEL-002` (pending) | `[ ]` |
| `TASK-KERNEL-003` | Config layering, versioning and validation | `REQ-KERNEL-004`; `ARCH/08-REQUIREMENTS.md:163` | `core/src/config.rs` + kernel config module | `TEST-KERNEL-003` (pending) | `[ ]` |
| `TASK-WORK-001` | One lifecycle, one scheduler: lanes, admission, outer bounds | `REQ-WORK-001, REQ-WORK-004`; `ARCH/08-REQUIREMENTS.md:201` | `core/src/{execution,work_gateway}.rs`, `blueprint/src/{jobs,kanban}.rs`, `work_cmds.rs`, `tasks_cmds.rs` | `TEST-WORK-001` (pending) | `[ ]` |
| `TASK-WORK-002` | Append-only session log + projections (ui-history, prompt-history, inbox, runs) | `REQ-WORK-002, REQ-WORK-008`; `ARCH/08-REQUIREMENTS.md:210` | `core/src/work_gateway.rs`, `audit` session_log, UI projections | `TEST-WORK-002` (pending) | `[ ]` |
| `TASK-WORK-003` | Durable resume, cancellation verbs, checkpoint cadence | `REQ-WORK-003, REQ-WORK-006, REQ-WORK-007`; `ARCH/08-REQUIREMENTS.md:219` | `core/src/execution.rs`, `blueprint/src/checkpoint.rs`, `work_cmds.rs` | `TEST-WORK-003` (pending) | `[ ]` |
| `TASK-WORK-004` | Budgets as maxima — warn / pause / surface, kill only by policy | `REQ-WORK-005`; `ARCH/08-REQUIREMENTS.md:237` | `core/src/execution.rs`, `audit/src/receipt.rs`, telemetry events | `TEST-WORK-004` (pending) | `[ ]` |
| `TASK-TRUST-002` | One decider, three layers, one approval primitive, irreducible catastrophic gate | `REQ-TRUST-002, REQ-TRUST-003, REQ-TRUST-006, REQ-TRUST-010`; `ARCH/08-REQUIREMENTS.md:293` | `guard/src/{decision,approval_policy,floors}.rs`, `guard_cmds.rs`, `core/src/tools.rs` | `TEST-TRUST-002` (pending) | `[ ]` |
| `TASK-TRUST-003` | Ticket binding + vault custody (use-only; rotation audited) | `REQ-TRUST-004, REQ-TRUST-005`; `ARCH/08-REQUIREMENTS.md:311` | `guard/src/ticket.rs`, `vault/src/{keyring,broker}.rs`, `vault_cmds.rs` | `TEST-TRUST-003` (pending) | `[ ]` |
| `TASK-TRUST-004` | Fail-closed trust + audit completeness (tamper-evident, access-controlled reads) | `REQ-TRUST-001, REQ-TRUST-007, REQ-TRUST-009`; `ARCH/08-REQUIREMENTS.md:284` | `guard/src/{netfloor,pathfloor}.rs`, `audit/src/{merkle,receipt}.rs`, `scripts/e2e/failure-injection.mjs` | `TEST-TRUST-004` (pending) | `[ ]` |
| `TASK-TRUST-005` | Projection-only external-agent boundary (least privilege per binding) | `REQ-TRUST-008`; `ARCH/08-REQUIREMENTS.md:347` | `guard/*`, `agentcowork-acp`, `agentcowork-mcp`, `channel_b.rs` | `TEST-TRUST-005` (pending) | `[ ]` |
| `TASK-CAP-001` | Unified capability registry: descriptor contract + census gate | `REQ-CAP-003, REQ-CAP-004, REQ-CAP-009, REQ-CAP-010`; `ARCH/08-REQUIREMENTS.md:394` | `guard/src/{capability_broker,capability_contract}.rs`, `core/src/capability_manifest.rs`, `agentcowork-mcp` façades | `TEST-CAP-001` (pending) | `[ ]` |
| `TASK-CAP-002` | Resolver, epoch-checked handles, deterministic failover | `REQ-CAP-002, REQ-CAP-007, REQ-CAP-008`; `ARCH/08-REQUIREMENTS.md:385` | `guard/src/capability_broker.rs`, `catalog/src/routing*.rs`, `core/src/tools.rs` | `TEST-CAP-002` (pending) | `[ ]` |
| `TASK-CAP-003` | Loading modes + guidance results + budgeted subsets (no flat dump) | `REQ-CAP-001, REQ-CAP-005, REQ-CAP-006`; `ARCH/08-REQUIREMENTS.md:376` | `core/src/capability_manifest.rs`, prompt assembly, `agentcowork-mcp` | `TEST-CAP-003` (pending) | `[ ]` |
| `TASK-PROV-004` | Adapter contract + declared classes + registry entry shape | `REQ-PROV-001, REQ-PROV-002, REQ-PROV-003, REQ-PROV-006, REQ-PROV-007, REQ-PROV-010`; `ARCH/08-REQUIREMENTS.md:468` | `agentcowork-catalog`, `agentcowork-mcp`, `agentcowork-acp`, `core/src/{providers,tools}.rs` | `TEST-PROV-004` (pending) | `[ ]` |
| `TASK-PROV-005` | Gateway client identity/session affinity + adapter egress and custody compliance | `REQ-PROV-008, REQ-PROV-009`; `ARCH/08-REQUIREMENTS.md:531` | `catalog/src/*`, `vault/src/broker.rs`, `guard/src/netfloor.rs` | `TEST-PROV-005` (pending) | `[ ]` |
| `TASK-AGENT-001` | External-agent delegation contract: every host child has Work identity, explicit isolation, deadline, lifecycle and evidence-bearing terminal receipt; native children remain agent-owned and honestly reported | `REQ-AGENT-001, REQ-AGENT-002, REQ-AGENT-003, REQ-AGENT-004`; `ARCH/08-REQUIREMENTS.md:1657` | `ARCH/15-AGENT-PLANE.md:63` (§7), `ARCH/46-ECOSYSTEM-ARCHITECTURE.md:51` (§5); `agentcowork-acp`, Work scheduler and artifact receipts | `TEST-AGENT-001` (pending) | `[ ]` |
| `TASK-CTX-001` | Named budget terms + pre-turn feasibility for Core-owned/adapter-supported inference; opaque engine budget only for offered packet | `REQ-CTX-002, REQ-CTX-005`; `ARCH/08-REQUIREMENTS.md:569` | `memory/src/compaction.rs`, `core/src/distill.rs`, context packet service, `coordinator/src/context-trace.ts`; `ARCH/16-CONTEXT.md` §3 | `TEST-CTX-001` (pending) | `[ ]` |
| `TASK-CTX-002` | Prune → compact → checkpoint pipeline; durable full output; log never rewritten | `REQ-CTX-006, REQ-CTX-007, REQ-CTX-008`; `ARCH/08-REQUIREMENTS.md:605` | `memory/src/{passport,compaction,summary}.rs`, `blueprint/src/plan_cache.rs`, artifact refs | `TEST-CTX-002` (pending) | `[ ]` |
| `TASK-CTX-003` | Cache stability + projections + non-touching reads | `REQ-CTX-001, REQ-CTX-004, REQ-CTX-009, REQ-CTX-010`; `ARCH/08-REQUIREMENTS.md:560` | new `ContextItem` infra (`16`), `memory/src/passport.rs`, `work_gateway` projections | `TEST-CTX-003` (pending) | `[ ]` |
| `TASK-CTX-004` | Two-layer split conformance — Core infrastructure vs agent control | `REQ-CTX-003`; `ARCH/08-REQUIREMENTS.md:578` | audit across `core` and the coordinator seams (an engine's own loop is the engine's, not ours) | `TEST-CTX-004` (pending) | `[ ]` |
| `TASK-MEM-001` | Store + extraction/write discipline (ADD-only, `superseded_by`, FTS5 triggers) | `REQ-MEM-001, REQ-MEM-002, REQ-MEM-007, REQ-MEM-008, REQ-MEM-009`; `ARCH/08-REQUIREMENTS.md:661` | `agentcowork-memory` (store path rework), `memory_cmds.rs` | `TEST-MEM-001` (pending) | `[ ]` |
| `TASK-MEM-002` | Recall, scoring and injection budget honesty | `REQ-MEM-003, REQ-MEM-004, REQ-MEM-012, REQ-MEM-015, REQ-MEM-019`; `ARCH/08-REQUIREMENTS.md:679` | `memory/src/{fusion,rerank,summary}.rs`, `core/src/memory_service.rs` | `TEST-MEM-002` (pending) | `[ ]` |
| `TASK-MEM-003` | Scope, isolation, sensitivity and projections | `REQ-MEM-005, REQ-MEM-006, REQ-MEM-013, REQ-MEM-014, REQ-MEM-020, REQ-MEM-022, REQ-MEM-023`; `ARCH/08-REQUIREMENTS.md:697` | `agentcowork-memory`, `guard` sensitivity filters, projection seams | `TEST-MEM-003` (pending) | `[ ]` |
| `TASK-MEM-004` | Forget, erasure integrity, growth bounds and retention | `REQ-MEM-010, REQ-MEM-016, REQ-MEM-017, REQ-MEM-026`; `ARCH/08-REQUIREMENTS.md:742` | `agentcowork-memory`, `agentcowork-audit` integration | `TEST-MEM-004` (pending) | `[ ]` |
| `TASK-MEM-005` | Ops: concurrency/single-writer, time correctness, schema/migration, inspect/export/import | `REQ-MEM-011, REQ-MEM-018, REQ-MEM-025, REQ-MEM-027`; `ARCH/08-REQUIREMENTS.md:751` | `agentcowork-memory`, store conventions (with `TASK-KERNEL-001`) | `TEST-MEM-005` (pending) | `[ ]` |
| `TASK-MEM-006` | Checkpoint authority + extractor budget/model policy/kill switch | `REQ-MEM-021, REQ-MEM-024`; `ARCH/08-REQUIREMENTS.md:841` | `memory/src/passport.rs`, `blueprint/src/checkpoint.rs`, model router (`18`) | `TEST-MEM-006` (pending) | `[ ]` |
| `TASK-MODEL-001` | One registry: descriptors + catalog-as-data | `REQ-MODEL-001, REQ-MODEL-002, REQ-MODEL-003`; `ARCH/08-REQUIREMENTS.md:906` | `agentcowork-catalog`, `model_cmds.rs` | `TEST-MODEL-001` (pending) | `[ ]` |
| `TASK-MODEL-002` | Deterministic routing + effort mapping + no silent downgrade | `REQ-MODEL-004, REQ-MODEL-011, REQ-MODEL-012`; `ARCH/08-REQUIREMENTS.md:933` | `catalog/src/{routing,routing_feed}.rs`, `core/src/routing.rs`, `coordinator/src/router.ts` (retire or re-home) | `TEST-MODEL-002` (pending) | `[ ]` |
| `TASK-MODEL-003` | Adapters, typed stream union, usage/cost invariant, retry ownership, watchdogs | `REQ-MODEL-005, REQ-MODEL-006, REQ-MODEL-007, REQ-MODEL-008, REQ-MODEL-009`; `ARCH/08-REQUIREMENTS.md:942` | `agentcowork-catalog`, `vault/src/broker.rs`, new adapter layer | `TEST-MODEL-003` (pending) | `[ ]` |
| `TASK-MODEL-004` | Local discovery + locality guarantee (Ollama/LM Studio/vLLM/llama.cpp) | `REQ-MODEL-010`; `ARCH/08-REQUIREMENTS.md:987` | `catalog/src/{discovery,probe}.rs`, `local_cmds.rs`, `runtime_cmds.rs` | `TEST-MODEL-004` (pending) | `[ ]` |
| `TASK-RTENV-001` | Process manager, environment records, health, bounded telemetry | `REQ-RTENV-003, REQ-RTENV-009, REQ-RTENV-010, REQ-RTENV-011`; `ARCH/08-REQUIREMENTS.md:1034` | `core/src/{supervisor,worker_pool,spool}.rs`, `runtime_cmds.rs` | `TEST-RTENV-001` (pending) | `[ ]` |
| `TASK-RTENV-002` | Sandbox backends fail closed + two PTY classes (Windows acceptance record) | `REQ-RTENV-001, REQ-RTENV-002, REQ-RTENV-005`; `ARCH/08-REQUIREMENTS.md:1016` | `guard/src/{sandbox,seccomp}.rs`, `core/src/terminal.rs`, `terminal_cmds.rs` | `TEST-RTENV-002` (pending) | `[ ]` |
| `TASK-RTENV-003` | Orphans, detached work, MCP lifecycle/epochs, helper consent, output bounds | `REQ-RTENV-004, REQ-RTENV-006, REQ-RTENV-007, REQ-RTENV-008`; `ARCH/08-REQUIREMENTS.md:1043` | `core/src/{supervisor,spool}.rs`, `agentcowork-mcp`, `terminal_cmds.rs` | `TEST-RTENV-003` (pending) | `[ ]` |
| `TASK-WF-001` | Typed IR, version pinning, composition (agent nodes, workflows-as-tools) | `REQ-WF-001, REQ-WF-002, REQ-WF-010`; `ARCH/08-REQUIREMENTS.md:1126` | `blueprint/src/{workflow,automation}.rs`, `core/src/automation_runtime.rs`, `scheduler_cmds.rs` | `TEST-WF-001` (pending) | `[ ]` |
| `TASK-WF-002` | Durability: deduped occurrences, one logical run per claim, authenticated event triggers, resume matrix, misfire and timezones; no exactly-once external-effect claim (DEC-057) | `REQ-WF-003, REQ-WF-004, REQ-WF-005, REQ-WF-006, REQ-WF-007`; `ARCH/08-REQUIREMENTS.md:1144` | `ARCH/20-WORKFLOW.md:41` (§Durability), `:91` (§Triggers); event store (`30`); remote owner transfer in `TASK-RTENV-005` | `TEST-WF-002` (pending) | `[ ]` |
| `TASK-WF-003` | Approvals, retry/timeout/concurrency bounds, run evidence | `REQ-WF-008, REQ-WF-009, REQ-WF-011`; `ARCH/08-REQUIREMENTS.md:1189` | `guard` approvals, `blueprint/src/jobs.rs`, `audit/src/receipt.rs` | `TEST-WF-003` (pending) | `[ ]` |
| `TASK-WORLD-002` | Collector set + per-kind identity, cursor/epoch, gap rescan | `REQ-WORLD-001, REQ-WORLD-002, REQ-WORLD-003, REQ-WORLD-004, REQ-WORLD-005`; `ARCH/08-REQUIREMENTS.md:1227` | `catalog/src/discovery.rs`, `storage/src/{walk,usn*}.rs`, `desktop/src/{apps,readiness}.rs` + new `21` module | `TEST-WORLD-002` (pending) | `[ ]` |
| `TASK-WORLD-003` | World graph, queries/subscriptions, freshness, consent, projections | `REQ-WORLD-006, REQ-WORLD-007, REQ-WORLD-008, REQ-WORLD-009, REQ-WORLD-010, REQ-WORLD-011`; `ARCH/08-REQUIREMENTS.md:1272` | new `21` module, `storage` index, UI query surface | `TEST-WORLD-003` (pending) | `[ ]` |

### W2 — Domain runtimes

| ID | Task | REQ | Touched paths (current tree) | Test | Status |
|---|---|---|---|---|---|
| `TASK-BROWSER-001` | Managed Chromium default, browser environments/lifecycle/recovery, attach/takeover, auth state | `REQ-BROWSER-001, REQ-BROWSER-002, REQ-BROWSER-003, REQ-BROWSER-009, REQ-BROWSER-010`; `ARCH/08-REQUIREMENTS.md:1429` | `agentcowork-browser`, `agentcowork-cdp`, `browser_cmds.rs`, `browse-view.tsx` | `TEST-BROWSER-001` (pending) | `[ ]` |
| `TASK-BROWSER-002` | BrowserWorld tabs/frames, bounded snapshots, refs, trusted input, structured-first, artifacts, no evasion | `REQ-BROWSER-004, REQ-BROWSER-005, REQ-BROWSER-006, REQ-BROWSER-007, REQ-BROWSER-008, REQ-BROWSER-011, REQ-BROWSER-012`; `ARCH/08-REQUIREMENTS.md:1456` | `browser/src/{ax,actions,tiers,capture}.rs`, `cdp`, `browse-view.tsx` | `TEST-BROWSER-002` (pending) | `[ ]` |
| `TASK-CUA-003` | Ladder-first actuation, per-platform matrix, epoch-scoped observations, bounded reads, elevation guidance | `REQ-CUA-001, REQ-CUA-002, REQ-CUA-003, REQ-CUA-004, REQ-CUA-005, REQ-CUA-006, REQ-CUA-007`; `ARCH/08-REQUIREMENTS.md:1539` | `desktop/src/{ladder,verify,readiness}.rs`, `desktop_cmds.rs`, `desktop-view.tsx` | `TEST-CUA-003` (pending) | `[ ]` |
| `TASK-CUA-004` | Untrusted screen content, gated/indicated/rate-limited input, confirmation thresholds, post-action verification | `REQ-CUA-008, REQ-CUA-009, REQ-CUA-010, REQ-CUA-011, REQ-CUA-012`; `ARCH/08-REQUIREMENTS.md:1602` | `desktop/src/*`, guard card path, `desktop_cmds.rs` | `TEST-CUA-004` (pending) | `[ ]` |
| `TASK-OFFICE-004` | One registry per format, L1→L3 access, resident/lease, crash-safe commit, batch atomicity | `REQ-OFFICE-001, REQ-OFFICE-002, REQ-OFFICE-003, REQ-OFFICE-004, REQ-OFFICE-005`; `ARCH/08-REQUIREMENTS.md:1328` | `agentcowork-office`, `office_cmds.rs`, `xlsx_cmds.rs` | `TEST-OFFICE-004` (pending) | `[ ]` |
| `TASK-OFFICE-005` | Declared limits, deterministic render/validate, risk hooks, formula recalc, templates, quarantine | `REQ-OFFICE-006, REQ-OFFICE-007, REQ-OFFICE-008, REQ-OFFICE-009, REQ-OFFICE-010, REQ-OFFICE-011`; `ARCH/08-REQUIREMENTS.md:1373` | `office/src/{conformance,limits,provenance}.rs`, `xlsx_cmds.rs` | `TEST-OFFICE-005` (pending) | `[ ]` |
| `TASK-FILES-003` | Incarnation-aware identity, re-keying, metadata index, watcher rescan, cursor recovery, freshness | `REQ-FILES-001, REQ-FILES-002, REQ-FILES-003, REQ-FILES-004, REQ-FILES-005, REQ-FILES-006`; `ARCH/08-REQUIREMENTS.md:1842` | `agentcowork-storage`, `fs_cmds.rs`, `storage_cmds.rs` | `TEST-FILES-003` (pending) | `[ ]` |
| `TASK-FILES-004` | Write leases, canonicalization, path scopes, workspace identity, unreadable-scope honesty | `REQ-FILES-007, REQ-FILES-008, REQ-FILES-009, REQ-FILES-010, REQ-FILES-011, REQ-FILES-012`; `ARCH/08-REQUIREMENTS.md:1896` | `agentcowork-storage`, `guard/src/pathfloor.rs`, `core` worktrees | `TEST-FILES-004` (pending) | `[ ]` |
| `TASK-CODE-001` | RepoGraph incremental store, labeled edges, bounded RepoMap, LSP bridge, refs-only retrieval | `REQ-CODE-001, REQ-CODE-002, REQ-CODE-003, REQ-CODE-004, REQ-CODE-005`; `ARCH/08-REQUIREMENTS.md:1952` | `agentcowork-codeintel`, `lsp_cmds.rs`, `ide-workbench.tsx` | `TEST-CODE-001` (pending) | `[ ]` |
| `TASK-CODE-002` | Worktrees per-spawn with merge/cleanup, policy-gated git, governed execution, bounded output | `REQ-CODE-006, REQ-CODE-007, REQ-CODE-008, REQ-CODE-009`; `ARCH/08-REQUIREMENTS.md:1997` | `blueprint/src/worktree.rs`, `core` worktrees, `git_cmds.rs`, `core/src/tools.rs` | `TEST-CODE-002` (pending) | `[ ]` |
| `TASK-CODE-003` | Index freshness, declared bounds/languages, generated-file provenance | `REQ-CODE-010, REQ-CODE-011, REQ-CODE-012`; `ARCH/08-REQUIREMENTS.md:2033` | `agentcowork-codeintel`, `git_cmds.rs` | `TEST-CODE-003` (pending) | `[ ]` |
| `TASK-SEARCH-001` | One implementation, model-free, scoped/sensitivity-filtered, projections, source adapters | `REQ-SEARCH-001, REQ-SEARCH-002, REQ-SEARCH-003, REQ-SEARCH-004, REQ-SEARCH-005, REQ-SEARCH-006`; `ARCH/08-REQUIREMENTS.md:2062` | `agentcowork-search`, `packages/core-search`, `search_cmds.rs`, coordinator `mcp-bridge` | `TEST-SEARCH-001` (pending) | `[ ]` |
| `TASK-SEARCH-002` | Canonical result shape, deterministic ranking, bounds, latency, staleness, references-not-answers | `REQ-SEARCH-007, REQ-SEARCH-008, REQ-SEARCH-009, REQ-SEARCH-010, REQ-SEARCH-011, REQ-SEARCH-012`; `ARCH/08-REQUIREMENTS.md:2116` | `agentcowork-search`, `search_cmds.ts` bridge, UI search surfaces | `TEST-SEARCH-002` (pending) | `[ ]` |
| `TASK-COMMS-002` | Connector lifecycle: descriptors/consent, risk classes, on-demand sync, sends gated, untrusted content, arrivals, isolation, failures | `REQ-COMMS-001, REQ-COMMS-002, REQ-COMMS-003, REQ-COMMS-004, REQ-COMMS-005, REQ-COMMS-006, REQ-COMMS-007, REQ-COMMS-008, REQ-COMMS-009, REQ-COMMS-010`; `ARCH/08-REQUIREMENTS.md:2172` | `core/src/{connectors,connector_hub,messaging,email}.rs`, `core-connectors`, `calendar_cmds.rs` | `TEST-COMMS-002` (pending) | `[ ]` |
| `TASK-COMMS-003` | `web.search`/`web.fetch` per DEC-037 — caps, TTL cache, citations, custody/egress/no-evasion | `REQ-COMMS-011, REQ-COMMS-012, REQ-COMMS-013`; `ARCH/08-REQUIREMENTS.md:2262` | `agentcowork-search`, `core/src/tools.rs`, `guard/src/netfloor.rs`, vault refs | `TEST-COMMS-003` (pending) | `[ ]` |

### W3 — Surfaces, events, verification

| ID | Task | REQ | Touched paths (current tree) | Test | Status |
|---|---|---|---|---|---|
| `TASK-ART-001` | Artifact + receipt models: immutable versions, mandatory provenance, library promotion, gateway refs | `REQ-ART-001, REQ-ART-002, REQ-ART-003, REQ-ART-004, REQ-ART-005, REQ-ART-006, REQ-ART-007`; `ARCH/08-REQUIREMENTS.md:2291` | `audit/src/receipt.rs`, `artifact_cmds.rs`, `artifact-view.tsx`, `blueprint/src/crystallize.rs` | `TEST-ART-001` (pending) | `[ ]` |
| `TASK-ART-002` | GC/retention vs pinned versions, previews (zero tokens), unresolved locations, atomic writes | `REQ-ART-008, REQ-ART-009, REQ-ART-010, REQ-ART-011, REQ-ART-012`; `ARCH/08-REQUIREMENTS.md:2354` | `artifact_cmds.rs`, `audit` retention, office/browser preview paths | `TEST-ART-002` (pending) | `[ ]` |
| `TASK-EVENTS-001` | One store + bus: typed vocabulary, refs-only, at-least-once/idempotent, backpressure/lag | `REQ-EVENTS-001, REQ-EVENTS-002, REQ-EVENTS-003, REQ-EVENTS-004, REQ-EVENTS-005, REQ-EVENTS-006`; `ARCH/08-REQUIREMENTS.md:2401` | `core/src/work_gateway.rs`, `spool.rs`, audit merkle, `coordinator/src/work-events.ts` | `TEST-EVENTS-001` (pending) | `[ ]` |
| `TASK-EVENTS-002` | Replay/retention/poison isolation, usage telemetry, filtered projections, ephemeral deltas | `REQ-EVENTS-007, REQ-EVENTS-008, REQ-EVENTS-009, REQ-EVENTS-010, REQ-EVENTS-011, REQ-EVENTS-012`; `ARCH/08-REQUIREMENTS.md:2455` | `replay_cmds.rs`, `trajectory_cmds.rs`, audit, telemetry path | `TEST-EVENTS-002` (pending) | `[ ]` |
| `TASK-SKILL-002` | Skill model/package, activation scoping, requirements via capability graph, guidance, untrusted instructions | `REQ-SKILL-001, REQ-SKILL-002, REQ-SKILL-003, REQ-SKILL-004, REQ-SKILL-012`; `ARCH/08-REQUIREMENTS.md:2511` | `blueprint/src/{skill_store,marketplace}.rs`, `guard/src/skillstore.rs`, `skills_cmds.rs` | `TEST-SKILL-002` (pending) | `[ ]` |
| `TASK-SKILL-003` | Plugin surfaces, review gate, sandboxed execution, versioning, crash auto-disable, uninstall, bounded extraction | `REQ-SKILL-005, REQ-SKILL-006, REQ-SKILL-007, REQ-SKILL-008, REQ-SKILL-009, REQ-SKILL-010, REQ-SKILL-011, REQ-SKILL-013`; `ARCH/08-REQUIREMENTS.md:2547` | `blueprint/src/{plugin,plugin_manifest}.rs`, `guard/src/skillstore.rs`, `guard/src/sandbox.rs` | `TEST-SKILL-003` (pending) | `[ ]` |
| `TASK-CHAN-003` | Agent Gateway: 7-item projection, workspace interception, identity/scoping, capability declarations, version typing | `REQ-CHAN-001, REQ-CHAN-002, REQ-CHAN-003, REQ-CHAN-004, REQ-CHAN-005, REQ-CHAN-010, REQ-CHAN-012`; `ARCH/08-REQUIREMENTS.md:2630` | `channel_b.rs`, `agentcowork-mcp` server, `guard` projections | `TEST-CHAN-003` (pending) | `[ ]` |
| `TASK-CHAN-004` | Approval routing, ACP typed-stream mapping, A2A opacity, CLI projection, disconnect cleanup | `REQ-CHAN-006, REQ-CHAN-007, REQ-CHAN-008, REQ-CHAN-009, REQ-CHAN-011, REQ-CHAN-013`; `ARCH/08-REQUIREMENTS.md:2675` | `acp/src/a2a.rs`, `core/src/{openai_server,messaging}.rs`, `openai_cmds.rs`, `replay_cmds.rs` | `TEST-CHAN-004` (pending) | `[ ]` |
| `TASK-VERIFY-001` | Risk→depth matrix as a floor, read-only pipeline order, receipt statements, stored-once records, hook contract/gaps | `REQ-VERIFY-001, REQ-VERIFY-002, REQ-VERIFY-003, REQ-VERIFY-004, REQ-VERIFY-005, REQ-VERIFY-006, REQ-VERIFY-007`; `ARCH/08-REQUIREMENTS.md:2758` | `agentcowork-eval`, `audit/src/receipt.rs`, domain verify hooks (`desktop/verify.rs`, `browser/diff.rs`, `office/conformance.rs`) | `TEST-VERIFY-001` (pending) | `[ ]` |
| `TASK-VERIFY-002` | Deterministic-first validators, consent-gated model/vision, reconciliation outcomes, bounded repair, observability | `REQ-VERIFY-008, REQ-VERIFY-009, REQ-VERIFY-010, REQ-VERIFY-011, REQ-VERIFY-012, REQ-VERIFY-013`; `ARCH/08-REQUIREMENTS.md:2821` | same paths + event stream (`30`), `agentcowork-eval` | `TEST-VERIFY-002` (pending) | `[ ]` |
| `TASK-UI-002` | Chat rendering: summarized reasoning, streaming-safe markdown, mermaid policy, tool states/grouping, plan bar, reasoning dial | `REQ-UI-001, REQ-UI-004, REQ-UI-005, REQ-UI-006, REQ-UI-007, REQ-UI-008`; `ARCH/08-REQUIREMENTS.md:1687` | `ui/src/components/**` (chat/message/tool/plan), `AGENTCOWORK-UI.md` | `TEST-UI-002` (pending) | `[ ]` |
| `TASK-UI-003` | Universal DocumentSurface, agent picker/owner model, truthful readiness/provenance | `REQ-UI-003, REQ-UI-009, REQ-UI-010, REQ-UI-011`; `ARCH/08-REQUIREMENTS.md:1705` | `office-*-view.tsx`, `right-rail.tsx`, `agent-model-picker.tsx`, `capability-status.ts` | `TEST-UI-003` (pending) | `[ ]` |
| `TASK-UI-004` | Keyboard-complete a11y, progressive disclosure, CLS=0 live regions, deterministic-first UI ops | `REQ-UI-002, REQ-UI-012, REQ-UI-013, REQ-UI-014`; `ARCH/08-REQUIREMENTS.md:1696` | `ui/src/components/**`, design tokens, `settings-panel.tsx` | `TEST-UI-004` (pending) | `[ ]` |
| `TASK-UI-042` | MCP effective era, its source and the persisted force-legacy hatch surfaced in the UI (the TS row type is narrower than the payload, so all three arrive untyped) | `REQ-UI-003, REQ-UI-013` · `REQ-PROV-004, REQ-PROV-005`; `ARCH/08-REQUIREMENTS.md:1705` | `ui/src/lib/mcp.ts:34-48` (`McpServerRow` has no `era` / `eraSource` / `forceLegacy`), `ui/src/components/panels/connectors-panel.tsx`, `mcp_cmds.rs:876-902` (`effective_era`), `:992-998`, `:1026-1037` (`mcp_remote_status`) | `TEST-UI-042` (pending) | `[ ]` |

**W3 follow-up note** (appended 2026-09-26). The command layer already returns the read-only era projection — `era.era` (the revision string in force), `era.eraSource` (`forced` · `cached` · `default`) and the persisted `forceLegacy` hatch — and computes it **without a probe**, so reading it is not a network round trip. The defect is the renderer side: `McpServerRow` (`ui/src/lib/mcp.ts:34-48`) declares none of the three, so the values deserialize untyped and no surface can render them. This row widens the type and renders it; it adds no command and no probe. Until it lands the hatch is unobservable in the UI, which is the honest state to record — the value exists and nothing shows it. Do not infer the era client-side (`REQ-UI-003`): the *source* is the whole point, since `forced` and `default` mean opposite things for a server that misreports its revision. The hatch is read-only in v1 — there is no user-facing toggle to build (`mcp_cmds.rs:992-998`).

### W4 — Retired 2026-09-27 (DEC-052)

The first-party native agent (`TASK-AGX-001…014`) is **not** an AgentCowork build item.
The engine is developed outside this repository and bound as an ordinary engine when it is
ready; the number is retired, not reused. The engine-agnostic obligations that lived in this
wave are re-seeded as `REQ-AGENT-001…004` in `ARCH/08-REQUIREMENTS.md` and tracked by
`TASK-AGENT-001` in W1.

### W5 — UI completeness (from the UX coverage audit, P10)

> **Evidence (read-only synthesis).** `ARCHIVE/v1-research/v1-sdd/ux-completeness-ledger.md` (726 lines; 209 rows `UXL-001…UXL-209` at 100 `present` / 53 `partial` / 53 `missing` / 3 `N/A`; P0 32 · P1 58 · P2 16), merged from three completed UI↔logic wiring studies of community-tested reference products. The merge pass edited no tracked file and ran no build, install or network. Every `ui/src` citation in the ledger was read in that pass; the nine rows marked `unverified against code` carry the check inline below.
>
> **Scope rule (this wave only).** A `partial` or `missing` row becomes a task — **106 rows → 53 tasks**. A `present` or `N/A` row never does; where a `present` row is the evidence that a behaviour is already shipped, it is cited inside a task's acceptance and is not work. Related rows are merged into one task and **every merged `UXL-###` id is listed in the task cell**; no distinct behaviour is dropped.
>
> **Id allocation.** The ledger's 50 proposed ids are suggestions, not authority. **48 kept verbatim**; **2 collided** with live W3 rows (`TASK-UI-002`, `TASK-UI-003`) and were re-allocated as the next free numbers of their domain, `TASK-UI-034…041`, minted *after* the kept series. Nothing is renumbered or reused (`ARCH/00-INDEX.md` §6).
>
> **Overlap with W0–W4.** No W0–W4 body is re-scoped here. Where a W5 task lands inside a live row's scope the dependency is named in the task cell; the live row keeps its id, REQ set and status. The cross-wave release gate above is stated for W0–W4 and is **not** edited by this wave — W5 inherits it.
>
> **`Doc` column provenance.** These W5 rows were drafted against `AGENTCOWORK-UI.md` and retain those baseline citations for source-path review. Each task row now also points to its final `ARCH/48-EXPERIENCE-SURFACES.md` section. Accessibility/quality lives in `ARCH/38-EXPERIENCE-QUALITY.md:5`; implementers must follow DEC-055/056 where the old baseline differs. In particular, a fixed six-slot rail and a fixed 44-section settings wall are not final requirements: the Workbench is multi-tab and Settings has searchable groups (`48` §§5, 7). W6 owns their end-to-end integration.
>
> **Decision reconciliation (2026-09-28).** DEC-056 resolves the five historical decision blocks on settings scope, named profiles, channel health, remote approval visibility and fork lineage. Their implementation remains open. The error-boundary source check found no boundary in `ui/src`, so `TASK-UI-029` is now an ordinary pending implementation task. The original ledger remains historical evidence, not the active decision state.

#### W5-P0 — 22 tasks · 57 ledger rows (32 P0 · 21 P1 · 4 P2)

Re-cut by owning behaviour from the ledger's 24-line P0 register; the register's own merges are kept and the controls that held their own P0 row are named in the acceptance. A task sits in the group of its **highest**-priority row, so a P0 behaviour merged with P1/P2 rows carries those rows with it.

| ID | Task | REQ | Doc | Touched paths (current tree) | Acceptance (ledger) | Test | Status |
|---|---|---|---|---|---|---|---|
| `TASK-UI-005` | Icon-only control register: every icon-only control carries an accessible name *and* a tooltip, plus an audit test that fails the build (UXL-002, UXL-003, UXL-016, UXL-021, UXL-031, UXL-033, UXL-034, UXL-035, UXL-044, UXL-045, UXL-046, UXL-047, UXL-110, UXL-161, UXL-162, UXL-164, UXL-171 — 17 rows, 6 P0) | `REQ-UI-012`; `ARCH/08-REQUIREMENTS.md:1786` | `AGENTCOWORK-UI.md` §9.1 (UI-12); final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `chat-panel.tsx:479-487,507-515,516-527,528-532,687-701,725-733`; `left-sidebar.tsx:468-487`; `right-rail.tsx:430-450,984-994,1000-1010,1060-1075`; `message-bubble.tsx:660-694`; `notifications-popover.tsx:227-243` | "A test iterates icon-only buttons and fails on a missing accessible name; the audit finds zero." + "No icon-only control ships without both an accessible name and a tooltip." Four controls hold their own P0 row: in-conversation search / prev / next / close, the ⋯ menu trigger (neither `aria-label` nor `title`), rail tab close, add-view | `TEST-UI-005` (pending) | `[ ]` |
| `TASK-UI-010` | Named, keyboard-complete dialogs replace every `window.confirm` / `window.prompt` (UXL-036, UXL-038, UXL-185 — 3 rows) | `REQ-UI-012`, `REQ-UI-013`; `ARCH/08-REQUIREMENTS.md:1786` | `AGENTCOWORK-UI.md` §5.10, §9.1 (UI-12); final `ARCH/48-EXPERIENCE-SURFACES.md:86` | 8 sites: `chat-panel.tsx:228,242,264,290`; `progress-view.tsx:197`; `setup-gate.tsx:125`; `agents-models-section.tsx:913`; `agent-model-picker.tsx:387` | "No destructive action uses `window.confirm`/`prompt`; each is a named, keyboard-complete dialog stating the consequence." + "Every destructive menu item opens a named, keyboard-complete dialog stating the consequence (UI-12; not `window.confirm`)." + "Goal entry is a field, not a blocking prompt." | `TEST-UI-010` (pending) | `[ ]` |
| `TASK-UI-011` | The Workbench is a region, not a power-mode gate (UXL-055) | `REQ-UI-012`, `REQ-UXQ-005`; `ARCH/08-REQUIREMENTS.md:1786` | `AGENTCOWORK-UI.md` §2.2 (R41; `OQ-UI-002`); final `ARCH/48-EXPERIENCE-SURFACES.md:5` | `ui/src/App.tsx:121-122` | "The Workbench is a region, not a mode; casual reduces density inside it, never its existence." | `TEST-UI-011` (pending) | `[ ]` |
| `TASK-UI-013` | Reasoning dial: model-derived levels, capability-negotiated (UXL-069) | `REQ-UI-008`; `ARCH/08-REQUIREMENTS.md:1750` | `AGENTCOWORK-UI.md` §5.4; final `ARCH/48-EXPERIENCE-SURFACES.md:26` | `ui/src/lib/acp.ts:93-101`; `ui/src/components/chat/agent-model-picker.tsx:482` | "The dial offers only levels the model descriptor supports, clamps on switch, keeps `auto` off the track, and names the current level." The data is already on the wire. Persistence scope is open as `OQ-UI-004`. | `TEST-UI-013` (pending) | `[ ]` |
| `TASK-UI-014` | `@` produces typed entity references; the host `/` namespace is visible *beside* the agent's own vocabulary (UXL-071, UXL-072) | `REQ-UI-012`, `REQ-UXQ-003`; `ARCH/08-REQUIREMENTS.md:1786` | `AGENTCOWORK-UI.md` §5.6, §5.7; final `ARCH/48-EXPERIENCE-SURFACES.md:26` | `ui/src/lib/at-refs.ts:4-13`; `chat-composer.tsx:58-61,544-557,678-710,741-745` | "`@` produces removable typed chips and emits an `embeddedContext` block only for kinds the agent advertised." + "A labelled `Host commands` group is visible *while* the agent's vocabulary is shown; fuzzy ranking applies to both." Today: regex refs plus literal `@files` / `@terminal`. Open as `OQ-UI-006` / `OQ-UI-012`. | `TEST-UI-014` (pending) | `[ ]` |
| `TASK-UI-019` | Plan bar bound to the running turn + verification drill-down from the step it verifies (UXL-096, UXL-097) | `REQ-UI-007`, `REQ-UXQ-001`; `ARCH/08-REQUIREMENTS.md:1741` | `AGENTCOWORK-UI.md` §4.4; final `ARCH/48-EXPERIENCE-SURFACES.md:41` | `ui/src/lib/bridge.ts:461-469`; `ui/src/lib/store.ts:253-263`; `views/blueprint-view.tsx:99-109`; `views/kanban-view.tsx:58` | "The plan bar appears only while its turn runs and disappears with it; a second version reports a delta and never claims 'no change' (UI-ACC-4)." + "A verification record renders as a drill-down from its plan step and an ambiguous `null` never reads as executed." `planStart` / `planStep` land in a Now-Doing strip today. | `TEST-UI-019` (pending) | `[ ]` |
| `TASK-UI-023` | Settings registry: exactly one key per user-visible setting, with scope, parser, owner and live copy (UXL-118, UXL-119, UXL-147) | `REQ-UI-015`; `ARCH/08-REQUIREMENTS.md:1813` | `AGENTCOWORK-UI.md` §5.13; `ARCH/13-CAPABILITY.md` §6; final `ARCH/48-EXPERIENCE-SURFACES.md:70` | `ui/src/lib/ui-prefs.ts:1-12,40-64`; `ui/src/lib/store.ts:947-965`; `ui/src/components/panels/settings-panel.tsx:100-190,362-379` | "Every user-visible setting has exactly one registered key with scope/parser/owner; an unknown or invalid key is dropped, never defaulted." + "Each setting declares its scope; a device-local choice never flips another device." + "A registered section's fields each declare a scope and an owner; an unowned field is a defect, not a default." Today: ad-hoc `usePref` calls under one flat `agentcowork.settings.` prefix, plus a second key scheme for permission mode. | `TEST-UI-023` (pending) | `[ ]` Decision resolved by DEC-056, DM-040 and CTR-032; implementation pending. |
| `TASK-UI-027` | Stream-health phases `connecting · live · reconnecting · auth-blocked · stale` with a bounded backoff (UXL-153, UXL-196) | `REQ-UI-003`; `ARCH/08-REQUIREMENTS.md:1705` | `AGENTCOWORK-UI.md` §6.2; `ARCH/30-EVENTS.md` §3; final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/components/shell/runtime-status-banner.tsx:45-66`; `ui/src/lib/store.ts:68-78`; `chat-panel.tsx:427-436` | "The run surface shows all five phases from real transitions, with a bounded backoff that never dead-ends." + "The UI says *why* events stopped, with a named phase and a bounded retry." Reads phases from the projection delivered by `TASK-EVENTS-003`; the phase/source split out of `SessionStatus` is `TASK-WORK-008`. | `TEST-UI-027` (pending) | `[ ]` |
| `TASK-UI-029` | App-level error boundary + a timed boot recovery region (UXL-167, UXL-168) | `REQ-UI-016`; `ARCH/08-REQUIREMENTS.md:1822` | `AGENTCOWORK-UI.md` §9.1; final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/**`; `ui/src/components/shell/runtime-status-banner.tsx` | A render exception shows Reload and copyable diagnostics; a slow or failed boot shows a timed status with Retry. Source check 2026-09-28 found no `ErrorBoundary`, `componentDidCatch` or `getDerivedStateFromError` in `ui/src`. | `TEST-UI-029` (pending) | `[ ]` |
| `TASK-UI-034` | Answer body renders the full token palette (headings, tables, blockquote, `hr`, `ol`, `img`) (UXL-085) | `REQ-UI-004`; `ARCH/08-REQUIREMENTS.md:1714` | `AGENTCOWORK-UI.md` §4.1; final `ARCH/48-EXPERIENCE-SURFACES.md:41` | `ui/src/components/chat/message-bubble.tsx:910-921,136-145`; `ui/package.json` (no `@tailwindcss/typography` plugin) | "A message with a heading, a table and a 400-line fence renders correctly from tokens (UI-ACC-1)." | `TEST-UI-034` (pending) | `[ ]` |
| `TASK-UI-035` | Streaming renderer: block cache, coalesced writes, closed-fence highlighter gate, rAF delta lanes (UXL-086, UXL-155) | `REQ-UI-004`; `ARCH/08-REQUIREMENTS.md:1714` | `AGENTCOWORK-UI.md` §4.1; final `ARCH/48-EXPERIENCE-SURFACES.md:41` | `ui/src/lib/bridge.ts:322-329`; `ui/src/lib/store.ts:2370-2377`; `message-bubble.tsx` | "At most one store write per animation frame; an open fence is never re-highlighted." + "Deltas coalesce with `requestAnimationFrame` on the visible session and a slower lane offscreen; a perf test bounds re-renders." | `TEST-UI-035` (pending) | `[ ]` |
| `TASK-UI-036` | Mermaid auto-conversion in the transcript through the policy-gated, `secure`-locked blob-`<img>` path (UXL-087) | `REQ-UI-005`; `ARCH/08-REQUIREMENTS.md:1723` | `AGENTCOWORK-UI.md` §4.2; final `ARCH/48-EXPERIENCE-SURFACES.md:41` | `ui/src/components/views/generative/generative-ui.tsx:295-347` (`:300` hardcoded `theme:'dark'`, `:341` `dangerouslySetInnerHTML`) | "A fenced `mermaid` block converts automatically through the gated blob-`<img>` path; a rejected diagram stays copyable source (UI-ACC-2)." Read the Mermaid config schema before coding — `AGENTCOWORK-UI.md` §13 carries it as `UNVERIFIED` (`OQ-UI-003`). | `TEST-UI-036` (pending) | `[ ]` |
| `TASK-UI-037` | Tool-call state union: five states, end to end (UXL-089) | `REQ-UI-006`; `ARCH/08-REQUIREMENTS.md:1732` | `AGENTCOWORK-UI.md` §4.3; final `ARCH/48-EXPERIENCE-SURFACES.md:41` | `ui/src/lib/store.ts:85`; `ui/src/lib/tauri.ts`; `ui/src/lib/bridge.ts` | "`ToolCallRecord.status` accepts `proposed` and `cancelled`; a `tool.proposed` event is reachable end to end." Today: `running · done · failed`. Depends on the `tool.proposed` event from `TASK-EVENTS-001` (W3). | `TEST-UI-037` (pending) | `[ ]` |
| `TASK-UI-039` | DocumentSurface tab identity: one tab per document identity, N documents per kind (UXL-053) | `REQ-UI-009`; `ARCH/08-REQUIREMENTS.md:1759` | `AGENTCOWORK-UI.md` §3, §2.3; final `ARCH/48-EXPERIENCE-SURFACES.md:47` | `ui/src/lib/store.ts:1112-1117`; `ui/src/components/shell/right-rail.tsx:377-463` | "Two spreadsheets and a PDF open together; one tab per document identity, N per kind." `officePaths` holds one path per view; `officeHistory` is a switcher, not tabs. Lands inside the W3 row `TASK-UI-003` (`REQ-UI-009`), which is not re-scoped here. | `TEST-UI-039` (pending) | `[ ]` |
| `TASK-CTX-005` | Context inspector panel + the composer→inspector route (UXL-056, UXL-070) | `REQ-CTX-011`; `ARCH/08-REQUIREMENTS.md:650` | `ARCH/16-CONTEXT.md` §7; `AGENTCOWORK-UI.md` §2.2; final `ARCH/48-EXPERIENCE-SURFACES.md:26` | new panel; `ui/src/lib/store.ts:265-269`; `ui/src/components/shell/status-bar.tsx:95-169`; `ui/src/components/chat/chat-composer.tsx` | "The Context slot opens an inspector with pin/exclude/focus/inspect and 'optimize now' that never forces compaction." + "The composer's context readout opens the same inspector as the Context slot." Today only `StreamStats.ctxPct` exists. Needs the snapshot from `TASK-CTX-001` / `TASK-CTX-002` (W2). | `TEST-CTX-005` (pending) | `[ ]` |
| `TASK-WORK-006` | `Run ▾` — now / background / workflow / schedule — plus the queue-or-steer choice (UXL-075, UXL-077) | `REQ-WORK-001`, `REQ-WORK-004`; `ARCH/08-REQUIREMENTS.md:201` | `ARCH/11-WORK.md`; `AGENTCOWORK-UI.md` §5.9 (R55); final `ARCH/48-EXPERIENCE-SURFACES.md:26` | `ui/src/components/chat/chat-composer.tsx:427`; `ui/src/components/chat/pending-queue-chips.tsx`; `ui/src/lib/store.ts:1308-1325` | "One send affordance expresses four scheduling intents; lanes and limits stay Core's." + "While busy, the user chooses queue **or** steer, and the choice is remembered per session." Open as `OQ-UI-005` (is background a new `Work`?); needs `TASK-WORK-001` (W1). | `TEST-WORK-006` (pending) | `[ ]` |
| `TASK-TRUST-006` | Approval card renders the four ticket scopes and an `unavailable` outcome (UXL-102, UXL-103) | `REQ-TRUST-002`; `ARCH/08-REQUIREMENTS.md:293` | `ARCH/12-TRUST.md` §5; `AGENTCOWORK-UI.md` §5.10; final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/components/chat/mcq-interrupt-card.tsx:290-298,374` | "The card renders the four ticket scopes, hides a scope the backend will not honour, and opens a second confirmation for a persistent grant." + "An `unavailable` decision renders as not-granted with a next action; a timeout is never shown as a decision." Unverified row — check: grep `unavailable` across `ui/src/components/chat/` and the bridge error branches. Depends on `TASK-CHAN-001` (W0). | `TEST-TRUST-006` (pending) | `[ ]` |
| `TASK-TRUST-007` | Approval card: focus trap, focus restore, Escape = deny, requester attribution, full keyboard path (UXL-104, UXL-105, UXL-183) | `REQ-TRUST-002`, `REQ-UI-012`; `ARCH/08-REQUIREMENTS.md:293` | `ARCH/12-TRUST.md` §5; `AGENTCOWORK-UI.md` §5.10; final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/components/chat/mcq-interrupt-card.tsx` | "The card traps focus, restores the previously focused element on close, and treats Escape as deny; each is covered by a DOM test." + "The card names the requesting session/subagent." + "An approval is fully decidable from the keyboard, and Escape is deny (fail-closed)." Per-card keyboard ownership, never a module-level stack. | `TEST-TRUST-007` (pending) | `[ ]` |
| `TASK-TRUST-009` | Approval / auto-accept authority moves to Core with a monotonic revision (UXL-122) | `REQ-TRUST-003`, `REQ-TRUST-006`; `ARCH/08-REQUIREMENTS.md:302` | `ARCH/12-TRUST.md` §1; final `ARCH/48-EXPERIENCE-SURFACES.md:70` | `ui/src/lib/store.ts:947-965` (`permissionMode` in `window.localStorage`), `:2198` | "Policy is read from and written to Core; a stale client completion cannot flip it; the UI renders the authoritative value. **Invariant: `INV-01`.**" Depends on `TASK-TRUST-002` / `TASK-TRUST-003` (W1) and on `TASK-UI-023`. | `TEST-TRUST-009` (pending) | `[ ]` |
| `TASK-EVENTS-003` | Reconcile degradation, orphaned-interaction settling, a first-class health event family, and "sync failure ≠ empty" (UXL-154, UXL-156, UXL-157, UXL-197, UXL-200) | `REQ-EVENTS-002`, `REQ-EVENTS-007`, `REQ-EVENTS-009`; `ARCH/08-REQUIREMENTS.md:2410` | `ARCH/30-EVENTS.md` §3; final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/lib/bridge.ts:322-329`; `ui/src/lib/store.ts:68-78`; `ui/src/components/chat/tool-chip.tsx:498-507`; each list view's fetch path | "When the engine status cannot be confirmed, `running` stops reading as confident progress and the last-confirmed time is shown." + "An orphaned interaction settles to a terminal state on reconcile, never to a live spinner." + "Health is a first-class event family, not a client-side derivation." + "A degraded projection shows the last-confirmed time and stops claiming progress." + "Every list surface distinguishes failed from empty; a failed scope preserves cached entities." Verify: read each list view's fetch path. | `TEST-EVENTS-003` (pending) | `[ ]` Decision resolved by DEC-056 and `ARCH/30-EVENTS.md` §3; implementation pending. |
| `TASK-CHAN-007` | Durable, channel-routed approvals + a local receipt for a decision taken on another channel (UXL-205, UXL-206) | `REQ-CHAN-006`, `REQ-TRUST-002`; `ARCH/08-REQUIREMENTS.md:2675` | `ARCH/32-CHANNELS.md` §7, §8; final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/**` (no pairing / expiry / approve-reject surface); `channel_b.rs`; `acp/src/a2a.rs` | "An approval survives a channel disconnect, re-surfaces on attach routed to the owning binding, and is visible in the local Guard/audit surface." + "An approval decided on a remote channel is recorded and visible in the local Guard/audit surface." Overlaps the W3 row `TASK-CHAN-004` (`REQ-CHAN-006`), which is not re-scoped here. | `TEST-CHAN-007` (pending) | `[ ]` Decision resolved by DEC-056: Core-mediated decisions use Core and appear locally; native approvals retain native provenance. |
| `TASK-VERIFY-004` | Automated accessibility gate in CI (UXL-187) | `REQ-UI-012`; `ARCH/08-REQUIREMENTS.md:1786` | `AGENTCOWORK-UI.md` §9.1 (G25); final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/scripts/`; `ui/package.json` | "A CI a11y pass runs green on shell + chat + settings and fails the build on a new violation." Must land after `TASK-UI-005`, `TASK-UI-010` and `TASK-UI-029` or it fails on day one; it extends the existing DOM-test baseline rather than replacing it. | `TEST-VERIFY-004` (pending) | `[ ]` |

#### W5-P1 — 23 tasks · 39 ledger rows (37 P1 · 2 P2)

| ID | Task | REQ | Doc | Touched paths (current tree) | Acceptance (ledger) | Test | Status |
|---|---|---|---|---|---|---|---|
| `TASK-UI-006` | Session-row context menu: archive / restore / forget / rename under one shared confirm policy (UXL-007) | `REQ-UI-012`; `ARCH/08-REQUIREMENTS.md:1786` | `AGENTCOWORK-UI.md` §9.1; final `ARCH/48-EXPERIENCE-SURFACES.md:11` | `ui/src/components/shell/left-sidebar.tsx:239-274`; `ui/src/components/chat/chat-panel.tsx:104-120,536-580` | "A row exposes a menu whose destructive items are named, keyboard-reachable and confirmed by one shared policy." The confirm policy is the dialog from `TASK-UI-010`; no context menu exists on the row today. | `TEST-UI-006` (pending) | `[ ]` |
| `TASK-UI-008` | Shared loading / empty / error / stale states reach every data-bearing surface, with a lint that enumerates the rest (UXL-025, UXL-052) | `REQ-UI-003`, `REQ-UI-013`; `ARCH/08-REQUIREMENTS.md:1705` | `AGENTCOWORK-UI.md` §9.2; final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/empty-state.tsx`, `ui/error-state.tsx`, `ui/loading-state.tsx` (4 of ~40 importers); `ui/src/components/views/**`; `ui/src/components/panels/**` | "Every data-bearing center screen and rail viewport renders one of the three shared states; a lint enumerates the ones that do not." + "Each of the 22 viewports documents a loading, empty, error and stale state with a screenshot per state." The two primitives this builds on are `present` and are evidence, not work: `UXL-191` "Adoption reaches every data-bearing view; a lint enumerates the remainder" (`ui/empty-state.tsx:37`) and `UXL-192` "An error names the cause and offers a route out (`Open-Settings`-class action)" (`ui/error-state.tsx:67-93`). | `TEST-UI-008` (pending) | `[ ]` |
| `TASK-UI-012` | Telemetry pills name what they measure, and every disabled mutating control discloses why (UXL-060, UXL-163) | `REQ-UI-012`, `REQ-UI-013`; `ARCH/08-REQUIREMENTS.md:1786` | `AGENTCOWORK-UI.md` §1.2 (UI-12, UI-04); final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/components/shell/status-bar.tsx:151-164`; dimmed-only: `chat-panel.tsx:492-506,534-582`; `right-rail.tsx:1025-1029`; `tool-chip.tsx` retry | "Each pill's accessible name is a word ('throughput', 'cache hit rate'), not an abbreviation." + "A disabled mutating control exposes *why* (and who can change it) via tooltip / `aria-describedby`; a test asserts the reason string." UXL-060 **corrects all three studies**: the accessible name is present, the label is cryptic. | `TEST-UI-012` (pending) | `[ ]` |
| `TASK-UI-015` | `+` attach/create menu widened to displayable kinds, attachments rendered as a list (UXL-073, UXL-074) | `REQ-UXQ-003`; `ARCH/08-REQUIREMENTS.md:3030` | `AGENTCOWORK-UI.md` §5.8; final `ARCH/48-EXPERIENCE-SURFACES.md:26` | `ui/src/components/chat/chat-composer.tsx:473,573-609,760-769,776-786` | "The attach menu offers the kinds the Workbench can display plus a create group; a refused kind names the reason and attaches nothing." + "Attachments render as a list; each item opens, opens externally, copies its path, and removes." Today: one `useState` slot and one chip. | `TEST-UI-015` (pending) | `[ ]` |
| `TASK-UI-017` | Every renderer-initiated navigation routes through Guard, answers included (UXL-088, UXL-202) | `REQ-TRUST-001`; `ARCH/08-REQUIREMENTS.md:284` | `ARCH/12-TRUST.md` §1; final `ARCH/48-EXPERIENCE-SURFACES.md:41` | `ui/src/components/chat/message-bubble.tsx:136-145,927-944`; `ui/src/lib/store.ts:1053,1828` | "Every answer link uses the same Guard-mediated route as citations; a missing file renders inert with a reason." + "Every renderer-initiated navigation goes through `openInBrowser`; no anchor opens a webview directly." Citations already route; answer links do not. | `TEST-UI-017` (pending) | `[ ]` |
| `TASK-UI-018` | Per-tool result registry, a "status unavailable" state, the one-rail turn summary, error groups that never auto-collapse, expansion that survives streaming (UXL-090, UXL-091, UXL-092, UXL-114, UXL-115, UXL-207) | `REQ-UI-006`, `REQ-UI-003`; `ARCH/08-REQUIREMENTS.md:1732` | `AGENTCOWORK-UI.md` §4.3; final `ARCH/48-EXPERIENCE-SURFACES.md:41` | `ui/src/components/chat/tool-chip.tsx:19-136,189-213,498-507,523-530,139-148` | "A group containing an error stays open; an explicit user toggle still wins (UI-ACC-3)." + "Each result shape has a registered renderer; the type switch is not a ~700-line `if`." + "A tool call whose terminal event is missing renders 'status unavailable', never a false spinner (UI-03/UI-18)." + "A settled turn collapses to one line reading total + step count." + "A row the user expanded stays expanded while deltas stream; a test streams into a list with an expanded row." + "No determinate affordance renders without a measured value; an unknown is indeterminate-with-label or absent." Depends on `TASK-UI-037`. | `TEST-UI-018` (pending) | `[ ]` |
| `TASK-UI-021` | Toast action channel: OK / Copy / Undo, and more than one toast may stack (UXL-112, UXL-169) | `REQ-UI-013`; `ARCH/08-REQUIREMENTS.md:1795` | `AGENTCOWORK-UI.md` §1.2 (UI-05, UI-07); final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/lib/toast-bridge.tsx:11-30`; `ui/src/lib/use-toast.ts:11` (`TOAST_LIMIT = 1`) | "A reversible destructive action (archive, close) offers Undo in a toast and restores state; a test asserts restoration." + "Success/info toasts offer OK; error/warning toasts offer Copy; a reversible action offers Undo; more than one toast may stack." Transport is ledger §6 row 6.6 (no superseding `DEC`). A toast action keeps keyboard reach — never strip `tabindex`. | `TEST-UI-021` (pending) | `[ ]` |
| `TASK-UI-022` | Settings search gains a synonym index and highlight-and-scroll to the target control (UXL-117) | `REQ-UI-017`; `ARCH/08-REQUIREMENTS.md:1831` | `AGENTCOWORK-UI.md` §5.13; final `ARCH/48-EXPERIENCE-SURFACES.md:70` | `ui/src/components/panels/settings-panel.tsx:300-354` | "Search matches synonyms, shows a no-results state, and highlights + scrolls to the target control." The no-results state already exists at `:342-354` (ledger §7.1 #2 corrects a study claim); the residual is the static index and the target highlight. Depends on `TASK-UI-023`. | `TEST-UI-022` (pending) | `[ ]` |
| `TASK-UI-024` | Dirty-switch guard (Save / Discard / Cancel) + named, user-managed settings profiles (UXL-120, UXL-121) | `REQ-UI-015`; `ARCH/08-REQUIREMENTS.md:1813` | `AGENTCOWORK-UI.md` §5.13; `ARCH/12-TRUST.md` §6; final `ARCH/48-EXPERIENCE-SURFACES.md:70` | `ui/src/lib/settings.ts:242-253,302-308`; `ui/src/lib/ui-prefs.ts:1-12` | "Editing a settings form then switching sections prompts 3 ways, and a failed save cancels the switch." + "A user can create/rename/delete/switch named profiles with an unsaved badge; secrets stay vault refs." `chooseAfterMutation` is a rollback-on-failure guard today, not an unsaved-edit guard. | `TEST-UI-024` (pending) | `[ ]` Decision resolved by DEC-056, DM-040 and CTR-032; implementation pending. |
| `TASK-UI-025` | The shortcut catalogue *is* the dispatch chain, plus a recorder that rejects protected collisions (UXL-131, UXL-132, UXL-182) | `REQ-UI-012`, `REQ-UI-017`; `ARCH/08-REQUIREMENTS.md:1786` | `AGENTCOWORK-UI.md` §2.5 (G24); final `ARCH/48-EXPERIENCE-SURFACES.md:70` | `ui/src/components/shell/keyboard-shortcuts.tsx:12-60,93-274`; `ui/src/components/panels/settings-sections-extra.tsx` | "The displayed catalogue is generated from the same data the dispatcher uses; a recorder rejects protected collisions and conflicts." + "No chord exists in the catalogue without a handler, or a handler without a catalogue entry; a test fails on drift." + "The displayed shortcut list is generated from the dispatch data." Verify: diff the catalogue mechanically against the `if` chain. Cheapest after `TASK-UI-023`. | `TEST-UI-025` (pending) | `[ ]` |
| `TASK-UI-030` | First run: immediate usable chat or guided agent setup, with local and remote recovery branches (UXL-173, UXL-174) | `REQ-UXQ-004`; `ARCH/08-REQUIREMENTS.md:3039` | `ARCH/48-EXPERIENCE-SURFACES.md` §2 (DEC-055); final `ARCH/48-EXPERIENCE-SURFACES.md:11` | `ui/src/components/overlays/onboarding-modal.tsx:376-876`; `ui/src/components/shell/setup-gate.tsx:206-388` | "First run covers local setup, remote connection and unreachable-host recovery as distinct states with a back path." + "First run offers immediate chat or a short guided setup with one primary action; missing capabilities prompt setup at point of need." Remote connection is a target capability under DEC-055/REQ-RTENV-012; show its actual configured availability and a local recovery state. No survey gates first value. | `TEST-UI-030` (pending) | `[ ]` |
| `TASK-UI-032` | The undo-over-confirm carve-out is written into the owning doc (UXL-186) | `REQ-UI-013`; `ARCH/08-REQUIREMENTS.md:1795` | `AGENTCOWORK-UI.md` §1.2 (UI-04, UI-07); final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/components/chat/turn-checkpoint.tsx`; `ui/src/components/chat/mcq-interrupt-card.tsx:299-311` | "The doc states which effects are hard-to-undo/external and therefore confirm vs undo. Copy change." Type-to-confirm stays reserved for high-blast effects; a routine action is never slowed to match. | `TEST-UI-032` (pending) | `[ ]` |
| `TASK-UI-033` | A Guard-denied tool call renders neutral ink + lock, never red (UXL-194) | `REQ-UI-006`; `ARCH/08-REQUIREMENTS.md:1732` | `AGENTCOWORK-UI.md` §1.2 (UI-04); final `ARCH/48-EXPERIENCE-SURFACES.md:41` | `ui/src/components/chat/tool-chip.tsx`; `ui/src/components/chat/message-bubble.tsx` | "A Guard-denied call renders neutral ink + lock with the reason; red stays reserved for failures." Unverified row — check: grep a `denied` / `blocked` status in `tool-chip.tsx` and the `toolResult` handler. | `TEST-UI-033` (pending) | `[ ]` |
| `TASK-UI-038` | Reasoning: per-turn override memory plus a hide preference (UXL-094) | `REQ-UI-001`; `ARCH/08-REQUIREMENTS.md:1687` | `AGENTCOWORK-UI.md` §4.4; final `ARCH/48-EXPERIENCE-SURFACES.md:41` | `ui/src/components/chat/message-bubble.tsx:166-239`; `ui/src/lib/store.ts:125-135` | "Once a user opens or closes a turn's reasoning, the auto-toggles stop for that turn; a hide preference exists." Auto-open live / auto-collapse settled is keyed only on `settled` today; a raw variant, if ever surfaced, sits behind a named *Technical details* disclosure (UI-14). | `TEST-UI-038` (pending) | `[ ]` |
| `TASK-UI-040` | Workbench tab launcher and command search expose all supported views without flooding the default pane (UXL-054) | `REQ-UXQ-005`; `ARCH/08-REQUIREMENTS.md:3048` | `ARCH/48-EXPERIENCE-SURFACES.md` §5 (DEC-055/056); final `ARCH/48-EXPERIENCE-SURFACES.md:47` | `ui/src/components/shell/right-rail.tsx:220-243`; `ui/src/components/shell/activity-rail.tsx` | "A compact, keyboard-complete tab launcher opens any supported Workbench surface; active tabs persist and unsupported views explain the fallback." The old fixed six-slot requirement is superseded by DEC-055/056; 22 `VIEW_META` peers still need migration. Needs `TASK-ART-001` (W3) for artifact refs. | `TEST-UI-040` (pending) | `[ ]` |
| `TASK-UI-041` | "Library" is the promoted-artifact inventory; the model-weights tab is renamed **Local models** (UXL-144) | `REQ-UXQ-007`; `ARCH/08-REQUIREMENTS.md:3066` | `AGENTCOWORK-UI.md` §7; final `ARCH/48-EXPERIENCE-SURFACES.md:66` | `ui/src/components/panels/local-models-panel.tsx:97`; `ui/src/components/panels/local-models-panel.dom.test.tsx` | "'Library' means the promoted-artifact inventory; the model-weights surface is renamed **Local models**. Copy change; `OQ-UI-001`." | `TEST-UI-041` (pending) | `[ ]` |
| `TASK-TRUST-008` | Per-decision-kind approval card components; the `plan` branch is unreachable today (UXL-108) | `REQ-TRUST-002`; `ARCH/08-REQUIREMENTS.md:293` | `ARCH/12-TRUST.md` §5; `AGENTCOWORK-UI.md` §5.10 (G38); final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/lib/store.ts:211` (6-member kind union); `ui/src/components/chat/mcq-interrupt-card.tsx:284-318` (5 branches) | "Every decision kind has its own card branch and each is covered by a test." One primitive, per-kind surfaces (`DEC-021`); a kind is never bolted onto one overloaded card. Depends on `TASK-TRUST-006`. | `TEST-TRUST-008` (pending) | `[ ]` |
| `TASK-TRUST-010` | A policy-blocked provider / model renders disabled with its reason and who can change it (UXL-145) | `REQ-UI-013`; `ARCH/08-REQUIREMENTS.md:1795` | `AGENTCOWORK-UI.md` §1.2 (UI-04); final `ARCH/48-EXPERIENCE-SURFACES.md:70` | `ui/src/components/panels/settings-panel.tsx:100-190,362-379` | "An org-blocked provider/model renders disabled with the reason and who can change it — never silently absent (`UI-04`)." A blocked tab is never hidden. | `TEST-TRUST-010` (pending) | `[ ]` |
| `TASK-WORK-005` | The Runs viewport lists work-tree items and links to the one run surface (UXL-057) | `REQ-WORK-008`; `ARCH/08-REQUIREMENTS.md:264` | `ARCH/11-WORK.md` §8; `AGENTCOWORK-UI.md` §6.1; final `ARCH/48-EXPERIENCE-SURFACES.md:80` | `ui/src/components/views/run-view.tsx:53-70` | "The `run` viewport lists work-tree items and links to the one run surface." The viewport reads only the active session today. | `TEST-WORK-005` (pending) | `[ ]` |
| `TASK-WORK-007` | Retention surfaces: eligible counts, honest run-now gating, and a declared session-retention policy (UXL-129, UXL-130) | `REQ-MEM-017`, `REQ-EVENTS-008`, `REQ-ART-009`; `ARCH/08-REQUIREMENTS.md:805` | `ARCH/17-MEMORY.md`; `ARCH/29-ARTIFACTS.md`; `ARCH/30-EVENTS.md` §4; final `ARCH/48-EXPERIENCE-SURFACES.md:80` | `ui/src/components/panels/settings-sections-extra.tsx:21-35`; `ui/src/lib/store.ts:893-945` | "Cleanup shows the eligible count, disables 'run now' while running, and reports completed vs failed separately." + "Session retention is a declared, auditable policy with an archive-vs-delete choice." Needs `TASK-MEM-004` (W1) and `TASK-ART-002` (W3). | `TEST-WORK-007` (pending) | `[ ]` |
| `TASK-WORK-008` | Transport health is its own projection, separate from work status (UXL-201) | `REQ-WORK-001`, `REQ-UI-003`; `ARCH/08-REQUIREMENTS.md:201` | `ARCH/11-WORK.md` §2; final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/lib/store.ts:68-78` (`reconnecting` sits inside `SessionStatus`) | "Transport health is a separate projection from work status, so a dropped channel never renders as a failed turn." Split the enum before `TASK-UI-027` reads phases from it; the degraded projection itself is `TASK-EVENTS-003`. | `TEST-WORK-008` (pending) | `[ ]` |
| `TASK-CHAN-005` | Approval events render, and a dropped channel replays from the last ack (UXL-158, UXL-159) | `REQ-CHAN-007`, `REQ-CHAN-013`, `REQ-EVENTS-007`; `ARCH/08-REQUIREMENTS.md:2684` | `ARCH/32-CHANNELS.md` §7, §8; `ARCH/30-EVENTS.md` §3; final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/lib/bridge.ts:481-508`; `ui/src/components/chat/chat-panel.tsx:427-436` | "`approval.granted` and `approval.expired` each have a render (receipt; expiry surfaced as deny)." + "A dropped channel replays the filtered stream from the last ack with no orphaned UI state." `ARCH/32-CHANNELS.md` §8 `EDGE-077`; one of the three approval shapes is mapped to a render today. | `TEST-CHAN-005` (pending) | `[ ]` |
| `TASK-CHAN-006` | An app-scheme / deep link is confirmed with its exact URL before the OS opens it (UXL-203) | `REQ-TRUST-001`; `ARCH/08-REQUIREMENTS.md:284` | `ARCH/32-CHANNELS.md` §3; `ARCH/12-TRUST.md` §1; final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `src-tauri/src/`; `ui/src/components/chat/message-bubble.tsx` | "An app-scheme link in a transcript is confirmed with its exact URL before the OS opens it; cancel is the default." Unverified row — check: grep `src-tauri/src/` for `deep-link`, `open-url`, `scheme`. | `TEST-CHAN-006` (pending) | `[ ]` |

#### W5-P2 — 8 tasks · 10 ledger rows (all P2)

| ID | Task | REQ | Doc | Touched paths (current tree) | Acceptance (ledger) | Test | Status |
|---|---|---|---|---|---|---|---|
| `TASK-UI-007` | Inline session rename in the rail (UXL-011) | `REQ-UI-012`; `ARCH/08-REQUIREMENTS.md:1786` | `AGENTCOWORK-UI.md` §9.1; final `ARCH/48-EXPERIENCE-SURFACES.md:11` | `ui/src/components/shell/left-sidebar.tsx:239-274`; `ui/src/components/chat/chat-panel.tsx:228` | "Rename is an inline field with Enter to commit, Escape to cancel, and a focus-visible ring." Rename is `window.prompt` in the chat header today; the rail has no rename affordance. Reuses the dialog from `TASK-UI-010` for its confirm half. | `TEST-UI-007` (pending) | `[ ]` |
| `TASK-UI-016` | Composer input-history recall, and Escape-twice-to-stop behind an armed prompt (UXL-082, UXL-083) | `REQ-UI-012`; `ARCH/08-REQUIREMENTS.md:1786` | `AGENTCOWORK-UI.md` §1.2 (UI-12); final `ARCH/48-EXPERIENCE-SURFACES.md:26` | `ui/src/components/chat/chat-composer.tsx`; `ui/src/components/chat/chat-panel.tsx:492-506` | "ArrowUp on an empty composer recalls prior prompts; editing exits recall and stash-restore returns the typed text." + "Escape arms a stop prompt, disarms on idle, and never stops on the first press." Cheap once `TASK-UI-005` lands. | `TEST-UI-016` (pending) | `[ ]` |
| `TASK-UI-026` | One reveal on boot at CLS 0 under reduced motion, and a settings nav usable at the 800 px window minimum (UXL-133, UXL-146) | `REQ-UI-014`, `REQ-UI-012`; `ARCH/08-REQUIREMENTS.md:1804` | `ARCH/48-EXPERIENCE-SURFACES.md` §7; `ARCH/38-EXPERIENCE-QUALITY.md` §Progressive interaction; final `ARCH/48-EXPERIENCE-SURFACES.md:70` | `ui/src/globals.css:772-781`; `ui/src/components/panels/settings-panel.tsx:100-190,362-379`; `ui/src/tauri.conf.json:19-20` | "One reveal on boot, no layout shift, all animation disabled under reduced motion." + "The grouped Settings navigation stays usable at the 800 px window minimum; the old 44-section wall is superseded by DEC-055." The settings panel has no width breakpoint today. Unverified row — check: an 800×600 screenshot pass over Settings. | `TEST-UI-026` (pending) | `[ ]` |
| `TASK-UI-028` | The transcript survives a center-screen switch mid-turn (UXL-160) | `REQ-WORK-002`; `ARCH/08-REQUIREMENTS.md:210` | `AGENTCOWORK-UI.md` §4.1; final `ARCH/48-EXPERIENCE-SURFACES.md:41` | `ui/src/lib/store.ts` (Zustand-held transcript) | "Switching center screens mid-turn does not drop the in-flight transcript. No cache migration inside W3 (ledger §6.5)." Explicit persist/hydrate now; a query-cache migration is out of scope for this wave. | `TEST-UI-028` (pending) | `[ ]` |
| `TASK-UI-031` | `Cmd/Ctrl+1–9` selects a visible session and is suppressed while a dialog owns focus (UXL-184) | `REQ-UI-012`; `ARCH/08-REQUIREMENTS.md:1786` | `AGENTCOWORK-UI.md` §2.5; final `ARCH/48-EXPERIENCE-SURFACES.md:11` | `ui/src/components/shell/keyboard-shortcuts.tsx:22,93-274` | "Cmd/Ctrl+1–9 selects a visible session, is suppressed while a dialog owns focus, and cancels until modifier release." The chord is in the catalogue with no held-modifier overlay. Unverified row — confirm the owner-suppression behaviour in the dispatcher. | `TEST-UI-031` (pending) | `[ ]` |
| `TASK-VERIFY-003` | The support bundle is enumerated, not just scrubbed (UXL-135) | `REQ-VERIFY-004`; `ARCH/08-REQUIREMENTS.md:2785` | `ARCH/34-EFFECT-VERIFICATION.md`; final `ARCH/48-EXPERIENCE-SURFACES.md:86` | `ui/src/components/panels/settings-sections-extra.tsx:671-684` | "A one-click bundle excludes secrets and lists exactly what it includes before the user shares it." The UI asserts structural scrubbing but never lists contents. | `TEST-VERIFY-003` (pending) | `[ ]` |
| `TASK-RTENV-004` | Sandbox setup progress dialog: real phases, Retry kept on error (UXL-180) | `REQ-RTENV-011`, `REQ-RTENV-002`; `ARCH/08-REQUIREMENTS.md:1106` | `ARCH/19-RUNTIME-ENVIRONMENTS.md`; final `ARCH/48-EXPERIENCE-SURFACES.md:70` | `ui/src/components/panels/settings-sections-extra.tsx:644-710` | "A first sandbox setup shows real phases; an error keeps the dialog with Retry + a native fallback." UI-only once the runtime reports phases (`REQ-RTENV-010`); posture is read-only diagnostics today. | `TEST-RTENV-004` (pending) | `[ ]` |
| `TASK-AGX-015` | Retired historical proposal for rail-level fork lineage (UXL-058) | `REQ-WORK-009` now belongs to `TASK-AGENT-002`; `ARCH/08-REQUIREMENTS.md:273` | `ARCH/11-WORK.md`; `AGENTCOWORK-UI.md` §2.1; final `ARCH/48-EXPERIENCE-SURFACES.md:11` | Historical source locations: `ui/src/lib/store.ts:272-310`; `ui/src/components/chat/chat-panel.tsx:671,872` | This row is a tombstone only. DEC-056 assigns engine-neutral lineage and navigation to `TASK-AGENT-002` in W6. | `TEST-AGX-015` (retired) | retired; do not implement or reuse this ID. |

#### W5 — not carried, and why

- **100 `present` rows and 3 `N/A` rows are not work.** They are the audit's evidence that a behaviour already ships (`UXL-165` focus rings, `UXL-189` Escape + focus restore, `UXL-208` tabular telemetry, `UXL-198` empty-transcript actions, `UXL-191` / `UXL-192` the shared state primitives, `UXL-188` the DOM-test baseline, …) or that a frozen invariant refuses it (`UXL-022` no second renderer menu surface, `UXL-181` no first-run survey, `UXL-209` no screenshot retention claim). One of them is an open **confirmation**, not a build: `UXL-140` (skills / plugins / marketplace, `present`) — "An installed extension shows provenance and per-component enable state; risky components default **off** with a reason" (`ui/src/components/panels/skills-panel.tsx`; `ui/src/lib/store.ts:893-945`). The ledger proposed `TASK-SKILL-004` for it; under this wave's scope rule a `present` row cannot become a task, so it is recorded here and **no `TASK-SKILL-004` id was minted**.
- **Eleven study claims about our code were wrong** (ledger §7.1). Three resolved to `present` and are not rows at all (`UXL-150` the event self-routing test exists at `lib/chat-event-routing.test.ts:86`; `UXL-116` the nav has 44 sections, counted at `store.ts:893-945`; `UXL-111` committed turns are editable and forkable). Three were false `title`-only positives and were dropped at the ledger (`message-bubble.tsx:534-582`, `tool-chip.tsx:266-286`). One survives only as its corrected residual (`UXL-117`, the no-results state already exists at `settings-panel.tsx:342-354`). The missing error boundary is open as `TASK-UI-029` after the 2026-09-28 source check.
- **DEC-056 reconciliation (2026-09-28).** The 16 previously unregistered W5 requirement gaps now cite the accepted `REQ-UI-015…017`, `REQ-CTX-011`, `REQ-WORK-009` or applicable DEC-055 `REQ-UXQ-*` requirements. `DM-040` and `CTR-032` own settings profiles/scope. Five decision blocks are resolved in the architecture; their implementation stays `[ ]`. `TASK-AGX-015` is retired and replaced by `TASK-AGENT-002`; its ID is never reused.

### W6 — Mission and final experience (DEC-054/055/056; design accepted, implementation pending)

This wave is the implementation backlog for the amended architecture. The historical W0–W5 plan and completed code claims remain as recorded; no row below is marked done merely because its design is written. Dependency order: stabilize existing Work/events/trust/artifacts → bind external agents and scoped ecosystem → Mission dispatch/recovery → Workbench/Library and workflows → run `ARCH/49-TEST-CASES.md` on pinned builds. The release gate is evidence in `ARCH/42-EVIDENCE-MAP.md`, not a paper checklist.

| ID | Task | REQ | Owner / dependency | Test slot | Status |
|---|---|---|---|---|---|
| `TASK-MISSION-001` | Durable GoalContract, requirements, PlanNodes, plan versions and Work attempt links | `REQ-MISSION-001, REQ-MISSION-002, REQ-MISSION-003`; `ARCH/08-REQUIREMENTS.md:2877` | `ARCH/35-MISSION.md:20` (§Canonical records), `:38` (§Controller), `:46` (§Storage); Work/events | `TEST-MISSION-001` (pending) | `[ ]` |
| `TASK-MISSION-002` | Outcome/evidence evaluator, requirement impact/invalidation and honest stop decision | `REQ-MISSION-004, REQ-MISSION-006`; `ARCH/08-REQUIREMENTS.md:2904` | `ARCH/36-OUTCOME-AND-RECOVERY.md:5` (§Two verification levels), `:11` (§Evidence); `ARCH/34-EFFECT-VERIFICATION.md` | `TEST-MISSION-002` (pending) | `[ ]` |
| `TASK-MISSION-003` | Environment fingerprint, restart reconciliation, bounded retry/no-progress, branch-local human wait | `REQ-MISSION-005, REQ-MISSION-007`; `ARCH/08-REQUIREMENTS.md:2913` | `ARCH/36-OUTCOME-AND-RECOVERY.md:15` (§Recovery), `:27` (§Resume); Work/runtime | `TEST-MISSION-003` (pending) | `[ ]` |
| `TASK-ECO-001` | Read-only staged agent discovery, capability negotiation and safe session overlays | `REQ-ECO-001, REQ-ECO-002, REQ-ECO-003`; `ARCH/08-REQUIREMENTS.md:2940` | `ARCH/46-ECOSYSTEM-ARCHITECTURE.md:27` (§Agent discovery); ACP/channels | `TEST-ECO-001` (pending) | `[ ]` |
| `TASK-ECO-002` | Native-vs-Core provenance, scoped MCP/skills/plugins and heterogeneous child handoff/integration | `REQ-ECO-004, REQ-ECO-005, REQ-ECO-006`; `ARCH/08-REQUIREMENTS.md:2967` | `ARCH/46-ECOSYSTEM-ARCHITECTURE.md:35` (§Extension scope), `:51` (§Teams); Trust/Mission | `TEST-ECO-002` (pending) | `[ ]` |
| `TASK-AGENT-002` | Engine-neutral session-fork lineage: parent/origin refs, event projection and two-way rail navigation without phantom Work | `REQ-WORK-009`; `ARCH/08-REQUIREMENTS.md:273` | `ARCH/11-WORK.md:76` (§Sessions), `ARCH/30-EVENTS.md:36` (§Stream vocabulary), `ARCH/48-EXPERIENCE-SURFACES.md:11` (§Navigation); replaces retired `TASK-AGX-015` | `TEST-AGENT-002` (pending) | `[ ]` |
| `TASK-CHAN-008` | Authenticated A2A remote-agent adapter with capability negotiation, opaque task/artifact exchange, cancellation and reconciliation | `REQ-CHAN-008, REQ-CHAN-013`; `ARCH/08-REQUIREMENTS.md:2693` | `ARCH/32-CHANNELS.md:59` (§A2A), `ARCH/46-ECOSYSTEM-ARCHITECTURE.md:27` (§Binding); Work/Trust | `TEST-CHAN-008` (pending) | `[ ]` |
| `TASK-CHAN-009` | Authenticated web/mobile Mission projection, steering, wait answers, notifications and device revocation | `REQ-CHAN-014`; `ARCH/08-REQUIREMENTS.md:2747` | `ARCH/32-CHANNELS.md:22` (§Surface map), `:67` (§Approvals); `ARCH/48-EXPERIENCE-SURFACES.md:5` (§Experience); requires `TASK-RTENV-005` or healthy local service | `TEST-CHAN-009` (pending) | `[ ]` |
| `TASK-LEARN-001` | Capture successful procedure, sanitize and evaluate candidate skill/workflow; external n8n/Activepieces provider integration | `REQ-LEARN-001, REQ-LEARN-002`; `ARCH/08-REQUIREMENTS.md:2994` | `ARCH/37-WORKFLOW-SKILL-LIFECYCLE.md:9` (§Capture), `:15` (§Providers); workflow/skill registry | `TEST-LEARN-001` (pending) | `[ ]` |
| `TASK-UXQ-001` | Composer `+`/`@`/`/`, agent and conditional model picker, access chip, voice/send/steer/queue and durable drafts | `REQ-UXQ-003`; `ARCH/08-REQUIREMENTS.md:3030` | `ARCH/48-EXPERIENCE-SURFACES.md:26` (§Composer); agent binding/Trust | `TEST-UXQ-001` (pending) | `[ ]` |
| `TASK-UXQ-002` | Outcome-based left navigation, first-run journey, searchable Settings and truthful agent/provider sign-in panel | `REQ-UXQ-001, REQ-UXQ-004`; `ARCH/08-REQUIREMENTS.md:3012` | `ARCH/48-EXPERIENCE-SURFACES.md:11` (§Navigation), `:70` (§Settings); `ARCH/46-ECOSYSTEM-ARCHITECTURE.md:27` | `TEST-UXQ-002` (pending) | `[ ]` |
| `TASK-UXQ-003` | Resizable Workbench with browser/takeover, file tree/worktree, PDF/Office/code/media tabs, typed selection, edit conflict/fallback | `REQ-UXQ-005`; `ARCH/08-REQUIREMENTS.md:3048` | `ARCH/48-EXPERIENCE-SURFACES.md:47` (§Right Workbench); files/browser/computer/office/artifacts | `TEST-UXQ-003` (pending) | `[ ]` |
| `TASK-UXQ-004` | Safe complete chat renderer: math, images, tables, citations, diagrams, interactive artifact isolation and accessible streaming | `REQ-UXQ-006`; `ARCH/08-REQUIREMENTS.md:3057` | `ARCH/48-EXPERIENCE-SURFACES.md:41` (§Conversation and results); UI renderer | `TEST-UXQ-004` (pending) | `[ ]` |
| `TASK-UXQ-005` | Library source filters, metadata, indexed exact-version retrieval, permissions, dependencies and stale propagation | `REQ-UXQ-007`; `ARCH/08-REQUIREMENTS.md:3066` | `ARCH/48-EXPERIENCE-SURFACES.md:66` (§Library), `ARCH/29-ARTIFACTS.md:53` (§Library); files/search/context/world | `TEST-UXQ-005` (pending) | `[ ]` |
| `TASK-UXQ-006` | Effective per-agent/session/workspace/Mission extension scope, native inventory and collision/revocation UI | `REQ-UXQ-008`; `ARCH/08-REQUIREMENTS.md:3075` | `ARCH/46-ECOSYSTEM-ARCHITECTURE.md:35` (§Extension scope), `ARCH/48-EXPERIENCE-SURFACES.md:70` (§Settings); Trust | `TEST-UXQ-006` (pending) | `[ ]` |
| `TASK-UXQ-007` | Agents/subagents panel, bounded heterogeneous team dispatch, result integration, child provenance and branch-local blockers | `REQ-UXQ-009`; `ARCH/08-REQUIREMENTS.md:3084` | `ARCH/15-AGENT-PLANE.md:63` (§7), `ARCH/46-ECOSYSTEM-ARCHITECTURE.md:51` (§Teams), `ARCH/48-EXPERIENCE-SURFACES.md:80` (§Teams) | `TEST-UXQ-007` (pending) | `[ ]` |
| `TASK-UXQ-008` | Workflow-to-skill and recorded-procedure lifecycle with review, versioning and trigger/run UI | `REQ-UXQ-010`; `ARCH/08-REQUIREMENTS.md:3093` | `ARCH/37-WORKFLOW-SKILL-LIFECYCLE.md:9` (§Capture), `ARCH/48-EXPERIENCE-SURFACES.md:80` (§Automations) | `TEST-UXQ-008` (pending) | `[ ]` |
| `TASK-RTENV-005` | Configured remote/cloud executor with authenticated Work handoff, fenced trigger-owner transfer and occurrence cursor, scoped credentials, background scheduling, cross-device steering and local-offline truth | `REQ-RTENV-012, REQ-MISSION-005, REQ-WF-004`; `ARCH/08-REQUIREMENTS.md:1115` | `ARCH/19-RUNTIME-ENVIRONMENTS.md:71` (§Remote/cloud execution handoff), `ARCH/20-WORKFLOW.md:41` (§Durability), `ARCH/35-MISSION.md:54` (§Recovery), `ARCH/32-CHANNELS.md:59` (§Remote agents) | `TEST-RTENV-005` (pending) | `[ ]` |
| `TASK-UXQ-009` | Run the model-agnostic product ladder and matched competitor study; publish raw outcomes, failure cases and accessibility results | `REQ-UXQ-002`; `ARCH/08-REQUIREMENTS.md:3021` | `ARCH/49-TEST-CASES.md:5` (§Measurement), `ARCH/47-MARKET-AND-BENCHMARKS.md:37` (§Comparison), `ARCH/42-EVIDENCE-MAP.md`; requires implementation above | `TEST-UXQ-009` (pending) | `[ ]` |
| `TASK-PROD-005` | Re-home `scripts/check-doc-sync.mjs` from archived `DESKTOP-APP-SPEC.md` to live `AGENTCOWORK-SPEC.md`/`ARCH` ownership and update its expected capability chain | Infrastructure gate; no user-facing REQ (the existing `REQ-PROD-005` is token discipline); `ARCH/08-REQUIREMENTS.md:116` | `ARCH/00-INDEX.md:35` (§Authority chain); `scripts/check-doc-sync.mjs` | `TEST-PROD-005` (pending) | `[ ]` |
| `TASK-PROD-006` | Audit dormant/superseded paths, prove zero production callers, retire or re-home them without deleting user work | `REQ-PROD-001, REQ-PROD-004`; `ARCH/08-REQUIREMENTS.md:80` | `ARCH/39-ARCHITECTURE-DELIVERY.md:5` (§Current-state delta); `core/src/chat.rs`, unused coordinator modules, `packages/core-ai`, duplicate sidecar MCP client | `TEST-PROD-006` (pending) | `[ ]` |


## History

> **Non-authoritative.** The v0-era ledger was reworked in P7 because its phase structure (Stage 0 · HARDENING · PHASE 0–12 · P13–P71 queues) and its copied v0 spec contradict the v1 document set: nothing below is a contract, and the v1 authorities win. `TODO.md` was exempt from the v1 archive (`ARCH/00-INDEX.md` §10) precisely so this record could be carried forward, not rewritten.
>
> **What was preserved where:**
> - every still-open row (375 open + 4 partial at 2026-09-26) is kept below with its bold title and status marker, grouped under its original section with done/open counts;
> - the census blockquotes (the dated, point-in-time landing log) are kept verbatim in the first collapse;
> - the full pre-v1 text — the normative v0 spec copy, the verification policy, and all closed-row evidence — is preserved in git history: `git show 3eeb8f0:TODO.md`;
> - still-true work was folded into the waves above: P50 honesty rows → `TASK-UI-*` / `TASK-PROD-003`; KERNEL GATE / STAGE 0 / HARDENING / H4 → closed or covered by W0–W1; PHASE 0–12 → W1–W3; P13–P27 / P36–P42 / P51–P52 steal queues → evidence only, consumed by the module docs; P53–P60 → W3/W4; P62 → W0; P63–P68 → `TASK-CHAN-*` / `TASK-RTENV-*` / `TASK-CUA-*`; P69 (ARCH thaw) → superseded by the v1 doc set; P70 → the cross-wave release gate; P71 (external-agent consolidation) → W4.

<details>
<summary>Census history — carried verbatim from the v0 ledger (archived, point-in-time; not current state)</summary>

> **Documentation ownership:** `DESKTOP-APP-SPEC.md` is the **product contract only** (behavior, schemas, UI layouts, invariants — not a build plan). This file is **detailed implementation**: what to build/remove, from which files, how, pointing at spec/ARCH. A **working copy of the architecture** is below so you can develop without flipping the spec for every turn. If the copy and the spec disagree, **the spec wins** — fix the copy. Historical notes: `SPEC-CHANGELOG.md`. Boundaries: `ARCH/`. Research: `RESEARCH/`.

> **Generated:** 2026-09-16 · **Architecture:** ARCH/CORE.md (root authority — see ARCH/ADR/0003) + ARCH/00–15 + the subsystem contracts + DIAGRAMS.md (**16 and 17 are archived** under `ARCH/archive/` — `ARCH/17` was the Native agent plane, whose “frozen” status and “orchestrator owned by us” claim are lifted by ADR/0003 and whose live content is `ARCH/AGENT.md` + `ARCH/EXTERNAL-AGENTS.md`; moved 2026-09-22 by `P71.5a`), current doc revision v4.06; its nine rows **B10/B11/C14/C15/F16/I14–I17** added in v3.76) · **History:** `SPEC-CHANGELOG.md`
> **Rule:** Mark `[DONE]` only after implementation + test pass **and** a live consumer (coordinator, Tauri command the UI actually calls, or a crate-scoped task whose checkbox is crate-only). Mock data, unused wrappers, and crate tests with no runtime path are `[NOT DONE]`.
> **Scope:** Complete product — **8 Full-Stack Modules with 166 submodules/functions** (spec §0 index; **v3.76** adds the nine native-agent-plane rows **B10/B11/C14/C15/F16/I14–I17**), 34 algorithms (Algorithm Index #34 = FSRS), 14 build phases (**Stage 0** + P0–P12) + UI implementation (P11.5).
> **Live checkbox count (2026-09-24 — P71 external-agent consolidation, the delivery half of `ADR/0005`; earlier landings 2026-09-20 — ARCH/CORE architecture thaw (P69) + v1 release programme (P70), and 2026-09-16 — Native Agent Plane First-Class Tools, Context Providers, OS Sandboxing, Multi-Agent Swarm Fleet, Avoidance Store, Calendar Schema v8, Context Mode, P66.3 picker, and P66.5 theme):** the shell integration + provenance plane landed — `crates/agentcowork-core/src/shell_integration.rs` (OSC 633 parser + nonce-verified command tracker), integration scripts for bash/zsh/fish/pwsh injected at spawn, `TerminalOrigin::{Human,Agent,Task}` + `terminal_run` (agent/task commands run in the same PTY host, audited, rendered as labelled read-only tabs), `shell-view.tsx` rewrite (provenance chips, cwd header, exit decorations, find, links, history picker, `computeXtermTheme` from live CSS tokens), `@terminal` composer context, Explorer "open terminal here", and the legacy piped `shell_cmds.rs` + `lib/shell.ts` **deleted** with the IDE workbench unified on the one PTY plane. Native agent plane first-class tools (`ask`, `plan`, `todo`, `subagent`), live `@Codebase`/`@Docs`/`@URL`/`@file` dynamic context resolution below `CACHE_BOUNDARY`, OS process sandbox backends (Windows Job Objects / Restricted Tokens, macOS Seatbelt, Linux bwrap), multi-agent swarm fleet isolation (`git_queue.rs` write mutex serialization + stale lock purge >5s, `worktrees.rs` task directories with `WorktreeCap` and 3-file blackboards, `undo_worktree`/`restore_branch`, `governor.rs` CPU/RAM capacity bounds), cognitive failure avoidance store (`avoid.rs` negative constraints), calendar & automations SQLCipher schema v8 + 6 Tauri IPC commands (`calendar_cmds.rs`) and bridge (`lib/calendar.ts`), context-mode 50KB truncation ceiling with query hints, and session capability loadout (`shared:fleet`, `shared:calendar`, `filterToolsByCapabilityLoadout`) landed. Two-pane full-screen agent picker (`agent-model-picker.tsx`), custom agent binary import/verify lifecycle (`acp_agent_import`, `acp_agent_verify`), and cool-blue semantic theme with user-selectable accent system (`globals.css`, `theme-provider.tsx`, `settings-sections.tsx`) landed. OPEN: P68.7 (Windows ConPTY acceptance — never run) and P68.8 (**splits only** — ring-buffer replay is landed *and wired*: 256 KiB per-session ring + cursor replay + honest truncation labels). The coordinator `script.run` → PTY seam **landed 2026-09-17** (loop-pinned `script.run` mounted every turn, read-only `terminal/*` observer arm, and the plane's state injected below `CACHE_BOUNDARY`). Also landed: the A8 local OpenAI-compatible server now forwards `tools`/`tool_choice`/`parallel_tool_calls`, returns native `tool_calls`, and streams incrementally per upstream chunk. Also landed (A11 capability-probe write-back): live probes persist a durable `ProviderObservation` (`crates/agentcowork-catalog/src/observations.rs`, `<data_dir>/provider-observations.json`) and every registry that feeds a claim replays them, so `capabilities_verified_at`/`verified_report` and routing health reflect what a probe actually observed; a failed probe never verifies, a MetadataOnly probe confirms no hard capability, and a report that confirmed nothing can no longer read as "fully verified". Also landed (A11 vault-mediated probing): keyed providers are observed at boot and on vault unlock/setup without the plaintext key leaving the vault — `Broker::probe_models` resolves the credential from the ring, GETs the provider's own `/models` endpoint (https or loopback only, no URL parameter), and never moves key health/budget because a metadata call is not a turn; a broker error yields no observation rather than a fabricated failed one. **2026-09-21 — P69 consolidation wave landed:** 33 rows carry `[IMPLEMENTED — unverified]` (C-series defect repairs C1–C4/C7–C12 · D-series ownership · B2 `AgentBinding` · the CI invariant gates). **2026-09-21 — P71 implementation wave (external agents are the v1 engines):** P71.1 (the `delegate.*` façade), P71.1b, P71.3a (subagents as child Work), P71.3b (swarm strategy), P71.3c (automation Work factory) and P71.3d (scheduler trigger plane) now carry `[IMPLEMENTED — unverified]` — 39 marker rows total; all still counted open by design; see `SPEC-CHANGELOG.md` 2026-09-21. **2026-09-22 — first full-suite verification pass over the P69/P71 waves (0 flips):** every suite in the repository was run for the first time since the external-agent consolidation began, and it is **green** — `cargo test --workspace` **2691 passed / 0 failed / 25 ignored** · `bun test` ui **383/0** · `bun test` coordinator **238/0** · `src-tauri` `cargo check` clean · `tsc --noEmit` clean (ui + coordinator) · `check-doc-sync` · `check-arch-invariants` · `ipc-parity` 0 broken (350 registered) · `gen-codebase-map --check` · `scripts/e2e/security-gate.mjs` **PASS**. The pass caught and repaired **five real defects in already-`[IMPLEMENTED]` code** (an assertion collapsed to `x !== x` by the built-in-id rename; `isAcpAgent` still accepting the retired `agentcowork-native` spelling; `upsert_bundle` returning the *previous* value so "created" and "refused" were indistinguishable; `chat.rs`'s `unwrap_or("inbuilt")` default defeating the fail-closed harness gate; and three MCP tests still asserting the internal 51-tool catalogue instead of the 19-façade shared plane an external client is actually served) plus one load-flaky vault timing test made exact — see `SPEC-CHANGELOG.md` **v3.96**. **Row-level acceptance is NOT claimed:** the 39 `[IMPLEMENTED — unverified]` markers keep their labels and stay counted open, because a green suite is evidence about the tree, not proof that each row's own criteria were demonstrated. **1679 total = 1304 done + 375 open**
>
> **Census history — ARCHIVED, point-in-time evidence (not current state).** From here down to the `> **Source reuse:**` line is the accumulated, dated record of past landings, newest first. The **only** current delivery count in this document is the "Live checkbox count" line above — the single number `scripts/check-doc-sync.mjs` verifies against the file; the current capability identity is `capabilities.yaml` (**166** rows). Per-release detail for the same dates is also in `SPEC-CHANGELOG.md`, which carries the full record. **Engine decision (2026-09-21):** [`ARCH/ADR/0005`](ARCH/ADR/0005-external-agents-are-the-v1-engines.md) makes external agents the only v1 engines and defers the built-in engine to post-v1, and [`ARCH/ADR/0006`](ARCH/ADR/0006-session-kinds.md) defines session kinds — so any archived entry below that describes a built-in engine as the default, or a session as always behind a Chat, is **historical, not current**.
>
> **Archived — 2026-09-14 — P49 V1-local Work Gateway wiring landed: 12 flips (P49.1/.2/.3/.4/.7/.8/.9/.13/.14/.15/.17/.18) — canonical addressing + `work:` locator, ExecutionNode pair/verify/bind/unbind/migrate, RunAuthority lease/fence with a monotonic fence counter + renew/recover, `CapabilityBroker` (opaque run-scoped handles, never a secret) + resolver `choose_best`/`choose_fallback`/`explain_choice`, client handshake (desktop vs restricted) + detach, ReviewQueue outcomes (approved/rejected/revision_requested leaving the Needs-Me inbox), steering queue/interrupt/checkpoint, `RuntimeManifest` freeze + restore-intersect (fail-closed narrow, unknown label → strictest), `AttachmentRef` scope+consumer enforcement — each with core methods + RPC arms + Tauri commands + tests. Also P57.4 foreground escalation (EscalationRequest + ForegroundSnapshot + `act_escalating` gesture gate + X11 `_NET_ACTIVE_WINDOW` snapshot/restore + Windows `Get`/`SetForegroundWindow`) and P57.5 Session 0/TCC honesty (capability fields + probes + readiness sentences) landed Rust-side; P57.6 Windows.Graphics.Capture landed (`platform/wgc.rs`, cross-compile + clippy clean for x86_64-pc-windows-msvc; runtime evidence pending a Windows runner) with `see_occluded_wgc` now a real probe, and P57.4's in-UI Guard-2 escalation card landed (`desktopEscalation` + `desktopActEscalating` + the card; **P57.4 now DONE** — 1 flip); P57.7 AT-SPI/AX remain open and are reported honestly, not faked. Census 157. Note: the counter counts `- [x]` + `- [ ]` only, so the 12 flips moved these rows from the `[~]` (uncounted) set into `done`; `open` is unchanged):** **1382 total = 1197 done + 185 open**
> **Archived — 2026-09-13 — P62 guard network-destination floor + protected agent-config surfaces landed: 2 done / 1 open / 1 partial, all Rust-side; P61 stays 11 done / 1 open:** **1370 total = 1184 done + 186 open**
> **Archived census log (2026-08-23 → 2026-09-13; older landings folded in below, "prior:" chain) — 2026-09-13 P61 first-five-minutes + approval-relationship UX landed (11 done / 1 open, all UI-side) plus two P32 honesty corrections:** **1367 total = 1182 done + 185 open** (2026-09-13 re-verified — P50.3 cross-layer wiring landed: P50.3.1 IPC parity inventory generator (0 broken), P50.3.2 twin task-contract tests (caught + fixed a real DeliveryState wire desync), P50.3.3 durable scheduler + task-ledger persistence with restart reconciliation, P50.3.4 remote MCP tools/call through the Guard-2 ticket + audit receipt, P50.3.5 MCP attach consent enforced in Rust + persisted attach state, P50.3.9 ACP governance truth in the picker, P50.3.10 session-switch lifecycle tests):** **1355 total = 1171 done + 184 open** (2026-09-12 P57.4/P57.7 background click delivery — **0 flips:** a coordinate click no longer has to move the real pointer. Windows: `WinUia::invoke_at` does `IUIAutomation::ElementFromPoint` → `InvokePattern::Invoke`, guarded by a PID check so an *occluding* window cannot answer for a point inside the target; when nothing there is invokable it falls back to `WM_LBUTTONDOWN`/`WM_LBUTTONUP` posted to the deepest child under the point via `ChildWindowFromPointEx` (search never leaves the requested window). X11: a synthetic `ButtonPress`/`ButtonRelease` pair addressed to the deepest child under the point (`send_event` with `propagate`), never XTEST. macOS: System Events has no message-level or AX-by-point primitive, so coordinate click/scroll/drag **refuse** under Background with an actionable sentence instead of warping the cursor (a named-element `click \"…\" of window 1` is AXPress and remains the background path). Scroll/drag refuse under Background on every backend (both are pointer motion by definition), and the same act now translates `ActKind::Click`'s **window-relative** coordinates to screen space on Windows (the pre-existing `send_spot` treated them as absolute). New `Capabilities::background_input` records the fact, and `readiness::derive` appends the limitation to the chip's sentence when the host has no non-moving path. **Live X evidence:** `live_background_click_leaves_the_pointer_alone` moved the pointer to the window origin, delivered a Background click at the OCR-located `GO` button, and asserted the pointer never moved — Tk ignores synthetic events, which is exactly why the honest failure is the verify cascade, not the click call (**P57.4 PARTIAL** — escalation card + foreground snapshot/restore still open; **P57.7 PARTIAL** — AT-SPI `Action`-invoke and an AX-by-point FFI layer remain). Also fixed while compiling the macOS backend for the first time ever (`cargo test` now compiles all three backends): **two real pre-existing errors** — a missing `image::GenericImageView` import and a `&str` passed to `DesktopError::Platform(String)`. Computeruse **56** tests + 3 live green, Windows cross-compile 0 errors/0 warnings, 255 UI / 30 + 2 src-tauri green, `tsc` + cargo fmt/check clean, ipc-parity 0 broken, doc-sync green) **prior:** **1355 total = 1171 done + 184 open** (2026-09-12 P57.1/P57.3/P57.2 path-launch + background contract — **3 flips:** **P57.1** `ActKind::LaunchApp { path, app }` carries the canonical path and each backend executes it (new `crates/agentcowork-desktop/src/launch.rs`: `resolve_target` canonicalizes or does a `PATH` lookup, `validate` refuses a relative path before any policy/platform work; Linux direct exec — the `sh -c \"command -v\"` string is gone — Windows `ShellExecuteExW` + `SW_SHOWNOACTIVATE` under Background / `SW_SHOWNORMAL` under Foreground, macOS `open -g` with `open -a <name>` fallback), plus H5's child contract (detached stdio + name-filtered env scrub that removes provider keys/tokens/passwords/`AGENTCOWORK_*` while `PATH`/`HOME`/`DISPLAY`/`WAYLAND_DISPLAY`/`DBUS_SESSION_BUS_ADDRESS`/`XDG_*` survive); **P57.3** the background contract is enforced in every backend, not just the engine (Linux refuses `stack_mode ABOVE` + `set_input_focus`, Windows refuses `SetForegroundWindow` + `ShowWindow(SW_RESTORE)`, macOS refuses `activate`, all under Background), and **P57.2** whose launch subject is now the program itself so an allow-listed path launches without a per-session toggle while risky classes stay Guard-2; `desktop_act` gained a `launch` kind. **Live evidence on a real X server (Xvfb + matchbox, `--ignored --test-threads=1`):** `live_x11_list_capture_ocr_act_verify` asserts the Background refusal message then drives the same window on the Foreground path (activate → XTEST click → OCR reads `CLICKED`), and `live_path_launch_executes_the_file_without_a_shell_or_secrets` launches a probe script through the X11 backend asserting `$0` is the **canonical path** and that a planted `AGENTCOWORK_LAUNCH_TEST_API_KEY` never reached the child while `PATH` did; Windows cross-compile verified (`cargo check --target x86_64-pc-windows-msvc` → 0 errors/0 warnings, new `Win32_UI_Shell` + `Win32_System_Registry` features); computeruse **54** tests green, `cargo fmt` clean, src-tauri check green, ipc-parity 0 broken, doc-sync green; macOS compile-unverified here (no darwin std target installed) and coordinate clicks still move the real pointer (UIA/AX/AT-SPI invoke-first remains) — both stated rather than implied) **prior:** **1355 total = 1168 done + 187 open** (2026-09-12 P57.8 Computer-use policy surface — **1 flip:** the Settings → Computer use surface is now real and writes the live Guard-2 policy: a searchable installed-app inventory (new `crates/agentcowork-desktop/src/apps.rs` — Linux freedesktop `.desktop` parsing with field-code stripping + strict group/`Hidden` rules, macOS `.app` bundles, Windows Start Menu `.lnk`, deduped by resolved path, all-character search owned by the command over a session-cached scan; the ignored `live_inventory_reads_this_host` test read this machine — 6 roots → 40 entries, 1 correctly marked never-automatable), **Add by path** through the native picker with platform-correct options, allow-list **rows with Remove** persisted in `<data_dir>/desktop.json` and audited with honest `appliedLive`, the enforced **Background/foreground default**, and the H4 readiness chip; hard-deny is enforced twice; `desktop_policy_*` + `desktop_status.readiness`; +8 helper tests + 5 DOM tests → 255 UI green, computeruse **46** green, src-tauri 30 + registration_sync 2 green, `tsc` + cargo fmt/check clean, ipc-parity 0 broken (282 registered), doc-sync green) **prior:** **1355 total = 1167 done + 188 open** (2026-09-12 P55/P58 honesty wave — **7 flips:** **P55.11** MCP F6 handshake (`mcp_attach_commit` now runs `initialize` + `tools/list`, fails honestly and tears the child down when the handshake fails, persists the advertised `tool_names` instead of a count, reconciles them into the live `ToolService` catalog via `attach_external_server`, and binds a `LoopExternal` dispatcher so stdio `tools/call` runs through the native `tool/exec`+`tool/commit` Guard-2/audit path; `mcp_external_tools` reads that same registry and reports `agentVisible`; crate test calls a discovered tool on the same child and asserts a server error is not an empty success), **P55.7** E10 product path (new `browser_read_url` runs the tiered stack static→Lightpanda/Obscura→Chrome and returns the tier/source that served the read; Browse labels it; `browser_status.engine=\"chrome\"` stops implying a light engine runs the interactive session; SSRF + `file://` floors asserted on the product path), **P55.8** `searx.space` client (`searx_space.rs` parse/eligibility/latency/cache with live|fresh_cache|stale_cache + 9 tests; `search_config.rs` local-first owner in `<data_dir>/search.json`, opt-in public instances, dedupe, malformed⇒local-only + 4 tests; `G8Cascade::set_endpoints` re-routes a live session; `search_config`/`search_instances`/`search_instances_apply` + Settings → **Search**), **P55.9** Cloud env mock deleted (the `cloud` id now routes to the real H33 `node_attach` Sync surface), **P55.10** PPT slide-edit honesty, **P58.3** Computer-use nav + Experts→Built-in roles, **P58.4** shared slash ownership; 242 UI / 620 core / 55 mcp / 23 search green, `tsc` + cargo fmt/check clean, ipc-parity 0 broken, doc-sync green) **prior:** **1355 total = 1160 done + 195 open** (2026-09-12 P60 model ownership + ACP configOptions + live-registry merge — 4 flips; current Native/provider ownership and external agent-owned config are in the current spec) **prior:** **1352 total = 1156 done + 196 open** (2026-09-12 P58.7 flipped — the agent/model picker + status bar now read live models.dev catalog rows ... P50.3.1 IPC parity inventory generator (0 broken), P50.3.2 twin task-contract tests (caught + fixed a real DeliveryState wire desync), P50.3.3 durable scheduler + task-ledger persistence with restart reconciliation, P50.3.4 remote MCP tools/call through the Guard-2 ticket + audit receipt, P50.3.5 MCP attach consent enforced in Rust + persisted attach state, P50.3.9 ACP governance truth in the picker, P50.3.10 session-switch lifecycle tests):** **1355 total = 1171 done + 184 open** (2026-09-12 P57.4/P57.7 background click delivery — **0 flips:** a coordinate click no longer has to move the real pointer. Windows: `WinUia::invoke_at` does `IUIAutomation::ElementFromPoint` → `InvokePattern::Invoke`, guarded by a PID check so an *occluding* window cannot answer for a point inside the target; when nothing there is invokable it falls back to `WM_LBUTTONDOWN`/`WM_LBUTTONUP` posted to the deepest child under the point via `ChildWindowFromPointEx` (search never leaves the requested window). X11: a synthetic `ButtonPress`/`ButtonRelease` pair addressed to the deepest child under the point (`send_event` with `propagate`), never XTEST. macOS: System Events has no message-level or AX-by-point primitive, so coordinate click/scroll/drag **refuse** under Background with an actionable sentence instead of warping the cursor (a named-element `click \"…\" of window 1` is AXPress and remains the background path). Scroll/drag refuse under Background on every backend (both are pointer motion by definition), and the same act now translates `ActKind::Click`'s **window-relative** coordinates to screen space on Windows (the pre-existing `send_spot` treated them as absolute). New `Capabilities::background_input` records the fact, and `readiness::derive` appends the limitation to the chip's sentence when the host has no non-moving click path. **Live X evidence:** `live_background_click_leaves_the_pointer_alone` moved the pointer to the window origin, delivered a Background click at the OCR-located `GO` button, and asserted the pointer never moved — Tk ignores synthetic events, which is exactly why the honest failure is the verify cascade, not the click call (**P57.4 PARTIAL** — escalation card + foreground snapshot/restore still open; **P57.7 PARTIAL** — AT-SPI `Action`-invoke and an AX-by-point FFI layer remain). Also fixed while compiling the macOS backend for the first time ever (`cargo test` now compiles all three backends): **two real pre-existing errors** — a missing `image::GenericImageView` import and a `&str` passed to `DesktopError::Platform(String)`. Computeruse **56** tests + 3 live green, Windows cross-compile 0 errors/0 warnings, 255 UI / 30 + 2 src-tauri green, `tsc` + cargo fmt/check clean, ipc-parity 0 broken, doc-sync green) **prior:** **1355 total = 1171 done + 184 open** (2026-09-12 P57.1/P57.3/P57.2 path-launch + background contract — **3 flips:** **P57.1** `ActKind::LaunchApp { path, app }` carries the canonical path and each backend executes it (new `crates/agentcowork-desktop/src/launch.rs`: `resolve_target` canonicalizes or does a `PATH` lookup, `validate` refuses a relative path before any policy/platform work; Linux direct exec — the `sh -c "command -v"` string is gone — Windows `ShellExecuteExW` + `SW_SHOWNOACTIVATE` under Background / `SW_SHOWNORMAL` under Foreground, macOS `open -g` with `open -a <name>` fallback), plus H5's child contract (detached stdio + name-filtered env scrub that removes provider keys/tokens/passwords/`AGENTCOWORK_*` while `PATH`/`HOME`/`DISPLAY`/`WAYLAND_DISPLAY`/`DBUS_SESSION_BUS_ADDRESS`/`XDG_*` survive); **P57.3** the background contract is enforced in every backend, not just the engine (Linux refuses `stack_mode ABOVE` + `set_input_focus`, Windows refuses `SetForegroundWindow` + `ShowWindow(SW_RESTORE)`, macOS refuses `activate`, all under Background), and **P57.2** whose launch subject is now the program itself so an allow-listed path launches without a per-session toggle while risky classes stay Guard-2; `desktop_act` gained a `launch` kind. **Live evidence on a real X server (Xvfb + matchbox, `--ignored --test-threads=1`):** `live_x11_list_capture_ocr_act_verify` asserts the Background refusal message then drives the same window on the Foreground path (activate → XTEST click → OCR reads `CLICKED`), and `live_path_launch_executes_the_file_without_a_shell_or_secrets` launches a probe script through the X11 backend asserting `$0` is the **canonical path** and that a planted `AGENTCOWORK_LAUNCH_TEST_API_KEY` never reached the child while `PATH` did; Windows cross-compile verified (`cargo check --target x86_64-pc-windows-msvc` → 0 errors/0 warnings, new `Win32_UI_Shell` + `Win32_System_Registry` features); computeruse **54** tests green, `cargo fmt` clean, src-tauri check green, ipc-parity 0 broken, doc-sync green; macOS compile-unverified here (no darwin std target installed) and coordinate clicks still move the real pointer (UIA/AX/AT-SPI invoke-first remains) — both stated rather than implied) **prior:** **1355 total = 1168 done + 187 open** (2026-09-12 P57.8 Computer-use policy surface — **1 flip:** the Settings → Computer use surface is now real and writes the policy the live Guard-2 preflight enforces: a searchable installed-app inventory (new `crates/agentcowork-desktop/src/apps.rs` — Linux freedesktop `.desktop` parsing with field-code stripping + a strict group/`Hidden` rule, macOS `.app` bundles, Windows Start Menu `.lnk`, deduped by resolved path with overlapping XDG/`/usr/share` roots collapsed, all-character search owned by the command over a session-cached scan; the ignored `live_inventory_reads_this_host` test read **this machine** — 6 roots → 40 entries, 1 correctly marked never-automatable), **Add by path** through the native picker with platform-correct options (`directory: true` on macOS since a `.app` is a directory), allow-list **rows with Remove** persisted in `<data_dir>/desktop.json` and audited (`desktop.policy.*` human_gesture) with `appliedLive` reported honestly, the **Background/foreground default** as an enforced `InteractionMode` (`DesktopEngine::act` refuses `ActivateWindow` under Background; the guard policy is behind a lock so a change reaches an already-attached engine), and the **H4 readiness chip** derived in new `crates/agentcowork-desktop/src/readiness.rs` (● Ready / ⚠ Permission required / ⚠ Driver missing / ⚠ App unsupported / ✕ Policy blocked from capabilities + policy + kill switch + attach error, 5 tests) rendered as label + glyph + sentence; hard-deny is enforced twice (picker shows `never automatable` with no Add, `AppPolicy::add_path` refuses in Rust); commands `desktop_policy_get` / `desktop_apps` / `desktop_policy_allow_path` / `desktop_policy_remove_path` / `desktop_policy_set_interaction` + `desktop_status.readiness`; +8 UI helper tests + 5 DOM tests (stateful fake backend) → 255 UI green, computeruse **46** green, src-tauri 30 + registration_sync 2 green, `tsc` + cargo fmt/check clean, ipc-parity 0 broken (282 registered), doc-sync green; P57.2/P57.3 annotated PARTIAL for the remaining platform launch/background halves) **prior:** **1355 total = 1167 done + 188 open** (2026-09-12 P55/P58 honesty wave — **7 flips:** **P55.11** MCP F6 handshake (`mcp_attach_commit` now runs `initialize` + `tools/list`, fails honestly and tears the child down when the handshake fails, persists the advertised `tool_names` instead of a count, reconciles them into the live `ToolService` catalog via `attach_external_server`, and binds a `LoopExternal` dispatcher so stdio `tools/call` runs through the native `tool/exec`+`tool/commit` Guard-2/audit path; `mcp_external_tools` reads that same registry and reports `agentVisible`; crate test calls a discovered tool on the same child and asserts a server error is not an empty success), **P55.7** E10 product path (new `browser_read_url` runs the tiered stack static→Lightpanda/Obscura→Chrome and returns the tier/source that served the read; Browse labels it; `browser_status.engine="chrome"` stops implying a light engine runs the interactive session; SSRF + `file://` floors asserted on the product path), **P55.8** `searx.space` client (`searx_space.rs` parse/eligibility/latency/cache with live|fresh_cache|stale_cache + 9 tests; `search_config.rs` local-first owner in `<data_dir>/search.json`, opt-in public instances, dedupe, malformed⇒local-only + 4 tests; `G8Cascade::set_endpoints` re-routes a live session; `search_config`/`search_instances`/`search_instances_apply` + Settings → **Search**), **P55.9** Cloud env mock deleted (the `cloud` id now routes to the real H33 `node_attach` Sync surface), **P55.10** PPT honesty (shell reads `No presentation open` not `Slide 3 / 12`; slide edits are handed to the agent's `office.pptx_patch` under Guard-2, viewer stays read-only), **P58.3** Settings nav IA (Computer use section wired to live `desktop_status`; Experts→**Built-in roles** de-duplicated against Subagents; P57.8 annotated PARTIAL for the remaining allow-list surface), **P58.4** composer slash (`ui/src/lib/slash-commands.ts` is the one owner for the intercept, the palette and the Settings switches under `commands.disabled`; the free-text add box that could advertise a handler-less `/name` is gone; ACP Chief keeps `available_commands`; +9 tests); also **test isolation fix** — `relay_dispatches_scheduler_requests` re-seats the relay's scheduler on a temp path instead of the developer's real `<data_dir>/scheduler.json`, which had made the assert depend on machine state; 242 UI / 620 core / 55 mcp / 23 search green, `tsc` + cargo fmt/check clean, ipc-parity 0 broken, doc-sync green) **prior:** **1355 total = 1160 done + 195 open** (2026-09-12 P60 model-ownership wave — 4 flips: **P55.4** no-static-seed occupancy (empty shell inventory ⇒ Native only + an explicit “Runtime inventory unavailable” note in the picker; Settings labels the seed a shipped catalog, not occupancy; `StatusBadge` reads `not installed` instead of `available`; the browser-preview fixture is the only place the seed may read as a runtime list), **P60.12** Native-vs-external model ownership (`isNativeRuntime` decides it; `getModelsForAgentLive` returns curated rows only for Native, so external ACP agents no longer render AgentCowork's provider catalog as a control over them; selection preserves the Native pin; status bar/picker/Settings paint the agent's own ACP value or `managed by <agent>`; runtime selection is installed-only), **P60.13** ACP session `configOptions` + `session/set_config_option` implemented end-to-end (Rust messages/client → `acp_session_config_options` / `acp_session_set_config_option` → `store.acpConfigOptions` → picker `Model · <agent>` block; `clientCapabilities.session.configOptions.boolean` advertised; agent-initiated `config_option_update` replaces the stored list), **P60.14** `launch_registry()` merges the cached official ACP registry into the curated seed for discovery/launch while occupancy stays `agent_installed`, the picker/Settings now treat the seed as a catalog rather than occupancy, and the path is verified against the **real CDN** (`acp_registry_refresh()` → 40 agents cached → memoised `launch_registry()` = 49 rows, +antigravity/+kimchi; plus a crate-level companion); the Native model catalog became a collapsed disclosure on the AgentCowork Native card (no peer Models tab); +17 UI tests → 233 green, src-tauri 23 + registration_sync 2 green (1 ignored live test), `tsc` clean, cargo check (workspace + src-tauri) green, ipc-parity 0 broken, doc-sync green) **prior:** **1352 total = 1156 done + 196 open** (2026-09-12 P58.7 flipped — the agent/model picker + status bar now read the live models.dev catalog rows for keyed/profiled/keyless providers (new `ui/src/lib/catalog-models.ts`; catalog pick carries its provider through `selectedModelProvider` → `resolveProviderModel` so the broker resolves the endpoint from the catalog instead of guessing by model id), curated rows kept as a clearly-labelled fallback; +12 UI tests → 203 green, tsc + vite build + ipc-parity (0 broken) + doc-sync green) **prior:** **1352 total = 1155 done + 197 open** (2026-09-12 P56 providers wave — **10 flips:** P56.1 live 4h `api.json` refresh job (conditional GET with `If-None-Match`, boot-if-stale, `catalog_set_interval`, atomic snapshot + `fetched_at`; `/api/providers.json` deliberately never fetched since it serves the SPA), P56.2 Settings → Providers list + all-character search + per-row `+` → **Provider activate screen**, P56.3 activate screen header (name/`npm`/`api`/`doc`) + key bars + MetadataOnly `provider_probe` `GET {api}/models` tick + `verified_at` persist + N-key `+` + keyless “use without key”, P56.4 OpenCode-shaped custom form (id/name/format/`base_url`/optional key/headers/body merge/temperature/models) persisting a `source: user-config` `ProviderProfile` **and** the vault key, P56.5 NVIDIA first-class + NIM overlay (`nvidia_nim_profile` → `http://localhost:8000/v1`), P56.6 three separate OpenCode rows (Zen `/zen/v1` keyed · Go `/zen/go/v1` keyed · Free keyless regex + `big-pickle`, no `Authorization`, `x-opencode-session`/request/client injected), P56.7 models.dev model table (model/id/context/output/price-per-1M/reasoning/tool-call/images + default-model dropdown + search), P56.8 429 `Retry-After` honoured + capped 24h, first-key retry after cooldown, **no** 5xx rotation, 401/403 suspends the key; plus **P55.5** (broker resolves `base_url`/transport from the catalog seed — `resolve_endpoints` at boot → `relay.with_profiles`, `DEFAULT_BASE_URLS` no longer the choke) and **P55.6** (persist keeps the URL/format/session flags); P58.7 **unblocked** (annotated, not flipped — the picker rewiring is the remaining work); catalog 103 / vault 134 / core 616 / UI 188 tests green, `registration_sync` 2/2, tsc + vite build + ipc-parity (0 broken) + doc-sync green) **prior:** **1352 total = 1145 done + 207 open** (2026-09-11 P54 terminal profiles + PTY host (H36) landed — 4 flips: P54.1 VS Code-exact profile detection + `terminal.*` registry in `agentcowork.toml` (Windows algorithm testable off-Windows via an injected `Platform`), P54.2 `TerminalBackend` with mandatory `abi_version` (Remote refuses instead of running locally), P54.3 real `portable-pty` unix-pty/ConPTY host (resize/kill/reap, session survives Shell-view unmount, live duplex round-trip test), P54.6 unsafe-path confirmation (`unsafeConfirmed` + `terminal_confirm_unsafe`); **P54.4 PARTIAL** — tabs + `+` profile dropdown + Select Default Profile + live profile/backend chip + xterm.js renderer + Ctrl+` focus + read-only⇄interactive landed, splits open; **P54.5 PARTIAL** — automation profile config + resolver landed, tasks/`script.run` consumer pending (no in-tree executor); P54.7/.8 annotated open; +25 core / +5 UI tests → 178 UI green, core terminal 25 green, tsc + vite build + doc-sync + ipc-parity (0 broken) green) **prior:** **1352 total = 1141 done + 211 open** (2026-09-11 P58 UI-honesty wave — 9 flips: P58.6 status-bar live occupancy, P58.8 browse a11y-tree label, P58.9 notification toggles wired/disabled (Guard approvals never suppressed), P58.10 experimental flags disabled with the owning queue named, P58.11 palette real chords + live agent list, P58.12 + P55.12 single persisted theme owner, P55.2 Settings → Permissions renders the live Guard matrix / allow-list / bundles, P55.3 Settings → Usage renders usage_snapshot + session_totals; P58.7 annotated BLOCKED on P56.7; +7 tests → 173 UI green, tsc clean) **prior:** **1352 total = 1132 done + 220 open** (2026-09-11 P58.1/.2/.5 cockpit-honesty flips — Settings → Keyboard renders the live shortcut map, About stamp injected from package.json at build time, Office flyout liveness derived from real attach state; P58.3 recorded PARTIAL — Subagents + Tool log nav landed, Computer use pending; verified tsc/bun/cargo/doc-sync/ipc-parity) **prior:** **1352 total = 1129 done + 223 open** (2026-09-10 P53.1–.4 flipped DONE — live ACP slash surface + Chief-dependent intercept + any-installed Chief + compact handoff bundle, verified tsc/bun/cargo; census 157) **prior:** **1351 total = 1121 done + 230 open** (2026-09-10 P60 Agent Runtime 10 open; census 157) **prior:** **1341 total = 1121 done + 220 open** (2026-09-10 P59 +6 high-accuracy ladder/contracts — 16 open; census 157) **prior:** **1335 total = 1121 done + 214 open** (2026-09-10 P59 two-surface CUA DAG + vision gate — 10 open, 0 flips; census 157) **prior:** **1325 total = 1121 done + 204 open** (2026-09-10 P58 cockpit UI stale/missing — 12 open, 0 flips; census 157) **prior:** **1313 total = 1121 done + 192 open** (2026-09-10 P56 providers UI/catalog/zen-free/429 + P57 E9 path-launch/background — 16 open, 0 flips; census 157) **prior:** **1297 total = 1121 done + 176 open** (2026-09-10 P55 honesty — 1 RAM-probe fix + 11 open; census 157) **prior:** **1285 total = 1120 done + 165 open** (2026-09-10 P54 queued — H36 terminal profiles + PTY backends, 8 open; census 157) **prior:** **1277 total = 1120 done + 157 open** (2026-09-09 P53 queued — installed-any Chief, ACP slash, compact handoff, subagent mix; 9 open, 0 flips of P38 crate rows) **prior:** **1268 total = 1120 done + 148 open** (2026-09-07 P51 wave-2 — stale-annotation pass over the 12 remaining P51 rows: P51.25 flipped DONE (cache% wire signal + timeline rail both verified landed); P51.33 chat-side /compact consumer landed (memory/compact RPC = BudgetFormula→select_tail→overflow_replay + composer /compact + store rewrite, opencode v2 formula constants confirmed — keep.tokens 15K / buffer 20K); P51.22 policy consumer confirmed + guard/set_policy_rules command + guard-panel rules editor (args-glob half now usable); P51.17 MCP name-sanitize (sanitize_attach_name enforced pre-ticket); P51.28 500-line skill budget (SKILL_MAX_LINES in SkillStore::save); P51.8/11/14/19/20/27/31 re-annotated with fresh evidence, none faked; 147 UI tests + core 24/24 + mcp 2 + blueprint 170 green) (2026-09-06 P51 wave — stale-annotation pass: reviewer/approval_policy/floors/rm_critical/notepad-incidents-doctor were all landed Rust-side and mostly wired; UI landed — task-rail ledger cost (P51.12 flip), automations Health tab + live runs (P51.32), bulk-remove with space-freed (P51.4), browse Pin-to-composer (P51.15), per-project sidebar groups (P51.13, 4 tests); P51.8/11/14/17/19/20/28/31 re-assessed with fresh evidence, none faked; 145 UI tests green) (2026-09-06 V1-packaged pass: P50.1.7 flipped — packaged release shell + AppImage clean-boot verified under Xvfb; P50.5.2 live SearXNG leg 3/3 via docker; P50.2.1 newSession-before-hydration race fixed in code (mergeHydratedSessions); Linux leg of the P50.5.8 matrix run green locally — search/MCP/gates/security/failure-injection/boot-check/parity/doc-sync; win/mac legs on push via p50-gates.yml) (2026-09-06: wave-1→5 UI landings on the P51/P52 queue — reasoning wire-path fix + live collapsible CoT rollup with timer (P52.23/P51.6), queue-while-generating + 2-stage stop + pause/promote chips (P51.5/P52.8), context/ctx-meter + breakdown popover incl. tok/s (P52.10/18/25 slice), provider-picker auto-route dead-click fix + sticky-vs-default badges (P51.1/P52.9), settings keyword search (P51.24), truncate-below edit + same-history regen (P52.22), message-action union bar (P52.24/21), find-in-page jump (P52.19), session goals (P51.9), reopen-closed + Ctrl+Tab cycling (P52.16), fuzzy @///! hints (P52.11), IME/Tab guard (P52.14), TTFB footer + partial-preserve layer-named error cards with Retry/Copy (P51.7/P51.21 slices), +19 UI tests (85→97/17 files); wave-6 same day: session-tab strip (P51.23 slice), translate-error codebook (P51.2 slice), model variant cycle ⌘⇧Q (P51.3 slice), per-pill status-bar prefs (P51.25 slice), skills catalog search/filter (P51.26 slice), Projects fuzzy search (P52.15 slice), review-changes banner (P52.17 slice), +7 UI tests (97→104/18 files), then all waves committed + pushed as 6c6d87a; Rust/coordinator-gated remainder assessed in the P51 § note — no checkbox-count flips: DONE/PARTIAL tags keep `[ ]` per the 2026-09-01 row rule, so 1117/151 stands) (2026-09-06 waves 7–8, uncommitted in tree: unified server surface + attachments pipeline + 10-attachment tests (wave 7); then P52.1/2/5 bound from already-registered Rust commands — per-quant fit traffic lights, Gallery parse/inspect tab, one-click Auto-pick — + pure model-fit helpers with 10 tests, P52.4/6/7 assessed Rust/Apple-gated, P52.20 re-confirmed wire-gated; composer audit fix — slash commands run locally before the busy-queue branch, mutating slash commands refuse mid-turn; +133 UI tests green) (2026-09-06 wave 9, uncommitted in tree: P52.4 serve-arg surface — ServeOptions kv/ngl/ctx/no-mmap/mlock → gguf_args + model_serve passthrough + per-model Advanced controls in local-server-view; P52.7 MLX sidecar — ServeRuntime::Mlx with /v1/models health-wait + LocalRuntime::Mlx + broker Mlx arm + UI runtime toggle; P52.17 real restore — fs_undo_restore + fs_undo_snapshot commands + true before/after DiffView with per-file Restore + last-turn filter; P52.15 Archive flyout — reopen-any + purge over the closed ring; P51.2 request-id — ChatError.requestId stamped from liveStreamId + copyable chip; core 37 + vault + Tauri-check green; P52.6 re-annotated — ModelCache pin/LRU still consumerless, tok/s probe pending; P52.20/P51.3/P51.23/P52.12/P52.2/P52.3 remain wire/Rust-gated as annotated) (2026-09-04: +33 P51 +24 P52, no flips) (2026-09-03: full WSL verification pass — 8 V1 rows flipped (P50.2.6/2.10/3.6/3.7/3.8/5.4/5.5/5.6) with 4 real bugs caught + fixed (router fallback violated the credential gate, p50_gates never compiled on non-Debug Vault, Google lookalike-host regression, Lightpanda-absent live assertion); release AppImage built and boots alive under Xvfb, packaged UI-struct 19/19; remaining V1: P50.1.7 packaged last mile, P50.2.1 packaged/UI E2E, P50.5.2 SearXNG-live run, P50.5.7 packaged+sandbox legs, P5.2 LadybugDB, P50.4.3/4.4 voice, P50.4.10 privacy badges) (2026-09-02: P50.4.1 first-run setup gate, P50.4.2 local-model downloads wiring, P50.4.8 capability matrix panel, P50.4.9 no-provider/offline UX, and the P50.5.1/5.3 real-provider E2E suites landed — P50.5.1 verified end-to-end vs real Ollama `qwen2.5:0.5b`; packaged shell verification remains the P50.5.8 matrix) (plus 1 spec-only ADR `[~]` P47.7 CC ring, post-v1 — counted separately; auto-reconciled 2026-08-30; P1.6 persona-selector UI row corrected to `[NOT DONE — post-v1]` 2026-09-01 — data path only, no UI dropdown; auto-reconciled 2026-08-30 — `scripts/check-doc-sync.mjs` verifies this line against the file; v3.64 Work Gateway / Session Runtime (spec §4.4, P49 — 20 new open items) added → open 84→91; v3.63 P47.3 agentcowork-types + P47.4 Execution→Work + P48.3 remainder landed → P47 7/7 done + 1 spec; open 87→84) (v3.61 P48.3 partial landed — browser human-path audit + E9 desktop seam + connector-write audit hook; 2 done / 2 open in P48; v3.60 +4 P48 rows — 1 done / 3 open; v3.59 +7 P47 rows — 2 done / 5 open; P36 21/21 + P31 9/9 landed 2026-08-24; P11 31+70 landed 2026-08-23; P9.1 8/8 landed 2026-08-23; P5.12 + P27 backend + MCP external-client E2E landed 2026-08-23; E9 landed 2026-08-23; v3.39 fields closed; P8.8/P8.9 landed; **P10.1–P10.5 50/50 landed 2026-08-25/26** — p10_e2e/p10_security/p10_bench suites + CI matrix/nightly/perf/pre-commit; **P11.6 + P35 landed 2026-08-25** (UX polish 8/8 + P35.1 sparkline gap closed); **P13–P26 batch closure 2026-08-24/25** — P13 11/11, P14 5/5 (P14.5 sync automation landed 2026-08-26), P15 5/5, P16 8/8, P17 10/10, P18 5/5, P19 4/4, P20 2/2, P21 2/2, P22 3/3, P23 4/4, P24 3/3, P25 4/4, P26 3/3; **P28/P29/P37/P16/P30/P32/P33/P38–P41 parallel landings 2026-08-24** (+211 tests; P38 re-verified 7/7 end-to-end — acp-10 + coordinator chief-10, Work-survives-Chief + delegation + dispatcher + E2E; P39 5/5 incl. P39.5 lazy-load; P40 3/3 incl. BYO-host `deploy/` pack; P41 4/4 incl. worktrees/ticketed writes/receipts); **P42 3/3 + P43 4/4 crate-done 2026-08-26** (49/49 connectors + 10/10 task_ledger green — UI wiring follow-ons); **P30.16 + P33.5/6 + P34.2–4.5/4.7 landed 2026-08-26** (companion chip consumer; LO optional companion per doc-29 verdict — no in-process LOKit; Google Docs browser-view read path; honest office viewers with read-only+takeover locks — MS-ribbon clones deliberately not built). **New matrix rows + queue (v3.56, 2026-08-26):** A11 Provider Record + alias layer and H34 Autonomy Level added to the spec/ARCH/09 (+2 rows, 151→153 spec index) and queued as **P44 (9 items; P44.1–3 now landed — see below)** — Hermes `providers.py` + OpenCode provider-directory source-read: provider identity = models.dev + overlays + user config + plugin profiles with alias normalization; OpenAI-compatible = one transport many profiles; capability-probe verification; Sandbox/Ask/Auto/Max chatbar autonomy presets on the existing permission engine (never a Guard bypass) with per-task snapshots, escalation cards, temporary elevation, live indicator; continuous resource-discovery surface. **P44.1–P44.4 landed 2026-08-26** (provider crate in `agentcowork-catalog` — `ProviderRecord` merge resolver + Hermes alias table + OpenCode-compatible profile table + capability-probe verification, 14 provider/probe tests green; autonomy/discovery/routing surfaces remain). **P44.6 landed in full 2026-08-26** (three-control composer `chat-composer.tsx` + `permissionMode` persistence + per-task `config_hash` freeze + Autonomy Limit escalation card + temporary elevation + live indicator + 7 store tests — `ui/src/lib/store.ts`/`bridge.ts`/`now-doing-strip.tsx`/`autonomy.test.ts`; Rust preset maps remain P44.5). **Still open:** P5.2 LadybugDB FFI, P6.9 Signal deferred, P9.2–P9.9 (20 post-v1), P12 GTM (47), P44 (3 — P44.7–9; P44.5 landed 2026-08-30), **P45 (9 — R4-gated micro-perf deltas, new 2026-08-26)**, **P46 (3 — spec I13 /learn · H35 CLI/doctor · J24 credential-fill; P46.4 ref-invalidation landed 2026-08-30; all post-v1/queued, new 2026-08-26)**, **P49 (4 open — Work Gateway / Session Runtime, v3.64 spec §4.4: the V1-local wiring landed 2026-09-14 (16/20 — addressing, nodes, authority, broker, resolver, clients, reviews, steering, manifest, attachments, presence, PTY/worktree/agent-session, remote-approval security); open = P49.5/.6 sandbox platform backends + runtime monitor, and P49.19/.20 remote clients + multi-node failover; post-v1 = remote clients + multi-node)**. (**v3.57 2026-08-26:** new spec/ARCH rows I13 + H35 + J24 (151→153→156 index, all ⚪ post-v1, queued as P46); composer redrawn as **three independent controls — Agent ▾ (WHO) / Work Mode ▾ (Auto·Plan·Build·Research — WHAT) / Autonomy ▾ (H34 — HOW MUCH)** replacing the v3.56 "Normal·Plan·Research·Quick·Code" pills (spec §4.1 + UI-DESIGN-PROMPT + P44.6); §0 one-liner narrowed to *side-effecting* operations (inference stays in the model/data plane); A11 wording split ProviderRecord into identity / profile / observation (ProviderObservation already a separate named contract, A7/H9); A6 snapshot-vs-live catalog clarified (vendored bootstrap + live refresh, ETag/hash); §8 non-goals written: team/shared Work in v1, ACP server in v1.) (Perf-review line-check 2026-08-26: P29 11/11 + P39 5/5 already landed — native sidecar seams, token streaming in Rust, payload budgets, keep-alive pooling, KV-cache, lazy-load; MessagePack/FlatBuffers/shmem stay **not adopted** per R4.) (P7.8/P7.9 sandbox rows provide profiles, broker, seccomp DSL, and a tested monitored Linux `bwrap` launcher primitive. ACP Tauri launch now uses the shared-pipe monitored transport — `ProcessTransport::spawn_sandboxed` over `LinuxBwrapBackend::spawn_stdio`, landed 2026-09-02 with a bwrap ≥0.9 flag fix (`--disable-userns/--assert-userns-disabled`) and a real bwrap round-trip test. MCP server launch still uses uncontrolled stdio constructors and must get the same shared-pipe treatment; macOS Seatbelt, Windows restricted-token/Job Object, escalation backends, packaged lifecycle enforcement, and native reviewed-import application remain release-gated. Sandboxed external imports must use a concrete monitored process plus the validated `ReviewedImport` manifest.) A2A verified v1.0.0 (complementary to MCP). v3.51 Algorithm Index 33→34 (FSRS #34).
> **Source reuse:** `@agentcowork/core-*` packages are **vendored in-repo** at `packages/core-*` as workspace deps (not copied; the old `../APP` sibling + `APP_CLONE_TOKEN` gate were removed 2026-08-29). Desktop-only additions go in `packages/coordinator/` or `crates/`.
> **Current reconciliation (2026-09-12):** P55.5/P55.6 and P56.1–P56.8 are landed and wired through the live catalog/profile/broker path. P58.7 is also landed: the picker and status bar consume reachable models.dev provider rows, carry the provider-qualified selection into routing, and label curated seed rows as fallback. Unsupported provider transports fail closed; the UI does not claim that every catalog row is chat-capable.
> **Provenance chain (how to find the research for any task):** task → SPEC row ID in the section header (e.g. `P1.7 (A4)`) → `ARCH/09-FEATURE-MATRIX.md` **Source** column for that row → `RESEARCH/desktop_app/` doc (01–91; the newest are **89** guard network/config-floor audit → P62, **90** native-agent peer-schema provenance → ARCH/17 / TODO P64, and **91** Windows-first runtime/picker/cowork evidence → ARCH/12/17 / TODO P66) → **doc 41** (steal-vs-reference-master-index) for the 🔴 STEAL / 🟡 ADAPT / 🟢 REFERENCE verdict + source files; **doc 63** (37-repo steal ledger, 2026-08-15) for the harness/browser/office/user-capability cluster verdicts; **doc 65** (batch-3 agent-infra/scraping/search/UI, 2026-08-15) for the A9/J11/G8/E14/I2/F8/I7/I11/P6/P5 extension steals; **doc 66** (anomalyco org, 2026-08-15) for the A6/A9/A7 models.dev catalog steal (TODO P14); **doc 67** (capability deltas + UI/UX finalization, 2026-08-15) for H29 dashboard artifacts (bolt.diy), B7 heartbeat automations (Hatchet lease pattern), and the H20 views-rail redesign (ARCH/12 v2.0); **doc 68** (final all-rounder market research, 2026-08-15) for H30 voice-memo→report, H31 corpus-research surface + audio digest, H32 agent picker + agent-scoped model surface, and the two-channel capability injection (F12/J17/F7); **doc 69** (ACP agent ecosystem + harness deep-dive, 2026-08-16) for the verified ACP entrypoint catalog (Claude Code/Codex/Cline/OpenCode/Hermes/OpenClaw/Copilot/Gemini/…) + Zed/Cline/Hermes steal queue (TODO P17); **doc 70** (mcpservers.org directory inbuilt analysis, 2026-08-16) for the MCP-directory verdict: **don't** bundle document/browser MCP servers (our Rust engines supersede them); **do** add three *native* inbuilt capabilities — PDF page ops (split/merge/rotate/reorder via lopdf, `oxidize-pdf` steal), content search + OCR (`dowse` adapt), and a Gmail/IMAP read-first connector (`mailwarden`/`Busymail` approve-before-send pattern) — TODO P18; **doc 71** (batch-4 coding agents/skills/harnesses, 2026-08-16) for the Kilo Gateway routing / ruflo swarm+federation / system-prompt structure / ui-ux-pro-max design-skill queue — TODO P19; **doc 72** (batch-5 code-intel/parallel/search, 2026-08-16) for the SeekStorm embedded hybrid index + Superset worktree-per-agent queue — TODO P20; **doc 73** (batch-6 computer-use/full-control, 2026-08-16) for the OpenAdapt demonstration compiler (B8 crystallization + E9) + ShowUI-Aloha learning half + auggie F12/ACP entry — TODO P21; **doc 74** (built-in MCP Server Manager, 2026-08-16) for the "bundle the manager, not the servers" optimization — mirror the ACP registry/installer/transport machinery to consume third-party MCP servers, postgres-mcp-hardened refuse-twice write template — TODO P22; **doc 75** (anthropic skills/plugins/cowork, 2026-08-16) for the `.claude-plugin/plugin.json` component schema (skills+agents+hooks+MCP+LSP+monitors), inbuilt native skill-wrappers vs marketplace "Add", and the source-available document-skills license boundary — TODO P23; **doc 76** (batch-7 design/browser/self-healing, 2026-08-16) for open-design `DESIGN.md` brand-system + composable design-skills, browser-harness self-healing, and the MagenticLite browser+FS+HITL validation — TODO P24; **doc 77** (batch-8 workflows/graphify/browser, 2026-08-16) for the agent-authored programmable-workflow model (Airflow DAG/retry/backfill semantics), Graphify queryable knowledge-graph, and addyosmani exit-criteria skills — TODO P25; **doc 78** (batch-9 marketplace/gws/jobs, 2026-08-16) for the wshobson/agents multi-harness plugin catalog, the `gws` Google Workspace connector, and the AIHawk "Jobs" vertical — TODO P26; **doc 79** (local-model fetch/download core, 2026-08-16) for the resumable HF GGUF/MLX downloader + canonical store + `local://` model URL — TODO P27. If a task lacks an inline doc ref, walk this chain before writing code — never re-research what's already mapped.
>
> **ADR-0001 (Connector-platform decision 2026-08-16):** MCP is the platform — no third-party aggregator (Composio/Zapier/Nango removed). Full rationale: [`ARCH/ADR/0001-connector-platform-mcp-first.md`](ARCH/ADR/0001-connector-platform-mcp-first.md).
>
> **ADR-0002 (UI v2 migration 2026-08-16):** the v2 cockpit replaced the v1 router pages; capability map + consequences: [`ARCH/ADR/0002-ui-v2-cockpit-replaces-v1-router-pages.md`](ARCH/ADR/0002-ui-v2-cockpit-replaces-v1-router-pages.md).
> **ADR-0003 (Architecture thaw 2026-09-20):** the v3.64 "no more architecture expansion" freeze is re-opened and re-frozen under a single root authority, `ARCH/CORE.md`; the term "Chief" is retired as a concept; the set is 16 primitives + 27 invariants. Full rationale: [`ARCH/ADR/0003-architecture-thaw-core-authority.md`](ARCH/ADR/0003-architecture-thaw-core-authority.md).
> **ADR-0004 (Behaviour-profile invariant 2026-09-20):** amends ADR-0003 by adding **I27** — behavioural policy is declared once, agent-agnostically, and compiled into each adapter's native mechanism; a clause an agent cannot enforce is reported `unenforceable` for that binding rather than silently dropped. Full rationale: [`ARCH/ADR/0004-behaviour-profile-invariant.md`](ARCH/ADR/0004-behaviour-profile-invariant.md).
> **ADR-0005 (External agents are the v1 engines 2026-09-21):** external agents are the only first-class main engines in v1; the built-in engine is **deferred to post-v1** as a *governed baseline binding*. Amends ADR-0003's "one option among equals" for v1 scope; re-scopes `ARCH/ROUTING.md` to agent routing; adds `ARCH/AUTOMATION.md`; narrows I10 to AgentCowork-managed credentials. **The `delegate.*` façade is a prerequisite** (`P71.1`) — without it the removals delete multiagent. Full rationale: [`ARCH/ADR/0005-external-agents-are-the-v1-engines.md`](ARCH/ADR/0005-external-agents-are-the-v1-engines.md).
> **ADR-0006 (Session kinds 2026-09-21):** `interactive` · `automation` · `delegated` — resolves the silence between `ARCH/SESSION.md`'s 1:1 Chat↔Session rule and `ARCH/WORK.md` §7's "every trigger creates Work": a trigger-created Session has **no Chat**, and every Work still has an owning Session. Full rationale: [`ARCH/ADR/0006-session-kinds.md`](ARCH/ADR/0006-session-kinds.md).
> **ADR-0008 (Session workbench projection and resource leases 2026-09-24):** the accepted, non-authoritative
> `SessionWorkbenchProjection` and typed Work/Run-owned `ResourceLease`/fencing contract; implementation and
> qualification remain open in `P71.10`. Full rationale: [`ARCH/ADR/0008-session-workbench-projection-and-resource-leases.md`](ARCH/ADR/0008-session-workbench-projection-and-resource-leases.md).

<!-- VERIFICATION POLICY: Every completed task MUST be verified before marking [DONE].
     Verification means: code compiles, tests pass, behavior confirmed (manual or automated).
     UI/product tasks: mock/demo data does not count. Crate-only tasks may be [DONE] when
     cargo/bun tests pass; wiring them into ChatRelay/Tauri/UI is a separate checkbox.
     If verification is not possible, document WHY and mark [DONE — unverified: reason]. -->

</details>

<details>
<summary>v0-era phase ledger — every still-open row, with section counts (2026-09-26 snapshot; non-authoritative)</summary>

## Architecture for implementers (copy from spec — develop here)  (0 done / 0 open as of 2026-09-26)

## P50 — Consumer Production-Integrity Audit (opened 2026-08-31)  (40 done / 6 open as of 2026-09-26)

### P50.2 — Remove demo data from native product behavior  (8 done / 2 open)
- P50.2.2 — Memory `[NOT]`
- P50.2.5 — Analytics and notifications `[NOT]`

### P50.4 — Consumer-facing capability gaps that must be explicit  (9 done / 1 open)
- P50.4.3 — Voice input `[PARTIAL]`

### P50.5 — Release verification gates  (5 done / 3 open)
- P50.5.2 — Real search E2E `[PARTIAL]`
- P50.5.7 — Security release gate `[PARTIAL]`
- P50.5.8 — Cross-platform release matrix `[NOT]`

## KERNEL GATE — v1 release blockers (Fix 3, 2026-08-30)  (3 done / 0 open as of 2026-09-26)

## STAGE 0 — Guard-Gated Tool Executor (landed 2026-08-20 · S0.1–S0.7 · production standard)  (29 done / 0 open as of 2026-09-26)

## HARDENING — Security-Posture Fixes + Agentic-Harness Patterns (adversarial deep-analysis 2026-08-19)  (30 done / 0 open as of 2026-09-26)

## H4 — Honesty leftovers (2026-08-20 line-by-line audit — were missing from this file)  (10 done / 0 open as of 2026-09-26)

## PHASE 0 — Workspace & Skeleton (~2 weeks)  (49 done / 0 open as of 2026-09-26)

## PHASE 1 — Chat + BYOK Key-Rings (~4 weeks)  (59 done / 1 open as of 2026-09-26)

### P1.6 Chat UI (H1/H7 — ARCH/11 A-1/A-10/C-1 + ARCH/12-UI-SPEC; SPEC H7 KaTeX+highlight; doc 41 P1 chatbox/jan; doc 16 Hermes SOUL.md B-2)  (5 done / 1 open)
- Truth-state correction 2026-09-01 `[NOT]`

## PHASE 2 — Browser Layer (~6 weeks)  (92 done / 0 open as of 2026-09-26)

## PHASE 3 — Cockpit & Audit UI (~4 weeks)  (14 done / 0 open as of 2026-09-26)

## PHASE 4 — Office Engine (~5 weeks) — ⏸ **ON HOLD (user directive 2026-08-22; partial lift 2026-08-26 — honest-viewer tier landed)**  (56 done / 0 open as of 2026-09-26)

## PHASE 5 — Memory Fusion + Token Economy (~5 weeks)  (71 done / 1 open as of 2026-09-26)

### P5.2 LadybugDB Graph Backend (C6, Algorithm #30 — doc 07, doc 34 §2, doc 46 Graphiti)  (5 done / 1 open)
- the swap-in seam is landed (2026-08-23): `graph::GraphBackend` trait + native `GraphStore` impl (P5.2), so a future FFI backend is a drop-in, never... `[NOT]`

## PHASE 6 — Orchestration + Connectors (~5 weeks)  (101 done / 1 open as of 2026-09-26)

### P6.9 Messaging Bridges (F13 — doc 36 §B Secure OpenClaw; doc 39 §B1 DeerFlow channels-first run_policy/dedupe)  (7 done / 1 open)
- Signal adapter + always-on 24×7 daemon + iMessage (macOS only) `[DEFERRED]`

## PHASE 7 — Forge + Guardrails Hardening (~4 weeks)  (64 done / 0 open as of 2026-09-26)

## PHASE 8 — Product Polish + Release (~3 weeks)  (42 done / 3 open as of 2026-09-26)

### P8.8 Packaging & Distribution (ARCH/01, doc 41 P8 — lencx/chatgpt + jan refs) — **verified landed 2026-08-22**  (4 done / 3 open)
- Build macOS .dmg + .app (code sign + notarize ready) — release matrix `macOS (app + dmg)`; APPLE_CERTIFICATE/IDENTITY/ID `[OUT]`
- Build Linux .deb + .rpm + .AppImage — release matrix `linux (deb + rpm + AppImage)`; webkit2gtk-4.1/appindicator/rsvg/rp `[OUT]`
- CI: build matrix for all 3 platforms — ci.yml (cargo test/clippy/fmt ×3 OS, green) + release.yml (installer build matrix `[OUT]`

## PHASE 9+ — Post-v1 (later)  (10 done / 19 open as of 2026-09-26)

### P9.1 Desktop Computer-Use (E9 — ChatGPT + Claude parity, **required / not a cut**; Module 5 Work-Native Primitives visual-grounding reference)  (7 done / 1 open)
- macOS twin `[OUT]`

### P9.2 WASM Fuel-Metered Sandbox (I3 — doc 09)  (0 done / 1 open)
- Implement wasmtime integration with fuel budgets + epoch interruption `[NOT]`

### P9.3 Voice Input (H15 — doc 33 §10, doc 50)  (0 done / 1 open)
- Implement VAD (Voice Activity Detection) + speech-to-text `[NOT]`

### P9.4 Remote Session Handoff (H18 — doc 35 §C OpenWebUI Computer pattern)  (0 done / 2 open)
- Implement LAN/Tailscale/tunnel view of running sessions `[NOT]`
- Implement resume from phone mid-run (extends session checkpointing + E2E sync) `[NOT]`

### P9.6 HTML→Video Reports (doc 46 Devin hyperframes)  (0 done / 1 open)
- Hyperframes integration for agent-generated video content `[NOT]`

### P9.7 Magic Completion (H16 — doc 01 AnythingLLM Magic Tab; doc 13 connector sync)  (0 done / 5 open)
- Implement inline context-aware completion (AnythingLLM pattern) `[NOT]`
- Connector sync → RAG (self-hosted pipeline via MCP/native connectors — aggregator sync removed per the Connector-platfor `[NOT]`
- AutomationBench eval harness (long-horizon desktop automation scoring) `[NOT]`
- 2026-08-29: signing + install/consent flow landed `[IN]`
- Self-hosted connector-hub server (doc 13 opt-in) `[NOT]`

### P9.8 Voice Output TTS + Wake Word (H28, H15 ext — doc 50)  (0 done / 5 open)
- sherpa-onnx first `[NOT]`
- Implement read-aloud toggle in chat (speaker button) + per-message TTS `[NOT]`
- Add optional BYOK cloud TTS (OpenAI/ElevenLabs) via provider rails `[NOT]`
- Add optional wake word (openWakeWord, Apache-2.0) for hands-free voice activation `[NOT]`
- Offline STT option selection (Vosk / sherpa-onnx / whisper.cpp) for H15 `[NOT]`

### P9.9 Image Generation (A10 — doc 50)  (0 done / 3 open)
- Implement image-gen provider endpoint (GPT-Image-1 / DALL·E 3 / Flux / Stable Diffusion / MCP image server) with key-rin `[NOT]`
- Implement chat image tool (text-to-image + image editing; ref-handle results → artifact card) `[NOT]`
- MCP image-server compatibility path (any MCP server via F6 client) `[NOT]`

## CROSS-CUTTING (applies to all phases)  (54 done / 0 open as of 2026-09-26)

## PHASE 10 — End-to-End Testing & Quality Assurance (cross-cutting — validates P0–P9; doc 26 red-team for P10.2)  (43 done / 14 open as of 2026-09-26)

### P10.3 Performance & Stress Testing (ARCH/02 budgets, doc 33 §9 replay scale, ARCH/05 token economy)  (13 done / 1 open)
- measure & publish the real number `[NOT]`

### P10.4 Cross-Platform Testing (ARCH/01 platforms, doc 41 P8 packaging refs)  (3 done / 5 open)
- Test full flow on macOS Sequoia (ARM) — same suite `[OUT]`
- Test full flow on Ubuntu 24.04 (x64) — same suite `[OUT]`
- Test Tauri auto-updater on all 3 platforms `[OUT]`
- Test Ollama integration on all 3 platforms (spawn, connect, chat) `[OUT]`
- Test system Chrome/Edge detection + fallback on all 3 platforms `[OUT]`

### P10.5 Regression & CI/CD (ARCH/01 CI, doc 29 LibreOffice oracle)  (5 done / 1 open)
- Set up CI matrix: cargo test + vitest + Tauri build for Win/Mac/Linux `[OUT]`

### P10.6 — Master test suites (TEST-CASES.md, opened 2026-09-25)  (0 done / 7 open)
- P10.6.1 — 50 `E2E-UC-01..50` cases `[NOT]`
- P10.6.2 — 8 `INT-01..08` cross-module suites `[NOT]`
- P10.6.3 — 48 `M#-*-*` module test IDs `[NOT]`
- P10.6.4 — Tier 3 nightly stress targets `[NOT]`
- P10.6.5 — Tier 4 Security Pen-Test Suite + WCAG 2.2 AA audit on the RC `[NOT]`
- P10.6.6 — `UX-TESTING-PLAN.md` R1/R2/R3 round execution `[NOT]`
- P10.6.7 — Windows v1 validation apparatus `[NOT]`

## PHASE 11 — UI/UX Design & Optimization (ARCH/12-UI-SPEC)  (31 done / 0 open as of 2026-09-26)

## PHASE 12 — Market Research & Go-to-Market (live market research — no research doc)  (0 done / 47 open as of 2026-09-26)

### P12.1 Competitive Analysis (Live) (live GTM research — desktop AI landscape, RESEARCH/2026-ai-landscape)  (0 done / 11 open)
- 2026-09-04 (doc 86, code/docs-level — install leg open) `[NOT]`
- 2026-09-04 (doc 86 — install leg open) `[NOT]`
- 2026-09-04 (doc 86 — hands-on leg open) `[NOT]`
- 2026-09-04 (doc 86 — hands-on leg open) `[NOT]`
- 2026-09-04 (doc 86 — hands-on leg open) `[NOT]`
- 2026-09-04 (doc 86) `[NOT]`
- 2026-09-04 (doc 86 — hands-on leg open) `[NOT]`
- 2026-09-04 (doc 86 §9) `[NOT]`
- 2026-09-04 (doc 86 §10) `[NOT]`
- holaOS (doc 58 — closest whole-product competitor) `[NOT]`
- UI-only reference pass (doc 58) `[NOT]`

### P12.2 Target Audience & Personas (live GTM research)  (0 done / 6 open)
- Define persona 1: Power developer (uses Claude Code/Codex daily, wants more control) `[NOT]`
- Define persona 2: Knowledge worker (Excel/Word/PDF daily, wants AI automation) `[NOT]`
- Define persona 3: Privacy-conscious researcher (local-first, no cloud, BYOK) `[NOT]`
- Define persona 4: Automation builder (Zapier/n8n user wanting AI-native workflows) `[NOT]`
- Map feature priorities per persona (which capabilities matter most to whom) `[NOT]`
- Define value propositions per persona (one sentence each) `[NOT]`

### P12.3 Positioning & Messaging (live GTM research; ARCH/09 differentiators)  (0 done / 6 open)
- Write product tagline (one sentence, <15 words) `[NOT]`
- Write product description (one paragraph, <100 words) `[NOT]`
- Write "Why AgentCowork?" page (3–5 key differentiators with evidence) `[NOT]`
- Write comparison pages: "AgentCowork vs ChatGPT", "vs Claude Code", "vs AnythingLLM" `[NOT]`
- Define naming: finalize product name (AgentCowork? Other?) `[NOT]`
- Design brand identity: logo, wordmark, color usage `[NOT]`

### P12.4 Launch Strategy (live GTM research)  (0 done / 9 open)
- Plan open-source launch: GitHub repo, LICENSE (MIT/Apache-2.0), CONTRIBUTING.md `[NOT]`
- Write README.md: hero description, screenshot, install instructions, feature list `[NOT]`
- Plan Hacker News launch post (title, Show HN format, key hooks) `[NOT]`
- Plan Reddit launch: r/LocalLLaMA, r/selfhosted, r/macapps, r/programming `[NOT]`
- Plan Twitter/X launch thread (8–10 tweets showing different capabilities) `[NOT]`
- Plan YouTube demo video (3–5 min, showing killer features in action) `[NOT]`
- Plan Product Hunt launch (timing, hunter, first comment, assets) `[NOT]`
- Identify early adopter communities: AI Discord servers, dev Slack groups, HN regulars `[NOT]`
- Plan beta program: 50–100 early testers, feedback channel, weekly builds `[NOT]`

### P12.5 Documentation & Community (ARCH/00–12 as contributor docs source)  (0 done / 10 open)
- Write installation guide (Windows, macOS, Linux — with screenshots) `[NOT]`
- Write "Getting Started" tutorial (first 5 minutes to value) `[NOT]`
- Write provider setup guides (Anthropic, OpenAI, DeepSeek, Ollama) `[NOT]`
- Write skill/plugin development guide (how to build extensions) `[NOT]`
- Write ACP integration guide (how to connect external agents) `[NOT]`
- Write architecture overview for contributors (simplified ARCH/01) `[NOT]`
- Set up community: GitHub Discussions or Discord server `[NOT]`
- Write CONTRIBUTING.md: code style, PR process, testing requirements `[NOT]`
- Write SECURITY.md: vulnerability reporting, threat model summary `[NOT]`
- Plan docs site (Docusaurus/VitePress, hosted on GitHub Pages) `[NOT]`

### P12.6 Monetization Research (future, not v1) (live GTM research)  (0 done / 5 open)
- Research open-source monetization models: open-core, support, hosting, marketplace `[NOT]`
- Evaluate skill/plugin marketplace potential (community-contributed, optional premium) `[NOT]`
- Evaluate "AgentCowork Pro" optional features: cloud sync, team sharing, priority support `[NOT]`
- Research pricing benchmarks: comparable tools (Jan, Cherry Studio, Cursor Pro pricing) `[NOT]`
- Define v1 = 100% free, v2+ = evaluate adding optional paid tier `[NOT]`

## P11.5 — UI Implementation (from ARCH/12-UI-SPEC, ~4 wks parallel)  (75 done / 0 open as of 2026-09-26)

## P13 — Batch-3 Steal Queue (doc 65, 2026-08-15 — 19 new repos, 8 steals → 11 tasks; all extend existing rows, none re-specified)  (11 done / 0 open as of 2026-09-26)

## P15 — Capability-Delta Queue (doc 67, 2026-08-15 — bolt.diy / Hatchet / durable-execution-the-hard-way)  (5 done / 0 open as of 2026-09-26)

## P14 — Model Catalog: models.dev Steal (doc 66, 2026-08-15 — anomalyco/models.dev, MIT)  (5 done / 0 open as of 2026-09-26)

## P16 — Final Market-Research Deltas (doc 68, 2026-08-15 — Microsoft Copilot Cowork / Gemini Notebook / agent picker / two-channel injection)  (5 done / 3 open as of 2026-09-26)
- H30 voice-memo → structured report (doc 68 §3) `[POST-V1]`
- H31 corpus-first research surface + audio digest (doc 68 §2.2) `[POST-V1]`
- Two-channel injection — Channel B (doc 68 §4) `[IMPLEMENTED]`

## P17 — ACP Agent Ecosystem Steal Queue (doc 69, 2026-08-16 — Claude Code / Codex / OpenCode / Cline / Hermes / OpenClaw + Zed re-deep-dive)  (10 done / 0 open as of 2026-09-26)

## P18 — MCP Directory Inbuilt Queue (doc 70, 2026-08-16 — mcpservers.org/all, 11,054 servers)  (5 done / 0 open as of 2026-09-26)

## P19 — Batch-4 Coding Agents / Skills / Harnesses Queue (doc 71, 2026-08-16)  (4 done / 0 open as of 2026-09-26)

## P20 — Batch-5 Code-Intel / Parallel Agents / Search Queue (doc 72, 2026-08-16)  (2 done / 0 open as of 2026-09-26)

## P21 — Batch-6 Computer-Use / Full-Computer Control Queue (doc 73, 2026-08-16)  (2 done / 0 open as of 2026-09-26)

## P22 — Built-In MCP Server Manager Queue (doc 74, 2026-08-16)  (3 done / 0 open as of 2026-09-26)

## P23 — Anthropic Skills / Plugins / Cowork Queue (doc 75, 2026-08-16)  (4 done / 0 open as of 2026-09-26)

## P24 — Batch-7 Design / Browser Self-Healing / Computer-Use Queue (doc 76, 2026-08-16)  (3 done / 0 open as of 2026-09-26)

## P25 — Batch-8 Programmable Workflows / Graphify / Browser Queue (doc 77, 2026-08-16)  (4 done / 0 open as of 2026-09-26)

## P26 — Batch-9 Marketplace / Google Workspace / Jobs Queue (doc 78, 2026-08-16)  (3 done / 0 open as of 2026-09-26)

## P27 — Local Model Fetch / Download Core Queue (doc 79, 2026-08-16)  (6 done / 0 open as of 2026-09-26)

## P51 — Competitor Desktop Deep-Dive Steal Queue (doc 86, 2026-09-04 — opencode/open-cowork/Hermes/AnythingLLM/Jan/Cherry/Chatbox/OpenWorker/Open-WebUI/Claude-Code/Codex/holaOS/Cowork/OpenChamber/OpenClaw/Crush/Zed/Cursor; all patterns-only where licenses require)  (5 done / 28 open as of 2026-09-26)
- P51.1 — Provider picker (dead-click fix + live route feed) `[DONE]`
- P51.2 — Spotlight fast-switch + localized error cards `[PARTIAL]`
- P51.3 — use-policies + variant cycle `[PARTIAL]`
- P51.4 — Jan A5 gold standard `[PARTIAL]`
- P51.5 — Queue-while-generating + 2-stage stop + editable pending chips `[DONE]`
- P51.6 — CoT single-collapsible rollup + live-thought header + true abort `[DONE]`
- P51.7 — Partial-preserve + retryable errors + TTFB footer `[PARTIAL]`
- P51.8 — One-composer Chat/Cowork switch + shared chats+tasks home `[NOT]`
- P51.9 — Session Goals `[DONE]`
- P51.11 — Bot=profile forever-chat + routines-as-namespaced-cron + typed-reason DMs + relay/peer duality `[NOT]`
- P51.13 — Zed-class worktree isolation + history-restore + per-project thread rows `[PARTIAL]`
- P51.14 — Agent cards (status/steps/files/tests) + Awaiting-Input pause + conflict flag + handoff artifacts `[IMPLEMENTED]`
- P51.15 — Annotate-to-composer browser loop `[PARTIAL]`
- P51.16 — OS-enforced sandbox profiles (Seatbelt/bwrap) + granular approval_policy + fail-closed auto-reviewer `[PARTIAL]`
- P51.17 — npx-resolution (system-over-bundled + trusted-list) + MCP name-sanitize + bundled-node PATH enrichment `[PARTIAL]`
- P51.19 — acpx driver shape `[IMPLEMENTED]`
- P51.20 — IM per-scope approve-then-read + Combos one-click bundles `[IMPLEMENTED]`
- P51.21 — Layer-named error cards + matched actions `[PARTIAL]`
- P51.22 — 3-mode composer approvals (Manual/Auto/Skip) + per-connector Always/Needs/Blocked matrix + per-task override `[PARTIAL]`
- P51.23 — Session tabs + multi-server switcher + drafts.sqlite `[PARTIAL]`
- P51.24 — Settings sidebar search `[DONE]`
- P51.26 — Skills catalog UX `[PARTIAL]`
- P51.27 — ClawHub-class publish/version/vector-search/pin + frontmatter security scan + default-off bridges `[PARTIAL]`
- P51.29 — Human-only floors `[PARTIAL]`
- P51.30 — Deny-first + critical-path rm guard + protected-paths + ask-survives-hook-allow `[PARTIAL]`
- P51.31 — Dual-provider remote-attach `[NOT]`
- P51.32 — Cron-continuity + per-job notepad + monitor-mode + preflight + model-drift guard + incidents-ack + doctor + runs ledger `[PARTIAL]`
- P51.33 — Compaction budget formula `[PARTIAL]`

## P52 — Local-model RESOURCE + handoff plane + Composer/Chat-UI Steal Queue (doc 87, reviewed 2026-09-25 — local-runtime interop + 10 composers + thread UI; REPORTED items flagged, all patterns-only where licenses require)  (1 done / 33 open as of 2026-09-26)
- P52.1 — Advisory hardware-fit estimate `[KEEP]`
- P52.2 — Gallery metadata parser `[KEEP]`
- P52.3 — Managed runtime lifecycle and server boundary `[REWRITE]`
- P52.4 — Explicit llama.cpp runtime options `[KEEP]`
- P52.5 — Auto-best variant `[KEEP]`
- P52.6 — Pin/LRU/TTL and benchmark ownership `[DEFER]`
- P52.7 — MLX `[DEFER]`
- P52.8 — Ordered queue `[DONE]`
- P52.9 — Sticky-vs-default badge + mid-switch cost warning `[DONE]`
- P52.10 — Scope/cost line in chrome `[DONE]`
- P52.11 — Fuzzy `@` + `/` catalog `[DONE]`
- P52.12 — Attach contract `[PARTIAL]`
- P52.13 — Quick Entry global hotkey + HUD mini-composer `[DONE]`
- P52.14 — Reserved Tab-completion + CJK-composition guard `[DONE]`
- P52.15 — Projects>Topics `[PARTIAL]`
- P52.16 — reopen-closed + Ctrl+Tab `[DONE]`
- P52.17 — Review Changes `[PARTIAL]`
- P52.18 — Context meter + breakdown popover `[DONE]`
- P52.19 — Find-in-page jump `[DONE]`
- P52.21 — Export Markdown `[DONE]`
- P52.22 — Truncate-below edit + same-history regen `[DONE]`
- P52.23 — Collapsible reasoning + live timer + tool chips w/ durations `[DONE]`
- P52.24 — Message-action union bar `[DONE]`

### P52-R — Local runtime interoperability (opened 2026-09-25; owner ARCH/16-LOCAL-RUNTIME-INTEROP.md)  (0 done / 10 open)
- P52-R1 — Runtime inventory `[NOT]`
- P52-R2 — Managed runtime lifecycle `[NOT]`
- P52-R3 — Status planes `[NOT]`
- P52-R4 — Agent handoff `[NOT]`
- P52-R5 — Set up a local runtime `[NOT]`
- P52-R6 — Connect a runtime `[NOT]`
- P52-R7 — Inventory surfaces `[NOT]`
- P52-R8 — FitResult `[NOT]`
- P52-R9 — Legacy path retirement `[NOT]`
- P52-R10 — Remote ComputeTargets `[NOT]`

## P53 — Installed-any Chief, ACP slash, compact handoff, subagent mix (spec §4.2.5a restated 2026-09-09 — B9/B3/F12/H32/J17; no new matrix IDs; P38 crate seams stay `[DONE]`)  (8 done / 1 open as of 2026-09-26)
- P53.9 — Swarms `[IMPLEMENTED]`

## P54 — Terminal profiles + PTY backends (H36, spec §4.5 — 2026-09-10)  (5 done / 3 open as of 2026-09-26)
- P54.5 — Automation profile (`terminal.automationProfile.<platform>` + `terminal_set_automation` + `PtyHost::resolve_automation_command` + `[PARTIAL]`
- P54.7 — Remote/CloudNode backend (H33 v1 attach) `[NOT]`
- P54.8 — Cmder / Cygwin / MSYS2 / Git Bash history notes `[IMPLEMENTED]`

## P68 — One terminal plane + shell integration + Copilot-style follow (v3.80, 2026-09-15)  (7 done / 1 open as of 2026-09-26)
- P68.7 — Windows ConPTY acceptance run `[NOT]`

## P55 — Honesty pass (2026-09-10 adversarial audit vs live source — no new matrix IDs)  (12 done / 0 open as of 2026-09-26)

## P56 — Provider catalog, Settings UI, OpenCode-free, custom inference, 429 honesty (A1/A2/A3/A6/A11 — 2026-09-10)  (8 done / 0 open as of 2026-09-26)

## P57 — Computer-use path-launch + background (E9 H3/H3a — 2026-09-10)  (5 done / 3 open as of 2026-09-26)
- P57.5 — Session 0 / missing a11y honesty (H4) `[PARTIAL]`
- P57.6 — Target-window capture in background `[PARTIAL]`
- P57.7 — Linux / macOS twins `[PARTIAL]`

## P58 — Cockpit UI stale / missing / wrong (2026-09-10 audit vs `ui/src`)  (12 done / 0 open as of 2026-09-26)

## P59 — Two-surface CUA: vision gate + Manager DAG (E9 — 2026-09-10)  (16 done / 0 open as of 2026-09-26)

## P60 — Agent Runtime: Chief / graph / Scout-Worker-Verifier / harness×model (spec §4.2.5b — 2026-09-10)  (14 done / 0 open as of 2026-09-26)

## P37 — Inspiration UI chrome (Cursor / Qoder / Ollama / TRAE / ZCode / Cowork layout reference — 2026-08-21)  (15 done / 1 open as of 2026-09-26)
- Voice VAD + STT + DJI/USB auto-send pipeline (P9.3). `[POST-V1]`

## P36 — v3.39 Kernel Contracts (named types/fields on existing rows — no new matrix IDs)  (22 done / 0 open as of 2026-09-26)

## P38 — Dynamic Chief: agent-native top brain (spec v3.45, 2026-08-23 — `primary_chief` on B9/F12/J17, no new matrix IDs)  (7 done / 0 open as of 2026-09-26)

## P39 — Performance & Footprint Queue (spec v3.46, 2026-08-23 — perf items on existing rows, no new matrix IDs)  (5 done / 0 open as of 2026-09-26)

## P40 — User-operated always-on executor node (spec H33; competitor-gap research 2026-08-23 — OpenClaw deploy pattern, source-read)  (3 done / 0 open as of 2026-09-26)

## P41 — Zed-class Rust IDE capability (spec I12; research 2026-08-23 — Zed pattern-only, Codex-app worktrees)  (4 done / 0 open as of 2026-09-26)

## P42 — M365/Google workspace connectors v2 (spec F14/F15; research 2026-08-23 — review's 365/Google depth gap)  (3 done / 0 open as of 2026-09-26)

## P43 — Long-running / detached-work task ledger (spec B7/H19; OpenClaw `tasks` pattern, research 2026-08-23 — docs.openclaw.ai/automation/tasks)  (4 done / 0 open as of 2026-09-26)

## P44 — Provider Record + Autonomy Level (spec v3.56 A11 + H34; Hermes `providers.py` + OpenCode provider-directory source-read 2026-08-26 — new matrix rows, +2)  (9 done / 0 open as of 2026-09-26)

## P45 — Micro-perf / footprint queue (perf-review re-check 2026-08-26 — SQLite/audit/UI tier-0 deltas, measurement-gated)  (0 done / 9 open as of 2026-09-26)
- P45.1 — SQLite pragma tuning on non-vault DBs (R4-gated) `[IMPLEMENTED]`
- P45.2 — `PRAGMA mmap_size` (256MB) on memory/search read-heavy DBs (R4-gated) `[IMPLEMENTED]`
- P45.3 — `PRAGMA journal_size_limit` + WAL checkpoint throttling (R4-gated) `[IMPLEMENTED]`
- P45.4 — SQLite connection pool (4 readers + 1 writer pre-opened) `[IMPLEMENTED]`
- P45.5 — Turn-boundary audit-NDJSON write batching (honesty-preserving) `[IMPLEMENTED]`
- P45.6 — `content-visibility: auto` on off-screen panels `[NOT]`
- P45.7 — Provider DNS cache (session-scoped) `[IMPLEMENTED]`
- P45.8 — Sidecar idle-kill (60s → respawn ~200ms) `[IMPLEMENTED]`
- P45.9 — UI micro-perf sweep `[IMPLEMENTED]`

## P46 — v3.57 review additions (spec I13 / H35 / J24 — new matrix rows; P46.2 doctor = V1, the rest post-v1)  (2 done / 2 open as of 2026-09-26)
- P46.1 — Knowledge → Skill Compiler (`/learn`, spec I13, post-v1) `[NOT]`
- P46.3 — Credential-provider abstraction (spec J24, post-v1) `[NOT]`

## P47 — Architecture finalization (v3.59 — the finalized-architecture decision block)  (6 done / 0 open + 1 partial as of 2026-09-26)
- P47.7 — CC Switch-class ClientCompatibility ring (post-v1 outer adapter; contract recorded, code post-v1) `[SPEC-ONLY]`

## P48 — Authorization provenance + mechanical proof layer (v3.60 — "a machine cannot manufacture human authorization")  (4 done / 0 open as of 2026-09-26)

## P49 — Work Gateway / Session Runtime (v3.64 — the missing durable-session layer; spec §4.4)  (29 done / 2 open + 2 partial as of 2026-09-26)
- P49.5 — agentcowork-sandbox crate (native-OS-first) `[PARTIAL]`
- P49.6 — Sandbox verification (SandboxReceipt) `[PARTIAL]`
- P49.19 — Remote Web/Mobile/CLI clients (post-v1)
- P49.20 — Node failover (post-v1)

## P29 — Native Sidecar Migration, Tiered (external review 2026-08-17; spec §9.1 R6, ARCH/01 §1.3)  (11 done / 0 open as of 2026-09-26)

## P30 — Competitor Batch Steal Queue (openworker · cc-switch · skales · deepseek-harness; doc 83, 2026-08-17)  (16 done / 0 open as of 2026-09-26)

## P31 — Custom Agent Builder + Simplified UI (B9; user directive 2026-08-17)  (10 done / 0 open as of 2026-09-26)

## P32 — Casual vs Power User UX Queue (doc 84; user directive 2026-08-17)  (8 done / 0 open as of 2026-09-26)

## P33 — Multi-View Right Panel + Office/PDF/Google (doc 84 + VS Code logic + LibreOffice/LOKit + Google Workspace; user directive 2026-08-17)  (8 done / 0 open as of 2026-09-26)

## P34 — Full-Fidelity Tool Surfaces (ARCH/12 v3.1; user directive 2026-08-17)  (7 done / 0 open as of 2026-09-26)

## P35 — Full Animation Wiring (design-doc motion table; user directive 2026-08-17)  (4 done / 0 open as of 2026-09-26)

## P61 — First-five-minutes + approval-relationship UX (v3.68 — user directive 2026-09-13)  (11 done / 1 open as of 2026-09-26)
- P61.12 — Non-blocking "Done — review" digest (WP4 remainder) `[PARTIAL]`

## P62 — Guard network-destination floor + protected agent-config surfaces (v3.69 — 2026-09-13)  (4 done / 1 open as of 2026-09-26)
- P62.5 — Guard-1 tool deflection & recovery nudges (shell-bias prevention) `[IMPLEMENTED]`

## P63 — External-agent model backend (v3.73 — 2026-09-14; F12/J17, no new matrix IDs)  (9 done / 3 open as of 2026-09-26)
- P63.8 — Native config-file writers (post-v1, P47.7-gated) `[NOT]`
- P63.11 — Host-bound Channel B MCP server instantiation in `acp_cmds.rs` (`session_new` `mcpServers` population) `[IMPLEMENTED]`
- P63.12 — Settings External Subagent Roster Management UI `[IMPLEMENTED]`

## P64 — Native agent plane (the `ARCH/17` freeze — spec v3.76, 2026-09-15; that document is **archived 2026-09-22** under `ARCH/archive/` by `P71.5a`; adds rows B10/B11/C14/C15/F16/I14–I17)  (5 done / 7 open as of 2026-09-26)
- P64.4 — Sub-agent execution side `[PARTIAL]`
- P64.5 — Unified native edit engine `[PARTIAL]`
- P64.6 — Risk-gated shadow preflight `[PARTIAL]`
- P64.7 — Checkpoint per mutating call + rollback UX `[IMPLEMENTED]`
- P64.8 — Validated skill distillation `[IMPLEMENTED]`
- P64.11 — Chat Collapsible Sub-boxes & CLI Stream Normalization `[PARTIALLY]`
- P64.12 — Context Passport Visual Inspection & Specialist Attribution `[IMPLEMENTED]`

## P65 — Settings Control Center (v3.77 — provider, agent, channel, schedule, extension surfaces)  (7 done / 1 open as of 2026-09-26)
- P65.8 — End-to-end settings acceptance pass `[IMPLEMENTED]`

## P66 — Windows-first runtime discovery, agent picker, session loadout, and cowork evidence (v3.78 — 2026-09-15)  (3 done / 6 open as of 2026-09-26)
- P66.1 — Windows runtime location contract `[PARTIAL]`
- — selecting them painted nothing; (4) `onboarding-modal` kept a third accent list that had drifted two presets behind the Settings picker; (5) `dat... `[PARTIAL]`
- P66.6 — Windows Office acceptance and upgrade review `[NOT]`
- P66.7 — Browser and Computer Use acceptance `[NOT]`
- P66.8 — Memory, MCP, skills, plugins, and schedules acceptance `[NOT]`
- P66.9 — Windows release acceptance matrix `[NOT]`

## P69 — Architecture thaw: ARCH/CORE authority, ownership consolidation (opened 2026-09-20)  (42 done / 89 open + 1 partial as of 2026-09-26)

### P69.A — The authority documents  (33 done / 2 open)
- P69.A28 — `DESKTOP-APP-SPEC.md` deltas `[PARTIAL]`
- P69.A30 — retire "Chief" from architecture text. `[PARTIAL]`

### P69.B — New canonical objects (contracts exist in docs; code follows)  (0 done / 16 open)
- P69.B1 — `Space` / `Project` / `Workspace` identity + migration `[IMPLEMENTED]`
- P69.B2 — `AgentBinding` as a first-class primitive `[IMPLEMENTED]`
- P69.B3 — `AgentAdapter` contract + runtime capability matrix `[IMPLEMENTED]`
- P69.B4 — `AgentBridge` `[IMPLEMENTED]`
- P69.B5 — `ContextSurface` + projection contracts `[IMPLEMENTED]`
- P69.B6 — `ContextCapsule` + `ContextPassport` `[IMPLEMENTED]`
- P69.B7 — `CapabilityPackManifest` / `CapabilitySet` / `AgentCapabilityProfile` `[IMPLEMENTED]`
- P69.B8 — `ResourceRef` + `ViewerProvider` + `ViewerRegistry` + `FileResource` `[IMPLEMENTED]`
- P69.B9 — `ModelRoute` `[IMPLEMENTED]`
- P69.B10 — context contracts `[IMPLEMENTED]`
- P69.B11 — deterministic reductions before compaction `[IMPLEMENTED]`
- P69.B12 — cache-aware summary request builder `[IMPLEMENTED]`
- P69.B13 — stable façades for capability backends `[IMPLEMENTED]`
- P69.B14 — pinned external tool schemas `[IMPLEMENTED]`
- P69.B15 — `AgentBehaviorProfile` compiler + per-adapter extension wiring [I27] `[IMPLEMENTED]`
- P69.B17 — the remaining File Workbench viewer families. `[IMPLEMENTED]`

### P69.C — Confirmed defects (verified in source; each violates a CORE invariant)  (0 done / 13 open)
- P69.C1 — the ACP permission path grants approval without consulting Guard. [I12] `[IMPLEMENTED]`
- P69.C2 — ACP `fs/*` and `terminal/*` are unhandled. [I14] `[IMPLEMENTED]`
- P69.C3 — mediated mode is not the default. [§7.5] `[IMPLEMENTED]`
- P69.C4 — a TypeScript package stores provider credentials. [I10 — most severe] `[IMPLEMENTED]`
- P69.C5 — purge the stale "everything is ticketed" wording `[IMPLEMENTED]`
- P69.C6 — verify no doc claims observability it does not have. `[IMPLEMENTED]`
- Agent sign-in is the handshake, not a global key bar. `[NOT]`
- P69.C7 — auth mode is inferred from the software license. [V5] `[IMPLEMENTED]`
- P69.C8 — registry `env` is discarded by the registry merge. [V6] `[IMPLEMENTED]`
- P69.C9 — a binary agent with a missing platform target launches bare. [V7] `[IMPLEMENTED]`
- P69.C10 — registry schema gaps. [V8] `[IMPLEMENTED]`
- P69.C11 — one auth-mode wire contract. [V9] `[IMPLEMENTED]`
- P69.C12 — wire the registry `Ask` consent surface end to end. [highest priority for the "any registry agent works" goal] `[IMPLEMENTED]`

### P69.D — Ownership consolidation (one owner per question)  (0 done / 32 open)
- P69.D1 — one `AgentRegistry` `[IMPLEMENTED]`
- P69.D2 — one `ToolRegistry` `[IMPLEMENTED]`
- P69.D3 — one authorization engine `[IMPLEMENTED]`
- P69.D4 — remove the TypeScript provider vault path `[IMPLEMENTED]`
- P69.D5 — `core-security` is reduced to a crypto/vault-support utility `[IMPLEMENTED]`
- P69.D6 — `core-tools` shrinks `[IMPLEMENTED]`
- P69.D7 — one context manager and one prompt assembler `[IMPLEMENTED]`
- P69.D8 — kill the competing conversation runtime `[IMPLEMENTED]`
- P69.D9 — one search implementation `[IMPLEMENTED]`
- P69.D10 — one retrieval/context pipeline `[IMPLEMENTED]`
- P69.D11 — one event store; one Work model. `[IMPLEMENTED]`
- P69.D12 — blueprint becomes declarative only `[IMPLEMENTED]`
- P69.D13 — MultiRun becomes a strategy over Runs `[IMPLEMENTED]`
- P69.D14 — subagents become child Work/Runs `[IMPLEMENTED]`
- P69.D15 — `core-domain` shrinks or merges; `[IMPLEMENTED]`
- P69.D16 — scheduler and workflow create Work, nothing else `[IMPLEMENTED]`
- P69.D17 — connector actions produce a canonical `EffectRequest` `[IMPLEMENTED]`
- P69.D18 — external MCP tools normalize into the canonical capability model `[IMPLEMENTED]`
- P69.D19 — `agentcowork-core` is shrunk `[SCOPED]`
- P69.D20 — services move out of the kernel `[SCOPED]`
- P69.D21 — kill the `Execution` ↔ `Work` compatibility alias; `[IMPLEMENTED]`
- P69.D22 — coordinator split `[IMPLEMENTED]`
- P69.D23 — namespace cleanup `[IMPLEMENTED]`
- P69.D24 — UI becomes projections `[IMPLEMENTED]`
- P69.D25 — `agentcowork-types` becomes the canonical schema layer. `[IMPLEMENTED]`
- P69.D26 — `agentcowork-ipc` is transport only. `[IMPLEMENTED]`
- P69.D27 — `agentcowork-engine` stays pure. `[OBSOLETE]`
- P69.D28 — `agentcowork-catalog` shrinks to metadata. `[IMPLEMENTED]`
- P69.D29 — one façade per capability service. `[IMPLEMENTED]`
- P69.D30 — `agentcowork-memory` consolidates to four classes. `[IMPLEMENTED]`
- P69.D31 — `agentcowork-cdp` is a backend under `BrowserService`, `[IMPLEMENTED]`
- P69.D32 — `agentcowork-eval` stays outside the runtime `[IMPLEMENTED]`

### P69.E — CI architecture checks (make the invariants machines' work)  (2 done / 8 open + 1 partial)
- P69.E1 — one-of-each assertions `[IMPLEMENTED]`
- P69.E2 — no privileged effect from TS `[IMPLEMENTED]`
- P69.E3 — every effect passes Guard `[IMPLEMENTED]`
- P69.E4 — no second event log, no scheduler/workflow/subagent-owned execution loop. `[IMPLEMENTED]`
- P69.E5 — no UI write to kernel state except through the Work Gateway. `[IMPLEMENTED]`
- P69.E6 — capability lockstep stays green `[IMPLEMENTED]`
- P69.E7 — no credential material in TS [I10] `[IMPLEMENTED]`
- P69.E8 — no hardcoded approval outside test paths [I12] `[IMPLEMENTED]`
- P69.E9 — prefix-stability guard [I16] `[PARTIAL]`

### P69.F — Migration order (do not start by deleting)  (0 done / 9 open)
- P69.F1 — Phase 1, foundation `[NOT]`
- P69.F2 — Phase 2, kernel extraction `[NOT]`
- P69.F3 — Phase 3, security `[NOT]`
- P69.F4 — Phase 4, agents `[NOT]`
- P69.F5 — Phase 5, capabilities `[NOT]`
- P69.F6 — Phase 6, intelligence `[NOT]`
- P69.F7 — Phase 7, UI `[NOT]`
- P69.F8 — Phase 8, cleanup `[NOT]`
- P69.F9 — architecture regression tests `[NOT]`

### P69.G — Contract sections without delivery rows (opened 2026-09-25)  (0 done / 9 open)
- P69.G1 — `ARCH/MEMORY.md` §12–§20 `[OPEN]`
- P69.G2 — `ARCH/RECOVERY.md` §10–§13 `[IN]`
- P69.G3 — `ARCH/UI.md` §9–§10 `[OPEN]`
- P69.G4 — `ARCH/DESKTOP.md` §9–§11 `[IN]`
- P69.G5 — `ARCH/04-OFFICE-ENGINE.md` §4.6–§4.8 + `ARCH/08-BROWSER-LAYER.md` §8.8 + `ARCH/15-CONNECT-STORE.md` registry `[IN]`
- P69.G6 — `ARCH/CONTEXT.md` `[OPEN]`
- P69.G7 — `ARCH/SECURITY.md` `[OPEN]`
- P69.G8 — `ARCH/WORK.md` 14-4/19-14/12-11/19-13/12-14/16-13/11-10 + `ARCH/SESSION.md` 12-4/10-6 + `ARCH/AUTOMATION.md` RTE-9/CON-3/12-5 + `ARCH/EXTE... `[OPEN]`
- P69.G9 — `ARCH/12-UI-SPEC.md` 12-14/16-13/19-8 `[OPEN]`

## P70 — v1 Release: packaging, qualification, launch (opened 2026-09-20)  (7 done / 49 open as of 2026-09-26)

### P70.A — Build and bundle  (0 done / 8 open)
- P70.A1 — bundle target matrix `[IMPLEMENTED]`
- reinstall `[IMPLEMENTED]`
- P70.A3 — per-platform native dependency audit `[IMPLEMENTED]`
- P70.A4 — app metadata + assets `[IMPLEMENTED]`
- P70.A5 — size + footprint budgets `[IMPLEMENTED]`
- P70.A6 — reproducible/CI-only builds `[IMPLEMENTED]`
- and `[IMPLEMENTED]`
- P70.A8 — schema/version stamps `[IMPLEMENTED]`

### P70.B — Signing, trust and provenance  (0 done / 7 open)
- P70.B1 — Windows code signing `[IMPLEMENTED]`
- P70.B2 — macOS signing + notarization `[OUT]`
- P70.B3 — Linux artifact integrity `[OUT]`
- P70.B4 — updater signing keypair `[IMPLEMENTED]`
- P70.B5 — SBOM + build provenance `[IMPLEMENTED]`
- P70.B6 — third-party licence compliance `[IMPLEMENTED]`
- P70.B7 — secret-leak gate on artifacts `[IMPLEMENTED]`

### P70.C — Auto-update and data migration  (0 done / 7 open)
- P70.C1 — update manifest + hosting `[IMPLEMENTED]`
- P70.C2 — channels `[IMPLEMENTED]`
- P70.C3 — staged rollout + kill switch `[IMPLEMENTED]`
- P70.C4 — update UX `[IMPLEMENTED]`
- P70.C5 — upgrade data migration `[IMPLEMENTED]`
- P70.C6 — downgrade + minimum-version policy `[IMPLEMENTED]`
- P70.C7 — rollback drill `[IMPLEMENTED]`

### P70.D — Install, first run, uninstall, support surface  (1 done / 8 open)
- P70.D1 — install layout `[IMPLEMENTED]`
- P70.D2 — first-run experience `[IMPLEMENTED]`
- P70.D3 — clean-profile boot check `[IMPLEMENTED]`
- P70.D4 — uninstall cleanliness `[IMPLEMENTED]`
- P70.D5 — OS support matrix `[IMPLEMENTED]`
- P70.D7 — non-Linux sandbox honesty `[IMPLEMENTED]`
- P70.D8 — support bundle / diagnostics `[IMPLEMENTED]`
- P70.D9 — in-app help + recovery `[IMPLEMENTED]`

### P70.E — Release qualification gates  (3 done / 9 open)
- P70.E2 — all suites green on the release commit `[RUNNABLE]`
- P70.E3 — documentation gates `[IMPLEMENTED]`
- P70.E4 — security gate suite `[RUNNABLE]`
- P70.E5 — live integration gates on real hosts `[BLOCKED]`
- P70.E6 — live-model soak `[BLOCKED]`
- P70.E7 — crash-free session soak `[RUNNABLE]`
- P70.E8 — upgrade/downgrade/rollback evidence `[BLOCKED]`
- P70.E9 — clean-machine install test `[BLOCKED]`
- P70.E10 — performance regression gate `[RUNNABLE]`

### P70.F — Distribution and launch surface  (2 done / 6 open)
- P70.F1 — published release artifacts `[IMPLEMENTED]`
- P70.F3 — download/landing surface `[IMPLEMENTED]`
- P70.F4 — release notes generation `[IMPLEMENTED]`
- P70.F5 — legal + policy documents `[IMPLEMENTED]`
- P70.F6 — telemetry posture stated `[IMPLEMENTED]`
- P70.F8 — v1 launch checklist `[IMPLEMENTED]`

### P70.G — Post-release operations  (1 done / 4 open)
- P70.G1 — rollout monitoring `[IMPLEMENTED]`
- P70.G2 — hotfix process `[IMPLEMENTED]`
- P70.G3 — patch cadence + deprecation policy `[IMPLEMENTED]`
- P70.G4 — post-v1 backlog transfer `[IMPLEMENTED]`

## P71 — External-Agent Consolidation: the v1 engine decision (ADR-0005, opened 2026-09-21)  (1 done / 38 open as of 2026-09-26)

### P71.1 — The prerequisite: delegation on the shared plane  (0 done / 2 open)
- P71.1 — `delegate.*` façade family (the blocker, land first) `[IMPLEMENTED]`
- P71.1b — façade naming reconciliation `[IMPLEMENTED]`

### P71.2 — Remove the built-in engine path  (0 done / 6 open)
- no `[IMPLEMENTED]`
- P71.2b — remove `HarnessProtocol::ModelBackend` `[IMPLEMENTED]`
- P71.2c — remove the sidecar reasoning path `[IMPLEMENTED]`
-  `[IMPLEMENTED]`
- P71.2e — archive the native loop `[IMPLEMENTED]`
- P71.2f — narrow the native tool catalogue `[IMPLEMENTED]`

### P71.3 — Re-home the duplicate runtimes (I8/I9 already require this)  (0 done / 7 open)
- `[IMPLEMENTED — unverified 2026-09-21: the executor is gone. `SubAgentRuntime` (its `HashMap` of active agents, `spawn`/
- P71.3b — `SwarmSession` → `SwarmStrategy` + `ResultReducer` `[IMPLEMENTED]`
- P71.3c — `AutomationRuntime` → Work factory/compiler `[IMPLEMENTED]`
- P71.3d — shrink `scheduler_service.rs` to a trigger plane `[IMPLEMENTED]`
- P71.3e — `MultiRun` stops being model-centric `[IMPLEMENTED]`
- P71.3f — canonical `AgentReadiness` `[IMPLEMENTED]`
- P71.3g — the Work-state contract in code (`ARCH/WORK.md` §4 vs `agentcowork-types`) `[IMPLEMENTED]`

### P71.4 — Keep as observability, not execution  (0 done / 1 open)
- P71.4 — usage and cost observability without a gateway `[IMPLEMENTED]`

### P71.5 — Vocabulary and archives  (0 done / 2 open)
- P71.5a — archive `ARCH/17-NATIVE-AGENT.md` `[IMPLEMENTED]`
- P71.5b — retire the residual "Chief" vocabulary `[IMPLEMENTED]`

### P71.6 — Make v1 work without the built-in engine  (0 done / 2 open)
- P71.6a — first run leads with agent discovery `[IMPLEMENTED]`
- P71.6b — prove the engine is optional `[IMPLEMENTED]`

### P71.7 — Post-v1 return  (0 done / 1 open)
- P71.7 — governed baseline binding (post-v1) `[NOT]`

### P71.8 — Session kinds (ADR-0006)  (0 done / 4 open)
- P71.8a — canonical `SessionKind` `[IMPLEMENTED]`
- P71.8b — trigger-created Sessions `[IMPLEMENTED]`
- P71.8c — surface non-interactive Sessions through their owner `[IMPLEMENTED]`
- P71.8d — scope resolution for non-interactive Sessions `[IMPLEMENTED]`

### P71.9 — UI for external-agent v1 (concrete surfaces, not a slogan)  (0 done / 9 open)
- every `[IMPLEMENTED]`
- P71.9b — First run leads with agent discovery. `[IMPLEMENTED]`
- P71.9c — Composer shows only the bound agent's own surface. `[IMPLEMENTED]`
- P71.9d — Delegation is visible, and configurable per agent. `[IMPLEMENTED]`
- P71.9e — Automations screen with honest run status. `[IMPLEMENTED]`
- P71.9f — Session kinds are surfaced through their owner, never as a Chat. `[IMPLEMENTED]`
- P71.9g — Agent-builder engine choices match v1. `[IMPLEMENTED]`
- P71.9h — Usage/cost is presented as observation, with an honest unknown. `[IMPLEMENTED]`
- P71.9i — Live Context Passport tool affinity steering block in `build_acp_prompt_with_passport` `[IMPLEMENTED]`

### P71.10 — Session workbench projection and resource leases (ADR-0008)  (1 done / 4 open)
- P71.11 — refuse ACP v2, never silently downgrade [ADR-0007 §4] `[PARTIAL]`
- P64.13 — cockpit honesty gaps found by rendering (2026-09-25) `[NOT]`
- P64.14 — `search_report` projection over the cascade `[NOT]`
- P71.10 — implement and qualify `SessionWorkbenchProjection`, `LensState`, and typed Work/Run-owned `ResourceLease`/generation fencing. `[PARTIALLY]`

## P72 — Dead-code retirement: delete `agentcowork-engine` (landed 2026-09-23)  (0 done / 2 open as of 2026-09-26)
- P72.1 — delete `crates/agentcowork-engine`. `[IMPLEMENTED]`
- P72.2 — retire the crate's purity gate (PURITY-2 / P69.D27). `[IMPLEMENTED]`

## SUMMARY  (0 done / 0 open as of 2026-09-26)

</details>

> **Do not cite anything below the waves as status or design.** Current status lives only in the wave tables above; current design lives in `AGENTCOWORK-SPEC.md` + `ARCH/`. The baseline snapshot in this file is dated 2026-09-26 and is re-verified per task.
