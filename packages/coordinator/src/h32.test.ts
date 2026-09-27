import { describe, expect, it } from "bun:test";
import { isAcpAgent, modelColumnState, sanitizeRequest, shouldForwardModel } from "./h32";

describe("H32 send policy (v1: nothing is forwarded)", () => {
  it("no send carries a per-agent model — there is no built-in engine", () => {
    for (const agent of ["claude-code", "codex", "opencode", "acp:custom-agent"]) {
      expect(isAcpAgent(agent)).toBe(true);
      expect(shouldForwardModel(agent)).toBe(false);
      const req = sanitizeRequest({ model: "gpt-5", messages: [] }, agent);
      expect(req).not.toHaveProperty("model");
    }
    // The retired built-in spellings are not agents, and they forward nothing
    // either — a stale config value must not resurrect the old branch.
    // DEC-053: these legacy spellings stay verbatim so retirement recognition keeps working.
    for (const retired of ["everyaios-native", "everyaios", "inbuilt", ""]) {
      expect(isAcpAgent(retired)).toBe(false);
      expect(shouldForwardModel(retired)).toBe(false);
      expect(sanitizeRequest({ model: "gpt-5" }, retired)).not.toHaveProperty("model");
    }
  });

  it("the model column is visible but never selectable", () => {
    const row = modelColumnState("claude-code");
    expect(row).toMatchObject({ visible: true, selectable: false });
    expect(row.hint).toContain("claude-code");
    expect(modelColumnState("everyaios-native")).toMatchObject({
      visible: true,
      selectable: false,
    });
  });
});
