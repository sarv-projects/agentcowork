// P11.6.2 — NPS prompt-timing tests (pure logic, injected storage).

import { describe, expect, test } from 'bun:test'
import {
  FIRST_PROMPT_AFTER_MS,
  RE_PROMPT_AFTER_MS,
  npsRecordScore,
  npsShouldPrompt,
  npsScores,
  type NpsStorage,
} from './nps'

function memStorage(): NpsStorage {
  const m = new Map<string, string>()
  return {
    get: (k) => m.get(k) ?? null,
    set: (k, v) => void m.set(k, v),
  }
}

const DAY = 86_400_000

describe('npsShouldPrompt', () => {
  test('stamps first-seen and does not prompt before 7 days', () => {
    const s = memStorage()
    expect(npsShouldPrompt(0, s)).toBe(false)
    expect(s.get('agentcowork.nps.first-seen')).toBe('0')
    // 6 days later: still too early.
    expect(npsShouldPrompt(6 * DAY, s)).toBe(false)
  })

  test('prompts exactly at the 7-day boundary and after', () => {
    const s = memStorage()
    npsShouldPrompt(0, s)
    expect(npsShouldPrompt(7 * DAY, s)).toBe(true)
    // Another fresh storage at 7 days + 1ms also prompts.
    const s2 = memStorage()
    npsShouldPrompt(0, s2)
    expect(npsShouldPrompt(FIRST_PROMPT_AFTER_MS + 1, s2)).toBe(true)
  })

  test('does not re-prompt within the 90-day window after a score', () => {
    const s = memStorage()
    npsShouldPrompt(0, s)
    expect(npsShouldPrompt(7 * DAY, s)).toBe(true)
    npsRecordScore(9, '', 7 * DAY, s)
    // 8 days later — inside the 90-day window — no prompt.
    expect(npsShouldPrompt(8 * DAY, s)).toBe(false)
    // 89 days after the prompt — still inside.
    expect(npsShouldPrompt(7 * DAY + 89 * DAY, s)).toBe(false)
  })

  test('re-prompts once the 90-day window has elapsed', () => {
    const s = memStorage()
    npsShouldPrompt(0, s)
    npsShouldPrompt(7 * DAY, s)
    npsRecordScore(8, '', 7 * DAY, s)
    expect(npsShouldPrompt(7 * DAY + RE_PROMPT_AFTER_MS + 1, s)).toBe(true)
  })

  test('records scores with a 20-entry cap and restarts the window', () => {
    const s = memStorage()
    npsShouldPrompt(0, s)
    for (let i = 0; i < 25; i++) {
      npsRecordScore(7, `c${i}`, i, s)
    }
    const scores = npsScores(s)
    expect(scores.length).toBe(20)
    expect(scores[0]!.comment).toBe('c5')
    expect(scores[19]!.comment).toBe('c24')
  })
})
