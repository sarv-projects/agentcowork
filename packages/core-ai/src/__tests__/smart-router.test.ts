import { describe, expect, it } from 'vitest';
import type { RouteContext } from '@agentcowork/core-domain';
import { HeuristicIntentClassifier, SmartRouter } from '../router/index.js';

const baseCtx: RouteContext = {
  hasByokKey: false,
  hasInternet: true,
  tier: 'free',
  activeConnectors: [],
};

const withKey: RouteContext = { ...baseCtx, hasByokKey: true };

describe('SmartRouter', () => {
  const router = new SmartRouter({ classifier: new HeuristicIntentClassifier() });

  it('routes weather queries to needs-web and managed free pool when no key', async () => {
    const decision = await router.route({ text: 'what is the weather today?' }, baseCtx);
    expect(decision.intent.category).toBe('needs-web');
    expect(decision.handler).toBe('MANAGED_FREE');
    expect(decision.reason).toContain('managed');
  });

  it('routes file questions to needs-files with retrieval plan', async () => {
    const decision = await router.route({ text: 'summarize my lease pdf' }, withKey);
    expect(decision.intent.category).toBe('needs-files');
    expect(decision.handler).toBe('BYOK');
    expect(decision.retrievalPlan?.sources).toContain('fts5');
  });

  it('routes greetings to conversational managed free pool', async () => {
    const decision = await router.route({ text: 'hello' }, baseCtx);
    expect(decision.intent.category).toBe('conversational');
    expect(decision.handler).toBe('MANAGED_FREE');
    expect(decision.reason).toContain('managed');
  });

  it('routes reminders to automation draft', async () => {
    const decision = await router.route({ text: 'remind me every morning at 8' }, baseCtx);
    expect(decision.intent.category).toBe('needs-automation');
    expect(decision.handler).toBe('AUTOMATION_DRAFT');
  });

  it('returns OFFLINE when offline even for needs-web', async () => {
    const decision = await router.route(
      { text: 'what is the weather today?' },
      { ...baseCtx, hasInternet: false },
    );
    expect(decision.handler).toBe('OFFLINE');
    expect(decision.reason).toContain('offline');
  });

  it('returns explainable reason on every decision', async () => {
    const decision = await router.route({ text: 'hi there' }, baseCtx);
    expect(decision.reason.length).toBeGreaterThan(10);
    expect(decision.intent.confidence).toBeGreaterThan(0);
  });
});