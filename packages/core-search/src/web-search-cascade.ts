import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';
import type { SearchCache } from './cache/search-cache.js';
import { rewriteSearchQuery } from './query-rewrite.js';
import { generateAspectQueries, mergeAndDedupe } from './fan-out.js';

/**
 * Provider-agnostic search cascade — no MCP / Node-only imports.
 *
 * P69.D9: not the desktop search path. The kernel owns search
 * (`agentcowork-search` + `search.*` tools, Guard-2-routed egress); this class
 * exists for the offline/preview projection and must never be wired into the
 * coordinator's turn loop (CI: LAYER-3 in `scripts/check-arch-invariants.mjs`).
 */
export class WebSearchCascade {
  private providers: SearchProvider[] = [];
  private cache: SearchCache | null;

  constructor(providers: SearchProvider[], cache: SearchCache | null = null) {
    this.providers = providers.filter((p) => p.kind === 'search');
    this.cache = cache;
  }

  async search(query: string, ctx: SearchContext): Promise<SearchResult[]> {
    const rewrittenQuery = rewriteSearchQuery(query);

    // If the BYOK provider supports native web search (e.g. Gemini Grounding),
    // the model handles search during generation — cascade runs to supplement.
    if (ctx.hasNativeGrounding) {
      // Still attempt the cascade for additional context; the LLM call will
      // handle provider-native grounding separately.
    }

    // Multi-query fan-out: derive aspect queries (comparison/compound splits,
    // local-intent locality hints). Never more than 3, fully deterministic.
    const fanOutOptions: Parameters<typeof generateAspectQueries>[1] = { maxAspects: 3 };
    if (ctx.location) {
      const loc: { city?: string; region?: string; country?: string } = {};
      if (ctx.location.city) loc.city = ctx.location.city;
      if (ctx.location.region) loc.region = ctx.location.region;
      if (ctx.location.country) loc.country = ctx.location.country;
      fanOutOptions.location = loc;
    }
    const aspects = generateAspectQueries(rewrittenQuery, fanOutOptions);

    // Sequential switch: try each provider in order; stop at first non-empty result.
    // Within a provider, fan the aspect queries out in PARALLEL and merge them.
    for (const provider of this.providers) {
      if (!provider.search) continue;

      try {
        const isAvailable = await provider.isAvailable(ctx);
        if (!isAvailable) continue;

        // Parallel fan-out across aspect queries against this provider.
        const batchResults = await Promise.all(
          aspects.map((aspectQuery) =>
            provider.search!(aspectQuery).catch(() => [] as SearchResult[]),
          ),
        );
        const results = mergeAndDedupe(batchResults, 12);

        if (results.length > 0) {
          if (this.cache && provider.name !== 'Search Cache') {
            this.cache.set(rewrittenQuery, results);
          }
          return results;
        }
      } catch (error) {
        console.warn(`[WebSearchCascade] Provider ${provider.name} failed:`, error);
      }
    }

    return [];
  }
}