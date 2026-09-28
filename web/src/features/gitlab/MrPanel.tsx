// 'mr' panel: a merge request — overview (description, merge widget,
// approvals), changes (tree + side-by-side diff), discussions, commits and
// pipelines. "Ask agent to review" hands the MR to the project's agent.

import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import {
  Bot,
  Check,
  ExternalLink,
  GitMerge,
  GitPullRequest,
  GitPullRequestClosed,
  GitPullRequestDraft,
  RefreshCw,
  RotateCcw,
  ThumbsUp,
  Undo2,
} from 'lucide-react'
import { openPanel, confirmDialog, toast, toastError } from '@/shell/actions'
import { askAgent } from '@/shell/agentBridge'
import type { PanelProps } from '@/shell/types'
import {
  Badge,
  Button,
  Checkbox,
  EmptyState,
  ErrorBox,
  Field,
  IconButton,
  Loading,
  Markdown,
  Modal,
  Spacer,
  Tabs,
  TextArea,
  TimeAgo,
} from '@/ui'
import { glApi, glk, useGitlabSummary, useMr, useMrCommits, useMrDiscussions, useMrPipelines } from './api'
import { Avatar, Duration, ExtLink, openPipeline, PipelineRow, RefLabel, StatusIcon, StatusText } from './components'
import { MrChanges } from './MrChanges'
import { MrDiscussions } from './MrDiscussions'
import { isActive, mergeStatusLabel, mrReviewPrompt, shortSha } from './logic'
import type { GitlabSummary, Mr } from './types'

export interface MrParams {
  projectId: string
  iid: number
}

type MrTab = 'overview' | 'changes' | 'discussions' | 'commits' | 'pipelines'

export function MrStateIcon({ mr, size = 15 }: { mr: Pick<Mr, 'state' | 'draft'>; size?: number }) {
  if (mr.state === 'merged') return <span className="gl-status gl-tone-accent" title="merged"><GitMerge size={size} /></span>
  if (mr.state === 'closed') return <span className="gl-status gl-tone-danger" title="closed"><GitPullRequestClosed size={size} /></span>
  if (mr.draft) return <span className="gl-status gl-tone-muted" title="draft"><GitPullRequestDraft size={size} /></span>
  return <span className="gl-status gl-tone-success" title="open"><GitPullRequest size={size} /></span>
}

function stateBadge(mr: Mr) {
  if (mr.state === 'merged') return <Badge tone="accent">Merged</Badge>
  if (mr.state === 'closed') return <Badge tone="danger">Closed</Badge>
  if (mr.draft) return <Badge>Draft</Badge>
  return <Badge tone="success">Open</Badge>
}

function MergeDialog({ projectId, mr, summary, onClose }: { projectId: string; mr: Mr; summary: GitlabSummary | undefined; onClose: () => void }) {
  const qc = useQueryClient()
  const squashOpt = summary?.squashOption ?? 'default_off'
  const running = isActive(mr.headPipeline?.status)
  const [squash, setSquash] = useState(squashOpt === 'always' || squashOpt === 'default_on' || mr.squash)
  const [remove, setRemove] = useState(mr.forceRemoveSourceBranch ?? summary?.removeSourceBranchAfterMerge ?? true)
  const [auto, setAuto] = useState(running)
  const [message, setMessage] = useState('')
  const [busy, setBusy] = useState(false)
  const merge = async () => {
    if (!mr.sha) return
    setBusy(true)
    try {
      const out = await glApi.merge(projectId, mr.iid, {
        sha: mr.sha,
        squash,
        removeSourceBranch: remove,
        autoMerge: auto,
        mergeCommitMessage: message.trim() || undefined,
      })
      qc.setQueryData(glk.mr(projectId, mr.iid), out)
      toast('success', auto ? `!${mr.iid} will merge when the pipeline succeeds` : `Merged !${mr.iid}`)
      onClose()
    } catch (e) {
      toastError(e, 'Merge failed')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title={`Merge !${mr.iid} into ${mr.targetBranch}`}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" icon={GitMerge} loading={busy} onClick={merge} disabled={!mr.sha}>
            {auto ? 'Set to auto-merge' : 'Merge'}
          </Button>
        </>
      }
    >
      <div className="wb-small wb-muted">
        Merges <span className="gl-mono">{shortSha(mr.sha)}</span> of <b>{mr.sourceBranch}</b>. GitLab refuses if the branch moved since you
        loaded it.
      </div>
      <Checkbox checked={squash} onChange={setSquash} disabled={squashOpt === 'always' || squashOpt === 'never'}>
        Squash commits{squashOpt === 'always' ? ' (required by the project)' : squashOpt === 'never' ? ' (disabled by the project)' : ''}
      </Checkbox>
      <Checkbox checked={remove} onChange={setRemove}>
        Delete source branch
      </Checkbox>
      <Checkbox checked={auto} onChange={setAuto}>
        Merge when the pipeline succeeds{running ? ' (a pipeline is running)' : ''}
      </Checkbox>
      <Field label={squash ? 'Squash commit message (optional)' : 'Merge commit message (optional)'}>
        <TextArea rows={3} value={message} onChange={(e) => setMessage(e.target.value)} placeholder="GitLab's default message" />
      </Field>
    </Modal>
  )
}

