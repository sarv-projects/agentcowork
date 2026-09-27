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
 * REST Countries connector — restcountries.com.
 *
 * 100% free, no auth, no signup. Public country metadata (name, capital,
 * region, languages, currencies, calling codes, etc.).
 *
 * Endpoints:
 *   GET https://restcountries.com/v3.1/name/{name}
 *   GET https://restcountries.com/v3.1/alpha/{code}
 *   GET https://restcountries.com/v3.1/all?fields=name,capital,region
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'query', type: 'string', description: 'Country name or ISO code (alpha-2 or alpha-3)' },
  ],
};

const RESTCOUNTRIES = 'https://restcountries.com/v3.1';

export class RestCountriesAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'restcountries';
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    if (/\b(capital|currency|country|iso|continent|population of|languages of|phone code|dialing code)\b/.test(q)) return 0.85;
    // 2-letter or 3-letter uppercase tokens
    if (/\b([A-Z]{2}|[A-Z]{3})\b/.test(q)) return 0.4;
    return 0.15;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text || '' };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { query?: string };
    const raw = (f.query || '').trim();
    if (!raw) {
      return { items: [], totalCount: 0, source: this.name };
    }
    const trimmed = raw.replace(/[?.!]+$/, '').trim();
    // Alpha-2 / alpha-3 code → alpha endpoint; otherwise name endpoint.
    const isCode = /^[A-Za-z]{2,3}$/.test(trimmed);
    const path = isCode ? `/alpha/${encodeURIComponent(trimmed)}` : `/name/${encodeURIComponent(trimmed)}`;
    const url = `${RESTCOUNTRIES}${path}?fields=name,capital,region,subregion,population,languages,currencies,idd,flag,area`;

    try {
      const res = await fetch(url, {
        headers: { Accept: 'application/json' },
        signal: ctx.signal ?? null,
      });
      if (!res.ok) {
        return { items: [], totalCount: 0, source: this.name };
      }
      const data = (await res.json()) as Array<{
        name?: { common?: string; official?: string };
        capital?: string[];
        region?: string;
        subregion?: string;
        population?: number;
        languages?: Record<string, string>;
        currencies?: Record<string, { name?: string; symbol?: string }>;
        idd?: { root?: string; suffixes?: string[] };
        flag?: string;
        area?: number;
      }>;

      const items: ConnectorResult['items'] = (Array.isArray(data) ? data : [data]).slice(0, 5).map((c, idx) => {
        const common = c.name?.common || '(unknown)';
        const capital = (c.capital ?? []).join(', ');
        const currencies = Object.entries(c.currencies ?? {}).map(([k, v]) => `${v.name || k} (${v.symbol || ''})`).join(', ');
        const languages = Object.values(c.languages ?? {}).join(', ');
        const dialing = (c.idd?.root ?? '') + ((c.idd?.suffixes ?? []).join(' '));
        const parts: string[] = [];
        if (capital) parts.push(`Capital: ${capital}`);
        if (c.region) parts.push(`${c.subregion || c.region}`);
        if (currencies) parts.push(`Currency: ${currencies}`);
        if (languages) parts.push(`Languages: ${languages}`);
        if (dialing) parts.push(`☎ ${dialing}`);
        if (c.population) parts.push(`Pop: ${c.population.toLocaleString()}`);

        return {
          id: `country-${idx}-${common}`,
          title: `${c.flag || ''} ${common}`.trim(),
          snippet: parts.join(' • ').slice(0, 280),
          metadata: {
            officialName: c.name?.official,
            region: c.region,
            subregion: c.subregion,
            areaKm2: c.area,
            population: c.population,
            languages: c.languages,
            currencies: c.currencies,
            dialing,
          },
        };
      });
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
