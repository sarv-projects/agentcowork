'use client'

import * as React from 'react'
import { Suspense } from 'react'
import {
  Folder,
  Terminal,
  Globe,
  Code2,
  Sparkles,
  Activity,
  Plus,
  Maximize2,
  Minimize2,
  PanelRightClose,
  PanelRight,
  GripVertical,
  ScanSearch,
  X,
  FileText,
  Table,
  Presentation,
  File,
  HardDrive,
  GitBranch,
  GitCompare,
  MonitorSmartphone,
  ShieldCheck,
  Download,
  RotateCw,
  Trash2,
  Wrench,
  SquareActivity,
} from 'lucide-react'
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useAppStore, type ViewId } from '@/lib/store'
import { AGENT_MAP } from '@/lib/agents'
import { dispatchOccupancy } from '@/lib/occupancy'
import { inTauri } from '@/lib/tauri'
import { cn } from '@/lib/utils'
import { motion, AnimatePresence } from 'framer-motion'

import FolderView from '@/components/views/folder-view'
import ShellView from '@/components/views/shell-view'
import BrowseView from '@/components/views/browse-view'
import DocxView from '@/components/views/office-docx-view'
import PptxView from '@/components/views/office-pptx-view'
import ProgressView from '@/components/views/progress-view'
import DiffView from '@/components/views/diff-view'
import KanbanView from '@/components/views/kanban-view'
import AuditView from '@/components/views/audit-view'
import StorageView from '@/components/views/storage-view'
import TrajectoryView from '@/components/views/trajectory-view'
import BlueprintView from '@/components/views/blueprint-view'
import LocalServerView from '@/components/views/local-server-view'
import ToolOutputView from '@/components/views/tool-output-view'
import { SessionTimeline } from '@/components/chat/session-timeline'

// P39.5 — heavy views load on first use, not at startup. The IDE workbench
// pulls Monaco (~4 MB), the spreadsheet view pulls IronCalc, the PDF view
// pulls pdf.js, and the generative view pulls the AG-UI surface — none of
// them should parse/execute at app boot (R6 fix #2 lazy activation).
const IdeWorkbench = React.lazy(() => import('@/components/views/ide/ide-workbench').then(m => ({ default: m.IdeWorkbench })))
const XlsxView = React.lazy(() => import('@/components/views/office-xlsx-view'))
const PdfView = React.lazy(() => import('@/components/views/office-pdf-view'))
const GenerativeView = React.lazy(() => import('@/components/views/generative-view'))
const ArtifactView = React.lazy(() => import('@/components/views/artifact-view'))
// The one run surface: identity, context/usage, ordered trace steps, produced
// files, working folder, and MCP servers for the current run. It aggregates;
// progress / trajectory / diff / artifact / tool-output stay as drill-downs.
const RunView = React.lazy(() => import('@/components/views/run-view'))
const DesktopView = React.lazy(() => import('@/components/views/desktop-view'))

interface RailItem {
  id: ViewId
  icon: React.ElementType
  label: string
  shortcut: string
  live?: boolean
}

const railItems: RailItem[] = [
  { id: 'folder', icon: Folder, label: 'Folder', shortcut: '⌘⇧E' },
  { id: 'shell', icon: Terminal, label: 'Shell', shortcut: 'Ctrl+`' },
  { id: 'browse', icon: Globe, label: 'Browse', shortcut: '⌘⇧B', live: true },
  { id: 'desktop', icon: MonitorSmartphone, label: 'Computer use', shortcut: '⌘⇧D', live: true },
  { id: 'code', icon: Code2, label: 'Code', shortcut: '⌘⇧C' },
]

const sessionItems: RailItem[] = [
  { id: 'progress', icon: Activity, label: 'Progress', shortcut: '⌘⇧P' },
  { id: 'trajectory', icon: ScanSearch, label: 'Trajectory', shortcut: '⌘⇧T' },
]

/** The four user-facing destinations used when the icon rail cannot fit. */
export const NARROW_RIGHT_TABS = [
  { id: 'chat', label: 'Chat', icon: Sparkles },
  { id: 'files', label: 'Files', icon: Folder },
  { id: 'preview', label: 'Preview', icon: MonitorSmartphone },
  { id: 'tools', label: 'Tools', icon: Wrench },
] as const

export type NarrowRightTab = (typeof NARROW_RIGHT_TABS)[number]['id']

export function narrowTabForView(view: ViewId): NarrowRightTab {
  if (view === 'folder') return 'files'
  if (view === 'progress' || view === 'trajectory' || view === 'diff' || view === 'audit' || view === 'storage' || view === 'timeline' || view === 'kanban' || view === 'blueprint' || view === 'local-server' || view === 'tool-output' || view === 'run') return 'tools'
  return 'preview'
}

