// Pull request "Conversation": a timeline of comments, reviews and review
// threads (with their diff context), replies, resolve/unresolve (GraphQL, so
// only with a token), and a comment box.

import { useMemo, useState, type ReactNode } from 'react'
import type { UseQueryResult } from '@tanstack/react-query'
import { useQueryClient } from '@tanstack/react-query'
import { CircleCheck, FileText, MessageSquarePlus, MessageSquareWarning } from 'lucide-react'
import { toastError } from '@/shell/actions'
import { Badge, Button, EmptyState, ErrorBox, Loading, Markdown, Select, TextArea, TimeAgo } from '@/ui'
import { ghApi, ghk, usePrComments, useReviews } from './api'
import { Avatar, StatusIcon } from './components'
import type { GhUser, IssueComment, PullDetail, Review, Thread } from './types'

const NEEDS_TOKEN = 'Commenting needs a GitHub token.'

export function Composer({
  placeholder,
  submitLabel,
  onSubmit,
  onCancel,
  autoFocus,
  disabled,
}: {
  placeholder: string
  submitLabel: string
  onSubmit: (body: string) => Promise<void>
  onCancel?: () => void
  autoFocus?: boolean
  disabled?: string | false
}) {
  const [body, setBody] = useState('')
  const [busy, setBusy] = useState(false)
  const submit = async () => {
    if (!body.trim() || disabled) return
    setBusy(true)
    try {
      await onSubmit(body)
      setBody('')
      onCancel?.()
    } catch (e) {
      toastError(e, 'Could not post the comment')
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="gh-compose">
      <TextArea
        value={body}
        placeholder={disabled || placeholder}
        disabled={!!disabled}
        autoFocus={autoFocus}
        onChange={(e) => setBody(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) void submit()
          if (e.key === 'Escape') onCancel?.()
        }}
      />
      <div className="bar">
        <span className="wb-small wb-subtle">Markdown · Ctrl+Enter to send</span>
        <span style={{ flex: 1 }} />
        {onCancel && (
          <Button size="small" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
        )}
        <Button size="small" variant="primary" loading={busy} disabled={!body.trim() || !!disabled} onClick={submit}>
          {submitLabel}
        </Button>
      </div>
    </div>
  )
}

export function Note({ user, body, time, extra }: { user: GhUser | null; body: string; time: string | null; extra?: ReactNode }) {
  return (
    <div className="gh-note">
      <Avatar user={user} />
      <div className="body">
        <div className="who">
          <b>{user?.login ?? '?'}</b>
          <span className="wb-subtle">
            · <TimeAgo time={time} />
          </span>
          {extra}
        </div>
        <Markdown text={body} />
      </div>
    </div>
  )
}

/** The last few lines of a comment's diff hunk, coloured. */
function Hunk({ hunk }: { hunk: string }) {
  const lines = hunk.split('\n').slice(-5)
  return (
    <pre className="gh-hunk">
      {lines.map((l, i) => (
        <div key={i} className={l.startsWith('+') ? 'a' : l.startsWith('-') ? 'd' : undefined}>
          {l || ' '}
        </div>
      ))}
    </pre>
  )
}

function ThreadView({
  projectId,
  pr,
  t,
  anonymous,
  onOpenFile,
}: {
  projectId: string
  pr: PullDetail
  t: Thread
  anonymous: boolean
  onOpenFile: (path: string) => void
}) {
  const qc = useQueryClient()
  const [replying, setReplying] = useState(false)
  const [busy, setBusy] = useState(false)
  const refresh = () => qc.invalidateQueries({ queryKey: ghk.pullPart(projectId, pr.number, 'threads') })
  const toggle = async () => {
    setBusy(true)
    try {
      await ghApi.resolve(projectId, pr.number, t.id, !t.resolved)
      await refresh()
    } catch (e) {
      toastError(e)
    } finally {
      setBusy(false)
    }
  }
  const first = t.comments[0]
  const line = t.line ?? t.originalLine
  return (
    <div className={t.resolved ? 'gh-thread resolved' : 'gh-thread'}>
      <div className="gh-thread-head">
        <a
          className="gh-link wb-row"
          href="#"
          onClick={(e) => {
            e.preventDefault()
            onOpenFile(t.path)
          }}
        >
          <FileText size={12} />
          <span className="gh-mono wb-ellipsis">
            {t.path}
            {line ? `:${line}` : ''}
          </span>
        </a>
        {t.outdated && <Badge>Outdated</Badge>}
        <span style={{ flex: 1 }} />
        {t.resolved && <span className="wb-success">resolved{t.resolvedBy ? ` by ${t.resolvedBy}` : ''}</span>}
        {t.canResolve && !anonymous && t.resolved !== null && (
          <Button size="small" variant="ghost" loading={busy} onClick={toggle}>
            {t.resolved ? 'Unresolve' : 'Resolve'}
          </Button>
        )}
      </div>
      {first?.diffHunk && <Hunk hunk={first.diffHunk} />}
      {t.comments.map((c) => (
        <Note key={c.id} user={c.user} body={c.body} time={c.createdAt} />
      ))}
      {replying ? (
        <Composer
          placeholder="Reply…"
          submitLabel="Reply"
          autoFocus
          onCancel={() => setReplying(false)}
          onSubmit={async (body) => {
            await ghApi.reply(projectId, pr.number, t.rootId, body)
            await refresh()
          }}
        />
      ) : (
        !anonymous && (
          <div className="gh-compose" style={{ padding: '4px 6px', alignItems: 'flex-start' }}>
            <Button size="small" variant="ghost" onClick={() => setReplying(true)}>
              Reply…
            </Button>
          </div>
        )
      )}
    </div>
  )
}

