import { getProviderById } from './registry.js';
import type {
  ConnectedProvider,
  KeyValueStore,
  StoredProviderRecord,
} from './types.js';

const VAULT_STORAGE_KEY = 'byok.providers.v1';

/** Simple promise-chain mutex to prevent lost-update race on read-modify-write operations. */
class VaultMutex {
  private chain: Promise<void> = Promise.resolve();
  async acquire<T>(fn: () => Promise<T>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      this.chain = this.chain.then(async () => {
        try {
          resolve(await fn());
        } catch (e) {
          reject(e);
        }
      });
    });
  }
}

/**
 * P69.C4 / P69.D4 — **availability + handle facade only.**
 *
 * Provider credentials live **only** in the Rust vault (`agentcowork-vault`);
 * the sidecar never holds them (AGENTS.md §15, `ARCH/CORE.md` I10). This class
 * used to seal/unseal API keys with `@agentcowork/core-security`, which made a
 * TypeScript package a second credential store — the single most severe live
 * violation of the strongest invariant.
 *
 * The surface is now deliberately credential-free:
 * - `save` records provider metadata + an **opaque `keyRef`** (a vault handle
 *   like `vault:<provider>:<n>`), never a key;
 * - `hasKey`/`keyRef` answer availability questions;
 * - there is **no** `getApiKey` — no TS call site can obtain raw key material,
 *   so none can leak it into a log, a request body, or a persisted record.
 *
 * Passing raw key material (a legacy `apiKey` field) is a hard error: the
 * caller goes through the Rust vault commands (`vault_key_add`) instead.
 */
export class ProviderVault {
  private readonly mutex = new VaultMutex();

  constructor(private readonly store: KeyValueStore) {}

  async list(): Promise<ConnectedProvider[]> {
    const records = await this.readRecords();
    return records
      .map((record) => this.toConnectedProvider(record))
      .filter((provider): provider is ConnectedProvider => provider != null);
  }

  async getActive(): Promise<ConnectedProvider[]> {
    const providers = await this.list();
    return providers.filter((provider) => provider.isActive);
  }

  /**
   * Record provider metadata + an opaque vault handle.
   *
   * `keyRef` is issued by the Rust vault; this class never mints, seals, or
   * reads a credential. Key *validation* also lives Rust-side now
   * (`provider_probe` → `Broker`), because validating a key means sending it —
   * a credential-bearing egress that must not originate in TypeScript.
   */
  async save(input: {
    id: string;
    /** Opaque vault handle (`vault:<provider>:<n>`); never a raw key. */
    keyRef?: string;
    model?: string;
    isActive?: boolean;
    /** Custom endpoint override (Azure/Databricks/Snowflake/Bedrock/Vertex). */
    baseUrl?: string;
    fetchImpl?: typeof fetch;
  }): Promise<ConnectedProvider> {
    // Fail loud on the removed credential path rather than silently ignoring
    // it (a silent ignore would let a caller believe it saved a key).
    if (Object.prototype.hasOwnProperty.call(input, 'apiKey')) {
      throw new Error(
        'ProviderVault no longer accepts key material (P69.C4): pass an opaque keyRef issued by the Rust vault (vault_key_add). Provider keys live only in agentcowork-vault.',
      );
    }
    const catalog = getProviderById(input.id);
    if (!catalog) {
      throw new Error(`Unknown provider: ${input.id}`);
    }

    const model = input.model?.trim() || catalog.defaultModel;
    const baseUrlOverride = input.baseUrl?.trim();
    const keyRef = input.keyRef?.trim() || undefined;

    // Validate outside mutex (any network work belongs to Rust), then lock for
    // read-modify-write only.
    const connected = await this.mutex.acquire(async () => {
      const records = await this.readRecords();
      const existing = records.find((r) => r.id === input.id);
      const nextRecord: StoredProviderRecord = {
        id: input.id,
        model,
        // #25: re-save must NOT silently re-activate a deactivated provider
        // (the old `?? true` did). Preserve the current state when unspecified.
        isActive: input.isActive ?? existing?.isActive ?? true,
        // Preserve an existing handle unless the caller explicitly replaces it.
        ...(keyRef ? { keyRef } : existing?.keyRef ? { keyRef: existing.keyRef } : {}),
        // Preserve original connectedAt on re-save; only set on first connect.
        connectedAt: existing?.connectedAt ?? new Date().toISOString(),
        ...(baseUrlOverride ? { baseUrl: baseUrlOverride } : {}),
      };
      const withoutCurrent = records.filter((record) => record.id !== input.id);
      await this.writeRecords([...withoutCurrent, nextRecord]);
      const result = this.toConnectedProvider(nextRecord);
      if (!result) {
        throw new Error(`Failed to persist provider: ${input.id}`);
      }
      return result;
    });
    return connected;
  }

  async remove(id: string): Promise<void> {
    await this.mutex.acquire(async () => {
      const records = await this.readRecords();
      await this.writeRecords(records.filter((record) => record.id !== id));
    });
  }

  async setActive(id: string, isActive: boolean): Promise<ConnectedProvider> {
    return this.mutex.acquire(async () => {
      const records = await this.readRecords();
      const record = records.find((entry) => entry.id === id);
      if (!record) {
        throw new Error(`Provider not connected: ${id}`);
      }
      const updated: StoredProviderRecord = { ...record, isActive };
      const nextRecords = records.map((entry) => (entry.id === id ? updated : entry));
      await this.writeRecords(nextRecords);
      const connected = this.toConnectedProvider(updated);
      if (!connected) {
        throw new Error(`Provider not connected: ${id}`);
      }
      return connected;
    });
  }

  /** Whether a vault handle is recorded for this provider (availability only). */
  async hasKey(id: string): Promise<boolean> {
    return this.keyRef(id).then((ref) => ref != null);
  }

  /** The opaque vault handle, or null. Never key material. */
  async keyRef(id: string): Promise<string | null> {
    return this.mutex.acquire(async () => {
      const records = await this.readRecords();
      const record = records.find((entry) => entry.id === id);
      return record?.keyRef ?? null;
    });
  }

  private toConnectedProvider(record: StoredProviderRecord): ConnectedProvider | null {
    const catalog = getProviderById(record.id);
    if (!catalog) {
      return null;
    }
    return {
      id: record.id,
      name: catalog.name,
      group: catalog.group,
      groupLabel: catalog.groupLabel,
      // Prefer the user's saved endpoint override; fall back to catalog default.
      baseUrl: record.baseUrl || catalog.baseUrl,
      model: record.model,
      isActive: record.isActive,
      connectedAt: record.connectedAt,
    };
  }

  private async readRecords(): Promise<StoredProviderRecord[]> {
    const raw = await this.store.getItem(VAULT_STORAGE_KEY);
    if (!raw) {
      return [];
    }
    try {
      const parsed = JSON.parse(raw) as StoredProviderRecord[];
      return Array.isArray(parsed) ? parsed : [];
    } catch {
      return [];
    }
  }

  private async writeRecords(records: StoredProviderRecord[]): Promise<void> {
    await this.store.setItem(VAULT_STORAGE_KEY, JSON.stringify(records));
  }
}
