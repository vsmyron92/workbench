import { beforeEach, describe, expect, it, vi } from 'vitest'

const api = vi.hoisted(() => ({ plotsList: vi.fn(), plotPut: vi.fn(), plotDelete: vi.fn() }))
vi.mock('./api', () => ({ debugApi: api }))
const shell = vi.hoisted(() => ({ toastError: vi.fn(), toast: vi.fn(), openPanel: vi.fn() }))
vi.mock('@/shell/actions', () => shell)
import { DEFAULT_WINDOW } from './plotMath'
import {
  copyOf,
  makePlot,
  MAX_EXPRESSION,
  MAX_NAME,
  MAX_PLOTS_PER_PROJECT,
  nextName,
  sanitizePlots,
  withoutSeries,
  withSeries,
  withToggled,
  usePlots,
  type PlotConfig,
} from './plotStore'

const plot = (over: Partial<PlotConfig> = {}): PlotConfig => ({ id: 'a', name: 'Plot 1', series: [], windowMs: DEFAULT_WINDOW, scale: 'shared', ...over })

describe('plot configurations', () => {
  it('names new plots with the lowest free number', () => {
    expect(nextName([])).toBe('Plot 1')
    expect(nextName([plot({ name: 'Plot 1' }), plot({ name: 'Plot 3' })])).toBe('Plot 2')
    expect(nextName([plot({ name: 'Motor' })])).toBe('Plot 1')
  })

  it('gives each series the first free colour and never repaints the others', () => {
    let c = plot()
    for (const e of ['a', 'b', 'c']) c = withSeries(c, e).cfg
    expect(c.series.map((s) => [s.expression, s.slot])).toEqual([['a', 1], ['b', 2], ['c', 3]])
    c = withoutSeries(c, 'b')
    expect(c.series.map((s) => s.slot)).toEqual([1, 3]) // c keeps its colour
    c = withSeries(c, 'd').cfg
    expect(c.series.map((s) => [s.expression, s.slot])).toEqual([['a', 1], ['c', 3], ['d', 2]]) // d takes the freed one
  })

  it('refuses a duplicate, an empty expression and a ninth series', () => {
    let c = plot()
    expect(withSeries(c, 'x').result).toBe('added')
    c = withSeries(c, 'x').cfg
    expect(withSeries(c, ' x ').result).toBe('exists')
    expect(withSeries(c, '   ').result).toBe('exists')
    for (let i = 0; i < 7; i++) c = withSeries(c, `s${i}`).cfg
    expect(c.series).toHaveLength(8)
    const full = withSeries(c, 'one more')
    expect(full.result).toBe('full')
    expect(full.cfg).toBe(c)
  })

  it('caps the length of an expression', () => {
    const { cfg } = withSeries(plot(), 'x'.repeat(MAX_EXPRESSION + 50))
    expect(cfg.series[0].expression).toHaveLength(MAX_EXPRESSION)
  })

  it('hides and shows a series without touching the rest', () => {
    let c = withSeries(withSeries(plot(), 'a').cfg, 'b').cfg
    c = withToggled(c, 'a')
    expect(c.series[0].hidden).toBe(true)
    expect(c.series[1].hidden).toBeUndefined()
    c = withToggled(c, 'a')
    expect(c.series[0].hidden).toBeUndefined()
    expect(withoutSeries(c, 'nope')).toBe(c)
  })

  it('copies a plot into one of its own, with the same colours', () => {
    const src = withSeries(withSeries(plot({ id: 'src', name: 'Motor' }), 'rpm').cfg, 'temp').cfg
    const a = copyOf(src, [src])
    expect(a.name).toBe('Motor copy')
    expect(a.id).not.toBe('src')
    expect(a.series).toEqual(src.series)
    expect(a.series[0]).not.toBe(src.series[0])
    expect(copyOf(src, [src, a]).name).toBe('Motor copy 2')
  })

  it('copies a plot with a long name without hanging, and keeps every name within the limit', () => {
    for (const len of [10, 50, 55, 59, 60]) {
      const src = plot({ id: 'src', name: 'm'.repeat(len) })
      const all = [src]
      for (let i = 0; i < 5; i++) all.push(copyOf(src, all))
      const names = all.map((p) => p.name)
      expect(new Set(names).size).toBe(6)
      expect(names.every((n) => n.length <= MAX_NAME)).toBe(true)
    }
  })

  it('makes a plot from expressions, as many as the colours allow', () => {
    const c = makePlot([], { expressions: Array.from({ length: 12 }, (_, i) => `v${i}`) })
    expect(c.series).toHaveLength(8)
    expect(c.name).toBe('Plot 1')
    expect(makePlot([c], { name: '  ADC  ' }).name).toBe('ADC')
    expect(makePlot([c], { name: '   ' }).name).toBe('Plot 2')
  })
})

