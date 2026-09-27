import { invoke, inTauri } from './tauri'
import { nativeCall } from './runtime'

export interface WorkAddress {
  workId: string
  projectId?: string
  sessionId?: string
  /** P71.8a — the owning Session's kind (`ADR-0006`); a record property, never
   * inferred from whether a Chat exists. */
  sessionKind?: 'interactive' | 'automation' | 'delegated'
  ownerId?: string
  nodeId?: string
  currentRunId?: string
  version: number
}

/** P71.8c — non-interactive Sessions surface **through their owner**: an
 * automation run appears in the Automation screen's run list, a delegated
 * Session in the Activity/work timeline. They never invent a Chat. */
export function sessionOwnerSurface(
  kind: WorkAddress['sessionKind'],
): 'chat' | 'automation' | 'activity' {
  switch (kind) {
    case 'automation': return 'automation'
    case 'delegated': return 'activity'
    default: return 'chat'
  }
}

/** P71.9f — the **user-facing** label for a Work's owning Session, in the
 * cockpit's own vocabulary (`ARCH/UI.md` §5): the container word is Chat, and
 * a kind is never shown as the word "Session". An automation run reads as an
 * automation, a child of an out-of-session delegation as delegated. */
export function sessionKindLabel(kind: WorkAddress['sessionKind']): string {
  switch (kind) {
    case 'automation': return 'automation'
    case 'delegated': return 'delegated'
    default: return 'chat'
  }
}

export interface WorkPresence {
  workId: string
  activeClients: string[]
  activeNodes: string[]
  activeRun?: string
  currentSurface?: string
  state?: string
}

export interface WorkEventEnvelope {
  workId: string
  sequence: number
  eventId: string
  event: WorkEvent
  timestamp: number
  traceId?: string
  causalParent?: number
}

// ---------------------------------------------------------------------------
// P51.14 — typed mirror of the Rust `WorkEvent` family (work_gateway.rs).
// `WorkEvent` is tagged by `class`; each class is tagged by `kind` with a
// `data` payload. The previous `event: unknown` mirror forced the timeline
// to guess labels — these types render the real step/file/test/artifact
// surfaces.
// ---------------------------------------------------------------------------

export type DomainEvent = {
  kind: 'work_created'
  data: { objective: string; projectId?: string; sessionId?: string }
} | {
  kind: 'work_updated'
  data: { patch: Record<string, unknown> }
} | {
  kind: 'run_queued' | 'run_started' | 'run_paused' | 'run_completed'
  data: { runId: string }
} | {
  kind: 'run_checkpointed'
  data: { runId: string; checkpoint: number }
} | {
  kind: 'run_waiting'
  data: { runId: string; reason: string; wait?: { reason: string; detail?: string; deadlineMs?: number; resumeOn?: string } }
} | {
  kind: 'run_interrupted'
  data: { runId: string; reason?: string }
} | {
  kind: 'run_failed'
  data: { runId: string; reason: string }
} | {
  kind: 'run_cancelled'
  data: { runId: string }
} | {
  kind: 'approval_requested'
  data: { ticketId: string }
} | {
  kind: 'approval_resolved'
  data: { ticketId: string; approved: boolean }
} | {
  kind: 'effect_attempted'
  data: { effectId: string; capabilityGrantId?: string }
} | {
  kind: 'effect_observed'
  data: { effectId: string; outcome: string }
} | {
  kind: 'effect_verified'
  data: { effectId: string; verified: boolean }
} | {
  kind: 'artifact_created' | 'artifact_updated'
  data: { artifactId: string }
} | {
  kind: 'review_requested'
  data: { reviewId: string }
}

export type OperationalEvent = {
  kind: 'tool_requested' | 'tool_started' | 'tool_completed'
  data: { toolId: string }
} | {
  kind: 'tool_failed'
  data: { toolId: string; error: string }
} | {
  kind: 'node_connected' | 'node_disconnected'
  data: { nodeId: string }
} | {
  kind: 'session_attached' | 'session_detached'
  data: { clientId: string }
} | {
  kind: 'file_touched'
  data: { path: string; writer_id: string }
} | {
  kind: 'test_ran'
  data: { name: string; passed: boolean }
} | {
  kind: 'write_conflict'
  data: { path: string; writers: string[] }
} | {
  kind: 'handoff_recorded'
  data: { artifact_id: string; from_agent: string; to_agent: string; summary: string }
}

export type PresenceEvent = {
  kind: 'presence_changed'
  data: { presence: WorkPresence }
} | {
  kind: 'agent_thought_summary'
  data: { text: string }
}

