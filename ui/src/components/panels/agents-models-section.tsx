'use client'

import { useEffect, useState } from 'react'
import {
  Boxes,
  Check,
  ChevronDown,
  ChevronRight,
  CircleDot,
  Cpu,
  Download,
  ExternalLink,
  Gauge,
  KeyRound,
  Layers,
  Loader2,
  RefreshCw,
  Route,
  Settings2,
  Terminal,
  Zap,
  X,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useAppStore } from '@/lib/store'
import {
  AGENTS,
  CAPABILITY_LABELS,
  ROUTING_BOUND_AGENT,
  TASK_LABELS,
  getModelsForAgentLive,
  isRuntimeUsable,
  type AgentRuntime,
  type TaskKind,
} from '@/lib/agents'
import { acpIdFor, acpInstallAwait, acpInstallCommit, acpInstallRequest, chiefSubagentSetPolicy } from '@/lib/acp'
import {
  agentBackendClear,
  agentBackendGet,
  agentBackendProbe,
  agentBackendProviders,
  agentBackendSet,
  channelLabel,
  hasLegacyVaultKeyRequest,
  type AgentBackendState,
  type AgentProviderRow,
  type ProviderProbeResult,
} from '@/lib/agent-backend'
import { refreshAgentCatalog } from '@/lib/bridge'
import { inTauri } from '@/lib/tauri'
import { cn } from '@/lib/utils'
import {
  agentAuthenticationLabel,
  assertNoAgentConfigWrite,
  hasHostLaunchOverride,
  nativeSurfaceNotReplaced,
  projectModelOwner,
  settingsAgentGet,
  type AgentSettings,
} from '@/lib/settings'
import { SectionShell } from './settings-shared'

