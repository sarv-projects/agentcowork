# AgentCowork v1 — support matrix

> **DEC-054/055 distinction:** This matrix records the currently planned/qualified **release artifact**, not a capability ceiling for the final architecture. Remote/cloud and additional desktop platforms remain target executor/surface options subject to implementation and acceptance. A discovered external agent keeps native credential custody and policy; the Rust vault claim below covers Core-managed provider credentials only.

> **What this file is.** The published statement of which platforms v1 ships for, what is verified on each, and
> what is explicitly **not** in v1. It is the product-facing half of the platform decision recorded in
> [`AGENTCOWORK-SPEC.md`](AGENTCOWORK-SPEC.md) §1 (v1.0 scope: Windows-first desktop) and
> [`ARCH/03-HLD.md`](ARCH/03-HLD.md) §6 (scoping model); the delivery checklist lives in [`TODO.md`](TODO.md)
> (`P70.D5`, `P70.D6`) and the installer/updater contract in [`docs/install-layout.md`](docs/install-layout.md)
> and [`docs/updating.md`](docs/updating.md).
>
> **The honesty rule that governs it.** A platform is listed as supported only when a recorded
> acceptance pass exists for the artifact that ships. "We wrote the code for it" is not a support claim,
> and this matrix is written on a Linux host: no Windows acceptance record can be produced here, so none
> is claimed — Windows is the v1 target and remains `not yet qualified` (`P70.E8`, currently `BLOCKED`
> in [`TODO.md`](TODO.md)).
>
> **Derivation note.** The v0 edition of this file was archived with the v0→v1 rebuild (`ARCHIVE/v0/`,
> git-ignored, read-only). This edition re-derives every claim from the current tree (release workflow
> matrix, `src-tauri/tauri.conf.json`, the sandbox and diagnostics sources) and narrows what it cannot
> evidence: OS-version rows the v0 edition named without in-tree evidence are folded back into one
> Windows target row.

---

## 1. v1 scope — decided 2026-09-22, re-confirmed by the v1 rebuild

**v1 ships for Windows. The Linux half ships as WSL. macOS and native Linux desktop are out of v1 scope.**

This is an evidence decision, not a preference. Desktop control is the one capability whose correctness
depends on the host OS, and Windows is where the product will run, so Windows is where the acceptance work
has to happen (`P70.D6`, `P70.E8`); WSL is how Linux-native agents stay reachable from that same machine
without a Linux desktop build.

Out of scope means **no artifact is published and no acceptance is claimed** — not that the code was deleted.
The Linux and macOS code paths remain in the tree (they are what the local verification harness and CI run on),
and returning them to scope later is a matrix change plus an acceptance pass, not a rewrite.

The published Windows artifact pair is `.msi` (WiX) + `.exe` (NSIS): the release workflow builds exactly
`x86_64-pc-windows-msvc` + `aarch64-pc-windows-msvc` on `windows-latest` with `--bundles nsis,msi`, and no
other workflow publishes (`P70.A1`, asserted by `scripts/check-release-matrix.mjs`).

## 2. Platform matrix

| Platform | v1 artifact | Cockpit | Desktop control | Terminal | Status |
|---|---|---|---|---|---|
| **Windows** (x64, arm64 — the two msvc targets the release matrix builds) | `.msi` (WiX) + `.exe` (NSIS) — target, not yet qualified | v1 target | UIA + Graphics Capture path (target) | ConPTY path (target) | **v1 target** — acceptance in progress (`P70.D6`, `P70.E8`); no acceptance record exists yet, and none can be produced from a Linux host |
| **WSL2** | none — supported **host for agents** | n/a (the cockpit is the Windows app) | n/a | the Windows terminal plane | **supported**: Linux-native agents and their ACP entrypoints run inside the distro; a discovered Linux path is launched through `wsl.exe -d <distro> -- <path>` and never enters a native Windows spawn |
| macOS (Apple silicon / Intel) | **none** | — | not claimed | — | **out of v1 scope** |
| Native Linux desktop (any distro) | **none** | — | verified on the development host only | — | **out of v1 scope** — the verification host, not a shipped platform |

### Windows specifics

