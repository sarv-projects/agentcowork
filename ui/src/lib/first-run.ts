// P32.10 / WP2 — "kill the blank canvas".
//
// Research (Wharton/Nielsen, 2026) is blunt about the first five minutes: users
// who face an empty prompt either over-scope the ask or freeze, and both fail.
// The fix is 3–5 *pre-scoped, achievable* first tasks rather than a text box
// with example phrases that only fill it.
//
// Two pieces of pure logic live here so they can be unit-tested without
// rendering (the same injectable-storage pattern as `nps.ts`):
//   1. `FIRST_TASKS` — the scoped starters the Home surface offers.
//   2. the 24-hour first-task nudge, which fires at most once and never for a
//      user who already has work.
//
// Nothing here touches the network, the store, or the guard.

import { readWithFallback } from './storage-compat'

export const DAY_MS = 86_400_000
/** Nudge once the user has had the app open this long without starting work. */
export const FIRST_TASK_NUDGE_AFTER_MS = DAY_MS

export const FIRST_SEEN_KEY = 'agentcowork.first-task.first-seen'
export const NUDGE_SHOWN_KEY = 'agentcowork.first-task.nudge-shown'

/** A pre-scoped starter: what it is, what will actually happen, and the ask. */
export interface FirstTask {
  id: string
  emoji: string
  label: string
  /** One plain sentence saying what the agent will do — no jargon. */
  detail: string
  /** The prompt the starter puts in the composer. */
  prompt: string
}

/**
 * The starters. Each one is achievable without any setup, and each states the
 * boundary up front ("before moving anything", "nothing gets sent") so the
 * first interaction is never a surprise.
 */
export const FIRST_TASKS: FirstTask[] = [
  {
    id: 'tidy-downloads',
    emoji: '📁',
    label: 'Tidy up my Downloads',
    detail: "I'll show you what's in there and propose a sorted layout — nothing moves until you approve.",
    prompt: 'Clean up my Downloads folder. Show me the plan before moving anything.',
  },
  {
    id: 'summarise-doc',
    emoji: '📄',
    label: 'Summarise a document',
    detail: "Attach a PDF or Word file and I'll pull out the key points.",
    prompt: 'Summarise the document I attach — key points and anything that needs action.',
  },
  {
    id: 'research',
    emoji: '🔍',
    label: 'Research something for me',
    detail: "I'll search the web and answer with the sources attached to every claim.",
    prompt: 'Research this for me and cite your sources: ',
  },
  {
    id: 'spreadsheet',
    emoji: '📊',
    label: 'Make sense of a spreadsheet',
    detail: "Attach a workbook and I'll read the numbers and answer your questions about them.",
    prompt: 'Help me understand the spreadsheet I attach — what do the numbers say?',
  },
  // WP7 — the student posture. This replaced an email starter that was not
  // actually achievable without a connector configured, which is the opposite
  // of what a pre-scoped first task is for.
  {
    id: 'explain',
    emoji: '🎓',
    label: 'Explain something simply',
    detail: "Ask about any topic or attach a paper — I'll explain it in plain words, at your level.",
    prompt: 'Explain this simply, in plain language, then ask me a question to check I understood: ',
  },
]

/** Storage surface — localStorage in the app, an in-memory shim in tests. */
export interface FirstRunStorage {
  get(key: string): string | null
  set(key: string, value: string): void
}

export const localStorageFirstRunStorage: FirstRunStorage = {
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
      /* storage unavailable — the nudge simply never fires */
    }
  },
}

/** Read a numeric stamp, or null when absent/unparseable. */
function readStamp(storage: FirstRunStorage, key: string): number | null {
  const raw = storage.get(key)
  // '' means "cleared" (the storage surface has no delete), so it reads as absent.
  if (raw == null || raw === '') return null
  const n = Number(raw)
  return Number.isFinite(n) ? n : null
}

/**
 * Stamp first-seen the first time this is called; later calls leave it alone,
 * so the 24-hour clock measures time since first launch, not since last open.
 */
export function markFirstSeen(
  storage: FirstRunStorage,
  now: number = Date.now(),
): number {
  const existing = readStamp(storage, FIRST_SEEN_KEY)
  if (existing != null) return existing
  storage.set(FIRST_SEEN_KEY, String(now))
  return now
}

/**
 * Should the Home surface nudge the user to start a first task?
 *
 * False when the user already has work (a nudge to someone who is busy is
 * noise), false once it has fired (it fires at most once), and false until the
 * nudge delay has elapsed since first launch.
 */
export function shouldNudgeFirstTask(
  storage: FirstRunStorage,
  now: number = Date.now(),
  hasWork: boolean = false,
): boolean {
  if (hasWork) return false
  if (readStamp(storage, NUDGE_SHOWN_KEY) != null) return false
  const firstSeen = markFirstSeen(storage, now)
  if (now - firstSeen < FIRST_TASK_NUDGE_AFTER_MS) return false
  storage.set(NUDGE_SHOWN_KEY, String(now))
  return true
}

/** Reset the nudge so it can fire again (used by tests and the demo reset). */
export function resetFirstRun(storage: FirstRunStorage): void {
  storage.set(FIRST_SEEN_KEY, '')
  storage.set(NUDGE_SHOWN_KEY, '')
}
