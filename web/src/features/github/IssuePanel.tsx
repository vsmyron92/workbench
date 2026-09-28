// 'gh.issue' panel: an issue with its description, comments, a comment box
// and close (completed / not planned) / reopen.

import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { CircleCheck, CircleSlash, ExternalLink } from 'lucide-react'
import { toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { Badge, Button, ErrorBox, Loading, Markdown, showMenuAt, TimeAgo } from '@/ui'
import { ghApi, ghk, useGithubSummary, useIssue } from './api'
import { Avatar, ExtLink } from './components'
import { IssueStateIcon } from './icons'
import { Composer, Note } from './PrConversation'

export interface IssueParams {
  projectId: string
  number: number
}

export function IssuePanel({ params, setTitle }: PanelProps<IssueParams>) {
  return <IssueView projectId={params.projectId} number={params.number} setTitle={setTitle} />
}

/** An issue with its comments (the panel, and the phone tab's detail view). */
export function IssueView({ projectId, number, setTitle }: { projectId: string; number: number; setTitle?: (title: string) => void }) {
  const qc = useQueryClient()
  const q = useIssue(projectId, number)
  const summary = useGithubSummary(projectId)
  const anonymous = summary.data ? !summary.data.auth.authenticated : false
  const [busy, setBusy] = useState<string | null>(null)
  const issue = q.data?.issue
  useEffect(() => {
    if (issue) setTitle?.(`#${issue.number} ${issue.title.length > 36 ? issue.title.slice(0, 35) + '…' : issue.title}`)
  }, [issue?.number, issue?.title, setTitle]) // eslint-disable-line react-hooks/exhaustive-deps

  if (q.error && !q.data) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!q.data || !issue) return <Loading />

  const refresh = () => qc.invalidateQueries({ queryKey: ghk.issue(projectId, number) })
  const setState = async (state: 'open' | 'closed', stateReason?: string) => {
    setBusy(state)
    try {
      await ghApi.updateIssue(projectId, number, { state, stateReason })
      toast('success', state === 'closed' ? `Closed #${number}` : `Reopened #${number}`)
      await refresh()
      void qc.invalidateQueries({ queryKey: ghk.issues(projectId) })
    } catch (e) {
      toastError(e)
    } finally {
      setBusy(null)
    }
  }
  const tokenTitle = anonymous ? 'Needs a GitHub token' : undefined
  return (
    <div className="wb-fill">
      <div className="gh-pr-head">
        <div className="wb-row" style={{ alignItems: 'flex-start', gap: 8 }}>
          <span style={{ paddingTop: 2 }}>
            <IssueStateIcon state={issue.state} reason={issue.stateReason} size={18} />
          </span>
          <h2 className="wb-grow">
            {issue.title} <span className="wb-muted" style={{ fontWeight: 400 }}>#{issue.number}</span>
          </h2>
          {issue.state === 'open' ? (
            <Button
              size="small"
              loading={busy === 'closed'}
              disabled={anonymous}
              title={tokenTitle}
              onClick={(e) =>
                showMenuAt(e.currentTarget, [
                  { label: 'Close as completed', icon: CircleCheck, run: () => void setState('closed', 'completed') },
                  { label: 'Close as not planned', icon: CircleSlash, run: () => void setState('closed', 'not_planned') },
                ])
              }
            >
              Close issue…
            </Button>
          ) : (
            <Button size="small" loading={busy === 'open'} disabled={anonymous} title={tokenTitle} onClick={() => setState('open')}>
              Reopen
            </Button>
          )}
          <ExtLink href={issue.htmlUrl}>
            <span className="wb-icon-btn small">
              <ExternalLink size={14} />
            </span>
          </ExtLink>
        </div>
        <div className="line">
          {issue.state === 'closed' ? (
            <Badge tone={issue.stateReason === 'not_planned' ? undefined : 'accent'}>{issue.stateReason === 'not_planned' ? 'Not planned' : 'Closed'}</Badge>
          ) : (
            <Badge tone="success">Open</Badge>
          )}
          <Avatar user={issue.user} small />
          <span>{issue.user?.login}</span>
          <span className="gh-sep">opened</span>
          <TimeAgo time={issue.createdAt} />
          {issue.assignees.length > 0 && (
            <>
              <span className="gh-sep">· assigned to</span>
              <span>{issue.assignees.map((a) => a.login).join(', ')}</span>
            </>
          )}
          {issue.milestone && <span className="gh-label">{issue.milestone.title}</span>}
          {issue.labels.map((l) => (
            <span className="gh-label" key={l.id || l.name}>
              {l.name}
            </span>
          ))}
        </div>
      </div>
      <div className="wb-scroll">
        <div style={{ padding: '12px 14px' }}>
          {issue.body?.trim() ? <Markdown text={issue.body} /> : <div className="wb-muted">No description.</div>}
        </div>
        <div className="gh-threads">
          {q.data.comments.map((c) => (
            <div className="gh-thread" key={c.id}>
              <Note user={c.user} body={c.body} time={c.createdAt} />
            </div>
          ))}
          <div className="gh-thread">
            <Composer
              placeholder="Add a comment…"
              submitLabel="Comment"
              disabled={anonymous ? 'Commenting needs a GitHub token.' : issue.locked ? 'This conversation is locked.' : false}
              onSubmit={async (body) => {
                await ghApi.issueComment(projectId, number, body)
                await refresh()
              }}
            />
          </div>
        </div>
      </div>
    </div>
  )
}
