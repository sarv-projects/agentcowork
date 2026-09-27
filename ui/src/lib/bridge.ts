// Live-data bridge (P0.7): when running inside the Tauri shell, hydrates
// real agent, usage, session, work, Guard, and chat state. In a plain-browser
// run it remains inactive and the UI may use explicitly labelled preview fixtures.
// Native command failures are recorded as degraded runtime state and are never
// converted into preview data or synthetic success.

import { useAppStore, sanitizeSessionRows, mergeHydratedSessions, type LiveBudget } from "./store";
import { inTauri, onChatEvent, type ChatWireEvent } from "./tauri";
import {
  acpAgents,
  acpIdFor,
  acpInstallStatus,
  acpLaunch,
  acpPrompt,
  acpHandleKey,
  acpHandleRecordFromLaunch,
  agentDirectoryList,
  currentBinding,
  isAgentReady,
  readinessLabel,
  type AgentDirectoryEntry,
  type AgentReadiness,
  type HarnessManifest,
  type InstallState,
} from "./acp";
import { limitationFor } from "./plain-language";
import { usageSnapshot } from "./spend";
import { AGENTS, readinessToInstallStatus, type AgentRuntime } from "./agents";
import { workList, workSnapshot } from "./work";
import {
  markRuntimeBooting,
  markRuntimeLive,
  nativeCall,
  markSidecarOffline,
  markVaultLocked,
  markVaultSetup,
  runtimeError,
  setRuntimeState,
} from "./runtime";
import { runtimeStatus as readRuntimeStatus, type RuntimeStatus } from "./tauri";
import { readPref } from "./ui-prefs";
import type { LiveNotification } from "./store";

/** P58.9 — the Settings → Notifications switches gate the live activity stream
 * for real. Guard approvals are category `always`: they are a safety surface and
 * are never suppressed by a chat/task preference. */
type NotifyCategory = "chat" | "task" | "wiki" | "always";
const NOTIFY_PREF: Record<Exclude<NotifyCategory, "always">, string> = {
  chat: "notify.chat",
  task: "notify.quest",
  wiki: "notify.wiki",
};
function pushLive(n: LiveNotification, category: NotifyCategory = "chat"): void {
  if (category !== "always" && !readPref(NOTIFY_PREF[category], true)) return;
  useAppStore.getState().pushLiveNotification(n);
}

/** ACP registry id → the v2 catalog's agent id (same brain, curated skin). */
const ACP_TO_CATALOG: Record<string, string> = {
  claude: "claude-code",
  codex: "codex-cli",
  grok: "grok-build",
  gemini: "gemini-cli",
  cursor: "cursor-agent",
  aider: "aider",
  opencode: "opencode",
};

/** Merge the live ACP registry + per-agent install state over the static
 * catalog. Install truth comes from the shell: an agent is `installed` only
 * when AgentCowork installed it (`acp_install_status`) or auto-discovery found
 * its CLI on PATH (kind "path"). PATH-discovered agents carry no version —
 * never fall back to a static example version. */
function mergeAgentCatalog(
  seed: AgentRuntime[],
  manifests: HarnessManifest[],
  installs: Record<string, InstallState>,
): AgentRuntime[] {
  const merged = seed.map((a) => ({ ...a }));
  const seen = new Set(merged.map((a) => a.id));
  for (const m of manifests) {
    const catalogId = ACP_TO_CATALOG[m.id] ?? m.id;
    const state = installs[m.id];
    // ADR-0005 (P71.2a): install truth is the shell's — there is no built-in
    // agent that is force-shown as installed.
    const status = state?.installed
      ? "installed"
      : state?.discovered
        ? "discovered"
        : "available";
    const existing = merged.find((a) => a.id === catalogId);
    if (existing) {
      existing.status = status;
      existing.readiness = state?.readiness;
      existing.discovered = state?.discovered ?? existing.discovered;
      existing.launchable = state?.launchable ?? existing.launchable;
      existing.version =
        state?.version ?? (state?.kind === "path" ? undefined : existing.version);
      existing.path = state?.binaryPath ?? existing.path;
      existing.location = state?.location ?? existing.location;
      existing.note = m.description;
    } else if (!seen.has(catalogId)) {
      const row = synthesizeAgent(m);
      row.status = status;
      row.readiness = state?.readiness;
      row.discovered = state?.discovered;
      row.launchable = state?.launchable;
      row.version = state?.version;
      row.path = state?.binaryPath ?? undefined;
      row.location = state?.location;
      merged.push(row);
      seen.add(catalogId);
    }
  }
  return merged;
}

/** Re-run agent discovery on demand (Settings → Refresh / Health): re-fetch
 * the ACP registry + install status (incl. PATH auto-discovery) and republish
 * the merged catalog to the picker + settings. Throws on failure so callers
 * can surface the error instead of silently keeping stale rows. */
export async function refreshAgentCatalog(): Promise<void> {
  const manifests = await acpAgents();
  const installs = await acpInstallStatus();
  let merged = mergeAgentCatalog(AGENTS, manifests, installs);
  // P69.D1 — the canonical directory carries what `acp_agents` cannot: local
  // `agent.toml` bundles. Best-effort: a shell without the command (older
  // build) must not break rotation of the rest of the catalog.
  try {
    const directory = await agentDirectoryList();
    merged = mergeDirectoryAgents(merged, directory.agents);
  } catch {
    /* directory unavailable — the ACP-merged catalog still stands */
  }
  useAppStore.getState().setLiveAgents(merged);
}

