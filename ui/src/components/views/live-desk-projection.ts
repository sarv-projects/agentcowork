import type { AgentCard } from '@/lib/agent-card'
import type { SessionStatus } from '@/lib/store'

export type LiveDeskStatus =
  | 'working'
  | 'waiting'
  | 'reconnecting'
  | 'scheduled'
  | 'finished'
  | 'stopped'
  | 'paused'
  | 'ready'

export interface LiveDeskProjection {
  title: string
  status: LiveDeskStatus
  statusLabel: string
  summary: string
  filesTouched: number
  checksReported: number
  checksPassed: number
  checksFailed: number
}

/** Build the human-facing Live Desk summary only from the active session and its Work journal. */
export function projectLiveDesk(input: {
  title?: string | null
  sessionStatus?: SessionStatus | null
  card: AgentCard
  needsAttention?: boolean
}): LiveDeskProjection {
  const { card } = input
  const sessionStatus = input.sessionStatus ?? 'idle'
  let status: LiveDeskStatus = 'ready'

  if (sessionStatus === 'action-required' || card.awaitingInput || input.needsAttention) status = 'waiting'
  else if (sessionStatus === 'paused') status = 'paused'
  else if (sessionStatus === 'reconnecting') status = 'reconnecting'
  else if (sessionStatus === 'scheduled') status = 'scheduled'
  else if (sessionStatus === 'running' || card.status === 'running') status = 'working'
  else if (sessionStatus === 'completed' || card.status === 'completed') status = 'finished'
  else if (
    sessionStatus === 'failed' ||
    sessionStatus === 'cancelled' ||
    sessionStatus === 'budget_exceeded' ||
    card.status === 'failed'
  ) status = 'stopped'

  const copy: Record<LiveDeskStatus, { label: string; summary: string }> = {
    working: { label: 'Working', summary: 'AgentCowork is working on your request.' },
    waiting: { label: 'Needs your attention', summary: 'This work is waiting for your response.' },
    reconnecting: { label: 'Reconnecting', summary: 'Restoring the connection to this work. Its progress will update when the connection returns.' },
    scheduled: { label: 'Scheduled', summary: 'This work is scheduled to start later.' },
    finished: { label: 'Finished', summary: 'This run finished. Review the results and reported checks below.' },
    stopped: { label: 'Stopped', summary: 'This run stopped before it could finish. Review its details below.' },
    paused: { label: 'Paused', summary: 'This work is paused. You can resume it when you are ready.' },
    ready: { label: 'Ready', summary: 'Your workspace is ready when you are.' },
  }
  const checksPassed = card.tests.filter((test) => test.passed).length
  const checksFailed = card.tests.length - checksPassed

  return {
    title: input.title?.trim() || 'Your work',
    status,
    statusLabel: copy[status].label,
    summary: copy[status].summary,
    filesTouched: card.files.length,
    checksReported: card.tests.length,
    checksPassed,
    checksFailed,
  }
}
