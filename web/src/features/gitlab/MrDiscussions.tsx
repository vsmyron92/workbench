// MR "Discussions" tab: threads with replies, resolve/unresolve, a new
// comment box, and (optionally) GitLab's activity notes.

import { useState } from 'react'
import type { UseQueryResult } from '@tanstack/react-query'
import { useQueryClient } from '@tanstack/react-query'
import { CircleCheck, FileText, MessageSquarePlus } from 'lucide-react'
import { toastError } from '@/shell/actions'
import { Button, Checkbox, EmptyState, ErrorBox, Loading, Markdown, Select, TextArea, TimeAgo } from '@/ui'
import { glApi, glk } from './api'
import { Avatar } from './components'
import type { Discussion, Mr, Note } from './types'

function Composer({
  placeholder,
  submitLabel,
  onSubmit,
  onCancel,
  autoFocus,
}: {
  placeholder: string
  submitLabel: string
  onSubmit: (body: string) => Promise<void>
  onCancel?: () => void
  autoFocus?: boolean
}) {
  const [body, setBody] = useState('')
  const [busy, setBusy] = useState(false)
  const submit = async () => {
    if (!body.trim()) return
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
    <div className="gl-compose">
      <TextArea
        value={body}
        placeholder={placeholder}
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
        <Button size="small" variant="primary" loading={busy} disabled={!body.trim()} onClick={submit}>
          {submitLabel}
        </Button>
      </div>
    </div>
  )
}

function NoteView({ n }: { n: Note }) {
  if (n.system) {
    return (
      <div className="gl-note system">
        <span>
          <b>{n.author?.username ?? 'GitLab'}</b> {n.body.split('\n')[0]} · <TimeAgo time={n.createdAt} />
        </span>
      </div>
    )
  }
  return (
    <div className="gl-note">
      <Avatar user={n.author} />
      <div className="body">
        <div className="who">
          <b>{n.author?.name ?? '?'}</b>
          <span className="wb-subtle">@{n.author?.username}</span>
          <span className="wb-subtle">
            · <TimeAgo time={n.createdAt} />
          </span>
          {n.resolved && n.resolvable && <CircleCheck size={12} className="wb-success" />}
        </div>
        <Markdown text={n.body} />
      </div>
    </div>
  )
}

function Thread({
  projectId,
  mr,
  d,
  onOpenFile,
}: {
  projectId: string
  mr: Mr
  d: Discussion
  onOpenFile: (path: string) => void
}) {
  const qc = useQueryClient()
  const [replying, setReplying] = useState(false)
  const [busy, setBusy] = useState(false)
  const first = d.notes[0]
  const p = first?.position
  const resolvable = d.notes.some((n) => n.resolvable)
  const resolved = resolvable && d.notes.filter((n) => n.resolvable).every((n) => n.resolved)
  const refresh = () => qc.invalidateQueries({ queryKey: glk.mrPart(projectId, mr.iid, 'discussions') })
  const toggleResolved = async () => {
    setBusy(true)
    try {
      await glApi.resolve(projectId, mr.iid, d.id, !resolved)
      await refresh()
    } catch (e) {
      toastError(e)
    } finally {
      setBusy(false)
    }
  }
  if (d.individualNote && first?.system) return <NoteView n={first} />
  return (
    <div className={resolved ? 'gl-thread resolved' : 'gl-thread'}>
      {(p || resolvable) && (
        <div className="gl-thread-head">
          {p && (
            <a
              className="gl-link wb-row"
              href="#"
              onClick={(e) => {
                e.preventDefault()
                onOpenFile(p.newPath ?? p.oldPath ?? '')
              }}
            >
              <FileText size={12} />
              <span className="gl-mono">
                {p.newPath ?? p.oldPath}:{p.newLine ?? p.oldLine}
              </span>
            </a>
          )}
          <span style={{ flex: 1 }} />
          {resolved && (
            <span className="wb-success">
              resolved{d.notes.find((n) => n.resolvedBy)?.resolvedBy ? ` by ${d.notes.find((n) => n.resolvedBy)!.resolvedBy!.username}` : ''}
            </span>
          )}
          {resolvable && (
            <Button size="small" variant="ghost" loading={busy} onClick={toggleResolved}>
              {resolved ? 'Unresolve' : 'Resolve'}
            </Button>
          )}
        </div>
      )}
      {d.notes.map((n) => (
        <NoteView key={n.id} n={n} />
      ))}
      {replying ? (
        <Composer
          placeholder="Reply…"
          submitLabel="Reply"
          autoFocus
          onCancel={() => setReplying(false)}
          onSubmit={async (body) => {
            await glApi.reply(projectId, mr.iid, d.id, body)
            await refresh()
          }}
        />
      ) : (
        <div className="gl-compose" style={{ padding: '4px 6px', alignItems: 'flex-start' }}>
          <Button size="small" variant="ghost" onClick={() => setReplying(true)}>
            Reply…
          </Button>
        </div>
      )}
    </div>
  )
}

export function MrDiscussions({
  projectId,
  mr,
  query,
  onOpenFile,
}: {
  projectId: string
  mr: Mr
  query: UseQueryResult<Discussion[], unknown>
  onOpenFile: (path: string) => void
}) {
  const qc = useQueryClient()
  const [filter, setFilter] = useState<'all' | 'unresolved'>('all')
  const [activity, setActivity] = useState(false)
  if (query.error) return <ErrorBox error={query.error} onRetry={() => query.refetch()} />
  if (!query.data) return <Loading />
  const isSystem = (d: Discussion) => d.notes.every((n) => n.system)
  const shown = query.data.filter((d) => {
    if (isSystem(d)) return activity && filter === 'all'
    if (filter === 'unresolved') return d.notes.some((n) => n.resolvable && !n.resolved)
    return true
  })
  return (
    <div className="wb-fill">
      <div className="gl-filters">
        <Select value={filter} onChange={(e) => setFilter(e.target.value as 'all' | 'unresolved')}>
          <option value="all">All threads</option>
          <option value="unresolved">Unresolved</option>
        </Select>
        <Checkbox checked={activity} onChange={setActivity}>
          <span className="wb-small">Show activity</span>
        </Checkbox>
      </div>
      <div className="wb-scroll">
        {!mr.discussionLocked && (
          <Composer
            placeholder="Add a comment to this merge request…"
            submitLabel="Comment"
            onSubmit={async (body) => {
              await glApi.addNote(projectId, mr.iid, body)
              await qc.invalidateQueries({ queryKey: glk.mrPart(projectId, mr.iid, 'discussions') })
            }}
          />
        )}
        {shown.length ? (
          <div className="gl-threads">
            {shown.map((d) => (
              <Thread key={d.id} projectId={projectId} mr={mr} d={d} onOpenFile={onOpenFile} />
            ))}
          </div>
        ) : (
          <EmptyState icon={MessageSquarePlus} title={filter === 'unresolved' ? 'No unresolved threads' : 'No comments yet'}>
            Right-click a line in Changes to start a review thread.
          </EmptyState>
        )}
      </div>
    </div>
  )
}
