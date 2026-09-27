// F12 / J17 — ACP harness bridge client (doc 45 §1, doc 57 §2). The agent
// picker: one manifest per agent, same chat bar, agent differs. Every entry
// drives an external agent CLI over ACP stdio and obeys the same Guard-2
// ticket card; there is no built-in engine and no assumed default
// (`ARCH/ADR/0005`).

import { invoke } from "./tauri";
import { nativeCall } from './runtime';

/**
 * Auth-mode badge (F12). **Canonical spelling** (P69.C11) — a projection of
 * `agentcowork_types::AuthMode`; one union for the whole stack, never a second
 * hand-maintained variant list.
 *
 * `local` means local inference on this machine (Ollama / llamafile /
 * on-device) per `ARCH/03-BYOK-KEYRINGS.md` §3.0 — it is not an open-source
 * marker. Anything the authoritative source is silent about is `unknown` and
 * must render as unknown. The legacy `'local_cli'` auth spelling is deleted
 * (it survives only in the readiness vocabulary, which is a UI state).
 */
export type AuthMode =
  | "subscription"
  | "api_key"
  | "local"
  | "keyless"
  | "unknown";

/** How the agent is driven. ADR-0005: `acp` is the whole v1 vocabulary —
 * the retired built-in (`inbuilt`) and model-backend (`model_backend`) paths
 * are gone and return post-v1. */
export type HarnessProtocol = "acp";

/** The agent's advertised ACP auth method (`authMethods` in initialize). */
export interface AuthMethod {
  id: string;
  name: string;
  description?: string;
  /** `agent` (default) | `url` | `terminal` — how login completes. */
  type?: "agent" | "url" | "terminal";
}

/** Button copy for one handshake method. The registry does not say this; the agent does. */
export function authMethodLabel(method: AuthMethod): string {
  const name = method.name?.trim() || "this agent";
  if (method.type === "terminal") return `Set up ${name}`;
  return `Sign in with ${name}`;
}

export interface HarnessManifest {
  id: string;
  name: string;
  description: string;
  authMode: AuthMode;
  protocol: HarnessProtocol;
  /** P50.3.9 — governance truth (present on every row from the shell). */
  governance?: GovernanceInfo;
}

/** P50.3.9 — how much of an agent's effects AgentCowork actually governs.
 * Never imply audit coverage that does not exist. */
export type GovernanceClass =
  | "GovernedMediated"
  | "SelfContained"
  | "NotGoverned";

export interface GovernanceInfo {
  class: GovernanceClass;
  /** True only when every effect lands on the AgentCowork audit trail. */
  auditedEffects: boolean;
  note: string;
}

/** Short picker/transcript badge label per governance class. */
export function governanceLabel(g: GovernanceInfo | undefined): string {
  switch (g?.class) {
    case "GovernedMediated":
      return "Governed — every effect ticketed + audited";
    case "SelfContained":
      return "Self-contained — approvals mediated; agent's own effects unaudited";
    case "NotGoverned":
      return "Not governed — no AgentCowork audit coverage";
    default:
      return "Governance unknown";
  }
}

export interface AcpConfigOptionValue {
  value: string | boolean
  name: string
  description?: string
}

export interface AcpConfigOption {
  id: string
  name: string
  description?: string
  category?: string
  type: 'select' | 'boolean' | string
  currentValue: string | boolean
  options?: AcpConfigOptionValue[]
}

export interface AcpHandleInfo {
  handle: string;
  agentId: string;
  agentName: string;
  /** Provider-native ACP session id (legacy wire name; not the app Session). */
  sessionId: string;
  /** Provider-native ACP session id, explicit in the current shell projection. */
  providerSessionId: string;
  /** AgentCowork application Session that owns the turn, once claimed. */
  applicationSessionId: string;
  /** Canonical Work id for the owning Session. */
  workId: string;
  /** Canonical AgentBinding id for the owning Work/Session. */
  bindingId: string;
  /** Current Run id, when a prompt has claimed the handle. */
  runId: string;
  protocol: string;
  /** True when the agent needs sign-in before it accepts a session. */
  authRequired: boolean;
  authMethods: AuthMethod[];
  /** P53.8 — the agent advertised `promptCapabilities.embeddedContext`. */
  embeddedContext: boolean;
  /** Complete agent-owned session options, including model when exposed. */
  configOptions: AcpConfigOption[];
}

