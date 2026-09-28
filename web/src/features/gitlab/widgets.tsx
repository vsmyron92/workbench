// Top bar: CI status of the local HEAD commit. Status bar: the current
// branch's latest pipeline. Both open the pipeline on click.

import { showToolWindow } from '@/shell/actions'
import { useGitlabSummary, useHasGitlab } from './api'
import { Duration, openPipeline, StatusIcon, useGlUi } from './components'
import { isActive, shortSha, statusLabel } from './logic'

export function CiTopbarWidget({ projectId }: { projectId: string | null }) {
  const has = useHasGitlab(projectId)
  const summary = useGitlabSummary(projectId, has)
  const setTab = useGlUi((s) => s.setTab)
  if (!has || !projectId) return null
  const s = summary.data
  const openList = () => {
    setTab('pipelines')
    showToolWindow('gitlab')
  }
  if (!s) {
    if (!summary.error) return null
    return (
      <button className="wb-topbar-widget gl-topbar-ci" title={`GitLab: ${(summary.error as Error).message}`} onClick={openList}>
        <StatusIcon status={null} size={15} title="GitLab unavailable" />
        <span className="label">CI</span>
      </button>
    )
  }
  const st = s.headStatus
  const title = st
    ? `CI for HEAD ${shortSha(s.head)}: ${statusLabel(st.status)}${st.ref ? ` (${st.ref})` : ''} — open pipeline`
    : s.head
      ? `No pipeline for HEAD ${shortSha(s.head)} (not pushed yet, or CI skipped)`
      : 'No commits yet'
  return (
    <button
      className="wb-topbar-widget gl-topbar-ci"
      title={title}
      onClick={() => (st?.pipelineId ? openPipeline(projectId, st.pipelineId) : openList())}
    >
      <StatusIcon status={st?.status ?? null} size={15} title={title} />
      <span className="label">{st ? statusLabel(st.status) : 'no CI'}</span>
    </button>
  )
}

export function PipelineStatusItem({ projectId }: { projectId: string | null }) {
  const has = useHasGitlab(projectId)
  const summary = useGitlabSummary(projectId, has)
  if (!has || !projectId) return null
  const p = summary.data?.branchPipeline
  if (!p) return null
  return (
    <button
      className="wb-status-item"
      title={`Latest pipeline on ${p.ref}: ${statusLabel(p.status)} — open`}
      onClick={() => openPipeline(projectId, p.id, p.iid)}
    >
      <StatusIcon status={p.status} size={12} />
      <span>
        Pipeline #{p.iid ?? p.id} {statusLabel(p.status)}
        {isActive(p.status) && (
          <>
            {' · '}
            <Duration p={p} />
          </>
        )}
      </span>
    </button>
  )
}
