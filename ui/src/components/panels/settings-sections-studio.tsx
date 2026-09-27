'use client'

import { useEffect, useMemo, useState } from 'react'
import {
  Bell,
  Cpu,
  Globe,
  HardDrive,
  Mic,
  Plus,
  QrCode,
  Radio,
  Smartphone,
  Sparkles,
  Trash2,
  FileSearch,
  UsersRound,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Slider } from '@/components/ui/slider'
import { Switch } from '@/components/ui/switch'
import {
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/lib/store'
import { inTauri } from '@/lib/tauri'
import { type PermissionMode, usePref } from '@/lib/ui-prefs'
import {
  addBlockReason,
  backgroundInputView,
  interactionCopy,
  pathTail,
  pickerOptions,
  readinessView,
  sourceLabel,
  type DesktopInstalledApp,
  type DesktopPolicy,
  type DesktopReadiness,
} from '@/lib/desktop-apps'
import {
  browserGetConfig,
  browserListInstalled,
  browserSetConfig,
  type BrowserCandidate,
  type BrowserConfig,
  type BrowserChannel,
} from '@/lib/browser'
import { Row, SectionShell } from './settings-shared'

function Honest({ children }: { children: React.ReactNode }) {
  return (
    <p className="rounded-md border border-warning/30 bg-warning/8 px-3 py-2 text-[10px] leading-relaxed text-warning/90">
      {children}
    </p>
  )
}

function RadioCard({
  selected,
  title,
  desc,
  onSelect,
}: {
  selected: boolean
  title: string
  desc: string
  onSelect: () => void
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        'flex w-full items-start gap-3 rounded-md border px-3 py-2.5 text-left transition-colors',
        selected
          ? 'border-brand/60 bg-brand/10'
          : 'border-border/50 bg-background/30 hover:border-border hover:bg-accent/40',
      )}
    >
      <span
        className={cn(
          'mt-0.5 grid h-3.5 w-3.5 shrink-0 place-items-center rounded-full border',
          selected ? 'border-brand' : 'border-muted-foreground/40',
        )}
      >
        {selected && <span className="h-1.5 w-1.5 rounded-full bg-brand" />}
      </span>
      <span className="min-w-0">
        <span className="block text-xs font-medium text-foreground">{title}</span>
        <span className="mt-0.5 block text-[10px] text-muted-foreground">{desc}</span>
      </span>
    </button>
  )
}

export function NotificationsSection() {
  // P58.9 — chat/task are read by the live bridge (`pushLive` in lib/bridge.ts)
  // and gate which wire events reach this list; Guard approvals are `always`
  // and are never suppressed by a preference. Banner/Sound/Volume have no
  // engine in this build, so they are disabled with a truthful status instead
  // of storing a preference nobody reads.
  const [chat, setChat] = usePref('notify.chat', true)
  const [quest, setQuest] = usePref('notify.quest', true)
  const notify = useAppStore((s) => s.notify)
  return (
    <SectionShell title="Notifications" desc="System toasts when a chat, task, or wiki job needs you">
      <Honest>
        These switches gate the live activity stream. Chat covers turn failures, cancellations, budgets, and tool
        errors; Task covers automation/monitor jobs. Guard approval requests are always shown — a notification
        preference never hides a pending approval.
      </Honest>
      <Row label="Chat notifications" desc="Turn failures, cancellations, budgets, and tool errors">
        <Switch checked={chat} onCheckedChange={setChat} />
      </Row>
      <Row label="Task notifications" desc="When a long-running automation or monitor job finishes or waits">
        <Switch checked={quest} onCheckedChange={setQuest} />
      </Row>
      <Row label="Repo wiki notifications" desc="When generated project docs finish">
        <span title="No repo-wiki generator in this build — the switch has nothing to gate yet">
          <Switch checked={false} disabled />
        </span>
      </Row>
      <Row label="Banner" desc="Windows / OS notification banner">
        <span title="No native OS banner bridge in this build — the in-app activity list is the live surface">
          <Switch checked={false} disabled />
        </span>
      </Row>
      <Row label="Sound" desc="Play a chime when a job needs you">
        <span title="No audio engine in this build — notification sounds are a staged surface">
          <Switch checked={false} disabled />
        </span>
      </Row>
      <Row label="Volume" desc="Chime volume">
        <div className="flex w-56 items-center gap-3" title="No audio engine in this build — volume is a staged surface">
          <Slider value={[0]} min={0} max={100} step={1} disabled />
          <span className="w-10 font-mono text-xs text-muted-foreground/50">—</span>
        </div>
      </Row>
      <div className="space-y-1.5">
        <div className="text-xs font-medium text-foreground">Sound effects</div>
        {[
          ['Task completed', 'done'],
          ['Waiting for action', 'ask'],
          ['Abnormally stopped', 'fail'],
        ].map(([label, id]) => (
          <div key={id} className="flex items-center justify-between rounded-md border border-border/50 bg-background/30 px-3 py-2">
            <span className="flex items-center gap-2 text-xs">
              <Bell className="h-3.5 w-3.5 text-brand" />
              {label}
            </span>
            <div className="flex gap-1">
              <Button size="sm" variant="ghost" className="h-6 px-2 text-[10px]" disabled title="No audio engine in this build — notification sounds are a staged surface">
                Preview
              </Button>
              <Button size="sm" variant="ghost" className="h-6 px-2 text-[10px]" disabled title="No audio engine in this build — custom sounds are a staged surface">
                Replace
              </Button>
            </div>
          </div>
        ))}
      </div>
    </SectionShell>
  )
}

export function VoiceSection() {
  const [input, setInput] = usePref('voice.input', true)
  const [tts, setTts] = usePref('voice.tts', false)
  const [pushToTalk, setPushToTalk] = usePref('voice.ptt', false)
  const [device, setDevice] = usePref('voice.device', 'default')
  const [noise, setNoise] = usePref('voice.noise', true)
  const [autoSend, setAutoSend] = usePref('voice.autoSend', true)
  const [speed, setSpeed] = usePref('voice.speed', 'normal')
  const [voiceName, setVoiceName] = usePref('voice.name', 'default')
  const [realtime, setRealtime] = usePref('voice.realtime', false)
  const [print, setPrint] = usePref('voice.print', false)
  const notify = useAppStore((s) => s.notify)
  return (
    <SectionShell title="Voice" desc="Mic, noise, shortcuts, and spoken replies">
      <Honest>
        Voice capture runs the crate VAD pipeline (composer mic). No on-device STT engine is installed, so
        a capture never invents a transcript. Read-aloud uses the platform speechSynthesis engine when present.
      </Honest>
      <div className="text-xs font-medium text-foreground">General</div>
      <Row label="Input device" desc="Microphone used for voice input">
        <Select value={device} onValueChange={setDevice}>
          <SelectTrigger className="h-8 w-56 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="default">System default</SelectItem>
            <SelectItem value="dji">External USB / DJI mic</SelectItem>
            <SelectItem value="headset">Headset</SelectItem>
          </SelectContent>
        </Select>
      </Row>
      <Row label="External mic auto-send" desc="Send when a USB / DJI-style mic stops recording">
        <Switch checked={autoSend} onCheckedChange={setAutoSend} />
      </Row>
      <div className="pt-1 text-xs font-medium text-foreground">Voice input</div>
      <Row label="Voice input" desc="Hold or tap the composer mic">
        <Switch checked={input} onCheckedChange={setInput} />
      </Row>
      <Row label="Shortcut" desc="Stays in sync with Keyboard shortcuts">
        <div className="flex items-center gap-1.5">
          <kbd className="rounded border border-border px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">Ctrl+Shift+R</kbd>
          <Button size="sm" variant="ghost" className="h-6 px-2 text-[10px]" onClick={() => useAppStore.getState().setSettingsSection('keyboard')}>
            Edit
          </Button>
        </div>
      </Row>
      <Row label="Voiceprint noise reduction" desc="Filter other speakers in the room">
        <Switch checked={noise} onCheckedChange={setNoise} />
      </Row>
      <Row label="Term correction" desc="Names, project names, jargon for recognition">
        <Button size="sm" variant="outline" className="h-7 text-[10px]" disabled title="Term correction lands with the v1 STT stack — not wired yet">
          No terms yet
        </Button>
      </Row>
      <Row label="History" desc="Up to 100 voice inputs from the last 30 days">
        <Button size="sm" variant="ghost" className="h-6 px-2 text-[10px]" disabled title="Voice history records once the v1 capture stack records — not wired yet">
          View history
        </Button>
      </Row>
      <div className="pt-1 text-xs font-medium text-foreground">Realtime voice</div>
      <Row label="Realtime voice">
        <Switch checked={realtime} onCheckedChange={setRealtime} />
      </Row>
      <Row label="Spoken voice">
        <Select value={voiceName} onValueChange={setVoiceName}>
          <SelectTrigger className="h-8 w-40 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="default">Default</SelectItem>
            <SelectItem value="warm">Warm</SelectItem>
            <SelectItem value="low">Low</SelectItem>
          </SelectContent>
        </Select>
      </Row>
      <Row label="Speaking speed">
        <Select value={speed} onValueChange={setSpeed}>
          <SelectTrigger className="h-8 w-32 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="slow">Slow</SelectItem>
            <SelectItem value="normal">Normal</SelectItem>
            <SelectItem value="fast">Fast</SelectItem>
          </SelectContent>
        </Select>
      </Row>
      <Row label="Voiceprint recognition" desc="Prefer your voice in live calls">
        <Switch checked={print} onCheckedChange={setPrint} />
      </Row>
      <Row label="Read replies aloud">
        <Switch checked={tts} onCheckedChange={setTts} />
      </Row>
      <Row label="Push-to-talk">
        <Switch checked={pushToTalk} onCheckedChange={setPushToTalk} />
      </Row>
      <Button size="sm" variant="outline" className="h-7 text-[10px]" disabled title="Microphone test needs the v1 capture pipeline — not wired yet">
        <Mic className="h-3.5 w-3.5" /> Test microphone
      </Button>
    </SectionShell>
  )
}

