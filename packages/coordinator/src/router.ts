/**
 * P1.9 (A6/A7) — task→model router.
 *
 * The router consumes the catalog's capability hints (tools / vision /
 * context window / cost) and picks a (provider, model) per task — the input
 * to A7's asymmetric tiering (`planner_model` / `subagent_models`, depth=2,
 * concurrency=6, writers=3).
 *
 * Selection order:
 * 1. Explicit `model`/`provider` lock always wins.
 * 2. Filter candidates by required capabilities (vision, tools, min context).
 * 3. Rank: cheapest first (costScore), stable by provider declaration order.
 *    `preferPowerful` inverts toward the most capable (planner tier).
 * 4. Return a human-readable `reason` so the UI can show why a model won.
 *
 * **P71.2c status — no production consumer.** The caller was the built-in turn
 * loop (`chat.ts`, archived to `ARCH/archive/coordinator-loop/`), so this is
 * retained as a **pure, tested** function and as the migration note for the
 * post-v1 governed binding (`P71.7`), never as a v1 execution authority:
 * `ARCH/ROUTING.md` §1/§10 — an external agent owns its own model and provider
 * fallback, and AgentCowork no longer selects one. `P71.4` decides whether the
 * route-observation half is re-homed onto agent reports or retired with this.
 */

import {
  brokerProviders,
  catalogModels,
  contextWindowFor,
  hintsFor,
  localModelsFor,
} from "./catalog";
import {
  obsKey,
  routeDecision,
  scorerScore,
  type ProviderObservation,
  type RouteDecision,
} from "./scorer";

/** Task classes the router understands (A7 tiering inputs). */
export type TaskKind = "quick" | "chat" | "coding" | "tools" | "vision" | "deep";

/** One router decision. */
export interface ModelSelection {
  provider: string;
  model: string;
  /** `number | undefined` (exactOptionalPropertyTypes-friendly). */
  contextWindow: number | undefined;
  /** Why this model won (UI-displayable). */
  reason: string;
}

export interface RouterOptions {
  /** Explicit provider lock (e.g. user picked Ollama). */
  provider?: string;
  /** Explicit model lock (wins over everything). */
  model?: string;
  /** Task class; drives capability requirements. */
  task?: TaskKind;
  /** Minimum context window (tokens). */
  minContext?: number;
  /** Pick the most capable model instead of the cheapest (planner tier). */
  preferPowerful?: boolean;
  /** Restrict to providers in this list (e.g. only local). */
  providers?: string[];
  /**
   * P50.3.6 — vault-credential gate for the *taken* route (mirrors the Rust
   * `RoutingFeed::set_credentialed` display gate). When present, keyed
   * providers absent from this set are excluded before ranking; keyless
   * local runtimes and keyless tiers (`ollama`, `llamafile`, `lmstudio`,
   * `local`, `opencode-free`) always pass.
   * The caller (chat turn) feeds the live vault key set; an explicit
   * provider/model lock still wins (user intent over gating).
   */
  credentialedProviders?: string[];
  /**
   * P51.3 — `provider.use` allowlist for the *taken* route (mirrors the Rust
   * `UsePolicy`). When present (non-empty), providers absent from this set
   * are excluded before ranking — but an explicit provider/model lock still
   * wins (user intent over policy), and keyless local runtimes always pass
   * (a use-policy must never strand the user with zero local route).
   */
  usePolicies?: string[];
  /**
   * P36 — live provider observations (`${provider}:${model}` → observation).
   * When present, ranking uses the deterministic RouteDecision consensus
   * scorer instead of raw cost-sort (the Rust `Scorer` port above).
   */
  observations?: Record<string, ProviderObservation>;
}

/** Providers that never need a vault key (P50.3.6 keyless bypass). Mirrors the
 * Rust `requires_vault_key` gate (only ApiKey/ApiKeyEnv need keys): local
 * runtimes + keyless cloud tiers (e.g. opencode-free) always pass. */
