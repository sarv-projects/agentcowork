/**
 * Session capability loadout definitions and utilities (P66.4).
 *
 * Each session holds an explicit capability loadout specifying which shared
 * cowork capabilities, MCP servers, skills, and tools are active for that
 * conversation. Changes apply to the next turn/run and are snapshot into the
 * Work RuntimeManifest.
 */

export interface SessionCapabilityItem {
  id: string
  name: string
  description: string
  category: 'office' | 'browser' | 'desktop' | 'search' | 'storage' | 'memory' | 'mcp' | 'skill' | 'connector' | 'fleet' | 'calendar'
  family: string
  nativeOrShared: 'native' | 'shared'
  enabled: boolean
  requiresApproval: boolean
  appliesFrom: 'next_turn'
  health: 'ready' | 'permission_required' | 'unverified' | 'disabled'
}

export interface SessionCapabilityLoadout {
  /** Map of capability id -> boolean override (true = enabled, false = disabled) */
  overrides: Record<string, boolean>
  updatedAt: number
}

/** Standard AgentCowork Shared Cowork capabilities available across all agents. */
export const STANDARD_SHARED_CAPABILITIES: SessionCapabilityItem[] = [
  {
    id: 'shared:office',
    name: 'Office Calc & Docs',
    description: 'Local Excel formulas (.xlsx), Word (.docx), PowerPoint (.pptx), PDF form filling',
    category: 'office',
    family: 'office',
    nativeOrShared: 'shared',
    enabled: true,
    requiresApproval: false,
    appliesFrom: 'next_turn',
    health: 'ready',
  },
  {
    id: 'shared:browser',
    name: 'Browser Automation',
    description: '37 CDP web automation tools, navigation, form fill, browser-profile reuse',
    category: 'browser',
    family: 'browser',
    nativeOrShared: 'shared',
    enabled: true,
    requiresApproval: true,
    appliesFrom: 'next_turn',
    health: 'ready',
  },
  {
    id: 'shared:desktop',
    name: 'Native Computer Use',
    description: 'Native OS desktop control via visual grounding, OCR, and mouse/keyboard events',
    category: 'desktop',
    family: 'desktop',
    nativeOrShared: 'shared',
    enabled: true,
    requiresApproval: true,
    appliesFrom: 'next_turn',
    health: 'ready',
  },
  {
    id: 'shared:search',
    name: 'Deep Web Search',
    description: 'Tiered SearXNG + DuckDuckGo research cascade ($0 search API fees)',
    category: 'search',
    family: 'search',
    nativeOrShared: 'shared',
    enabled: true,
    requiresApproval: false,
    appliesFrom: 'next_turn',
    health: 'ready',
  },
  {
    id: 'shared:storage',
    name: 'Storage Intelligence',
    description: '7-stage hash deduplication (xxHash3/BLAKE3) & disk space treemaps',
    category: 'storage',
    family: 'storage',
    nativeOrShared: 'shared',
    enabled: true,
    requiresApproval: false,
    appliesFrom: 'next_turn',
    health: 'ready',
  },
  {
    id: 'shared:memory',
    name: '5-Tier Cognitive Memory',
    description: 'Working, episodic, semantic, personal taste profile, and knowledge graph',
    category: 'memory',
    family: 'memory',
    nativeOrShared: 'shared',
    enabled: true,
    requiresApproval: false,
    appliesFrom: 'next_turn',
    health: 'ready',
  },
  {
    id: 'shared:fleet',
    name: 'Multi-Agent Swarm Fleet',
    description: 'Autonomous multi-worktree subagent delegation with GitOperationQueue synchronization',
    category: 'fleet',
    family: 'fleet',
    nativeOrShared: 'shared',
    enabled: true,
    requiresApproval: true,
    appliesFrom: 'next_turn',
    health: 'ready',
  },
  {
    id: 'shared:calendar',
    name: 'Calendar & AI Automations',
    description: 'Conversational event scheduling, RFC 5545 recurrence rules, background automations',
    category: 'calendar',
    family: 'calendar',
    nativeOrShared: 'shared',
    enabled: true,
    requiresApproval: false,
    appliesFrom: 'next_turn',
    health: 'ready',
  },
]

/**
 * Determine if a specific capability is enabled under the given session loadout.
 */
export function isCapabilityEnabled(
  capabilityId: string,
  loadout?: SessionCapabilityLoadout | null,
  defaultValue = true,): boolean {
  if (!loadout || !loadout.overrides) return defaultValue
  if (capabilityId in loadout.overrides) {
    return Boolean(loadout.overrides[capabilityId])
  }
  return defaultValue
}

/**
 * Filter tool items according to a session's capability loadout overrides.
 */
export function filterToolsByCapabilityLoadout<T extends { id: string; family?: string }>(
  tools: T[],
  loadout?: SessionCapabilityLoadout | null,
): T[] {
  if (!loadout || !loadout.overrides || Object.keys(loadout.overrides).length === 0) {
    return tools
  }

  return tools.filter((tool) => {
    if (tool.id in loadout.overrides && !loadout.overrides[tool.id]) {
      return false
    }
    if (tool.family) {
      const sharedFamilyKey = 'shared:' + tool.family
      if (sharedFamilyKey in loadout.overrides && !loadout.overrides[sharedFamilyKey]) {
        return false
      }
    }
    return true
  })
}
