// P2.3 / P6.x — the real 42-tool MCP registry (`agentcowork-mcp`: 37 browser +
// 5 storage tools) surfaced to the Connectors panel. Mirrors `mcp_cmds.rs`;
// in a plain-browser preview it returns a small demo so the tab is explorable.

import { inTauri, invoke } from "./tauri";
import { nativeCall } from './runtime';
import { guardTickets } from './guard';

export interface ToolInfo {
  name: string;
  kind: string;
  read_only: boolean;
  open_world: boolean;
  profile: string;
  description: string;
  args: number;
}

export interface McpCatalog {
  total: number;
  browser: number;
  storage: number;
  read_only: number;
  open_world: number;
  tools: ToolInfo[];
}

export async function mcpCatalog(): Promise<McpCatalog> {
  if (!inTauri()) return demoCatalog();
  return nativeCall('MCP catalog', () => invoke<McpCatalog>("mcp_catalog"));
}

/** P11.5.8 — one known/attached MCP server row. */
export interface McpServerRow {
  name: string;
  status: "connected" | "disconnected";
  transport: "stdio" | "http" | "native";
  tools: number;
  desc: string;
  /** P55.11 — the tool names the server actually advertised in `tools/list`.
   * Empty = no handshake on record (never a fabricated list). */
  toolNames: string[];
  /** P51.18 — persisted spawn identity (survives Stop). */
  command?: string;
  args?: string[];
  autoStart?: boolean;
  stopped?: boolean;
}

/** P11.5.8 — the installed/user MCP servers list (replaces hardcoded rows). */
export async function mcpServers(): Promise<McpServerRow[]> {
  if (!inTauri()) return demoServers();
  return nativeCall('MCP server list', () => invoke<McpServerRow[]>("mcp_servers"));
}

/** P55.11 — one tool discovered from an attached external MCP server. */
export interface McpExternalTool {
  name: string;
  description: string;
  readOnly: boolean;
  openWorld: boolean;
  source: string;
}

/** P55.11 — the live external tools: read from the agent's own tool catalog
 * (the one `tool/list` serves) so a surfaced tool is always callable.
 * `agentVisible=false` means the runtime has not reconciled yet — an honest
 * "not wired here", not an empty success. */
export interface McpExternalCatalog {
  native: number;
  external: number;
  total: number;
  agentVisible: boolean;
  tools: McpExternalTool[];
}

export const EMPTY_EXTERNAL_CATALOG: McpExternalCatalog = {
  native: 0,
  external: 0,
  total: 0,
  agentVisible: false,
  tools: [],
};

export async function mcpExternalTools(): Promise<McpExternalCatalog> {
  if (!inTauri()) return EMPTY_EXTERNAL_CATALOG;
  return nativeCall('MCP external catalog', () =>
    invoke<McpExternalCatalog>("mcp_external_tools"));
}

/** P11.5.8 + P50.3.5 — attach a user-supplied stdio MCP server, **request**
 * half: the shell mints a Guard-2 ticket bound to the exact command + args.
 * Consent is enforced in Rust (no ticket → no spawn), not only in UI copy. */
export async function mcpAttachRequest(
  name: string,
  command: string,
  args: string[],
): Promise<{ action: "allow" | "ask"; ticketId: string; approvalNonce: string }> {
  if (!inTauri()) return { action: "allow", ticketId: "preview", approvalNonce: "preview" };
  return nativeCall('MCP attach request', () =>
    invoke<{ action: "allow" | "ask"; ticketId: string; approvalNonce: string }>(
      "mcp_attach_request",
      { name, command, args },
    ));
}

/** P50.3.5 — attach, **commit** half: consume the ticket and spawn. Only
 * succeeds after the guard-window approval of exactly this command line. */
