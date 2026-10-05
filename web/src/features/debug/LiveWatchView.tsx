// Variables of the running program, read over and over without stopping it: the debug server's Tcl
// port reads their memory (gdb cannot while the program runs). Each row shows the latest value, how
// it moved (a sparkline of the recent readings) and, in the tooltip, where it lives. Only things at
// a fixed address can be watched: a global, a member of one, `buf[3]`, `*(uint32_t*)0x50000014`.

import { useEffect, useRef, useState } from 'react'
import { AlertTriangle, ChartLine, ChartSpline, Plus, X } from 'lucide-react'
import { Button, EmptyState, IconButton, Input, Select, showMenu, showMenuAt, type MenuEntry } from '@/ui'
import { toast, toastError } from '@/shell/actions'
import { debugApi } from './api'
import { PAUSING_EXPLAINED, setPausing } from './livePausing'
import { refreshLive, useLiveSnapshot } from './liveSnapshot'
import { emptyLive, useLive } from './liveStore'
import { formatLiveValue, type Radix } from './logic'
import { trackOf, usePlotClock } from './plotBuffer'
import { plotGapMs, sparklinePoints, spanExtent, windowLabel, type Track } from './plotMath'
import { addSeriesTo, newPlot, newPlotFromWatched, openPlot, plottable } from './plotActions'
import { usePlots, type PlotConfig } from './plotStore'
import type { DebugSession, LiveItem, LiveSample } from './types'

const INTERVALS = [50, 100, 250, 500, 1000, 2000, 5000]
const W = 120
const H = 22
/** The sparkline shows at least this much time (more at a slow poll), however fast the readings come. */
const SPARK_MS = 30_000
const sparkSpan = (intervalMs: number) => Math.max(SPARK_MS, intervalMs * 60)
const NO_PLOTS: PlotConfig[] = []
const PLOTS_IN_ROW_MENU = 8

function copy(text: string) {
  void navigator.clipboard?.writeText(text).then(
    () => toast('success', 'Copied', { timeout: 1500 }),
    () => toast('error', 'Could not copy'),
  )
}

function Sparkline({ item, track, intervalMs }: { item: LiveItem; track: Track | undefined; intervalMs: number }) {
  if (item.kind === 'bytes') return <span className="spark" />
  const runs = sparklinePoints(track, sparkSpan(intervalMs), W, H, plotGapMs(intervalMs))
  return (
    <svg className="spark" width={W} height={H} viewBox={`0 0 ${W} ${H}`} aria-hidden>
      {runs.map((points, i) => (points.includes(' ') ? <polyline key={i} points={points} /> : <circle key={i} cx={points.split(',')[0]} cy={points.split(',')[1]} r={1.5} />))}
    </svg>
  )
}

function LiveRow({ s, item, samples, plots, intervalMs }: { s: DebugSession; item: LiveItem; samples: LiveSample[]; plots: PlotConfig[]; intervalMs: number }) {
  const track = trackOf(s.id, item.id)
  const [radix, setRadix] = useState<Radix>('dec')
  const last = samples[samples.length - 1]
  const prev = samples[samples.length - 2]
  const error = item.error ?? last?.e
  const text = error ? '' : formatLiveValue(item.kind, item.size, last?.v, radix)
  const changed = !error && !!prev && prev.v !== last?.v
  const integer = item.kind === 'int' || item.kind === 'uint' || item.kind === 'enum'
  const where = item.address !== undefined ? `0x${item.address.toString(16).padStart(8, '0')} · ${item.size} byte${item.size === 1 ? '' : 's'} · ${item.typeName}` : ''
  const newest = track?.t.length ? track.t[track.t.length - 1] : null
  const extent = newest === null ? null : spanExtent(track, newest - sparkSpan(intervalMs), newest)
  const range = extent ? `\nlast ${windowLabel(sparkSpan(intervalMs))}: ${extent[0]} to ${extent[1]}` : ''
  const remove = () => debugApi.liveRemove(s.projectId, s.id, item.id).catch((e) => toastError(e, `Could not stop watching ${item.expression}`))
  return (
    <div
      className={['wb-dbg-live-row', changed && 'changed'].filter(Boolean).join(' ')}
      title={`${item.expression}\n${where}${range}`}
      onContextMenu={(e) =>
        showMenu(e, [
          ...(integer ? ([{ label: 'Show as Decimal', run: () => setRadix('dec') }, { label: 'Show as Hex', run: () => setRadix('hex') }, { label: 'Show as Binary', run: () => setRadix('bin') }, 'separator'] as const) : []),
          { label: 'Copy Value', disabled: !text, run: () => copy(text) },
          { label: 'Copy Expression', run: () => copy(item.expression) },
          'separator',
          { label: 'Plot in New Plot', icon: ChartLine, disabled: !plottable(item), run: () => void newPlot(s.projectId, { expressions: [item.expression] }) },
          ...plots.slice(0, PLOTS_IN_ROW_MENU).map(
            (p): MenuEntry => ({
              label: `Add to ${p.name}`,
              disabled: !plottable(item) || p.series.some((x) => x.expression === item.expression),
              run: () => void addSeriesTo(s.projectId, p.id, item.expression, s).then((added) => added && openPlot(s.projectId, p)),
            }),
          ),
          'separator',
          { label: 'Stop Watching', run: () => void remove() },
        ])
      }
    >
      <span className="name">
        {item.expression}
        {item.peripheral && <AlertTriangle size={12} className="peri" aria-label="Peripheral register" />}
      </span>
      {error ? <span className="value error" title={error}>{error}</span> : <span className="value" onDoubleClick={() => integer && setRadix(radix === 'dec' ? 'hex' : radix === 'hex' ? 'bin' : 'dec')}>{text}</span>}
      <Sparkline item={item} track={track} intervalMs={intervalMs} />
      <IconButton className="remove" size="small" icon={X} label="Stop watching" onClick={() => void remove()} />
    </div>
  )
}

