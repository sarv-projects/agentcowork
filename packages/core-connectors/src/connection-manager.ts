/**
 * ConnectionManager — end-to-end connector lifecycle inspired by Nango's pattern.
 *
 * Flow (per connector):
 *   1. User taps "Connect [Service]" → the host opens the provider OAuth URL
 *   2. User authenticates in the browser → the host completes the callback
 *   3. The host exchanges and stores the credential in the Rust vault
 *   4. TypeScript receives only an opaque handle and delegates the HTTP request
 *
 * The connector HTTP path is host-mediated. A missing host transport disables
 * credentialed connectors; it never falls back to a bearer token in TypeScript.
 *
 * For native-only connectors (Calendar): no OAuth, just permission request → ready.
 */
import type { ConnectorName } from '@agentcowork/core-domain';

export type ConnectorStatus = 'disconnected' | 'connecting' | 'connected' | 'error';

export interface ConnectionInfo {
  connectorId: string;
  label: string;
  icon: string;
  status: ConnectorStatus;
  /** User-readable status message */
  message: string;
  /** Whether this connector requires OAuth (server-side flow) */
  requiresOAuth: boolean;
  /** Whether this connector is native-only (no server) */
  isNative: boolean;
  /** OAuth provider name for the host-managed redirect */
  oauthProvider?: string;
  /** API cost tier */
  cost: 'free' | 'free-tier' | 'paid';
  /** Category for grouped browsing (Productivity, Developer, Communication, etc.) */
  category: string;
  /** Optional badge: 'new' | 'interactive' */
  badge?: 'new' | 'interactive';
}

/**
 * Non-secret metadata for a vault-owned connector credential.
 *
 * `handle` is an opaque Rust-owned reference (`vault:*` or `cred:*`), never
 * the credential itself. The host resolves it inside the vault/Auth Bridge and
 * injects the appropriate provider authentication before egress.
 */
export interface ConnectorCredentialHandle {
  readonly handle: string;
  readonly provider: string;
  readonly expiresAtMs?: number;
}

export interface ConnectorHostHttpRequest {
  readonly url: string;
  readonly method: 'GET' | 'POST';
  readonly headers?: Readonly<Record<string, string>>;
  readonly body?: string;
}

export interface ConnectorHostRequest {
  readonly connector: ConnectorName;
  readonly userId: string;
  readonly credential: ConnectorCredentialHandle;
  readonly request: ConnectorHostHttpRequest;
  readonly signal?: AbortSignal;
}

export interface ConnectorHostResponse {
  readonly ok: boolean;
  readonly status: number;
  json(): Promise<unknown>;
  text(): Promise<string>;
}

/**
 * Rust-host seam for connector HTTP calls.
 *
 * The host owns credential resolution, refresh, and provider authentication.
 * The response contains provider data only; the request crossing into this
 * interface must not contain an Authorization header, API key, token, or
 * password in its URL/body/headers.
 */
export interface ConnectorHostTransport {
  resolveCredential(
    connector: ConnectorName,
    userId: string,
  ): Promise<ConnectorCredentialHandle | null>;
  request(req: ConnectorHostRequest): Promise<ConnectorHostResponse>;
}

export interface ConnectorRequestInput {
  readonly connector: ConnectorName;
  readonly userId: string;
  readonly request: ConnectorHostHttpRequest;
  readonly signal?: AbortSignal;
}

let activeConnectorHostTransport: ConnectorHostTransport | null = null;

/** Attach the Rust connector transport; `null` safely disables credentialed connectors. */
export function setConnectorHostTransport(transport: ConnectorHostTransport | null): void {
  activeConnectorHostTransport = transport;
}

/** Whether a host-mediated connector transport is currently attached. */
export function hasConnectorHostTransport(): boolean {
  return activeConnectorHostTransport !== null;
}

const CREDENTIAL_QUERY_KEYS = new Set([
  'accesstoken',
  'accesskey',
  'apikey',
  'authorization',
  'key',
  'password',
  'secret',
  'token',
]);

function normalizedCredentialKey(key: string): string {
  return key.toLowerCase().replace(/[^a-z0-9]/g, '');
}