describe('what comes back from storage', () => {
  it('keeps a good plot as it is', () => {
    const good = plot({ id: 'p1', name: 'Motor', windowMs: 60_000, scale: 'normalized', series: [{ expression: 'rpm', slot: 4, hidden: true }, { expression: 'temp', slot: 2 }] })
    expect(sanitizePlots({ proj: [good] })).toEqual({ proj: [good] })
  })

  it('repairs what it can and drops what it cannot', () => {
    const out = sanitizePlots({
      proj: [
        { id: 'p1', name: 5, windowMs: 1234, scale: 'weird', series: [{ expression: 'a', slot: 99 }, { expression: 'a', slot: 1 }, { expression: 'b', slot: 1 }, { expression: '', slot: 2 }, { slot: 3 }, 'x', null] },
        { id: 'p1', name: 'twin' }, // the id is taken
        { name: 'no id' },
        'junk',
      ],
      broken: 'not a list',
      empty: [],
    })
    expect(Object.keys(out)).toEqual(['proj'])
    expect(out.proj).toHaveLength(1)
    const p = out.proj[0]
    expect(p.name).toBe('Plot')
    expect(p.windowMs).toBe(DEFAULT_WINDOW)
    expect(p.scale).toBe('shared')
    // 'a' got a free colour for its bad slot, the duplicate 'a' went, 'b' wanted slot 1 which 'a' now has.
    expect(p.series.map((s) => s.expression)).toEqual(['a', 'b'])
    expect(new Set(p.series.map((s) => s.slot)).size).toBe(2)
    expect(p.series.every((s) => s.slot >= 1 && s.slot <= 8)).toBe(true)
  })

  it('does not let a project id set the prototype', () => {
    const out = sanitizePlots(JSON.parse('{"__proto__": [{"id": "x", "name": "y"}], "ok": [{"id": "a", "name": "A"}]}'))
    expect(Object.keys(out)).toEqual(['ok'])
    expect(Object.getPrototypeOf(out)).toBe(Object.prototype)
    expect(({} as Record<string, unknown>).series).toBeUndefined()
  })

  it('survives anything that is not an object', () => {
    for (const bad of [null, undefined, 3, 'x', [], true]) expect(sanitizePlots(bad)).toEqual({})
  })

  it('keeps no more plots than a project may have', () => {
    const many = Array.from({ length: MAX_PLOTS_PER_PROJECT + 10 }, (_, i) => ({ id: `p${i}`, name: `P${i}`, series: [] }))
    expect(sanitizePlots({ proj: many }).proj).toHaveLength(MAX_PLOTS_PER_PROJECT)
  })
})

const tick = () => new Promise((r) => setTimeout(r, 0))
const stored = (name: string, id = name.toLowerCase()): PlotConfig => ({ id, name, series: [{ expression: 'x', slot: 2 }], windowMs: 30_000, scale: 'shared' })

/** A server that remembers what it is sent, as the real one does. */
const server = new Map<string, PlotConfig>()

