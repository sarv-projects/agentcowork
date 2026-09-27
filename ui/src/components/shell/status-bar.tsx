'use client'

import * as React from 'react'
import {
  CircleDot,
  Cpu,
  Database,
  HardDrive,
  MonitorSmartphone,
  Network,
  ShieldCheck,
  Sparkles,
  Wifi,
  Zap,
  Activity,
  Clock,
  CheckCircle2,
  AlertTriangle,
  Loader2,
  Bell,
  Radar,
} from 'lucide-react'
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useAppStore } from '@/lib/store'
import * as perfLib from '@/lib/perf'
import { isRuntimeUsable } from '@/lib/agents'
import { CompanionChip } from './companion-chip'
import { cn } from '@/lib/utils'
import { useRuntimeState } from '@/lib/runtime'
import { ARCH_VERSION } from '@/lib/version'


interface Stat {
  icon: React.ElementType
  label: string
  value: string
  color?: string
  tooltip: string
  /** P50.3.7/8 — optional click-through to the owning surface. */
  onClick?: () => void
}

/** P52.10/P52.18 — live context + cost pill for the casual status bar.
 * Renders only when the shell reported real figures (never a seeded look):
 * context % + tokens/s while streaming, spend + cache hit once a budget
 * snapshot exists. Click opens the breakdown popover (rows show only what
 * the shell reported — absent figures are omitted, never zeroed). */
