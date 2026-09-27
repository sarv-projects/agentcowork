'use client'

import { useEffect, useState } from 'react'
import { motion, AnimatePresence, MotionConfig, useReducedMotion } from 'framer-motion'
import {
  Sparkles,
  FileSpreadsheet,
  Globe,
  Monitor,
  Brain,
  ShieldCheck,
  Palette,
  Sun,
  Moon,
  KeyRound,
  Check,
  CheckCircle2,
  ArrowRight,
  ArrowLeft,
  Download,
  RefreshCw,
  Layers,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { useAppStore } from '@/lib/store'
import { ACCENT_PRESETS, useTheme } from '@/components/theme-provider'
import { inTauri, invoke } from '@/lib/tauri'
import { useRuntimeState } from '@/lib/runtime'
import {
  acpInstallStatus,
  acpInstallRequest,
  acpInstallCommit,
  acpInstallAwait,
  acpIdFor,
  chiefDefaultSet,
  isAgentReady,
  readinessLabel,
  type AgentReadiness,
} from '@/lib/acp'
import { AGENTS } from '@/lib/agents'
import { cn } from '@/lib/utils'
import { FIRST_TASKS } from '@/lib/first-run'

type VaultSetupReceipt = { ok?: boolean; needsSetup?: boolean }
type VaultInvoker = <T>(command: string, args?: Record<string, unknown>) => Promise<T>

export type OnboardingVaultPath =
  | { kind: 'passphrase'; passphrase: string }
  | { kind: 'device-key' }

export interface VaultSetupDependencies {
  inTauri: () => boolean
  invoke: VaultInvoker
}

/**
 * Run the selected vault path, then mark onboarding complete. Keeping completion
 * behind this awaited boundary makes a native rejection unable to persist a
 * false "finished" state. The device-key path verifies an existing usable key;
 * it does not pretend that an absent OS keyring was provisioned.
 */
export async function completeOnboardingAfterVault(
  path: OnboardingVaultPath,
  finish: () => void,
  dependencies: VaultSetupDependencies = { inTauri, invoke },
): Promise<'passphrase' | 'device-key' | 'preview'> {
  if (!dependencies.inTauri()) {
    finish()
    return 'preview'
  }

  if (path.kind === 'device-key') {
    const status = await dependencies.invoke<VaultSetupReceipt>('vault_key_status')
    if (status.ok !== true || status.needsSetup !== false) {
      throw new Error(
        'The device-managed vault key is not available. Enter a master passphrase to create the vault, or restore the device key and retry.',
      )
    }
    finish()
    return 'device-key'
  }

  const result = await dependencies.invoke<VaultSetupReceipt>('vault_setup', {
    passphrase: path.passphrase,
  })
  if (result.ok !== true || result.needsSetup !== false) {
    throw new Error('The desktop did not confirm that vault setup completed.')
  }
  finish()
  return 'passphrase'
}

const BRAND_WORDS = [
  'AgentCowork',
  'EveryAgent',
  'EveryWork',
  'EveryDoc',
  'EveryModel',
  'EveryTask',
]

const CAPABILITIES = [
  {
    icon: FileSpreadsheet,
    title: 'Work-Native Office',
    desc: 'Embedded IronCalc spreadsheet formula DAG recalculation & surgical OOXML document patcher.',
    tag: 'IronCalc 0.8.3',
    color: 'text-emerald-400 bg-emerald-500/10 border-emerald-500/20',
  },
  {
    icon: Globe,
    title: 'Tiered Stealth Browser',
    desc: 'Lightpanda + Chrome CDP browser automation with zero anti-bot detection fingerprints.',
    tag: 'CDP + Stealth',
    color: 'text-sky-400 bg-sky-500/10 border-sky-500/20',
  },
  {
    icon: Monitor,
    title: 'Desktop Computer Use',
    desc: '1000x1000 normalized coordinate grounding, differential vision loop, and OS app launch.',
    tag: 'Win32 / A11y',
    color: 'text-purple-400 bg-purple-500/10 border-purple-500/20',
  },
  {
    icon: Brain,
    title: '5-Tier Cognitive Memory',
    desc: 'ACT-R activation decay, SQLite FTS5 BM25 search, knowledge graph, and instant replay.',
    tag: 'ACT-R + Graph',
    color: 'text-amber-400 bg-amber-500/10 border-amber-500/20',
  },
  {
    icon: ShieldCheck,
    title: '7-Layer Guard-2 Security',
    desc: 'Zero-I/O SSRF netfloor, lexical pathfloor, secret file shield (.env/.pem), and single-use tickets.',
    tag: 'Guard-2 Kernel',
    color: 'text-cyan-400 bg-cyan-500/10 border-cyan-500/20',
  },
  {
    icon: Layers,
    title: 'Universal Agent Swarm',
    desc: 'Host Claude Code, Codex CLI, OpenCode, Aider, Cline in isolated Git worktrees with 3-way merge.',
    tag: 'ACP Harness',
    color: 'text-indigo-400 bg-indigo-500/10 border-indigo-500/20',
  },
]

// P70 — the accent dots read the shared preset list instead of keeping a
// second copy. The duplicate had drifted two presets behind the picker in
// Settings, so half the selectable accents were unreachable from here and
// nothing caught it: the two lists were never compared.

export function OnboardingModal() {
  const onboardingDone = useAppStore((s) => s.onboardingDone)
  const setOnboardingDone = useAppStore((s) => s.setOnboardingDone)
  const notify = useAppStore((s) => s.notify)
  const closeSetup = useAppStore((s) => s.closeSetup)
  const setCenterScreen = useAppStore((s) => s.setCenterScreen)
  const setComposerValue = useAppStore((s) => s.setComposerValue)
  const newSession = useAppStore((s) => s.newSession)
  const runtime = useRuntimeState()
  const reduceMotion = useReducedMotion()
  const { theme, setTheme, accent, setAccent } = useTheme()

  const [step, setStep] = useState(0)
  const [brandIndex, setBrandIndex] = useState(0)

  // Agent auto-detection state. P71.2a — nothing is pre-marked ready: v1 ships
  // no built-in agent, so an empty map is the honest starting state and only a
  // real discovery result may fill it.
  const [detectedAgents, setDetectedAgents] = useState<Record<string, boolean>>({})
  // P71.6a — discovery is not binding. A detected CLI cannot serve a turn until
  // the user binds it, so this step tracks the binding, not the detection.
  const [boundId, setBoundId] = useState<string | null>(null)
  const [bindingAgent, setBindingAgent] = useState<string | null>(null)
  const liveAgents = useAppStore((s) => s.liveAgents)
  const [scanning, setScanning] = useState(false)
  const [installingAgent, setInstallingAgent] = useState<string | null>(null)
  const [firstTaskDraft, setFirstTaskDraft] = useState(FIRST_TASKS[0]?.prompt ?? '')

  // Optional Passphrase state
  const [passphrase, setPassphrase] = useState('')
  const [confirmPassphrase, setConfirmPassphrase] = useState('')
  const [passError, setPassError] = useState<string | null>(null)
  const [vaultError, setVaultError] = useState<string | null>(null)
  const [vaultBusy, setVaultBusy] = useState(false)

  // Cycle brand title animation on Step 0
  useEffect(() => {
    if (step !== 0) return
    const interval = setInterval(() => {
      setBrandIndex((prev) => (prev + 1) % BRAND_WORDS.length)
    }, 2000)
    return () => clearInterval(interval)
  }, [step])

  // Real-time Agent Auto-Detection via ACP Harness
  const scanAgents = async () => {
    setScanning(true)
    // Never seeded: every entry below is evidence from the shell's discovery.
    const detected: Record<string, boolean> = {}

    if (inTauri()) {
      try {
        const statusMap = await acpInstallStatus()
        for (const ag of AGENTS) {
          const acpId = acpIdFor(ag.id)
          const state = statusMap[acpId] || statusMap[ag.id]
          if (state?.installed || state?.discovered || state?.launchable) {
            detected[ag.id] = true
          }
        }
      } catch {
        // Fallback to local discovery
      }
    } else {
      // Browser preview simulated detection
      detected['claude-code'] = true
      detected['opencode'] = true
    }

    setDetectedAgents(detected)
    setScanning(false)
  }

  useEffect(() => {
    if (step === 2) {
      void scanAgents()
    }
  }, [step])

  if (onboardingDone) return null

  const finish = () => {
    closeSetup()
    setOnboardingDone(true)
    notify(
      boundId
        ? 'Welcome to AgentCowork — your agent is bound and ready.'
        : 'Welcome to AgentCowork. No agent is bound yet, so the first message will ask you to pick one.',
    )
  }

  const boundRuntime = boundId
    ? liveAgents.find((agent) => acpIdFor(agent.id) === boundId)
    : undefined
  const firstTaskReady = Boolean(boundId && isAgentReady(boundRuntime?.readiness as AgentReadiness))

  const startFirstTask = () => {
    if (!firstTaskReady) return
    const prompt = firstTaskDraft.trim() || FIRST_TASKS[0]?.prompt || 'Help me with my first task.'
    if (useAppStore.getState().sessions.length === 0) newSession()
    setComposerValue(prompt)
    setCenterScreen('chat')
    finish()
  }

  // F8 / P69.C12 — the same plan-before-touch handshake the picker and the
  // first-run gate use: request (ticket or auto-allow) → consent in the Guard
  // window → commit. Onboarding must not be a privileged install path that
  // skips the approval card.
  const handleInstallAgent = async (agentId: string) => {
    setInstallingAgent(agentId)
    try {
      if (inTauri()) {
        const acpId = acpIdFor(agentId)
        const req = await acpInstallRequest(acpId)
        if (req.action !== 'allow') {
          const { approved } = await acpInstallAwait(req.ticketId)
          if (!approved) {
            notify('Install was not approved — nothing was downloaded.')
            return
          }
        }
        await acpInstallCommit(acpId, req.ticketId)
        const { refreshAgentCatalog } = await import('@/lib/bridge')
        await refreshAgentCatalog().catch(() => {})
      }
      setDetectedAgents((prev) => ({ ...prev, [agentId]: true }))
      // "Installed" is not "bound": the row still has to be chosen, because an
      // agent is never adopted on the user's behalf (`P71.2a`).
      notify(`${agentId} installed — choose it as your agent to start.`)
    } catch (e) {
      notify(e instanceof Error ? e.message : `Install note: install CLI globally via terminal, or use auto-detected path.`)
    } finally {
      setInstallingAgent(null)
    }
  }

  // P71.6a — binding is the v1 replacement for "the built-in engine is always
  // there". The shell refuses an unknown/uninstalled id fail-closed, so a
  // refusal surfaces instead of silently leaving no agent bound.
  const handleBindAgent = async (agentId: string) => {
    setBindingAgent(agentId)
    try {
      const binding = acpIdFor(agentId)
      await chiefDefaultSet(binding)
      useAppStore.getState().setSelectedAgent(agentId)
      useAppStore.getState().setUserDefaultChief(binding)
      setBoundId(binding)
      notify(`${agentId} bound — your messages will run under it.`)
    } catch (e) {
      notify(e instanceof Error ? e.message : `Could not bind ${agentId}`)
    } finally {
      setBindingAgent(null)
    }
  }

  const handleSetPassphrase = async (skip = false) => {
    setPassError(null)
    setVaultError(null)
    if (!skip) {
      if (passphrase.length > 0 && passphrase.length < 8) {
        setPassError('Passphrase must be at least 8 characters')
        return
      }
      if (passphrase !== confirmPassphrase) {
        setPassError('Passphrases do not match')
        return
      }
    }

    setVaultBusy(true)
    try {
      const outcome = await completeOnboardingAfterVault(
        skip ? { kind: 'device-key' } : { kind: 'passphrase', passphrase },
        finish,
      )
      if (outcome === 'device-key') {
        notify('Device-managed vault key verified.')
      } else if (outcome === 'passphrase') {
        notify('Master passphrase set. Vault encrypted.')
      } else {
        // Browser preview has no vault to create or keychain to exercise. This
        // completes only the preview walkthrough; it is not native evidence.
        notify('Preview mode — no desktop vault was changed.')
      }
    } catch (error) {
      const detail = error instanceof Error && error.message
        ? error.message
        : 'The desktop shell rejected vault setup.'
      setVaultError(`Vault setup failed: ${detail}`)
      notify(`Vault setup failed: ${detail}`, 'error')
    } finally {
      setVaultBusy(false)
    }
  }

  return (
    <MotionConfig reducedMotion="user">
      <Dialog open onOpenChange={() => {}}>
      <DialogContent
        data-first-run-owner="onboarding"
        onEscapeKeyDown={(event) => { event.preventDefault(); event.stopPropagation() }}
        className="max-h-[calc(100vh-2rem)] max-w-2xl gap-0 overflow-x-hidden overflow-y-auto border border-border/70 bg-card/95 p-0 shadow-2xl backdrop-blur-xl"
        showCloseButton={false}
      >
        {/* Progress Bar Top */}
        <div className="flex h-1.5 w-full bg-muted/40">
          {[0, 1, 2, 3].map((i) => (
            <div
              key={i}
              className={cn(
                'h-full flex-1 transition-all duration-500 ease-out',
                i <= step ? 'bg-gradient-to-r from-brand to-primary' : 'bg-transparent',
              )}
            />
          ))}
        </div>

        <div className="flex min-h-[460px] flex-col justify-between p-6 sm:p-8">
          <AnimatePresence mode="wait">
            {/* STEP 0: Welcome & Dynamic Brand Animation */}
            {step === 0 && (
              <motion.div
                key="step-0"
                initial={{ opacity: 0, y: 12 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -12 }}
                transition={reduceMotion ? { duration: 0 } : { duration: 0.3 }}
                className="flex flex-1 flex-col items-center justify-center text-center"
              >
                <div className="mb-4 inline-flex items-center gap-2 rounded-full border border-brand/30 bg-brand/10 px-3 py-1 text-[11px] font-medium text-brand shadow-sm">
                  <Sparkles className="h-3.5 w-3.5 animate-pulse" />
                  <span>The Universal Desktop Agentic OS</span>
                </div>

                <div className="my-3 flex h-14 items-center justify-center">
                  <AnimatePresence mode="wait">
                    <motion.h1
                      key={BRAND_WORDS[brandIndex]}
                      initial={{ opacity: 0, y: 16, filter: 'blur(4px)' }}
                      animate={{ opacity: 1, y: 0, filter: 'blur(0px)' }}
                      exit={{ opacity: 0, y: -16, filter: 'blur(4px)' }}
                      transition={reduceMotion ? { duration: 0 } : { duration: 0.35, ease: 'easeOut' }}
                      className="bg-gradient-to-r from-foreground via-foreground/90 to-brand bg-clip-text text-4xl font-extrabold tracking-tight text-transparent sm:text-5xl"
                    >
                      {BRAND_WORDS[brandIndex]}
                    </motion.h1>
                  </AnimatePresence>
                </div>

                <p className="max-w-md text-sm leading-relaxed text-muted-foreground sm:text-base">
                  An AI coworker on <span className="font-semibold text-foreground">your computer</span>. It uses your files, apps, browser, documents, and tools to finish real work—while you approve anything that changes the world.
                </p>

                <div
                  role="status"
                  className="mx-auto mt-5 flex max-w-lg items-start gap-2 rounded-lg border border-border/70 bg-background/50 px-3 py-2 text-left"
                >
                  <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0 text-brand" aria-hidden="true" />
                  <div className="text-[11px] leading-relaxed text-muted-foreground">
                    <span className="font-medium text-foreground">Safety state: </span>
                    {runtime.status === 'preview'
                      ? 'Preview only — no external agent or vault can run here.'
                      : runtime.status === 'vault-locked' || runtime.status === 'vault-setup'
                        ? 'The vault still needs attention before work can start.'
                        : runtime.status === 'live'
                          ? 'The desktop is live. Guard still asks before consequential effects.'
                          : 'The desktop is starting. No agent is claimed ready until the shell reports it.'}
                  </div>
                </div>

                <details className="mt-8 w-full max-w-lg rounded-xl border border-border/60 bg-card/40 px-3 py-2 text-left">
                  <summary className="cursor-pointer list-none text-[11px] font-medium text-foreground marker:hidden focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60">
                    See what AgentCowork can do later
                  </summary>
                  <div className="mt-3 grid grid-cols-1 gap-3 sm:grid-cols-3">
                  <div className="rounded-xl border border-border/60 bg-card/60 p-3 text-left">
                    <div className="text-xs font-semibold text-foreground">100% Local-First</div>
                    <div className="mt-0.5 text-[10px] text-muted-foreground">SQLCipher AES-256 vault. Zero founder servers.</div>
                  </div>
                  <div className="rounded-xl border border-border/60 bg-card/60 p-3 text-left">
                    <div className="text-xs font-semibold text-foreground">Any Model & Agent</div>
                    <div className="mt-0.5 text-[10px] text-muted-foreground">BYOK, offline Ollama/GGUF, Claude Code, Codex.</div>
                  </div>
                  <div className="rounded-xl border border-border/60 bg-card/60 p-3 text-left">
                    <div className="text-xs font-semibold text-foreground">Guard-2 Safety</div>
                    <div className="mt-0.5 text-[10px] text-muted-foreground">No file changes or deletions without consent.</div>
                  </div>
                  </div>
                </details>
              </motion.div>
            )}

            {/* STEP 1: Capabilities Showcase & Theme Selection */}
            {step === 1 && (
              <motion.div
                key="step-1"
                initial={{ opacity: 0, y: 12 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -12 }}
                transition={reduceMotion ? { duration: 0 } : { duration: 0.3 }}
                className="flex flex-1 flex-col space-y-4"
              >
                <div>
                  <h2 className="text-xl font-bold tracking-tight text-foreground">Capabilities & Appearance</h2>
                  <p className="text-xs text-muted-foreground">Explore advanced capabilities and tune your cockpit later.</p>
                </div>

                {/* Capabilities Grid */}
                <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-3">
                  {CAPABILITIES.map((cap) => (
                    <div
                      key={cap.title}
                      className={cn(
                        'group flex flex-col justify-between rounded-lg border p-2.5 transition-all hover:scale-[1.02]',
                        cap.color,
                      )}
                    >
                      <div>
                        <div className="flex items-center justify-between">
                          <cap.icon className="h-4 w-4" />
                          <span className="font-mono text-[9px] uppercase tracking-wider opacity-80">{cap.tag}</span>
                        </div>
                        <div className="mt-1.5 text-xs font-semibold text-foreground">{cap.title}</div>
                        <p className="mt-0.5 text-[10px] leading-tight text-muted-foreground">{cap.desc}</p>
                      </div>
                    </div>
                  ))}
                </div>

                {/* Theme & Accent Switcher */}
                <div className="rounded-xl border border-border/70 bg-card/50 p-3.5">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <div className="flex items-center gap-2">
                      <Palette className="h-4 w-4 text-brand" />
                      <div>
                        <div className="text-xs font-medium text-foreground">Theme & Accent Tone</div>
                        <div className="text-[10px] text-muted-foreground">Select your cockpit aesthetic.</div>
                      </div>
                    </div>

                    <div className="flex items-center gap-3">
                      {/* Theme mode buttons */}
                      <div className="flex rounded-md border border-border/60 bg-background/80 p-0.5">
                        <button
                          type="button"
                          onClick={() => setTheme('dark')}
                          className={cn(
                            'flex items-center gap-1 rounded px-2 py-1 text-[10px] font-medium transition-colors',
                            theme === 'dark' ? 'bg-accent text-foreground' : 'text-muted-foreground hover:text-foreground',
                          )}
                        >
                          <Moon className="h-3 w-3" /> Dark
                        </button>
                        <button
                          type="button"
                          onClick={() => setTheme('light')}
                          className={cn(
                            'flex items-center gap-1 rounded px-2 py-1 text-[10px] font-medium transition-colors',
                            theme === 'light' ? 'bg-accent text-foreground' : 'text-muted-foreground hover:text-foreground',
                          )}
                        >
                          <Sun className="h-3 w-3" /> Light
                        </button>
                      </div>

                      {/* Accent color dots — the swatch is icon-only, so each
                          button carries an accessible name and announces its
                          pressed state; the ring (not the hue) marks the pick. */}
                      <div className="flex items-center gap-1.5" role="group" aria-label="Accent color">
                        {ACCENT_PRESETS.map((acc) => (
                          <button
                            key={acc.id}
                            type="button"
                            onClick={() => setAccent(acc.id)}
                            title={acc.label}
                            aria-label={acc.label}
                            aria-pressed={accent === acc.id}
                            className={cn(
                              'size-6 rounded-full transition-transform',
                              acc.swatch,
                              accent === acc.id
                                ? 'scale-110 ring-2 ring-foreground/40 ring-offset-1 ring-offset-background'
                                : 'opacity-70 hover:opacity-100',
                            )}
                          />
                        ))}
                      </div>
                    </div>
                  </div>
                </div>
              </motion.div>
            )}

            {/* STEP 2: Agent Discovery & Auto-Detection */}
            {step === 2 && (
              <motion.div
                key="step-2"
                initial={{ opacity: 0, y: 12 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -12 }}
                transition={reduceMotion ? { duration: 0 } : { duration: 0.3 }}
                className="flex flex-1 flex-col space-y-4"
              >
                <div className="flex items-center justify-between">
                  <div>
                    <h2 className="text-xl font-bold tracking-tight text-foreground">Pick the agent that will do the thinking</h2>
                    <p className="text-xs text-muted-foreground">
                      AgentCowork ships no built-in engine: it discovers the agent CLIs on this machine,
                      and the one you bind runs your messages. Detection is evidence, not readiness.
                    </p>
                  </div>
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={scanAgents}
                    disabled={scanning}
                    className="h-7 text-xs"
                  >
                    <RefreshCw className={cn('mr-1.5 h-3 w-3', scanning && 'animate-spin')} />
                    {scanning ? 'Scanning…' : 'Rescan System'}
                  </Button>
                </div>

                {/* P71.6a — the honest finish condition: nothing is bound, so no
                    turn can run yet. Finishing is allowed (the app is still
                    usable), but the consequence is stated here, not discovered
                    as a refusal on the first message. */}
                {!boundId && (
                  <div className="rounded-lg border border-dashed border-border/60 bg-card/40 px-3 py-2 text-[11px] leading-relaxed text-muted-foreground">
                    No agent is bound yet — nothing can answer until one is. You can finish and pick
                    one later; the first message will ask instead of failing.
                  </div>
                )}

                <div className="max-h-[260px] space-y-2 overflow-y-auto pr-1">
                  {AGENTS.map((ag) => {
                    // P58.6/P71.3f — the live catalog is the only source of
                    // occupancy and readiness; the static seed is a fixture and
                    // never paints a runtime as installed or ready.
                    const live = liveAgents.find((a) => a.id === ag.id)
                    const isDetected = detectedAgents[ag.id] || live !== undefined
                    const readiness = live?.readiness as AgentReadiness | undefined
                    const ready = isAgentReady(readiness as AgentReadiness)
                    const isBound = boundId === acpIdFor(ag.id)
                    const isBusy = installingAgent === ag.id || bindingAgent === ag.id

                    return (
                      <div
                        key={ag.id}
                        className={cn(
                          'flex items-center justify-between rounded-lg border p-3 transition-colors',
                          isDetected
                            ? 'border-emerald-500/30 bg-emerald-500/5'
                            : 'border-border/60 bg-card/40 hover:bg-accent/20',
                        )}
                      >
                        <div className="flex items-center gap-3">
                          <div className={cn('flex h-8 w-8 shrink-0 items-center justify-center rounded-md font-bold text-xs shadow-sm', ag.accent)}>
                            {ag.mark}
                          </div>
                          <div>
                            <div className="flex items-center gap-2">
                              <span className="text-xs font-semibold text-foreground">{ag.name}</span>
                              <span className="text-[10px] text-muted-foreground font-mono">({ag.vendor})</span>
                            </div>
                            <p className="text-[10px] text-muted-foreground line-clamp-1">{ag.tagline}</p>
                          </div>
                        </div>

                        <div className="flex items-center gap-2">
                          {isDetected && (
                            <span
                              className={cn(
                                'text-[10px] font-mono',
                                ready ? 'text-emerald-400' : 'text-muted-foreground',
                              )}
                            >
                              {readinessLabel(readiness)}
                            </span>
                          )}
                          {isBound ? (
                            <Badge className="bg-emerald-500/15 text-emerald-400 border border-emerald-500/20 text-[10px]">
                              <CheckCircle2 className="mr-1 h-3 w-3" /> Your agent
                            </Badge>
                          ) : ready ? (
                            <Button
                              size="sm"
                              onClick={() => void handleBindAgent(ag.id)}
                              disabled={isBusy}
                              className="h-7 bg-brand text-[11px] text-black hover:bg-brand"
                            >
                              <Check className="mr-1 h-3 w-3" />
                              {isBusy ? 'Binding…' : 'Use this agent'}
                            </Button>
                          ) : (
                            <Button
                              size="sm"
                              variant="outline"
                              onClick={() => void handleInstallAgent(ag.id)}
                              disabled={isBusy}
                              className="h-7 text-[11px]"
                            >
                              <Download className="mr-1 h-3 w-3" />
                              {isBusy ? 'Setting up…' : 'Install / Link'}
                            </Button>
                          )}
                        </div>
                      </div>
                    )
                  })}
                </div>

                {firstTaskReady && (
                  <div
                    data-testid="onboarding-first-task"
                    className="rounded-xl border border-brand/35 bg-brand/5 p-3"
                  >
                    <div className="flex items-start gap-2">
                      <Sparkles className="mt-0.5 h-4 w-4 shrink-0 text-brand" aria-hidden="true" />
                      <div className="min-w-0 flex-1">
                        <div className="text-xs font-semibold text-foreground">Your first task is ready</div>
                        <p className="mt-0.5 text-[10px] leading-relaxed text-muted-foreground">
                          Keep it small. You can change the wording before the first send; the agent owns the work, while Guard explains anything consequential.
                        </p>
                      </div>
                    </div>
                    <label htmlFor="onboarding-first-task" className="mt-3 block text-[10px] font-medium text-foreground">
                      What should it do first?
                    </label>
                    <Input
                      id="onboarding-first-task"
                      value={firstTaskDraft}
                      onChange={(event) => setFirstTaskDraft(event.target.value)}
                      className="mt-1 h-8 text-xs"
                      placeholder="Describe one small outcome…"
                    />
                    <div className="mt-2 flex flex-wrap gap-1.5">
                      {FIRST_TASKS.slice(0, 3).map((task) => (
                        <button
                          key={task.id}
                          type="button"
                          onClick={() => setFirstTaskDraft(task.prompt)}
                          className="rounded-full border border-border bg-background/60 px-2 py-1 text-[10px] text-muted-foreground hover:border-brand/40 hover:bg-accent/40 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
                        >
                          {task.label}
                        </button>
                      ))}
                    </div>
                  </div>
                )}

                <div className="flex items-start gap-2 rounded-lg border border-border/60 bg-background/40 px-3 py-2 text-[10px] leading-relaxed text-muted-foreground">
                  <ShieldCheck className="mt-0.5 h-3.5 w-3.5 shrink-0 text-brand" aria-hidden="true" />
                  <span>
                    {boundId && !firstTaskReady
                      ? 'The binding is recorded, but the shell has not confirmed this agent is runnable yet. Finish its sign-in or rescan before sending.'
                      : 'Every consequential action still passes the desktop safety gate. The agent keeps its own account; AgentCowork does not claim a model is ready before the shell verifies it.'}
                  </span>
                </div>
              </motion.div>
            )}

            {/* STEP 3: Security & Passphrase (deferred until requested) */}
            {step === 3 && (
              <motion.div
                key="step-3"
                initial={{ opacity: 0, y: 12 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -12 }}
                transition={reduceMotion ? { duration: 0 } : { duration: 0.3 }}
                className="flex flex-1 flex-col space-y-4"
              >
                <div>
                  <h2 className="text-xl font-bold tracking-tight text-foreground">Finish safety setup</h2>
                  <p className="text-xs text-muted-foreground">The vault passphrase from the first screen is what unlocks this device. This step does not replace it with a silent key.</p>
                </div>

                <div className="rounded-xl border border-border/70 bg-card/50 p-4 space-y-3">
                  <div className="flex items-start gap-3">
                    <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-brand/10 text-brand">
                      <KeyRound className="h-4 w-4" />
                    </div>
                    <div>
                      <div className="text-xs font-semibold text-foreground">Choose Your Protection Level</div>
                      <p className="text-[11px] text-muted-foreground">
                        Leave this blank if you already created the vault passphrase on the first screen. A second passphrase here only wraps that vault. It does not sign an agent in, and it does not invent a device key.
                      </p>
                    </div>
                  </div>

                  <div className="space-y-2 pt-2">
                    <div>
                      <label className="mb-1 block text-[10px] uppercase tracking-wider text-muted-foreground">
                        Master Passphrase (Optional)
                      </label>
                      <Input
                        type="password"
                        placeholder="Leave blank — the vault passphrase already set is enough"
                        value={passphrase}
                        onChange={(e) => {
                          setPassphrase(e.target.value)
                          setVaultError(null)
                        }}
                        className="h-8 font-mono text-xs"
                      />
                    </div>

                    {passphrase.length > 0 && (
                      <div>
                        <label className="mb-1 block text-[10px] uppercase tracking-wider text-muted-foreground">
                          Confirm Passphrase
                        </label>
                        <Input
                          type="password"
                          placeholder="Re-enter passphrase"
                          value={confirmPassphrase}
                          onChange={(e) => {
                            setConfirmPassphrase(e.target.value)
                            setVaultError(null)
                          }}
                          className="h-8 font-mono text-xs"
                        />
                      </div>
                    )}

                    {passError && <p className="text-[10px] text-red-400 font-mono">{passError}</p>}
                    {vaultError && (
                      <div
                        role="alert"
                        aria-live="assertive"
                        className="rounded-lg border border-red-500/30 bg-red-500/5 px-3 py-2 text-[10px] leading-relaxed text-red-300"
                      >
                        <p className="font-mono">{vaultError}</p>
                        <p className="mt-1 text-muted-foreground">
                          Onboarding was not completed. Retry setup; if a custom passphrase still
                          fails, check the vault folder permissions and available disk space.
                        </p>
                      </div>
                    )}
                  </div>
                </div>

                <div className="rounded-lg border border-emerald-500/20 bg-emerald-500/5 p-3 flex items-center gap-2 text-emerald-400 text-xs">
                  <ShieldCheck className="h-4 w-4 shrink-0" />
                  <span>Guard remains the approval boundary: routine work can proceed, while consequential actions still ask.</span>
                </div>
              </motion.div>
            )}
          </AnimatePresence>

          {/* Navigation Controls Bottom */}
          <div className="mt-6 flex items-center justify-between border-t border-border/60 pt-4">
            <div>
              {step > 0 && (
                <Button variant="ghost" size="sm" onClick={() => setStep(step - 1)}>
                  <ArrowLeft className="mr-1.5 h-3.5 w-3.5" />
                  Back
                </Button>
              )}
            </div>

            <div className="flex items-center gap-2">
              {step === 0 && (
                <>
                  <Button variant="ghost" size="sm" onClick={() => setStep(1)} className="text-muted-foreground">
                    Customize later
                  </Button>
                  <Button
                    size="sm"
                    data-testid="onboarding-find-agent"
                    onClick={() => setStep(2)}
                    className="bg-brand text-black hover:bg-brand/90 font-medium"
                  >
                    Find my agent
                    <ArrowRight className="ml-1.5 h-3.5 w-3.5" />
                  </Button>
                </>
              )}
              {step === 1 && (
                <>
                  <Button variant="ghost" size="sm" onClick={finish} className="text-muted-foreground">
                    Do this later
                  </Button>
                  <Button size="sm" onClick={() => setStep(2)} className="bg-brand text-black hover:bg-brand/90 font-medium">
                    Continue to agent
                    <ArrowRight className="ml-1.5 h-3.5 w-3.5" />
                  </Button>
                </>
              )}
              {step === 2 && firstTaskReady && (
                <>
                  <Button variant="ghost" size="sm" onClick={() => setStep(3)} className="text-muted-foreground">
                    Set up vault later
                  </Button>
                  <Button
                    size="sm"
                    data-testid="onboarding-start-task"
                    onClick={startFirstTask}
                    className="bg-brand text-black hover:bg-brand/90 font-semibold"
                  >
                    Start first task
                    <ArrowRight className="ml-1.5 h-3.5 w-3.5" />
                  </Button>
                </>
              )}
              {step === 2 && !firstTaskReady && (
                <>
                  <Button variant="ghost" size="sm" onClick={finish} className="text-muted-foreground">
                    I&apos;ll choose later
                  </Button>
                  <Button size="sm" onClick={() => setStep(3)} className="bg-brand text-black hover:bg-brand/90 font-medium">
                    Review safety setup
                    <ArrowRight className="ml-1.5 h-3.5 w-3.5" />
                  </Button>
                </>
              )}
              {step === 3 && (
                <Button
                  size="sm"
                  disabled={vaultBusy}
                  onClick={() => void handleSetPassphrase(passphrase.length === 0)}
                  className="bg-brand text-black hover:bg-brand/90 font-semibold"
                >
                  <Check className="mr-1.5 h-3.5 w-3.5" />
                  {vaultBusy
                    ? 'Setting up…'
                    : vaultError
                      ? 'Retry Setup'
                      : passphrase.length === 0
                        ? 'Start (Use Device Key)'
                        : 'Set Passphrase & Start'}
                </Button>
              )}
            </div>
          </div>
        </div>
      </DialogContent>
      </Dialog>
    </MotionConfig>
  )
}

