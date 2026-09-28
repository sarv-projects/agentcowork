# 42 — Evidence Map

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P4). How claims get evidence, how the implementation will be accepted, and the consolidated **code-phase fix register**. This doc is the bridge between the frozen docs and the future code phase.
> **P7 pass (2026-09-26):** line-checked; cross-references verified.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Rule:** no claim ships without evidence; a passing unit test proves that behavior only (never “it works”); readiness claims require real platform acceptance records.

> **DEC-055 documentation audit (2026-09-28; historical snapshot before later amendments):** A repository-wide Markdown structural scan covered all 71 non-archived Markdown files (H1, code-fence balance, trailing whitespace); the one trailing-space finding in the UI baseline was corrected. The then-current active requirement headings/matrix rows matched 323/323 with unique IDs. `node scripts/check-doc-sync.mjs` was already blocked before content checking because it opens removed `DESKTOP-APP-SPEC.md`; `TODO.md` `TASK-PROD-005` owns that repair. These checks establish structure/traceability, not semantic proof of every statement or implementation quality.
> **DEC-058/059/061 documentation update (2026-09-28):** The active requirement registry/matrix contain 343 unique rows across 31 domains. `TASK-SEARCH-003`, `TASK-UXQ-010`, `TASK-OBS-001…010`, `TASK-PROD-007`, `TASK-ECO-003` and `TASK-PROV-003` are explicit in W6 and point to acceptance slots, owner sections, code paths and pinned source paths where applicable. MCP SDK adoption remains an architecture target until custom-transport parity passes. Machine Observer cases `TC-040…050` and flows `FLOW-048/049`, plus candidate-comparison `TC-052`/`FLOW-050`, remain design oracles, not implementation evidence. First System Workbench use or an unconsented user/agent request presents in-app consent before sampling; agents cannot grant it and Work sharing is separately scoped. NSIS remains the no-admin per-user consumer route, while the pinned Tauri WiX MSI is machine-scope/admin-managed. Installer UAC, product consent and optional operation-level UAC are distinct; real package qualification remains pending.
> **Current documentation-gate evidence (2026-09-28):** `node scripts/check-doc-sync.mjs` passes against the live authority set: 343 requirement headings, 343 matrix rows, and 181 referenced task IDs; active TODO links to `ARCH/08` resolve to the first referenced requirement heading. `node scripts/check-doc-refs.mjs` also passes with zero unresolved references. This validates structural traceability only; it does not establish semantic completeness, implementation, or product acceptance.

## 1. Evidence rules (recap)

- **External claims:** `path:line` for clones, URL for primary docs; confidence H/M/L; `[inference]` labeled; `UNVERIFIED` allowed only temporarily and must say what would verify it.
- **Lost provenance:** the original `/tmp` recon (reports 01–29) was destroyed by a temp cleanup; v0 comparative work survives in `REPO-COMPARE/**` + `BRIEFS/**`. v1 research lanes re-verified key claims from fresh clones with pinned HEADs recorded in each deliverable. Never cite the lost reports.
- **Windows-first acceptance:** a mock, browser preview, static catalog entry, or unit-only result is **not** acceptance evidence for readiness claims (repo policy, carried into v1).

## 2. Evidence index (v1)

| Source | What it proves |
|---|---|
| `ARCHIVE/v1-research/memory.md` (406 lines) | Memory mechanisms + minimal design; mem0/Graphiti/Letta/NOOA/claude-mem/Codex/Claude Code evidence |
| `ARCHIVE/v1-research/agent-harness-verification.md` (717 lines) | Codex/Grok/OpenCode/DeepSeek claims (8 VERIFIED / 3 PARTIAL / 0 WRONG) + corrections; pinned clone HEADs |
| `ARCHIVE/v1-research/ui-architecture-evidence.md` (1,710 lines) | Current theme/UI inventory + competitor chat rendering + shell/composer proposals |
| `ARCHIVE/v1-research/mcp-provider-verification.md` (641 lines) | MCP spec state (2026-07-28), dual-era requirements, clone implementations, SDK state |
| `ARCHIVE/v1-research/world-model-verification.md` (359 lines) | UIA reality, MFT/USN identity, watcher overflow semantics, browser-world patterns |
| `ARCHIVE/v1-research/workflow-engine-verification.md` (690 lines) | Temporal/n8n/Copilot Studio claims, trigger taxonomy, minimal durable design |
| `ARCHIVE/v1-research/office-runtime-verification.md` (448 lines) | OfficeCLI/GenOffice claims, format-engine reality, resident design, op sets |
| `REPO-COMPARE/**` | 190-item register (count corrected in `DISPOSITION.md` §a), dispositions, license ledger, briefs, clone2 (55 clones) · clone3 (2) |
| In-repo code (`crates/**`, `ui/**`, `src-tauri/**`) | Read-only facts about the frozen baseline (bugs, seams, existing primitives) |

