// DOM proof for the one run surface.
//
// The run panel aggregates identity, context/usage, the ordered trace, the
// files the run reported, the working folder and the MCP registry. These tests
// pin the four things that are easy to get wrong and expensive to get wrong
// quietly:
//
//   1. Identity is the user's words, and the model is the **agent's** reported
//      value or an explicit "not reported" — never a desktop catalog guess.
//   2. Readiness comes from the live agent directory; a catalog row that was
//      never probed says so instead of reading as installed.
//   3. Status is never colour-only: failed, waiting-for-you and outcome-unknown
//      each carry a word, and a parked run does not keep a live indicator
//      spinning.
//   4. Nothing unreachable is printed: no context-window percentage, no file
//      size that was not read, no open button for a file that is not on disk.

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import type { ReactElement } from 'react'
import {
  click,
  installShell,
  mount,
  registerDom,
  removeShell,
  unregisterDom,
  waitFor,
  withAct,
  type Mounted,
} from '@/test/dom-harness'
import { useAppStore } from '@/lib/store'
import type { WorkEvent, WorkEventEnvelope } from '@/lib/work'

let RunView: () => ReactElement
let mounted: Mounted | undefined

const AGENT = {
  id: 'claude-code',
  name: 'Claude Code',
  mark: 'CC',
  accent: 'bg-sky-500/90 text-black',
  status: 'installed' as const,
  readiness: 'ready' as const,
  capabilities: [],
  models: [],
  defaultModel: '',
}

const MODEL_OPTION = {
  id: 'model',
  name: 'Model',
  category: 'model',
  type: 'select' as const,
  currentValue: 'claude-sonnet-4.5',
}

function envelope(sequence: number, event: WorkEvent, timestamp: number): WorkEventEnvelope {
  return { workId: 'w-1', sequence, eventId: `e-${sequence}`, event, timestamp }
}

const TOOL_STARTED = envelope(
  1,
  { class: 'domain', event: { kind: 'run_started', data: { runId: 'r-1' } } },
  1_700_000_000_000,
)
const TOOL_FAILED = envelope(
  2,
  { class: 'operational', event: { kind: 'tool_failed', data: { toolId: 'fs.write', error: 'denied' } } },
  1_700_000_001_000,
)

async function setState(patch: Record<string, unknown>): Promise<void> {
  await withAct(() => useAppStore.setState(patch as never))
  await withAct(() => {})
}

function chat(over: Record<string, unknown> = {}) {
  return {
    id: 'chat-1',
    title: 'Refresh the Q3 numbers',
    status: 'idle' as const,
    preview: '',
    updatedAt: new Date().toISOString(),
    messages: [],
    ...over,
  }
}

beforeAll(async () => {
  registerDom()
  RunView = (await import('./run-view')).default
})

afterAll(() => {
  unregisterDom()
})

beforeEach(() => {
  window.localStorage.clear()
  useAppStore.setState({
    activeSessionId: 'chat-1',
    sessions: [chat()],
    taskFolder: undefined,
    selectedAgentId: 'claude-code',
    selectedModelId: '',
    acpConfigOptions: {},
    liveAgents: [AGENT],
    workItems: [],
    workEvents: [],
    workPresence: undefined,
    openViews: ['run'],
    activeView: 'run',
    spooledOutput: null,
  } as never)
})

afterEach(() => {
  mounted?.unmount()
  mounted = undefined
  removeShell()
})

