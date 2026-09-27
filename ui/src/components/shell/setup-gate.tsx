'use client'

/**
 * P71.6a / P71.9b — First-run gate: **bind an agent, because v1 has no engine.**
 *
 * The old gate completed a "zero-install" path: paste a cloud key or start a
 * local runtime, and chat worked because AgentCowork ran the model itself. `P71.2c`
 * removed that loop and `P71.2d` removed the inference path, so the honest
 * first-run step is now the one that was previously optional: install or pick an
 * **external agent** and bind it. Zero-install returns with the post-v1 governed
 * baseline binding (`P71.7`).
 *
 * What this gate must never do is imply chat will work without an agent. The
 * key vault (`Settings → Providers`) and the local-runtime catalogue
 * (`Settings → Local models`) are still real surfaces, but they are
 * **observation**: keys are handed to the agent you bind, and AgentCowork makes no
 * model call of its own.
 */

import { useCallback, useEffect, useState } from 'react'
import { ArrowRight, Check, CheckCircle2, Loader2, Lock, ShieldCheck, Sparkles } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent } from '@/components/ui/dialog'
import { useAppStore, type SettingsSectionId } from '@/lib/store'
import { inTauri } from '@/lib/tauri'
import {
  acpIdFor,
  governanceLabel,
  isAgentReady,
  readinessLabel,
  type AgentReadiness,
} from '@/lib/acp'
import { refreshAgentCatalog } from '@/lib/bridge'
import { AGENTS, type AgentRuntime } from '@/lib/agents'
import { cn } from '@/lib/utils'

function readinessTone(r: AgentReadiness | undefined): string {
  if (isAgentReady(r as AgentReadiness)) return 'text-emerald-400'
  if (r === 'auth_required' || r === 'authenticating' || r === 'launchable') return 'text-warning'
  return 'text-muted-foreground'
}

