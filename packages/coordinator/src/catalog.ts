/**
 * P1.9 (A6) — desktop model catalog: per-provider model registry with
 * capability hints (tools / vision / context window) consumed by the router
 * (A7 asymmetric tiering) and the UI's context-warning gauge.
 *
 * Source of truth is APP's `@agentcowork/core-providers` capability-registry —
 * a pi.dev snapshot of **15 providers / 280 models** (`model-catalog.generated
 * .json`) — imported as a workspace dep, never copied (the reuse rule).
 *
 * Desktop additions layered on top:
 * - **Broker-id aliasing**: the Rust broker keys (`nvidia`, `chatgpt-pro`,
 *   `copilot`, `qwen`, `ollama`, `llamafile`) differ from catalog ids
 *   (`nvidia-nim` …). [`catalogIdForProvider`] maps them.
 * - **Local models**: `ollama list` output is merged in (via IPC) with the
 *   effective context window the broker forces (doc 33 §7.4 floor).
 * - **supportsTools heuristic**: pi.dev does not flag tool-calling; instruct/
 *   reasoning models ≥8K ctx are treated as tool-capable (small/embed/rerank
 *   models are not). B5's grammar constraint covers the weak tail anyway.
 */

import {
  getModelCapabilities,
  getModelsForProvider,
  getProviderById,
  modelSupportsReasoning,
  modelSupportsVision,
} from "@agentcowork/core-providers";
import type { ModelCapabilities } from "@agentcowork/core-providers";

/** Rust broker provider key → APP catalog id (mirror of P1.2's drift note). */
export const BROKER_TO_CATALOG_ID: Record<string, string> = {
  nvidia: "nvidia-nim",
  openai: "openai",
  anthropic: "anthropic",
  deepseek: "deepseek",
  groq: "groq",
};

/** Providers with NO catalog entry (OAuth subscriptions + local runtimes). */
export const KEYLESS_DESKTOP_PROVIDERS = [
  "chatgpt-pro",
  "copilot",
  "qwen",
  "ollama",
  "llamafile",
] as const;

/** Local model reported by Rust (`LocalManager::list_ollama_models`). */
export interface LocalModelInfo {
  name: string;
  sizeBytes: number;
  /** Effective context window (min of forced num_ctx and model max). */
  contextWindow: number;
}

/** Capability hints for one (provider, model) pair (A6). */
export interface ModelHints {
  provider: string;
  model: string;
  /** `number | undefined` (exactOptionalPropertyTypes-friendly). */
  contextWindow: number | undefined;
  supportsTools: boolean;
  supportsVision: boolean;
  supportsReasoning: boolean;
  /** Rough per-1M-token cost score (input + output) for ranking; 0 = free. */
  costScore: number;
}

/** Locally installed models (provider → list), set via IPC by the run loop. */
const localModels = new Map<string, LocalModelInfo[]>();

/** Merge installed local models (called by the run loop on `local/models`). */
export function setLocalModels(provider: string, models: LocalModelInfo[]): void {
  localModels.set(provider, models);
}

/** The catalog id backing a broker provider key, if any. */
export function catalogIdForProvider(provider: string): string | undefined {
  return BROKER_TO_CATALOG_ID[provider] ?? provider;
}

/** Raw pi.dev capabilities for a provider id (empty when unknown). */
export function catalogModels(provider: string): ModelCapabilities[] {
  const id = catalogIdForProvider(provider);
  if (!id) return [];
  return getModelsForProvider(id);
}

/** Context window for a (provider, model): local override, else pi.dev. */
export function contextWindowFor(provider: string, model: string): number | undefined {
  const local = localModels.get(provider)?.find((m) => m.name === model);
  if (local) return local.contextWindow;
  const caps = getModelCapabilities(catalogIdForProvider(provider) ?? "", model);
  // pi.dev contextWindow is `number | null` — normalize null → undefined.
  return caps?.contextWindow ?? undefined;
}

/** Capability hints for a (provider, model) pair (A6 — router + UI input). */
export function hintsFor(provider: string, model: string): ModelHints {
  const local = localModels.get(provider)?.find((m) => m.name === model);
  const caps = getModelCapabilities(catalogIdForProvider(provider) ?? "", model);
  // Normalize pi.dev's `number | null` contextWindow → `number | undefined`.
  const contextWindow = local?.contextWindow ?? caps?.contextWindow ?? undefined;
  const supportsVision = local ? false : modelSupportsVision(catalogIdForProvider(provider) ?? "", model);
  const supportsReasoning = local ? false : modelSupportsReasoning(catalogIdForProvider(provider) ?? "", model);
  return {
    provider,
    model,
    contextWindow,
    supportsTools: supportsToolsHeuristic(model, contextWindow),
    supportsVision,
    supportsReasoning,
    // pi.dev cost fields are `number | null` (free/open models) and the
    // object itself is optional.
    costScore: caps ? (caps.cost?.input ?? 0) + (caps.cost?.output ?? 0) : 0,
  };
}

/**
 * Tool-calling heuristic (pi.dev has no tool flag): modern instruct/reasoning
 * models with a sane context window are tool-capable; embed/rerank/tts/asr and
 * <8K-ctx tiny models are not.
 */
export function supportsToolsHeuristic(model: string, contextWindow?: number): boolean {
  const weak = /(embed|rerank|tts|asr|whisper|davinci|babbage|curie|image-|gpt-3\.5-turbo-instruct)/i;
  if (weak.test(model)) return false;
  if (contextWindow !== undefined && contextWindow < 8_192) return false;
  return true;
}

/** Installed local models for a provider (empty when none merged yet). */
export function localModelsFor(provider: string): LocalModelInfo[] {
  return localModels.get(provider) ?? [];
}

/** Every provider the desktop broker can serve (cloud catalog + desktop). */
export function brokerProviders(): string[] {
  const ids = Object.keys(BROKER_TO_CATALOG_ID).concat(
    ...KEYLESS_DESKTOP_PROVIDERS.map((p) => [p as string]),
  );
  // De-dupe, keep order.
  return [...new Set(ids)];
}

/** Provider display metadata for the UI (id, label, has local models). */
export function providerLabel(provider: string): string {
  const entry = getProviderById(catalogIdForProvider(provider) ?? "");
  if (entry) return entry.name ?? entry.id;
  const labels: Record<string, string> = {
    "chatgpt-pro": "ChatGPT Pro (OAuth)",
    copilot: "GitHub Copilot (OAuth)",
    qwen: "Qwen Code (OAuth)",
    ollama: "Ollama (local)",
    llamafile: "Llamafile (local)",
  };
  return labels[provider] ?? provider;
}
