/**
 * Wikipedia Search API — free, no API key.
 * Returns encyclopedia article snippets and page info.
 * https://www.mediawiki.org/wiki/API:Search
 */
import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';

type WikiSearchItem = {
  title: string;
  pageid: number;
  snippet: string;
  wordcount: number;
  timestamp: string;
};

export class WikipediaSearchProvider implements SearchProvider {
  readonly name = 'wikipedia';
  readonly kind = 'search' as const;

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return true;
  }

  async search(query: string): Promise<SearchResult[]> {
    const params = new URLSearchParams({
      action: 'query',
      list: 'search',
      srsearch: query,
      format: 'json',
      srlimit: '8',
      origin: '*',
    });
    const url = `https://en.wikipedia.org/w/api.php?${params}`;
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 10_000);

    try {
      const resp = await fetch(url, {
        signal: controller.signal,
        headers: { 'User-Agent': 'PersonalAI/1.0 (dev@personalai.app)' },
      });
      if (!resp.ok) return [];
      const data = (await resp.json()) as {
        query?: { search: WikiSearchItem[] };
      };

      const items = data.query?.search ?? [];
      if (items.length === 0) return [];

      return items.map((item, i) => ({
        title: item.title,
        url: `https://en.wikipedia.org/wiki/${encodeURIComponent(item.title.replace(/ /g, '_'))}`,
        snippet: item.snippet.replace(/<[^>]*>/g, ''),
        score: Math.max(1, 8 - i),
        source: 'wikipedia',
      }));
    } catch {
      return [];
    } finally {
      clearTimeout(timeout);
    }
  }
}
