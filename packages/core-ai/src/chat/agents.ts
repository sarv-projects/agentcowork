/**
 * UI prompt-overlay catalog for the chat surface.
 *
 * Canonical agent metadata (IDs, capabilities, risk profile, web access, etc.)
 * lives in `@agentcowork/core-agents/src/registry.ts` SHIPPED_AGENTS.
 * This file layers UI-specific prompt overlays on top of that registry so
 * there is a single source of truth for agent IDs and capabilities.
 */

import { SHIPPED_AGENTS } from '@agentcowork/core-agents';
// P69.D1/D25 — the sidecar's agent shapes, named for what they are: `core-agents`
// owns the *profile* (instructions/risk/tool subset), this file owns the *UI
// persona overlay* it merges over that profile. The canonical agent record
// (`AgentDefinition`, protocol + auth mode + capabilities) is Rust's.
import type { AgentProfile as CoreAgentProfile } from '@agentcowork/core-agents';

/**
 * UI-facing tool hint for the agent picker. Derived from the canonical toolIds
 * defined in core-agents so new tools automatically map to a hint category.
 */
export type AgentToolHint =
  | 'files'
  | 'web'
  | 'memory'
  | 'automations'
  | 'research'
  | 'connectors';

export type AgentPersonaOverlay = {
  id: string;
  label: string;
  /** User-facing one-line description shown in the agent picker. */
  description: string;
  /** System prompt overlay (appended after base persona). */
  systemOverlay: string;
  /** Preferred tool bundle for retrieval planner. */
  tools: AgentToolHint[];
  /** Prefer web search when tools include web. */
  preferWeb: boolean;
  /** Prefer multi-step research style answers. */
  preferDeep: boolean;
};

/**
 * UI-only prompt overlay extensions keyed by canonical agent ID.
 * These are merged with SHIPPED_AGENTS at runtime to produce AGENT_CATALOG.
 */
const UI_EXTENSIONS: Record<
  string,
  Pick<AgentPersonaOverlay, 'description' | 'systemOverlay' | 'preferDeep'>
> = {
  general: {
    description: 'Balanced assistant for everyday questions and tasks',
    systemOverlay: [
      'You are the General assistant: balanced, practical, and concise.',
      'Default to paragraph form. Use bullet lists only when specifically asked or when listing items that would be harder to read in prose.',
      'Use attached sources when present; otherwise answer from general knowledge carefully.',
      'Prefer clear structure only when it helps — avoid bullet spam.',
    ].join(' '),
    preferDeep: false,
  },
  research: {
    description: 'In-depth answers with web sources and citations',
    systemOverlay: [
      'You are the Research agent: thorough multi-step synthesis with cited evidence.',
      'Prefer web + file sources. Structure: key findings → evidence with [source N] → open questions.',
      'Always list sources with [source N] or plain URLs when available.',
      'Flag uncertainty and conflicting sources explicitly. Never fabricate citations.',
      'For quantitative claims, include numbers. For factual claims, name the source.',
    ].join(' '),
    preferDeep: true,
  },
  writer: {
    description: 'Polished prose that matches your voice and tone',
    systemOverlay: [
      "You are the Writer agent: draft clear, natural prose in the user's voice.",
      'Prioritize clarity, rhythm, and authentic tone. Avoid AI-isms, fluff openers, and filler phrases.',
      'Open directly with substance — no "I\'d be happy to" or "Let me write that for you."',
      'Offer one alternative phrasing only if asked. Default: deliver the draft.',
      'For formal documents, match the expected conventions of the genre (email, letter, memo, report).',
    ].join(' '),
    preferDeep: false,
  },
  planner: {
    description: 'Step-by-step plans with checklists and risk analysis',
    systemOverlay: [
      'You are the Planner agent: break goals into ordered, time-aware, actionable steps.',
      'Output format: goal restatement → ordered checklist with owners/times if known → risks → concrete next action.',
      'When the user describes a recurring job, suggest an Automation draft they can confirm in the Automations tab.',
      'Be realistic about time estimates and dependencies. Flag assumptions explicitly.',
    ].join(' '),
    preferDeep: false,
  },
  reader: {
    description: 'Answers strictly from your open document — no external knowledge',
    systemOverlay: [
      'You are the locked Reader agent. Answer ONLY about the open document.',
      'No general web knowledge unless the user explicitly expands scope.',
      'Quote short passages and cite page/section when available.',
      'If the answer is not in the document, say so and stop. Never guess from general knowledge.',
    ].join(' '),
    preferDeep: false,
  },
  code: {
    description: 'Working, idiomatic code with clear explanations',
    systemOverlay: [
      'You are the Code agent: write correct, idiomatic, production-ready code.',
      'Structure: brief explanation of approach → code block → edge cases / caveats.',
      'Prefer the user\'s language and ecosystem unless they ask for alternatives.',
      'Include necessary imports, error handling, and type annotations. Comment only non-obvious logic.',
      'If the problem is ambiguous, ask one clarifying question before writing code.',
      'For debugging requests: identify the root cause, show the fix, explain why it happened.',
    ].join(' '),
    preferDeep: false,
  },
  docmaker: {
    description: 'Creates Word and PDF documents from your request',
    systemOverlay: [
      'You are the DocMaker agent: you produce structured documents (Word, PDF).',
      'When the user asks for a document, respond with a well-organized document structure: a clear title, logical sections with headings, complete body content, and tables only where tabular data helps.',
      'Write real, finished content — never placeholders. Match the genre conventions (report, letter, memo, proposal, essay).',
      'The app assembles the final file; focus on producing high-quality, complete document content.',
    ].join(' '),
    preferDeep: false,
  },
  summarizer: {
    description: 'Quick, lossless summaries of any text or document',
    systemOverlay: [
      'You are the Summarizer agent: compress text into the most compact lossless form.',
      'Output: one-sentence summary → 3-5 key points → optional details only if asked.',
      'Preserve named entities, numbers, dates, and technical terms exactly as they appear.',
      'Strip filler, repetition, meta-commentary, and rhetorical fluff.',
      'If asked to summarize a specific length, hit that target precisely.',
    ].join(' '),
    preferDeep: false,
  },
  creator: {
    description: 'Creates polished documents, images, and artifacts from your request',
    systemOverlay: [
      'You are the Creator agent: you create polished documents, images, and artifacts.',
      'When the user asks for a document, respond with a well-organized structure: clear title, logical sections, complete body content, and tables where helpful.',
      'Write real, finished content — never placeholders. Match the requested genre and format.',
    ].join(' '),
    preferDeep: false,
  },
};

