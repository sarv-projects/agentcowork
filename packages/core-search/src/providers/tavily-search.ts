/**
 * Tavily Search Provider — AI-optimized search with content extraction.
 * Free tier: 1000 requests/month. https://tavily.com
 *
 * Returns an "answer" summary plus extracted content from web results,
 * similar to Perplexity-level search quality.
 *
 * Credential and egress custody (P69.C4/D4): this provider never reads an API
 * key, never reads `process.env` and never calls `fetch` itself. It asks the
 * host through the governed transport, which resolves the `tavily` secret from
 * `agentcowork-vault` and routes the request through Guard-2 `netfloor`. With no
 * transport attached the provider reports unavailable — there is deliberately
 * no environment-variable fallback.
 */
import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';
import {
  getGovernedSearchTransport,
  type GovernedSearchTransport,
} from '../governed-transport.js';

const TAVILY_ENDPOINT = 'https://api.tavily.com/search';

type TavilyResponse = {
  query: string;
  answer?: string;
  results: Array<{
    title: string;
    url: string;
    content: string;
    score: number;
    raw_content?: string;
  }>;
  response_time: number;
};

export class TavilySearchProvider implements SearchProvider {
  readonly name = 'tavily';
  readonly kind = 'search' as const;

  constructor(private readonly transport: GovernedSearchTransport | null = getGovernedSearchTransport()) {}

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return this.transport !== null;
  }

  async search(query: string): Promise<SearchResult[]> {
    if (!this.transport) return [];

    try {
      const response = await this.transport.request({
        provider: 'tavily',
        url: TAVILY_ENDPOINT,
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        // No `api_key` field: the host injects the vault credential.
        body: {
          query,
          search_depth: 'advanced',
          include_answer: true,
          include_raw_content: false,
          max_results: 10,
        },
        timeoutMs: 15_000,
      });

      if (response.status >= 400) {
        console.warn(`[Tavily] HTTP ${response.status}`);
        return [];
      }

      const data = (await response.json()) as TavilyResponse;
      const results: SearchResult[] = [];

      if (data.answer) {
        results.push({
          title: 'AI Answer',
          url: '',
          snippet: data.answer,
          score: 100,
          source: 'tavily-answer',
        });
      }

      for (const r of data.results ?? []) {
        results.push({
          title: r.title,
          url: r.url,
          snippet: r.content || r.raw_content || '',
          score: r.score * 10,
          source: 'tavily',
        });
      }

      return results;
    } catch (e) {
      console.warn('[Tavily] search failed:', e);
      return [];
    }
  }
}