type Item =
  | { kind: 'comment'; at: string; c: IssueComment }
  | { kind: 'review'; at: string; r: Review }
  | { kind: 'thread'; at: string; t: Thread }

const REVIEW_WORDS: Record<string, string> = {
  APPROVED: 'approved these changes',
  CHANGES_REQUESTED: 'requested changes',
  COMMENTED: 'reviewed',
  DISMISSED: 'had a review dismissed',
}

export function PrConversation({
  projectId,
  pr,
  threads,
  anonymous,
  onOpenFile,
}: {
  projectId: string
  pr: PullDetail
  threads: UseQueryResult<Thread[], unknown>
  anonymous: boolean
  onOpenFile: (path: string) => void
}) {
  const qc = useQueryClient()
  const comments = usePrComments(projectId, pr.number)
  const reviews = useReviews(projectId, pr.number)
  const [filter, setFilter] = useState<'all' | 'unresolved'>('all')
  const items = useMemo(() => {
    const out: Item[] = []
    for (const c of comments.data ?? []) out.push({ kind: 'comment', at: c.createdAt ?? '', c })
    for (const r of reviews.data ?? []) {
      // Reviews that are only their line comments show as those threads.
      if (r.state === 'COMMENTED' && !r.body?.trim()) continue
      if (r.state === 'PENDING') continue
      out.push({ kind: 'review', at: r.submittedAt ?? '', r })
    }
    for (const t of threads.data ?? []) out.push({ kind: 'thread', at: t.comments[0]?.createdAt ?? '', t })
    return out.sort((a, b) => a.at.localeCompare(b.at))
  }, [comments.data, reviews.data, threads.data])
  const error = comments.error ?? threads.error
  if (error) return <ErrorBox error={error} onRetry={() => (comments.refetch(), threads.refetch())} />
  if (!comments.data || !threads.data) return <Loading />
  const shown = filter === 'unresolved' ? items.filter((i) => i.kind === 'thread' && i.t.resolved === false) : items
  const unknownResolution = (threads.data ?? []).some((t) => t.resolved === null)
  return (
    <div className="wb-fill">
      <div className="gh-filters">
        <Select value={filter} onChange={(e) => setFilter(e.target.value as 'all' | 'unresolved')}>
          <option value="all">Everything</option>
          <option value="unresolved" disabled={unknownResolution}>
            Unresolved threads
          </option>
        </Select>
        <span className="wb-small wb-subtle wb-ellipsis">
          {comments.data.length} comments · {threads.data.length} review threads
        </span>
      </div>
      <div className="wb-scroll">
        <Composer
          placeholder="Add a comment to this pull request…"
          submitLabel="Comment"
          disabled={anonymous ? NEEDS_TOKEN : pr.locked ? 'This conversation is locked.' : false}
          onSubmit={async (body) => {
            await ghApi.comment(projectId, pr.number, body)
            await qc.invalidateQueries({ queryKey: ghk.pullPart(projectId, pr.number, 'comments') })
          }}
        />
        {shown.length ? (
          <div className="gh-threads">
            {shown.map((i) =>
              i.kind === 'comment' ? (
                <div className="gh-thread" key={`c${i.c.id}`}>
                  <Note user={i.c.user} body={i.c.body} time={i.c.createdAt} />
                </div>
              ) : i.kind === 'review' ? (
                <div className="gh-thread" key={`r${i.r.id}`}>
                  <div className="gh-thread-head">
                    {i.r.state === 'APPROVED' ? (
                      <CircleCheck size={13} className="wb-success" />
                    ) : i.r.state === 'CHANGES_REQUESTED' ? (
                      <MessageSquareWarning size={13} className="wb-danger" />
                    ) : (
                      <StatusIcon state={null} size={13} />
                    )}
                    <b>{i.r.user?.login}</b>
                    <span>{REVIEW_WORDS[i.r.state] ?? i.r.state.toLowerCase()}</span>
                    <span>
                      · <TimeAgo time={i.r.submittedAt} />
                    </span>
                  </div>
                  {i.r.body?.trim() && <Note user={i.r.user} body={i.r.body} time={i.r.submittedAt} />}
                </div>
              ) : (
                <ThreadView key={`t${i.t.id}`} projectId={projectId} pr={pr} t={i.t} anonymous={anonymous} onOpenFile={onOpenFile} />
              ),
            )}
          </div>
        ) : (
          <EmptyState icon={MessageSquarePlus} title={filter === 'unresolved' ? 'No unresolved threads' : 'No conversation yet'}>
            {!anonymous && 'Right-click a line in Files changed to start a review thread.'}
          </EmptyState>
        )}
      </div>
    </div>
  )
}
