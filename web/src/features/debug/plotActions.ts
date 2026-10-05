// What the plot viewer's entry points share: opening a plot, making one, and putting a watched
// expression into one (watching it first when the running session does not). The Live tab, the palette
// and the viewer itself all go through these.

import { openPanel, toast, toastError } from '@/shell/actions'
import { debugApi } from './api'
import { refreshLive } from './liveSnapshot'
import { useLive } from './liveStore'
import { PLOT_SLOTS } from './plotMath'
import { MAX_PLOTS_PER_PROJECT, usePlots, type PlotConfig } from './plotStore'
import type { DebugSession, LiveItem } from './types'

export const plotPanelId = (projectId: string, plotId: string) => `debug.plot:${projectId}:${plotId}`

export function openPlot(projectId: string, plot: Pick<PlotConfig, 'id' | 'name'>) {
  openPanel({ kind: 'debug.plot', id: plotPanelId(projectId, plot.id), title: plot.name, params: { projectId, plotId: plot.id } })
}

/** Only numbers, booleans, pointers and enums can be drawn; bytes and what could not be resolved cannot. */
export const plottable = (item: LiveItem): boolean => item.kind !== 'bytes' && !item.error && item.address !== undefined

/** A new plot, opened. Null (and a message) when the project keeps as many as it may. */
export function newPlot(projectId: string, init?: { name?: string; expressions?: string[] }): PlotConfig | null {
  const plot = usePlots.getState().create(projectId, init)
  if (!plot) {
    toast('error', `A project keeps up to ${MAX_PLOTS_PER_PROJECT} plots`)
    return null
  }
  openPlot(projectId, plot)
  return plot
}

/** A new plot of everything the session watches that can be drawn (the first eight, which is what the colours allow). */
export function newPlotFromWatched(s: DebugSession): PlotConfig | null {
  const items = (useLive.getState().sessions[s.id]?.items ?? []).filter(plottable)
  if (!items.length) {
    toast('info', 'Nothing watched can be drawn yet', { timeout: 2500 })
    return null
  }
  if (items.length > PLOT_SLOTS) toast('info', `A plot draws up to ${PLOT_SLOTS} series: the first ${PLOT_SLOTS} are in it`, { timeout: 3500 })
  return newPlot(s.projectId, { expressions: items.map((i) => i.expression) })
}

/**
 * Watch an expression in the session (Live Watch resolves it and starts reading it) and hand back the item.
 * Null, with the reason shown, when the debugger cannot watch it or it cannot be drawn.
 */
export async function watchForPlot(s: DebugSession, expression: string): Promise<LiveItem | null> {
  const find = () => useLive.getState().sessions[s.id]?.items.find((i) => i.expression === expression)
  let known = find()
  // The store may not have the list yet, or be behind it. The server's list decides whether the watch is new: adding an
  // expression that is already watched answers with the existing item, and that one must never be removed below.
  let listed = false
  if (!known) {
    try {
      await refreshLive(s.projectId, s.id)
      listed = true
      known = find()
    } catch {
      // The add below reports what is wrong.
    }
  }
  if (known) {
    if (!plottable(known)) {
      toast('error', known.error ? `${expression}: ${known.error}` : `${expression} holds ${known.typeName}: only numbers can be drawn`)
      return null
    }
    return known
  }
  try {
    const item = await debugApi.liveAdd(s.projectId, s.id, expression)
    await refreshLive(s.projectId, s.id)
    if (!plottable(item)) {
      // Do not leave a watch behind that the plot cannot use (only one made here: without the server's list we cannot tell).
      if (listed) {
        await debugApi.liveRemove(s.projectId, s.id, item.id).catch(() => {})
        await refreshLive(s.projectId, s.id).catch(() => {})
      }
      toast('error', item.error ? `${expression}: ${item.error}` : `${expression} holds ${item.typeName}: only numbers can be drawn`)
      return null
    }
    return item
  } catch (e) {
    toastError(e, `Could not watch ${expression}`)
    return null
  }
}

/**
 * Make `expression` a series of the plot. With a live session it is watched first, so it has readings at once;
 * without one it is only added (it draws when a session watches it).
 */
export async function addSeriesTo(projectId: string, plotId: string, expression: string, session: DebugSession | null): Promise<boolean> {
  const wanted = expression.trim()
  const plot = usePlots.getState().byProject[projectId]?.find((p) => p.id === plotId)
  if (!wanted || !plot) return false
  if (plot.series.some((s) => s.expression === wanted)) {
    toast('info', `${wanted} is already in ${plot.name}`, { timeout: 2000 })
    return false
  }
  if (plot.series.length >= PLOT_SLOTS) {
    toast('error', `A plot draws up to ${PLOT_SLOTS} series: make another plot for ${wanted}`)
    return false
  }
  let name = wanted
  if (session?.live) {
    const item = await watchForPlot(session, wanted)
    if (!item) return false
    name = item.expression
  }
  // The plot can have changed while the watch was being made.
  const result = usePlots.getState().addSeries(projectId, plotId, name)
  if (result === 'exists') toast('info', `${name} is already in ${plot.name}`, { timeout: 2000 })
  else if (result === 'full') toast('error', `${plot.name} already draws ${PLOT_SLOTS} series: ${name} is watched, but not in it`)
  else if (result === 'missing') toast('info', `${plot.name} was deleted: ${name} is watched, but not in a plot`)
  return result === 'added'
}
