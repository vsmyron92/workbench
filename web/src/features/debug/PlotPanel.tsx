// The `debug.plot` panel: several watched values drawn together on one chart, each plot a saved
// configuration (`plotStore`) of its own, so a project can keep a plot per question ("motor", "ADC",
// "state machine"). The values are Live Watch's: the panel follows the project's debug session and
// draws what `plotBuffer` collected, without stopping the program.
//
// One axis only. Values of different sizes (an ADC count beside a temperature) either share the axis in
// their own unit or are each drawn as a percentage of their own range in view (the "Each as % of range"
// scale), which keeps one axis and says so; the tooltip, legend and table always give the real values.

import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type PointerEvent as ReactPointerEvent } from 'react'
import { ChartLine, ChevronsRight, Ellipsis, Eye, Pause, Play, Table2, X } from 'lucide-react'
import { confirmDialog, promptDialog, toast } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { Button, copyText, EmptyState, IconButton, Input, Loading, Select, showMenuAt } from '@/ui'
import { useLiveSnapshot } from './liveSnapshot'
import { useLive } from './liveStore'
import { formatLiveValue } from './logic'
import { setPausing } from './livePausing'
import { addSeriesTo, openPlot, plottable, watchForPlot } from './plotActions'
import { trackOf, usePlotClock } from './plotBuffer'
import { drawPlot, edgeTime, layoutFor, readPlotTheme, snapTime, stepTime, timeAtX, type DrawSeries, type PlotGeometry, type PlotTheme } from './plotDraw'
import { formatAgo, formatAgoExact, nextWindow, panTo, plotCsv, plotGapMs, plotRows, PLOT_SLOTS, PLOT_WINDOWS, readingAt, scrollThumb, windowLabel, zoomEnd, type Track } from './plotMath'
import { usePlots, type PlotConfig, type PlotScale, type PlotSeries } from './plotStore'
import { useDebug, activeSession } from './store'
import type { DebugSession, LiveItem } from './types'

export interface PlotParams {
  projectId: string
  plotId: string
}

const NO_ITEMS: LiveItem[] = []

/** A series with what the session has for it. */
interface Row extends PlotSeries {
  item?: LiveItem
  track?: Track
}

/** A reading as written: by its type, and from the exact digits when the double cannot hold the whole number (64-bit values). */
const readingText = (item: LiveItem | undefined, v: number, raw?: string) => (!item ? (raw ?? String(v)) : item.kind === 'bool' ? (v ? 'true' : 'false') : formatLiveValue(item.kind, item.size, raw ?? v))

/** Each series with the item the session watches for it and the readings so far. Resolved when drawing, not kept: a series' readings
 *  start to exist after the panel does. */
function resolveRows(series: PlotSeries[], items: LiveItem[], sid: string | undefined): Row[] {
  return series.map((s) => {
    const item = items.find((i) => i.expression === s.expression)
    return { ...s, item, track: trackOf(sid, item?.id) }
  })
}

/** The time of the oldest reading of any series. */
function oldestTime(rows: Row[]): number | null {
  let best: number | null = null
  for (const r of rows) if (r.track?.t.length && (best === null || r.track.t[0] < best)) best = r.track.t[0]
  return best
}

/** The time of the newest reading of any series. */
function newestTime(rows: Row[]): number | null {
  let best: number | null = null
  for (const r of rows) {
    const n = r.track?.t.length
    if (n && (best === null || r.track!.t[n - 1] > best)) best = r.track!.t[n - 1]
  }
  return best
}

const swatch = (slot: number) => ({ background: `var(--plot-${slot})` })

// ---------------------------------------------------------------- legend

/** `sid` is the session whose readings are shown (an ended one still has its last ones); `watch` is the session new
 *  watches can be started in (null once it has ended). */
