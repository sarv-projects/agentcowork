/**
 * P71.5b — the Primary Agent (retired name: "Chief"). The dispatcher side of
 * the session's bound external agent (ADR-0005: external agents are the only
 * v1 engines; there is no `inbuilt` value to fall back to — a retired spelling
 * resolves to nothing, never to a substitute).
 *
 * - `resolvePrimaryAgentId` — the fail-closed resolution: explicit session
 *   value → user default → **none**. Unknown/empty ids refuse at the caller;
 *   the coordinator never invents an engine.
 * - `injectPrimaryContext` — the memory passport (C10) + taste profile (C9)
 *   injected into the primary agent's initial prompt; one path (the bound
 *   agent), so transport never changes governance.
 * - `SubagentLimits` — B3 delegation limits that apply to subagent chains
 *   under ANY bound agent: depth ≤2, concurrency ≤6, strict budgets, derived
 *   child permissions (parent ∩ deny ∩ explicit grants). Subagent launch under
 *   an external agent is an ACP tool call, never a bypass.
 * - `PrimaryAgentRegistry` — per-session records so Work survives agent
 *   death: the same intent → plan → checkpoints → receipts chain resumes
 *   under a new agent without re-explanation or replayed non-idempotent
 *   effects.
 */

/** A bound agent id (an ACP-registered/installed agent). Never `inbuilt`. */
export type PrimaryAgentId = string;

/**
 * P71.5b — the retired built-in spellings name *no agent* (ADR-0005 §1).
 * Kept as data so the resolution can fail closed on them by name.
 * DEC-053: the legacy spellings stay so already-retired agents keep being recognized, never resurrected.
 */
export const RETIRED_AGENT_IDS: readonly string[] = ["inbuilt", "everyaios", "everyaios-native"];

/** A retired spelling is not a binding; the resolution must refuse it. */
export function isRetiredAgentId(id: string | undefined | null): boolean {
  return id !== undefined && id !== null && RETIRED_AGENT_IDS.includes(id);
}

/** The governance badge per agent (spec §4.2.5a §3, corrected v3.46). */
export type GovernanceMode =
  | { kind: "mediated"; fs: boolean; terminal: boolean }
  | { kind: "self_contained"; channelB: boolean }
  | { kind: "not_governed" };

export function governanceBadge(mode: GovernanceMode): string {
  switch (mode.kind) {
    case "mediated":
      return "Governed-Mediated";
    case "self_contained":
      return "Self-contained";
    case "not_governed":
      return "NotGoverned";
  }
}

/**
 * Fail-closed resolution (spec §4.2.5a §1, ADR-0005 §1): explicit session
 * value → user default → **none** (`null` = no agent is bound; the turn path
 * refuses an unbound turn, it never substitutes an engine). Retired built-in
 * spellings are skipped like empty values, by name.
 */
export function resolvePrimaryAgentId(
  explicit?: string,
  userDefault?: string,
): PrimaryAgentId | null {
  for (const candidate of [explicit, userDefault]) {
    if (candidate !== undefined && candidate !== "" && !isRetiredAgentId(candidate)) {
      return candidate;
    }
  }
  return null;
}

/**
 * P38 (spec §4.2.5a §1) — per-session resolution, the single dispatch
 * decision the coordinator's chat path uses. Precedence: **session pin** →
 * **user default** → none. A session pins its own agent via `chief/set_session`
 * (fail-closed: retired/empty ids refuse, never a silent fallback).
 */
export function resolveSessionPrimaryAgent(opts: {
  sessionPin?: string;
  userDefault?: string;
}): PrimaryAgentId | null {
  return resolvePrimaryAgentId(opts.sessionPin, opts.userDefault);
}

/**
 * P38 — validate a session-level pin before recording it. Empty pins throw
 * fail-closed (they would silently read as "no pin"); a retired built-in
 * spelling throws by name — it is not an agent, it cannot be pinned.
 */
export function validateSessionPin(pin: string): PrimaryAgentId {
  if (pin === "") {
    throw new Error(`empty primary-agent pin — fail-closed (no silent fallback)`);
  }
  if (isRetiredAgentId(pin)) {
    throw new Error(`retired built-in agent ${pin === "inbuilt" ? '"inbuilt"' : `"${pin}"`} is not a v1 binding (ADR-0005) — pin a discovered external agent`);
  }
  return pin;
}

export interface PrimaryAgentContext {
  /** Memory passport (C10): the session's durable facts. */
  passport: string;
  /** Taste profile (C9): the user's style/preference summary. */
  taste: string;
  /** The active governance badge. */
  governance: GovernanceMode;
}

/**
 * Inject the memory passport + taste profile into the primary agent's initial
 * prompt (spec §4.2.5a §2). Returns the initial prompt text the dispatcher
 * sends as the first message of a session.
 */
export function injectPrimaryContext(ctx: PrimaryAgentContext, baseSystemPrompt: string): string {
  const parts: string[] = [baseSystemPrompt.trim()];
  if (ctx.passport.trim().length > 0) {
    parts.push(`## Memory passport (C10)\n${ctx.passport.trim()}`);
  }
  if (ctx.taste.trim().length > 0) {
    parts.push(`## Taste profile (C9)\n${ctx.taste.trim()}`);
  }
  parts.push(`## Governance\nThis session runs under ${governanceBadge(ctx.governance)}.`);
  return parts.join("\n\n");
}

