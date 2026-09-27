/**
 * P22-2 — Tauri/Connectors surface for the MCP server manager (doc 74 §3):
 * `mcp_servers` / `mcp_install` / `mcp_run` / `mcp_tools` command bodies +
 * the data the "MCP Servers" tab renders.
 *
 * The Rust `agentcowork-mcp::manager` owns the registry/allow-list/sha256
 * contract; this module is the host-side surface: it serves the P18 seed
 * (user-supplied, hosted), turns an install into an `AttachPlan` the Rust
 * `AttachedServer::spawn` executes (reusing P37 validation), tracks managed
 * runs, and shapes the tool surface. Every install is Guard-2-gated here
 * (no silent install), mirroring the ACP installer posture.
 */

import { CATALOG_SEED, type McpCatalogEntry } from "./mcp-catalog";
import { attachablePlans, type AttachPlan } from "./mcp-install";

/** The tab's list: the seed + hosts the user has installed. */
export interface McpServerView {
  entry: McpCatalogEntry;
  state: "discovered" | "installed" | "running" | "quarantined";
}

/** The Guard-2-shaped approval an install/run must carry. */
export interface ManagerApproval {
  ticketId: string;
  target: string;
  used: boolean;
}

/** Outcome of one install click. */
export type InstallOutcome =
  | { ok: true; view: McpServerView; plan: AttachPlan }
  | { ok: false; reason: string };

const installed = new Map<string, { plan: AttachPlan; running: boolean }>();
const quarantined = new Set<string>();
const approvals = new Map<string, ManagerApproval>();

/** `mcp_servers` — the tab's list (seed + live state). */
export function mcpServers(): McpServerView[] {
  return CATALOG_SEED.map((entry) => {
    const rec = installed.get(entry.id);
    if (quarantined.has(entry.id)) return { entry, state: "quarantined" };
    if (rec?.running) return { entry, state: "running" };
    if (rec) return { entry, state: "installed" };
    return { entry, state: "discovered" };
  });
}

/**
 * `mcp_install` — Guard-2-gated: an approval must exist for this server
 * (the card flow issues it), then the install is planned through the P37
 * validator and recorded.
 */
export function mcpInstall(
  id: string,
  approval?: ManagerApproval,
  version?: string,
): InstallOutcome {
  if (quarantined.has(id)) return { ok: false, reason: `\`${id}\` is quarantined` };
  const entry = CATALOG_SEED.find((e) => e.id === id);
  if (!entry || !entry.install) {
    return { ok: false, reason: `no install command for \`${id}\` (hosted server — supply one)` };
  }
  if (!approval || approval.target !== id || approval.used) {
    return { ok: false, reason: "install requires a fresh Guard-2 approval" };
  }
  approvals.set(id, { ...approval, used: true });
  const { plans, rejected } = attachablePlans([
    { id: entry.id, name: entry.name, command: entry.install, ...(version ? { version } : {}) },
  ]);
  const planned = plans[0];
  if (!planned) {
    return {
      ok: false,
      reason: `install refused: ${rejected[0]?.reason ?? "invalid"} (K6: supply a pinned version)`,
    };
  }
  installed.set(id, { plan: planned.plan, running: false });
  return { ok: true, view: { entry, state: "installed" }, plan: planned.plan };
}

/** `mcp_run` — start an installed server as a managed child. */
export function mcpRun(id: string): { ok: boolean; reason?: string } {
  const rec = installed.get(id);
  if (!rec) return { ok: false, reason: `\`${id}\` is not installed` };
  if (rec.running) return { ok: false, reason: `\`${id}\` is already running` };
  installed.set(id, { ...rec, running: true });
  return { ok: true };
}

/** `mcp_stop` — the host kills the managed child. */
export function mcpStop(id: string): void {
  const rec = installed.get(id);
  if (rec) installed.set(id, { ...rec, running: false });
}

/** `mcp_quarantine` — K6: a discovered bad server is never allowed again. */
export function mcpQuarantine(id: string): void {
  installed.delete(id);
  quarantined.add(id);
}

/** `mcp_tools` — the tool surface for a running server (read/write kinds). */
export interface ToolSurfaceView {
  name: string;
  kind: "read" | "write";
  origin: string;
}

export function mcpTools(id: string): ToolSurfaceView[] {
  const rec = installed.get(id);
  if (!rec || !rec.running) return [];
  // The real tool list comes from the server's `tools/list` via the Rust
  // merge_into_catalog surface; this shape is what the registry renders.
  return [
    { name: `${id}.list`, kind: "read", origin: id },
    { name: `${id}.get`, kind: "read", origin: id },
  ];
}
