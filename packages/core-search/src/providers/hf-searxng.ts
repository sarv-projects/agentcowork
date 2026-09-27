import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';

const REQUEST_TIMEOUT_MS = 8_000;
const MAX_RESULTS = 15;

function normalizeResult(raw: Record<string, unknown>, source: string): SearchResult {
  const url = String(raw.url ?? raw.link ?? '');
  return {
    title: String(raw.title ?? ''),
    url: url.startsWith('//') ? `https:${url}` : url,
    snippet: String(raw.content ?? raw.snippet ?? raw.description ?? ''),
    score: 0.8,
    source,
  };
}

/**
 * Self-hosted SearXNG engine running on Hugging Face Spaces.
 *
 * Endpoint: `https://<user>-personal-ai-searxng.hf.space/search?q=...&format=json`
 * Cost: $0 (HF Spaces free tier + Cloudflare keep-alive)
 * Requires: none — no API key, no auth
 */
export class HfSearxngProvider implements SearchProvider {
  name = 'SearXNG (HF)';
  kind = 'search' as const;

  constructor(private readonly baseUrl: string) {
    if (!baseUrl) {
      throw new Error('HfSearxngProvider requires a baseUrl');
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
      const url = `${this.baseUrl}/search?q=${encodeURIComponent(trimmed)}&format=json&categories=general`;
      const response = await fetch(url, {
        signal: controller.signal,
        headers: { Accept: 'application/json' },
      });

      if (!response.ok) {
        throw new Error(`SearXNG returned HTTP ${response.status}`);
      }

      const data = (await response.json()) as { results?: Array<Record<string, unknown>> };
      const results: SearchResult[] = (data.results ?? [])
        .slice(0, MAX_RESULTS)
        .map((r: Record<string, unknown>) => normalizeResult(r, 'SearXNG'));

      if (results.length === 0) {
        throw new Error('SearXNG returned empty results');
      }

      return results;
    } catch (error) {
      if (error instanceof Error && error.name === 'AbortError') {
        throw new Error('SearXNG (HF) request timed out');
      }
      throw error;
    } finally {
      clearTimeout(timer);
    }
  }
}
