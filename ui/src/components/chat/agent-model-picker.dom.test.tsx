// P71.9a/P71.9c — DOM proof for the composer picker in v1.
//
// The picker used to render AgentCowork's own models.dev catalog and a "Curated
// seed · AgentCowork Native" list for the built-in engine. There is no built-in
// engine any more (ADR-0005 §1), AgentCowork owns no model surface (P71.2d), and
// the retired built-in spellings resolve to *nothing*. What this file proves is
// the v1 contract instead:
//
//   1. agent rows come from the shell's discovery result — never from the static
//      seed pretending to be occupancy;
//   2. an undiscovered machine says so and offers **no** substitute runtime;
//   3. the model surface shown is the **agent's own** ACP option, or an explicit
//      statement that it exposes none;
//   4. a retired spelling in `primary_chief` reads as "no agent bound".

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import type { ReactElement } from 'react'
import {
  click,
  findButton,
  installShell,
  mount,
  registerDom,
  removeShell,
  tick,
  unregisterDom,
  waitFor,
  withAct,
  type Mounted,
} from '@/test/dom-harness'
import { useAppStore } from '@/lib/store'
import { AGENTS, type AgentRuntime } from '@/lib/agents'

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/** A runtime row as the shell's merged catalog would publish it. */
function runtimeRow(id: string, status: AgentRuntime['status']): AgentRuntime {
  const seed = AGENTS.find((a) => a.id === id)
  if (!seed) throw new Error(`fixture needs a real catalog row for ${id}`)
  return { ...seed, status, models: [], defaultModel: '' }
}

/** The agent's own model option, as its session config advertises it. */
const MODEL_OPTION = {
  id: 'model',
  name: 'Model',
  category: 'model',
  type: 'select',
  currentValue: 'claude-sonnet-4.5',
  options: [
    { value: 'claude-sonnet-4.5', name: 'Sonnet 4.5' },
    { value: 'claude-opus-4.1', name: 'Opus 4.1' },
  ],
}

interface StubOptions {
  /** The shell's `primary_chief` value; `null` means the command rejects. */
  primaryChief?: string | null
}

