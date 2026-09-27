'use client'

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { classifyAttachment, validateImage } from '@/lib/attachments'
import {
  ArrowUp,
  FileText,
  Info,
  KeyRound,
  Mic,
  Plus,
  TerminalSquare,
  type LucideIcon,
} from 'lucide-react'
import type { PermissionMode } from '@/lib/ui-prefs'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { useAppStore, type AgentSendBlocker, type ChatMode } from '@/lib/store'
import { cn } from '@/lib/utils'
import { fuzzyRank } from '@/lib/fuzzy'
import { splitAtRefs } from '@/lib/at-refs'
import AgentModelPicker from './agent-model-picker'
import PendingQueueChips from './pending-queue-chips'
import ComposerTelemetry from './composer-telemetry'
import WebSearchControl from './web-search-control'
import { sendUserMessage } from '@/lib/bridge'
import {
  acpIdFor,
  currentBinding,
  isAgentReady,
  readinessLabel,
  type AgentReadiness,
} from '@/lib/acp'
import { PLAIN_AUTONOMY_ORDER, toPlainAutonomy } from '@/lib/plain-language'
import { inTauri } from '@/lib/tauri'
import { captureUtterance, voiceProcessUtterance } from '@/lib/voice'
import {
  WEB_SEARCH_DEFAULT,
  webSearchDirective,
  type WebSearchStatus,
} from '@/lib/search-controls'

/** v3.57 Work Mode — WHAT. Code/browser/Office/terminal are capabilities inside Build. */
const WORK_MODES: { id: ChatMode; emoji: string; label: string; hint: string }[] = [
  { id: 'auto', emoji: '🤖', label: 'Auto', hint: 'Agent chooses and may switch Plan → Build → Research as the work evolves' },
  { id: 'plan', emoji: '📐', label: 'Plan', hint: 'Analyze and propose — no mutations until you approve' },
  { id: 'build', emoji: '🔨', label: 'Build', hint: 'Execute and verify — files, browser, Office, terminal live here' },
  { id: 'research', emoji: '🔎', label: 'Research', hint: 'Investigate and cite — read-only, then you can switch to Build' },
]

const MACROS: { cmd: string; desc: string; expand: string }[] = [
  { cmd: '!deploy', desc: 'Append the prod deploy checklist instruction', expand: '(follow the production deploy checklist: verify, stage, confirm before each irreversible step)' },
  { cmd: '!pnpm', desc: 'Append "use pnpm instead of npm"', expand: '(use pnpm instead of npm for every package command)' },
  { cmd: '!lintcommit', desc: 'Append "lint before committing"', expand: '(run the linter and fix findings before committing anything)' },
  { cmd: '!deploy-checklist', desc: 'Append the deploy checklist instruction', expand: '(follow the production deploy checklist: verify, stage, confirm before each irreversible step)' },
]

const MENTIONS: { cmd: string; desc: string; icon: LucideIcon }[] = [
  { cmd: '@files', desc: 'Attach a workspace file as turn context', icon: FileText },
  { cmd: '@terminal', desc: 'Attach the last terminal command + output as turn context', icon: TerminalSquare },
]

export { splitAtRefs } from '@/lib/at-refs'
// (Re-exported for the P53.8 test path — the composer imports it from the lib.)

// ---------------------------------------------------------------------------
// The send gate — one table, one wording
// ---------------------------------------------------------------------------

export type SendGateCode =
  | 'empty'
  | 'unbound'
  | 'readiness-unknown'
  | 'not-ready'
  | 'preview'

export interface SendGate {
  /** True only when a turn can actually start. */
  ok: boolean
  code: SendGateCode
  /** One line for the status row. */
  title: string
  /** The sentence that says why, in plain words. */
  detail: string
  agentId?: string
  /** Whether the row offers the "choose an agent" action. */
  actionable: boolean
}

const GATE_OK: SendGate = {
  ok: true,
  code: 'empty',
  title: '',
  detail: '',
  actionable: false,
}

export interface SendGateInput {
  hasContent: boolean
  /** The bound agent, or `null` when the chat resolves to nothing. */
  boundAgentId: string | null
  /** The agent's display name when the live catalog knows it. */
  agentName?: string
  /** The canonical readiness from the live catalog (P71.3f). */
  readiness?: AgentReadiness
  /** A Tauri shell is attached (the turn cannot run in a browser preview). */
  live: boolean
}

/**
 * Can this composer start a turn right now?
 *
 * The chat bar is the most trusted surface in the product, so it may not look
 * sendable and then quietly refuse (or worse, queue a turn nothing will ever
 * run). The same four questions the turn path asks (`bridge.sendUserMessage` /
 * P71.2c, ADR-0005 §1) are answered here, in one table, so the bar can state
 * the reason *before* the user presses send.
 */