function Legend({ projectId, plot, items, sid, watch }: { projectId: string; plot: PlotConfig; items: LiveItem[]; sid: string | undefined; watch: DebugSession | null }) {
  usePlotClock() // the values move
  const rows = resolveRows(plot.series, items, sid)
  const { toggleSeries, removeSeries } = usePlots.getState()
  return (
    <div className="wb-dbg-plot-legend" role="list" aria-label="Series">
      {rows.map((r) => {
        // The newest reading, not the newest good one: after a failed read the old number would pass for current.
        const newest = r.track?.v.length ? r.track.v[r.track.v.length - 1] : undefined
        const newestRaw = r.track?.v.length ? r.track.raw?.[r.track.v.length - 1] : undefined
        const failed = r.item?.error
        return (
          <div key={r.expression} role="listitem" className={['chip', r.hidden && 'off'].filter(Boolean).join(' ')}>
            <button
              type="button"
              className="key"
              aria-pressed={!r.hidden}
              title={`${r.expression}\n${r.hidden ? 'Click to show it on the chart' : 'Click to hide it from the chart'}`}
              onClick={() => toggleSeries(projectId, plot.id, r.expression)}
            >
              <span className="swatch" style={swatch(r.slot)} aria-hidden />
              <span className="name">{r.expression}</span>
            </button>
            {!r.item ? (
              watch ? (
                <Button size="small" variant="ghost" icon={Eye} onClick={() => void watchForPlot(watch, r.expression)} title="Start reading it from the running program">
                  Watch
                </Button>
              ) : (
                <span className="val muted">not read</span>
              )
            ) : failed ? (
              <span className="val error" title={failed}>
                {failed}
              </span>
            ) : (
              <span className="val" title={newest !== undefined && !Number.isFinite(newest) ? 'The last reading failed, or is not a number' : undefined}>
                {newest !== undefined && Number.isFinite(newest) ? readingText(r.item, newest, newestRaw) : '—'}
              </span>
            )}
            <IconButton className="remove" size="small" icon={X} label={`Remove ${r.expression} from this plot`} onClick={() => removeSeries(projectId, plot.id, r.expression)} />
          </div>
        )
      })}
    </div>
  )
}

// ---------------------------------------------------------------- chart

type Hover = { x: number } | { t: number } | null

interface ChartProps {
  plot: PlotConfig
  items: LiveItem[]
  sid: string | undefined
  /** The time at the right edge, or null while the chart follows the newest reading. */
  pausedAt: number | null
  intervalMs: number
  visible: boolean
  /** The view moved (scrolled, dragged, paged): its new right edge, null to follow the newest reading again. */
  onView: (end: number | null) => void
  /** The view zoomed to another span, ending at `end` (null: following). */
  onZoom: (windowMs: number, end: number | null) => void
}

