import type { ProviderGroup } from '@agentcowork/core-domain';

/** Minimal async key-value store used by ProviderVault. */
export interface KeyValueStore {
  getItem(key: string): Promise<string | null>;
  setItem(key: string, value: string): Promise<void>;
  removeItem(key: string): Promise<void>;
}

/** Static catalog entry for a BYOK provider tile. */
export interface ProviderCatalogEntry {
  id: string;
  name: string;
  group: ProviderGroup;
  groupLabel: string;
  signupUrl: string;
  baseUrl: string;
  defaultModel: string;
  description: string;
  recommended?: boolean;
  /** openai = /v1/models or chat ping; key-only = non-LLM BYOK (search, image, MCP). */
  validation?: 'openai' | 'key-only';
}

/**
 * Persisted provider record (P69.C4).
 *
 * Credentials never live here: `keyRef` is an **opaque Rust-vault handle**
 * (`vault:<provider>:<n>`), not key material. Sealing/serializing keys in
 * TypeScript was removed with the rest of the TS credential path — provider
 * keys live only in `agentcowork-vault` (AGENTS.md §15, I10).
 */
export interface StoredProviderRecord {
  id: string;
  model: string;
  isActive: boolean;
  /** Opaque vault handle; presence means "a key is configured" (availability). */
  keyRef?: string;
  connectedAt: string;
  /** User-supplied base URL override (Azure/Databricks/Snowflake/Bedrock/Vertex custom endpoints). */
  baseUrl?: string;
}

/** Provider visible to the app after loading from vault. */
export interface ConnectedProvider {
  id: string;
  name: string;
  group: ProviderGroup;
  groupLabel: string;
  baseUrl: string;
  model: string;
  isActive: boolean;
  connectedAt: string;
}
// P71.2d (ADR-0005) — `OpenAiProviderConfig` and `ValidationResult` described
// the deleted inference clients' runtime config and key-probe result. Nothing
// in v1 calls a provider from TypeScript, so the shapes are gone rather than
// left as a standing invitation to re-add the path.