export type RuntimeEvent = {
  kind: 'pty_started'
  data: { ptyId: string; processId?: number; rows: number; cols: number }
} | {
  kind: 'pty_output'
  data: { ptyId: string; chunk: string }
} | {
  kind: 'pty_resize'
  data: { ptyId: string; rows: number; cols: number }
} | {
  kind: 'pty_signal'
  data: { ptyId: string; signal: string }
} | {
  kind: 'pty_exit'
  data: { ptyId: string; code?: number }
} | {
  kind: 'worktree_created'
  data: { worktreeId: string; branch: string }
} | {
  kind: 'worktree_attached'
  data: { worktreeId: string; runId: string }
} | {
  kind: 'worktree_merged'
  data: { worktreeId: string; into: string }
} | {
  kind: 'worktree_reverted' | 'worktree_destroyed'
  data: { worktreeId: string }
} | {
  kind: 'agent_session_spawned'
  data: { agentSessionId: string; agentId: string; lifetime: string }
} | {
  kind: 'agent_session_message'
  data: { agentSessionId: string; direction: string }
} | {
  kind: 'agent_session_attached' | 'agent_session_detached' | 'agent_session_steered' | 'agent_session_terminated'
  data: { agentSessionId: string }
} | {
  kind: 'agent_session_checkpointed'
  data: { agentSessionId: string; checkpoint: number }
}

export type WorkEvent =
  | { class: 'domain'; event: DomainEvent }
  | { class: 'operational'; event: OperationalEvent }
  | { class: 'presence'; event: PresenceEvent }
  | { class: 'runtime'; event: RuntimeEvent }

/** Human-readable label for a WorkPresenceState (wire: snake_case). */
export function presenceLabel(state: string | undefined): string {
  switch (state) {
    case 'running': return 'Running'
    case 'waiting_for_user': return 'Waiting for you'
    case 'waiting_for_approval': return 'Waiting for approval'
    case 'blocked': return 'Blocked'
    case 'completed': return 'Completed'
    case 'failed': return 'Failed'
    case 'cancelled': return 'Cancelled'
    case 'offline': return 'Offline'
    case 'reconnecting': return 'Reconnecting'
    default: return state ?? 'Connected'
  }
}

/** P71.3g — the canonical Work lifecycle states (`ARCH/WORK.md` §4); Rust's
 * `agentcowork_types::WorkState` is the authority, this is the wire projection. */
export type WorkLifecycleState =
  | 'created' | 'planning' | 'ready' | 'running'
  | 'waiting_tool' | 'waiting_approval' | 'waiting_user' | 'checkpointed'
  | 'verifying' | 'completed' | 'failed' | 'cancelled' | 'paused' | 'recoverable'

/** Human label for a Work lifecycle state. */
export function workStateLabel(state: string | undefined): string {
  switch (state) {
    case 'created': return 'Created'
    case 'planning': return 'Planning'
    case 'ready': return 'Ready'
    case 'running': return 'Running'
    case 'waiting_tool': return 'Running a tool'
    case 'waiting_approval': return 'Waiting for approval'
    case 'waiting_user': return 'Waiting for you'
    case 'checkpointed': return 'Checkpointed'
    case 'verifying': return 'Verifying'
    case 'completed': return 'Completed'
    case 'failed': return 'Failed'
    case 'cancelled': return 'Cancelled'
    case 'paused': return 'Paused'
    // Recoverable is resumable with knowns and unknowns — never "failed".
    case 'recoverable': return 'Interrupted — resumable'
    default: return state ?? 'Unknown'
  }
}

/** Human label for a wait reason (`AUTOMATION.md` §8). */
export function waitReasonLabel(reason: string | undefined): string {
  switch (reason) {
    case 'approval': return 'approval pending'
    case 'user_input': return 'waiting for your answer'
    case 'timer': return 'on a timer'
    case 'external_event': return 'waiting for an event'
    case 'resource': return 'waiting for a resource'
    case 'agent': return 'waiting on another agent'
    case 'retry': return 'retrying shortly'
    default: return reason ?? 'unknown wait'
  }
}

export interface WorkEventDescription {
  /** Short headline shown on the timeline card. */
  label: string
  /** Optional secondary detail (tool args, outcome, reason…). */
  detail?: string
  /** Timeline tone drives the icon + accent. */
  tone: 'step' | 'tool' | 'file' | 'run' | 'approval' | 'thought' | 'session' | 'node' | 'worktree' | 'pty' | 'review' | 'conflict' | 'handoff' | 'test'
  /** done | active | failed — drives the status dot. */
  status: 'done' | 'active' | 'failed'
}

