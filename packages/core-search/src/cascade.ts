/**
 * TypeScript search cascade — **not** the desktop implementation (P69.D9).
 *
 * In the shipped app, search is kernel-owned: `agentcowork-search` (Rust) plus
 * the `search.*` tools in the canonical ToolRegistry execute every query, and
 * egress goes through Guard-2 `netfloor`. This cascade is retained as a
 * projection/offline-preview path only; wiring it back into the turn loop
 * would recreate the second implementation D9 removed. CI enforces that with
 * the LAYER-3 check in `scripts/check-arch-invariants.mjs`.
 */
import type { SearchContext } from '@agentcowork/core-domain';
import { buildCascadeProviders } from './build-cascade-providers.js';
import { ParallelSearchProvider } from './providers/parallel-search.js';
import { WebSearchCascade } from './web-search-cascade.js';

export { WebSearchCascade } from './web-search-cascade.js';
export { rewriteSearchQuery } from './query-rewrite.js';
export { buildCascadeProviders } from './build-cascade-providers.js';

export function buildDefaultCascade(ctx: SearchContext): WebSearchCascade {
  const { providers, cache } = buildCascadeProviders(ctx, {
    parallel: () => new ParallelSearchProvider(),
  });
  return new WebSearchCascade(providers, cache);
}