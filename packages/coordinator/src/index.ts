#!/usr/bin/env bun
// NOTE: Heap pressure is handled by this process's own heap monitor (heap.ts —
// self-restart at 80%, J13); the Rust ProcessSupervisor must NOT set
// BUN_JSC_heapSize — Bun ≥1.3 rejects it as an invalid JSC env var and exits(1)
// before running any app code (verified 2026-08-17). For Node.js:
// --max-old-space-size=512. For dev: `bun --smol run src/index.ts`
/**
 * AgentCowork coordinator sidecar — hello-world IPC responder (P0.3).
 *
 * Speaks the exact `agentcowork-ipc` wire contract:
 * - JSON-RPC 2.0 over stdio
 * - length-prefix framing `[u32 LE length][JSON payload]`
 * - ACP-style `initialize` handshake (protocolVersion + default-off
 *   capabilities, doc 45) so the contract evolves without breaking peers.
 *
 * P0.3 scope: the loop + handshake + echo. Later phases plug the real
 * `@agentcowork/core-*` engine stages (chat, memory, office, connectors)
 * into this same process.
 */

import { FrameDecoder, encodeJson, notify } from "./frame";
import { dispatchAguiLine } from "./agui";
import { noteSidecarRequest, SIDECAR_IDLE_EXIT_CODE, sidecarIdleDecision, sidecarIdleSnapshot, startHeapMonitor } from "./heap";
import { startOrphanWatch } from "./orphan";
import {
  ERROR_CODES,
  err,
  isRequest,
  methodNotFound,
  ok,
  type Request,
  type Response,
} from "./message";
// P71.2c — the coordinator no longer owns a turn loop: `chat.ts`, `plan.ts` and
// the native tool catalogue (`tools.ts`) moved to
// `ARCH/archive/coordinator-loop/` with the built-in engine (ADR-0005 §2), and
// `@agentcowork/core-engine` moved to `ARCH/archive/core-engine/`. A turn is
// driven by the bound external agent on the ACP channel
// (`acp_launch` → `acp_prompt`, in the shell), so the `chat/*` and `plan/*`
// arms below are gone with their producers. What this process still owns is the
// **shared plane**: memory/context, guard, work, skills, delegation, scheduler
// ingress, connectors, MCP, catalog and readiness — the services a turn calls
// into, never the reasoning that decides what to call.
import { startWebhookIngress } from "./scheduler";
import { resolveHeartbeatIntervalMs } from "./env";
import { hydrateObservations, type DurableUsageRow } from "./observations";
import { connectorCatalog, queryConnectors } from "./connector-bridge";
import { searchExternalMcp } from "./mcp-bridge";
import {
  governanceBadge,
  isRetiredAgentId,
  primaryAgentRegistry,
  resolvePrimaryAgentId,
  resolveSessionPrimaryAgent,
  type GovernanceMode,
} from "./primary-agent";

/** Must stay in lock-step with `agentcowork_ipc::PROTOCOL_VERSION` (Rust, = 1). */
export const PROTOCOL_VERSION = 1;

/** Capabilities this side supports; advertised at handshake (all default-off). */
export interface Capabilities {
  streamDeltas?: boolean;
  passByReference?: boolean;
  [key: string]: unknown;
}

export const DEFAULT_CAPABILITIES: Capabilities = {
  // P1.4: the sidecar now streams chat token deltas (capability flips on).
  streamDeltas: true,
  passByReference: true,
};

// P71.2c — the provider bridge, the `chat/*` notification envelope and the
// `streamId → identity` registry are gone with the loop that used them: the
// sidecar no longer asks Rust to run a provider call and no longer emits turn
// events (ADR-0005 §2). `RunIdentity`/`envelopeEvent` remain exported for the
// **Work-event** plane, which is a different producer (the work gateway).

/** Outbound request correlation: id → pending promise (sidecar → Rust). */
const pending = new Map<
  string,
  { resolve: (v: unknown) => void; reject: (e: Error) => void }