export function composerSendState(input: SendGateInput): SendGate {
  if (!input.hasContent) return { ...GATE_OK, code: 'empty' }
  if (!input.boundAgentId) {
    return {
      ok: false,
      code: 'unbound',
      title: 'No agent is bound to this chat',
      detail:
        'Pick an installed, ready agent first — v1 runs your message through the agent you choose, and the desktop has no built-in engine to fall back to.',
      actionable: true,
    }
  }
  if (!input.live) {
    return {
      ok: false,
      code: 'preview',
      title: 'Preview cannot run an agent',
      detail: 'This is a browser preview. Open the desktop app to send this to your agent.',
      agentId: input.boundAgentId,
      actionable: false,
    }
  }
  if (input.readiness === undefined) {
    return {
      ok: false,
      code: 'readiness-unknown',
      title: `${input.agentName ?? input.boundAgentId} is not verified as runnable`,
      detail:
        'The agent is bound, but the desktop has no readiness result for it yet. Rescan agent discovery and finish setup before sending.',
      agentId: input.boundAgentId,
      actionable: true,
    }
  }
  if (!isAgentReady(input.readiness)) {
    return {
      ok: false,
      code: 'not-ready',
      title: `${input.agentName ?? input.boundAgentId} is not ready`,
      detail: `The binding is ${readinessLabel(input.readiness)}. Finish that setup before sending — the chat stays idle until the agent can run.`,
      agentId: input.boundAgentId,
      actionable: true,
    }
  }
  return GATE_OK
}

function HintPopover({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="absolute bottom-full left-2 z-30 mb-1.5 w-64 overflow-hidden rounded-md border border-border bg-popover shadow-lg">
      <div className="border-b border-border bg-zinc-900/60 px-2 py-1 font-mono text-[10px] text-muted-foreground">
        {title}
      </div>
      <div className="scroll-thin max-h-56 overflow-y-auto py-0.5">{children}</div>
    </div>
  )
}

interface HintItem { cmd: string; desc: string; icon?: LucideIcon; color?: string }

function HintRow({ item, onSelect }: { item: HintItem; onSelect: (command: string) => void }) {
  const Icon = item.icon
  return (
    <button
      type="button"
      onClick={() => onSelect(item.cmd)}
      className="flex w-full items-center gap-2 px-2 py-1 text-left hover:bg-accent/60"
    >
      {Icon && <Icon className="h-3 w-3 text-muted-foreground" aria-hidden="true" />}
      <span className={cn('font-mono text-[11px]', item.color ?? 'text-brand')}>{item.cmd}</span>
      <span className="ml-auto truncate text-[10px] text-muted-foreground">{item.desc}</span>
    </button>
  )
}

const AUTONOMY: { id: PermissionMode; emoji: string; label: string; hint: string }[] = [
  { id: 'sandbox', emoji: '🛡', label: 'Sandbox', hint: 'Plan + read-only. Every mutation is denied.' },
  { id: 'ask', emoji: '👀', label: 'Ask', hint: 'Default. Safe reads auto-allow; mutations show a Guard-2 card.' },
  { id: 'auto', emoji: '⚡', label: 'Auto', hint: 'Low-risk workspace writes auto-allow. Destructive / money / secrets still ask.' },
  { id: 'full', emoji: '🚀', label: 'Maximum', hint: 'Maximum autonomy within hard floors — never skips destructive/secret/financial denies.' },
]

function WorkModeChip({ compact }: { compact?: boolean }) {
  const mode = useAppStore((s) => s.composerMode)
  const setMode = useAppStore((s) => s.setComposerMode)
  return (
    <select
      aria-label="Work mode"
      value={mode}
      onChange={(e) => setMode(e.target.value as ChatMode)}
      title={WORK_MODES.find((m) => m.id === mode)?.hint ?? 'Work mode (WHAT)'}
      className={cn(
        'rounded-md border border-border bg-background/40 font-mono text-[10px] text-foreground',
        compact ? 'h-6 max-w-[7.5rem] px-1' : 'h-6 px-1.5',
      )}
    >
      {WORK_MODES.map((m) => (
        <option key={m.id} value={m.id} title={m.hint}>
          {m.emoji} {m.label}
        </option>
      ))}
    </select>
  )
}

