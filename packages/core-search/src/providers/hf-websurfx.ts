import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';

const REQUEST_TIMEOUT_MS = 6_000;
const MAX_RESULTS = 15;

/**
 * Self-hosted Websurfx engine running on Hugging Face Spaces.
 *
 * Websurfx is a Rust-based meta-search engine — fastest response,
 * lowest RAM, aggregates DuckDuckGo + Brave + Mojeek.
 *
 * Endpoint: `https://<user>-personal-ai-websurfx.hf.space/search?q=...&format=json`
 * Cost: $0 (HF Spaces free tier + Cloudflare keep-alive)
 */
export class HfWebsurfxProvider implements SearchProvider {
  name = 'Websurfx (HF)';
  kind = 'search' as const;

  constructor(private readonly baseUrl: string) {
    if (!baseUrl) {
      throw new Error('HfWebsurfxProvider requires a baseUrl');
    }
  }

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return true;
  }

  async search(query: string): Promise<SearchResult[]> {
    const trimmed = query.trim();
    if (!trimmed) return [];

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);

    try {
      const url = `${this.baseUrl}/search?q=${encodeURIComponent(trimmed)}&format=json`;
      const response = await fetch(url, {
        signal: controller.signal,
        headers: { Accept: 'application/json' },
      });

      if (!response.ok) {
        throw new Error(`Websurfx returned HTTP ${response.status}`);
      }

      const data = (await response.json()) as { results?: Array<Record<string, unknown>> };
      const rawResults: Array<Record<string, unknown>> = data.results ?? [];

      const results: SearchResult[] = rawResults
        .slice(0, MAX_RESULTS)
        .map((r) => {
          const url = String(r.url ?? r.link ?? '');
          return {
            title: String(r.title ?? ''),
            url: url.startsWith('//') ? `https:${url}` : url,
            snippet: String(r.description ?? r.snippet ?? r.content ?? ''),
            score: 0.7,
            source: 'Websurfx',
          };
        });

      if (results.length === 0) {
        throw new Error('Websurfx returned empty results');
      }

      return results;
    } catch (error) {
      if (error instanceof Error && error.name === 'AbortError') {
        throw new Error('Websurfx (HF) request timed out');
      }
      throw error;
    } finally {
      clearTimeout(timer);
    }
  }
}
