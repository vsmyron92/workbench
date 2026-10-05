// Plot configurations of the plot viewer: which watched expressions are drawn together, in which colours,
// over which time span. The server keeps them per project (`debug/plots`), so every browser and device of the user sees
// the same ones, and tells them when they change (`debug.plots`); this store shows them at once and writes each edit
// back. The expressions themselves are Live Watch's, so a plot only names them and a session that watches them
// supplies the readings. Reducers are pure (and tested); the store only applies them.

import { create } from 'zustand'
import { toastError } from '@/shell/actions'
import { debugApi } from './api'
import { DEFAULT_WINDOW, freeSlot, PLOT_SLOTS, PLOT_WINDOWS } from './plotMath'

export type PlotScale = 'shared' | 'normalized'

/** Where earlier versions kept the plots: in this browser. They move to the server the first time a project is loaded. */
const LEGACY_KEY = 'wb.debug.plots.v1'

export interface PlotSeries {
  expression: string
  /** The colour (`--plot-<slot>`), given when the series is added and never changed: removing another series repaints nothing. */
  slot: number
  /** Hidden from the chart (the legend still lists it); a click on its legend entry toggles it. */
  hidden?: boolean
}

export interface PlotConfig {
  id: string
  name: string
  series: PlotSeries[]
  /** The time span shown, in milliseconds (one of `PLOT_WINDOWS`). */
  windowMs: number
  /** `shared`: one axis in the values' own unit. `normalized`: every series as a percentage of its own range in view, for values of different sizes. */
  scale: PlotScale
}

export const MAX_PLOTS_PER_PROJECT = 40
export const MAX_NAME = 60
export const MAX_EXPRESSION = 300

export type AddResult = 'added' | 'exists' | 'full'

const clean = (s: string, max: number) => s.trim().slice(0, max)

/** "Plot 1", "Plot 2"…: the lowest number not in use. */
export function nextName(plots: readonly PlotConfig[]): string {
  const used = new Set(plots.map((p) => p.name))
  for (let n = 1; ; n++) if (!used.has(`Plot ${n}`)) return `Plot ${n}`
}

export function newId(): string {
  return `p${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`
}

/** `cfg` with the expression added in the first free colour. */
export function withSeries(cfg: PlotConfig, expression: string): { cfg: PlotConfig; result: AddResult } {
  const expr = clean(expression, MAX_EXPRESSION)
  if (!expr || cfg.series.some((s) => s.expression === expr)) return { cfg, result: 'exists' }
  const slot = freeSlot(cfg.series.map((s) => s.slot))
  if (slot === null) return { cfg, result: 'full' }
  return { cfg: { ...cfg, series: [...cfg.series, { expression: expr, slot }] }, result: 'added' }
}

export function withoutSeries(cfg: PlotConfig, expression: string): PlotConfig {
  return cfg.series.some((s) => s.expression === expression) ? { ...cfg, series: cfg.series.filter((s) => s.expression !== expression) } : cfg
}

export function withToggled(cfg: PlotConfig, expression: string): PlotConfig {
  return { ...cfg, series: cfg.series.map((s) => (s.expression === expression ? { ...s, hidden: s.hidden ? undefined : true } : s)) }
}

/** A copy that is a plot of its own: a new id and name, and the same series in the same colours. */
export function copyOf(cfg: PlotConfig, plots: readonly PlotConfig[]): PlotConfig {
  const used = new Set(plots.map((p) => p.name))
  // The stem leaves room for " copy 999": a long name is cut here, so that a suffix always makes a new name
  // (cutting the whole name afterwards would return the same text for ever once it is long enough).
  const stem = cfg.name.slice(0, MAX_NAME - ' copy 999'.length)
  let name = `${stem} copy`
  for (let n = 2; used.has(name); n++) name = `${stem} copy ${n}`
  return { ...cfg, id: newId(), name, series: cfg.series.map((s) => ({ ...s })) }
}

/** A plot with these expressions (the first eight, the rest do not fit the colours). */
export function makePlot(plots: readonly PlotConfig[], init: { name?: string; expressions?: string[] } = {}): PlotConfig {
  let cfg: PlotConfig = { id: newId(), name: clean(init.name ?? '', MAX_NAME) || nextName(plots), series: [], windowMs: DEFAULT_WINDOW, scale: 'shared' }
  for (const e of init.expressions ?? []) cfg = withSeries(cfg, e).cfg
  return cfg
}

// ---------------------------------------------------------------- what comes back from localStorage

function isObj(x: unknown): x is Record<string, unknown> {
  return typeof x === 'object' && x !== null && !Array.isArray(x)
}