function isOpaqueCredentialHandle(handle: string): boolean {
  return /^(?:vault|cred):\S+$/.test(handle);
}

function isCredentialFreeHttpRequest(request: ConnectorHostHttpRequest): boolean {
  for (const name of Object.keys(request.headers ?? {})) {
    const normalized = normalizedCredentialKey(name);
    if (
      normalized === 'authorization' ||
      normalized === 'cookie' ||
      normalized === 'setcookie' ||
      normalized.includes('apikey') ||
      normalized.includes('accesstoken') ||
      normalized.includes('password') ||
      normalized.includes('secret') ||
      normalized.includes('token')
    ) {
      return false;
    }
  }

  try {
    const url = new URL(request.url);
    for (const name of url.searchParams.keys()) {
      if (CREDENTIAL_QUERY_KEYS.has(normalizedCredentialKey(name))) return false;
    }
  } catch {
    return false;
  }

  return true;
}

/**
 * Execute a connector request through the attached Rust host.
 *
 * `null` means no safe execution path is available (transport absent, connector
 * disconnected, invalid handle, or credential-bearing request fields). Adapters
 * treat that as unavailable; there is deliberately no raw-token fallback.
 */
export async function requestConnector(
  input: ConnectorRequestInput,
): Promise<ConnectorHostResponse | null> {
  const transport = activeConnectorHostTransport;
  if (!transport || !isCredentialFreeHttpRequest(input.request)) return null;

  try {
    const credential = await transport.resolveCredential(input.connector, input.userId);
    if (!credential || !isOpaqueCredentialHandle(credential.handle)) return null;

    return await transport.request({
      connector: input.connector,
      userId: input.userId,
      credential,
      request: input.request,
      ...(input.signal ? { signal: input.signal } : {}),
    });
  } catch {
    // Never surface/log a host error: a misbehaving bridge must not turn a
    // credential or provider response into sidecar diagnostics.
    return null;
  }
}

