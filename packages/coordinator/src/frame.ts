/**
 * AgentCowork IPC framing — the TS mirror of `agentcowork-ipc/src/frame.rs`.
 *
 * Wire format (identical on both sides of the stdio pipe):
 *
 * ```text
 * [u32 LE length][JSON payload]
 * ```
 *
 * - The length prefix is 4 bytes little-endian, followed by exactly that
 *   many payload bytes.
 * - Payloads larger than [`MAX_FRAME_LEN`] are rejected by the transport;
 *   the app layer must use `ref:` handles instead (spec C10).
 * - The byte layout must never drift from the Rust side — the E2E echo test
 *   proves both sides speak the same protocol.
 */

/** Hard cap on a single frame's payload (16 MiB) — mirrors `MAX_FRAME_LEN` in Rust. */
export const MAX_FRAME_LEN = 16 * 1024 * 1024;

/** Encode a payload into a framed byte buffer: `[u32 LE length][payload]`. */
export function encode(payload: Uint8Array): Uint8Array {
  if (payload.byteLength > MAX_FRAME_LEN) {
    throw new RangeError(`payload exceeds MAX_FRAME_LEN (${payload.byteLength} > ${MAX_FRAME_LEN})`);
  }
  const out = new Uint8Array(4 + payload.byteLength);
  const view = new DataView(out.buffer);
  view.setUint32(0, payload.byteLength, true); // little-endian
  out.set(payload, 4);
  return out;
}

/** Encode a JSON value into a framed byte buffer. */
export function encodeJson(value: unknown): Uint8Array {
  return encode(new TextEncoder().encode(JSON.stringify(value)));
}

/**
 * Emit a JSON-RPC 2.0 notification (no `id` — MUST NOT be replied to) to
 * stdout, length-prefixed. Shared by the coordinator loop (`index.ts`) and the
 * heap monitor (`heap.ts`) so the notification envelope can't drift between
 * the two emitters.
 */
export function notify(method: string, params: Record<string, unknown>): void {
  process.stdout.write(encodeJson({ jsonrpc: "2.0", method, params }));
}

/**
 * Streaming decoder: feed raw bytes as they arrive from the pipe, get back
 * complete payloads. Handles partial frames across multiple `push` calls.
 */
export class FrameDecoder {
  private buffer = new Uint8Array(0);

  /**
   * Append new bytes; returns all complete payloads decoded so far.
   * Throws `FrameError` on an oversized or truncated frame header — the
   * decoder resets its buffer on error so the stream can resync instead of
   * staying wedged on the poisoned bytes.
   */
  push(chunk: Uint8Array): Uint8Array[] {
    const joined = new Uint8Array(this.buffer.byteLength + chunk.byteLength);
    joined.set(this.buffer, 0);
    joined.set(chunk, this.buffer.byteLength);
    this.buffer = joined;

    const frames: Uint8Array[] = [];
    let offset = 0;
    try {
      while (this.buffer.byteLength - offset >= 4) {
        const view = new DataView(this.buffer.buffer, this.buffer.byteOffset + offset);
        const len = view.getUint32(0, true);
        if (len > MAX_FRAME_LEN) {
          throw new FrameError(`frame too large: ${len} bytes (max ${MAX_FRAME_LEN})`);
        }
        if (this.buffer.byteLength - offset - 4 < len) break; // incomplete — wait for more
        frames.push(this.buffer.slice(offset + 4, offset + 4 + len));
        offset += 4 + len;
      }
    } finally {
      // Keep whatever partial frame remains; on error this is an empty
      // slice, so the decoder resyncs from the next chunk.
      this.buffer = this.buffer.slice(offset);
    }
    return frames;
  }

  /** Whether the decoder still holds a partial frame. */
  get hasPartial(): boolean {
    return this.buffer.byteLength > 0;
  }
}

export class FrameError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "FrameError";
  }
}