>();
let requestCounter = 0;

/**
 * Send a request to Rust (the core) and await its response. Responses are
 * matched by id in `run()`; the sidecar never blocks its frame loop.
 */
function sendRequest(method: string, params: unknown): Promise<unknown> {
  const id = `c${++requestCounter}`;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    process.stdout.write(encodeJson({ jsonrpc: "2.0", method, params, id }));
  });
}

export const VERSION = "0.1.0";

/** Heartbeat interval in ms (default 10s). Env-overridable for tests. */
export const DEFAULT_HEARTBEAT_MS = 10_000;

export interface InitializeParams {
  protocolVersion?: number;
  clientName?: string;
  capabilities?: Capabilities;
}

/**
 * Handle one request object → response. Unknown methods get
 * METHOD_NOT_FOUND; notifications (no id) return `null` (nothing to send —
 * JSON-RPC 2.0 §2.2: the server MUST NOT reply to a notification).
 */
export function handleRequest(req: Request): Response | null {
  const id = req.id ?? null;

  let response: Response | null;

  switch (req.method) {
    case "initialize": {
      const p = (req.params ?? {}) as InitializeParams;
      const peerVersion = p.protocolVersion ?? 0;
      if (peerVersion !== PROTOCOL_VERSION) {
        response = err(
          id,
          ERROR_CODES.INVALID_REQUEST,
          `unsupported protocolVersion: ${peerVersion} (this side speaks ${PROTOCOL_VERSION})`,
        );
      } else {
        response = ok(id, {
          protocolVersion: PROTOCOL_VERSION,
          serverName: "@agentcowork/coordinator",
          serverVersion: VERSION,
          capabilities: DEFAULT_CAPABILITIES,
          status: "ready",
        });
      }
      break;
    }

    case "echo": {
      const p = (req.params ?? {}) as { text?: unknown; data?: unknown };
      response = ok(id, {
        text: p.text ?? p.data ?? null,
        echoed: true,
      });
      break;
    }

    case "chief/resolve": {
      // P38 — the dispatcher resolves `primary_chief` (explicit session value
      // → user default → none) and records the session's primary agent so Work
      // survives agent death (same intent→plan→checkpoints→receipts chain).
      // P71.5b — retired built-in spellings name no agent, so `chiefId` may be
      // null: the response says `null` instead of inventing an engine.
      const p = (req.params ?? {}) as {
        explicit?: string;
        userDefault?: string;
        sessionId?: string;
        governance?: GovernanceMode;
      };
      try {
        const chiefId = resolvePrimaryAgentId(p.explicit, p.userDefault);
        // Bugfix — the response badge used to be hardcoded `not_governed`,
        // ignoring the governance this call resolves/records. Reflect the
        // actual mode (or stay honestly not_governed when none is supplied —
        // there is no governance signal to report then).
        const governance: GovernanceMode = p.governance ?? { kind: "not_governed" };
        if (typeof p.sessionId === "string" && chiefId !== null) {
          const prev = primaryAgentRegistry.get(p.sessionId);
          primaryAgentRegistry.record({
            sessionId: p.sessionId,
            agentId: chiefId,
            governance,
            lastCompletedTurn: prev?.lastCompletedTurn ?? 0,
            configHash: prev?.configHash ?? "",
          });
        }
        response = ok(id, { chiefId, badge: governanceBadge(governance) });
      } catch (e) {
        response = err(
          id,
          ERROR_CODES.INVALID_REQUEST,
          e instanceof Error ? e.message : String(e),
        );
      }
      break;
    }

    case "chief/set_session": {
      // P38 — pin a session to an agent (per-session override). Fail-closed:
      // empty and retired built-in ids refuse (P71.5b); the resolved pin is
      // returned so the caller can confirm the effective agent for the session.
      const p = (req.params ?? {}) as { sessionId?: string; chiefId?: string };
      if (typeof p.sessionId !== "string" || p.sessionId === "" || typeof p.chiefId !== "string") {
        response = err(id, ERROR_CODES.INVALID_REQUEST, "chief/set_session requires sessionId and chiefId");
        break;
      }
      try {
        const agentId = primaryAgentRegistry.setSessionPin(p.sessionId, p.chiefId);
        const governance: GovernanceMode = { kind: "self_contained", channelB: true };
        response = ok(id, { sessionId: p.sessionId, chiefId: agentId, badge: governanceBadge(governance) });
      } catch (e) {
        response = err(
          id,
          ERROR_CODES.INVALID_REQUEST,
          e instanceof Error ? e.message : String(e),
        );
      }
      break;
    }

    case "chief/resolve_session": {
      // P38 — the dispatcher-side read: what agent does THIS session run
      // under right now? Session pin → user default → none (P71.5b — there is
      // no built-in engine to fall back to, and a retired spelling resolves
      // to `null`, which the turn path refuses by name). Used by the chat
      // path as the single dispatch decision and by the firing path
      // (`scheduler_fire.rs`) as the automation binding.
      const p = (req.params ?? {}) as { sessionId?: string; userDefault?: string };
      if (typeof p.sessionId !== "string" || p.sessionId === "") {
        response = err(id, ERROR_CODES.INVALID_REQUEST, "chief/resolve_session requires sessionId");
        break;
      }
      try {
        const pin = primaryAgentRegistry.sessionPin(p.sessionId);
        const chiefId = resolveSessionPrimaryAgent({
          ...(pin !== undefined ? { sessionPin: pin } : {}),
          ...(typeof p.userDefault === "string" && !isRetiredAgentId(p.userDefault)
            ? { userDefault: p.userDefault }
            : {}),
        });
        response = ok(id, {
          sessionId: p.sessionId,
          chiefId,
          source: pin ? "session-pin" : chiefId !== null ? "user-default" : "none",
        });
      } catch (e) {
        response = err(
          id,
          ERROR_CODES.INVALID_REQUEST,
          e instanceof Error ? e.message : String(e),
        );
      }
      break;
    }

    case "session/ping": {
      response = ok(id, { pong: true, ts: Date.now(), heapMB: heapUsedMB() });
      break;
    }

    // P71.2c — `chat/stream`, `chat/cancel`, `chat/tool_retry`,
    // `chat/provider_chunk`, `plan/execute`, `plan/respond` and `plan/cancel`
    // were the built-in engine's dispatch surface. They are deleted with it
    // (ADR-0005 §2): a turn is driven by the bound external agent through the
    // shell's ACP channel, so the coordinator is never asked to run one. A
    // request that still arrives gets an honest METHOD_NOT_FOUND below rather
    // than a silent no-op.

    case "mcp/search": {
      const p = (req.params ?? {}) as { endpoint?: unknown; query?: unknown; toolName?: unknown };
      if (typeof p.endpoint !== "string" || typeof p.query !== "string" || p.query.trim().length === 0) {
        response = err(id, ERROR_CODES.INVALID_REQUEST, "mcp/search requires endpoint and query");
        break;
      }
      void searchExternalMcp(
        p.endpoint,
        p.query,
        typeof p.toolName === "string" ? p.toolName : "search",
      ).then(
        (results) => {
          if (req.id !== undefined) process.stdout.write(encodeJson(ok(req.id, { results })));
        },
        (e: Error) => {
          if (req.id !== undefined) process.stdout.write(encodeJson(err(id, ERROR_CODES.INTERNAL_ERROR, e.message)));
        },
      );
      response = null;
      break;
    }

    case "connector/list": {
      response = ok(id, { connectors: connectorCatalog() });
      break;
    }

    case "connector/query": {
      const p = (req.params ?? {}) as { query?: unknown; activeNames?: unknown };
      if (typeof p.query !== "string" || p.query.trim().length === 0) {
        response = err(id, ERROR_CODES.INVALID_REQUEST, "connector/query requires a non-empty query");
        break;
      }
      const activeNames = Array.isArray(p.activeNames)
        ? p.activeNames.filter((name): name is string => typeof name === "string")
        : undefined;
      // Connector adapters own their authorization checks. Results are
      // normalized and returned without exposing credential material.
      void queryConnectors(p.query, activeNames).then(
        (results) => {
          if (req.id !== undefined) process.stdout.write(encodeJson(ok(req.id, { results })));
        },
        (e: Error) => {
          if (req.id !== undefined) process.stdout.write(encodeJson(err(id, ERROR_CODES.INTERNAL_ERROR, e.message)));
        },
      );
      response = null;
      break;
    }

    case "session/shutdown": {
      // Graceful stop: flush the reply (if this was a request) through the
      // write callback, then exit — process.exit() alone would truncate
      // buffered stdout.
      const reply = ok(id, { shuttingDown: true });
      if (req.id !== undefined) {
        process.stdout.write(encodeJson(reply), () => process.exit(0));
      } else {
        queueMicrotask(() => process.exit(0));
      }
      response = null;
      break;
    }

    default:
      response = methodNotFound(id, req.method);
  }

  // JSON-RPC 2.0 §2.2: never reply to a notification.
  return req.id === undefined ? null : response;
}

