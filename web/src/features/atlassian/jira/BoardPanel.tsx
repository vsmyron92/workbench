// The `jira.board` panel: a Jira Software board. Columns come from the board's
// configuration (each maps statuses); cards are the issues of the chosen sprint
// (scrum), of the whole board (kanban) or of the backlog. Dragging a card to another
// column runs the workflow transition that lands in that column (a menu when several
// do), optimistically. Quick filters and a text filter narrow the cards.

import { useEffect, useMemo, useState, type DragEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ExternalLink, Filter, Kanban, ListTodo, RefreshCw, Search, X } from 'lucide-react'
import { ApiError } from '@/api/client'
import { toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { useUi } from '@/state/store'
import { Badge, EmptyState, ErrorBox, IconButton, Input, JiraIcon, Loading, Select, showMenu, Spacer, Spinner, TimeAgo, Toolbar } from '@/ui'
import {
  jiraApi,
  qk,
  useBoard,
  useSprints,
  type BoardColumn,
  type BoardDetail,
  type BoardIssues,
  type BoardScope,
  type IssueSummary,
  type JiraTransition,
  type Sprint,
} from '../api'
import { initials } from '../links'
import { usePrefs } from '../state'
import { openJiraIssue } from '../confluence/actions'
import { SetupHint } from '../confluence/ToolWindow'
import { groupByColumn, limitState, moveIssue, transitionsInto } from './board'
import { IssueTypeIcon, StatusBadge } from './common'

export interface BoardParams {
  boardId: number
  projectId?: string
}

const DRAG_TYPE = 'application/x-workbench-issue'

/**
 * The saved choice ('active', 'backlog', 'board' or a sprint id) as a scope: `none`
 * when the board has no active sprint, `loading` until its sprints are known.
 */
function scopeOf(choice: string, board: BoardDetail, sprints: Sprint[] | undefined): BoardScope | 'none' | 'loading' {
  if (!board.hasSprints || choice === 'board') return { kind: 'board' }
  if (choice === 'backlog') return { kind: 'backlog' }
  if (choice === 'active' || !choice) {
    if (!sprints) return 'loading'
    const s = sprints.find((x) => x.state === 'active')
    return s ? { kind: 'sprint', sprintId: s.id } : 'none'
  }
  const id = Number(choice)
  return Number.isFinite(id) && id > 0 ? { kind: 'sprint', sprintId: id } : { kind: 'board' }
}

const scopeKey = (s: BoardScope) => (s.kind === 'sprint' ? `sprint:${s.sprintId}` : s.kind)

function Card({ issue, onOpen, busy }: { issue: IssueSummary; onOpen: () => void; busy: boolean }) {
  return (
    <div
      className={['jira-card', busy && 'busy', issue.status?.category === 'done' && 'done'].filter(Boolean).join(' ')}
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData(DRAG_TYPE, issue.key)
        e.dataTransfer.setData('text/plain', issue.key)
        e.dataTransfer.effectAllowed = 'move'
      }}
      onClick={onOpen}
      title={`${issue.key} ${issue.summary}`}
    >
      <div className="sum">{issue.summary}</div>
      <div className="foot">
        <IssueTypeIcon type={issue.issueType} size={13} />
        <span className="key">{issue.key}</span>
        {issue.priority && <span className="wb-xs wb-subtle">{issue.priority.name}</span>}
        <Spacer />
        {busy && <Spinner size={10} />}
        <span className="atl-avatar" title={issue.assignee?.displayName ?? 'Unassigned'} style={issue.assignee ? undefined : { opacity: 0.4 }}>
          {issue.assignee ? initials(issue.assignee.displayName) : '–'}
        </span>
      </div>
    </div>
  )
}

