// P52.x (guard-UX wave) — `guard-event` push contract, TTL math, extend
// payload shape, and block-explanation mapping. All run outside the Tauri
// shell (preview bridge), so they prove the UI contract, not the Rust gate
// (covered by `cargo test -p agentcowork-core --lib guard`).

import { describe, expect, test } from "bun:test";
import {
  guardExplainBlock,
  guardExtendTtl,
  ttlSecondsLeft,
  type GuardTicket,
} from "./guard";

describe("P52.x — guard-event payload contract", () => {
  test("lifecycle event kinds are the four documented shapes", () => {
    const kinds = ["minted", "approved", "rejected", "expired"] as const;
    for (const kind of kinds) {
      const ev = { kind, ticketId: "tkt:1", batch: false };
      expect(ev.ticketId.startsWith("tkt:")).toBe(true);
      expect(typeof ev.batch).toBe("boolean");
    }
  });

  test("pending cards carry the why-asked reason through", () => {
    const card = {
      ticketId: "tkt:7",
      reason: "policy:ask permissions rule for write says ask",
    } as GuardTicket;
    expect(card.reason).toStartWith("policy:ask");
  });
});

describe("P52.x — TTL chip math", () => {
  test("seconds-left clamps at zero and ceils up", () => {
    expect(ttlSecondsLeft(61_000, 0)).toBe(61);
    expect(ttlSecondsLeft(1_500, 0)).toBe(2);
    expect(ttlSecondsLeft(0, 1_000)).toBe(0);
    expect(ttlSecondsLeft(-500, 0)).toBe(0);
  });
});

describe("P52.x — extend + explain preview bridge", () => {
  test("guardExtendTtl resolves the preview shape outside the shell", async () => {
    const ext = await guardExtendTtl("tkt:1", 60_000);
    expect(ext.ticketId).toBe("tkt:1");
    expect(ext.expiresAtMs).toBeGreaterThan(Date.now());
    expect(typeof ext.approvalNonce).toBe("string");
  });

  test("guardExplainBlock resolves a class + hint outside the shell", async () => {
    const exp = await guardExplainBlock("policy denies write");
    expect(typeof exp.class).toBe("string");
    expect(exp.hint.length).toBeGreaterThan(0);
  });
});
