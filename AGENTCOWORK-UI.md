# AGENTCOWORK-UI — UI Architecture: shell · DocumentSurface · chat · composer

> **DEC-055 target amendment (2026-09-28):** `ARCH/48-EXPERIENCE-SURFACES.md` is the final interaction HLD/LLD. This document is the frozen v1 baseline and source-path inventory. Where it prescribes a first-party Native picker, obligatory Work mode, permanently exposed model/reasoning/context controls, `/eaios:*` host commands, or a separate older shell, `48` supersedes it. New UI work follows `48`, the requirement registry, and the scenario oracles in `ARCH/49-TEST-CASES.md`.

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P5).
> **P7 pass (2026-09-26):** line-checked; requirements proposed (`REQ-UI-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Authority:** root for the **UI** row of `ARCH/00-INDEX.md` §2 — "UI/UX architecture, chat rendering, interaction model. Derives from SPEC + Experience plane." Compliance order: `AGENTCOWORK-SPEC.md` §9 (Experience contract — WHAT) → `ARCH/03-HLD.md` §2/§5 (HOW) → this doc. A change that alters behaviour described here requires a `DEC-*`; this doc MUST NOT contradict a `DEC-*`, `INV-*`, `DM-*` or `CTR-*`.
> **Derives from:** SPEC §3/§4/§6/§9/§11 · HLD §2 (Experience plane: *rendering, input, presentation, view state — never domain logic, execution, policy*), §5, §6, §7 · `ARCH/13-CAPABILITY.md` §6 (composer capability negotiation) · `ARCH/18-MODEL-ROUTING.md` §5 (normalized reasoning dial) · `ARCH/30-EVENTS.md` §3 (typed stream — *the UI's only progress channel*) · `ARCH/32-CHANNELS.md` §1/§3/§7 (surfaces are projections; approval routing) · `ARCH/29-ARTIFACTS.md` · `ARCH/22-OFFICE.md` · `ARCH/16-CONTEXT.md` §7 (Context Inspector) · `ARCH/11-WORK.md` §2/§3/§8.
> **Evidence base:** `ARCHIVE/v1-research/ui-architecture-evidence.md` (Draft 2026-09-26; 1,710 lines) — first-hand `ui/` inventory with `path:line`, the shipped token contract, competitor teardown, chat proposals R1–R40, shell/composer proposals R41–R56, the token-discipline table, gaps G1–G38 and the three delivery slices. Cited here as `ev: evidence §n / Rn / Gn`; direct `ui/` reads made in this pass are cited as `ev: ui/…:line`.
> **Current theme only (product-owner constraint).** This doc introduces **no new theme**. Colour, radius, spacing, motion and typography come from the shipped token contract — `ui/DESIGN-SYSTEM.md` (the named current theme source, `ARCH/00-INDEX.md` §10) and `ui/src/globals.css` — and every new component MUST read those tokens. Constraints C1–C11 (`ev: evidence §2`) are normative. The v1 design work is the **chat surface, the shell and the composer**, not the palette.
> **Names:** AgentCowork (product) · Core (runtime); v1 names only (`ARCH/01-NAMING.md`).
> **Code:** frozen. This doc describes the target; it changes no code.

**Evidence notation:** `ev: path:line` = first-hand source read · `ev: evidence §n / Rn / Gn` = the evidence base · `inf:` = inference, labelled · confidence `H/M/L` · `UNVERIFIED` is temporary and names what would confirm it (§13).

---

## 1. Scope, contract and UI rules

### 1.1 Owns / never owns

| Owns | Never owns |
|---|---|
| Rendering, input, presentation, view state (HLD §2 Experience plane) | Domain logic, execution, policy (HLD §2) |
| Shell layout · Workbench · universal DocumentSurface · transcript and chat rendering · composer · approvals UI · Runs/subagent depiction · artifacts and Library UI | Core state, stores, scheduling, authorization (P-01) |
| The UI side of token discipline (INV-13) and the accessibility/motion/type standards | Capability semantics, provider selection, model routing (`13`/`14`/`18`) |

**P-01 restated for the UI:** no surface is a source of truth. Every list on screen is a projection over Core state; every mutation leaves through a contract. The projection map:

| Surface | Reads (projection) | Mutates through |
|---|---|---|
| Transcript | session `ui-history` projection (CTR-004) + `run.*` / `step.*` / `model.delta` / `tool.*` events (`30` §3) | `WorkService` admission (CTR-003) |
| Runs | work-tree projection (CTR-003/026, `11` §8) | scheduler verbs only (`11` §6) |
| Approvals | `ApprovalService` + `approval.*` events (CTR-012, `12` §5) | `decide()` |
| DocumentSurface | `ArtifactService` refs (CTR-018) + domain preview/render hooks (`22` §5) | open/close view state only |
| Agent/model pickers | the bound agent's own ACP config options + `ModelDescriptor` capabilities (CTR-014, `18` §2) | session config-option write |
| Context indicator | `ContextProvider` snapshot + controller pins/excludes (CTR-006/007, `16` §7) | pin / exclude / focus |
| Status bar · Audit | runtime health projection · audit chain (`12` §9) | — (read-only) |

The UI never evaluates policy, never opens a socket, and never talks to a provider directly (`INV-04`, `INV-05`, `INV-15`).

### 1.2 UI rules (normative)

| ID | Rule | Evidence |
|---|---|---|
| UI-01 | **Projection only.** No UI component owns durable state or evaluates policy. | P-01; `03-HLD.md` §2 |
| UI-02 | **Deterministic-first.** Rendering, navigation, previewing, diagram conversion and listing never call a model. | SPEC §9 · `DEC-015` · `INV-13` · §8 |
| UI-03 | **Never present an inferred value as measured.** Unknown renders `—` or renders nothing; a figure is either reported or absent. | `ev: ui/src/components/views/run-projection.tsx:33-56`; `ev: ui/src/lib/store.ts:191-194`; C10 |
| UI-04 | **Presence with a lock beats absence.** A policy-blocked control stays visible, marked blocked, with the reason and who can change it. Red is reserved for failures; blocked uses neutral ink + lock. | `ev: evidence §3.8` (openwork P4, C5) |
| UI-05 | **Nothing auto-navigates.** A result may *offer* an inline action; a tool finishing never steals focus, opens a pane, or moves the page. | `ev: evidence §3.8` (S5) |
| UI-06 | **One rail per turn.** A turn's steps read as one rail (leading slot · sentence label · duration right); a finished turn collapses to one line. | `ev: evidence §3.8` (T1), R22 |
| UI-07 | **Progressive disclosure.** Advanced detail sits behind a *labelled* collapsed row; open-by-default only when the content *is* the answer. | `ev: evidence §3.8` (P3, S3) |
| UI-08 | **Reserve space for anything that ticks.** CLS = 0 is a standing standard: `min-h` on live regions, fixed slots for readouts, intrinsic-size hints on the transcript. | `ev: ui/src/components/chat/message-bubble.tsx:197`; `ev: ui/src/components/chat/tool-chip.tsx:533,541`; C8 |
| UI-09 | **Tokens only.** No literal hue outside `globals.css`'s token blocks; never assume the accent is blue; a label on the accent uses `--brand-foreground`. | C1–C4; `ev: ui/src/globals.css:44-105,244-345` |
| UI-10 | **Telemetry is mono + tabular.** Every duration, token count, percentage or id is JetBrains Mono with `tabular-nums`. | C5; `ev: ui/src/globals.css:119-120`; `ev: ui/src/components/chat/message-bubble.tsx:202,209` |
| UI-11 | **Motion is a vocabulary, not a mood.** ~100–300 ms utility animations (the chart crossfade runs to 400 ms, §9.2); damped springs for the few high-impact moments; exit faster than enter; no entrance animation on high-frequency surfaces; reduced motion kills all. | C7; `ev: ui/src/globals.css:627-745,772-781` |
| UI-12 | **Keyboard-complete.** Every action reachable by keyboard, `focus-visible` ring on all interactives, `aria-live` on status, no nested scroll traps except one explicitly labelled overflow region. | `ev: ui/src/globals.css:791-795`; `ev: ui/src/components/chat/tool-chip.tsx:545,554`; §9 |
| UI-13 | **No raw ids / JSON / ANSI in the transcript.** Tool ids live in *Technical details*; CLI streams are normalized at the drawer boundary. | `ev: ui/src/components/chat/tool-chip.tsx:121-136,199-213`; C11 |
| UI-14 | **Never render raw chain-of-thought as prose.** Reasoning renders only through the `reasoning` projection; a raw variant, if ever surfaced, sits behind a named *Technical details* disclosure. | R29; `18` §5 |
| UI-15 | **The transcript is linear.** Trees and DAGs belong to the Runs lens (and Computer-use's existing DAG projection), never to the conversation. | R46; §6 |
| UI-16 | **Name the owner.** Each surface states which projection it reads; no hidden side channels for state. | `INV-23`; `30` §1 |
| UI-17 | **Windows-first provenance.** An agent/runtime row shows a discriminated *location* (`managed · windows_path · app_paths · user_path · package_manager · wsl · unavailable`) and keeps `installed` · `discovered` · `launchable` distinct; a catalog or registry row is never occupancy. | §5.13; UI/UX skill §8 |
| UI-18 | **No fabricated progress.** A determinate progress affordance reflects a measured value; an unknown is indeterminate-with-label or absent. Never a fake percentage, and never a spinner standing in for a value we do not have. | §3.3, §11; UI-03 |

### 1.3 What already exists (verified) — and two corrections to the evidence base

Verified in this pass:

- **22 viewports** exist (`ViewId`, `ev: ui/src/lib/store.ts:31-57`) and all 22 are labelled once in `VIEW_META` (`ev: ui/src/components/shell/right-rail.tsx:220-243`). **12 center screens** exist (`centerScreen`, `ev: ui/src/lib/store.ts:1211-1223`; render dispatch `ev: ui/src/components/shell/center-column.tsx:49-88`). The prior "19 viewports" note is stale (`ev: evidence §0.3`).
- **The Workbench machinery already exists:** multi-tab strip with HTML5 drag-reorder and per-tab close (`ev: ui/src/components/shell/right-rail.tsx:908-949`), a **live dot only on proven attachment** (`:915-924`), resize clamp **28–70 %** (`:791-811`), full-bleed mode (`:905`), and a narrow-shell bottom strip capped at `min(42vh, 22rem)` with the honest line "Chat stays in the main pane" (`:828-891`, `ev: ui/src/App.tsx:27-31`).
- **The four-region shell already exists** — TitleBar → [LeftSidebar | CenterColumn | ActivityRail | RightViewport] → StatusBar (`ev: ui/src/App.tsx:107-128`) — but the Workbench is gated on power mode (`ev: ui/src/App.tsx:121-122`; G17). The window minimum is 800×600 (`ev: src-tauri/tauri.conf.json:19-20`), so the <900 px shell is a real state, not an edge case.

Two corrections to the evidence base, from reading `ui/` directly:

1. **G20 is stale.** Per-session Workbench layout persistence *exists*: `SessionLayout.openViews` (`ev: ui/src/lib/store.ts:379-387`) is saved and restored per session (`saveSessionLayout` `:3051`, `restoreSessionLayout` `:3065-3085`; writes at `:1859,1868,1930`). It is localStorage-backed view-id persistence, not a Core-side projection. The real remaining gap is **document-level tabs and re-derivation from refs** (§3), not tab persistence.
2. **The tab strip rows are not per-document.** `officePaths` holds exactly one path per office viewport (`ev: ui/src/lib/store.ts:1112`), with `officeHistory` (`:1116`) as a switcher — so two spreadsheets still cannot be open together. G18 stands; the fix is the DocumentSurface tab identity, not more view ids.

---

## 2. Shell layout — far rail · session sidebar · main session · Workbench

### 2.1 Regions

```
TitleBar
RuntimeStatusBanner
┌ FarRail (48px) │ SessionSidebar │ MainSession │ Workbench ┐
StatusBar · CommandPalette · ApprovalStack (transcript-owned) · Toasts
```

| Region | Owns | Ships from today |
|---|---|---|
| **Far rail** | Global destinations: workspace switcher · Home · Activity · Projects · Files · Automations · Library · Settings, plus an "All screens" entry exposing Memory · Guard · Connectors · Analytics · Agents. Collapsed to 48 px, expandable. | The shipped sidebar's icon-rail collapse + nav set Home/Activity/Projects/Files/Automations + Settings (`ev: ui/src/components/shell/left-sidebar.tsx:515-554,639-642`; `ev: ui/DESIGN-SYSTEM.md:43`) |
| **Session sidebar** | Sessions **scoped to the selected workspace**: search, status (running · awaiting approval · budget · failed), fork lineage (`children`/`parentId`), new-session. | The shipped "Recent chats" block + `Session` list (`ev: ui/src/lib/store.ts:272-310`) |
| **Main session** | The transcript, plan bar, composer, approvals; or one of the 12 center screens when the user is not in a session (`ev: ui/src/lib/store.ts:1211-1223`). | `ChatPanel` / center dispatch (`ev: ui/src/components/shell/center-column.tsx:49-88`) |
| **Workbench** | The panel launcher + tab strip: open documents, tools, runs and context beside the session. | `ActivityRail` + `RightViewport` (`ev: ui/src/components/shell/right-rail.tsx`; `ev: ui/src/App.tsx:121-122`) |

**Two structural moves, both stated as proposals pending the open questions they touch:**

- The far rail and session sidebar split the shipped single collapsible sidebar into two panes (global destinations vs workspace-scoped sessions). This is a restructure of existing behaviour, not new surface area → **OQ-UI-011**.
- Nothing is removed. Every destination that exists today still exists, reachable through the rail, the session sidebar, the Workbench launcher, or the command palette (which keeps its exact group set `actions | navigate | chats | views | settings`, `ev: ui/src/components/shell/command-palette.tsx:52`; the `views` group lists the viewports `:89-112` — 20 of 22 today, with `run` and `tool-output` added in v1).

### 2.2 Workbench: a launcher of six slots, not a tab bar of 22 peers

The 22 viewports are a *capability* count, not a *navigation* count (`ev: evidence §5.1`, R43). The launcher offers six named slots; the remaining viewports stay reachable as drill-downs, launcher entries and palette rows.

| Slot | What it opens | Existing implementation to reuse |
|---|---|---|
| **Explorer** | Workspace tree; open file → DocumentSurface tab | `views/folder-view.tsx` + `views/ide/explorer-panel` |
| **Source Control** | Pending patches, review banner, diff lens | `views/diff-view.tsx`, `views/ide/scm-panel`, `views/ide/diff-rail.tsx`, pending-patch banner (`ev: ui/src/components/chat/chat-panel.tsx:592-608`) |
| **Browser** | Guard-mediated browsing | `views/browse-view.tsx`; navigation routes through `openInBrowser` (never a bare webview navigation) |
| **Terminal** | Session terminal | `views/shell-view.tsx` |
| **Runs** | The one run surface + the runs list | `views/run-view.tsx` (+ §6) |
| **Context** | Context inspector (window/usable/current, per-source breakdown, pins, excludes, checkpoint age) | **New panel**; data specified in `ARCH/16-CONTEXT.md` §7; `StreamStats.ctxPct` already exists (`ev: ui/src/lib/store.ts:265-269`) |

Remaining viewports — Progress · Trajectory · Blueprint · Kanban · Audit · Storage · Timeline · Generative UI · Artifact · Computer use · Tool output · Local Server · the four Office views — remain openable from the `+` launcher, the palette, and drill-down links (the `Run` view already routes to them through the store's own `addView`, `ev: ui/src/components/views/run-view.tsx:1-21`; `ev: evidence §1.3`).

**Casual vs power governs control density inside regions, never the existence of a region** (`inf`, H). The gate at `ev: ui/src/App.tsx:121-122` contradicts the owner brief, which treats the Workbench as a permanent fourth region; remove it (R41).

### 2.3 Tab-strip mechanics (keep; extend)

- **Tab identity is the document identity, not the view id** (§3). One tab per identity; reopening focuses the existing tab; N documents per kind.
- **Light refs only.** Persist `{kind, ref}` per session and re-derive content on load — the same principle as the shipped `SessionLayout`, extended from view ids to document refs (`ev: evidence §3.8`, panel-tab-store).
- **Live dot only on proven attachment** — keep the existing rule (`ev: ui/src/components/shell/right-rail.tsx:915-924`) and extend it, provably, to Runs ("a run is executing") and Context ("context is under pressure"). Never "this tab exists" (R44).
- **Drag-reorder, per-tab close, 28–70 % resize, full-bleed** — all already correct (`:791-811,905,908-949`).
- **Disclosure state is shared between transcript and panel.** "Open" for the same object means the same thing in both hosts, keyed once per object (`ev: evidence §3.8`, R44).
- **Close means release the view; it never deletes work.** Every tab can be reopened from the object that produced it.

### 2.4 Narrow shell (< 900 px)

Keep the shipped bottom-strip variant exactly: three tabs, `min(42vh, 22rem)` cap, the copy that chat stays in the main pane (`ev: ui/src/components/shell/right-rail.tsx:828-891`), and the sidebar becomes a drawer (`ev: ui/src/App.tsx:113-118,130-141`). Windows-first plus an 800 px minimum window means this state ships; whether v1 later sets a larger minimum is **OQ-UI-007**.

### 2.5 Shortcuts

The shortcut catalogue and the dispatch chain are two different sources today — one list for display (`ev: ui/src/components/shell/keyboard-shortcuts.tsx:12-60`), an ~20-branch `if` chain for handling (`:93-274`; G24). The catalogue MUST become the single dispatch source: every displayed chord is registered, every handler is data, and no chord exists in one without the other.

---

## 3. Universal DocumentSurface — one surface, one tab model, every content kind

The product has no universal document surface today: each file kind has a bespoke viewport and a viewport holds exactly one document (`officePaths`, `ev: ui/src/lib/store.ts:1112`; G18/G19). v1 introduces **one** tab component that switches on a closed `preview` union.

### 3.1 Contract

```
DocumentRef = { kind: PreviewKind, ref: FileIdentity | ArtifactRef | RunId | Url | SessionId, version? }
PreviewKind =
  | 'code' | 'text' | 'markdown' | 'csv' | 'sheet' | 'document' | 'slides'
  | 'pdf' | 'image' | 'diff' | 'html' | 'webapp' | 'browser'
```

- **One tab per `DocumentRef` identity.** A second open focuses the existing tab. `FileIdentity` comes from `25-FILES`; artifact refs from `29`; a run or session reference opens a pinned inspection tab, not a copy.
- **N documents per kind.** The one-path-per-viewport constraint is removed; document state lives on the tab, not on the view id (`inf`, H).
- **Content is re-derived on load** from the ref; the persisted record is a light ref (§2.3).
- **Existence is pre-checked in the host, never in the render layer.** A missing file renders an inert *not found* row with a re-link action — never a tab that opens onto "file not found" (`ev: evidence §4.1`, R9's `checkFile` reason).
- **Opening and rendering consume zero model tokens** (SPEC §9; `DEC-015`; S-07).
- **Version selector is a toolbar control on the surface**, sourced from the artifact's version list — not a separate screen (R40). Provenance is one expansion away (R37).

### 3.2 Kind → renderer

| `preview` kind | Renderer | Notes |
|---|---|---|
| `code` / `text` | `views/ide/monaco-pane.tsx` + `editor-tabs.tsx` (`ev: ui/src/components/views/ide/`) | Monaco state preserved across tab switches |
| `diff` | `views/diff-view.tsx` (+ `views/ide/diff-rail.tsx`) | same component as the in-transcript diff (§4.5) |
| `markdown` | the chat markdown pipeline (§4.1), `variant="document"` | one renderer, two hosts |
| `csv` | the sheet renderer, read-only | **new kind** relative to the shipped artifact union (`ev: ui/src/lib/store.ts:182`) |
| `sheet` | `views/office-xlsx-view.tsx` | Office runtime underneath (`22`) |
| `document` | `views/office-docx-view.tsx` | Office runtime underneath |
| `slides` | `views/office-pptx-view.tsx` | Office runtime underneath |
| `pdf` | `views/office-pdf-view.tsx` + `pdf-canvas.tsx` | redact must remove content, never annotate (`22` §10) |
| `image` | the existing canvas/lightbox pattern | zoom/pan without internal scroll traps |
| `html` / `webapp` | `views/artifact-view.tsx` + loopback artifact server (`ev: ui/src/lib/artifact.ts:24-51`) | CSP: `frame-src 'none'` bans a frame whose `src` addresses a URL (`ev: src-tauri/tauri.conf.json:27`); HTML preview therefore keeps to the sandboxed `srcdoc` artifact path (§3.4) |
| `browser` | `views/browse-view.tsx` | live dot only while CDP is attached |

### 3.3 Office is a runtime under this surface — not a sidebar mode

`DEC-013` and `ARCH/22-OFFICE.md` §1 name this explicitly, and the UI consequence is precise:

- The four office viewports become renderer implementations behind the surface, selected by `preview` kind — their labels stop being navigation identity.
- **Resident context + exclusive lease** states surface honestly: a document held by another session shows "in use — read-only render or wait", never a silent second writer (`22` §7).
- **Render/validate before commit** shows as status on the surface (validating · validated · validation failed), with the validation result reachable from the receipt (`22` §5, `34`). No fake progress bars.
- Engine limits (`charts/pivots/SmartArt` subset, `22` §2/§9) surface as typed `guidance` copy naming the limitation — never a silent lossy path.
- The operations themselves stay Core-side (L1/L2/L3, `22` §3); the surface only invokes and renders.

### 3.4 Isolation rules (from the security envelope)

- `frame-src 'none'` (`ev: src-tauri/tauri.conf.json:27`) blocks a frame whose `src` addresses a URL — so a preview must not point an in-shell `<iframe src=…>` at the loopback artifact server, and the archived v0 proposal to revive generative UI as a `src`-addressed inline frame MUST NOT be revived. The app's generative sandbox keeps to `srcdoc` (`ev: ui/src/components/views/generative/generative-ui.tsx:166-187`); `artifact-view.tsx`'s real-server branch frames the loopback URL via `src` (`ev: ui/src/components/views/artifact-view.tsx:100-106`), which the envelope does not permit — a gap v1 closes by moving the real-server preview to the `srcdoc`/Browser route. The mermaid path adds no frame at all (blob `<img>`, §4.2).
- `img-src 'self' data: blob: https:` (same line) permits the blob-`<img>` pattern used for diagrams (§4.2).
- **No document or diagram is injected via `dangerouslySetInnerHTML`.** After the mermaid change (§4.2), the only remaining site is the chart primitive (`ev: evidence §4.2` R15).

---

## 4. Chat surface

Anatomy of one assistant turn today, in render order: memory passport → reasoning sub-box → grouped tool drawer → progress steps → markdown body → error card → citations → checkpoint strip → artifact cards → interrupt card → footer (`ev: ui/src/components/chat/message-bubble.tsx:882-985`). v1 keeps this order and fixes the body, the diagrams, the states, the plan and the diff hosting.

### 4.1 Markdown pipeline

| # | Move | What it replaces | Evidence |
|---|---|---|---|
| **R1** | **Two-path renderer behind one component:** a streaming renderer with block-level caching for the in-flight message, `react-markdown` for committed replies. One shared options object; **all plugin arrays and component identities are module-level constants** (an inline arrow silently defeats the streaming cache). | Today: one `react-markdown` path re-parses on every delta (`ev: ui/src/lib/bridge.ts:327`, `ev: ui/src/lib/store.ts:2370-2377`; G6) | `ev: evidence §4.1`; four comparators agree |
| **R2** | **Install `@tailwindcss/typography`** (`@plugin` in `globals.css`) mapped onto the token palette **and** add explicit overrides for `headings · table · blockquote · hr · ol · img`, with tables inside a `role="region" tabIndex={0}` scroll wrapper. | `prose prose-invert` is currently inert (`ev: ui/src/components/chat/message-bubble.tsx:910`; no plugin in `ui/package.json`) — a GFM table renders borderless today, and this is the **single largest chat gap** (G1, P0) | `ev: evidence §4.1` R2 |
| **R3** | **Gate raw HTML off** (`skipHtml`) — defence in depth behind the CSP. | — | R3 |
| **R4** | **Constrain math:** `rehype-katex` with `{ trust: false, strict: 'ignore', maxExpand: 1000, maxSize: 20 }` and a fallback label for a broken expression. | `trust` defaults stop `\htmlClass`-style escapes | R4 |
| **R5** | **Never re-highlight an in-flight code fence.** While a fence is open, render plain monospace; hand it to the highlighter when it closes. | Today `rehype-highlight` re-runs over the growing block per commit | R5 (jan's measured webkit-gtk cost) |
| **R6** | **Coalesce stream writes** to at most one store write per animation frame, keep the raw text exact, expose the committed-prefix length for block caching. | Per-delta store writes today; no batching anywhere | R6 |
| **R7** | **Defer the markdown body** (`useDeferredValue` or equivalent) so typing and scrolling stay responsive under a large stream. | — | R7 |
| **R8** | **Code block anatomy:** keep the language label + copy button; add a wrap toggle, `aria-live="polite"` copy result, 2.2 s reset, explicit failure state — and **replace `bg-zinc-950` + the global dark highlight theme with token-derived surfaces**. | `ev: ui/src/components/chat/message-bubble.tsx:27,72-73` (dark well on a warm-cream canvas; G16, **OQ-UI-008**) | R8 |
| **R9** | **Links are routed, not raw.** Every link in an answer goes through the same Guard-mediated `openInBrowser` the citations already use; file links pre-check existence and render inert with a reason when missing. | Answer links currently open a new window (`ev: ui/src/components/chat/message-bubble.tsx:136-145`) while citations route through `openInBrowser` (`:937-944`); G15, P1 | R9 |

Streaming behaviour to keep: blinking brand caret while streaming (`ev: ui/src/components/chat/message-bubble.tsx:918-920`), auto-scroll that releases the moment the user scrolls up (`ev: ui/src/components/chat/chat-panel.tsx:768-780`), and cheap windowing via `[content-visibility:auto] [contain-intrinsic-size:auto_240px]` (`:779-780`). Whether v1 upgrades to true virtualization is **OQ-UI-010** — `ui/DESIGN-SYSTEM.md:73`'s "virtual scrolling in chat timeline" currently overstates the chat path (`ev: evidence §1.4`).

### 4.2 Mermaid auto-conversion (policy-gated, themed, isolated)

**A fenced `mermaid` code block in the transcript MUST convert automatically; a diagram behind a button does not satisfy the Experience contract** (SPEC §9; R10). The full policy, in order:

1. **Dispatch on `language === 'mermaid'`** in the code renderer, no user action (R10).
2. **Do not render while the fence is open.** Show a `role="status"` line and the source; render when the fence closes — partial diagram source wastes work and flashes errors (R11).
3. **Policy gate before rendering.** Reject >50 000 chars and any source containing front-matter (`---`), `%%{init}%%`, `img:` shapes, `url(`, `@import`, `@font-face`, or HTML other than `<br>` — **after** decoding escapes and stripping CSS comments so the check cannot be bypassed. Reason on record: strict mode disables scripts but *image shapes still create `Image()` during layout* (R12). A rejected diagram stays as copyable source.
4. **`securityLevel: 'strict'` then lock the directives.** Initialize with `{ startOnLoad: false, securityLevel: 'strict', suppressErrorRendering: true, htmlLabels: false, theme: 'base', themeVariables: <from tokens>, maxTextSize: 50_000, maxEdges: 500, secure: [...] }` where the `secure` array names `securityLevel`, `htmlLabels`, `themeCSS`, `themeVariables`, `dompurifyConfig`, `maxTextSize`, `maxEdges`, `fontFamily` and the flowchart config. Including `dompurifyConfig` matters: source directives can otherwise reach it (R13).
5. **Theme from the live tokens.** Read `--brand`, `--foreground`, `--surface-*`, `--border` via `getComputedStyle` into `themeVariables`, resolve dark mode from the appearance state, and observe the `data-theme`/accent attributes so a theme or accent change re-renders (R14). The shipped renderer hardcodes `theme: 'dark'` and never re-inits (`ev: ui/src/components/views/generative/generative-ui.tsx:300`; G3).
6. **Render to a blob and show it as an `<img>`.** `URL.createObjectURL(new Blob([svg], {type:'image/svg+xml'}))`, revoked on unmount — "an image, not live SVG/HTML: no scripts, click handlers or bound callbacks" (R15). This is CSP-compatible (`img-src … blob:`), and it removes the `dangerouslySetInnerHTML` diagram site (G4).
7. **Reserve the size to keep CLS at zero.** Parse the `viewBox` into the `<img>`'s width/height; on re-render keep the previous image visible rather than collapsing to a placeholder (R16).
8. **Serialize and bound.** Mermaid configuration is process-wide: serialize through a module-level promise queue, debounce the render, cap `maxEdges`/`maxTextSize` (R17). No web worker in v1 (no comparator uses one).
9. **Failure is a state.** `role="status"` failure line + copyable source + explicit retry (R18).

**Two surfaces, two economics.** The *Generative UI* viewport keeps its token-cheap static preview ("Mermaid diagram — click 'Make live' to render") gate (`ev: ui/src/components/views/generative/generative-ui.tsx:263-271`); *in the transcript*, the diagram is part of the answer and renders automatically because conversion is local (R19). Both are token-free; only one is automatic.

### 4.3 Tool-call component model

**Keep the shipped model — it is better than most competitors** (R20). Keep: sentence-first labels that never echo the tool id, with the neutral fallback sentence (`ev: ui/src/components/chat/tool-chip.tsx:19-136`); the grouped drawer for 2+ calls (`:491-568`); CLI/ANSI normalization at the drawer boundary (`:189-213`); risk chips (`:171-177`); per-call retry (`:475-484`); `@specialist` attribution on the row (`ev: ui/src/lib/store.ts:92-94`; `tool-chip.tsx:388-397`); spooling above `SPOOL_TOKEN_BUDGET = 2000` with "Inspect in Right Rail ↗", inline expand and copy (`:215-311`).

Changes:

| # | Rule | Evidence |
|---|---|---|
| **R21** | **Normalize the state vocabulary to five — `proposed · running · succeeded · failed · cancelled` — and name them once.** `proposed` is required because Guard approvals now sit between proposal and execution (`03-HLD.md` §5). The event vocabulary already distinguishes the transitions (`30` §3: `tool.proposed`, `tool.started`, `tool.progress`, `tool.completed`). Today the union is three (`'running' | 'done' | 'failed'`, `ev: ui/src/lib/store.ts:85`; G8). | R21 |
| **R22** | **One rail per turn.** Steps render as one rail — leading slot, sentence label, duration right — and a settled turn collapses to `Worked for 1m 19s · 12 steps`. Inputs already exist: grouped wall-clock total and per-row durations (`tool-chip.tsx:523-530,139-148`). | R22 / T1 |
| **R23** | **A group containing an error MUST NOT auto-collapse.** Today the open/closed effect keys only on `settled` (`ev: ui/src/components/chat/tool-chip.tsx:499-507`), so a failed settle collapses the failure out of sight (G9). Forced-open on error or focus, with an explicit-user-toggle guard. | R23 |
| **R24** | **Group identity is not positional.** Key a group on its first item's identity so "user expanded group 2" survives regrouping. | R24 |
| **R25** | **Per-tool renderers via a registry, not a switch** — distinct result shapes (patch → diff, shell → terminal, read/write → file chip, search → result list, ask → form, todo → checklist) get a registered renderer; everything else falls back to the generic sentence row. The ~700-line type switch in the comparison set is the anti-pattern. | R25 |
| **R26** | **Big payloads spool; the full body resolves on demand.** Keep the 2 000-token spool card and add lazy full-payload fetch so a collapsed row can open the complete record without it riding the transcript. | R26 |
| **R27** | **The transcript never becomes a scroll trap.** Inline widgets fit one scroll of the response, carry ≤2 primary actions, never scroll internally; escalation to the Workbench is an explicit panel button, not a growing widget. | R27 / S4 |

**Streaming is `running`, not a sixth state.** A call still emitting output — a CLI stream or a partial result — stays `running` and carries `tool.progress` events (`30` §3); it does not gain a separate `streaming` member, so the vocabulary stays the five names above and the icon/border treatment has exactly one live look. `running` on a settled turn with no terminal event is itself a defect (UI-03).

Blocked ≠ error: a Guard-denied call renders neutral ink + lock with the reason; red is reserved for failures (UI-04).

### 4.4 Reasoning and the plan

**Reasoning (R28/R29/R30).** Reasoning is a first-class, closed-by-default, timer-bearing disclosure. The shipped behaviour is already close to correct: auto-opens only on the live transition, unconditionally auto-collapses when the turn settles, tabular live timer, reserved `min-h-[24px]` (`ev: ui/src/components/chat/message-bubble.tsx:166-239`; `:197`). Add:

- **Explicit-override memory:** once the user has opened or closed it for a turn, the auto-toggles stop for that turn.
- **A persisted "hide reasoning entirely" preference**, alongside the existing preference style (G12).
- **Section breaks are signal-driven:** "Thought N" separators come from the reasoning section markers, not from chunks (`message-bubble.tsx:220-235` today renders one per array item; `30`/provider vocabulary models the break explicitly — R30).
- **Never render raw chain-of-thought as prose** (UI-14). The UI renders only the `reasoning` projection; a raw variant belongs behind a named *Technical details* disclosure, exactly as raw tool payloads do (`tool-chip.tsx:415-456`).

**Plan bar (R31).** The clearest missing pattern (G7): plans render today only in the Blueprint and Kanban viewports (`ev: ui/src/components/views/blueprint-view.tsx:99-109`; `ev: ui/src/components/views/kanban-view.tsx:58`; `evidence §1.4`), and the `plan` interrupt kind is unreachable in the card (kind exists at `ev: ui/src/lib/store.ts:211`; the card's branches cover only `diff | autonomy | permission | mcq | budget`, `ev: ui/src/components/chat/mcq-interrupt-card.tsx:284-347` — verified no `plan` branch; G38).

The plan bar MUST:

- sit **above the composer**, matching the composer's width class (watch the content-box padding trap — no horizontal padding on the root);
- bind to the **running turn** (`turn_id === activeTurnId`); render nothing, and reserve nothing, when no plan belongs to this turn, so a finished plan never hangs over the next one;
- be keyboard-operable (`role="button"`, `aria-expanded`, Enter/Space);
- cap its open height at `min(22vh, 180px)`;
- show `completed/total` in the header;
- on a **new plan version** for the same turn, report the **delta** in the header (`2 started · 1 completed · 1 removed`), highlight rows whose status changed, keep the full list collapsed — and never claim "no change" when the previous list is unknown;
- draw step status with the **existing** `ProgressStep` vocabulary (`done | active | pending | failed`, `ev: ui/src/lib/store.ts:197-205`), not a new one.

**Verification drill-down (R32).** `VerificationRecord` already carries `{ taskId, checks[], report, passed: boolean | null }` with `null` meaning ambiguous and never claimed as executed (`ev: ui/src/lib/store.ts:253-263`). Render it as a drill-down from the plan step it verifies.

### 4.5 Diffs

- **One diff component, two hosts** (R33): inline in an approval card when the decision *is* the diff; as a Workbench tab when the work is *reviewing* the diff. Today `DiffView` is in the MCQ card (`ev: ui/src/components/chat/mcq-interrupt-card.tsx:284`) with a separate viewport — two hosts are right, two implementations would not be.
- **Per-hunk review is a promise, not a decoration** (R34). Today the product states the truth: per-hunk Keep/Reject and checkpoint restore are gated on an undo-restore command surface that does not exist yet (`ev: ui/src/components/chat/chat-panel.tsx:592-608`). Either build the command surface and ship per-hunk, or ship honest whole-file accept/reject and say so. The house rule (UI-03) forbids pretending.
- **A diff is reachable from the tool row that produced it** (R35): a patch call offers the same "open in the Diff lens" jump that spooled output offers as "Inspect in Right Rail ↗" (`tool-chip.tsx:266-273`, `:475-484`; `run-view.tsx` drill-down routing).
- **Review mode is a state, not a modal** (R36): "this turn changed 4 files" is a per-turn summary the surface can show without a second diff pass.

### 4.6 Artifacts in the transcript

Artifact cards keep their shape: type-coloured cards in a responsive grid with entrance stagger (`ev: ui/src/components/chat/message-bubble.tsx:957-966`; `artifact-card.tsx`), an optional action checklist, a live loopback preview for webapps, and **honest figures** — absent figures render nothing rather than a guess (`ev: ui/src/lib/store.ts:191-194`). The artifact type union is extended toward the single preview union used by the DocumentSurface (`browser`, `html`, `external` join the eight shipped members; R39), and each card gains the promotion action of §7.

**Collection is deterministic** (R38): references are derived by URL/file regexes, markdown links and a known-tool-name set — the `OpenTarget` model with `{ kind, value, name, preview, confidence, reason }`, where only preview kinds in the fixed set are collected (`ev: evidence §3.8`). No model call, ever (UI-02).

---

## 5. Composer

### 5.1 Anatomy (left → right)

`+` attach/create · `@` entity reference · **agent picker** · **model picker** · **reasoning dial** · **context indicator** · `Run ▾` · one round send/stop button (R49; T5). Two footer invariants stay: the reserved fixed-width slot that never moves when state changes (`ev: ui/src/components/chat/chat-composer.tsx:929-965`) and the one-table send gate.

### 5.2 Send gate

Keep the shipped gate verbatim: the five codes `empty · unbound · readiness-unknown · not-ready · preview` (`empty` is the pass), the four refusing cases each refusing **before anything moves** with a plain-language reason and an actionable next step (`ev: ui/src/components/chat/chat-composer.tsx:70-163`, `:617-645`). The gate is the reason the chat bar can never look sendable and then quietly refuse — it is a product invariant, not an implementation detail.

### 5.3 Agent and model pickers — ownership unchanged

Keep the ownership rule exactly (R50): the trigger paints the **agent-owned** model value or an explicit em dash (`ev: ui/src/components/chat/agent-model-picker.tsx:474-489`); the picker lists the agent's own ACP config options; selection is **installed-only**, and catalog-only rows render an explicit not-installed state with an install affordance instead of becoming selectable (`:526-549`). Governance badges keep their three kinds and tooltips: Governed-Mediated (green) · Self-contained (amber) · Not Governed (red) (`ev: ui/DESIGN-SYSTEM.md:58`). A model switch during a live stream applies to the next turn and says so (comment `:511-516`). Where the agent reports no model surface, the picker says "managed by \<agent\>" — the platform never invents a model list (`ev: ui/DESIGN-SYSTEM.md:11`).

### 5.4 Reasoning dial — capability-negotiated, model-derived

Adopt the normalized dial of `ARCH/18-MODEL-ROUTING.md` §5: **`auto · minimal · low · medium · high · extra_high`**, rendered **only** at levels the model's descriptor supports; the value clamps on model switch; an unsupported model says so rather than showing a dead control (R51). Slider semantics: `auto` is not on the track (it has no ordinal position — its own row); `off`-equivalent levels sit at the far left; the pill stays visible even at `auto`; the header label tracks live while dragging; one description line states what the current level means.

**The data is already on the wire.** `AcpConfigOption` carries `category`, `type: 'select' | 'boolean'`, `currentValue` and `options` (`ev: ui/src/lib/acp.ts:93-101`), and the picker currently reads **only** `category === 'model'` (`ev: ui/src/components/chat/agent-model-picker.tsx:482`; G26). The reasoning dial is therefore a rendering build, not a protocol build. Persistence scope (per turn / session / agent) is **OQ-UI-004**.

### 5.5 Context indicator

A compact readout in the composer footer (today `StreamStats.ctxPct`, `ev: ui/src/lib/store.ts:265-269`) that opens the **Context inspector**: window / usable / current, per-source breakdown, pinned, excluded, recent checkpoint age; actions are **focus · pin · exclude · inspect**, and "optimize now" invokes the controller's decision (prune / compact / no-op) — it never forces compaction (`16` §7). The same inspector is the Workbench **Context** slot (§2.2, fixing G21).

### 5.6 `@` — a structured `EntityReference` picker

Replace raw-text refs (`splitAtRefs` regex, `ev: ui/src/lib/at-refs.ts:4-13`; G27) with a typed reference the composer keeps in its draft model and serialises at send:

```
EntityReference =
  | { kind: 'file';       path; range?; exists?; size? }
  | { kind: 'folder';     path }
  | { kind: 'session';    sessionId }
  | { kind: 'run';        runId }
  | { kind: 'artifact';   artifactId; version? }
  | { kind: 'url';        url }
  | { kind: 'connector';  connectorId; objectId? }
  | { kind: 'terminal';   ptyId; lastCommand? }
  | { kind: 'memory';     memoryId }
```

Why it must be structured: a chip is removable without regex surgery; the ACP `resource` block is emitted only for kinds the agent advertised `embeddedContext` for (`at-refs.ts:1-4` documents the current dual behavior) without re-parsing prose; existence can be pre-checked through the host (never by touching disk in the render layer); and a reference can point at an artifact, run or session — which a path regex cannot express (R52). The picker is keyboard-complete: type-ahead, arrow navigation, Backspace removes the last chip, and each chip is announced by its kind. The legacy `@files`/`@terminal` literals (`ev: ui/src/components/chat/chat-composer.tsx:58-61`) are removed with the string path — **OQ-UI-006**.

### 5.7 `/` — one reserved host namespace, agent grammar untouched

**Normative rule (R53):** the host reserves exactly one namespace — **`/eaios:*`** (SPEC §9; `ARCH/02-THESIS.md:43`) — for its own commands. Every other `/name` is the bound agent's native vocabulary, forwarded verbatim as prompt text with no local interception. The shipped behaviour is already right in substance: while an agent is bound, the local table is hidden and the list comes from the agent's live `available_commands` (`ev: ui/src/components/chat/chat-composer.tsx:544-557`; comment `:513-518`; G28 covers the missing host half).

Two additions:

1. **The host namespace must be visible even while the agent's vocabulary is shown** — a labelled second group (`Host commands`), never an override, or the user can never discover them.
2. **Fuzzy ranking stays** — the shipped `fuzzyRank` matcher (`ev: ui/src/lib/fuzzy.ts`; used at `:554`) is the right behaviour for both groups.

The final token renames with the brand at freeze (tie: `OQ-003`, `OQ-001`); until then `/eaios:*` is the reserved spelling because SPEC §9 names it.

### 5.8 `+` attach / create

Keep the honest refusal behaviour: attachments are read-classified and validated, and kinds the wire cannot carry are refused **with the reason** and "nothing was attached" (`ev: ui/src/components/chat/chat-composer.tsx:573-609`; G29). Widen the menu to the kinds the Workbench can display — **file · folder · image · screenshot · URL · current Workbench selection** — with "current selection" making the composer feel connected to the pane beside it. Add a *create* group for what the platform can already mint: new session, and **Save this conversation as a workflow** when the agent authored one (`15` §6; `DEC-008`). Attachments are a **list**, not a single slot (G31).

### 5.9 `Run ▾` — four scheduling intents

Split the single send affordance: **Run now** (default, Enter) · **Run in background** · **Run as workflow** · **Schedule…** (R55). Rationale: a chat turn, workflow run, background job and scheduled automation are all `Work` with a lifecycle (SPEC §3; `DEC-003`), so one button cannot express four intents; the current "queue while busy" behaviour is the degenerate case of Run in background. Lanes and limits are Core's (`DEC-031`); the UI chooses the intent and shows the resulting kind/status, never its own queue. Whether Run-in-background is a new `Work` or the same `Work` continued is **OQ-UI-005** (`11`).

### 5.10 Approvals stack — above the composer, four choices, fail-closed

- The consent card is the focal element while a decision is pending and sits **where the user acts — above the composer** — naming action · data · risk (T4; R56).
- Vocabulary: **`once · session · always · deny`**, with the available choices driven by what the backend will honour; "always" is hidden when the backend would not honour a permanent grant, and when offered it opens a **second confirmation** because it persists (R56). These are the ticket-scope choices of the one approval primitive; the recorded decision set (`approve/reject/edit/provide-data`, `DM-010`) and the policy enum that decides *when* to ask (`ARCH/12-TRUST.md` §2/§5) are the backend vocabulary this card maps onto.
- Outcomes map onto the fail-closed set — `allowed-once · rejected · cancelled · unavailable` — and **`unavailable` never reads as a grant**.
- The **transcript owns the queue**, not each execution row; execution rows never mount or re-home it (R56). Approvals route through the channel that owns the binding and stay durable if the channel disconnects (`32` §7/§8).
- The shipped `MCQInterrupt` already carries a Guard approval nonce and urgency (`ev: ui/src/lib/store.ts:207-222`); v1 **splits the one card into per-decision-kind components** — the current single card covers six kinds and its `plan` branch is unreachable (G38). Diff decisions use the shared diff component (§4.5); autonomy limits keep the frozen per-task snapshot semantics (`ev: ui/src/lib/store.ts:235-252`).

### 5.11 Work mode and autonomy

Keep the shipped split: casual shows one simple autonomy dial; power shows `WorkModeChip` (Auto · Plan · Build · Research) + `AutonomyChip` (Sandbox · Ask · Auto · Maximum) (`ev: ui/src/components/chat/chat-composer.tsx:44-49,193-198`). The per-task autonomy snapshot is frozen at task start with a `configHash` so a live chatbar change never mutates in-flight work (`ev: ui/src/lib/store.ts:235-252`) — the UI MUST continue to say when a change applies to the *next* turn.

### 5.12 Mic and telemetry honesty

The microphone capture is real but the missing speech-to-text engine is reported instead of a fabricated transcript (`ev: evidence §1.5`; `ev: ui/src/components/chat/chat-composer.tsx:850-886`): keep that honesty (UI-03). The telemetry footer shows web-search state in its reserved slot plus token counts in mono/tabular (`ev: ui/src/components/chat/chat-composer.tsx:929-966`).

### 5.13 Agent configuration — agent-owner model, two-pane runtime surface, Windows-first provenance

The composer's agent control is the compact expression of the agent-owner model; the configuration surface behind it is where provenance and readiness are told truthfully. This subsection completes §5.3 (ownership unchanged); it adds the surface and the Windows-first rules the shipped picker does not yet state.

- **Compact control set.** Power mode: `Agent ▾ → agent-owned Model ▾ → Work Mode ▾ → Autonomy ▾` (§5.11); casual keeps one plain dial and no identity stack, matching the shipped split. The identity readback a casual user sees lives in the status bar, not the composer (`ev: ui/src/components/chat/chat-composer.tsx:795-797`).
- **Opening agent configuration expands to a responsive two-pane / full-screen surface** — never a flat tool dump and never a second picker. Left: the installed / discovered runtimes for this workspace; right: the *selected* runtime's own model / auth / native-capability surface plus the AgentCowork shared grants. AgentCowork Native uses the AgentCowork provider / catalog / local surface; an external ACP agent shows only what it exposes through `configOptions`, or the explicit "managed by \<agent\>" state — **a Native model is never offered for an external runtime** (`ev: ui/DESIGN-SYSTEM.md:11`; §5.3).
- **Provenance is a discriminated location, not a path string.** A runtime row renders exactly one of: `managed` (an AgentCowork-pinned absolute executable) · `windows_path` (`.exe`/`.cmd`/`.bat` from the effective PATH) · Windows **App Paths** · `user_path` · `package_manager` (`npx`/`uvx`) · `wsl` (distro + Linux path + `wsl.exe` launcher) · `unavailable`. **A catalog or registry row is never occupancy** (UI-17).
- **Three facts never collapse:** `installed` (a managed install or package-manager readiness) · `discovered` (a verified location exists, e.g. WSL-only) · `launchable` (the current adapter can actually start it). A `wsl` row is *discovered* but not *launchable* until its spawn adapter exists — render it non-selectable with an honest reason, and never hand a Linux path to `CreateProcess` (`ev: ARCH/06-DATA-MODEL.md` DM-014 `installed/available/disabled`; `ARCH/13-CAPABILITY.md` §6 effective set).
- **Session capability loadout, not a tool dump.** MCP servers, skills, plugins, connectors, Office, Browser, Computer Use, artifacts and memory are session rows carrying `enabled` · `health` · `scope` · `source` · `native_or_shared` · `applies_from`; a change applies to the next turn/run and is frozen into the Work manifest — it never silently changes an in-flight run and never dumps every schema into the prompt.
- **Vault reuse is a launch-time binding.** "Use key from AgentCowork vault" shows environment-variable **names**, never values, and MUST NOT write the external agent's own config file (`INV-02`).
- **Readiness is evidence-gated.** Office / Browser / Computer Use / Memory / MCP / skills / plugins stay `unverified` (or `available`) until a real Windows acceptance record exists; a mock row, browser preview, static catalog entry, or unit-only Office test is **not** pass evidence. Every status colour carries an accessible label, never colour alone (UI-04, UI-18).

**Sources:** the workspace UI/UX skill (`.agents/skills/ui-ux/SKILL.md`, section 8) and the Windows-first release target; `ARCH/12-TRUST.md` §8 (projection-only), `ARCH/13-CAPABILITY.md` §6, `ARCH/32-CHANNELS.md` §3, `ARCH/43-GLOSSARY.md` (Catalog).

---

## 6. Runs and subagents

### 6.1 One run surface, a list, a tree

- **Runs list** exists as a gap today: the `run` viewport reads only the active session (`ev: ui/src/components/views/run-view.tsx:53-70`; G22). v1 adds the **work-tree list** (ongoing + recent, filterable by kind/status/workspace) and keeps the one-run surface as the aggregation projection it already is: identity → context/usage → ordered trace → artifacts → working folder → MCP servers (`run-view.tsx:1-21,50-51`), with every derivation a **pure projection** where an unknown value stays `null` to the screen (`run-projection.tsx:1-16`; UI-03).
- **Drill-down lenses stay where they are** — progress · trajectory · diff · artifact · tool-output remain reachable from `Run` through the store's own `addView`, never duplicated (R46).
- **Nesting shows only where nesting is real.** A child (worker) renders inside its parent's step when it exists as a unit of work; the parent's view of a worker is its **worker receipt** — status · scope · summary · findings · changed files · tests · artifacts · blockers · confidence · usage (`DM-016`, `DEC-029`) — never its transcript.

### 6.2 Status vocabulary, one mapping

Display statuses derive from `Work.status` (`queued → running → waiting | paused | awaiting_approval → completed | failed | cancelled | expired`, `11` §2) and, for delegated workers, from the single **subagent label machine**: `waiting-permission · waiting-question · waiting-result · waiting-start · retrying · no-new-activity · reconnecting · shimmer · failed · completed`. Rules on record: a pending ask is the actionable state and is not a function of stream health; a lost connection only downgrades the live treatment (R47). Status colours use the semantic tokens; blocked uses neutral + lock (UI-04).

### 6.3 Live tree and the optional DAG

- The **live tree** is the default: parent work → children workers with per-worker rows, elapsed (mono/tabular), live/done/failed, and expandable detail surfaces (trace · changed files · tests · artifacts · blockers · usage · receipts). This satisfies "one Runs surface" for S-01.
- An **optional DAG** exists only where dependency structure is real, and it reuses the existing DAG projection (`CuaDagLayout`, `dependsOn`, ready frontier, `replanSeq` — `ev: ui/src/lib/cua-dag.ts:1-45`) rather than growing a second layout engine. **The transcript stays linear** (UI-15; R46): a conversation with a DAG in it stops being a narrative.
- **Attribution is per step** (R48): the shipped `@specialist` stamp on tool rows stays and extends to runs — a delegated run names its delegating parent.

### 6.4 Work verbs and budgets

`interrupt` (stop the current step, keep the session) · `cancel` (terminate the work) · `dispose` (release the environment) are three distinct actions with distinct copy (`11` §6). Budget exhaustion **pauses and surfaces** — it never silently overruns (`11` §5, `DEC-031`); the surface shows used/cap with honest numbers only. The review queue is *our own* product-layer build if it ships at all (`DEC-029`; **OQ-UI-013**).

---

## 7. Artifacts vs Library

Three objects, three names, one lifecycle (R37; `DEC-014`; `43-GLOSSARY.md`):

| Object | What it is | Where it appears | Model |
|---|---|---|---|
| **Reference** | A file/URL the turn *touched*, discovered deterministically | Transcript footnotes and inline file chips; DocumentSurface tabs via its `preview` kind; `@` picker | `{ kind, value, name, preview, confidence, reason, exists?, size?, updatedAt? }` (deterministic collector; R38) |
| **Artifact** | The *output of this run* — versioned, provenance-carrying | Transcript artifact card; version toolbar on the DocumentSurface; Run → artifacts section | `DM-019`; versions immutable; `parent_artifact` lineage |
| **Library item** | A *reusable inventory* entry — promoted explicitly | Far rail → **Library** (global): saved artifacts, skills, workflows, templates, prompts, connectors, plugins, agents | `DM-023`, `29` §4 |

**Promotion lifecycle (explicit, never automatic — `DEC-014`, `INV-18`):** artifact card → **Save to Library** (or "Save as template") → Library item carrying `saved_from_artifact_id` → usable in later sessions/workspaces. Un-promotion/deprecation is an explicit operation. Version selection and provenance are properties of the surface (R40), not dialogs; receipt-pinned versions are never GC'd (`DEC-032`), and the UI shows provenance chain and promotion origin one expansion away.

**Naming.** "Library" is already defined as the reusable inventory (`43-GLOSSARY.md:39`); the Settings surface that shows local model weights must stop owning that word (its tab is literally named `library`, `ev: ui/src/components/panels/local-models-panel.tsx:97`; G33). Proposal: keep **Library** = inventory; rename the weights surface **Local models**. This is **OQ-UI-001** because it is copy.

---

## 8. Token discipline — operations that must never call a model

`DEC-015` / `INV-13`, restated as a UI contract (SPEC §9). Every row is already local today; the rebuild must keep it that way and add an assertion.

| Operation | Why it must be local | Existing evidence |
|---|---|---|
| Rendering a message, list, table, diff | Pure function of already-fetched state | `ev: ui/src/components/views/run-projection.tsx:1-16` |
| Navigation, panel open/close, tab switching | A keystroke must not wait on a model | `ev: ui/src/App.tsx:107-128` |
| Diagram conversion (fenced `mermaid` → SVG) | Deterministic parse + render | `ev: ui/src/components/views/generative/generative-ui.tsx:295-347`; static-preview gate `:263-271` |
| Syntax highlighting, math, tables | Deterministic | `ev: ui/src/components/chat/message-bubble.tsx:911-917` |
| Preview-kind detection / reference discovery | Deterministic from extension, MIME, tool name | `ev: evidence §5.4` (OpenTarget) |
| Draft goal → task list | Deterministic splitter already exists | `ev: ui/src/lib/plan-draft.ts:1-45` |
| Error explanation | Static label table | `ev: ui/src/lib/errors.ts`; `evidence §5.4` |
| Plain-language status rewrites | Static tables | `ev: ui/src/lib/plain-language.ts`; `tool-chip.tsx:19-136` |
| Title generation · summarisation · "what next" | **May** use a model — only as explicit opt-in with visible cost | `ev: evidence §5.4` |

Additionally: **a preview must not silently spend budget.** The bridge already has an explicit `preview` branch that refuses to forge live state (`ev: ui/src/lib/artifact.ts:31-39`); v1 makes "this operation is local" a first-class assertion on that branch, so a future refactor cannot quietly turn a render into a model call (`inf`, H).

---

## 9. Accessibility, motion, typography, accent — the current foundations, completed

### 9.1 Accessibility (WCAG 2.2 AA, keyboard-complete)

Existing: `focus-visible` ring on every interactive using the ring token (`ev: ui/src/globals.css:791-795`); high-contrast mode (`:800-811`); font scaling (`:819-821`); density retuning `--spacing` (`:831-833`); RTL logical-property pass (`:838-845`); Radix focus traps; pervasive `aria-*` (179 uses across chat + shell, `ev: evidence §1.7`) including `aria-live` on tool status (`ev: ui/src/components/chat/tool-chip.tsx:554`) and `aria-expanded` on disclosures (`:545`).

Required in v1:

- **Delete the dead orange focus rule.** `globals.css:767-770` sets `outline: 2px solid hsl(19 100% 48% / 0.55)` — the retired orange as a literal, which the utility-alias quarantine cannot reach — and is then overridden by the correct rule. Two rules, one pointing at a banned colour.
- **An automated accessibility gate** — none exists (`ui/scripts/` contains only `build.mjs` + `check-monaco-dompurify.mjs`; G25). Colour contrast is machine-checked (`design-tokens.test.ts`); focus order, announcements and keyboard reachability are not. A CI a11y pass (axe or equivalent) is the missing half of the WCAG claim.
- **Keyboard models named once:** Workbench = a real tablist (`role="tab"`/`tabpanel`; the narrow strip already uses `role="tabpanel"`, `ev: ui/src/components/shell/right-rail.tsx:854`), transcript = disclosures with Enter/Space, composer = combobox semantics for `@`/`/`, IME-safe Enter (shipped: `isComposing || keyCode === 229`, `ev: chat-composer.tsx:803-848`).
- **`aria-live` discipline:** streaming text is not announced token-by-token; status changes are (`polite`), approvals are (`assertive`-adjacent through focus placement, not repetition).

### 9.2 Motion

The shipped `@utility` vocabulary is the vocabulary — `live-pulse` 1 s · `enter-approval` 250 ms · `enter-step` 100 ms · `enter-surface` 150 ms · `cell-flash` 200 ms · `scale-in-palette` 120 ms · `step-shake` 150 ms · `chart-crossfade` 400 ms · `toast-enter` 200 ms · `treemap-morph` 300 ms · `spark-draw` 300 ms · `score-roll` 300 ms · `agent-switch-pulse` 200 ms · `enter-stagger` (per-index via `staggerStyle(i)`) · `widget-enter` 300 ms, plus the blanket 150 ms press rule with `scale(0.98)` (`ev: ui/src/globals.css:627-765`). Reduced motion kills all animation and hides the caret (`:772-781`).

Rules: reuse utility names rather than new keyframes; **damped springs** (Framer Motion, already a dependency) only for the few high-impact moments — message entrance, streaming indicator, rail width (shipped at 300 ms `cubic-bezier(0.4,0,0.2,1)`, `ev: ui/src/components/shell/right-rail.tsx:895-906`); no entrance animation on high-frequency surfaces; exit faster than enter; shimmer only on the currently-running step (UI-11).

### 9.3 Typography

Inter for UI text, **JetBrains Mono + `tabular-nums` for every telemetry value** — durations, token counts, timers, percentages, ids (`ev: ui/src/globals.css:119-120`; `message-bubble.tsx:202,209`; `tool-chip.tsx:139-148,252-254,387`; UI-10). Chat type scale stays 11–13 px with `leading-relaxed`: body 12 px (`message-bubble.tsx:132`), lists 12 px (`:127`), inline code 11 px (`:59`), code block 11 px (`:99`), chrome 9–10 px (`:202`), composer 13 px (`chat-composer.tsx:846`). Hierarchy comes from two or three size steps plus weight/opacity — do not add steps. The R2 typography mapping defines headings/tables/blockquotes inside the answer body; nothing else changes.

### 9.4 Accent and colour

- **Brand is a semantic, user-selectable token** — cool-blue `--brand` default, six alternates, each tuned twice and contrast-tested (`ev: ui/src/globals.css:130-133,244-345`; `ev: ui/src/lib/design-tokens.test.ts:153,178`). New components MUST read `--brand` and MUST NOT assume blue (C3).
- **Orange is never brand or selection.** The retired orange family is aliased to `--brand` (`globals.css:63-72`) and `amber`/`yellow` to `--warning` (`:73-92`), with semantic utilities `bg-brand`/`text-warning`/`border-success` (`:100-105`).
- **The label on the accent uses `--brand-foreground`**, which flips to dark ink in dark mode because a bright accent plus white cannot reach AA (C4).
- **Ten literal-orange values survive in the shipped stylesheet** and render orange today, contradicting the "no longer reachable" claim: `globals.css:525` (`.gradient-border::before`), `:534-535` (`glow-pulse`), `:573` (`.hover-lift:hover`), `:578` (`.border-glow:hover`), `:583` (`.bg-radial-fade`), `:599` (`.accent-top-gradient::before`), `:729-730` (`agent-switch-pulse`), `:767-770` (the duplicate focus rule) — **fix in the hygiene slice; one of them is also dead code** (G36).
- **Code blocks**: replace `bg-zinc-950` and the global dark highlight theme with token-derived surfaces, or explicitly bless the dark well (G16; **OQ-UI-008**).
- **Fonts are fetched from Google Fonts** (`ev: ui/index.html:11-14`): a first-paint network dependency and fallback-font CLS risk for a local-first app → **OQ-UI-009**.

---

## 10. Gap analysis and priorities

All rows: `ev: evidence §6` unless a direct read is noted. Priority: **P0** blocks the chat-surface claim · **P1** required for coherent v1 · **P2** quality · **P3** later.

### Chat surface

| # | Gap | Pri | Note |
|---|---|---|---|
| G1 | Headings/tables/blockquotes/`hr`/`ol`/`img` unrendered; `prose prose-invert` inert | **P0** | Largest single gap; fix = R2 |
| G2 | No mermaid conversion in chat | **P0** | Only renderer is a Workbench viewport (`generative-ui.tsx:295-347`, verified) |
| G3 | Mermaid hardcodes `theme: 'dark'`, never re-inits | **P0** | Verified `generative-ui.tsx:300` |
| G4 | Mermaid via `dangerouslySetInnerHTML` | **P0** | Verified `generative-ui.tsx:341` |
| G5 | No mermaid render-policy gate / `secure:` lock | **P0** | §4.2 |
| G6 | Markdown re-parses and re-highlights per delta; no coalescing/deferral/fence gate | **P0** | R5–R7 |
| G7 | No plan bar; plans render only in two right-rail viewports | **P0** | R31 |
| G8 | Tool-call state union lacks `proposed`/`cancelled` | P1 | R21 |
| G9 | Failed tool group auto-collapses | P1 | Verified `tool-chip.tsx:499-507`; R23 |
| G10 | No per-tool renderer registry | P1 | R25 |
| G11 | No explicit override memory for reasoning toggles | P2 | R28 |
| G12 | No "hide reasoning entirely" preference | P2 | §4.4 |
| G13 | No encoded "never raw CoT as prose" rule | P2 | UI-14 fixes this doc-side |
| G14 | Per-hunk Keep/Reject not implemented (correctly labelled) | P1 | R34 |
| G15 | Answer links bypass the Guard-mediated route citations use | P1 | Verified `message-bubble.tsx:136-145` vs `:937-944`; R9 |
| G16 | Code blocks dark-only, non-token zinc | P2 | OQ-UI-008 |

### Shell / Workbench

| # | Gap | Pri | Note |
|---|---|---|---|
| G17 | Workbench hidden in casual mode | **P0** | Verified `App.tsx:121-122`; R41 |
| G18 | One document per viewport | P1 | Verified `store.ts:1112`; §3 |
| G19 | No universal DocumentSurface | P1 | §3 |
| G20 | "Tabs not persisted per session" | — | **Corrected: persistence exists** (`store.ts:379-387,3051-3085`); the real residue is document-level identity (§1.3) |
| G21 | No Context panel | P1 | §2.2/§5.5 |
| G22 | No Runs list; `run` reads only the active session | P1 | Verified `run-view.tsx:53-70`; §6 |
| G23 | 22 peers, no six-slot launcher | P2 | §2.2 |
| G24 | Shortcuts: display catalogue ≠ dispatch chain | P2 | §2.5 |
| G25 | No automated accessibility gate | P2 | §9.1 |
| G26 | No reasoning dial (data already on the wire) | **P0** | Verified `acp.ts:93-101`, `agent-model-picker.tsx:482`; §5.4 |
| G27 | `@` refs are raw text, not entities | **P0** | Verified `at-refs.ts:4-13`; §5.6 |
| G28 | No visible `/eaios:*` host namespace | P1 | §5.7 |
| G29 | `+` attach is one text-only file input | P1 | §5.8 |
| G30 | No `Run ▾` split | P1 | §5.9 |
| G31 | Attachments are a single slot | P2 | §5.8 |
| G32 | No artifact version history in UI | P1 | §7 |
| G33 | No library/promotion lifecycle; "Library" means model weights | P1 | Verified `local-models-panel.tsx:97`; §7 |
| G34 | Preview kinds lack `browser`/`html`/`external` | P2 | §3/§7 |
| G35 | No deterministic reference discovery | P2 | R38 |
| G36 | Ten literal-orange values (one dead duplicate rule) | P1 | Verified `globals.css:525,534-535,573,578,583,599,729-730,767-770`; §9.4 |
| G37 | Google-Fonts first-paint dependency | P2 | OQ-UI-009 |
| G38 | One card for six decision kinds; `plan` branch unreachable | P2 | Verified `store.ts:211`, `mcq-interrupt-card.tsx:284-347`; §5.10 |

### Delivery slices (from the evidence)

1. **Slice A — "the answer is readable":** G1, G6, G16 (typography mapping, stream coalescing + fence gate, code-well tokens).
2. **Slice B — "diagrams and plans":** G2, G3, G4, G5, G7, G26 (mermaid pipeline, plan bar, reasoning dial).
3. **Slice C — "the workspace":** G17, G18, G19, G20-residue, G21, G22, G27, G30 (Workbench always on, DocumentSurface, Context panel, Runs list, structured `@`, `Run ▾`).

### Acceptance hooks (inputs for `ARCH/42-EVIDENCE-MAP.md`)

| ID | Check |
|---|---|
| UI-ACC-1 | A message containing a heading, a table and a 400-line fence renders correctly, and a trace of the turn's render path shows zero model calls after the first token. |
| UI-ACC-2 | A fenced `mermaid` block converts automatically; a diagram with `img:` shapes or `%%{init}%%` renders as copyable source with a status line; a theme/accent flip re-renders it; a failed render leaves the previous image visible (CLS measured 0). |
| UI-ACC-3 | A failed tool call never auto-collapses; a settled turn collapses to one line; a resumed session replays the same grouping. |
| UI-ACC-4 | The plan bar appears only while its turn runs and disappears with it; a second plan version reports a delta and never claims "no change" from an unknown previous list. |
| UI-ACC-5 | `/` while an agent is bound forwards to the agent; `/eaios:` lists host commands; `@` produces removable typed chips and emits an `embeddedContext` block only when advertised. |
| UI-ACC-6 | Two spreadsheets and a PDF open together; a per-session reload restores the tab set; a missing file renders inert with a reason. |
| UI-ACC-7 | Keyboard-only: every Workbench slot, tab, disclosure, approval choice and composer control is reachable with a visible ring; the a11y gate passes. |
| UI-ACC-8 | `grep` finds no literal hue outside `globals.css` token blocks and zero `dangerouslySetInnerHTML` outside the chart primitive. |

---

## 11. UI ↔ Core interop and failure modes

**Depends on:** `11` (SessionLog/Work projections — CTR-003/004/026) · `12` (approvals, tickets, projections — CTR-011/012) · `13`/`14` (capability descriptors, handles — CTR-009/010) · `15` (AgentEngine sessions, ACP config surface — CTR-001/002/021) · `16` (context service — CTR-006/007) · `18` (model descriptors/router — CTR-014) · `22`–`28` (domain previews/renders) · `29` (artifacts/receipts — CTR-018) · `30` (events — CTR-019) · `32` (channel/projection rules) · `34` (verification records).
**Exposes to:** nobody. The Experience plane is a leaf; no module may depend on the UI (`03-HLD.md` §4 rule 1).

| Failure | UI behavior |
|---|---|
| Core/sidecar unavailable | Last projection stays on screen with a status banner; no fabricated state, no fake spinners (`RuntimeStatusBanner`; UI-03). |
| Approval channel disconnects | The card stays visible and durable; it re-surfaces on attach; no timeout shown as a decision (`32` §8). |
| Provider epoch bump / stale handle | The affected control shows `InvalidState` + re-resolve action; running work is not silently retried (`13` §9, `12` §11). |
| Budget exhausted | Work pauses and surfaces with used/cap; the composer blocks new foreground turns per gate; nothing silently overruns (`11` §5). |
| Document unresolved (moved/deleted) | Tab renders inert *unresolved* with re-link; receipts keep their digest (`29` §8). |
| Markdown/diagram render failure | Failure is a state: status line + copyable source + retry — never a crash or a blank (`R18`). |
| Browser preview (`inTauri() === false`) | The send gate refuses with the `preview` reason; preview never forges live agent state (`chat-composer.tsx:131-140`; `artifact.ts:31-39`). |
| Projection leak attempt | Impossible by construction — the UI receives slices only; any reversal is a `12`/`32` violation, not a UI fix (`INV-11`). |

---

## 12. Open questions (`OQ-UI-*`)

| ID | Question | Why it matters | Resolve by |
|---|---|---|---|
| OQ-UI-001 | In-product noun for the promoted-artifact inventory ("Library" is currently the model-weights tab, `local-models-panel.tsx:97`) | Top-level surface name + data-model owner (`29`) | Before UI copy freeze (P5) |
| OQ-UI-002 | Does the Workbench stay visible in casual mode, or does casual get a single-slot preview pane? | Whether the rail is a region or a mode (`App.tsx:121-122`) | This doc's §2 position is "region"; product sign-off pending |
| OQ-UI-003 | Streaming renderer: adopt a Streamdown-class dependency + plugins, or build block caching onto the shipped `react-markdown` stack? | New dependency vs re-implementation; four comparators use the former | Before UI implementation starts (`ev: evidence §7` OQ-UI-003) |
| OQ-UI-004 | Is the reasoning dial per-turn, per-session or per-agent? | Persistence and survival across a session switch | This doc + product |
| OQ-UI-005 | `Run ▾`: is "Run in background" a new `Work` or the same `Work` continued, and does the chat stay attached? | The UI must know whether the session detaches | `ARCH/11-WORK.md` |
| OQ-UI-006 | Do `@files`/`@terminal` literals survive once references are structured? | Removes a legacy literal from user-visible copy | This doc proposes removal |
| OQ-UI-007 | Does the <900 px bottom strip survive v1, or does Windows-first set a minimum window size? | The strip is real work (`right-rail.tsx:828-891`); the window minimum is 800×600 | Product + `32` |
| OQ-UI-008 | Token-ise the code well (replace zinc + `github-dark.css`) or bless the dark well on light canvas? | Token discipline (C1) vs a stated aesthetic (`globals.css:378-379`) | This doc proposes token-ise |
| OQ-UI-009 | Bundle the fonts locally instead of Google Fonts? | First-paint network dependency + CLS risk in a local-first app | Performance pass |
| OQ-UI-010 | Transcript windowing: keep `content-visibility` hints or adopt true virtualization? | `DESIGN-SYSTEM.md:73` currently overstates the chat path | Performance pass |
| OQ-UI-011 | Split the leading column into far rail + session sidebar, or keep one collapsible sidebar? | Structural change to a shipped region | Product sign-off |
| OQ-UI-012 | What may be sent to an agent that advertises **no** `embeddedContext` — text-ified reference, or blocked with a reason? | Changes what an external agent sees from `@` | This doc + `13`/`32` |
| OQ-UI-013 | If the review queue ships (`OQ-AX-01`), does it live in the Runs lens or a session inbox? | One surface or two | `15` + this doc |

Carried from the evidence base (OQ-UI-001…008; wording tightened above and resolution pressure added where this doc takes a position). OQ-001 (`00-INDEX`) stays open for the product shorthand.

---

## 13. Evidence, corrections and UNVERIFIED carry-forward

**First-hand in this pass:** `ui/src/App.tsx:27-31,107-148` · `ui/src/lib/store.ts:31-57,80-95,179-205,207-269,272-310,379-387,1104-1120,1211-1223,1859,1930,3051-3085` · `ui/src/components/shell/right-rail.tsx:220-243,791-811,828-891,894-959` · `ui/src/components/shell/command-palette.tsx:45-115` · `ui/src/components/shell/left-sidebar.tsx:515-554,639-642` · `ui/src/components/shell/center-column.tsx:49-88` · `ui/src/components/chat/message-bubble.tsx:100-249,880-987` · `ui/src/components/chat/tool-chip.tsx:15-244,455-564` · `ui/src/components/chat/chat-composer.tsx:40-209,508-577` · `ui/src/components/chat/mcq-interrupt-card.tsx:280-350` · `ui/src/components/views/run-view.tsx:1-75` · `ui/src/components/views/generative/generative-ui.tsx:260-347` · `ui/src/components/views/chat-panel.tsx:585-610,765-782` · `ui/src/components/panels/local-models-panel.tsx:93-120` · `ui/src/lib/acp.ts:85-104` · `ui/src/lib/at-refs.ts:1-13` · `ui/src/lib/artifact.ts:1-61` · `ui/src/globals.css:1-200,755-845` (plus a repo-wide grep for the literal orange and the `@utility` names) · `ui/index.html:8-16` · `src-tauri/tauri.conf.json:19-27` · `ui/DESIGN-SYSTEM.md` · all v1 `ARCH/` docs cited above · `AGENTCOWORK-SPEC.md` §3/§4/§6/§9/§11.

**Corrections to the evidence base recorded here:** G20 is stale (per-session tab persistence exists, `store.ts:379-387`/`:3051-3085`); the "one tab strip exists" fact extends to light-ref persistence but not to document identity, which is the actual gap.

**Still `UNVERIFIED` (external URLs were unreachable in the evidence pass; no new fetch was attempted):**

| Claim | URL to verify | What would confirm it |
|---|---|---|
| Mermaid exposes `securityLevel`, `secure`, `suppressErrorRendering`, `dompurifyConfig`, `maxTextSize`, `maxEdges`, `themeVariables` | `https://mermaid.js.org/config/schema-docs/config.html` | that `secure` is the array which stops a diagram's own init directive overriding the listed keys, and that `dompurifyConfig` is among the reachable keys |
| Mermaid's own security guidance | `https://mermaid.js.org/config/usage.html` (security section) | what `securityLevel: 'strict'` does and does not block |
| Streamdown is a streaming markdown renderer with block-level caching and configurable mermaid handling | `https://www.npmjs.com/package/streamdown`, `https://github.com/lobehub/streamdown` | the documented props, and whether mermaid is first-class or a plugin (this decides OQ-UI-003) |
| `@tailwindcss/typography` is the intended Tailwind v4 prose plugin | `https://github.com/tailwindlabs/tailwindcss-typography` | the v4 `@plugin` import form |
| Tauri v2 `app.security.csp` semantics | `https://v2.tauri.app/reference/config/` | the documented CSP field options; the shipped value is read first-hand (`tauri.conf.json:27`), only the documentation is unverified |

**Known weakness of the evidence pass, carried forward:** competitor conclusions are drawn from reading clones, not running them; where a mechanism's behaviour depends on runtime configuration the file does not show, the conclusion is inference (`inf`) and is labelled. Not covered here and still open for a further pass: onboarding/setup/vault gates, localisation (`ui/src/lib/i18n.ts` unread), and the mobile/compact proposal beyond the 900 px breakpoint.

---

## 14. Requirements (`REQ-UI-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-UI-001` | Reasoning is summarized, never rendered as raw chain-of-thought; reasoning renders only through the `reasoning` projection (§4.4; UI-14). |
| `REQ-UI-002` | Rendering, navigation, diagram conversion and reference discovery are local and never spend model tokens (§8; UI-02). |
| `REQ-UI-003` | An inferred value is never presented as measured — unknown renders `—` or nothing, and no fabricated progress or spinner stands in for a value we lack (§1.2; §11). |
| `REQ-UI-004` | The answer body renders headings, tables, blockquotes, `hr`, ordered lists and images from the token palette, streaming-safe (block caching, closed-fence code gate, coalesced writes) (§4.1, R1–R8). |
| `REQ-UI-005` | A fenced `mermaid` block auto-converts only through the policy-gated, `secure`-locked, blob-`<img>` path — never inline SVG/HTML — themed from live tokens at CLS 0, with failure as a state (§4.2, R10–R18). |
| `REQ-UI-006` | Tool calls carry five states (`proposed · running · succeeded · failed · cancelled`); streaming stays `running`; a group containing an error never auto-collapses; a turn reads as one rail (§4.3, R21–R23). |
| `REQ-UI-007` | A plan bar binds to the running turn, appears and disappears with it, and reports a version delta rather than a false "no change" (§4.4, R31). |
| `REQ-UI-008` | The reasoning dial offers only model-supported levels, clamps on model switch, keeps `auto` off the track, and names the current level (§5.4, R51). |
| `REQ-UI-009` | One document surface opens one tab per document identity and N documents per kind, re-deriving content from a light ref and pre-checking existence in the host (§3, R45). |
| `REQ-UI-010` | The agent picker paints the agent-owned model or an explicit em dash, selects installed-only, and opens a two-pane runtime surface; a Native model is never offered for an external runtime (§5.3, §5.13). |
| `REQ-UI-011` | Runtime rows show a discriminated location and keep `installed`/`discovered`/`launchable` distinct; readiness is evidence-gated, never fabricated from a catalog, preview, mock or unit-only result (§5.13; UI-17). |
| `REQ-UI-012` | Every workflow, tab, disclosure, approval choice and composer control is keyboard-reachable with a visible focus ring, aria-live discipline, and an automated accessibility gate (§9.1; UI-12). |
| `REQ-UI-013` | Advanced detail sits behind a labelled collapsed row; a blocked control stays visible with its reason; a finished tool never steals focus or navigates (§1.2; UI-04, UI-05, UI-07). |
| `REQ-UI-014` | Live regions reserve their space (`min-h`, fixed readout slots, intrinsic-size hints); CLS = 0 is a standing standard (§1.2; UI-08). |