const KEYLESS_PROVIDERS = new Set(["ollama", "llamafile", "lmstudio", "local", "opencode-free"]);

function normalizeProviderId(id: string): string {
  return id.trim().toLowerCase().replace(/[_ ]/g, "-");
}

/** Default capability requirements per task class. */
const TASK_REQUIREMENTS: Record<TaskKind, { vision?: boolean; tools?: boolean; minContext?: number }> = {
  quick: { minContext: 8_192 },
  chat: { minContext: 16_384 },
  coding: { tools: true, minContext: 32_768 },
  tools: { tools: true, minContext: 16_384 },
  vision: { vision: true, tools: true, minContext: 16_384 },
  deep: { tools: true, minContext: 64_000 },
};

/** A7 asymmetric tiering defaults (SPEC A7 row). */
export const ASYMMETRIC_TIERS = {
  depth: 2,
  concurrency: 6,
  writers: 3,
} as const;

/**
 * Deterministic task classification used when the caller did not explicitly
 * select a tier. It is deliberately conservative: ambiguous prompts stay in
 * the normal chat tier instead of silently receiving broad tool access.
 */
export function classifyTask(text: string): TaskKind {
  const normalized = text.toLowerCase();
  if (/\b(screenshot|image|photo|picture|visual|vision)\b/.test(normalized)) return "vision";
  if (/\b(debug|refactor|implement|code|compile|test|file|repo|repository|patch)\b/.test(normalized)) return "coding";
  if (/\b(research|compare|analy[sz]e|investigate|deep dive|sources|citations)\b/.test(normalized)) return "deep";
  if (/\b(search|find|lookup|browse|fetch|navigate|open)\b/.test(normalized)) return "tools";
  if (normalized.trim().length < 48) return "quick";
  return "chat";
}

