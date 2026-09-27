import type { SearchContext, SearchProvider } from '@agentcowork/core-domain';
import * as cheerio from 'cheerio';

export class ReadabilityFetcherProvider implements SearchProvider {
  name = 'On-Device Readability Fetcher';
  kind = 'fetch' as const;

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    // Always available (on-device fallback)
    return true;
  }

  async fetch(url: string): Promise<string> {
    console.log(`[ReadabilityFetcherProvider] Fetching: ${url}`);

    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 15_000);

    let html: string;
    try {
      const response = await fetch(url, {
        headers: {
          'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Safari/537.36'
        },
        signal: controller.signal,
      });

      if (!response.ok) {
        throw new Error(`HTTP ${response.status} when fetching ${url}`);
      }

      html = await response.text();
    } finally {
      clearTimeout(timeout);
    }
    const $ = cheerio.load(html);

    // Basic Readability/Trafilatura heuristic equivalent using cheerio
    $('script, style, noscript, iframe, nav, footer, header, aside').remove();
    
    // Attempt to target main content
    let content = $('main').text();
    if (!content.trim()) {
      content = $('article').text();
    }
    if (!content.trim()) {
      content = $('body').text();
    }

    // Clean up whitespace
    return content.replace(/\s+/g, ' ').trim();
  }
}
