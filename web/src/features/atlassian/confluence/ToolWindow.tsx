// The Confluence tool window: search, space picker (project spaces first), the
// project's pinned and root pages, recently opened pages and the lazy page tree.

import { useEffect, useMemo, useRef, useState } from 'react'
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query'
import { Archive, FilePlus, FileText, Layers, Pin, RefreshCw, Search, X } from 'lucide-react'
import { ApiError } from '@/api/client'
import { configFileHint, useHealth } from '@/api/health'
import { useProject } from '@/api/queries'
import { useUi } from '@/state/store'
import { Button, EmptyState, ErrorBox, IconButton, Input, Loading, Section, Select, Spinner, TimeAgo } from '@/ui'
import { ConfluenceIcon } from '@/ui/brand'
import { confluenceApi, needsSetup, qk, refreshAtlassianStatus, useAtlassianStatusQuery, useSpaces, type SearchHit, type Space, type TreeNode } from '../api'
import { confluenceLinks, setupSummary } from '../links'
import { useAtlassianUi, usePrefs } from '../state'
import { openConfluencePage } from './actions'
import { nodeMenu, PageTree } from './PageTree'

/**
 * The status check failed or found a problem: explain how to set Atlassian up. Setup
 * messages lose the server's own copy of the snippet (the formatted one below says it).
 */
export function SetupHint({ error, message, onRetry }: { error?: unknown; message?: string | null; onRetry?: () => void }) {
  const setup = !error || (error instanceof ApiError && error.notConfigured)
  const text = error instanceof Error ? error.message : message
  const err = setup ? new ApiError(412, 'not_configured', setupSummary(text)) : error
  // Where the server says config.toml is (its status answers even where Atlassian is not set
  // up, and nothing may have asked yet: a panel restored in a project without links); until
  // it answers, its OS's usual place.
  const projectId = useUi((s) => s.projectId)
  const os = useHealth()?.os
  const configFile = useAtlassianStatusQuery(projectId).data?.configFile ?? configFileHint(os)
  return (
    <div className="wb-scroll atl-setup">
      <ErrorBox error={err} onRetry={onRetry} />
      <div className="wb-pad wb-small wb-muted" style={{ lineHeight: 1.6 }}>
        Workbench reads Confluence and Jira with an Atlassian API token. In <code style={{ overflowWrap: 'anywhere' }}>{configFile}</code>:
        <pre className="mono wb-small" style={{ background: 'var(--bg-inset)', padding: 8, borderRadius: 6, overflowX: 'auto' }}>
          {`[atlassian]
site = "https://<your-site>.atlassian.net"
email = "you@example.com"
token = "atlassian"

[secrets]
atlassian = { file = "~/.atlassian_token" }`}
        </pre>
        Create a token at id.atlassian.com → Security → API tokens. Jira turns on by itself when the site has it.
      </div>
    </div>
  )
}

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value)
  useEffect(() => {
    const t = window.setTimeout(() => setV(value), ms)
    return () => window.clearTimeout(t)
  }, [value, ms])
  return v
}

function SearchResults({ projectId, q, space, archived }: { projectId: string | null; q: string; space: string; archived: boolean }) {
  const query = useInfiniteQuery({
    queryKey: qk.search(projectId, q, space, archived),
    queryFn: ({ pageParam }) => confluenceApi.search(projectId, q, space, archived, pageParam),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
    retry: false,
    staleTime: 60_000,
  })
  if (query.isLoading) return <Loading label="Searching…" />
  if (query.error) return <ErrorBox error={query.error} onRetry={() => query.refetch()} />
  const hits: SearchHit[] = query.data?.pages.flatMap((p) => p.results) ?? []
  if (!hits.length) return <EmptyState icon={Search} title="No pages found">{space ? `in ${space} — try all spaces` : 'Try other words'}</EmptyState>
  return (
    <div className="wb-scroll">
      {hits.map((h) => (
        <div
          key={h.id}
          className="atl-hit"
          onClick={() => openConfluencePage(h.id, h.title)}
          onContextMenu={(e) =>
            nodeMenu(e, { id: h.id, title: h.title, type: h.type, status: h.status, hasChildren: null, position: null, parentId: null, spaceId: null }, projectId)
          }
        >
          <div className="t">
            <FileText size={13} className="wb-muted" />
            <span className="wb-ellipsis">{h.title}</span>
            {h.status === 'archived' && <span className="wb-subtle wb-xs">archived</span>}
          </div>
          {h.excerpt && <div className="x">{h.excerpt}</div>}
          <div className="m">
            {h.spaceName ?? h.spaceKey}
            {h.lastModified && (
              <>
                {' · '}
                <TimeAgo time={h.lastModified} />
              </>
            )}
          </div>
        </div>
      ))}
      {query.hasNextPage && (
        <div className="wb-pad">
          <Button size="small" loading={query.isFetchingNextPage} onClick={() => query.fetchNextPage()}>
            More results
          </Button>
        </div>
      )}
    </div>
  )
}