function MergeWidget({ projectId, mr, summary }: { projectId: string; mr: Mr; summary: GitlabSummary | undefined }) {
  const qc = useQueryClient()
  const [busy, setBusy] = useState<string | null>(null)
  const [merging, setMerging] = useState(false)
  const act = async (what: string, fn: () => Promise<Mr | unknown>, ok: string) => {
    setBusy(what)
    try {
      const out = await fn()
      if (out && typeof out === 'object' && 'iid' in out) qc.setQueryData(glk.mr(projectId, mr.iid), out)
      else qc.invalidateQueries({ queryKey: glk.mr(projectId, mr.iid) })
      toast('success', ok)
    } catch (e) {
      toastError(e)
    } finally {
      setBusy(null)
    }
  }
  const open = mr.state === 'opened'
  const a = mr.approvals
  const p = mr.headPipeline
  const status = mr.detailedMergeStatus
  const canMerge = open && !!mr.user?.canMerge
  const mergeable = status === 'mergeable' || ((status === 'ci_still_running' || status === 'ci_must_pass') && isActive(p?.status))
  const tone = status === 'mergeable' ? 'success' : status === 'conflict' || mr.hasConflicts ? 'danger' : 'warning'
  return (
    <div className="gl-card">
      <div className="row">
        {p ? (
          <>
            <StatusIcon status={p.status} size={16} />
            <span className="grow">
              <a className="gl-link" href="#" onClick={(e) => (e.preventDefault(), openPipeline(projectId, p.id, p.iid))}>
                Pipeline #{p.iid ?? p.id}
              </a>{' '}
              <StatusText status={p.status} />
            </span>
            <span className="wb-muted wb-small">
              <Duration p={p} />
            </span>
          </>
        ) : (
          <span className="wb-muted">No pipeline for the head commit</span>
        )}
      </div>
      <div className="row">
        <ThumbsUp size={15} className={a?.approved ? 'wb-success' : 'wb-muted'} />
        <span className="grow">
          {!a
            ? 'Approvals unavailable'
            : a.approvedBy.length
              ? `Approved by ${a.approvedBy.map((x) => x.user.username).join(', ')}`
              : a.approvalsRequired
                ? `Requires ${a.approvalsLeft} more approval${a.approvalsLeft === 1 ? '' : 's'}`
                : 'Approval is optional'}
        </span>
        {open && a?.userCanApprove && !a.userHasApproved && (
          <Button size="small" icon={Check} loading={busy === 'approve'} onClick={() => act('approve', () => glApi.approve(projectId, mr.iid, mr.sha), 'Approved')}>
            Approve
          </Button>
        )}
        {open && a?.userHasApproved && (
          <Button size="small" icon={Undo2} loading={busy === 'unapprove'} onClick={() => act('unapprove', () => glApi.unapprove(projectId, mr.iid), 'Approval revoked')}>
            Revoke
          </Button>
        )}
      </div>
      {open ? (
        <div className="row">
          <StatusIcon status={tone === 'success' ? 'success' : tone === 'danger' ? 'failed' : 'pending'} size={16} />
          <span className="grow">
            {mergeStatusLabel(status)}
            {mr.divergedCommitsCount ? <span className="wb-muted"> · {mr.divergedCommitsCount} behind {mr.targetBranch}</span> : null}
            {mr.mergeError && <div className="wb-small wb-danger">{mr.mergeError}</div>}
            {mr.mergeWhenPipelineSucceeds && <div className="wb-small wb-muted">Set to merge when the pipeline succeeds</div>}
          </span>
          {(status === 'need_rebase' || (mr.divergedCommitsCount ?? 0) > 0) && canMerge && (
            <Button
              size="small"
              icon={RotateCcw}
              loading={busy === 'rebase' || !!mr.rebaseInProgress}
              onClick={() => act('rebase', () => glApi.rebase(projectId, mr.iid), 'Rebase started')}
            >
              Rebase
            </Button>
          )}
        </div>
      ) : (
        <div className="row">
          <MrStateIcon mr={mr} size={16} />
          <span className="grow">
            {mr.state === 'merged' ? (
              <>
                Merged{mr.mergeUser ? ` by ${mr.mergeUser.username}` : ''} <TimeAgo time={mr.mergedAt} />
                {(mr.mergeCommitSha || mr.squashCommitSha) && <span className="gl-mono"> · {shortSha(mr.mergeCommitSha ?? mr.squashCommitSha)}</span>}
              </>
            ) : (
              <>
                Closed <TimeAgo time={mr.closedAt} />
              </>
            )}
          </span>
        </div>
      )}
      {open && (
        <div className="actions">
          {canMerge && (
            <Button variant="primary" size="small" icon={GitMerge} disabled={!mergeable || !mr.sha} onClick={() => setMerging(true)}>
              {isActive(p?.status) ? 'Merge…' : 'Merge'}
            </Button>
          )}
          <Button
            size="small"
            loading={busy === 'draft'}
            onClick={() => act('draft', () => glApi.updateMr(projectId, mr.iid, { draft: !mr.draft }), mr.draft ? 'Marked as ready' : 'Marked as draft')}
          >
            {mr.draft ? 'Mark as ready' : 'Mark as draft'}
          </Button>
          <Spacer />
          <Button
            size="small"
            variant="ghost"
            loading={busy === 'close'}
            onClick={async () => {
              if (await confirmDialog({ title: `Close !${mr.iid}?`, message: mr.title, confirmLabel: 'Close merge request', danger: true }))
                void act('close', () => glApi.updateMr(projectId, mr.iid, { stateEvent: 'close' }), 'Closed')
            }}
          >
            Close
          </Button>
        </div>
      )}
      {mr.state === 'closed' && (
        <div className="actions">
          <Button size="small" loading={busy === 'reopen'} onClick={() => act('reopen', () => glApi.updateMr(projectId, mr.iid, { stateEvent: 'reopen' }), 'Reopened')}>
            Reopen
          </Button>
        </div>
      )}
      {merging && <MergeDialog projectId={projectId} mr={mr} summary={summary} onClose={() => setMerging(false)} />}
    </div>
  )
}

