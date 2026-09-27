# Auto-update, channels and data migration (P70.C1–C7)

The updater is `tauri-plugin-updater` (v2.10.x). Its trust anchor is the
minisign **public key** in `src-tauri/tauri.conf.json` →
`plugins.updater.pubkey`; only artifacts whose manifest signature verifies
against that key are ever installed (`scripts/check-updater-keys.mjs` keeps
the anchor and the local keypair in agreement).

## 1. Manifest + hosting (C1)

`release.yml` (tag-triggered) builds the signed Windows installers and
`tauri-action` attaches the signed `latest.json` to the GitHub release. Two
endpoints are configured, in order:

1. **Hosted:** `https://releases.everyaios.dev/{channel}/{target}/{arch}/{current_version}` — the host still serves the pre-rename name; the endpoint rename to a `releases.agentcowork.dev` host is a pending release/infra decision (it needs the DNS host and the signed manifest base moved together), so the doc states the endpoint the build actually uses rather than the one we intend.
   — the primary. A request answers with the signed static JSON manifest for
   the requested platform/version pair (tauri-plugin-updater's dynamic
   "is there anything newer" shape). See §2 for the channel segment.
2. **Fallback:** the GitHub Releases `latest.json` for the stable channel —
   the identical artifact tauri-action publishes. If the hosted endpoint is
   unreachable the plugin simply tries the next endpoint; if *both* are
   unreachable the check fails honestly (an error string the UI renders
   verbatim) and the running app is untouched.

**Fallback rules (never brick, never silently downgrade):**

- A failed check never mutates the running app — the periodic auto-check
  swallows the error and retries on the next interval (`src-tauri/src/updater_cmds.rs`
  → `spawn_periodic_check`; 45 s after boot, then every 4 h).
- The plugin compares semantic versions; a manifest advertising an *older*
  version answers "no update". There is no downgrade path through the
  updater (downgrades are a manual reinstall; see §6).
- The manifest is minisign-signed; a bad signature is refused before anything
  is written. There is no "install unsigned" escape hatch.

## 2. Channels (C2)

Two channels: **`stable`** (default) and **`beta`**. The selection persists
in `<data_dir>/update_channel.json` (atomic write; a corrupt or unknown value
reads back as `stable`). It is applied by **endpoint selection**, not by
manifest surgery:

| Channel | Endpoints consulted |
|---|---|
| `stable` | hosted `…/{target}/{arch}/{current_version}` → GitHub `latest.json` |
| `beta` | hosted `…/beta/{target}/{arch}/{current_version}` (no GitHub fallback) |

The GitHub fallback is stable-only by design: `latest.json` always describes
the newest stable release, so pointing the beta channel at it would silently
hand beta users stable builds. Until the hosted endpoint serves a `beta/`
prefix, selecting beta therefore reports "no update" honestly — a user can
not strand themselves on a channel that cannot feed them.

Surface: Settings → About → Release channel (`updater_channel_get` /
`updater_channel_set`). `scripts/check-update-pipeline.mjs` enforces that the
config endpoints and the code's endpoint builder keep agreeing, and that the
GitHub fallback is never offered to beta.

## 3. Staged rollout + kill switch (C3)

Rollout is a property of the **hosted endpoint** (the GitHub fallback has no
partial-rollout capability):

- **Staged:** the hosted manifest for `{current_version}` can answer
  "no update" for a fraction of requests (deterministic on a client-stable
  key, e.g. a hash of the install id) while answering the new version for the
  rest. Widening the fraction is a manifest-config change, not a release.
- **Kill switch:** pointing the hosted manifest back at the previous version
  (or answering "no update" for the offending version) halts the rollout
  **without publishing a new binary**. Clients on the bad build that already
  downloaded it keep it; clients that have not stop receiving it.
- **Rollback:** the manifest for the current version can be re-signed to
  advertise the previous version — the updater refuses only *downgrades*,
  so a genuine rollback is done by publishing a **new** patch release built
  from the previous tag (see §6). A pure manifest-side "downgrade" is
  deliberately not implemented because the plugin's comparator rejects it.