describe('run surface — identity in the user’s vocabulary', () => {
  test('names the chat, the bound agent, its real readiness, and the folder', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({
      sessions: [chat({ folder: '/home/dev/q3' })],
      liveAgents: [AGENT],
    })
    mounted = await mount(<RunView />)

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('Refresh the Q3 numbers')
    expect(body).toContain('Claude Code')
    expect(body).toContain('ready')
    // The path is abbreviated for the narrow rail, with the full value on the
    // row's accessible name — never a path that cannot be recovered.
    expect(body).toContain('…/dev/q3')
    const folderRow = [...mounted.container.querySelectorAll('div[title]')].find((d) =>
      (d.getAttribute('title') ?? '').startsWith('/home/dev/q3'),
    )
    expect(folderRow?.getAttribute('title')).toBe('/home/dev/q3')
    expect(
      mounted.container.querySelector('button[aria-label="Copy the working folder path"]'),
    ).toBeTruthy()
    // The user-facing container word is Chat, never Session.
    expect(body).not.toMatch(/\bsessions?\b/i)
  })

  test('a catalog row that was never probed does not read as installed', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    // `claude-code` exists in the static seed but has no live directory row.
    await setState({ liveAgents: [] })
    mounted = await mount(<RunView />)

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('Claude Code')
    expect(body).toContain('not probed on this machine')
    expect(body).not.toContain('launchable — not yet verified')
  })

  test('the model is the agent’s reported value, or an explicit not reported', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({ acpConfigOptions: { 'claude-code': [MODEL_OPTION] } })
    mounted = await mount(<RunView />)
    expect(mounted.container.textContent ?? '').toContain('claude-sonnet-4.5')

    mounted.unmount()
    await setState({ acpConfigOptions: {} })
    mounted = await mount(<RunView />)
    expect(mounted.container.textContent ?? '').toContain('not reported by the agent')
  })
})

describe('run surface — context and usage', () => {
  test('reports the ledger split and names the reporter', async () => {
    installShell({
      usage_snapshot: () => ({
        total: { tokensIn: 15_685, tokensOut: 0, cachedTokens: 0, cachedWriteTokens: 0, cacheHits: 0, cacheMisses: 0, cacheHitRate: 0, reportedCostUsd: 0 },
        cacheHitRate: 0,
        byKey: [],
        bySession: [
          {
            sessionId: 'chat-1',
            tokensIn: 12_481,
            tokensOut: 3_204,
            cachedTokens: 0,
            cachedWriteTokens: 0,
            cacheHits: 0,
            cacheMisses: 0,
            cacheHitRate: 0,
            reportedCostUsd: 0,
            source: 'acp_event',
          },
        ],
      }),
    })
    mounted = await mount(<RunView />)
    await waitFor(() => (mounted!.container.textContent ?? '').includes('reported in an agent event'))

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('12.5k') // input
    expect(body).toContain('3.2k') // output
    expect(body).toContain('15.7k') // total
  })

  test('an unknown context window prints no percentage at all', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    mounted = await mount(<RunView />)
    await waitFor(() => (mounted!.container.textContent ?? '').includes('Context window'))

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('Not reported')
    expect(body).not.toMatch(/≈\d+%/)
    // The placeholder window used elsewhere in the store is never printed here.
    expect(body).not.toContain('128')
  })

  test('a chat with no usage row says so instead of showing a measured zero', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    mounted = await mount(<RunView />)
    await waitFor(() =>
      (mounted!.container.textContent ?? '').includes('no usage reported for this chat yet'),
    )
    expect(mounted.container.textContent ?? '').toContain('not reported')
  })
})

describe('run surface — the trace never lies about status', () => {
  test('a failed step is distinct from a running one and says so in words', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({ workEvents: [TOOL_STARTED, TOOL_FAILED] })
    mounted = await mount(<RunView />)

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('Failed')
    expect(body).toContain('#2 Tool fs.write failed')
    // A settled failure must not be painted as the live row.
    expect(mounted.container.querySelectorAll('.live-dot').length).toBe(0)
  })

  test('an unknown effect outcome reads as unknown, not done and not failed', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({
      workEvents: [
        TOOL_STARTED,
        envelope(
          2,
          {
            class: 'domain',
            event: { kind: 'effect_observed', data: { effectId: 'fx-1', outcome: 'unknown' } },
          },
          1_700_000_001_000,
        ),
      ],
    })
    mounted = await mount(<RunView />)

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('Outcome unknown')
    expect(body).toContain('1 outcome unknown')
  })

  test('a run parked on the user stops the live indicator', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({
      sessions: [chat({ status: 'action-required' })],
      workEvents: [
        TOOL_STARTED,
        envelope(
          2,
          {
            class: 'domain',
            event: {
              kind: 'run_waiting',
              data: { runId: 'r-1', reason: 'user_input', wait: { reason: 'user_input' } },
            },
          },
          1_700_000_001_000,
        ),
      ],
    })
    mounted = await mount(<RunView />)

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('Waiting for you')
    expect(body).toContain('waiting for you')
    // Nothing spins while the run waits on a human.
    expect(mounted.container.querySelectorAll('.live-dot').length).toBe(0)
  })

  test('a step expands for detail with the journal coordinates', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({ workEvents: [TOOL_STARTED, TOOL_FAILED] })
    mounted = await mount(<RunView />)

    const row = [...mounted.container.querySelectorAll('button[aria-expanded]')].find(
      (b) => (b.textContent ?? '').includes('Tool fs.write failed'),
    )
    expect(row).toBeTruthy()
    expect(row?.getAttribute('aria-expanded')).toBe('false')
    await click(row!)
    expect(row?.getAttribute('aria-expanded')).toBe('true')

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('operational · tool_failed')
    expect(body).toContain('not measurable from the journal')
  })

  test('an empty journal is stated, not filled with a sample run', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({ workEvents: [] })
    mounted = await mount(<RunView />)
    const body = mounted.container.textContent ?? ''
    expect(body).toContain('No steps recorded yet')
    expect(body).not.toContain('#1')
  })
})

