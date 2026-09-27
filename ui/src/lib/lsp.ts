// P11.5.3 — LSP bridge (lsp_cmds.rs → agentcowork-codeintel LspRunner).

import { invoke, inTauri } from './tauri'
import { nativeCall } from './runtime'

export interface LspProblem {
  path: string
  line: number
  col: number
  severity: number // 1=error 2=warning 3=info
  severityLabel: string
  message: string
  source: string | null
}

export interface LspResult {
  rows: LspProblem[]
  count: number
  error?: string
}

export async function lspDiagnostics(
  root: string,
  path: string,
  language: string,
  text: string,
): Promise<LspResult> {
  if (!inTauri()) {
    // Honest preview: no fake problems. Real diagnostics need the Tauri
    // shell + an installed language server.
    return { rows: [], count: 0, error: 'preview — LSP needs the Tauri shell' }
  }
  try {
    return await nativeCall('LSP diagnostics', () => invoke<LspResult>('lsp_diagnostics', { root, path, language, text }))
  } catch (e) {
    return { rows: [], count: 0, error: String(e) }
  }
}