function sortSpaces(spaces: Space[], preferredKey: string | null): Space[] {
  return [...spaces].sort((a, b) => {
    const pa = a.key === preferredKey ? 0 : a.type === 'personal' ? 2 : 1
    const pb = b.key === preferredKey ? 0 : b.type === 'personal' ? 2 : 1
    return pa - pb || a.name.localeCompare(b.name)
  })
}

function SpaceTree({ projectId, space, archived }: { projectId: string | null; space: Space; archived: boolean }) {
  const status = archived ? 'all' : 'current'
  const q = useQuery({
    queryKey: qk.roots(projectId, space.id, status),
    queryFn: () => confluenceApi.roots(projectId, space.id, status),
    staleTime: 5 * 60_000,
    retry: false,
  })
  if (q.isLoading) return <div className="atl-row-note" style={{ paddingLeft: 12 }}><Spinner size={11} /> Loading pages…</div>
  if (q.error) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  const roots = q.data?.children ?? []
  if (!roots.length) return <div className="atl-row-note" style={{ paddingLeft: 12 }}>No pages</div>
  return <PageTree roots={roots} projectId={projectId} archived={archived} />
}

function PageList({ projectId, ids, labels, archived }: { projectId: string | null; ids: string[]; labels?: Record<string, string>; archived: boolean }) {
  const q = useQuery({
    queryKey: qk.byIds(projectId, ids),
    queryFn: () => confluenceApi.byIds(projectId, ids),
    enabled: ids.length > 0,
    staleTime: 10 * 60_000,
    retry: false,
  })
  if (q.isLoading) return <div className="atl-row-note" style={{ paddingLeft: 12 }}><Spinner size={11} /> Loading…</div>
  if (q.error) return <div className="atl-row-note wb-danger" style={{ paddingLeft: 12 }}>{(q.error as Error).message}</div>
  const nodes: TreeNode[] = (q.data ?? []).map((n) => ({ ...n, title: labels?.[n.id] ?? n.title }))
  return <PageTree roots={nodes} projectId={projectId} archived={archived} />
}

function Recent({ projectId }: { projectId: string | null }) {
  const recent = usePrefs((s) => s.recent)
  if (!recent.length) return <div className="atl-row-note" style={{ paddingLeft: 12 }}>Pages you open appear here</div>
  return (
    <div>
      {recent.slice(0, 8).map((r) => (
        <div
          key={r.id}
          className="atl-row"
          style={{ paddingLeft: 8 }}
          title={r.title}
          onClick={() => openConfluencePage(r.id, r.title)}
          onContextMenu={(e) => nodeMenu(e, { id: r.id, title: r.title, type: 'page', status: 'current', hasChildren: false, position: null, parentId: null, spaceId: null }, projectId)}
        >
          <span className="ico page">
            <FileText size={14} />
          </span>
          <span className="name">{r.title}</span>
          <span className="hint">
            <TimeAgo time={r.at} />
          </span>
        </div>
      ))}
    </div>
  )
}

