import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';
import { McpSearchClient } from '../mcp-client.js';

export class ParallelSearchProvider implements SearchProvider {
  name = 'Parallel Search MCP';
  kind = 'search' as const;
  private client: McpSearchClient | null;

  constructor() {
    const url = process.env.EXPO_PUBLIC_PARALLEL_MCP_URL?.trim();
    this.client = url ? new McpSearchClient(url) : null;
  }

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return this.client != null;
  }

  async search(query: string): Promise<SearchResult[]> {
    if (!this.client) return [];
    try {
      const results = await this.client.search(query, 'parallel_search');
      return results.map(r => ({ ...r, source: 'Parallel' }));
    } catch (e) {
      console.warn(`[ParallelSearchProvider] MCP search failed:`, e);
      return [];
    }
  }
}
