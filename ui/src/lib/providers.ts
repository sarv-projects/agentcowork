// Provider directory (opencode BYOK pattern over the Rust registry).
//
// The shell already vendors all 212 models.dev providers
// (`agentcowork-catalog::provider_seed`, served via `discovery_inventory` as
// provider cards). This module merges those cards with the live vault key set
// (`vault_keys_list`) so the UI can render one honest row per provider:
// configured keys unlock routes, everything else explains where the key goes.
// Secrets never appear here — only the `keyConfigured` fact plus the auth
// *shape* (env-var name) the card already carries.

import { discoveryInventory, type ResourceCard } from './discovery'
import { inTauri, invoke } from './tauri'
import { nativeCall } from './runtime'
import { MODELS, type AgentModel, type ModelProvider } from './agents'

export interface VaultKeyRow {
  provider: string
  keyId: string
  opaqueHandle: string
  status: string
}

export interface ProviderEntry {
  id: string
  name: string
  /** Explicit endpoint ('' = SDK default / user override applies). */
  baseUrl: string
  /** models.dev provider page — full model list, pricing, docs link. */
  docUrl: string
  /** Auth shape, e.g. `api_key_env:OPENAI_API_KEY`, `keyless`, `aws_sdk`. */
  auth: string
  /** First env-var handle from the auth shape, if any. */
  envVar: string | null
  source: string
  status: string
  capabilities: string[]
  capabilitiesVerified: boolean
  /** True when the vault holds ≥1 key for this provider. */
  keyConfigured: boolean
  /** Curated models in this build for the provider (may be empty — the full
   * list lives on the models.dev page linked above). */
  models: AgentModel[]
}

/** `api_key_env:OPENAI_API_KEY` → `OPENAI_API_KEY`, else null. */
export function envVarFromAuth(auth: string): string | null {
  const m = /^api_key_env:(.+)$/.exec(auth.trim())
  return m ? m[1] : null
}

export function modelsDevUrl(providerId: string): string {
  return `https://models.dev/providers/${providerId}`
}

/** Catalog provider id → the static curated-model provider key(s). Unknown
 * providers carry no curated rows (their models live on models.dev). */
const PROVIDER_MODEL_MAP: Record<string, ModelProvider[]> = {
  anthropic: ['anthropic'],
  openai: ['openai'],
  'openai-api': ['openai'],
  xai: ['xai'],
  google: ['google'],
  'google-vertex': ['google'],
  'google-vertex-anthropic': ['google'],
  deepseek: ['deepseek'],
  mistral: ['mistral'],
  meta: ['meta'],
  qwen: ['qwen'],
  alibaba: ['qwen'],
  'alibaba-cn': ['qwen'],
  moonshotai: ['qwen'],
  'moonshotai-cn': ['qwen'],
  local: ['meta', 'qwen'],
  lmstudio: ['meta', 'qwen'],
  'ollama-cloud': ['meta', 'qwen'],
}

export function providerModels(providerId: string): AgentModel[] {
  const keys = PROVIDER_MODEL_MAP[providerId]
  if (!keys) return []
  return MODELS.filter((m) => keys.includes(m.provider))
}

export function toProviderEntry(
  card: ResourceCard,
  keyedProviders: Set<string>,
): ProviderEntry {
  return {
    id: card.id,
    name: card.name || card.id,
    baseUrl: card.baseUrl ?? '',
    docUrl: card.docUrl ?? modelsDevUrl(card.id),
    auth: card.auth,
    envVar: envVarFromAuth(card.auth),
    source: card.source,
    status: card.status,
    capabilities: card.capabilities ?? [],
    capabilitiesVerified: card.capabilitiesVerified ?? false,
    keyConfigured: keyedProviders.has(card.id),
    models: providerModels(card.id),
  }
}

/** Common providers shown when the live inventory is unreachable (preview /
 * offline). Honest by construction: `keyConfigured` is always false and the
 * source reads `preview` — never a claim about this machine. */
const COMMON_FALLBACK_IDS = [
  'anthropic',
  'openai',
  'google',
  'deepseek',
  'xai',
  'mistral',
  'openrouter',
  'nvidia',
  'togetherai',
  'groq',
  'mistral',
  'openai-api',
] as const

