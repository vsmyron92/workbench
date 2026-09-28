// Ctrl+F12 File Structure: the file's symbols as an indented list; type to filter
// (matches keep their parents' indentation), Enter to jump.

import { useMemo, useState } from 'react'
import { Command as Cmdk } from 'cmdk'
import { Kbd, Spinner } from '@/ui'
import { toRange } from './convert'
import { usePopups, type StructureState } from './store'
import { SymbolIcon } from './SymbolIcon'

export function FileStructureHost() {
  const s = usePopups((x) => x.structure)
  if (!s) return null
  return <FileStructure state={s} />
}

function FileStructure({ state }: { state: StructureState }) {
  const [q, setQ] = useState('')
  const close = () => {
    usePopups.getState().set({ structure: null })
    state.editor.focus()
  }
  // Start on the symbol around the caret.
  const caretLine = (state.editor.getPosition()?.lineNumber ?? 1) - 1
  const items = useMemo(() => {
    const needle = q.trim().toLowerCase()
    return state.symbols
      .map((s, i) => ({ s, key: String(i) }))
      .filter(({ s }) => !needle || s.name.toLowerCase().includes(needle) || (s.detail ?? '').toLowerCase().includes(needle))
  }, [state.symbols, q])
  const initial = useMemo(() => {
    let best: string | undefined
    for (const it of items) if (it.s.range.start.line <= caretLine && it.s.range.end.line >= caretLine) best = it.key
    return best ?? items[0]?.key
  }, [items, caretLine])
  const [selected, setSelected] = useState<string | undefined>(undefined)
  return (
    <Cmdk.Dialog
      open
      onOpenChange={(o) => !o && close()}
      label="File structure"
      className="wb-palette lsp-structure"
      overlayClassName="wb-palette-overlay"
      shouldFilter={false}
      loop
      value={selected ?? initial ?? ''}
      onValueChange={setSelected}
    >
      <div className="lsp-popup-title">Structure of {state.title}</div>
      <Cmdk.Input value={q} onValueChange={setQ} placeholder="Type to filter symbols" autoFocus />
      <Cmdk.List>
        {state.loading && (
          <div className="lsp-popup-empty">
            <Spinner /> Loading symbols…
          </div>
        )}
        {state.error && <div className="lsp-popup-empty wb-danger">{state.error}</div>}
        {!state.loading && !state.error && !items.length && <Cmdk.Empty>{q ? 'Nothing matches.' : 'No symbols in this file.'}</Cmdk.Empty>}
        {items.map(({ s, key }) => (
          <Cmdk.Item
            key={key}
            value={key}
            onSelect={() => {
              usePopups.getState().set({ structure: null })
              const r = toRange(s.selectionRange)
              state.editor.setSelection({ startLineNumber: r.startLineNumber, startColumn: r.startColumn, endLineNumber: r.startLineNumber, endColumn: r.startColumn })
              state.editor.revealRangeInCenterIfOutsideViewport(r)
              state.editor.focus()
            }}
          >
            <span style={{ width: (q ? 0 : Math.min(s.depth, 8)) * 16, flex: 'none' }} />
            <SymbolIcon kind={s.kind} />
            <span className="wb-ellipsis">{s.name}</span>
            {s.detail && <span className="lsp-popup-detail wb-ellipsis">{s.detail}</span>}
            <span className="wb-grow" />
            <span className="lsp-popup-where">{s.selectionRange.start.line + 1}</span>
          </Cmdk.Item>
        ))}
      </Cmdk.List>
      <div className="wb-quickopen-footer">
        <span className="wb-grow">{state.symbols.length ? `${state.symbols.length} symbols` : ''}</span>
        <Kbd>Enter</Kbd> go to <Kbd>Esc</Kbd> close
      </div>
    </Cmdk.Dialog>
  )
}
