import type {
  ConnectorAdapter,
  ConnectorContext,
  ConnectorFilter,
  ConnectorMetadataSchema,
  ConnectorName,
  ConnectorResult,
  UserQuery,
  MemoryFact,
} from '@agentcowork/core-domain';

/**
 * Wikipedia connector — Wikimedia REST API.
 *
 * Free: 100% free, no API key, no signup. Rate limit: 200 req/min/IP
 * if a compliant User-Agent is set. Strict policy: must include a meaningful
 * User-Agent with contact info.
 *
 * Endpoints:
 *   GET https://en.wikipedia.org/api/rest_v1/page/summary/{title}
 *   GET https://en.wikipedia.org/api/rest_v1/search/page?q={query}&limit={n}
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'query', type: 'string', description: 'Article title or search query' },
    { name: 'limit', type: 'number', description: 'Max results (default 5)' },
  ],
};

const WIKI_REST = 'https://en.wikipedia.org/api/rest_v1';
// Required by Wikimedia's robot policy. Override in deploy config if needed.
const USER_AGENT = 'PersonalAI/1.0 (https://github.com/sarv-projects/APP)';

export class WikipediaAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'wikipedia';
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['wikipedia', 'wiki', 'who is', 'what is', 'tell me about', 'define', 'history of', 'biography'];
    return terms.some((t) => q.includes(t)) ? 0.85 : 0.2;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text || '', limit: 5 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { query?: string; limit?: number; mode?: 'summary' | 'search' };
    const query = (f.query || '').trim();
    const limit = Math.min(Math.max(Number(f.limit) || 5, 1), 10);
    if (!query) {
      return { items: [], totalCount: 0, source: this.name };
    }

    // Default: `summary` mode for "tell me about X" / "what is X" patterns.
    // Wikimedia's /page/summary/{title} returns the article extract + thumbnail +
    // description in one shot — much better for those prompts than /search/page.
    const mode = f.mode ?? (/(what|tell me about|who is|define|describe|history of|biography of)\b/i.test(query) ? 'summary' : 'search');

    try {
      if (mode === 'summary') {
        const summaryTitle = encodeURIComponent(query.replace(/\s+/g, '_'));
        const res = await fetch(`${WIKI_REST}/page/summary/${summaryTitle}`, {
          headers: { 'User-Agent': USER_AGENT, Accept: 'application/json' },
          signal: ctx.signal ?? null,
        });
        if (!res.ok) {
          // Fallback to search if summary can't resolve.
          const searchParams = new URLSearchParams({ q: query, limit: String(limit) });
          const fallbackRes = await fetch(`${WIKI_REST}/search/page?${searchParams.toString()}`, {
            headers: { 'User-Agent': USER_AGENT, Accept: 'application/json' },
            signal: ctx.signal ?? null,
          });
          if (!fallbackRes.ok) return { items: [], totalCount: 0, source: this.name };
          return this.parseSearch(await fallbackRes.json());
        }
        const data = (await res.json()) as {
          title?: string;
          extract?: string;
          description?: string;
          content_urls?: { desktop?: { page?: string } };
          thumbnail?: { source?: string };
        };
        const pageUrl = data.content_urls?.desktop?.page || (data.title ? `https://en.wikipedia.org/wiki/${encodeURIComponent(data.title.replace(/\s+/g, '_'))}` : '');
        return {
          items: [
            {
              id: data.title || query,
              title: data.title || query,
              snippet: (data.extract || data.description || '').slice(0, 280),
              url: pageUrl,
              metadata: { thumbnail: data.thumbnail?.source },
            },
          ],
          totalCount: 1,
          source: this.name,
        };
      }

      const params = new URLSearchParams({ q: query, limit: String(limit) });
      const res = await fetch(`${WIKI_REST}/search/page?${params.toString()}`, {
        headers: { 'User-Agent': USER_AGENT, Accept: 'application/json' },
        signal: ctx.signal ?? null,
      });
      if (!res.ok) {
        return { items: [], totalCount: 0, source: this.name };
      }
      return this.parseSearch(await res.json());
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }

  private async parseSearch(raw: unknown): Promise<ConnectorResult> {
    const data = raw as { pages?: Array<{ key?: string; title?: string; excerpt?: string; description?: string }> };
    const items: ConnectorResult['items'] = (data.pages ?? []).map((p, idx) => ({
      id: String(idx),
      title: p.title || p.key || 'Untitled',
      snippet: (p.excerpt || p.description || '').replace(/<[^>]+>/g, '').slice(0, 280),
      url: p.key ? `https://en.wikipedia.org/wiki/${encodeURIComponent(p.key)}` : '',
    }));
    return { items, totalCount: items.length, source: this.name };
  }

  /** 
   * Token refresh is handled by the Cloudflare Worker OAuth proxy.
   * This adapter assumes a valid token is injected via filter.token.
   * @see packages/cloudflare-server/src/index.ts OAuth refresh routes
   */
}