const COMMON_FALLBACK_NAMES: Record<string, { name: string; auth: string }> = {
  anthropic: { name: 'Anthropic', auth: 'api_key_env:ANTHROPIC_API_KEY' },
  openai: { name: 'OpenAI', auth: 'api_key_env:OPENAI_API_KEY' },
  google: { name: 'Google', auth: 'api_key_env:GOOGLE_API_KEY' },
  deepseek: { name: 'DeepSeek', auth: 'api_key_env:DEEPSEEK_API_KEY' },
  xai: { name: 'xAI', auth: 'api_key_env:XAI_API_KEY' },
  mistral: { name: 'Mistral', auth: 'api_key_env:MISTRAL_API_KEY' },
  openrouter: { name: 'OpenRouter', auth: 'api_key_env:OPENROUTER_API_KEY' },
  nvidia: { name: 'Nvidia', auth: 'api_key_env:NVIDIA_API_KEY' },
  togetherai: { name: 'Together AI', auth: 'api_key_env:TOGETHER_API_KEY' },
  groq: { name: 'Groq', auth: 'api_key_env:GROQ_API_KEY' },
  'openai-api': { name: 'OpenAI API (Responses/Codex)', auth: 'api_key_env:OPENAI_API_KEY' },
}

function fallbackEntries(): ProviderEntry[] {
  const seen = new Set<string>()
  const out: ProviderEntry[] = []
  for (const id of COMMON_FALLBACK_IDS) {
    if (seen.has(id)) continue
    seen.add(id)
    const meta = COMMON_FALLBACK_NAMES[id] ?? { name: id, auth: 'api_key_env' }
    out.push({
      id,
      name: meta.name,
      baseUrl: '',
      docUrl: modelsDevUrl(id),
      auth: meta.auth,
      envVar: envVarFromAuth(meta.auth),
      source: 'preview',
      status: 'inventoried',
      capabilities: [],
      capabilitiesVerified: false,
      keyConfigured: false,
      models: providerModels(id),
    })
  }
  return out
}

/** P56.2 — the models.dev logo asset for a provider id. */
export function logoUrl(providerId: string): string {
  return `https://models.dev/logos/${providerId}.svg`
}

export interface ProviderDirectory {
  providers: ProviderEntry[]
  /** False in preview/offline fallback — rows are common-provider hints. */
  live: boolean
  /** Providers holding ≥1 vault key (drives the setup gate + route feed). */
  keyedIds: string[]
}

// ---------------------------------------------------------------------------
// P56 — the live models.dev catalog bridge
// ---------------------------------------------------------------------------

/** P56.1 — cheap refresh status (never ships the 4.6 MB snapshot). */
export interface CatalogStatus {
  source: string
  hasSnapshot: boolean
  fetchedAt: number
  stale: boolean
  intervalHours: number
  providers: number
  models: number
  lastDecision: string | null
  lastFailed: boolean
  snapshotBytes?: number | null
}

/** One row of the P56.2 Settings → Providers list. */
export interface CatalogProviderRow {
  id: string
  name: string
  aliases?: string[]
  env?: string[]
  auth?: string
  transport?: string | null
  baseUrl: string
  docUrl?: string | null
  npm?: string | null
  api?: string | null
  logoUrl: string
  source: string
  modelIds?: string[]
  modelCount?: number
  keyConfigured: boolean
  profileSource?: string | null
  format?: string | null
  keyless?: boolean
  sessionHeaders?: boolean
  verifiedAt?: string | null
  /**
   * P44.4 — when a live probe last *observed* this provider.
   *
   * Distinct from `verifiedAt` on purpose: a probe observes that the endpoint
   * answered (and how many models it served) and confirms no hard capability.
   * A provider can be observed and still have no verified capabilities.
   */
  observedAt?: string | null
  /**
   * Did the last observation reach the endpoint? `true` = it answered.
   * `false` = the check failed — including an answered rejection (401/429),
   * where the endpoint is reachable but refused us.
   */
  reachable?: boolean | null
  /** Models the last observation saw (absent = unknown, never a guess). */
  observedModelCount?: number | null
}

/** One row of the P56.7 model table. */
export interface CatalogModel {
  id: string
  name: string
  description?: string
  family?: string
  context?: number
  output?: number
  priceInput?: number
  priceOutput?: number
  cacheRead?: number | null
  cacheWrite?: number | null
  reasoning?: boolean
  toolCall?: boolean
  structuredOutput?: boolean
  attachment?: boolean
  temperature?: boolean
  images?: boolean
  pdf?: boolean
  openWeights?: boolean
  knowledge?: string | null
  releaseDate?: string | null
  lastUpdated?: string | null
  status?: string | null
  free?: boolean
  fromProfile?: boolean
}

