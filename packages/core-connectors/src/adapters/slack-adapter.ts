/**
 * Slack connector — Slack Web API OAuth 2.0.
 *
 * Free tier: Tier 3 scope — 50+ requests/min for `conversations.history`
 * and `search.messages` (https://api.slack.com/docs/rate-limits).
 * Powers "summarise my unread DMs" use cases.
 *
 * Auth: OAuth 2.0 (server-side flow through OAUTH_PROVIDERS['slack']).
 * Scope: `channels:history,groups:history,im:history,mpim:history,
 *         channels:read,groups:read,im:read,mpim:read,users:read,search:read`
 * — read-only, no write or admin privileges.
 *
 * The Rust host owns the OAuth exchange, vault credential, refresh, and
 * authenticated request. This adapter handles only query intent and response
 * normalization.
 */
import type {
  ConnectorAdapter,
  ConnectorContext,
  ConnectorFilter,
  ConnectorResult,
  MemoryFact,
  UserQuery,
} from '@agentcowork/core-domain';
import { requestConnector } from '../connection-manager.js';

const SLACK_API = 'https://slack.com/api';
const CONNECTOR_NAME = 'slack' as const;

export class SlackAdapter implements ConnectorAdapter {
  readonly name = CONNECTOR_NAME;
  readonly credentialMode = 'host-mediated' as const;
  readonly metadataSchema = {
    fields: [
      { name: 'query', type: 'string' as const, description: 'Free-text search (slack /search) or channel filter (#channel)' },
      { name: 'limit', type: 'number' as const, description: 'Maximum messages to return (default 20)' },
    ],
  };

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    if (/\b(unread|inbox|dm|@[a-z])\b/.test(q)) return 0.8;
    if (/(#|slack)/.test(q)) return 0.75;
    if (/\b(channel|message|conversation|team chat)\b/.test(q)) return 0.55;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text || '', limit: 20 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = ctx.filter as { query?: string; limit?: number };
    const q = (f.query || '').trim();
    const limit = Math.min(f.limit ?? 20, 50);
    const headers = { 'Content-Type': 'application/json; charset=utf-8' };
    const hostRequest = async (url: string) =>
      requestConnector({
        connector: CONNECTOR_NAME,
        userId: ctx.userId,
        request: { url, method: 'GET', headers },
        ...(ctx.signal ? { signal: ctx.signal } : {}),
      });

    try {
      // Detect a #channel filter and route to conversations.history; otherwise
      // run a /search.messages query.
      const channelMatch = q.match(/#([a-z0-9_-]{1,80})/i);

      if (channelMatch) {
        // Resolve channel name → ID via conversations.list
        const listRes = await hostRequest(
          `${SLACK_API}/conversations.list?types=public_channel,private_channel,im,mpim&limit=200`,
        );
        if (!listRes?.ok) return { items: [], totalCount: 0, source: CONNECTOR_NAME };
        const listData = (await listRes.json()) as {
          ok: boolean;
          channels?: Array<{ id: string; name?: string }>;
          error?: string;
        };
        if (!listData.ok) {
          return {
            items: [
              {
                id: `slack:err`,
                title: 'Slack API error',
                snippet: listData.error ?? 'conversations.list failed',
              },
            ],
            totalCount: 1,
            source: CONNECTOR_NAME,
          };
        }
        const ch = (listData.channels ?? []).find((c) => c.name === channelMatch[1]);
        if (!ch) {
          return {
            items: [
              {
                id: `unknown:${channelMatch[1]}`,
                title: `Channel #${channelMatch[1]}`,
                snippet: 'Channel not visible to this credential (private or not joined)',
              },
            ],
            totalCount: 1,
            source: CONNECTOR_NAME,
          };
        }
        const histRes = await hostRequest(
          `${SLACK_API}/conversations.history?channel=${encodeURIComponent(ch.id)}&limit=${limit}`,
        );
        if (!histRes?.ok) return { items: [], totalCount: 0, source: CONNECTOR_NAME };
        const histData = (await histRes.json()) as {
          ok: boolean;
          messages?: Array<{ ts: string; text?: string; user?: string; thread_ts?: string }>;
        };
        if (!histData.ok) return { items: [], totalCount: 0, source: CONNECTOR_NAME };
        const items: ConnectorResult['items'] = (histData.messages ?? []).map((m) => ({
          id: `slack:${m.ts}`,
          title: `${channelMatch[1]} — ${new Date(Number(m.ts) * 1000).toLocaleString()}`,
          snippet: (m.text ?? '').slice(0, 280),
          date: new Date(Number(m.ts) * 1000).toISOString(),
          metadata: { channel: ch.id, user: m.user, thread_ts: m.thread_ts },
        }));
        return { items, totalCount: items.length, source: CONNECTOR_NAME };
      }

      // Default: search messages across the workspace. Slack requires
      // a non-empty query for search.messages.
      if (!q) {
        // Empty query → fetch unread inbox via conversations.list + history 0
        // Sort unread by recent activity and pull last-limit messages across
        // the first 5 DMs/channels to keep under rate limits.
        const listRes = await hostRequest(
          `${SLACK_API}/conversations.list?types=public_channel,private_channel,im,mpim&limit=10`,
        );
        if (!listRes?.ok) return { items: [], totalCount: 0, source: CONNECTOR_NAME };
        const listData = (await listRes.json()) as {
          ok: boolean;
          channels?: Array<{ id: string; name?: string }>;
        };
        if (!listData.ok) return { items: [], totalCount: 0, source: CONNECTOR_NAME };
        const items: ConnectorResult['items'] = [];
        for (const ch of listData.channels ?? []) {
          const histRes = await hostRequest(
            `${SLACK_API}/conversations.history?channel=${encodeURIComponent(ch.id)}&limit=3`,
          );
          if (!histRes?.ok) return { items: [], totalCount: 0, source: CONNECTOR_NAME };
          const histData = (await histRes.json()) as { ok: boolean; messages?: Array<{ ts: string; text?: string }> };
          if (!histData.ok) continue;
          for (const m of histData.messages ?? []) {
            items.push({
              id: `slack:${m.ts}:${ch.id}`,
              title: `${ch.name ?? ch.id} — ${new Date(Number(m.ts) * 1000).toLocaleString()}`,
              snippet: (m.text ?? '').slice(0, 280),
              date: new Date(Number(m.ts) * 1000).toISOString(),
              metadata: { channel: ch.id },
            });
            if (items.length >= limit) break;
          }
          if (items.length >= limit) break;
        }
        return { items, totalCount: items.length, source: CONNECTOR_NAME };
      }

      // Search by free-text query.
      const searchRes = await hostRequest(
        `${SLACK_API}/search.messages?query=${encodeURIComponent(q)}&count=${limit}`,
      );
      if (!searchRes?.ok) return { items: [], totalCount: 0, source: CONNECTOR_NAME };
      const searchData = (await searchRes.json()) as {
        ok: boolean;
        messages?: { matches?: Array<{ ts: string; text?: string; channel?: { name?: string; id?: string }; user?: string }> };
        error?: string;
      };
      if (!searchData.ok) {
        return {
          items: [
            {
              id: `slack:err`,
              title: 'Slack search error',
              snippet: searchData.error ?? 'search.messages failed',
            },
          ],
          totalCount: 1,
          source: CONNECTOR_NAME,
        };
      }
      const items: ConnectorResult['items'] = (searchData.messages?.matches ?? []).map((m) => ({
        id: `slack:${m.ts}`,
        title: `${m.channel?.name ?? m.channel?.id ?? 'dm'} — ${new Date(Number(m.ts) * 1000).toLocaleString()}`,
        snippet: (m.text ?? '').slice(0, 280),
        date: new Date(Number(m.ts) * 1000).toISOString(),
        metadata: { channel: m.channel?.id, user: m.user },
      }));
      return { items, totalCount: items.length, source: CONNECTOR_NAME };
    } catch {
      return { items: [], totalCount: 0, source: CONNECTOR_NAME };
    }
  }

}
