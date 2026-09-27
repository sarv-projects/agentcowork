// === Agent runtime + model catalog ============================================
// Underlying coding-agent runtimes (Claude Code, Codex CLI, Grok Build, etc.)
// and the models each can drive. Used by the composer picker + settings panel.

export type AgentCapability =
  | 'code'
  | 'plan'
  | 'research'
  | 'browser'
  | 'shell'
  | 'office'
  | 'vision'
  | 'tools'
  | 'parallel'

export type AgentInstallStatus = 'installed' | 'discovered' | 'available' | 'updating' | 'disabled'

export interface AgentModel {
  id: string
  /** Display label short, e.g. "Sonnet 4.5" */
  label: string
  /** Full model slug, e.g. "claude-sonnet-4.5" */
  slug: string
  /** Provider / vendor key */
  provider: ModelProvider
  /** Context window in tokens */
  context: number
  /** USD per 1M input tokens */
  inputPrice: number
  /** USD per 1M output tokens */
  outputPrice: number
  /** Whether the model is currently reachable from this runtime */
  available: boolean
  /** Capabilities the model is strong at */
  strengths: string[]
  /** Optional accent for logo dot */
  tone?: string
  /** Recommended for */
  recommendedFor?: string
}

import { isAgentReady, type AgentReadiness, type RuntimeLocation } from './acp'

export type ModelProvider =
  | 'anthropic'
  | 'openai'
  | 'xai'
  | 'google'
  | 'deepseek'
  | 'meta'
  | 'mistral'
  | 'qwen'

export interface AgentRuntime {
  id: string
  /** Short name, e.g. "Claude Code" */
  name: string
  /** Vendor / publisher */
  vendor: string
  /** One-line tagline */
  tagline: string
  /** Install/discovery status on this machine */
  status: AgentInstallStatus
  /** P71.3f — the canonical readiness state from the Rust directory. When
   * present it answers `isRuntimeUsable`; `status` stays the UI's interaction
   * vocabulary (disabled/updating are user/install activity, not readiness). */
  readiness?: AgentReadiness
  /** A verified runtime location was found, even if it cannot launch yet. */
  discovered?: boolean
  /** The current adapter can launch this runtime in this environment. */
  launchable?: boolean
  /** Binary path if installed */
  path?: string
  /** P66 — non-secret discovery/install provenance */
  location?: RuntimeLocation
  /** Reported version */
  version?: string
  /** Logo mark — 1-2 chars shown in a colored square */
  mark: string
  /** Tailwind bg/text classes for the logo square */
  accent: string
  /** Capabilities this runtime exposes to AgentCowork */
  capabilities: AgentCapability[]
  /** Models this runtime can drive (must reference MODEL ids) */
  models: string[]
  /** Default model id */
  defaultModel: string
  /** Whether AgentCowork can invoke this runtime headless */
  headless: boolean
  /** Whether the runtime can sandbox (Guard-aware) */
  sandbox: 'strict' | 'soft' | 'none'
  /** Notes / install hint */
  note?: string
  /** P50.3.9 — governance truth: how much of this agent's effects AgentCowork
   * actually governs. Rendered in the picker; never imply un-audited coverage. */
  governance?: import('./acp').GovernanceInfo
}

// === Model catalog ============================================================
// Flat list of every model any runtime can drive. IDs are stable slugs.

