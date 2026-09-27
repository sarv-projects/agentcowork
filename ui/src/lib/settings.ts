// P65 — Settings Control Center client (ARCH/17 §17.12).
//
// Typed wrappers over the `settings_*` Tauri commands. These are the *unified*
// read models: each inventory row is assembled from the subsystem that owns it
// (catalog/vault, acp/agent-backend, mcp/oauth, scheduler, skill store), and
// every mutation goes through the one funnel that answers the §17.12.3 envelope
// `{ appliedLive, restartRequired, state, health, lastError? }`.
//
import type { AuthMode } from './acp'

// Two rules this module keeps:
// - **Keys stay by reference.** `authRef` is an opaque vault handle
//   (`vault:oauth:<provider>:<id>`); no raw secret ever crosses this boundary.
// - **Preview invents nothing.** With no shell attached we return empty
//   inventories, so a panel renders its honest empty state instead of a
//   fabricated green row. There is no demo data here on purpose.

import { invoke } from './tauri'
import { bridgeCall } from './runtime'

// ───────────────────────────────────────────────────────────────────────────
// Canonical read models (§17.12.2)
// ───────────────────────────────────────────────────────────────────────────

export type SettingsKind = 'provider' | 'agent' | 'connection' | 'schedule' | 'extension'

export type SettingsState =
  | 'discovered'
  | 'installed'
  | 'configured'
  | 'connected'
  | 'disconnected'
  | 'degraded'
  | 'disabled'
  | 'unavailable'

export type SettingsHealth = 'ready' | 'permission_required' | 'missing' | 'failed' | 'unknown'

/** The shared row shape every Settings inventory reuses. */
export interface SettingsReadModel {
  id: string
  kind: SettingsKind
  state: SettingsState
  health: SettingsHealth
  lastError?: string
  configHash: string
  appliedLive: boolean
  restartRequired: boolean
}

/**
 * Discriminated runtime location (§17.12.4, Windows-first). A discovery fact,
 * never a display string: a catalog row is not occupancy, and
 * `installed` / `discovered` / `launchable` stay three distinct facts.
 */
export type RuntimeLocation =
  | { kind: 'managed'; executable: string; installRoot: string; version: string }
  | { kind: 'windows_path'; executable: string; source: string }
  | { kind: 'windows_registry'; executable: string; source: string }
  | { kind: 'user_path'; executable: string; source: string }
  | { kind: 'package_manager'; manager: string; package: string; version?: string }
  | { kind: 'wsl'; distro: string; linuxPath: string; windowsLauncher: string }
  | { kind: 'unavailable'; reason: string }

/** The P63 compatibility binding, as reported to the UI: non-secret NAMES only. */
export interface BackendBindingView {
  providerId: string
  /** Credential-free launch inputs, such as model or base URL. */
  injectedEnvNames: string[]
  unexpressed: string[]
  /** Always `false` — AgentCowork never writes an external agent's own config. */
  writesToAgentConfig: boolean
  /**
   * @deprecated Historical compatibility only. It is not authentication or
   * readiness evidence and never means a host-vault key was delegated.
   */
  keyPresent: boolean
  refusal?: string
}

export interface AgentConfigOptionView {
  id: string
  name: string
  value?: unknown
  options?: string[]
}

/** One session capability-loadout row — a loadout, not a tool dump. */
export interface SessionLoadoutRow {
  capabilityId: string
  source: string
  nativeOrShared: 'native' | 'shared'
  enabled: boolean
  health: string
  scope: string
  requiresApproval: boolean
  /** Fixed: loadout changes apply to the next turn/run. */
  appliesFrom: string
}

/** P71.9g — the **canonical** `AgentProtocol` union, mirroring
 * `agentcowork_types::AgentProtocol` (`Acp` · `ModelOnly`). The previous spelling
 * (`'inbuilt' | 'acp' | 'mcp'`) was a hand-maintained duplicate that agreed
 * with neither serializer and named a built-in engine v1 does not ship
 * (`ADR-0005`); `model_only` is the variant the shell actually emits
 * (`settings_cmds::protocol_for`, `agent_cmds::entry_json`). One canonical
 * spelling per variant (P69.C11 / I4). */
export type AgentProtocol = 'acp' | 'model_only'
/** P69.C11 — the auth-mode contract has exactly one declaration: the
 * canonical `AuthMode` union in `./acp` (a projection of
 * `agentcowork_types::AuthMode`). This name is kept for call sites. */
export type AgentAuthMode = AuthMode
export type ModelOwner = 'native' | 'agent' | 'managed'
export type AgentReadiness =
  | 'ready'
  | 'sign_in_required'
  | 'api_key_required'
  | 'local_cli'
  | 'not_installed'
  | 'unavailable'
  | 'health_failed'

