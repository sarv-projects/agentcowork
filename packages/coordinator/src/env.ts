/**
 * DEC-053 renamed-variable resolution (TS sidecar).
 *
 * The `EVERYAIOS_*` environment variables were renamed to `AGENTCOWORK_*`.
 * Every read goes through {@link envWithFallback}: the new name wins, the
 * legacy spelling is honored as a fallback, and an absent variable yields the
 * caller's existing default. Anything this process spawns or writes uses the
 * new name only. The one-time data migration (data home, config files) is
 * owned solely by Core (Rust) — this module never moves user data.
 *
 * The live OAuth gate is Rust (`oauth_status` over `OAUTH_ENV_FLAG`, whose
 * rename + fallback the Rust lane owns); {@link isOAuthEnabled} mirrors its
 * "set means on" semantics so the OAUTH rename precedence stays unit-tested
 * on this side too.
 */

/** Minimal env surface — `process.env` and plain test maps both satisfy it. */
export interface EnvLike {
  readonly [key: string]: string | undefined;
}

/** New → legacy pairs, one entry per renamed variable (DEC-053). */
export const HEARTBEAT_ENV_NEW = "AGENTCOWORK_HEARTBEAT_MS";
export const HEARTBEAT_ENV_LEGACY = "EVERYAIOS_HEARTBEAT_MS";
export const WEBHOOK_PORT_ENV_NEW = "AGENTCOWORK_WEBHOOK_PORT";
export const WEBHOOK_PORT_ENV_LEGACY = "EVERYAIOS_WEBHOOK_PORT";
export const OAUTH_ENV_NEW = "AGENTCOWORK_OAUTH";
export const OAUTH_ENV_LEGACY = "EVERYAIOS_OAUTH";

/**
 * Resolve one renamed variable: the new name wins, the legacy spelling is
 * honored when the new one is absent or empty, otherwise `undefined` (the
 * caller applies its existing default). Empty counts as absent so a blank
 * export cannot shadow a set legacy value.
 */
export function envWithFallback(
  newName: string,
  legacyName: string,
  env: EnvLike = process.env,
): string | undefined {
  const next = env[newName];
  if (next !== undefined && next !== "") return next;
  const legacy = env[legacyName];
  if (legacy !== undefined && legacy !== "") return legacy;
  return undefined;
}

/**
 * Heartbeat interval in ms: renamed var → legacy var → `fallbackMs`.
 * Non-numeric and non-positive values fall through to the default, exactly as
 * before the rename.
 */
export function resolveHeartbeatIntervalMs(
  env: EnvLike = process.env,
  fallbackMs = 10_000,
): number {
  const raw = envWithFallback(HEARTBEAT_ENV_NEW, HEARTBEAT_ENV_LEGACY, env);
  const n = raw === undefined ? NaN : Number(raw);
  return Number.isFinite(n) && n > 0 ? n : fallbackMs;
}

/**
 * Webhook ingress port: renamed var → legacy var → `0` (ephemeral). A
 * non-numeric value yields `NaN`, as before — `Bun.serve` rejects it and the
 * ingress stays down (non-fatal by design).
 */
export function resolveWebhookPort(env: EnvLike = process.env): number {
  const raw = envWithFallback(WEBHOOK_PORT_ENV_NEW, WEBHOOK_PORT_ENV_LEGACY, env);
  return raw === undefined ? 0 : Number(raw);
}

/**
 * Whether subscription OAuth is flagged on. Mirrors the Rust gate
 * (`std::env::var(...).is_ok()`): either spelling set — even to an empty
 * value — means on; neither set means off.
 */
export function isOAuthEnabled(env: EnvLike = process.env): boolean {
  const next = env[OAUTH_ENV_NEW];
  if (next !== undefined) return true;
  return env[OAUTH_ENV_LEGACY] !== undefined;
}
