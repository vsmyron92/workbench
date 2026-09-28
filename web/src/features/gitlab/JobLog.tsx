// A CI job log: incremental polling by byte offset while the job runs, the
// log in AnsiLog with foldable sections (an outline to fold and jump), and
// search through xterm's search addon.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { SearchAddon } from '@xterm/addon-search'
import { ArrowDownToLine, ChevronDown, ChevronRight, ChevronsDownUp, ChevronsUpDown, ChevronUp, ListTree, Search } from 'lucide-react'
import { AnsiLog, ErrorBox, formatDuration, IconButton, Input, Loading } from '@/ui'
import { fetchTrace } from './api'
import { isActive, isCollapsed, parseLog, renderLog, sectionSearchText, type LogSection } from './logic'

/** Characters of log kept in the browser; older output is dropped. */
const MAX_CHARS = 8_000_000

export interface TraceState {
  text: string
  status: string | null
  complete: boolean
  truncated: boolean
  loading: boolean
  error: unknown
}

/**
 * Poll a job's log. `active` pauses polling (hidden panel); `generation`
 * restarts from scratch (after a retry of the same id, or a manual reload).
 */
export function useJobTrace(projectId: string, jobId: number, active: boolean, generation = 0): TraceState {
  const [state, setState] = useState<TraceState>({ text: '', status: null, complete: false, truncated: false, loading: true, error: null })
  const offset = useRef(0)
  const text = useRef('')
  const truncated = useRef(false)
  const done = useRef(false)

  // Reset when the job changes.
  useEffect(() => {
    offset.current = 0
    text.current = ''
    truncated.current = false
    done.current = false
    setState({ text: '', status: null, complete: false, truncated: false, loading: true, error: null })
  }, [projectId, jobId, generation])

  useEffect(() => {
    if (!active) return
    let cancelled = false
    let timer: number | undefined
    const ctrl = new AbortController()
    const tick = async () => {
      if (done.current) return
      try {
        const c = await fetchTrace(projectId, jobId, offset.current, ctrl.signal)
        if (cancelled) return
        let t = c.reset ? c.text : text.current + c.text
        if (c.reset) truncated.current = c.truncated
        if (t.length > MAX_CHARS) {
          const cut = t.indexOf('\n', t.length - MAX_CHARS)
          t = t.slice(cut + 1)
          truncated.current = true
        }
        text.current = t
        offset.current = c.offset
        done.current = c.complete
        setState({ text: t, status: c.status, complete: c.complete, truncated: truncated.current, loading: false, error: null })
        if (!c.complete) timer = window.setTimeout(tick, isActive(c.status) && c.status !== 'pending' ? 2000 : 5000)
      } catch (e) {
        if (cancelled || ctrl.signal.aborted) return
        setState((s) => ({ ...s, loading: false, error: e }))
        timer = window.setTimeout(tick, 10_000)
      }
    }
    void tick()
    return () => {
      cancelled = true
      ctrl.abort()
      window.clearTimeout(timer)
    }
  }, [projectId, jobId, active, generation])

  return state
}

function sectionDuration(s: LogSection): string {
  return s.end !== null ? formatDuration(Math.max(0, s.end - s.start)) : '…'
}

/** Log viewer with outline, folding and search. The trace comes from `useJobTrace`. */
export function JobLogView({
  trace,
  outline = true,
  onRetry,
}: {
  trace: TraceState
  /** Show the sections outline (off on phones). */
  outline?: boolean
  onRetry?: () => void
}) {
  const [overrides, setOverrides] = useState<Map<number, boolean>>(new Map())
  const [follow, setFollow] = useState(true)
  const [showOutline, setShowOutline] = useState(true)
  const [query, setQuery] = useState('')
  const search = useRef<SearchAddon | null>(null)

  const parsed = useMemo(() => parseLog(trace.text), [trace.text])
  const rendered = useMemo(() => {
    const body = renderLog(parsed, overrides)
    return trace.truncated ? `\x1b[2m[… the start of this log is not shown]\x1b[22m\n${body}` : body
  }, [parsed, overrides, trace.truncated])

  const toggle = useCallback((s: LogSection) => {
    setFollow(false)
    setOverrides((o) => new Map(o).set(s.id, !isCollapsed(s, o)))
  }, [])
  const setAll = (collapsed: boolean) => {
    setFollow(false)
    setOverrides(new Map(parsed.sections.map((s) => [s.id, collapsed])))
  }
  const jump = (s: LogSection) => {
    setFollow(false)
    // Unfold its parents so the header is visible, then find it.
    setOverrides((o) => {
      const n = new Map(o)
      for (const p of parsed.sections) if (p.id !== s.id && p.depth < s.depth && p.line < s.line && (p.end ?? Infinity) >= s.start) n.set(p.id, false)
      return n
    })
    window.setTimeout(() => search.current?.findNext(sectionSearchText(s, overrides), { caseSensitive: true }), 30)
  }
  const find = (dir: 1 | -1) => {
    if (!query) return
    setFollow(false)
    if (dir > 0) search.current?.findNext(query, { incremental: false })
    else search.current?.findPrevious(query)
  }

  if (trace.error && !trace.text) return <ErrorBox error={trace.error} onRetry={onRetry} />
  if (trace.loading && !trace.text) return <Loading label="Loading log…" />

  const hasSections = parsed.sections.length > 0
  return (
    <div className="gl-log-main">
      <div className="gl-filters">
        {outline && hasSections && (
          <IconButton icon={ListTree} size="small" label="Sections" active={showOutline} onClick={() => setShowOutline(!showOutline)} />
        )}
        <Search size={13} className="wb-subtle" />
        <Input
          small
          className="gl-search"
          placeholder="Search log"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') find(e.shiftKey ? -1 : 1)
          }}
        />
        <IconButton icon={ChevronUp} size="small" label="Previous match (Shift+Enter)" onClick={() => find(-1)} disabled={!query} />
        <IconButton icon={ChevronDown} size="small" label="Next match (Enter)" onClick={() => find(1)} disabled={!query} />
        <span style={{ flex: 1 }} />
        {hasSections && (
          <>
            <IconButton icon={ChevronsUpDown} size="small" label="Expand all sections" onClick={() => setAll(false)} />
            <IconButton icon={ChevronsDownUp} size="small" label="Collapse all sections" onClick={() => setAll(true)} />
          </>
        )}
        <IconButton icon={ArrowDownToLine} size="small" label="Follow the end of the log" active={follow} onClick={() => setFollow(!follow)} />
      </div>
      <div className="gl-log-body">
        {outline && hasSections && showOutline && (
          <div className="gl-outline" role="tree">
            {parsed.sections.map((s) => {
              const folded = isCollapsed(s, overrides)
              return (
                <div key={s.id} className="gl-outline-row" style={{ paddingLeft: 4 + s.depth * 12 }} onClick={() => jump(s)} title={s.title}>
                  <span
                    className="fold"
                    onClick={(e) => {
                      e.stopPropagation()
                      toggle(s)
                    }}
                    role="button"
                    aria-label={folded ? 'Expand section' : 'Collapse section'}
                  >
                    {folded ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
                  </span>
                  <span className="t">{s.title}</span>
                  <span className="d">{sectionDuration(s)}</span>
                </div>
              )
            })}
          </div>
        )}
        <div className="gl-log-main">
          <AnsiLog text={rendered} follow={follow} onSearchReady={(s) => (search.current = s)} />
        </div>
      </div>
    </div>
  )
}
