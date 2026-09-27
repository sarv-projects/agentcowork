'use client'

// The run's ordered trace, in the same vocabulary the Progress timeline uses.
//
// Every row is a projected `WorkEventEnvelope` (label, tone, status all come
// from `describeWorkEvent` + `timelineStatus` — never re-derived here), and the
// status set is deliberately wider than done/running/failed:
//
//   • failed and running are visually distinct **and** carry a word, so nothing
//     depends on colour alone;
//   • an unknown effect outcome renders as "Outcome unknown" with a question
//     mark — never as done and never as failed;
//   • a run parked on the user shows a static pause and the words "Waiting for
//     you", so the indicator stops instead of spinning forever.
//
// Durations are printed only where the journal can prove one: a settled tool
// against its own start, a settled run against its start. Everything else shows
// no number rather than an invented one.
//
// A run journal is long, so the list is operable: a class filter and a text
// filter with honest "showing N of M" counts, plus the APG listbox keyboard
// pattern (one tab stop, arrows/Home/End to move) instead of one tab stop per
// step. State changes are announced politely, because the rail's only live
// signal for a run is inside this list (ARCH/12 §0.1).

import { useEffect, useId, useRef, useState } from 'react'
import {
  Brain,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  CircleDot,
  CircleHelp,
  ExternalLink,
  GitBranch,
  Loader2,
  PauseCircle,
  Radio,
  Search,
  ShieldCheck,
  Terminal,
  TestTube,
  Wrench,
  X,
  XCircle,
  Zap,
} from 'lucide-react'
import {
  RUN_FILTERS,
  RUN_STATUS_LABEL,
  countRunSteps,
  filterRunSteps,
  formatClock,
  formatDuration,
  type RunFilter,
  type RunStep,
  type RunStepStatus,
} from '@/components/views/run-projection'
import { useAppStore } from '@/lib/store'
import { cn } from '@/lib/utils'

/** The tone → icon map, mirroring the Progress timeline's. */
function ToneIcon({ tone }: { tone: string }) {
  const cls = 'h-3.5 w-3.5'
  switch (tone) {
    case 'step':
      return <Zap className={cn(cls, 'text-brand')} />
    case 'tool':
      return <Wrench className={cn(cls, 'text-warning')} />
    case 'file':
      return <Zap className={cn(cls, 'text-emerald-400')} />
    case 'approval':
      return <ShieldCheck className={cn(cls, 'text-violet-400')} />
    case 'thought':
      return <Brain className={cn(cls, 'text-sky-400')} />
    case 'node':
      return <CircleDot className={cn(cls, 'text-cyan-400')} />
    case 'worktree':
      return <GitBranch className={cn(cls, 'text-teal-400')} />
    case 'pty':
      return <Terminal className={cn(cls, 'text-zinc-400')} />
    case 'test':
      return <TestTube className={cn(cls, 'text-emerald-400')} />
    default:
      return <Radio className={cn(cls, 'text-violet-400')} />
  }
}

/** Status → (icon, row chrome, dot, icon tint). The word beside it is the
 *  accessible name; the colour is decoration on top of it. */
function statusFace(status: RunStepStatus) {
  switch (status) {
    case 'running':
      return {
        Icon: Loader2,
        row: 'border-brand/50 bg-brand/[0.07]',
        dot: 'bg-brand',
        icon: 'text-brand',
      }
    case 'waiting':
      return {
        Icon: PauseCircle,
        row: 'border-warning/50 bg-warning/[0.07]',
        dot: 'bg-warning',
        icon: 'text-warning',
      }
    case 'failed':
      return {
        Icon: XCircle,
        row: 'border-rose-500/50 bg-rose-500/[0.07]',
        dot: 'bg-rose-400',
        icon: 'text-rose-300',
      }
    case 'uncertain':
      return {
        Icon: CircleHelp,
        row: 'border-dashed border-zinc-500/50 bg-zinc-500/[0.07]',
        dot: 'bg-zinc-400',
        icon: 'text-zinc-300',
      }
    default:
      return {
        Icon: CheckCircle2,
        row: 'border-border',
        dot: 'bg-emerald-500',
        icon: 'text-emerald-400',
      }
  }
}