export function MobileSection() {
  // Remote session handoff + mobile companion is post-v1 (capabilities.yaml
  // H18, ARCH/09 ⚪). Per P50.4.7 the section renders a truthful post-v1
  // surface: the pairing preview stays visible as a forward-looking cue, but
  // no persisted dead switches pretend remote pairing works today.
  return (
    <SectionShell title="Mobile" desc="Pair a phone to resume a chat on the LAN">
      <Honest>
        Remote session handoff + mobile companion is post-v1 (H18) — not built. This preview shows the intended
        pairing flow; no toggle below is live yet.
      </Honest>
      <div className="flex items-center gap-4 rounded-md border border-border/50 bg-background/30 p-4">
        <div className="grid h-28 w-28 place-items-center rounded-md border border-dashed border-border bg-background/40">
          <QrCode className="h-12 w-12 text-muted-foreground/50" />
        </div>
        <div className="space-y-2">
          <div className="text-xs font-medium">Connect this workspace with mobile</div>
          <p className="max-w-sm text-[10px] text-muted-foreground">
            Install the phone app, sign in with the same vault, then scan. No founder server.
          </p>
          <div className="flex flex-wrap gap-1.5">
            <Button size="sm" className="h-7 bg-brand text-black hover:bg-brand" disabled title="Post-v1 (H18) — remote pairing backend not wired">
              <Smartphone className="h-3.5 w-3.5" /> Install mobile
            </Button>
            <Button size="sm" variant="outline" className="h-7 text-[10px]" disabled title="Post-v1 (H18) — pairing backend not wired">
              Refresh code
            </Button>
          </div>
        </div>
      </div>
      <Row label="Allow remote chats" desc="Post-v1 (H18) — phone view/continue is not built; switch inert">
        <Switch disabled />
      </Row>
      <Row label="Allow phone to control this device" desc="Post-v1 (H18) — control is not built; switch inert">
        <Switch disabled />
      </Row>
      <Row label="Keep the computer awake" desc="Unrelated to pairing — stays available as a native concern">
        <Switch disabled title="Wake-lock is a post-v1 pairing concern — not wired"/>
      </Row>
    </SectionShell>
  )
}

export function ChatAutoRunSection() {
  const permissionMode = useAppStore((s) => s.permissionMode)
  const setPermissionMode = useAppStore((s) => s.setPermissionMode)
  const [ctx, setCtx] = usePref('chat.ctx', 4096)
  const [cloudNet, setCloudNet] = usePref('chat.cloudNet', true)
  const [queue, setQueue] = usePref('chat.queue', true)
  return (
    <SectionShell title="Chat & Auto-run" desc="How much the agent may do without asking — and local context">
      <Honest>
        Autonomy radios call `guard_set_autonomy` (H34). Local context, cloud-net, and queue on this page are localStorage only — they do not change the next turn. The Guard capability matrix and tool allow-list live in Settings → Permissions (`guard_permissions_matrix` / `guard_set_policy_rules`), not here.
      </Honest>
      <div className="space-y-1.5">
        <div className="text-xs font-medium">Auto-run</div>
        <RadioCard
          selected={permissionMode === 'sandbox'}
          title="🛡 Sandbox"
          desc="Plan + read-only. Every mutation is denied."
          onSelect={() => setPermissionMode('sandbox')}
        />
        <RadioCard
          selected={permissionMode === 'ask'}
          title="👀 Ask"
          desc="Default. Safe reads auto-allow; mutations show a Guard-2 card."
          onSelect={() => setPermissionMode('ask')}
        />
        <RadioCard
          selected={permissionMode === 'auto'}
          title="⚡ Auto"
          desc="Low-risk workspace writes auto-allow. Destructive, secrets, money, and new domains still ask."
          onSelect={() => setPermissionMode('auto')}
        />
        <RadioCard
          selected={permissionMode === 'full'}
          title="🚀 Maximum"
          desc="Maximum autonomy within hard floors — never a Guard bypass. Destructive / secret / financial / R4 still ask."
          onSelect={() => setPermissionMode('full')}
        />
      </div>
      <Row label="Local context window" desc="Soft cap used when a local runtime is selected (Ollama-style)">
        <Select value={String(ctx)} onValueChange={(v) => setCtx(Number(v))}>
          <SelectTrigger className="h-8 w-36 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            {[2048, 4096, 8192, 16384, 32768].map((n) => (
              <SelectItem key={n} value={String(n)}>{n.toLocaleString()} tok</SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Row>
      <Row label="Cloud / network for local models" desc="Let a local runtime fetch tokenizer files">
        <Switch checked={cloudNet} onCheckedChange={setCloudNet} />
      </Row>
      <Row label="Queue follow-up turns" desc="Stack messages while a turn is running">
        <Switch checked={queue} onCheckedChange={setQueue} />
      </Row>
    </SectionShell>
  )
}

// P50.2.x — removed: PermissionsSection (duplicate of ChatAutoRunSection's
// four RadioCards plus a config button with no editor). The permissions
// settings route renders ChatAutoRunSection (see settings-panel SectionBody).

export function BrowserNetworkSection() {
  const [engine, setEngine] = usePref('browser.engine', 'builtin')
  const [protect, setProtect] = usePref('browser.protect', 'off')
  const [http2, setHttp2] = usePref('browser.http2', true)
  const [proxy, setProxy] = usePref('browser.proxy', '')
  const [localLinks, setLocalLinks] = usePref('browser.localLinks', 'inapp')
  const [webLinks, setWebLinks] = usePref('browser.webLinks', true)
  const notify = useAppStore((s) => s.notify)

  const [browserConfig, setBrowserConfigState] = useState<BrowserConfig>({
    preferred_channel: 'auto',
    custom_executable_path: null,
    profile_mode: 'isolated',
    headless: true,
    extra_args: ['--mute-audio'],
  })
  const [candidates, setCandidates] = useState<BrowserCandidate[]>([])
  const [loadingConfig, setLoadingConfig] = useState(true)

  useEffect(() => {
    let active = true
    void (async () => {
      try {
        const [cfg, list] = await Promise.all([
          browserGetConfig(),
          browserListInstalled(),
        ])
        if (active) {
          setBrowserConfigState(cfg)
          setCandidates(list)
          setLoadingConfig(false)
        }
      } catch {
        if (active) setLoadingConfig(false)
      }
    })()
    return () => {
      active = false
    }
  }, [])

  const updateConfig = async (patch: Partial<BrowserConfig>) => {
    const updated = { ...browserConfig, ...patch }
    setBrowserConfigState(updated)
    try {
      await browserSetConfig(updated)
      notify('Browser configuration saved', 'default')
    } catch (e) {
      notify(e instanceof Error ? e.message : 'Failed to save browser config', 'error')
    }
  }

  const selectedCandidate = useMemo(() => {
    if (browserConfig.preferred_channel === 'auto') {
      return candidates.find((c) => c.is_default) ?? candidates[0]
    }
    return candidates.find((c) => c.channel === browserConfig.preferred_channel)
  }, [candidates, browserConfig.preferred_channel])

  return (
    <SectionShell title="Browser & Network" desc="Where the agent opens pages, browser selection, protection, HTTP, required domains">
      <div className="text-xs font-medium">Browser Selection</div>
      <Row label="Preferred Browser" desc="Choose Brave, Chrome, Edge, Chromium, Arc, Vivaldi, or custom binary">
        <Select
          value={browserConfig.preferred_channel}
          onValueChange={(v) => updateConfig({ preferred_channel: v as BrowserChannel })}
          disabled={loadingConfig}
        >
          <SelectTrigger className="h-8 w-56 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="auto">Auto-detect Default</SelectItem>
            <SelectItem value="brave">Brave Browser</SelectItem>
            <SelectItem value="chrome">Google Chrome</SelectItem>
            <SelectItem value="edge">Microsoft Edge</SelectItem>
            <SelectItem value="chromium">Chromium</SelectItem>
            <SelectItem value="arc">Arc Browser</SelectItem>
            <SelectItem value="vivaldi">Vivaldi</SelectItem>
            <SelectItem value="custom">Custom Path...</SelectItem>
          </SelectContent>
        </Select>
      </Row>

      {browserConfig.preferred_channel === 'custom' && (
        <Row label="Custom Executable Path" desc="Full path to custom browser binary (must support CDP)">
          <Input
            value={browserConfig.custom_executable_path ?? ''}
            onChange={(e) => updateConfig({ custom_executable_path: e.target.value.trim() || null })}
            placeholder="/usr/bin/google-chrome or C:\...\chrome.exe"
            className="h-8 w-64 font-mono text-xs"
          />
        </Row>
      )}

      {selectedCandidate && (
        <div className="rounded-md border border-neutral-800 bg-neutral-900/40 p-2.5 text-xs text-neutral-300">
          <div className="flex items-center justify-between">
            <span className="font-medium text-neutral-200">{selectedCandidate.name}</span>
            {selectedCandidate.version && (
              <Badge variant="outline" className="text-[10px] font-mono">
                v{selectedCandidate.version}
              </Badge>
            )}
          </div>
          <p className="mt-1 truncate font-mono text-[10px] text-neutral-400" title={selectedCandidate.executable_path}>
            {selectedCandidate.executable_path}
          </p>
        </div>
      )}

      <Row label="Headless browser" desc="Run browser in background without showing a window">
        <Switch
          checked={browserConfig.headless}
          onCheckedChange={(v) => updateConfig({ headless: v })}
        />
      </Row>

      <Row label="Profile Isolation" desc="Isolate a profile per channel in the AgentCowork data directory">
        <Select
          value={browserConfig.profile_mode}
          onValueChange={(v) => updateConfig({ profile_mode: v as 'isolated' | 'paired' })}
        >
          <SelectTrigger className="h-8 w-44 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="isolated">Channel Isolated</SelectItem>
            <SelectItem value="paired">Paired Profile</SelectItem>
          </SelectContent>
        </Select>
      </Row>

      <div className="pt-2 text-xs font-medium">Browser Automation</div>
      <Row label="Browser automation" desc="Which surface receives agent clicks">
        <Select value={engine} onValueChange={setEngine}>
          <SelectTrigger className="h-8 w-52 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="builtin">Browse tab</SelectItem>
            <SelectItem value="system">System Chrome / Edge / Brave</SelectItem>
            <SelectItem value="ask">Ask each time</SelectItem>
          </SelectContent>
        </Select>
      </Row>
      <Row label="Browser protection" desc="Stop the agent from running browser tools on its own">
        <Select value={protect} onValueChange={setProtect}>
          <SelectTrigger className="h-8 w-36 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="off">Off</SelectItem>
            <SelectItem value="ask">Ask</SelectItem>
            <SelectItem value="block">Block</SelectItem>
          </SelectContent>
        </Select>
      </Row>
      <Row label="Open local links in Browse" desc="localhost URLs open in the in-app tab">
        <Switch checked={localLinks === 'inapp'} onCheckedChange={(v) => setLocalLinks(v ? 'inapp' : 'system')} />
      </Row>
      <Row label="Open web links in Browse" desc="http/https open in the in-app tab">
        <Switch checked={webLinks} onCheckedChange={setWebLinks} />
      </Row>
      <div className="pt-1 text-xs font-medium">Network</div>
      <Row label="HTTP compatibility" desc="HTTP/2 for low-latency streams; drop to HTTP/1.1 behind some VPNs">
        <Select value={http2 ? 'h2' : 'h1'} onValueChange={(v) => setHttp2(v === 'h2')}>
          <SelectTrigger className="h-8 w-28 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="h2">HTTP/2</SelectItem>
            <SelectItem value="h1">HTTP/1.1</SelectItem>
          </SelectContent>
        </Select>
      </Row>
      <Row label="HTTPS / SOCKS proxy" desc="e.g. http://127.0.0.1:7890">
        <Input value={proxy} onChange={(e) => setProxy(e.target.value)} placeholder="none" className="h-8 w-56 font-mono text-xs" />
      </Row>
      <Row label="Required domains" desc="Must be reachable for models and MCP">
        <Button
          size="sm"
          variant="ghost"
          className="h-6 px-2 text-[10px]"
          onClick={() => {
            const domains = 'huggingface.co, api hosts of your configured providers, your MCP server URLs'
            void navigator.clipboard
              ?.writeText(domains)
              .then(() => notify('Required-domains hint copied'))
              .catch(() => notify(domains))
          }}
        >
          Copy · Show
        </Button>
      </Row>
      <Row label="Network diagnostics">
        <Button
          size="sm"
          variant="outline"
          className="h-7 text-[10px]"
          onClick={() =>
            void (async () => {
              try {
                const { doctorReport } = await import('@/lib/doctor')
                const report = await doctorReport()
                const bad = report.checks.filter((c) => c.status !== 'ok')
                notify(
                  bad.length === 0
                    ? `Diagnostic: all ${report.checks.length} checks ok`
                    : `Diagnostic: ${bad.length} attention — ${bad.map((c) => c.name).join(', ')} (see Doctor)`,
                  bad.length === 0 ? 'default' : 'error',
                )
                if (bad.length > 0) useAppStore.getState().setSettingsSection('doctor')
              } catch (e) {
                notify(e instanceof Error ? e.message : 'Diagnostic failed', 'error')
              }
            })()
          }
        >
          <Globe className="h-3.5 w-3.5" /> Run diagnostic
        </Button>
      </Row>
    </SectionShell>
  )
}

