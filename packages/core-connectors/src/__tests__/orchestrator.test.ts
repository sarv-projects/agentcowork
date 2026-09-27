import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import type { ConnectorName } from '@agentcowork/core-domain';
import { ConnectorOrchestrator } from '../orchestrator';
import { setConnectorHostTransport, type ConnectorHostRequest } from '../connection-manager';
import * as publicApi from '../index';
import { WeatherAdapter } from '../adapters/weather-adapter';
import { RssAdapter } from '../adapters/rss-adapter';
import { GitHubAdapter } from '../adapters/github-adapter';
import { NotionOAuthAdapter } from '../adapters/notion-oauth-adapter';
import { TelegramAdapter } from '../adapters/telegram-adapter';

const WEATHER_OPENMETEO_RESPONSE = {
  current: {
    temperature_2m: 15,
    weather_code: 3,
    wind_speed_10m: 8,
    relative_humidity_2m: 60,
  },
  current_units: { temperature_2m: '°C', wind_speed_10m: 'km/h' },
  timezone: 'Europe/London',
};

describe('ConnectorOrchestrator', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: true,
      json: async () => WEATHER_OPENMETEO_RESPONSE,
      text: async () => JSON.stringify(WEATHER_OPENMETEO_RESPONSE),
    } as Response));
  });
  afterEach(() => {
    setConnectorHostTransport(null);
    vi.unstubAllGlobals();
  });

  it('registers and lists adapters', () => {
    const orch = new ConnectorOrchestrator();
    orch.register(new WeatherAdapter());
    orch.register(new RssAdapter());
    expect(orch.list()).toHaveLength(2);
  });

  it('plan returns empty shape when none authorized', async () => {
    const orch = new ConnectorOrchestrator();
    orch.register(new GitHubAdapter()); // below relevance threshold
    const plan = await orch.plan({ text: 'weather in London' }, []);
    expect(plan.adapters).toHaveLength(0);
    expect(plan.shape).toBe('single');
  });

  it('execute runs a single adapter', async () => {
    const orch = new ConnectorOrchestrator();
    const weather = new WeatherAdapter();
    orch.register(weather);

    const results = await orch.execute(
      { adapters: [weather], shape: 'single', filters: {} },
      { userId: 'test', query: { text: 'weather in London' } },
    );
    expect(results.length).toBeGreaterThan(0);
    expect(results[0]!.source).toBe('weather');
    expect(results[0]!.status).toBe('completed');
  });

  it('does not export the raw OAuth token fetcher', () => {
    expect(Object.prototype.hasOwnProperty.call(publicApi, 'fetchWorkerOAuthToken')).toBe(false);
  });

  it('fails closed when a host-mediated connector has no Rust transport', async () => {
    const orch = new ConnectorOrchestrator();
    const notion = new NotionOAuthAdapter();
    const fetchSpy = vi.spyOn(notion, 'fetch');

    const [outcome] = await orch.execute(
      {
        adapters: [notion],
        shape: 'single',
        filters: { notion: { query: 'roadmap', token: 'raw-oauth-value' } },
      },
      { userId: 'test', query: { text: 'notion roadmap' } },
    );

    expect(outcome?.status).toBe('unavailable');
    expect(outcome?.reason).toContain('not attached');
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('refuses a host credential resolver that returns raw material instead of a handle', async () => {
    const request = vi.fn();
    setConnectorHostTransport({
      resolveCredential: async () => ({
        handle: 'raw-oauth-value',
        provider: 'notion',
      }),
      request,
    });

    const result = await new NotionOAuthAdapter().fetch({
      userId: 'test',
      query: { text: 'notion roadmap' },
      filter: { query: 'roadmap' },
    });

    expect(result.items).toEqual([]);
    expect(request).not.toHaveBeenCalled();
    expect(JSON.stringify(result)).not.toContain('raw-oauth-value');
  });

  it('passes only an opaque handle and non-secret intent to the Rust connector host', async () => {
    const orch = new ConnectorOrchestrator();
    const notion = new NotionOAuthAdapter();
    let hostRequest: ConnectorHostRequest | undefined;
    const request = vi.fn(async (req: ConnectorHostRequest) => {
      hostRequest = req;
      return {
        ok: true,
        status: 200,
        json: async () => ({ results: [] }),
        text: async () => '{"results":[]}',
      };
    });
    setConnectorHostTransport({
      resolveCredential: async () => ({
        handle: 'vault:oauth:notion:user-1',
        provider: 'notion',
        expiresAtMs: 1234,
      }),
      request,
    });

    const [outcome] = await orch.execute(
      {
        adapters: [notion],
        shape: 'single',
        filters: {
          notion: {
            query: 'roadmap',
            token: 'raw-oauth-value',
            apiKey: 'raw-api-value',
          },
        },
      },
      { userId: 'test', query: { text: 'notion roadmap' } },
    );

    expect(outcome?.status).toBe('completed');
    expect(request).toHaveBeenCalledTimes(1);
    expect(hostRequest?.credential.handle).toBe('vault:oauth:notion:user-1');
    expect(hostRequest?.credential.provider).toBe('notion');
    expect(hostRequest?.request.headers).not.toHaveProperty('Authorization');
    expect(JSON.stringify(hostRequest)).not.toContain('raw-oauth-value');
    expect(JSON.stringify(hostRequest)).not.toContain('raw-api-value');
  });

  it('writeBack returns facts from results', async () => {
    const orch = new ConnectorOrchestrator();
    const facts = await orch.writeBack([
      {
        source: 'weather' as ConnectorName,
        status: 'completed' as const,
        result: {
          items: [
            { id: '1', title: 'London Weather', snippet: '15°C, cloudy with light rain expected throughout the day', url: '' },
          ],
          totalCount: 1,
          source: 'weather' as ConnectorName,
        },
      },
    ]);
    expect(facts.length).toBeGreaterThan(0);
    expect(facts[0]!.content).toContain('London');
  });
});

