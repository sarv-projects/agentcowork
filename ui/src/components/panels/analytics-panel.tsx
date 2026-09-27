'use client'

import { useEffect, useState } from 'react'
import {
  Area, AreaChart, Bar, BarChart, Cell, Pie, PieChart,
  ResponsiveContainer, Tooltip, XAxis, YAxis,
} from 'recharts'
import { BarChart3, Coins, Cpu, DollarSign, Layers, Timer } from 'lucide-react'
import { useAppStore } from '@/lib/store'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { inTauri } from '@/lib/tauri'
import {
  costReadout,
  sessionTotals,
  unreportedOwners,
  usageSnapshot,
} from '@/lib/spend'
import { ChartCard, ModelLeaderboard, SessionsTable, AgentBreakdown } from './analytics-sections'

const KPIS = [
  { label: 'Total spent', value: '$5.42', icon: DollarSign, tone: 'text-brand' },
  { label: 'Tokens used', value: '1.2M', icon: Cpu, tone: 'text-sky-300' },
  { label: 'Chats', value: '12', icon: Layers, tone: 'text-foreground' },
  { label: 'Avg cost/chat', value: '$0.45', icon: Timer, tone: 'text-emerald-300' },
]

const SPEND_30D = Array.from({ length: 30 }, (_, i) => ({
  day: `${i + 1}`,
  spend: +(0.05 + Math.abs(Math.sin(i / 4)) * 0.4 + (i % 7 === 0 ? 0.2 : 0)).toFixed(2),
}))

const TOKENS_BY_MODEL = [
  { model: 'Claude', tokens: 480 },
  { model: 'GPT-4o', tokens: 320 },
  { model: 'Gemini', tokens: 180 },
  { model: 'DeepSeek', tokens: 140 },
  { model: 'Ollama', tokens: 80 },
]

const COST_BY_CATEGORY = [
  { name: 'Chat', value: 2.4, color: 'hsl(25 95% 53%)' },
  { name: 'Browser', value: 1.1, color: 'hsl(217 91% 60%)' },
  { name: 'Office', value: 0.92, color: 'hsl(142 71% 45%)' },
  { name: 'Code', value: 0.7, color: 'hsl(38 92% 50%)' },
  { name: 'Research', value: 0.3, color: 'hsl(280 65% 60%)' },
]

const TOOLTIP_STYLE = {
  background: 'hsl(240 8% 9%)',
  border: '1px solid hsl(240 6% 16%)',
  borderRadius: 6,
  fontSize: 11,
}