function Column({
  column,
  issues,
  onDrop,
  moving,
}: {
  column: BoardColumn | null
  issues: IssueSummary[]
  onDrop?: (key: string, e: DragEvent) => void
  moving: string | null
}) {
  const [over, setOver] = useState(false)
  const limit = column ? limitState(column, issues.length) : null
  return (
    <div
      className={['jira-col', over && 'over', !column && 'other'].filter(Boolean).join(' ')}
      onDragOver={(e) => {
        if (!onDrop || ![...e.dataTransfer.types].includes(DRAG_TYPE)) return
        e.preventDefault()
        e.dataTransfer.dropEffect = 'move'
        setOver(true)
      }}
      onDragLeave={(e) => !e.currentTarget.contains(e.relatedTarget as Node) && setOver(false)}
      onDrop={(e) => {
        setOver(false)
        const key = e.dataTransfer.getData(DRAG_TYPE)
        if (key && onDrop) {
          e.preventDefault()
          onDrop(key, e)
        }
      }}
    >
      <div className="jira-col-head">
        <span className="name wb-ellipsis">{column ? column.name : 'Other statuses'}</span>
        <span className={limit ? 'count limit' : 'count'} title={limit ? (limit === 'over' ? `Over the limit of ${column?.max}` : `Under the minimum of ${column?.min}`) : undefined}>
          {issues.length}
          {column?.max ? ` / ${column.max}` : ''}
        </span>
      </div>
      <div className="jira-col-body">
        {issues.map((i) => (
          <Card key={i.key} issue={i} busy={moving === i.key} onOpen={() => openJiraIssue(i.key, i.summary)} />
        ))}
        {!issues.length && <div className="jira-col-empty">{column ? 'No issues' : ''}</div>}
      </div>
    </div>
  )
}

