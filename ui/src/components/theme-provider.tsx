'use client'

import * as React from 'react'
import {
  readStoredFlag,
  readStoredText,
  removeStoredKey,
  writeStoredFlag,
  writeStoredText,
} from '@/lib/ui-prefs'

/** The **resolved** theme actually applied to the document. */
export type Theme = 'dark' | 'light'
/** What the user picked — `system` follows the OS and never writes a class of its own. */
export type ThemeMode = Theme | 'system'
export type Accent = 'blue' | 'sky' | 'emerald' | 'violet' | 'amber' | 'rose' | 'teal'
export type FontScale = 'sm' | 'md' | 'lg'
export type Density = 'compact' | 'comfortable' | 'spacious'

interface ThemeContextValue {
  /** Effective theme (`system` already resolved) — what consumers paint with. */
  theme: Theme
  /** The stored preference, including `system`. */
  mode: ThemeMode
  setMode: (m: ThemeMode) => void
  /** Pick an explicit theme; equivalent to `setMode(theme)`. */
  setTheme: (t: Theme) => void
  /** Flip to the opposite of what is showing now (leaves `system` behind). */
  toggle: () => void
  accent: Accent
  setAccent: (a: Accent) => void
  fontScale: FontScale
  setFontScale: (s: FontScale) => void
  highContrast: boolean
  setHighContrast: (on: boolean) => void
  density: Density
  setDensity: (d: Density) => void
  /** Restore every appearance dimension to its shipped default. */
  resetAppearance: () => void
}

const ThemeContext = React.createContext<ThemeContextValue | null>(null)

// P55.12 / P58.12 / P66.5 / P70 — one appearance owner. The provider is the
// only writer of every appearance attribute on <html>, so the title-bar
// toggle, the command palette and Settings → Appearance can never disagree,
// and — the bug this fixes — font scale, high contrast and density survive a
// reload instead of only applying while the Appearance panel happens to be
// mounted.
export const THEME_STORAGE_KEY = 'agentcowork.theme'
export const ACCENT_STORAGE_KEY = 'agentcowork.accent'
/** `usePref` keys (prefixed `agentcowork.settings.` on write). */
export const FONT_SCALE_PREF_KEY = 'fontScale'
export const HIGH_CONTRAST_PREF_KEY = 'highContrast'
export const DENSITY_PREF_KEY = 'density'

/** The accent presets, with a label and a preview swatch. One list owns the
 * picker, the pre-paint script's whitelist and the CSS `[data-accent]` blocks,
 * so a preset cannot be offered without a stylesheet that paints it. */
export const ACCENT_PRESETS: readonly { id: Accent; label: string; swatch: string }[] = [
  { id: 'blue', label: 'Blue', swatch: 'bg-blue-600' },
  { id: 'sky', label: 'Sky', swatch: 'bg-sky-500' },
  { id: 'emerald', label: 'Emerald', swatch: 'bg-emerald-500' },
  { id: 'violet', label: 'Violet', swatch: 'bg-violet-500' },
  { id: 'amber', label: 'Amber', swatch: 'bg-warning' },
  { id: 'rose', label: 'Rose', swatch: 'bg-rose-500' },
  { id: 'teal', label: 'Teal', swatch: 'bg-teal-500' },
]

/** `light` · `dark` · `system`, in picker order. */
export const THEME_PRESETS: readonly { id: ThemeMode; label: string; hint: string }[] = [
  { id: 'light', label: 'Light', hint: 'Warm cream canvas' },
  { id: 'dark', label: 'Dark', hint: 'Low-glare night work' },
  { id: 'system', label: 'System', hint: 'Follow the OS setting' },
]

/** `compact` · `comfortable` · `spacious`, in picker order. */
export const DENSITY_PRESETS: readonly { id: Density; label: string }[] = [
  { id: 'compact', label: 'Compact' },
  { id: 'comfortable', label: 'Comfortable' },
  { id: 'spacious', label: 'Spacious' },
]

