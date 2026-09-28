// 'pr' panel: a pull request — overview (description, checks, reviews, merge
// box with the repository's merge methods), files changed (tree + Monaco diff
// with inline review threads), conversation and commits. "Ask agent to review"
// hands the pull request to the project's agent.

import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import {
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  ExternalLink,
  GitMerge,
  MessageSquareWarning,
  RefreshCw,
} from 'lucide-react'
import { confirmDialog, openPanel, toast, toastError } from '@/shell/actions'
import { askAgent } from '@/shell/agentBridge'
import type { PanelProps } from '@/shell/types'
import { Badge, Button, Checkbox, EmptyState, ErrorBox, Field, IconButton, Input, Loading, Markdown, Modal, Spacer, Tabs, TextArea, TimeAgo } from '@/ui'
import { ghApi, ghk, useGithubSummary, usePrCommits, usePull, useThreads } from './api'
import { Avatar, ExtLink, openJob, openRun, RefLabel, StatusIcon } from './components'
import { PrStateIcon } from './icons'
import { PrConversation } from './PrConversation'
import { PrFilesView } from './PrFiles'
import { aggregateState, canMergeNow, eventLabel, ghLabel, mergeBox, prReviewPrompt, prState, shortSha, stateLabel } from './logic'
import type { CheckItem, GithubSummary, Pull, PullDetail } from './types'

export interface PrParams {
  projectId: string
  number: number
}

type PrTab = 'overview' | 'files' | 'conversation' | 'commits'

const NEEDS_TOKEN = 'Needs a GitHub token'

function stateBadge(p: Pull) {
  const s = prState(p)
  if (s === 'merged') return <Badge tone="accent">Merged</Badge>
  if (s === 'closed') return <Badge tone="danger">Closed</Badge>
  if (s === 'draft') return <Badge>Draft</Badge>
  return <Badge tone="success">Open</Badge>
}

// ---------------------------------------------------------------- checks

function CheckRow({ projectId, c, showEvent }: { projectId: string; c: CheckItem; showEvent: boolean }) {
  const inApp = c.jobId !== null || c.runId !== null
  const open = () => (c.jobId !== null ? openJob(projectId, c.jobId, c.name) : c.runId !== null ? openRun(projectId, c.runId, c.name) : undefined)
  const name = c.workflow && c.workflow !== c.name ? `${c.workflow} / ${c.name}` : c.name
  return (
    <div className="gh-check">
      <StatusIcon state={c.state} size={14} title={c.kind === 'status' ? c.status : ghLabel(c.status, c.conclusion)} />
      <span className={inApp ? 'n clickable' : 'n'} onClick={inApp ? open : undefined} title={c.description ?? name}>
        {name}
        {showEvent && c.event && <span className="wb-muted"> ({eventLabel(c.event)})</span>}
        {c.description && <span className="wb-muted"> — {c.description}</span>}
      </span>
      <span className="r">
        {c.app && c.app !== 'GitHub Actions' && <span>{c.app}</span>}
        <ExtLink href={c.url} />
      </span>
    </div>
  )
}

/** Failures first, then what is still going, then the rest. */
const CHECK_RANK = ['failed', 'running', 'pending', 'manual', 'canceled']
const checkRank = (s: string) => {
  const i = CHECK_RANK.indexOf(s)
  return i < 0 ? CHECK_RANK.length : i
}

