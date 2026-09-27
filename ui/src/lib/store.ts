import { create } from 'zustand'
import { inTauri } from './tauri'
import { nativeCall, runtimeError, setRuntimeState } from './runtime'
import { guardAutonomy, guardSetAutonomy } from './guard'
import { inheritContext } from './plain-language'
import {
  recordSessionCreated,
  recordToolResult,
  recordTurnCompleted,
  recordTurnFailed,
} from './ux-metrics'
import {
  AGENT_MAP,
  DEFAULT_ROUTING,
  type AgentRuntime,
  type TaskKind,
} from './agents'
import type { ComposerRole, PermissionMode } from './ui-prefs'
import { getLocalItem } from './storage-compat'
import type { WorkAddress, WorkEventEnvelope, WorkPresence } from './work'
import type { SessionCapabilityLoadout } from './capabilities'
import { layoutWalkthroughStops, type WalkthroughStop } from './walkthrough'
import {
  acpHandleKey,
  findAcpHandleRecord,
  parseAcpHandleKey,
  type AcpHandleRecord,
} from './acp'

// === Types ============================================================

export type ViewId =
  | 'folder'
  | 'shell'
  | 'browse'
  | 'code'
  | 'office-xlsx'
  | 'office-docx'
  | 'office-pptx'
  | 'office-pdf'
  | 'progress'
  | 'diff'
  | 'audit'
  | 'storage'
  | 'timeline'
  | 'trajectory'
  | 'blueprint'
  | 'local-server'
  | 'kanban'
  | 'generative'
  | 'artifact'
  | 'desktop'
  | 'tool-output'
  /** The one run surface: identity, context/usage, ordered trace steps,
   * artifacts, workspace files, and MCP servers for the current run. An
   * aggregation **projection** — progress / trajectory / diff / artifact /
   * tool-output stay reachable as drill-down lenses from it. */
  | 'run'

/** v3.57 Work Mode (WHAT) — Code/browser/Office/terminal live *inside* Build. */
export type ChatMode = 'auto' | 'plan' | 'build' | 'research'

export function normalizeChatMode(m: unknown): ChatMode {
  if (m === 'plan' || m === 'research' || m === 'build' || m === 'auto') return m
  if (m === 'code') return 'build'
  return 'auto'
}

export type SessionStatus =
  | 'idle'
  | 'running'
  | 'action-required'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'budget_exceeded'
  | 'paused'
  | 'scheduled'
  | 'reconnecting'

export interface ToolCallRecord {
  id: string
  toolId: string
  args?: Record<string, unknown>
  result?: unknown
  status: 'running' | 'done' | 'failed'
  risk?: string
  error?: string
  progress?: string
  /** Wall-clock start (ms) — set when the call starts; the chip shows a live
   * elapsed timer while running and the settled duration once done. */
  startedAt?: number
  endedAt?: number
  /** P64.12 — which specialist ran this delegated step, when the host said so. */
  specialist?: string
}

/** One spooled tool result opened for inspection in the right rail. The
 * `tool-output` view renders this; it is ephemeral UI state (never
 * persisted, never a lease) — switching chats keeps the text until another
 * spooled card replaces it. */
export interface SpooledOutput {
  /** The tool-call record the output came from. */
  toolCallId: string
  /** Raw tool id (the view shows a short kind + the full id). */
  toolId: string
  /** The full cleaned output text (already CLI-normalized by the card). */
  text: string
  failed: boolean
  /** Wall-clock ms when it was opened. */
  openedAt: number
}

export interface ChatMessage {
  id: string
  role: 'user' | 'assistant' | 'system'
  content: string
  timestamp: string
  artifacts?: Artifact[]
  steps?: ProgressStep[]
  toolCalls?: ToolCallRecord[]
  mcq?: MCQInterrupt
  /** Native stream identity. Kept on the message so a late terminal event
   * cannot settle a newer turn in the same session. */
  streamId?: string
  /** Chain-of-thought text. Provider reasoning is a *delta stream* (many
   * wire events per turn), so chunks coalesce onto the last entry while the
   * turn streams and each entry ends as one displayable thought block.
   * Populated only from live `reasoning` wire events — never seeded. */
  reasoning?: string[]
  /** Wall-clock (ms) when this message's reasoning stream began. */
  reasoningStartedAt?: number
  /** Wall-clock (ms) when this message finished streaming (set on
   * finalize/fail/budget-kill/plan-done), powers the live-turn clock. */
  startedAt?: number
  endedAt?: number
  pinned?: boolean
  /** P51.7/P51.21 — structured failure on an assistant message. The partial
   * content is preserved (never clobbered with a marker line); the bubble
   * renders a layer-named error card with matched actions below the text. */
  error?: ChatError
  /** P51.7 — time-to-first-byte: ms from turn start to the first content or
   * reasoning delta. Undefined when the turn died before any token arrived. */
  ttfbMs?: number
  /** P52.20 — numbered citations produced from live search hits. */
  citations?: Array<{ index: number; title: string; url: string; snippet?: string; source?: string }>
  /** P64.12 — warm memories, skills, and governance shown in the passport pill. */
  passport?: { memories: string[]; skills: string[]; governance: string }
}

/** P51.21 — which layer reported a failed turn, shown as the card's badge.
 * `retryable` decides whether the card offers a one-click same-history retry
 * (agent/provider/tool errors are; budget kills are not). */
export interface ChatError {
  layer: 'provider' | 'guard' | 'tool' | 'agent' | 'budget' | 'runtime'
  code?: string
  detail: string
  retryable: boolean
  /** P51.2 — the failing turn's request id: the live stream id the send path
   * returned (the same key `chat_cancel`/support uses). Surfaced as a
   * copyable chip on the error card so a report can name the exact turn. */
  requestId?: string
}

export interface ArtifactActionUi {
  index: number
  label: string
  state: 'pending' | 'running' | 'complete' | 'aborted' | 'failed'
  formatted?: string
}

export interface ArtifactServerState {
  port: number
  url: string
  status: 'stopped' | 'serving'
  /** true when the server is a browser-preview stand-in (no Rust socket). */
  demo?: boolean
}

export interface Artifact {
  id: string
  name: string
  type: 'docx' | 'xlsx' | 'pptx' | 'pdf' | 'code' | 'markdown' | 'image' | 'webapp'
  preview: string
  view?: ViewId
  /** Absolute path when this artifact is a real office file. */
  path?: string
  /** P15-H29 — bolt.diy-style inline action checklist (auto-expands while running). */
  actions?: ArtifactActionUi[]
  /** P15-H29 — live loopback preview server for webapp artifacts. */
  server?: ArtifactServerState
  /** P32.3 — figures this run actually reported for the artifact (cells
   * changed, slides, files touched). Absent ⇒ the card renders no figures at
   * all rather than a plausible-looking guess. */
  figures?: string[]
}

export interface ProgressStep {
  id: string
  label: string
  status: 'done' | 'active' | 'pending' | 'failed'
  type: 'file' | 'edit' | 'chart' | 'browser' | 'shell' | 'code' | 'office' | 'export' | 'tool'
  detail?: string
  output?: string
  timestamp?: string
}

export interface MCQInterrupt {
  id: string
  title: string
  description: string
  kind: 'diff' | 'permission' | 'mcq' | 'budget' | 'plan' | 'autonomy'
  diff?: { file: string; added: string[]; removed: string[] }[]
  options?: { label: string; value: string }[]
  budget?: { used: number; cap: number }
  /** Guard-2 permission cards must echo this nonce when approving/rejecting. */
  approvalNonce?: string
  /** P11.2 — urgency level drives the card's badge + default selection. */
  urgency?: 'low' | 'medium' | 'high'
  /** P44.6 — autonomy-limit cards: the action that hit the frozen level and why. */
  autonomyAction?: string
  autonomyReason?: string
}

/** P44.6 — task-scoped autonomy elevation. `oneShot` = Do Once (consumed by
 * the next use), task-scoped = Allow For This Task (expires at task end).
 * `elevatedUntil` is the wall-clock expiry for the temporary elevation. */
export interface TaskElevation {
  level: PermissionMode
  grantedAt: number
  oneShot: boolean
  /** Task-scoped elevation end (wall-clock ms); one-shots are consumed on use. */
  elevatedUntil?: number
}

/** P44.6 — the frozen per-task autonomy snapshot. Captured at task start so a
 * live chatbar change never mutates an in-flight Work (same principle as
 * scheduled-run snapshots). `configHash` is the deterministic fingerprint of
 * autonomy + mode + workspace + agent scope (mirrors the Rust
 * `RuntimeManifest`/`execution/bind_runtime` config_hash contract). */
export interface TaskSnapshot {
  frozenAt: number
  autonomyLevel: PermissionMode
  mode: ChatMode
  workspaceScope: string
  agentScope: string
  /** The session this task belongs to — a ticket from another session never
   * mutates this task's frozen policy. */
  sessionId: string
  configHash: string
  elevation?: TaskElevation
}

export interface VerificationRecord {
  /** The plan-task id the checks belong to. */
  taskId: string
  /** The declared checks (K1). */
  checks: string[]
  /** The model's verification report (raw, bounded). */
  report: string
  /** true / false / null = ambiguous — never claimed as executed. */
  passed: boolean | null
  tsMs: number
}

export interface StreamStats {
  tokensPerSec: number
  ctxPct: number
  activeKey?: string
  tokensThisTurn: number
}

export interface Session {
  id: string
  title: string
  status: SessionStatus
  preview: string
  updatedAt: string
  pinned?: boolean
  messages: ChatMessage[]
  children?: Session[]
  parentId?: string
  agent?: string
  folder?: string
  spent?: number
  tokens?: number
  view?: ViewId
  officeDoc?: string
  railCollapsed?: boolean
  /** P38 — the session's pinned Chief (`inbuilt` | ACP id). Persisted on the
   * Session object so the vault round-trip (`session_put`) makes the pin
   * durable across app restarts; `sessionChiefs` is the fast store mirror. */
  chiefPin?: string
  /** P38 — the session was explicitly unpinned (a pin existed and was
   * cleared). Persisted like `chiefPin` so a restarted session that had a pin
   * clearly shows the user default applies again instead of reading as
   * never-pinned. Cleared when a new pin is set. */
  chiefUnpinned?: boolean
  /** P51.9 — the session's finish-line goal (rides the vault round-trip so
   * it survives app restarts, same as `chiefPin`). */
  goal?: string
  /** P51.9 — the user marked the goal as achieved this run. */
  goalAchieved?: boolean
  /** P66.4 — per-session capability loadout overrides (MCP, skills, connectors, shared cowork tools) */
  capabilityLoadout?: SessionCapabilityLoadout
  /** P71.9f — when a Chat was opened **from** an automation run (`ADR-0006` §7),
   * this records the run's Work and the Session it belonged to. The Chat is a
   * continuation of that run, not a second Session — so the 1:1 rule between a
   * Chat and its Session holds again, and no Chat is ever fabricated per run. */
  originRunId?: string
  originRunSessionId?: string
}

/**
 * P50.2.1 — keep vault rows that can render as sessions, drop the rest. The
 * Rust parse boundary already drops malformed JSON; this drops JSON-valid
 * but schema-wrong rows (missing/empty `id`) that would otherwise enter the
 * store verbatim and render as broken chats. Pure (unit-tested); dropping is
 * always safe because the vault still holds the raw row.
 */
/** Render a session transcript as Markdown (copy/export share this). */
export function sessionTranscriptMarkdown(session: Session): string {
  const lines = [`# ${session.title}`, '']
  for (const m of session.messages) {
    lines.push(m.role === 'user' ? '## You' : m.role === 'assistant' ? '## Assistant' : '## System', '', m.content, '')
    if (m.error) lines.push(`> ⛔ ${m.error.layer} error: ${m.error.detail}`, '')
    for (const t of m.toolCalls ?? []) {
      lines.push(`- tool \`${t.toolId}\` — ${t.status}${t.error ? `: ${t.error}` : ''}`)
    }
  }
  return lines.join('\n')
}

export function sanitizeSessionRows(rows: unknown): Session[] {
  if (!Array.isArray(rows)) return []
  return rows.filter(
    (r): r is Session =>
      typeof r === 'object' &&
      r !== null &&
      typeof (r as { id?: unknown }).id === 'string' &&
      (r as { id: string }).id.length > 0,
  )
}

/**
 * P50.2.1 — merge `session_list` vault rows into the live list.
 * Vault rows are authoritative, but a session the user created while the
 * list was in flight is local-only (not yet persisted — it reaches the vault
 * later via `session_put`) and must survive the merge; likewise the active
 * target is preserved whenever it still exists, so a late hydration (or a
 * supervisor-restart rehydrate) never silently wipes the user's new session
 * or yanks focus to the newest vault row mid-conversation.
 */
export function mergeHydratedSessions(
  current: Session[],
  vault: Session[],
  activeId: string,
): { sessions: Session[]; activeSessionId: string } {
  const localOnly = current.filter((s) => !vault.some((v) => v.id === s.id))
  const sessions = [...localOnly, ...vault]
  const activeSessionId = sessions.some((m) => m.id === activeId) ? activeId : sessions[0]?.id ?? ''
  return { sessions, activeSessionId }
}

export interface Automation {
  id: string
  name: string
  trigger: string
  triggerKind: 'schedule' | 'webhook' | 'event' | 'slack'
  action: string
  activity: number[]
  enabled: boolean
  runs: number
  success: number
  failed: number
  lastRun?: string
}

/** P11.5.3 — per-session layout snapshot (persisted per sessionId). */
export interface SessionLayout {
  view?: ViewId
  officeDoc?: string
  railCollapsed?: boolean
  splitRatio?: number
  composerMode?: ChatMode
  /** P33.7 — the open tab set (persisted + restored per session). */
  openViews?: ViewId[]
}

/** P30.15 — one dream-diary entry (visible memory consolidation). */
export interface DiaryEntry {
  id: string
  /** UNIX ms of the consolidation run. */
  atMs: number
  /** Short plain-language headline ("Consolidated 3 sessions about the Q3 plan"). */
  headline: string
  /** Facts folded into long-term memory this run. */
  foldedFacts: number
  /** Facts decayed this run (pruned as noise). */
  decayedFacts: number
  /** The morning-brief line (rendered in the storage view). */
  brief: string
}

/** P11.5.3 — a real pending patch (agent file mutation w/ undo snapshot). */
export interface PendingPatch {
  id: string
  sessionId: string
  path: string
  beforeBytes: number
  applied?: boolean
}

export interface Connector {
  id: string
  name: string
  // P50.2.6 — only surfaces the Rust shell can prove: native OAuth rows
  // (vault accounts) and MCP rows (attached/live). Legacy aggregator
  // categories return only if a native command reports them.
  category: 'native' | 'mcp'
  status: 'connected' | 'disconnected' | 'error'
  tools: number
  type?: 'oauth' | 'apiKey' | 'stdio' | 'http'
}

