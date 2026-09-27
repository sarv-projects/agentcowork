# Install layout and recovery (P70.D1 · D9)

This document is the published statement of **what gets written where** on
install and first run, and **how a user recovers** from the five named
failure classes. `SUPPORT-MATRIX.md` owns *which platforms ship*; this file
owns *what the shipped install touches*.

## 1. Install model (D1)

**Per-user, per-machine-none.** AgentCowork installs for the current Windows
user only:

| Setting | Value | Where |
|---|---|---|
| NSIS `installMode` | `currentUser` | `src-tauri/tauri.conf.json` → `bundle.windows.nsis` |
| MSI scope | per-user (no `perMachine` scope declared) | `bundle.windows.msi` |
| MSI `upgradeCode` | pinned (bundler-derived UUIDv5) | `bundle.windows.msi.upgradeCode` |
| Elevated install | never required | no `perMachine`, no UAC elevation prompts |
| Shortcuts | Start-menu + desktop, current user | installer defaults |

### Paths (Windows)

| What | Where | Written when |
|---|---|---|
| Binaries + resources | `%LOCALAPPDATA%\AgentCowork\` (installer) | install / update |
| **User data directory** | `%USERPROFILE%\.agentcowork\` (override: `AGENTCOWORK_HOME`; legacy `%USERPROFILE%\.everyaios\` and `EVERYAIOS_*` are read as a fallback and migrated once by Core — DEC-053) | first run |
| Vault (`vault.db`) | `<data>\vault.db` | first unlock |
| Audit ledger | `<data>\audit.ndjson` | boot |
| Store manifest | `<data>\store-schema.json` | boot (`store_schema::boot()`) |
| Update channel | `<data>\update_channel.json` | channel change |
| Agent workspaces | `<data>\workspace\<id>\` | per-task |
| Model cache | `<data>\models\` | first local-model download |
| Coordinator sidecar | `<install>\bin\coordinator.exe` | install (bundled resource) |

The registry is touched only by the uninstaller's own registration (Add/Remove
Programs). The app itself registers **no** services, **no** scheduled tasks,
**no** autostart entries and **no** `PATH` edits — the MRP-registry check in
`scripts/check-diagnostics-surface.mjs` keeps this statement honest in the docs.
(`scripts/check-artifact-hygiene.mjs` is a different gate: it scans *produced*
bundles for secret leaks at packaging time, not this contract.)

## 2. Uninstall & data (D4 contract)

Uninstalling removes the binaries **and leaves the data directory** — vault,
sessions, memory and audit survive a reinstall. The explicit **remove-all**
path lives in **Settings → Diagnostics → Remove all data** (typed `DELETE`
confirmation, Tauri command `data_remove_all`): it deletes `<data>` wholesale,
re-creates the directory and re-opens a fresh audit ledger so the running app
stays coherent. Because nothing outside `<data>` was written, nothing can be
orphaned.

## 3. Recovery playbook (D9)

The five failure classes, with the recovery a user can actually perform:

| # | Failure | What the user sees | Recovery |
|---|---|---|---|
| 1 | **Failed vault unlock** (forgotten passphrase / lost keyfile) | unlock screen rejects; doctor shows Vault ✕ | If a keyfile exists, restore it; if the passphrase is lost, the vault is **unreadable by design** (no backdoor). Delete `vault.db` (Settings → Diagnostics → Remove all data is the sledgehammer; deleting `vault.db` alone keeps sessions/memory) and re-provision keys. |
| 2 | **Corrupt database** (vault or a manifest store) | boot error naming the store, or `ManifestCorrupt` refusal naming the file | The manifest is *reported, never overwritten* — fix or remove the named file. For a corrupt non-vault store, delete that one file; it re-stamps on next boot. For a corrupt vault, see #1. |
| 3 | **Missing sidecar** (`coordinator` binary absent) | doctor shows Sidecar ✕ "the install is broken or incomplete — reinstall" | Reinstall the app (the sidecar is a bundled resource). Dev: `pnpm --filter @agentcowork/coordinator build` or set `AGENTCOWORK_COORDINATOR_BIN`. |
| 4 | **Dead agent runtime** (an ACP agent crashes / hangs mid-turn) | turn ends with the agent's error; agent row shows not-ready | `acp_shutdown` / kill the child; re-launch from the agent tab. Persistent failure: reinstall that agent (`acp_install`) — app data is untouched. |
| 5 | **Half-applied migration** (boot died between store writes) | next boot refuses: `NewerThanApp` naming the store, or a re-run of `boot()` re-stamps cleanly | A newer-stamped store is refused *before any write* (nothing is half-migrated); follow the refused store's note in Settings → Doctor / `docs/updating.md` §6. If a manifest store was stamped but its writer died, the store is *adopted* (flagged), not claimed migrated. |

The doctor surface (Settings → Doctor, or `agentcowork-core doctor`) is the first
stop for all five: it names the broken subsystem and the remedy.
