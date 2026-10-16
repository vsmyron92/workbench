// The data slice's client state: per project the series, plots and followed files, kept current
// by events; and the linked view that plots which are linked share. Samples are not kept here:
// a plot asks the server for the window it draws (`dataApi.view`), so a reload, a second browser
// and a phone all see the same thing and nothing large lives in the page.

import { useEffect } from 'react'
import { create } from 'zustand'
import { toastError } from '@/shell/actions'
import { subscribe } from '@/api/events'
import { dataApi } from './api'
import type { PlotConfig, SeriesInfo, Snapshot, SourceInfo } from './types'

export interface ProjectData {
  loaded: boolean
  error: string | null
  series: SeriesInfo[]
  plots: PlotConfig[]
  sources: SourceInfo[]
}

const EMPTY: ProjectData = { loaded: false, error: null, series: [], plots: [], sources: [] }

interface State {
  byProject: Record<string, ProjectData>
  /** Bumped whenever samples of a series changed: a view of it is stale. */
  changed: Record<string, Record<string, number>>
}

export const useData = create<State>(() => ({ byProject: {}, changed: {} }))

const loading = new Set<string>()

function patch(pid: string, f: (p: ProjectData) => Partial<ProjectData>) {
  useData.setState((s) => {
    const cur = s.byProject[pid] ?? EMPTY
    return { byProject: { ...s.byProject, [pid]: { ...cur, ...f(cur) } } }
  })
}

function bump(pid: string, names: string[]) {
  if (!names.length) return
  useData.setState((s) => {
    const cur = { ...s.changed[pid] }
    for (const n of names) cur[n] = (cur[n] ?? 0) + 1
    return { changed: { ...s.changed, [pid]: cur } }
  })
}

function apply(pid: string, snap: Snapshot) {
  patch(pid, () => ({ loaded: true, error: null, series: snap.series, plots: snap.plots, sources: snap.sources }))
  bump(pid, snap.series.map((s) => s.name))
}

/** Load a project's data once (and again after `force`). */
export async function loadData(pid: string, force = false): Promise<void> {
  if (loading.has(pid) || (!force && useData.getState().byProject[pid]?.loaded)) return
  loading.add(pid)
  try {
    apply(pid, await dataApi.snapshot(pid))
  } catch (e) {
    patch(pid, () => ({ error: e instanceof Error ? e.message : String(e) }))
  } finally {
    loading.delete(pid)
  }
}

/** A project's data, loaded on first use. */
export function useProjectData(pid: string | null): ProjectData {
  useEffect(() => {
    if (pid) void loadData(pid)
  }, [pid])
  return useData((s) => (pid ? (s.byProject[pid] ?? EMPTY) : EMPTY))
}

/** A number that changes whenever any of `names` got samples (or was cleared). */
export function useChangeTick(pid: string, names: string[]): number {
  return useData((s) => {
    const c = s.changed[pid]
    if (!c) return 0
    let n = 0
    for (const name of names) n += c[name] ?? 0
    return n
  })
}

// ---------------------------------------------------------------- events

interface Ev<T> {
  projectId?: string
  data: T
}

let installed = false

/** Wire the server's events into the store (once). */
export function installDataEvents(): () => void {
  if (installed) return () => {}
  installed = true
  const known = (pid: string | undefined) => !!pid && !!useData.getState().byProject[pid]?.loaded
  const offs = [
    subscribe('data.series', (ev) => {
      const e = ev as unknown as Ev<{ projectId: string; series: SeriesInfo[] }>
      const pid = e.data?.projectId
      if (!known(pid)) return
      const before = useData.getState().byProject[pid].series
      patch(pid, () => ({ series: e.data.series }))
      // A clear, a new kind or a new offset makes every view of the series stale.
      const stale = e.data.series.filter((s) => {
        const b = before.find((x) => x.name === s.name)
        return !b || b.generation !== s.generation || b.offset !== s.offset || b.kind !== s.kind || b.count !== s.count
      })
      bump(pid, stale.map((s) => s.name))
    }),
    subscribe('data.changed', (ev) => {
      const e = ev as unknown as Ev<{ projectId: string; series: SeriesInfo[] }>
      const pid = e.data?.projectId
      if (!known(pid)) return
      patch(pid, (p) => {
        const series = p.series.slice()
        for (const s of e.data.series) {
          const i = series.findIndex((x) => x.name === s.name)
          if (i >= 0) series[i] = s
          else series.push(s)
        }
        return { series }
      })
      bump(pid, e.data.series.map((s) => s.name))
    }),
    subscribe('data.plots', (ev) => {
      const e = ev as unknown as Ev<{ projectId: string; plots: PlotConfig[] }>
      if (known(e.data?.projectId)) patch(e.data.projectId, () => ({ plots: e.data.plots }))
    }),
    subscribe('data.sources', (ev) => {
      const e = ev as unknown as Ev<{ projectId: string; sources: SourceInfo[] }>
      if (known(e.data?.projectId)) patch(e.data.projectId, () => ({ sources: e.data.sources }))
    }),
    subscribe('resync', () => {
      for (const pid of Object.keys(useData.getState().byProject)) void loadData(pid, true)
    }),
  ]
  return () => {
    offs.forEach((o) => o())
    installed = false
  }
}

// ---------------------------------------------------------------- writes

/** Save a plot: shown at once, written to the server, put back when that fails. */
export async function savePlot(pid: string, plot: PlotConfig): Promise<void> {
  const before = useData.getState().byProject[pid]?.plots ?? []
  patch(pid, (p) => ({ plots: p.plots.some((x) => x.id === plot.id) ? p.plots.map((x) => (x.id === plot.id ? plot : x)) : [...p.plots, plot] }))
  try {
    await dataApi.putPlot(pid, plot)
  } catch (e) {
    patch(pid, () => ({ plots: before }))
    toastError(e)
  }
}

// ---------------------------------------------------------------- linked views

/** What linked plots share: the window in time, or following the newest samples over a span. */
export type LinkedView = { kind: 'follow'; span: number } | { kind: 'fixed'; t0: number; t1: number }

export const useLinked = create<{ views: Record<string, LinkedView | null> }>(() => ({ views: {} }))

export function setLinked(pid: string, v: LinkedView | null) {
  useLinked.setState((s) => ({ views: { ...s.views, [pid]: v } }))
}