export interface MemoryItem {
  id: string
  title: string
  category: string
  trigger?: string
  macro?: string
  scope: string
  enabled: boolean
  source: 'manual' | 'learned' | 'suggested'
}

export interface PermissionEntry {
  id: string
  action: string
  target: string
  status: 'approved' | 'auto' | 'pending' | 'blocked'
  timestamp: string
  scope?: string
}

// === Mock data ============================================================

const now = new Date()
const iso = (offsetMin: number) =>
  new Date(now.getTime() - offsetMin * 60_000).toISOString()

export const mockSessions: Session[] = [
  {
    id: 's1',
    title: 'Q3 report — refresh numbers + deck',
    status: 'action-required',
    preview: 'Regenerating revenue chart and exec summary',
    updatedAt: iso(2),
    pinned: true,
    folder: '~/work/q3-report',
    agent: 'analyst',
    spent: 1.84,
    tokens: 184_220,
    view: 'office-xlsx',
    officeDoc: 'Q3-Financials.xlsx',
    messages: [
      {
        id: 'm1',
        role: 'user',
        content:
          'Refresh the Q3 numbers from the new actuals spreadsheet, regenerate the revenue chart, then update the executive summary in the deck.',
        timestamp: iso(28),
      },
      {
        id: 'm2',
        role: 'assistant',
        content:
          'On it. I opened `Q3-Financials.xlsx`, mapped the new actuals to cells B7:B12 on `Sheet1`, then ran a deterministic sort + sum through IronCalc (no LLM math). The revenue chart is regenerating now, after which I will patch the executive-summary paragraph in `exec-summary.docx`.',
        timestamp: iso(26),
        steps: [
          { id: 'p1', label: 'Opened Q3-Financials.xlsx', status: 'done', type: 'file', detail: 'Sheet1' },
          { id: 'p2', label: 'Updated B7:B12 with Q3 actuals', status: 'done', type: 'edit', detail: '6 cells · surgical patch' },
          { id: 'p3', label: 'Regenerating revenue chart', status: 'active', type: 'chart', detail: 'IronCalc recalc' },
          { id: 'p4', label: 'Patch exec-summary.docx §3.2', status: 'pending', type: 'office', detail: 'block-patch' },
          { id: 'p5', label: 'Export final PDF', status: 'pending', type: 'export' },
        ],
        artifacts: [
          {
            id: 'a1',
            name: 'Q3-Financials.xlsx',
            type: 'xlsx',
            preview: 'Sheet1 · B7:B12 updated',
            view: 'office-xlsx',
          },
        ],
      },
      {
        id: 'm3',
        role: 'assistant',
        content:
          'The exec summary rewriter needs a green light — this overwrites a paragraph in `exec-summary.docx`.',
        timestamp: iso(1),
        mcq: {
          id: 'mcq1',
          title: 'Approve paragraph rewrite in exec-summary.docx',
          description:
            'Replace the §3.2 paragraph "Revenue grew 14% QoQ…" with the new numbers from the actuals sheet ($1.8M, +20% QoQ).',
          kind: 'diff',
          diff: [
            {
              file: 'exec-summary.docx §3.2',
              added: [
                'Revenue grew 20% QoQ, reaching $1.8M driven by enterprise deals.',
                'Churn rate: 2.1% (down from 3.4%).',
              ],
              removed: [
                'Revenue grew 14% QoQ, reaching $1.5M.',
                'Churn rate: 3.4%.',
              ],
            },
          ],
        },
      },
    ],
  },
  {
    id: 's2',
    title: 'Scraper — competitor pricing refresh',
    status: 'running',
    preview: 'Crawling 47 product pages on competitor site',
    updatedAt: iso(8),
    folder: '~/work/price-watch',
    agent: 'browser',
    spent: 0.92,
    tokens: 88_410,
    view: 'browse',
    messages: [
      {
        id: 's2m1',
        role: 'user',
        content: 'Refresh pricing for all 47 products on competitor.acme.com. Use my logged-in Chrome profile.',
        timestamp: iso(12),
      },
      {
        id: 's2m2',
        role: 'assistant',
        content:
          'Switched browser to **system Chrome** with your signed-in profile (vault profile `acme-personal`). Tier-2 engine. Currently on page 23/47 — `agentcowork-cdp` is taking accessibility-tree snapshots, extracting the price via the `[data-product-card]` locator, and writing rows to `pricing.csv`.',
        timestamp: iso(8),
        steps: [
          { id: 's2p1', label: 'Chrome login inherited', status: 'done', type: 'browser' },
          { id: 's2p2', label: 'Crawling product pages (23/47)', status: 'active', type: 'browser', detail: 'Lightpanda → Chrome escalation on 2 pages' },
          { id: 's2p3', label: 'Writing to pricing.csv', status: 'pending', type: 'file' },
        ],
      },
    ],
  },
  {
    id: 's3',
    title: 'Refactor api/users.ts → typed router',
    status: 'paused',
    preview: 'User took over — switched shell to writable',
    updatedAt: iso(35),
    folder: '~/code/backend-api',
    agent: 'coder',
    spent: 0.51,
    tokens: 51_330,
    view: 'code',
    messages: [
      {
        id: 's3m1',
        role: 'user',
        content: 'Refactor src/api/users.ts to use the typed router + db.query. Add tests.',
        timestamp: iso(40),
      },
    ],
  },
  {
    id: 's4',
    title: 'Invoice batch — fill & sign PDFs',
    status: 'completed',
    preview: '42 invoices filled, 42 signatures applied',
    updatedAt: iso(140),
    folder: '~/work/invoices',
    agent: 'analyst',
    spent: 2.41,
    tokens: 240_100,
    view: 'office-pdf',
    messages: [],
  },
  {
    id: 's5',
    title: 'Daily standup digest (scheduled)',
    status: 'scheduled',
    preview: 'Next run: tomorrow 09:00 IST',
    updatedAt: iso(360),
    folder: '~/automations',
    agent: 'analyst',
    messages: [],
  },
]

// P50.2.6 — removed: mockAutomations (Daily backup / CI fixer demo rows)
// deleted with mockConnectors. Automations render from the live scheduler
// ledger; an empty ledger renders empty, never seeded runs.

// P50.2.6 — removed: mockConnectors (Composio/Slack/Linear demo rows) deleted
// 2026-09-03. Only installed/attached/authenticated resources may appear
// connected; the Connectors panel renders vault OAuth accounts + live MCP
// rows + the store catalog, never a seeded list.

export const mockMemory: MemoryItem[] = [
  {
    id: 'mem1',
    title: 'Use pnpm not npm',
    category: 'Coding standards',
    trigger: 'package management',
    macro: '!pnpm',
    scope: 'all projects',
    enabled: true,
    source: 'manual',
  },
  {
    id: 'mem2',
    title: 'Deploy to prod checklist',
    category: 'Deployment',
    trigger: 'deploying, production',
    macro: '!deploy',
    scope: 'backend-api project',
    enabled: true,
    source: 'learned',
  },
  {
    id: 'mem3',
    title: 'User prefers concise replies, no emojis',
    category: 'Personal prefs',
    scope: 'all projects',
    enabled: true,
    source: 'manual',
  },
  {
    id: 'mem4',
    title: 'Always run lint before commit',
    category: 'Coding standards',
    trigger: 'git commit',
    macro: '!lintcommit',
    scope: 'all projects',
    enabled: false,
    source: 'suggested',
  },
  {
    id: 'mem5',
    title: 'API responses are JSON:API spec',
    category: 'Project context',
    scope: 'backend-api project',
    enabled: true,
    source: 'manual',
  },
]

// P50.2.6 — removed: mockPermissions (Read/Write/Execute demo rows) deleted
// with mockConnectors. The Guard panel renders live tickets/receipts; an
// empty trail renders empty, never seeded approvals.

// === Live-data bridge state (src/lib/bridge.ts) ===============================

/** Real budget from the shell (P5.9 spend snapshot). */
export interface LiveBudget {
  spent: number
  cap: number
  tokens: number
  cacheHitRate?: number
}

/** P51.5 — one queued user turn (sent automatically once the current turn of
 * the same session finishes). Created when the user sends while the agent is
 * still generating; edited/deleted via the pending chips above the composer. */
export interface QueuedTurn {
  id: string
  text: string
  /** Attachment captured when the turn was queued (sent with it). */
  context?: { title: string; content: string }
}

/** Bridge-side dispatcher the store calls when a queued turn should fire.
 * Registered once by the bridge at startup (store stays import-free of the
 * bridge to avoid a cycle). */
export type TurnDispatcher = (turn: { sessionId: string; text: string; context?: { title: string; content: string }; bypassQueue: true }) => void

/** P50.2.5 — one live notification row, fed by the bridge from real wire
 * events (chat errors, budget kills, Guard-2 tickets, monitor outcomes).
 * Never seeded: a fresh shell has zero rows until an event lands. */
export interface LiveNotification {
  id: string
  kind: 'info' | 'success' | 'warning' | 'error' | 'cost' | 'guard' | 'agent' | 'git'
  title: string
  detail: string
  ts: number
  unread: boolean
  source?: string
}

/**
 * Why the current agent cannot accept a turn yet. This is deliberately
 * session-scoped: a blocked message in one chat must not make another chat look
 * busy or imply that its binding is broken.
 */
export interface AgentSendBlocker {
  sessionId: string
  code: 'unbound' | 'readiness-unknown' | 'not-ready' | 'preview'
  title: string
  detail: string
  agentId?: string
}

// The assistant message currently being streamed **(keyed by sessionId, not a
// global)**. Streaming is routed to the session the turn started on, so a chat
// switch mid-stream never lands tokens on the wrong transcript (chat-events
// carry a sessionId; see the bridge). module-level since zustand actions can't
// hold instance state.
let activeStreamMsg: Record<string, string> = {} // sessionId -> assistant msg id
let activeStreamId: Record<string, string> = {} // sessionId -> native stream id
/** Last terminal stream per session; late events from it must never bind to a
 * newer turn whose assistant message has not received its first event yet. */
let lastTerminalStreamId: Record<string, string> = {}
let streamT0BySession: Record<string, number> = {}
let streamTokBySession: Record<string, number> = {}
/** P51.7 — first content/reasoning delta per session (ms), for the TTFB
 * footer. Cleared when the turn settles. */
let streamFirstDelta: Record<string, number> = {}

/** P51.7 — record the first response byte for `sid` (first writer wins). */
function markFirstDelta(sid: string) {
  if (streamFirstDelta[sid] === undefined) streamFirstDelta[sid] = Date.now()
}

/** P51.7 — ttfbMs for a settling turn (undefined when no delta ever landed). */
function ttfbFor(sid: string, startedAt?: number): number | undefined {
  const t = streamFirstDelta[sid]
  if (t === undefined || !startedAt || t < startedAt) return undefined
  return Math.max(0, t - startedAt)
}
let turnDispatcher: TurnDispatcher | undefined
let queueSeq = 0
let idSeq = 0
/** Monotonic id mint: `Date.now()` alone collides when two streams/sessions
 * are created in the same millisecond (tests hit this; live fast forks too). */
function freshId(prefix: string): string {
  idSeq += 1
  return `${prefix}-${Date.now()}-${idSeq}`
}

/** Resolve the session a stream action targets: explicit id > active session. */
function streamSessionId(sessionId?: string): string {
  return sessionId ?? useAppStore.getState().activeSessionId
}

/** True when `sessionId` has an in-flight assistant turn. */
function hasActiveStream(sessionId: string): boolean {
  return !!activeStreamMsg[sessionId]
}

/** Bind an event's native stream id to the session's in-flight message. A
 * different id is a stale/concurrent event and must not mutate this turn. */
function bindStreamId(sessionId: string, streamId?: string): boolean {
  if (!streamId) return true
  const current = activeStreamId[sessionId]
  if (current !== undefined && current !== streamId) return false
  // A terminal event clears the active binding, but the stream id remains a
  // tombstone so a late batch/tool event cannot start mutating the next turn.
  if (current === undefined && lastTerminalStreamId[sessionId] === streamId) return false
  activeStreamId[sessionId] = streamId
  return true
}

function retireStream(sessionId: string): void {
  const streamId = activeStreamId[sessionId]
  if (streamId !== undefined) lastTerminalStreamId[sessionId] = streamId
}

/** Live turn clock: ms since the current stream started (0 when idle). The
 * clock is session-scoped so two sessions cannot inflate each other's elapsed
 * time. */
export function streamElapsedMs(sessionId?: string): number {
  const sid = streamSessionId(sessionId)
  const started = streamT0BySession[sid] ?? 0
  return started === 0 ? 0 : Math.max(0, Date.now() - started)
}

/** TEST-ONLY isolation helper: the zustand store and the module-level stream
 * registry are process-global, and bun test files share one module instance,
 * so tests exercising the streaming/queue lifecycle must reset before and
 * after. Never called by app code. */
export function resetStreamingTestState(): void {
  activeStreamMsg = {}
  activeStreamId = {}
  lastTerminalStreamId = {}
  streamT0BySession = {}
  streamTokBySession = {}
  turnDispatcher = undefined
  streamTestReset?.()
}

// Bound by the store creator below (TDZ-safe indirection: resetStreamingTestState
// may run before the store exists only if a caller never awaits module init).
let streamTestReset: (() => void) | undefined

function patchActiveAssistant(
  set: (partial: object | ((s: { sessions: Session[]; activeSessionId: string }) => object)) => void,
  fn: (m: ChatMessage) => ChatMessage,
  sessionId?: string,
) {
  set((s) => ({
    sessions: s.sessions.map((x) => {
      if (x.id !== streamSessionId(sessionId)) return x
      const targetId = activeStreamMsg[x.id] ?? undefined
      if (!targetId) return x
      return {
        ...x,
        messages: x.messages.map((m) => (m.id === targetId ? fn(m) : m)),
      }
    }),
  }))
}

/** Patch the streaming assistant message (or any message by id in a
 * session) without touching siblings — identity-stable for memoized bubbles. */
function patchStreamMessage(
  set: (partial: object | ((s: { sessions: Session[]; activeSessionId: string }) => object)) => void,
  patch: (m: ChatMessage) => ChatMessage,
  sessionId?: string,
) {
  set((s) => ({
    sessions: s.sessions.map((x) => {
      if (x.id !== streamSessionId(sessionId)) return x
      const targetId = activeStreamMsg[x.id] ?? undefined
      if (!targetId) return x
      return {
        ...x,
        messages: x.messages.map((m) => (m.id === targetId ? patch(m) : m)),
      }
    }),
  }))
}

