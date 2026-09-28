# 23 — Browser Runtime

> **DEC-054 amendment:** The shared browser is an optional scoped capability offered to external agents through negotiated adapters/MCP; it does not replace their native browser. A task records which path was used. Browser profile/session ownership, login handoff, user takeover and observed final-state evidence are required for long-horizon Work (`46`, `38`).

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P3).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-BROWSER-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** the managed browser execution environment + **Browser World**. DEC-012: managed Chromium is the default; Chrome/Edge/Firefox/Opera are selectable **adapters**, not parallel embedded runtimes. The browser is an execution environment: research → browser → sources → evidence → artifact.
> **Dependencies:** `19-RUNTIME-ENVIRONMENTS` (process/environment hosting) · `21-WORLD-MODEL` (W5 browser collector) · `12-TRUST` (per-origin policy, consent) · `29-ARTIFACTS` (downloads/captures) · `14-PROVIDERS` (browser capability descriptors). **Consumers:** `15` (agent browser ops), `28` (web connectors), `34` (verification).
> **Evidence:** `ARCHIVE/v1-research/world-model-verification.md` §4 (browser world in the clones: `agent-browser`, `rustwright`, `obscura`, `open-computer-use`, CDP patterns) · product-owner brief (managed Chromium; adapter choices; Playwright/CDP facts) · DEC-012 / DEC-016 / INV-21.

## 1. Purpose & rules

**Owns:** managed Chromium lifecycle (profiles · launches · headless/headful · crash recovery) · the CDP bridge · the tab/frame model (**BrowserWorld**) · the compact AX/DOM snapshot pipeline · the ephemeral ref table · trusted-input actions · downloads/uploads routing · auth/session state handling (consent-gated) · screenshot/diff utilities.

**Never owns:** anti-bot/CAPTCHA evasion (`DEC-016`, `INV-21` — never a design goal) · general computer use (`24`) · network policy (`12`) · artifact storage (`29`).

1. **Managed Chromium default** — predictable version, isolated profile, fast launch, reliable automation, headless/background operation; no dependency on the user's setup.
2. **Adapters are integrations, not forks** — Chrome/Edge attachment or launch is offered only when a supported debugging/extension bridge is probed and the user consents. Existing logins may not be transferable. Firefox/Opera adapters are capability-probed targets, not assumed parity. Differences are declared per adapter.
3. **Structured-first inside the browser** — connectors/APIs (`28`) → DOM/AX structured ops → CDP trusted input → screenshot vision (`24`) only when structure fails.
4. **Refs are ephemeral and not a security boundary** — every ref is best-effort until the next event.

## 2. Runtime model

- **Instances** are environments (`19`): per-workspace/per-task managed profiles; isolated cookies/storage by default. An explicit “user browser” mode attaches to Chrome/Edge only through a supported, consented bridge; failure leaves the managed profile path available.
- **Lifecycle:** launch → ready → operate → park (hibernate) → close; crash recovery re-launches and re-establishes targets; tabs survive where the profile allows.
- **Visibility:** headless for background work; headful with a visible indicator when the agent operates; **user takeover** is supported (user and agent share the tab; the agent yields input).
- **Downloads/uploads:** downloads land in a managed staging area and become artifacts (29); uploads are user-gated or policy-gated.

## 3. BrowserWorld (tab/frame model — `21` W5)

| Object | Identity / notes |
|---|---|
| Tab | CDP `targetId` — session-scoped; stable ordering in the world model; never persisted across launches |
| Frame | CDP session id per frame; cross-origin iframes are a known rough edge (expand one level; skip blocked; bounded depth ≈5) |
| Element ref | `@eN` — valid until the next snapshot/navigation; **never recycled** within a session; re-resolve by role+name after events |
| Page state | URL · title · forms · downloads · auth state · freshness stamps |
| Element kinds | role taxonomy (button/link/textbox/…) with masked password fields |

Observation is a **compact text/AX snapshot** (≈200–400 tokens for a page vs ≈3–5k for raw DOM) — never raw HTML by default. Snapshots are bounded (nodes · depth · text length) and masked for protected fields.

## 4. Snapshot + actions pipeline

1. **Snapshot:** CDP AX tree (+ DOM deltas) → compact role/name/value graph with refs.
2. **Re-resolve:** after any navigation/event, invalidate refs; re-snapshot or locate by role+name.
3. **Act:** CDP trusted input events (`Input.dispatchMouseEvent`-class), never `element.click()`-style gestureless calls; keyboard/type/scroll/drag as needed; waits are condition-based (network/DOM), not sleeps.
4. **Observe:** structured state first; screenshots for verification/diff or vision fallback; vision captures are size-capped (`24`).
5. **Verify:** structured assertion (URL/title/field values); pixel/line diff utilities for visual checks; failures degrade to `guidance`/`needs_attention`, never silent retries.

## 5. Sessions, auth & consent