function formatTokens(n: number): string {
  if (!n || n <= 0) return '—'
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`
  return String(n)
}

// === P63 — per-agent model backend ==========================================

/**
 * The compatibility disclosure: an optional credential-free host launch
 * override, such as a model or base URL.
 *
 * Authentication remains self-contained in the external agent. Nothing here
 * writes its config, delegates a host-vault key, or turns vault presence into
 * readiness. A non-secret override lasts only for the child launch; the panel
 * shows variable names, never values, and reports settings the agent cannot
 * express instead of silently dropping them.
 */
function AgentBackendPanel({ agentId }: { agentId: string }) {
  const notify = useAppStore((s) => s.notify)
  const [state, setState] = useState<AgentBackendState | null>(null)
  const [providers, setProviders] = useState<AgentProviderRow[]>([])
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState(false)
  const [checks, setChecks] = useState<Record<string, ProviderProbeResult>>({})

  const load = async () => {
    try {
      const [s, p] = await Promise.all([
        agentBackendGet(agentId),
        agentBackendProviders(agentId),
      ])
      setState(s)
      // Host-vault presence is inventory only, never an agent-auth or
      // readiness signal, so it must not rank or enable provider choices.
      setProviders(p)
    } catch (e) {
      notify(e instanceof Error ? e.message : 'Could not read this agent\u2019s backend', 'error')
    }
  }

  useEffect(() => {
    void load()
    // Re-read when the agent id changes (the card is reused across rows).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentId])

  const choose = async (row: AgentProviderRow) => {
    setBusy(true)
    try {
      const next = await agentBackendSet({
        agentId,
        provider: row.id,
      })
      setState(next)
      notify(
        `${row.name} host override set — the agent keeps its own sign-in and credentials`,
      )
    } catch (e) {
      notify(e instanceof Error ? e.message : 'Could not set the provider', 'error')
    } finally {
      setBusy(false)
    }
  }

  const clear = async () => {
    setBusy(true)
    const wasLegacyRequest = state?.configured?.useVaultKey === true
    try {
      setState(await agentBackendClear(agentId))
      notify(
        wasLegacyRequest
          ? `Legacy host-vault request cleared — sign in or configure ${agentId} in its own interface`
          : `${agentId} host override cleared — the agent returns to its own configuration`,
      )
    } catch (e) {
      notify(e instanceof Error ? e.message : 'Could not clear the binding', 'error')
    } finally {
      setBusy(false)
    }
  }

  const test = async (row: AgentProviderRow) => {
    try {
      const r = await agentBackendProbe(row.id)
      setChecks((c) => ({ ...c, [row.id]: r }))
      notify(
        r.ok
          ? `${row.name}: reachable (${r.models} models)`
          : `${row.name}: not reachable \u2014 ${r.message}`,
        r.ok ? 'default' : 'error',
      )
    } catch (e) {
      notify(e instanceof Error ? e.message : 'Probe failed', 'error')
    }
  }

  if (!state) {
    return (
      <div className="mt-2 flex items-center gap-2 rounded-md border border-border/40 bg-background/30 px-2 py-2 text-[10px] text-muted-foreground">
        <Loader2 className="h-3 w-3 animate-spin" /> reading this agent’s backend…
      </div>
    )
  }

  const q = query.trim().toLowerCase()
  const visible = (q
    ? providers.filter(
        (p) => p.id.toLowerCase().includes(q) || p.name.toLowerCase().includes(q),
      )
    : providers
  ).slice(0, 8)
  const legacyVaultKeyRequest = hasLegacyVaultKeyRequest(state)

  return (
    <div
      className="mt-2 rounded-md border border-border/50 bg-background/40 p-2"
      data-testid={`agent-backend-${agentId}`}
    >
      <div className="flex items-center gap-1.5 font-mono text-[9px] uppercase tracking-wider text-muted-foreground">
        <KeyRound className="h-2.5 w-2.5" />
        {channelLabel(state.channel)}
        {state.configFile && (
          <span className="ml-auto truncate normal-case text-muted-foreground/70">
            {state.configFile}
          </span>
        )}
      </div>

      <p className="mt-1 text-[10px] leading-relaxed text-muted-foreground">
        {state.note}
      </p>
      <p className="mt-1 text-[10px] leading-relaxed text-muted-foreground">
        Authentication: agent-owned / self-contained. Host-vault injection: unavailable.
      </p>

        {state.configured && (
          <div
            className={cn(
              'mt-2 rounded border px-2 py-1.5 text-[10px]',
              legacyVaultKeyRequest
                ? 'border-warning/40 bg-warning/5 text-warning'
                : 'border-emerald-500/20 bg-emerald-500/5 text-emerald-100/90',
            )}
          >
            <div className="flex items-center gap-1.5">
              {legacyVaultKeyRequest ? (
                <KeyRound className="h-2.5 w-2.5" />
              ) : (
                <Check className="h-2.5 w-2.5 text-emerald-300" />
              )}
              <span className="font-mono">
                {legacyVaultKeyRequest && 'legacy host-vault request · '}
                {state.configured.provider}
                {state.configured.model ? ` \u00b7 ${state.configured.model}` : ''}
              </span>
              <Button
                size="sm"
                variant="ghost"
                className="ml-auto h-5 px-1 text-[9px]"
                aria-label={
                  legacyVaultKeyRequest
                    ? `Clear legacy host-vault request for ${agentId}`
                    : `Clear host launch override for ${agentId}`
                }
                disabled={busy}
                onClick={() => void clear()}
              >
                <X className="h-2.5 w-2.5" /> {legacyVaultKeyRequest ? 'clear legacy' : 'clear'}
              </Button>
            </div>
            {legacyVaultKeyRequest ? (
              <div className="mt-0.5">
                No host vault key was or will be injected. Clear this legacy record, then sign in
                or configure {agentId} in its own interface.
              </div>
            ) : (
              <>
                <div className="mt-0.5 text-emerald-100/70">
                  {state.injectedEnv.length > 0
                    ? `Credential-free host launch inputs: ${state.injectedEnv.join(', ')}`
                    : 'This choice adds no non-secret host launch input; the agent remains model-owned.'}
                </div>
                {state.unexpressed.length > 0 && (
                  <div className="text-warning/80">
                    Not expressible by this agent: {state.unexpressed.join(', ')}
                  </div>
                )}
                <div className="mt-0.5 text-emerald-100/50">
                  No credential is copied, and nothing is written to this agent’s own config file.
                </div>
              </>
            )}
          </div>
        )}

        {state.refusal && (
          <div className="mt-2 text-[10px] text-warning/80">{state.refusal}</div>
        )}

        {state.injectable ? (
          <>
            <input
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search host override providers…"
              aria-label={`Credential-free host override provider for ${agentId}`}
              className="mt-2 h-7 w-full rounded border border-border bg-background px-2 font-mono text-[10px] text-foreground placeholder:text-muted-foreground/60"
            />

            <div className="mt-1.5 space-y-1">
            {visible.length === 0 && (
              <div className="px-1 text-[10px] text-muted-foreground">
                No provider matches “{query}”.
              </div>
            )}
            {visible.map((p) => {
              const check = checks[p.id]
              const active = state.configured?.provider === p.id
              return (
                <div
                  key={p.id}
                  className={cn(
                    'flex items-center gap-1.5 rounded border px-1.5 py-1 text-[10px]',
                    active
                      ? 'border-emerald-500/40 bg-emerald-500/5'
                      : 'border-border/40 bg-background/30',
                  )}
                >
                  <span className="truncate text-foreground">{p.name}</span>
                  {p.env && (
                    <span
                      className="truncate font-mono text-[8px] text-muted-foreground/70"
                      title="Agent-owned provider convention; not a host-vault credential"
                    >
                      {p.env}
                    </span>
                  )}
                  {p.local && (
                    <Badge className="bg-blue-500/15 text-[8px] text-blue-300">local</Badge>
                  )}
                  {check && (
                    <Badge
                      className={cn(
                        'text-[8px]',
                        check.ok
                          ? 'bg-emerald-500/15 text-emerald-300'
                          : 'bg-rose-500/15 text-rose-300',
                      )}
                    >
                      {check.ok ? `healthy \u00b7 ${check.models}` : 'unreachable'}
                    </Badge>
                  )}
                  {!check && p.verifiedAt && (
                    <Badge className="bg-emerald-500/10 text-[8px] text-emerald-300/80">
                      verified
                    </Badge>
                  )}
                  <Button
                    size="sm"
                    variant="ghost"
                    className="ml-auto h-5 px-1 text-[9px]"
                    aria-label={`Test provider endpoint reachability for ${p.name}`}
                    onClick={() => void test(p)}
                  >
                    <Gauge className="h-2.5 w-2.5" /> test
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    className="h-5 px-1.5 text-[9px]"
                    aria-label={`${active ? 'Using' : 'Use'} credential-free host override for ${p.name}`}
                    disabled={busy || active}
                    onClick={() => void choose(p)}
                  >
                    {active ? 'in use' : 'use'}
                  </Button>
                </div>
              )
            })}
          </div>
        </>
      ) : (
        <p className="mt-1 text-[10px] leading-relaxed text-muted-foreground">
          {state.refusal && !state.configured ? state.refusal : state.note}
          {state.channel === 'config_file' && state.configFile
            ? ` (${state.configFile})`
            : ''}
        </p>
      )}
    </div>
  )
}

// === Agent card ===============================================================

function StatusBadge({ status }: { status: AgentRuntime['status'] }) {
  const tone =
    status === 'installed'
      ? 'bg-emerald-500/15 text-emerald-300'
      : status === 'discovered'
        ? 'bg-sky-500/15 text-sky-300'
        : status === 'updating'
          ? 'bg-blue-500/15 text-blue-300'
          : status === 'disabled'
            ? 'bg-rose-500/15 text-rose-300'
            : 'bg-zinc-500/15 text-zinc-400'
  // P55.4/P66 — catalog membership is not occupancy. `discovered` means a
  // verified location exists; it does not necessarily mean the current
  // adapter can launch it (for example, WSL before the WSL adapter lands).
  const label = status === 'available' ? 'not installed' : status
  return (
    <Badge className={cn('text-[9px] capitalize', tone)}>
      <CircleDot className="h-2.5 w-2.5" />
      {label}
    </Badge>
  )
}

function AgentLogo({ agent }: { agent: AgentRuntime }) {
  return (
    <span
      className={cn(
        'flex h-9 w-9 items-center justify-center rounded-md font-mono text-[13px] font-bold',
        agent.accent,
      )}
    >
      {agent.mark}
    </span>
  )
}

// === P65.2 — dual-card agent detail ===========================================
// Native capabilities (owned by the agent itself) vs shared cowork (AgentCowork
// grants that apply from the next turn). Never copy an external agent's keys
// into AgentCowork config surfaces — this detail shows env *names* only, never
// values, and external model picks stay inside the agent's own config.

type AgentReadiness = 'ready' | 'degraded' | 'unavailable' | 'unverified'

/** Installed is not Ready. A catalog row with no executable, package, or WSL path cannot serve a turn. */
function agentHasProgram(agent: AgentRuntime): boolean {
  if (agent.path?.trim()) return true
  const location = agent.location
  if (!location || location.kind === 'unavailable') return false
  if (location.kind === 'path' || location.kind === 'windows_path') return Boolean(location.executable)
  if (location.kind === 'wsl') return Boolean(location.linuxPath)
  if (location.kind === 'package_manager') return Boolean(location.package || location.command)
  if (location.kind === 'managed') return Boolean(location.executable)
  return false
}

function agentReadiness(agent: AgentRuntime): { state: AgentReadiness; reason: string } {
  // P71.2a — no runtime is "always live": readiness is the only evidence a
  // runtime can serve a turn, built-in or not.
  if (agent.status === 'disabled') {
    return { state: 'unavailable', reason: 'disabled on this machine' }
  }
  if (agent.status === 'updating') {
    return { state: 'degraded', reason: 'update in progress — launch may fail' }
  }
  const launchable = agent.launchable ?? agent.status === 'installed'
  const hasProgram = agentHasProgram(agent)
  if (agent.status === 'installed' && launchable && !hasProgram) {
    return { state: 'unverified', reason: 'no program path on this machine — not ready' }
  }
  if (agent.status === 'installed' && launchable && hasProgram) {
    return { state: 'ready', reason: 'verified launch path on this machine' }
  }
  if (agent.status === 'installed' && !launchable) {
    return { state: 'degraded', reason: 'installed but the current adapter cannot launch it yet' }
  }
  if (agent.status === 'discovered') {
    // WSL-only is the canonical case: a verified location exists but the
    // current adapter cannot start it — never send a Linux path to CreateProcess.
    if (agent.location?.kind === 'wsl') {
      return { state: 'unavailable', reason: `WSL-only (${agent.location.distro}) — needs the WSL adapter` }
    }
    return { state: 'unverified', reason: 'location verified, launch not confirmed' }
  }
  return { state: 'unavailable', reason: 'not installed — catalog entry only' }
}

function readinessTone(state: AgentReadiness): string {
  switch (state) {
    case 'ready':
      return 'bg-emerald-500/15 text-emerald-300'
    case 'degraded':
      return 'bg-warning/15 text-warning'
    case 'unverified':
      return 'bg-sky-500/15 text-sky-300'
    case 'unavailable':
    default:
      return 'bg-zinc-500/15 text-zinc-400'
  }
}

function AuthOwnerBadge({ agent }: { agent: AgentRuntime }) {
  // Authentication and the baseline model stay with the external agent. A
  // credential-free host launch override, when one exists, is named separately
  // in the detail rather than being called ownership of the agent's model.
  return (
    <Badge
      className="bg-zinc-500/15 text-zinc-300 text-[9px]"
      title={`${agent.name} owns its authentication, credentials, and baseline model; the host never injects a vault-held provider key`}
    >
      {`auth owner: ${agent.name}`}
    </Badge>
  )
}

function AgentDetailCards({ agent }: { agent: AgentRuntime }) {
  const acpOptions = useAppStore((s) => s.acpConfigOptions[agent.id])
  const [live, setLive] = useState<AgentSettings | null>(null)
  useEffect(() => {
    let cancelled = false
    void settingsAgentGet(agent.id)
      .then((row) => {
        if (!cancelled) setLive(row)
      })
      .catch(() => {
        if (!cancelled) setLive(null)
      })
    return () => {
      cancelled = true
    }
  }, [agent.id])
  const effectiveModelOwner = live ? projectModelOwner(live) : 'agent'
  const hostLaunchOverride = hasHostLaunchOverride(live?.backendBinding)
  return (
    <div className="mt-2 grid gap-2 sm:grid-cols-2 [contain-intrinsic-size:auto_120px]">
      {/* Native capabilities — owned by the agent itself. */}
      <div className="rounded-md border border-border/50 bg-background/30 p-2">
        <div className="flex items-center gap-1.5">
          <span className="text-[10px] font-medium text-foreground">Agent capabilities</span>
          <Badge variant="outline" className="text-[8px] text-muted-foreground">
            {`owned by ${agent.name}`}
          </Badge>
        </div>
        <p className="mt-1 text-[10px] leading-relaxed text-muted-foreground">
          What this runtime itself exposes. Authentication is self-contained; model, sign-in, and
          routing stay in the agent and are observed here only by reference.
        </p>
        <div className="mt-1.5 flex flex-wrap gap-1">
          {((live && nativeSurfaceNotReplaced(live) && live.nativeCapabilities?.length
            ? live.nativeCapabilities
            : agent.capabilities) ?? []
          ).map((c) => (
            <Badge key={c} variant="secondary" className="bg-background/60 text-[8px] font-normal text-muted-foreground">
              {CAPABILITY_LABELS[c as keyof typeof CAPABILITY_LABELS] ?? c}
            </Badge>
          ))}
        </div>
        <dl className="mt-1.5 space-y-0.5 font-mono text-[9px] text-muted-foreground">
          <div className="flex justify-between gap-2">
            <dt>ACP readiness</dt>
            <dd className="text-foreground/80">
              {live?.readiness ?? 'unknown — live agent state unavailable'}
            </dd>
          </div>
          <div className="flex justify-between gap-2">
            <dt>authentication</dt>
            <dd className="text-foreground/80">{agentAuthenticationLabel(live?.authMode)}</dd>
          </div>
          {live?.backendBinding && (
            <div className="flex justify-between gap-2">
              <dt>writesToAgentConfig</dt>
              <dd className="text-foreground/80">
                {(() => {
                  assertNoAgentConfigWrite(live.backendBinding)
                  return String(live.backendBinding.writesToAgentConfig)
                })()}
              </dd>
            </div>
          )}
          <div className="flex justify-between gap-2">
            <dt>launch</dt>
            <dd className="text-foreground/80">
              {(agent.launchable ?? agent.status === 'installed') ? 'launchable' : 'not launchable'}
              {agent.location ? ` · ${agent.location.kind}/${agent.location.source.replaceAll('_', ' ')}` : ''}
            </dd>
          </div>
          <div className="flex justify-between gap-2">
            <dt>models</dt>
            <dd className="text-foreground/80">
              {acpOptions?.length
                ? `${acpOptions.length} agent-owned options`
                : effectiveModelOwner === 'managed' && hostLaunchOverride
                  ? `host override · ${live?.backendBinding?.injectedEnvNames.length ?? 0} non-secret inputs`
                  : 'agent-owned · no host override'}
            </dd>
          </div>
        </dl>
        <p className="mt-1.5 rounded border border-border/40 bg-background/40 px-1.5 py-1 text-[9px] leading-relaxed text-muted-foreground">
          Authentication stays in the agent&apos;s own sign-in. AgentCowork can add only
          credential-free launch inputs such as model or base URL; it never copies or injects a
          host-vault credential.
        </p>
      </div>
      {/* Shared cowork — AgentCowork grants, next-turn only. */}
      <div className="rounded-md border border-border/50 bg-background/30 p-2">
        <div className="flex items-center gap-1.5">
          <span className="text-[10px] font-medium text-foreground">Shared cowork</span>
          <Badge variant="outline" className="text-[8px] text-muted-foreground">AgentCowork grants</Badge>
        </div>
        <p className="mt-1 text-[10px] leading-relaxed text-muted-foreground">
          Office, Browser, Computer Use, connectors, memory, and MCP for this runtime.
          Changes apply from the next turn and freeze into the Work manifest — never into an in-flight run.
        </p>
        <ul className="mt-1.5 space-y-0.5">
          {[
            { label: 'Office (xlsx/docx/pptx/pdf)', hint: 'surgical patches' },
            { label: 'Browser (tiered)', hint: 'Lightpanda → Chrome CDP' },
            { label: 'Computer use', hint: 'guarded desktop' },
            { label: 'MCP + connectors', hint: 'chat loadout' },
            { label: 'Memory (5-tier)', hint: 'scoped recall' },
          ].map((r) => (
            <li key={r.label} className="flex items-center justify-between gap-2 text-[9px]">
              <span className="text-foreground/80">{r.label}</span>
              <span className="font-mono text-muted-foreground/70">{r.hint}</span>
            </li>
          ))}
        </ul>
        <p className="mt-1.5 font-mono text-[8px] text-muted-foreground/70">
          Capability resolution is native-first, augmentation-second. Status is unverified until a
          real Windows acceptance record exists.
        </p>
      </div>
    </div>
  )
}

// === P63.12 — external subagent roster card =====================================
// One configuration card per external CLI agent (Codex CLI, Claude Code,
// OpenCode, Aider, …): dual role toggles, specialist domain tags, a
// concurrency cap, and per-turn budget ceilings. This card only persists roster
// policy through `chief_subagent_set_policy` — there is no mock loop here and
// no live `delegate.spawn` path. Admission stays with the delegation policy on
// the Work gateway (the scheduler never enforces these caps), and only a Ready
// runtime may be hired.

const SUBAGENT_DOMAINS = ['coding', 'architecture', 'research', 'scraping', 'office'] as const
type SubagentDomain = (typeof SUBAGENT_DOMAINS)[number]

function formatCentsPerTurn(cents: number): string {
  if (!cents || cents <= 0) return 'chain cap'
  return `$${(cents / 100).toFixed(2)} / turn`
}

function SubagentConfigCard({ agent }: { agent: AgentRuntime }) {
  const notify = useAppStore((s) => s.notify)
  const [allowAsPrimary, setAllowAsPrimary] = useState(true)
  const [enableAsSubagent, setEnableAsSubagent] = useState(true)
  const [domains, setDomains] = useState<SubagentDomain[]>([])
  const [maxConcurrency, setMaxConcurrency] = useState(2)
  const [maxCentsPerTurn, setMaxCentsPerTurn] = useState(0)
  const [maxTokensPerTurn, setMaxTokensPerTurn] = useState(0)
  const [saving, setSaving] = useState(false)
  const [savedAt, setSavedAt] = useState<string | null>(null)
  const readiness = agentReadiness(agent)
  const hireable = readiness.state === 'ready'

  const roleSummary =
    allowAsPrimary && enableAsSubagent
      ? 'primary + subagent'
      : allowAsPrimary
        ? 'primary only'
        : enableAsSubagent
          ? 'subagent only'
          : 'disabled'

  const toggleDomain = (tag: SubagentDomain) => {
    setSavedAt(null)
    setDomains((d) => (d.includes(tag) ? d.filter((t) => t !== tag) : [...d, tag]))
  }

  const markDirty = () => setSavedAt(null)

  const save = async () => {
    if (!inTauri()) {
      notify('Subagent roster persists in the desktop shell — preview edits are not saved', 'error')
      return
    }
    setSaving(true)
    try {
      await chiefSubagentSetPolicy(agent.id, {
        allowAsPrimary,
        enableAsSubagent,
        domains: [...domains],
        maxConcurrency: Math.min(8, Math.max(1, Math.floor(maxConcurrency) || 2)),
        maxCentsPerTurn: Math.max(0, Math.floor(maxCentsPerTurn) || 0),
        maxTokensPerTurn: Math.max(0, Math.floor(maxTokensPerTurn) || 0),
      })
      setSavedAt(new Date().toLocaleTimeString())
      notify(`Subagent roster saved for ${agent.name} (${roleSummary})`)
    } catch (e) {
      notify(e instanceof Error ? e.message : `Could not save subagent config for ${agent.name}`, 'error')
    } finally {
      setSaving(false)
    }
  }

  return (
    <div
      className="mt-2 rounded-md border border-border/50 bg-background/40 p-2"
      data-testid={`subagent-config-${agent.id}`}
    >
      <div className="flex items-center gap-1.5 font-mono text-[9px] uppercase tracking-wider text-muted-foreground">
        <Boxes className="h-2.5 w-2.5" />
        Subagent roster
        <span
          className={cn(
            'ml-auto truncate normal-case',
            hireable ? 'text-emerald-300/80' : 'text-muted-foreground/70',
          )}
          title={readiness.reason}
        >
          {hireable ? 'Ready — hireable via delegate.spawn' : `${readiness.state} — not hireable until Ready`}
        </span>
      </div>

      <p className="mt-1 text-[10px] leading-relaxed text-muted-foreground">
        How this external CLI may serve: as the session&apos;s primary binding, as a hired
        specialist, or both. Caps are enforced by the delegation policy on the Work gateway —
        the scheduler never enforces them.
      </p>

      <div className="mt-2 grid gap-1.5 sm:grid-cols-2">
        <label className="flex cursor-pointer items-center gap-2 rounded border border-border/40 bg-background/30 px-2 py-1.5">
          <Switch
            checked={allowAsPrimary}
            onCheckedChange={(v) => {
              setAllowAsPrimary(v)
              markDirty()
            }}
            aria-label={`Allow ${agent.name} as primary`}
          />
          <span className="text-[10px]">
            <span className="font-medium text-foreground">Allow as primary</span>
            <br />
            <span className="text-muted-foreground">may occupy the primary slot</span>
          </span>
        </label>
        <label className="flex cursor-pointer items-center gap-2 rounded border border-border/40 bg-background/30 px-2 py-1.5">
          <Switch
            checked={enableAsSubagent}
            onCheckedChange={(v) => {
              setEnableAsSubagent(v)
              markDirty()
            }}
            aria-label={`Enable ${agent.name} as subagent`}
          />
          <span className="text-[10px]">
            <span className="font-medium text-foreground">Enable as subagent</span>
            <br />
            <span className="text-muted-foreground">may be hired through delegate.spawn</span>
          </span>
        </label>
      </div>
      <div className="mt-1 font-mono text-[9px] text-muted-foreground/70">
        effective role: <span className="text-foreground/80">{roleSummary}</span>
      </div>

      <div className="mt-2">
        <div className="font-mono text-[9px] uppercase tracking-wider text-muted-foreground">
          Specialist domains
        </div>
        <div className="mt-1 flex flex-wrap gap-1">
          {SUBAGENT_DOMAINS.map((tag) => {
            const on = domains.includes(tag)
            return (
              <button
                key={tag}
                type="button"
                aria-pressed={on}
                data-testid={`subagent-domain-${agent.id}-${tag}`}
                onClick={() => toggleDomain(tag)}
                className={cn(
                  'rounded border px-1.5 py-0.5 font-mono text-[10px] capitalize transition-colors',
                  on
                    ? 'border-sky-500/50 bg-sky-500/10 text-sky-200'
                    : 'border-border/60 text-muted-foreground hover:border-border hover:text-foreground',
                )}
              >
                {tag}
              </button>
            )
          })}
        </div>
        <p className="mt-1 text-[9px] leading-relaxed text-muted-foreground/80">
          When the primary omits a target, the one Ready agent with that domain is admitted —
          zero or several matches is a denial with a reason, never a silent swap.
        </p>
      </div>

      <div className="mt-2 grid grid-cols-3 gap-1.5">
        <label className="text-[10px] text-muted-foreground">
          Max concurrency
          <input
            type="number"
            min={1}
            max={8}
            value={maxConcurrency}
            aria-label={`Max concurrent instances for ${agent.id}`}
            onChange={(e) => {
              markDirty()
              setMaxConcurrency(Math.min(8, Math.max(1, Math.floor(Number(e.target.value)) || 1)))
            }}
            className="mt-0.5 h-7 w-full rounded border border-border bg-background px-1.5 font-mono text-[10px] text-foreground"
          />
        </label>
        <label className="text-[10px] text-muted-foreground">
          Max cents / turn
          <input
            type="number"
            min={0}
            value={maxCentsPerTurn}
            aria-label={`Max cents per turn for ${agent.id}`}
            onChange={(e) => {
              markDirty()
              setMaxCentsPerTurn(Math.max(0, Math.floor(Number(e.target.value)) || 0))
            }}
            className="mt-0.5 h-7 w-full rounded border border-border bg-background px-1.5 font-mono text-[10px] text-foreground"
          />
        </label>
        <label className="text-[10px] text-muted-foreground">
          Max tokens / turn
          <input
            type="number"
            min={0}
            value={maxTokensPerTurn}
            aria-label={`Max tokens per turn for ${agent.id}`}
            onChange={(e) => {
              markDirty()
              setMaxTokensPerTurn(Math.max(0, Math.floor(Number(e.target.value)) || 0))
            }}
            className="mt-0.5 h-7 w-full rounded border border-border bg-background px-1.5 font-mono text-[10px] text-foreground"
          />
        </label>
      </div>
      <p className="mt-1 text-[9px] leading-relaxed text-muted-foreground/80">
        Budget ceiling: {formatCentsPerTurn(maxCentsPerTurn)}
        {maxTokensPerTurn > 0 ? ` · ${formatTokens(maxTokensPerTurn)} tokens` : ' · no token cap'}. A
        0 budget means the chain cap, never a free pass past Guard. Excess delegation requests
        queue on the child Work or return backpressure.
      </p>

      <p className="mt-1.5 rounded border border-border/40 bg-background/30 px-2 py-1 text-[9px] leading-relaxed text-muted-foreground">
        Sandbox: coding specialists run worktree-isolated (ephemeral, branched off the current
        branch via <span className="font-mono">agentcowork-core worktrees</span>) — no direct
        mutation of the primary branch without review and an explicit merge.
      </p>

      <div className="mt-2 flex items-center gap-2">
        <Button
          size="sm"
          className="h-6 px-2 text-[10px]"
          disabled={saving}
          data-testid={`subagent-save-${agent.id}`}
          onClick={() => void save()}
        >
          {saving ? <Loader2 className="h-3 w-3 animate-spin" /> : <Check className="h-3 w-3" />}
          {saving ? 'Saving…' : 'Save roster entry'}
        </Button>
        {savedAt && (
          <span className="font-mono text-[9px] text-emerald-300/80">saved {savedAt}</span>
        )}
      </div>
      {!hireable && (
        <p className="mt-1 text-[9px] text-muted-foreground/80">
          Roster saves once this CLI is installed — the host refuses unknown or uninstalled ids,
          and only a Ready runtime may be hired ({readiness.reason}).
        </p>
      )}
      <p className="mt-1 font-mono text-[8px] text-muted-foreground/70">
        Settings card only: the live hire (child Work, worktree, chat badge inside the
        parent&apos;s tool drawer) is not yet a path a person can finish.
      </p>
    </div>
  )
}

function AgentCard({
  agent,
  catalogExpanded,
  onToggleCatalog,
}: {
  agent: AgentRuntime
  /** Native only — the model catalog is a disclosure on the Native card. */
  catalogExpanded?: boolean
  onToggleCatalog?: () => void
}) {
  const selectedAgentId = useAppStore((s) => s.selectedAgentId)
  const setSelectedAgent = useAppStore((s) => s.setSelectedAgent)
  const notify = useAppStore((s) => s.notify)
  const isSelected = selectedAgentId === agent.id
  // Live-gated: stub model counts for uninstalled runtimes are never shown —
  // the list loads live only after install.
  const liveAgents = useAppStore((s) => s.liveAgents)
  const liveSource = liveAgents.length > 0 ? liveAgents : undefined
  const models = getModelsForAgentLive(agent.id, liveSource)
  const usable = isRuntimeUsable(
    liveSource?.find((a) => a.id === agent.id) ?? agent,
  )
  // P60/P71.2d — ownership. Every CLI owns its own model and exposes it (if at
  // all) over ACP config options; AgentCowork owns no model surface to show.
  const acpOptions = useAppStore((s) => s.acpConfigOptions[agent.id])
  const [busyInstall, setBusyInstall] = useState(false)
  const [busyScan, setBusyScan] = useState(false)
  // P63 — the legacy per-agent host override is a disclosure, not a permanent
  // control on every runtime row. It never carries a credential.
  const [configOpen, setConfigOpen] = useState(false)
  // P65.2 — dual-card detail is a disclosure (open by default for the active
  // runtime so its ownership boundary is visible without a click).
  const [detailOpen, setDetailOpen] = useState(isSelected)
  // P63.12 — the subagent roster entry is a disclosure per external CLI row.
  const [subagentOpen, setSubagentOpen] = useState(false)
  const readiness = agentReadiness(agent)

  // Re-run discovery (ACP registry + install status + PATH probe) so the
  // row reflects what is actually on this machine right now.
  const rescan = async () => {
    setBusyScan(true)
    try {
      await refreshAgentCatalog()
      const live = useAppStore.getState().liveAgents
      const installed = live.filter((a) => a.status === 'installed').length
      notify(`Discovery re-scan: ${live.length} runtimes, ${installed} installed`)
    } catch (e) {
      notify(e instanceof Error ? e.message : 'Discovery re-scan failed', 'error')
    } finally {
      setBusyScan(false)
    }
  }

  // F8 — real plan-before-touch install (Guard-2 ticket), same as the
  // composer picker. Registry ids, not catalog ids (claude-code → claude).
  const installAgent = async () => {
    setBusyInstall(true)
    try {
      const rid = acpIdFor(agent.id)
      const req = await acpInstallRequest(rid)
      const licenseNote = req.license ? ` · license: ${req.license}` : ''
      const cmd = (req.exactCommand ?? []).join(' ')
      if (req.consentRequired && cmd) {
        const ok = window.confirm(
          `SEP-1024 exact-command consent\n\nInstall ${agent.name}?${licenseNote}\n\n${cmd}${req.preferNative ? '\n\nPrefer verified native artifact.' : ''}`,
        )
        if (!ok) {
          notify('Install cancelled')
          return
        }
      }
      if (req.action === 'allow') {
        await acpInstallCommit(rid, req.ticketId)
        notify(`${agent.name} installed — runtimes re-scanned`)
        await rescan()
      } else {
        // P69.C12 — wait for the one explicit consent, then commit (or show
        // the honest refusal), never a silent stall.
        notify(
          `Consent needed${licenseNote} — ${req.reason ?? 'proprietary agent'}; approve the Guard-2 card #${req.ticketId.slice(0, 8)}`,
        )
        const { approved, reason } = await acpInstallAwait(req.ticketId)
        if (!approved) {
          notify(reason ?? 'Install not approved', 'error')
          return
        }
        await acpInstallCommit(rid, req.ticketId)
        notify(`${agent.name} installed — runtimes re-scanned`)
        await rescan()
      }
    } catch (e) {
      notify(e instanceof Error ? e.message : `Installing ${agent.name} failed`, 'error')
    } finally {
      setBusyInstall(false)
    }
  }

  return (
    <div
      className={cn(
        'rounded-lg border bg-background/40 p-3 transition-all hover-lift border-glow',
        isSelected ? 'border-sky-500/60 bg-sky-500/5 gradient-border' : 'border-border/60 hover:border-border',
      )}
    >
      <div className="flex items-start gap-2.5">
        <AgentLogo agent={agent} />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="text-[13px] font-semibold text-foreground">{agent.name}</span>
            <StatusBadge status={agent.status} />
            <Badge className={cn('text-[9px]', readinessTone(readiness.state))} aria-label={`Readiness: ${readiness.state} — ${readiness.reason}`}>
              {readiness.state}
            </Badge>
            <AuthOwnerBadge agent={agent} />
            {isSelected && (
              <Badge className="bg-sky-500/20 text-[9px] text-sky-300">active</Badge>
            )}
          </div>
          <div className="mt-0.5 font-mono text-[10px] text-muted-foreground">
            {agent.vendor} · v{agent.version ?? '—'} · {agent.path ?? 'no path'}
          </div>
          {agent.location && (
            <div className="mt-0.5 flex flex-wrap gap-x-2 text-[9px] text-muted-foreground/70">
              <span>source: {agent.location.source.replaceAll('_', ' ')}</span>
              <span>location: {agent.location.kind}</span>
              {agent.location.kind === 'wsl' && <span>distro: {agent.location.distro}</span>}
            </div>
          )}
          <p className="mt-1 text-[11px] text-muted-foreground">{agent.tagline}</p>
        </div>
      </div>

      <div className="mt-2 flex flex-wrap gap-1">
        {agent.capabilities.map((c) => (
          <Badge
            key={c}
            variant="secondary"
            className="bg-background/60 text-[8px] font-normal text-muted-foreground"
          >
            {CAPABILITY_LABELS[c]}
          </Badge>
        ))}
      </div>

      <div className="mt-2 flex items-center gap-2 font-mono text-[10px] text-muted-foreground">
        <Layers className="h-3 w-3" />
        <span>
          {usable
            ? acpOptions?.length
              ? `agent-owned · ${acpOptions.length} options`
              : 'agent-owned · own config'
            : 'models on install'}
        </span>
        <span className="text-muted-foreground/30">|</span>
        <Terminal className="h-3 w-3" />
        <span className={cn(agent.headless ? 'text-emerald-300' : 'text-warning')}>
          {agent.headless ? 'headless' : 'needs UI'}
        </span>
        <span className="text-muted-foreground/30">|</span>
        <Settings2 className="h-3 w-3" />
        <span
          className={cn(
            agent.sandbox === 'strict'
              ? 'text-emerald-300'
              : agent.sandbox === 'soft'
                ? 'text-warning'
                : 'text-rose-300',
          )}
        >
          sandbox: {agent.sandbox}
        </span>
      </div>

      {agent.note && (
        <p className="mt-2 rounded border border-border/40 bg-background/30 px-2 py-1 text-[10px] text-muted-foreground/90">
          {agent.note}
        </p>
      )}

      {detailOpen && <AgentDetailCards agent={agent} />}

      {subagentOpen && <SubagentConfigCard agent={agent} />}

      {configOpen && usable && <AgentBackendPanel agentId={agent.id} />}

      <div className="mt-2.5 flex items-center gap-1">
        {usable ? (
          <Button
            size="sm"
            variant={isSelected ? 'default' : 'outline'}
            className={cn(
              'h-7 px-2 text-[10px]',
              isSelected && 'bg-sky-500 text-black hover:bg-sky-400',
            )}
            onClick={() => setSelectedAgent(agent.id)}
            disabled={isSelected}
          >
            {isSelected ? (
              <>
                <Check className="h-3 w-3" />
                Selected
              </>
            ) : (
              'Use runtime'
            )}
          </Button>
        ) : (
          <Button
            size="sm"
            variant="outline"
            className="h-7 px-2 text-[10px]"
            disabled={busyInstall}
            onClick={() => void installAgent()}
          >
            {busyInstall ? <Loader2 className="h-3 w-3 animate-spin" /> : <Download className="h-3 w-3" />}
            {busyInstall ? 'installing…' : 'Install'}
          </Button>
        )}
        {onToggleCatalog && (
          <Button
            size="sm"
            variant="outline"
            className="h-7 px-2 text-[10px]"
            aria-expanded={!!catalogExpanded}
            data-testid="native-catalog-toggle"
            onClick={onToggleCatalog}
          >
            {catalogExpanded ? (
              <ChevronDown className="h-3 w-3" />
            ) : (
              <ChevronRight className="h-3 w-3" />
            )}
            Model catalog ({models.length})
          </Button>
        )}
        {usable && (
          <Button
            size="sm"
            variant="outline"
            className="h-7 px-2 text-[10px]"
            aria-expanded={configOpen}
            aria-label={`${configOpen ? 'Hide' : 'Configure'} credential-free host override for ${agent.id}`}
            data-testid={`agent-configure-${agent.id}`}
            onClick={() => setConfigOpen((v) => !v)}
          >
            {configOpen ? (
              <ChevronDown className="h-3 w-3" />
            ) : (
              <ChevronRight className="h-3 w-3" />
            )}
            Host model override
          </Button>
        )}
        <Button
          size="sm"
          variant="outline"
          className="h-7 px-2 text-[10px]"
          aria-expanded={subagentOpen}
          aria-label={`${subagentOpen ? 'Hide' : 'Show'} ${agent.name} subagent roster config`}
          data-testid={`subagent-config-toggle-${agent.id}`}
          onClick={() => setSubagentOpen((v) => !v)}
        >
          {subagentOpen ? (
            <ChevronDown className="h-3 w-3" />
          ) : (
            <ChevronRight className="h-3 w-3" />
          )}
          Subagent
        </Button>
        <Button
          size="sm"
          variant="ghost"
          className="h-7 px-2 text-[10px]"
          aria-expanded={detailOpen}
          aria-label={`${detailOpen ? 'Hide' : 'Show'} ${agent.name} detail (native vs shared)`}
          onClick={() => setDetailOpen((v) => !v)}
        >
          {detailOpen ? (
            <ChevronDown className="h-3 w-3" />
          ) : (
            <ChevronRight className="h-3 w-3" />
          )}
          Detail
        </Button>
        <Button
          size="sm"
          variant="ghost"
          className="h-7 px-2 text-[10px]"
          disabled={busyScan}
          onClick={() => void rescan()}
        >
          <RefreshCw className={cn('h-3 w-3', busyScan && 'animate-spin')} />
          Re-scan
        </Button>
        {agent.path && agent.path.startsWith('internal://') !== true && (
          <Button
            size="sm"
            variant="ghost"
            className="ml-auto h-7 max-w-[180px] px-2 text-[10px] text-muted-foreground"
            title="Copy the detected binary path"
            onClick={() => {
              void navigator.clipboard
                ?.writeText(agent.path ?? '')
                .then(() => notify(`Copied ${agent.path}`))
                .catch(() => notify(`Path: ${agent.path}`))
            }}
          >
            <ExternalLink className="h-3 w-3" />
            {agent.path}
          </Button>
        )}
      </div>
    </div>
  )
}

