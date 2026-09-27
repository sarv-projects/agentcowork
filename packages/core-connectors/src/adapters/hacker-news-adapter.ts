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
 * Hacker News connector — Algolia HN Search API (no auth, no signup, no key).
 * https://hn.algolia.com/api
 * Use Algolia (not Firebase) so we get full text in one request.
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'query', type: 'string', description: 'Search query' },
    { name: 'tags', type: 'string', description: 'Optional tags filter, e.g. "story" or "front_page"' },
    { name: 'limit', type: 'number', description: 'Max results (default 10)' },
  ],
};

const HN_API = 'https://hn.algolia.com/api/v1';

export class HackerNewsAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'hacker-news';
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['hacker news', 'hn', 'tech news', 'startup', 'show hn', 'ask hn'];
    if (terms.some((t) => q.includes(t))) return 0.9;
    // Lightly score general tech questions
    if (/(latest|trending|today|new).*\b(tech|news|story|stories|article|post)/.test(q)) return 0.5;
    return 0.15;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    const text = (query.text || '').trim();
    // If user asks for top/front page (no query), use search_by_date with empty q
    const isTopQuery = /^(top|front|trending|popular|latest)\b/i.test(text);
    return {
      query: isTopQuery ? '' : text,
      tags: isTopQuery ? 'front_page' : '',
      limit: 10,
    };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { query?: string; tags?: string; limit?: number };
    const query = f.query || '';
    const tags = (f.tags || '').trim();
    const limit = Math.min(Math.max(Number(f.limit) || 10, 1), 30);

    try {
      const params = new URLSearchParams();
      params.set('hitsPerPage', String(limit));
      if (query) params.set('query', query);
      if (tags) params.set('tags', tags);

      const res = await fetch(`${HN_API}/search?${params.toString()}`, {
        headers: { Accept: 'application/json' },
        signal: ctx.signal ?? null,
      });
      if (!res.ok) {
        return { items: [], totalCount: 0, source: this.name };
      }
      const data = (await res.json()) as {
        hits?: Array<{
          objectID?: string;
          title?: string | null;
          story_title?: string | null;
          url?: string | null;
          story_url?: string | null;
          author?: string;
          points?: number | null;
          num_comments?: number | null;
          created_at_i?: number;
        }>;
      };
      const items: ConnectorResult['items'] = (data.hits ?? []).map((h) => {
        const title = h.title || h.story_title || '(untitled)';
        const url = h.url || h.story_url || (h.objectID ? `https://news.ycombinator.com/item?id=${h.objectID}` : '');
        const parts = [
          h.author ? `by ${h.author}` : '',
          typeof h.points === 'number' ? `${h.points} pts` : '',
          typeof h.num_comments === 'number' ? `${h.num_comments} comments` : '',
        ].filter(Boolean);
        return {
          id: h.objectID || '',
          title,
          snippet: parts.join(' • '),
          url,
          date: h.created_at_i ? new Date(h.created_at_i * 1000).toISOString() : '',
          metadata: { points: h.points ?? 0, comments: h.num_comments ?? 0 },
        };
      });
      return { items, totalCount: items.length, source: this.name };
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }

  /** 
   * Token refresh is handled by the Cloudflare Worker OAuth proxy.
   * This adapter assumes a valid token is injected via filter.token.
   * @see packages/cloudflare-server/src/index.ts OAuth refresh routes
   */
}
