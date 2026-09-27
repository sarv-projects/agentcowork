/**
 * ComposioAdapter — wraps Composio SDK for server-side connector management.
 *
 * This adapter runs on the GCP Cloud Run server (Node.js), NOT on the mobile client.
 * The mobile app calls Cloudflare Worker proxy → GCP Cloud Run → Composio SDK.
 *
 * Flow:
 * 1. Mobile calls CF Worker /v1/connectors/composio/authorize?toolkit=GMAIL
 * 2. CF Worker proxies to GCP Cloud Run /v1/composio/authorize
 * 3. GCP calls session.authorize('GMAIL') → returns Connect Link URL
 * 4. User opens Connect Link → authenticates with Google
 * 5. Composio stores token → redirects back to app
 * 6. App calls /v1/connectors/composio/status to verify
 *
 * For tool execution:
 * 1. Mobile calls CF Worker /v1/connectors/composio/execute
 * 2. CF Worker proxies to GCP Cloud Run /v1/composio/execute
 * 3. GCP calls session.execute(toolSlug, args) → returns result
 */

import type {
  ConnectorAdapter,
  ConnectorContext,
  ConnectorResult,
  ConnectorMetadataSchema,
  ConnectorName,
  UserQuery,
  MemoryFact,
} from '@agentcowork/core-domain';

/**
 * ComposioAdapter — implements ConnectorAdapter interface for Composio-managed toolkits.
 *
 * Unlike direct adapters, this one delegates ALL auth and API calls to Composio.
 * The adapter itself is thin — it mainly maps between Personal AI's connector
 * model and Composio's session/tool model.
 */
export class ComposioAdapter implements ConnectorAdapter {
  readonly name: ConnectorName;
  readonly metadataSchema: ConnectorMetadataSchema = { fields: [] };
  private toolkit: string;
  private connectorId: string;

  constructor(
    name: ConnectorName,
    _displayName: string,
    toolkit: string,
    connectorId: string,
  ) {
    this.name = name;
    this.toolkit = toolkit;
    this.connectorId = connectorId;
  }

  /**
   * Check if a user has authorized this Composio toolkit.
   * In practice, this checks via the Composio API server-side.
   */
  async isAuthorized(_userId: string): Promise<boolean> {
    // This is checked server-side via Composio SDK
    // The mobile app caches connection status locally
    return true; // Optimistic — actual check happens on execute
  }

