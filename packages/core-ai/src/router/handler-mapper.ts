import type {
  IntentCategory,
  IntentClassification,
  RetrievalPlan,
  RouteContext,
  RouteDecision,
  RouteHandler,
  UserQuery,
} from '@agentcowork/core-domain';
import { buildRetrievalPlan } from './retrieval-planner.js';

function routeResult(
  handler: RouteHandler,
  retrievalPlan: RetrievalPlan | undefined,
  reason: string,
): Pick<RouteDecision, 'handler' | 'retrievalPlan' | 'reason'> {
  if (retrievalPlan) {
    return { handler, retrievalPlan, reason };
  }
  return { handler, reason };
}

const SIMPLE_CLOUD_CATEGORIES: IntentCategory[] = ['conversational', 'out-of-scope'];

const GENERATION_CATEGORIES: IntentCategory[] = [
  'needs-files',
  'needs-web',
  'needs-connector',
  'needs-docs',
];

function isLowPower(ctx: RouteContext): boolean {
  return ctx.batteryLevel != null && ctx.batteryLevel < 0.15;
}

function isOffline(ctx: RouteContext): boolean {
  return !ctx.hasInternet;
}

function detectInputModalities(
  query: UserQuery,
): { hasImages: boolean; hasAudio: boolean; hasVideo: boolean } {
  const hasImages = Boolean(
    query.attachments?.some((a) => a.endsWith('.png') || a.endsWith('.jpg') || a.endsWith('.jpeg') || a.endsWith('.webp') || a.endsWith('.gif')),
  );
  const hasAudio = Boolean(
    query.attachments?.some((a) => a.endsWith('.mp3') || a.endsWith('.wav') || a.endsWith('.m4a') || a.endsWith('.ogg')),
  );
  const hasVideo = Boolean(
    query.attachments?.some((a) => a.endsWith('.mp4') || a.endsWith('.mov') || a.endsWith('.webm')),
  );
  return { hasImages, hasAudio, hasVideo };
}

function managedRouteForCapabilities(
  ctx: RouteContext,
  modalities: { hasImages: boolean; hasAudio: boolean; hasVideo: boolean },
): RouteHandler {
  // Vision route: if images/audio/video present, use MiMo V2.5
  if (modalities.hasImages || modalities.hasAudio || modalities.hasVideo) {
    return 'MANAGED_VISION';
  }
  // Text-only: respect user's explicit model mode choice
  if (ctx.tier === 'free') return 'MANAGED_FREE';
  if (ctx.modelMode === 'smart') return 'MANAGED_SMART';
  if ((ctx.modelMode as string) === 'frontier') return 'MANAGED_PAID';
  return 'MANAGED_FAST';
}

export function resolveHandler(
  intent: IntentClassification,
  query: UserQuery,
  ctx: RouteContext,
): Pick<RouteDecision, 'handler' | 'retrievalPlan' | 'reason'> {
  const retrievalPlan = buildRetrievalPlan(intent.category, query, ctx);

  if (isOffline(ctx)) {
    if (intent.category === 'needs-automation') {
      return routeResult(
        'AUTOMATION_DRAFT',
        retrievalPlan,
        'offline: queue automation draft locally until connectivity returns',
      );
    }
    return routeResult(
      'OFFLINE',
      retrievalPlan,
      'offline: reader, files, and search work; AI answers need internet',
    );
  }

  if (isLowPower(ctx) && intent.category === 'needs-automation') {
    return routeResult(
      'AUTOMATION_DRAFT',
      retrievalPlan,
      'low-battery: queue automation draft until device is charged',
    );
  }

  if (intent.category === 'needs-automation') {
    return routeResult('AUTOMATION_DRAFT', retrievalPlan, 'automation intent: draft → commit flow');
  }

  if (intent.category === 'out-of-scope') {
    if (ctx.hasByokKey) {
      return routeResult('BYOK', retrievalPlan, 'out-of-scope: route to user provider');
    }
    return routeResult(
      managedRouteForCapabilities(ctx, detectInputModalities(query)),
      retrievalPlan,
      'out-of-scope: polite cloud decline via managed route',
    );
  }

  if (SIMPLE_CLOUD_CATEGORIES.includes(intent.category)) {
    if (ctx.hasByokKey) {
      return routeResult(
        'BYOK',
        retrievalPlan,
        'simple conversational intent: route to user provider',
      );
    }
    return routeResult(
      managedRouteForCapabilities(ctx, detectInputModalities(query)),
      retrievalPlan,
      'simple conversational intent: managed route (Fast/Smart/Vision)',
    );
  }

  if (GENERATION_CATEGORIES.includes(intent.category)) {
    if (ctx.hasByokKey) {
      return routeResult(
        'BYOK',
        retrievalPlan,
        `route ${intent.category} to user provider with assembled context`,
      );
    }
    return routeResult(
      managedRouteForCapabilities(ctx, detectInputModalities(query)),
      retrievalPlan,
      `route ${intent.category} via managed route with capability detection`,
    );
  }

  return routeResult(
    ctx.hasByokKey ? 'BYOK' : managedRouteForCapabilities(ctx, detectInputModalities(query)),
    retrievalPlan,
    'default route',
  );
}