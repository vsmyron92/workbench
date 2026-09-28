// 'gitlab.issue' panel: an issue with its description, comments, a comment box
// and close/reopen.

import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { CircleDot, CircleX, ExternalLink } from 'lucide-react'
import { toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { Badge, Button, ErrorBox, Loading, Markdown, Spacer, TextArea, TimeAgo } from '@/ui'
import { glApi, glk, useGitlabSummary, useIssue } from './api'
import { Avatar, ExtLink } from './components'

export interface IssueParams {
  projectId: string
  iid: number
}

export function IssueStateIcon({ state, size = 15 }: { state: string; size?: number }) {
  return state === 'closed' ? (
    <span className="gl-status gl-tone-accent" title="closed">
      <CircleX size={size} />
    </span>
  ) : (
    <span className="gl-status gl-tone-success" title="open">
      <CircleDot size={size} />
    </span>
  )
}

export function IssuePanel({ params, setTitle }: PanelProps<IssueParams>) {
  const { projectId, iid } = params
  const qc = useQueryClient()
  const q = useIssue(projectId, iid)
  const summary = useGitlabSummary(projectId)
  const [body, setBody] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const issue = q.data?.issue
  useEffect(() => {
    if (issue) setTitle(`#${issue.iid} ${issue.title.length > 36 ? issue.title.slice(0, 35) + '…' : issue.title}`)
  }, [issue?.iid, issue?.title, setTitle]) // eslint-disable-line react-hooks/exhaustive-deps

  if (q.error && !q.data) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!q.data || !issue) return <Loading />

  const refresh = () => qc.invalidateQueries({ queryKey: glk.issue(projectId, iid) })
  const comment = async () => {
    setBusy('comment')
    try {
      await glApi.issueNote(projectId, iid, body)
      setBody('')
      await refresh()
    } catch (e) {
      toastError(e, 'Could not post the comment')
    } finally {
      setBusy(null)
    }
  }
  const setState = async (ev: 'close' | 'reopen') => {
    setBusy(ev)
    try {
      await glApi.updateIssue(projectId, iid, { stateEvent: ev })
      toast('success', ev === 'close' ? `Closed #${iid}` : `Reopened #${iid}`)
      await refresh()
      qc.invalidateQueries({ queryKey: glk.issues(projectId) })
    } catch (e) {
      toastError(e)
    } finally {
      setBusy(null)
    }
  }
  const notes = q.data.notes.filter((n) => !n.system)
  const web = summary.data?.webUrl
  return (
    <div className="wb-fill">
      <div className="gl-mr-head">
        <div className="wb-row" style={{ alignItems: 'flex-start', gap: 8 }}>
          <span style={{ paddingTop: 2 }}>
            <IssueStateIcon state={issue.state} size={18} />
          </span>
          <h2 className="wb-grow">
            {issue.title} <span className="wb-muted" style={{ fontWeight: 400 }}>#{issue.iid}</span>
          </h2>
          {issue.state === 'opened' ? (
            <Button size="small" loading={busy === 'close'} onClick={() => setState('close')}>
              Close issue
            </Button>
          ) : (
            <Button size="small" loading={busy === 'reopen'} onClick={() => setState('reopen')}>
              Reopen
            </Button>
          )}
          <ExtLink href={issue.webUrl}>
            <span className="wb-icon-btn small">
              <ExternalLink size={14} />
            </span>
          </ExtLink>
        </div>
        <div className="line">
          {issue.state === 'closed' ? <Badge tone="accent">Closed</Badge> : <Badge tone="success">Open</Badge>}
          {issue.confidential && <Badge tone="warning">Confidential</Badge>}
          <Avatar user={issue.author} small />
          <span>{issue.author?.username}</span>
          <span className="gl-sep">opened</span>
          <TimeAgo time={issue.createdAt} />
          {issue.assignees.length > 0 && (
            <>
              <span className="gl-sep">· assigned to</span>
              <span>{issue.assignees.map((a) => a.username).join(', ')}</span>
            </>
          )}
          {issue.milestone && <span className="gl-label">{issue.milestone.title}</span>}
          {issue.labels.map((l) => (
            <span className="gl-label" key={l}>
              {l}
            </span>
          ))}
        </div>
      </div>
      <div className="wb-scroll">
        <div style={{ padding: '12px 14px' }}>
          {issue.description?.trim() ? (
            <Markdown text={issue.description} resolveImage={(src) => (src.startsWith('/uploads/') && web ? `${web}${src}` : src)} />
          ) : (
            <div className="wb-muted">No description.</div>
          )}
        </div>
        <div className="gl-threads">
          {notes.map((n) => (
            <div className="gl-thread" key={n.id}>
              <div className="gl-note">
                <Avatar user={n.author} />
                <div className="body">
                  <div className="who">
                    <b>{n.author?.name}</b>
                    <span className="wb-subtle">
                      @{n.author?.username} · <TimeAgo time={n.createdAt} />
                    </span>
                  </div>
                  <Markdown text={n.body} />
                </div>
              </div>
            </div>
          ))}
          <div className="gl-thread">
            <div className="gl-compose" style={{ borderTop: 0 }}>
              <TextArea
                placeholder="Add a comment…"
                value={body}
                onChange={(e) => setBody(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && (e.ctrlKey || e.metaKey) && body.trim() && void comment()}
              />
              <div className="bar">
                <span className="wb-small wb-subtle">Markdown · Ctrl+Enter to send</span>
                <Spacer />
                <Button size="small" variant="primary" loading={busy === 'comment'} disabled={!body.trim()} onClick={comment}>
                  Comment
                </Button>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}