export function IndexingSection() {
  const [lsp, setLsp] = usePref('index.lsp', true)
  const [lspWt, setLspWt] = usePref('index.lspWorktree', true)
  const [grep, setGrep] = usePref('index.grep', true)
  const [hier, setHier] = usePref('index.hierIgnore', false)
  const [sym, setSym] = usePref('index.symlinks', false)
  const [maxLocal, setMaxLocal] = usePref('index.maxLocal', 8)
  const [maxRemote, setMaxRemote] = usePref('index.maxRemote', 2)
  const [pct] = usePref('index.pct', 100)
  const notify = useAppStore((s) => s.notify)
  return (
    <SectionShell title="Code intelligence & indexing" desc="Grep index, ignore rules, LSP counts, extra docs">
      <Honest>LSP runner is crate-landed (`agentcowork-codeintel`). This panel does not start rust-analyzer/pyright until those binaries are install-gated.</Honest>
      <div className="rounded-md border border-border/50 bg-background/30 px-3 py-2">
        <div className="flex items-center justify-between text-xs">
          <span>Code index</span>
          <span className="font-mono text-emerald-300">{pct}%</span>
        </div>
        <div className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-muted">
          <div className="h-full bg-emerald-500" style={{ width: `${pct}%` }} />
        </div>
        <Button
          size="sm"
          variant="ghost"
          className="mt-1 h-6 px-0 text-[10px]"
          disabled
          title="Workspace reindex lands with the P20 SeekStorm/FTS path — not wired yet"
        >
          Reindex
        </Button>
      </div>
      <div className="text-xs font-medium">Codebase</div>
      <Row label="Index repositories for instant grep" desc="Local only. Speeds filename and content search">
        <Switch checked={grep} onCheckedChange={setGrep} />
      </Row>
      <div className="text-xs font-medium">Ignore files</div>
      <Row label="Hierarchical ignore" desc="Apply ignore files to all subdirectories">
        <Switch checked={hier} onCheckedChange={setHier} />
      </Row>
      <Row label="Ignore symlinks during discovery" desc="Skip symlink loops. Enable only when ignore files are reachable without them">
        <Switch checked={sym} onCheckedChange={setSym} />
      </Row>
      <Row label="Edit ignore file">
        <Button
          size="sm"
          variant="outline"
          className="h-7 text-[10px]"
          onClick={() =>
            void (async () => {
              const st = useAppStore.getState()
              if (!inTauri()) {
                notify('Ignore-file editing needs the Tauri shell', 'error')
                return
              }
              const folder = st.taskFolder
              if (!folder) {
                notify('Attach a workspace folder first (chat empty state → Open folder)', 'error')
                return
              }
              // DEC-053: `.everyaiosignore` stays — it is a user-authored
              // on-disk filename with no TS read path to attach a fallback
              // to; renaming the constructed path would orphan existing
              // files. Any rename + migration belongs to Core (Rust).
              const path = `${folder.replace(/\/+$/, '')}/.everyaiosignore`
              try {
                const { fsReadFile } = await import('@/lib/fs')
                const f = await fsReadFile(path).catch(() => ({ content: '' }))
                window.dispatchEvent(
                  new CustomEvent('agentcowork:open-file', { detail: { path, content: f.content } }),
                )
                st.setActiveView('code')
              } catch (e) {
                notify(e instanceof Error ? e.message : 'Could not open the ignore file', 'error')
              }
            })()
          }
        >
          Edit ignore
        </Button>
      </Row>
      <div className="text-xs font-medium">LSP</div>
      <Row label="Enable language servers" desc="Diagnostics in the Code view">
        <Switch checked={lsp} onCheckedChange={setLsp} />
      </Row>
      <Row label="Enable LSPs for worktrees" desc="Agent checkouts get their own servers">
        <Switch checked={lspWt} onCheckedChange={setLspWt} />
      </Row>
      <Row label="Max local LSP workspaces">
        <Select value={String(maxLocal)} onValueChange={(v) => setMaxLocal(Number(v))}>
          <SelectTrigger className="h-8 w-24 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            {[1, 2, 4, 8, 16].map((n) => (
              <SelectItem key={n} value={String(n)}>{n}</SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Row>
      <Row label="Max remote LSP workspaces">
        <Select value={String(maxRemote)} onValueChange={(v) => setMaxRemote(Number(v))}>
          <SelectTrigger className="h-8 w-24 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            {[0, 1, 2, 4].map((n) => (
              <SelectItem key={n} value={String(n)}>{n}</SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Row>
      <Row label="Docs for AI Q&A" desc="URL or local upload as extra context">
        <Button
          size="sm"
          className="h-7 bg-brand text-black hover:bg-brand"
          disabled
          title="Document ingestion for retrieval lands with the P20 index — not wired yet"
        >
          <Plus className="h-3.5 w-3.5" /> Add docs
        </Button>
      </Row>
    </SectionShell>
  )
}

// P50.2.6 — removed: McpMarketSection + MCP_DIRECTORY (static 5-row catalog
// with a fake attach). The MCP surface is the live Connectors panel
// (vault OAuth + attached servers + store catalog); the settings MCP route
// renders it (see settings-panel SectionBody).

const PLUGIN_CATS = ['Featured', 'Code review', 'Coding', 'Database', 'Design', 'DevOps', 'Knowledge', 'Workflow', 'Installed']

export function MarketplaceSection() {
  const [cat, setCat] = useState('Featured')
  const notify = useAppStore((s) => s.notify)
  return (
    <SectionShell
      title="Marketplace"
      desc="Plugins that bundle MCPs, skills, and agents. Installed list is empty until you add one."
      action={
        <Button
          size="sm"
          variant="outline"
          className="h-7 text-[10px]"
          disabled
          title="Plugin authoring lands with the post-v1 marketplace fetch — not wired yet"
        >
          Create plugin
        </Button>
      }
    >
      <div className="flex flex-wrap gap-1">
        {PLUGIN_CATS.map((c) => (
          <button
            key={c}
            type="button"
            onClick={() => setCat(c)}
            className={cn(
              'rounded-md border px-2 py-1 text-[10px]',
              cat === c ? 'border-brand bg-brand/15 text-brand' : 'border-border text-muted-foreground hover:text-foreground',
            )}
          >
            {c}
          </button>
        ))}
      </div>
      <div className="grid gap-2 sm:grid-cols-2">
        {[
          { name: 'Superpowers', desc: 'TDD, systematic debug, parallel dispatch, plan writing' },
          { name: 'Knowledge', desc: 'Q&A over your repo wiki + uploaded docs' },
          { name: 'STAROps', desc: 'Agentic ops from data queries' },
          { name: 'Design libraries', desc: 'Brand tokens + composable design skills' },
        ].map((p) => (
          <div key={p.name} className="rounded-md border border-border/50 bg-background/30 p-3">
            <div className="text-xs font-medium">{p.name}</div>
            <p className="mt-1 text-[10px] text-muted-foreground">{p.desc}</p>
            <Button
              size="sm"
              className="mt-2 h-6 bg-brand px-2 text-[10px] text-black hover:bg-brand"
              disabled
              title="Marketplace fetch is not wired — skill_store loads local SKILL.md files (see Skills)"
            >
              Install
            </Button>
          </div>
        ))}
      </div>
      <div className="text-[10px] text-muted-foreground">User · this machine · custom — no plugins installed.</div>
    </SectionShell>
  )
}

