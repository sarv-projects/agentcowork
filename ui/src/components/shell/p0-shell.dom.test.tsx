import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import * as React from 'react'
import {
  click,
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
import { AGENTS } from '@/lib/agents'

let LeftSidebar: typeof import('./left-sidebar').LeftSidebar
let groupSessionsByAttention: typeof import('./left-sidebar').groupSessionsByAttention
let RightViewport: typeof import('./right-rail').RightViewport
let NarrowRightTabBar: typeof import('./right-rail').NarrowRightTabBar
let FirstRunSurfaces: typeof import('@/App').FirstRunSurfaces
let ThemeProvider: typeof import('../theme-provider').ThemeProvider
let TooltipProvider: typeof import('../ui/tooltip').TooltipProvider

let mounted: Mounted | undefined

function readyAgent() {
  const seed = AGENTS.find((agent) => agent.id === 'claude-code')
  if (!seed) throw new Error('agent fixture is missing')
  return {
    ...seed,
    status: 'installed' as const,
    readiness: 'ready' as const,
    discovered: true,
    launchable: true,
  }
}

function resetState() {
  useAppStore.setState({
    onboardingDone: false,
    setupOpen: false,
    sessions: [],
    activeSessionId: '',
    sessionsHydrated: true,
    liveAgents: [],
    selectedAgentId: '',
    userDefaultChief: undefined,
    composerValue: '',
    centerScreen: 'home',
    sidebarCollapsed: false,
    railCollapsed: false,
    powerMode: false,
    fullscreenView: false,
    activeView: 'office-xlsx',
    openViews: ['folder', 'shell', 'browse', 'office-xlsx'],
    workItems: [],
    workEvents: [],
  } as never)
  window.localStorage.removeItem('agentcowork.settings.onboardingDone')
}

function shellHandlers(overrides: Record<string, (args?: Record<string, unknown>) => unknown> = {}) {
  return {
    acp_install_status: () => ({ 'claude-code': { installed: true, discovered: true, launchable: true } }),
    chief_default_set: () => ({ ok: true }),
    ...overrides,
  }
}

async function setInputValue(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set
  setter?.call(input, value)
  input.dispatchEvent(new window.Event('input', { bubbles: true }))
  await tick()
}

beforeAll(async () => {
  registerDom()
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: (query: string) => ({
      matches: query.includes('prefers-reduced-motion'),
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  })
  const sidebar = await import('./left-sidebar')
  LeftSidebar = sidebar.LeftSidebar
  groupSessionsByAttention = sidebar.groupSessionsByAttention
  const rail = await import('./right-rail')
  RightViewport = rail.RightViewport
  NarrowRightTabBar = rail.NarrowRightTabBar
  FirstRunSurfaces = (await import('@/App')).FirstRunSurfaces
  ThemeProvider = (await import('../theme-provider')).ThemeProvider
  TooltipProvider = (await import('../ui/tooltip')).TooltipProvider
})

afterAll(() => {
  unregisterDom()
})

beforeEach(() => {
  resetState()
})

afterEach(() => {
  mounted?.unmount()
  mounted = undefined
  removeShell()
})

describe('first-run ownership and draft preservation', () => {
  test('keeps one visible owner and reaches a first task through the short path', async () => {
    installShell(shellHandlers())
    await withAct(() => useAppStore.setState({ liveAgents: [readyAgent()] } as never))
    mounted = await mount(
      <ThemeProvider defaultTheme="dark">
        <TooltipProvider>
          <FirstRunSurfaces />
        </TooltipProvider>
      </ThemeProvider>,
    )

    expect(useAppStore.getState().onboardingDone).toBe(false)
    expect(document.querySelectorAll('[data-first-run-owner="onboarding"]')).toHaveLength(1)
    expect(document.querySelector('[data-first-run-owner="setup"]')).toBeNull()

    const findAgent = document.querySelector<HTMLButtonElement>('[data-testid="onboarding-find-agent"]')
    expect(findAgent).not.toBeNull()
    await click(findAgent!)
    expect(await waitFor(() => document.body.textContent?.includes('Use this agent') ?? false)).toBe(true)

    const useAgent = [...document.querySelectorAll<HTMLButtonElement>('button')].find((button) => button.textContent?.includes('Use this agent'))
    expect(useAgent).not.toBeNull()
    await click(useAgent!)
    expect(await waitFor(() => document.querySelector('[data-testid="onboarding-first-task"]') !== null)).toBe(true)

    const input = document.querySelector<HTMLInputElement>('#onboarding-first-task')
    expect(input).not.toBeNull()
    await setInputValue(input!, 'Summarise the file I attach.')
    const start = document.querySelector<HTMLButtonElement>('[data-testid="onboarding-start-task"]')
    expect(start).not.toBeNull()
    await click(start!)

    expect(useAppStore.getState().onboardingDone).toBe(true)
    expect(useAppStore.getState().composerValue).toBe('Summarise the file I attach.')
    expect(useAppStore.getState().centerScreen).toBe('chat')
    expect(useAppStore.getState().sessions).toHaveLength(1)
  })

  test('hands ownership to the setup gate after onboarding is complete', async () => {
    installShell(shellHandlers())
    await withAct(() => useAppStore.setState({ onboardingDone: true, setupOpen: true } as never))
    mounted = await mount(
      <ThemeProvider defaultTheme="dark">
        <FirstRunSurfaces />
      </ThemeProvider>,
    )
    expect(await waitFor(() => document.querySelector('[data-first-run-owner="setup"]') !== null)).toBe(true)
    expect(document.querySelector('[data-first-run-owner="onboarding"]')).toBeNull()
  })

  test('keeps the passphrase draft when the vault rejects setup', async () => {
    installShell(shellHandlers({
      vault_setup: () => {
        throw new Error('vault write denied')
      },
    }))
    await withAct(() => useAppStore.setState({ liveAgents: [readyAgent()] } as never))
    mounted = await mount(
      <ThemeProvider defaultTheme="dark">
        <TooltipProvider>
          <FirstRunSurfaces />
        </TooltipProvider>
      </ThemeProvider>,
    )

    await click(document.querySelector<HTMLButtonElement>('[data-testid="onboarding-find-agent"]')!)
    expect(await waitFor(() => document.body.textContent?.includes('Use this agent') ?? false)).toBe(true)
    const useAgent = [...document.querySelectorAll<HTMLButtonElement>('button')].find((button) => button.textContent?.includes('Use this agent'))
    await click(useAgent!)
    const later = [...document.querySelectorAll<HTMLButtonElement>('button')].find((button) => button.textContent?.includes('Set up vault later'))
    expect(later).not.toBeNull()
    await click(later!)

    const firstPassphrase = document.querySelector<HTMLInputElement>('input[type="password"]')
    expect(firstPassphrase).not.toBeNull()
    await setInputValue(firstPassphrase!, 'correct horse battery')
    const inputs = [...document.querySelectorAll<HTMLInputElement>('input[type="password"]')]
    expect(inputs).toHaveLength(2)
    await setInputValue(inputs[1]!, 'correct horse battery')
    const submit = [...document.querySelectorAll<HTMLButtonElement>('button')].find((button) => button.textContent?.includes('Set Passphrase & Start'))
    expect(submit).not.toBeNull()
    await click(submit!)

    expect(await waitFor(() => document.body.textContent?.includes('vault write denied') ?? false)).toBe(true)
    expect((document.querySelectorAll('input[type="password"]')[0] as HTMLInputElement).value).toBe('correct horse battery')
    expect((document.querySelectorAll('input[type="password"]')[1] as HTMLInputElement).value).toBe('correct horse battery')
    expect(useAppStore.getState().onboardingDone).toBe(false)
  })
})

describe('attention-first sidebar', () => {
  test('groups needs attention, running, and recent/idle with owner, reason, and next action', async () => {
    const groups = groupSessionsByAttention([
      { id: 'idle', status: 'idle', title: 'Idle', preview: 'Ready', updatedAt: '2026-01-01', agent: 'claude-code', folder: '~/work/idle' },
      { id: 'run', status: 'running', title: 'Running', preview: '', updatedAt: '2026-01-02', agent: 'codex-cli', folder: '~/work/run' },
      { id: 'ask', status: 'action-required', title: 'Ask', preview: '', updatedAt: '2026-01-03', agent: 'claude-code', folder: '~/work/ask' },
    ] as never)
    expect(groups.map((group) => group.id)).toEqual(['needs-attention', 'running', 'recent'])
    expect(groups[0]?.sessions[0]?.id).toBe('ask')

    installShell(shellHandlers())
    await withAct(() => useAppStore.setState({
      sessions: [
        { id: 'ask', title: 'Ask', status: 'action-required', preview: '', updatedAt: '2026-01-03', agent: 'claude-code', folder: '~/work/ask', messages: [] },
        { id: 'run', title: 'Running', status: 'running', preview: '', updatedAt: '2026-01-02', agent: 'codex-cli', folder: '~/work/run', messages: [] },
        { id: 'idle', title: 'Idle', status: 'idle', preview: 'Ready', updatedAt: '2026-01-01', agent: 'claude-code', folder: '~/work/idle', messages: [] },
      ],
      activeSessionId: 'ask',
    } as never))
    mounted = await mount(
      <TooltipProvider>
        <LeftSidebar />
      </TooltipProvider>,
    )
    const body = document.body.textContent ?? ''
    expect(body).toContain('Needs attention')
    expect(body).toContain('Running')
    expect(body).toContain('Recent / idle')
    expect(body).toContain('Claude Code')
    expect(body).toContain('Waiting for your approval')
    expect(body).toContain('Review')
    expect(body).toContain('run')
  })
})

describe('narrow shell fallback', () => {
  test('uses an accessible drawer and restores focus after Escape', async () => {
    function DrawerHarness() {
      const [open, setOpen] = React.useState(false)
      return (
        <>
          <button type="button" data-testid="drawer-trigger" onClick={() => setOpen(true)}>Open</button>
          <LeftSidebar narrow drawerOpen={open} onDrawerOpenChange={setOpen} />
        </>
      )
    }

    installShell(shellHandlers())
    mounted = await mount(<DrawerHarness />)
    const trigger = mounted.container.querySelector<HTMLButtonElement>('[data-testid="drawer-trigger"]')!
    trigger.focus()
    await click(trigger)
    const drawer = document.querySelector<HTMLElement>('[data-sidebar-mode="overlay"]')
    expect(drawer).not.toBeNull()
    expect(drawer?.getAttribute('role')).toBe('dialog')
    expect(drawer?.getAttribute('aria-modal')).toBe('true')
    expect(document.activeElement?.getAttribute('data-sidebar-close')).not.toBeNull()

    await withAct(() => document.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true })))
    await tick()
    expect(document.querySelector('[data-sidebar-mode="overlay"]')).toBeNull()
    expect(document.activeElement?.getAttribute('data-testid')).toBe('drawer-trigger')
  })

  test('renders the four narrow lens tabs with roving keyboard focus', async () => {
    function TabsHarness() {
      const [tab, setTab] = React.useState<import('./right-rail').NarrowRightTab>('chat')
      return <NarrowRightTabBar activeTab={tab} onChange={setTab} />
    }

    installShell(shellHandlers())
    mounted = await mount(<TabsHarness />)
    const tabs = [...document.querySelectorAll<HTMLButtonElement>('[role="tab"]')]
    expect(tabs.map((tab) => tab.textContent?.trim())).toEqual(['Chat', 'Files', 'Preview', 'Tools'])
    expect(tabs[0]?.getAttribute('aria-selected')).toBe('true')

    tabs[0]?.focus()
    await withAct(() => { tabs[0]?.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true })) })
    await tick()
    expect(document.querySelector('[role="tab"][aria-selected="true"]')?.textContent?.trim()).toBe('Files')
    expect(document.activeElement?.textContent?.trim()).toBe('Files')
  })

  test('right viewport becomes a stacked tab surface in narrow mode', async () => {
    installShell(shellHandlers())
    await withAct(() => useAppStore.setState({ activeView: 'progress', openViews: ['progress'], railCollapsed: false } as never))
    mounted = await mount(<RightViewport narrow />)
    const rail = document.querySelector<HTMLElement>('[data-narrow-right-rail]')
    expect(rail).not.toBeNull()
    expect(rail?.className).toContain('min-w-0')
    expect(rail?.className).toContain('overflow-hidden')
    expect([...document.querySelectorAll('[role="tab"]')].map((tab) => tab.textContent?.trim())).toEqual(['Chat', 'Files', 'Preview', 'Tools'])
    for (const tab of document.querySelectorAll<HTMLElement>('[role="tab"]')) {
      const controls = tab.getAttribute('aria-controls')
      expect(controls && document.getElementById(controls)).not.toBeNull()
    }
  })
})
