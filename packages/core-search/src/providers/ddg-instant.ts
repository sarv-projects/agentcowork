/**
 * DuckDuckGo Instant Answer API — free, no API key.
 * Returns abstracts, instant answers, and related topics.
 * Quality is near-Perplexity for knowledge questions.
 * https://api.duckduckgo.com/?q=query&format=json
 */
import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';

type DdgInstantResponse = {
  Abstract?: string;
  AbstractText?: string;
  AbstractURL?: string;
  AbstractSource?: string;
  Heading?: string;
  Answer?: string;
  AnswerType?: string;
  RelatedTopics?: Array<{ Text?: string; FirstURL?: string; Icon?: unknown }>;
  Results?: Array<{ Text?: string; FirstURL?: string }>;
};

export class DdgInstantAnswerProvider implements SearchProvider {
  readonly name = 'ddg-instant';
  readonly kind = 'search' as const;

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return true;
  }

  async search(query: string): Promise<SearchResult[]> {
    const url = `https://api.duckduckgo.com/?q=${encodeURIComponent(query)}&format=json&no_html=1&skip_disambig=1`;
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 10_000);

    try {
      const resp = await fetch(url, { signal: controller.signal });
      if (!resp.ok) return [];
      const data = (await resp.json()) as DdgInstantResponse;
      const results: SearchResult[] = [];

      const abstractText = data.AbstractText || data.Abstract || '';
      if (abstractText && data.AbstractURL) {
        results.push({
          title: data.Heading || 'Answer',
          url: data.AbstractURL,
          snippet: abstractText,
          score: 100,
          source: 'ddg-instant',
        });
      }

      for (const topic of data.RelatedTopics ?? []) {
        if (!topic.Text) continue;
        results.push({
          title: topic.Text.split(' - ')[0]?.trim() || topic.Text.slice(0, 80),
          url: topic.FirstURL || '',
          snippet: topic.Text,
          score: 60,
          source: 'ddg-instant',
        });
      }

      for (const r of data.Results ?? []) {
        results.push({
          title: r.Text?.split(' - ')[0]?.trim() || r.Text?.slice(0, 80) || '',
          url: r.FirstURL || '',
          snippet: r.Text || '',
          score: 40,
          source: 'ddg-instant',
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
