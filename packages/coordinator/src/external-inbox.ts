/**
 * P30.3 — EXTERNAL-risk → unattended inbox hook (openworker pattern, doc 83
 * §1, reimplemented). Background/unattended runs (scheduler jobs, automation,
 * headless) must never act on off-machine side effects without a human. When
 * the guard's autonomy layer returns `park_in_inbox` (EXTERNAL / EXEC /
 * destructive-WRITE_LOCAL under an unattended policy), the ask parks here
 * instead of acting — the messaging + automation proactivity layer's inbox.
 *
 * Mirrors `agentcowork-guard::autonomy::{AutonomyVerdict, AutonomyPolicy}`
 * semantics on the TS side (the Rust verdict is authoritative when present;
 * this class is the coordinator-side queue + delivery surface).
 */

export type InboxRiskClass = "READ" | "WRITE_LOCAL" | "EXEC" | "EXTERNAL";

/** One parked ask from an unattended run (P30.3). */
export interface ExternalAsk {
  id: string;
  /** What the run wanted to do (one sentence, plain language). */
  summary: string;
  /** Effect class (EXTERNAL = off-machine side effect). */
  riskClass: InboxRiskClass;
  /** The exact operation name (ticket `operation` vocabulary). */
  operation: string;
  /** SHA-256 of the serialized args (single-use ticket binding). */
  argsHash: string;
  /** The unattended run that parked it (job / session / agent). */
  sourceRun: string;
  /** UNIX ms when parked. */
  parkedAtMs: number;
  /** Resolved state. */
  state: "open" | "approved" | "rejected" | "expired";
  /** Set when approved: the minted ticket id the run may consume. */
  ticketId?: string;
}

export interface ParkAskInput {
  summary: string;
  riskClass: InboxRiskClass;
  operation: string;
  argsHash: string;
  sourceRun: string;
}

/** Deterministic id (time + counter). */
let inboxSeq = 0;
export function nextAskId(now = Date.now()): string {
  inboxSeq += 1;
  return `ask-${now.toString(36)}-${inboxSeq.toString(36)}`;
}

/**
 * The unattended inbox. A parked ask is *never* auto-approved; a human
 * resolves it, which mints a single-use ticket the run can then consume.
 */
export class ExternalInbox {
  private asks = new Map<string, ExternalAsk>();

  constructor(private now: () => number = Date.now) {}

  /** Park an ask (P30.3 — the hook the unattended loop calls). */
  park(input: ParkAskInput): ExternalAsk {
    const ask: ExternalAsk = {
      id: nextAskId(this.now()),
      summary: input.summary,
      riskClass: input.riskClass,
      operation: input.operation,
      argsHash: input.argsHash,
      sourceRun: input.sourceRun,
      parkedAtMs: this.now(),
      state: "open",
    };
    this.asks.set(ask.id, ask);
    return ask;
  }

  /** All open asks (for the inbox UI). */
  list(): ExternalAsk[] {
    return [...this.asks.values()]
      .filter((a) => a.state === "open")
      .sort((a, b) => a.parkedAtMs - b.parkedAtMs);
  }

  pendingCount(): number {
    return this.list().length;
  }

  /** A human approves → mint a ticket id the run may consume (single-use). */
  approve(id: string, ticketId: string): boolean {
    const ask = this.asks.get(id);
    if (!ask || ask.state !== "open") return false;
    ask.state = "approved";
    ask.ticketId = ticketId;
    return true;
  }

  /** A human rejects → the run must not act. */
  reject(id: string): boolean {
    const ask = this.asks.get(id);
    if (!ask || ask.state !== "open") return false;
    ask.state = "rejected";
    return true;
  }

  /** Reaper: expire asks older than `ttlMs` (default 7 days, P43.4 class). */
  expireOlderThan(ttlMs = 7 * 24 * 60 * 60 * 1000): number {
    const cutoff = this.now() - ttlMs;
    let n = 0;
    for (const ask of this.asks.values()) {
      if (ask.state === "open" && ask.parkedAtMs < cutoff) {
        ask.state = "expired";
        n += 1;
      }
    }
    return n;
  }
}

/**
 * The unattended-loop decision: should this action run, ask, or park?
 * Mirrors `AutonomyPolicy::unattended_verdict` — off-machine and local-exec
 * effects park; reversible local writes and reads proceed.
 */
export function unattendedVerdict(riskClass: InboxRiskClass, destructive: boolean): "act" | "park_in_inbox" {
  if (riskClass === "READ") return "act";
  if (riskClass === "WRITE_LOCAL" && !destructive) return "act";
  return "park_in_inbox";
}
