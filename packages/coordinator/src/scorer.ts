/**
 * P36 / P0-5 — the deterministic consensus scorer, extracted dependency-free
 * so the coordinator and the Rust crate (`agentcowork-core::routing::Scorer`)
 * can be locked against each other by pure unit tests.
 *
 * This is a faithful TS port of `Scorer::score` in
 * `crates/agentcowork-core/src/routing.rs`. Honest ceiling: without
 * observations the router still falls back to capability-filter + cost-sort
 * (there is nothing to score against yet); with observations it ranks by the
 * consensus score, not raw cost.
 */

/** One provider/model observation (mirrors `ProviderObservation` in routing.rs). */
export interface ProviderObservation {
  provider: string;
  model: string;
  ok: boolean;
  /** 0..=1 recent health (0 = dead). */
  health: number;
  /** 0..=1 quota remaining (defaults to 1 = unconstrained). */
  quotaRemaining?: number;
  /** Cost in $ per 1K tokens (0 = unknown/keyless — treated as cheapest). */
  cost: number;
  /** Round-trip latency in ms (0 = unknown — treated as best). */
  latencyMs: number;
  /** Prompt-cache read tokens — earns the cache-affinity bonus. */
  cacheReadTokens?: number;
}

/** Observation lookup key: `${provider}:${model}`. */
export function obsKey(provider: string, model: string): string {
  return `${provider}:${model}`;
}

/**
 * The consensus score (routing.rs `Scorer::score`): blends health (0.20),
 * quota (0.10), cost-inverse (0.15 · costWeight), latency-inverse
 * (latencyWeight) and a cache-affinity bonus (0.08), clamped to 0..=1.
 * A failed or dead observation scores exactly 0 (hard fail-closed).
 */
export function scorerScore(
  obs: ProviderObservation,
  costWeight = 0.15,
  latencyWeight = 0.12,
): number {
  if (!obs.ok || obs.health <= 0) return 0;
  const health = 0.2 * obs.health;
  const quota = Math.min(Math.max(obs.quotaRemaining ?? 1, 0), 1) * 0.1;
  const costInv = obs.cost <= 0 ? 0.15 : 0.15 * (1 / (1 + obs.cost));
  const latencyInv =
    obs.latencyMs === 0 ? latencyWeight : latencyWeight * (1 / (1 + obs.latencyMs / 1000));
  const cacheBonus = (obs.cacheReadTokens ?? 0) > 0 ? 0.08 : 0;
  return Math.min(Math.max(health + quota + costInv * costWeight + latencyInv + cacheBonus, 0), 1);
}

/** One scored route decision (winner + ranked alternatives). */
export interface RouteDecision {
  winner: { provider: string; model: string; score: number };
  ranked: Array<{ provider: string; model: string; score: number }>;
  /** Why the winner won (UI-displayable). */
  reason: string;
}

/**
 * Rank candidates by the consensus scorer. Only candidates that pass the
 * capability filter are scored; zero-observation candidates get score 0
 * (never chosen while a scored candidate exists).
 */
export function routeDecision(
  candidates: Array<{ provider: string; model: string }>,
  observations: Record<string, ProviderObservation>,
  opts: { costWeight?: number; latencyWeight?: number } = {},
): RouteDecision {
  const ranked = candidates
    .map((c) => ({
      provider: c.provider,
      model: c.model,
      score: scorerScore(
        observations[obsKey(c.provider, c.model)] ?? {
          provider: c.provider,
          model: c.model,
          ok: false,
          health: 0,
          cost: 0,
          latencyMs: 0,
        },
        opts.costWeight,
        opts.latencyWeight,
      ),
    }))
    .sort((a, b) => {
      if (b.score !== a.score) return b.score - a.score;
      // Deterministic tiebreak: stable by (provider, model) declaration order.
      return `${a.provider}:${a.model}`.localeCompare(`${b.provider}:${b.model}`);
    });
  const winner = ranked[0];
  const scored = ranked.filter((r) => r.score > 0);
  return {
    winner: winner
      ? { provider: winner.provider, model: winner.model, score: winner.score }
      : { provider: "nvidia", model: "meta/llama-3.1-8b-instruct", score: 0 },
    ranked: ranked.map((r) => ({ provider: r.provider, model: r.model, score: r.score })),
    reason:
      scored.length > 0
        ? `RouteDecision scorer — ${scored.length} scored candidate(s), winner score ${winner?.score.toFixed(3)}`
        : `RouteDecision scorer — no scored candidates (all observations failed or absent)`,
  };
}