function LiveContextMeter() {
  const liveBudget = useAppStore((s) => s.liveBudget)
  const streamStats = useAppStore((s) => s.streamStats)
  const activeStatus = useAppStore((s) => s.sessions.find((x) => x.id === s.activeSessionId)?.status)
  const tokens = useAppStore((s) => s.sessions.find((x) => x.id === s.activeSessionId)?.tokens)
  const spent = useAppStore((s) => s.sessions.find((x) => x.id === s.activeSessionId)?.spent)
  // P51.25 — per-pill customization: the meter renders only the pill classes
  // the user left on (localStorage-backed), and the popover hosts the toggles.
  const pills = useAppStore((s) => s.statusBarPills)
  const setPills = useAppStore((s) => s.setStatusBarPills)

  // Nothing real yet — keep the bar quiet.
  if (!liveBudget && !streamStats.tokensPerSec && !streamStats.ctxPct) return null

  const live = streamStats.tokensPerSec > 0 && (activeStatus === 'running' || activeStatus === 'action-required')
  const ctxPct = streamStats.ctxPct
  const tone =
    ctxPct >= 90
      ? 'text-red-400'
      : ctxPct >= 75
        ? 'text-warning'
        : 'text-muted-foreground/70'
  const cachePct =
    liveBudget?.cacheHitRate != null ? `${Math.round(liveBudget.cacheHitRate * 100)}%` : null
  const detailBits = [
    pills.context && ctxPct > 0 ? `context ${ctxPct}%` : null,
    pills.throughput && live && streamStats.tokensPerSec > 0 ? `${streamStats.tokensPerSec.toFixed(0)} tok/s` : null,
    pills.cache && cachePct ? `cache ${cachePct}` : null,
    pills.cost && (tokens || spent != null) ? `${(tokens ?? 0) / 1000}kt · $${(spent ?? 0).toFixed(2)}` : null,
  ].filter(Boolean)
  if (detailBits.length === 0) return null

  const rows: { label: string; value: string }[] = []
  if (pills.context && ctxPct > 0) rows.push({ label: 'context', value: `${ctxPct}%` })
  if (pills.throughput && live && streamStats.tokensPerSec > 0)
    rows.push({ label: 'throughput', value: `${streamStats.tokensPerSec.toFixed(0)} tok/s` })
  if (pills.cost && tokens) rows.push({ label: 'chat tokens', value: `${(tokens / 1000).toFixed(1)}k` })
  if (pills.cost && liveBudget?.tokens) rows.push({ label: 'lifetime tokens', value: `${(liveBudget.tokens / 1000).toFixed(1)}k` })
  if (pills.cache && cachePct) rows.push({ label: 'prompt cache', value: cachePct })
  if (pills.cost && spent != null) rows.push({ label: 'chat spend', value: `$${spent.toFixed(4)}` })
  if (pills.cost && liveBudget) rows.push({ label: 'lifetime spend', value: `$${liveBudget.spent.toFixed(4)} / cap $${liveBudget.cap.toFixed(2)}` })

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label="Context and usage breakdown"
          className="flex items-center gap-1 px-2 py-0.5 font-mono text-[10px] text-muted-foreground/70 transition-colors hover:text-foreground"
          title={`${detailBits.join(' · ')} — click for the full breakdown`}
        >
          {pills.context && (
            <span className={cn('relative h-1 w-10 overflow-hidden rounded-full bg-border', tone)}>
              {ctxPct > 0 && (
                <span
                  className={cn(
                    'absolute inset-y-0 left-0 rounded-full',
                    ctxPct >= 90 ? 'bg-red-500/80' : ctxPct >= 75 ? 'bg-warning/80' : 'bg-brand/70',
                  )}
                  style={{ width: `${Math.min(100, ctxPct)}%` }}
                />
              )}
            </span>
          )}
          {pills.throughput && live && streamStats.tokensPerSec > 0 && (
            <span className="tabular-nums">{streamStats.tokensPerSec.toFixed(0)} t/s</span>
          )}
          {pills.cost && spent ? <span className="tabular-nums text-emerald-400/80">${spent.toFixed(2)}</span> : null}
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" align="end" className="w-56 font-mono text-[10px]">
        <div className="px-1 pb-1 text-[9px] uppercase tracking-wider text-muted-foreground/60">
          Context &amp; usage
        </div>
        {rows.map((r) => (
          <div key={r.label} className="flex items-center justify-between gap-3 py-0.5">
            <span className="text-muted-foreground">{r.label}</span>
            <span className="tabular-nums text-foreground/90">{r.value}</span>
          </div>
        ))}        {ctxPct >= 75 && (
          <div className="mt-1 border-t border-border/60 pt-1 text-[9px] text-warning/90">
            {ctxPct >= 90
              ? 'Context nearly full — clear this chat or fork before it stalls.'
              : 'Context is high — clear or fork soon.'}
          </div>
        )}
        {/* P51.25 — per-pill customization: which live pills the status bar
            may render. Persisted to localStorage via the store. */}
        <div className="mt-1.5 flex items-center gap-1 border-t border-border/60 pt-1.5">
          <span className="text-[9px] uppercase tracking-wider text-muted-foreground/50">Pills</span>
          {(
            [
              ['context', 'ctx'],
              ['throughput', 't/s'],
              ['cache', 'cache'],
              ['cost', 'cost'],
            ] as const
          ).map(([key, label]) => (
            <button
              key={key}
              type="button"
              onClick={() => setPills({ ...pills, [key]: !pills[key] })}
              className={cn(
                'rounded border px-1.5 py-0.5 text-[9px] transition-colors',
                pills[key]
                  ? 'border-brand/40 bg-brand/15 text-brand'
                  : 'border-border text-muted-foreground/50 hover:text-foreground',
              )}
              title={pills[key] ? `Hide the ${label} pill` : `Show the ${label} pill`}
            >
              {label}
            </button>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  )
}

export function StatusBar() {
  const agentPaused = useAppStore((s) => s.agentPaused)
  // Perf: select only the active session's agent label, not the whole
  // sessions array — streamed token batches replace the array identity every
  // tick, and this bar is always mounted (with a blur backdrop, each extra
  // render recomposites the full width).
  const activeAgent = useAppStore((s) => s.sessions.find((x) => x.id === s.activeSessionId)?.agent)
  // Same scoping for the status pill (changes rarely; message text churn must
  // not re-render this bar).
  const activeStatus = useAppStore((s) => s.sessions.find((x) => x.id === s.activeSessionId)?.status)
  const hasActive = useAppStore((s) => s.sessions.some((x) => x.id === s.activeSessionId))
  const activeId = useAppStore((s) => s.activeSessionId)
  const selectedAgentId = useAppStore((s) => s.selectedAgentId)
  const autoRoute = useAppStore((s) => s.autoRoute)
  const liveBudget = useAppStore((s) => s.liveBudget)
  const browserAttached = useAppStore((s) => s.browserAttached)
  // P50.3.7 — desktop engine attachment in the dev strip (mirrors browser).
  const desktopAttached = useAppStore((s) => s.desktopAttached)
  const desktopReason = useAppStore((s) => s.desktopReason)
  const setActiveView = useAppStore((s) => s.setActiveView)
  const devMode = useAppStore((s) => s.devMode)
  const statusBarPills = useAppStore((s) => s.statusBarPills)
  const monitorBadge = useAppStore((s) => s.monitorBadge)
  const clearMonitorBadge = useAppStore((s) => s.clearMonitorBadge)
  const cockpitOpen = useAppStore((s) => s.cockpitOpen)
  const setCockpitOpen = useAppStore((s) => s.setCockpitOpen)
  const { usePerfSnapshot } = perfLib
  const runtime = useRuntimeState()
  // P58.6 — occupancy comes from the live install/discovery catalog, never the
  // static seed: before hydration (or when discovery fails) the live list is
  // empty and the agent block simply does not render. A curated row for an
  // uninstalled CLI must never be painted as the active runtime.
  const liveAgents = useAppStore((s) => s.liveAgents)
  const agent = liveAgents.find((a) => a.id === selectedAgentId && isRuntimeUsable(a))
  // P60 / P71.2d — model ownership. Every runtime is an external ACP agent that
  // owns its own model, and AgentCowork owns no model surface to paint here.
  const acpModelOption = useAppStore((s) => {
    const opts = s.acpConfigOptions[selectedAgentId]
    if (!opts?.length) return undefined
    return (
      opts.find((o) => o.category === 'model') ??
      opts.find((o) => o.id.toLowerCase().includes('model'))
    )
  })
  // P60 / P71.2d — the label is the agent's own ACP value, or an explicit
  // "managed by <agent>". There is no desktop model pin left to name, so the
  // bar cannot imply AgentCowork picked the model.
  const modelLabel = acpModelOption
    ? String(acpModelOption.currentValue)
    : agent?.name || selectedAgentId
      ? `managed by ${agent?.name ?? selectedAgentId}`
      : 'no agent bound'
  // Agent health, latency, uptime, and task counts are not available from the
  // runtime contract yet. Never invent them; show unknown until a live probe
  // supplies evidence.
  const healthLatency: number | undefined = undefined
  const HealthIcon = CircleDot
  const healthCol = 'text-zinc-400'
  const runtimeValue = runtime.status === 'live'
    ? 'live'
    : runtime.status === 'preview'
      ? 'preview'
      : runtime.status

  const stats: Stat[] = [
    {
      icon: Sparkles,
      label: 'agent',
      value: activeAgent ?? '—',
      tooltip: activeAgent ? `Active agent: ${activeAgent}` : 'No active chat agent is available.',
    },
    {
      icon: Cpu,
      label: 'sidecar',
      value: runtimeValue,
      color: runtime.status === 'live' ? 'text-emerald-400' : 'text-warning',
      tooltip: runtime.detail ?? 'Coordinator readiness is reported by the native runtime probe.',
    },
    {
      icon: Database,
      label: 'core',
      value: runtime.status === 'live' ? 'available' : 'unknown',
      color: runtime.status === 'live' ? 'text-emerald-400' : 'text-muted-foreground',
      tooltip: 'Core availability is not claimed until the runtime is live.',
    },
    {
      icon: HardDrive,
      label: 'db',
      value: '—',
      tooltip: 'Database size is shown only when a live storage probe supplies it.',
    },
    {
      icon: Network,
      label: 'mcp',
      value: '—',
      tooltip: 'MCP endpoint is not attached or reported by the runtime probe.',
    },
    {
      icon: Wifi,
      label: 'browser',
      value: browserAttached ? 'attached' : 'not attached',
      color: browserAttached ? 'text-emerald-400' : 'text-muted-foreground',
      tooltip: browserAttached
        ? 'CDP attached — the browse view drives a real browser.'
        : 'No CDP connection — start the browser from the Browse surface.',
      onClick: () => setActiveView('browse'),
    },
    {
      icon: MonitorSmartphone,
      label: 'desktop',
      value: desktopAttached ? 'attached' : 'not attached',
      color: desktopAttached ? 'text-emerald-400' : 'text-muted-foreground',
      tooltip: desktopAttached
        ? 'Desktop engine attached — the Computer use view drives native windows.'
        : desktopReason
          ? `Desktop engine detached — ${desktopReason}`
          : 'Desktop engine detached — open the Computer use surface to probe it.',
      onClick: () => setActiveView('desktop'),
    },
    {
      icon: Zap,
      label: 'cache',
      // P51.25 — respects the per-pill cache toggle (hidden = still measured,
      // just not rendered).
      value: statusBarPills.cache && liveBudget?.cacheHitRate != null
        ? `${Math.round(liveBudget.cacheHitRate * 100)}%`
        : '—',
      color: statusBarPills.cache && liveBudget?.cacheHitRate != null ? 'text-emerald-400' : 'text-muted-foreground',
      tooltip: statusBarPills.cache && liveBudget?.cacheHitRate != null
        ? `Prompt cache hit rate · ${Math.round(liveBudget.cacheHitRate * 100)}% (live)`
        : statusBarPills.cache
          ? 'Prompt cache hit rate is unavailable until the live usage ledger responds.'
          : 'The cache pill is hidden in status-bar pills — reopen its toggle from the context meter.',
    },
  ]

  // P11.5.4 — per-session takeover indicator (the active session's pause).
  const pausedSessions = useAppStore((s) => s.pausedSessions)
  const activePaused = !!pausedSessions[activeId]
  // P11.4 — LCP / TTI readout (dev mode) + discreet cold-start chip (casual).
  const perf = usePerfSnapshot()

  // Casual default: hide the debug telemetry strip. Show a single discreet
  // state pill + privacy reassurance; Settings → General → "Developer Mode"
  // restores the full 12-badge telemetry (devMode).
  if (!devMode) {
    const busy = activeStatus === 'running' || activeStatus === 'action-required'
    const preview = runtime.status === 'preview'
    return (
      <footer className="shrink-0 h-6 border-t border-border bg-sidebar flex items-center text-[10.5px] font-mono no-select">
        <div className="flex items-center gap-1.5 px-3">
          <span className={cn(
            'h-1.5 w-1.5 rounded-full',
            preview ? 'bg-warning' : activePaused ? 'bg-warning' : busy ? 'bg-brand live-dot' : 'bg-emerald-400'
          )} />
          <span className="text-muted-foreground">
            {preview ? 'Development preview' : activePaused ? '⏸ Paused' : busy ? 'Processing…' : runtimeValue}
          </span>
        </div>
        <div className="flex-1" />
        {perf.coldStartMs != null && perf.coldStartMs < 2000 && (
          <span className="px-3 text-muted-foreground/50" title="Cold start (P11.4)">
            boot {perf.coldStartMs}ms
          </span>
        )}
        {preview ? (
          <span className="flex items-center gap-1.5 px-3 text-muted-foreground/70">
            <AlertTriangle className="h-2.5 w-2.5 text-warning" />
            Plain-browser preview — not connected to the shell
          </span>
        ) : (
          <span className="flex items-center gap-1.5 px-3 text-muted-foreground/70">
            <ShieldCheck className={cn('h-2.5 w-2.5', runtime.status === 'live' ? 'text-emerald-400' : 'text-warning')} />
            {runtime.status === 'live' ? 'Privacy depends on selected provider' : 'Privacy status unavailable'}
          </span>
        )}
        {/* P52.10 — context + cost at a glance (live when the shell is up;
            absent in preview). Clickless, hover explains. */}
        <LiveContextMeter />
        <CompanionChip />
        <button
          type="button"
          onClick={() => setCockpitOpen(!cockpitOpen)}
          className="flex items-center gap-1 px-1 text-muted-foreground/70 hover:text-brand"
          title="Cockpit — live agent cards and interrupts"
          aria-label="Toggle cockpit slide-over"
        >
          <Radar className="h-2.5 w-2.5" />
          cockpit
        </button>
        <span className="pr-3 text-muted-foreground/40">AgentCowork {ARCH_VERSION}</span>
      </footer>
    )
  }

  return (
    <footer className="shrink-0 h-6 border-t border-border bg-sidebar flex items-center text-[10.5px] font-mono no-select">
      {/* Left cluster — agent health monitor */}
      <div className="flex items-center gap-1.5 px-2 border-r border-border/60 h-full">
        <span className={cn(
          'h-1.5 w-1.5 rounded-full',
          agentPaused || activePaused ? 'bg-warning' : 'bg-brand live-dot'
        )} />
        <span className={cn(
          'text-muted-foreground',
          agentPaused || activePaused ? '' : 'text-brand'
        )}>
          {agentPaused || activePaused ? 'paused' : 'live'}
        </span>
        {perf.lcpMs != null && perf.ttiMs != null && (
          <Tooltip>
            <TooltipTrigger asChild>
              <span className="text-muted-foreground/50" title="P11.4 — LCP / TTI">
                {perf.lcpMs}ms · {perf.ttiMs}ms
              </span>
            </TooltipTrigger>
            <TooltipContent>LCP (target &lt;1s) · TTI (target &lt;2s)</TooltipContent>
          </Tooltip>
        )}
        {monitorBadge.count > 0 && (
          <button
            type="button"
            onClick={() => clearMonitorBadge()}
            className="ml-1 inline-flex items-center gap-0.5 rounded-full border border-brand/40 bg-brand/15 px-1.5 py-0 text-[9px] text-brand"
            title={monitorBadge.last ?? 'Monitor'}
          >
            <Bell className="h-2.5 w-2.5" />
            {monitorBadge.count}
            {monitorBadge.stopped ? ' · stopped' : ''}
          </button>
        )}
        <span className="text-muted-foreground/40">·</span>
        <span className="text-muted-foreground/80">
          {activeStatus === 'action-required' ? 'awaiting approval' : activeStatus === 'running' ? 'running' : hasActive ? 'idle' : 'no active work'}
        </span>
        {/* Agent runtime health indicator */}          {agent && (

          <Tooltip>
            <TooltipTrigger asChild>
              <div className="flex items-center gap-1.5 cursor-default">
                <span className="text-muted-foreground/30">│</span>
                <span className={cn('h-3.5 w-3.5 rounded text-[6px] font-bold flex items-center justify-center', agent.accent)}>{agent.mark}</span>
                <HealthIcon className={cn('h-2.5 w-2.5', healthCol)} />
                <span className={cn('text-[9.5px]', healthCol)}>
                  {healthLatency ?? '—'}ms
                </span>
                <span className="max-w-[16rem] truncate text-muted-foreground/50">
                  {modelLabel}
                </span>
                {autoRoute && (
                  <span className="text-brand/60">auto</span>
                )}
              </div>
            </TooltipTrigger>
            <TooltipContent side="top" className="font-mono text-[11px] max-w-xs">
              <div className="space-y-1">
                <div className="font-semibold">{agent.name} · {modelLabel}</div>
                <div>Status: unavailable until the selected runtime is probed</div>
                <div>Tasks: — · Error rate: —</div>
                <div>Uptime: —</div>
              </div>
            </TooltipContent>
          </Tooltip>
        )}
      </div>

      {/* Center — stats (devMode only) */}
      <div className="flex items-center gap-0 flex-1 overflow-x-auto scroll-thin">
        {stats.map((stat, i) => {
          const Icon = stat.icon
          const body = (
            <div className={cn(
              'flex items-center gap-1.5 px-2 h-full whitespace-nowrap',
              i < stats.length - 1 && 'border-r border-border/40',
              stat.onClick && 'cursor-pointer hover:bg-accent/50 rounded transition-colors'
            )}>
              <Icon className={cn('h-2.5 w-2.5 text-muted-foreground/70')} />
              <span className="text-muted-foreground/60">{stat.label}</span>
              <span className={cn('text-foreground/80', stat.color)}>
                {stat.value}
              </span>
            </div>
          )
          return (
            <Tooltip key={stat.label}>
              <TooltipTrigger asChild>
                {stat.onClick ? (
                  <button type="button" onClick={stat.onClick} aria-label={`Open ${stat.label} surface`} className="h-full">
                    {body}
                  </button>
                ) : body}
              </TooltipTrigger>
              <TooltipContent side="top" className="font-mono text-[11px]">
                {stat.tooltip}
              </TooltipContent>
            </Tooltip>
          )
        })}
      </div>

      {/* Right cluster — guard + version */}
      <div className="flex items-center gap-2 px-2 border-l border-border/60 h-full">
        <div className={cn('flex items-center gap-1', runtime.status === 'live' ? 'text-emerald-400' : 'text-warning')}>
          <ShieldCheck className="h-2.5 w-2.5" />
          <span>guard · {runtime.status === 'live' ? 'available' : 'unknown'}</span>
        </div>
        <span className="text-muted-foreground/40">·</span>
        <span className="text-muted-foreground/70">vault · {runtime.status === 'vault-locked' ? 'locked' : runtime.status === 'vault-setup' ? 'setup required' : '—'}</span>
        <span className="text-muted-foreground/40">·</span>
        <span className="text-muted-foreground/70">audit · {runtime.status === 'live' ? 'available' : '—'}</span>
        <span className="text-muted-foreground/40">·</span>
        <button
          type="button"
          onClick={() => setCockpitOpen(!cockpitOpen)}
          className="flex items-center gap-1 text-muted-foreground/70 hover:text-brand"
          title="Cockpit — live agent cards and interrupts"
          aria-label="Toggle cockpit slide-over"
        >
          <Radar className="h-2.5 w-2.5" />
          cockpit
        </button>
        <span className="text-muted-foreground/40">·</span>
        <span className="text-muted-foreground/50">AgentCowork {ARCH_VERSION}</span>
      </div>
    </footer>
  )
}
