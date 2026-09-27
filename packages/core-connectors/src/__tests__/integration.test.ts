/**
 * Integration tests for real connector adapters.
 * These tests call public APIs (Open-Meteo, GitHub, RSS) and require
 * network access. Skipped by default; run with `--run` to execute.
 */
import { describe, it, expect } from 'vitest';

// Integration tests require network. Run explicitly:
//   RUN_CONNECTOR_INTEGRATION=1 pnpm --filter @agentcowork/core-connectors test
const runIntegration = process.env.RUN_CONNECTOR_INTEGRATION === '1';
const describeMaybe = runIntegration ? describe : describe.skip;

describeMaybe('WeatherAdapter integration', () => {
  it('fetches weather data from Open-Meteo API', async () => {
    const { WeatherAdapter } = await import('../adapters/weather-adapter');
    const a = new WeatherAdapter();
    const result = await a.fetch({
      userId: 'test',
      query: { text: 'weather in London' },
      filter: { location: 'London', days: 1 },
    });
    expect(result.totalCount).toBeGreaterThan(0);
    expect(result.items[0]!.title).toContain('London');
    expect(result.items[0]!.snippet).toBeTruthy();
  });
});

describeMaybe('GitHubAdapter integration', () => {
  it('fetches public repo data from GitHub API', async () => {
    const { GitHubAdapter } = await import('../adapters/github-adapter');
    const a = new GitHubAdapter();
    const result = await a.fetch({
      userId: 'test',
      query: { text: 'torvalds/linux' },
      filter: { query: 'torvalds/linux', limit: 3 },
    });
    expect(result.totalCount).toBeGreaterThan(0);
    expect(result.items[0]!.title).toContain('linux');
  });
});

describeMaybe('RssAdapter integration', () => {
  it('fetches RSS feed from a known URL', async () => {
    const { RssAdapter } = await import('../adapters/rss-adapter');
    const a = new RssAdapter();
    const result = await a.fetch({
      userId: 'test',
      query: { text: 'tech news' },
      filter: { url: 'https://hnrss.org/frontpage', max: 3 },
    });
    expect(result.totalCount).toBeGreaterThan(0);
    expect(result.items[0]!.title).toBeTruthy();
  });
});
