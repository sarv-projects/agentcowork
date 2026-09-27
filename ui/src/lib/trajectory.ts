// P5.9 (J5) — Trajectory bridge. Inspect, by source, which context blocks
// (persona / user_document / memory / tool_result / blueprint) were injected
// into the prompt each turn. Mirrors `agentcowork-audit::session_log` types; in
// a plain-browser preview the callers fall back to demo data.

import { inTauri, invoke } from "./tauri";
import { nativeCall } from './runtime';

/** One context-injection record (one ContextInjection audit event). */
export interface ContextInjection {
  seq: number;
  ts_ms: number;
  session: string;
  agent: string;
  source: string;
  tokens?: number;
  ref_id: string;
}

/** The canonical J5 sources, plus the `other` bucket for unknown ones. */
export const TRAJECTORY_SOURCES = [
  "persona",
  "user_document",
  "memory",
  "tool_result",
  "blueprint",
  "other",
] as const;

export type TrajectorySource = (typeof TRAJECTORY_SOURCES)[number];

/** The session ids that have a context-injection log. */
export async function trajectorySessions(): Promise<string[]> {
  if (!inTauri()) return demoSessions();
  return nativeCall('trajectory sessions', () => invoke<string[]>("trajectory_sessions"));
}

/** One session's context-injection records (newest-last). */
export async function trajectorySnapshot(
  sessionId: string,
): Promise<ContextInjection[]> {
  if (!inTauri()) return demoInjections(sessionId);
  return nativeCall('trajectory snapshot', () => invoke<ContextInjection[]>("trajectory_snapshot", { sessionId }));
}

/** Group a session's injections by source (stable TRAJECTORY_SOURCES order). */
export function groupBySource(
  records: ContextInjection[],
): Map<TrajectorySource, ContextInjection[]> {
  const map = new Map<TrajectorySource, ContextInjection[]>();
  for (const src of TRAJECTORY_SOURCES) map.set(src, []);
  for (const r of records) {
    const src = (TRAJECTORY_SOURCES as readonly string[]).includes(r.source)
      ? (r.source as TrajectorySource)
      : "other";
    map.get(src)!.push(r);
  }
  return map;
}

function demoSessions(): string[] {
  return ["sess-q3-budget", "sess-web-scrape"];
}

function demoInjections(sessionId: string): ContextInjection[] {
  const base = Date.now() - 40_000;
  const samples: Array<[string, string, number]> = [
    ["persona", "coder-terse", 96],
    ["user_document", "ARCH/05.md", 1_204],
    ["memory", "mem:7", 412],
    ["tool_result", "ls -la", 388],
    ["blueprint", "q3-budget", 640],
    ["memory", "mem:11", 208],
  ];
  return samples.map(([source, refId, tokens], i) => ({
    seq: i + 1,
    ts_ms: base + i * 2_100,
    session: sessionId,
    agent: "planner",
    source,
    tokens,
    ref_id: refId,
  }));
}
