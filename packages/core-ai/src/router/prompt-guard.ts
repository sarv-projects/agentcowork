import type { IntentClassification, RouteContext, RouteDecision, UserQuery } from '@agentcowork/core-domain';
import {
  LARGE_INPUT_CHARS,
  SLM_BLOCK_CHARS,
  buildClassificationText,
  createChatIntentAnchor,
  prepareSlmInput,
} from './prompt-limits.js';
import { buildRetrievalPlan } from './retrieval-planner.js';

export type PromptGuardAction =
  | 'allow'
  | 'allow_truncated'
  | 'block_slm'
  | 'file_first'
  | 'byok_large_input'
  | 'offline';

export type PromptGuardResult = {
  action: PromptGuardAction;
  notice?: string;
  slmPrompt?: string;
};

export type InputGuardPreview = {
  level: 'ok' | 'warn' | 'block';
  message: string;
  charCount: number;
};

function isScopedToDocument(
  query: UserQuery,
  ctx: Pick<RouteContext, 'openDocumentId'> = {},
): boolean {
  return Boolean(ctx.openDocumentId || query.scope === 'open-document' || query.attachments?.length);
}

function fileFirstMessage(charCount: number): string {
  return (
    `This input is ${charCount.toLocaleString()} characters — too large to paste into chat. ` +
    'Save it to Library so it can be indexed, then ask a short question about the file. ' +
    'Or connect an AI provider in Library → AI Providers for long-context answers.'
  );
}

function chatBlockMessage(charCount: number): string {
  return (
    `This message is ${charCount.toLocaleString()} characters. Chat is limited to ` +
    `${SLM_BLOCK_CHARS.toLocaleString()} characters without a connected provider. Attach or open a file, or connect an AI provider.`
  );
}

const OFFLINE_NOTICE =
  "You're offline — reading, files, and search still work. AI answers resume when you're back online.";

/** Pre-send UI hint in the composer. */
export function previewInputGuard(
  text: string,
  ctx: Pick<RouteContext, 'openDocumentId' | 'hasInternet'> = { hasInternet: true },
): InputGuardPreview {
  const charCount = text.trim().length;
  if (charCount === 0) {
    return { level: 'ok', message: '', charCount: 0 };
  }
  if (ctx.hasInternet === false) {
    return { level: 'block', message: OFFLINE_NOTICE, charCount };
  }
  if (isScopedToDocument({ text }, ctx)) {
    if (charCount > SLM_BLOCK_CHARS) {
      return {
        level: 'warn',
        message: 'Long question about this file — a connected provider will handle it best.',
        charCount,
      };
    }
    return { level: 'ok', message: '', charCount };
  }
  if (charCount > SLM_BLOCK_CHARS) {
    return { level: 'block', message: chatBlockMessage(charCount), charCount };
  }
  if (charCount > LARGE_INPUT_CHARS) {
    return {
      level: 'warn',
      message: 'Large paste — save to Library and ask a short question, or connect a provider.',
      charCount,
    };
  }
  return { level: 'ok', message: '', charCount };
}

const MANAGED_HANDLERS = new Set(['MANAGED_FREE', 'MANAGED_PAID']);

/** Size-aware overrides applied after keyword routing. */
export function applySizeAwareRouting(
  query: UserQuery,
  ctx: RouteContext,
  _intent: IntentClassification,
  base: Pick<RouteDecision, 'handler' | 'retrievalPlan' | 'reason'>,
): Pick<RouteDecision, 'handler' | 'retrievalPlan' | 'reason'> {
  const charCount = query.text.trim().length;
  const scoped = isScopedToDocument(query, ctx);

  if (scoped) {
    return base;
  }

  if (charCount > SLM_BLOCK_CHARS && MANAGED_HANDLERS.has(base.handler)) {
    const sizedIntent: IntentClassification = {
      category: 'needs-files',
      confidence: 0.9,
      subCategory: 'large-paste',
      depth: 'standard',
    };
    const retrievalPlan = buildRetrievalPlan(sizedIntent.category, query, ctx);
    return {
      handler: ctx.hasByokKey ? 'BYOK' : 'PROMPT_BYOK',
      ...(retrievalPlan ? { retrievalPlan } : {}),
      reason: `large input (${charCount} chars): route to indexed files / provider, not managed pool`,
    };
  }

  if (charCount > LARGE_INPUT_CHARS && MANAGED_HANDLERS.has(base.handler)) {
    const sizedIntent: IntentClassification = {
      category: 'needs-files',
      confidence: 0.85,
      subCategory: 'large-paste',
      depth: 'standard',
    };
    const retrievalPlan = buildRetrievalPlan(sizedIntent.category, query, ctx);
    return {
      handler: ctx.hasByokKey ? 'BYOK' : 'PROMPT_BYOK',
      ...(retrievalPlan ? { retrievalPlan } : {}),
      reason: `input over ${LARGE_INPUT_CHARS} chars: prefer file index or BYOK over managed pool`,
    };
  }

  return base;
}

/** Post-route guard before streaming or showing BYOK card. */
export function evaluatePromptGuard(
  query: UserQuery,
  ctx: RouteContext,
  decision: RouteDecision,
): PromptGuardResult {
  const charCount = query.text.trim().length;
  const scoped = isScopedToDocument(query, ctx);

  if (decision.handler === 'OFFLINE') {
    return { action: 'offline', notice: OFFLINE_NOTICE };
  }

  if (decision.handler === 'PROMPT_BYOK' && !scoped && charCount > LARGE_INPUT_CHARS) {
    return { action: 'file_first', notice: fileFirstMessage(charCount) };
  }

  // Connected BYOK users pay their own provider costs — never block large
  // inputs for them (C.14). Only PROMPT_BYOK (no key connected yet) gets the
  // "save to Library / connect a provider" push.
  if (decision.handler === 'BYOK' && !scoped && charCount > LARGE_INPUT_CHARS) {
    return { action: 'allow', notice: 'Large input routed to your connected provider.' };
  }

  if (MANAGED_HANDLERS.has(decision.handler)) {
    if (!scoped && charCount > SLM_BLOCK_CHARS) {
      return { action: 'block_slm', notice: chatBlockMessage(charCount) };
    }

    const prep = prepareSlmInput(query.text);
    if (prep.truncated) {
      return {
        action: 'allow_truncated',
        slmPrompt: prep.text,
        notice:
          `Only the last ~${prep.estimatedDroppedTokens} tokens of your message were used ` +
          '(context limit). For full-document answers, save to Library or use a provider.',
      };
    }
    return { action: 'allow', slmPrompt: query.text };
  }

  return { action: 'allow' };
}

export { buildClassificationText, createChatIntentAnchor };