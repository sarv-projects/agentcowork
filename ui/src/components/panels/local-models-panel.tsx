'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  AlertCircle,
  BookOpen,
  Check,
  CheckCircle2,
  Cpu,
  Download,
  Eye,
  Gauge,
  HardDrive,
  Pause,
  Play,
  Search,
  Server,
  Sparkles,
  Trash2,
  Wrench,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Progress } from '@/components/ui/progress'
import { Switch } from '@/components/ui/switch'
import { useAppStore } from '@/lib/store'
import {
  readinessLabel,
  type AcpConfigOption,
  type AgentReadiness,
} from '@/lib/acp'
import {
  cpuCores,
  formatBytes,
  formatDownloads,
  getHardware,
  getLocalPrefs,
  getBoundAgentConfigOptions,
  hubCaps,
  listHubFiles,
  listRuntimeInventory,
  listRuntimeModels,
  quantFromPath,
  ramBytes,
  relativeUpdated,
  requestAgentLaunchOverride,
  resolveAgentModelControlLevel,
  searchHub,
  setBoundAgentConfigOption,
  setLocalPrefs,
  startManagedRuntime,
  stopManagedRuntime,
  type AgentModelControlLevel,
  type HardwareProfile,
  type HubFile,
  type HubModel,
  type LocalPrefs,
  type RuntimeAgentCompatibility,
  type RuntimeCompatibility,
  type RuntimeHealth,
  type RuntimeInventoryRow,
  type RuntimeModelObservation,
  type RuntimeStartResult,
} from '@/lib/local-models'
import {
  bestPick,
  cancelDownload,
  downloadsAvailable,
  estimateFit,
  listDownloads,
  onModelDownloadEvent,
  parseGalleryYaml,
  recommendQuant,
  registryList,
  removeModel,
  serveModel,
  startDownload,
  type GalleryEntry,
  type GalleryIndex,
  type ModelDownloadRow,
  type OrphanPart,
  type RegistryEntry,
} from '@/lib/models-download'
import {
  buildCandidates,
  bytesToGib,
  DEFAULT_QUANT,
  FIT_CTX,
  FIT_CTX_LABEL,
  hostHwClass,
  tierTone,
  type FitEstimate,
} from '@/lib/model-fit'
import { cn } from '@/lib/utils'

type Tab = 'runtimes' | 'library' | 'explore' | 'hardware'
type ExploreTab = 'discover' | 'gallery'
type HubSort = 'downloads' | 'likes' | 'lastModified'

/** A small, explicit sample for the advanced Gallery inspector. It is never
 * presented as a downloaded artifact or as an active runtime. */
const GALLERY_SAMPLE = `# index.yaml — gallery sample
# entries pin files by sha256; parsing does not install anything.
version: 1
models:
  - id: gallery@model
    backend_override: ollama
    preload: false
    files:
      - path: model-Q4_K_M.gguf
        sha256: 0000000000000000000000000000000000000000000000000000000000000000
`

type LibraryRow = RegistryEntry & {
  verified?: boolean
  verification?: string
}

const NO_CONFIG_OPTIONS: AcpConfigOption[] = []

type SetupPhase = 'idle' | 'starting' | 'healthy' | 'degraded' | 'failed'
type SetupOutcome = { phase: SetupPhase; message: string; reason?: string }
type HandoffState = { status: 'requested' | 'error'; message: string }

