'use client'

import * as React from 'react'
import {
  Search,
  Command,
  CornerDownLeft,
  Hash,
  Folder,
  Clock,
  Plug,
  ShieldCheck,
  Brain,
  BarChart3,
  Plus,
  Settings,
  FileText,
  Globe,
  Terminal,
  Code2,
  Activity,
  FileSpreadsheet,
  Presentation,
  Sparkles,
  Sun,
  Moon,
  Cpu,
  Route,
  ScanSearch,
  PanelRight,
  Radar,
  GitCompare,
  GitBranch,
  MonitorSmartphone,
} from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { useAppStore, type ViewId, type SettingsSectionId } from '@/lib/store'
import { getModelsForAgentLive, isRuntimeUsable } from '@/lib/agents'
import { useTheme } from '@/components/theme-provider'
import { cn } from '@/lib/utils'

interface PaletteItem {
  id: string
  label: string
  hint?: string
  icon: React.ElementType
  group: 'actions' | 'navigate' | 'chats' | 'views' | 'settings'
  shortcut?: string
  keywords?: string
  onSelect: () => void
}

export function CommandPalette() {
  const open = useAppStore((s) => s.paletteOpen)
  const setOpen = useAppStore((s) => s.setPaletteOpen)
  const sessions = useAppStore((s) => s.sessions)
  const setActiveSession = useAppStore((s) => s.setActiveSession)
  const setActiveView = useAppStore((s) => s.setActiveView)
  const setCenterScreen = useAppStore((s) => s.setCenterScreen)
  const setSettingsSection = useAppStore((s) => s.setSettingsSection)
  const newSession = useAppStore((s) => s.newSession)
  const notify = useAppStore((s) => s.notify)
  const setSelectedAgent = useAppStore((s) => s.setSelectedAgent)
  const setSelectedModel = useAppStore((s) => s.setSelectedModel)
  const cycleModelVariant = useAppStore((s) => s.cycleModelVariant)
  const setAutoRoute = useAppStore((s) => s.setAutoRoute)
  const autoRoute = useAppStore((s) => s.autoRoute)
  const powerMode = useAppStore((s) => s.powerMode)
  const togglePowerMode = useAppStore((s) => s.togglePowerMode)
  const cockpitOpen = useAppStore((s) => s.cockpitOpen)
  const setCockpitOpen = useAppStore((s) => s.setCockpitOpen)
  const selectedAgentId = useAppStore((s) => s.selectedAgentId)
  const officePaths = useAppStore((s) => s.officePaths)
  // P58.11 — agent/model rows come from the live catalog, never the static seed
  // (same rule as P58.6); an empty live list contributes no rows instead of
  // offering runtimes that may not be installed.
  const liveAgents = useAppStore((s) => s.liveAgents)
  const { theme, toggle } = useTheme()

  const [query, setQuery] = React.useState('')
  const [selectedIdx, setSelectedIdx] = React.useState(0)

  const items: PaletteItem[] = React.useMemo(() => {
    const viewMeta: { id: ViewId; label: string; icon: React.ElementType; shortcut: string }[] = [
      { id: 'folder', label: 'Folder', icon: Folder, shortcut: '⌘⇧E' },
      { id: 'shell', label: 'Shell', icon: Terminal, shortcut: 'Ctrl+`' },
      { id: 'browse', label: 'Browse', icon: Globe, shortcut: '⌘⇧B' },
      { id: 'code', label: 'Code', icon: Code2, shortcut: '⌘⇧C' },
      // P58.11 — only the xlsx row carries ⌘⇧O: that chord opens the default
      // office view, so stamping it on all four rows overstated the map.
      { id: 'office-xlsx', label: officePaths['office-xlsx'] ? `Excel · ${officePaths['office-xlsx']!.split(/[\\/]/).pop()}` : 'Excel · no file open', icon: FileSpreadsheet, shortcut: '⌘⇧O' },
      { id: 'office-docx', label: officePaths['office-docx'] ? `Word · ${officePaths['office-docx']!.split(/[\\/]/).pop()}` : 'Word · no file open', icon: FileText, shortcut: '' },
      { id: 'office-pptx', label: officePaths['office-pptx'] ? `Slides · ${officePaths['office-pptx']!.split(/[\\/]/).pop()}` : 'Slides · no file open', icon: Presentation, shortcut: '' },
      { id: 'office-pdf', label: officePaths['office-pdf'] ? `PDF · ${officePaths['office-pdf']!.split(/[\\/]/).pop()}` : 'PDF · no file open', icon: FileText, shortcut: '' },
      { id: 'progress', label: 'Progress timeline', icon: Activity, shortcut: '⌘⇧P' },
      { id: 'diff', label: 'Diff (pending patches)', icon: GitCompare, shortcut: '⌘⇧D' },
      { id: 'timeline', label: 'Chat timeline', icon: Activity, shortcut: '' },
      { id: 'audit', label: 'Audit & Replay', icon: ShieldCheck, shortcut: '' },
      { id: 'storage', label: 'Storage intelligence', icon: BarChart3, shortcut: '' },
      { id: 'trajectory', label: 'Trajectory (context injection)', icon: ScanSearch, shortcut: '⌘⇧T' },
      { id: 'blueprint', label: 'Blueprint (plan board)', icon: FileText, shortcut: '' },
      { id: 'kanban', label: 'Kanban (parallel work)', icon: GitBranch, shortcut: '' },
      { id: 'generative', label: 'Generative UI', icon: Sparkles, shortcut: '' },
      { id: 'artifact', label: 'Artifact preview', icon: MonitorSmartphone, shortcut: '' },
      { id: 'desktop', label: 'Computer use (desktop)', icon: MonitorSmartphone, shortcut: '' },
      { id: 'local-server', label: 'Local OpenAI server', icon: Cpu, shortcut: '' },
    ]

    return [
      {
        id: 'new-session',
        label: 'New work',
        icon: Plus,
        group: 'actions',
        shortcut: '⌘N',
        onSelect: () => {
          newSession()
          setOpen(false)
        },
      },
      {
        id: 'toggle-theme',
        label: theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme',
        icon: theme === 'dark' ? Sun : Moon,
        group: 'actions',
        onSelect: () => {
          toggle()
          setOpen(false)
        },
      },
      {
        id: 'toggle-power-mode',
        label: powerMode ? 'Switch to casual mode' : 'Switch to power mode',
        hint: 'Show or hide the activity rail, viewport, and advanced controls',
        icon: PanelRight,
        group: 'actions',
        shortcut: '⌘.',
        keywords: 'casual power cockpit viewport rail',
        onSelect: () => {
          togglePowerMode()
          setOpen(false)
        },
      },
      {
        id: 'toggle-cockpit',
        label: cockpitOpen ? 'Close cockpit (multi-agent)' : 'Open cockpit (multi-agent)',
        hint: 'Live agent cards, interrupts, and per-agent stop/undo',
        icon: Radar,
        group: 'actions',
        keywords: 'cockpit flight deck agents interrupts',
        onSelect: () => {
          setCockpitOpen(!cockpitOpen)
          setOpen(false)
        },
      },
      ...sessions.map((s) => ({
        id: `session-${s.id}`,
        label: s.title,
        hint: s.preview,
        icon: Clock,
        group: 'chats' as const,
        keywords: s.preview,
        onSelect: () => {
          setActiveSession(s.id)
          setOpen(false)
        },
      })),
      ...viewMeta.map((v) => ({
        id: `view-${v.id}`,
        label: v.label,
        icon: v.icon,
        group: 'views' as const,
        shortcut: v.shortcut,
        onSelect: () => {
          setActiveView(v.id)
          setOpen(false)
        },
      })),
      {
        id: 'nav-automations',
        label: 'Open Automations',
        icon: Clock,
        group: 'navigate',
        shortcut: '⌘⇧A',
        onSelect: () => {
          setCenterScreen('automations')
          setOpen(false)
        },
      },
      {
        id: 'nav-home',
        label: 'Home',
        icon: Sparkles,
        group: 'navigate',
        onSelect: () => {
          setCenterScreen('home')
          setOpen(false)
        },
      },
      {
        id: 'nav-activity',
        label: 'Activity',
        icon: Clock,
        group: 'navigate',
        onSelect: () => {
          setCenterScreen('activity')
          setOpen(false)
        },
      },
      {
        id: 'nav-projects',
        label: 'Projects',
        icon: Folder,
        group: 'navigate',
        onSelect: () => {
          setCenterScreen('projects')
          setOpen(false)
        },
      },
      {
        id: 'nav-files',
        label: 'Files',
        icon: FileText,
        group: 'navigate',
        onSelect: () => {
          setCenterScreen('files')
          setOpen(false)
        },
      },
      {
        id: 'nav-guard',
        label: 'Guard (control center)',
        icon: ShieldCheck,
        group: 'settings',
        shortcut: '⌘⇧G',
        onSelect: () => {
          setCenterScreen('guard')
          setOpen(false)
        },
      },
      {
        id: 'nav-connectors',
        label: 'Connectors',
        icon: Plug,
        group: 'settings',
        onSelect: () => {
          setCenterScreen('connectors')
          setOpen(false)
        },
      },
      {
        id: 'nav-memory',
        label: 'Memory',
        icon: Brain,
        group: 'settings',
        shortcut: '⌘⇧M',
        onSelect: () => {
          setSettingsSection('memory')
          setCenterScreen('settings')
          setOpen(false)
        },
      },
      {
        id: 'nav-analytics',
        label: 'Analytics',
        icon: BarChart3,
        group: 'settings',
        onSelect: () => {
          setCenterScreen('analytics')
          setOpen(false)
        },
      },
      {
        id: 'nav-settings',
        label: 'Open Settings',
        icon: Settings,
        group: 'settings',
        onSelect: () => {
          setCenterScreen('settings')
          setOpen(false)
        },
      },
      ...([
        ['chat', 'Chat & Auto-run'],
        ['subagents', 'Subagents'],
        ['tool-log', 'Tool log'],
        ['permissions', 'Permissions'],
        ['browser', 'Browser & Network'],
        ['indexing', 'Indexing & LSP'],
        ['voice', 'Voice'],
        ['mobile', 'Mobile'],
        ['mcp', 'MCP'],
        ['marketplace', 'Marketplace'],
        ['skills', 'Skills'],
        ['hooks', 'Hooks'],
        ['launch', 'Launch CLI'],
        ['local', 'Local models'],
        ['import', 'Import & migrate'],
        ['rules', 'Rules & memory'],
      ] as [SettingsSectionId, string][]).map(([id, label]) => ({
        id: `settings-${id}`,
        label: `Settings · ${label}`,
        icon: Settings,
        group: 'settings' as const,
        keywords: `settings ${label} ${id}`,
        onSelect: () => {
          setSettingsSection(id)
          setCenterScreen('settings')
          setOpen(false)
        },
      })),
      // === Agent runtime switching ===
      // P58.11 — no invented chords: switching agent has no keyboard binding
      // (the old ⌘⇧1/2/3 labels were never implemented in the key handler).
      ...liveAgents.filter((a) => isRuntimeUsable(a)).map((a) => ({
        id: `agent-${a.id}`,
        label: `Switch to ${a.name}`,
        hint: `${a.vendor} · installed`,
        icon: Cpu,
        group: 'actions' as const,
        keywords: `agent runtime ${a.vendor} ${a.name}`,
        onSelect: () => {
          setSelectedAgent(a.id)
          notify(`Switched to ${a.name}`)
          setOpen(false)
        },
      })),
      // P71.2d — no desktop model entries and no model cycle: AgentCowork owns no
      // model surface to switch. A bound agent's model is changed through that
      // agent's own ACP config options, which the composer picker renders; a
      // palette row that pinned a desktop model would name something no agent
      // receives. The rows were removed rather than left as no-ops.
      // === Auto-route toggle ===
      {
        id: 'toggle-autoroute',
        label: autoRoute ? 'Disable auto-route' : 'Enable auto-route',
        hint: 'Automatically pick best runtime per task kind',
        icon: Route,
        group: 'settings',
        keywords: 'auto route routing',
        onSelect: () => {
          setAutoRoute(!autoRoute)
          notify(autoRoute ? 'Auto-route off' : 'Auto-route on')
          setOpen(false)
        },
      },
    ]
  }, [sessions, theme, toggle, powerMode, togglePowerMode, setActiveSession, setActiveView, setCenterScreen, setSettingsSection, newSession, setOpen, notify, setSelectedAgent, setSelectedModel, cycleModelVariant, setAutoRoute, autoRoute, selectedAgentId, liveAgents, officePaths, cockpitOpen, setCockpitOpen])

  const filtered = React.useMemo(() => {
    if (!query) return items
    const q = query.toLowerCase()
    return items.filter((it) =>
      it.label.toLowerCase().includes(q) ||
      it.keywords?.toLowerCase().includes(q) ||
      it.group.toLowerCase().includes(q)
    )
  }, [items, query])

  const grouped = React.useMemo(() => {
    const g: Record<string, PaletteItem[]> = {}
    filtered.forEach((it) => {
      g[it.group] = g[it.group] || []
      g[it.group].push(it)
    })
    return g
  }, [filtered])

  const flatList = Object.values(grouped).flat()

  React.useEffect(() => {
    setSelectedIdx(0)
  }, [query])

  React.useEffect(() => {
    if (!open) return
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        setSelectedIdx((i) => Math.min(i + 1, flatList.length - 1))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setSelectedIdx((i) => Math.max(i - 1, 0))
      } else if (e.key === 'Enter') {
        e.preventDefault()
        flatList[selectedIdx]?.onSelect()
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [flatList, selectedIdx, open])

  // Reset query when palette closes
  React.useEffect(() => {
    if (!open) setQuery('')
  }, [open])

  let runningIdx = -1

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="scale-in-palette p-0 gap-0 max-w-[640px] top-[15%] translate-y-0 overflow-hidden">
        <DialogHeader className="sr-only">
          <DialogTitle>Command palette</DialogTitle>
        </DialogHeader>
        <div className="flex items-center gap-2 px-3 h-11 border-b border-border">
          <Search className="h-4 w-4 text-muted-foreground" />
          <input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search chats, files, commands…"
            className="flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground/60"
          />
          <kbd className="text-[10px] text-muted-foreground/60 font-mono border border-border rounded px-1.5 py-0.5">
            ESC
          </kbd>
        </div>
        <div className="max-h-[420px] overflow-y-auto scroll-thin p-2">
          {flatList.length === 0 && (
            <div className="py-12 text-center text-sm text-muted-foreground">
              No results for "{query}"
            </div>
          )}
          {Object.entries(grouped).map(([group, list]) => (
            <div key={group} className="mb-1">
              <div className="px-2 py-1.5 text-[10px] uppercase tracking-wider text-muted-foreground/70 font-semibold">
                {group}
              </div>
              {list.map((item) => {
                runningIdx += 1
                const idx = runningIdx
                const Icon = item.icon
                const isSelected = idx === selectedIdx
                return (
                  <button
                    key={item.id}
                    onClick={item.onSelect}
                    onMouseEnter={() => setSelectedIdx(idx)}
                    className={cn(
                      'w-full flex items-center gap-3 px-2 py-1.5 rounded-md text-sm transition-colors',
                      isSelected ? 'bg-accent text-foreground' : 'hover:bg-accent/60'
                    )}
                  >
                    <Icon className={cn('h-4 w-4 shrink-0', isSelected ? 'text-brand' : 'text-muted-foreground')} />
                    <div className="flex-1 text-left min-w-0">
                      <div className="truncate">{item.label}</div>
                      {item.hint && (
                        <div className="text-[11px] text-muted-foreground/70 truncate">{item.hint}</div>
                      )}
                    </div>
                    {item.shortcut && (
                      <kbd className="text-[10px] text-muted-foreground/60 font-mono">
                        {item.shortcut}
                      </kbd>
                    )}
                    {isSelected && (
                      <CornerDownLeft className="h-3 w-3 text-brand" />
                    )}
                  </button>
                )
              })}
            </div>
          ))}
        </div>
        <div className="flex items-center justify-between px-3 py-1.5 border-t border-border bg-sidebar/40 text-[10px] text-muted-foreground">
          <div className="flex items-center gap-2">
            <span className="flex items-center gap-1">
              <Command className="h-2.5 w-2.5" /> K
            </span>
            <span>·</span>
            <span>AgentCowork</span>
          </div>
          <div className="flex items-center gap-3">
            <span>↑↓ navigate</span>
            <span>↵ select</span>
            <span>esc close</span>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
