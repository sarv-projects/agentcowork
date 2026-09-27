// P43 (B7 v3.53) — detached-work task ledger bridge. Thin wrappers over the
// Tauri `tasks_*` commands; in a plain-browser preview every call falls back
// to a demo set so the UI stays explorable. The durable state machine lives
// in Rust (`agentcowork-core::task_ledger`); these are just the wire + types.
// Push completion: the shell emits `task-update` on every terminal transition
// (registered at boot) — the rail re-fetches on that event, never polls.

import { invoke, listen } from "./tauri";
import { bridgeCall } from './runtime';

/** Mirror of the Rust `TaskKind` serde shape. */
export type TaskKind = "automation" | "subagent" | "acp" | "cli" | "scheduled";

/** Mirror of the Rust `TaskStatus` serde shape. */
export type TaskStatus =
  | "queued"
  | "running"
  | "succeeded"
  | "failed"
  | "timed_out"
  | "cancelled"
  | "lost";

/** Mirror of the Rust `DeliveryState` serde shape. Unit variants serialize as
 * plain strings (externally-tagged enum, P50.3.2 contract test pins this). */
export type DeliveryState =
  | "pending"
  | "delivered"
  | { blocked: { retries: number; deadline_ms: number } }
  | "dismissed";

/** Mirror of the Rust `TaskRecord` serde shape. */
export interface TaskRecord {
  id: string;
  kind: TaskKind;
  title: string;
  status: TaskStatus;
  requester: string | null;
  created_ms: number;
  started_ms: number | null;
  finished_ms: number | null;
  last_heartbeat_ms: number | null;
  error: string | null;
  retry_generation: number;
  delivery: DeliveryState;
  /** P51.12 cost join — vault-side spend stamped per row (serde defaults to
   * zero for pre-P51.12 mirrors; `tasks_list` re-joins at display time). */
  tokens_in?: number;
  tokens_out?: number;
  cost_usd?: number;
}

/** Demo records — the preview-mode fallback (mirror the real shapes). */
const DEMO_TASKS: TaskRecord[] = [
  {
    id: "task-000001",
    kind: "automation",
    title: "Morning brief",
    requester: "s-brief",
    status: "succeeded",
    created_ms: Date.now() - 32 * 60_000,
    started_ms: Date.now() - 31 * 60_000,
    finished_ms: Date.now() - 29 * 60_000,
    last_heartbeat_ms: Date.now() - 29 * 60_000,
    error: null,
    retry_generation: 0,
    delivery: "delivered",
  },
  {
    id: "task-000002",
    kind: "subagent",
    title: "Research: competitor pricing",
    requester: "s-research",
    status: "running",
    created_ms: Date.now() - 4 * 60_000,
    started_ms: Date.now() - 3 * 60_000,
    finished_ms: null,
    last_heartbeat_ms: Date.now() - 20_000,
    error: null,
    retry_generation: 0,
    delivery: "pending",
  },
  {
    id: "task-000003",
    kind: "acp",
    title: "Claude Code: fix TS build",
    requester: "s-ci",
    status: "failed",
    created_ms: Date.now() - 90 * 60_000,
    started_ms: Date.now() - 89 * 60_000,
    finished_ms: Date.now() - 84 * 60_000,
    last_heartbeat_ms: Date.now() - 84 * 60_000,
    error: "Timeout after 3 retries",
    retry_generation: 1,
    delivery: { blocked: { retries: 2, deadline_ms: Date.now() + 30 * 60_000 } },
  },
];

export async function tasksList(status?: string): Promise<TaskRecord[]> {
  return bridgeCall({
    operation: 'task list',
    live: async () => {
      const out = await invoke<unknown>('tasks_list', { status });
      if (!Array.isArray(out)) throw new Error('task list returned an invalid response');
      return out as TaskRecord[];
    },
    preview: () => DEMO_TASKS,
  });
}

export async function tasksShow(id: string): Promise<TaskRecord | null> {
  return bridgeCall({
    operation: 'task show',
    live: async () => {
      try {
        return await invoke<TaskRecord>("tasks_show", { id });
      } catch (error) {
        // A missing task is a valid domain result; all other native failures
        // must remain visible to the caller/runtime policy.
        if (String(error).toLowerCase().includes('not found')) return null;
        throw error;
      }
    },
    preview: () => DEMO_TASKS.find((task) => task.id === id) ?? null,
  });
}

export async function tasksCancel(id: string): Promise<boolean> {
  return bridgeCall({
    operation: 'task cancel',
    live: async () => {
      await invoke<unknown>("tasks_cancel", { id });
      return true;
    },
    preview: () => true,
  });
}

export async function tasksRetry(id: string): Promise<string | null> {
  return bridgeCall({
    operation: 'task retry',
    live: async () => {
      const out = await invoke<{ id: string }>("tasks_retry", { id });
      if (!out?.id) throw new Error('task retry returned no task id');
      return out.id;
    },
    preview: () => id,
  });
}

export async function tasksEnqueue(args: {
  kind: TaskKind;
  title: string;
  requester?: string;
}): Promise<string | null> {
  return bridgeCall({
    operation: 'task enqueue',
    live: async () => {
      const out = await invoke<{ id: string }>("tasks_enqueue", args);
      if (!out?.id) throw new Error('task enqueue returned no task id');
      return out.id;
    },
    preview: () => 'preview-task',
  });
}

/** Push completion: fires `task-update` on every terminal transition.
 * Returns a promise of the unlisten function (Tauri's `listen` shape). */
export function onTaskUpdate(cb: (record: TaskRecord) => void): Promise<() => void> {
  return listen<TaskRecord>("task-update", (event) => cb(event.payload));
}

const STATUS_LABEL: Record<TaskStatus, string> = {
  queued: "Queued",
  running: "Running",
  succeeded: "Succeeded",
  failed: "Failed",
  timed_out: "Timed out",
  cancelled: "Cancelled",
  lost: "Lost",
};

export function taskStatusLabel(s: TaskStatus): string {
  return STATUS_LABEL[s];
}

const KIND_LABEL: Record<TaskKind, string> = {
  automation: "Automation",
  subagent: "Subagent",
  acp: "ACP",
  cli: "CLI",
  scheduled: "Scheduled",
};

export function taskKindLabel(k: TaskKind): string {
  return KIND_LABEL[k];
}