// P51.25 — per-pill status-bar customization, persisted to localStorage.
// Toggles which live pills the casual footer may render (context meter,
// throughput, cache, cost). Default: everything on.
const STATUS_BAR_PILLS_KEY = 'agentcowork.settings.ui.statusBarPills'
const STATUS_BAR_PILLS_DEFAULT = { context: true, throughput: true, cache: true, cost: true }
export type StatusBarPills = typeof STATUS_BAR_PILLS_DEFAULT
export const readStatusBarPills = (): StatusBarPills => {
  if (typeof window === 'undefined') return STATUS_BAR_PILLS_DEFAULT
  try {
    // DEC-053: legacy `everyaios.*` key honored + promoted once.
    const raw = getLocalItem(STATUS_BAR_PILLS_KEY)
    if (!raw) return STATUS_BAR_PILLS_DEFAULT
    const parsed = JSON.parse(raw) as Partial<StatusBarPills>
    return { ...STATUS_BAR_PILLS_DEFAULT, ...parsed }
  } catch {
    return STATUS_BAR_PILLS_DEFAULT
  }
}
const writeStatusBarPills = (p: StatusBarPills) => {
  if (typeof window === 'undefined') return
  try {
    window.localStorage.setItem(STATUS_BAR_PILLS_KEY, JSON.stringify(p))
  } catch {
    /* storage may be unavailable */
  }
}

// Progressive-disclosure preference (B9/P31) persisted to localStorage.
const POWER_MODE_KEY = 'agentcowork.settings.ui.powerMode'
const readPowerMode = (): boolean => {
  if (typeof window === 'undefined') return true
  try {
    // DEC-053: legacy `everyaios.*` key honored + promoted once.
    const v = getLocalItem(POWER_MODE_KEY)
    // Unset → full cockpit. Explicit '0' selects the casual layout.
    if (v === null) return true
    return v === '1'
  } catch {
    return true
  }
}
const writePowerMode = (v: boolean) => {
  if (typeof window === 'undefined') return
  try {
    window.localStorage.setItem(POWER_MODE_KEY, v ? '1' : '0')
  } catch {
    /* storage may be unavailable */
  }
}

export const SETTINGS_SECTION_IDS = [
  'general',
  'appearance',
  'notifications',
  'voice',
  'mobile',
  'agents',
  'local',
  'capabilities',
  'apikeys',
  'experts',
  'subagents',
  'tool-log',
  'launch',
  'chat',
  'permissions',
  'browser',
  // P55.8 — Settings → Search (SearXNG endpoints + the searx.space feed).
  'search',
  'indexing',
  'mcp',
  'marketplace',
  'skills',
  'commands',
  'hooks',
  'worktree',
  'rules',
  'memory',
  // P58.3 — Settings → Computer use got its specified nav row.
  'computer',
  // P65.4 — Settings → Schedules (compact surface over the shared scheduler).
  'schedules',
  // P55.9 — `cloud` is kept as an accepted deep-link id; it now routes to the
  // real H33 node attach on the Sync surface (the docker-package mock is gone).
  'cloud',
  'import',
  'usage',
  'ux',
  'feedback',
  'resources',
  'beta',
  'privacy',
  'sync',
  'keyboard',
  'advanced',
  'doctor',
  // P70.D4/D7/D8 — sandbox honesty, support bundle, remove-all-data.
  'diagnostics',
  'discover',
  'runtime',
  'about',
] as const
export type SettingsSectionId = (typeof SETTINGS_SECTION_IDS)[number]

const PERMISSION_KEY = 'agentcowork.settings.permissionMode'
const readPermission = (): PermissionMode => {
  if (typeof window === 'undefined') return 'ask'
  try {
    // DEC-053: legacy `everyaios.*` key honored + promoted once.
    const v = getLocalItem(PERMISSION_KEY)
    if (v === 'sandbox' || v === 'ask' || v === 'auto' || v === 'full') return v
  } catch {
    /* ignore */
  }
  return 'ask'
}
const writePermission = (v: PermissionMode) => {
  if (typeof window === 'undefined') return
  try {
    window.localStorage.setItem(PERMISSION_KEY, v)
  } catch {
    /* ignore */
  }
}

/** P44.6 — deterministic FNV-1a fingerprint of the frozen task scope. Mirrors
 * the Rust `RuntimeManifest::compute_hash` idea (SHA-256 there; the UI mirror
 * is a stable fingerprint for display + equality checks, not the audit hash). */
export function taskScopeHash(autonomyLevel: string, mode: string, workspace: string, agent: string): string {
  const input = JSON.stringify({
    autonomy: autonomyLevel,
    mode,
    workspace,
    agent,
  })
  let h = 0x811c9dc5
  for (let i = 0; i < input.length; i++) {
    h ^= input.charCodeAt(i)
    h = Math.imul(h, 0x01000193)
  }
  return (h >>> 0).toString(16).padStart(8, '0')
}

// === Zustand store ============================================================

interface AppState {
  // Sessions & navigation
  sessions: Session[]
  workItems: WorkAddress[]
  workPresence?: WorkPresence
  workEvents: WorkEventEnvelope[]
  /** P51.10 — Changes Walkthrough stops from `chat/walkthrough`. */
  walkthroughStops: WalkthroughStop[]
  streamWalkthrough: (stops: unknown[], sessionId?: string, streamId?: string) => void
  setWorkProjection: (items: WorkAddress[], presence?: WorkPresence, events?: WorkEventEnvelope[]) => void
  /** P51.8 — Chat/Cowork lens. Cowork mode folds the live Work Gateway
   * projection (agent cards) into the chat column instead of hiding it in
   * the right rail; ephemeral view state, never persisted. */
  coworkMode: boolean
  setCoworkMode: (on: boolean) => void
  activeSessionId: string
  /** True once the shell has taken ownership of the sessions list (loaded via
   * `session_list` or a new session was created). Until then the list is the
   * demo seed and must never be persisted (bugfix 3). */
  sessionsHydrated: boolean
  markSessionsHydrated: () => void
  setActiveSession: (id: string) => void
  newSession: () => void
  /** P71.9f — open an automation run as a Chat (`ADR-0006` §7): the run's
   * Session gains a Chat **on demand**, so the run list stays the surface that
   * owns headless work and nothing invents a hidden Chat per firing. Returns
   * the new chat's id. */
  openAutomationRun: (run: { id: string; sessionId: string; objective: string }) => string
  deleteSession: (id: string) => Promise<void>
  /** Rename a session (title). Auto-persisted via the session subscription. */
  renameSession: (id: string, title: string) => void
  /** Flip the session's pinned flag. Auto-persisted. */
  toggleSessionPinned: (id: string) => void
  /** Clear a session's transcript (messages only — the session survives). */
  clearSessionMessages: (id: string) => void
  /** P51.33 — apply a `/compact` verdict (keep from `keptFrom`, prepend the
   * synthetic continue marker when Rust pruned an overflow). */
  compactSessionMessages: (id: string, keptFrom: number, marker: string | null) => void
  /** Fork a session: duplicate transcript into a new session. Returns the id. */
  forkSession: (id: string) => string | null
  /** P52.22 — truncate-below edit. Given a user message, drop that message
   * and everything after it from the transcript (session flips to idle) and
   * return its text so the caller can prefill the composer — the corrected
   * ask then *replaces* it on send, so no duplicate resubmit appears. Returns
   * null when the message is not a user message or the session is streaming. */
  rewindToUserMessage: (sessionId: string, messageId: string) => string | null
  /** P52.22 — same-history regen for an assistant message: truncate from the
   * user turn that produced it (inclusive) and return that prompt text for a
   * real re-ask, so the corrected run reads in place. Null if not found or a
   * turn is live. */
  rewindBeforeAssistant: (sessionId: string, assistantMessageId: string) => string | null
  /** P51.9 — per-session goal text (persisted on the Session via the vault
   * round-trip). Cleared when achieved or dismissed. */
  setSessionGoal: (sessionId: string, goal: string | undefined) => void
  /** P51.9 — mark the session goal achieved (finish-line check). */
  markGoalAchieved: (sessionId: string, achieved: boolean) => void
  monitorBadge: { count: number; last?: string; stopped: boolean }
  pushMonitor: (ev: { notified: boolean; stopped: boolean; current: string; jobId?: string }) => void
  clearMonitorBadge: () => void

  // Active view (right viewport)
  activeView: ViewId
  setActiveView: (v: ViewId) => void
  /** P33.6 — pending URL for the browser view (Google Docs/Sheets read path). */
  browserUrl: string | null
  /** P33.6 — route a Google Docs/Sheets link into the authenticated browser view. */
  openInBrowser: (url: string) => void
  /** P50.3.8 — live CDP attachment state (reported by the browse view so the
   * status bar and rail reflect the real session, never a hardcoded value). */
  browserAttached: boolean
  setBrowserAttached: (attached: boolean) => void
  /** P50.3.8 — consume a routed browser URL (clears it so reopening the same
   * URL retriggers the navigation effect). */
  clearBrowserUrl: () => void
  /** P50.3.7 — live desktop-engine attachment state (reported by the desktop
   * view; mirrors `desktop_status`: attached + reason while detached). Lets
   * the status bar and rail reflect the real engine, never a static claim. */
  desktopAttached: boolean
  desktopReason: string | null
  setDesktopAttached: (attached: boolean, reason?: string | null) => void
  /** P59.3 — Computer use was refused because the model cannot take images. */
  cuaVisionGate: boolean
  setCuaVisionGate: (open: boolean) => void

  /** P50.4.1/4.9 — live vault-keys fact: `null` = unknown (not yet probed),
   * `false` = vault has zero provider keys. Feeds the first-run setup gate,
   * the no-provider chat empty state, and the capability matrix. Refreshed
   * by the bridge at hydration and after every key add/remove. */
  providerKeysConfigured: boolean | null
  setProviderKeysConfigured: (configured: boolean) => void
  /** P50.4.1 — first-run provider-setup overlay (opened when no provider is
   * configured and the user asks to set one up; dismissible — the chat empty
   * state re-surfaces it). */
  setupOpen: boolean
  openSetup: () => void
  closeSetup: () => void
  /** Last session-scoped reason a send was refused before transcript mutation. */
  agentSendBlocker?: AgentSendBlocker
  setAgentSendBlocker: (blocker?: AgentSendBlocker) => void

  /** P38 — per-session Chief pins: sessionId → Chief id. An empty/absent pin
   * means the user default applies; a pin outranks it for that session's
   * turns. Kept in the store so the chat send path resolves the effective
   * Chief per turn without extra IPC. */
  sessionChiefs: Record<string, string>
  setSessionChiefPin: (sessionId: string, chiefId: string) => void
  clearSessionChiefPin: (sessionId: string) => void
  /** P38 — the user's `primary_chief` default (loaded from the shell). */
  userDefaultChief?: string
  setUserDefaultChief: (chiefId: string) => void
  railCollapsed: boolean
  toggleRail: () => void
  setRailCollapsed: (v: boolean) => void
  /** Full-window mode for the active cockpit lens. */
  fullscreenView: boolean
  setFullscreenView: (v: boolean) => void

  // Multi-view panel (ARCH/12 v3.0 — VS Code-style tabs). Open views are a
  // tabbed set; defaults Folder · Shell · Browser; "+" adds more; close × removes.
  openViews: ViewId[]
  addView: (v: ViewId) => void
  closeView: (v: ViewId) => void
  /** P33.7 — drag-reorder the open tab set. */
  reorderViews: (from: number, to: number) => void
  /** Real file path per office view (replaces hardcoded Q3-Financials.xlsx labels). */
  officePaths: Partial<Record<ViewId, string>>
  /** P1.7 — opened-file history per office view (most recent first). Opening
   * a second file of the same kind no longer loses the first: the view's
   * file switcher lists this history so any prior file can be re-opened. */
  officeHistory: Partial<Record<ViewId, string[]>>
  /** Switch the given office view to a previously opened file path. */
  switchOfficeDoc: (view: ViewId, path: string) => void
  /** Office hold lift (P33.1) — close one opened file of a kind; falls back
   * to the most recent remaining file when the active one closes. */
  closeOfficeDoc: (view: ViewId, path: string) => void
  /** Infer kind from extension, set path, add the view. */
  openOfficeDoc: (path: string) => void

  // PDF study mode (chat scoped to an open document)
  scopedView?: ViewId
  setScopedView: (v?: ViewId) => void
  /** P33 scoped-PDF fix — the open document's extracted text (grounding). */
  scopedDoc?: { title: string; content: string }
  setScopedDoc: (d?: { title: string; content: string }) => void
  // P15-H29 — artifact server + inline action checklist.
  artifactServer: ArtifactServerState | null
  artifactActions: ArtifactActionUi[]
  patchArtifactServer: (server: ArtifactServerState | null) => void
  setArtifactActions: (actions: ArtifactActionUi[]) => void
  /** Spooled tool output inspection surface (the `tool-output` right-rail
   * view). Ephemeral projection state — the full cleaned text of one tool
   * result, never persisted. Opening it is a lens action, not a lease. */
  spooledOutput: SpooledOutput | null
  openSpooledOutput: (s: Omit<SpooledOutput, 'openedAt'>) => void

  // P30.12 — AIPointer quick-ask overlay (hotkey-anchored ask-box).
  aiPointerOpen: boolean
  setAiPointerOpen: (v: boolean) => void

  // P30.15 — visible memory consolidation (Dream Diary / morning brief).
  dreamDiary: DiaryEntry[]
  pushDiaryEntry: (e: DiaryEntry) => void
  clearDiary: () => void

  // Sidebar collapse
  sidebarCollapsed: boolean
  toggleSidebar: () => void

  // Progressive disclosure (B9/P31) — casual (default) vs power mode.
  // Casual = collapsed rail (agent switcher · new chat · recents · settings).
  // Power  = full 248px nav + right activity rail + advanced panels.
  powerMode: boolean
  togglePowerMode: () => void
  setPowerMode: (v: boolean) => void

  // Developer telemetry (status-bar debug strip) — off by default.
  devMode: boolean
  setDevMode: (v: boolean) => void
  /** P51.25 — which live pills the status bar may render (per-pill
   * customization, persisted to localStorage). */
  statusBarPills: StatusBarPills
  setStatusBarPills: (p: StatusBarPills) => void

  // Chat composer
  composerMode: ChatMode
  setComposerMode: (m: ChatMode) => void
  composerValue: string
  setComposerValue: (v: string) => void

  // Underlying agent runtime + model selection (Claude Code / Codex / Grok Build / etc.)
  selectedAgentId: string
  setSelectedAgent: (id: string) => void
  selectedModelId: string
  /** P58.7 — the catalog provider a live models.dev pick came from. Set only
   * by the picker's catalog rows; `undefined` means the selection is a curated
   * `MODELS` row and the send path resolves it via `MODEL_MAP`. */
  selectedModelProvider?: string
  /** P58.7 — `provider` is set only for a catalog pick (see above). */
  setSelectedModel: (id: string, provider?: string) => void
  /** P51.3 (UI slice) — cycle the current agent's model variants (next /
   * previous). Pinning a variant turns auto-route off so the pick is real,
   * same semantics as clicking a row in the picker. Returns the chosen id. */
  cycleModelVariant: (dir: 1 | -1) => string | undefined
  personaId: string
  setPersonaId: (id: string) => void
  soulId: string
  setSoulId: (id: string) => void
  /** P32.2 — the agent's chosen name (name-your-agent ownership moment). */
  agentName: string
  setAgentName: (name: string) => void
  streamStats: StreamStats
  // P11.5.12 — reconnect chip state (dropped IPC stream → auto-resume).
  reconnect: { show: boolean; lastToken: string; tokens: number }
  setReconnect: (r: { show: boolean; lastToken: string; tokens: number }) => void
  noteStreamTick: (tokenCount: number, sessionId?: string, streamId?: string) => void
  forkFromMessage: (messageId: string) => void