function pickerHandlers(opts: StubOptions = {}): Record<string, (args?: Record<string, unknown>) => unknown> {
  return {
    chief_default_get: () => {
      if (opts.primaryChief === null) throw new Error('no chief default')
      return { primaryChief: opts.primaryChief ?? '', known: [] }
    },
    chief_default_set: () => ({ ok: true }),
    acp_registry_status: () => null,
    agent_directory_list: () => ({ entries: [] }),
    acp_install_status: () => ({}),
  }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

let Picker: (props: { compact?: boolean }) => ReactElement | null
let mounted: Mounted

beforeAll(async () => {
  registerDom()
  Picker = (await import('./agent-model-picker')).default
})

afterAll(() => {
  unregisterDom()
})

beforeEach(async () => {
  installShell(pickerHandlers())
  await withAct(() =>
    useAppStore.setState({
      // Unbound is the v1 starting state: there is no built-in row to default to.
      selectedAgentId: '',
      selectedModelId: '',
      selectedModelProvider: undefined,
      autoRoute: false,
      liveAgents: [],
      acpConfigOptions: {},
      acpHandles: {},
      lastToast: undefined,
    }),
  )
  mounted = await mount(<Picker />)
})

afterEach(() => {
  mounted.unmount()
  removeShell()
})

function trigger(): HTMLButtonElement {
  return findButton(mounted.container, 'button[aria-label="Choose agent and model"]')
}

function agentRow(id: string): HTMLButtonElement | null {
  return mounted.container.querySelector<HTMLButtonElement>(`button[data-agent-id="${id}"]`)
}

async function openPicker(): Promise<void> {
  await click(trigger())
  await tick()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('P71.9a — rows come from discovery, never from the seed', () => {
  test('an empty inventory is stated and no runtime is offered in its place', async () => {
    await openPicker()

    const ok = await waitFor(() =>
      (mounted.container.textContent ?? '').includes('Runtime inventory unavailable'),
    )
    expect(ok).toBe(true)
    // Nothing is substituted: no curated row, and certainly no built-in one.
    expect(agentRow('claude-code')).toBeNull()
    // DEC-053: this legacy spelling stays verbatim so retirement recognition keeps working.
    expect(agentRow('everyaios-native')).toBeNull()
    expect(mounted.container.textContent ?? '').not.toContain('always available')
  })

  test('a discovered installed runtime is listed and selectable', async () => {
    await withAct(() =>
      useAppStore.setState({ liveAgents: [runtimeRow('claude-code', 'installed')] }),
    )
    await openPicker()

    const row = agentRow('claude-code')
    expect(row).not.toBeNull()
    expect(mounted.container.textContent ?? '').not.toContain('Runtime inventory unavailable')
    if (!row) return
    await click(row)
    expect(useAppStore.getState().selectedAgentId).toBe('claude-code')
  })

  test('a registry entry with no binary reads `not installed` and is not selectable', async () => {
    await withAct(() =>
      useAppStore.setState({
        liveAgents: [runtimeRow('claude-code', 'available'), runtimeRow('codex-cli', 'installed')],
      }),
    )
    await openPicker()

    const row = agentRow('claude-code')
    expect(row).not.toBeNull()
    expect(row?.textContent ?? '').toContain('not installed')
    if (!row) return
    await click(row)
    // It must not become a selection the send path cannot launch.
    expect(useAppStore.getState().selectedAgentId).toBe('')
  })

  test('outside the shell the labelled preview fixture is kept', async () => {
    removeShell()
    await openPicker()
    // The browser preview has no shell to ask, so the fixture stands in — but
    // it is the only place the seed is allowed to read as a runtime list.
    expect(agentRow('claude-code')).not.toBeNull()
    expect(mounted.container.textContent ?? '').not.toContain('Runtime inventory unavailable')
  })
})

describe('P71.9c — the model surface is the agent\u2019s own', () => {
  test('shows the agent\u2019s ACP model option, not an AgentCowork catalog', async () => {
    await withAct(() =>
      useAppStore.setState({
        liveAgents: [runtimeRow('claude-code', 'installed')],
        selectedAgentId: 'claude-code',
        acpConfigOptions: { 'claude-code': [MODEL_OPTION] },
      }),
    )
    await openPicker()

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('Model: claude-sonnet-4.5')
    // The retired surfaces must not reappear inside the picker.
    expect(body).not.toContain('Your providers · models.dev')
    expect(body).not.toContain('Curated seed')
  })

  test('says so when the agent exposes no model option', async () => {
    await withAct(() =>
      useAppStore.setState({
        liveAgents: [runtimeRow('claude-code', 'installed')],
        selectedAgentId: 'claude-code',
        acpConfigOptions: {},
      }),
    )
    await openPicker()

    expect(mounted.container.textContent ?? '').toContain(
      'No ACP model option is exposed. AgentCowork will not show or inject its Native BYOK/local models here.',
    )
  })
})

describe('P71.2a — the slot never names a built-in engine', () => {
  test('a retired spelling reads as unbound, not as an engine', async () => {
    removeShell()
    installShell(pickerHandlers({ primaryChief: 'inbuilt' }))
    mounted.unmount()
    mounted = await mount(<Picker />)
    await openPicker()

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('no agent bound')
    expect(body).not.toContain('inbuilt engine')
    // Nor does it render a "swappable" badge for an engine that does not exist.
    expect(body).not.toContain('swappable')
  })

  test('a real default is named as the agent it is', async () => {
    removeShell()
    installShell(pickerHandlers({ primaryChief: 'claude-code' }))
    mounted.unmount()
    mounted = await mount(<Picker />)
    await openPicker()

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('claude-code')
    expect(body).not.toContain('no agent bound')
  })
})