export async function mcpAttachCommit(
  name: string,
  command: string,
  args: string[],
  ticketId: string,
): Promise<{
  name: string;
  tools: string[];
  desc: string;
  /** Ids the agent loop registered (native collisions skipped). */
  registered: string[];
  /** False when no agent runtime was attached to register into. */
  agentVisible: boolean;
}> {
  // P55.11 — a failed handshake rejects here (no row, no child): the catch in
  // the panel surfaces the real reason instead of a fake "0 tools attached".
  if (!inTauri())
    return {
      name,
      tools: ["mcp_tool_1"],
      desc: "demo attach",
      registered: ["mcp_tool_1"],
      agentVisible: true,
    };
  return nativeCall('MCP attach commit', () =>
    invoke("mcp_attach_commit", { name, command, args, ticketId }));
}

/** P50.3.5 — detach a server: row removed + live child killed + disconnect
 * persisted (the server will not reappear connected after a restart). */
export async function mcpDetach(name: string): Promise<boolean> {
  if (!inTauri()) return true;
  return nativeCall('MCP detach', () => invoke<boolean>("mcp_detach", { name }));
}

/** P51.18 — AnythingLLM Stop: kill the child, keep identity. */
export async function mcpStop(name: string): Promise<boolean> {
  if (!inTauri()) return true;
  return nativeCall('MCP stop', () => invoke<boolean>("mcp_stop", { name }));
}

/** P51.18 — AnythingLLM Start: spawn from persisted command. */
export async function mcpStart(name: string): Promise<{ name: string; alreadyRunning?: boolean }> {
  if (!inTauri()) return { name, alreadyRunning: true };
  return nativeCall('MCP start', () => invoke("mcp_start", { name }));
}

export async function mcpSetAutostart(name: string, autoStart: boolean): Promise<McpServerRow> {
  if (!inTauri()) {
    return {
      name,
      status: "disconnected",
      transport: "stdio",
      tools: 0,
      desc: "",
      toolNames: [],
      autoStart,
    };
  }
  return nativeCall('MCP autoStart', () =>
    invoke<McpServerRow>("mcp_set_autostart", { name, autoStart }));
}

/** P51.18 — no-restart refresh + auto-start of eligible rows. */
export async function mcpRefresh(): Promise<{
  servers: McpServerRow[];
  prunedDead: string[];
  autoStarted?: string[];
}> {
  if (!inTauri()) return { servers: demoServers(), prunedDead: [] };
  return nativeCall('MCP refresh', () => invoke("mcp_refresh"));
}

/**
 * Wait until a Guard-2 ticket leaves the pending stack (approved or rejected
 * in the dedicated guard window). `allow` requests are already auto-approved,
 * so callers can skip straight to commit. Resolves `true` on approval
 * (ticket gone), `false` on timeout.
 */
export async function waitForTicketResolution(
  ticketId: string,
  timeoutMs = 120_000,
): Promise<boolean> {
  if (!inTauri()) return true;
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const tickets = await guardTickets();
    if (!tickets.some((t) => t.ticketId === ticketId)) return true;
    await new Promise((r) => setTimeout(r, 1_000));
  }
  return false;
}

/** Connect-Store — the curated "click → sign in → use" connector list. */
export interface StoreEntry {
  id: string;
  kind: "remote-mcp" | "connector";
  name: string;
  description: string;
  url: string | null;
  flow: "pkce" | "device-code" | "api-key";
  vaultProvider: string;
  toolHint: number;
  scopesPlain: string[];
  canMutate: boolean;
  indexesIntoMemory: boolean;
}

export async function storeCatalog(): Promise<StoreEntry[]> {
  if (!inTauri()) return demoStore();
  return nativeCall('connector store catalog', () => invoke<StoreEntry[]>("store_catalog"));
}

/** One remote tool drawn from a connected server's tools/list. */
export interface RemoteToolInfo {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
}

/** Fetch tools/list from a connected remote MCP server (merge into catalog). */
export async function mcpRemoteTools(
  storeId: string,
): Promise<RemoteToolInfo[]> {
  if (!inTauri()) return [];
  return nativeCall('remote MCP tools', () => invoke<RemoteToolInfo[]>("mcp_remote_tools", { storeId }));
}