function ChecksBlock({ projectId, d }: { projectId: string; d: PullDetail }) {
  const items = d.checks?.items ?? []
  const failing = items.filter((i) => i.state === 'failed').length
  const pending = items.filter((i) => i.state === 'running' || i.state === 'pending').length
  const [open, setOpen] = useState(failing > 0 || pending > 0)
  const state = d.checks?.state ?? null
  // The same job under two triggers (push and pull_request) needs telling apart.
  const manyEvents = new Set(items.map((i) => i.event).filter(Boolean)).size > 1
  const summary = !items.length
    ? 'No checks for the head commit'
    : failing
      ? `${failing} failing${pending ? `, ${pending} in progress` : ''} of ${items.length} checks`
      : pending
        ? `${pending} of ${items.length} checks in progress`
        : `All ${items.length} checks ${stateLabel(state)}`
  return (
    <>
      <div className="row" style={{ cursor: items.length ? 'pointer' : undefined }} onClick={() => items.length && setOpen(!open)}>
        <StatusIcon state={state} size={16} />
        <span className="grow">{summary}</span>
        {items.length > 0 && (open ? <ChevronDown size={14} /> : <ChevronRight size={14} />)}
      </div>
      {open && items.length > 0 && (
        <div className="checks">
          {[...items]
            .sort((a, b) => checkRank(a.state) - checkRank(b.state))
            .map((c, i) => (
              <CheckRow key={`${c.kind}:${c.name}:${i}`} projectId={projectId} c={c} showEvent={manyEvents} />
            ))}
        </div>
      )}
    </>
  )
}

// ---------------------------------------------------------------- dialogs

function MergeDialog({ projectId, pr, summary, onClose }: { projectId: string; pr: PullDetail; summary: GithubSummary | undefined; onClose: () => void }) {
  const qc = useQueryClient()
  const allowed = summary?.mergeMethods ?? { merge: true, squash: true, rebase: true }
  const methods = (['merge', 'squash', 'rebase'] as const).filter((m) => allowed[m])
  const [method, setMethod] = useState<'merge' | 'squash' | 'rebase'>(methods[0] ?? 'merge')
  const [title, setTitle] = useState('')
  const [message, setMessage] = useState('')
  const [del, setDel] = useState(summary?.deleteBranchOnMerge ?? true)
  const [busy, setBusy] = useState(false)
  const sameRepo = !!pr.head?.repo && pr.head.repo.id === pr.base?.repo?.id
  const label = { merge: 'Create a merge commit', squash: 'Squash and merge', rebase: 'Rebase and merge' }
  const merge = async () => {
    if (!pr.head?.sha) return
    setBusy(true)
    try {
      const out = await ghApi.merge(projectId, pr.number, {
        sha: pr.head.sha,
        method,
        title: method !== 'rebase' ? title.trim() || undefined : undefined,
        message: method !== 'rebase' ? message.trim() || undefined : undefined,
        deleteBranch: sameRepo && !summary?.deleteBranchOnMerge && del,
      })
      qc.setQueryData(ghk.pull(projectId, pr.number), out)
      toast('success', `Merged #${pr.number}${out.warnings.length ? ` (${out.warnings.join('; ')})` : ''}`)
      onClose()
    } catch (e) {
      toastError(e, 'Merge failed')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title={`Merge #${pr.number} into ${pr.base?.ref ?? '?'}`}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" icon={GitMerge} loading={busy} onClick={merge} disabled={!pr.head?.sha || !methods.length}>
            {label[method]}
          </Button>
        </>
      }
    >
      <div className="wb-small wb-muted">
        Merges <span className="gh-mono">{shortSha(pr.head?.sha)}</span> of <b>{pr.head?.ref}</b>. GitHub refuses if the branch moved since you loaded it.
      </div>
      <Field label="Method">
        <div className="gh-methods">
          {methods.map((m) => (
            <label className="wb-checkbox" key={m}>
              <input type="radio" name="gh-merge-method" checked={method === m} onChange={() => setMethod(m)} />
              {label[m]}
            </label>
          ))}
          {!methods.length && <span className="wb-warning wb-small">This repository allows no merge method.</span>}
        </div>
      </Field>
      {method !== 'rebase' && (
        <>
          <Field label="Commit title (optional)">
            <Input value={title} onChange={(e) => setTitle(e.target.value)} placeholder={method === 'squash' ? `${pr.title} (#${pr.number})` : `Merge pull request #${pr.number}`} />
          </Field>
          <Field label="Commit message (optional)">
            <TextArea rows={3} value={message} onChange={(e) => setMessage(e.target.value)} placeholder="GitHub's default message" />
          </Field>
        </>
      )}
      {sameRepo && !summary?.deleteBranchOnMerge && (
        <Checkbox checked={del} onChange={setDel}>
          Delete {pr.head?.ref} afterwards
        </Checkbox>
      )}
      {summary?.deleteBranchOnMerge && <div className="wb-small wb-muted">The repository deletes merged branches automatically.</div>}
    </Modal>
  )
}