export function NarrowRightTabBar({
  activeTab,
  onChange,
  onCollapse,
}: {
  activeTab: NarrowRightTab
  onChange: (tab: NarrowRightTab) => void
  onCollapse?: () => void
}) {
  const refs = React.useRef<Partial<Record<NarrowRightTab, HTMLButtonElement | null>>>({})
  const onKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (event.key !== 'ArrowRight' && event.key !== 'ArrowLeft' && event.key !== 'Home' && event.key !== 'End') return
    event.preventDefault()
    const last = NARROW_RIGHT_TABS.length - 1
    const nextIndex =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? last
          : event.key === 'ArrowRight'
            ? (index + 1) % NARROW_RIGHT_TABS.length
            : (index - 1 + NARROW_RIGHT_TABS.length) % NARROW_RIGHT_TABS.length
    const next = NARROW_RIGHT_TABS[nextIndex]
    if (!next) return
    onChange(next.id)
    refs.current[next.id]?.focus()
  }

  return (
    <div className="flex min-w-0 shrink-0 items-stretch border-b border-border bg-sidebar/70">
      <div
        role="tablist"
        aria-label="Chat, files, preview, and tools"
        className="scroll-thin flex min-w-0 flex-1 items-stretch gap-0.5 overflow-x-auto px-1 pt-1"
      >
        {NARROW_RIGHT_TABS.map((tab, index) => {
          const Icon = tab.icon
          const selected = activeTab === tab.id
          return (
            <button
              key={tab.id}
              ref={(node) => { refs.current[tab.id] = node }}
              type="button"
              role="tab"
              id={`narrow-right-tab-${tab.id}`}
              aria-controls={`narrow-right-panel-${tab.id}`}
              aria-selected={selected}
              tabIndex={selected ? 0 : -1}
              onClick={() => onChange(tab.id)}
              onKeyDown={(event) => onKeyDown(event, index)}
              className={cn(
                'flex min-w-[4.5rem] shrink-0 items-center justify-center gap-1.5 rounded-t-md border border-b-0 px-2 py-1.5 text-[10.5px] transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-brand/60',
                selected
                  ? 'border-border bg-card text-foreground'
                  : 'border-transparent text-muted-foreground hover:bg-accent/60 hover:text-foreground',
              )}
            >
              <Icon className={cn('h-3.5 w-3.5', selected && 'text-brand')} aria-hidden="true" />
              <span>{tab.label}</span>
            </button>
          )
        })}
      </div>
      {onCollapse && (
        <button
          type="button"
          aria-label="Hide work lenses"
          onClick={onCollapse}
          className="mr-1 grid h-7 w-7 shrink-0 place-items-center self-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
        >
          <PanelRightClose aria-hidden className="h-3.5 w-3.5" />
        </button>
      )}
    </div>
  )
}

// P58.5 — `live` was a constant `false` for all four (a lie: the xlsx/docx/
// pdf engines are real; pptx is engine-read). The flyout now derives liveness
// from the real attach state (`officePaths[id]` — a real file is open in that
// view), so the dot means what the tab strip's dot means. No constant.
const officeFlyoutItems = [
  { id: 'office-xlsx' as ViewId, label: 'Spreadsheet', type: 'Sheets' },
  { id: 'office-docx' as ViewId, label: 'Document', type: 'Word' },
  { id: 'office-pptx' as ViewId, label: 'Slides', type: 'Slides' },
  { id: 'office-pdf' as ViewId, label: 'PDF', type: 'PDF' },
]

// View metadata for the multi-view tab strip (ARCH/12 v3.0 — VS Code-style).
// P50.3.7 — office labels are kind names until a real file is open; the tab
// strip and flyout show `officePaths[v]` filenames once attached (never demo
// filenames as if they were open).
const VIEW_META: Record<ViewId, { label: string; icon: React.ElementType }> = {
  folder: { label: 'Folder', icon: Folder },
  shell: { label: 'Terminal', icon: Terminal },
  browse: { label: 'Browser', icon: Globe },
  code: { label: 'Code', icon: Code2 },
  'office-xlsx': { label: 'Spreadsheet', icon: Table },
  'office-docx': { label: 'Document', icon: FileText },
  'office-pptx': { label: 'Slides', icon: Presentation },
  'office-pdf': { label: 'PDF', icon: File },
  progress: { label: 'Progress', icon: Activity },
  diff: { label: 'Diff', icon: GitCompare },
  audit: { label: 'Audit', icon: ShieldCheck },
  storage: { label: 'Storage', icon: HardDrive },
  timeline: { label: 'Timeline', icon: Activity },
  trajectory: { label: 'Trajectory', icon: ScanSearch },
  blueprint: { label: 'Blueprint', icon: FileText },
  'local-server': { label: 'Local Server', icon: FileText },
  kanban: { label: 'Kanban', icon: GitBranch },
  generative: { label: 'Generative UI', icon: Sparkles },
  artifact: { label: 'Artifact', icon: MonitorSmartphone },
  desktop: { label: 'Computer use', icon: MonitorSmartphone },
  'tool-output': { label: 'Tool output', icon: FileText },
  run: { label: 'Run', icon: SquareActivity },
}

function ViewportContent({ view }: { view: ViewId }) {
  return (
    <Suspense
      fallback={
        <div className="h-full w-full flex items-center justify-center text-xs text-muted-foreground animate-pulse">
          Loading view…
        </div>
      }
    >
      {renderView(view)}
    </Suspense>
  )
}

