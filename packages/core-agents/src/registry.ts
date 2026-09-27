/**
 * Agent metadata registry (toolIds, maxRisk, webAccess, etc.).
 *
 * UI-specific prompt overlays live in
 * @agentcowork/core-ai/src/chat/agents.ts AGENT_CATALOG — this file
 * defines the canonical tool/risk profile. Agent IDs shared between
 * BOTH files (general, research, reader) MUST be kept in sync.
 *
 * Custom agents are persisted to SQLite (agents table) and survive restarts.
 * Shipped agents are constants — not stored in DB.
 */

import type { AgentProfile, AgentProfileRepository } from './types';

export const SHIPPED_AGENTS: AgentProfile[] = [
  {
    id: 'general', name: 'General Assistant', icon: '🤖',
    instructions: 'You are a helpful general assistant. Answer questions, help with tasks, and be concise.',
    // toolIds must match core-engine FAMILY_TO_TOOLS + app tool-definitions.
    toolIds: [
      'search_local_files',
      'search_web',
      'search_chat_history',
      'read_memory',
      'draft_automation',
      'create_markdown',
    ],
    maxRisk: 'local-write', webAccess: true, memoryScope: 'full',
    preferredModel: [], maxToolCallsPerTurn: 8,
  },
  {
    id: 'research', name: 'Research Agent', icon: '🔬',
    instructions: 'You are a research assistant. Prioritize web search and deep analysis. Cite sources with [N]. Multi-step synthesis.',
    toolIds: ['search_web', 'fetch_web_page', 'search_local_files', 'create_markdown'],
    maxRisk: 'read', webAccess: true, memoryScope: 'project',
    preferredModel: [], maxToolCallsPerTurn: 12,
  },
  {
    id: 'reader', name: 'Reader Agent', icon: '📖',
    instructions: 'You are a reading assistant. Answers come only from the open document. Use search_current_document. Cite page numbers.',
    toolIds: ['search_current_document', 'get_document_page', 'create_highlight', 'create_note', 'explain_selection', 'translate_selection'],
    maxRisk: 'local-write', webAccess: false, memoryScope: 'none',
    preferredModel: [], maxToolCallsPerTurn: 6,
  },
  {
    id: 'creator', name: 'Creator Agent', icon: '✍️',
    instructions: 'You create polished documents. Generate structured markdown that converts to DOCX/PDF.',
    toolIds: ['create_markdown', 'create_docx', 'create_pdf', 'search_local_files', 'search_web'],
    maxRisk: 'local-write', webAccess: true, memoryScope: 'project',
    preferredModel: [], maxToolCallsPerTurn: 10,
  },
  {
    id: 'writer', name: 'Writer Agent', icon: '📝',
    instructions: 'You write polished prose in the user\'s voice. Prioritize clarity, rhythm, and authentic tone. Avoid AI-isms and filler phrases.',
    toolIds: ['search_local_files', 'search_chat_history', 'read_memory', 'create_markdown'],
    maxRisk: 'local-write', webAccess: false, memoryScope: 'project',
    preferredModel: [], maxToolCallsPerTurn: 8,
  },
  {
    id: 'planner', name: 'Planner Agent', icon: '📋',
    instructions: 'You break goals into ordered, time-aware, actionable steps. Suggest automations when the user describes recurring work.',
    toolIds: [
      'draft_automation',
      'list_automations',
      'search_local_files',
      'search_chat_history',
      'read_memory',
      'create_markdown',
    ],
    maxRisk: 'local-write', webAccess: false, memoryScope: 'project',
    preferredModel: [], maxToolCallsPerTurn: 10,
  },
  {
    id: 'code', name: 'Code Agent', icon: '💻',
    instructions: 'You write correct, idiomatic, production-ready code. Include imports, error handling, and type annotations. Explain only non-obvious logic.',
    toolIds: ['search_local_files', 'search_web', 'create_markdown'],
    maxRisk: 'local-write', webAccess: true, memoryScope: 'project',
    preferredModel: [], maxToolCallsPerTurn: 10,
  },
  {
    id: 'docmaker', name: 'DocMaker', icon: '📄',
    instructions: 'You produce structured documents (Word, PDF). Write real, finished content — never placeholders. Match the genre conventions of the requested format.',
    toolIds: ['create_markdown', 'create_docx', 'create_pdf', 'search_local_files', 'read_memory'],
    maxRisk: 'local-write', webAccess: false, memoryScope: 'project',
    preferredModel: [], maxToolCallsPerTurn: 8,
  },
  {
    id: 'summarizer', name: 'Summarizer', icon: '📉',
    instructions: 'You compress text into the most compact lossless form. Preserve named entities, numbers, dates, and technical terms exactly. Strip filler.',
    toolIds: ['search_local_files', 'search_chat_history', 'read_memory', 'create_markdown'],
    maxRisk: 'read', webAccess: false, memoryScope: 'project',
    preferredModel: [], maxToolCallsPerTurn: 6,
  },
];