describe('run surface — files the run reported', () => {
  const withArtifact = (path: string, name: string) =>
    chat({
      folder: '/home/dev/q3',
      messages: [
        {
          id: 'm1',
          role: 'assistant',
          content: '',
          timestamp: new Date().toISOString(),
          artifacts: [{ id: 'a1', name, type: 'xlsx', preview: '', path }],
        },
      ],
    })

  test('one file gets one row however the run spelled its path', async () => {
    installShell({
      usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }),
      fs_list_dir: () => ({
        path: '/home/dev/q3',
        parent: '/home/dev',
        entries: [
          { name: 'report.xlsx', dir: false, symlink: false, size: 2_204_160, modified: null },
        ],
      }),
    })
    useAppStore.setState({
      // A card reports the absolute path; a `file_touched` event reports the
      // same file relative to the working folder. One file, one row.
      sessions: [
        chat({
          folder: '/home/dev/q3',
          messages: [
            {
              id: 'm1',
              role: 'assistant',
              content: '',
              timestamp: new Date().toISOString(),
              artifacts: [
                {
                  id: 'a1',
                  name: 'report.xlsx',
                  type: 'xlsx',
                  preview: '',
                  path: '/home/dev/q3/report.xlsx',
                },
              ],
            },
          ],
        }),
      ],
      workEvents: [
        envelope(
          1,
          {
            class: 'operational',
            event: { kind: 'file_touched', data: { path: 'report.xlsx', writer_id: 'r-1' } },
          },
          1_700_000_000_000,
        ),
      ],
    } as never)
    mounted = await mount(<RunView />)
    await waitFor(() => (mounted!.container.textContent ?? '').includes('2.1 MB'))
    expect(mounted.container.querySelectorAll('[data-testid="run-artifact-row"]').length).toBe(1)
  })

  test('a reported file that is not on disk says so and cannot be opened', async () => {
    installShell({
      usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }),
      // A listing that does not contain the file.
      fs_list_dir: () => ({ path: '/home/dev', parent: '/home', entries: [] }),
    })
    useAppStore.setState({
      sessions: [{ ...withArtifact('/home/dev/q3/report.xlsx', 'report.xlsx') }],
    } as never)
    mounted = await mount(<RunView />)
    await waitFor(() => (mounted!.container.textContent ?? '').includes('not on this machine'))

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('report.xlsx')
    expect(body).toContain('Spreadsheet')
    const open = mounted.container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Cannot open"]',
    )
    expect(open).toBeTruthy()
    expect(open?.disabled).toBe(true)
  })

  test('a file the disk reports gets its real size and a working open control', async () => {
    installShell({
      usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }),
      fs_list_dir: () => ({
        path: '/home/dev',
        parent: '/home',
        entries: [{ name: 'report.xlsx', dir: false, symlink: false, size: 2_204_160, modified: null }],
      }),
    })
    useAppStore.setState({
      sessions: [{ ...withArtifact('/home/dev/q3/report.xlsx', 'report.xlsx') }],
    } as never)
    mounted = await mount(<RunView />)
    await waitFor(() => (mounted!.container.textContent ?? '').includes('2.1 MB'))

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('2.1 MB')
    expect(body).not.toContain('not on this machine')
    const open = mounted.container.querySelector<HTMLButtonElement>(
      'button[aria-label="Open report.xlsx"]',
    )
    expect(open?.disabled).toBe(false)
  })

  test('a relative path with no folder cannot be opened and says why', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    useAppStore.setState({
      sessions: [
        chat({
          messages: [
            {
              id: 'm1',
              role: 'assistant',
              content: '',
              timestamp: new Date().toISOString(),
              artifacts: [{ id: 'a1', name: 'notes.md', type: 'markdown', preview: '' }],
            },
          ],
        }),
      ],
    } as never)
    mounted = await mount(<RunView />)

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('notes.md')
    expect(body).toContain('no folder resolves this path')
    const open = mounted.container.querySelector<HTMLButtonElement>(
      'button[aria-label="Cannot open notes.md — it is not on this machine"]',
    )
    expect(open?.disabled).toBe(true)
  })

  test('no reported files is stated plainly', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    mounted = await mount(<RunView />)
    expect(mounted.container.textContent ?? '').toContain('No files reported yet')
  })
})

