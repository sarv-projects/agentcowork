import { describe, expect, it } from 'vitest';
import type { RouteContext, RouteDecision } from '@agentcowork/core-domain';
import {
  HeuristicIntentClassifier,
  SmartRouter,
  applySizeAwareRouting,
  classificationPrefix,
  createChatIntentAnchor,
  evaluatePromptGuard,
  previewInputGuard,
} from '../router/index.js';
import { resolveHandler } from '../router/handler-mapper.js';

const baseCtx: RouteContext = {
  hasByokKey: false,
  hasInternet: true,
  tier: 'free',
  activeConnectors: [],
};

describe('prompt guard', () => {
  it('uses only the first 250 chars for classification prefix', () => {
    const long = `${'a'.repeat(300)} summarize my lease pdf`;
    expect(classificationPrefix(long)).toHaveLength(251);
    expect(createChatIntentAnchor(long)).toBe(`${'a'.repeat(250)}…`);
  });

  it('anchors follow-up routing to the opening message', async () => {
    const router = new SmartRouter({ classifier: new HeuristicIntentClassifier() });
    const anchor = createChatIntentAnchor('summarize my lease pdf please');
    const decision = await router.route(
      { text: 'yes go ahead' },
      { ...baseCtx, chatIntentAnchor: anchor },
    );
    expect(decision.intent.category).toBe('needs-files');
    expect(decision.handler).toBe('MANAGED_FREE');
  });

  it('blocks managed pool for inputs over 2000 chars', () => {
    const text = 'x'.repeat(2100);
    const preview = previewInputGuard(text);
    expect(preview.level).toBe('block');
    const intent = { category: 'conversational' as const, confidence: 0.65, depth: 'standard' as const };
    const base = resolveHandler(intent, { text }, baseCtx);
    const sized = applySizeAwareRouting({ text }, baseCtx, intent, base);
    expect(sized.handler).toBe('PROMPT_BYOK');
  });

  it('routes large paste away from managed pool at 1000 chars', () => {
    const text = 'y'.repeat(1200);
    const preview = previewInputGuard(text);
    expect(preview.level).toBe('warn');
    const intent = { category: 'conversational' as const, confidence: 0.65, depth: 'standard' as const };
    const base = resolveHandler(intent, { text }, baseCtx);
    const sized = applySizeAwareRouting({ text }, baseCtx, intent, base);
    expect(sized.handler).toBe('PROMPT_BYOK');
  });

  it('shows file-first notice for large PROMPT_BYOK inputs', () => {
    const text = 'z'.repeat(1500);
    const decision: RouteDecision = {
      handler: 'PROMPT_BYOK',
      intent: { category: 'needs-files', confidence: 0.9, depth: 'standard' },
      reason: 'test',
    };
    const guard = evaluatePromptGuard({ text }, baseCtx, decision);
    expect(guard.action).toBe('file_first');
    expect(guard.notice).toContain('Library');
  });

  it('allows open-document scope even when long', () => {
    const text = 'q'.repeat(2500);
    const ctx = { ...baseCtx, openDocumentId: 'doc-1' };
    const preview = previewInputGuard(text, ctx);
    expect(preview.level).toBe('warn');
    const intent = { category: 'needs-files' as const, confidence: 0.88, depth: 'standard' as const };
    const base = resolveHandler(intent, { text, scope: 'open-document' }, ctx);
    const sized = applySizeAwareRouting({ text, scope: 'open-document' }, ctx, intent, base);
    expect(sized.handler).toBe('MANAGED_FREE');
    const guard = evaluatePromptGuard({ text, scope: 'open-document' }, ctx, {
      handler: 'MANAGED_FREE',
      intent,
      reason: 'scoped',
    });
    expect(guard.action).toBe('allow');
  });
});