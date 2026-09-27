# Packaging and native dependencies

> **What this file is.** The P70.A3 audit: what the shipped bundle contains, what it needs on the user's
> machine, and — for every program the runtime can execute — who provides it and what happens when it is
> absent. It is the evidence behind the "Bundled runtime" claim in [`SUPPORT-MATRIX.md`](SUPPORT-MATRIX.md) §2
> and the home of the size/footprint budgets (`P70.A5`).
>
> **The rule it enforces.** A user installs the Windows installer and runs the app. They have no Rust, no
> Node, no Bun, no pnpm, and no repository checkout. Anything the app needs beyond Windows itself must either
> ship inside the installer or degrade into a stated "not available" — never into a silent failure or a
> fabricated success. `scripts/check-native-deps.mjs` fails the build when the tree gains an external program
> that is not classified below, and when the coordinator stops being shipped as a self-contained binary.
>
> **Derivation note.** The v0 edition of this file was archived with the v0→v1 rebuild (`ARCHIVE/v0/`,
> git-ignored, read-only). This edition re-derives every table from the current tree: the §2 rows cover
> exactly the `Command::new("…")` literals the gate discovers in shipping code (test modules excluded),
> and the §6 rows mirror `crates/agentcowork-core/src/store_schema.rs` row for row.

---

## 1. What ships inside the bundle

| Item | Where it comes from | Notes |
|---|---|---|
| application binary | `tauri build` (`src-tauri`) | the Rust shell; statically linked against the workspace crates |
| **coordinator sidecar** | `bun build --compile` in `packages/coordinator` | compiled to a **standalone** executable and staged as a bundle resource at `bin/coordinator.exe` (`bundle.resources` carries `bin/coordinator*`; the release workflow stages `packages/coordinator/dist/coordinator.exe` and injects it via `TAURI_CONFIG`); the Bun runtime is a *build-time* tool and is not installed on the user's machine |
| icons | `src-tauri/icons/` | `icon.ico` plus PNG sizes referenced by `bundle.icon` (`P70.A4`) |
| updater artifacts + manifest | produced with `createUpdaterArtifacts` | signed with the release private key (`TAURI_SIGNING_PRIVATE_KEY`); consumed by `tauri-plugin-updater` |
| installer metadata | `src-tauri/tauri.conf.json` | product name, publisher, identifier, category, per-user NSIS install (`installMode: currentUser`) |

**Build-time only** (never required by a user): Rust + Cargo, Node 22, pnpm, Bun.

## 2. Runtime external programs

Every program the shell or the kernel can execute is listed here. "Optional" means the app probes for it,
runs without it, and says what is unavailable; none of them gates app startup. The first column is the
classification the gate checks: every `Command::new("…")` literal in shipping code (`src-tauri/src`,
`crates/*/src`, test modules excluded) must name one of these programs.

