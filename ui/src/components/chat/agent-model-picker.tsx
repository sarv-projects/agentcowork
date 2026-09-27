'use client'

import { useEffect, useState } from 'react'
import {
  Check,
  ChevronDown,
  Cpu,
  Download,
  KeyRound,
  Loader2,
  RotateCw,
  Route,
  Sparkles,
  FileSpreadsheet,
  Globe,
  Monitor,
  Search,
  HardDrive,
  Brain,
  Sliders,
  CheckCircle2,
  AlertCircle,
  FolderOpen,
  Wrench,
  RefreshCw,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useAppStore } from '@/lib/store'
import {
  AGENTS,
  isRuntimeUsable,
  CAPABILITY_LABELS,
  type AgentRuntime,
} from '@/lib/agents'
import { cn } from '@/lib/utils'
import {
  acpIdFor,
  chiefDefaultGet,
  chiefDefaultSet,
  currentBinding,
  governanceLabel,
  acpAgentImport,
  acpAgentVerify,
  authMethodLabel,
  getAgentLifecycleState,
  type AcpConfigOption,
  type AgentLifecycleState,
  type AuthMethod,
} from '@/lib/acp'
import { STANDARD_SHARED_CAPABILITIES, isCapabilityEnabled } from '@/lib/capabilities'
import { refreshAgentCatalog } from '@/lib/bridge'
import { inTauri } from '@/lib/tauri'

// A stable empty list: a selector that returns a fresh `[]` on every render
// makes `useSyncExternalStore` re-render forever under zustand v5.
const NO_CONFIG_OPTIONS: AcpConfigOption[] = []

// P71.2a/2d — there is no built-in row to single out: every row in the catalog
// is an external agent the user installed or discovered.

function StatusDot({ status }: { status: AgentRuntime['status'] }) {
  const tone =
    status === 'installed'
      ? 'bg-emerald-400'
      : status === 'discovered'
        ? 'bg-sky-400'
        : status === 'updating'
          ? 'bg-blue-400'
          : status === 'available'
            ? 'bg-zinc-500'
            : 'bg-zinc-700'
  return <span className={cn('inline-block h-1.5 w-1.5 rounded-full', tone)} />
}

function LifecycleBadge({ state }: { state: AgentLifecycleState }) {
  switch (state) {
    case 'ready':
      return (
        <Badge className="border-emerald-500/30 bg-emerald-500/15 px-1 text-[8px] font-mono text-emerald-300">
          ready
        </Badge>
      )
    case 'verify':
      return (
        <Badge className="border-sky-500/30 bg-sky-500/15 px-1 text-[8px] font-mono text-sky-300">
          verify
        </Badge>
      )
    case 'import':
      return (
        <Badge className="border-indigo-500/30 bg-indigo-500/15 px-1 text-[8px] font-mono text-indigo-300">
          import
        </Badge>
      )
    case 'inspect':
      return (
        <Badge className="border-warning/30 bg-warning/15 px-1 text-[8px] font-mono text-warning">
          inspect
        </Badge>
      )
    case 'discover':
    default:
      return (
        <Badge className="border-zinc-700 bg-zinc-800/80 px-1 text-[8px] font-mono text-zinc-400">
          discover · not installed
        </Badge>
      )
  }
}

function ProvenanceBadge({ location }: { location?: AgentRuntime['location'] }) {
  if (!location) return null
  const kind = location.kind
  const source = location.source
  let label: string = source.replaceAll('_', ' ')
  let tone = 'bg-zinc-800 text-zinc-300 border-zinc-700'

  if (kind === 'managed' || source === 'everyaios_install') { // DEC-053: persisted source enum — stays so stored runtimes keep matching.
    label = 'Managed'
    tone = 'bg-emerald-500/10 text-emerald-400 border-emerald-500/30'
  } else if (source === 'user_selected') {
    label = 'User Import'
    tone = 'bg-sky-500/10 text-sky-300 border-sky-500/30'
  } else if (source === 'path_probe') {
    label = 'PATH'
    tone = 'bg-zinc-800 text-zinc-300 border-zinc-700'
  } else if (source === 'app_paths') {
    label = 'App Paths'
    tone = 'bg-zinc-800 text-zinc-300 border-zinc-700'
  } else if (kind === 'package_manager') {
    label = 'npx/uvx'
    tone = 'bg-zinc-800 text-zinc-300 border-zinc-700'
  } else if (kind === 'wsl' || source === 'wsl_probe') {
    label = `WSL · ${'distro' in location ? location.distro : 'Ubuntu'}`
    tone = 'bg-purple-500/10 text-purple-300 border-purple-500/30'
  }

  return (
    <span className={cn('inline-flex items-center rounded border px-1 py-0.2 font-mono text-[7px]', tone)}>
      {label}
    </span>
  )
}

function getCapabilityIcon(family: string) {
  switch (family) {
    case 'office':
      return <FileSpreadsheet className="h-3 w-3 text-sky-400" />
    case 'browser':
      return <Globe className="h-3 w-3 text-cyan-400" />
    case 'desktop':
      return <Monitor className="h-3 w-3 text-indigo-400" />
    case 'search':
      return <Search className="h-3 w-3 text-teal-400" />
    case 'storage':
      return <HardDrive className="h-3 w-3 text-blue-400" />
    case 'memory':
      return <Brain className="h-3 w-3 text-purple-400" />
    default:
      return <Sparkles className="h-3 w-3 text-sky-400" />
  }
}

