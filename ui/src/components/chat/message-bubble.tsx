'use client'

import { Children, isValidElement, memo, useEffect, useId, useMemo, useRef, useState } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import remarkMath from 'remark-math'
import rehypeKatex from 'rehype-katex'
import rehypeHighlight from 'rehype-highlight'
import {
  AlertTriangle,
  Brain,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Download,
  ExternalLink,
  Fingerprint,
  GitFork,
  Image as ImageIcon,
  Pencil,
  Quote,
  RotateCw,
  ShieldAlert,
  Sparkles,
  User,
  Volume2,
} from 'lucide-react'
import 'katex/dist/katex.min.css'
import 'highlight.js/styles/github-dark.css'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible'
import type { Artifact, ChatError, ChatMessage } from '@/lib/store'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/lib/store'
import { explainError } from '@/lib/errors'
import { saferMode, saferPrompt, differentlyPrompt, hasUndoableWork } from '@/lib/recovery'
import { checkpointSummary, isMutatingMessage } from '@/lib/checkpoints'
import ArtifactCard, { openArtifactInWorkspace } from './artifact-card'
import { staggerStyle } from '@/lib/stagger'
import McqInterruptCard from './mcq-interrupt-card'
import ProgressSteps from './progress-steps'
import ToolChips, { toolActivitySummary } from './tool-chip'
import { TurnCheckpoint } from './turn-checkpoint'
import { applyCitationMarks, citationAnchorId, formatCitationExport } from '@/lib/citations'
import { speakText, speechSynthesisAvailable, stopSpeaking } from '@/lib/voice'

function CodeBlock({ children, className, ...props }: React.ComponentProps<'code'> & { inline?: boolean }) {
  const [copied, setCopied] = useState(false)
  // Detect block code (inside <pre>) vs inline code by checking className or children type
  const isBlock = String(children).includes('\n') || (className && className.includes('language-'))

  if (!isBlock) {
    return (
      <code
        className={cn(
          'rounded bg-zinc-800/70 px-1 py-0.5 font-mono text-[11px] text-brand',
          className
        )}
        {...props}
      >
        {children}
      </code>
    )
  }

  const codeContent = String(children).replace(/\n$/, '')

  return (
    <div className="group/code relative my-2 overflow-hidden rounded-md border border-border bg-zinc-950">
      <div className="flex items-center justify-between border-b border-border/40 bg-zinc-900/60 px-2 py-1">
        <span className="font-mono text-[9px] uppercase tracking-wider text-muted-foreground/70">
          {(className?.replace('language-', '') || 'code')}
        </span>
        <button
          onClick={() => {
            navigator.clipboard?.writeText(codeContent)
            setCopied(true)
            setTimeout(() => setCopied(false), 1500)
          }}
          className="flex items-center gap-1 rounded px-1.5 py-0.5 font-mono text-[9px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          title="Copy code"
        >
          {copied ? (
            <>
              <Check className="h-2.5 w-2.5 text-emerald-400" />
              Copied
            </>
          ) : (
            <>
              <Copy className="h-2.5 w-2.5" />
              Copy
            </>
          )}
        </button>
      </div>
      <pre className="overflow-x-auto p-2 font-mono text-[11px] scroll-thin">
        <code className={className} {...props}>
          {children}
        </code>
      </pre>
    </div>
  )
}

function safeMarkdownWebUrl(href: string | undefined): string | null {
  if (!href) return null
  try {
    const url = new URL(href)
    return url.protocol === 'https:' || url.protocol === 'http:' ? url.href : null
  } catch {
    return null
  }
}

const katexOptions = {
  output: 'htmlAndMathml' as const,
  trust: false,
  maxExpand: 1000,
  maxSize: 20,
}

function MarkdownImage({ src, alt, title }: React.ComponentProps<'img'>) {
  const safeUrl = safeMarkdownWebUrl(src)
  const description = alt?.trim()
  return (
    <figure className="my-3 max-w-full overflow-hidden rounded-xl border border-border bg-background/60">
      <div className="flex min-h-36 flex-col items-center justify-center gap-2 bg-gradient-to-br from-muted/70 via-card to-muted/40 px-4 py-5 text-center sm:min-h-44">
        <span className="inline-flex h-10 w-10 items-center justify-center rounded-full border border-border bg-background/80 text-muted-foreground">
          <ImageIcon aria-hidden className="h-5 w-5" />
        </span>
        <p className="text-xs font-medium text-foreground">Image preview</p>
        <p className="max-w-md text-[11px] leading-relaxed text-muted-foreground">
          {description || 'This image has no text description.'}
        </p>
        {safeUrl ? (
          <button
            type="button"
            onClick={() => useAppStore.getState().openInBrowser(safeUrl)}
            className="mt-1 inline-flex min-h-9 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-[11px] font-medium text-brand transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
          >
            <ExternalLink aria-hidden className="h-3 w-3" />
            Open image in Browse
          </button>
        ) : (
          <p className="text-[10px] text-muted-foreground">This image reference can’t be opened safely from chat.</p>
        )}
      </div>
      {title ? <figcaption className="border-t border-border px-3 py-2 text-[10px] text-muted-foreground">{title}</figcaption> : null}
    </figure>
  )
}

interface CitationNode {
  type: string
  value?: string
  url?: string
  children?: CitationNode[]
}

function containsKatexParseError(node: React.ReactNode): boolean {
  return Children.toArray(node).some((child) => {
    if (!isValidElement(child)) return false
    const props = child.props as { mathcolor?: string; children?: React.ReactNode }
    if (child.type === 'mstyle' && props.mathcolor === '#cc0000' && containsUnknownMathCommand(props.children)) return true
    return containsKatexParseError(props.children)
  })
}

