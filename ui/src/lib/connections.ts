// P65.3 — ConnectionView: one honest connection state over the existing
// Connector / McpServerRow / OAuthAccount / StoreEntry shapes.
//
// No new backend commands. Every adapter is a pure mapping from facts the
// shell already reports; a failed probe renders fail-closed (Disconnected or
// Degraded, never Connected).
//
// NAMING — why this is `ConnectionView` and not `ConnectionRecord`:
// `ConnectionRecord` is the **wire** read model owned by ARCH/17 §17.12.2 and
// implemented in `src-tauri/src/settings_cmds.rs`, whose TypeScript mirror is
// `@/lib/settings`. This module is the **display projection** over the domain
// libs, so it is deliberately named `ConnectionView` to keep one name meaning
// one shape. Two incompatible types sharing the name `ConnectionRecord` is
// exactly the contract drift `agentcowork-types` exists to prevent on the Rust
// side. Use `ConnectionView` for rendering; use `@/lib/settings`'s
// `ConnectionRecord` when reading the authoritative wire state.
//
// States:
//   Discovered   — catalog knows it, no live session (store entry not yet
//                  connected, MCP row seen before but not running).
//   Installed    — on this machine but not a live socket (built-in native
//                  catalog, package-managed runtime).
//   Connected    — a live session/token proves it (vault OAuth row, MCP row
//                  with tools advertised, store probe passing).
//   Disconnected — explicitly not connected (never attempted, detached, or a
//                  probe that failed closed).
//   Degraded     — reachable but not healthy (handshake with zero tools,
//                  expired token, error status).

import type { Connector } from './store'
import type { McpServerRow } from './mcp'
import type { OAuthAccount } from './oauth'
import type { StoreEntry } from './mcp'

// NOTE: this display vocabulary is PascalCase and lacks `revoked`, while the
// §17.12.2 wire vocabulary is snake_case and includes it. Which one Settings
// should standardise on is an open decision recorded in CURRENT_RUN.md — the
// plan prose says `Discovered|Installed|Connected|Disconnected|Degraded`, the
// normative contract says `discovered|installed|connected|disconnected|
// degraded|revoked`. Not silently reconciled here.
export type ConnectionViewState =
  | 'Discovered'
  | 'Installed'
  | 'Connected'
  | 'Disconnected'
  | 'Degraded'

export interface ConnectionView {
  id: string
  name: string
  kind: 'connector' | 'mcp-server' | 'oauth-account' | 'store-entry'
  state: ConnectionViewState
  /** Plain-language reason, safe to render verbatim. */
  detail: string
  /** Where the fact came from (vault, mcp_servers, store probe, preview). */
  source: string
}

export function connectionTone(state: ConnectionViewState): string {
  switch (state) {
    case 'Connected':
      return 'bg-emerald-500/15 text-emerald-300'
    case 'Installed':
      return 'bg-sky-500/15 text-sky-300'
    case 'Discovered':
      return 'bg-zinc-500/15 text-zinc-300'
    case 'Degraded':
      return 'bg-warning/15 text-warning'
    case 'Disconnected':
    default:
      return 'bg-zinc-500/10 text-muted-foreground'
  }
}

/** Accessible label — status is never color-alone. */
export function connectionLabel(r: ConnectionView): string {
  return `${r.name}: ${r.state} — ${r.detail}`
}

export function connectorToRecord(c: Connector): ConnectionView {
  if (c.status === 'connected') {
    return {
      id: c.id,
      name: c.name,
      kind: 'connector',
      state: 'Connected',
      detail: 'vault holds a live credential',
      source: 'vault',
    }
  }
  if (c.status === 'error') {
    return {
      id: c.id,
      name: c.name,
      kind: 'connector',
      state: 'Degraded',
      detail: 'last check reported an error — not usable until it passes',
      source: 'vault',
    }
  }
  return {
    id: c.id,
    name: c.name,
    kind: 'connector',
    state: 'Disconnected',
    detail: 'no credential in the vault',
    source: 'vault',
  }
}

export function mcpRowToRecord(row: McpServerRow): ConnectionView {
  const connected = row.status === 'connected'
  if (!connected) {
    // Seen before (tools on record) but not running now.
    if (row.toolNames.length > 0 || row.tools > 0) {
      return {
        id: row.name,
        name: row.name,
        kind: 'mcp-server',
        state: 'Discovered',
        detail: 'seen before, not running now',
        source: 'mcp_servers',
      }
    }
    return {
      id: row.name,
      name: row.name,
      kind: 'mcp-server',
      state: 'Disconnected',
      detail: 'not attached',
      source: 'mcp_servers',
    }
  }
  // Connected cases.
  if (row.transport === 'native') {
    return {
      id: row.name,
      name: row.name,
      kind: 'mcp-server',
      state: 'Installed',
      detail: 'built-in catalog — no external socket',
      source: 'mcp_servers',
    }
  }
  if (row.toolNames.length === 0) {
    return {
      id: row.name,
      name: row.name,
      kind: 'mcp-server',
      state: 'Degraded',
      detail: 'connected but no handshake on record — re-attach to discover tools',
      source: 'mcp_servers',
    }
  }
  return {
    id: row.name,
    name: row.name,
    kind: 'mcp-server',
    state: 'Connected',
    detail: `${row.toolNames.length} tools advertised`,
    source: 'mcp_servers',
  }
}

export function oauthToRecord(a: OAuthAccount): ConnectionView {
  const expired = typeof a.expiresAt === 'number' && a.expiresAt > 0 && a.expiresAt < Date.now()
  if (expired) {
    return {
      id: `${a.provider}:${a.accountId}`,
      name: a.provider,
      kind: 'oauth-account',
      state: 'Degraded',
      detail: 'token expired — reconnect to refresh',
      source: 'vault',
    }
  }
  return {
    id: `${a.provider}:${a.accountId}`,
    name: a.provider,
    kind: 'oauth-account',
    state: 'Connected',
    detail: a.email ?? a.accountId,
    source: 'vault',
  }
}

export function storeEntryToRecord(
  e: StoreEntry,
  connected: boolean,
  probeFailed = false,
): ConnectionView {
  // Fail-closed: a probe failure never renders Connected.
  if (probeFailed && !connected) {
    return {
      id: e.id,
      name: e.name,
      kind: 'store-entry',
      state: 'Disconnected',
      detail: 'status check failed — shown as not connected',
      source: 'store probe',
    }
  }
  if (connected) {
    return {
      id: e.id,
      name: e.name,
      kind: 'store-entry',
      state: 'Connected',
      detail: 'vault token verified',
      source: 'store probe',
    }
  }
  // Curated catalog rows that were never connected are Discovered, not
  // Disconnected — the catalog knows them, this machine has no session.
  return {
    id: e.id,
    name: e.name,
    kind: 'store-entry',
    state: 'Discovered',
    detail: 'in the curated store — not connected yet',
    source: 'store catalog',
  }
}