export const ACCENTS: readonly Accent[] = ACCENT_PRESETS.map((a) => a.id)

const THEME_MODES: readonly ThemeMode[] = ['light', 'dark', 'system']
const FONT_SCALES: readonly FontScale[] = ['sm', 'md', 'lg']
const DENSITIES: readonly Density[] = DENSITY_PRESETS.map((d) => d.id)

/** The OS preference, or `null` when the environment cannot answer. */
export function systemPrefersDark(): boolean | null {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return null
  try {
    return window.matchMedia('(prefers-color-scheme: dark)').matches
  } catch {
    return null
  }
}

/** Resolve a stored mode against the OS preference. `system` with no
 * available media query falls back to the app default (dark). */
export function resolveTheme(mode: ThemeMode, fallback: Theme = 'dark'): Theme {
  if (mode === 'light' || mode === 'dark') return mode
  const dark = systemPrefersDark()
  return dark === null ? fallback : dark ? 'dark' : 'light'
}

export function readStoredTheme(fallback: Theme = 'dark'): Theme {
  return resolveTheme(readStoredThemeMode(), fallback)
}

export function readStoredThemeMode(fallback: ThemeMode = 'system'): ThemeMode {
  if (!THEME_MODES.includes(fallback)) fallback = 'system'
  const v = readStoredText(THEME_STORAGE_KEY, fallback)
  return THEME_MODES.includes(v as ThemeMode) ? (v as ThemeMode) : fallback
}

export function readStoredAccent(fallback: Accent = 'blue'): Accent {
  const v = readStoredText(ACCENT_STORAGE_KEY, fallback)
  return ACCENTS.includes(v as Accent) ? (v as Accent) : fallback
}

export function readStoredFontScale(fallback: FontScale = 'md'): FontScale {
  const v = readStoredText(FONT_SCALE_PREF_KEY, fallback)
  return FONT_SCALES.includes(v as FontScale) ? (v as FontScale) : fallback
}

export function readStoredHighContrast(fallback = false): boolean {
  return readStoredFlag(HIGH_CONTRAST_PREF_KEY, fallback)
}

export function readStoredDensity(fallback: Density = 'comfortable'): Density {
  const v = readStoredText(DENSITY_PREF_KEY, fallback)
  return DENSITIES.includes(v as Density) ? (v as Density) : fallback
}

/** Apply one appearance snapshot to <html>. Pure DOM — no state. Shared by the
 * provider and the pre-paint script's contract so both stay in step. */
export function applyAppearance(a: {
  theme: Theme
  accent: Accent
  fontScale: FontScale
  highContrast: boolean
  density: Density
}): void {
  if (typeof document === 'undefined') return
  const root = document.documentElement
  root.classList.remove('light', 'dark')
  root.classList.add(a.theme)
  root.setAttribute('data-accent', a.accent)
  root.setAttribute('data-density', a.density)
  root.classList.remove('font-scale-sm', 'font-scale-md', 'font-scale-lg')
  root.classList.add(`font-scale-${a.fontScale}`)
  root.classList.toggle('high-contrast', a.highContrast)
}

