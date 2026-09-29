'use client'

import { useState, type ElementType } from 'react'
import { useReducedMotion } from 'framer-motion'
import { ArrowUpRight, CalendarClock, Check, CircleDot, FileStack, Pause, Play, RotateCcw, ShieldAlert, ShieldCheck, Wifi } from 'lucide-react'
import { usePref } from '@/lib/ui-prefs'
import type { AgentCard } from '@/lib/agent-card'
import { useAppStore, type Artifact, type SessionStatus } from '@/lib/store'
import { cn } from '@/lib/utils'
import { projectLiveDesk, type LiveDeskProjection, type LiveDeskStatus } from './live-desk-projection'
import type { LiveDeskUpdate } from './live-desk-activity'

const STATUS_STYLE: Record<LiveDeskStatus, { icon: ElementType; tone: string }> = {
  working: { icon: CircleDot, tone: 'text-brand' },
  waiting: { icon: Pause, tone: 'text-warning' },
  reconnecting: { icon: Wifi, tone: 'text-warning' },
  scheduled: { icon: CalendarClock, tone: 'text-muted-foreground' },
  finished: { icon: Check, tone: 'text-success' },
  stopped: { icon: ShieldAlert, tone: 'text-rose-300' },
  paused: { icon: Pause, tone: 'text-sky-300' },
  ready: { icon: CircleDot, tone: 'text-muted-foreground' },
}

interface LiveDeskSnapshot {
  projection: LiveDeskProjection
  updates: LiveDeskUpdate[]
  artifacts: Artifact[]
}

