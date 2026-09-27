# Signing and key custody

> **What this file is.** The custody, renewal, rotation and revocation procedures for the two signing
> identities a release depends on. **P70.B1** (Authenticode code signing) and **P70.B4** (the updater
> minisign keypair) both fail loudly when their material is absent — a lapsed certificate or a missing key
> must stop a release, never quietly produce an unsigned one (`scripts/check-release-matrix.mjs` enforces
> the workflow-side assertions).

---

## 1. The two identities

| | Authenticode (P70.B1) | Updater minisign (P70.B4) |
|---|---|---|
| What it signs | every produced executable + installer (`.exe`, `.msi`) | the updater manifest and its `.sig` artifacts |
| Where the private material lives | **GitHub secret** `WINDOWS_CERTIFICATE` (PFX, base64) + `WINDOWS_CERTIFICATE_PASSWORD` — never in the repo | **`.tauri/` locally, gitignored**; on CI only as the `TAURI_SIGNING_PRIVATE_KEY` (+ `_PASSWORD`) secrets |
| What the public half is | the certificate's chain, visible in the installer's Properties dialog | the `pubkey` string in `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`) — **this is the trust anchor**: a client refuses an update whose signature does not verify against it |
| Rotation | new certificate, update the two secrets | generate a new keypair, update `pubkey` in the same release that stops signing with the old key (see §4) |

The private key has never been committed: `.tauri/` is in `.gitignore`, `check-artifact-hygiene.mjs`
fails any bundle containing `tauri_signing_private*` or key material, and the pre-release gate re-checks
produced artifacts, not just the tree.

## 2. Why the pinning matters (what a mistake would cost)

- **The minisign keypair is pinned to `tauri.conf.json`.** It was generated 2026-08-21
  (`.tauri/agentcowork-updater.key[.pub]`), and the base64 `.pub` is embedded in the config. The updater's
  pubkey is the root of update trust: whatever string is in the shipped config decides which signatures a
  user's install accepts. Changing it without shipping a *new installer* bricks updates for every existing
  install (they verify against the old anchor).
- **The MSI upgrade code is pinned** (`bundle.windows.wix.upgradeCode`): it keeps Windows treating every
  release as the same application. Renaming the product without keeping the code produces duplicate
  installs instead of upgrades (`check-app-metadata.mjs` verifies the pin equals the bundler's derived
  value).
- **Authenticode timestamps** (RFC 3161, applied by `tauri-bundler` when the certificate is in the user
  store) keep signatures valid after the certificate itself expires — a lapsed certificate must not
  invalidate past releases.

## 3. Renewal and rotation procedures

### 3.1 Authenticode certificate renewal (before expiry)

1. Obtain the renewed certificate (same or higher assurance level) as a PFX.
2. `base64` the PFX and update the `WINDOWS_CERTIFICATE` secret; update `WINDOWS_CERTIFICATE_PASSWORD`
   if the password changed.
3. Run a release build to a **draft**: verify the produced installer's digital-signature details name the
   new certificate and carry a valid countersignature.
4. Publish. Old releases keep validating via the timestamp — no re-signing needed.

**If the certificate lapses silently:** the `Require the Authenticode certificate (P70.B1)` step fails the
release job when the secrets are absent, and an expired-but-present certificate produces a signature that
Windows flags — the draft-verification step in 3.3 is where that is caught. Either way the release does
not ship unsigned.

### 3.2 Updater keypair rotation (compromise or routine)

1. Generate a new keypair: `tauri signer generate -w .tauri/agentcowork-updater.key` (keep it out of git).
2. Base64-encode the new `.pub` (`base64 -w0 < agentcowork-updater.key.pub`) and place it in
   `plugins.updater.pubkey`.
3. Ship this change **in a release signed with the OLD key** — the anchor update reaches users only
   through a signed update they accept. The release *after* that one signs with the new key.
4. Verify: install the anchor-carrying build, confirm the next update verifies and installs.

### 3.3 Revocation (compromise of the private key)

1. **Stop signing immediately** — rotate the `TAURI_SIGNING_PRIVATE_KEY` secret to the new key from 3.2
   step 1 (this revokes the compromised key's ability to produce future releases).
2. Publish the anchor-carrying release (3.2 step 3) as the recovery path; users on older anchors must
   reinstall from the published installer — say so in the release notes.
3. If Authenticode material is compromised, revoke the certificate with the CA and follow 3.1; Windows
   distributes the revocation through certificate trust lists.

## 4. Where this is enforced

| Property | Enforced by |
|---|---|
| release job fails without the Authenticode secrets | `release.yml` (`Require the Authenticode certificate`) |
| updater secrets stay wired into the build env | `scripts/check-release-matrix.mjs` |
| private key never in the repo / never in a bundle | `.gitignore` + `scripts/check-artifact-hygiene.mjs` |
| pubkey in config matches the local public key | `scripts/check-updater-keys.mjs` |
| MSI identity stays pinned | `scripts/check-app-metadata.mjs` |