/** Render a typed WorkEvent into a timeline card description (P51.14). */
export function describeWorkEvent(envelope: WorkEventEnvelope): WorkEventDescription {
  const w = envelope.event
  const ev = w.event
  switch (w.class) {
    case 'domain':
      switch (ev.kind) {
        case 'work_created':
          return { label: 'Work created', detail: ev.data.objective || undefined, tone: 'session', status: 'done' }
        case 'work_updated':
          return { label: 'Work updated', tone: 'run', status: 'done' }
        case 'run_started':
          return { label: `Run ${shortId(ev.data.runId)} started`, tone: 'run', status: 'active' }
        case 'run_queued':
          return { label: `Run ${shortId(ev.data.runId)} queued`, tone: 'run', status: 'done' }
        case 'run_checkpointed':
          return { label: `Run ${shortId(ev.data.runId)} checkpoint ${ev.data.checkpoint}`, tone: 'run', status: 'done' }
        case 'run_waiting':
          return {
            label: `Run ${shortId(ev.data.runId)} waiting — ${waitReasonLabel(ev.data.wait?.reason ?? ev.data.reason)}`,
            detail: ev.data.wait?.resumeOn ?? ev.data.wait?.detail,
            tone: 'approval',
            status: 'active',
          }
        case 'run_interrupted':
          // Recoverable: the effect outcome is unknown — say so, never "failed".
          return { label: `Run ${shortId(ev.data.runId)} interrupted — resumable`, detail: ev.data.reason, tone: 'run', status: 'active' }
        case 'run_paused':
          return { label: `Run ${shortId(ev.data.runId)} paused`, tone: 'run', status: 'active' }
        case 'run_completed':
          return { label: `Run ${shortId(ev.data.runId)} completed`, tone: 'run', status: 'done' }
        case 'run_failed':
          return { label: `Run ${shortId(ev.data.runId)} failed`, detail: ev.data.reason, tone: 'run', status: 'failed' }
        case 'run_cancelled':
          return { label: `Run ${shortId(ev.data.runId)} cancelled`, tone: 'run', status: 'failed' }
        case 'approval_requested':
          return { label: 'Approval requested', detail: shortId(ev.data.ticketId), tone: 'approval', status: 'active' }
        case 'approval_resolved':
          return { label: ev.data.approved ? 'Approval granted' : 'Approval refused', detail: shortId(ev.data.ticketId), tone: 'approval', status: ev.data.approved ? 'done' : 'failed' }
        case 'effect_attempted':
          return { label: `Effect ${shortId(ev.data.effectId)} attempted`, tone: 'step', status: 'active' }
        case 'effect_observed':
          return { label: `Effect ${shortId(ev.data.effectId)} observed`, detail: ev.data.outcome, tone: 'step', status: 'done' }
        case 'effect_verified':
          return { label: `Effect ${shortId(ev.data.effectId)} verified`, tone: 'step', status: ev.data.verified ? 'done' : 'failed' }
        case 'artifact_created':
          return { label: `Artifact ${shortId(ev.data.artifactId)} created`, tone: 'file', status: 'done' }
        case 'artifact_updated':
          return { label: `Artifact ${shortId(ev.data.artifactId)} updated`, tone: 'file', status: 'done' }
        case 'review_requested':
          return { label: 'Review requested', detail: shortId(ev.data.reviewId), tone: 'review', status: 'active' }
      }
      break
    case 'operational':
      switch (ev.kind) {
        case 'tool_requested':
          return { label: `Tool ${ev.data.toolId} requested`, tone: 'tool', status: 'active' }
        case 'tool_started':
          return { label: `Tool ${ev.data.toolId} started`, tone: 'tool', status: 'active' }
        case 'tool_completed':
          return { label: `Tool ${ev.data.toolId} completed`, tone: 'tool', status: 'done' }
        case 'tool_failed':
          return { label: `Tool ${ev.data.toolId} failed`, detail: ev.data.error, tone: 'tool', status: 'failed' }
        case 'node_connected':
        case 'node_disconnected':
          return { label: `Node ${shortId(ev.data.nodeId)} ${ev.kind === 'node_connected' ? 'connected' : 'disconnected'}`, tone: 'node', status: ev.kind === 'node_connected' ? 'done' : 'failed' }
        case 'session_attached':
        case 'session_detached':
          return { label: `Client ${shortId(ev.data.clientId)} ${ev.kind === 'session_attached' ? 'attached' : 'detached'}`, tone: 'session', status: 'done' }
        case 'file_touched':
          return { label: `File ${ev.data.path}`, detail: ev.data.writer_id, tone: 'file', status: 'done' }
        case 'test_ran':
          return { label: `Test ${ev.data.name}`, detail: ev.data.passed ? 'passed' : 'failed', tone: 'test', status: ev.data.passed ? 'done' : 'failed' }
        case 'write_conflict':
          return { label: `Conflict on ${ev.data.path}`, detail: ev.data.writers.join(', '), tone: 'conflict', status: 'failed' }
        case 'handoff_recorded':
          return { label: `Handoff ${ev.data.from_agent} → ${ev.data.to_agent}`, detail: ev.data.summary, tone: 'handoff', status: 'done' }
      }
      break
    case 'presence':
      if (ev.kind === 'agent_thought_summary') {
        const text = ev.data.text.length > 140 ? `${ev.data.text.slice(0, 140)}…` : ev.data.text
        return { label: text, detail: ev.data.text, tone: 'thought', status: 'active' }
      }
      return { label: 'Presence changed', tone: 'session', status: 'done' }
    case 'runtime':
      switch (ev.kind) {
        case 'pty_started':
          return { label: `PTY ${shortId(ev.data.ptyId)} started`, tone: 'pty', status: 'active' }
        case 'pty_output':
          return { label: `PTY ${shortId(ev.data.ptyId)} output`, tone: 'pty', status: 'active' }
        case 'pty_exit':
          return { label: `PTY ${shortId(ev.data.ptyId)} exited${ev.data.code !== undefined ? ` (${ev.data.code})` : ''}`, tone: 'pty', status: 'done' }
        case 'pty_resize':
        case 'pty_signal':
          return { label: `PTY ${shortId(ev.data.ptyId)} ${ev.kind === 'pty_resize' ? 'resized' : 'signalled'}`, tone: 'pty', status: 'done' }
        case 'worktree_created':
          return { label: `Worktree ${shortId(ev.data.worktreeId)} created`, detail: ev.data.branch, tone: 'worktree', status: 'done' }
        case 'worktree_attached':
          return { label: `Worktree ${shortId(ev.data.worktreeId)} attached`, tone: 'worktree', status: 'done' }
        case 'worktree_merged':
          return { label: `Worktree ${shortId(ev.data.worktreeId)} merged into ${ev.data.into}`, tone: 'worktree', status: 'done' }
        case 'worktree_reverted':
          return { label: `Worktree ${shortId(ev.data.worktreeId)} reverted`, tone: 'worktree', status: 'done' }
        case 'worktree_destroyed':
          return { label: `Worktree ${shortId(ev.data.worktreeId)} destroyed`, tone: 'worktree', status: 'done' }
        case 'agent_session_spawned':
          return { label: `Agent run ${shortId(ev.data.agentSessionId)} spawned`, detail: `${ev.data.agentId} (${ev.data.lifetime})`, tone: 'session', status: 'active' }
        case 'agent_session_message':
          return { label: `Agent message ${ev.data.direction === 'toAgent' ? 'sent' : 'received'}`, detail: shortId(ev.data.agentSessionId), tone: 'session', status: 'done' }
        case 'agent_session_attached':
          return { label: `Agent run ${shortId(ev.data.agentSessionId)} attached`, tone: 'session', status: 'active' }
        case 'agent_session_detached':
          return { label: `Agent run ${shortId(ev.data.agentSessionId)} detached`, tone: 'session', status: 'done' }
        case 'agent_session_steered':
          return { label: `Agent run ${shortId(ev.data.agentSessionId)} steered`, tone: 'session', status: 'active' }
        case 'agent_session_terminated':
          return { label: `Agent run ${shortId(ev.data.agentSessionId)} terminated`, tone: 'session', status: 'done' }
        case 'agent_session_checkpointed':
          return { label: `Agent run ${shortId(ev.data.agentSessionId)} checkpoint ${ev.data.checkpoint}`, tone: 'session', status: 'done' }
      }
      break
  }
  return { label: `Work event #${envelope.sequence}`, tone: 'run', status: 'done' }
}