export const CONNECTOR_CATALOG: ConnectionInfo[] = [
  {
    connectorId: 'calendar-native',
    label: 'Calendar',
    icon: 'calendar',
    status: 'disconnected',
    message: 'Access device calendar events',
    requiresOAuth: false,
    isNative: true,
    cost: 'free',
    category: 'Calendar',
  },
  {
    connectorId: 'health-native',
    label: 'Health',
    icon: 'pulse-outline',
    status: 'disconnected',
    message: 'Read steps & heart rate from Health Connect',
    requiresOAuth: false,
    isNative: true,
    cost: 'free',
    category: 'Health',
  },
  {
    connectorId: 'contacts-native',
    label: 'Contacts',
    icon: 'people',
    status: 'disconnected',
    message: 'Search your device contacts',
    requiresOAuth: false,
    isNative: true,
    cost: 'free',
    category: 'Productivity',
  },
  {
    connectorId: 'location-native',
    label: 'Location',
    icon: 'location',
    status: 'disconnected',
    message: 'Get current GPS location',
    requiresOAuth: false,
    isNative: true,
    cost: 'free',
    category: 'Utilities',
  },
  {
    connectorId: 'youtube',
    label: 'YouTube',
    icon: 'logo-youtube',
    status: 'disconnected',
    message: 'Search videos, get channel info',
    requiresOAuth: false,
    isNative: false,
    cost: 'free-tier',
    category: 'Entertainment',
  },
  {
    connectorId: 'notion',
    label: 'Notion',
    icon: 'document-text',
    status: 'disconnected',
    message: 'Read and search your Notion workspace',
    requiresOAuth: true,
    isNative: false,
    oauthProvider: 'notion',
    cost: 'free',
    category: 'Productivity',
  },
  {
    connectorId: 'dropbox',
    label: 'Dropbox',
    icon: 'cloud',
    status: 'disconnected',
    message: 'Access files in your Dropbox',
    requiresOAuth: true,
    isNative: false,
    oauthProvider: 'dropbox',
    cost: 'free-tier',
    category: 'Storage',
  },
  {
    connectorId: 'google-drive',
    label: 'Google Drive',
    icon: 'cloud-done',
    status: 'disconnected',
    message: 'Search and read Google Drive files',
    requiresOAuth: true,
    isNative: false,
    oauthProvider: 'google-drive',
    cost: 'free-tier',
    category: 'Storage',
  },
  {
    connectorId: 'github',
    label: 'GitHub',
    icon: 'logo-github',
    status: 'disconnected',
    message: 'Search repos, read code, manage issues',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'Developer',
  },
  {
    connectorId: 'weather',
    label: 'Weather',
    icon: 'partly-sunny',
    status: 'connected',
    message: 'Weather forecasts (auto-configured)',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'Utilities',
  },
  {
    connectorId: 'rss',
    label: 'RSS Feeds',
    icon: 'newspaper',
    status: 'disconnected',
    message: 'Fetch articles from any RSS/Atom feed',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'News',
  },
  {
    connectorId: 'wikipedia',
    label: 'Wikipedia',
    icon: 'book',
    status: 'connected',
    message: 'Search & summarize Wikipedia articles',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'Knowledge',
  },
  {
    connectorId: 'hacker-news',
    label: 'Hacker News',
    icon: 'flame',
    status: 'connected',
    message: 'Trending tech stories from Hacker News',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'News',
  },
  {
    connectorId: 'public-holidays',
    label: 'Public Holidays',
    icon: 'calendar-clear',
    status: 'connected',
    message: 'Look up public holidays for any country',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'Utilities',
  },
  {
    connectorId: 'nominatim',
    label: 'Geocoding',
    icon: 'location',
    status: 'connected',
    message: 'Address ↔ coordinates via OpenStreetMap',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'Utilities',
  },
  {
    connectorId: 'worldtime',
    label: 'World Time',
    icon: 'globe',
    status: 'connected',
    message: 'Time & timezone lookups',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'Utilities',
  },
  {
    connectorId: 'ical',
    label: 'iCal / ICS',
    icon: 'calendar',
    status: 'disconnected',
    message: 'Subscribe to any public ICS calendar feed',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'Calendar',
  },
  {
    connectorId: 'restcountries',
    label: 'Country Info',
    icon: 'flag',
    status: 'connected',
    message: 'Country, capital, currency & language lookup',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'Knowledge',
  },
  {
    connectorId: 'microsoft-mail',
    label: 'Outlook Mail',
    icon: 'mail',
    status: 'disconnected',
    message: 'Search and read your Outlook inbox',
    requiresOAuth: true,
    isNative: false,
    oauthProvider: 'microsoft-graph',
    cost: 'free-tier',
    category: 'Communication',
  },
  {
    connectorId: 'microsoft-calendar',
    label: 'Outlook Calendar',
    icon: 'calendar',
    status: 'disconnected',
    message: 'Read your Outlook / Microsoft 365 calendar events',
    requiresOAuth: true,
    isNative: false,
    oauthProvider: 'microsoft-graph',
    cost: 'free-tier',
    category: 'Calendar',
  },
  {
    connectorId: 'microsoft-onedrive',
    label: 'OneDrive',
    icon: 'cloud',
    status: 'disconnected',
    message: 'Browse and search your OneDrive files',
    requiresOAuth: true,
    isNative: false,
    oauthProvider: 'microsoft-graph',
    cost: 'free-tier',
    category: 'Storage',
  },
  {
    connectorId: 'spotify',
    label: 'Spotify',
    icon: 'musical-notes',
    status: 'disconnected',
    message: 'Search tracks, artists, albums, playlists',
    requiresOAuth: true,
    isNative: false,
    oauthProvider: 'spotify',
    cost: 'free-tier',
    category: 'Entertainment',
  },
  {
    connectorId: 'reddit',
    label: 'Reddit',
    icon: 'logo-reddit',
    status: 'disconnected',
    message: 'Search Reddit discussions and threads',
    requiresOAuth: true,
    isNative: false,
    oauthProvider: 'reddit',
    cost: 'free',
    category: 'Social',
  },
  {
    connectorId: 'todoist',
    label: 'Todoist',
    icon: 'briefcase',
    status: 'disconnected',
    message: 'View and create Todoist tasks',
    requiresOAuth: true,
    isNative: false,
    oauthProvider: 'todoist',
    cost: 'free',
    category: 'Productivity',
  },
  {
    connectorId: 'google-places',
    label: 'Google Places',
    icon: 'location',
    status: 'disconnected',
    message: 'Search POIs, restaurants, shops nearby',
    requiresOAuth: false,
    isNative: false,
    cost: 'free-tier',
    category: 'Utilities',
  },
  {
    connectorId: 'coingecko',
    label: 'Crypto Prices',
    icon: 'trending-up',
    status: 'connected',
    message: 'Live crypto prices & market cap',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'Finance',
  },
  {
    connectorId: 'stackexchange',
    label: 'StackExchange',
    icon: 'code-slash',
    status: 'connected',
    message: 'Search Stack Overflow Q&A',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'Developer',
  },
  {
    connectorId: 'openlibrary',
    label: 'Open Library',
    icon: 'library',
    status: 'connected',
    message: 'Search books, authors, ISBNs',
    requiresOAuth: false,
    isNative: false,
    cost: 'free',
    category: 'Knowledge',
  },
  {
    connectorId: 'finnhub',
    label: 'Stocks & Markets',
    icon: 'trending-up',
    status: 'disconnected',
    message: 'Real-time stock, forex & ETF quotes',
    requiresOAuth: false,
    isNative: false,
    cost: 'free-tier',
    category: 'Finance',
  },
  {
    connectorId: 'trello',
    label: 'Trello',
    icon: 'briefcase',
    status: 'disconnected',
    message: 'Read your Trello boards, lists & cards',
    requiresOAuth: true,
    isNative: false,
    oauthProvider: 'trello',
    cost: 'free',
    category: 'Productivity',
  },
  {
    connectorId: 'slack',
    label: 'Slack',
    icon: 'chatbubbles',
    status: 'disconnected',
    message: 'Search messages, summarise DMs and channels',
    requiresOAuth: true,
    isNative: false,
    oauthProvider: 'slack',
    cost: 'free-tier',
    category: 'Communication',
  },
  {
    connectorId: 'aviationstack',
    label: 'Flight Status',
    icon: 'airplane',
    status: 'connected',
    message: 'Live flight tracking & delays',
    requiresOAuth: false,
    isNative: false,
    cost: 'free-tier',
    category: 'Travel',
  },
  {
    connectorId: 'soundcloud',
    label: 'SoundCloud',
    icon: 'musical-note',
    status: 'disconnected',
    message: 'Search tracks, artists & genres',
    requiresOAuth: true,
    isNative: false,
    oauthProvider: 'soundcloud',
    cost: 'free-tier',
    category: 'Entertainment',
  },
  {
    connectorId: 'composio-gmail',
    label: 'Gmail',
    icon: 'mail',
    status: 'disconnected',
    message: 'Read, search, and send emails via Gmail',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Communication',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-google-drive',
    label: 'Google Drive',
    icon: 'cloud-done',
    status: 'disconnected',
    message: 'Search and read files from Google Drive',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Storage',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-google-calendar',
    label: 'Google Calendar',
    icon: 'calendar',
    status: 'disconnected',
    message: 'Read and manage Google Calendar events',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Calendar',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-outlook',
    label: 'Outlook Mail',
    icon: 'mail',
    status: 'disconnected',
    message: 'Read and send emails via Outlook',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Communication',
  },
  {
    connectorId: 'composio-instagram',
    label: 'Instagram',
    icon: 'logo-instagram',
    status: 'disconnected',
    message: 'Read Instagram posts and insights',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Social',
  },
  {
    connectorId: 'composio-slack',
    label: 'Slack',
    icon: 'logo-slack',
    status: 'disconnected',
    message: 'Read and send Slack messages',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Communication',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-notion',
    label: 'Notion',
    icon: 'document-text',
    status: 'disconnected',
    message: 'Read and search Notion workspace',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Productivity',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-github',
    label: 'GitHub',
    icon: 'logo-github',
    status: 'disconnected',
    message: 'Search repos, read code, manage issues',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Developer',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-trello',
    label: 'Trello',
    icon: 'layers',
    status: 'disconnected',
    message: 'Read and manage Trello boards',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Productivity',
  },
  {
    connectorId: 'composio-dropbox',
    label: 'Dropbox',
    icon: 'cloud',
    status: 'disconnected',
    message: 'Access files in Dropbox',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Storage',
  },
  {
    connectorId: 'composio-spotify',
    label: 'Spotify',
    icon: 'musical-notes',
    status: 'disconnected',
    message: 'Search tracks, artists, and playlists',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Entertainment',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-reddit',
    label: 'Reddit',
    icon: 'logo-reddit',
    status: 'disconnected',
    message: 'Search Reddit discussions',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Social',
  },
  {
    connectorId: 'composio-todoist',
    label: 'Todoist',
    icon: 'briefcase',
    status: 'disconnected',
    message: 'Manage Todoist tasks and projects',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Productivity',
  },
  {
    connectorId: 'composio-facebook',
    label: 'Facebook',
    icon: 'logo-facebook',
    status: 'disconnected',
    message: 'Read Facebook pages and posts',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Social',
  },
  {
    connectorId: 'composio-linkedin',
    label: 'LinkedIn',
    icon: 'logo-linkedin',
    status: 'disconnected',
    message: 'Read LinkedIn profile and posts',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Social',
  },
  {
    connectorId: 'composio-canva',
    label: 'Canva',
    icon: 'color-palette',
    status: 'disconnected',
    message: 'Create and edit Canva designs',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Design',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-google-sheets',
    label: 'Google Sheets',
    icon: 'grid',
    status: 'disconnected',
    message: 'Read and write Google Sheets data',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Productivity',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-google-docs',
    label: 'Google Docs',
    icon: 'document-text',
    status: 'disconnected',
    message: 'Read and create Google Docs',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Productivity',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-google-tasks',
    label: 'Google Tasks',
    icon: 'checkbox',
    status: 'disconnected',
    message: 'Manage Google Tasks and to-do lists',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Productivity',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-onedrive',
    label: 'OneDrive',
    icon: 'cloud',
    status: 'disconnected',
    message: 'Browse and search OneDrive files',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Storage',
  },
  {
    connectorId: 'composio-teams',
    label: 'Microsoft Teams',
    icon: 'people',
    status: 'disconnected',
    message: 'Read and send Teams messages',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Communication',
  },
  {
    connectorId: 'composio-discord',
    label: 'Discord',
    icon: 'logo-discord',
    status: 'disconnected',
    message: 'Read and send Discord messages',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Communication',
  },
  {
    connectorId: 'composio-clickup',
    label: 'ClickUp',
    icon: 'fitness',
    status: 'disconnected',
    message: 'Manage ClickUp tasks and spaces',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Productivity',
  },
  {
    connectorId: 'composio-gitlab',
    label: 'GitLab',
    icon: 'git-branch',
    status: 'disconnected',
    message: 'Read and manage GitLab projects',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Developer',
  },
  {
    connectorId: 'composio-linear',
    label: 'Linear',
    icon: 'pulse',
    status: 'disconnected',
    message: 'Manage Linear issues and projects',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Productivity',
  },
  {
    connectorId: 'composio-browserbase',
    label: 'Browserbase',
    icon: 'globe',
    status: 'disconnected',
    message: 'Cloud browser sessions for agent web interaction',
    requiresOAuth: true,
    isNative: false,
    cost: 'free-tier',
    category: 'Automation',
  },
  {
    connectorId: 'composio-zapier',
    label: 'Zapier',
    icon: 'flash',
    status: 'disconnected',
    message: 'Thousands of SaaS actions via Zapier',
    requiresOAuth: true,
    isNative: false,
    cost: 'free-tier',
    category: 'Automation',
  },
  {
    connectorId: 'composio-hubspot',
    label: 'HubSpot',
    icon: 'share',
    status: 'disconnected',
    message: 'Manage HubSpot contacts and deals',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Sales',
  },
  {
    connectorId: 'composio-salesforce',
    label: 'Salesforce',
    icon: 'trending-up',
    status: 'disconnected',
    message: 'Read and manage Salesforce data',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Sales',
  },
  {
    connectorId: 'composio-zoom',
    label: 'Zoom',
    icon: 'videocam',
    status: 'disconnected',
    message: 'Schedule and manage Zoom meetings',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Communication',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-box',
    label: 'Box',
    icon: 'cube',
    status: 'disconnected',
    message: 'Access files in Box',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Storage',
  },

  {
    connectorId: 'composio-figma',
    label: 'Figma',
    icon: 'color-palette',
    status: 'disconnected',
    message: 'Generate diagrams and better code from Figma context',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Design',
    badge: 'interactive',
  },
  {
    connectorId: 'composio-jira',
    label: 'Jira',
    icon: 'bug',
    status: 'disconnected',
    message: 'Create, update, comment, and search issues',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Project Management',
  },
  {
    connectorId: 'composio-confluence',
    label: 'Confluence',
    icon: 'book',
    status: 'disconnected',
    message: 'Create and manage Confluence pages and spaces',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Knowledge',
  },
  {
    connectorId: 'composio-calendly',
    label: 'Calendly',
    icon: 'calendar',
    status: 'disconnected',
    message: 'Schedule meetings and manage availability',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Calendar',
  },
  {
    connectorId: 'composio-asana',
    label: 'Asana',
    icon: 'list',
    status: 'disconnected',
    message: 'Manage Asana tasks, projects, and workspaces',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Project Management',
  },
  {
    connectorId: 'composio-airtable',
    label: 'Airtable',
    icon: 'grid',
    status: 'disconnected',
    message: 'Read and write Airtable records and bases',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Productivity',
  },
  {
    connectorId: 'composio-monday',
    label: 'Monday',
    icon: 'calendar',
    status: 'disconnected',
    message: 'Manage Monday boards, items, and workflows',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Project Management',
  },
  {
    connectorId: 'composio-miro',
    label: 'Miro',
    icon: 'albums',
    status: 'disconnected',
    message: 'Create and edit Miro boards and sticky notes',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Design',
  },
  {
    connectorId: 'composio-twitter',
    label: 'Twitter / X',
    icon: 'logo-twitter',
    status: 'disconnected',
    message: 'Post tweets, search, and manage your feed',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Social',
  },
  {
    connectorId: 'composio-sentry',
    label: 'Sentry',
    icon: 'warning',
    status: 'disconnected',
    message: 'Monitor errors, issues, and performance',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Developer',
  },
  {
    connectorId: 'composio-pagerduty',
    label: 'PagerDuty',
    icon: 'notifications',
    status: 'disconnected',
    message: 'Manage incidents, schedules, and alerts',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Developer',
  },
  {
    connectorId: 'composio-shopify',
    label: 'Shopify',
    icon: 'cart',
    status: 'disconnected',
    message: 'Manage products, orders, and customers',
    requiresOAuth: true,
    isNative: false,
    cost: 'free',
    category: 'Commerce',
  },
];

/**
 * OAuth cost analysis per connector:
 *
 * google-drive:   FREE - 15GB storage, 10k API req/day, OAuth required
 * dropbox:        FREE - 2GB storage, basic API quota, OAuth required
 * notion:         FREE - Internal integration (no OAuth), Public (OAuth)
 * youtube:        FREE - 10k units/day, API key only, NO OAuth needed
 * spotify:        FREE - Web API with OAuth, rate-limited per user
 * slack:          FREE - Tier 3 (50+ req/min) read-only scopes
 * soundcloud:     FREE - 15k/day OAuth read-only
 * trello:         FREE - 300 req/10s OAuth or personal-token
 * discord:        FREE - Bot token, no OAuth for bot
 *
 * What requires payment:
 * - google-drive > 15GB storage or > 10k req/day
 * - dropbox > 2GB storage or Dropbox Business features
 * - notion > 1k collaborators or API rate limit increase
 * - youtube > 10k units/day (need quota increase request)
 * - spotify > rate limit increase
 * - aviationstack > 100 req/month (free tier)
 * - finnhub > 60 req/min
 */
