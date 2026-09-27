import type { SearchContext, SearchProvider } from '@agentcowork/core-domain';

/**
 * Strip HTML tags and extract text. Lightweight alternative to cheerio for mobile.
 */
function extractText(html: string, selector: 'body' | 'article' | 'main'): string {
  let section = html;

  // Regex patterns per selector
  const mainMatch = html.match(/<main[^>]*>([\s\S]*?)<\/main>/i);
  const articleMatch = html.match(/<article[^>]*>([\s\S]*?)<\/article>/i);
  const bodyMatch = html.match(/<body[^>]*>([\s\S]*?)<\/body>/i);

  if (selector === 'main' && mainMatch?.[1]) section = mainMatch[1];
  else if (selector === 'article' && articleMatch?.[1]) section = articleMatch[1];
  else if (selector === 'body' && bodyMatch?.[1]) section = bodyMatch[1];

  // Remove tags that shouldn't contribute to text
  section = section
    .replace(/<script[\s\S]*?<\/script>/gi, '')
    .replace(/<style[\s\S]*?<\/style>/gi, '')
    .replace(/<noscript[\s\S]*?<\/noscript>/gi, '')
    .replace(/<iframe[\s\S]*?<\/iframe>/gi, '')
    .replace(/<nav[\s\S]*?<\/nav>/gi, '')
    .replace(/<footer[\s\S]*?<\/footer>/gi, '')
    .replace(/<header[\s\S]*?<\/header>/gi, '')
    .replace(/<aside[\s\S]*?<\/aside>/gi, '')
    .replace(/<[^>]+>/g, ' ')
    .replace(/&[^;]+;/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();

  return section;
}

export class ReadabilityFetcherProvider implements SearchProvider {
  readonly name = 'On-Device Readability Fetcher';
  readonly kind = 'fetch' as const;

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return true;
  }

  async fetch(url: string): Promise<string> {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 15_000);

    let html: string;
    try {
      const response = await fetch(url, {
        headers: {
          'User-Agent':
            'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36',
        },
        signal: controller.signal,
      });

      if (!response.ok) {
        throw new Error(`HTTP ${response.status} when fetching ${url}`);
      }

      html = await response.text();
    } catch (e) {
      clearTimeout(timeout);
      throw new Error(`Fetch failed for ${url}: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      clearTimeout(timeout);
    }

    // Try main → article → body, same as cheerio version
    return (
      extractText(html, 'main') ||
      extractText(html, 'article') ||
      extractText(html, 'body')
    );
  }
}
