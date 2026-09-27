/**
 * @agentcowork/core-connectors
 *
 * Connector adapters for all supported services.
 *
 * Credentialed connectors use the Rust host transport: the host owns OAuth
 * completion, refresh, vault storage, credential injection, and egress. This
 * package receives only an opaque handle and non-secret provider data.
 */

export type { ConnectorAdapter } from '@agentcowork/core-domain';
export {
  ConnectorOrchestrator,
  type ConnectorPlan,
  type ConnectorExecutionResult,
  type ConnectorExecutionStatus,
  type HostMediatedConnectorAdapter,
} from './orchestrator.js';
export {
  CONNECTOR_CATALOG,
  hasConnectorHostTransport,
  requestConnector,
  setConnectorHostTransport,
  type ConnectionInfo,
  type ConnectorCredentialHandle,
  type ConnectorHostHttpRequest,
  type ConnectorHostRequest,
  type ConnectorHostResponse,
  type ConnectorHostTransport,
  type ConnectorRequestInput,
  type ConnectorStatus,
} from './connection-manager.js';
export {
  ComposioAdapter,
  buildComposioAdapters,
} from './composio-adapter.js';
export {
  COMPOSIO_MANAGED_TOOLKITS,
  type ComposioToolkitEntry,
} from './composio-catalog.js';

// Re-export adapter classes
export { WeatherAdapter } from './adapters/weather-adapter.js';
export { RssAdapter } from './adapters/rss-adapter.js';
export { GitHubAdapter } from './adapters/github-adapter.js';
export { NotionOAuthAdapter } from './adapters/notion-oauth-adapter.js';
export { DropboxAdapter } from './adapters/dropbox-adapter.js';
export { YouTubeAdapter } from './adapters/youtube-adapter.js';
export { GoogleDriveOAuthAdapter } from './adapters/google-drive-oauth-adapter.js';
export { TelegramAdapter } from './adapters/telegram-adapter.js';
export { WikipediaAdapter } from './adapters/wikipedia-adapter.js';
export { HackerNewsAdapter } from './adapters/hacker-news-adapter.js';
export { PublicHolidaysAdapter } from './adapters/public-holidays-adapter.js';
export { NominatimAdapter } from './adapters/nominatim-adapter.js';
export { WorldtimeAdapter } from './adapters/worldtime-adapter.js';
export { IcalAdapter } from './adapters/ical-adapter.js';
export { RestCountriesAdapter } from './adapters/restcountries-adapter.js';
export { MicrosoftGraphAdapter } from './adapters/microsoft-graph-adapter.js';
export { SpotifyAdapter } from './adapters/spotify-adapter.js';
export { RedditAdapter } from './adapters/reddit-adapter.js';
export { TodoistAdapter } from './adapters/todoist-adapter.js';
export { GooglePlacesAdapter } from './adapters/google-places-adapter.js';
export { CoingeckoAdapter } from './adapters/coingecko-adapter.js';
export { StackExchangeAdapter } from './adapters/stackexchange-adapter.js';
export { OpenLibraryAdapter } from './adapters/openlibrary-adapter.js';
export { FinnhubAdapter } from './adapters/finnhub-adapter.js';
export { TrelloAdapter } from './adapters/trello-adapter.js';
export { SlackAdapter } from './adapters/slack-adapter.js';
export { AviationstackAdapter } from './adapters/aviationstack-adapter.js';
export { SoundcloudAdapter } from './adapters/soundcloud-adapter.js';

