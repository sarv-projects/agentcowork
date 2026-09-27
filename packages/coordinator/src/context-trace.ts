/**
 * The **context inspector** trace surface: an honest, read-only projection of
 * what the context controller decided for a turn.
 *
 * The rule this module exists to keep: the trace is a **projection**, not an
 * owner. It holds no context state, makes no selection decision, and cannot
 * influence what the model sees — it only reports what already happened, in
 * the same shape the Rust controller emits (`ContextController::trace`):
 *
 * - the **named budget terms** (window, every reserve, the safety buffer, the
 *   retained-recent term, and the resulting usable figure), so the UI can show
 *   where the window actually went rather than a single opaque number;
 * - the **pre-turn feasibility verdict** — including the refusal case, which
 *   carries guidance instead of a provider error;
 * - what was **selected, pruned and compacted**, with the full bytes of every
 *   pruned item still reachable by reference (no silent loss);
 * - the **measured** injected tokens, split into the always-on and the
 *   relevant block, so "zero relevant hits ⇒ zero relevant-block tokens" is
 *   visible rather than asserted;
 * - the **cache telemetry** (baseline vs delta, frozen-block hits vs busts).
 *
 * The existing `ContextTrace` in this file's history is the narrower
 * "model-visible means logged" check: it proves every recorded block appears in
 * the prompt that was sent. That check is kept here, because the inspector and
 * the reconstructability proof answer different questions and both belong to
 * the same audit surface.
 */

import { createHash } from "node:crypto";

/** A context source a block can come from. */
export type ContextSource =
  | "system"
  | "user"
  | "memory_warm_set"
  | "memory_always_on"
  | "memory_relevant"
  | "tool_index"
  | "repo_map"
  | "user_document"
  | "style_memory"
  | "trajectory"
  // The agent's own shell state, read from the one PTY plane.
  | "terminal_plane"
  // The durable session-log projection rendered at a compaction boundary.
  | "conversation_checkpoint";

/** The named budget terms. Kept in step with the Rust `BudgetBreakdown`. */
export interface BudgetTerms {
  modelWindowResolved: number;
  outputReserve: number;
  reasoningReserve: number;
  summaryOutputReserve: number;
  toolSchemaReserve: number;
  systemReserve: number;
  safetyBuffer: number;
  /** The retained-recent term. */
  keep: number;
  reservedTotal: number;
  usable: number;
}

/**
 * The pre-send feasibility verdict. There is deliberately no "sent" verdict:
 * overflow is decided before the send, so a trace that claims a provider
 * overflow cannot be constructed.
 */
export type Feasibility =
  | { kind: "fits" }
  | { kind: "needs_recovery"; overBy: number }
  | { kind: "refused"; reason: RefusalReason; shortfall: number; guidance: string[] };

export type RefusalReason =
  | "terms_invalid"
  | "minimum_irreducible"
  | "keep_too_large";

/** The three recall outcomes, kept distinguishable so a miss is not a fault. */
export type RecallOutcome = "hit" | "abstain" | "error";

/** One selected item, with the provenance the inspector must show. */
export interface SelectedItem {
  id: string;
  source: ContextSource | string;
  trustTier: "user_explicit" | "agent_asserted" | "derived_untrusted" | "import";
  sensitivity: "public" | "personal" | "confidential";
  tokens: number;
  /** A reference, not a copy: the content is resolved on demand. */
  contentRef: string;
  /** May this item be dropped and rebuilt deterministically? */
  reconstructable: boolean;
  pinned: boolean;
  /** The provenance ref, when one exists. */
  sourceRef?: string | null;
  /** True when the referenced source was pruned; the item still renders. */
  sourceUnavailable?: boolean;
}

/** One pruned item: a bounded preview plus a ref to the full, durable bytes. */
export interface PrunedItem {
  /** The log sequence number of the pruned entry. */
  seq: number;
  /** The bounded preview that replaced the full output. */
  preview: string;
  /** Where the full, untruncated output lives. */
  durableRef: string;
  reconstructable: boolean;
}

