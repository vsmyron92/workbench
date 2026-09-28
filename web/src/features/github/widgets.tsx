// Top bar: checks of the local HEAD commit. Status bar: the current branch's
// newest workflow run. Both only for projects whose forge is GitHub (a project
// on both GitLab and GitHub shows GitLab's CI there), and both open the run.

import { showToolWindow } from '@/shell/actions'
import { useGithubIsForge, useGithubSummary } from './api'
import { Duration, openRun, runTitle, StatusIcon, useGhUi } from './components'
import { ghLabel, isActive, shortSha, stateLabel } from './logic'

export function CiTopbarWidget({ projectId }: { projectId: string | null }) {
  const mine = useGithubIsForge(projectId)
  const summary = useGithubSummary(projectId, mine)
  const setTab = useGhUi((s) => s.setTab)
  if (!mine || !projectId) return null
  const s = summary.data
  const openList = () => {
    setTab('runs')
    showToolWindow('github')
  }
  if (!s) {
    if (!summary.error) return null
    return (
      <button className="wb-topbar-widget gh-topbar-ci" title={`GitHub: ${(summary.error as Error).message}`} onClick={openList}>
        <StatusIcon state={null} size={15} title="GitHub unavailable" />
        <span className="label">CI</span>
      </button>
    )
  }
  const st = s.headStatus
  const title = st
    ? `Checks for HEAD ${shortSha(s.head)}: ${stateLabel(st.status)}${st.ref ? ` (${st.ref})` : ''} — open the run`
    : s.head
      ? `No checks for HEAD ${shortSha(s.head)} (not pushed yet, or nothing ran)`
      : 'No commits yet'
  return (
    <button
      className="wb-topbar-widget gh-topbar-ci"
      title={title}
      onClick={() => (st?.pipelineId ? openRun(projectId, st.pipelineId) : openList())}
    >
      <StatusIcon state={st?.status ?? null} size={15} title={title} />
      <span className="label">{st ? stateLabel(st.status) : 'no CI'}</span>
    </button>
  )
}

export function RunStatusItem({ projectId }: { projectId: string | null }) {
  const mine = useGithubIsForge(projectId)
  const summary = useGithubSummary(projectId, mine)
  if (!mine || !projectId) return null
  const r = summary.data?.branchRun
  if (!r) return null
  return (
    <button className="wb-status-item" title={`Newest run on ${r.headBranch}: ${ghLabel(r.status, r.conclusion)} — open`} onClick={() => openRun(projectId, r.id, runTitle(r, r.id))}>
      <StatusIcon state={r.state} size={12} />
      <span>
        {runTitle(r, r.id)} {stateLabel(r.state)}
        {isActive(r.state) && (
          <>
            {' · '}
            <Duration p={r} />
          </>
        )}
      </span>
    </button>
  )
}
