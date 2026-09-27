// P48.3 (E9) — desktop computer-use bridge (desktop_cmds.rs / agentcowork-
// computeruse). See / read / act on native windows through the effect funnel:
// every `act` is Guard-2 gated + Merkle-audited with human-gesture provenance,
// and risky classes fail closed on the Rust side.
//
// Human-gesture path only: the user drives this view directly (like the
// shell/git/office human path). In a plain-browser preview (no shell) the
// callers fall back to honest demo data so the surface is explorable.

import { inTauri, invoke } from './tauri'
import { nativeCall } from './runtime'

export interface DesktopCapabilities {
  see: string
  see_occluded: boolean
  uia_tree: boolean
  invoke_set_value: boolean
  send_input: boolean
  /** True when coordinate clicks can be delivered without moving the user's pointer. */
  background_input: boolean
  ocr: boolean
  window_list: boolean
  launch_app: boolean
}
export interface DesktopReadiness {
  state: string
  detail: string
  usable: boolean
}
export interface DesktopStatus {
  attached: boolean
  reason?: string | null
  interactionDefault?: 'background' | 'foreground'
  readiness?: DesktopReadiness
  capabilities?: DesktopCapabilities
}
export interface DesktopWindow {
  id: number
  title: string
  app: string
  x: number
  y: number
  width: number
  height: number
}
export type DesktopActKind = 'click' | 'clickByName' | 'type' | 'setValue' | 'launch'

/**
 * P57.4 — why a Background act cannot be delivered, and what escalating to
 * Foreground would cost. The engine never escalates silently: it returns this,
 * the UI renders the Guard-2 card, and only an explicit human gesture flips the
 * interaction mode for that one act.
 */
export interface EscalationRequest {
  reason: string
  blocked_act: unknown
  requires_gesture: boolean
  target: string
}

export async function desktopStatus(): Promise<DesktopStatus> {
  if (!inTauri()) return { attached: false, reason: 'requires desktop shell' }
  return nativeCall('desktop status', () => invoke<DesktopStatus>('desktop_status'))
}

export async function desktopAttach(): Promise<DesktopStatus> {
  if (!inTauri()) return { attached: false, reason: 'requires desktop shell' }
  return nativeCall('desktop attach', () => invoke<DesktopStatus>('desktop_attach'))
}

export async function desktopWindows(): Promise<DesktopWindow[]> {
  if (!inTauri()) return demoWindows()
  const r = await nativeCall('desktop windows', () => invoke<{ windows: DesktopWindow[] }>('desktop_windows'))
  return r.windows
}

export async function desktopRead(
  windowId: number,
): Promise<{ tree: string; has_tree: boolean; dpi_scale: number }> {
  if (!inTauri()) return { tree: '[0] Window "Demo"\n[1] Button "OK"', has_tree: true, dpi_scale: 1 }
  return nativeCall('desktop read', () => invoke('desktop_read', { windowId }))
}

export async function desktopSee(
  windowId: number,
): Promise<{ png: string; width: number; height: number }> {
  return nativeCall('desktop see', () => invoke('desktop_see', { windowId }))
}

/** Execute ONE human-initiated act. Risky classes fail closed on the Rust side. */
export async function desktopAct(
  windowId: number,
  kind: DesktopActKind,
  opts: { x?: number; y?: number; name?: string; text?: string } = {},
): Promise<{ ok: boolean; act: string }> {
  return nativeCall('desktop act', () => invoke('desktop_act', { windowId, kind, ...opts }))
}

/**
 * P57.4 — does this act need a foreground escalation under the current default?
 * Pure read: nothing moves. `null` means the act is deliverable as-is.
 */
export async function desktopEscalation(
  windowId: number,
  kind: DesktopActKind,
  opts: { x?: number; y?: number; name?: string; text?: string } = {},
): Promise<EscalationRequest | null> {
  if (!inTauri()) return null
  return nativeCall('desktop escalation', () =>
    invoke<EscalationRequest | null>('desktop_escalation', { windowId, kind, ...opts }),
  )
}

/**
 * P57.4 — run an act that needs a foreground escalation, **only** with an
 * explicit human gesture. Without one the Rust side returns a refusal carrying
 * the reason and raises nothing. With one, the previous foreground window is
 * snapshotted, the default flips to Foreground for this single act, and both are
 * restored afterwards (an honest restore failure is reported).
 */
export async function desktopActEscalating(
  windowId: number,
  kind: DesktopActKind,
  gestureApproved: boolean,
  opts: { x?: number; y?: number; name?: string; text?: string } = {},
): Promise<{ ok: boolean; act: string; escalated: boolean; restored: number | null }> {
  return nativeCall('desktop act escalating', () =>
    invoke('desktop_act_escalating', { windowId, kind, gestureApproved, ...opts }),
  )
}

export async function desktopStop(): Promise<{ stopped: boolean }> {
  if (!inTauri()) return { stopped: true }
  return nativeCall('desktop stop', () => invoke('desktop_stop'))
}

// ---- demo data (preview mode) ------------------------------------------

function demoWindows(): DesktopWindow[] {
  return [
    { id: 1, title: 'Untitled — TextEdit', app: 'TextEdit', x: 100, y: 80, width: 720, height: 480 },
    { id: 2, title: 'AgentCowork — Chromium', app: 'Chromium', x: 200, y: 120, width: 1200, height: 800 },
  ]
}

/** P59.8 — load the persisted CUA DAG. Preview / missing file → null (never a fake plan). */
export async function cuaDagGet(workId: string): Promise<{ ok: boolean; dag: unknown | null; reason?: string }> {
  if (!inTauri()) return { ok: true, dag: null, reason: 'preview — no CUA graph' }
  return nativeCall('cua dag get', () => invoke('cua_dag_get', { workId }))
}

/** P59.8 — edit a remaining DAG node (verified nodes are refused by Rust). */
export async function cuaDagEditRemaining(
  workId: string,
  nodeId: string,
  patch: { name?: string; info?: string },
): Promise<{ ok: boolean; replanSeq?: number; dag?: unknown }> {
  return nativeCall('cua dag edit remaining', () =>
    invoke('cua_dag_edit_remaining', { workId, nodeId, ...patch }),
  )
}
