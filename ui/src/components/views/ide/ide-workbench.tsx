'use client'

import { useEffect, useRef, useState } from 'react'
import {
  Files,
  Search as SearchIcon,
  GitBranch,
  Play,
  Blocks,
  Sparkles,
  Terminal as TerminalIcon,
  AlertCircle,
  X,
  GitCompare,
} from 'lucide-react'
import { cn } from '@/lib/utils'
import { EditorTabs, type OpenFile } from './editor-tabs'
import { MonacoPane } from './monaco-pane'
import { ExplorerPanel } from './explorer-panel'
import { ScmPanel } from './scm-panel'
import { ProblemsPanel } from './problems-panel'
import { DiffRail } from './diff-rail'
import ShellView from '@/components/views/shell-view'
import { useAppStore } from '@/lib/store'

type ActivityId = 'explorer' | 'search' | 'scm' | 'run' | 'extensions' | 'agentcowork'
type PanelId = 'problems' | 'terminal' | 'output' | 'diff'

/**
 * P11.5.3 — AgentCowork Code: a VS Code-like workbench over our Rust backends.
 *
 * Layout mirrors VS Code exactly — Activity Bar · Explorer/Search/SCM
 * sidebar · editor tabs + Monaco (MIT — VS Code's own editor) · bottom
 * Problems/Terminal panel · status bar — but every surface talks to the
 * AgentCowork Rust layer: real FS explorer (fs_cmds), real git SCM
 * (git_cmds), real LSP diagnostics (lsp_cmds → agentcowork-codeintel), and
 * the *same* real PTY terminal as the Shell view (terminal_cmds — one
 * plane, provenance-tracked; the legacy piped `shell_cmds` path is gone).
 *
 * Honest ceilings: search/run/extensions are honest placeholders (grep,
 * debug adapters and the extension marketplace are follow-ups).
 */