function Overview({ projectId, mr, summary }: { projectId: string; mr: Mr; summary: GitlabSummary | undefined }) {
  const web = summary?.webUrl
  return (
    <div className="gl-cq wb-scroll">
      <div className="gl-overview">
        <div className="desc">
          {mr.description?.trim() ? (
            <Markdown
              text={mr.description}
              resolveImage={(src) => (src.startsWith('/uploads/') && web ? `${web}${src}` : src)}
            />
          ) : (
            <div className="wb-muted">No description.</div>
          )}
        </div>
        <div className="side">
          <MergeWidget projectId={projectId} mr={mr} summary={summary} />
          <div className="gl-card">
            <div className="gl-kv gl-card-body">
              <span className="k">Author</span>
              <span className="wb-row">
                <Avatar user={mr.author} small /> {mr.author?.name ?? '—'}
              </span>
              <span className="k">Assignees</span>
              <span>{mr.assignees.map((u) => u.username).join(', ') || '—'}</span>
              <span className="k">Reviewers</span>
              <span>{mr.reviewers.map((u) => u.username).join(', ') || '—'}</span>
              <span className="k">Labels</span>
              <span className="wb-row" style={{ flexWrap: 'wrap' }}>
                {mr.labels.length ? mr.labels.map((l) => <span className="gl-label" key={l}>{l}</span>) : '—'}
              </span>
              <span className="k">Created</span>
              <TimeAgo time={mr.createdAt} />
              <span className="k">Updated</span>
              <TimeAgo time={mr.updatedAt} />
              <span className="k">Head</span>
              <span className="gl-mono">{shortSha(mr.sha)}</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

function Commits({ projectId, iid }: { projectId: string; iid: number }) {
  const q = useMrCommits(projectId, iid)
  if (q.error) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!q.data) return <Loading />
  if (!q.data.length) return <EmptyState title="No commits" />
  return (
    <div className="gl-list">
      {q.data.map((c) => (
        <div
          key={c.id}
          className="gl-row"
          onClick={() => openPanel({ kind: 'commit', id: `commit:${projectId}:${c.id}`, title: c.shortId, params: { projectId, sha: c.id } })}
          title="Open the commit (needs the commit in the local clone)"
        >
          <span className="gl-mono wb-muted">●</span>
          <span className="title">{c.title}</span>
          <span className="right">
            <ExtLink href={c.webUrl} />
          </span>
          <span className="meta">
            <span className="gl-mono">{c.shortId}</span>
            <span>{c.authorName}</span>
          </span>
          <span className="right">
            <TimeAgo time={c.committedDate ?? c.authoredDate} />
          </span>
        </div>
      ))}
    </div>
  )
}

function Pipelines({ projectId, iid }: { projectId: string; iid: number }) {
  const q = useMrPipelines(projectId, iid)
  if (q.error) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!q.data) return <Loading />
  if (!q.data.length) return <EmptyState title="No pipelines for this merge request" />
  return (
    <div className="gl-list">
      {q.data.map((p) => (
        <PipelineRow key={p.id} p={p} onOpen={() => openPipeline(projectId, p.id, p.iid)} />
      ))}
    </div>
  )
}

