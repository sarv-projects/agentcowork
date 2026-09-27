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
 * Public Holidays connector — Nager.Date API.
 *
 * 100% free, no API key, no signup, no rate limit (per docs).
 * Endpoints (v3):
 *   GET https://date.nager.at/api/v3/PublicHolidays/{year}/{countryCode}
 *   GET https://date.nager.at/api/v3/AvailableCountries
 *   GET https://date.nager.at/api/v3/LongWeekend/{year}/{countryCode}
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'countryCode', type: 'string', description: 'ISO 3166-1 alpha-2 country code (e.g. US, IN, DE)' },
    { name: 'year', type: 'number', description: 'Year (default current year)' },
  ],
};

const NAGER_BASE = 'https://date.nager.at/api/v3';
// Sensible default that works without geo-detection.
const DEFAULT_COUNTRY = 'US';

export class PublicHolidaysAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'public-holidays';
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['holiday', 'holidays', 'long weekend', 'public holiday', 'bank holiday', 'off day'];
    return terms.some((t) => q.includes(t)) ? 0.85 : 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    const text = query.text || '';
    // Try to find an ISO country code in the query (uppercase 2 letters)
    const codeMatch = text.match(/\b([A-Z]{2})\b/);
    return {
      countryCode: codeMatch ? codeMatch[1] : DEFAULT_COUNTRY,
      year: new Date().getFullYear(),
    };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { countryCode?: string; year?: number };
    const countryCode = (f.countryCode || DEFAULT_COUNTRY).toUpperCase();
    const year = Number(f.year) || new Date().getFullYear();

    try {
      const url = `${NAGER_BASE}/PublicHolidays/${year}/${encodeURIComponent(countryCode)}`;
      const res = await fetch(url, {
        headers: { Accept: 'application/json', 'User-Agent': 'PersonalAI/1.0' },
        signal: ctx.signal ?? null,
      });
      if (!res.ok) {
        return { items: [], totalCount: 0, source: this.name };
      }
      const data = (await res.json()) as Array<{
        date?: string;
        localName?: string;
        name?: string;
        countryCode?: string;
        global?: boolean;
        types?: string[];
      }>;
      const items: ConnectorResult['items'] = data.slice(0, 30).map((h, idx) => ({
        id: `${idx}-${h.date || ''}`,
        title: h.localName || h.name || 'Holiday',
        snippet: `${h.name || ''} • ${h.global ? 'National' : 'Regional'} • ${(h.types ?? []).join(', ')}`.trim(),
        date: h.date || '',
        metadata: { countryCode: h.countryCode, global: h.global, types: h.types },
      }));
      return { items, totalCount: items.length, source: this.name };
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