## 3. Acceptance mapping (implementation phase)

Each doc’s claims map to a verification class; tests are sized to the claim. (Module docs carry the specific gates; this table is the map.)

| Doc | Primary acceptance evidence |
|---|---|
| `10` Kernel | Unit tests: ids, error taxonomy, config layering, migrations (forward-only/idempotent). |
| `11` Work | Lifecycle tests; kill/restart resume; lane limits; budget pause+surface; queue rebuild from log. |
| `12` Trust | Fail-closed tests; ticket lifecycle (issue/expire/revoke/epoch); vault isolation; denial + audit coverage; rate-limit tests. |
| `13` Capability | Resolver/failover/guidance tests; epoch invalidation; census gate (unique ids, coverage). |
| `14` Providers | Adapter conformance suite per class; MCP dual-era detection tests; egress enforcement; health/epochs. |
| `15` Agent plane | Binding capability-negotiation and honest unsupported outcomes; every Core-mediated call takes the same Guard path; native effects retain separate provenance; host delegation receipts and adapter parity where supported. |
| `16` Context | Budget/feasibility tests; overflow recovery; cache-stability checks; projection filtering. |
| `17` Memory | The §13 eval suite: write quality, update/conflict, recall golden set, budget honesty, isolation, lifecycle, ops. |
| `18` Models | Router matrix tests; local discovery; reasoning mapping; usage accounting. |
| `19` Runtime | Process/sandbox tests; helper consent; detached rehydration; **Windows sandbox acceptance record**. |
| `20` Workflow | Durability suite: kill/restart, one logical run per deduped occurrence, fenced trigger-owner handoff, ambiguous effect reconciliation, misfire policies, approval flows and version pinning. |
| `21` World | Collector tests + consent records; overflow/rescan; **Windows acceptance matrix** (real apps: Notepad/Explorer/Office/UWP/Electron). |
| `22` Office | Round-trip/fidelity suite per format; resident crash tests; redact extraction check; op-registry docs-sync gate. |
| `23` Browser | Task suite (snapshot/act/verify); ref invalidation; adapter fallback; no-evasion checks. |
| `24` Computer-Use | Ladder fallback tests; per-call budget/isolation; vision sizing caps; real-app matrices on Windows. |
| `25` Files | Identity tests (incl. incarnation); lease conflicts; watcher overflow; cursor recovery. |
| `26` Code | Incremental index tests; LSP fallback; worktree provision/merge; test-runner wrappers. |
| `27` Search | Scope/abstention tests; freshness flags; p95 targets. |
| `28` Comms | Connector auth/reconnect; send approval + no-duplicate-send; event ingestion. |
| `29` Artifacts | Version immutability; GC vs pin; replay; gateway permissions. |
| `30` Events | Delivery/idempotency/replay; retention; poison isolation. |
| `31` Skills/Plugins | Activation relevance; review gate; crash-loop auto-disable; version skew. |
| `32` Channels | Gateway 7-item projection tests; ACP mapping; approval routing. |
| `34` Verification | Risk→depth matrix tests; unverified-state surfacing; repair paths. |
| `35`/`36` Mission | Contract/plan versioning, Work replacement, drift invalidation, independent outcome checks and stop decisions; `TC-027…036`. |
| `46` Ecosystem | Read-only discovery, supported overlay negotiation, native/Core evidence separation, extension scope/collision/revocation and heterogeneous child handoff. |
| `48` Experience | First-run comprehension, composer controls, responsive/accessible renderer, universal file-handler/fallback path with no file-selection auto-run, Workbench edit/takeover, Library retrieval, truthful Settings and child-state display; `TC-001…038`, `TC-049`, `TC-051`. |
| `46`/`48` candidate comparison | Same frozen task baseline, actual worker capability/policy loadouts, enforced write isolation, bounded partial failures, criterion-level evidence/cost, safe selection/integration and native-vs-Core effect honesty; `TC-052`. |
| `51` Machine Observer | Typed snapshots/provider gaps, explicit local consent and exact-operation elevation, bounded history/query, least-rights process diagnostics, storage/GPU/WSL limits, standalone IPC and approachable System Workbench; `TC-040…050`. |
| `49` Test cases | Pinned fixture/scorer/run metadata and raw result bundles, including unsupported and failed runs; no pass claim until executed. |
| `40`/`41` | Flow e2e scripts + edge-case regression tests. |
| `AGENTCOWORK-SPEC` / `AGENTCOWORK-UI` | Product acceptance criteria + UI checks (defined by the product + UI docs). |

