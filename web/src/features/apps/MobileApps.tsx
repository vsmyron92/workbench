// Phone tab: environment health at a glance, and run configurations to start/stop.

import { ExternalLink, Play, Rocket, Square } from 'lucide-react'
import { useProject } from '@/api/queries'
import { EmptyState, ErrorBox, Loading, Spinner, StatusDot, TimeAgo } from '@/ui'
import { openExternal, startRun, stopRun, useEnvs, useRuns } from './api'
import { DevcontainerEntry } from './AppsToolWindow'
import { formatLatency, groupRuns, healthTone, isActive, runStateLabel, runTone, shortSha } from './logic'

export function MobileApps({ projectId }: { projectId: string | null }) {
  const envs = useEnvs(projectId)
  const runs = useRuns(projectId)
  const project = useProject(projectId)
  if (!projectId) return <EmptyState icon={Rocket} title="No project selected" />
  const groups = groupRuns((runs.data ?? []).filter((r) => r.config.group !== 'suggested'))
  const dc = project.data?.summary.devcontainer
  return (
    <div className="wb-fill wb-scroll wb-apps-mobile">
      {dc && (
        <>
          <div className="wb-section-header">Dev container</div>
          <DevcontainerEntry pid={projectId} dc={dc} big />
        </>
      )}
      <div className="wb-section-header">Environments</div>
      {envs.isLoading && <Loading />}
      {envs.error && <ErrorBox error={envs.error} onRetry={() => envs.refetch()} />}
      {envs.data?.length === 0 && <div className="wb-pad wb-small wb-muted">No environments.</div>}
      {envs.data?.map((e) => (
        <div key={e.name} className="wb-apps-mrow">
          <StatusDot tone={healthTone(e.health.status)} />
          <div className="wb-grow">
            <div className="wb-row">
              <b>{e.name}</b>
              {e.version?.sha && <span className="mono wb-xs wb-muted">{shortSha(e.version.sha)}</span>}
            </div>
            <div className="wb-xs wb-muted">
              {e.health.status}
              {e.health.latencyMs != null && ` · ${formatLatency(e.health.latencyMs)}`}
              {e.health.checkedAt && (
                <>
                  {' · '}
                  <TimeAgo time={e.health.checkedAt} />
                </>
              )}
            </div>
          </div>
          <button className="wb-btn" onClick={() => openExternal(e.url)} aria-label={`Open ${e.name}`}>
            <ExternalLink size={16} />
          </button>
        </div>
      ))}
      <div className="wb-section-header">Run configurations</div>
      {runs.isLoading && <Loading />}
      {runs.error && <ErrorBox error={runs.error} onRetry={() => runs.refetch()} />}
      {groups.map((g) =>
        g.runs.map((r) => {
          const active = isActive(r.state)
          return (
            <div key={r.name} className="wb-apps-mrow">
              <StatusDot tone={runTone(r)} pulse={r.state === 'starting'} />
              <div className="wb-grow" style={{ minWidth: 0 }}>
                <div className="wb-ellipsis">
                  {r.name}
                  {r.inContainer && <span className="wb-apps-ctr" style={{ marginLeft: 6 }}>container</span>}
                </div>
                <div className="wb-xs wb-muted wb-ellipsis">{runStateLabel(r) || g.label}</div>
              </div>
              {r.state === 'starting' && <Spinner size={14} />}
              {active ? (
                <button className="wb-btn" onClick={() => void stopRun(projectId, r.name)} aria-label={`Stop ${r.name}`}>
                  <Square size={15} className="wb-danger" />
                </button>
              ) : (
                <button className="wb-btn" onClick={() => void startRun(projectId, r.name)} aria-label={`Run ${r.name}`}>
                  <Play size={16} className="wb-success" />
                </button>
              )}
            </div>
          )
        }),
      )}
    </div>
  )
}
