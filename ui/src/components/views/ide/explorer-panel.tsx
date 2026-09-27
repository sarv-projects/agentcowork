'use client'

import { useCallback, useEffect, useState } from 'react'
import { ChevronDown, ChevronRight, File, FileText, Folder, FolderOpen, RefreshCw, TerminalSquare } from 'lucide-react'
import { cn } from '@/lib/utils'
import { fsHome, fsListDir, fsReadFile, type FsEntry, type FsList } from '@/lib/fs'
import { SkeletonBlock } from '@/components/ui/loading-state'
import { inTauri } from '@/lib/tauri'
import { terminalProfiles, terminalSpawn } from '@/lib/terminal'
import { useAppStore } from '@/lib/store'

/**
 * Explorer panel (VS Code left sidebar) over the real disk: lazy-loads
 * directories from `fs_list_dir`, opens text files into the editor tabs
 * (dispatch `agentcowork:open-file`), refreshes on demand.
 */
export function ExplorerPanel({
  onOpenFile,
  activePath,
}: {
  onOpenFile: (path: string) => void
  activePath: string | null
}) {
  const [root, setRoot] = useState<string | null>(null)
  const [listing, setListing] = useState<FsList | null>(null)
  const [openDirs, setOpenDirs] = useState<Record<string, boolean>>({})
  const [loading, setLoading] = useState(false)
  const notify = useAppStore((s) => s.notify)

  const load = useCallback(async (path: string) => {
    setLoading(true)
    try {
      const list = await fsListDir(path)
      setListing(list)
    } catch {
      /* non-dir — ignore */
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void fsHome().then((h) => {
      setRoot(h)
      void load(h)
    })
  }, [load])

  const openFile = async (e: FsEntry, full: string) => {
    const f = await fsReadFile(full)
    if (f.binary || f.truncated) return
    onOpenFile(full)
    window.dispatchEvent(
      new CustomEvent('agentcowork:open-file', { detail: { path: full, content: f.content } })
    )
  }

  /** P68 — spawn the default profile rooted at this directory. The Rust side
   * re-verifies the dir exists (the renderer never spawns by raw path trust). */
  const openTerminalHere = async (dir: string) => {
    if (!inTauri()) {
      notify('Open-in-terminal needs the desktop shell', 'error')
      return
    }
    try {
      const reg = await terminalProfiles()
      const offered = reg.profiles.filter((p) => p.offered)
      const profile =
        offered.find((p) => p.profileName === reg.defaultProfile) ??
        offered.find((p) => p.isDefault) ??
        offered[0]
      if (!profile) {
        notify('No terminal profile is offered on this machine', 'error')
        return
      }
      await terminalSpawn(profile.profileName, 24, 80, dir)
      notify(`Terminal opened in ${dir.split('/').pop() || dir}`)
    } catch (e) {
      notify(e instanceof Error ? e.message : 'Could not open terminal here', 'error')
    }
  }

  const renderTree = (entries: FsEntry[], base: string) =>
    entries.map((e) => {
      const full = `${base}/${e.name}`.replace(/\/+/g, '/')
      if (!e.dir) {
        const ext = e.name.split('.').pop()?.toLowerCase()
        return (
          <button
            key={full}
            onClick={() => void openFile(e, full)}
            className={cn(
              'flex w-full items-center gap-1.5 rounded px-1.5 py-0.5 text-left text-[11px] hover:bg-accent/50',
              full === activePath && 'bg-accent/60 text-foreground'
            )}
            style={{ paddingLeft: 8 }}
            title={full}
          >
            {ext === 'rs' || ext === 'ts' || ext === 'tsx' ? (
              <FileText className="h-3.5 w-3.5 shrink-0 text-sky-400/80" />
            ) : (
              <File className="h-3.5 w-3.5 shrink-0 text-muted-foreground/70" />
            )}
            <span className="truncate text-foreground">{e.name}</span>
          </button>
        )
      }
      const open = !!openDirs[full]
      return (
        <div key={full}>
          <button
            className="group flex w-full items-center gap-1.5 rounded px-1.5 py-0.5 text-left text-[11px] hover:bg-accent/50"
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
            {open ? <FolderOpen className="h-3.5 w-3.5 text-primary" /> : <Folder className="h-3.5 w-3.5 text-primary" />}
            <span className="truncate text-foreground">{e.name}</span>
            {/* P68 — open a terminal session rooted at this directory. */}
            <button
              type="button"
              aria-label={`Open terminal in ${e.name}`}
              title={`Open terminal in ${e.name}`}
              onClick={(ev) => {
                ev.stopPropagation()
                void openTerminalHere(full)
              }}
              className="ml-auto shrink-0 rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:text-foreground group-hover:opacity-100"
            >
              <TerminalSquare className="h-3 w-3" />
            </button>
          </button>
          {open && (
            <div className="ml-3 border-l border-border/60 pl-1.5">
              {loading && listing?.path === full ? (
                <div className="py-1 pl-1.5"><SkeletonBlock lines={2} /></div>
              ) : (
                listing?.path === full && renderTree(listing.entries, full)
              )}
            </div>
          )}
        </div>
      )
    })

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-border px-3">
        <span className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">Explorer</span>
        <button
          onClick={() => root && void load(root)}
          aria-label="Refresh explorer"
          className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          <RefreshCw className="h-3 w-3" />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-1.5">
        {!listing && <div className="p-2"><SkeletonBlock lines={5} /></div>}
        {listing && renderTree(listing.entries, listing.path)}
      </div>
    </div>
  )
}