## 4. Code-phase fix register (re-verify each before fixing)

**Standing rule:** v1 is frozen; each item is re-verified against the current tree before its fix lands (some v0 findings predate the freeze).

**How a landed row reads (code phase, from 2026-09-26).** A row keeps its original finding and adds, in place: what the re-verification actually found (**narrower**, **wider**, or **half wrong** — a wrong finding is corrected, not quietly dropped), the landed code anchors as `path:line`, and an explicit verification status. `landed` means the code is in the tree; it never means *verified on the target platform*. Where the only evidence is a verdict-shape test on a non-target host or a type check, the row says so and names the acceptance record still owed.

| FIX | Item | Source / evidence |
|---|---|---|
| FIX-01 | Connector OAuth token custody → vault — **landed, with a narrower finding than this row claimed**: the command layer (`src-tauri/src/oauth_cmds.rs`) and the TypeScript side (`packages/core-connectors/src/`) were already compliant and are unchanged. The real defect was a **read-value** token API in the Rust connector layer — a connector held the access token as a `String` field (clonable, serializable, outliving the call) — replaced by the use-style `TokenSource` over a `VaultTokenRef` (`core/src/connectors/mod.rs:59-90`); the hub is routing metadata that serializes with no credential (`core/src/connector_hub.rs:330-362`), asserted on the struct itself (`core/src/connectors/gmail.rs:691-698`) | `MASTER-COMPARISON` SEC-1 + landed code |
| FIX-02 | Control-plane rate limiting for `nativeCall`/Guard — **landed**: two composing token-bucket tiers (one global, one per `(caller, command)`) in `agentcowork-guard/src/ratelimit.rs:266-347`, with an LRU-capped bucket map + idle-bucket TTL (`:370-390`), a poisoned lock as a denial (`:272-281`) and a caller-supplied monotonic clock. The shell gate wraps the `invoke_handler` (`src-tauri/src/lib.rs:845`, gate at `:82-149`) so a refused call reaches no command body, state lock or disk; the kernel tool path checks before a ticket is minted (`core/src/tools.rs:1176-1192`). The refusal is the canonical `Unavailable` code + a `rate_limited` reason + `retryAfterMs` — **no new error code**, because the `10` §3 taxonomy is canonical. Shape specified in `ARCH/12-TRUST.md` §1.1; the concrete numbers are a product knob | SEC-2 + landed code |
| FIX-03 | ACP permission bridge: once/always/reject + diff previews, fail-closed | SEC-3 |
| FIX-04 | UI-blob secret scan | SEC-21 |
| FIX-05 | `skills_uninstall` arbitrary recursive deletion (`skill_store.rs:442`) | v0 audit (re-verify path) |
| FIX-06 | Unticketed `fs_*`/`terminal_run` with false `AgentTicket` provenance | v0 audit (re-verify) |
| FIX-07 | Hardcoded governance badge `SelfContained { channel_b: true }` (`acp_cmds.rs:1676`) — the code comment argues it is correct post-ADR-0005; **annotated close pending a `DEC`/evidence note, not a code patch** | v0 audit + `code-state-inventory` §7 item 5 / §8 item 6 |
| FIX-08 | `tool/commit` emitted no receipt — the register entry was **half wrong**: a live driver existed and still does; the actual gap was that a committed mutating effect produced **no receipt** for its externally visible effect. **Landed**: the receipt is built *before* the response, so an effect cannot complete without one (`core/src/tools.rs:1643-1682`, builder at `:1694-1740`); an unobserved or failed effect is recorded as a `has_gap` with a reason, never a clean success (`:1718-1727`); it cites the authorizing ticket and is replayable as evidence via `tool/receipt` (`:1213-1220`), never re-execution | v0 audit (re-verify) + landed code |
| FIX-09 | Netfloor bypasses — re-verified 2026-09-26: **none** of the sites this row assumed were pre-checked actually were (the `tools.rs` comment claimed a prior check that did not exist). **Landed**: the single entry point `netfloor::preflight_url` (`crates/agentcowork-guard/src/netfloor.rs:310-323`, failing closed on an unparseable URL, a non-`http(s)` scheme or a hostless URL) now gates each of them at the socket — `core/src/tools.rs:2536-2549` (download) and `:2966-2972` (search), `core/src/messaging.rs:42-46` (webhook), `core/src/search_config.rs:105-106` (instance feed), `core/src/models/probe.rs:127-138` (the endpoint probe that had accepted a link-local `http://169.254.169.254/` "local runtime" base), `core/src/models/hf.rs:110-114`, `core/src/challenge.rs:174-184` (BYO solver), `core/src/models/mod.rs:252-258`, and the vault's custody egress `vault/src/oauth.rs:1030` + `:1060` through `egress_preflight` (`:1094-1100`), covered by `vault/src/oauth_tests.rs:767-783`. No site falls back to a direct client | v0 audit + `code-state-inventory` §7 item 4 + landed code |
| FIX-10 | Windows file identity: `walk.rs:131-157` zeroes dev/ino → corrupts dedup (`dedup.rs:106-118`) | `world-model-verification.md` §3 |
| FIX-11 | `usn_winapi.rs` unwired — wire as W1 delta source | same |
| FIX-12 | MCP remote client sends no `_meta`/modern headers (`remote.rs`) | `mcp-provider-verification.md` |
| FIX-13 | MCP façade missing `server/discover` + `Mcp-Method`/`Mcp-Name` validation | same |
| FIX-14 | Office resident/lease missing (commit/snapshot primitives exist); op-log replay not yet implemented (`22` §4 designs it; EDGE-050) | `office-runtime-verification.md` §4 |
| FIX-15 | PDF “redact” annotates instead of removing content (P0) | v0 audit + `office-runtime-verification.md` |
| FIX-16 | fsync before atomic swap in the Office commit path — the XLSX save path is the gap (`src-tauri/src/xlsx_cmds.rs:301-312` `atomic_write` = write + rename, no fsync); the docx/PDF paths already use `agentcowork_office::write_atomic` (fsync) | `office-runtime-verification.md` §1.A |
| FIX-17 | UIA collector hardening (AutomationId-as-hint, UIAccess limits, CDP for browser) | `world-model-verification.md` §2 |
| FIX-18 | WGC readiness verification for window capture — **the compositor step of the readiness chain is landed and no longer unprobed**: `Win32_Graphics_Dwm` is enabled in the crate manifest (`crates/agentcowork-desktop/Cargo.toml:29`), the host readiness step calls `DwmIsCompositionEnabled` (`crates/agentcowork-desktop/src/platform/win.rs:1036`) and maps it through `compositor_verdict` (`:1049-1063`) to `CaptureCheck::CompositorRunning` or the typed `CaptureFault::NoCompositor` with its guidance (`src/capture.rs:194`) — a *failing* call is also `NoCompositor`, deliberately, so a capture that would come back black is never attempted. **The verdict shape is verified on every host** by `crates/agentcowork-desktop/tests/acceptance_dwm_compositor.rs`; the raw `DwmIsCompositionEnabled` call is Windows-only and unit-tested behind `#[cfg(windows)]` (`src/platform/win.rs:1911-1940`), so **the Windows path is type-checked, not runtime-verified** — closing this row still needs the Windows acceptance record (OQ-WM-7) | `world-model-verification.md` §6 (OQ-WM-7) + landed code |

