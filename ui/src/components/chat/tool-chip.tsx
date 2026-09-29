'use client'

import { memo, useEffect, useMemo, useState } from 'react'
import { Check, CheckCircle2, ChevronDown, Copy, Database, Loader2, RotateCw, ShieldAlert, Wrench, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { ToolCallRecord } from '@/lib/store'
import { parseToolResult } from '@/lib/tool-json'
import { useAppStore } from '@/lib/store'

/** The short, user-facing state of a tool call. */
export type ToolActivityStatus = 'working' | 'completed' | 'failed'

export interface ToolActivitySummary {
  sentence: string
  status: string
  duration: string
}

const TOOL_ACTIONS: Record<
  string,
  { ongoing: string; complete: string; failed: string; subject: string }
> = {
  read: {
    subject: 'your files',
    ongoing: 'Reading your files',
    complete: 'Finished reading your files',
    failed: 'I could not finish reading your files',
  },
  list: {
    subject: 'the available items',
    ongoing: 'Checking the available items',
    complete: 'Finished checking the available items',
    failed: 'I could not check the available items',
  },
  open: {
    subject: 'the requested resource',
    ongoing: 'Opening the requested resource',
    complete: 'Opened the requested resource',
    failed: 'I could not open the requested resource',
  },
  search: {
    subject: 'your files',
    ongoing: 'Searching your files',
    complete: 'Finished searching your files',
    failed: 'I could not search your files',
  },
  write: {
    subject: 'the requested resource',
    ongoing: 'Updating the requested resource',
    complete: 'Finished updating the requested resource',
    failed: 'I could not update the requested resource',
  },
  edit: {
    subject: 'the requested resource',
    ongoing: 'Updating the requested resource',
    complete: 'Finished updating the requested resource',
    failed: 'I could not update the requested resource',
  },
  update: {
    subject: 'the requested resource',
    ongoing: 'Updating the requested resource',
    complete: 'Finished updating the requested resource',
    failed: 'I could not update the requested resource',
  },
  delete: {
    subject: 'the requested resource',
    ongoing: 'Removing the requested resource',
    complete: 'Finished removing the requested resource',
    failed: 'I could not remove the requested resource',
  },
  remove: {
    subject: 'the requested resource',
    ongoing: 'Removing the requested resource',
    complete: 'Finished removing the requested resource',
    failed: 'I could not remove the requested resource',
  },
  navigate: {
    subject: 'the requested page',
    ongoing: 'Opening the requested page',
    complete: 'Opened the requested page',
    failed: 'I could not open the requested page',
  },
  click: {
    subject: 'the requested page',
    ongoing: 'Interacting with the requested page',
    complete: 'Finished interacting with the requested page',
    failed: 'I could not interact with the requested page',
  },
  type: {
    subject: 'the requested information',
    ongoing: 'Entering the requested information',
    complete: 'Finished entering the requested information',
    failed: 'I could not enter the requested information',
  },
  execute: {
    subject: 'the requested task',
    ongoing: 'Running the requested task',
    complete: 'Finished running the requested task',
    failed: 'I could not run the requested task',
  },
  run: {
    subject: 'the requested task',
    ongoing: 'Running the requested task',
    complete: 'Finished running the requested task',
    failed: 'I could not run the requested task',
  },
  connect: {
    subject: 'the requested connection',
    ongoing: 'Connecting the requested service',
    complete: 'Connected the requested service',
    failed: 'I could not connect the requested service',
  },
  query: {
    subject: 'the connected information',
    ongoing: 'Checking connected information',
    complete: 'Finished checking connected information',
    failed: 'I could not check connected information',
  },
}

function actionForTool(toolId: string): (typeof TOOL_ACTIONS)[string] {
  const normalized = toolId.toLowerCase().replace(/[^a-z0-9]+/g, ' ')
  const words = normalized.split(/\s+/).filter(Boolean)
  // Prefer the verb anywhere in the id, but never echo the id itself. Unknown
  // tools get a neutral sentence rather than leaking an implementation name.
  for (const word of words) {
    const action = TOOL_ACTIONS[word]
    if (action) return action
  }
  return {
    subject: 'the requested task',
    ongoing: 'Working on the requested task',
    complete: 'Finished the requested task',
    failed: 'I could not finish the requested task',
  }
}

/** Format a call duration without hiding short calls. */
export function formatToolDuration(start?: number, end?: number, now = Date.now()): string {
  if (start === undefined) return ''
  const base = end !== undefined && end >= start ? end : now
  const ms = Math.max(0, base - start)
  if (ms < 1000) return '<1s'
  const seconds = Math.round(ms / 1000)
  return seconds >= 60
    ? `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, '0')}s`
    : `${seconds}s`
}

/** A sentence-first projection of a raw tool record for the casual surface. */
export function toolActivitySummary(
  rec: Pick<ToolCallRecord, 'toolId' | 'status' | 'startedAt' | 'endedAt'>,
  now = Date.now(),
): ToolActivitySummary {
  const action = actionForTool(rec.toolId)
  const status: ToolActivityStatus =
    rec.status === 'running' ? 'working' : rec.status === 'failed' ? 'failed' : 'completed'
  const sentence =
    status === 'working'
      ? `${action.ongoing}…`
      : status === 'failed'
        ? `${action.failed}.`
        : `${action.complete}.`
  return {
    sentence,
    status: status === 'working' ? 'Working' : status === 'failed' ? 'Needs attention' : 'Done',
    duration: formatToolDuration(rec.startedAt, rec.endedAt, now),
  }
}

function riskTone(risk?: string): string {
  const r = (risk ?? '').toLowerCase()
  if (r === 'high' || r === 'destructive') return 'border-rose-500/40 bg-rose-500/10 text-rose-300'
  if (r === 'medium' || r === 'external-write') return 'border-warning/40 bg-warning/10 text-warning'
  if (r === 'low' || r === 'read') return 'border-emerald-500/40 bg-emerald-500/10 text-emerald-300'
  return 'border-border bg-background/40 text-muted-foreground'
}

function safeJson(value: unknown): string {
  if (value == null) return ''
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2) ?? String(value)
  } catch {
    return String(value)
  }
}

