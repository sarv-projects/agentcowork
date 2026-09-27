import type { SearchContext, SearchProvider } from '@agentcowork/core-domain';

/** Mobile v1: URL fetch via on-device Readability first; MCP deferred. */
export class BrightDataWebMcpProvider implements SearchProvider {
  name = 'Bright Data Web MCP';
  kind = 'fetch' as const;

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return false;
  }

  async fetch(_url: string): Promise<string> {
    throw new Error('Bright Data MCP is not available on mobile in v1');
  }
}