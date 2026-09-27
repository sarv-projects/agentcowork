import { afterAll, describe, expect, test } from 'bun:test'
import { handleChatEvent } from './bridge'
import { resetStreamingTestState, useAppStore } from './store'

function freshSession(): string {
  resetStreamingTestState()
  useAppStore.getState().markSessionsHydrated()
  useAppStore.getState().newSession()
  return useAppStore.getState().activeSessionId
}

function lastMessage(sessionId: string) {
  const session = useAppStore.getState().sessions.find((s) => s.id === sessionId)
  return session?.messages[session.messages.length - 1]
}

// P58.9 — the Settings → Notifications switches must actually gate the live
// stream. `readPref` reads `window.localStorage`, so the tests stub `window`
// with a single-key store and restore it afterwards.
const originalWindow = (globalThis as { window?: unknown }).window

function stubNotifyPref(key: string, value: boolean) {
  ;(globalThis as { window?: unknown }).window = {
    localStorage: {
      getItem: (k: string) =>
        k === `agentcowork.settings.${key}` ? JSON.stringify(value) : null,
    },
  }
}

function restoreWindow() {
  if (originalWindow === undefined) {
    delete (globalThis as { window?: unknown }).window
  } else {
    ;(globalThis as { window?: unknown }).window = originalWindow
  }
}

afterAll(() => {
  resetStreamingTestState()
  restoreWindow()
})

describe('chat event routing', () => {
  test('routes events to their session even after the active tab changes', () => {
    const sessionA = freshSession()
    useAppStore.getState().newSession()
    const sessionB = useAppStore.getState().activeSessionId

    handleChatEvent({ type: 'ttft', sessionId: sessionA, streamId: 'stream-a' })
    useAppStore.getState().setActiveSession(sessionB)
    handleChatEvent({ type: 'batch', sessionId: sessionA, streamId: 'stream-a', text: 'answer A' })
    handleChatEvent({ type: 'done', sessionId: sessionA, streamId: 'stream-a', fullText: 'answer A', totalTokens: 1 })

    expect(lastMessage(sessionA)?.content).toBe('answer A')
    expect(lastMessage(sessionB)?.content ?? '').not.toContain('answer A')
    expect(useAppStore.getState().sessions.find((s) => s.id === sessionA)?.status).toBe('completed')
  })

  test('cancellation and budget events settle the exact stream and release live state', () => {
    const sid = freshSession()
    const st = useAppStore.getState()
    st.setLiveStreamId(sid, 'stream-cancel')
    handleChatEvent({ type: 'ttft', sessionId: sid, streamId: 'stream-cancel' })
    handleChatEvent({ type: 'batch', sessionId: sid, streamId: 'stream-cancel', text: 'partial' })
    handleChatEvent({ type: 'cancelled', sessionId: sid, streamId: 'stream-cancel' })

    expect(useAppStore.getState().sessions.find((s) => s.id === sid)?.status).toBe('cancelled')
    expect(useAppStore.getState().liveStreamId[sid]).toBeUndefined()
    expect(lastMessage(sid)?.content).toBe('partial')

    handleChatEvent({ type: 'ttft', sessionId: sid, streamId: 'stream-budget' })
    handleChatEvent({ type: 'batch', sessionId: sid, streamId: 'stream-budget', text: 'partial budget' })
    handleChatEvent({
      type: 'budgetExceeded',
      sessionId: sid,
      streamId: 'stream-budget',
      limit: 2,
      spent: 2.1,
    })

    expect(useAppStore.getState().sessions.find((s) => s.id === sid)?.status).toBe('budget_exceeded')
    expect(lastMessage(sid)?.content).toBe('partial budget')
  })

  test('rejects missing identity and fences late events from a retired stream', () => {
    const sid = freshSession()
    const before = useAppStore.getState().liveNotifications.length
    handleChatEvent({ type: 'batch', streamId: 'missing-session', text: 'must not route' })
    expect(useAppStore.getState().liveNotifications.length).toBe(before + 1)

    handleChatEvent({ type: 'ttft', sessionId: sid, streamId: 'stream-old' })
    handleChatEvent({ type: 'done', sessionId: sid, streamId: 'stream-old', fullText: 'old', totalTokens: 1 })
    handleChatEvent({ type: 'ttft', sessionId: sid, streamId: 'stream-new' })
    handleChatEvent({ type: 'batch', sessionId: sid, streamId: 'stream-old', text: 'stale' })

    expect(lastMessage(sid)?.content).toBe('')
    expect(useAppStore.getState().sessions.find((s) => s.id === sid)?.messages.map((m) => m.content)).toContain('old')
  })
})

describe('notification preferences (P58.9)', () => {
  test('chat notifications are on by default', () => {
    restoreWindow()
    const sid = freshSession()
    const before = useAppStore.getState().liveNotifications.length
    handleChatEvent({ type: 'cancelled', sessionId: sid, streamId: 'stream-pref-default' })
    expect(useAppStore.getState().liveNotifications.length).toBe(before + 1)
  })

  test('turning chat notifications off suppresses the chat-category push', () => {
    stubNotifyPref('notify.chat', false)
    const sid = freshSession()
    const before = useAppStore.getState().liveNotifications.length
    handleChatEvent({ type: 'cancelled', sessionId: sid, streamId: 'stream-pref-off' })
    expect(useAppStore.getState().liveNotifications.length).toBe(before)
    // The turn itself still settles — the preference gates the notification,
    // never the transcript state.
    expect(useAppStore.getState().sessions.find((s) => s.id === sid)?.status).toBe('cancelled')
    restoreWindow()
  })

  test('the task preference does not silence chat-category notifications', () => {
    stubNotifyPref('notify.quest', false)
    const sid = freshSession()
    const before = useAppStore.getState().liveNotifications.length
    handleChatEvent({ type: 'cancelled', sessionId: sid, streamId: 'stream-pref-task' })
    expect(useAppStore.getState().liveNotifications.length).toBe(before + 1)
    restoreWindow()
  })

  test('cua_requires_vision opens the vision-gate dialog (P59.3)', () => {
    const sid = freshSession()
    handleChatEvent({ type: 'ttft', sessionId: sid, streamId: 'stream-vl' })
    handleChatEvent({
      type: 'error',
      sessionId: sid,
      streamId: 'stream-vl',
      code: 'cua_requires_vision',
      message: 'Computer use needs a vision model',
    })
    expect(useAppStore.getState().cuaVisionGate).toBe(true)
    useAppStore.getState().setCuaVisionGate(false)
  })

  test('citations land on the live assistant message (P52.20)', () => {
    const sid = freshSession()
    handleChatEvent({ type: 'ttft', sessionId: sid, streamId: 'stream-cite' })
    handleChatEvent({
      type: 'citations',
      sessionId: sid,
      streamId: 'stream-cite',
      citations: [{ index: 1, title: 'A', url: 'https://ex.test/a' }],
    })
    const msg = lastMessage(sid)
    expect(msg?.citations?.[0]?.url).toBe('https://ex.test/a')
    expect(msg?.citations?.[0]?.index).toBe(1)
  })
})
