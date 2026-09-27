import { describe, expect, it } from 'vitest';
import type { RouteContext } from '@agentcowork/core-domain';
import { resolveHandler } from '../router/handler-mapper.js';

const baseCtx: RouteContext = {
  hasByokKey: false,
  hasInternet: true,
  tier: 'free',
  activeConnectors: [],
};

const withKey: RouteContext = { ...baseCtx, hasByokKey: true };
const offline: RouteContext = { ...baseCtx, hasInternet: false };
const lowBattery: RouteContext = { ...baseCtx, batteryLevel: 0.1 };

describe('resolveHandler', () => {
  it('conversational + base → MANAGED_FREE', () => {
    const result = resolveHandler(
      { category: 'conversational', confidence: 0.75, depth: 'standard' },
      { text: 'hello' },
      baseCtx,
    );
    expect(result.handler).toBe('MANAGED_FREE');
    expect(result.reason).toContain('managed');
  });

  it('out-of-scope + base → MANAGED_FREE', () => {
    const result = resolveHandler(
      { category: 'out-of-scope', confidence: 0.75, depth: 'standard' },
      { text: 'not relevant' },
      baseCtx,
    );
    expect(result.handler).toBe('MANAGED_FREE');
    expect(result.reason).toContain('out-of-scope');
  });

  it('out-of-scope + offline → OFFLINE', () => {
    const result = resolveHandler(
      { category: 'out-of-scope', confidence: 0.75, depth: 'standard' },
      { text: 'not relevant' },
      offline,
    );
    expect(result.handler).toBe('OFFLINE');
  });

  it('needs-web + base (no key) → MANAGED_FREE', () => {
    const result = resolveHandler(
      { category: 'needs-web', confidence: 0.75, depth: 'standard' },
      { text: 'weather today' },
      baseCtx,
    );
    expect(result.handler).toBe('MANAGED_FREE');
    expect(result.reason).toContain('managed');
  });

  it('needs-web + withKey → BYOK', () => {
    const result = resolveHandler(
      { category: 'needs-web', confidence: 0.75, depth: 'standard' },
      { text: 'weather today' },
      withKey,
    );
    expect(result.handler).toBe('BYOK');
    expect(result.reason).toContain('route');
  });

  it('needs-automation + base → AUTOMATION_DRAFT', () => {
    const result = resolveHandler(
      { category: 'needs-automation', confidence: 0.75, depth: 'standard' },
      { text: 'remind me every morning' },
      baseCtx,
    );
    expect(result.handler).toBe('AUTOMATION_DRAFT');
    expect(result.reason).toContain('automation');
  });

  it('needs-automation + offline → AUTOMATION_DRAFT', () => {
    const result = resolveHandler(
      { category: 'needs-automation', confidence: 0.75, depth: 'standard' },
      { text: 'remind me' },
      offline,
    );
    expect(result.handler).toBe('AUTOMATION_DRAFT');
    expect(result.reason).toContain('offline');
  });

  it('needs-files + base (no key) → MANAGED_FREE', () => {
    const result = resolveHandler(
      { category: 'needs-files', confidence: 0.75, depth: 'standard' },
      { text: 'find my lease' },
      baseCtx,
    );
    expect(result.handler).toBe('MANAGED_FREE');
  });

  it('needs-files + withKey → BYOK', () => {
    const result = resolveHandler(
      { category: 'needs-files', confidence: 0.75, depth: 'standard' },
      { text: 'summarize my lease' },
      withKey,
    );
    expect(result.handler).toBe('BYOK');
  });

  it('conversational + offline → OFFLINE', () => {
    const result = resolveHandler(
      { category: 'conversational', confidence: 0.75, depth: 'standard' },
      { text: 'hello' },
      offline,
    );
    expect(result.handler).toBe('OFFLINE');
    expect(result.reason).toContain('offline');
  });

  it('needs-web + lowBattery → MANAGED_FREE', () => {
    const result = resolveHandler(
      { category: 'needs-web', confidence: 0.75, depth: 'standard' },
      { text: 'weather' },
      lowBattery,
    );
    expect(result.handler).toBe('MANAGED_FREE');
  });

  it('needs-files + withKey → retrievalPlan has fts5 and vector', () => {
    const result = resolveHandler(
      { category: 'needs-files', confidence: 0.75, depth: 'standard' },
      { text: 'analyze my document' },
      withKey,
    );
    expect(result.handler).toBe('BYOK');
    expect(result.retrievalPlan).toBeDefined();
    expect(result.retrievalPlan!.sources).toContain('fts5');
    expect(result.retrievalPlan!.sources).toContain('vector');
  });
});