// The composer's "Search the web" control — one owner for what the desktop can
// honestly say about web search, and one owner for turning the switch on.
//
// What this module is NOT: a search engine. The web search itself is the bound
// agent's own tool (ADR-0005 — the agent owns its native capabilities). This
// file therefore does exactly three things:
//
//   1. read the *existing* search surface — `search_config` / `search_instances`
//      (`src-tauri/src/search_cmds.rs`), which is the same persisted file the
//      live cascade reads (`agentcowork_core::search_config`), so the composer
//      can never describe a different cascade than the one that will run;
//   2. derive one honest summary of it (what is configured, what the upstream
//      feed reports) with an explicit "not reported" for anything the desktop
//      has no evidence for;
//   3. hand the turn a *directive* when the switch is on — the desktop does not
//      run the query, so the composer never claims that it did.
//
// Honesty rules baked in here (I15 / P70.E11):
//
//   * A configured endpoint is **not** a reachable one. The cascade health-gates
//     endpoints at query time (`G8Cascade::is_healthy`) and only the kernel
//     knows the answer, so the composer says "in the cascade — reachability is
//     decided when a query runs", never "online".
//   * A `searx.space` feed row is somebody else's health probe, not ours. It is
//     labelled as the feed's report and never promoted to "this machine can
//     reach it".
//   * There is **no** Tauri command that returns the cascade's per-endpoint
//     outcome or a result count, so those two read "not reported" rather than a
//     plausible number or a spinner that never resolves.

import { invoke } from './tauri'

// ---------------------------------------------------------------------------
// Wire shapes — the exact JSON `src-tauri/src/search_cmds.rs` returns
// ---------------------------------------------------------------------------

/** `search_config()` — the resolved cascade, read without any network access. */
export interface SearchConfigRow {
  usePublic: boolean
  /** The endpoints the live cascade will try, in order (local first). */
  endpoints: string[]
  /** The built-in local SearXNG defaults. A default is not a configuration. */
  localEndpoints: string[]
  /** Public instances the user explicitly opted into (may be empty). */
  publicEndpoints: string[]
}

/** One row of the `searx.space` feed (`search_instances()`). */
export interface SearchInstanceRow {
  url: string
  version?: string | null
  median_seconds?: number | null
  /** Upstream-reported share of successful searches. Not measured locally. */
  search_success_percentage?: number | null
}

/** `search_instances()` — the discovered feed plus where the list came from. */
export interface SearchFeedRow {
  source: 'live' | 'fresh_cache' | 'stale_cache' | null
  count: number
  instances: SearchInstanceRow[]
}

/** `search_instances_apply()` — what the opt-in actually changed. */
export interface SearchApplyRow {
  usePublic: boolean
  endpoints: string[]
  discovered: number
  /** False means the change is persisted and takes effect at the next boot. */
  appliedLive: boolean
}

// ---------------------------------------------------------------------------
// Derived, presentable state
// ---------------------------------------------------------------------------

/**
 * One backend row. `state` is a fact, not a vibe:
 *
 * - `in-cascade`     — this URL is in the endpoint list the cascade will try.
 * - `not-in-cascade` — known to the config (or a built-in default), but the
 *                      cascade will not use it right now.
 *
 * There is deliberately no `reachable` state: only the kernel knows that, and
 * only after a query runs.
 */
export type SearchBackendState = 'in-cascade' | 'not-in-cascade'

export interface SearchBackendRow {
  url: string
  kind: 'local' | 'public'
  state: SearchBackendState
  /** Always an icon + this text; never colour alone. */
  label: string
  /** The evidence behind the label, or why there is none. */
  note: string
  /** Only set for public rows the feed reported on. */
  upstreamSuccessPct?: number
  /** Only set for public rows. Never read as "reachable from here". */
  upstreamTiming?: string
}

export interface SearchSummary {
  /** One line for the composer footer slot. */
  head: string
  /** The sentence that says *why*, or why we cannot say. */
  detail: string
  tone: 'ok' | 'warn' | 'muted'
  /** How many endpoints the cascade will actually try. */
  inCascade: number
  /** How many the upstream feed currently considers eligible. */
  eligible: number
}

const FEED_SOURCE_LABEL: Record<string, string> = {
  live: 'fetched just now',
  fresh_cache: 'cached (fresh)',
  stale_cache: 'cached — last known good',
}

/** Where a feed list came from, in words. An unknown source is not a good one. */
export function feedSourceLabel(source: SearchFeedRow['source']): string {
  if (source === null || source === undefined) return 'source not reported'
  return FEED_SOURCE_LABEL[source] ?? 'source not reported'
}

function normalize(url: string | null | undefined): string {
  return (url ?? '').trim().replace(/\/+$/, '')
}

/** Feed-reported timing, rendered as reported (or explicitly not). */
function upstreamTimingLabel(row: SearchInstanceRow | undefined): string | undefined {
  if (!row) return undefined
  if (row.median_seconds === null || row.median_seconds === undefined) return undefined
  return `${row.median_seconds.toFixed(2)}s median (feed)`
}

