#!/usr/bin/env node
/**
 * P50.5.1 — Real vertical chat E2E.
 *
 * Drives the REAL coordinator sidecar over its production stdio JSON-RPC
 * wire (the same contract the Rust relay speaks) and performs REAL provider
 * calls (NVIDIA NIM / BYOK / ollama from env — see lib/provider.mjs). No
 * mocked provider responses anywhere: every token streamed is a real
 * provider response.
 *
 * Asserted legs:
 *   1. handshake (initialize) against the real sidecar
 *   2. a REAL turn: chat/stream → provider/stream → real HTTP SSE →
 *      chat/ttft + chat/batch* + chat/done with non-empty real text
 *   3. cancel: a real in-flight stream aborts → chat/cancelled, never done
 *   4. error honesty: a real provider failure (EOL model) surfaces
 *      chat/error with the real message — never an empty success
 *   5. retry validation: malformed chat/tool_retry is refused
 *   6. restart continuity: a new coordinator process answers the same
 *      sessionId (session identity survives at the wire; vault persistence is
 *      the Rust/UI layer, unit-tested under P50.2.1)
 *
 * Exit: 0 PASS / 1 FAIL / 2 SKIP (no real provider configured).
 */
import { CoordinatorClient, defaultRustReplies } from "./lib/protocol.mjs";
import { resolveProvider, streamChatCompletion } from "./lib/provider.mjs";

const provider = resolveProvider();
if (!provider) {
  console.log("[P50.5.1] SKIP — no real provider configured (set NVIDIA_API_KEY or AGENTCOWORK_E2E_*)");
  process.exit(2);
}
console.log(`[P50.5.1] real provider: ${provider.name} (${provider.baseUrl}, models ${provider.models.join(", ")})`);

/** Inter-leg spacing so a throttled real endpoint can recover (env-tunable). */
const STAGGER_MS = Number(process.env.AGENTCOWORK_E2E_STAGGER_MS ?? process.env.EVERYAIOS_E2E_STAGGER_MS ?? 5_000);
/** The intentionally-broken model for the error-honesty leg (real HTTP 410). */
const EOL_MODEL = "meta/llama-3.1-8b-instruct";

