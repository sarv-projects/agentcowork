/**
 * SoundCloud connector — SoundCloud Web API OAuth 2.0.
 *
 * Free tier: 15,000 requests / day per OAuth client (https://developers.soundcloud.com/docs/api/rate-limits).
 * Mobile-useful for "what's this song" / "tracks by [artist]" queries.
 *
 * Auth: OAuth 2.0 via OAUTH_PROVIDERS['soundcloud']. Scope:
 *   `non-expiring` — read-only public data; no user-stream or upload access.
 *
 * The SoundCloud /tracks endpoint returns permalink_url and stream_url —
 * we deliberately OMIT `stream_url` from the snippet because streaming
 * full tracks under our client_id may violate the SoundCloud API ToS for
 * non-paying apps. We surface title + artist + permalink instead; the AI
 * answers with explanations and linkable URLs.
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

const SOUNDCLOUD_API = 'https://api.soundcloud.com';
const CONNECTOR_NAME = 'soundcloud' as const;

export class SoundcloudAdapter implements ConnectorAdapter {
  readonly name = CONNECTOR_NAME;
  readonly credentialMode = 'host-mediated' as const;
  readonly metadataSchema = {
    fields: [
      { name: 'query', type: 'string' as const, description: 'Track or artist search keyword' },
      { name: 'limit', type: 'number' as const, description: 'Max tracks to return' },
    ],
  };

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    if (/\b(song|track|album|artist|music|playlist|listen|play)\b/.test(q)) return 0.75;
    if (/\b(soundcloud|remix|beat|hip.?hop|lo.?fi|edm)\b/.test(q)) return 0.8;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text || '', limit: 10 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = ctx.filter as { query?: string; limit?: number };
    const q = (f.query || '').trim();
    const limit = Math.min(f.limit ?? 10, 30);
    if (!q) {
      // No query → return empty so callers can ask the AI "list my recent
      // tracks" via a follow-up. SoundCloud returns 400 on empty /tracks.
      return { items: [], totalCount: 0, source: CONNECTOR_NAME };
    }

    const params = new URLSearchParams({ q, limit: String(limit) });
    try {
      const res = await requestConnector({
        connector: CONNECTOR_NAME,
        userId: ctx.userId,
        request: {
          url: `${SOUNDCLOUD_API}/tracks?${params.toString()}`,
          method: 'GET',
          headers: { Accept: 'application/json' },
        },
        ...(ctx.signal ? { signal: ctx.signal } : {}),
      });
      if (!res) return { items: [], totalCount: 0, source: CONNECTOR_NAME };
      if (!res.ok) {
        return {
          items: [
            {
              id: `sc:err:${res.status}`,
              title: 'SoundCloud lookup failed',
              snippet: `HTTP ${res.status}. Re-authorize the connector if 401.`,
              metadata: { status: res.status },
            },
          ],
          totalCount: 1,
          source: CONNECTOR_NAME,
        };
      }
      const data = (await res.json()) as Array<{
        id: number;
        title: string;
        duration?: number;
        playback_count?: number;
        user?: { username?: string; permalink_url?: string };
        permalink_url?: string;
        genre?: string;
        release?: string;
        description?: string;
      }>;

      const items: ConnectorResult['items'] = data.slice(0, limit).map((track) => {
        const artist = track.user?.username ?? 'Unknown artist';
        const duration = track.duration ? formatDuration(track.duration) : '';
        const plays = track.playback_count != null ? `${track.playback_count.toLocaleString()} plays` : '';
        const genre = track.genre ? ` · ${track.genre}` : '';
        const item: ConnectorResult['items'][number] = {
          id: `sc:${track.id}`,
          title: track.title,
          snippet: `${artist} — ${[duration, plays].filter(Boolean).join(' · ')}${genre}`,
          metadata: {
            artist,
            artistUrl: track.user?.permalink_url,
            duration: track.duration,
            plays: track.playback_count,
            genre: track.genre,
            description: track.description?.slice(0, 400),
          },
        };
        if (track.permalink_url) item.url = track.permalink_url;
        if (track.release) item.date = `${track.release}T00:00:00Z`;
        return item;
      });

      return { items, totalCount: items.length, source: CONNECTOR_NAME };
    } catch {
      return { items: [], totalCount: 0, source: CONNECTOR_NAME };
    }
  }

}

function formatDuration(ms: number): string {
  const totalSec = Math.floor(ms / 1000);
  const m = Math.floor(totalSec / 60);
  const s = String(totalSec % 60).padStart(2, '0');
  return `${m}:${s}`;
}
