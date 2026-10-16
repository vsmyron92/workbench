// The Data tool window (bottom): what the project's agents and scripts have captured: series,
// the plots made of them, and the files being followed. Select series and press Plot to look at
// them together; an agent makes plots itself and they open next to its session.

import { useEffect, useMemo, useState } from 'react'
import { Activity, ChartLine, Download, FileInput, Plus, Square, Tag, Trash2, Waves, X } from 'lucide-react'
import { confirmDialog, promptDialog, toastError } from '@/shell/actions'
import { Button, EmptyState, ErrorBox, IconButton, Loading, Section, showMenu, StatusDot, type MenuEntry } from '@/ui'
import { exportSeries, newPlotFrom, openPlot } from './actions'
import { dataApi } from './api'
import { formatDuration, formatValue } from './math'
import { useProjectData } from './store'
import type { Kind, PlotConfig, SeriesInfo, SourceInfo } from './types'

const KIND_ICON = { analog: Activity, digital: Waves, events: Tag }
const KIND_TITLE: Record<Kind, string> = { analog: 'Analog: numbers over time', digital: 'Digital: levels over time', events: 'Events: text at a time' }

/** 1234 → "1.2k", 2_500_000 → "2.5M". */
export function compact(n: number): string {
  if (n < 1000) return String(n)
  if (n < 1e6) return `${(n / 1e3).toFixed(n < 1e4 ? 1 : 0)}k`
  return `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0)}M`
}

function lastText(s: SeriesInfo): string {
  if (s.kind === 'events') return s.count ? `${s.count} events` : '—'
  if (s.last === null) return '—'
  return s.kind === 'digital' ? (s.last > 1 ? `0x${Math.round(s.last).toString(16).toUpperCase()}` : String(s.last)) : formatValue(s.last, s.unit)
}

/** Seconds since the series last got samples: "live" while they keep coming. */
function useNow(ms = 1000): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), ms)
    return () => window.clearInterval(id)
  }, [ms])
  return now
}