  /**
   * Score relevance of a query for this connector.
   * Simple keyword matching — Composio tools handle the real routing.
   */
  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const text = (query.text || '').toLowerCase();
    const keywords = this.getKeywords();
    let score = 0;
    for (const kw of keywords) {
      if (text.includes(kw)) score += 0.3;
    }
    return Math.min(score, 1.0);
  }

  /**
   * Build filter for this connector based on the query.
   * Composio handles the actual API call — this is just metadata.
   */
  buildFilter(query: UserQuery): Record<string, unknown> {
    return {
      toolkit: this.toolkit,
      query: query.text,
      connectorId: this.connectorId,
    };
  }

  /**
   * Execute a Composio search-style tool via the Worker proxy → GCP.
   * Mobile / orchestrator path: POST /v1/connectors/composio/execute
   * with toolkit + query; server runs Composio session.execute.
   */
  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const base =
      (typeof process !== 'undefined' &&
        (process.env?.EXPO_PUBLIC_WORKER_BASE || process.env?.EXPO_PUBLIC_API_BASE)) ||
      (ctx.filter?.proxyBase as string | undefined) ||
      '';
    if (!base) {
      // No proxy configured — return empty (server-side SDK path will handle real execute).
      return { items: [], totalCount: 0, source: this.name };
    }
    try {
      const url = `${String(base).replace(/\/+$/, '')}/v1/connectors/composio/execute`;
      const init: RequestInit = {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'x-user-id': ctx.userId || 'mobile',
          'x-device-id': String(ctx.filter?.deviceId ?? ctx.userId ?? 'mobile'),
        },
        body: JSON.stringify({
          toolkit: this.toolkit,
          connectorId: this.connectorId,
          action: 'search',
          args: {
            query: ctx.query?.text ?? '',
            ...(ctx.filter ?? {}),
          },
        }),
      };
      if (ctx.signal) init.signal = ctx.signal;
      const res = await fetch(url, init);
      if (!res.ok) {
        return { items: [], totalCount: 0, source: this.name };
      }
      const data = (await res.json()) as {
        items?: Array<{ id?: string; title?: string; snippet?: string; url?: string }>;
        result?: { data?: unknown; items?: Array<{ id?: string; title?: string; snippet?: string; url?: string }> };
      };
      const rawItems = data.items ?? data.result?.items ?? [];
      const items = rawItems.map((item, i) => {
        const row: {
          id: string;
          title: string;
          snippet: string;
          url?: string;
        } = {
          id: item.id ?? `${this.connectorId}-${i}`,
          title: item.title ?? this.connectorId,
          snippet: item.snippet ?? '',
        };
        if (item.url) row.url = item.url;
        return row;
      });
      return { items, totalCount: items.length, source: this.name };
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }

  /**
   * Token refresh is handled by Composio automatically.
   */
  async refreshToken(_userId: string): Promise<boolean> {
    return true;
  }

  private getKeywords(): string[] {
    const keywordMap: Record<string, string[]> = {
      'composio-gmail': ['email', 'gmail', 'inbox', 'mail', 'send email', 'read email'],
      'composio-google-drive': ['drive', 'google drive', 'files', 'cloud storage'],
      'composio-google-calendar': ['calendar', 'event', 'meeting', 'schedule', 'appointment'],
      'composio-google-sheets': ['spreadsheet', 'sheets', 'google sheets', 'data'],
      'composio-google-docs': ['document', 'google docs', 'doc', 'write'],
      'composio-google-tasks': ['task', 'todo', 'google tasks', 'checklist'],
      'composio-outlook': ['outlook', 'microsoft email', 'office 365'],
      'composio-onedrive': ['onedrive', 'microsoft cloud', 'office files'],
      'composio-teams': ['teams', 'microsoft teams', 'team chat'],
      'composio-instagram': ['instagram', 'ig', 'insta', 'post', 'story'],
      'composio-facebook': ['facebook', 'fb', 'meta'],
      'composio-linkedin': ['linkedin', 'professional network', 'connections'],
      'composio-slack': ['slack', 'channel', 'workspace'],
      'composio-reddit': ['reddit', 'subreddit', 'thread', 'post'],
      'composio-discord': ['discord', 'server', 'channel'],
      'composio-notion': ['notion', 'workspace', 'wiki', 'knowledge base'],
      'composio-todoist': ['todoist', 'task', 'todo', 'project'],
      'composio-trello': ['trello', 'board', 'card', 'kanban'],
      'composio-clickup': ['clickup', 'task', 'space', 'sprint'],
      'composio-dropbox': ['dropbox', 'file', 'cloud storage'],
      'composio-github': ['github', 'repo', 'repository', 'code', 'issue', 'pr'],
      'composio-browserbase': ['browser', 'browserbase', 'cloud browser', 'web session'],
      'composio-zapier': ['zapier', 'automation', 'zap', 'workflow'],
      'composio-gitlab': ['gitlab', 'repo', 'pipeline', 'merge request'],
      'composio-linear': ['linear', 'issue', 'project', 'sprint'],
      'composio-canva': ['canva', 'design', 'graphic', 'poster', 'presentation'],
      'composio-spotify': ['spotify', 'music', 'song', 'playlist', 'artist'],
      'composio-hubspot': ['hubspot', 'crm', 'contact', 'deal', 'lead'],
      'composio-salesforce': ['salesforce', 'crm', 'opportunity', 'account'],
      'composio-zoom': ['zoom', 'meeting', 'webinar', 'conference'],
      'composio-box': ['box', 'cloud storage', 'enterprise files'],
    };
    return keywordMap[this.connectorId] || [];
  }
}

/**
 * Build ComposioAdapter instances from the catalog.
 */
export function buildComposioAdapters(): ComposioAdapter[] {
  // Lazy import to avoid circular deps
  const { COMPOSIO_MANAGED_TOOLKITS } = require('./composio-catalog');
  return COMPOSIO_MANAGED_TOOLKITS.map(
    (entry: { toolkit: string; connectorId: string; label: string }) =>
      new ComposioAdapter(
        entry.connectorId as ConnectorName,
        entry.label,
        entry.toolkit,
        entry.connectorId,
      ),
  );
}
