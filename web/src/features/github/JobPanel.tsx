// 'gh.job' panel: one Actions job — header with actions, and the log (steps
// and groups fold). GitHub publishes a log only when the job has finished, and
// only to signed-in users: until then the steps and annotations stand in.

import { useEffect } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Bot, CircleAlert, CircleX, Download, ExternalLink, Info, KeyRound, RefreshCw, TriangleAlert } from 'lucide-react'
import type { PanelProps } from '@/shell/types'
import { Button, EmptyState, ErrorBox, formatDuration, IconButton, Loading, Spacer, TimeAgo, Toolbar } from '@/ui'
import { ghApi, ghk, useAnnotations, useGithubSummary, useJob, useJobLog, useRun } from './api'
import { Duration, ExtLink, openRun, RefLabel, runTitle, StatusIcon, StatusText } from './components'
import { JobLogView } from './JobLog'
import { askAgentToFix, RerunJobButton } from './RunPanel'
import { ghLabel, shortSha } from './logic'
import type { Annotation, Job, JobLog } from './types'

export interface JobParams {
  projectId: string
  jobId: number
}

function stepSeconds(a: string | null, b: string | null): number | null {
  if (!a || !b) return null
  const s = (Date.parse(b) - Date.parse(a)) / 1000
  return Number.isFinite(s) ? Math.max(0, s) : null
}

export function StepsList({ job }: { job: Job }) {
  if (!job.steps.length) return null
  return (
    <>
      <div className="gh-section-title">Steps</div>
      <div className="gh-steps">
        {job.steps.map((s) => (
          <div className="gh-step" key={s.number}>
            <StatusIcon state={s.state} size={14} title={ghLabel(s.status, s.conclusion)} />
            <span className={s.state === 'failed' ? 'n wb-danger' : 'n'}>{s.name}</span>
            <span className="d">{s.state === 'skipped' ? 'skipped' : formatDuration(stepSeconds(s.startedAt, s.completedAt))}</span>
          </div>
        ))}
      </div>
    </>
  )
}

const LEVEL_ICON = { failure: CircleX, warning: TriangleAlert, notice: Info } as const
const LEVEL_TONE = { failure: 'wb-danger', warning: 'wb-warning', notice: 'wb-muted' } as const

export function Annotations({ items }: { items: Annotation[] }) {
  if (!items.length) return null
  return (
    <>
      <div className="gh-section-title">Annotations</div>
      <div className="gh-annotations">
        {items.map((a, i) => {
          const level = (a.annotationLevel in LEVEL_ICON ? a.annotationLevel : 'notice') as keyof typeof LEVEL_ICON
          const I = LEVEL_ICON[level]
          return (
            <div className="gh-annotation" key={i}>
              <I size={14} className={LEVEL_TONE[level]} />
              <span className="where">
                {a.path}
                {a.startLine ? `:${a.startLine}` : ''}
                {a.title ? ` · ${a.title}` : ''}
              </span>
              <span className="msg">{a.message}</span>
            </div>
          )
        })}
      </div>
    </>
  )
}

/** What stands in for the log when GitHub has none for us. */
function NoLog({ log, annotations }: { log: JobLog; annotations: Annotation[] }) {
  const icon = log.reason === 'needs_token' ? KeyRound : log.reason === 'running' ? Info : CircleAlert
  const I = icon
  return (
    <div className="wb-scroll">
      <div className={log.reason === 'gone' ? 'gh-banner warning' : 'gh-banner info'}>
        <I size={14} />
        <span className="wb-grow">{log.message}</span>
      </div>
      <StepsList job={log.job} />
      <Annotations items={annotations} />
    </div>
  )
}

