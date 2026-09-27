/**
 * Deterministic pre-router — handles most traffic without an LLM call.
 *
 * "Do not use an LLM to decide where every request goes.
 * A rule-based pre-router should handle most traffic in a few milliseconds."
 *
 * Routing priority (lexicographic):
 * 1. Correct privacy route — Local, Managed, or BYOK
 * 2. Required capability — text, vision, tools, JSON, context capacity
 * 3. User choice — Fast, Smart, or exact BYOK model
 * 4. Cache affinity — same model/deployment for active thread
 * 5. Latency — healthy endpoint with best p95 TTFT
 * 6. Cost — lowest-cost route meeting quality bar
 * 7. Fallback resilience — only after timeout/error
 */

import type { AssistantRequestPlan } from '@agentcowork/core-domain';
import type { RouteClass } from '../router/affinity-tracker.js';
import type { RouteHealth } from '../metrics/metrics-collector.js';

// ─── Route decision ──────────────────────────────────────────────────

export type PreRouteDecision = {
  routeClass: RouteClass;
  provider: string;
  reason: string;
  /** If vision extraction is needed before final answer */
  needsVisionExtraction: boolean;
  /** If hedging should be considered */
  eligibleForHedge: boolean;
};

// ─── SLO thresholds ──────────────────────────────────────────────────

const SLO = {
  fastShortMs: 700,     // p50 TTFT target for Fast short text
  fastCachedMs: 1000,   // p50 TTFT target for Fast cached long thread
  smartMs: 1500,        // p50 TTFT target for Smart text
  visionMs: 2500,       // p50 TTFT target for vision extraction
  hedgeThresholdMs: 1200, // If no first token by this time, consider hedging
};

// ─── Pre-router ──────────────────────────────────────────────────────

/**
 * Deterministic pre-router — handles traffic without classifier latency.
 */
export function preRoute(
  plan: AssistantRequestPlan,
  affinity: { stayPinned: boolean; suggestedRoute: RouteClass } | null,
  routeHealth: Map<string, RouteHealth>,
): PreRouteDecision {
  // 1. Local mode — storage/retrieval on-device; generation requires downloaded model
  if (plan.privacyMode === 'local') {
    return {
      routeClass: 'local',
      provider: 'device',
      reason: 'local mode: storage and retrieval available; generation requires downloaded model',
      needsVisionExtraction: false,
      eligibleForHedge: false,
    };
  }

  // 2. BYOK mode — user-selected provider
  if (plan.privacyMode === 'byok') {
    return {
      routeClass: 'byok',
      provider: plan.context.personaOverlay ?? 'user-selected',
      reason: 'byok: user-selected provider',
      needsVisionExtraction: false,
      eligibleForHedge: false,
    };
  }

  // 3. Vision pipeline — images/audio/video present
  if (plan.input.hasImages || plan.input.hasScannedPages || plan.input.hasAudio || plan.input.hasVideo) {
    return {
      routeClass: 'vision-extract',
      provider: 'mimo-v2.5',
      reason: 'vision: visual input detected, route to MiMo V2.5',
      needsVisionExtraction: true,
      eligibleForHedge: false, // Vision is expensive, don't hedge
    };
  }

  // 4. User explicit choice
  if (plan.modelMode === 'fast') {
    const health = routeHealth.get('fast-text') ?? null;
    return {
      routeClass: 'fast-text',
      provider: 'deepseek-v4-flash',
      reason: 'user selected Fast',
      needsVisionExtraction: false,
      eligibleForHedge: isHedgeEligible('fast-text', health),
    };
  }

  if (plan.modelMode === 'smart') {
    return {
      routeClass: 'smart-text',
      provider: 'deepseek-v4-pro',
      reason: 'user selected Smart',
      needsVisionExtraction: false,
      eligibleForHedge: false, // Smart is expensive, don't hedge
    };
  }

  // 5. Cache affinity — stay pinned if possible
  if (affinity?.stayPinned) {
    return {
      routeClass: affinity.suggestedRoute,
      provider: getProviderForRoute(affinity.suggestedRoute),
      reason: `cache affinity: pinned to ${affinity.suggestedRoute}`,
      needsVisionExtraction: false,
      eligibleForHedge: isHedgeEligible(affinity.suggestedRoute, routeHealth.get(affinity.suggestedRoute) ?? null),
    };
  }

  // 6. Auto-route based on task complexity
  return autoRoute(plan, routeHealth);
}

// ─── Auto-route classifier ───────────────────────────────────────────

function autoRoute(
  plan: AssistantRequestPlan,
  routeHealth: Map<string, RouteHealth>,
): PreRouteDecision {
  const task = plan.task;

  // Smart signals — complex tasks
  const smartSignals = [
    task.kind === 'code',
    task.kind === 'plan',
    task.kind === 'research',
    task.depth === 'detailed',
    task.outputFormat === 'json' || task.outputFormat === 'structured',
    plan.controls.allowedTools.length > 2,
    plan.scope.citationRequired,
    plan.scope.mode === 'project',
  ];

  const smartScore = smartSignals.filter(Boolean).length;

  // Fast signals — simple tasks
  const fastSignals = [
    task.kind === 'chat' || task.kind === 'explain' || task.kind === 'summarize',
    task.depth === 'quick',
    task.outputFormat === 'prose',
    plan.controls.allowedTools.length <= 1,
    !plan.scope.citationRequired,
  ];

  const fastScore = fastSignals.filter(Boolean).length;

  // Decision
  if (smartScore >= 2 || (smartScore >= 1 && fastScore === 0)) {
    return {
      routeClass: 'smart-text',
      provider: 'deepseek-v4-pro',
      reason: `auto-route: ${smartScore} smart signals`,
      needsVisionExtraction: false,
      eligibleForHedge: false,
    };
  }

  return {
    routeClass: 'fast-text',
    provider: 'deepseek-v4-flash',
    reason: `auto-route: ${fastScore} fast signals, default Fast`,
    needsVisionExtraction: false,
    eligibleForHedge: isHedgeEligible('fast-text', routeHealth.get('fast-text') ?? null),
  };
}

// ─── Hedging eligibility ─────────────────────────────────────────────

function isHedgeEligible(routeClass: RouteClass, health: RouteHealth | null): boolean {
  // Only hedge Fast short text under degradation
  if (routeClass !== 'fast-text') return false;
  if (!health) return false;

  // Hedge if error rate or timeout rate is elevated
  if (health.errorRate5m > 0.1) return true;
  if (health.timeoutRate5m > 0.1) return true;
  if (health.p95TtftMs > SLO.hedgeThresholdMs) return true;

  return false;
}

// ─── Provider resolution ─────────────────────────────────────────────

function getProviderForRoute(route: RouteClass): string {
  switch (route) {
    case 'fast-text': return 'deepseek-v4-flash';
    case 'smart-text': return 'deepseek-v4-pro';
    case 'vision-extract': return 'mimo-v2.5';
    case 'byok': return 'user-selected';
    case 'local': return 'device';
  }
}