/** Register the queue dispatcher once: the store calls it (cycle-free) when
 * a queued turn should fire after the current one ends. */
export function registerTurnDispatcher(): void {
  useAppStore.getState().setTurnDispatcher((turn) => {
    // Switch to the queued turn's session so it sends into the right work.
    const st = useAppStore.getState()
    if (st.activeSessionId !== turn.sessionId) {
      st.setActiveSession(turn.sessionId)
    }
    void sendUserMessage(turn.text, turn.context, { bypassQueue: true })
  })
}

/** Every ACP agent that has no curated catalog entry gets a synthesized
 * picker row (mark + accent + install state), so the full registry is
 * choosable even before its curated models land. */
/**
 * P69.D1 — merge the canonical agent directory (`agent_directory_list`) into
 * the picker catalog. The directory is the composition point: it is the only
 * place local `agent.toml` bundles and discovered rows appear, so a
 * user-authored agent is choosable without the UI keeping an agent list of
 * its own. Entries the ACP merge already produced are only annotated (never
 * duplicated); bundle rows are joined by their bundle slug.
 */
function mergeDirectoryAgents(
  merged: AgentRuntime[],
  entries: AgentDirectoryEntry[],
): AgentRuntime[] {
  const seen = new Set(merged.map((a) => a.id));
  for (const entry of entries) {
    const existing = merged.find((a) => a.id === entry.id);
    if (existing) {
      // Stale catalog rows must still learn what the directory knows.
      existing.readiness = entry.readiness;
      existing.status = readinessToInstallStatus(entry.readiness, existing.status);
      existing.note = entry.description || existing.note;
      continue;
    }
    if (seen.has(entry.id)) continue;
    merged.push({
      id: entry.id,
      name: entry.name,
      vendor:
        entry.source === 'local_bundle'
          ? 'custom agent'
          : entry.authMode === 'subscription'
            ? 'subscription'
            : entry.authMode === 'api_key'
              ? 'API key'
              : entry.authMode === 'local'
                ? 'local inference'
                : 'unknown',
      tagline: entry.description,
      readiness: entry.readiness,
      status: readinessToInstallStatus(entry.readiness, 'available'),
      discovered: entry.source === 'discovered' || !entry.installed,
      path: entry.locator ?? undefined,
      mark: entry.name.replace(/[^A-Za-z0-9]/g, '').slice(0, 2).toUpperCase() || 'A',
      accent:
        entry.source === 'local_bundle'
          ? 'bg-violet-500 text-slate-950'
          : 'bg-sky-500 text-slate-950',
      capabilities: [],
      models: [],
      defaultModel: '',
      headless: true,
      sandbox: 'soft',
      note:
        entry.source === 'local_bundle'
          ? 'Custom agent (agent.toml bundle)'
          : entry.locator ?? undefined,
    });
    seen.add(entry.id);
  }
  return merged;
}

function synthesizeAgent(m: HarnessManifest): AgentRuntime {
  const mark =
    m.name.replace(/[^A-Za-z0-9]/g, "").slice(0, 2).toUpperCase() || "A";
  return {
    id: m.id,
    name: m.name,
    vendor:
      m.authMode === "subscription"
        ? "subscription"
        : m.authMode === "api_key"
          ? "API key"
          : "local",
    tagline: m.description,
    status: "available",
    mark,
    accent: "bg-sky-500 text-slate-950",
    capabilities: [],
    models: [],
    defaultModel: "",
    headless: true,
    sandbox: "soft",
    governance: m.governance,
    note: "Installed from the ACP registry (F8)",
  };
}

/** Map a circuit-break option value (from the Rust McqOption) to the
 * human label the card renders. Values are lowercase; labels title-case. */
function mcqLabel(value: string): string {
  switch (value) {
    case "skip":
      return "Skip this task";
    case "retry":
      return "Retry once";
    case "escalate":
      return "Escalate to me";
    case "takeover":
      return "Take over manually";
    case "approve":
      return "Approve & continue";
    case "reject":
      return "Reject";
    default:
      return value.charAt(0).toUpperCase() + value.slice(1);
  }
}

/**
 * J11 budget-kill surface: the broker's message is
 * `session 'X' stopped: $2.00 limit (spent $2.10)`. Normalize it to the
 * canonical `stopped: $spent / $limit` form the composer strip shows.
 */
function budgetKillText(message: string): string {
  const m = message.match(/stopped:\s*\$([\d.]+)\s*limit\s*\(spent\s*\$([\d.]+)\)/i);
  if (!m) return message.includes("stopped") ? message : `stopped: limit ${message}`;
  return `stopped: $${m[2]} / $${m[1]}`;
}

function budgetEventText(e: ChatWireEvent): string {
  if (typeof e.spent === 'number' && typeof e.limit === 'number') {
    return `stopped: $${e.spent.toFixed(2)} / $${e.limit.toFixed(2)}`;
  }
  return budgetKillText(e.message ?? 'Budget limit reached');
}

