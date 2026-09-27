/**
 * P1.2 — smoke-import of `@agentcowork/core-providers` into the coordinator
 * sidecar. Proves the workspace dep resolves, its **observability** surface is
 * intact, and its synchronous catalog functions behave — without any network
 * call.
 *
 * P71.2d (ADR-0005): the inference clients (`streamCompletion`,
 * `streamAnthropicCompletion`, the key probes) are **deleted** — v1 has no
 * AgentCowork-owned model call, because the bound external agent owns its own
 * model. The catalogue, pricing, capability and credential-façade surface is
 * what remains, and the test now asserts the removal too.
 *
 * The dep is a pnpm workspace link to the vendored `packages/core-providers`
 * (copied in from the reference APP repo — this repo is self-contained).
 */
import { describe, expect, test } from "bun:test";
import {
  AI_PROVIDER_GROUPS,
  PROVIDER_CATALOG,
  ProviderVault,
  fetchProviderPricing,
  filterProvidersBySection,
  formatPricingLine,
  getModelsForProvider,
  getProviderById,
  getRecommendedProviders,
  modelSupportsReasoning,
  modelSupportsVision,
} from "@agentcowork/core-providers";

describe("core-providers smoke-import (APP workspace dep)", () => {
  test("module resolves and the catalog is populated", () => {
    expect(PROVIDER_CATALOG.length).toBeGreaterThan(0);
    expect(AI_PROVIDER_GROUPS.length).toBeGreaterThan(0);
  });

  test("every provider the desktop broker targets is in the catalog", () => {
    // Mirror of agentcowork-vault broker.rs DEFAULT_BASE_URLS. Known id drift
    // (documented 2026-08-10): the broker's provider key is "nvidia" while
    // the APP catalog id is "nvidia-nim" — SAME base URL
    // (https://integrate.api.nvidia.com/v1), different identifiers. If the
    // catalog id is ever renamed to "nvidia", drop the alias below.
    const brokerKeyToCatalogId: Record<string, string> = {
      nvidia: "nvidia-nim",
      openai: "openai",
      anthropic: "anthropic",
      deepseek: "deepseek",
      groq: "groq",
    };
    for (const [brokerKey, catalogId] of Object.entries(brokerKeyToCatalogId)) {
      const entry = getProviderById(catalogId);
      expect(entry, `catalog entry for broker key ${brokerKey}`).toBeDefined();
    }
  });

  test("getProviderById finds a provider and returns undefined for junk", () => {
    const openai = getProviderById("openai");
    expect(openai).toBeDefined();
    expect(openai!.id).toBe("openai");
    expect(getProviderById("definitely-not-a-provider")).toBeUndefined();
  });

  test("selector helpers return shaped results without I/O", () => {
    expect(getRecommendedProviders().length).toBeGreaterThan(0);
    const openai = getProviderById("openai");
    expect(openai).toBeDefined();
    // ProviderGroup is a string-literal union from @agentcowork/core-domain;
    // the entry's group must be a member of the AI groups array.
    expect(AI_PROVIDER_GROUPS.includes(openai!.group)).toBe(true);
    expect(openai!.groupLabel.length).toBeGreaterThan(0);
    const aiSection = filterProvidersBySection("ai");
    expect(aiSection.length).toBeGreaterThan(0);
    const models = getModelsForProvider("openai");
    expect(Array.isArray(models)).toBe(true);
  });

  test("the observability surface is present and correctly typed", () => {
    // Functions exist (and are not called — no network).
    expect(typeof fetchProviderPricing).toBe("function");
    expect(typeof formatPricingLine).toBe("function");
    expect(typeof modelSupportsVision).toBe("function");
    expect(typeof modelSupportsReasoning).toBe("function");
    // Class export — the credential façade (custody stays in the Rust vault).
    expect(typeof ProviderVault).toBe("function");
  });

  test("no AgentCowork-owned inference path is exported (P71.2d, ADR-0005)", async () => {
    const mod = (await import("@agentcowork/core-providers")) as Record<string, unknown>;
    for (const gone of [
      "streamCompletion",
      "fetchAvailableModels",
      "validateApiKey",
      "streamAnthropicCompletion",
      "validateAnthropicApiKey",
      "ANTHROPIC_KNOWN_MODELS",
    ]) {
      expect(mod[gone], `${gone} must not be exported`).toBeUndefined();
    }
  });
});
