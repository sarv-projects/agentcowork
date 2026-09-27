/**
 * YouTube connector — YouTube Data API v3, API key only (no OAuth needed).
 *
 * Free: 10,000 quota units/day.
 * - Search: 100 units/request
 * - Channel info: 1 unit/request
 * - Video details: 1 unit/request
 * = ~100 searches + 9000 detail lookups per day free.
 *
 * No OAuth required for public data (search, channel info, video metadata).
 * OAuth needed only for user-specific actions (likes, playlists, subscriptions).
 *
 * Flow:
 *   1. User connects YouTube through the host
 *   2. The API key remains vault-owned
 *   3. fetch() sends only query intent through the Rust host
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

const YT_API = 'https://www.googleapis.com/youtube/v3';
const CONNECTOR_NAME = 'youtube' as const;

export class YouTubeAdapter implements ConnectorAdapter {
  readonly name = CONNECTOR_NAME;
  readonly credentialMode = 'host-mediated' as const;
  readonly metadataSchema = {
    fields: [
      { name: 'query', type: 'string' as const, description: 'Search query' },
      { name: 'maxResults', type: 'number' as const, description: 'Max results (1-50)' },
    ],
  };

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['youtube', 'video', 'watch', 'channel', 'subscribe', 'tutorial', 'how to'];
    return terms.some((t) => q.includes(t)) ? 0.75 : 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text || '', maxResults: 10 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const filter = ctx.filter as { query?: string; maxResults?: number };
    const searchQuery = filter.query || '';
    const maxResults = Math.min(filter.maxResults ?? 10, 50);

    if (!searchQuery) {
      return { items: [], totalCount: 0, source: CONNECTOR_NAME };
    }

    try {
      const url = new URL(`${YT_API}/search`);
      url.searchParams.set('part', 'snippet');
      url.searchParams.set('q', searchQuery);
      url.searchParams.set('maxResults', String(maxResults));
      url.searchParams.set('type', 'video');

      const res = await requestConnector({
        connector: CONNECTOR_NAME,
        userId: ctx.userId,
        request: { url: url.toString(), method: 'GET' },
        ...(ctx.signal ? { signal: ctx.signal } : {}),
      });
      if (!res?.ok) {
        // Quota exceeded or error
        return { items: [], totalCount: 0, source: CONNECTOR_NAME };
      }

      const data = (await res.json()) as {
        items?: Array<{
          id: { videoId: string };
          snippet: {
            title: string;
            description: string;
            channelTitle: string;
            publishedAt: string;
            thumbnails?: { default?: { url: string } };
          };
        }>;
      };

      const items: ConnectorResult['items'] = (data.items ?? []).map((item) => ({
        id: item.id.videoId,
        title: item.snippet.title,
        snippet: `${item.snippet.channelTitle}: ${item.snippet.description.slice(0, 200)}`,
        url: `https://www.youtube.com/watch?v=${item.id.videoId}`,
        date: item.snippet.publishedAt,
        metadata: {
          channel: item.snippet.channelTitle,
          videoId: item.id.videoId,
          thumbnail: item.snippet.thumbnails?.default?.url,
        },
      }));

      return { items, totalCount: items.length, source: CONNECTOR_NAME };
    } catch {
      return { items: [], totalCount: 0, source: CONNECTOR_NAME };
    }
  }

}