function updateLiveBudget(e: ChatWireEvent, st: ReturnType<typeof useAppStore.getState>): void {
  if (typeof e.spent !== 'number' && typeof e.limit !== 'number') return;
  const current = st.liveBudget;
  st.setLiveBudget({
    spent: e.spent ?? current?.spent ?? 0,
    cap: e.limit ?? current?.cap ?? 2,
    tokens: current?.tokens ?? 0,
    ...(current?.cacheHitRate !== undefined ? { cacheHitRate: current.cacheHitRate } : {}),
  });
}

/** Route a live chat wire event into the session it belongs to. Chat-events
 * carry a `sessionId` so switching chats mid-stream never lands tokens on the
 * wrong transcript (bugfix 2). Events without identity are rejected rather
 * than routed through the active session. */
export function handleChatEvent(e: ChatWireEvent): void {
  const st = useAppStore.getState();
  // Runtime events must be self-routing. A missing identity is a protocol
  // defect, not permission to mutate whichever tab is active now.
  const sid = e.sessionId;
  if (!sid || !e.streamId) {
    pushLive({
      id: `live:protocol:${e.eventId ?? `${e.type}:${Date.now()}`}`,
      kind: 'error',
      title: 'Agent event was ignored',
      detail: 'The runtime returned an event without a chat or stream identity.',
      ts: Date.now(),
      unread: true,
      source: 'Runtime',
    });
    return;
  }

  switch (e.type) {
    case "ttft":
      st.streamStart(sid, e.streamId);
      break;
    // P52.23 — provider reasoning (CoT) deltas. Forwarded all the way from
    // the engine's `reasoning` stream; coalesced per thought block by the
    // store so the collapsible renders whole thoughts, never wire fragments.
    // Reasoning arrives *before* the first text batch on thinking models, so
    // it also opens the assistant message (no ttft dependency).
    case "reasoning":
      st.streamStart(sid, e.streamId);
      st.appendReasoning(e.text ?? "", sid, e.streamId);
      break;
    case "batch":
      st.streamAppend(e.text ?? "", false, sid, e.streamId);
      st.noteStreamTick(e.tokenCount ?? Math.max(1, Math.round((e.text ?? "").length / 4)), sid, e.streamId);
      break;
    case "citations":
      st.streamStart(sid, e.streamId);
      st.streamCitations(e.citations ?? [], sid, e.streamId);
      break;
    case "walkthrough":
      st.streamWalkthrough(e.stops ?? [], sid, e.streamId);
      break;
    case "done":
      // `fullText` is authoritative even when it is intentionally empty.
      // A falsy check here used to leave empty successful turns stuck in the
      // running state forever.
      st.streamFinalize(e.fullText ?? e.text ?? "", sid, e.streamId);
      break;
    case "budgetExceeded": {
      const detail = budgetEventText(e);
      updateLiveBudget(e, st);
      st.streamBudgetKill(detail, sid, e.streamId);
      pushLive({
        id: `live:cost:${e.streamId ?? sid}:${e.eventId ?? Date.now()}`,
        kind: 'cost',
        title: 'Budget limit reached',
        detail,
        ts: Date.now(),
        unread: true,
        source: 'Spend',
      });
      break;
    }
    case "cancelled":
      st.streamCancelled(sid, e.streamId);
      pushLive({
        id: `live:cancelled:${e.streamId}:${e.eventId ?? Date.now()}`,
        kind: 'info',
        title: 'Turn cancelled',
        detail: 'The agent stopped before completing this turn.',
        ts: Date.now(),
        unread: true,
        source: 'Agent',
      });
      break;
    case "error":
      if (e.code === "budget_exceeded") {
        const detail = budgetKillText(e.message ?? "");
        updateLiveBudget(e, st);
        st.streamBudgetKill(detail, sid, e.streamId);
        pushLive({
          id: `live:cost:${e.streamId ?? sid}:${Date.now()}`,
          kind: 'cost',
          title: 'Budget limit reached',
          detail,
          ts: Date.now(),
          unread: true,
          source: 'Spend',
        });
      } else if (e.code === "cua_requires_vision") {
        st.setCuaVisionGate(true);
        st.streamFail(e.message ?? "Computer use needs a vision model", sid, {
          layer: "agent",
          code: e.code,
          detail: e.message ?? "Computer use needs a vision model",
          retryable: false,
        }, e.streamId);
      } else if (e.code === "tool_failed" || e.toolId) {
        st.streamToolResult(e.toolId ?? "tool", undefined, e.message ?? "tool failed", sid, e.streamId);
        // P51.21 — the failure card is layer-named (Tool) and retryable.
        st.streamFail(e.message ?? "tool failed", sid, {
          layer: "tool",
          code: e.code,
          detail: e.message ?? "tool failed",
          retryable: true,
        }, e.streamId);
        pushLive({
          id: `live:tool:${e.toolId ?? 'tool'}:${Date.now()}`,
          kind: 'warning',
          title: `Tool failed: ${e.toolId ?? 'tool'}`,
          detail: e.message ?? "tool failed",
          ts: Date.now(),
          unread: true,
          source: 'Agent',
        });
      } else {
        // P32.4 — honest-limitation surfacing: say plainly what failed +
        // offer the nearest alternative (Wharton: no technical framing).
        const lim = limitationFor(e.message ?? "Agent error");
        st.streamFail(`${lim.plain} — ${lim.alternative}`, sid, undefined, e.streamId);
        pushLive({
          id: `live:error:${e.streamId ?? sid}:${Date.now()}`,
          kind: 'error',
          title: 'Turn failed',
          detail: lim.plain,
          ts: Date.now(),
          unread: true,
          source: 'Agent',
        });
      }
      break;
    case "stage": {
      if (typeof e.stage !== "string") break;
      const parts = e.stage.split(":");
      if (parts[0] === "tool") {
        st.streamToolProgress(parts[1] ?? "tool", parts.slice(2).join(":") || "running", sid, e.streamId);
      } else {
        // Every non-tool lifecycle stage is visible in Now Doing. Keep the
        // wire value as the detail while plain-language rendering owns the
        // user-facing label.
        st.streamStep(e.stage, sid, e.streamId);
      }
      break;
    }
    case "planStart":
      st.streamStep(`plan:${e.tasks ?? 0} task(s)`, sid, e.streamId);
      break;
    case "planStep":
      st.streamStep(`plan:${e.planId ?? 'plan'}:${e.taskId ?? 'task'}:${e.status ?? 'running'}`, sid, e.streamId);
      break;
    case "memoryExtracted":
      st.streamStep(`memory: saved ${e.facts?.length ?? 0} fact(s)`, sid, e.streamId);
      break;
    case "monitor":
      if (e.notified || e.stopped) {
        st.pushMonitor({
          notified: e.notified ?? false,
          stopped: e.stopped ?? false,
          current: e.current ?? "",
          jobId: e.jobId,
        });
        pushLive({
          id: `live:monitor:${e.jobId ?? e.streamId ?? 'job'}:${Date.now()}`,
          kind: 'info',
          title: e.stopped ? 'Monitor stopped' : 'Monitor updated',
          detail: e.current ?? "",
          ts: Date.now(),
          unread: true,
          source: 'Automation',
        }, 'task');
      }
      break;
    case "toolCall":
      st.streamToolCall(e.toolId ?? e.text ?? e.code ?? "tool", e.args, e.risk, sid, e.streamId);
      break;
    case "verification": {
      // P41.4 — K1 verification receipt: model-reported pass/fail per check,
      // surfaced inline in the editor's Diff rail.
      st.pushVerification({
        taskId: e.taskId ?? "",
        checks: e.checks ?? [],
        report: e.report ?? "",
        passed: e.passed === undefined ? null : e.passed,
        tsMs: Date.now(),
      });
      break;
    }
    case "toolResult": {
      const err =
        e.error ??
        (e.result && typeof e.result === "object" && e.result !== null && "error" in e.result
          ? String((e.result as { error?: unknown }).error ?? "")
          : undefined);
      st.streamToolResult(e.toolId ?? "tool", e.result, err || undefined, sid, e.streamId);
      break;
    }
    // P6.3 Stage-0: the plan executor's circuit breaker tripped — render the
    // H2 cockpit MCQ card. P71.2c deleted the plan executor with the built-in
    // engine, so nothing emits this today; the arm is kept because the card is
    // the honest surface for an interrupt whenever one arrives (the agent's own
    // permission prompts come through the ACP path instead).
    case "interrupt":
      st.pushMcq(
        {
          id: e.breakId ?? `${e.planId ?? "plan"}-break`,
          title: e.title ?? "Agent needs a decision",
          description:
            e.description ?? "The plan hit a limit or loop. Choose how to continue.",
          kind: "mcq",
          options: (e.options ?? []).map((v) => ({ label: mcqLabel(v), value: v })),
        },
        sid,
      );
      break;
    // P6.3 Stage-0: the plan finished (or halted) — end the streaming state.
    case "planDone":
      st.streamAppend(
        e.error
          ? `⚠ Plan halted: ${e.error}`
          : `✅ Plan complete · ${e.tasksDone ?? 0} task(s) done`,
        true,
        sid,
        e.streamId,
      );
      break;
    default:
      break;
  }
}

