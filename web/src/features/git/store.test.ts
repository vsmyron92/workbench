import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useOps } from './store'

describe('remote operation cards', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.stubGlobal('window', globalThis)
  })
  afterEach(() => {
    vi.useRealTimers()
    vi.unstubAllGlobals()
    useOps.setState({ ops: {} })
  })

  it('auto-dismisses a clean success', () => {
    useOps.getState().start({ opId: 'a', projectId: 'p', op: 'pull', title: 'Update main (merge)' })
    useOps.getState().event('p', { opId: 'a', op: 'pull', done: true, ok: true, message: 'Updated: 1 file changed' })
    expect(useOps.getState().ops.a?.ok).toBe(true)
    vi.advanceTimersByTime(5000)
    expect(useOps.getState().ops.a).toBeUndefined()
  })

  it('keeps an update whose local changes conflict on screen, marked as conflicts', () => {
    useOps.getState().start({ opId: 'b', projectId: 'p', op: 'pull', title: 'Update main (merge)' })
    useOps.getState().event('p', {
      opId: 'b',
      op: 'pull',
      done: true,
      ok: false,
      conflicts: true,
      message: 'Updated, but re-applying your local changes conflicts in 1 file.',
    })
    vi.advanceTimersByTime(60_000)
    const op = useOps.getState().ops.b
    expect(op).toBeDefined()
    expect(op.done && !op.ok && op.conflicts).toBe(true)
  })
})
