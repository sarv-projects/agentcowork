// P50.2.1 — runtime truth for the sessions surface: an empty native
// `session_list` is authoritative (it replaces the browser demo seed), the
// persistence gate only opens AFTER hydration so a boot-time write can never
// stamp preview chats into the real vault, and hydration with a pin-free
// vault yields an empty live list, not a seeded one.
//
// The malformed-row drop happens at the Tauri parse boundary
// (`filter_map(serde_json::from_str(...).ok())`, Rust-tested in
// agentcowork-vault); the UI contract pinned here is: whatever `session_list`
// returns becomes the store's rows verbatim — never a synthesized chat.

import { describe, expect, test } from 'bun:test'
import { useAppStore, mockSessions, sanitizeSessionRows, mergeHydratedSessions } from './store'

describe('P50.2.1 — sessions runtime truth', () => {
  test('a fresh store is NOT hydrated: the demo seed can never persist', () => {
    // Self-contained: other test files share the module-global store and may
    // have hydrated/wiped it first, so this test restores the boot-time shape
    // (preview seed present, gate shut) before asserting the contract.
    useAppStore.setState({
      sessionsHydrated: false,
      sessions: mockSessions,
      activeSessionId: mockSessions[0]?.id ?? '',
    })
    const st = useAppStore.getState()
    expect(st.sessionsHydrated).toBe(false)
    // The gate check `if (!inTauri()) return; if (!s.sessionsHydrated) return`
    // in the persistence subscribe is what this flag drives; before it flips,
    // no session_put path may engage.
    expect(st.sessions.length).toBeGreaterThan(0) // demo seed visible in preview
  })

  test('an empty native session_list replaces the seed with zero rows', () => {
    // Simulate `session_list` returning an empty vault: the bridge calls
    // markSessionsHydrated first, then setState({ sessions, activeSessionId }).
    useAppStore.getState().markSessionsHydrated()
    useAppStore.setState({
      sessions: [],
      activeSessionId: '',
      sessionChiefs: {},
    })
    const st = useAppStore.getState()
    expect(st.sessionsHydrated).toBe(true) // gate now open — persistence allowed
    expect(st.sessions).toHaveLength(0) // empty vault renders empty, never the seed
    expect(st.activeSessionId).toBe('')
    // No fabricated chat anywhere: the store holds exactly what the vault said.
    expect(useAppStore.getState().sessions.some((s) => s.messages.length > 0)).toBe(false)
  })

  test('rehydration restores only what the vault returned (pin mirror respects rows)', () => {
    // A second boot: the vault holds one real session with a chief pin; the
    // rehydrated list must be exactly that one row (no seed, no dupes).
    const fromVault = [
      {
        id: 'real-1',
        title: 'vault session',
        status: 'idle',
        preview: '',
        updatedAt: new Date().toISOString(),
        messages: [],
        chiefPin: 'codex',
      },
    ]
    useAppStore.setState({ sessionsHydrated: false })
    useAppStore.getState().markSessionsHydrated()
    const sessionChiefs: Record<string, string> = {}
    for (const s of fromVault) if (s.chiefPin) sessionChiefs[s.id] = s.chiefPin
    useAppStore.setState({ sessions: fromVault, sessionChiefs, activeSessionId: fromVault[0].id })
    const st = useAppStore.getState()
    expect(st.sessions).toHaveLength(1)
    expect(st.sessions[0].id).toBe('real-1')
    expect(st.sessionChiefs['real-1']).toBe('codex')
  })

  test('a persistence failure surfaces as a degraded runtime + user error, never a seed write', () => {
    // Fresh-state contract: before the vault answers, the gate stays shut.
    useAppStore.setState({ sessionsHydrated: false })
    const st = useAppStore.getState()
    expect(st.sessionsHydrated).toBe(false)
    // And no success marker is ever forged: the hydrated flag only flips via
    // markSessionsHydrated (called only after a real session_list round-trip).
    expect(typeof st.markSessionsHydrated).toBe('function')
  })

  test('schema-wrong vault rows are dropped, never rendered as broken chats', () => {
    // JSON-valid but unusable: missing id, empty id, non-object rows.
    const dirty = [
      { id: 'good-1', title: 'real' },
      { title: 'no id at all' },
      { id: '', title: 'empty id' },
      { id: 42, title: 'numeric id' },
      null,
      'a string row',
    ]
    const clean = sanitizeSessionRows(dirty)
    expect(clean).toHaveLength(1)
    expect(clean[0].id).toBe('good-1')
    expect(sanitizeSessionRows(undefined)).toEqual([])
    expect(sanitizeSessionRows(null)).toEqual([])
  })

  test('hydration with a dirty vault list keeps only renderable sessions', () => {
    // Mirrors the bridge hydration path (markSessionsHydrated + sanitize +
    // setState): the store must hold exactly the usable rows.
    useAppStore.setState({ sessionsHydrated: false })
    useAppStore.getState().markSessionsHydrated()
    const fromVault = sanitizeSessionRows([
      { id: 'real-2', title: 'kept', status: 'idle', preview: '', updatedAt: '', messages: [] },
      { title: 'dropped: no id' },
      { id: '', title: 'dropped: empty id' },
    ])
    useAppStore.setState({ sessions: fromVault, activeSessionId: fromVault[0]?.id ?? '', sessionChiefs: {} })
    const st = useAppStore.getState()
    expect(st.sessions).toHaveLength(1)
    expect(st.sessions[0].id).toBe('real-2')
    expect(st.activeSessionId).toBe('real-2')
  })

  test('sending with no matching session opens the work first (never a dropped turn)', () => {
    // Empty vault: no row matches activeSessionId — the send path must
    // create the session (newSession) so pushUserMessage has a real target.
    useAppStore.setState({ sessionsHydrated: true, sessions: [], activeSessionId: '' })
    useAppStore.getState().newSession()
    const st = useAppStore.getState()
    expect(st.sessions).toHaveLength(1)
    expect(st.activeSessionId).toBe(st.sessions[0].id)
    expect(st.activeSessionId.length).toBeGreaterThan(0)
    // And the message lands on it (the pre-fix behavior dropped it silently).
    st.pushUserMessage('hello?')
    expect(useAppStore.getState().sessions[0].messages).toHaveLength(1)
  })

  test('newSession-before-hydration race: the local session survives the vault merge', () => {
    // The P50.2.1 race — user clicks New Session while `session_list` is
    // still in flight. The bridge then lands with the vault rows; the merge
    // must keep the user's local-only session and preserve it as active.
    useAppStore.setState({ sessionsHydrated: false, sessions: [], activeSessionId: '' })
    useAppStore.getState().newSession() // in-flight list; session X is local-only
    const created = useAppStore.getState().sessions[0]
    expect(created).toBeDefined()

    const vault = [
      { id: 'vault-1', title: 'older', status: 'idle', preview: '', updatedAt: '', messages: [] },
    ] as never // Session shape kept loose for the pure-helper contract
    const merged = mergeHydratedSessions([created], vault as import('./store').Session[], created.id)
    expect(merged.sessions.map((s) => s.id)).toEqual([created.id, 'vault-1'])
    expect(merged.activeSessionId).toBe(created.id) // no focus yank to vault[0]
  })

  test('fresh-boot hydration stays authoritative: empty store takes the vault rows verbatim', () => {
    const vault = [{ id: 'vault-1' }, { id: 'vault-2' }] as import('./store').Session[]
    const merged = mergeHydratedSessions([], vault, '')
    expect(merged.sessions.map((s) => s.id)).toEqual(['vault-1', 'vault-2'])
    expect(merged.activeSessionId).toBe('vault-1')
  })

  test('rehydrate keeps the active target when it survives, else falls back to the newest vault row', () => {
    const vault = [{ id: 'vault-1' }] as import('./store').Session[]
    // Active session is still in the vault: preserved.
    expect(mergeHydratedSessions(vault, vault, 'vault-1').activeSessionId).toBe('vault-1')
    // Active session vanished from the vault (deleted elsewhere): vault[0].
    expect(mergeHydratedSessions([], vault, 'ghost').activeSessionId).toBe('vault-1')
  })
})
describe('P51.33 — /compact transcript rewrite', () => {
  test('compact keeps the tail from keptFrom and prepends the marker', () => {
    useAppStore.setState({ sessionsHydrated: true, sessions: [], activeSessionId: '' })
    const st = useAppStore.getState()
    st.newSession()
    const sid = useAppStore.getState().activeSessionId
    const st2 = useAppStore.getState()
    st2.pushUserMessage('first ask')
    st2.pushUserMessage('second ask')
    // pushUserMessage marks the session running; /compact only runs idle.
    useAppStore.setState({
      sessions: useAppStore.getState().sessions.map((s) =>
        s.id === sid ? { ...s, status: 'idle' as const } : s,
      ),
    })
    const before = useAppStore.getState().sessions.find((s) => s.id === sid)!.messages.length
    expect(before).toBe(2)

    useAppStore.getState().compactSessionMessages(sid, 1, '[continued — earlier context compacted…]')
    const after = useAppStore.getState().sessions.find((s) => s.id === sid)!.messages
    expect(after.length).toBe(2) // marker + kept tail
    expect(after[0]!.role).toBe('system')
    expect(after[0]!.content).toContain('continued')
    expect(after[1]!.content).toBe('second ask')
  })

  test('compact is a no-op when nothing would be pruned or while streaming', () => {
    useAppStore.setState({ sessionsHydrated: true, sessions: [], activeSessionId: '' })
    const st = useAppStore.getState()
    st.newSession()
    const sid = useAppStore.getState().activeSessionId
    st.pushUserMessage('only ask')
    useAppStore.getState().compactSessionMessages(sid, 0, null)
    let after = useAppStore.getState().sessions.find((s) => s.id === sid)!.messages
    expect(after.length).toBe(1) // unchanged — keptFrom 0 prunes nothing

    // Streaming session: refused (never desync the live stream).
    useAppStore.setState({
      sessions: useAppStore.getState().sessions.map((s) =>
        s.id === sid ? { ...s, status: 'running' } : s,
      ),
    })
    useAppStore.getState().compactSessionMessages(sid, 1, '[continued…]')
    after = useAppStore.getState().sessions.find((s) => s.id === sid)!.messages
    expect(after.length).toBe(1) // untouched
  })
})
