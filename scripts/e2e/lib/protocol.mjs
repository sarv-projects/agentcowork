#!/usr/bin/env node
/**
 * P50.5 E2E — the agentcowork-ipc wire protocol for driving the REAL
 * coordinator sidecar over stdio, exactly as the Rust relay does:
 *
 *   [u32 LE length][JSON payload]
 *
 * The coordinator is the JSON-RPC *server*: it reads requests on stdin and
 * writes responses + notifications on stdout. It also issues its own
 * requests (outbound to "Rust") on stdout with an `id`; the harness plays
 * the Rust/broker role and MUST reply to those on stdin.
 *
 * Exit-code contract for every gate script:
 *   0 = PASS (real evidence gathered)
 *   1 = FAIL (an assertion failed)
 *   2 = SKIP (environment lacks a real provider / display — never fake-pass)
 */
import { spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import { resolve, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { homedir } from "node:os";
import { existsSync } from "node:fs";

const HERE = dirname(fileURLToPath(import.meta.url));
/** packages/coordinator is the sidecar's home (spawn target + cwd). */
export const COORDINATOR_DIR = resolve(HERE, "../../../packages/coordinator");
export const REPO_ROOT = resolve(HERE, "../../..");

/** Encode one JSON payload as [u32 LE len][bytes]. */
export function encodeFrame(value) {
  const payload = Buffer.from(JSON.stringify(value), "utf8");
  const out = Buffer.alloc(4 + payload.length);
  out.writeUInt32LE(payload.length, 0);
  payload.copy(out, 4);
  return out;
}

/** Streaming frame decoder (mirror of src/frame.ts FrameDecoder). */
export class FrameDecoder {
  constructor() {
    this.buffer = Buffer.alloc(0);
  }
  push(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    const frames = [];
    let offset = 0;
    while (this.buffer.length - offset >= 4) {
      const len = this.buffer.readUInt32LE(offset);
      if (len > 16 * 1024 * 1024) throw new Error(`frame too large: ${len}`);
      if (this.buffer.length - offset - 4 < len) break;
      frames.push(JSON.parse(this.buffer.toString("utf8", offset + 4, offset + 4 + len)));
      offset += 4 + len;
    }
    this.buffer = this.buffer.subarray(offset);
    return frames;
  }
}

/**
 * Drives one coordinator process. `onRequest` handles the coordinator's
 * outbound requests (method + id) and returns the `result` to reply with
 * (or throws → error reply). `onNotification` observes events.
 */
export class CoordinatorClient extends EventEmitter {
  /** @param {string} cmd  e.g. "bun run src/index.ts" (cwd = packages/coordinator) */
  constructor(cmd = "bun run src/index.ts", { env = {}, cwd = COORDINATOR_DIR } = {}) {
    super();
    this.pending = new Map();
    this.nextId = 0;
    this.decoder = new FrameDecoder();
    this.buffer = "";
    this.setMaxListeners(100); // a gate runs many turns on one client
    const bunDir = join(homedir(), ".bun", "bin");
    const cargoDir = join(homedir(), ".cargo", "bin");
    const existingPath = env.PATH ?? process.env.PATH ?? "";
    const fullPath = `${bunDir}:${cargoDir}:${existingPath}`;
    this.child = spawn("bash", ["-lc", cmd], {
      cwd,
      env: { ...process.env, ...env, PATH: fullPath },
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child.stdout.on("data", (d) => {
      let frames;
      try {
        frames = this.decoder.push(d);
      } catch (e) {
        this.emit("protocol-error", e);
        return;
      }
      for (const frame of frames) this.#onFrame(frame);
    });
    this.child.stderr.on("data", (d) => {
      this.buffer += d.toString();
      this.emit("stderr", d.toString());
    });
    this.child.on("exit", (code, signal) => this.emit("exit", code, signal));
  }

  get stderrText() {
    return this.buffer;
  }

  #onFrame(frame) {
    if (frame === null || typeof frame !== "object") return;
    // A response to one of our requests: {id, result|error} with no method.
    if (
      frame.jsonrpc === "2.0" &&
      frame.id !== undefined &&
      frame.method === undefined &&
      ("result" in frame || "error" in frame)
    ) {
      const p = this.pending.get(String(frame.id));
      if (p) {
        this.pending.delete(String(frame.id));
        if (frame.error) p.reject(new Error(frame.error.message ?? "coordinator error"));
        else p.resolve(frame.result);
      }
      return;
    }
    if (frame.method !== undefined && frame.id !== undefined) {
      // The coordinator asks "Rust" something (provider/stream, tool/list,
      // memory/plan, execution/begin, usage/recent...). Reply on stdin.
      const reply = (result) =>
        this.child.stdin.write(encodeFrame({ jsonrpc: "2.0", id: frame.id, result }));
      const replyErr = (message) =>
        this.child.stdin.write(
          encodeFrame({ jsonrpc: "2.0", id: frame.id, error: { code: -32000, message } }),
        );
      this.emit("request", frame.method, frame.params ?? {}, reply, replyErr);
      return;
    }
    // A notification: {method} with no id.
    this.emit("notification", frame.method, frame.params ?? {});
  }

  /** Send a request (we are the client side here) and await the response. */
  request(method, params = {}) {
    const id = `h${++this.nextId}`;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.child.stdin.write(encodeFrame({ jsonrpc: "2.0", method, params, id }));
    });
  }

  /** Send a notification (no id). */
  notify(method, params = {}) {
    this.child.stdin.write(encodeFrame({ jsonrpc: "2.0", method, params }));
  }

  /**
   * Wait for a notification of `method`; returns its params.
   * `match(params)` optionally scopes it (e.g. by streamId) so a signal from
   * an unrelated stream can never satisfy another leg's wait.
   */
  waitForNotification(method, { timeoutMs = 120_000, match = () => true } = {}) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`timeout waiting for notification ${method}`)),
        timeoutMs,
      );
      const on = (m, params) => {
        if (m !== method || !match(params)) return;
        clearTimeout(timer);
        this.off("notification", on);
        resolve(params);
      };
      this.on("notification", on);
    });
  }

  /** Collect notifications of `method` until `until(params)` resolves true. */
  async collectUntil(method, until, { timeoutMs = 120_000 } = {}) {
    const got = [];
    const done = new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`timeout collecting ${method} (got ${got.length})`)),
        timeoutMs,
      );
      const on = (params) => {
        got.push(params);
        if (until(params)) {
          clearTimeout(timer);
          this.off("notification", on);
          resolve(got);
        }
      };
      this.on("notification", (m, params) => {
        if (m === method) on(params);
      });
    });
    return done;
  }

  async kill() {
    if (this.child.exitCode !== null || this.child.signalCode !== null) return;
    this.child.stdin.end();
    const exited = new Promise((r) => this.child.once("exit", r));
    this.child.kill("SIGTERM");
    await Promise.race([exited, new Promise((r) => setTimeout(r, 3000))]);
    if (this.child.exitCode === null && this.child.signalCode === null) this.child.kill("SIGKILL");
  }
}

