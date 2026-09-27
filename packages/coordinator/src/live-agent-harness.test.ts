/**
 * Live End-to-End Agent Harness & Real Binary Execution Test
 *
 * Exercises the actual installed binaries on the machine:
 * 1. OpenCode (`opencode acp`): Real stdio ACP JSON-RPC handshake
 * 2. Grok Build (`grok`): Real binary execution and CLI inspection
 * 3. Primary-agent resolution & installed-ness derivation
 * 4. Two-Plane Shared Cowork Capability injection
 */

import { describe, expect, test } from "bun:test";
import { spawn, spawnSync } from "node:child_process";
import {
  primaryAgentRegistry,
  resolveSessionPrimaryAgent,
  checkSpawn,
  deriveChildPermissions,
  type SpawnState,
} from "./primary-agent";
import {
  STANDARD_SHARED_CAPABILITIES,
  isCapabilityEnabled,
} from "../../../ui/src/lib/capabilities";

/**
 * Is a real agent CLI installed on this machine?
 *
 * The live tests in this file spawn the **installed** `opencode` / `grok`
 * binaries. CI installs neither, so without this gate they failed with
 * `ENOENT` (2 failed + 2 errors) — reporting an environment gap as a product
 * defect and making the suite red for everyone. A missing binary must SKIP.
 *
 * `spawnSync` without a shell sets `error` only when the spawn itself failed
 * (ENOENT/EACCES); a binary that exists but exits non-zero is still present.
 */
function hasBinary(name: string): boolean {
  return !spawnSync(name, ["--version"], { stdio: "ignore" }).error;
}

const hasOpencode = hasBinary("opencode");
const hasGrok = hasBinary("grok");

if (!hasOpencode || !hasGrok) {
  // Say which side is missing, so a developer running locally knows what to
  // install rather than assuming the harness passed.
  console.warn(
    `[live-agent-harness] skipping real-binary tests — ` +
      `opencode=${hasOpencode ? "present" : "MISSING"} grok=${hasGrok ? "present" : "MISSING"}`,
  );
}

