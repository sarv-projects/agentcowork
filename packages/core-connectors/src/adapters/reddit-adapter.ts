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
import { requestConnector } from '../connection-manager.js';

/**
 * Reddit adapter — host-mediated OAuth, mobile-friendly community Q&A feed.
 *
 * Free for personal-use AI tools (Reddit's commercial API rules don't apply
 * to non-commercial end-user rate <100 req/min).
 *
 * Endpoints:
 *   GET https://oauth.reddit.com/search?q={q}&sort=relevance&limit={n}
 *   GET https://oauth.reddit.com/r/{subreddit}/comments/{id}
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'query', type: 'string', description: 'Search text or r/subreddit_topic' },
    { name: 'subreddit', type: 'string', description: 'Optional subreddit name (without r/)' },
    { name: 'sort', type: 'string', description: 'relevance|hot|top|new|comments (default relevance)' },
    { name: 'limit', type: 'number', description: 'Max results (default 10)' },
  ],
};

const REDDIT_API = 'https://oauth.reddit.com';

export class RedditAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'reddit';
  readonly credentialMode = 'host-mediated' as const;
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['reddit', 'subreddit', '/r/', 'opinions on', 'discussions about', 'thread', 'community', 'reddit search'];
    if (terms.some((t) => q.includes(t))) return 0.85;
    // Detect "what do people say about X" patterns
    if (/what do people think|what do reddit|reddit thinks/i.test(q)) return 0.7;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    const text = query.text || '';
    const subredditMatch = text.match(/\/r\/([a-z0-9_]+)/i);
    const subreddit = subredditMatch ? subredditMatch[1]! : '';
    const cleaned = subredditMatch ? text.replace(subredditMatch[0], '').trim() : text;
    return { query: cleaned, subreddit, sort: 'relevance', limit: 10 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { query?: string; subreddit?: string; sort?: string; limit?: number };
    const q = (f.query || '').trim();
    const sort = (f.sort || 'relevance').toLowerCase();
    const limit = Math.min(Math.max(Number(f.limit) || 10, 1), 25);
    if (!q && !f.subreddit) return { items: [], totalCount: 0, source: this.name };

    const path = f.subreddit ? `/r/${encodeURIComponent(f.subreddit)}/${sort}` : `/search`;
    const params = new URLSearchParams({ limit: String(limit), restrict_sr: f.subreddit ? 'true' : 'false' });
    if (q) params.set('q', q);
    if (!f.subreddit) params.set('sort', sort);
    const url = `${REDDIT_API}${path}?${params.toString()}`;

    try {
      const res = await requestConnector({
        connector: this.name,
        userId: ctx.userId,
        request: {
          url,
          method: 'GET',
          headers: {
            'User-Agent': 'PersonalAI/1.0 (https://github.com/sarv-projects/APP)',
            Accept: 'application/json',
          },
        },
        ...(ctx.signal ? { signal: ctx.signal } : {}),
      });
      if (!res?.ok) return { items: [], totalCount: 0, source: this.name };
      const raw = (await res.json()) as {
        data?: {
          children?: Array<{
            data?: {
              id: string;
              title: string;
              subreddit: string;
              score: number;
              num_comments: number;
              permalink: string;
              url: string;
              selftext?: string;
              author: string;
            };
          }>;
        };
      };
      const items: ConnectorResult['items'] = (raw.data?.children ?? [])
        .map((c) => c.data)
        .filter((d): d is NonNullable<typeof d> => !!d)
        .map((d) => ({
          id: `reddit-${d.id}`,
          title: d.title,
          snippet: `${d.score} pts • ${d.num_comments} comments • by u/${d.author}${d.selftext ? ' • ' + d.selftext.slice(0, 120) : ''}`.slice(0, 280),
          url: d.permalink ? `https://reddit.com${d.permalink}` : d.url,
          metadata: {
            subreddit: d.subreddit,
            score: d.score,
            comments: d.num_comments,
            author: d.author,
          },
        }));
      return { items, totalCount: items.length, source: this.name };
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }

}