function containsUnknownMathCommand(node: React.ReactNode): boolean {
  return Children.toArray(node).some((child) => {
    if (!isValidElement(child)) return false
    const props = child.props as { children?: React.ReactNode }
    if (child.type === 'mtext') {
      return Children.toArray(props.children).some((text) => typeof text === 'string' && /^\\[a-z]+/i.test(text))
    }
    return containsUnknownMathCommand(props.children)
  })
}

function remarkCitationReferences(options: { messageId: string; indexes: number[] }) {
  const knownIndexes = new Set(options.indexes)
  return (tree: CitationNode) => {
    const visit = (node: CitationNode) => {
      if (!node.children) return
      const next: CitationNode[] = []
      for (const child of node.children) {
        if (child.type !== 'text' || !child.value) {
          visit(child)
          next.push(child)
          continue
        }

        const marker = /\[\^(\d+)\]/g
        let offset = 0
        let match: RegExpExecArray | null
        while ((match = marker.exec(child.value))) {
          const index = Number(match[1])
          if (!knownIndexes.has(index)) continue
          if (match.index > offset) next.push({ type: 'text', value: child.value.slice(offset, match.index) })
          next.push({
            type: 'link',
            url: `#${citationAnchorId(index, options.messageId)}`,
            children: [{ type: 'text', value: `[^${index}]` }],
          })
          offset = match.index + match[0].length
        }
        if (offset > 0) {
          if (offset < child.value.length) next.push({ type: 'text', value: child.value.slice(offset) })
        } else {
          next.push(child)
        }
      }
      node.children = next
    }
    visit(tree)
  }
}

function uniqueArtifactsByName(artifacts: Artifact[]): Map<string, Artifact> {
  const grouped = new Map<string, Map<string, Artifact>>()
  for (const artifact of artifacts) {
    const name = artifact.name.trim()
    if (!name) continue
    const matches = grouped.get(name) ?? new Map<string, Artifact>()
    matches.set(artifact.id, artifact)
    grouped.set(name, matches)
  }
  return new Map([...grouped.entries()].flatMap(([name, matches]) =>
    matches.size === 1 ? [[name, [...matches.values()][0]] as const] : [],
  ))
}

function remarkArtifactReferences(options: { artifacts: Artifact[] }) {
  const byName = uniqueArtifactsByName(options.artifacts)
  return (tree: CitationNode) => {
    const visit = (node: CitationNode, insideLink = false) => {
      if (!node.children) return
      const next: CitationNode[] = []
      for (const child of node.children) {
        if (child.type !== 'text' || !child.value || insideLink) {
          visit(child, insideLink || child.type === 'link')
          next.push(child)
          continue
        }

        let offset = 0
        const matches: { start: number; end: number; artifact: Artifact }[] = []
        for (const [name, artifact] of byName) {
          let cursor = 0
          while (cursor < child.value.length) {
            const start = child.value.indexOf(name, cursor)
            if (start < 0) break
            const end = start + name.length
            const before = start > 0 ? child.value[start - 1] : ''
            const after = end < child.value.length ? child.value[end] : ''
            const boundaryBefore = !before || !/[\w.]/u.test(before)
            const boundaryAfter = !after || !/[\w.]/u.test(after)
            if (boundaryBefore && boundaryAfter) matches.push({ start, end, artifact })
            cursor = Math.max(end, start + 1)
          }
        }
        matches.sort((a, b) => a.start - b.start || b.end - a.end)
        for (const match of matches) {
          if (match.start < offset) continue
          if (match.start > offset) next.push({ type: 'text', value: child.value.slice(offset, match.start) })
          next.push({
            type: 'link',
            url: `#artifact/${encodeURIComponent(match.artifact.id)}`,
            children: [{ type: 'text', value: child.value.slice(match.start, match.end) }],
          })
          offset = match.end
        }
        if (offset > 0) {
          if (offset < child.value.length) next.push({ type: 'text', value: child.value.slice(offset) })
        } else {
          next.push(child)
        }
      }
      node.children = next
    }
    visit(tree)
  }
}

const mdComponents = {
  code: CodeBlock,
  pre: ({ children }: React.ComponentProps<'pre'>) => <>{children}</>,
  strong: ({ children, ...props }: React.ComponentProps<'strong'>) => (
    <strong className="font-semibold text-foreground" {...props}>
      {children}
    </strong>
  ),
  em: ({ children, ...props }: React.ComponentProps<'em'>) => (
    <em className="italic text-muted-foreground" {...props}>
      {children}
    </em>
  ),
  ul: ({ children, ...props }: React.ComponentProps<'ul'>) => (
    <ul className="my-1 list-disc space-y-0.5 pl-5" {...props}>
      {children}
    </ul>
  ),
  li: ({ children, ...props }: React.ComponentProps<'li'>) => (
    <li className="text-[12px] leading-relaxed text-foreground/90" {...props}>
      {children}
    </li>
  ),
  table: ({ children, ...props }: React.ComponentProps<'table'>) => (
    <div
      role="region"
      aria-label="Response table. Scroll horizontally to view all columns."
      tabIndex={0}
      className="my-3 max-w-full overflow-x-auto rounded-lg border border-border focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
    >
      <table className="w-full border-collapse text-left text-[11px]" {...props}>
        {children}
      </table>
    </div>
  ),
  th: ({ children, ...props }: React.ComponentProps<'th'>) => (
    <th className="whitespace-nowrap border-b border-border bg-muted/60 px-3 py-2 font-semibold text-foreground" {...props}>
      {children}
    </th>
  ),
  td: ({ children, ...props }: React.ComponentProps<'td'>) => (
    <td className="border-b border-border/60 px-3 py-2 align-top text-foreground/90" {...props}>
      {children}
    </td>
  ),
  span: ({ children, className, ...props }: React.ComponentProps<'span'>) => {
    if (className?.split(/\s+/).includes('katex') && containsKatexParseError(children)) {
      return (
        <span role="note" className="inline-flex flex-wrap items-baseline gap-1 rounded bg-destructive/10 px-1 text-destructive">
          <span className="sr-only">Math could not be rendered. The original expression follows.</span>
          {children}
          <span aria-hidden="true" className="text-[9px]">Math could not be rendered</span>
        </span>
      )
    }
    return <span className={className} {...props}>{children}</span>
  },
  p: ({ children, ...props }: React.ComponentProps<'p'>) => {
    const hasBlockMedia = Children.toArray(children).some((child) =>
      isValidElement(child) && (child.type === MarkdownImage || ['figure', 'div', 'table', 'pre'].includes(String(child.type))),
    )
    const className = 'text-[12px] leading-relaxed text-foreground/90 [&:not(:first-child)]:mt-2'
    return hasBlockMedia
      ? <div className={className}>{children}</div>
      : <p className={className} {...props}>{children}</p>
  },
  img: MarkdownImage,
}

