'use client'

// P55.2 / P55.3 — the Settings sections that used to render the wrong panels.
//
// Before this file, `settings-panel.tsx` mapped `permissions` → `ChatAutoRunSection`
// (composer behaviour, not the Guard matrix) and `usage` → `UxMetricsSection`
// (interaction telemetry, not spend). The nav keywords said guard/approval/...
// and cost/spend/tokens, so both sections lied.
//
// These sections call the same live commands the owning surfaces use:
//   guard_policy · guard_permissions_matrix · guard_set_policy_rules
//   guard_combos · guard_apply_combo          (via lib/guard)
//   usage_snapshot · session_totals           (via lib/spend)
// The Guard surface remains the owner of ticket approvals; this section owns
// the *policy view* of the same service (one writer, no second store).

import { useCallback, useEffect, useState } from 'react'
import { AlertTriangle, RefreshCw, ShieldCheck } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/lib/store'
import { inTauri } from '@/lib/tauri'
import {
  guardApplyCombo,
  guardCombos,
  guardPermissionsMatrix,
  guardPolicy,
  guardSetPolicyRules,
  type GuardCombo,
  type GuardPolicy,
  type MatrixCell,
  type PolicyRule,
} from '@/lib/guard'
import {
  costReadout,
  unreportedOwners,
  usageSnapshot,
  usageSourceLabel,
  type UsageSnapshot,
} from '@/lib/spend'
import { SessionsTable } from './analytics-sections'
import { Row, SectionShell } from './settings-shared'

// The grid labels mirror the Rust capability×scope matrix (presentation only —
// every DECISION comes from `guard_permissions_matrix`).
const CAPABILITIES = ['read', 'write', 'execute', 'network', 'browser']
const SCOPES = ['workspace', 'home', 'shell', 'external', 'browser']

type Cell = MatrixCell['decision']

const CELL_TONE: Record<Cell, string> = {
  allow: 'bg-emerald-500/70 text-emerald-50',
  ask: 'bg-brand/70 text-brand',
  block: 'bg-red-500/70 text-red-50',
  off: 'bg-zinc-700/40 text-zinc-400',
}

function Legend() {
  return (
    <div className="flex flex-wrap gap-2">
      {(['allow', 'ask', 'block', 'off'] as const).map((c) => (
        <span key={c} className="inline-flex items-center gap-1 text-[9px] text-muted-foreground">
          <span className={cn('inline-block size-2 rounded-sm', CELL_TONE[c].split(' ')[0])} />
          {c}
        </span>
      ))}
    </div>
  )
}

