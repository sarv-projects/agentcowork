import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SearchContext } from '@agentcowork/core-domain';
import { resetSearxPoolHealthForTests, SearXNGPoolProvider } from '../providers/searxng-pool.js';

const ctx: SearchContext = {
  hasNativeGrounding: false,
  hasByokSearchKey: false,
  query: 'test query',
  userId: 'test-user',
};

const instances = [
  { url: 'https://alpha.example/', label: 'alpha' },
  { url: 'https://beta.example/', label: 'beta' },
  { url: 'https://gamma.example/', label: 'gamma' },
];

function jsonResponse(results: Array<{ title: string; url: string; content: string }>) {
  return {
    ok: true,
    status: 200,
    headers: {
      get: (name: string) => (name.toLowerCase() === 'content-type' ? 'application/json' : null),
    },
    text: async () => JSON.stringify({ results }),
  };
}

describe('SearXNGPoolProvider', () => {
  beforeEach(() => {
    resetSearxPoolHealthForTests();
    vi.stubGlobal('fetch', vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    resetSearxPoolHealthForTests();
  });

  it('maps valid JSON search results', async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockResolvedValueOnce(
      jsonResponse([{ title: 'Result A', url: 'https://a.test', content: 'Snippet A' }]) as unknown as Response,
    );

    const provider = new SearXNGPoolProvider([instances[0]!]);
    const results = await provider.search('hello world');

    expect(results).toEqual([
      {
        title: 'Result A',
        url: 'https://a.test',
        snippet: 'Snippet A',
        score: 1,
        source: 'SearXNG',
      },
    ]);
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it('races two healthy instances and returns the first valid response', async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockImplementation(async (input) => {
      const url = String(input);
      if (url.startsWith('https://beta.example/')) {
        await new Promise((resolve) => setTimeout(resolve, 30));
        return jsonResponse([{ title: 'Beta', url: 'https://beta.test', content: 'beta' }]) as unknown as Response;
      }
      return jsonResponse([{ title: 'Alpha', url: 'https://alpha.test', content: 'alpha' }]) as unknown as Response;
    });

    const provider = new SearXNGPoolProvider(instances);
    const results = await provider.search('race query');

    expect(results[0]?.title).toBe('Alpha');
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('opens a 5 minute cooldown after three plain failures', async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockResolvedValue({
      ok: false,
      status: 500,
      headers: { get: () => 'text/plain' },
      text: async () => 'error',
    } as unknown as Response);

    const provider = new SearXNGPoolProvider([instances[0]!]);

    await provider.search('one');
    await provider.search('two');
    await provider.search('three');
    await provider.search('four');

    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(await provider.isAvailable(ctx)).toBe(false);
  });

  it('blocks an instance for 24 hours on captcha/HTML responses', async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => 'text/html; charset=utf-8' },
      text: async () => '<html>captcha challenge</html>',
    } as unknown as Response);

    const provider = new SearXNGPoolProvider([instances[0]!]);
    const first = await provider.search('blocked');
    const second = await provider.search('blocked again');

    expect(first).toEqual([]);
    expect(second).toEqual([]);
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(await provider.isAvailable(ctx)).toBe(false);
  });

  it('blocks an instance for 24 hours on HTTP 403', async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockResolvedValueOnce({
      ok: false,
      status: 403,
      headers: { get: () => 'text/plain' },
      text: async () => 'forbidden',
    } as unknown as Response);

    const provider = new SearXNGPoolProvider([instances[0]!]);
    await provider.search('forbidden');
    await provider.search('retry');

    expect(fetchMock).toHaveBeenCalledOnce();
    expect(await provider.isAvailable(ctx)).toBe(false);
  });

  it('honors Retry-After on HTTP 429', async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockResolvedValueOnce({
      ok: false,
      status: 429,
      headers: {
        get: (name: string) => (name.toLowerCase() === 'retry-after' ? '120' : 'text/plain'),
      },
      text: async () => 'too many requests',
    } as unknown as Response);

    const provider = new SearXNGPoolProvider([instances[0]!]);
    await provider.search('limited');
    await provider.search('limited again');

    expect(fetchMock).toHaveBeenCalledOnce();
    expect(await provider.isAvailable(ctx)).toBe(false);
  });
});