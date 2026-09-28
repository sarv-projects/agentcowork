# AgentCowork Design System (P11.1)

> **DEC-055 target amendment:** This file remains the current token/component and shipped-shell inventory. `ARCH/48-EXPERIENCE-SURFACES.md` supersedes its frozen v1 interaction layout, three-control composer, first-party Native card and 22 peer Workbench viewports where they conflict. Reuse the existing semantic tokens while implementing the final progressive left navigation, composer, right Workbench and grouped Settings; do not treat the inventory below as proof of the target UI.

> **What this is:** the implementable surrogate of the "Figma/design file with
> all components + layouts" item. A real Figma file is a design-tool artifact
> this repo cannot host; this document is the source of truth that *would*
> live inside it — every token, component, layout and state, keyed to the
> code that implements it. Any designer can turn it into Figma frames 1:1.
>
> **Status:** Current UI design reference. The three-control composer and honest office-viewer behavior are defined by the normative product spec; historical changes belong in `../SPEC-CHANGELOG.md`.

> **Current provider/model behavior (2026-09-12, re-scoped 2026-09-21 by [`ARCH/ADR/0005`](ARCH/ADR/0005-external-agents-are-the-v1-engines.md)):** the agent-model picker and status bar use reachable live catalog rows, preserve provider-qualified selections through routing, and label curated seed rows as fallback. **Ownership is per-agent:** an external ACP agent owns the provider/model surface, so it shows its own ACP `configOptions`/`available_commands`; the AgentCowork catalogue/BYOK/local rows belong to the **post-v1 built-in binding**, which does not exist in v1 — where an agent reports no model surface of its own, the picker says “managed by &lt;agent&gt;”. Runtime rows are installed-only selectable, and Settings keeps a single agent surface — the Native model catalog is a collapsed disclosure on the AgentCowork Native card rather than a peer Models tab.

## 1. Tokens (code: `src/globals.css` `:root` / `.dark`)

| Token | Light | Dark | Notes |
|---|---|---|---|
| Canvas `--surface-0` | `#F7F7F4` | `#1A1917` | warm cream, never sterile white |
| Panel `--surface-1` | `#FFFFFF` | `#232220` | |
| Card `--surface-2` | `#FFFFFF` | `#2D2C29` | + 1px `#E8E5E0` hairline |
| Hover `--surface-3` | `#F0EFEB` | `#353330` | |
| Ink `--foreground` | `#26251E` | `#F0EFEB` | warm near-black |
| Muted `--muted-foreground` | `#6B6860` | `#B9B6AD` | |
| Brand (sole accent) | `#2563EB` | `#5E99F7` | **the v3.78 cool-blue semantic accent token (P66.5 landed)**; `[data-accent]` overrides it, so every surface follows the active accent choice |
| Success / Warning / Danger / Info | `#117E39` / `#966703` / `#D72323` / `#2563EB` | `#1FB757` / `#D29104` / `#E97777` / `#5E99F7` | semantic only, and tuned per theme |

> **Contrast (P66.5).** Every semantic token is measured against WCAG 2.2 AA by
> `src/lib/design-tokens.test.ts`, in both themes, on every surface, for every
> selectable accent — that test is the authority on these numbers, not this
table. Light-mode status hues were darkened and dark-mode ones brightened
> because the two themes need opposite adjustments: AA for text on a white card
> wants a *darker* hue, on a dark card a *brighter* one. That is also why the
> label on a dark accent is the dark ink rather than white.

> **Landed (P66.5):** brand is a *semantic* token — `--brand` (cool-blue `221 83% 53%` light / `217 91% 67%` dark) with `[data-accent]` overrides for sky / emerald / violet / amber, so Settings can offer selectable accent themes and every viewport inherits the active choice. The retired orange `#F54E00` is no longer reachable through the theme: the whole legacy `orange-*` utility family is aliased to `--brand`, and `amber-*` / `yellow-*` to `--warning`, in the `@theme inline` map in `globals.css`. (Ten literal `hsl(19 100% 48% …)` values remain in individual components — tracked in `AGENTCOWORK-UI.md` G36 and cleaned up in the code phase.) Status meanings (success/live/warning/error) keep their own semantic colors; orange is neither the brand nor a selection state. The semantic status tokens also carry dark-canvas values, which they previously did not.

**Radius** `--radius: 0.5rem` (sm/md/lg/xl derived). **Fonts** Inter (sans) +
JetBrains Mono (mono). **Spacing** 4px grid. **Motion** 150–300ms
`cubic-bezier(0.4, 0, 0.2, 1)`; reduced-motion kills all (globals.css).

## 2. Layouts (code: `src/components/shell/*`)

