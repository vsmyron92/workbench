// The Jira tool window. Issues: saved filters (assigned to me, the project's issues
// from [links.jira]) and a JQL box; results page with Jira's nextPageToken. Boards:
// the Jira Software boards (the project's first), each opening a board panel.

import { useEffect, useMemo, useRef, useState } from 'react'
import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query'
import { ExternalLink, Kanban, Plus, RefreshCw, Search, X } from 'lucide-react'
import { useProject } from '@/api/queries'
import { Badge, Button, EmptyState, ErrorBox, IconButton, Input, JiraIcon, Loading, Select, showMenu, Tabs, TimeAgo } from '@/ui'
import { jiraApi, needsSetup, qk, refreshAtlassianStatus, useAtlassianStatusQuery, useBoards, type Board, type IssueSummary } from '../api'
import { initials, jqlFilters } from '../links'
import { useAtlassianUi, usePrefs } from '../state'
import { openJiraBoard, openJiraIssue } from '../confluence/actions'
import { SetupHint } from '../confluence/ToolWindow'
import { IssueTypeIcon, StatusBadge } from './common'

function Results({ projectId, jql }: { projectId: string | null; jql: string }) {
  const q = useInfiniteQuery({
    queryKey: qk.jiraSearch(projectId, jql),
    queryFn: ({ pageParam }) => jiraApi.search(projectId, jql, pageParam),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => (last.isLast ? null : last.nextPageToken),
    retry: false,
    staleTime: 30_000,
  })
  if (q.isLoading) return <Loading label="Searching…" />
  if (q.error) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  const issues: IssueSummary[] = q.data?.pages.flatMap((p) => p.issues) ?? []
  if (!issues.length) return <EmptyState icon={Search} title="No issues match" />
  return (
    <div className="wb-scroll">
      {issues.map((i) => (
        <div
          key={i.key}
          className={['jira-row', i.status?.category === 'done' && 'done'].filter(Boolean).join(' ')}
          onClick={() => openJiraIssue(i.key, i.summary)}
          onContextMenu={(e) =>
            showMenu(e, [
              { label: 'Open', run: () => openJiraIssue(i.key, i.summary) },
              { label: 'Copy key', run: () => void navigator.clipboard?.writeText(i.key) },
            ])
          }
          title={`${i.key} ${i.summary}${i.updated ? ` · updated ${new Date(i.updated).toLocaleString()}` : ''}`}
        >
          <IssueTypeIcon type={i.issueType} />
          <span className="key">{i.key}</span>
          <span className="sum">{i.summary}</span>
          <StatusBadge status={i.status} />
          <span className="atl-avatar" title={i.assignee?.displayName ?? 'Unassigned'} style={i.assignee ? undefined : { opacity: 0.4 }}>
            {i.assignee ? initials(i.assignee.displayName) : '–'}
          </span>
        </div>
      ))}
      {q.hasNextPage && (
        <div className="wb-pad">
          <Button size="small" loading={q.isFetchingNextPage} onClick={() => q.fetchNextPage()}>
            More issues
          </Button>
        </div>
      )}
      <div className="wb-pad wb-xs wb-subtle">
        {issues.length} issue{issues.length === 1 ? '' : 's'}
        {q.hasNextPage ? ' (more available)' : ''} · updated <TimeAgo time={q.dataUpdatedAt} />
      </div>
    </div>
  )
}

/** Boards: the project's own (by [links.jira] project keys) first, then by name. */
export function sortBoards(boards: Board[], projectKeys: string[]): Board[] {
  const mine = new Set(projectKeys.map((k) => k.toUpperCase()))
  return [...boards].sort((a, b) => {
    const pa = a.projectKey && mine.has(a.projectKey) ? 0 : 1
    const pb = b.projectKey && mine.has(b.projectKey) ? 0 : 1
    return pa - pb || a.name.localeCompare(b.name)
  })
}

function Boards({ projectId, projectKeys }: { projectId: string | null; projectKeys: string[] }) {
  const q = useBoards(projectId)
  const [text, setText] = useState('')
  if (q.isLoading) return <Loading label="Loading boards…" />
  if (q.error) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  const needle = text.trim().toLowerCase()
  const boards = sortBoards(q.data?.boards ?? [], projectKeys).filter(
    (b) => !needle || b.name.toLowerCase().includes(needle) || (b.projectKey ?? '').toLowerCase().includes(needle),
  )
  return (
    <>
      <div className="atl-tw-bar">
        <div className="atl-search">
          <Search size={13} className="icon" />
          <Input small value={text} placeholder="Filter boards" onChange={(e) => setText(e.target.value)} onKeyDown={(e) => e.key === 'Escape' && setText('')} />
          {text && <IconButton className="clear" size="small" icon={X} label="Clear filter" onClick={() => setText('')} />}
        </div>
      </div>
      <div className="wb-scroll">
        {!boards.length && (
          <EmptyState icon={Kanban} title={needle ? 'No boards match' : 'No boards'}>
            {needle ? null : 'This account sees no Jira Software boards.'}
          </EmptyState>
        )}
        {boards.map((b) => (
          <div key={b.id} className="jira-row board" onClick={() => openJiraBoard(b.id, b.name)} title={b.projectName ?? b.name}>
            <Kanban size={14} className="wb-muted" />
            <span className="sum">{b.name}</span>
            {b.projectKey && <span className="key">{b.projectKey}</span>}
            <Badge>{b.type}</Badge>
          </div>
        ))}
        {q.data?.truncated && <div className="wb-pad wb-xs wb-subtle">Showing the first 500 boards; filter by name.</div>}
      </div>
    </>
  )
}

