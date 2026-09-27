'use client'

// P15-H29 — local dashboard artifacts (doc 67 §1, bolt.diy STEAL).
//
// Bridge for the artifact action stream + loopback preview server:
//   - `startArtifactServer(workspace)` serves the guarded artifact folder on
//     127.0.0.1:<port> via the Rust `ArtifactServer` (agentcowork-script).
//   - `stopArtifactServer(port)` tears it down.
//   - `tickArtifactActions` reflects runner state into the store checklist.
//
// In the Tauri webview these are real commands; outside it (browser
// preview / dev) the bridge synthesizes a demo server on a mock URL so the
// surface stays explorable — the same graceful-fallback convention as the
// rest of the bridge layer.

import { inTauri, invoke } from '@/lib/tauri'
import { bridgeCall } from '@/lib/runtime'
import {
  useAppStore,
  type ArtifactActionUi,
  type ArtifactServerState,
} from '@/lib/store'

export async function startArtifactServer(
  workspace: string,
): Promise<ArtifactServerState> {
  return bridgeCall({
    operation: 'artifact server start',
    live: async () => {
      const port = (await invoke('artifact_serve', { workspace })) as number
      return { port, url: `http://127.0.0.1:${port}/`, status: 'serving' as const }
    },
    preview: () => {
      const demoPort = 4500 + Math.floor(Math.random() * 200)
      return { port: demoPort, url: `http://127.0.0.1:${demoPort}/`, status: 'serving' as const, demo: true }
    },
  })
}

export async function stopArtifactServer(port: number): Promise<void> {
  await bridgeCall({
    operation: 'artifact server stop',
    live: async () => {
      await invoke('artifact_stop', { port })
    },
    preview: () => undefined,
  })
  if (!inTauri()) {
    useAppStore.getState().patchArtifactServer({ port, url: '', status: 'stopped' })
  }
}

export function demoActionChecklist(prefix = 'q3-dashboard'): ArtifactActionUi[] {
  return [
    { index: 0, label: `write ${prefix}/index.html`, state: 'complete' },
    { index: 1, label: `write ${prefix}/app.css`, state: 'complete' },
    { index: 2, label: 'run npm install', state: 'complete' },
    { index: 3, label: 'start preview server', state: 'running' },
    { index: 4, label: 'finish', state: 'pending' },
  ]
}