export interface CatalogProviderPayload {
  providers: CatalogProviderRow[]
  status: CatalogStatus
  profiles: unknown[]
}

/** True when a row is a real catalog row (not the preview fallback). */
export function isLiveRow(row: { source?: string }): boolean {
  return row.source !== 'preview'
}

/** P56.2 — the merged list. Preview/offline falls back to the common-provider
 * hints with `source: 'preview'` (never a claim about this machine). */
export async function catalogProviders(): Promise<{
  providers: CatalogProviderRow[]
  status: CatalogStatus | null
  live: boolean
}> {
  if (!inTauri()) return { providers: [], status: null, live: false }
  try {
    const payload = await nativeCall('catalog providers', () =>
      invoke<CatalogProviderPayload>('catalog_providers', {}),
    )
    return { providers: payload.providers ?? [], status: payload.status ?? null, live: true }
  } catch {
    return { providers: [], status: null, live: false }
  }
}

/** P56.7 — the full model table for one provider (opencode-free = the keyless
 * free subset). `live: false` means "no snapshot stored yet — refresh". */
export async function catalogProviderModels(provider: string): Promise<{
  models: CatalogModel[]
  profileModels: CatalogModel[]
  count: number
  live: boolean
  freeSubset: string[] | null
}> {
  if (!inTauri()) {
    return { models: [], profileModels: [], count: 0, live: false, freeSubset: null }
  }
  const r = await invoke<{
    models?: CatalogModel[]
    profileModels?: CatalogModel[]
    count?: number
    live?: boolean
    freeSubset?: string[] | null
  }>('catalog_provider_models', { provider })
  return {
    models: r.models ?? [],
    profileModels: r.profileModels ?? [],
    count: r.count ?? 0,
    live: r.live ?? false,
    freeSubset: r.freeSubset ?? null,
  }
}

/** P56.1 — run the refresh job now (`force` skips the staleness check). */
export async function catalogRefresh(force = true): Promise<{
  accepted: boolean
  persisted: boolean
  summary: string
  status: CatalogStatus
}> {
  return nativeCall('catalog refresh', () =>
    invoke('catalog_refresh', { force }),
  )
}

/** P56.1 — the 1–24h cadence (the shell clamps and reports the clamp). */
export async function catalogSetInterval(hours: number): Promise<{
  ok: boolean
  intervalHours: number
  clamped: boolean
}> {
  return nativeCall('catalog interval', () => invoke('catalog_set_interval', { hours }))
}

/** P56.3 — the activate screen's `MetadataOnly` probe. Read-only: a failed
 * probe never leaves a half-configured provider behind. */
export async function providerProbe(
  provider: string,
  key?: string,
): Promise<{ ok: boolean; status: number; message: string; models: number; url?: string }> {
  return nativeCall('provider probe', () =>
    invoke('provider_probe', { provider, key: key ?? '' }),
  )
}

/** P55.6/P56.4 — persist the endpoint half of a provider (never the secret).,
 * plus the P56.3 verification stamp on a successful probe tick. */
export async function providerProfileUpsert(profile: {
  id: string
  name?: string
  format?: string
  baseUrl?: string
  keyRequired?: boolean
  headers?: Record<string, string>
  body?: unknown
  temperature?: number | null
  models?: Array<{ id: string; name?: string; context?: number; output?: number }>
  verifiedAt?: string | null
  verifiedModels?: number
  sessionHeaders?: boolean
}): Promise<unknown> {
  return nativeCall('provider profile save', () =>
    invoke('provider_profile_upsert', { profile }),
  )
}

export async function providerProfileRemove(id: string): Promise<unknown> {
  return nativeCall('provider profile remove', () =>
    invoke('provider_profile_remove', { id }),
  )
}

/** P56.5 — the shipped NVIDIA NIM overlay (`http://localhost:8000/v1`). */
export async function providerNimProfile(baseUrl?: string): Promise<unknown> {
  return nativeCall('nvidia nim profile', () =>
    invoke('provider_nim_profile', { baseUrl: baseUrl ?? null }),
  )
}

/** Format a per-1M price for the model table (`—` when the catalog omitted it). */
export function formatPerM(value: number | null | undefined, free?: boolean): string {
  if (free) return 'free'
  if (value === null || value === undefined || Number.isNaN(value)) return '—'
  if (value === 0) return '$0'
  return `$${value < 1 ? value.toFixed(3) : value.toFixed(2)}`
}

/** P56.2 — the preview/no-shell rows: the same common providers the fallback
 * directory carries, mapped to the catalog row shape with `source: 'preview'`.
 * Honest by construction — no key fact, no claim about this machine. */
