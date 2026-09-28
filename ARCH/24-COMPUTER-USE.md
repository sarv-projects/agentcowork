# 24 — Computer Use

> **DEC-054 amendment:** Shared computer use is an optional scoped capability. Prefer structured API/site tool/DOM when it is suitable, then accessibility/visual desktop interaction; the external agent may choose a native tool and the UI reports that choice honestly. User takeover, application identity, action observation and revalidation after environment drift are part of the Mission contract (`46`, `36`).

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P3).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-CUA-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** operate the desktop when no better rung exists. **The ladder:** native API → structured UI → browser DOM/AX → CLI/MCP → **vision fallback** → raw input.
> **Honest framing (recorded):** screenshot-first is the industry default (OpenAI computer tool · Anthropic computer-use · UI-TARS “solely perceives the screenshots”). **AgentCowork chooses structured-first** — verified hybrids (Agent-S a11y+OCR, open-codex AX-first, arXiv 2511.19477) outperform where semantics exist; vision remains a first-class rung, not the default.
> **Dependencies:** `21-WORLD-MODEL` (observations, W3/W4) · `23-BROWSER` (browser rung) · `12-TRUST` (consent/approvals) · `18-MODEL-ROUTING` (vision models). **Consumers:** `15`, `22`–`28`, `34`.
> **Evidence:** `ARCHIVE/v1-research/world-model-verification.md` §1–§2 (UIA caveats, ladder reality, vision costs) · local `crates/agentcowork-desktop` (`platform/win.rs:1-23,76-97`, `ladder.rs:16-24`, `ocr.rs:1-7`, `types.rs:235-247`) · DEC-011/016 · INV-20/21.

## 1. Purpose & rules

**Owns:** UI automation per platform (UIA on Windows · AX on macOS · AT-SPI on Linux) · element resolution + the action ladder · the vision rung (capture → model → act → observe) · the raw-input rung · input safety/confirmation · observation bounds.
**Never owns:** world scanning (`21` provides structure) · browser internals (`23`) · policy decisions (`12`).

1. **Highest deterministic rung first** — never screenshot what an API or a tree can answer.
2. **Per-platform capability matrix, not abstract promises** — the accessibility rung is *not* uniform (`ladder.rs:16-24`: none by point on macOS in this build; no AT-SPI client on bare X11). The matrix is declared, tested, and honest.
3. **Observations are epoch-scoped** — valid for one action; re-read per step; never persisted as identity.
4. **Vision is fallback + verification** — size-capped, cost-visible, locally-OCR-preferred.
5. **No stealth, ever** — input synthesis is consent-gated and visibly indicated (INV-20/21).

## 2. The ladder (verified reality)

| Rung | Mechanism | Reality notes (evidence) |
|---|---|---|
| 0 | Native API | First-class APIs always win (calendar, git, office, …). |
| 1 | Structured UI (UIA / AX / AT-SPI) | Real but conditional: `AutomationId` optional + **not build-stable**; elevated UI needs UIAccess; Chromium’s UIA provider is opt-in → use CDP instead. No surveyed agent relies on UIA alone. |
| 2 | Browser DOM/AX | `23` — CDP snapshot pipeline. |
| 3 | CLI / app API / MCP | Deterministic interfaces; exec-policy gated (`12`). |
| 4 | Vision | Screenshot-first is the industry default; we use it for canvas/WebGL/poor semantics, verification, and structured misses. |
| 5 | Raw input | Absolute fallback; gated by human authorization, visibly indicated. |

Local mapping: `platform/win.rs:76-97` is the only full 3-rung click ladder in the repo (accessibility → synthetic event → raw input, raw gated); `ocr.rs:1-7` + `types.rs:235-247` already model “empty tree → vision-fallback path”.

## 3. Structured UI automation (Windows-first)

- **Read:** UIA raw/control views; bounded (nodes · depth · text limits, mirrors open-codex bounds 1200/64/500); tree is **lazy and changing** — re-read per action, never cache structure as identity.
- **Resolve:** element handle = `(runtime_id | role+name+automationId+bounds)`, valid for one action; ambiguous matches are **rejected, not guessed** (openwork pattern).
- **Act:** pattern-first (`Invoke` · `Value` · `Toggle` · `Scroll` · `Selection` · `Text` · `Window`), re-querying supported patterns per action (patterns are dynamic); then synthetic events; then raw input (rung 5, gated).
- **Elevation limits:** unreachable elevated regions are marked unknown; no partial-input attempts.
- **Timeouts:** no MS-documented UIA timeout — adopt a per-call budget + worker isolation so a hung provider cannot stall the agent.

## 4. Vision rung

- **When:** canvas/WebGL/custom render, poor semantics, verification, or a structured miss.
- **Capture:** WGC (`21` W4) with explicit **size caps** — newer Claude-class models ≈2576 px long edge / ≈4784 visual tokens; earlier ≈1568 px — the API does not downscale for us; we size before send.
- **Loop:** screenshot → model → action → observe; a `zoom`-class action for legibility (Anthropic’s own toolset adds it for exactly this).
- **Preference order:** DOM/AX-informed capture with highlights (UI-TARS pattern) > plain screenshot; **local OCR word-boxes** (`ocr.rs`) when the tree is empty and text suffices — zero model cost.
- **Injection safety:** on-screen content is **untrusted input** (prompt-injection surface); consequential actions require the approval primitive; screenshots may contain secrets — capture is consent-gated and masked where fields are protected.