/**
 * The renderer's live handle record.  The map is keyed by
 * `applicationSessionId + bindingId + workId + agentId`, never by agent alone:
 * two chats may bind the same external CLI at the same time.
 *
 * `key` is deliberately explicit so the store can keep one ordinary handle
 * table without introducing a second registry.  Empty `bindingId`/`workId`
 * values are valid only between `acp_launch` and the first successful prompt.
 */
export interface AcpHandleRecord {
  key: string;
  handle: string;
  applicationSessionId: string;
  bindingId: string;
  workId: string;
  /** Catalog/binding id used by the UI (not necessarily the ACP registry id). */
  agentId: string;
  providerSessionId: string;
  runId: string;
}

/** Opaque, collision-safe key for one Session/binding/Work handle record. */
export function acpHandleKey(
  applicationSessionId: string,
  bindingId: string,
  workId: string,
  agentId: string,
): string {
  if (!applicationSessionId.trim() || !agentId.trim()) {
    throw new Error('ACP handle records require a chat id and agent id');
  }
  return JSON.stringify([applicationSessionId, bindingId, workId, agentId]);
}

export type AcpHandleIdentity = Omit<
  AcpHandleRecord,
  'handle' | 'key' | 'providerSessionId' | 'runId'
>

export function parseAcpHandleKey(key: string): AcpHandleIdentity | null {
  try {
    const value: unknown = JSON.parse(key);
    if (!Array.isArray(value) || value.length !== 4 || value.some((part) => typeof part !== 'string')) {
      return null;
    }
    const [applicationSessionId, bindingId, workId, agentId] = value as [string, string, string, string];
    if (!applicationSessionId || !agentId) return null;
    return { applicationSessionId, bindingId, workId, agentId };
  } catch {
    return null;
  }
}

/** Build the store record returned by `acp_launch` before ownership is claimed. */
export function acpHandleRecordFromLaunch(
  info: AcpHandleInfo,
  applicationSessionId: string,
  agentId: string,
): AcpHandleRecord {
  const bindingId = info.bindingId ?? ''
  const workId = info.workId ?? ''
  return {
    key: acpHandleKey(applicationSessionId, bindingId, workId, agentId),
    handle: info.handle,
    applicationSessionId,
    bindingId,
    workId,
    agentId,
    providerSessionId: info.providerSessionId || info.sessionId,
    runId: info.runId,
  };
}

/**
 * Resolve a handle only inside the requested application Session.  Canonical
 * records (with binding/work identity) win over a launch-time provisional row.
 */
export function findAcpHandleRecord(
  handles: Readonly<Record<string, string>>,
  applicationSessionId: string,
  agentId?: string,
  bindingId?: string,
  workId?: string,
): AcpHandleRecord | undefined {
  const matches: AcpHandleRecord[] = [];
  for (const [key, handle] of Object.entries(handles)) {
    const parsed = parseAcpHandleKey(key);
    if (!parsed || parsed.applicationSessionId !== applicationSessionId) continue;
    if (agentId !== undefined && parsed.agentId !== agentId) continue;
    if (bindingId !== undefined && parsed.bindingId !== bindingId) continue;
    if (workId !== undefined && parsed.workId !== workId) continue;
    matches.push({
      key,
      handle,
      ...parsed,
      providerSessionId: '',
      runId: '',
    });
  }
  // `setAcpHandle` can retain more than one canonical binding for a Session.
  // Prefer the newest canonical row, while still preferring any canonical row
  // over a launch-time provisional row.
  matches.reverse();
  matches.sort((a, b) => {
    const aOwned = a.bindingId !== '' && a.workId !== '' ? 1 : 0;
    const bOwned = b.bindingId !== '' && b.workId !== '' ? 1 : 0;
    return bOwned - aOwned;
  });
  return matches[0];
}

/** One live slash command advertised by the agent (P53.1). */
export interface AvailableCommand {
  name: string
  description: string
  /** Optional JSON-schema-ish input hint (opaque — rendered as help text). */
  input?: unknown
}

export interface AcpPromptUpdate {
  sessionId?: string;
  sessionUpdate?: string;
  content?: { type?: string; text?: string }[];
  toolCallId?: string;
  title?: string;
  status?: string;
  /** P53.1 — live slash vocabulary (only on `available_commands_update`). */
  availableCommands?: AvailableCommand[];
  /** Complete agent-owned config state (only on `config_option_update`). */
  configOptions?: AcpConfigOption[];
}

