import type {
  ConnectorAdapter,
  ConnectorContext,
  ConnectorFilter,
  ConnectorMetadataSchema,
  ConnectorName,
  ConnectorResult,
  UserQuery,
  MemoryFact,
} from '@agentcowork/core-domain';
import { requestConnector } from '../connection-manager.js';

/**
 * Spotify adapter — search tracks, artists, albums, playlists.
 *
 * Free OAuth (app registration is free, no quota charges for personal-use rate).
 * The Rust host resolves and refreshes the vault credential, then performs the
 * authenticated request. This adapter handles only non-secret response mapping.
 *
 * Endpoints:
 *   GET https://api.spotify.com/v1/search?q={q}&type={type}&limit={n}
 *   GET https://api.spotify.com/v1/artists/{id}
 *   GET https://api.spotify.com/v1/albums/{id}
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'query', type: 'string', description: 'Track, artist, album, or playlist name' },
    { name: 'type', type: 'string', description: 'One of track|artist|album|playlist (default track)' },
    { name: 'limit', type: 'number', description: 'Max results (default 5)' },
  ],
};

const SPOTIFY_API = 'https://api.spotify.com/v1';

async function spotifySearch(
  connector: ConnectorName,
  userId: string,
  query: string,
  type: string,
  limit: number,
  signal: AbortSignal | undefined,
): Promise<{ items: ConnectorResult['items']; totalCount: number }> {
  const url = `${SPOTIFY_API}/search?q=${encodeURIComponent(query)}&type=${encodeURIComponent(type)}&limit=${limit}`;
  const res = await requestConnector({
    connector,
    userId,
    request: { url, method: 'GET', headers: { Accept: 'application/json' } },
    ...(signal ? { signal } : {}),
  });
  if (!res?.ok) return { items: [], totalCount: 0 };
  const raw = (await res.json()) as {
    tracks?: { items: Array<SpotifyTrack> };
    artists?: { items: Array<SpotifyArtist> };
    albums?: { items: Array<SpotifyAlbum> };
    playlists?: { items: Array<SpotifyPlaylist> };
  };
  const items: ConnectorResult['items'] = [];
  if (raw.tracks?.items) {
    for (const t of raw.tracks.items) {
      items.push({
        id: `track-${t.id}`,
        title: t.name,
        snippet: `${t.artists?.map((a) => a.name).join(', ') || 'Unknown'} • ${t.album?.name || ''}`.slice(0, 280),
        url: t.external_urls?.spotify || '',
        metadata: {
          artist: t.artists?.map((a) => a.name).join(', '),
          album: t.album?.name,
          duration_ms: t.duration_ms,
          popularity: t.popularity,
        },
      });
    }
  }
  if (raw.artists?.items) {
    for (const a of raw.artists.items) {
      items.push({
        id: `artist-${a.id}`,
        title: a.name,
        snippet: `${a.followers?.total?.toLocaleString() || 0} followers • ${a.genres?.join(', ') || 'no genres'}`.slice(0, 280),
        url: a.external_urls?.spotify || '',
        metadata: { genres: a.genres?.join(', '), popularity: a.popularity },
      });
    }
  }
  if (raw.albums?.items) {
    for (const al of raw.albums.items) {
      items.push({
        id: `album-${al.id}`,
        title: al.name,
        snippet: `${al.artists?.map((x) => x.name).join(', ') || '?'} • ${al.release_date || ''}`.slice(0, 280),
        url: al.external_urls?.spotify || '',
        metadata: {
          artist: al.artists?.map((x) => x.name).join(', '),
          release_date: al.release_date,
          total_tracks: al.total_tracks,
        },
      });
    }
  }
  if (raw.playlists?.items) {
    for (const p of raw.playlists.items) {
      items.push({
        id: `playlist-${p.id}`,
        title: p.name,
        snippet: `By ${p.owner?.display_name || '?'} • ${p.tracks?.total ?? 0} tracks`.slice(0, 280),
        url: p.external_urls?.spotify || '',
        metadata: { owner: p.owner?.display_name, tracks: p.tracks?.total },
      });
    }
  }
  return { items, totalCount: items.length };
}

type SpotifyTrack = { id: string; name: string; artists?: Array<{ name: string }>; album?: { name: string }; duration_ms?: number; popularity?: number; external_urls?: { spotify?: string } };
type SpotifyArtist = { id: string; name: string; followers?: { total?: number }; genres?: string[]; popularity?: number; external_urls?: { spotify?: string } };
type SpotifyAlbum = { id: string; name: string; artists?: Array<{ name: string }>; release_date?: string; total_tracks?: number; external_urls?: { spotify?: string } };
type SpotifyPlaylist = { id: string; name: string; owner?: { display_name?: string }; tracks?: { total?: number }; external_urls?: { spotify?: string } };

export class SpotifyAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'spotify';
  readonly credentialMode = 'host-mediated' as const;
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true; // Credential resolution is host-owned
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['spotify', 'playlist', 'album', 'artist', 'song', 'track', 'music', 'listen to', 'play ', 'spotify play'];
    if (terms.some((t) => q.includes(t))) return 0.85;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    const text = query.text || '';
    const type = /playlist/.test(text) ? 'playlist'
      : /album/.test(text) ? 'album'
      : /artist/.test(text) ? 'artist'
      : 'track';
    return { query: text, type, limit: 5 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { query?: string; type?: string; limit?: number };
    const q = (f.query || '').trim();
    if (!q) return { items: [], totalCount: 0, source: this.name };
    const limit = Math.min(Math.max(Number(f.limit) || 5, 1), 10);
    const type = f.type || 'track';
    try {
      const { items, totalCount } = await spotifySearch(this.name, ctx.userId, q, type, limit, ctx.signal);
      return { items, totalCount, source: this.name };
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }
}
