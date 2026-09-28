// 'pipeline' panel: header (status, ref, commit, timing, test summary),
// retry/cancel, and the stage columns with job cards; with a test report, a Tests
// tab with the failed tests (TestReport.tsx).

import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Bot, ExternalLink, RefreshCw, RotateCw, Square } from 'lucide-react'
import { toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { Button, EmptyState, ErrorBox, formatDuration, IconButton, Loading, Spacer, Tabs, TimeAgo } from '@/ui'
import { glApi, glk, useGitlabSummary, usePipeline } from './api'
import { Duration, ExtLink, openJob, RefLabel, StatusIcon, StatusText } from './components'
import { askAgentToFix, JobActions } from './JobPanel'
import { isActive, rowKeys, shortSha } from './logic'
import { TestReport } from './TestReport'
import type { Job, PipelineDetail, Stage } from './types'

export interface PipelineParams {
  projectId: string
  pipelineId: number
}

function JobCard({ projectId, projectPath, job, onOpen }: { projectId: string; projectPath: string; job: Job; onOpen: (j: Job) => void }) {
  const failed = job.status === 'failed'
  return (
    <div
      className={failed && !job.allowFailure ? 'gl-job failed' : 'gl-job'}
      onClick={() => onOpen(job)}
      {...rowKeys(() => onOpen(job))}
      title={`${job.name} — open log`}
    >
      <StatusIcon status={job.status} size={15} />
      <span className="name">{job.name}</span>
      <span className="dur">
        <Duration p={job} />
      </span>
      {(job.failureReason || job.allowFailure || job.kind === 'bridge') && (
        <span className="sub">
          {job.kind === 'bridge' && <span>trigger{job.downstreamPipeline ? ` → #${job.downstreamPipeline.id}` : ''}</span>}
          {failed && job.failureReason && <span className="wb-danger">{job.failureReason.replace(/_/g, ' ')}</span>}
          {job.allowFailure && <span>allowed to fail</span>}
        </span>
      )}
      {job.kind !== 'bridge' && (failed || isActive(job.status) || job.status === 'manual') && (
        <span className="actions" onClick={(e) => (e.target as HTMLElement).closest('button') && e.stopPropagation()}>
          <JobActions projectId={projectId} job={job} />
          {failed && (
            <Button size="small" icon={Bot} onClick={() => askAgentToFix(projectId, projectPath, job)} title="Ask the project's agent to fix this job">
              Ask agent
            </Button>
          )}
        </span>
      )}
    </div>
  )
}

/** Stage columns (desktop) or a vertical list of stages (phone). */
export function PipelineGraph({
  projectId,
  projectPath,
  stages,
  vertical,
  onOpenJob,
}: {
  projectId: string
  projectPath: string
  stages: Stage[]
  vertical?: boolean
  onOpenJob: (j: Job) => void
}) {
  if (!stages.length) return <EmptyState title="No jobs in this pipeline" />
  return (
    <div className={vertical ? 'gl-graph vertical' : 'gl-graph'}>
      {stages.map((s) => (
        <div className="gl-stage" key={s.name}>
          <div className="gl-stage-head">
            <StatusIcon status={s.status} size={13} />
            <span className="wb-ellipsis">{s.name}</span>
            <span className="wb-subtle">{s.jobs.length}</span>
          </div>
          {s.jobs.map((j) => (
            <JobCard key={j.id} projectId={projectId} projectPath={projectPath} job={j} onOpen={onOpenJob} />
          ))}
        </div>
      ))}
    </div>
  )
}

export function PipelineHeader({ projectId, d, compact, onShowTests }: { projectId: string; d: PipelineDetail; compact?: boolean; onShowTests?: () => void }) {
  const qc = useQueryClient()
  const [busy, setBusy] = useState<string | null>(null)
  const p = d.pipeline
  const act = async (what: 'retry' | 'cancel') => {
    setBusy(what)
    try {
      await (what === 'retry' ? glApi.retryPipeline(projectId, p.id) : glApi.cancelPipeline(projectId, p.id))
      toast('success', what === 'retry' ? `Retrying pipeline #${p.iid ?? p.id}` : `Canceled pipeline #${p.iid ?? p.id}`)
      qc.invalidateQueries({ queryKey: glk.pipeline(projectId, p.id) })
    } catch (e) {
      toastError(e, `Could not ${what} the pipeline`)
    } finally {
      setBusy(null)
    }
  }
  const t = d.testSummary?.total
  const failedJobs = d.stages.flatMap((s) => s.jobs).filter((j) => j.status === 'failed' && !j.allowFailure).length
  return (
    <div className="gl-pipe-head">
      <div className="line1">
        <StatusIcon status={p.status} size={18} />
        <h2>Pipeline #{p.iid ?? p.id}</h2>
        <StatusText status={p.status} />
        {!compact && p.commitTitle && <span className="wb-ellipsis wb-muted">{p.commitTitle}</span>}
        <Spacer />
        {isActive(p.status) && (
          <Button size="small" icon={Square} loading={busy === 'cancel'} onClick={() => act('cancel')}>
            Cancel
          </Button>
        )}
        {['failed', 'canceled'].includes(p.status) && (
          <Button size="small" icon={RotateCw} loading={busy === 'retry'} onClick={() => act('retry')}>
            Retry failed
          </Button>
        )}
        {!compact && (
          <>
            <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={() => qc.invalidateQueries({ queryKey: glk.pipeline(projectId, p.id) })} />
            <ExtLink href={p.webUrl}>
              <span className="wb-icon-btn small">
                <ExternalLink size={14} />
              </span>
            </ExtLink>
          </>
        )}
      </div>
      <div className="line2">
        <RefLabel name={p.ref} tag={p.tag} />
        <span className="gl-mono">{shortSha(p.sha)}</span>
        {compact && p.commitTitle && <span className="wb-ellipsis">{p.commitTitle}</span>}
        <span className="gl-sep">·</span>
        <span>
          {p.source?.replace(/_/g, ' ') ?? 'pipeline'}
          {p.user ? ` by ${p.user.username}` : ''} <TimeAgo time={p.createdAt} />
        </span>
        <span className="gl-sep">·</span>
        <span>
          <Duration p={p} />
          {p.queuedDuration ? ` (queued ${formatDuration(p.queuedDuration)})` : ''}
        </span>
        {failedJobs > 0 && (
          <>
            <span className="gl-sep">·</span>
            <span className="wb-danger">
              {failedJobs} failed job{failedJobs > 1 ? 's' : ''}
            </span>
          </>
        )}
      </div>
      {t && (
        <div className="gl-tests">
          <span className="wb-muted">Tests</span>
          <span>{t.count} total</span>
          {onShowTests && t.failed + t.error > 0 ? (
            <button type="button" className="gl-tests-link wb-danger" onClick={onShowTests} title="Show the failed tests">
              {t.failed + t.error} failed
            </button>
          ) : (
            <span className={t.failed + t.error > 0 ? 'wb-danger' : 'wb-success'}>{t.failed + t.error} failed</span>
          )}
          <span className="wb-muted">{t.skipped} skipped</span>
          <span className="wb-muted">{formatDuration(t.time)}</span>
        </div>
      )}
      {p.yamlErrors && <div className="wb-small wb-danger">{p.yamlErrors}</div>}
    </div>
  )
}

export function PipelinePanel({ params, setTitle }: PanelProps<PipelineParams>) {
  const { projectId, pipelineId } = params
  const q = usePipeline(projectId, pipelineId)
  const summary = useGitlabSummary(projectId)
  const [tab, setTab] = useState<'jobs' | 'tests'>('jobs')
  const iid = q.data?.pipeline.iid
  useEffect(() => {
    if (iid) setTitle(`Pipeline #${iid}`)
  }, [iid, setTitle])
  if (q.error && !q.data) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!q.data) return <Loading />
  const t = q.data.testSummary?.total
  const failedTests = t ? t.failed + t.error : 0
  const shown = t ? tab : 'jobs'
  return (
    <div className="wb-fill">
      <PipelineHeader projectId={projectId} d={q.data} onShowTests={() => setTab('tests')} />
      {t && (
        <Tabs
          value={shown}
          onChange={setTab}
          tabs={[
            { id: 'jobs', label: 'Jobs' },
            { id: 'tests', label: 'Tests', badge: failedTests > 0 ? <span className="gl-tab-count">{failedTests}</span> : null },
          ]}
        />
      )}
      {shown === 'tests' ? (
        <TestReport projectId={projectId} repoPath={summary.data?.path ?? projectId} pipeline={q.data.pipeline} />
      ) : (
        <PipelineGraph
          projectId={projectId}
          projectPath={summary.data?.path ?? projectId}
          stages={q.data.stages}
          onOpenJob={(j) => openJob(projectId, j.id, j.name)}
        />
      )}
    </div>
  )
}