export interface AcpPromptResult {
  handle: string;
  /** Canonical identity returned by the owner after the turn is admitted. */
  applicationSessionId: string;
  workId: string;
  bindingId: string;
  runId: string;
  providerSessionId: string;
  stopReason: string;
  updateCount: number;
  permissionCount: number;
  pendingTickets: string[];
  /** Actual assistant/session output collected by the ACP client. */
  finalText?: string;
  updates?: AcpPromptUpdate[];
  executionId?: string;
}

/** P66 — non-secret runtime provenance. Catalog membership is not occupancy. */
// DEC-053: `everyaios_install` is a persisted source enum — the spelling stays so stored runtimes keep matching (parsed in `agent-model-picker.tsx`, `local-models.ts`).
export type RuntimeLocation =
  | { kind: 'managed'; source: 'everyaios_install'; executable?: string; version?: string | null; verifiedAt?: string | null }
  | { kind: 'path' | 'windows_path'; source: 'path_probe' | 'app_paths' | 'user_selected'; executable: string; version?: string | null; verifiedAt?: string | null }
  | { kind: 'package_manager'; source: 'path_probe' | 'everyaios_install'; manager: 'npx' | 'uvx'; command?: string; package?: string; version?: string | null; verifiedAt?: string | null }
  | { kind: 'wsl'; source: 'wsl_probe' | 'user_selected'; distro: string; linuxPath: string; windowsLauncher: string; version?: string | null; verifiedAt?: string | null }
  | { kind: 'unavailable'; source: 'registry_catalog' | 'path_probe'; reason: string };

/** One agent's install state (F8/P66 — flip Install ↔ Launch honestly). */
export interface InstallState {
  /** P71.3f — the canonical readiness state; the booleans below are its
   * projections (kept for surfaces that still read them). */
  readiness?: AgentReadiness;
  /** AgentCowork-managed install or package-manager-ready launch path. */
  installed: boolean;
  /** A catalog entry has a verified runtime location, including WSL-only. */
  discovered?: boolean;
  /** The selected native launch adapter can currently start it. */
  launchable?: boolean;
  version?: string;
  kind?: string;
  binaryPath?: string | null;
  location?: RuntimeLocation;
}

/** The install-request verdict (Guard-2 ticket minted, or auto-allowed).
 * Ticket-every-effect: both `allow` and `ask` carry a single-use ticket. */
export interface InstallRequest {
  action: "allow" | "ask";
  agentId: string;
  version: string;
  ticketId: string;
  exactCommand?: string[];
  consentRequired?: boolean;
  preferNative?: boolean;
  /** P69.C12 — the consent surface's evidence: license (+ published URL),
   * verdict, why, and which catalog the row came from. Never empty. */
  license?: string;
  licenseUrl?: string | null;
  verdict?: "allow" | "ask";
  reason?: string;
  source?: string;
}

/** The result of `acp_authenticate` (url-type pending vs completed). */
export interface AuthenticateResult {
  ok: boolean;
  sessionId?: string;
  url?: string;
  pending?: boolean;
}

/** Catalog (UI) agent id → ACP registry id. The UI picker labels curated
 * rows with catalog ids (`claude-code`, `codex-cli`, `grok-build`, …); the
 * ACP launch/install registry keys them by registry id (`claude`, `codex`,
 * `grok`, …). Synthesized registry rows already carry their registry id, so
 * unknown ids pass through unchanged. Always translate before `acp_launch` /
 * `acp_install_*` calls. */
// P71.2a — no built-in row, so nothing maps to `everyaios` any more: that id
// named the retired built-in agent (ADR-0005 §1).
const CATALOG_TO_ACP: Record<string, string> = {
  "claude-code": "claude",
  "codex-cli": "codex",
  "grok-build": "grok",
  "gemini-cli": "gemini",
  "cursor-agent": "cursor",
  aider: "aider",
  opencode: "opencode",
};

export function acpIdFor(catalogId: string): string {
  return CATALOG_TO_ACP[catalogId] ?? catalogId;
}

/**
 * P71.2c — the retired built-in binding spellings (ADR-0005 §1). They are not
 * agents: resolving one to something else would substitute an engine the user
 * never chose, so they resolve to *nothing* and the turn refuses by name.
 *
 * This is the single source of truth for that predicate — `bridge.ts` (the turn
 * path) and the composer picker both read it, so no screen can disagree about
 * whether a spelling still names an agent.
 */