function markdownComponentsFor(citationIds: ReadonlySet<string>, artifactsById: ReadonlyMap<string, Artifact>) {
  return {
    ...mdComponents,
    a: ({ children, href, ...props }: React.ComponentProps<'a'>) => {
      if (href?.startsWith('#artifact/')) {
        try {
          const artifact = artifactsById.get(decodeURIComponent(href.slice('#artifact/'.length)))
          if (artifact) {
            return (
              <button
                type="button"
                onClick={() => openArtifactInWorkspace(artifact)}
                aria-label={`Open ${artifact.name}`}
                className="text-brand underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
              >
                {children}
              </button>
            )
          }
        } catch {
          // Malformed or unknown internal refs fall through to the inert link path.
        }
      }
      const citationId = href?.startsWith('#') ? href.slice(1) : undefined
      if (citationId && citationIds.has(citationId)) {
        return (
          <a {...props} href={href} className="text-brand underline-offset-2 hover:underline">
            {children}
          </a>
        )
      }

      const safeUrl = safeMarkdownWebUrl(href)
      if (!safeUrl) {
        return (
          <span className="text-muted-foreground" title="This link cannot be opened from chat.">
            {children}<span className="ml-1 text-[10px]">(link unavailable)</span>
          </span>
        )
      }
      return (
        <a
          {...props}
          href={safeUrl}
          rel="noreferrer"
          onClick={(event) => {
            event.preventDefault()
            useAppStore.getState().openInBrowser(safeUrl)
          }}
          className="text-brand underline-offset-2 hover:underline"
        >
          {children}
        </a>
      )
    },
  }
}

/** Live clock for in-flight work (reasoning/turn/tool). Ticks at ~4 Hz while
 * `active` and renders the settled duration once `end` is set. */
function useLiveElapsed(start?: number, end?: number, active?: boolean): string {
  const [, force] = useState(0)
  const startMs = start ?? 0
  const endMs = end ?? 0
  useEffect(() => {
    if (!active || !startMs || endMs) return
    const t = setInterval(() => force((v) => v + 1), 250)
    return () => clearInterval(t)
  }, [active, startMs, endMs])
  const base = endMs && endMs >= startMs ? endMs : startMs ? Date.now() : 0
  const totalMs = base > 0 && base >= startMs ? base - startMs : 0
  if (totalMs < 1000) return totalMs > 0 ? '<1s' : ''
  const s = Math.floor(totalMs / 1000)
  return s >= 60 ? `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s` : `${s}s`
}

/** P64.11 — <ReasoningSubbox />: live ticking stopwatch while the turn streams
 * (`Thinking… 1.8s`, auto-expands); unconditionally auto-collapses to the
 * compact `Thought for 4s` pill the moment the turn settles, so focus returns
 * to the answer. Tabular figures plus a reserved min-height keep CLS at zero
 * while it ticks. */
function ReasoningSubbox({
  items,
  startedAt,
  endedAt,
}: {
  items: string[]
  startedAt?: number
  endedAt?: number
}) {
  const [open, setOpen] = useState(false)
  const live = !endedAt
  const settled = !live
  // Auto-open while the model is actively thinking; unconditionally
  // auto-collapse the moment the turn settles (closed-on-complete).
  const firstRender = useRef(true)
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false
      if (live) setOpen(true)
    }
  }, [live])
  useEffect(() => {
    if (settled) setOpen(false)
  }, [settled])
  const elapsed = useLiveElapsed(startedAt, endedAt, live)
  return (
    <Collapsible open={open} onOpenChange={setOpen} className="mt-2 min-h-[24px]">
      <CollapsibleTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="h-6 gap-1.5 px-2 text-[10px] tabular-nums text-muted-foreground hover:text-foreground"
        >
          <Brain
            className={cn('h-3 w-3 text-violet-300', live && 'animate-pulse')}
          />
          {live ? 'Thinking' : `Thought for ${elapsed || 'a moment'}`}
          {live && elapsed && (
            <span className="font-mono text-[9px] tabular-nums text-muted-foreground/50">
              {elapsed}
            </span>
          )}
          {live && <span className="h-1 w-1 animate-pulse rounded-full bg-violet-300" />}
          <ChevronRight
            className={cn('h-3 w-3 transition-transform', open && 'rotate-90')}
          />
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent className="mt-1 rounded-md border border-violet-500/20 bg-violet-500/5 px-3 py-2">
        {items.map((r, i) => (
          <div key={i} className="space-y-1.5">
            <div className="flex items-center gap-2">
              <span className="h-px w-3 shrink-0 bg-violet-300/40" />
              <span className="font-mono text-[9px] uppercase tracking-wider text-violet-300/60">
                Thought {i + 1}
              </span>
              {live && i === items.length - 1 && (
                <span className="h-1 w-1 animate-pulse rounded-full bg-violet-300" />
              )}
            </div>
            <p className="whitespace-pre-wrap text-[11px] leading-relaxed text-muted-foreground">
              {r.trim()}
            </p>
          </div>
        ))}
      </CollapsibleContent>
    </Collapsible>
  )
}