type BridgeDisposer = () => void;

// `main.tsx` mounts the bootstrap effect inside React.StrictMode. Keep one
// shared bridge with reference-counted cleanup so setup→cleanup→setup does not
// duplicate listeners, polling, or side effects.
let bridgeUsers = 0;
let bridgeStart: Promise<BridgeDisposer> | null = null;

export function initBridge(): Promise<BridgeDisposer> {
  bridgeUsers += 1;
  if (!bridgeStart) bridgeStart = startBridge();
  return bridgeStart.then((dispose) => {
    let released = false;
    return () => {
      if (released) return;
      released = true;
      bridgeUsers = Math.max(0, bridgeUsers - 1);
      if (bridgeUsers === 0) {
        dispose();
        bridgeStart = null;
      }
    };
  });
}

async function startBridge(): Promise<BridgeDisposer> {
  if (!inTauri()) return () => undefined;

  // P51.5 — the queue dispatcher must exist before any turn ends, or a
  // queued ask would never fire. Register once at bridge start (idempotent).
  registerTurnDispatcher();

  let alive = true;
  let hydrated = false;
  let hydrating = false;
  let wasSidecarReady = false;
  let fault: string | undefined;
  let readinessTimer: ReturnType<typeof setInterval> | undefined;
  let guardTimer: ReturnType<typeof setInterval> | undefined;
  let unlistenChat: (() => void) | undefined;
  const seenTickets = new Set<string>();

  const recordFault = (operation: string, error: unknown) => {
    fault = `${operation}: ${runtimeError(error)}`;
    setRuntimeState('degraded', fault);
  };

  const updateReadiness = async (): Promise<RuntimeStatus | null> => {
    if (!alive) return null;
    try {
      const status = await readRuntimeStatus();
      if (!alive) return status;
      if (status.vault === 'setup') {
        markVaultSetup();
      } else if (status.vault === 'locked') {
        markVaultLocked();
      } else if (!status.sidecar) {
        wasSidecarReady = false;
        // P70.A2 — ask the shell WHY the sidecar is down. A missing bundled
        // binary is a broken install and must say so (with the remedy), not
        // masquerade as a transient "connecting" state.
        try {
          const { sidecarProbe } = await import('./tauri');
          const probe = await sidecarProbe();
          markSidecarOffline(
            probe.state === 'missing'
              ? probe.detail
              : 'The coordinator sidecar is not available.',
          );
        } catch {
          markSidecarOffline();
        }
      } else {
        // A supervisor restart is a new live session. Rehydrate native
        // projections and discard transient errors from the old relay.
        if (!wasSidecarReady) {
          hydrated = false;
          fault = undefined;
        }
        wasSidecarReady = true;
        if (status.persistence === 'ephemeral') {
          setRuntimeState('degraded', 'Vault persistence is unavailable.');
        } else if (fault) {
          setRuntimeState('degraded', fault);
        } else {
          markRuntimeLive();
        }
      }
      return status;
    } catch (error) {
      recordFault('runtime readiness probe', error);
      return null;
    }
  };

  const loadLiveData = async () => {
    if (!alive || hydrating) return;
    hydrating = true;
    fault = undefined;
    try {
      try {
        const manifests = await acpAgents();
        const installs = await acpInstallStatus();
        const merged = mergeAgentCatalog(AGENTS, manifests, installs);
        if (alive) useAppStore.getState().setLiveAgents(merged);
      } catch (error) {
        recordFault('agent registry', error);
      }

      try {
        const snap = await usageSnapshot();
        const budget: LiveBudget = {
          spent: snap.byKey.reduce((s, k) => s + (k.costUsd ?? 0), 0),
          cap: 2,
          tokens: snap.total.tokensIn + snap.total.tokensOut,
          cacheHitRate: snap.cacheHitRate,
        };
        if (alive) {
          useAppStore.getState().setLiveBudget(budget);
          const top = snap.byKey[0];
          if (top?.key) {
            const st = useAppStore.getState();
            useAppStore.setState({ streamStats: { ...st.streamStats, activeKey: top.key } });
          }
        }
      } catch (error) {
        recordFault('usage ledger', error);
      }

      try {
        const { invoke } = await import('./tauri');
        const listed = await nativeCall('session list', () => invoke<{ sessions?: Array<import('./store').Session> }>('session_list'));
        if (alive) {
          // An empty native list is authoritative. It replaces the browser
          // seed with an empty real vault and prevents fake chats persisting.
          useAppStore.getState().markSessionsHydrated();
          // P50.2.1 — schema-wrong rows (valid JSON, no usable id) are
          // dropped, never rendered as broken chats.
          const sessions = sanitizeSessionRows(listed?.sessions);
          // P38 — rehydrate per-session Chief pins from the vault round-trip
          // (each Session carries its durable `chiefPin`), so pins set before
          // the app restarted are live again in the store mirror.
          const sessionChiefs: Record<string, string> = {}
          for (const s of sessions) {
            if (s.chiefPin) sessionChiefs[s.id] = s.chiefPin
          }
          // P50.2.1 — hydration is authoritative for vault rows but never
          // clobbers a session created while `session_list` was in flight
          // (local-only, persisted later via session_put), and keeps the
          // active target when it survives (no mid-conversation focus yank).
          useAppStore.setState((st) => {
            const merged = mergeHydratedSessions(st.sessions, sessions, st.activeSessionId)
            return {
              sessions: merged.sessions,
              activeSessionId: merged.activeSessionId,
              ...(Object.keys(sessionChiefs).length > 0 ? { sessionChiefs } : {}),
            }
          });
        }
      } catch (error) {
        recordFault('chat store', error);
      }

      try {
        const items = await workList();
        if (alive) {
          const active = useAppStore.getState().activeSessionId;
          const current = items.find((w) => w.sessionId === active) ?? items[0];
          const snapshot = current ? await workSnapshot(current.workId) : null;
          useAppStore.getState().setWorkProjection(items, snapshot?.presence, snapshot?.events);
        }
      } catch (error) {
        recordFault('work gateway', error);
      }

      // P50.4.1/4.9 — the live provider-configured fact (vault has ≥1 BYOK
      // key). Drives the first-run setup gate, the no-provider chat empty
      // state, and the capability matrix. `null` until first probe.
      try {
        const { invoke } = await import('./tauri');
        const listed = await nativeCall('vault keys', () =>
          invoke<{ keys?: unknown[] }>('vault_keys_list'),
        );
        if (alive) {
          useAppStore.getState().setProviderKeysConfigured((listed?.keys?.length ?? 0) > 0);
        }
      } catch (error) {
        // Vault locked is a normal early state — the fact stays `null`
        // (unknown) until the vault opens; never guess.
        if (alive && (useAppStore.getState().providerKeysConfigured === null)) {
          const status = await readRuntimeStatus().catch(() => null);
          if (status?.vault === 'ready') recordFault('vault keys', error);
        }
      }

      // P38 — the user's primary_chief default, read live at hydration so the
      // chat send path resolves the effective Chief without stale config.
      try {
        const { chiefDefaultGet } = await import('./acp');
        const cfg = await chiefDefaultGet();
        if (alive && cfg?.primaryChief) useAppStore.getState().setUserDefaultChief(cfg.primaryChief);
      } catch (error) {
        recordFault('chief default', error);
      }

      if (alive) {
        const { guardTickets } = await import('./guard');
        const pollGuard = async () => {
          try {
            const tickets = await guardTickets();
            for (const t of tickets) {
              if (!alive || seenTickets.has(t.ticketId)) continue;
              seenTickets.add(t.ticketId);
              const st = useAppStore.getState();
              // P58.9 — Guard approvals are a safety surface: category
              // `always`, never suppressed by a notification preference.
              pushLive({
                id: `live:guard:${t.ticketId}`,
                kind: 'guard',
                title: `Approval needed: ${t.operation}`,
                detail: `${t.paths.join(', ')} — ${t.risk} risk`,
                ts: Date.now(),
                unread: true,
                source: 'Guard',
              }, 'always');
              const snap = st.taskSnapshot;
              const frozenLow = !!snap && snap.sessionId === t.sessionId &&
                (snap.autonomyLevel === 'sandbox' || snap.autonomyLevel === 'ask');
              if (frozenLow) {
                st.pushAutonomyLimit({
                  id: t.ticketId,
                  action: `${t.operation} · ${t.paths.join(', ')}`,
                  reason: t.decision?.goal ?? `${t.operation} on ${t.paths.length} path(s) — ${t.risk} risk`,
                  sessionId: t.sessionId,
                });
              } else {
                st.pushMcq({
                  id: t.ticketId,
                  title: `${t.operation} · ${t.paths.join(', ')}`,
                  description: t.decision?.goal ?? `${t.operation} on ${t.paths.length} path(s) — ${t.risk} risk`,
                  kind: 'permission',
                  approvalNonce: t.approvalNonce,
                  options: [
                    { label: 'Approve & run', value: 'approve' },
                    { label: 'Reject', value: 'reject' },
                  ],
                }, t.sessionId);
              }
            }
          } catch (error) {
            recordFault('Guard-2 ticket poll', error);
          }
        };
        await pollGuard();
        guardTimer = setInterval(() => void pollGuard(), 2000);
      }
      hydrated = true;
      if (alive) {
        const status = await readRuntimeStatus().catch(() => null);
        if (status?.vault === 'ready' && status.sidecar && !fault && status.persistence !== 'ephemeral') {
          markRuntimeLive();
        }
      }
    } finally {
      hydrating = false;
    }
  };

  markRuntimeBooting();
  try {
    // Tauri events are process-wide. Attach exactly once for this bridge
    // lifetime; sidecar restarts reuse the same event channel and must not
    // create duplicate transcript updates.
    unlistenChat = await onChatEvent(handleChatEvent);
    if (!alive) {
      unlistenChat();
      unlistenChat = undefined;
    }
  } catch (error) {
    recordFault('chat event listener', error);
  }
  const initial = await updateReadiness();
  if (initial?.vault === 'ready' && initial.sidecar) {
    await loadLiveData();
  }
  readinessTimer = setInterval(() => {
    void updateReadiness().then((status) => {
      if (status?.vault === 'ready' && status.sidecar && !hydrated) void loadLiveData();
    });
    // 5s: a status probe needs no faster cadence — sidecar restart detection
    // within ~5s is fine and each tick is a Tauri IPC round-trip that can
    // otherwise keep the UI busy on slower machines (P45 R4 micro-perf).
  }, 5000);

  return () => {
    alive = false;
    if (readinessTimer) clearInterval(readinessTimer);
    if (guardTimer) clearInterval(guardTimer);
    unlistenChat?.();
    unlistenChat = undefined;
    seenTickets.clear();
  };
}