const EXPERTS = [
  { id: 'researcher', name: 'Researcher', desc: 'Read-only research, code location, environment inspect, reports. Scout child by default.' },
  { id: 'engineer', name: 'Full-stack engineer', desc: 'Implement and modify frontend and backend. Writers=1 unless you raise the cap.' },
  { id: 'qa', name: 'QA', desc: 'Tests, builds, validation evidence.' },
  { id: 'reviewer', name: 'Code reviewer', desc: 'Risks and improvement notes. No writes.' },
  { id: 'ui', name: 'UI operator', desc: 'Browser and UI end-to-end. Computer-use still E9 / CDP.' },
  { id: 'explore', name: 'Explore', desc: 'General-purpose browse of a tree. Depth ≤2.' },
  { id: 'debug', name: 'Debug engineer', desc: 'Reproduce failures, find root cause, suggest a fix. Writes only after a ticket.' },
  { id: 'general', name: 'General purpose', desc: 'Default subagent when no specialist matches.' },
]

export function ToolLogSection() {
  const sessionId = useAppStore((s) => s.activeSessionId)
  const [rows, setRows] = useState<import('@/lib/acp').AcpToolLogEntry[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const load = () => {
    if (!inTauri() || !sessionId) return
    setLoading(true)
    void import('@/lib/acp').then(({ acpToolLog }) => acpToolLog(sessionId))
      .then((next) => { setRows(next); setError(null) })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false))
  }
  useEffect(load, [sessionId])
  return (
    <SectionShell title="Tool log" desc="ACP activity observability — metrics only, never added to chat context.">
      <Honest>External agents keep their private tool history outside the transcript. This view shows the sanitized per-turn log written by the ACP bridge.</Honest>
      {!inTauri() ? <p className="text-xs text-muted-foreground">Tool logs are available in the desktop shell.</p> : !sessionId ? <p className="text-xs text-muted-foreground">Open a chat to inspect its tool log.</p> : (
        <>
          <div className="flex items-center justify-between">
            <span className="text-xs text-muted-foreground">{rows === null ? 'Not loaded' : `${rows.length} turn${rows.length === 1 ? '' : 's'}`}</span>
            <Button size="sm" variant="outline" className="h-7 text-[10px]" onClick={load} disabled={loading}><FileSearch className="mr-1 h-3 w-3" />{loading ? 'Loading…' : 'Refresh'}</Button>
          </div>
          {error && <p className="text-[10px] text-red-300">Could not load tool log: {error}</p>}
          {rows !== null && rows.length === 0 && !error && <p className="rounded-md border border-dashed border-border/60 px-3 py-5 text-center text-[10px] text-muted-foreground">No ACP turns recorded for this chat.</p>}
          {rows && rows.length > 0 && <ScrollArea className="max-h-[28rem] rounded-md border border-border/50"><div className="space-y-2 p-2">
            {rows.map((row, i) => <div key={`${row.tsMs}-${i}`} className="rounded border border-border/40 bg-background/30 p-2 text-[10px]">
              <div className="flex justify-between gap-2 font-mono text-muted-foreground"><span>{new Date(row.tsMs).toLocaleString()}</span><span>{row.stopReason}</span></div>
              <div className="mt-1 truncate text-foreground">{row.promptPrefix}</div>
              <div className="mt-1 text-muted-foreground">{row.toolCalls.length === 0 ? 'No tool calls' : `${row.toolCalls.length} tool call${row.toolCalls.length === 1 ? '' : 's'}`}</div>
              {row.toolCalls.length > 0 && <ul className="mt-1 space-y-0.5 text-muted-foreground">{row.toolCalls.map((tool) => <li key={tool.toolCallId} className="truncate">{tool.kind ?? 'tool'} · {tool.title} · {tool.status ?? 'unknown'}</li>)}</ul>}
            </div>)}
          </div></ScrollArea>}
        </>
      )}
    </SectionShell>
  )
}