The channel file is an input to all three: flipping a cohort's channel (or
reverting it) is the fastest halting lever that does not touch the manifest.

## 4. Update UX (C4)

- **Checks:** one shortly after boot (delayed 45 s so it never competes with
  startup work), then every 4 hours, plus the manual button in Settings →
  About.
- **Download:** background, with progress relayed as `updater-status` events
  (`downloading` + percent, `downloaded`, `failed`). The artifact (an NSIS or
  MSI installer) is held in memory until the user acts.
- **Install:** passive (`plugins.updater.windows.installMode: "passive"`),
  on the user's explicit **Restart to update** — never mid-session.
- **Visible state:** the About section renders the actual phase — `Up to
  date`, `vX available`, `Downloading… N%`, `downloaded — ready to install`,
  `Installing…`, or the error string. The version badge itself is the build's
  own version and never claims more than that.

## 5. Upgrade data migration (C5)

The durable-store contract is `crates/agentcowork-core/src/store_schema.rs`
(P70.A8): every store carries a schema version, stamped at boot; a store
recorded at a **newer** version refuses the boot (C6); data written before
stamps existed is **adopted** at the current version, not claimed migrated.

`crates/agentcowork-core/tests/acceptance_upgrade_evidence.rs` is the
compile-time statement of what a v(N-1) → v(N) upgrade must preserve — vault
hydration, Work/event log, checkpoints, memory, calendar, automations, audit
chain validity, no orphaned records — and what the boot path does when it
meets a newer store. **Executing it against real sequential builds on a
Windows host is the remaining evidence** (`P70.C5`/`P70.E8`) and is recorded
in TODO.md as such; nothing in this section is "proved" until then.

## 6. Downgrade + minimum-version policy (C6)

- **Data:** a build refusing to open data written by a newer schema is
  enforced at boot — `store_schema::ensure_all()` runs before any store
  opens, and `agentcowork-vault` returns `VaultError::NewerSchema` for a newer
  vault database. The refusal names the store and both versions.
- **Signing prerequisite (C1):** release builds only produce updater
  artifacts when the GitHub secrets `TAURI_SIGNING_PRIVATE_KEY` (and its
  password) are present; the release workflow fails loudly without them
  (`docs/signing.md`).
- **Binary minimum version:** the supported upgrade window is **the previous
  release** (N-1 → N). Older installs upgrade by installing the new build
  directly (the installers are per-user and self-contained); there is no
  intermediate-hop requirement. This paragraph is the explicit statement the
  row asks for; the doctor surface reports the running version so support can
  tell what a user is on.
- **Downgrades:** reinstalling an older build over newer data is *refused at
  boot* by the newer-schema check (the store names itself). The recovery is
  documented in §7, not hidden.

## 7. Rollback drill (C7)

The drill to perform per release candidate on a real Windows host (recorded
under `P70.E8`):

1. Install build N-1, configure a provider, run one task so every durable
   store has content.
2. Install build N over it (upgrade path). Verify: vault unlocks, Work/event
   log intact, checkpoints listed, memory + calendar intact, automations
   still fire, audit chain verifies (`agentcowork-audit` Merkle chain).
3. Uninstall build N, reinstall build N-1 over the same data. **Expected
   honest failure:** if N migrated a store to a newer schema version, the N-1
   binary refuses to boot with the store + versions named — that is the
   designed behaviour, and the drill records exactly which stores refuse.
   User data is untouched (the refusal happens before any write).
4. Restore path: reinstall N (data opens again). The drill's record includes
   whether any store had been migrated at all — a release that migrated
   nothing rolls back transparently, and that fact is recorded too.

**Until the drill is executed on Windows, P70.C5/C6/C7 remain
`[IMPLEMENTED — unverified]` with the residual named — the mechanisms are
in the tree; the evidence is not.**