export function JobPanel({ params, setTitle }: PanelProps<JobParams>) {
  const { projectId, jobId } = params
  const summary = useGithubSummary(projectId)
  const anonymous = summary.data ? !summary.data.auth.authenticated : false
  const job = useJob(projectId, jobId, anonymous)
  const j = job.data
  const finished = j?.status === 'completed'
  const log = useJobLog(projectId, jobId, !!j)
  const noLog = log.data && !log.data.available
  const annotations = useAnnotations(projectId, jobId, !!noLog && finished)
  const run = useRun(projectId, j?.runId ?? 0, anonymous)
  const runData = j ? run.data?.run : undefined
  const qc = useQueryClient()
  // Without a token nothing polls: this (and run events) is how the job moves on.
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ghk.job(projectId, jobId), exact: true })
    if (j) void qc.invalidateQueries({ queryKey: ghk.run(projectId, j.runId) })
    if (noLog) void log.refetch()
  }

  useEffect(() => {
    if (j) setTitle(j.name)
  }, [j?.name, setTitle]) // eslint-disable-line react-hooks/exhaustive-deps

  // A running job's log appears when it finishes: ask again then.
  useEffect(() => {
    if (finished && log.data?.reason === 'running') void log.refetch()
  }, [finished]) // eslint-disable-line react-hooks/exhaustive-deps

  if (job.error && !j) return <ErrorBox error={job.error} onRetry={() => job.refetch()} />
  if (!j) return <Loading />

  const failed = j.state === 'failed'
  const repo = summary.data?.path ?? projectId
  const failedStep = j.steps.find((s) => s.state === 'failed')
  return (
    <div className="wb-fill">
      <Toolbar>
        <StatusIcon state={j.state} size={16} title={ghLabel(j.status, j.conclusion)} />
        <span className="title wb-ellipsis">{j.name}</span>
        <StatusText state={j.state} status={j.status} conclusion={j.conclusion} />
        <span className="gh-sep">·</span>
        <span className="wb-small wb-muted wb-ellipsis">
          <a className="gh-link" href="#" onClick={(e) => (e.preventDefault(), openRun(projectId, j.runId, runTitle(runData, j.runId)))}>
            {runTitle(runData, j.runId)}
          </a>
        </span>
        <Spacer />
        <RerunJobButton projectId={projectId} job={j} anonymous={anonymous} />
        {failed && (
          <Button size="small" icon={Bot} onClick={() => askAgentToFix(projectId, repo, j, runData?.name)}>
            Ask agent to fix
          </Button>
        )}
        {log.data?.available && (
          <a className="wb-btn small ghost" href={ghApi.logUrl(projectId, j.id)} download title="Download the log as text">
            <Download size={13} />
          </a>
        )}
        <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={refresh} />
        <ExtLink href={j.htmlUrl}>
          <span className="wb-btn small ghost">
            <ExternalLink size={13} />
          </span>
        </ExtLink>
      </Toolbar>
      <div className="gh-banner" style={{ gap: 10 }}>
        {j.headBranch && <RefLabel name={j.headBranch} />}
        <span className="gh-mono">{shortSha(j.headSha)}</span>
        {runData?.commitTitle && (
          <span className="wb-ellipsis wb-small" style={{ flex: '1 1 auto', minWidth: 0 }} title={runData.commitTitle}>
            {runData.commitTitle}
          </span>
        )}
        <span className="wb-small wb-muted wb-ellipsis" style={{ flex: '0 1 auto', minWidth: 0, marginLeft: 'auto' }}>
          <Duration p={j} />
          {j.completedAt ? (
            <>
              {' · finished '}
              <TimeAgo time={j.completedAt} />
            </>
          ) : null}
          {j.runnerName ? ` · ${j.runnerName}` : ''}
          {j.labels.length ? ` · ${j.labels.join(', ')}` : ''}
        </span>
      </div>
      {failed && (
        <div className="gh-banner danger">
          <StatusIcon state="failed" />
          <span>
            Failed{failedStep ? ` at “${failedStep.name}”` : ''}
            {j.conclusion && j.conclusion !== 'failure' ? ` (${ghLabel(j.status, j.conclusion)})` : ''}
          </span>
        </div>
      )}
      {log.error && !log.data ? (
        <ErrorBox error={log.error} onRetry={() => log.refetch()} />
      ) : !log.data ? (
        <Loading label="Loading log…" />
      ) : log.data.available && log.data.text !== undefined ? (
        log.data.text.trim() ? (
          <JobLogView text={log.data.text} steps={log.data.job.steps} marks={log.data.steps ?? []} truncated={!!log.data.truncated} />
        ) : (
          <EmptyState title="This job wrote no log" />
        )
      ) : (
        <NoLog log={log.data} annotations={annotations.data ?? []} />
      )}
    </div>
  )
}
