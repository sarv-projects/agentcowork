// P44.7/44.8 — Discovery surface + routing feed bridge (discovery_cmds.rs).
// Aggregates every managed-resource class (Agents/Models/Providers/MCP/Skills/
// Browsers) into one Discover surface with per-resource cards + lifecycle
// status, plus the provider-level route decision. Auth is a *shape*, never a
// secret. Demo fallback keeps the panel explorable in a plain browser.

import { inTauri, invoke } from './tauri'
import { nativeCall } from './runtime'

export type ResourceKind = 'agent' | 'model' | 'provider' | 'mcp' | 'skill' | 'browser'
export type ManagedStatus =
  | 'discovered' | 'validated' | 'installed' | 'inventoried' | 'enabled'
  | 'started' | 'healthy' | 'in_use' | 'updating' | 'rolling_back' | 'removed'

export interface ResourceCard {
  kind: ResourceKind
  id: string
  name: string
  version: string
  source: string
  auth: string
  capabilities: string[]
  capabilitiesVerified: boolean
  governance: string
  /** Explicit endpoint when the catalog knows one ('' = SDK default). */
  baseUrl?: string
  /** models.dev provider page (full model list + provider docs link). */
  docUrl?: string
  status: ManagedStatus
}
export interface ResourceCounts {
  agents: number
  models: number
  providers: number
  mcp: number
  skills: number
  browsers: number
}
export interface DiscoveryInventory {
  counts: ResourceCounts
  cards: ResourceCard[]
  generation: number
}

export interface RouteDecision {
  ranked: { id: string; score: number; verifiedCapabilities: string[]; health: string }[]
  excluded: { id: string; reason: string }[]
  generation: number
}

export async function discoveryInventory(): Promise<DiscoveryInventory> {
  if (!inTauri()) return demoInventory()
  return nativeCall('discovery inventory', () => invoke<DiscoveryInventory>('discovery_inventory'))
}

export async function routingFeedDecide(req: {
  requiresTools?: boolean
  requiresStructuredOutput?: boolean
  requiresCodex?: boolean
}): Promise<RouteDecision> {
  if (!inTauri()) return { ranked: [], excluded: [], generation: 0 }
  return nativeCall('routing decision', () => invoke<RouteDecision>('routing_feed_decide', req))
}

function demoInventory(): DiscoveryInventory {
  // P71.9g — no built-in agent row. v1's agents are **discovered external
  // agents only** (`ADR-0005`): the demo fallback used to seed an
  // `id: 'inbuilt'` AgentCowork card with `capabilitiesVerified: true` and
  // `status: 'healthy'`, i.e. a browser preview advertised a runnable engine
  // that no install can produce (I15). It is gone; an empty agent count is the
  // honest preview state.
  const cards: ResourceCard[] = [
    { kind: 'provider', id: 'openai', name: 'OpenAI', version: 'catalog', source: 'models.dev', auth: 'api_key_env:OPENAI_API_KEY', capabilities: ['tools', 'vision'], capabilitiesVerified: false, governance: '', status: 'inventoried' },
    { kind: 'provider', id: 'anthropic', name: 'Anthropic', version: 'catalog', source: 'models.dev', auth: 'api_key_env:ANTHROPIC_API_KEY', capabilities: ['tools'], capabilitiesVerified: false, governance: '', status: 'inventoried' },
    { kind: 'model', id: 'ollama/llama3', name: 'llama3', version: '', source: 'local_runtime', auth: 'keyless', capabilities: ['ctx:8192'], capabilitiesVerified: true, governance: 'local', status: 'healthy' },
    { kind: 'browser', id: 'chromium', name: 'chromium', version: '', source: 'system', auth: 'none', capabilities: ['cdp'], capabilitiesVerified: true, governance: '', status: 'healthy' },
  ]
  return {
    counts: { agents: 0, models: 1, providers: 2, mcp: 0, skills: 0, browsers: 1 },
    cards,
    generation: 1,
  }
}