/** Current heap usage in MB (J13 heap-safety groundwork, P0.3.7). */
export function heapUsedMB(): number {
  try {
    return Math.round(process.memoryUsage().heapUsed / 1024 / 1024);
  } catch {
    return -1;
  }
}

/**
 * Announce readiness on boot. This is the child's first byte on stdout — the
 * Rust ProcessSupervisor treats it as the connect signal (Starting → Running)
 * and it arms the idle-watchdog clock.
 */
export function announceReady(): void {
  notify("session/ready", {
    protocolVersion: PROTOCOL_VERSION,
    serverName: "@agentcowork/coordinator",
    serverVersion: VERSION,
    status: "ready",
  });
}

/**
 * Resolve the heartbeat interval from `AGENTCOWORK_HEARTBEAT_MS` (tests use a
 * short interval), falling back to the legacy `EVERYAIOS_HEARTBEAT_MS`
 * spelling (DEC-053) and then to [`DEFAULT_HEARTBEAT_MS`].
 */
export function heartbeatIntervalMS(): number {
  return resolveHeartbeatIntervalMs(process.env, DEFAULT_HEARTBEAT_MS);
}

/**
 * Start the periodic `session/heartbeat` notification (default every 10s).
 *
 * The supervisor's idle watchdog (30s of silence → kill) must never false-kill
 * a healthy-but-idle process, so the sidecar emits a heartbeat well inside the
 * idle window. Returns the timer (unref'd so it never holds the loop open).
 */
