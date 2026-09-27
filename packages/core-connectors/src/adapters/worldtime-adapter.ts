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
 * World time / timezone connector — WorldTimeAPI.
 *
 * 100% free, no API key, no signup. (Hosted by David Fowler / community fork
 * 'worldtimeapi.org' but also available at 'timeapi.io'.)
 *
 * Endpoints:
 *   GET https://worldtimeapi.org/api/timezone/{area}/{location}
 *   GET https://worldtimeapi.org/api/ip (best-effort city/timezone lookup)
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'timezone', type: 'string', description: 'IANA timezone, e.g. "Europe/Berlin"' },
  ],
};

const WORLDTIME = 'https://worldtimeapi.org/api/timezone';

export class WorldtimeAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'worldtime';
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['time in', 'timezone', 'utc', 'gmt', 'current time', 'date in', 'what time'];
    return terms.some((t) => q.includes(t)) ? 0.8 : 0.15;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    const text = query.text || '';
    // Try to extract a timezone
    const tzMatch = text.match(/\b(UTC|GMT)([+-]\d{1,2})?\b/i);
    return {
      timezone: tzMatch ? tzMatch[0].toUpperCase().replace('GMT', 'Etc/GMT') : '',
    };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { timezone?: string };
    const tz = (f.timezone || '').trim();

    try {
      let url: string;
      if (tz) {
        url = `${WORLDTIME}/${encodeURIComponent(tz)}`;
      } else {
        // Fallback: UTC
        url = `${WORLDTIME}/Etc/UTC`;
      }
      const res = await fetch(url, {
        headers: { Accept: 'application/json' },
        signal: ctx.signal ?? null,
      });
      if (!res.ok) {
        return { items: [], totalCount: 0, source: this.name };
      }
      const data = (await res.json()) as {
        timezone?: string;
        utc_datetime?: string;
        datetime?: string;
        unixtime?: number;
        utc_offset?: string;
        abbreviation?: string;
        client_ip?: string;
      };
      const tzLabel = data.timezone || tz || 'UTC';
      const dtLabel = data.utc_datetime || data.datetime || '';
      const item: ConnectorResult['items'][number] = {
        id: `tz-${tzLabel}`,
        title: `Time in ${tzLabel}`,
        snippet: `Local: ${dtLabel} • Offset: ${data.utc_offset || ''} • Abbr: ${data.abbreviation || ''}`.trim(),
        metadata: {
          timezone: tzLabel,
          unixtime: data.unixtime,
          abbreviations: data.abbreviation,
          offset: data.utc_offset,
        },
      };
      if (dtLabel) item.date = dtLabel;
      return { items: [item], totalCount: 1, source: this.name };
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