function PlotChart({ plot, items, sid, pausedAt, intervalMs, visible, onView, onZoom }: ChartProps) {
  const wrap = useRef<HTMLDivElement>(null)
  const canvas = useRef<HTMLCanvasElement>(null)
  const geom = useRef<PlotGeometry | null>(null)
  const [size, setSize] = useState({ w: 0, h: 0 })
  const [hover, setHover] = useState<Hover>(null)
  const [dragging, setDragging] = useState(false)
  const drag = useRef<{ x: number; end: number; moved: boolean } | null>(null)
  const [themeTick, setThemeTick] = useState(0)
  const theme = useRef<{ tick: number; read: PlotTheme } | null>(null) // the tokens as read for that theme change
  usePlotClock() // new readings: draw again

  useEffect(() => {
    const el = wrap.current
    if (!el) return
    const ro = new ResizeObserver(([e]) => {
      const w = Math.floor(e.contentRect.width)
      const h = Math.floor(e.contentRect.height)
      setSize((s) => (s.w === w && s.h === h ? s : { w, h }))
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  // A theme change recolours the tokens the canvas reads.
  useEffect(() => {
    const mo = new MutationObserver(() => setThemeTick((n) => n + 1))
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'class'] })
    return () => mo.disconnect()
  }, [])

  const rows = resolveRows(plot.series, items, sid)
  const draws: DrawSeries[] = rows.map((r) => ({ name: r.expression, slot: r.slot, track: r.track, hidden: !!r.hidden }))
  const shown = draws.filter((d) => !d.hidden)
  const oldest = oldestTime(rows)
  const newest = newestTime(rows)
  const t1 = pausedAt ?? newest ?? Date.now()
  const g = layoutFor({ width: size.w, height: size.h, series: draws, windowMs: plot.windowMs, scale: plot.scale, t1 })
  const msPerPx = plot.windowMs / Math.max(1, g.x1 - g.x0)
  /** Move the view so that it ends at `end`: kept within the readings, or following the newest. */
  const view = (end: number) => onView(panTo(end, oldest, newest, plot.windowMs))
  /** Zoom to the next span, keeping `anchor` where it is on screen (a chart that follows the newest reading keeps following). */
  const zoom = (dir: 1 | -1, anchor: number) => {
    const next = nextWindow(plot.windowMs, dir)
    if (next === plot.windowMs) return
    onZoom(next, pausedAt === null ? null : panTo(zoomEnd(anchor, t1, plot.windowMs, next), oldest, newest, next))
  }
  // The wheel scrolls through time and Ctrl+wheel zooms. A listener of our own: React's wheel handlers are passive and
  // cannot stop the page from zooming. It reads the latest values from a ref, so it is attached once.
  const latest = useRef({ g, t1, view, zoom, msPerPx })
  latest.current = { g, t1, view, zoom, msPerPx }
  useEffect(() => {
    const el = wrap.current
    if (!el) return
    const onWheel = (e: WheelEvent) => {
      const L = latest.current
      e.preventDefault()
      if (e.ctrlKey || e.metaKey) {
        const x = e.clientX - el.getBoundingClientRect().left
        L.zoom(e.deltaY < 0 ? -1 : 1, Math.min(L.g.t1, Math.max(L.g.t0, timeAtX(L.g, x))))
        return
      }
      const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? L.g.x1 - L.g.x0 : 1
      const d = (Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY) * unit
      L.view(L.t1 + d * L.msPerPx)
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
  }, [])
  const wanted = !hover ? null : 'x' in hover ? snapTime(draws, timeAtX(g, hover.x)) : hover.t
  // A crosshair outside the span (the window slid on under a key-chosen time) is no crosshair.
  const cursorT = wanted !== null && wanted >= g.t0 && wanted <= g.t1 ? wanted : null

  // Runs after every render: renders happen when readings, size, hover or settings change.
  useLayoutEffect(() => {
    const c = canvas.current
    if (!c || !visible || size.w < 40 || size.h < 40) return
    const dpr = window.devicePixelRatio || 1
    const W = Math.round(size.w * dpr)
    const H = Math.round(size.h * dpr)
    if (c.width !== W || c.height !== H) {
      c.width = W
      c.height = H
    }
    const ctx = c.getContext('2d')
    if (!ctx) return
    // Reading the tokens costs a style lookup each: once per theme change, not once per frame.
    if (theme.current?.tick !== themeTick) theme.current = { tick: themeTick, read: readPlotTheme(c) }
    geom.current = drawPlot(ctx, { width: size.w, height: size.h, dpr, series: draws, windowMs: plot.windowMs, scale: plot.scale, t1, intervalMs, cursorT, theme: theme.current.read })
  })

  const gap = plotGapMs(intervalMs)
  const readout =
    cursorT === null
      ? null
      : {
          when: `${formatAgo(t1 - cursorT)} · ${new Date(cursorT).toLocaleTimeString([], { hour12: false })}`,
          lines: rows
            .filter((r) => !r.hidden)
            .map((r) => {
              const hit = readingAt(r.track, cursorT, gap)
              return { expression: r.expression, slot: r.slot, text: hit ? readingText(r.item, hit.v, hit.raw) : '—' }
            }),
        }

  const onKey = (e: KeyboardEvent) => {
    if (e.altKey || e.ctrlKey || e.metaKey) return // Alt+Left is the browser's Back, Ctrl+Arrow belongs to the shell
    let next: number | null | undefined
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      const dir = e.key === 'ArrowRight' ? 1 : -1
      const stepped = cursorT === null ? edgeTime(draws, g, 1) : stepTime(draws, cursorT, dir)
      next = stepped ?? cursorT
      // Past the edge of the view: scroll so that the crosshair stays in it.
      if (next !== null && next < g.t0) view(next + plot.windowMs * 0.9)
      else if (next !== null && next > g.t1) view(next + plot.windowMs * 0.1)
    } else if (e.key === 'Home') {
      next = oldest
      if (oldest !== null) view(oldest + plot.windowMs * 0.5)
    } else if (e.key === 'End') {
      next = newest
      onView(null)
    } else if (e.key === 'PageUp' || e.key === 'PageDown') {
      view(t1 + (e.key === 'PageDown' ? 1 : -1) * plot.windowMs * 0.9)
      next = cursorT
    } else if (e.key === '+' || e.key === '=' || e.key === '-') {
      zoom(e.key === '-' ? 1 : -1, cursorT ?? t1)
      next = cursorT
    } else if (e.key === 'Escape') next = null
    else return
    e.preventDefault()
    setHover(next === null || next === undefined ? null : { t: next })
  }

  const cx = cursorT === null ? 0 : g.x0 + ((cursorT - g.t0) / (g.t1 - g.t0)) * (g.x1 - g.x0)
  const flip = cx > size.w * 0.55
  const label = `Plot of ${shown.map((d) => d.name).join(', ') || 'no series'}. Left and Right move through the readings, Page Up and Page Down scroll, plus and minus zoom, Escape clears.`
  // What a screen reader hears when the keys (not the pointer) move the crosshair.
  const spoken = readout && hover && 't' in hover ? `${readout.when}: ${readout.lines.map((l) => `${l.expression} ${l.text}`).join(', ')}` : ''

  return (
    <>
      <div className="wb-dbg-plot-chart" ref={wrap}>
        <canvas
          ref={canvas}
          tabIndex={0}
          role="img"
          aria-label={label}
          className={dragging ? 'dragging' : undefined}
          style={{ width: size.w, height: size.h }}
          onPointerDown={(e) => {
            if (e.button !== 0) return
            e.currentTarget.setPointerCapture(e.pointerId)
            drag.current = { x: e.clientX, end: t1, moved: false }
          }}
          onPointerMove={(e) => {
            const d = drag.current
            if (d) {
              const dx = e.clientX - d.x
              if (!d.moved && Math.abs(dx) < 4) return
              if (!d.moved) {
                d.moved = true
                setDragging(true)
                setHover(null)
              }
              view(d.end - dx * msPerPx) // dragging the chart to the right shows earlier readings
              return
            }
            const x = e.clientX - e.currentTarget.getBoundingClientRect().left
            const cur = geom.current
            setHover(cur && x >= cur.x0 && x <= cur.x1 ? { x } : null)
          }}
          onPointerUp={() => {
            drag.current = null
            setDragging(false)
          }}
          onPointerCancel={() => {
            drag.current = null
            setDragging(false)
          }}
          onPointerLeave={() => setHover((h) => (h && 'x' in h ? null : h))}
          onKeyDown={onKey}
          onBlur={() => setHover((h) => (h && 't' in h ? null : h))}
        />
        <div className="wb-dbg-plot-live" role="status" aria-live="polite">
          {spoken}
        </div>
        {readout && (
          <div className="wb-dbg-plot-tip" style={flip ? { right: size.w - cx + 12, top: g.y0 + 6 } : { left: cx + 12, top: g.y0 + 6 }} aria-hidden>
            <div className="when">{readout.when}</div>
            {readout.lines.map((l) => (
              <div className="row" key={l.expression}>
                <span className="line" style={swatch(l.slot)} />
                <span className="val">{l.text}</span>
                <span className="nm">{l.expression}</span>
              </div>
            ))}
          </div>
        )}
      </div>
      <PlotScrollbar rows={rows} windowMs={plot.windowMs} t1={t1} following={pausedAt === null} left={g.x0} right={size.w - g.x1} onView={onView} />
    </>
  )
}

// ---------------------------------------------------------------- scroll bar

/** A scroll bar for time: its track is the readings kept, its thumb the span in view. Dragging the thumb scrolls, a click on
 *  the track centres the view there, and Follow goes back to the newest reading. It lines up with the chart's time axis. */
function PlotScrollbar({ rows, windowMs, t1, following, left, right, onView }: { rows: Row[]; windowMs: number; t1: number; following: boolean; left: number; right: number; onView: (end: number | null) => void }) {
  usePlotClock()
  const track = useRef<HTMLDivElement>(null)
  const drag = useRef<{ x: number; end: number } | null>(null)
  const oldest = oldestTime(rows)
  const newest = newestTime(rows)
  const total = oldest !== null && newest !== null ? newest - oldest : 0
  const thumb = scrollThumb(oldest, newest, t1 - windowMs, t1)
  const canScroll = total > windowMs
  const pos = thumb.size >= 1 ? 100 : Math.round((thumb.start / (1 - thumb.size)) * 100)
  const to = (end: number) => onView(panTo(end, oldest, newest, windowMs))
  const width = () => track.current?.getBoundingClientRect().width || 1
  const onKey = (e: KeyboardEvent) => {
    if (e.altKey || e.ctrlKey || e.metaKey || !canScroll) return
    if (e.key === 'ArrowLeft') to(t1 - windowMs * 0.1)
    else if (e.key === 'ArrowRight') to(t1 + windowMs * 0.1)
    else if (e.key === 'PageUp') to(t1 - windowMs * 0.9)
    else if (e.key === 'PageDown') to(t1 + windowMs * 0.9)
    else if (e.key === 'Home' && oldest !== null) to(oldest + windowMs)
    else if (e.key === 'End') onView(null)
    else return
    e.preventDefault()
  }
  return (
    <div className="wb-dbg-plot-scroll" style={{ paddingLeft: left, paddingRight: Math.max(10, right) }}>
      <div
        ref={track}
        className={['bar', !canScroll && 'off'].filter(Boolean).join(' ')}
        role="scrollbar"
        aria-label="Scroll through the readings"
        aria-orientation="horizontal"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pos}
        aria-disabled={!canScroll}
        tabIndex={canScroll ? 0 : -1}
        onKeyDown={onKey}
        onPointerDown={(e: ReactPointerEvent<HTMLDivElement>) => {
          if (!canScroll || oldest === null || e.button !== 0) return
          const rect = e.currentTarget.getBoundingClientRect()
          const f = (e.clientX - rect.left) / rect.width
          const onThumb = f >= thumb.start && f <= thumb.start + thumb.size
          e.currentTarget.setPointerCapture(e.pointerId)
          if (!onThumb) to(oldest + f * total + windowMs / 2) // centre the view on the click
          drag.current = { x: e.clientX, end: onThumb ? t1 : oldest + f * total + windowMs / 2 }
        }}
        onPointerMove={(e) => {
          const d = drag.current
          if (d && oldest !== null) to(d.end + ((e.clientX - d.x) / width()) * total)
        }}
        onPointerUp={() => (drag.current = null)}
        onPointerCancel={() => (drag.current = null)}
      >
        <div className="thumb" style={{ left: `${thumb.start * 100}%`, width: `${thumb.size * 100}%` }} />
      </div>
      {following ? (
        <span className="live wb-small">Live</span>
      ) : (
        <Button size="small" variant="primary" icon={ChevronsRight} onClick={() => onView(null)} title="Go back to the newest readings and follow them">
          Follow
        </Button>
      )}
    </div>
  )
}