## 5. Raw input rung

Synthetic input with a `HumanAuthorization`-class gate; visible indicator; rate-limited; never used for evasion (DEC-016) and never the first choice.

## 6. Input safety & confirmations

- Consequential actions (sends, purchases, deletes, permission changes) → approval primitive (DEC-021).
- Destructive/persistent patterns are denied by policy defaults (`12` §3); the ladder never overrides policy.
- No bulk-input storms: bounded action rate per target.

## 7. Observation model & context discipline

- The World Model (`21`) holds structure; agents **query** it instead of re-screenshotting (token discipline, DEC-015).
- Observation age/epoch is surfaced; stale observations are re-validated before acting.
- Snapshots entering context are bounded (`16` budget); screenshots never enter context unless the vision rung ran.

## 8. Failure modes

| Failure | Behavior |
|---|---|
| UIA provider hangs | Per-call budget + worker isolation; mark partial; ladder falls through. |
| Tree empty / semantics poor | OCR patch → vision rung (evidence-backed preference). |
| Ambiguous element | Reject; re-read; escalate to user rather than guess. |
| Elevation blocked | Mark unknown; surface guidance; no blind synthetic input. |
| Vision model unavailable when scheduled | Degrade with a recorded gap; dangerous classes require human confirmation instead (EDGE-055). |
| Vision mislocates | Verify after action (structured re-read or second observation); bounded retries; `needs_attention` on repeat failure. |
| Input rejected by policy | Typed `AuthorizationDenied`; surfaced; no fallback bypass. |

## 9. Interop

**Depends on:** `21` (observations/capture) · `23` (browser rung) · `12` (consent/approvals) · `18` (vision models) · `19` (worker isolation).
**Exposes to:** `15` (computer-use capability set), `22`–`28` (fallback execution), `34` (verification), UI (indicator surfaces).
**DAG check:** computer use observes and acts through capabilities; it never scans the world itself (`21` owns collectors) nor governs itself.

## 10. Not in v1

Cross-platform AT-SPI/AX parity beyond the declared matrix · continuous UIA event subscription (measure first) · audio/voice control · mobile surfaces.

## 11. Open questions (`OQ-CU-*`)

1. Vision model choice (managed vs local VLM) and its budget defaults (`18` tie).
2. Local OCR vs vision thresholds per scenario.
3. Per-call UIA budget + worker-isolation implementation details.
4. Confirmation thresholds (which action classes require approval).
5. Whether highlight-before-capture ships in v1 or v1.5.

## 12. Evidence

`ARCHIVE/v1-research/world-model-verification.md` §1–§2 — UIA docs + caveats; Agent-S `GroundingAgent.py:164-188,264-305` (a11y + OCR patch); open-codex `AccessibilitySnapshot.swift:48-62,92-97` + `ToolDefinitions.swift:34-56` (bounded AX, click methods); open-computer-use (vision-only outlier); Anthropic computer-use doc (2576 px/4784 tokens; zoom; injection guidance); arXiv 2511.19477 (hybrid ≈85%) · local `platform/win.rs:76-97`, `ladder.rs:16-24`, `ocr.rs:1-7` · DEC-011/016 · INV-20/21.

## 13. Requirements (`REQ-CUA-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-CUA-001` | Highest deterministic rung first (native API → structured UI → DOM/AX → CLI/MCP → vision → raw input); never screenshot what structure answers (DEC-011) |
| `REQ-CUA-002` | Per-platform capability matrix declared, tested and honest; unavailable rungs yield guidance, never silent failure |
| `REQ-CUA-003` | Observations epoch-scoped (one action), re-read per step, never identity; ambiguous matches rejected, not guessed (DM-026) |
| `REQ-CUA-004` | Structured reads bounded (nodes/depth/text); per-call budget + worker isolation so a hung provider cannot stall the agent |
| `REQ-CUA-005` | Pattern-first actuation (`Invoke`/`Value`/`Toggle`/…), then synthetic events, raw input last and gated |
| `REQ-CUA-006` | Elevated regions unreachable ⇒ marked unknown + typed guidance; no partial-input attempts |
| `REQ-CUA-007` | Vision only for canvas/poor semantics/verification/miss; captures size-capped before send; zoom for legibility (DEC-011) |
| `REQ-CUA-008` | Highlight capture > plain screenshot; local OCR when the tree is empty and text suffices; screenshots enter context only via the vision rung (DEC-015, INV-22) |
| `REQ-CUA-009` | On-screen content is untrusted input; consequential actions need approval; capture consent-gated with masked protected fields (DEC-021) |
| `REQ-CUA-010` | Raw input gated by `HumanAuthorization`, visibly indicated, rate-limited, never first, never evasion (DEC-016, INV-21) |
| `REQ-CUA-011` | Sends/purchases/deletes/permission changes require approval; policy denial is never bypassed; bounded action rate per target |
| `REQ-CUA-012` | Post-action verification with bounded retries; repeated failure lands in `needs_attention` — never an unbounded loop (DEC-022) |
