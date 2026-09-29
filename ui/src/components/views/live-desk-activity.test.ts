import { describe, expect, test } from 'bun:test'
import type { WorkEvent, WorkEventEnvelope } from '@/lib/work'
import { hasPendingUserAction, projectLiveDeskUpdates } from './live-desk-activity'

function envelope(sequence: number, event: WorkEvent, timestamp = sequence * 1000): WorkEventEnvelope {
  return {
    workId: 'work-1',
    sequence,
    eventId: `event-${sequence}`,
    event,
    timestamp,
  }
}

describe('Live Desk activity projection', () => {
  test('uses human wording and never infers a tool action from its identifier', () => {
    const updates = projectLiveDeskUpdates([
      envelope(1, { class: 'operational', event: { kind: 'tool_started', data: { toolId: 'gmail.search' } } }),
      envelope(2, { class: 'operational', event: { kind: 'file_touched', data: { path: '/reports/q3.xlsx', writer_id: 'worker-1' } } }),
    ])

    expect(updates).toHaveLength(1)
    expect(updates[0]?.label).toBe('Worked with q3.xlsx')
    expect(JSON.stringify(updates)).not.toContain('gmail')
  })

  test('shows an unresolved approval but suppresses its stale request after resolution', () => {
    const pending = projectLiveDeskUpdates([
      envelope(1, { class: 'domain', event: { kind: 'approval_requested', data: { ticketId: 'ticket-1' } } }),
    ])
    const resolved = projectLiveDeskUpdates([
      envelope(1, { class: 'domain', event: { kind: 'approval_requested', data: { ticketId: 'ticket-1' } } }),
      envelope(2, { class: 'domain', event: { kind: 'approval_resolved', data: { ticketId: 'ticket-1', approved: true } } }),
    ])

    expect(pending.map((update) => update.label)).toEqual(['An action needs your approval'])
    expect(pending[0]?.needsUser).toBe(true)
    expect(resolved.map((update) => update.label)).toEqual(['Approval granted'])
  })

  test('removes a stale user-wait update after the same run resumes', () => {
    const events = [
      envelope(1, {
        class: 'domain',
        event: { kind: 'run_waiting', data: { runId: 'run-1', reason: 'user_input' } },
      }),
      envelope(2, { class: 'domain', event: { kind: 'run_started', data: { runId: 'run-1' } } }),
    ]
    const updates = projectLiveDeskUpdates(events)

    expect(updates).toEqual([])
    expect(hasPendingUserAction(events)).toBe(false)
  })

  test('keeps an older unresolved approval actionable outside the recent-update window', () => {
    const events = [
      envelope(1, { class: 'domain', event: { kind: 'approval_requested', data: { ticketId: 'ticket-1' } } }),
      ...Array.from({ length: 6 }, (_, index) =>
        envelope(index + 2, { class: 'domain', event: { kind: 'run_completed', data: { runId: `run-${index}` } } }),
      ),
    ]

    expect(projectLiveDeskUpdates(events)).toHaveLength(4)
    expect(hasPendingUserAction(events)).toBe(true)
  })

  test('surfaces verifier evidence separately from recorded worker checks', () => {
    const updates = projectLiveDeskUpdates([
      envelope(1, { class: 'operational', event: { kind: 'test_ran', data: { name: 'unit', passed: true } } }),
      envelope(2, { class: 'domain', event: { kind: 'effect_verified', data: { effectId: 'effect-1', verified: true } } }),
    ])

    expect(updates.map((update) => [update.label, update.state])).toEqual([
      ['A change was verified', 'verified'],
      ['A recorded check passed', 'recorded'],
    ])
  })

  test('deduplicates replayed event ids, caps the list and sorts newest first', () => {
    const events = Array.from({ length: 6 }, (_, index) =>
      envelope(index + 1, { class: 'domain', event: { kind: 'run_completed', data: { runId: `run-${index}` } } }),
    )
    events.push({ ...events[5]!, sequence: 7 })

    const updates = projectLiveDeskUpdates(events)

    expect(updates).toHaveLength(4)
    expect(updates.map((update) => update.at)).toEqual([6000, 5000, 4000, 3000])
  })

  test('ignores private reasoning summaries', () => {
    const privateSummary = envelope(1, {
      class: 'presence',
      event: { kind: 'agent_thought_summary', data: { text: 'private reasoning summary' } },
    })

    expect(projectLiveDeskUpdates([privateSummary])).toEqual([])
  })
})