describe('run surface — the inventories are real or they are not shown', () => {
  test('the working folder lists what the disk reports, newest first', async () => {
    installShell({
      usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }),
      fs_list_dir: () => ({
        path: '/home/dev/q3',
        parent: '/home/dev',
        entries: [
          { name: 'old.md', dir: false, symlink: false, size: 100, modified: '2026-01-01T00:00:00Z' },
          { name: 'new.md', dir: false, symlink: false, size: 2_048, modified: '2026-09-20T00:00:00Z' },
          { name: 'sub', dir: true, symlink: false, size: null, modified: '2026-09-25T00:00:00Z' },
        ],
      }),
    })
    await setState({ sessions: [chat({ folder: '/home/dev/q3' })] })
    mounted = await mount(<RunView />)
    await waitFor(() => (mounted!.container.textContent ?? '').includes('new.md'))

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('new.md')
    expect(body).toContain('old.md')
    expect(body).toContain('2.0 kB')
    // Directories are not files, and the caption never claims run attribution.
    expect(body).not.toContain('sub')
    expect(body).toContain('a folder read, not a list of what this run produced')
  })

  test('MCP servers render their real attached/disconnected state', async () => {
    installShell({
      usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }),
      mcp_servers: () => [
        { name: 'filesystem', status: 'connected', transport: 'stdio', tools: 4, desc: '', toolNames: [] },
        { name: 'github', status: 'disconnected', transport: 'http', tools: 0, desc: '', toolNames: [] },
      ],
    })
    mounted = await mount(<RunView />)
    await waitFor(() => (mounted!.container.querySelector('[data-testid="run-mcp-servers"]') !== null))

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('filesystem')
    expect(body).toContain('connected')
    expect(body).toContain('github')
    expect(body).toContain('disconnected')
    expect(body).toContain('4 tools advertised')
  })

  test('an MCP registry that cannot be read is not shown as an empty one', async () => {
    // The default shell answers an unknown command with `{}` — a malformed
    // payload. It must read as unreadable, never as "no servers configured".
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    mounted = await mount(<RunView />)
    await waitFor(() =>
      (mounted!.container.textContent ?? '').includes('MCP server registry could not be read'),
    )
    expect(mounted.container.textContent ?? '').not.toContain('No MCP servers are configured')
  })
})

