/**
 * StreamSession — vendored from `@agentcowork/core-ai` `streaming/stream-session.ts`.
 *
 * Provides TTFT + 33ms batch-flush + token counting during a streaming turn.
 * The coordinator consumes only `ttft`/`batch`/`error` events and
 * `getTokenCount()`; the APP's `done` event, checkpoint persistence, and
 * `metrics-collector` telemetry are dead weight here (the coordinator counts
 * its own tokens and computes its own done timing). This vendored copy keeps
 * the exact batching semantics the engine loop depends on — first-token fires
 * TTFT immediately, deltas accumulate in a buffer flushed on a
 * `setTimeout(batchIntervalMs)` — with **zero** external imports, so the
 * 33ms cadence and token counts are identical to the APP while the sidecar no
 * longer pays for core-ai's metrics/channel graph.
 */

export type StreamEvent =
  | { type: 'ttft'; latencyMs: number }
  | { type: 'batch'; text: string; tokenCount: number }
  | { type: 'error'; error: string };

export type StreamSessionConfig = {
  /** Token batch interval in ms (default 33ms = ~30fps). */
  batchIntervalMs?: number;
};

export class StreamSession {
  private buffer = '';
  private tokenCount = 0;
  private firstTokenTime = 0;
  private startTime = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private destroyed = false;
  private readonly batchIntervalMs: number;
  private readonly onEvent: (event: StreamEvent) => void;

  constructor(
    private readonly turnId: string,
    onEvent: (event: StreamEvent) => void,
    config: StreamSessionConfig = {},
  ) {
    this.onEvent = onEvent;
    this.startTime = Date.now();
    this.batchIntervalMs = config.batchIntervalMs ?? 33;
  }

  pushToken(text: string): void {
    if (this.destroyed) return;

    if (!this.firstTokenTime) {
      this.firstTokenTime = Date.now();
      this.onEvent({ type: 'ttft', latencyMs: this.firstTokenTime - this.startTime });
    }

    this.buffer += text;
    this.tokenCount++;

    if (!this.timer) {
      this.timer = setTimeout(() => this.flushBatch(), this.batchIntervalMs);
    }
  }

  private flushBatch(): void {
    if (this.destroyed) return;
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    if (this.buffer.length > 0) {
      const batch = this.buffer;
      this.buffer = '';
      this.onEvent({ type: 'batch', text: batch, tokenCount: this.tokenCount });
    }
  }

  complete(): void {
    // Snapshot BEFORE flush — flushBatch() clears the buffer, so a caller
    // reading getFullText() afterward would see ''.
    this.flushBatch();
    // The APP also emits a `done` event + records metrics here; the
    // coordinator never consumes either, so both are omitted.
  }

  error(message: string): void {
    this.onEvent({ type: 'error', error: message });
  }

  destroy(): void {
    this.destroyed = true;
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.buffer = '';
  }

  getFullText(): string {
    return this.buffer;
  }

  getTokenCount(): number {
    return this.tokenCount;
  }
}