/** Cache telemetry for the turn: baseline + deltas, frozen-block hits/busts. */
export interface CacheTelemetry {
  turns: number;
  prefixHits: number;
  prefixMisses: number;
  frozenBlockHits: number;
  frozenBlockMisses: number;
  baselineEmitted: number;
  deltasEmitted: number;
}

/** The full trace for one turn. */
export interface ContextDecisionTrace {
  turn: number;
  budget: BudgetTerms;
  /** What the turn wanted to send before packing. */
  footprint: { systemTokens: number; messageTokens: number; toolSchemaTokens: number };
  feasibility: Feasibility;
  recallOutcome: RecallOutcome;
  selected: SelectedItem[];
  /** Ids the user pinned; a pin is a floor, not a budget bypass. */
  pinned: string[];
  /** Ids the user excluded; an exclusion is absolute. */
  excluded: string[];
  pruned: PrunedItem[];
  /** The log sequence the last compaction covered, if any. */
  compactedThroughSeq: number | null;
  checkpointId: string | null;
  /**
   * Tokens measured on the *rendered* blocks. `relevantTokens` is zero when
   * there were no query-relevant hits; the always-on block is reported
   * separately and exists only when pinned items exist.
   */
  relevantTokens: number;
  alwaysOnTokens: number;
  cache: CacheTelemetry;
  /** Gates that deferred work rather than running it, reported honestly. */
  deferred: string[];
}

/** A compact per-source/per-block census for the inspector header. */
export interface InspectorSummary {
  turn: number;
  window: number;
  usable: number;
  used: number;
  perSource: Record<string, number>;
  perClass: Record<string, number>;
  pinnedCount: number;
  excludedCount: number;
  relevantTokens: number;
  alwaysOnTokens: number;
  droppedForBudget: number;
  prunedCount: number;
  checkpointId: string | null;
  feasibility: Feasibility["kind"];
}

const EMPTY_TELEMETRY: CacheTelemetry = {
  turns: 0,
  prefixHits: 0,
  prefixMisses: 0,
  frozenBlockHits: 0,
  frozenBlockMisses: 0,
  baselineEmitted: 0,
  deltasEmitted: 0,
};

const EMPTY_BUDGET: BudgetTerms = {
  modelWindowResolved: 0,
  outputReserve: 0,
  reasoningReserve: 0,
  summaryOutputReserve: 0,
  toolSchemaReserve: 0,
  systemReserve: 0,
  safetyBuffer: 0,
  keep: 0,
  reservedTotal: 0,
  usable: 0,
};

/**
 * Derive the usable window from the named terms, with the same saturating
 * arithmetic the kernel uses. Exported so a caller can check the arithmetic
 * rather than trust it.
 */
export function usableWindow(budget: BudgetTerms): number {
  const reserved =
    budget.outputReserve +
    budget.reasoningReserve +
    budget.summaryOutputReserve +
    budget.toolSchemaReserve +
    budget.systemReserve +
    budget.safetyBuffer;
  return Math.max(0, budget.modelWindowResolved - reserved);
}

/** Cheap token estimate (the same conservative 4-chars-per-token convention). */
export function estimateTokens(s: string): number {
  return Math.ceil(s.length / 4);
}

export function sha256Hex(s: string): string {
  return createHash("sha256").update(s).digest("hex");
}

/**
 * A per-turn trace recorder. It holds only what it is told; it never reads the
 * controller's state and cannot influence it.
 */
export class ContextInspector {
  private traces: ContextDecisionTrace[] = [];

  /** Record one turn's decision. Returns the stored trace for convenience. */
  record(trace: ContextDecisionTrace): ContextDecisionTrace {
    this.telemetry.turns += 1;
    this.traces.push(trace);
    return trace;
  }

  /** The telemetry of the recorded turns, derived rather than stored twice. */
  readonly telemetry: CacheTelemetry = { ...EMPTY_TELEMETRY };

  /** The most recent turn's trace. */
  last(): ContextDecisionTrace | undefined {
    return this.traces[this.traces.length - 1];
  }

  all(): ContextDecisionTrace[] {
    return [...this.traces];
  }