function ReviewDialog({ projectId, pr, event, onClose }: { projectId: string; pr: PullDetail; event: 'APPROVE' | 'REQUEST_CHANGES'; onClose: () => void }) {
  const qc = useQueryClient()
  const [body, setBody] = useState('')
  const [busy, setBusy] = useState(false)
  const approve = event === 'APPROVE'
  const submit = async () => {
    setBusy(true)
    try {
      await ghApi.review(projectId, pr.number, { event, body: body.trim() || undefined, sha: pr.head?.sha })
      toast('success', approve ? `Approved #${pr.number}` : `Requested changes on #${pr.number}`)
      void qc.invalidateQueries({ queryKey: ghk.pull(projectId, pr.number) })
      onClose()
    } catch (e) {
      toastError(e, 'Could not submit the review')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title={approve ? `Approve #${pr.number}` : `Request changes on #${pr.number}`}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" icon={approve ? Check : MessageSquareWarning} loading={busy} disabled={!approve && !body.trim()} onClick={submit}>
            {approve ? 'Approve' : 'Request changes'}
          </Button>
        </>
      }
    >
      <div className="wb-small wb-muted">
        Reviews commit <span className="gh-mono">{shortSha(pr.head?.sha)}</span>.
      </div>
      <Field label={approve ? 'Comment (optional)' : 'What should change'} hint="Markdown">
        <TextArea
          rows={5}
          value={body}
          onChange={(e) => setBody(e.target.value)}
          autoFocus
          onKeyDown={(e) => e.key === 'Enter' && (e.ctrlKey || e.metaKey) && (approve || body.trim()) && void submit()}
        />
      </Field>
    </Modal>
  )
}

// ---------------------------------------------------------------- merge box

