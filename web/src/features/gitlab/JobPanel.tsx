// 'job' panel: one CI job — header with actions, failure banner and the log.

import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Bot, Download, ExternalLink, Package, Play, RotateCw, Square } from 'lucide-react'
import { askAgent } from '@/shell/agentBridge'
import { toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { Button, ErrorBox, formatBytes, Loading, Spacer, TimeAgo, Toolbar } from '@/ui'
import { fetchTraceTail, glApi, glk, useGitlabSummary, useJob } from './api'
import { Duration, ExtLink, openJob, openPipeline, RefLabel, StatusIcon, StatusText } from './components'
import { JobLogView, useJobTrace } from './JobLog'
import { isActive, jobFixPrompt, shortSha } from './logic'
import type { Job } from './types'

export interface JobParams {
  projectId: string
  jobId: number
}

/** Ask the project's agent to fix a failed job (with the end of its log). */
export async function askAgentToFix(projectId: string, projectPath: string, job: Job) {
  try {
    const tail = await fetchTraceTail(projectId, job.id, 150)
    await askAgent({
      projectId,
      prompt: jobFixPrompt({
        projectPath,
        job,
        pipeline: job.pipeline,
        log: tail.text,
        shownLines: Math.min(150, tail.totalLines),
        totalLines: tail.totalLines,
      }),
    })
  } catch (e) {
    toastError(e, 'Could not read the job log')
  }
}

export function JobActions({ projectId, job, compact }: { projectId: string; job: Job; compact?: boolean }) {
  const qc = useQueryClient()
  const [busy, setBusy] = useState<string | null>(null)
  const run = async (what: 'retry' | 'cancel' | 'play') => {
    setBusy(what)
    try {
      const j =
        what === 'retry' ? await glApi.retryJob(projectId, job.id) : what === 'cancel' ? await glApi.cancelJob(projectId, job.id) : await glApi.playJob(projectId, job.id)
      toast('success', what === 'retry' ? `Retrying ${job.name}` : what === 'cancel' ? `Canceled ${job.name}` : `Started ${job.name}`)
      if (j.id !== job.id) openJob(projectId, j.id, j.name)
      qc.invalidateQueries({ queryKey: glk.job(projectId, job.id) })
      if (job.pipeline) qc.invalidateQueries({ queryKey: glk.pipeline(projectId, job.pipeline.id) })
    } catch (e) {
      toastError(e, `Could not ${what} the job`)
    } finally {
      setBusy(null)
    }
  }
  const size = 'small' as const
  return (
    <>
      {job.status === 'manual' && (
        <Button size={size} icon={Play} loading={busy === 'play'} onClick={() => run('play')}>
          {compact ? '' : 'Run'}
        </Button>
      )}
      {isActive(job.status) && (
        <Button size={size} icon={Square} loading={busy === 'cancel'} onClick={() => run('cancel')}>
          {compact ? '' : 'Cancel'}
        </Button>
      )}
      {['failed', 'canceled', 'success'].includes(job.status) && (
        <Button size={size} icon={RotateCw} loading={busy === 'retry'} onClick={() => run('retry')}>
          {compact ? '' : 'Retry'}
        </Button>
      )}
    </>
  )
}

export function JobPanel({ params, setTitle, visible }: PanelProps<JobParams>) {
  const { projectId, jobId } = params
  const qc = useQueryClient()
  const job = useJob(projectId, jobId)
  const summary = useGitlabSummary(projectId)
  const trace = useJobTrace(projectId, jobId, visible)
  const j = job.data

  useEffect(() => {
    if (j) setTitle(j.name)
  }, [j?.name, setTitle]) // eslint-disable-line react-hooks/exhaustive-deps

  // The log tells us when the status moves; refresh the job header then.
  useEffect(() => {
    if (trace.status && j && trace.status !== j.status) qc.invalidateQueries({ queryKey: glk.job(projectId, jobId) })
  }, [trace.status]) // eslint-disable-line react-hooks/exhaustive-deps

  if (job.error && !j) return <ErrorBox error={job.error} onRetry={() => job.refetch()} />
  if (!j) return <Loading />

  const failed = j.status === 'failed'
  const projectPath = summary.data?.path ?? projectId
  return (
    <div className="wb-fill">
      <Toolbar>
        <StatusIcon status={j.status} size={16} />
        <span className="title wb-ellipsis">{j.name}</span>
        <StatusText status={j.status} />
        <span className="gl-sep">·</span>
        <span className="wb-small wb-muted wb-ellipsis">
          {j.stage}
          {j.pipeline && (
            <>
              {' · '}
              <a className="gl-link" href="#" onClick={(e) => (e.preventDefault(), openPipeline(projectId, j.pipeline!.id, j.pipeline!.iid))}>
                pipeline #{j.pipeline.iid ?? j.pipeline.id}
              </a>
            </>
          )}
        </span>
        <Spacer />
        <JobActions projectId={projectId} job={j} />
        {failed && (
          <Button size="small" icon={Bot} onClick={() => askAgentToFix(projectId, projectPath, j)}>
            Ask agent to fix
          </Button>
        )}
        <a className="wb-btn small ghost" href={glApi.logUrl(projectId, j.id)} download title="Download the log as text">
          <Download size={13} />
        </a>
        {j.artifactsFile && (
          <a className="wb-btn small ghost" href={glApi.artifactsUrl(projectId, j.id)} download title={`Download artifacts (${formatBytes(j.artifactsFile.size)})`}>
            <Package size={13} />
          </a>
        )}
        <ExtLink href={j.webUrl}>
          <span className="wb-btn small ghost">
            <ExternalLink size={13} />
          </span>
        </ExtLink>
      </Toolbar>
      <div className="gl-banner" style={{ gap: 10 }}>
        <RefLabel name={j.ref} tag={j.tag} />
        {j.commit && (
          <span className="wb-ellipsis wb-small" style={{ flex: '1 1 auto', minWidth: 0 }} title={j.commit.title}>
            <span className="gl-mono">{shortSha(j.commit.id)}</span> {j.commit.title}
          </span>
        )}
        <span className="wb-small wb-muted wb-ellipsis" style={{ flex: '0 1 auto', minWidth: 0, marginLeft: 'auto' }} title={j.runner?.description}>
          <Duration p={j} />
          {j.queuedDuration && j.queuedDuration >= 1 ? ` (queued ${Math.round(j.queuedDuration)}s)` : ''}
          {j.finishedAt ? (
            <>
              {' · finished '}
              <TimeAgo time={j.finishedAt} />
            </>
          ) : null}
          {j.runner ? ` · ${j.runner.description}` : ''}
        </span>
      </div>
      {failed && (
        <div className="gl-banner danger">
          <StatusIcon status="failed" />
          <span>
            Failed{j.failureReason ? `: ${j.failureReason.replace(/_/g, ' ')}` : ''}
            {j.allowFailure ? ' (allowed to fail)' : ''}
          </span>
        </div>
      )}
      <JobLogView trace={trace} />
    </div>
  )
}
