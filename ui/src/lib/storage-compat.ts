/**
 * DEC-053 localStorage rename fallback (UI).
 *
 * Preference keys moved from `everyaios.*` to `agentcowork.*`. Existing local
 * profiles still hold values under the legacy keys, so every preference read
 * goes through {@link readWithFallback}: the new key wins; when it is absent
 * the legacy key is honored **and** promoted (the new key is written once, so
 * the migration is self-healing and happens exactly once per key). The legacy
 * key is never deleted — rollback stays possible. Writes always use the new
 * key only. An **explicit clear** is the one exception: it must reach the
 * legacy key too, or a pre-rename value would resurrect on the next read
 * (see {@link clearWithFallback}).
 *
 * The one-time migration of Rust-owned data (data home, config files) belongs
 * solely to Core; this module covers webview-local preference keys only. It
 * is deliberately dependency-free (no React) so pure-logic modules (`nps`,
 * `first-run`, `session-recording`, …) can use it without gaining imports.
 */

const NEW_SCOPE = 'agentcowork.'
const LEGACY_SCOPE = 'everyaios.'

/** The legacy spelling of a renamed key, or `null` when the key was never renamed. */
export function legacyKeyFor(key: string): string | null {
  return key.startsWith(NEW_SCOPE) ? LEGACY_SCOPE + key.slice(NEW_SCOPE.length) : null
}

/** Minimal key/value surface — `localStorage` and the injectable test shims both fit. */
export interface CompatStore {
  get(key: string): string | null
  set(key: string, value: string): void
}

/**
 * Read one key with legacy fallback + self-healing promotion. Absent means
 * `null` (or a runtime `undefined`) only — an empty string is a stored value
 * and is returned as-is, so pre-rename "cleared" sentinels keep their meaning
 * and promotion never fires for a key that is actually present.
 */
export function readWithFallback(store: CompatStore, key: string): string | null {
  let raw: string | null
  try {
    raw = store.get(key) ?? null
  } catch {
    return null
  }
  if (raw !== null) return raw
  const legacy = legacyKeyFor(key)
  if (legacy === null) return null
  let legacyRaw: string | null
  try {
    legacyRaw = store.get(legacy) ?? null
  } catch {
    return null
  }
  if (legacyRaw === null) return null
  try {
    store.set(key, legacyRaw)
  } catch {
    /* promotion is best-effort (e.g. getItem-only stubs in tests) */
  }
  return legacyRaw
}

/** `window.localStorage` read with the DEC-053 fallback, or `null` outside the browser. */
export function getLocalItem(key: string): string | null {
  if (typeof window === 'undefined') return null
  try {
    const ls = window.localStorage
    if (!ls) return null
    return readWithFallback(
      {
        get: (k) => ls.getItem(k),
        set: (k, v) => ls.setItem(k, v),
      },
      key,
    )
  } catch {
    return null
  }
}

/** A store that can also delete — required so a clear reaches the legacy key. */
export interface ClearableStore extends CompatStore {
  remove(key: string): void
}

/**
 * Clear one key for real: the new spelling **and** its legacy spelling.
 *
 * {@link readWithFallback} keeps the legacy key so rollback stays possible,
 * which is right for a read but wrong for a clear: if only the new key were
 * removed, the next read would find the legacy value still there, promote it
 * and hand the user back the thing they just discarded. A clear means "forget
 * this", so it has to reach both. Deleting the legacy key here costs nothing —
 * the new key already holds the live value for any rollback that matters.
 */
export function clearWithFallback(store: ClearableStore, key: string): void {
  try {
    store.remove(key)
  } catch {
    /* storage may be unavailable */
  }
  const legacy = legacyKeyFor(key)
  if (legacy === null) return
  try {
    store.remove(legacy)
  } catch {
    /* storage may be unavailable */
  }
}

/** `window.localStorage` clear that reaches the legacy spelling too. */
export function clearLocalKey(key: string): void {
  if (typeof window === 'undefined') return
  try {
    const ls = window.localStorage
    if (!ls) return
    clearWithFallback(
      {
        get: (k) => ls.getItem(k),
        set: (k, v) => ls.setItem(k, v),
        remove: (k) => ls.removeItem(k),
      },
      key,
    )
  } catch {
    /* storage may be unavailable */
  }
}