export interface SearchBackendProjection {
  rows: SearchBackendRow[]
  /** Feed instances present but not opted into. */
  eligible: number
}

/**
 * Project the search config + the upstream feed into one row per backend.
 *
 * `cfg === null` means the desktop could not read the config at all, which is
 * an *unknown*, not an empty machine: it returns `null` so the caller can say
 * "not reported" rather than "no backends".
 */
export function searchBackends(
  cfg: SearchConfigRow | null | undefined,
  feed: SearchFeedRow | null | undefined,
): SearchBackendProjection | null {
  if (!cfg) return null
  const inCascade = new Set((cfg.endpoints ?? []).map(normalize).filter(Boolean))
  const optedIn = new Set((cfg.publicEndpoints ?? []).map(normalize).filter(Boolean))
  const feedByUrl = new Map<string, SearchInstanceRow>()
  for (const row of feed?.instances ?? []) {
    const key = normalize(row?.url)
    if (key) feedByUrl.set(key, row)
  }
  const rows: SearchBackendRow[] = []
  const seen = new Set<string>()
  let eligible = 0

  const push = (url: string, kind: 'local' | 'public') => {
    const key = normalize(url)
    if (!key || seen.has(key)) return
    seen.add(key)
    const configured = inCascade.has(key)
    const fed = feedByUrl.get(key)
    const pct = fed?.search_success_percentage ?? undefined
    const row: SearchBackendRow = {
      url: key,
      kind,
      state: configured ? 'in-cascade' : 'not-in-cascade',
      label: configured
        ? 'in the cascade'
        : kind === 'public'
          ? 'not used — public instances are off'
          : 'default only — not used',
      note: configured
        ? kind === 'public' && pct !== undefined
          ? `in the cascade · the feed reports ${pct}% upstream success, measured elsewhere`
          : 'in the cascade · reachability is decided when a query runs'
        : kind === 'public'
          ? 'known to the config but not used — public instances are off'
          : 'a built-in default that is not in the cascade',
    }
    if (pct !== undefined) row.upstreamSuccessPct = pct
    const timing = upstreamTimingLabel(fed)
    if (timing) row.upstreamTiming = timing
    rows.push(row)
  }

  for (const url of cfg.localEndpoints ?? []) push(url, 'local')
  for (const url of cfg.publicEndpoints ?? []) push(url, 'public')
  // Feed rows the user has not opted into: the panel counts them, it does not
  // list twenty strangers' servers in the chat bar.
  for (const url of feedByUrl.keys()) {
    if (seen.has(url) || optedIn.has(url)) continue
    seen.add(url)
    eligible += 1
  }
  return { rows, eligible }
}

/** Where the composer should send a user who needs to change the cascade. */
export const SEARCH_SETTINGS_HINT = 'Settings → Search'

/**
 * The one line the composer's reserved footer slot shows. It is a *projection*
 * of {@link searchSummary} plus the switch itself — never a claim that anything
 * was searched.
 */
export interface WebSearchStatus {
  on: boolean
  /** e.g. `2 backends in the cascade` / `no search backend configured`. */
  head: string
  /** The sentence behind it. */
  detail: string
  tone: 'ok' | 'warn' | 'muted'
  inCascade: number
}

/**
 * One honest sentence about the cascade.
 *
 * `error` is a real failure (the command rejected, or the shell is absent) and
 * is stated as one; it is never smoothed into an empty machine.
 */
export function searchSummary(
  cfg: SearchConfigRow | null | undefined,
  feed: SearchFeedRow | null | undefined,
  error?: string | null,
): SearchSummary {
  const projection = searchBackends(cfg, feed)
  const inCascade = projection?.rows.filter((r) => r.state === 'in-cascade').length ?? 0
  const eligible = projection?.eligible ?? 0
  if (error) {
    return {
      head: 'search status not reported',
      detail: error,
      tone: 'warn',
      inCascade: 0,
      eligible: 0,
    }
  }
  if (!projection) {
    return {
      head: 'search status not reported',
      detail: 'The desktop has not reported a search configuration for this machine yet.',
      tone: 'muted',
      inCascade: 0,
      eligible: 0,
    }
  }
  if (inCascade === 0) {
    return {
      head: 'no search backend configured',
      detail: `The cascade has no endpoint to try. Point it at a SearXNG instance in ${SEARCH_SETTINGS_HINT}.`,
      tone: 'warn',
      inCascade: 0,
      eligible,
    }
  }
  const names = projection.rows
    .filter((r) => r.state === 'in-cascade')
    .map((r) => r.url)
    .join(' · ')
  const suffix =
    eligible > 0 ? ` ${eligible} more eligible in the feed — not enabled.` : ''
  return {
    head: `${inCascade} backend${inCascade === 1 ? '' : 's'} in the cascade`,
    detail: `${names}.${suffix}`.trim(),
    tone: 'ok',
    inCascade,
    eligible,
  }
}