export function ThemeProvider({
  children,
  defaultTheme = 'dark',
  defaultAccent = 'blue',
  enableSystem = true,
}: {
  children: React.ReactNode
  defaultTheme?: Theme
  defaultAccent?: Accent
  enableSystem?: boolean
}) {
  const [mode, setModeState] = React.useState<ThemeMode>(() =>
    enableSystem ? readStoredThemeMode('system') : readStoredThemeMode(defaultTheme),
  )
  const [accent, setAccentState] = React.useState<Accent>(() => readStoredAccent(defaultAccent))
  const [fontScale, setFontScaleState] = React.useState<FontScale>(() => readStoredFontScale())
  const [highContrast, setHighContrastState] = React.useState<boolean>(() => readStoredHighContrast())
  const [density, setDensityState] = React.useState<Density>(() => readStoredDensity())

  const [theme, setThemeState] = React.useState<Theme>(() => resolveTheme(readStoredThemeMode('system'), defaultTheme))

  // A `system` mode must track the OS *live*, not only at boot.
  React.useEffect(() => {
    if (mode !== 'system') {
      setThemeState(mode)
      return
    }
    setThemeState(resolveTheme('system', defaultTheme))
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return
    let mq: MediaQueryList
    try {
      mq = window.matchMedia('(prefers-color-scheme: dark)')
    } catch {
      return
    }
    const onChange = () => setThemeState(mq.matches ? 'dark' : 'light')
    mq.addEventListener('change', onChange)
    return () => mq.removeEventListener('change', onChange)
  }, [mode, defaultTheme])

  React.useEffect(() => {
    applyAppearance({ theme, accent, fontScale, highContrast, density })
  }, [theme, accent, fontScale, highContrast, density])

  React.useEffect(() => {
    writeStoredText(THEME_STORAGE_KEY, mode)
  }, [mode])
  React.useEffect(() => {
    writeStoredText(ACCENT_STORAGE_KEY, accent)
  }, [accent])
  React.useEffect(() => {
    writeStoredText(FONT_SCALE_PREF_KEY, fontScale)
  }, [fontScale])
  React.useEffect(() => {
    writeStoredFlag(HIGH_CONTRAST_PREF_KEY, highContrast)
  }, [highContrast])
  React.useEffect(() => {
    writeStoredText(DENSITY_PREF_KEY, density)
  }, [density])

  const setMode = React.useCallback((m: ThemeMode) => setModeState(m), [])
  const setAccent = React.useCallback((a: Accent) => setAccentState(a), [])
  const setFontScale = React.useCallback((s: FontScale) => setFontScaleState(s), [])
  const setHighContrast = React.useCallback((on: boolean) => setHighContrastState(on), [])
  const setDensity = React.useCallback((d: Density) => setDensityState(d), [])
  // "Put it back the way it was" needs one owner. Prefs reset to their
  // shipped values with the *stored* keys cleared, so a reload cannot restore
  // the choice the user just discarded.
  const resetAppearance = React.useCallback(() => {
    setModeState(enableSystem ? 'system' : defaultTheme)
    setAccentState(defaultAccent)
    setFontScaleState('md')
    setHighContrastState(false)
    setDensityState('comfortable')
    for (const key of [
      THEME_STORAGE_KEY,
      ACCENT_STORAGE_KEY,
      FONT_SCALE_PREF_KEY,
      HIGH_CONTRAST_PREF_KEY,
      DENSITY_PREF_KEY,
    ]) {
      removeStoredKey(key)
    }
  }, [defaultAccent, defaultTheme, enableSystem])
  // `toggle` is the title-bar/palette affordance: it always lands on an
  // explicit theme, so a `system` user who clicks the sun still gets a
  // predictable, stable result.
  const toggle = React.useCallback(() => {
    setModeState((m) => (resolveTheme(m, defaultTheme) === 'dark' ? 'light' : 'dark'))
  }, [defaultTheme])

  const value = React.useMemo(
    () => ({
      theme,
      mode,
      setMode,
      setTheme: setMode,
      toggle,
      accent,
      setAccent,
      fontScale,
      setFontScale,
      highContrast,
      setHighContrast,
      density,
      setDensity,
      resetAppearance,
    }),
    [
      theme,
      mode,
      setMode,
      toggle,
      accent,
      setAccent,
      fontScale,
      setFontScale,
      highContrast,
      setHighContrast,
      density,
      setDensity,
      resetAppearance,
    ],
  )

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
}

export function useTheme() {
  const ctx = React.useContext(ThemeContext)
  if (!ctx) throw new Error('useTheme must be used within ThemeProvider')
  return ctx
}
