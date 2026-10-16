// The `data.plot` panel: lanes of live measurements under one time axis, like an oscilloscope or a
// logic analyzer next to the agent session that is capturing them.
//
// What it draws is asked of the server for the window in view, at the width of the plot, so the
// page never holds a capture: a reload, a second browser and a phone-sized panel all see the same
// thing, and a million-sample capture costs a few thousand points per redraw. A plot is a saved
// layout (`PlotConfig`); the view (where in time, following or not) is the browser's own, unless
// the plot is linked, when linked plots pan and zoom together.
//
// Wheel zooms around the pointer, drag pans, Shift+drag measures a range, double-click follows
// the newest samples again.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type PointerEvent as ReactPointerEvent } from 'react'
import { ChartLine, ChevronsRight, Download, Ellipsis, EyeOff, Link2, Link2Off, Maximize2, Pause, Plus, X } from 'lucide-react'
import { confirmDialog, promptDialog, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { Button, copyText, EmptyState, IconButton, Loading, Select, showMenuAt, type MenuEntry } from '@/ui'
import { addSeriesTo, exportSeries } from './actions'
import { dataApi } from './api'
import { drawPlot, geometryOf, geometryPad, nearestFinite, neededHeight, readTheme, timeAt, xAt, type DrawLane, type DrawModel, type DrawTheme, type Geometry } from './draw'
import { atOrBefore, formatDuration, formatTimeExact, formatValue, isWall, laneKind, SPANS, spanLabel, zoomRange } from './math'
import { savePlot, setLinked, useChangeTick, useLinked, useProjectData, type LinkedView } from './store'
import type { Kind, Lane, PlotConfig, PlotParams, RangeStats, SeriesInfo, SeriesView, Trace } from './types'

/** Where in time the plot looks. */
type View = { follow: true; span: number } | { follow: false; t0: number; t1: number }

const MIN_FETCH_GAP_MS = 60
const FALLBACK_SPAN = 10

function extentOf(names: string[], infos: Map<string, SeriesInfo>): { oldest: number | null; newest: number | null } {
  let oldest = Infinity
  let newest = -Infinity
  for (const n of names) {
    const i = infos.get(n)
    if (!i) continue
    if (i.tStart !== null) oldest = Math.min(oldest, i.tStart)
    if (i.tEnd !== null) newest = Math.max(newest, i.tEnd)
  }
  return { oldest: Number.isFinite(oldest) ? oldest : null, newest: Number.isFinite(newest) ? newest : null }
}

/** The window a view shows, given what the series hold. */
export function rangeOf(v: View, ext: { oldest: number | null; newest: number | null }): [number, number] {
  if (!v.follow) return [v.t0, v.t1]
  const end = ext.newest ?? Date.now() / 1000
  if (v.span > 0) return [end - v.span, end]
  const start = ext.oldest ?? end - FALLBACK_SPAN
  const w = Math.max(end - start, 1e-9)
  return [start, end + w * 0.02]
}

/** What the legend says of a trace: its value under the cursor, or its newest. */
function readout(kind: Kind, view: SeriesView | undefined, info: SeriesInfo | undefined, t: number | null): string {
  if (!info) return 'no such series'
  if (kind === 'events') {
    if (t !== null && view?.events) {
      let last: string | null = null
      for (const [et, text] of view.events) if (et <= t) last = text
      if (last !== null) return last
    }
    return `${info.count} event${info.count === 1 ? '' : 's'}`
  }
  const text = (v: number | null) => {
    if (v === null || !Number.isFinite(v)) return '—'
    if (kind === 'digital') return v > 1 ? `0x${Math.round(v).toString(16).toUpperCase()}` : String(v)
    return formatValue(v, info.unit)
  }
  if (t !== null && view?.t && view.v) {
    if (kind === 'digital') {
      const i = atOrBefore(view.t, t)
      return i < 0 ? '—' : text(view.v[i])
    }
    const i = nearestFinite(view.t, view.v, t)
    return i < 0 ? '—' : text(view.v[i])
  }
  return info.last === null ? '—' : text(info.last)
}

function integralUnit(unit: string): { label: string; unit: string } {
  const m = /^([pnuµμmkMG]?)(A|W)$/.exec(unit.trim())
  if (m) return m[2] === 'A' ? { label: 'Charge', unit: `${m[1]}C` } : { label: 'Energy', unit: `${m[1]}J` }
  return { label: 'Area', unit: unit ? `${unit}·s` : 's' }
}

// ---------------------------------------------------------------- the panel

export function PlotPanel({ params, setTitle, close, visible }: PanelProps<PlotParams>) {
  const { projectId, plotId } = params
  const data = useProjectData(projectId)
  const plot = data.plots.find((p) => p.id === plotId)
  const infos = useMemo(() => new Map(data.series.map((s) => [s.name, s])), [data.series])

  useEffect(() => {
    if (plot) setTitle(plot.title)
  }, [plot, setTitle])

  const traceNames = useMemo(() => (plot ? plot.lanes.flatMap((l) => l.traces.map((t) => t.series)) : []), [plot])
  const markerNames = plot?.markers
  const fetchNames = useMemo(() => {
    const shown = plot ? plot.lanes.flatMap((l) => l.traces.filter((t) => !t.hidden).map((t) => t.series)) : []
    return [...new Set([...shown, ...(markerNames ?? [])])]
  }, [plot, markerNames])
  const ext = useMemo(() => extentOf([...traceNames, ...(markerNames ?? [])], infos), [traceNames, markerNames, infos])

  // The view: this browser's own, or the one linked plots share.
  const [own, setOwn] = useState<View | null>(null)
  const linked = useLinked((s) => s.views[projectId] ?? null)
  const effective: View = useMemo(() => {
    if (plot?.linked && linked) return linked.kind === 'follow' ? { follow: true, span: linked.span } : { follow: false, t0: linked.t0, t1: linked.t1 }
    return own ?? { follow: true, span: plot?.span ?? FALLBACK_SPAN }
  }, [own, linked, plot?.linked, plot?.span])
  // A plot that does not follow starts where its data is, once there is some, and stays there.
  useEffect(() => {
    if (own === null && plot && !plot.follow && ext.oldest !== null && ext.newest !== null) {
      const w = Math.max(ext.newest - ext.oldest, 1e-6)
      setOwn({ follow: false, t0: ext.oldest - w * 0.02, t1: ext.newest + w * 0.02 })
    }
  }, [own, plot, ext.oldest, ext.newest])
  const [t0, t1] = useMemo(() => rangeOf(effective, ext), [effective, ext])
  // A clock when the series in view say so (the server decides: `SeriesInfo.wall`), else by the times themselves.
  const wall = traceNames.some((n) => infos.get(n)?.wall) || isWall(t1)

  const commit = useCallback(
    (v: View) => {
      setOwn(v)
      if (plot?.linked) setLinked(projectId, v.follow ? { kind: 'follow', span: v.span } : ({ kind: 'fixed', t0: v.t0, t1: v.t1 } satisfies LinkedView))
    },
    [plot?.linked, projectId],
  )

  // The chart region: its size, and the theme tokens it sees.
  const [region, setRegion] = useState<HTMLDivElement | null>(null)
  const [size, setSize] = useState({ w: 0, h: 0 })
  useLayoutEffect(() => {
    if (!region) return
    const update = () => setSize({ w: region.clientWidth, h: region.clientHeight })
    update()
    const ro = new ResizeObserver(update)
    ro.observe(region)
    return () => ro.disconnect()
  }, [region])

  // The tokens are read when drawing, not once: a panel's DOM may be built before it is attached
  // (a tab opened in the background), when custom properties resolve to nothing. A theme change redraws.
  const [themeRev, setThemeRev] = useState(0)
  useEffect(() => {
    const mo = new MutationObserver(() => setThemeRev((n) => n + 1))
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'class'] })
    return () => mo.disconnect()
  }, [])

  // The window of every shown series, from the server. One request in flight; the newest wish wins.
  const [views, setViews] = useState<Record<string, SeriesView>>({})
  const tick = useChangeTick(projectId, fetchNames)
  const buckets = Math.max(60, Math.round(size.w - geometryPad.left - geometryPad.right))
  const wanted = useRef<{ names: string[]; t0: number; t1: number; buckets: number } | null>(null)
  const busy = useRef(false)
  const lastStart = useRef(0)
  const alive = useRef(true)
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])
  const pump = useCallback(() => {
    if (busy.current || !wanted.current || !alive.current) return
    const wait = Math.max(0, MIN_FETCH_GAP_MS - (performance.now() - lastStart.current))
    busy.current = true
    window.setTimeout(() => {
      const req = wanted.current
      wanted.current = null
      if (!req || !alive.current) {
        busy.current = false
        return
      }
      lastStart.current = performance.now()
      dataApi
        .view(projectId, req.names, req.t0, req.t1, req.buckets)
        .then((r) => {
          if (alive.current) setViews(r.views)
        })
        .catch(() => undefined)
        .finally(() => {
          busy.current = false
          pump()
        })
    }, wait)
  }, [projectId])
  const fetchKey = fetchNames.join('\u0000')
  useEffect(() => {
    if (!visible || !fetchNames.length || size.w < 60 || !Number.isFinite(t0) || !Number.isFinite(t1)) return
    wanted.current = { names: fetchNames, t0, t1, buckets }
    pump()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId, fetchKey, t0, t1, buckets, tick, visible, size.w > 0, pump])

  // The cursor and a measured range. A cursor the view moved away from is no cursor.
  const [cursorAt, setCursorT] = useState<number | null>(null)
  const cursorT = cursorAt !== null && cursorAt >= t0 && cursorAt <= t1 ? cursorAt : null
  const [selection, setSelection] = useState<[number, number] | null>(null)

  // The drawing's model.
  const model: Omit<DrawModel, 'theme'> | null = useMemo(() => {
    if (!plot || size.w < 60) return null
    const lanes: DrawLane[] = plot.lanes.map((l) => {
      const traces = l.traces.map((t) => {
        const i = infos.get(t.series)
        return {
          name: t.series,
          kind: i?.kind ?? 'analog',
          unit: i?.unit ?? '',
          color: t.color,
          style: t.style,
          hidden: !!t.hidden,
          view: views[t.series],
          end: i?.tEnd ?? null,
        }
      })
      const kind = laneKind(l, (n) => infos.get(n)?.kind)
      const unit = traces.find((t) => t.kind === 'analog' && t.unit)?.unit ?? ''
      return { title: l.title, kind, unit, log: !!l.log, min: l.min, max: l.max, weight: l.weight, traces }
    })
    const anyData = lanes.some((l) => l.traces.some((t) => t.view && ((t.view.t?.length ?? 0) > 0 || (t.view.events?.length ?? 0) > 0)))
    const height = Math.max(size.h, neededHeight(lanes))
    return {
      width: size.w,
      height,
      dpr: window.devicePixelRatio || 1,
      t0,
      t1,
      lanes,
      markers: (plot.markers ?? []).map((n) => ({ name: n, events: views[n]?.events ?? [] })),
      cursorT,
      selection,
      wall,
      empty: anyData ? null : lanes.length ? 'Waiting for data…' : 'This plot has no series yet',
    }
  }, [plot, size.w, size.h, infos, views, t0, t1, cursorT, selection, wall])

  const geom: Geometry | null = useMemo(() => (model ? geometryOf(model) : null), [model])
  const geomRef = useRef<Geometry | null>(null)
  geomRef.current = geom
  const rangeRef = useRef<[number, number]>([t0, t1])
  rangeRef.current = [t0, t1]
  const effectiveRef = useRef(effective)
  effectiveRef.current = effective

  const [canvas, setCanvas] = useState<HTMLCanvasElement | null>(null)
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  canvasRef.current = canvas
  useEffect(() => {
    const c = canvas
    if (!c || !model) return
    const w = Math.round(model.width * model.dpr)
    const h = Math.round(model.height * model.dpr)
    if (c.width !== w) c.width = w
    if (c.height !== h) c.height = h
    const ctx = c.getContext('2d')
    if (!ctx) return
    let raf = 0
    let tries = 0
    const paint = () => {
      const theme: DrawTheme = readTheme(c)
      // Not attached yet (or not styled): try again next frame rather than paint in no colours.
      if (!c.isConnected || !theme.surface) {
        if (tries++ < 60) raf = requestAnimationFrame(paint)
        return
      }
      drawPlot(ctx, { ...model, theme })
    }
    raf = requestAnimationFrame(paint)
    return () => cancelAnimationFrame(raf)
  }, [model, canvas, themeRev])

  // ------------------------------------------------------------ interactions

  const drag = useRef<{ mode: 'pan' | 'select'; startX: number; t0: number; t1: number; at: number; moved: boolean } | null>(null)
  const [dragging, setDragging] = useState(false)

  const timeFromEvent = (clientX: number): number | null => {
    const c = canvasRef.current
    const g = geomRef.current
    if (!c || !g) return null
    const r = c.getBoundingClientRect()
    return timeAt(g, rangeRef.current[0], rangeRef.current[1], clientX - r.left)
  }

  const fixedWindow = (a: number, b: number): View => {
    // Never narrower than what a double can tell apart at these times: a few tens of microseconds on a
    // clock (Unix seconds), a nanosecond on an instrument's own time.
    const width = Math.max(b - a, 1e-9, Math.abs(b) * 5e-14)
    const lo = ext.oldest ?? a
    const hi = ext.newest ?? b
    // Not so far out that the data is out of sight.
    const span = Math.max(hi - lo, 1e-6)
    const cap = Math.max(span * 50, 1)
    const w = Math.min(width, cap)
    let s = a
    if (s + w < lo - w) s = lo - w
    if (s > hi + w) s = hi + w
    return { follow: false, t0: s, t1: s + w }
  }

  const onPointerDown = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    if (e.button !== 0) return
    const g = geomRef.current
    const t = timeFromEvent(e.clientX)
    if (!g || t === null) return
    const r = e.currentTarget.getBoundingClientRect()
    if (e.clientX - r.left < g.x0 || e.clientX - r.left > g.x1) return
    e.currentTarget.setPointerCapture(e.pointerId)
    e.currentTarget.focus({ focusVisible: false } as FocusOptions)
    drag.current = { mode: e.shiftKey ? 'select' : 'pan', startX: e.clientX, t0: rangeRef.current[0], t1: rangeRef.current[1], at: t, moved: false }
    if (e.shiftKey) setSelection([t, t])
    else setDragging(true)
  }

  const onPointerMove = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    const g = geomRef.current
    const d = drag.current
    if (d && g) {
      const per = (d.t1 - d.t0) / (g.x1 - g.x0)
      if (Math.abs(e.clientX - d.startX) > 2) d.moved = true
      if (d.mode === 'pan' && d.moved) {
        const dt = -(e.clientX - d.startX) * per
        commit(fixedWindow(d.t0 + dt, d.t1 + dt))
      } else if (d.mode === 'select') {
        const t = d.t0 + ((e.clientX - e.currentTarget.getBoundingClientRect().left - g.x0) / (g.x1 - g.x0)) * (d.t1 - d.t0)
        setSelection([d.at, t])
      }
      return
    }
    const t = timeFromEvent(e.clientX)
    const r = e.currentTarget.getBoundingClientRect()
    const x = e.clientX - r.left
    setCursorT(g && t !== null && x >= g.x0 && x <= g.x1 ? t : null)
  }

  const onPointerUp = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    const d = drag.current
    drag.current = null
    setDragging(false)
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId)
    if (d?.mode === 'select') {
      setSelection((s) => (s && Math.abs(s[1] - s[0]) > 0 && d.moved ? ([Math.min(s[0], s[1]), Math.max(s[0], s[1])] as [number, number]) : null))
    } else if (d && !d.moved) {
      // A click that did not drag clears a measured range.
      setSelection(null)
    }
  }

  // Wheel: native, so it can stop the page from scrolling.
  useEffect(() => {
    const c = canvas
    if (!c) return
    const onWheel = (e: WheelEvent) => {
      const g = geomRef.current
      if (!g) return
      const r = c.getBoundingClientRect()
      const x = e.clientX - r.left
      if (x < g.x0 || x > g.x1) return
      e.preventDefault()
      const [a, b] = rangeRef.current
      if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
        const dt = ((e.deltaX || e.deltaY) / (g.x1 - g.x0)) * (b - a)
        commit(fixedWindow(a + dt, b + dt))
        return
      }
      const anchor = timeAt(g, a, b, x)
      const [na, nb] = zoomRange(a, b, anchor, Math.exp(e.deltaY * 0.0015))
      commit(fixedWindow(na, nb))
    }
    c.addEventListener('wheel', onWheel, { passive: false })
    return () => c.removeEventListener('wheel', onWheel)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canvas, commit, ext.oldest, ext.newest])

  const follow = useCallback(() => {
    setSelection(null)
    commit({ follow: true, span: plot?.span ?? FALLBACK_SPAN })
  }, [commit, plot?.span])

  const fit = useCallback(() => {
    if (ext.oldest === null || ext.newest === null) return
    const w = Math.max(ext.newest - ext.oldest, 1e-6)
    commit({ follow: false, t0: ext.oldest - w * 0.02, t1: ext.newest + w * 0.02 })
  }, [commit, ext.oldest, ext.newest])

  const onKeyDown = (e: ReactKeyboardEvent<HTMLCanvasElement>) => {
    const [a, b] = rangeRef.current
    const w = b - a
    const mid = (a + b) / 2
    switch (e.key) {
      case '+':
      case '=':
        commit(fixedWindow(...zoomRange(a, b, cursorT ?? mid, 0.5)))
        break
      case '-':
        commit(fixedWindow(...zoomRange(a, b, cursorT ?? mid, 2)))
        break
      case 'ArrowLeft':
        commit(fixedWindow(a - w * 0.1, b - w * 0.1))
        break
      case 'ArrowRight':
        commit(fixedWindow(a + w * 0.1, b + w * 0.1))
        break
      case 'PageUp':
        commit(fixedWindow(a - w * 0.9, b - w * 0.9))
        break
      case 'PageDown':
        commit(fixedWindow(a + w * 0.9, b + w * 0.9))
        break
      case 'Home':
        if (ext.oldest !== null) commit(fixedWindow(ext.oldest, ext.oldest + w))
        break
      case 'End':
        follow()
        break
      case 'f':
      case 'F':
        fit()
        break
      case 'Escape':
        setSelection(null)
        break
      default:
        return
    }
    e.preventDefault()
  }

  // ------------------------------------------------------------ editing the layout

  const update = useCallback(
    (f: (p: PlotConfig) => PlotConfig) => {
      if (plot) void savePlot(projectId, f(plot))
    },
    [plot, projectId],
  )

  const setSpan = (span: number) => {
    update((p) => ({ ...p, span }))
    if (effective.follow) commit({ follow: true, span })
    else if (span > 0) commit(fixedWindow(t1 - span, t1))
    else fit()
  }

  const addMenu = (el: HTMLElement) => {
    if (!plot) return
    const have = new Set(traceNames)
    const rest = data.series.filter((s) => !have.has(s.name))
    if (!rest.length) {
      showMenuAt(el, [{ label: 'Every series is in this plot', disabled: true, run: () => undefined }])
      return
    }
    const items: MenuEntry[] = rest.slice(0, 40).map((s) => ({
      label: `${s.name}${s.unit ? ` [${s.unit}]` : ''}`,
      run: () => update((p) => addSeriesTo(p, s, (n) => infos.get(n))),
    }))
    if (rest.length > 40) items.push({ label: `${rest.length - 40} more: use the Data window`, disabled: true, run: () => undefined })
    showMenuAt(el, items)
  }

  const laneMenu = (el: HTMLElement, li: number) => {
    if (!plot) return
    const lane = plot.lanes[li]
    const kind = laneKind(lane, (n) => infos.get(n)?.kind)
    const edit = (f: (l: Lane) => Lane) => update((p) => ({ ...p, lanes: p.lanes.map((l, i) => (i === li ? f(l) : l)) }))
    const items: MenuEntry[] = []
    if (kind === 'analog') {
      items.push({ label: lane.log ? 'Linear scale' : 'Logarithmic scale', run: () => edit((l) => ({ ...l, log: !l.log, min: null, max: null })) })
      items.push({
        label: 'Set range…',
        run: () => {
          void promptDialog({ title: 'Lane range', label: 'Lowest and highest value, in the unit shown (for example 0, 3.3)', initial: lane.min !== null && lane.max !== null ? `${lane.min}, ${lane.max}` : '', placeholder: '0, 3.3' }).then((v) => {
            if (v === null) return
            const parts = v.split(/[,;\s]+/).filter(Boolean).map(Number)
            if (parts.length === 2 && parts.every(Number.isFinite) && parts[0] < parts[1]) edit((l) => ({ ...l, min: parts[0], max: parts[1] }))
            else if (!v.trim()) edit((l) => ({ ...l, min: null, max: null }))
            else toastError(new Error('Two numbers, the lower first: 0, 3.3'))
          })
        },
      })
      if (lane.min !== null || lane.max !== null) items.push({ label: 'Fit the range to the data', run: () => edit((l) => ({ ...l, min: null, max: null })) })
      items.push({ label: 'Rename lane…', run: () => void promptDialog({ title: 'Lane title', initial: lane.title }).then((v) => v !== null && edit((l) => ({ ...l, title: v.trim() }))) })
      items.push('separator')
    }
    if (li > 0) items.push({ label: 'Move up', run: () => update((p) => swap(p, li, li - 1)) })
    if (li < plot.lanes.length - 1) items.push({ label: 'Move down', run: () => update((p) => swap(p, li, li + 1)) })
    items.push({ label: 'Remove lane', danger: true, run: () => update((p) => ({ ...p, lanes: p.lanes.filter((_, i) => i !== li) })) })
    showMenuAt(el, items)
  }

  const moreMenu = (el: HTMLElement) => {
    if (!plot) return
    showMenuAt(el, [
      { label: 'Rename plot…', run: () => void promptDialog({ title: 'Plot title', initial: plot.title }).then((v) => v && update((p) => ({ ...p, title: v.trim() }))) },
      { label: 'Save visible range as CSV', run: () => void exportSeries(projectId, traceNames, t0, t1, plot.title) },
      { label: 'Save all as CSV', run: () => void exportSeries(projectId, traceNames, undefined, undefined, plot.title) },
      'separator',
      {
        label: 'Delete plot',
        danger: true,
        run: () =>
          void confirmDialog({ title: `Delete plot “${plot.title}”?`, message: 'The measurements stay: only this layout goes.', confirmLabel: 'Delete', danger: true }).then((ok) => {
            if (!ok) return
            void dataApi.deletePlot(projectId, plot.id).then(close).catch(toastError)
          }),
      },
    ])
  }

  // ------------------------------------------------------------ measured range

  const [stats, setStats] = useState<Record<string, RangeStats> | null>(null)
  const selKey = selection ? `${selection[0]}:${selection[1]}` : ''
  useEffect(() => {
    if (!selection || selection[0] === selection[1] || !fetchNames.length) {
      setStats(null)
      return
    }
    const [a, b] = [Math.min(...selection), Math.max(...selection)]
    const h = window.setTimeout(() => {
      dataApi
        .stats(projectId, traceNames, a, b)
        .then((r) => setStats(r.stats))
        .catch(() => setStats(null))
    }, 120)
    return () => window.clearTimeout(h)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selKey, projectId, traceNames.join('\u0000'), tick])

  // ------------------------------------------------------------ render

  if (!plot) {
    if (!data.loaded) return <Loading />
    return (
      <EmptyState icon={ChartLine} title="This plot is gone" action={<Button onClick={close}>Close</Button>}>
        It was deleted. The plots of a project are listed in the Data window.
      </EmptyState>
    )
  }

  const following = effective.follow
  const hasData = ext.newest !== null
  return (
    <div className="wb-data-plot">
      <div className="wb-data-bar">
        <button type="button" className="title" title="Rename" onClick={() => void promptDialog({ title: 'Plot title', initial: plot.title }).then((v) => v && update((p) => ({ ...p, title: v.trim() })))}>
          {plot.title}
        </button>
        <Button size="small" variant={following ? 'primary' : 'default'} icon={following ? ChevronsRight : Pause} onClick={follow} title={following ? 'Following the newest samples' : 'Paused: click to follow the newest samples (End)'}>
          {following ? 'Following' : 'Follow'}
        </Button>
        <Select
          value={following ? String(effective.span) : ''}
          onChange={(e) => setSpan(Number(e.target.value))}
          title="Time in view while following"
          aria-label="Time in view"
        >
          {!following && <option value="">{formatDuration(t1 - t0)}</option>}
          {SPANS.map((s) => (
            <option key={s} value={String(s)}>
              {spanLabel(s)}
            </option>
          ))}
          {following && !(SPANS as readonly number[]).includes(effective.span) && <option value={String(effective.span)}>{formatDuration(effective.span)}</option>}
        </Select>
        <IconButton size="small" icon={Maximize2} label="Fit everything captured (F)" onClick={fit} disabled={!hasData} />
        <IconButton
          size="small"
          icon={plot.linked ? Link2 : Link2Off}
          active={plot.linked}
          label={plot.linked ? 'Linked: pans and zooms together with the project’s other linked plots' : 'Not linked: click to pan and zoom together with other plots'}
          onClick={() => update((p) => ({ ...p, linked: !p.linked }))}
        />
        <span className="grow" />
        <IconButton size="small" icon={Plus} label="Add a series to this plot" onClick={(e) => addMenu(e.currentTarget)} />
        <IconButton size="small" icon={Download} label="Save as CSV" onClick={() => void exportSeries(projectId, traceNames, t0, t1, plot.title)} disabled={!traceNames.length} />
        <IconButton size="small" icon={Ellipsis} label="More" onClick={(e) => moreMenu(e.currentTarget)} />
      </div>

      <Legend plot={plot} infos={infos} views={views} cursorT={cursorT} update={update} />

      <div className="wb-data-region" ref={setRegion}>
        {model && geom && (
          <div className="inner" style={{ width: model.width, height: model.height }}>
            <canvas
              ref={setCanvas}
              className={dragging ? 'dragging' : undefined}
              style={{ width: model.width, height: model.height }}
              tabIndex={0}
              role="img"
              aria-label={`${plot.title}: ${plot.lanes.length} lanes. Wheel zooms, drag pans, Shift+drag measures a range, double-click follows the newest samples.`}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
              onPointerLeave={() => !drag.current && setCursorT(null)}
              onDoubleClick={follow}
              onKeyDown={onKeyDown}
            />
            {plot.lanes.map((l, i) => (
              <button
                key={i}
                type="button"
                className="wb-data-lanebtn"
                style={{ left: geom.x1 - 22, top: geom.lanes[i].box.y0 + 2 }}
                title="Lane options"
                aria-label={`Options of lane ${l.title || l.traces.map((t) => t.series).join(', ')}`}
                onClick={(e) => laneMenu(e.currentTarget, i)}
              >
                <Ellipsis size={13} />
              </button>
            ))}
            {cursorT !== null && <CursorTip t={cursorT} x={xAt(geom, t0, t1, cursorT)} wall={wall} width={size.w} />}
          </div>
        )}
      </div>

      <OverviewBar ext={ext} t0={t0} t1={t1} following={following} onMove={(a) => commit(fixedWindow(a, a + (t1 - t0)))} onFollow={follow} />

      {selection && selection[0] !== selection[1] && (
        <Measurements
          plot={plot}
          infos={infos}
          stats={stats}
          range={[Math.min(...selection), Math.max(...selection)]}
          wall={wall}
          onClose={() => setSelection(null)}
          onZoom={() => {
            commit(fixedWindow(Math.min(...selection), Math.max(...selection)))
            setSelection(null)
          }}
        />
      )}
    </div>
  )
}

