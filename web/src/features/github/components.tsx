// Small shared pieces of the GitHub feature: the GitHub mark, status icons,
// run rows, avatars, the public-mode banner, panel openers and the UI store.

import { useState, type ComponentType, type MouseEvent, type ReactNode } from 'react'
import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import {
  ChevronDown,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleHelp,
  CirclePause,
  CircleSlash,
  CircleX,
  ExternalLink,
  GitBranch,
  Globe,
  Hourglass,
  LoaderCircle,
  Tag,
} from 'lucide-react'
import { openPanel, toast } from '@/shell/actions'
import { formatDuration, TimeAgo } from '@/ui'
import { elapsedSeconds, eventLabel, ghLabel, initials, rateLow, rateText, rowKeys, shortSha, stateLabel, stateTone } from './logic'
import type { GhUser, GithubSummary, Run } from './types'
import './github.css'

// ---------------------------------------------------------------- the mark

/** The GitHub mark (single colour, currentColor). */
export function GitHubIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden>
      <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
    </svg>
  )
}

// ---------------------------------------------------------------- UI store

export type GhTab = 'runs' | 'pulls' | 'issues' | 'releases'

interface GhUi {
  tab: GhTab
  setTab: (t: GhTab) => void
  /** Project whose "Create pull request" dialog is open. */
  createPr: string | null
  openCreatePr: (pid: string) => void
  /** Project (and workflow) whose "Run workflow" dialog is open. */
  runWorkflow: { pid: string; workflowId: number | null } | null
  openRunWorkflow: (pid: string, workflowId?: number | null) => void
  closeDialogs: () => void
}

export const useGhUi = create<GhUi>()(
  persist(
    (set) => ({
      tab: 'runs',
      setTab: (tab) => set({ tab }),
      createPr: null,
      openCreatePr: (pid) => set({ createPr: pid, runWorkflow: null }),
      runWorkflow: null,
      openRunWorkflow: (pid, workflowId = null) => set({ runWorkflow: { pid, workflowId }, createPr: null }),
      closeDialogs: () => set({ createPr: null, runWorkflow: null }),
    }),
    { name: 'wb.github.v1', partialize: (s) => ({ tab: s.tab }) },
  ),
)

// ---------------------------------------------------------------- panels

const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n - 1)}…` : s)

export function runTitle(r: Pick<Run, 'name' | 'runNumber'> | null | undefined, id: number): string {
  return r ? `${r.name ?? 'Run'} #${r.runNumber}` : `Run ${id}`
}

export function openRun(pid: string, id: number, title?: string) {
  openPanel({ kind: 'gh.run', id: `gh.run:${pid}:${id}`, title: title ? clip(title, 32) : `Run ${id}`, params: { projectId: pid, runId: id } })
}

export function openJob(pid: string, id: number, name?: string) {
  openPanel({ kind: 'gh.job', id: `gh.job:${pid}:${id}`, title: name ? clip(name, 32) : `Job ${id}`, params: { projectId: pid, jobId: id } })
}

export function openPr(pid: string, n: number, title?: string) {
  openPanel({ kind: 'pr', id: `pr:${pid}:${n}`, title: title ? `#${n} ${clip(title, 36)}` : `#${n}`, params: { projectId: pid, number: n } })
}

export function openIssue(pid: string, n: number, title?: string) {
  openPanel({
    kind: 'gh.issue',
    id: `gh.issue:${pid}:${n}`,
    title: title ? `#${n} ${clip(title, 36)}` : `#${n}`,
    params: { projectId: pid, number: n },
  })
}

export async function copyText(text: string, what = 'Copied') {
  try {
    await navigator.clipboard.writeText(text)
    toast('success', what)
  } catch {
    toast('error', 'The clipboard is not available here')
  }
}

// ---------------------------------------------------------------- pieces

type Icon = ComponentType<{ size?: number }>

const STATE_ICONS: Record<string, Icon> = {
  success: CircleCheck,
  failed: CircleX,
  running: LoaderCircle,
  pending: CirclePause,
  canceled: CircleSlash,
  skipped: CircleDashed,
  manual: Hourglass,
}

/** A state icon (shared vocabulary); `title` defaults to the state in words. */
export function StatusIcon({ state, size = 14, title }: { state: string | null | undefined; size?: number; title?: string }) {
  const I = (state && STATE_ICONS[state]) || CircleHelp
  const cls = `gh-status gh-tone-${stateTone(state)}${state === 'running' ? ' gh-spin' : ''}`
  return (
    <span className={cls} title={title ?? stateLabel(state)} aria-label={title ?? stateLabel(state)}>
      <I size={size} />
    </span>
  )
}