- **Install mode:** per-user install, no elevation required (NSIS `installMode: currentUser` in
  `src-tauri/tauri.conf.json`, asserted by `scripts/check-release-matrix.mjs`).
- **Bundled runtime:** the coordinator sidecar ships as an application resource (compiled **standalone** via
  `bun build --compile`; see [`PACKAGING.md`](PACKAGING.md) §2). Rust, Node, Bun and pnpm are
  **build-time** tools and are never required on a user's machine (`P70.A3`).
- **Agents:** any installed ACP agent runs as a normal user process; WSL-hosted agents are launched through
  their distro. Core-managed provider credentials live only in the Rust vault — never in the sidecar; a discovered agent retains its own native credential custody (DEC-054).
- **Distro coverage (WSL):** in-tree examples use the Ubuntu LTS class (settings fixtures, terminal tests);
  distro-version coverage beyond "a WSL2 distro the user has installed" is unqualified and is not claimed.
- **Sandbox posture:** **Ambient** on Windows. The only confined backend in the tree is the Linux `bwrap`
  backend (`crates/agentcowork-guard/src/sandbox.rs`); there is no Job-Object or restricted-token sandbox
  backend, so Settings → Diagnostics reports "this platform has no native sandbox backend yet (`P49.5`)"
  (`src-tauri/src/diagnostics_cmds.rs`) and third-party MCP servers run ticket-gated, audited and
  net-floored but **unconfined** on Windows. The Linux `bwrap` backend is the only confined path today
  (`P70.D7`).

## 3. What is explicitly not claimed

- **Windows desktop control is not yet verified.** No acceptance pass on a real Windows host exists, so no
  surface may describe the Windows UIA, Graphics Capture, or ConPTY paths as working.
- **No release candidate is signed off.** `P70.E8` (upgrade/downgrade/rollback evidence) is `BLOCKED` in
  [`TODO.md`](TODO.md); until the Windows acceptance record exists the matrix above describes a
  Windows-first target, not a completed Windows qualification.
- **No macOS signature, notarization or artifact** exists, and none is planned for v1.
- **No Linux package** (`.deb`, `.rpm`, `.AppImage`, Flatpak, AUR) is published for v1.
- **No file associations and no URL scheme are registered.** `src-tauri/tauri.conf.json` declares neither,
  so documents open through the app's own file-open path; registering a double-click target would route
  files to a process that cannot receive them (`P70.A4`).

## 4. Data and upgrade policy

- User data lives in a per-user, home-relative data directory (the DEC-053 resolver: `AGENTCOWORK_HOME` →
  `~/.agentcowork` if it exists → legacy `EVERYAIOS_HOME` → `~/.everyaios` → else `~/.agentcowork`) and
  survives uninstall by default, with an explicit "remove all data" path (`data_remove_all`;
  see [`docs/install-layout.md`](docs/install-layout.md) §2).
- Every durable store carries a schema version and a forward-only migration path (`P70.A8`; the table is
  [`PACKAGING.md`](PACKAGING.md) §6). A build refuses to open data written by a newer schema rather than
  guessing (`P70.C6`; see [`docs/updating.md`](docs/updating.md) §6).
- Upgrade over a previous v1 build must preserve the vault, Work/event log, checkpoints, memory, calendar,
  automations and audit-chain validity; the evidence for that is outstanding (`P70.E8`), and it is recorded
  before release, not assumed.

## 5. Where this decision is enforced

| Concern | Where |
|---|---|
| Bundle targets and Windows installer settings | `src-tauri/tauri.conf.json` (`bundle.targets`, `bundle.windows`) |
| Published artifact pair and Windows-only matrix | `.github/workflows/release.yml` (asserted by `scripts/check-release-matrix.mjs`) |
| Platform scope, host evidence rule | this file; [`ARCH/03-HLD.md`](ARCH/03-HLD.md) §6 |
| Native-dependency and bundled-runtime audit | [`PACKAGING.md`](PACKAGING.md) §2 (asserted by `scripts/check-native-deps.mjs`) |
| Durable-store versions and the forward-only path | [`PACKAGING.md`](PACKAGING.md) §6 (asserted by `scripts/check-store-schemas.mjs`) |
| Release gates that must pass on the shipped platform | [`TODO.md`](TODO.md) `P70.E` |