/** A stored plot made safe to use, or null: the storage is the user's own, but a bad edit or an older version must not break the panel. */
function sanitizePlot(raw: unknown): PlotConfig | null {
  if (!isObj(raw) || typeof raw.id !== 'string' || !raw.id || raw.id.length > 40) return null
  const series: PlotSeries[] = []
  const slots = new Set<number>()
  for (const s of Array.isArray(raw.series) ? raw.series : []) {
    if (!isObj(s) || typeof s.expression !== 'string') continue
    const expression = clean(s.expression, MAX_EXPRESSION)
    if (!expression || series.some((x) => x.expression === expression)) continue
    let slot = typeof s.slot === 'number' && Number.isInteger(s.slot) && s.slot >= 1 && s.slot <= PLOT_SLOTS ? s.slot : 0
    if (!slot || slots.has(slot)) slot = freeSlot(slots) ?? 0
    if (!slot) continue
    slots.add(slot)
    series.push({ expression, slot, ...(s.hidden === true ? { hidden: true } : {}) })
  }
  return {
    id: raw.id,
    name: typeof raw.name === 'string' && raw.name.trim() ? clean(raw.name, MAX_NAME) : 'Plot',
    series,
    windowMs: typeof raw.windowMs === 'number' && PLOT_WINDOWS.includes(raw.windowMs) ? raw.windowMs : DEFAULT_WINDOW,
    scale: raw.scale === 'normalized' ? 'normalized' : 'shared',
  }
}

export function sanitizePlots(raw: unknown): Record<string, PlotConfig[]> {
  const out: Record<string, PlotConfig[]> = {}
  if (!isObj(raw)) return out
  for (const [pid, list] of Object.entries(raw)) {
    if (!Array.isArray(list) || pid === '__proto__') continue
    const seen = new Set<string>()
    const plots: PlotConfig[] = []
    for (const p of list) {
      const cfg = sanitizePlot(p)
      if (!cfg || seen.has(cfg.id) || plots.length >= MAX_PLOTS_PER_PROJECT) continue
      seen.add(cfg.id)
      plots.push(cfg)
    }
    if (plots.length) out[pid] = plots
  }
  return out
}

// ---------------------------------------------------------------- the browser's own copy of earlier versions

function legacyPlots(pid: string): PlotConfig[] {
  try {
    const raw = typeof localStorage === 'undefined' ? null : localStorage.getItem(LEGACY_KEY)
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    return sanitizePlots(isObj(parsed) && isObj(parsed.state) ? parsed.state.byProject : undefined)[pid] ?? []
  } catch {
    return []
  }
}

function forgetLegacy(pid: string) {
  try {
    const raw = localStorage.getItem(LEGACY_KEY)
    if (!raw) return
    const parsed = JSON.parse(raw) as { state?: { byProject?: Record<string, unknown> } }
    if (parsed.state?.byProject) delete parsed.state.byProject[pid]
    localStorage.setItem(LEGACY_KEY, JSON.stringify(parsed))
  } catch {
    // Storage that cannot be written leaves the old copy; it is only read again for a project with no plots on the server.
  }
}

// ---------------------------------------------------------------- the store

interface PlotsState {
  byProject: Record<string, PlotConfig[]>
  /** The project's list came from the server (before it, an empty list means "not known yet"). */
  loaded: Record<string, boolean>
  /** Fetch the project's plots once (and bring the ones an earlier version kept in this browser to the server). */
  load: (pid: string) => Promise<void>
  /** The server says the project's plots changed (`debug.plots`). */
  apply: (pid: string, plots: unknown) => void
  /** After a reconnect: fetch again what was loaded. */
  reloadAll: () => void
  /** A new plot; null when the project already has the most it keeps. */
  create: (pid: string, init?: { name?: string; expressions?: string[] }) => PlotConfig | null
  rename: (pid: string, id: string, name: string) => void
  duplicate: (pid: string, id: string) => PlotConfig | null
  remove: (pid: string, id: string) => void
  addSeries: (pid: string, id: string, expression: string) => AddResult | 'missing'
  removeSeries: (pid: string, id: string, expression: string) => void
  toggleSeries: (pid: string, id: string, expression: string) => void
  setWindow: (pid: string, id: string, windowMs: number) => void
  setScale: (pid: string, id: string, scale: PlotScale) => void
}

/** Writes in flight, by project: while there are any, what the server announces is our own echo or about to be, and is not applied. */
const writing = new Map<string, number>()
const loading = new Map<string, Promise<void>>()

