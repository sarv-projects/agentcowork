import { describe, expect, it } from 'vitest';
import type { SearchContext } from '@agentcowork/core-domain';
import { buildCascadeProviders } from '../build-cascade-providers.js';

const baseCtx: SearchContext = {
  hasNativeGrounding: false,
  hasByokSearchKey: false,
  query: 'test',
  userId: 'user-1',
};

function providerNames(ctx: SearchContext): string[] {
  const { providers } = buildCascadeProviders(ctx, {
    parallel: () => ({
      name: 'Parallel Search MCP',
      kind: 'search' as const,
      isAvailable: async () => true,
    }),
  });
  return providers.map((provider) => provider.name);
}

describe('buildCascadeProviders', () => {
  it('uses BYOK Exa only when hasByokSearchKey is true', () => {
    expect(providerNames({ ...baseCtx, hasByokSearchKey: true })).toEqual([
      'Search Cache',
      'Exa Search MCP',
    ]);
  });

  it('uses free-tier chain in order when BYOK is absent', () => {
    // Must match build-cascade-providers.ts free path (no hfConfig):
    // Cache → Instant → HTML → Tavily → SearXNG → DuckDuckGo Lite → Wikipedia → Parallel
    expect(providerNames(baseCtx)).toEqual([
      'Search Cache',
      'ddg-instant',
      'ddg-html',
      'tavily',
      'SearXNG Pool',
      'DuckDuckGo',
      'wikipedia',
      'Parallel Search MCP',
    ]);
  });
});
