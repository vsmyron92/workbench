import { describe, expect, it, vi } from 'vitest'

vi.mock('@/shell/actions', () => ({ openPanel: vi.fn(), toast: vi.fn(), toastError: vi.fn() }))
vi.mock('./api', () => ({ dataApi: {} }))
import { addSeriesTo, plotPanelId, type InfoOf } from './actions'
import type { PlotConfig } from './types'

const plot = (lanes: PlotConfig['lanes'] = []): PlotConfig => ({ id: 'p', title: 'P', lanes, span: 10, follow: true, linked: true, markers: [] })
const lane = (...series: [string, number][]) => ({ title: '', traces: series.map(([s, c]) => ({ series: s, color: c, style: '' as const })), min: null, max: null, weight: 0 })

const infos: Record<string, { kind: 'analog' | 'digital' | 'events'; unit: string }> = {
  I1: { kind: 'analog', unit: 'A' },
  I2: { kind: 'analog', unit: 'A' },
  V: { kind: 'analog', unit: 'V' },
  raw: { kind: 'analog', unit: '' },
  CLK: { kind: 'digital', unit: '' },
  marks: { kind: 'events', unit: '' },
}
const infoOf: InfoOf = (n) => infos[n]
const add = (p: PlotConfig, name: string) => addSeriesTo(p, { name, ...infos[name] }, infoOf)

describe('adding a series to a plot', () => {
  it('joins the lane of analog series with its unit and takes the next colour', () => {
    const p = add(plot([lane(['I1', 1])]), 'I2')
    expect(p.lanes).toHaveLength(1)
    expect(p.lanes[0].traces.map((t) => [t.series, t.color])).toEqual([['I1', 1], ['I2', 2]])
  })

  it('makes a lane of its own for another unit, before the digital lanes', () => {
    const p = add(plot([lane(['I1', 1]), lane(['CLK', 0])]), 'V')
    expect(p.lanes.map((l) => l.traces[0].series)).toEqual(['I1', 'V', 'CLK'])
    expect(p.lanes[1].traces[0].color).toBe(2)
  })

  it('puts digital lines and events in lanes of their own, uncoloured, at the end', () => {
    let p = add(plot([lane(['I1', 1])]), 'CLK')
    p = add(p, 'marks')
    expect(p.lanes.map((l) => l.traces[0].series)).toEqual(['I1', 'CLK', 'marks'])
    expect(p.lanes[1].traces[0].color).toBe(0)
    expect(p.lanes[2].traces[0].color).toBe(0)
  })

  it('does not put unit-less series together: nothing says they are comparable', () => {
    const p = add(plot([lane(['raw', 1])]), 'I1')
    expect(p.lanes).toHaveLength(2)
  })

  it('leaves a plot that has the series alone, and the colours of the others', () => {
    const p = plot([lane(['I1', 3])])
    expect(add(p, 'I1')).toBe(p)
    expect(add(p, 'V').lanes[1].traces[0].color).toBe(1)
  })
})

describe('panel ids', () => {
  it('follow the convention of the panels table', () => {
    expect(plotPanelId('lab', 'power')).toBe('data.plot:lab:power')
  })
})