  count(): number {
    return this.traces.length;
  }

  /** The header view for the inspector panel. */
  summary(trace: ContextDecisionTrace = this.last()!): InspectorSummary {
    const used = trace.footprint.systemTokens + trace.footprint.messageTokens + trace.footprint.toolSchemaTokens;
    const perSource: Record<string, number> = {};
    const perClass: Record<string, number> = {};
    for (const item of trace.selected) {
      perSource[item.source] = (perSource[item.source] ?? 0) + 1;
      perClass[item.trustTier] = (perClass[item.trustTier] ?? 0) + 1;
    }
    return {
      turn: trace.turn,
      window: trace.budget.modelWindowResolved,
      usable: trace.budget.usable,
      used,
      perSource,
      perClass,
      pinnedCount: trace.pinned.length,
      excludedCount: trace.excluded.length,
      relevantTokens: trace.relevantTokens,
      alwaysOnTokens: trace.alwaysOnTokens,
      // A drop is whole-item, never a truncation, so this count is the number
      // of items that did not fit — not a measure of mangled text.
      droppedForBudget: trace.selected.filter((i) => i.tokens === 0).length,
      prunedCount: trace.pruned.length,
      checkpointId: trace.checkpointId,
      feasibility: trace.feasibility.kind,
    };
  }

  /** Forget every recorded turn. The inspector is per-session, so this is a
   * session boundary, not a state mutation. */
  reset(): void {
    this.traces = [];
  }
}

/** Build a trace with sane zero defaults, so a caller supplies only what it knows. */
export function newTrace(turn: number, overrides: Partial<ContextDecisionTrace> = {}): ContextDecisionTrace {
  return {
    turn,
    budget: { ...EMPTY_BUDGET },
    footprint: { systemTokens: 0, messageTokens: 0, toolSchemaTokens: 0 },
    feasibility: { kind: "fits" },
    recallOutcome: "abstain",
    selected: [],
    pinned: [],
    excluded: [],
    pruned: [],
    compactedThroughSeq: null,
    checkpointId: null,
    relevantTokens: 0,
    alwaysOnTokens: 0,
    cache: { ...EMPTY_TELEMETRY },
    deferred: [],
    ...overrides,
  };
}

/**
 * The reconstructability proof, kept from the original trace surface: every
 * recorded block must appear in the prompt that was actually sent. The inspector
 * answers "what did you decide"; this answers "is the decision honest".
 */
export interface ContextLogEntry {
  source: ContextSource;
  hash: string;
  tokens: number;
}

export interface ContextLogResult {
  ok: boolean;
  entries: ContextLogEntry[];
  missing: ContextSource[];
}

export class ContextTrace {
  private entries: ContextLogEntry[] = [];

  /** Record a block that is about to be injected into the prompt. */
  record(source: ContextSource, content: string): void {
    this.entries.push({
      source,
      hash: sha256Hex(content),
      tokens: estimateTokens(content),
    });
  }

  entriesFor(source: ContextSource): ContextLogEntry[] {
    return this.entries.filter((e) => e.source === source);
  }

  all(): ContextLogEntry[] {
    return [...this.entries];
  }

  count(): number {
    return this.entries.length;
  }
}

export function verifyEntry(
  trace: ContextTrace,
  source: ContextSource,
  originalContent: string,
  promptSent: string,
): boolean {
  const entries = trace.entriesFor(source);
  const expected = sha256Hex(originalContent);
  if (!entries.some((e) => e.hash === expected)) return false;
  return promptSent.includes(originalContent);
}

/** Full invariant: every recorded block is present in the sent prompt. */
export function assertAllLogged(
  trace: ContextTrace,
  blocks: Array<{ source: ContextSource; content: string }>,
  promptSent: string,
): ContextLogResult {
  const missing: ContextSource[] = [];
  for (const b of blocks) {
    if (!verifyEntry(trace, b.source, b.content, promptSent)) {
      missing.push(b.source);
    }
  }
  return { ok: missing.length === 0, entries: trace.all(), missing };
}
