import type { SearchContext, SearchProvider } from '@agentcowork/core-domain';
import { McpSearchClient } from '../mcp-client.js';

export class BrightDataWebMcpProvider implements SearchProvider {
  name = 'Bright Data Web MCP';
  kind = 'fetch' as const;
  private client = new McpSearchClient(process.env.EXPO_PUBLIC_BRIGHTDATA_MCP_URL ?? '');

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return Boolean(process.env.EXPO_PUBLIC_BRIGHTDATA_MCP_URL);
  }

  async fetch(url: string): Promise<string> {
    console.log(`[BrightDataWebMcpProvider] Fetching via MCP: ${url}`);
    try {
      return await this.client.fetch(url, 'brightdata_fetch');
    } catch (e) {
      console.warn(`[BrightDataWebMcpProvider] MCP fetch failed:`, e);
      throw e;
    }
  }
}