import { ConnectorOrchestrator } from './orchestrator.js';
import { WeatherAdapter } from './adapters/weather-adapter.js';
import { RssAdapter } from './adapters/rss-adapter.js';
import { GitHubAdapter } from './adapters/github-adapter.js';
import { NotionOAuthAdapter } from './adapters/notion-oauth-adapter.js';
import { DropboxAdapter } from './adapters/dropbox-adapter.js';
import { YouTubeAdapter } from './adapters/youtube-adapter.js';
import { GoogleDriveOAuthAdapter } from './adapters/google-drive-oauth-adapter.js';
import { TelegramAdapter } from './adapters/telegram-adapter.js';
import { WikipediaAdapter } from './adapters/wikipedia-adapter.js';
import { HackerNewsAdapter } from './adapters/hacker-news-adapter.js';
import { PublicHolidaysAdapter } from './adapters/public-holidays-adapter.js';
import { NominatimAdapter } from './adapters/nominatim-adapter.js';
import { WorldtimeAdapter } from './adapters/worldtime-adapter.js';
import { IcalAdapter } from './adapters/ical-adapter.js';
import { RestCountriesAdapter } from './adapters/restcountries-adapter.js';
import { MicrosoftGraphAdapter } from './adapters/microsoft-graph-adapter.js';
import { SpotifyAdapter } from './adapters/spotify-adapter.js';
import { RedditAdapter } from './adapters/reddit-adapter.js';
import { TodoistAdapter } from './adapters/todoist-adapter.js';
import { GooglePlacesAdapter } from './adapters/google-places-adapter.js';
import { CoingeckoAdapter } from './adapters/coingecko-adapter.js';
import { StackExchangeAdapter } from './adapters/stackexchange-adapter.js';
import { OpenLibraryAdapter } from './adapters/openlibrary-adapter.js';
import { FinnhubAdapter } from './adapters/finnhub-adapter.js';
import { TrelloAdapter } from './adapters/trello-adapter.js';
import { SlackAdapter } from './adapters/slack-adapter.js';
import { AviationstackAdapter } from './adapters/aviationstack-adapter.js';
import { SoundcloudAdapter } from './adapters/soundcloud-adapter.js';
import { buildComposioAdapters } from './composio-adapter.js';

export function createDefaultRegistry(): ConnectorOrchestrator {
  const orch = new ConnectorOrchestrator();

  // No-auth public API adapters
  orch.register(new WeatherAdapter());         // Open-Meteo
  orch.register(new RssAdapter());             // User-supplied feed URL
  orch.register(new GitHubAdapter());          // Anonymous public GitHub
  orch.register(new YouTubeAdapter());         // Google API key
  orch.register(new WikipediaAdapter());       // Wikimedia REST
  orch.register(new HackerNewsAdapter());      // Algolia HN search
  orch.register(new PublicHolidaysAdapter());  // Nager.Date
  orch.register(new NominatimAdapter());       // OpenStreetMap
  orch.register(new WorldtimeAdapter());       // WorldTimeAPI
  orch.register(new IcalAdapter());            // User-supplied ICS URL
  orch.register(new RestCountriesAdapter());   // restcountries.com
  orch.register(new CoingeckoAdapter());       // CoinGecko
  orch.register(new StackExchangeAdapter());   // StackOverflow / SE
  orch.register(new OpenLibraryAdapter());     // Internet Archive Open Library
  orch.register(new GooglePlacesAdapter());    // Google Places (needs API key)
  orch.register(new FinnhubAdapter());         // Finnhub finance (API key)

  // Host-mediated adapters receive only an opaque credential handle.
  orch.register(new NotionOAuthAdapter());
  orch.register(new DropboxAdapter());
  orch.register(new GoogleDriveOAuthAdapter());
  // Microsoft Graph: one host-owned account, three sub-services
  orch.register(new MicrosoftGraphAdapter('microsoft-mail', 'mail'));
  orch.register(new MicrosoftGraphAdapter('microsoft-calendar', 'calendar'));
  orch.register(new MicrosoftGraphAdapter('microsoft-onedrive', 'onedrive'));
  // New free OAuth: Spotify, Reddit, Todoist
  orch.register(new SpotifyAdapter());
  orch.register(new RedditAdapter());
  orch.register(new TodoistAdapter());
  // Batch 3 connectors: Slack / SoundCloud OAuth + host-mediated Trello
  orch.register(new SlackAdapter());
  orch.register(new SoundcloudAdapter());
  orch.register(new TrelloAdapter());
  // AviationStack: API key, proxied through Worker because free tier is HTTP
  orch.register(new AviationstackAdapter());

  // Host-mediated bot connector
  orch.register(new TelegramAdapter());

  // Composio managed-auth adapters (Gmail, Calendar, Tasks, Drive, …).
  // Auth + execute go through Worker → GCP; local adapters score relevance
  // and build filters so the orchestrator can plan multi-source turns.
  // Prefer Composio over direct Google sensitive-scope OAuth (CASA).
  try {
    const adapters = buildComposioAdapters();
    for (const adapter of adapters) {
      orch.register(adapter);
    }
  } catch {
    // Composio catalog optional in minimal builds
  }

  return orch;
}