export function isRetiredBinding(agentId: string): boolean {
  // DEC-053: the legacy spellings stay so already-retired bindings keep being recognized, never resurrected.
  return agentId === "everyaios-native" || agentId === "everyaios" || agentId === "inbuilt";
}

/** The live binding for an id, or `null` when the id names nothing runnable. */
export function currentBinding(agentId: string | undefined): string | null {
  if (typeof agentId !== "string") return null;
  const id = agentId.trim();
  if (id === "" || isRetiredBinding(id)) return null;
  return id;
}

/** P69.D1 — one directory entry, composed server-side by
 * `agentcowork_agents::AgentDirectory`. The UI renders these rows; it never
 * merges agent lists of its own (the ACP registry and the local bundle store
 * meet in one place, in Rust). ADR-0005: there is no built-in row and no
 * default agent. */
export interface AgentDirectoryEntry {
  id: string;
  name: string;
  description: string;
  protocol: HarnessProtocol;
  authMode: AuthMode;
  /** Where the row came from — provenance the picker must be able to show. */
  source: 'acp_registry' | 'discovered' | 'local_bundle' | 'mcp';
  /** P71.3f — the one readiness state (Rust is the authority). */
  readiness: AgentReadiness;
  /** Whether the agent may serve a turn right now (ready | degraded). */
  ready: boolean;
  /** Derived from `readiness` — never a second truth. */
  installed: boolean;
  /** Whether the user may remove this row (discovered / local bundles only). */
  removable: boolean;
  /** Install/distribution hint, e.g. `npx: @scope/pkg` — never a secret. */
  locator: string | null;
}

/// P71.3f — the canonical readiness vocabulary (`agentcowork_types::AgentReadiness`
/// is the authority; this is the wire projection). `installed` is not `ready`:
/// an installed agent can still be unauthenticated, unnegotiated or degraded,
/// and the UI must say which.
export type AgentReadiness =
  | 'unknown'
  | 'discovered'
  | 'installed'
  | 'launchable'
  | 'protocol_compatible'
  | 'auth_required'
  | 'authenticating'
  | 'ready'
  | 'degraded'
  | 'unavailable'
  | 'failed'

/// A runtime is present on this machine.
export function isAgentInstalled(r: AgentReadiness): boolean {
  return (
    r === 'installed' ||
    r === 'launchable' ||
    r === 'protocol_compatible' ||
    r === 'auth_required' ||
    r === 'authenticating' ||
    r === 'ready' ||
    r === 'degraded'
  )
}

/// The agent may serve a turn (top rung, or top rung with a stated reduction).
export function isAgentReady(r: AgentReadiness): boolean {
  return r === 'ready' || r === 'degraded'
}

/**
 * Plain words for the canonical readiness state (ARCH/AGENT.md §3.1).
 *
 * One owner for the wording: the first-run gate, the onboarding scan and any
 * future surface must not each invent their own phrase for `auth_required` —
 * "sign in to continue" is the difference between a user knowing what to do and
 * reading a state name.
 */
export function readinessLabel(r: AgentReadiness | undefined): string {
  switch (r) {
    case 'ready':
      return 'ready';
    case 'degraded':
      return 'degraded — may fail mid-turn';
    case 'auth_required':
      return 'sign in to continue';
    case 'authenticating':
      return 'signing in…';
    case 'protocol_compatible':
      return 'installed — protocol not negotiated';
    case 'launchable':
      return 'launchable — not yet verified';
    case 'installed':
      return 'installed';
    case 'discovered':
      return 'found on this machine';
    case 'unavailable':
      return 'unavailable on this platform';
    case 'failed':
      return 'last launch failed';
    default:
      return 'not installed';
  }
}

/// The readiness half of "allowed as a subagent" — the Rust delegation gate
/// applies the same rule, so the UI never offers what the kernel will refuse.
export function canAgentDelegate(r: AgentReadiness): boolean {
  return r === 'ready'
}

export interface AgentDirectorySnapshot {
  agents: AgentDirectoryEntry[];
  defaultAgentId: string;
  bundleCount: number;
  total: number;
}

/**
 * The canonical agent directory (P69.D1). Prefer this over `acpAgents()` when
 * a surface needs to *enumerate* agents (picker, settings list, subagent
 * mix): it is the composed truth including local `agent.toml` bundles.
 */