export function LiveWatchView({ s }: { s: DebugSession }) {
  const sess = useLive((l) => l.sessions[s.id]) ?? emptyLive()
  const plots = usePlots((st) => st.byProject[s.projectId]) ?? NO_PLOTS
  useEffect(() => {
    void usePlots.getState().load(s.projectId)
  }, [s.projectId])
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const input = useRef<HTMLInputElement>(null)
  usePlotClock() // the sparklines move with the readings

  useLiveSnapshot(s)

  const plotsMenu = (el: HTMLElement) =>
    showMenuAt(el, [
      { label: 'New Plot', icon: Plus, run: () => void newPlot(s.projectId) },
      { label: 'New Plot from Watched Values', icon: ChartSpline, disabled: !sess.items.some(plottable), run: () => void newPlotFromWatched(s) },
      ...(plots.length ? (['separator', ...plots.map((p): MenuEntry => ({ label: p.name, icon: ChartLine, run: () => openPlot(s.projectId, p) }))] as MenuEntry[]) : []),
    ])

  const add = async () => {
    const expression = text.trim()
    if (!expression || busy) return
    setBusy(true)
    try {
      await debugApi.liveAdd(s.projectId, s.id, expression)
      setText('')
      await refreshLive(s.projectId, s.id)
    } catch (e) {
      toastError(e, `Could not watch ${expression}`)
    } finally {
      setBusy(false)
      input.current?.focus()
    }
  }

  const ended = s.state === 'terminated' || s.state === 'failed'
  // A server with no side channel: the values are read by stopping the program for a moment, which the user allows.
  const pausingMode = s.liveMode === 'pausing'
  return (
    <div className="wb-dbg-live">
      <div className="wb-dbg-live-head">
        <Input
          small
          ref={input}
          value={text}
          readOnly={busy} // not disabled: focus goes back to the box when the watch is made
          disabled={ended}
          placeholder="Watch a variable or address: ticks, cfg.limit, *(uint32_t*)0x50000014"
          aria-label="Expression to watch"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void add()
          }}
        />
        <Select
          value={sess.intervalMs}
          aria-label="Read every"
          title="How often the values are read"
          onChange={(e) => void debugApi.liveInterval(s.projectId, s.id, Number(e.target.value)).catch((err) => toastError(err, 'Could not change the interval'))}
        >
          {[...new Set([...INTERVALS.filter((ms) => !pausingMode || ms >= 100), sess.intervalMs])].sort((a, b) => a - b).map((ms) => (
            <option key={ms} value={ms}>
              {ms >= 1000 ? `${ms / 1000} s` : `${ms} ms`}
            </option>
          ))}
        </Select>
        <IconButton icon={ChartLine} label="Plots: draw watched values together on one chart" onClick={(e) => plotsMenu(e.currentTarget)} />
      </div>
      {pausingMode && (
        <div className={['wb-dbg-live-pausing', sess.pausing && 'on'].filter(Boolean).join(' ')}>
          {sess.pausing ? (
            <>
              <span>
                Reading by pausing the program for a moment, every {Math.max(100, sess.intervalMs)} ms or slower.
                {sess.pauseMs !== null && (
                  <>
                    {' '}
                    Each read stops it for about {sess.pauseMs} ms: {Math.round((100 * sess.pauseMs) / (Math.max(100, sess.intervalMs) + sess.pauseMs))}% of the time at this rate.
                  </>
                )}
              </span>
              <Button size="small" onClick={() => void setPausing(s, false)}>
                Stop doing that
              </Button>
            </>
          ) : (
            <>
              <span title={PAUSING_EXPLAINED}>This debug server cannot read the program while it runs, so the values stay blank until you allow reading them by pausing the program for a moment.</span>
              <Button size="small" variant="primary" disabled={ended} onClick={() => void setPausing(s, true)}>
                Allow…
              </Button>
            </>
          )}
        </div>
      )}
      <div className="wb-scroll wb-dbg-live-list">
        {sess.items.length === 0 ? (
          <EmptyState title="Nothing watched yet">
            Type a variable above and press Enter. Its value is read from the running program over and over,{' '}
            {pausingMode ? 'by pausing the program for a moment (once you allow that).' : 'without stopping it.'}
          </EmptyState>
        ) : (
          sess.items.map((item) => <LiveRow key={item.id} s={s} item={item} samples={sess.samples[item.id] ?? []} plots={plots} intervalMs={sess.intervalMs} />)
        )}
      </div>
      {ended && <div className="wb-dbg-live-note wb-muted wb-small">The session ended: these are the last readings.</div>}
    </div>
  )
}
