// === P31 — Custom agent builder (B9) ==========================================
// Mirrors the Rust agentcowork-agents crate surface: bundle manifest, the 8
// wizard templates, engine binding, model pin, per-agent scopes.
// The Rust registry (AgentRegistry) is the durable store; this module is the
// browser-side builder state + template catalog + TOML export.

/**
 * The bundle's brain (P31.8) — mirrors `agentcowork_agents::bundle::EngineBinding`.
 * ADR-0005: v1 binds an external ACP agent; the built-in engine is post-v1 and
 * is deliberately not a choice here. A bundle with no binding is a draft: it
 * saves, but it is not an agent and never joins the runnable directory.
 */
export type EngineBinding =
  | { kind: 'acp'; cli: string }
  | { kind: 'model-only' }

export interface ModelPin {
  provider?: string
  model?: string
}

export interface ToolScope {
  allow: string[]
  deny: string[]
}

export interface AgentBundle {
  schemaVersion: number
  name: string
  emoji: string
  description: string
  persona?: string
  systemPrompt?: string
  /** Unset = no engine bound yet (a draft — ADR-0005). */
  engine?: EngineBinding
  model: ModelPin
  mcpServers: string[]
  connectors: string[]
  skills: string[]
  tools: ToolScope
  blueprints: string[]
  automations: string[]
}

export const BUNDLE_SCHEMA_VERSION = 1

export type AgentTemplateId =
  | 'general'
  | 'coder'
  | 'researcher'
  | 'email-triager'
  | 'data-analyst'
  | 'writer'
  | 'meeting-notes'
  | 'browser-operator'
  | 'study'

export interface AgentTemplate {
  id: AgentTemplateId
  label: string
  emoji: string
  description: string
  preset: Omit<AgentBundle, 'name' | 'schemaVersion' | 'emoji'>
}

export const AGENT_TEMPLATES: AgentTemplate[] = [
  {
    id: 'general',
    label: 'General',
    emoji: '🤖',
    description: 'A helpful general-purpose assistant.',
    preset: {
      description: 'A helpful general-purpose assistant.',
      model: {},
      mcpServers: [],
      connectors: [],
      skills: [],
      tools: { allow: [], deny: [] },
      blueprints: [],
      automations: [],
    },
  },
  {
    id: 'coder',
    label: 'Coder',
    emoji: '👨‍💻',
    description: 'Writes and fixes code with editor + terminal access.',
    preset: {
      description: 'Writes and fixes code with editor + terminal access.',
      model: {},
      mcpServers: ['filesystem'],
      connectors: [],
      skills: [],
      tools: { allow: ['fs.read', 'fs.write', 'shell', 'search'], deny: [] },
      blueprints: [],
      automations: [],
    },
  },
  {
    id: 'researcher',
    label: 'Researcher',
    emoji: '🔬',
    description: 'Deep web + document research with citation cards.',
    preset: {
      description: 'Deep web + document research with citation cards.',
      model: {},
      mcpServers: [],
      connectors: [],
      skills: [],
      tools: { allow: ['search', 'memory.read'], deny: ['fs.write', 'shell'] },
      blueprints: [],
      automations: [],
    },
  },
  {
    id: 'email-triager',
    label: 'Email Triager',
    emoji: '📬',
    description: 'Triage your inbox: summarize, draft, never send without approval.',
    preset: {
      description: 'Triage your inbox: summarize, draft, never send without approval.',
      model: {},
      mcpServers: [],
      connectors: ['gmail'],
      skills: [],
      tools: { allow: ['email.read', 'email.draft'], deny: [] },
      blueprints: [],
      automations: [],
    },
  },
  {
    id: 'data-analyst',
    label: 'Data Analyst',
    emoji: '📊',
    description: 'Sum, pivot, and chart spreadsheets; never invents numbers.',
    preset: {
      description: 'Sum, pivot, and chart spreadsheets; never invents numbers.',
      model: {},
      mcpServers: [],
      connectors: [],
      skills: ['spreadsheet'],
      tools: { allow: ['office.read', 'office.write'], deny: ['shell'] },
      blueprints: [],
      automations: [],
    },
  },
  {
    id: 'writer',
    label: 'Writer',
    emoji: '✍️',
    description: 'Long-form writing and rewriting with your style memory.',
    preset: {
      description: 'Long-form writing and rewriting with your style memory.',
      model: {},
      mcpServers: [],
      connectors: [],
      skills: [],
      tools: { allow: ['fs.read', 'fs.write'], deny: ['shell'] },
      blueprints: [],
      automations: [],
    },
  },
  {
    id: 'meeting-notes',
    label: 'Meeting Notes',
    emoji: '📝',
    description: 'Turns transcripts into structured notes + action items.',
    preset: {
      description: 'Turns transcripts into structured notes + action items.',
      model: {},
      mcpServers: [],
      connectors: [],
      skills: [],
      tools: { allow: ['office.write'], deny: [] },
      blueprints: [],
      automations: [],
    },
  },
  {
    id: 'browser-operator',
    label: 'Browser Operator',
    emoji: '🌐',
    description: 'Drives the browser: navigate, snapshot, act, verify.',
    preset: {
      description: 'Drives the browser: navigate, snapshot, act, verify.',
      model: {},
      mcpServers: [],
      connectors: [],
      skills: [],
      tools: { allow: ['browser.navigate', 'browser.act', 'browser.snapshot'], deny: [] },
      blueprints: [],
      automations: [],
    },
  },
  // WP7 — the student posture. Read-only on purpose: a learner should be able
  // to hand this to an agent without wondering what it will change. Works with
  // a local model or a free provider row, which is the audience most likely to
  // have neither a key nor a GPU.
  {
    id: 'study',
    label: 'Study Buddy',
    emoji: '🎓',
    description: 'Explains topics in plain words and quizzes you — read-only, nothing on your machine changes.',
    preset: {
      description: 'Explains topics in plain words and quizzes you — read-only, nothing on your machine changes.',
      model: {},
      mcpServers: [],
      connectors: [],
      skills: [],
      tools: { allow: ['search', 'memory.read', 'office.read'], deny: ['fs.write', 'shell'] },
      blueprints: [],
      automations: [],
    },
  },
]