function AgentsTab() {
  const notify = useAppStore((s) => s.notify)
  const liveAgents = useAppStore((s) => s.liveAgents)
  const [busyDiscover, setBusyDiscover] = useState(false)
  // Live discovery (ACP registry + PATH probe) when the shell reported rows;
  // the shipped catalog otherwise. P55.4 — that fallback is a *candidate
  // list*, not occupancy: an empty live list in the desktop shell means
  // discovery has not reported yet, so the banner below says so instead of
  // letting the seed read as "these are on this machine".
  const catalog = liveAgents.length > 0 ? liveAgents : AGENTS
  const occupancyUnknown = inTauri() && liveAgents.length === 0
  // P71.2d — there is no built-in runtime card and no "AgentCowork Native model
  // catalog" disclosure: the desktop's models.dev table is *observation*
  // (Providers / Local models), not a surface any agent receives. Every row in
  // `catalog` is an external agent, so the grid is the whole list.

  const discoverMore = async () => {
    setBusyDiscover(true)
    try {
      const { acpRegistryRefresh } = await import('@/lib/acp')
      const snap = await acpRegistryRefresh()
      await refreshAgentCatalog()
      const live = useAppStore.getState().liveAgents
      const installed = live.filter((a) => a.status === 'installed').length
      notify(
        snap.agentCount
          ? `ACP registry refreshed (${snap.agentCount} agents) — ${live.length} runtimes shown, ${installed} installed`
          : `Runtimes re-scanned — ${live.length} shown, ${installed} installed`,
      )
    } catch (e) {
      notify(e instanceof Error ? e.message : 'Registry refresh failed (offline?)', 'error')
    } finally {
      setBusyDiscover(false)
    }
  }

  return (
    <SectionShell
      title="Agent runtimes"
      desc="The underlying coding-agent CLI / IDE plugin AgentCowork can drive. Installed state is detected live — AgentCowork-installed or auto-discovered on PATH. Each runtime ships its own model support."
      action={
        <Button
          size="sm"
          variant="outline"
          className="h-8"
          disabled={busyDiscover}
          onClick={() => void discoverMore()}
        >
          <Boxes className="h-3.5 w-3.5" />
          {busyDiscover ? 'Refreshing…' : 'Discover more'}
        </Button>
      }
    >
      {occupancyUnknown && (
        <div className="mb-3 rounded-md border border-dashed border-warning/40 bg-warning/5 px-3 py-2 text-[11px] leading-relaxed text-warning/90">
          Runtime inventory unavailable — the shell has not reported which agent CLIs are installed.
          The list below is the shipped catalog of installable runtimes, not occupancy; every
          external row reads <span className="font-mono">not installed</span> until discovery
          confirms it. Use <span className="text-warning">Discover more</span>.
        </div>
      )}
      <div className="grid grid-cols-1 gap-2.5 lg:grid-cols-2 [contain-intrinsic-size:auto_120px]">
        {catalog.length === 0 ? (
          <p className="rounded-md border border-dashed border-border/60 px-3 py-6 text-center text-[11px] text-muted-foreground">
            No runtimes discovered yet — use Discover more to refresh the registry. v1 ships no
            built-in agent, so an empty list means nothing can run a turn until one is installed.
          </p>
        ) : (
          catalog.map((a) => (
            <AgentCard key={a.id} agent={a} />
          ))
        )}
      </div>
    </SectionShell>
  )
}

