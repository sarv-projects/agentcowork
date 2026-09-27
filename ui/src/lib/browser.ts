// P11.5.3 — browse view over a real CDP session (browser_cmds.rs). The Rust
// side spawns a headless Chrome, connects through agentcowork-cdp and holds
// the session; these calls drive it. Without the shell, the demo fallback
// serves a canned page snapshot.

import { invoke, inTauri } from './tauri'
import { nativeCall } from './runtime'

export type BrowserChannel =
  | 'auto'
  | 'brave'
  | 'chrome'
  | 'edge'
  | 'chromium'
  | 'arc'
  | 'vivaldi'
  | 'custom'

export type CandidateSource =
  | 'system_path'
  | 'windows_app_paths'
  | 'standard_program_files'
  | 'local_app_data'
  | 'managed_cft'
  | 'user_config'

export interface BrowserCandidate {
  channel: BrowserChannel
  name: string
  executable_path: string
  version?: string | null
  is_default: boolean
  source: CandidateSource
}

export type BrowserProfileMode = 'isolated' | 'paired'

export interface BrowserConfig {
  preferred_channel: BrowserChannel
  custom_executable_path?: string | null
  profile_mode: BrowserProfileMode
  headless: boolean
  extra_args: string[]
}

export interface BrowserStatus {
  attached: boolean
  url?: string
  /** P55.7 — the engine serving the interactive session (always `chrome`: a
   * scripted session needs a full engine). Reported, never assumed. */
  engine?: string
  channel?: BrowserChannel
  name?: string
  version?: string | null
}

export async function browserListInstalled(): Promise<BrowserCandidate[]> {
  if (!inTauri()) {
    return [
      {
        channel: 'chrome',
        name: 'Google Chrome (Demo)',
        executable_path: '/usr/bin/google-chrome',
        version: '128.0.6613.119',
        is_default: true,
        source: 'system_path',
      },
      {
        channel: 'brave',
        name: 'Brave Browser (Demo)',
        executable_path: '/usr/bin/brave-browser',
        version: '128.1.69.153',
        is_default: false,
        source: 'system_path',
      },
    ]
  }
  return nativeCall('browser list installed', () =>
    invoke<BrowserCandidate[]>('browser_list_installed')
  )
}

export async function browserGetConfig(): Promise<BrowserConfig> {
  if (!inTauri()) {
    return {
      preferred_channel: 'auto',
      custom_executable_path: null,
      profile_mode: 'isolated',
      headless: true,
      extra_args: ['--mute-audio'],
    }
  }
  return nativeCall('browser get config', () =>
    invoke<BrowserConfig>('browser_get_config')
  )
}

export async function browserSetConfig(config: BrowserConfig): Promise<BrowserConfig> {
  if (!inTauri()) return config
  return nativeCall('browser set config', () =>
    invoke<BrowserConfig>('browser_set_config', { config })
  )
}

export async function browserStart(): Promise<BrowserStatus> {
  if (!inTauri()) return { attached: false }
  return nativeCall('browser start', () => invoke<BrowserStatus>('browser_start'))
}

/** P55.7 — one read through the E10 tiered engine stack (static extraction →
 * Lightpanda/Obscura → Chrome), reporting the tier that actually served it. */
export interface TieredRead {
  url: string
  tier: 'static' | 'lightpanda' | 'obscura' | 'chrome'
  source: string
  truncated: boolean
  text: string
}

export async function browserReadUrl(url: string, needsJs = false): Promise<TieredRead> {
  if (!inTauri()) {
    return {
      url,
      tier: 'static',
      source: 'plain_html',
      truncated: false,
      text: `# Demo read\n\nStart the app shell to fetch ${url} through the tiered engine stack.`,
    }
  }
  return nativeCall('browser tiered read', () =>
    invoke<TieredRead>('browser_read_url', { url, needsJs }));
}

/** Human label for the tier that served a read — used verbatim in the UI so
 * the surface can never imply a light engine ran when Chrome did. */
export function tierLabel(tier: TieredRead['tier']): string {
  switch (tier) {
    case 'static':
      return 'static fetch (no browser launched)'
    case 'lightpanda':
      return 'Lightpanda (tier 1)'
    case 'obscura':
      return 'Obscura (tier 1)'
    case 'chrome':
      return 'Chrome (tier 2)'
  }
}

export async function browserNavigate(url: string): Promise<{ url: string }> {
  if (!inTauri()) return { url }
  return nativeCall('browser navigate', () => invoke('browser_navigate', { url }))
}

export async function browserSnapshot(): Promise<{ url: string; documentId: string; text: string }> {
  if (!inTauri()) return demoSnapshot()
  return nativeCall('browser snapshot', () => invoke('browser_snapshot'))
}

export async function browserRead(): Promise<{ url: string; text: string }> {
  if (!inTauri()) return { url: 'about:blank', text: '# Demo page\n\nStart the browser from the shell to browse the live web.' }
  return nativeCall('browser read', () => invoke('browser_read'))
}

export async function browserClick(refId: string): Promise<{ ok: boolean; added: string[]; removed: string[] }> {
  if (!inTauri()) return { ok: true, added: [], removed: [] }
  return nativeCall('browser click', () => invoke('browser_click', { refId }))
}

export async function browserType(refId: string | null, text: string): Promise<{ ok: boolean }> {
  if (!inTauri()) return { ok: true }
  return nativeCall('browser type', () => invoke('browser_type', { refId, text }))
}

export async function browserStop(): Promise<{ stopped: boolean }> {
  if (!inTauri()) return { stopped: false }
  return nativeCall('browser stop', () => invoke('browser_stop'))
}

export async function browserStatus(): Promise<BrowserStatus> {
  if (!inTauri()) return { attached: false }
  return nativeCall('browser status', () => invoke<BrowserStatus>('browser_status'))
}

function demoSnapshot() {
  return {
    url: 'about:blank',
    documentId: 'demo',
    text: [
      'webArea Demo page',
      '  heading Start browsing',
      '  paragraph Start the browser from the toolbar — the snapshot below will be the real accessibility tree.',
      '  link example.com [ref=e1]',
    ].join('\n'),
  }
}
