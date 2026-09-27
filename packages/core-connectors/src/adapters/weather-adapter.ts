import type {
  ConnectorAdapter,
  ConnectorContext,
  ConnectorFilter,
  ConnectorItem,
  ConnectorMetadataSchema,
  ConnectorName,
  ConnectorResult,
  MemoryFact,
  UserQuery,
} from '@agentcowork/core-domain';

/**
 * Weather connector — Open-Meteo (free, no key, Day-0 per spec §12.1).
 * Pure fetch, works in RN and Cloudflare Worker.
 * Read-only. Returns current + simple forecast as items.
 */

const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'location', type: 'string', description: 'City or lat,lon' },
    { name: 'days', type: 'number', description: 'Forecast days (1-7)' },
  ],
};

export class WeatherAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'weather';
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    // Open-Meteo requires no auth
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const weatherTerms = ['weather', 'temperature', 'rain', 'forecast', 'temp', 'celsius', 'fahrenheit', 'humidity'];
    const score = weatherTerms.some((term) => q.includes(term)) ? 0.85 : 0.1;
    return score;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    const q = query.text || '';
    const locMatch = q.match(/in ([A-Za-z ]+)/i) || q.match(/([A-Za-z]+) weather/i);
    const location = locMatch?.[1] ? locMatch[1].trim() : 'Bengaluru';
    return { location, days: 2 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { location?: string; days?: number };
    const location = f.location || 'Bengaluru';
    const days = f.days || 2;

    // Geocode roughly via open-meteo search (or hardcode common; for real use better geocode)
    // To keep zero-dep and simple: use a small known list + fallback to query param if numeric.
    let lat = '12.97';
    let lon = '77.59';
    const known: Record<string, [string, string]> = {
      bengaluru: ['12.97', '77.59'],
      bangalore: ['12.97', '77.59'],
      mumbai: ['19.07', '72.87'],
      delhi: ['28.61', '77.23'],
      'new delhi': ['28.61', '77.23'],
      hyderabad: ['17.38', '78.48'],
      chennai: ['13.08', '80.27'],
      kolkata: ['22.57', '88.36'],
    };
    const locStr = location || '';
    const key = locStr.toLowerCase();
    if (known[key]) {
      [lat, lon] = known[key];
    } else if (/-?\d+\.?\d*,\s*-?\d+\.?\d*/.test(locStr)) {
      const parts = locStr.split(',').map((s: string) => s.trim());
      if (parts[0]) lat = parts[0];
      if (parts[1]) lon = parts[1];
    }

    const url = `https://api.open-meteo.com/v1/forecast?latitude=${lat}&longitude=${lon}&current=temperature_2m,relative_humidity_2m,weather_code&daily=weather_code,temperature_2m_max,temperature_2m_min&timezone=auto&forecast_days=${Math.min(Math.max(1, Number(days) || 2), 7)}`;

    try {
      const res = await fetch(url, { signal: ctx.signal ?? null });
      if (!res.ok) throw new Error(`Open-Meteo ${res.status}`);
      const data = (await res.json()) as {
        current?: { temperature_2m?: number; relative_humidity_2m?: number; weather_code?: number };
        daily?: {
          time?: string[];
          temperature_2m_max?: number[];
          temperature_2m_min?: number[];
          weather_code?: number[];
        };
      };

      const items: ConnectorResult['items'] = [];

      const current = data.current;
      if (current) {
        items.push({
          id: `current-${lat}-${lon}`,
          title: `Current weather in ${location}`,
          snippet: `Temp: ${current.temperature_2m}°C, Humidity: ${current.relative_humidity_2m}%`,
          date: new Date().toISOString(),
          metadata: { type: 'current', location, lat, lon },
        });
      }

      const daily = data.daily;
      if (daily?.time?.length) {
        const maxArr = daily.temperature_2m_max;
        const minArr = daily.temperature_2m_min;
        for (let i = 0; i < Math.min(daily.time.length, 3); i++) {
          const day = daily.time[i];
          if (!day) continue;
          const dayItem: ConnectorItem = {
            id: `daily-${day}`,
            title: `Forecast ${day}`,
            snippet: `Max ${maxArr?.[i] ?? '?'}°C / Min ${minArr?.[i] ?? '?'}°C`,
            metadata: { type: 'forecast', location },
          };
          dayItem.date = day;
          items.push(dayItem);
        }
      }

      return {
        items,
        totalCount: items.length,
        source: 'weather',
      };
    } catch {
      return {
        items: [],
        totalCount: 0,
        source: 'weather',
      };
    }
  }

  /** 
   * Token refresh is handled by the Cloudflare Worker OAuth proxy.
   * This adapter assumes a valid token is injected via filter.token.
   * @see packages/cloudflare-server/src/index.ts OAuth refresh routes
   */
}