function renderView(view: ViewId) {
  switch (view) {
    case 'folder': return <FolderView />
    case 'shell': return <ShellView />
    case 'browse': return <BrowseView />
    case 'code': return <IdeWorkbench />
    case 'office-xlsx': return <XlsxView />
    case 'office-docx': return <DocxView />
    case 'office-pptx': return <PptxView />
    case 'office-pdf': return <PdfView />
    case 'progress': return <ProgressView />
    case 'diff': return <DiffView />
    case 'audit': return <AuditView />
    case 'storage': return <StorageView />
    case 'timeline': return <SessionTimeline />
    case 'trajectory': return <TrajectoryView />
    case 'blueprint': return <BlueprintView />
    case 'local-server': return <LocalServerView />
    case 'kanban': return <KanbanView />
    case 'generative': return <GenerativeView />
    case 'artifact': return <ArtifactView />
    case 'desktop': return <DesktopView />
    case 'tool-output': return <ToolOutputView />
    case 'run': return <RunView />
    default:
      return (
        <div className="grid h-full w-full place-items-center p-6 text-center">
          <p className="text-[11px] text-muted-foreground">
            Unknown view “{view satisfies never}” — pick a lens from the rail.
          </p>
        </div>
      )
  }
}

export function ActivityRail({ narrow = false }: { narrow?: boolean } = {}) {
  const activeView = useAppStore((s) => s.activeView)
  const setActiveView = useAppStore((s) => s.setActiveView)
  const officePaths = useAppStore((s) => s.officePaths)
  const railCollapsed = useAppStore((s) => s.railCollapsed)
  const toggleRail = useAppStore((s) => s.toggleRail)
  const setOfficeFlyoutOpen = useAppStore((s) => s.setOfficeFlyoutOpen)
  const officeFlyoutOpen = useAppStore((s) => s.officeFlyoutOpen)
  const setRailCollapsed = useAppStore((s) => s.setRailCollapsed)
  // P50.3.7/8 — rail dots reflect live attachment, never static flags.
  const browserAttached = useAppStore((s) => s.browserAttached)
  // Progress dot = real activity (running turn or gateway work), not a
  // hardcoded flag.
  const progressLive =
    useAppStore((s) =>
      s.sessions.some((x) => x.id === s.activeSessionId && x.status === 'running'),
    ) || useAppStore((s) => s.workEvents.length > 0 || s.workItems.length > 0)
  // Occupancy is the currently picked primary agent — not DEFAULT_ROUTING per view.
  const selectedAgentId = useAppStore((s) => s.selectedAgentId)
  const occupancyId = dispatchOccupancy(activeView, selectedAgentId, {})
  const occupancyAgent = AGENT_MAP[occupancyId]

  const handleClick = (item: RailItem) => {
    if (item.id === activeView && !railCollapsed) {
      setRailCollapsed(true)
    } else {
      setActiveView(item.id)
    }
  }

  if (narrow) return null

  return (
    <div className="shrink-0 w-12 border-l border-border bg-sidebar flex flex-col items-center py-2 gap-1 no-select relative z-20">
      {railItems.map((item) => {
        const Icon = item.icon
        const isActive = activeView === item.id && !railCollapsed
        // P50.3.8 — the Browse dot means a live CDP session is attached.
        const showLive = item.id === 'browse' ? browserAttached : item.live
        return (
          <Tooltip key={item.id}>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={() => handleClick(item)}
                aria-label={`Open ${item.label} view`}
                className={cn(
                  'group relative grid h-9 w-9 place-items-center rounded-md transition-all',
                  isActive
                    ? 'bg-brand/15 text-brand ring-1 ring-brand/30'
                    : 'text-muted-foreground hover:bg-accent hover:text-foreground'
                )}
              >
                <Icon className="h-4 w-4" />
                {showLive && (
                  <span className="absolute -top-0.5 -right-0.5 h-2 w-2 rounded-full bg-brand live-dot ring-2 ring-sidebar" />
                )}
                {isActive && (
                  <span className="absolute left-0 top-1.5 bottom-1.5 w-0.5 rounded-r bg-brand" />
                )}
              </button>
            </TooltipTrigger>
            <TooltipContent side="left" sideOffset={8}>
              {item.label}
              <span className="ml-2 text-muted-foreground text-[10px] font-mono">{item.shortcut}</span>
              {showLive && (
                <span className="ml-2 inline-flex items-center gap-1 text-[10px] text-brand">
                  <span className="h-1 w-1 rounded-full bg-brand live-dot" /> Live
                </span>
              )}
              {occupancyAgent && (
                  <span className="mt-0.5 flex items-center gap-1 text-[10px] text-muted-foreground/80">
                    <span className={cn('h-3 w-3 rounded text-[6px] font-bold flex items-center justify-center', occupancyAgent.accent)}>{occupancyAgent.mark}</span>
                    {occupancyAgent.name}
                  </span>
              )}
            </TooltipContent>
          </Tooltip>
        )
      })}

      <div className="w-5 h-px bg-border my-1.5" />

      <Popover open={officeFlyoutOpen} onOpenChange={setOfficeFlyoutOpen}>
        <Tooltip>
          <TooltipTrigger asChild>
            <PopoverTrigger asChild>
              <button
                type="button"
                onClick={() => {
                  if (!activeView.startsWith('office-')) setActiveView('office-xlsx')
                  setRailCollapsed(false)
                  setOfficeFlyoutOpen(true)
                }}
                aria-label="Open Office documents"
                className={cn(
                  'group relative grid h-9 w-9 place-items-center rounded-md transition-all',
                  activeView.startsWith('office-') && !railCollapsed
                    ? 'bg-brand/15 text-brand ring-1 ring-brand/30'
                    : 'text-muted-foreground hover:bg-accent hover:text-foreground'
                )}
              >
                <span className="text-[12px] font-bold leading-none">W</span>
              </button>
            </PopoverTrigger>
          </TooltipTrigger>
          <TooltipContent side="left" sideOffset={8}>
            Office · Word/Excel/Slides/PDF
            <span className="ml-2 text-muted-foreground text-[10px] font-mono">⌘⇧O</span>
          </TooltipContent>
        </Tooltip>
        <PopoverContent side="left" align="start" sideOffset={8} className="scale-in w-64 p-2">
          <div className="space-y-0.5">
            <div className="px-2 py-1 text-[10.5px] uppercase tracking-wider text-muted-foreground/70 font-semibold">
              Open documents
            </div>
            {officeFlyoutItems.map((doc) => (
              <button
                key={doc.id}
                onClick={() => {
                  setActiveView(doc.id)
                  setOfficeFlyoutOpen(false)
                }}
                className={cn(
                  'w-full flex items-center gap-2 rounded-md px-2 py-1.5 text-[12px] hover:bg-accent transition-colors',
                  activeView === doc.id && 'bg-accent text-brand'
                )}
              >
                <span className="text-[10px] font-mono text-muted-foreground w-12">{doc.type}</span>
                <span className="flex-1 text-left truncate">
                  {officePaths[doc.id]?.split(/[\\/]/).pop() ?? doc.label}
                </span>
                {/* P50.3.7 — the dot means a real file is open, never a demo
                    filename. Kind label alone = nothing attached yet. */}
                {officePaths[doc.id] && (
                  <span className="h-1.5 w-1.5 rounded-full bg-brand live-dot" />
                )}
              </button>
            ))}
            <div className="h-px bg-border my-1.5" />
            <button
              className="w-full flex items-center gap-2 rounded-md px-2 py-1.5 text-[12px] text-muted-foreground hover:bg-accent hover:text-foreground transition-colors"
              onClick={() =>
                void (async () => {
                  const st = useAppStore.getState()
                  if (!inTauri()) {
                    st.notify('File picker needs the Tauri shell — open a file from the Office views instead')
                    return
                  }
                  try {
                    const { open } = await import('@tauri-apps/plugin-dialog')
                    const picked = await open({
                      multiple: false,
                      title: 'Open an Office document',
                      filters: [{ name: 'Office', extensions: ['docx', 'xlsx', 'xlsm', 'pptx', 'pdf'] }],
                    })
                    if (typeof picked === 'string' && picked.trim()) st.openOfficeDoc(picked.trim())
                  } catch (e) {
                    st.notify(e instanceof Error ? e.message : 'File picker failed', 'error')
                  }
                  setOfficeFlyoutOpen(false)
                })()
              }
            >
              <Plus className="h-3.5 w-3.5" />
              <span>Open another…</span>
            </button>
          </div>
        </PopoverContent>
      </Popover>

      <div className="w-5 h-px bg-border my-1.5" />

      {sessionItems.map((item) => {
        const Icon = item.icon
        const isActive = activeView === item.id && !railCollapsed
        const showLive = item.id === 'progress' ? progressLive : item.live
        return (
          <Tooltip key={item.id}>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={() => handleClick(item)}
                aria-label={`Open ${item.label} view`}
                className={cn(
                  'group relative grid h-9 w-9 place-items-center rounded-md transition-all',
                  isActive
                    ? 'bg-brand/15 text-brand ring-1 ring-brand/30'
                    : 'text-muted-foreground hover:bg-accent hover:text-foreground'
                )}
              >
                <Icon className="h-4 w-4" />
                {showLive && (
                  <span className="absolute -top-0.5 -right-0.5 h-2 w-2 rounded-full bg-brand live-dot ring-2 ring-sidebar" />
                )}
              </button>
            </TooltipTrigger>
            <TooltipContent side="left" sideOffset={8}>
              {item.label}
              <span className="ml-2 text-muted-foreground text-[10px] font-mono">{item.shortcut}</span>
            </TooltipContent>
          </Tooltip>
        )
      })}

      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            onClick={() => setActiveView('timeline')}
            aria-label="Open activity timeline"
            className="grid h-9 w-9 place-items-center rounded-md text-muted-foreground/60 hover:bg-accent hover:text-foreground transition-all mt-1 border border-dashed border-border"
          >
            <Activity className="h-4 w-4" />
          </button>
        </TooltipTrigger>
        <TooltipContent side="left" sideOffset={8}>
          Timeline · Diff · Audit · Storage
        </TooltipContent>
      </Tooltip>

      <div className="flex-1" />

      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            onClick={toggleRail}
            aria-label={railCollapsed ? 'Expand viewport' : 'Collapse viewport'}
            className="grid h-8 w-8 place-items-center rounded-md text-muted-foreground/60 hover:bg-accent hover:text-foreground transition-all"
          >
            {railCollapsed ? (
              <PanelRight className="h-3.5 w-3.5" />
            ) : (
              <PanelRightClose className="h-3.5 w-3.5" />
            )}
          </button>
        </TooltipTrigger>
        <TooltipContent side="left" sideOffset={8}>
          {railCollapsed ? 'Expand viewport (⌘\\)' : 'Collapse viewport (⌘\\)'}
        </TooltipContent>
      </Tooltip>
    </div>
  )
}

