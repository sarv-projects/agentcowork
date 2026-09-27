// P11.6.4 — key UX metrics, tracked locally (Task completion rate,
// time-to-value, error rate). Deliberately content-free: counters only, no
// message text, no provider payloads. Persisted to localStorage; surfaced in
// Settings → Usage. This is NOT telemetry — nothing leaves the device.

import { clearLocalKey, getLocalItem } from './storage-compat'

export interface UxMetrics {
  firstSeenAtMs: number
  sessionsCreated: number
  turnsCompleted: number
  turnsFailed: number
  firstTurnAtMs: number | null
  firstToolResultAtMs: number | null
  approvalsGranted: number
  approvalsRejected: number
}

const KEY = 'agentcowork.ux-metrics.v1'

const EMPTY: UxMetrics = {
  firstSeenAtMs: Date.now(),
  sessionsCreated: 0,
  turnsCompleted: 0,
  turnsFailed: 0,
  firstTurnAtMs: null,
  firstToolResultAtMs: null,
  approvalsGranted: 0,
  approvalsRejected: 0,
}

function load(): UxMetrics {
  try {
    // DEC-053: legacy `everyaios.*` key honored + promoted once.
    const raw = getLocalItem(KEY)
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<UxMetrics>
      return { ...EMPTY, ...parsed }
    }
  } catch {
    /* fresh start */
  }
  return { ...EMPTY }
}

function save(m: UxMetrics): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(m))
  } catch {
    /* quota — drop silently, metrics are best-effort */
  }
}

function mutate(fn: (m: UxMetrics) => void): UxMetrics {
  const m = load()
  fn(m)
  save(m)
  return m
}

export function recordSessionCreated(): void {
  mutate((m) => {
    m.sessionsCreated += 1
  })
}

/** A turn that streamed to completion (streamFinalize). */
export function recordTurnCompleted(): void {
  mutate((m) => {
    if (m.firstTurnAtMs === null) m.firstTurnAtMs = Date.now()
    m.turnsCompleted += 1
  })
}

/** A turn that ended in an error (streamFail). */
export function recordTurnFailed(): void {
  mutate((m) => {
    m.turnsFailed += 1
  })
}

/** The first successful tool result — the time-to-value anchor. */
export function recordToolResult(): void {
  mutate((m) => {
    if (m.firstToolResultAtMs === null) m.firstToolResultAtMs = Date.now()
  })
}

/** A Guard-2 approve/reject decision (from the dedicated guard window flow). */
export function recordApprovalDecision(approved: boolean): void {
  mutate((m) => {
    if (approved) m.approvalsGranted += 1
    else m.approvalsRejected += 1
  })
}

export function getUxMetrics(): UxMetrics {
  return load()
}

export function resetUxMetrics(): void {
  // DEC-053: an explicit clear must also drop the legacy `everyaios.*` key, or
  // a pre-rename value is promoted back on the next read.
  clearLocalKey(KEY)
}

/** ms from first launch to first successful tool result (null = not reached). */
export function timeToValueMs(m: UxMetrics = load()): number | null {
  if (m.firstToolResultAtMs === null || m.firstTurnAtMs === null) return null
  return Math.max(0, m.firstToolResultAtMs - m.firstTurnAtMs)
}

/** completed / (completed + failed) — null when no turns yet. */
export function completionRate(m: UxMetrics = load()): number | null {
  const total = m.turnsCompleted + m.turnsFailed
  if (total === 0) return null
  return m.turnsCompleted / total
}

/** failed / (completed + failed) — null when no turns yet. */
export function errorRate(m: UxMetrics = load()): number | null {
  const total = m.turnsCompleted + m.turnsFailed
  if (total === 0) return null
  return m.turnsFailed / total
}
