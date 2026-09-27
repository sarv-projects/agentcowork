// P5.9 — token/cost dashboard bridge (H9). Mirrors the Rust
// `MemoryService::usage_snapshot()` shape (agentcowork-core). In a plain-browser
// preview (no shell) the caller falls back to demo data so the page is
// explorable.

import { invoke } from "./tauri";
import { bridgeCall } from './runtime';

export interface UsageRecord {
  tokensIn: number;
  tokensOut: number;
  cachedTokens: number;
  cachedWriteTokens: number;
  cacheHits: number;
  cacheMisses: number;
  cacheHitRate: number;
  /** P71.4 — cost the *producer* reported. `0` when it reported none; this is
   * an observation and is never added to the price-based estimate. */
  reportedCostUsd: number;
}

/** P71.4 — who reported a figure (`ARCH/ROUTING.md` §5). */
export type UsageSource =
  | 'agent_report'
  | 'acp_event'
  | 'provider_report'
  | 'capability_call'

export interface KeyUsage extends UsageRecord {
  key: string;
  /** **Our estimate** from configured prices — `null` when no price is set. */
  costUsd: number | null;
  /** P71.4 — the reporter for this key, or `null` if nothing was reported. */
  source?: UsageSource | null;
}

export interface SessionUsage extends UsageRecord {
  sessionId: string;
  source?: UsageSource | null;
}

/** P71.4 — tokens per observation source, plus the owners whose turns produced
 * **no** usage report (so a surface can say so instead of showing zero). */
export interface UsageObservations {
  by_source?: Partial<Record<UsageSource, UsageRecord>>;
  unreported?: Record<string, number>;
}

/** The user-facing label for a reporter. */
export function usageSourceLabel(source: UsageSource | null | undefined): string {
  switch (source) {
    case 'agent_report': return 'reported by the agent'
    case 'acp_event': return 'reported in an agent event'
    case 'provider_report': return 'reported by the provider'
    case 'capability_call': return 'measured by a capability call'
    default: return 'not reported'
  }
}

/** P71.4 — the owners (agents, else chats) that finished turns without
 * reporting usage. Non-empty means part of this ledger is *absent*, not zero. */
export function unreportedOwners(snapshot: UsageSnapshot): Array<[string, number]> {
  return Object.entries(snapshot.observations?.unreported ?? {}).sort((a, b) => b[1] - a[1])
}

/** P71.4 — cost as the ledger actually knows it: `reported` when a producer
 * priced its own turns, `estimated` from configured prices otherwise, and
 * `unknown` when neither exists. Never blended into one number (I15). */
export function costReadout(
  snapshot: UsageSnapshot,
): { kind: 'reported' | 'estimated' | 'unknown'; usd: number | null } {
  const reported = snapshot.byKey.reduce((sum, k) => sum + (k.reportedCostUsd ?? 0), 0)
  if (reported > 0) return { kind: 'reported', usd: reported }
  const estimated = snapshot.byKey.reduce((sum, k) => sum + (k.costUsd ?? 0), 0)
  if (estimated > 0) return { kind: 'estimated', usd: estimated }
  return { kind: 'unknown', usd: null }
}

export interface PrimarySpendView {
  primaryTokens: number
  workerTokens: number
  share: number
  warnNotDelegating: boolean
}

export interface UsageSnapshot {
  total: UsageRecord;
  cacheHitRate: number;
  byKey: KeyUsage[];
  bySession: SessionUsage[];
  /**
   * P71.4 — the primary-agent share of run tokens, computed from the **agent**
   * dimension when the turn path attributed a primary agent (renamed from
   * `chiefSpend` in this row). `null` means nothing was attributed: a surface
   * must render "not attributed" rather than a 0% share.
   */
  primarySpend?: PrimarySpendView | null;
  /** P71.4 — provenance for every figure above. */
  observations?: UsageObservations;
}

/** P60.9 — dashboard warning only; never aborts a turn. (P71.5b: primary/worker vocabulary.) */
export function primarySpendWarning(primaryTokens: number, workerTokens: number): PrimarySpendView {
  const total = primaryTokens + workerTokens
  const share = total === 0 ? 0 : primaryTokens / total
  return {
    primaryTokens,
    workerTokens,
    share,
    warnNotDelegating: total > 0 && share > 0.2,
  }
}

