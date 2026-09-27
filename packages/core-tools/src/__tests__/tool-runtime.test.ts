import { describe, it, expect } from 'vitest';
import { ToolRuntime } from '../index';
import { z } from 'zod';
import type { ToolContract } from '../types';

describe('ToolRuntime', () => {
  it('registers and executes tools', async () => {
    const runtime = new ToolRuntime();
    const mockExec = async () => ({ files: ['file1.pdf'] });
    const tool: ToolContract = {
      id: 'search_local_files',
      family: 'knowledge',
      riskLevel: 'read',
      inputSchema: z.object({ query: z.string() }),
      outputSchema: z.any(),
      surfaceAllowlist: ['chat'],
      execute: mockExec,
    };
    runtime.register(tool);

    const result = await runtime.execute('search_local_files', { surface: 'chat' }, { query: 'test' });
    expect(result.durationMs).toBeGreaterThanOrEqual(0);
  });

  it('throws for unknown tool', async () => {
    const runtime = new ToolRuntime();
    await expect(runtime.execute('unknown', { surface: 'chat' }, {})).rejects.toThrow('Tool not found');
  });
});

// Permission classification (permission gate, trust ladder) now lives in
// `@agentcowork/core-engine` (`src/policy/`) and is covered there. This package
// declares tools; it does not decide whether an effect may run — Guard does.