export interface AgentSettings {
  agentId: string
  installed: boolean
  protocol: AgentProtocol
  authMode: AgentAuthMode
  nativeCapabilities: string[]
  sharedCapabilities: string[]
  /**
   * Shell-reported compatibility label. The UI projects `managed` down to
   * `agent` unless a non-empty, credential-free host launch override exists.
   */
  modelOwner: ModelOwner
  backendBinding?: BackendBindingView
  configOptions: AgentConfigOptionView[]
  /** Live agent/ACP state; host-vault presence is not a readiness input. */
  readiness: AgentReadiness
  location: RuntimeLocation
  sessionLoadout: SessionLoadoutRow[]
}

export type ConnectionKind =
  | 'remote_mcp'
  | 'oauth_connector'
  | 'native_adapter'
  | 'message_channel'

export type ConnectionState =
  | 'discovered'
  | 'installed'
  | 'connected'
  | 'disconnected'
  | 'degraded'
  | 'revoked'

export interface ConnectionRecord {
  id: string
  kind: ConnectionKind
  transport: 'stdio' | 'http' | 'oauth' | 'api_key' | 'browser_session' | 'native'
  scopes: string[]
  enabledConsumers: string[]
  state: ConnectionState
  health: string
  /** Opaque vault reference only; never a secret. */
  authRef?: string
  configHash: string
}

export type ScheduleTrigger = 'cron' | 'interval' | 'event' | 'webhook'
/** P71.3d — trigger plane: firing-paused or enabled; run status lives in Work. */
export type ScheduleState = 'armed' | 'paused' | 'disabled'

export interface ScheduleSettings {
  id: string
  /** Human label owned by the scheduler job. Display only — `id` is identity. */
  name: string
  trigger: ScheduleTrigger
  /** The session/Work this schedule reawakens (frozen manifest per run). */
  target: string
  chiefAgentId: string
  capabilityScope: string[]
  autonomy: string
  budget: string
  networkPolicy: string
  timezone: string
  enabled: boolean
  configHash: string
  nextRunAt?: number
  /** Last trigger firing (occurrence record — not a run outcome, `I3`). */
  lastFiredAt?: number
  state: ScheduleState
}

export type ExtensionKind = 'skill' | 'plugin' | 'mcp' | 'acp' | 'hook' | 'tool'

export interface InstalledExtension {
  id: string
  kind: ExtensionKind
  version: string
  abiVersion?: number
  provenance: string
  digest: string
  signatureStatus: string
  capabilitiesRequested: string[]
  capabilitiesGranted: string[]
  boundAgents: string[]
  activation: 'lazy' | 'active' | 'disabled'
  health: string
}

/**
 * The §17.12.3 mutation envelope. Field order matches the contract. The UI
 * must render `state`/`health` from this reread — never its own optimistic
 * guess — and surface `lastError` when the authoritative reread disagrees.
 */
export interface SettingsMutationResult {
  appliedLive: boolean
  restartRequired: boolean
  state: string
  health: string
  lastError?: string
}

// ───────────────────────────────────────────────────────────────────────────
// Envelopes (per-command response shapes)
// ───────────────────────────────────────────────────────────────────────────

/** One provider row: the catalog row plus the P65 state/health/hash fields. */
export type ProviderSettingsRow = SettingsReadModel & {
  id: string
  name?: string
  keyConfigured?: boolean
  keyless?: boolean
  reachable?: boolean
  observedAt?: number
  observedModelCount?: number
  verifiedAt?: string
  [key: string]: unknown
}

export interface ProviderGroups {
  configured: string[]
  popular: string[]
  all: string[]
}

export interface ProvidersEnvelope {
  providers: ProviderSettingsRow[]
  groups: ProviderGroups
  defaultModel: { provider?: string; model?: string }
  status: unknown
}

export interface DefaultModelEnvelope {
  defaultModel: { provider?: string; model?: string }
}

export interface AgentsEnvelope {
  agents: AgentSettings[]
}

export interface ConnectionsEnvelope {
  connections: ConnectionRecord[]
}

export interface SchedulesEnvelope {
  schedules: ScheduleSettings[]
}

export interface ExtensionsEnvelope {
  extensions: InstalledExtension[]
}

/** P65.3 — group the unified inventory without a second registry. */
export function groupConnectionRecords(rows: ConnectionRecord[]): Record<ConnectionState, ConnectionRecord[]> {
  const empty = (): ConnectionRecord[] => []
  const out: Record<ConnectionState, ConnectionRecord[]> = {
    discovered: empty(),
    installed: empty(),
    connected: empty(),
    disconnected: empty(),
    degraded: empty(),
    revoked: empty(),
  }
  for (const r of rows) {
    const bucket = out[r.state] ?? out.disconnected
    bucket.push(r)
  }
  return out
}

