/**
 * P39.1 — per-message-type IPC payload budgets (doc-42 §1.4, spec §9.3 §1).
 *
 * The TS mirror of `agentcowork-ipc/src/budget.rs`. The transport hard cap is
 * `MAX_FRAME_LEN` (16 MiB, frame.ts); this module adds the per-message-type
 * budgets the doc-42 table calls for, so a 60 KB tool result arrives as a
 * ≤50 KB payload + ref — never a 60 KB frame.
 *
 * The table must never drift from the Rust side: same kinds, same limits.
 * `budgetFor` is the table; `RefRegistry` is the C10 pass-by-reference seam
 * the coordinator uses when it emits an oversized payload (park the full
 * payload, send `{ ref, preview }`); `budgetJson` is the emit-point helper.
 */

/** The message kinds the doc-42 §1.4 table budgets (mirrors Rust `MessageKind`). */
export enum MessageKind {
  ToolResult = "tool_result",
  ScrapedPage = "scraped_page",
  A11ySnapshot = "a11y_snapshot",
  AuditExport = "audit_export",
  Default = "default",
}

export interface PayloadBudget {
  /** Max inline payload bytes for this kind. */
  inlineLimit: number;
  /** Max inline preview bytes when the full payload is deferred to a ref. */
  previewLimit: number;
}

/** The doc-42 §1.4 table — must match `budget_for` in agentcowork-ipc. */
export function budgetFor(kind: MessageKind): PayloadBudget {
  switch (kind) {
    case MessageKind.ToolResult:
      return { inlineLimit: 50 * 1024, previewLimit: 2 * 1024 };
    case MessageKind.ScrapedPage:
      return { inlineLimit: 2 * 1024, previewLimit: 2 * 1024 };
    case MessageKind.A11ySnapshot:
      return { inlineLimit: 8 * 1024, previewLimit: 1024 };
    case MessageKind.AuditExport:
      return { inlineLimit: 64 * 1024, previewLimit: 4 * 1024 };
    default:
      return { inlineLimit: 256 * 1024, previewLimit: 4 * 1024 };
  }
}

/** The result of applying a budget to a payload. */
export interface Budgeted {
  /** The inline bytes the peer receives (≤ the type's inlineLimit). */
  inline: Uint8Array;
  /** Whether the payload was truncated into a ref. */
  truncated: boolean;
  /** The full payload, present only when truncated — park it in a RefRegistry. */
  full?: Uint8Array;
}

const MARKER = (limit: number) =>
  `\n[truncated by payload budget — full payload behind ref handle; inline preview capped at ${limit} bytes]`;

/**
 * Enforce a message kind's budget on raw bytes. Small payloads stay inline;
 * oversized payloads return a bounded inline extract plus the full payload
 * for the caller to park in a [`RefRegistry`].
 */
export function applyBudget(kind: MessageKind, payload: Uint8Array): Budgeted {
  const budget = budgetFor(kind);
  if (payload.byteLength <= budget.inlineLimit) {
    return { inline: payload, truncated: false };
  }
  const previewLen = Math.min(budget.previewLimit, payload.byteLength);
  const marker = new TextEncoder().encode(MARKER(budget.previewLimit));
  const inline = new Uint8Array(previewLen + marker.byteLength);
  inline.set(payload.slice(0, previewLen), 0);
  inline.set(marker, previewLen);
  return { inline, truncated: true, full: payload };
}

/**
 * The coordinator-side ref registry (C10 pass-by-reference seam): one-shot
 * storage of full payloads behind `ref:handle:<n>` wire forms (mirrors the
 * Rust side's stateless content-addressed `HandleStore` in agentcowork-memory).
 *
 * ## Lifecycle (W5 — "refs split-brain" fault line)
 *
 * Handles are **leased, not permanent** — a handle the peer never resolves
 * (turn aborted, error path, compaction) cannot leak forever:
 * - each handle expires after `ttlMs` (default 10 min), checked lazily on
 *   `put`/`take`/`size`;
 * - the registry is capacity-bound (oldest inserted evicted past
 *   `MAX_REF_HANDLES`), so memory stays bounded under adversarial emission;
 * - `take` is still one-shot (burn on resolve, mirrors Rust `HandleStore`).
 *
 * Abort-time revocation is not per-turn: refs are resolved synchronously
 * within the turn that emitted them, and a whole-registry `clear()` would
 * invalidate a concurrent stream's in-flight refs — the TTL + capacity bound
 * closes the leak without cross-stream interference.
 */
export const DEFAULT_REF_TTL_MS = 10 * 60_000;
export const MAX_REF_HANDLES = 64;

interface RefEntry {
  bytes: Uint8Array;
  expiresAt: number;
}

export class RefRegistry {
  private store = new Map<string, RefEntry>();
  private next = 1;
  private readonly now: () => number;

  constructor(now: () => number = Date.now) {
    this.now = now;
  }

  /** Store a payload (leased); returns its `ref:handle:<n>` wire form. */
  put(bytes: Uint8Array, ttlMs: number = DEFAULT_REF_TTL_MS): string {
    this.sweep();
    const id = `handle:${this.next++}`;
    this.store.set(id, { bytes, expiresAt: this.now() + ttlMs });
    // Bounded capacity: evict the oldest-inserted handle (Map preserves
    // insertion order) when the cap is crossed.
    while (this.store.size > MAX_REF_HANDLES) {
      const oldest = this.store.keys().next().value as string;
      this.store.delete(oldest);
    }
    return `ref:${id}`;
  }

  /** One-shot fetch by wire form; `undefined` for unknown/expired/taken. */
  take(wire: string): Uint8Array | undefined {
    const id = wire.replace(/^ref:/, "");
    const entry = this.store.get(id);
    if (entry === undefined) return undefined;
    if (entry.expiresAt <= this.now()) {
      this.store.delete(id);
      return undefined;
    }
    this.store.delete(id); // burn on first use
    return entry.bytes;
  }

  /** Drop expired handles; returns how many were removed. */
  sweep(): number {
    const now = this.now();
    let removed = 0;
    for (const [id, e] of this.store) {
      if (e.expiresAt <= now) {
        this.store.delete(id);
        removed += 1;
      }
    }
    return removed;
  }

  /** Revoke everything — process teardown / maintenance only. */
  clear(): void {
    this.store.clear();
  }

  get size(): number {
    this.sweep();
    return this.store.size;
  }
}

/** Shared registry for the coordinator's emit points (chat.ts, plan.ts). */
export const refRegistry = new RefRegistry();

/**
 * Emit-point helper: budget a JSON result before it leaves the coordinator.
 * Oversized results become `{ ref, preview, truncated: true }` with the full
 * payload parked in `registry` (the peer resolves the ref on demand); small
 * results pass through unchanged. `kind` defaults to ToolResult — the common
 * emit case.
 */
export function budgetJson(
  value: unknown,
  registry: RefRegistry,
  kind: MessageKind = MessageKind.ToolResult,
): unknown {
  const budget = budgetFor(kind);
  const json = JSON.stringify(value);
  const bytes = new TextEncoder().encode(json);
  if (bytes.byteLength <= budget.inlineLimit) {
    return value;
  }
  const b = applyBudget(kind, bytes);
  const ref = b.full !== undefined ? registry.put(b.full) : "";
  return {
    ref,
    preview: new TextDecoder().decode(b.inline),
    truncated: true,
    byteLength: bytes.byteLength,
  };
}