/** GitHub's word for a run/job/step, coloured by its state. */
export function StatusText({ state, status, conclusion }: { state: string; status?: string; conclusion?: string | null }) {
  const label = status !== undefined ? ghLabel(status, conclusion) : stateLabel(state)
  return <span className={`gh-status-text gh-tone-${stateTone(state)}`}>{label}</span>
}

export function Avatar({ user, small }: { user: Pick<GhUser, 'login' | 'name'> | null | undefined; small?: boolean }) {
  return (
    <span className={small ? 'gh-avatar small' : 'gh-avatar'} title={user ? (user.name ? `${user.name} (@${user.login})` : `@${user.login}`) : undefined}>
      {initials(user)}
    </span>
  )
}

export function RefLabel({ name, tag }: { name: string; tag?: boolean }) {
  const I = tag ? Tag : GitBranch
  return (
    <span className="gh-ref" title={name}>
      <I size={11} />
      <span>{name}</span>
    </span>
  )
}

export function ExtLink({ href, children, title }: { href: string | null | undefined; children?: ReactNode; title?: string }) {
  if (!href) return null
  return (
    <a className="gh-link wb-row" href={href} target="_blank" rel="noopener noreferrer" title={title ?? 'Open on GitHub'}>
      {children ?? <ExternalLink size={13} />}
    </a>
  )
}

export function Duration({ p }: { p: { duration: number | null; state: string; startedAt?: string | null; runStartedAt?: string | null } }) {
  const s = elapsedSeconds(p)
  return <>{s === null ? '' : formatDuration(s)}</>
}

/** One workflow run in a list: state, workflow and number, title; branch, event, actor, age. */
export function RunRow({
  r,
  onOpen,
  onContextMenu,
  selected,
}: {
  r: Run
  onOpen: () => void
  onContextMenu?: (e: MouseEvent) => void
  selected?: boolean
}) {
  return (
    <div className={selected ? 'gh-row selected' : 'gh-row'} onClick={onOpen} onContextMenu={onContextMenu} {...rowKeys(onOpen)}>
      <StatusIcon state={r.state} size={15} title={ghLabel(r.status, r.conclusion)} />
      <span className="title">
        {r.displayTitle ?? r.commitTitle ?? <span className="wb-muted">{ghLabel(r.status, r.conclusion)}</span>}
      </span>
      <span className="right">
        <Duration p={r} />
      </span>
      <span className="meta">
        <span className="gh-wf">
          {r.name ?? 'Run'} #{r.runNumber}
        </span>
        {r.headBranch && <RefLabel name={r.headBranch} />}
        <span>{eventLabel(r.event)}</span>
        {r.actor && <span className="wb-ellipsis">{r.actor.login}</span>}
      </span>
      <span className="right">
        <TimeAgo time={r.createdAt} />
      </span>
    </div>
  )
}

/**
 * Without a token GitHub serves public repositories read-only, 60 requests an
 * hour: say so once, compactly, with the quota and how to add a token.
 */
export function PublicModeBanner({ s }: { s: GithubSummary }) {
  const [open, setOpen] = useState(false)
  const rate = s.auth.rate
  const low = rateLow(rate)
  if (s.auth.authenticated) {
    if (!low) return null
    return (
      <div className="gh-banner warning">
        <CircleAlert size={14} />
        <span className="wb-grow">GitHub rate limit nearly used up: {rateText(rate)}</span>
      </div>
    )
  }
  return (
    <div className={low ? 'gh-banner warning gh-public' : 'gh-banner info gh-public'}>
      <div className="wb-row" style={{ gap: 6, cursor: 'pointer' }} onClick={() => setOpen(!open)} role="button" aria-expanded={open}>
        {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <Globe size={13} />
        <span className="wb-grow wb-ellipsis">
          <b>Public, read-only</b>
          <span className="wb-muted"> · {rateText(rate) ?? '60 requests an hour'}</span>
        </span>
      </div>
      {open && (
        <div className="gh-public-body wb-small">
          <p>
            No GitHub token is set up for {s.host}, so Workbench reads this public repository anonymously. GitHub allows 60 requests an hour
            without a token, so views refresh slowly, job logs are not available, and nothing can be changed from here.
          </p>
          {s.auth.reason && <p className="wb-muted">{s.auth.reason}</p>}
          <p>
            Add a token (a fine-grained token with read access is enough for logs):
            <code className="gh-code">[github] token = "github"</code>
            <code className="gh-code">[secrets] github = {'{'} file = "~/.github_token" {'}'}</code>
          </p>
        </div>
      )}
    </div>
  )
}

export { shortSha }
