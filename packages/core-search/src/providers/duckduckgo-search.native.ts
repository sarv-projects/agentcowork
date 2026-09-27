/**
 * Mobile entry — lightweight HTML parser without cheerio (avoids node:stream).
 */
import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';

/**
 * Parse DuckDuckGo lite HTML result rows using regex (no DOM library needed).
 */
export function parseDuckDuckGoLiteHtml(html: string): SearchResult[] {
  const results: SearchResult[] = [];
  const linkRegex = /<a[^>]+class="result-link"[^>]*href="([^"]*)"[^>]*>([\s\S]*?)<\/a>/gi;
  const snippetRegex = /<td[^>]*class="result-snippet"[^>]*>([\s\S]*?)<\/td>/gi;

  const links: string[] = [];
  const titles: string[] = [];
  const snippets: string[] = [];

  let linkMatch: RegExpExecArray | null;
  while ((linkMatch = linkRegex.exec(html)) !== null) {
    links.push(linkMatch[1]!.trim());
    titles.push(linkMatch[2]!.replace(/<[^>]*>/g, '').trim());
  }

  let snippetMatch: RegExpExecArray | null;
  while ((snippetMatch = snippetRegex.exec(html)) !== null) {
    snippets.push(snippetMatch[1]!.replace(/<[^>]*>/g, '').trim());
  }

  for (let i = 0; i < links.length; i++) {
    if (links[i] && titles[i]) {
      results.push({
        title: titles[i] ?? '',
        url: links[i]!.startsWith('http') ? links[i]! : `https://${links[i]}`,
        snippet: snippets[i] ?? '',
        score: 1,
        source: 'duckduckgo',
      });
    }
  }

  return results;
}

export class DuckDuckGoSearchProvider implements SearchProvider {
  readonly name = 'duckduckgo';
  readonly kind = 'search' as const;

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return true;
  }

  async search(query: string): Promise<SearchResult[]> {
    const baseUrl = 'https://lite.duckduckgo.com/lite';
    const url = `${baseUrl}/?q=${encodeURIComponent(query)}`;
    const response = await fetch(url, {
      headers: { 'User-Agent': 'Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36' },
    });
    if (!response.ok) return [];
    const html = await response.text();
    return parseDuckDuckGoLiteHtml(html);
  }
}
