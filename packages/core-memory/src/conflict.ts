import type { FactCandidate, MemoryFact, MemoryRepository } from '@agentcowork/core-domain';

export type EmbedVectorFn = (text: string) => Promise<Float32Array>;

export type ConflictDetectionResult =
  | { type: 'duplicate'; existingFact: MemoryFact; confidence: number }
  | { type: 'conflict'; existingFact: MemoryFact; confidence: number }
  | { type: 'new'; confidence: number };

const DUPLICATE_THRESHOLD = 0.92;
const CONFLICT_THRESHOLD = 0.75;

/** Max cached embeddings to avoid unbounded memory growth on-device. */
const MAX_CACHE_SIZE = 1_000;

function cosineSimilarity(a: Float32Array, b: Float32Array): number {
  // Mismatched dimensions are not comparable — do not silently truncate.
  if (a.length !== b.length || a.length === 0) {
    return 0;
  }
  let dot = 0;
  let normA = 0;
  let normB = 0;
  for (let i = 0; i < a.length; i += 1) {
    const av = a[i]!;
    const bv = b[i]!;
    dot += av * bv;
    normA += av * av;
    normB += bv * bv;
  }
  if (normA === 0 || normB === 0) {
    return 0;
  }
  return dot / (Math.sqrt(normA) * Math.sqrt(normB));
}

/**
 * Simple LRU map with max-size eviction. Used to cache fact embeddings so
 * that `detect()` does not re-embed every existing fact on each new store.
 * Keyed by fact ID; values are the embedding vectors.
 */
class EmbeddingCache {
  private cache = new Map<number, { embedding: Float32Array; content: string }>();

  get(factId: number, content: string): Float32Array | undefined {
    const entry = this.cache.get(factId);
    if (entry && entry.content === content) {
      // Move to most-recently-used (delete + re-insert preserves Map order).
      this.cache.delete(factId);
      this.cache.set(factId, entry);
      return entry.embedding;
    }
    // Content changed — invalidate stale entry.
    if (entry) this.cache.delete(factId);
    return undefined;
  }

  set(factId: number, content: string, embedding: Float32Array): void {
    if (this.cache.has(factId)) {
      this.cache.delete(factId);
    } else if (this.cache.size >= MAX_CACHE_SIZE) {
      // Evict oldest (first) entry.
      const oldest = this.cache.keys().next().value;
      if (oldest !== undefined) this.cache.delete(oldest);
    }
    this.cache.set(factId, { embedding, content });
  }

  /** Invalidate a single fact (e.g. after supersede or delete). */
  invalidate(factId: number): void {
    this.cache.delete(factId);
  }

  /** Bulk invalidate (e.g. after a schema migration resets facts). */
  invalidateAll(): void {
    this.cache.clear();
  }

  get size(): number {
    return this.cache.size;
  }
}

/** Spec §8.5 — embedding dedup (≥0.92) and conflict (0.75–0.92) detection. */
export class MemoryConflictResolver {
  private readonly embeddingCache = new EmbeddingCache();

  constructor(private readonly embedFn: EmbedVectorFn) {}

  async detect(
    candidate: FactCandidate,
    existingFacts: MemoryFact[],
  ): Promise<ConflictDetectionResult> {
    if (existingFacts.length === 0) {
      return { type: 'new', confidence: 1 };
    }

    const newEmbedding = await this.embedFn(candidate.content);

    // Only embed facts whose embeddings are not cached — avoids re-embedding
    // all 500+ facts on every single remember() call.
    const activeFacts = existingFacts.filter(f => f.isActive);

    const existingEmbeddings = await Promise.all(
      activeFacts.map(async (f) => {
        const cached = this.embeddingCache.get(f.id, f.content);
        if (cached) return cached;
        const embedding = await this.embedFn(f.content);
        this.embeddingCache.set(f.id, f.content, embedding);
        return embedding;
      }),
    );

    for (let i = 0; i < activeFacts.length; i++) {
      const existing = activeFacts[i]!;
      const existingEmbedding = existingEmbeddings[i]!;
      const similarity = cosineSimilarity(newEmbedding, existingEmbedding);

      if (similarity >= DUPLICATE_THRESHOLD) {
        return { type: 'duplicate', existingFact: existing, confidence: similarity };
      }

      if (
        similarity >= CONFLICT_THRESHOLD &&
        existing.category === candidate.category &&
        existing.subcategory === candidate.subcategory
      ) {
        return { type: 'conflict', existingFact: existing, confidence: similarity };
      }
    }

    return { type: 'new', confidence: 1 };
  }

  async resolveDuplicate(repository: MemoryRepository, existing: MemoryFact): Promise<MemoryFact> {
    await repository.bumpAccessCount(existing.id);
    const refreshed = await repository.getById(existing.id);
    return refreshed ?? existing;
  }

  async resolveConflict(
    repository: MemoryRepository,
    existing: MemoryFact,
    candidate: FactCandidate,
  ): Promise<MemoryFact> {
    if (candidate.confidence === 'high') {
      await repository.supersede(existing.id, candidate);
      // Invalidate the superseded fact's cached embedding.
      this.embeddingCache.invalidate(existing.id);
      const rows = await repository.listFacts({
        category: candidate.category,
        includeInactive: false,
      });
      const newest = rows.find((fact) => fact.supersedesId === existing.id);
      if (newest) {
        return newest;
      }
    }
    return existing;
  }

  /** Expose cache size for diagnostics / tests. */
  get cacheSize(): number {
    return this.embeddingCache.size;
  }
}