function isFitEstimate(value: unknown): value is FitEstimate {
  if (!value || typeof value !== 'object') return false
  const row = value as Record<string, unknown>
  return (
    (row.tier === 'fits' || row.tier === 'may_be_slow' || row.tier === 'wont_fit') &&
    typeof row.fileGb === 'number' &&
    typeof row.kvGb === 'number' &&
    typeof row.totalGb === 'number'
  )
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/** One fit estimate. Missing evidence is a neutral dot, never a red failure. */
function FitDot({ est }: { est?: FitEstimate | null }) {
  if (!est) {
    return (
      <span
        data-fit="unavailable"
        aria-label="Fit estimate unavailable"
        title="Fit estimate unavailable — no hardware estimate was returned."
        className="inline-block h-1.5 w-1.5 shrink-0 rounded-full bg-border/70"
      />
    )
  }
  const tone = tierTone(est.tier)
  const dot =
    tone.key === 'ok'
      ? 'bg-emerald-400'
      : tone.key === 'warn'
        ? 'bg-warning'
        : 'bg-red-400'
  const split = `${est.fileGb.toFixed(1)} GiB file + ${(est.kvGb * 1000).toFixed(0)} MB KV ≈ ${est.totalGb.toFixed(2)} GiB total`
  return (
    <span
      data-fit={est.tier}
      aria-label={`Fit estimate: ${tone.label}`}
      className={cn('inline-block h-1.5 w-1.5 shrink-0 rounded-full', dot)}
      title={`Estimate at ${FIT_CTX_LABEL} context — ${split}. ${tone.hint}`}
    />
  )
}

function CapChip({
  on,
  label,
  icon,
}: {
  on: boolean
  label: string
  icon: React.ReactNode
}) {
  if (!on) return null
  return (
    <span className="inline-flex items-center gap-0.5 rounded border border-border/50 bg-background/50 px-1 py-0.5 font-mono text-[8px] text-muted-foreground">
      {icon}
      {label}
    </span>
  )
}

function healthView(health: RuntimeHealth): {
  label: string
  className: string
  dot: string
} {
  switch (health) {
    case 'healthy':
      return { label: 'Healthy', className: 'border-emerald-500/30 bg-emerald-500/10 text-emerald-300', dot: 'bg-emerald-400' }
    case 'degraded':
      return { label: 'Degraded', className: 'border-warning/30 bg-warning/10 text-warning', dot: 'bg-warning' }
    case 'observed':
      return { label: 'Observed', className: 'border-sky-500/30 bg-sky-500/10 text-sky-300', dot: 'bg-sky-400' }
    case 'starting':
      return { label: 'Starting', className: 'border-sky-500/30 bg-sky-500/10 text-sky-300', dot: 'bg-sky-400' }
    case 'stopped':
      return { label: 'Stopped', className: 'border-border/60 bg-muted/40 text-muted-foreground', dot: 'bg-muted-foreground/60' }
    case 'down':
      return { label: 'Down', className: 'border-red-500/30 bg-red-500/10 text-red-300', dot: 'bg-red-400' }
    case 'unavailable':
      return { label: 'Unavailable', className: 'border-red-500/30 bg-red-500/10 text-red-300', dot: 'bg-red-400' }
    case 'failed':
      return { label: 'Failed', className: 'border-red-500/30 bg-red-500/10 text-red-300', dot: 'bg-red-400' }
    default:
      return { label: 'Unknown', className: 'border-border/60 bg-muted/40 text-muted-foreground', dot: 'bg-muted-foreground/60' }
  }
}

function ownershipView(ownership: RuntimeInventoryRow['ownership']): string {
  if (ownership === 'managed') return 'Managed'
  if (ownership === 'remote') return 'Remote'
  return 'External'
}

function compatibilityFor(
  runtime: RuntimeInventoryRow,
  controlLevel: AgentModelControlLevel,
  advertised?: RuntimeAgentCompatibility,
  agentId?: string,
): { status: RuntimeCompatibility; reason: string } {
  if (advertised) return advertised
  if (
    runtime.agentCompatibilityRows?.length === 1 &&
    (!agentId || !runtime.agentCompatibilityRows[0].agentId || runtime.agentCompatibilityRows[0].agentId === agentId)
  ) {
    return runtime.agentCompatibilityRows[0]
  }
  if (
    runtime.agentCompatibility &&
    (!agentId || !runtime.agentCompatibility.agentId || runtime.agentCompatibility.agentId === agentId)
  ) {
    return runtime.agentCompatibility
  }
  if (controlLevel === 'native_only') {
    return {
      status: 'not_supported',
      reason: 'This agent manages its model inside the agent, so AgentCowork is not offering a runtime handoff.',
    }
  }
  if (controlLevel === 'unknown') {
    return {
      status: 'unknown',
      reason: 'The agent has not advertised a model-control level for this runtime.',
    }
  }
  return {
    status: 'unknown',
    reason: 'The agent can receive a control, but compatibility with this endpoint is not confirmed yet.',
  }
}

function compatibilityLabel(status: RuntimeCompatibility): string {
  if (status === 'not_supported') return 'Not supported'
  if (status === 'supported') return 'Supported'
  return 'Unknown'
}

function runtimeKindLabel(kind: string, advanced = false): string {
  if (!advanced && /gguf|llamafile|llama\.cpp/i.test(kind)) return 'Local model service'
  return kind || 'Local runtime'
}

function runtimeDisplayLabel(id: string, advanced = false): string {
  if (!advanced && /gguf|llamafile|llama\.cpp/i.test(id)) return 'Local model service'
  return id
}

function DownloadRow({
  row,
  onCancel,
  onResume,
}: {
  row: ModelDownloadRow
  onCancel: (id: string) => void
  onResume: (repo: string, filename: string) => void
}) {
  const pct =
    row.totalBytes > 0 ? Math.min(100, Math.round((row.doneBytes / row.totalBytes) * 100)) : 0
  return (
    <div className="rounded-md border border-border/60 bg-background/50 px-2 py-1.5">
      <div className="flex items-center justify-between gap-2">
        <div className="min-w-0 truncate font-mono text-[10px] text-foreground">
          {row.repo} · {row.filename}
        </div>
        {row.phase === 'downloading' && (
          <Button
            size="sm"
            variant="ghost"
            className="h-6 shrink-0 px-1.5 text-[10px] text-muted-foreground hover:text-red-300"
            onClick={() => onCancel(row.id)}
          >
            <Pause className="mr-1 h-3 w-3" />
            Cancel
          </Button>
        )}
        {(row.phase === 'cancelled' || row.phase === 'error') && (
          <Button
            size="sm"
            className="h-6 shrink-0 bg-brand px-2 text-[10px] text-white hover:bg-brand-hover"
            onClick={() => onResume(row.repo, row.filename)}
          >
            <Play className="mr-1 h-3 w-3" />
            Resume
          </Button>
        )}
        {row.phase === 'done' && (
          <span className="flex shrink-0 items-center gap-1 font-mono text-[10px] text-emerald-300">
            <CheckCircle2 className="h-3 w-3" />
            Downloaded
          </span>
        )}
      </div>
      {row.phase === 'downloading' && (
        <div className="mt-1.5 flex items-center gap-2">
          <Progress value={pct} className="h-1.5" />
          <span className="shrink-0 font-mono text-[9px] text-muted-foreground">
            {pct}% · {formatBytes(row.doneBytes)} / {formatBytes(row.totalBytes)}
          </span>
        </div>
      )}
      {(row.phase === 'cancelled' || row.phase === 'error') && (
        <div className="mt-1 font-mono text-[9px] text-muted-foreground">
          {row.phase === 'cancelled'
            ? 'Paused — the partial file is kept; Resume continues from where it stopped.'
            : `Download failed: ${row.error ?? 'the native service did not name a cause'}`}
        </div>
      )}
    </div>
  )
}

function AgentHandoffCard({
  runtime,
  agentName,
  readiness,
  controlLevel,
  configOptions,
  configLoading,
  configError,
  handoffState,
  onApplyOption,
  onRequestOverride,
}: {
  runtime: RuntimeInventoryRow
  agentName: string
  readiness?: AgentReadiness
  controlLevel: AgentModelControlLevel
  configOptions: AcpConfigOption[]
  configLoading: boolean
  configError: string | null
  handoffState?: HandoffState
  onApplyOption: (option: AcpConfigOption, value: string | boolean) => void
  onRequestOverride: (endpoint: string, model: string) => void
}) {
  const [endpoint, setEndpoint] = useState(runtime.endpoint ?? '')
  const [model, setModel] = useState('')
  const compatibility = compatibilityFor(runtime, controlLevel)
  const agentIsReady = readiness === 'ready'
  const agentTone = readiness === 'ready'
    ? 'border-emerald-500/30 bg-emerald-500/10 text-emerald-300'
    : readiness === 'degraded'
      ? 'border-warning/30 bg-warning/10 text-warning'
      : 'border-border/60 bg-muted/40 text-muted-foreground'

  useEffect(() => {
    setEndpoint(runtime.endpoint ?? '')
  }, [runtime.endpoint, runtime.id])

  return (
    <div
      data-testid={`agent-handoff-${runtime.id}`}
      className="space-y-2 rounded-lg border border-brand/30 bg-brand/5 p-2.5"
    >
      <div className="flex flex-wrap items-center gap-2">
        <div className="min-w-0 flex-1">
          <div className="font-mono text-[9px] uppercase tracking-wider text-brand">
            Agent handoff
          </div>
          <div className="truncate text-[11px] font-semibold text-foreground">{agentName}</div>
        </div>
        <Badge className={cn('font-mono text-[9px]', agentTone)}>
          {readiness ? readinessLabel(readiness) : 'readiness unknown'}
        </Badge>
        {agentIsReady && <span className="sr-only">Agent readiness verified</span>}
      </div>

      <div className="rounded-md border border-border/50 bg-background/40 px-2 py-1.5 text-[10px] text-muted-foreground">
        <span className="text-foreground">Compatibility:</span>{' '}
        <span className={cn(
          compatibility.status === 'supported' && 'text-emerald-300',
          compatibility.status === 'not_supported' && 'text-warning',
          compatibility.status === 'unknown' && 'text-sky-300',
        )}>
          {compatibilityLabel(compatibility.status)}
        </span>{' '}
        — {compatibility.reason}
      </div>

      {controlLevel === 'native_only' && (
        <div className="rounded-md border border-border/50 bg-background/30 px-2 py-2 text-[11px] text-foreground">
          This agent manages its own model — configure it inside the agent.
        </div>
      )}

      {configLoading && controlLevel === 'unknown' && (
        <div className="rounded-md border border-border/50 bg-background/30 px-2 py-2 text-[11px] text-muted-foreground">
          Reading the agent's advertised model control…
        </div>
      )}

      {!configLoading && controlLevel === 'unknown' && (
        <div className="rounded-md border border-border/50 bg-background/30 px-2 py-2 text-[11px] text-foreground">
          Model control for this agent is unknown — not offered.
        </div>
      )}

      {configError && controlLevel !== 'session_config' && (
        <div className="rounded-md border border-warning/30 bg-warning/5 px-2 py-1.5 text-[10px] text-warning">
          {configError}
        </div>
      )}

      {controlLevel === 'session_config' && (
        <div className="space-y-2">
          {configLoading && (
            <div className="text-[10px] text-muted-foreground">Reading the agent's own options…</div>
          )}
          {configError && (
            <div className="rounded-md border border-warning/30 bg-warning/5 px-2 py-1.5 text-[10px] text-warning">
              {configError}
            </div>
          )}
          {configOptions.length === 0 && !configLoading && !configError && (
            <div className="text-[10px] text-muted-foreground">
              The agent has not advertised a model option for this Chat yet.
            </div>
          )}
          {configOptions.map((option) => {
            const isBoolean = option.type === 'boolean'
            const isSelect = option.type === 'select' || (option.options && option.options.length > 0)
            return (
              <label key={option.id} className="block space-y-1">
                <span className="flex items-center justify-between gap-2 text-[10px] text-foreground">
                  <span>{option.name}</span>
                  <span className="font-mono text-[9px] text-muted-foreground">{String(option.currentValue)}</span>
                </span>
                {isBoolean ? (
                  <span className="flex items-center gap-2 text-[10px] text-muted-foreground">
                    <input
                      type="checkbox"
                      aria-label={`${agentName} ${option.name}`}
                      checked={Boolean(option.currentValue)}
                      onChange={(event) => onApplyOption(option, event.target.checked)}
                      className="accent-[var(--brand)]"
                    />
                    <span>Enable this agent-owned option</span>
                  </span>
                ) : isSelect ? (
                  <select
                    aria-label={`${agentName} ${option.name}`}
                    value={String(option.currentValue)}
                    onChange={(event) => onApplyOption(option, event.target.value)}
                    className="h-7 w-full rounded border border-border bg-background px-2 font-mono text-[10px] text-foreground"
                  >
                    {(option.options ?? []).map((choice) => (
                      <option key={String(choice.value)} value={String(choice.value)}>
                        {choice.name}
                      </option>
                    ))}
                  </select>
                ) : (
                  <div className="text-[10px] text-muted-foreground">
                    This option has no choices to apply from the desktop.
                  </div>
                )}
              </label>
            )
          })}
          {handoffState?.status === 'requested' && (
            <div className="rounded-md border border-sky-500/30 bg-sky-500/5 px-2 py-1.5 text-[10px] text-sky-300">
              requested · {handoffState.message}
            </div>
          )}
          {handoffState?.status === 'error' && (
            <div className="rounded-md border border-red-500/30 bg-red-500/5 px-2 py-1.5 text-[10px] text-red-300">
              {handoffState.message}
            </div>
          )}
        </div>
      )}

      {controlLevel === 'launch_override' && (
        <form
          className="space-y-2"
          onSubmit={(event) => {
            event.preventDefault()
            onRequestOverride(endpoint.trim(), model.trim())
          }}
        >
          <div className="grid gap-2 sm:grid-cols-2">
            <label className="space-y-1 text-[10px] text-muted-foreground">
              Endpoint
              <Input
                value={endpoint}
                onChange={(event) => setEndpoint(event.target.value)}
                placeholder="http://127.0.0.1:…"
                className="h-7 font-mono text-[10px]"
              />
            </label>
            <label className="space-y-1 text-[10px] text-muted-foreground">
              Model id
              <Input
                value={model}
                onChange={(event) => setModel(event.target.value)}
                placeholder="agent model id"
                className="h-7 font-mono text-[10px]"
              />
            </label>
          </div>
          <div className="text-[10px] text-muted-foreground">
            Non-secret inputs only. This request applies to the next launch; the agent decides whether to accept it.
          </div>
          <Button
            type="submit"
            size="sm"
            disabled={!endpoint.trim() || !model.trim()}
            className="h-7 bg-brand px-2.5 text-[10px] text-white hover:bg-brand-hover"
          >
            Request next launch
          </Button>
          {handoffState?.status === 'requested' && (
            <div className="rounded-md border border-sky-500/30 bg-sky-500/5 px-2 py-1.5 text-[10px] text-sky-300">
              requested · {handoffState.message}
            </div>
          )}
          {handoffState?.status === 'error' && (
            <div className="rounded-md border border-red-500/30 bg-red-500/5 px-2 py-1.5 text-[10px] text-red-300">
              {handoffState.message}
            </div>
          )}
        </form>
      )}

      <div className="text-[9px] leading-relaxed text-muted-foreground">
        The runtime prepares an endpoint. It does not choose the model the agent will use.
      </div>
    </div>
  )
}

export default function LocalModelsPanel() {
  const [tab, setTab] = useState<Tab>('runtimes')
  const [exploreTab, setExploreTab] = useState<ExploreTab>('discover')
  const [query, setQuery] = useState('')
  const [sort, setSort] = useState<HubSort>('downloads')
  const [catalog, setCatalog] = useState<HubModel[]>([])
  const [selected, setSelected] = useState<HubModel | null>(null)
  const [files, setFiles] = useState<HubFile[]>([])
  const [picked, setPicked] = useState<string | null>(null)
  const [hw, setHw] = useState<HardwareProfile | null>(null)
  const [inventory, setInventory] = useState<RuntimeInventoryRow[]>([])
  const [inventoryLoading, setInventoryLoading] = useState(true)
  const [inventoryError, setInventoryError] = useState<string | null>(null)
  const [runtimeModels, setRuntimeModels] = useState<Record<string, RuntimeModelObservation[]>>({})
  const [modelsProbed, setModelsProbed] = useState<Record<string, boolean>>({})
  const [runtimeModelErrors, setRuntimeModelErrors] = useState<Record<string, string>>({})
  const [runtimeBusy, setRuntimeBusy] = useState<Record<string, 'starting' | 'stopping'>>({})
  const [runtimeNotices, setRuntimeNotices] = useState<Record<string, string>>({})
  const [setupOutcome, setSetupOutcome] = useState<SetupOutcome>({ phase: 'idle', message: '' })
  const [selectedForRemoval, setSelectedForRemoval] = useState<Set<string>>(new Set())
  const [downloads, setDownloads] = useState<ModelDownloadRow[]>([])
  const [orphans, setOrphans] = useState<OrphanPart[]>([])
  const [registry, setRegistry] = useState<LibraryRow[]>([])
  const [recommended, setRecommended] = useState<{ quant: string; availableRamBytes: number } | null>(null)
  const [hubLoading, setHubLoading] = useState(false)
  const [hubError, setHubError] = useState<string | null>(null)
  const [nativeError, setNativeError] = useState<string | null>(null)
  const [busyFile, setBusyFile] = useState<string | null>(null)
  const [prefs, setPrefs] = useState<LocalPrefs>(getLocalPrefs)
  const [fitMap, setFitMap] = useState<Record<string, FitEstimate | null>>({})
  const [artifactFitMap, setArtifactFitMap] = useState<Record<string, FitEstimate | null>>({})
  const [pickingBest, setPickingBest] = useState(false)
  const [autoPicked, setAutoPicked] = useState(false)
  const [galleryYaml, setGalleryYaml] = useState(GALLERY_SAMPLE)
  const [gallery, setGallery] = useState<GalleryIndex | null>(null)
  const [galleryError, setGalleryError] = useState<string | null>(null)
  const [galleryBusy, setGalleryBusy] = useState(false)
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [endpointNotes, setEndpointNotes] = useState<Record<string, string>>({})
  const [handoffStates, setHandoffStates] = useState<Record<string, HandoffState>>({})
  const [configLoading, setConfigLoading] = useState(false)
  const [configError, setConfigError] = useState<string | null>(null)
  const [advertisedControl, setAdvertisedControl] = useState<{
    agentId: string
    level: AgentModelControlLevel
  } | null>(null)

  const notify = useAppStore((s) => s.notify)
  const selectedAgentId = useAppStore((s) => s.selectedAgentId)
  const liveAgents = useAppStore((s) => s.liveAgents)
  const acpHandles = useAppStore((s) => s.acpHandles)
  const acpConfigOptions = useAppStore((s) =>
    selectedAgentId ? s.acpConfigOptions[selectedAgentId] ?? NO_CONFIG_OPTIONS : NO_CONFIG_OPTIONS,
  )
  const setAcpConfigOptions = useAppStore((s) => s.setAcpConfigOptions)
  const canDownload = downloadsAvailable()
  const ram = ramBytes(hw)
  const boundAgent = liveAgents.find((agent) => agent.id === selectedAgentId)
  const boundAgentName = boundAgent?.name?.trim() || selectedAgentId || 'No agent bound'
  const boundReadiness = boundAgent?.readiness
  const boundHandle = selectedAgentId ? acpHandles[selectedAgentId] : undefined
  const inferredControlLevel = resolveAgentModelControlLevel(boundAgent, acpConfigOptions)
  const controlLevel = advertisedControl?.agentId === selectedAgentId
    ? advertisedControl.level
    : inferredControlLevel
  const shouldLoadConfigOptions = Boolean(selectedAgentId && boundHandle)

  const hardwareFitSummary = useMemo(() => {
    const totalRamGb = ram > 0 ? ram / 1e9 : 8
    const comfortableLimitGb = totalRamGb * 0.60
    const warningLimitGb = totalRamGb * 0.85
    return {
      totalRamGb,
      comfortableLimitGb,
      warningLimitGb,
      gpuLabel: hw?.gpu && hw.gpu !== '—' ? hw.gpu : 'Integrated / CPU-only',
      cpuCount: cpuCores(hw) || 4,
    }
  }, [ram, hw])

  const recommendedRuntime = useMemo(
    () => ({
      id: 'ollama',
      kind: 'ollama',
      name: 'Ollama',
      disk: 'about 1–2 GB for the runtime; model files are separate',
      ram: 'shares available RAM when a model is loaded',
      reason: 'A supported OpenAI-compatible local runtime for a casual setup',
      hardwareKnown: ram > 0,
      supported: ram === 0 || ram >= 4_000_000_000,
    }),
    [ram],
  )

  const refreshInventory = useCallback(async () => {
    setInventoryLoading(true)
    try {
      const rows = await listRuntimeInventory()
      setInventory(rows)
      setInventoryError(null)
    } catch (error) {
      setInventory([])
      setInventoryError(error instanceof Error ? error.message : 'The runtime inventory could not be read.')
    } finally {
      setInventoryLoading(false)
    }
  }, [])

  const refreshNative = useCallback(async () => {
    if (!canDownload) return
    try {
      const [downloadResult, registryResult] = await Promise.all([listDownloads(), registryList()])
      const dl = downloadResult as unknown as { active?: ModelDownloadRow[]; orphans?: OrphanPart[] }
      const reg = registryResult as unknown as { models?: LibraryRow[] }
      setDownloads(Array.isArray(dl.active) ? dl.active : [])
      setOrphans(Array.isArray(dl.orphans) ? dl.orphans : [])
      setRegistry(Array.isArray(reg.models) ? reg.models : [])
      setNativeError(null)
    } catch (error) {
      setNativeError(error instanceof Error ? error.message : 'The local model library could not be read.')
    }
  }, [canDownload])

  useEffect(() => {
    void getHardware()
      .then((value) => value && setHw(value))
      .catch(() => setHw(null))
    void refreshInventory()
    void refreshNative()
  }, [refreshInventory, refreshNative])

  useEffect(() => {
    const eventBridgeAvailable =
      typeof window !== 'undefined' && '__TAURI_EVENT_PLUGIN_INTERNALS__' in window
    if (!canDownload || !eventBridgeAvailable) return
    let alive = true
    let unlisten: (() => void) | undefined
    void onModelDownloadEvent((event) => {
      if (event.phase === 'done' || event.phase === 'error' || event.phase === 'cancelled' || event.kind === 'serve') {
        void refreshNative()
      } else {
        setDownloads((current) => {
          const row = current.find((item) => item.id === event.id)
          if (!row) return current
          return current.map((item) =>
            item.id === event.id
              ? {
                  ...item,
                  phase: event.phase as ModelDownloadRow['phase'],
                  doneBytes: event.doneBytes ?? item.doneBytes,
                  totalBytes: event.totalBytes ?? item.totalBytes,
                }
              : item,
          )
        })
      }
    })
      .then((unlistenFn) => {
        if (alive) unlisten = unlistenFn
      })
      .catch(() => {
        // The library remains readable even if the event stream is unavailable.
      })
    return () => {
      alive = false
      unlisten?.()
    }
  }, [canDownload, refreshNative])

  useEffect(() => {
    const pending = inventory.filter(
      (runtime) => !runtime.modelsObserved && !modelsProbed[runtime.id],
    )
    if (pending.length === 0) return
    let alive = true
    for (const runtime of pending) {
      void listRuntimeModels(runtime.id)
        .then((models) => {
          if (!alive) return
          setRuntimeModels((current) => ({ ...current, [runtime.id]: models }))
          setModelsProbed((current) => ({ ...current, [runtime.id]: true }))
          setRuntimeModelErrors((current) => {
            const next = { ...current }
            delete next[runtime.id]
            return next
          })
        })
        .catch((error) => {
          if (!alive) return
          setModelsProbed((current) => ({ ...current, [runtime.id]: true }))
          setRuntimeModelErrors((current) => ({
            ...current,
            [runtime.id]: error instanceof Error ? error.message : 'Models were not observed.',
          }))
        })
    }
    return () => {
      alive = false
    }
  }, [inventory, modelsProbed])

  useEffect(() => {
    if (tab !== 'explore' || exploreTab !== 'discover') return
    let cancelled = false
    setHubLoading(true)
    setHubError(null)
    const timer = setTimeout(() => {
      void searchHub(query, sort)
        .then((rows) => {
          if (cancelled) return
          setCatalog(rows)
          setSelected((current) => {
            if (current && rows.some((row) => row.id === current.id)) return current
            return rows[0] ?? null
          })
        })
        .catch((error) => {
          if (cancelled) return
          setCatalog([])
          setSelected(null)
          setHubError(error instanceof Error ? error.message : 'The model search was not available.')
        })
        .finally(() => {
          if (!cancelled) setHubLoading(false)
        })
    }, query ? 280 : 0)
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [exploreTab, query, sort, tab])

  useEffect(() => {
    setPicked(null)
    setRecommended(null)
    if (!selected) {
      setFiles([])
      return
    }
    let cancelled = false
    void listHubFiles(selected.id)
      .then((rows) => {
        if (!cancelled) setFiles(rows)
      })
      .catch(() => {
        if (!cancelled) setFiles([])
      })
    if (canDownload) {
      void recommendQuant(selected.id)
        .then((value) => {
          if (!cancelled && value && typeof value.quant === 'string') setRecommended(value)
        })
        .catch(() => {
          // Recommendations are optional evidence, not a gate.
        })
    }
    return () => {
      cancelled = true
    }
  }, [canDownload, selected])

  const gguf = files.filter((file) => file.path.toLowerCase().endsWith('.gguf'))
  const ggufKey = gguf.map((file) => `${file.path}:${file.size}`).join('|')
  useEffect(() => {
    if (!canDownload || !selected || gguf.length === 0) {
      setFitMap({})
      return
    }
    let cancelled = false
    void Promise.all(
      gguf.map(async (file) => {
        try {
          const value = await estimateFit(bytesToGib(file.size), FIT_CTX)
          return [file.path, isFitEstimate(value) ? value : null] as const
        } catch {
          return [file.path, null] as const
        }
      }),
    ).then((rows) => {
      if (!cancelled) setFitMap(Object.fromEntries(rows))
    })
    return () => {
      cancelled = true
    }
  }, [canDownload, selected?.id, ggufKey])

  useEffect(() => {
    if (!canDownload || registry.length === 0) {
      setArtifactFitMap({})
      return
    }
    let cancelled = false
    void Promise.all(
      registry.map(async (row) => {
        try {
          const value = await estimateFit(bytesToGib(row.size), row.ctx || FIT_CTX)
          return [row.id, isFitEstimate(value) ? value : null] as const
        } catch {
          return [row.id, null] as const
        }
      }),
    ).then((rows) => {
      if (!cancelled) setArtifactFitMap(Object.fromEntries(rows))
    })
    return () => {
      cancelled = true
    }
  }, [canDownload, registry])

  useEffect(() => {
    if (!selectedAgentId || !boundHandle || !shouldLoadConfigOptions) {
      setConfigLoading(false)
      setConfigError(null)
      setAdvertisedControl(null)
      return
    }
    let alive = true
    setConfigLoading(true)
    setConfigError(null)
    void getBoundAgentConfigOptions(boundHandle)
      .then((result) => {
        if (!alive) return
        setAcpConfigOptions(selectedAgentId, result.options)
        if (result.control) setAdvertisedControl({ agentId: selectedAgentId, level: result.control })
        setConfigLoading(false)
      })
      .catch(() => {
        if (!alive) return
        setAdvertisedControl(null)
        setConfigLoading(false)
        setConfigError("The bound-agent option bridge is not available yet; no host model is being offered.")
      })
    return () => {
      alive = false
    }
  }, [boundHandle, selectedAgentId, setAcpConfigOptions, shouldLoadConfigOptions])

  const chosen = useMemo(() => {
    if (picked) return files.find((file) => file.path === picked) ?? null
    const quant = recommended?.quant?.toLowerCase()
    if (quant) {
      const match = gguf.find((file) => quantFromPath(file.path).toLowerCase() === quant)
      if (match) return match
    }
    return gguf.find((file) => /Q4_K_M/i.test(file.path)) ?? gguf[0] ?? files[0] ?? null
  }, [files, gguf, picked, recommended])

  const registryBytes = registry.reduce((total, row) => total + row.size, 0)
  const savePrefs = (next: LocalPrefs) => {
    setPrefs(next)
    setLocalPrefs(next)
  }

  const download = async (repo: string, file: HubFile) => {
    if (!canDownload) {
      notify('Model downloads require the Tauri desktop shell.')
      return
    }
    setBusyFile(file.path)
    setNativeError(null)
    try {
      const result = await startDownload(repo, file.path)
      if (result.alreadyInstalled) notify(`Already downloaded: ${result.id ?? file.path}`)
      else notify(result.resuming ? 'Resuming download…' : `Downloading ${file.path}…`)
      await refreshNative()
    } catch (error) {
      setNativeError(error instanceof Error ? error.message : 'The download could not start.')
    } finally {
      setBusyFile(null)
    }
  }

  const resume = async (repo: string, filename: string) => {
    if (!canDownload) return
    setNativeError(null)
    try {
      await startDownload(repo, filename)
      await refreshNative()
    } catch (error) {
      setNativeError(error instanceof Error ? error.message : 'The download could not resume.')
    }
  }

  const cancel = async (id: string) => {
    if (!canDownload) return
    try {
      await cancelDownload(id)
      await refreshNative()
    } catch (error) {
      setNativeError(error instanceof Error ? error.message : 'The download could not be cancelled.')
    }
  }

  const remove = async (id: string) => {
    if (!canDownload) return
    try {
      await removeModel(id)
      await refreshNative()
      notify(`Removed ${id}`)
    } catch (error) {
      setNativeError(error instanceof Error ? error.message : 'The artifact could not be removed.')
    }
  }

  const bulkRemove = async () => {
    if (!canDownload || selectedForRemoval.size === 0) return
    const ids = [...selectedForRemoval]
    setNativeError(null)
    let removed = 0
    let failed = 0
    for (const id of ids) {
      try {
        await removeModel(id)
        removed += 1
      } catch (error) {
        failed += 1
        setNativeError(error instanceof Error ? error.message : `Removal failed for ${id}.`)
      }
    }
    await refreshNative()
    if (removed > 0) {
      setSelectedForRemoval(new Set())
      notify(`Removed ${removed} artifact${removed === 1 ? '' : 's'} — space freed on disk`)
    }
    if (failed > 0) notify(`Artifact removal: ${failed} failed`, 'error')
  }

  /** Endpoint preparation never changes the bound agent's model. */
  const prepareEndpoint = async (id: string) => {
    if (!canDownload) {
      notify('Endpoint preparation requires the Tauri desktop shell.')
      return
    }
    try {
      const result = await serveModel(id)
      const endpoint = result.baseUrl || 'the configured endpoint'
      const message = `Endpoint prepared at ${endpoint}. Health is not confirmed here; no agent model was selected.`
      setEndpointNotes((current) => ({ ...current, [id]: message }))
      notify(`Endpoint prepared for ${id} — no agent model was selected.`)
      await refreshNative()
    } catch (error) {
      setNativeError(error instanceof Error ? error.message : 'The endpoint could not be prepared.')
      notify('Endpoint preparation failed.', 'error')
    }
  }

  const autoPick = async () => {
    if (!selected || !canDownload || gguf.length < 2) return
    setPickingBest(true)
    setAutoPicked(false)
    setNativeError(null)
    try {
      const candidates = buildCandidates(selected.id, gguf)
      const best = await bestPick(hostHwClass(hw), candidates)
      if (best) {
        setPicked(best.file)
        setAutoPicked(true)
        notify(`Suggested ${best.quant} (${best.hw}-class build) for this hardware.`)
      } else {
        notify('No downloadable build could be estimated from this result.')
      }
    } catch (error) {
      setNativeError(error instanceof Error ? error.message : 'The variant estimate could not be read.')
    } finally {
      setPickingBest(false)
    }
  }

  const parseGallery = async () => {
    if (!canDownload || !galleryYaml.trim()) return
    setGalleryBusy(true)
    setGalleryError(null)
    try {
      setGallery(await parseGalleryYaml(galleryYaml))
    } catch (error) {
      setGallery(null)
      setGalleryError(error instanceof Error ? error.message : 'The gallery index could not be parsed.')
    } finally {
      setGalleryBusy(false)
    }
  }

  const startRecommendedRuntime = async () => {
    setSetupOutcome({ phase: 'starting', message: 'Start requested. Waiting for a health result…' })
    try {
      const result: RuntimeStartResult = await startManagedRuntime(
        { id: recommendedRuntime.id, ownership: 'managed' },
        { runtimeKind: recommendedRuntime.kind },
      )
      if (!result.ok || (!result.started && !result.alreadyManaged)) {
        throw new Error(result.reason || 'The native runtime did not confirm a start.')
      }
      let health = result.health
      let reason = result.reason
      let row = result.runtime
      for (let attempt = 0; attempt < 5; attempt += 1) {
        if (health === 'healthy' || health === 'degraded' || health === 'down' || health === 'unavailable' || health === 'failed') break
        await delay(150)
        try {
          const rows = await listRuntimeInventory()
          const observed = rows.find((item) => item.id === recommendedRuntime.id)
          if (observed) {
            row = observed
            health = observed.health
            reason = observed.reason || observed.lastError || reason
          }
        } catch (error) {
          reason = error instanceof Error ? error.message : reason
        }
      }
      if (health === 'healthy') {
        const message = `${recommendedRuntime.name} ${result.alreadyManaged ? 'is already managed and' : 'started and'} passed its health check. The endpoint is available; the agent still chooses its own model.`
        setSetupOutcome({ phase: 'healthy', message })
        notify(`${recommendedRuntime.name} started and passed its health check.`)
      } else {
        const detail = reason || (health === 'observed' || health === 'unknown' || health === 'starting'
          ? 'the health result has not been confirmed'
          : 'the native service reported a reduced state')
        const message = `${recommendedRuntime.name} ${result.alreadyManaged ? 'is already managed, but the result is' : 'started, but the result is'} degraded — ${detail}.`
        setSetupOutcome({ phase: 'degraded', message, reason: detail })
        notify(`${recommendedRuntime.name} started with a degraded result.`, 'error')
      }
      if (row) {
        setRuntimeNotices((current) => ({ ...current, [row.id]: messageForRuntime(row) }))
      }
      await refreshInventory()
    } catch (error) {
      const reason = error instanceof Error ? error.message : 'The native service did not name a cause.'
      const message = `${recommendedRuntime.name} could not start — ${reason}.`
      setSetupOutcome({ phase: 'failed', message, reason })
      notify(message, 'error')
    }
  }

  const changeRuntime = async (runtime: RuntimeInventoryRow, action: 'start' | 'stop') => {
    if (runtime.ownership !== 'managed') return
    setRuntimeBusy((current) => ({ ...current, [runtime.id]: action === 'start' ? 'starting' : 'stopping' }))
    setNativeError(null)
    try {
      const result = action === 'start'
        ? await startManagedRuntime(runtime, {
            runtimeKind: runtime.kind,
            modelId: runtime.models[0]?.id,
          })
        : await stopManagedRuntime(runtime)
      if (!result.ok) throw new Error(result.reason || 'The native service did not confirm the action.')
      await refreshInventory()
      const resultHealth = result.runtime?.health ?? result.health
      const resultLabel = healthView(resultHealth).label
      const message = action === 'start'
        ? `${runtime.kind} start requested · ${resultLabel}. ${result.reason || 'Health evidence is shown above when available.'}`
        : `${runtime.kind} stop requested · ${resultLabel}. ${result.reason || 'Health evidence is shown above when available.'}`
      setRuntimeNotices((current) => ({ ...current, [runtime.id]: message }))
      notify(action === 'start' ? `${runtime.kind} start requested.` : `${runtime.kind} stop requested.`)
    } catch (error) {
      const reason = error instanceof Error ? error.message : 'The native service did not name a cause.'
      setRuntimeNotices((current) => ({ ...current, [runtime.id]: `${action === 'start' ? 'Start' : 'Stop'} failed: ${reason}` }))
      notify(`${runtime.kind} ${action} failed: ${reason}`, 'error')
    } finally {
      setRuntimeBusy((current) => {
        const next = { ...current }
        delete next[runtime.id]
        return next
      })
    }
  }

  const handoffKey = (runtimeId: string) => `${selectedAgentId || 'unbound'}:${runtimeId}`
  const handoffState = (runtimeId: string) => handoffStates[handoffKey(runtimeId)]

  const applyConfigOption = async (runtimeId: string, option: AcpConfigOption, value: string | boolean) => {
    if (!selectedAgentId || !boundHandle) {
      const message = 'No live agent is bound to this Chat.'
      setHandoffStates((current) => ({ ...current, [handoffKey(runtimeId)]: { status: 'error', message } }))
      notify(message, 'error')
      return
    }
    const key = handoffKey(runtimeId)
    setHandoffStates((current) => ({
      ...current,
      [key]: { status: 'requested', message: `waiting for ${boundAgentName} to confirm` },
    }))
    try {
      const result = await setBoundAgentConfigOption(boundHandle, option.id, value)
      setAcpConfigOptions(selectedAgentId, result.options)
      setAdvertisedControl({ agentId: selectedAgentId, level: result.control ?? 'session_config' })
      const message = `${option.name} was sent to ${boundAgentName}; waiting for the agent to confirm.`
      setHandoffStates((current) => ({ ...current, [key]: { status: 'requested', message } }))
      notify(`Requested ${option.name} for ${boundAgentName}.`)
    } catch (error) {
      const reason = error instanceof Error ? error.message : 'The agent did not name a cause.'
      const message = `The request was not confirmed: ${reason}`
      setHandoffStates((current) => ({ ...current, [key]: { status: 'error', message } }))
      notify(message, 'error')
    }
  }

  const requestOverride = async (runtimeId: string, endpoint: string, model: string) => {
    if (!selectedAgentId) {
      const message = 'No agent is bound to receive a launch override.'
      setHandoffStates((current) => ({ ...current, [handoffKey(runtimeId)]: { status: 'error', message } }))
      notify(message, 'error')
      return
    }
    const key = handoffKey(runtimeId)
    setHandoffStates((current) => ({
      ...current,
      [key]: { status: 'requested', message: `waiting for ${boundAgentName} to accept the next-launch request` },
    }))
    try {
      await requestAgentLaunchOverride({ agentId: selectedAgentId, endpoint, model, runtimeId })
      const message = `${boundAgentName} received a non-secret next-launch request; it is not confirmed yet.`
      setHandoffStates((current) => ({ ...current, [key]: { status: 'requested', message } }))
      notify(`Launch override requested for ${boundAgentName}.`)
    } catch (error) {
      const reason = error instanceof Error ? error.message : 'The native service did not name a cause.'
      const message = `The launch request failed: ${reason}`
      setHandoffStates((current) => ({ ...current, [key]: { status: 'error', message } }))
      notify(message, 'error')
    }
  }

  return (
    <div className="flex h-full min-h-[520px] flex-col">
      <div className="mb-3 flex items-center gap-1 rounded-md border border-border/60 bg-background/40 p-0.5">
        {([
          ['runtimes', 'Runtimes'],
          ['library', 'Library'],
          ['explore', 'Explore'],
          ['hardware', 'Hardware'],
        ] as const).map(([id, label]) => (
          <button
            key={id}
            type="button"
            onClick={() => setTab(id)}
            className={cn(
              'flex-1 rounded px-2 py-1 text-[11px] font-medium transition-colors',
              tab === id ? 'bg-brand/15 text-brand' : 'text-muted-foreground hover:text-foreground',
            )}
          >
            {label}
          </button>
        ))}
      </div>

      {nativeError && (
        <div className="mb-2 rounded border border-red-500/30 bg-red-500/5 px-2 py-1.5 text-[11px] text-red-300">
          {nativeError}
        </div>
      )}

      {tab === 'runtimes' && (
        <section data-testid="runtimes-section" className="min-h-0 flex-1 space-y-3 overflow-y-auto pr-0.5">
          <div className="flex items-start justify-between gap-3">
            <div>
              <h2 className="flex items-center gap-2 text-[13px] font-semibold text-foreground">
                <Server className="h-4 w-4 text-brand" />
                Runtimes
              </h2>
              <p className="mt-0.5 text-[10px] leading-relaxed text-muted-foreground">
                Inventory of local and remote inference services. Starting a service prepares an endpoint; it never chooses the bound agent's model.
              </p>
            </div>
            <Button
              size="sm"
              variant="ghost"
              className="h-7 shrink-0 px-2 text-[10px] text-muted-foreground"
              onClick={() => void refreshInventory()}
            >
              Check again
            </Button>
          </div>

          {inventoryLoading && (
            <div className="rounded-lg border border-dashed border-border/70 bg-background/30 px-3 py-4 text-[11px] text-muted-foreground">
              Reading runtime inventory…
            </div>
          )}

          {!inventoryLoading && inventoryError && (
            <div className="rounded-lg border border-warning/30 bg-warning/5 px-3 py-3 text-[11px] text-warning">
              Runtime inventory is unavailable: {inventoryError}. No runtime is being reported on this surface.
            </div>
          )}

          {!inventoryLoading && !inventoryError && inventory.length === 0 && (
            <div className="space-y-3">
              <div className="rounded-lg border border-dashed border-border/70 bg-background/30 px-3 py-4 text-[11px] text-muted-foreground">
                {canDownload
                  ? 'No local runtime detected. You can set one up below, or use a service that is already running on this machine.'
                  : 'No local runtime can be inspected in browser preview. Open the desktop app to detect one.'}
              </div>
              {canDownload && (
                <div className="rounded-xl border border-brand/35 bg-brand/5 p-3.5" data-testid="managed-runtime-setup">
                  <div className="flex items-start gap-2">
                    <Sparkles className="mt-0.5 h-4 w-4 shrink-0 text-brand" />
                    <div className="min-w-0 flex-1">
                      <h3 className="text-[12px] font-semibold text-foreground">Set up a local runtime</h3>
                      <p className="mt-1 text-[11px] leading-relaxed text-muted-foreground">
                        {recommendedRuntime.hardwareKnown ? 'Suggested for this machine' : 'Suggested runtime'}: <span className="font-medium text-foreground">{recommendedRuntime.name}</span>. {recommendedRuntime.reason}.
                        {!recommendedRuntime.hardwareKnown && ' Hardware compatibility will be checked when setup starts.'}
                      </p>
                      <div className="mt-2 grid gap-1.5 text-[10px] text-muted-foreground sm:grid-cols-2">
                        <div className="rounded-md border border-border/50 bg-background/40 px-2 py-1.5">
                          <span className="text-foreground">Disk estimate</span><br />{recommendedRuntime.disk}
                        </div>
                        <div className="rounded-md border border-border/50 bg-background/40 px-2 py-1.5">
                          <span className="text-foreground">Memory</span><br />{recommendedRuntime.ram}
                        </div>
                      </div>
                      {!recommendedRuntime.supported && (
                        <div className="mt-2 rounded-md border border-warning/30 bg-warning/5 px-2 py-1.5 text-[10px] text-warning">
                          This machine has less than the usual 4 GB memory floor. You can try, but the result may be degraded.
                        </div>
                      )}
                      <div className="mt-3 flex flex-wrap items-center gap-2">
                        <Button
                          size="sm"
                          className="h-7 bg-brand px-2.5 text-[10px] text-white hover:bg-brand-hover"
                          disabled={setupOutcome.phase === 'starting'}
                          onClick={() => void startRecommendedRuntime()}
                        >
                          {setupOutcome.phase === 'starting' ? 'Starting — waiting for health result' : `Set up ${recommendedRuntime.name}`}
                        </Button>
                        <span className="font-mono text-[9px] text-muted-foreground">Guard-gated start</span>
                      </div>
                    </div>
                  </div>
                  {setupOutcome.phase !== 'idle' && (
                    <div
                      data-testid="managed-runtime-outcome"
                      className={cn(
                        'mt-3 rounded-md border px-2 py-2 text-[11px]',
                        setupOutcome.phase === 'healthy'
                          ? 'border-emerald-500/30 bg-emerald-500/5 text-emerald-200'
                          : setupOutcome.phase === 'degraded'
                            ? 'border-warning/30 bg-warning/5 text-warning'
                            : setupOutcome.phase === 'failed'
                              ? 'border-red-500/30 bg-red-500/5 text-red-300'
                              : 'border-sky-500/30 bg-sky-500/5 text-sky-300',
                      )}
                    >
                      <span className="font-semibold">
                        {setupOutcome.phase === 'healthy'
                          ? 'Started + healthy'
                          : setupOutcome.phase === 'degraded'
                            ? 'Started + degraded'
                            : setupOutcome.phase === 'failed'
                              ? 'Failed'
                              : 'Waiting for health result'}
                      </span>{' '}
                      {setupOutcome.message}
                    </div>
                  )}
                </div>
              )}
            </div>
          )}

          {inventory.map((runtime) => {
            const view = healthView(runtime.health)
            const advertisedCompatibility = selectedAgentId
              ? runtime.agentCompatibilityRows.find((item) => item.agentId === selectedAgentId) ??
                (runtime.agentCompatibilityRows.length === 1 &&
                (!runtime.agentCompatibilityRows[0].agentId ||
                  runtime.agentCompatibilityRows[0].agentId === selectedAgentId)
                  ? runtime.agentCompatibilityRows[0]
                  : undefined)
              : undefined
            const handoffLevel = advertisedCompatibility?.control ?? controlLevel
            const compatibility = compatibilityFor(runtime, handoffLevel, advertisedCompatibility, selectedAgentId)
            const observedModels = runtime.modelsObserved ? runtime.models : runtimeModels[runtime.id] ?? []
            const modelsWereProbed = runtime.modelsObserved || modelsProbed[runtime.id]
            const busy = runtimeBusy[runtime.id]
            const isManaged = runtime.ownership === 'managed'
            return (
              <article key={runtime.id} data-testid={`runtime-${runtime.id}`} className="space-y-2.5 rounded-xl border border-border/70 bg-card/50 p-3">
                <div className="flex flex-wrap items-start justify-between gap-2">
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <h3 className="truncate text-[12px] font-semibold text-foreground" title={runtime.id}>{runtimeDisplayLabel(runtime.id, showAdvanced)}</h3>
                      <Badge className="border-border/60 bg-background/60 px-1.5 text-[9px] text-muted-foreground">
                        {ownershipView(runtime.ownership)}
                      </Badge>
                    </div>
                    <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 font-mono text-[9px] text-muted-foreground">
                      <span title="Runtime kind">kind: {runtimeKindLabel(runtime.kind, showAdvanced)}</span>
                      <span title="Runtime protocol">protocol: {runtime.protocol ?? 'unknown'}</span>
                      <span title="Runtime version">version: {runtime.version ?? 'unknown'}</span>
                    </div>
                  </div>
                  <div className="flex items-center gap-1.5">
                    <Badge className={cn('font-mono text-[9px]', view.className)}>
                      <span className={cn('h-1.5 w-1.5 rounded-full', view.dot)} />
                      {view.label}
                    </Badge>
                    {isManaged && busy && <span className="font-mono text-[9px] text-muted-foreground">working…</span>}
                  </div>
                </div>

                {runtime.endpoint && (
                  <div className="truncate rounded-md border border-border/50 bg-background/35 px-2 py-1.5 font-mono text-[10px] text-muted-foreground">
                    endpoint: <span className="text-foreground">{runtime.endpoint}</span>
                  </div>
                )}

                {runtime.health === 'observed' && (
                  <div className="rounded-md border border-sky-500/25 bg-sky-500/5 px-2 py-1.5 text-[10px] text-sky-200">
                    Observed, not healthy yet. The endpoint is visible, but no successful health probe has been reported.
                  </div>
                )}
                {(runtime.health === 'degraded' || runtime.health === 'down' || runtime.health === 'unavailable' || runtime.health === 'failed') && (
                  <div className="rounded-md border border-warning/30 bg-warning/5 px-2 py-1.5 text-[10px] text-warning">
                    {runtime.reason ?? runtime.lastError ?? 'The native inventory did not provide a reason.'}
                  </div>
                )}
                {runtimeNotices[runtime.id] && (
                  <div className="rounded-md border border-sky-500/25 bg-sky-500/5 px-2 py-1.5 text-[10px] text-sky-200">
                    {runtimeNotices[runtime.id]}
                  </div>
                )}

                <div className="rounded-md border border-border/50 bg-background/30 px-2 py-1.5 text-[10px] text-muted-foreground">
                  <span className="text-foreground">Models observed:</span>{' '}
                  {observedModels.length > 0
                    ? observedModels.map((model) => model.name).join(' · ')
                    : modelsWereProbed
                      ? 'none reported'
                      : 'not observed yet'}
                  {runtimeModelErrors[runtime.id] && (
                    <span className="ml-1 text-warning">({runtimeModelErrors[runtime.id]})</span>
                  )}
                </div>

                {isManaged && (
                  <div className="flex flex-wrap items-center gap-1.5">
                    {runtime.health !== 'healthy' && runtime.health !== 'starting' && (
                      <Button
                        size="sm"
                        className="h-7 bg-brand px-2.5 text-[10px] text-white hover:bg-brand-hover"
                        disabled={busy !== undefined}
                        data-testid={`runtime-start-${runtime.id}`}
                        onClick={() => void changeRuntime(runtime, 'start')}
                      >
                        <Play className="mr-1 h-3 w-3" /> Start
                      </Button>
                    )}
                    {runtime.health !== 'stopped' && runtime.health !== 'down' && runtime.health !== 'unavailable' && (
                      <Button
                        size="sm"
                        variant="outline"
                        className="h-7 px-2.5 text-[10px]"
                        disabled={busy !== undefined}
                        data-testid={`runtime-stop-${runtime.id}`}
                        onClick={() => void changeRuntime(runtime, 'stop')}
                      >
                        Stop
                      </Button>
                    )}
                  </div>
                )}

                <div className="rounded-md border border-border/50 bg-background/30 px-2 py-1.5 text-[10px] text-muted-foreground">
                  <span className="text-foreground">Compatibility:</span>{' '}
                  <span className={cn(
                    compatibility.status === 'supported' && 'text-emerald-300',
                    compatibility.status === 'not_supported' && 'text-warning',
                    compatibility.status === 'unknown' && 'text-sky-300',
                  )}>
                    {compatibilityLabel(compatibility.status)}
                  </span>{' '}
                  — {compatibility.reason}
                </div>

                <AgentHandoffCard
                  runtime={runtime}
                  agentName={boundAgentName}
                  readiness={boundReadiness}
                  controlLevel={handoffLevel}
                  configOptions={acpConfigOptions}
                  configLoading={configLoading}
                  configError={configError}
                  handoffState={handoffState(runtime.id)}
                  onApplyOption={(option, value) => void applyConfigOption(runtime.id, option, value)}
                  onRequestOverride={(endpoint, model) => void requestOverride(runtime.id, endpoint, model)}
                />
              </article>
            )
          })}
        </section>
      )}

      {tab === 'library' && (
        <section data-testid="library-section" className="min-h-0 flex-1 space-y-2 overflow-y-auto pr-0.5">
          <div>
            <h2 className="flex items-center gap-2 text-[13px] font-semibold text-foreground">
              <BookOpen className="h-4 w-4 text-brand" />
              Library
            </h2>
            <p className="mt-0.5 text-[10px] leading-relaxed text-muted-foreground">
              Downloaded artifacts kept on this machine. Fit is an estimate; preparing an endpoint does not select an agent model.
            </p>
          </div>

          {selectedForRemoval.size > 0 && (
            <div className="flex items-center justify-between gap-2 rounded-md border border-brand/40 bg-brand/10 px-3 py-2">
              <span className="text-[11px] text-brand">
                {selectedForRemoval.size} selected · frees {formatBytes(registry.filter((row) => selectedForRemoval.has(row.id)).reduce((total, row) => total + row.size, 0))}
              </span>
              <div className="flex items-center gap-1.5">
                <Button size="sm" variant="ghost" className="h-6 px-2 text-[10px] text-muted-foreground" onClick={() => setSelectedForRemoval(new Set())}>Clear</Button>
                <Button size="sm" className="h-6 bg-red-500/90 px-2 text-[10px] text-white hover:bg-red-600" onClick={() => void bulkRemove()}>
                  <Trash2 className="h-3 w-3" /> Remove selected
                </Button>
              </div>
            </div>
          )}

          {downloads.length > 0 && (
            <div className="space-y-1.5">
              {downloads.map((row) => <DownloadRow key={row.id} row={row} onCancel={cancel} onResume={resume} />)}
            </div>
          )}

          {orphans.length > 0 && (
            <div className="space-y-1.5">
              {orphans.map((orphan) => {
                const relative = orphan.rel.replace(/\.part$/, '')
                const parts = relative.split('/')
                const filename = parts.pop() ?? ''
                const repo = parts.length >= 2 ? `${parts[0]}/${parts[1]}` : relative
                return (
                  <div key={orphan.dest} className="flex items-center justify-between gap-2 rounded-md border border-dashed border-border/70 bg-background/30 px-2 py-1.5">
                    <div className="min-w-0 font-mono text-[10px] text-muted-foreground">Interrupted: {relative} · {formatBytes(orphan.doneBytes)} downloaded</div>
                    <Button size="sm" className="h-6 shrink-0 bg-brand px-2 text-[10px] text-white hover:bg-brand-hover" onClick={() => void resume(repo, filename)}>
                      <Play className="mr-1 h-3 w-3" /> Resume
                    </Button>
                  </div>
                )
              })}
            </div>
          )}

          {registry.length === 0 && downloads.length === 0 && orphans.length === 0 && (
            <div className="rounded-lg border border-dashed border-border p-6 text-center text-[12px] text-muted-foreground" data-testid="library-empty">
              No downloaded artifacts yet. Explore can find models and size estimates; nothing is downloaded until you choose a file.
              <div className="mt-2">
                <Button size="sm" className="h-7 bg-brand text-[10px] text-white hover:bg-brand-hover" onClick={() => setTab('explore')}>
                  Open Explore
                </Button>
              </div>
            </div>
          )}

          {registry.map((row) => {
            const verified = row.verified === true || row.verification === 'verified' || Boolean(row.sha256)
            return (
              <article key={row.id} data-testid={`library-artifact-${row.id}`} className="rounded-lg border border-border/60 bg-background/40 px-3 py-2">
                <div className="flex items-start gap-2">
                  <button
                    type="button"
                    aria-label={`Select ${row.id}`}
                    onClick={() => setSelectedForRemoval((current) => {
                      const next = new Set(current)
                      if (next.has(row.id)) next.delete(row.id)
                      else next.add(row.id)
                      return next
                    })}
                    className={cn('mt-0.5 flex size-4 shrink-0 items-center justify-center rounded border', selectedForRemoval.has(row.id) ? 'border-brand bg-brand text-white' : 'border-border bg-background text-transparent')}
                  >
                    <Check className="h-3 w-3" />
                  </button>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className="truncate text-[12px] font-medium text-foreground">{row.id}</span>
                      <Badge className="bg-emerald-500/20 px-1 text-[8px] text-emerald-300">downloaded</Badge>
                    </div>
                    <div className="mt-1 flex flex-wrap gap-x-2 gap-y-1 font-mono text-[9px] text-muted-foreground">
                      <span>quant {row.quant || 'unknown'}</span>
                      <span>· {formatBytes(row.size)}</span>
                      <span>· ctx {row.ctx > 0 ? row.ctx.toLocaleString() : 'unknown'}</span>
                      <span>· sha {row.sha256 ? `${row.sha256.slice(0, 12)}…` : 'not reported'}</span>
                    </div>
                    <div className="mt-1.5 flex flex-wrap items-center gap-2 text-[9px] text-muted-foreground">
                      <span className="inline-flex items-center gap-1"><FitDot est={artifactFitMap[row.id]} /> Fit estimate</span>
                      <span className={verified ? 'text-emerald-300' : 'text-muted-foreground'}>
                        {verified ? 'verified' : 'verification not reported'}
                      </span>
                    </div>
                    {endpointNotes[row.id] && <div className="mt-1.5 rounded border border-sky-500/25 bg-sky-500/5 px-2 py-1 text-[9px] text-sky-200">{endpointNotes[row.id]}</div>}
                  </div>
                  <div className="flex shrink-0 flex-col items-end gap-1">
                    {canDownload && (
                      <Button size="sm" variant="outline" className="h-6 px-2 text-[9px]" onClick={() => void prepareEndpoint(row.id)}>
                        Prepare endpoint
                      </Button>
                    )}
                    <Button size="sm" variant="ghost" className="h-6 px-1.5 text-[10px] text-muted-foreground hover:text-red-300" onClick={() => void remove(row.id)} aria-label={`Remove ${row.id}`}>
                      <Trash2 className="h-3 w-3" />
                    </Button>
                  </div>
                </div>
                {showAdvanced && row.path && (
                  <div className="mt-2 border-t border-border/40 pt-2 font-mono text-[9px] text-muted-foreground">file: {row.path}</div>
                )}
              </article>
            )
          })}
          {registry.length > 0 && <div className="font-mono text-[9px] text-muted-foreground">{registry.length} artifact{registry.length === 1 ? '' : 's'} · {formatBytes(registryBytes)} on disk</div>}
        </section>
      )}

      {tab === 'explore' && (
        <section data-testid="explore-section" className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto pr-0.5">
          <div className="flex items-center gap-1 rounded-md border border-border/60 bg-background/40 p-0.5">
            {([['discover', 'Discover'], ['gallery', 'Gallery']] as const).map(([id, label]) => (
              <button key={id} type="button" onClick={() => setExploreTab(id)} className={cn('flex-1 rounded px-2 py-1 text-[10px] font-medium', exploreTab === id ? 'bg-brand/15 text-brand' : 'text-muted-foreground hover:text-foreground')}>
                {label}
              </button>
            ))}
          </div>

          {exploreTab === 'discover' && (
            <div className="space-y-3">
              <div className="mb-1 flex items-center gap-2">
                <div className="relative flex-1">
                  <Search className="pointer-events-none absolute left-2 top-1/2 h-3 w-3 -translate-y-1/2 text-muted-foreground" />
                  <Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search models by name or author" className="h-8 pl-7 font-mono text-[11px]" />
                </div>
                {([['downloads', 'Most downloads'], ['likes', 'Most likes'], ['lastModified', 'Recently updated']] as const).map(([id, label]) => (
                  <button key={id} type="button" onClick={() => setSort(id)} className={cn('rounded-md border px-2 py-1 text-[10px]', sort === id ? 'border-brand/50 bg-brand/10 text-brand' : 'border-border text-muted-foreground')}>
                    {label}
                  </button>
                ))}
              </div>

              {hubError && <div className="rounded border border-red-500/30 bg-red-500/5 px-2 py-1.5 text-[11px] text-red-300">Search unavailable: {hubError}. No results are being presented as verified.</div>}

              <div className="grid min-h-0 grid-cols-[minmax(0,280px)_1fr] overflow-hidden rounded-lg border border-border/60">
                <div className="scroll-thin max-h-[560px] overflow-y-auto border-r border-border/60">
                  {hubLoading && catalog.length === 0 && <div className="p-3 font-mono text-[10px] text-muted-foreground">Searching the model library…</div>}
                  {!hubLoading && !hubError && catalog.length === 0 && <div className="p-3 text-[11px] text-muted-foreground">No model estimates found yet. Search by name or author.</div>}
                  {catalog.map((model) => {
                    const caps = hubCaps(model)
                    const active = selected?.id === model.id
                    return (
                      <button key={model.id} type="button" onClick={() => setSelected(model)} className={cn('flex w-full flex-col gap-0.5 border-b border-border/40 px-2.5 py-2 text-left', active ? 'bg-brand/10' : 'hover:bg-accent/40')}>
                        <span className="truncate text-[11px] font-semibold text-foreground">{model.id}</span>
                        <div className="font-mono text-[9px] text-muted-foreground">↓ {formatDownloads(model.downloads)} · ★ {model.likes}{model.lastModified ? ` · ${relativeUpdated(model.lastModified)}` : ''}</div>
                        <div className="flex flex-wrap gap-1">
                          <CapChip on={caps.vision} label="Vision" icon={<Eye className="h-2 w-2" />} />
                          <CapChip on={caps.toolUse} label="Tool use" icon={<Wrench className="h-2 w-2" />} />
                          <CapChip on={caps.reasoning} label="Reasoning" icon={<AlertCircle className="h-2 w-2" />} />
                        </div>
                      </button>
                    )
                  })}
                </div>

                <div className="scroll-thin max-h-[560px] overflow-y-auto p-3">
                  {selected ? (
                    <>
                      <div className="text-[13px] font-semibold text-foreground">{selected.id}</div>
                      <div className="mt-0.5 flex flex-wrap gap-2 font-mono text-[10px] text-muted-foreground"><span>↓ {formatDownloads(selected.downloads)}</span><span>★ {selected.likes}</span>{selected.lastModified && <span>{relativeUpdated(selected.lastModified)}</span>}{selected.pipelineTag && <span>{selected.pipelineTag}</span>}</div>
                      <div className="mt-2 flex flex-wrap gap-1">
                        {hubCaps(selected).vision && <CapChip on label="Vision" icon={<Eye className="h-2.5 w-2.5" />} />}
                        {hubCaps(selected).toolUse && <CapChip on label="Tool use" icon={<Wrench className="h-2.5 w-2.5" />} />}
                        {hubCaps(selected).reasoning && <CapChip on label="Reasoning" icon={<AlertCircle className="h-2.5 w-2.5" />} />}
                        {showAdvanced && hubCaps(selected).gguf && <Badge className="bg-emerald-500/15 px-1 text-[8px] text-emerald-300">GGUF</Badge>}
                        {showAdvanced && hubCaps(selected).mlx && <Badge className="bg-sky-500/15 px-1 text-[8px] text-sky-300">MLX</Badge>}
                      </div>
                      <div className="mt-3 flex items-center justify-between gap-2">
                        <span className="text-[11px] font-semibold">Download estimates</span>
                        <button type="button" onClick={() => setShowAdvanced((value) => !value)} className="font-mono text-[10px] text-brand hover:underline">{showAdvanced ? 'Hide technical details' : 'Show technical details'}</button>
                      </div>
                      <div className="mt-1.5 rounded-md border border-border/60 bg-background/40 p-2.5">
                        {chosen ? (
                          <div className="flex items-center justify-between gap-2">
                            <div className="min-w-0">
                              <div className="text-xs font-semibold text-foreground">{showAdvanced ? quantFromPath(chosen.path) : 'Suggested download'}</div>
                              <div className="font-mono text-[10px] text-muted-foreground">Size estimate: {formatBytes(chosen.size)}{showAdvanced ? ` · ${chosen.path}` : ''}</div>
                            </div>
                            <Button size="sm" className="h-7 shrink-0 bg-brand px-3 text-[10px] text-black hover:bg-brand/90" disabled={busyFile !== null || !canDownload} onClick={() => void download(selected.id, chosen)}>
                              {busyFile === chosen.path ? <span className="mr-1 text-[9px]">starting…</span> : <Download className="mr-1 h-3 w-3" />} Download
                            </Button>
                          </div>
                        ) : (
                          <div className="font-mono text-[10px] text-muted-foreground">No downloadable file was listed for this result.</div>
                        )}
                        {showAdvanced && gguf.length > 1 && (
                          <div className="mt-3 space-y-1.5 border-t border-border/40 pt-2">
                            <div className="text-[10px] font-medium text-muted-foreground">Available quantization estimates</div>
                            <div className="flex flex-wrap gap-1">
                              {gguf.slice(0, 10).map((file) => {
                                const quant = quantFromPath(file.path)
                                const isDefault = quant === (recommended?.quant ?? DEFAULT_QUANT)
                                return (
                                  <button key={file.path} type="button" onClick={() => { setPicked(file.path); setAutoPicked(false) }} className={cn('flex items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-[9px]', chosen?.path === file.path ? 'border-brand/60 bg-brand/10 text-brand' : 'border-border/50 text-muted-foreground hover:text-foreground')}>
                                    <FitDot est={fitMap[file.path]} /><span>{quant}</span>{isDefault && <span className="rounded bg-emerald-500/20 px-1 text-[8px] text-emerald-300">suggested</span>}<span className="opacity-70">{formatBytes(file.size)}</span>
                                  </button>
                                )
                              })}
                            </div>
                            <div className="flex flex-wrap items-center gap-2 pt-1 font-mono text-[9px] text-muted-foreground">
                              <span className="inline-flex items-center gap-1"><span className="h-1.5 w-1.5 rounded-full bg-emerald-400" />fits</span>
                              <span className="inline-flex items-center gap-1"><span className="h-1.5 w-1.5 rounded-full bg-warning" />may be slow</span>
                              <span className="inline-flex items-center gap-1"><span className="h-1.5 w-1.5 rounded-full bg-red-400" />outside estimate</span>
                              <span>estimate at {FIT_CTX_LABEL} context</span>
                            </div>
                          </div>
                        )}
                        {canDownload && gguf.length > 1 && (
                          <Button size="sm" variant="ghost" className="mt-2 h-6 px-1.5 text-[9px] text-brand" disabled={pickingBest} onClick={() => void autoPick()}>
                            {pickingBest ? 'Estimating…' : 'Suggest a hardware-fit build'}{autoPicked ? ' · selected' : ''}
                          </Button>
                        )}
                      </div>
                      {!canDownload && <div className="mt-2 text-[10px] text-warning">Downloading needs the Tauri desktop shell. The results and fit values shown here are estimates only.</div>}
                    </>
                  ) : (
                    <div className="text-[11px] text-muted-foreground">Search the model library to see size and hardware-fit estimates. No model has been selected for an agent.</div>
                  )}
                </div>
              </div>
            </div>
          )}

          {exploreTab === 'gallery' && (
            <div className="space-y-2">
              <div className="rounded-md border border-border/60 bg-background/40 px-2.5 py-2 text-[11px] text-muted-foreground">Advanced gallery inspector. It parses declared entries and hashes; it does not install or start anything.</div>
              {!canDownload && <div className="rounded border border-warning/30 bg-warning/5 px-2 py-1.5 text-[11px] text-warning">Gallery parsing needs the Tauri desktop shell.</div>}
              <textarea value={galleryYaml} onChange={(event) => setGalleryYaml(event.target.value)} spellCheck={false} className="h-28 w-full resize-y rounded-md border border-border/60 bg-background/40 p-2 font-mono text-[10px] text-foreground focus:border-brand/60 focus:outline-none" placeholder="Paste a gallery index…" />
              <div className="flex items-center gap-1.5">
                <Button size="sm" className="h-7 bg-brand px-2.5 text-[10px] text-white hover:bg-brand-hover" disabled={!canDownload || galleryBusy || !galleryYaml.trim()} onClick={() => void parseGallery()}>{galleryBusy ? 'Parsing…' : 'Parse & inspect'}</Button>
                {galleryYaml !== GALLERY_SAMPLE && <Button size="sm" variant="ghost" className="h-7 px-2 text-[10px] text-muted-foreground hover:text-foreground" onClick={() => { setGalleryYaml(GALLERY_SAMPLE); setGallery(null); setGalleryError(null) }}>Reset sample</Button>}
              </div>
              {galleryError && <div className="rounded border border-red-500/30 bg-red-500/5 px-2 py-1.5 font-mono text-[10px] text-red-300">{galleryError}</div>}
              {gallery && (
                <div className="space-y-1.5">
                  <div className="font-mono text-[10px] text-muted-foreground">{gallery.models.length} entr{gallery.models.length === 1 ? 'y' : 'ies'} · declared hashes, not local verification</div>
                  {gallery.models.map((entry: GalleryEntry) => (
                    <div key={entry.id} className="rounded-md border border-border/60 bg-background/40 px-2.5 py-2">
                      <div className="flex flex-wrap items-center gap-1.5"><span className="font-mono text-[11px] font-medium text-foreground">{entry.id}</span>{entry.backend_override && <Badge className="bg-sky-500/15 px-1 text-[8px] text-sky-300">runtime: {entry.backend_override}</Badge>}{entry.preload && <Badge className="bg-warning/15 px-1 text-[8px] text-warning">preload</Badge>}<span className="ml-auto font-mono text-[9px] text-muted-foreground">{entry.files.length} pinned</span></div>
                      <div className="mt-1.5 space-y-1">{entry.files.map((file) => <div key={file.path} className="flex items-center gap-2 font-mono text-[9px] text-muted-foreground"><span className="min-w-0 truncate">{file.path}</span><span className="shrink-0 text-emerald-300/80" title={`sha256 ${file.sha256}`}>sha:{file.sha256.slice(0, 12)}…</span></div>)}</div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
        </section>
      )}

      {tab === 'hardware' && (
        <section data-testid="hardware-section" className="min-h-0 flex-1 space-y-3 overflow-y-auto pr-0.5">
          <div className="rounded-xl border border-border/70 bg-card/60 p-3.5 space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border/40 pb-2.5">
              <div className="flex items-center gap-2"><Cpu className="h-4 w-4 text-brand" /><span className="text-xs font-semibold text-foreground">Hardware</span></div>
              <div className="flex items-center gap-2 font-mono text-[10px] text-muted-foreground"><Badge variant="outline" className="border-border/60 font-mono text-[10px]">RAM: {formatBytes(ram)}</Badge>{hw?.gpu && hw.gpu !== '—' && <Badge variant="outline" className="border-border/60 font-mono text-[10px]">GPU: {hw.gpu}</Badge>}<Badge variant="outline" className="border-border/60 font-mono text-[10px]">CPU: {cpuCores(hw) || '—'} Cores</Badge></div>
            </div>
            <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-3">
              <div className="flex flex-col justify-between rounded-lg border border-emerald-500/20 bg-emerald-500/10 p-2.5 text-emerald-400"><div><div className="flex items-center justify-between gap-1"><div className="flex items-center gap-1.5"><span className="inline-block h-2 w-2 rounded-full bg-emerald-400" /><span className="text-[11px] font-semibold text-foreground">Fast &amp; Smooth Fit</span></div><span className="font-mono text-[9px] opacity-80">&le; {hardwareFitSummary.comfortableLimitGb.toFixed(1)} GB</span></div><div className="mt-1 text-[11px] font-medium text-foreground">60% Safe Memory Zone</div><p className="mt-0.5 text-[10px] leading-tight text-muted-foreground">Model files up to {hardwareFitSummary.comfortableLimitGb.toFixed(1)} GB fit comfortably in your {formatBytes(ram)} RAM with room for long contexts.</p></div></div>
              <div className="flex flex-col justify-between rounded-lg border border-amber-500/20 bg-amber-500/10 p-2.5 text-amber-400"><div><div className="flex items-center justify-between gap-1"><div className="flex items-center gap-1.5"><span className="inline-block h-2 w-2 rounded-full bg-amber-400" /><span className="text-[11px] font-semibold text-foreground">Capable / Moderate</span></div><span className="font-mono text-[9px] opacity-80">{hardwareFitSummary.comfortableLimitGb.toFixed(1)} – {hardwareFitSummary.warningLimitGb.toFixed(1)} GB</span></div><div className="mt-1 text-[11px] font-medium text-foreground">60% – 85% Memory Usage</div><p className="mt-0.5 text-[10px] leading-tight text-muted-foreground">Models in this range may experience memory pressure during deep context turns.</p></div></div>
              <div className="flex flex-col justify-between rounded-lg border border-red-500/20 bg-red-500/10 p-2.5 text-red-400"><div><div className="flex items-center justify-between gap-1"><div className="flex items-center gap-1.5"><span className="inline-block h-2 w-2 rounded-full bg-red-400" /><span className="text-[11px] font-semibold text-foreground">Resource Intensive</span></div><span className="font-mono text-[9px] opacity-80">&gt; {hardwareFitSummary.warningLimitGb.toFixed(1)} GB</span></div><div className="mt-1 text-[11px] font-medium text-foreground">High RAM / GPU Required</div><p className="mt-0.5 text-[10px] leading-tight text-muted-foreground">Exceeds the safe host-memory estimate; a dedicated accelerator may be needed.</p></div></div>
            </div>
          </div>

          <div className="grid gap-3 sm:grid-cols-2">
            <div className="rounded-lg border border-border/60 bg-background/40 p-3"><div className="mb-2 flex items-center gap-1.5 text-[12px] font-semibold"><Cpu className="h-3.5 w-3.5 text-brand" />CPU</div><div className="font-mono text-[11px]">{cpuCores(hw) || '—'} cores</div></div>
            <div className="rounded-lg border border-border/60 bg-background/40 p-3"><div className="mb-2 flex items-center gap-1.5 text-[12px] font-semibold"><Gauge className="h-3.5 w-3.5 text-brand" />Memory</div><div className="font-mono text-[11px]">RAM {formatBytes(ram)}</div><div className="mt-1 font-mono text-[10px] text-muted-foreground">GPU class: {hw?.gpu ?? '—'}</div></div>
          </div>

          <div className="rounded-lg border border-border/60 bg-background/40 p-3"><div className="flex items-center justify-between gap-2"><div><div className="text-[11px] font-medium">Offload KV cache to GPU memory</div><div className="text-[10px] text-muted-foreground">Stored in this browser only until the runtime binder lands.</div></div><Switch checked={prefs.kvOffload} onCheckedChange={(value) => savePrefs({ ...prefs, kvOffload: value })} /></div></div>
          <div className="rounded-lg border border-border/60 bg-background/40 p-3"><div className="mb-1 flex items-center gap-1.5 text-[12px] font-semibold"><HardDrive className="h-3.5 w-3.5 text-brand" />Resource monitor</div><div className="font-mono text-[11px] text-muted-foreground">Live RAM probes drive hardware-fit estimates in Explore; disk free is checked before a download starts.</div></div>
          <div className="rounded-lg border border-border/60 bg-background/40 p-3"><div className="flex items-center justify-between gap-2"><div><div className="text-[11px] font-medium">Model loading guardrails</div><div className="text-[10px] text-muted-foreground">UI preference only. Loading still uses the native hardware-fit guard.</div></div><Switch checked={prefs.guardrails} onCheckedChange={(value) => savePrefs({ ...prefs, guardrails: value })} /></div></div>
          <div className="rounded-lg border border-border/60 bg-background/40 p-3"><div className="flex items-center justify-between gap-2"><div><div className="text-[11px] font-medium">Start local service on login</div><div className="text-[10px] text-muted-foreground">UI preference only — login start is not enabled by this screen.</div></div><Switch checked={prefs.startOnLogin} onCheckedChange={(value) => savePrefs({ ...prefs, startOnLogin: value })} /></div></div>
        </section>
      )}
    </div>
  )
}

function messageForRuntime(runtime: RuntimeInventoryRow): string {
  if (runtime.health === 'healthy') return 'Inventory reports a healthy runtime after the start request.'
  if (runtime.health === 'observed') return 'The endpoint is observed, but its health is not confirmed yet.'
  return runtime.reason ?? `The native service reports ${runtime.health}.`
}