function swap(p: PlotConfig, a: number, b: number): PlotConfig {
  const lanes = p.lanes.slice()
  ;[lanes[a], lanes[b]] = [lanes[b], lanes[a]]
  return { ...p, lanes }
}

// ---------------------------------------------------------------- legend

function Legend({
  plot,
  infos,
  views,
  cursorT,
  update,
}: {
  plot: PlotConfig
  infos: Map<string, SeriesInfo>
  views: Record<string, SeriesView>
  cursorT: number | null
  update: (f: (p: PlotConfig) => PlotConfig) => void
}) {
  const toggle = (name: string) => update((p) => ({ ...p, lanes: p.lanes.map((l) => ({ ...l, traces: l.traces.map((t) => (t.series === name ? { ...t, hidden: !t.hidden } : t)) })) }))
  const remove = (name: string) => update((p) => ({ ...p, lanes: p.lanes.map((l) => ({ ...l, traces: l.traces.filter((t) => t.series !== name) })).filter((l) => l.traces.length) }))
  const traces: Trace[] = plot.lanes.flatMap((l) => l.traces)
  return (
    <div className="wb-data-legend" role="list" aria-label="Series">
      {traces.map((t) => {
        const info = infos.get(t.series)
        const kind: Kind = info?.kind ?? 'analog'
        return (
          <div key={t.series} role="listitem" className={['chip', t.hidden && 'off'].filter(Boolean).join(' ')}>
            <button type="button" className="key" aria-pressed={!t.hidden} title={`${t.series}${info?.label ? `\n${info.label}` : ''}\n${t.hidden ? 'Click to show it' : 'Click to hide it'}`} onClick={() => toggle(t.series)}>
              <span className={`wb-data-swatch ${kind}`} style={kind === 'analog' && t.color > 0 ? { background: `var(--plot-${t.color})` } : undefined} aria-hidden />
              <span className="name">{t.series}</span>
            </button>
            <span className={['val', !info && 'muted'].filter(Boolean).join(' ')} title={info ? undefined : 'No series of this name yet: it appears when samples arrive'}>
              {info ? readout(kind, views[t.series], info, cursorT) : 'waiting'}
            </span>
            {t.hidden && <EyeOff size={12} aria-hidden />}
            <IconButton className="remove" size="small" icon={X} label={`Remove ${t.series} from this plot`} onClick={() => remove(t.series)} />
          </div>
        )
      })}
      {!traces.length && <span className="wb-data-note">No series yet: add one with +</span>}
      {(plot.markers ?? []).map((m) => (
        <div key={`m:${m}`} className="chip marker" role="listitem">
          <span className="wb-data-swatch marker" aria-hidden />
          <span className="name">{m}</span>
          <span className="val muted">markers</span>
        </div>
      ))}
    </div>
  )
}

