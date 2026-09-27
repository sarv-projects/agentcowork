export {
  PROVIDER_CATALOG,
  AI_PROVIDER_GROUPS,
  WEB_OTHER_PROVIDER_GROUPS,
  getProviderById,
  getRecommendedProviders,
  filterProvidersBySection,
  isAiProviderGroup,
  groupProvidersByLabel,
} from './registry.js';
// ADR-0005 (P71.2d) — the inference clients (`openai-client`, `anthropic-client`)
// are deleted: v1 has no AgentCowork-owned model call, because the bound external
// agent owns its own model. What remains here is the **observability** half —
// provider catalogue, model capabilities, live pricing and the credential
// façade — which the coordinator still reads to report cost/usage.
export {
  fetchProviderPricing,
  formatPricingLine,
  type ProviderPricing,
} from './pricing/live-pricing.js';
export { ProviderVault } from './vault.js';
export {
  getModelCatalog,
  setModelCatalog,
  getBundledCatalogVersion,
  getModelsForProvider,
  getModelCapabilities,
  modelSupportsReasoning,
  modelSupportsVision,
  type ModelCatalog,
  type ModelCapabilities,
  type ProviderModels,
  type ModelInputModality,
} from './capability-registry.js';
export type {
  ConnectedProvider,
  KeyValueStore,
  ProviderCatalogEntry,
  StoredProviderRecord,
} from './types.js';
export type {
  ModelAdapter,
  CapabilityProfile,
  CostEstimate,
  NormalizedModelResult,
  NormalizedToolCall,
  ProviderRequest,
} from './types/model-adapter.js';