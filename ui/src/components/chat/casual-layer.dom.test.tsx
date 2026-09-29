import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import type { ReactElement } from 'react'
import {
  click,
  installShell,
  mount,
  registerDom,
  removeShell,
  tick,
  unregisterDom,
  withAct,
  type Mounted,
} from '@/test/dom-harness'
import { useAppStore, type ChatMessage, type MCQInterrupt, type ToolCallRecord } from '@/lib/store'

let ToolChips: (props: { calls: ToolCallRecord[] }) => ReactElement | null
let McqInterruptCard: (props: { mcq: MCQInterrupt }) => ReactElement
let MessageBubble: (props: { message: ChatMessage; streaming?: boolean }) => ReactElement
let mounted: Mounted

beforeAll(async () => {
  registerDom()
  ToolChips = (await import('./tool-chip')).default
  McqInterruptCard = (await import('./mcq-interrupt-card')).default
  MessageBubble = (await import('./message-bubble')).default
})

afterAll(() => {
  unregisterDom()
})

beforeEach(async () => {
  installShell()
  await withAct(() => useAppStore.setState({ powerMode: false, taskSnapshot: undefined }))
})

afterEach(() => {
  mounted?.unmount()
  removeShell()
})

describe('casual tool activity', () => {
  test('leads with a sentence and keeps raw activity behind Technical details', async () => {
    mounted = await mount(
      <ToolChips
        calls={[
          {
            id: 'tc-1',
            toolId: 'fs.read',
            args: { path: '/private/report.txt' },
            result: 'partial output that must remain available',
            risk: 'high',
            status: 'done',
            startedAt: 1_000,
            endedAt: 3_000,
          },
        ]}
      />,
    )
    await tick()

    const before = mounted.container.textContent ?? ''
    expect(before).toContain('Finished reading your files')
    expect(before).toContain('2s')
    expect(before).not.toContain('fs.read')
    expect(before).not.toContain('/private/report.txt')
    expect(before).not.toContain('partial output that must remain available')
    expect(before).not.toContain('high')

    const disclosure = mounted.container.querySelector<HTMLButtonElement>(
      'button[aria-label*="technical details"]',
    )
    expect(disclosure).not.toBeNull()
    expect(disclosure?.getAttribute('aria-expanded')).toBe('false')
    await click(disclosure!)

    const after = mounted.container.textContent ?? ''
    expect(after).toContain('fs.read')
    expect(after).toContain('/private/report.txt')
    expect(after).toContain('partial output that must remain available')
    expect(disclosure?.getAttribute('aria-expanded')).toBe('true')
  })
})

describe('partial assistant output', () => {
  test('keeps the partial answer while raw error/request data waits for disclosure', async () => {
    mounted = await mount(
      <MessageBubble
        message={{
          id: 'message-error',
          role: 'assistant',
          content: 'The draft is ready, but the final check stopped.',
          timestamp: new Date().toISOString(),
          error: {
            layer: 'tool',
            code: 'RAW_ERROR_CODE',
            detail: 'raw diagnostic text',
            requestId: 'raw-request-id',
            retryable: false,
          },
        }}
      />,
    )
    await tick()

    const before = mounted.container.textContent ?? ''
    expect(before).toContain('The draft is ready, but the final check stopped.')
    expect(before).not.toContain('raw diagnostic text')
    expect(before).not.toContain('raw-request-id')
    expect(before).not.toContain('RAW_ERROR_CODE')

    const disclosure = mounted.container.querySelector<HTMLButtonElement>(
      'button[aria-label="Show technical details"]',
    )
    expect(disclosure).not.toBeNull()
    await click(disclosure!)
    const after = mounted.container.textContent ?? ''
    expect(after).toContain('raw diagnostic text')
    expect(after).toContain('raw-request-id')
    expect(after).toContain('RAW_ERROR_CODE')
  })
})

