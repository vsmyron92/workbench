// The lists in the GitLab tool window (and the phone's CI tab): pipelines,
// merge requests, issues, environments and registry tags.

import { useMemo, useState, type MouseEvent, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, Copy, ExternalLink, GitPullRequest, Globe, MessageSquare, Package, Play, Plus, RefreshCw, RotateCw, Square } from 'lucide-react'
import { promptDialog, toast, toastError } from '@/shell/actions'
import { Badge, Button, EmptyState, ErrorBox, formatBytes, IconButton, Input, Loading, Select, showMenu, TimeAgo } from '@/ui'
import {
  glApi,
  glk,
  useDeployments,
  useEnvironments,
  useIssues,
  useMrs,
  usePipelines,
  useRegistry,
  useTag,
  useTags,
  type IssueFilters,
  type MrFilters,
} from './api'
import { copyText, ExtLink, openIssue, openMr, openPipeline, PipelineRow, RefLabel, StatusIcon, useGlUi } from './components'
import { IssueStateIcon } from './IssuePanel'
import { isActive, mergePipelines, rowKeys, shortSha } from './logic'
import { MrStateIcon } from './MrPanel'
import type { GitlabSummary, Pipeline, RegistryTag } from './types'

function More({ has, loading, onMore }: { has: boolean; loading: boolean; onMore: () => void }) {
  if (!has) return null
  return (
    <div className="gl-more">
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

function Comments({ n }: { n: number }) {
  if (!n) return null
  return (
    <span className="wb-row" style={{ gap: 3, justifyContent: 'flex-end' }} title={`${n} comments`}>
      <MessageSquare size={11} />
      {n}
    </span>
  )
}

// ---------------------------------------------------------------- pipelines

export function PipelinesList({
  projectId,
  summary,
  onOpen,
  compact,
}: {
  projectId: string
  summary: GitlabSummary | undefined
  onOpen?: (p: Pipeline) => void
  compact?: boolean
}) {
  const qc = useQueryClient()
  const openRun = useGlUi((s) => s.openRunPipeline)
  const [refFilter, setRefFilter] = useState('')
  const [status, setStatus] = useState('')
  const q = usePipelines(projectId, { ref: refFilter || undefined, status: status || undefined })
  const items = useMemo(() => mergePipelines(q.data?.pages.map((p) => p.items) ?? []), [q.data])
  const total = q.data?.pages[0]?.total
  const open = onOpen ?? ((p: Pipeline) => openPipeline(projectId, p.id, p.iid))
  const refs = [...new Set([summary?.branch, summary?.defaultBranch].filter((r): r is string => !!r))]

  const menu = (e: MouseEvent, p: Pipeline) =>
    showMenu(e, [
      { label: 'Open', run: () => open(p) },
      { label: 'Open in GitLab', icon: ExternalLink, run: () => window.open(p.webUrl, '_blank', 'noopener,noreferrer') },
      'separator',
      {
        label: 'Retry failed jobs',
        icon: RotateCw,
        disabled: !['failed', 'canceled'].includes(p.status),
        run: () => glApi.retryPipeline(projectId, p.id).then(() => toast('success', `Retrying #${p.iid ?? p.id}`), (err) => toastError(err)),
      },
      {
        label: 'Cancel',
        icon: Square,
        disabled: !isActive(p.status),
        run: () => glApi.cancelPipeline(projectId, p.id).then(() => toast('success', `Canceled #${p.iid ?? p.id}`), (err) => toastError(err)),
      },
      'separator',
      { label: 'Copy commit sha', icon: Copy, run: () => copyText(p.sha, 'Commit sha copied') },
    ])

  return (
    <div className="wb-fill">
      <div className="gl-filters">
        <Select value={refFilter} onChange={(e) => setRefFilter(e.target.value)} title="Ref">
          <option value="">All refs</option>
          {refs.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </Select>
        <Select value={status} onChange={(e) => setStatus(e.target.value)} title="Status">
          <option value="">Any status</option>
          <option value="running">Running</option>
          <option value="pending">Pending</option>
          <option value="success">Passed</option>
          <option value="failed">Failed</option>
          <option value="canceled">Canceled</option>
          <option value="manual">Manual</option>
        </Select>
        <span style={{ flex: 1 }} />
        {total !== null && total !== undefined && <span className="wb-xs wb-subtle">{total}</span>}
        {!compact && <IconButton icon={Play} size="small" label="Run pipeline…" onClick={() => openRun(projectId)} />}
        <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={() => qc.invalidateQueries({ queryKey: glk.pipelines(projectId) })} />
      </div>
      <div className="gl-list">
        <ListState
          error={q.error}
          loading={q.isLoading}
          retry={() => q.refetch()}
          empty={!items.length && <EmptyState title="No pipelines" >{refFilter || status ? 'Nothing matches these filters.' : 'This project has not run CI yet.'}</EmptyState>}
        >
          {items.map((p) => (
            <PipelineRow key={p.id} p={p} onOpen={() => open(p)} onContextMenu={(e) => menu(e, p)} />
          ))}
          <More has={!!q.hasNextPage} loading={q.isFetchingNextPage} onMore={() => q.fetchNextPage()} />
        </ListState>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- merge requests

export function MrList({ projectId }: { projectId: string }) {
  const openCreate = useGlUi((s) => s.openCreateMr)
  const [filters, setFilters] = useState<MrFilters>({ state: 'opened' })
  const [search, setSearch] = useState('')
  const q = useMrs(projectId, filters)
  const items = q.data?.pages.flatMap((p) => p.items) ?? []
  return (
    <div className="wb-fill">
      <div className="gl-filters">
        <Select value={filters.state} onChange={(e) => setFilters({ ...filters, state: e.target.value })}>
          <option value="opened">Open</option>
          <option value="merged">Merged</option>
          <option value="closed">Closed</option>
          <option value="all">All</option>
        </Select>
        <Input
          small
          className="wb-grow"
          placeholder="Search"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && setFilters({ ...filters, search: search.trim() || undefined })}
          onBlur={() => setFilters({ ...filters, search: search.trim() || undefined })}
        />
        <IconButton icon={Plus} size="small" label="Create merge request…" onClick={() => openCreate(projectId)} />
      </div>
      <div className="gl-list">
        <ListState
          error={q.error}
          loading={q.isLoading}
          retry={() => q.refetch()}
          empty={
            !items.length && (
              <EmptyState icon={GitPullRequest} title={filters.state === 'opened' ? 'No open merge requests' : 'No merge requests'}>
                <Button size="small" icon={Plus} onClick={() => openCreate(projectId)}>
                  Create merge request
                </Button>
              </EmptyState>
            )
          }
        >
          {items.map((m) => (
            <div key={m.iid} className="gl-row" onClick={() => openMr(projectId, m.iid, m.title)} {...rowKeys(() => openMr(projectId, m.iid, m.title))}>
              <MrStateIcon mr={m} />
              <span className="title">
                <span className="num">!{m.iid}</span>
                {m.title}
              </span>
              <span className="right">
                <Comments n={m.userNotesCount} />
              </span>
              <span className="meta">
                <RefLabel name={m.sourceBranch} />
                <span className="gl-sep">→</span>
                <span className="wb-ellipsis">{m.targetBranch}</span>
                <span>· {m.author?.username}</span>
                {m.draft && <Badge>Draft</Badge>}
                {m.hasConflicts && <Badge tone="danger">Conflicts</Badge>}
              </span>
              <span className="right">
                <TimeAgo time={m.updatedAt} />
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

export function IssuesList({ projectId }: { projectId: string }) {
  const qc = useQueryClient()
  const [filters, setFilters] = useState<IssueFilters>({ state: 'opened' })
  const [search, setSearch] = useState('')
  const q = useIssues(projectId, filters)
  const items = q.data?.pages.flatMap((p) => p.items) ?? []
  const create = async () => {
    const title = await promptDialog({ title: 'New issue', label: 'Title', confirmLabel: 'Next' })
    if (!title?.trim()) return
    const description = await promptDialog({ title: 'New issue', label: 'Description (Markdown, optional)', multiline: true, confirmLabel: 'Create issue' })
    if (description === null) return
    try {
      const issue = await glApi.createIssue(projectId, { title: title.trim(), description: description || undefined })
      toast('success', `Created #${issue.iid}`)
      qc.invalidateQueries({ queryKey: glk.issues(projectId) })
      openIssue(projectId, issue.iid, issue.title)
    } catch (e) {
      toastError(e, 'Could not create the issue')
    }
  }
  return (
    <div className="wb-fill">
      <div className="gl-filters">
        <Select value={filters.state} onChange={(e) => setFilters({ ...filters, state: e.target.value })}>
          <option value="opened">Open</option>
          <option value="closed">Closed</option>
          <option value="all">All</option>
        </Select>
        <Input
          small
          className="wb-grow"
          placeholder="Search"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && setFilters({ ...filters, search: search.trim() || undefined })}
          onBlur={() => setFilters({ ...filters, search: search.trim() || undefined })}
        />
        <IconButton icon={Plus} size="small" label="New issue…" onClick={create} />
      </div>
      <div className="gl-list">
        <ListState
          error={q.error}
          loading={q.isLoading}
          retry={() => q.refetch()}
          empty={!items.length && <EmptyState title={filters.state === 'opened' ? 'No open issues' : 'No issues'} />}
        >
          {items.map((i) => (
            <div key={i.iid} className="gl-row" onClick={() => openIssue(projectId, i.iid, i.title)} {...rowKeys(() => openIssue(projectId, i.iid, i.title))}>
              <IssueStateIcon state={i.state} />
              <span className="title">
                <span className="num">#{i.iid}</span>
                {i.title}
              </span>
              <span className="right">
                <Comments n={i.userNotesCount} />
              </span>
              <span className="meta">
                {i.labels.slice(0, 3).map((l) => (
                  <span className="gl-label" key={l}>
                    {l}
                  </span>
                ))}
                <span>{i.author?.username}</span>
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

// ---------------------------------------------------------------- environments

function Deployments({ projectId, env }: { projectId: string; env: string }) {
  const q = useDeployments(projectId, env)
  if (q.error) return <ErrorBox error={q.error} />
  if (!q.data) return <Loading />
  if (!q.data.items.length) return <div className="wb-small wb-muted" style={{ padding: '4px 30px' }}>No deployments</div>
  return (
    <>
      {q.data.items.map((d) => (
        <div
          key={d.id}
          className="gl-row"
          style={{ paddingLeft: 26 }}
          onClick={() => d.deployable?.pipeline && openPipeline(projectId, d.deployable.pipeline.id, d.deployable.pipeline.iid)}
        >
          <StatusIcon status={d.status} />
          <span className="title">
            <span className="num">#{d.iid ?? d.id}</span>
            <span className="gl-mono">{shortSha(d.sha)}</span> {d.ref}
          </span>
          <span className="right">
            <TimeAgo time={d.finishedAt ?? d.createdAt} />
          </span>
        </div>
      ))}
    </>
  )
}

export function EnvironmentsList({ projectId }: { projectId: string }) {
  const q = useEnvironments(projectId)
  const [open, setOpen] = useState<string | null>(null)
  return (
    <div className="gl-list">
      <ListState error={q.error} loading={q.isLoading} retry={() => q.refetch()} empty={!q.data?.length && <EmptyState icon={Globe} title="No environments" />}>
        {q.data?.map((e) => {
          const d = e.lastDeployment
          return (
            <div key={e.id}>
              <div className="gl-row" onClick={() => setOpen(open === e.name ? null : e.name)} {...rowKeys(() => setOpen(open === e.name ? null : e.name))}>
                {open === e.name ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                <span className="title">
                  {e.name} {e.tier && <Badge>{e.tier}</Badge>}
                </span>
                <span className="right">
                  <ExtLink href={e.externalUrl} title={e.externalUrl ?? undefined} />
                </span>
                <span className="meta">
                  {d ? (
                    <>
                      <StatusIcon status={d.status} size={12} />
                      <span className="gl-mono">{shortSha(d.sha)}</span>
                      <span className="wb-ellipsis">{d.ref}</span>
                      {d.user && <span>· {d.user.username}</span>}
                    </>
                  ) : (
                    <span>never deployed</span>
                  )}
                </span>
                <span className="right">{d && <TimeAgo time={d.finishedAt ?? d.createdAt} />}</span>
              </div>
              {open === e.name && <Deployments projectId={projectId} env={e.name} />}
            </div>
          )
        })}
      </ListState>
    </div>
  )
}

// ---------------------------------------------------------------- registry

function TagDetail({ projectId, rid, tag }: { projectId: string; rid: number; tag: RegistryTag }) {
  const q = useTag(projectId, rid, tag.name)
  const t = q.data ?? tag
  return (
    <div className="gl-card" style={{ margin: '0 8px 6px 30px' }}>
      <div className="gl-kv gl-card-body">
        <span className="k">Digest</span>
        <span className="gl-mono wb-ellipsis" title={t.digest ?? ''}>
          {t.digest ?? (q.isLoading ? '…' : '—')}
        </span>
        <span className="k">Created</span>
        <span>{t.createdAt ? new Date(t.createdAt).toLocaleString() : '—'}</span>
        {t.publishedAt && (
          <>
            <span className="k">Published</span>
            <span>{new Date(t.publishedAt).toLocaleString()}</span>
          </>
        )}
        {t.totalSize ? (
          <>
            <span className="k">Size</span>
            <span>{formatBytes(t.totalSize)}</span>
          </>
        ) : null}
      </div>
      <div className="actions">
        <Button size="small" icon={Copy} onClick={() => copyText(`docker pull ${t.location}`, 'Pull command copied')}>
          Copy pull command
        </Button>
        {t.digest && (
          <Button size="small" variant="ghost" onClick={() => copyText(t.digest!, 'Digest copied')}>
            Copy digest
          </Button>
        )}
      </div>
    </div>
  )
}

export function RegistryView({ projectId, head }: { projectId: string; head: string | null }) {
  const repos = useRegistry(projectId)
  const [repoId, setRepoId] = useState<number | null>(null)
  const rid = repoId ?? repos.data?.[0]?.id ?? null
  const tags = useTags(projectId, rid)
  const [open, setOpen] = useState<string | null>(null)
  const items = tags.data?.pages.flatMap((p) => p.items) ?? []
  const first = tags.data?.pages[0]
  const headShort = head ? head.slice(0, 8) : null
  // Digest of `latest`, to mark which sha tag it points at.
  const latest = items.find((t) => t.name === 'latest')?.digest
  if (repos.error) return <ErrorBox error={repos.error} onRetry={() => repos.refetch()} />
  if (repos.isLoading) return <Loading />
  if (!repos.data?.length) return <EmptyState icon={Package} title="No container images" />
  return (
    <div className="wb-fill">
      <div className="gl-filters">
        {repos.data.length > 1 ? (
          <Select value={rid ?? ''} onChange={(e) => setRepoId(Number(e.target.value))}>
            {repos.data.map((r) => (
              <option key={r.id} value={r.id}>
                {r.path}
              </option>
            ))}
          </Select>
        ) : (
          <span className="wb-small wb-ellipsis gl-mono" title={repos.data[0].location}>
            {repos.data[0].location}
          </span>
        )}
        <span style={{ flex: 1 }} />
        <span className="wb-xs wb-subtle">
          {first?.total ?? repos.data.find((r) => r.id === rid)?.tagsCount ?? ''} tags{first?.order === 'name' ? ' · A–Z' : ''}
        </span>
      </div>
      <div className="gl-list">
        <ListState error={tags.error} loading={tags.isLoading} retry={() => tags.refetch()} empty={!items.length && <EmptyState title="No tags" />}>
          {items.map((t) => (
            <div key={t.name}>
              <div className="gl-row" onClick={() => setOpen(open === t.name ? null : t.name)} {...rowKeys(() => setOpen(open === t.name ? null : t.name))}>
                <Package size={14} className="wb-muted" />
                <span className="title">
                  <span className="gl-mono" style={{ fontSize: 'var(--fs-sm)' }}>
                    {t.name}
                  </span>{' '}
                  {t.name === headShort && <Badge tone="accent">HEAD</Badge>}{' '}
                  {latest && t.name !== 'latest' && t.digest === latest && <Badge tone="success">latest</Badge>}
                </span>
                <span className="right">{t.totalSize ? formatBytes(t.totalSize) : ''}</span>
                <span className="meta">
                  <span className="gl-mono">{t.digest ? t.digest.replace('sha256:', '').slice(0, 12) : ''}</span>
                </span>
                <span className="right">
                  <TimeAgo time={t.publishedAt ?? t.createdAt} />
                </span>
              </div>
              {open === t.name && rid !== null && <TagDetail projectId={projectId} rid={rid} tag={t} />}
            </div>
          ))}
          <More has={!!tags.hasNextPage} loading={tags.isFetchingNextPage} onMore={() => tags.fetchNextPage()} />
        </ListState>
      </div>
    </div>
  )
}
