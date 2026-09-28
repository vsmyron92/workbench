// The `db.console` panel: a SQL console on one data source (CLion's query console).
// Ctrl+Enter runs the statement at the caret (or the selection), Ctrl+Shift+Enter
// the whole text; results below, one tab per statement that returned rows. The
// console keeps its own server session (BEGIN / SET last across runs) and its text
// in this browser.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { editor } from 'monaco-editor'
import { useQueryClient } from '@tanstack/react-query'
import { Copy, Database, Play, PlaySquare, Square, TriangleAlert } from 'lucide-react'
import { toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { useUi } from '@/state/store'
import { Badge, Button, EmptyState, IconButton, MonacoEditor, Select, Spinner, Splitter, Tabs } from '@/ui'
import { dbApi, dbKeys, useDbSources, type QueryOutcome, type ResultSet } from './api'
import { numericColumn, positionToLineColumn, sourceLabel, statementAt, toTsv } from './logic'

type Monaco = typeof import('monaco-editor')

export interface ConsoleParams {
  projectId: string
  source: string
  consoleId: string
  /** SQL to add and run (Open Table), with a stamp so the same request runs again. */
  request?: { sql: string; t: number } | null
}

export function consolePanelId(projectId: string, source: string, consoleId: string) {
  return `db.console:${projectId}:${source}:${consoleId}`
}

const textKey = (p: ConsoleParams) => `wb.db.console.${p.projectId}.${p.source}.${p.consoleId}`
const MAX_ROW_CHOICES = [100, 500, 2000, 10000]

function loadText(p: ConsoleParams): string {
  try {
    return localStorage.getItem(textKey(p)) ?? ''
  } catch {
    return ''
  }
}

interface Run {
  sql: string
  /** Where the statement starts in the editor text (to place error markers). */
  offset: number
  at: number
  outcome?: QueryOutcome
  failed?: string
}

export function ConsolePanel({ params, setTitle }: PanelProps<ConsoleParams>) {
  const { projectId, source, consoleId } = params
  const qc = useQueryClient()
  const sources = useDbSources(projectId)
  const src = sources.data?.sources.find((s) => s.name === source) ?? null
  const theme = useUi((s) => s.prefs.theme)
  const fontSize = useUi((s) => s.prefs.editorFontSize)
  const [ed, setEd] = useState<{ e: editor.IStandaloneCodeEditor; monaco: Monaco } | null>(null)
  const [running, setRunning] = useState<number | null>(null)
  const [runs, setRuns] = useState<Run[]>([])
  const [maxRows, setMaxRows] = useState(500)
  const [split, setSplit] = useState(0.42)
  const [tab, setTab] = useState('0')
  const [now, setNow] = useState(Date.now())
  const box = useRef<HTMLDivElement>(null)
  const startSplit = useRef(split)
  const live = useRef({ maxRows, running })
  live.current = { maxRows, running }

  useEffect(() => setTitle(`${source} · console`), [source, setTitle])
  useEffect(() => {
    if (running === null) return
    const t = window.setInterval(() => setNow(Date.now()), 200)
    return () => window.clearInterval(t)
  }, [running])
  // The session ends with the panel.
  useEffect(() => () => void dbApi.close(projectId, source, consoleId).catch(() => {}), [projectId, source, consoleId])

  const last = runs[runs.length - 1]

  const execute = useCallback(
    async (sql: string, offset: number) => {
      if (!sql.trim() || live.current.running !== null) return
      const at = Date.now()
      setRunning(at)
      setNow(at)
      setRuns((r) => [...r.slice(-49), { sql, offset, at }])
      setTab('0')
      const model = ed?.e.getModel()
      if (model && ed) ed.monaco.editor.setModelMarkers(model, 'db', [])
      try {
        const outcome = await dbApi.query(projectId, source, sql, consoleId, live.current.maxRows)
        setRuns((r) => r.map((x) => (x.at === at ? { ...x, outcome } : x)))
        if (outcome.error?.position && model && ed) {
          const text = model.getValue()
          const { line, column } = positionToLineColumn(text, offset + outcome.error.position)
          ed.monaco.editor.setModelMarkers(model, 'db', [
            { severity: ed.monaco.MarkerSeverity.Error, message: outcome.error.message, startLineNumber: line, startColumn: column, endLineNumber: line, endColumn: column + 1 },
          ])
        }
        // DDL may have changed the tree.
        if (/^\s*(create|drop|alter|comment|truncate)\b/im.test(sql)) void qc.invalidateQueries({ queryKey: dbKeys.catalog(projectId, source) })
      } catch (e) {
        setRuns((r) => r.map((x) => (x.at === at ? { ...x, failed: e instanceof Error ? e.message : String(e) } : x)))
      } finally {
        setRunning(null)
      }
    },
    [projectId, source, consoleId, ed, qc],
  )

  const runAtCaret = useCallback(() => {
    if (!ed) return
    const model = ed.e.getModel()
    if (!model) return
    const sel = ed.e.getSelection()
    if (sel && !sel.isEmpty()) {
      const text = model.getValueInRange(sel)
      void execute(text, model.getOffsetAt(sel.getStartPosition()))
      return
    }
    const text = model.getValue()
    const pos = ed.e.getPosition()
    const st = statementAt(text, pos ? model.getOffsetAt(pos) : 0)
    if (!st) {
      toast('info', 'Nothing to run')
      return
    }
    // Show what runs.
    const a = model.getPositionAt(st.start)
    const b = model.getPositionAt(st.end)
    ed.e.setSelection(new ed.monaco.Selection(a.lineNumber, a.column, b.lineNumber, b.column))
    void execute(st.text, st.start)
  }, [ed, execute])

  const runAll = useCallback(() => {
    const model = ed?.e.getModel()
    if (model) void execute(model.getValue(), 0)
  }, [ed, execute])

  const cancel = async () => {
    try {
      await dbApi.cancel(projectId, source, consoleId)
    } catch (e) {
      toastError(e, 'Could not cancel')
    }
  }

  // Keys: added per editor (never global).
  const actions = useRef({ runAtCaret, runAll })
  actions.current = { runAtCaret, runAll }
  const paramsRef = useRef(params)
  paramsRef.current = params
  const onMount = useCallback((e: editor.IStandaloneCodeEditor, monaco: Monaco) => {
    e.addAction({ id: 'wb.db.run', label: 'Execute Statement', keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter], run: () => actions.current.runAtCaret() })
    e.addAction({ id: 'wb.db.runAll', label: 'Execute Script', keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.Enter], run: () => actions.current.runAll() })
    e.onDidChangeModelContent(() => {
      try {
        localStorage.setItem(textKey(paramsRef.current), e.getValue())
      } catch {
        /* storage full or blocked: the text lives in the panel */
      }
    })
    setEd({ e, monaco })
  }, [])

  // Open Table: add the statement at the end and run it.
  const handled = useRef<number | null>(null)
  useEffect(() => {
    const r = params.request
    if (!r || !ed || handled.current === r.t) return
    handled.current = r.t
    const model = ed.e.getModel()
    if (!model) return
    const text = model.getValue()
    const prefix = text.trim() ? (text.endsWith('\n') ? '\n' : '\n\n') : ''
    const start = text.length + prefix.length
    const endPos = model.getPositionAt(text.length)
    model.pushEditOperations([], [{ range: new ed.monaco.Range(endPos.lineNumber, endPos.column, endPos.lineNumber, endPos.column), text: `${prefix}${r.sql};\n` }], () => null)
    const a = model.getPositionAt(start)
    ed.e.setPosition(a)
    ed.e.revealLineInCenter(a.lineNumber)
    void execute(r.sql, start)
  }, [params.request, ed, execute])

  const onResize = (d: number) => {
    const h = box.current?.getBoundingClientRect().height ?? 1
    setSplit(Math.min(0.85, Math.max(0.12, startSplit.current + d / h)))
  }

  const outcome = last?.outcome
  const sets = useMemo(() => (outcome ? outcome.results.filter((r) => r.columns.length > 0) : []), [outcome])
  const current = sets[Number(tab)] ?? sets[0] ?? null
  const counts = outcome?.results.filter((r) => r.columns.length === 0 && r.rowsAffected !== null) ?? []

  if (sources.data && !src) {
    return (
      <EmptyState icon={Database} title={`No data source ${source}`}>
        It was removed or renamed; open a console from the Database tool window.
      </EmptyState>
    )
  }

  return (
    <div className="wb-fill wb-db-console" ref={box}>
      <div className="wb-db-bar">
        <Database size={14} className="wb-subtle" />
        <b className="wb-ellipsis">{source}</b>
        {src && <span className="wb-subtle wb-small wb-ellipsis">{sourceLabel(src)}</span>}
        {src?.readOnly && <Badge tone="warning">read-only</Badge>}
        <span className="wb-grow" />
        {running !== null ? (
          <>
            <span className="wb-small wb-subtle">
              <Spinner size={11} /> {((now - running) / 1000).toFixed(1)} s
            </span>
            <Button size="small" icon={Square} onClick={() => void cancel()}>
              Cancel
            </Button>
          </>
        ) : (
          <>
            <Button size="small" variant="primary" icon={Play} onClick={runAtCaret} title="Run the statement at the caret, or the selection (Ctrl+Enter)">
              Run
            </Button>
            <IconButton icon={PlaySquare} size="small" label="Run the whole console (Ctrl+Shift+Enter)" onClick={runAll} />
          </>
        )}
        <Select className="wb-db-max" value={String(maxRows)} onChange={(e) => setMaxRows(Number(e.target.value))} aria-label="Rows to fetch">
          {MAX_ROW_CHOICES.map((n) => (
            <option key={n} value={n}>
              {n} rows
            </option>
          ))}
        </Select>
      </div>
      <div className="wb-db-editor" style={{ flexBasis: `${split * 100}%` }}>
        <MonacoEditor
          theme={theme === 'dark' ? 'workbench-dark' : 'workbench-light'}
          language="sql"
          path={`inmemory://db-console/${projectId}/${source}/${consoleId}.sql`}
          defaultValue={loadText(params) || `-- ${source}: Ctrl+Enter runs the statement at the caret\n`}
          onMount={onMount}
          options={{ minimap: { enabled: false }, fontSize, scrollBeyondLastLine: false, automaticLayout: true, glyphMargin: false, wordWrap: 'on' }}
        />
      </div>
      <Splitter direction="h" onResizeStart={() => (startSplit.current = split)} onResize={onResize} />
      <div className="wb-db-results">
        {!last ? (
          <EmptyState icon={Play} title="Run a statement">
            Ctrl+Enter runs the statement at the caret (or the selection); Ctrl+Shift+Enter runs everything.
          </EmptyState>
        ) : last.failed ? (
          <div className="wb-db-error">
            <TriangleAlert size={14} /> {last.failed}
          </div>
        ) : !outcome ? (
          <div className="wb-db-note">
            <Spinner size={12} /> Running…
          </div>
        ) : (
          <>
            {outcome.error && (
              <div className="wb-db-error">
                <TriangleAlert size={14} />
                <div>
                  <b>{outcome.error.message}</b>
                  {outcome.error.code && <span className="wb-subtle"> ({outcome.error.code})</span>}
                  {outcome.error.detail && <div>{outcome.error.detail}</div>}
                  {outcome.error.hint && <div className="wb-subtle">Hint: {outcome.error.hint}</div>}
                </div>
              </div>
            )}
            {sets.length > 1 && (
              <Tabs
                value={String(sets.indexOf(current!))}
                onChange={setTab}
                tabs={sets.map((s, i) => ({ id: String(i), label: `Result ${i + 1}`, badge: <span className="wb-subtle wb-xs"> {s.rows.length}</span> }))}
              />
            )}
            {current && <Grid set={current} />}
            <div className="wb-db-foot">
              {current ? (
                <span>
                  {current.rows.length} row{current.rows.length === 1 ? '' : 's'}
                  {current.truncated ? ` (the first ${current.rows.length}; more exist)` : ''}
                </span>
              ) : (
                counts.length > 0 && <span>{counts.map((c) => `${c.rowsAffected} row${c.rowsAffected === 1 ? '' : 's'} affected`).join(' · ')}</span>
              )}
              {!current && !counts.length && !outcome.error && <span>Done</span>}
              <span className="wb-subtle">· {outcome.ms} ms</span>
              {outcome.notices.length > 0 && <span className="wb-db-notices">{outcome.notices.join(' · ')}</span>}
              <span className="wb-grow" />
              {current && current.rows.length > 0 && (
                <IconButton
                  icon={Copy}
                  size="small"
                  label="Copy all rows (tab-separated)"
                  onClick={() => void copyText(toTsv(current.columns, current.rows), `${current.rows.length} rows copied`)}
                />
              )}
            </div>
          </>
        )}
      </div>
    </div>
  )
}

