// P6.4 (B7/H14) — scheduled-task bridge. Thin wrappers over the Tauri
// `scheduler_*` commands; in a plain-browser preview every call falls back to
// the demo set so the UI stays explorable. The scheduler is a **trigger
// plane** (P71.3d — `ARCH/AUTOMATION.md` §9): definitions, triggers and
// occurrence records only — no leases, retries or run ledger. Run history is
// the Event Log's; these are just the wire + types.

import { invoke, inTauri } from "./tauri";
import { bridgeCall } from "./runtime";

/**
 * A caller-owned scheduler delivery identity. Create it once for a logical
 * action and pass the same value to every retry.
 */
export type SchedulerRequestKey = string;

const schedulerRetryKeys = new Map<string, SchedulerRequestKey>();
let schedulerKeyCounter = 0;

function normalizeSchedulerRequestKey(value: string): SchedulerRequestKey {
  const key = value.trim();
  if (!key || key.length > 256 || /[\u0000-\u001f\u007f]/u.test(key)) {
    throw new Error("scheduler ingress requires a valid caller idempotency key");
  }
  return key;
}

/**
 * Create a fresh UI-side request key without using the wall clock or a
 * payload digest. The caller must retain it for retries.
 */
export function createSchedulerRequestKey(prefix = "ui"): SchedulerRequestKey {
  const randomUUID = (globalThis.crypto as (Crypto & { randomUUID?: () => string }) | undefined)
    ?.randomUUID;
  if (randomUUID) return `${prefix}:${randomUUID.call(globalThis.crypto)}`;

  // Older webviews without randomUUID still get a process-local unique value;
  // this is an identity fallback, never a timestamp-derived key.
  schedulerKeyCounter = (schedulerKeyCounter + 1) >>> 0;
  return `${prefix}:${schedulerKeyCounter.toString(36)}-${Math.random().toString(36).slice(2)}`;
}

function schedulerKeyFor(operation: string, supplied?: SchedulerRequestKey): SchedulerRequestKey {
  if (supplied !== undefined) return normalizeSchedulerRequestKey(supplied);
  const existing = schedulerRetryKeys.get(operation);
  if (existing) return existing;
  const key = createSchedulerRequestKey("ui");
  schedulerRetryKeys.set(operation, key);
  return key;
}

function settleSchedulerKey(operation: string, key: SchedulerRequestKey, ok: boolean): void {
  // A failed native request keeps its key for a retry. A successful one-shot
  // operation releases it so a later deliberate Run Now gets a distinct key.
  if (ok && schedulerRetryKeys.get(operation) === key) {
    schedulerRetryKeys.delete(operation);
  }
}

/** Mirror of the Rust `TriggerSpec` serde shape. */
export type SchedulerTrigger =
  | { type: "cron"; expr: string }
  | { type: "interval"; secs: number }
  | { type: "event"; kind: string; filter: string }
  | { type: "webhook"; path: string; schema: string[] };

/** Mirror of the Rust trigger-plane `Job` serde shape. */
export interface SchedulerJob {
  id: string;
  name: string;
  sessionId: string;
  trigger: SchedulerTrigger;
  steps: unknown[];
  policy: {
    suppressOnBattery: boolean;
    maxRunsPerHour?: number;
    scope?: string;
  };
  enabled: boolean;
  /** Trigger-plane pause: stop firing without losing the definition. */
  paused: boolean;
  nextRunAt?: number;
  /** Last firing (occurrence record — "why did this run?"). */
  lastFiredAt?: number;
  /** Rolling 1h firing timestamps (frequency admission). */
  recentFires: number[];
}

export interface SchedulerList {
  jobs: SchedulerJob[];
  onBattery: boolean;
}

export interface NudgeSuggestion {
  goal: string;
  cron: string;
  confidence: number;
  observedAt: string[];
}

/** Demo jobs — the preview-mode fallback (mirror the real trigger-plane shape). */
const DEMO_JOBS: SchedulerJob[] = [
  {
    id: "j-daily-brief",
    name: "Morning brief",
    sessionId: "s-brief",
    trigger: { type: "cron", expr: "0 8 * * *" },
    steps: [{ step: "online_search", query: "latest AI news" }],
    policy: { suppressOnBattery: true, maxRunsPerHour: 1 },
    enabled: true,
    paused: false,
    recentFires: [],
  },
  {
    id: "j-ci-fixer",
    name: "CI Fixer",
    sessionId: "s-ci",
    trigger: { type: "event", kind: "ci_build_fail", filter: "" },
    steps: [{ step: "run_code", language: "bash", code: "# fix the build" }],
    policy: { suppressOnBattery: false, maxRunsPerHour: 4 },
    enabled: true,
    paused: false,
    recentFires: [],
  },
  {
    id: "j-dep-scan",
    name: "Weekly deps scan",
    sessionId: "s-deps",
    trigger: { type: "cron", expr: "0 6 * * 1" },
    steps: [{ step: "run_code", language: "bash", code: "# npm audit" }],
    policy: { suppressOnBattery: true },
    enabled: false,
    paused: false,
    recentFires: [],
  },
];

