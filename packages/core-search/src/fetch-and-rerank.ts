import type { SearchContext, SearchResult } from '@agentcowork/core-domain';
import { rerankByBm25 } from './bm25-rerank.js';
import { WebFetchCascade } from './fetch-cascade.js';
import { ReadabilityFetcherProvider } from './providers/readability-fetcher.js';

const SNIPPET_MAX_LEN = 3000;

function excerpt(text: string, maxLen = SNIPPET_MAX_LEN): string {
  const trimmed = text.replace(/\s+/g, ' ').trim();
  if (trimmed.length <= maxLen) {
    return trimmed;
  }
  return `${trimmed.slice(0, maxLen)}…`;
}

/**
 * Fetch top search result URLs and rerank fetched content with BM25-lite.
 */
export async function fetchAndRerankSearchResults(
  query: string,
  results: SearchResult[],
  ctx: SearchContext,
  options: { maxUrls?: number } = {},
): Promise<SearchResult[]> {
  const maxUrls = options.maxUrls ?? Math.min(5, results.length);
  if (maxUrls === 0 || results.length === 0) {
    return results;
  }

  const fetchCascade = new WebFetchCascade([new ReadabilityFetcherProvider()]);
  const candidates = results.slice(0, maxUrls);

  const fetched = await Promise.all(
    candidates.map(async (result) => {
      try {
        const content = await fetchCascade.fetch(result.url, ctx);
        return { result, text: content };
      } catch {
        return { result, text: result.snippet || result.content || '' };
      }
    }),
  );

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