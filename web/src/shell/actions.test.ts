import { beforeAll, describe, expect, it } from 'vitest'

// actions.ts schedules toast expiry on `window`.
beforeAll(() => {
  ;(globalThis as unknown as { window: typeof globalThis }).window ??= globalThis
})

describe('openPanel on a phone', () => {
  it('routes to the tab that can show the panel, and never queues', async () => {
    const { openPanel, setMobileRouter, useToasts, isMobileShell } = await import('./actions')
    const seen: string[] = []
    setMobileRouter((p) => {
      seen.push(`${p.kind}:${String(p.params.terminalId ?? '')}`)
      return p.kind === 'terminal'
    })
    expect(isMobileShell()).toBe(true)
    // "Open" on an attention toast, or after asking an agent.
    expect(openPanel({ kind: 'terminal', id: 'terminal:t1', title: 'Pinger', params: { terminalId: 't1' } })).toBe('terminal:t1')
    expect(seen).toEqual(['terminal:t1'])
    expect(useToasts.getState().toasts).toEqual([])
    // A desktop-only panel says so instead of vanishing.
    openPanel({ kind: 'diff', title: 'app.js', params: { path: 'app.js' } })
    expect(useToasts.getState().toasts.map((t) => t.message)).toEqual(['app.js opens on the desktop'])
    setMobileRouter(null)
    expect(isMobileShell()).toBe(false)
  })
})
