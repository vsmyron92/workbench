// Phone "GitHub" tab: Actions, pull requests, issues and releases (the tool
// window's tabs), with a back bar into a run → its jobs → a job's log, a pull
// request (without the side-by-side diff) or an issue.

import { useEffect, useRef, type ReactNode } from 'react'
import { ArrowLeft } from 'lucide-react'
import { Badge, EmptyState, ErrorBox, IconButton, Loading, Tabs } from '@/ui'
import { useGithubSummary, useHasGithub, useJob, useJobLog, useRun } from './api'
import { GitHubIcon, PublicModeBanner, runTitle, StatusIcon, StatusText, useGhUi, type GhTab } from './components'
import { IssueView } from './IssuePanel'
import { JobLogView } from './JobLog'
import { StepsList } from './JobPanel'
import { IssuesList, PullsList, ReleasesList, RunsList } from './Lists'
import { useGhMobile } from './mobile'
import { PrView } from './PrPanel'
import { JobGrid, RerunJobButton, RunHeader } from './RunPanel'

function Back({ onBack, children }: { onBack: () => void; children: ReactNode }) {
  return (
    <div className="gh-mobile-back">
      <IconButton icon={ArrowLeft} label="Back" onClick={onBack} />
      {children}
    </div>
  )
}

function MobileJob({ projectId, id, anonymous, onBack }: { projectId: string; id: number; anonymous: boolean; onBack: () => void }) {
  const job = useJob(projectId, id, anonymous)
  const log = useJobLog(projectId, id, !!job.data)
  const j = job.data
  return (
    <div className="wb-fill">
      <Back onBack={onBack}>
        {j && <StatusIcon state={j.state} size={16} />}
        <span className="wb-grow wb-ellipsis" style={{ fontWeight: 600 }}>
          {j?.name ?? `Job ${id}`}
        </span>
        {j && <StatusText state={j.state} status={j.status} conclusion={j.conclusion} />}
        {j && <RerunJobButton projectId={projectId} job={j} anonymous={anonymous} compact />}
      </Back>
      {job.error && !j ? (
        <ErrorBox error={job.error} />
      ) : !log.data ? (
        log.error ? <ErrorBox error={log.error} /> : <Loading label="Loading log…" />
      ) : log.data.available && log.data.text !== undefined ? (
        <JobLogView text={log.data.text} steps={log.data.job.steps} marks={log.data.steps ?? []} truncated={!!log.data.truncated} outline={false} />
      ) : (
        <div className="wb-scroll">
          <div className="gh-banner info">
            <span className="wb-grow">{log.data.message}</span>
          </div>
          <StepsList job={log.data.job} />
        </div>
      )}
    </div>
  )
}

function MobileRun({ projectId, id, anonymous, onBack, onJob }: { projectId: string; id: number; anonymous: boolean; onBack: () => void; onJob: (id: number) => void }) {
  const q = useRun(projectId, id, anonymous)
  const summary = useGithubSummary(projectId)
  return (
    <div className="wb-fill">
      <Back onBack={onBack}>
        <span className="wb-grow wb-ellipsis" style={{ fontWeight: 600 }}>
          {runTitle(q.data?.run, id)}
        </span>
      </Back>
      {q.error && !q.data ? (
        <ErrorBox error={q.error} onRetry={() => q.refetch()} />
      ) : !q.data ? (
        <Loading />
      ) : (
        <div className="wb-scroll">
          <RunHeader projectId={projectId} d={q.data} anonymous={anonymous} compact />
          <JobGrid projectId={projectId} repo={summary.data?.path ?? projectId} d={q.data} anonymous={anonymous} vertical onOpenJob={(j) => onJob(j.id)} />
        </div>
      )}
    </div>
  )
}

/** A pull request or an issue under a back bar. */
function MobileDetail({ label, onBack, children }: { label: string; onBack: () => void; children: ReactNode }) {
  return (
    <div className="wb-fill">
      <Back onBack={onBack}>
        <span className="wb-grow wb-ellipsis wb-muted">{label}</span>
      </Back>
      {children}
    </div>
  )
}

function MobileGithub({ projectId }: { projectId: string | null }) {
  const has = useHasGithub(projectId)
  const summary = useGithubSummary(projectId, has)
  const view = useGhMobile((s) => s.view)
  const setView = useGhMobile((s) => s.setView)
  const tab = useGhUi((s) => s.tab)
  const setTab = useGhUi((s) => s.setTab)
  // Back to the lists when the project changes, unless `openPanel` just chose
  // a view of the new project.
  const shownFor = useRef(projectId)
  useEffect(() => {
    if (shownFor.current === projectId) return
    shownFor.current = projectId
    const v = useGhMobile.getState().view
    if (v.kind !== 'list' && v.pid !== projectId) useGhMobile.getState().setView({ kind: 'list' })
  }, [projectId])
  if (!projectId) return <EmptyState title="No project selected" />
  if (!has) return <EmptyState icon={GitHubIcon} title="This project is not on GitHub" />
  const anonymous = summary.data ? !summary.data.auth.authenticated : false
  const toList = () => setView({ kind: 'list' })
  const detail = view.kind !== 'list' && view.pid === projectId ? view : null
  if (detail?.kind === 'job')
    return (
      <MobileJob
        projectId={projectId}
        id={detail.id}
        anonymous={anonymous}
        onBack={() => setView(detail.runId ? { kind: 'run', pid: projectId, id: detail.runId } : { kind: 'list' })}
      />
    )
  if (detail?.kind === 'run')
    return (
      <MobileRun
        projectId={projectId}
        id={detail.id}
        anonymous={anonymous}
        onBack={toList}
        onJob={(jobId) => setView({ kind: 'job', pid: projectId, id: jobId, runId: detail.id })}
      />
    )
  if (detail?.kind === 'pr')
    return (
      <MobileDetail label="Pull requests" onBack={toList}>
        <PrView projectId={projectId} number={detail.number} compact />
      </MobileDetail>
    )
  if (detail?.kind === 'issue')
    return (
      <MobileDetail label="Issues" onBack={toList}>
        <IssueView projectId={projectId} number={detail.number} />
      </MobileDetail>
    )
  if (summary.error && !summary.data) return <ErrorBox error={summary.error} onRetry={() => summary.refetch()} />
  const s = summary.data
  const count = (n: number | null | undefined) => (n ? <Badge>{n}</Badge> : undefined)
  const tabs: { id: GhTab; label: string; badge?: ReactNode }[] = [
    { id: 'runs', label: 'Actions' },
    { id: 'pulls', label: 'Pull requests', badge: count(s?.openPrCount) },
  ]
  if (!s || s.hasIssues) tabs.push({ id: 'issues', label: 'Issues', badge: count(s?.openIssueCount) })
  tabs.push({ id: 'releases', label: 'Releases' })
  const active = tabs.some((t) => t.id === tab) ? tab : 'runs'
  return (
    <div className="wb-fill gh-mobile">
      {s && <PublicModeBanner s={s} />}
      <Tabs<GhTab> tabs={tabs} value={active} onChange={setTab} />
      {active === 'runs' && <RunsList projectId={projectId} summary={s} compact onOpen={(r) => setView({ kind: 'run', pid: projectId, id: r.id })} />}
      {/* Their rows open panels, which a phone routes back here (see index.ts). */}
      {active === 'pulls' && <PullsList projectId={projectId} anonymous={anonymous} />}
      {active === 'issues' && <IssuesList projectId={projectId} anonymous={anonymous} />}
      {active === 'releases' && <ReleasesList projectId={projectId} />}
    </div>
  )
}

export default MobileGithub
