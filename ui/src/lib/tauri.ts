// Tauri IPC bridge (P0.7). `invoke` proxies the Tauri v2 command bridge.
// Preview fixtures are selected explicitly by each bridge; a native rejection
// is never converted into preview data or success.
//
// P1.4: chat streaming — `chat_stream` dispatches a turn through the Rust core
// (→ coordinator engine → broker), and `chat-event` emits carry the streamed
// deltas (ttft/batch/done/error/cancelled/budgetExceeded/interrupt/planDone)
// to the UI. P6.3 Stage-0: `plan_execute` runs a blueprint plan through the
// coordinator's plan executor; `plan_respond` returns a circuit-break card
// choice (the interrupt path).

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { nativeCall } from './runtime';

export { listen };

export type UnlistenFn = () => void;

/** True when running inside the Tauri webview (v2 sets this global). */
export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Invoke a Rust command through the Tauri bridge. */
export async function invoke<T = unknown>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return nativeCall(`Tauri command ${cmd}`, () => tauriInvoke<T>(cmd, args));
}

/** Wire events from the Rust chat relay (camelCase mirror of ChatWireEvent). */
export interface ChatWireEvent {
  type:
    | "ttft"
    | "batch"
    | "reasoning"
    | "stage"
    | "toolCall"
    | "toolResult"
    | "done"
    | "error"
    | "cancelled"
    | "budgetExceeded"
    | "interrupt"
    | "planStart"
    | "planStep"
    | "memoryExtracted"
    | "planDone"
    | "monitor"
    | "verification"
    | "citations"
    | "walkthrough";
  streamId?: string;
  latencyMs?: number;
  text?: string;
  tokenCount?: number;
  turnId?: string;
  fullText?: string;
  totalTokens?: number;
  code?: string;
  message?: string;
  stage?: string;
  toolId?: string;
  args?: Record<string, unknown>;
  result?: unknown;
  retryable?: boolean;
  risk?: string;
  error?: string;
  sessionId?: string;
  limit?: number;
  spent?: number;
  /** Plan lifecycle fields share the chat-event envelope. */
  tasks?: number;
  status?: string;
  facts?: string[];
  /** P6.3 Stage-0 — circuit-break interrupt card payload. */
  planId?: string;
  breakId?: string;
  options?: string[];
  tasksDone?: number;
  title?: string;
  description?: string;
  jobId?: string;
  changed?: boolean;
  notified?: boolean;
  stopped?: boolean;
  current?: string;
  notifications?: number;
  /** P41.4 — K1 verification receipt (Diff rail). */
  taskId?: string;
  /** Canonical execution correlation. Native events always provide this for
   * live streams; consumers must never infer it from the active tab. */
  workId?: string;
  executionId?: string;
  runId?: string;
  eventId?: string;
  sequence?: number;
  schemaVersion?: number;
  timestamp?: number;
  checks?: string[];
  report?: string;
  passed?: boolean | null;
  /** P52.20 — numbered citations from live search hits. */
  citations?: Array<{ index: number; title: string; url: string; snippet?: string; source?: string }>;
  /** P51.10 — Changes Walkthrough stops from `execution/multirun`. */
  stops?: unknown[];
}

/** Pause every scheduled job bound to a chat (delete-session cascade). */
export async function schedulerPauseSession(sessionId: string): Promise<number> {
  return invoke<number>("scheduler_pause_session", { sessionId });
}

// P71.2c — `chatToolRetry`, `planExecute` and `planRespond` are deleted with the
// built-in engine (ADR-0005 §2): they dispatched a turn to the coordinator's
// loop, which no longer exists. Tool retry and plan **execution** therefore
// return with the governed binding (post-v1, P71.7), while the plan *draft* the
// composer renders stays read-only. Callers say so instead of invoking a
// command that would 404.

/** Subscribe to chat events; returns an unsubscribe function. */
export async function onChatEvent(
  cb: (e: ChatWireEvent) => void,
): Promise<() => void> {
  return listen<ChatWireEvent>("chat-event", (event) => cb(event.payload));
}

export interface RuntimeStatus {
  vault: 'ready' | 'setup' | 'locked' | 'unknown'
  sidecar: boolean
  persistence: 'durable' | 'ephemeral' | 'unknown'
}

/** Read-only shell readiness probe used by the canonical runtime state. */
export async function runtimeStatus(): Promise<RuntimeStatus> {
  return invoke<RuntimeStatus>('runtime_status')
}

/** P70.A2 — why is the sidecar offline? `missing` is a broken install (or an
 * unbuilt dev sidecar) and carries the actionable remedy in `detail`. */
export interface SidecarProbe {
  connected: boolean;
  state: 'connected' | 'connecting' | 'missing';
  detail: string;
}

export async function sidecarProbe(): Promise<SidecarProbe> {
  return invoke<SidecarProbe>('sidecar_probe')
}

// ---------------------------------------------------------------------------
// P3.2 — cockpit / ambient flight-deck (H2, doc 33 §9.5).
// ---------------------------------------------------------------------------

/** One live agent card (mirrors `agentcowork_audit::cockpit::AgentCard`). */
export interface AgentCard {
  agent_id: string;
  display_name: string;
  model: string;
  status: string; // Running | Done | Waiting | Idle
  last_tool: string;
  last_summary: string;
  last_ts_ms: number;
  tokens_in: number;
  tokens_out: number;
}

/** An open interrupt the user must answer. */
export interface InterruptCard {
  agent_id: string;
  kind: string; // approval | mcq | stop
  prompt: string;
  options: string[];
}

/** Full cockpit snapshot (mirrors `CockpitState`). */
export interface CockpitState {
  agents: AgentCard[];
  interrupts: InterruptCard[];
  quiet: boolean;
}

/** Poll the live cockpit state (agent cards + interrupts + quiet flag). */
export async function cockpitSnapshot(): Promise<CockpitState> {
  if (!inTauri()) return demoCockpit();
  return invoke<CockpitState>("cockpit_snapshot");
}

/** STOP: kill the agent loop (control-channel `agent/stop`). */
export async function agentStop(sessionId: string): Promise<void> {
  if (!inTauri()) return;
  return invoke<void>("agent_stop", { sessionId });
}

/** UNDO: request revert of the last action (control-channel `agent/undo`). */
export async function agentUndo(sessionId: string): Promise<void> {
  if (!inTauri()) return;
  return invoke<void>("agent_undo", { sessionId });
}

function demoCockpit(): CockpitState {
  const now = Date.now();
  return {
    agents: [
      {
        agent_id: "agent-1",
        display_name: "Default Agent",
        model: "claude-sonnet-4",
        status: "Running",
        last_tool: "file.open",
        last_summary: "Opened Q3-Financials.xlsx",
        last_ts_ms: now - 2_000,
        tokens_in: 12_400,
        tokens_out: 3_200,
      },
      {
        agent_id: "agent-2",
        display_name: "Research Sub-Agent",
        model: "gpt-4o",
        status: "Waiting",
        last_tool: "browser.search",
        last_summary: "Searching competitor pricing…",
        last_ts_ms: now - 8_000,
        tokens_in: 4_100,
        tokens_out: 980,
      },
    ],
    interrupts: [
      {
        agent_id: "agent-1",
        kind: "approval",
        prompt: "Allow shell.exec `npm test`?",
        options: ["Allow", "Deny"],
      },
    ],
    quiet: false,
  };
}