| Program | Provided by | Needed for | If absent |
|---|---|---|---|
| `bin/coordinator[.exe]` | **the bundle** | the whole sidecar plane (agent turns, tool surface) | startup reports *missing* and tells a packaged install to reinstall / a checkout to build the sidecar (`P70.A2`) |
| `git` | host (optional) | worktrees, diff, commit, repository-backed features (`agentcowork-blueprint/src/worktree.rs`, `agentcowork-core` git queue/commit, `src-tauri/src/git_cmds.rs`) | those features report unavailable; the rest of the app is unaffected |
| `ollama` | host (optional) | local-model runtime: GGUF bind via `ollama create`, probed with `ollama --version` (`agentcowork-core/src/models/mod.rs`) | the local-model path fails closed (`ollama not on PATH`) and provider/BYOK models still work |
| `mlx_lm.server` | host (optional) | local-model runtime probe (`--help` probe in `agentcowork-core/src/models/mod.rs`) | unavailable — Apple-silicon path, not reachable in v1 scope (no macOS artifact) |
| `soffice` / `libreoffice` | host (optional, PATH lookup) | headless LibreOffice: legacy `--convert-to` (`agentcowork-office/src/legacy.rs`) and the open-clean oracle (`agentcowork-office/src/conformance.rs`); spawned via a resolved `PathBuf`, not a literal, so the gate sees it as dynamic — it is classified here anyway | `LibreOffice (soffice) is not installed or not on PATH — legacy conversion unavailable` |
| `tesseract` | host (optional) | OCR tier: availability probe (`agentcowork-storage/src/content.rs`) and the desktop OCR binary | the OCR tier reports unavailable; UIA/accessibility paths still work |
| `nvidia-smi` | host (NVIDIA driver) | GPU probe for local inference (`agentcowork-core/src/models/probe.rs`) | falls back to the CPU path |
| `bwrap` | host (Linux sandbox backend) | strong filesystem confinement (`agentcowork-guard/src/sandbox.rs`) | the sandbox reports its weaker posture (`Ambient`) instead of claiming confinement (`P70.D7`) |
| `pgrep` | host (Linux) | child-PID process probe for the RSS measurement (`agentcowork-core/src/rss_measure.rs`) | that probe reports unavailable (the `/proc` and portable paths are unaffected) |
| `reg` / `reg.exe` | **Windows** | installed-agent discovery: `App Paths` registry query (`src-tauri/src/acp_cmds.rs`) and the Windows build probe (`agentcowork-core/src/terminal.rs`) | discovery falls back to filesystem probes |
| `wsl.exe` | **Windows** | enumerating (`-l -q`) and launching Linux-native agents inside WSL (`src-tauri/src/acp_cmds.rs`) | the WSL agent path is reported unavailable |
| `bin/*` agent executables | the user's own installed agents | external-agent turns (ACP) | that agent is listed as not installed; nothing is simulated |
| `bin/*` LSP servers / connector binaries | host or the user's installs | code intelligence, connector surfaces | those surfaces report unavailable |
| `osascript`, `screencapture`, `open` | host (macOS only) | desktop automation + external-open on macOS (`agentcowork-desktop/src/platform/macos.rs`) | not reachable in v1 (no macOS artifact); the code path stays in the tree |
| `/bin/sh`, `/usr/bin/env` | host (POSIX) | test-only today: both literals live inside `mod tests` (`agentcowork-desktop/src/launch.rs`), so the gate does not count them as shipping spawns; no shipping path spawns a POSIX shell on the Windows host — WSL-hosted agents run inside their distro (see §3) | n/a (not shipped) |

The toolchain names (`node`, `npm`, `npx`, `pnpm`, `bun`, `yarn`, `cargo`, `rustc`, `rustup`, `python`,
`python3`, `tsc`, `deno`) are a **denylist**: the gate fails if the shipping paths spawn any of them. They
are build-time tools, and a runtime dependency on one would make the installer unshippable.

## 3. WSL as an agent host, not a cockpit

WSL2 is audited only as a *host for Linux-native agents* (`SUPPORT-MATRIX.md` §2). A discovered Linux path is
launched through `wsl.exe -d <distro> -- <path>`; it never enters a native Windows spawn. The cockpit itself
runs on Windows — WSL is not required to install, start or use the app.

## 4. Size and footprint budgets

Installer/installed-size budgets and the idle-RSS budget are recorded in
[`docs/packaging/budgets.json`](docs/packaging/budgets.json) and asserted by
`scripts/check-size-budget.mjs` against measurements taken from a produced bundle (`P70.A5`).

## 5. How this is verified

| Claim | Gate |
|---|---|
| every spawned program is classified above | `scripts/check-native-deps.mjs` |
| the sidecar ships compiled as a bundle resource | `scripts/check-native-deps.mjs`, `scripts/check-release-matrix.mjs` |
| installer metadata + icons + stable MSI identity | `scripts/check-app-metadata.mjs` |
| the published platform set is Windows-only | `scripts/check-release-matrix.mjs` |
| one version across installer, updater, sidecar and UI | `scripts/check-versions.mjs` |
| no key material or dev database inside a produced bundle | `scripts/check-artifact-hygiene.mjs` |
| every durable store carries a schema version and a forward-only path | `scripts/check-store-schemas.mjs` |

