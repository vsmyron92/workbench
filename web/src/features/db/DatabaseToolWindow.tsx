// The Database tool window (right; CLion's Database view): the project's data
// sources, and under each its schemas, tables and views, and their columns. Double-
// click a table for its first rows in the source's console.

import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, Columns3, Copy, Database, Eye, FolderTree, KeyRound, Pencil, Plus, RefreshCw, SquareTerminal, Table2, Trash2, Zap } from 'lucide-react'
import { confirmDialog, openPanel, toast, toastError } from '@/shell/actions'
import { Badge, EmptyState, ErrorBox, IconButton, Loading, showMenu, Spinner, type MenuEntry } from '@/ui'
import { consolePanelId, type ConsoleParams } from './ConsolePanel'
import { dbApi, dbKeys, useCatalog, useDbSources, useTableInfo, type DbSource, type Relation } from './api'
import { newConsoleId, qualified, sourceLabel } from './logic'
import { useSourceDialog } from './SourceDialog'

/** Open (or focus) a console on a source; `sql` is added and run. */
export function openConsole(projectId: string, source: string, consoleId = 'main', sql?: string) {
  const params: ConsoleParams = { projectId, source, consoleId, request: sql ? { sql, t: Date.now() } : null }
  openPanel({ kind: 'db.console', id: consolePanelId(projectId, source, consoleId), title: `${source} · console`, params: params as unknown as Record<string, unknown> })
}

async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    toast('success', 'Copied')
  } catch {
    toast('warning', 'The browser refused the clipboard')
  }
}

async function test(pid: string, name: string) {
  try {
    const t = await dbApi.test(pid, name)
    toast('success', `${name}: connected in ${t.ms} ms`, { detail: `${t.version.split(',')[0]} · ${t.display} · as ${t.user}${t.ssl ? ' · TLS' : ' · no TLS'}` })
  } catch (e) {
    toastError(e, `${name} did not connect`)
  }
}

const KIND_ICON = { table: Table2, partitioned: Table2, foreign: Table2, view: Eye, matview: Eye }

export function DatabaseToolWindow({ projectId }: { projectId: string | null }) {
  const qc = useQueryClient()
  const q = useDbSources(projectId)
  const show = useSourceDialog((s) => s.show)
  const [open, setOpen] = useState<Set<string>>(new Set())
  const [sel, setSel] = useState<string | null>(null)
  if (!projectId) return <EmptyState icon={Database} title="No project selected" />
  const toggle = (k: string) =>
    setOpen((s) => {
      const n = new Set(s)
      if (n.has(k)) n.delete(k)
      else n.add(k)
      return n
    })
  const sources = q.data?.sources ?? []
  const selectedSource = sel ? sources.find((s) => sel === `s:${s.name}` || sel.startsWith(`${s.name}/`)) : sources[0]
  return (
    <div className="wb-fill wb-db">
      <div className="wb-db-toolbar">
        <IconButton icon={Plus} size="small" label="Add Data Source…" onClick={() => show(projectId, null)} />
        <IconButton icon={SquareTerminal} size="small" label="New Query Console" disabled={!selectedSource} onClick={() => selectedSource && openConsole(projectId, selectedSource.name, newConsoleId())} />
        <IconButton
          icon={RefreshCw}
          size="small"
          label="Refresh"
          onClick={() => {
            void qc.invalidateQueries({ queryKey: dbKeys.sources(projectId) })
            void qc.invalidateQueries({ queryKey: ['db', projectId, 'catalog'] })
            void qc.invalidateQueries({ queryKey: ['db', projectId, 'table'] })
          }}
        />
        <IconButton icon={FolderTree} size="small" label="Collapse All" onClick={() => setOpen(new Set())} />
      </div>
      {q.error ? (
        <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
      ) : !q.data ? (
        <Loading />
      ) : !sources.length ? (
        <EmptyState icon={Database} title="No data sources" action={<button className="wb-db-link" onClick={() => show(projectId, null)}>Add a PostgreSQL data source</button>}>
          Or define them as [[database]] entries in the project's config (the repository's .workbench.toml, with secret names only, or the machine overlay).
        </EmptyState>
      ) : (
        <div className="wb-scroll wb-db-tree" role="tree" aria-label="Data sources">
          {sources.map((s) => (
            <SourceNode key={s.name} pid={projectId} s={s} open={open} toggle={toggle} sel={sel} setSel={setSel} />
          ))}
        </div>
      )}
    </div>
  )
}

