// The lists in the GitHub tool window (and the phone's GitHub tab): workflow
// runs, pull requests, issues and releases.

import { useMemo, useState, type MouseEvent, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { CircleDot, Copy, Download, ExternalLink, GitPullRequest, MessageSquare, Package, Play, Plus, RefreshCw, RotateCw, Square, Tag } from 'lucide-react'
import { promptDialog, toast, toastError } from '@/shell/actions'
import { Badge, Button, EmptyState, ErrorBox, formatBytes, IconButton, Input, Loading, Select, showMenu, TimeAgo } from '@/ui'
import { ghApi, ghk, useIssues, usePulls, useReleases, useRuns, useWorkflows, type IssueFilters, type PullFilters } from './api'
import { copyText, ExtLink, openIssue, openPr, openRun, RefLabel, RunRow, runTitle, useGhUi } from './components'
import { IssueStateIcon, PrStateIcon } from './icons'
import { isActive, mergeRuns, rowKeys } from './logic'
import type { GithubSummary, Run } from './types'

const TOKEN_HINT = 'needs a GitHub token'

function More({ has, loading, onMore }: { has: boolean; loading: boolean; onMore: () => void }) {
  if (!has) return null
  return (
    <div className="gh-more">
      <Button size="small" loading={loading} onClick={onMore}>
        Load more
      </Button>
    </div>
  )
}

function ListState({ error, loading, empty, retry, children }: { error: unknown; loading: boolean; empty: ReactNode | false; retry: () => void; children: ReactNode }) {
  if (error) return <ErrorBox error={error} onRetry={retry} />
  if (loading) return <Loading />
  if (empty) return <>{empty}</>
  return <>{children}</>
}

function Comments({ n }: { n: number | null | undefined }) {
  if (!n) return null
  return (
    <span className="wb-row" style={{ gap: 3, justifyContent: 'flex-end' }} title={`${n} comments`}>
      <MessageSquare size={11} />
      {n}
    </span>
  )
}

function SearchBox({ value, onSearch }: { value: string | undefined; onSearch: (v: string | undefined) => void }) {
  const [text, setText] = useState(value ?? '')
  return (
    <Input
      small
      className="wb-grow"
      placeholder="Search"
      value={text}
      onChange={(e) => setText(e.target.value)}
      onKeyDown={(e) => e.key === 'Enter' && onSearch(text.trim() || undefined)}
      onBlur={() => onSearch(text.trim() || undefined)}
    />
  )
}

// ---------------------------------------------------------------- runs

export function RunsList({
  projectId,
  summary,
  onOpen,
  compact,
}: {
  projectId: string
  summary: GithubSummary | undefined
  onOpen?: (r: Run) => void
  compact?: boolean
}) {
  const qc = useQueryClient()
  const openRunWorkflow = useGhUi((s) => s.openRunWorkflow)
  const anon = summary ? !summary.auth.authenticated : false
  const [branch, setBranch] = useState('')
  const [status, setStatus] = useState('')
  const [workflowId, setWorkflowId] = useState<number | undefined>(undefined)
  const workflows = useWorkflows(projectId, !compact)
  const q = useRuns(projectId, { branch: branch || undefined, status: status || undefined, workflowId }, anon)
  const items = useMemo(() => mergeRuns(q.data?.pages.map((p) => p.items) ?? []), [q.data])
  const total = q.data?.pages[0]?.total
  const open = onOpen ?? ((r: Run) => openRun(projectId, r.id, runTitle(r, r.id)))
  const branches = [...new Set([summary?.branch, summary?.defaultBranch].filter((r): r is string => !!r))]

  const act = (what: string, fn: () => Promise<unknown>, ok: string) =>
    fn().then(
      () => {
        toast('success', ok)
        void qc.invalidateQueries({ queryKey: ghk.runs(projectId) })
      },
      (err) => toastError(err, `Could not ${what}`),
    )
  const menu = (e: MouseEvent, r: Run) =>
    showMenu(e, [
      { label: 'Open', run: () => open(r) },
      { label: 'Open on GitHub', icon: ExternalLink, run: () => window.open(r.htmlUrl, '_blank', 'noopener,noreferrer') },
      'separator',
      {
        label: anon ? `Re-run failed jobs (${TOKEN_HINT})` : 'Re-run failed jobs',
        icon: RotateCw,
        disabled: anon || r.status !== 'completed' || !['failed', 'canceled'].includes(r.state),
        run: () => act('re-run', () => ghApi.rerunFailed(projectId, r.id), `Re-running the failed jobs of ${runTitle(r, r.id)}`),
      },
      {
        label: anon ? `Re-run all jobs (${TOKEN_HINT})` : 'Re-run all jobs',
        icon: RotateCw,
        disabled: anon || r.status !== 'completed',
        run: () => act('re-run', () => ghApi.rerun(projectId, r.id), `Re-running ${runTitle(r, r.id)}`),
      },
      {
        label: anon ? `Cancel (${TOKEN_HINT})` : 'Cancel',
        icon: Square,
        disabled: anon || !isActive(r.state),
        run: () => act('cancel', () => ghApi.cancel(projectId, r.id), `Cancelling ${runTitle(r, r.id)}`),
      },
      'separator',
      { label: 'Copy commit sha', icon: Copy, run: () => copyText(r.headSha, 'Commit sha copied') },
    ])

  return (
    <div className="wb-fill">
      <div className="gh-filters">
        {!compact && (workflows.data?.length ?? 0) > 1 && (
          <Select value={workflowId ?? ''} onChange={(e) => setWorkflowId(e.target.value ? Number(e.target.value) : undefined)} title="Workflow">
            <option value="">Workflow</option>
            {workflows.data!.map((w) => (
              <option key={w.id} value={w.id}>
                {w.name}
              </option>
            ))}
          </Select>
        )}
        <Select value={branch} onChange={(e) => setBranch(e.target.value)} title="Branch">
          <option value="">Branch</option>
          {branches.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </Select>
        <Select value={status} onChange={(e) => setStatus(e.target.value)} title="Status">
          <option value="">Status</option>
          <option value="in_progress">In progress</option>
          <option value="queued">Queued</option>
          <option value="success">Success</option>
          <option value="failure">Failure</option>
          <option value="cancelled">Cancelled</option>
          <option value="waiting">Waiting</option>
        </Select>
        <span style={{ flex: 1 }} />
        {total !== null && total !== undefined && <span className="wb-xs wb-subtle">{total}</span>}
        {!compact && (
          <IconButton
            icon={Play}
            size="small"
            label={anon ? `Run workflow… (${TOKEN_HINT})` : 'Run workflow…'}
            disabled={anon}
            onClick={() => openRunWorkflow(projectId, workflowId ?? null)}
          />
        )}
        <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={() => qc.invalidateQueries({ queryKey: ghk.runs(projectId) })} />
      </div>
      <div className="gh-list">
        <ListState
          error={q.error}
          loading={q.isLoading}
          retry={() => q.refetch()}
          empty={
            !items.length && (
              <EmptyState title="No workflow runs">{branch || status || workflowId ? 'Nothing matches these filters.' : 'This repository has not run GitHub Actions yet.'}</EmptyState>
            )
          }
        >
          {items.map((r) => (
            <RunRow key={r.id} r={r} onOpen={() => open(r)} onContextMenu={(e) => menu(e, r)} />
          ))}
          <More has={!!q.hasNextPage} loading={q.isFetchingNextPage} onMore={() => q.fetchNextPage()} />
        </ListState>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- pull requests

export function PullsList({ projectId, anonymous }: { projectId: string; anonymous: boolean }) {
  const openCreate = useGhUi((s) => s.openCreatePr)
  const [filters, setFilters] = useState<PullFilters>({ state: 'open' })
  const q = usePulls(projectId, filters)
  const items = q.data?.pages.flatMap((p) => p.items) ?? []
  return (
    <div className="wb-fill">
      <div className="gh-filters">
        <Select value={filters.state} onChange={(e) => setFilters({ ...filters, state: e.target.value })}>
          <option value="open">Open</option>
          <option value="merged">Merged</option>
          <option value="closed">Closed</option>
          <option value="all">All</option>
        </Select>
        <SearchBox value={filters.search} onSearch={(search) => setFilters({ ...filters, search })} />
        <IconButton
          icon={Plus}
          size="small"
          label={anonymous ? `Create pull request… (${TOKEN_HINT})` : 'Create pull request…'}
          disabled={anonymous}
          onClick={() => openCreate(projectId)}
        />
      </div>
      <div className="gh-list">
        <ListState
          error={q.error}
          loading={q.isLoading}
          retry={() => q.refetch()}
          empty={
            !items.length && (
              <EmptyState icon={GitPullRequest} title={filters.state === 'open' ? 'No open pull requests' : 'No pull requests'}>
                {!anonymous && (
                  <Button size="small" icon={Plus} onClick={() => openCreate(projectId)}>
                    Create pull request
                  </Button>
                )}
              </EmptyState>
            )
          }
        >
          {items.map((p) => (
            <div key={p.number} className="gh-row" onClick={() => openPr(projectId, p.number, p.title)} {...rowKeys(() => openPr(projectId, p.number, p.title))}>
              <PrStateIcon pr={p} />
              <span className="title">
                <span className="num">#{p.number}</span>
                {p.title}
              </span>
              <span className="right">
                <Comments n={p.comments} />
              </span>
              <span className="meta">
                {p.head && <RefLabel name={p.head.ref} />}
                {p.base && (
                  <>
                    <span className="gh-sep">→</span>
                    <span className="wb-ellipsis">{p.base.ref}</span>
                  </>
                )}
                <span>· {p.user?.login}</span>
                {p.draft && <Badge>Draft</Badge>}
              </span>
              <span className="right">
                <TimeAgo time={p.updatedAt} />
              </span>
            </div>
          ))}
          <More has={!!q.hasNextPage} loading={q.isFetchingNextPage} onMore={() => q.fetchNextPage()} />
        </ListState>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- issues

export function IssuesList({ projectId, anonymous }: { projectId: string; anonymous: boolean }) {
  const qc = useQueryClient()
  const [filters, setFilters] = useState<IssueFilters>({ state: 'open' })
  const q = useIssues(projectId, filters)
  const items = q.data?.pages.flatMap((p) => p.items) ?? []
  const create = async () => {
    const title = await promptDialog({ title: 'New issue', label: 'Title', confirmLabel: 'Next' })
    if (!title?.trim()) return
    const body = await promptDialog({ title: 'New issue', label: 'Description (Markdown, optional)', multiline: true, confirmLabel: 'Create issue' })
    if (body === null) return
    try {
      const issue = await ghApi.createIssue(projectId, { title: title.trim(), body: body || undefined })
      toast('success', `Created #${issue.number}`)
      void qc.invalidateQueries({ queryKey: ghk.issues(projectId) })
      openIssue(projectId, issue.number, issue.title)
    } catch (e) {
      toastError(e, 'Could not create the issue')
    }
  }
  return (
    <div className="wb-fill">
      <div className="gh-filters">
        <Select value={filters.state} onChange={(e) => setFilters({ ...filters, state: e.target.value })}>
          <option value="open">Open</option>
          <option value="closed">Closed</option>
          <option value="all">All</option>
        </Select>
        <SearchBox value={filters.search} onSearch={(search) => setFilters({ ...filters, search })} />
        <IconButton icon={Plus} size="small" label={anonymous ? `New issue… (${TOKEN_HINT})` : 'New issue…'} disabled={anonymous} onClick={create} />
      </div>
      <div className="gh-list">
        <ListState
          error={q.error}
          loading={q.isLoading}
          retry={() => q.refetch()}
          empty={!items.length && <EmptyState icon={CircleDot} title={filters.state === 'open' ? 'No open issues' : 'No issues'} />}
        >
          {items.map((i) => (
            <div key={i.number} className="gh-row" onClick={() => openIssue(projectId, i.number, i.title)} {...rowKeys(() => openIssue(projectId, i.number, i.title))}>
              <IssueStateIcon state={i.state} reason={i.stateReason} />
              <span className="title">
                <span className="num">#{i.number}</span>
                {i.title}
              </span>
              <span className="right">
                <Comments n={i.comments} />
              </span>
              <span className="meta">
                {i.labels.slice(0, 3).map((l) => (
                  <span className="gh-label" key={l.id || l.name}>
                    {l.name}
                  </span>
                ))}
                <span>{i.user?.login}</span>
              </span>
              <span className="right">
                <TimeAgo time={i.updatedAt} />
              </span>
            </div>
          ))}
          <More has={!!q.hasNextPage} loading={q.isFetchingNextPage} onMore={() => q.fetchNextPage()} />
        </ListState>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- releases

export function ReleasesList({ projectId }: { projectId: string }) {
  const q = useReleases(projectId)
  const [open, setOpen] = useState<number | null>(null)
  const items = q.data?.pages.flatMap((p) => p.items) ?? []
  return (
    <div className="gh-list">
      <ListState error={q.error} loading={q.isLoading} retry={() => q.refetch()} empty={!items.length && <EmptyState icon={Tag} title="No releases" />}>
        {items.map((r, i) => (
          <div
            key={r.id}
            className="gh-row gh-release"
            onClick={() => setOpen(open === r.id ? null : r.id)}
            {...rowKeys(() => setOpen(open === r.id ? null : r.id))}
          >
            <Package size={14} className={i === 0 && !r.draft && !r.prerelease ? 'wb-success' : 'wb-muted'} />
            <span className="title">
              {r.name || r.tagName}{' '}
              {r.draft && <Badge>Draft</Badge>} {r.prerelease && <Badge tone="warning">Pre-release</Badge>}{' '}
              {i === 0 && !r.draft && !r.prerelease && <Badge tone="success">Latest</Badge>}
            </span>
            <span className="right">
              <ExtLink href={r.htmlUrl} />
            </span>
            <span className="meta">
              <RefLabel name={r.tagName} tag />
              {r.author && <span>{r.author.login}</span>}
              {r.assets.length > 0 && <span>{r.assets.length} asset{r.assets.length === 1 ? "" : "s"}</span>}
            </span>
            <span className="right">
              <TimeAgo time={r.publishedAt ?? r.createdAt} />
            </span>
            {open === r.id && r.assets.length > 0 && (
              <span className="assets" onClick={(e) => e.stopPropagation()}>
                {r.assets.map((a) => (
                  <a key={a.id} className="gh-link wb-row" href={a.browserDownloadUrl} target="_blank" rel="noopener noreferrer" title={`${a.downloadCount} downloads`}>
                    <Download size={11} /> {a.name} <span className="wb-subtle">{formatBytes(a.size)}</span>
                  </a>
                ))}
              </span>
            )}
          </div>
        ))}
        <More has={!!q.hasNextPage} loading={q.isFetchingNextPage} onMore={() => q.fetchNextPage()} />
      </ListState>
    </div>
  )
}