  // Auto-route per task kind — when true, agent selection follows routing table
  autoRoute: boolean
  setAutoRoute: (v: boolean) => void
  routing: Record<TaskKind, string>
  setRouting: (task: TaskKind, agentId: string) => void

  // Left-panel mode (which sub-screen is showing in the center for non-chat panels)
  centerScreen:
    | 'home'
    | 'chat'
    | 'activity'
    | 'projects'
    | 'files'
    | 'automations'
    | 'memory'
    | 'guard'
    | 'connectors'
    | 'analytics'
    | 'agents'
    | 'settings'
  setCenterScreen: (s: AppState['centerScreen']) => void
  settingsSection: SettingsSectionId
  setSettingsSection: (s: SettingsSectionId) => void
  permissionMode: PermissionMode
  setPermissionMode: (m: PermissionMode) => void
  /** P44.5 — reconcile the UI level with the Rust GuardService preset on boot. */
  syncAutonomyFromRust: () => Promise<void>
  // P44.6 — frozen per-task autonomy snapshot + temporary elevation.
  taskSnapshot?: TaskSnapshot
  freezeTaskSnapshot: () => void
  clearTaskSnapshot: () => void
  /** Apply an autonomy-limit card choice (Do Once / Allow For This Task /
   * Change Level) to the frozen task snapshot. Returns the resulting level. */
  respondAutonomyCard: (id: string, choice: string) => PermissionMode
  /** P44.6 — surface an autonomy-limit card (escalation) for a blocked
   * action. Callers: the bridge when a Guard ticket arrives while a task is
   * frozen at a lower autonomy level. */
  pushAutonomyLimit: (opts: {
    id: string
    action: string
    reason: string
    sessionId?: string
  }) => void
  /** Effective level for the live indicator: elevation wins while valid,
   * then the frozen level, then the global mode when idle. */
  effectiveAutonomyLevel: () => PermissionMode
  composerRole: ComposerRole
  setComposerRole: (r: ComposerRole) => void
  taskFolder?: string
  setTaskFolder: (folder?: string) => void

  // Office flyout state
  officeFlyoutOpen: boolean
  setOfficeFlyoutOpen: (v: boolean) => void

  // Command palette
  paletteOpen: boolean
  setPaletteOpen: (v: boolean) => void

  // Cockpit slide-over (P3.2 multi-agent flight deck)
  cockpitOpen: boolean
  setCockpitOpen: (v: boolean) => void

  // Pause/resume
  agentPaused: boolean
  toggleAgentPause: () => void

  // Toast trigger helper (kept simple)
  lastToast?: string
  lastToastKind?: 'default' | 'error'
  notify: (msg: string, kind?: 'default' | 'error') => void
  notifyMcpError: (msg: string) => void

  // P50.2.5 — live notification event stream: real events pushed by the
  // bridge (chat wire events + Guard-2 tickets) when the shell is up. The
  // notifications popover renders this in Tauri; it stays empty until a live
  // event lands, so a fresh shell never shows synthetic activity.
  liveNotifications: LiveNotification[]
  pushLiveNotification: (n: LiveNotification) => void
  markLiveNotificationsRead: () => void

  // Live data (bridge) — empty until the shell answers
  liveAgents: AgentRuntime[]
  setLiveAgents: (a: AgentRuntime[]) => void
  liveBudget?: LiveBudget
  setLiveBudget: (b: LiveBudget) => void

  /** Bugfix — the live `chat_stream` id per session, so Stop/Pause can cancel
   * the real Rust stream (`chat_cancel`) instead of only flipping local state. */
  liveStreamId: Record<string, string>
  setLiveStreamId: (sessionId: string, streamId: string) => void
  clearLiveStreamId: (sessionId: string, streamId?: string) => void

  // Chat streaming (bridge) — real turns through the Tauri relay
  pushUserMessage: (text: string) => void
  streamStart: (sessionId?: string, streamId?: string) => void
  /** P52.23 — append a reasoning delta to the live assistant turn.
   * Provider reasoning arrives as a *delta stream*, so consecutive chunks
   * coalesce onto the last thought entry; the entry list therefore holds
   * displayable thought blocks, not raw wire events. No-op when no assistant
   * turn is streaming (a stray reasoning event is never fabricated into a
   * message). */
  appendReasoning: (text: string, sessionId?: string, streamId?: string) => void

  /** P51.5 — queue-while-generating: turns sent while the session's stream is
   * live land here (pending chips above the composer) and fire automatically
   * when the current turn ends. */
  queueTurn: (sessionId: string, text: string, context?: { title: string; content: string }) => void
  /** Edit a queued turn's text in place (pending-chip inline editing). */
  editQueuedTurn: (sessionId: string, queueId: string, text: string) => void
  /** Remove one queued turn (chip ×). */
  removeQueuedTurn: (sessionId: string, queueId: string) => void
  /** P52.8 — pause auto-fire of a session's queue (chips stay visible,
   * nothing dispatches until resumed). */
  setQueuePaused: (sessionId: string, paused: boolean) => void
  /** P52.8 — move a queued turn to the head of the queue (Send-Now / steer). */
  promoteQueuedTurn: (sessionId: string, queueId: string) => void
  /** P52.16 — re-open a session deleted earlier this run (recent-closed). */
  reopenClosedSession: () => boolean
  /** Dispatch the head of a session's queue when its stream is idle. Called
   * from every terminal stream action and from the chip's send button. */
  dequeueNextTurn: (sessionId: string) => void
  /** Register the bridge dispatcher (once). */
  setTurnDispatcher: (fn: TurnDispatcher) => void

  streamAppend: (text: string, done: boolean, sessionId?: string, streamId?: string) => void
  streamFinalize: (fullText: string, sessionId?: string, streamId?: string) => void
  streamFail: (msg: string, sessionId?: string, error?: ChatError, streamId?: string) => void
  streamBudgetKill: (msg: string, sessionId?: string, streamId?: string) => void
  streamCancelled: (sessionId?: string, streamId?: string) => void
  streamStep: (label: string, sessionId?: string, streamId?: string) => void
  streamToolCall: (toolId: string, args?: Record<string, unknown>, risk?: string, sessionId?: string, streamId?: string) => void
  streamToolResult: (toolId: string, result?: unknown, error?: string, sessionId?: string, streamId?: string) => void
  streamCitations: (
    citations: Array<{ index: number; title: string; url: string; snippet?: string; source?: string }>,
    sessionId?: string,
    streamId?: string,
  ) => void
  streamToolProgress: (toolId: string, progress: string, sessionId?: string, streamId?: string) => void
  retryToolCall: (recordId: string) => Promise<void>

  // Guard-2 tickets (bridge) — live approval cards in the transcript
  pushMcq: (mcq: MCQInterrupt, sessionId?: string) => void
  respondMcq: (id: string, choice: string) => void

  /** P52.16 — sessions deleted this run (for reopen-closed). Each row keeps
   * its transcript so reopening restores the full chat. */
  closedSessions: Session[]
  /** P52.16 — jump to the next/previous session (Ctrl+Tab cycling). */
  cycleSession: (dir: 1 | -1) => void
  /** P52.15 — reopen a specific closed session from the ring (Archive view). */
  reopenClosedSessionId: (id: string) => boolean
  /** P52.15 — permanently forget one closed session (trash). */
  purgeClosedSession: (id: string) => void
  /** P52.15 — empty the whole closed ring. */
  purgeAllClosed: () => void

  /**
   * Live ACP handle table keyed by application Session + binding + Work (+ the
   * catalog agent id). The value stays an opaque provider handle; the typed
   * owner record is materialized by `getAcpHandle`.
   */
  acpHandles: Record<string, string>
  setAcpHandle: (record: AcpHandleRecord) => void
  getAcpHandle: (
    sessionId: string,
    agentId?: string,
    bindingId?: string,
    workId?: string,
  ) => AcpHandleRecord | undefined
  clearAcpHandles: (sessionId: string, handle?: string) => void
  /** Agent-owned ACP config options keyed by catalog agent id. Native provider
   * model state never enters this map. */
  acpConfigOptions: Record<string, import('./acp').AcpConfigOption[]>
  setAcpConfigOptions: (agentId: string, options: import('./acp').AcpConfigOption[]) => void

  pendingPlan?: {
    planId: string
    sessionId: string
    streamId?: string
    workId?: string
    tasks: { id: string; goal: string; dependsOn?: string[] }[]
  }
  setPendingPlan: (
    p: {
      planId: string
      sessionId: string
      streamId?: string
      workId?: string
      tasks: { id: string; goal: string; dependsOn?: string[] }[]
    } | undefined,
  ) => void

  // P41.3 — ticketed editor writes: ticketId → { path, content } waiting on
  // the Guard-2 approval card; the commit runs once the card is approved.
  /** P51.5 — per-session FIFO of queued user turns (pending chips). */
  pendingQueue: Record<string, QueuedTurn[]>
  /** P52.8 — per-session queue pause: while true the queue holds (chips stay
   * visible, dequeue is a no-op) until the user resumes. */
  queuePaused: Record<string, boolean>

  pendingEditorWrites: Record<string, { path: string; content: string }>
  parkEditorWrite: (ticketId: string, w: { path: string; content: string }) => void
  takeEditorWrite: (ticketId: string) => { path: string; content: string } | undefined

  // P41.4 — K1 verification receipts for the editor's Diff rail.
  verifications: VerificationRecord[]
  pushVerification: (v: VerificationRecord) => void

  // P11.2 — onboarding (first launch → add key → first chat → success)
  onboardingDone: boolean
  setOnboardingDone: (v: boolean) => void

  // P11.5.3 — per-session layout persistence
  sessionLayouts: Record<string, SessionLayout>
  saveSessionLayout: (sessionId: string, partial: Partial<SessionLayout>) => void
  restoreSessionLayout: (sessionId: string) => void

  // P11.5.4 — takeover / resume (per-session pause + describe-changes draft)
  pausedSessions: Record<string, boolean>
  setSessionPaused: (sessionId: string, paused: boolean) => void

  // P11.5.3 — real pending patches (fed by fs_undo_list)
  pendingPatches: PendingPatch[]
  setPendingPatches: (patches: PendingPatch[]) => void

  // P11.5.5 — NL automation draft text
  nlAutomationDraft?: string
  setNlAutomationDraft: (v?: string) => void

  // P66.4 — per-session capability loadout overrides
  setSessionCapabilityOverride: (sessionId: string, capabilityId: string, enabled: boolean) => void
  resetSessionCapabilities: (sessionId: string) => void
}

// Bind the test-isolation hook to the store once it exists (see
// resetStreamingTestState above).
streamTestReset = () => {
  useAppStore.setState({
    sessions: [],
    activeSessionId: '',
    sessionsHydrated: false,
    pendingQueue: {},
    queuePaused: {},
    closedSessions: [],
    liveStreamId: {},
    acpHandles: {},
    agentSendBlocker: undefined,
    setupOpen: false,
  })
}