/** P64.12 — inspectable `<memory_passport>` pill. Renders discreetly in the
 * assistant turn header (counts only); clicking opens the inspector drawer
 * with the exact warm memories, active skills, and governance constraints
 * injected into that turn. Closed by default so it reserves no layout. */
function MemoryPassportPill({
  passport,
}: {
  passport: NonNullable<ChatMessage['passport']>
}) {
  const [open, setOpen] = useState(false)
  const generatedId = useId()
  const id = `memory-passport-${generatedId.replace(/:/g, '')}`
  const memCount = passport.memories.length
  const skillCount = passport.skills.length
  return (
    <div className="mb-2 min-h-[22px]">
      <button
        type="button"
        className="inline-flex items-center gap-1.5 rounded-full border border-teal-500/30 bg-teal-500/5 px-2 py-0.5 text-[10px] text-teal-200/90 transition-colors hover:bg-teal-500/15 hover:text-teal-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-controls={id}
        aria-label={`${open ? 'Hide' : 'Show'} context passport: ${memCount} memories, ${skillCount} skills`}
        title="Inspect the memories, skills, and governance injected into this turn"
      >
        <Fingerprint aria-hidden className="h-3 w-3 shrink-0" />
        <span className="font-mono">memory passport</span>
        <span className="font-mono text-[9px] tabular-nums text-teal-200/60">
          {memCount} mem · {skillCount} skills
        </span>
        <ChevronDown
          aria-hidden
          className={cn('h-3 w-3 transition-transform motion-reduce:transition-none', open && 'rotate-180')}
        />
      </button>
      {open && (
        <div id={id} className="mt-1 rounded-md border border-teal-500/20 bg-teal-500/5 px-3 py-2">
          <p className="text-[10px] uppercase tracking-wider text-teal-200/60">Governance</p>
          <p className="mt-0.5 text-[11px] leading-relaxed text-muted-foreground">
            {passport.governance}
          </p>
          <p className="mt-2 text-[10px] uppercase tracking-wider text-teal-200/60">
            Warm memories ({memCount})
          </p>
          {memCount > 0 ? (
            <ul className="mt-0.5 list-disc space-y-0.5 pl-5 text-[11px] leading-relaxed text-foreground/90">
              {passport.memories.map((m, i) => (
                <li key={i}>{m}</li>
              ))}
            </ul>
          ) : (
            <p className="mt-0.5 text-[11px] text-muted-foreground/70">No warm memories injected.</p>
          )}
          <p className="mt-2 text-[10px] uppercase tracking-wider text-teal-200/60">
            Active skills ({skillCount})
          </p>
          {skillCount > 0 ? (
            <div className="mt-1 flex flex-wrap gap-1">
              {passport.skills.map((s) => (
                <span
                  key={s}
                  className="rounded-full border border-border bg-background/60 px-1.5 py-px font-mono text-[10px] text-foreground/90"
                >
                  {s}
                </span>
              ))}
            </div>
          ) : (
            <p className="mt-0.5 text-[11px] text-muted-foreground/70">No skills active.</p>
          )}
        </div>
      )}
    </div>
  )
}

function TimeStamp({ ts }: { ts: string }) {
  let label = ''
  try {
    const d = new Date(ts)
    label = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  } catch {
    label = ts
  }
  return <span className="font-mono text-[9px] text-muted-foreground/80">{label}</span>
}

/** Raw diagnostics stay behind a named, keyboard-operable disclosure. */
function TechnicalDetails({ children }: { children: React.ReactNode }) {
  const [open, setOpen] = useState(false)
  const generatedId = useId()
  const id = `technical-details-${generatedId.replace(/:/g, '')}`
  return (
    <div className="mt-1.5">
      <button
        type="button"
        className="inline-flex items-center gap-1 rounded px-1.5 py-1 text-[10px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-controls={id}
        aria-label={`${open ? 'Hide' : 'Show'} technical details`}
      >
        Technical details
        <ChevronDown
          aria-hidden
          className={cn('h-3 w-3 transition-transform motion-reduce:transition-none', open && 'rotate-180')}
        />
      </button>
      {open && (
        <div id={id} className="mt-1 rounded-md border border-border/60 bg-background/50 p-2 font-mono text-[10px] text-muted-foreground">
          {children}
        </div>
      )}
    </div>
  )
}

function rawJson(value: unknown): string {
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2) ?? String(value)
  } catch {
    return String(value)
  }
}

/** P52.21 — one message rendered as Markdown (per-message export shares this). */
export function messageMarkdown(m: ChatMessage): string {
  const role =
    m.role === 'user' ? '## You' : m.role === 'assistant' ? '## Assistant' : '## System'
  const body = formatCitationExport(m.content, m.citations ?? [])
  const lines = [role, '', body]
  const technical: string[] = []
  if (m.error) {
    lines.push('', `> The ${m.error.layer} step needs attention.`)
    technical.push(`- layer: ${m.error.layer}`)
    if (m.error.code) technical.push(`- code: ${m.error.code}`)
    technical.push(`- detail: ${m.error.detail}`)
    if (m.error.requestId) technical.push(`- request ID: ${m.error.requestId}`)
  }
  if (m.toolCalls?.length) {
    lines.push('', '### Work activity')
    for (const t of m.toolCalls) {
      const summary = toolActivitySummary(t)
      lines.push(`- ${summary.sentence} (${summary.status}${summary.duration ? `, ${summary.duration}` : ''})`)
      technical.push(`- tool \`${t.toolId}\` — ${t.status}${t.risk ? ` · ${t.risk}` : ''}`)
      if (t.args) technical.push(`  - arguments: \`${rawJson(t.args).replace(/\n/g, ' ')}\``)
      if (t.error) technical.push(`  - error: ${t.error}`)
      else if (t.result != null) technical.push(`  - result: ${rawJson(t.result).replace(/\n/g, ' ')}`)
    }
  }
  if (technical.length > 0) {
    lines.push('', '<details><summary>Technical details</summary>', '', ...technical, '', '</details>')
  }
  if (m.artifacts && m.artifacts.length > 0) {
    lines.push('', `- artifacts: ${m.artifacts.map((a) => a.name).join(', ')}`)
  }
  return lines.join('\n')
}

