export const MERMAID_MAX_SOURCE_CHARS = 50_000
export const MERMAID_MAX_EDGES = 500

export type MermaidSourceCheck =
  | { ok: true; source: string }
  | { ok: false; reason: string }

/** Validate user/model-provided Mermaid before passing it to the renderer. */
export function checkMermaidSource(source: string): MermaidSourceCheck {
  const normalized = source.replace(/^\uFEFF/, '').trim()
  if (!normalized) return { ok: false, reason: 'The diagram is empty.' }
  if (normalized.length > MERMAID_MAX_SOURCE_CHARS) {
    return { ok: false, reason: `This diagram is over the ${MERMAID_MAX_SOURCE_CHARS.toLocaleString()} character limit.` }
  }
  // Mermaid directives can override renderer configuration. This surface keeps
  // security and theme options controlled by the host, so directives are refused.
  if (/%%\s*\{/i.test(normalized)) {
    return { ok: false, reason: 'This diagram uses a configuration directive that is not allowed here.' }
  }

  const edges = normalized.match(/(?:<)?(?:o|x)?[-.=]{2,}(?:o|x)?(?:>)?/g)?.length ?? 0
  if (edges > MERMAID_MAX_EDGES) {
    return { ok: false, reason: `This diagram has more than ${MERMAID_MAX_EDGES} connections.` }
  }
  return { ok: true, source: normalized }
}
