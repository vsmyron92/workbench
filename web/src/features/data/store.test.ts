import { beforeEach, describe, expect, it, vi } from 'vitest'

const api = vi.hoisted(() => ({ snapshot: vi.fn(), putPlot: vi.fn() }))
vi.mock('./api', () => ({ dataApi: api }))
const shell = vi.hoisted(() => ({ toastError: vi.fn() }))
vi.mock('@/shell/actions', () => shell)
const bus = vi.hoisted(() => {
  const handlers = new Map<string, ((ev: unknown) => void)[]>()
  return {
    handlers,
    subscribe: (type: string, fn: (ev: unknown) => void) => {
      handlers.set(type, [...(handlers.get(type) ?? []), fn])
      return () => handlers.set(type, (handlers.get(type) ?? []).filter((f) => f !== fn))
    },
    emit: (type: string, data: unknown) => (handlers.get(type) ?? []).forEach((f) => f({ type, data })),
  }
})
vi.mock('@/api/events', () => ({ subscribe: bus.subscribe }))
import { installDataEvents, loadData, savePlot, setLinked, useData, useLinked } from './store'
import type { PlotConfig, SeriesInfo, Snapshot } from './types'

const info = (name: string, over: Partial<SeriesInfo> = {}): SeriesInfo => ({
  name, kind: 'analog', unit: 'A', label: '', group: '', offset: 0, maxPoints: 1000, rate: null, count: 0, appended: 0, dropped: 0,
  tStart: null, tEnd: null, wall: false, last: null, generation: 0, updatedAt: 0, ...over,
})
const plot = (id: string, over: Partial<PlotConfig> = {}): PlotConfig => ({ id, title: id, lanes: [], span: 10, follow: true, linked: true, markers: [], ...over })
const snap = (over: Partial<Snapshot> = {}): Snapshot => ({ series: [info('I')], plots: [plot('a')], sources: [], ...over })

let off: () => void
beforeEach(() => {
  off?.()
  bus.handlers.clear()
  useData.setState({ byProject: {}, changed: {} })
  useLinked.setState({ views: {} })
  api.snapshot.mockReset()
  api.putPlot.mockReset()
  shell.toastError.mockReset()
  off = installDataEvents()
})

describe('loading', () => {
  it('loads a project once and marks every series as changed, so views are fetched', async () => {
    api.snapshot.mockResolvedValue(snap())
    await loadData('lab')
    await loadData('lab')
    expect(api.snapshot).toHaveBeenCalledTimes(1)
    const s = useData.getState()
    expect(s.byProject.lab.loaded).toBe(true)
    expect(s.byProject.lab.series.map((x) => x.name)).toEqual(['I'])
    expect(s.changed.lab.I).toBe(1)
  })

  it('keeps the error when the server cannot be reached, and loads again on request', async () => {
    api.snapshot.mockRejectedValueOnce(new Error('offline'))
    await loadData('lab')
    expect(useData.getState().byProject.lab.error).toBe('offline')
    api.snapshot.mockResolvedValue(snap())
    await loadData('lab', true)
    expect(useData.getState().byProject.lab.loaded).toBe(true)
  })
})

describe('events', () => {
  beforeEach(async () => {
    api.snapshot.mockResolvedValue(snap())
    await loadData('lab')
  })

  it('merges the series a data.changed names and marks them changed', () => {
    const before = useData.getState().changed.lab.I
    bus.emit('data.changed', { projectId: 'lab', series: [info('I', { count: 500, tEnd: 5 }), info('V', { unit: 'V' })] })
    const s = useData.getState()
    expect(s.byProject.lab.series.map((x) => [x.name, x.count])).toEqual([['I', 500], ['V', 0]])
    expect(s.changed.lab.I).toBe(before + 1)
    expect(s.changed.lab.V).toBe(1)
  })

  it('replaces the list on data.series and marks a cleared series stale', () => {
    bus.emit('data.changed', { projectId: 'lab', series: [info('I', { count: 500, generation: 0 })] })
    const before = useData.getState().changed.lab.I
    bus.emit('data.series', { projectId: 'lab', series: [info('I', { count: 0, generation: 1 })] })
    expect(useData.getState().changed.lab.I).toBe(before + 1)
    // A list that changes nothing about a series does not make its views stale.
    const again = useData.getState().changed.lab.I
    bus.emit('data.series', { projectId: 'lab', series: [info('I', { count: 0, generation: 1 })] })
    expect(useData.getState().changed.lab.I).toBe(again)
  })

  it('takes plots and sources, and ignores a project it never loaded', () => {
    bus.emit('data.plots', { projectId: 'lab', plots: [plot('a'), plot('b')] })
    expect(useData.getState().byProject.lab.plots.map((p) => p.id)).toEqual(['a', 'b'])
    bus.emit('data.plots', { projectId: 'other', plots: [plot('z')] })
    expect(useData.getState().byProject.other).toBeUndefined()
  })

  it('reloads what it holds after a resync', async () => {
    api.snapshot.mockClear()
    bus.emit('resync', null)
    await vi.waitFor(() => expect(api.snapshot).toHaveBeenCalledTimes(1))
  })
})

describe('saving a plot', () => {
  beforeEach(async () => {
    api.snapshot.mockResolvedValue(snap())
    await loadData('lab')
  })

  it('shows the change at once and writes it', async () => {
    api.putPlot.mockResolvedValue(undefined)
    const p = plot('a', { title: 'Renamed' })
    const done = savePlot('lab', p)
    expect(useData.getState().byProject.lab.plots[0].title).toBe('Renamed')
    await done
    expect(api.putPlot).toHaveBeenCalledWith('lab', p)
  })

  it('puts the old plot back and says why when the server refuses', async () => {
    api.putPlot.mockRejectedValue(new Error('too many lanes'))
    await savePlot('lab', plot('a', { title: 'Nope' }))
    expect(useData.getState().byProject.lab.plots[0].title).toBe('a')
    expect(shell.toastError).toHaveBeenCalled()
  })
})

describe('linked views', () => {
  it('are kept per project', () => {
    setLinked('lab', { kind: 'fixed', t0: 1, t1: 2 })
    setLinked('other', { kind: 'follow', span: 5 })
    expect(useLinked.getState().views.lab).toEqual({ kind: 'fixed', t0: 1, t1: 2 })
    setLinked('lab', null)
    expect(useLinked.getState().views.lab).toBeNull()
    expect(useLinked.getState().views.other).toEqual({ kind: 'follow', span: 5 })
  })
})