export function previewCatalogRows(): CatalogProviderRow[] {
  return fallbackEntries().map((e) => ({
    id: e.id,
    name: e.name,
    aliases: [],
    env: e.envVar ? [e.envVar] : [],
    auth: e.auth,
    transport: null,
    baseUrl: e.baseUrl,
    docUrl: e.docUrl,
    npm: null,
    api: null,
    logoUrl: logoUrl(e.id),
    source: 'preview',
    modelIds: e.models.map((m) => m.id),
    modelCount: e.models.length,
    keyConfigured: false,
    profileSource: null,
    format: null,
    keyless: e.auth === 'keyless',
    sessionHeaders: false,
    verifiedAt: null,
  }))
}

/** The fields one row searches over, kept separate so a query never matches
 * by stitching characters across two unrelated fields (the reason `nim` must
 * not hit `openai` + `gpt-5-mini`). */
export function providerFields(row: CatalogProviderRow): string[] {
  return [
    row.id,
    row.name,
    ...(row.aliases ?? []),
    ...(row.env ?? []),
    row.auth ?? '',
    row.npm ?? '',
    row.profileSource ?? '',
    ...(row.modelIds ?? []),
  ].map((s) => s.toLowerCase())
}

/** P56.2 — subsequence ("all characters") match: every char of `q` appears in
 * `haystack` in order, so `ocg` finds `OpenCode Go` and `gpt5` finds `gpt-5`.
 * An empty query matches everything. */
export function subsequenceMatch(haystack: string, q: string): boolean {
  const needle = q.trim().toLowerCase()
  if (!needle) return true
  let i = 0
  for (const ch of haystack) {
    if (ch === needle[i]) i += 1
    if (i === needle.length) return true
  }
  return false
}

export function providerMatches(row: CatalogProviderRow, q: string): boolean {
  const needle = q.trim().toLowerCase()
  if (!needle) return true
  // A plain substring is the strongest signal; the ordered-characters pass is
  // the fallback that makes `ocf` find `opencode-free`.
  return providerFields(row).some(
    (f) => f.includes(needle) || subsequenceMatch(f, needle),
  )
}

/** P56.2 — filter the catalog rows, keyed providers first, then name order. */
export function searchProviders(
  rows: CatalogProviderRow[],
  q: string,
): CatalogProviderRow[] {
  const hit = rows.filter((r) => providerMatches(r, q))
  return hit.sort((a, b) => {
    if (a.keyConfigured !== b.keyConfigured) return a.keyConfigured ? -1 : 1
    return a.name.localeCompare(b.name)
  })
}

/** P56.2 — split the catalog into "yours" (a key/profile exists) and the
 * rest, so the Settings list reads top-down like the user's own setup. */
export function configuredProviders(rows: CatalogProviderRow[]): CatalogProviderRow[] {
  return rows.filter((r) => r.keyConfigured || r.profileSource)
}

// ---------------------------------------------------------------------------
// P56.4 — the custom inference form's pure parsers
// ---------------------------------------------------------------------------

/** Parse a `Header-Name: value` block (one per line, `#` comments). Blank
 * lines are skipped; a line without `:` is reported rather than silently
 * dropped, so a typo never becomes a hidden request header. */
export function parseHeaderLines(
  text: string,
): { headers: Record<string, string>; errors: string[] } {
  const headers: Record<string, string> = {}
  const errors: string[] = []
  text.split('\n').forEach((raw, i) => {
    const line = raw.trim()
    if (!line || line.startsWith('#')) return
    const idx = line.indexOf(':')
    if (idx <= 0) {
      errors.push(`line ${i + 1}: expected \`Name: value\``)
      return
    }
    const name = line.slice(0, idx).trim()
    const value = line.slice(idx + 1).trim()
    if (!name || !value) {
      errors.push(`line ${i + 1}: empty name or value`)
      return
    }
    headers[name] = value
  })
  return { headers, errors }
}

/** Parse the models table lines: `id | name | context | output`. Only the id
 * is required; name defaults to the id, and `free` is honoured as a 5th
 * column. Duplicate ids keep the last row (a table, not a list). */
