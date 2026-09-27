import type {
  FactCandidate,
  FactWriteResult,
  MemoryFact,
  MemoryRepository,
  RecallOptions,
} from '@agentcowork/core-domain';
import { normalizeMemoryCategory } from '@agentcowork/core-domain';
import { MemoryConflictResolver, type EmbedVectorFn } from './conflict.js';

export type RememberOptions = {
  category?: string;
  subcategory?: string;
  tags?: string[];
  source?: string;
  sourceId?: string;
  projectId?: string;
  confidence?: 'high' | 'low';
};

export class MemoryService {
  private readonly conflictResolver: MemoryConflictResolver;

  constructor(
    private readonly repository: MemoryRepository,
    embedFn: EmbedVectorFn,
  ) {
    this.conflictResolver = new MemoryConflictResolver(embedFn);
  }

  /** Smart store with embedding-based dedup and conflict resolution (§8.5). */
  async remember(content: string, options: RememberOptions = {}): Promise<MemoryFact> {
    const candidate: FactCandidate = {
      content: content.trim(),
      category: normalizeMemoryCategory(options.category ?? 'personal'),
      source: options.source ?? 'chat',
      confidence: options.confidence ?? 'low',
      ...(options.subcategory ? { subcategory: options.subcategory } : {}),
      ...(options.tags?.length ? { tags: options.tags } : {}),
      ...(options.sourceId ? { sourceId: options.sourceId } : {}),
      ...(options.projectId ? { projectId: options.projectId } : {}),
    };

    const existing = await this.repository.listFacts({
      categories: [candidate.category],
      includeInactive: false,
    });

    const detection = await this.conflictResolver.detect(candidate, existing);

    if (detection.type === 'duplicate') {
      return this.conflictResolver.resolveDuplicate(this.repository, detection.existingFact);
    }

    if (detection.type === 'conflict') {
      return this.conflictResolver.resolveConflict(
        this.repository,
        detection.existingFact,
        candidate,
      );
    }

    const result = await this.repository.store(candidate);
    if (result === 'stored' || result === 'superseded') {
      // Prefer exact match by content+category over listFacts[0] (which may be unrelated).
      const latest = await this.repository.listFacts({
        category: candidate.category,
        includeInactive: true,
        limit: 20,
      });
      const stored = latest.find(
        (f) => f.content === candidate.content && f.category === candidate.category,
      ) ?? latest[0];
      if (!stored) throw new Error('Failed to retrieve stored fact');
      if (stored.id > 0) {
        return stored;
      }
      throw new Error('Memory fact stored but could not be reloaded with a valid id');
    }

    throw new Error(`Failed to store memory fact: ${result}`);
  }

  /** Scoped recall — categories, subcategory, source_id filters. */
  async recall(query: string, options: RecallOptions = {}): Promise<MemoryFact[]> {
    return this.repository.recall(query, options);
  }

  async getByCategory(category: string, limit = 20): Promise<MemoryFact[]> {
    return this.repository.recall('', {
      categories: [normalizeMemoryCategory(category)],
      limit,
    });
  }

  async storeCandidate(candidate: FactCandidate): Promise<FactWriteResult> {
    const normalized: FactCandidate = {
      ...candidate,
      category: normalizeMemoryCategory(candidate.category),
    };

    const existing = await this.repository.listFacts({
      categories: [normalized.category],
      includeInactive: false,
    });

    const detection = await this.conflictResolver.detect(normalized, existing);
    if (detection.type === 'duplicate') {
      await this.repository.bumpAccessCount(detection.existingFact.id);
      return 'duplicate';
    }
    if (detection.type === 'conflict') {
      if (normalized.confidence === 'high') {
        await this.repository.supersede(detection.existingFact.id, normalized);
        return 'superseded';
      }
      return 'drafted';
    }

    return this.repository.store(normalized);
  }

  async decayAll(): Promise<void> {
    await this.repository.decay();
  }
}

/** Returns true if >24 hours have elapsed since `lastDecayTimestamp`, or if never run. */
export function shouldRunDecay(lastDecayTimestamp: number | null): boolean {
  if (lastDecayTimestamp === null) return true;
  return Date.now() - lastDecayTimestamp > 24 * 60 * 60 * 1000;
}