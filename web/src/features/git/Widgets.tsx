// Top bar branch button, status bar branch/sync/state item, stripe badge.

import { ChevronDown, FolderGit2, GitBranch, ShieldAlert } from 'lucide-react'
import { repoOfScope, scopeProject } from '@/api/repos'
import { useRepos } from '@/api/useRepos'
import { showToolWindow } from '@/shell/actions'
import { Spinner, StatusDot } from '@/ui'
import { isUnsafeRepo, useGitStatus } from './api'
import { shortSha, stateLabel } from './logic'
import { toggleRepoPopover } from './RepoSwitcher'
import { useGitUi, useRunningOp } from './store'
import type { GitStatus } from './types'

function branchName(s: GitStatus) {
  return s.branch ?? (s.head ? shortSha(s.head) : 'No commits yet')
}

function togglePopover(pid: string, el: HTMLElement, from: 'topbar' | 'statusbar') {
  const ui = useGitUi.getState()
  if (ui.popover?.projectId === pid && ui.popover.from === from) ui.closePopover()
  else ui.openPopover(pid, el.getBoundingClientRect(), from)
}

function Sync({ s }: { s: GitStatus }) {
  if (!s.ahead && !s.behind) return null
  return (
    <span className="git-sync" title={`${s.ahead} outgoing, ${s.behind} incoming commit(s) relative to ${s.upstream}`}>
      {s.ahead > 0 && `↑${s.ahead}`}
      {s.ahead > 0 && s.behind > 0 && ' '}
      {s.behind > 0 && `↓${s.behind}`}
    </span>
  )
}

/** The repository the status bar's git items are about, when the project has several: a click switches it. */
function RepoStatusItem({ scope }: { scope: string }) {
  const projectId = scopeProject(scope)
  const repos = useRepos(projectId)
  const repo = repoOfScope(repos, scope)
  if (!repo || repos.length < 2) return null
  return (
    <button
      className="wb-status-item"
      data-git-repo-anchor=""
      aria-haspopup="listbox"
      title={`Repository ${repo.name}${repo.path ? ` (${repo.path})` : ''}. Switch repository`}
      onClick={(e) => toggleRepoPopover(projectId, e.currentTarget, 'statusbar')}
    >
      <FolderGit2 size={13} />
      <span>{repo.name}</span>
    </button>
  )
}

/** CLion's branch widget in the top bar. */
export function BranchTopbarWidget({ projectId }: { projectId: string | null }) {
  const st = useGitStatus(projectId)
  const s = st.data
  if (!projectId || !s) return null
  const title = s.branch
    ? `Branch ${s.branch}${s.upstream ? ` → ${s.upstream}` : ' (no upstream)'}${s.upstreamGone ? ' (upstream gone)' : ''}`
    : s.head
      ? `Detached HEAD at ${s.head}`
      : 'No commits yet'
  return (
    <button
      className={`wb-topbar-widget git-branch-btn${s.branch ? '' : ' detached'}`}
      data-git-branch-anchor=""
      onClick={(e) => togglePopover(projectId, e.currentTarget, 'topbar')}
      title={title}
    >
      <GitBranch size={14} className="wb-muted" />
      <span className="name">{branchName(s)}</span>
      <Sync s={s} />
      {s.state !== 'clean' && <span className="wb-badge warning">{stateLabel(s.state)}</span>}
      <ChevronDown size={13} className="wb-muted" />
    </button>
  )
}

/**
 * Git refuses the repository (another user owns the folder): say so where the branch would
 * be. The Commit tool window shows git's message and copies the command that trusts it.
 */
function UntrustedStatusItem({ message }: { message: string }) {
  return (
    <button className="wb-status-item" title={`${message}\n\nClick to show it in the Commit tool window.`} onClick={() => showToolWindow('commit')}>
      <ShieldAlert size={13} className="wb-warning" />
      <span className="wb-warning">Untrusted repository</span>
    </button>
  )
}

/** Branch, sync and repository state in the status bar; a running fetch/pull/push. */
export function GitStatusbarWidget({ projectId }: { projectId: string | null }) {
  const st = useGitStatus(projectId)
  const running = useRunningOp(projectId)
  const s = st.data
  if (projectId && !s && isUnsafeRepo(st.error)) return <UntrustedStatusItem message={st.error.message} />
  if (!projectId || !s) return null
  return (
    <>
      <RepoStatusItem scope={projectId} />
      <button
        className="wb-status-item"
        data-git-branch-anchor=""
        onClick={(e) => togglePopover(projectId, e.currentTarget, 'statusbar')}
        title={s.upstream ? `${s.branch ?? 'HEAD'} → ${s.upstream}` : 'Branches'}
      >
        <GitBranch size={13} />
        <span>{branchName(s)}</span>
        <Sync s={s} />
        {s.state !== 'clean' && (
          <span className="wb-warning" style={{ fontWeight: 600 }}>
            {stateLabel(s.state).toUpperCase()}
            {s.stateDetail.step && s.stateDetail.total ? ` ${s.stateDetail.step}/${s.stateDetail.total}` : ''}
          </span>
        )}
      </button>
      {running && (
        <span className="wb-status-item" title={running.lastLine}>
          <Spinner size={11} />
          {running.title}
        </span>
      )}
    </>
  )
}

/** Stripe badge of the Commit tool window: a red dot while conflicts exist. */
export function CommitBadge({ projectId }: { projectId: string | null }) {
  const st = useGitStatus(projectId)
  const conflicts = st.data?.files.filter((f) => f.conflict).length ?? 0
  return conflicts ? <StatusDot tone="danger" title={`${conflicts} conflicted file(s)`} /> : null
}

/** Mobile tab badge: number of changed files. */
export function ChangesBadge({ projectId }: { projectId: string | null }) {
  const st = useGitStatus(projectId)
  const n = st.data?.files.filter((f) => f.index !== '!').length ?? 0
  return n ? <span className="git-count-badge">{n > 99 ? '99+' : n}</span> : null
}
