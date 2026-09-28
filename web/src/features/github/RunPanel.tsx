// 'gh.run' panel: a workflow run — header (state, workflow, branch, commit,
// event, timing), re-run / re-run failed / cancel, and its jobs (matrix jobs
// grouped) with the step that failed. A failed job can go to the agent. Below them,
// the run's artifacts, downloaded through Workbench.

import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Bot, Download, ExternalLink, Package, RefreshCw, RotateCw, Square } from 'lucide-react'
import { toast, toastError } from '@/shell/actions'
import { askAgent } from '@/shell/agentBridge'
import type { PanelProps } from '@/shell/types'
import { Button, EmptyState, ErrorBox, formatBytes, IconButton, Loading, Spacer, TimeAgo } from '@/ui'
import { fetchAnnotations, fetchLogTail, ghApi, ghk, useGithubSummary, useRun, useRunArtifacts } from './api'
import { Duration, ExtLink, openJob, openPr, RefLabel, runTitle, StatusIcon, StatusText } from './components'
import { eventLabel, expiresIn, ghLabel, groupJobs, isActive, jobFixPrompt, rowKeys, shortSha } from './logic'
import type { Job, RunDetail } from './types'

export interface RunParams {
  projectId: string
  runId: number
}

const NEEDS_TOKEN = 'Needs a GitHub token'

/** Ask the project's agent to fix a failed job (with the end of its log, or its annotations). */
export async function askAgentToFix(projectId: string, repo: string, job: Job, runName?: string | null) {
  try {
    const tail = await fetchLogTail(projectId, job.id, 150)
    const annotations = tail.available ? [] : await fetchAnnotations(projectId, job.id).catch(() => [])
    await askAgent({
      projectId,
      prompt: jobFixPrompt({
        repo,
        job,
        runName,
        log: tail.available ? tail.text : null,
        shownLines: Math.min(150, tail.totalLines),
        totalLines: tail.totalLines,
        annotations,
      }),
    })
  } catch (e) {
    toastError(e, 'Could not read the job log')
  }
}

