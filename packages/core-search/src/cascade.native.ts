import type { SearchContext } from '@agentcowork/core-domain';
import { buildCascadeProviders } from './build-cascade-providers.native.js';
import { ParallelSearchProvider } from './providers/parallel-search.native.js';
import { ParallelWebSearchCascade } from './parallel-web-search-cascade.js';

export { rewriteSearchQuery } from './query-rewrite.js';
export { buildCascadeProviders } from './build-cascade-providers.native.js';

export function buildDefaultCascade(ctx: SearchContext): ParallelWebSearchCascade {
  const { cascade } = buildCascadeProviders(ctx, {
    parallel: () => new ParallelSearchProvider(),
  });
  return cascade;
}
