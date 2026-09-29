'use client'

import { useEffect, useState } from 'react'
import {
  Check,
  Code,
  Copy,
  Download,
  ExternalLink,
  File,
  FileSpreadsheet,
  FileText,
  Image as ImageIcon,
  Loader2,
  MonitorSmartphone,
  Presentation,
  X,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { useAppStore, type Artifact } from '@/lib/store'
import { cn } from '@/lib/utils'
import { preciseFigures } from '@/lib/plain-language'

const TYPE_ACCENT: Record<Artifact['type'], string> = {
  webapp: 'text-emerald-400',
  xlsx: 'text-emerald-400',
  docx: 'text-sky-300',
  pptx: 'text-brand',
  pdf: 'text-rose-400',
  code: 'text-violet-300',
  markdown: 'text-warning',
  image: 'text-fuchsia-300',
}

function TypeIcon({ type, className }: { type: Artifact['type']; className?: string }) {
  const cls = cn('h-3.5 w-3.5', TYPE_ACCENT[type], className)
  switch (type) {
    case 'xlsx':
      return <FileSpreadsheet className={cls} />
    case 'docx':
      return <FileText className={cls} />
    case 'pptx':
      return <Presentation className={cls} />
    case 'pdf':
      return <FileText className={cls} />
    case 'webapp':
      return <MonitorSmartphone className={cls} />
    case 'code':
      return <Code className={cls} />
    case 'image':
      return <ImageIcon className={cls} />
    default:
      return <File className={cls} />
  }
}

function ArtifactTile({ artifact }: { artifact: Artifact }) {
  if (artifact.type === 'webapp') {
    const state = artifact.server?.status
    return (
      <div className="relative flex aspect-video w-full items-center justify-center overflow-hidden rounded-xl border border-border bg-gradient-to-br from-emerald-950/70 via-zinc-950 to-brand/10">
        <div className="absolute inset-x-0 top-0 flex h-7 items-center gap-1.5 border-b border-border/60 bg-background/20 px-3">
          <span className="size-2 rounded-full bg-red-400/70" />
          <span className="size-2 rounded-full bg-warning/70" />
          <span className="size-2 rounded-full bg-emerald-400/70" />
        </div>
        <div className="flex flex-col items-center gap-2 pt-5 text-center">
          <MonitorSmartphone className="h-7 w-7 text-emerald-300/80" aria-hidden />
          <span className="text-xs font-medium text-foreground">Interactive app</span>
          <span className="font-mono text-[10px] text-muted-foreground">
            {state === 'serving' && artifact.server
              ? artifact.server.demo
                ? 'Preview simulation'
                : `Preview ready · 127.0.0.1:${artifact.server.port}`
              : state === 'stopped' ? 'Preview is stopped' : 'Preview is not available yet'}
          </span>
        </div>
      </div>
    )
  }

  const label: Record<Exclude<Artifact['type'], 'webapp'>, string> = {
    xlsx: 'Spreadsheet',
    docx: 'Document',
    pptx: 'Presentation',
    pdf: 'PDF document',
    code: 'Code file',
    markdown: 'Markdown file',
    image: 'Image file',
  }
  const officeType = ['xlsx', 'docx', 'pptx', 'pdf'].includes(artifact.type)
  const hasSupportedOfficePath = Boolean(
    officeType && artifact.path && /\.(xlsx|xlsm|docx|pptx|pdf)$/i.test(artifact.path),
  )
  const canOpen = Boolean(artifact.view || hasSupportedOfficePath)

  return (
    <div className="flex aspect-video w-full items-center justify-center gap-3 rounded-xl border border-border bg-gradient-to-br from-card via-muted/40 to-brand/5 px-4">
      <span className="grid size-12 shrink-0 place-items-center rounded-2xl border border-border bg-background/70 shadow-sm">
        <TypeIcon type={artifact.type} className="h-6 w-6" />
      </span>
      <div className="min-w-0">
        <div className="truncate text-sm font-medium text-foreground">{label[artifact.type]}</div>
        <div className="mt-1 text-[10px] text-muted-foreground">
          {canOpen ? 'Open to view the actual file' : 'File reference or viewer is not available yet'}
        </div>
      </div>
    </div>
  )
}

interface Props {
  artifact: Artifact
}

export default function ArtifactCard({ artifact }: Props) {
  const activeView = useAppStore((s) => s.activeView)
  const setActiveView = useAppStore((s) => s.setActiveView)
  const notify = useAppStore((s) => s.notify)
  const isLive = artifact.view && artifact.view === activeView
  // P32.3 (corrected) — figures the run actually reported. Empty ⇒ no badge.
  const figures = preciseFigures(artifact)

  const openArtifact = () => {
    const officeType = ['xlsx', 'docx', 'pptx', 'pdf'].includes(artifact.type)
    const officePath = artifact.path && /\.(xlsx|xlsm|docx|pptx|pdf)$/i.test(artifact.path)
      ? artifact.path
      : undefined

    if (officeType || officePath) {
      if (officePath) {
        useAppStore.getState().openOfficeDoc(officePath)
      } else if (artifact.view) {
        setActiveView(artifact.view)
      } else {
        notify(`“${artifact.name}” has no available file location or viewer yet`, 'error')
      }
      return
    }
    if (artifact.view) {
      setActiveView(artifact.view)
      return
    }
    notify(`No viewer for “${artifact.name}” yet — use Save to download it`, 'error')
  }

  return (
    <Card
      onClick={() => openArtifact()}
      className="group cursor-pointer gap-0 overflow-hidden border-border bg-card/60 p-0 transition-colors hover:border-brand/40"
    >
      <div className="flex items-center justify-between border-b border-border px-3 py-2">
        <div className="flex min-w-0 items-center gap-2">
          <TypeIcon type={artifact.type} />
          <span className="truncate font-mono text-xs text-foreground">{artifact.name}</span>
        </div>
        {isLive && (
          <Badge
            variant="outline"
            className="gap-1 border-brand/40 bg-brand/10 text-[10px] text-brand"
          >
            <span className="live-dot h-1.5 w-1.5 rounded-full bg-brand" />
            Live
          </Badge>
        )}
        {/* P32.3 (corrected) — only figures this run actually reported. No
            receipt figures means no badge at all; earlier revisions invented
            sample counts under a tooltip that claimed receipt provenance. */}
        {figures.length > 0 && (
          <Badge
            variant="outline"
            className="ml-auto shrink-0 border-emerald-500/30 bg-emerald-500/5 font-mono text-[9px] text-emerald-300"
            title="Figures reported by this run"
          >
            {figures.join(' · ')}
          </Badge>
        )}
      </div>

      <div className="px-3 pt-2.5 pb-3">
        <ArtifactTile artifact={artifact} />
        <p className="mt-2 truncate font-mono text-[10px] text-muted-foreground">
          {artifact.preview}
        </p>

        {/* P15-H29 — inline artifact action checklist (bolt.diy Artifact.tsx
            pattern): auto-expands while any action is running, collapses to
            a progress line when everything is terminal. */}
        {artifact.actions && artifact.actions.length > 0 && (
          <div className="mt-2 border-t border-border pt-2">
            <ActionChecklist
              key={artifact.id}
              actions={artifact.actions}
              running={
                artifact.actions.some((a) => a.state === 'running' || a.state === 'pending')
              }
            />
          </div>
        )}

        <div className="mt-2.5 flex items-center gap-1 border-t border-border pt-2">
          <Button
            size="sm"
            variant="ghost"
            className="h-7 gap-1 px-2 text-[11px] text-muted-foreground hover:text-foreground"
            onClick={(e) => {
              e.stopPropagation()
              const src = artifact.path ?? artifact.preview
              void navigator.clipboard
                ?.writeText(src)
                .then(() => notify('Artifact reference copied'))
                .catch(() => notify('Copy failed — clipboard unavailable', 'error'))
            }}
          >
            <Code className="h-3 w-3" />
            Source
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-7 gap-1 px-2 text-[11px] text-muted-foreground hover:text-foreground"
            onClick={(e) => {
              e.stopPropagation()
              void navigator.clipboard
                ?.writeText(artifact.preview)
                .then(() => notify('Copied to clipboard'))
                .catch(() => notify('Copy failed — clipboard unavailable', 'error'))
            }}
          >
            <Copy className="h-3 w-3" />
            Copy
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-7 gap-1 px-2 text-[11px] text-muted-foreground hover:text-foreground"
            onClick={(e) => {
              e.stopPropagation()
              const blob = new Blob([artifact.preview], { type: 'text/plain' })
              const url = URL.createObjectURL(blob)
              const a = document.createElement('a')
              a.href = url
              a.download = artifact.name.replace(/[^\w\-. ]+/g, '').trim() || 'artifact.txt'
              a.click()
              URL.revokeObjectURL(url)
              notify('Artifact saved')
            }}
          >
            <Download className="h-3 w-3" />
            Save
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="ml-auto h-7 gap-1 px-2 text-[11px] text-brand hover:text-brand"
            onClick={(e) => {
              e.stopPropagation()
              openArtifact()
            }}
          >
            Open
            <ExternalLink className="h-3 w-3" />
          </Button>
        </div>
      </div>
    </Card>
  )
}

/* ---- P15-H29 inline action checklist ---- */

function ActionChecklist({
  actions,
  running,
}: {
  actions: NonNullable<Artifact['actions']>
  running: boolean
}) {
  const done = actions.filter((a) => a.state === 'complete').length
  const failed = actions.filter((a) => a.state === 'failed').length
  // Auto-expand while running; collapse to a one-line progress summary once
  // everything is terminal (bolt.diy Artifact.tsx behavior).
  const [expanded, setExpanded] = useState(running)
  useEffect(() => {
    if (running) setExpanded(true)
  }, [running])

  return (
    <div className="rounded-md border border-border/70 bg-background/40">
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        className="flex w-full items-center gap-2 px-2.5 py-1.5 text-left"
      >
        {running ? (
          <Loader2 className="h-3 w-3 animate-spin text-brand" />
        ) : failed > 0 ? (
          <X className="h-3 w-3 text-rose-400" />
        ) : (
          <Check className="h-3 w-3 text-emerald-400" />
        )}
        <span className="text-[10px] font-medium text-foreground">
          Actions {done}/{actions.length}
        </span>
        {failed > 0 && (
          <span className="text-[10px] text-rose-400">{failed} failed</span>
        )}
        <span className="ml-auto font-mono text-[9px] text-muted-foreground">
          {expanded ? '−' : '+'}
        </span>
      </button>
      {expanded && (
        <ol className="space-y-0.5 border-t border-border/70 px-2.5 py-1.5">
          {actions.map((a) => (
            <li key={a.index} className="flex items-center gap-2 text-[10px]">
              <span
                className={cn(
                  'flex size-3.5 shrink-0 items-center justify-center rounded-full border',
                  a.state === 'complete' && 'border-emerald-500/40 bg-emerald-500/10 text-emerald-400',
                  a.state === 'running' && 'border-brand/40 bg-brand/10 text-brand',
                  a.state === 'failed' && 'border-rose-500/40 bg-rose-500/10 text-rose-400',
                  a.state === 'aborted' && 'border-muted-foreground/40 text-muted-foreground',
                  a.state === 'pending' && 'border-muted-foreground/30 text-muted-foreground/50'
                )}
              >
                {a.state === 'complete' && <Check className="h-2 w-2" />}
                {a.state === 'running' && <Loader2 className="h-2 w-2 animate-spin" />}
                {a.state === 'failed' && <X className="h-2 w-2" />}
                {a.state === 'aborted' && <X className="h-2 w-2" />}
              </span>
              <span
                className={cn(
                  'truncate font-mono',
                  a.state === 'failed' ? 'text-rose-300/90' : 'text-foreground/80',
                  a.state === 'pending' && 'text-muted-foreground/60'
                )}
              >
                {a.label}
              </span>
              {a.state === 'failed' && a.formatted && (
                <span className="ml-auto shrink-0 font-mono text-[9px] text-rose-400/80">
                  {a.formatted}
                </span>
              )}
            </li>
          ))}
        </ol>
      )}
    </div>
  )
}