export function BoardPanel({ params, setTitle }: PanelProps<BoardParams>) {
  const qc = useQueryClient()
  const uiProject = useUi((s) => s.projectId)
  const projectId = params.projectId ?? uiProject
  const boardId = Number(params.boardId)
  const board = useBoard(projectId, boardId)
  const sprints = useSprints(projectId, boardId, !!board.data?.hasSprints)
  const prefKey = `${projectId ?? ''}|${boardId}`
  const choice = usePrefs((s) => s.boardScope[prefKey] ?? 'active')
  const setChoice = (v: string) => usePrefs.getState().setBoardScope(prefKey, v)
  const [filters, setFilters] = useState<number[]>([])
  const [text, setText] = useState('')
  const [moving, setMoving] = useState<string | null>(null)

  useEffect(() => {
    if (board.data) setTitle(board.data.name)
  }, [board.data?.name]) // eslint-disable-line react-hooks/exhaustive-deps

  const resolved = board.data ? scopeOf(choice, board.data, sprints.isError ? [] : sprints.data) : 'loading'
  const scope = typeof resolved === 'object' ? resolved : null
  const jql = useMemo(
    () =>
      (board.data?.quickFilters ?? [])
        .filter((f) => filters.includes(f.id))
        .map((f) => `(${f.jql})`)
        .join(' AND '),
    [board.data, filters],
  )
  const issuesKey = scope ? qk.boardIssues(projectId, boardId, scopeKey(scope), jql) : null
  const issues = useQuery({
    queryKey: issuesKey ?? ['jira', 'boardIssues', 'none'],
    queryFn: () => jiraApi.boardIssues(projectId, boardId, scope!, jql),
    enabled: !!scope,
    staleTime: 30_000,
    retry: false,
  })

  if (!(boardId > 0)) return <EmptyState icon={JiraIcon} title="Not a Jira board" />
  if (board.isLoading) return <Loading label="Loading board…" />
  if (board.error) {
    if (board.error instanceof ApiError && board.error.notConfigured) return <SetupHint error={board.error} />
    return <ErrorBox error={board.error} onRetry={() => board.refetch()} />
  }
  const b = board.data!

  const refresh = () => {
    qc.invalidateQueries({ queryKey: qk.board(projectId, boardId) })
    qc.invalidateQueries({ queryKey: qk.sprints(projectId, boardId) })
    qc.invalidateQueries({ predicate: (q) => (q.queryKey as unknown[])[1] === 'boardIssues' && (q.queryKey as unknown[])[3] === boardId })
  }

  const all = issues.data?.issues ?? []
  const needle = text.trim().toLowerCase()
  const shown = needle ? all.filter((i) => i.key.toLowerCase().includes(needle) || i.summary.toLowerCase().includes(needle)) : all
  const grouped = groupByColumn(b.columns, shown)
  const sprint = scope?.kind === 'sprint' ? sprints.data?.find((s) => s.id === scope.sprintId) : undefined

  const drop = async (key: string, column: BoardColumn, e: DragEvent) => {
    const issue = all.find((i) => i.key === key)
    if (!issue || !issuesKey || column.statusIds.includes(issue.status?.id ?? '')) return
    setMoving(key)
    let ts: JiraTransition[]
    try {
      ts = transitionsInto(await jiraApi.transitions(projectId, key), column)
    } catch (err) {
      setMoving(null)
      return toastError(err, key)
    }
    const run = async (t: JiraTransition) => {
      const before = qc.getQueryData<BoardIssues>(issuesKey)
      if (before) qc.setQueryData<BoardIssues>(issuesKey, { ...before, issues: moveIssue(before.issues, key, column, t.to?.name) })
      setMoving(key)
      try {
        if (t.hasScreen) toast('info', `“${t.name}” may ask for more fields in Jira; if it fails, finish it there`)
        await jiraApi.transition(projectId, key, t.id)
        toast('success', `${key} → ${t.to?.name ?? column.name}`)
      } catch (err) {
        if (before) qc.setQueryData(issuesKey, before)
        toastError(err, `${key} could not move to ${column.name}`)
      } finally {
        setMoving(null)
        void qc.invalidateQueries({ queryKey: issuesKey })
        void qc.invalidateQueries({ queryKey: qk.issue(projectId, key) })
      }
    }
    setMoving(null)
    if (!ts.length) return toast('warning', `${key} has no transition from “${issue.status?.name ?? '?'}” into “${column.name}” (the workflow does not allow it)`)
    if (ts.length === 1) return void run(ts[0])
    showMenu(
      { clientX: e.clientX, clientY: e.clientY },
      ts.map((t) => ({ label: t.to && t.to.name !== t.name ? `${t.name} → ${t.to.name}` : t.name, run: () => void run(t) })),
    )
  }

  const bySprintState = (state: string) => (sprints.data ?? []).filter((s) => s.state === state)
  const days = sprint?.endDate ? Math.ceil((Date.parse(sprint.endDate) - Date.now()) / 86_400_000) : null

  return (
    <div className="cf-page">
      <Toolbar>
        <Kanban size={14} className="wb-muted" />
        <span className="title wb-ellipsis">{b.name}</span>
        <Badge>{b.type}</Badge>
        {b.projectKey && <span className="wb-xs wb-subtle">{b.projectKey}</span>}
        {b.hasSprints && (
          <Select value={choice} onChange={(e) => setChoice(e.target.value)} aria-label="Sprint" style={{ maxWidth: 240 }}>
            <option value="active">Active sprint</option>
            {bySprintState('active').length > 1 &&
              bySprintState('active').map((s) => (
                <option key={s.id} value={String(s.id)}>
                  {s.name} (active)
                </option>
              ))}
            {bySprintState('future').length > 0 && (
              <optgroup label="Future sprints">
                {bySprintState('future').map((s) => (
                  <option key={s.id} value={String(s.id)}>
                    {s.name}
                  </option>
                ))}
              </optgroup>
            )}
            {bySprintState('closed').length > 0 && (
              <optgroup label="Closed sprints">
                {bySprintState('closed')
                  .slice()
                  .reverse()
                  .map((s) => (
                    <option key={s.id} value={String(s.id)}>
                      {s.name}
                    </option>
                  ))}
              </optgroup>
            )}
            <option value="backlog">Backlog</option>
            <option value="board">All board issues</option>
          </Select>
        )}
        <Spacer />
        <div className="atl-search" style={{ width: 200 }}>
          <Search size={13} className="icon" />
          <Input small value={text} placeholder="Filter cards" onChange={(e) => setText(e.target.value)} onKeyDown={(e) => e.key === 'Escape' && setText('')} aria-label="Filter cards" />
          {text && <IconButton className="clear" size="small" icon={X} label="Clear filter" onClick={() => setText('')} />}
        </div>
        {issues.isFetching && <Spinner size={11} />}
        <IconButton icon={RefreshCw} label="Refresh" onClick={refresh} />
        <IconButton icon={ExternalLink} label="Open in Jira" onClick={() => window.open(b.webUrl, '_blank', 'noopener,noreferrer')} />
      </Toolbar>
      {(b.quickFilters.length > 0 || sprint) && (
        <div className="jira-board-bar">
          {sprint && (
            <span className="wb-small wb-ellipsis" title={sprint.goal ?? undefined}>
              <strong>{sprint.name}</strong>
              {sprint.goal ? ` · ${sprint.goal}` : ''}
              {sprint.state === 'active' && days !== null ? ` · ${days >= 0 ? `${days} day${days === 1 ? '' : 's'} left` : 'overdue'}` : ''}
              {sprint.state === 'closed' && sprint.completeDate ? (
                <>
                  {' · closed '}
                  <TimeAgo time={sprint.completeDate} />
                </>
              ) : null}
            </span>
          )}
          <Spacer />
          {b.quickFilters.length > 0 && <Filter size={12} className="wb-subtle" />}
          {b.quickFilters.map((f) => (
            <button
              key={f.id}
              className={filters.includes(f.id) ? 'jira-chip on' : 'jira-chip'}
              title={f.description ?? f.jql}
              onClick={() => setFilters((l) => (l.includes(f.id) ? l.filter((x) => x !== f.id) : [...l, f.id]))}
            >
              {f.name}
            </button>
          ))}
        </div>
      )}
      {resolved === 'none' ? (
        <EmptyState icon={Kanban} title="No active sprint">
          Pick a future or closed sprint, or the backlog, above.
        </EmptyState>
      ) : !scope || issues.isLoading ? (
        <Loading label="Loading issues…" />
      ) : issues.error ? (
        <ErrorBox error={issues.error} onRetry={() => issues.refetch()} />
      ) : scope.kind === 'backlog' ? (
        <div className="wb-scroll">
          {shown.length === 0 && <EmptyState icon={ListTodo} title="The backlog is empty" />}
          {shown.map((i) => (
            <div key={i.key} className="jira-row" onClick={() => openJiraIssue(i.key, i.summary)} title={`${i.key} ${i.summary}`}>
              <IssueTypeIcon type={i.issueType} />
              <span className="key">{i.key}</span>
              <span className="sum">{i.summary}</span>
              <StatusBadge status={i.status} />
              <span className="atl-avatar" title={i.assignee?.displayName ?? 'Unassigned'} style={i.assignee ? undefined : { opacity: 0.4 }}>
                {i.assignee ? initials(i.assignee.displayName) : '–'}
              </span>
            </div>
          ))}
        </div>
      ) : (
        <div className="jira-board">
          {b.columns.map((c, n) => (
            <Column key={c.name + n} column={c} issues={grouped.columns[n]} moving={moving} onDrop={(key, e) => void drop(key, c, e)} />
          ))}
          {grouped.unmapped.length > 0 && <Column column={null} issues={grouped.unmapped} moving={moving} />}
        </div>
      )}
      {issues.data?.truncated && (
        <div className="cf-banner">
          Showing the first {issues.data.issues.length} of {issues.data.total} issues; filter to narrow them.
        </div>
      )}
    </div>
  )
}
