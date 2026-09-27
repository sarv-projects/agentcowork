/**
 * Tiered context compaction — builds the prompt context in layers,
 * from most reusable (cache-friendly) to most volatile.
 *
 * "Your prompt context should not grow linearly with chat history."
 *
 * Tiers:
 * 1. Recent turns — verbatim, small bounded window (last N turns)
 * 2. Older conversational context — canonical session summary
 * 3. Approved memory — retrieved selectively under 800-token hard cap
 * 4. Files / Reader / projects — retrieved only under explicit scope
 * 5. Tool results — structured summaries, not raw payloads
 *
 * Key invariant: blocks 1-6 of the 12-segment prompt are fixed across turns.
 * Only tiers 3-5 change per turn. Never regenerate summaries or re-rank
 * the entire prompt every turn — it damages provider-cache locality.
 */

import type { ChatMessage } from '@agentcowork/core-domain';
import { CHAT_CONTEXT_TOKENS, CHAT_MAX_OUTPUT_TOKENS, CHARS_PER_TOKEN_ESTIMATE } from '../router/prompt-limits.js';

const MAX_RECENT_TURNS = 6;
const MEMORY_TOKEN_BUDGET = 800;
const CHARS_PER_TOKEN = CHARS_PER_TOKEN_ESTIMATE;

// ─── Types ───────────────────────────────────────────────────────────

export type CompactContext = {
  recentTurns: ChatMessage[];
  olderSummary: string | null;
  memoryBlock: string | null;
  retrievalBlock: string | null;
  toolResultBlock: string | null;
  totalEstimateTokens: number;
};

export type ContextBudget = {
  totalTokens: number;
  recentTurnsTokens: number;
  summaryTokens: number;
  memoryTokens: number;
  retrievalTokens: number;
  toolResultTokens: number;
};

// ─── Budget calculator ───────────────────────────────────────────────

/**
 * Calculate token budgets for each context tier.
 * Recent turns get the largest share; older turns get compressed.
 */
export function calculateBudget(
  maxContextTokens: number = CHAT_CONTEXT_TOKENS,
  maxOutputTokens: number = CHAT_MAX_OUTPUT_TOKENS,
): ContextBudget {
  const inputBudget = maxContextTokens - maxOutputTokens;

  // Tiers must sum to <= 100% of inputBudget — a fixed 800-token memory tier
  // on top of 100%-budget tiers overshoots the context window (C.5).
  return {
    totalTokens: inputBudget,
    recentTurnsTokens: Math.floor(inputBudget * 0.35),
    summaryTokens: Math.floor(inputBudget * 0.15),
    memoryTokens: Math.floor(inputBudget * 0.10),
    retrievalTokens: Math.floor(inputBudget * 0.25),
    toolResultTokens: Math.floor(inputBudget * 0.15),
  };
}

// ─── Recent turns tier ───────────────────────────────────────────────

/**
 * Select the most recent N turns for verbatim inclusion.
 * These are cache-friendly — they don't change between turns.
 */
export function selectRecentTurns(
  messages: ChatMessage[],
  maxTurns: number = MAX_RECENT_TURNS,
): ChatMessage[] {
  if (messages.length <= maxTurns) return messages;

  const system = messages[0]?.role === 'system' ? messages[0] : null;
  const nonSystem = system ? messages.slice(1) : messages;
  const recent = nonSystem.slice(-maxTurns);

  return system ? [system, ...recent] : recent;
}

// ─── Older summary tier ──────────────────────────────────────────────

/**
 * Build a canonical summary of turns beyond the recent window.
 * This summary is cached per conversation — do NOT regenerate every turn.
 *
 * "Do not regenerate summaries or re-rank the entire prompt every turn.
 * It damages provider-cache locality and can make the assistant's
 * behaviour drift between messages."
 */
export function buildOlderSummary(
  messages: ChatMessage[],
  recentCount: number = MAX_RECENT_TURNS,
  maxChars: number = 2000,
): string | null {
  if (messages.length <= recentCount + 1) return null;

  const system = messages[0]?.role === 'system' ? messages[0] : null;
  const nonSystem = system ? messages.slice(1) : messages;
  const older = nonSystem.slice(0, -recentCount);

  if (older.length === 0) return null;

  const summary = older
    .map((m) => {
      const role = m.role === 'assistant' ? 'Assistant' : 'User';
      const content = m.content.length > 200
        ? m.content.slice(0, 200) + '…'
        : m.content;
      return `${role}: ${content}`;
    })
    .join('\n');

  if (summary.length > maxChars) {
    return summary.slice(0, maxChars) + '…';
  }
  return summary;
}

