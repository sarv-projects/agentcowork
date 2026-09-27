import { describe, expect, test, beforeEach } from "bun:test";
import {
  buildResumePrompt,
  checkSpawn,
  deriveChildPermissions,
  governanceBadge,
  injectPrimaryContext,
  isRetiredAgentId,
  primaryAgentRegistry,
  resolvePrimaryAgentId,
  resolveSessionPrimaryAgent,
  validateSessionPin,
  type PrimaryAgentRecord,
  type SpawnState,
} from "./primary-agent";

describe("resolveSessionPrimaryAgent — per-session pin precedence", () => {
  beforeEach(() => {
    // Module-singleton registry: clear any pin a prior test may have left on
    // the session ids this suite touches.
    primaryAgentRegistry.clearSessionPin("s-pin");
    primaryAgentRegistry.clearSessionPin("s2");
  });

  test("session pin outranks the user default", () => {
    primaryAgentRegistry.setSessionPin("s-pin", "codex");
    const pin = primaryAgentRegistry.sessionPin("s-pin");
    expect(pin).toBe("codex");
    expect(
      resolveSessionPrimaryAgent({
        ...(pin !== undefined ? { sessionPin: pin } : {}),
        userDefault: "claude-code",
      }),
    ).toBe("codex");
  });

  test("no pin → user default → none (P71.5b: there is no inbuilt fallback)", () => {
    expect(resolveSessionPrimaryAgent({ userDefault: "claude-code" })).toBe("claude-code");
    expect(resolveSessionPrimaryAgent({})).toBe(null);
  });

  test("installed-any: any non-empty external pin pins (no enum gate)", () => {
    const pin = primaryAgentRegistry.setSessionPin("s2", "some-new-agent");
    expect(pin).toBe("some-new-agent");
    primaryAgentRegistry.clearSessionPin("s2");
  });

  test("empty pin refuses fail-closed", () => {
    expect(() => primaryAgentRegistry.setSessionPin("s2", "")).toThrow();
  });

  test("retired built-in spellings are refused by name (ADR-0005)", () => {
    // DEC-053: these legacy spellings stay verbatim so retirement recognition keeps working.
    for (const retired of ["inbuilt", "everyaios", "everyaios-native"]) {
      expect(isRetiredAgentId(retired)).toBe(true);
      expect(() => primaryAgentRegistry.setSessionPin("s2", retired)).toThrow(/retired built-in/);
      expect(resolvePrimaryAgentId(retired, "claude-code")).toBe("claude-code");
    }
    expect(resolvePrimaryAgentId("inbuilt", undefined)).toBe(null);
    primaryAgentRegistry.clearSessionPin("s2");
  });

  test("resolvePrimaryAgentId skips empty and retired values at every precedence level", () => {
    expect(resolvePrimaryAgentId("", "codex")).toBe("codex");
    expect(resolvePrimaryAgentId("inbuilt", "codex")).toBe("codex");
    expect(resolvePrimaryAgentId(undefined, "inbuilt")).toBe(null);
  });
});

describe("validateSessionPin", () => {
  test("empty throws", () => {
    expect(() => validateSessionPin("")).toThrow(/fail-closed/);
  });
  test("retired throws with the agent-naming sentence", () => {
    expect(() => validateSessionPin("everyaios")).toThrow(/ADR-0005/);
  });
  test("a real agent id passes through", () => {
    expect(validateSessionPin("claude-code")).toBe("claude-code");
  });
});

describe("injectPrimaryContext — one path, same governance injection", () => {
  test("passport + taste + governance are appended", () => {
    const out = injectPrimaryContext(
      { passport: "P", taste: "T", governance: { kind: "mediated", fs: true, terminal: true } },
      "BASE",
    );
    expect(out).toContain("BASE");
    expect(out).toContain("## Memory passport (C10)\nP");
    expect(out).toContain("## Taste profile (C9)\nT");
    expect(out).toContain("Governed-Mediated");
  });
  test("empty passport/taste add nothing", () => {
    const out = injectPrimaryContext(
      { passport: "", taste: "", governance: { kind: "not_governed" } },
      "BASE",
    );
    // The name is the contract: an empty passport/taste contributes no section,
    // so the governance line is the only thing appended.
    expect(out).not.toContain("## Memory passport");
    expect(out).not.toContain("## Taste profile");
    expect(out).toBe("BASE\n\n## Governance\nThis session runs under NotGoverned.");
  });
  test("governance badge vocabulary", () => {
    expect(governanceBadge({ kind: "self_contained", channelB: true })).toBe("Self-contained");
  });
});

describe("B3 delegation limits — depth ≤2, concurrency ≤6, strict budgets", () => {
  const base: SpawnState = {
    depth: 0,
    active: 0,
    stepsUsed: 0,
    parentPermissions: new Set(["fs.read", "fs.write"]),
    denies: new Set(["fs.write"]),
    grants: new Set(["net.fetch"]),
  };

  test("depth gate", () => {
    expect(checkSpawn({ ...base, depth: 1 }).allowed).toBe(true);
    expect(checkSpawn({ ...base, depth: 2 }).allowed).toBe(false);
  });
  test("concurrency gate", () => {
    expect(checkSpawn({ ...base, active: 5 }).allowed).toBe(true);
    expect(checkSpawn({ ...base, active: 6 }).allowed).toBe(false);
  });
  test("chain budget gate", () => {
    expect(checkSpawn({ ...base, stepsUsed: 999 }).allowed).toBe(true);
    expect(checkSpawn({ ...base, stepsUsed: 1000 }).allowed).toBe(false);
  });
  test("derived child permissions: parent ∩ deny-removed ∪ grants", () => {
    const child = deriveChildPermissions(base);
    expect(child.has("fs.read")).toBe(true);
    expect(child.has("fs.write")).toBe(false);
    expect(child.has("net.fetch")).toBe(true);
  });
});

describe("Work survives agent death", () => {
  const record: PrimaryAgentRecord = {
    sessionId: "s-resume",
    agentId: "claude-code",
    governance: { kind: "not_governed" },
    lastCompletedTurn: 7,
    configHash: "abcdef1234567890",
  };

  test("the resume prompt continues the same Work and never replays effects", () => {
    const out = buildResumePrompt(record, "ship the feature", "- [x] step one\n- [ ] step two");
    expect(out).toContain("s-resume");
    expect(out).toContain("abcdef12");
    expect(out).toContain("previous primary agent completed 7 turns");
    expect(out).toContain("do not replay completed effects");
    expect(out).toContain("step two");
  });

  test("swap keeps the record chain and changes only the agent", () => {
    primaryAgentRegistry.record(record);
    const next = primaryAgentRegistry.swap("s-resume", "codex", { kind: "self_contained", channelB: true });
    expect(next?.agentId).toBe("codex");
    expect(next?.lastCompletedTurn).toBe(7);
    expect(next?.configHash).toBe("abcdef1234567890");
  });
});