function AutonomyChip({ compact }: { compact?: boolean }) {
  const mode = useAppStore((s) => s.permissionMode)
  const setMode = useAppStore((s) => s.setPermissionMode)
  const notify = useAppStore((s) => s.notify)
  return (
    <select
      aria-label="Autonomy"
      value={mode}
      onChange={(e) => {
        const next = e.target.value as PermissionMode
        setMode(next)
        if (next === 'full') {
          notify('Maximum still honors hard floors — destructive, secrets, money, and Guard-2 R4 never auto-run')
        }
      }}
      title={AUTONOMY.find((m) => m.id === mode)?.hint ?? 'Autonomy (HOW MUCH)'}
      className={cn(
        'rounded-md border border-border bg-background/40 font-mono text-[10px] text-foreground',
        compact ? 'h-6 max-w-[7.5rem] px-1' : 'h-6 px-1.5',
      )}
    >
      {AUTONOMY.map((m) => (
        <option key={m.id} value={m.id} title={m.hint}>
          {m.emoji} {m.label}
        </option>
      ))}
    </select>
  )
}

/**
 * P32.9 / WP1 — the casual composer asks one question, in plain words.
 *
 * Casual mode had three controls stacked before the user typed anything
 * (Agent ▾ + Work Mode ▾ + Autonomy ▾) with labels like "Sandbox" and
 * "Maximum". This is the single dial the research calls for. It is a display
 * layer over the existing four-value `PermissionMode` — no store, wire or
 * guard change — so the per-task permission freeze and the Rust preset sync
 * are unaffected.
 */
function SimpleAutonomyDial() {
  const mode = useAppStore((s) => s.permissionMode)
  const setMode = useAppStore((s) => s.setPermissionMode)
  const notify = useAppStore((s) => s.notify)
  const plain = toPlainAutonomy(mode)
  return (
    <>
      <select
        aria-label="How much can I do on my own?"
        value={mode}
        onChange={(e) => {
          const next = e.target.value as PermissionMode
          setMode(next)
          if (next === 'full') {
            notify('“Just do it” still stops for deletes, payments, secrets and Guard-2 — those always ask')
          }
        }}
        title={plain.hint || 'How much can I do on my own?'}
        className="h-6 shrink-0 rounded-md border border-border bg-background/40 px-1.5 font-mono text-[10px] text-foreground"
      >
        {PLAIN_AUTONOMY_ORDER.map((id) => {
          const p = toPlainAutonomy(id)
          return (
            <option key={id} value={id} title={p.hint}>
              {p.emoji} {p.label}
            </option>
          )
        })}
      </select>
      {/* The dial's meaning, in one sentence — never a bare label. */}
      <span className="min-w-0 flex-1 truncate pl-1 text-[10px] text-muted-foreground">
        {plain.hint}
      </span>
    </>
  )
}

function IconBtn({ icon: Icon, label, onClick, hidden, active, disabled, title }: {
  icon: LucideIcon
  label: string
  onClick?: () => void
  hidden?: boolean
  active?: boolean
  disabled?: boolean
  title?: string
}) {
  return (
    <Button
      size="icon"
      variant="ghost"
      className={cn(
        'h-7 w-7 text-muted-foreground hover:text-foreground',
        active && 'bg-brand/15 text-brand',
        hidden && 'hidden sm:inline-flex',
        disabled && 'cursor-not-allowed opacity-40 hover:text-muted-foreground'
      )}
      onClick={disabled ? undefined : onClick}
      disabled={disabled}
      title={title ?? label}
      // Every icon-only control names itself: the title is a tooltip, the
      // accessible name is what a screen reader reads out.
      aria-label={label}
      aria-disabled={disabled}
    >
      <Icon className="h-3.5 w-3.5" aria-hidden="true" />
    </Button>
  )
}

/**
 * The composer's status row.
 *
 * One fixed height, always present, so a state change never moves the bar
 * (CLS = 0). Its content is the single most important truth about the bar:
 * why a message cannot be sent, what a running turn is doing with the queue,
 * and — when a turn is in flight — which autonomy level is actually frozen
 * into it (P44.6: a live bar change never mutates a running Work).
 */
