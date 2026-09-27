import type {
  ConnectorAdapter,
  ConnectorContext,
  ConnectorFilter,
  ConnectorItem,
  ConnectorMetadataSchema,
  ConnectorName,
  ConnectorResult,
  MemoryFact,
  UserQuery,
} from '@agentcowork/core-domain';

/**
 * RSS / News connector (Day-0, zero cost, spec §12.1).
 * Basic fetch + very lightweight RSS/Atom item extraction (no external parser to avoid new deps).
 * Good enough for headlines + links. Real prod would use a small robust parser.
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'url', type: 'string', description: 'RSS or Atom feed URL' },
    { name: 'max', type: 'number', description: 'Max items to return (default 5)' },
  ],
};

function extractItems(xml: string, max: number): Array<{ title: string; link?: string; snippet: string; date?: string }> {
  const items: Array<{ title: string; link?: string; snippet: string; date?: string }> = [];
  // Naive but functional extraction for <item> and <entry>
  const itemRegex = /<(?:item|entry)[^>]*>([\s\S]*?)<\/(?:item|entry)>/gi;
  let match;
  let count = 0;

  while ((match = itemRegex.exec(xml)) && count < max) {
    let content = '';
    if (match && match[1]) {
      content = match[1];
    }
    const getTag = (tag: string) => {
      const r = new RegExp(`<${tag}[^>]*>([\\s\\S]*?)</${tag}>`, 'i');
      const m = r.exec(content);
      if (!m || !m[1]) return '';
      return m[1].replace(/<!\[CDATA\[([\s\S]*?)\]\]>/gi, '$1').trim();
    };

    const title = getTag('title').slice(0, 200);
    let link: string | undefined = getTag('link');
    if (!link) {
      const hrefMatch = /<link[^>]+href=["']([^"']+)["']/i.exec(content);
      if (hrefMatch && hrefMatch[1]) link = hrefMatch[1];
    }
    const desc = getTag('description') || getTag('summary') || getTag('content');
    const pub = getTag('pubDate') || getTag('updated') || getTag('published');

    if (title) {
      const it: { title: string; link?: string; snippet: string; date?: string } = {
        title,
        snippet: desc.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').slice(0, 280),
      };
      if (link) it.link = link;
      if (pub) it.date = pub;
      items.push(it);
      count++;
    }
  }
  return items;
}

export class RssAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'rss';
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    if (/rss|feed|news|latest|headlines|blog/.test(q)) return 0.9;
    return 0.15;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    const text = query.text || '';
    const urlMatch = text.match(/https?:\/\/[^\s]+feed|https?:\/\/[^\s]+\.xml|https?:\/\/[^\s]+rss/i);
    const u = urlMatch ? urlMatch[0] : 'https://hnrss.org/frontpage';
    return {
      url: u,
      max: 5,
    };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { url?: string; max?: number };
    const rawUrl = f.url;
    const max = f.max || 5;
    const url: string = (rawUrl && typeof rawUrl === 'string' && /^https?:\/\//.test(rawUrl)) ? rawUrl : '';
    if (!url) {
      return { items: [], totalCount: 0, source: 'rss' };
    }

    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 5000);
    try {
      const res = await fetch(url, {
        headers: { 'User-Agent': 'PersonalAI/1.0 (+https://example)' },
        signal: ctx.signal ?? controller.signal,
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const text = await res.text();
      const rawItems = extractItems(text, Math.min(10, Number(max) || 5));

      const items: ConnectorItem[] = rawItems.map((it, idx) => {
        const item: ConnectorItem = {
          id: `${(url as string)}#${idx}`,
          title: it.title,
          snippet: it.snippet || '',
          metadata: { feed: (url as string) },
        };
        if (it.link) item.url = it.link;
        if (it.date) item.date = it.date;
        return item;
      });

      return {
        items,
        totalCount: items.length,
        source: 'rss',
      };
    } catch {
      return { items: [], totalCount: 0, source: 'rss' };
    } finally {
      clearTimeout(timeout);
    }
  }

  /** 
   * Token refresh is handled by the Cloudflare Worker OAuth proxy.
   * This adapter assumes a valid token is injected via filter.token.
   * @see packages/cloudflare-server/src/index.ts OAuth refresh routes
   */
}
