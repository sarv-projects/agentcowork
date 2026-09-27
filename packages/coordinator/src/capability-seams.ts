/**
 * P30.10 — capability seams SD/Provider/Consumer (deepseek-harness Extension
 * ABI pattern, doc 83 §1): formalize the extension contract around the
 * Service-Definition / Provider / Consumer triad, with **reversible
 * registration** — a skill/plugin registration unwinds on unload. This is the
 * coordinator-side ABI the extension registry (I6) and `agentcowork-mcp`
 * manager share.
 *
 * - `ServiceDefinition` — declares a capability (id + version + contract).
 * - `Provider` — a registered implementation of a definition.
 * - `Consumer` — depends on a service; unregistering the provider notifies
 *   (and refuses further use by) its consumers.
 */

export interface ServiceDefinition {
  /** Stable capability id, e.g. "office.docx.render". */
  id: string;
  /** Semver-ish version the implementation must match. */
  version: string;
  /** One-line contract description. */
  description: string;
}

/** A registered provider (the implementation + its registration handle). */
export interface ProviderHandle {
  definitionId: string;
  implementationId: string;
  /** Callers resolve a service through this. */
  invoke: (args: unknown) => Promise<unknown>;
}

export interface ConsumerBinding {
  consumerId: string;
  definitionId: string;
}

/** Unload result — what unwound. */
export interface UnloadReport {
  unregisteredProviders: string[];
  notifiedConsumers: string[];
}

export class CapabilityError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CapabilityError";
  }
}

/**
 * The seam registry: definitions declare, providers register, consumers bind.
 * Registration is reversible: `unregister(implementationId)` removes the
 * provider AND notifies every bound consumer (they must stop using it).
 */
export class CapabilitySeamRegistry {
  private definitions = new Map<string, ServiceDefinition>();
  private providers = new Map<string, ProviderHandle>();
  private consumers = new Map<string, ConsumerBinding>();

  /** Declare a service definition (idempotent, re-declare updates). */
  declare(def: ServiceDefinition): void {
    this.definitions.set(def.id, def);
  }

  hasDefinition(id: string): boolean {
    return this.definitions.has(id);
  }

  /** Register a provider implementation. Refuses undeclared services. */
  register(
    defId: string,
    implementationId: string,
    invoke: (args: unknown) => Promise<unknown>,
  ): ProviderHandle {
    if (!this.definitions.has(defId)) {
      throw new CapabilityError(`service '${defId}' is not declared`);
    }
    const handle: ProviderHandle = { definitionId: defId, implementationId, invoke };
    this.providers.set(implementationId, handle);
    return handle;
  }

  /** Bind a consumer to a service. Refuses when no provider is registered. */
  bind(consumerId: string, defId: string): ConsumerBinding {
    if (!this.providers.has(providerFor(this.providers, defId))) {
      throw new CapabilityError(`no provider for service '${defId}'`);
    }
    const binding: ConsumerBinding = { consumerId, definitionId: defId };
    this.consumers.set(consumerKey(consumerId, defId), binding);
    return binding;
  }

  /** Resolve + invoke the provider for a service. */
  async resolve(defId: string, args: unknown): Promise<unknown> {
    const provider = findProvider(this.providers, defId);
    if (!provider) throw new CapabilityError(`no provider for service '${defId}'`);
    return provider.invoke(args);
  }

  /** Reverse a provider registration — unwinds its consumers too (I6). */
  unregister(implementationId: string): UnloadReport {
    const handle = this.providers.get(implementationId);
    if (!handle) return { unregisteredProviders: [], notifiedConsumers: [] };
    this.providers.delete(implementationId);
    const notified: string[] = [];
    for (const [key, binding] of [...this.consumers.entries()]) {
      if (binding.definitionId === handle.definitionId) {
        this.consumers.delete(key);
        notified.push(binding.consumerId);
      }
    }
    return { unregisteredProviders: [implementationId], notifiedConsumers: notified };
  }

  providerCount(): number {
    return this.providers.size;
  }
}

function providerFor(
  providers: Map<string, ProviderHandle>,
  defId: string,
): string {
  for (const [id, h] of providers) {
    if (h.definitionId === defId) return id;
  }
  return "";
}

function findProvider(
  providers: Map<string, ProviderHandle>,
  defId: string,
): ProviderHandle | undefined {
  for (const h of providers.values()) {
    if (h.definitionId === defId) return h;
  }
  return undefined;
}

function consumerKey(consumerId: string, defId: string): string {
  return `${consumerId}::${defId}`;
}
