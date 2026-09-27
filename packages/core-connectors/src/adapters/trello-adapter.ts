/**
 * Trello connector — host-mediated Atlassian Trello authentication.
 *
 * Trello requires both a user credential and an app key. The Rust host owns
 * both values, performs the authenticated request, and returns only provider
 * data. This adapter never accepts or constructs either secret.
 */
import type {
  ConnectorAdapter,
  ConnectorContext,
  ConnectorFilter,
  ConnectorResult,
  MemoryFact,
  UserQuery,
} from '@agentcowork/core-domain';
import { requestConnector } from '../connection-manager.js';

const TRELLO_API = 'https://api.trello.com/1';
const CONNECTOR_NAME = 'trello' as const;

export class TrelloAdapter implements ConnectorAdapter {
  readonly name = CONNECTOR_NAME;
  readonly credentialMode = 'host-mediated' as const;
  readonly metadataSchema = {
    fields: [
      { name: 'query', type: 'string' as const, description: 'Text to search across boards/cards' },
      { name: 'boardId', type: 'string' as const, description: 'Optional board filter' },
    ],
  };

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['trello', 'board', 'kanban', 'list', 'card', 'todo board', 'task board', 'project board'];
    if (terms.some((t) => q.includes(t))) return 0.8;
    if (/\b(organize|organise|status|sticky)\b/.test(q)) return 0.35;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text || '' };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = ctx.filter as {
      query?: string;
      boardId?: string;
      limit?: number;
    };
    const limit = Math.min(f.limit ?? 20, 50);
    const q = (f.query || '').trim();
    const params = new URLSearchParams({ limit: String(limit) });
    if (f.boardId) params.set('idBoards', f.boardId);

    try {
      // If the user typed a query, hit /search; otherwise enumerate the
      // user's own cards across all boards.
      let endpoint = `${TRELLO_API}/members/me/cards`;
      if (q) {
        endpoint = `${TRELLO_API}/search`;
        params.set('query', q);
        params.set('cards', 'true');
        params.set('card_fields', 'name,desc,due,dueComplete,idList,idBoard,shortUrl,labels,url');
        params.delete('limit'); // search uses its own paging model
      } else {
        params.set('fields', 'name,desc,due,dueComplete,idList,idBoard,shortUrl,labels,url');
      }

      const res = await requestConnector({
        connector: CONNECTOR_NAME,
        userId: ctx.userId,
        request: { url: `${endpoint}?${params.toString()}`, method: 'GET' },
        ...(ctx.signal ? { signal: ctx.signal } : {}),
      });
      if (!res) return { items: [], totalCount: 0, source: CONNECTOR_NAME };
      if (!res.ok) {
        return {
          items: [
            {
              id: `err:${res.status}`,
              title: 'Trello request failed',
              snippet: `HTTP ${res.status}. Reconnect Trello and try again.`,
              metadata: { status: res.status },
            },
          ],
          totalCount: 1,
          source: CONNECTOR_NAME,
        };
      }

      type TrelloCard = {
        id: string;
        name: string;
        desc?: string;
        due?: string | null;
        dueComplete?: boolean;
        shortUrl?: string;
        url?: string;
        idBoard?: string;
        idList?: string;
        labels?: Array<{ name?: string; color?: string }>;
      };
      const data = (await res.json()) as TrelloCard[] | { cards?: TrelloCard[] };

      const list = Array.isArray(data) ? data : data.cards ?? [];

      const items: ConnectorResult['items'] = list.slice(0, limit).map((card) => {
        const labels = (card.labels ?? [])
          .map((l) => l.name || l.color)
          .filter(Boolean)
          .join(', ');
        const due =
          card.due && !card.dueComplete
            ? `due ${new Date(card.due).toLocaleDateString()}`
            : card.dueComplete
              ? 'done'
              : '';
        const snippet = [due, labels ? `[${labels}]` : '', card.desc?.slice(0, 120)]
          .filter(Boolean)
          .join(' · ');
        const item: ConnectorResult['items'][number] = {
          id: card.id,
          title: card.name || 'Untitled card',
          snippet: snippet || '(no description)',
          metadata: {
            boardId: card.idBoard,
            listId: card.idList,
            due: card.due,
            done: card.dueComplete,
            labels: card.labels,
          },
        };
        const cardUrl = card.shortUrl ?? card.url;
        if (cardUrl) item.url = cardUrl;
        if (card.due) item.date = card.due;
        return item;
      });

      return { items, totalCount: items.length, source: CONNECTOR_NAME };
    } catch {
      return { items: [], totalCount: 0, source: CONNECTOR_NAME };
    }
  }

}
