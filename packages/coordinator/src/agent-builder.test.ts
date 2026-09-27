// Regression tests for the P31 builder's `bundleToToml`: the emitted
// agent.toml must round-trip through the Rust `AgentBundle::from_toml`
// schema (`agentcowork-agents::bundle`), which is what `agent_registry_save`
// feeds. The critical layout rule: in TOML a table header absorbs every key
// after it, so array fields MUST precede `[engine]`/`[model]`/`[tools]` — a
// `mcp_servers` line after `[model]` silently lands in the model table and
// is dropped by serde. (Found + fixed 2026-08-24; the Rust parse was
// verified independently with a serde probe on the exact bun-emitted bytes.)

import { describe, expect, test } from "bun:test";
import {
  bundleFromTemplate,
  bundleToToml,
  slug,
} from "../../../ui/src/lib/agent-builder";

describe("bundleToToml — Rust AgentBundle schema layout", () => {
  test("array fields precede all table headers", () => {
    const b = bundleFromTemplate("coder", "Coder v2", "👨‍💻");
    b.engine = { kind: "acp", cli: "claude-code" };
    b.model = { provider: "anthropic", model: "claude-sonnet-4" };
    const toml = bundleToToml(b);
    const arrIdx = toml.indexOf("mcp_servers =");
    const engineIdx = toml.indexOf("[engine]");
    const modelIdx = toml.indexOf("[model]");
    const toolsIdx = toml.indexOf("[tools]");
    expect(arrIdx).toBeGreaterThan(-1);
    expect(engineIdx).toBeGreaterThan(-1);
    expect(modelIdx).toBeGreaterThan(-1);
    expect(toolsIdx).toBeGreaterThan(-1);
    // Arrays first; `[engine]`/`[model]`/`[tools]` last, in that order.
    expect(arrIdx).toBeLessThan(engineIdx);
    expect(arrIdx).toBeLessThan(modelIdx);
    expect(engineIdx).toBeLessThan(modelIdx);
    expect(modelIdx).toBeLessThan(toolsIdx);
  });

  test("model-only serializes as a plain string; a draft emits no engine key", () => {
    const mo = bundleFromTemplate("general", "Chatter", "💬");
    mo.engine = { kind: "model-only" };
    expect(bundleToToml(mo)).toContain('engine = "model-only"');
    // ADR-0005: an unbound bundle is a draft — the key is omitted entirely so
    // the Rust schema reads `None`, never a built-in default.
    const draft = bundleFromTemplate("writer", "Writer", "✍️");
    expect(draft.engine).toBeUndefined();
    const toml = bundleToToml(draft);
    expect(toml).not.toContain("engine");
    expect(toml).not.toContain('"inbuilt"');
  });

  test("engine.acp serializes to an [engine] table with the cli", () => {
    const b = bundleFromTemplate("general", "Codex Brain", "🧠");
    b.engine = { kind: "acp", cli: "codex" };
    const toml = bundleToToml(b);
    expect(toml).toContain("[engine]\nacp = \"codex\"");
    expect(toml).not.toContain("[engine]\nacp = \"claude-code\"");
  });

  test("model pin emits [model] only when pinned", () => {
    const b = bundleFromTemplate("general", "Plain", "🤖");
    expect(bundleToToml(b)).not.toContain("[model]");
    b.model = { provider: "groq", model: "llama-3.3-70b" };
    const toml = bundleToToml(b);
    expect(toml).toContain("[model]");
    expect(toml).toContain('provider = "groq"');
    expect(toml).toContain('model = "llama-3.3-70b"');
  });

  test("slug matches the Rust registry slug (ascii-alnum → dash)", () => {
    expect(slug("Budget Analyst")).toBe("budget-analyst");
    // Every non-ascii-alnum char → '-', exactly like the Rust slug():
    // "Café 2.0" → c,a,f,-, ,-,2,-,0 → "caf--2-0".
    expect(slug("Café 2.0")).toBe("caf--2-0");
    expect(slug("!!!")).toBe("agent");
  });
});