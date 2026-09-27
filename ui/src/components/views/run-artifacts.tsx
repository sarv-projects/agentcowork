'use client'

// Files the run produced, with the size the disk actually reports and an
// open action that routes through the file commands the shell already uses.
//
// Honesty rules this section keeps:
//   • A file whose path resolves to nothing on this machine says **"not on this
//     machine"** and its open control is disabled — never a button that opens
//     an empty editor.
//   • Size comes from a real `fs_list_dir` of the file's own directory. When
//     that read is impossible the size cell is a dash, not a guess.
//   • Only paths the run actually reported are listed. Nothing is inferred from
//     a name in the transcript.

import { useEffect, useMemo, useState } from 'react'
import { ExternalLink, FolderOpen, TriangleAlert } from 'lucide-react'
import { useAppStore } from '@/lib/store'
import { inTauri } from '@/lib/tauri'
import { fsListDir } from '@/lib/fs'
import {
  ArtifactKindIcon,
  artifactKind,
  artifactKindLabel,
  formatBytes,
  resolveRunPath,
  splitRunPath,
  type ArtifactKind,
} from '@/components/views/run-projection'

/** Kinds the Office engines own — the existing `openOfficeDoc` route. */
const OFFICE_EXTENSIONS = new Set(['docx', 'xlsx', 'xlsm', 'pptx', 'pdf'])

export interface ProducedFile {
  /** Stable key: the reported path. */
  key: string
  name: string
  /** Absolute path, or `null` when a relative path had no folder to join. */
  path: string | null
  /** The path exactly as the run reported it, for the title attribute. */
  reportedPath: string
  kind: ArtifactKind
  /** True when a real listing found the file on this machine. */
  present: boolean | null
  /** Real byte size from the listing, or `null`. */
  size: number | null
  /**
   * Figures **this run reported** for the artifact (cells changed, slides
   * written…). Absent stays absent: the row renders no figures at all rather
   * than a plausible-looking stand-in (`ARCH/UI.md` §2, I15).
   */
  figures?: string[]
}

/** Everything the run reported as a produced file, de-duplicated by path. */
export function collectProducedFiles(args: {
  artifacts: { name: string; path?: string; type?: string; figures?: string[] }[]
  touchedFiles: readonly string[]
  workingDir: string | null
}): ProducedFile[] {
  const seen = new Set<string>()
  const out: ProducedFile[] = []
  const push = (rawPath: string, declaredType?: string, figures?: string[]) => {
    const key = rawPath.trim()
    if (!key) return
    const path = resolveRunPath(key, args.workingDir)
    // Dedupe on the **resolved** path when there is one: a card that reports
    // `/root/out/a.md` and a `file_touched` that reports `a.md` are the same
    // file, and one file gets one row.
    const identity = path ?? key
    if (seen.has(identity)) {
      // A later report of the same file may carry the figures the first one
      // omitted; keep the richer record rather than dropping the fact.
      const existing = out.find((f) => f.key === identity)
      if (existing && !existing.figures?.length && figures?.length) {
        existing.figures = [...figures]
      }
      return
    }
    seen.add(identity)
    const name = key.replace(/\\/g, '/').split('/').pop() ?? key
    out.push({
      key: identity,
      name,
      path,
      reportedPath: key,
      kind: artifactKind(name, declaredType),
      present: null,
      size: null,
      ...(figures && figures.length > 0 ? { figures: [...figures] } : {}),
    })
  }
  for (const a of args.artifacts) push(a.path ?? a.name, a.type, a.figures)
  for (const f of args.touchedFiles) push(f)
  return out
}

/** `dir\0base` — the cache key for one file's on-disk listing result. */
function probeKey(path: string): string {
  const { dir, base } = splitRunPath(path)
  return `${dir}\u0000${base}`
}

