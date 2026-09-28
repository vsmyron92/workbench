// The TODO tool window (bottom, CLion's TODO view): TODO / FIXME / XXX / HACK
// comments of the project or the current file, grouped by file. A click opens the
// file at the comment. It rescans shortly after files change (agents editing).

import { useEffect, useMemo, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, FileCode, ListTodo, RefreshCw } from 'lucide-react'
import { useEvent } from '@/api/events'
import { EmptyState, ErrorBox, IconButton, Input, Loading, Spinner, Tabs } from '@/ui'
import { filesApi, type TodoItem } from './api'
import { openFile } from './openers'
import { useActiveEditor } from './store'

const KINDS: TodoItem['kind'][] = ['TODO', 'FIXME', 'XXX', 'HACK']
const RESCAN_DELAY = 1500

export function TodoToolWindow({ projectId }: { projectId: string | null }) {
  const q = useQuery({
    queryKey: ['files', projectId, 'todos'],
    queryFn: ({ signal }) => filesApi.todos(projectId!, signal),
    enabled: !!projectId,
    staleTime: 30_000,
  })
  const refetch = q.refetch
  // Files changed: rescan once things settle.
  const timer = useRef<number | undefined>(undefined)
  useEvent<{ paths?: string[] }>('fs.changed', (ev) => {
    if (ev.projectId !== projectId) return
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => void refetch(), RESCAN_DELAY)
  })
  useEffect(() => () => window.clearTimeout(timer.current), [])

  const active = useActiveEditor((s) => s.current)
  const currentPath = active && active.projectId === projectId ? active.path : null
  const [tab, setTab] = useState<'project' | 'file'>('project')
  const [hidden, setHidden] = useState<Set<TodoItem['kind']>>(new Set())
  const [filter, setFilter] = useState('')
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())

  const items = q.data?.items
  const scoped = useMemo(() => (items ?? []).filter((t) => tab === 'project' || t.path === currentPath), [items, tab, currentPath])
  const counts = useMemo(() => {
    const c: Record<string, number> = {}
    for (const t of scoped) c[t.kind] = (c[t.kind] ?? 0) + 1
    return c
  }, [scoped])
  const groups = useMemo(() => {
    const needle = filter.trim().toLowerCase()
    const byFile = new Map<string, TodoItem[]>()
    for (const t of scoped) {
      if (hidden.has(t.kind)) continue
      if (needle && !t.text.toLowerCase().includes(needle) && !t.path.toLowerCase().includes(needle)) continue
      const list = byFile.get(t.path)
      if (list) list.push(t)
      else byFile.set(t.path, [t])
    }
    return [...byFile.entries()]
  }, [scoped, hidden, filter])
  const shown = groups.reduce((n, [, l]) => n + l.length, 0)
  const fileCount = currentPath ? (items ?? []).filter((t) => t.path === currentPath).length : 0

  if (!projectId) return <EmptyState icon={ListTodo} title="No project" />
  if (q.error) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />

  const toggleKind = (k: TodoItem['kind']) =>
    setHidden((s) => {
      const n = new Set(s)
      if (n.has(k)) n.delete(k)
      else n.add(k)
      return n
    })

  return (
    <div className="wb-fill wb-todo">
      <div className="wb-todo-bar">
        <Tabs
          value={tab}
          onChange={setTab}
          tabs={[
            { id: 'project', label: 'Project', badge: items?.length ? <span className="wb-todo-count">{items.length}</span> : null },
            { id: 'file', label: 'Current File', badge: fileCount ? <span className="wb-todo-count">{fileCount}</span> : null },
          ]}
        />
        <div className="wb-todo-kinds" role="group" aria-label="Show">
          {KINDS.map((k) => (
            <button
              key={k}
              type="button"
              className={`wb-todo-kind k-${k.toLowerCase()}${hidden.has(k) ? '' : ' on'}`}
              aria-pressed={!hidden.has(k)}
              title={hidden.has(k) ? `Show ${k}` : `Hide ${k}`}
              onClick={() => toggleKind(k)}
            >
              {k}
              <span>{counts[k] ?? 0}</span>
            </button>
          ))}
        </div>
        <Input small className="wb-todo-filter" placeholder="Filter" value={filter} onChange={(e) => setFilter(e.target.value)} aria-label="Filter TODO items" />
        <span className="wb-grow" />
        <IconButton icon={ChevronRight} size="small" label="Collapse all" onClick={() => setCollapsed(new Set(groups.map(([p]) => p)))} />
        <IconButton icon={ChevronDown} size="small" label="Expand all" onClick={() => setCollapsed(new Set())} />
        <IconButton icon={RefreshCw} size="small" label="Rescan" onClick={() => void q.refetch()} />
      </div>
      <div className="wb-scroll wb-todo-list" role="tree">
        {!q.data && <Loading label="Scanning for TODO comments…" />}
        {q.data && !groups.length && (
          <EmptyState icon={ListTodo} title={tab === 'file' && !currentPath ? 'No file is focused' : scoped.length ? 'Nothing matches' : 'No TODO comments'}>
            {tab === 'file' && !currentPath
              ? 'Focus an editor to see its TODO comments.'
              : scoped.length
                ? 'Change the filter or the shown kinds.'
                : 'Comments with TODO, FIXME, XXX or HACK show up here.'}
          </EmptyState>
        )}
        {groups.map(([path, list]) => {
          const open = !collapsed.has(path)
          const slash = path.lastIndexOf('/')
          return (
            <div key={path} role="treeitem" aria-expanded={open}>
              <div
                className="wb-list-row wb-todo-file"
                onClick={() =>
                  setCollapsed((s) => {
                    const n = new Set(s)
                    if (open) n.add(path)
                    else n.delete(path)
                    return n
                  })
                }
              >
                {open ? <ChevronDown size={13} className="wb-subtle" /> : <ChevronRight size={13} className="wb-subtle" />}
                <FileCode size={13} className="wb-subtle" />
                <span className="wb-todo-name">{path.slice(slash + 1)}</span>
                <span className="wb-subtle wb-small wb-ellipsis">{path.slice(0, Math.max(0, slash))}</span>
                <span className="wb-subtle wb-small">
                  {list.length} item{list.length === 1 ? '' : 's'}
                </span>
              </div>
              {open &&
                list.map((t) => (
                  <div
                    key={`${t.line}:${t.column}`}
                    className="wb-list-row wb-todo-row"
                    title={t.text}
                    onClick={() => openFile({ projectId, path: t.path, line: t.line, column: t.column, endColumn: t.endColumn })}
                  >
                    <span className={`wb-todo-badge k-${t.kind.toLowerCase()}`}>{t.kind}</span>
                    <span className="wb-todo-text wb-ellipsis">{t.text.slice(t.kind.length).replace(/^[\s:\-–—]+/, '') || t.text}</span>
                    <span className="wb-todo-pos">:{t.line}</span>
                  </div>
                ))}
            </div>
          )
        })}
        {q.data?.truncated && <div className="wb-todo-note">Only the first 5000 items are listed.</div>}
        {q.data && (
          <div className="wb-todo-note">
            {shown} of {scoped.length} shown · {q.data.filesSearched} files scanned in {q.data.elapsedMs} ms
          </div>
        )}
      </div>
      {q.isFetching && q.data && (
        <span className="wb-todo-busy">
          <Spinner size={10} />
        </span>
      )}
    </div>
  )
}
