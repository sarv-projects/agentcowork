// P11.6.5 — opt-in session recording for UX analysis.
//
// Records only clicks and view/navigation changes — never message content,
// never tool payloads, never provider responses. Element identity comes from
// `aria-label` / `data-testid` / `data-view` / tag name only (textContent is
// deliberately excluded). Off by default; ring-buffered in localStorage with
// a hard cap; exportable as JSON from Settings → Usage.

import { clearWithFallback, readWithFallback } from './storage-compat'

export interface RecordedSessionEvent {
  ts: number
  kind: 'click' | 'navigate'
  /** Element identity (aria-label/testid/view/tag) — never content. */
  target: string
}

const PREF_KEY = 'agentcowork.session-recording'
const EVENTS_KEY = 'agentcowork.session-recording.events'
/** Hard ring-buffer cap — keeps the opt-in recording bounded. */
export const MAX_RECORDED_EVENTS = 2000

/** Storage surface — localStorage in the app, a shim in tests. */
export interface RecordingStorage {
  get(key: string): string | null
  set(key: string, value: string): void
  remove(key: string): void
}

export const localStorageRecordingStorage: RecordingStorage = {
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
  remove: (key) => {
    try {
      localStorage.removeItem(key)
    } catch {
      /* ignore */
    }
  },
}

let storage: RecordingStorage = localStorageRecordingStorage

/** Tests inject a deterministic shim. */
export function __setRecordingStorage(next: RecordingStorage): void {
  storage = next
}

export function sessionRecordingEnabled(): boolean {
  return storage.get(PREF_KEY) === '1'
}

export function setSessionRecording(enabled: boolean): void {
  storage.set(PREF_KEY, enabled ? '1' : '0')
  if (!enabled) clearRecordedEvents()
}

/** Derive a content-free identity for an element (duck-typed so it works in
 * any runtime — the app passes real DOM nodes, tests pass plain objects). */
function elementIdentity(target: EventTarget | null): string {
  const el = target as { getAttribute?: (n: string) => string | null; tagName?: string } | null
  if (!el || typeof el.getAttribute !== 'function') return 'unknown'
  const tag = (el.tagName ?? 'el').toLowerCase()
  const label =
    el.getAttribute('aria-label') ??
    el.getAttribute('data-testid') ??
    el.getAttribute('data-view') ??
    el.getAttribute('title')
  if (label) return `${tag}:${label.slice(0, 60)}`
  return tag
}

export function recordSessionEvent(kind: RecordedSessionEvent['kind'], target: EventTarget | null): void {
  if (!sessionRecordingEnabled()) return
  const events = loadEvents()
  events.push({ ts: Date.now(), kind, target: elementIdentity(target) })
  // Ring buffer: keep the newest MAX_RECORDED_EVENTS.
  storage.set(EVENTS_KEY, JSON.stringify(events.slice(-MAX_RECORDED_EVENTS)))
}

export function getRecordedEvents(): RecordedSessionEvent[] {
  return loadEvents()
}

export function clearRecordedEvents(): void {
  // DEC-053: an explicit clear must also drop the legacy `everyaios.*` key, or
  // a pre-rename value is promoted back on the next read.
  clearWithFallback(storage, EVENTS_KEY)
}

function loadEvents(): RecordedSessionEvent[] {
  try {
    const raw = storage.get(EVENTS_KEY)
    if (raw) {
      const parsed = JSON.parse(raw)
      if (Array.isArray(parsed)) return parsed as RecordedSessionEvent[]
    }
  } catch {
    /* ignore */
  }
  return []
}

/** Export as a downloadable JSON blob (used by Settings → Usage). */
export function exportRecordedEvents(): string {
  return JSON.stringify({ generatedAtMs: Date.now(), events: loadEvents() }, null, 2)
}
