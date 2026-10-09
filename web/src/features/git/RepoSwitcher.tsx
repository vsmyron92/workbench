// The repository switcher of a project with several git repositories: a top bar button
// (CLion's repository chooser), a chip for the Commit and Git Log windows and the list they
// open (the phone's select is `shell/RepoUi.tsx`). The git, GitLab and GitHub views follow the
// repository chosen here (`api/repos.ts`, `useGitScope`).

import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Check, ChevronDown, FolderGit2, GitBranch, TriangleAlert } from 'lucide-react'
import { repoOfScope, scopeProject, setActiveRepo } from '@/api/repos'
import type { RepoSummary } from '@/api/types'
import { useActiveRepo, useRepos } from '@/api/useRepos'
import { Spinner } from '@/ui'
import { useRepoInfos } from './api'
import { shortSha } from './logic'
import { useGitUi } from './store'
import type { GitRepoInfo } from './types'

type From = 'topbar' | 'statusbar'

/** Open the repository list below (or, in the status bar, above) `el`; a second click closes it. */
export function toggleRepoPopover(projectId: string, el: HTMLElement, from: From) {
  const ui = useGitUi.getState()
  if (ui.repoPopover?.projectId === projectId && ui.repoPopover.from === from) ui.closeRepoPopover()
  else ui.openRepoPopover(projectId, el.getBoundingClientRect(), from)
}

/** Open the list wherever the switcher button is (the palette's "Switch Repository…"). */
export function openRepoSwitcher(projectId: string) {
  const el = document.querySelector<HTMLElement>('.git-repo-btn')
  useGitUi.getState().openRepoPopover(projectId, el?.getBoundingClientRect() ?? null, 'topbar')
}

/** The top bar button; only a project with several repositories has one. */
export function RepoTopbarWidget({ projectId }: { projectId: string | null }) {
  const { repo, repos } = useActiveRepo(projectId)
  if (!projectId || !repo || repos.length < 2) return null
  return (
    <button
      className="wb-topbar-widget git-repo-btn"
      data-git-repo-anchor=""
      aria-haspopup="listbox"
      aria-label={`Repository: ${repo.name}. Switch repository`}
      title={`Repository ${repo.name}${repo.path ? ` (${repo.path})` : ''}`}
      onClick={(e) => toggleRepoPopover(projectId, e.currentTarget, 'topbar')}
    >
      <FolderGit2 size={14} className="wb-muted" />
      <span className="name">{repo.name}</span>
      <ChevronDown size={13} className="wb-muted" />
    </button>
  )
}

/**
 * Which repository a git window shows (its scope id), as a button that opens the list.
 * `readOnly`: the window is tied to its repository (a panel), so it only says which.
 */
export function RepoChip({ scope, readOnly }: { scope: string; readOnly?: boolean }) {
  const projectId = scopeProject(scope)
  const repos = useRepos(projectId)
  const repo = repoOfScope(repos, scope)
  if (!repo || repos.length < 2) return null
  if (readOnly) {
    return (
      <span className="git-repo-chip" title={`Repository ${repo.name}${repo.path ? ` (${repo.path})` : ''}`}>
        <FolderGit2 size={12} />
        <span className="wb-ellipsis">{repo.name}</span>
      </span>
    )
  }
  return (
    <button
      className="git-repo-chip"
      data-git-repo-anchor=""
      aria-haspopup="listbox"
      title={`Repository ${repo.name}${repo.path ? ` (${repo.path})` : ''}. Switch repository`}
      onClick={(e) => toggleRepoPopover(projectId, e.currentTarget, 'topbar')}
    >
      <FolderGit2 size={12} />
      <span className="wb-ellipsis">{repo.name}</span>
    </button>
  )
}

/** The list the buttons open (mounted once by the git provider). */
export function RepoPopover() {
  const pop = useGitUi((s) => s.repoPopover)
  if (!pop) return null
  return <RepoPopoverBody key={pop.projectId} projectId={pop.projectId} anchor={pop.anchor} from={pop.from} />
}