export function MrPanel({ params, setTitle, visible }: PanelProps<MrParams>) {
  const { projectId, iid } = params
  const qc = useQueryClient()
  const q = useMr(projectId, iid)
  const summary = useGitlabSummary(projectId)
  const [tab, setTab] = useState<MrTab>('overview')
  const [focusPath, setFocusPath] = useState<string | null>(null)
  const discussions = useMrDiscussions(projectId, iid)
  const mr = q.data

  useEffect(() => {
    if (mr) setTitle(`!${mr.iid} ${mr.title.length > 36 ? mr.title.slice(0, 35) + '…' : mr.title}`)
  }, [mr?.iid, mr?.title, setTitle]) // eslint-disable-line react-hooks/exhaustive-deps

  if (q.error && !mr) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  if (!mr) return <Loading />

  const threads = (discussions.data ?? []).filter((d) => d.notes.some((n) => !n.system))
  const unresolved = threads.filter((d) => d.notes.some((n) => n.resolvable && !n.resolved)).length
  const review = () =>
    askAgent({
      projectId,
      prompt: mrReviewPrompt({
        projectPath: summary.data?.path ?? projectId,
        iid: mr.iid,
        title: mr.title,
        sourceBranch: mr.sourceBranch,
        targetBranch: mr.targetBranch,
        sha: mr.sha,
      }),
    })

  return (
    <div className="wb-fill">
      <div className="gl-mr-head">
        <div className="wb-row" style={{ alignItems: 'flex-start', gap: 8 }}>
          <span style={{ paddingTop: 2 }}>
            <MrStateIcon mr={mr} size={18} />
          </span>
          <h2 className="wb-grow">
            {mr.title} <span className="wb-muted" style={{ fontWeight: 400 }}>!{mr.iid}</span>
          </h2>
          <Button size="small" icon={Bot} onClick={review}>
            Ask agent to review
          </Button>
          <IconButton
            icon={RefreshCw}
            size="small"
            label="Refresh"
            onClick={() => qc.invalidateQueries({ queryKey: glk.mr(projectId, iid) })}
          />
          <ExtLink href={mr.webUrl}>
            <span className="wb-icon-btn small">
              <ExternalLink size={14} />
            </span>
          </ExtLink>
        </div>
        <div className="line">
          {stateBadge(mr)}
          <Avatar user={mr.author} small />
          <span>{mr.author?.username}</span>
          <span className="gl-sep">wants to merge</span>
          <RefLabel name={mr.sourceBranch} />
          <span className="gl-sep">into</span>
          <RefLabel name={mr.targetBranch} />
          <span className="gl-sep">·</span>
          <TimeAgo time={mr.createdAt} />
          {mr.hasConflicts && <Badge tone="danger">Conflicts</Badge>}
        </div>
      </div>
      <Tabs<MrTab>
        tabs={[
          { id: 'overview', label: 'Overview' },
          { id: 'changes', label: 'Changes', badge: mr.changesCount ? <Badge>{mr.changesCount}</Badge> : undefined },
          {
            id: 'discussions',
            label: 'Discussions',
            badge: threads.length ? <Badge tone={unresolved ? 'warning' : undefined}>{unresolved ? `${unresolved}/${threads.length}` : threads.length}</Badge> : undefined,
          },
          { id: 'commits', label: 'Commits' },
          { id: 'pipelines', label: 'Pipelines' },
        ]}
        value={tab}
        onChange={setTab}
      />
      {tab === 'overview' && <Overview projectId={projectId} mr={mr} summary={summary.data} />}
      {tab === 'changes' && (
        <MrChanges projectId={projectId} mr={mr} discussions={discussions.data ?? []} focusPath={focusPath} visible={visible} />
      )}
      {tab === 'discussions' && (
        <MrDiscussions
          projectId={projectId}
          mr={mr}
          query={discussions}
          onOpenFile={(path) => {
            setFocusPath(path)
            setTab('changes')
          }}
        />
      )}
      {tab === 'commits' && <Commits projectId={projectId} iid={iid} />}
      {tab === 'pipelines' && <Pipelines projectId={projectId} iid={iid} />}
    </div>
  )
}