export async function agentDirectoryList(): Promise<AgentDirectorySnapshot> {
  return nativeCall('agent directory', () => invoke<AgentDirectorySnapshot>('agent_directory_list'));
}

export async function acpAgents(): Promise<HarnessManifest[]> {
  return nativeCall('ACP agent registry', () => invoke<HarnessManifest[]>("acp_agents"));
}

/** F8 — per-agent install state (installed? version? kind?). */
export async function acpInstallStatus(): Promise<Record<string, InstallState>> {
  return nativeCall('ACP install status', () => invoke<Record<string, InstallState>>("acp_install_status"));
}

/** F8 — plan-before-touch: resolve the plan + mint a Guard-2 ticket (or
 * auto-allow). Nothing is downloaded until `acpInstallCommit`. */
export async function acpInstallRequest(agentId: string): Promise<InstallRequest> {
  return nativeCall('ACP install request', () => invoke<InstallRequest>("acp_install_request", { agentId }));
}

/** F8 — the executor half: consume the (mandatory) single-use ticket and
 * install. Both auto-allowed and approved requests commit with a ticket. */
export async function acpInstallCommit(
  agentId: string,
  ticketId: string,
): Promise<{ agentId: string; version: string; kind: string; binaryPath?: string }> {
  return nativeCall('ACP install commit', () => invoke("acp_install_commit", { agentId, ticketId }));
}

/** P69.C12 — wait for the user's Guard-window answer on an `ask` install,
 * then commit only when approved. A rejection or timeout resolves
 * `approved: false` with an honest reason; nothing is installed. */
export async function acpInstallAwait(
  ticketId: string,
  timeoutMs?: number,
): Promise<{ approved: boolean; reason?: string }> {
  return nativeCall('ACP install await consent', () =>
    invoke<{ approved: boolean; reason?: string }>("acp_install_await", { ticketId, timeoutMs }),
  );
}

/** Launch an agent: spawn + ACP handshake → a live handle. May report
 * `authRequired` (the agent needs sign-in before it accepts a session). */
export async function acpLaunch(
  agentId: string,
  cwd: string,
): Promise<AcpHandleInfo> {
  return nativeCall('ACP launch', () => invoke<AcpHandleInfo>("acp_launch", { agentId, cwd }));
}

/** Drive the ACP `authenticate` flow on a live handle. Agent-type methods
 * complete immediately; url-type returns `{pending: true, url}` — open the
 * URL in the system browser, then call `acpAuthenticate` again. */
export async function acpAuthenticate(
  handle: string,
  methodId: string,
): Promise<AuthenticateResult> {
  return nativeCall('ACP authenticate', () => invoke<AuthenticateResult>("acp_authenticate", { handle, methodId }));
}

/** Drive one ACP turn. Returns the stop reason + any minted Guard-2 tickets. */
export async function acpPrompt(
  handle: string,
  sessionId: string,
  text: string,
  handoff?: string,
  refs?: string[],
  bindingId?: string,
): Promise<AcpPromptResult> {
  // The application Session is the owner identity on every turn.  The ACP
  // provider session id is private adapter state and must never be used as
  // this argument. Once claimed, the binding id is sent as an additional
  // ownership assertion; the first turn may omit it while Work/Binding are
  // being established. A blank Session is refused before provider I/O.
  if (!sessionId.trim()) {
    throw new Error('ACP prompt requires a chat id');
  }
  return nativeCall('ACP prompt', () => invoke<AcpPromptResult>("acp_prompt", {
    handle,
    sessionId,
    text,
    ...(handoff ? { handoff } : {}),
    ...(refs?.length ? { refs } : {}),
    ...(bindingId ? { bindingId } : {}),
  }));
}

/** P53.1 — the agent's live slash vocabulary for one ACP handle (from the
 * most recent `available_commands_update`; empty until the agent sends one).
 * Never a hardcoded per-harness table. */
export async function acpSessionCommands(handle: string): Promise<AvailableCommand[]> {
  return nativeCall('ACP session commands', () => invoke<AvailableCommand[]>("acp_session_commands", { handle }));
}

/** The selected external agent's own session configuration vocabulary. */
export async function acpSessionConfigOptions(handle: string): Promise<AcpConfigOption[]> {
  return nativeCall('ACP session config options', () =>
    invoke<AcpConfigOption[]>("acp_session_config_options", { handle }),
  )
}