// eslint-disable-next-line no-control-regex
const ANSI_CSI_RE = /[\u001b\u009b][[()#;?]*(?:\d{1,4}(?:;\d{0,4})*)?[0-9A-ORZcf-nqry=><]/g
const ANSI_OSC_RE = /\u001b\][^\x07\u001b]*(?:\x07|\u001b\\)/g

/** P64.11 — strip terminal escape sequences (SGR colors, cursor moves, OSC
 * hyperlinks) from a raw CLI stream. */
export function stripAnsi(input: string): string {
  return input.replace(ANSI_OSC_RE, '').replace(ANSI_CSI_RE, '')
}

/** P64.11 — CLI stream normalization at the drawer boundary. The render-side
 * twin of the `UIEventEnvelope` normalization in `agentcowork-acp`: collapse
 * `\r` spinner/progress frames to their final frame, drop ANSI churn, and
 * trim blank-line runs — so raw external CLI stdout/stderr never spills past
 * the tool drawer onto the chat surface. Display-only; stored records keep
 * the original bytes. */
export function normalizeCliStream(input: string): string {
  const frames = stripAnsi(input)
    .split('\n')
    .map((line) => {
      const parts = line.split('\r')
      return parts[parts.length - 1]
    })
  return frames.join('\n').replace(/\n{3,}/g, '\n\n').replace(/[ \t]+$/gm, '').trimEnd()
}

/** P64.11 — payloads over this token budget render as a spooled blob card
 * (stats + preview + inspect action) instead of an inline dump. */
export const SPOOL_TOKEN_BUDGET = 2000

/** Rough token estimate (4 chars ≈ 1 token) for spool decisions. */
export function estimateTokens(text: string): number {
  if (!text) return 0
  return Math.ceil(text.length / 4)
}

/** Short display kind for a tool id (`codex_terminal_exec` → `Codex`). Used
 * for the grouped drawer summary; the full id stays in Technical details. */
function shortToolKind(toolId: string): string {
  const word =
    toolId.toLowerCase().replace(/[^a-z0-9]+/g, ' ').split(/\s+/).find(Boolean) ?? 'task'
  return word.charAt(0).toUpperCase() + word.slice(1)
}

/** P64.11 — spooled big-payload card. Results over ~2,000 tokens are treated
 * as spooled to content-addressed disk storage (`artifact.retrieve_original(hash)` on
 * the host): the drawer shows stats plus a short preview, and `Inspect in
 * Right Rail ↗` opens the full cleaned output in the dedicated `tool-output`
 * right-rail view without bloating the thread. The inline expand remains as
 * the fallback when the rail is unavailable. */
function SpooledBlobCard({ id, toolId, text, failed }: { id: string; toolId: string; text: string; failed: boolean }) {
  const [expanded, setExpanded] = useState(false)
  const [copied, setCopied] = useState(false)
  const notify = useAppStore((s) => s.notify)
  const openSpooledOutput = useAppStore((s) => s.openSpooledOutput)
  const tokens = estimateTokens(text)
  const lines = text.split('\n').length
  const preview = text.split('\n').slice(0, 8).join('\n')
  const inspectId = `spooled-full-${id}`
  return (
    <div className="rounded-md border border-border/60 bg-background/60 p-2" data-testid="spooled-blob">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[10px] text-muted-foreground">
        <Database aria-hidden className="h-3 w-3 shrink-0" />
        <span className="font-mono tabular-nums">
          ~{tokens.toLocaleString()} tokens · {lines.toLocaleString()} lines ·{' '}
          {text.length.toLocaleString()} chars
        </span>
        <span className="rounded-full border border-border px-1.5 py-px font-mono text-[9px] uppercase tracking-wide">
          spooled
        </span>
      </div>
      {!expanded && (
        <pre className="mt-1 max-h-24 overflow-hidden whitespace-pre-wrap break-words font-mono text-[10px] text-muted-foreground/80">
          {preview}
        </pre>
      )}
      <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded border border-brand/40 bg-brand/10 px-2 py-1 text-[10px] text-brand transition-colors hover:bg-brand/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
          onClick={() => openSpooledOutput({ toolCallId: id, toolId, text, failed })}
          title="Open the full spooled output in the right rail"
        >
          Inspect in Right Rail ↗
        </button>
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded border border-border px-2 py-1 text-[10px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
          aria-controls={inspectId}
          title="Show the full spooled output inline here instead"
        >
          {expanded ? 'Collapse inline' : 'Expand inline'}
        </button>
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded border border-border px-2 py-1 text-[10px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
          onClick={() => {
            navigator.clipboard?.writeText(text)
            setCopied(true)
            setTimeout(() => setCopied(false), 1500)
            notify('Full tool output copied')
          }}
        >
          {copied ? <Check aria-hidden className="h-2.5 w-2.5 text-emerald-400" /> : <Copy aria-hidden className="h-2.5 w-2.5" />}
          {copied ? 'Copied' : 'Copy full output'}
        </button>
      </div>
      {expanded && (
        <pre
          id={inspectId}
          className={cn(
            'mt-1.5 max-h-96 overflow-auto whitespace-pre-wrap break-words font-mono text-[10px]',
            failed ? 'text-rose-300' : 'text-emerald-300/80',
          )}
        >
          {text}
        </pre>
      )}
    </div>
  )
}

// P45.9 — memoized: tool-call rows only re-render when the record changes
// (streaming updates mutate the running call's object identity; settled calls
// keep identity, so untouched chips skip re-render entirely).
const ToolChip = memo(function ToolChip({ rec }: { rec: ToolCallRecord }) {
  const [detailsOpen, setDetailsOpen] = useState(false)
  const retry = useAppStore((s) => s.retryToolCall)
  const [, force] = useState(0)
  useEffect(() => {
    if (rec.status !== 'running' || rec.startedAt === undefined) return
    const t = setInterval(() => force((v) => v + 1), 1000)
    return () => clearInterval(t)
  }, [rec.status, rec.startedAt])
  const summary = toolActivitySummary(rec)
  const source = rec.error ?? rec.result
  // P64.11 — normalize CLI streams at the drawer boundary: the render-side
  // twin of the `UIEventEnvelope` normalization in `agentcowork-acp`. Raw
  // stdout/stderr (ANSI, spinners, progress churn) never reaches the chat
  // surface unformatted; only the quarantined drawer shows it, cleaned.
  const normalized = useMemo(
    () => (typeof source === 'string' ? normalizeCliStream(source) : safeJson(source)),
    [source],
  )
  const [parsedLarge, setParsedLarge] = useState<string | null>(null)
  useEffect(() => {
    if (normalized.length < 8_000) {
      setParsedLarge(null)
      return
    }
    let cancelled = false
    void parseToolResult(normalized).then((value) => {
      if (cancelled) return
      setParsedLarge(typeof value === 'string' ? value : JSON.stringify(value, null, 2))
    })
    return () => {
      cancelled = true
    }
  }, [normalized])
  const displayText = parsedLarge ?? normalized
  // P64.11 — payloads over ~2,000 tokens count as spooled: the drawer shows
  // the compact blob card instead of the full dump, so the thread stays lean.
  const spooled = displayText.length > 0 && estimateTokens(displayText) > SPOOL_TOKEN_BUDGET
  const rawResult = displayText
  const requestId = (rec as ToolCallRecord & { requestId?: string }).requestId
  const detailsId = `tool-details-${rec.id}`

  return (
    <div
      role="listitem"
      className={cn(
        'min-h-[44px] overflow-hidden rounded-lg border bg-background/40',
        rec.status === 'failed' && 'border-rose-500/40',
        rec.status === 'running' && 'border-brand/30',
        rec.status === 'done' && 'border-border',
      )}
      data-testid="tool-activity"
      data-tool-status={rec.status}
    >
      <div className="flex min-w-0 items-center gap-2 px-2.5 py-1.5">
        {rec.status === 'running' ? (
          <Loader2
            aria-hidden
            className="h-3.5 w-3.5 shrink-0 animate-spin text-brand motion-reduce:animate-none"
          />
        ) : rec.status === 'failed' ? (
          <X aria-hidden className="h-3.5 w-3.5 shrink-0 text-rose-400" />
        ) : (
          <CheckCircle2 aria-hidden className="h-3.5 w-3.5 shrink-0 text-emerald-400" />
        )}
        <div className="min-w-0 flex-1">
          <p className="truncate text-[11px] leading-relaxed text-foreground" aria-live="polite">
            {summary.sentence}
          </p>
          <div className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[9px] text-muted-foreground">
            <span>{summary.status}</span>
            {summary.duration && <span className="font-mono tabular-nums">{summary.duration}</span>}
            {/* P64.12 — specialist attribution badge: visible on the row
                itself, so delegation is obvious without opening details. */}
            {rec.specialist && (
              <span
                title={`Ran by ${rec.specialist}`}
                className="inline-flex items-center rounded-full border border-sky-500/40 bg-sky-500/10 px-1.5 py-px font-mono text-[9px] text-sky-300"
              >
                @{rec.specialist}
              </span>
            )}
          </div>
        </div>
        <button
          type="button"
          className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-1 text-[10px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
          onClick={() => setDetailsOpen((v) => !v)}
          aria-expanded={detailsOpen}
          aria-controls={detailsId}
          aria-label={`${detailsOpen ? 'Hide' : 'Show'} technical details for this step`}
        >
          Technical details
          <ChevronDown
            aria-hidden
            className={cn('h-3 w-3 transition-transform motion-reduce:transition-none', detailsOpen && 'rotate-180')}
          />
        </button>
      </div>
      {detailsOpen && (
        <div id={detailsId} className="space-y-2 border-t border-border/60 bg-zinc-950/40 px-2.5 py-2">
          <dl className="grid grid-cols-[auto_1fr] gap-x-2 gap-y-1 text-[10px]">
            <dt className="text-muted-foreground">Tool</dt>
            <dd className="break-all font-mono text-foreground">{rec.toolId}</dd>
            {rec.specialist && (
              <>
                <dt className="text-muted-foreground">Specialist</dt>
                <dd className="font-mono text-foreground">@{rec.specialist}</dd>
              </>
            )}
            {rec.risk && (
              <>
                <dt className="text-muted-foreground">Risk</dt>
                <dd>
                  <span
                    className={cn(
                      'inline-flex items-center gap-1 rounded-full border px-1.5 py-0 font-mono text-[9px] uppercase tracking-wide',
                      riskTone(rec.risk),
                    )}
                  >
                    <ShieldAlert aria-hidden className="h-2.5 w-2.5" />
                    {rec.risk}
                  </span>
                </dd>
              </>
            )}
            {requestId && (
              <>
                <dt className="text-muted-foreground">Request ID</dt>
                <dd className="break-all font-mono text-foreground">{requestId}</dd>
              </>
            )}
          </dl>
          {rec.args && (
            <div>
              <p className="mb-1 text-[9px] uppercase tracking-wide text-muted-foreground">Arguments</p>
              <pre className="max-h-40 overflow-auto whitespace-pre-wrap break-words font-mono text-[10px] text-muted-foreground">
                {safeJson(rec.args)}
              </pre>
            </div>
          )}
          {rawResult && !spooled && (
            <div>
              <p className="mb-1 text-[9px] uppercase tracking-wide text-muted-foreground">
                {rec.error ? 'Error' : 'Result'}
              </p>
              <pre
                className={cn(
                  'max-h-40 overflow-auto whitespace-pre-wrap break-words font-mono text-[10px]',
                  rec.status === 'failed' ? 'text-rose-300' : 'text-emerald-300/80',
                )}
              >
                {rawResult}
              </pre>
            </div>
          )}
          {spooled && (
            <SpooledBlobCard id={rec.id} toolId={rec.toolId} text={rawResult} failed={rec.status === 'failed'} />
          )}
          {rec.status === 'failed' && (
            <button
              type="button"
              onClick={() => void retry(rec.id)}
              className="inline-flex items-center gap-1 rounded border border-rose-500/40 bg-rose-500/10 px-2 py-1 text-[10px] text-rose-200 hover:bg-rose-500/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
            >
              <RotateCw aria-hidden className="h-2.5 w-2.5" />
              Retry this step
            </button>
          )}
        </div>
      )}
    </div>
  )
})

/** P64.11 — <ToolExecutionBox />: the grouped multi-tool drawer. Two or more
 * actions in one turn collapse into a single compact header (`Executed N
 * actions (Read, Terminal, Patch) · 1.8s`) that is closed by default once
 * settled; expanding reveals the itemized rows (params, normalized output,
 * spooled refs, retry). A single-tool turn renders as its own
 * closed-by-default chip above, which is the same one-row pattern. The
 * reserved min-height plus content-visibility keeps streaming CLS at zero. */
export default function ToolChips({ calls }: { calls: ToolCallRecord[] }) {
  const settled = calls.every((call) => call.status !== 'running')
  // Closed by default once settled; follows live transitions (opens while
  // work runs, collapses when the turn settles) but never fights an explicit
  // user toggle in between — the effect only runs when `settled` flips.
  const [open, setOpen] = useState(!settled)
  const [, force] = useState(0)
  useEffect(() => {
    setOpen(!settled)
  }, [settled])
  // Live tick so the grouped total keeps time while work is running.
  useEffect(() => {
    if (settled) return
    const t = setInterval(() => force((v) => v + 1), 1000)
    return () => clearInterval(t)
  }, [settled])
  const kinds = useMemo(() => {
    const seen: string[] = []
    for (const c of calls) {
      const k = shortToolKind(c.toolId)
      if (!seen.includes(k)) seen.push(k)
      if (seen.length >= 3) break
    }
    return seen
  }, [calls])
  // Grouped wall-clock total: earliest start → latest end (or now live).
  // Computed inline (not memoized) so the live tick above refreshes it.
  const starts = calls.map((c) => c.startedAt).filter((v): v is number => v !== undefined)
  const ends = calls.map((c) => c.endedAt).filter((v): v is number => v !== undefined)
  const total =
    starts.length === 0
      ? ''
      : formatToolDuration(Math.min(...starts), !settled || ends.length === 0 ? Date.now() : Math.max(...ends))
  if (calls.length === 0) return null
  const list = (
    <div className="mt-2 flex min-h-[44px] flex-col gap-1.5 [content-visibility:auto] [contain-intrinsic-size:auto_44px]" role="list" aria-label="Work activity">
      {calls.map((c) => (
        <ToolChip key={c.id} rec={c} />
      ))}
    </div>
  )
  if (calls.length < 2) return list
  return (
    <div className="mt-2 min-h-[28px]" data-testid="tool-execution-box">
      <button
        type="button"
        className="inline-flex min-h-[24px] items-center gap-1.5 rounded border border-border/60 bg-background/40 px-2 py-1 text-[10px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
        title={open ? 'Collapse the grouped tool runs' : 'Expand the grouped tool runs'}
      >
        {settled ? (
          <Wrench aria-hidden className="h-3 w-3 shrink-0" />
        ) : (
          <Loader2 aria-hidden className="h-3 w-3 shrink-0 animate-spin text-brand motion-reduce:animate-none" />
        )}
        <span aria-live="polite">
          Executed {calls.length} actions
          {kinds.length > 0 && <span> ({kinds.join(', ')})</span>}
          {total && <span className="font-mono tabular-nums"> · {total}</span>}
          {!settled && <span> · running</span>}
        </span>
        <ChevronDown
          aria-hidden
          className={cn('h-3 w-3 transition-transform motion-reduce:transition-none', open && 'rotate-180')}
        />
      </button>
      {open && list}
    </div>
  )
}
