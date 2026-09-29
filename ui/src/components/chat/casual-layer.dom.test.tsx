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
