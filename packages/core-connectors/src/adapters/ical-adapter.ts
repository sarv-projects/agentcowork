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

/**
 * iCal / ICS calendar connector — parses any user-provided public ICS feed URL.
 *
 * 100% free in the sense of "no app registration" — the user pastes a public
 * iCal URL (Google Calendar public URL, Outlook 365 webcal, Apple iCloud
 * shared, holiday ICS, etc.). The fetch is done from user-supplied URL.
 *
 * We DO NOT scrape — fetch only URL prefix-validated to https:// + .
 * VEVENT extraction is done locally (no external parser dep).
 */
const metadataSchema: ConnectorMetadataSchema = {
  fields: [
    { name: 'url', type: 'string', description: 'HTTPS URL to a public ICS/iCal feed' },
    { name: 'days', type: 'number', description: 'Lookahead window in days from now (default 30)' },
    { name: 'max', type: 'number', description: 'Max events returned (default 20)' },
  ],
};

interface VEvent {
  uid: string;
  summary: string;
  description: string;
  location: string;
  start: string;
  end: string;
}

function unescapeIcsText(s: string): string {
  return s
    .replace(/\\n/g, ' ')
    .replace(/\\,/g, ',')
    .replace(/\\;/g, ';')
    .replace(/\\\\/g, '\\');
}

function foldLines(raw: string): string {
  // ICS line folding: lines starting with space/tab are continuations.
  // RFC 5545 allows arbitrarily nested folding, so iterate until stable.
  let prev = raw;
  let cur = prev.replace(/\r?\n[ \t]/g, '');
  while (cur !== prev) {
    prev = cur;
    cur = prev.replace(/\r?\n[ \t]/g, '');
  }
  return cur;
}

// Block hostnames that could be used for SSRF (private IP space + cloud-metadata
// + DNS rebinding loopback tricks). The Worker egress should also be locked
// down via wrangler.toml `outbound` policy, but block at the adapter for
// defense in depth.
//
// IPv4 ranges covered:
//   - 10.0.0.0/8        RFC 1918 private
//   - 172.16.0.0/12     RFC 1918 private (172.16–172.31)
//   - 192.168.0.0/16    RFC 1918 private
//   - 100.64.0.0/10     CGNAT shared address space
//   - 127.0.0.0/8       IPv4 loopback
//   - 169.254.0.0/16    IPv4 link-local (incl. AWS / GCP metadata 169.254.169.254)
//
// IPv6 ranges covered:
//   - ::1               IPv6 loopback
//   - fc00::/7          ULA (fc / fd first octet)
//   - fe80::/10         IPv6 link-local
//   - ::ffff:IPv4       IPv4-mapped IPv6 (so the embedded IPv4 must also be private)
//
// Hostnames covered:
//   - localhost / *.localhost
const PRIVATE_HOST_PATTERNS: RegExp[] = [
  /^127\./,
  /^10\./,
  /^192\.168\./,
  /^172\.(1[6-9]|2\d|3[01])\./, // 172.16–172.31
  /^169\.254\./,
  /^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\./, // 100.64–100.127
  /^localhost$/i,
  /^.*\.localhost$/i, // any subdomain of localhost
  /^::1$/,
  /^0:0:0:0:0:0:0:1$/, // IPv6 loopback full 8-group form (same address as ::1)
  /^f[cd][0-9a-f]{2}:/i, // fc00::/7 (fc / fd ULA)
  /^fe[89abcd][0-9a-f]{2}:/i, // fe80::/10 link-local
  // IPv4-mapped IPv6: ::ffff:127.x, ::ffff:10.x, ::ffff:192.168.x, ::ffff:172.16–31.x,
  // ::ffff:169.254.x, ::ffff:100.64–127.x
  /^::ffff:(127\.|10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.|169\.254\.|100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\.)/i,
];