export function RunTrace({ steps }: { steps: readonly RunStep[] }) {
  const [filter, setFilter] = useState<RunFilter>('all')
  const [query, setQuery] = useState('')
  const [openKey, setOpenKey] = useState<string | null>(null)
  const addView = useAppStore((s) => s.addView)

  const counts = countRunSteps(steps)
  const visible = filterRunSteps(steps, { filter, query })
  const total = steps.length
  const shown = visible.length
  const filtered = shown !== total

  // APG listbox keyboard pattern: the list is one tab stop and the arrows move
  // between rows, instead of a run journal costing one tab stop per step.
  const listId = useId()
  const [cursor, setCursor] = useState(0)
  const rowRefs = useRef<(HTMLButtonElement | null)[]>([])
  useEffect(() => {
    if (cursor > shown - 1) setCursor(Math.max(0, shown - 1))
  }, [shown, cursor])

  const onListKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp' && event.key !== 'Home' && event.key !== 'End') {
      return
    }
    if (shown === 0) return
    event.preventDefault()
    const last = shown - 1
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? last
          : event.key === 'ArrowDown'
            ? Math.min(last, cursor + 1)
            : Math.max(0, cursor - 1)
    setCursor(next)
    rowRefs.current[next]?.focus()
  }

  // The live row is the only live signal this panel has, so a change in the
  // state counts is announced rather than left to the pulse alone.
  const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`
  const announcement = [
    counts.running > 0 ? plural(counts.running, 'step running', 'steps running') : null,
    counts.waiting > 0 ? plural(counts.waiting, 'step waiting for you', 'steps waiting for you') : null,
    counts.failed > 0 ? plural(counts.failed, 'step failed', 'steps failed') : null,
    counts.uncertain > 0
      ? plural(counts.uncertain, 'step with an unknown outcome', 'steps with an unknown outcome')
      : null,
  ]
    .filter(Boolean)
    .join(', ')

  if (total === 0) {
    return (
      <p className="rounded-md border border-dashed border-border px-2 py-2 text-[10px] leading-relaxed text-muted-foreground">
        No steps recorded yet. The run journal fills this list as the agent works — nothing is
        pre-filled here.
      </p>
    )
  }

  return (
    <div data-testid="run-trace">
      {/* Reserved-height toolbar: the filter and the search never move the list
          when they appear, so CLS stays at zero. */}
      <div className="mb-1.5 flex min-h-[22px] flex-wrap items-center gap-1">
        <div role="group" aria-label="Filter steps by kind" className="flex flex-wrap items-center gap-0.5">
          {RUN_FILTERS.map((f) => (
            <button
              key={f.id}
              type="button"
              onClick={() => setFilter(f.id)}
              aria-pressed={filter === f.id}
              className={cn(
                'rounded-full border px-1.5 py-px font-mono text-[9px] transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60',
                filter === f.id
                  ? 'border-brand/50 bg-brand/10 text-brand'
                  : 'border-transparent text-muted-foreground hover:bg-accent hover:text-foreground',
              )}
            >
              {f.label}
            </button>
          ))}
        </div>
        <div className="relative ml-auto min-w-[92px] flex-1">
          <Search
            aria-hidden
            className="pointer-events-none absolute left-1.5 top-1/2 h-2.5 w-2.5 -translate-y-1/2 text-muted-foreground/70"
          />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') setQuery('')
            }}
            placeholder="Filter (Esc clears)"
            aria-label="Filter steps by text"
            className="h-[22px] w-full rounded-md border border-border bg-background/60 pl-5 pr-5 font-mono text-[9px] text-foreground placeholder:text-muted-foreground/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
          />
          {query ? (
            <button
              type="button"
              onClick={() => setQuery('')}
              aria-label="Clear the text filter"
              className="absolute right-0.5 top-1/2 grid h-4 w-4 -translate-y-1/2 place-items-center rounded text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
            >
              <X aria-hidden className="h-2.5 w-2.5" />
            </button>
          ) : null}
        </div>
      </div>

      <div className="mb-1.5 flex flex-wrap items-center gap-1.5 font-mono text-[9px] tabular-nums text-muted-foreground">
        <span>
          {filtered
            ? `${shown} of ${total} steps`
            : `${total} step${total === 1 ? '' : 's'}`}
        </span>
        <span aria-hidden>·</span>
        <span>{counts.done} done</span>
        {counts.running > 0 ? (
          <>
            <span aria-hidden>·</span>
            <span className="text-brand">{counts.running} running</span>
          </>
        ) : null}
        {counts.waiting > 0 ? (
          <>
            <span aria-hidden>·</span>
            <span className="text-warning">{counts.waiting} waiting for you</span>
          </>
        ) : null}
        {counts.failed > 0 ? (
          <>
            <span aria-hidden>·</span>
            <span className="text-rose-300">{counts.failed} failed</span>
          </>
        ) : null}
        {counts.uncertain > 0 ? (
          <>
            <span aria-hidden>·</span>
            <span className="text-zinc-300">{counts.uncertain} outcome unknown</span>
          </>
        ) : null}
      </div>

      {shown === 0 ? (
        <p className="rounded-md border border-dashed border-border px-2 py-2 text-[10px] leading-relaxed text-muted-foreground">
          No step matches this filter. The run recorded {total} step{total === 1 ? '' : 's'} — clear the
          filter to see {total === 1 ? 'it' : 'them'}.
        </p>
      ) : (
        <div
          role="listbox"
          id={listId}
          aria-label="Run steps"
          aria-orientation="vertical"
          onKeyDown={onListKeyDown}
          className="focus-visible:outline-none"
        >
          {visible.map((step, index) => (
            <TraceRow
              key={step.key}
              step={step}
              focusable={index === cursor}
              registerRef={(node) => {
                rowRefs.current[index] = node
              }}
              open={openKey === step.key}
              onToggle={() => setOpenKey(openKey === step.key ? null : step.key)}
              onFollow={() => {
                const link = step.link
                if (!link) return
                if (link.target === 'code') {
                  window.dispatchEvent(
                    new CustomEvent('agentcowork:open-file', {
                      detail: { path: link.path, content: '' },
                    }),
                  )
                  addView('code')
                } else if (link.target === 'audit') {
                  addView('audit')
                } else {
                  addView('shell')
                }
              }}
            />
          ))}
        </div>
      )}

      {/* Politely announced, visually hidden: the state change is the only
          live signal this list carries. */}
      <p aria-live="polite" role="status" className="sr-only">
        {announcement}
      </p>
    </div>
  )
}

function TraceRow({
  step,
  open,
  focusable,
  registerRef,
  onToggle,
  onFollow,
}: {
  step: RunStep
  open: boolean
  focusable: boolean
  registerRef: (node: HTMLButtonElement | null) => void
  onToggle: () => void
  onFollow: () => void
}) {
  const face = statusFace(step.status)
  const duration = formatDuration(step.durationMs)
  const panelId = `${step.key}-detail`
  const buttonId = `${step.key}-button`
  const StatusIcon = face.Icon

  return (
    <div role="option" aria-selected={open} className={cn('rounded-md border', face.row)}>
      <div className="flex items-center gap-1.5">
        <button
          type="button"
          id={buttonId}
          ref={registerRef}
          tabIndex={focusable ? 0 : -1}
          aria-expanded={open}
          aria-controls={panelId}
          onClick={onToggle}
          className="flex min-w-0 flex-1 items-center gap-1.5 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-white/[0.03] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-brand/60"
        >
          <span
            aria-hidden
            className={cn(
              'relative h-2 w-2 shrink-0 rounded-full ring-2 ring-card',
              face.dot,
              step.status === 'running' && 'live-dot',
            )}
          />
          <ToneIcon tone={step.tone} />
          <span className="shrink-0 font-mono text-[9px] tabular-nums text-muted-foreground/70">
            {formatClock(step.atMs)}
          </span>
          <span className="min-w-0 flex-1 truncate text-[10.5px] text-foreground/90">
            {step.label}
          </span>
          {duration ? (
            <span
              className="shrink-0 font-mono text-[9px] tabular-nums text-muted-foreground/70"
              title={step.durationNote ?? undefined}
            >
              {duration}
            </span>
          ) : null}
          <span
            className={cn(
              'inline-flex shrink-0 items-center gap-1 font-mono text-[9px]',
              face.icon,
            )}
          >
            <StatusIcon aria-hidden className="h-3 w-3" />
            {RUN_STATUS_LABEL[step.status]}
          </span>
          {step.detail || step.link ? (
            open ? (
              <ChevronDown aria-hidden className="h-3 w-3 shrink-0 text-muted-foreground" />
            ) : (
              <ChevronRight aria-hidden className="h-3 w-3 shrink-0 text-muted-foreground" />
            )
          ) : null}
        </button>
        {/* The one honest drill-down, offered only when the entry names a
            resource a lens can actually open. */}
        {step.link ? (
          <button
            type="button"
            onClick={onFollow}
            title={step.link.label}
            aria-label={`${step.link.label}: ${step.label}`}
            className="mr-1.5 grid h-6 w-6 shrink-0 place-items-center rounded-md border border-border text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
          >
            <ExternalLink aria-hidden className="h-3 w-3" />
          </button>
        ) : null}
      </div>
      {open ? (
        <div
          id={panelId}
          role="region"
          aria-labelledby={buttonId}
          className="enter-surface mx-2 mb-2 rounded border border-border/60 bg-background/50 p-2"
        >
          {step.detail ? (
            <p className="whitespace-pre-wrap break-words font-mono text-[10px] leading-relaxed text-foreground/80">
              {step.detail}
            </p>
          ) : (
            <p className="font-mono text-[10px] italic text-muted-foreground">
              This step reported no detail.
            </p>
          )}
          <dl className="mt-1.5 grid grid-cols-2 gap-x-2 gap-y-0.5 font-mono text-[9px] text-muted-foreground/70">
            <dt>status</dt>
            <dd className="text-foreground/80">{RUN_STATUS_LABEL[step.status]}</dd>
            <dt>journal</dt>
            <dd className="truncate text-foreground/80">
              {step.eventClass} · {step.eventKind}
            </dd>
            <dt>sequence</dt>
            <dd className="tabular-nums text-foreground/80">#{step.sequence}</dd>
            <dt>duration</dt>
            <dd className="text-foreground/80">
              {duration
                ? `${duration} ${step.durationNote ?? ''}`.trim()
                : 'not measurable from the journal'}
            </dd>
            {step.link ? (
              <>
                <dt>names</dt>
                <dd className="truncate text-foreground/80">
                  {step.link.target === 'code' ? step.link.path : step.link.id}
                </dd>
              </>
            ) : null}
            {step.traceId ? (
              <>
                <dt>trace</dt>
                <dd className="truncate text-foreground/80">{step.traceId}</dd>
              </>
            ) : null}
          </dl>
        </div>
      ) : null}
    </div>
  )
}
