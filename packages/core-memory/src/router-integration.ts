import type { IntentCategory, RetrievalPlan, RouteContext, UserQuery } from '@agentcowork/core-domain';
import { memoryCategoriesForIntent } from './categories.js';

export type MemoryRetrievalHint = {
  categories: ReturnType<typeof memoryCategoriesForIntent>;
  sourceId?: string;
  limit: number;
};

/** Derive scoped memory retrieval hints for Smart Router / retrieval-service. */
export function buildMemoryRetrievalHint(
  category: IntentCategory,
  query: UserQuery,
  ctx: RouteContext,
): MemoryRetrievalHint {
  const intentOptions = ctx.openDocumentId
    ? { openDocumentId: ctx.openDocumentId }
    : {};
  const categories = memoryCategoriesForIntent(category, query.text, intentOptions);

  const hint: MemoryRetrievalHint = {
    categories,
    limit: 8,
  };

  if (ctx.openDocumentId) {
    hint.sourceId = ctx.openDocumentId;
  }

  return hint;
}

export function attachMemoryScopeToPlan(
  plan: RetrievalPlan,
  hint: MemoryRetrievalHint,
): RetrievalPlan {
  return {
    ...plan,
    memoryCategories: hint.categories,
    ...(hint.sourceId ? { memorySourceId: hint.sourceId } : {}),
    maxResults: Math.max(plan.maxResults, hint.limit),
  };
}