export function SetupGate() {
  const setupOpen = useAppStore((s) => s.setupOpen)
  const closeSetup = useAppStore((s) => s.closeSetup)
  const setCenterScreen = useAppStore((s) => s.setCenterScreen)
  const setSettingsSection = useAppStore((s) => s.setSettingsSection)
  const notify = useAppStore((s) => s.notify)

  const [rows, setRows] = useState<AgentRuntime[]>([])
  const [discovering, setDiscovering] = useState(false)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [bound, setBound] = useState<AgentRuntime | null>(null)
  const [probeError, setProbeError] = useState<string | null>(null)

  const inShell = inTauri()

  const discover = useCallback(async () => {
    setDiscovering(true)
    setProbeError(null)
    try {
      await refreshAgentCatalog()
      // Read the republished list: the merge (seed + registry + install records)
      // is owned by `bridge`, so this screen never composes its own catalog.
      setRows(useAppStore.getState().liveAgents)
      const state = useAppStore.getState()
      const requestedId = state.selectedAgentId || state.userDefaultChief
      const requested = requestedId
        ? state.liveAgents.find((agent) => acpIdFor(agent.id) === requestedId || agent.id === requestedId)
        : undefined
      if (requested && isAgentReady(requested.readiness as AgentReadiness)) setBound(requested)
    } catch (e) {
      setRows([])
      setProbeError(
        e instanceof Error
          ? `Agent discovery failed: ${e.message}`
          : 'Agent discovery failed — the shell did not answer.',
      )
    } finally {
      setDiscovering(false)
    }
  }, [])

  useEffect(() => {
    if (!setupOpen) return
    setError(null)
    setBound(null)
    void discover()
  }, [setupOpen, discover])

  const bind = async (row: AgentRuntime) => {
    setBusyId(row.id)
    setError(null)
    try {
      const binding = acpIdFor(row.id)
      // The shell refuses an unknown/uninstalled id fail-closed (never a silent
      // fallback to a built-in engine that does not exist).
      const { chiefDefaultSet } = await import('@/lib/acp')
      await chiefDefaultSet(binding)
      useAppStore.getState().setSelectedAgent(row.id)
      useAppStore.getState().setUserDefaultChief(binding)
      setBound(row)
      notify(`${row.name} bound — this chat runs under that agent`)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not bind that agent')
    } finally {
      setBusyId(null)
    }
  }

  // F8 / P69.C12 — plan-before-touch install with the same Guard-2 handshake the
  // picker uses: request (ticket or auto-allow) → consent in the Guard window →
  // commit. Nothing is downloaded before the commit.
  const install = async (row: AgentRuntime) => {
    setBusyId(row.id)
    setError(null)
    try {
      const { acpInstallRequest, acpInstallCommit, acpInstallAwait } = await import('@/lib/acp')
      const rid = acpIdFor(row.id)
      const req = await acpInstallRequest(rid)
      const licenseNote = req.license ? ` · license: ${req.license}` : ''
      const cmd = (req.exactCommand ?? []).join(' ')
      if (req.consentRequired && cmd) {
        const ok = window.confirm(
          `SEP-1024 exact-command consent\n\nInstall ${row.name}?${licenseNote}\n\n${cmd}${req.preferNative ? '\n\nPrefer verified native artifact.' : ''}`,
        )
        if (!ok) return
      }
      if (req.action !== 'allow') {
        notify(
          `Consent needed${licenseNote} — ${req.reason ?? 'proprietary agent'}; approve the Guard-2 card #${req.ticketId.slice(0, 8)}`,
        )
        const { approved, reason } = await acpInstallAwait(req.ticketId)
        if (!approved) {
          setError(reason ?? 'Install was not approved')
          return
        }
      }
      await acpInstallCommit(rid, req.ticketId)
      notify(`${row.name} installed — bind it to start`)
      await discover()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Install failed')
    } finally {
      setBusyId(null)
    }
  }

  // The agent owns its sign-in. Launch it, and if it advertises a URL-based
  // method, open that page; the user finishes there and the row flips to `ready`
  // on the next probe. Nothing here copies a credential into AgentCowork.
  const signIn = async (row: AgentRuntime) => {
    setBusyId(row.id)
    setError(null)
    try {
      const { acpLaunch, acpAuthenticate } = await import('@/lib/acp')
      const info = await acpLaunch(acpIdFor(row.id), '~')
      if (!info.authRequired || info.authMethods.length === 0) {
        notify(`${row.name} is connected — bind it to start`)
        await discover()
        return
      }
      const method = info.authMethods[0]
      if (!method) {
        setError(`${row.name} asks for sign-in but advertises no method`)
        return
      }
      const res = await acpAuthenticate(info.handle, method.id)
      if (res.pending && res.url) {
        window.open(res.url, '_blank')
        notify('Opened the sign-in page — approve there, then re-check.')
      } else if (res.ok || res.sessionId) {
        notify(`${row.name} is signed in — bind it to start`)
      } else {
        notify(`Waiting for ${row.name}'s own login to complete…`)
      }
      await discover()
    } catch (e) {
      setError(e instanceof Error ? e.message : `${row.name} could not be launched`)
    } finally {
      setBusyId(null)
    }
  }

  const startChatting = () => {
    // A blocked send intentionally leaves the user's draft in the composer.
    // Finishing setup should recover that draft, not erase it.
    closeSetup()
    setCenterScreen('chat')
  }

  const openSettings = (section: SettingsSectionId) => {
    closeSetup()
    setSettingsSection(section)
    setCenterScreen('settings')
  }

  if (!setupOpen) return null

  // In the shell an empty list means discovery has not answered — not "nothing is
  // installed" — so the seed is never painted as occupancy. The browser preview
  // has no shell to ask and keeps the labelled fixture.
  const catalog = rows.length > 0 ? rows : inShell ? [] : AGENTS

  return (
    <Dialog open onOpenChange={() => closeSetup()}>
      <DialogContent
        data-first-run-owner="setup"
        aria-label="Agent setup"
        className="max-w-md gap-0 p-0"
      >
        <div className="flex flex-col gap-3 p-6">
          {bound ? (
            <>
              <div className="flex items-center gap-2">
                <CheckCircle2 className="h-5 w-5 text-emerald-400" />
                <h2 className="text-sm font-semibold">{bound.name} is bound</h2>
              </div>
              <p className="text-xs leading-relaxed text-muted-foreground">
                Your messages now run under that agent — it does the reasoning, and AgentCowork
                governs what it may touch. There is no demo reply and no AgentCowork-owned model
                in front of it.
              </p>
              <Button
                className="mt-2 h-8 w-full bg-brand text-xs text-black hover:bg-brand"
                onClick={startChatting}
              >
                <Sparkles className="mr-1 h-3.5 w-3.5" />
                Start chatting
              </Button>
              <button
                type="button"
                onClick={() => openSettings('agents')}
                className="text-[10px] text-muted-foreground underline-offset-2 hover:text-brand hover:underline"
              >
                Review agent settings
              </button>
            </>
          ) : (
            <>
              <div className="flex items-center gap-2">
                <Sparkles className="h-5 w-5 text-brand" />
                <h2 className="text-sm font-semibold">Give it an agent to think with</h2>
              </div>
              <p className="text-xs leading-relaxed text-muted-foreground">
                AgentCowork ships no built-in engine in v1 — the agent you bind does the reasoning and
                holds its own model and credentials, while AgentCowork keeps the workspace, the memory
                and the permission gate. Install or pick one to send your first message.
              </p>

              {!inShell && (
                <div className="rounded-lg border border-dashed border-border/60 bg-card/40 px-3 py-2 text-[11px] leading-relaxed text-muted-foreground">
                  Preview inventory only — no row here is proven runnable. Open the desktop app to
                  discover, install, sign in, and bind an agent.
                </div>
              )}

              {discovering && (
                <div className="flex items-center gap-1.5 px-1 font-mono text-[10px] text-muted-foreground">
                  <Loader2 className="h-3 w-3 animate-spin" />
                  looking for agents on this machine…
                </div>
              )}

              <div className="max-h-64 space-y-1.5 overflow-y-auto">
                {catalog.map((row) => {
                  const readiness = row.readiness
                  const usable = isAgentReady(readiness as AgentReadiness)
                  const needsAuth = readiness === 'auth_required' || readiness === 'authenticating'
                  const busy = busyId === row.id
                  return (
                    <div
                      key={row.id}
                      data-agent-id={row.id}
                      className="rounded-lg border border-border bg-background px-3 py-2.5 text-xs"
                    >
                      <div className="flex items-start justify-between gap-2">
                        <div className="min-w-0">
                          <div className="flex items-center gap-1.5">
                            <span className="truncate font-medium text-foreground">{row.name}</span>
                            <span className={cn('font-mono text-[9px]', readinessTone(readiness))}>
                              {readinessLabel(readiness)}
                            </span>
                          </div>
                          <div className="mt-0.5 truncate text-[10px] text-muted-foreground">
                            {row.tagline || row.note}
                          </div>
                          {row.version && (
                            <div className="truncate font-mono text-[9px] text-muted-foreground/70">
                              {row.vendor} · v{row.version}
                              {row.location ? ` · ${row.location.kind}` : ''}
                            </div>
                          )}
                          {row.governance && (
                            <div
                              className={cn(
                                'mt-0.5 truncate text-[9px] font-medium',
                                row.governance.class === 'GovernedMediated'
                                  ? 'text-emerald-400/90'
                                  : row.governance.class === 'SelfContained'
                                    ? 'text-warning/90'
                                    : 'text-red-400/90',
                              )}
                              title={row.governance.note}
                            >
                              {governanceLabel(row.governance)}
                            </div>
                          )}
                        </div>
                        <div className="flex shrink-0 items-center gap-1">
                          {usable ? (
                            <Button
                              size="sm"
                              className="h-6 bg-brand px-2 text-[10px] text-black hover:bg-brand"
                              disabled={!inShell || busy}
                              onClick={() => void bind(row)}
                            >
                              {busy ? <Loader2 className="h-3 w-3 animate-spin" /> : <Check className="h-3 w-3" />}
                              Use
                            </Button>
                          ) : needsAuth ? (
                            <Button
                              size="sm"
                              variant="outline"
                              className="h-6 px-2 text-[10px]"
                              disabled={!inShell || busy}
                              onClick={() => void signIn(row)}
                            >
                              {busy ? <Loader2 className="h-3 w-3 animate-spin" /> : <Lock className="h-3 w-3" />}
                              Sign in
                            </Button>
                          ) : (
                            <Button
                              size="sm"
                              variant="outline"
                              className="h-6 px-2 text-[10px]"
                              disabled={!inShell || busy}
                              onClick={() => void install(row)}
                            >
                              {busy ? <Loader2 className="h-3 w-3 animate-spin" /> : <ArrowRight className="h-3 w-3" />}
                              Install
                            </Button>
                          )}
                        </div>
                      </div>
                    </div>
                  )
                })}
              </div>

              {catalog.length === 0 && !discovering && (
                <div className="rounded-lg border border-dashed border-border/60 bg-card/40 p-3 text-xs text-muted-foreground">
                  {inShell
                    ? 'No agent discovered yet. Nothing can run a turn until one is installed — AgentCowork has no built-in engine to fall back to.'
                    : 'Preview mode — run inside the desktop shell to discover agents on this machine.'}
                </div>
              )}

              {probeError && <p className="text-[11px] text-warning">{probeError}</p>}
              {error && <p className="text-[11px] text-red-400">{error}</p>}

              <div className="mt-1 flex items-start gap-1.5 rounded border border-dashed border-border/60 px-2 py-1.5 text-[10px] text-muted-foreground">
                <ShieldCheck className="mt-0.5 h-3 w-3 shrink-0 text-emerald-400" />
                <span>
                  No API-key step here: provider keys live in your local vault and are handed to the
                  agent you bind — AgentCowork makes no model call of its own. Providers and Local
                  models in Settings are catalogue, vault and usage surfaces.
                </span>
              </div>

              <div className="mt-1 flex items-center justify-between">
                <button
                  type="button"
                  onClick={() => openSettings('agents')}
                  className="text-[10px] text-muted-foreground underline-offset-2 hover:text-brand hover:underline"
                >
                  Manage agents in settings
                </button>
                <Button variant="ghost" size="sm" onClick={() => closeSetup()}>
                  Explore first
                </Button>
              </div>
            </>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
