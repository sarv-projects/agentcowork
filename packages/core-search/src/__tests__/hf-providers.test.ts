import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SearchContext } from '@agentcowork/core-domain';
import { HfSearxngProvider } from '../providers/hf-searxng.js';
import { HfWhoogleProvider } from '../providers/hf-whoogle.js';
import { HfWebsurfxProvider } from '../providers/hf-websurfx.js';
import { HfSearchRotator, buildHfProviders, hfDeepResearch, type HfSearchConfig } from '../providers/hf-rotator.js';

const ctx: SearchContext = {
  hasNativeGrounding: false,
  hasByokSearchKey: false,
  query: 'test query',
  userId: 'test-user',
};

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: {
      get: (name: string) => (name.toLowerCase() === 'content-type' ? 'application/json' : null),
    },
    json: async () => body,
  } as unknown as Response;
}

function errorResponse(status: number): Response {
  return {
    ok: false,
    status,
    headers: { get: () => null },
    json: async () => ({}),
  } as unknown as Response;
}

// ---------------------------------------------------------------------------
// HfSearxngProvider
// ---------------------------------------------------------------------------

describe('HfSearxngProvider', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('maps valid JSON search results from SearXNG format', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(
      jsonResponse({
        results: [
          { title: 'SearXNG Result', url: 'https://searxng.test/1', content: 'First snippet' },
          { title: 'Second', url: 'https://searxng.test/2', content: 'Second snippet' },
        ],
      }),
    );

    const provider = new HfSearxngProvider('https://test-searxng.hf.space');
    const results = await provider.search('hello');

    expect(results).toHaveLength(2);
    expect(results[0]).toEqual({
      title: 'SearXNG Result',
      url: 'https://searxng.test/1',
      snippet: 'First snippet',
      score: 0.8,
      source: 'SearXNG',
    });
  });

  it('throws when no baseUrl provided', () => {
    expect(() => new HfSearxngProvider('')).toThrow('HfSearxngProvider requires a baseUrl');
  });

  it('isAvailable always returns true', async () => {
    const provider = new HfSearxngProvider('https://test.hf.space');
    expect(await provider.isAvailable(ctx)).toBe(true);
  });

  it('returns empty array for empty query', async () => {
    const provider = new HfSearxngProvider('https://test.hf.space');
    expect(await provider.search('')).toEqual([]);
    expect(await provider.search('   ')).toEqual([]);
  });

  it('throws on empty results from server', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonResponse({ results: [] }));

    const provider = new HfSearxngProvider('https://test.hf.space');
    await expect(provider.search('hello')).rejects.toThrow('empty results');
  });

  it('throws on HTTP error', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(errorResponse(502));

    const provider = new HfSearxngProvider('https://test.hf.space');
    await expect(provider.search('hello')).rejects.toThrow('HTTP 502');
  });

  it('normalizes protocol-relative URLs', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(
      jsonResponse({
        results: [{ title: 'Protocol Relative', url: '//example.com/page', content: 'test' }],
      }),
    );

    const provider = new HfSearxngProvider('https://test.hf.space');
    const results = await provider.search('hello');
    expect(results[0]!.url).toBe('https://example.com/page');
  });
});

// ---------------------------------------------------------------------------
// HfWhoogleProvider
// ---------------------------------------------------------------------------

describe('HfWhoogleProvider', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('maps Whoogle results using desc field for snippet', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(
      jsonResponse([
        { title: 'Whoogle Hit', url: 'https://whoogle.test/1', desc: 'Description here' },
      ]),
    );

    const provider = new HfWhoogleProvider('https://test-whoogle.hf.space');
    const results = await provider.search('hello');

    expect(results).toHaveLength(1);
    expect(results[0]!.snippet).toBe('Description here');
    expect(results[0]!.source).toBe('Whoogle');
  });

  it('falls back to snippet when desc is missing', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(
      jsonResponse([
        { title: 'Alt Format', url: 'https://test/1', snippet: 'Fallback snippet' },
      ]),
    );

    const provider = new HfWhoogleProvider('https://test-whoogle.hf.space');
    const results = await provider.search('hello');

    expect(results[0]!.snippet).toBe('Fallback snippet');
  });

  it('throws on empty results', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonResponse([]));

    const provider = new HfWhoogleProvider('https://test.hf.space');
    await expect(provider.search('hello')).rejects.toThrow('empty results');
  });
});

