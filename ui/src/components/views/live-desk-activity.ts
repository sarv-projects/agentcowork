import type { WorkEventEnvelope } from '@/lib/work'

export type LiveDeskUpdateState = 'recorded' | 'waiting' | 'verified' | 'failed' | 'cancelled'

export interface LiveDeskUpdate {
  key: string
  label: string
  state: LiveDeskUpdateState
  at: number | null
  needsUser?: boolean
}

/** Keep unresolved requests actionable even when they fall outside the recent-update window. */
export function hasPendingUserAction(events: readonly WorkEventEnvelope[]): boolean {
  const ordered = [...events].sort((a, b) => a.sequence - b.sequence || a.timestamp - b.timestamp || a.eventId.localeCompare(b.eventId))
  const resolvedApprovals = new Map<string, number>()
  const laterRunTransitions = new Map<string, number[]>()

  for (const envelope of ordered) {
    const body = envelope.event
    if (body?.class !== 'domain') continue
    if (body.event.kind === 'approval_resolved') {
      resolvedApprovals.set(body.event.data.ticketId, envelope.sequence)
    } else if (
      body.event.kind === 'run_started' ||
      body.event.kind === 'run_completed' ||
      body.event.kind === 'run_failed' ||
      body.event.kind === 'run_cancelled'
    ) {
      const runId = body.event.data.runId
      laterRunTransitions.set(runId, [...(laterRunTransitions.get(runId) ?? []), envelope.sequence])
    }
  }

  return ordered.some((envelope) => {
    const body = envelope.event
    if (body?.class !== 'domain') return false
    if (body.event.kind === 'approval_requested') {
      return (resolvedApprovals.get(body.event.data.ticketId) ?? -1) <= envelope.sequence
    }
    if (body.event.kind === 'run_waiting') {
      const isUserWait = ['user_input', 'waiting_user'].includes(body.event.data.reason) ||
        ['user_input', 'waiting_user'].includes(body.event.data.wait?.reason ?? '')
      return isUserWait && !(laterRunTransitions.get(body.event.data.runId)?.some((sequence) => sequence > envelope.sequence))
    }
    return false
  })
}

/** Project only explicit, human-meaningful journal facts; opaque events stay out of the scene. */
export function projectLiveDeskUpdates(events: readonly WorkEventEnvelope[]): LiveDeskUpdate[] {
  const ordered = [...events].sort((a, b) => a.sequence - b.sequence || a.timestamp - b.timestamp || a.eventId.localeCompare(b.eventId))
  const seen = new Set<string>()
  const resolvedApprovals = new Map(
    ordered.flatMap((envelope) => {
      const body = envelope.event
      return body?.class === 'domain' && body.event.kind === 'approval_resolved'
        ? [[body.event.data.ticketId, envelope.sequence] as const]
        : []
    }),
  )
  const laterRunTransitions = new Map<string, number[]>()
  for (const envelope of ordered) {
    const body = envelope.event
    if (body?.class !== 'domain') continue
    if (
      body.event.kind === 'run_started' ||
      body.event.kind === 'run_completed' ||
      body.event.kind === 'run_failed' ||
      body.event.kind === 'run_cancelled'
    ) {
      const runId = body.event.data.runId
      laterRunTransitions.set(runId, [...(laterRunTransitions.get(runId) ?? []), envelope.sequence])
    }
  }
  const updates: LiveDeskUpdate[] = []

  for (const envelope of ordered) {
    if (seen.has(envelope.eventId)) continue
    seen.add(envelope.eventId)
    const body = envelope.event
    if (!body || typeof body !== 'object' || !('class' in body)) continue
    const event = body.event
    let label: string | undefined
    let state: LiveDeskUpdateState = 'recorded'
    let needsUser = false

    if (body.class === 'domain') {
      switch (event.kind) {
        case 'run_queued':
          label = 'Work is queued'
          break
        case 'run_paused':
          label = 'Work was paused'
          state = 'waiting'
          break
        case 'run_waiting': {
          if (laterRunTransitions.get(event.data.runId)?.some((sequence) => sequence > envelope.sequence)) break
          needsUser = ['user_input', 'waiting_user'].includes(event.data.reason) ||
            ['user_input', 'waiting_user'].includes(event.data.wait?.reason ?? '')
          label = needsUser
            ? 'Waiting for your response'
            : 'Waiting for the next step'
          state = needsUser ? 'waiting' : 'recorded'
          break
        }
        case 'run_interrupted':
          label = 'A run was interrupted; its outcome may need review'
          break
        case 'run_completed':
          label = 'A run finished'
          break
        case 'run_failed':
          label = 'A run stopped before finishing'
          state = 'failed'
          break
        case 'run_cancelled':
          label = 'A run was stopped'
          state = 'cancelled'
          break
        case 'approval_requested':
          if ((resolvedApprovals.get(event.data.ticketId) ?? -1) > envelope.sequence) break
          label = 'An action needs your approval'
          state = 'waiting'
          needsUser = true
          break
        case 'approval_resolved':
          label = event.data.approved ? 'Approval granted' : 'Approval declined'
          state = event.data.approved ? 'recorded' : 'cancelled'
          break
        case 'effect_verified':
          label = event.data.verified ? 'A change was verified' : 'A change could not be verified'
          state = event.data.verified ? 'verified' : 'failed'
          break
        case 'artifact_created':
          label = 'A new output was created'
          break
        case 'artifact_updated':
          label = 'An output was updated'
          break
      }
    } else if (body.class === 'operational') {
      switch (event.kind) {
        case 'file_touched': {
          const parts = event.data.path.replace(/\\/g, '/').split('/').filter(Boolean)
          const name = parts[parts.length - 1]
          label = name ? `Worked with ${name}` : 'A file was part of this work'
          break
        }
        case 'test_ran':
          label = event.data.passed ? 'A recorded check passed' : 'A recorded check failed'
          state = event.data.passed ? 'recorded' : 'failed'
          break
        case 'write_conflict':
          label = 'Two changes overlap and need review'
          state = 'failed'
          break
        case 'handoff_recorded':
          label = 'Another helper took on part of the work'
          break
      }
    }

    if (label) {
      updates.push({
        key: `${envelope.workId}:${envelope.sequence}:${envelope.eventId}`,
        label,
        state,
        at: Number.isFinite(envelope.timestamp) && envelope.timestamp > 0 ? envelope.timestamp : null,
        ...(needsUser ? { needsUser: true } : {}),
      })
    }
  }

  return updates.slice(-4).reverse()
}