// ---------------------------------------------------------------- table

const TABLE_ROWS = 200

function PlotTable({ plot, items, sid, pausedAt }: { plot: PlotConfig; items: LiveItem[]; sid: string | undefined; pausedAt: number | null }) {
  usePlotClock()
  const rows = resolveRows(plot.series, items, sid)
  const t1 = pausedAt ?? newestTime(rows) ?? Date.now()
  const data = plotRows(rows.map((r) => r.track), t1 - plot.windowMs, t1, TABLE_ROWS)
  return (
    <div className="wb-scroll wb-dbg-plot-table-wrap">
      <table className="wb-dbg-plot-table">
        <thead>
          <tr>
            <th>Time</th>
            {rows.map((r) => (
              <th key={r.expression}>
                <span className="line" style={swatch(r.slot)} />
                {r.expression}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {data.map((row) => (
            <tr key={row.t}>
              <td>{formatAgoExact(t1 - row.t)}</td>
              {row.v.map((v, i) => (
                <td key={rows[i].expression}>{v === undefined ? '—' : readingText(rows[i].item, v, row.raw?.[i])}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {!data.length && <div className="wb-muted wb-small wb-dbg-plot-none">No readings in the last {windowLabel(plot.windowMs)}.</div>}
    </div>
  )
}

// ---------------------------------------------------------------- the panel

export function PlotPanel({ params, setTitle, close, visible }: PanelProps<PlotParams>) {
  const { projectId, plotId } = params
  const plot = usePlots((s) => s.byProject[projectId]?.find((p) => p.id === plotId))
  const plotsKnown = usePlots((s) => !!s.loaded[projectId])
  useEffect(() => {
    void usePlots.getState().load(projectId)
  }, [projectId])
  const session = useDebug((st) => activeSession(st, projectId))
  const liveSession = session?.live ? session : null
  useLiveSnapshot(liveSession)
  const items = useLive((l) => (liveSession ? l.sessions[liveSession.id]?.items : undefined)) ?? NO_ITEMS
  const intervalMs = useLive((l) => (liveSession ? l.sessions[liveSession.id]?.intervalMs : undefined)) ?? 250
  const pausingAllowed = useLive((l) => (liveSession ? l.sessions[liveSession.id]?.pausing : undefined)) ?? false
  const [view, setView] = useState<'chart' | 'table'>('chart')
  const [pausedAt, setPausedAt] = useState<number | null>(null)
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const input = useRef<HTMLInputElement>(null)
  const listId = useId()

  const name = plot?.name
  useEffect(() => {
    if (name) setTitle(name)
  }, [name, setTitle])

  const sid = liveSession?.id

  if (!plot) {
    // Before the project's plots arrive from the server a plot is not known yet, which is not the same as deleted.
    if (!plotsKnown) return <Loading label="Loading the plot…" />
    return <EmptyState icon={ChartLine} title="This plot was deleted" action={<Button onClick={close}>Close</Button>} />
  }

  const store = usePlots.getState()
  const rows = resolveRows(plot.series, items, sid)
  const unwatched = rows.filter((r) => !r.item)
  const ended = session?.state === 'terminated' || session?.state === 'failed'
  // The session to watch things in: one that has ended cannot read anything any more.
  const usable = liveSession && !ended ? liveSession : null
  const suggestions = items.filter((i) => plottable(i) && !plot.series.some((s) => s.expression === i.expression))

  const add = async () => {
    const expression = text.trim()
    if (!expression || busy) return
    setBusy(true)
    try {
      if (await addSeriesTo(projectId, plot.id, expression, usable)) setText('')
    } finally {
      setBusy(false)
      input.current?.focus()
    }
  }

  const watchAll = async () => {
    if (!usable) return
    for (const r of unwatched) await watchForPlot(usable, r.expression)
  }

  const rename = async () => {
    const name = await promptDialog({ title: 'Rename plot', initial: plot.name, confirmLabel: 'Rename' })
    if (name?.trim()) store.rename(projectId, plot.id, name)
  }

  const copyCsv = async () => {
    const fresh = resolveRows(plot.series, items, sid)
    const t1 = pausedAt ?? newestTime(fresh) ?? Date.now()
    const data = plotRows(fresh.map((r) => r.track), t1 - plot.windowMs, t1, 20_000)
    if (!data.length) {
      toast('info', 'No readings in view yet', { timeout: 2000 })
      return
    }
    if (await copyText(plotCsv(fresh.map((r) => r.expression), data, t1))) toast('success', `Copied ${data.length} readings as CSV`, { timeout: 2000 })
    else toast('error', 'Could not copy')
  }

  const menu = (el: HTMLElement) =>
    showMenuAt(el, [
      { label: 'Rename…', run: () => void rename() },
      {
        label: 'Duplicate Plot',
        run: () => {
          const copy = store.duplicate(projectId, plot.id)
          if (copy) openPlot(projectId, copy)
          else toast('error', 'This project keeps as many plots as it can')
        },
      },
      { label: 'Copy Readings as CSV', run: () => void copyCsv() },
      { label: 'Watch All Series', icon: Eye, disabled: !usable || !unwatched.length, run: () => void watchAll() },
      'separator',
      {
        label: 'Delete Plot…',
        danger: true,
        run: () => {
          void confirmDialog({
            title: `Delete ${plot.name}?`,
            message: 'The plot is removed from this browser. The variables stay in Live Watch.',
            confirmLabel: 'Delete',
            danger: true,
          }).then((ok) => {
            if (!ok) return
            store.remove(projectId, plot.id)
            close()
          })
        },
      },
    ])

  const paused = pausedAt !== null
  return (
    <div className="wb-dbg-plot">
      <div className="wb-dbg-plot-bar">
        <button type="button" className="title" title="Rename" onClick={() => void rename()}>
          {plot.name}
        </button>
        <Input
          small
          ref={input}
          value={text}
          list={listId}
          readOnly={busy}
          placeholder={`Add a variable to draw (up to ${PLOT_SLOTS}): ticks, adc.raw, *(uint32_t*)0x50000014`}
          aria-label="Variable to add to this plot"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void add()
          }}
        />
        <datalist id={listId}>
          {suggestions.map((i) => (
            <option key={i.id} value={i.expression} />
          ))}
        </datalist>
        <Select value={plot.windowMs} aria-label="Time span" title="How much time the chart shows" onChange={(e) => store.setWindow(projectId, plot.id, Number(e.target.value))}>
          {PLOT_WINDOWS.map((ms) => (
            <option key={ms} value={ms}>
              {windowLabel(ms)}
            </option>
          ))}
        </Select>
        <Select
          value={plot.scale}
          aria-label="Scale"
          title="Same axis: one axis in the values' own unit. Each as % of range: every series scaled to its own range in view, for values of different sizes (the tooltip and legend keep the real values)."
          onChange={(e) => store.setScale(projectId, plot.id, e.target.value as PlotScale)}
        >
          <option value="shared">Same axis</option>
          <option value="normalized">Each as % of range</option>
        </Select>
        <IconButton
          icon={paused ? Play : Pause}
          active={paused}
          label={paused ? 'Follow the live readings' : 'Pause the chart (readings keep being collected)'}
          onClick={() => setPausedAt(paused ? null : (newestTime(resolveRows(plot.series, items, sid)) ?? Date.now()))}
        />
        <IconButton icon={view === 'chart' ? Table2 : ChartLine} label={view === 'chart' ? 'Show the readings as a table' : 'Show the chart'} onClick={() => setView(view === 'chart' ? 'table' : 'chart')} />
        <IconButton icon={Ellipsis} label="More" onClick={(e) => menu(e.currentTarget)} />
      </div>
      {rows.length > 0 && <Legend projectId={projectId} plot={plot} items={items} sid={sid} watch={usable} />}
      {!session ? (
        <div className="wb-dbg-plot-note wb-muted wb-small">No debug session in this project. Start one on a target with Live Watch (OpenOCD) and these series draw here.</div>
      ) : !liveSession ? (
        <div className="wb-dbg-plot-note wb-muted wb-small">This debug session cannot read values while the program runs. Live Watch needs a debug server with a Tcl port (the OpenOCD preset has one).</div>
      ) : liveSession.liveMode === 'pausing' && !pausingAllowed && !ended ? (
        <div className="wb-dbg-plot-note wb-small">
          This debug server cannot be read while the program runs, so nothing is drawn until you allow reading by pausing the program for a moment.{' '}
          <Button size="small" variant="primary" onClick={() => void setPausing(liveSession, true)}>
            Allow…
          </Button>
        </div>
      ) : usable && unwatched.length > 0 ? (
        <div className="wb-dbg-plot-note wb-small">
          {unwatched.length === 1 ? '1 series is' : `${unwatched.length} series are`} not read in this session.{' '}
          <Button size="small" icon={Eye} onClick={() => void watchAll()}>
            Watch {unwatched.length === 1 ? 'it' : 'them'}
          </Button>
        </div>
      ) : null}
      <div className="wb-dbg-plot-body">
        {rows.length === 0 ? (
          <EmptyState icon={ChartLine} title="Nothing to plot yet">
            Type a variable in the box above and press Enter. Everything you add is drawn on the same chart, each in its own colour. Several plots can be open side by side: make another from the Live tab.
          </EmptyState>
        ) : view === 'chart' ? (
          <PlotChart
            plot={plot}
            items={items}
            sid={sid}
            pausedAt={pausedAt}
            intervalMs={intervalMs}
            visible={visible}
            onView={setPausedAt}
            onZoom={(windowMs, end) => {
              store.setWindow(projectId, plot.id, windowMs)
              setPausedAt(end)
            }}
          />
        ) : (
          <PlotTable plot={plot} items={items} sid={sid} pausedAt={pausedAt} />
        )}
      </div>
      {ended && rows.length > 0 && <div className="wb-dbg-plot-note wb-muted wb-small">The session ended: these are the last readings.</div>}
    </div>
  )
}
