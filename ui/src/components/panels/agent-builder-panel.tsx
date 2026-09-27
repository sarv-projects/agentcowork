'use client'

import * as React from 'react'
import { useState } from 'react'
import { useAppStore } from '@/lib/store'
import { inTauri } from '@/lib/tauri'
import {
  agentRegistryList,
  agentRegistryRemove,
  agentRegistrySave,
  RegisteredAgent,
} from '@/lib/agent-registry'
import {
  AGENT_TEMPLATES,
  AgentBundle,
  bundleFromTemplate,
  bundleToToml,
  exportBundle,
  slug,
} from '@/lib/agent-builder'
import { suggestAgentNames } from '@/lib/plain-language'
import { getLocalItem } from '@/lib/storage-compat'
import {
  Bot,
  Brain,
  Check,
  Copy,
  Download,
  Pencil,
  Plus,
  Rocket,
  Sparkles,
  Trash2,
  Wrench,
} from 'lucide-react'

type Step = 1 | 2 | 3 | 4

interface Draft extends AgentBundle {}

export default function AgentBuilderPanel() {
  const setCenterScreen = useAppStore((s) => s.setCenterScreen)
  const [step, setStep] = useState<Step>(1)
  const [templateId, setTemplateId] = useState<string>('general')
  const [name, setName] = useState<string>('')
  const [emoji, setEmoji] = useState<string>('🤖')
  // ADR-0005: v1's engine is an installed ACP agent. The wizard does not
  // prefill any CLI — a hardcoded default agent is the bug this removes — so
  // the user must name the agent they installed before the bundle can save.
  const [engineKind, setEngineKind] = useState<'acp' | 'model-only'>('acp')
  const [acpCli, setAcpCli] = useState<string>('')
  const [provider, setProvider] = useState<string>('')
  const [model, setModel] = useState<string>('')
  const [mcpServers, setMcpServers] = useState<string>('')
  const [connectors, setConnectors] = useState<string>('')
  const [skills, setSkills] = useState<string>('')
  const [blueprints, setBlueprints] = useState<string>('')
  const [bundle, setBundle] = useState<AgentBundle | null>(null)
  const [saved, setSaved] = useState(false)
  const [copied, setCopied] = useState(false)

  // The Tauri-backed registry (P31.10): `agent_registry_list` reads the Rust
  // AgentRegistry (`~/.agentcowork/agents/`). Outside Tauri we mirror the same
  // shape in localStorage (demo state).
  const [registryAgents, setRegistryAgents] = useState<RegisteredAgent[]>([])
  const [registryErr, setRegistryErr] = useState<string | null>(null)

  const live = inTauri()

  const loadRegistry = async () => {
    try {
      const { agents } = await agentRegistryList()
      setRegistryAgents(agents)
      setRegistryErr(null)
    } catch (e) {
      setRegistryErr(String(e))
    }
  }

  React.useEffect(() => {
    void loadRegistry()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Demo mirror (browser preview): full bundles, localStorage-backed.
  const [localBundles, setLocalBundles] = useState<AgentBundle[]>(() => {
    try {
      // DEC-053: legacy `everyaios.*` key honored + promoted once.
      const raw = getLocalItem('agentcowork.agents')
      return raw ? (JSON.parse(raw) as AgentBundle[]) : []
    } catch {
      return []
    }
  })

  const persistBundles = (list: AgentBundle[]) => {
    setLocalBundles(list)
    try {
      localStorage.setItem('agentcowork.agents', JSON.stringify(list))
    } catch {
      /* storage unavailable — demo state only */
    }
  }

  /** The row count shown under "Your agents": Tauri registry or demo mirror. */
  const agentCount = live ? registryAgents.length : localBundles.length

  const startFromTemplate = (id: string) => {
    setTemplateId(id)
    const t = AGENT_TEMPLATES.find((x) => x.id === id)
    setName(t?.label ?? '')
    setEmoji(t?.emoji ?? '🤖')
    setEngineKind('acp')
    setAcpCli('')
    setMcpServers(t?.preset.mcpServers.join(', ') ?? '')
    setConnectors(t?.preset.connectors.join(', ') ?? '')
    setSkills(t?.preset.skills.join(', ') ?? '')
    setBlueprints(t?.preset.blueprints.join(', ') ?? '')
    setStep(1)
    setSaved(false)
    setBundle(null)
  }

  const splitIds = (raw: string): string[] =>
    raw.split(',').map((s) => s.trim()).filter(Boolean)

  /** A bundle may only save with a bound engine (ADR-0005). Model-only needs
   * no id; ACP needs the installed agent's id. */
  const engineReady = engineKind === 'model-only' || acpCli.trim().length > 0

  const build = (): AgentBundle => {
    const b = bundleFromTemplate(templateId, name || 'My Agent', emoji)
    b.engine =
      engineKind === 'acp' ? { kind: 'acp', cli: acpCli.trim() } : { kind: 'model-only' }
    if (provider || model) b.model = { provider: provider || undefined, model: model || undefined }
    b.mcpServers = splitIds(mcpServers)
    b.connectors = splitIds(connectors)
    b.skills = splitIds(skills)
    b.blueprints = splitIds(blueprints)
    return b
  }

  const saveAgent = async () => {
    const b = build()
    if (live) {
      // The Rust store is the durable registry — save the exact agent.toml
      // the crate parses (bundleToToml emits the serde schema).
      try {
        await agentRegistrySave(bundleToToml(b))
        await loadRegistry()
        setBundle(b)
        setSaved(true)
        setRegistryErr(null)
      } catch (e) {
        setRegistryErr(String(e))
      }
      return
    }
    const list = [...localBundles.filter((x) => slug(x.name) !== slug(b.name)), b]
    persistBundles(list)
    setBundle(b)
    setSaved(true)
  }

  const removeAgent = async (id: string) => {
    if (live) {
      try {
        await agentRegistryRemove(id)
        await loadRegistry()
      } catch (e) {
        setRegistryErr(String(e))
      }
      return
    }
    persistBundles(localBundles.filter((x) => slug(x.name) !== id))
  }

  const copyToml = async () => {
    const b = bundle ?? build()
    const { content } = exportBundle(b)
    try {
      await navigator.clipboard.writeText(content)
      setCopied(true)
      setTimeout(() => setCopied(false), 1200)
    } catch {
      /* clipboard unavailable */
    }
  }

  const downloadToml = () => {
    const b = bundle ?? build()
    const { name: fname, content } = exportBundle(b)
    const blob = new Blob([content], { type: 'text/plain' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = fname
    a.click()
    URL.revokeObjectURL(url)
  }

  const steps: { n: Step; label: string; icon: typeof Bot }[] = [
    { n: 1, label: 'Identity', icon: Bot },
    { n: 2, label: 'Brain', icon: Brain },
    { n: 3, label: 'Capabilities', icon: Wrench },
    { n: 4, label: 'Workflows', icon: Rocket },
  ]

  return (
    <div className="mx-auto max-w-3xl px-6 py-6">
      <div className="mb-5 flex items-center gap-2">
        <Sparkles className="h-4 w-4 text-brand" />
        <h2 className="text-lg font-semibold text-foreground">Custom Agent Builder</h2>
      </div>

      {/* Stepper */}
      <div className="mb-6 flex items-center gap-2">
        {steps.map((s, i) => (
          <React.Fragment key={s.n}>
            {i > 0 && <div className="h-px flex-1 bg-border" />}
            <button
              onClick={() => setStep(s.n)}
              className={`flex items-center gap-1.5 rounded-full px-3 py-1.5 text-xs transition-colors ${
                step === s.n
                  ? 'bg-brand/15 text-brand ring-1 ring-brand/30'
                  : 'text-muted-foreground hover:text-foreground'
              }`}
            >
              <s.icon className="h-3 w-3" />
              {s.label}
            </button>
          </React.Fragment>
        ))}
      </div>

      {step === 1 && (
        <div className="space-y-4">
          <div>
            <div className="mb-2 text-xs font-medium uppercase tracking-wide text-muted-foreground">
              Start from a template
            </div>
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
              {AGENT_TEMPLATES.map((t) => (
                <button
                  key={t.id}
                  onClick={() => startFromTemplate(t.id)}
                  className={`rounded-lg border p-3 text-left transition-colors ${
                    templateId === t.id
                      ? 'border-brand/50 bg-brand/10'
                      : 'border-border bg-card hover:border-brand/30'
                  }`}
                >
                  <div className="text-xl">{t.emoji}</div>
                  <div className="mt-1 text-xs font-medium text-foreground">{t.label}</div>
                  <div className="mt-0.5 line-clamp-2 text-[10px] leading-tight text-muted-foreground">
                    {t.description}
                  </div>
                </button>
              ))}
            </div>
          </div>
          <div className="rounded-lg border border-border bg-card p-3">
            <label className="mb-1 block text-xs font-medium text-foreground">
              Give your agent a name
            </label>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. Budget Analyst"
              className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground outline-none focus:border-brand/50"
            />
            {/* P32.2 — the name-your-agent ownership moment (Wharton: naming
                drives ownership). Suggested names are one tap away so the
                step never stalls on a blank field. */}
            <div className="mt-2">
              <div className="mb-1 text-[10px] text-muted-foreground">
                Or pick one — naming it makes it yours:
              </div>
              <div className="flex flex-wrap gap-1">
                {suggestAgentNames(4).map((n) => (
                  <button
                    key={n}
                    type="button"
                    onClick={() => setName(n)}
                    className="rounded-full border border-border bg-background/50 px-2 py-0.5 text-[11px] text-muted-foreground transition-colors hover:border-brand/40 hover:text-foreground"
                  >
                    {n}
                  </button>
                ))}
              </div>
            </div>
            <div className="mt-1 text-[11px] text-muted-foreground">
              id: <code className="text-brand">{slug(name || 'agent')}</code>
            </div>
          </div>
          <div className="flex justify-end">
            <button
              onClick={() => setStep(2)}
              className="rounded-md bg-brand px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-brand-hover"
            >
              Next — Brain
            </button>
          </div>
        </div>
      )}

      {step === 2 && (
        <div className="space-y-3">
          <div className="rounded-lg border border-border bg-card p-3">
            <div className="mb-2 text-xs font-medium text-foreground">Engine binding (P31.8)</div>
            <div className="mb-2 text-[11px] text-muted-foreground">
              Your agent runs an installed external agent (ADR-0005). AgentCowork has no built-in engine in v1.
            </div>
            {(
              [
                ['acp', 'ACP agent (installed CLI)'],
                ['model-only', 'Model-only (chat, no tools)'],
              ] as const
            ).map(([k, label]) => (
              <label key={k} className="flex items-center gap-2 py-1 text-sm text-foreground">
                <input
                  type="radio"
                  checked={engineKind === k}
                  onChange={() => setEngineKind(k)}
                  className="accent-brand"
                />
                {label}
              </label>
            ))}
            {engineKind === 'acp' && (
              <input
                value={acpCli}
                onChange={(e) => setAcpCli(e.target.value)}
                placeholder="installed agent id (e.g. claude-code)"
                className="mt-2 w-full rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground outline-none focus:border-brand/50"
              />
            )}
            {!engineReady && (
              <div className="mt-2 text-[11px] text-amber-500">
                Name the installed agent this bundle runs — a bundle with no engine cannot be saved.
              </div>
            )}
          </div>
          <div className="rounded-lg border border-border bg-card p-3">
            <div className="mb-2 text-xs font-medium text-foreground">
              Model pin (P31.7) — leave empty to inherit from the chat bar
            </div>
            <div className="flex gap-2">
              <input
                value={provider}
                onChange={(e) => setProvider(e.target.value)}
                placeholder="provider (optional)"
                className="w-1/2 rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground outline-none focus:border-brand/50"
              />
              <input
                value={model}
                onChange={(e) => setModel(e.target.value)}
                placeholder="model (optional)"
                className="w-1/2 rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground outline-none focus:border-brand/50"
              />
            </div>
          </div>
          <div className="flex justify-between">
            <button onClick={() => setStep(1)} className="text-sm text-muted-foreground hover:text-foreground">
              ← Back
            </button>
            <button
              onClick={() => setStep(3)}
              className="rounded-md bg-brand px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-brand-hover"
            >
              Next — Capabilities
            </button>
          </div>
        </div>
      )}

      {step === 3 && (
        <div className="space-y-3">
          <IdListField label="MCP servers (exact subset, never all)" value={mcpServers} onChange={setMcpServers} placeholder="filesystem, github" />
          <IdListField label="Connectors" value={connectors} onChange={setConnectors} placeholder="gmail" />
          <IdListField label="Skills" value={skills} onChange={setSkills} placeholder="spreadsheet" />
          <div className="flex justify-between">
            <button onClick={() => setStep(2)} className="text-sm text-muted-foreground hover:text-foreground">
              ← Back
            </button>
            <button
              onClick={() => setStep(4)}
              className="rounded-md bg-brand px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-brand-hover"
            >
              Next — Workflows
            </button>
          </div>
        </div>
      )}

      {step === 4 && (
        <div className="space-y-3">
          <div className="rounded-lg border border-border bg-card p-3 text-sm text-muted-foreground">
            Blueprints (B2) and scheduled automations (B7) attach to this agent — runs land in the
            audit timeline (P31.9). Linking happens from the Automations panel once the agent is
            saved; the bundle carries the ids.
          </div>
          <div className="rounded-lg border border-border bg-card p-3">
            <div className="mb-2 text-xs font-medium text-foreground">Workflow ids</div>
            <input
              value={blueprints}
              onChange={(e) => setBlueprints(e.target.value)}
              placeholder="blueprint-id (comma separated)"
              className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground outline-none focus:border-brand/50"
            />
          </div>
          <div className="flex justify-between">
            <button onClick={() => setStep(3)} className="text-sm text-muted-foreground hover:text-foreground">
              ← Back
            </button>
            <button
              onClick={saveAgent}
              disabled={!engineReady}
              title={engineReady ? undefined : 'Bind an engine in step 2 first'}
              className="rounded-md bg-brand px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-brand-hover disabled:cursor-not-allowed disabled:opacity-50"
            >
              {saved ? <span className="flex items-center gap-1"><Check className="h-3.5 w-3.5" /> Saved</span> : 'Save agent'}
            </button>
          </div>
        </div>
      )}

      {saved && bundle && (
        <div className="mt-4 rounded-lg border border-brand/30 bg-brand/5 p-3">
          <div className="flex items-center justify-between">
            <div className="text-sm font-medium text-foreground">
              {bundle.emoji} {bundle.name} saved to registry
            </div>
            <div className="flex items-center gap-2">
              <button
                onClick={copyToml}
                className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs text-muted-foreground hover:text-foreground"
              >
                {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
                Copy agent.toml
              </button>
              <button
                onClick={downloadToml}
                className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs text-muted-foreground hover:text-foreground"
              >
                <Download className="h-3 w-3" />
                Export
              </button>
            </div>
          </div>
          <pre className="mt-2 max-h-40 overflow-auto rounded bg-background/60 p-2 text-[11px] leading-snug text-muted-foreground scroll-thin">
            {bundleToToml(bundle)}
          </pre>
          <button onClick={() => setCenterScreen('chat')} className="mt-2 text-xs text-brand hover:underline">
            Start chatting with {bundle.name} →
          </button>
        </div>
      )}

      {/* Registry (P31.10) — the Rust AgentRegistry in Tauri, demo mirror out */}
      <div className="mt-8">
        <div className="mb-2 flex items-center justify-between">
          <div className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
            Your agents ({agentCount})
          </div>
          <button
            onClick={() => {
              setStep(1)
              setName('')
              setSaved(false)
              setBundle(null)
            }}
            className="flex items-center gap-1 text-xs text-brand hover:text-brand"
          >
            <Plus className="h-3 w-3" /> New
          </button>
        </div>
        {registryErr && (
          <div className="mb-2 rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
            {registryErr}
          </div>
        )}
        {agentCount === 0 ? (
          <div className="rounded-lg border border-dashed border-border p-6 text-center text-sm text-muted-foreground">
            No custom agents yet — pick a template above to create your first one.
          </div>
        ) : live ? (
          <div className="space-y-2">
            {registryAgents.map((a) => (
              <div
                key={a.id}
                className="flex items-center gap-3 rounded-lg border border-border bg-card p-3"
              >
                <div className="text-xl">{a.emoji}</div>
                <div className="min-w-0 flex-1">
                  <div className="text-sm font-medium text-foreground">{a.name}</div>
                  <div className="truncate text-xs text-muted-foreground">{a.description}</div>
                  <div className="mt-0.5 flex flex-wrap gap-1 text-[10px] text-muted-foreground">
                    <span className="rounded bg-background/60 px-1.5 py-0.5">
                      engine: {a.engine.toLowerCase()}
                    </span>
                    <span className="rounded bg-background/60 px-1.5 py-0.5">id: {a.id}</span>
                  </div>
                </div>
                <button
                  onClick={() => removeAgent(a.id)}
                  className="rounded-md border border-border p-1.5 text-muted-foreground hover:text-red-400"
                  title="Remove"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
            ))}
          </div>
        ) : (
          <div className="space-y-2">
            {localBundles.map((b) => (
              <div
                key={slug(b.name)}
                className="flex items-center gap-3 rounded-lg border border-border bg-card p-3"
              >
                <div className="text-xl">{b.emoji}</div>
                <div className="min-w-0 flex-1">
                  <div className="text-sm font-medium text-foreground">{b.name}</div>
                  <div className="truncate text-xs text-muted-foreground">{b.description}</div>
                  <div className="mt-0.5 flex flex-wrap gap-1 text-[10px] text-muted-foreground">
                    <span className="rounded bg-background/60 px-1.5 py-0.5">
                      engine: {b.engine ? (b.engine.kind === 'acp' ? `acp:${b.engine.cli}` : 'model-only') : 'unbound'}
                    </span>
                    {b.model.model && (
                      <span className="rounded bg-background/60 px-1.5 py-0.5">
                        {b.model.model}
                      </span>
                    )}
                    {b.mcpServers.length > 0 && (
                      <span className="rounded bg-background/60 px-1.5 py-0.5">
                        mcp: {b.mcpServers.length}
                      </span>
                    )}
                    {b.connectors.length > 0 && (
                      <span className="rounded bg-background/60 px-1.5 py-0.5">
                        conn: {b.connectors.length}
                      </span>
                    )}
                  </div>
                </div>
                <button
                  onClick={() => {
                    const b2 = localBundles.find((x) => slug(x.name) === slug(b.name))
                    if (b2) {
                      setBundle(b2)
                      setName(b2.name)
                      setEmoji(b2.emoji)
                      setMcpServers(b2.mcpServers.join(', '))
                      setConnectors(b2.connectors.join(', '))
                      setSkills(b2.skills.join(', '))
                      setBlueprints(b2.blueprints.join(', '))
                      setSaved(true)
                      navigator.clipboard.writeText(exportBundle(b2).content).catch(() => {})
                    }
                  }}
                  className="rounded-md border border-border p-1.5 text-muted-foreground hover:text-foreground"
                  title="Edit"
                >
                  <Pencil className="h-3.5 w-3.5" />
                </button>
                <button
                  onClick={() => removeAgent(slug(b.name))}
                  className="rounded-md border border-border p-1.5 text-muted-foreground hover:text-red-400"
                  title="Remove"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}

function IdListField({ label, value, onChange, placeholder }: { label: string; value: string; onChange: (v: string) => void; placeholder?: string }) {
  return (
    <div className="rounded-lg border border-border bg-card p-3">
      <div className="mb-1 flex items-center gap-2 text-xs font-medium text-foreground">
        <Plus className="h-3 w-3 text-brand" />
        {label}
      </div>
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="mt-1 w-full rounded-md border border-border bg-background px-2 py-1.5 font-mono text-xs text-foreground outline-none focus:border-brand/50"
      />
      <div className="mt-1 text-[11px] text-muted-foreground">Comma-separated ids — saved into the bundle.</div>
    </div>
  )
}