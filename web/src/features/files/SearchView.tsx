// Find in Files (tool window and `search` panel): regex / case / word / file-mask
// toggles, results grouped by file with highlighted matches (virtualized), and
// replace with a server-side preview that is applied only to unchanged files.

import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { keepPreviousData, useQuery } from '@tanstack/react-query'
import { CaseSensitive, ChevronDown, ChevronRight, ChevronsDownUp, PanelRight, RefreshCw, Regex, Replace, TextSearch, WholeWord } from 'lucide-react'
import { ApiError } from '@/api/client'
import type { PanelProps } from '@/shell/types'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { Button, Checkbox, EmptyState, ErrorBox, IconButton, Input, Spinner } from '@/ui'
import { filesApi, type ReplacePreviewFile, type SearchHit, type SearchParams } from './api'
import { FileIcon } from './icons'
import { openFile, openSearchPanel } from './openers'
import { groupRows, previewParts, type SearchRow } from './searchModel'
import { basename, dirname } from './paths'
import { useSearchInputs, useSearchStore } from './store'
import { VirtualList, type VirtualListHandle } from './VirtualList'

const ROW = 22

type Row = SearchRow

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value)
  useEffect(() => {
    const t = window.setTimeout(() => setV(value), ms)
    return () => window.clearTimeout(t)
  }, [value, ms])
  return v
}

export function SearchToolWindow({ projectId }: { projectId: string | null }) {
  if (!projectId) return <EmptyState icon={TextSearch} title="No project selected" />
  return <SearchView key={projectId} projectId={projectId} variant="tool" />
}

export function SearchPanel({ params, setTitle }: PanelProps<{ projectId: string; query?: string }>) {
  const q = useSearchInputs(params.projectId).q
  // Opened with a query (e.g. by an agent through `ui.open`): take it over.
  useEffect(() => {
    if (params.projectId && params.query) useSearchStore.getState().set(params.projectId, { q: params.query })
  }, [params.projectId, params.query])
  useEffect(() => setTitle(q ? `Find: ${q.slice(0, 40)}` : 'Find in Files'), [q, setTitle])
  if (!params.projectId) return <EmptyState title="No project" />
  return <SearchView projectId={params.projectId} variant="panel" />
}

