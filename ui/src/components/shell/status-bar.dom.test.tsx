// P71.2d — DOM proof for the status bar's model label.
//
// The bar used to name the exact `(provider, model-id)` pair the broker would
// receive for AgentCowork's own model call. There is no such call any more: the
// bound agent owns its model, so the bar paints **that agent's** ACP value (or an
// explicit "managed by <agent>") and never a desktop-sourced model name. With no
// usable agent row it must claim nothing at all.

import { afterAll, afterEach, beforeAll, describe, expect, test } from 'bun:test'
import type { ReactElement } from 'react'
import { installShell, mount, registerDom, unregisterDom, withAct, type Mounted } from '@/test/dom-harness'
import { useAppStore } from '@/lib/store'

let StatusBar: () => ReactElement
let mounted: Mounted

/** A usable external runtime row (the bar only paints the label for one). */
const AGENT = {
  id: 'claude-code',
  name: 'Claude Code',
  mark: 'CC',
  accent: 'bg-brand text-black',
  status: 'installed' as const,
  capabilities: [],
  models: [],
  defaultModel: '',
}

/**
 * The agent's own model option, exactly as `available_commands_update` /
 * session config advertises it.
 */
const MODEL_OPTION = {
  id: 'model',
  name: 'Model',
  category: 'model',
  type: 'select',
  currentValue: 'claude-sonnet-4.5',
}

async function setState(patch: Record<string, unknown>): Promise<void> {
  await withAct(() => useAppStore.setState(patch as never))
  await withAct(() => {})
}

beforeAll(async () => {
  registerDom()
  StatusBar = (await import('./status-bar')).StatusBar
})

afterAll(() => {
  unregisterDom()
})

afterEach(() => {
  mounted.unmount()
})

describe('P71.2d — the bar names the agent\u2019s own model, never AgentCowork\u2019s', () => {
  test('paints the agent\u2019s ACP model value', async () => {
    installShell()
    await setState({
      selectedAgentId: 'claude-code',
      selectedModelId: '',
      selectedModelProvider: undefined,
      acpConfigOptions: { 'claude-code': [MODEL_OPTION] },
      liveAgents: [AGENT],
      activeSessionId: null,
      sessions: [],
      autoRoute: false,
      // The full bar (with the agent/model block) is the devMode surface; the
      // default compact bar is a preview strip that carries no model claim.
      devMode: true,
    })
    mounted = await mount(<StatusBar />)

    expect(mounted.container.textContent ?? '').toContain('claude-sonnet-4.5')
  })

  test('says who owns the model when the agent exposes none', async () => {
    installShell()
    await setState({
      selectedAgentId: 'claude-code',
      selectedModelId: 'anthropic/claude-sonnet-4',
      selectedModelProvider: 'openrouter',
      acpConfigOptions: {},
      liveAgents: [AGENT],
      activeSessionId: null,
      sessions: [],
      autoRoute: false,
      devMode: true,
    })
    mounted = await mount(<StatusBar />)

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('managed by Claude Code')
    // A leftover desktop pair must never be painted as if it governed the agent.
    expect(body).not.toContain('openrouter · anthropic/claude-sonnet-4')
  })

  test('claims no model when no live runtime is usable', async () => {
    installShell()
    await setState({
      selectedAgentId: 'claude-code',
      selectedModelId: 'anthropic/claude-sonnet-4',
      selectedModelProvider: 'openrouter',
      acpConfigOptions: {},
      liveAgents: [],
      activeSessionId: null,
      sessions: [],
      autoRoute: false,
      devMode: true,
    })
    mounted = await mount(<StatusBar />)

    // The model block is gated on a usable agent row: without one the bar must
    // not paint a model the runtime cannot be shown to serve.
    expect(mounted.container.textContent ?? '').not.toContain('openrouter · anthropic/claude-sonnet-4')
  })
})
