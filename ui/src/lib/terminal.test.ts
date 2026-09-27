import { afterAll, describe, expect, test } from 'bun:test'
import { backendLabel, decodeChunk, demoProfiles, terminalProfiles } from '@/lib/terminal'

// H36 (P54) — the Shell view trusts three things from this bridge: the raw
// byte decode (a bad frame must never be written to xterm), the backend label,
// and the honest preview fallback. Spawning is exercised in Rust
// (`agentcowork-core::terminal` PTY round-trip test) — not faked here.

const originalWindow = (globalThis as { window?: unknown }).window

afterAll(() => {
  if (originalWindow === undefined) {
    delete (globalThis as { window?: unknown }).window
  } else {
    ;(globalThis as { window?: unknown }).window = originalWindow
  }
})

describe('terminal bridge', () => {
  test('decodes base64 PTY frames to raw bytes', () => {
    // "hi\r" — the exact shape a keystroke produces.
    const bytes = decodeChunk(btoa('hi\r'))
    expect(bytes).not.toBeNull()
    expect(Array.from(bytes!)).toEqual([104, 105, 13])
  })

  test('preserves non-UTF8 bytes (CSI sequences, latin1 output)', () => {
    const raw = new Uint8Array([0x1b, 0x5b, 0x33, 0x31, 0x6d, 0xff, 0xfe])
    let bin = ''
    for (const b of raw) bin += String.fromCharCode(b)
    const bytes = decodeChunk(btoa(bin))
    expect(Array.from(bytes!)).toEqual(Array.from(raw))
  })

  test('returns null for an empty or undecodable frame', () => {
    expect(decodeChunk('')).toBeNull()
    // `!!` is not valid base64 — must not throw or write garbage.
    expect(decodeChunk('!!not-base64!!')).toBeNull()
  })

  test('labels each backend', () => {
    expect(backendLabel('local')).toBe('Local PTY')
    expect(backendLabel('wsl')).toBe('WSL')
    expect(backendLabel('remote')).toBe('Remote node')
  })

  test('preview fallback exposes exactly one default and marks remote unavailable', async () => {
    // No `__TAURI_INTERNALS__` → the bridge must return the labelled demo
    // registry, never a fabricated spawn result.
    const reg = await terminalProfiles()
    expect(reg.remoteBackendAvailable).toBe(false)
    expect(reg.hostAbiVersion).toBe(1)
    const offered = reg.profiles.filter((p) => p.offered)
    expect(offered.length).toBeGreaterThan(0)
    expect(reg.profiles.filter((p) => p.isDefault).length).toBe(1)
    // The demo default must be a profile that is actually offered.
    expect(offered.map((p) => p.profileName)).toContain(reg.defaultProfile)
    expect(demoProfiles().platform).toBe(reg.platform)
  })
})