const failures = [];
function assert(cond, label) {
  if (cond) {
    console.log(`  ok — ${label}`);
  } else {
    console.error(`  FAIL — ${label}`);
    failures.push(label);
  }
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function spawnCoordinator() {
  const client = new CoordinatorClient();
  client.on("stderr", (s) => {
    // Keep a bounded tail so failures can be diagnosed.
    const line = s.trim();
    if (line) process.stderr.write(`[coordinator] ${line}\n`);
  });
  const ready = client.waitForNotification("session/ready", { timeoutMs: 30_000 });
  defaultRustReplies(client, {
    providerStreamHandler: async (params) => {
      const messages = Array.isArray(params.messages) ? params.messages : [];
      const noFallback = params.model === EOL_MODEL; // the error leg must fail honestly
      await streamChatCompletion(provider, messages, {
        preferredModel: params.model,
        noFallback,
        onDelta: (text) =>
          client.notify("chat/provider_chunk", { streamId: params.streamId, delta: text }),
        onDone: (u) => {
          if (u.promptTokens !== undefined) {
            client.notify("chat/provider_chunk", {
              streamId: params.streamId,
              usage: { promptTokens: u.promptTokens, completionTokens: u.completionTokens ?? 0 },
            });
          }
          client.notify("chat/provider_chunk", { streamId: params.streamId, ended: true });
        },
      });
    },
  });
  await ready;
  return client;
}

/** One real turn. Positive signals (ack, ttft) are awaited; negative signals
 *  (done, err, cancelled) are returned as LIVE promises so the caller decides
 *  whether to await them or assert their absence. */
async function runRealTurn(client, { sessionId, streamId, text, model, providerName, cancelAfterBatch = false }) {
  const byStream = (p) => p.streamId === streamId;
  const ttftP = client.waitForNotification("chat/ttft", { timeoutMs: 120_000, match: byStream }).catch(() => null);
  const doneP = client.waitForNotification("chat/done", { timeoutMs: 90_000, match: byStream }).catch(() => null);
  const errP = client.waitForNotification("chat/error", { timeoutMs: 90_000, match: byStream }).catch(() => null);
  const cancelledP = client.waitForNotification("chat/cancelled", { timeoutMs: 30_000, match: byStream }).catch(() => null);
  const batches = [];
  const onBatch = (m, p) => {
    if (m === "chat/batch" && p.streamId === streamId) batches.push(p);
  };
  client.on("notification", onBatch);
  const ack = await client.request("chat/stream", {
    sessionId,
    streamId,
    text,
    surface: "chat",
    provider: providerName ?? provider.name,
    model: model ?? provider.models[0],
  });
  if (cancelAfterBatch) {
    // Wait for real tokens to be flowing (cap 60s), then abort.
    const start = Date.now();
    while (batches.length === 0 && Date.now() - start < 60_000) await sleep(50);
    client.notify("chat/cancel", { streamId });
  }
  const ttft = await ttftP;
  // The batch listener stays until the caller has awaited the terminal
  // signal (batches stream AFTER ttft). streamId-scoped arrays make stale
  // listeners harmless across legs.
  return { ack, ttft, batches, done: doneP, err: errP, cancelled: cancelledP };
}

/** Assert a signal does NOT arrive within `ms`. `notifP` is a live promise. */
async function assertAbsent(notifP, label, ms = 4_000) {
  const timer = new Promise((r) => setTimeout(() => r("timeout"), ms));
  const winner = await Promise.race([notifP.then(() => "arrived"), timer]);
  assert(winner === "timeout", label);
}

// ---- 1. handshake ----------------------------------------------------------
const c1 = await spawnCoordinator();
try {
  const init = await c1.request("initialize", { protocolVersion: 1, clientName: "p50.5.1" });
  assert(init?.serverName === "@agentcowork/coordinator", "handshake: initialize returns the real sidecar identity");
  assert(init?.status === "ready", "handshake: status ready");
  const ping = await c1.request("session/ping");
  assert(ping?.pong === true, "session/ping answers");

  // ---- 2. a REAL turn ------------------------------------------------------
  console.log("  real turn: chat/stream against the real provider…");
  const t1 = await runRealTurn(c1, {
    sessionId: "e2e-s1",
    streamId: "e2e-st1",
    text: "Reply with exactly the word: hello",
  });
  await sleep(STAGGER_MS);
  assert(t1.ack?.accepted === true, "chat/stream accepted");
  assert(t1.ttft !== null, "chat/ttft emitted");
  const t1Done = await t1.done;
  const t1Err = await t1.err;
  assert(t1Err === null, "no chat/error on the healthy turn");
  assert(typeof t1Done?.fullText === "string" && t1Done.fullText.length > 0,
    `chat/done carries REAL non-empty text (${t1Done?.fullText?.length ?? 0} chars)`);
  assert(t1.batches.some((b) => (b.text ?? "").length > 0), "chat/batch streamed real tokens");
  assert(t1.ttft?.latencyMs !== undefined, "ttft carries latency");

  // ---- 3. cancel a real in-flight stream -----------------------------------
  console.log("  cancel: abort a real in-flight stream…");
  const t2 = await runRealTurn(c1, {
    sessionId: "e2e-s2",
    streamId: "e2e-st2",
    text: "Write a 300-word essay about the history of typewriters.",
    cancelAfterBatch: true,
  });
  assert(t2.ack?.accepted === true, "cancel leg: stream accepted");
  const t2Cancelled = await t2.cancelled;
  assert(t2Cancelled !== null, "chat/cancelled emitted for the aborted stream");
  await assertAbsent(t2.done, "no chat/done after cancel");

  // ---- 4. honest provider failure (real EOL model on NVIDIA) ---------------
  console.log("  error: real provider failure must surface honestly…");
  const t3 = await runRealTurn(c1, {
    sessionId: "e2e-s3",
    streamId: "e2e-st3",
    text: "hello",
    model: EOL_MODEL, // EOL on NVIDIA → real HTTP 410
  });
  assert(t3.ack?.accepted === true, "error leg: stream accepted");
  const t3Err = await t3.err;
  assert(t3Err !== null, "chat/error surfaced for the real provider failure");
  assert(t3Err?.message?.includes?.("410") || t3Err?.message?.includes?.("end of life"),
    `error message carries the REAL provider detail (${(t3Err?.message ?? "").slice(0, 80)})`);
  await assertAbsent(t3.done, "no empty 'successful' done on provider failure");

  // ---- 5. retry validation -------------------------------------------------
  console.log("  retry: malformed chat/tool_retry must be refused…");
  const retryErr = await c1
    .request("chat/tool_retry", { sessionId: "e2e-s1", streamId: "x" })
    .then(() => null)
    .catch((e) => e);
  assert(retryErr instanceof Error && /toolId|INVALID|required/i.test(retryErr.message),
    `malformed chat/tool_retry refused with an error (${(retryErr?.message ?? "none").slice(0, 60)})`);
} finally {
  await c1.kill();
}

// ---- 6. restart continuity -------------------------------------------------
{
  console.log("  restart: same sessionId on a fresh coordinator process…");
  const c2 = await spawnCoordinator();
  try {
      const t4 = await runRealTurn(c2, {
      sessionId: "e2e-s1", // same session as before the restart
      streamId: "e2e-st4",
      text: "Reply with exactly the word: hello",
    });
    assert(t4.ack?.accepted === true, "post-restart turn on the same sessionId accepted");
    const t4Done = await t4.done;
    assert(typeof t4Done?.fullText === "string" && t4Done.fullText.length > 0,
      "post-restart turn streams REAL text (session continuity at the wire)");
  } finally {
    await c2.kill();
  }
}

if (failures.length > 0) {
  console.error(`[P50.5.1] FAIL — ${failures.length} assertion(s) failed`);
  process.exit(1);
}
console.log("[P50.5.1] PASS — real vertical chat E2E (real provider, no mocks)");