export function RightViewport({ narrow = false }: { narrow?: boolean } = {}) {
  const railCollapsed = useAppStore((s) => s.railCollapsed)
  const activeView = useAppStore((s) => s.activeView)
  const setActiveView = useAppStore((s) => s.setActiveView)
  const openViews = useAppStore((s) => s.openViews)
  const officePaths = useAppStore((s) => s.officePaths)
  // P50.3.7/8 — per-tab attachment dots (browse = CDP session, desktop =
  // engine, office = open file) so the strip never claims live wrongly.
  const browserAttached = useAppStore((s) => s.browserAttached)
  const desktopAttached = useAppStore((s) => s.desktopAttached)
  const addView = useAppStore((s) => s.addView)
  const closeView = useAppStore((s) => s.closeView)
  const reorderViews = useAppStore((s) => s.reorderViews)
  const fullscreenView = useAppStore((s) => s.fullscreenView)
  const setFullscreenView = useAppStore((s) => s.setFullscreenView)
  const setRailCollapsed = useAppStore((s) => s.setRailCollapsed)
  const setCenterScreen = useAppStore((s) => s.setCenterScreen)

  // Narrow mode keeps the selected lens mounted while the user moves between
  // the four coarse destinations. The desktop tab strip and store remain the
  // source of truth; these are presentation layers, not a second view registry.
  const initialNarrowViewRef = React.useRef<ViewId | null>(null)
  if (initialNarrowViewRef.current === null) {
    const preview = openViews.find((view) => narrowTabForView(view) === 'preview')
    initialNarrowViewRef.current = preview ?? (narrowTabForView(activeView) === 'preview' ? activeView : 'browse')
  }
  const initialNarrowView = initialNarrowViewRef.current ?? activeView
  const previewViewRef = React.useRef<ViewId>(initialNarrowView)
  const [narrowTab, setNarrowTab] = React.useState<NarrowRightTab>(() => narrowTabForView(activeView))
  const [mountedNarrowViews, setMountedNarrowViews] = React.useState<ViewId[]>(() => [initialNarrowView])

  React.useEffect(() => {
    if (!narrow) return
    if (narrowTabForView(activeView) === 'preview') previewViewRef.current = activeView
    const targetView: ViewId =
      narrowTab === 'files' ? 'folder' : narrowTab === 'tools' ? 'progress' : previewViewRef.current
    setMountedNarrowViews((current) => {
      const next = current.includes(activeView) ? [...current] : [...current, activeView]
      return next.includes(targetView) ? next : [...next, targetView]
    })
  }, [activeView, narrow, narrowTab])

  // P33.7 — drag-reorder state for the tab strip.
  const [dragIndex, setDragIndex] = React.useState<number | null>(null)
  const [dropIndex, setDropIndex] = React.useState<number | null>(null)

  // Resize state — percentage of total window width
  const [viewportPct, setViewportPct] = React.useState<number>(45)
  const [isResizing, setIsResizing] = React.useState(false)

  const notify = useAppStore((s) => s.notify)

  // Create an empty untitled file in the workspace folder and open it in the
  // workbench. Human-gesture path (clicked button = trusted gesture); needs
  // the shell + an attached folder, otherwise says so.
  const newUntitledFile = () =>
    void (async () => {
      const st = useAppStore.getState()
      if (!inTauri()) {
        st.notify('New file needs the Tauri shell')
        return
      }
      const folder = st.taskFolder
      if (!folder) {
        st.notify('Attach a workspace folder first (chat empty state → Open folder)', 'error')
        return
      }
      const name = `untitled-${Date.now().toString(36)}.md`
      const path = `${folder.replace(/\/+$/, '')}/${name}`
      try {
        const { fsWriteFile } = await import('@/lib/fs')
        await fsWriteFile(path, '')
        window.dispatchEvent(
          new CustomEvent('agentcowork:open-file', { detail: { path, content: '' } }),
        )
        st.setActiveView('code')
        st.notify(`Created ${name}`)
      } catch (e) {
        st.notify(e instanceof Error ? e.message : 'New file failed', 'error')
      }
    })()

  const exportWorkLog = () => {
    const st = useAppStore.getState()
    const events = st.workEvents ?? []
    if (events.length === 0) {
      st.notify('No work events to export yet')
      return
    }
    const blob = new Blob([events.map((e) => JSON.stringify(e)).join('\n')], {
      type: 'application/x-ndjson',
    })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = 'work-events.ndjson'
    a.click()
    URL.revokeObjectURL(url)
    st.notify(`Exported ${events.length} work events`)
  }

  // View-specific action buttons — every button does something real (nav,
  // IPC, or download). Rows without a backing action were removed, not
  // left as toast-only stubs.
  const viewActions: Record<string, { icon: React.ElementType; label: string; action: () => void }[]> = {
    folder: [
      {
        icon: Plus,
        label: 'New file',
        action: newUntitledFile,
      },
      {
        icon: GitCompare,
        label: 'Diff',
        action: () => addView('diff'),
      },
    ],
    shell: [
      {
        icon: Terminal,
        label: 'Focus terminal',
        action: () => setActiveView('shell'),
      },
    ],
    browse: [
      // P50.3.8 — header actions must do real things: the session is single-
      // page (no tabs) and the snapshot lives in the view, so these route to
      // the surface instead of claiming fake tabs/inspections.
      {
        icon: Globe,
        label: 'Open Browse',
        action: () => setActiveView('browse'),
      },
      {
        icon: ScanSearch,
        label: 'Snapshot',
        action: () => setActiveView('browse'),
      },
    ],
    desktop: [
      {
        icon: MonitorSmartphone,
        label: 'Open Computer use',
        action: () => setActiveView('desktop'),
      },
    ],
    code: [
      {
        icon: Plus,
        label: 'New file',
        action: newUntitledFile,
      },
      {
        icon: GitCompare,
        label: 'Diff',
        action: () => addView('diff'),
      },
    ],
    progress: [
      { icon: Activity, label: 'Timeline', action: () => setActiveView('timeline') },
      {
        icon: Download,
        label: 'Export log',
        action: exportWorkLog,
      },
      // The run surface aggregates this timeline, so it is reachable from here.
      // No new rail icon: a view opens when it is used (ARCH/12 §4.0), and the
      // "+ Add view" menu is the sanctioned slot.
      { icon: SquareActivity, label: 'Run summary', action: () => addView('run') },
    ],
    timeline: [
      { icon: Activity, label: 'Progress', action: () => setActiveView('progress') },
      {
        icon: ShieldCheck,
        label: 'Audit',
        action: () => addView('audit'),
      },
    ],
    'office-xlsx': [
      // P50.3.7 — route into the sheet surface instead of claiming fake
      // recalc results; the view's own Recalc runs IronCalc on the open file.
      {
        icon: RotateCw,
        label: 'Recalculate',
        action: () => {
          setActiveView('office-xlsx')
          notify('Use Recalc in the sheet — runs on the open file')
        },
      },
    ],
    'office-docx': [
      {
        icon: FileText,
        label: 'Open document',
        action: () => setActiveView('office-docx'),
      },
    ],
    'office-pptx': [
      {
        icon: Presentation,
        label: 'Open deck',
        action: () => setActiveView('office-pptx'),
      },
    ],
    'office-pdf': [
      {
        icon: ScanSearch,
        label: 'Search in PDF',
        action: () => {
          setActiveView('office-pdf')
          notify('Use “Find in PDF” in the open document')
        },
      },
    ],
    audit: [
      {
        icon: ShieldCheck,
        label: 'Open audit view',
        action: () => setActiveView('audit'),
      },
    ],
    storage: [
      {
        icon: Trash2,
        label: 'Review cleanup',
        action: () => setActiveView('storage'),
      },
    ],
  }
  const actions = viewActions[activeView] ?? []

  const chooseNarrowTab = (tab: NarrowRightTab) => {
    setNarrowTab(tab)
    if (tab === 'chat') {
      setCenterScreen('chat')
      return
    }
    const nextView: ViewId =
      tab === 'files'
        ? 'folder'
        : tab === 'tools'
          ? 'progress'
          : previewViewRef.current
    setMountedNarrowViews((current) =>
      current.includes(nextView) ? current : [...current, nextView],
    )
    setActiveView(nextView)
    setCenterScreen('chat')
  }

  // Resize drag handlers
  React.useEffect(() => {
    if (!isResizing) return
    const onMove = (e: MouseEvent) => {
      const w = window.innerWidth
      // Right viewport occupies from (window - viewportPx) to window
      const newPx = w - e.clientX
      const pct = Math.min(70, Math.max(28, (newPx / w) * 100))
      setViewportPct(pct)
    }
    const onUp = () => setIsResizing(false)
    document.body.style.cursor = 'col-resize'
    document.body.style.userSelect = 'none'
    window.addEventListener('mousemove', onMove)
    window.addEventListener('mouseup', onUp)
    return () => {
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
      window.removeEventListener('mousemove', onMove)
      window.removeEventListener('mouseup', onUp)
    }
  }, [isResizing])

  if (narrow && railCollapsed) {
    return (
      <div data-narrow-right-collapsed className="flex min-h-0 w-full flex-none items-center justify-end border-t border-border bg-card/40 px-2 py-1">
        <button
          type="button"
          onClick={() => setRailCollapsed(false)}
          aria-label="Show work lenses"
          className="grid h-7 w-7 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
        >
          <PanelRight aria-hidden className="h-3.5 w-3.5" />
        </button>
      </div>
    )
  }

  if (narrow) {
    const visibleView: ViewId =
      narrowTab === 'files' ? 'folder' : narrowTab === 'tools' ? 'progress' : previewViewRef.current
    return (
      <section
        data-narrow-right-rail
        data-testid="narrow-right-rail"
        aria-label="Work lenses"
        className={cn(
          'flex min-h-0 min-w-0 flex-none flex-col overflow-hidden border-t border-border bg-card/40',
          fullscreenView ? 'h-full' : 'h-[min(42vh,22rem)]',
        )}
      >
        <NarrowRightTabBar
          activeTab={narrowTab}
          onChange={chooseNarrowTab}
          onCollapse={() => setRailCollapsed(true)}
        />
        <div className="min-h-0 flex-1 overflow-hidden">
          {NARROW_RIGHT_TABS.map((tab) => {
            const active = narrowTab === tab.id
            const tabViews = mountedNarrowViews.filter((view) => narrowTabForView(view) === tab.id)
            return (
              <div
                key={tab.id}
                id={`narrow-right-panel-${tab.id}`}
                role="tabpanel"
                aria-labelledby={`narrow-right-tab-${tab.id}`}
                hidden={!active}
                className="h-full min-h-0"
              >
                {tab.id === 'chat' ? (
                  <div className="grid h-full place-items-center px-5 text-center">
                    <div className="max-w-sm">
                      <Sparkles className="mx-auto h-5 w-5 text-brand" aria-hidden="true" />
                      <p className="mt-2 text-xs font-medium text-foreground">Chat stays in the main pane</p>
                      <p className="mt-1 text-[11px] leading-relaxed text-muted-foreground">
                        Your task, approvals, and composer remain above. Use Files, Preview, or Tools when you need to inspect the work around it.
                      </p>
                    </div>
                  </div>
                ) : (
                  <div className="relative h-full min-h-0">
                    {tabViews.map((view) => {
                      const visible = view === visibleView
                      return (
                        <div
                          key={view}
                          data-lens-view={view}
                          hidden={!visible}
                          className="absolute inset-0 min-h-0"
                        >
                          <ViewportContent view={view} />
                        </div>
                      )
                    })}
                  </div>
                )}
              </div>
            )
          })}
        </div>
      </section>
    )
  }

  return (
    <AnimatePresence initial={false} mode="wait">
      {!railCollapsed && (
        <motion.section
          key="viewport"
          initial={{ width: 0, opacity: 0 }}
          animate={{ width: fullscreenView ? '100%' : `${viewportPct}%`, opacity: 1 }}
          exit={{ width: 0, opacity: 0 }}
          transition={{ duration: 0.3, ease: [0.4, 0, 0.2, 1] }}
          className={cn(
            'border-l border-border bg-card/40 overflow-hidden flex flex-col min-w-0 relative',
            fullscreenView && 'fixed inset-0 z-50 w-full rounded-none bg-background',
          )}
        >
          {/* Multi-view tab strip (VS Code-style: default Terminal · Folder · Browser, "+" to add, × to close) */}
          <div className="flex shrink-0 items-center gap-0.5 overflow-x-auto scroll-thin border-b border-border bg-sidebar/60 px-1 pt-1 no-select">
            {openViews.map((v, idx) => {
              const meta = VIEW_META[v]
              const Icon = meta.icon
              const isActive = v === activeView
              const isDragTarget = dragIndex !== null && idx === dropIndex
              // P50.3.7/8 — live dot only on proven attachment: browse needs
              // the CDP session, desktop the engine, office an open file.
              const tabLive =
                v === 'browse'
                  ? browserAttached
                  : v === 'desktop'
                    ? desktopAttached
                    : v.startsWith('office-')
                      ? !!officePaths[v]
                      : false
              const tabTitle =
                v === 'browse'
                  ? browserAttached
                    ? 'Browser — CDP attached'
                    : 'Browser — detached'
                  : v === 'desktop'
                    ? desktopAttached
                      ? 'Computer use — engine attached'
                      : 'Computer use — detached'
                    : (officePaths[v] ?? meta.label)
              return (
                <div
                  key={v}
                  draggable
                  onDragStart={(e) => {
                    setDragIndex(idx)
                    e.dataTransfer.effectAllowed = 'move'
                  }}
                  onDragOver={(e) => {
                    e.preventDefault()
                    if (dragIndex !== null && idx !== dragIndex) setDropIndex(idx)
                  }}
                  onDragLeave={() => {
                    if (dragIndex !== null && idx === dropIndex) setDropIndex(null)
                  }}
                  onDrop={(e) => {
                    e.preventDefault()
                    if (dragIndex !== null && dropIndex !== null && dragIndex !== dropIndex) {
                      reorderViews(dragIndex, dropIndex)
                    }
                    setDragIndex(null)
                    setDropIndex(null)
                  }}
                  onDragEnd={() => {
                    setDragIndex(null)
                    setDropIndex(null)
                  }}
                  onClick={() => setActiveView(v)}
                  title={tabTitle}
                  className={cn(
                    'group flex cursor-pointer items-center gap-1.5 rounded-t-md border border-b-0 px-2 py-1 text-[10.5px] transition-colors',
                    isActive
                      ? 'border-border bg-card text-foreground'
                      : 'border-transparent text-muted-foreground hover:bg-accent/60 hover:text-foreground',
                  )}
                >
                  <Icon className={cn('h-3 w-3 shrink-0', isActive && 'text-brand')} />
                  {tabLive && (
                    <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-brand live-dot" title={tabTitle} />
                  )}
                  <span
                    className={cn(
                      'max-w-[130px] truncate',
                      isDragTarget && 'rounded bg-brand/20 ring-1 ring-brand/50',
                      dragIndex === idx && 'opacity-40'
                    )}
                  >
                    {officePaths[v]?.split(/[\\/]/).pop() ?? meta.label}
                  </span>
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation()
                      closeView(v)
                    }}
                    className="rounded p-0.5 opacity-0 transition-opacity group-hover:opacity-100 hover:bg-accent"
                    title="Close view"
                  >
                    <X className="h-3 w-3" />
                  </button>
                </div>
              )
            })}

            {/* "+" — add any view (VS Code panel add) */}
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="grid h-6 w-6 shrink-0 place-items-center rounded text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                  title="Add view"
                >
                  <Plus className="h-3.5 w-3.5" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" sideOffset={4} className="w-56">
                <DropdownMenuLabel className="text-[11px]">Add view</DropdownMenuLabel>
                {Object.entries(VIEW_META)
                  .filter(([id]) => !openViews.includes(id as ViewId))
                  .map(([id, meta]) => {
                    const Icon = meta.icon
                    return (
                      <DropdownMenuItem
                        key={id}
                        onClick={() => addView(id as ViewId)}
                        className="text-xs"
                      >
                        <Icon className="h-3.5 w-3.5 text-muted-foreground" />
                        {meta.label}
                      </DropdownMenuItem>
                    )
                  })}
                {Object.keys(VIEW_META).every((id) => openViews.includes(id as ViewId)) && (
                  <DropdownMenuItem disabled className="text-[10px] text-muted-foreground">
                    All views open
                  </DropdownMenuItem>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          </div>

          {/* Resize handle */}
          <div
            onMouseDown={(e) => {
              e.preventDefault()
              setIsResizing(true)
            }}
            onDoubleClick={() => setViewportPct(45)}
            className={cn(
              'absolute left-0 top-0 z-20 h-full w-1 cursor-col-resize transition-colors',
              isResizing
                ? 'bg-brand/80'
                : 'bg-transparent hover:bg-brand/40',
            )}
            title="Drag to resize · double-click to reset"
          >
            <div className="absolute left-0 top-1/2 h-10 w-1 -translate-y-1/2 rounded-r">
              <GripVertical className="h-3.5 w-3.5 text-muted-foreground/40 opacity-0 transition-opacity hover:opacity-100" />
            </div>
          </div>

          <div className="shrink-0 h-8 border-b border-border bg-sidebar/60 flex items-center px-2 gap-2 no-select">
            <span className="text-[11px] font-medium text-foreground/80 truncate flex-1 capitalize">
              {activeView === 'timeline' ? 'Chat Timeline' : activeView.replace('office-', '').replace('-', ' ')}
            </span>
            {actions.map((act) => {
              const ActIcon = act.icon
              return (
                <Tooltip key={act.label}>
                  <TooltipTrigger asChild>
                    <button
                      onClick={act.action}
                      className="grid h-5 w-5 place-items-center rounded hover:bg-accent text-muted-foreground hover:text-foreground transition-colors"
                    >
                      <ActIcon className="h-3 w-3" />
                    </button>
                  </TooltipTrigger>
                  <TooltipContent side="bottom" sideOffset={4}>{act.label}</TooltipContent>
                </Tooltip>
              )
            })}
            <button
              type="button"
              onClick={() => setFullscreenView(!fullscreenView)}
              aria-label={fullscreenView ? 'Exit fullscreen view' : 'Open fullscreen view'}
              title={fullscreenView ? 'Exit fullscreen (⌘⇧F)' : 'Fullscreen (⌘⇧F)'}
              className="grid h-5 w-5 place-items-center rounded hover:bg-accent text-muted-foreground hover:text-foreground"
            >
              {fullscreenView ? <Minimize2 className="h-3 w-3" /> : <Maximize2 className="h-3 w-3" />}
            </button>
          </div>

          <div className="flex-1 min-h-0 overflow-hidden">
            {/* Surface crossfade (design doc: no horizontal slides) — enter-surface */}
            <AnimatePresence initial={false} mode="wait">
              <motion.div
                key={activeView}
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.15, ease: 'easeOut' }}
                className="enter-surface h-full"
              >
                <ViewportContent view={activeView} />
              </motion.div>
            </AnimatePresence>
          </div>
        </motion.section>
      )}
    </AnimatePresence>
  )
}
