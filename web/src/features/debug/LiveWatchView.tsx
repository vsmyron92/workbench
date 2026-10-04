// Variables of the running program, read over and over without stopping it: the debug server's Tcl
// port reads their memory (gdb cannot while the program runs). Each row shows the latest value, how
// it moved (a sparkline of the recent readings) and, in the tooltip, where it lives. Only things at
// a fixed address can be watched: a global, a member of one, `buf[3]`, `*(uint32_t*)0x50000014`.

import { useEffect, useRef, useState } from 'react'
import { AlertTriangle, X } from 'lucide-react'
import { EmptyState, IconButton, Input, Select, showMenu } from '@/ui'
import { toast, toastError } from '@/shell/actions'
import { debugApi } from './api'
import { emptyLive, useLive } from './liveStore'
import { formatLiveValue, numericValue, sparklineRuns, type Radix } from './logic'
import type { DebugSession, LiveItem, LiveSample } from './types'

const INTERVALS = [50, 100, 250, 500, 1000, 2000, 5000]
const W = 120
const H = 22

function copy(text: string) {
  void navigator.clipboard?.writeText(text).then(
    () => toast('success', 'Copied', { timeout: 1500 }),
    () => toast('error', 'Could not copy'),
  )
}

function Sparkline({ item, samples }: { item: LiveItem; samples: LiveSample[] }) {
  if (item.kind === 'bytes') return <span className="spark" />
  const runs = sparklineRuns(samples.map((s) => numericValue(s.v)), W, H)
  return (
    <svg className="spark" width={W} height={H} viewBox={`0 0 ${W} ${H}`} aria-hidden>
      {runs.map((points, i) => (points.includes(' ') ? <polyline key={i} points={points} /> : <circle key={i} cx={points.split(',')[0]} cy={points.split(',')[1]} r={1.5} />))}
    </svg>
  )
}

function LiveRow({ s, item, samples }: { s: DebugSession; item: LiveItem; samples: LiveSample[] }) {
  const [radix, setRadix] = useState<Radix>('dec')
  const last = samples[samples.length - 1]
  const prev = samples[samples.length - 2]
  const error = item.error ?? last?.e
  const text = error ? '' : formatLiveValue(item.kind, item.size, last?.v, radix)
  const changed = !error && !!prev && prev.v !== last?.v
  const integer = item.kind === 'int' || item.kind === 'uint' || item.kind === 'enum'
  const where = item.address !== undefined ? `0x${item.address.toString(16).padStart(8, '0')} · ${item.size} byte${item.size === 1 ? '' : 's'} · ${item.typeName}` : ''
  const lo = Math.min(...samples.map((x) => numericValue(x.v)).filter((x): x is number => x !== null))
  const hi = Math.max(...samples.map((x) => numericValue(x.v)).filter((x): x is number => x !== null))
  const range = Number.isFinite(lo) ? `\nlast ${samples.length} readings: ${lo} to ${hi}` : ''
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
          { label: 'Stop Watching', run: () => void remove() },
        ])
      }
    >
      <span className="name">
        {item.expression}
        {item.peripheral && <AlertTriangle size={12} className="peri" aria-label="Peripheral register" />}
      </span>
      {error ? <span className="value error" title={error}>{error}</span> : <span className="value" onDoubleClick={() => integer && setRadix(radix === 'dec' ? 'hex' : radix === 'hex' ? 'bin' : 'dec')}>{text}</span>}
      <Sparkline item={item} samples={samples} />
      <IconButton className="remove" size="small" icon={X} label="Stop watching" onClick={() => void remove()} />
    </div>
  )
}

export function LiveWatchView({ s }: { s: DebugSession }) {
  const sess = useLive((l) => l.sessions[s.id]) ?? emptyLive()
  const load = useLive((l) => l.load)
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const input = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const ctl = new AbortController()
    debugApi
      .liveList(s.projectId, s.id, ctl.signal)
      .then((snap) => load(s.id, snap))
      .catch(() => {})
    return () => ctl.abort()
  }, [s.projectId, s.id, load])

  const add = async () => {
    const expression = text.trim()
    if (!expression || busy) return
    setBusy(true)
    try {
      await debugApi.liveAdd(s.projectId, s.id, expression)
      setText('')
      load(s.id, await debugApi.liveList(s.projectId, s.id))
    } catch (e) {
      toastError(e, `Could not watch ${expression}`)
    } finally {
      setBusy(false)
      input.current?.focus()
    }
  }

  const ended = s.state === 'terminated' || s.state === 'failed'
  return (
    <div className="wb-dbg-live">
      <div className="wb-dbg-live-head">
        <Input
          small
          ref={input}
          value={text}
          disabled={busy || ended}
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
          {[...new Set([...INTERVALS, sess.intervalMs])].sort((a, b) => a - b).map((ms) => (
            <option key={ms} value={ms}>
              {ms >= 1000 ? `${ms / 1000} s` : `${ms} ms`}
            </option>
          ))}
        </Select>
      </div>
      <div className="wb-scroll wb-dbg-live-list">
        {sess.items.length === 0 ? (
          <EmptyState title="Nothing watched yet">Type a variable above and press Enter. Its value is read from the running program over and over, without stopping it.</EmptyState>
        ) : (
          sess.items.map((item) => <LiveRow key={item.id} s={s} item={item} samples={sess.samples[item.id] ?? []} />)
        )}
      </div>
      {ended && <div className="wb-dbg-live-note wb-muted wb-small">The session ended: these are the last readings.</div>}
    </div>
  )
}