export const useAppStore = create<AppState>((set, get) => ({
  // Tauri starts with no client-side fixtures. The shell hydrates this list
  // from the encrypted vault; only the plain-browser preview uses mock data.
  sessions: inTauri() ? [] : mockSessions,
  workItems: [],
  workEvents: [],
  walkthroughStops: [],
  setWorkProjection: (items, presence, events = []) => set({ workItems: items, workPresence: presence, workEvents: events }),
  coworkMode: false,
  setCoworkMode: (on) => set({ coworkMode: on }),
  activeSessionId: inTauri() ? '' : 's1',
  sessionsHydrated: false,
  markSessionsHydrated: () => set({ sessionsHydrated: true }),
  setActiveSession: (id) => {
    // P11.5.3 — persist the outgoing session's layout, restore the incoming.
    const st = get()
    if (st.activeSessionId !== id) {
      st.saveSessionLayout(st.activeSessionId, {
        view: st.activeView,
        railCollapsed: st.railCollapsed,
        composerMode: st.composerMode,
        officeDoc: st.sessions.find((x) => x.id === st.activeSessionId)?.officeDoc,
      })
      set({ activeSessionId: id, centerScreen: 'chat' })
      st.restoreSessionLayout(id)
    } else {
      set({ activeSessionId: id, centerScreen: 'chat' })
    }
  },
  newSession: () => {
    const id = freshId('s')
    // P11.6.4 — local UX metric: a user-created session.
    recordSessionCreated()
    // A user-created session is a real turn target — once one exists the
    // store owns live state (bugfix 3): stop persisting the demo seed.
    if (!get().sessionsHydrated) set({ sessionsHydrated: true })
    // P32.6 — fewest-questions context inheritance: pre-fill the folder so
    // the first ask needs no setup (onboarding stays enforced).
    const inherited = inheritContext()
    const fresh: Session = {
      id,
      title: inherited.title ?? 'New work',
      status: 'idle',
      preview: 'What would you like to do?',
      updatedAt: new Date().toISOString(),
      agent: '',
      messages: [],
    }
    set((s) => ({
      sessions: [fresh, ...s.sessions],
      activeSessionId: id,
      centerScreen: 'chat',
      composerValue: '',
      ...(inherited.folder ? { taskFolder: inherited.folder } : {}),
    }))
  },
  openAutomationRun: (run) => {
    // ADR-0006 §7 — the run's Session gains a Chat here; the Chat is the
    // continuation, not a second Session. The title is the run's own Work
    // objective, never an invented label.
    const id = freshId('s')
    const objective = run.objective.trim() || 'Automation run'
    if (!get().sessionsHydrated) set({ sessionsHydrated: true })
    const fresh: Session = {
      id,
      title: objective.length > 60 ? `${objective.slice(0, 57)}…` : objective,
      status: 'idle',
      preview: 'Opened from an automation run — continue or inspect it here.',
      updatedAt: new Date().toISOString(),
      messages: [],
      originRunId: run.id,
      originRunSessionId: run.sessionId,
    }
    set((s) => ({
      sessions: [fresh, ...s.sessions],
      activeSessionId: id,
      centerScreen: 'chat',
      composerValue: '',
    }))
    get().notify('Automation run opened as a chat')
    return id
  },
  monitorBadge: { count: 0, stopped: false },
  pushMonitor: (ev) => {
    set((s) => ({
      monitorBadge: {
        count: ev.notified ? s.monitorBadge.count + 1 : s.monitorBadge.count,
        last: ev.current,
        stopped: ev.stopped || s.monitorBadge.stopped,
      },
    }))
    if (ev.notified) {
      get().notify(
        ev.stopped ? `Monitor stopped · ${ev.current}` : `Monitor · ${ev.current}`,
      )
    }
  },
  clearMonitorBadge: () => set({ monitorBadge: { count: 0, stopped: false } }),
  deleteSession: async (id) => {
    if (inTauri()) {
      try {
        const { schedulerPauseSession, invoke } = await import('./tauri')
        await schedulerPauseSession(id)
        await nativeCall('session delete', () => invoke('session_delete', { sessionId: id }))
      } catch (error) {
        get().notify(`Could not delete this work: ${error instanceof Error ? error.message : String(error)}`, 'error')
        return
      }
    }
    const st = get()
    // Handles are Session-owned, not agent-owned. Remove the deleted Session's
    // records so a later chat can never inherit a stale provider handle.
    st.clearAcpHandles(id)
    const remaining = st.sessions.filter((x) => x.id !== id)
    const nextActive =
      st.activeSessionId === id ? (remaining[0]?.id ?? '') : st.activeSessionId
    // P38 — a deleted session takes its Chief pin with it (no orphan pin).
    const pins = { ...st.sessionChiefs }
    delete pins[id]
    set({
      sessions: remaining,
      activeSessionId: nextActive,
      sessionChiefs: pins,
      // P52.16 — keep the deleted session (with its transcript) so it can be
      // reopened this run. Bounded ring: keep the last 5 closed.
      closedSessions: (
        [...st.closedSessions, st.sessions.find((x) => x.id === id)].filter(
          (x): x is Session => Boolean(x),
        ) as Session[]
      ).slice(-5),
    })
  },
  reopenClosedSession: () => {
    const st = get()
    const last = st.closedSessions[st.closedSessions.length - 1]
    if (!last) return false
    return st.reopenClosedSessionId(last.id)
  },
  // P52.15 — restore a specific closed row under a fresh id (the old vault
  // row is gone; this is a new live row the persist subscription writes on
  // the next change). Shared by the Reopen-last action and the Archive flyout.
  reopenClosedSessionId: (id) => {
    const st = get()
    const row = st.closedSessions.find((x) => x.id === id)
    if (!row) return false
    const nid = freshId('s')
    const copy: Session = {
      ...row,
      id: nid,
      status: 'idle',
      updatedAt: new Date().toISOString(),
      title: `${row.title}`,
      messages: row.messages.map((m) => ({
        ...m,
        toolCalls: m.toolCalls?.map((t) => ({ ...t })),
        steps: m.steps?.map((s) => ({ ...s })),
        artifacts: m.artifacts?.map((a) => ({ ...a })),
      })),
    }
    set((s) => ({
      sessions: [copy, ...s.sessions],
      activeSessionId: nid,
      closedSessions: s.closedSessions.filter((x) => x.id !== id),
      centerScreen: 'chat',
    }))
    return true
  },
  purgeClosedSession: (id) =>
    set((s) => ({ closedSessions: s.closedSessions.filter((x) => x.id !== id) })),
  purgeAllClosed: () => set({ closedSessions: [] }),
  cycleSession: (dir) => {
    const st = get()
    const list = st.sessions
    if (list.length === 0) return
    const cur = list.findIndex((x) => x.id === st.activeSessionId)
    const next = (cur + dir + list.length) % list.length
    const target = list[next]
    if (target) st.setActiveSession(target.id)
  },
  renameSession: (id, title) => {
    const name = title.trim()
    if (!name) return
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === id ? { ...x, title: name, updatedAt: new Date().toISOString() } : x,
      ),
    }))
  },
  toggleSessionPinned: (id) => {
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === id ? { ...x, pinned: !x.pinned } : x,
      ),
    }))
  },
  clearSessionMessages: (id) => {
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === id
          ? { ...x, messages: [], status: 'idle' as const, preview: 'What would you like to do?', updatedAt: new Date().toISOString() }
          : x,
      ),
    }))
  },
  /** P51.33 — apply a `/compact` verdict to the transcript: keep everything
   * from `keptFrom` (index into the pre-compact message list) and, when the
   * Rust side pruned an overflow, prepend the synthetic continue marker as a
   * system message so the visible cut is honest. Refuses to run mid-turn
   * (caller checks), never drops the live stream. */
  compactSessionMessages: (id, keptFrom, marker) => {
    set((s) => ({
      sessions: s.sessions.map((x) => {
        if (x.id !== id || x.status === 'running' || x.status === 'action-required') return x
        const kept = x.messages.slice(keptFrom)
        if (kept.length === 0 || kept.length === x.messages.length) return x
        const markerMsg = marker
          ? [{
              id: `compact-${Date.now()}`,
              role: 'system' as const,
              content: marker,
              timestamp: new Date().toISOString(),
            }]
          : []
        return {
          ...x,
          messages: [...markerMsg, ...kept],
          preview: 'Context compacted',
          updatedAt: new Date().toISOString(),
        }
      }),
    }))
  },
  setSessionGoal: (id, goal) => {
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === id
          ? { ...x, goal: goal?.trim() ? goal.trim() : undefined, goalAchieved: goal ? x.goalAchieved : false }
          : x,
      ),
    }))
  },
  markGoalAchieved: (id, achieved) => {
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === id ? { ...x, goalAchieved: achieved } : x)),
    }))
  },
  rewindToUserMessage: (sessionId, messageId) => {
    const s = get()
    const sess = s.sessions.find((x) => x.id === sessionId)
    if (!sess) return null
    // Never rewrite a live transcript (the turn owns the tail).
    if (sess.status === 'running' || sess.status === 'action-required') return null
    const idx = sess.messages.findIndex((m) => m.id === messageId)
    if (idx < 0 || sess.messages[idx]?.role !== 'user') return null
    const text = sess.messages[idx]!.content
    set((st) => ({
      sessions: st.sessions.map((x) =>
        x.id === sessionId
          ? { ...x, messages: x.messages.slice(0, idx), status: 'idle' as const }
          : x,
      ),
    }))
    return text
  },
  rewindBeforeAssistant: (sessionId, assistantMessageId) => {
    const s = get()
    const sess = s.sessions.find((x) => x.id === sessionId)
    if (!sess) return null
    if (sess.status === 'running' || sess.status === 'action-required') return null
    const idx = sess.messages.findIndex((m) => m.id === assistantMessageId)
    if (idx < 0 || sess.messages[idx]?.role !== 'assistant') return null
    // The user turn that produced this answer is the message before it. Cut
    // from that prompt (inclusive) so the re-ask runs in place with full
    // history intact above.
    let promptIdx = idx - 1
    while (promptIdx >= 0 && sess.messages[promptIdx]?.role !== 'user') promptIdx -= 1
    if (promptIdx < 0) return null
    const text = sess.messages[promptIdx]!.content
    set((st) => ({
      sessions: st.sessions.map((x) =>
        x.id === sessionId
          ? { ...x, messages: x.messages.slice(0, promptIdx), status: 'idle' as const }
          : x,
      ),
    }))
    return text
  },
  forkSession: (id) => {
    const src = get().sessions.find((x) => x.id === id)
    if (!src) return null
    const nid = freshId('s')
    const copy: Session = {
      ...src,
      id: nid,
      title: `${src.title} (fork)`,
      status: 'idle',
      updatedAt: new Date().toISOString(),
      messages: src.messages.map((m) => ({
        ...m,
        toolCalls: m.toolCalls?.map((t) => ({ ...t })),
        steps: m.steps?.map((st) => ({ ...st })),
        artifacts: m.artifacts?.map((a) => ({ ...a })),
      })),
      pinned: false,
      chiefPin: undefined,
      chiefUnpinned: undefined,
    }
    set((s) => ({ sessions: [copy, ...s.sessions], activeSessionId: nid }))
    return nid
  },

  activeView: 'office-xlsx',
  setActiveView: (v) => {
    set((s) => ({
      activeView: v,
      railCollapsed: false,
      powerMode: true,
      openViews: s.openViews.includes(v) ? s.openViews : [...s.openViews, v],
    }))
    // P11.5.3 — persist the layout as it changes.
    get().saveSessionLayout(get().activeSessionId, { view: v, railCollapsed: false })
  },
  browserUrl: null,
  browserAttached: false,
  setBrowserAttached: (attached) => set({ browserAttached: attached }),
  clearBrowserUrl: () => set({ browserUrl: null }),
  desktopAttached: false,
  desktopReason: null,
  setDesktopAttached: (attached, reason) =>
    set({ desktopAttached: attached, desktopReason: reason ?? null }),
  cuaVisionGate: false,
  setCuaVisionGate: (open) => set({ cuaVisionGate: open }),
  providerKeysConfigured: null,
  setProviderKeysConfigured: (configured) => set({ providerKeysConfigured: configured }),
  setupOpen: false,
  openSetup: () => set({ setupOpen: true }),
  closeSetup: () => set({ setupOpen: false }),
  agentSendBlocker: undefined,
  setAgentSendBlocker: (blocker) => set({ agentSendBlocker: blocker }),

  // P38 — starts empty: no session is pinned until the user pins one. The
  // pin is ALSO written onto the Session object so the vault persist
  // round-trip (`session_put`) makes it durable across app restarts; the
  // mirror map is rehydrated from the loaded sessions at bridge start.
  sessionChiefs: {},
  setSessionChiefPin: (sessionId, chiefId) =>
    set((s) => ({
      sessionChiefs: { ...s.sessionChiefs, [sessionId]: chiefId },
      sessions: s.sessions.map((x) =>
        // A fresh pin clears any prior explicit-unpin marker.
        x.id === sessionId ? { ...x, chiefPin: chiefId, chiefUnpinned: false } : x,
      ),
    })),
  clearSessionChiefPin: (sessionId) =>
    set((s) => {
      const next = { ...s.sessionChiefs }
      delete next[sessionId]
      return {
        sessionChiefs: next,
        // Persist an explicit unpin: the session HAD a pin (or a marker) and
        // now follows the user default again. The marker rides the vault
        // round-trip so a restarted session shows "default applies — pin
        // cleared" instead of reading as never-pinned.
        sessions: s.sessions.map((x) =>
          x.id === sessionId
            ? { ...(({ chiefPin, ...rest }) => rest)(x), chiefUnpinned: true }
            : x,
        ),
      }
    }),
  userDefaultChief: undefined,
  setUserDefaultChief: (chiefId) => set({ userDefaultChief: chiefId, agentSendBlocker: undefined }),
  openInBrowser: (url) => {
    set((s) => ({
      browserUrl: url,
      activeView: 'browse',
      railCollapsed: false,
      powerMode: true,
      openViews: s.openViews.includes('browse') ? s.openViews : [...s.openViews, 'browse'],
    }))
    get().saveSessionLayout(get().activeSessionId, { view: 'browse', railCollapsed: false })
  },
  railCollapsed: false,
  toggleRail: () => {
    set((s) => ({ railCollapsed: !s.railCollapsed }))
    get().saveSessionLayout(get().activeSessionId, { railCollapsed: get().railCollapsed })
  },
  setRailCollapsed: (v) => {
    set({ railCollapsed: v })
    get().saveSessionLayout(get().activeSessionId, { railCollapsed: v })
  },
  fullscreenView: false,
  setFullscreenView: (v) => set({ fullscreenView: v }),

  openViews: ['folder', 'shell', 'browse', 'office-xlsx'],
  addView: (v) => {
    set((s) => ({
      openViews: s.openViews.includes(v) ? s.openViews : [...s.openViews, v],
      activeView: v,
      railCollapsed: false,
      powerMode: true,
    }))
    // P33.7 — persist the tab set per session.
    get().saveSessionLayout(get().activeSessionId, { openViews: get().openViews })
  },
  closeView: (v) => {
    set((s) => {
      const next = s.openViews.filter((x) => x !== v)
      if (next.length === 0) return { openViews: next, railCollapsed: true, fullscreenView: false }
      const active = s.activeView === v ? next[next.length - 1] : s.activeView
      return { openViews: next, activeView: active, fullscreenView: false }
    })
    get().saveSessionLayout(get().activeSessionId, { openViews: get().openViews })
  },
  officePaths: {},
  officeHistory: {},
  switchOfficeDoc: (view, path) =>
    set((s) => ({ officePaths: { ...s.officePaths, [view]: path } })),
  // Office hold lift (P33.1 — office files as tabs): close one opened file of
  // a kind. If it was the active file, fall back to the most recent remaining
  // one (the view stays open until its last tab is closed).
  closeOfficeDoc: (view, path) =>
    set((s) => {
      const history = s.officeHistory[view] ?? []
      const next = history.filter((p) => p !== path)
      const wasActive = s.officePaths[view] === path
      const fallback = wasActive ? (next[0] ?? null) : s.officePaths[view]
      return {
        officePaths: { ...s.officePaths, [view]: fallback ?? undefined },
        officeHistory: { ...s.officeHistory, [view]: next },
      }
    }),
  openOfficeDoc: (path) => {
    const ext = path.split('.').pop()?.toLowerCase() ?? ''
    const view: ViewId | undefined =
      ext === 'xlsx' || ext === 'xlsm'
        ? 'office-xlsx'
        : ext === 'docx'
          ? 'office-docx'
          : ext === 'pptx'
            ? 'office-pptx'
            : ext === 'pdf'
              ? 'office-pdf'
              : undefined
    if (!view) {
      // P50.3.7 — never drop an open request silently: surface the supported
      // set so callers (rail prompt, palette, artifacts) explain the refusal.
      get().notify(`Cannot open “${path}” — supported: .docx .xlsx/.xlsm .pptx .pdf`)
      return
    }
    const label = path.split(/[\\/]/).pop() ?? path
    set((s) => {
      const history = s.officeHistory[view] ?? []
      const next = [path, ...history.filter((p) => p !== path)].slice(0, 8)
      return {
        officePaths: { ...s.officePaths, [view]: path },
        officeHistory: { ...s.officeHistory, [view]: next },
        sessions: s.sessions.map((x) =>
          x.id === s.activeSessionId ? { ...x, officeDoc: label, view } : x,
        ),
      }
    })
    get().addView(view)
  },
  reorderViews: (from, to) => {
    set((s) => {
      if (from === to || from < 0 || to < 0 || from >= s.openViews.length || to >= s.openViews.length) {
        return s
      }
      const next = [...s.openViews]
      const [moved] = next.splice(from, 1)
      next.splice(to, 0, moved!)
      return { openViews: next }
    })
    get().saveSessionLayout(get().activeSessionId, { openViews: get().openViews })
  },

  scopedView: undefined,
  setScopedView: (v) => set({ scopedView: v }),
  scopedDoc: undefined,
  setScopedDoc: (d) => set({ scopedDoc: d }),

  // P15-H29 — artifact server + inline action checklist.
  artifactServer: null as ArtifactServerState | null,
  artifactActions: [] as ArtifactActionUi[],
  patchArtifactServer: (server) => set({ artifactServer: server }),
  setArtifactActions: (actions) => set({ artifactActions: actions }),
  spooledOutput: null,
  openSpooledOutput: (s) => {
    set({ spooledOutput: { ...s, openedAt: Date.now() } })
    get().addView('tool-output')
  },

  aiPointerOpen: false,
  setAiPointerOpen: (v) => set({ aiPointerOpen: v }),

  dreamDiary: [],
  pushDiaryEntry: (e) =>
    set((s) => ({ dreamDiary: [e, ...s.dreamDiary].slice(0, 30) })),
  clearDiary: () => set({ dreamDiary: [] }),

  sidebarCollapsed: false,
  toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),

  powerMode: readPowerMode(),
  togglePowerMode: () =>
    set((s) => {
      const next = !s.powerMode
      writePowerMode(next)
      return { powerMode: next, fullscreenView: next ? s.fullscreenView : false }
    }),
  setPowerMode: (v) => {
    writePowerMode(v)
    set((s) => ({ powerMode: v, fullscreenView: v ? s.fullscreenView : false }))
  },

  devMode: false,
  setDevMode: (v) => set({ devMode: v }),
  // P51.25 — per-pill status-bar prefs (read once at boot, written on change).
  statusBarPills: readStatusBarPills(),
  setStatusBarPills: (p) => {
    writeStatusBarPills(p)
    set({ statusBarPills: p })
  },

  composerMode: 'auto',
  setComposerMode: (m) => set({ composerMode: normalizeChatMode(m) }),
  composerValue: '',
  setComposerValue: (v) => set({ composerValue: v }),

  // P71.2a/2d — selection starts **unbound**: an empty id means "no agent bound
  // yet", and the turn path leads with discovery rather than substituting an
  // engine (ADR-0005 §1). There is no built-in row to default to.
  selectedAgentId: '',
  setSelectedAgent: (id) => {
    // Occupancy is live: a row published by the ACP registry refresh is
    // selectable even though it has no entry in the static seed. The static
    // map stays the fallback for the pre-hydration UI.
    const row = AGENT_MAP[id] ?? get().liveAgents.find((a) => a.id === id)
    if (!row) return
    // P60/P71.2d — model ownership. An external ACP agent owns its model, auth
    // and routing; since *every* runtime is external now, selection never snaps
    // a desktop-side model pin onto an agent and never resets one on the way out.
    set({ selectedAgentId: id, agentSendBlocker: undefined })
  },
  // P71.2d — no desktop-side model pin exists to default: any model a turn uses
  // belongs to the agent. Kept as an empty string so the composer can say "the
  // agent decides" rather than naming a model nobody receives.
  selectedModelId: '',
  selectedModelProvider: undefined,
  setSelectedModel: (id, provider) =>
    set({ selectedModelId: id, selectedModelProvider: provider }),
  // P51.3 — variant cycle over the current agent's available models. The
  // agent's own control surface owns the choice; this compatibility action
  // never creates a host-side provider/model pin.
  cycleModelVariant: (_dir) => {
    // P71.2d — there is no AgentCowork-owned model list to cycle. A bound agent's
    // model is switched through that agent's own ACP config options (the picker
    // renders them from `available_commands_update` / session config), never by
    // AgentCowork mutating a pin the agent never receives. Returning `undefined`
    // is the honest answer, so callers fall through instead of showing a switch
    // that did nothing.
    return undefined
  },
  personaId: 'straight-shooter',
  setPersonaId: (id) => set({ personaId: id }),
  soulId: 'default',
  setSoulId: (id) => set({ soulId: id }),
  agentName: '',
  setAgentName: (name) => set({ agentName: name }),
  streamStats: { tokensPerSec: 0, ctxPct: 0, tokensThisTurn: 0 },
  // P11.5.12 — reconnect chip state: set when the IPC stream drops, cleared
  // when the stream resumes or the user dismisses the chip.
  reconnect: { show: false, lastToken: '', tokens: 0 },
  setReconnect: (r) => set({ reconnect: r }),
  noteStreamTick: (tokenCount, sessionId?, streamId?) => {
    const sid = streamSessionId(sessionId)
    if (streamId !== undefined && !bindStreamId(sid, streamId)) return
    const now = Date.now()
    if (streamT0BySession[sid] === undefined) streamT0BySession[sid] = now
    streamTokBySession[sid] = (streamTokBySession[sid] ?? 0) + tokenCount
    const elapsed = Math.max(0.25, (now - streamT0BySession[sid]!) / 1000)
    set((s) => {
      // Keep the status-bar counters tied to the visible session; background
      // sessions still retain independent counters for when the user returns.
      if (s.activeSessionId !== sid) return s
      const sess = s.sessions.find((x) => x.id === sid)
      const tokens = streamTokBySession[sid] ?? 0
      const used = (sess?.tokens ?? 0) + tokens
      const ctxWindow = 128_000
      return {
        streamStats: {
          tokensPerSec: tokens / elapsed,
          tokensThisTurn: tokens,
          ctxPct: Math.min(100, Math.round((used / ctxWindow) * 100)),
          activeKey: s.streamStats.activeKey,
        },
      }
    })
  },
  forkFromMessage: (messageId) => {
    const s = get()
    const cur = s.sessions.find((x) => x.id === s.activeSessionId)
    if (!cur) return
    const idx = cur.messages.findIndex((m) => m.id === messageId)
    if (idx < 0) return
    const id = `s-${Date.now()}`
    const forked: Session = {
      ...cur,
      id,
      parentId: cur.id,
      title: `${cur.title} ⑂`,
      status: 'idle',
      updatedAt: new Date().toISOString(),
      messages: cur.messages.slice(0, idx + 1).map((m) => ({ ...m })),
      children: undefined,
    }
    set({
      sessions: [forked, ...s.sessions],
      activeSessionId: id,
      centerScreen: 'chat',
    })
    get().notify(`Forked from message — history truncated after that turn`)
  },

  autoRoute: true,
  setAutoRoute: (v) => set({ autoRoute: v }),
  routing: { ...DEFAULT_ROUTING },
  setRouting: (task, agentId) =>
    set((s) => ({ routing: { ...s.routing, [task]: agentId } })),

  centerScreen: 'home',
  setCenterScreen: (s) => set({ centerScreen: s }),
  settingsSection: 'agents',
  setSettingsSection: (s) => set({ settingsSection: s }),
  permissionMode: readPermission(),
  setPermissionMode: (m) => {
    writePermission(m)
    set({ permissionMode: m })
    // P44.5 — push the level to the live GuardService (Rust preset is
    // authoritative). The returned level confirms what actually applied.
    if (inTauri()) {
      void guardSetAutonomy(m).then((applied) => {
        if (applied && applied !== m) {
          writePermission(applied)
          set({ permissionMode: applied })
        }
      })
    }
  },

  // P44.5 — reconcile the UI autonomy level with the Rust preset on load:
  // the applied GuardService level wins over stale localStorage.
  syncAutonomyFromRust: async () => {
    if (!inTauri()) return
    const level = await guardAutonomy()
    if (level) {
      writePermission(level)
      set({ permissionMode: level })
    }
  },

  // P44.6 — freeze the autonomy scope at task start; live chatbar changes
  // never mutate an in-flight Work (same principle as scheduled-run
  // snapshots — the config_hash is computed once and stays put).
  taskSnapshot: undefined,
  freezeTaskSnapshot: () => {
    const s = get()
    const session = s.sessions.find((x) => x.id === s.activeSessionId)
    const workspace = session?.folder ?? s.taskFolder ?? '~'
    // P71.2a — an unbound session freezes as `''`, not as the retired built-in
    // id: the snapshot must not name an engine that does not exist.
    const agent = s.selectedAgentId || ''
    const configHash = taskScopeHash(s.permissionMode, s.composerMode, workspace, agent)
    set({
      taskSnapshot: {
        frozenAt: Date.now(),
        autonomyLevel: s.permissionMode,
        mode: s.composerMode,
        workspaceScope: workspace,
        agentScope: agent,
        sessionId: s.activeSessionId,
        configHash,
      },
    })
  },
  clearTaskSnapshot: () => set({ taskSnapshot: undefined }),
  respondAutonomyCard: (id, choice) => {
    const snap = get().taskSnapshot
    const base = snap?.autonomyLevel ?? get().permissionMode
    let nextLevel: PermissionMode = base
    const now = Date.now()
    if (choice === 'do-once') {
      nextLevel = 'auto'
      set({
        taskSnapshot: snap
          ? {
              ...snap,
              elevation: { level: 'auto', grantedAt: now, oneShot: true },
            }
          : undefined,
      })
    } else if (choice === 'allow-task') {
      nextLevel = 'auto'
      // Temporary elevation: expires when the task snapshot is cleared at
      // turn completion (and is wall-clock capped as a backstop).
      set({
        taskSnapshot: snap
          ? {
              ...snap,
              elevation: {
                level: 'auto',
                grantedAt: now,
                oneShot: false,
                elevatedUntil: now + 30 * 60_000,
              },
            }
          : undefined,
      })
    } else if (choice === 'change-level' && snap) {
      // Change Level — pick the level from the card options; the card sends
      // the target level as the choice value (e.g. "level:auto").
      const target = choice.startsWith('level:') ? (choice.slice(6) as PermissionMode) : 'auto'
      nextLevel = target
      set({
        permissionMode: target,
        taskSnapshot: { ...snap, autonomyLevel: target, elevation: undefined },
      })
      writePermission(target)
    }
    // Consume the card (clear it off the message, resume the session).
    set((s) => ({
      sessions: s.sessions.map((x) => ({
        ...x,
        status: x.status === 'action-required' ? 'running' : x.status,
        messages: x.messages
          .map((m) => (m.mcq?.id === id ? { ...m, mcq: undefined } : m))
          .filter((m) => m.content !== '' || m.mcq !== undefined || m.role !== 'assistant'),
      })),
    }))
    return nextLevel
  },
  pushAutonomyLimit: (opts) => {
    const snap = get().taskSnapshot
    const level = snap?.autonomyLevel ?? get().permissionMode
    get().pushMcq(
      {
        id: opts.id,
        title: 'Autonomy limit',
        description: `This action is above the frozen autonomy level (${level}) for this task.`, 
        kind: 'autonomy',
        autonomyAction: opts.action,
        autonomyReason: opts.reason,
        options: [
          { label: 'Do Once', value: 'do-once' },
          { label: 'Allow For This Task', value: 'allow-task' },
          { label: 'Change Level', value: 'change-level' },
        ],
      },
      opts.sessionId,
    )
  },
  effectiveAutonomyLevel: () => {
    const snap = get().taskSnapshot
    if (!snap) return get().permissionMode
    const el = snap.elevation
    if (el) {
      if (el.oneShot) {
        // Do Once — the elevation is consumed on first use.
        set({
          taskSnapshot: { ...snap, elevation: undefined },
        })
        return el.level
      }
      if (!el.elevatedUntil || el.elevatedUntil > Date.now()) return el.level
      // Wall-clock expiry backstop — elevation over.
      set({ taskSnapshot: { ...snap, elevation: undefined } })
    }
    return snap.autonomyLevel
  },
  composerRole: 'agent',
  setComposerRole: (r) => set({ composerRole: r }),
  taskFolder: undefined,
  setTaskFolder: (folder) => set({ taskFolder: folder }),

  officeFlyoutOpen: false,
  setOfficeFlyoutOpen: (v) => set({ officeFlyoutOpen: v }),

  paletteOpen: false,
  setPaletteOpen: (v) => set({ paletteOpen: v }),

  cockpitOpen: false,
  setCockpitOpen: (v) => set({ cockpitOpen: v }),

  agentPaused: false,
  toggleAgentPause: () => {
    const s = get()
    const pausing = !s.agentPaused
    if (pausing && inTauri()) {
      // P71.2c — the turn runs inside the bound agent's own process, so a
      // pause has to reach *that* agent: cancel its live ACP session. The
      // native provider stream (and its `chat_cancel` handle) is gone with the
      // built-in engine, so there is no stream id to cancel.
      const bound = s.sessionChiefs[s.activeSessionId] ?? s.userDefaultChief ?? ''
      const record = bound ? s.getAcpHandle(s.activeSessionId, bound) : undefined
      if (record?.bindingId && record.workId) {
        void import('./acp').then(({ acpCancel }) => {
          void acpCancel(record.handle, record.applicationSessionId, record.bindingId).catch(() => {})
        })
      }
    }
    set({ agentPaused: pausing })
  },

  lastToast: undefined,
  lastToastKind: 'default',
  notify: (msg, kind = 'default') => set({ lastToast: msg, lastToastKind: kind }),
  notifyMcpError: (msg) => set({ lastToast: msg, lastToastKind: 'error' }),

  liveAgents: [],
  setLiveAgents: (a) => set({ liveAgents: a }),
  liveBudget: undefined,
  setLiveBudget: (b) => set({ liveBudget: b }),

  // P50.2.5 — starts empty; only the bridge pushes real wire events.
  // `pushLiveNotification` upserts by id so a re-emit (or a read toggle) never
  // duplicates a row; the list is capped so the popover stays bounded.
  liveNotifications: [],
  pushLiveNotification: (n) =>
    set((s) => {
      const without = s.liveNotifications.filter((x) => x.id !== n.id)
      return { liveNotifications: [n, ...without].slice(0, 50) }
    }),
  markLiveNotificationsRead: () =>
    set((s) => ({ liveNotifications: s.liveNotifications.map((n) => ({ ...n, unread: false })) })),

  liveStreamId: {},
  setLiveStreamId: (sessionId, streamId) =>
    set((s) => ({ liveStreamId: { ...s.liveStreamId, [sessionId]: streamId } })),
  clearLiveStreamId: (sessionId, streamId) =>
    set((s) => {
      if (streamId !== undefined && s.liveStreamId[sessionId] !== streamId) return s
      const next = { ...s.liveStreamId }
      delete next[sessionId]
      return { liveStreamId: next }
    }),

  pushUserMessage: (text) => {
    const id = freshId('u')
    const msg: ChatMessage = {
      id,
      role: 'user',
      content: text,
      timestamp: new Date().toISOString(),
    }
    set((s) => ({
      composerValue: '',
      sessions: s.sessions.map((x) =>
        x.id === s.activeSessionId
          ? { ...x, status: 'running', messages: [...x.messages, msg] }
          : x,
      ),
    }))
  },
  streamStart: (sessionId?, streamId?) => {
    const sid = streamSessionId(sessionId)
    if (!bindStreamId(sid, streamId) || hasActiveStream(sid)) return
    const id = freshId(`a-${sid.slice(-6)}`)
    activeStreamMsg[sid] = id
    streamT0BySession[sid] = Date.now()
    streamTokBySession[sid] = 0
    const msg: ChatMessage = {
      id,
      role: 'assistant',
      content: '',
      timestamp: new Date().toISOString(),
      startedAt: Date.now(),
      ...(streamId !== undefined ? { streamId } : {}),
    }
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === sid
          ? { ...x, status: 'running', messages: [...x.messages, msg] }
          : x,
      ),
    }))
  },
  appendReasoning: (text, sessionId?, streamId?) => {
    const sid = streamSessionId(sessionId)
    const msgId = activeStreamMsg[sid]
    if (!msgId || (streamId !== undefined && !bindStreamId(sid, streamId))) return
    if (!text) return
    markFirstDelta(sid)
    patchStreamMessage(
      set,
      (m) => {
        const list = m.reasoning ?? []
        const prev = list.length > 0 ? list[list.length - 1]! : ''
        // Coalesce the delta onto the in-flight thought block. The last
        // entry stays open while this message is the active stream; a new
        // wire `reasoning` event for a *settled* message opens a fresh block.
        const reasoningStartedAt = m.reasoningStartedAt ?? Date.now()
        return {
          ...m,
          reasoning: [...list.slice(0, -1), prev + text],
          reasoningStartedAt,
        }
      },
      sessionId,
    )
    // The first reasoning delta is a turn-start signal too (ttft may not
    // have fired yet for pure-reasoning providers).
    if (streamT0BySession[sid] === undefined) streamT0BySession[sid] = Date.now()
  },
  streamAppend: (text, done, sessionId?, streamId?) => {
    const sid = streamSessionId(sessionId)
    const msgId = activeStreamMsg[sid]
    if (!msgId || !bindStreamId(sid, streamId)) return
    markFirstDelta(sid)
    const endedAt = done ? Date.now() : undefined
    const startedAt = get().sessions.find((x) => x.id === sid)?.messages.find((mm) => mm.id === msgId)?.startedAt
    const ttfb = done ? ttfbFor(sid, startedAt) : undefined
    set((s) => ({
      sessions: s.sessions.map((x) => {
        if (x.id !== sid) return x
        return {
          ...x,
          status: done ? 'completed' : 'running',
          messages: x.messages.map((m) =>
            m.id === msgId
              ? {
                  ...m,
                  content: m.content + text,
                  ...(endedAt ? { endedAt } : {}),
                  ...(ttfb !== undefined ? { ttfbMs: ttfb } : {}),
                }
              : m,
          ),
        }
      }),
    }))
    if (done) {
      retireStream(sid)
      delete activeStreamMsg[sid]
      delete activeStreamId[sid]
      delete streamFirstDelta[sid]
      delete streamT0BySession[sid]
      delete streamTokBySession[sid]
      get().clearLiveStreamId(sid)
      // P44.6 — the turn is over: the frozen snapshot + any temporary
      // elevation expire here (live changes never leak into the next task).
      set({ taskSnapshot: undefined })
      recordTurnCompleted()
      get().dequeueNextTurn(sid)
    }
  },
  /** P-bugfix 1: the `done` chat-event carries the *whole* message (fullText)
   * after `batch` deltas were already appended token-by-token. Appending it
   * would duplicate the reply, so this **replaces** the streamed content with
   * the authoritative final text (and clears the in-flight cursor). */
  streamFinalize: (fullText, sessionId?, streamId?) => {
    const sid = streamSessionId(sessionId)
    const msgId = activeStreamMsg[sid]
    if (!msgId || !bindStreamId(sid, streamId)) return
    const startedAt = get().sessions.find((x) => x.id === sid)?.messages.find((mm) => mm.id === msgId)?.startedAt
    const ttfb = ttfbFor(sid, startedAt)
    set((s) => ({
      sessions: s.sessions.map((x) => {
        if (x.id !== sid) return x
        return {
          ...x,
          status: 'completed',
          messages: x.messages.map((m) =>
            m.id === msgId
              ? {
                  ...m,
                  content: fullText,
                  endedAt: Date.now(),
                  ...(ttfb !== undefined ? { ttfbMs: ttfb } : {}),
                }
              : m,
          ),
        }
      }),
    }))
    retireStream(sid)
    delete activeStreamMsg[sid]
    delete activeStreamId[sid]
    delete streamFirstDelta[sid]
    delete streamT0BySession[sid]
    delete streamTokBySession[sid]
    get().clearLiveStreamId(sid)
    // P44.6 — turn complete: the frozen task scope + elevation expire.
    set({ taskSnapshot: undefined })
    // P11.6.4 — local UX metric: a completed turn.
    recordTurnCompleted()
    // P51.5 — the turn ended: fire the next queued ask for this session.
    get().dequeueNextTurn(sid)
  },
  /** P51.7/P51.21 — a failed turn. The partial content streamed so far is
   * **preserved** (never clobbered with a marker line); the structured
   * `error` renders as a layer-named card with matched actions. */
  streamFail: (msg, sessionId?, error?, streamId?) => {
    const sid = streamSessionId(sessionId)
    const msgId = activeStreamMsg[sid]
    if (streamId !== undefined && !bindStreamId(sid, streamId)) return
    // P11.6.4 — local UX metric: a failed turn (only when one was attempted).
    if (msgId) recordTurnFailed()
    const startedAt = get().sessions.find((x) => x.id === sid)?.messages.find((mm) => mm.id === msgId)?.startedAt
    const ttfb = ttfbFor(sid, startedAt)
    const err: ChatError = error ?? {
      layer: 'agent',
      detail: msg,
      retryable: true,
    }
    // P51.2 — stamp the failing turn's request id from the live stream id when
    // the reporter didn't supply one, so every error card names its turn.
    if (!err.requestId) {
      const live = get().liveStreamId[sid]
      if (live) err.requestId = live
    }
    set((s) => ({
      sessions: s.sessions.map((x) => {
        if (x.id !== sid) return x
        return {
          ...x,
          status: 'failed',
          messages: msgId
            ? x.messages.map((m) =>
                m.id === msgId
                  ? {
                      ...m,
                      endedAt: Date.now(),
                      error: err,
                      ...(ttfb !== undefined ? { ttfbMs: ttfb } : {}),
                    }
                  : m,
              )
            : x.messages,
        }
      }),
    }))
    retireStream(sid)
    delete activeStreamMsg[sid]
    delete activeStreamId[sid]
    delete streamFirstDelta[sid]
    delete streamT0BySession[sid]
    delete streamTokBySession[sid]
    get().clearLiveStreamId(sid)
    // P44.6 — failed turn: the frozen task scope + elevation expire.
    set({ taskSnapshot: undefined })
    // P51.5 — a failed turn also releases the queue (never silently stall).
    get().dequeueNextTurn(sid)
  },
  streamBudgetKill: (msg, sessionId?, streamId?) => {
    const sid = streamSessionId(sessionId)
    const msgId = activeStreamMsg[sid]
    if (streamId !== undefined && !bindStreamId(sid, streamId)) return
    const startedAt = get().sessions.find((x) => x.id === sid)?.messages.find((mm) => mm.id === msgId)?.startedAt
    const ttfb = ttfbFor(sid, startedAt)
    // P51.7 — partial-preserve: the text streamed before the wall stays in
    // the transcript; the card explains the kill. Budget hits are not
    // retryable (the same wall would re-trigger) — retry via the ledger.
    set((s) => ({
      sessions: s.sessions.map((x) => {
        if (x.id !== sid) return x
        return {
          ...x,
          status: 'budget_exceeded',
          messages: msgId
            ? x.messages.map((m) =>
                m.id === msgId
                  ? {
                      ...m,
                      endedAt: Date.now(),
                      error: { layer: 'budget', code: 'budget_exceeded', detail: msg, retryable: false } satisfies ChatError,
                      ...(ttfb !== undefined ? { ttfbMs: ttfb } : {}),
                    }
                  : m,
              )
            : x.messages,
        }
      }),
    }))
    retireStream(sid)
    delete activeStreamMsg[sid]
    delete activeStreamId[sid]
    delete streamFirstDelta[sid]
    delete streamT0BySession[sid]
    delete streamTokBySession[sid]
    get().clearLiveStreamId(sid)
    // P44.6 — budget kill ends the task: frozen scope + elevation expire.
    set({ taskSnapshot: undefined })
    // P51.5 — budget kill releases the queue too (head fires, may hit the
    // same wall — that is the honest loop, visible in the transcript).
    get().dequeueNextTurn(sid)
  },
  streamCancelled: (sessionId?, streamId?) => {
    const sid = streamSessionId(sessionId)
    const msgId = activeStreamMsg[sid]
    if (streamId !== undefined && !bindStreamId(sid, streamId)) return
    const startedAt = get().sessions.find((x) => x.id === sid)?.messages.find((m) => m.id === msgId)?.startedAt
    const ttfb = ttfbFor(sid, startedAt)
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id !== sid
          ? x
          : {
              ...x,
              status: 'cancelled',
              messages: msgId
                ? x.messages.map((m) =>
                    m.id === msgId
                      ? { ...m, endedAt: Date.now(), ...(ttfb !== undefined ? { ttfbMs: ttfb } : {}) }
                      : m,
                  )
                : x.messages,
            },
      ),
    }))
    retireStream(sid)
    delete activeStreamMsg[sid]
    delete activeStreamId[sid]
    delete streamFirstDelta[sid]
    delete streamT0BySession[sid]
    delete streamTokBySession[sid]
    get().clearLiveStreamId(sid)
    set({ taskSnapshot: undefined })
    get().dequeueNextTurn(sid)
  },
  streamCitations: (citations, sessionId?, streamId?) => {
    const sid = streamSessionId(sessionId)
    if (!bindStreamId(sid, streamId)) return
    if (!citations.length) return
    patchActiveAssistant(set, (m) => ({
      ...m,
      citations: [...(m.citations ?? []), ...citations],
    }), sid)
  },
  streamWalkthrough: (stops) => {
    set({
      walkthroughStops: layoutWalkthroughStops(
        (Array.isArray(stops) ? stops : []) as Array<{
          seq?: number
          path?: string
          hunk?: string
          narrative?: string
        }>,
      ),
    })
  },
  streamToolCall: (toolId, args, risk, sessionId?, streamId?) => {
    const sid = streamSessionId(sessionId)
    if (!bindStreamId(sid, streamId)) return
    if (!hasActiveStream(sid)) get().streamStart(sid, streamId)
    const now = Date.now()
    const rec: ToolCallRecord = {
      id: `tc-${now}-${toolId}`,
      toolId,
      status: 'running',
      startedAt: now,
      ...(args ? { args } : {}),
      ...(risk ? { risk } : {}),
    }
    patchActiveAssistant(set, (m) => ({
      ...m,
      toolCalls: [...(m.toolCalls ?? []), rec],
    }), sid)
  },
  streamToolResult: (toolId, result, error, sessionId?, streamId?) => {
    const sid = streamSessionId(sessionId)
    if (!bindStreamId(sid, streamId)) return
    const endedAt = Date.now()
    patchActiveAssistant(set, (m) => {
      const list = [...(m.toolCalls ?? [])]
      const idx = [...list].reverse().findIndex((t) => t.toolId === toolId && t.status === 'running')
      const real = idx === -1 ? -1 : list.length - 1 - idx
      if (real >= 0) {
        list[real] = {
          ...list[real]!,
          result,
          error,
          status: error ? 'failed' : 'done',
          ...(endedAt ? { endedAt } : {}),
        }
      } else {
        list.push({
          id: `tc-${endedAt}-${toolId}`,
          toolId,
          result,
          error,
          status: error ? 'failed' : 'done',
          startedAt: endedAt,
          endedAt,
        })
      }
      return { ...m, toolCalls: list }
    }, sid)
    // P11.6.4 — local UX metric: first successful tool result = time-to-value.
    if (!error) recordToolResult()
  },
  streamToolProgress: (toolId, progress, sessionId?, streamId?) => {
    const sid = streamSessionId(sessionId)
    if (!bindStreamId(sid, streamId)) return
    patchActiveAssistant(set, (m) => {
      const list = (m.toolCalls ?? []).map((t) =>
        t.toolId === toolId && t.status === 'running' ? { ...t, progress } : t,
      )
      return { ...m, toolCalls: list }
    }, sid)
  },
  retryToolCall: async (recordId) => {
    const st = get()
    const session = st.sessions.find((x) => x.id === st.activeSessionId)
    const rec = session?.messages.flatMap((m) => m.toolCalls ?? []).find((t) => t.id === recordId)
    if (!rec) return
    patchActiveAssistant(set, (m) => ({
      ...m,
      toolCalls: (m.toolCalls ?? []).map((t) =>
        t.id === recordId
          ? { ...t, status: 'running' as const, error: undefined, progress: 'retrying…' }
          : t,
      ),
    }))
    if (!inTauri()) {
      st.notify('Preview mode — retry needs the live executor')
      return
    }
    try {
      // P71.2c — the retry used to re-enter the coordinator's built-in tool loop
      // (`chat_tool_retry`), which is deleted with that loop (ADR-0005 §2). A
      // turn's tools belong to the bound agent now: retrying is asking **that
      // agent** again, so this refuses with the reason instead of invoking a
      // command that no longer exists. Re-establishing a one-click retry on the
      // agent channel is `P71.9c`.
      throw new Error(
        'Tool retry ran on the built-in engine, which v1 does not ship (ADR-0005). Ask the bound agent to retry instead.',
      )
    } catch (err) {
      patchActiveAssistant(set, (m) => ({
        ...m,
        toolCalls: (m.toolCalls ?? []).map((t) =>
          t.id === recordId
            ? {
                ...t,
                status: 'failed' as const,
                error: err instanceof Error ? err.message : String(err),
              }
            : t,
        ),
      }))
    }
  },
  streamStep: (label, sessionId?, streamId?) => {
    const sid = streamSessionId(sessionId)
    if (!bindStreamId(sid, streamId)) return
    set((s) => {
      const session = s.sessions.find((x) => x.id === sid)
      if (!session) return {}
      const steps: ProgressStep[] = session.messages
        .flatMap((m) => m.steps ?? [])
        .slice(-5)
      const last = steps[steps.length - 1]
      const merged = last && last.status === 'active'
        ? steps.map((p, i) => (i === steps.length - 1 ? { ...p, label, status: 'done' as const } : p))
        : [...steps, { id: `p-${Date.now()}`, label, status: 'active' as const, type: 'tool' as const }]
      return {
        sessions: s.sessions.map((x) => {
          if (x.id !== sid) return x
          const lastMsg = x.messages[x.messages.length - 1]
          if (!lastMsg || lastMsg.role !== 'assistant') return x
          return {
            ...x,
            messages: x.messages.map((m, i) =>
              i === x.messages.length - 1 ? { ...m, steps: merged } : m,
            ),
          }
        }),
      }
    })
  },
  pushMcq: (mcq, sessionId) => {
    const targetId = sessionId ?? get().activeSessionId
    set((s) => {
      const target = s.sessions.find((x) => x.id === targetId)
      if (!target) return {}
      // Skip if this card is already attached to any message
      const already = target.messages.some((m) => m.mcq?.id === mcq.id)
      if (already) return {}
      const last = target.messages[target.messages.length - 1]
      if (last && last.role === 'assistant' && !last.mcq) {
        return {
          sessions: s.sessions.map((x) =>
            x.id === targetId
              ? {
                  ...x,
                  status: 'action-required',
                  messages: x.messages.map((m, i) =>
                    i === x.messages.length - 1 ? { ...m, mcq } : m,
                  ),
                }
              : x,
          ),
        }
      }
      const standalone: ChatMessage = {
        id: `mcq-${mcq.id}`,
        role: 'assistant',
        content: '',
        timestamp: new Date().toISOString(),
        mcq,
      }
      return {
        sessions: s.sessions.map((x) =>
          x.id === targetId
            ? { ...x, status: 'action-required', messages: [...x.messages, standalone] }
            : x,
        ),
      }
    })
  },
  respondMcq: (id, choice) => {
    // P44.6 — autonomy-limit cards are resolved entirely in the UI: the
    // elevation / level change lives in the frozen task snapshot, never a
    // Guard bypass (the permission engine still evaluates every effect).
    const autonomyKind = get().sessions
      .flatMap((s) => s.messages)
      .find((m) => m.mcq?.id === id)?.mcq?.kind
    if (autonomyKind === 'autonomy') {
      get().respondAutonomyCard(id, choice)
      return
    }
    void (async () => {
      // Real shell: route by card kind — permission tickets go to Guard-2,
      // P6.3 circuit-break interrupts go to the plan executor (planRespond).
      if (!inTauri()) {
        get().notify('Preview mode — approval actions are not connected to a live executor')
        return
      }
      try {
        const kind = get().sessions
          .flatMap((s) => s.messages)
          .find((m) => m.mcq?.id === id)?.mcq?.kind
        if (kind === 'plan') {
          const pending = get().pendingPlan
          // P71.2c — the plan **executor** ran on the built-in engine's provider
          // broker, so it is deferred to post-v1 with the governed binding
          // (ADR-0005 §2, recorded in `P71.2c`). The draft stays: it is what the
          // user reads and approves, and the same task list becomes Work once an
          // executor is bound again. Approving must not imply execution, so the
          // card resolves with that stated.
          get().setPendingPlan(undefined)
          clearMcq(id)
          if (choice === 'approve' && pending) {
            get().notify(
              `Plan approved — ${pending.tasks.length} task(s). Execution needs a bound agent engine (post-v1, ADR-0005); nothing was run.`,
            )
          }
          return
        }
        if (kind === 'mcq') {
          // P71.2c — the circuit-break responder belonged to the deleted plan
          // executor. An agent's own questions arrive over ACP, not here.
          clearMcq(id)
          get().notify('Dismissed. Answers route through the bound agent (ACP) in v1.')
          return
        }
        // F1 — approval happens in the dedicated guard window, never in the
        // main renderer. Keep the card visible until that window records the
        // decision; removing it here would falsely imply approval.
        const { openGuardWindow } = await import('./guard')
        await openGuardWindow()
        get().notify('Guard-2: approval opened in the dedicated window')
      } catch (error) {
        get().notify(`Approval action failed: ${error instanceof Error ? error.message : String(error)}`, 'error')
      }
    })()

    function clearMcq(cardId: string) {
      set((s) => ({
        sessions: s.sessions.map((x) => ({
          ...x,
          status: x.status === 'action-required' ? 'running' : x.status,
          messages: x.messages
            .map((m) => (m.mcq?.id === cardId ? { ...m, mcq: undefined } : m))
            .filter((m) => m.content !== '' || m.mcq !== undefined || m.role !== 'assistant'),
        })),
      }))
    }
  },

  // One ordinary handle table; identity lives in its key and is projected by
  // getAcpHandle. There is intentionally no agent-only alias: a shared agent
  // has one independent record per application Session/binding/Work.
  acpHandles: {},
  setAcpHandle: (record) =>
    set((s) => {
      const key = acpHandleKey(
        record.applicationSessionId,
        record.bindingId,
        record.workId,
        record.agentId,
      )
      const next: Record<string, string> = {}
      for (const [existingKey, existingHandle] of Object.entries(s.acpHandles)) {
        const parsed = parseAcpHandleKey(existingKey)
        // A Session/binding/agent has one current live handle. Replace the
        // provisional launch row when the first prompt returns canonical
        // Work/Binding identity, and never leave a stale same-owner row behind.
        if (
          parsed &&
          parsed.applicationSessionId === record.applicationSessionId &&
          parsed.agentId === record.agentId &&
          (
            existingHandle === record.handle ||
            (record.bindingId === '' &&
              record.workId === '' &&
              parsed.bindingId === '' &&
              parsed.workId === '') ||
            (record.bindingId !== '' &&
              record.workId !== '' &&
              parsed.bindingId === '' &&
              parsed.workId === '')
          )
        ) {
          continue
        }
        next[existingKey] = existingHandle
      }
      next[key] = record.handle
      return { acpHandles: next }
    }),
  getAcpHandle: (sessionId, agentId, bindingId, workId) =>
    findAcpHandleRecord(get().acpHandles, sessionId, agentId, bindingId, workId),
  clearAcpHandles: (sessionId, handle) =>
    set((s) => {
      const next: Record<string, string> = {}
      for (const [key, existingHandle] of Object.entries(s.acpHandles)) {
        const parsed = parseAcpHandleKey(key)
        if (parsed?.applicationSessionId === sessionId && (handle === undefined || existingHandle === handle)) {
          continue
        }
        next[key] = existingHandle
      }
      return { acpHandles: next }
    }),
  acpConfigOptions: {},
  setAcpConfigOptions: (agentId, options) =>
    set((s) => ({ acpConfigOptions: { ...s.acpConfigOptions, [agentId]: options } })),

  closedSessions: [],

  pendingPlan: undefined,
  setPendingPlan: (p) => set({ pendingPlan: p }),

  // P51.5 — queue-while-generating (per-session FIFO). The dispatcher is
  // registered by the bridge at startup so the store can fire queued turns
  // without importing the bridge (cycle-free).
  pendingQueue: {},
  queuePaused: {},
  setTurnDispatcher: (fn) => {
    turnDispatcher = fn
  },
  queueTurn: (sessionId, text, context) => {
    const clean = text.trim()
    if (!clean) return
    set((s) => {
      const list = s.pendingQueue[sessionId] ?? []
      // Coalesce consecutive queued turns from the same composer burst?
      // No — each send is a distinct ask; keep them ordered and discrete.
      // `queueSeq` guarantees uniqueness even for same-ms burst queues (two
      // turns must never share an id — edit/remove would hit both).
      queueSeq += 1
      return {
        pendingQueue: {
          ...s.pendingQueue,
          [sessionId]: [
            ...list,
            { id: `q-${Date.now()}-${queueSeq}`, text: clean, ...(context ? { context } : {}) },
          ],
        },
      }
    })
  },
  editQueuedTurn: (sessionId, queueId, text) => {
    set((s) => {
      const list = s.pendingQueue[sessionId] ?? []
      return {
        pendingQueue: {
          ...s.pendingQueue,
          [sessionId]: list.map((q) => (q.id === queueId ? { ...q, text } : q)),
        },
      }
    })
  },
  removeQueuedTurn: (sessionId, queueId) => {
    set((s) => {
      const list = (s.pendingQueue[sessionId] ?? []).filter((q) => q.id !== queueId)
      return {
        pendingQueue: {
          ...s.pendingQueue,
          [sessionId]: list,
        },
      }
    })
  },
  setQueuePaused: (sessionId, paused) => {
    set((s) => ({ queuePaused: { ...s.queuePaused, [sessionId]: paused } }))
  },
  promoteQueuedTurn: (sessionId, queueId) => {
    set((s) => {
      const list = [...(s.pendingQueue[sessionId] ?? [])]
      const idx = list.findIndex((q) => q.id === queueId)
      if (idx <= 0) return s
      const item = list[idx]
      if (!item) return s
      list.splice(idx, 1)
      return {
        pendingQueue: {
          ...s.pendingQueue,
          [sessionId]: [item, ...list],
        },
      }
    })
  },
  dequeueNextTurn: (sessionId) => {
    if (!turnDispatcher) return
    const s = get()
    const session = s.sessions.find((x) => x.id === sessionId)
    // P52.8 — a paused queue holds its chips and never auto-fires.
    if (s.queuePaused[sessionId]) return
    // Only dispatch when this session's stream is actually idle — never
    // mid-turn (the current turn must finish first; the terminal action that
    // calls us just cleared the active-stream marker, so status is the check).
    if (!session || session.status === 'running' || session.status === 'action-required') return
    const queue = s.pendingQueue[sessionId] ?? []
    if (queue.length === 0) return
    const head = queue[0]!
    set((s2) => ({
      pendingQueue: {
        ...s2.pendingQueue,
        [sessionId]: (s2.pendingQueue[sessionId] ?? []).slice(1),
      },
    }))
    // Fire the next queued ask on the same session, never re-queued
    // (bypassQueue keeps the FIFO strictly one-in-flight).
    turnDispatcher({
      sessionId,
      text: head.text,
      ...(head.context ? { context: head.context } : {}),
      bypassQueue: true,
    })
  },

  pendingEditorWrites: {},
  parkEditorWrite: (ticketId, w) =>
    set((s) => ({ pendingEditorWrites: { ...s.pendingEditorWrites, [ticketId]: w } })),
  takeEditorWrite: (ticketId) => {
    const w = get().pendingEditorWrites[ticketId]
    if (!w) return undefined
    set((s) => {
      const next = { ...s.pendingEditorWrites }
      delete next[ticketId]
      return { pendingEditorWrites: next }
    })
    return w
  },

  verifications: [],
  pushVerification: (v) =>
    set((s) => ({ verifications: [...s.verifications.slice(-49), v] })),

  // P11.2 — onboarding. Persisted so the flow only shows on first launch.
  onboardingDone: (() => {
    if (typeof window === 'undefined') return true
    try {
      // DEC-053: legacy `everyaios.*` key honored + promoted once.
      return getLocalItem('agentcowork.settings.onboardingDone') === '1'
    } catch {
      return true
    }
  })(),
  setOnboardingDone: (v) => {
    try {
      window.localStorage.setItem('agentcowork.settings.onboardingDone', v ? '1' : '0')
    } catch {
      /* storage may be unavailable */
    }
    set({ onboardingDone: v })
  },

  // P11.5.3 — per-session layout persistence. Save on session switch/view
  // change; restore on session activation (rail/view/composer back to where
  // the user left them; new sessions stay rail-collapsed until a tool needs
  // a view — the Cursor reset bug we do not copy).
  sessionLayouts: {},
  saveSessionLayout: (sessionId, partial) => {
    set((s) => {
      const next = { ...(s.sessionLayouts[sessionId] ?? {}), ...partial }
      try {
        window.localStorage.setItem(
          `agentcowork.layout.${sessionId}`,
          JSON.stringify(next),
        )
      } catch {
        /* ignore */
      }
      return { sessionLayouts: { ...s.sessionLayouts, [sessionId]: next } }
    })
  },
  restoreSessionLayout: (sessionId) => {
    let saved: SessionLayout | undefined
    try {
      // DEC-053: legacy `everyaios.*` key honored + promoted once.
      const raw = getLocalItem(`agentcowork.layout.${sessionId}`)
      if (raw) saved = JSON.parse(raw) as SessionLayout
    } catch {
      /* ignore */
    }
    if (!saved) return
    set((s) => ({
      activeView: saved.view ?? s.activeView,
      railCollapsed: saved.railCollapsed ?? s.railCollapsed,
      composerMode: normalizeChatMode(saved.composerMode ?? s.composerMode),
      openViews: saved.openViews && saved.openViews.length > 0 ? saved.openViews : s.openViews,
      sessions: s.sessions.map((x) =>
        x.id === sessionId
          ? {
              ...x,
              view: saved.view ?? x.view,
              officeDoc: saved.officeDoc ?? x.officeDoc,
              railCollapsed: saved.railCollapsed ?? x.railCollapsed,
            }
          : x,
      ),
    }))
  },

  // P11.5.4 — per-session takeover pause. The agent loop is paused (editable
  // panels) until the user resumes with a describe-changes note.
  pausedSessions: {},
  setSessionPaused: (sessionId, paused) =>
    set((s) => ({
      pausedSessions: { ...s.pausedSessions, [sessionId]: paused },
      sessions: s.sessions.map((x) =>
        x.id === sessionId ? { ...x, status: paused ? 'paused' : 'idle' } : x,
      ),
    })),

  // P11.5.3 — real pending patches from `fs_undo_list` (diff view source).
  pendingPatches: [],
  setPendingPatches: (patches) => set({ pendingPatches: patches }),

  // P11.5.5 — NL automation draft text.
  nlAutomationDraft: undefined,
  setNlAutomationDraft: (v) => set({ nlAutomationDraft: v }),

  // P66.4 — per-session capability loadout overrides
  setSessionCapabilityOverride: (sessionId, capabilityId, enabled) =>
    set((s) => {
      const session = s.sessions.find((x) => x.id === sessionId)
      if (!session) return s
      const currentLoadout = session.capabilityLoadout ?? { overrides: {}, updatedAt: Date.now() }
      const nextLoadout: SessionCapabilityLoadout = {
        overrides: { ...currentLoadout.overrides, [capabilityId]: enabled },
        updatedAt: Date.now(),
      }
      return {
        sessions: s.sessions.map((x) =>
          x.id === sessionId ? { ...x, capabilityLoadout: nextLoadout } : x,
        ),
      }
    }),
  resetSessionCapabilities: (sessionId) =>
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === sessionId
          ? { ...x, capabilityLoadout: { overrides: {}, updatedAt: Date.now() } }
          : x,
      ),
    })),
}))

/** Vault-backed persist (Codex JSONL / Claude transcripts analog). The browser
 * preview keeps mockSessions; the shell loads via `session_list` in initBridge.
 * Persistence only engages after the shell owns the list (`sessionsHydrated`) —
 * otherwise a boot-time state write would stamp preview chats into the real vault. */
if (typeof window !== 'undefined') {
  let persistTimer: ReturnType<typeof setTimeout> | undefined
  useAppStore.subscribe((s) => {
    if (!inTauri()) return
    if (!s.sessionsHydrated) return // never persist the demo seed (bugfix 3)
    if (persistTimer) clearTimeout(persistTimer)
    persistTimer = setTimeout(() => {
      void (async () => {
        try {
          const { invoke } = await import('./tauri')
          for (const sess of s.sessions) {
            await nativeCall('session save', () => invoke('session_put', { session: sess }))
          }
        } catch (error) {
          // Persistence failure is a product state, not a recoverable no-op:
          // keep the in-memory UI intact but tell the runtime and user that
          // the latest session projection is not durable.
          setRuntimeState('degraded', `chat persistence: ${runtimeError(error)}`)
          s.notify('Chat changes could not be saved. Check the vault and retry.', 'error')
        }
      })()
    }, 400)
  })
}
