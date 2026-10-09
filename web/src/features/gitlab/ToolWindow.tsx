// The 'gitlab' tool window: project header (current branch CI and MR) and
// tabs for pipelines, merge requests, issues, environments and the registry.

import type { ReactNode } from 'react'
import { GitPullRequestCreate } from 'lucide-react'
import { NotOnForge } from '@/shell/RepoUi'
import { Badge, EmptyState, ErrorBox, GitLabIcon, Loading, Tabs } from '@/ui'
import { useGitlabSummary, useHasGitlab } from './api'
import { Duration, ExtLink, openMr, openPipeline, RefLabel, StatusIcon, useGlUi, type GlTab } from './components'
import { EnvironmentsList, IssuesList, MrList, PipelinesList, RegistryView } from './Lists'
import { statusLabel } from './logic'
import { MrStateIcon } from './MrPanel'
import type { GitlabSummary } from './types'

function Header({ projectId, s }: { projectId: string; s: GitlabSummary }) {
  const openCreate = useGlUi((st) => st.openCreateMr)
  const p = s.branchPipeline
  const mr = s.currentMr
  const onDefault = !!s.branch && s.branch === s.defaultBranch
  return (
    <div className="gl-head">
      <div className="path">
        <GitLabIcon size={14} />
        <span className="wb-ellipsis">{s.path}</span>
        <ExtLink href={s.webUrl} />
      </div>
      <div className="ci">
        {s.branch ? <RefLabel name={s.branch} /> : <span className="wb-muted wb-small">detached HEAD</span>}
        {p ? (
          <button className="gl-chip" onClick={() => openPipeline(projectId, p.id, p.iid)} title={`Latest pipeline on ${p.ref}: ${statusLabel(p.status)}`}>
            <StatusIcon status={p.status} size={13} />
            <span className="txt">#{p.iid ?? p.id}</span>
            <span className="wb-muted">
              <Duration p={p} />
            </span>
          </button>
        ) : (
          <span className="wb-small wb-subtle">no pipeline</span>
        )}
        {mr ? (
          <button className="gl-chip mr" onClick={() => openMr(projectId, mr.iid, mr.title)} title={mr.title}>
            <MrStateIcon mr={mr} size={13} />
            <span className="txt">
              !{mr.iid} {mr.title}
            </span>
          </button>
        ) : (
          !onDefault &&
          s.branch && (
            <button className="gl-chip" onClick={() => openCreate(projectId)} title="Create a merge request for this branch">
              <GitPullRequestCreate size={13} />
              <span className="txt">Create MR</span>
            </button>
          )
        )}
      </div>
    </div>
  )
}

export function GitlabToolWindow({ projectId }: { projectId: string | null }) {
  const has = useHasGitlab(projectId)
  const summary = useGitlabSummary(projectId, has)
  const tab = useGlUi((s) => s.tab)
  const setTab = useGlUi((s) => s.setTab)
  if (!projectId) return <EmptyState title="No project selected" />
  // The project has a repository on GitLab, but not the active one.
  if (!has) return <NotOnForge scope={projectId} forge="gitlab" icon={GitLabIcon} />
  const s = summary.data
  if (summary.error && !s) return <ErrorBox error={summary.error} onRetry={() => summary.refetch()} />
  if (!s) return <Loading />

  const count = (n: number | null | undefined) => (n ? <Badge>{n}</Badge> : undefined)
  const tabs: { id: GlTab; label: string; badge?: ReactNode }[] = [{ id: 'pipelines', label: 'Pipelines' }]
  if (s.mergeRequestsEnabled) tabs.push({ id: 'mrs', label: 'MRs', badge: count(s.openMrCount) })
  if (s.issuesEnabled) tabs.push({ id: 'issues', label: 'Issues', badge: count(s.openIssueCount) })
  if (s.environmentCount) tabs.push({ id: 'envs', label: 'Envs', badge: count(s.environmentCount) })
  if (s.registryEnabled) tabs.push({ id: 'registry', label: 'Registry' })
  const active = tabs.some((t) => t.id === tab) ? tab : 'pipelines'

  return (
    <div className="wb-fill">
      <Header projectId={projectId} s={s} />
      <Tabs<GlTab> tabs={tabs} value={active} onChange={setTab} />
      {active === 'pipelines' && <PipelinesList projectId={projectId} summary={s} />}
      {active === 'mrs' && <MrList projectId={projectId} />}
      {active === 'issues' && <IssuesList projectId={projectId} />}
      {active === 'envs' && <EnvironmentsList projectId={projectId} />}
      {active === 'registry' && <RegistryView projectId={projectId} head={s.head} />}
    </div>
  )
}