function Meta({ info }: { info: GitRepoInfo | undefined }): ReactNode {
  if (!info) return null
  if (info.error) {
    return (
      <span className="wb-warning" title={info.error.message} aria-label={info.error.message}>
        <TriangleAlert size={13} />
      </span>
    )
  }
  return (
    <>
      <span className="git-repo-branch" title={info.branch ? `Branch ${info.branch}` : `Detached HEAD at ${info.head ?? ''}`}>
        <GitBranch size={12} />
        {info.branch ?? (info.head ? shortSha(info.head) : 'no commits')}
      </span>
      {(info.ahead > 0 || info.behind > 0) && (
        <span className="git-sync">
          {info.ahead > 0 && `↑${info.ahead}`} {info.behind > 0 && `↓${info.behind}`}
        </span>
      )}
      {info.conflicts > 0 && <span className="wb-badge danger">{info.conflicts} conflict{info.conflicts === 1 ? '' : 's'}</span>}
      {info.changed > 0 && (
        <span className="git-count-badge" title={`${info.changed} changed file${info.changed === 1 ? '' : 's'}`}>
          {info.changed > 99 ? '99+' : info.changed}
        </span>
      )}
    </>
  )
}

function RepoPopoverBody({ projectId, anchor, from }: { projectId: string; anchor: DOMRect | null; from: From }) {
  const close = useGitUi((s) => s.closeRepoPopover)
  const { repo: current, repos } = useActiveRepo(projectId)
  const infos = useRepoInfos(projectId)
  const [active, setActive] = useState(() => Math.max(0, repos.findIndex((r) => r.id === current?.id)))
  const ref = useRef<HTMLDivElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const refetch = infos.refetch

  // The branch and counts are fresh when the list opens.
  useEffect(() => {
    void refetch()
    listRef.current?.focus()
  }, [refetch])

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as HTMLElement
      // The buttons toggle the list themselves.
      if (ref.current?.contains(t) || t.closest?.('[data-git-repo-anchor]')) return
      close()
    }
    window.addEventListener('mousedown', onDown, true)
    return () => window.removeEventListener('mousedown', onDown, true)
  }, [close])

  const pick = (r: RepoSummary) => {
    setActiveRepo(projectId, r.id)
    close()
  }
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault()
      close()
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      setActive((i) => (i + (e.key === 'ArrowDown' ? 1 : -1) + repos.length) % repos.length)
    } else if (e.key === 'Home' || e.key === 'End') {
      e.preventDefault()
      setActive(e.key === 'Home' ? 0 : repos.length - 1)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      if (repos[active]) pick(repos[active])
    }
  }

  const width = Math.min(420, window.innerWidth - 16)
  const left = anchor ? Math.max(8, Math.min(anchor.left, window.innerWidth - width - 8)) : (window.innerWidth - width) / 2
  const style: React.CSSProperties =
    from === 'statusbar' && anchor ? { left, bottom: window.innerHeight - anchor.top + 4 } : { left, top: anchor ? anchor.bottom + 4 : 56 }

  return (
    <div ref={ref} className="git-popover git-repo-popover" style={{ ...style, width }} role="dialog" aria-label="Repositories">
      <div className="git-pop-head">
        Repositories {infos.isFetching && <Spinner size={11} />}
      </div>
      <div ref={listRef} className="git-pop-list" role="listbox" aria-label="Repositories" tabIndex={-1} onKeyDown={onKeyDown}>
        {repos.map((r, i) => (
          <div
            key={r.id}
            role="option"
            aria-selected={r.id === current?.id}
            className={`git-pop-row git-repo-row${i === active ? ' active' : ''}${r.id === current?.id ? ' current' : ''}`}
            onMouseMove={() => i !== active && setActive(i)}
            onClick={() => pick(r)}
            title={r.remote ?? undefined}
          >
            {r.id === current?.id ? <Check size={14} className="icon" /> : <FolderGit2 size={14} className="icon" />}
            <span className="label">{r.name}</span>
            {r.path && <span className="git-repo-path">{r.path}</span>}
            <span className="meta">
              <Meta info={infos.data?.find((x) => x.id === r.id)} />
            </span>
          </div>
        ))}
      </div>
    </div>
  )
}