/** P65.6 — a failed envelope never reads as live-applied success. */
export function mutationLooksLive(r: SettingsMutationResult): boolean {
  return r.appliedLive === true && !r.lastError
}

/**
 * P65.6 — injected-failure rollback. If the envelope is not live-applied,
 * keep the previous snapshot; never keep the optimistic next state.
 */
export function chooseAfterMutation<T>(
  previous: T,
  optimistic: T,
  envelope: SettingsMutationResult,
): T {
  return mutationLooksLive(envelope) ? optimistic : previous
}

/** P65.7 — AgentCowork never writes an external agent's own config file. */
export function assertNoAgentConfigWrite(binding: BackendBindingView): void {
  if (binding.writesToAgentConfig) {
    throw new Error('AgentCowork never writes an external agent config')
  }
}

/** True only for an actual non-empty, credential-free host launch override. */
export function hasHostLaunchOverride(
  binding: Pick<BackendBindingView, 'injectedEnvNames'> | undefined,
): boolean {
  return (binding?.injectedEnvNames ?? []).some((name) => name.trim().length > 0)
}

/**
 * Defensive UI projection for older shell replies: a `managed` label without
 * a concrete host launch input is not host-managed. Authentication and the
 * model remain agent-owned.
 */
export function projectModelOwner(
  row: Pick<AgentSettings, 'modelOwner' | 'backendBinding'>,
): ModelOwner {
  if (row.modelOwner === 'managed' && !hasHostLaunchOverride(row.backendBinding)) {
    return 'agent'
  }
  return row.modelOwner
}

/** Authentication always belongs to the agent; the method may still be unknown. */
export function agentAuthenticationLabel(authMode: AgentAuthMode | undefined): string {
  switch (authMode) {
    case 'subscription':
      return 'agent-owned · subscription'
    case 'api_key':
      return 'agent-owned · API key'
    case 'local':
      return 'agent-owned · local'
    case 'keyless':
      return 'agent-owned · keyless'
    default:
      return 'agent-owned · method not verified'
  }
}

/**
 * P65.2 — an external agent's native model/tools stay its own. Shared
 * cowork grants are additional; they must not replace native occupancy
 * or flip the effective `modelOwner` to AgentCowork-managed.
 */
export function nativeSurfaceNotReplaced(
  row: Pick<AgentSettings, 'protocol' | 'modelOwner' | 'backendBinding'> & {
    /**
     * Absent on a partial or legacy row (e.g. an older shell reply, or a
     * capability probe that failed). Treated as empty, never a crash.
     */
    nativeCapabilities?: string[]
    sharedCapabilities?: string[]
  },
): boolean {
  // Only an **external agent** has a native surface another grant could
  // replace. A `model_only` bundle has no external agent (its model is pinned
  // by the user in the bundle), and the retired `'mcp'` spelling was never a
  // value any serializer emitted (P71.9g).
  if (row.protocol === 'acp') {
    const modelOwner = projectModelOwner(row)
    if (modelOwner === 'managed' || modelOwner === 'native') return false
    if (row.backendBinding?.writesToAgentConfig) return false
  }
  const native = new Set(row.nativeCapabilities ?? [])
  for (const cap of row.sharedCapabilities ?? []) {
    if (native.has(cap)) return false
  }
  return true
}

/** P65.7 — OAuth revoke / extension install never write another agent's config. */
export const SETTINGS_IPC_MATRIX: ReadonlyArray<{
  family: 'providers' | 'agents' | 'connections' | 'schedules' | 'extensions'
  command: string
  writesAgentConfig: false
}> = [
  { family: 'providers', command: 'settings_default_model_set', writesAgentConfig: false },
  { family: 'agents', command: 'settings_agent_get', writesAgentConfig: false },
  { family: 'connections', command: 'oauth_revoke', writesAgentConfig: false },
  { family: 'schedules', command: 'settings_schedule_set_enabled', writesAgentConfig: false },
  { family: 'extensions', command: 'skills_install', writesAgentConfig: false },
]

export function assertSettingsCommandDoesNotWriteAgentConfig(command: string): void {
  const row = SETTINGS_IPC_MATRIX.find((r) => r.command === command)
  if (!row) {
    throw new Error(`settings IPC ${command} is not on the ownership matrix`)
  }
  if (row.writesAgentConfig) {
    throw new Error(`settings IPC ${command} must not write an agent config`)
  }
}

// ───────────────────────────────────────────────────────────────────────────
// Commands
// ───────────────────────────────────────────────────────────────────────────

/** P65.1 — Configured / Popular / All provider inventory + live default model. */
export function settingsProvidersList(): Promise<ProvidersEnvelope> {
  return bridgeCall<ProvidersEnvelope>({
    operation: 'settings_providers_list',
    live: () => invoke<ProvidersEnvelope>('settings_providers_list'),
    preview: () => ({
      providers: [],
      groups: { configured: [], popular: [], all: [] },
      defaultModel: {},
      status: null,
    }),
  })
}

