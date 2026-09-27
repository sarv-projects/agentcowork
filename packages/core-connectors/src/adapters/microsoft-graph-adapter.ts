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
 * Microsoft Graph adapter — Outlook Mail / Calendar / OneDrive.
 *
 * One OAuth provider `microsoft-graph` covers 3 services. We register three
 * instances of this class with different `name` + `subService` so the mobile
 * app can pick a sub-service independently:
 *
 *   - 'microsoft-mail'      → Graph /me/messages
 *   - 'microsoft-calendar'  → Graph /me/events
 *   - 'microsoft-onedrive'  → Graph /me/drive/root/children
 *
 * All three use the same host-owned Microsoft OAuth account. TypeScript sends
 * only query intent; the Rust host resolves the vault credential and performs
 * the Graph request.
 */
const GRAPH = 'https://graph.microsoft.com/v1.0';

export type MicrosoftSubService = 'mail' | 'calendar' | 'onedrive';

const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'query', type: 'string', description: 'Search / filter text (subject, name)' },
    { name: 'top', type: 'number', description: 'Max results (default 10)' },
  ],
};

export class MicrosoftGraphAdapter implements ConnectorAdapter {
  readonly name: ConnectorName;
  readonly credentialMode = 'host-mediated' as const;
  readonly metadataSchema = metadataSchema;
  private readonly subService: MicrosoftSubService;

  constructor(name: ConnectorName, subService: MicrosoftSubService) {
    this.name = name;
    this.subService = subService;
  }

