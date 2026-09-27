'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  Folder,
  FolderOpen,
  File,
  FileSpreadsheet,
  FileText,
  Presentation,
  ChevronRight,
  ChevronDown,
  HardDrive,
  ArrowLeft,
  RefreshCw,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'
import { fsHome, fsListDir, fsReadFile, type FsEntry, type FsList } from '@/lib/fs'
import { useAppStore } from '@/lib/store'
import { SkeletonBlock } from '@/components/ui/loading-state'

/**
 * P11.5.3 — folder view over the real disk. The tree lazy-loads each
 * directory from `fs_list_dir` (dirs first, alpha); the breadcrumb walks the
 * parent chain; opening a text file loads it into the code view (view
 * switch), office files open their office viewer.
 */
export default function FolderView() {
  const [cwd, setCwd] = useState<string | null>(null)
  const [listing, setListing] = useState<FsList | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [openDirs, setOpenDirs] = useState<Record<string, boolean>>({})
  const [root, setRoot] = useState('/')
  const setActiveView = useAppStore((s) => s.setActiveView)

  useEffect(() => {
    void fsHome().then((h) => {
      setRoot(h)
      setCwd(h)
    })
  }, [])

  const load = useCallback(async (path: string) => {
    setLoading(true)
    setError(null)
    try {
      const list = await fsListDir(path)
      setListing(list)
      setCwd(path)
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (cwd) void load(cwd)
  }, [cwd, load])

  const breadcrumbs = useMemo(() => {
    if (!cwd) return []
    const parts = cwd.split('/').filter(Boolean)
    const out: { label: string; path: string }[] = []
    let acc = cwd.startsWith('/') ? '' : ''
    for (const p of parts) {
      acc = acc === '' ? `/${p}` : `${acc}/${p}`
      out.push({ label: p, path: acc })
    }
    return out
  }, [cwd])

  const openFile = (entry: FsEntry, fullPath: string) => {
    const ext = entry.name.split('.').pop()?.toLowerCase() ?? ''
    if (['xlsx', 'docx', 'pptx', 'pdf'].includes(ext)) {
      useAppStore.getState().openOfficeDoc(fullPath)
      return
    }
    // text files → code view (keeps the real path for save)
    void fsReadFile(fullPath).then((f) => {
      if (f.binary || f.truncated) return
      window.dispatchEvent(
        new CustomEvent('agentcowork:open-file', { detail: { path: fullPath, content: f.content } })
      )
      setActiveView('code' as never)
    })
  }

  const renderTree = (entries: FsEntry[], base: string) =>
    entries.map((e) => {
      const full = `${base}/${e.name}`.replace(/\/+/g, '/')
      if (!e.dir) {
        return (
          <button
            key={full}
            onClick={() => openFile(e, full)}
            className="flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-left text-xs hover:bg-accent"
            style={{ paddingLeft: 6 }}
          >
            <span className="w-3" />
            <FileIcon name={e.name} />
            <span className="flex-1 truncate text-foreground">{e.name}</span>
            {e.size != null && (
              <span className="font-mono text-[10px] text-muted-foreground">{formatBytes(e.size)}</span>
            )}
          </button>
        )
      }
      const open = !!openDirs[full]
      return (
        <div key={full}>
          <button
            className="flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-left text-xs hover:bg-accent"
            style={{ paddingLeft: 6 }}
            onClick={() => {
              const next = !open
              setOpenDirs((d) => ({ ...d, [full]: next }))
              if (next) void load(full)
            }}
          >
            {open ? (
              <ChevronDown className="h-3 w-3 text-muted-foreground" />
            ) : (
              <ChevronRight className="h-3 w-3 text-muted-foreground" />
            )}
            {open ? <FolderOpen className="h-4 w-4 text-brand" /> : <Folder className="h-4 w-4 text-brand" />}
            <span className="flex-1 truncate text-foreground">{e.name}</span>
          </button>
          {open && (
            <div className="ml-3 border-l border-border/60 pl-1.5">
              {loading && cwd === full ? (
                <div className="py-1 pl-1.5"><SkeletonBlock lines={2} /></div>
              ) : (
                listing &&
                listing.path === full &&
                renderTree(listing.entries, full)
              )}
            </div>
          )}
        </div>
      )
    })

  return (
    <div className="flex h-full w-full flex-col">
      <header className="flex items-center justify-between border-b border-border px-4 py-2.5">
        <div className="flex min-w-0 items-center gap-1.5 font-mono text-xs text-muted-foreground">
          <button
            onClick={() => cwd?.includes('/') && void load(cwd.slice(0, cwd.lastIndexOf('/')) || '/')}
            aria-label="Parent directory"
            className="rounded p-0.5 hover:bg-accent hover:text-foreground"
          >
            <ArrowLeft className="h-3.5 w-3.5" />
          </button>
          <ChevronRight className="h-3 w-3" />
          <span className="flex min-w-0 items-center gap-1 overflow-hidden">
            {breadcrumbs.map((b, i) => (
              <span key={b.path} className="flex items-center gap-1">
                {i > 0 && <ChevronRight className="h-3 w-3 shrink-0" />}
                <button
                  onClick={() => void load(b.path)}
                  className={cn(
                    'truncate rounded px-0.5 hover:bg-accent',
                    i === breadcrumbs.length - 1 ? 'font-medium text-foreground' : 'text-muted-foreground'
                  )}
                >
                  {b.label || '/'}
                </button>
              </span>
            ))}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <Badge variant="outline" className="gap-1 text-[10px]">
            <HardDrive className="h-3 w-3" /> {root}
          </Badge>
          <button
            onClick={() => cwd && void load(cwd)}
            aria-label="Refresh"
            className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
          >
            <RefreshCw className="h-3 w-3" />
          </button>
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-auto p-2">
        {error && <div className="px-2 py-1 text-xs text-destructive">{error}</div>}
        {!listing && !error && (
          <div className="p-3"><SkeletonBlock lines={6} /></div>
        )}
        {listing && renderTree(listing.entries, listing.path)}
        {listing && listing.entries.length === 0 && (
          <div className="px-2 py-6 text-center text-xs text-muted-foreground">Empty directory</div>
        )}
      </div>
    </div>
  )
}

function FileIcon({ name }: { name: string }) {
  const ext = name.split('.').pop()?.toLowerCase()
  const c = 'h-4 w-4 shrink-0'
  if (ext === 'xlsx' || ext === 'csv') return <FileSpreadsheet className={cn(c, 'text-emerald-400')} />
  if (ext === 'docx') return <FileText className={cn(c, 'text-blue-400')} />
  if (ext === 'pptx') return <Presentation className={cn(c, 'text-brand')} />
  if (ext === 'pdf') return <FileText className={cn(c, 'text-red-400')} />
  return <File className={cn(c, 'text-muted-foreground')} />
}

function formatBytes(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)} GB`
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)} MB`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)} KB`
  return `${n} B`
}
