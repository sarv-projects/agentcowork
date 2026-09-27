/**
 * AviationStack connector — real-time flight status / schedule lookup.
 *
 * Free tier: 100 requests / month (https://aviationstack.com/pricing).
 * Equivalent to Gemini's Google Flights extension; answers "is my flight
 * delayed" with live data.
 *
 * IMPORTANT: AviationStack's free tier serves `http://`, not HTTPS. Mobile
 * apps block cleartext (iOS ATS, Android default), so this adapter routes
 * through the Worker via `/v1/connectors/proxy/aviationstack`. The Worker
 * fetches the proxy target without the mobile needing to deal with TLS.
 *
 * Auth is injected by the configured server-side provider proxy. User key
 * material is never accepted into this adapter.
 */
import type {
  ConnectorAdapter,
  ConnectorContext,
  ConnectorFilter,
  ConnectorResult,
  MemoryFact,
  UserQuery,
} from '@agentcowork/core-domain';

const AVIATIONSTACK_VIA_WORKER = '/v1/connectors/proxy/aviationstack';
const CONNECTOR_NAME = 'aviationstack' as const;

export class AviationstackAdapter implements ConnectorAdapter {
  readonly name = CONNECTOR_NAME;
  readonly metadataSchema = {
    fields: [
      { name: 'query', type: 'string' as const, description: 'Flight IATA code (e.g. "UA123") or airline route' },
      { name: 'limit', type: 'number' as const, description: 'Max results (1-10)' },
    ],
  };

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    if (/\b(flight|flights|airline|airport|boarding|departure|arrival|gate|delayed)\b/.test(q)) return 0.85;
    if (/[a-z]{2}\d{1,4}[a-z]?/i.test(q)) return 0.7; // bare flight code
    if (/\b(book(ed|ing)?|ticket)\b/.test(q) && /\b(airport|fly|flight)\b/.test(q)) return 0.6;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text || '' };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = ctx.filter as { query?: string; limit?: number };
    const q = (f.query || '').trim();
    const limit = Math.min(f.limit ?? 5, 10);
    if (!q) return { items: [], totalCount: 0, source: CONNECTOR_NAME };

    // Detect a flight IATA code (e.g. "UA123", "BA2490"). If the query
    // is a free-form text, fall back to the flights endpoint with a search.
    const flightMatch = q.match(/\b([A-Za-z]{2})\s?\d{1,4}[A-Za-z]?\b/);
    const params = new URLSearchParams();
    if (flightMatch) {
      // AviationStack accepts flight_iata parameter.
      params.set('flight_iata', flightMatch[0].replace(/\s+/g, '').toUpperCase());
    } else {
      params.set('airline_name', q);
    }
    params.set('limit', String(limit));

    // Use Worker proxy. Worker's addUserHeaders will inject the per-user API
    // key from KV if present; without the proxy the mobile can't reach
    // AviationStack (HTTP-only free tier).
    const proxyUrl = `${AVIATIONSTACK_VIA_WORKER}?${params.toString()}`;

    try {
      const res = await fetch(proxyUrl);
      if (!res.ok) {
        return {
          items: [
            {
              id: `flight:err:${res.status}`,
              title: 'AviationStack lookup failed',
              snippet: `Worker proxy returned ${res.status}. Free tier is 100 req/month; check quota or supply your own key.`,
              metadata: { status: res.status },
            },
          ],
          totalCount: 1,
          source: CONNECTOR_NAME,
        };
      }
      const data = (await res.json()) as {
        data?: Array<{
          flight_date?: string;
          flight_status?: string;
          departure?: { airport?: string; iata?: string; scheduled?: string; estimated?: string };
          arrival?: { airport?: string; iata?: string; scheduled?: string; estimated?: string };
          airline?: { name?: string; iata?: string };
          flight?: { iata?: string; number?: string };
        }>;
        error?: { info?: string };
      };

      if (data.error) {
        return {
          items: [
            {
              id: `flight:err:api`,
              title: 'AviationStack API error',
              snippet: data.error.info ?? 'Unknown API error',
            },
          ],
          totalCount: 1,
          source: CONNECTOR_NAME,
        };
      }

      const items: ConnectorResult['items'] = (data.data ?? []).slice(0, limit).map((flt) => {
        const dep = `${flt.departure?.iata ?? '?'} ${flt.departure?.scheduled ?? ''}`;
        const arr = `${flt.arrival?.iata ?? '?'} ${flt.arrival?.scheduled ?? ''}`;
        const status = flt.flight_status ?? 'unknown';
        const item: ConnectorResult['items'][number] = {
          id: `${flt.flight?.iata ?? 'flight'}:${flt.flight_date ?? Date.now()}`,
          title: `${flt.flight?.iata ?? 'flight'} — ${status}`,
          snippet: `${flt.airline?.name ?? 'Airline'} · ${dep} → ${arr}`,
          metadata: {
            airline: flt.airline?.iata,
            flight: flt.flight?.iata,
            status,
            departure: flt.departure,
            arrival: flt.arrival,
          },
        };
        if (flt.flight_date) item.date = `${flt.flight_date}T00:00:00Z`;
        return item;
      });

      return { items, totalCount: items.length, source: CONNECTOR_NAME };
    } catch {
      return { items: [], totalCount: 0, source: CONNECTOR_NAME };
    }
  }

}
