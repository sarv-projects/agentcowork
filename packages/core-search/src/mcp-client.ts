import { Client } from '@modelcontextprotocol/sdk/client';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import type { SearchResult } from '@agentcowork/core-domain';

/** Shape of a single MCP content item as returned by callTool. */
interface McpContentItem {
  type: string;
  text?: string;
  [key: string]: unknown;
}

export class McpSearchClient {
  private client: Client | null = null;
  private transport: StreamableHTTPClientTransport | null = null;
  private isConnected = false;

  constructor(private readonly endpoint: string) {}

  async connect(): Promise<void> {
    if (this.isConnected) return;
    
    // MCP 2026 transport: prefer Streamable HTTP. The endpoint is already
    // validated by the desktop bridge (HTTPS or loopback); the SDK owns the
    // protocol/session negotiation and reconnect behavior.
    this.transport = new StreamableHTTPClientTransport(new URL(this.endpoint));
    this.client = new Client({ name: 'agentcowork-sidecar', version: '1.0.0' }, { capabilities: {} });
    
    // SDK 1.30's exactOptionalPropertyTypes declarations disagree between
    // `Client.connect` and StreamableHTTPClientTransport.sessionId. The
    // runtime implements the same Transport contract; keep the cast local to
    // this compatibility seam instead of weakening the package tsconfig.
    await this.client.connect(
      this.transport as unknown as Parameters<Client['connect']>[0],
    );
    this.isConnected = true;
  }

  private parseContent(response: { content: McpContentItem[] }): string {
    if (!Array.isArray(response.content)) return '';
    const item = response.content.find((c) => c.type === 'text');
    return item?.text ?? '';
  }

  async search(query: string, toolName: string = 'search'): Promise<SearchResult[]> {
    if (!this.isConnected || !this.client) {
      await this.connect();
    }

    try {
      const response = await this.client!.callTool({
        name: toolName,
        arguments: { query }
      });

      const textContent = this.parseContent(response as { content: McpContentItem[] });
      if (!textContent) return [];

      try {
        const parsed = JSON.parse(textContent);
        if (!Array.isArray(parsed)) return [];
        return parsed.filter((item): item is Record<string, unknown> =>
          item !== null && typeof item === 'object'
        ).map((item) => ({
          title: String(item.title ?? `Search: ${query}`),
          url: String(item.url ?? ''),
          snippet: String(item.snippet ?? item.content ?? '').substring(0, 200),
          content: String(item.content ?? item.snippet ?? ''),
          score: Number(item.score ?? 1.0),
          source: 'mcp',
        }));
      } catch {
        return [{
          title: `Search: ${query}`,
          url: '',
          snippet: textContent.substring(0, 200),
          content: textContent,
          score: 1.0,
          source: 'mcp',
        }];
      }
    } catch (error) {
      console.error(`[McpSearchClient] Tool call failed:`, error);
      throw error;
    }
  }

  async fetch(url: string, toolName: string = 'fetch'): Promise<string> {
    if (!this.isConnected || !this.client) {
      await this.connect();
    }

    const response = await this.client!.callTool({
      name: toolName,
      arguments: { url }
    });

    return this.parseContent(response as { content: McpContentItem[] });
  }

  async disconnect(): Promise<void> {
    if (this.transport) {
      await this.transport.close();
    }
    this.isConnected = false;
  }
}
