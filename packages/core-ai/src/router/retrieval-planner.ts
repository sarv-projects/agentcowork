import type { IntentCategory, RetrievalPlan, RouteContext, UserQuery } from '@agentcowork/core-domain';
import { buildMemoryRetrievalHint, attachMemoryScopeToPlan } from '@agentcowork/core-memory';

export function buildRetrievalPlan(
  category: IntentCategory,
  query: UserQuery,
  ctx: RouteContext,
): RetrievalPlan | undefined {
  switch (category) {
    case 'needs-files': {
      const scopeDocumentId =
        ctx.openDocumentId ?? (query.scope === 'open-document' ? 'open' : undefined);
      const sources: RetrievalPlan['sources'] = ['fts5', 'vector'];
      if (query.scope === 'memory') {
        sources.push('memory');
      }
      if (query.scope === 'web') {
        sources.push('web');
      }
      const plan: RetrievalPlan = {
        sources,
        query: query.text,
        maxResults: 8,
      };
      if (scopeDocumentId) {
        return attachMemoryScopeToPlan(
          { ...plan, scopeDocumentId },
          buildMemoryRetrievalHint(category, query, ctx),
        );
      }
      if (query.scope === 'all-files' || query.scope === 'memory' || query.scope === 'web') {
        return attachMemoryScopeToPlan(plan, buildMemoryRetrievalHint(category, query, ctx));
      }
      return plan;
    }
    case 'needs-web':
      return {
        sources: ['web'],
        query: query.text,
        maxResults: 6,
      };
    case 'needs-connector': {
      const base: RetrievalPlan = {
        sources: ['connector', 'memory'],
        query: query.text,
        maxResults: 5,
        connectorFilters: { active: ctx.activeConnectors },
      };
      return attachMemoryScopeToPlan(base, buildMemoryRetrievalHint(category, query, ctx));
    }
    case 'needs-docs':
      return {
        sources: ['docs', 'web'],
        query: query.text,
        maxResults: 6,
      };
    case 'conversational': {
      if (query.scope === 'memory') {
        const base: RetrievalPlan = { sources: ['memory'], query: query.text, maxResults: 8 };
        return attachMemoryScopeToPlan(base, buildMemoryRetrievalHint(category, query, ctx));
      }
      return undefined;
    }
    default:
      return undefined;
  }
}