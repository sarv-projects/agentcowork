import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SearchContext } from '@agentcowork/core-domain';
import {
  DuckDuckGoSearchProvider,
  parseDuckDuckGoLiteHtml,
} from '../providers/duckduckgo-search.js';

const ctx: SearchContext = {
  hasNativeGrounding: false,
  hasByokSearchKey: false,
  query: 'android privacy',
  userId: 'test-user',
};

const SAMPLE_HTML = `<!DOCTYPE html><html><body>
<table>
<tr><td><a rel="nofollow" href="https://example.com/a" class='result-link'>Result Alpha</a></td></tr>
<tr><td class='result-snippet'>Alpha snippet about privacy.</td></tr>
<tr><td><a rel="nofollow" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fb" class='result-link'>Result Beta</a></td></tr>
<tr><td class='result-snippet'>Beta snippet text.</td></tr>
</table>
</body></html>`;

describe('parseDuckDuckGoLiteHtml', () => {
  it('maps result-link rows and snippets into SearchResult objects', () => {
    const results = parseDuckDuckGoLiteHtml(SAMPLE_HTML);
    expect(results).toHaveLength(2);
    expect(results[0]).toMatchObject({
      title: 'Result Alpha',
      url: 'https://example.com/a',
      snippet: 'Alpha snippet about privacy.',
      source: 'DuckDuckGo',
    });
    expect(results[1]?.url).toBe('https://example.com/b');
  });
});

describe('DuckDuckGoSearchProvider', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('is always available on free tier', async () => {
    const provider = new DuckDuckGoSearchProvider();
    await expect(provider.isAvailable(ctx)).resolves.toBe(true);
  });

  it('posts to DuckDuckGo Lite and parses HTML results', async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      text: async () => SAMPLE_HTML,
    } as Response);

    const provider = new DuckDuckGoSearchProvider();
    const results = await provider.search('android privacy');

    expect(results[0]?.title).toBe('Result Alpha');
    expect(fetchMock).toHaveBeenCalledOnce();
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(String(url)).toBe('https://lite.duckduckgo.com/lite/');
    expect(init?.method).toBe('POST');
    expect(init?.body).toBe('q=android+privacy');
  });

  it('throws when HTML has no results', async () => {
    vi.mocked(fetch).mockResolvedValueOnce({
      ok: true,
      status: 200,
      text: async () => '<html><body>No results</body></html>',
    } as Response);

    const provider = new DuckDuckGoSearchProvider();
    await expect(provider.search('empty query')).rejects.toThrow(/no parseable results/i);
  });
});