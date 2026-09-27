/**
 * DuckDuckGo HTML Search — free, no API key.
 * Scrapes html.duckduckgo.com (non-JS version) for web search results.
 * More reliable than lite version — returns full result links with snippets.
 */
import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';

function decodeDdgUrl(encodedUrl: string): string {
  const match = encodedUrl.match(/uddg=([^&]+)/);
  if (match) {
    return decodeURIComponent(match[1]!);
  }
  if (encodedUrl.startsWith('//')) {
    return `https:${encodedUrl}`;
  }
  return encodedUrl;
}

function stripHtml(html: string): string {
  return html.replace(/<[^>]*>/g, '').replace(/&[a-z]+;/gi, (m) => {
    const map: Record<string, string> = {
      '&amp;': '&', '&lt;': '<', '&gt;': '>',
      '&quot;': '"', '&#x27;': "'", '&apos;': "'",
    };
    return map[m] || m;
  }).trim();
}

export class DdgHtmlSearchProvider implements SearchProvider {
  readonly name = 'ddg-html';
  readonly kind = 'search' as const;

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return true;
  }

  async search(query: string): Promise<SearchResult[]> {
    const url = 'https://html.duckduckgo.com/html/';
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 12_000);

    try {
      const resp = await fetch(url, {
        method: 'POST',
        headers: {
          'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
          'Content-Type': 'application/x-www-form-urlencoded',
        },
        body: `q=${encodeURIComponent(query)}`,
        signal: controller.signal,
      });
      if (!resp.ok) return [];
      const html = await resp.text();

      const results: SearchResult[] = [];
      const linkRegex = /<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>([\s\S]*?)<\/a>/gi;
      const snipRegex = /<a[^>]*class="result__snippet"[^>]*>([\s\S]*?)<\/a>/gi;

      const rawLinks: Array<{ href: string; title: string }> = [];
      let m: RegExpExecArray | null;
      while ((m = linkRegex.exec(html)) !== null) {
        rawLinks.push({ href: m[1]!, title: stripHtml(m[2]!) });
      }

      const snippets: string[] = [];
      while ((m = snipRegex.exec(html)) !== null) {
        snippets.push(stripHtml(m[1]!));
      }

      for (let i = 0; i < rawLinks.length; i++) {
        const link = rawLinks[i]!;
        const decoded = decodeDdgUrl(link.href);
        if (!decoded || decoded.includes('duckduckgo.com')) continue;
        results.push({
          title: link.title || decoded,
          url: decoded,
          snippet: snippets[i] ?? '',
          score: Math.max(1, 10 - i),
          source: 'ddg-html',
        });
      }

      return results;
    } catch {
      return [];
    } finally {
      clearTimeout(timeout);
    }
  }
}
