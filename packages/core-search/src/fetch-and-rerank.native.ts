import type { SearchContext, SearchResult } from '@agentcowork/core-domain';
import { rerankByBm25 } from './bm25-rerank.js';
import { WebFetchCascade } from './fetch-cascade.js';
import { ReadabilityFetcherProvider } from './providers/readability-fetcher.native.js';
import { getCachedContent, setCachedContent } from './parallel-web-search-cascade.js';

const SNIPPET_MAX_LEN = 3000;
const MAX_CONCURRENT_FETCHES = 5;

function excerpt(text: string, maxLen = SNIPPET_MAX_LEN): string {
  const trimmed = text.replace(/\s+/g, ' ').trim();
  if (trimmed.length <= maxLen) {
    return trimmed;
  }
  return `${trimmed.slice(0, maxLen)}…`;
}

async function pooledFetch<T>(
  tasks: Array<() => Promise<T>>,
  concurrency: number,
): Promise<T[]> {
  const results: T[] = [];
  let i = 0;
  async function next(): Promise<void> {
    while (i < tasks.length) {
      const idx = i++;
      results[idx] = await tasks[idx]!();
    }
  }
  await Promise.all(Array.from({ length: Math.min(concurrency, tasks.length) }, () => next()));
  return results;
}

/**
 * Fetch top search result URLs and rerank fetched content with BM25-lite.
 * Uses content cache (6h TTL) and concurrency pool (5 max) to limit memory + bandwidth.
 */
export async function fetchAndRerankSearchResults(
  query: string,
  results: SearchResult[],
  ctx: SearchContext,
  options: { maxUrls?: number } = {},
): Promise<SearchResult[]> {
  const maxUrls = options.maxUrls ?? Math.min(8, results.length);
  if (maxUrls === 0 || results.length === 0) {
    return results;
  }

  const fetchCascade = new WebFetchCascade([new ReadabilityFetcherProvider()]);
  const candidates = results.slice(0, maxUrls);

  const fetchTasks = candidates.map((result) => async () => {
    if (!result.url) return { result, text: result.snippet || result.content || '' };

    const cached = getCachedContent(result.url);
    if (cached) return { result, text: cached };

    try {
      const content = await fetchCascade.fetch(result.url, ctx);
      if (content && content.trim().length > 0) {
        setCachedContent(result.url, content);
      }
      return { result, text: content };
    } catch {
      return { result, text: result.snippet || result.content || '' };
    }
  });

  const fetched = await pooledFetch(fetchTasks, MAX_CONCURRENT_FETCHES);

  const ranked = rerankByBm25(
    query,
    fetched.map((entry) => ({
      text: entry.text,
      url: entry.result.url,
      title: entry.result.title,
    })),
  );

  const byUrl = new Map(fetched.map((entry) => [entry.result.url, entry.result]));

  return ranked.map((item) => {
    const original = byUrl.get(item.url);
    if (!original) {
      return {
        title: item.title ?? item.url,
        url: item.url,
        snippet: excerpt(item.text),
        content: item.text,
        score: item.score,
        source: 'fetch',
      };
    }
    return {
      ...original,
      snippet: excerpt(item.text || original.snippet),
      content: item.text || original.content || '',
      score: item.score,
      title: item.title || original.title,
      url: item.url,
    };
  });
}
