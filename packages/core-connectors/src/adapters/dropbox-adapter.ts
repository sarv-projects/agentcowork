/**
 * Dropbox connector — OAuth-based access to Dropbox files.
 *
 * Free: 2GB storage, Dropbox API v2 (search, list, read).
 * OAuth: Required for user-specific file access.
 * Flow:
 *   1. User taps "Connect Dropbox"
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

const DROPBOX_API = 'https://api.dropboxapi.com/2';
const CONNECTOR_NAME = 'dropbox' as const;

export class DropboxAdapter implements ConnectorAdapter {
  readonly name = CONNECTOR_NAME;
  readonly credentialMode = 'host-mediated' as const;
  readonly metadataSchema = {
    fields: [
      { name: 'query', type: 'string' as const, description: 'Search query for files' },
      { name: 'path', type: 'string' as const, description: 'Folder path to list (default: root)' },
    ],
  };

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['dropbox', 'file', 'document', 'pdf', 'photo', 'attachment'];
    return terms.some((t) => q.includes(t)) ? 0.6 : 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    return { query: query.text || '' };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const filter = ctx.filter as { query?: string; path?: string };
    const results: ConnectorResult['items'] = [];

    try {
      const isSearch = Boolean(filter.query);
      const res = await requestConnector({
        connector: CONNECTOR_NAME,
        userId: ctx.userId,
        request: {
          url: isSearch ? `${DROPBOX_API}/files/search_v2` : `${DROPBOX_API}/files/list_folder`,
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(
            isSearch
              ? { query: filter.query, max_results: 20 }
              : { path: filter.path || '', recursive: false, limit: 20 },
          ),
        },
        ...(ctx.signal ? { signal: ctx.signal } : {}),
      });
      if (!res?.ok) return { items: [], totalCount: 0, source: CONNECTOR_NAME };

      if (isSearch) {
        const data = (await res.json()) as {
          matches?: Array<{
            metadata: {
              metadata: {
                '.tag': string;
                name: string;
                path_lower: string;
                id: string;
                client_modified?: string;
                size?: number;
              };
            };
          }>;
        };
        for (const match of data.matches ?? []) {
          const meta = match.metadata.metadata;
          if (meta['.tag'] === 'file') {
            results.push({
              id: meta.id,
              title: meta.name,
              snippet: `${meta.name} (${formatSize(meta.size ?? 0)})`,
              url: `https://www.dropbox.com/home${meta.path_lower}`,
              ...(meta.client_modified ? { date: meta.client_modified } : {}),
              metadata: { tag: 'file', size: meta.size, path: meta.path_lower },
            });
          }
        }
      } else {
        const data = (await res.json()) as {
          entries?: Array<{
            '.tag': string;
            name: string;
            path_lower: string;
            id: string;
            client_modified?: string;
            size?: number;
          }>;
        };
        for (const entry of data.entries ?? []) {
          results.push({
            id: entry.id,
            title: entry.name,
            snippet: `${entry.name} (${entry['.tag'] === 'folder' ? 'folder' : formatSize(entry.size ?? 0)})`,
            url: `https://www.dropbox.com/home${entry.path_lower}`,
            ...(entry.client_modified ? { date: entry.client_modified } : {}),
            metadata: { tag: entry['.tag'], size: entry.size, path: entry.path_lower },
          });
        }
      }
    } catch {
      return { items: [], totalCount: 0, source: CONNECTOR_NAME };
    }

    return { items: results, totalCount: results.length, source: CONNECTOR_NAME };
  }
}

function formatSize(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
}
