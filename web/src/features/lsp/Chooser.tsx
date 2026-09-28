// A popup at the caret listing locations (several declarations or implementations,
// Show Usages): type to filter, arrows, Enter to go, Escape to close.

import { useEffect, useMemo, useRef, useState } from 'react'
import { Command as Cmdk } from 'cmdk'
import { FileCode, Library } from 'lucide-react'
import { navigate } from './actions'
import { displayPath, type Loc } from './convert'
import { previewParts } from './logic'
import { lineOf, textOf } from './nav'
import { usePopups, type ChooserState } from './store'

export function ChooserHost() {
  const chooser = usePopups((s) => s.chooser)
  if (!chooser) return null
  return <Chooser key={`${chooser.title}:${chooser.x}:${chooser.y}`} state={chooser} />
}

function basename(p: string) {
  return p.slice(p.lastIndexOf('/') + 1)
}

/** Preview lines of every location, loaded file by file. */
export function usePreviews(locs: Loc[]): Map<string, string | null> {
  const [texts, setTexts] = useState<Map<string, string | null>>(new Map())
  useEffect(() => {
    let cancelled = false
    const uris = [...new Set(locs.map((l) => l.uri))].slice(0, 200)
    for (const u of uris) {
      void textOf(u).then((t) => {
        if (!cancelled) setTexts((m) => new Map(m).set(u, t))
      })
    }
    return () => {
      cancelled = true
    }
  }, [locs])
  return texts
}

function Chooser({ state }: { state: ChooserState }) {
  const close = () => {
    usePopups.getState().set({ chooser: null })
    state.editor?.focus()
  }
  const texts = usePreviews(state.locs)
  const [q, setQ] = useState('')
  const ref = useRef<HTMLDivElement>(null)
  const items = useMemo(
    () =>
      state.locs.map((l, i) => {
        const path = displayPath(l.uri)
        const text = lineOf(texts.get(l.uri) ?? null, l.range.start.line)
        return { key: String(i), loc: l, path, name: basename(path), text }
      }),
    [state.locs, texts],
  )
  const shown = q ? items.filter((it) => `${it.path} ${it.text}`.toLowerCase().includes(q.toLowerCase())) : items
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) usePopups.getState().set({ chooser: null })
    }
    window.addEventListener('mousedown', onDown, true)
    return () => window.removeEventListener('mousedown', onDown, true)
  }, [])
  const width = Math.min(640, window.innerWidth - 16)
  const left = Math.max(8, Math.min(state.x, window.innerWidth - width - 8))
  const below = state.y + 360 < window.innerHeight
  const style: React.CSSProperties = below ? { left, top: state.y, width } : { left, bottom: Math.max(8, window.innerHeight - state.y + 24), width }
  return (
    <div ref={ref} className="lsp-popup" style={style}>
      <Cmdk label={state.title} shouldFilter={false} loop onKeyDown={(e) => e.key === 'Escape' && (e.preventDefault(), close())}>
        <div className="lsp-popup-title">{state.title}</div>
        <Cmdk.Input value={q} onValueChange={setQ} placeholder="Type to filter" autoFocus />
        <Cmdk.List>
          {!shown.length && <Cmdk.Empty>Nothing matches.</Cmdk.Empty>}
          {shown.map((it) => {
            const line = it.loc.range.start.line
            const parts = previewParts(it.text, it.loc.range.start.character, it.loc.range.end.line === line ? it.loc.range.end.character : it.text.length, 140)
            const lib = it.loc.uri.startsWith('lsp-src://')
            return (
              <Cmdk.Item
                key={it.key}
                value={it.key}
                onSelect={() => {
                  usePopups.getState().set({ chooser: null })
                  navigate(state.editor, it.loc)
                }}
              >
                {lib ? <Library size={14} className="wb-subtle" /> : <FileCode size={14} className="wb-subtle" />}
                <span className="lsp-popup-code wb-grow wb-ellipsis">
                  {parts.before}
                  <b>{parts.match}</b>
                  {parts.after}
                </span>
                <span className="lsp-popup-where wb-ellipsis" title={it.path}>
                  {it.name}:{line + 1}
                </span>
              </Cmdk.Item>
            )
          })}
        </Cmdk.List>
      </Cmdk>
    </div>
  )
}