beforeEach(() => {
  vi.clearAllMocks()
  server.clear()
  usePlots.setState({ byProject: {}, loaded: {} })
  api.plotsList.mockImplementation(async () => ({ plots: [...server.values()] }))
  api.plotPut.mockImplementation(async (_pid: string, p: PlotConfig) => (server.set(p.id, p), p))
  api.plotDelete.mockImplementation(async (_pid: string, id: string) => (server.delete(id), { ok: true }))
  vi.stubGlobal('localStorage', undefined)
})

describe('the store, kept by the server', () => {
  it('loads a project\'s plots once, makes sense of what comes back, and shares one request between callers', async () => {
    api.plotsList.mockResolvedValue({ plots: [stored('Motor'), { id: 'bad', name: 5, series: 'no', windowMs: 1 }, 'junk'] })
    await Promise.all([usePlots.getState().load('p'), usePlots.getState().load('p')])
    expect(api.plotsList).toHaveBeenCalledTimes(1)
    expect(usePlots.getState().byProject.p.map((p) => p.name)).toEqual(['Motor', 'Plot'])
    expect(usePlots.getState().loaded.p).toBe(true)
    await usePlots.getState().load('p')
    expect(api.plotsList).toHaveBeenCalledTimes(1)
  })

  it('stays unloaded when the server cannot be reached, and tries again next time', async () => {
    api.plotsList.mockRejectedValueOnce(new Error('offline'))
    await usePlots.getState().load('p')
    expect(usePlots.getState().loaded.p).toBeUndefined()
    await usePlots.getState().load('p')
    expect(usePlots.getState().loaded.p).toBe(true)
  })

  it('writes every edit to the server at once, one plot at a time, and shows it before the answer', async () => {
    const a = usePlots.getState().create('p', { expressions: ['rpm', 'temp'] })!
    expect(usePlots.getState().byProject.p).toEqual([a]) // shown now
    await tick()
    expect(api.plotPut).toHaveBeenLastCalledWith('p', a)
    const s = usePlots.getState()
    expect(s.addSeries('p', a.id, 'adc')).toBe('added')
    expect(s.addSeries('p', a.id, 'adc')).toBe('exists')
    expect(s.addSeries('p', 'nope', 'adc')).toBe('missing')
    s.setWindow('p', a.id, 60_000)
    s.setWindow('p', a.id, 1234) // not one of the spans: nothing to send
    s.setScale('p', a.id, 'normalized')
    s.rename('p', a.id, '  Motor  ')
    s.rename('p', a.id, '   ')
    s.toggleSeries('p', a.id, 'temp')
    s.removeSeries('p', a.id, 'rpm')
    await tick()
    const last = api.plotPut.mock.calls.at(-1)![1] as PlotConfig
    expect(last).toMatchObject({ id: a.id, name: 'Motor', windowMs: 60_000, scale: 'normalized' })
    expect(last.series.map((x) => [x.expression, x.slot, !!x.hidden])).toEqual([['temp', 2, true], ['adc', 3, false]])
    expect(api.plotPut).toHaveBeenCalledTimes(1 + 6) // create, add, window, scale, rename, toggle, remove: 1234 and the blank name sent nothing
    const copy = s.duplicate('p', a.id)!
    expect(copy.name).toBe('Motor copy')
    s.remove('p', a.id)
    await tick()
    expect(api.plotDelete).toHaveBeenCalledWith('p', a.id)
    expect(usePlots.getState().byProject.p.map((p) => p.id)).toEqual([copy.id])
    s.remove('p', 'nope')
    expect(api.plotDelete).toHaveBeenCalledTimes(1)
  })

  it('takes what the server announces, but not while its own writes are on the way', async () => {
    usePlots.getState().apply('p', [stored('Theirs')])
    expect(usePlots.getState().byProject.p.map((p) => p.name)).toEqual(['Theirs'])
    expect(usePlots.getState().loaded.p).toBe(true)
    let finish!: () => void
    api.plotPut.mockImplementationOnce((_p: string, plot: PlotConfig) => new Promise((r) => (finish = () => r(plot))))
    const mine = usePlots.getState().create('p', { name: 'Mine' })!
    await tick()
    // The echo of our own write (and anything else) waits until the write is done: it would flicker the screen back.
    usePlots.getState().apply('p', [stored('Theirs')])
    expect(usePlots.getState().byProject.p.map((p) => p.name)).toEqual(['Theirs', 'Mine'])
    api.plotsList.mockResolvedValue({ plots: [stored('Theirs'), mine] })
    finish()
    await tick()
    await tick()
    expect(api.plotsList).toHaveBeenCalledTimes(1) // settled on the server's list once the write was done
    expect(usePlots.getState().byProject.p.map((p) => p.name)).toEqual(['Theirs', 'Mine'])
  })

  it('says so when the server refuses, and shows what it kept', async () => {
    api.plotPut.mockRejectedValueOnce(new Error('a plot name of 1 to 60 characters'))
    api.plotsList.mockResolvedValue({ plots: [] })
    usePlots.getState().create('p', { name: 'x' })
    await tick()
    await tick()
    expect(shell.toastError).toHaveBeenCalledWith(expect.any(Error), 'Could not save the plot')
    expect(usePlots.getState().byProject.p).toEqual([]) // the refused plot is gone from the screen
  })

  it('treats a plot as the server\'s own: nothing is created past what a project keeps', () => {
    for (let i = 0; i < 40; i++) expect(usePlots.getState().create('p', { name: `P${i}` })).not.toBeNull()
    expect(usePlots.getState().create('p')).toBeNull()
    expect(usePlots.getState().duplicate('p', usePlots.getState().byProject.p[0].id)).toBeNull()
  })

  it('fetches again after a reconnect, only what was loaded', async () => {
    await usePlots.getState().load('a')
    api.plotsList.mockClear()
    usePlots.getState().reloadAll()
    await tick()
    expect(api.plotsList).toHaveBeenCalledTimes(1)
    expect(api.plotsList).toHaveBeenCalledWith('a')
  })
})