const TASKS: TaskKind[] = [
  'code',
  'plan',
  'research',
  'browser',
  'shell',
  'office',
  'diff',
  'long-context',
]

function RoutingTab() {
  const routing = useAppStore((s) => s.routing)
  const setRouting = useAppStore((s) => s.setRouting)
  const autoRoute = useAppStore((s) => s.autoRoute)
  const setAutoRoute = useAppStore((s) => s.setAutoRoute)
  const liveAgents = useAppStore((s) => s.liveAgents)
  const streamStats = useAppStore((s) => s.streamStats)
  const liveBudget = useAppStore((s) => s.liveBudget)
  // Honest routing-table sources: live discovery first, static catalog
  // fallback (never a claim that a runtime is installed).
  const catalog = liveAgents.length > 0 ? liveAgents : AGENTS
  const installedCount = catalog.filter(
    (a) => a.status === 'installed' || a.status === 'updating',
  ).length

  return (
    <SectionShell
      title="Task → runtime routing"
      desc="Occupancy is the composer picker: Browse, Computer use, Office, and the right rail all run as the currently picked primary agent. This table is not that path — it is per-task bookkeeping only (P71.5b: the retired \u2018Chief\u2019 name)."
      action={
        <div className="flex items-center gap-2 rounded-md border border-border/60 bg-background/40 px-2 py-1">
          <Route className="h-3 w-3 text-sky-400" />
          <span className="text-[10px] font-medium text-foreground">Auto-route</span>
          <Switch checked={autoRoute} onCheckedChange={setAutoRoute} className="scale-75" />
        </div>
      }
    >
      <div className="overflow-x-auto rounded-lg border border-border/60 scroll-thin">
        <table className="w-full min-w-[520px] text-[11px]">
          <thead className="bg-zinc-900/60 font-mono text-[9px] uppercase tracking-wider text-muted-foreground/80">
            <tr>
              <th className="px-3 py-1.5 text-left">Task kind</th>
              <th className="px-3 py-1.5 text-left min-w-[180px]">Routed runtime</th>
              <th className="px-3 py-1.5 text-left">Why</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border/40">
            {TASKS.map((t) => {
              const agent = catalog.find((a) => a.id === routing[t]) ?? catalog[0]
              return (
                <tr key={t} className="hover:bg-accent/30">
                  <td className="px-3 py-2 font-medium text-foreground">{TASK_LABELS[t]}</td>
                  <td className="px-3 py-2">
                    <div className="flex items-center gap-1.5">
                      <span
                        className={cn(
                          'flex h-5 w-5 items-center justify-center rounded text-[9px] font-bold',
                          agent.accent,
                        )}
                      >
                        {agent.mark}
                      </span>
                      <select
                        value={routing[t]}
                        onChange={(e) => setRouting(t, e.target.value)}
                        disabled={!autoRoute}
                        className="h-7 rounded border border-border bg-background px-1.5 font-mono text-[10px] text-foreground disabled:cursor-not-allowed disabled:opacity-50"
                      >
                        {/* P71.9c — the empty value is a named option: an unset
                            row follows the session's bound agent, it is not a
                            silent pick of the first row. */}
                        <option value="">{ROUTING_BOUND_AGENT}</option>
                        {catalog.map(
                          (a) => (
                            <option key={a.id} value={a.id}>
                              {a.name}
                            </option>
                          ),
                        )}
                      </select>
                    </div>
                  </td>
                  <td className="px-3 py-2 text-[10px] text-muted-foreground">
                    {agent.capabilities.includes(t as any) || t === 'long-context'
                      ? 'Strong match'
                      : 'Fallback'}
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>

      <div className="grid grid-cols-3 gap-2">
        <div className="rounded-md border border-border/60 bg-background/40 p-2">
          <div className="flex items-center gap-1.5 text-[10px] font-medium text-foreground">
            <Zap className="h-3 w-3 text-sky-400" />
            Tokens tracked
          </div>
          <div className="mt-1 font-mono text-lg text-foreground">
            {formatTokens(liveBudget?.tokens ?? 0)}
          </div>
          <div className="text-[9px] text-muted-foreground/70">live usage ledger</div>
        </div>
        <div className="rounded-md border border-border/60 bg-background/40 p-2">
          <div className="flex items-center gap-1.5 text-[10px] font-medium text-foreground">
            <Gauge className="h-3 w-3 text-emerald-400" />
            Active stream key
          </div>
          <div className="mt-1 truncate font-mono text-sm text-foreground">
            {streamStats.activeKey ?? '—'}
          </div>
          <div className="text-[9px] text-muted-foreground/70">per-turn stream stats</div>
        </div>
        <div className="rounded-md border border-border/60 bg-background/40 p-2">
          <div className="flex items-center gap-1.5 text-[10px] font-medium text-foreground">
            <Cpu className="h-3 w-3 text-sky-400" />
            Installed runtimes
          </div>
          <div className="mt-1 font-mono text-lg text-foreground">
            {installedCount} / {catalog.length}
          </div>
          <div className="text-[9px] text-muted-foreground/70">live discovery</div>
        </div>
      </div>
    </SectionShell>
  )
}

// === Section shell ===========================================================

export default function AgentsModelsSection() {
  const [tab, setTab] = useState('agents')
  return (
    <SectionShell
      title="Agent CLIs & Runtimes"
      desc="Every row is an external agent CLI discovered from the ACP registry or PATH — each owns its own authentication and model configuration (ADR-0005: v1 ships no built-in engine)."
    >
      <Tabs value={tab} onValueChange={setTab}>
        <TabsList className="h-8 bg-background/40">
          <TabsTrigger value="agents" className="text-[11px] data-[state=active]:bg-sky-500/15 data-[state=active]:text-sky-300">
            <Boxes className="mr-1 h-3 w-3" />
            Runtimes
          </TabsTrigger>
          <TabsTrigger value="routing" className="text-[11px] data-[state=active]:bg-sky-500/15 data-[state=active]:text-sky-300">
            <Route className="mr-1 h-3 w-3" />
            Routing
          </TabsTrigger>
        </TabsList>
        <TabsContent value="agents" className="mt-3">
          <AgentsTab />
        </TabsContent>
        <TabsContent value="routing" className="mt-3">
          <RoutingTab />
        </TabsContent>
      </Tabs>
    </SectionShell>
  )
}