export function RerunJobButton({ projectId, job, anonymous, compact }: { projectId: string; job: Job; anonymous: boolean; compact?: boolean }) {
  const qc = useQueryClient()
  const [busy, setBusy] = useState(false)
  if (job.status !== 'completed') return null
  const run = async () => {
    setBusy(true)
    try {
      await ghApi.rerunJob(projectId, job.id)
      toast('success', `Re-running ${job.name}`)
      void qc.invalidateQueries({ queryKey: ghk.run(projectId, job.runId) })
      void qc.invalidateQueries({ queryKey: ghk.job(projectId, job.id) })
    } catch (e) {
      toastError(e, 'Could not re-run the job')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Button size="small" icon={RotateCw} loading={busy} disabled={anonymous} title={anonymous ? NEEDS_TOKEN : 'Re-run this job (and the jobs that need it)'} onClick={run}>
      {compact ? '' : 'Re-run'}
    </Button>
  )
}

function JobCard({ projectId, repo, runName, job, anonymous, onOpen }: { projectId: string; repo: string; runName: string | null; job: Job; anonymous: boolean; onOpen: (j: Job) => void }) {
  const failed = job.state === 'failed'
  const failedStep = job.steps.find((s) => s.state === 'failed')
  const current = job.steps.find((s) => s.state === 'running')
  return (
    <div className={failed ? 'gh-job failed' : 'gh-job'} onClick={() => onOpen(job)} {...rowKeys(() => onOpen(job))} title={`${job.name} — open`}>
      <StatusIcon state={job.state} size={15} title={ghLabel(job.status, job.conclusion)} />
      <span className="name">{job.name}</span>
      <span className="dur">
        <Duration p={job} />
      </span>
      {(failedStep || current || job.state === 'manual') && (
        <span className="sub">
          {failedStep && <span className="wb-danger wb-ellipsis">✗ {failedStep.name}</span>}
          {current && <span className="wb-ellipsis">● {current.name}</span>}
          {job.state === 'manual' && <span>waiting for approval</span>}
        </span>
      )}
      {failed && (
        <span className="actions" onClick={(e) => (e.target as HTMLElement).closest('button') && e.stopPropagation()}>
          <RerunJobButton projectId={projectId} job={job} anonymous={anonymous} />
          <Button size="small" icon={Bot} onClick={() => askAgentToFix(projectId, repo, job, runName)} title="Ask the project's agent to fix this job">
            Ask agent
          </Button>
        </span>
      )}
    </div>
  )
}

/** Jobs as a grid of groups (matrix jobs together); one column on phones. */
export function JobGrid({
  projectId,
  repo,
  d,
  anonymous,
  vertical,
  onOpenJob,
}: {
  projectId: string
  repo: string
  d: RunDetail
  anonymous: boolean
  vertical?: boolean
  onOpenJob: (j: Job) => void
}) {
  if (!d.jobs.length) {
    return <EmptyState title={isActive(d.run.state) ? 'Waiting for jobs to start…' : 'No jobs in this run'} />
  }
  const groups = groupJobs(d.jobs)
  return (
    <div className={vertical ? 'gh-jobs vertical' : 'gh-jobs'}>
      {groups.map((g) => (
        <div className="gh-group" key={g.name}>
          {g.jobs.length > 1 && (
            <div className="gh-group-head">
              <StatusIcon state={g.state} size={13} />
              <span className="wb-ellipsis">{g.name}</span>
              <span className="wb-subtle">{g.jobs.length}</span>
            </div>
          )}
          {g.jobs.map((j) => (
            <JobCard key={j.id} projectId={projectId} repo={repo} runName={d.run.name} job={j} anonymous={anonymous} onOpen={onOpenJob} />
          ))}
        </div>
      ))}
      {d.truncated && <div className="wb-small wb-warning">More jobs exist than are shown.</div>}
    </div>
  )
}

export function RunHeader({ projectId, d, anonymous, compact }: { projectId: string; d: RunDetail; anonymous: boolean; compact?: boolean }) {
  const qc = useQueryClient()
  const [busy, setBusy] = useState<string | null>(null)
  const r = d.run
  const act = async (what: 'rerun' | 'rerunFailed' | 'cancel') => {
    setBusy(what)
    try {
      await (what === 'rerun' ? ghApi.rerun(projectId, r.id) : what === 'rerunFailed' ? ghApi.rerunFailed(projectId, r.id) : ghApi.cancel(projectId, r.id))
      toast('success', what === 'cancel' ? `Cancelling ${runTitle(r, r.id)}` : `Re-running ${what === 'rerunFailed' ? 'the failed jobs of ' : ''}${runTitle(r, r.id)}`)
      void qc.invalidateQueries({ queryKey: ghk.run(projectId, r.id) })
    } catch (e) {
      toastError(e, `Could not ${what === 'cancel' ? 'cancel' : 're-run'} the run`)
    } finally {
      setBusy(null)
    }
  }
  const failedJobs = d.jobs.filter((j) => j.state === 'failed').length
  const tokenTitle = anonymous ? NEEDS_TOKEN : undefined
  return (
    <div className="gh-run-head">
      <div className="line1">
        <StatusIcon state={r.state} size={18} title={ghLabel(r.status, r.conclusion)} />
        <h2>{runTitle(r, r.id)}</h2>
        <StatusText state={r.state} status={r.status} conclusion={r.conclusion} />
        {!compact && (r.displayTitle || r.commitTitle) && <span className="wb-ellipsis wb-muted">{r.displayTitle ?? r.commitTitle}</span>}
        <Spacer />
        {isActive(r.state) && (
          <Button size="small" icon={Square} loading={busy === 'cancel'} disabled={anonymous} title={tokenTitle} onClick={() => act('cancel')}>
            Cancel
          </Button>
        )}
        {r.status === 'completed' && (r.state === 'failed' || r.state === 'canceled') && (
          <Button size="small" icon={RotateCw} loading={busy === 'rerunFailed'} disabled={anonymous} title={tokenTitle} onClick={() => act('rerunFailed')}>
            {compact ? 'Failed' : 'Re-run failed'}
          </Button>
        )}
        {r.status === 'completed' && (
          <Button size="small" icon={RotateCw} loading={busy === 'rerun'} disabled={anonymous} title={tokenTitle} onClick={() => act('rerun')}>
            {compact ? 'All' : 'Re-run all'}
          </Button>
        )}
        {!compact && (
          <>
            <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={() => qc.invalidateQueries({ queryKey: ghk.run(projectId, r.id) })} />
            <ExtLink href={r.htmlUrl}>
              <span className="wb-icon-btn small">
                <ExternalLink size={14} />
              </span>
            </ExtLink>
          </>
        )}
      </div>
      {compact && (r.displayTitle || r.commitTitle) && <div className="wb-small wb-ellipsis">{r.displayTitle ?? r.commitTitle}</div>}
      <div className="line2">
        {r.headBranch && <RefLabel name={r.headBranch} />}
        <span className="gh-mono">{shortSha(r.headSha)}</span>
        <span className="gh-sep">·</span>
        <span>
          {eventLabel(r.event)}
          {r.actor ? ` by ${r.actor.login}` : ''} <TimeAgo time={r.createdAt} />
        </span>
        {r.pullRequests.map((p) => (
          <a key={p.number} className="gh-link" href="#" onClick={(e) => (e.preventDefault(), openPr(projectId, p.number))}>
            #{p.number}
          </a>
        ))}
        <span className="gh-sep">·</span>
        <span>
          <Duration p={r} />
        </span>
        {(r.runAttempt ?? 1) > 1 && (
          <>
            <span className="gh-sep">·</span>
            <span>attempt {r.runAttempt}</span>
          </>
        )}
        {failedJobs > 0 && (
          <>
            <span className="gh-sep">·</span>
            <span className="wb-danger">
              {failedJobs} failed job{failedJobs > 1 ? 's' : ''}
            </span>
          </>
        )}
      </div>
    </div>
  )
}

/** The run's artifacts; downloads need a token, even on public repositories. */
function Artifacts({ projectId, runId, updatedAt, anonymous }: { projectId: string; runId: number; updatedAt: string | null; anonymous: boolean }) {
  const q = useRunArtifacts(projectId, runId, updatedAt)
  const list = q.data ?? []
  if (!list.length) return null
  return (
    <div className="gh-artifacts">
      <div className="gh-artifacts-head">
        <Package size={13} /> Artifacts <span className="wb-subtle">{list.length}</span>
      </div>
      {list.map((a) => (
        <div key={a.id} className={`gh-artifact${a.expired ? ' expired' : ''}`}>
          <Package size={13} className="wb-subtle" />
          <span className="wb-ellipsis">{a.name}</span>
          <span className="wb-subtle wb-xs">{formatBytes(a.sizeInBytes)}</span>
          <span className="wb-grow" />
          {a.expired ? (
            <span className="wb-subtle wb-xs">expired</span>
          ) : (
            <>
              {a.expiresAt && (
                <span className="wb-subtle wb-xs" title={new Date(a.expiresAt).toLocaleString()}>
                  expires {expiresIn(a.expiresAt)}
                </span>
              )}
              {anonymous ? (
                <span className="wb-subtle wb-xs" title="GitHub serves artifacts to signed-in users only">
                  {NEEDS_TOKEN}
                </span>
              ) : (
                <a className="wb-btn small ghost" href={ghApi.artifactUrl(projectId, a.id)} download title={`Download ${a.name}.zip`}>
                  <Download size={13} /> Download
                </a>
              )}
            </>
          )}
        </div>
      ))}
    </div>
  )
}

export function RunPanel({ params, setTitle }: PanelProps<RunParams>) {
  const { projectId, runId } = params
  const summary = useGithubSummary(projectId)
  const anonymous = summary.data ? !summary.data.auth.authenticated : false
  const q = useRun(projectId, runId, anonymous)
  const r = q.data?.run
  useEffect(() => {
    if (r) setTitle(runTitle(r, runId))
  }, [r?.name, r?.runNumber, setTitle]) // eslint-disable-line react-hooks/exhaustive-deps
  if (q.error && !q.data) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!q.data) return <Loading />
  return (
    <div className="wb-fill">
      <RunHeader projectId={projectId} d={q.data} anonymous={anonymous} />
      <JobGrid
        projectId={projectId}
        repo={summary.data?.path ?? projectId}
        d={q.data}
        anonymous={anonymous}
        onOpenJob={(j) => openJob(projectId, j.id, j.name)}
      />
      <Artifacts projectId={projectId} runId={runId} updatedAt={q.data.run.updatedAt} anonymous={anonymous} />
    </div>
  )
}
