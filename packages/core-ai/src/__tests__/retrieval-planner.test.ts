import { describe, expect, it, vi } from 'vitest';
import type { RouteContext } from '@agentcowork/core-domain';

vi.mock('@agentcowork/core-memory', () => ({
  buildMemoryRetrievalHint: vi.fn(() => ({ topicHints: [] })),
  attachMemoryScopeToPlan: vi.fn(
    (plan: Record<string, unknown>, _hint: unknown) => ({
      ...plan,
      memoryCategories: ['test'],
    }),
  ),
}));

import { buildRetrievalPlan } from '../router/retrieval-planner.js';

const baseCtx: RouteContext = {
  hasByokKey: false,
  hasInternet: true,
  tier: 'free',
  activeConnectors: ['gmail', 'slack'],
};

describe('buildRetrievalPlan', () => {
  it('needs-files → sources include fts5 and vector, maxResults=8', () => {
    const plan = buildRetrievalPlan('needs-files', { text: 'find my lease' }, baseCtx);
    expect(plan).toBeDefined();
    expect(plan!.sources).toContain('fts5');
    expect(plan!.sources).toContain('vector');
    expect(plan!.maxResults).toBe(8);
  });

  it('needs-files + scope=memory → sources include memory', () => {
    const plan = buildRetrievalPlan(
      'needs-files',
      { text: 'remember this', scope: 'memory' },
      baseCtx,
    );
    expect(plan).toBeDefined();
    expect(plan!.sources).toContain('memory');
    expect(plan!.sources).toContain('fts5');
    expect(plan!.sources).toContain('vector');
  });

  it('needs-files + scope=web → sources include web', () => {
    const plan = buildRetrievalPlan(
      'needs-files',
      { text: 'web search', scope: 'web' },
      baseCtx,
    );
    expect(plan).toBeDefined();
    expect(plan!.sources).toContain('web');
    expect(plan!.sources).toContain('fts5');
    expect(plan!.sources).toContain('vector');
  });

  it('needs-files + openDocumentId → scopeDocumentId set', () => {
    const ctx = { ...baseCtx, openDocumentId: 'doc-456' };
    const plan = buildRetrievalPlan('needs-files', { text: 'my doc' }, ctx);
    expect(plan).toBeDefined();
    expect(plan!.sources).toContain('fts5');
    expect(plan!.sources).toContain('vector');
  });

  it('needs-files + scope=all-files → memoryCategories attached via attachMemoryScopeToPlan', () => {
    const plan = buildRetrievalPlan(
      'needs-files',
      { text: 'all files please', scope: 'all-files' },
      baseCtx,
    );
    expect(plan).toBeDefined();
    expect(plan!.memoryCategories).toEqual(['test']);
  });

  it('needs-web → sources=[web], maxResults=6', () => {
    const plan = buildRetrievalPlan('needs-web', { text: 'weather' }, baseCtx);
    expect(plan).toBeDefined();
    expect(plan!.sources).toEqual(['web']);
    expect(plan!.maxResults).toBe(6);
  });

  it('needs-connector → sources include connector and memory, connectorFilters has active', () => {
    const plan = buildRetrievalPlan(
      'needs-connector',
      { text: 'check my email' },
      baseCtx,
    );
    expect(plan).toBeDefined();
    expect(plan!.sources).toContain('connector');
    expect(plan!.sources).toContain('memory');
    expect(plan!.maxResults).toBe(5);
    expect(plan!.connectorFilters).toEqual({ active: ['gmail', 'slack'] });
  });

  it('conversational → undefined (no scope)', () => {
    const plan = buildRetrievalPlan('conversational', { text: 'hello' }, baseCtx);
    expect(plan).toBeUndefined();
  });

  it('conversational + scope=memory → sources=[memory], maxResults=8', () => {
    const plan = buildRetrievalPlan(
      'conversational',
      { text: 'what did we do', scope: 'memory' },
      baseCtx,
    );
    expect(plan).toBeDefined();
    expect(plan!.sources).toEqual(['memory']);
    expect(plan!.maxResults).toBe(8);
  });

  it('out-of-scope → undefined', () => {
    const plan = buildRetrievalPlan('out-of-scope', { text: 'nope' }, baseCtx);
    expect(plan).toBeUndefined();
  });

  it('needs-automation → undefined', () => {
    const plan = buildRetrievalPlan('needs-automation', { text: 'remind me' }, baseCtx);
    expect(plan).toBeUndefined();
  });
});