function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 10)}…` : id
}

export interface WorkSnapshot {
  address: WorkAddress
  presence: WorkPresence
  events: WorkEventEnvelope[]
  clients: unknown[]
  nodes: unknown[]
  reviews: unknown[]
}

export async function workList(): Promise<WorkAddress[]> {
  if (!inTauri()) return []
  return nativeCall('work list', () => invoke<WorkAddress[]>('work_list'))
}

export async function workSnapshot(workId: string): Promise<WorkSnapshot | null> {
  if (!inTauri()) return null
  return nativeCall('work snapshot', () => invoke<WorkSnapshot | null>('work_snapshot', { workId }))
}

export async function workEvents(workId: string, fromSequence = 0): Promise<WorkEventEnvelope[]> {
  if (!inTauri()) return []
  return nativeCall('work events', () => invoke<WorkEventEnvelope[]>('work_events', { workId, fromSequence }))
}

export async function workPresence(workId: string): Promise<WorkPresence | null> {
  if (!inTauri()) return null
  return nativeCall('work presence', () => invoke<WorkPresence | null>('work_presence', { workId }))
}

export async function workReviews(workId: string): Promise<unknown[]> {
  if (!inTauri()) return []
  return nativeCall('work reviews', () => invoke<unknown[]>('work_reviews', { workId }))
}

// --- P49.10–12 session-runtime lifecycle -----------------------------------

export interface PtySession {
  pty_id: string
  process_id?: number
  rows: number
  cols: number
  state: string
  output: string
}
export interface AgentSession {
  agentSessionId: string
  workId: string
  runId: string
  agentId: string
  lifetime: string
  ptyId?: string
  worktreeId?: string
  runtimeState: string
  lastCheckpoint: number
  attached: boolean
}

export async function workPtySpawn(workId: string, ptyId: string, rows = 24, cols = 80, processId?: number) {
  return nativeCall('work PTY spawn', () => invoke('work_pty_spawn', { workId, ptyId, processId, rows, cols }))
}
export async function workPtyResize(workId: string, ptyId: string, rows: number, cols: number) {
  return nativeCall('work PTY resize', () => invoke('work_pty_resize', { workId, ptyId, rows, cols }))
}
export async function workPtySignal(workId: string, ptyId: string, signal: string) {
  return nativeCall('work PTY signal', () => invoke('work_pty_signal', { workId, ptyId, signal }))
}
export async function workPtyClose(workId: string, ptyId: string, code?: number) {
  return nativeCall('work PTY close', () => invoke('work_pty_close', { workId, ptyId, code }))
}
export async function workPtySnapshot(ptyId: string): Promise<PtySession | null> {
  if (!inTauri()) return null
  return nativeCall('work PTY snapshot', () => invoke<PtySession | null>('work_pty_snapshot', { ptyId }))
}

export async function workWorktreeCreate(args: {
  workId: string; runId: string; worktreeId: string; repoRoot: string;
  worktreeRoot: string; baseRevision: string; branch: string; isolationMode?: string
}) {
  return nativeCall('work worktree create', () => invoke('work_worktree_create', args))
}
export async function workWorktreeAttach(workId: string, worktreeId: string, runId: string) {
  return nativeCall('work worktree attach', () => invoke('work_worktree_attach', { workId, worktreeId, runId }))
}
export async function workWorktreeOp(workId: string, worktreeId: string, op: 'merge' | 'revert' | 'destroy', into?: string) {
  return nativeCall('work worktree operation', () => invoke('work_worktree_op', { workId, worktreeId, op, into }))
}

export async function workAgentSpawn(args: {
  workId: string; runId: string; agentSessionId: string; agentId: string;
  lifetime: 'ephemeral' | 'persistent'; ptyId?: string; worktreeId?: string
}) {
  return nativeCall('work agent spawn', () => invoke('work_agent_spawn', args))
}
export async function workAgentOp(workId: string, agentSessionId: string, op: 'attach' | 'detach' | 'steer' | 'checkpoint' | 'terminate') {
  return nativeCall('work agent operation', () => invoke('work_agent_op', { workId, agentSessionId, op }))
}
export async function workAgentSessions(workId: string): Promise<AgentSession[]> {
  if (!inTauri()) return []
  return nativeCall('work agent sessions', () => invoke<AgentSession[]>('work_agent_sessions', { workId }))
}

/** P71.9d — child Works below one Work (the delegation tree), read-only. */
export interface ChildWork {
  workId: string
  parentWorkId?: string
  sessionId?: string
  sessionKind?: 'interactive' | 'automation' | 'delegated'
  currentRunId?: string
  version: number
}
export async function workChildren(workId: string): Promise<ChildWork[]> {
  if (!inTauri()) return []
  return nativeCall('work children', () => invoke<ChildWork[]>('work_children', { workId }))
}
