// The 'github' tool window: repository header (current branch's runs and pull
// request), the public-mode banner, and tabs for Actions, pull requests,
// issues and releases.

import type { ReactNode } from 'react'
import { GitPullRequestCreate } from 'lucide-react'
import { Badge, EmptyState, ErrorBox, Loading, Tabs } from '@/ui'
import { useGithubSummary } from './api'
import { Duration, ExtLink, GitHubIcon, openPr, openRun, PublicModeBanner, RefLabel, runTitle, StatusIcon, useGhUi, type GhTab } from './components'
import { ghLabel } from './logic'
import { PrStateIcon } from './icons'
import { IssuesList, PullsList, ReleasesList, RunsList } from './Lists'
import type { GithubSummary } from './types'

function Header({ projectId, s }: { projectId: string; s: GithubSummary }) {
  const openCreate = useGhUi((st) => st.openCreatePr)
  const r = s.branchRun
  const pr = s.currentPr
  const onDefault = !!s.branch && s.branch === s.defaultBranch
  return (
    <div className="gh-head">
      <div className="path">
        <GitHubIcon size={14} />
        <span className="wb-ellipsis" title={s.description ?? undefined}>
          {s.path}
        </span>
        {s.private && <Badge>Private</Badge>}
        {s.archived && <Badge tone="warning">Archived</Badge>}
        <ExtLink href={s.webUrl} />
      </div>
      <div className="ci">
        {s.branch ? <RefLabel name={s.branch} /> : <span className="wb-muted wb-small">detached HEAD</span>}
        {r ? (
          <button className="gh-chip" onClick={() => openRun(projectId, r.id, runTitle(r, r.id))} title={`${runTitle(r, r.id)} on ${r.headBranch}: ${ghLabel(r.status, r.conclusion)}`}>
            <StatusIcon state={r.state} size={13} />
            <span className="txt">
              {r.name ?? 'Run'} #{r.runNumber}
            </span>
            <span className="wb-muted">
              <Duration p={r} />
            </span>
          </button>
        ) : (
          <span className="wb-small wb-subtle">no runs</span>
        )}
        {pr ? (
          <button className="gh-chip pr" onClick={() => openPr(projectId, pr.number, pr.title)} title={pr.title}>
            <PrStateIcon pr={pr} size={13} />
            <span className="txt">
              #{pr.number} {pr.title}
            </span>
          </button>
        ) : (
          !onDefault &&
          s.branch &&
          s.auth.authenticated && (
            <button className="gh-chip" onClick={() => openCreate(projectId)} title="Create a pull request for this branch">
              <GitPullRequestCreate size={13} />
              <span className="txt">Create PR</span>
            </button>
          )
        )}
      </div>
    </div>
  )
}

export function GithubToolWindow({ projectId }: { projectId: string | null }) {
  const summary = useGithubSummary(projectId)
  const tab = useGhUi((s) => s.tab)
  const setTab = useGhUi((s) => s.setTab)
  if (!projectId) return <EmptyState title="No project selected" />
  const s = summary.data
  if (summary.error && !s) return <ErrorBox error={summary.error} onRetry={() => summary.refetch()} />
  if (!s) return <Loading />

  const anon = !s.auth.authenticated
  const count = (n: number | null | undefined) => (n ? <Badge>{n}</Badge> : undefined)
  const tabs: { id: GhTab; label: string; badge?: ReactNode }[] = [
    { id: 'runs', label: 'Actions' },
    { id: 'pulls', label: 'Pull requests', badge: count(s.openPrCount) },
  ]
  if (s.hasIssues) tabs.push({ id: 'issues', label: 'Issues', badge: count(s.openIssueCount) })
  tabs.push({ id: 'releases', label: 'Releases' })
  const active = tabs.some((t) => t.id === tab) ? tab : 'runs'

  return (
    <div className="wb-fill">
      <Header projectId={projectId} s={s} />
      <PublicModeBanner s={s} />
      <Tabs<GhTab> tabs={tabs} value={active} onChange={setTab} />
      {active === 'runs' && <RunsList projectId={projectId} summary={s} />}
      {active === 'pulls' && <PullsList projectId={projectId} anonymous={anon} />}
      {active === 'issues' && <IssuesList projectId={projectId} anonymous={anon} />}
      {active === 'releases' && <ReleasesList projectId={projectId} />}
    </div>
  )
}