export function startHeartbeat(
  intervalMs: number = heartbeatIntervalMS(),
): NodeJS.Timeout {
  const timer = setInterval(() => {
    notify("session/heartbeat", { ts: Date.now() });
  }, intervalMs);
  if (typeof timer === "object" && "unref" in timer) {
    (timer as NodeJS.Timeout).unref();
  }
  return timer;
}

/** The IPC event loop: read frames from `stdin`, write responses to `stdout`. */
export function run(reader: NodeJS.ReadableStream = process.stdin): void {
  const decoder = new FrameDecoder();
  const textDecoder = new TextDecoder();

  reader.on("data", (chunk: Uint8Array) => {
    let frames: Uint8Array[];
    try {
      frames = decoder.push(chunk);
    } catch (e) {
      // Framing violation — reply with PARSE_ERROR if we can, then stop.
      const msg = e instanceof Error ? e.message : String(e);
      process.stdout.write(
        encodeJson(err(null, ERROR_CODES.PARSE_ERROR, `framing error: ${msg}`)),
      );
      return;
    }

    for (const frame of frames) {
      let parsed: unknown;
      try {
        parsed = JSON.parse(textDecoder.decode(frame));
      } catch {
        process.stdout.write(
          encodeJson(err(null, ERROR_CODES.PARSE_ERROR, "invalid JSON payload")),
        );
        continue;
      }

      // A response to one of our outbound requests (id + result/error, no
      // method) — resolve the pending promise BEFORE the isRequest guard.
      const raw = parsed as {
        jsonrpc?: string;
        id?: unknown;
        result?: unknown;
        error?: { message?: string };
      };
      if (
        raw !== null &&
        typeof raw === "object" &&
        raw.jsonrpc === "2.0" &&
        raw.id !== undefined &&
        ("result" in raw || "error" in raw)
      ) {
        const p = pending.get(String(raw.id));
        if (p) {
          pending.delete(String(raw.id));
          if (raw.error) {
            p.reject(new Error(raw.error.message ?? "sidecar error"));
          } else {
            p.resolve(raw.result);
          }
        }
        continue;
      }

      if (!isRequest(parsed)) {
        process.stdout.write(
          encodeJson(err(null, ERROR_CODES.INVALID_REQUEST, "not a JSON-RPC 2.0 request")),
        );
        continue;
      }

      const response = noteSidecarRequest(() => handleRequest(parsed));
      if (response !== null) {
        process.stdout.write(encodeJson(response));
      }
    }
  });

  reader.on("end", () => {
    // Parent closed the pipe — exit cleanly (orphan-prevention baseline).
    process.exit(0);
  });

  reader.on("error", (e: Error) => {
    // EPIPE/EIO from a dead parent — exit rather than crash unhandled.
    console.error(`coordinator: stdin error: ${e.message}`);
    process.exit(1);
  });
}