/** Shared action icon-button classes for the union bar. */
const baseBtn =
  'h-6 w-6 inline-flex items-center justify-center rounded text-muted-foreground/70 transition-all hover:bg-accent hover:text-foreground opacity-0 group-hover/msg:opacity-100 focus:opacity-100'

/** P51.21/P52.22 — shared same-history retry: truncate below `messageId` and
 * re-ask the exact user prompt that produced it (reads in place, never
 * appends a duplicate ask). False when no user turn precedes the message.
 *
 * WP3 — `transform` lets the recovery exits re-ask the same prompt with an
 * explicit instruction (work more cautiously / try a different approach)
 * without inventing a new user turn. */
async function regenerateTurn(
  messageId: string,
  transform?: (prompt: string) => string,
): Promise<boolean> {
  const st = useAppStore.getState()
  const sess = st.sessions.find((s) => s.messages.some((m) => m.id === messageId))
  if (!sess) return false
  const { sendUserMessage } = await import('@/lib/bridge')
  const shape = (p: string) => (transform ? transform(p) : p)
  const prompt = st.rewindBeforeAssistant(sess.id, messageId)
  if (prompt) {
    await sendUserMessage(shape(prompt), undefined, { bypassQueue: true })
    return true
  }
  const priorUser = [...sess.messages]
    .slice(0, sess.messages.findIndex((m) => m.id === messageId))
    .reverse()
    .find((m) => m.role === 'user')
  if (!priorUser) return false
  await sendUserMessage(shape(priorUser.content))
  return true
}

const ERROR_LAYER_LABEL: Record<ChatError['layer'], string> = {
  provider: 'Provider',
  guard: 'Guard',
  tool: 'Tool',
  agent: 'Agent',
  budget: 'Budget',
  runtime: 'Runtime',
}

/** P51.7/P51.21 — layer-named error card for a failed assistant turn. The
 * partial answer stays above; the card says which layer failed, why, and
 * offers the matched actions (Retry when the failure is retryable, Copy). */