async function copyText(text: string, what: string) {
  try {
    await navigator.clipboard.writeText(text)
    toast('success', what)
  } catch {
    toast('warning', 'The browser refused the clipboard')
  }
}

/** The result grid: row numbers, sticky header, NULL shown as such, numbers right-aligned; click selects a row, Ctrl+C copies it. */
function Grid({ set }: { set: ResultSet }) {
  const [sel, setSel] = useState<Set<number>>(new Set())
  const numeric = useMemo(() => set.columns.map((_, i) => numericColumn(set.rows, i)), [set])
  useEffect(() => setSel(new Set()), [set])
  const onKeyDown = (e: React.KeyboardEvent) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'c' && sel.size) {
      e.preventDefault()
      const rows = [...sel].sort((a, b) => a - b).map((i) => set.rows[i])
      void copyText(toTsv(set.columns, rows), `${rows.length} row${rows.length === 1 ? '' : 's'} copied`)
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') {
      e.preventDefault()
      setSel(new Set(set.rows.map((_, i) => i)))
    }
  }
  return (
    <div className="wb-scroll wb-db-grid" tabIndex={0} onKeyDown={onKeyDown}>
      <table>
        <thead>
          <tr>
            <th className="n" />
            {set.columns.map((c, i) => (
              <th key={i} className={numeric[i] ? 'num' : undefined}>
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {set.rows.map((r, i) => (
            <tr
              key={i}
              className={sel.has(i) ? 'sel' : undefined}
              onClick={(e) =>
                setSel((s) => {
                  if (e.shiftKey && s.size) {
                    const from = Math.min(...s)
                    const n = new Set<number>()
                    for (let k = Math.min(from, i); k <= Math.max(from, i); k++) n.add(k)
                    return n
                  }
                  if (e.ctrlKey || e.metaKey) {
                    const n = new Set(s)
                    if (n.has(i)) n.delete(i)
                    else n.add(i)
                    return n
                  }
                  return new Set([i])
                })
              }
            >
              <td className="n">{i + 1}</td>
              {r.map((v, j) => (
                <td key={j} className={v === null ? 'null' : numeric[j] ? 'num' : undefined} title={v !== null && v.length > 60 ? v.slice(0, 2000) : undefined}>
                  {v === null ? 'null' : v}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {!set.rows.length && <div className="wb-db-note">No rows</div>}
    </div>
  )
}
