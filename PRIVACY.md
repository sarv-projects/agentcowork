# Privacy statement (P70.F5 · F6)

**Your data stays on your machine. There is no telemetry sender in this build.**

## What leaves the machine

| Destination | What | When | Can it be off? |
|---|---|---|---|
| The model provider **you** configured (BYOK) or a local runtime | your prompts, the context you attached, and tool results — to the endpoint you chose | only when you send a turn | yes — disconnect the provider, or use a local model |
| The external agent **you** installed | the same turn content, over ACP | only during a turn | yes — no agent bound, no turn |
| Pages **you** ask an agent to read | the URL, via your browser/Chrome profile | only for that read | yes — don't ask for it; `netfloor` blocks anything the policy denies |
| Update checks | the channel + target + current version, to the release endpoint | at boot and every 4h | yes — the check is a plain manifest GET; set the channel and read `docs/updating.md` §3 for the kill-switch |
| Nothing else | — | — | — |

## What never leaves the machine

- **Provider API keys and OAuth tokens.** They live only in the encrypted vault
  (`agentcowork-vault`, SQLCipher). The TypeScript sidecar never holds one; this is
  a machine-checked architecture invariant (`scripts/check-arch-invariants.mjs`).
- **Your files, sessions, memory, calendar, audit ledger and agent bindings.**
  They are written under `~/.agentcowork` and are never uploaded by the app.
- **Diagnostics.** The support bundle (`Settings → Diagnostics → Export`) is
  assembled locally and saved locally. It is built from an allow-list — the
  vault is never read, secret-shaped keys are dropped structurally, and long
  strings are collapsed to byte counts.

## Telemetry posture (P70.F6)

- **No analytics/telemetry SDK exists in the dependency tree**, and
  `scripts/check-public-surface.mjs` fails the build if one is added (checked
  against both lockfiles).
- **No ambient capture.** Screen recording / always-on OCR is an explicit
  privacy non-goal (`DESKTOP-APP-SPEC.md` §9.1): the app observes what you drop,
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