export function IdeWorkbench() {
  const activeSessionId = useAppStore((s) => s.activeSessionId)
  const [activity, setActivity] = useState<ActivityId>('explorer')
  const [files, setFiles] = useState<OpenFile[]>([])
  const [activePath, setActivePath] = useState<string | null>(null)
  const [panel, setPanel] = useState<PanelId>('problems')
  const [cwd, setCwd] = useState<string | null>(null)

  const activeFile = files.find((f) => f.path === activePath) ?? null

  // Listen for open-file events from the folder/explorer views.
  useEffect(() => {
    const handler = (e: Event) => {
      const d = (e as CustomEvent<{ path: string; content: string }>).detail
      setFiles((prev) => {
        const exists = prev.some((f) => f.path === d.path)
        if (!exists) return [...prev, { path: d.path, name: d.path.split('/').pop() ?? d.path, content: d.content }]
        return prev
      })
      setActivePath(d.path)
      if (d.path.includes('/')) setCwd(d.path.slice(0, d.path.lastIndexOf('/')))
    }
    window.addEventListener('agentcowork:open-file', handler)
    return () => window.removeEventListener('agentcowork:open-file', handler)
  }, [])

  // The bottom terminal panel is the same PTY plane as the Shell view
  // (sessions are owned by the Rust shell, so mounting here never spawns a
  // duplicate — ShellView reattaches to live sessions on boot).

  const onDirty = (path: string, dirty: boolean) =>
    setFiles((prev) => prev.map((f) => (f.path === path ? { ...f, dirty } : f)))

  const onSaved = (path: string, content: string) =>
    setFiles((prev) => prev.map((f) => (f.path === path ? { ...f, content, dirty: false } : f)))

  const closeTab = (path: string) => {
    setFiles((prev) => {
      const next = prev.filter((f) => f.path !== path)
      if (activePath === path) setActivePath(next[next.length - 1]?.path ?? null)
      return next
    })
  }

  const jumpToLine = (line: number, col: number) => {
    window.dispatchEvent(new CustomEvent('agentcowork:editor-jump', { detail: { line, col } }))
  }

  const sidebar = () => {
    switch (activity) {
      case 'explorer':
        return <ExplorerPanel onOpenFile={(p) => setActivePath(p)} activePath={activePath} />
      case 'scm':
        return <ScmPanel cwd={cwd} />
      case 'search':
        return (
          <div className="flex h-full flex-col gap-2 p-3">
            <div className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">Search</div>
            <p className="text-[11px] leading-relaxed text-muted-foreground">
              Full-workspace grep is wired to the storage filename index
              (follow-up) — file-level find/replace lives in the editor (⌘F).
            </p>
          </div>
        )
      default:
        return (
          <div className="flex h-full flex-col gap-2 p-3">
            <div className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
              {activity === 'run' ? 'Run / Debug' : 'Extensions'}
            </div>
            <p className="text-[11px] leading-relaxed text-muted-foreground">
              {activity === 'run'
                ? 'Debug adapters (DAP) are a follow-up — scheduled tasks and the agent loop already execute here.'
                : 'The ACP agent registry (F8) is the extension marketplace for now; a full extension host is post-v1.'}
            </p>
          </div>
        )
    }
  }

  return (
    <div className="flex h-full w-full flex-col bg-[#1e1e1e] text-[#cccccc]">
      <div className="flex min-h-0 flex-1">
        {/* Activity bar */}
        <div className="flex w-12 shrink-0 flex-col items-center gap-1 border-r border-[#333] bg-[#181818] py-1.5 no-select">
          {(
            [
              ['explorer', Files, 'Explorer (Ctrl+Shift+E)'],
              ['search', SearchIcon, 'Search (Ctrl+Shift+F)'],
              ['scm', GitBranch, 'Source Control (Ctrl+Shift+G)'],
              ['run', Play, 'Run and Debug (Ctrl+Shift+D)'],
              ['extensions', Blocks, 'Extensions (Ctrl+Shift+X)'],
            ] as const
          ).map(([id, Icon, label]) => (
            <button
              key={id}
              onClick={() => setActivity(id)}
              aria-label={label}
              title={label}
              className={cn(
                'relative grid h-11 w-11 place-items-center rounded-md transition-colors',
                activity === id ? 'text-white' : 'text-[#858585] hover:text-white'
              )}
            >
              {activity === id && <span className="absolute left-0 top-1/2 h-6 w-0.5 -translate-y-1/2 rounded bg-white" />}
              <Icon className="h-5 w-5" strokeWidth={1.5} />
            </button>
          ))}
          <div className="flex-1" />
          <button
            onClick={() => setActivity('agentcowork')}
            aria-label="AgentCowork"
            title="AgentCowork"
            className={cn(
              'grid h-11 w-11 place-items-center rounded-md',
              activity === 'agentcowork' ? 'text-brand' : 'text-[#858585] hover:text-brand'
            )}
          >
            <Sparkles className="h-5 w-5" strokeWidth={1.5} />
          </button>
        </div>

        {/* Sidebar */}
        <div className="flex w-60 shrink-0 flex-col border-r border-[#333] bg-[#252526]">
          {sidebar()}
        </div>

        {/* Editor area */}
        <div className="flex min-w-0 flex-1 flex-col">
          <EditorTabs files={files} activePath={activePath} onSelect={setActivePath} onClose={closeTab} />
          {activeFile ? (
            <MonacoPane file={activeFile} onDirty={onDirty} onSaved={onSaved} />
          ) : (
            <div className="flex flex-1 flex-col items-center justify-center gap-2 text-center">
              <Files className="h-6 w-6 text-[#858585]/60" strokeWidth={1.2} />
              <p className="text-xs text-[#858585]">
                Open a file from the Explorer or the Folder view to start editing.
              </p>
            </div>
          )}
        </div>
      </div>

      {/* Bottom panel */}
      <div className="flex h-44 shrink-0 flex-col border-t border-[#333] bg-[#181818]">
        <div className="flex h-7 shrink-0 items-center gap-1 border-b border-[#333] px-2 no-select">
          {(
            [
              ['problems', AlertCircle, 'Problems'],
              ['terminal', TerminalIcon, 'Terminal'],
              ['output', TerminalIcon, 'Output'],
              ['diff', GitCompare, 'Diff rail'],
            ] as const
          ).map(([id, Icon, label]) => (
            <button
              key={id}
              onClick={() => setPanel(id)}
              className={cn(
                'flex h-6 items-center gap-1.5 rounded px-2 text-[11px] transition-colors',
                panel === id ? 'bg-[#37373d] text-white' : 'text-[#858585] hover:text-white'
              )}
            >
              <Icon className="h-3 w-3" /> {label}
              {id === 'problems' && activeFile && <span className="text-[9px] text-rose-400" />}
            </button>
          ))}
          <div className="flex-1" />
          <X className="h-3 w-3 text-[#858585]" />
        </div>
        <div className="min-h-0 flex-1 overflow-auto">
          {panel === 'problems' && (
            <ProblemsPanel
              file={activeFile ? { path: activeFile.path, name: activeFile.name, content: activeFile.content } : null}
              onJump={jumpToLine}
            />
          )}
          {panel === 'terminal' && (
            <div className="h-full">
              <ShellView />
            </div>
          )}
          {panel === 'output' && (
            <div className="p-2 font-mono text-[11px] text-[#858585]">
              Output channels appear here (agent logs, scheduler runs, office recalc).
            </div>
          )}
          {panel === 'diff' && <DiffRail />}
        </div>
      </div>

      {/* Status bar */}
      <div className="flex h-5 shrink-0 items-center gap-3 bg-[#007acc] px-3 font-mono text-[10px] text-white no-select">
        <span className="flex items-center gap-1"><GitBranch className="h-2.5 w-2.5" /> {cwd ? 'workspace' : 'no folder'}</span>
        <span className="flex-1" />
        {activeFile && <span>{activeFile.name.split('.').pop()}</span>}
        <span>UTF-8</span>
        <span className="flex items-center gap-1"><Sparkles className="h-2.5 w-2.5" /> AgentCowork Guard</span>
      </div>
    </div>
  )
}