export function templateById(id: string): AgentTemplate {
  // AGENT_TEMPLATES is never empty (the catalog is static); the non-null
  // assertion keeps strict noUncheckedIndexedAccess consumers (coordinator
  // tsconfig) happy.
  return AGENT_TEMPLATES.find((t) => t.id === id) ?? AGENT_TEMPLATES[0]!
}

/** Deterministic id from a display name (mirrors the Rust slug()). */
export function slug(name: string): string {
  const base = name.toLowerCase().replace(/[^a-z0-9]/g, '-')
  return base.replace(/^-+|-+$/g, '') || 'agent'
}

/** Start a bundle from a template + the wizard's chosen name. */
export function bundleFromTemplate(templateId: string, name: string, emoji: string): AgentBundle {
  const t = templateById(templateId)
  return {
    schemaVersion: BUNDLE_SCHEMA_VERSION,
    name,
    emoji,
    ...t.preset,
  }
}

function q(s: string): string {
  return JSON.stringify(s)
}

function arr(a: string[]): string {
  return `[${a.map((x) => q(x)).join(', ')}]`
}

/**
 * The bundle → agent.toml export. Layout mirrors the Rust serializer
 * (`toml::to_string`): every scalar/array field first, then the tables —
 * `[engine]` (Acp newtype only), `[model]`, `[tools]`. In TOML, opening a
 * table absorbs every following key into it, so arrays MUST come before the
 * table headers or they silently land inside `[model]` and are dropped by
 * `AgentBundle::from_toml` (verified against the crate's serde schema).
 */
export function bundleToToml(b: AgentBundle): string {
  const L: string[] = []
  L.push(`schema_version = ${b.schemaVersion}`)
  L.push(`name = ${q(b.name)}`)
  L.push(`emoji = ${q(b.emoji)}`)
  L.push(`description = ${q(b.description)}`)
  if (b.persona) L.push(`persona = ${q(b.persona)}`)
  if (b.systemPrompt) L.push(`system_prompt = ${q(b.systemPrompt)}`)
  L.push(`mcp_servers = ${arr(b.mcpServers)}`)
  L.push(`connectors = ${arr(b.connectors)}`)
  L.push(`skills = ${arr(b.skills)}`)
  L.push(`blueprints = ${arr(b.blueprints)}`)
  L.push(`automations = ${arr(b.automations)}`)
  // Tables last (Rust serializer order) — see the doc comment above.
  if (b.engine?.kind === 'acp') {
    L.push(`[engine]`)
    L.push(`acp = ${q(b.engine.cli)}`)
  } else if (b.engine) {
    L.push(`engine = ${q(b.engine.kind)}`)
  }
  // No binding → no `engine` key at all: an unbound bundle is a draft, and the
  // Rust schema reads the absence as `None` (never as a default engine).
  if (b.model.provider || b.model.model) {
    L.push(`[model]`)
    if (b.model.provider) L.push(`provider = ${q(b.model.provider)}`)
    if (b.model.model) L.push(`model = ${q(b.model.model)}`)
  }
  L.push(`[tools]`)
  L.push(`allow = ${arr(b.tools.allow)}`)
  L.push(`deny = ${arr(b.tools.deny)}`)
  return L.join('\n')
}

/** Export the bundle as agent.toml (share = the file, future K6). */
export function exportBundle(b: AgentBundle): { name: string; content: string } {
  return { name: `${slug(b.name)}.agent.toml`, content: bundleToToml(b) }
}