## 5. Pass gates

> **DEC-054/055/056/057/058 amendment:** New `REQ-MISSION-*`, `REQ-ECO-*`, `REQ-LEARN-*`, `REQ-UXQ-*`, `REQ-OBS-*` and the exact-version search amendment are implementation debt, not verified features. Acceptance evidence must demonstrate native config preservation, real Channel-B identity, heterogeneous child isolation, crash/session replacement, contract impact invalidation, independent outcome checks, workflow wait reconciliation, progressive control, consent-before-sampling, UAC denial fallback and clean standalone Observer extraction. `45` is source evidence for design, not product acceptance evidence. `TODO.md` W6 now owns the linked implementation tasks; none is complete based on documentation alone.

- **v1 freeze conditions (P8):** every doc passes `00-INDEX` §5; the interop matrix is updated; the fix register is owned by the code phase; no `UNVERIFIED` claim remains silent.
- **Post-freeze:** the code phase re-verifies FIX-01…18, then implements per `TODO.md` (W0–W4); the register's historical sequencing sketch is `MASTER-COMPARISON §5` (Wave 0 P0 → Wave 1 floors → Wave 2 context/token → Wave 3 engine hosting → Wave 4 work/scheduler → Wave 5 polish).

## 6. Open questions (`OQ-EVID-*`)

1. Where acceptance fixtures live (test corpus ownership) once code starts.
2. Which Windows acceptance matrix defines “ready” per domain (apps + flows list).
3. Whether the fix register becomes `TODO.md` items directly or a separate tracker first.

## 7. Evidence

All v1 lane deliverables (§2) · `REPO-COMPARE/MASTER-COMPARISON.md` §5 (sequencing) · `ARCH/22-OFFICE.md` §10, `ARCH/14-PROVIDERS.md` §4, `ARCH/21-WORLD-MODEL.md` §10, `ARCH/25-FILES.md` §2 (fix items) · repo audit notes as marked (re-verify before fixing).
