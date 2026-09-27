import type { RouteContext, RouteDecision, UserQuery } from '@agentcowork/core-domain';
import type { SmartRouterOptions } from './types.js';
import { resolveHandler } from './handler-mapper.js';
import { applySizeAwareRouting, buildClassificationText } from './prompt-guard.js';
import { preRoute } from './pre-router.js';
import { getAffinityTracker } from './affinity-tracker.js';

export class SmartRouter {
  private readonly classifier: SmartRouterOptions['classifier'];

  constructor(options: SmartRouterOptions) {
    this.classifier = options.classifier;
  }

  async route(query: UserQuery, ctx: RouteContext): Promise<RouteDecision> {
    const affinityTracker = getAffinityTracker();

    // Deterministic pre-router: handles privacy, vision, user choice, cache affinity
    const affinity = affinityTracker.get((ctx as unknown as { conversationId?: string }).conversationId ?? '');
    const preDecision = preRoute(
      {
        requestId: '',
        conversationId: '',
        turnId: '',
        privacyMode: ctx.tier === 'free' && !ctx.hasByokKey ? 'managed' : ctx.hasByokKey ? 'byok' : 'managed',
        modelMode: (ctx.modelMode as 'fast' | 'smart' | 'user_selected') ?? 'fast',
        task: { kind: 'chat', depth: 'standard', outputFormat: 'prose', language: 'en', style: 'straight_shooter' },
        scope: { mode: 'none', allowedSourceIds: [], citationRequired: false, retrievalRequired: false },
        input: { hasImages: ctx.hasImages ?? false, hasScannedPages: false, hasAudio: ctx.hasAudio ?? false, hasVideo: ctx.hasVideo ?? false },
        context: { stablePrefixVersion: '1.0', systemPolicy: '', personaOverlay: null, approvedMemory: [], conversation: [], retrievedSources: [], toolResults: [] },
        controls: { maxOutputTokens: 4096, maxCreditCost: 1.0, allowWeb: false, allowedTools: [], allowedConnectorScopes: [], requireWriteConfirmation: false, requireStructuredOutput: false },
      },
      affinity ? { stayPinned: true, suggestedRoute: affinity.routeClass } : null,
      new Map(),
    );

    // If pre-route found a local route, skip classification (offline mode).
    if (preDecision.routeClass === 'local') {
      return {
        handler: 'OFFLINE',
        intent: { category: 'conversational', confidence: 1.0, depth: 'standard' },
        reason: preDecision.reason,
      };
    }

    // BYOK users still flow through heuristic classification below — intent and
    // retrieval plan (e.g. needs-files → fts5) must survive when a key is present.
    // resolveHandler() keeps the BYOK handler for BYOK-key users, and correctly
    // falls back to OFFLINE when offline (a BYOK provider can't be reached).

    // Fall through to heuristic classification for managed routes
    const classificationText = buildClassificationText(query.text, ctx.chatIntentAnchor);
    const intent = await this.classifier.classify({ ...query, text: classificationText });
    const routed = applySizeAwareRouting(query, ctx, intent, resolveHandler(intent, query, ctx));

    return {
      handler: routed.handler,
      intent,
      reason: routed.reason,
      ...(routed.retrievalPlan ? { retrievalPlan: routed.retrievalPlan } : {}),
    };
  }
}