// ---------------------------------------------------------------------------
// HfWebsurfxProvider
// ---------------------------------------------------------------------------

describe('HfWebsurfxProvider', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('maps Websurfx results with description field', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(
      jsonResponse({
        results: [
          { title: 'Websurfx Result', url: 'https://wsfx.test/1', description: 'Rust-powered' },
        ],
      }),
    );

    const provider = new HfWebsurfxProvider('https://test-websurfx.hf.space');
    const results = await provider.search('hello');

    expect(results).toHaveLength(1);
    expect(results[0]!.source).toBe('Websurfx');
    expect(results[0]!.score).toBe(0.7);
  });

  it('throws on HTTP error', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(errorResponse(503));

    const provider = new HfWebsurfxProvider('https://test.hf.space');
    await expect(provider.search('hello')).rejects.toThrow('HTTP 503');
  });
});

// ---------------------------------------------------------------------------
// buildHfProviders
// ---------------------------------------------------------------------------

describe('buildHfProviders', () => {
  it('returns empty array when all URLs are empty', () => {
    const config: HfSearchConfig = { searxngUrl: '', whoogleUrl: '', websurfxUrl: '' };
    expect(buildHfProviders(config)).toHaveLength(0);
  });

  it('returns only providers with configured URLs', () => {
    const config: HfSearchConfig = {
      searxngUrl: 'https://test-searxng.hf.space',
      whoogleUrl: '',
      websurfxUrl: 'https://test-websurfx.hf.space',
    };
    const providers = buildHfProviders(config);
    expect(providers).toHaveLength(2);
    expect(providers[0]!.name).toBe('SearXNG (HF)');
    expect(providers[1]!.name).toBe('Websurfx (HF)');
  });

  it('returns all three when all configured', () => {
    const config: HfSearchConfig = {
      searxngUrl: 'https://s.hf.space',
      whoogleUrl: 'https://w.hf.space',
      websurfxUrl: 'https://x.hf.space',
    };
    expect(buildHfProviders(config)).toHaveLength(3);
  });
});

// ---------------------------------------------------------------------------
// HfSearchRotator
// ---------------------------------------------------------------------------

describe('HfSearchRotator', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('isAvailable returns true when providers configured', async () => {
    const config: HfSearchConfig = {
      searxngUrl: 'https://s.hf.space',
      whoogleUrl: '',
      websurfxUrl: '',
    };
    const rotator = new HfSearchRotator(config);
    expect(await rotator.isAvailable(ctx)).toBe(true);
    expect(rotator.providerCount).toBe(1);
  });

  it('isAvailable returns false when no providers configured', async () => {
    const config: HfSearchConfig = { searxngUrl: '', whoogleUrl: '', websurfxUrl: '' };
    const rotator = new HfSearchRotator(config);
    expect(await rotator.isAvailable(ctx)).toBe(false);
  });

  it('returns results from the first healthy provider', async () => {
    const randomSpy = vi.spyOn(Math, 'random').mockReturnValue(0.5);

    vi.mocked(fetch).mockResolvedValueOnce(
      jsonResponse({
        results: [{ title: 'SearXNG Hit', url: 'https://a.test', content: 'Good' }],
      }),
    );

    const config: HfSearchConfig = {
      searxngUrl: 'https://s.hf.space',
      whoogleUrl: 'https://w.hf.space',
      websurfxUrl: '',
    };
    const rotator = new HfSearchRotator(config);
    const results = await rotator.search('hello');

    randomSpy.mockRestore();

    expect(results).toHaveLength(1);
    expect(results[0]!.source).toBe('SearXNG');
  });

  it('falls through to second provider when first fails', async () => {
    const randomSpy = vi.spyOn(Math, 'random').mockReturnValue(0.5);

    vi.mocked(fetch)
      .mockRejectedValueOnce(new Error('SearXNG down'))
      .mockResolvedValueOnce(
        jsonResponse([{ title: 'Whoogle Saves', url: 'https://b.test', desc: 'Backup' }]),
      );

    const config: HfSearchConfig = {
      searxngUrl: 'https://s.hf.space',
      whoogleUrl: 'https://w.hf.space',
      websurfxUrl: '',
    };
    const rotator = new HfSearchRotator(config);
    const results = await rotator.search('hello');

    randomSpy.mockRestore();

    expect(results).toHaveLength(1);
    expect(results[0]!.source).toBe('Whoogle');
  });

  it('throws when all providers fail', async () => {
    vi.mocked(fetch)
      .mockRejectedValueOnce(new Error('SearXNG down'))
      .mockRejectedValueOnce(new Error('Whoogle down'));

    const config: HfSearchConfig = {
      searxngUrl: 'https://s.hf.space',
      whoogleUrl: 'https://w.hf.space',
      websurfxUrl: '',
    };
    const rotator = new HfSearchRotator(config);
    await expect(rotator.search('hello')).rejects.toThrow('All HF search engines failed');
  });

  it('throws when no providers configured', async () => {
    const config: HfSearchConfig = { searxngUrl: '', whoogleUrl: '', websurfxUrl: '' };
    const rotator = new HfSearchRotator(config);
    await expect(rotator.search('hello')).rejects.toThrow('No HF search providers configured');
  });
});

