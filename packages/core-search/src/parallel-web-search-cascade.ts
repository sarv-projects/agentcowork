import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';
import type { SearchCache } from './cache/search-cache.js';
import { rewriteSearchQuery } from './query-rewrite.js';

const CONTENT_CACHE_TTL = 6 * 60 * 60 * 1000; // 6 hours
const CONTENT_CACHE_MAX = 20; // LRU cap

interface ContentCacheEntry {
  text: string;
  fetchedAt: number;
}

const contentCache = new Map<string, ContentCacheEntry>();

function evictOldest(): void {
  if (contentCache.size <= CONTENT_CACHE_MAX) return;
  let oldestKey: string | null = null;
  let oldestTime = Infinity;
  for (const [key, entry] of contentCache) {
    if (entry.fetchedAt < oldestTime) {
      oldestTime = entry.fetchedAt;
      oldestKey = key;
    }
  }
  if (oldestKey) contentCache.delete(oldestKey);
}

export function getCachedContent(url: string): string | null {
  const entry = contentCache.get(url);
  if (!entry) return null;
  if (Date.now() - entry.fetchedAt > CONTENT_CACHE_TTL) {
    contentCache.delete(url);
    return null;
  }
  return entry.text;
}

export function setCachedContent(url: string, text: string): void {
  contentCache.set(url, { text, fetchedAt: Date.now() });
  evictOldest();
}

function normalizeUrl(url: string): string {
  try {
    const u = new URL(url);
    return `${u.hostname}${u.pathname}`.replace(/\/$/, '');
  } catch {
    return url;
  }
}

function deduplicateResults(results: SearchResult[]): SearchResult[] {
  const seen = new Set<string>();
  const deduplicated: SearchResult[] = [];

  for (const result of results) {
    if (!result.url) {
      deduplicated.push(result);
      continue;
    }
    const key = normalizeUrl(result.url);
    if (seen.has(key)) continue;
    seen.add(key);
    deduplicated.push(result);
  }

  return deduplicated;
}

export class ParallelWebSearchCascade {
  private providers: SearchProvider[] = [];
  private cache: SearchCache | null;

  constructor(providers: SearchProvider[], cache: SearchCache | null = null) {
    this.providers = providers.filter((p) => p.kind === 'search');
    this.cache = cache;
  }

  async search(query: string, ctx: SearchContext): Promise<SearchResult[]> {
    // C.20: read AND write the cache under the SAME rewritten key the sequential
    // cascade uses — the parallel path was reading/writing the raw query while
    // the sequential path used rewriteSearchQuery(query), so the shared cache
    // never matched across the two paths.
    const rewrittenQuery = rewriteSearchQuery(query);

    // Check cache first
    if (this.cache) {
      const cached = this.cache.get(rewrittenQuery);
      if (cached && cached.length > 0) {
        return cached;
      }
    }

    // Run ALL available providers in parallel
    const availableProviders = await this.getAvailableProviders(ctx);
    if (availableProviders.length === 0) return [];

    const settled = await Promise.allSettled(
      availableProviders.map(async (provider) => {
        try {
          const results = provider.search ? await provider.search(rewrittenQuery) : [];
          return { provider: provider.name, results: results ?? [] };
        } catch (e) {
          console.warn(`[ParallelSearch] ${provider.name} failed:`, e);
          return { provider: provider.name, results: [] };
        }
      }),
    );

    // Merge results from all providers
    const allResults: SearchResult[] = [];
    for (const outcome of settled) {
      if (outcome.status === 'fulfilled' && outcome.value.results.length > 0) {
        allResults.push(...outcome.value.results);
      }
    }

    if (allResults.length === 0) return [];

    // Deduplicate by URL domain+path
    const deduplicated = deduplicateResults(allResults);

    // Sort by score (highest first)
    deduplicated.sort((a, b) => (b.score ?? 0) - (a.score ?? 0));

    // Cap at top 30 results
    const topResults = deduplicated.slice(0, 30);

    // Cache the merged results
    if (this.cache) {
      this.cache.set(rewrittenQuery, topResults);
    }

    return topResults;
  }

  private async getAvailableProviders(ctx: SearchContext): Promise<SearchProvider[]> {
    const checks = await Promise.allSettled(
      this.providers.map(async (p) => {
        const available = await p.isAvailable(ctx);
        return { provider: p, available };
      }),
    );

    return checks
      .filter((c): c is PromiseFulfilledResult<{ provider: SearchProvider; available: boolean }> =>
        c.status === 'fulfilled' && c.value.available,
      )
      .map((c) => c.value.provider);
  }
}
