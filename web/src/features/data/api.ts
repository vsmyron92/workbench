// REST of the data slice (server/src/data/routes.rs).

import { api } from '@/api/client'
import type { PlotConfig, RangeStats, SeriesInfo, SeriesView, Snapshot, SourceInfo } from './types'

const enc = encodeURIComponent
const base = (pid: string) => `/api/projects/${enc(pid)}/data`

export const dataApi = {
  snapshot: (pid: string) => api.get<Snapshot>(base(pid)),
  view: (pid: string, series: string[], from: number, to: number, buckets: number, signal?: AbortSignal) =>
    api.post<{ views: Record<string, SeriesView> }>(`${base(pid)}/view`, { series, from, to, buckets }, undefined, signal),
  stats: (pid: string, series: string[], from?: number, to?: number) => api.post<{ stats: Record<string, RangeStats> }>(`${base(pid)}/stats`, { series, from, to }),
  define: (pid: string, body: Partial<SeriesInfo> & { name: string }) => api.put<SeriesInfo>(`${base(pid)}/series`, body),
  clear: (pid: string, names: string[]) => api.post<{ cleared: string[] }>(`${base(pid)}/series/clear`, { names }),
  remove: (pid: string, names: string[]) => api.post<{ deleted: string[] }>(`${base(pid)}/series/delete`, { names }),
  createPlot: (pid: string, body: { title?: string; series?: string[]; span?: number }) => api.post<PlotConfig>(`${base(pid)}/plots`, body),
  putPlot: (pid: string, plot: PlotConfig) => api.put<PlotConfig>(`${base(pid)}/plots/${enc(plot.id)}`, plot),
  deletePlot: (pid: string, id: string) => api.del<{ ok: boolean }>(`${base(pid)}/plots/${enc(id)}`),
  follow: (pid: string, body: { path: string; follow?: boolean; format?: string; rate?: number }) => api.post<SourceInfo>(`${base(pid)}/sources`, body),
  stopSource: (pid: string, id: string) => api.post<{ ok: boolean }>(`${base(pid)}/sources/${enc(id)}/stop`),
  removeSource: (pid: string, id: string) => api.del<{ ok: boolean }>(`${base(pid)}/sources/${enc(id)}`),
  /** CSV of series over a window, as text (saved by the caller). */
  exportCsv: (pid: string, series: string[], from?: number, to?: number) => api.post<string>(`${base(pid)}/export`, { series, from, to }),
}
