import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';
import { HfSearxngProvider } from './hf-searxng.js';
import { HfWhoogleProvider } from './hf-whoogle.js';
import { HfWebsurfxProvider } from './hf-websurfx.js';

/**
 * Configuration for self-hosted HF Spaces search engines.
 *
 * Each URL points to a Hugging Face Space running a search engine Docker container.
 * Set to empty string to disable a provider.
 */
export interface HfSearchConfig {
  /** Hugging Face Space URL for SearXNG, e.g. https://user-personal-ai-searxng.hf.space */
  searxngUrl: string;
  /** Hugging Face Space URL for Whoogle, e.g. https://user-personal-ai-whoogle.hf.space */
  whoogleUrl: string;
  /** Hugging Face Space URL for Websurfx, e.g. https://user-personal-ai-websurfx.hf.space */
  websurfxUrl: string;
}

/**
 * Build the list of available HF search providers from config.
 * Providers with empty URLs are excluded.
 */
export function buildHfProviders(config: HfSearchConfig): SearchProvider[] {
  const providers: SearchProvider[] = [];
  if (config.searxngUrl) {
    providers.push(new HfSearxngProvider(config.searxngUrl));
  }
  if (config.whoogleUrl) {
    providers.push(new HfWhoogleProvider(config.whoogleUrl));
  }
  if (config.websurfxUrl) {
    providers.push(new HfWebsurfxProvider(config.websurfxUrl));
  }
  return providers;
}

/**
 * HF Search Rotator — picks one available HF engine per query.
 *
 * **Normal mode** for the free-tier cascade:
 * - Shuffles providers to distribute load across HF Spaces
 * - Falls through to next provider on failure
 * - Returns empty if all HF engines fail
 *
 * Used in `buildCascadeProviders()` between DuckDuckGo and Parallel MCP.
 */
export class HfSearchRotator implements SearchProvider {
  name = 'HF Search';
  kind = 'search' as const;

  private readonly providers: SearchProvider[];

  constructor(config: HfSearchConfig) {
    this.providers = buildHfProviders(config);
  }

  get providerCount(): number {
    return this.providers.length;
  }

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return this.providers.length > 0;
  }

  async search(query: string): Promise<SearchResult[]> {
    if (this.providers.length === 0) {
      throw new Error('No HF search providers configured');
    }

    // Shuffle to distribute load evenly across Spaces
    const shuffled = [...this.providers].sort(() => Math.random() - 0.5);

    for (const provider of shuffled) {
      try {
        const results = await provider.search!(query);
        if (results.length > 0) {
          return results;
        }
      } catch {
        // Provider failed — try the next one
        continue;
      }
    }

    // All HF engines failed — return empty, cascade will fall through
    throw new Error('All HF search engines failed');
  }
}

/**
 * HF Deep Research — queries ALL configured HF engines in parallel,
 * merges results, and deduplicates by URL.
 *
 * **Deep Research mode**: fires SearXNG + Whoogle + Websurfx simultaneously.
 * Returns a consolidated, deduplicated result set. If one engine fails,
 * results from the others are still returned.
 *
 * Use this explicitly for "Deep Research" queries, not for normal rotation.
 */
export async function hfDeepResearch(
  config: HfSearchConfig,
  query: string,
): Promise<SearchResult[]> {
  const providers = buildHfProviders(config);
  if (providers.length === 0) {
    return [];
  }

  const trimmed = query.trim();
  if (!trimmed) return [];

  // Fire all engines in parallel with individual timeouts
  const results = await Promise.allSettled(
    providers.map((p) => p.search!(trimmed)),
  );

  // Merge all successful results
  const allResults: SearchResult[] = [];
  for (const r of results) {
    if (r.status === 'fulfilled') {
      allResults.push(...r.value);
    }
  }

  // Deduplicate by URL
  const seen = new Set<string>();
  const deduped: SearchResult[] = [];
  for (const item of allResults) {
    if (!seen.has(item.url)) {
      seen.add(item.url);
      deduped.push(item);
    }
  }

  return deduped;
}
