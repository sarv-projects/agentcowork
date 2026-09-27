'use client'

// Run identity, in the words the user chose: which Chat this is, what state it
// is in, the bound agent **with its real readiness**, the working folder with a
// working copy-path control, and the model **as the agent reported it**.
//
// Every one of these is a projection. Nothing here is asserted by the panel:
// an unbound agent says so, a curated catalog row says it has not been probed,
// and a model the agent never advertised reads "not reported" rather than
// borrowing AgentCowork's own catalog (ADR-0005 §2 / ARCH/16 §3).

import { useState } from 'react'
import {
  Bot,
  Check,
  CircleDot,
  Copy,
  Cpu,
  FolderOpen,
  MessageSquare,
} from 'lucide-react'
import { useAppStore } from '@/lib/store'
import { AGENT_MAP } from '@/lib/agents'
import { isAgentReady, readinessLabel } from '@/lib/acp'
import { agentCardFromEvents } from '@/lib/agent-card'
import { chatStateLabel, chatStateTone, shortenPath } from '@/components/views/run-projection'
import { RunFact } from '@/components/views/run-section'
import { cn } from '@/lib/utils'

export function RunHeader() {
  const activeSessionId = useAppStore((s) => s.activeSessionId)
  const chat = useAppStore((s) => s.sessions.find((x) => x.id === s.activeSessionId))
  const taskFolder = useAppStore((s) => s.taskFolder)
  const selectedAgentId = useAppStore((s) => s.selectedAgentId)
  const liveAgents = useAppStore((s) => s.liveAgents)
  const workEvents = useAppStore((s) => s.workEvents)
  const notify = useAppStore((s) => s.notify)
  const addView = useAppStore((s) => s.addView)
  // The agent's own model control, exactly as the ACP handshake reported it.
  const modelOption = useAppStore((s) => {
    const opts = s.acpConfigOptions[s.selectedAgentId]
    if (!opts || opts.length === 0) return undefined
    return (
      opts.find((o) => o.category === 'model') ??
      opts.find((o) => o.id.toLowerCase().includes('model'))
    )
  })

  const [copied, setCopied] = useState(false)
  const card = agentCardFromEvents(workEvents)
  const workingDir = chat?.folder ?? taskFolder ?? null
  const boundLive = liveAgents.find((a) => a.id === selectedAgentId)
  const curated = AGENT_MAP[selectedAgentId ?? '']
  const modelReported =
    modelOption && modelOption.currentValue !== undefined && String(modelOption.currentValue) !== ''
      ? String(modelOption.currentValue)
      : null

  const copyPath = async () => {
    if (!workingDir) return
    try {
      await navigator.clipboard.writeText(workingDir)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1600)
    } catch {
      notify('Could not copy the folder path', 'error')
    }
  }

  return (
    <div data-testid="run-header" className="border-b border-border px-4 py-2.5">
      <div className="flex items-start gap-2">
        <MessageSquare aria-hidden className="mt-0.5 h-3.5 w-3.5 shrink-0 text-brand" />
        <div className="min-w-0 flex-1">
          <h2
            className="truncate text-[13px] font-semibold leading-tight text-foreground"
            title={chat?.title}
          >
            {chat?.title?.trim() || 'Untitled chat'}
          </h2>
          <p className="mt-0.5 flex items-center gap-1.5 text-[10px] text-muted-foreground">
            <span
              className={cn(
                'inline-flex items-center gap-1 rounded-full border px-1.5 py-px font-mono text-[9px] uppercase tracking-wide',
                chatStateTone(chat?.status, card.awaitingInput),
              )}
            >
              {chat?.status === 'running' && !card.awaitingInput ? (
                <CircleDot aria-hidden className="h-2.5 w-2.5 live-dot" />
              ) : null}
              {chatStateLabel(chat?.status, card.awaitingInput)}
            </span>
            {activeSessionId ? (
              <span className="truncate font-mono opacity-60" title={activeSessionId}>
                {shortId(activeSessionId)}
              </span>
            ) : null}
          </p>
        </div>
      </div>

      <div className="mt-2 space-y-0.5">
        <RunFact
          label="Agent"
          value={
            <span className="flex items-center gap-1.5">
              <Bot aria-hidden className="h-3 w-3 shrink-0 text-muted-foreground" />
              <span className="truncate">
                {boundLive?.name ?? curated?.name ?? (selectedAgentId || 'none bound')}
              </span>
            </span>
          }
        />
        <RunFact
          label="Readiness"
          value={
            boundLive?.readiness ? (
              <span className={isAgentReady(boundLive.readiness) ? 'text-success' : 'text-warning'}>
                {readinessLabel(boundLive.readiness)}
              </span>
            ) : selectedAgentId ? (
              <span className="text-muted-foreground">
                not probed on this machine — install state unknown
              </span>
            ) : (
              <span className="text-muted-foreground">no agent bound to this chat</span>
            )
          }
          title="Readiness comes from the live agent directory, never from a catalog row."
        />
        <RunFact
          label="Model"
          value={
            modelReported ? (
              <span className="flex items-center gap-1.5">
                <Cpu aria-hidden className="h-3 w-3 shrink-0 text-muted-foreground" />
                <span className="truncate" title={modelReported}>
                  {modelReported}
                </span>
              </span>
            ) : (
              <span className="text-muted-foreground">
                {selectedAgentId ? 'not reported by the agent' : 'no agent bound — no model reported'}
              </span>
            )
          }
          title="The value the bound agent advertised over ACP. AgentCowork does not choose it."
        />
        <RunFact
          label="Folder"
          value={
            <span className="flex items-center gap-1.5">
              {workingDir ? (
                <>
                  <FolderOpen aria-hidden className="h-3 w-3 shrink-0 text-muted-foreground" />
                  <button
                    type="button"
                    onClick={() => addView('folder')}
                    title="Open the folder view"
                    className="min-w-0 truncate text-left underline decoration-dotted underline-offset-2 transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
                  >
                    {shortenPath(workingDir, 2)}
                  </button>
                  <button
                    type="button"
                    onClick={() => void copyPath()}
                    aria-label="Copy the working folder path"
                    title="Copy the full path"
                    className="grid h-5 w-5 shrink-0 place-items-center rounded border border-border text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
                  >
                    {copied ? (
                      <Check aria-hidden className="h-2.5 w-2.5 text-success" />
                    ) : (
                      <Copy aria-hidden className="h-2.5 w-2.5" />
                    )}
                  </button>
                  <span aria-live="polite" className="sr-only">
                    {copied ? 'Folder path copied' : ''}
                  </span>
                </>
              ) : (
                <span className="text-muted-foreground">
                  no folder attached — the copy control stays inert
                </span>
              )}
            </span>
          }
          title={workingDir ?? undefined}
        />
      </div>
    </div>
  )
}

function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 12)}…` : id
}
