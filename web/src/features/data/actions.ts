// What the viewer's entry points share: opening a plot, making one from series, adding a series
// to a plot (lanes chosen the way the server chooses them), and saving a CSV.

import { openPanel, toast, toastError } from '@/shell/actions'
import { dataApi } from './api'
import type { Lane, PlotConfig, SeriesInfo } from './types'

export const plotPanelId = (projectId: string, plotId: string) => `data.plot:${projectId}:${plotId}`

export function openPlot(projectId: string, plot: Pick<PlotConfig, 'id' | 'title'>) {
  openPanel({ kind: 'data.plot', id: plotPanelId(projectId, plot.id), title: plot.title, params: { projectId, plotId: plot.id } })
}

/** A new plot of `series`, opened. The server chooses the lanes (by unit and kind). */
export async function newPlotFrom(projectId: string, series: string[], title?: string): Promise<PlotConfig | null> {
  if (!series.length) {
    toast('info', 'Select the series to plot first', { timeout: 2500 })
    return null
  }
  try {
    const plot = await dataApi.createPlot(projectId, { title: title ?? (series.length === 1 ? series[0] : 'Plot'), series })
    openPlot(projectId, plot)
    return plot
  } catch (e) {
    toastError(e)
    return null
  }
}

export const MAX_TRACES_PER_LANE = 8
export const MAX_LANES = 32
const COLORS = 8

/** The trace colours in use by a plot's analog traces. */
function usedColors(p: PlotConfig): Set<number> {
  return new Set(p.lanes.flatMap((l) => l.traces.map((t) => t.color)).filter((c) => c > 0))
}

/** What the plot's other series are (kind and unit), for choosing a lane. */
export type InfoOf = (name: string) => Pick<SeriesInfo, 'kind' | 'unit'> | undefined

/** `plot` with `info`'s series added: into a lane of analog series of its unit when there is room, else a lane of its own. */
export function addSeriesTo(plot: PlotConfig, info: Pick<SeriesInfo, 'name' | 'kind' | 'unit'>, infoOf: InfoOf): PlotConfig {
  if (plot.lanes.some((l) => l.traces.some((t) => t.series === info.name))) return plot
  if (plot.lanes.length >= MAX_LANES) return plot
  const used = usedColors(plot)
  const color = info.kind === 'analog' ? ([...Array(COLORS)].map((_, i) => i + 1).find((c) => !used.has(c)) ?? 1) : 0
  const trace = { series: info.name, color, style: '' as const }
  const lanes: Lane[] = plot.lanes.map((l) => ({ ...l, traces: [...l.traces] }))
  if (info.kind === 'analog' && info.unit) {
    const lane = lanes.find(
      (l) =>
        l.traces.length < MAX_TRACES_PER_LANE &&
        l.traces.every((t) => {
          const i = infoOf(t.series)
          return i?.kind === 'analog' && i.unit === info.unit
        }),
    )
    if (lane) {
      lane.traces.push(trace)
      return { ...plot, lanes }
    }
  }
  const fresh: Lane = { title: '', traces: [trace], min: null, max: null, weight: 0 }
  if (info.kind === 'analog') {
    // Analog lanes first, then digital, then events: as the server lays them out.
    const firstOther = lanes.findIndex((l) => l.traces.some((t) => infoOf(t.series)?.kind !== 'analog'))
    lanes.splice(firstOther < 0 ? lanes.length : firstOther, 0, fresh)
  } else lanes.push(fresh)
  return { ...plot, lanes }
}

export function downloadText(text: string, filename: string, mime = 'text/csv') {
  const url = URL.createObjectURL(new Blob([text], { type: `${mime};charset=utf-8` }))
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  a.remove()
  window.setTimeout(() => URL.revokeObjectURL(url), 1000)
}

const safeName = (s: string) => s.replace(/[^A-Za-z0-9._-]+/g, '_').replace(/^_+|_+$/g, '') || 'data'

/** Save series (over a window) as CSV: the browser's download. */
export async function exportSeries(projectId: string, names: string[], from?: number, to?: number, label = 'workbench-data') {
  try {
    const csv = await dataApi.exportCsv(projectId, names, from, to)
    downloadText(typeof csv === 'string' ? csv : String(csv), `${safeName(label)}.csv`)
  } catch (e) {
    toastError(e)
  }
}
