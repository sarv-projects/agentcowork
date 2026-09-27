// P3.2 — cockpit / ambient flight-deck bridge (H2, doc 33 §9.5). Mirrors the
// Rust types in agentcowork-audit/src/cockpit.rs. In a plain-browser preview
// (no shell) the callers fall back to demo data so the page is explorable.

import { inTauri, invoke } from "./tauri";
import { nativeCall } from './runtime';

export type AgentStatus = "running" | "waiting" | "done" | "failed" | "idle";

export interface LiveAction {
  ts_ms: number;
  tool: string;
  summary: string;
}

export interface TokenCounters {
  tokens_in: number;
  tokens_out: number;
}

export interface AgentCard {
  agent_id: string;
  label: string;
  model: string;
  provider: string;
  status: AgentStatus;
  tokens: TokenCounters;
  started_ms: number;
  last_action_ms: number;
  actions: LiveAction[];
}

export interface InterruptCard {
  id: string;
  agent_id: string;
  prompt: string;
  options: string[];
  responded: number | null;
  created_ms: number;
}

export interface CockpitState {
  agents: AgentCard[];
  interrupts: InterruptCard[];
  quiet: boolean;
}

/** Full flight-deck snapshot (polled by the page). */
export async function cockpitSnapshot(): Promise<CockpitState> {
  if (!inTauri()) return demoState();
  return nativeCall('cockpit snapshot', () => invoke<CockpitState>("cockpit_snapshot"));
}

/** Feed seam: record a live agent action. */
export async function cockpitActivity(
  agentId: string,
  tool: string,
  summary: string,
): Promise<void> {
  if (!inTauri()) return;
  return nativeCall('cockpit activity', () => invoke<void>("cockpit_activity", { agentId, tool, summary }));
}

/** Feed seam: update an agent's token counters. */
export async function cockpitTokens(
  agentId: string,
  tokensIn: number,
  tokensOut: number,
): Promise<void> {
  if (!inTauri()) return;
  return nativeCall('cockpit tokens', () => invoke<void>("cockpit_tokens", { agentId, tokensIn, tokensOut }));
}

/** Register/refresh an agent card (coordinator registration feed). */
export async function cockpitUpsertAgent(args: {
  agentId: string;
  label: string;
  model: string;
  provider: string;
}): Promise<void> {
  if (!inTauri()) return;
  return nativeCall('cockpit agent update', () => invoke<void>("cockpit_upsert_agent", args));
}

/** Toggle quiet mode: collapse to a single-sentence tray status. */
export async function cockpitQuiet(
  quiet: boolean,
  status?: string,
): Promise<void> {
  if (!inTauri()) return;
  return nativeCall('cockpit quiet mode', () => invoke<void>("cockpit_quiet", { quiet, status: status ?? null }));
}

/** STOP/UNDO: control-channel agent controls (canonical in `./tauri`). */
export { agentStop, agentUndo } from './tauri'

/** Answer a circuit-break MCQ interrupt card. */
export async function interruptRespond(
  interruptId: string,
  choice: number,
): Promise<void> {
  if (!inTauri()) return;
  return nativeCall('interrupt response', () => invoke<void>("interrupt_respond", { interruptId, choice }));
}

// ---------------------------------------------------------------------------
// demo fallback (plain-browser preview)
// ---------------------------------------------------------------------------

const now = () => Date.now();

function demoState(): CockpitState {
  const t = now();
  return {
    quiet: false,
    agents: [
      {
        agent_id: "agent-researcher",
        label: "Researcher",
        model: "claude-sonnet-4-5",
        provider: "anthropic",
        status: "running",
        tokens: { tokens_in: 12_481, tokens_out: 3_204 },
        started_ms: t - 94_000,
        last_action_ms: t - 2_000,
        actions: [
          { ts_ms: t - 94_000, tool: "browser.navigate", summary: "Opened search results" },
          { ts_ms: t - 61_000, tool: "browser.act", summary: "Extracting competitor pricing" },
          { ts_ms: t - 30_000, tool: "office.patch", summary: "Updating report table" },
          { ts_ms: t - 2_000, tool: "memory.recall", summary: "Pulling last quarter's numbers" },
        ],
      },
      {
        agent_id: "agent-forge",
        label: "Forge",
        model: "codex",
        provider: "openai",
        status: "idle",
        tokens: { tokens_in: 4_112, tokens_out: 987 },
        started_ms: t - 220_000,
        last_action_ms: t - 140_000,
        actions: [
          { ts_ms: t - 220_000, tool: "git.status", summary: "Checked working tree" },
          { ts_ms: t - 140_000, tool: "edit.patch", summary: "Applied doc fix" },
        ],
      },
    ],
    interrupts: [
      {
        id: "int-9",
        agent_id: "agent-researcher",
        prompt: "Send this competitive summary to the client email?",
        options: ["Skip", "Retry", "Escalate", "Do it manually"],
        responded: null,
        created_ms: t - 5_000,
      },
    ],
  };
}