function CursorTip({ t, x, wall, width }: { t: number; x: number; wall: boolean; width: number }) {
  const label = formatTimeExact(t, wall)
  const right = x > width / 2
  return (
    <div className="wb-data-tip" style={right ? { right: Math.max(4, width - x + 8) } : { left: x + 8 }}>
      {label}
    </div>
  )
}

// ---------------------------------------------------------------- overview

function OverviewBar({ ext, t0, t1, following, onMove, onFollow }: { ext: { oldest: number | null; newest: number | null }; t0: number; t1: number; following: boolean; onMove: (t0: number) => void; onFollow: () => void }) {
  const ref = useRef<HTMLDivElement>(null)
  if (ext.oldest === null || ext.newest === null || ext.newest <= ext.oldest) return <div className="wb-data-scroll off" aria-hidden />
  const lo = Math.min(ext.oldest, t0)
  const hi = Math.max(ext.newest, t1)
  const total = hi - lo
  const size = Math.min(1, Math.max(0.03, (t1 - t0) / total))
  const start = Math.min(1 - size, Math.max(0, (t0 - lo) / total))
  const at = (clientX: number) => {
    const r = ref.current!.getBoundingClientRect()
    return lo + ((clientX - r.left) / r.width) * total
  }
  return (
    <div className="wb-data-scroll">
      <div
        ref={ref}
        className="bar"
        role="scrollbar"
        aria-orientation="horizontal"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(start * 100)}
        aria-label="Position in the capture"
        onPointerDown={(e) => {
          e.currentTarget.setPointerCapture(e.pointerId)
          const centre = at(e.clientX)
          onMove(centre - (t1 - t0) / 2)
        }}
        onPointerMove={(e) => {
          if (e.buttons & 1) onMove(at(e.clientX) - (t1 - t0) / 2)
        }}
      >
        <div className="thumb" style={{ left: `${start * 100}%`, width: `${size * 100}%` }} />
      </div>
      <Button size="small" variant="ghost" className={following ? 'live on' : 'live'} onClick={onFollow} title="Follow the newest samples (End)">
        Live
      </Button>
    </div>
  )
}

