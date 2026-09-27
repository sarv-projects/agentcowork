import type {
  ConnectorAdapter,
  ConnectorContext,
  ConnectorFilter,
  ConnectorMetadataSchema,
  ConnectorName,
  ConnectorResult,
  UserQuery,
  MemoryFact,
} from '@agentcowork/core-domain';
import { requestConnector } from '../connection-manager.js';

/**
 * Todoist adapter — host-mediated OAuth, REST v2.
 *
 * Free OAuth (registration is free on todoist.com). Used for productivity
 * queries ("add X to my todays", "what's on my list").
 *
 * Endpoints:
 *   GET https://api.todoist.com/rest/v2/tasks?filter={query}
 *   GET https://api.todoist.com/rest/v2/projects
 *   POST https://api.todoist.com/rest/v2/tasks  (used by tool layer, not connector)
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'query', type: 'string', description: 'Todoist filter expression (e.g. "today", "p1")' },
    { name: 'project_id', type: 'string', description: 'Optional project ID filter' },
    { name: 'limit', type: 'number', description: 'Max results (default 15)' },
  ],
};

const TODOIST_API = 'https://api.todoist.com/rest/v2';

export class TodoistAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'todoist';
  readonly credentialMode = 'host-mediated' as const;
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['todoist', 'todo', 'task', 'task list', 'tasks today', 'add to my list', 'remind me to', 'add to tasks', 'what tasks', 'due today'];
    if (terms.some((t) => q.includes(t))) return 0.85;
    if (/add\b.*\bto\b.*\b(list|tasks)/i.test(q)) return 0.75;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    const text = query.text || '';
    let filter = '';
    if (/today|due today/i.test(text)) filter = 'today';
    else if (/tomorrow/i.test(text)) filter = 'tomorrow';
    else if (/this week|upcoming/i.test(text)) filter = 'week';
    else if (/overdue|late/i.test(text)) filter = 'overdue';
    else filter = text.length > 60 ? text.slice(0, 60) : text;
    return { query: filter, limit: 15 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { query?: string; project_id?: string; limit?: number };
    const limit = Math.min(Math.max(Number(f.limit) || 15, 1), 50);
    const params = new URLSearchParams();
    if (f.project_id) params.set('project_id', f.project_id);
    if (f.query) params.set('filter', f.query);
    const url = `${TODOIST_API}/tasks?${params.toString() || `limit=${limit}`}`;

    try {
      const res = await requestConnector({
        connector: this.name,
        userId: ctx.userId,
        request: { url, method: 'GET', headers: { Accept: 'application/json' } },
        ...(ctx.signal ? { signal: ctx.signal } : {}),
      });
      if (!res?.ok) return { items: [], totalCount: 0, source: this.name };
      const raw = (await res.json()) as Array<{
        id: string;
        content: string;
        description?: string;
        due?: { date?: string; datetime?: string; string?: string };
        priority?: number;
        project_id?: string;
        url?: string;
      }>;
      const items: ConnectorResult['items'] = (Array.isArray(raw) ? raw : []).slice(0, limit).map((t) => ({
        id: `todo-${t.id}`,
        title: t.content,
        snippet: [t.due?.string || t.due?.date, t.priority ? `p${t.priority}` : ''].filter(Boolean).join(' · ').slice(0, 280) || (t.description || '').slice(0, 280),
        url: t.url || '',
        metadata: {
          due: t.due?.string || t.due?.date,
          priority: t.priority,
          project_id: t.project_id,
        },
      }));
      return { items, totalCount: items.length, source: this.name };
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }

}