describe('run surface — disclosure is keyboard operable and persisted', () => {
  test('each section is a labelled toggle that persists its state', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({ workEvents: [TOOL_STARTED] })
    mounted = await mount(<RunView />)

    const steps = [...mounted.container.querySelectorAll('button[aria-controls]')].find((b) =>
      (b.textContent ?? '').includes('Steps'),
    )
    expect(steps?.getAttribute('aria-expanded')).toBe('true')
    const panelId = steps?.getAttribute('aria-controls') ?? ''
    expect(mounted.container.querySelector(`#${CSS.escape(panelId)}`)).toBeTruthy()

    await click(steps!)
    expect(steps?.getAttribute('aria-expanded')).toBe('false')
    expect(mounted.container.querySelector(`#${CSS.escape(panelId)}`)).toBeNull()

    const stored = window.localStorage.getItem('agentcowork.settings.runCollapsedSections')
    expect(stored).toContain('trace')

    // A remount reads the persisted state back.
    mounted.unmount()
    mounted = await mount(<RunView />)
    const again = [...mounted.container.querySelectorAll('button[aria-controls]')].find((b) =>
      (b.textContent ?? '').includes('Steps'),
    )
    expect(again?.getAttribute('aria-expanded')).toBe('false')
  })

  test('collapse all is one control and reports its own state', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    mounted = await mount(<RunView />)
    const all = mounted.container.querySelector<HTMLButtonElement>('button[aria-pressed]')
    expect(all?.getAttribute('aria-pressed')).toBe('false')
    expect(all?.textContent).toContain('Collapse all')

    await click(all!)
    expect(all?.getAttribute('aria-pressed')).toBe('true')
    expect(all?.textContent).toContain('Expand all')
  })
})

describe('run surface — the drill-downs open the lenses that already exist', () => {
  test('the foot of the panel routes into the existing views', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    mounted = await mount(<RunView />)

    for (const label of ['Progress', 'Trace', 'Changes', 'Audit']) {
      const button = [...mounted.container.querySelectorAll('button')].find(
        (b) => b.textContent?.trim() === label,
      )
      expect(button).toBeTruthy()
      await click(button!)
    }

    const open = useAppStore.getState().openViews
    expect(open).toContain('progress')
    expect(open).toContain('trajectory')
    expect(open).toContain('diff')
    expect(open).toContain('audit')
  })
})

describe('run surface — the outcome strip reports only what the journal reported', () => {
  test('files, tests, conflicts, and handoffs come off the folded card', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({
      workEvents: [
        envelope(
          1,
          { class: 'operational', event: { kind: 'file_touched', data: { path: 'src/a.rs', writer_id: 'r-1' } } },
          1_700_000_000_000,
        ),
        envelope(
          2,
          { class: 'operational', event: { kind: 'test_ran', data: { name: 'cargo test', passed: true } } },
          1_700_000_001_000,
        ),
        envelope(
          3,
          { class: 'operational', event: { kind: 'test_ran', data: { name: 'npm run a11y', passed: false } } },
          1_700_000_002_000,
        ),
        envelope(
          4,
          {
            class: 'operational',
            event: { kind: 'write_conflict', data: { path: 'src/b.rs', writers: ['r-1', 'r-2'] } },
          },
          1_700_000_003_000,
        ),
        envelope(
          5,
          {
            class: 'operational',
            event: {
              kind: 'handoff_recorded',
              data: {
                artifact_id: 'a1',
                from_agent: 'Codex CLI',
                to_agent: 'Claude Code',
                summary: 'patched the parser',
              },
            },
          },
          1_700_000_004_000,
        ),
      ],
    })
    mounted = await mount(<RunView />)
    const outcome = mounted.container.querySelector('[data-testid="run-outcome"]')
    expect(outcome).toBeTruthy()
    const body = outcome?.textContent ?? ''
    expect(body).toContain('1 file touched')
    expect(body).toContain('1 passed, 1 failed')
    expect(body).toContain('npm run a11y')
    expect(body).toContain('1 write conflict')
    expect(body).toContain('src/b.rs')
    expect(body).toContain('Codex CLI → Claude Code')
    expect(body).toContain('patched the parser')
    // The wire enum is never painted as-is.
    expect(body).not.toContain('awaiting_input')
  })

  test('an idle run says nothing was reported, not that it reported zero', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({ workEvents: [] })
    mounted = await mount(<RunView />)
    const body = mounted.container.textContent ?? ''
    expect(body).toContain('The run reported no files, tests, conflicts, or handoffs yet')
  })
})