function isPrivateHost(hostname: string): boolean {
  // Fast path: hostname is a traditional IPv4 literal or bare host name.
  if (PRIVATE_HOST_PATTERNS.some((re) => re.test(hostname))) return true;
  // IPv6: URL parser can hand us either the compressed form (`::1`) or the full
  // 8-group form (`0:0:0:0:0:0:0:1`) depending on platform/version. Walk the
  // first 16-bit word so both forms are covered.
  if (hostname.includes(':')) {
    const firstGroup = hostname.split(':')[0] ?? '';
    if (firstGroup) {
      const first = parseInt(firstGroup, 16);
      if (!isNaN(first)) {
        // ULA: fc00::/7 (top byte 0xfc–0xfd)
        if ((first & 0xfe00) === 0xfc00) return true;
        // Link-local: fe80::/10 (top 10 bits 0xfe80–0xfebf)
        if ((first & 0xffc0) === 0xfe80) return true;
      }
    }
    // IPv4-mapped IPv6: `::ffff:127.0.0.1` style — re-check the IPv4 portion.
    if (hostname.startsWith('::ffff:')) {
      const v4 = hostname.slice('::ffff:'.length);
      if (PRIVATE_HOST_PATTERNS.some((re) => re.test(v4))) return true;
    }
  }
  return false;
}