describe('assistant Markdown rendering', () => {
  test('renders inline and display math through the bounded accessible KaTeX path', async () => {
    mounted = await mount(
      <MessageBubble
        message={{
          id: 'message-math',
          role: 'assistant',
          content: 'Inline $x^2$ and display:\n\n$$\\frac{a}{b}$$',
          timestamp: new Date().toISOString(),
        }}
      />,
    )
    await tick()

    expect(mounted.container.querySelectorAll('.katex')).toHaveLength(2)
    expect(mounted.container.querySelector('.katex-mathml math')).not.toBeNull()
    expect(mounted.container.textContent).toContain('Inline')
  })

  test('keeps an invalid equation readable with a local fallback label', async () => {
    mounted = await mount(
      <MessageBubble
        message={{
          id: 'message-bad-math',
          role: 'assistant',
          content: 'Valid color: $\\color{#cc0000}{x}$; this expression failed: $\\unknowncommand{x}$',
          timestamp: new Date().toISOString(),
        }}
      />,
    )
    await tick()

    const fallback = mounted.container.querySelector('[role="note"]')
    expect(fallback?.textContent).toContain('Math could not be rendered.')
    expect(fallback?.textContent).toContain('\\unknowncommand')
    expect(mounted.container.querySelectorAll('[role="note"]')).toHaveLength(1)
    expect(mounted.container.textContent).toContain('Valid color:')
  })

  test('renders GFM tables in a labelled keyboard-scrollable region', async () => {
    mounted = await mount(
      <MessageBubble
        message={{
          id: 'message-table',
          role: 'assistant',
          content: '| Month | Revenue |\n|:--|--:|\n| April | $12,400 |\n| May | $15,100 |',
          timestamp: new Date().toISOString(),
        }}
      />,
    )
    await tick()

    const region = mounted.container.querySelector('[role="region"][tabindex="0"]')
    expect(region?.getAttribute('aria-label')).toContain('Scroll horizontally')
    expect(region?.querySelectorAll('th')).toHaveLength(2)
    expect(region?.querySelectorAll('tbody tr')).toHaveLength(2)
    expect(region?.textContent).toContain('$15,100')
  })

  test('routes web links through the in-app browser and leaves non-web links inert', async () => {
    mounted = await mount(
      <MessageBubble
        message={{
          id: 'message-links',
          role: 'assistant',
          content: '[safe](https://example.com/research) [unsafe](javascript:alert(1)) [local](file:///private/report.pdf)',
          timestamp: new Date().toISOString(),
        }}
      />,
    )
    await tick()

    const safe = mounted.container.querySelector<HTMLAnchorElement>('a[href="https://example.com/research"]')
    expect(safe).not.toBeNull()
    expect(safe?.getAttribute('target')).toBeNull()
    expect(mounted.container.textContent).toContain('unsafe(link unavailable)')
    expect(mounted.container.textContent).toContain('local(link unavailable)')

    await click(safe!)
    expect(useAppStore.getState().browserUrl).toBe('https://example.com/research')
    expect(useAppStore.getState().activeView).toBe('browse')
  })

  test('links inline citation markers to the source list for that exact message', async () => {
    mounted = await mount(
      <MessageBubble
        message={{
          id: 'message-citations',
          role: 'assistant',
          content: 'The report cites https://example.com/report.',
          timestamp: new Date().toISOString(),
          citations: [{ index: 1, title: 'Annual report', url: 'https://example.com/report' }],
        }}
      />,
    )
    await tick()

    const marker = mounted.container.querySelector<HTMLAnchorElement>('a[href="#cite-message-citations-1"]')
    expect(marker?.textContent).toBe('[^1]')
    expect(mounted.container.querySelector('#cite-message-citations-1')?.textContent).toContain('Annual report')
    expect(mounted.container.querySelector('#cite-1')).toBeNull()
    expect(useAppStore.getState().browserUrl).not.toBe('https://example.com/report')
  })

  test('shows remote images in an accessible placeholder and does not fetch until the user opens Browse', async () => {
    mounted = await mount(
      <MessageBubble
        message={{
          id: 'message-images',
          role: 'assistant',
          content: '![A red bicycle by the river](https://images.example.test/bicycle.jpg "Bicycle")',
          timestamp: new Date().toISOString(),
        }}
      />,
    )
    await tick()

    expect(mounted.container.querySelector('img')).toBeNull()
    expect(mounted.container.querySelector('p figure')).toBeNull()
    expect(mounted.container.querySelector('p p, p div, p figure, p button')).toBeNull()
    expect(mounted.container.textContent).toContain('A red bicycle by the river')
    expect(mounted.container.textContent).toContain('Bicycle')
    expect(useAppStore.getState().browserUrl).not.toBe('https://images.example.test/bicycle.jpg')

    const open = Array.from(mounted.container.querySelectorAll<HTMLButtonElement>('button')).find((button) =>
      button.textContent?.includes('Open image in Browse'),
    )
    expect(open).toBeDefined()
    await click(open!)
    expect(useAppStore.getState().browserUrl).toBe('https://images.example.test/bicycle.jpg')
    expect(useAppStore.getState().activeView).toBe('browse')
  })

  test('does not execute raw HTML or load an unsafe Markdown image URL', async () => {
    mounted = await mount(
      <MessageBubble
        message={{
          id: 'message-untrusted-markdown',
          role: 'assistant',
          content: '<img src="https://evil.example/track" onerror="alert(1)">\n\n![unsafe](javascript:alert(1))',
          timestamp: new Date().toISOString(),
        }}
      />,
    )
    await tick()

    expect(mounted.container.querySelector('img')).toBeNull()
    expect(mounted.container.querySelector('[onerror]')).toBeNull()
    expect(mounted.container.textContent).toContain('This image reference can’t be opened safely')
    expect(mounted.container.querySelector('button')?.textContent).not.toContain('Open image in Browse')
  })

  test('artifact cards do not invent preview contents for files without a real thumbnail', async () => {
    mounted = await mount(
      <MessageBubble
        message={{
          id: 'message-artifact',
          role: 'assistant',
          content: 'The workbook is ready.',
          timestamp: new Date().toISOString(),
          artifacts: [{
            id: 'artifact-sales',
            name: 'Sales review.xlsx',
            type: 'xlsx',
            preview: 'Q2 source reconciled · Summary sheet updated',
          }],
        }}
      />,
    )
    await tick()

    expect(mounted.container.textContent).toContain('Spreadsheet')
    expect(mounted.container.textContent).toContain('Q2 source reconciled · Summary sheet updated')
    expect(mounted.container.textContent).not.toContain('$1.8M')
    expect(mounted.container.textContent).not.toContain('1_800_000')
  })

  test('saving an artifact preview exports a .txt preview instead of a fake Office file', async () => {
    mounted = await mount(
      <MessageBubble
        message={{
          id: 'message-save-preview',
          role: 'assistant',
          content: 'Workbook generated.',
          timestamp: new Date().toISOString(),
          artifacts: [{
            id: 'artifact-save-preview',
            name: 'Quarterly results.xlsx',
            type: 'xlsx',
            preview: 'Summary sheet updated',
            view: 'office-xlsx',
          }],
        }}
      />,
    )
    await tick()

    let savedName = ''
    let savedBlob: Blob | undefined
    const originalCreateObjectURL = URL.createObjectURL
    const originalAnchorClick = HTMLAnchorElement.prototype.click
    URL.createObjectURL = (blob) => {
      savedBlob = blob
      return 'blob:preview-test'
    }
    HTMLAnchorElement.prototype.click = function () {
      savedName = this.download
    }
    try {
      const save = Array.from(mounted.container.querySelectorAll<HTMLButtonElement>('button')).find((button) =>
        button.textContent?.includes('Save preview'),
      )
      expect(save?.title).toContain('does not export the original artifact')
      await click(save!)
      expect(savedName).toBe('Quarterly results.xlsx.preview.txt')
      expect(savedBlob?.type).toBe('text/plain;charset=utf-8')
      expect(await savedBlob?.text()).toBe('Summary sheet updated')
    } finally {
      URL.createObjectURL = originalCreateObjectURL
      HTMLAnchorElement.prototype.click = originalAnchorClick
    }
  })

  test('an Office artifact without a file reference does not open its display name as a path', async () => {
    mounted = await mount(
      <MessageBubble
        message={{
          id: 'message-unresolved-artifact',
          role: 'assistant',
          content: 'The workbook is ready.',
          timestamp: new Date().toISOString(),
          artifacts: [{
            id: 'artifact-unresolved-sales',
            name: 'Sales review.xlsx',
            type: 'xlsx',
            preview: 'Summary sheet updated',
          }],
        }}
      />,
    )
    await tick()

    expect(mounted.container.textContent).toContain('File reference or viewer is not available yet')
    const open = Array.from(mounted.container.querySelectorAll<HTMLButtonElement>('button')).find((button) =>
      button.textContent?.trim() === 'Open',
    )
    expect(open).toBeDefined()
    await click(open!)
    expect(useAppStore.getState().officePaths['office-xlsx']).toBeUndefined()
    expect(useAppStore.getState().lastToast).toContain('no available file location or viewer')
  })
})