export function RunArtifacts({
  workingDir,
  artifacts,
  touchedFiles,
}: {
  workingDir: string | null
  artifacts: { name: string; path?: string; type?: string; figures?: string[] }[]
  touchedFiles: readonly string[]
}) {
  const openOfficeDoc = useAppStore((s) => s.openOfficeDoc)
  const addView = useAppStore((s) => s.addView)
  const notify = useAppStore((s) => s.notify)

  const files = useMemo(
    () => collectProducedFiles({ artifacts, touchedFiles, workingDir }),
    [artifacts, touchedFiles, workingDir],
  )

  // One directory read per distinct parent, so N produced files in one folder
  // cost one IPC call. Results live in component state; nothing is persisted.
  const [probed, setProbed] = useState<Record<string, { size: number | null }>>({})
  const probeSignature = files.map((f) => (f.path ? probeKey(f.path) : '')).join('|')

  useEffect(() => {
    // Outside the shell `fsListDir` answers with a demo tree, so no read is
    // attempted and every row honestly reads "size unknown".
    if (!inTauri() || probeSignature === '') return
    let cancelled = false
    const byDir = new Map<string, Set<string>>()
    for (const f of files) {
      if (!f.path) continue
      const { dir, base } = splitRunPath(f.path)
      const set = byDir.get(dir) ?? new Set<string>()
      set.add(base)
      byDir.set(dir, set)
    }
    void (async () => {
      const next: Record<string, { size: number | null }> = {}
      await Promise.all(
        [...byDir.entries()].map(async ([dir, bases]) => {
          try {
            const listing = await fsListDir(dir)
            if (cancelled) return
            for (const base of bases) {
              const hit = listing.entries.find((e) => e.name === base && !e.dir)
              next[`${dir}\u0000${base}`] = { size: hit?.size ?? null }
            }
          } catch {
            // A directory we cannot list proves nothing about the file: leave
            // the key absent so the row reads "size unknown", not "missing".
          }
        }),
      )
      if (!cancelled) setProbed(next)
    })()
    return () => {
      cancelled = true
    }
    // `probeSignature` is the stable identity of the probe set.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [probeSignature])

  if (files.length === 0) {
    return (
      <p className="rounded-md border border-dashed border-border px-2 py-2 text-[10px] leading-relaxed text-muted-foreground">
        No files reported yet. A file appears here only when the run states it wrote or touched one.
      </p>
    )
  }

  const open = (file: ProducedFile) => {
    if (!file.path) {
      notify('That path has no folder to resolve against — attach a folder first', 'error')
      return
    }
    const ext = file.name.split('.').pop()?.toLowerCase() ?? ''
    if (OFFICE_EXTENSIONS.has(ext)) {
      openOfficeDoc(file.path)
      return
    }
    // The shell's existing route into the code workbench for a text file.
    window.dispatchEvent(
      new CustomEvent('agentcowork:open-file', { detail: { path: file.path, content: '' } }),
    )
    addView('code')
  }

  return (
    <ul data-testid="run-artifacts" className="space-y-1">
      {files.map((file) => {
        const probe = file.path ? probed[probeKey(file.path)] : undefined
        const present = probe ? probe.size !== null : null
        const isOffice = OFFICE_EXTENSIONS.has(file.name.split('.').pop()?.toLowerCase() ?? '')
        const canOpen = file.path !== null && present !== false
        return (
          <li
            key={file.key}
            data-testid="run-artifact-row"
            className="flex items-center gap-2 rounded-md border border-border/60 bg-background/30 px-2 py-1.5"
          >
            <ArtifactKindIcon kind={file.kind} className="h-3.5 w-3.5 text-muted-foreground" />
            <span className="min-w-0 flex-1">
              <span className="flex items-baseline gap-1.5">
                <span
                  className="min-w-0 truncate text-[10.5px] text-foreground/90"
                  title={file.reportedPath}
                >
                  {file.name}
                </span>
                <span className="shrink-0 font-mono text-[8.5px] uppercase tracking-wide text-muted-foreground/60">
                  {artifactKindLabel(file.kind)}
                </span>
              </span>
              <span className="mt-0.5 flex items-center gap-1.5 font-mono text-[9px] tabular-nums text-muted-foreground/70">
                {present === false ? (
                  <span className="inline-flex items-center gap-1 text-warning">
                    <TriangleAlert aria-hidden className="h-2.5 w-2.5" />
                    not on this machine
                  </span>
                ) : (
                  <>
                    <span className="shrink-0">
                      {formatBytes(probe?.size ?? null) ?? 'size unknown'}
                    </span>
                    {file.path ? (
                      <span className="truncate opacity-70" title={file.path}>
                        · {file.path}
                      </span>
                    ) : (
                      <span className="truncate text-warning">· no folder resolves this path</span>
                    )}
                  </>
                )}
              </span>
              {file.figures && file.figures.length > 0 ? (
                <span className="mt-0.5 flex flex-wrap gap-1">
                  {file.figures.map((fig) => (
                    <span
                      key={fig}
                      className="rounded-full border border-border bg-background/50 px-1.5 py-px font-mono text-[8.5px] text-muted-foreground"
                    >
                      {fig}
                    </span>
                  ))}
                </span>
              ) : null}
            </span>
            <button
              type="button"
              onClick={() => open(file)}
              disabled={!canOpen}
              aria-label={
                canOpen
                  ? `Open ${file.name}`
                  : `Cannot open ${file.name} — it is not on this machine`
              }
              title={
                canOpen
                  ? isOffice
                    ? 'Open in the matching Office viewer'
                    : 'Open in the code workbench'
                  : file.path
                    ? 'Not on this machine — nothing to open'
                    : 'Attach a folder so a relative path can be resolved'
              }
              className="grid h-6 w-6 shrink-0 place-items-center rounded-md border border-border text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {isOffice ? (
                <FolderOpen aria-hidden className="h-3 w-3" />
              ) : (
                <ExternalLink aria-hidden className="h-3 w-3" />
              )}
            </button>
          </li>
        )
      })}
    </ul>
  )
}
