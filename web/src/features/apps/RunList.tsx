// Run configurations, grouped (Development, Tests, Builds, Unity, …, Suggested).

import type { ComponentType } from 'react'
import {
  AlertTriangle,
  AppWindow,
  ChevronDown,
  ChevronRight,
  Container,
  Copy,
  Database,
  Eye,
  FlaskConical,
  Globe,
  Hammer,
  Laptop,
  Play,
  RotateCw,
  Square,
  SquareTerminal,
} from 'lucide-react'
import type { RunKind } from '@/api/types'
import { toast } from '@/shell/actions'
import { showMenu, Spinner, StatusDot, type MenuEntry } from '@/ui'
import { openRunOutput, openRunPreview, setRunOnHost, startRun, stopRun } from './api'
import { groupRuns, isActive, runStateLabel, runTone, runUrl } from './logic'
import { useAppsPrefs } from './store'
import type { RunView } from './types'

export const KIND_ICON: Record<RunKind, ComponentType<{ size?: number; className?: string }>> = {
  server: Globe,
  service: Database,
  test: FlaskConical,
  build: Hammer,
  editor: AppWindow,
  task: SquareTerminal,
}

export function runMenu(pid: string, r: RunView): MenuEntry[] {
  const active = isActive(r.state)
  const url = runUrl(r)
  return [
    active
      ? { label: `Stop ${r.name}`, icon: Square, danger: true, run: () => void stopRun(pid, r.name) }
      : { label: `Run ${r.name}`, icon: Play, run: () => void startRun(pid, r.name) },
    { label: 'Restart', icon: RotateCw, disabled: !active, run: () => void startRun(pid, r.name, true) },
    'separator',
    { label: 'Show output', icon: SquareTerminal, disabled: !r.terminalId, run: () => openRunOutput(r) },
    { label: 'Open preview', icon: Eye, disabled: !url, run: () => openRunPreview(pid, r) },
    'separator',
    // The project runs its runs in its dev container: this one may stay on the host.
    ...((r.inContainer || r.hostPinned
      ? [
          r.hostPinned
            ? { label: 'Run in the dev container', icon: Container, run: () => void setRunOnHost(pid, r.name, false) }
            : { label: 'Always run on the host', icon: Laptop, run: () => void setRunOnHost(pid, r.name, true) },
          'separator',
        ]
      : []) as MenuEntry[]),
    {
      label: 'Copy command',
      icon: Copy,
      run: () =>
        void navigator.clipboard?.writeText(r.config.command).then(
          () => toast('success', 'Command copied'),
          () => toast('error', 'Clipboard is not available'),
        ),
    },
  ]
}

function RunRow({ pid, r, selected, onSelect }: { pid: string; r: RunView; selected?: boolean; onSelect?: (name: string) => void }) {
  const Icon = KIND_ICON[r.config.kind] ?? SquareTerminal
  const active = isActive(r.state)
  const label = runStateLabel(r)
  const tone = runTone(r)
  const url = runUrl(r)
  const tip = [
    r.config.command,
    r.config.cwd !== '.' ? `in ${r.config.cwd}` : null,
    r.config.dependsOn.length ? `after ${r.config.dependsOn.join(', ')}` : null,
    r.config.freePort && r.config.port ? `frees :${r.config.port} before starting (free_port)` : null,
    r.needsConfirm ? 'asks before starting: it may deploy, release or reach a remote host' : null,
    r.inContainer ? `runs in the dev container${r.reach === 'container-ip' ? ' (port reached on the container address)' : ''}` : null,
    r.hostPinned ? 'runs on the host (pinned)' : null,
    r.config.source ? `from ${r.config.source}` : 'from project config',
    r.error ? `⚠ ${r.error}` : null,
  ]
    .filter(Boolean)
    .join('\n')
  return (
    <div
      className={`wb-list-row wb-apps-run${selected ? ' selected' : ''}`}
      title={tip}
      onClick={() => onSelect?.(r.name)}
      onDoubleClick={() => (r.terminalId ? openRunOutput(r) : void startRun(pid, r.name))}
      onContextMenu={(e) => showMenu(e, runMenu(pid, r))}
    >
      <Icon size={14} className={`kind ${r.config.kind}`} />
      <span className="wb-ellipsis name">{r.name}</span>
      {r.inContainer && (
        <span className="wb-apps-ctr" title="Runs in the project's dev container">
          <Container size={10} />
          container
        </span>
      )}
      {r.problems.length > 0 && (
        <span className="wb-warning" title={r.problems.join('\n')}>
          <AlertTriangle size={12} />
        </span>
      )}
      <span className="wb-grow" />
      {r.state === 'starting' && <Spinner size={11} />}
      {label ? (
        <span className={`wb-apps-chip ${tone}`}>
          {r.state !== 'starting' && <StatusDot tone={tone} />}
          {label}
        </span>
      ) : r.portInUse ? (
        <span className="wb-apps-chip warning" title={`Something else listens on :${r.config.port}`}>
          :{r.config.port} busy
        </span>
      ) : null}
      <span className="actions" onClick={(e) => e.stopPropagation()}>
        {url && active && (
          <button className="wb-icon-btn small" title={`Preview ${url}`} aria-label={`Preview ${r.name}`} onClick={() => openRunPreview(pid, r)}>
            <Eye size={13} />
          </button>
        )}
        {r.terminalId && (
          <button className="wb-icon-btn small" title="Show output" aria-label={`Show output of ${r.name}`} onClick={() => openRunOutput(r)}>
            <SquareTerminal size={13} />
          </button>
        )}
        {active && (
          <button className="wb-icon-btn small" title="Restart" aria-label={`Restart ${r.name}`} onClick={() => void startRun(pid, r.name, true)}>
            <RotateCw size={13} />
          </button>
        )}
        {active ? (
          <button className="wb-icon-btn small stop" title="Stop" aria-label={`Stop ${r.name}`} onClick={() => void stopRun(pid, r.name)}>
            <Square size={12} />
          </button>
        ) : (
          <button className="wb-icon-btn small play" title={`Run ${r.name}${r.needsConfirm ? ' (asks first: it may deploy or reach a remote host)' : ''}`} aria-label={`Run ${r.name}`} onClick={() => void startRun(pid, r.name)}>
            <Play size={13} />
          </button>
        )}
      </span>
    </div>
  )
}

export function RunGroups({
  pid,
  runs,
  selected,
  onSelect,
}: {
  pid: string
  runs: RunView[]
  selected?: string | null
  onSelect?: (name: string) => void
}) {
  const groups = groupRuns(runs)
  const prefs = useAppsPrefs((s) => s.groups)
  const setGroup = useAppsPrefs((s) => s.setGroup)
  return (
    <div className="wb-apps-runs">
      {groups.map((g) => {
        const key = `${pid}:${g.id}`
        const open = prefs[key] ?? g.id !== 'suggested'
        const activeCount = g.runs.filter((r) => isActive(r.state)).length
        return (
          <div key={g.id}>
            <div className="wb-apps-group" onClick={() => setGroup(key, !open)}>
              {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
              <span>{g.label}</span>
              <span className="wb-subtle">{g.runs.length}</span>
              {activeCount > 0 && <span className="wb-apps-chip accent">{activeCount} active</span>}
            </div>
            {open && g.runs.map((r) => <RunRow key={r.name} pid={pid} r={r} selected={selected === r.name} onSelect={onSelect} />)}
          </div>
        )
      })}
    </div>
  )
}
