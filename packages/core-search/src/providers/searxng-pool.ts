import type { SearchContext, SearchProvider, SearchResult } from '@agentcowork/core-domain';
import poolData from '../data/searx-pool.json' with { type: 'json' };

const FAILURE_THRESHOLD = 3;
const COOLDOWN_MS = 5 * 60 * 1000;
const BLOCK_MS = 24 * 60 * 60 * 1000;
const RACE_COUNT = 2;
const REQUEST_TIMEOUT_MS = 12_000;

export interface SearxPoolInstance {
  url: string;
  label: string;
  uptimeMonth?: number | null;
  version?: string | null;
}

interface InstanceHealth {
  failures: number;
  cooldownUntil: number;
  blockedUntil: number;
}

interface SearxJsonResult {
  title?: string;
  url?: string;
  content?: string;
}

const instanceHealth = new Map<string, InstanceHealth>();

const HEALTH_STORAGE_KEY = 'searxng.instanceHealth.v1';

/** Persisted shape for AsyncStorage / SecureStore. */
type PersistedHealth = Record<string, InstanceHealth>;

/** Load health state from a key-value store (call once on app boot). */
export async function loadSearxPoolHealth(store: {
  getItem(key: string): Promise<string | null>;
}): Promise<void> {
  try {
    const raw = await store.getItem(HEALTH_STORAGE_KEY);
    if (!raw) return;
    const parsed = JSON.parse(raw) as PersistedHealth;
    for (const [url, health] of Object.entries(parsed)) {
      instanceHealth.set(url, health);
    }
  } catch {
    // Corrupted state — start fresh
  }
}

/** Save current health state to a key-value store (call after each mutation). */
async function saveSearxPoolHealth(store: {
  setItem(key: string, value: string): Promise<void>;
}): Promise<void> {
  try {
    const persisted: PersistedHealth = {};
    for (const [url, health] of instanceHealth.entries()) {
      persisted[url] = health;
    }
    await store.setItem(HEALTH_STORAGE_KEY, JSON.stringify(persisted));
  } catch (err) {
    console.warn('[searxng-pool] Failed to persist health state:', err);
  }
}

/** Optional store reference — set once from the mobile app to enable persistence. */
let persistenceStore: {
  getItem(key: string): Promise<string | null>;
  setItem(key: string, value: string): Promise<void>;
} | null = null;

/** Wire a persistence store (e.g. AsyncStorage) for circuit breaker state. */
export function setSearxPoolPersistenceStore(store: {
  getItem(key: string): Promise<string | null>;
  setItem(key: string, value: string): Promise<void>;
}): void {
  persistenceStore = store;
}

function getHealth(url: string): InstanceHealth {
  let health = instanceHealth.get(url);
  if (!health) {
    health = { failures: 0, cooldownUntil: 0, blockedUntil: 0 };
    instanceHealth.set(url, health);
  }
  return health;
}

function isInstanceHealthy(url: string, now = Date.now()): boolean {
  const health = getHealth(url);
  return now >= health.blockedUntil && now >= health.cooldownUntil;
}

function recordSuccess(url: string): void {
  const health = getHealth(url);
  health.failures = 0;
  health.cooldownUntil = 0;
  if (persistenceStore) {
    void saveSearxPoolHealth(persistenceStore);
  }
}

function recordFailure(url: string, blockMs = 0): void {
  const health = getHealth(url);
  health.failures += 1;
  if (blockMs > 0) {
    health.blockedUntil = Date.now() + blockMs;
    health.failures = 0;
    if (persistenceStore) {
      void saveSearxPoolHealth(persistenceStore);
    }
    return;
  }
  if (health.failures >= FAILURE_THRESHOLD) {
    health.cooldownUntil = Date.now() + COOLDOWN_MS;
    health.failures = 0;
  }
  if (persistenceStore) {
    void saveSearxPoolHealth(persistenceStore);
  }
}

function parseRetryAfterMs(header: string | null): number | null {
  if (!header) return null;
  const seconds = Number(header);
  if (Number.isFinite(seconds) && seconds > 0) {
    return seconds * 1000;
  }
  const date = Date.parse(header);
  if (Number.isFinite(date)) {
    const delta = date - Date.now();
    return delta > 0 ? delta : null;
  }
  return null;
}

function looksLikeCaptcha(body: string): boolean {
  const lower = body.toLowerCase();
  return (
    lower.includes('captcha') ||
    lower.includes('cf-challenge') ||
    lower.includes('challenge-platform') ||
    lower.includes('just a moment')
  );
}