/** P50.2.6 — live connected state for a store entry (vault token present?).
 * Preview returns not-connected; native rejection propagates (fail-closed). */
export async function mcpRemoteStatus(
  storeId: string,
): Promise<{ connected: boolean }> {
  if (!inTauri()) return { connected: false };
  return nativeCall('remote MCP status', () => invoke<{ connected: boolean }>("mcp_remote_status", { storeId }));
}

/** Start a remote-MCP OAuth 2.1 connect (discovery + PKCE) → auth URL. */
export async function mcpConnectStart(
  storeId: string,
): Promise<{ authUrl: string; state: string; redirectUri: string }> {
  if (!inTauri()) {
    return { authUrl: "https://example.com/oauth/authorize?demo=1", state: "demo", redirectUri: "http://127.0.0.1:0/oauth/callback" };
  }
  return nativeCall('MCP connect start', () => invoke("mcp_connect_start", { storeId }));
}

function demoStore(): StoreEntry[] {
  return [
    {
      id: "github",
      kind: "remote-mcp",
      name: "GitHub",
      description: "Repos, issues, PRs via the official GitHub MCP server.",
      url: "https://api.githubcopilot.com/mcp/",
      flow: "device-code",
      vaultProvider: "copilot",
      toolHint: 30,
      scopesPlain: ["Read your repositories, issues, and pull requests"],
      canMutate: true,
      indexesIntoMemory: false,
    },
    {
      id: "google-drive",
      kind: "remote-mcp",
      name: "Google Drive",
      description: "Read and write your Google Drive via the official Drive connector.",
      url: "https://mcp.googleapis.com/mcp/",
      flow: "pkce",
      vaultProvider: "google",
      toolHint: 12,
      scopesPlain: ["View your Google Drive file list and metadata"],
      canMutate: true,
      indexesIntoMemory: false,
    },
    {
      id: "microsoft-graph",
      kind: "remote-mcp",
      name: "Microsoft Graph",
      description: "Outlook mail, OneDrive, and Calendar via Microsoft Graph.",
      url: "https://mcp.microsoft.com/mcp/",
      flow: "pkce",
      vaultProvider: "microsoft",
      toolHint: 20,
      scopesPlain: ["Read your Outlook mail headers", "Read your OneDrive file list"],
      canMutate: true,
      indexesIntoMemory: false,
    },
  ];
}

function demoServers(): McpServerRow[] {
  return [
    { name: "AgentCowork native (built-in)", status: "connected", transport: "native", tools: 42, desc: "37 browser + 5 storage tools", toolNames: [] },
    { name: "GitHub MCP", status: "connected", transport: "stdio", tools: 18, desc: "Repo, issues, PRs", toolNames: ["list_issues", "create_pr", "get_file"] },
    { name: "Filesystem MCP", status: "connected", transport: "stdio", tools: 7, desc: "Read/write local files", toolNames: ["read_file", "write_file", "list_dir"] },
  ];
}

function demoCatalog(): McpCatalog {
  const samples: ToolInfo[] = [
    { name: "click", kind: "edit", read_only: false, open_world: false, profile: "core", description: "Click an element by ref", args: 1 },
    { name: "snapshot", kind: "read", read_only: true, open_world: false, profile: "core", description: "Accessibility tree snapshot", args: 1 },
    { name: "navigate", kind: "fetch", read_only: false, open_world: true, profile: "network", description: "Navigate to a URL", args: 1 },
    { name: "type", kind: "edit", read_only: false, open_world: false, profile: "core", description: "Type text into an element", args: 2 },
    { name: "disk_scan", kind: "read", read_only: true, open_world: false, profile: "all", description: "Scan a directory tree", args: 1 },
  ];
  return {
    total: samples.length,
    browser: 37,
    storage: 5,
    read_only: 2,
    open_world: 1,
    tools: samples,
  };
}
