import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SearchContext, SearchResult } from '@agentcowork/core-domain';
import { SearchCache, SearchCacheProvider, getSearchCache } from '../cache/search-cache.js';

const sampleResults: SearchResult[] = [
  { title: 'Result A', url: 'https://a.test', snippet: 'Snippet A', score: 1, source: 'Test' },
  { title: 'Result B', url: 'https://b.test', snippet: 'Snippet B', score: 0.8, source: 'Test' },
];

const ctx: SearchContext = {
  hasNativeGrounding: false,
  hasByokSearchKey: false,
  query: 'test query',
  userId: 'test-user',
};

describe('SearchCache', () => {
  let cache: SearchCache;

  beforeEach(() => {
    cache = new SearchCache();
  });

  describe('get', () => {
    it('returns null when cache is empty', () => {
      expect(cache.get('anything')).toBeNull();
    });
  });

  describe('get/set', () => {
    it('stores and retrieves results by query', () => {
      cache.set('hello world', sampleResults);
      expect(cache.get('hello world')).toEqual(sampleResults);
    });

    it('normalizes query — "Hello World" and "  hello world  " return same results', () => {
      cache.set('Hello World', sampleResults);
      expect(cache.get('  hello world  ')).toEqual(sampleResults);
    });

    it('is case insensitive — "Test" and "test" return same results', () => {
      cache.set('Test', sampleResults);
      expect(cache.get('test')).toEqual(sampleResults);
    });
  });

  describe('expiry', () => {
    beforeEach(() => {
      vi.useFakeTimers();
    });

    afterEach(() => {
      vi.useRealTimers();
    });

    it('returns null for expired entries', () => {
      cache.set('expire-me', sampleResults);

      // Advance past the 24h TTL
      vi.advanceTimersByTime(86400001);

      expect(cache.get('expire-me')).toBeNull();
    });
  });

  describe('set', () => {
    it('overwrites existing entry for same query', () => {
      const newerResults: SearchResult[] = [
        { title: 'Newer', url: 'https://new.test', snippet: 'Updated', score: 1, source: 'Test' },
      ];

      cache.set('query', sampleResults);
      cache.set('query', newerResults);

      expect(cache.get('query')).toEqual(newerResults);
    });
  });

  describe('clear', () => {
    it('removes all entries', () => {
      cache.set('a', sampleResults);
      cache.set('b', sampleResults);

      cache.clear();

      expect(cache.get('a')).toBeNull();
      expect(cache.get('b')).toBeNull();
    });

    it('after clear, get returns null', () => {
      cache.set('something', sampleResults);
      cache.clear();
      expect(cache.get('something')).toBeNull();
    });
  });
});

describe('SearchCacheProvider', () => {
  let cache: SearchCache;
  let provider: SearchCacheProvider;

  beforeEach(() => {
    cache = new SearchCache();
    provider = new SearchCacheProvider(cache);
  });

  it('returns cached results via search', async () => {
    cache.set('hello', sampleResults);
    const results = await provider.search('hello');
    expect(results).toEqual(sampleResults);
  });

  it('returns empty array when cache misses', async () => {
    const results = await provider.search('nothing-cached');
    expect(results).toEqual([]);
  });

  it('isAvailable always returns true', async () => {
    expect(await provider.isAvailable(ctx)).toBe(true);
  });

  it('uses search cache correctly', async () => {
    // First call populates cache
    const first = await provider.search('populate');
    expect(first).toEqual([]);

    // Manually set cache via underlying cache
    cache.set('populate', sampleResults);

    // Second call reads from cache
    const second = await provider.search('populate');
    expect(second).toEqual(sampleResults);
  });
});

describe('getSearchCache singleton', () => {
  it('returns the same instance on multiple calls', () => {
    const a = getSearchCache();
    const b = getSearchCache();
    expect(a).toBe(b);
  });

  it('multiple calls share the same cache store', () => {
    const first = getSearchCache();
    const second = getSearchCache();

    first.set('shared', sampleResults);
    expect(second.get('shared')).toEqual(sampleResults);

    // Clear via one, both see the effect
    first.clear();
    expect(second.get('shared')).toBeNull();
  });
});
