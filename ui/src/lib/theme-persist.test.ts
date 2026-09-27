import { afterAll, describe, expect, test } from 'bun:test'
import { THEME_STORAGE_KEY, ACCENT_STORAGE_KEY, readStoredTheme, readStoredAccent } from '@/components/theme-provider'

// P55.12 / P58.12 / P66.5 — one theme & accent owner that survives a reload. These cover the
// stored-value contract the provider relies on: the keys are stable and valid.

const originalWindow = (globalThis as { window?: unknown }).window

function stubStorage(themeVal: string | null, accentVal: string | null = null) {
  ;(globalThis as { window?: unknown }).window = {
    localStorage: {
      getItem: (k: string) => {
        if (k === THEME_STORAGE_KEY) return themeVal
        if (k === ACCENT_STORAGE_KEY) return accentVal
        return null
      },
    },
  }
}

afterAll(() => {
  if (originalWindow === undefined) {
    delete (globalThis as { window?: unknown }).window
  } else {
    ;(globalThis as { window?: unknown }).window = originalWindow
  }
})

describe('theme and accent persistence', () => {
  test('uses the shared storage keys', () => {
    expect(THEME_STORAGE_KEY).toBe('agentcowork.theme')
    expect(ACCENT_STORAGE_KEY).toBe('agentcowork.accent')
  })

  test('restores a stored theme', () => {
    stubStorage('dark')
    expect(readStoredTheme('light')).toBe('dark')
    stubStorage('light')
    expect(readStoredTheme('dark')).toBe('light')
  })

  test('restores a stored accent', () => {
    stubStorage(null, 'sky')
    expect(readStoredAccent('blue')).toBe('sky')
    stubStorage(null, 'emerald')
    expect(readStoredAccent('blue')).toBe('emerald')
    stubStorage(null, 'violet')
    expect(readStoredAccent('blue')).toBe('violet')
  })

  test('falls back when the stored value is absent or unrecognised', () => {
    stubStorage(null)
    expect(readStoredTheme('light')).toBe('light')
    expect(readStoredAccent('blue')).toBe('blue')
    stubStorage('sepia', 'neon-pink')
    expect(readStoredTheme('light')).toBe('light')
    expect(readStoredAccent('blue')).toBe('blue')
  })

  test('falls back when storage is unavailable', () => {
    delete (globalThis as { window?: unknown }).window
    expect(readStoredTheme('light')).toBe('light')
    expect(readStoredAccent('blue')).toBe('blue')
  })
})