function TurnErrorCard({ message }: { message: ChatMessage }) {
  const notify = useAppStore((s) => s.notify)
  const permissionMode = useAppStore((s) => s.permissionMode)
  const setPermissionMode = useAppStore((s) => s.setPermissionMode)
  const [copied, setCopied] = useState(false)
  const [busy, setBusy] = useState(false)
  const err = message.error
  if (!err) return null
  // WP3 — the dial-back affordances. "Safer" is hidden at the safest setting
  // (it would be a no-op), and "Undo" only appears when a tool actually ran. */
  const safer = saferMode(permissionMode)
  const canUndo = hasUndoableWork(message.toolCalls)
  const copy = () => {
    navigator.clipboard?.writeText(err.detail)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }
  const retryWith = (transform?: (p: string) => string, label = 'Retry') => {
    void (async () => {
      setBusy(true)
      try {
        const ok = await regenerateTurn(message.id, transform)
        if (!ok) notify(`Nothing to ${label.toLowerCase()} — no user turn before this message`, 'error')
      } catch (e) {
        notify(e instanceof Error ? e.message : `${label} failed`, 'error')
      } finally {
        setBusy(false)
      }
    })()
  }
  const retry = () => retryWith(undefined, 'Retry')
  const retrySafer = () => {
    if (!safer) return
    setPermissionMode(safer)
    notify(`Working more cautiously now — the ask is repeated under “${safer}”`)
    retryWith(saferPrompt, 'Retry')
  }
  const retryDifferently = () => retryWith(differentlyPrompt, 'Retry')
  const undo = () => {
    void (async () => {
      try {
        const st = useAppStore.getState()
        const sess = st.sessions.find((s) => s.messages.some((m) => m.id === message.id))
        if (!sess) return
        const { agentUndo } = await import('@/lib/tauri')
        await agentUndo(sess.id)
        notify('Undo requested — the run rolls back whatever it applied')
      } catch (e) {
        notify(e instanceof Error ? e.message : 'Undo failed', 'error')
      }
    })()
  }
  return (
    <div className="mt-1.5 rounded-lg border border-rose-500/30 bg-rose-500/5 px-3 py-2">
      <div className="flex items-center gap-1.5">
        <AlertTriangle className="h-3 w-3 shrink-0 text-rose-400" />
        <span className="text-[10px] font-medium uppercase tracking-wider text-rose-300">
          {ERROR_LAYER_LABEL[err.layer]} error
        </span>
      </div>
      {/* P51.2 — localized translation: a plain-language read of the code
          plus a concrete recovery hint, per layer. The raw diagnostic stays
          behind Technical details so the casual surface does not lead with
          implementation text. */}
      {(() => {
        const x = explainError(err)
        if (x.explain === err.detail) {
          return (
            <p className="mt-1 text-[11px] leading-relaxed text-rose-100/70">
              This step could not finish. Open technical details for the exact diagnostic, or try a different approach.
            </p>
          )
        }
        return (
          <p className="mt-1 text-[11px] leading-relaxed text-rose-100/70">
            {x.explain}
            {x.hint ? (
              <>
                {' '}
                <span className="text-rose-100/50">{x.hint}</span>
              </>
            ) : null}
          </p>
        )
      })()}
      <div className="mt-1.5 flex items-center gap-1">
        {err.retryable && (
          <button
            onClick={retry}
            disabled={busy}
            className="inline-flex h-5 items-center gap-1 rounded bg-rose-500/20 px-1.5 text-[10px] text-rose-200 transition-colors hover:bg-rose-500/30 disabled:opacity-50"
          >
            <RotateCw className={cn('h-2.5 w-2.5', busy && 'animate-spin motion-reduce:animate-none')} />
            Retry
          </button>
        )}
        {/* WP3 — the exits that make a failure recoverable rather than final. */}
        {err.retryable && safer && (
          <button
            onClick={retrySafer}
            disabled={busy}
            title={`Repeat this ask with tighter limits (autonomy → ${safer})`}
            className="inline-flex h-5 items-center gap-1 rounded bg-warning/20 px-1.5 text-[10px] text-warning transition-colors hover:bg-warning/30 disabled:opacity-50"
          >
            <ShieldAlert className="h-2.5 w-2.5" />
            Try again safer
          </button>
        )}
        {err.retryable && (
          <button
            onClick={retryDifferently}
            disabled={busy}
            title="Repeat this ask and tell the agent not to repeat the same steps"
            className="inline-flex h-5 items-center gap-1 rounded bg-sky-500/20 px-1.5 text-[10px] text-sky-200 transition-colors hover:bg-sky-500/30 disabled:opacity-50"
          >
            <GitFork className="h-2.5 w-2.5" />
            Try differently
          </button>
        )}
        {canUndo && (
          <button
            onClick={undo}
            title="Roll back whatever this turn applied"
            className="inline-flex h-5 items-center gap-1 rounded bg-brand/20 px-1.5 text-[10px] text-brand transition-colors hover:bg-brand/30"
          >
            <RotateCw className="h-2.5 w-2.5 scale-x-[-1]" />
            Undo
          </button>
        )}
        <button
          onClick={copy}
          className="inline-flex h-5 items-center gap-1 rounded bg-rose-500/10 px-1.5 text-[10px] text-rose-200/90 transition-colors hover:bg-rose-500/20"
        >
          {copied ? <Check className="h-2.5 w-2.5 text-emerald-400" /> : <Copy className="h-2.5 w-2.5" />}
          {copied ? 'Copied' : 'Copy error'}
        </button>
      </div>
      <TechnicalDetails>
        <div className="space-y-1">
          <div>Layer: {err.layer}</div>
          {err.code && <div>Code: {err.code}</div>}
          <div className="whitespace-pre-wrap break-words">Detail: {err.detail}</div>
          {err.requestId && (
            <div className="flex flex-wrap items-center gap-2">
              <span className="break-all">Request ID: {err.requestId}</span>
              <button
                type="button"
                className="rounded border border-border px-1.5 py-0.5 text-[9px] text-foreground hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
                onClick={() => {
                  navigator.clipboard?.writeText(err.requestId!)
                  notify('Request ID copied — include it when reporting this turn')
                }}
              >
                Copy request ID
              </button>
            </div>
          )}
        </div>
      </TechnicalDetails>
    </div>
  )
}

/** P52.24/P52.22 — assistant-message action bar: copy · quote-to-composer ·
 * same-history regenerate · fork · export-as-Markdown. Speak uses the
 * platform speechSynthesis engine when present (P50.4.4); otherwise the
 * button stays disabled with an honest title. */