  async isAuthorized(_userId: string): Promise<boolean> {
    return true; // Credential resolution is host-owned
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const common = ['outlook', 'office', 'microsoft', 'office365', 'office 365'];
    if (!common.some((t) => q.includes(t))) {
      // Generic terms per sub-service
      if (this.subService === 'mail' && /(inbox|my email|mail from|unread)/.test(q)) return 0.7;
      if (this.subService === 'calendar' && /(calendar|event|appointment|meeting|schedule|my schedule)/.test(q)) return 0.75;
      if (this.subService === 'onedrive' && /(onedrive|my files|sharepoint)/.test(q)) return 0.7;
      return 0.1;
    }
    // Microsoft-family keyword always nudges relevance; sub-service boosts further.
    if (this.subService === 'calendar' && /(calendar|event|meeting|schedule)/.test(q)) return 0.85;
    if (this.subService === 'mail' && /(mail|inbox|email)/.test(q)) return 0.85;
    if (this.subService === 'onedrive' && /(onedrive|file|document)/.test(q)) return 0.85;
    return 0.6;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text || '', top: 10 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const filter = (ctx.filter || {}) as { query?: string; top?: number };
    const top = Math.min(Math.max(Number(filter.top) || 10, 1), 25);
    const headers: Record<string, string> = { ConsistencyLevel: 'eventual' };

    try {
      const query = (filter.query || '').trim();
      let url = '';
      switch (this.subService) {
        case 'mail': {
          if (query) {
            const params = new URLSearchParams({
              $search: `"${query.replace(/"/g, '')}"`,
              $top: String(top),
            });
            url = `${GRAPH}/me/messages?${params.toString()}`;
          } else {
            const params = new URLSearchParams({
              $top: String(top),
              $orderby: 'receivedDateTime desc',
              $select: 'subject,from,receivedDateTime,bodyPreview,webLink',
            });
            url = `${GRAPH}/me/messages?${params.toString()}`;
          }
          break;
        }
        case 'calendar': {
          const params = new URLSearchParams({
            $top: String(top),
            $orderby: 'start/dateTime asc',
            $select: 'subject,start,end,location,organizer,webLink',
          });
          if (query) params.set('$filter', `contains(subject,'${query.replace(/'/g, "''")}')`);
          url = `${GRAPH}/me/events?${params.toString()}`;
          break;
        }
        case 'onedrive': {
          const params = new URLSearchParams({
            $top: String(top),
            $orderby: 'lastModifiedDateTime desc',
            $select: 'name,size,lastModifiedDateTime,webUrl,file,folder',
          });
          if (query) params.set('$filter', `contains(name,'${query.replace(/'/g, "''")}')`);
          url = `${GRAPH}/me/drive/root/children?${params.toString()}`;
          break;
        }
      }

      const res = await requestConnector({
        connector: this.name,
        userId: ctx.userId,
        request: { url, method: 'GET', headers },
        ...(ctx.signal ? { signal: ctx.signal } : {}),
      });
      if (!res?.ok) {
        return { items: [], totalCount: 0, source: this.name };
      }
      const data = (await res.json()) as { value?: Array<Record<string, unknown>> };
      const items: ConnectorResult['items'] = (data.value ?? []).map((row, idx) =>
        mapGraphRow(this.subService, row, idx),
      );
      return { items, totalCount: items.length, source: this.name };
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }

}

function mapGraphRow(sub: MicrosoftSubService, row: Record<string, unknown>, idx: number): ConnectorResult['items'][number] {
  if (sub === 'mail') {
    const subject = (row.subject as string) || '(no subject)';
    const from = row.from
      ? ((row.from as { emailAddress?: { name?: string; address?: string } }).emailAddress?.name ||
          (row.from as { emailAddress?: { address?: string } }).emailAddress?.address ||
          'unknown')
      : '';
    // bodyPreview is HTML per Graph docs — strip tags + decode entities before display.
    const previewText = decodeHtmlEntities(stripHtmlTags(String(row.bodyPreview || '')));
    const cleaned = previewText.replace(/\s+/g, ' ').trim();
    const snippet = [from && `From: ${from}`, cleaned].filter(Boolean).join(' • ');
    return {
      id: (row.id as string) || `mail-${idx}`,
      title: subject,
      snippet: snippet.slice(0, 280),
      url: (row.webLink as string) || '',
      date: (row.receivedDateTime as string) || '',
      metadata: { subService: 'mail' },
    };
  }
  if (sub === 'calendar') {
    const subject = (row.subject as string) || '(no subject)';
    const start = (row.start as { dateTime?: string })?.dateTime || '';
    const loc = (row.location as { displayName?: string })?.displayName || '';
    return {
      id: (row.id as string) || `event-${idx}`,
      title: subject,
      snippet: [start && `When: ${start}`, loc && `Where: ${loc}`].filter(Boolean).join(' • ').slice(0, 280),
      url: (row.webLink as string) || '',
      date: start,
      metadata: {
        subService: 'calendar',
        location: loc,
        start,
        end: (row.end as { dateTime?: string })?.dateTime,
      },
    };
  }
  // onedrive
  const name = (row.name as string) || '(untitled)';
  const size = typeof row.size === 'number' ? (row.size as number) : 0;
  return {
    id: (row.id as string) || `file-${idx}`,
    title: name,
    snippet: `${row.folder ? 'Folder' : row.file ? 'File' : 'Item'} • ${formatSize(size)}`,
    url: (row.webUrl as string) || '',
    date: (row.lastModifiedDateTime as string) || '',
    metadata: { subService: 'onedrive', size, mimeType: (row.file as { mimeType?: string })?.mimeType },
  };
}

function formatSize(bytes: number): string {
  if (!bytes) return '—';
  const k = 1024;
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), units.length - 1);
  return `${(bytes / Math.pow(k, i)).toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

function stripHtmlTags(s: string): string {
  return s.replace(/<[^>]+>/g, '');
}

// Decode the standard HTML entities Microsoft Graph bodyPreview typically contains
// (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;`, numeric `&#NNN;`). Without this,
// raw entities leak into the snippet and downstream prompt contexts.
const NAMED_ENTITIES: Record<string, string> = {
  amp: '&',
  lt: '<',
  gt: '>',
  quot: '"',
  apos: "'",
  nbsp: ' ',
};
function decodeHtmlEntities(s: string): string {
  return s.replace(/&(#x?[0-9a-fA-F]+|[a-zA-Z]+);/g, (match, body: string) => {
    const lower = body.toLowerCase();
    if (lower.startsWith('#x') || lower.startsWith('#')) {
      const codePoint = lower.startsWith('#x') ? parseInt(lower.slice(2), 16) : parseInt(lower.slice(1), 10);
      if (!Number.isFinite(codePoint) || codePoint < 0 || codePoint > 0x10ffff) return match;
      try {
        return String.fromCodePoint(codePoint);
      } catch {
        return match;
      }
    }
    return NAMED_ENTITIES[lower] ?? match;
  });
}