/** Default replies for the coordinator's best-effort outbound requests. */
export function defaultRustReplies(client, { providerStreamHandler }) {
  client.on("request", (method, params, reply, replyErr) => {
    switch (method) {
      case "usage/recent":
        reply([]);
        break;
      case "tool/list":
        reply({ tools: [] });
        break;
      case "memory/plan":
        reply({ coreFacts: [] });
        break;
      case "memory/cache_get":
        reply({ hit: false });
        break;
      case "memory/cache_put":
        reply({ ok: true });
        break;
      case "execution/begin":
        reply({ ok: true });
        break;
      // B7 scheduler: the sidecar ticks due jobs; empty queues are the
      // correct broker truth when no work exists. A `{}` reply here crashes
      // the sidecar (`list.jobs.map`), so these MUST be shaped.
      case "scheduler/due":
        reply({ due: [] });
        break;
      case "scheduler/list":
        reply({ jobs: [] });
        break;
      case "scheduler/lease_start":
        reply({ ok: false, resumed: false, checkpoint: 0 });
        break;
      case "scheduler/lease_finish":
      case "scheduler/lease_heartbeat":
      case "scheduler/monitor":
        reply({ ok: true, changed: false, notified: false, stopped: false, current: "", notifications: 0 });
        break;
      case "scheduler/fire_webhook":
        reply({ fired: [] });
        break;
      case "provider/stream":
        // ACK first (the engine then waits on chat/provider_chunk frames),
        // then run the REAL provider call in the background.
        reply({ accepted: true });
        if (providerStreamHandler) {
          void providerStreamHandler(params).catch((e) => {
            client.notify("chat/provider_chunk", {
              streamId: params.streamId,
              error: e instanceof Error ? e.message : String(e),
            });
            client.notify("chat/provider_chunk", { streamId: params.streamId, ended: true });
          });
        }
        break;
      default:
        reply({});
    }
  });
}
