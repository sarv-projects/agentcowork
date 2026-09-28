# 21 — World Model

> **DEC-055 Experience integration:** Workbench selections and Library retrieval reference World/Artifact identities plus observed version and location, never a path or window caption alone (`48` §§5–6). Browser/desktop takeover invalidates observations until a new snapshot. World facts carry source, freshness and confidence; they are context candidates, not Mission requirements or policy authority.

> **DEC-054 amendment:** Mission resume queries freshness-stamped World objects and external resource versions to detect drift before mutation; it does not copy World state into Mission or grant the agent global observation. Stable file/repository identity and artifact dependency edges support selective invalidation (`35`, `36`).

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P2).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-WORLD-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Thesis:** *“Don't make the AI look at the computer. Make the computer explain itself to the AI.”* — structural state first; vision is a fallback rung (`24-COMPUTER-USE`).
> **Dependencies:** `30-EVENTS` (stream), `25-FILES` (file identity), `23-BROWSER` (browser world), `12-TRUST` (consent/guard), `19-RUNTIME-ENVIRONMENTS` (collector hosts/helpers), `16-CONTEXT` (primary consumer).
> **Evidence:** `ARCHIVE/v1-research/world-model-verification.md` (359 lines, citations per claim) · clones `agent-browser` · `rustwright` · `obscura` · `open-codex-computer-use` · `Agent-S` · `UI-TARS-desktop` · `open-computer-use` · MS docs (UIA, MFT/USN, `FILE_ID_INFO`) · arXiv 2511.19477 · local code (`crates/agentcowork-desktop`, `agentcowork-storage`).

## 1. Purpose & honest framing

A continuously updated **structural map** of the machine — apps, windows, processes, files, browser, devices, and the relationships between them — plus a change stream. Consumers *query* the world; they do not screenshot it by default.

**Framing correction (recorded):** screenshot-first is the current *industry default* (OpenAI computer tool, Anthropic computer-use, UI-TARS “solely perceives the screenshots”). Structured-first hybrids demonstrably outperform where semantics exist (Agent-S defaults to `a11y_tree` + OCR patch; open-codex is AX-first with bounded snapshots; a production browser-agent paper reports ~85% vs ~50% prior agents with AX+selective vision). **AgentCowork chooses structured-first deliberately** — it is a design choice we justify by latency/token/precision, not a description of what everyone does. Vision stays a first-class rung.

**Owns:** collectors · registries (app/window/process/file/browser) · world graph · event stream · freshness/staleness · consent records · incremental update machinery.
**Never owns:** acting on the world (capabilities do that) · file *content* indexing (`25`/`26`) · browser automation (`23`) · permission decisions (`12`).

## 2. Collector set — v1 (Windows-first)

| ID | Collector | Observation | Delta source | Consent class | v1 |
|---|---|---|---|---|---|
| W1 | File inventory + deltas | paths · stable file identity (§3) · size/times/attrs/links (metadata only) | MFT enum (`FSCTL_ENUM_USN_DATA`) + USN journal (`FSCTL_READ_USN_JOURNAL`); RDCW + bounded re-walk fallback | Elevated (admin/helper) for USN/MFT; basic = walk/RDCW | ✅ |
| W2 | Process + window registry | PID+start time · window handle/title/app/bounds · foreground | poll (1–5 s) + `SetWinEventHook` | Standard | ✅ |
| W3 | UI tree (on demand) | UIA raw/control view, bounded nodes/depth; `AutomationId` as hint only | re-read on action; optional UIA events (pending measurement) | Per-app automation allow-list | ✅ |
| W4 | Window capture (on demand) | window PNG (WGC → PrintWindow/ScreenDC fallback) | pull only | Screen-recording consent | ✅ (behind W3 miss/verify) |
| W5 | Browser world | tabs/frames · AX/DOM snapshot + ephemeral refs | CDP target/navigation events; re-snapshot on action | Browser-session consent; per-origin policy | ✅ (cross-platform) |
| W6 | Devices, registry, network shares | — | — | — | ❌ deferred |
| W7 | Content index / OCR | — | — | — | ❌ deferred (metadata-first) |
| M1/L1 | macOS / Linux parity | parity of W1–W5 | FSEvents / fanotify+inotify | TCC / user-session scopes | parity lanes (not v1 blockers) |

Rules: no full rescan per query · event delivery never triggers unbounded work · every collector is independently enable-able/disable-able and health-reported. Browser collector ships with v1 (same CDP machinery as `23`).

## 3. Identity model (`DM-026`)

