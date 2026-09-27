import { describe, expect, it } from 'vitest';
import {
  buildMemoryRetrievalHint,
  attachMemoryScopeToPlan,
} from '../router-integration.js';
import type { IntentCategory, RetrievalPlan, RouteContext, UserQuery } from '@agentcowork/core-domain';

describe('buildMemoryRetrievalHint', () => {
  it('returns books category and sourceId when openDocumentId is present in context', () => {
    const hint = buildMemoryRetrievalHint(
      'conversational' as IntentCategory,
      { text: 'summarize chapter 3' } as UserQuery,
      { openDocumentId: 'doc-123' } as RouteContext,
    );

    expect(hint.categories).toContain('books');
    expect(hint.sourceId).toBe('doc-123');
    expect(hint.limit).toBe(8);
  });

  it('returns categories based on intent category without openDocumentId, no sourceId, limit = 8', () => {
    const hint = buildMemoryRetrievalHint(
      'needs-web' as IntentCategory,
      { text: 'what is the weather' } as UserQuery,
      {} as RouteContext,
    );

    // needs-web falls to default → ['personal']
    expect(hint.categories).toEqual(['personal']);
    expect(hint.sourceId).toBeUndefined();
    expect(hint.limit).toBe(8);
  });

  it('returns books, work, projects for needs-files intent without openDocumentId', () => {
    const hint = buildMemoryRetrievalHint(
      'needs-files' as IntentCategory,
      { text: 'find my documents' } as UserQuery,
      {} as RouteContext,
    );

    expect(hint.categories).toEqual(['books', 'work', 'projects']);
  });

  it('delegates to inferMemoryCategoriesFromQuery for conversational intent', () => {
    const hint = buildMemoryRetrievalHint(
      'conversational' as IntentCategory,
      { text: 'project meeting at work' } as UserQuery,
      {} as RouteContext,
    );

    // query matches work/projects pattern
    expect(hint.categories).toContain('work');
    expect(hint.categories).toContain('projects');
    expect(hint.categories).not.toContain('books');
  });

  it('delegates to inferMemoryCategoriesFromQuery for needs-connector intent', () => {
    const hint = buildMemoryRetrievalHint(
      'needs-connector' as IntentCategory,
      { text: 'tax payment reminder' } as UserQuery,
      {} as RouteContext,
    );

    // query matches finance pattern
    expect(hint.categories).toContain('finance');
  });
});

describe('attachMemoryScopeToPlan', () => {
  const basePlan: RetrievalPlan = {
    sources: ['fts5', 'vector'],
    query: 'test query',
    maxResults: 5,
  };

  it('adds memoryCategories to the plan', () => {
    const result = attachMemoryScopeToPlan(basePlan, {
      categories: ['books', 'work'],
      limit: 8,
    });

    expect(result.memoryCategories).toEqual(['books', 'work']);
  });

  it('adds memorySourceId when hint has sourceId', () => {
    const result = attachMemoryScopeToPlan(basePlan, {
      categories: ['books'],
      sourceId: 'doc-456',
      limit: 8,
    });

    expect(result.memorySourceId).toBe('doc-456');
  });

  it('sets maxResults to max(plan.maxResults, hint.limit)', () => {
    // plan.maxResults=5, hint.limit=8 → max=8
    const result1 = attachMemoryScopeToPlan(basePlan, {
      categories: [],
      limit: 8,
    });
    expect(result1.maxResults).toBe(8);

    // plan.maxResults=10, hint.limit=8 → max=10
    const planWithHigherMax: RetrievalPlan = {
      sources: ['fts5'],
      query: 'test',
      maxResults: 10,
    };
    const result2 = attachMemoryScopeToPlan(planWithHigherMax, {
      categories: [],
      limit: 8,
    });
    expect(result2.maxResults).toBe(10);
  });

  it('does not add memorySourceId when hint has no sourceId', () => {
    const result = attachMemoryScopeToPlan(basePlan, {
      categories: ['personal'],
      limit: 8,
    });

    expect(result.memorySourceId).toBeUndefined();
  });

  it('preserves all existing plan fields', () => {
    const plan: RetrievalPlan = {
      sources: ['fts5', 'vector', 'memory'],
      query: 'find my documents',
      maxResults: 3,
      scopeDocumentId: 'scope-999',
    };

    const result = attachMemoryScopeToPlan(plan, {
      categories: ['work'],
      limit: 8,
    });

    expect(result.sources).toEqual(['fts5', 'vector', 'memory']);
    expect(result.query).toBe('find my documents');
    expect(result.scopeDocumentId).toBe('scope-999');
  });
});
