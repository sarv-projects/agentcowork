/**
 * Exa REST search provider.
 *
 * Credential and egress custody (P69.C4/D4): the key lives in
 * `agentcowork-vault` and the request leaves through Guard-2 `netfloor`. This
 * provider therefore holds only a *provider id* and an endpoint — it never
 * reads `process.env`, never accepts an API key argument, and never calls
 * `fetch` itself. No governed transport attached ⇒ unavailable.
 */
import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';
import {
  getGovernedSearchTransport,
  type GovernedSearchTransport,
} from '../governed-transport.js';

const EXA_API_URL = 'https://api.exa.ai/search';

export class ExaRestSearchProvider implements SearchProvider {
  name = 'Exa REST';
  kind = 'search' as const;

  constructor(private readonly transport: GovernedSearchTransport | null = getGovernedSearchTransport()) {}

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return this.transport !== null;
  }

  async search(query: string): Promise<SearchResult[]> {
    if (!this.transport) throw new Error('Governed search transport not attached');

    const res = await this.transport.request({
      provider: 'exa',
      url: EXA_API_URL,
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      // No `x-api-key`: the host resolves the vault secret and adds the header.
      body: {
        query,
        numResults: 8,
        type: 'auto',
        contents: {
          highlights: true,
          text: { maxCharacters: 300 },
        },
      },
      timeoutMs: 8_000,
    });

    if (res.status >= 400) {
      const detail = await res.text().catch(() => '');
      console.warn(`[ExaRest] ${res.status}: ${detail.slice(0, 100)}`);
      return [];
    }

    const body = (await res.json()) as {
      results?: Array<{
        title?: string;
        url?: string;
        text?: string;
        highlights?: string[];
        score?: number;
        publishedDate?: string;
      }>;
    };

    return (body.results ?? []).map((r) => ({
      title: r.title ?? '',
      url: r.url ?? '',
      snippet: r.highlights?.[0] ?? r.text ?? '',
      score: r.score ?? 0,
      source: 'Exa',
    }));
  }
}
