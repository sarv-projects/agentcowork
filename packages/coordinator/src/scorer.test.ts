/**
 * P36 / P0-5 — pure unit tests for the consensus-scorer port. Imports only
 * `./scorer` (dependency-free) so these run without the APP workspace deps.
 * Locks the TS port against the Rust `agentcowork-core::routing::Scorer::score`
 * semantics: failed/dead → 0, health 0.20 · quota 0.10 · cost-inverse ·
 * latency-inverse · cache bonus, clamped 0..=1.
 */
import { describe, expect, it } from "bun:test";
import {
  obsKey,
  routeDecision,
  scorerScore,
  type ProviderObservation,
} from "./scorer";

const obs = (p: string, m: string, o: Partial<ProviderObservation>): ProviderObservation => ({
  provider: p,
  model: m,
  ok: true,
  health: 1,
  cost: 0.5,
  latencyMs: 200,
  ...o,
});

describe("scorerScore (routing.rs Scorer::score port)", () => {
  it("scores exactly 0 for a failed or dead observation (hard fail-closed)", () => {
    expect(scorerScore(obs("a", "m", { ok: false, health: 0.9 }))).toBe(0);
    expect(scorerScore(obs("a", "m", { ok: true, health: 0 }))).toBe(0);
  });

  it("healthy+cheap+fast scores higher than healthy+expensive+slow", () => {
    const cheap = scorerScore(obs("a", "m1", { cost: 0.5, latencyMs: 200 }));
    const pricey = scorerScore(obs("b", "m2", { cost: 50, latencyMs: 4000 }));
    expect(cheap).toBeGreaterThan(pricey);
  });

  it("health dominates: dead-cheap never beats alive-pricey", () => {
    const dead = scorerScore(obs("a", "m1", { ok: false, cost: 0.01, latencyMs: 1 }));
    const alive = scorerScore(obs("b", "m2", { cost: 99, latencyMs: 9999 }));
    expect(alive).toBeGreaterThan(dead);
  });

  it("a cache-affinity bonus raises the score and stays within 0..=1", () => {
    const plain = scorerScore(obs("a", "m", { health: 1, cost: 0, latencyMs: 0 }));
    const cached = scorerScore(
      obs("a", "m", { health: 1, cost: 0, latencyMs: 0, cacheReadTokens: 1_000_000 }),
    );
    expect(cached).toBeGreaterThan(plain);
    expect(cached).toBeLessThanOrEqual(1);
  });

  it("matches the Rust weights exactly (0.20/0.10/0.15/0.12 + 0.08 bonus)", () => {
    // health 1 → 0.20; quota default 1 → 0.10; cost 0 → 0.15 · costWeight 0.15;
    // latency 0 → latencyWeight 0.12; cache bonus 0.08 = 0.20+0.10+0.0225+0.12+0.08.
    const s = scorerScore(obs("a", "m", { health: 1, cost: 0, latencyMs: 0, cacheReadTokens: 1 }));
    expect(s).toBeCloseTo(0.5225, 5);
  });
});

describe("routeDecision", () => {
  it("ranks by score desc and picks the healthy winner", () => {
    const observations = {
      [obsKey("a", "m1")]: obs("a", "m1", { cost: 0.5, latencyMs: 100 }),
      [obsKey("b", "m2")]: obs("b", "m2", { cost: 10, latencyMs: 3000 }),
      [obsKey("c", "m3")]: obs("c", "m3", { ok: false, health: 0, cost: 0.1, latencyMs: 50 }),
    };
    const d = routeDecision(
      [
        { provider: "a", model: "m1" },
        { provider: "b", model: "m2" },
        { provider: "c", model: "m3" },
      ],
      observations,
    );
    expect(d.winner.provider).toBe("a");
    expect(d.winner.model).toBe("m1");
    expect(d.ranked.find((r) => r.provider === "c")!.score).toBe(0);
    expect(d.reason).toContain("RouteDecision scorer");
  });

  it("never picks an unobserved candidate while a scored one exists", () => {
    const d = routeDecision(
      [
        { provider: "a", model: "m1" },
        { provider: "b", model: "m2" },
      ],
      { [obsKey("a", "m1")]: obs("a", "m1", { cost: 0.5, latencyMs: 100 }) },
    );
    expect(d.winner.provider).toBe("a");
  });

  it("declares score-0 winner when every candidate is failed/unobserved (no crash)", () => {
    const d = routeDecision(
      [{ provider: "a", model: "m1" }],
      { [obsKey("a", "m1")]: obs("a", "m1", { ok: false, health: 0 }) },
    );
    expect(d.winner.score).toBe(0);
  });
});