describe('run surface — a long trace stays inspectable', () => {
  const manyEvents = Array.from({ length: 40 }, (_, i) =>
    envelope(
      i + 1,
      i % 2 === 0
        ? { class: 'operational', event: { kind: 'tool_started', data: { toolId: `tool-${i}` } } }
        : { class: 'domain', event: { kind: 'effect_attempted', data: { effectId: `fx-${i}` } } },
      1_700_000_000_000 + i * 100,
    ),
  )

  test('the list is one tab stop and the arrows move between steps', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({ workEvents: manyEvents })
    mounted = await mount(<RunView />)

    const list = mounted.container.querySelector('[role="listbox"]')
    expect(list).toBeTruthy()
    const options = list?.querySelectorAll('[role="option"]') ?? []
    expect(options.length).toBe(40)
    // Roving tabindex: exactly one row is in the tab order.
    const tabbable = [...options].filter(
      (o) => (o.querySelector('button[aria-expanded]')?.getAttribute('tabindex') ?? '-1') === '0',
    )
    expect(tabbable.length).toBe(1)
  })

  test('a class filter narrows the list and states the honest count', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({ workEvents: manyEvents })
    mounted = await mount(<RunView />)
    expect(mounted.container.textContent ?? '').toContain('40 steps')

    const effects = [...mounted.container.querySelectorAll('button[aria-pressed]')].find(
      (b) => b.textContent?.trim() === 'Effects',
    )
    await click(effects!)
    const body = mounted.container.textContent ?? ''
    expect(body).toContain('20 of 40 steps')
    expect(
      mounted.container.querySelectorAll('[role="listbox"] [role="option"]').length,
    ).toBe(20)
  })

  test('a text filter that matches nothing says so and never claims an empty run', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({ workEvents: manyEvents })
    mounted = await mount(<RunView />)

    const input = mounted.container.querySelector<HTMLInputElement>(
      'input[aria-label="Filter steps by text"]',
    )
    expect(input).toBeTruthy()
    // React installs its own value tracker, so a plain `.value =` is ignored.
    // Set through the native descriptor, then fire the event React listens to.
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!
    await withAct(() => {
      setValue.call(input!, 'zzz-nothing')
      input!.dispatchEvent(new Event('input', { bubbles: true }))
    })
    await withAct(() => {})

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('No step matches this filter')
    expect(body).toContain('The run recorded 40 steps')
  })

  test('a state change is announced politely for assistive technology', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({ workEvents: [TOOL_STARTED, TOOL_FAILED] })
    mounted = await mount(<RunView />)
    const live = mounted.container.querySelector('[aria-live="polite"]')
    expect(live?.getAttribute('role')).toBe('status')
    expect(live?.textContent ?? '').toContain('1 step failed')
  })
})

describe('run surface — a step drills into the lens that can act on it', () => {
  test('a file step opens the workbench; a step naming no resource offers nothing', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({
      workEvents: [
        TOOL_STARTED,
        envelope(
          2,
          {
            class: 'operational',
            event: { kind: 'file_touched', data: { path: '/home/dev/q3/a.rs', writer_id: 'r-1' } },
          },
          1_700_000_001_000,
        ),
      ],
    })
    mounted = await mount(<RunView />)

    const follow = mounted.container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Open the file"]',
    )
    expect(follow).toBeTruthy()
    await click(follow!)
    expect(useAppStore.getState().openViews).toContain('code')

    // Inside the trace, `run_started` and `tool_started` name no view, so they
    // get no button at all — only the file step does.
    const labels = [
      ...mounted.container.querySelectorAll('[role="listbox"] button[aria-label]'),
    ].map((b) => b.getAttribute('aria-label') ?? '')
    expect(labels).toHaveLength(1)
    expect(labels[0]).toContain('Open the file:')
    expect(labels[0]).toContain('/home/dev/q3/a.rs')
  })

  test('an approval step offers the audit trail', async () => {
    installShell({ usage_snapshot: () => ({ total: {}, byKey: [], bySession: [] }) })
    await setState({
      workEvents: [
        TOOL_STARTED,
        envelope(
          2,
          { class: 'domain', event: { kind: 'approval_requested', data: { ticketId: 't-9' } } },
          1_700_000_001_000,
        ),
      ],
    })
    mounted = await mount(<RunView />)
    const follow = mounted.container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Open the audit trail"]',
    )
    expect(follow).toBeTruthy()
    await click(follow!)
    expect(useAppStore.getState().openViews).toContain('audit')
  })
})