/**
 * P6.4 / P71.2c: the trigger plane's loopback webhook ingress. The sidecar no
 * longer ticks or executes firings — the host owns the due loop
 * (`src-tauri/src/scheduler_fire.rs`) and runs each firing through the
 * session's **bound agent**, because agent execution is never the scheduler's
 * (`ARCH/AUTOMATION.md` §9).
 */
export const schedulerWebhooks = startWebhookIngress(sendRequest);

// Only start the loop when run directly (not when imported by tests).
if (import.meta.main) {
  startOrphanWatch();
  startHeapMonitor();
  // P45.8 — exit after 60s with no inbound request and no in-flight turn.
  // The supervisor parks on code 75 and spawns again when the shell sets
  // the resume flag. Heartbeats are outbound and do not reset this clock.
  const idleTimer = setInterval(() => {
    if (sidecarIdleDecision(sidecarIdleSnapshot()) === "exit") {
      process.exit(SIDECAR_IDLE_EXIT_CODE);
    }
  }, 5_000);
  if (typeof idleTimer === "object" && "unref" in idleTimer) idleTimer.unref();
  // ARCH/05 durable-observation seam: hydrate the RouteDecision ring once
  // from the vault's `token_usage` ledger (provider/model/cost per completed
  // call) so routing survives restarts. Best-effort — a missing vault or
  // relay (e.g. first boot) leaves the ring empty and routing falls back to
  // capability-filter + cost-sort until live turns record observations.
  void sendRequest("usage/recent", { limit: 200 })
    .then((res) => {
      const rows = Array.isArray(res) ? (res as DurableUsageRow[]) : [];
      if (rows.length > 0) {
        hydrateObservations(rows);
      }
    })
    .catch(() => {
      /* best-effort — ring stays empty */
    });
  // First byte on stdout → supervisor promotes Starting → Running.
  announceReady();
  // Keeps the supervisor's idle watchdog (30s) from false-killing an idle
  // but healthy process.
  startHeartbeat();
  // P6.4 (B7): host the F11 loopback webhook listener (the due loop runs in
  // the host — the sidecar executes no firings).
  schedulerWebhooks.start();
  run();
}