## 6. Durable stores and their schema versions

`P70.A8`. The registry is `crates/agentcowork-core/src/store_schema.rs`; `boot()` stamps it and refuses to
open data written by a newer build. Stores marked **in-store** stamp themselves (the vault database carries a
`schema_meta.schema_version` and `agentcowork-vault` returns `NewerSchema` rather than reading a newer schema);
**manifest** stores record their version in `<data_dir>/store-schema.json`, because rewriting an append-only
log to insert metadata would invalidate every hash after the insertion point. **Derived** stores are caches:
losing one costs a rebuild, and stamping them would misstate what an upgrade has to preserve.

| Store | Path | Version | Policy | What an upgrade must preserve |
|---|---|---|---|---|
| `vault` | `vault.db` | v8 | in-store | provider keys, key rings, token usage — forward-only, never read by an older build |
| `calendar` | `vault.db` (`ui_calendars`, `ui_calendar_events`) | v8 | in-store | user calendars and events, inside the same encrypted database |
| `audit` | `audit.ndjson` | v1 | manifest | the append-only trail: sequence continuity and the Merkle chain |
| `memory` | `memory.json` | v1 | manifest | warm set, core facts, avoidance records — user-authored content |
| `work_journal` | `work/events.jsonl` | v1 | manifest | the Work/event journal; replay stays fail-closed on duplicates |
| `checkpoints` | `workspace/*/*/bp.json` | v1 | manifest | plan + step checkpoints that recovery reads to classify interrupted work |
| `scheduler` | `scheduler.json` | v1 | manifest | the automations the user created |
| `installed_agents` | `agents/*/installed.json` | v1 | manifest | which external agents the user installed and where they run |
| `tasks` | `tasks.json` | v1 | manifest | the user's task list |
| `catalog_observations` | `provider-observations.json` | v1 | manifest | the last live probe per provider (routing evidence) |
| `cua_graph` | `dependency_graph.json` | v1 | manifest | the computer-use dependency graph the replan path reads |
| `cua_replan_log` | `replan_log.jsonl` | v1 | manifest | replan history, as user-openable diagnostic evidence |
| `plan_cache` | `plans.db` | v1 | derived | nothing — a miss rebuilds the plan |
| `repo_cache` | `repo_cache.db` | v1 | derived | nothing — a stale index falls back to a full rebuild |
| `execution_kernel_checkpoint` | `work/execution-kernel.checkpoint.json` | v1 | derived | nothing — accepted only after validating against the replayed Work events, else rebuilt |
| `acpx_sessions` | `acpx-sessions.json` | v1 | manifest | the named acpx driver sessions the user resumes by name (P51.19) |
| `update_channel` | `update_channel.json` | v1 | manifest | the user reverts to the stable channel — never a broken state (P70.C2) |
| `spool` | `spool/*.blob` | v1 | derived | nothing — the content-addressed tool-output spool: the file name *is* the SHA-256 of its contents, so there is no inner schema to stamp; bounded retention (`SPOOL_RETENTION_DAYS`, `SPOOL_MAX_TOTAL_BYTES`) reclaims it, and the transcript's own tool result already covers any reclaimed drilldown |

A store that already holds data but has no recorded version is **adopted** at the current version and marked
as such in the manifest: the honest statement is "written before stamps existed", not "already migrated".
A version bump that changes the on-disk shape must land a forward-only migration in the owning crate in the
same change, and the registry row moves with it.

## 7. Where this file is enforced

`scripts/check-native-deps.mjs` (programs and toolchains), `scripts/check-app-metadata.mjs` (installer
identity and assets), `scripts/check-release-matrix.mjs` (the published platform set and provenance),
`scripts/check-size-budget.mjs` (budgets, at release time), `scripts/check-store-schemas.mjs` (this store
table) — all wired into `.github/workflows/ci.yml`, and the size/footprint gate additionally into the release
job where the bundle actually exists.
