// P63 — legacy per-agent backend compatibility client (Agent runtimes card).
//
// External agents own their account, authentication, model, and config. The
// host may retain only a credential-free launch override such as a model or
// base URL; it never reads a provider key from the AgentCowork vault for a child
// process and never writes the agent's own config. `agentBackendGet` returns
// non-secret variable names only.

import { invoke } from './tauri'
import { nativeCall } from './runtime'

/** How the agent accepts a provider binding (`agentcowork_acp::BackendChannel`). */
export type BackendChannel =
  | 'provider_env'
  | 'fixed_env'
  | 'config_file'
  | 'subscription'
  | 'unknown'

/** Authentication is always owned and completed by the external agent. */
export type AgentCredentialMode = 'agent_owned'

/** The host has no approved way to delegate a vault-held provider key. */
export type HostVaultInjection = 'unavailable'

export interface AgentBackendChoice {
  provider: string
  model: string
  /**
   * @deprecated Legacy compatibility only. `true` identifies an old record that
   * must be cleared; the host never injects an AgentCowork vault key.
   */
  useVaultKey: boolean
  baseUrl: string | null
}

export interface AgentBackendState {
  agentId: string
  channel: BackendChannel
  credentialMode: AgentCredentialMode
  hostVaultInjection: HostVaultInjection
  /** True only when a non-secret launch override can be planned. */
  injectable: boolean
  note: string
  configFile: string | null
  configured: AgentBackendChoice | null
  /** Credential-free variable names a launch may carry. Never values. */
  injectedEnv: string[]
  /** Requested settings this agent has no variable for. */
  unexpressed: string[]
  /**
   * @deprecated Historical compatibility field. It is always `false` and is not
   * evidence of agent authentication or host-vault injection.
   */
  keyPresent: boolean
  /** Always false: no agent config file is written. */
  writesToDisk: boolean
  /** Why a binding cannot be expressed, when that is the case. */
  refusal: string | null
}

export interface AgentProviderRow {
  id: string
  name: string
  /** The provider's own env var name (models.dev convention). */
  env: string | null
  baseUrl: string
  /** Loopback / localhost endpoint — a keyless local runtime. */
  local: boolean
  /**
   * @deprecated Host-vault inventory observation only. It never means the key is
   * available to, shared with, or accepted by the external agent.
   */
  keyInVault: boolean
  verifiedAt: string | null
}

export interface ProviderProbeResult {
  ok: boolean
  status: number
  message: string
  models: number
  url?: string
}

/** Human label for the channel — the card's badge. */
export function channelLabel(channel: BackendChannel): string {
  switch (channel) {
    case 'provider_env':
      return 'agent-owned provider auth'
    case 'fixed_env':
      return 'agent-owned env auth'
    case 'config_file':
      return 'agent-owned config'
    case 'subscription':
      return 'agent-owned sign-in'
    default:
      return 'auth not verified'
  }
}

/** True when an old saved record requested the now-unavailable vault path. */
export function hasLegacyVaultKeyRequest(
  state: Pick<AgentBackendState, 'configured'>,
): boolean {
  return state.configured?.useVaultKey === true
}

export async function agentBackendGet(agentId: string): Promise<AgentBackendState> {
  return nativeCall('agent backend state', () =>
    invoke<AgentBackendState>('agent_backend_get', { agentId }),
  )
}

export async function agentBackendProviders(agentId: string): Promise<AgentProviderRow[]> {
  const r = await nativeCall('agent backend providers', () =>
    invoke<{ providers: AgentProviderRow[] }>('agent_backend_providers', { agentId }),
  )
  return r.providers ?? []
}

export async function agentBackendSet(input: {
  agentId: string
  provider: string
  model?: string
  /** @deprecated Compatibility only. New UI writes always default to `false`. */
  useVaultKey?: boolean
  baseUrl?: string | null
}): Promise<AgentBackendState> {
  return nativeCall('agent backend set', () =>
    invoke<AgentBackendState>('agent_backend_set', {
      agentId: input.agentId,
      provider: input.provider,
      model: input.model ?? null,
      useVaultKey: input.useVaultKey ?? false,
      baseUrl: input.baseUrl ?? null,
    }),
  )
}

export async function agentBackendClear(agentId: string): Promise<AgentBackendState> {
  return nativeCall('agent backend clear', () =>
    invoke<AgentBackendState>('agent_backend_clear', { agentId }),
  )
}

export async function agentBackendProbe(provider: string): Promise<ProviderProbeResult> {
  return nativeCall('agent backend probe', () =>
    invoke<ProviderProbeResult>('agent_backend_probe', { provider }),
  )
}
