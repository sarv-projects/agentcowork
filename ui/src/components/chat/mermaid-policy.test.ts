import { describe, expect, it } from 'bun:test'
import { checkMermaidSource } from './mermaid-policy'

describe('Mermaid source policy', () => {
  it('accepts a normal diagram and strips surrounding whitespace', () => {
    expect(checkMermaidSource('\n  flowchart LR\n A --> B  \n')).toEqual({
      ok: true,
      source: 'flowchart LR\n A --> B',
    })
  })

  it('rejects empty, oversized, and renderer-configuration input', () => {
    expect(checkMermaidSource('  ')).toMatchObject({ ok: false })
    expect(checkMermaidSource('x'.repeat(50_001))).toMatchObject({ ok: false })
    expect(checkMermaidSource('flowchart LR\n%%{init: {"securityLevel":"loose"}}%%\nA-->B')).toMatchObject({ ok: false })
  })

  it('rejects graphs over the connection cap', () => {
    const edges = Array.from({ length: 501 }, (_, index) => `n${index} --> n${index + 1}`).join('\n')
    expect(checkMermaidSource(`flowchart LR\n${edges}`)).toMatchObject({ ok: false })
  })
})
