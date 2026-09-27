/**
 * Agent *profiles* — the sidecar's runtime agent shape: instructions, tool
 * subset, risk ceiling, memory scope, model preference.
 *
 * Naming matters here (P69.D1/D25): the canonical **agent record**
 * (`AgentDefinition` — protocol, auth mode, capabilities, provenance) is owned
 * by Rust (`agentcowork_types::AgentDefinition`) and read through
 * `agent_directory_list`; this is the sandbox/profile half that never travels
 * on the registry wire. Keeping one name per concept is what stops a second
 * "the agent" type from appearing at a boundary.
 */
import type { RiskLevel } from '@agentcowork/core-tools';

export type MemoryScope = 'full' | 'project' | 'none';

export interface AgentProfile {
  id: string;
  name: string;
  icon: string;
  instructions: string;
  toolIds: string[];
  maxRisk: RiskLevel;
  webAccess: boolean;
  memoryScope: MemoryScope;
  preferredModel: string[];
  maxToolCallsPerTurn: number;
  maxCreditsPerRun?: number;
  outputSchema?: Record<string, unknown>;
}

export interface AgentProfileRepository {
  get(id: string): Promise<AgentProfile | null>;
  list(): Promise<AgentProfile[]>;
  save(agent: AgentProfile): Promise<void>;
  delete(id: string): Promise<void>;
}