export function SubagentsSection() {
  const notify = useAppStore((s) => s.notify)
  const [rows, setRows] = useState<import('@/lib/acp').SubagentRow[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [editing, setEditing] = useState<string | null>(null)
  const [draft, setDraft] = useState('')
  // P71.9d — which row's delegation profile is expanded, and its draft.
  const [policyFor, setPolicyFor] = useState<string | null>(null)
  const [policyDraft, setPolicyDraft] = useState<import('@/lib/acp').SubagentProfile | null>(null)
  const load = () => {
    if (!inTauri()) return
    void import('@/lib/acp').then(({ chiefSubagents }) => chiefSubagents())
      .then(setRows)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
  }
  useEffect(load, [])
  return (
    <SectionShell title="Subagents" desc="Installed agent CLIs the primary agent may delegate to (P71.5b: the retired \u2018Chief\u2019 name). Each row carries its when-to-use note.">
      <Honest>B3 delegation is bounded at depth ≤2 and concurrency ≤6. Only a Ready external agent can be hired. There is no built-in engine in v1. A registry entry with no program on this machine is not Ready and is not selectable.</Honest>
      <div className="flex items-center justify-between">
        <span className="text-xs font-medium">Installed delegation candidates {rows === null ? '…' : `(${rows.length})`}</span>
        <div className="flex gap-2">
          <Button size="sm" variant="outline" className="h-7 text-[10px]" onClick={() => useAppStore.getState().setSettingsSection('agents')}><Plus className="mr-1 h-3 w-3" />Discover agents</Button>
          <Button size="sm" variant="outline" className="h-7 text-[10px]" onClick={load}>Refresh</Button>
        </div>
      </div>
      {error && <p className="text-[10px] text-red-300">Could not load installed CLIs: {error}</p>}
      {!inTauri() && <p className="text-xs text-muted-foreground">Subagent discovery is available in the desktop shell.</p>}
      {rows?.length === 0 && !error && <p className="rounded-md border border-dashed border-border/60 px-3 py-6 text-center text-[10px] text-muted-foreground">No installed agent CLIs yet. Use Discover agents to install or connect one.</p>}
      <ul className="space-y-2">
        {(rows ?? []).map((r) => (
          <li key={r.agentId} className="rounded-md border border-border/50 bg-background/30 px-3 py-2">
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0">
                <div className="text-xs font-medium">{r.name} <span className="font-mono text-[9px] text-muted-foreground">{r.agentId}</span></div>
                {editing === r.agentId ? <Textarea value={draft} onChange={(e) => setDraft(e.target.value)} placeholder={r.defaultWhenToUse} className="mt-1 min-h-[56px] font-mono text-[11px]" /> : <p className="mt-1 text-[10px] text-muted-foreground">{r.whenToUse}{r.customized ? ' (customized)' : ''}</p>}
              </div>
              <Switch checked={r.enabled} onCheckedChange={(enabled) => void (async () => {
                try {
                  const { chiefSubagentSetEnabled, chiefSubagents } = await import('@/lib/acp')
                  await chiefSubagentSetEnabled(r.agentId, enabled)
                  setRows(await chiefSubagents())
                } catch (e) { notify(e instanceof Error ? e.message : 'Could not update delegation mix', 'error') }
              })()} aria-label={`Delegate to ${r.name}`} />
            </div>
            {/* P71.9d — per-agent delegation profile: role · model policy ·
                may-spawn · depth/concurrency/children caps · workspace ·
                budget. Saved through `chief_subagent_set_policy`; absent
                fields mean the spec default, never zero. */}
            <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
              <Button size="sm" variant="ghost" className="h-6 px-2 text-[10px]" onClick={() => {
                if (policyFor === r.agentId) { setPolicyFor(null); return }
                setPolicyFor(r.agentId)
                setPolicyDraft({
                  modelPolicy: 'agent-default',
                  role: '',
                  maySpawn: false,
                  maxChildren: 6,
                  maxDepth: 2,
                  maxConcurrency: 6,
                  workspace: 'shared',
                  budget: 0,
                  allowAsPrimary: true,
                  enableAsSubagent: true,
                  domains: [],
                  maxCentsPerTurn: 0,
                  maxTokensPerTurn: 0,
                })
              }}>
                {policyFor === r.agentId ? 'Hide profile' : 'Delegation profile'}
              </Button>
              {editing === r.agentId ? <>
                <Button size="sm" className="h-6 bg-brand px-2 text-[10px] text-black" onClick={() => void (async () => { try { const { chiefSubagentSetNote, chiefSubagents } = await import('@/lib/acp'); await chiefSubagentSetNote(r.agentId, draft); setRows(await chiefSubagents()); setEditing(null) } catch (e) { notify(e instanceof Error ? e.message : 'Save failed', 'error') } })()}>Save</Button>
                <Button size="sm" variant="outline" className="h-6 px-2 text-[10px]" onClick={() => setEditing(null)}>Cancel</Button>
              </> : <Button size="sm" variant="outline" className="h-6 px-2 text-[10px]" onClick={() => { setEditing(r.agentId); setDraft(r.customized ? r.whenToUse : '') }}>Edit when-to-use</Button>}
            </div>
            {policyFor === r.agentId && policyDraft && (
              <div className="mt-2 grid grid-cols-2 gap-2 rounded-md border border-border/50 bg-background/50 p-2 sm:grid-cols-3">
                <label className="text-[10px] text-muted-foreground">
                  Role
                  <input
                    value={policyDraft.role}
                    onChange={(e) => setPolicyDraft({ ...policyDraft, role: e.target.value })}
                    placeholder="e.g. researcher"
                    className="mt-0.5 h-6 w-full rounded border border-border bg-background px-1.5 font-mono text-[10px] text-foreground"
                  />
                </label>
                <label className="text-[10px] text-muted-foreground">
                  Model policy
                  <select
                    value={policyDraft.modelPolicy}
                    onChange={(e) => setPolicyDraft({ ...policyDraft, modelPolicy: e.target.value })}
                    className="mt-0.5 h-6 w-full rounded border border-border bg-background px-1 font-mono text-[10px] text-foreground"
                  >
                    <option value="agent-default">agent default</option>
                    <option value="inherit">inherit primary</option>
                  </select>
                </label>
                <label className="text-[10px] text-muted-foreground">
                  Workspace
                  <select
                    value={policyDraft.workspace}
                    onChange={(e) => setPolicyDraft({ ...policyDraft, workspace: e.target.value as 'shared' | 'isolated' })}
                    className="mt-0.5 h-6 w-full rounded border border-border bg-background px-1 font-mono text-[10px] text-foreground"
                  >
                    <option value="shared">shared</option>
                    <option value="isolated">isolated</option>
                  </select>
                </label>
                <label className="flex items-center gap-1.5 text-[10px] text-muted-foreground">
                  <Switch
                    checked={policyDraft.maySpawn}
                    onCheckedChange={(v) => setPolicyDraft({ ...policyDraft, maySpawn: v })}
                    aria-label="May spawn children"
                  />
                  May spawn children
                </label>
                <label className="flex items-center gap-1.5 text-[10px] text-muted-foreground">
                  <Switch
                    checked={policyDraft.allowAsPrimary}
                    onCheckedChange={(v) => setPolicyDraft({ ...policyDraft, allowAsPrimary: v })}
                    aria-label="Allow as primary"
                  />
                  Allow as primary
                </label>
                <label className="flex items-center gap-1.5 text-[10px] text-muted-foreground">
                  <Switch
                    checked={policyDraft.enableAsSubagent}
                    onCheckedChange={(v) => setPolicyDraft({ ...policyDraft, enableAsSubagent: v })}
                    aria-label="Enable as subagent"
                  />
                  Enable as subagent
                </label>
                <label className="col-span-2 text-[10px] text-muted-foreground sm:col-span-3">
                  Domain tags
                  <span className="mt-0.5 flex flex-wrap gap-1">
                    {(['coding', 'architecture', 'research', 'scraping', 'office'] as const).map((tag) => {
                      const domains = policyDraft.domains ?? []
                      const on = domains.includes(tag)
                      return (
                        <button
                          key={tag}
                          type="button"
                          aria-pressed={on}
                          className={`rounded border px-1.5 py-0.5 font-mono text-[10px] ${on ? 'border-brand text-foreground' : 'border-border text-muted-foreground'}`}
                          onClick={() => setPolicyDraft({
                            ...policyDraft,
                            domains: on ? domains.filter((item) => item !== tag) : [...domains, tag],
                          })}
                        >
                          {tag}
                        </button>
                      )
                    })}
                  </span>
                </label>
                <label className="text-[10px] text-muted-foreground">
                  Max cents / turn
                  <input
                    type="number"
                    min={0}
                    value={policyDraft.maxCentsPerTurn}
                    aria-label="Max cents per turn"
                    onChange={(e) => setPolicyDraft({ ...policyDraft, maxCentsPerTurn: Math.max(0, Number(e.target.value) || 0) })}
                    className="mt-0.5 h-6 w-full rounded border border-border bg-background px-1.5 font-mono text-[10px] text-foreground"
                  />
                </label>
                <label className="text-[10px] text-muted-foreground">
                  Max tokens / turn
                  <input
                    type="number"
                    min={0}
                    value={policyDraft.maxTokensPerTurn}
                    aria-label="Max tokens per turn"
                    onChange={(e) => setPolicyDraft({ ...policyDraft, maxTokensPerTurn: Math.max(0, Number(e.target.value) || 0) })}
                    className="mt-0.5 h-6 w-full rounded border border-border bg-background px-1.5 font-mono text-[10px] text-foreground"
                  />
                </label>
                {(['maxChildren', 'maxDepth', 'maxConcurrency', 'budget'] as const).map((k) => (
                  <label key={k} className="text-[10px] text-muted-foreground">
                    {k === 'maxChildren' ? 'Max children' : k === 'maxDepth' ? 'Max depth' : k === 'maxConcurrency' ? 'Max concurrency' : 'Budget (tokens)'}
                    <input
                      type="number"
                      min={k === 'budget' ? 0 : 1}
                      value={policyDraft[k]}
                      onChange={(e) => setPolicyDraft({ ...policyDraft, [k]: Math.max(0, Number(e.target.value) || 0) })}
                      className="mt-0.5 h-6 w-full rounded border border-border bg-background px-1.5 font-mono text-[10px] text-foreground"
                    />
                  </label>
                ))}
                <div className="col-span-2 flex items-center gap-2 sm:col-span-3">
                  <Button
                    size="sm"
                    className="h-6 bg-brand px-2 text-[10px] text-black"
                    onClick={() => void (async () => {
                      try {
                        const { chiefSubagentSetPolicy } = await import('@/lib/acp')
                        await chiefSubagentSetPolicy(r.agentId, policyDraft)
                        notify(`Delegation profile saved for ${r.name}`)
                        setPolicyFor(null)
                      } catch (e) {
                        notify(e instanceof Error ? e.message : 'Save failed', 'error')
                      }
                    })()}
                  >
                    Save profile
                  </Button>
                  <span className="text-[9px] text-muted-foreground">
                    Depth ≤2 / concurrency ≤6 are the B3 chain caps — the Work gateway's live gauge stays the admission authority.
                  </span>
                </div>
              </div>
            )}
          </li>
        ))}
      </ul>
    </SectionShell>
  )
}

export function ExpertsSection() {
  const notify = useAppStore((s) => s.notify)
  const [on, setOn] = usePref<Record<string, boolean>>(
    'experts.on',
    Object.fromEntries(EXPERTS.map((e) => [e.id, e.id !== 'ui'])),
  )
  // P53.6 — Settings → Subagents: installed CLIs only (same `agent_installed`
  // predicate primary-agent occupancy uses) + user-editable when-to-use per row.
  const [subs, setSubs] = useState<{ agentId: string; name: string; defaultWhenToUse: string; whenToUse: string; customized: boolean; enabled: boolean }[] | null>(null)
  const [subsErr, setSubsErr] = useState<string | null>(null)
  const [editing, setEditing] = useState<string | null>(null)
  const [draft, setDraft] = useState('')
  useEffect(() => {
    let alive = true
    void (async () => {
      if (!inTauri()) return
      try {
        const { chiefSubagents } = await import('@/lib/acp')
        const rows = await chiefSubagents()
        if (alive) {
          setSubs(rows)
          setSubsErr(null)
        }
      } catch (e) {
        if (alive) {
          setSubs([])
          setSubsErr(e instanceof Error ? e.message : String(e))
        }
      }
    })()
    return () => {
      alive = false
    }
  }, [])
  const saveNote = (agentId: string) =>
    void (async () => {
      try {
        const { chiefSubagentSetNote, chiefSubagents } = await import('@/lib/acp')
        await chiefSubagentSetNote(agentId, draft)
        const rows = await chiefSubagents()
        setSubs(rows)
        setEditing(null)
        notify(draft.trim() ? `When-to-use saved for ${agentId}` : `When-to-use reset to default for ${agentId}`)
      } catch (e) {
        notify(e instanceof Error ? e.message : 'Save failed', 'error')
      }
    })()
  return (
    <SectionShell title="Delegation roles" desc="v1 ships no in-app role profiles (the built-in engine is deferred, ADR-0005) — installed agent CLIs are configured under Subagents.">
      <Honest>B3 subagents are specified (depth ≤2, concurrency ≤6). Installed agent CLIs are listed under Subagents, where each row carries its shipped when-to-use (editable — the primary agent reads it at delegate time). v1 ships no in-app role profiles (ADR-0005).</Honest>
      {inTauri() && (
        <div className="space-y-1.5">
          <div className="text-xs font-medium">Installed subagent CLIs {subs === null ? '…' : `(${subs.length})`}</div>
          {subsErr && <p className="text-[10px] text-muted-foreground">Could not load installed CLIs: {subsErr}</p>}
          {subs !== null && subs.length === 0 && !subsErr && (
            <p className="text-[10px] text-muted-foreground">No agent CLIs installed on this machine yet — install one (Agents & Models) to delegate.</p>
          )}
          <ul className="space-y-1.5">
            {(subs ?? []).map((r) => (
              <li key={r.agentId} className="rounded-md border border-border/50 bg-background/30 px-3 py-2">
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <div className="text-xs font-medium">{r.name} <span className="font-mono text-[9px] text-muted-foreground">{r.agentId}</span></div>
                    {editing === r.agentId ? (
                      <Textarea
                        value={draft}
                        onChange={(e) => setDraft(e.target.value)}
                        placeholder={r.defaultWhenToUse}
                        className="mt-1 min-h-[56px] font-mono text-[11px]"
                      />
                    ) : (
                      <p className="text-[10px] text-muted-foreground">{r.whenToUse}{r.customized ? ' (customized)' : ''}</p>
                    )}
                  </div>
                </div>
                <div className="mt-1.5 flex items-center gap-2">
                  <Switch
                    checked={r.enabled}
                    onCheckedChange={(enabled) => void (async () => {
                      try {
                        const { chiefSubagentSetEnabled, chiefSubagents } = await import('@/lib/acp')
                        await chiefSubagentSetEnabled(r.agentId, enabled)
                        setSubs(await chiefSubagents())
                      } catch (e) {
                        notify(e instanceof Error ? e.message : 'Could not update delegation mix', 'error')
                      }
                    })()}
                    aria-label={`Delegate to ${r.name}`}
                  />
                  <span className="text-[10px] text-muted-foreground">{r.enabled ? 'In delegation mix' : 'Excluded from delegation mix'}</span>
                  {editing === r.agentId ? (
                    <>
                      <Button size="sm" className="h-6 bg-brand px-2 text-[10px] text-black hover:bg-brand" onClick={() => saveNote(r.agentId)}>Save</Button>
                      <Button size="sm" variant="outline" className="h-6 px-2 text-[10px]" onClick={() => setEditing(null)}>Cancel</Button>
                      {r.customized && (
                        <Button size="sm" variant="outline" className="h-6 px-2 text-[10px]" onClick={() => { setDraft(''); setEditing(r.agentId); }} title="Clear the override back to the shipped default">Reset</Button>
                      )}
                    </>
                  ) : (
                    <Button size="sm" variant="outline" className="h-6 px-2 text-[10px]" onClick={() => { setEditing(r.agentId); setDraft(r.customized ? r.whenToUse : '') }}>Edit when-to-use</Button>
                  )}
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}
      <ul className="space-y-1.5">
        {EXPERTS.map((e) => (
          <li key={e.id} className="flex items-start justify-between gap-3 rounded-md border border-border/50 bg-background/30 px-3 py-2">
            <div>
              <div className="text-xs font-medium">{e.name}</div>
              <p className="text-[10px] text-muted-foreground">{e.desc}</p>
            </div>
            <Switch
              checked={!!on[e.id]}
              onCheckedChange={(v) => setOn({ ...on, [e.id]: v })}
            />
          </li>
        ))}
      </ul>
      <div className="rounded-md border border-dashed border-border/60 px-3 py-6 text-center">
        <div className="text-xs text-muted-foreground">No custom expert yet</div>
        <div className="mt-2 flex justify-center gap-2">
          <Button
            size="sm"
            variant="outline"
            className="h-7 text-[10px]"
            onClick={() =>
              void (async () => {
                const st = useAppStore.getState()
                if (!inTauri()) {
                  notify('Bundle import needs the Tauri shell', 'error')
                  return
                }
                try {
                  const { open } = await import('@tauri-apps/plugin-dialog')
                  const picked = await open({
                    multiple: false,
                    title: 'Import an agent bundle (agent.toml)',
                    filters: [{ name: 'Agent bundle', extensions: ['toml'] }],
                  })
                  if (typeof picked !== 'string') return
                  const { fsReadFile } = await import('@/lib/fs')
                  const { agentRegistrySave } = await import('@/lib/agent-registry')
                  const f = await fsReadFile(picked)
                  if (f.binary) {
                    notify('That file is binary — pick an agent.toml', 'error')
                    return
                  }
                  const id = await agentRegistrySave(f.content)
                  notify(`Imported agent bundle as “${id}”`)
                } catch (e) {
                  notify(e instanceof Error ? e.message : 'Import failed', 'error')
                }
              })()
            }
          >
            Import
          </Button>
          <Button
            size="sm"
            className="h-7 bg-brand text-[10px] text-black hover:bg-brand"
            onClick={() => useAppStore.getState().setCenterScreen('agents')}
          >
            + New
          </Button>
        </div>
      </div>
    </SectionShell>
  )
}

// P50.2.x — removed: SkillsSection (always-empty `rows = []` plus a sync
// toast). The skills surface is the live SkillsPanel (signed store
// install/uninstall); the settings skills route renders it (see
// settings-panel SectionBody).

export function LaunchCliSection() {
  const notify = useAppStore((s) => s.notify)
  const agents = [
    { id: 'claude-code', name: 'Claude Code', desc: 'ACP coding agent', cmd: 'agentcowork acp launch claude-code' },
    { id: 'codex', name: 'Codex', desc: 'ACP coding agent', cmd: 'agentcowork acp launch codex' },
    { id: 'grok-build', name: 'Grok Build', desc: 'ACP coding agent', cmd: 'agentcowork acp launch grok-build' },
    { id: 'opencode', name: 'OpenCode', desc: 'ACP coding agent', cmd: 'agentcowork acp launch opencode' },
  ]
  return (
    <SectionShell title="Launch" desc="Copy a command and run it in your terminal. Same ACP agents as the picker — not a second product.">
      <Honest>F8 install + J17 launch exist as Tauri commands. These cards copy a suggested CLI string; the shell does not spawn from this list until you paste it.</Honest>
      <ul className="space-y-2">
        {agents.map((a) => (
          <li key={a.id} className="rounded-md border border-border/50 bg-background/30 px-3 py-2">
            <div className="text-xs font-medium">{a.name}</div>
            <p className="text-[10px] text-muted-foreground">{a.desc}</p>
            <div className="mt-1.5 flex items-center justify-between gap-2 rounded border border-border/40 bg-background/40 px-2 py-1 font-mono text-[10px]">
              <span className="truncate">{a.cmd}</span>
              <Button size="sm" variant="ghost" className="h-6 px-2 text-[10px]" onClick={() => {
                void navigator.clipboard?.writeText(a.cmd).catch(() => undefined)
                notify(`Copied ${a.cmd}`)
              }}>
                Copy
              </Button>
            </div>
          </li>
        ))}
      </ul>
    </SectionShell>
  )
}


export function HooksSection() {
  const [name, setName] = useState('')
  const [event, setEvent] = usePref('hooks.event', 'PreToolUse')
  const notify = useAppStore((s) => s.notify)
  return (
    <SectionShell
      title="Hooks"
      desc="Task-lifecycle commands. PreToolUse may deny only — it cannot skip a Guard-2 ticket."
      action={
        <Button
          size="sm"
          className="h-7 bg-brand text-black hover:bg-brand"
          disabled
          title="Hook execution lands with the I6 runner — the event/command form above is the staged surface"
        >
          <Plus className="h-3.5 w-3.5" /> Add hook
        </Button>
      }
    >
      <Honest>Empty hooks is the default. Changes apply to new chats.</Honest>
      <Row label="Event">
        <Select value={event} onValueChange={setEvent}>
          <SelectTrigger className="h-8 w-48 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            {['PreToolUse', 'PostToolUse', 'PostToolBatch', 'UserPromptSubmit', 'Stop', 'SessionStart'].map((e) => (
              <SelectItem key={e} value={e}>{e}</SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Row>
      <Row label="Command">
        <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="path/to/script" className="h-8 w-64 font-mono text-xs" />
      </Row>
      <div className="rounded-md border border-dashed border-border/60 px-3 py-6 text-center text-[11px] text-muted-foreground">
        No hooks configured
      </div>
    </SectionShell>
  )
}

export function WorktreeSection() {
  const [path, setPath] = usePref('worktree.path', '~/.agentcowork/worktrees')
  const [cap, setCap] = usePref('worktree.gb', 20)
  return (
    <SectionShell title="Worktree" desc="Disk for isolated agent checkouts">
      <Row label="Root">
        <Input value={path} onChange={(e) => setPath(e.target.value)} className="h-8 w-72 font-mono text-xs" />
      </Row>
      <Row label="Disk cap">
        <div className="flex w-56 items-center gap-3">
          <Slider value={[cap]} min={1} max={200} step={1} onValueChange={(v) => setCap(v[0])} />
          <span className="w-12 font-mono text-xs text-brand">{cap} GB</span>
        </div>
      </Row>
      <Honest>Isolated worktree-per-agent is P20 / P7.8. This cap is a preference, not an enforcer yet.</Honest>
    </SectionShell>
  )
}

export function RulesSection() {
  const [agents, setAgents] = usePref('rules.agents', '# AGENTS.md\n\n- Prefer small diffs.\n- Never commit secrets.\n')
  const [claude, setClaude] = usePref('rules.claude', '# CLAUDE.md\n\nProject conventions live here.\n')
  const [mem, setMem] = usePref('rules.memoryOn', true)
  return (
    <SectionShell title="Rules & memory" desc="AGENTS.md and CLAUDE.md are project instructions the lead agent reads on folder open">
      <Row label="Memory" desc="Off = this chat does not write long-term facts">
        <Switch checked={mem} onCheckedChange={setMem} />
      </Row>
      <div className="space-y-1">
        <div className="text-xs font-medium">AGENTS.md</div>
        <Textarea value={agents} onChange={(e) => setAgents(e.target.value)} className="min-h-[120px] font-mono text-[11px]" />
      </div>
      <div className="space-y-1">
        <div className="text-xs font-medium">CLAUDE.md</div>
        <Textarea value={claude} onChange={(e) => setClaude(e.target.value)} className="min-h-[80px] font-mono text-[11px]" />
      </div>
      <p className="text-[10px] text-muted-foreground">Edits stay in localStorage until the workspace file write is ticketed.</p>
    </SectionShell>
  )
}

// P55.9 — `CloudEnvSection` (a disabled docker-package dropdown) is deleted.
// The H33 story is a user-owned ExecutionNode attach, which is what Sync
// already does (`node_attach`): a control-plane address, a confirmed X25519
// handshake, a ledger reconciliation, and an honest `guardParked` flag. There
// is no founder image pull to configure, so there is no Cloud env surface.

/** P57.8 — Settings → Computer use. The whole surface: the derived H4 readiness
 * chip, the interaction default, the allow-list of exact paths, the searchable
 * installed-app inventory, and **Add by path**. Every control writes the policy
 * the live Guard-2 preflight enforces (`desktop_policy_*`), so nothing here is
 * chrome: a row added in this panel changes whether the driver may touch that
 * app. */
export function ComputerUseSection() {
  const [status, setStatus] = useState<{
    attached: boolean
    reason?: string | null
    readiness?: DesktopReadiness
    interactionDefault?: string
    capabilities?: {
      background_input?: boolean
    }
  } | null>(null)
  const [policy, setPolicy] = useState<DesktopPolicy | null>(null)
  const [apps, setApps] = useState<DesktopInstalledApp[] | null>(null)
  const [appTotal, setAppTotal] = useState(0)
  const [query, setQuery] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const notify = useAppStore((s) => s.notify)

  const loadStatus = async () => {
    const { invoke } = await import('@/lib/tauri')
    const s = await invoke<{
      attached: boolean
      reason?: string | null
      readiness?: DesktopReadiness
      interactionDefault?: string
      capabilities?: {
        background_input?: boolean
      }
    }>('desktop_status')
    const p = await invoke<{ policy: DesktopPolicy; readiness: DesktopReadiness }>('desktop_policy_get')
    setStatus(s)
    setPolicy(p.policy)
  }

  const attach = async () => {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      const { invoke } = await import('@/lib/tauri')
      const next = await invoke<{
        attached: boolean
        reason?: string | null
        readiness?: DesktopReadiness
        interactionDefault?: string
        capabilities?: { background_input?: boolean }
      }>('desktop_attach')
      setStatus(next)
      await loadStatus()
      notify(next.attached ? 'Desktop driver attached — capability surface measured' : (next.reason ?? 'Desktop driver did not attach'))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const loadApps = async (q: string) => {
    const { invoke } = await import('@/lib/tauri')
    const out = await invoke<{ apps: DesktopInstalledApp[]; total: number }>('desktop_apps', {
      query: q,
    })
    setApps(out.apps)
    setAppTotal(out.total)
  }

  useEffect(() => {
    let alive = true
    if (!inTauri()) return
    void (async () => {
      try {
        await loadStatus()
        if (alive) await loadApps('')
      } catch (e) {
        if (alive) setError(e instanceof Error ? e.message : String(e))
      }
    })()
    return () => {
      alive = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // The picker search runs in Rust against the session-cached inventory, so a
  // keystroke never re-scans the disk (and the match rule has one owner).
  useEffect(() => {
    if (!inTauri()) return
    let alive = true
    const t = setTimeout(() => {
      void (async () => {
        try {
          const { invoke } = await import('@/lib/tauri')
          const out = await invoke<{ apps: DesktopInstalledApp[]; total: number }>('desktop_apps', {
            query,
          })
          if (alive) {
            setApps(out.apps)
            setAppTotal(out.total)
          }
        } catch (e) {
          if (alive) setError(e instanceof Error ? e.message : String(e))
        }
      })()
    }, 120)
    return () => {
      alive = false
      clearTimeout(t)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query])

  const setInteraction = async (mode: 'background' | 'foreground') => {
    if (busy) return
    setBusy(true)
    try {
      const { invoke } = await import('@/lib/tauri')
      const out = await invoke<{ appliedLive: boolean; policy: DesktopPolicy }>(
        'desktop_policy_set_interaction',
        { mode },
      )
      setPolicy(out.policy)
      await loadStatus()
      notify(
        mode === 'background'
          ? `Background contract on — the driver will not raise windows${out.appliedLive ? ' (applied live)' : ' (applies to the next attach)'}`
          : `Foreground escalation enabled — the driver may raise windows${out.appliedLive ? ' (applied live)' : ' (applies to the next attach)'}`,
      )
    } catch (e) {
      notify(e instanceof Error ? e.message : String(e), 'error')
    } finally {
      setBusy(false)
    }
  }

  const allowPath = async (path: string) => {
    if (busy) return
    setBusy(true)
    try {
      const { invoke } = await import('@/lib/tauri')
      const out = await invoke<{ added: string; appliedLive: boolean; policy: DesktopPolicy }>(
        'desktop_policy_allow_path',
        { path },
      )
      setPolicy(out.policy)
      await loadApps(query)
      notify(`Allow-listed ${out.added}${out.appliedLive ? ' — live' : ' — applies to the next attach'}`)
    } catch (e) {
      notify(e instanceof Error ? e.message : String(e), 'error')
    } finally {
      setBusy(false)
    }
  }

  const removePath = async (path: string) => {
    if (busy) return
    setBusy(true)
    try {
      const { invoke } = await import('@/lib/tauri')
      const out = await invoke<{ removed: boolean; appliedLive: boolean; policy: DesktopPolicy }>(
        'desktop_policy_remove_path',
        { path },
      )
      setPolicy(out.policy)
      await loadApps(query)
      notify(out.removed ? `Removed ${path}` : 'That path was not on the allow-list')
    } catch (e) {
      notify(e instanceof Error ? e.message : String(e), 'error')
    } finally {
      setBusy(false)
    }
  }

  const pickPath = async () => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog')
      const opts = pickerOptions(
        typeof navigator === 'undefined' ? '' : `${navigator.platform ?? ''} ${navigator.userAgent ?? ''}`,
      )
      const picked = await open({
        multiple: false,
        directory: opts.directory,
        title: opts.title,
        filters: opts.filters,
      })
      if (typeof picked !== 'string') return
      await allowPath(picked)
    } catch (e) {
      notify(e instanceof Error ? e.message : 'File picker unavailable', 'error')
    }
  }

  const chip = readinessView(status?.readiness ?? null)
  const backgroundClick = backgroundInputView(
    status?.attached ?? false,
    status?.capabilities?.background_input,
  )
  const allowPaths = policy?.allowPaths ?? []
  const interaction = (policy?.interactionDefault ?? 'background') as 'background' | 'foreground'

  return (
    <SectionShell
      title="Computer use"
      desc="The native desktop driver (E9): see/read/act on real windows through the Guard-ticketed executor."
    >
      {!inTauri() ? (
        <p className="text-xs text-muted-foreground">The desktop driver status is available in the desktop shell.</p>
      ) : error ? (
        <p className="text-[10px] text-red-300">Could not read the desktop driver: {error}</p>
      ) : (
        <>
          {/* H4 — derived readiness, never a binary "works". */}
          <Row label="Readiness" desc="Derived from the backend's capability surface and the policy — H4">
            {chip ? (
              <Badge variant="outline" className={cn('text-[10px]', chip.tone)}>
                {chip.glyph} {chip.label}
              </Badge>
            ) : (
              <span className="text-xs text-muted-foreground">Checking…</span>
            )}
          </Row>
          {chip && <p className="font-mono text-[10px] text-muted-foreground">{chip.detail}</p>}
          <Row
            label="Background coordinate click"
            desc="Whether this host can click at a coordinate without moving your real pointer"
          >
            <Badge variant="outline" className={cn('text-[10px]', backgroundClick.tone)}>
              {backgroundClick.glyph} {backgroundClick.label}
            </Badge>
          </Row>
          <p className="font-mono text-[10px] text-muted-foreground">{backgroundClick.detail}</p>
          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" variant="outline" className="h-7 text-[10px]" disabled={busy} onClick={() => void attach()}>
              {status?.attached ? 'Re-measure driver' : 'Attach and measure'}
            </Button>
            {status?.attached && <span className="text-[10px] text-muted-foreground">Live native capability report</span>}
          </div>
          {status && !status.attached && status.reason && (
            <p className="font-mono text-[10px] text-warning/80">{status.reason}</p>
          )}

          {/* P57.3/.4 — the interaction default the engine enforces. */}
          <div className="space-y-1.5">
            <div className="text-xs font-medium">Default interaction</div>
            <div className="flex items-center gap-2">
              {(['background', 'foreground'] as const).map((m) => (
                <Button
                  key={m}
                  size="sm"
                  variant={interaction === m ? 'default' : 'outline'}
                  className="h-7 text-[10px]"
                  disabled={busy || !policy}
                  onClick={() => void setInteraction(m)}
                  aria-pressed={interaction === m}
                >
                  {m === 'background' ? 'Background' : 'Foreground'}
                </Button>
              ))}
            </div>
            <p className="text-[10px] text-muted-foreground">{interactionCopy(interaction)}</p>
          </div>

          {/* P57.2 — the allow-list of exact paths. */}
          <div className="space-y-1.5">
            <div className="flex items-center justify-between">
              <span className="text-xs font-medium">
                Allow-listed apps {policy ? `(${allowPaths.length})` : '…'}
              </span>
              <Button size="sm" variant="outline" className="h-7 text-[10px]" disabled={busy} onClick={() => void pickPath()}>
                <Plus className="mr-1 h-3 w-3" /> Add by path
              </Button>
            </div>
            {allowPaths.length === 0 ? (
              <p className="rounded-md border border-dashed border-border/60 px-3 py-2 text-[10px] text-muted-foreground">
                Nothing allow-listed. Until a path is here, an unlisted app needs a per-session confirmation (and risky classes are always Guard-2).
              </p>
            ) : (
              <ul className="divide-y divide-border/40 rounded-md border border-border/50">
                {allowPaths.map((p) => (
                  <li key={p} className="flex items-center justify-between gap-3 px-3 py-2">
                    <div className="min-w-0">
                      <div className="truncate text-[11px]">{p.split('/').pop() || p}</div>
                      <div className="truncate font-mono text-[10px] text-muted-foreground" title={p}>
                        {pathTail(p)}
                      </div>
                    </div>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-6 px-2 text-[10px]"
                      disabled={busy}
                      onClick={() => void removePath(p)}
                      aria-label={`Remove ${p} from the allow-list`}
                    >
                      <Trash2 className="h-3 w-3" />
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </div>

          {/* P57.8 — the searchable installed-app inventory. */}
          <div className="space-y-1.5">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-medium">
                Installed apps {apps === null ? '…' : `(${apps.length}/${appTotal})`}
              </span>
              <Input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search apps…"
                className="h-7 w-48 text-xs"
                aria-label="Search installed apps"
              />
            </div>
            {apps === null ? (
              <p className="text-[10px] text-muted-foreground">Reading the installed-app inventory…</p>
            ) : apps.length === 0 ? (
              <p className="text-[10px] text-muted-foreground">
                No installed app matches “{query}”. Use <strong>Add by path</strong> if it lives somewhere this platform does not index.
              </p>
            ) : (
              <ScrollArea className="max-h-64 rounded-md border border-border/50">
                <ul className="divide-y divide-border/40">
                  {apps.map((a) => {
                    const block = addBlockReason(a)
                    return (
                      <li key={a.path} className="flex items-center justify-between gap-3 px-3 py-2">
                        <div className="min-w-0">
                          <div className="truncate text-[11px]">{a.name}</div>
                          <div className="truncate font-mono text-[10px] text-muted-foreground" title={a.path}>
                            {pathTail(a.path)} · {sourceLabel(a.source)}
                          </div>
                          {a.hardDenied && (
                            <div className="truncate text-[10px] text-warning/80">{a.hardDenied}</div>
                          )}
                        </div>
                        {block ? (
                          <Badge variant="outline" className="shrink-0 text-[10px] text-muted-foreground">
                            {a.hardDenied ? 'never automatable' : 'listed'}
                          </Badge>
                        ) : (
                          <Button
                            size="sm"
                            variant="outline"
                            className="h-6 shrink-0 px-2 text-[10px]"
                            disabled={busy}
                            onClick={() => void allowPath(a.path)}
                          >
                            <Plus className="mr-1 h-3 w-3" /> Add
                          </Button>
                        )}
                      </li>
                    )
                  })}
                </ul>
              </ScrollArea>
            )}
          </div>
        </>
      )}
      <Honest>
        These writes go to the Guard-2 policy the driver enforces (<span className="font-mono">&lt;data_dir&gt;/desktop.json</span>): a listed path is launchable without toggling
        computer use per session, and the Background default refuses to raise a window at all. Risky classes (delete · money · install · CAPTCHA · transmit) still reach the
        human gate, and the hard-deny list (terminal, password managers, UAC, AgentCowork itself) can never be allow-listed — hidden here and refused by the backend.
      </Honest>
    </SectionShell>
  )
}

/** The shape `search_config` returns (camelCase over the persisted file). */
interface SearchConfigRow {
  usePublic: boolean
  endpoints: string[]
  localEndpoints: string[]
  publicEndpoints: string[]
}

interface SearchInstanceRow {
  url: string
  version: string
  median_seconds?: number | null
  search_success_percentage?: number | null
}

interface SearchFeedRow {
  source: 'live' | 'fresh_cache' | 'stale_cache'
  count: number
  instances: SearchInstanceRow[]
}

/**
 * P55.8 — Settings → Search. The G8 cascade is local-first: your own SearXNG,
 * then the DDG fallback. Public instances come from the `searx.space` feed and
 * are only used when switched on here — routing a query through a stranger's
 * server is the user's decision, not a default. The endpoint list shown is the
 * one the live cascade reads (`<data_dir>/search.json`), so the two cannot
 * disagree; a feed that cannot be fetched is an error, and a cached list says
 * it is cached.
 */
export function SearchEnginesSection() {
  const [config, setConfig] = useState<SearchConfigRow | null>(null)
  const [feed, setFeed] = useState<SearchFeedRow | null>(null)
  const [feedError, setFeedError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const notify = useAppStore((s) => s.notify)

  const load = async () => {
    if (!inTauri()) return
    const { invoke } = await import('@/lib/tauri')
    setConfig(await invoke<SearchConfigRow>('search_config'))
  }

  useEffect(() => {
    let alive = true
    void (async () => {
      try {
        if (!inTauri()) return
        const { invoke } = await import('@/lib/tauri')
        const cfg = await invoke<SearchConfigRow>('search_config')
        if (alive) setConfig(cfg)
      } catch (e) {
        if (alive) setFeedError(e instanceof Error ? e.message : String(e))
      }
    })()
    return () => {
      alive = false
    }
  }, [])

  const discover = async (refresh: boolean) => {
    if (busy) return
    setBusy(true)
    setFeedError(null)
    try {
      const { invoke } = await import('@/lib/tauri')
      const out = await invoke<SearchFeedRow>('search_instances', { refresh })
      setFeed(out)
    } catch (e) {
      setFeed(null)
      setFeedError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const apply = async (usePublic: boolean) => {
    if (busy) return
    setBusy(true)
    setFeedError(null)
    try {
      const { invoke } = await import('@/lib/tauri')
      const out = await invoke<{
        usePublic: boolean
        endpoints: string[]
        discovered: number
        appliedLive: boolean
      }>('search_instances_apply', { usePublic })
      await load()
      notify(
        usePublic
          ? `Public instances on — ${out.discovered} in the cascade; ${out.appliedLive ? 'applied live' : 'applies on next boot'}`
          : 'Public instances off — local SearXNG + DDG fallback only',
      )
    } catch (e) {
      setFeedError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const sourceLabel = (s: SearchFeedRow['source']) =>
    s === 'live' ? 'fetched just now' : s === 'fresh_cache' ? 'cached (fresh)' : 'cached — last known good'

  return (
    <SectionShell
      title="Search"
      desc="Which endpoints the G8 search cascade uses. Local-first by default — no public instance is queried until you turn it on."
    >
      <Honest>
        The cascade tries these in order and stops at the first that answers. Public instances come from the{' '}
        <span className="font-mono">searx.space</span> feed (health-probed upstream); instances that are offline, Tor-only, or
        failing are dropped rather than listed as healthy. A fetch that fails is an error, never an empty “success”.
      </Honest>
      <Row label="Endpoints in use" desc={`${config?.endpoints.length ?? 0} endpoint(s), local first`}>
        <div className="flex flex-col items-end gap-0.5">
          {(config?.endpoints ?? []).slice(0, 6).map((e) => (
            <span key={e} className="font-mono text-[10px] text-muted-foreground">
              {e}
            </span>
          ))}
          {(config?.endpoints.length ?? 0) > 6 && (
            <span className="font-mono text-[10px] text-muted-foreground">
              +{(config?.endpoints.length ?? 0) - 6} more
            </span>
          )}
          {config && config.endpoints.length === 0 && (
            <span className="font-mono text-[10px] text-warning/80">none configured</span>
          )}
        </div>
      </Row>
      <Row label="Include public instances" desc="Off = local SearXNG, then the DuckDuckGo fallback">
        <Switch
          checked={!!config?.usePublic}
          disabled={busy || !config}
          onCheckedChange={(v) => void apply(v)}
          aria-label="Include public SearXNG instances"
        />
      </Row>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" variant="outline" className="h-7 text-[10px]" disabled={busy} onClick={() => void discover(true)}>
          {busy ? 'Working…' : 'Check the instance feed'}
        </Button>
        {feed && (
          <span className="font-mono text-[10px] text-muted-foreground">
            {feed.count} eligible · {sourceLabel(feed.source)}
          </span>
        )}
      </div>
      {feed && feed.instances.length > 0 && (
        <ul className="max-h-40 space-y-1 overflow-auto rounded-md border border-border/50 p-2">
          {feed.instances.slice(0, 20).map((i) => (
            <li key={i.url} className="flex items-center justify-between gap-2 font-mono text-[10px] text-muted-foreground">
              <span className="truncate">{i.url}</span>
              <span className="shrink-0">
                {i.median_seconds != null ? `${i.median_seconds.toFixed(2)}s` : 'no timing'}
              </span>
            </li>
          ))}
        </ul>
      )}
      {feedError && (
        <div className="rounded-md border border-warning/30 bg-warning/5 px-3 py-2 font-mono text-[10px] text-warning/90">
          instance feed: {feedError}
        </div>
      )}
    </SectionShell>
  )
}

// P50.2.x — removed: ImportSection (six buttons whose parsers were never
// built — every click ended in an error toast). Migration import returns
// with the parser backend; until then the route falls back to General
// (see settings-panel SectionBody).

// P50.2.x — removed: UsageSection (static Requests/Tokens/USD zeros that
// duplicated AnalyticsPanel + UxMetricsSection). The usage route renders the
// live UxMetricsSection (see settings-panel SectionBody).

export function ResourcesSection() {
  const notify = useAppStore((s) => s.notify)
  const [disk, setDisk] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    void (async () => {
      try {
        const { doctorReport } = await import('@/lib/doctor')
        const report = await doctorReport()
        const diskCheck = report.checks.find((c) => c.name.toLowerCase().includes('disk'))
        if (alive) setDisk(diskCheck?.detail ?? null)
      } catch {
        if (alive) setDisk(null)
      }
    })()
    return () => {
      alive = false
    }
  }, [])
  return (
    <SectionShell title="Resources" desc="Disk from the Doctor report. Process CPU/memory are not sampled in this build.">
      <div className="grid gap-2 sm:grid-cols-3">
        {([
          { label: 'CPU', val: '—', Icon: Cpu, title: 'Process CPU is not sampled in this build' },
          { label: 'Memory', val: '—', Icon: Radio, title: 'Process memory is not sampled in this build' },
          { label: 'Disk', val: disk ?? '—', Icon: HardDrive, title: 'From the Doctor disk check' },
        ]).map(({ label, val, Icon, title }) => (
            <div key={label} className="rounded-md border border-border/50 bg-background/30 px-3 py-3" title={title}>
              <div className="flex items-center gap-1.5 text-[10px] text-muted-foreground">
                <Icon className="h-3 w-3" /> {label}
              </div>
              <div className="font-mono text-sm">{val}</div>
            </div>
        ))}
      </div>
      <Button
        size="sm"
        variant="outline"
        className="h-7 text-[10px]"
        onClick={() => useAppStore.getState().setSettingsSection('doctor')}
      >
        Open Doctor
      </Button>
    </SectionShell>
  )
}

export function BetaSection() {
  const [beta, setBeta] = usePref('beta.on', false)
  const [solo, setSolo] = usePref('beta.solo', false)
  return (
    <SectionShell title="Beta" desc="Preview flags. Off by default.">
      <Row label="Beta channel">
        <Switch checked={beta} onCheckedChange={setBeta} />
      </Row>
      <Row label="SOLO / unattended long jobs" desc="Preference only — tray keep-alive is not enforced yet">
        <Switch checked={solo} onCheckedChange={setSolo} />
      </Row>
    </SectionShell>
  )
}

export function GeneralExtras() {
  const [proxy, setProxy] = usePref('general.proxy', '')
  const [tray, setTray] = usePref('general.tray', true)
  const [archive, setArchive] = usePref('general.archiveDays', 30)
  const [md, setMd] = usePref('general.mdOpen', 'editor')
  const [keymap, setKeymap] = usePref('general.keymap', 'default')
  return (
    <>
      <Row label="HTTPS / SOCKS proxy">
        <Input value={proxy} onChange={(e) => setProxy(e.target.value)} placeholder="optional" className="h-8 w-56 font-mono text-xs" />
      </Row>
      <Row label="Keep running in tray">
        <Switch checked={tray} onCheckedChange={setTray} />
      </Row>
      <Row label="Archive idle chats after">
        <div className="flex w-56 items-center gap-3">
          <Slider value={[archive]} min={7} max={365} step={1} onValueChange={(v) => setArchive(v[0])} />
          <span className="w-12 font-mono text-xs text-brand">{archive}d</span>
        </div>
      </Row>
      <Row label="Open markdown with">
        <Select value={md} onValueChange={setMd}>
          <SelectTrigger className="h-8 w-40 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="editor">Code editor</SelectItem>
            <SelectItem value="preview">Preview</SelectItem>
          </SelectContent>
        </Select>
      </Row>
      <Row label="Keymap">
        <Select value={keymap} onValueChange={setKeymap}>
          <SelectTrigger className="h-8 w-44 text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="default">AgentCowork</SelectItem>
            <SelectItem value="vscode">VS Code</SelectItem>
            <SelectItem value="cursor">Cursor-like</SelectItem>
          </SelectContent>
        </Select>
      </Row>
    </>
  )
}