| Object | Identity | Caveats |
|---|---|---|
| File (Windows) | `(VolumeSerialNumber, FILE_ID_128)`; 64-bit index + serial as fallback; USN FRN for event correlation | IDs are **reused after delete** → identity = `(volume, fileId, incarnation)` (creation time / sequence evidence); FAT IDs can change |
| File (POSIX) | `(st_dev, st_ino)` + `st_nlink` | Inodes reusable; pair with size/mtime guards; hardlinks via link count |
| Browser tab | CDP `targetId` (+ session id for frames) | Per-browser-session; never persisted across launches |
| Browser element ref | none durable — `@eN` valid until next snapshot/navigation; never recycled within a session | Re-resolve by role+name after every event; refs are **not a security boundary** |
| Window | `HWND` + PID + class + title + process launch time | Handles reused; ambiguous matches are rejected, not guessed |
| Process | PID + **process start time** (generation discriminator) | PID reuse is the failure case |
| UI element (desktop) | **not persistent** — epoch-scoped observation handle `(runtime_id | role+name+automationId+bounds)` valid for one observation/action | `AutomationId` is optional and not build-stable; re-read per action |
| Content hash | content identity, kept **separate** from file identity | Many files → one blob |

**Code-phase fix identified (frozen code):** `crates/agentcowork-storage/src/walk.rs:131-157` zeroes `dev`/`ino` on Windows, corrupting dedup (`dedup.rs:106-118`). Fix = MFT-based `(volume, fileId)` identity per this model.

## 4. Incremental updates, epochs, gaps

- **Cursor model:** every collector instance stores `(source, scope, epoch, cursor, observed_at)`.
- **Epoch per source:** Windows = `JournalID`; macOS = device UUID + monotonic event ID; Linux = mount ID + watch generation; browser = `documentId`/URL; UIA = snapshot generation.
- **Gap policy (mandatory):** every native watcher is lossy on overflow — inotify `IN_Q_OVERFLOW`, fanotify `FAN_Q_OVERFLOW`, FSEvents `MustScanSubDirs`/`KernelDropped`, RDCW zero-length buffer, dead/truncated USN journal. Gaps **abort and force a scoped rescan** (smallest known scope). Never a silent gap: record a freshness anomaly event. The local USN contract (`usn.rs:77-90`: records at/below cursor are errors, never silently applied) is the pattern to extend to all collectors.
- **Freshness contract:** every object carries `observed_at` + `source` + `epoch`; consumers receive explicit staleness; write paths re-validate stale objects; per-collector TTLs (files: minutes; process/window: seconds) can mark objects `unknown`.
- **Bounds:** queries read the index; they never walk the filesystem (INV-20).

## 5. Consent & privacy

**Record per collector instance** (DM-backed, audited): collector id + version · scope (paths/volumes/apps/origins) · capability required (standard | elevated | OS-permission) · elevation/permission actually granted and how · event source + epoch/cursor · data classes (metadata/names/pixels/text) · start/stop + retention · revocation path · audit receipt.

Rules:
1. Deny-by-default; no silent scope expansion; **no persistent “always allow” in v1**.
2. Visible indicator while any capture collector is active.
3. **Metadata-first:** collectors never read file content; screenshots only for explicit view, action verification, or W3 miss/verify; `IsPassword`/protected fields excluded or masked.
4. Elevated file index options (USN/MFT require admin — verified): (a) small privileged helper (Everything pattern, opt-in, no service/autostart by default), (b) per-scan elevation prompt, (c) non-admin mode = walk/RDCW only. Mode is recorded per instance; choice resolved with `12-TRUST` (OQ-WM-02).
5. Local-first: no upload path exists in the world-model contract.

## 6. Query surface & consumers

`CTR-017 WorldService`: `query(filter)` · `subscribe(filter) → Stream` · projections for `16-CONTEXT` and external agents (`32`, sensitivity-filtered).

Canonical use cases (why the model exists):
- “Open the spreadsheet from yesterday” → file/world resolution + artifact hint → Office capability (no GUI navigation).
- “Go back to that GitHub tab” → browser world tab recall (target identity).
- “What is the active document?” → window + process + (optional) UIA read — structured, no screenshot.

## 7. Failure modes

| Failure | Behavior |
|---|---|
| UIA provider hangs | Per-call budget + worker isolation; partial tree marked partial; ladder falls through (`24`). |
| Elevated UI unreachable (no UIAccess) | Collect what is reachable; mark elevated regions unknown; no input synthesis without consent. |
| Chromium UIA provider unavailable | Browser paths use CDP (23), never UIA; native apps use UIA. |
| Journal deleted / truncated | Discard cursor, rescan that volume; record epoch reset. |
| Watcher overflow | Scoped rescan + freshness anomaly event (never silent). |
| Permission denied during scan | Scoped skip + surfaced count (metadata-mode honesty) — never an elevated bypass (EDGE-049). |
| Canonical-path swap mid-scan (symlink/junction) | The path is re-validated at use; a mismatch denies the read and is recorded (EDGE-046). |
| Collector crash | Health degraded; stale markers; queries return partial with freshness. |
| Helper/service absent (elevated mode) | Fall back to non-admin mode with a surfaced capability note. |

