/**
 * auto-promote.ts — Auto-promote correction patterns to persistent memory.
 *
 * When a correction has been detected 3+ times, this module:
 * 1. Checks for duplicate facts (exact content match)
 * 2. Stores the correction as a persistent memory fact
 * 3. Invokes an optional callback for event emission
 */

import type { MemoryRepository, MemoryFact } from '@agentcowork/core-domain';
import type { PromotionCandidate } from './correction-detector.js';
import { removeTrackingRow } from './correction-store.js';
import { correctionConfidenceFromCount } from './correction-store.js';

/** The memory category used for promoted learned behaviors. */
const PROMOTED_CATEGORY = 'personal';

export interface AutoPromoteResult {
  /** The newly stored or existing duplicate fact. */
  fact: MemoryFact;
  /** Whether a new fact was created (true) or a duplicate existed (false). */
  wasCreated: boolean;
}

export type OnPromoteCallback = (fact: MemoryFact, source: string) => void;

/**
 * Attempt to auto-promote a correction candidate to persistent memory.
 *
 * Checks dedup by exact content match before storing. If a fact with the
 * same content already exists, it bumps the existing fact's access count
 * instead of creating a duplicate.
 *
 * @param repository - MemoryRepository for storage
 * @param candidate - The promotion candidate from the detector
 * @param count - The current correction count (used for confidence)
 * @param onPromote - Optional callback for event emission (e.g. EventBus)
 * @returns The stored fact and whether it was newly created
 */
export async function autoPromote(
  repository: MemoryRepository,
  candidate: PromotionCandidate,
  count: number,
  onPromote?: OnPromoteCallback,
): Promise<AutoPromoteResult> {
  // Check for exact-content dedup in the target category
  const existing = await repository.listFacts({
    categories: [PROMOTED_CATEGORY],
    includeInactive: false,
  });

  const exactMatch = existing.find(
    (f) =>
      f.content.toLowerCase() === candidate.pattern.toLowerCase() &&
      f.isActive &&
      (f.subcategory === candidate.category || (!f.subcategory && candidate.category === 'style')),
  );

  if (exactMatch) {
    // Fact already exists — just bump its access count
    await repository.bumpAccessCount(exactMatch.id);
    return { fact: exactMatch, wasCreated: false };
  }

  // Store the promoted fact
  const confidence = correctionConfidenceFromCount(count);

  const factCandidate = {
    content: candidate.pattern,
    category: PROMOTED_CATEGORY,
    subcategory: candidate.category,
    source: 'learned_behavior',
    confidence: confidence >= 0.7 ? 'high' as const : 'low' as const,
    tags: ['learned_behavior'],
  };

  void repository.store(factCandidate);

  const stored = await repository.listFacts({
    category: PROMOTED_CATEGORY,
    includeInactive: true,
    limit: 10,
  });

  const newFact = stored.find(
    (f) => f.content === candidate.pattern && f.subcategory === candidate.category,
  );

  if (!newFact) {
    const fallback = stored.find((f) => f.content === candidate.pattern);
    if (!fallback) throw new Error('Failed to retrieve auto-promoted fact');
    await removeTrackingRow(repository, candidate.pattern, candidate.category);
    if (onPromote) onPromote(fallback, 'correction_detection');
    return { fact: fallback, wasCreated: true };
  }

  await removeTrackingRow(repository, candidate.pattern, candidate.category);
  if (onPromote) onPromote(newFact, 'correction_detection');
  return { fact: newFact, wasCreated: true };
}
