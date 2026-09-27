/** P6.6 — coordinator consumer for APP/core-connectors.
 *
 * The desktop sidecar owns orchestration only; credential resolution is still
 * delegated to the host/vault boundary. This bridge makes the existing APP
 * connector registry a real consumer instead of a package-only dependency.
 * It exposes discovery and read/query planning without importing tokens into
 * the sidecar. Mutating connector actions remain Rust/Guard-2 work.
 */
import {
  createDefaultRegistry,
  type ConnectorOrchestrator,
} from "@agentcowork/core-connectors";

let registry: ConnectorOrchestrator | undefined;

function getRegistry(): ConnectorOrchestrator {
  registry ??= createDefaultRegistry();
  return registry;
}

export interface ConnectorCatalogEntry {
  name: string;
  description: string;
  requiresAuth: boolean;
}

export function connectorCatalog(): ConnectorCatalogEntry[] {
  return getRegistry()
    .list()
    .map((adapter) => ({
      name: adapter.name,
      description: adapter.metadataSchema.fields.map((field) => field.description).join("; ") || "Connector adapter",
      requiresAuth: adapter.name.startsWith("composio-") || ["gmail", "google-calendar", "notion", "slack", "github"].includes(adapter.name),
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

/**
 * Plan and execute an authorized read query through core-connectors. The
 * adapter package performs its own authorization check; no credential is
 * accepted as a parameter and no token is returned to the caller.
 */
export async function queryConnectors(query: string, activeNames?: string[]): Promise<unknown[]> {
  const orch = getRegistry();
  const plan = await orch.plan({ text: query }, [], activeNames);
  const results = await orch.execute(plan, { userId: "local", query: { text: query } });
  return results.map((result) => ({
    source: result.source,
    items: result.result.items,
    compressedSnippet: result.compressedSnippet,
  }));
}

export function resetConnectorRegistryForTests(): void {
  registry = undefined;
}