export function LiveDeskSummary({
  title,
  sessionStatus,
  card,
  updates = [],
  needsAttention = false,
  artifacts = [],
}: {
  title?: string | null
  sessionStatus?: SessionStatus | null
  card: AgentCard
  updates?: readonly LiveDeskUpdate[]
  needsAttention?: boolean
  artifacts?: readonly Artifact[]
}) {
  const systemReducedMotion = useReducedMotion()
  const [motionEnabled, setMotionEnabled] = usePref('liveDeskMotion', true)
  const setActiveView = useAppStore((state) => state.setActiveView)
  const openOfficeDoc = useAppStore((state) => state.openOfficeDoc)
  const liveProjection = projectLiveDesk({
    title,
    sessionStatus,
    card,
    needsAttention: needsAttention || updates.some((update) => update.needsUser),
  })
  const liveArtifacts = [...artifacts].reverse().filter((artifact, index, reversed) =>
    reversed.findIndex((candidate) => candidate.id === artifact.id) === index,
  ).slice(0, 3)
  const [heldSnapshot, setHeldSnapshot] = useState<LiveDeskSnapshot | null>(null)
  const projection = heldSnapshot?.projection ?? liveProjection
  const visibleUpdates = heldSnapshot?.updates ?? updates
  const recentArtifacts = heldSnapshot?.artifacts ?? liveArtifacts
  const { icon: StatusIcon, tone } = STATUS_STYLE[projection.status]
  const animate = motionEnabled && !systemReducedMotion
  const running = projection.status === 'working'

  return (
    <section
      aria-labelledby="live-desk-heading"
      className="relative overflow-hidden rounded-xl border border-border bg-gradient-to-br from-brand/[0.09] via-card to-card px-4 py-4 shadow-sm"
      data-testid="live-desk-summary"
    >
      <div aria-hidden className="pointer-events-none absolute -right-8 -top-10 h-32 w-32 rounded-full bg-brand/[0.08] blur-2xl" />
      <div className="relative">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <p className="text-[10px] font-medium uppercase tracking-[0.16em] text-muted-foreground">Live Desk</p>
            <h2 id="live-desk-heading" className="mt-1 truncate text-base font-semibold text-foreground" title={projection.title}>
              {projection.title}
            </h2>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <button
              type="button"
              aria-pressed={!motionEnabled}
              aria-label={motionEnabled ? 'Pause Live Desk animation' : 'Resume Live Desk animation'}
              title={systemReducedMotion ? 'Reduced motion is enabled on this device' : undefined}
              disabled={!!systemReducedMotion}
              onClick={() => setMotionEnabled(!motionEnabled)}
              className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-border/70 bg-background/70 px-2 text-[10px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60 disabled:cursor-default disabled:opacity-60"
            >
              {motionEnabled && !systemReducedMotion ? <Pause aria-hidden className="h-3 w-3" /> : <Play aria-hidden className="h-3 w-3" />}
              <span>{systemReducedMotion ? 'Reduced motion' : motionEnabled ? 'Pause motion' : 'Static'}</span>
            </button>
            {heldSnapshot ? (
              <button
                type="button"
                aria-label="Resume Live Desk updates"
                onClick={() => setHeldSnapshot(null)}
                className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-border/70 bg-background/70 px-2 text-[10px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
              >
                <RotateCcw aria-hidden className="h-3 w-3" />
                Resume updates
              </button>
            ) : (
              <button
                type="button"
                aria-label="Pause Live Desk updates"
                onClick={() => setHeldSnapshot({ projection: liveProjection, updates: [...updates], artifacts: liveArtifacts })}
                className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-border/70 bg-background/70 px-2 text-[10px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
              >
                <Pause aria-hidden className="h-3 w-3" />
                Pause updates
              </button>
            )}
          </div>
        </div>

        <div
          key={projection.status}
          className={cn('mt-4 flex items-start gap-3', animate && 'enter-surface')}
          role="status"
          aria-live="polite"
          aria-atomic="true"
        >
          <span className={cn('mt-0.5 inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-background/80 ring-1 ring-border/70', tone)}>
            <StatusIcon aria-hidden className={cn('h-4 w-4', running && animate && 'live-desk-pulse')} />
          </span>
          <div className="min-w-0 flex-1">
            <p className={cn('text-sm font-semibold', tone)}>{projection.statusLabel}</p>
            <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{projection.summary}</p>
            {heldSnapshot ? (
              <p className="mt-2 text-[10px] font-medium text-warning">Updates are paused. This view may be out of date; the work continues.</p>
            ) : null}
          </div>
        </div>

        {visibleUpdates.length > 0 ? (
          <ol className="mt-4 space-y-2 border-t border-border/60 pt-3" aria-label="Recent work updates" aria-live="polite" aria-relevant="additions" data-testid="live-desk-updates">
            {visibleUpdates.map((update) => (
              <li key={update.key} className={cn('flex min-w-0 items-start gap-2 text-xs', animate && 'enter-surface')}>
                <UpdateIcon state={update.state} />
                <span className="min-w-0 flex-1 leading-relaxed text-foreground/85">{update.label}</span>
                <time className="shrink-0 pt-0.5 text-[10px] text-muted-foreground" dateTime={update.at ? new Date(update.at).toISOString() : undefined}>
                  {formatTime(update.at)}
                </time>
              </li>
            ))}
          </ol>
        ) : null}

        {recentArtifacts.length > 0 ? (
          <div className="mt-4 border-t border-border/60 pt-3" aria-label="Recent outputs from this chat">
            <p className="mb-2 text-[10px] font-medium uppercase tracking-[0.12em] text-muted-foreground">Recent outputs</p>
            <ul className="space-y-1.5">
              {recentArtifacts.map((artifact) => {
                const officePath = artifact.path ?? artifact.name
                const officeType = /\.(xlsx|xlsm|docx|pptx|pdf)$/i.test(officePath)
                  ? officePath
                  : /\.(xlsx|xlsm|docx|pptx|pdf)$/i.test(artifact.name)
                    ? artifact.name
                    : null
                const canOpen = Boolean(officeType || artifact.view)
                return (
                  <li key={`${artifact.id}:${artifact.name}`}>
                    <button
                      type="button"
                      disabled={!canOpen}
                      aria-label={canOpen ? `Open ${artifact.name} in the workspace` : `${artifact.name} has no workspace viewer`}
                      onClick={() => {
                        if (officeType) openOfficeDoc(officeType)
                        else if (artifact.view) setActiveView(artifact.view)
                      }}
                      className="flex min-h-10 w-full items-center gap-2 rounded-lg border border-border/70 bg-background/55 px-2.5 text-left transition-colors hover:border-brand/40 hover:bg-background disabled:cursor-default disabled:opacity-70 disabled:hover:border-border/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
                    >
                      <span className="min-w-0 flex-1 truncate text-xs text-foreground">{artifact.name}</span>
                      <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[9px] uppercase text-muted-foreground">{artifact.type}</span>
                      {canOpen ? <ArrowUpRight aria-hidden className="h-3 w-3 shrink-0 text-muted-foreground" /> : null}
                    </button>
                  </li>
                )
              })}
            </ul>
          </div>
        ) : null}

        {(projection.filesTouched > 0 || projection.checksReported > 0) ? (
          <div className="mt-4 flex flex-wrap gap-2" aria-label="Reported results">
            {projection.filesTouched > 0 ? (
              <ResultPill icon={FileStack} label={`${projection.filesTouched} file${projection.filesTouched === 1 ? '' : 's'} involved`} />
            ) : null}
            {projection.checksReported > 0 ? (
              <ResultPill
                icon={Check}
                label={`Agent-reported checks: ${projection.checksPassed} of ${projection.checksReported} passed${projection.checksFailed > 0 ? ` · ${projection.checksFailed} failed` : ''}`}
                tone={projection.checksFailed > 0 ? 'warning' : 'default'}
              />
            ) : null}
          </div>
        ) : null}
      </div>
    </section>
  )
}

function UpdateIcon({ state }: { state: LiveDeskUpdate['state'] }) {
  const Icon = state === 'waiting'
    ? Pause
    : state === 'verified'
      ? ShieldCheck
      : state === 'failed'
        ? ShieldAlert
        : state === 'cancelled'
          ? CircleDot
          : Check
  const tone = state === 'waiting'
    ? 'text-warning'
    : state === 'verified'
      ? 'text-success'
      : state === 'failed'
        ? 'text-rose-300'
        : 'text-muted-foreground'
  return <Icon aria-hidden className={cn('mt-0.5 h-3.5 w-3.5 shrink-0', tone)} />
}

function formatTime(timestamp: number | null): string {
  if (timestamp === null || !Number.isFinite(timestamp) || timestamp <= 0) return 'Time unknown'
  return new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' }).format(timestamp)
}

function ResultPill({
  icon: Icon,
  label,
  tone = 'default',
}: {
  icon: ElementType
  label: string
  tone?: 'default' | 'warning'
}) {
  return (
    <span className={cn('inline-flex items-center gap-1.5 rounded-full border bg-background/70 px-2.5 py-1 text-[10px]', tone === 'warning' ? 'border-warning/40 text-warning' : 'border-border/70 text-muted-foreground')}>
      <Icon aria-hidden className="h-3 w-3" />
      {label}
    </span>
  )
}
