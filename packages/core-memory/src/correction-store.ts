/**
 * correction-store.ts — Persist correction counts across sessions.
 *
 * Uses the existing memory_facts table with:
 *   category: 'learned_behavior'  (normalized to 'other' by the repository)
 *   subcategory: the pattern category (style|format|content|behavior)
 *   content: the extracted preference string
 *   source: 'correction_detection'
 *
 * The access_count field serves as the correction counter.
 * On startup, stored patterns are loaded into the in-memory detector.
 */

import type { MemoryRepository, MemoryFact, FactCandidate } from '@agentcowork/core-domain';
import type { PromotionCandidate } from './correction-detector.js';
import { seedCorrectionCounts } from './correction-detector.js';

/** External category identifier used for scoped queries. */
const LEARNED_BEHAVIOR_CATEGORY = 'learned_behavior';
const CORRECTION_SOURCE = 'correction_detection';

/**
 * Compute a numeric correction confidence from a count.
 * Maps: 1→0.3, 2→0.5, 3→0.7, 4+→0.9  (caps at 0.9)
 */
export function correctionConfidenceFromCount(count: number): number {
  const raw = 0.1 + count * 0.2;
  return Math.min(0.9, Math.round(raw * 100) / 100);
}

/**
 * Load persisted correction patterns into the in-memory detector.
 * Call once at app startup.
 */
export async function loadCorrectionPatterns(repository: MemoryRepository): Promise<void> {
  const facts = await repository.listFacts({
    categories: [LEARNED_BEHAVIOR_CATEGORY],
    includeInactive: false,
  });

  const entries: Array<{ pattern: string; count: number }> = [];
  for (const fact of facts) {
    entries.push({
      pattern: fact.content,
      count: fact.accessCount + 1, // adjust for 0-based DB access_count
    });
  }

  seedCorrectionCounts(entries);
}

/**
 * Increment the correction count for a pattern in the persistent store.
 *
 * Creates a new row if this is the first time the pattern is seen,
 * or bumps the access_count on the existing row.
 *
 * @returns The updated count for this pattern.
 */
export async function incrementCorrectionCount(
  repository: MemoryRepository,
  candidate: PromotionCandidate,
): Promise<number> {
  // Find existing row for this pattern
  const existing = await repository.listFacts({
    categories: [LEARNED_BEHAVIOR_CATEGORY],
    includeInactive: true,
  });

  const match = existing.find(
    (f) => f.content === candidate.pattern && f.subcategory === candidate.category,
  );

  if (match) {
    // Increment existing counter
    await repository.bumpAccessCount(match.id);
    return match.accessCount + 1;
  }

  // Create new row for this pattern
  const factCandidate: FactCandidate = {
    content: candidate.pattern,
    category: LEARNED_BEHAVIOR_CATEGORY,
    subcategory: candidate.category,
    source: CORRECTION_SOURCE,
    confidence: 'low',
    tags: [],
  };

  await repository.store(factCandidate);
  return 1;
}

/**
 * Remove a learned-behavior tracking row after promotion.
 * Prevents the tracking row from cluttering memory injection.
 */
export async function removeTrackingRow(
  repository: MemoryRepository,
  pattern: string,
  category: string,
): Promise<void> {
  const existing = await repository.listFacts({
    categories: [LEARNED_BEHAVIOR_CATEGORY],
    includeInactive: true,
  });

  const match = existing.find(
    (f) => f.content === pattern && f.subcategory === category,
  );

  if (match) {
    await repository.delete(match.id);
  }
}

/**
 * List all tracked correction patterns with their counts.
 */
export async function listTrackedPatterns(
  repository: MemoryRepository,
): Promise<Array<{ fact: MemoryFact; count: number }>> {
  const facts = await repository.listFacts({
    categories: [LEARNED_BEHAVIOR_CATEGORY],
    includeInactive: false,
  });

  return facts.map((fact) => ({
    fact,
    count: fact.accessCount + 1,
  }));
}
