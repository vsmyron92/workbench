// Git log: branch/text/author/path filters, a virtualized commit list with the
// lane graph and ref badges, and a details pane. Used by the 'gitlog' tool
// window (bottom) and the 'gitlog' panel.

import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { GitGraph, RefreshCw, TextSelect, X } from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import type { PanelProps } from '@/shell/types'
import { EmptyState, ErrorBox, IconButton, Input, Loading, Select, Spinner, Splitter, timeAgo, showMenu } from '@/ui'
import { gk, isNotRepo, useBisect, useBranches, useGitLog, useGitStatus } from './api'
import { checkout, checkoutRemote, openCommit } from './actions'
import { BisectBanner } from './Bisect'
import { CommitDetailsView, RefBadge, commitMenu } from './CommitDetails'
import { layoutGraph, segmentPath, type GraphRow } from './graph'
import { bisectMark, shortSha, splitPath, type BisectMark } from './logic'
import type { LogCommit, LogFilters, RefLabel } from './types'

const ROW = 24
const LANE = 14
const MAX_LANES = 24
const OVERSCAN = 12
/** Below this width the details pane goes under the list instead of beside it. */
const STACK_BELOW = 760
/** Beside the list, the details pane never takes more than this share. */
const SIDE_MAX_SHARE = 0.45
const SIDE_MIN = 240

function readNumber(key: string, dflt: number) {
  try {
    return Number(localStorage.getItem(key)) || dflt
  } catch {
    return dflt
  }
}

function saveNumber(key: string, v: number) {
  try {
    localStorage.setItem(key, String(Math.round(v)))
  } catch {
    /* private mode */
  }
}

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value)
  useEffect(() => {
    const t = window.setTimeout(() => setV(value), ms)
    return () => window.clearTimeout(t)
  }, [value, ms])
  return v
}

export function GitLogToolWindow({ projectId }: { projectId: string | null }) {
  if (!projectId) return <EmptyState title="No project selected" />
  return <LogView key={projectId} pid={projectId} />
}

export function GitLogPanel({ params }: PanelProps<{ projectId: string; path?: string; ref?: string; lines?: string; worktreeLines?: boolean }>) {
  if (!params?.projectId) return <EmptyState title="No project" />
  return (
    <LogView
      key={params.projectId}
      pid={params.projectId}
      path={params.path}
      gitRef={params.ref}
      lines={params.lines}
      worktreeLines={!!params.worktreeLines}
    />
  )
}

const MARK_LABEL: Record<BisectMark, string> = { bad: 'bad', good: 'good', skip: 'skip', current: 'testing', result: 'first bad' }

/** The graph of one row; like CLion, the subject starts right after this row's lanes. */
const GraphCell = memo(function GraphCell({ row, merge }: { row: GraphRow; merge: boolean }) {
  const x = row.col * LANE + LANE / 2
  const width = Math.min(Math.max(row.width, 1), MAX_LANES) * LANE + 2
  return (
    <svg className="git-graph" width={width} height={ROW} aria-hidden>
      {row.segments.map((s, i) => (
        <path key={i} d={segmentPath(s, LANE, ROW)} className={`git-lane-${s.color % 8}`} />
      ))}
      <circle cx={x} cy={ROW / 2} r={merge ? 3.2 : 3.6} className={`git-lane-${row.color % 8}${merge ? ' merge' : ''}`} />
    </svg>
  )
})

