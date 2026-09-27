// P3.1 — replay & audit bridge. Mirrors the Rust types in
// agentcowork-audit/src/replay.rs; `invoke` proxies the Tauri command bridge,
// and in a plain-browser preview (no shell) the callers fall back to demo
// data so the page is still explorable.

import { inTauri, invoke } from "./tauri";
import { nativeCall } from './runtime';

/** Per-document segment metadata (replay_segments row). */
export interface Segment {
  document_id: string;
  tab_id: string;
  first_ts_ms: number;
  last_ts_ms: number;
  event_count: number;
  size_bytes: number;
  has_gap: boolean;
  created_ms: number;
}

/** One recorded event (one NDJSON line). */
export interface ReplayEvent {
  seq: number;
  ts_ms: number;
  kind: string;
  tab_id?: string;
  document_id?: string;
  data?: Record<string, unknown>;
}

/** Scrubber data for one document. */
export interface Timeline {
  segment: Segment | null;
  events: ReplayEvent[];
  screenshot_steps: number[];
}

/** Searchable sessions list. */
export async function replaySessions(query: string): Promise<Segment[]> {
  if (!inTauri()) return demoSessionSearch(query);
  return nativeCall('replay sessions', () => invoke<Segment[]>("replay_sessions", { query: query || null }));
}

/** Full scrubber data for one document. */
export async function replayTimeline(documentId: string): Promise<Timeline> {
  if (!inTauri()) return demoTimeline(documentId);
  return nativeCall('replay timeline', () => invoke<Timeline>("replay_timeline", { documentId }));
}

/** A step's screenshot as a data URL, or null when missing. */
export async function replayScreenshot(
  documentId: string,
  step: number,
): Promise<string | null> {
  if (!inTauri()) {
    return step % 2 === 0
      ? "data:image/svg+xml;base64," + btoa(demoShotSvg(step))
      : null;
  }
  return nativeCall('replay screenshot', () => invoke<string | null>("replay_screenshot", { documentId, step }));
}

/** Watch: live tail of a document's stream since a seq. */
export async function watchEvents(
  documentId: string,
  sinceSeq: number,
): Promise<ReplayEvent[]> {
  if (!inTauri()) return [];
  return nativeCall('watch audit events', () => invoke<ReplayEvent[]>("watch_events", { documentId, sinceSeq }));
}

/** Stop: JSON-RPC agent/stop over the control channel (canonical in `./tauri`). */
export { agentStop } from './tauri'

// ---------------------------------------------------------------------------
// demo fallback (plain-browser preview)
// ---------------------------------------------------------------------------

function demoSessionSearch(query: string): Segment[] {
  const now = Date.now();
  const all: Segment[] = [
    {
      document_id: "docDemoA1B2",
      tab_id: "tab-7",
      first_ts_ms: now - 12_000,
      last_ts_ms: now - 2_000,
      event_count: 24,
      size_bytes: 3_842,
      has_gap: false,
      created_ms: now - 12_000,
    },
    {
      document_id: "docDemoC3D4",
      tab_id: "tab-12",
      first_ts_ms: now - 86_000,
      last_ts_ms: now - 60_000,
      event_count: 61,
      size_bytes: 11_207,
      has_gap: true,
      created_ms: now - 86_000,
    },
  ];
  const q = query.trim().toLowerCase();
  return all.filter(
    (s) =>
      !q ||
      s.document_id.toLowerCase().includes(q) ||
      s.tab_id.toLowerCase().includes(q),
  );
}

function demoTimeline(documentId: string): Timeline {
  const base = Date.now() - 10_000;
  const kinds = ["navigate", "click", "scroll", "input", "dom_mutation"];
  const events: ReplayEvent[] = Array.from({ length: 12 }, (_, i) => ({
    seq: i + 1,
    ts_ms: base + i * 800,
    kind: kinds[i % kinds.length],
    data: { step: i + 1 },
  }));
  const segment: Segment = {
    document_id: documentId,
    tab_id: "tab-7",
    first_ts_ms: base,
    last_ts_ms: base + 11 * 800,
    event_count: events.length,
    size_bytes: 1_900,
    has_gap: false,
    created_ms: base,
  };
  return {
    segment,
    events,
    screenshot_steps: [2, 4, 6, 8, 10, 12].filter((s) => s <= events.length),
  };
}

function demoShotSvg(step: number): string {
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180">` +
    `<rect width="100%" height="100%" fill="#111a33"/>` +
    `<rect x="10" y="${10 + (step % 5) * 18}" width="60%" height="10" rx="3" fill="#7c3aed"/>` +
    `<text x="10" y="170" fill="#94a3b8" font-family="monospace" font-size="12">step ${step} · demo</text>` +
    `</svg>`
  );
}