export const MODELS: AgentModel[] = [
  // Anthropic
  {
    id: 'claude-opus-4.1',
    label: 'Opus 4.1',
    slug: 'claude-opus-4-1-20250805',
    provider: 'anthropic',
    context: 200_000,
    inputPrice: 15,
    outputPrice: 75,
    available: true,
    strengths: ['agentic', 'long-context', 'reasoning', 'vision'],
    recommendedFor: 'Hard multi-step agents',
    tone: 'bg-sky-500/20 text-sky-300',
  },
  {
    id: 'claude-sonnet-4.5',
    label: 'Sonnet 4.5',
    slug: 'claude-sonnet-4-5-20250929',
    provider: 'anthropic',
    context: 200_000,
    inputPrice: 3,
    outputPrice: 15,
    available: true,
    strengths: ['balanced', 'code', 'vision'],
    recommendedFor: 'Default coding',
    tone: 'bg-sky-500/20 text-sky-300',
  },
  {
    id: 'claude-haiku-4.5',
    label: 'Haiku 4.5',
    slug: 'claude-haiku-4-5',
    provider: 'anthropic',
    context: 200_000,
    inputPrice: 0.8,
    outputPrice: 4,
    available: true,
    strengths: ['fast', 'cheap', 'classification'],
    recommendedFor: 'Quick turns',
    tone: 'bg-sky-500/20 text-sky-300',
  },
  // OpenAI
  {
    id: 'gpt-5',
    label: 'GPT-5',
    slug: 'gpt-5-2025-08-07',
    provider: 'openai',
    context: 272_000,
    inputPrice: 5,
    outputPrice: 15,
    available: true,
    strengths: ['reasoning', 'agentic', 'vision'],
    recommendedFor: 'Reasoning-heavy',
    tone: 'bg-emerald-500/20 text-emerald-300',
  },
  {
    id: 'gpt-5-codex',
    label: 'GPT-5-Codex',
    slug: 'gpt-5-codex',
    provider: 'openai',
    context: 272_000,
    inputPrice: 5,
    outputPrice: 15,
    available: true,
    strengths: ['code', 'diff', 'parallel'],
    recommendedFor: 'Bulk code edits',
    tone: 'bg-emerald-500/20 text-emerald-300',
  },
  {
    id: 'gpt-5-mini',
    label: 'GPT-5 mini',
    slug: 'gpt-5-mini',
    provider: 'openai',
    context: 128_000,
    inputPrice: 0.25,
    outputPrice: 2,
    available: true,
    strengths: ['fast', 'cheap'],
    recommendedFor: 'Quick turns',
    tone: 'bg-emerald-500/20 text-emerald-300',
  },
  {
    id: 'o4-mini',
    label: 'o4-mini',
    slug: 'o4-mini-2025-07-18',
    provider: 'openai',
    context: 200_000,
    inputPrice: 1.1,
    outputPrice: 4.4,
    available: true,
    strengths: ['reasoning', 'tools'],
    recommendedFor: 'Tool-heavy',
    tone: 'bg-emerald-500/20 text-emerald-300',
  },
  // xAI
  {
    id: 'grok-4',
    label: 'Grok 4',
    slug: 'grok-4-0809',
    provider: 'xai',
    context: 256_000,
    inputPrice: 3,
    outputPrice: 15,
    available: true,
    strengths: ['reasoning', 'realtime', 'tools'],
    recommendedFor: 'Realtime + web',
    tone: 'bg-zinc-500/20 text-zinc-200',
  },
  {
    id: 'grok-4-fast',
    label: 'Grok 4 fast',
    slug: 'grok-4-fast',
    provider: 'xai',
    context: 256_000,
    inputPrice: 0.2,
    outputPrice: 0.5,
    available: true,
    strengths: ['fast', 'cheap', 'realtime'],
    recommendedFor: 'Quick turns',
    tone: 'bg-zinc-500/20 text-zinc-200',
  },
  {
    id: 'grok-4-heavy',
    label: 'Grok 4 heavy',
    slug: 'grok-4-heavy',
    provider: 'xai',
    context: 256_000,
    inputPrice: 12,
    outputPrice: 60,
    available: false,
    strengths: ['deep-reasoning', 'agentic'],
    recommendedFor: 'Deep planning',
    tone: 'bg-zinc-500/20 text-zinc-200',
  },
  // Google
  {
    id: 'gemini-2.5-pro',
    label: 'Gemini 2.5 Pro',
    slug: 'gemini-2.5-pro',
    provider: 'google',
    context: 2_000_000,
    inputPrice: 1.25,
    outputPrice: 10,
    available: true,
    strengths: ['long-context', 'vision', 'video'],
    recommendedFor: 'Huge files',
    tone: 'bg-sky-500/20 text-sky-300',
  },
  {
    id: 'gemini-2.5-flash',
    label: 'Gemini 2.5 Flash',
    slug: 'gemini-2.5-flash',
    provider: 'google',
    context: 1_000_000,
    inputPrice: 0.075,
    outputPrice: 0.3,
    available: true,
    strengths: ['fast', 'cheap', 'vision'],
    recommendedFor: 'Quick turns',
    tone: 'bg-sky-500/20 text-sky-300',
  },
  // DeepSeek
  {
    id: 'deepseek-v3.1',
    label: 'DeepSeek V3.1',
    slug: 'deepseek-chat',
    provider: 'deepseek',
    context: 128_000,
    inputPrice: 0.27,
    outputPrice: 1.1,
    available: true,
    strengths: ['code', 'reasoning', 'cheap'],
    recommendedFor: 'Budget coding',
    tone: 'bg-indigo-500/20 text-indigo-300',
  },
  {
    id: 'deepseek-r1',
    label: 'DeepSeek R1',
    slug: 'deepseek-reasoner',
    provider: 'deepseek',
    context: 128_000,
    inputPrice: 0.55,
    outputPrice: 2.19,
    available: true,
    strengths: ['deep-reasoning', 'math'],
    recommendedFor: 'Hard problems',
    tone: 'bg-indigo-500/20 text-indigo-300',
  },
  // Mistral
  {
    id: 'codestral-25.01',
    label: 'Codestral 25.01',
    slug: 'codestral-2501',
    provider: 'mistral',
    context: 256_000,
    inputPrice: 0.3,
    outputPrice: 0.9,
    available: true,
    strengths: ['code', 'fast'],
    recommendedFor: 'Fill-in-the-middle',
    tone: 'bg-warning/20 text-warning',
  },
  // Meta / local via Ollama
  {
    id: 'llama-3.3-70b',
    label: 'Llama 3.3 70B',
    slug: 'llama3.3:70b',
    provider: 'meta',
    context: 128_000,
    inputPrice: 0,
    outputPrice: 0,
    available: true,
    strengths: ['local', 'private', 'code'],
    recommendedFor: 'Air-gapped',
    tone: 'bg-rose-500/20 text-rose-300',
  },
  {
    id: 'qwen2.5-coder-32b',
    label: 'Qwen2.5 Coder 32B',
    slug: 'qwen2.5-coder:32b',
    provider: 'qwen',
    context: 128_000,
    inputPrice: 0,
    outputPrice: 0,
    available: true,
    strengths: ['local', 'code', 'fill-in-the-middle'],
    recommendedFor: 'Local coding',
    tone: 'bg-rose-500/20 text-rose-300',
  },
]

