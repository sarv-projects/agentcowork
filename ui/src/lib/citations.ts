/**
 * P52.20 — numbered citation jump targets for the chat bubble.
 * Producers live in the coordinator (`citationsFromSearchResult`); this
 * module only formats and locates them.
 */

export interface ChatCitation {
  index: number
  title: string
  url: string
  snippet?: string
  source?: string
}

export function citationAnchorId(index: number, messageId?: string): string {
  if (!messageId) return `cite-${index}`
  const safeMessageId = messageId.replace(/[^a-zA-Z0-9_-]/g, '_')
  return `cite-${safeMessageId}-${index}`
}

/** P51.7 — exportable dump: marked body + numbered source list. */
export function formatCitationExport(text: string, citations: ChatCitation[]): string {
  const body = applyCitationMarks(text, citations)
  if (!citations.length) return body
  const refs = [...citations]
    .sort((a, b) => a.index - b.index)
    .map((c) => `[^${c.index}]: ${c.title} (${c.url})`)
    .join('\n')
  return `${body}\n\n## Sources\n${refs}`
}

/** Replace a bare URL mention with `[^n]` when that URL is a known citation. */
export function applyCitationMarks(text: string, citations: ChatCitation[]): string {
  if (!citations.length) return text
  let out = text
  for (const c of citations) {
    const mark = `[^${c.index}]`
    if (out.includes(mark)) continue
    if (c.url && out.includes(c.url)) {
      out = out.replace(c.url, `${c.url} ${mark}`)
    }
  }
  return out
}
