// P71.2a/P71.2d — Settings renders one agent surface, and every row is external.
//
// The product contract is one primary Settings surface for agent/runtime
// management (ARCH/12): there is no "Native models" peer tab, no built-in runtime
// card, and no "AgentCowork Native model catalog" disclosure — because AgentCowork
// owns no model surface at all and ships no built-in agent in v1 (ADR-0005 §1).

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import type { ReactElement } from 'react'
import { click, installShell, mount, registerDom, removeShell, unregisterDom, waitFor, withAct, type Mounted } from '@/test/dom-harness'
import { useAppStore } from '@/lib/store'
import { AGENTS } from '@/lib/agents'
import type { AgentBackendState } from '@/lib/agent-backend'

let Section: () => ReactElement
let mounted: Mounted

const EXTERNAL = AGENTS.find((a) => a.id === 'claude-code')!

const LIVE_AGENT_SETTINGS = {
  agentId: 'claude-code',
  installed: true,
  protocol: 'acp',
  authMode: 'subscription',
  nativeCapabilities: ['chat', 'tools'],
  sharedCapabilities: [],
  modelOwner: 'agent',
  configOptions: [],
  readiness: 'ready',
  location: { kind: 'path', source: 'path', executable: 'claude', version: '1.2.3' },
  sessionLoadout: [],
}

function backendState(overrides: Partial<AgentBackendState> = {}): AgentBackendState {
  return {
    agentId: 'claude-code',
    channel: 'fixed_env',
    credentialMode: 'agent_owned',
    hostVaultInjection: 'unavailable',
    injectable: true,
    note: 'Credential-free launch inputs only.',
    configFile: null,
    configured: null,
    injectedEnv: [],
    unexpressed: [],
    keyPresent: false,
    writesToDisk: false,
    refusal: null,
    ...overrides,
  }
}

beforeAll(async () => {
  registerDom()
  Section = (await import('./agents-models-section')).default
})

afterAll(() => {
  unregisterDom()
})

beforeEach(async () => {
  // Register the capability row the surface fetches on mount. Left unregistered,
  // the harness answers `{}` — which is a partial row, and must render, not throw.
  installShell({
    settings_agent_get: () => LIVE_AGENT_SETTINGS,
  })
  await withAct(() =>
    useAppStore.setState({
      selectedAgentId: 'claude-code',
      selectedModelId: '',
      selectedModelProvider: undefined,
      liveAgents: [{ ...EXTERNAL, status: 'installed', version: '1.2.3' }],
      acpConfigOptions: {},
    }),
  )
  mounted = await mount(<Section />)
})

afterEach(() => {
  // A mount that throws must surface that error, not a secondary crash here.
  mounted?.unmount()
  removeShell()
})

