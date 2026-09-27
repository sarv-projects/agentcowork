/**
 * P36 / P0-5 — live provider observation ledger.
 *
 * `runChatStream` recorded one observation per completed or errored turn
 * (`provider:model`); `selectModelForTask` fed `currentObservations()` into
 * the deterministic RouteDecision consensus scorer (the `Scorer::score` port
 * in `router.ts`) so the *next* turn ranked by observed health/cost/latency
 * instead of static cost-sort alone.
 *
 * **P71.2c status — the producer is gone.** `runChatStream` was archived with
 * the built-in engine (ADR-0005 §2), so nothing in v1 writes a live turn
 * observation: AgentCowork makes no provider call to observe. The durable half
 * (`hydrateObservations` from the vault's `token_usage` ledger) still loads and
 * the scorer still consumes it. `P71.4` owns the decision — re-home the
 * observation onto **agent reports** (`ARCH/ROUTING.md` §5: tokens and cost are
 * observations sourced from agent/ACP events, and where an agent reports
 * nothing the surface says so rather than inventing a number) or retire the
 * module with the router. Nothing may read a stale in-process observation as if
 * it described a v1 turn.
 *
 * Honest ceiling: health is outcome-derived (recent success ratio over a
 * 5-turn ring), quota is unknown (treated as unconstrained), and cost is the
 * catalog costScore estimate (per-1M → per-1K for the scorer). A provider
 * that just errored gets `ok:false` on that ring entry, so the consensus
 * score excludes it until it recovers.
 */

import type { ProviderObservation } from "./scorer";

const MAX_PER_KEY = 5;

interface LedgerEntry {
  provider: string;
  model: string;
  ok: boolean;
  latencyMs: number;
  cost: number;
}

const ledger = new Map<string, LedgerEntry[]>();

/** Keys that have live (this-process) entries — the durable ledger never
 * overwrites them (this process's observations are strictly newer). */
const liveKeys = new Set<string>();

/** Record one turn outcome for a (provider, model). */
export function recordObservation(
  provider: string,
  model: string,
  partial: { ok: boolean; latencyMs?: number; tokens?: number; costScore?: number },
): void {
  const key = `${provider}:${model}`;
  liveKeys.add(key);
  const ring = ledger.get(key) ?? [];
  // costScore is per-1M-token; the scorer's `cost` field is per-1K.
  const cost = partial.costScore !== undefined ? partial.costScore / 1000 : 0;
  ring.push({
    provider,
    model,
    ok: partial.ok,
    latencyMs: partial.latencyMs ?? 0,
    cost,
  });
  if (ring.length > MAX_PER_KEY) ring.shift();
  ledger.set(key, ring);
}

/** Snapshot of the ledger for the router: one consensus observation per key. */
export function currentObservations(): Record<string, ProviderObservation> {
  const out: Record<string, ProviderObservation> = {};
  for (const [, ring] of ledger) {
    const last = ring[ring.length - 1];
    if (!last) continue;
    const okCount = ring.filter((o) => o.ok).length;
    const avgLatency =
      ring.reduce((a, o) => a + o.latencyMs, 0) / ring.length;
    out[`${last.provider}:${last.model}`] = {
      provider: last.provider,
      model: last.model,
      ok: okCount > 0,
      health: okCount / ring.length,
      cost: last.cost,
      latencyMs: avgLatency,
    };
  }
  return out;
}

/**
 * A durable ledger row from the vault's `token_usage` table (Rust
 * `RecentUsage`, camelCase wire shape). Cost is total $ for the call;
 * latency is not recorded by the broker.
 */
export interface DurableUsageRow {
  tsMs: number;
  provider: string;
  model: string;
  inTokens: number;
  outTokens: number;
  cacheRead: number;
  cacheWrite: number;
  cost: number;
}

/**
 * Bootstrap the ring from the vault's durable ledger (ARCH/05 seam — the
 * coordinator hydrates once at boot via `usage/recent` so routing survives
 * restarts). Durable rows are successes with measured total-$ cost; latency
 * is unknown (0) until the live process records fresh turns. A key with
 * LIVE entries (recorded this process) is never overwritten; ledger keys
 * otherwise seed up to MAX_PER_KEY rows (newest first).
 */
export function hydrateObservations(rows: DurableUsageRow[]): number {
  let added = 0;
  for (const row of rows) {
    if (!row.provider || !row.model) continue;
    const key = `${row.provider}:${row.model}`;
    if (liveKeys.has(key)) continue; // live ring wins — strictly newer
    const totalTokens = row.inTokens + row.outTokens;
    // Convert total-$ to per-1K-token cost (the scorer's `cost` convention).
    const costPer1k = totalTokens > 0 ? (row.cost / totalTokens) * 1000 : 0;
    const ring = ledger.get(key) ?? [];
    if (ring.length >= MAX_PER_KEY) continue;
    ring.push({
      provider: row.provider,
      model: row.model,
      ok: true,
      latencyMs: 0,
      cost: costPer1k,
    });
    ledger.set(key, ring);
    added += 1;
  }
  return added;
}

/** Test hook: clear the ledger + live-keys set between runs. */
export function resetObservations(): void {
  ledger.clear();
  liveKeys.clear();
}
