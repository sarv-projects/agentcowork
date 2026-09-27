/**
 * Scheduled-task **webhook ingress** (P6.4 — B7; re-scoped by `P71.2c` /
 * `ARCH/AUTOMATION.md` §9).
 *
 * Until `P71.2c` this module was also the *executor*: it ticked
 * `scheduler/due`, reawakened each job's session through its own built-in chat
 * turn (`runChatStream` → `core-engine`) and then recorded the firing. That
 * path assumed an AgentCowork-owned model, which `ARCH/ADR/0005` retires for v1 —
 * and `ARCH/AUTOMATION.md` §9/§6 are explicit that
 * **agent execution is never the scheduler's**: a firing runs through the
 * session's *bound agent*.
 *
 * The firing therefore lives in the shell (`src-tauri/src/scheduler_fire.rs`),
 * which already owns the trigger plane, the Work gateway and the ACP channel.
 * What remains here is the one piece the Rust core deliberately does not host:
 * the **F11 loopback webhook listener** — `127.0.0.1` only, body validation and
 * occurrence recording still owned by Rust (`scheduler/fire_webhook`).
 *
 * Nothing in this module reasons, executes or holds credentials; it is a
 * transport adapter onto the trigger plane.
 */

import { resolveWebhookPort } from "./env";

/** Outbound JSON-RPC request to Rust. */
type SchedulerRequest = (method: string, params: unknown) => Promise<unknown>;

const WEBHOOK_IDEMPOTENCY_HEADERS = ["idempotency-key", "x-idempotency-key"] as const;
const WEBHOOK_BODY_IDEMPOTENCY_KEYS = [
  "deliveryId",
  "delivery_id",
  "idempotencyKey",
  "idempotency_key",
  "requestId",
  "request_id",
  "eventId",
  "event_id",
] as const;

function normalizeIdempotencyKey(value: unknown, source: string): string {
  if (typeof value !== "string") {
    throw new Error(`${source} idempotency key must be a string`);
  }
  const key = value.trim();
  if (!key || key.length > 256 || /[\u0000-\u001f\u007f]/u.test(key)) {
    throw new Error(`${source} idempotency key is invalid`);
  }
  return key;
}

/**
 * Resolve the caller-owned key for a loopback webhook.
 *
 * The standard HTTP header wins, while the legacy body spellings remain an
 * explicit compatibility path. A missing or ambiguous key is rejected before
 * the request reaches the scheduler; no timestamp or body digest is generated
 * here. If a caller supplied both forms, they must agree.
 */
export function webhookIdempotencyKey(
  req: { headers: { get(name: string): string | null } },
  body: unknown,
): string {
  const headerKeys = WEBHOOK_IDEMPOTENCY_HEADERS.map((name) => req.headers.get(name))
    .filter((value): value is string => value !== null)
    .map((value) => normalizeIdempotencyKey(value, "webhook header"));
  if (new Set(headerKeys).size > 1) {
    throw new Error("webhook ingress supplied conflicting idempotency headers");
  }

  let bodyKey: string | undefined;
  if (body !== null && typeof body === "object" && !Array.isArray(body)) {
    const record = body as Record<string, unknown>;
    const bodyKeys = WEBHOOK_BODY_IDEMPOTENCY_KEYS.filter((name) => name in record).map((name) =>
      normalizeIdempotencyKey(record[name], "webhook body"),
    );
    if (new Set(bodyKeys).size > 1) {
      throw new Error("webhook ingress supplied conflicting body idempotency keys");
    }
    bodyKey = bodyKeys[0];
  }

  const headerKey = headerKeys[0];
  if (headerKey && bodyKey && headerKey !== bodyKey) {
    throw new Error("webhook ingress header and body idempotency keys conflict");
  }
  const key = headerKey ?? bodyKey;
  if (!key) {
    throw new Error("webhook ingress requires a caller idempotency key");
  }
  return key;
}

/** The loopback webhook listener handle (tests call `stop()`; `0` port = not started). */
export interface WebhookIngress {
  start(): void;
  stop(): void;
  webhookPort(): number;
}

/**
 * Start the F11 loopback webhook listener: `127.0.0.1` only, POST bodies
 * forwarded verbatim to Rust, which checks the path against the registered
 * webhook triggers and the required keys against the trigger's schema. A body
 * that matches no trigger (or misses a required key) is a `422`, never a fire.
 */
export function startWebhookIngress(request: SchedulerRequest): WebhookIngress {
  let stopped = false;
  let webhookPort = 0;

  function start(): void {
    if (webhookPort > 0 || stopped) return;
    // DEC-053: `AGENTCOWORK_WEBHOOK_PORT`, with the legacy
    // `EVERYAIOS_WEBHOOK_PORT` spelling honored (absent → 0, ephemeral).
    const port = resolveWebhookPort();
    try {
      const server = Bun.serve({
        port,
        hostname: "127.0.0.1",
        async fetch(req) {
          if (req.method !== "POST") {
            return new Response("method not allowed", { status: 405 });
          }
          const url = new URL(req.url);
          const raw = await req.text();
          let body: unknown;
          try {
            body = raw ? JSON.parse(raw) : {};
          } catch {
            return new Response("invalid JSON", { status: 400 });
          }
          let idempotencyKey: string;
          try {
            idempotencyKey = webhookIdempotencyKey(req, body);
          } catch {
            return new Response(JSON.stringify({ ok: false, error: "idempotency key required" }), {
              status: 400,
              headers: { "content-type": "application/json" },
            });
          }
          try {
            const out = (await request("scheduler/fire_webhook", {
              path: url.pathname,
              body,
              idempotencyKey,
              now: Math.floor(Date.now() / 1000),
            })) as { fired: string[] };
            return new Response(
              JSON.stringify({ ok: true, fired: out.fired }),
              { status: 200, headers: { "content-type": "application/json" } },
            );
          } catch {
            return new Response(JSON.stringify({ ok: false }), {
              status: 422,
              headers: { "content-type": "application/json" },
            });
          }
        },
      });
      webhookPort = server.port ?? 0;
    } catch {
      // No Bun.serve in the test runner / platform without it — non-fatal.
      webhookPort = 0;
    }
  }

  function stop(): void {
    stopped = true;
  }

  return { start, stop, webhookPort: () => webhookPort };
}