/** Pick a (provider, model) for a task (A6 → A7 planner/subagent feed). */
export function selectModelForTask(opts: RouterOptions): ModelSelection {
  // 1. Explicit lock.
  if (opts.model) {
    const provider = opts.provider ?? "nvidia";
    return {
      provider,
      model: opts.model,
      contextWindow: contextWindowFor(provider, opts.model),
      reason: `explicit selection`,
    };
  }

  const req = TASK_REQUIREMENTS[opts.task ?? "chat"];
  const minContext = opts.minContext ?? req.minContext ?? 0;
  const needVision = req.vision ?? false;
  const needTools = req.tools ?? false;

  const providers = opts.providers ?? brokerProviders();
  const credentialed = opts.credentialedProviders
    ? new Set(opts.credentialedProviders.map(normalizeProviderId))
    : null;
  // P51.3 — `provider.use` allowlist (empty/absent = allow all).
  const useAllowed =
    opts.usePolicies && opts.usePolicies.length > 0
      ? new Set(opts.usePolicies.map(normalizeProviderId))
      : null;
  const candidates: Array<{ provider: string; model: string }> = [];
  // P50.3.6 — providers that survived the credential gate (mirrors the Rust
  // feed's gate-holding fallback: the taken route can never name a provider
  // the picker showed excluded, even on the no-candidate path below).
  const gatePassed: string[] = [];
  for (const provider of providers) {
    // P50.3.6 — exclude keyed providers with no vault credential before
    // ranking (keyless runtimes always pass). Mirrors the Rust feed gate so
    // the taken route can never pick a provider the picker showed excluded.
    if (
      credentialed !== null &&
      !KEYLESS_PROVIDERS.has(normalizeProviderId(provider)) &&
      !credentialed.has(normalizeProviderId(provider))
    ) {
      continue;
    }
    // P51.3 — use-policy exclusion (keyless local runtimes always pass).
    if (
      useAllowed !== null &&
      !KEYLESS_PROVIDERS.has(normalizeProviderId(provider)) &&
      !useAllowed.has(normalizeProviderId(provider))
    ) {
      continue;
    }
    gatePassed.push(provider);
    for (const m of catalogModels(provider)) {
      candidates.push({ provider, model: m.id });
    }
    // Local models (ollama/llamafile) merged via IPC are valid candidates.
    for (const m of localModelsFor(provider)) {
      candidates.push({ provider, model: m.name });
    }
  }

  let pass = candidates
    .map((c) => ({ c, hints: hintsFor(c.provider, c.model) }))
    .filter(({ hints }) => {
      if (needVision && !hints.supportsVision) return false;
      if (needTools && !hints.supportsTools) return false;
      if (minContext > 0 && (hints.contextWindow ?? 0) < minContext) return false;
      return true;
    });

  if (pass.length === 0) {
    // P50.3.6 / P51.3 — both provider gates hold in the fallback too: prefer
    // an explicit lock that passed the gates, else the first gate-passing
    // provider, else the "nvidia" default. This keeps a keyless local runtime
    // from being stranded by a use-policy when it has no catalog candidate.
    const providerGateActive = credentialed !== null || useAllowed !== null;
    const fallbackProvider = providerGateActive
      ? (opts.provider && gatePassed.includes(opts.provider) ? opts.provider : (gatePassed[0] ?? "nvidia"))
      : (opts.provider ?? "nvidia");
    const provider = fallbackProvider;
    const gateNote =
      credentialed !== null
        ? " — no vault credential for any candidate (add a provider key in Settings → Keys)"
        : "";
    return {
      provider,
      model: fallbackModel(provider, opts.task),
      contextWindow: undefined,
      reason: `no candidate met requirements (vision=${needVision}, tools=${needTools}, ctx≥${minContext})${gateNote} — fell back to ${provider} default`,
    };
  }

  // 2. Rank. With P36 observations, the deterministic RouteDecision consensus
  // scorer decides (same algorithm as `agentcowork-core::routing::Scorer`);
  // without observations we fall back to capability-filter + cost-sort.
  if (opts.observations) {
    const scored = pass.map((p) => ({
      p,
      score: scorerScore(
        opts.observations![obsKey(p.c.provider, p.c.model)] ?? {
          provider: p.c.provider,
          model: p.c.model,
          ok: false,
          health: 0,
          cost: 0,
          latencyMs: 0,
        },
      ),
    }));
    const healthy = scored.filter((s) => s.score > 0);
    if (healthy.length > 0) {
      if (opts.preferPowerful) {
        // Planner tier: the consensus score is a *health gate*, not a
        // capability axis — among healthy candidates the most capable wins
        // (highest cost proxy, then largest context), matching the no-
        // observation planner behavior.
        healthy.sort((a, b) => {
          const costA = a.p.hints.costScore;
          const costB = b.p.hints.costScore;
          if (costA !== costB) return costB - costA;
          return (b.p.hints.contextWindow ?? 0) - (a.p.hints.contextWindow ?? 0);
        });
        const winner = healthy[0]!.p;
        return {
          provider: winner.c.provider,
          model: winner.c.model,
          contextWindow: winner.hints.contextWindow,
          reason: `RouteDecision health gate — ${healthy.length}/${scored.length} observed candidate(s) healthy, planner picks most capable · ${describeReason(winner.hints, {
            needVision,
            needTools,
            minContext,
            preferPowerful: true,
          })}`,
        };
      }
      // Subagent/cheap tier: rank by the consensus score (cheapest healthy
      // wins — the scorer already bakes cost-inverse in).
      const decision = routeDecision(
        pass.map((p) => p.c),
        opts.observations,
      );
      const winner = pass.find(
        (p) => p.c.provider === decision.winner.provider && p.c.model === decision.winner.model,
      );
      if (winner) {
        return {
          provider: winner.c.provider,
          model: winner.c.model,
          contextWindow: winner.hints.contextWindow,
          reason: `${decision.reason} · ${describeReason(winner.hints, {
            needVision,
            needTools,
            minContext,
            preferPowerful: false,
          })}`,
        };
      }
    }
    // No observed candidate is healthy — fall through to the deterministic
    // cost-sort so the turn still gets a model. But live evidence wins over
    // the sort: a candidate with a *real failed* observation (ok:false /
    // health 0) is excluded while any unobserved candidate remains — never
    // route to a provider we just watched fail because it is cheap. If every
    // candidate is failed-observed, keep them all (best effort stands).
    {
      const failed = (p: { c: { provider: string; model: string } }): boolean => {
        const o = opts.observations![obsKey(p.c.provider, p.c.model)];
        return !!o && (!o.ok || o.health <= 0);
      };
      const alive = pass.filter((p) => !failed(p));
      if (alive.length > 0) pass = alive;
    }
  }

  pass.sort((a, b) => {
    const costA = a.hints.costScore;
    const costB = b.hints.costScore;
    if (opts.preferPowerful) {
      if (costA !== costB) return costB - costA;
      return (b.hints.contextWindow ?? 0) - (a.hints.contextWindow ?? 0);
    }
    if (costA !== costB) return costA - costB;
    return (a.hints.contextWindow ?? 0) - (b.hints.contextWindow ?? 0);
  });

  // pass.length > 0 was checked above.
  const winner = pass[0]!;
  const { hints } = winner;
  return {
    provider: winner.c.provider,
    model: winner.c.model,
    contextWindow: hints.contextWindow,
    reason: describeReason(hints, {
      needVision,
      needTools,
      minContext,
      preferPowerful: opts.preferPowerful,
    }),
  };
}

