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
 * Google Places (New) adapter — text search + nearby POI lookup.
 *
 * Uses a vault-owned Google Places API key. The Rust host injects it after
 * validating the request; this adapter never receives key material.
 *
 * Endpoints:
 *   POST https://places.googleapis.com/v1/places:searchText (host injects auth)
 *   POST https://places.googleapis.com/v1/places:searchNearby
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'query', type: 'string', description: 'Place / business name or search text' },
    { name: 'lat', type: 'number', description: 'Latitude for nearby search' },
    { name: 'lon', type: 'number', description: 'Longitude for nearby search' },
    { name: 'radius', type: 'number', description: 'Radius (m) for nearby search, default 1500' },
    { name: 'open_now', type: 'boolean', description: 'Filter to currently-open places' },
    { name: 'limit', type: 'number', description: 'Max results (default 5)' },
  ],
};

const PLACES_API = 'https://places.googleapis.com/v1';

export class GooglePlacesAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'google-places';
  readonly credentialMode = 'host-mediated' as const;
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true; // Credential resolution is host-owned
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['nearby', 'near me', 'restaurant', 'cafe', 'coffee', 'shop', 'store', 'gym', 'park', 'hotel', 'bar', 'atm', 'pharmacy'];
    if (terms.some((t) => q.includes(t))) return 0.7;
    if (/open (now|right now|today)/i.test(q)) return 0.85;
    if (/find\b.*\b(near|in)\b/i.test(q)) return 0.65;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    const text = query.text || '';
    const openNow = /open (now|right now)/i.test(text);
    return { query: text, open_now: openNow, limit: 5, radius: 1500 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { query?: string; lat?: number; lon?: number; radius?: number; open_now?: boolean; limit?: number };
    const limit = Math.min(Math.max(Number(f.limit) || 5, 1), 20);
    let url: string;
    let body: Record<string, unknown>;
    if (typeof f.lat === 'number' && typeof f.lon === 'number') {
      url = `${PLACES_API}/places:searchNearby`;
      body = {
        maxResultCount: limit,
        locationRestriction: { circle: { center: { latitude: f.lat, longitude: f.lon }, radius: f.radius || 1500 } },
        ...(f.open_now ? { openNow: true } : {}),
      };
    } else {
      const text = (f.query || '').trim();
      if (!text) return { items: [], totalCount: 0, source: this.name };
      url = `${PLACES_API}/places:searchText`;
      body = { textQuery: text, maxResultCount: limit, ...(f.open_now ? { openNow: true } : {}) };
    }

    try {
      const res = await requestConnector({
        connector: this.name,
        userId: ctx.userId,
        request: {
          url,
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'X-Goog-FieldMask': 'places.id,places.displayName,places.formattedAddress,places.rating,places.priceLevel,places.types,places.currentOpeningHours.openNow,places.location',
            Accept: 'application/json',
          },
          body: JSON.stringify(body),
        },
        ...(ctx.signal ? { signal: ctx.signal } : {}),
      });
      if (!res?.ok) return { items: [], totalCount: 0, source: this.name };
      const raw = (await res.json()) as { places?: Array<{
        id: string;
        displayName?: { text?: string };
        formattedAddress?: string;
        rating?: number;
        priceLevel?: string;
        types?: string[];
        currentOpeningHours?: { openNow?: boolean };
        location?: { latitude: number; longitude: number };
      }> };
      const items: ConnectorResult['items'] = (raw.places ?? []).map((p) => ({
        id: `place-${p.id}`,
        title: p.displayName?.text || 'Unknown place',
        snippet: [
          p.formattedAddress,
          p.rating ? `★ ${p.rating}` : '',
          p.priceLevel ? p.priceLevel : '',
          p.currentOpeningHours?.openNow ? 'open now' : '',
        ].filter(Boolean).join(' · ').slice(0, 280),
        url: `https://www.google.com/maps/search/?api=1&query=${encodeURIComponent(p.displayName?.text || '')}&query_place_id=${encodeURIComponent(p.id)}`,
        metadata: {
          address: p.formattedAddress,
          rating: p.rating,
          open: p.currentOpeningHours?.openNow,
          types: p.types?.join(','),
        },
      }));
      return { items, totalCount: items.length, source: this.name };
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }

}