function MergeBoxCard({ projectId, pr, summary }: { projectId: string; pr: PullDetail; summary: GithubSummary | undefined }) {
  const qc = useQueryClient()
  const [busy, setBusy] = useState<string | null>(null)
  const [dialog, setDialog] = useState<'merge' | 'APPROVE' | 'REQUEST_CHANGES' | null>(null)
  const anon = summary ? !summary.auth.authenticated : false
  const tokenTitle = anon ? NEEDS_TOKEN : undefined
  const own = !!summary?.auth.viewer && summary.auth.viewer === pr.user?.login
  const open = pr.state === 'open' && !pr.merged
  const box = mergeBox(pr)
  const act = async (what: string, fn: () => Promise<PullDetail>, ok: string) => {
    setBusy(what)
    try {
      qc.setQueryData(ghk.pull(projectId, pr.number), await fn())
      toast('success', ok)
    } catch (e) {
      toastError(e)
    } finally {
      setBusy(null)
    }
  }
  const approved = pr.reviewStates.filter((r) => r.state === 'APPROVED')
  const changes = pr.reviewStates.filter((r) => r.state === 'CHANGES_REQUESTED')
  const reviewTone = changes.length ? 'failed' : approved.length ? 'success' : null
  return (
    <div className="gh-card">
      <ChecksBlock projectId={projectId} d={pr} />
      <div className="row">
        <StatusIcon state={reviewTone} size={16} title="Reviews" />
        <span className="grow">
          {changes.length ? `Changes requested by ${changes.map((r) => r.user?.login).join(', ')}` : ''}
          {changes.length && approved.length ? ' · ' : ''}
          {approved.length ? `Approved by ${approved.map((r) => r.user?.login).join(', ')}` : ''}
          {!changes.length && !approved.length && (pr.requestedReviewers.length ? `Review requested from ${pr.requestedReviewers.map((u) => u.login).join(', ')}` : 'No reviews yet')}
        </span>
      </div>
      <div className="row">
        <StatusIcon state={box.tone === 'success' ? 'success' : box.tone === 'danger' ? 'failed' : box.tone === 'accent' ? 'success' : box.tone === 'warning' ? 'pending' : null} size={16} />
        <span className="grow">
          {pr.merged ? (
            <>
              Merged{pr.mergedBy ? ` by ${pr.mergedBy.login}` : ''} <TimeAgo time={pr.mergedAt} />
              {pr.mergeCommitSha && <span className="gh-mono"> · {shortSha(pr.mergeCommitSha)}</span>}
            </>
          ) : pr.state === 'closed' ? (
            <>
              Closed <TimeAgo time={pr.closedAt} />
            </>
          ) : (
            box.label
          )}
        </span>
      </div>
      {open && (
        <div className="actions">
          {(summary?.canPush ?? true) && (
            <Button
              variant="primary"
              size="small"
              icon={GitMerge}
              disabled={anon || !canMergeNow(pr) || !pr.head?.sha}
              title={tokenTitle ?? (canMergeNow(pr) ? undefined : box.label)}
              onClick={() => setDialog('merge')}
            >
              Merge…
            </Button>
          )}
          {!own && (
            <>
              <Button size="small" icon={Check} disabled={anon} title={tokenTitle} onClick={() => setDialog('APPROVE')}>
                Approve
              </Button>
              <Button size="small" icon={MessageSquareWarning} disabled={anon} title={tokenTitle} onClick={() => setDialog('REQUEST_CHANGES')}>
                Request changes
              </Button>
            </>
          )}
          <Button
            size="small"
            loading={busy === 'draft'}
            disabled={anon}
            title={tokenTitle}
            onClick={() => act('draft', () => ghApi.updatePr(projectId, pr.number, { draft: !pr.draft }), pr.draft ? 'Marked as ready for review' : 'Converted to draft')}
          >
            {pr.draft ? 'Ready for review' : 'Convert to draft'}
          </Button>
          <Spacer />
          <Button
            size="small"
            variant="ghost"
            loading={busy === 'close'}
            disabled={anon}
            title={tokenTitle}
            onClick={async () => {
              if (await confirmDialog({ title: `Close #${pr.number}?`, message: pr.title, confirmLabel: 'Close pull request', danger: true }))
                void act('close', () => ghApi.updatePr(projectId, pr.number, { state: 'closed' }), 'Closed')
            }}
          >
            Close
          </Button>
        </div>
      )}
      {pr.state === 'closed' && !pr.merged && (
        <div className="actions">
          <Button size="small" loading={busy === 'reopen'} disabled={anon} title={tokenTitle} onClick={() => act('reopen', () => ghApi.updatePr(projectId, pr.number, { state: 'open' }), 'Reopened')}>
            Reopen
          </Button>
        </div>
      )}
      {dialog === 'merge' && <MergeDialog projectId={projectId} pr={pr} summary={summary} onClose={() => setDialog(null)} />}
      {(dialog === 'APPROVE' || dialog === 'REQUEST_CHANGES') && <ReviewDialog projectId={projectId} pr={pr} event={dialog} onClose={() => setDialog(null)} />}
    </div>
  )
}