// ---------------------------------------------------------------- measurements

function Measurements({
  plot,
  infos,
  stats,
  range,
  wall,
  onClose,
  onZoom,
}: {
  plot: PlotConfig
  infos: Map<string, SeriesInfo>
  stats: Record<string, RangeStats> | null
  range: [number, number]
  wall: boolean
  onClose: () => void
  onZoom: () => void
}) {
  const traces = plot.lanes.flatMap((l) => l.traces).filter((t) => !t.hidden)
  const width = range[1] - range[0]
  const rows = traces.map((t) => {
    const st = stats?.[t.series]
    const info = infos.get(t.series)
    return { t, st, info }
  })
  const infoKinds = new Set(rows.map((r) => r.info?.kind))
  const showAnalog = infoKinds.has('analog')
  const showDigital = infoKinds.has('digital')
  const copy = () => {
    const lines = rows.map(({ t, st, info }) => {
      if (!st || !info) return `${t.series}\t—`
      if (info.kind === 'analog' && st.count) return `${t.series}\tmin ${formatValue(st.min ?? NaN, info.unit)}\tmax ${formatValue(st.max ?? NaN, info.unit)}\tmean ${formatValue(st.mean ?? NaN, info.unit)}\trms ${formatValue(st.rms ?? NaN, info.unit)}`
      if (info.kind === 'digital') return `${t.series}\tfrequency ${st.frequency ? formatValue(st.frequency, 'Hz') : '—'}\tduty ${st.duty !== null && st.duty !== undefined ? `${(st.duty * 100).toFixed(1)} %` : '—'}\tedges ${(st.rising ?? 0) + (st.falling ?? 0)}`
      return `${t.series}\t${st.count ?? 0}`
    })
    void copyText([`Range ${formatDuration(width)} (${formatTimeExact(range[0], wall)} to ${formatTimeExact(range[1], wall)})`, ...lines].join('\n'))
  }
  return (
    <div className="wb-data-measure" aria-label="Measurements of the selected range">
      <div className="head">
        <strong>{formatDuration(width)}</strong>
        <span className="muted">{width > 0 ? `${formatValue(1 / width, 'Hz')}` : ''}</span>
        <span className="grow" />
        <Button size="small" variant="ghost" onClick={onZoom}>
          Zoom to range
        </Button>
        <Button size="small" variant="ghost" onClick={copy}>
          Copy
        </Button>
        <IconButton size="small" icon={X} label="Close the measurement (Esc)" onClick={onClose} />
      </div>
      <table>
        <thead>
          <tr>
            <th>Series</th>
            {showAnalog && (
              <>
                <th>Min</th>
                <th>Max</th>
                <th>Mean</th>
                <th>RMS</th>
                <th>Peak to peak</th>
                <th>Area</th>
              </>
            )}
            {showDigital && (
              <>
                <th>Frequency</th>
                <th>Duty</th>
                <th>Edges</th>
              </>
            )}
          </tr>
        </thead>
        <tbody>
          {rows.map(({ t, st, info }) => {
            const unit = info?.unit ?? ''
            const num = (v: number | null | undefined) => (v === null || v === undefined ? '—' : formatValue(v, unit))
            const analog = info?.kind === 'analog'
            const digital = info?.kind === 'digital'
            const iu = integralUnit(unit)
            return (
              <tr key={t.series}>
                <th>
                  <span className={`wb-data-swatch ${info?.kind ?? 'analog'}`} style={analog && t.color > 0 ? { background: `var(--plot-${t.color})` } : undefined} aria-hidden /> {t.series}
                </th>
                {!st ? (
                  <td colSpan={9} className="muted">
                    …
                  </td>
                ) : info?.kind === 'events' ? (
                  <td colSpan={9} className="note">
                    {st.count ?? 0} events
                  </td>
                ) : (
                  <>
                    {showAnalog && (
                      <>
                        <td>{analog ? num(st.min) : ''}</td>
                        <td>{analog ? num(st.max) : ''}</td>
                        <td title={analog && st.timeMean !== null && st.timeMean !== undefined ? `Time-weighted mean ${formatValue(st.timeMean, unit)}` : undefined}>{analog ? num(st.mean) : ''}</td>
                        <td>{analog ? num(st.rms) : ''}</td>
                        <td>{analog ? num(st.peakToPeak) : ''}</td>
                        <td title={`${iu.label}: the area under the trace`}>{analog && st.integral !== undefined ? formatValue(st.integral, iu.unit) : ''}</td>
                      </>
                    )}
                    {showDigital && (
                      <>
                        <td>{digital && st.frequency ? formatValue(st.frequency, 'Hz') : digital ? '—' : ''}</td>
                        <td>{digital && st.duty !== null && st.duty !== undefined ? `${(st.duty * 100).toFixed(1)} %` : digital ? '—' : ''}</td>
                        <td>{digital ? (st.rising ?? 0) + (st.falling ?? 0) : ''}</td>
                      </>
                    )}
                  </>
                )}
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}