export const usePlots = create<PlotsState>()((set, get) => {
  const list = (pid: string) => get().byProject[pid] ?? []
  const put = (pid: string, plots: PlotConfig[]) => set((st) => ({ byProject: { ...st.byProject, [pid]: plots } }))

  /** Fetch the list and show it. */
  const refetch = async (pid: string): Promise<PlotConfig[] | null> => {
    try {
      const r = await debugApi.plotsList(pid)
      const plots = sanitizePlots({ [pid]: r.plots })[pid] ?? []
      set((st) => ({ byProject: { ...st.byProject, [pid]: plots }, loaded: { ...st.loaded, [pid]: true } }))
      return plots
    } catch {
      return null // offline: what is shown stays, and the next load or reconnect tries again
    }
  }

  /** Send a change to the server. A refusal is told, and the list is fetched again so the screen shows what the server kept. */
  const write = (pid: string, send: () => Promise<unknown>) => {
    writing.set(pid, (writing.get(pid) ?? 0) + 1)
    void Promise.resolve()
      .then(send)
      .catch((e) => toastError(e, 'Could not save the plot'))
      .finally(() => {
        const left = (writing.get(pid) ?? 1) - 1
        if (left > 0) return writing.set(pid, left)
        writing.delete(pid)
        void refetch(pid) // settle on the server's list: also what another device changed meanwhile
      })
  }
  const save = (pid: string, plot: PlotConfig) => write(pid, () => debugApi.plotPut(pid, plot))

  /** Change one plot; the changed plot, or null when nothing changed. */
  const edit = (pid: string, id: string, f: (c: PlotConfig) => PlotConfig): PlotConfig | null => {
    const plots = list(pid)
    let changed: PlotConfig | null = null
    const next = plots.map((p) => {
      if (p.id !== id) return p
      const n = f(p)
      if (n !== p) changed = n
      return n
    })
    if (!changed) return null
    put(pid, next)
    save(pid, changed)
    return changed
  }

  return {
    byProject: {},
    loaded: {},
    load: (pid) => {
      if (get().loaded[pid]) return Promise.resolve()
      const running = loading.get(pid)
      if (running) return running
      const run = (async () => {
        const plots = await refetch(pid)
        // A project with no plots on the server, in a browser that kept some the old way: move them over.
        if (plots && !plots.length) {
          const old = legacyPlots(pid)
          if (old.length) {
            const sent = await Promise.allSettled(old.map((p) => debugApi.plotPut(pid, p)))
            if (sent.every((r) => r.status === 'fulfilled')) forgetLegacy(pid)
            await refetch(pid)
          }
        }
      })().finally(() => loading.delete(pid))
      loading.set(pid, run)
      return run
    },
    apply: (pid, plots) => {
      if (writing.get(pid)) return
      put(pid, sanitizePlots({ [pid]: plots })[pid] ?? [])
      set((st) => ({ loaded: { ...st.loaded, [pid]: true } }))
    },
    reloadAll: () => {
      for (const pid of Object.keys(get().loaded)) void refetch(pid)
    },
    create: (pid, init) => {
      const plots = list(pid)
      if (plots.length >= MAX_PLOTS_PER_PROJECT) return null
      const cfg = makePlot(plots, init)
      put(pid, [...plots, cfg])
      save(pid, cfg)
      return cfg
    },
    rename: (pid, id, name) => {
      const n = clean(name, MAX_NAME)
      if (n) edit(pid, id, (c) => (c.name === n ? c : { ...c, name: n }))
    },
    duplicate: (pid, id) => {
      const plots = list(pid)
      const from = plots.find((p) => p.id === id)
      if (!from || plots.length >= MAX_PLOTS_PER_PROJECT) return null
      const cfg = copyOf(from, plots)
      put(pid, [...plots, cfg])
      save(pid, cfg)
      return cfg
    },
    remove: (pid, id) => {
      if (!list(pid).some((p) => p.id === id)) return
      put(pid, list(pid).filter((p) => p.id !== id))
      write(pid, () => debugApi.plotDelete(pid, id))
    },
    addSeries: (pid, id, expression) => {
      const cur = list(pid).find((p) => p.id === id)
      if (!cur) return 'missing'
      const { cfg, result } = withSeries(cur, expression)
      if (result === 'added') edit(pid, id, () => cfg)
      return result
    },
    removeSeries: (pid, id, expression) => void edit(pid, id, (c) => withoutSeries(c, expression)),
    toggleSeries: (pid, id, expression) => void edit(pid, id, (c) => withToggled(c, expression)),
    setWindow: (pid, id, windowMs) => {
      if (PLOT_WINDOWS.includes(windowMs)) edit(pid, id, (c) => (c.windowMs === windowMs ? c : { ...c, windowMs }))
    },
    setScale: (pid, id, scale) => void edit(pid, id, (c) => (c.scale === scale ? c : { ...c, scale })),
  }
})

/** The project's plots, in the order they were made. */
export const plotsOf = (pid: string | null | undefined): PlotConfig[] => (pid ? (usePlots.getState().byProject[pid] ?? []) : [])
