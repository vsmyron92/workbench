// The Find Usages tool window (bottom, Alt+F7 fills it): one tab per search, usages
// grouped by file with the line of each and the usage highlighted. A click opens it.

import { useMemo, useState } from 'react'
import { ChevronDown, ChevronRight, FileCode, Library, LocateFixed, X } from 'lucide-react'
import { EmptyState, IconButton, Spinner } from '@/ui'
import { displayPath, groupByFile, shortenPath } from './convert'
import { previewParts } from './logic'
import { lineOf, openLocation } from './nav'
import { usePreviews } from './Chooser'
import { useUsages, type UsageSearch } from './store'

export function UsagesWindow({ projectId }: { projectId: string | null }) {
  const searches = useUsages((s) => s.searches)
  const active = useUsages((s) => s.active)
  const mine = searches.filter((s) => s.projectId === projectId)
  const cur = mine.find((s) => s.id === active) ?? mine[mine.length - 1]
  if (!cur) {
    return (
      <EmptyState icon={LocateFixed} title="No usages searched yet">
        Put the caret on a symbol in the editor and press Alt+F7.
      </EmptyState>
    )
  }
  return (
    <div className="wb-fill lsp-usages">
      <div className="lsp-problems-bar">
        <div className="lsp-tabs" role="tablist">
          {mine.map((s) => (
            <span key={s.id} className={s.id === cur.id ? 'lsp-tab-wrap active' : 'lsp-tab-wrap'}>
              <button role="tab" aria-selected={s.id === cur.id} className={s.id === cur.id ? 'active' : ''} onClick={() => useUsages.getState().select(s.id)}>
                {s.title}
                {s.state === 'done' && <span className="lsp-tab-count">{s.locs.length}</span>}
              </button>
              <IconButton icon={X} size="small" label="Close" onClick={() => useUsages.getState().close(s.id)} />
            </span>
          ))}
        </div>
      </div>
      <Results key={cur.id} search={cur} />
    </div>
  )
}

function Results({ search }: { search: UsageSearch }) {
  const groups = useMemo(() => groupByFile(search.locs), [search.locs])
  const texts = usePreviews(search.locs)
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  if (search.state === 'loading') {
    return (
      <div className="wb-empty">
        <Spinner size={18} />
        <span>Searching usages of {search.title}…</span>
      </div>
    )
  }
  if (search.state === 'error') return <EmptyState icon={LocateFixed} title={`Could not find usages of ${search.title}`}>{search.error}</EmptyState>
  if (!search.locs.length) return <EmptyState icon={LocateFixed} title={`No usages of ${search.title} found`} />
  return (
    <div className="wb-scroll lsp-problems-list">
      <div className="lsp-usages-summary">
        Usages of <b>{search.title}</b>: {search.locs.length} in {groups.length} file{groups.length === 1 ? '' : 's'}
      </div>
      {groups.map((g) => {
        const lib = g.uri.startsWith('lsp-src://')
        const path = displayPath(g.uri)
        const name = path.slice(path.lastIndexOf('/') + 1)
        const dir = path.slice(0, Math.max(0, path.lastIndexOf('/')))
        const open = !collapsed.has(g.uri)
        const text = texts.get(g.uri) ?? null
        return (
          <div key={g.uri}>
            <div
              className="wb-list-row lsp-file-row"
              title={path}
              onClick={() =>
                setCollapsed((s) => {
                  const n = new Set(s)
                  if (open) n.add(g.uri)
                  else n.delete(g.uri)
                  return n
                })
              }
            >
              {open ? <ChevronDown size={13} className="wb-subtle" /> : <ChevronRight size={13} className="wb-subtle" />}
              {lib ? <Library size={13} className="wb-subtle" /> : <FileCode size={13} className="wb-subtle" />}
              <span className="lsp-file-name">{name}</span>
              <span className="wb-subtle wb-small wb-ellipsis">{lib ? shortenPath(dir) : dir}</span>
              <span className="wb-subtle wb-small">
                {g.locs.length} usage{g.locs.length === 1 ? '' : 's'}
              </span>
            </div>
            {open &&
              g.locs.map((l, i) => {
                const line = lineOf(text, l.range.start.line)
                const end = l.range.end.line === l.range.start.line ? l.range.end.character : line.length
                const p = previewParts(line, l.range.start.character, end)
                const origin = l.uri === search.origin.uri && l.range.start.line === search.origin.line
                return (
                  <div key={i} className={`wb-list-row lsp-usage-row${origin ? ' origin' : ''}`} onClick={() => openLocation(l.uri, l.range)}>
                    <span className="lsp-usage-num">{l.range.start.line + 1}</span>
                    <span className="lsp-usage-code wb-ellipsis">
                      {text === null && !texts.has(l.uri) ? '…' : p.before}
                      <b>{p.match}</b>
                      {p.after}
                    </span>
                  </div>
                )
              })}
          </div>
        )
      })}
    </div>
  )
}