export function PermissionsSection() {
  const notify = useAppStore((s) => s.notify)
  const [policy, setPolicy] = useState<GuardPolicy | null>(null)
  const [matrix, setMatrix] = useState<MatrixCell[]>([])
  const [combos, setCombos] = useState<GuardCombo[]>([])
  const [rules, setRules] = useState<PolicyRule[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [applying, setApplying] = useState<string | null>(null)
  const [reload, setReload] = useState(0)

  useEffect(() => {
    let alive = true
    setLoading(true)
    void (async () => {
      try {
        const [p, m, c] = await Promise.all([
          guardPolicy(),
          guardPermissionsMatrix(),
          guardCombos(),
        ])
        if (!alive) return
        setPolicy(p)
        setMatrix(m)
        setCombos(c)
        setRules(p?.approvalRules ?? [])
        setError(null)
      } catch (e) {
        if (!alive) return
        // A failed load must not keep a stale projection on screen.
        setPolicy(null)
        setMatrix([])
        setCombos([])
        setError(e instanceof Error ? e.message : 'Guard is unavailable')
      } finally {
        if (alive) setLoading(false)
      }
    })()
    return () => {
      alive = false
    }
  }, [reload])

  const cellFor = useCallback(
    (capability: string, scope: string) =>
      matrix.find((c) => c.capability === capability && c.scope === scope)?.decision,
    [matrix],
  )

  const applyRules = async () => {
    try {
      const payload = rules.map((r) => ({
        tool: r.tool.trim(),
        ...(r.argsGlob?.trim() ? { argsGlob: r.argsGlob.trim() } : {}),
        approval: r.approval,
      }))
      const n = await guardSetPolicyRules(payload)
      notify(`Policy applied — ${n} rule(s)`)
    } catch (e) {
      notify(e instanceof Error ? e.message : 'Policy apply failed', 'error')
    }
  }

  const autonomy = policy?.autonomyLevel ?? policy?.profile ?? null

  return (
    <SectionShell
      title="Permissions"
      desc="The live Guard policy: capability × scope decisions, the tool allow-list, and one-click bundles. Hard floors (destructive actions, protected paths) are never relaxed here."
      action={
        <Button
          size="sm"
          variant="outline"
          className="h-7 gap-1 text-[10px]"
          onClick={() => setReload((v) => v + 1)}
        >
          <RefreshCw className={cn('h-3 w-3', loading && 'animate-spin')} /> Refresh
        </Button>
      }
    >
      {error && (
        <div className="flex items-center justify-between gap-3 rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-[11px] text-red-300">
          <span className="flex items-center gap-1.5">
            <AlertTriangle className="h-3 w-3" /> Guard policy is unavailable. {error}
          </span>
          <Button size="sm" variant="outline" className="h-6 shrink-0 text-[10px]" onClick={() => setReload((v) => v + 1)}>
            Retry
          </Button>
        </div>
      )}

      <Row label="Current posture" desc="Reported by guard_policy — the same source the Guard surface reads">
        <div className="flex items-center gap-2">
          {autonomy && (
            <Badge variant="outline" className="font-mono text-[10px]">
              {autonomy}
            </Badge>
          )}
          {policy && (
            <span className="font-mono text-[10px] text-muted-foreground">
              auto ≥ {Math.round((policy.minConfidenceForAuto ?? 0) * 100)}%
              {policy.estopPulled ? ' · estop pulled' : ''}
            </span>
          )}
          {!policy && !loading && !error && (
            <span className="font-mono text-[10px] text-muted-foreground">unknown</span>
          )}
          {loading && <span className="font-mono text-[10px] text-muted-foreground">loading…</span>}
        </div>
      </Row>

      <section className="rounded-md border border-border/50 bg-background/30 px-3 py-3">
        <div className="mb-2 flex items-center justify-between">
          <span className="text-xs font-medium text-foreground">Capability × scope</span>
          <Legend />
        </div>
        {inTauri() && !loading && matrix.length === 0 && !error ? (
          <p className="text-[11px] text-muted-foreground">
            The Guard service returned no matrix — nothing is assumed allowed.
          </p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full border-separate border-spacing-1 text-[10px]">
              <thead>
                <tr>
                  <th className="text-left font-normal text-muted-foreground/70" />
                  {SCOPES.map((s) => (
                    <th key={s} className="font-mono font-normal text-muted-foreground/70">
                      {s}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {CAPABILITIES.map((cap) => (
                  <tr key={cap}>
                    <td className="pr-2 font-mono text-muted-foreground">{cap}</td>
                    {SCOPES.map((scope) => {
                      const decision = cellFor(cap, scope)
                      return (
                        <td key={scope}>
                          <div
                            title={`${cap} · ${scope}${decision ? ` — ${decision}` : ' — not reported'}`}
                            className={cn(
                              'grid h-6 place-items-center rounded font-mono text-[9px]',
                              decision ? CELL_TONE[decision] : 'bg-zinc-800/40 text-zinc-500',
                            )}
                          >
                            {decision ?? '—'}
                          </div>
                        </td>
                      )
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      <section className="rounded-md border border-border/50 bg-background/30 px-3 py-3">
        <div className="mb-2 flex items-center justify-between">
          <span className="text-xs font-medium text-foreground">Tool allow-list</span>
          <Button
            size="sm"
            variant="outline"
            className="h-6 px-2 text-[10px]"
            onClick={() => setRules([...rules, { tool: '', approval: 'ask' }])}
          >
            + Rule
          </Button>
        </div>
        <p className="mb-2 text-[10px] leading-relaxed text-muted-foreground">
          Deny wins. Patterns like <code className="font-mono">fs.write</code> or{' '}
          <code className="font-mono">browser.*</code>, with an optional args glob (e.g.{' '}
          <code className="font-mono">rm -rf *</code>). Rules can only tighten the auto path.
        </p>
        {rules.length === 0 ? (
          <p className="text-[10px] text-muted-foreground/70">
            No policy rules are set — the built-in defaults apply.
          </p>
        ) : (
          <div className="space-y-1.5">
            {rules.map((r, i) => (
              <div key={i} className="flex items-center gap-1.5">
                <input
                  value={r.tool}
                  onChange={(e) => {
                    const next = [...rules]
                    next[i] = { ...next[i], tool: e.target.value }
                    setRules(next)
                  }}
                  placeholder="tool.pattern"
                  aria-label="Tool pattern"
                  className="h-6 min-w-0 flex-1 rounded-md border border-border bg-background/40 px-1.5 font-mono text-[10px] text-foreground"
                />
                <input
                  value={r.argsGlob ?? ''}
                  onChange={(e) => {
                    const next = [...rules]
                    next[i] = { ...next[i], argsGlob: e.target.value }
                    setRules(next)
                  }}
                  placeholder="args glob (optional)"
                  aria-label="Args glob"
                  className="h-6 min-w-0 flex-1 rounded-md border border-border bg-background/40 px-1.5 font-mono text-[10px] text-foreground"
                />
                <select
                  value={r.approval}
                  aria-label="Decision"
                  onChange={(e) => {
                    const next = [...rules]
                    next[i] = { ...next[i], approval: e.target.value as PolicyRule['approval'] }
                    setRules(next)
                  }}
                  className="h-6 rounded-md border border-border bg-background/40 px-1 font-mono text-[10px] text-foreground"
                >
                  <option value="allow">allow</option>
                  <option value="ask">ask</option>
                  <option value="deny">deny</option>
                </select>
                <button
                  type="button"
                  aria-label="Remove rule"
                  onClick={() => setRules(rules.filter((_, j) => j !== i))}
                  className="text-muted-foreground hover:text-foreground"
                >
                  ✕
                </button>
              </div>
            ))}
          </div>
        )}
        <Button
          size="sm"
          className="mt-2 h-6 gap-1 px-2 text-[10px]"
          disabled={rules.some((r) => !r.tool.trim())}
          onClick={() => void applyRules()}
        >
          <ShieldCheck className="h-3 w-3" /> Apply rules
        </Button>
      </section>

      <section className="rounded-md border border-border/50 bg-background/30 px-3 py-3">
        <div className="mb-1 flex items-center justify-between">
          <span className="text-xs font-medium text-foreground">One-click bundles</span>
          <Badge variant="outline" className="text-[9px]">guard_combos</Badge>
        </div>
        {combos.length === 0 ? (
          <p className="text-[10px] text-muted-foreground/70">
            {inTauri() ? 'No bundles reported by the Guard service.' : 'Bundles need the desktop shell.'}
          </p>
        ) : (
          <div className="space-y-1.5">
            {combos.map((c) => (
              <div key={c.name} className="flex items-start gap-2 rounded-md border border-border/60 bg-background/40 p-2">
                <div className="min-w-0 flex-1">
                  <div className="font-mono text-[10px] font-medium text-foreground">{c.name}</div>
                  <p className="mt-0.5 text-[10px] leading-relaxed text-muted-foreground">{c.description}</p>
                </div>
                <Button
                  size="sm"
                  variant="outline"
                  className="h-6 shrink-0 px-2 text-[10px]"
                  disabled={applying === c.name}
                  onClick={() => {
                    setApplying(c.name)
                    void (async () => {
                      try {
                        const n = await guardApplyCombo(c.name)
                        const [fresh, m] = await Promise.all([guardPolicy(), guardPermissionsMatrix()])
                        setPolicy(fresh)
                        setMatrix(m)
                        setRules(fresh?.approvalRules ?? rules)
                        notify(`Bundle ${c.name} applied (${n} rules)`)
                      } catch (e) {
                        notify(e instanceof Error ? e.message : 'Bundle apply failed', 'error')
                      } finally {
                        setApplying(null)
                      }
                    })()
                  }}
                >
                  {applying === c.name ? 'Applying…' : 'Apply'}
                </Button>
              </div>
            ))}
          </div>
        )}
      </section>
    </SectionShell>
  )
}

export function UsageSection() {
  const [snapshot, setSnapshot] = useState<UsageSnapshot | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    let alive = true
    void (async () => {
      try {
        const snap = await usageSnapshot()
        if (!alive) return
        setSnapshot(snap)
        setError(null)
      } catch (e) {
        if (!alive) return
        setSnapshot(null)
        setError(e instanceof Error ? e.message : 'Usage ledger is unavailable')
      } finally {
        if (alive) setLoading(false)
      }
    })()
    return () => {
      alive = false
    }
  }, [])

  // P71.4 — every figure here is an **observation**. Cost is shown as reported
  // when a producer priced its own turns, as our estimate from configured
  // prices when it did not, and as an honest dash when neither exists.
  const cost = snapshot ? costReadout(snapshot) : null
  const tokens = snapshot ? snapshot.total.tokensIn + snapshot.total.tokensOut : null
  const gaps = snapshot ? unreportedOwners(snapshot) : []

  return (
    <SectionShell
      title="Usage"
      desc="Token and spend observations read from the encrypted usage ledger — each figure names who reported it (wire: usage_snapshot)."
    >
      {error && (
        <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-[11px] text-red-300">
          Usage ledger is unavailable. {error}
        </div>
      )}
      <Row
        label={cost?.kind === 'reported' ? 'Total spent (reported)' : cost?.kind === 'estimated' ? 'Total spent (estimated)' : 'Total spent'}
        desc="Reported when an agent priced its own turns; otherwise AgentCowork's estimate from configured prices — never the two blended"
      >
        <span className="font-mono text-xs text-brand">
          {loading ? '…' : cost?.usd != null ? `$${cost.usd.toFixed(4)}` : '—'}
        </span>
      </Row>
      <Row label="Tokens" desc="Input + output reported across every recorded key">
        <span className="font-mono text-xs text-foreground/80">
          {loading ? '…' : tokens != null ? tokens.toLocaleString() : '—'}
        </span>
      </Row>
      {!loading && gaps.length > 0 && (
        <Row
          label="Turns with no usage report"
          desc="These turns are absent from the totals above — absent is not zero"
        >
          <span className="font-mono text-xs text-warning">
            {gaps.slice(0, 3).map(([owner, n]) => `${owner} ×${n}`).join(' · ')}
            {gaps.length > 3 ? ` · +${gaps.length - 3}` : ''}
          </span>
        </Row>
      )}
      <Row label="Prompt cache hit rate" desc="Cached / (cached + uncached) prompt tokens">
        <span className="font-mono text-xs text-foreground/80">
          {loading
            ? '…'
            : snapshot
              ? `${Math.round(snapshot.cacheHitRate * 100)}%`
              : '—'}
        </span>
      </Row>

      <section className="rounded-md border border-border/50 bg-background/30 px-3 py-3">
        <div className="mb-2 text-xs font-medium text-foreground">By provider key</div>
        {snapshot && snapshot.byKey.length === 0 && (
          <p className="text-[10px] text-muted-foreground/70">
            No key has recorded usage yet — send a turn with a configured provider.
          </p>
        )}
        {snapshot && snapshot.byKey.length > 0 && (
          <div className="space-y-1">
            {snapshot.byKey.map((k) => (
              <div key={k.key} className="flex items-center justify-between gap-3 font-mono text-[10px]">
                <span className="truncate text-muted-foreground">
                  {k.key}
                  <span className="ml-1 text-muted-foreground/60">
                    · {usageSourceLabel(k.source)}
                  </span>
                </span>
                <span className="shrink-0 text-foreground/80">
                  {(k.tokensIn + k.tokensOut).toLocaleString()} tok
                  {/* read/write cache split, labelled as reported — a read and a
                      write are billed differently and are never merged */}
                  {(k.cachedTokens > 0 || k.cachedWriteTokens > 0) && (
                    <span className="text-muted-foreground/70">
                      {' '}· cache {k.cachedTokens.toLocaleString()}
                      {k.cachedWriteTokens > 0 ? `+${k.cachedWriteTokens.toLocaleString()}w` : ''}
                    </span>
                  )}
                  {k.reportedCostUsd > 0
                    ? ` · $${k.reportedCostUsd.toFixed(4)} reported`
                    : k.costUsd != null
                      ? ` · $${k.costUsd.toFixed(4)} est`
                      : ''}
                </span>
              </div>
            ))}
          </div>
        )}
        {loading && <p className="text-[10px] text-muted-foreground/70">Loading live ledger…</p>}
      </section>

      {/* The per-session table owns its own fetch + honest empty/error state
          (P5.9); reuse it instead of a second spend view. */}
      <SessionsTable />

      <Row label="Pill visibility" desc="Which live pills the status-bar context meter may render">
        <PillToggles />
      </Row>
    </SectionShell>
  )
}

function PillToggles() {
  const pills = useAppStore((s) => s.statusBarPills)
  const setPills = useAppStore((s) => s.setStatusBarPills)
  return (
    <div className="flex items-center gap-3">
      {(
        [
          ['context', 'Context'],
          ['throughput', 'tok/s'],
          ['cache', 'Cache'],
          ['cost', 'Cost'],
        ] as const
      ).map(([key, label]) => (
        <label key={key} className="flex items-center gap-1.5 text-[10px] text-muted-foreground">
          <Switch
            checked={pills[key]}
            onCheckedChange={(v) => setPills({ ...pills, [key]: v })}
          />
          {label}
        </label>
      ))}
    </div>
  )
}