- User credentials are **user-controlled**: the agent never harvests passwords; logins are user-assisted or connector-based (OAuth via `28`).
- Cookies/storage live in the managed profile; profile export/import is explicit; auth state is *surfaced* in BrowserWorld, not extracted into context.
- Per-origin policy: allowed/blocked origins, read-only origins, download policy (all enforced by `12`).
- Consent: browser-session consent records (`21` §5) name the browser instance, profile, origins, and capabilities granted.

## 6. Screenshot & verification use

- Screenshots are pulled for: explicit user view · action verification · structured-tree misses (canvas/WebGL).
- Cost controls per `24` (image size/token caps); highlight-before-capture (DOM/AX-informed vision) is the preferred pattern when vision is needed.
- Diff tooling (pixel + line) supports verification (`34`) without model cost.

## 7. Failure modes

| Failure | Behavior |
|---|---|
| Browser crash | Environment restart + target re-establishment; runs resume from world state. |
| Attach fails (user browser) | Fall back to managed Chromium with a surfaced note. |
| Stale/invalid ref | Re-resolve; never click by stale ref. |
| Interrupted download | Staged bytes are never promoted to an artifact; the failure is surfaced — no partial artifact. |
| Cross-origin iframe blocked | Skip + mark partial snapshot; escalate to vision rung if critical. |
| CAPTCHA / bot check | **Surface to the user** — no evasion tooling (DEC-016). |
| Site blocks automation | Report `guidance`; do not rotate identities/proxies. |

## 8. Interop

**Depends on:** `10` · `12` (policy/egress) · `19` (hosting) · `21` (world integration) · `29` (downloads/captures).
**Exposes to:** `15` (browser capability set), `28` (web connectors’ transport), `34` (verification), UI (browser surface in the Workbench).
**DAG check:** the runtime executes browser operations; it never decides policy and never generalizes to desktop control (that is `24`).

## 9. Not in v1

Opera-specific infrastructure beyond the Chromium adapter · multi-browser farms/parallel profiles beyond declared concurrency · stealth/fingerprint tooling (never) · Firefox deep parity (automation target only until proven).

## 10. Open questions (`OQ-BRW-*`)

1. Default in Workbench: managed Chromium embedded surface vs system browser hand-off (UI tie).
2. Profile isolation granularity (per workspace vs per task).
3. User-takeover UX details (how control is handed back).
4. Whether screenshot diff belongs to `34` verification defaults or is opt-in.
5. Firefox parity scope (post-v1 evaluation).

## 11. Evidence

`ARCHIVE/v1-research/world-model-verification.md` §4 — agent-browser role taxonomy + refs (`snapshot.rs:107-179`, `snapshot-refs.md:19-27,81-83`), rustwright a11y snapshot + trusted input (`snapshot.js`, `README.md:116-128`), obscura AX tree + ref table (`accessibility.rs:37-56,520-592`), open-computer-use vision-only outlier · product-owner brief (managed Chromium default; adapters; Playwright/CDP facts) · DEC-012/016 · INV-21.

## 12. Requirements (`REQ-BROWSER-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-BROWSER-001` | Managed Chromium is the default; Chrome/Edge/Firefox/system browsers are declared adapters, never parallel engines (DEC-012) |
| `REQ-BROWSER-002` | Browser instances are environments (`19`) with lifecycle + crash recovery; per-workspace/task profile isolation by default |
| `REQ-BROWSER-003` | User-browser attach is consent-gated with fallback to managed Chromium; agent operation is indicated; user takeover supported |
| `REQ-BROWSER-004` | BrowserWorld: tab = CDP `targetId` (session-scoped), frames bounded (≈5), page state includes forms/downloads/auth/freshness (DM-026) |
| `REQ-BROWSER-005` | Compact bounded snapshots (≈200–400 tokens, never raw HTML by default) with masked protected fields |
| `REQ-BROWSER-006` | Element refs are ephemeral, never recycled, re-resolved by role+name after events; never clicked stale; not a security boundary |
| `REQ-BROWSER-007` | CDP trusted input only (no gestureless clicks); condition-based waits, never fixed sleeps |
| `REQ-BROWSER-008` | Structured-first inside the browser (connectors → DOM/AX → CDP → vision); captures size-capped (DEC-011) |
| `REQ-BROWSER-009` | Credentials are user-controlled (user-assisted/connector OAuth); auth state surfaced, never harvested (INV-02) |
| `REQ-BROWSER-010` | Per-origin policy (allow/block/read-only/download) enforced by Trust; session consent records name instance/profile/origins (INV-20) |
| `REQ-BROWSER-011` | Downloads stage into artifacts with provenance; uploads are user- or policy-gated (CTR-018) |
| `REQ-BROWSER-012` | CAPTCHA/bot blocks are surfaced or return `guidance`; zero evasion, identity rotation or proxy tooling (DEC-016, INV-21) |
