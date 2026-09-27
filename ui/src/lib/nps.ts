// P11.6.2 — NPS prompt timing, as pure logic over an injectable storage
// (localStorage in the app, an in-memory shim in tests). Rules: prompt once
// the user has been around FIRST_PROMPT_AFTER_MS (7 days), then at most once
// per RE_PROMPT_AFTER_MS (90 days).

import { readWithFallback } from './storage-compat'

export const DAY_MS = 86_400_000
/** Prompt once the user has been around this long. */
export const FIRST_PROMPT_AFTER_MS = 7 * DAY_MS
/** Don't ask again for this long after the previous prompt. */
export const RE_PROMPT_AFTER_MS = 90 * DAY_MS

export const FIRST_SEEN_KEY = 'agentcowork.nps.first-seen'
export const LAST_PROMPT_KEY = 'agentcowork.nps.last-prompt'
export const SCORES_KEY = 'agentcowork.nps.scores'

/** Storage surface — localStorage in the app, a shim in tests. */
export interface NpsStorage {
  get(key: string): string | null
  set(key: string, value: string): void
}

/** The real adapter used by the component. */
export const localStorageNpsStorage: NpsStorage = {
  get: (key) => {
    try {
      // DEC-053: legacy `everyaios.*` key honored + promoted once.
      return readWithFallback(
        {
          get: (k) => localStorage.getItem(k),
          set: (k, v) => localStorage.setItem(k, v),
        },
        key,
      )
    } catch {
      return null
    }
  },
  set: (key, value) => {
    try {
      localStorage.setItem(key, value)
    } catch {
      /* ignore */
    }
  },
}

/** Should the NPS prompt show now? Stamps first-seen on first call. */
export function npsShouldPrompt(now: number, storage: NpsStorage): boolean {
  const firstRaw = storage.get(FIRST_SEEN_KEY)
  let firstSeen: number
  if (firstRaw === null) {
    firstSeen = now
    storage.set(FIRST_SEEN_KEY, String(now))
  } else {
    const parsed = Number(firstRaw)
    firstSeen = Number.isFinite(parsed) ? parsed : now
  }
  const parsedLast = Number(storage.get(LAST_PROMPT_KEY) ?? 0)
  const lastPrompt = Number.isFinite(parsedLast) ? parsedLast : 0
  if (now - firstSeen < FIRST_PROMPT_AFTER_MS) return false
  // The re-prompt window only applies once a prompt has actually happened
  // (lastPrompt > 0) — otherwise the first eligible moment would be blocked.
  if (lastPrompt > 0 && now - lastPrompt < RE_PROMPT_AFTER_MS) return false
  return true
}

/** Record a score + optional comment; sets last-prompt so the window restarts. */
export function npsRecordScore(score: number, comment: string, now: number, storage: NpsStorage): void {
  let scores: { ts: number; score: number; comment: string }[] = []
  try {
    scores = JSON.parse(storage.get(SCORES_KEY) ?? '[]') as typeof scores
    if (!Array.isArray(scores)) scores = []
  } catch {
    scores = []
  }
  scores.push({ ts: now, score, comment })
  storage.set(SCORES_KEY, JSON.stringify(scores.slice(-20)))
  storage.set(LAST_PROMPT_KEY, String(now))
}

/** Stored NPS history (for the UX metrics surface). */
export function npsScores(storage: NpsStorage): { ts: number; score: number; comment: string }[] {
  try {
    const parsed = JSON.parse(storage.get(SCORES_KEY) ?? '[]')
    return Array.isArray(parsed) ? (parsed as { ts: number; score: number; comment: string }[]) : []
  } catch {
    return []
  }
}