function AssistantActions({ message }: { message: ChatMessage }) {
  const [copied, setCopied] = useState(false)
  const [regenerating, setRegenerating] = useState(false)
  const notify = useAppStore((s) => s.notify)
  const setComposerValue = useAppStore((s) => s.setComposerValue)
  const forkFromMessage = useAppStore((s) => s.forkFromMessage)

  const copy = () => {
    navigator.clipboard?.writeText(message.content)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }
  const quote = () => {
    setComposerValue(`> ${message.content.replace(/\n+/g, '\n> ').slice(0, 400)}\n\n`)
    notify('Quoted — keep typing or press Enter to send')
  }
  const exportMd = () => {
    const blob = new Blob([messageMarkdown(message)], { type: 'text/markdown' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `message-${message.id.slice(-8)}.md`
    a.click()
    URL.revokeObjectURL(url)
    notify('Message exported as Markdown')
  }

  // P52.22 — same-history regenerate: drop this answer (and anything below)
  // and re-ask the exact user prompt that produced it, so the corrected run
  // reads in place. Falls back to a plain re-ask if the seam is unavailable.
  const regenerate = () => {
    void (async () => {
      setRegenerating(true)
      try {
        const ok = await regenerateTurn(message.id)
        if (!ok) notify('Nothing to regenerate — no user turn before this message', 'error')
      } catch (e) {
        notify(e instanceof Error ? e.message : 'Regenerate failed', 'error')
      } finally {
        setRegenerating(false)
      }
    })()
  }

  return (
    <div className="flex items-center gap-0.5 px-1">
      <button className={baseBtn} onClick={copy} title="Copy message">
        {copied ? <Check className="h-3 w-3 text-emerald-400" /> : <Copy className="h-3 w-3" />}
      </button>
      <button className={baseBtn} onClick={quote} title="Quote into composer">
        <Quote className="h-3 w-3" />
      </button>
      <button className={baseBtn} onClick={regenerate} title="Regenerate — drop this answer and re-ask its prompt">
        <RotateCw className={cn('h-3 w-3', regenerating && 'animate-spin')} />
      </button>
      <button
        className={baseBtn}
        onClick={() => forkFromMessage(message.id)}
        title="Fork conversation from here"
      >
        <GitFork className="h-3 w-3" />
      </button>
      <button className={baseBtn} onClick={exportMd} title="Export this message as Markdown">
        <Download className="h-3 w-3" />
      </button>
      <button
        className={cn(baseBtn, !speechSynthesisAvailable() && 'cursor-not-allowed opacity-30 hover:bg-transparent hover:text-muted-foreground/70')}
        title={
          speechSynthesisAvailable()
            ? 'Read this message aloud'
            : 'Read aloud needs a platform speech engine (speechSynthesis) — none is available here'
        }
        aria-disabled={!speechSynthesisAvailable()}
        onClick={() => {
          if (!speechSynthesisAvailable()) return
          stopSpeaking()
          speakText(message.content)
        }}
      >
        <Volume2 className="h-3 w-3" />
      </button>
    </div>
  )
}

/** P52.22 — inline correction of a user message. Editing rewinds the
 * transcript to just before that ask (truncate-below, no rewrite of what
 * came above) and hands the corrected text to the composer as a fresh real
 * turn — so the fix is visible, never a silent history edit. */
function UserEditInline({
  sessionId,
  message,
  onDone,
}: {
  sessionId: string
  message: ChatMessage
  onDone: () => void
}) {
  const notify = useAppStore((s) => s.notify)
  const [draft, setDraft] = useState(message.content)
  const save = () => {
    const st = useAppStore.getState()
    const text = draft.trim()
    if (!text) {
      notify('Message cannot be empty', 'error')
      return
    }
    // Truncate below this ask and re-dispatch the corrected text.
    if (st.rewindToUserMessage(sessionId, message.id) === null) {
      notify('Could not edit — this message is not the last turn', 'error')
      return
    }
    void (async () => {
      try {
        const { sendUserMessage } = await import('@/lib/bridge')
        await sendUserMessage(text, undefined, { bypassQueue: true })
      } catch (e) {
        notify(e instanceof Error ? e.message : 'Re-ask failed', 'error')
      }
    })()
    onDone()
  }
  return (
    <div className="w-full">
      <textarea
        autoFocus
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
            e.preventDefault()
            save()
          }
          if (e.key === 'Escape') {
            e.stopPropagation()
            onDone()
          }
        }}
        className="max-h-48 w-full resize-y rounded-md border border-border bg-background/60 px-2 py-1.5 text-[12px] leading-relaxed text-foreground focus:border-brand/50 focus:outline-none"
        rows={Math.min(6, Math.max(2, message.content.split('\n').length))}
      />
      <div className="mt-1 flex items-center gap-1.5">
        <button
          type="button"
          onClick={save}
          className="rounded bg-brand px-2 py-0.5 text-[10px] font-medium text-white hover:bg-brand-hover"
        >
          Edit & re-ask
        </button>
        <button
          type="button"
          onClick={onDone}
          className="rounded px-2 py-0.5 text-[10px] text-muted-foreground hover:text-foreground"
        >
          Cancel
        </button>
        <span className="font-mono text-[9px] text-muted-foreground/60">
          ⌘⏎ send · esc cancel
        </span>
      </div>
    </div>
  )
}


interface Props {
  message: ChatMessage
  /** When true, append a blinking orange caret to the streamed text */
  streaming?: boolean
}

/**
 * P64.7 — per-turn checkpoint affordance under a mutating assistant message.
 * Lazy: the shell snapshot list loads only when the user opens Details, so a
 * long transcript does not fan out one IPC call per turn. Eager data lives in
 * the timeline; this row is the in-context entry point to the same restore.
 */
function AssistantCheckpoint({ message }: { message: ChatMessage }) {
  const sessionId = useAppStore((s) => s.activeSessionId)
  const { summary, turnIndex } = useMemo(() => {
    const st = useAppStore.getState()
    const sess = st.sessions.find((s) => s.id === st.activeSessionId)
    const assistants = (sess?.messages ?? []).filter((m) => m.role === 'assistant')
    const idx = assistants.findIndex((m) => m.id === message.id)
    return {
      summary: checkpointSummary(message),
      turnIndex: idx >= 0 ? idx + 1 : assistants.length > 0 ? assistants.length : 1,
    }
  }, [message])
  if (!sessionId) return null
  return (
    <div className="mt-1.5">
      <TurnCheckpoint
        sessionId={sessionId}
        messageId={message.id}
        timestamp={message.timestamp}
        summary={summary}
        turnIndex={turnIndex}
      />
    </div>
  )
}

