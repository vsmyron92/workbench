// Small shared pieces of the GitLab feature: status icons, rows, avatars,
// panel openers and the feature's UI store.

import type { ComponentType, MouseEvent, ReactNode } from 'react'
import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import {
  CircleAlert,
  CircleCheck,
  CircleChevronRight,
  CircleDashed,
  CircleHelp,
  CirclePause,
  CirclePlay,
  CircleSlash,
  CircleX,
  Clock,
  ExternalLink,
  GitBranch,
  LoaderCircle,
  Tag,
} from 'lucide-react'
import { openPanel, toast } from '@/shell/actions'
import { formatDuration, TimeAgo } from '@/ui'
import { elapsedSeconds, initials, rowKeys, shortSha, statusLabel, statusTone } from './logic'
import type { GlUser, Pipeline } from './types'
import './gitlab.css'

// ---------------------------------------------------------------- UI store

export type GlTab = 'pipelines' | 'mrs' | 'issues' | 'envs' | 'registry'

interface GlUi {
  tab: GlTab
  setTab: (t: GlTab) => void
  /** Project whose "Create merge request" dialog is open. */
  createMr: string | null
  openCreateMr: (pid: string) => void
  /** Project whose "Run pipeline" dialog is open. */
  runPipeline: string | null
  openRunPipeline: (pid: string) => void
  closeDialogs: () => void
}

export const useGlUi = create<GlUi>()(
  persist(
    (set) => ({
      tab: 'pipelines',
      setTab: (tab) => set({ tab }),
      createMr: null,
      openCreateMr: (pid) => set({ createMr: pid, runPipeline: null }),
      runPipeline: null,
      openRunPipeline: (pid) => set({ runPipeline: pid, createMr: null }),
      closeDialogs: () => set({ createMr: null, runPipeline: null }),
    }),
    { name: 'wb.gitlab.v1', partialize: (s) => ({ tab: s.tab }) },
  ),
)

// ---------------------------------------------------------------- panels

const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n - 1)}…` : s)

export function openPipeline(pid: string, id: number, iid?: number | null) {
  openPanel({
    kind: 'pipeline',
    id: `pipeline:${pid}:${id}`,
    title: `Pipeline #${iid ?? id}`,
    params: { projectId: pid, pipelineId: id },
  })
}

export function openJob(pid: string, id: number, name?: string) {
  openPanel({ kind: 'job', id: `job:${pid}:${id}`, title: name ? clip(name, 32) : `Job ${id}`, params: { projectId: pid, jobId: id } })
}

export function openMr(pid: string, iid: number, title?: string) {
  openPanel({
    kind: 'mr',
    id: `mr:${pid}:${iid}`,
    title: title ? `!${iid} ${clip(title, 36)}` : `!${iid}`,
    params: { projectId: pid, iid },
  })
}

export function openIssue(pid: string, iid: number, title?: string) {
  openPanel({
    kind: 'gitlab.issue',
    id: `gitlab.issue:${pid}:${iid}`,
    title: title ? `#${iid} ${clip(title, 36)}` : `#${iid}`,
    params: { projectId: pid, iid },
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

const STATUS_ICONS: Record<string, Icon> = {
  success: CircleCheck,
  success_with_warnings: CircleAlert,
  failed: CircleX,
  running: LoaderCircle,
  pending: CirclePause,
  waiting_for_resource: CirclePause,
  preparing: CircleDashed,
  created: CircleDashed,
  canceled: CircleSlash,
  canceling: CircleSlash,
  skipped: CircleChevronRight,
  manual: CirclePlay,
  scheduled: Clock,
}

export function StatusIcon({ status, size = 14, title }: { status: string | null | undefined; size?: number; title?: string }) {
  const I = (status && STATUS_ICONS[status]) || CircleHelp
  const cls = `gl-status gl-tone-${statusTone(status)}${status === 'running' ? ' gl-spin' : ''}`
  return (
    <span className={cls} title={title ?? statusLabel(status)} aria-label={statusLabel(status)}>
      <I size={size} />
    </span>
  )
}

export function StatusText({ status }: { status: string | null | undefined }) {
  return <span className={`gl-status-text gl-tone-${statusTone(status)}`}>{statusLabel(status)}</span>
}

export function Avatar({ user, small }: { user: GlUser | null | undefined; small?: boolean }) {
  return (
    <span className={small ? 'gl-avatar small' : 'gl-avatar'} title={user ? `${user.name} (@${user.username})` : undefined}>
      {initials(user)}
    </span>
  )
}

export function RefLabel({ name, tag }: { name: string; tag?: boolean }) {
  const I = tag ? Tag : GitBranch
  return (
    <span className="gl-ref" title={name}>
      <I size={11} />
      <span>{name}</span>
    </span>
  )
}

export function ExtLink({ href, children, title }: { href: string | null | undefined; children?: ReactNode; title?: string }) {
  if (!href) return null
  return (
    <a className="gl-link wb-row" href={href} target="_blank" rel="noopener noreferrer" title={title ?? 'Open in GitLab'}>
      {children ?? <ExternalLink size={13} />}
    </a>
  )
}

export function Duration({ p }: { p: { duration: number | null; startedAt: string | null; status: string } }) {
  const s = elapsedSeconds(p)
  return <>{s === null ? '' : formatDuration(s)}</>
}

/** One pipeline in a list: status, number, commit title; ref, sha, source, age. */
export function PipelineRow({
  p,
  onOpen,
  onContextMenu,
  selected,
}: {
  p: Pipeline
  onOpen: () => void
  onContextMenu?: (e: MouseEvent) => void
  selected?: boolean
}) {
  return (
    <div className={selected ? 'gl-row selected' : 'gl-row'} onClick={onOpen} onContextMenu={onContextMenu} {...rowKeys(onOpen)}>
      <StatusIcon status={p.status} size={15} />
      <span className="title">
        <span className="num">#{p.iid ?? p.id}</span>
        {p.commitTitle ?? <span className="wb-muted">{statusLabel(p.status)}</span>}
      </span>
      <span className="right">
        <Duration p={p} />
      </span>
      <span className="meta">
        <RefLabel name={p.ref} tag={p.tag} />
        <span className="gl-mono">{shortSha(p.sha)}</span>
        {p.source && <span>{p.source.replace(/_/g, ' ')}</span>}
        {p.user && <span className="wb-ellipsis">{p.user.username}</span>}
      </span>
      <span className="right">
        <TimeAgo time={p.createdAt} />
      </span>
    </div>
  )
}
