// Top bar widgets: the CLion-style run selector ([config ▾] ▶ ■ ↻) and environment
// status pills. Status bar: environment dots and a running-runs indicator.

import { AlertTriangle, ChevronDown, Play, RotateCw, Square } from 'lucide-react'
import { showToolWindow } from '@/shell/actions'
import { showMenuAt, Spinner, StatusDot, timeAgo, type MenuEntry } from '@/ui'
import { startRun, stopRun, useEnvs, useRuns } from './api'
import { defaultRun, envShortLabel, formatLatency, groupRuns, healthTone, isActive, pillEnvs, runStateLabel, runTone, shortSha } from './logic'
import { KIND_ICON } from './RunList'
import { useAppsPrefs } from './store'
import type { EnvView } from './types'

export function RunSelector({ projectId }: { projectId: string | null }) {
  const runs = useRuns(projectId)
  const selectedName = useAppsPrefs((s) => (projectId ? s.selected[projectId] : undefined))
  const select = useAppsPrefs((s) => s.select)
  if (!projectId || !runs.data?.length) return null
  const list = runs.data
  const current = list.find((r) => r.name === selectedName) ?? list.find((r) => r.name === defaultRun(list)) ?? null
  if (!current) return null
  const Icon = KIND_ICON[current.config.kind]
  const active = isActive(current.state)
  const tone = runTone(current)
  // Why a start would fail (a program that is not installed…), shown before the click.
  const problems = current.problems.map((p) => `⚠ ${p}`)

  const open = (el: HTMLElement) => {
    const items: MenuEntry[] = []
    for (const g of groupRuns(list)) {
      if (g.id === 'suggested') continue
      if (items.length) items.push('separator')
      for (const r of g.runs) {
        const st = runStateLabel(r)
        items.push({ label: st ? `${r.name}  —  ${st}` : r.name, icon: KIND_ICON[r.config.kind], run: () => select(projectId, r.name) })
      }
    }
    const suggested = list.filter((r) => r.config.group === 'suggested')
    if (suggested.length) {
      items.push('separator')
      for (const r of suggested.slice(0, 12)) items.push({ label: `Suggested: ${r.name}`, icon: KIND_ICON.task, run: () => select(projectId, r.name) })
    }
    items.push('separator', { label: 'Edit configurations…', run: () => showToolWindow('apps') })
    showMenuAt(el, items)
  }

  return (
    <div className="wb-apps-runbar" role="group" aria-label="Run configuration">
      <button className="wb-topbar-widget wb-apps-runsel" onClick={(e) => open(e.currentTarget)} title={[current.config.command, runStateLabel(current) || 'stopped', ...problems].join('\n')}>
        <span className="icon">
          <Icon size={14} />
          {current.state !== 'stopped' && (
            <span className="dot">
              <StatusDot tone={tone} pulse={current.state === 'starting'} />
            </span>
          )}
        </span>
        <span className="wb-ellipsis">{current.name}</span>
        {problems.length > 0 && <AlertTriangle size={12} className="wb-warning" aria-label="Problems" />}
        <ChevronDown size={13} className="wb-muted" />
      </button>
      {active ? (
        <button className="wb-icon-btn wb-apps-rerun" title={`Restart ${current.name}`} aria-label={`Restart ${current.name}`} onClick={() => void startRun(projectId, current.name, true)}>
          <RotateCw size={15} />
        </button>
      ) : (
        <button className="wb-icon-btn play" title={[`Run ${current.name}${current.needsConfirm ? ' (asks first)' : ''}`, ...problems].join('\n')} aria-label={`Run ${current.name}`} onClick={() => void startRun(projectId, current.name)}>
          <Play size={16} />
        </button>
      )}
      <button className="wb-icon-btn stop" title={`Stop ${current.name}`} aria-label={`Stop ${current.name}`} disabled={!active} onClick={() => void stopRun(projectId, current.name)}>
        {current.state === 'starting' ? <Spinner size={12} /> : <Square size={14} />}
      </button>
    </div>
  )
}

export function envTooltip(e: EnvView): string {
  const h = e.health
  return [
    `${e.name}: ${h.status}${h.httpStatus ? ` (HTTP ${h.httpStatus})` : ''}${h.latencyMs != null ? ` · ${formatLatency(h.latencyMs)}` : ''}`,
    e.url,
    h.checkedAt ? `checked ${timeAgo(h.checkedAt)}` : null,
    e.version?.sha ? `version ${shortSha(e.version.sha)}` : null,
    h.error ? `⚠ ${h.error}` : null,
  ]
    .filter(Boolean)
    .join('\n')
}

export function EnvPills({ projectId }: { projectId: string | null }) {
  const envs = useEnvs(projectId)
  if (!projectId || !envs.data?.length) return null
  return (
    <div className="wb-apps-pills">
      {pillEnvs(envs.data).map((e) => (
        <button key={e.name} className={`wb-apps-pill ${healthTone(e.health.status)}`} title={envTooltip(e)} onClick={() => showToolWindow('apps')}>
          <StatusDot tone={healthTone(e.health.status)} pulse={!!e.deploying} />
          {envShortLabel(e)}
        </button>
      ))}
    </div>
  )
}

export function StatusbarApps({ projectId }: { projectId: string | null }) {
  const envs = useEnvs(projectId)
  const runs = useRuns(projectId)
  if (!projectId) return null
  const active = runs.data?.filter((r) => isActive(r.state)) ?? []
  return (
    <>
      {active.length > 0 && (
        <button
          className="wb-status-item"
          title={active.map((r) => `${r.name}: ${runStateLabel(r)}`).join('\n')}
          onClick={() => showToolWindow('run')}
        >
          <Play size={12} className="wb-success" />
          {active.length === 1 ? active[0].name : `${active.length} running`}
        </button>
      )}
      {envs.data?.map((e) => (
        <button key={e.name} className="wb-status-item" title={envTooltip(e)} onClick={() => showToolWindow('apps')}>
          <StatusDot tone={healthTone(e.health.status)} />
          {envShortLabel(e)}
        </button>
      ))}
    </>
  )
}