interface NodeProps {
  pid: string
  open: Set<string>
  toggle: (k: string) => void
  sel: string | null
  setSel: (k: string) => void
}

function Chevron({ open }: { open: boolean }) {
  return open ? <ChevronDown size={13} className="wb-subtle" /> : <ChevronRight size={13} className="wb-subtle" />
}

function SourceNode({ pid, s, open, toggle, sel, setSel }: NodeProps & { s: DbSource }) {
  const key = `s:${s.name}`
  const isOpen = open.has(key)
  const show = useSourceDialog((x) => x.show)
  const qc = useQueryClient()
  const menu = (e: React.MouseEvent) => {
    setSel(key)
    const items: MenuEntry[] = [
      { label: 'Open Console', icon: SquareTerminal, run: () => openConsole(pid, s.name) },
      { label: 'New Console', icon: SquareTerminal, run: () => openConsole(pid, s.name, newConsoleId()) },
      { label: 'Test Connection', icon: Zap, run: () => void test(pid, s.name) },
      { label: 'Refresh', icon: RefreshCw, run: () => void qc.invalidateQueries({ queryKey: dbKeys.catalog(pid, s.name) }) },
      'separator',
      { label: s.origin === 'repository' ? 'Edit (as a machine copy)…' : 'Edit…', icon: Pencil, run: () => show(pid, s) },
      {
        label: 'Remove…',
        icon: Trash2,
        danger: true,
        disabled: s.origin === 'repository',
        run: async () => {
          if (!(await confirmDialog({ title: `Remove ${s.name}?`, message: 'The [[database]] entry leaves the machine overlay. The database and its secrets are untouched.', confirmLabel: 'Remove', danger: true }))) return
          try {
            await dbApi.deleteSource(pid, s.name)
            void qc.invalidateQueries({ queryKey: dbKeys.sources(pid) })
          } catch (err) {
            toastError(err, `Could not remove ${s.name}`)
          }
        },
      },
    ]
    showMenu(e, items)
  }
  return (
    <div role="treeitem" aria-expanded={isOpen}>
      <div className={`wb-list-row wb-db-row${sel === key ? ' selected' : ''}`} onClick={() => (setSel(key), toggle(key))} onDoubleClick={() => openConsole(pid, s.name)} onContextMenu={menu} title={sourceLabel(s)}>
        <Chevron open={isOpen} />
        <Database size={13} />
        <span className="wb-db-name">{s.name}</span>
        <span className="wb-subtle wb-small wb-ellipsis">{sourceLabel(s)}</span>
        {s.readOnly && <Badge tone="warning">ro</Badge>}
        {s.origin === 'repository' && <Badge>repo</Badge>}
      </div>
      {isOpen && <SourceChildren pid={pid} s={s} open={open} toggle={toggle} sel={sel} setSel={setSel} />}
    </div>
  )
}

function SourceChildren({ pid, s, open, toggle, sel, setSel }: NodeProps & { s: DbSource }) {
  const q = useCatalog(pid, s.name, true)
  if (q.isLoading)
    return (
      <div className="wb-db-row wb-db-d1 wb-subtle wb-small">
        <Spinner size={11} /> Connecting…
      </div>
    )
  if (q.error)
    return (
      <div className="wb-db-d1">
        <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
      </div>
    )
  const c = q.data
  if (!c) return null
  return (
    <>
      {c.schemas.map((sc) => {
        const key = `${s.name}/${sc.name}`
        const isOpen = open.has(key)
        return (
          <div key={sc.name} role="treeitem" aria-expanded={isOpen}>
            <div className={`wb-list-row wb-db-row wb-db-d1${sel === key ? ' selected' : ''}`} onClick={() => (setSel(key), toggle(key))}>
              <Chevron open={isOpen} />
              <FolderTree size={13} className="wb-subtle" />
              <span className="wb-db-name">{sc.name}</span>
              <span className="wb-subtle wb-xs">{sc.relations.length}</span>
            </div>
            {isOpen &&
              sc.relations.map((r) => <RelationNode key={r.name} pid={pid} source={s.name} schema={sc.name} r={r} open={open} toggle={toggle} sel={sel} setSel={setSel} />)}
          </div>
        )
      })}
      {c.truncated && <div className="wb-db-row wb-db-d1 wb-subtle wb-small">Only the first 5000 tables and views are listed.</div>}
    </>
  )
}

