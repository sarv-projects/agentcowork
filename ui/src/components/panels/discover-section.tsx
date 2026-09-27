'use client'

import { useEffect, useMemo, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { SectionShell } from './settings-shared'
import {
  discoveryInventory,
  type DiscoveryInventory,
  type ResourceCard,
  type ResourceKind,
} from '@/lib/discovery'

/**
 * P44.7 — Discover surface. One inventory across every managed-resource class
 * (Agents / Models / Providers / MCP / Skills / Browsers) with per-resource
 * cards + lifecycle status. Discovery ≠ install ≠ activation; auth is shown as
 * a shape, never a secret. Continuous refresh on demand + on mount.
 */
const KINDS: { kind: ResourceKind | 'all'; label: string }[] = [
  { kind: 'all', label: 'All' },
  { kind: 'agent', label: 'Agents' },
  { kind: 'model', label: 'Models' },
  { kind: 'provider', label: 'Providers' },
  { kind: 'mcp', label: 'MCP' },
  { kind: 'skill', label: 'Skills' },
  { kind: 'browser', label: 'Browsers' },
]

function statusTone(status: string): string {
  if (status === 'healthy' || status === 'in_use' || status === 'started') return 'text-emerald-400 border-emerald-500/30'
  if (status === 'enabled' || status === 'installed' || status === 'inventoried') return 'text-sky-400 border-sky-500/30'
  if (status === 'updating' || status === 'rolling_back') return 'text-warning border-warning/30'
  if (status === 'removed') return 'text-red-400 border-red-500/30'
  return 'text-slate-400 border-slate-500/30'
}

export function DiscoverSection() {
  const [inv, setInv] = useState<DiscoveryInventory | null>(null)
  const [loading, setLoading] = useState(false)
  const [filter, setFilter] = useState<ResourceKind | 'all'>('all')

  async function refresh() {
    setLoading(true)
    try {
      setInv(await discoveryInventory())
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    void refresh()
    // continuous background refresh (startup → registry refresh → health → UI).
    const id = setInterval(() => void refresh(), 15000)
    return () => clearInterval(id)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const cards = useMemo(() => {
    const all = inv?.cards ?? []
    return filter === 'all' ? all : all.filter((c) => c.kind === filter)
  }, [inv, filter])

  const c = inv?.counts

  return (
    <SectionShell
      title="Discover"
      desc="Everything AgentCowork manages — discovered, not necessarily installed or active. Auth is a shape, never a secret."
    >
      <div className="space-y-3">
        {/* Header counters */}
        <div className="grid grid-cols-3 gap-2 sm:grid-cols-6">
          {([
            ['Agents', c?.agents],
            ['Models', c?.models],
            ['Providers', c?.providers],
            ['MCP', c?.mcp],
            ['Skills', c?.skills],
            ['Browsers', c?.browsers],
          ] as const).map(([label, n]) => (
            <div key={label} className="rounded-md border border-border/50 bg-background/30 p-2 text-center">
              <div className="font-mono text-lg font-semibold text-brand">{n ?? 0}</div>
              <div className="text-[10px] text-muted-foreground">{label}</div>
            </div>
          ))}
        </div>

        {/* Filter + refresh */}
        <div className="flex flex-wrap items-center gap-1.5">
          {KINDS.map((k) => (
            <button
              key={k.kind}
              onClick={() => setFilter(k.kind)}
              className={`rounded px-2 py-0.5 text-[11px] ${
                filter === k.kind
                  ? 'bg-brand/20 text-brand'
                  : 'text-muted-foreground hover:bg-accent/40'
              }`}
            >
              {k.label}
            </button>
          ))}
          <Button size="sm" variant="outline" className="ml-auto h-7 text-xs" disabled={loading} onClick={refresh}>
            {loading ? 'Refreshing…' : 'Refresh'}
          </Button>
        </div>

        {/* Cards */}
        <ul className="space-y-1.5">
          {cards.length === 0 ? (
            <li className="rounded-md border border-dashed border-border p-4 text-center text-[11px] text-muted-foreground">
              Nothing discovered in this class yet.
            </li>
          ) : (
            cards.map((card: ResourceCard) => (
              <li key={`${card.kind}:${card.id}`} className="rounded-md border border-border/50 bg-card px-3 py-2">
                <div className="flex items-center gap-2">
                  <Badge variant="secondary" className="text-[9px] uppercase">{card.kind}</Badge>
                  <span className="flex-1 truncate text-xs font-medium text-foreground">{card.name || card.id}</span>
                  {card.version && <span className="font-mono text-[10px] text-muted-foreground">{card.version}</span>}
                  <Badge variant="outline" className={`text-[9px] ${statusTone(card.status)}`}>{card.status}</Badge>
                </div>
                <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-[10px] text-muted-foreground">
                  <span>source: <span className="font-mono">{card.source}</span></span>
                  <span>auth: <span className="font-mono">{card.auth}</span></span>
                  {card.governance && <span>governance: <span className="font-mono">{card.governance}</span></span>}
                  {card.capabilities.length > 0 && (
                    <span>
                      caps: <span className="font-mono">{card.capabilities.join(', ')}</span>
                      {card.capabilitiesVerified ? (
                        <span className="ml-1 text-emerald-400">✓ verified</span>
                      ) : (
                        <span className="ml-1 text-warning">advertised</span>
                      )}
                    </span>
                  )}
                </div>
              </li>
            ))
          )}
        </ul>
        <p className="text-[10px] text-muted-foreground">
          Lifecycle: discovered → validated → installed → inventoried → enabled → started → healthy →
          in&nbsp;use. Discovery only finds + describes; installing and enabling are separate,
          explicit steps. Credentials never enter here — only auth shapes.
        </p>
      </div>
    </SectionShell>
  )
}
