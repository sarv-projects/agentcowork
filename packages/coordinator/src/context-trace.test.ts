import { describe, expect, test } from "bun:test";
import {
  ContextInspector,
  ContextTrace,
  assertAllLogged,
  newTrace,
  sha256Hex,
  usableWindow,
  verifyEntry,
} from "./context-trace";

describe("model-visible-means-logged", () => {
  test("records blocks with content hashes", () => {
    const trace = new ContextTrace();
    trace.record("user", "<user>hi</user>");
    trace.record("system", "You are AgentCowork.");
    expect(trace.count()).toBe(2);
    const e = trace.entriesFor("user")[0]!;
    expect(e.hash).toBe(sha256Hex("<user>hi</user>"));
    expect(e.tokens).toBeGreaterThan(0);
  });

  test("verifyEntry proves a block is present in the sent prompt", () => {
    const trace = new ContextTrace();
    trace.record("memory_warm_set", "<memory_warm_set>a</memory_warm_set>");
    const prompt = "sys\n\n<memory_warm_set>a</memory_warm_set>\n<user>x</user>";
    expect(verifyEntry(trace, "memory_warm_set", "<memory_warm_set>a</memory_warm_set>", prompt)).toBe(true);
  });

  test("assertAllLogged flags a block dropped at send time", () => {
    const trace = new ContextTrace();
    trace.record("tool_index", "<tool_index>a</tool_index>");
    trace.record("user", "<user>q</user>");
    const prompt = "<user>q</user>"; // tool_index was dropped before sending
    const result = assertAllLogged(
      trace,
      [
        { source: "tool_index", content: "<tool_index>a</tool_index>" },
        { source: "user", content: "<user>q</user>" },
      ],
      prompt,
    );
    expect(result.ok).toBe(false);
    expect(result.missing).toContain("tool_index");
  });

  test("assertAllLogged passes when everything is present", () => {
    const trace = new ContextTrace();
    trace.record("system", "S");
    trace.record("user", "U");
    const prompt = "S\nU";
    const result = assertAllLogged(
      trace,
      [
        { source: "system", content: "S" },
        { source: "user", content: "U" },
      ],
      prompt,
    );
    expect(result.ok).toBe(true);
    expect(result.missing).toEqual([]);
  });
});

describe("usable window arithmetic", () => {
  test("subtracts every named reserve and buffer", () => {
    expect(
      usableWindow({
        modelWindowResolved: 200_000,
        outputReserve: 8_000,
        reasoningReserve: 4_000,
        summaryOutputReserve: 2_000,
        toolSchemaReserve: 4_000,
        systemReserve: 2_000,
        safetyBuffer: 20_000,
        keep: 8_000,
        reservedTotal: 40_000,
        usable: 160_000,
      }),
    ).toBe(160_000);
  });

  test("saturates at zero rather than reporting a negative window", () => {
    expect(
      usableWindow({
        modelWindowResolved: 1_000,
        outputReserve: 4_000,
        reasoningReserve: 4_000,
        summaryOutputReserve: 4_000,
        toolSchemaReserve: 4_000,
        systemReserve: 4_000,
        safetyBuffer: 4_000,
        keep: 0,
        reservedTotal: 0,
        usable: 0,
      }),
    ).toBe(0);
  });
});