function rowsLabel(n: number | null): string {
  if (n === null) return ''
  if (n >= 1_000_000) return `~${(n / 1_000_000).toFixed(1)}M rows`
  if (n >= 10_000) return `~${Math.round(n / 1000)}k rows`
  return `~${n} rows`
}

function RelationNode({ pid, source, schema, r, open, toggle, sel, setSel }: NodeProps & { source: string; schema: string; r: Relation }) {
  const key = `${source}/${schema}/${r.name}`
  const isOpen = open.has(key)
  const Icon = KIND_ICON[r.kind]
  const name = qualified(schema, r.name)
  const openData = () => openConsole(pid, source, 'main', `SELECT * FROM ${name} LIMIT 100`)
  return (
    <div role="treeitem" aria-expanded={isOpen}>
      <div
        className={`wb-list-row wb-db-row wb-db-d2${sel === key ? ' selected' : ''}`}
        onClick={() => (setSel(key), toggle(key))}
        onDoubleClick={openData}
        onContextMenu={(e) => {
          setSel(key)
          showMenu(e, [
            { label: 'Open Data (first 100 rows)', icon: Table2, run: openData },
            { label: 'Count Rows', icon: Columns3, run: () => openConsole(pid, source, 'main', `SELECT count(*) FROM ${name}`) },
            { label: 'Copy Qualified Name', icon: Copy, run: () => void copy(name) },
          ])
        }}
        title={r.comment ?? `${r.kind} ${name}${r.rows !== null ? ` · ${rowsLabel(r.rows)} (estimate)` : ''}\nDouble-click for its first rows`}
      >
        <Chevron open={isOpen} />
        <Icon size={13} className={r.kind === 'table' || r.kind === 'partitioned' ? undefined : 'wb-subtle'} />
        <span className="wb-db-name">{r.name}</span>
        {r.kind !== 'table' && <span className="wb-subtle wb-xs">{r.kind}</span>}
        <span className="wb-grow" />
        <span className="wb-subtle wb-xs">{rowsLabel(r.rows)}</span>
      </div>
      {isOpen && <Columns pid={pid} source={source} schema={schema} table={r.name} />}
    </div>
  )
}

function Columns({ pid, source, schema, table }: { pid: string; source: string; schema: string; table: string }) {
  const q = useTableInfo(pid, source, schema, table, true)
  if (q.isLoading)
    return (
      <div className="wb-db-row wb-db-d3 wb-subtle wb-small">
        <Spinner size={11} />
      </div>
    )
  if (q.error) return <div className="wb-db-row wb-db-d3 wb-danger wb-small">{q.error instanceof Error ? q.error.message : String(q.error)}</div>
  const t = q.data
  if (!t) return null
  return (
    <>
      {t.columns.map((c) => (
        <div key={c.name} className="wb-list-row wb-db-row wb-db-d3" title={[c.comment, c.default ? `default ${c.default}` : null].filter(Boolean).join('\n') || undefined}>
          {c.primaryKey ? <KeyRound size={12} className="wb-db-pk" /> : <Columns3 size={12} className="wb-subtle" />}
          <span className="wb-db-name">{c.name}</span>
          <span className="wb-subtle wb-xs wb-ellipsis">
            {c.dataType}
            {c.nullable ? '' : ' not null'}
          </span>
        </div>
      ))}
      {t.indexes.map((i) => (
        <div key={i.name} className="wb-list-row wb-db-row wb-db-d3 wb-subtle wb-xs" title={i.definition}>
          <span className="wb-db-idx">index</span> <span className="wb-ellipsis">{i.name}</span>
        </div>
      ))}
      {t.foreignKeys.map((f) => (
        <div key={f.name} className="wb-list-row wb-db-row wb-db-d3 wb-subtle wb-xs" title={f.definition}>
          <span className="wb-db-idx">fk</span> <span className="wb-ellipsis">{f.definition}</span>
        </div>
      ))}
    </>
  )
}
