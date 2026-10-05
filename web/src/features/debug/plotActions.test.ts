import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { LiveItem, LiveSnapshot } from './types'

const api = vi.hoisted(() => ({ liveList: vi.fn(), liveAdd: vi.fn(), liveRemove: vi.fn(), liveHistory: vi.fn(), plotsList: vi.fn(), plotPut: vi.fn(), plotDelete: vi.fn() }))
vi.mock('./api', () => ({ debugApi: api }))
const shell = vi.hoisted(() => ({ openPanel: vi.fn(), toast: vi.fn(), toastError: vi.fn() }))
vi.mock('@/shell/actions', () => shell)

import { useLive } from './liveStore'
import { addSeriesTo, plottable, watchForPlot } from './plotActions'
import { usePlots, type PlotConfig } from './plotStore'
import type { DebugSession } from './types'

const session = { id: 's1', projectId: 'p', live: true } as DebugSession
const item = (id: number, expression: string, over: Partial<LiveItem> = {}): LiveItem => ({ id, expression, address: 0x20000000 + id * 4, size: 4, kind: 'uint', typeName: 'uint32_t', ...over })
const bytes = (id: number, expression: string) => item(id, expression, { kind: 'bytes', size: 8, typeName: 'uint8_t [8]' })
const snap = (...items: LiveItem[]): LiveSnapshot => ({ items, intervalMs: 250, last: {} })

/** A server that remembers the plots it is sent. */
const server = new Map<string, PlotConfig>()

beforeEach(() => {
  vi.clearAllMocks()
  server.clear()
  useLive.setState({ sessions: {} })
  usePlots.setState({ byProject: {}, loaded: {} })
  api.liveHistory.mockResolvedValue({ intervalMs: 250, items: [], series: {}, now: 0 })
  api.plotsList.mockImplementation(async () => ({ plots: [...server.values()] }))
  api.plotPut.mockImplementation(async (_pid: string, p: PlotConfig) => (server.set(p.id, p), p))
  api.plotDelete.mockImplementation(async (_pid: string, id: string) => (server.delete(id), { ok: true }))
})

describe('what can be drawn', () => {
  it('takes numbers and refuses bytes and what could not be resolved', () => {
    expect(plottable(item(1, 'a'))).toBe(true)
    expect(plottable(item(1, 'a', { kind: 'float' }))).toBe(true)
    expect(plottable(bytes(1, 'a'))).toBe(false)
    expect(plottable(item(1, 'a', { error: 'No symbol' }))).toBe(false)
    expect(plottable(item(1, 'a', { address: undefined }))).toBe(false)
  })
})

describe('watching an expression for a plot', () => {
  it('never removes a watch the user already had, even when the store did not know it yet', async () => {
    // A char buf[8] is watched in the Live tab; this panel has just opened and its snapshot has not landed.
    api.liveList.mockResolvedValue(snap(bytes(7, 'buf')))
    api.liveAdd.mockResolvedValue(bytes(7, 'buf')) // the server's add is idempotent: it answers with the existing item
    expect(await watchForPlot(session, 'buf')).toBeNull()
    expect(api.liveRemove).not.toHaveBeenCalled()
    expect(api.liveAdd).not.toHaveBeenCalled()
    expect(shell.toast).toHaveBeenCalledWith('error', expect.stringContaining('only numbers'))
  })

  it('removes the watch it just made when the value cannot be drawn', async () => {
    api.liveList.mockResolvedValueOnce(snap()).mockResolvedValue(snap(bytes(9, 'buf')))
    api.liveAdd.mockResolvedValue(bytes(9, 'buf'))
    expect(await watchForPlot(session, 'buf')).toBeNull()
    expect(api.liveRemove).toHaveBeenCalledWith('p', 's1', 9)
  })

  it('leaves a watch alone when the server\'s list could not be had to tell whether it was new', async () => {
    api.liveList.mockRejectedValue(new Error('offline'))
    api.liveAdd.mockResolvedValue(bytes(9, 'buf'))
    expect(await watchForPlot(session, 'buf')).toBeNull()
    expect(api.liveRemove).not.toHaveBeenCalled()
  })

  it('watches a new expression and hands back its item', async () => {
    api.liveList.mockResolvedValue(snap())
    api.liveAdd.mockResolvedValue(item(3, 'ticks'))
    expect((await watchForPlot(session, 'ticks'))?.id).toBe(3)
    expect(api.liveAdd).toHaveBeenCalledWith('p', 's1', 'ticks')
  })

  it('uses a value the store already has without asking the server', async () => {
    useLive.setState({ sessions: { s1: { items: [item(4, 'adc')], intervalMs: 250, samples: {}, loaded: true, pausing: false, pauseMs: null } } })
    expect((await watchForPlot(session, 'adc'))?.id).toBe(4)
    expect(api.liveList).not.toHaveBeenCalled()
    expect(api.liveAdd).not.toHaveBeenCalled()
  })

  it('reports why when the debugger cannot watch it', async () => {
    api.liveList.mockResolvedValue(snap())
    api.liveAdd.mockRejectedValue(new Error('No symbol "nope" in current context.'))
    expect(await watchForPlot(session, 'nope')).toBeNull()
    expect(shell.toastError).toHaveBeenCalled()
  })
})

describe('adding a series to a plot', () => {
  const plot = () => usePlots.getState().create('p', { name: 'Motor' })!

  it('adds without a session, as an expression to draw later', async () => {
    const p = plot()
    expect(await addSeriesTo('p', p.id, ' rpm ', null)).toBe(true)
    expect(usePlots.getState().byProject.p[0].series.map((s) => s.expression)).toEqual(['rpm'])
    expect(api.liveAdd).not.toHaveBeenCalled()
  })

  it('watches first when a session is live, and adds what the server resolved', async () => {
    const p = plot()
    api.liveList.mockResolvedValue(snap())
    api.liveAdd.mockResolvedValue(item(3, 'ticks'))
    expect(await addSeriesTo('p', p.id, 'ticks', session)).toBe(true)
    expect(api.liveAdd).toHaveBeenCalled()
    expect(usePlots.getState().byProject.p[0].series[0]).toMatchObject({ expression: 'ticks', slot: 1 })
  })

  it('adds nothing when the value cannot be drawn', async () => {
    const p = plot()
    api.liveList.mockResolvedValue(snap())
    api.liveAdd.mockResolvedValue(bytes(3, 'buf'))
    expect(await addSeriesTo('p', p.id, 'buf', session)).toBe(false)
    expect(usePlots.getState().byProject.p[0].series).toEqual([])
  })

  it('says so when the plot is full or was deleted meanwhile, and when the series is already there', async () => {
    const p = usePlots.getState().create('p', { expressions: ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'] })!
    expect(await addSeriesTo('p', p.id, 'i', session)).toBe(false)
    expect(shell.toast).toHaveBeenCalledWith('error', expect.stringContaining('8 series'))
    shell.toast.mockClear()
    expect(await addSeriesTo('p', p.id, 'a', null)).toBe(false)
    expect(shell.toast).toHaveBeenCalledWith('info', expect.stringContaining('already in'), expect.anything())
    // Deleted while the watch was being made:
    const q = plot()
    api.liveList.mockResolvedValue(snap())
    api.liveAdd.mockImplementation(async () => {
      usePlots.getState().remove('p', q.id)
      return item(5, 'late')
    })
    shell.toast.mockClear()
    expect(await addSeriesTo('p', q.id, 'late', session)).toBe(false)
    expect(shell.toast).toHaveBeenCalledWith('info', expect.stringContaining('deleted'))
  })
})
