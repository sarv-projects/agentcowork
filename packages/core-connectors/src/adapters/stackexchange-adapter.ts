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
 * StackExchange adapter — search across all StackExchange sites (default: StackOverflow).
 *
 * Free without an API key; 300 requests/day per IP. Credential-bearing quota
 * elevation belongs to the Rust host and is not accepted by this adapter.
 *
 * Endpoints:
 *   GET https://api.stackexchange.com/2.3/search/advanced?order=desc&sort=relevance&q={q}
 *   GET https://api.stackexchange.com/2.3/questions?order=desc&sort=votes&site=stackoverflow
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'query', type: 'string', description: 'Search text (e.g. "kotlin coroutine cancellation")' },
    { name: 'site', type: 'string', description: 'StackExchange site (default stackoverflow)' },
    { name: 'limit', type: 'number', description: 'Max results (default 10)' },
    { name: 'tagged', type: 'string', description: 'Optional tag filter (e.g. typescript;react)' },
  ],
};

const SE_API = 'https://api.stackexchange.com/2.3';

export class StackExchangeAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'stackexchange';
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['stackoverflow', 'stackexchange', 'stack overflow', 'so question', 'how to code', 'error code', 'exception', 'sigsegv', 'ts error', "can't compile", 'compile error'];
    if (terms.some((t) => q.includes(t))) return 0.9;
    if (/\berror\b|\bbug\b|\b(undefined|null|nan|nan)\b|\bsegfault\b/i.test(q)) return 0.45;
    if (/\bhow (do|to|can)\b.*\b(in|with|on)\b/i.test(q)) return 0.4;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    const text = query.text || '';
    // Extract tag patterns like #typescript or [react]
    const tagMatch = text.match(/#([a-z0-9_-]+)/gi) || text.match(/\[([a-z0-9_-]+)\]/gi);
    const tagged = tagMatch ? tagMatch.map((t) => t.replace(/[#[\]]/g, '')).join(';') : '';
    const cleanedText = tagMatch ? text.replace(/#([a-z0-9_-]+)/gi, '').replace(/\[([a-z0-9_-]+)\]/gi, '').trim() : text;
    return { query: cleanedText || text, site: 'stackoverflow', tagged, limit: 10 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { query?: string; site?: string; tagged?: string; limit?: number };
    const q = (f.query || '').trim();
    if (!q && !f.tagged) return { items: [], totalCount: 0, source: this.name };
    const site = f.site || 'stackoverflow';
    const limit = Math.min(Math.max(Number(f.limit) || 10, 1), 30);
    const params = new URLSearchParams({
      order: 'desc',
      sort: 'relevance',
      site,
      pagesize: String(limit),
      filter: '!9Z(-x*)W', // include body excerpt, exclude noisy fields
    });
    if (q) params.set('q', q);
    if (f.tagged) params.set('tagged', f.tagged);

    try {
      const res = await fetch(`${SE_API}/search/advanced?${params.toString()}`, {
        headers: { Accept: 'application/json' },
        signal: ctx.signal ?? null,
      });
      if (!res.ok) return { items: [], totalCount: 0, source: this.name };
      const raw = (await res.json()) as { items?: Array<{
        question_id: number;
        title: string;
        link: string;
        score: number;
        answer_count: number;
        view_count: number;
        is_answered: boolean;
        tags: string[];
        body?: string;
      }> };
      const items: ConnectorResult['items'] = (raw.items ?? []).map((q2) => {
        const bodyExcerpt = (q2.body || '').replace(/<[^>]+>/g, '').slice(0, 200);
        return {
          id: `so-${q2.question_id}`,
          title: q2.title,
          snippet: `${q2.is_answered ? '✓' : '✗'} • ${q2.score} pts • ${q2.answer_count} answers • ${q2.view_count} views${bodyExcerpt ? ' • ' + bodyExcerpt : ''}`.slice(0, 280),
          url: q2.link,
          metadata: {
            score: q2.score,
            answers: q2.answer_count,
            views: q2.view_count,
            is_answered: q2.is_answered,
            tags: q2.tags?.join(','),
            body_excerpt: bodyExcerpt,
          },
        };
      });
      return { items, totalCount: items.length, source: this.name };
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }

}