function SearchView({ projectId, variant }: { projectId: string; variant: 'tool' | 'panel' }) {
  const inputs = useSearchInputs(projectId)
  const set = (p: Parameters<ReturnType<typeof useSearchStore.getState>['set']>[1]) => useSearchStore.getState().set(projectId, p)
  const focusTick = useSearchStore((s) => s.focusTick)
  const inputRef = useRef<HTMLInputElement>(null)
  const list = useRef<VirtualListHandle>(null)
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const [selected, setSelected] = useState(-1)
  const [preview, setPreview] = useState<{ files: ReplacePreviewFile[]; total: number; excluded: Set<string> } | null>(null)
  const [busy, setBusy] = useState(false)

  const params: SearchParams = useMemo(
    () => ({ q: inputs.q, regex: inputs.regex, case: inputs.case, word: inputs.word, glob: inputs.glob }),
    [inputs.q, inputs.regex, inputs.case, inputs.word, inputs.glob],
  )
  const debounced = useDebounced(params, 250)
  const max = variant === 'panel' ? 10_000 : 2000
  const query = useQuery({
    queryKey: ['files', 'search', projectId, debounced, max],
    queryFn: ({ signal }) => filesApi.search(projectId, debounced, max, signal),
    enabled: debounced.q.length > 0,
    placeholderData: keepPreviousData,
    retry: false,
    staleTime: 10_000,
  })

  useEffect(() => {
    if (variant !== 'tool' || focusTick === 0) return
    inputRef.current?.focus()
    inputRef.current?.select()
  }, [focusTick, variant])
  useEffect(() => {
    if (variant === 'panel') inputRef.current?.focus()
  }, [variant])

  // A new query invalidates any replace preview.
  useEffect(() => setPreview(null), [debounced])

  const matches = inputs.q ? (query.data?.matches ?? []) : []
  const { rows, files } = useMemo(() => groupRows(matches, collapsed), [matches, collapsed])

  const open = (row: Row) => {
    if (row.kind === 'file') {
      setCollapsed((c) => {
        const n = new Set(c)
        if (n.has(row.path)) n.delete(row.path)
        else n.add(row.path)
        return n
      })
      return
    }
    const h = row.hits[0]
    openFile({ projectId, path: h.path, line: h.line, column: h.column, endColumn: h.endColumn })
  }

  const onListKey = (e: KeyboardEvent) => {
    const move = (i: number) => {
      const n = Math.max(0, Math.min(rows.length - 1, i))
      setSelected(n)
      list.current?.scrollToIndex(n)
    }
    if (e.key === 'ArrowDown') move(selected + 1)
    else if (e.key === 'ArrowUp') {
      if (selected <= 0) inputRef.current?.focus()
      else move(selected - 1)
    } else if (e.key === 'Enter' && rows[selected]) open(rows[selected])
    else if (e.key === 'ArrowLeft' && rows[selected]?.kind === 'file' && !collapsed.has(rows[selected].path)) open(rows[selected])
    else if (e.key === 'ArrowRight' && rows[selected]?.kind === 'file' && collapsed.has(rows[selected].path)) open(rows[selected])
    else return
    e.preventDefault()
  }

  const runPreview = async () => {
    const paths = [...new Set(matches.map((m) => m.path))]
    if (!paths.length) return
    setBusy(true)
    try {
      const r = await filesApi.replace(projectId, { ...params, replacement: inputs.replacement, paths, dryRun: true })
      setPreview({ files: r.files ?? [], total: r.total, excluded: new Set() })
      if (r.conflicts.length) toast('warning', `${r.conflicts.length} file(s) cannot be replaced`, { detail: r.conflicts.slice(0, 4).map((c) => `${c.path}: ${c.message}`).join('\n') })
    } catch (e) {
      toastError(e, 'Replace preview failed')
    } finally {
      setBusy(false)
    }
  }

  const applyReplace = async () => {
    if (!preview) return
    const chosen = preview.files.filter((f) => !preview.excluded.has(f.path))
    const count = chosen.reduce((n, f) => n + f.count, 0)
    const ok = await confirmDialog({
      title: 'Replace in files?',
      message: `Replace ${count} occurrence(s) of “${params.q}” with “${inputs.replacement}” in ${chosen.length} file(s).`,
      confirmLabel: 'Replace',
      danger: true,
    })
    if (!ok) return
    setBusy(true)
    try {
      const r = await filesApi.replace(projectId, {
        ...params,
        replacement: inputs.replacement,
        paths: chosen.map((f) => f.path),
        expected: Object.fromEntries(chosen.map((f) => [f.path, f.etag])),
      })
      setPreview(null)
      toast('success', `Replaced ${r.total} occurrence(s) in ${r.replaced.length} file(s)`)
      if (r.conflicts.length)
        toast('warning', `${r.conflicts.length} file(s) skipped (changed since the preview)`, { detail: r.conflicts.slice(0, 5).map((c) => c.path).join('\n') })
      void query.refetch()
    } catch (e) {
      toastError(e, 'Replace failed')
    } finally {
      setBusy(false)
    }
  }

  const err = query.error
  const badPattern = err instanceof ApiError && err.status === 400
  const data = inputs.q ? query.data : undefined

  return (
    <div className={`wb-fill wb-search ${variant}`}>
      <div className="wb-search-form">
        <div className="wb-search-field">
          <Input
            ref={inputRef}
            small
            placeholder="Find in files"
            value={inputs.q}
            spellCheck={false}
            onChange={(e) => set({ q: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void query.refetch()
              else if (e.key === 'ArrowDown' && rows.length) {
                e.preventDefault()
                setSelected(0)
                list.current?.element()?.focus()
              }
            }}
          />
          <span className="wb-search-toggles">
            <IconButton icon={CaseSensitive} size="small" label="Match case" active={inputs.case} onClick={() => set({ case: !inputs.case })} />
            <IconButton icon={WholeWord} size="small" label="Words" active={inputs.word} onClick={() => set({ word: !inputs.word })} />
            <IconButton icon={Regex} size="small" label="Regex" active={inputs.regex} onClick={() => set({ regex: !inputs.regex })} />
          </span>
        </div>
        <IconButton icon={Replace} size="small" label="Replace" active={inputs.replaceOpen} onClick={() => set({ replaceOpen: !inputs.replaceOpen })} />
      </div>
      {inputs.replaceOpen && (
        <div className="wb-search-form">
          <Input small placeholder={inputs.regex ? 'Replace with ($1 for groups)' : 'Replace with'} value={inputs.replacement} spellCheck={false} onChange={(e) => set({ replacement: e.target.value })} />
          <Button size="small" disabled={!matches.length || busy} loading={busy && !preview} onClick={() => void runPreview()}>
            Preview
          </Button>
        </div>
      )}
      <div className="wb-search-form">
        <Input small placeholder="File mask: *.rs, !tests/**" value={inputs.glob} spellCheck={false} onChange={(e) => set({ glob: e.target.value })} />
      </div>
      <div className="wb-search-status">
        {query.isFetching && <Spinner size={10} />}
        <span className="wb-grow wb-ellipsis">
          {badPattern ? (
            <span className="wb-danger">{(err as Error).message}</span>
          ) : data ? (
            <>
              {data.matches.length} match{data.matches.length === 1 ? '' : 'es'} in {files} file{files === 1 ? '' : 's'}
              {data.truncated ? <span className="wb-warning"> · {data.timedOut ? 'timed out' : 'more not shown'}</span> : null}
              <span className="wb-subtle"> · {data.filesSearched} searched · {data.elapsedMs} ms</span>
            </>
          ) : (
            <span className="wb-subtle">Type to search the project (gitignored files are skipped)</span>
          )}
        </span>
        <IconButton icon={RefreshCw} size="small" label="Search again" disabled={!inputs.q} onClick={() => void query.refetch()} />
        <IconButton
          icon={ChevronsDownUp}
          size="small"
          label="Collapse all"
          disabled={!matches.length}
          onClick={() => setCollapsed((c) => (c.size ? new Set() : new Set(matches.map((m) => m.path))))}
        />
        {variant === 'tool' && <IconButton icon={PanelRight} size="small" label="Open in editor area" onClick={() => openSearchPanel(projectId)} />}
      </div>
      {err && !badPattern ? (
        <ErrorBox error={err} onRetry={() => void query.refetch()} />
      ) : preview ? (
        <ReplacePreview
          projectId={projectId}
          preview={preview}
          busy={busy}
          onToggle={(path) =>
            setPreview((p) => {
              if (!p) return p
              const ex = new Set(p.excluded)
              if (ex.has(path)) ex.delete(path)
              else ex.add(path)
              return { ...p, excluded: ex }
            })
          }
          onApply={() => void applyReplace()}
          onCancel={() => setPreview(null)}
        />
      ) : inputs.q && data && !data.matches.length && !query.isFetching ? (
        <EmptyState title="No matches">{data.sensitiveSkipped ? `${data.sensitiveSkipped} sensitive file(s) were not searched.` : null}</EmptyState>
      ) : (
        <VirtualList
          ref={list}
          className="wb-search-results"
          tabIndex={0}
          count={rows.length}
          rowHeight={ROW}
          onKeyDown={onListKey}
          renderRow={(i) => <ResultRow row={rows[i]} selected={i === selected} onClick={() => (setSelected(i), open(rows[i]))} />}
        />
      )}
    </div>
  )
}

function ResultRow({ row, selected, onClick }: { row: Row; selected: boolean; onClick: () => void }) {
  if (row.kind === 'file') {
    const dir = dirname(row.path)
    return (
      <div className={`wb-search-row file${selected ? ' selected' : ''}`} onClick={onClick} title={row.path}>
        {row.collapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
        <FileIcon path={row.path} />
        <span className="name">{basename(row.path)}</span>
        <span className="dir wb-ellipsis">{dir && dir !== '/' ? dir : ''}</span>
        <span className="count">{row.count}</span>
      </div>
    )
  }
  return (
    <div className={`wb-search-row line${selected ? ' selected' : ''}`} onClick={onClick}>
      <span className="ln">{row.line}</span>
      <span className="preview wb-ellipsis">{highlightHits(row.hits)}</span>
    </div>
  )
}

/** The preview line with every match on it highlighted. */
function highlightHits(hits: SearchHit[]): ReactNode {
  const { cut, segments } = previewParts(hits)
  return (
    <>
      {cut && '…'}
      {segments.map((seg, i) => (seg.hit ? <mark key={i}>{seg.text}</mark> : <span key={i}>{seg.text}</span>))}
    </>
  )
}

function ReplacePreview({
  projectId,
  preview,
  busy,
  onToggle,
  onApply,
  onCancel,
}: {
  projectId: string
  preview: { files: ReplacePreviewFile[]; total: number; excluded: Set<string> }
  busy: boolean
  onToggle: (path: string) => void
  onApply: () => void
  onCancel: () => void
}) {
  const chosen = preview.files.filter((f) => !preview.excluded.has(f.path))
  return (
    <div className="wb-fill">
      <div className="wb-search-status">
        <span className="wb-grow">
          {preview.total} replacement(s) in {preview.files.length} file(s)
        </span>
        <Button size="small" onClick={onCancel}>
          Cancel
        </Button>
        <Button size="small" variant="primary" disabled={!chosen.length} loading={busy} onClick={onApply}>
          Replace in {chosen.length} file{chosen.length === 1 ? '' : 's'}
        </Button>
      </div>
      <div className="wb-scroll wb-replace-preview">
        {preview.files.map((f) => (
          <div key={f.path} className={preview.excluded.has(f.path) ? 'excluded' : ''}>
            <div className="wb-search-row file">
              <Checkbox checked={!preview.excluded.has(f.path)} onChange={() => onToggle(f.path)} />
              <FileIcon path={f.path} />
              <span className="name">{basename(f.path)}</span>
              <span className="dir wb-ellipsis">{dirname(f.path) === '/' ? '' : dirname(f.path)}</span>
              <span className="count">{f.count}</span>
            </div>
            {f.lines.map((l) => (
              <div key={l.line} className="wb-replace-line" onClick={() => openFile({ projectId, path: f.path, line: l.line })}>
                <span className="ln">{l.line}</span>
                <div className="wb-grow">
                  <div className="before wb-ellipsis">{l.before.trim()}</div>
                  <div className="after wb-ellipsis">{l.after.trim()}</div>
                </div>
              </div>
            ))}
          </div>
        ))}
      </div>
    </div>
  )
}
