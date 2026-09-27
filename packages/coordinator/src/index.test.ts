import { describe, expect, test } from "bun:test";
import { FrameDecoder, encodeJson, encode, MAX_FRAME_LEN } from "./frame";
import { handleRequest, PROTOCOL_VERSION, VERSION } from "./index";
import { isRequest, methodNotFound, ERROR_CODES } from "./message";

describe("frame.ts — length-prefix framing (mirror of agentcowork-ipc/frame.rs)", () => {
  test("encode produces [u32 LE length][payload]", () => {
    const payload = new TextEncoder().encode(JSON.stringify({ jsonrpc: "2.0", method: "echo" }));
    const framed = encode(payload);
    const view = new DataView(framed.buffer);
    expect(view.getUint32(0, true)).toBe(payload.byteLength);
    expect(framed.byteLength).toBe(4 + payload.byteLength);
  });

  test("decoder recovers frames across partial pushes", () => {
    const payloadA = encodeJson({ id: 1, method: "echo" });
    const payloadB = encodeJson({ id: 2, method: "echo" });
    const joined = new Uint8Array(payloadA.byteLength + payloadB.byteLength);
    joined.set(payloadA, 0);
    joined.set(payloadB, payloadA.byteLength);

    const decoder = new FrameDecoder();
    // Feed one byte at a time — every boundary must survive.
    const all: Uint8Array[] = [];
    for (const byte of joined) {
      all.push(...decoder.push(new Uint8Array([byte])));
    }
    expect(all.length).toBe(2);
    expect(JSON.parse(new TextDecoder().decode(all[0]!))).toMatchObject({ id: 1 });
    expect(JSON.parse(new TextDecoder().decode(all[1]!))).toMatchObject({ id: 2 });
  });

  test("oversized frame rejected", () => {
    const decoder = new FrameDecoder();
    const header = new Uint8Array(4);
    new DataView(header.buffer).setUint32(0, MAX_FRAME_LEN + 1, true);
    expect(() => decoder.push(header)).toThrow(/too large/);
  });
});

describe("message.ts — JSON-RPC 2.0 shape", () => {
  test("isRequest accepts camelCase requests", () => {
    expect(isRequest({ jsonrpc: "2.0", method: "echo", id: 1 })).toBe(true);
    expect(isRequest({ jsonrpc: "2.0", method: "echo", id: null })).toBe(true); // null id ok (Rust: Some(Null))
    expect(isRequest({ jsonrpc: "2.0", method: "echo" })).toBe(true); // notification
    expect(isRequest({ jsonrpc: "1.0", method: "echo" })).toBe(false);
    expect(isRequest(null)).toBe(false);
  });

  test("notification detection matches Rust is_notification() (absent id only)", () => {
    // absent id → notification → handleRequest returns null
    expect(handleRequest({ jsonrpc: "2.0", method: "echo", params: { text: "x" } })).toBeNull();
    // explicit null id → request with null id → gets a reply with id null
    const res = handleRequest({ jsonrpc: "2.0", method: "echo", params: { text: "x" }, id: null });
    expect(res).not.toBeNull();
    expect(res!.id).toBeNull();
    expect(res!.result).toEqual({ text: "x", echoed: true });
  });

  test("methodNotFound carries -32601", () => {
    const r = methodNotFound(7, "nope/method");
    expect(r.error?.code).toBe(ERROR_CODES.METHOD_NOT_FOUND);
    expect(r.error?.message).toContain("nope/method");
  });
});

describe("index.ts — request handling", () => {
  test("initialize handshake negotiates protocolVersion + capabilities", () => {
    const res = handleRequest({
      jsonrpc: "2.0",
      method: "initialize",
      params: { protocolVersion: PROTOCOL_VERSION, clientName: "agentcowork-core" },
      id: 1,
    });
    expect(res).not.toBeNull();
    expect(res!.result).toMatchObject({
      protocolVersion: PROTOCOL_VERSION,
      serverName: "@agentcowork/coordinator",
      serverVersion: VERSION,
      capabilities: { streamDeltas: true, passByReference: true },
      status: "ready",
    });
  });

  test("initialize rejects mismatched protocolVersion", () => {
    const res = handleRequest({
      jsonrpc: "2.0",
      method: "initialize",
      params: { protocolVersion: 999 },
      id: 2,
    });
    expect(res!.error?.code).toBe(ERROR_CODES.INVALID_REQUEST);
  });

  test("echo returns payload", () => {
    const res = handleRequest({ jsonrpc: "2.0", method: "echo", params: { text: "hi" }, id: 3 });
    expect(res!.result).toEqual({ text: "hi", echoed: true });
  });

  test("unknown method → -32601", () => {
    const res = handleRequest({ jsonrpc: "2.0", method: "no/such", id: 4 });
    expect(res!.error?.code).toBe(ERROR_CODES.METHOD_NOT_FOUND);
  });

  test("notification (no id) returns null — nothing to send", () => {
    const res = handleRequest({ jsonrpc: "2.0", method: "echo", params: { text: "fire" } });
    expect(res).toBeNull();
  });
});