// Single source of truth for catalog→registry id translation and live binding
// resolution lives in `./acp` (`acpIdFor` / `currentBinding`); do not
// re-introduce either map here.

/**
 * Send a user turn through the bound external agent when the desktop shell is
 * live. Preview and blocked readiness paths never submit a synthetic turn.
 * `context` (P4.7 chat-overlay) injects an open document's text below the
 * cache boundary as a J6 `<user_document>`.
 */
export async function sendUserMessage(
  text: string,
  context?: { title: string; content: string },
  opts?: { bypassQueue?: boolean },
): Promise<void> {
  let st = useAppStore.getState();
  const trimmed = text.trim();
  if (!trimmed) return;

  // P50.2.1 — never dispatch a turn against a session that does not exist.
  // An empty vault (or a wiped id) means the first message opens the work:
  // create the session first so the turn has a real target and the message
  // is never silently dropped by pushUserMessage's id match.
  let sessionId = st.activeSessionId;
  if (!st.sessions.some((s) => s.id === sessionId)) {
    st.newSession();
    st = useAppStore.getState();
    sessionId = st.activeSessionId;
  }

  // P71.2c — there is no built-in engine to fall back to (ADR-0005 §1/§2), so
  // a turn runs under the session's **bound agent**: the session pin → the user
  // default → the selected agent. Resolve the same first candidate the composer
  // and setup gate show; an explicit non-runnable pin does not silently fall
  // through to a different agent.
  const boundAgent =
    currentBinding(st.sessionChiefs[sessionId]) ??
    currentBinding(st.userDefaultChief) ??
    currentBinding(st.selectedAgentId);
  const boundAcpId = boundAgent ? acpIdFor(boundAgent) : undefined;
  const runtime = boundAcpId
    ? st.liveAgents.find((agent) => acpIdFor(agent.id) === boundAcpId)
    : undefined;
  const readiness = runtime?.readiness;

  // The transcript and running state are the expensive, user-visible lifecycle
  // boundary. Refuse before either changes when there is no proven runnable
  // binding, and before a busy turn is allowed to grow the queue.
  if (!boundAgent || !isAgentReady(readiness as AgentReadiness)) {
    const agentLabel = runtime?.name ?? boundAgent;
    const blocker = !boundAgent
      ? {
          sessionId,
          code: 'unbound' as const,
          title: 'No runnable agent bound',
          detail:
            'Choose an installed, ready agent before sending. AgentCowork ships no built-in engine in v1.',
        }
      : !runtime
        ? {
            sessionId,
            code: 'readiness-unknown' as const,
            title: `${agentLabel} is not verified as runnable`,
            detail:
              'The agent is bound, but the desktop has no readiness result for it. Rescan and finish setup before sending.',
            agentId: boundAgent,
          }
        : {
            sessionId,
            code: 'not-ready' as const,
            title: `${agentLabel} is not ready`,
            detail: `The agent is bound, but it is ${readinessLabel(readiness)}. Finish that setup before sending.`,
            agentId: boundAgent,
          };
    st.setAgentSendBlocker(blocker);
    if (!st.composerValue.trim()) st.setComposerValue(trimmed);
    st.openSetup();
    st.notify(blocker.detail, 'error');
    return;
  }
  st.setAgentSendBlocker(undefined);

  // Browser preview has fixtures, never a runnable external agent. Keep the
  // composer draft and the session idle here as well; a preview must not forge
  // a submitted turn that can never start.
  if (!inTauri()) {
    st.setAgentSendBlocker({
      sessionId,
      code: 'preview',
      title: 'Preview cannot run an agent',
      detail: 'Open the desktop app and bind a ready agent before sending this message.',
      ...(boundAgent ? { agentId: boundAgent } : {}),
    });
    if (!st.composerValue.trim()) st.setComposerValue(trimmed);
    st.notify('Preview mode — open the desktop app to run an external agent.', 'error');
    return;
  }

  // P51.5 — queue-while-generating: while this session's agent is busy, a
  // new ask becomes a pending chip (editable/removable above the composer)
  // and fires when the current turn ends. The composer stays usable and no
  // message is dropped or silently merged. The queue dispatcher passes
  // bypassQueue so the FIFO always runs one-in-flight.
  const busy = st.sessions.some(
    (s) =>
      s.id === sessionId &&
      (s.status === 'running' || s.status === 'action-required'),
  );
  if (busy && !opts?.bypassQueue) {
    st.queueTurn(sessionId, trimmed, context);
    st.notify('Queued — I’ll start it when the current turn finishes');
    return;
  }

  // P33 scoped-PDF fix — when the study-mode chip is set (chat scoped to an
  // open document) and no explicit context was passed, attach the open
  // document's extracted text so answers are grounded in it.
  let effectiveContext = context;
  if (!effectiveContext && st.scopedView === 'office-pdf' && st.scopedDoc) {
    effectiveContext = { title: st.scopedDoc.title, content: st.scopedDoc.content };
  }

  st.pushUserMessage(trimmed);
  // P44.6 — freeze the autonomy scope (level + mode + workspace + agent) into
  // the task's config_hash at start. Live chatbar changes never mutate an
  // in-flight Work; the snapshot + any temporary elevation clear at turn end.
  st.freezeTaskSnapshot();

  try {
    if (st.composerMode === "plan") {
      const { draftPlanTasks } = await import("./plan-draft");
      const tasks = draftPlanTasks(trimmed);
      const planId = `plan-${Date.now()}`;
      const workId = sessionId;
      st.setPendingPlan({ planId, sessionId, streamId: `plan-draft-${Date.now()}`, workId, tasks });
      st.streamStart(sessionId);
      const body = [
        "Plan (read-only — Codex/Claude plan mode). Approve to execute:",
        ...tasks.map((t, i) => `${i + 1}. ${t.goal}`),
      ].join("\n");
      st.streamAppend(body, false, sessionId);
      st.pushMcq({
        id: planId,
        title: "Approve this plan?",
        description: `${tasks.length} task(s). Nothing has been executed.`,
        kind: "plan",
        options: [
          { label: "Approve & run", value: "approve" },
          { label: "Discard", value: "reject" },
        ],
      });
      return;
    }
    {
      // P71.2c — the session runs under its bound agent; launch THAT agent on
      // the ACP channel. This is the only v1 turn path (ADR-0005 §1).
      // P53.4 — the FIRST ACP turn after other work carries the
      // compact-before-swap handoff bundle (compacted transcript + goal +
      // taste + file refs; tool blobs stripped). Follow-up turns on the same
      // handle send no bundle (the agent already holds the context).
      const chiefId = boundAgent;
      const acpId = acpIdFor(chiefId);
      // Handle identity uses the canonical ACP registry id. The catalog id is
      // retained only for the agent-owned config projection below.
      const handleKey = acpId;
      const configKey = chiefId;
      let handleRecord = st.getAcpHandle(sessionId, handleKey);
      let firstTurn = false;
      if (!handleRecord) {
        const folder =
          st.sessions.find((s) => s.id === sessionId)?.folder ?? "~";
        const info = await acpLaunch(acpId, folder);
        handleRecord = acpHandleRecordFromLaunch(info, sessionId, handleKey);
        st.setAcpHandle(handleRecord);
        // P60 — the session-new response carries the agent's own config
        // vocabulary (model/mode/reasoning). Keep it keyed by the agent so the
        // composer can show what that agent actually exposes.
        if (info.configOptions) st.setAcpConfigOptions(configKey, info.configOptions);
        firstTurn = true;
      }
      let handoff: string | undefined;
      if (firstTurn) {
        const { buildChiefHandoff } = await import("./chief-handoff");
        handoff = buildChiefHandoff(sessionId) ?? undefined;
      }
      const handle = handleRecord.handle;
      // P53.8 — refs are sent separately so an ACP agent with
      // `embeddedContext` receives resource blocks. P33 — a chat scoped to an
      // open document travels as labelled prompt text, because the native
      // prompt compiler that used to wrap it (J6 `<user_document>`) went with
      // the built-in engine; the proper attachment surface for a bound agent
      // is `P71.9`, and dropping the document silently would be worse.
      const refPaths = [...trimmed.matchAll(/(?:^|\s)@([A-Za-z0-9_.\-][\w\-./]*)/g)].map((m) => m[1]).filter(Boolean);
      const promptText = effectiveContext
        ? `${trimmed}\n\nDocument in scope — ${effectiveContext.title}:\n${effectiveContext.content}`
        : trimmed;
      const result = await acpPrompt(
        handle,
        sessionId,
        promptText,
        handoff,
        refPaths,
        handleRecord?.bindingId || undefined,
      );
      if (result.handle !== handle || (result.applicationSessionId && result.applicationSessionId !== sessionId)) {
        throw new Error('ACP prompt returned a handle owned by another chat');
      }
      // The shell returns the canonical owner after durable Work/Binding/Run
      // admission. Re-key the live record from its provisional launch identity
      // to that explicit tuple before the next turn can resolve it.
      if (result.applicationSessionId && result.workId && result.bindingId && handleRecord) {
        handleRecord = {
          ...handleRecord,
          applicationSessionId: result.applicationSessionId,
          workId: result.workId,
          bindingId: result.bindingId,
          runId: result.runId,
          providerSessionId: result.providerSessionId,
          key: acpHandleKey(
            result.applicationSessionId,
            result.bindingId,
            result.workId,
            handleRecord.agentId,
          ),
        };
        st.setAcpHandle(handleRecord);
      }
      // P53.5 — visible assistant text folds into the compacted session;
      // tool history stays in the per-session observability file (never
      // imported into chat context). Refresh the cached live slash vocab
      // (P53.1) so the composer's next `/` reflects this turn's advert.
      const seenCommands = (result.updates ?? []).some(
        (u) => u.sessionUpdate === "available_commands_update" && (u.availableCommands?.length ?? 0) > 0,
      );
      if (seenCommands) {
        void import("./acp").then(({ acpSessionCommands }) =>
          acpSessionCommands(handle).catch(() => []),
        );
      }
      // P60 — an agent-initiated `config_option_update` (e.g. it fell back to
      // another model) replaces the stored list; reflect it instead of
      // rendering a stale selection.
      const configUpdate = (result.updates ?? []).find(
        (u) =>
          u.sessionUpdate === "config_option_update" &&
          (u.configOptions?.length ?? 0) > 0,
      );
      if (configUpdate?.configOptions) {
        st.setAcpConfigOptions(configKey, configUpdate.configOptions);
      }
      const pending = result.pendingTickets?.length
        ? ` · ${result.pendingTickets.length} approval(s)`
        : "";
      st.streamStart(sessionId);
      // ACP currently returns collected session updates rather than streaming
      // them over Tauri. Render the actual returned assistant text when the
      // native contract provides it; never replace it with an "ACP done"
      // status-only placeholder. Older shells still get an honest fallback.
      const finalText = result.finalText?.trim() ||
        (result.updates ?? [])
          .flatMap((u) => u.content ?? [])
          .map((b) => b.text)
          .filter(Boolean)
          .join('') ||
        `ACP ${result.stopReason ?? "done"}${pending}`;
      st.streamAppend(finalText, true, sessionId);
      return;
    }
  } catch (err) {
    // P11.5.12 — a dropped IPC mid-stream surfaces the reconnect chip instead
    // of a hard failure; the coordinator's StreamRegistry holds the last-token
    // cursor so a resume replays byte-continuously (the chip auto-clears when
    // the next batch lands via streamAppend).
    const stNow = useAppStore.getState();
    const targetSess = stNow.sessions.find((s) => s.id === sessionId);
    const running = targetSess?.status === "running";
    if (running) {
      stNow.setReconnect({
        show: true,
        lastToken: stNow.streamStats.tokensThisTurn > 0 ? "…" : "",
        tokens: stNow.streamStats.tokensThisTurn,
      });
      return;
    }
    const message = err instanceof Error ? err.message : "Failed to reach the agent";
    setRuntimeState('degraded', `chat stream: ${message}`);
    st.streamFail(message, sessionId);
  }
}
