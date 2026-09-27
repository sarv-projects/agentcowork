// Model ownership boundary — restated for v1 (`P71.2a`/`P71.2d`, ADR-0005 §1/§2).
//
// The P60 boundary used to read "AgentCowork Native owns the provider/model
// catalog; every *other* runtime owns its own". v1 removes the first half: there
// is **no** built-in runtime, and AgentCowork owns no model surface for any agent.
// The catalogue, the key vault and the usage ledger are *observation*; the model
// a turn runs on belongs to the bound agent, exposed (if at all) through that
// agent's own ACP config options.
//
// These tests pin the two failure modes that made the old picker dishonest, in
// their v1 form: (1) rendering a desktop model list as if it controlled an
// agent, and (2) treating a curated seed or a retired built-in spelling as an
// agent that can run.

import { afterAll, beforeEach, describe, expect, test } from 'bun:test'
import {
  AGENTS,
  getModelsForAgentLive,
  isRuntimeUsable,
  type AgentRuntime,
} from './agents'
import { currentBinding, isRetiredBinding } from './acp'
import { useAppStore } from './store'

const EXTERNAL = AGENTS.find((a) => a.id === 'claude-code')!

// bun's runner shares the module registry across test files, so the store
// mutations below must not leak into other suites.
afterAll(() => {
  useAppStore.setState({
    selectedAgentId: '',
    selectedModelId: '',
    selectedModelProvider: undefined,
    liveAgents: [],
    autoRoute: false,
  })
})

/** An external runtime that discovery verified on this machine. */
function installed(a: AgentRuntime): AgentRuntime {
  return { ...a, status: 'installed', path: '/usr/local/bin/claude', version: '1.2.3' }
}

describe('P71.2a — no built-in runtime exists', () => {
  // DEC-053: the legacy spellings below stay verbatim so retirement recognition keeps working.
  test('the catalog carries no built-in row', () => {
    expect(AGENTS.some((a) => a.id === 'everyaios-native')).toBe(false)
    expect(AGENTS.some((a) => a.id === 'everyaios')).toBe(false)
    expect(AGENTS.length).toBeGreaterThan(0)
  })

  test('the retired built-in spellings are not bindings', () => {
    expect(isRetiredBinding('everyaios-native')).toBe(true)
    expect(isRetiredBinding('everyaios')).toBe(true)
    expect(isRetiredBinding('inbuilt')).toBe(true)
    expect(isRetiredBinding('claude-code')).toBe(false)
    expect(currentBinding('everyaios-native')).toBeNull()
    expect(currentBinding('inbuilt')).toBeNull()
    expect(currentBinding('')).toBeNull()
    expect(currentBinding(undefined)).toBeNull()
    expect(currentBinding('claude-code')).toBe('claude-code')
  })
})

describe('P71.2d — AgentCowork owns no model surface for any runtime', () => {
  test('no curated model list is offered, installed or not', () => {
    expect(getModelsForAgentLive('claude-code', [installed(EXTERNAL)])).toEqual([])
    expect(getModelsForAgentLive('claude-code', [EXTERNAL])).toEqual([])
    expect(getModelsForAgentLive('everyaios-native', [installed(EXTERNAL)])).toEqual([])
    expect(getModelsForAgentLive('opencode', [installed({ ...EXTERNAL, id: 'opencode' })])).toEqual([])
  })

  test('usability is evidence: a curated seed is never "installed"', () => {
    // The seed marks external runtimes `available` — a catalog entry, not an
    // install claim. Before discovery confirms it, nothing may present models.
    expect(isRuntimeUsable({ ...EXTERNAL, status: 'available' })).toBe(false)
    expect(isRuntimeUsable(installed(EXTERNAL))).toBe(true)
    expect(isRuntimeUsable(undefined)).toBe(false)
  })
})

describe('P60/P71.2d — selecting an agent is id-only', () => {
  beforeEach(() => {
    useAppStore.setState({
      selectedAgentId: '',
      selectedModelId: '',
      selectedModelProvider: undefined,
      liveAgents: [],
    })
  })

  test('selection never invents a desktop model pin', () => {
    useAppStore.getState().setSelectedAgent('claude-code')
    const st = useAppStore.getState()
    expect(st.selectedAgentId).toBe('claude-code')
    expect(st.selectedModelId).toBe('')
    expect(st.selectedModelProvider).toBeUndefined()
  })

  test('a runtime known only from live discovery is selectable', () => {
    // A freshly refreshed ACP registry row has no static seed entry; refusing
    // it would make the dynamic catalog unusable.
    useAppStore.setState({
      liveAgents: [
        {
          ...EXTERNAL,
          id: 'crow-cli',
          name: 'Crow CLI',
          status: 'installed',
          models: [],
          defaultModel: '',
        },
      ],
    })
    useAppStore.getState().setSelectedAgent('crow-cli')
    expect(useAppStore.getState().selectedAgentId).toBe('crow-cli')
  })

  test('an unknown id is still refused fail-closed', () => {
    useAppStore.getState().setSelectedAgent('no-such-agent')
    expect(useAppStore.getState().selectedAgentId).toBe('')
  })

  test('variant cycling is gone — there is no desktop model list to cycle', () => {
    useAppStore.setState({ selectedAgentId: 'claude-code' })
    expect(useAppStore.getState().cycleModelVariant(1)).toBeUndefined()
    expect(useAppStore.getState().cycleModelVariant(-1)).toBeUndefined()
  })
})
