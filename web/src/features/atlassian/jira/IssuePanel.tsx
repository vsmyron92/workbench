// The `jira` panel: an issue with its status (transition menu), fields, rendered
// description (edited as markdown → ADF), comments, and "Start agent on this issue".

import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, Bot, ChevronDown, Copy, ExternalLink, Pencil, RefreshCw, UserCheck } from 'lucide-react'
import { ApiError } from '@/api/client'
import { askAgent } from '@/shell/agentBridge'
import { toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { useUi } from '@/state/store'
import { Badge, Button, EmptyState, ErrorBox, IconButton, Input, JiraIcon, Loading, Select, showMenuAt, Spacer, TextArea, TimeAgo, Toolbar } from '@/ui'
import { jiraApi, qk, useIssue, type Issue } from '../api'
import { initials, issuePrompt, statusTone } from '../links'
import { StaleNotice } from '../StaleNotice'
import { usePrefs } from '../state'
import { copyText, openJiraIssue } from '../confluence/actions'
import { sanitize } from '../confluence/PageView'
import { SetupHint } from '../confluence/ToolWindow'
import { IssueTypeIcon } from './common'

export interface JiraParams {
  key: string
  projectId?: string
}

function HtmlBlock({ html }: { html: string }) {
  const onClick = (e: React.MouseEvent) => {
    const a = (e.target as HTMLElement).closest('a')
    if (!a) return
    e.preventDefault()
    if (a.dataset.wbIssue) openJiraIssue(a.dataset.wbIssue)
    else if (a.getAttribute('href')) window.open(a.getAttribute('href')!, '_blank', 'noopener,noreferrer')
  }
  return <div className="wb-prose wb-cf" onClick={onClick} dangerouslySetInnerHTML={{ __html: sanitize(html) }} />
}

export function JiraPanel({ params, setTitle }: PanelProps<JiraParams>) {
  const qc = useQueryClient()
  const uiProject = useUi((s) => s.projectId)
  const projectId = params.projectId ?? uiProject
  const key = String(params.key ?? '').toUpperCase()
  const q = useIssue(projectId, key)
  const issue = q.data
  const addRecentIssue = usePrefs((s) => s.addRecentIssue)
  const [editingSummary, setEditingSummary] = useState<string | null>(null)
  const [editingDesc, setEditingDesc] = useState<string | null>(null)
  const [labels, setLabels] = useState<string | null>(null)
  const [comment, setComment] = useState('')
  const [busy, setBusy] = useState<string | null>(null)

  useEffect(() => {
    if (!issue) return
    setTitle(`${issue.key} ${issue.summary}`)
    addRecentIssue({ id: issue.key, summary: issue.summary })
  }, [issue?.key, issue?.summary]) // eslint-disable-line react-hooks/exhaustive-deps

  if (!/^[A-Z][A-Z0-9_]*-\d+$/.test(key)) return <EmptyState icon={JiraIcon} title="Not a Jira issue key">“{params.key}”</EmptyState>
  if (q.isLoading) return <Loading label={`Loading ${key}…`} />
  // A failed refresh keeps the issue (and edits in progress) on screen.
  if (q.error && !issue) {
    if (q.error instanceof ApiError && q.error.notConfigured) return <SetupHint error={q.error} />
    return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  }
  if (!issue) return null

  const act = async (what: string, fn: () => Promise<unknown>, done: string) => {
    setBusy(what)
    try {
      await fn()
      toast('success', done)
      await qc.invalidateQueries({ queryKey: qk.issue(projectId, key) })
      qc.invalidateQueries({ queryKey: ['jira', 'search'] })
      return true
    } catch (e) {
      toastError(e, `${key}`)
      return false
    } finally {
      setBusy(null)
    }
  }

  const transitionMenu = (el: HTMLElement) =>
    showMenuAt(
      el,
      issue.transitions.length
        ? issue.transitions.map((t) => ({
            label: t.to && t.to.name !== t.name ? `${t.name} → ${t.to.name}` : t.name,
            run: () => {
              if (t.hasScreen) toast('info', `“${t.name}” asks for more fields; if it fails, finish it in Jira`)
              void act('transition', () => jiraApi.transition(projectId, key, t.id), `${key}: ${t.to?.name ?? t.name}`)
            },
          }))
        : [{ label: 'No transitions available', disabled: true, run: () => {} }],
    )

  const startAgent = () =>
    void askAgent({ projectId, newSession: true, name: issue.key, prompt: issuePrompt(issue) })

  const saveSummary = async () => {
    const v = editingSummary?.trim()
    if (!v || v === issue.summary) return setEditingSummary(null)
    if (await act('summary', () => jiraApi.update(projectId, key, { summary: v }), 'Summary updated')) setEditingSummary(null)
  }

  const saveDescription = async () => {
    if (editingDesc === null) return
    if (await act('description', () => jiraApi.update(projectId, key, { description: editingDesc }), 'Description updated')) setEditingDesc(null)
  }

  const saveLabels = async () => {
    if (labels === null) return
    const list = labels.split(/[\s,]+/).map((l) => l.trim()).filter(Boolean)
    if (await act('labels', () => jiraApi.update(projectId, key, { labels: list }), 'Labels updated')) setLabels(null)
  }

  const addComment = async () => {
    if (!comment.trim()) return
    if (await act('comment', () => jiraApi.comment(projectId, key, comment), 'Comment added')) setComment('')
  }

  const tone = statusTone(issue.status?.category)

  return (
    <div className="cf-page">
      <Toolbar>
        <IssueTypeIcon type={issue.issueType} />
        <span className="mono wb-small">{issue.key}</span>
        {issue.projectName && <span className="wb-small wb-muted wb-ellipsis">· {issue.projectName}</span>}
        {issue.parent && (
          <button className="wb-btn ghost small" onClick={() => openJiraIssue(issue.parent!.key, issue.parent!.summary ?? undefined)} title="Parent">
            ↑ {issue.parent.key}
          </button>
        )}
        {q.isFetching && <span className="wb-spinner" style={{ width: 11, height: 11 }} />}
        <Spacer />
        <Button size="small" icon={Bot} onClick={startAgent} title="Start a new agent session on this issue">
          Start agent
        </Button>
        <IconButton icon={ExternalLink} label="Open in Jira" onClick={() => window.open(issue.webUrl, '_blank', 'noopener,noreferrer')} />
        <IconButton icon={Copy} label="Copy link" onClick={() => void copyText(issue.webUrl)} />
        <IconButton icon={RefreshCw} label="Refresh" onClick={() => q.refetch()} />
      </Toolbar>
      {q.error && <StaleNotice what="the issue" error={q.error} onRetry={() => void q.refetch()} />}
      <div className="jira-issue">
        <div className="jira-main">
          {editingSummary !== null ? (
            <Input
              autoFocus
              value={editingSummary}
              onChange={(e) => setEditingSummary(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void saveSummary()
                if (e.key === 'Escape') setEditingSummary(null)
              }}
              onBlur={() => void saveSummary()}
              style={{ width: '100%', fontSize: 18, height: 36, marginBottom: 12 }}
            />
          ) : (
            <div className="jira-summary wb-row" style={{ alignItems: 'flex-start' }}>
              <span className="wb-grow">{issue.summary}</span>
              {issue.editable.summary && <IconButton size="small" icon={Pencil} label="Edit summary" onClick={() => setEditingSummary(issue.summary)} />}
            </div>
          )}
          <div className="wb-row" style={{ gap: 8, marginBottom: 8 }}>
            <button className={['jira-status-btn', tone].filter(Boolean).join(' ')} onClick={(e) => transitionMenu(e.currentTarget)} disabled={busy === 'transition'}>
              {issue.status?.name ?? 'Status'} <ChevronDown size={12} />
            </button>
            {issue.resolution && <Badge tone="success">{issue.resolution}</Badge>}
          </div>

          <div className="jira-section">
            Description
            <Spacer />
            {issue.editable.description && editingDesc === null && (
              <Button size="small" variant="ghost" icon={Pencil} onClick={() => setEditingDesc(issue.descriptionMarkdown)}>
                Edit
              </Button>
            )}
          </div>
          {editingDesc !== null ? (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {issue.descriptionLossy && (
                <div className="wb-row wb-small wb-warning">
                  <AlertTriangle size={14} /> The description has rich content (media, mentions, panels…) that markdown cannot keep; saving simplifies it.
                </div>
              )}
              <TextArea className="mono" rows={14} value={editingDesc} onChange={(e) => setEditingDesc(e.target.value)} autoFocus />
              <div className="wb-row">
                <span className="wb-xs wb-subtle">Markdown (converted for Jira)</span>
                <Spacer />
                <Button size="small" onClick={() => setEditingDesc(null)}>
                  Cancel
                </Button>
                <Button size="small" variant="primary" loading={busy === 'description'} onClick={() => void saveDescription()}>
                  Save
                </Button>
              </div>
            </div>
          ) : issue.descriptionHtml.trim() ? (
            <HtmlBlock html={issue.descriptionHtml} />
          ) : (
            <div className="wb-muted wb-small">No description.</div>
          )}

          <div className="jira-section">Comments ({issue.commentsTotal})</div>
          {issue.comments.map((c) => (
            <div key={c.id} className="jira-comment">
              <div className="cf-comment-head" style={{ marginBottom: 4 }}>
                <span className="atl-avatar">{initials(c.author?.displayName)}</span>
                <span className="who">{c.author?.displayName ?? 'Someone'}</span>
                <span className="wb-xs wb-subtle">
                  <TimeAgo time={c.created} />
                </span>
              </div>
              <HtmlBlock html={c.html} />
            </div>
          ))}
          {issue.commentsTotal > issue.comments.length && (
            <div className="wb-small wb-muted">
              Showing {issue.comments.length} of {issue.commentsTotal}.{' '}
              <a href={issue.webUrl} target="_blank" rel="noopener noreferrer">
                See all in Jira
              </a>
            </div>
          )}
          <div className="cf-compose" style={{ borderTop: 0, padding: '8px 0' }}>
            <TextArea
              rows={3}
              value={comment}
              placeholder="Add a comment (markdown)…"
              onChange={(e) => setComment(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && (e.ctrlKey || e.metaKey) && void addComment()}
            />
            <div className="wb-row">
              <span className="wb-xs wb-subtle">Ctrl+Enter</span>
              <Spacer />
              <Button size="small" variant="primary" loading={busy === 'comment'} disabled={!comment.trim()} onClick={() => void addComment()}>
                Comment
              </Button>
            </div>
          </div>
        </div>
        <Fields issue={issue} projectId={projectId} busy={busy} labels={labels} setLabels={setLabels} saveLabels={saveLabels} act={act} />
      </div>
    </div>
  )
}

function Fields({
  issue,
  projectId,
  busy,
  labels,
  setLabels,
  saveLabels,
  act,
}: {
  issue: Issue
  projectId: string | null
  busy: string | null
  labels: string | null
  setLabels: (v: string | null) => void
  saveLabels: () => Promise<void>
  act: (what: string, fn: () => Promise<unknown>, done: string) => Promise<boolean>
}) {
  const person = (u: { displayName: string } | null, empty = 'Unassigned') =>
    u ? (
      <span className="wb-row">
        <span className="atl-avatar">{initials(u.displayName)}</span>
        <span className="wb-ellipsis">{u.displayName}</span>
      </span>
    ) : (
      <span className="wb-muted">{empty}</span>
    )
  return (
    <div className="jira-fields">
      <div className="jira-field">
        <span className="k">Assignee</span>
        <span className="wb-row" style={{ minWidth: 0 }}>
          <span className="wb-grow">{person(issue.assignee)}</span>
          {issue.editable.assignee && (
            <IconButton
              size="small"
              icon={UserCheck}
              label="Assign to me"
              disabled={busy === 'assign'}
              onClick={() => void act('assign', () => jiraApi.assign(projectId, issue.key, 'me'), `${issue.key} assigned to you`)}
            />
          )}
        </span>
      </div>
      <div className="jira-field">
        <span className="k">Reporter</span>
        {person(issue.reporter, 'Unknown')}
      </div>
      <div className="jira-field">
        <span className="k">Priority</span>
        {issue.editable.priority && issue.editable.priorities.length ? (
          <Select
            value={issue.priority?.id ?? ''}
            onChange={(e) => void act('priority', () => jiraApi.update(projectId, issue.key, { priorityId: e.target.value }), 'Priority updated')}
            style={{ height: 24, fontSize: 'var(--fs-sm)' }}
          >
            {!issue.priority && <option value="">None</option>}
            {issue.editable.priorities.map((p) => (
              <option key={p.id ?? p.name} value={p.id ?? ''}>
                {p.name}
              </option>
            ))}
          </Select>
        ) : (
          <span>{issue.priority?.name ?? '—'}</span>
        )}
      </div>
      <div className="jira-field">
        <span className="k">Type</span>
        <span className="wb-row">
          <IssueTypeIcon type={issue.issueType} size={13} /> {issue.issueType?.name ?? '—'}
        </span>
      </div>
      <div className="jira-field" style={{ alignItems: 'flex-start', paddingTop: 5 }}>
        <span className="k">Labels</span>
        {labels !== null ? (
          <Input
            small
            autoFocus
            value={labels}
            placeholder="space or comma separated"
            onChange={(e) => setLabels(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void saveLabels()
              if (e.key === 'Escape') setLabels(null)
            }}
            onBlur={() => void saveLabels()}
          />
        ) : (
          <span
            className="wb-row"
            style={{ flexWrap: 'wrap', gap: 4, cursor: issue.editable.labels ? 'pointer' : undefined }}
            title={issue.editable.labels ? 'Edit labels' : undefined}
            onClick={() => issue.editable.labels && setLabels(issue.labels.join(' '))}
          >
            {issue.labels.length ? issue.labels.map((l) => <Badge key={l}>{l}</Badge>) : <span className="wb-muted">None</span>}
          </span>
        )}
      </div>
      <div className="jira-field">
        <span className="k">Created</span>
        <TimeAgo time={issue.created} />
      </div>
      <div className="jira-field">
        <span className="k">Updated</span>
        <TimeAgo time={issue.updated} />
      </div>
      {issue.due && (
        <div className="jira-field">
          <span className="k">Due</span>
          <span>{issue.due}</span>
        </div>
      )}
    </div>
  )
}
