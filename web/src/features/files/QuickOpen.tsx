// "Go to File" (Ctrl+P): fuzzy search over the project's file list (ranked on the
// server), recent files first, `name:12` opens at a line, `:12` moves the cursor
// in the active editor. Shift+Enter opens to the side.

import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { Command as Cmdk } from 'cmdk'
import { keepPreviousData, useQuery } from '@tanstack/react-query'
import { Clock, CornerDownLeft } from 'lucide-react'
import { useUi } from '@/state/store'
import { Kbd, Spinner } from '@/ui'
import { filesApi } from './api'
import { FileIcon } from './icons'
import { openFile } from './openers'
import { basename, dirname, parseGoto } from './paths'
import { nextSelection, rankItems, type QuickOpenItem } from './quickOpenModel'
import { recentFiles, useActiveEditor, useQuickOpen } from './store'

function Highlighted({ text, positions, offset }: { text: string; positions: number[]; offset: number }) {
  if (!positions.length) return <>{text}</>
  const set = new Set(positions.map((p) => p - offset))
  const out: ReactNode[] = []
  let run = ''
  let runHit = false
  for (let i = 0; i < text.length; i++) {
    const hit = set.has(i)
    if (hit !== runHit && run) {
      out.push(runHit ? <b key={i}>{run}</b> : run)
      run = ''
    }
    runHit = hit
    run += text[i]
  }
  if (run) out.push(runHit ? <b key="end">{run}</b> : run)
  return <>{out}</>
}

export function QuickOpenHost() {
  const { open, initial, hide } = useQuickOpen()
  const projectId = useUi((s) => s.projectId)
  if (!open || !projectId) return null
  return <QuickOpenDialog projectId={projectId} initial={initial} onClose={hide} />
}

function QuickOpenDialog({ projectId, initial, onClose }: { projectId: string; initial: string; onClose: () => void }) {
  const [input, setInput] = useState(initial)
  const [debounced, setDebounced] = useState(initial)
  const [selected, setSelected] = useState('')
  useEffect(() => {
    const t = window.setTimeout(() => setDebounced(input), 60)
    return () => window.clearTimeout(t)
  }, [input])
  const goto = parseGoto(input)
  const q = parseGoto(debounced).query
  const recent = useMemo(() => recentFiles(projectId), [projectId])
  const active = useActiveEditor((s) => s.current)

  const found = useQuery({
    queryKey: ['files', 'find', projectId, q],
    queryFn: ({ signal }) => filesApi.find(projectId, q, 60, signal),
    enabled: q.length > 0,
    placeholderData: keepPreviousData,
    staleTime: 5_000,
    retry: false,
  })

  const items: QuickOpenItem[] = useMemo(() => rankItems(q, found.data?.results ?? [], recent), [q, found.data, recent])

  const pick = useQuickOpen((s) => s.pick)
  const choose = (path: string, side = false) => {
    onClose()
    if (pick) pick.run(path)
    else openFile({ projectId, path, line: goto.line, column: goto.column, side })
  }

  const gotoLineOnly = !goto.query && goto.line !== undefined
  // Keep the selection on a rendered item (see nextSelection). `shownFor` is the query
  // the list belongs to: null while the previous query's results stand in. The list
  // is `settled` once it matches what is typed; Enter before that (typing fast) is
  // held and applied to the results of what was typed, not to the stale list.
  const values = useMemo(() => (gotoLineOnly ? (active ? ['goto-line'] : []) : items.map((it) => it.path)), [gotoLineOnly, active, items])
  const shownFor = gotoLineOnly ? ':' : !q ? '' : (found.data && !found.isPlaceholderData) || found.isError ? q : null
  const settled = gotoLineOnly || (input === debounced && shownFor !== null)
  const lastShown = useRef<string | null>(null)
  const pendingEnter = useRef<{ side: boolean; selected: string } | null>(null)
  const chooseRef = useRef(choose)
  useEffect(() => {
    chooseRef.current = choose
  })
  useEffect(() => {
    const fresh = shownFor !== null && shownFor !== lastShown.current
    if (fresh) lastShown.current = shownFor
    setSelected((s) => nextSelection(s, values, fresh))
    const pending = pendingEnter.current
    if (pending && settled) {
      pendingEnter.current = null
      const path = nextSelection(pending.selected, values, fresh)
      if (path && path !== 'goto-line') chooseRef.current(path, pending.side)
    }
  }, [values, shownFor, settled])

  return (
    <Cmdk.Dialog
      open
      onOpenChange={(o) => !o && onClose()}
      label="Go to file"
      className="wb-palette wb-quickopen"
      overlayClassName="wb-palette-overlay"
      shouldFilter={false}
      loop
      value={selected}
      onValueChange={setSelected}
    >
      <Cmdk.Input
        value={input}
        onValueChange={(v) => {
          pendingEnter.current = null // typing on: that Enter was for other text
          setInput(v)
        }}
        placeholder={pick ? pick.title : 'Go to file (name:line to jump to a line, :line in the current file)'}
        autoFocus
        onKeyDown={(e) => {
          if (e.key === 'Enter' && !settled) {
            e.preventDefault()
            e.stopPropagation()
            pendingEnter.current = { side: e.shiftKey, selected }
            return
          }
          if (e.key === 'Enter' && e.shiftKey && selected && selected !== 'goto-line') {
            e.preventDefault()
            e.stopPropagation()
            choose(selected, true)
          }
        }}
      />
      <Cmdk.List>
        {gotoLineOnly ? (
          <Cmdk.Item
            value="goto-line"
            onSelect={() => {
              onClose()
              active?.goto(goto.line!, goto.column)
            }}
            disabled={!active}
          >
            <CornerDownLeft size={15} />
            <span className="wb-grow">{active ? `Go to line ${goto.line} in ${basename(active.path)}` : 'No active editor'}</span>
          </Cmdk.Item>
        ) : (
          <>
            {!q && items.length > 0 && <div className="wb-quickopen-heading">Recent files</div>}
            {q && !found.isFetching && found.data && !items.length && <Cmdk.Empty>No matching files.</Cmdk.Empty>}
            {!q && !items.length && <div className="wb-quickopen-hint">Type part of a file name or path.</div>}
            {items.map((it) => {
              const name = basename(it.path)
              const dir = dirname(it.path)
              const nameOffset = it.path.length - name.length
              return (
                <Cmdk.Item key={it.path} value={it.path} onSelect={() => choose(it.path)}>
                  <FileIcon path={it.path} />
                  <span className="wb-quickopen-name">
                    <Highlighted text={name} positions={it.positions} offset={nameOffset} />
                  </span>
                  <span className="wb-quickopen-dir wb-ellipsis">
                    <Highlighted text={dir === '/' ? '' : dir} positions={it.positions} offset={0} />
                  </span>
                  {it.recent && <Clock size={12} className="wb-subtle" />}
                </Cmdk.Item>
              )
            })}
          </>
        )}
      </Cmdk.List>
      <div className="wb-quickopen-footer">
        {found.isFetching && <Spinner size={10} />}
        <span className="wb-grow">
          {found.data && q
            ? `${found.data.matched} of ${found.data.indexed} files${found.data.indexTruncated ? ' (index truncated)' : ''}`
            : ''}
        </span>
        <Kbd>Enter</Kbd> open <Kbd>Shift+Enter</Kbd> to the side
      </div>
    </Cmdk.Dialog>
  )
}
