// Ctrl+Alt+Shift+N Go to Symbol: `workspace/symbol` across the project's running
// language servers, as you type. Shift+Enter opens to the side.

import { useEffect, useRef, useState } from 'react'
import { Command as Cmdk } from 'cmdk'
import { Kbd, Spinner } from '@/ui'
import type { LspSymbolInformation } from './api'
import { lsp } from './client'
import { displayPath, shortenPath } from './convert'
import { openLocation } from './nav'
import { usePopups } from './store'
import { SymbolIcon } from './SymbolIcon'

export function GotoSymbolHost() {
  const g = usePopups((s) => s.gotoSymbol)
  if (!g) return null
  return <GotoSymbol projectId={g.projectId} />
}

interface State {
  loading: boolean
  error?: string
  items: LspSymbolInformation[]
  /** No server of this project answers workspace/symbol. */
  none?: boolean
}

function GotoSymbol({ projectId }: { projectId: string }) {
  const [q, setQ] = useState('')
  const [st, setSt] = useState<State>({ loading: false, items: [] })
  const [selected, setSelected] = useState('')
  const seq = useRef(0)
  const close = () => {
    usePopups.getState().set({ gotoSymbol: null })
    lsp.lastEditor?.focus()
  }
  useEffect(() => {
    const query = q.trim()
    const n = ++seq.current
    const ctrl = new AbortController()
    if (!query) {
      setSt({ loading: false, items: [] })
      return
    }
    setSt((s) => ({ ...s, loading: true }))
    const t = setTimeout(async () => {
      try {
        const conn = await lsp.connect(projectId)
        if (!conn) throw new Error('Code intelligence is off for this project')
        await conn.whenOpen()
        const r = await conn.request<LspSymbolInformation[] | null>('workspace/symbol', { query }, { signal: ctrl.signal })
        if (n !== seq.current) return
        const items = (r.result ?? []).filter((s) => s.location && 'uri' in s.location).slice(0, 300)
        setSt({ loading: false, items })
      } catch (e) {
        if (n !== seq.current || ctrl.signal.aborted) return
        const msg = e instanceof Error ? e.message : String(e)
        setSt({ loading: false, items: [], none: /no language server/i.test(msg), error: /no language server/i.test(msg) ? undefined : msg })
      }
    }, 150)
    return () => {
      clearTimeout(t)
      ctrl.abort()
    }
  }, [q, projectId])

  const choose = (s: LspSymbolInformation, side = false) => {
    usePopups.getState().set({ gotoSymbol: null })
    openLocation(s.location.uri, 'range' in s.location ? s.location.range : undefined, { side })
  }
  const key = (s: LspSymbolInformation, i: number) => `${i}:${s.name}`

  return (
    <Cmdk.Dialog
      open
      onOpenChange={(o) => !o && close()}
      label="Go to symbol"
      className="wb-palette wb-quickopen lsp-symbols"
      overlayClassName="wb-palette-overlay"
      shouldFilter={false}
      loop
      value={selected}
      onValueChange={setSelected}
    >
      <Cmdk.Input
        value={q}
        onValueChange={setQ}
        placeholder="Go to symbol (functions, types, constants…)"
        autoFocus
        onKeyDown={(e) => {
          if (e.key === 'Enter' && e.shiftKey) {
            const i = st.items.findIndex((s, j) => key(s, j) === selected)
            if (i >= 0) {
              e.preventDefault()
              choose(st.items[i], true)
            }
          }
        }}
      />
      <Cmdk.List>
        {!q.trim() && <div className="wb-quickopen-hint">Type a symbol name. Only running language servers are asked.</div>}
        {st.none && <div className="wb-quickopen-hint">No language server is running for this project: open one of its files to start one.</div>}
        {st.error && <div className="wb-quickopen-hint wb-danger">{st.error}</div>}
        {q.trim() && !st.loading && !st.error && !st.none && !st.items.length && <Cmdk.Empty>No matching symbols.</Cmdk.Empty>}
        {st.items.map((s, i) => {
          const path = displayPath(s.location.uri)
          const line = 'range' in s.location ? s.location.range.start.line + 1 : null
          return (
            <Cmdk.Item key={key(s, i)} value={key(s, i)} onSelect={() => choose(s)}>
              <SymbolIcon kind={s.kind} />
              <span className="wb-quickopen-name">{s.name}</span>
              {s.containerName && <span className="lsp-popup-detail wb-ellipsis">{s.containerName}</span>}
              <span className="wb-quickopen-dir wb-ellipsis" style={{ textAlign: 'right' }} title={path}>
                {shortenPath(path)}
                {line ? `:${line}` : ''}
              </span>
            </Cmdk.Item>
          )
        })}
      </Cmdk.List>
      <div className="wb-quickopen-footer">
        {st.loading && <Spinner size={10} />}
        <span className="wb-grow">{st.items.length ? `${st.items.length}${st.items.length >= 300 ? '+' : ''} symbols` : ''}</span>
        <Kbd>Enter</Kbd> open <Kbd>Shift+Enter</Kbd> to the side
      </div>
    </Cmdk.Dialog>
  )
}
