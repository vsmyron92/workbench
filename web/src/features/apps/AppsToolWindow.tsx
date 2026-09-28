// The "Apps" tool window (right): environments and run configurations.

import { useState } from 'react'
import { Container, Play, RefreshCw, Rocket, ScanSearch, Square } from 'lucide-react'
import { useProject } from '@/api/queries'
import type { ProjectSummary } from '@/api/types'
import { devcontainerAction } from '@/shell/devcontainerBridge'
import { EmptyState, ErrorBox, IconButton, Loading, Section, Spinner, StatusDot } from '@/ui'
import { checkAllEnvs, redetect, stopAllRuns, useEnvs, useRuns } from './api'
import { EnvCard } from './EnvCard'
import { isActive } from './logic'
import { RunGroups } from './RunList'
import { useAppsPrefs } from './store'

const DC_LABEL = { none: 'not created', stopped: 'stopped', running: 'running', building: 'starting…', error: 'failed' } as const
const DC_TONE = { none: 'muted', stopped: 'warning', running: 'success', building: 'accent', error: 'danger' } as const

/** The project's dev container (devcontainer slice): state, start/stop, its panel. */
export function DevcontainerEntry({ pid, dc, big }: { pid: string; dc: NonNullable<ProjectSummary['devcontainer']>; big?: boolean }) {
  const running = dc.state === 'running'
  return (
    <div className="wb-apps-dc">
      <Container size={big ? 16 : 14} />
      {dc.state === 'building' ? <Spinner size={11} /> : <StatusDot tone={DC_TONE[dc.state]} />}
      <button className="wb-apps-dc-link wb-ellipsis" onClick={() => devcontainerAction(pid, 'panel')} title="Open the Dev container panel">
        Dev container {DC_LABEL[dc.state]}
        {dc.inContainer ? ' · runs go inside' : ''}
      </button>
      <span className="wb-grow" />
      {running ? (
        <button className={big ? 'wb-btn' : 'wb-icon-btn small stop'} onClick={() => devcontainerAction(pid, 'stop')} aria-label="Stop the dev container" title="Stop the dev container">
          <Square size={big ? 15 : 12} className={big ? 'wb-danger' : undefined} />
        </button>
      ) : (
        <button
          className={big ? 'wb-btn' : 'wb-icon-btn small play'}
          disabled={dc.state === 'building' || dc.configs.length === 0}
          onClick={() => devcontainerAction(pid, 'start')}
          aria-label="Start the dev container"
          title="Start the dev container (asks first)"
        >
          <Play size={big ? 16 : 13} className={big ? 'wb-success' : undefined} />
        </button>
      )}
    </div>
  )
}

export function AppsToolWindow({ projectId }: { projectId: string | null }) {
  const project = useProject(projectId)
  const envs = useEnvs(projectId)
  const runs = useRuns(projectId)
  const selected = useAppsPrefs((s) => (projectId ? s.selected[projectId] : undefined))
  const select = useAppsPrefs((s) => s.select)
  const [checking, setChecking] = useState(false)
  if (!projectId) return <EmptyState icon={Rocket} title="No project selected" />
  const activeRuns = runs.data?.filter((r) => isActive(r.state)).length ?? 0
  const warnings = project.data?.summary.warnings ?? []

  return (
    <div className="wb-fill wb-apps">
      <div className="wb-toolbar">
        <IconButton
          size="small"
          icon={RefreshCw}
          label="Check environments now"
          disabled={checking || !envs.data?.length}
          onClick={async () => {
            setChecking(true)
            await checkAllEnvs(projectId)
            setChecking(false)
          }}
        />
        <IconButton size="small" icon={ScanSearch} label="Re-detect run configurations and environments" onClick={() => void redetect()} />
        <IconButton size="small" icon={Square} label="Stop all runs" disabled={!activeRuns} onClick={() => void stopAllRuns(projectId)} />
        <span className="wb-grow" />
        {activeRuns > 0 && <span className="wb-xs wb-muted">{activeRuns} running</span>}
      </div>
      <div className="wb-scroll">
        {warnings.length > 0 && (
          <div className="wb-apps-warnings wb-small">
            {warnings.map((w, i) => (
              <div key={i}>⚠ {w}</div>
            ))}
          </div>
        )}
        {project.data?.summary.devcontainer && <DevcontainerEntry pid={projectId} dc={project.data.summary.devcontainer} />}
        <Section title="Environments" count={envs.data?.length}>
          {envs.isLoading ? (
            <Loading />
          ) : envs.error ? (
            <ErrorBox error={envs.error} onRetry={() => envs.refetch()} />
          ) : envs.data?.length ? (
            <div className="wb-apps-envs">
              {envs.data.map((e) => (
                <EnvCard key={e.name} pid={projectId} env={e} />
              ))}
            </div>
          ) : (
            <div className="wb-apps-empty wb-small wb-muted">
              No environments. Workbench finds them in a Caddyfile and deploy/*.sh, or add <code>[[env]]</code> to{' '}
              <code>.workbench.toml</code>.
            </div>
          )}
        </Section>
        <Section title="Run configurations" count={runs.data?.length}>
          {runs.isLoading ? (
            <Loading />
          ) : runs.error ? (
            <ErrorBox error={runs.error} onRetry={() => runs.refetch()} />
          ) : runs.data?.length ? (
            <RunGroups pid={projectId} runs={runs.data} selected={selected} onSelect={(n) => select(projectId, n)} />
          ) : (
            <div className="wb-apps-empty wb-small wb-muted">
              No run configurations were detected. Add <code>[[run]]</code> entries to <code>.workbench.toml</code>.
            </div>
          )}
        </Section>
      </div>
    </div>
  )
}