function Overview({ projectId, pr, summary }: { projectId: string; pr: PullDetail; summary: GithubSummary | undefined }) {
  const web = summary?.webUrl
  return (
    <div className="gh-cq wb-scroll">
      <div className="gh-overview">
        <div className="desc">
          {pr.body?.trim() ? (
            <Markdown text={pr.body} resolveImage={(src) => (src.startsWith('/') && web ? `${new URL(web).origin}${src}` : src)} />
          ) : (
            <div className="wb-muted">No description.</div>
          )}
        </div>
        <div className="side">
          <MergeBoxCard projectId={projectId} pr={pr} summary={summary} />
          {pr.warnings.length > 0 && <div className="wb-small wb-warning">{pr.warnings.join(' · ')}</div>}
          <div className="gh-card">
            <div className="gh-kv gh-card-body">
              <span className="k">Author</span>
              <span className="wb-row">
                <Avatar user={pr.user} small /> {pr.user?.login ?? '—'}
              </span>
              <span className="k">Assignees</span>
              <span>{pr.assignees.map((u) => u.login).join(', ') || '—'}</span>
              <span className="k">Reviewers</span>
              <span>{[...new Set([...pr.reviewStates.map((r) => r.user?.login), ...pr.requestedReviewers.map((u) => u.login)].filter(Boolean))].join(', ') || '—'}</span>
              <span className="k">Labels</span>
              <span className="wb-row" style={{ flexWrap: 'wrap' }}>
                {pr.labels.length ? pr.labels.map((l) => <span className="gh-label" key={l.id || l.name}>{l.name}</span>) : '—'}
              </span>
              {pr.milestone && (
                <>
                  <span className="k">Milestone</span>
                  <span>{pr.milestone.title}</span>
                </>
              )}
              <span className="k">Changes</span>
              <span>
                {pr.changedFiles ?? 0} files <span className="gh-add">+{pr.additions ?? 0}</span> <span className="gh-del">−{pr.deletions ?? 0}</span> · {pr.commits ?? 0} commits
              </span>
              <span className="k">Created</span>
              <TimeAgo time={pr.createdAt} />
              <span className="k">Updated</span>
              <TimeAgo time={pr.updatedAt} />
              <span className="k">Head</span>
              <span className="gh-mono">{shortSha(pr.head?.sha)}</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

function Commits({ projectId, n }: { projectId: string; n: number }) {
  const q = usePrCommits(projectId, n)
  if (q.error) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!q.data) return <Loading />
  if (!q.data.length) return <EmptyState title="No commits" />
  return (
    <div className="gh-list">
      {q.data.map((c) => (
        <div
          key={c.sha}
          className="gh-row"
          onClick={() => openPanel({ kind: 'commit', id: `commit:${projectId}:${c.sha}`, title: c.shortSha, params: { projectId, sha: c.sha } })}
          title="Open the commit (needs the commit in the local clone)"
        >
          <span className="gh-mono wb-muted">●</span>
          <span className="title">{c.title}</span>
          <span className="right">
            <ExtLink href={c.htmlUrl} />
          </span>
          <span className="meta">
            <span className="gh-mono">{c.shortSha}</span>
            <span>{c.authorLogin ?? c.authorName}</span>
          </span>
          <span className="right">
            <TimeAgo time={c.date} />
          </span>
        </div>
      ))}
    </div>
  )
}

export function PrPanel({ params, setTitle }: PanelProps<PrParams>) {
  return <PrView projectId={params.projectId} number={params.number} setTitle={setTitle} />
}

/**
 * A pull request: header, Overview, Files changed, Conversation, Commits.
 * `compact` (the phone tab) leaves out the side-by-side diff, which needs a
 * wide screen; review threads there link to the files on GitHub instead.
 */
export function PrView({ projectId, number, setTitle, compact }: { projectId: string; number: number; setTitle?: (title: string) => void; compact?: boolean }) {
  const qc = useQueryClient()
  const summary = useGithubSummary(projectId)
  const anonymous = summary.data ? !summary.data.auth.authenticated : false
  const q = usePull(projectId, number, anonymous)
  const threads = useThreads(projectId, number)
  const [tab, setTab] = useState<PrTab>('overview')
  const [focusPath, setFocusPath] = useState<string | null>(null)
  const pr = q.data

  useEffect(() => {
    if (pr) setTitle?.(`#${pr.number} ${pr.title.length > 36 ? pr.title.slice(0, 35) + '…' : pr.title}`)
  }, [pr?.number, pr?.title, setTitle]) // eslint-disable-line react-hooks/exhaustive-deps

  if (q.error && !pr) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!pr) return <Loading />

  const unresolved = (threads.data ?? []).filter((t) => t.resolved === false).length
  const threadCount = threads.data?.length ?? 0
  const convo = (pr.comments ?? 0) + threadCount
  const review = () =>
    askAgent({
      projectId,
      prompt: prReviewPrompt({ repo: summary.data?.path ?? projectId, number: pr.number, title: pr.title, head: pr.head?.ref ?? null, base: pr.base?.ref ?? null, sha: pr.head?.sha ?? null }),
    })
  const checksState = aggregateState([pr.checks?.state])

  return (
    <div className="wb-fill">
      <div className="gh-pr-head">
        <div className="wb-row" style={{ alignItems: 'flex-start', gap: 8 }}>
          <span style={{ paddingTop: 2 }}>
            <PrStateIcon pr={pr} size={18} />
          </span>
          <h2 className="wb-grow">
            {pr.title} <span className="wb-muted" style={{ fontWeight: 400 }}>#{pr.number}</span>
          </h2>
          {compact ? (
            <IconButton icon={Bot} size="small" label="Ask agent to review" onClick={review} />
          ) : (
            <Button size="small" icon={Bot} onClick={review}>
              Ask agent to review
            </Button>
          )}
          <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={() => qc.invalidateQueries({ queryKey: ['github', projectId, 'pull', number] })} />
          <ExtLink href={pr.htmlUrl}>
            <span className="wb-icon-btn small">
              <ExternalLink size={14} />
            </span>
          </ExtLink>
        </div>
        <div className="line">
          {stateBadge(pr)}
          <Avatar user={pr.user} small />
          <span>{pr.user?.login}</span>
          <span className="gh-sep">{pr.merged ? 'merged' : 'wants to merge'}</span>
          {pr.head && <RefLabel name={pr.head.repo && pr.base?.repo && pr.head.repo.id !== pr.base.repo.id ? pr.head.label : pr.head.ref} />}
          <span className="gh-sep">into</span>
          {pr.base && <RefLabel name={pr.base.ref} />}
          <span className="gh-sep">·</span>
          <TimeAgo time={pr.createdAt} />
          {checksState && (
            <>
              <span className="gh-sep">·</span>
              <StatusIcon state={checksState} size={13} />
              <span>checks {stateLabel(checksState)}</span>
            </>
          )}
          {pr.mergeableState === 'dirty' && <Badge tone="danger">Conflicts</Badge>}
        </div>
      </div>
      <Tabs<PrTab>
        tabs={[
          { id: 'overview', label: 'Overview' },
          ...(compact ? [] : [{ id: 'files' as const, label: 'Files changed', badge: pr.changedFiles ? <Badge>{pr.changedFiles}</Badge> : undefined }]),
          {
            id: 'conversation',
            label: 'Conversation',
            badge: convo ? <Badge tone={unresolved ? 'warning' : undefined}>{unresolved ? `${unresolved}/${convo}` : convo}</Badge> : undefined,
          },
          { id: 'commits', label: 'Commits', badge: pr.commits ? <Badge>{pr.commits}</Badge> : undefined },
        ]}
        value={tab}
        onChange={setTab}
      />
      {tab === 'overview' && <Overview projectId={projectId} pr={pr} summary={summary.data} />}
      {tab === 'files' && <PrFilesView projectId={projectId} pr={pr} threads={threads.data ?? []} focusPath={focusPath} anonymous={anonymous} />}
      {tab === 'conversation' && (
        <PrConversation
          projectId={projectId}
          pr={pr}
          threads={threads}
          anonymous={anonymous}
          onOpenFile={(path) => {
            if (compact) {
              window.open(`${pr.htmlUrl}/files`, '_blank', 'noopener,noreferrer')
              return
            }
            setFocusPath(path)
            setTab('files')
          }}
        />
      )}
      {tab === 'commits' && <Commits projectId={projectId} n={number} />}
    </div>
  )
}
