'use client'

import { useEffect, useState } from 'react'
import { Download, Trash2, Video } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { useAppStore } from '@/lib/store'
import {
  completionRate,
  errorRate,
  getUxMetrics,
  resetUxMetrics,
  timeToValueMs,
  type UxMetrics,
} from '@/lib/ux-metrics'
import {
  clearRecordedEvents,
  exportRecordedEvents,
  getRecordedEvents,
  sessionRecordingEnabled,
  setSessionRecording,
} from '@/lib/session-recording'
import { Row, SectionShell } from './settings-shared'

// P11.6.4 + P11.6.5 — key UX metrics (task completion, time-to-value, error
// rate) and the opt-in session recorder (clicks/navigation only, never
// content). Both are strictly local — the metrics are counters in
// localStorage, the recording is a bounded ring buffer, and export is the
// user's own action.
export function UxMetricsSection() {
  const [metrics, setMetrics] = useState<UxMetrics>(() => getUxMetrics())
  const [recording, setRecording] = useState<boolean>(() => sessionRecordingEnabled())
  const [eventCount, setEventCount] = useState<number>(() => getRecordedEvents().length)
  const notify = useAppStore((s) => s.notify)

  const refresh = () => {
    setMetrics(getUxMetrics())
    setEventCount(getRecordedEvents().length)
  }

  useEffect(() => {
    // Live-update while the section is open.
    const t = setInterval(refresh, 2000)
    return () => clearInterval(t)
  }, [])

  const ttv = timeToValueMs(metrics)
  const cr = completionRate(metrics)
  const er = errorRate(metrics)

  const fmtMs = (ms: number): string => {
    if (ms < 1000) return `${ms}ms`
    if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
    return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`
  }
  const fmtPct = (v: number | null): string => (v === null ? '—' : `${Math.round(v * 100)}%`)

  const exportRecording = () => {
    try {
      const blob = new Blob([exportRecordedEvents()], { type: 'application/json' })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = `agentcowork-activity-recording-${Date.now()}.json`
      a.click()
      URL.revokeObjectURL(url)
      notify('Activity recording exported (JSON)')
    } catch {
      notify('Export failed', 'error')
    }
  }

  return (
    <SectionShell
      title="UX metrics & recording"
      desc="Local-only product metrics and the opt-in activity recorder. Counters and click/nav events stay on this device — nothing is sent anywhere."
    >
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
        {[
          ['Chats', String(metrics.sessionsCreated)],
          ['Turns completed', String(metrics.turnsCompleted)],
          ['Turns failed', String(metrics.turnsFailed)],
          ['Completion rate', fmtPct(cr)],
          ['Error rate', fmtPct(er)],
          ['Time to first tool result', ttv === null ? '—' : fmtMs(ttv)],
          ['Approvals granted', String(metrics.approvalsGranted)],
          ['Approvals rejected', String(metrics.approvalsRejected)],
        ].map(([k, v]) => (
          <div key={k} className="rounded-md border border-border/50 bg-background/30 px-3 py-2.5">
            <div className="text-[10px] text-muted-foreground">{k}</div>
            <div className="font-mono text-xs text-brand">{v}</div>
          </div>
        ))}
      </div>
      <Button size="sm" variant="outline" className="h-7 gap-1 text-[10px]" onClick={() => { resetUxMetrics(); refresh(); notify('UX metrics reset') }}>
        <Trash2 className="h-3 w-3" />
        Reset metrics
      </Button>

      <Row label="Opt-in activity recording" desc="Records clicks and view navigation only — never message content, tool payloads, or provider data">
        <Switch
          checked={recording}
          onCheckedChange={(v) => {
            setRecording(v)
            setSessionRecording(v)
            refresh()
            notify(v ? 'Activity recording on (clicks/navigation only)' : 'Activity recording off')
          }}
        />
      </Row>
      <p className="text-[10px] text-muted-foreground">
        <Video className="mr-1 inline h-3 w-3" />
        {eventCount} event(s) recorded (ring-buffered at 2000, newest kept).
      </p>
      <div className="flex gap-2">
        <Button size="sm" variant="outline" className="h-7 gap-1 text-[10px]" onClick={exportRecording} disabled={eventCount === 0}>
          <Download className="h-3 w-3" />
          Export recording (JSON)
        </Button>
        <Button size="sm" variant="outline" className="h-7 gap-1 text-[10px]" onClick={() => { clearRecordedEvents(); refresh(); notify('Recording cleared') }} disabled={eventCount === 0}>
          <Trash2 className="h-3 w-3" />
          Clear
        </Button>
      </div>
    </SectionShell>
  )
}
