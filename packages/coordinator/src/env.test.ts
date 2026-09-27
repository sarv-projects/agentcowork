import { describe, expect, test } from "bun:test";
import {
  HEARTBEAT_ENV_LEGACY,
  HEARTBEAT_ENV_NEW,
  OAUTH_ENV_LEGACY,
  OAUTH_ENV_NEW,
  WEBHOOK_PORT_ENV_LEGACY,
  WEBHOOK_PORT_ENV_NEW,
  envWithFallback,
  isOAuthEnabled,
  resolveHeartbeatIntervalMs,
  resolveWebhookPort,
  type EnvLike,
} from "./env";

// DEC-053: each renamed variable resolves new-wins → legacy-honored → default.
// Plain maps stand in for `process.env` so no test mutates the real env.

describe("envWithFallback — one resolver for the DEC-053 rename", () => {
  test("new name wins when both spellings are set", () => {
    const env: EnvLike = { [HEARTBEAT_ENV_NEW]: "111", [HEARTBEAT_ENV_LEGACY]: "222" };
    expect(envWithFallback(HEARTBEAT_ENV_NEW, HEARTBEAT_ENV_LEGACY, env)).toBe("111");
  });

  test("legacy spelling is honored when the new name is absent", () => {
    const env: EnvLike = { [HEARTBEAT_ENV_LEGACY]: "222" };
    expect(envWithFallback(HEARTBEAT_ENV_NEW, HEARTBEAT_ENV_LEGACY, env)).toBe("222");
  });

  test("empty new name falls through to the legacy value", () => {
    const env: EnvLike = { [HEARTBEAT_ENV_NEW]: "", [HEARTBEAT_ENV_LEGACY]: "222" };
    expect(envWithFallback(HEARTBEAT_ENV_NEW, HEARTBEAT_ENV_LEGACY, env)).toBe("222");
  });

  test("absent pair yields undefined (the caller applies its default)", () => {
    expect(envWithFallback(HEARTBEAT_ENV_NEW, HEARTBEAT_ENV_LEGACY, {})).toBeUndefined();
    const env: EnvLike = { [HEARTBEAT_ENV_NEW]: "", [HEARTBEAT_ENV_LEGACY]: "" };
    expect(envWithFallback(HEARTBEAT_ENV_NEW, HEARTBEAT_ENV_LEGACY, env)).toBeUndefined();
  });
});

describe("AGENTCOWORK_HEARTBEAT_MS → EVERYAIOS_HEARTBEAT_MS", () => {
  const FALLBACK = 10_000;
  test("new wins; legacy honored; absent yields the existing default", () => {
    expect(
      resolveHeartbeatIntervalMs({ [HEARTBEAT_ENV_NEW]: "500", [HEARTBEAT_ENV_LEGACY]: "200" }, FALLBACK),
    ).toBe(500);
    expect(resolveHeartbeatIntervalMs({ [HEARTBEAT_ENV_LEGACY]: "200" }, FALLBACK)).toBe(200);
    expect(resolveHeartbeatIntervalMs({}, FALLBACK)).toBe(FALLBACK);
  });

  test("non-numeric, zero and negative values fall back to the default", () => {
    for (const raw of ["abc", "0", "-5", ""]) {
      expect(resolveHeartbeatIntervalMs({ [HEARTBEAT_ENV_NEW]: raw }, FALLBACK)).toBe(FALLBACK);
    }
    // A blank new name must not shadow a set legacy value.
    expect(
      resolveHeartbeatIntervalMs({ [HEARTBEAT_ENV_NEW]: "", [HEARTBEAT_ENV_LEGACY]: "200" }, FALLBACK),
    ).toBe(200);
  });
});

describe("AGENTCOWORK_WEBHOOK_PORT → EVERYAIOS_WEBHOOK_PORT", () => {
  test("new wins; legacy honored; absent yields 0 (ephemeral)", () => {
    expect(
      resolveWebhookPort({ [WEBHOOK_PORT_ENV_NEW]: "8080", [WEBHOOK_PORT_ENV_LEGACY]: "9090" }),
    ).toBe(8080);
    expect(resolveWebhookPort({ [WEBHOOK_PORT_ENV_LEGACY]: "9090" })).toBe(9090);
    expect(resolveWebhookPort({})).toBe(0);
  });
});

describe("AGENTCOWORK_OAUTH → EVERYAIOS_OAUTH", () => {
  test("new wins; legacy honored; absent yields off", () => {
    expect(isOAuthEnabled({ [OAUTH_ENV_NEW]: "1", [OAUTH_ENV_LEGACY]: "1" })).toBe(true);
    expect(isOAuthEnabled({ [OAUTH_ENV_NEW]: "1" })).toBe(true);
    expect(isOAuthEnabled({ [OAUTH_ENV_LEGACY]: "1" })).toBe(true);
    expect(isOAuthEnabled({})).toBe(false);
  });

  test("set-but-empty still enables (mirrors the Rust `is_ok()` gate)", () => {
    expect(isOAuthEnabled({ [OAUTH_ENV_NEW]: "" })).toBe(true);
    expect(isOAuthEnabled({ [OAUTH_ENV_LEGACY]: "" })).toBe(true);
  });

  test("legacy fallback is reachable through the shared resolver", () => {
    expect(envWithFallback(OAUTH_ENV_NEW, OAUTH_ENV_LEGACY, { [OAUTH_ENV_LEGACY]: "1" })).toBe("1");
    expect(envWithFallback(OAUTH_ENV_NEW, OAUTH_ENV_LEGACY, {})).toBeUndefined();
  });
});
