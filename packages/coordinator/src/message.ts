/**
 * AgentCowork IPC JSON-RPC message types — the TS mirror of
 * `agentcowork-ipc/src/message.rs`. Wire shape is camelCase and must match
 * the Rust `serde(rename_all = "camelCase")` structs byte-for-byte.
 */

/** JSON-RPC 2.0 marker — every message declares this. */
export const JSONRPC = "2.0";

/** Standard JSON-RPC error codes (mirror `JsonRpcError` constants in Rust). */
export const ERROR_CODES = {
  PARSE_ERROR: -32700,
  INVALID_REQUEST: -32600,
  METHOD_NOT_FOUND: -32601,
  INTERNAL_ERROR: -32603,
} as const;

/** A JSON-RPC request or notification (no `id` = notification). */
export interface Request {
  jsonrpc: string;
  /** Method name (e.g. `chat/stream`, `browser/act`, `vault/rotate`). */
  method: string;
  /** Positional or named params. Omitted when absent. */
  params?: unknown;
  /** Present = a request awaiting a response; absent = notification.
   *  Explicit `null` is allowed (JSON-RPC 2.0 §2.2; Rust: `Some(Null)`). */
  id?: string | number | null;
}

/** A JSON-RPC error object (code/message/data). */
export interface JsonRpcError {
  code: number;
  message: string;
  data?: unknown;
}

/** A JSON-RPC response (either `result` or `error`, never both). */
export interface Response {
  jsonrpc: string;
  id: string | number | null;
  result?: unknown;
  error?: JsonRpcError;
}

/**
 * Guards: is this value a well-formed request object?
 *
 * `id` may be a string, number, or explicit `null` (JSON-RPC 2.0 §2.2 allows
 * null; mirrors Rust `Option<Value>` where `Some(Null)` is a request, not a
 * notification). Only an *absent* `id` means notification — matching Rust's
 * `is_notification()` (`id.is_none()`).
 */
export function isRequest(value: unknown): value is Request {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    v.jsonrpc === JSONRPC &&
    typeof v.method === "string" &&
    (v.id === undefined || v.id === null || typeof v.id === "string" || typeof v.id === "number")
  );
}

/** Build a success response. */
export function ok(id: string | number | null, result: unknown): Response {
  return { jsonrpc: JSONRPC, id, result };
}

/** Build an error response. */
export function err(id: string | number | null, code: number, message: string): Response {
  return { jsonrpc: JSONRPC, id, error: { code, message } };
}

/** Convenience: METHOD_NOT_FOUND response. */
export function methodNotFound(id: string | number | null, method: string): Response {
  return err(id, ERROR_CODES.METHOD_NOT_FOUND, `method not found: ${method}`);
}