/** Planner-tier pick (A7): the most capable model meeting the task needs. */
export function plannerForTask(task: TaskKind, providers?: string[]): ModelSelection {
  return selectModelForTask({ task, preferPowerful: true, ...(providers ? { providers } : {}) });
}

/** Subagent-tier pick (A7): the cheapest model meeting the task needs. */
export function subagentForTask(task: TaskKind, providers?: string[]): ModelSelection {
  return selectModelForTask({ task, preferPowerful: false, ...(providers ? { providers } : {}) });
}

/** Last-resort default per provider (the broker's DEFAULT_BASE_URLS pairs). */
function fallbackModel(provider: string, task?: TaskKind): string {
  if (provider === "ollama") return "qwen3:4b";
  if (provider === "llamafile") return "qwen2.5-0.5b-instruct.llamafile";
  const models = catalogModels(provider);
  if (models.length > 0) return models[0]!.id;
  const defaults: Record<string, string> = {
    nvidia: "meta/llama-3.1-8b-instruct",
    openai: "gpt-4o",
    anthropic: "claude-sonnet-4-5",
    deepseek: "deepseek-chat",
    groq: "llama-3.3-70b-versatile",
    "chatgpt-pro": "gpt-4o",
    copilot: "gpt-4o",
    qwen: "qwen3-coder",
  };
  return defaults[provider] ?? (task === "quick" ? "meta/llama-3.1-8b-instruct" : "meta/llama-3.1-70b-instruct");
}

/** Human-readable selection reason for the UI. */
function describeReason(
  hints: ReturnType<typeof hintsFor>,
  req: { needVision: boolean; needTools: boolean; minContext: number; preferPowerful: boolean | undefined },
): string {
  const bits: string[] = [];
  if (req.preferPowerful) bits.push("planner tier");
  else bits.push("cheapest fit");
  if (req.needVision) bits.push("vision");
  if (req.needTools) bits.push("tools");
  if (req.minContext > 0) bits.push(`ctx≥${req.minContext}`);
  bits.push(`ctx=${hints.contextWindow ?? "?"}`);
  return bits.join(" · ");
}

// ---------------------------------------------------------------------------
// P36/RouteDecision — the deterministic consensus scorer.
//
// The scorer lives in `./scorer` (dependency-free) so the coordinator and the
// Rust crate (`agentcowork-core::routing::Scorer`) can be locked against each
// other by pure unit tests. Imported above and re-exported for callers of
// `router.ts`.
// ---------------------------------------------------------------------------

export {
  obsKey,
  routeDecision,
  scorerScore,
  type ProviderObservation,
  type RouteDecision,
} from "./scorer";