// ---------------------------------------------------------------------------
// The switch itself
// ---------------------------------------------------------------------------

/**
 * Web search is OFF by default. Every turn a user sends through a stranger's
 * index (or their own) is their decision, not a product default.
 */
export const WEB_SEARCH_DEFAULT = false

/**
 * The line appended to a turn while the switch is on.
 *
 * This is a **directive to the bound agent**, not a claim about work the
 * desktop did: the desktop has not queried anything. The wording says so, so a
 * transcript reader can never mistake the chip for a completed search.
 */
export const WEB_SEARCH_DIRECTIVE =
  '[web search is on in the composer] Use your own web-search tool for this one and cite what you find. The desktop has not run a query.'

/** `undefined`/empty input still gets the directive (an ask may be all file). */
export function webSearchDirective(on: boolean): string {
  return on ? WEB_SEARCH_DIRECTIVE : ''
}

// ---------------------------------------------------------------------------
// What actually ran — the agent's own tool log
// ---------------------------------------------------------------------------

/** One row of `acp_tool_log` (only the fields this control reads). */
export interface ToolLogEntryLike {
  tsMs?: number
  stopReason?: string
  toolCalls?: { toolCallId?: string; title?: string; kind?: string; status?: string }[]
}

export interface SearchRunReport {
  /** When the turn that ran the search finished. */
  at: number
  calls: number
  ok: number
  failed: number
  /** Still running / no status at all — counted apart from ok and failed. */
  unsettled: number
  /** Always "not reported": no command returns the cascade's per-endpoint run. */
  backendDetail: string
  /** Always "not reported": the tool log carries no result count. */
  resultCount: string
  stopReason: string
}

/** A tool call counts as a search when ACP says `kind: "search"`, or when the
 * call is named for search. Anything else is not claimed as a web search. */
export function isSearchToolCall(call: { title?: string; kind?: string } | undefined): boolean {
  if (!call) return false
  if ((call.kind ?? '').toLowerCase() === 'search') return true
  return /search/i.test(`${call.title ?? ''} ${call.kind ?? ''}`)
}

function statusBucket(status: string | undefined): 'ok' | 'failed' | 'unsettled' {
  const s = (status ?? '').toLowerCase()
  if (s === 'completed' || s === 'success' || s === 'ok') return 'ok'
  if (s === 'failed' || s === 'error' || s === 'cancelled') return 'failed'
  return 'unsettled'
}

/**
 * The newest turn that actually ran a search, summarised from the ACP tool log
 * — the only per-turn record the shell exposes. Returns `null` when no turn ran
 * a search (which is an absence, and the caller says so).
 */
export function lastSearchRun(entries: ToolLogEntryLike[] | null | undefined): SearchRunReport | null {
  if (!Array.isArray(entries)) return null
  for (let i = entries.length - 1; i >= 0; i -= 1) {
    const entry = entries[i]
    const calls = (entry?.toolCalls ?? []).filter(isSearchToolCall)
    if (calls.length === 0) continue
    const buckets = calls.map((c) => statusBucket(c.status))
    return {
      at: entry?.tsMs ?? 0,
      calls: calls.length,
      ok: buckets.filter((b) => b === 'ok').length,
      failed: buckets.filter((b) => b === 'failed').length,
      unsettled: buckets.filter((b) => b === 'unsettled').length,
      backendDetail: 'not reported',
      resultCount: 'not reported',
      stopReason: entry?.stopReason ?? 'not reported',
    }
  }
  return null
}

/** One sentence for the search panel's "last run" line. Never a spinner. */
export function searchRunLine(report: SearchRunReport | null): string {
  if (!report) return 'No turn has run a web search yet.'
  const parts = [`${report.calls} search call${report.calls === 1 ? '' : 's'}`]
  if (report.ok) parts.push(`${report.ok} completed`)
  if (report.failed) parts.push(`${report.failed} failed`)
  if (report.unsettled) parts.push(`${report.unsettled} with no result yet`)
  return `${parts.join(' · ')} · which backend answered: ${report.backendDetail} · results: ${report.resultCount}`
}

// ---------------------------------------------------------------------------
// The existing IPC surface — read only, never re-implemented
// ---------------------------------------------------------------------------

/** `search_config` — no network access, so this is safe to call on open. */
export async function readSearchConfig(): Promise<SearchConfigRow> {
  return invoke<SearchConfigRow>('search_config')
}

/** `search_instances` — `refresh` bypasses the 6h freshness window. */
export async function readSearchInstances(refresh = false): Promise<SearchFeedRow> {
  return invoke<SearchFeedRow>('search_instances', { refresh })
}

/** `search_instances_apply` — the opt-in, owned by Settings → Search. */
export async function applyPublicInstances(usePublic: boolean): Promise<SearchApplyRow> {
  return invoke<SearchApplyRow>('search_instances_apply', { usePublic })
}