- **Cockpit** (`App.tsx`): TitleBar → [LeftSidebar | CenterColumn | ActivityRail | RightViewport] → StatusBar. Never 9 peer tabs.
- **LeftSidebar**: workspace selector, nav (Home/Activity/Projects/Files/Automations), Recent chats (P11.5.1 child forks indent), Settings. Collapsible to 48px.
- **CenterColumn**: chat (timeline + composer + approve cards) or one panel screen.
- **ActivityRail + RightViewport** (`right-rail.tsx`): 48px rail (Folder/Shell/Browse/**Computer use**/Code + Office flyout + Progress/Trajectory), one open surface, drag-resize 28–70%, per-session persistence (P11.5.3). The `ViewId` union in `src/lib/store.ts` names all **22** viewports and `VIEW_META` in `right-rail.tsx` labels every one of them: Folder · Terminal · Browser · Code · Spreadsheet · Document · Slides · PDF · Progress · Diff · Audit · Storage · Timeline · Trajectory · Blueprint · Local Server · Kanban · Generative UI · Artifact · Computer use · Tool output · Run.
- **StatusBar**: live runtime state (● Live / ⏸ Paused / Processing) + privacy reassurance + the current provider/model label when available; dev-mode telemetry strip incl. LCP/TTI (P11.4). Never present a curated seed row as live catalog data.

## 3. Components (code: `src/components/ui/*` + `src/components/panels|chat|views/*`)

| Component | File | States |
|---|---|---|
| Button (primary/ghost/outline/icon) | `ui/button.tsx` | default / hover / focus-visible / disabled |
| Badge, Card, Input, Select, Switch, Slider | `ui/*` | + loading skeletons (`ui/loading-state.tsx`) |
| EmptyState | `ui/empty-state.tsx` | icon + title + desc + action (P11.2) |
| ErrorState (5 kinds) | `ui/error-state.tsx` | network / keyRevoked / provider5xx / budget / unknown |
| LoadingState (5 kinds) | `ui/loading-state.tsx` | ttft / compaction / tool / agent / generic |
| MessageBubble, ChatComposer, MCQ card | `chat/*` | Composer: Work Mode ▾ · Agent ▾ (installed agents) · Autonomy ▾. Slash/`@` follow the pinned agent (v1: the agent's live ACP `available_commands`; a built-in catalog is post-v1 — ADR-0005). Casual chips `[🤖 Auto] [🛡 Ask]`. |
| Agent-picker governance badge | `chat/agent-model-picker.tsx` | P50.3.9: Governed-Mediated (green, "every effect ticketed + audited") · Self-contained (amber, "approvals mediated; agent's own effects unaudited") · NotGoverned (red) — honest note on hover; data from `acp_agents` `governance` |
| OnboardingModal | `onboarding-modal.tsx` | 4 steps, non-dismissible, skip allowed |
| Folder/Shell/Browse/Code/Diff views | `views/*` | real backends (fs / H36 profile-backed PTY / CDP / undo-list) |
| Cockpit slideover | `shell/cockpit-slideover.tsx` | animated open/close, per-agent pause/resume |

## 4. Accessibility (P11.3)

WCAG 2.2 AA target: focus-visible ring on every interactive element; high-contrast
mode (`html.high-contrast`); reduced motion; font scaling
(`html.font-scale-*`); RTL (`html[dir=rtl]` + logical-property pass);
aria-labels on icon-only buttons; keyboard nav via `KeyboardShortcuts` +
Radix focus traps.

## 5. Performance UX (P11.4)

Skeletons on async views; debounced search (`useDebouncedValue`); the chat
timeline uses `[content-visibility:auto]` for long logs, and `useVirtualList`
drives the `timeline` viewport; lazy chunks (pdf/charts/markdown); LCP/TTI
measured in `lib/perf.ts` and surfaced in the status bar.

## 6. Layouts index (all screens)

Center screens — all **12**, the `centerScreen` union in `src/lib/store.ts`:
Home · Chat · Activity · Projects · Files · Automations · Memory · Guard ·
Connectors · Analytics · Agents · Settings.

Right rail — all **22** viewports, the `ViewId` union in `src/lib/store.ts`
labelled by `VIEW_META` in `right-rail.tsx`:
Folder · Terminal · Browser · Computer use (real OS see-pane, vision-gated, DAG) ·
Code (IDE workbench) · Spreadsheet · Document · Slides · PDF (honest viewers + LO
fallback + file switcher) · Progress · Diff · Audit · Storage · Timeline ·
Trajectory · Blueprint · Local Server · Kanban · **Run** (the one run surface —
identity, context/usage, ordered trace steps, artifacts, workspace files, MCP
servers, with progress/trajectory/diff/artifact/tool-output as its drill-down
lenses) · Generative UI · Artifact · Tool output.