export function JiraToolWindow({ projectId }: { projectId: string | null }) {
  const qc = useQueryClient()
  const status = useAtlassianStatusQuery(projectId)
  const project = useProject(projectId)
  const filters = useMemo(() => jqlFilters(project.data?.config), [project.data])
  const prefs = usePrefs()
  const key = projectId ?? ''
  const filterId = prefs.jiraFilter[key] ?? filters[0].id
  const savedJql = prefs.jiraJql[key]
  const active = filters.find((f) => f.id === filterId)
  const jql = filterId === 'custom' ? savedJql ?? filters[0].jql : active?.jql ?? filters[0].jql
  const [draft, setDraft] = useState(jql)
  const inputRef = useRef<HTMLInputElement>(null)
  const focusJql = useAtlassianUi((s) => s.focusJql)
  const mode = usePrefs((s) => s.jiraMode)
  const projectKeys = project.data?.config?.links?.jira?.project_keys ?? []

  useEffect(() => setDraft(jql), [jql])
  useEffect(() => {
    if (focusJql) {
      usePrefs.getState().setJiraMode('issues')
      window.setTimeout(() => inputRef.current?.focus(), 30)
    }
  }, [focusJql])

  const recheck = () => void refreshAtlassianStatus(qc, projectId)
  if (status.isLoading) return <Loading label="Connecting to Atlassian…" />
  if (status.error) return <SetupHint error={status.error} onRetry={recheck} />
  const st = status.data!
  if (needsSetup(st)) return <SetupHint message={st.error} onRetry={recheck} />
  if (!st.jira)
    return (
      <EmptyState icon={JiraIcon} title="No Jira on this site">
        {st.site.replace(/^https?:\/\//, '')} has no Jira (or this account cannot see it). Jira appears here automatically when it is available.
      </EmptyState>
    )

  const run = () => {
    const v = draft.trim()
    if (!v) return
    prefs.setJira(projectId, 'custom', v)
    qc.invalidateQueries({ queryKey: qk.jiraSearch(projectId, v) })
  }

  const modeTabs = (
    <div className="atl-tw-bar">
      <Tabs
        tabs={[
          { id: 'issues', label: 'Issues' },
          { id: 'boards', label: 'Boards' },
        ]}
        value={mode}
        onChange={(m) => usePrefs.getState().setJiraMode(m)}
      />
    </div>
  )
  const foot = (
    <div className="atl-foot">
      <JiraIcon size={11} />
      <span className="wb-ellipsis">{st.jiraTitle ?? st.site.replace(/^https?:\/\//, '')}</span>
      {st.user && <span className="wb-ellipsis">· {st.user.displayName}</span>}
    </div>
  )
  if (mode === 'boards')
    return (
      <div className="atl-tw">
        {modeTabs}
        <Boards projectId={projectId} projectKeys={projectKeys} />
        {foot}
      </div>
    )

  return (
    <div className="atl-tw">
      {modeTabs}
      <div className="atl-tw-bar">
        <Select
          value={filterId}
          onChange={(e) => {
            const f = filters.find((x) => x.id === e.target.value)
            prefs.setJira(projectId, e.target.value, f?.jql ?? draft)
          }}
          aria-label="Saved filter"
        >
          {filters.map((f) => (
            <option key={f.id} value={f.id}>
              {f.label}
            </option>
          ))}
          <option value="custom">Custom JQL</option>
        </Select>
        <IconButton size="small" icon={RefreshCw} label="Refresh" onClick={() => qc.invalidateQueries({ queryKey: ['jira'] })} />
        <IconButton size="small" icon={Plus} label="Create issue…" onClick={() => useAtlassianUi.getState().openCreateIssue({})} />
        <IconButton size="small" icon={ExternalLink} label="Open in Jira" onClick={() => window.open(`${st.site}/issues/?jql=${encodeURIComponent(jql)}`, '_blank', 'noopener,noreferrer')} />
      </div>
      <div className="atl-tw-bar">
        <div className="atl-search">
          <Search size={13} className="icon" />
          <Input
            ref={inputRef}
            small
            className="mono"
            value={draft}
            spellCheck={false}
            placeholder="JQL, e.g. project = ABC AND statusCategory != Done"
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') run()
              if (e.key === 'Escape') setDraft(jql)
            }}
            title="Press Enter to run"
          />
        </div>
      </div>
      <Results key={jql} projectId={projectId} jql={jql} />
      {foot}
    </div>
  )
}