describe('inline consent', () => {
  test('names the Guard facts and keeps a real deny path', async () => {
    let answer: string | undefined
    await withAct(() =>
      useAppStore.setState({
        respondMcq: (_id: string, choice: string) => {
          answer = choice
        },
      }),
    )
    const mcq: MCQInterrupt = {
      id: 'consent-1',
      title: 'Update the project brief',
      description: 'Replace the draft paragraph after checking the project brief.',
      kind: 'permission',
      approvalNonce: 'nonce-raw-value',
    }
    mounted = await mount(<McqInterruptCard mcq={mcq} />)
    await tick()

    const body = mounted.container.textContent ?? ''
    expect(body).toContain('Action')
    expect(body).toContain('Data / resource')
    expect(body).toContain('Scope / recipient')
    expect(body).toContain('Reversibility')
    expect(body).toContain('Guard is the approval authority')
    expect(body).toContain('Deny')
    expect(body).not.toContain('nonce-raw-value')

    const approve = Array.from(mounted.container.querySelectorAll('button')).find((button) =>
      (button.textContent ?? '').trim() === 'Approve',
    )
    const deny = Array.from(mounted.container.querySelectorAll('button')).find((button) =>
      (button.textContent ?? '').trim() === 'Deny',
    )
    expect(approve).toBeDefined()
    expect(deny).toBeDefined()
    await click(approve!)
    expect(answer).toBe('approve')
    await click(deny!)
    expect(answer).toBe('reject')

    const technical = mounted.container.querySelector<HTMLButtonElement>(
      'button[aria-label*="technical details"]',
    )
    expect(technical).not.toBeNull()
    await click(technical!)
    expect(mounted.container.textContent ?? '').toContain('nonce-raw-value')
  })
})
