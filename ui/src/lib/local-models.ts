import { inTauri, invoke } from "./tauri";
import { nativeCall } from './runtime';
import { getLocalItem } from './storage-compat';

export interface HardwareProfile {
  ram_bytes?: number;
  ramBytes?: number;
  cpu_cores?: number;
  cpuCores?: number;
  gpu?: string;
}

export interface HubModel {
  id: string;
  downloads: number;
  likes: number;
  lastModified: string;
  pipelineTag: string;
  tags: string[];
  private: boolean;
}

export interface HubFile {
  path: string;
  size: number;
  type: string;
}

export interface LocalPrefs {
  guardrails: boolean;
  kvOffload: boolean;
  startOnLogin: boolean;
}

const HF = "https://huggingface.co/api/models";
const PREFS_KEY = "agentcowork.local.prefs";

export function formatBytes(n: number): string {
  if (!n || n <= 0) return "—";
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(0)} MB`;
  return `${n} B`;
}

export function formatDownloads(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

export function relativeUpdated(iso: string): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return iso;
  const days = Math.max(0, Math.round((Date.now() - t) / 86_400_000));
  if (days === 0) return "today";
  if (days === 1) return "1 day ago";
  return `${days} days ago`;
}

export function quantFromPath(path: string): string {
  const upper = path.toUpperCase();
  for (const tag of [
    "Q8_0",
    "Q6_K",
    "Q5_K_M",
    "Q5_K_S",
    "Q4_K_M",
    "Q4_K_S",
    "Q4_0",
    "Q3_K_M",
    "IQ4_XS",
    "F16",
    "BF16",
  ]) {
    if (upper.includes(tag)) return tag;
  }
  return path.split("/").pop() ?? path;
}

export function ramBytes(hw: HardwareProfile | null): number {
  return hw?.ram_bytes ?? hw?.ramBytes ?? 0;
}

export function cpuCores(hw: HardwareProfile | null): number {
  return hw?.cpu_cores ?? hw?.cpuCores ?? 0;
}

type HubSort = "downloads" | "likes" | "lastModified";

/** Live Hugging Face Hub — no baked model names. */
export async function searchHub(
  query: string,
  sort: HubSort = "downloads",
  limit = 40,
): Promise<HubModel[]> {
  const params = new URLSearchParams({
    filter: "gguf",
    sort,
    direction: "-1",
    limit: String(limit),
  });
  const q = query.trim();
  if (q) params.set("search", q);
  const res = await fetch(`${HF}?${params.toString()}`);
  if (!res.ok) throw new Error(`Hugging Face Hub ${res.status}`);
  const rows = (await res.json()) as Array<{
    id: string;
    downloads?: number;
    likes?: number;
    lastModified?: string;
    pipeline_tag?: string;
    tags?: string[];
    private?: boolean;
  }>;
  return rows.map((m) => ({
    id: m.id,
    downloads: m.downloads ?? 0,
    likes: m.likes ?? 0,
    lastModified: m.lastModified ?? "",
    pipelineTag: m.pipeline_tag ?? "",
    tags: m.tags ?? [],
    private: m.private ?? false,
  }));
}

export async function listHubFiles(repo: string): Promise<HubFile[]> {
  const res = await fetch(`${HF}/${encodeURIComponent(repo)}/tree/main`);
  if (!res.ok) throw new Error(`Hugging Face tree ${res.status}`);
  const rows = (await res.json()) as Array<{ path?: string; size?: number; type?: string }>;
  return rows
    .filter((r) => (r.path ?? "").toLowerCase().endsWith(".gguf") || (r.path ?? "").toLowerCase().endsWith(".safetensors"))
    .map((r) => ({
      path: r.path ?? "",
      size: r.size ?? 0,
      type: r.type ?? "file",
    }));
}

export function hubCaps(m: HubModel) {
  const t = m.tags.map((x) => x.toLowerCase());
  return {
    vision: t.some((x) => x.includes("vision") || x.includes("image")),
    toolUse: t.some((x) => x.includes("tool") || x.includes("function")),
    reasoning: t.some((x) => x.includes("reason") || x.includes("r1")),
    gguf: t.some((x) => x.includes("gguf")),
    mlx: t.some((x) => x.includes("mlx")),
  };
}

export async function getHardware(): Promise<HardwareProfile | null> {
  if (!inTauri()) return null;
  return nativeCall('hardware profile', () => invoke("local_hardware"));
}

export function getLocalPrefs(): LocalPrefs {
  try {
    // DEC-053: legacy `everyaios.*` key honored + promoted once.
    const raw = getLocalItem(PREFS_KEY);
    if (raw) return { ...{ guardrails: false, kvOffload: true, startOnLogin: true }, ...JSON.parse(raw) };
  } catch {
    /* ignore */
  }
  return { guardrails: false, kvOffload: true, startOnLogin: true };
}

export function setLocalPrefs(prefs: LocalPrefs): void {
  localStorage.setItem(PREFS_KEY, JSON.stringify(prefs));
}

// ---------------------------------------------------------------------------
// Runtime inventory bridge
// ---------------------------------------------------------------------------

/**
 * A runtime is an environment resource, not an agent model.  The desktop may
 * observe an existing process, attach a remote endpoint, or own a managed
 * process whose start/stop lifecycle it can control.  These fields are kept
 * deliberately small at the UI boundary so a future Rust projection can add
 * diagnostics without making the panel infer ownership or health.
 */
export type RuntimeOwnership = "managed" | "external" | "remote";

/** `observed` is intentionally separate from `healthy`: a visible endpoint is
 * not a successful health probe. */
export type RuntimeHealth =
  | "observed"
  | "healthy"
  | "degraded"
  | "stopped"
  | "starting"
  | "down"
  | "unavailable"
  | "failed"
  | "unknown";

export type RuntimeCompatibility = "supported" | "not_supported" | "unknown";

export interface RuntimeModelObservation {
  id: string;
  name: string;
  contextWindow?: number;
  sizeBytes?: number;
  source?: string;
  observedAt?: string;
}

export interface RuntimeAgentCompatibility {
  status: RuntimeCompatibility;
  reason: string;
  /** Binding id when the runtime projection carries agent-specific evidence. */
  agentId?: string;
  /** Maximum control surface advertised for this binding. */
  control?: AgentModelControlLevel;
  /** True/false when a probe proved usability; undefined means unknown. */
  usable?: boolean;
}

export interface RuntimeInventoryRow {
  id: string;
  kind: string;
  endpoint?: string;
  version?: string;
  protocol?: string;
  ownership: RuntimeOwnership;
  health: RuntimeHealth;
  /** A health probe result is different from an inventory observation. */
  healthObserved: boolean;
  reason?: string;
  lastError?: string;
  observedAt?: string;
  models: RuntimeModelObservation[];
  /** True when the payload itself contained a models observation. */
  modelsObserved: boolean;
  /** The compatibility row selected for the current projection, if any. */
  agentCompatibility?: RuntimeAgentCompatibility;
  /** All binding-specific compatibility rows returned by the native inventory. */
  agentCompatibilityRows: RuntimeAgentCompatibility[];
}

export interface RuntimeStartResult {
  ok: boolean;
  started: boolean;
  alreadyManaged?: boolean;
  health: RuntimeHealth;
  reason?: string;
  runtime?: RuntimeInventoryRow;
}

export interface RuntimeStopResult {
  ok: boolean;
  stopped: boolean;
  health: RuntimeHealth;
  reason?: string;
  runtime?: RuntimeInventoryRow;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function firstString(row: Record<string, unknown>, keys: string[]): string | undefined {
  for (const key of keys) {
    const value = row[key];
    if (typeof value === "string" && value.trim()) return value.trim();
    if (typeof value === "number" && Number.isFinite(value)) return String(value);
  }
  return undefined;
}

function firstBoolean(row: Record<string, unknown>, keys: string[]): boolean | undefined {
  for (const key of keys) {
    const value = row[key];
    if (typeof value === "boolean") return value;
  }
  return undefined;
}

function firstNumber(row: Record<string, unknown>, keys: string[]): number | undefined {
  for (const key of keys) {
    const value = row[key];
    if (typeof value === "number" && Number.isFinite(value)) return value;
  }
  return undefined;
}

function normalizeOwnership(value: unknown): RuntimeOwnership {
  const text = typeof value === "string" ? value.toLowerCase().replace(/[-\s]/g, "_") : "";
  // DEC-053: persisted ownership spellings — the legacy tokens stay so stored runtimes keep parsing as managed.
  if (text === "managed" || text === "managed_by_everyaios" || text === "everyaios" || text === "owned" || text === "host_managed") return "managed";
  if (text === "remote" || text === "hosted" || text === "cloud" || text === "external_remote") return "remote";
  return "external";
}

function normalizeHealth(value: unknown, hasEndpoint: boolean): {
  health: RuntimeHealth;
  observed: boolean;
} {
  const nested = asRecord(value);
  if (nested) {
    const nestedValue = firstString(nested, ["status", "state", "health", "healthState", "health_state"]);
    if (nestedValue) return normalizeHealth(nestedValue, hasEndpoint);
  }
  const text = typeof value === "string" ? value.toLowerCase().replace(/[-\s]/g, "_") : "";
  switch (text) {
    case "healthy":
    case "health":
    case "ready":
    case "live":
    case "up":
      return { health: "healthy", observed: true };
    case "degraded":
    case "partial":
      return { health: "degraded", observed: true };
    case "stopped":
    case "inactive":
      return { health: "stopped", observed: true };
    case "starting":
    case "launching":
      return { health: "starting", observed: true };
    case "unavailable":
    case "unsupported":
      return { health: "unavailable", observed: true };
    case "down":
      return { health: "down", observed: true };
    case "failed":
    case "error":
      return { health: "failed", observed: true };
    case "observed":
    case "detected":
    case "discovered":
      return { health: "observed", observed: false };
    case "unknown":
      return { health: hasEndpoint ? "observed" : "unknown", observed: false };
    default:
      // Inventory without an explicit health result is an observation, never a
      // green readiness claim.  This is the important distinction for an
      // externally supplied endpoint.
      return { health: hasEndpoint ? "observed" : "unknown", observed: false };
  }
}

function normalizeCompatibility(value: unknown, reason?: string): RuntimeAgentCompatibility | undefined {
  if (Array.isArray(value)) {
    return value
      .map((row) => normalizeCompatibility(row))
      .find((row): row is RuntimeAgentCompatibility => row !== undefined);
  }
  if (typeof value === "string") {
    const text = value.toLowerCase().replace(/[-\s]/g, "_");
    if (text === "supported" || text === "yes" || text === "compatible") {
      return { status: "supported", reason: reason ?? "The agent advertised compatibility." };
    }
    if (text === "not_supported" || text === "unsupported" || text === "no") {
      return { status: "not_supported", reason: reason ?? "The agent advertised no compatibility." };
    }
    if (text === "unknown" || text === "unverified") {
      return { status: "unknown", reason: reason ?? "The agent has not confirmed compatibility." };
    }
  }
  const record = asRecord(value);
  if (!record) return undefined;
  const usable = firstBoolean(record, ["usable", "supported", "canUse", "can_use"]);
  const rawStatus = firstString(record, ["status", "level", "compatibility"]);
  const status: RuntimeCompatibility =
    usable === true || rawStatus === "supported" || rawStatus === "yes" || rawStatus === "compatible"
      ? "supported"
      : usable === false || rawStatus === "not_supported" || rawStatus === "unsupported" || rawStatus === "no"
        ? "not_supported"
        : "unknown";
  const control: AgentModelControlLevel | undefined =
    parseControlLevel(firstString(record, ["control", "controlLevel", "control_level"])) ?? undefined;
  return {
    status,
    reason:
      reason ??
      firstString(record, ["reason", "detail", "message"]) ??
      (status === "supported"
        ? "The agent reported this runtime as usable."
        : status === "not_supported"
          ? "The agent reported this runtime as not usable."
          : "The agent has not confirmed compatibility."),
    agentId: firstString(record, ["agentId", "agent_id", "id"]),
    control,
    usable,
  };
}

function normalizeRuntimeModels(value: unknown): RuntimeModelObservation[] {
  const rows = Array.isArray(value)
    ? value
    : (() => {
        const record = asRecord(value);
        if (!record) return [];
        const candidate = record.models ?? record.modelIds ?? record.items;
        return Array.isArray(candidate) ? candidate : [];
      })();
  return rows
    .map((raw): RuntimeModelObservation | null => {
      if (typeof raw === "string") return { id: raw, name: raw };
      const row = asRecord(raw);
      if (!row) return null;
      const id = firstString(row, ["id", "model", "name", "modelId"]);
      if (!id) return null;
      return {
        id,
        name: firstString(row, ["name", "label", "model", "id"]) ?? id,
        contextWindow: firstNumber(row, ["contextWindow", "context_window", "context", "ctx"]),
        sizeBytes: firstNumber(row, ["sizeBytes", "size_bytes", "size"]),
        source: firstString(row, ["source", "runtime", "provider"]),
        observedAt: firstString(row, ["observedAt", "observed_at"]),
      };
    })
    .filter((row): row is RuntimeModelObservation => row !== null);
}

function normalizeRuntimeRow(raw: unknown): RuntimeInventoryRow | null {
  const row = asRecord(raw);
  if (!row) return null;
  const id = firstString(row, ["id", "runtimeId", "runtime_id", "name"]);
  if (!id) return null;
  const endpoint = firstString(row, ["endpoint", "baseUrl", "base_url", "url"]);
  const explicitHealth = row.health ?? row.healthState ?? row.health_state;
  const rawHealth = explicitHealth ?? row.probeState ?? row.probe_state ?? row.healthStatus ?? row.health_status ?? row.state ?? row.status;
  const health = normalizeHealth(rawHealth, Boolean(endpoint));
  // `status: ready` is common install metadata, not a health probe. Without an
  // explicit health field it remains an observation rather than a green claim.
  if (
    explicitHealth === undefined &&
    typeof row.status === 'string' &&
    ['ready', 'live', 'up'].includes(row.status.toLowerCase())
  ) {
    health.health = endpoint ? 'observed' : 'unknown';
    health.observed = false;
  }
  const reason = firstString(row, ["healthReason", "health_reason", "reason", "detail"]);
  const rawModels = row.models ?? row.modelIds ?? row.model_ids;
  const explicitModelsObserved = firstBoolean(row, ["modelsObserved", "models_observed"]);
  const hasModels = explicitModelsObserved ?? (Array.isArray(rawModels) ? rawModels.length > 0 : rawModels !== undefined);
  const rawCompatibility = row.agentCompatibility ?? row.agent_compatibility ?? row.compatibility;
  const agentCompatibilityRows = Array.isArray(rawCompatibility)
    ? rawCompatibility
        .map((item) => normalizeCompatibility(item))
        .filter((item): item is RuntimeAgentCompatibility => item !== undefined)
    : (() => {
        const item = normalizeCompatibility(rawCompatibility, firstString(row, ["compatibilityReason", "compatibility_reason"]));
        return item ? [item] : [];
      })();
  return {
    id,
    kind: firstString(row, ["kind", "type", "runtimeKind", "runtime_kind"]) ?? "Local runtime",
    endpoint,
    version: firstString(row, ["version", "runtimeVersion", "runtime_version"]),
    protocol: firstString(row, ["protocol", "transport", "api"]),
    ownership: normalizeOwnership(
      row.ownership ?? row.ownershipKind ?? row.ownership_kind ?? row.owner ?? (row.managed === true ? 'managed' : row.managed === false ? 'external' : undefined),
    ),
    health: health.health,
    healthObserved: health.observed,
    reason,
    lastError: firstString(row, ["lastError", "last_error", "error"]),
    observedAt: firstString(row, ["observedAt", "observed_at", "seenAt", "seen_at"]),
    models: normalizeRuntimeModels(rawModels),
    modelsObserved: hasModels,
    agentCompatibility: agentCompatibilityRows[0],
    agentCompatibilityRows,
  };
}

/** Normalize either the command's array shape or `{ runtimes: [...] }`. */
export function normalizeRuntimeInventory(value: unknown): RuntimeInventoryRow[] {
  const rows = Array.isArray(value)
    ? value
    : (() => {
        const record = asRecord(value);
        if (!record) return [];
        const candidate = record.runtimes ?? record.entries ?? record.items;
        return Array.isArray(candidate) ? candidate : [];
      })();
  return rows
    .map(normalizeRuntimeRow)
    .filter((row): row is RuntimeInventoryRow => row !== null);
}

function normalizeMutationResult(value: unknown, action: 'start' | 'stop'): RuntimeStartResult | RuntimeStopResult {
  const row = normalizeRuntimeRow(value);
  const record = asRecord(value);
  const explicitHealth = record?.health ?? record?.healthState ?? record?.health_state;
  const rawHealth = explicitHealth ?? record?.probeState ?? record?.probe_state ?? record?.healthStatus ?? record?.health_status ?? record?.state ?? record?.status;
  const normalized = normalizeHealth(rawHealth, Boolean(row?.endpoint));
  if (
    explicitHealth === undefined &&
    typeof record?.status === 'string' &&
    ['ready', 'live', 'up'].includes(record.status.toLowerCase())
  ) {
    normalized.health = row?.endpoint ? 'observed' : 'unknown';
    normalized.observed = false;
  }
  const started = action === 'start'
    ? (firstBoolean(record ?? {}, ["started", "ok"]) ?? true)
    : (firstBoolean(record ?? {}, ["stopped", "ok"]) ?? true);
  const common = {
    ok: firstBoolean(record ?? {}, ["ok"]) ?? started,
    health: normalized.health,
    reason: firstString(record ?? {}, ["reason", "detail", "message", "lastError", "last_error"]),
    runtime: row ?? undefined,
  };
  return action === 'start'
    ? {
        ...common,
        started,
        alreadyManaged: firstBoolean(record ?? {}, ['alreadyManaged', 'already_managed']) ?? false,
      }
    : { ...common, stopped: started };
}

/** Read the current local runtime inventory from the native projection. */
export async function listRuntimeInventory(): Promise<RuntimeInventoryRow[]> {
  if (!inTauri()) return [];
  const value = await nativeCall('local runtime inventory', () =>
    invoke<unknown>('runtime_inventory_list'),
  );
  return normalizeRuntimeInventory(value);
}

/** Start a managed runtime. Ownership is checked before crossing IPC. */
export async function startManagedRuntime(
  runtime: string | Pick<RuntimeInventoryRow, 'id' | 'ownership'>,
  options: { runtimeKind?: string; modelId?: string } = {},
): Promise<RuntimeStartResult> {
  if (!inTauri()) throw new Error('Starting a local runtime requires the Tauri desktop shell');
  const id = typeof runtime === 'string' ? runtime : runtime.id;
  if (typeof runtime !== 'string' && runtime.ownership !== 'managed') {
    throw new Error('Only a Managed runtime can be started by AgentCowork');
  }
  const args: Record<string, unknown> = { runtimeId: id };
  if (options.runtimeKind) args.runtimeKind = options.runtimeKind;
  if (options.modelId) args.modelId = options.modelId;
  const value = await nativeCall('start managed local runtime', () =>
    invoke<unknown>('runtime_start', args),
  );
  return normalizeMutationResult(value, 'start') as RuntimeStartResult;
}

/** Stop a managed runtime. Ownership is checked before crossing IPC. */
export async function stopManagedRuntime(
  runtime: string | Pick<RuntimeInventoryRow, 'id' | 'ownership'>,
): Promise<RuntimeStopResult> {
  if (!inTauri()) throw new Error('Stopping a local runtime requires the Tauri desktop shell');
  const id = typeof runtime === 'string' ? runtime : runtime.id;
  if (typeof runtime !== 'string' && runtime.ownership !== 'managed') {
    throw new Error('Only a Managed runtime can be stopped by AgentCowork');
  }
  const value = await nativeCall('stop managed local runtime', () =>
    invoke<unknown>('runtime_stop', { runtimeId: id }),
  );
  return normalizeMutationResult(value, 'stop') as RuntimeStopResult;
}

/** Read model observations for one runtime. This is inventory, not a model picker. */
export async function listRuntimeModels(runtimeId: string): Promise<RuntimeModelObservation[]> {
  if (!inTauri()) return [];
  const value = await nativeCall('local runtime models', () =>
    invoke<unknown>('runtime_models', { runtimeId }),
  );
  return normalizeRuntimeModels(value);
}

// ---------------------------------------------------------------------------
// Agent handoff bridge
// ---------------------------------------------------------------------------

export type AgentModelControlLevel =
  | 'native_only'
  | 'session_config'
  | 'launch_override'
  | 'unknown';

function parseControlLevel(value: unknown): AgentModelControlLevel | null {
  if (typeof value !== 'string') return null;
  const text = value
    .replace(/([a-z])([A-Z])/g, '$1_$2')
    .toLowerCase()
    .replace(/[-\s]/g, '_');
  if (text === 'native_only' || text === 'session_config' || text === 'launch_override' || text === 'unknown') {
    return text;
  }
  return null;
}

/**
 * Resolve only an advertised control level. A model option is enough to expose
 * the agent's own session vocabulary; the absence of one is not evidence that
 * AgentCowork may choose a model.
 */
export function resolveAgentModelControlLevel(
  agent: unknown,
  configOptions: readonly import('./acp').AcpConfigOption[] = [],
): AgentModelControlLevel {
  const record = asRecord(agent);
  if (record) {
    const direct =
      parseControlLevel(record.controlLevel) ??
      parseControlLevel(record.control_level) ??
      parseControlLevel(record.modelControlLevel) ??
      parseControlLevel(record.model_control_level) ??
      parseControlLevel(record.modelControl) ??
      parseControlLevel(record.model_control) ??
      parseControlLevel(record.control);
    if (direct) return direct;
    const nested = asRecord(record.modelControl ?? record.model_control);
    const nestedLevel = nested && parseControlLevel(nested.level ?? nested.controlLevel ?? nested.control_level);
    if (nestedLevel) return nestedLevel;
    const controlRecord = asRecord(record.control);
    const controlLevel = controlRecord && parseControlLevel(controlRecord.level ?? controlRecord.controlLevel ?? controlRecord.control_level);
    if (controlLevel) return controlLevel;
    if (record.nativeOnly === true || record.native_only === true) return 'native_only';
  }
  if (configOptions.length > 0) return 'session_config';
  return 'unknown';
}

export interface AgentConfigOptionsResult {
  options: import('./acp').AcpConfigOption[];
  control?: AgentModelControlLevel;
  reason?: string;
  state?: string;
  agentConfirmed?: boolean;
}

function normalizeAgentConfigResult(value: unknown): AgentConfigOptionsResult {
  if (Array.isArray(value)) {
    return {
      options: value as import('./acp').AcpConfigOption[],
      control: value.length > 0 ? 'session_config' : 'native_only',
    };
  }
  const record = asRecord(value);
  const rawOptions = record?.options ?? record?.configOptions ?? [];
  const options = Array.isArray(rawOptions)
    ? (rawOptions as import('./acp').AcpConfigOption[])
    : [];
  return {
    options,
    control: parseControlLevel(record?.control) ?? undefined,
    reason: firstString(record ?? {}, ['reason', 'detail', 'message']),
    state: firstString(record ?? {}, ['state']),
    agentConfirmed: firstBoolean(record ?? {}, ['agentConfirmed', 'agent_confirmed']),
  };
}

/** Read the bound agent's own ACP config vocabulary (never a host model list). */
export async function getBoundAgentConfigOptions(
  handle: string,
): Promise<AgentConfigOptionsResult> {
  if (!inTauri()) throw new Error('Agent configuration requires the Tauri desktop shell');
  const value = await nativeCall('bound agent config options', () =>
    invoke<unknown>('acp_config_options', { handle }),
  );
  return normalizeAgentConfigResult(value);
}

/** Request one agent-owned config option; the result remains requested until ACP confirms. */
export async function setBoundAgentConfigOption(
  handle: string,
  configId: string,
  value: string | boolean,
): Promise<AgentConfigOptionsResult> {
  if (!inTauri()) throw new Error('Agent configuration requires the Tauri desktop shell');
  const result = await nativeCall('set bound agent config option', () =>
    invoke<unknown>('acp_set_session_config_option', {
      handle,
      configId,
      value,
    }),
  );
  return normalizeAgentConfigResult(result);
}

/**
 * Request a credential-free endpoint/model binding for the next agent launch.
 * The existing backend command owns persistence and agent compatibility; the
 * panel labels the result as requested because the agent has not confirmed it.
 */
export async function requestAgentLaunchOverride(input: {
  agentId: string;
  endpoint?: string;
  model?: string;
  runtimeId?: string;
}): Promise<unknown> {
  if (!inTauri()) throw new Error('Launch overrides require the Tauri desktop shell');
  return nativeCall('request agent launch override', () =>
    invoke<unknown>('agent_backend_set', {
      agentId: input.agentId,
      provider: input.runtimeId ?? 'local',
      model: input.model ?? null,
      baseUrl: input.endpoint ?? null,
      useVaultKey: false,
    }),
  );
}