export function ConfluenceToolWindow({ projectId }: { projectId: string | null }) {
  const qc = useQueryClient()
  const status = useAtlassianStatusQuery(projectId)
  const project = useProject(projectId)
  const links = confluenceLinks(project.data?.config)
  const spacesQ = useSpaces(projectId, !!status.data?.confluence)
  const prefs = usePrefs()
  const archived = prefs.archived || links.archived
  const [text, setText] = useState('')
  const [allSpaces, setAllSpaces] = useState(false)
  const q = useDebounced(text.trim(), 300)
  const inputRef = useRef<HTMLInputElement>(null)
  const focusSearch = useAtlassianUi((s) => s.focusSearch)

  useEffect(() => {
    if (focusSearch) window.setTimeout(() => inputRef.current?.focus(), 30)
  }, [focusSearch])

  const spaces = useMemo(() => sortSpaces(spacesQ.data ?? [], links.space), [spacesQ.data, links.space])
  const chosenId = prefs.spaceByProject[projectId ?? '']
  const space = spaces.find((s) => s.id === chosenId) ?? spaces.find((s) => s.key === links.space) ?? spaces[0] ?? null
  const hasProjectRoots = links.rootPages.length > 0
  const wholeSpace = !hasProjectRoots || !!prefs.wholeSpace[projectId ?? '']

  const recheck = () => void refreshAtlassianStatus(qc, projectId)
  if (status.isLoading) return <Loading label="Connecting to Atlassian…" />
  if (status.error) return <SetupHint error={status.error} onRetry={recheck} />
  const st = status.data!
  if (needsSetup(st)) return <SetupHint message={st.error ?? 'Atlassian rejected the API token'} onRetry={recheck} />
  if (!st.confluence)
    return (
      <EmptyState icon={ConfluenceIcon} title="No Confluence on this site">
        {st.error ?? `${st.site} has no Confluence (or this account cannot see it).`}
      </EmptyState>
    )

  const refresh = () => {
    qc.invalidateQueries({ queryKey: ['confluence'] })
    recheck()
  }
  const searching = q.length >= 2

  return (
    <div className="atl-tw">
      <div className="atl-tw-bar">
        <div className="atl-search">
          <Search size={13} className="icon" />
          <Input
            ref={inputRef}
            small
            value={text}
            placeholder="Search pages (text ~)"
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => e.key === 'Escape' && setText('')}
          />
          {text && <IconButton className="clear" size="small" icon={X} label="Clear search" onClick={() => setText('')} />}
        </div>
        <IconButton size="small" icon={Archive} label={archived ? 'Hide archived pages' : 'Show archived pages'} active={archived} onClick={() => prefs.setArchived(!prefs.archived)} />
        <IconButton size="small" icon={RefreshCw} label="Refresh" onClick={refresh} />
        <IconButton
          size="small"
          icon={FilePlus}
          label="New page…"
          onClick={() => useAtlassianUi.getState().openNewPage({ spaceId: space?.id })}
        />
      </div>
      <div className="atl-tw-bar">
        {spacesQ.error ? (
          <span className="wb-danger wb-small wb-ellipsis">{(spacesQ.error as Error).message}</span>
        ) : (
          <Select value={space?.id ?? ''} onChange={(e) => prefs.setSpace(projectId, e.target.value)} aria-label="Space">
            {!spaces.length && <option value="">Loading spaces…</option>}
            {spaces.map((s) => (
              <option key={s.id} value={s.id}>
                {s.name}
                {s.type === 'personal' ? ' (personal)' : ` · ${s.key}`}
              </option>
            ))}
          </Select>
        )}
        {hasProjectRoots && (
          <IconButton
            size="small"
            icon={Layers}
            active={wholeSpace}
            label={wholeSpace ? "Show only this project's pages" : 'Show the whole space'}
            onClick={() => prefs.setWholeSpace(projectId, !wholeSpace)}
          />
        )}
        {searching && (
          <IconButton
            size="small"
            icon={Search}
            active={allSpaces}
            label={allSpaces ? 'Searching all spaces' : `Searching ${space?.key ?? 'this space'} only`}
            onClick={() => setAllSpaces(!allSpaces)}
          />
        )}
      </div>
      {searching ? (
        <SearchResults projectId={projectId} q={q} space={allSpaces ? '' : space?.key ?? ''} archived={archived} />
      ) : (
        <div className="wb-scroll">
          {links.pinned.length > 0 && (
            <Section title="Pinned" count={links.pinned.length}>
              <PageList
                projectId={projectId}
                ids={links.pinned.map((p) => p.id)}
                labels={Object.fromEntries(links.pinned.map((p) => [p.id, p.name]))}
                archived={archived}
              />
            </Section>
          )}
          <Section title="Recent" defaultOpen={false}>
            <Recent projectId={projectId} />
          </Section>
          {hasProjectRoots && !wholeSpace ? (
            <Section title={project.data ? `${project.data.summary.name} pages` : 'Project pages'} actions={<Pin size={12} className="wb-subtle" />}>
              <PageList projectId={projectId} ids={links.rootPages} archived={archived} />
            </Section>
          ) : space ? (
            <Section title={space.name}>
              <SpaceTree key={space.id} projectId={projectId} space={space} archived={archived} />
            </Section>
          ) : spacesQ.isLoading ? (
            <Loading label="Loading spaces…" />
          ) : (
            <EmptyState title="No spaces">This account cannot see any Confluence spaces.</EmptyState>
          )}
        </div>
      )}
      <div className="atl-foot" title={st.user?.email ?? undefined}>
        <ConfluenceIcon size={11} />
        <span className="wb-ellipsis">{st.site.replace(/^https?:\/\//, '')}</span>
        {st.user && <span className="wb-ellipsis">· {st.user.displayName}</span>}
      </div>
    </div>
  )
}
