import type {
  ConnectorAdapter,
  ConnectorContext,
  ConnectorFilter,
  ConnectorName,
  ConnectorResult,
  UserQuery,
  MemoryFact,
} from '@agentcowork/core-domain';
import { hasConnectorHostTransport } from './connection-manager.js';

/**
 * ConnectorOrchestrator per spec §12.2 and blueprint §15.1.
 * This is the reasoning loop extracted so router stays thin.
 *
 * plan() -> decide which connectors + shape
 * execute() -> fetch + (minimal) stage
 * writeBack() -> memory + provenance (stub for now, since memory write lives in core-memory)
 */

export interface ConnectorPlan {
  adapters: ConnectorAdapter[];
  shape: 'single' | 'parallel' | 'sequential';
  filters: Record<string, ConnectorFilter>;
}

/** Marker implemented by adapters whose authenticated HTTP path is host-owned. */
export interface HostMediatedConnectorAdapter extends ConnectorAdapter {
  readonly credentialMode: 'host-mediated';
}

export type ConnectorExecutionStatus = 'completed' | 'unavailable';

export interface ConnectorExecutionResult {
  source: string;
  result: ConnectorResult;
  compressedSnippet?: string;
  status: ConnectorExecutionStatus;
  /** Stable, non-secret explanation for a disabled connector path. */
  reason?: string;
}

function isHostMediated(adapter: ConnectorAdapter): adapter is HostMediatedConnectorAdapter {
  return (adapter as Partial<HostMediatedConnectorAdapter>).credentialMode === 'host-mediated';
}

const CREDENTIAL_FILTER_KEYS = new Set([
  'accesstoken',
  'accesskey',
  'apikey',
  'authorization',
  'botoken',
  'cookie',
  'key',
  'password',
  'secret',
  'token',
]);

function normalizedCredentialKey(key: string): string {
  return key.toLowerCase().replace(/[^a-z0-9]/g, '');
}

function withoutCredentialFields(filter: ConnectorFilter): ConnectorFilter {
  const safe: ConnectorFilter = {};
  for (const [key, value] of Object.entries(filter)) {
    const normalized = normalizedCredentialKey(key);
    if (
      CREDENTIAL_FILTER_KEYS.has(normalized) ||
      normalized.includes('apikey') ||
      normalized.includes('accesstoken') ||
      normalized.includes('password') ||
      normalized.includes('secret') ||
      normalized.includes('token')
    ) {
      continue;
    }
    safe[key] = value;
  }
  return safe;
}

function emptyResult(source: ConnectorName): ConnectorResult {
  return { items: [], totalCount: 0, source };
}

export class ConnectorOrchestrator {
  private adapters: Map<string, ConnectorAdapter> = new Map();

  register(adapter: ConnectorAdapter) {
    this.adapters.set(adapter.name, adapter);
  }

  list(): ConnectorAdapter[] {
    return Array.from(this.adapters.values());
  }

  /**
   * plan: enumerate active (authorized) + score > threshold.
   * For v1 we do simple parallel for multiple.
   */
  async plan(query: UserQuery, memory: MemoryFact[], activeNames?: string[]): Promise<ConnectorPlan> {
    const candidates: ConnectorAdapter[] = [];

    for (const adapter of this.adapters.values()) {
      if (activeNames && activeNames.length > 0 && !activeNames.includes(adapter.name)) continue;

      const authorized = await adapter.isAuthorized('current-user'); // userId resolution happens in caller
      if (!authorized) continue;

      const score = adapter.scoreRelevance(query, memory);
      if (score > 0.3) {
        candidates.push(adapter);
      }
    }

    // Sort by score desc
    candidates.sort((a, b) => b.scoreRelevance(query, memory) - a.scoreRelevance(query, memory));

    const shape = candidates.length > 1 ? 'parallel' : 'single';

    const filters: Record<string, ConnectorFilter> = {};
    for (const a of candidates) {
      filters[a.name] = a.buildFilter(query);
    }

    return { adapters: candidates, shape, filters };
  }

  /**
   * Execute the planned adapters.
   *
   * Credential-bearing adapters never receive secret fields. They are routed
   * through the Rust connector transport; when that transport is not attached
   * they return an explicit unavailable result instead of a raw-token fallback.
   */
  async execute(
    plan: ConnectorPlan,
    baseContext: Partial<ConnectorContext>,
  ): Promise<ConnectorExecutionResult[]> {
    const results: ConnectorExecutionResult[] = [];

    const runOne = async (adapter: ConnectorAdapter) => {
      if (isHostMediated(adapter) && !hasConnectorHostTransport()) {
        results.push({
          source: adapter.name,
          result: emptyResult(adapter.name),
          status: 'unavailable',
          reason: 'Rust connector host transport is not attached',
        });
        return;
      }

      const ctx: ConnectorContext = {
        userId: baseContext.userId || 'local',
        query: baseContext.query || { text: '' },
        filter: withoutCredentialFields(plan.filters[adapter.name] || {}),
        ...(baseContext.signal ? { signal: baseContext.signal } : {}),
      };

      const result = await adapter.fetch(ctx);
      // very light "compression" for now (already done in adapter usually)
      const compressed = result.items.map((i) => `${i.title}: ${i.snippet}`).join(' | ').slice(0, 1200);

      results.push({
        source: adapter.name,
        result,
        compressedSnippet: compressed,
        status: 'completed',
      });
    };

    if (plan.shape === 'parallel') {
      await Promise.all(plan.adapters.map(runOne));
    } else {
      for (const a of plan.adapters) {
        await runOne(a);
      }
    }

    return results;
  }

  /**
   * writeBack: apply memory rules + provenance.
   * Returns candidate facts. If `persistFn` is provided, each fact is persisted
   * and the returned array contains only successfully persisted facts.
   */
  async writeBack(
    executionResults: ConnectorExecutionResult[],
    persistFn?: (fact: { content: string; category: string; source: string }) => Promise<boolean> | boolean,
  ): Promise<Array<{ content: string; category: string; source: string }>> {
    const facts: Array<{ content: string; category: string; source: string }> = [];

    for (const ex of executionResults) {
      for (const item of ex.result.items) {
        if (item.snippet && item.snippet.length > 20) {
          const fact = {
            content: `${ex.source}: ${item.title} — ${item.snippet}`,
            category: this.classifyConnectorCategory(ex.source, item),
            source: ex.source,
          };

          if (persistFn) {
            try {
              const ok = await persistFn(fact);
              if (ok) facts.push(fact);
            } catch {
              // Skip facts that fail persistence; caller can observe via side effect.
            }
          } else {
            facts.push(fact);
          }
        }
      }
    }
    return facts;
  }

  /** Map connector name/source to a canonical memory category. */
  private classifyConnectorCategory(
    source: string,
    item: { title?: string; snippet?: string },
  ): string {
    const lowerSource = source.toLowerCase();
    if (lowerSource.includes('weather')) return 'other';
    if (lowerSource.includes('github')) return 'work';
    if (lowerSource.includes('notion') || lowerSource.includes('dropbox') || lowerSource.includes('drive')) {
      return 'work';
    }
    if (lowerSource.includes('telegram') || lowerSource.includes('rss')) return 'personal';
    const text = `${item.title ?? ''} ${item.snippet ?? ''}`.toLowerCase();
    if (text.includes('meeting') || text.includes('project') || text.includes('work')) return 'work';
    if (text.includes('health') || text.includes('medical')) return 'health';
    if (text.includes('finance') || text.includes('money') || text.includes('bill')) return 'finance';
    if (text.includes('book') || text.includes('read')) return 'books';
    return 'other';
  }
}
