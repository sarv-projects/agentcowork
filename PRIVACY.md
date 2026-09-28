# Privacy statement (P70.F5 · F6)

**Local storage is the default in the current build. There is no telemetry sender in this build.** A configured remote model, self-contained agent, connected app or future remote/cloud executor can process data within its own declared scope; inspect the binding and destination before use (DEC-054).

## What leaves the machine

| Destination | What | When | Can it be off? |
|---|---|---|---|
| The model provider **you** configured (BYOK) or a local runtime | your prompts, the context you attached, and tool results — to the endpoint you chose | only when you send a turn | yes — disconnect the provider, or use a local model |
| The external agent **you** installed | the same turn content, over ACP | only during a turn | yes — no agent bound, no turn |
| Pages **you** ask an agent to read | the URL, via your browser/Chrome profile | only for that read | yes — don't ask for it; `netfloor` blocks anything the policy denies |
| Update checks | the channel + target + current version, to the release endpoint | at boot and every 4h | yes — the check is a plain manifest GET; set the channel and read `docs/updating.md` §3 for the kill-switch |
| Agent-native tools and extensions | Data they access under that agent's policy; some agents may contact their own services | Only while that external agent runs | Disable or change the agent; Core cannot attest to all native effects |
| Future configured cloud/remote executor | Only the Mission inputs/environment grants explicitly assigned to it | When the user configures and selects that executor | Yes — keep execution local |

## What never leaves the machine

- **Core-managed provider API keys and OAuth tokens.** They live only in the encrypted vault
  (`agentcowork-vault`, SQLCipher). The TypeScript sidecar never holds one; this is
  a machine-checked architecture invariant (`scripts/check-arch-invariants.mjs`).
- **Local files, sessions, memory, calendar, audit ledger and agent bindings by default.**
  They are written under `~/.agentcowork`. Explicit connector/model/remote-executor work may send selected data to the chosen destination; native agents have their own policies and stores. The app must disclose these scopes rather than promise that no data can leave.
- **Diagnostics.** The support bundle (`Settings → Diagnostics → Export`) is
  assembled locally and saved locally. It is built from an allow-list — the
  vault is never read, secret-shaped keys are dropped structurally, and long
  strings are collapsed to byte counts.

## Telemetry posture (P70.F6)

- **No analytics/telemetry SDK exists in the dependency tree**, and
  `scripts/check-public-surface.mjs` fails the build if one is added (checked
  against both lockfiles).
- **No ambient capture by Core.** Screen recording / always-on OCR is an explicit
  privacy non-goal (`ARCH/21-WORLD-MODEL.md` and `ARCH/24-COMPUTER-USE.md`): the app observes what you drop,
  open or ask it to read — governed, visible, per-window computer use only.
- **The app is fully functional with no network.** Nothing degrades into a
  nagging telemetry prompt; local models, local agents and the whole cockpit work
  offline.
- **Settings → Privacy** states the posture in place: telemetry is a disabled
  switch, not a hidden default.
- **Usage numbers** (tokens, spend) come from the local usage ledger written by
  your own provider calls — they are an observation of *your* usage, never a
  report about you.

## Retention and deletion

Retention windows are yours to set (Settings → Privacy: audit and memory
retention, applied by the retention job). **Remove all data**
(Settings → Diagnostics) deletes the entire data directory — vault, keys,
sessions, memory, audit — with a typed confirmation. Uninstalling does **not**
delete it by default; the location and the explicit path are documented in
[`docs/install-layout.md`](docs/install-layout.md) §2.
