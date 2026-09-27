/**
 * Model adapter interface — normalizes each provider behind one interface.
 * Part of the model-agnostic prompt orchestration system (spec §6.5-6.6).
 */

import type { AssistantRequestPlan } from '@agentcowork/core-domain';

export type CapabilityProfile = {
  provider: string;
  modelRef: string;
  serviceClass: 'fast' | 'smart' | 'vision' | 'byok';
  text: boolean;
  vision: boolean;
  audio: boolean;
  video: boolean;
  tools: boolean;
  jsonSchema: boolean;
  maxContextTokens: number;
  maxOutputTokens: number;
  supportsPromptCache: boolean;
  qualityScores: Record<string, number>;
  latencyP50Ms: number;
  latencyP95Ms: number;
  health: 'healthy' | 'degraded' | 'disabled';
};

export type CostEstimate = {
  inputTokens: number;
  outputTokens: number;
  estimatedCredits: number;
  estimatedCostUsd: number;
};

export type NormalizedModelResult = {
  text: string;
  finishReason: 'stop' | 'length' | 'tool_call' | 'error';
  toolCalls: NormalizedToolCall[];
  usage: {
    inputTokens: number;
    cachedInputTokens?: number;
    outputTokens: number;
  };
  latencyMs: number;
  providerError?: string;
};

export type NormalizedToolCall = {
  id: string;
  name: string;
  arguments: Record<string, unknown>;
};

export interface ModelAdapter {
  id: string;

  /** Return this adapter's capability profile. */
  capabilities(): CapabilityProfile;

  /** Estimate cost for a request plan without executing it. */
  estimate(plan: AssistantRequestPlan): CostEstimate;

  /** Execute a compiled provider request, streaming tokens as they arrive. */
  generate(
    compiled: ProviderRequest,
    onToken: (token: string) => void,
  ): Promise<NormalizedModelResult>;
}

/**
 * Provider-specific request shape — adapters transform AssistantRequestPlan
 * into this before calling the actual API.
 */
export type ProviderRequest = {
  model: string;
  messages: Array<{ role: string; content: string | unknown[] }>;
  tools?: unknown[];
  temperature?: number;
  max_tokens?: number;
  stream?: boolean;
  [key: string]: unknown;
};