const DEMO_SUGGESTIONS: NudgeSuggestion[] = [
  {
    goal: "Morning brief",
    cron: "0 8 * * *",
    confidence: 0.9,
    observedAt: ["08:00"],
  },
];

export async function schedulerList(): Promise<SchedulerList> {
  return bridgeCall({
    operation: 'scheduler list',
    live: () => invoke<SchedulerList>('scheduler_list'),
    preview: () => ({ jobs: DEMO_JOBS, onBattery: false }),
  });
}

export async function schedulerCreate(args: {
  id: string;
  name: string;
  sessionId: string;
  trigger: SchedulerTrigger;
  steps: unknown[];
  policy?: SchedulerJob["policy"];
}): Promise<boolean> {
  return bridgeCall({
    operation: 'scheduler create',
    live: () => invoke<boolean>('scheduler_create', args),
    preview: () => true,
  });
}

export async function schedulerDelete(id: string): Promise<boolean> {
  return bridgeCall({
    operation: 'scheduler delete',
    live: () => invoke<boolean>('scheduler_delete', { id }),
    preview: () => true,
  });
}

export async function schedulerEnable(id: string, enabled: boolean): Promise<boolean> {
  return bridgeCall({
    operation: 'scheduler enable',
    live: () => invoke<boolean>('scheduler_enable', { id, enabled }),
    preview: () => true,
  });
}

export async function schedulerPause(id: string): Promise<boolean> {
  return bridgeCall({
    operation: 'scheduler pause',
    live: () => invoke<boolean>('scheduler_pause', { id }),
    preview: () => true,
  });
}

export async function schedulerResume(id: string): Promise<boolean> {
  return bridgeCall({
    operation: 'scheduler resume',
    live: () => invoke<boolean>('scheduler_resume', { id }),
    preview: () => true,
  });
}

/**
 * Request a manual run. Pass a caller-created key when the action may be
 * retried; omitting it uses the documented one-shot compatibility boundary,
 * which keeps the generated key until the request settles. That cache is
 * process-local and is not a substitute for a caller-retained retry key.
 */
export async function schedulerRunNow(
  id: string,
  idempotencyKey?: SchedulerRequestKey,
): Promise<boolean> {
  const operation = `manual:${id}`;
  const key = schedulerKeyFor(operation, idempotencyKey);
  try {
    const result = await bridgeCall({
      operation: 'scheduler run now',
      live: () => invoke<boolean>('scheduler_run_now', { id, idempotencyKey: key }),
      preview: () => true,
    });
    settleSchedulerKey(operation, key, true);
    return result;
  } catch (error) {
    settleSchedulerKey(operation, key, false);
    throw error;
  }
}

export async function schedulerBattery(onBattery: boolean): Promise<boolean> {
  return bridgeCall({
    operation: 'scheduler battery update',
    live: () => invoke<boolean>('scheduler_battery', { onBattery }),
    preview: () => true,
  });
}

/**
 * Fire an event trigger (CI fail / regression / repo change / ticket / metric).
 * The event producer owns the key and must reuse it for a delivery retry.
 */
export async function schedulerFireEvent(
  kind: string,
  payload: Record<string, unknown>,
  idempotencyKey?: SchedulerRequestKey,
): Promise<string[]> {
  const operation = `event:${kind}`;
  const key = schedulerKeyFor(operation, idempotencyKey);
  try {
    const result = await bridgeCall({
      operation: 'scheduler fire event',
      live: () => invoke<string[]>("scheduler_fire_event", { kind, payload, idempotencyKey: key }),
      preview: () => [],
    });
    settleSchedulerKey(operation, key, true);
    return result;
  } catch (error) {
    settleSchedulerKey(operation, key, false);
    throw error;
  }
}

/** Nudge sentinels: repeating-pattern schedule suggestions (H14 nudge cards). */
export async function schedulerNudges(): Promise<NudgeSuggestion[]> {
  return bridgeCall({
    operation: 'scheduler nudges',
    live: () => invoke<NudgeSuggestion[]>('scheduler_nudges'),
    preview: () => DEMO_SUGGESTIONS,
  });
}

/** Record a goal observation (feeds the nudge sentinels). */
export async function schedulerNudge(goal: string, ts?: number): Promise<boolean> {
  return bridgeCall({
    operation: 'scheduler nudge',
    live: () => invoke<boolean>('scheduler_nudge', { goal, ts }),
    preview: () => true,
  });
}

// ---- P51.32 — notepad + incidents + doctor (Hermes pattern) ----------------

