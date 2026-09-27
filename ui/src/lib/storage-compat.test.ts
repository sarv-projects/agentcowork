// DEC-053 — localStorage rename fallback: `agentcowork.*` wins, legacy
// `everyaios.*` is honored once and promoted, absent yields null.

import { describe, expect, test } from 'bun:test'
import {
  clearWithFallback,
  getLocalItem,
  legacyKeyFor,
  readWithFallback,
  type ClearableStore,
  type CompatStore,
} from './storage-compat'

/** In-memory store with a `set` call log, so promotion is observable. */
function memStore(initial: Record<string, string> = {}): CompatStore & { map: Map<string, string>; sets: string[] } {
  const map = new Map<string, string>(Object.entries(initial))
  const sets: string[] = []
  return {
    map,
    sets,
    get: (k) => map.get(k) ?? null,
    set: (k, v) => {
      sets.push(k)
      map.set(k, v)
    },
  }
}

describe('legacyKeyFor', () => {
  test('maps the renamed scope, and only the renamed scope', () => {
    expect(legacyKeyFor('agentcowork.settings.notify.chat')).toBe('everyaios.settings.notify.chat')
    expect(legacyKeyFor('agentcowork.theme')).toBe('everyaios.theme')
    expect(legacyKeyFor('agentcowork.layout.abc')).toBe('everyaios.layout.abc')
    expect(legacyKeyFor('fontScale')).toBeNull()
    expect(legacyKeyFor('other.key')).toBeNull()
  })
})

describe('readWithFallback', () => {
  test('new key present: returned as-is, legacy ignored, nothing promoted', () => {
    const s = memStore({ 'agentcowork.settings.x': '{"v":1}', 'everyaios.settings.x': '{"v":2}' })
    expect(readWithFallback(s, 'agentcowork.settings.x')).toBe('{"v":1}')
    expect(s.sets).toEqual([])
    expect(s.map.get('everyaios.settings.x')).toBe('{"v":2}')
  })

  test('legacy only: returned and promoted to the new key, legacy kept', () => {
    const s = memStore({ 'everyaios.settings.x': '{"v":2}' })
    expect(readWithFallback(s, 'agentcowork.settings.x')).toBe('{"v":2}')
    expect(s.map.get('agentcowork.settings.x')).toBe('{"v":2}')
    // Never deleted — rollback stays possible.
    expect(s.map.get('everyaios.settings.x')).toBe('{"v":2}')
  })

  test('neither key: null so the caller applies its default', () => {
    const s = memStore()
    expect(readWithFallback(s, 'agentcowork.settings.x')).toBeNull()
    expect(s.sets).toEqual([])
  })

  test('second call after promotion reads the new key and does not re-promote', () => {
    const s = memStore({ 'everyaios.settings.x': '{"v":2}' })
    expect(readWithFallback(s, 'agentcowork.settings.x')).toBe('{"v":2}')
    expect(s.sets).toEqual(['agentcowork.settings.x'])
    s.sets.length = 0
    expect(readWithFallback(s, 'agentcowork.settings.x')).toBe('{"v":2}')
    expect(s.sets).toEqual([])
  })

  test('empty string is a stored value, not an absent key', () => {
    const s = memStore({ 'agentcowork.settings.x': '', 'everyaios.settings.x': 'legacy' })
    expect(readWithFallback(s, 'agentcowork.settings.x')).toBe('')
    expect(s.sets).toEqual([])
  })

  test('a getItem-only store (no set) still reads without throwing', () => {
    const backing = new Map<string, string>([['everyaios.settings.x', 'legacy']])
    const stub = { get: (k: string) => backing.get(k) ?? null } as CompatStore
    expect(readWithFallback(stub, 'agentcowork.settings.x')).toBe('legacy')
  })

  test('a throwing store reads as absent', () => {
    const stub = {
      get: () => {
        throw new Error('denied')
      },
      set: () => {},
    }
    expect(readWithFallback(stub, 'agentcowork.settings.x')).toBeNull()
  })
})

describe('clearWithFallback', () => {
  function memStore(initial: Record<string, string> = {}): ClearableStore & { data: Record<string, string> } {
    const data = { ...initial }
    return {
      data,
      get: (k) => (k in data ? data[k] : null),
      set: (k, v) => {
        data[k] = v
      },
      remove: (k) => {
        delete data[k]
      },
    }
  }

  test('removes the new key and the legacy key', () => {
    const store = memStore({
      'agentcowork.ux-metrics.v1': 'new',
      'everyaios.ux-metrics.v1': 'legacy',
    })
    clearWithFallback(store, 'agentcowork.ux-metrics.v1')
    expect(store.data).toEqual({})
  })

  test('a clear after a legacy promotion does not let the value come back', () => {
    const store = memStore({ 'everyaios.ux-metrics.v1': 'legacy' })
    // first read promotes the legacy value
    expect(readWithFallback(store, 'agentcowork.ux-metrics.v1')).toBe('legacy')
    // the user clears it
    clearWithFallback(store, 'agentcowork.ux-metrics.v1')
    // the next read must see nothing, not the resurrected legacy value
    expect(readWithFallback(store, 'agentcowork.ux-metrics.v1')).toBeNull()
  })

  test('a key that was never renamed is removed once, with no legacy lookup', () => {
    const store = memStore({ 'agentcowork.ux-metrics.v1': 'x' })
    clearWithFallback(store, 'agentcowork.ux-metrics.v1')
    expect(store.data).toEqual({})
  })

  test('a throwing store does not propagate', () => {
    const hostile: ClearableStore = {
      get: () => {
        throw new Error('unavailable')
      },
      set: () => {
        throw new Error('unavailable')
      },
      remove: () => {
        throw new Error('unavailable')
      },
    }
    expect(() => clearWithFallback(hostile, 'agentcowork.settings.x')).not.toThrow()
  })
})

describe('getLocalItem', () => {
  test('outside the browser it returns null without throwing', () => {
    const originalWindow = (globalThis as { window?: unknown }).window
    delete (globalThis as { window?: unknown }).window
    try {
      expect(getLocalItem('agentcowork.settings.x')).toBeNull()
    } finally {
      if (originalWindow !== undefined) {
        ;(globalThis as { window?: unknown }).window = originalWindow
      }
    }
  })
})
