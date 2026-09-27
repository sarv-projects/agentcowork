import type { SearchContext, SearchProvider } from '@agentcowork/core-domain';
import { getSearchCache, SearchCacheProvider } from './cache/search-cache.js';
import type { SearchCache } from './cache/search-cache.js';
import { DuckDuckGoSearchProvider } from './providers/duckduckgo-search.native.js';
import { DdgInstantAnswerProvider } from './providers/ddg-instant.js';
import { DdgHtmlSearchProvider } from './providers/ddg-html-search.js';
import { ExaSearchProvider } from './providers/exa-search.js';
import { ExaRestSearchProvider } from './providers/exa-rest-search.js';
import { SearXNGPoolProvider } from './providers/searxng-pool.native.js';
import { HfSearchRotator, type HfSearchConfig } from './providers/hf-rotator.js';
import { TavilySearchProvider } from './providers/tavily-search.js';
import { WikipediaSearchProvider } from './providers/wikipedia-search.js';
import { ParallelWebSearchCascade } from './parallel-web-search-cascade.js';

export type CascadeProviderFactory = {
  parallel: () => SearchProvider;
};

export function buildCascadeProviders(
  ctx: SearchContext,
  factories: CascadeProviderFactory,
  hfConfig?: HfSearchConfig,
): { providers: SearchProvider[]; cache: SearchCache; cascade: ParallelWebSearchCascade } {
  const cache = getSearchCache();
  const providers: SearchProvider[] = [new SearchCacheProvider(cache)];

  const hfRotator = hfConfig ? new HfSearchRotator(hfConfig) : null;

  const searchProviders: SearchProvider[] = [
    new ExaRestSearchProvider(),
    new DdgInstantAnswerProvider(),
    new DdgHtmlSearchProvider(),
    new WikipediaSearchProvider(),
    new TavilySearchProvider(),
    new DuckDuckGoSearchProvider(),
    new SearXNGPoolProvider(),
  ];

  if (hfRotator && hfRotator.providerCount > 0) {
    searchProviders.push(hfRotator);
  }

  if (ctx.hasByokSearchKey) {
    providers.push(new ExaSearchProvider());
  }

  searchProviders.push(factories.parallel());
  providers.push(...searchProviders);

  const cascade = new ParallelWebSearchCascade(searchProviders, cache);

  return { providers, cache, cascade };
}