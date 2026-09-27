import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';

const REQUEST_TIMEOUT_MS = 8_000;
const MAX_RESULTS = 15;

function normalizeResult(raw: Record<string, unknown>, source: string): SearchResult {
  const url = String(raw.url ?? raw.link ?? '');
  return {
    title: String(raw.title ?? ''),
    url: url.startsWith('//') ? `https:${url}` : url,
    snippet: String(raw.desc ?? raw.snippet ?? raw.description ?? ''),
    score: 0.8,
    source,
  };
}

/**
 * Self-hosted Whoogle engine running on Hugging Face Spaces.
 *
 * Endpoint: `https://<user>-personal-ai-whoogle.hf.space/search?q=...&format=json`
 * Returns: ad-free, tracking-free Google results
 * Cost: $0 (HF Spaces free tier + Cloudflare keep-alive)
 */
export class HfWhoogleProvider implements SearchProvider {
  name = 'Whoogle (HF)';
  kind = 'search' as const;

  constructor(private readonly baseUrl: string) {
    if (!baseUrl) {
      throw new Error('HfWhoogleProvider requires a baseUrl');
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
        throw new Error(`Whoogle returned HTTP ${response.status}`);
      }

      const data = (await response.json()) as Array<Record<string, unknown>> | { results?: Array<Record<string, unknown>> };
      const results: SearchResult[] = (Array.isArray(data) ? data : (data.results ?? []))
        .slice(0, MAX_RESULTS)
        .map((r: Record<string, unknown>) => normalizeResult(r, 'Whoogle'));

      if (results.length === 0) {
        throw new Error('Whoogle returned empty results');
      }

      return results;
    } catch (error) {
      if (error instanceof Error && error.name === 'AbortError') {
        throw new Error('Whoogle (HF) request timed out');
      }
      throw error;
    } finally {
      clearTimeout(timer);
    }
  }
}
