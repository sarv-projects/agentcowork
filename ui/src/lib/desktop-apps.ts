// P57.8 — Settings → Computer use presentation helpers.
//
// One owner for the two things the surface must not guess: what the derived H4
// readiness state is *called* (a label + a sentence, never a bare green dot),
// and what the OS file picker should accept for **Add by path** (a Windows
// `.exe`, a macOS `.app` bundle — which is a *directory*, so the picker must be
// in directory mode there — or a Linux binary).
//
// The rules here are pure so they are testable without a display, mirroring the
// Rust side (`crates/agentcowork-desktop/src/readiness.rs` + `apps.rs`): the
// backend derives the state and the inventory, the UI only names them.

export type DesktopReadinessState =
  | 'ready'
  | 'permission_required'
  | 'driver_missing'
  | 'app_unsupported'
  | 'policy_blocked'

export interface DesktopReadiness {
  state: DesktopReadinessState
  detail: string
  usable: boolean
}

export interface DesktopInstalledApp {
  name: string
  path: string
  source: 'desktop_entry' | 'app_bundle' | 'start_menu' | 'user_path'
  /** Non-null = the policy can never automate this app; show a block, no Add. */
  hardDenied: string | null
  allowListed: boolean
}

export interface DesktopPolicy {
  allowList: string[]
  allowPaths: string[]
  strict: boolean
  interactionDefault: 'background' | 'foreground'
  allowsRaisingWindows: boolean
}

/** Badge presentation for one readiness state: label, tone and glyph. */
export interface ReadinessView {
  label: string
  tone: string
  glyph: string
  detail: string
  usable: boolean
}

export interface BackgroundInputView {
  label: string
  tone: string
  glyph: string
  detail: string
}

const READINESS: Record<DesktopReadinessState, { label: string; tone: string; glyph: string }> = {
  ready: { label: 'Ready', tone: 'text-emerald-300', glyph: '●' },
  permission_required: { label: 'Permission required', tone: 'text-warning', glyph: '⚠' },
  driver_missing: { label: 'Driver missing', tone: 'text-warning', glyph: '⚠' },
  app_unsupported: { label: 'App unsupported', tone: 'text-warning', glyph: '⚠' },
  policy_blocked: { label: 'Blocked by policy', tone: 'text-red-300', glyph: '✕' },
}

/** The H4 chip: always a label + the backend's sentence, never a bare dot. */
export function readinessView(r: DesktopReadiness | null): ReadinessView | null {
  if (!r) return null
  const known = READINESS[r.state] ?? {
    label: 'Unknown',
    tone: 'text-muted-foreground',
    glyph: '?',
  }
  return { ...known, detail: r.detail, usable: r.usable }
}

/** What the native file picker should accept for **Add by path**. */
export function pickerOptions(platform: string): {
  directory: boolean
  filters: { name: string; extensions: string[] }[]
  title: string
} {
  const p = platform.toLowerCase()
  if (p.includes('mac') || p.includes('darwin')) {
    // A `.app` is a directory: the picker must be in directory mode or the
    // user cannot select it at all.
    return {
      directory: true,
      filters: [],
      title: 'Choose an application bundle (.app)',
    }
  }
  if (p.includes('win')) {
    return {
      directory: false,
      filters: [{ name: 'Programs', extensions: ['exe', 'cmd', 'bat', 'lnk'] }],
      title: 'Choose a program (.exe)',
    }
  }
  return {
    directory: false,
    filters: [],
    title: 'Choose an executable',
  }
}

/** The row's secondary line: the path itself (never a truncated claim). */
export function pathTail(path: string, max = 56): string {
  const p = path.replace(/^\\\\\?\\/, '').replace(/\\/g, '/')
  if (p.length <= max) return p
  return `…${p.slice(p.length - max + 1)}`
}

/** Where an inventory row came from, in words. */
export function sourceLabel(source: DesktopInstalledApp['source']): string {
  switch (source) {
    case 'desktop_entry':
      return 'desktop entry'
    case 'app_bundle':
      return 'application bundle'
    case 'start_menu':
      return 'start menu'
    case 'user_path':
      return 'added by path'
    default:
      return 'unknown'
  }
}

/**
 * Why a row cannot be added, or `null` when it can. `allowListed` rows are not
 * "addable" either — the picker renders them as already listed.
 */
export function addBlockReason(app: DesktopInstalledApp): string | null {
  if (app.hardDenied) return app.hardDenied
  if (app.allowListed) return 'already allow-listed'
  return null
}

/**
 * Host-level capability, not a promise that every application will accept a
 * synthetic event. A native attach is required before the capability is known;
 * the backend's verify cascade still decides whether a specific action worked.
 */
export function backgroundInputView(
  attached: boolean,
  backgroundInput: boolean | undefined,
): BackgroundInputView {
  if (!attached || backgroundInput === undefined) {
    return {
      label: 'Not measured',
      tone: 'text-muted-foreground',
      glyph: '·',
      detail: 'The desktop driver is not attached, so this host capability has not been measured yet.',
    }
  }
  if (backgroundInput) {
    return {
      label: 'Available on this host',
      tone: 'text-emerald-300',
      glyph: '●',
      detail: 'A non-moving coordinate-click path exists; this does not guarantee that every app will honor a synthetic event. Verification remains authoritative.',
    }
  }
  return {
    label: 'Unavailable on this host',
    tone: 'text-warning',
    glyph: '⚠',
    detail: 'The Background coordinate-click path is unavailable here; coordinate clicks will refuse rather than move your pointer. Use a named accessibility action or switch to Foreground.',
  }
}

/** Honest copy for the two interaction defaults. */
export function interactionCopy(mode: DesktopPolicy['interactionDefault']): string {
  return mode === 'foreground'
    ? 'Foreground — the driver may raise a window and take focus. Use only while an app refuses background input; the last foreground is restored after.'
    : 'Background (default) — the driver never raises a window or takes your cursor/focus. Raising needs an explicit foreground escalation.'
}
