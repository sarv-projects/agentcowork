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

/**
 * Geocoding connector — OpenStreetMap Nominatim.
 *
 * 100% free, no signup, no API key. Policy requires:
 *   - meaningful User-Agent with contact info
 *   - max 1 req/sec
 *   - cache results where possible
 *   - do NOT do bulk geocoding
 *
 * Endpoints:
 *   GET https://nominatim.openstreetmap.org/search?q={q}&format=jsonv2&limit={n}
 *   GET https://nominatim.openstreetmap.org/reverse?lat={lat}&lon={lon}&format=jsonv2
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'query', type: 'string', description: 'Free-text location to search' },
    { name: 'lat', type: 'number', description: 'Latitude (for reverse geocoding)' },
    { name: 'lon', type: 'number', description: 'Longitude (for reverse geocoding)' },
    { name: 'limit', type: 'number', description: 'Max results (default 3)' },
  ],
};

const NOMINATIM = 'https://nominatim.openstreetmap.org';
// Required by Nominatim policy — must include contact info.
const USER_AGENT = 'PersonalAI/1.0 (https://github.com/sarv-projects/APP)';
// Optional Cloudflare Worker proxy cap. Mobile sets EXPO_PUBLIC_CONNECTOR_PROXY
// to the Worker URL; the Worker applies per-user rate gates and centralises
// the User-Agent. Direct fetches remain as a fallback so on-device tools keep
// working when the Worker is unreachable.
const PROXY_BASE = (typeof process !== 'undefined' && (process as { env?: Record<string, string | undefined> }).env?.EXPO_PUBLIC_CONNECTOR_PROXY) || '';

// Single-flight queue: Nominatim's Acceptable Use Policy requires
// `max 1 request per second`. We serialise fetch() calls and pad a 1.1s gap
// between consecutive requests so a chat loop can't trip the IP-banned gate.
let lastRequestAt = 0;
const MIN_INTERVAL_MS = 1100;

async function takeNominatimSlot(): Promise<void> {
  const wait = Math.max(0, MIN_INTERVAL_MS - (Date.now() - lastRequestAt));
  if (wait > 0) await new Promise((r) => setTimeout(r, wait));
  lastRequestAt = Date.now();
}

/**
 * @internal Test-only. Resets the module-level throttle clock so tests that
 * mix fake/real timers can't leak a stale `lastRequestAt` (which would make
 * the next real-timer fetch wait up to 1.1s and trip vitest's 5s timeout).
 */
export function __resetNominatimThrottleForTests(): void {
  lastRequestAt = 0;
}

type OsmRow = {
  display_name?: string;
  name?: string;
  lat?: string;
  lon?: string;
  type?: string;
  importance?: number;
  address?: Record<string, string>;
};

function osmItems(raw: unknown): ConnectorResult['items'] {
  const list: OsmRow[] = Array.isArray(raw) ? (raw as OsmRow[]) : [raw as OsmRow];
  return list.filter(Boolean).map((r, idx) => {
    const addressBits = r.address
      ? `${r.address.city || r.address.town || r.address.village || r.address.county || ''}, ${r.address.country || ''}`
      : '';
    return {
      id: `place-${idx}-${r.lat || ''}-${r.lon || ''}`,
      title: r.name || (addressBits ? addressBits.replace(/^,\s*/, '') : r.display_name || 'Location'),
      snippet: (r.display_name || addressBits || '').slice(0, 280),
      metadata: { lat: Number(r.lat), lon: Number(r.lon), type: r.type },
      url: r.lat && r.lon ? `https://www.openstreetmap.org/?mlat=${r.lat}&mlon=${r.lon}#map=15/${r.lat}/${r.lon}` : '',
    };
  });
}

function osmResult(raw: unknown): ConnectorResult {
  const items = osmItems(raw);
  return { items, totalCount: items.length, source: 'nominatim' };
}

export class NominatimAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'nominatim';
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['geocode', 'coordinates', 'latitude', 'longitude', 'where is', 'address of', 'lat ', 'lon '];
    if (terms.some((t) => q.includes(t))) return 0.8;
    // Detect "X" pattern (city/country lookups)
    if (/where (is|in) [a-z]/i.test(q)) return 0.5;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text || '', limit: 3 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { query?: string; lat?: number; lon?: number; limit?: number };
    const limit = Math.min(Math.max(Number(f.limit) || 3, 1), 5);

    try {
      // Forward-search → Worker proxy when configured (per-user rate gate,
      // central User-Agent, optional Cloud NAT-bound egress IP).
      if (PROXY_BASE && typeof f.query === 'string' && f.query) {
        const proxyUrl =
          `${PROXY_BASE}/v1/connectors/proxy/osm-search?q=${encodeURIComponent(f.query)}&limit=${limit}` +
          ((ctx as unknown as { deviceId?: string }).deviceId ? `&deviceId=${encodeURIComponent((ctx as unknown as { deviceId?: string }).deviceId!)}` : '');
        const res = await fetch(proxyUrl, {
          headers: {
            'x-device-id': (ctx as unknown as { deviceId?: string }).deviceId || '',
            'User-Agent': USER_AGENT,
            Accept: 'application/json',
          },
          signal: ctx.signal ?? null,
        });
        if (res.ok) return osmResult(await res.json());
        // Fall through to direct path on proxy failure / 429.
        if (res.status !== 429) {
          return { items: [], totalCount: 0, source: this.name };
        }
      }

      // Honour the 1 req/sec AUP.
      await takeNominatimSlot();

      let url: string;
      if (typeof f.lat === 'number' && typeof f.lon === 'number') {
        const params = new URLSearchParams({
          lat: String(f.lat),
          lon: String(f.lon),
          format: 'jsonv2',
        });
        url = `${NOMINATIM}/reverse?${params.toString()}`;
      } else {
        const query = (f.query || '').trim();
        if (!query) {
          return { items: [], totalCount: 0, source: this.name };
        }
        const params = new URLSearchParams({
          q: query,
          format: 'jsonv2',
          limit: String(limit),
          addressdetails: '1',
        });
        url = `${NOMINATIM}/search?${params.toString()}`;
      }

      const res = await fetch(url, {
        headers: { 'User-Agent': USER_AGENT, Accept: 'application/json' },
        signal: ctx.signal ?? null,
      });
      if (!res.ok) {
        return { items: [], totalCount: 0, source: this.name };
      }

      // Search returns an array; reverse returns a single object.
      const raw = await res.json();
      return osmResult(raw);
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }

  /** 
   * Token refresh is handled by the Cloudflare Worker OAuth proxy.
   * This adapter assumes a valid token is injected via filter.token.
   * @see packages/cloudflare-server/src/index.ts OAuth refresh routes
   */
}