// P45.9 — memoized: store updates are immutable (untouched messages keep
// identity), so a shallow compare re-renders only the message whose object
// changed (the one streaming). Custom comparison is avoided: `streaming` is a
// primitive per-message flag, so the default shallow prop compare is exact.
const MessageBubble = memo(function MessageBubble({ message, streaming }: Props) {
  // P52.22 — user-message inline edit (rewind + re-ask). State lives at the
  // top so every bubble (any role) renders the same hook order.
  const [editing, setEditing] = useState(false)

  if (message.role === 'system') {
    return (
      <div className="fade-up my-2 flex justify-center">
        <div className="rounded-full border border-border bg-background/40 px-3 py-1 text-center text-[11px] italic text-muted-foreground">
          {message.content}
        </div>
      </div>
    )
  }

  if (message.role === 'user') {
    const sessionId = useAppStore.getState().activeSessionId
    return (
      <div className="fade-up flex flex-row-reverse gap-2.5">
        <Avatar className="h-6 w-6 shrink-0 border border-border bg-secondary">
          <AvatarFallback className="bg-secondary text-muted-foreground">
            <User className="h-3.5 w-3.5" />
          </AvatarFallback>
        </Avatar>
        <div className="flex max-w-[78%] flex-col items-end gap-1">
          {editing ? (
            <div className="w-full rounded-2xl rounded-tr-sm border border-brand/30 bg-secondary px-2 py-2">
              <UserEditInline
                sessionId={sessionId}
                message={message}
                onDone={() => setEditing(false)}
              />
            </div>
          ) : (
            <div className="rounded-2xl rounded-tr-sm bg-secondary px-3 py-2 text-[12px] leading-relaxed text-foreground">
              {message.content}
            </div>
          )}
          <div className="flex items-center gap-1">
            <TimeStamp ts={message.timestamp} />
            {/* P52.22 — correct an ask in place (rewind + re-ask). */}
            <button
              className="rounded p-0.5 text-muted-foreground/70 hover:text-foreground"
              title="Edit this ask — rewinds the conversation to here and re-asks the corrected text"
              onClick={() => setEditing(true)}
            >
              <Pencil className="h-3 w-3" />
            </button>
            <button
              className="rounded p-0.5 text-muted-foreground/70 hover:text-foreground"
              title="Fork from here"
              onClick={() => useAppStore.getState().forkFromMessage(message.id)}
            >
              <GitFork className="h-3 w-3" />
            </button>
          </div>
        </div>
      </div>
    )
  }

  // assistant
  return (
    <div className="group/msg fade-up flex gap-2.5">
      <Avatar className="h-6 w-6 shrink-0 border border-brand/30 bg-brand/15">
        <AvatarFallback className="bg-transparent text-brand">
          <Sparkles className="h-3.5 w-3.5" />
        </AvatarFallback>
      </Avatar>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="max-w-full rounded-2xl rounded-tl-sm border border-border bg-card/60 px-3 py-2">
          {/* P64.12 — inspectable context-passport pill (turn header). */}
          {message.passport && <MemoryPassportPill passport={message.passport} />}
          {/* P64.11 tier 1 — reasoning sub-box (closed-on-complete). */}
          {message.reasoning && message.reasoning.length > 0 && (
            <ReasoningSubbox
              items={message.reasoning}
              startedAt={message.reasoningStartedAt}
              endedAt={message.endedAt}
            />
          )}
          {/* P64.11 tier 2 — grouped tool execution drawer + progress (closed once settled). */}
          {message.toolCalls && message.toolCalls.length > 0 && (
            <ToolChips calls={message.toolCalls} />
          )}
          {message.steps && message.steps.length > 0 && (
            <ProgressSteps steps={message.steps} />
          )}
          {/* P64.11 tier 3 — response body. */}
          <div className="prose prose-invert max-w-none">
            <ReactMarkdown
              remarkPlugins={[
                remarkGfm,
                [remarkArtifactReferences, { artifacts: message.artifacts ?? [] }],
                [remarkCitationReferences, {
                  messageId: message.id,
                  indexes: (message.citations ?? []).map((citation) => citation.index),
                }],
                remarkMath,
              ]}
              rehypePlugins={[[rehypeKatex, katexOptions], rehypeHighlight]}
              components={markdownComponentsFor(
                new Set((message.citations ?? []).map((citation) => citationAnchorId(citation.index, message.id))),
                new Map((message.artifacts ?? []).map((artifact) => [artifact.id, artifact])),
              )}
            >
              {applyCitationMarks(message.content, message.citations ?? [])}
            </ReactMarkdown>
            {streaming && (
              <span className="caret-blink ml-0.5 inline-block h-3.5 w-[2px] translate-y-0.5 rounded-sm bg-brand" />
            )}
          </div>

        </div>

        {message.error && <TurnErrorCard message={message} />}

        {message.citations && message.citations.length > 0 && (
          <ol className="mt-1 space-y-0.5 rounded-md border border-border/50 bg-background/40 px-2 py-1.5 text-[11px]">
            {message.citations.map((c) => (
              <li key={c.index} id={citationAnchorId(c.index, message.id)} className="flex gap-1.5">
                <span className="font-mono text-muted-foreground">[^{c.index}]</span>
                {/* A citation opens in the rail's Browse surface, not a bare
                    webview navigation: `openInBrowser` routes through
                    `browser_navigate`, which is a Guard/netfloor-mediated
                    command, and it uses the app profile rather than handing the
                    URL to the user's default browser. */}
                <button
                  type="button"
                  onClick={() => useAppStore.getState().openInBrowser(c.url)}
                  className="min-w-0 truncate text-left text-brand underline-offset-2 hover:underline"
                  title={c.snippet ?? c.url}
                >
                  {c.title}
                </button>
              </li>
            ))}
          </ol>
        )}

        {/* P64.11 tier 4 — approvals, checkpoints, and artifact cards live
            below the response body (TurnErrorCard above, checkpoint +
            artifacts + MCQ below). */}
        {isMutatingMessage(message) && !streaming && (
          <AssistantCheckpoint message={message} />
        )}

        {message.artifacts && message.artifacts.length > 0 && (
          <div className="grid gap-2 sm:grid-cols-2">
            {message.artifacts.map((a, i) => (
              // P35.2 — entrance stagger on artifact cards.
              <div key={a.id} className="enter-stagger" style={staggerStyle(i)}>
                <ArtifactCard artifact={a} />
              </div>
            ))}
          </div>
        )}

        {message.mcq && <McqInterruptCard mcq={message.mcq} />}

        <div className="flex items-center gap-2 px-1">
          <TimeStamp ts={message.timestamp} />
          {message.ttfbMs !== undefined && message.endedAt && (
            <span className="font-mono text-[9px] text-muted-foreground/50">
              first token {(message.ttfbMs / 1000).toFixed(1)}s
            </span>
          )}
          {message.pinned && (
            <span className="font-mono text-[9px] text-brand/70">pinned</span>
          )}
          <AssistantActions message={message} />
        </div>
      </div>
    </div>
  )
})

export default MessageBubble