function AgentLogo({ agent, size = 'md' }: { agent: AgentRuntime; size?: 'sm' | 'md' }) {
  return (
    <span
      className={cn(
        'flex shrink-0 items-center justify-center rounded font-mono font-bold',
        size === 'sm' ? 'h-5 w-5 text-[9px]' : 'h-6 w-6 text-[10px]',
        agent.accent,
      )}
    >
      {agent.mark}
    </span>
  )
}

interface Props {
  /** When set, hides the popover trigger chevron (used in tight rows) */
  compact?: boolean
}

export default function AgentModelPicker({ compact }: Props) {
  const [open, setOpen] = useState(false)
  const selectedAgentId = useAppStore((s) => s.selectedAgentId)
  const setSelectedAgent = useAppStore((s) => s.setSelectedAgent)
  const setCenterScreen = useAppStore((s) => s.setCenterScreen)
  const notify = useAppStore((s) => s.notify)

  const liveAgents = useAppStore((s) => s.liveAgents)
  // P55.4 / P60.14 / P71.2a — occupancy is never painted from the static seed.
  // `liveAgents` is the shell's *merged* catalog (seed + ACP registry + install
  // records), so a non-empty list is the only evidence of what this machine
  // actually has. An empty list **in the desktop shell** means discovery has not
  // produced a result — not "nothing is installed" — and v1 ships no built-in
  // agent to offer in its place, so the list stays empty behind an honest
  // inventory note. The plain-browser preview keeps the fixture because it has
  // no shell to ask.
  const inShell = inTauri()
  const occupancyUnknown = inShell && liveAgents.length === 0
  const catalog = liveAgents.length > 0 ? liveAgents : inShell ? [] : AGENTS
  const [installing, setInstalling] = useState(false)
  const [connecting, setConnecting] = useState(false)
  const [connected, setConnected] = useState<string | null>(null)

  // P66.2 — Custom binary import & verification state
  const [customBinaryPath, setCustomBinaryPath] = useState('')
  const [verifying, setVerifying] = useState(false)
  const [verifyResult, setVerifyResult] = useState<{ ok: boolean; message: string } | null>(null)
  const [showImportSection, setShowImportSection] = useState(false)

  // P66.4 — Session capability loadout bindings
  const activeSessionId = useAppStore((s) => s.activeSessionId)
  const activeSession = useAppStore((s) => s.sessions.find((x) => x.id === s.activeSessionId))
  const setSessionCapabilityOverride = useAppStore((s) => s.setSessionCapabilityOverride)
  const resetSessionCapabilities = useAppStore((s) => s.resetSessionCapabilities)

  const acpConfigOptions = useAppStore(
    (s) => s.acpConfigOptions[selectedAgentId] ?? NO_CONFIG_OPTIONS,
  )
  const setAcpConfigOptions = useAppStore((s) => s.setAcpConfigOptions)
  const [auth, setAuth] = useState<{
    handle: string
    methods: AuthMethod[]
    waitingUrl?: string
  } | null>(null)

  // Handlers for custom binary import & verification
  const handleImportCustomBinary = async (agentId: string) => {
    if (!customBinaryPath.trim()) {
      notify('Please enter a valid binary path', 'error')
      return
    }
    setVerifying(true)
    setVerifyResult(null)
    try {
      const importRes = await acpAgentImport(agentId, customBinaryPath.trim())
      const verifyRes = await acpAgentVerify(agentId)
      if (verifyRes.status === 'ready') {
        setVerifyResult({
          ok: true,
          message: `Verified v${verifyRes.version ?? 'unknown'} (${verifyRes.executable ?? importRes.binaryPath})`,
        })
        notify(`${agent.name} imported and verified successfully!`)
        void refreshAgentCatalog().catch(() => {})
      } else {
        const reason = verifyRes.reason ?? 'Binary failed execution probe'
        setVerifyResult({
          ok: false,
          message: reason,
        })
        notify(`Import recorded, but verification failed: ${reason}`, 'error')
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setVerifyResult({ ok: false, message: msg })
      notify(msg, 'error')
    } finally {
      setVerifying(false)
    }
  }

  const handleVerifyAgent = async (agentId: string) => {
    setVerifying(true)
    setVerifyResult(null)
    try {
      const verifyRes = await acpAgentVerify(agentId)
      if (verifyRes.status === 'ready') {
        setVerifyResult({
          ok: true,
          message: `Verified v${verifyRes.version ?? 'unknown'} (${verifyRes.executable ?? agent.path ?? 'executable'})`,
        })
        notify(`${agent.name} binary verified!`)
        void refreshAgentCatalog().catch(() => {})
      } else {
        const reason = verifyRes.reason ?? 'Verification probe failed'
        setVerifyResult({
          ok: false,
          message: reason,
        })
        notify(`Verification failed: ${reason}`, 'error')
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setVerifyResult({ ok: false, message: msg })
      notify(msg, 'error')
    } finally {
      setVerifying(false)
    }
  }

  // The trigger must name the *selection*, even when the inventory is
  // unknown — so fall back to the seed row by id (label only, never
  // occupancy) before the first catalogue row.
  const agent =
    catalog.find((a) => a.id === selectedAgentId && a.name) ??
    AGENTS.find((a) => a.id === selectedAgentId && a.name) ??
    catalog.find((a) => a.name) ??
    AGENTS[0]
  if (!agent) return null

  // P38 — the default-agent slot: which agent occupies the top brain by
  // default. `null` means **no default is set**, which is a real state in v1:
  // there is no built-in engine to occupy the slot on the user's behalf, and
  // the retired spellings (`inbuilt` / `everyaios` / `everyaios-native`) are
  // read as "nothing set" rather than as an engine (ADR-0005 §1, P71.5b — the
  // retired name "Chief"). Resolution is fail-closed upstream.
  // DEC-053: these legacy spellings stay verbatim so retirement recognition keeps working.
  const [defaultAgent, setDefaultAgent] = useState<string | null>(null)
  useEffect(() => {
    chiefDefaultGet()
      .then((r) => setDefaultAgent(currentBinding(r.primaryChief)))
      .catch(() => {})
  }, [])
  // Map the picker's runtime id onto a binding id. P53.3 — installed-any: every
  // registry row is bindable, and install truth is enforced by the shell
  // (`chief_default_set` refuses unknown/uninstalled ids fail-closed), so the
  // picker never gates on a hardcoded trio.
  const acpIdForAgent = (runtimeId: string): string | null =>
    runtimeId ? acpIdFor(runtimeId) : null
  const chiefEligibleId = acpIdForAgent(agent.id)
  const defaultChiefLabel = defaultAgent ?? 'no agent bound'
  const handleSetDefaultAgent = async () => {
    if (!chiefEligibleId) return
    try {
      await chiefDefaultSet(chiefEligibleId)
      setDefaultAgent(chiefEligibleId)
      notify(`Default agent set to ${chiefEligibleId}`)
    } catch (e) {
      notify(e instanceof Error ? e.message : String(e), 'error')
    }
  }
  const activeFolder = useAppStore(
    (s) => s.sessions.find((x) => x.id === s.activeSessionId)?.folder,
  )

  // P38 — per-session agent pin: pin this agent as the session's primary
  // session only (outranks the user default for that session's turns). The
  // pin lives in the store and the chat send path passes it as primaryChief;
  // every v1 binding takes the ACP branch, so a pin can only ever select an
  // installed external agent (ADR-0005).
  const sessionPin = useAppStore((s) => s.sessionChiefs[s.activeSessionId])
  // P38 — was the active session explicitly unpinned? Persisted on the
  // Session (vault round-trip) so a restarted session that had a pin shows
  // "default applies — pin cleared" instead of silence.
  const sessionUnpinned = useAppStore(
    (s) => s.sessions.find((x) => x.id === s.activeSessionId)?.chiefUnpinned === true,
  )
  const setSessionChiefPin = useAppStore((s) => s.setSessionChiefPin)
  const clearSessionChiefPin = useAppStore((s) => s.clearSessionChiefPin)
  const handlePinAgent = () => {
    const sessionId = useAppStore.getState().activeSessionId
    if (!sessionId) {
      notify('No active chat to pin to')
      return
    }
    if (!chiefEligibleId) {
      notify(`${agent.name} has no binding id to pin`)
      return
    }
    // P38 — clicking the already-pinned agent unpins (surfaces the durable
    // pin lifecycle, not just pin-once).
    if (sessionPin === chiefEligibleId) {
      clearSessionChiefPin(sessionId)
      notify(`Agent pin cleared for this chat — default applies again`)
      return
    }
    setSessionChiefPin(sessionId, chiefEligibleId)
    notify(`Chat pinned to ${chiefEligibleId} — this chat now routes through that agent`)
  }

  // F8 — plan-before-touch install: request (Guard-2 ticket or auto-allow),
  // then commit. The approved card shows in the transcript via the bridge.
  // The ACP registry keys agents by registry id (`claude`), while this picker
  // row carries the catalog id (`claude-code`) — always translate.
  const installAgent = async (agentId: string) => {
    setInstalling(true)
    try {
      const { acpInstallRequest, acpInstallCommit, acpInstallAwait } = await import('@/lib/acp')
      const rid = acpIdFor(agentId)
      const req = await acpInstallRequest(rid)
      const licenseNote = req.license ? ` · license: ${req.license}` : ''
      const cmd = (req.exactCommand ?? []).join(' ')
      if (req.consentRequired && cmd) {
        const ok = window.confirm(
          `SEP-1024 exact-command consent\n\nInstall ${agentId}?${licenseNote}\n\n${cmd}${req.preferNative ? '\n\nPrefer verified native artifact.' : ''}`,
        )
        if (!ok) {
          notify('Install cancelled')
          return
        }
      }
      if (req.action === 'allow') {
        // Auto-allowed still consumes its pre-approved single-use ticket.
        await acpInstallCommit(rid, req.ticketId)
        notify(`${agent?.name} installed — pick it and send`)
        // Re-run discovery so the row flips to installed (installer record
        // now exists; PATH probe also sees the fresh binary).
        void refreshAgentCatalog().catch(() => {})
        setOpen(false)
      } else {
        // P69.C12 — one explicit consent in the dedicated Guard window, then
        // commit; a decline/timeout surfaces the visible reason instead of a
        // silent stall.
        notify(
          `Consent needed${licenseNote} — ${req.reason ?? 'proprietary agent'}; approve the Guard-2 card #${req.ticketId.slice(0, 8)}`,
        )
        const { approved, reason } = await acpInstallAwait(req.ticketId)
        if (!approved) {
          notify(reason ?? 'Install not approved')
          return
        }
        await acpInstallCommit(rid, req.ticketId)
        notify(`${agent?.name} installed — pick it and send`)
        void refreshAgentCatalog().catch(() => {})
        setOpen(false)
      }
    } catch (err) {
      notify(err instanceof Error ? err.message : 'Install failed')
    } finally {
      setInstalling(false)
    }
  }

  // J17 — connect + auth: spawn the agent over ACP; if it demands sign-in,
  // surface its authMethods (url-type opens the system browser, then retry).
  const connectAgent = async (agentId: string) => {
    setConnecting(true)
    setAuth(null)
    try {
      const { acpLaunch } = await import('@/lib/acp')
      // Registry id, not the picker's catalog id (claude-code → claude).
      const info = await acpLaunch(acpIdFor(agentId), activeFolder ?? '~')
      // P60 — record the agent-owned config vocabulary this session negotiated
      // (model/mode/reasoning) instead of falling back to Native's models.
      if (info.configOptions) setAcpConfigOptions(agentId, info.configOptions)
      if (!info.authRequired || info.authMethods.length === 0) {
        setConnected(info.handle)
        notify(`${agent?.name} connected (${info.handle.slice(0, 8)}…)`)
        return
      }
      setAuth({ handle: info.handle, methods: info.authMethods })
    } catch (err) {
      notify(err instanceof Error ? err.message : 'Failed to launch agent')
    } finally {
      setConnecting(false)
    }
  }

  const signIn = async (methodId: string) => {
    if (!auth) return
    const { acpAuthenticate } = await import('@/lib/acp')
    try {
      const res = await acpAuthenticate(auth.handle, methodId)
      if (res.ok || res.sessionId) {
        setConnected(auth.handle)
        setAuth(null)
        notify('Signed in — the agent is ready')
      } else if (res.pending && res.url) {
        setAuth({ ...auth, waitingUrl: res.url })
        window.open(res.url, '_blank')
        notify('Opened the sign-in page — approve there, then retry')
      } else if (res.pending) {
        setAuth({ ...auth, waitingUrl: undefined })
        notify('Waiting for the agent-side login to complete…')
      }
    } catch (err) {
      notify(err instanceof Error ? err.message : 'Sign-in failed')
    }
  }

  const agentUsable = isRuntimeUsable(
    liveAgents.find((a) => a.id === selectedAgentId) ??
      catalog.find((a) => a.id === selectedAgentId),
  )
  // P60/P71.2d — model ownership: every runtime is an external ACP agent that
  // owns its own model; P71.9c removed the AgentCowork-owned branches this
  // constant used to guard, so the agent's ACP model option is the only row.
  const externalModelOption =
    acpConfigOptions.find((o) => o.category === 'model') ??
    acpConfigOptions.find((o) => o.id.toLowerCase().includes('model'))
  const externalModelLabel = externalModelOption
    ? String(externalModelOption.currentValue)
    : null
  // P71.9c — the trigger paints the agent-owned model value (or an explicit
  // em dash); AgentCowork has no model list of its own to pin.
  const pinnedLabel = externalModelLabel ?? '—'

  useEffect(() => {
    if (!open || !agentUsable) return
    let alive = true
    void (async () => {
      try {
        const st = useAppStore.getState()
        const handle = st.acpHandles[selectedAgentId]
        if (!handle) return
        const { acpSessionConfigOptions } = await import('@/lib/acp')
        const options = await acpSessionConfigOptions(handle)
        if (alive) setAcpConfigOptions(selectedAgentId, options)
      } catch {
        if (alive) setAcpConfigOptions(selectedAgentId, [])
      }
    })()
    return () => {
      alive = false
    }
  }, [open, agentUsable, selectedAgentId, setAcpConfigOptions])

  // P51.1 — is any session mid-turn right now? A model switch during a live
  // stream applies to the *next* turn, never the in-flight one; surface that
  // instead of letting the user believe the running turn switched too.
  const anyBusy = useAppStore((s) =>
    s.sessions.some((x) => x.status === 'running' || x.status === 'action-required'),
  )

  // P60 — selection is installed-only. `catalog` also carries registry rows
  // that exist as *catalog entries* but have no binary on this machine; those
  // render with an explicit not-installed state and an install affordance
  // instead of becoming a selectable agent the send path cannot launch.
  const usableRows = catalog.filter((a) => isRuntimeUsable(a))
  const unavailableRows = catalog.filter((a) => !isRuntimeUsable(a))
  const agentList = [...usableRows, ...unavailableRows]

  return (
    <div className="relative">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-label="Choose agent and model"
        className={cn(
          'group flex max-w-[220px] items-center gap-1.5 rounded-md border border-border bg-background px-2 py-1 font-mono text-[11px] transition-all duration-200 hover:border-sky-500/40 hover:bg-sky-500/5',
          open && 'border-sky-500/60 bg-sky-500/10',
        )}
      >
        <span key={selectedAgentId} className="agent-switch-pulse inline-flex">
          <AgentLogo agent={agent} size="sm" />
        </span>
        <span className="flex min-w-0 items-baseline gap-1">
          <span className={cn('truncate text-foreground', compact ? 'max-w-[7rem]' : '')}>
            {agent.name}
          </span>
          {!compact && (
            <>
              <span className="text-muted-foreground/40">·</span>
              <span className="max-w-[9rem] truncate text-sky-300">{pinnedLabel}</span>
            </>
          )}
        </span>
        <ChevronDown
          className={cn('h-3 w-3 text-muted-foreground transition-transform', open && 'rotate-180')}
        />
      </button>

      {open && (
        <>
          <button
            type="button"
            aria-label="Close picker"
            className="fixed inset-0 z-20 cursor-default"
            onClick={() => setOpen(false)}
          />
          <div className="scale-in fixed inset-4 z-30 flex max-h-[calc(100vh-2rem)] min-h-[min(640px,calc(100vh-2rem))] flex-col overflow-hidden rounded-xl border border-border bg-popover shadow-2xl">
            <div className="flex items-center justify-between border-b border-border bg-zinc-900/60 px-3 py-1.5">
              <div className="flex items-center gap-1.5">
                <Cpu className="h-3 w-3 text-sky-400" />
                <span className="text-[11px] font-semibold text-foreground">
                  Agent runtime & model
                </span>
              </div>
              <button
                type="button"
                onClick={() => {
                  setOpen(false)
                  setCenterScreen('settings')
                }}
                className="text-[10px] text-muted-foreground underline-offset-2 hover:text-sky-300 hover:underline"
              >
                Manage in settings
              </button>
            </div>

            {/* P38 — Default-agent slot: the swappable top brain (P71.5b: the retired name was \u2018Chief\u2019) */}
            <div className="flex items-center justify-between gap-2 border-b border-border bg-zinc-950/40 px-3 py-1.5">
              <div className="flex min-w-0 items-center gap-1.5">
                <Route className="h-3 w-3 shrink-0 text-sky-400" />
                <span className="truncate text-[10px] text-muted-foreground">
                  Default agent:{' '}
                  <span className="font-mono text-sky-300">{defaultChiefLabel}</span>
                </span>
                {defaultAgent && (
                  <Badge className="shrink-0 bg-emerald-500/15 px-1 text-[8px] text-emerald-300">
                    swappable
                  </Badge>
                )}
              </div>
              <div className="flex shrink-0 items-center gap-2">
                {chiefEligibleId && (
                  <button
                    type="button"
                    onClick={handlePinAgent}
                    className="shrink-0 text-[10px] text-muted-foreground underline-offset-2 hover:text-sky-300 hover:underline"
                    title="Pin this agent for the active chat only (outranks the user default); click again to unpin"
                  >
                    {sessionPin
                      ? chiefEligibleId === sessionPin
                        ? 'unpin from this chat'
                        : `Pin ${agent.name} for this chat`
                      : `Pin ${agent.name} for this chat`}
                  </button>
                )}
                {chiefEligibleId && (
                  <button
                    type="button"
                    onClick={handleSetDefaultAgent}
                    disabled={chiefEligibleId === defaultAgent}
                    className="shrink-0 text-[10px] text-muted-foreground underline-offset-2 hover:text-sky-300 hover:underline disabled:cursor-default disabled:opacity-40 disabled:hover:text-muted-foreground disabled:hover:no-underline"
                  >
                    {chiefEligibleId === defaultAgent
                      ? 'default agent'
                      : `Set ${agent.name} as default agent`}
                  </button>
                )}
              </div>
            </div>
            {/* P38 — effective-binding readout: shows what THIS chat actually
                routes under (pin → default → **unbound**). Unbound is not a
                silent substitution: the turn refuses and opens agent discovery
                (`P71.6a`). An explicitly unpinned chat shows "default applies
                — pin cleared" instead of silence, even after a restart (the
                marker is vault-persisted). */}
            {sessionPin && (
              <div className="border-b border-border bg-sky-500/5 px-3 py-1 font-mono text-[9px] text-sky-300/90">
                Chat pinned to <span className="font-semibold">{sessionPin}</span> — outranks the user default for this chat.
              </div>
            )}
            {!sessionPin && sessionUnpinned && (
              <div className="border-b border-border bg-emerald-500/5 px-3 py-1 font-mono text-[9px] text-emerald-300/90">
                <span className="font-semibold">Default Chief applies</span> — this chat's pin was cleared; it follows the user default again.
              </div>
            )}

            <div className="grid min-h-0 flex-1 grid-cols-1 md:grid-cols-[minmax(0,300px)_minmax(0,1fr)]">
              {/* Agent column */}
              <div className="scroll-thin max-h-48 min-h-0 overflow-y-auto border-b border-border p-2 md:max-h-none md:border-b-0 md:border-r">
                <div className="px-1 pb-1 font-mono text-[9px] uppercase tracking-wider text-muted-foreground/70">
                  Runtimes
                </div>
                {occupancyUnknown && (
                  <div className="mb-1.5 rounded-md border border-dashed border-warning/40 bg-warning/5 px-2 py-1.5 text-[10px] leading-relaxed text-warning/90">
                    Runtime inventory unavailable — the shell has not reported which agent CLIs are
                    on this machine, so no runtime is listed and **no turn can run**: v1 ships no
                    built-in engine to fall back to. Re-run discovery from{' '}
                    <span className="text-warning">Settings → Agent runtimes</span>.
                  </div>
                )}
                {agentList.map((a) => {
                  const isActive = a.id === selectedAgentId
                  const usable = isRuntimeUsable(a)
                  const lifecycle = getAgentLifecycleState(a.status, usable, Boolean(a.version))
                  return (
                    <button
                      key={a.id}
                      type="button"
                      data-agent-id={a.id}
                      onClick={() => {
                        if (usable) {
                          setSelectedAgent(a.id)
                        }
                      }}
                      title={
                        usable
                          ? undefined
                          : `${a.name} is not installed — select to inspect, import, or install`
                      }
                      className={cn(
                        'flex w-full items-start gap-2 rounded-md border px-2 py-1.5 text-left transition-colors',
                        isActive
                          ? 'border-sky-500/60 bg-sky-500/10'
                          : 'border-transparent hover:border-border hover:bg-accent/40',
                        !usable && 'opacity-80',
                      )}
                    >
                      <AgentLogo agent={a} />
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-1.5">
                          <span className={cn('text-[11px] font-medium', isActive ? 'text-sky-200' : 'text-foreground')}>
                            {a.name}
                          </span>
                          <StatusDot status={a.status} />
                          <LifecycleBadge state={lifecycle} />
                        </div>
                        <div className="mt-0.5 flex flex-wrap items-center gap-1 font-mono text-[9px] text-muted-foreground">
                          <span className="truncate">{a.vendor} · v{a.version ?? '—'}</span>
                          {a.location && <ProvenanceBadge location={a.location} />}
                        </div>
                        {a.location && (
                          <div className="truncate font-mono text-[8px] text-muted-foreground/70" title={a.path ?? undefined}>
                            {a.location.kind === 'wsl' ? `WSL · ${a.location.distro || 'default'}` : a.path ? a.path : a.location.source.replaceAll('_', ' ')}
                          </div>
                        )}
                        <div className="truncate text-[10px] text-muted-foreground/80">{a.tagline}</div>
                        {/* P50.3.9 — governance truth badge: the picker never
                            implies AgentCowork audit coverage that does not exist. */}
                        {a.governance && (
                          <div
                            className={cn(
                              'truncate text-[9px] font-medium',
                              a.governance.class === 'GovernedMediated'
                                ? 'text-emerald-400/90'
                                : a.governance.class === 'SelfContained'
                                  ? 'text-warning/90'
                                  : 'text-red-400/90',
                            )}
                            title={a.governance.note}
                          >
                            {governanceLabel(a.governance)}
                          </div>
                        )}
                      </div>
                      {isActive && <Check className="mt-1 h-3 w-3 shrink-0 text-sky-400" />}
                    </button>
                  )
                })}
              </div>

              {/* Model & Capabilities column */}
              <div className="scroll-thin min-h-0 overflow-y-auto p-3">
                {/* External Agent Lifecycle & Verification Header */}
                <div className="mb-3 rounded-lg border border-border/80 bg-zinc-950/60 p-2.5">
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-1.5">
                        <Wrench className="h-3 w-3 text-sky-4" />
                        <span className="font-mono text-[10px] font-semibold text-foreground">
                          Agent Lifecycle · {agent.name}
                        </span>
                      </div>
                      <LifecycleBadge
                        state={getAgentLifecycleState(agent.status, agentUsable, Boolean(agent.version))}
                      />
                    </div>

                    {/* Step progression */}
                    <div className="mt-2 grid grid-cols-5 gap-1 text-center font-mono text-[8px]">
                      {[
                        { step: 'Discover', active: true },
                        { step: 'Inspect', active: agent.status === 'discovered' || agentUsable },
                        { step: 'Import/Install', active: agent.status === 'installed' || agentUsable },
                        { step: 'Verify', active: Boolean(agent.version) || agentUsable },
                        { step: 'Ready', active: agentUsable },
                      ].map((s, idx) => (
                        <div
                          key={s.step}
                          className={cn(
                            'rounded py-0.5 border',
                            s.active
                              ? 'border-sky-500/40 bg-sky-500/10 text-sky-300 font-semibold'
                              : 'border-border/40 bg-background/40 text-muted-foreground/50',
                          )}
                        >
                          {idx + 1}. {s.step}
                        </div>
                      ))}
                    </div>

                    {/* Custom binary import & verify accordion / controls */}
                    <div className="mt-2.5 border-t border-border/50 pt-2">
                      <div className="flex items-center justify-between">
                        <span className="text-[10px] text-muted-foreground">
                          {agent.path ? `Binary: ${agent.path}` : 'Custom binary executable'}
                        </span>
                        <div className="flex items-center gap-1.5">
                          {agent.path && (
                            <Button
                              size="sm"
                              variant="ghost"
                              disabled={verifying}
                              className="h-5 gap-1 px-1.5 font-mono text-[9px] text-sky-300 hover:bg-sky-500/10"
                              onClick={() => handleVerifyAgent(agent.id)}
                            >
                              <RefreshCw className={cn('h-2.5 w-2.5', verifying && 'animate-spin')} />
                              Re-verify
                            </Button>
                          )}
                          <Button
                            size="sm"
                            variant="ghost"
                            className="h-5 px-1.5 font-mono text-[9px] text-muted-foreground hover:text-foreground"
                            onClick={() => setShowImportSection((v) => !v)}
                          >
                            {showImportSection ? 'Hide Import' : 'Import Custom Path'}
                          </Button>
                        </div>
                      </div>

                      {showImportSection && (
                        <div className="mt-2 space-y-1.5 rounded-md border border-border/60 bg-background/50 p-2">
                          <div className="text-[9px] text-muted-foreground">
                            Specify the full absolute path to the <span className="font-mono text-foreground">{agent.name}</span> binary:
                          </div>
                          <div className="flex gap-1.5">
                            <input
                              type="text"
                              value={customBinaryPath}
                              onChange={(e) => setCustomBinaryPath(e.target.value)}
                              placeholder="e.g. C:\bin\agent.exe or /usr/local/bin/agent"
                              className="h-6 flex-1 rounded border border-border bg-background px-2 font-mono text-[9px] text-foreground placeholder:text-muted-foreground/50 focus:border-sky-500 focus:outline-none"
                            />
                            <Button
                              size="sm"
                              disabled={verifying || !customBinaryPath.trim()}
                              className="h-6 gap-1 bg-sky-500 px-2 font-mono text-[9px] text-white hover:bg-sky-600"
                              onClick={() => handleImportCustomBinary(agent.id)}
                            >
                              {verifying ? (
                                <Loader2 className="h-2.5 w-2.5 animate-spin" />
                              ) : (
                                <CheckCircle2 className="h-2.5 w-2.5" />
                              )}
                              Import & Verify
                            </Button>
                          </div>
                        </div>
                      )}

                      {verifyResult && (
                        <div
                          className={cn(
                            'mt-1.5 flex items-center gap-1.5 rounded px-2 py-1 font-mono text-[9px]',
                            verifyResult.ok
                              ? 'border border-emerald-500/30 bg-emerald-500/10 text-emerald-300'
                              : 'border border-red-500/30 bg-red-500/10 text-red-300',
                          )}
                        >
                          {verifyResult.ok ? (
                            <CheckCircle2 className="h-3 w-3 shrink-0 text-emerald-400" />
                          ) : (
                            <AlertCircle className="h-3 w-3 shrink-0 text-red-400" />
                          )}
                          <span className="truncate">{verifyResult.message}</span>
                        </div>
                      )}
                    </div>
                  </div>

                {/* P66.4 — Session Capability Loadout Section */}
                <div className="mb-3 rounded-lg border border-border/80 bg-zinc-950/40 p-2.5">
                  <div className="flex items-center justify-between pb-1.5">
                    <div className="flex items-center gap-1.5">
                      <Sliders className="h-3 w-3 text-sky-400" />
                      <span className="font-mono text-[10px] font-semibold text-foreground">
                        Chat Capability Loadout
                      </span>
                      {anyBusy && (
                        <Badge className="bg-warning/15 px-1 text-[7px] text-warning">
                          applies next turn
                        </Badge>
                      )}
                    </div>
                    {activeSessionId && activeSession?.capabilityLoadout?.overrides && Object.keys(activeSession.capabilityLoadout.overrides).length > 0 && (
                      <Button
                        size="sm"
                        variant="ghost"
                        className="h-4 px-1.5 font-mono text-[8px] text-muted-foreground hover:text-sky-300"
                        onClick={() => {
                          resetSessionCapabilities(activeSessionId)
                          notify('Chat capabilities reset to defaults')
                        }}
                      >
                        Reset Defaults
                      </Button>
                    )}
                  </div>
                  <div className="grid grid-cols-1 gap-1.5 sm:grid-cols-2">
                    {STANDARD_SHARED_CAPABILITIES.map((cap) => {
                      const isEnabled = isCapabilityEnabled(
                        cap.id,
                        activeSession?.capabilityLoadout,
                        cap.enabled,
                      )
                      return (
                        <div
                          key={cap.id}
                          className={cn(
                            'flex items-center justify-between rounded-md border p-1.5 transition-colors',
                            isEnabled
                              ? 'border-sky-500/30 bg-sky-500/5'
                              : 'border-border/50 bg-background/30 opacity-70',
                          )}
                        >
                          <div className="flex min-w-0 items-start gap-1.5">
                            <span className="mt-0.5 shrink-0">{getCapabilityIcon(cap.family)}</span>
                            <div className="min-w-0 flex-1">
                              <div className="flex items-center gap-1">
                                <span className={cn('truncate text-[10px] font-medium', isEnabled ? 'text-foreground' : 'text-muted-foreground')}>
                                  {cap.name}
                                </span>
                                {cap.requiresApproval && (
                                  <Badge className="bg-warning/10 px-0.5 text-[6px] text-warning">
                                    ticketed
                                  </Badge>
                                )}
                              </div>
                              <p className="line-clamp-1 text-[8px] text-muted-foreground/80">
                                {cap.description}
                              </p>
                            </div>
                          </div>
                          <Switch
                            checked={isEnabled}
                            onCheckedChange={(checked) => {
                              if (activeSessionId) {
                                setSessionCapabilityOverride(activeSessionId, cap.id, checked)
                                if (anyBusy) {
                                  notify(`${cap.name} ${checked ? 'enabled' : 'disabled'} for next turn`)
                                }
                              }
                            }}
                            className="scale-75 shrink-0 ml-1"
                          />
                        </div>
                      )
                    })}
                  </div>
                </div>

                <div className="mb-1 flex items-center justify-between px-1">
                  <div className="font-mono text-[9px] uppercase tracking-wider text-muted-foreground/70">
                    {`Model · ${agent.name}`}
                  </div>
                </div>

                {/* Agent capabilities strip */}
                <div className="mb-2 flex flex-wrap gap-1 px-1">
                  {agent.capabilities.slice(0, 6).map((c) => (
                    <Badge
                      key={c}
                      variant="secondary"
                      className="bg-background/60 text-[8px] font-normal text-muted-foreground"
                    >
                      {CAPABILITY_LABELS[c]}
                    </Badge>
                  ))}
                </div>

                {/* P60/P71.9c — the agent's own ACP config option is the only
                    model surface here. AgentCowork ships no model catalogue of
                    its own to fall back to (ADR-0005 §4). */}
                <div className="mb-2 rounded-md border border-emerald-500/20 bg-emerald-500/5 px-2 py-2 text-[10px] leading-relaxed text-emerald-100/80">
                    <div className="mb-1 flex items-center gap-1.5 font-mono text-[9px] uppercase tracking-wider text-emerald-300/90">
                      <KeyRound className="h-2.5 w-2.5" />
                      {externalModelOption ? `Model managed by ${agent.name}` : `${agent.name} owns its model configuration`}
                    </div>
                    {externalModelOption ? (
                      <div className="flex items-center justify-between gap-2">
                        <span className="truncate text-foreground">{externalModelOption.name}: {externalModelLabel}</span>
                        {externalModelOption.type === 'select' && (externalModelOption.options?.length ?? 0) > 0 && (
                          <select
                            aria-label={`${agent.name} model`}
                            value={String(externalModelOption.currentValue)}
                            onChange={(e) => {
                              const handle = useAppStore.getState().acpHandles[selectedAgentId]
                              if (!handle) return
                              void import('@/lib/acp').then(({ acpSessionSetConfigOption }) =>
                                acpSessionSetConfigOption(handle, externalModelOption.id, e.target.value),
                              ).then((next) => setAcpConfigOptions(selectedAgentId, next))
                                .catch((e) => notify(e instanceof Error ? e.message : 'Could not change the agent-owned model', 'error'))
                            }}
                            className="h-6 max-w-[12rem] rounded border border-border bg-background px-1 font-mono text-[9px] text-foreground"
                          >
                            {externalModelOption.options?.map((o) => (
                              <option key={String(o.value)} value={String(o.value)}>{o.name}</option>
                            ))}
                          </select>
                        )}
                      </div>
                    ) : (
                      <span>No ACP model option is exposed. AgentCowork will not show or inject its Native BYOK/local models here.</span>
                    )}
                  </div>


                <div className="mt-1.5 flex items-center gap-1 px-1 font-mono text-[9px] text-muted-foreground/60">
                  <Sparkles className="h-2.5 w-2.5" />
                  Selected: {agent.name} ·{' '}
                  {!agentUsable
                    ? 'not installed'
                    : externalModelOption
                      ? `agent-owned · ${externalModelLabel}`
                      : `managed by ${agent.name}`}
                </div>

                {/* Install + connect (F8/J17) — one click, then use */}
                <div className="mt-2 space-y-1.5 border-t border-border/60 pt-2">
                  {agentUsable ? (
                    <div className="flex items-center justify-between px-1">
                      <span className="flex items-center gap-1 font-mono text-[9px] text-emerald-400">
                        <Check className="h-2.5 w-2.5" />
                        {agent.status === 'discovered' ? 'discovered · launchable' : 'installed'}
                        {agent.version ? ` · v${agent.version}` : ''}
                      </span>
                      <span className="font-mono text-[9px] text-muted-foreground/60">
                        {agent.vendor}
                      </span>
                    </div>
                  ) : (
                    <div className="flex items-center gap-1.5">
                      {agent.status === 'discovered' && (
                        <span className="font-mono text-[9px] text-sky-300">
                          discovered · launch adapter unavailable
                        </span>
                      )}
                      <Button
                        size="sm"
                        disabled={installing}
                        className="h-6 gap-1 bg-sky-500 px-2.5 text-[10px] text-white hover:bg-sky-600"
                        onClick={() => installAgent(agent.id)}
                      >
                        {installing ? (
                          <>
                            <Loader2 className="h-3 w-3 animate-spin" />
                            installing…
                          </>
                        ) : (
                          <>
                            <Download className="h-3 w-3" />
                            Install
                          </>
                        )}
                      </Button>
                      <span className="font-mono text-[9px] text-muted-foreground/60">
                        {agent.note ?? 'Fetch from the ACP registry'}
                      </span>
                    </div>
                  )}

                  {connected ? (
                        <div className="flex items-center gap-1.5 px-1">
                          <span className="flex items-center gap-1 font-mono text-[9px] text-emerald-400">
                            <Check className="h-2.5 w-2.5" />
                            connected
                          </span>
                          <span className="font-mono text-[9px] text-muted-foreground/50">
                            {connected.slice(0, 10)}…
                          </span>
                        </div>
                      ) : auth ? (
                        <div className="space-y-1 rounded-md border border-sky-500/30 bg-sky-500/5 p-2">
                          <div className="flex items-center gap-1 font-mono text-[9px] text-sky-300">
                            <KeyRound className="h-2.5 w-2.5" />
                            {auth.waitingUrl ? 'Waiting for sign-in…' : `${agent.name} needs sign-in`}
                          </div>
                          {auth.methods.map((m) => (
                            <div key={m.id} className="flex items-center gap-1.5">
                              <Button
                                size="sm"
                                className="h-5 gap-1 px-2 text-[9px]"
                                onClick={() => signIn(m.id)}
                              >
                                {authMethodLabel(m)}
                              </Button>
                              {m.description && (
                                <span className="truncate font-mono text-[8px] text-muted-foreground/60">
                                  {m.description}
                                </span>
                              )}
                            </div>
                          ))}
                          {auth.waitingUrl && (
                            <Button
                              size="sm"
                              variant="outline"
                              className="h-5 gap-1 px-2 text-[9px]"
                              onClick={() => signIn(auth.methods[0]?.id ?? '')}
                            >
                              <RotateCw className="h-2.5 w-2.5" />
                              I finished sign-in — retry
                            </Button>
                          )}
                        </div>
                      ) : (
                        <div className="flex items-center gap-1.5">
                          <Button
                            size="sm"
                            variant="outline"
                            disabled={connecting}
                            className="h-6 gap-1 px-2.5 text-[10px]"
                            onClick={() => connectAgent(agent.id)}
                          >
                            {connecting ? (
                              <Loader2 className="h-3 w-3 animate-spin" />
                            ) : (
                              <KeyRound className="h-3 w-3 text-sky-400" />
                            )}
                            {connecting ? 'connecting…' : 'Connect'}
                          </Button>
                          <span className="font-mono text-[9px] text-muted-foreground/50">
                            Sign-in appears only if this agent asks for it
                          </span>
                        </div>
                      )}
                </div>
              </div>
            </div>
          </div>
        </>
      )}
    </div>
  )
}
