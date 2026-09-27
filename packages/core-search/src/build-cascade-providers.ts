import type { SearchContext, SearchProvider } from '@agentcowork/core-domain';
import { getSearchCache, SearchCacheProvider } from './cache/search-cache.js';
import type { SearchCache } from './cache/search-cache.js';
import { DuckDuckGoSearchProvider } from './providers/duckduckgo-search.js';
import { DdgInstantAnswerProvider } from './providers/ddg-instant.js';
import { DdgHtmlSearchProvider } from './providers/ddg-html-search.js';
import { ExaSearchProvider } from './providers/exa-search.js';
import { SearXNGPoolProvider } from './providers/searxng-pool.js';
import { HfSearchRotator, type HfSearchConfig } from './providers/hf-rotator.js';
import { TavilySearchProvider } from './providers/tavily-search.js';
import { WikipediaSearchProvider } from './providers/wikipedia-search.js';

export type CascadeProviderFactory = {
  parallel: () => SearchProvider;
};

/**
 * Build search providers for the cascade:
 * 1. Cache (always)
 * 2. If BYOK web search key → Exa only
 * 3. Else → SearXNG → DuckDuckGo → HF Rotator → Parallel (sequential switch on empty/failure)
 *
 * HF Rotator (3 self-hosted engines on Hugging Face Spaces) sits between
 * DuckDuckGo and Parallel MCP.  Config URLs passed via `hfConfig` — empty
 * URLs disable the rotator for that engine.
 */
export function buildCascadeProviders(
  ctx: SearchContext,
  factories: CascadeProviderFactory,
  hfConfig?: HfSearchConfig,
): { providers: SearchProvider[]; cache: SearchCache } {
  const cache = getSearchCache();
  const providers: SearchProvider[] = [new SearchCacheProvider(cache)];

  if (ctx.hasByokSearchKey) {
    providers.push(new ExaSearchProvider());
  } else {
    const hfRotator = hfConfig ? new HfSearchRotator(hfConfig) : null;

    const cascade: SearchProvider[] = [
      new DdgInstantAnswerProvider(),
      new DdgHtmlSearchProvider(),
      new TavilySearchProvider(),
      new SearXNGPoolProvider(),
      new DuckDuckGoSearchProvider(),
      new WikipediaSearchProvider(),
    ];

    // Insert HF rotator between DuckDuckGo and Parallel if configured
    if (hfRotator && hfRotator.providerCount > 0) {
      cascade.push(hfRotator);
    }

    cascade.push(factories.parallel());
    providers.push(...cascade);
  }

  return { providers, cache };
}