## 8. Interop

**Depends on:** `10` kernel · `12` trust (consent/guard) · `19` runtime (collector hosts, helper) · `25` files (identity) · `23` browser (CDP) · `30` events (stream).
**Exposes to:** `16` context · `20` workflow triggers · `22`–`24` domains · UI (world browser) · `32` projections.
**DAG check:** the world observes; it never executes capabilities or decides permissions.

## 9. Not in v1

W6 devices/registry/network shares · W7 content index/OCR · continuous UIA event subscription (pending event-volume measurement) · macOS/Linux parity beyond the browser collector (parity lanes M1/L1) · SMB/ReFS edge cases · content search.

## 10. Code-phase fixes identified (frozen code, do not touch now)

1. `agentcowork-storage/src/walk.rs:131-157` — dev/ino zeroing on Windows corrupts dedup (`dedup.rs:106-118`); replace with `(VolumeSerial, FILE_ID_128)` + incarnation.
2. `usn_winapi.rs` is present but unwired — wire it as W1's delta source.
3. `agentcowork-desktop` ladder caveats: accessibility rung not uniform per platform (`ladder.rs:16-24`); WGC readiness (`platform/wgc.rs`) needs a Windows acceptance record.
4. UIA collector must treat `AutomationId` as a hint, handle UIAccess elevation limits, and use CDP for browser content (Chromium UIA is opt-in).

## 11. Open questions (`OQ-WM-*`)

1. Windows-first vs parity for collectors (existing OQ-002) — evidence favors Windows-first + cross-platform browser.
2. Elevated file index: helper service vs per-scan prompt vs non-admin fallback (with `12-TRUST`).
3. Is NTFS 64-bit FRN sequence behavior sufficient for delete→recreate incarnations? (empirical harness needed).
4. UIA per-call timeout budget + worker isolation tuning (no MS-documented timeout exists).
5. Browser: CDP-only for v1 (yes) vs chasing Chromium UIA provider (no).
6. macOS FSEvents file-level flags + Full Disk Access scoping (parity lane).
7. WGC readiness verification (Windows acceptance record).
8. Continuous UIA event subscription: worth it vs on-demand reads? (measure).
9. Vision rung: managed models vs local OCR/grounding (`24` decision; `ocr.rs` seam exists).
10. Content-index phase (deferred; trigger = metadata index proven + product demand).

## 12. Evidence

`ARCHIVE/v1-research/world-model-verification.md` — claims A–E with per-claim citations; §2 ladder reality; §3 identity/cursor/freshness; §4 browser-world patterns; §5 collector set + consent; §6 open questions. Key anchors: MS UIA tree/property/pattern docs · MS MFT/USN + `FILE_ID_INFO` docs · Chromium a11y/UIA docs · Agent-S `GroundingAgent.py:164-188,264-305` · open-codex `AccessibilitySnapshot.swift:48-62,92-97` · agent-browser `snapshot-refs.md:19-27,81-83` · local `platform/win.rs:1-23,76-97` · `ladder.rs:16-24` · `walk.rs:131-157`.

## 13. Requirements (`REQ-WORLD-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-WORLD-001` | Structural state first: consumers query indexed world objects; capture only on demand (explicit view/verification/miss) (DEC-011) |
| `REQ-WORLD-002` | Collector set W1–W5 with independent enable/disable + health; W6/W7 deferred; browser collector shares `23` CDP machinery |
| `REQ-WORLD-003` | Per-kind identity (DM-026): file `(volume, fileId, incarnation)`, process PID+start, window handle+launch, tab session-scoped, epoch-scoped UI handles |
| `REQ-WORLD-004` | Every collector stores `(source, scope, epoch, cursor, observed_at)`; epoch reset discards the cursor and rescans |
| `REQ-WORLD-005` | Watcher/journal gaps abort to a smallest-scope rescan + freshness anomaly event — never a silent gap (INV-20) |
| `REQ-WORLD-006` | Every object carries `observed_at` + `source` + `epoch`; stale objects are explicit (TTL → `unknown`); write paths re-validate (CTR-017) |
| `REQ-WORLD-007` | Queries/subscriptions read the index, never walk the filesystem; event delivery never triggers unbounded work (INV-20) |
| `REQ-WORLD-008` | Deny-by-default consent records per collector instance; no silent scope expansion; no persistent "always allow" in v1 (INV-20) |
| `REQ-WORLD-009` | Metadata-first (no content reads), capture-gated with visible indicator, masked protected fields, strictly local (no upload path) |
| `REQ-WORLD-010` | Elevated mode absent/denied ⇒ non-admin fallback, recorded per instance, surfaced — never silent elevation (INV-24) |
| `REQ-WORLD-011` | `CTR-017` `query`/`subscribe` only; external projections sensitivity-filtered; internals never exposed (INV-11) |
