// P31.10 — agent-registry bridge. The Rust `agentcowork-agents` crate owns the
// durable store (`~/.agentcowork/agents/<id>/agent.toml`); these wrappers call
// the Tauri commands so the builder panel reads/writes the real registry.
// Outside the Tauri webview (`inTauri()` false) they fall back to the
// browser-local mirror (same shape, demo state only).

import { inTauri, invoke } from "./tauri";
import { nativeCall } from './runtime';
import { getLocalItem } from './storage-compat';

/** One registry row (light meta — never the full bundle). */
export interface RegisteredAgent {
  id: string
  name: string
  emoji: string
  engine: string
  disabled: boolean
  description: string
}

export interface RegistryList {
  agents: RegisteredAgent[]
  root: string
}

/** List every registered agent (light rows). */
export async function agentRegistryList(): Promise<RegistryList> {
  if (!inTauri()) return demoList()
  return nativeCall('agent registry list', () => invoke<RegistryList>("agent_registry_list"))
}

/** Save a bundle (agent.toml string) into the registry → the derived id. */
export async function agentRegistrySave(agentToml: string): Promise<string> {
  if (!inTauri()) {
    // Demo path: mirror what the Rust side would do (id = slug of the name).
    // Parsing the TOML client-side would need a dep; the builder panel keeps
    // its own bundle object and mirrors it in demo mode (see panel).
    return "demo"
  }
  return nativeCall('agent registry save', () => invoke<string>("agent_registry_save", { agentToml }))
}

/** Fetch one bundle as agent.toml (edit / export path). */
export async function agentRegistryGet(id: string): Promise<string> {
  if (!inTauri()) throw new Error("registry_get: demo path has no TOML store")
  return nativeCall('agent registry get', () => invoke<string>("agent_registry_get", { id }))
}

/** Remove an agent (+ its per-agent asset dir) from the registry. */
export async function agentRegistryRemove(id: string): Promise<void> {
  if (!inTauri()) return
  return nativeCall('agent registry remove', () => invoke<void>("agent_registry_remove", { id }))
}

/** Duplicate an agent under a new name. */
export async function agentRegistryDuplicate(
  id: string,
  newName: string,
): Promise<string> {
  if (!inTauri()) return "demo"
  return nativeCall('agent registry duplicate', () => invoke<string>("agent_registry_duplicate", { id, newName }))
}

/** Toggle an agent's disabled flag. */
export async function agentRegistrySetDisabled(
  id: string,
  disabled: boolean,
): Promise<void> {
  if (!inTauri()) return
  return nativeCall('agent registry toggle', () => invoke<void>("agent_registry_set_disabled", { id, disabled }))
}

// --- demo fallback (browser preview, no Rust side) --------------------------

interface DemoAgent {
  id: string
  name: string
  emoji: string
  engine: string
  disabled: boolean
  description: string
}

function demoLoad(): DemoAgent[] {
  try {
    // DEC-053: legacy `everyaios.*` key honored + promoted once.
    const raw = getLocalItem("agentcowork.agents.demo")
    return raw ? (JSON.parse(raw) as DemoAgent[]) : []
  } catch {
    return []
  }
}

async function demoList(): Promise<RegistryList> {
  return { agents: demoLoad(), root: "~/.agentcowork/agents (demo)" }
}