/**
 * B3 delegation policy under any bound agent (spec §4.2.5a §5): depth ≤2,
 * concurrency ≤6, strict token/step budgets, derived child permissions
 * (parent ∩ deny ∩ explicit grants). Subagent launch under an external agent
 * is an ACP tool call, never a bypass.
 */
export interface SubagentLimits {
  maxDepth: number;
  maxConcurrency: number;
  maxStepsPerSubagent: number;
  /** Shared budget cap across the whole chain (e.g. spend units or steps). */
  chainBudget: number;
}

export const DEFAULT_SUBAGENT_LIMITS: SubagentLimits = {
  maxDepth: 2,
  maxConcurrency: 6,
  maxStepsPerSubagent: 200,
  chainBudget: 1000,
};

export interface SpawnState {
  /** Current chain depth (the primary agent is 0). */
  depth: number;
  /** Live subagent count at this level. */
  active: number;
  /** Chain steps consumed so far. */
  stepsUsed: number;
  /** Permissions the parent holds (derived child = parent ∩ deny ∩ grants). */
  parentPermissions: ReadonlySet<string>;
  /** Explicit denies that shrink the child's set. */
  denies: ReadonlySet<string>;
  /** Explicit grants that widen it. */
  grants: ReadonlySet<string>;
}

export type SpawnVerdict = { allowed: true } | { allowed: false; reason: string };

export function checkSpawn(state: SpawnState, limits: SubagentLimits = DEFAULT_SUBAGENT_LIMITS): SpawnVerdict {
  if (state.depth >= limits.maxDepth) {
    return { allowed: false, reason: `depth ${state.depth} ≥ max ${limits.maxDepth}` };
  }
  if (state.active >= limits.maxConcurrency) {
    return { allowed: false, reason: `concurrency ${state.active} ≥ max ${limits.maxConcurrency}` };
  }
  if (state.stepsUsed >= limits.chainBudget) {
    return { allowed: false, reason: `chain budget exhausted (${state.stepsUsed} ≥ ${limits.chainBudget})` };
  }
  return { allowed: true };
}

/** Derived child permissions: parent ∩ (deny removed) ∪ explicit grants. */
export function deriveChildPermissions(state: SpawnState): Set<string> {
  const derived = new Set(state.parentPermissions);
  for (const d of state.denies) derived.delete(d);
  for (const g of state.grants) derived.add(g);
  return derived;
}

/** One session's primary-agent record (Work-survives-agent-death + audit). */
export interface PrimaryAgentRecord {
  sessionId: string;
  agentId: PrimaryAgentId;
  governance: GovernanceMode;
  /** Last completed turn index in the event-sourced session log. */
  lastCompletedTurn: number;
  /** config_hash of the run (immutable manifest the new agent resumes under). */
  configHash: string;
}

/**
 * Work survives agent death (spec §4.2.5a §4): swap the bound agent mid-Work
 * and resume the same intent → plan → checkpoints → receipts chain from the
 * last completed turn. `configHash` stays the same; approvals are not lost;
 * non-idempotent effects are never replayed (they live in the receipt chain,
 * not the agent's context).
 */
export function buildResumePrompt(
  record: PrimaryAgentRecord,
  intent: string,
  completedPlan: string,
): string {
  return [
    `Resuming an existing Work session (${record.sessionId}, config ${record.configHash.slice(0, 12)}).`,
    `The previous primary agent completed ${record.lastCompletedTurn} turns. Continue the SAME Work — do not re-explain the task, do not replay completed effects.`,
    `## Intent\n${intent}`,
    `## Completed plan so far (checkpoints + receipts)\n${completedPlan}`,
    `## Next\nContinue from the next unfinished checkpoint.`,
  ].join("\n\n");
}

/** In-memory per-session records (the durable log is Rust's event log). */
export class PrimaryAgentRegistry {
  private records = new Map<string, PrimaryAgentRecord>();
  /** P38 — session-level pins: sessionId → pinned agent id. A pin outranks
   * the user default for every turn of that session; absent a pin, the user
   * default applies. Retired built-in spellings are refused by
   * `validateSessionPin` — they name no agent (ADR-0005). */
  private pins = new Map<string, PrimaryAgentId>();

  record(r: PrimaryAgentRecord): void {
    this.records.set(r.sessionId, r);
  }

  get(sessionId: string): PrimaryAgentRecord | undefined {
    return this.records.get(sessionId);
  }

  /** P38 — pin a session to an agent (per-session override). Returns the pin. */
  setSessionPin(sessionId: string, agentId: PrimaryAgentId): PrimaryAgentId {
    const validated = validateSessionPin(agentId);
    this.pins.set(sessionId, validated);
    // Keep the Work-survives-agent-death record in sync so the resume chain
    // and the pin never disagree.
    const prev = this.records.get(sessionId);
    if (prev) {
      this.records.set(sessionId, { ...prev, agentId: validated });
    }
    return validated;
  }

  sessionPin(sessionId: string): PrimaryAgentId | undefined {
    return this.pins.get(sessionId);
  }

  clearSessionPin(sessionId: string): void {
    this.pins.delete(sessionId);
  }

  /** Swap the agent for a session, keeping the Work chain intact. */
  swap(sessionId: string, agentId: PrimaryAgentId, governance: GovernanceMode): PrimaryAgentRecord | undefined {
    const prev = this.records.get(sessionId);
    if (!prev) return undefined;
    const next: PrimaryAgentRecord = {
      ...prev,
      agentId,
      governance,
    };
    this.records.set(sessionId, next);
    return next;
  }

  get size(): number {
    return this.records.size;
  }
}

export const primaryAgentRegistry = new PrimaryAgentRegistry();