export function DataToolWindow({ projectId }: { projectId: string | null }) {
  const data = useProjectData(projectId)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const now = useNow()

  // A series that is gone cannot stay selected.
  useEffect(() => {
    setSelected((s) => {
      const names = new Set(data.series.map((x) => x.name))
      const next = new Set([...s].filter((n) => names.has(n)))
      return next.size === s.size ? s : next
    })
  }, [data.series])

  const groups = useMemo(() => {
    const m = new Map<string, SeriesInfo[]>()
    for (const s of data.series) {
      const g = s.group || ''
      if (!m.has(g)) m.set(g, [])
      m.get(g)!.push(s)
    }
    return [...m.entries()].sort((a, b) => (a[0] === '' ? 1 : b[0] === '' ? -1 : a[0].localeCompare(b[0])))
  }, [data.series])

  if (!projectId) return <EmptyState icon={ChartLine} title="No project selected" />
  if (data.error && !data.loaded) return <ErrorBox error={new Error(data.error)} />
  if (!data.loaded) return <Loading />

  const names = [...selected]
  const toggle = (n: string, on?: boolean) =>
    setSelected((s) => {
      const next = new Set(s)
      if (on ?? !next.has(n)) next.add(n)
      else next.delete(n)
      return next
    })

  const plotSelected = () => void newPlotFrom(projectId, names)
  const clear = (list: string[]) =>
    void dataApi
      .clear(projectId, list)
      .catch(toastError)
  const remove = async (list: string[]) => {
    const ok = await confirmDialog({ title: `Delete ${list.length === 1 ? list[0] : `${list.length} series`}?`, message: 'Their samples go. Plots that show them stay and wait for new ones.', confirmLabel: 'Delete', danger: true })
    if (ok) void dataApi.remove(projectId, list).catch(toastError)
  }

  const seriesMenu = (e: React.MouseEvent, s: SeriesInfo) => {
    const target = selected.has(s.name) ? names : [s.name]
    const items: MenuEntry[] = [
      { label: target.length > 1 ? `Plot ${target.length} series` : 'Plot', icon: ChartLine, run: () => void newPlotFrom(projectId, target) },
      ...data.plots.slice(0, 12).map<MenuEntry>((p) => ({ label: `Add to “${p.title}”`, run: () => void addTo(p, target) })),
      'separator',
      { label: 'Save as CSV…', icon: Download, run: () => void exportSeries(projectId, target, undefined, undefined, target[0]) },
      { label: 'Set unit…', run: () => void setUnit(s) },
      { label: 'Clear samples', run: () => clear(target) },
      { label: 'Delete', icon: Trash2, danger: true, run: () => void remove(target) },
    ]
    showMenu(e, items)
  }

  const addTo = async (p: PlotConfig, list: string[]) => {
    const { addSeriesTo } = await import('./actions')
    const infos = new Map(data.series.map((s) => [s.name, s]))
    let next = p
    for (const n of list) {
      const i = infos.get(n)
      if (i) next = addSeriesTo(next, i, (x) => infos.get(x))
    }
    await dataApi.putPlot(projectId, next).catch(toastError)
    openPlot(projectId, next)
  }

  const setUnit = async (s: SeriesInfo) => {
    const unit = await promptDialog({ title: `Unit of ${s.name}`, label: 'For example A, mA, V, W, °C (SI prefixes are applied for you)', initial: s.unit })
    if (unit !== null) void dataApi.define(projectId, { name: s.name, unit: unit.trim() }).catch(toastError)
  }

  const followFile = async () => {
    const path = await promptDialog({
      title: 'Follow a file',
      label: 'A CSV, JSON lines or VCD file inside the project, written by a capture tool (read live while it grows)',
      placeholder: 'captures/power.csv',
      confirmLabel: 'Follow',
    })
    if (!path?.trim()) return
    try {
      await dataApi.follow(projectId, { path: path.trim() })
    } catch (e) {
      toastError(e)
    }
  }

  const nothing = !data.series.length && !data.plots.length && !data.sources.length
  return (
    <div className="wb-fill wb-data">
      <div className="wb-data-toolbar">
        <Button size="small" icon={ChartLine} disabled={!names.length} onClick={plotSelected} title="Draw the selected series together in a new plot">
          Plot{names.length > 1 ? ` ${names.length}` : ''}
        </Button>
        <IconButton size="small" icon={Download} label="Save the selected series as CSV" disabled={!names.length} onClick={() => void exportSeries(projectId, names, undefined, undefined, names[0])} />
        <IconButton size="small" icon={Square} label="Clear the samples of the selected series" disabled={!names.length} onClick={() => clear(names)} />
        <IconButton size="small" icon={Trash2} label="Delete the selected series" disabled={!names.length} onClick={() => void remove(names)} />
        <span className="grow" />
        <Button size="small" variant="ghost" icon={FileInput} onClick={() => void followFile()} title="Read a capture file live">
          Follow file…
        </Button>
      </div>
      <div className="wb-scroll wb-data-body">
        {nothing ? (
          <EmptyState icon={Waves} title="No measurements yet">
            An agent puts them here as it captures: ask one to record a power profile, a logic analyzer trace or a probe waveform. Plots open next to its session and fill live.
          </EmptyState>
        ) : (
          <>
            <Section title="Series" count={data.series.length}>
              {groups.map(([group, list]) => (
                <div key={group || '·'}>
                  {(group || groups.length > 1) && <div className="wb-data-group">{group || 'Ungrouped'}</div>}
                  {list.map((s) => {
                    const Icon = KIND_ICON[s.kind]
                    const live = s.updatedAt > 0 && now - s.updatedAt < 2000
                    const span = s.tStart !== null && s.tEnd !== null ? s.tEnd - s.tStart : null
                    return (
                      <div
                        key={s.name}
                        className={['wb-list-row', 'wb-data-row', selected.has(s.name) && 'selected'].filter(Boolean).join(' ')}
                        onClick={(e) => (e.ctrlKey || e.metaKey || e.shiftKey ? toggle(s.name) : setSelected(new Set([s.name])))}
                        onDoubleClick={() => void newPlotFrom(projectId, [s.name])}
                        onContextMenu={(e) => seriesMenu(e, s)}
                        title={`${s.name}${s.label ? `\n${s.label}` : ''}\n${KIND_TITLE[s.kind]}\n${compact(s.appended)} samples taken${s.dropped ? `, ${compact(s.dropped)} rolled off` : ''}`}
                      >
                        <input type="checkbox" checked={selected.has(s.name)} aria-label={`Select ${s.name}`} onChange={(e) => toggle(s.name, e.target.checked)} onClick={(e) => e.stopPropagation()} />
                        <Icon size={13} className="kind" aria-hidden />
                        <span className="name">{s.name}</span>
                        {s.unit && <span className="unit">{s.unit}</span>}
                        <span className="grow" />
                        <span className="last">{lastText(s)}</span>
                        <span className="count">{compact(s.count)}</span>
                        <span className="span">{span !== null && span > 0 ? formatDuration(span) : ''}</span>
                        {live && <StatusDot tone="success" pulse title="Receiving samples" />}
                      </div>
                    )
                  })}
                </div>
              ))}
              {!data.series.length && <div className="wb-data-note">No series yet.</div>}
            </Section>

            <Section title="Plots" count={data.plots.length}>
              {data.plots.map((p) => (
                <div key={p.id} className="wb-list-row wb-data-row" onClick={() => openPlot(projectId, p)} title={`Open ${p.title}`}>
                  <ChartLine size={13} className="kind" aria-hidden />
                  <span className="name">{p.title}</span>
                  <span className="grow" />
                  <span className="count">
                    {p.lanes.length} lane{p.lanes.length === 1 ? '' : 's'} · {p.lanes.reduce((n, l) => n + l.traces.length, 0)} series
                  </span>
                  <IconButton
                    size="small"
                    icon={X}
                    label={`Delete plot ${p.title}`}
                    onClick={(e) => {
                      e.stopPropagation()
                      void dataApi.deletePlot(projectId, p.id).catch(toastError)
                    }}
                  />
                </div>
              ))}
              {!data.plots.length && <div className="wb-data-note">No plots yet. Select series and press Plot.</div>}
            </Section>

            <Section
              title="Followed files"
              count={data.sources.length}
              actions={<IconButton size="small" icon={Plus} label="Follow a file…" onClick={() => void followFile()} />}
            >
              {data.sources.map((s) => (
                <SourceRow key={s.id} s={s} projectId={projectId} />
              ))}
              {!data.sources.length && <div className="wb-data-note">A capture tool’s CSV, JSON lines or VCD file is read as it is written.</div>}
            </Section>
          </>
        )}
      </div>
    </div>
  )
}

