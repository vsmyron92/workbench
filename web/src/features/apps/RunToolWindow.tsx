// The "Run" tool window (bottom): this project's run terminals and environment
// commands (deploys, logs) on the left; the selected run's details and test
// results on the right. The terminal view itself belongs to the terminals slice.

import { useState } from 'react'
import { AlertTriangle, CheckCircle2, CircleMinus, Eye, Play, RotateCw, ScrollText, Square, SquareTerminal, XCircle } from 'lucide-react'
import { useTerminals } from '@/api/queries'
import type { TerminalInfo } from '@/api/types'
import { Button, EmptyState, StatusDot, TimeAgo } from '@/ui'
import { openRunOutput, openRunPreview, openTerminal, startRun, stopRun, useRuns } from './api'
import { isActive, runStateLabel, runTone, runUrl } from './logic'
import { KIND_ICON } from './RunList'
import type { ResultItem, RunView } from './types'

const ITEM_ICON = { passed: CheckCircle2, failed: XCircle, skipped: CircleMinus }
const ITEM_CLASS = { passed: 'wb-success', failed: 'wb-danger', skipped: 'wb-subtle' }

function Results({ r }: { r: RunView }) {
  const res = r.result
  if (!res) return null
  const items: ResultItem[] = [...(res.items ?? [])].sort((a, b) => (a.status === 'failed' ? -1 : 0) - (b.status === 'failed' ? -1 : 0))
  return (
    <div className="wb-apps-results">
      <div className="wb-row summary">
        <CheckCircle2 size={14} className="wb-success" /> {res.passed} passed
        <XCircle size={14} className={res.failed ? 'wb-danger' : 'wb-subtle'} /> {res.failed} failed
        {res.truncated && <span className="wb-xs wb-subtle">(first {items.length} shown)</span>}
      </div>
      {items.map((it, i) => {
        const I = ITEM_ICON[it.status]
        return (
          <div key={i} className="wb-apps-result" title={it.detail}>
            <I size={13} className={ITEM_CLASS[it.status]} />
            <span className="wb-ellipsis">{it.name}</span>
            {it.detail && <span className="wb-ellipsis wb-subtle detail">{it.detail}</span>}
          </div>
        )
      })}
    </div>
  )
}

function Details({ pid, r }: { pid: string; r: RunView }) {
  const active = isActive(r.state)
  const url = runUrl(r)
  return (
    <div className="wb-apps-details">
      <div className="wb-row">
        <StatusDot tone={runTone(r)} pulse={r.state === 'starting'} />
        <b className="wb-ellipsis">{r.name}</b>
        <span className="wb-muted wb-small">{runStateLabel(r) || r.state}</span>
        <span className="wb-grow" />
        {active ? (
          <>
            <Button size="small" icon={RotateCw} aria-label={`Restart ${r.name}`} onClick={() => void startRun(pid, r.name, true)}>
              Restart
            </Button>
            <Button size="small" icon={Square} aria-label={`Stop ${r.name}`} onClick={() => void stopRun(pid, r.name)}>
              Stop
            </Button>
          </>
        ) : (
          <Button size="small" icon={Play} aria-label={`Run ${r.name}`} onClick={() => void startRun(pid, r.name)}>
            Run
          </Button>
        )}
        {url && active && (
          <Button size="small" icon={Eye} onClick={() => openRunPreview(pid, r)}>
            Preview
          </Button>
        )}
        <Button size="small" icon={SquareTerminal} disabled={!r.terminalId} onClick={() => openRunOutput(r)}>
          Output
        </Button>
      </div>
      <code className="wb-apps-cmd">{r.config.command}</code>
      <div className="wb-xs wb-muted">
        {r.config.cwd !== '.' && <>in {r.config.cwd} · </>}
        {r.startedAt ? (
          <>
            started <TimeAgo time={r.startedAt} />
          </>
        ) : (
          'never started'
        )}
        {r.url && <> · {r.url}</>}
      </div>
      {r.error && <div className="wb-small wb-danger">{r.error}</div>}
      {!active &&
        r.problems.map((p) => (
          <div key={p} className="wb-small wb-warning wb-row">
            <AlertTriangle size={12} /> {p}
          </div>
        ))}
      <Results r={r} />
    </div>
  )
}

export function RunToolWindow({ projectId }: { projectId: string | null }) {
  const runs = useRuns(projectId)
  const terms = useTerminals()
  const [sel, setSel] = useState<string | null>(null)
  if (!projectId) return <EmptyState title="No project selected" />
  const all = runs.data ?? []
  // Runs that have (or had) a terminal in this session, active first.
  const withOutput = all.filter((r) => r.terminalId || isActive(r.state)).sort((a, b) => Number(isActive(b.state)) - Number(isActive(a.state)) || (b.startedAt ?? 0) - (a.startedAt ?? 0))
  const envTerms: TerminalInfo[] = (terms.data ?? [])
    .filter((t) => t.projectId === projectId && t.kind === 'command' && typeof t.meta?.env === 'string')
    .sort((a, b) => b.createdAt - a.createdAt)
    .slice(0, 20)
  const selected = all.find((r) => r.name === sel) ?? withOutput[0] ?? null

  if (!withOutput.length && !envTerms.length) {
    return (
      <EmptyState icon={Play} title="Nothing has run yet">
        Start a run configuration from the top bar or the Apps tool window.
      </EmptyState>
    )
  }
  return (
    <div className="wb-apps-runwin">
      <div className="wb-apps-runwin-list">
        {withOutput.map((r) => {
          const Icon = KIND_ICON[r.config.kind]
          return (
            <div
              key={r.name}
              className={`wb-list-row${selected?.name === r.name ? ' selected' : ''}`}
              title={r.terminalId ? 'Show output' : undefined}
              onClick={() => {
                setSel(r.name)
                if (r.terminalId) openRunOutput(r)
              }}
            >
              <StatusDot tone={runTone(r)} pulse={r.state === 'starting'} />
              <Icon size={13} />
              <span className="wb-ellipsis">{r.name}</span>
              <span className="wb-grow" />
              <span className="wb-xs wb-muted">{runStateLabel(r) || r.state}</span>
            </div>
          )
        })}
        {envTerms.length > 0 && <div className="wb-apps-group static">Environments</div>}
        {envTerms.map((t) => (
          <div key={t.id} className="wb-list-row" onClick={() => openTerminal(t.id, t.title)} title={t.argv.join(' ')}>
            <StatusDot tone={t.status === 'exited' ? (t.exit?.code === 0 ? 'success' : 'danger') : 'accent'} pulse={t.status !== 'exited'} />
            <ScrollText size={13} />
            <span className="wb-ellipsis">{t.title}</span>
            <span className="wb-grow" />
            <span className="wb-xs wb-muted">
              <TimeAgo time={t.createdAt} />
            </span>
          </div>
        ))}
      </div>
      <div className="wb-apps-runwin-detail">{selected ? <Details pid={projectId} r={selected} /> : <EmptyState title="Select a run" />}</div>
    </div>
  )
}