const SHIPPED_IDS = new Set(SHIPPED_AGENTS.map((a) => a.id));

/**
 * SQLite-backed agent repository.
 *
 * DB schema (agents table):
 *   id TEXT PK, name TEXT, icon TEXT, instructions_md TEXT,
 *   tools_json TEXT, permissions_json TEXT, memory_scope TEXT,
 *   model_policy_json TEXT, budget_json TEXT, schema_json TEXT,
 *   enabled INTEGER, device_id TEXT, op_seq INTEGER
 *
 * - Shipped agents loaded from constants (never written to DB)
 * - Custom agents persisted to `agents` table, survive restarts
 */
export function createAgentProfileRepository(db?: {
  getAll: (sql: string, ...params: unknown[]) => Promise<Record<string, unknown>[]>;
  run: (sql: string, ...params: unknown[]) => Promise<void>;
  get: (sql: string, ...params: unknown[]) => Promise<Record<string, unknown> | null>;
}): AgentProfileRepository {
  const cache = new Map<string, AgentProfile>();
  for (const a of SHIPPED_AGENTS) cache.set(a.id, a);

  if (!db) {
    return {
      async get(id) { return cache.get(id) ?? null; },
      async list() { return [...cache.values()]; },
      async save(agent) { cache.set(agent.id, agent); },
      async delete(id) { cache.delete(id); },
    };
  }

  let loaded = false;
  async function ensureLoaded() {
    if (loaded) return;
    loaded = true;
    try {
      const rows = await db!.getAll('SELECT * FROM agents WHERE enabled = 1');
      for (const row of rows) {
        // Skip shipped agents — they come from constants
        if (SHIPPED_IDS.has(row.id as string)) continue;

        let permissions: {
          maxRisk?: AgentProfile['maxRisk'];
          webAccess?: boolean;
          maxToolCallsPerTurn?: number;
        } = {};
        try {
          permissions = row.permissions_json
            ? (JSON.parse(row.permissions_json as string) as typeof permissions)
            : {};
        } catch {
          permissions = {};
        }
        let preferredModel: string[] = [];
        try {
          const policy = row.model_policy_json
            ? (JSON.parse(row.model_policy_json as string) as { preferredModel?: string[] })
            : null;
          preferredModel = policy?.preferredModel ?? [];
        } catch {
          preferredModel = [];
        }
        const agent: AgentProfile = {
          id: row.id as string,
          name: row.name as string,
          icon: (row.icon as string) ?? '🤖',
          instructions: (row.instructions_md as string) ?? '',
          toolIds: row.tools_json ? JSON.parse(row.tools_json as string) : [],
          maxRisk: permissions.maxRisk ?? 'local-write',
          // Prefer explicit permissions_json.webAccess; fall back to true only if omitted.
          webAccess: permissions.webAccess ?? true,
          memoryScope: (row.memory_scope as AgentProfile['memoryScope']) ?? 'full',
          maxToolCallsPerTurn: permissions.maxToolCallsPerTurn ?? 8,
          preferredModel,
        };
        cache.set(agent.id, agent);
      }
    } catch {
      // Table may not exist yet — silently ignore
    }
  }

  return {
    async get(id) {
      await ensureLoaded();
      return cache.get(id) ?? null;
    },

    async list() {
      await ensureLoaded();
      return [...cache.values()];
    },

    async save(agent) {
      await ensureLoaded();
      cache.set(agent.id, agent);

      // Never write shipped agents to DB
      if (SHIPPED_IDS.has(agent.id)) return;

      try {
        const permissionsJson = JSON.stringify({
          maxRisk: agent.maxRisk ?? 'local-write',
          webAccess: agent.webAccess ?? true,
          maxToolCallsPerTurn: agent.maxToolCallsPerTurn ?? 8,
        });
        const modelPolicyJson = agent.preferredModel?.length
          ? JSON.stringify({ preferredModel: agent.preferredModel })
          : null;

        await db!.run(
          `INSERT OR REPLACE INTO agents
           (id, name, icon, instructions_md, tools_json, permissions_json,
            memory_scope, model_policy_json, enabled)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1)`,
          agent.id,
          agent.name,
          agent.icon ?? null,
          agent.instructions ?? '',
          JSON.stringify(agent.toolIds ?? []),
          permissionsJson,
          agent.memoryScope ?? 'full',
          modelPolicyJson,
        );
      } catch {
        // DB write failure is non-fatal — in-memory cache still updated
      }
    },

    async delete(id) {
      await ensureLoaded();
      cache.delete(id);

      if (!SHIPPED_IDS.has(id)) {
        try {
          await db!.run('DELETE FROM agents WHERE id = ?', id);
        } catch {
          // Non-fatal
        }
      }
    },
  };
}