function SourceRow({ s, projectId }: { s: SourceInfo; projectId: string }) {
  const running = s.state === 'running' || s.state === 'waiting'
  const tone = s.state === 'error' ? 'danger' : s.state === 'running' ? 'success' : s.state === 'waiting' ? 'warning' : 'muted'
  const text = s.state === 'error' ? s.error : s.state === 'waiting' ? 'waiting for the file' : `${compact(s.rows)} rows${s.skipped ? ` · ${s.skipped} skipped` : ''}${s.state === 'done' ? ' · imported' : ''}${s.state === 'stopped' ? ' · stopped' : ''}`
  return (
    <div className="wb-list-row wb-data-row" title={`${s.path}\n${s.series.length} series · ${s.format}`}>
      <StatusDot tone={tone} pulse={s.state === 'running'} />
      <span className="name">{s.path}</span>
      <span className="grow" />
      <span className={`count ${s.state === 'error' ? 'err' : ''}`}>{text}</span>
      {running ? (
        <IconButton size="small" icon={Square} label="Stop reading this file" onClick={() => void dataApi.stopSource(projectId, s.id).catch(toastError)} />
      ) : (
        <IconButton size="small" icon={X} label="Remove from the list" onClick={() => void dataApi.removeSource(projectId, s.id).catch(toastError)} />
      )}
    </div>
  )
}