function StatusRow({
  gate,
  agentBusy,
  queuedCount,
  frozenAutonomy,
  liveAutonomy,
  onFix,
}: {
  gate: SendGate
  agentBusy: boolean
  queuedCount: number
  frozenAutonomy?: PermissionMode
  liveAutonomy: PermissionMode
  onFix: () => void
}) {
  const frozenDiffers = frozenAutonomy !== undefined && frozenAutonomy !== liveAutonomy
  return (
    <div
      id="composer-status"
      role="status"
      aria-live="polite"
      className="flex h-6 items-center gap-1.5 overflow-hidden border-t border-border/70 px-2 font-mono text-[9px] text-muted-foreground"
    >
      {!gate.ok ? (
        <>
          <Info className="h-3 w-3 shrink-0 text-warning" aria-hidden="true" />
          <span className="shrink-0 font-medium text-warning">{gate.title}</span>
          <span className="hidden min-w-0 truncate sm:inline">· {gate.detail}</span>
          {gate.actionable && (
            <button
              type="button"
              onClick={onFix}
              className="ml-auto flex shrink-0 items-center gap-1 rounded px-1 py-0.5 text-[9px] text-muted-foreground hover:bg-accent hover:text-foreground"
            >
              <KeyRound className="h-2.5 w-2.5" aria-hidden="true" />
              Choose an agent
            </button>
          )}
        </>
      ) : agentBusy ? (
        <>
          <Info className="h-3 w-3 shrink-0 text-brand" aria-hidden="true" />
          <span className="min-w-0 truncate">
            Working — sends are queued and start when this turn finishes
            {queuedCount > 0 ? ` (${queuedCount} waiting)` : ''}
            {frozenDiffers
              ? ` · this turn runs at “${toPlainAutonomy(frozenAutonomy!).label}”`
              : ''}
          </span>
        </>
      ) : (
        <>
          <span className="hidden sm:inline">Enter send · Shift+Enter newline · Esc clear</span>
          <span className="hidden sm:inline">·</span>
          <span className="hidden min-w-0 truncate sm:inline">
            Tab completes · @ mention · / command · ! macro
          </span>
        </>
      )}
    </div>
  )
}

interface Props {
  /**
   * The shell's live usage figures. Only `tokens` is painted, and only as a
   * measurement: `spent` and `cap` arrive without provenance (the bridge seeds
   * `cap` with a constant, and `spent` is our own arithmetic over configured
   * prices), so the bar reads the cost through `spend.costReadout` instead —
   * which separates *reported* from *estimated* from *unknown*. A `$0.00` here
   * used to mean "no report", not "no spend".
   */
  budget?: { spent: number; cap: number; tokens: number }
  /** Center-lift the composer on the empty/new-chat state; bottom-pin once chat starts */
  centered?: boolean
}

