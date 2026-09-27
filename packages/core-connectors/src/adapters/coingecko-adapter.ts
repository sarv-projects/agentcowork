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
 * CoinGecko adapter — free crypto prices, no auth required.
 *
 * Free tier (public): 10-30 req/min. /coins/markets and /search endpoints.
 * Credential-bearing quota elevation is host-owned and never enters this adapter.
 *
 * Endpoints:
 *   GET https://api.coingecko.com/api/v3/coins/markets?vs_currency=usd&ids={ids}
 *   GET https://api.coingecko.com/api/v3/search?query={q}
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'query', type: 'string', description: 'Coin name or symbol (e.g. bitcoin, eth, sol)' },
    { name: 'vs_currency', type: 'string', description: 'Fiat currency (default usd)' },
    { name: 'limit', type: 'number', description: 'Max results (default 5)' },
  ],
};

const CG_API = 'https://api.coingecko.com/api/v3';

export class CoingeckoAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'coingecko';
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = [
      'crypto', 'bitcoin', 'btc', 'ethereum', 'eth', 'solana', 'sol', 'token', 'coin', 'price of',
      'market cap', 'cryptocurrency', 'altcoin', 'stablecoin', 'usdt', 'usdc',
    ];
    if (terms.some((t) => q.includes(t))) return 0.9;
    if (/coin|token/i.test(q)) return 0.5;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    const text = query.text || '';
    const m = text.match(/\b(btc|eth|sol|ada|doge|xrp|usdt|usdc|bnb|matic|dot|ltc|avax|trx|link|ton|shib)\b/i);
    const coinId = m ? m[1]!.toLowerCase() : text;
    return { query: coinId, vs_currency: 'usd', limit: 5 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { query?: string; vs_currency?: string; limit?: number };
    if (!f.query) return { items: [], totalCount: 0, source: this.name };
    const vs = (f.vs_currency || 'usd').toLowerCase();
    const limit = Math.min(Math.max(Number(f.limit) || 5, 1), 25);
    const url = `${CG_API}/coins/markets?vs_currency=${encodeURIComponent(vs)}&ids=${encodeURIComponent(f.query.toLowerCase())}&order=market_cap_desc&per_page=${limit}&sparkline=false&price_change_percentage=24h`;
    const headers: Record<string, string> = { Accept: 'application/json' };
    const signal = (ctx as unknown as { signal?: AbortSignal }).signal;

    try {
      const res = await fetch(url, {
        headers,
        ...(signal ? { signal } : {}),
      });
      if (!res.ok) {
        // Fallback: search endpoint to resolve coin id from name
        if (res.status === 404) {
          return await this.searchFallback(f.query, limit, signal);
        }
        return { items: [], totalCount: 0, source: this.name };
      }
      const raw = (await res.json()) as Array<{
        id: string;
        symbol: string;
        name: string;
        current_price: number;
        market_cap: number;
        market_cap_rank: number;
        price_change_percentage_24h: number;
        image?: string;
      }>;
      const items: ConnectorResult['items'] = (Array.isArray(raw) ? raw : []).map((c) => ({
        id: `coin-${c.id}`,
        title: `${c.name} (${c.symbol.toUpperCase()})`,
        snippet: `$${c.current_price?.toLocaleString()} • mcap $${Math.round((c.market_cap || 0) / 1_000_000)}M • 24h ${(c.price_change_percentage_24h ?? 0).toFixed(2)}% • rank #${c.market_cap_rank}`.slice(0, 280),
        url: `https://www.coingecko.com/en/coins/${c.id}`,
        metadata: {
          symbol: c.symbol,
          price: c.current_price,
          market_cap: c.market_cap,
          change_24h: c.price_change_percentage_24h,
          rank: c.market_cap_rank,
        },
      }));
      return { items, totalCount: items.length, source: this.name };
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }

  private async searchFallback(query: string, limit: number, signal: AbortSignal | undefined): Promise<ConnectorResult> {
    const headers: Record<string, string> = { Accept: 'application/json' };
    try {
      const res = await fetch(`${CG_API}/search?query=${encodeURIComponent(query)}`, {
        headers,
        ...(signal ? { signal } : {}),
      });
      if (!res.ok) return { items: [], totalCount: 0, source: 'coingecko' };
      const raw = (await res.json()) as { coins?: Array<{ id: string; name: string; symbol: string; market_cap_rank?: number }> };
      const coins = (raw.coins ?? []).slice(0, limit);
      const items: ConnectorResult['items'] = coins.map((c) => ({
        id: `coin-search-${c.id}`,
        title: `${c.name} (${c.symbol.toUpperCase()})`,
        snippet: `Match by name${c.market_cap_rank ? ' • rank #' + c.market_cap_rank : ''}`,
        url: `https://www.coingecko.com/en/coins/${c.id}`,
        metadata: { symbol: c.symbol, rank: c.market_cap_rank },
      }));
      return { items, totalCount: items.length, source: 'coingecko' };
    } catch {
      return { items: [], totalCount: 0, source: 'coingecko' };
    }
  }

}