export const MODEL_MAP: Record<string, AgentModel> = Object.fromEntries(
  MODELS.map((m) => [m.id, m]),
)

export function getModel(id: string): AgentModel | undefined {
  return MODEL_MAP[id]
}

export function formatContext(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(tokens % 1_000_000 ? 1 : 0)}M ctx`
  return `${Math.round(tokens / 1000)}K ctx`
}

export function formatPrice(price: number): string {
  if (price === 0) return 'free'
  if (price < 1) return `$${price.toFixed(2)}`
  return `$${price.toFixed(0)}`
}

// === Agent catalog ============================================================
// Static catalog of the runtimes AgentCowork can drive. **Status is honest**: a
// runtime is only `installed` when the live ACP install-status probe (bridge
// hydration, `acp_install_status`) found it — either AgentCowork-installed or
// auto-discovered on PATH. The static seed marks external runtimes
// `available` (catalog entry, not an install claim) so the pre-hydration and
// plain-browser UI never pretends a CLI exists on this machine.

export const AGENTS: AgentRuntime[] = [
  // P71.2a (ADR-0005 §1) — there is **no** built-in agent row. Every entry here
  // is an external agent the user installs or binds; nothing is force-shown as
  // installed and nothing is "always ready".
  {
    id: 'claude-code',
    name: 'Claude Code',
    vendor: 'Anthropic',
    tagline: 'Terminal coding agent with diff-first edits and MCP tools',
    status: 'available',
    mark: 'CC',
    accent: 'bg-sky-500/90 text-black',
    capabilities: ['code', 'plan', 'shell', 'tools', 'vision'],
    models: ['claude-opus-4.1', 'claude-sonnet-4.5', 'claude-haiku-4.5'],
    defaultModel: 'claude-sonnet-4.5',
    headless: true,
    sandbox: 'soft',
    note: 'Install via `npm i -g @anthropic-ai/claude-code` (or connect — detected on PATH if already installed). Strong at agentic edits.',
  },
  {
    id: 'codex-cli',
    name: 'Codex CLI',
    vendor: 'OpenAI',
    tagline: 'OpenAI coding agent — runs GPT-5-Codex with sandboxed shell',
    status: 'available',
    mark: 'Cx',
    accent: 'bg-emerald-500/90 text-black',
    capabilities: ['code', 'plan', 'shell', 'tools', 'parallel'],
    models: ['gpt-5', 'gpt-5-codex', 'gpt-5-mini', 'o4-mini'],
    defaultModel: 'gpt-5-codex',
    headless: true,
    sandbox: 'strict',
    note: 'Sandboxed by default. Good for parallel bulk edits.',
  },
  {
    id: 'grok-build',
    name: 'Grok Build',
    vendor: 'xAI',
    tagline: 'Grok-powered builder with realtime web + tool calls',
    status: 'available',
    mark: 'Gr',
    accent: 'bg-zinc-700 text-zinc-100',
    capabilities: ['code', 'research', 'browser', 'tools'],
    models: ['grok-4', 'grok-4-fast', 'grok-4-heavy'],
    defaultModel: 'grok-4',
    headless: true,
    sandbox: 'soft',
    note: 'Realtime data advantage — good for research + scraping.',
  },
  {
    id: 'gemini-cli',
    name: 'Gemini CLI',
    vendor: 'Google',
    tagline: 'Massive-context agent — drives Gemini 2.5 Pro (2M ctx)',
    status: 'available',
    mark: 'Ge',
    accent: 'bg-sky-500/90 text-black',
    capabilities: ['code', 'plan', 'research', 'vision', 'tools'],
    models: ['gemini-2.5-pro', 'gemini-2.5-flash'],
    defaultModel: 'gemini-2.5-pro',
    headless: true,
    sandbox: 'soft',
    note: 'Unmatched for huge files / video / whole-repo reads.',
  },
  {
    id: 'cursor-agent',
    name: 'Cursor Agent',
    vendor: 'Cursor',
    tagline: 'IDE-coupled agent — calls Claude / GPT / Gemini through Cursor',
    status: 'available',
    mark: 'Cu',
    accent: 'bg-zinc-600 text-white',
    capabilities: ['code', 'plan', 'tools'],
    models: ['claude-opus-4.1', 'claude-sonnet-4.5', 'gpt-5', 'gpt-5-mini', 'gemini-2.5-pro'],
    defaultModel: 'claude-sonnet-4.5',
    headless: false,
    sandbox: 'none',
    note: 'Requires Cursor desktop running. Best for inline IDE edits.',
  },
  {
    id: 'aider',
    name: 'Aider',
    vendor: 'Open-source',
    tagline: 'Git-first pair-programmer — many models via LiteLLM',
    status: 'available',
    mark: 'Ai',
    accent: 'bg-rose-500/90 text-black',
    capabilities: ['code', 'plan', 'shell'],
    models: [
      'claude-opus-4.1',
      'claude-sonnet-4.5',
      'gpt-5',
      'deepseek-v3.1',
      'codestral-25.01',
    ],
    defaultModel: 'claude-sonnet-4.5',
    headless: true,
    sandbox: 'none',
    note: 'Commits every edit. Bring your own keys via LiteLLM.',
  },
  {
    id: 'opencode',
    name: 'OpenCode',
    vendor: 'Open-source',
    tagline: 'TUI agent — drop-in Aider/Claude Code alternative',
    status: 'available',
    mark: 'Oc',
    accent: 'bg-purple-500/90 text-black',
    capabilities: ['code', 'plan', 'shell', 'tools'],
    models: ['claude-sonnet-4.5', 'gpt-5', 'gpt-5-mini', 'deepseek-v3.1', 'qwen2.5-coder-32b'],
    defaultModel: 'claude-sonnet-4.5',
    headless: true,
    sandbox: 'soft',
    note: 'Supports local Ollama models — fully air-gapped capable.',
  },
]

export const AGENT_MAP: Record<string, AgentRuntime> = Object.fromEntries(
  AGENTS.map((a) => [a.id, a]),
)

export function getAgent(id: string): AgentRuntime | undefined {
  return AGENT_MAP[id]
}

export function getModelsForAgent(agentId: string): AgentModel[] {
  const a = AGENT_MAP[agentId]
  if (!a) return []
  return a.models.map((id) => MODEL_MAP[id]).filter(Boolean) as AgentModel[]
}

/** Model ownership boundary (P60, tightened by P71.2d): **no** runtime owns an
 * AgentCowork model surface any more. Every agent here is an external ACP agent
 * that owns its own model, credentials and routing; the desktop's catalogue,
 * key vault and usage ledger are *observation*, never a control over an agent.
 * The UI must never present a provider/model list as something it can push
 * into an agent. There is no built-in runtime id to single out. */

/** A runtime is usable when its install was verified on this machine. Anything
 * else must not present models — the model list loads live only after install.
 *
 * P71.3f — when the canonical readiness is present it decides: installable is
 * not usable (`launchable` only means the process can start, not that it can
 * serve a turn), so an installed-but-unauthenticated runtime stays out of the
 * model lists. The boolean fallbacks below remain for curated rows that
 * predate the directory read. */
export function isRuntimeUsable(a: AgentRuntime | undefined): boolean {
  if (!a) return false
  if (a.readiness !== undefined) return isAgentReady(a.readiness)
  if (a.launchable !== undefined) return a.launchable
  return a.status === 'installed' || a.status === 'updating'
}

/** P71.3f — project the one readiness state onto the UI's interaction status.
 * A runtime's curated seed never describes an installed runtime; readiness (or
 * the install probe) is the only source of that claim.
 *
 * `disabled` (user policy) and `updating` (install activity) are preserved:
 * they are not readiness states, so readiness must not overwrite them. */
export function readinessToInstallStatus(
  readiness: AgentReadiness,
  current?: AgentInstallStatus,
): AgentInstallStatus {
  if (current === 'disabled' || current === 'updating') return current
  if (isAgentReady(readiness)) return 'installed'
  if (
    readiness === 'discovered' ||
    readiness === 'unknown' ||
    readiness === 'unavailable' ||
    readiness === 'failed'
  ) {
    return 'available'
  }
  // installed | launchable | protocol_compatible | auth_required | authenticating
  return 'discovered'
}

/** Model rows this runtime is allowed to display — **`[]` for every runtime**
 * (`P71.2d`, ADR-0005 §2).
 *
 * No runtime's model surface is AgentCowork's to draw any more: an external ACP
 * agent's models are whatever the agent itself exposes over ACP `configOptions`
 * (`category: "model"`), and the desktop's models.dev catalogue is
 * *observation* (Providers / Local models), never a list this agent receives.
 * The curated seed describes a provider catalog no agent gets, so rendering it
 * would claim control AgentCowork does not have.
 *
 * Kept as the single choke point the picker, the palette and the settings panel
 * all route through: if a model-owning runtime ever returns (the post-v1
 * governed binding, `P71.7`), this is the one function that has to change. */
export function getModelsForAgentLive(_agentId: string, _live?: AgentRuntime[]): AgentModel[] {
  return []
}

export function getDefaultModelForAgent(agentId: string): string {
  return AGENT_MAP[agentId]?.defaultModel ?? 'claude-sonnet-4.5'
}

// === Task routing =============================================================
// When "Auto-route by task" is on, AgentCowork picks the runtime per task kind.

export type TaskKind =
  | 'code'
  | 'plan'
  | 'research'
  | 'browser'
  | 'shell'
  | 'office'
  | 'diff'
  | 'long-context'

export const TASK_LABELS: Record<TaskKind, string> = {
  code: 'Code edits',
  plan: 'Planning',
  research: 'Research',
  browser: 'Browser tasks',
  shell: 'Shell ops',
  office: 'Office (xlsx/docx/pptx/pdf)',
  diff: 'Diff review',
  'long-context': 'Long-context (>200K)',
}

/** Task-kind → agent table. **Empty is the v1 default and it is meaningful:**
 * an empty row means "run this task under the session's bound agent" — v1 has
 * no built-in engine to fall back to, so AgentCowork cannot name a default agent
 * on the user's behalf (ADR-0005 §2). A non-empty value is a choice the user
 * made in Settings. */
export const DEFAULT_ROUTING: Record<TaskKind, string> = {
  code: '',
  plan: '',
  research: '',
  browser: '',
  shell: '',
  office: '',
  diff: '',
  'long-context': '',
}

/** Label for an unset routing row ("follow the bound agent"). */
export const ROUTING_BOUND_AGENT = 'follow the bound agent'

export const CAPABILITY_LABELS: Record<AgentCapability, string> = {
  code: 'Code',
  plan: 'Plan',
  research: 'Research',
  browser: 'Browser',
  shell: 'Shell',
  office: 'Office',
  vision: 'Vision',
  tools: 'Tools',
  parallel: 'Parallel',
}

export const PROVIDER_LABELS: Record<ModelProvider, string> = {
  anthropic: 'Anthropic',
  openai: 'OpenAI',
  xai: 'xAI',
  google: 'Google',
  deepseek: 'DeepSeek',
  meta: 'Meta (local)',
  mistral: 'Mistral',
  qwen: 'Qwen (local)',
}