/** One incident ledger row (unacked first, explicit ack only). */
export interface SchedulerIncident {
  id: string;
  jobId?: string;
  title: string;
  detail?: string;
  at?: string;
  acked?: boolean;
  [k: string]: unknown;
}

/** A job's durable notepad (the only continuity the trigger plane keeps). */
export interface SchedulerNotepad {
  notepad: string;
  [k: string]: unknown;
}

/** P51.32a — read a job's durable notepad. */
export async function schedulerNotepadGet(id: string): Promise<SchedulerNotepad> {
  return bridgeCall({
    operation: 'scheduler notepad read',
    live: () => invoke<SchedulerNotepad>('scheduler_notepad_get', { id }),
    preview: () => ({ notepad: '' }),
  });
}

/** P51.32a — append one line to a job's durable notepad. */
export async function schedulerNotepadAppend(id: string, line: string): Promise<boolean> {
  return bridgeCall({
    operation: 'scheduler notepad',
    live: () => invoke<boolean>('scheduler_notepad_append', { id, line }),
    preview: () => true,
  });
}

/** P51.32e — open (unacked-first) incidents ledger. */
export async function schedulerIncidents(): Promise<SchedulerIncident[]> {
  return bridgeCall({
    operation: 'scheduler incidents',
    live: () => invoke<SchedulerIncident[]>('scheduler_incidents'),
    preview: () => [],
  });
}

/** P51.32e — acknowledge one incident (explicit only, no auto-clear). */
export async function schedulerIncidentAck(id: string): Promise<boolean> {
  return bridgeCall({
    operation: 'scheduler incident ack',
    live: () => invoke<boolean>('scheduler_incident_ack', { id }),
    preview: () => true,
  });
}

// ---- P71.9e — the §11 run surface (runs · duplicate · export) --------------

/** One automation run's honest status, straight from the ExecutionLedger
 * (`scheduler_fire.rs` writes it; this is the read-back). `waitingApproval`
 * is the §11 "waiting for approval" state. */
export interface AutomationRun {
  id: string;
  sessionId: string;
  objective: string;
  phase: string;
  waitingApproval: boolean;
  createdAtMs: number;
  context?: string;
}

/** Recent runs for one automation (or all, when `jobId` is empty). The
 * scheduler keeps no run history (I3) — this reads the ledger that owns it. */
export async function schedulerRuns(jobId = ""): Promise<{ runs: AutomationRun[]; count: number }> {
  return bridgeCall({
    operation: 'automation runs',
    live: () => invoke<{ runs: AutomationRun[]; count: number }>('scheduler_runs', { jobId }),
    preview: () => ({ runs: [], count: 0 }),
  });
}

/** Duplicate an automation (same definition, new id, starts disabled). */
export async function schedulerDuplicate(id: string): Promise<string> {
  return bridgeCall({
    operation: 'automation duplicate',
    live: () => invoke<string>('scheduler_duplicate', { id }),
    preview: () => `${id}-copy`,
  });
}

/** The `*.automation.json` export body — the definition + session binding
 * only; no secrets ever ride this file (`AUTOMATION.md` §11). */
export interface AutomationExport {
  // DEC-053: wire value emitted by Rust (`scheduler_cmds.rs`, `scheduler_service.rs`) — the legacy spelling stays so both sides keep agreeing.
  kind: "everyaios.automation";
  version: number;
  automation: {
    name: string;
    sessionId: string;
    boundAgent: string | null;
    trigger: SchedulerTrigger;
    steps: unknown[];
    policy: SchedulerJob["policy"];
  };
}

/** Export an automation definition. */
export async function schedulerExport(id: string): Promise<AutomationExport> {
  return bridgeCall({
    operation: 'automation export',
    live: () => invoke<AutomationExport>('scheduler_export', { id }),
    // DEC-053: same wire value as above — renaming one side would break the export contract.
    preview: () => ({ kind: "everyaios.automation", version: 1, automation: { name: "", sessionId: "", boundAgent: null, trigger: { type: "cron", expr: "0 9 * * *" }, steps: [], policy: { suppressOnBattery: true } } }),
  });
}

/** P51.32f — read-only trigger-plane health (missed schedule fires, registry depth). */
export async function schedulerDoctor(): Promise<Record<string, unknown>> {
  return bridgeCall({
    operation: 'scheduler doctor',
    live: () => invoke<Record<string, unknown>>('scheduler_doctor'),
    preview: () => ({}),
  });
}

/** Human label for a trigger (the H14 list rows). */
export function triggerLabel(t: SchedulerTrigger): string {
  switch (t.type) {
    case "cron":
      return t.expr;
    case "interval":
      return `every ${t.secs}s`;
    case "event":
      return `on ${t.kind.replaceAll("_", " ")}${t.filter ? ` · ${t.filter}` : ""}`;
    case "webhook":
      return `POST ${t.path}`;
  }
}