function validateIcsUrl(rawUrl: string): boolean {
  if (!/^https:\/\//i.test(rawUrl)) return false;
  let u: URL;
  try {
    u = new URL(rawUrl);
  } catch {
    return false;
  }
  if (u.protocol !== 'https:') return false;
  // Strip surrounding brackets used to wrap IPv6 hosts (e.g. https://[::1]/).
  // URL.hostname retains the brackets, so a bare-hostname regex won't match.
  const host = u.hostname.replace(/^\[|]$/g, '').toLowerCase();
  if (isPrivateHost(host)) return false;
  return true;
}

function parseIcsEvents(ics: string): VEvent[] {
  const text = foldLines(ics);
  const events: VEvent[] = [];
  const blocks = text.split(/BEGIN:VEVENT/i).slice(1);
  for (const block of blocks) {
    const end = block.indexOf('END:VEVENT');
    if (end === -1) continue;
    const body = block.slice(0, end);
    const evt: VEvent = { uid: '', summary: '', description: '', location: '', start: '', end: '' };
    const lines = body.split(/\r?\n/);
    for (const line of lines) {
      const colonIdx = line.indexOf(':');
      if (colonIdx === -1) continue;
      const keyRaw = line.slice(0, colonIdx);
      const key = keyRaw.split(';')[0]!.toUpperCase();
      const val = line.slice(colonIdx + 1);
      switch (key) {
        case 'UID':
          evt.uid = unescapeIcsText(val);
          break;
        case 'SUMMARY':
          evt.summary = unescapeIcsText(val);
          break;
        case 'DESCRIPTION':
          evt.description = unescapeIcsText(val);
          break;
        case 'LOCATION':
          evt.location = unescapeIcsText(val);
          break;
        case 'DTSTART':
          evt.start = val;
          break;
        case 'DTEND':
          evt.end = val;
          break;
        default:
          break;
      }
    }
    if (evt.summary || evt.start) events.push(evt);
  }
  return events;
}

/** Yahoo-style YYYYMMDDTHHMMSSZ and YYYYMMDD parsers. */
function parseIcsDate(raw: string): number {
  if (!raw) return NaN;
  const compact = raw.match(/^(\d{4})(\d{2})(\d{2})(T(\d{2})(\d{2})(\d{2})Z?)?$/);
  if (compact) {
    const [, y, m, d, , hh, mm, ss] = compact;
    if (hh && mm && ss) return Date.UTC(Number(y), Number(m) - 1, Number(d), Number(hh), Number(mm), Number(ss));
    // DATE-only — midnight UTC
    return Date.UTC(Number(y), Number(m) - 1, Number(d));
  }
  const tIdx = raw.indexOf('T');
  if (tIdx >= 0 && raw.endsWith('Z')) {
    const date = new Date(raw);
    if (!isNaN(date.getTime())) return date.getTime();
  }
  const date = new Date(raw);
  return date.getTime();
}

export class IcalAdapter implements ConnectorAdapter {
  readonly name: ConnectorName = 'ical';
  readonly metadataSchema = metadataSchema;

  async isAuthorized(_userId: string): Promise<boolean> {
    return true;
  }

  scoreRelevance(query: UserQuery, _memory: MemoryFact[]): number {
    const q = (query.text || '').toLowerCase();
    const terms = ['ical', 'ics', '.ics', 'webcal', 'my calendar', 'subscribe to', 'public calendar'];
    if (terms.some((t) => q.includes(t))) return 0.85;
    // Detect .ics URLs even without explicit keyword
    if (/\.ics(\b|$)/.test(q) || /webcal:\/\//.test(q)) return 0.85;
    return 0.1;
  }

  buildFilter(query: UserQuery): ConnectorFilter {
    const text = query.text || '';
    const urlMatch = text.match(/https?:\/\/[^\s<>"]+\.ics/i) || text.match(/webcal:\/\/[^\s<>"]+/i);
    let url = '';
    if (urlMatch) {
      const cleaned = urlMatch[0].replace(/^webcal:\/\//, 'https://');
      url = cleaned;
    }
    return { url, days: 30, max: 20 };
  }

  async fetch(ctx: ConnectorContext): Promise<ConnectorResult> {
    const f = (ctx.filter || {}) as { url?: string; days?: number; max?: number };
    const rawUrl = (f.url || '').trim();
    if (!validateIcsUrl(rawUrl)) {
      return { items: [], totalCount: 0, source: this.name };
    }
    const max = Math.min(Math.max(Number(f.max) || 20, 1), 50);
    const daysWindow = Number(f.days) || 30;
    const now = Date.now();
    const cutoff = now + daysWindow * 24 * 60 * 60 * 1000;

    try {
      // #21 (SSRF): the URL-string guard above is necessary but not sufficient —
      // a public host can 30x-redirect to 169.254.169.254 or another private
      // target. Follow redirects MANUALLY and re-validate every hop against the
      // same private-host blocklist; any hop that fails validation aborts.
      const MAX_REDIRECTS = 3;
      let currentUrl = rawUrl;
      let res: Response | null = null;
      for (let hop = 0; hop <= MAX_REDIRECTS; hop++) {
        if (!validateIcsUrl(currentUrl)) {
          return { items: [], totalCount: 0, source: this.name };
        }
        res = await fetch(currentUrl, {
          headers: { 'User-Agent': 'PersonalAI/1.0 (ICS)', Accept: 'text/calendar' },
          signal: ctx.signal ?? null,
          redirect: 'manual',
        });
        if (res.status >= 300 && res.status < 400) {
          const location = res.headers.get('location');
          if (!location) {
            return { items: [], totalCount: 0, source: this.name };
          }
          currentUrl = new URL(location, currentUrl).toString();
          continue;
        }
        break;
      }
      if (!res || !res.ok) {
        return { items: [], totalCount: 0, source: this.name };
      }
      const body = await res.text();
      const events = parseIcsEvents(body);
      const upcoming = events
        .map((e) => ({ ...e, _startMs: parseIcsDate(e.start) }))
        .filter((e) => !isNaN(e._startMs) && e._startMs >= now - 24 * 60 * 60 * 1000 && e._startMs <= cutoff)
        .sort((a, b) => a._startMs - b._startMs)
        .slice(0, max);

      const items: ConnectorResult['items'] = upcoming.map((e) => ({
        id: e.uid || `${e.start}-${e.summary}`,
        title: e.summary || '(untitled event)',
        snippet: [e.location, e.description].filter(Boolean).join(' • ').slice(0, 280),
        date: e.start ? new Date(e._startMs).toISOString() : '',
        metadata: {
          location: e.location,
          start: e.start,
          end: e.end,
          sourceUrl: rawUrl,
        },
      }));
      return { items, totalCount: items.length, source: this.name };
    } catch {
      return { items: [], totalCount: 0, source: this.name };
    }
  }

  /** 
   * Token refresh is handled by the Cloudflare Worker OAuth proxy.
   * This adapter assumes a valid token is injected via filter.token.
   * @see packages/cloudflare-server/src/index.ts OAuth refresh routes
   */
}