describe('WeatherAdapter', () => {
  it('requires no auth', async () => {
    const a = new WeatherAdapter();
    expect(await a.isAuthorized('any')).toBe(true);
  });

  it('scores weather queries high', () => {
    const a = new WeatherAdapter();
    const score = a.scoreRelevance({ text: 'what is the weather in Tokyo' }, []);
    expect(score).toBeGreaterThan(0.8);
  });

  it('scores non-weather queries low', () => {
    const a = new WeatherAdapter();
    const score = a.scoreRelevance({ text: 'how do I cook pasta' }, []);
    expect(score).toBeLessThan(0.3);
  });

  it('builds a filter with location', () => {
    const a = new WeatherAdapter();
    const f = a.buildFilter({ text: 'weather in Paris France' });
    expect(f.location).toMatch(/Paris/i);
  });
});

describe('GitHubAdapter', () => {
  it('allows anonymous access for public repos', async () => {
    const a = new GitHubAdapter();
    expect(await a.isAuthorized('test')).toBe(true);
  });

  it('scores code/repo queries high', () => {
    const a = new GitHubAdapter();
    const score = a.scoreRelevance({ text: 'find the repository for my project' }, []);
    expect(score).toBeGreaterThan(0.7);
  });
});

describe('RssAdapter', () => {
  it('requires no auth', async () => {
    const a = new RssAdapter();
    expect(await a.isAuthorized('any')).toBe(true);
  });

  it('scores news queries medium', () => {
    const a = new RssAdapter();
    const score = a.scoreRelevance({ text: 'latest tech news' }, []);
    expect(score).toBeGreaterThan(0.5);
  });
});

describe('NotionOAuthAdapter', () => {
  it('is host-mediated for authorization', async () => {
    const a = new NotionOAuthAdapter();
    expect(await a.isAuthorized('any')).toBe(true);
  });

  it('scores notion queries', () => {
    const a = new NotionOAuthAdapter();
    const score = a.scoreRelevance({ text: 'my notes from notion' }, []);
    expect(score).toBeGreaterThan(0.3);
  });
});

describe('TelegramAdapter', () => {
  it('is host-mediated and stores no bot credential', async () => {
    const a = new TelegramAdapter('-1001234');
    expect(a.credentialMode).toBe('host-mediated');
    expect(await a.isAuthorized('any')).toBe(true);
    expect(Object.values(a)).not.toContain('test:token');
  });

  it('builds filter from query text', () => {
    const a = new TelegramAdapter();
    const f = a.buildFilter({ text: 'send telegram message' });
    expect(typeof f).toBe('object');
  });
});

describe('ConnectorOrchestrator writeBack persistFn', () => {
  it('returns only facts for which persistFn resolves truthy', async () => {
    const orch = new ConnectorOrchestrator();
    const results = [
      {
        source: 'weather' as ConnectorName,
        status: 'completed' as const,
        result: {
          items: [
            { id: 'w1', title: 'Sunny', snippet: 'Sunny skies are expected across the region tomorrow', url: '' },
            { id: 'w2', title: 'Rainy', snippet: 'Rainy skies are expected across the region tomorrow', url: '' },
          ],
          totalCount: 2,
          source: 'weather' as ConnectorName,
        },
      },
    ];
    const seen: string[] = [];
    const facts = await orch.writeBack(results, async (fact) => {
      seen.push(fact.content);
      return fact.content.includes('Sunny');
    });
    expect(seen.length).toBe(2);
    expect(facts.length).toBe(1);
    expect(facts[0]!.content).toContain('Sunny');
  });

  it('skips facts when persistFn throws', async () => {
    const orch = new ConnectorOrchestrator();
    const results = [
      {
        source: 'rss' as ConnectorName,
        status: 'completed' as const,
        result: {
          items: [
            { id: 'r1', title: 'News', snippet: 'A long enough snippet to satisfy the minimum length filter', url: '' },
          ],
          totalCount: 1,
          source: 'rss' as ConnectorName,
        },
      },
    ];
    const facts = await orch.writeBack(results, async () => {
      throw new Error('persist failed');
    });
    expect(facts).toHaveLength(0);
  });
});

describe('ConnectorOrchestrator writeBack category classification', () => {
  it('classifies weather as other', async () => {
    const orch = new ConnectorOrchestrator();
    const facts = await orch.writeBack([
      {
        source: 'weather' as ConnectorName,
        status: 'completed' as const,
        result: {
          items: [
            { id: 'w', title: 'Today', snippet: 'Sunny with light winds expected this afternoon', url: '' },
          ],
          totalCount: 1,
          source: 'weather' as ConnectorName,
        },
      },
    ]);
    expect(facts[0]!.category).toBe('other');
  });

  it('classifies notion as work', async () => {
    const orch = new ConnectorOrchestrator();
    const facts = await orch.writeBack([
      {
        source: 'notion' as ConnectorName,
        status: 'completed' as const,
        result: {
          items: [
            { id: 'n', title: 'project plan', snippet: 'Project plan document with team goals and deadlines', url: '' },
          ],
          totalCount: 1,
          source: 'notion' as ConnectorName,
        },
      },
    ]);
    expect(facts[0]!.category).toBe('work');
  });
});