/** Set one external agent-owned session option; returns the complete updated list. */
export async function acpSessionSetConfigOption(
  handle: string,
  configId: string,
  value: string | boolean,
): Promise<AcpConfigOption[]> {
  return nativeCall('ACP session config option', () =>
    invoke<AcpConfigOption[]>("acp_session_set_config_option", {
      handle,
      configId,
      value,
    }),
  )
}

/** P53.5 — per-session tool observability (one row per ACP turn). Metrics
 * the user can open — never imported into chat context. */
export interface AcpToolLogEntry {
  tsMs: number
  handle: string
  agentId: string
  promptPrefix: string
  stopReason: string
  toolCalls: { toolCallId: string; title: string; kind?: string; status?: string }[]
}

/** P53.5 — read the session's tool log (newest last). Empty until the first
 * ACP turn lands for that session. */
export async function acpToolLog(sessionId: string): Promise<AcpToolLogEntry[]> {
  return nativeCall('ACP tool log', () => invoke<AcpToolLogEntry[]>("acp_tool_log", { sessionId }));
}

/** P53.6 — one installed subagent CLI + its shipped vs user when-to-use. */
export interface SubagentRow {
  agentId: string
  name: string
  defaultWhenToUse: string
  whenToUse: string
  customized: boolean
  enabled: boolean
}

/** P53.6 — installed CLIs only (same `agent_installed` predicate Chief
 * occupancy uses). Empty = none installed on this machine yet. */
export async function chiefSubagents(): Promise<SubagentRow[]> {
  return nativeCall('chief subagents', () => invoke<SubagentRow[]>("chief_subagents"));
}

/** P53.6 — set (or clear, with an empty note) one installed subagent's
 * when-to-use override. Refuses unknown/uninstalled ids fail-closed. */
export async function chiefSubagentSetNote(agentId: string, note: string): Promise<string> {
  return nativeCall('chief subagent note', () => invoke<string>("chief_subagent_set_note", { agentId, note }));
}

/** P53.6 — include/exclude an installed CLI from the Chief delegation mix. */
export async function chiefSubagentSetEnabled(agentId: string, enabled: boolean): Promise<boolean> {
  return nativeCall('chief subagent enabled', () => invoke<boolean>("chief_subagent_set_enabled", { agentId, enabled }));
}

/** P53.6 — enabled installed CLIs, for handoff/delegation context. */
export async function chiefSubagentMix(): Promise<SubagentRow[]> {
  return nativeCall('chief subagent mix', () => invoke<SubagentRow[]>("chief_subagent_mix"));
}

/** F8 — refresh the official ACP registry cache from the CDN (network).
 * Returns the catalog stats. Fails honestly when offline/uncached. */
export async function acpRegistryRefresh(): Promise<{
  version?: string
  agentCount?: number
  fromCache?: boolean
}> {
  return nativeCall('ACP registry refresh', () => invoke("acp_registry_refresh"));
}

async function cancelOwnedAcpTurn(
  handle: string,
  sessionId: string,
  bindingId: string,
): Promise<void> {
  if (!sessionId.trim() || !bindingId.trim()) {
    throw new Error('ACP cancellation requires the owning chat and binding id');
  }
  return nativeCall('ACP cancel', () => invoke("acp_cancel", {
    handle,
    sessionId,
    bindingId,
  }));
}

/**
 * Interrupt the ACP turn owned by one application Session/binding.
 *
 * The identity is mandatory. A one-argument overload remains in the type only
 * for older picker/pause call sites to compile; it fails closed instead of
 * guessing an owner from a bare handle.
 */
export function acpCancel(handle: string, sessionId: string, bindingId: string): Promise<void>;
export function acpCancel(handle: string): Promise<void>;
export async function acpCancel(
  handle: string,
  sessionId?: string,
  bindingId?: string,
): Promise<void> {
  if (sessionId === undefined || bindingId === undefined) {
    throw new Error('ACP cancellation requires the owning chat and binding id');
  }
  return cancelOwnedAcpTurn(handle, sessionId, bindingId);
}

/** Tear an ACP session down (kill + reap). */
export async function acpShutdown(handle: string): Promise<boolean> {
  return nativeCall('ACP shutdown', () => invoke<boolean>("acp_shutdown", { handle }));
}

/** Live ACP handles. */
export async function acpSessions(): Promise<AcpHandleInfo[]> {
  return nativeCall('ACP sessions', () => invoke<AcpHandleInfo[]>("acp_sessions"));
}

