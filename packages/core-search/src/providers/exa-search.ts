import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';
import { McpSearchClient } from '../mcp-client.js';

export class ExaSearchProvider implements SearchProvider {
  name = 'Exa Search MCP';
  kind = 'search' as const;
  private client: McpSearchClient | null;

  constructor() {
    const url = process.env.EXPO_PUBLIC_EXA_MCP_URL?.trim();
    this.client = url ? new McpSearchClient(url) : null;
  }

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return this.client != null;
  }

  async search(query: string): Promise<SearchResult[]> {
    if (!this.client) throw new Error('Exa MCP URL not configured');
    console.log(`[ExaSearchProvider] Searching for: ${query} via MCP`);
    try {
      const results = await this.client.search(query, 'exa_search');
      return results.map(r => ({ ...r, source: 'Exa' }));
    } catch (e) {
      console.warn(`[ExaSearchProvider] MCP search failed:`, e);
      throw e;
    }
  }
}