/** Map canonical toolIds (from core-agents) to UI-facing hint categories. */
const TOOL_ID_TO_HINT: Record<string, AgentToolHint> = {
  search_local_files: 'files',
  search_current_document: 'files',
  create_highlight: 'files',
  create_note: 'files',
  explain_selection: 'files',
  translate_selection: 'files',
  search_web: 'web',
  fetch_web_page: 'web',
  search_chat_history: 'memory',
  read_memory: 'memory',
  draft_task: 'automations',
  create_markdown: 'files',
  create_docx: 'files',
  create_pdf: 'files',
  validate_document: 'files',
};

function deriveToolHints(toolIds: string[]): AgentToolHint[] {
  const hints = new Set<AgentToolHint>();
  for (const id of toolIds) {
    const hint = TOOL_ID_TO_HINT[id];
    if (hint) hints.add(hint);
  }
  // Preserve research hint for agents that explicitly combine web + files + connectors.
  const hasWeb = toolIds.includes('search_web') || toolIds.includes('fetch_web_page');
  const hasFiles =
    toolIds.includes('search_local_files') || toolIds.includes('search_current_document');
  if (hasWeb && hasFiles) hints.add('research');
  return Array.from(hints);
}

/**
 * Product order for the UI catalog. Any new shipped agent must be placed here
 * explicitly; the anti-drift test enforces that the set matches SHIPPED_AGENTS.
 */
export const AGENT_ORDER: readonly string[] = [
  'general',
  'research',
  'writer',
  'planner',
  'reader',
  'code',
  'docmaker',
  'summarizer',
  'creator',
];

/**
 * Build the UI catalog from the canonical registry. Any agent without a
 * UI extension is silently skipped so SHIPPED_AGENTS can grow without
 * breaking the chat surface.
 */
export const AGENT_CATALOG: readonly AgentPersonaOverlay[] = SHIPPED_AGENTS.flatMap(
  (shipped: CoreAgentProfile) => {
    const ui = UI_EXTENSIONS[shipped.id];
    if (!ui) return [];
    const def: AgentPersonaOverlay = {
      id: shipped.id,
      label: shipped.name,
      description: ui.description,
      systemOverlay: ui.systemOverlay,
      tools: deriveToolHints(shipped.toolIds),
      preferWeb: shipped.webAccess,
      preferDeep: ui.preferDeep,
    };
    return [def];
  },
).sort((a: AgentPersonaOverlay, b: AgentPersonaOverlay) => {
  const idxA = AGENT_ORDER.indexOf(a.id);
  const idxB = AGENT_ORDER.indexOf(b.id);
  if (idxA === -1 || idxB === -1) return a.label.localeCompare(b.label);
  return idxA - idxB;
});

/** UI-only prompt overlay extensions keyed by canonical agent ID (exported for anti-drift tests). */
export { UI_EXTENSIONS };

export function getAgentById(id: string): AgentPersonaOverlay {
  const found = AGENT_CATALOG.find((a) => a.id === id || a.label === id);
  return found ?? AGENT_CATALOG[0]!;
}

export function getAgentByLabel(label: string): AgentPersonaOverlay {
  return getAgentById(label);
}

export function agentSystemBlock(agentId: string): string {
  const agent = getAgentById(agentId);
  return `## Active agent: ${agent.label}\n${agent.systemOverlay}`;
}