/** P38 — the `primary_chief` default (inbuilt | ACP agent id). */
export async function chiefDefaultGet(): Promise<{
  primaryChief: string
  known: string[]
}> {
  return nativeCall('chief default get', () => invoke<{ primaryChief: string; known: string[] }>("chief_default_get"));
}

/** P38 — set the `primary_chief` default. Unknown ids are refused (fail
 * closed — never a silent fallback to the inbuilt engine). */
/**
 * P71.9d — one installed agent's delegation profile. Absent fields mean the
 * spec default (B3: depth ≤2, concurrency ≤6), never a silent zero.
 */
export interface SubagentProfile {
  modelPolicy: string
  role: string
  maySpawn: boolean
  maxChildren: number
  maxDepth: number
  maxConcurrency: number
  workspace: 'shared' | 'isolated'
  budget: number
  /** P63.12 — may occupy the primary slot (absent until the backend carries it). */
  allowAsPrimary?: boolean
  /** P63.12 — may be hired through delegate.spawn (absent until the backend carries it). */
  enableAsSubagent?: boolean
  /** coding, architecture, research, scraping, office. */
  domains?: string[]
  /** Dollar ceiling in cents. 0 means unset. */
  maxCentsPerTurn?: number
  /** Token ceiling. 0 means unset. */
  maxTokensPerTurn?: number
}

/**
 * P71.9d — persist one installed agent's delegation profile
 * (Settings → Subagents). The Rust command fills unspecified fields with
 * their spec defaults and validates the workspace vocabulary.
 */
export async function chiefSubagentSetPolicy(
  agentId: string,
  policy: Partial<SubagentProfile>,
): Promise<SubagentProfile> {
  return nativeCall('chief subagent set policy', () =>
    invoke<SubagentProfile>('chief_subagent_set_policy', {
      agentId,
      modelPolicy: policy.modelPolicy ?? undefined,
      role: policy.role ?? undefined,
      maySpawn: policy.maySpawn ?? undefined,
      maxChildren: policy.maxChildren ?? undefined,
      maxDepth: policy.maxDepth ?? undefined,
      maxConcurrency: policy.maxConcurrency ?? undefined,
      workspace: policy.workspace ?? undefined,
      budget: policy.budget ?? undefined,
      allowAsPrimary: policy.allowAsPrimary ?? undefined,
      enableAsSubagent: policy.enableAsSubagent ?? undefined,
      domains: policy.domains ?? undefined,
      maxCentsPerTurn: policy.maxCentsPerTurn ?? undefined,
      maxTokensPerTurn: policy.maxTokensPerTurn ?? undefined,
    }),
  )
}

export async function chiefDefaultSet(primaryChief: string): Promise<string> {
  return nativeCall('chief default set', () => invoke<string>("chief_default_set", { primaryChief }));
}

export type AgentLifecycleState = 'discover' | 'inspect' | 'import' | 'verify' | 'ready'

export interface AgentVerificationResult {
  agentId: string
  status: 'ready' | 'degraded' | 'unavailable'
  executable?: string
  version?: string
  reason?: string
  verifiedAt: number
}

/** P66.2 — Import a user-specified executable path for an agent. */
export async function acpAgentImport(
  agentId: string,
  binaryPath: string,
): Promise<{
  agentId: string
  status: string
  binaryPath: string
  location?: unknown
  auditSeq?: number
}> {
  return nativeCall('ACP agent import', () =>
    invoke('acp_agent_import', { agentId, binaryPath }),
  )
}

/** P66.2 — Probe and verify an agent executable. */
export async function acpAgentVerify(
  agentId: string,
): Promise<AgentVerificationResult> {
  return nativeCall('ACP agent verify', () =>
    invoke<AgentVerificationResult>('acp_agent_verify', { agentId }),
  )
}

/**
 * Derive the current explicit lifecycle state for an agent.
 */
export function getAgentLifecycleState(
  status: string,
  launchable: boolean,
  verified?: boolean,
): AgentLifecycleState {
  if (status === 'installed' || (status === 'discovered' && launchable && verified)) {
    return 'ready'
  }
  if (status === 'discovered' && launchable) {
    return 'verify'
  }
  if (status === 'discovered') {
    return 'inspect'
  }
  if (status === 'updating') {
    return 'import'
  }
  return 'discover'
}

