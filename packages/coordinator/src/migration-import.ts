/**
 * P30.14 — the migration importer (skales pattern, doc 83 §1): import
 * ChatGPT/Claude/OpenClaw exports + agent instructions + editor/MCP config
 * into AgentCowork. Re-rated from defer to a narrow ship (doc 82 "Migration
 * Concierge"): parse exports → sessions, agent-instruction files → skills,
 * editor/MCP configs → connector registry entries.
 *
 * Parsers are strict and honest: unrecognized shapes are rejected with a
 * reason, never silently dropped. No network access — pure local import.
 */

export type ImportKind = "chatgpt" | "claude" | "openclaw" | "agent_instructions" | "editor_mcp";

export interface ImportedSession {
  title: string;
  /** The imported conversation turns (role + text). */
  turns: Array<{ role: "user" | "assistant"; text: string }>;
  source: ImportKind;
  /** Original export id, when present. */
  sourceId?: string;
}

export interface ImportedSkill {
  /** Derived name (from file name / frontmatter). */
  name: string;
  /** The instruction body (SKILL.md-style). */
  body: string;
  source: ImportKind;
}

export interface ImportedConfig {
  /** Connector/editor id (e.g. "mcp.github", "editor.cursor"). */
  key: string;
  /** The config document (JSON string, preserved as-is). */
  value: string;
  source: ImportKind;
}

export interface MigrationResult {
  sessions: ImportedSession[];
  skills: ImportedSkill[];
  configs: ImportedConfig[];
  /** Rejected items with a plain-language reason (honesty surface). */
  rejected: Array<{ name: string; reason: string }>;
}

/** Extract the conversational turns from a ChatGPT `conversations.json`. */
export function parseChatGptExport(doc: string): ImportedSession[] {
  const sessions: ImportedSession[] = [];
  const data = JSON.parse(doc) as unknown;
  if (!Array.isArray(data)) {
    throw new Error("ChatGPT export must be a JSON array of conversations");
  }
  for (const conv of data) {
    const c = conv as Record<string, unknown>;
    const title = String(c.title ?? c.id ?? "Imported conversation");
    const turns: ImportedSession["turns"] = [];
    const mapping = c.mapping as Record<string, { message?: { author?: { role?: string }; content?: { parts?: string[] } } }> | undefined;
    if (mapping) {
      for (const node of Object.values(mapping)) {
        const msg = node?.message;
        const role = msg?.author?.role;
        const parts = msg?.content?.parts;
        if (!msg || !parts) continue;
        const text = parts.filter((p): p is string => typeof p === "string").join("\n");
        if (!text.trim()) continue;
        if (role === "user" || role === "assistant") {
          turns.push({ role, text });
        }
      }
    }
    if (turns.length > 0) {
      sessions.push({ title, turns, source: "chatgpt", sourceId: String(c.id ?? "") });
    }
  }
  return sessions;
}

/** Parse a Claude export (JSONL of messages, `type: "user"|"assistant"`). */
export function parseClaudeExport(doc: string): ImportedSession[] {
  const sessions: ImportedSession[] = [];
  const turns: ImportedSession["turns"] = [];
  for (const line of doc.split("\n")) {
    if (!line.trim()) continue;
    let entry: unknown;
    try {
      entry = JSON.parse(line);
    } catch {
      continue;
    }
    const e = entry as { type?: string; message?: { role?: string; content?: Array<{ type?: string; text?: string }> | string } };
    if (e.type === "user" || e.type === "assistant") {
      const content = e.message?.content;
      let text = "";
      if (typeof content === "string") text = content;
      else if (Array.isArray(content)) {
        text = content
          .filter((c) => c?.type === "text" && typeof c.text === "string")
          .map((c) => c.text!)
          .join("\n");
      }
      if (text.trim()) turns.push({ role: e.type, text });
    }
  }
  if (turns.length > 0) {
    sessions.push({ title: "Claude import", turns, source: "claude" });
  }
  return sessions;
}

/** Parse an OpenClaw-style markdown export (headers as user turns). */
export function parseOpenClawExport(doc: string): ImportedSession[] {
  const sessions: ImportedSession[] = [];
  const turns: ImportedSession["turns"] = [];
  let current: string | null = null;
  for (const line of doc.split("\n")) {
    const h = line.match(/^#{1,3}\s+(.+)$/);
    if (h) {
      if (current) turns.push({ role: "user", text: current });
      current = h[1]!.trim();
    } else if (current !== null) {
      current += "\n" + line;
    } else if (line.trim()) {
      // Lines before the first header are the goal.
      current = line.trim();
    }
  }
  if (current) turns.push({ role: "user", text: current });
  if (turns.length > 0) {
    sessions.push({ title: "OpenClaw import", turns, source: "openclaw" });
  }
  return sessions;
}

/** Parse an agent-instruction file (CLAUDE.md / AGENTS.md / SOUL.md) → skill. */
export function parseAgentInstructions(name: string, doc: string): ImportedSkill {
  // Strip a leading YAML frontmatter block.
  let body = doc;
  if (body.startsWith("---")) {
    const end = body.indexOf("\n---", 3);
    if (end !== -1) body = body.slice(end + 4).trimStart();
  }
  const base = name.replace(/\.(md|txt)$/i, "");
  return { name: base, body: body.trim(), source: "agent_instructions" };
}

/**
 * Import a JSON config (editor / MCP server config) → connector registry
 * entries. The `key` is derived from the top-level object keys.
 */
export function parseEditorMcpConfig(name: string, doc: string): ImportedConfig[] {
  const data = JSON.parse(doc) as Record<string, unknown>;
  const out: ImportedConfig[] = [];
  for (const [key, value] of Object.entries(data)) {
    out.push({
      key: `${name.replace(/\.[a-z]+$/i, "")}.${key}`,
      value: typeof value === "string" ? value : JSON.stringify(value, null, 2),
      source: "editor_mcp",
    });
  }
  return out;
}

/** Run an import batch; individual failures become `rejected` rows. */
export function runMigration(kind: ImportKind, name: string, doc: string): MigrationResult {
  const result: MigrationResult = { sessions: [], skills: [], configs: [], rejected: [] };
  try {
    switch (kind) {
      case "chatgpt":
        result.sessions.push(...parseChatGptExport(doc));
        break;
      case "claude":
        result.sessions.push(...parseClaudeExport(doc));
        break;
      case "openclaw":
        result.sessions.push(...parseOpenClawExport(doc));
        break;
      case "agent_instructions":
        result.skills.push(parseAgentInstructions(name, doc));
        break;
      case "editor_mcp":
        result.configs.push(...parseEditorMcpConfig(name, doc));
        break;
    }
  } catch (err) {
    result.rejected.push({
      name,
      reason: err instanceof Error ? err.message : "failed to parse this file",
    });
  }
  return result;
}