function mapResults(raw: SearxJsonResult[]): SearchResult[] {
  return raw
    .filter((item) => Boolean(item.title && item.url))
    .map((item, index) => ({
      title: item.title ?? '',
      url: item.url ?? '',
      snippet: item.content ?? '',
      score: Math.max(0.1, 1 - index * 0.05),
      source: 'SearXNG',
    }));
}

async function queryInstance(baseUrl: string, query: string): Promise<SearchResult[]> {
  const normalized = baseUrl.endsWith('/') ? baseUrl : `${baseUrl}/`;
  const endpoint = new URL('search', normalized);
  endpoint.searchParams.set('q', query);
  endpoint.searchParams.set('format', 'json');

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);

  // Record exactly ONE failure per attempt — the explicit failure paths below
  // set blockMs (403/429/captcha → long blocks); the catch-all records a
  // plain failure only when nothing else already did (C.19 network errors).
  let failureRecorded = false;
  const record = (blockMs = 0): void => {
    if (failureRecorded) return;
    failureRecorded = true;
    recordFailure(baseUrl, blockMs);
  };

  try {
    const response = await fetch(endpoint.toString(), {
      signal: controller.signal,
      headers: {
        Accept: 'application/json',
        'User-Agent':
          'Mozilla/5.0 (Linux; Android 14; Mobile) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Mobile Safari/537.36',
      },
    });

    if (response.status === 403) {
      record(BLOCK_MS);
      throw new Error(`SearXNG ${baseUrl} blocked (403)`);
    }

    if (response.status === 429) {
      const retryMs = parseRetryAfterMs(response.headers.get('retry-after')) ?? COOLDOWN_MS;
      record(retryMs);
      throw new Error(`SearXNG ${baseUrl} rate limited (429)`);
    }

    const contentType = response.headers.get('content-type') ?? '';
    const bodyText = await response.text();

    if (!response.ok) {
      record();
      throw new Error(`SearXNG ${baseUrl} failed: HTTP ${response.status}`);
    }

    if (!contentType.includes('application/json') || looksLikeCaptcha(bodyText)) {
      record(BLOCK_MS);
      throw new Error(`SearXNG ${baseUrl} returned captcha/HTML`);
    }

    const payload = JSON.parse(bodyText) as { results?: SearxJsonResult[] };
    const results = mapResults(payload.results ?? []);
    if (results.length === 0) {
      record();
      return [];
    }

    recordSuccess(baseUrl);
    return results;
  } catch (error) {
    // C.19: record failure for ALL errors — network-level failures (DNS,
    // connection refused, fetch failed) were previously ignored, so dead
    // instances were raced on every single query. `record` is a no-op when
    // an explicit failure path above already recorded this attempt.
    record();
    throw error;
  } finally {
    clearTimeout(timer);
  }
}

export class SearXNGPoolProvider implements SearchProvider {
  name = 'SearXNG Pool';
  kind = 'search' as const;

  private instances: SearxPoolInstance[];

  constructor(instances: SearxPoolInstance[] = poolData.instances as SearxPoolInstance[]) {
    this.instances = instances;
  }

  async isAvailable(_ctx: SearchContext): Promise<boolean> {
    return this.getHealthyInstances().length > 0;
  }

  private getHealthyInstances(): SearxPoolInstance[] {
    return this.instances.filter((instance) => isInstanceHealthy(instance.url));
  }

  async search(query: string): Promise<SearchResult[]> {
    const healthy = this.getHealthyInstances();
    if (healthy.length === 0) {
      return [];
    }

    for (let offset = 0; offset < healthy.length; offset += RACE_COUNT) {
      const batch = healthy.slice(offset, offset + RACE_COUNT);
      const results = await raceHealthyInstances(batch, query);
      if (results.length > 0) {
        return results;
      }
    }

    return [];
  }
}

async function raceHealthyInstances(
  racers: SearxPoolInstance[],
  query: string,
): Promise<SearchResult[]> {
  if (racers.length === 0) {
    return [];
  }

  return new Promise((resolve) => {
    let pending = racers.length;
    let settled = false;

    for (const instance of racers) {
      queryInstance(instance.url, query)
        .then((results) => {
          if (!settled && results.length > 0) {
            settled = true;
            resolve(results);
          }
        })
        .catch((error) => {
          console.warn(`[SearXNGPoolProvider] ${instance.url} failed:`, error);
        })
        .finally(() => {
          pending -= 1;
          if (!settled && pending === 0) {
            resolve([]);
          }
        });
    }
  });
}

/** Test helper — resets in-memory circuit breaker state. */
export function resetSearxPoolHealthForTests(): void {
  instanceHealth.clear();
}