describe("the context inspector projection", () => {
  const budget = {
    modelWindowResolved: 100_000,
    outputReserve: 8_000,
    reasoningReserve: 4_000,
    summaryOutputReserve: 2_000,
    toolSchemaReserve: 4_000,
    systemReserve: 2_000,
    safetyBuffer: 20_000,
    keep: 8_000,
    reservedTotal: 40_000,
    usable: 60_000,
  };

  test("a zero-hit turn reports zero relevant-block tokens and no always-on block", () => {
    const inspector = new ContextInspector();
    const trace = inspector.record(
      newTrace(1, {
        budget,
        recallOutcome: "abstain",
        relevantTokens: 0,
        alwaysOnTokens: 0,
      }),
    );
    expect(trace.relevantTokens).toBe(0);
    expect(trace.alwaysOnTokens).toBe(0);
    const s = inspector.summary(trace);
    expect(s.relevantTokens).toBe(0);
    expect(s.alwaysOnTokens).toBe(0);
    expect(s.perSource).toEqual({});
  });

  test("the always-on block is reported separately from the relevant block", () => {
    const inspector = new ContextInspector();
    const trace = inspector.record(
      newTrace(2, {
        budget,
        recallOutcome: "hit",
        alwaysOnTokens: 40,
        relevantTokens: 120,
        selected: [
          {
            id: "m1",
            source: "memory_always_on",
            trustTier: "user_explicit",
            sensitivity: "personal",
            tokens: 40,
            contentRef: "memory:m1",
            reconstructable: true,
            pinned: true,
          },
          {
            id: "m2",
            source: "memory_relevant",
            trustTier: "derived_untrusted",
            sensitivity: "personal",
            tokens: 120,
            contentRef: "memory:m2",
            reconstructable: true,
            pinned: false,
          },
        ],
      }),
    );
    const s = inspector.summary(trace);
    expect(s.relevantTokens).toBe(120);
    expect(s.alwaysOnTokens).toBe(40);
    expect(s.perSource).toEqual({ memory_always_on: 1, memory_relevant: 1 });
    expect(s.perClass).toEqual({ user_explicit: 1, derived_untrusted: 1 });
    expect(s.pinnedCount).toBe(0);
  });

  test("every named budget term reaches the trace verbatim", () => {
    const inspector = new ContextInspector();
    const trace = inspector.record(newTrace(3, { budget }));
    for (const term of [
      "modelWindowResolved",
      "outputReserve",
      "reasoningReserve",
      "summaryOutputReserve",
      "toolSchemaReserve",
      "systemReserve",
      "safetyBuffer",
      "keep",
      "usable",
    ]) {
      expect(trace.budget).toHaveProperty(term);
    }
    const s = inspector.summary(trace);
    expect(s.window).toBe(100_000);
    expect(s.usable).toBe(60_000);
  });

  test("a refusal is reported with guidance, never as a provider error", () => {
    const inspector = new ContextInspector();
    const trace = inspector.record(
      newTrace(4, {
        budget,
        feasibility: {
          kind: "refused",
          reason: "minimum_irreducible",
          shortfall: 5_000,
          guidance: ["Reduce the tool set or switch to a larger-window model."],
        },
      }),
    );
    expect(trace.feasibility.kind).toBe("refused");
    const s = inspector.summary(trace);
    expect(s.feasibility).toBe("refused");
    if (trace.feasibility.kind === "refused") {
      expect(trace.feasibility.guidance.length).toBeGreaterThan(0);
      expect(trace.feasibility.shortfall).toBe(5_000);
    }
  });

  test("a recoverable oversize is reported as recovery, not as an error", () => {
    const inspector = new ContextInspector();
    const trace = inspector.record(
      newTrace(5, { budget, feasibility: { kind: "needs_recovery", overBy: 1_200 } }),
    );
    expect(trace.feasibility).toEqual({ kind: "needs_recovery", overBy: 1_200 });
    expect(inspector.summary(trace).feasibility).toBe("needs_recovery");
  });

  test("pruned items are reported with a ref to their full durable bytes", () => {
    const inspector = new ContextInspector();
    const trace = inspector.record(
      newTrace(6, {
        budget,
        pruned: [
          {
            seq: 3,
            preview: "cargo test … [full output at artifact ref]",
            durableRef: "ctxout:3",
            reconstructable: true,
          },
        ],
        compactedThroughSeq: 7,
        checkpointId: "ckpt:7",
      }),
    );
    expect(inspector.summary(trace).prunedCount).toBe(1);
    expect(trace.pruned[0]!.durableRef).toBe("ctxout:3");
    expect(trace.pruned[0]!.reconstructable).toBe(true);
    expect(inspector.summary(trace).checkpointId).toBe("ckpt:7");
  });

  test("deferred gates are reported honestly rather than hidden", () => {
    const inspector = new ContextInspector();
    const trace = inspector.record(
      newTrace(7, { budget, deferred: ["budget_exhausted", "kill_switch"] }),
    );
    expect(trace.deferred).toEqual(["budget_exhausted", "kill_switch"]);
  });

  test("abstention and error stay distinguishable in the trace", () => {
    const inspector = new ContextInspector();
    expect(inspector.record(newTrace(8, { recallOutcome: "abstain" })).recallOutcome).toBe("abstain");
    expect(inspector.record(newTrace(9, { recallOutcome: "error" })).recallOutcome).toBe("error");
    expect(inspector.record(newTrace(10, { recallOutcome: "hit" })).recallOutcome).toBe("hit");
  });

  test("a zero-token selected item is counted as dropped for the budget", () => {
    const inspector = new ContextInspector();
    const trace = inspector.record(
      newTrace(11, {
        budget,
        selected: [
          {
            id: "m1",
            source: "memory_relevant",
            trustTier: "user_explicit",
            sensitivity: "personal",
            tokens: 0,
            contentRef: "memory:m1",
            reconstructable: true,
            pinned: false,
          },
        ],
      }),
    );
    // Whole-item drop, never a truncation: a dropped item has zero tokens.
    expect(inspector.summary(trace).droppedForBudget).toBe(1);
  });

  test("the inspector is a projection: it never selects or mutates", () => {
    const inspector = new ContextInspector();
    const trace = newTrace(12, { budget, recallOutcome: "hit" });
    // A fresh trace with defaults carries no selection, so recording one cannot
    // have influenced anything: the inspector holds only what it was given.
    expect(trace.selected).toEqual([]);
    expect(trace.pinned).toEqual([]);
    expect(trace.excluded).toEqual([]);
    inspector.record(trace);
    expect(inspector.count()).toBe(1);
    expect(inspector.last()).toBe(trace);
    inspector.reset();
    expect(inspector.count()).toBe(0);
  });
});