export function parseProfileModels(
  text: string,
): { models: Array<{ id: string; name: string; context: number; output: number; free: boolean }>; errors: string[] } {
  const byId = new Map<string, { id: string; name: string; context: number; output: number; free: boolean }>()
  const errors: string[] = []
  text.split('\n').forEach((raw, i) => {
    const line = raw.trim()
    if (!line || line.startsWith('#')) return
    const [idRaw, nameRaw, ctxRaw, outRaw, freeRaw] = line.split('|').map((s) => s.trim())
    const id = idRaw ?? ''
    if (!id) {
      errors.push(`line ${i + 1}: model id required`)
      return
    }
    const int = (v: string | undefined) => {
      const n = Number.parseInt(v ?? '', 10)
      return Number.isFinite(n) && n > 0 ? n : 0
    }
    byId.set(id, {
      id,
      name: nameRaw || id,
      context: int(ctxRaw),
      output: int(outRaw),
      free: (freeRaw ?? '').toLowerCase() === 'free' || (freeRaw ?? '').toLowerCase() === 'true',
    })
  })
  return { models: [...byId.values()], errors }
}

/** Parse the optional JSON body-merge box. Empty text is `{}` (no merge). */
export function parseJsonObject(
  text: string,
): { ok: true; value: Record<string, unknown> } | { ok: false; error: string } {
  const t = text.trim()
  if (!t) return { ok: true, value: {} }
  try {
    const v = JSON.parse(t)
    if (v === null || typeof v !== 'object' || Array.isArray(v)) {
      return { ok: false, error: 'body merge must be a JSON object' }
    }
    return { ok: true, value: v as Record<string, unknown> }
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : 'invalid JSON' }
  }
}

/** P56.4 — the profile the custom inference form edits (never a secret). */
export interface CustomProfilePayload {
  id: string
  name: string
  format: string
  baseUrl: string
  keyRequired: boolean
  headers: Record<string, string>
  body: Record<string, unknown>
  temperature: number | null
  models: Array<{ id: string; name: string; context: number; output: number; free: boolean }>
}

/** P56.4 — the OpenCode-shaped form → the profile payload (no secrets). */
export function toProfilePayload(form: {
  id: string
  name: string
  format: string
  baseUrl: string
  keyRequired: boolean
  headers: string
  body: string
  temperature: string
  models: string
}): { profile: CustomProfilePayload; errors: string[] } {
  const errors: string[] = []
  const name = form.name.trim()
  const id = (form.id.trim() || name.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-')).replace(/^-+|-+$/g, '')
  if (!name) errors.push('name required')
  if (!id) errors.push('id required')
  const baseUrl = form.baseUrl.trim()
  if (!baseUrl) errors.push('base URL required')
  else if (!/^https?:\/\//.test(baseUrl)) errors.push('base URL must start with http:// or https://')
  else if (/(chat\/completions|\/completions|\/messages|\/responses)$/.test(baseUrl.replace(/\/+$/, '')))
    errors.push('base URL must stop at the version root (remove the request path)')

  const parsedHeaders = parseHeaderLines(form.headers)
  errors.push(...parsedHeaders.errors)
  const parsedModels = parseProfileModels(form.models)
  errors.push(...parsedModels.errors)
  const parsedBody = parseJsonObject(form.body)
  if (!parsedBody.ok) errors.push(parsedBody.error)
  const tempRaw = form.temperature.trim()
  const temperature = tempRaw === '' ? null : Number(tempRaw)
  if (temperature !== null && (!Number.isFinite(temperature) || temperature < 0 || temperature > 2))
    errors.push('temperature must be between 0 and 2')

  return {
    profile: {
      id,
      name,
      format: form.format,
      baseUrl,
      keyRequired: form.keyRequired,
      headers: parsedHeaders.headers,
      body: parsedBody.ok ? parsedBody.value : {},
      temperature,
      models: parsedModels.models,
    },
    errors,
  }
}

export async function loadProviderDirectory(): Promise<ProviderDirectory> {
  if (!inTauri()) return { providers: fallbackEntries(), live: false, keyedIds: [] }
  try {
    const [inv, keyed] = await Promise.all([
      discoveryInventory(),
      invoke<{ keys?: VaultKeyRow[] }>('vault_keys_list', {}).catch((): {
        keys?: VaultKeyRow[]
      } => ({ keys: [] })),
    ])
    const cards = (inv.cards ?? []).filter((c) => c.kind === 'provider')
    const keyedSet = new Set((keyed.keys ?? []).map((k) => k.provider))
    const providers = cards
      .map((c) => toProviderEntry(c, keyedSet))
      .sort((a, b) => a.name.localeCompare(b.name))
    return { providers, live: true, keyedIds: [...keyedSet] }
  } catch {
    return { providers: fallbackEntries(), live: false, keyedIds: [] }
  }
}