export default function ChatComposer({ budget, centered }: Props) {
  const composerValue = useAppStore((s) => s.composerValue)
  const setComposerValue = useAppStore((s) => s.setComposerValue)
  const notify = useAppStore((s) => s.notify)
  const [listening, setListening] = useState(false)
  const activeSession = useAppStore((s) =>
    s.sessions.find((x) => x.id === s.activeSessionId)
  )
  // P51.5 — while the agent is generating, the composer stays enabled and a
  // send lands in the pending queue instead of being dropped or disabled.
  const agentBusy =
    activeSession?.status === 'running' ||
    activeSession?.status === 'action-required'
  const queuedCount = useAppStore(
    (s) => (s.pendingQueue[s.activeSessionId] ?? []).length,
  )
  // WP1 — casual mode collapses the three-control contract to one plain dial.
  // Power mode keeps Agent · Work Mode · Autonomy exactly as they were.
  const powerMode = useAppStore((s) => s.powerMode)
  // P44.6 — the autonomy actually frozen into the running Work. A live change
  // in the bar applies to the next message, never to the turn in flight.
  const frozenAutonomy = useAppStore((s) =>
    agentBusy ? s.taskSnapshot?.autonomyLevel : undefined,
  )
  const liveAutonomy = useAppStore((s) => s.permissionMode)
  const openSetup = useAppStore((s) => s.openSetup)
  const setCenterScreen = useAppStore((s) => s.setCenterScreen)
  const setSettingsSection = useAppStore((s) => s.setSettingsSection)

  // The one bound agent. `selectedAgentId` is what the picker sets; a chat pin
  // and the user default are how that choice is carried, and `currentBinding`
  // is the single predicate that refuses the retired built-in spellings — the
  // same resolution the turn path uses, so the bar and the turn can never
  // disagree about who is answering.
  const boundAgentId = useAppStore((s) => {
    const sid = s.activeSessionId
    return (
      currentBinding(s.sessionChiefs[sid]) ??
      currentBinding(s.userDefaultChief) ??
      currentBinding(s.selectedAgentId)
    )
  })
  const boundAgentName = useAppStore((s) => {
    const id = boundAgentId
    if (!id) return undefined
    const acpId = acpIdFor(id)
    return s.liveAgents.find((a) => acpIdFor(a.id) === acpId)?.name ?? id
  })
  const boundReadiness = useAppStore((s) => {
    const id = boundAgentId
    if (!id) return undefined
    const acpId = acpIdFor(id)
    return s.liveAgents.find((a) => acpIdFor(a.id) === acpId)?.readiness
  })

  const [attachment, setAttachment] = useState<{ title: string; content: string } | null>(null)
  const fileRef = useRef<HTMLInputElement>(null)

  // Web search is OFF by default and is a directive to the bound agent, not a
  // desktop-side search (the agent owns that tool — ADR-0005).
  const [webSearch, setWebSearch] = useState<boolean>(WEB_SEARCH_DEFAULT)
  const [webPanel, setWebPanel] = useState(false)
  const [webStatus, setWebStatus] = useState<WebSearchStatus | null>(null)

  /** Turning the switch on reveals the cascade it will use; turning it off
   *  closes the panel. The footer's reserved chip can reopen it either way. */
  const toggleWebSearch = useCallback((next: boolean) => {
    setWebSearch(next)
    setWebPanel(next)
  }, [])

  const hasContent = composerValue.trim().length > 0 || attachment !== null
  const gate = composerSendState({
    hasContent,
    boundAgentId,
    agentName: boundAgentName ?? undefined,
    readiness: boundReadiness as AgentReadiness | undefined,
    live: inTauri(),
  })
  const canSend = hasContent && gate.ok

  // Keep the footer's search slot truthful without polling: the control owns
  // the read, and reports the status back for the one-line footer. The callback
  // is stable so the control can publish on change without re-rendering the bar.
  const onWebStatus = useCallback((next: WebSearchStatus | null) => setWebStatus(next), [])

  const hint = useMemo(() => {
    const v = composerValue.trimStart()
    if (!v) return null
    if (v.startsWith('/')) return { kind: 'slash' as const, q: v.slice(1) }
    if (v.startsWith('!')) return { kind: 'macro' as const, q: v.slice(1) }
    if (v.startsWith('@')) return { kind: 'mention' as const, q: v.slice(1) }
    return null
  }, [composerValue])

  // P52.11 — fuzzy subsequence match (typo-tolerant) instead of strict
  // substring: '/mdoe' still surfaces '/mode', '@fl' finds '@files'.
  // P53.1/53.2 — while an agent is bound, the local AgentCowork slash table is
  // hidden: the agent's live vocabulary (from the most recent
  // `available_commands_update`) is the only `/` source, and its items submit
  // as `session/prompt` text (never a local intercept).
  const [liveSlash, setLiveSlash] = useState<{ name: string; description: string }[]>([])
  useEffect(() => {
    // P71.9c — only the bound agent's own command vocabulary is fetched; there
    // is no local table to fall back to (every binding is an external agent).
    if (!boundAgentId) {
      setLiveSlash([])
      return
    }
    let alive = true
    void (async () => {
      try {
        const st = useAppStore.getState()
        const handle = st.acpHandles[st.selectedAgentId] ?? st.acpHandles[boundAgentId]
        if (!handle) return
        const { acpSessionCommands } = await import('@/lib/acp')
        const rows = await acpSessionCommands(handle)
        if (alive) setLiveSlash(rows.map((r) => ({ name: r.name, description: r.description })))
      } catch {
        if (alive) setLiveSlash([])
      }
    })()
    return () => {
      alive = false
    }
  }, [boundAgentId, composerValue === '' ? 'empty' : 'typing'])
  const hintList: { title: string; items: HintItem[] } | null = (() => {
    if (!hint) return null
    const q = hint.q
    if (hint.kind === 'slash') {
      if (boundAgentId) {
        // P53.2 — no local intercept: show only the agent's live commands.
        const live = liveSlash.map((c) => ({ cmd: `/${c.name}`, desc: c.description }))
        if (live.length === 0) return null
        return {
          title: boundAgentName ? `/${boundAgentName} commands (live)` : 'Agent commands (live)',
          items: fuzzyRank(q, live, (c) => c.cmd).map((c) => ({ ...c, color: 'text-emerald-300' })),
        }
      }
      return null
    }
    if (hint.kind === 'macro')
      return {
        title: 'Macros',
        items: fuzzyRank(q, MACROS, (c) => c.cmd)
          .map((c) => ({ ...c, color: 'text-brand' })),
      }
    return {
      title: 'Mention',
      items: fuzzyRank(q, MENTIONS, (c) => c.cmd).map((c) => ({ ...c, color: 'text-sky-300' })),
    }
  })()

  const pickFile = () => fileRef.current?.click()

  const onFileChosen = (file: File | undefined) => {
    if (!file) return
    // P52.12 — classify before reading: images must NOT be readAsText'd into
    // the text seam (that would attach binary garbage as a “user document”).
    const cls = classifyAttachment(file.name, file.type)
    if (cls === 'image') {
      const gate = validateImage(file)
      if (gate.ok) {
        notify(
          `“${file.name}” is a valid attach-candidate image (PNG/JPEG/GIF/WebP ≤ 20 MiB) but image message parts are wire-gated — only text/SVG attach today (P52.12). Nothing was attached.`,
          'error',
        )
      } else {
        notify(gate.reason, 'error')
      }
      return
    }
    if (cls === 'unsupported') {
      notify(`“${file.name}” — binary files can't attach; only text, Markdown, CSV and SVG ride the context seam`, 'error')
      return
    }
    if (file.size > 512 * 1024) {
      notify(`“${file.name}” is ${(file.size / 1024).toFixed(0)} KB — attachments cap at 512 KB of text`, 'error')
      return
    }
    const reader = new FileReader()
    reader.onerror = () => notify(`Could not read “${file.name}” as text`, 'error')
    reader.onload = () => {
      const text = typeof reader.result === 'string' ? reader.result : ''
      if (!text.trim()) {
        notify(`“${file.name}” has no readable text — binary files can't attach`, 'error')
        return
      }
      setAttachment({ title: file.name, content: text.slice(0, 200_000) })
    }
    reader.readAsText(file)
  }

  const withDirective = (text: string) => {
    const directive = webSearchDirective(webSearch)
    if (!directive) return text
    return `${text.trimEnd()}\n\n${directive}`
  }

  const send = () => {
    if (!hasContent) return
    const st = useAppStore.getState()
    // The bar refuses *before* anything moves: no transcript row, no queue
    // entry, no cleared draft. The reason is already on screen; the toast is
    // only there for the Enter-key user who never looked.
    if (!gate.ok) {
      // The same four reasons the turn path records, written once. The bar
      // refuses before anything moves, so `bridge` is never asked to dispatch a
      // turn that cannot start.
      const code: AgentSendBlocker['code'] =
        gate.code === 'unbound'
          ? 'unbound'
          : gate.code === 'not-ready'
            ? 'not-ready'
            : gate.code === 'preview'
              ? 'preview'
              : 'readiness-unknown'
      st.setAgentSendBlocker({
        sessionId: st.activeSessionId,
        code,
        title: gate.title,
        detail: gate.detail,
        ...(gate.agentId ? { agentId: gate.agentId } : {}),
      })
      if (gate.actionable) st.openSetup()
      notify(gate.detail, 'error')
      return
    }
    if (st.centerScreen === 'home') {
      const cur = st.sessions.find((x) => x.id === st.activeSessionId)
      if (cur && cur.messages.length > 0) st.newSession()
      st.setCenterScreen('chat')
    }
    // P51.5 — when already generating, the send key queues the ask instead of
    // silently dropping it (the queue shows as pending chips and auto-fires).
    const busy = st.sessions.some(
      (s) =>
        s.id === st.activeSessionId &&
        (s.status === 'running' || s.status === 'action-required'),
    )
    if (busy) {
      const text = composerValue.trim()
      if (text) {
        st.queueTurn(st.activeSessionId, withDirective(text), attachment ?? undefined)
        setComposerValue('')
        setAttachment(null)
        notify('Queued — starts when the current turn finishes')
      }
      return
    }
    let text = composerValue
    // Macros expand to prompt augmentations (visible in the sent text).
    const first = text.trimStart().split(/\s+/, 1)[0]
    const macro = MACROS.find((m) => m.cmd === first)
    if (macro) text = `${text} ${macro.expand}`
    if (!text.trim() && !attachment) return
    // P68 — `@terminal`: Copilot-style follow. The shell's own trusted record
    // (command, cwd, exit code, output) rides as the turn's attachment. When
    // the shell has reported nothing trusted, the user is told instead of a
    // fabricated block being sent.
    if (/^@terminal(?=\s|$)/.test(text.trimStart())) {
      void (async () => {
        try {
          const { terminalStatus, terminalLastCommandContext } = await import('@/lib/terminal')
          const { ptys } = await terminalStatus()
          // Prefer a live human tab; fall back to the newest live session.
          const live = ptys.filter((p) => p.running)
          const target = live.find((p) => p.origin === 'human') ?? live[0]
          if (!target) {
            notify('No live terminal — open one in the Terminal view first', 'error')
            return
          }
          const block = await terminalLastCommandContext(target.ptyId)
          if (!block) {
            notify(
              'The terminal has no trusted command record yet (shell integration reports it after a command finishes)',
              'error',
            )
            return
          }
          const rest = text.replace(/^@terminal/, '').trim()
          setComposerValue('')
          setAttachment(null)
          await sendUserMessage(withDirective(rest || 'Explain this terminal result'), {
            title: `terminal · ${target.profileId}`,
            content: block,
          })
        } catch (e) {
          notify(e instanceof Error ? e.message : 'Could not read terminal context', 'error')
        }
      })()
      return
    }
    const { clean, refs } = splitAtRefs(text)
    const refSuffix = refs.length > 0 ? `\n\n[refs: ${refs.map((r) => `@${r}`).join(' ')}]` : ''
    const sendText = withDirective((clean.trim() ? clean : text) + refSuffix)
    const ctx = attachment
    setComposerValue('')
    setAttachment(null)
    void sendUserMessage(sendText, ctx ? { title: ctx.title, content: ctx.content } : undefined)
  }

  const openSearchSettings = () => {
    setSettingsSection('search')
    setCenterScreen('settings')
  }

  return (
    <div
      className={cn(
        'relative',
        centered
          ? 'rounded-xl border border-border bg-card shadow-lg'
          : 'border-t border-border bg-card/80',
      )}
    >
      {hintList && hintList.items.length > 0 && (
        <HintPopover title={hintList.title}>
          {hintList.items.map((c) => (
            <HintRow
              key={c.cmd}
              item={c}
              onSelect={(command) => {
                if (command === '@files') {
                  setComposerValue('')
                  pickFile()
                  return
                }
                setComposerValue(`${command} `)
              }}
            />
          ))}
        </HintPopover>
      )}

      {/* P51.5 — pending asks above the composer while the agent is busy.
          Mounted only when there is something to show: the chip list renders
          nothing for an empty queue, and leaving it mounted would subscribe to
          a store selector that builds a fresh array per read. */}
      {queuedCount > 0 && <PendingQueueChips />}

      {/* The chat bar is the field. Controls live in a one-line footer, not a stack above. */}
      {attachment && (
        <div className="mx-2 mt-2 flex items-center gap-1.5 rounded-md border border-brand/30 bg-brand/5 px-2 py-1 font-mono text-[10px] text-brand">
          <FileText className="h-3 w-3 shrink-0" aria-hidden="true" />
          <span className="min-w-0 flex-1 truncate">{attachment.title} · {(attachment.content.length / 1024).toFixed(1)} KB attached</span>
          <button
            type="button"
            onClick={() => setAttachment(null)}
            className="shrink-0 rounded px-1 text-muted-foreground hover:text-foreground"
            title="Remove attachment"
            aria-label={`Remove attachment ${attachment.title}`}
          >
            <span aria-hidden="true">✕</span>
          </button>
        </div>
      )}
      <div className="flex flex-nowrap items-center gap-1 px-2 pt-2 pb-1">
        <input
          ref={fileRef}
          type="file"
          className="hidden"
          aria-label="Attach a text file (images are wire-gated — see P52.12)"
          onChange={(e) => {
            onFileChosen(e.target.files?.[0])
            e.target.value = ''
          }}
        />
        <IconBtn icon={Plus} label="Attach file" onClick={pickFile} />
        <WebSearchControl
          enabled={webSearch}
          open={webPanel}
          onOpenChange={setWebPanel}
          onToggle={toggleWebSearch}
          onOpenSettings={openSearchSettings}
          onStatus={onWebStatus}
        />
        {/* WP1 — the agent/model picker is a power-mode control. Casual keeps
            the identity visible in the status bar, which always names the
            runtime and model currently answering. */}
        {powerMode && (
          <div className="shrink-0">
            <AgentModelPicker />
          </div>
        )}
        <Textarea
          value={composerValue}
          onChange={(e) => setComposerValue(e.target.value)}
          onKeyDown={(e) => {
            // P52.14 — CJK/IME composition guard: while an IME is composing,
            // Enter confirms the candidate (not the message). Browsers set
            // isComposing (or keyCode 229); sending mid-composition would
            // fire on a half-typed sentence.
            const composing = e.nativeEvent?.isComposing || (e as unknown as { keyCode?: number }).keyCode === 229
            if (e.key === 'Enter' && !e.shiftKey) {
              if (composing) return
              e.preventDefault()
              send()
            }
            // P52.14 — Tab accepts the top hint instead of leaving focus
            // (completion is the hint's job; the user is still typing).
            if (e.key === 'Tab' && !e.shiftKey && hintList && hintList.items.length > 0) {
              e.preventDefault()
              const first = hintList.items[0]
              if (first) {
                if (first.cmd === '@files') {
                  setComposerValue('')
                  pickFile()
                } else {
                  setComposerValue(`${first.cmd} `)
                }
              }
              return
            }
            if (e.key === 'Escape') {
              e.stopPropagation()
              setComposerValue('')
            }
          }}
          placeholder={
            !gate.ok && hasContent
              ? 'Bind a ready agent to send this…'
              : agentBusy
                ? queuedCount > 0
                  ? `Next queued ask will follow… (${queuedCount} pending)`
                  : 'Still working — type to queue your next ask…'
                : 'Tell AgentCowork what you need…'
          }
          className="max-h-28 min-h-[36px] min-w-0 flex-1 resize-none border-0 bg-transparent px-1 py-1.5 text-[13px] leading-relaxed shadow-none focus-visible:ring-0"
          rows={1}
        />
        <div className="flex shrink-0 items-center gap-0.5 pb-0.5">
          {/* P50.4.3 — mic captures PCM and runs the crate VAD/STT pipeline.
              No STT engine is installed, so a capture reports the gap instead
              of inventing a transcript. */}
          <IconBtn
            icon={Mic}
            label={listening ? 'Listening…' : 'Voice input'}
            title={
              inTauri()
                ? listening
                  ? 'Listening — tap again after you finish speaking'
                  : 'Capture a voice utterance (VAD is live; STT engine not installed — no invented transcript)'
                : 'Voice capture needs the Tauri shell'
            }
            disabled={!inTauri() || listening}
            active={listening}
            onClick={() => {
              void (async () => {
                setListening(true)
                try {
                  const samples = await captureUtterance(2500)
                  const ev = await voiceProcessUtterance(samples, 2500, false)
                  if (ev.transcribed && ev.text.trim()) {
                    const cur = useAppStore.getState().composerValue
                    setComposerValue(cur ? `${cur} ${ev.text}` : ev.text)
                  } else {
                    notify(
                      'Heard the mic — no on-device STT engine is installed, so no transcript was invented.',
                    )
                  }
                } catch (e) {
                  notify(e instanceof Error ? e.message : 'Microphone capture failed', 'error')
                } finally {
                  setListening(false)
                }
              })()
            }}
          />
          <Button
            size="icon"
            className={cn(
              'h-8 w-8 shrink-0 rounded-md text-white transition-colors',
              agentBusy ? 'bg-emerald-500 hover:bg-emerald-600' : 'bg-brand hover:bg-brand-hover',
              'disabled:opacity-40',
            )}
            disabled={!canSend}
            onClick={send}
            aria-label={
              !gate.ok && hasContent
                ? `Cannot send — ${gate.title}`
                : agentBusy
                  ? 'Queue this ask (it runs after the current turn)'
                  : 'Send'
            }
            aria-describedby={!gate.ok && hasContent ? 'composer-status' : undefined}
            title={
              !gate.ok && hasContent
                ? gate.detail
                : agentBusy
                  ? 'Queue this ask (runs after the current turn)'
                  : 'Send'
            }
          >
            <ArrowUp className="h-4 w-4" aria-hidden="true" />
          </Button>
        </div>
      </div>

      <div className="flex h-8 items-center gap-1 border-t border-border/70 px-2">
        {powerMode ? (
          <>
            <WorkModeChip compact />
            <AutonomyChip compact />
          </>
        ) : (
          <SimpleAutonomyDial />
        )}
        {/* Reserved slot — always laid out, so turning web search on or off
            never moves the readouts (CLS = 0). It is also the way back into
            the details once the switch is on. */}
        <button
          type="button"
          aria-haspopup="dialog"
          aria-expanded={webPanel}
          aria-label={
            webStatus
              ? `Web search is on — ${webStatus.head}. Show the backends.`
              : 'Turn on web search and show its backends'
          }
          onClick={() => (webSearch ? setWebPanel((v) => !v) : toggleWebSearch(true))}
          data-telemetry="web-search"
          className="hidden h-6 w-[6.5rem] shrink-0 items-center justify-end gap-1 truncate rounded px-0.5 font-mono text-[10px] text-muted-foreground hover:bg-accent/60 hover:text-foreground md:flex"
          title={
            webStatus
              ? `${webStatus.head} — ${webStatus.detail}`
              : 'Web search is off. The agent searches with its own tool when you ask it to.'
          }
        >
          <span
            className={cn(
              'h-1.5 w-1.5 shrink-0 rounded-full',
              !webSearch
                ? 'bg-muted-foreground/40'
                : webStatus?.tone === 'ok'
                  ? 'bg-emerald-400'
                  : webStatus?.tone === 'warn'
                    ? 'bg-warning'
                    : 'bg-muted-foreground',
            )}
            aria-hidden="true"
          />
          <span className="truncate">
            {webStatus
              ? `web ${webStatus.inCascade > 0 ? `${webStatus.inCascade} backend${webStatus.inCascade === 1 ? '' : 's'}` : 'not configured'}`
              : 'web off'}
          </span>
        </button>
        <ComposerTelemetry tokens={budget?.tokens ?? null} />
      </div>

      <StatusRow
        gate={gate}
        agentBusy={agentBusy}
        queuedCount={queuedCount}
        frozenAutonomy={frozenAutonomy}
        liveAutonomy={liveAutonomy}
        onFix={() => openSetup()}
      />
    </div>
  )
}
