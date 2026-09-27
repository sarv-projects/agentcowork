import type {
  ConnectorAdapter,
  ConnectorContext,
  ConnectorFilter,
  ConnectorMetadataSchema,
  ConnectorName,
  ConnectorResult,
  MemoryFact,
  UserQuery,
} from '@agentcowork/core-domain';
import { requestConnector } from '../connection-manager.js';

/**
 * Telegram Bot connector — the bot credential is vault-owned by the Rust host.
 * The `{credential}` path placeholder is expanded only inside the host.
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'chatId', type: 'string', description: 'Target chat id (user or group)' },
  ],
};

export class TelegramAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'telegram';
  readonly credentialMode = 'host-mediated' as const;
  readonly metadataSchema = metadataSchema;

  private readonly defaultChatId: string | undefined;

  constructor(defaultChatId?: string) {
    this.defaultChatId = defaultChatId;
  }

  async isAuthorized(_userId: string): Promise<boolean> {
    return true; // Credential resolution is host-owned
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = query.text.toLowerCase();
    return /telegram|bot|message|notify/.test(q) ? 0.7 : 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as Record<string, unknown>;
    const chatId = (typeof f.chatId === 'string' ? f.chatId : this.defaultChatId) ?? '';

    try {
      const meRes = await requestConnector({
        connector: this.name,
        userId: ctx.userId,
        request: { url: 'https://api.telegram.org/bot{credential}/getMe', method: 'GET' },
        ...(ctx.signal ? { signal: ctx.signal } : {}),
      });
      if (!meRes?.ok) return { items: [], totalCount: 0, source: this.name };
      const me = (await meRes.json()) as { result?: { username?: string } };

      const items: ConnectorResult['items'] = [
        {
          id: 'bot-info',
          title: `Telegram bot: ${me.result?.username || 'connected'}`,
          snippet: 'Bot is reachable',
          metadata: { bot: true },
        },
      ];

      if (chatId) {
        items.push({
          id: 'chat-linked',
          title: `Linked chat ${chatId}`,
          snippet: 'Ready to send notifications or read via getUpdates in future',
        });
      }

      return { items, totalCount: items.length, source: this.name };
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }
}
