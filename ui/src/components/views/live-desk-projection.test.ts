import { describe, expect, test } from 'bun:test'
import type { AgentCard } from '@/lib/agent-card'
import { projectLiveDesk } from './live-desk-projection'

function card(overrides: Partial<AgentCard> = {}): AgentCard {
  return {
    status: 'idle',
    awaitingInput: false,
    steps: [],
    files: [],
    tests: [],
    conflicts: [],
    handoffs: [],
    ...overrides,
  }
}

describe('Live Desk summary projection', () => {
  test('shows measured session state and uses the conversation title', () => {
    const projection = projectLiveDesk({
      title: 'Plan a science fair project',
      sessionStatus: 'running',
      card: card(),
    })

    expect(projection).toMatchObject({
      title: 'Plan a science fair project',
      status: 'working',
      statusLabel: 'Working',
      filesTouched: 0,
      checksReported: 0,
    })
  })

  test('prioritizes a user wait over a still-running card', () => {
    const projection = projectLiveDesk({
      sessionStatus: 'action-required',
      card: card({ status: 'running', awaitingInput: true }),
    })

    expect(projection.status).toBe('waiting')
  })

  test('keeps an outstanding attention item visible even if it is outside the recent activity list', () => {
    const projection = projectLiveDesk({
      sessionStatus: 'running',
      card: card({ status: 'running' }),
      needsAttention: true,
    })

    expect(projection.status).toBe('waiting')
  })

  test('preserves scheduled and reconnecting states instead of implying idle', () => {
    expect(projectLiveDesk({ sessionStatus: 'scheduled', card: card() }).status).toBe('scheduled')
    expect(projectLiveDesk({ sessionStatus: 'reconnecting', card: card() }).status).toBe('reconnecting')
  })

  test('labels check counts as worker-reported, not independently verified', () => {
    const projection = projectLiveDesk({
      sessionStatus: 'completed',
      card: card({
        status: 'completed',
        tests: [
          { name: 'unit checks', passed: true },
          { name: 'browser check', passed: false },
        ],
      }),
    })

    expect(projection.status).toBe('finished')
    expect(projection.checksReported).toBe(2)
    expect(projection.checksPassed).toBe(1)
    expect(projection.checksFailed).toBe(1)
  })

  test('does not invent counts when the journal has no result events', () => {
    const projection = projectLiveDesk({ card: card() })

    expect(projection.filesTouched).toBe(0)
    expect(projection.checksReported).toBe(0)
    expect(projection.checksPassed).toBe(0)
    expect(projection.checksFailed).toBe(0)
  })
})
