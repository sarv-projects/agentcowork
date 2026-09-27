import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';

const TTL_MS = 24 * 60 * 60 * 1000;
const MAX_ENTRIES = 50;

interface CacheEntry {
  results: SearchResult[];
  expiresAt: number;
}

function normalizeQuery(query: string): string {
  return query.trim().toLowerCase();
}

export class SearchCache {
  private store = new Map<string, CacheEntry>();

  get(query: string): SearchResult[] | null {
    const key = normalizeQuery(query);
    const entry = this.store.get(key);
    if (!entry) {
      return null;
    }
    if (Date.now() > entry.expiresAt) {
      this.store.delete(key);
      return null;
    }
    return entry.results;
  }

  set(query: string, results: SearchResult[]): void {
    const key = normalizeQuery(query);
    this.store.set(key, {
      results,
      expiresAt: Date.now() + TTL_MS,
    });
    this.evict();
  }

  clear(): void {
    this.store.clear();
  }

  private evict(): void {
    if (this.store.size <= MAX_ENTRIES) return;
    let oldestKey: string | null = null;
    let oldestExp = Infinity;
    for (const [key, entry] of this.store) {
      if (entry.expiresAt < oldestExp) {
        oldestExp = entry.expiresAt;
        oldestKey = key;
      }
    }
    if (oldestKey) this.store.delete(oldestKey);
  }
}

let sharedCache: SearchCache | null = null;

export function getSearchCache(): SearchCache {
  if (!sharedCache) {
    sharedCache = new SearchCache();
  }
  return sharedCache;
}

export class SearchCacheProvider implements SearchProvider {
  name = 'Search Cache';
  kind = 'search' as const;

  constructor(private cache: SearchCache = getSearchCache()) {}

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return true;
  }

  async search(query: string): Promise<SearchResult[]> {
    return this.cache.get(query) ?? [];
  }
}