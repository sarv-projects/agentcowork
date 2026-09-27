// P50.3.2 — task response contract (wire-shape test).
//
// `tasks_list` / `tasks_show` return the Rust `TaskRecord` serde shape
// (crates/agentcowork-core/src/task_ledger.rs). The Rust side carries a mirror
// test (`serialized_record_matches_ts_bridge_contract`) asserting the exact
// key set + enum spellings this validator accepts. If either side changes,
// one of the two tests fails — the `tasks/*` responses can no longer desync
// from the activity rail through a silent cast.

import { describe, expect, test } from "bun:test";
import type { TaskRecord } from "./tasks";

/** Runtime validator for one wire record (mirror of the Rust serde shape). */
function assertTaskRecord(raw: unknown): TaskRecord {
  if (typeof raw !== "object" || raw === null) throw new Error("record is not an object");
  const r = raw as Record<string, unknown>;
  const req = (k: string): unknown => {
    if (!(k in r)) throw new Error(`missing field: ${k}`);
    return r[k];
  };
  if (typeof req("id") !== "string") throw new Error("id: string");
  if (!["automation", "subagent", "acp", "cli", "scheduled"].includes(String(r["kind"])))
    throw new Error(`bad kind: ${String(r["kind"])}`);
  if (typeof req("title") !== "string") throw new Error("title: string");
  if (
    !["queued", "running", "succeeded", "failed", "timed_out", "cancelled", "lost"].includes(
      String(r["status"]),
    )
  )
    throw new Error(`bad status: ${String(r["status"])}`);
  if (typeof r["created_ms"] !== "number") throw new Error("created_ms: number");
  for (const k of ["started_ms", "finished_ms", "last_heartbeat_ms"] as const) {
    const v = r[k];
    if (v !== null && typeof v !== "number") throw new Error(`${k}: null | number`);
  }
  const err = r["error"];
  if (err !== null && typeof err !== "string") throw new Error("error: null | string");
  if (typeof req("retry_generation") !== "number") throw new Error("retry_generation: number");
  const d = req("delivery");
  // Unit variants are plain strings; only `Blocked` is an object variant
  // (externally-tagged enum — the Rust contract test pins this exact shape).
  if (d === "pending" || d === "delivered" || d === "dismissed") {
    // ok
  } else if (typeof d === "object" && d !== null) {
    const keys = Object.keys(d);
    if (keys.length !== 1 || keys[0] !== "blocked") throw new Error("delivery: unknown variant");
    const b = (d as Record<string, unknown>)["blocked"] as Record<string, unknown>;
    if (typeof b?.["retries"] !== "number" || typeof b?.["deadline_ms"] !== "number")
      throw new Error("delivery.blocked: { retries, deadline_ms }");
  } else {
    throw new Error("delivery: 'pending' | 'delivered' | 'dismissed' | { blocked: … }");
  }
  return r as unknown as TaskRecord;
}

/** Fixture captured from the Rust ledger serde (`tasks/list` response item)
 * — including the `Option` fields serialized as explicit `null`s. */
const RUST_LEDGER_RECORD = {
  id: "task-000042",
  kind: "scheduled",
  title: "Monday competitor digest",
  status: "running",
  requester: "s-digest",
  created_ms: 1_756_700_000_000,
  started_ms: 1_756_700_060_000,
  finished_ms: null,
  last_heartbeat_ms: 1_756_700_120_000,
  error: null,
  retry_generation: 0,
  delivery: "pending",
};

describe("P50.3.2 — tasks/* wire contract", () => {
  test("a real Rust ledger record validates against the TS mirror", () => {
    const rec = assertTaskRecord(RUST_LEDGER_RECORD);
    expect(rec.id).toBe("task-000042");
    expect(rec.delivery).toBe("pending");
  });

  test("blocked delivery carries the fenced retry shape", () => {
    const rec = assertTaskRecord({
      ...RUST_LEDGER_RECORD,
      status: "succeeded",
      finished_ms: 1_756_700_900_000,
      delivery: { blocked: { retries: 2, deadline_ms: 1_756_701_000_000 } },
    });
    expect(rec.delivery).toEqual({
      blocked: { retries: 2, deadline_ms: 1_756_701_000_000 },
    });
  });

  test("a missing or renamed field is a hard error, never a silent cast", () => {
    const missing = { ...RUST_LEDGER_RECORD } as Record<string, unknown>;
    delete missing["retry_generation"];
    expect(() => assertTaskRecord(missing)).toThrow(/retry_generation/);
    expect(() => assertTaskRecord({ ...RUST_LEDGER_RECORD, kind: "batch_job" })).toThrow(
      /bad kind/,
    );
    expect(() => assertTaskRecord({ ...RUST_LEDGER_RECORD, delivery: {} })).toThrow(
      /unknown variant/,
    );
  });

  test("snake_case casing is preserved end-to-end (no camelCase rewrite)", () => {
    const camel = { ...RUST_LEDGER_RECORD };
    delete (camel as Record<string, unknown>)["created_ms"];
    expect(() => assertTaskRecord(camel)).toThrow(/created_ms/);
  });
});