export default function AnalyticsPanel() {
  const notify = useAppStore((s) => s.notify)
  const [live, setLive] = useState<{
    spent: number
    /** P71.4 — whether `spent` is a producer's report or our price estimate. */
    costKind: 'reported' | 'estimated' | 'unknown'
    tokens: number
    sessions: number
    warnNotDelegating?: boolean
    /** Owners whose turns produced no usage report (I15 — absent, not zero). */
    unreported: Array<[string, number]>
    /** Agreed reporters, labelled. */
    reporters: string[]
  } | null>(null)
  const [liveError, setLiveError] = useState<string | null>(null)
  const [exporting, setExporting] = useState(false)
  useEffect(() => {
    if (!inTauri()) return
    let active = true
    setLiveError(null)
    Promise.all([usageSnapshot(), sessionTotals()]).then(([snapshot, totals]) => {
      if (active) {
        // P71.4 — the primary/worker split exists only when the turn path
        // attributed a primary agent; `null` renders as "not attributed",
        // never as a 0% share. The old fallback invented a share from the
        // totals, which is exactly what this row removes (I15).
        const spend = snapshot.primarySpend ?? null
        const cost = costReadout(snapshot)
        setLive({
          spent: cost.usd ?? 0,
          costKind: cost.kind,
          tokens: snapshot.total.tokensIn + snapshot.total.tokensOut,
          sessions: totals.length,
          warnNotDelegating: spend?.warnNotDelegating,
          unreported: unreportedOwners(snapshot),
          reporters: Object.keys(snapshot.observations?.by_source ?? {}),
        })
      }
    }).catch((e) => { if (active) { setLive(null); setLiveError(e instanceof Error ? e.message : String(e)) } })
    return () => { active = false }
  }, [])

  const exportCsv = () => {
    void (async () => {
      setExporting(true)
      try {
        const rows = await sessionTotals()
        if (rows.length === 0) {
          notify('No ledger rows to export yet')
          return
        }
        const csv = [
          'chat,tokens_in,tokens_out,cost_usd',
          ...rows.map((r) => `${JSON.stringify(r.session)},${r.tokensIn},${r.tokensOut},${r.cost.toFixed(4)}`),
        ].join('\n')
        const blob = new Blob([csv], { type: 'text/csv' })
        const url = URL.createObjectURL(blob)
        const a = document.createElement('a')
        a.href = url
        a.download = 'chat-usage.csv'
        a.click()
        URL.revokeObjectURL(url)
        notify(`Exported ${rows.length} ledger rows`)
      } catch (e) {
        notify(e instanceof Error ? e.message : 'CSV export failed', 'error')
      } finally {
        setExporting(false)
      }
    })()
  }

  const kpis = live
    ? [
        {
          label:
            live.costKind === 'reported'
              ? 'Total spent (reported)'
              : live.costKind === 'estimated'
                ? 'Total spent (estimated)'
                : 'Total spent',
          // P71.4 — an agent that reported no cost gets an honest dash, never a
          // plausible number built from a price table (I15).
          value: live.costKind === 'unknown' ? '—' : `$${live.spent.toFixed(2)}`,
          icon: DollarSign,
          tone: 'text-brand',
        },
        { label: 'Tokens used', value: live.tokens >= 1_000_000 ? `${(live.tokens / 1_000_000).toFixed(1)}M` : `${Math.round(live.tokens / 1000)}K`, icon: Cpu, tone: 'text-sky-300' },
        { label: 'Chats', value: String(live.sessions), icon: Layers, tone: 'text-foreground' },
        { label: 'Avg cost/chat', value: live.sessions > 0 ? `$${(live.spent / live.sessions).toFixed(2)}` : '—', icon: Timer, tone: 'text-emerald-300' },
      ]
    : KPIS

  return (
    <div className="flex h-full w-full flex-col">
      <header className="flex flex-wrap items-center justify-between gap-2 border-b border-border px-4 py-3">
        <div className="flex items-center gap-2">
          <BarChart3 className="h-4 w-4 text-brand" />
          <h2 className="text-sm font-semibold text-foreground">Analytics</h2>
          <Badge variant="secondary" className="text-[9px]">token &amp; cost</Badge>
          {inTauri() ? (
            live ? (
              <>
                <Badge className="bg-emerald-500/15 text-[9px] text-emerald-300">live ledger</Badge>
                {live.warnNotDelegating && (
                  <Badge className="bg-warning/15 text-[9px] text-warning">Primary agent is not delegating</Badge>
                )}
                {live.costKind === 'estimated' && (
                  <Badge className="bg-muted text-[9px] text-muted-foreground"
                    title="No producer reported a cost — this is AgentCowork's estimate from configured prices">
                    cost estimated
                  </Badge>
                )}
                {live.costKind === 'unknown' && (
                  <Badge className="bg-muted text-[9px] text-muted-foreground"
                    title="Nothing has reported tokens or cost yet — absent is not zero">
                    no usage reported
                  </Badge>
                )}
                {live.reporters.length > 0 && (
                  <Badge className="bg-muted text-[9px] text-muted-foreground">
                    reported by {live.reporters.join(', ')}
                  </Badge>
                )}
              </>
            ) : liveError ? (
              <Badge className="bg-rose-500/15 text-[9px] text-rose-300">ledger unavailable</Badge>
            ) : null
          ) : (
            <Badge className="bg-brand/15 text-[9px] text-brand">sample data</Badge>
          )}
        </div>
      </header>

      <div className="scroll-thin min-h-0 flex-1 overflow-y-auto">
        <div className="space-y-4 p-4">
          {inTauri() && !live && (
            <div className="rounded-lg border border-dashed border-border bg-card p-4 text-xs text-muted-foreground">
              {liveError ? `Live ledger unavailable — ${liveError}` : 'Loading live analytics from the encrypted usage ledger…'}
            </div>
          )}
          {/* P71.4 — the honest gap: turns that finished with no usage report.
              Shown as a fact about the ledger, not as zero spending. */}
          {live && live.unreported.length > 0 && (
            <div className="rounded-lg border border-warning/30 bg-warning/5 px-4 py-3 text-[11px] text-warning">
              <div className="font-medium">Some turns reported no usage</div>
              <p className="mt-0.5 text-muted-foreground">
                {live.unreported
                  .slice(0, 4)
                  .map(([owner, turns]) => `${owner} ×${turns}`)
                  .join(' · ')}
                {live.unreported.length > 4 ? ` · +${live.unreported.length - 4} more` : ''}
                {' '}— those turns are absent from these totals, not zero.
              </p>
            </div>
          )}
          {/* KPI cards — live ledger in the shell, labeled sample data in preview */}
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
            {kpis.map((k) => {
              const Icon = k.icon
              return (
                <div key={k.label} className="rounded-lg border border-border bg-card p-4">
                  <div className="flex items-center gap-1.5 text-[10px] text-muted-foreground">
                    <Icon className="h-3 w-3" />
                    {k.label}
                  </div>
                  <div className={cn('mt-1 font-mono text-xl font-semibold', k.tone)}>{k.value}</div>
                </div>
              )
            })}
          </div>

          {!inTauri() && (
          <>
          {/* Sample charts — design preview only, never ledger data */}
          <ChartCard
            title="Daily spend"
            subtitle="sample data · design preview"
            right={<span className="font-mono text-xs text-brand">$5.42 total</span>}
          >
            <div className="chart-crossfade h-44 w-full">
              <ResponsiveContainer width="100%" height="100%">
                <AreaChart data={SPEND_30D} margin={{ top: 4, right: 4, bottom: 0, left: -24 }}>
                  <defs>
                    <linearGradient id="spendGrad" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="0%" stopColor="hsl(25 95% 53%)" stopOpacity={0.6} />
                      <stop offset="100%" stopColor="hsl(25 95% 53%)" stopOpacity={0.02} />
                    </linearGradient>
                  </defs>
                  <XAxis dataKey="day" tick={{ fontSize: 9, fill: 'hsl(240 5% 55%)' }} tickLine={false} axisLine={false} interval={5} />
                  <YAxis tick={{ fontSize: 9, fill: 'hsl(240 5% 55%)' }} tickLine={false} axisLine={false} width={28} />
                  <Tooltip cursor={{ stroke: 'hsl(25 95% 53%)', strokeWidth: 1 }} contentStyle={TOOLTIP_STYLE} />
                  <Area type="monotone" dataKey="spend" stroke="hsl(25 95% 53%)" strokeWidth={2} fill="url(#spendGrad)" />
                </AreaChart>
              </ResponsiveContainer>
            </div>
          </ChartCard>

          {/* Second row: tokens by model + cost donut (sample data) */}
          <div className="grid gap-3 lg:grid-cols-2">
            <ChartCard title="Tokens by model" subtitle="sample data · design preview">
              <div className="chart-crossfade h-40 w-full">
                <ResponsiveContainer width="100%" height="100%">
                  <BarChart data={TOKENS_BY_MODEL} margin={{ top: 4, right: 4, bottom: 0, left: -24 }}>
                    <XAxis dataKey="model" tick={{ fontSize: 9, fill: 'hsl(240 5% 55%)' }} tickLine={false} axisLine={false} />
                    <YAxis tick={{ fontSize: 9, fill: 'hsl(240 5% 55%)' }} tickLine={false} axisLine={false} width={28} />
                    <Tooltip cursor={{ fill: 'hsl(25 95% 53% / 0.1)' }} contentStyle={TOOLTIP_STYLE} />
                    <Bar dataKey="tokens" fill="hsl(25 95% 53%)" radius={[3, 3, 0, 0]} />
                  </BarChart>
                </ResponsiveContainer>
              </div>
            </ChartCard>

            <ChartCard title="Cost by category" subtitle="sample data · design preview">
              <div className="flex items-center gap-2">
                <div className="h-32 w-1/2">
                  <ResponsiveContainer width="100%" height="100%">
                    <PieChart>
                      <Pie data={COST_BY_CATEGORY} dataKey="value" nameKey="name" innerRadius={28} outerRadius={56} paddingAngle={2} stroke="none">
                        {COST_BY_CATEGORY.map((e, i) => (
                          <Cell key={i} fill={e.color} />
                        ))}
                      </Pie>
                      <Tooltip contentStyle={TOOLTIP_STYLE} />
                    </PieChart>
                  </ResponsiveContainer>
                </div>
                <ul className="flex-1 space-y-1">
                  {COST_BY_CATEGORY.map((c) => (
                    <li key={c.name} className="flex items-center gap-2 text-[11px]">
                      <span className="inline-block size-2.5 rounded-sm" style={{ background: c.color }} />
                      <span className="flex-1 text-foreground/80">{c.name}</span>
                      <span className="font-mono text-muted-foreground">${c.value.toFixed(2)}</span>
                    </li>
                  ))}
                </ul>
              </div>
            </ChartCard>
          </div>

          </>
          )}
          <SessionsTable />
          {!inTauri() && (
            <div className="grid gap-3 lg:grid-cols-2">
              <ModelLeaderboard />
              <AgentBreakdown />
            </div>
          )}
        </div>
      </div>

      <footer className="flex items-center justify-between border-t border-border bg-card px-4 py-2">
        <span className="font-mono text-[10px] text-muted-foreground">
          <Coins className="mr-1 inline h-3 w-3" />
          {inTauri() ? 'durable usage ledger · per-chat aggregates' : 'preview pricing metadata'}
        </span>
        <Button
          size="sm"
          variant="outline"
          className="h-7 text-xs"
          disabled={exporting || !inTauri()}
          title={inTauri() ? 'Download the live ledger as CSV' : 'CSV export needs the Tauri shell'}
          onClick={exportCsv}
        >
          {exporting ? 'Exporting…' : 'Export CSV'}
        </Button>
      </footer>
    </div>
  )
}