// ---------------------------------------------------------------------------
// hfDeepResearch — parallel gather + dedupe
// ---------------------------------------------------------------------------

describe('hfDeepResearch', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('queries all configured engines in parallel and deduplicates by URL', async () => {
    vi.mocked(fetch)
      // SearXNG returns 2 results
      .mockResolvedValueOnce(
        jsonResponse({
          results: [
            { title: 'Shared Result', url: 'https://shared.test/1', content: 'From SearXNG' },
            { title: 'SearXNG Only', url: 'https://sx.test/unique', content: 'Unique' },
          ],
        }),
      )
      // Whoogle returns 2 results, one duplicate
      .mockResolvedValueOnce(
        jsonResponse([
          { title: 'Shared Result (dup)', url: 'https://shared.test/1', desc: 'From Whoogle' },
          { title: 'Whoogle Only', url: 'https://wg.test/unique', desc: 'Unique too' },
        ]),
      )
      // Websurfx returns 1 result
      .mockResolvedValueOnce(
        jsonResponse({
          results: [
            { title: 'Websurfx Hit', url: 'https://wsfx.test/new', description: 'Third source' },
          ],
        }),
      );

    const config: HfSearchConfig = {
      searxngUrl: 'https://s.hf.space',
      whoogleUrl: 'https://w.hf.space',
      websurfxUrl: 'https://x.hf.space',
    };

    const results = await hfDeepResearch(config, 'multi engine test');

    // 5 total results minus 1 duplicate = 4 unique
    expect(results).toHaveLength(4);
    const urls = results.map((r) => r.url);
    expect(urls).toContain('https://shared.test/1');
    expect(urls).toContain('https://sx.test/unique');
    expect(urls).toContain('https://wg.test/unique');
    expect(urls).toContain('https://wsfx.test/new');

    // Duplicate URL appears only once
    const sharedHits = urls.filter((u) => u === 'https://shared.test/1');
    expect(sharedHits).toHaveLength(1);
  });

  it('returns partial results when one engine fails', async () => {
    vi.mocked(fetch)
      .mockRejectedValueOnce(new Error('SearXNG down'))
      .mockResolvedValueOnce(
        jsonResponse([{ title: 'Whoogle Works', url: 'https://wg.test/1', desc: 'Alive' }]),
      )
      .mockResolvedValueOnce(jsonResponse({ results: [] }));

    const config: HfSearchConfig = {
      searxngUrl: 'https://s.hf.space',
      whoogleUrl: 'https://w.hf.space',
      websurfxUrl: 'https://x.hf.space',
    };

    const results = await hfDeepResearch(config, 'partial failure test');
    expect(results).toHaveLength(1);
    expect(results[0]!.url).toBe('https://wg.test/1');
  });

  it('returns empty when no providers configured', async () => {
    const config: HfSearchConfig = { searxngUrl: '', whoogleUrl: '', websurfxUrl: '' };
    const results = await hfDeepResearch(config, 'nothing configured');
    expect(results).toEqual([]);
  });

  it('returns empty for empty query', async () => {
    const config: HfSearchConfig = { searxngUrl: 'https://s.hf.space', whoogleUrl: '', websurfxUrl: '' };
    expect(await hfDeepResearch(config, '')).toEqual([]);
    expect(await hfDeepResearch(config, '   ')).toEqual([]);
  });
});