function LogView({
  pid,
  path: initialPath,
  gitRef,
  lines: initialLines,
  worktreeLines,
}: {
  pid: string
  path?: string
  gitRef?: string
  lines?: string
  worktreeLines?: boolean
}) {
  const qc = useQueryClient()
  const branches = useBranches(pid)
  const status = useGitStatus(pid)
  const bisecting = status.data?.state === 'bisecting'
  const bisect = useBisect(pid, bisecting)
  const bis = bisecting ? bisect.data : undefined
  const candidates = useMemo(() => new Set(bis?.active && !bis.result ? bis.candidates : []), [bis])
  // Branch filter: '' = all branches, 'HEAD' = current, else a ref name.
  const [scope, setScope] = useState(gitRef ?? (initialPath ? 'HEAD' : ''))
  const [text, setText] = useState('')
  const [author, setAuthor] = useState('')
  const [path, setPath] = useState(initialPath ?? '')
  // History of a line range of `path` (Show History for Selection).
  const [lineRange, setLineRange] = useState(initialLines ?? '')
  useEffect(() => {
    // The panel was re-targeted (Show History on another file, Show Log of a branch, a line range).
    if (initialPath !== undefined) setPath(initialPath)
    if (gitRef !== undefined) setScope(gitRef)
    setLineRange(initialLines ?? '')
  }, [initialPath, gitRef, initialLines])

  const grep = useDebounced(text.trim(), 300)
  const authorQ = useDebounced(author.trim(), 300)
  const pathQ = useDebounced(path.trim(), 400)
  const ranged = !!lineRange && !!pathQ
  const filters: LogFilters = {
    all: scope === '' && !ranged ? true : undefined,
    ref: scope && scope !== 'HEAD' ? scope : undefined,
    grep: grep || undefined,
    author: authorQ || undefined,
    path: pathQ || undefined,
    lines: ranged ? lineRange : undefined,
    worktreeLines: ranged && worktreeLines ? true : undefined,
  }
  const log = useGitLog(pid, filters)

  const commits = useMemo(() => {
    const seen = new Set<string>()
    const out: LogCommit[] = []
    for (const p of log.data?.pages ?? [])
      for (const c of p.commits)
        if (!seen.has(c.sha)) {
          seen.add(c.sha)
          out.push(c)
        }
    return out
  }, [log.data])
  const linear = !!log.data?.pages[0]?.linear
  const graph = useMemo(
    () => layoutGraph(linear ? commits.map((c, i) => ({ sha: c.sha, parents: i + 1 < commits.length ? [commits[i + 1].sha] : [] })) : commits),
    [commits, linear],
  )
  const headSha = branches.data?.head ?? null

  const [selected, setSelected] = useState<string | null>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const [view, setView] = useState({ top: 0, height: 400 })
  const [sideWidth, setSideWidth] = useState(() => readNumber('wb.git.logSide', 420))
  const [sideHeight, setSideHeight] = useState(() => readNumber('wb.git.logSideH', 280))
  const sideStart = useRef(sideWidth)
  // The log lives in a bottom tool window or in a (possibly narrow) editor-area
  // group: size the details pane from the space actually available.
  const bodyRef = useRef<HTMLDivElement>(null)
  const [body, setBody] = useState({ w: 0, h: 0 })

  useLayoutEffect(() => {
    const el = listRef.current
    if (!el) return
    const ro = new ResizeObserver(() => setView({ top: el.scrollTop, height: el.clientHeight }))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  useLayoutEffect(() => {
    const el = bodyRef.current
    if (!el) return
    const measure = () => setBody({ w: el.clientWidth, h: el.clientHeight })
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    measure()
    return () => ro.disconnect()
  }, [])
  const stacked = body.w > 0 && body.w < STACK_BELOW
  const sideW = body.w ? Math.max(SIDE_MIN, Math.min(sideWidth, Math.round(body.w * SIDE_MAX_SHARE))) : sideWidth
  const sideH = body.h ? Math.max(120, Math.min(sideHeight, Math.round(body.h * 0.6))) : sideHeight

  // Select the first commit once loaded (CLion shows details immediately).
  useEffect(() => {
    if (!selected && commits.length) setSelected(commits[0].sha)
  }, [commits, selected])

  const first = Math.max(0, Math.floor(view.top / ROW) - OVERSCAN)
  const last = Math.min(commits.length, Math.ceil((view.top + view.height) / ROW) + OVERSCAN)

  // Infinite scroll: fetch the next page when the viewport nears the end.
  useEffect(() => {
    if (last > commits.length - 80 && log.hasNextPage && !log.isFetchingNextPage) void log.fetchNextPage()
  }, [last, commits.length, log])

  const selectIndex = (i: number) => {
    const c = commits[Math.max(0, Math.min(commits.length - 1, i))]
    if (!c) return
    setSelected(c.sha)
    const el = listRef.current
    const idx = commits.indexOf(c)
    if (el) {
      if (idx * ROW < el.scrollTop) el.scrollTop = idx * ROW
      else if ((idx + 1) * ROW > el.scrollTop + el.clientHeight) el.scrollTop = (idx + 1) * ROW - el.clientHeight
    }
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    const i = commits.findIndex((c) => c.sha === selected)
    const page = Math.max(1, Math.floor(view.height / ROW) - 1)
    const moves: Record<string, number> = { ArrowDown: 1, ArrowUp: -1, PageDown: page, PageUp: -page }
    if (e.key in moves) {
      e.preventDefault()
      selectIndex(i + moves[e.key])
    } else if (e.key === 'Home') {
      e.preventDefault()
      selectIndex(0)
    } else if (e.key === 'End') {
      e.preventDefault()
      selectIndex(commits.length - 1)
    } else if (e.key === 'Enter' && selected) {
      openCommit(pid, selected)
    }
  }

  const checkoutRef = (r: RefLabel) => {
    if (r.kind === 'remote') {
      const slash = r.name.indexOf('/')
      const branch = r.name.slice(slash + 1)
      void checkoutRemote(pid, r.name, branch, !!branches.data?.local.some((b) => b.name === branch))
    } else void checkout(pid, { ref: r.name }, r.name)
  }

  const scopeOptions = branches.data
  const notRepo = isNotRepo(log.error)

  return (
    <div className="git-log">
      <div className="git-log-filters">
        <Select value={scope} onChange={(e) => setScope(e.target.value)} style={{ maxWidth: 200 }} aria-label="Branch filter">
          <option value="">All branches</option>
          <option value="HEAD">HEAD{branches.data?.current ? ` (${branches.data.current})` : ''}</option>
          {scopeOptions && scopeOptions.local.length > 0 && (
            <optgroup label="Local">
              {scopeOptions.local.map((b) => (
                <option key={b.name} value={b.name}>
                  {b.name}
                </option>
              ))}
            </optgroup>
          )}
          {scopeOptions && scopeOptions.remote.length > 0 && (
            <optgroup label="Remote">
              {scopeOptions.remote.slice(0, 500).map((b) => (
                <option key={b.name} value={b.name}>
                  {b.name}
                </option>
              ))}
            </optgroup>
          )}
          {scope && scope !== 'HEAD' && !scopeOptions?.local.some((b) => b.name === scope) && !scopeOptions?.remote.some((b) => b.name === scope) && (
            <option value={scope}>{scope}</option>
          )}
        </Select>
        <Input small placeholder="Text or message" value={text} onChange={(e) => setText(e.target.value)} style={{ width: 180 }} aria-label="Message filter" />
        <Input small placeholder="Author" value={author} onChange={(e) => setAuthor(e.target.value)} style={{ width: 120 }} aria-label="Author filter" />
        <Input
          small
          placeholder="Path"
          value={path}
          onChange={(e) => {
            setPath(e.target.value)
            setLineRange('')
          }}
          style={{ width: 170 }}
          aria-label="Path filter"
        />
        {ranged && (
          <span
            className="wb-badge git-range-chip"
            title={`Commits that changed lines ${lineRange.replace(',', '–')} of ${pathQ}${worktreeLines ? ' (as numbered in the working tree)' : ''}`}
          >
            <TextSelect size={11} />
            {splitPath(pathQ).name}:{lineRange.replace(',', '–')}
            <button type="button" className="git-chip-x" aria-label="Show the whole file's history" title="Whole file" onClick={() => setLineRange('')}>
              <X size={11} />
            </button>
          </span>
        )}
        {(text || author || path || scope !== '') && (
          <IconButton
            size="small"
            icon={X}
            label="Clear filters"
            onClick={() => {
              setText('')
              setAuthor('')
              setPath('')
              setLineRange('')
              setScope('')
            }}
          />
        )}
        <span style={{ flex: 1 }} />
        {log.isFetching && <Spinner size={12} />}
        <span className="wb-xs wb-subtle">
          {commits.length}
          {log.hasNextPage ? '+' : ''} commits
        </span>
        <IconButton size="small" icon={RefreshCw} label="Refresh" onClick={() => void qc.invalidateQueries({ queryKey: gk.all(pid) })} />
      </div>
      {bisecting && <BisectBanner pid={pid} inLog />}
      <div className={`git-log-body${stacked ? ' stacked' : ''}`} ref={bodyRef}>
        <div className="git-log-main">
          <div className="git-log-header">
            <span style={{ width: LANE + 2, flex: 'none' }} />
            <span className="wb-grow">Subject</span>
            <span className="author">Author</span>
            <span className="date">Date</span>
          </div>
          <div
            className="git-log-list"
            ref={listRef}
            tabIndex={0}
            onKeyDown={onKeyDown}
            onScroll={(e) => setView({ top: e.currentTarget.scrollTop, height: e.currentTarget.clientHeight })}
          >
            {log.isLoading && <Loading label="Loading history…" />}
            {log.error && (notRepo ? <EmptyState icon={GitGraph} title="Not a git repository" /> : <ErrorBox error={log.error} onRetry={() => void log.refetch()} />)}
            {!log.isLoading && !log.error && commits.length === 0 && <EmptyState icon={GitGraph} title="No commits match" />}
            <div style={{ height: commits.length * ROW + (log.hasNextPage ? ROW : 0), position: 'relative' }}>
              {commits.slice(first, last).map((c, k) => {
                const i = first + k
                const mark = bisectMark(bis, c.sha)
                const outside = candidates.size > 0 && !candidates.has(c.sha) && !mark
                return (
                  <div
                    key={c.sha}
                    className={`git-log-row${c.sha === selected ? ' selected' : ''}${outside ? ' outside' : ''}`}
                    style={{ top: i * ROW, height: ROW }}
                    onClick={() => setSelected(c.sha)}
                    onDoubleClick={() => openCommit(pid, c.sha)}
                    onContextMenu={(e) => {
                      setSelected(c.sha)
                      showMenu(e, commitMenu(pid, c, checkoutRef, { headSha, bisecting: !!bis?.active, branch: status.data?.branch ?? null }))
                    }}
                    title={`${shortSha(c.sha)} ${c.author} <${c.email}>\n${new Date(c.time).toLocaleString()}\n\n${c.subject}`}
                  >
                    <GraphCell row={graph.rows[i]} merge={c.parents.length > 1} />
                    <span className="subject">
                      {mark && <span className={`git-bis-mark ${mark}`}>{mark === 'bad' || mark === 'good' ? (mark === 'bad' ? bis!.termBad : bis!.termGood) : MARK_LABEL[mark]}</span>}
                      {c.refs.map((r) => (
                        <RefBadge key={`${r.kind}:${r.name}`} r={r} />
                      ))}
                      <span className={c.sha === headSha ? 'text head' : 'text'}>{c.subject}</span>
                    </span>
                    <span className="author">{c.author}</span>
                    <span className="date">{timeAgo(c.time)}</span>
                  </div>
                )
              })}
              {log.hasNextPage && (
                <div className="git-log-row" style={{ top: commits.length * ROW, height: ROW }}>
                  <Spinner size={12} /> <span className="wb-small wb-muted">Loading more…</span>
                </div>
              )}
            </div>
          </div>
        </div>
        {stacked ? (
          <Splitter
            direction="h"
            onResizeStart={() => (sideStart.current = sideH)}
            onResize={(d) => {
              const h = Math.max(120, Math.min(body.h * 0.8, sideStart.current - d))
              setSideHeight(h)
              saveNumber('wb.git.logSideH', h)
            }}
          />
        ) : (
          <Splitter
            direction="v"
            onResizeStart={() => (sideStart.current = sideW)}
            onResize={(d) => {
              const w = Math.max(SIDE_MIN, Math.min(body.w * 0.7, sideStart.current - d))
              setSideWidth(w)
              saveNumber('wb.git.logSide', w)
            }}
          />
        )}
        <div className="git-log-side" style={stacked ? { height: sideH } : { width: sideW }}>
          <CommitDetailsView
            pid={pid}
            sha={selected}
            layout={stacked && body.w >= 480 ? 'split' : 'stacked'}
            onSelectSha={(sha) => (commits.some((c) => c.sha === sha) ? setSelected(sha) : openCommit(pid, sha))}
          />
        </div>
      </div>
    </div>
  )
}
