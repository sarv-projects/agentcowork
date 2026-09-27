/**
 * Google Drive connector — OAuth-based access to Drive files.
 *
 * Free: 15GB storage, 10k API requests/day per project.
 * OAuth: Required. Scopes: drive.readonly or drive.metadata.readonly.
 *
 * What works free:
 * - List files: 1 req/call
 * - Search files: 1 req/call
 * - Read metadata: 1 req/file
 * - Download text-based files: 1 req/file + bandwidth
 *
 * What needs payment:
 * - >15GB storage (Google One)
 * - >10k API req/day (quota increase paid)
 * - Drive audit/activity APIs
 *
 * Flow:
 *   1. User taps "Connect Google Drive"
 *   2. OAuth runs through the host Auth Bridge
 *   3. The credential remains vault-owned
 *   4. fetch() sends credential-free intent through the Rust host
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

const DRIVE_API = 'https://www.googleapis.com/drive/v3';
const CONNECTOR_NAME = 'google-drive' as const;

export class GoogleDriveOAuthAdapter implements ConnectorAdapter {
  readonly name = CONNECTOR_NAME;
  readonly credentialMode = 'host-mediated' as const;
  readonly metadataSchema = {
    fields: [
      { name: 'query', type: 'string' as const, description: 'Search query (files)' },
      { name: 'mimeType', type: 'string' as const, description: 'Filter by MIME type' },
    ],
  };

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['drive', 'file', 'document', 'sheet', 'slide', 'pdf', 'my file'];
    return terms.some((t) => q.includes(t)) ? 0.7 : 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text || '', mimeType: '' };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const filter = ctx.filter as { query?: string; mimeType?: string };
    const searchQuery = filter.query || '';

    try {
      const url = new URL(`${DRIVE_API}/files`);
      url.searchParams.set('pageSize', '20');
      url.searchParams.set(
        'fields',
        'files(id,name,mimeType,size,modifiedTime,webViewLink,owners/displayName),nextPageToken',
      );

      if (searchQuery) {
        let q = `name contains '${searchQuery.replace(/'/g, "\\'")}'`;
        if (filter.mimeType) {
          q += ` and mimeType = '${filter.mimeType}'`;
        }
        url.searchParams.set('q', q);
      } else {
        // Recent files
        url.searchParams.set('orderBy', 'modifiedTime desc');
        url.searchParams.set('q', 'trashed = false');
      }

      const res = await requestConnector({
        connector: CONNECTOR_NAME,
        userId: ctx.userId,
        request: { url: url.toString(), method: 'GET' },
        ...(ctx.signal ? { signal: ctx.signal } : {}),
      });

      if (!res?.ok) {
        return { items: [], totalCount: 0, source: CONNECTOR_NAME };
      }

      const data = (await res.json()) as {
        files?: Array<{
          id: string;
          name: string;
          mimeType: string;
          size?: string;
          modifiedTime: string;
          webViewLink?: string;
          owners?: Array<{ displayName: string }>;
        }>;
      };

      const items: ConnectorResult['items'] = (data.files ?? []).map((file) => ({
        id: file.id,
        title: file.name,
        snippet: `${file.mimeType}${file.size ? ` (${formatSize(Number(file.size))})` : ''} — ${file.owners?.[0]?.displayName ?? 'Unknown'}`,
        ...(file.webViewLink ? { url: file.webViewLink } : {}),
        date: file.modifiedTime,
        metadata: {
          mimeType: file.mimeType,
          size: file.size,
          owner: file.owners?.[0]?.displayName,
        },
      }));

      return { items, totalCount: items.length, source: CONNECTOR_NAME };

    } catch {
      return { items: [], totalCount: 0, source: CONNECTOR_NAME };
    }
  }

}

function formatSize(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
}