describe("E2E — real child process over stdin/stdout", () => {
  test("hello-world echo round-trip with length-prefix framing", async () => {
    // Compose: initialize + echo + ping in one write
    // (also exercises frame splitting across a single write).
    const payloads = [
      encodeJson({ jsonrpc: "2.0", method: "initialize", params: { protocolVersion: 1 }, id: 1 }),
      encodeJson({ jsonrpc: "2.0", method: "echo", params: { text: "hello-agentcowork" }, id: 2 }),
      encodeJson({ jsonrpc: "2.0", method: "session/ping", id: 3 }),
    ];
    const joined = new Uint8Array(payloads.reduce((n, p) => n + p.byteLength, 0));
    let offset = 0;
    for (const p of payloads) {
      joined.set(p, offset);
      offset += p.byteLength;
    }

    const proc = Bun.spawn({
      cmd: ["bun", "run", import.meta.dir + "/index.ts"],
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
    });

    // Write the frames, then close stdin so the child exits cleanly.
    proc.stdin!.write(joined);
    proc.stdin!.end();

    const [stdout, stderr] = await Promise.all([
      new Response(proc.stdout as ReadableStream).arrayBuffer(),
      new Response(proc.stderr as ReadableStream).arrayBuffer(),
    ]);
    const exitCode = await proc.exited;

    expect(new TextDecoder().decode(stderr)).toBe("");
    expect(exitCode).toBe(0);

    const decoder = new FrameDecoder();
    const frames = decoder.push(new Uint8Array(stdout));
    // usage/recent (P30 boot hydration request) + session/ready (boot
    // notification) + initialize + echo + ping.
    expect(frames.length).toBe(5);
    const results = frames.map((f) => JSON.parse(new TextDecoder().decode(f)));
    // Frame 0 is the boot hydration request — the sidecar asks the core for
    // durable usage observations before anything else.
    expect(results[0]!.method).toBe("usage/recent");
    // Frame 1 is the boot notification — no id, MUST NOT have a reply-shaped result.
    expect(results[1]!.method).toBe("session/ready");
    expect(results[1]!.id).toBeUndefined();
    expect(results[1]!.params).toMatchObject({ protocolVersion: 1, status: "ready" });
    expect(results[2]!.result).toMatchObject({ protocolVersion: 1, status: "ready" });
    expect(results[3]!.result).toEqual({ text: "hello-agentcowork", echoed: true });
    expect(results[4]!.result).toMatchObject({ pong: true });
  });

  test("sidecar emits session/ready on boot and periodic heartbeats", async () => {
    const proc = Bun.spawn({
      cmd: ["bun", "run", import.meta.dir + "/index.ts"],
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
      // Short heartbeat for the test; the supervisor's idle watchdog is 30s,
      // so production uses the 10s default.
      env: { ...process.env, EVERYAIOS_HEARTBEAT_MS: "200" },
    });

    const decoder = new FrameDecoder();
    const frames: unknown[] = [];
    const reader = (proc.stdout as ReadableStream<Uint8Array>).getReader();
    const readLoop = (async () => {
      try {
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          if (value) {
            for (const f of decoder.push(new Uint8Array(value))) {
              frames.push(JSON.parse(new TextDecoder().decode(f)));
            }
          }
        }
      } catch {
        // child may exit mid-read; ignore
      }
    })();

    // Wait until a heartbeat frame arrives (or 5s). A fixed 700ms sleep
    // flakes under full-suite load when boot itself takes most of the window.
    try {
      const deadline = Date.now() + 5000;
      while (Date.now() < deadline) {
        if (frames.some((f) => (f as { method?: string }).method === "session/heartbeat")) {
          break;
        }
        await new Promise((r) => setTimeout(r, 50));
      }
    } finally {
      try {
        proc.stdin!.end();
      } catch {
        // stdin may already be closed; ignore
      }
      await proc.exited;
      await readLoop;
    }

    const methods = frames.map((f) => (f as { method?: string }).method);
    expect(methods).toContain("session/ready");
    expect(methods).toContain("session/heartbeat");
    const ready = frames.find((f) => (f as { method?: string }).method === "session/ready") as {
      id?: unknown;
      params: { status?: string; protocolVersion?: number };
    };
    expect(ready.id).toBeUndefined(); // notification — no reply expected
    expect(ready.params.status).toBe("ready");
    expect(ready.params.protocolVersion).toBe(PROTOCOL_VERSION);
  });
});