describe("Live Real-World Agent Harness Verification", () => {
  test.skipIf(!hasOpencode)("Live OpenCode ACP stdio handshake (real binary)", async () => {
    // 1. Spawn real opencode acp process
    const proc = spawn("opencode", ["acp"], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, PATH: `${process.env.HOME}/.bun/bin:${process.env.PATH}` },
    });

    let stdoutBuffer = "";
    proc.stdout.on("data", (chunk) => {
      stdoutBuffer += chunk.toString();
    });

    // 2. Send ACP initialize over stdio
    const initRequest = {
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: 1,
        clientInfo: {
          name: "AgentCowork",
          version: "2.0.0",
        },
      },
    };

    proc.stdin.write(JSON.stringify(initRequest) + "\n");

    // 3. Wait for response (allow up to 15s for cold start)
    await new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => {
        proc.kill();
        reject(new Error("Timeout waiting for opencode acp response"));
      }, 15000);

      const interval = setInterval(() => {
        if (stdoutBuffer.includes('"jsonrpc":"2.0"')) {
          clearTimeout(timeout);
          clearInterval(interval);
          resolve();
        }
      }, 100);
    });

    proc.kill();

    // 4. Parse and verify response from real OpenCode binary
    const responseLine = stdoutBuffer.split("\n").find((l) => l.includes('"jsonrpc":"2.0"'));
    expect(responseLine).toBeDefined();

    const parsed = JSON.parse(responseLine!);
    expect(parsed.id).toBe(1);
    expect(parsed.result).toBeDefined();
    expect(parsed.result.protocolVersion).toBe(1);
    expect(parsed.result.agentInfo.name).toBe("OpenCode");
    expect(typeof parsed.result.agentInfo.version).toBe("string");
    expect(parsed.result.agentCapabilities).toBeDefined();
    expect(parsed.result.agentCapabilities.loadSession).toBe(true);
  }, 15000);

  test.skipIf(!hasGrok)("Live Grok Build binary presence and execution", async () => {
    const proc = spawn("grok", ["--version"], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, PATH: `${process.env.HOME}/.bun/bin:${process.env.PATH}` },
    });

    let stdout = "";
    proc.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });

    const exitCode = await new Promise<number>((resolve) => {
      proc.on("close", resolve);
    });

    expect(exitCode).toBe(0);
    expect(stdout).toContain("grok");
  });

  test("Primary: OpenCode agent delegating to Grok Build Subagent with Shared Cowork loadout", () => {
    // Session pinned to real installed OpenCode
    const sessionId = "live-session-opencode-primary";
    primaryAgentRegistry.setSessionPin(sessionId, "opencode");
    expect(resolveSessionPrimaryAgent({ sessionPin: "opencode" })).toBe("opencode");

    // OpenCode delegates to Grok Build subagent
    const spawnState: SpawnState = {
      depth: 1,
      active: 0,
      stepsUsed: 5,
      parentPermissions: new Set([
        "read",
        "edit",
        "terminal",
        "shared:office",
        "shared:browser",
        "shared:desktop",
        "shared:calendar",
      ]),
      denies: new Set(),
      grants: new Set(["shared:fleet"]),
    };

    const verdict = checkSpawn(spawnState);
    expect(verdict.allowed).toBe(true);

    const childPermissions = deriveChildPermissions(spawnState);
    expect(childPermissions.has("shared:office")).toBe(true);
    expect(childPermissions.has("shared:browser")).toBe(true);
    expect(childPermissions.has("shared:desktop")).toBe(true);
    expect(childPermissions.has("shared:calendar")).toBe(true);
    expect(childPermissions.has("shared:fleet")).toBe(true);
  });

  test("Primary: Grok Build agent delegating to OpenCode Subagent with selective denial", () => {
    // Session pinned to real installed Grok Build
    const sessionId = "live-session-grok-primary";
    primaryAgentRegistry.setSessionPin(sessionId, "grok");
    expect(resolveSessionPrimaryAgent({ sessionPin: "grok" })).toBe("grok");

    // Grok Build delegates code editing to OpenCode subagent while restricting desktop computer use
    const spawnState: SpawnState = {
      depth: 1,
      active: 1,
      stepsUsed: 15,
      parentPermissions: new Set([
        "read",
        "edit",
        "terminal",
        "shared:office",
        "shared:browser",
        "shared:desktop",
      ]),
      denies: new Set(["shared:desktop"]), // Restrict desktop GUI control for subagent
      grants: new Set(),
    };

    const verdict = checkSpawn(spawnState);
    expect(verdict.allowed).toBe(true);

    const childPermissions = deriveChildPermissions(spawnState);
    expect(childPermissions.has("read")).toBe(true);
    expect(childPermissions.has("edit")).toBe(true);
    expect(childPermissions.has("shared:office")).toBe(true);
    expect(childPermissions.has("shared:browser")).toBe(true);
    expect(childPermissions.has("shared:desktop")).toBe(false); // Verified denial
  });

  test("Shared Cowork capability resolution across both agents", () => {
    // Verify that every single shared cowork capability is accessible
    const capabilities = STANDARD_SHARED_CAPABILITIES.map((c) => c.id);
    for (const capId of capabilities) {
      expect(isCapabilityEnabled(capId, null)).toBe(true);
    }
  });

  test.skipIf(!hasOpencode)(
    "OpenCode model verification confirms opencode/big-pickle zero-login model",
    async () => {
    const proc = spawn("opencode", ["models"], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, PATH: `${process.env.HOME}/.bun/bin:${process.env.PATH}` },
    });

    let stdout = "";
    proc.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });

    const exitCode = await new Promise<number>((resolve) => {
      proc.on("close", resolve);
    });

    expect(exitCode).toBe(0);
    expect(stdout).toContain("opencode/big-pickle");
    },
    15000,
  );

  test.skipIf(!hasGrok)("Grok Build models command lists available execution tiers", async () => {
    const proc = spawn("grok", ["models"], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, PATH: `${process.env.HOME}/.bun/bin:${process.env.PATH}` },
    });

    let stdout = "";
    proc.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });

    const exitCode = await new Promise<number>((resolve) => {
      proc.on("close", resolve);
    });

    expect(exitCode).toBe(0);
    expect(stdout).toContain("grok");
  }, 15000);
});