describe('plots that an earlier version kept in the browser', () => {
  const legacy = (byProject: Record<string, PlotConfig[]>) => {
    const mem = new Map<string, string>([['wb.debug.plots.v1', JSON.stringify({ state: { byProject }, version: 0 })]])
    vi.stubGlobal('localStorage', { getItem: (k: string) => mem.get(k) ?? null, setItem: (k: string, v: string) => void mem.set(k, v), removeItem: (k: string) => void mem.delete(k) })
    return mem
  }

  it('move to the server when it has none for the project, and are forgotten here (the other projects\' stay)', async () => {
    const mem = legacy({ p: [stored('Old one'), stored('Old two')], q: [stored('Other project')] })
    await usePlots.getState().load('p')
    expect(api.plotPut.mock.calls.map((c) => (c[1] as PlotConfig).name)).toEqual(['Old one', 'Old two'])
    const left = JSON.parse(mem.get('wb.debug.plots.v1')!).state.byProject
    expect(Object.keys(left)).toEqual(['q'])
  })

  it('stay put when the server already has plots, or when sending them failed', async () => {
    const mem = legacy({ p: [stored('Old')] })
    api.plotsList.mockResolvedValue({ plots: [stored('Server')] })
    await usePlots.getState().load('p')
    expect(api.plotPut).not.toHaveBeenCalled()
    expect(JSON.parse(mem.get('wb.debug.plots.v1')!).state.byProject.p).toHaveLength(1)

    usePlots.setState({ byProject: {}, loaded: {} })
    api.plotsList.mockResolvedValue({ plots: [] })
    api.plotPut.mockRejectedValue(new Error('offline'))
    await usePlots.getState().load('p')
    expect(JSON.parse(mem.get('wb.debug.plots.v1')!).state.byProject.p).toHaveLength(1)
  })

  it('are ignored when the browser has none or keeps garbage', async () => {
    await usePlots.getState().load('p')
    vi.stubGlobal('localStorage', { getItem: () => '{not json', setItem: () => undefined, removeItem: () => undefined })
    usePlots.setState({ byProject: {}, loaded: {} })
    await usePlots.getState().load('p')
    expect(api.plotPut).not.toHaveBeenCalled()
  })
})
