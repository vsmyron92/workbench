// A GitHub Actions job log in AnsiLog: steps and `##[group]` sections fold (an
// outline to fold and jump), message markers are coloured, and search runs on
// xterm's search addon (it unfolds everything first, so nothing is missed).

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { SearchAddon } from '@xterm/addon-search'
import { ChevronDown, ChevronRight, ChevronsDownUp, ChevronsUpDown, ChevronUp, ListTree, Search } from 'lucide-react'
import { AnsiLog, formatDuration, IconButton, Input } from '@/ui'
import { StatusIcon } from './components'
import { headerText, isCollapsed, parseGhLog, renderGhLog, type LogSection } from './logic'
import type { Step, StepMark } from './types'

export function JobLogView({
  text,
  steps,
  marks,
  truncated,
  outline = true,
}: {
  text: string
  steps: Step[]
  marks: StepMark[]
  truncated: boolean
  /** Show the outline (off on phones). */
  outline?: boolean
}) {
  const [overrides, setOverrides] = useState<Map<number, boolean>>(new Map())
  const [showOutline, setShowOutline] = useState(true)
  const [query, setQuery] = useState('')
  const search = useRef<SearchAddon | null>(null)
  const jumped = useRef(false)

  const parsed = useMemo(() => parseGhLog(text, steps, marks), [text, steps, marks])
  const rendered = useMemo(() => {
    const body = renderGhLog(parsed, overrides)
    return truncated ? `\x1b[2m[… the start of this log is not shown]\x1b[22m\n${body}` : body
  }, [parsed, overrides, truncated])

  const toggle = useCallback((s: LogSection) => setOverrides((o) => new Map(o).set(s.id, !isCollapsed(s, o))), [])
  const setAll = (collapsed: boolean) => setOverrides(new Map(parsed.sections.map((s) => [s.id, collapsed])))
  const find = (s: LogSection, o: ReadonlyMap<number, boolean>) =>
    window.setTimeout(() => search.current?.findNext(headerText(s, isCollapsed(s, o)), { caseSensitive: true }), 40)
  const jump = (s: LogSection) => {
    // Unfold its parents so the header is visible, then find it.
    const n = new Map(overrides)
    for (const p of parsed.sections) if (p.id !== s.id && p.depth < s.depth && p.line < s.line && p.kind === 'step') n.set(p.id, false)
    setOverrides(n)
    find(s, n)
  }
  const doSearch = (dir: 1 | -1) => {
    if (!query) return
    const folded = parsed.sections.some((s) => isCollapsed(s, overrides))
    if (folded) setAll(false)
    window.setTimeout(() => {
      if (dir > 0) search.current?.findNext(query, { incremental: false })
      else search.current?.findPrevious(query)
    }, folded ? 40 : 0)
  }

  // Show the first failed step once the log is there (GitHub opens it too).
  useEffect(() => {
    if (jumped.current || !parsed.sections.length) return
    const failed = parsed.sections.find((s) => s.kind === 'step' && s.state === 'failed')
    if (!failed) return
    jumped.current = true
    find(failed, overrides)
  }, [parsed]) // eslint-disable-line react-hooks/exhaustive-deps

  const hasSections = parsed.sections.length > 0
  return (
    <div className="gh-log-main">
      <div className="gh-filters">
        {outline && hasSections && (
          <IconButton icon={ListTree} size="small" label="Steps and groups" active={showOutline} onClick={() => setShowOutline(!showOutline)} />
        )}
        <Search size={13} className="wb-subtle" />
        <Input
          small
          className="gh-search"
          placeholder="Search log"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') doSearch(e.shiftKey ? -1 : 1)
          }}
        />
        <IconButton icon={ChevronUp} size="small" label="Previous match (Shift+Enter)" onClick={() => doSearch(-1)} disabled={!query} />
        <IconButton icon={ChevronDown} size="small" label="Next match (Enter)" onClick={() => doSearch(1)} disabled={!query} />
        <span style={{ flex: 1 }} />
        {hasSections && (
          <>
            <IconButton icon={ChevronsUpDown} size="small" label="Expand all" onClick={() => setAll(false)} />
            <IconButton icon={ChevronsDownUp} size="small" label="Collapse all" onClick={() => setAll(true)} />
          </>
        )}
      </div>
      <div className="gh-log-body">
        {outline && hasSections && showOutline && (
          <div className="gh-outline" role="tree">
            {parsed.sections.filter((s) => !s.echo).map((s) => {
              const folded = isCollapsed(s, overrides)
              return (
                <div key={s.id} className="gh-outline-row" style={{ paddingLeft: 4 + s.depth * 14 }} onClick={() => jump(s)} title={s.title}>
                  <span
                    className="fold"
                    onClick={(e) => {
                      e.stopPropagation()
                      toggle(s)
                    }}
                    role="button"
                    aria-label={folded ? 'Expand' : 'Collapse'}
                  >
                    {folded ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
                  </span>
                  {s.kind === 'step' && <StatusIcon state={s.state} size={13} />}
                  <span className="t">{s.title}</span>
                  {s.kind === 'step' && <span className="d">{s.duration !== null && s.duration !== undefined ? formatDuration(s.duration) : ''}</span>}
                </div>
              )
            })}
          </div>
        )}
        <div className="gh-log-main">
          <AnsiLog text={rendered} follow={false} onSearchReady={(s) => (search.current = s)} />
        </div>
      </div>
    </div>
  )
}
