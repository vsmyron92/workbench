// The Structure tool window (Alt+7, CLion's): the focused file's symbols as a tree,
// from its language server. It follows the editor you work in, refreshes after
// edits, marks the symbol around the caret, and a click moves the caret there.

import { useEffect, useMemo, useRef, useState } from 'react'
import type { editor } from 'monaco-editor'
import { ChevronDown, ChevronRight, ListTree } from 'lucide-react'
import { Button, EmptyState, Input, Spinner } from '@/ui'
import { navigate } from './actions'
import type { LspDocumentSymbol, LspSymbolInformation } from './api'
import { has, lsp, useLspRuntime } from './client'
import { flattenSymbols, type FlatSymbol } from './convert'
import { usePopups } from './store'
import { SymbolIcon } from './SymbolIcon'

const REFRESH_MS = 600

interface State {
  uri: string | null
  loading: boolean
  symbols: FlatSymbol[]
  error?: string
  /** Why there is nothing to show (no editor, no server…). */
  none?: string
  projectOff?: string
}

export function StructureWindow() {
  const activeUri = useLspRuntime((s) => s.activeUri)
  const tick = useLspRuntime((s) => s.tick)
  const [st, setSt] = useState<State>({ uri: null, loading: false, symbols: [] })
  const [caret, setCaret] = useState(0)
  const [filter, setFilter] = useState('')
  const [collapsed, setCollapsed] = useState<Set<number>>(new Set())
  const edRef = useRef<editor.ICodeEditor | null>(null)
  const seq = useRef(0)

  // The editor behind the active document, and its symbols (again after edits).
  useEffect(() => {
    const ed = lsp.lastEditor
    const model = ed?.getModel()
    if (!ed || !model || model.uri.toString() !== activeUri) {
      edRef.current = null
      setSt({ uri: activeUri, loading: false, symbols: [], none: 'Focus an editor to see its structure.' })
      return
    }
    edRef.current = ed
    const load = async () => {
      const n = ++seq.current
      const t = lsp.target(model)
      if (!t) {
        const pid = /^file:\/\/\/([^/~][^/]*)\//.exec(model.uri.toString())?.[1]
        setSt({ uri: activeUri, loading: false, symbols: [], none: lsp.whyNot(model), projectOff: pid && lsp.isEnabled(pid) === false ? pid : undefined })
        return
      }
      if (!has(t.caps, 'documentSymbolProvider')) {
        setSt({ uri: activeUri, loading: false, symbols: [], none: `${t.server} does not list symbols.` })
        return
      }
      setSt((s) => ({ ...s, uri: activeUri, loading: true, none: undefined, projectOff: undefined }))
      try {
        const r = await t.conn.request<(LspDocumentSymbol | LspSymbolInformation)[] | null>('textDocument/documentSymbol', { textDocument: { uri: t.uri } })
        if (n === seq.current) setSt({ uri: activeUri, loading: false, symbols: flattenSymbols(r.result) })
      } catch (e) {
        if (n === seq.current) setSt((s) => ({ ...s, loading: false, error: e instanceof Error ? e.message : String(e) }))
      }
    }
    void load()
    let timer: number | undefined
    const subs = [
      model.onDidChangeContent(() => {
        window.clearTimeout(timer)
        timer = window.setTimeout(() => void load(), REFRESH_MS)
      }),
      ed.onDidChangeCursorPosition((e) => setCaret(e.position.lineNumber - 1)),
    ]
    setCaret((ed.getPosition()?.lineNumber ?? 1) - 1)
    return () => {
      window.clearTimeout(timer)
      subs.forEach((s) => s.dispose())
    }
    // `tick` changes when servers start or documents get their server.
  }, [activeUri, tick])

  // The innermost symbol around the caret.
  const around = useMemo(() => {
    let best = -1
    st.symbols.forEach((s, i) => {
      if (s.range.start.line <= caret && s.range.end.line >= caret) best = i
    })
    return best
  }, [st.symbols, caret])

  const rows = useMemo(() => {
    const needle = filter.trim().toLowerCase()
    const out: { s: FlatSymbol; i: number; hasKids: boolean }[] = []
    let hideBelow: number | null = null
    st.symbols.forEach((s, i) => {
      const hasKids = (st.symbols[i + 1]?.depth ?? -1) > s.depth
      if (needle) {
        if (s.name.toLowerCase().includes(needle) || (s.detail ?? '').toLowerCase().includes(needle)) out.push({ s, i, hasKids: false })
        return
      }
      if (hideBelow !== null && s.depth > hideBelow) return
      hideBelow = null
      out.push({ s, i, hasKids })
      if (hasKids && collapsed.has(i)) hideBelow = s.depth
    })
    return out
  }, [st.symbols, filter, collapsed])

  const listRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    listRef.current?.querySelector('.lsp-structure-row.current')?.scrollIntoView({ block: 'nearest' })
  }, [around])

  const name = st.uri ? decodeURIComponent(st.uri.split('/').pop() ?? '') : ''
  return (
    <div className="wb-fill lsp-structure-window">
      <div className="lsp-problems-bar">
        <span className="lsp-hierarchy-title wb-ellipsis">{name || 'Structure'}</span>
        <span className="wb-grow" />
        <Input small className="lsp-structure-filter" placeholder="Filter" value={filter} onChange={(e) => setFilter(e.target.value)} aria-label="Filter symbols" />
        {st.loading && <Spinner size={10} />}
      </div>
      <div ref={listRef} className="wb-scroll lsp-structure-list" role="tree">
        {st.none ? (
          <EmptyState
            icon={ListTree}
            title="No structure"
            action={
              st.projectOff ? (
                <Button size="small" variant="primary" onClick={() => usePopups.getState().set({ enable: { projectId: st.projectOff! } })}>
                  Enable code intelligence…
                </Button>
              ) : undefined
            }
          >
            {st.none}
          </EmptyState>
        ) : st.error ? (
          <div className="lsp-hierarchy-note wb-danger">{st.error}</div>
        ) : !rows.length && !st.loading ? (
          <EmptyState icon={ListTree} title={filter ? 'Nothing matches' : 'No symbols'} />
        ) : (
          rows.map(({ s, i, hasKids }) => (
            <div
              key={i}
              role="treeitem"
              aria-expanded={hasKids ? !collapsed.has(i) : undefined}
              className={`wb-list-row lsp-structure-row${i === around ? ' current' : ''}`}
              style={{ paddingLeft: 6 + (filter ? 0 : s.depth * 14) }}
              title={s.detail ? `${s.name} — ${s.detail}` : s.name}
              onClick={() => {
                const ed = edRef.current
                if (ed && st.uri) {
                  navigate(ed, { uri: st.uri, range: s.selectionRange })
                  ed.focus()
                }
              }}
            >
              <span
                className="lsp-hierarchy-toggle"
                onClick={(e) => {
                  if (!hasKids) return
                  e.stopPropagation()
                  setCollapsed((c) => {
                    const n = new Set(c)
                    if (n.has(i)) n.delete(i)
                    else n.add(i)
                    return n
                  })
                }}
              >
                {hasKids ? collapsed.has(i) ? <ChevronRight size={13} className="wb-subtle" /> : <ChevronDown size={13} className="wb-subtle" /> : null}
              </span>
              <SymbolIcon kind={s.kind} />
              <span className="lsp-hierarchy-name wb-ellipsis">{s.name}</span>
              {s.detail && <span className="lsp-diag-src wb-ellipsis">{s.detail}</span>}
              {filter && s.container && <span className="lsp-diag-src wb-ellipsis">in {s.container}</span>}
            </div>
          ))
        )}
      </div>
    </div>
  )
}