export function settingsDefaultModelGet(): Promise<DefaultModelEnvelope> {
  return bridgeCall<DefaultModelEnvelope>({
    operation: 'settings_default_model_get',
    live: () => invoke<DefaultModelEnvelope>('settings_default_model_get'),
    preview: () => ({ defaultModel: {} }),
  })
}

/**
 * P65.1 — persist the default model through the one mutation funnel.
 *
 * Refused by the shell when an external agent owns the model surface
 * (`modelOwner === 'agent'`): the native catalog must not override another
 * agent's own model. The refusal arrives as the envelope's `lastError`.
 */
export function settingsDefaultModelSet(
  provider: string,
  model: string,
): Promise<SettingsMutationResult> {
  return invoke<SettingsMutationResult>('settings_default_model_set', { provider, model })
}

/** P65.2 — every agent's two-plane settings (native vs shared cowork). */
export function settingsAgentsList(): Promise<AgentsEnvelope> {
  return bridgeCall<AgentsEnvelope>({
    operation: 'settings_agents_list',
    live: () => invoke<AgentsEnvelope>('settings_agents_list'),
    preview: () => ({ agents: [] }),
  })
}

/** P65.2 — one agent's detail (auth mode, readiness, runtime location). */
export function settingsAgentGet(agentId: string): Promise<AgentSettings> {
  return invoke<AgentSettings>('settings_agent_get', { agentId })
}

/** P65.2 / §17.12.5 — the session capability loadout for one agent. */
export function settingsAgentLoadout(agentId: string): Promise<SessionLoadoutRow[]> {
  return invoke<{ loadout?: SessionLoadoutRow[] }>('settings_agent_loadout', { agentId }).then(
    (r) => r.loadout ?? [],
  )
}

/** P65.3 — MCP servers, OAuth connectors and local adapters, unified. */
export function settingsConnectionsList(): Promise<ConnectionsEnvelope> {
  return bridgeCall<ConnectionsEnvelope>({
    operation: 'settings_connections_list',
    live: () => invoke<ConnectionsEnvelope>('settings_connections_list'),
    preview: () => ({ connections: [] }),
  })
}

/** P65.4 — schedules as the Settings contract sees them. */
export function settingsSchedulesList(): Promise<SchedulesEnvelope> {
  return bridgeCall<SchedulesEnvelope>({
    operation: 'settings_schedules_list',
    live: () => invoke<SchedulesEnvelope>('settings_schedules_list'),
    preview: () => ({ schedules: [] }),
  })
}

export function settingsScheduleGet(id: string): Promise<ScheduleSettings> {
  return invoke<ScheduleSettings>('settings_schedule_get', { id })
}

/**
 * P65.4 — enable/pause through the mutation funnel. Editing a schedule must
 * not mutate an in-flight run; the envelope's `appliedLive` reports whether
 * the change took effect without a restart.
 */
export function settingsScheduleSetEnabled(
  id: string,
  enabled: boolean,
): Promise<SettingsMutationResult> {
  return invoke<SettingsMutationResult>('settings_schedule_set_enabled', { id, enabled })
}

/** P65 — installed skills / plugins / MCP / ACP / hooks with their grants. */
export function settingsExtensionsList(): Promise<ExtensionsEnvelope> {
  return bridgeCall<ExtensionsEnvelope>({
    operation: 'settings_extensions_list',
    live: () => invoke<ExtensionsEnvelope>('settings_extensions_list'),
    preview: () => ({ extensions: [] }),
  })
}

// ───────────────────────────────────────────────────────────────────────────
// Presentation helpers (pure)
// ───────────────────────────────────────────────────────────────────────────

/**
 * Plain-language runtime location for the Settings row. Deliberately shows
 * provenance + exact path instead of collapsing to a boolean: "installed"
 * alone cannot distinguish a Windows App Paths hit from a WSL backend.
 */
export function runtimeLocationLabel(loc: RuntimeLocation): string {
  switch (loc.kind) {
    case 'managed':
      return `managed · ${loc.executable}${loc.version ? ` · v${loc.version}` : ''}`
    case 'windows_path':
    case 'windows_registry':
    case 'user_path':
      return `${loc.kind.replace('_', ' ')} · ${loc.executable}`
    case 'package_manager':
      return `${loc.manager} · ${loc.package}${loc.version ? `@${loc.version}` : ''}`
    case 'wsl':
      return `wsl · ${loc.distro} · ${loc.linuxPath}`
    case 'unavailable':
      return `unavailable · ${loc.reason}`
  }
}

/** True only when a location proves an on-disk executable exists. */
export function isLaunchable(loc: RuntimeLocation): boolean {
  return loc.kind !== 'unavailable' && loc.kind !== 'package_manager'
}