/** One per-session aggregate row (mirrors the vault's `SessionTotal`). */
export interface SessionTotal {
  session: string
  tokensIn: number
  tokensOut: number
  cost: number
}

/** The per-key/per-session/cache-hit dashboard data (polled by the page). */
export async function usageSnapshot(): Promise<UsageSnapshot> {
  return bridgeCall({
    operation: 'usage snapshot',
    live: () => invoke<UsageSnapshot>("usage_snapshot"),
    preview: () => demoSnapshot(),
  });
}

/** P5.9 — real per-session cost/token breakdown from the durable ledger. */
export async function sessionTotals(): Promise<SessionTotal[]> {
  return bridgeCall({
    operation: 'session totals',
    live: () => invoke<SessionTotal[]>("session_totals"),
    preview: () => demoSessionTotals(),
  });
}

function demoSessionTotals(): SessionTotal[] {
  return [
    { session: 'sess-q3-budget', tokensIn: 184_000, tokensOut: 22_400, cost: 1.84 },
    { session: 'sess-invoice-batch', tokensIn: 240_000, tokensOut: 31_800, cost: 2.41 },
    { session: 'sess-soc2-review', tokensIn: 142_000, tokensOut: 18_100, cost: 1.31 },
    { session: 'sess-competitor-crawl', tokensIn: 88_000, tokensOut: 12_300, cost: 0.92 },
    { session: 'sess-refactor-users', tokensIn: 51_000, tokensOut: 8_600, cost: 0.51 },
    { session: 'sess-dns-migration', tokensIn: 48_000, tokensOut: 6_900, cost: 0.38 },
  ];
}

function demoSnapshot(): UsageSnapshot {
  return {
    cacheHitRate: 0.66,
    total: {
      tokensIn: 18_120,
      tokensOut: 4_511,
      cachedTokens: 8_940,
      cachedWriteTokens: 620,
      cacheHits: 12,
      cacheMisses: 6,
      cacheHitRate: 0.66,
      reportedCostUsd: 0,
    },
    byKey: [
      {
        key: "anthropic",
        tokensIn: 12_481,
        tokensOut: 3_204,
        cachedTokens: 6_900,
        cachedWriteTokens: 420,
        cacheHits: 9,
        cacheMisses: 3,
        cacheHitRate: 0.75,
        costUsd: 0.18,
        reportedCostUsd: 0,
        source: 'agent_report',
      },
      {
        key: "openai",
        tokensIn: 4_112,
        tokensOut: 987,
        cachedTokens: 2_040,
        cachedWriteTokens: 200,
        cacheHits: 3,
        cacheMisses: 2,
        cacheHitRate: 0.6,
        costUsd: 0.05,
        reportedCostUsd: 0,
        source: 'provider_report',
      },
      {
        key: "deepseek",
        tokensIn: 1_527,
        tokensOut: 320,
        cachedTokens: 0,
        cachedWriteTokens: 0,
        cacheHits: 0,
        cacheMisses: 1,
        cacheHitRate: 0,
        costUsd: 0.01,
        reportedCostUsd: 0,
        source: null,
      },
    ],
    bySession: [
      {
        sessionId: "sess-q3-budget",
        tokensIn: 12_481,
        tokensOut: 3_204,
        cachedTokens: 6_900,
        cachedWriteTokens: 420,
        cacheHits: 9,
        cacheMisses: 3,
        cacheHitRate: 0.75,
        reportedCostUsd: 0,
        source: 'agent_report',
      },
      {
        sessionId: "sess-web-scrape",
        tokensIn: 5_639,
        tokensOut: 1_307,
        cachedTokens: 2_040,
        cachedWriteTokens: 200,
        cacheHits: 3,
        cacheMisses: 3,
        cacheHitRate: 0.5,
        reportedCostUsd: 0,
        source: 'agent_report',
      },
    ],
    primarySpend: null,
    observations: {
      by_source: {
        agent_report: {
          tokensIn: 18_120,
          tokensOut: 4_511,
          cachedTokens: 8_940,
          cachedWriteTokens: 620,
          cacheHits: 12,
          cacheMisses: 6,
          cacheHitRate: 0.66,
          reportedCostUsd: 0,
        },
      },
      unreported: {},
    },
  };
}
