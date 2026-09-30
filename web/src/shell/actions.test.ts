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

describe('the agents column', () => {
  it('takes terminal and agents-home panels instead of the dock, also ones asked for before it is mounted', async () => {
    const { closePanel, isColumnKind, isPanelOpen, openPanel, setColumnHost } = await import('./actions')
    expect([isColumnKind('terminal'), isColumnKind('agents.home'), isColumnKind('editor')]).toEqual([true, true, false])
    // Startup: a terminal is opened before the column exists.
    expect(openPanel({ kind: 'terminal', id: 'terminal:t1', title: 'Build', params: { terminalId: 't1' } })).toBe('terminal:t1')
    const open = new Set<string>()
    const seen: string[] = []
    setColumnHost({
      open: (p) => {
        seen.push(`${p.id}:${p.focus ? 'focus' : 'quiet'}`)
        open.add(p.id)
      },
      close: (id) => void open.delete(id),
      isOpen: (id) => open.has(id),
    })
    expect(seen).toEqual(['terminal:t1:focus'])
    openPanel({ kind: 'agents.home', id: 'agents.home' })
    openPanel({ kind: 'terminal', id: 'terminal:t2', params: { terminalId: 't2' }, focus: false })
    expect(seen).toEqual(['terminal:t1:focus', 'agents.home:focus', 'terminal:t2:quiet'])
    expect([isPanelOpen('terminal:t1'), isPanelOpen('terminal:t9')]).toEqual([true, false])
    closePanel('terminal:t1')
    expect(isPanelOpen('terminal:t1')).toBe(false)
    setColumnHost(null)
  })

  it('is not there on a phone: the Agents tab shows the terminal', async () => {
    const { openPanel, setColumnHost, setMobileRouter } = await import('./actions')
    const column: string[] = []
    const phone: string[] = []
    setColumnHost({ open: (p) => void column.push(p.id), close: () => {}, isOpen: () => false })
    setMobileRouter((p) => {
      phone.push(p.id)
      return true
    })
    openPanel({ kind: 'terminal', id: 'terminal:t1', params: { terminalId: 't1' } })
    expect([column, phone]).toEqual([[], ['terminal:t1']])
    setMobileRouter(null)
    setColumnHost(null)
  })
})