describe('P71.2a — Settings renders one agent surface with no built-in row', () => {
  test('the only peer tabs are Runtimes and Routing', async () => {
    await waitFor(() => mounted.container.querySelector('[role="tablist"]') !== null)

    const tabs = Array.from(mounted.container.querySelectorAll('[role="tab"]')).map(
      (t) => (t.textContent ?? '').trim(),
    )
    expect(tabs).toEqual(['Runtimes', 'Routing'])
    // Neither the old peer "Native models" tab nor a built-in runtime card
    // exists in v1.
    expect(tabs).not.toContain('Native models')
    const body = mounted.container.textContent ?? ''
    expect(body).not.toContain('AgentCowork Native')
    expect(body).not.toContain('native-catalog-toggle')
  })

  test('shows live ACP readiness and agent-owned authentication without a host-vault claim', async () => {
    await waitFor(() => mounted.container.querySelector('[role="tablist"]') !== null)

    const rows = Array.from(mounted.container.querySelectorAll('dl > div')).map((row) => [
      row.querySelector('dt')?.textContent?.trim() ?? '',
      row.querySelector('dd')?.textContent?.trim() ?? '',
    ])
    expect(rows).toContainEqual(['ACP readiness', 'ready'])
    expect(rows).toContainEqual(['authentication', 'agent-owned · subscription'])
    expect(rows).toContainEqual(['models', 'agent-owned · no host override'])

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('auth owner: Claude Code')
    expect(body).not.toContain('Key: from the AgentCowork vault')
  })

  test('a legacy vault-key record has a visible clear/migrate path and clears through IPC', async () => {
    mounted.unmount()
    let cleared = false
    let clearedAgent = ''
    const legacy = backendState({
      injectable: false,
      configured: {
        provider: 'anthropic',
        model: 'legacy-model',
        useVaultKey: true,
        baseUrl: null,
      },
      refusal: 'host provider binding unavailable; no vault key will be injected',
    })
    const invoked = installShell({
      settings_agent_get: () => LIVE_AGENT_SETTINGS,
      agent_backend_get: () => (cleared ? backendState() : legacy),
      agent_backend_providers: () => ({ providers: [] }),
      agent_backend_clear: (args) => {
        clearedAgent = String(args?.agentId ?? '')
        cleared = true
        return backendState()
      },
    })
    mounted = await mount(<Section />)

    await waitFor(() => mounted.container.querySelector('[role="tablist"]') !== null)
    const toggle = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="agent-configure-claude-code"]',
    )
    expect(toggle).not.toBeNull()
    await click(toggle!)
    await waitFor(() => mounted.container.querySelector('[data-testid="agent-backend-claude-code"]') !== null)

    let body = mounted.container.textContent ?? ''
    expect(body).toContain('legacy host-vault request')
    expect(body).toContain('No host vault key was or will be injected')
    expect(body).not.toContain('injected from the AgentCowork vault')

    const clear = mounted.container.querySelector<HTMLButtonElement>(
      '[aria-label="Clear legacy host-vault request for claude-code"]',
    )
    expect(clear).not.toBeNull()
    await click(clear!)
    await waitFor(() => !(mounted.container.textContent ?? '').includes('legacy host-vault request'))

    expect(clearedAgent).toBe('claude-code')
    expect(invoked).toContain('agent_backend_clear')
    body = mounted.container.textContent ?? ''
    expect(body).toContain('Authentication: agent-owned / self-contained')
  })

  test('does not project an empty host binding as managed model ownership', async () => {
    mounted.unmount()
    installShell({
      settings_agent_get: () => ({
        ...LIVE_AGENT_SETTINGS,
        modelOwner: 'managed',
        backendBinding: {
          providerId: 'anthropic',
          injectedEnvNames: [],
          unexpressed: [],
          writesToAgentConfig: false,
          keyPresent: true,
        },
      }),
    })
    mounted = await mount(<Section />)

    await waitFor(() => mounted.container.querySelector('[role="tablist"]') !== null)
    const models = Array.from(mounted.container.querySelectorAll('dl > div')).find(
      (row) => row.querySelector('dt')?.textContent?.trim() === 'models',
    )
    expect(models?.querySelector('dd')?.textContent?.trim()).toBe('agent-owned · no host override')
  })

  test('a partial capability row renders instead of throwing', async () => {
    // Regression: the shell can answer with a row that predates the
    // capability fields (or a probe that failed). Rendering it must degrade,
    // not crash the whole panel.
    mounted.unmount()
    installShell({ settings_agent_get: () => ({}) })
    mounted = await mount(<Section />)

    await waitFor(() => mounted.container.querySelector('[role="tablist"]') !== null)
    expect((mounted.container.textContent ?? '').includes('Agent capabilities')).toBe(true)
  })

  test('an undiscovered machine says nothing can run, rather than offering a built-in', async () => {
    mounted.unmount()
    await withAct(() => useAppStore.setState({ liveAgents: [] }))
    mounted = await mount(<Section />)

    await waitFor(() => mounted.container.querySelector('[role="tablist"]') !== null)
    const body = mounted.container.textContent ?? ''
    // The discovery warning is explicit that an empty list is not occupancy…
    expect(body).toContain('not occupancy')
    // …and nothing is painted as an always-available runtime.
    expect(body).not.toContain('AgentCowork Native')
    expect(body).not.toContain('always live')
  })
})