// ─── Memory tier ─────────────────────────────────────────────────────

/**
 * Format approved memory facts for injection.
 * Hard cap: a fraction of the input budget (10% by default).
 * Never inject proposed/unapproved memories.
 */
export function buildMemoryTier(
  facts: Array<{ content: string; category: string }>,
  tokenBudget: number = MEMORY_TOKEN_BUDGET,
): string | null {
  if (facts.length === 0) return null;

  const charBudget = tokenBudget * CHARS_PER_TOKEN;
  const lines: string[] = [];
  let totalChars = 0;

  for (const fact of facts) {
    const line = `[${fact.category}] ${fact.content}`;
    if (totalChars + line.length > charBudget) break;
    lines.push(line);
    totalChars += line.length;
  }

  if (lines.length === 0) return null;
  return lines.join('\n');
}

// ─── Tool result tier ────────────────────────────────────────────────

/**
 * Summarize tool results for prompt injection.
 * Never inject raw API payloads — always truncate and summarize.
 */
export function buildToolResultTier(
  results: Array<{ toolId: string; result: string; success: boolean }>,
  maxCharsPerResult: number = 500,
): string | null {
  if (results.length === 0) return null;

  const lines: string[] = [];
  for (const r of results) {
    const truncated = r.result.length > maxCharsPerResult
      ? r.result.slice(0, maxCharsPerResult) + '…'
      : r.result;
    const status = r.success ? '✓' : '✗';
    lines.push(`[${status} ${r.toolId}] ${truncated}`);
  }

  return lines.join('\n');
}

// ─── Assembler ───────────────────────────────────────────────────────

/**
 * Assemble the full compact context from all tiers.
 * This is the input to the 12-segment prompt compiler.
 */
export function assembleCompactContext(
  messages: ChatMessage[],
  options: {
    memoryFacts?: Array<{ content: string; category: string }>;
    retrievalSources?: string;
    toolResults?: Array<{ toolId: string; result: string; success: boolean }>;
    maxContextTokens?: number;
  } = {},
): CompactContext {
  const budget = calculateBudget(options.maxContextTokens);

  const recentTurns = selectRecentTurns(messages);
  const olderSummary = buildOlderSummary(messages, MAX_RECENT_TURNS, budget.summaryTokens * CHARS_PER_TOKEN);
  const memoryBlock = buildMemoryTier(options.memoryFacts ?? [], budget.memoryTokens);
  const retrievalBlock = options.retrievalSources ?? null;
  const toolResultBlock = buildToolResultTier(options.toolResults ?? []);

  const totalEstimateTokens =
    estimateTurnTokens(recentTurns) +
    estimateStringTokens(olderSummary) +
    estimateStringTokens(memoryBlock) +
    estimateStringTokens(retrievalBlock) +
    estimateStringTokens(toolResultBlock);

  return {
    recentTurns,
    olderSummary,
    memoryBlock,
    retrievalBlock,
    toolResultBlock,
    totalEstimateTokens,
  };
}

// ─── Utilities ───────────────────────────────────────────────────────

function estimateTurnTokens(messages: ChatMessage[]): number {
  const totalChars = messages.reduce((sum, m) => sum + m.content.length, 0);
  return Math.ceil(totalChars / CHARS_PER_TOKEN);
}

function estimateStringTokens(text: string | null): number {
  if (!text) return 0;
  return Math.ceil(text.length / CHARS_PER_TOKEN);
}

/**
 * Check if compaction is needed — context exceeds budget.
 */
export function needsCompaction(
  messages: ChatMessage[],
  maxContextTokens: number = CHAT_CONTEXT_TOKENS,
): boolean {
  const totalChars = messages.reduce((sum, m) => sum + m.content.length, 0);
  const estimatedTokens = Math.ceil(totalChars / CHARS_PER_TOKEN);
  const budget = maxContextTokens - CHAT_MAX_OUTPUT_TOKENS;
  return estimatedTokens > budget;
}
