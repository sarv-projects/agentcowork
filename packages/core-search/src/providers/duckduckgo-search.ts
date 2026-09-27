import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';
import * as cheerio from 'cheerio';

const LITE_ENDPOINT = 'https://lite.duckduckgo.com/lite/';
const REQUEST_TIMEOUT_MS = 12_000;
const MAX_RESULTS = 10;

const MOBILE_USER_AGENT =
  'Mozilla/5.0 (Linux; Android 14; Mobile) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Mobile Safari/537.36';

function normalizeUrl(raw: string): string {
  if (raw.startsWith('//')) {
    return `https:${raw}`;
  }
  if (raw.startsWith('/')) {
    return new URL(raw, 'https://duckduckgo.com').toString();
  }
  return raw;
}

function decodeDuckDuckGoRedirect(url: string): string {
  try {
    const parsed = new URL(normalizeUrl(url));
    if (parsed.hostname.endsWith('duckduckgo.com') && parsed.pathname === '/l/') {
      const target = parsed.searchParams.get('uddg');
      if (target) {
        return decodeURIComponent(target);
      }
    }
  } catch {
    // keep original url
  }
  return url;
}

export function parseDuckDuckGoLiteHtml(html: string, maxResults = MAX_RESULTS): SearchResult[] {
  const $ = cheerio.load(html);
  const links = $('a.result-link')
    .map((_, element) => {
      const href = $(element).attr('href')?.trim() ?? '';
      const title = $(element).text().replace(/\s+/g, ' ').trim();
      return { title, href };
    })
    .get()
    .filter((item) => item.title.length > 0 && item.href.length > 0);

  const snippets = $('td.result-snippet')
    .map((_, element) => $(element).text().replace(/\s+/g, ' ').trim())
    .get();

  const results: SearchResult[] = [];
  for (let index = 0; index < links.length && results.length < maxResults; index += 1) {
    const link = links[index]!;
    const url = decodeDuckDuckGoRedirect(link.href);
    if (!url.startsWith('http://') && !url.startsWith('https://')) {
      continue;
    }
    results.push({
      title: link.title,
      url,
      snippet: snippets[index] ?? '',
      score: Math.max(0.1, 1 - index * 0.05),
      source: 'DuckDuckGo',
    });
  }

  return results;
}

/** Free-tier web search via DuckDuckGo Lite HTML (no API key). */
export class DuckDuckGoSearchProvider implements SearchProvider {
  name = 'DuckDuckGo';
  kind = 'search' as const;

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return true;
  }

  async search(query: string): Promise<SearchResult[]> {
    const trimmed = query.trim();
    if (!trimmed) {
      return [];
    }

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);

    try {
      const response = await fetch(LITE_ENDPOINT, {
        method: 'POST',
        signal: controller.signal,
        headers: {
          'Content-Type': 'application/x-www-form-urlencoded',
          Accept: 'text/html',
          'User-Agent': MOBILE_USER_AGENT,
        },
        body: new URLSearchParams({ q: trimmed }).toString(),
      });

      if (!response.ok) {
        throw new Error(`DuckDuckGo failed: HTTP ${response.status}`);
      }

      const html = await response.text();
      const results = parseDuckDuckGoLiteHtml(html);
      if (results.length === 0) {
        throw new Error('DuckDuckGo returned no parseable results');
      }

      return results;
    } catch (error) {
      if (error instanceof Error && error.name === 'AbortError') {
        throw new Error('DuckDuckGo request timed out');
      }
      throw error;
    } finally {
      clearTimeout(timer);
    }
  }
}