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
 * OpenLibrary adapter — book/author metadata search, no auth, free.
 *
 * The Open Library is a project of the Internet Archive; ~100 req/5 min cap.
 * Covers 30M+ books with author + publication + ISBN coverage.
 *
 * Endpoints:
 *   GET https://openlibrary.org/search.json?q={q}&limit={n}
 *   GET https://openlibrary.org/search.json?author={author}&title={title}&limit={n}
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'query', type: 'string', description: 'Title, author, or ISBN to search' },
    { name: 'limit', type: 'number', description: 'Max results (default 10)' },
  ],
};

const OL_API = 'https://openlibrary.org';

export class OpenLibraryAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'openlibrary';
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['book', 'novel', 'author', 'isbn', 'openlibrary', 'who wrote', 'wrote the book', 'book by', 'published', 'book summary'];
    if (terms.some((t) => q.includes(t))) return 0.85;
    // Detect isbn pattern (10 or 13 digit)
    if (/\b\d{9}[\dxX]\b|\b97[89]\d{10}\b/.test(q)) return 0.9;
    if (/who wrote|written by|author of/.test(q)) return 0.7;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text || '', limit: 10 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { query?: string; limit?: number };
    const q = (f.query || '').trim();
    if (!q) return { items: [], totalCount: 0, source: this.name };
    const limit = Math.min(Math.max(Number(f.limit) || 10, 1), 50);
    const url = `${OL_API}/search.json?q=${encodeURIComponent(q)}&limit=${limit}`;
    try {
      const res = await fetch(url, {
        headers: { Accept: 'application/json', 'User-Agent': 'PersonalAI/1.0 (Books)' },
        signal: ctx.signal ?? null,
      });
      if (!res.ok) return { items: [], totalCount: 0, source: this.name };
      const raw = (await res.json()) as { docs?: Array<{
        key: string;
        title: string;
        author_name?: string[];
        first_publish_year?: number;
        publisher?: string[];
        isbn?: string[];
        cover_i?: number;
        ratings_average?: number;
        subject?: string[];
      }> };
      const items: ConnectorResult['items'] = (raw.docs ?? []).map((b) => ({
        id: `book-${b.key?.replace('/works/', '') || Math.random().toString(36).slice(2)}`,
        title: b.title,
        snippet: [
          (b.author_name || []).slice(0, 2).join(', '),
          b.first_publish_year ? `first published ${b.first_publish_year}` : '',
          (b.publisher || [])[0] || '',
          typeof b.ratings_average === 'number' ? `★ ${b.ratings_average.toFixed(1)}` : '',
        ].filter(Boolean).join(' · ').slice(0, 280),
        url: `https://openlibrary.org${b.key}`,
        metadata: {
          authors: (b.author_name || []).join(', '),
          year: b.first_publish_year,
          publisher: (b.publisher || [])[0],
          isbn: (b.isbn || [])[0],
          cover_id: b.cover_i,
          rating: b.ratings_average,
        },
      }));
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
