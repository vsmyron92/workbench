// CLion's Recent Files (Ctrl+E; Ctrl+E again: changed files only) and Recent
// Locations (Ctrl+Shift+E: the places the caret has been, with their code).
// Typing filters; Enter opens, Shift+Enter opens to the side.

import { useMemo, useState } from 'react'
import { Command as Cmdk } from 'cmdk'
import { useQuery } from '@tanstack/react-query'
import { create } from 'zustand'
import { api } from '@/api/client'
import { useUi } from '@/state/store'
import { Kbd } from '@/ui'
import { bufferKey, getModel } from './buffers'
import { FileIcon } from './icons'
import { navHistory, type NavEntry } from './navHistory'
import { openFile } from './openers'
import { basename, dirname } from './paths'
import { isScratch, SCRATCH_ID } from './scratchStore'
import { recentFiles, useActiveEditor } from './store'

export const useRecentPopup = create<{
  which: 'files' | 'locations' | null
  changedOnly: boolean
  show: (which: 'files' | 'locations') => void
  hide: () => void
}>()((set, get) => ({
  which: null,
  changedOnly: false,
  show: (which) => {
    // Ctrl+E while Recent Files is open: toggle "changed files only", as in CLion.
    if (which === 'files' && get().which === 'files') set({ changedOnly: !get().changedOnly })
    else set({ which, changedOnly: false })
  },
  hide: () => set({ which: null }),
}))

/** Letters of `query` in order in `text` (case-insensitive). */
export function fuzzyMatch(text: string, query: string): boolean {
  const q = query.toLowerCase().replace(/\s+/g, '')
  const t = text.toLowerCase()
  let j = 0
  for (let i = 0; i < t.length && j < q.length; i++) if (t[i] === q[j]) j++
  return j === q.length
}

export function RecentPopupHost() {
  const which = useRecentPopup((s) => s.which)
  const projectId = useUi((s) => s.projectId)
  if (!which || !projectId) return null
  return which === 'files' ? <RecentFiles projectId={projectId} /> : <RecentLocations projectId={projectId} />
}

interface GitStatusLite {
  files: { path: string }[]
}

/** Recent Files values of scratch files (the others are project paths). */
const SCRATCH_PREFIX = 'scratch:'

function RecentFiles({ projectId }: { projectId: string }) {
  const { changedOnly, hide } = useRecentPopup()
  const [q, setQ] = useState('')
  const active = useActiveEditor((s) => s.current)
  // The git slice's shared status query (docs/ARCHITECTURE.md, cross-slice contracts).
  const status = useQuery({
    queryKey: ['git', projectId, 'status'],
    queryFn: ({ signal }) => api.get<GitStatusLite>(`/api/projects/${encodeURIComponent(projectId)}/git/status`, undefined, signal),
    enabled: changedOnly,
    retry: false,
  })
  const paths = useMemo(() => {
    const recent = recentFiles(projectId)
    if (!changedOnly) return recent
    const changed = new Set((status.data?.files ?? []).map((f) => f.path))
    // Recent changed files first, then the other changed ones.
    return [...recent.filter((p) => changed.has(p)), ...[...changed].filter((p) => !recent.includes(p))]
  }, [projectId, changedOnly, status.data])
  const shown = useMemo(() => (q.trim() ? paths.filter((p) => fuzzyMatch(p, q)) : paths), [paths, q])
  // Recent scratch files follow, as in CLion (not among changed files: they have no VCS).
  const scratches = useMemo(() => {
    if (changedOnly || isScratch(projectId)) return []
    const all = recentFiles(SCRATCH_ID)
    return q.trim() ? all.filter((p) => fuzzyMatch(p, q)) : all
  }, [changedOnly, projectId, q])
  // Like CLion: the previous file is selected, so Ctrl+E Enter switches back.
  const initial = !q.trim() && shown[0] && active && active.projectId === projectId && shown[0] === active.path ? shown[1] : shown[0]
  const [selected, setSelected] = useState<string | undefined>(undefined)
  const value = selected !== undefined && (shown.includes(selected) || scratches.includes(selected.slice(SCRATCH_PREFIX.length))) ? selected : (initial ?? '')

  const open = (value: string, side: boolean) => {
    hide()
    if (value.startsWith(SCRATCH_PREFIX)) openFile({ projectId: SCRATCH_ID, path: value.slice(SCRATCH_PREFIX.length), side })
    else openFile({ projectId, path: value, side })
  }

  return (
    <Cmdk.Dialog
      open
      onOpenChange={(o) => !o && hide()}
      label="Recent files"
      className="wb-palette wb-quickopen"
      overlayClassName="wb-palette-overlay"
      shouldFilter={false}
      loop
      value={value}
      onValueChange={setSelected}
    >
      <div className="wb-quickopen-heading wb-recent-heading">
        {changedOnly ? 'Recently changed files' : 'Recent files'}
        <span className="wb-grow" />
        <span className="wb-subtle">
          <Kbd>Ctrl+E</Kbd> {changedOnly ? 'all recent files' : 'changed only'}
        </span>
      </div>
      <Cmdk.Input
        value={q}
        onValueChange={setQ}
        placeholder="Type to filter"
        autoFocus
        onKeyDown={(e) => {
          if (e.key === 'Enter' && e.shiftKey && value) {
            e.preventDefault()
            open(value, true)
          }
        }}
      />
      <Cmdk.List>
        {!shown.length && !scratches.length && <Cmdk.Empty>{changedOnly ? (status.isLoading ? 'Loading…' : 'No changed files.') : 'No recent files.'}</Cmdk.Empty>}
        {shown.map((p) => (
          <Cmdk.Item key={p} value={p} onSelect={() => open(p, false)}>
            <FileIcon path={p} />
            <span className="wb-quickopen-name">{basename(p)}</span>
            <span className="wb-quickopen-dir wb-ellipsis">{dirname(p) === '/' ? '' : dirname(p)}</span>
          </Cmdk.Item>
        ))}
        {scratches.length > 0 && (
          <Cmdk.Group heading="Scratches">
            {scratches.map((p) => (
              <Cmdk.Item key={`s:${p}`} value={SCRATCH_PREFIX + p} onSelect={() => open(SCRATCH_PREFIX + p, false)}>
                <FileIcon path={p} />
                <span className="wb-quickopen-name">{basename(p)}</span>
                <span className="wb-quickopen-dir wb-ellipsis">scratch</span>
              </Cmdk.Item>
            ))}
          </Cmdk.Group>
        )}
      </Cmdk.List>
      <div className="wb-quickopen-footer">
        <span className="wb-grow" />
        <Kbd>Enter</Kbd> open <Kbd>Shift+Enter</Kbd> to the side
      </div>
    </Cmdk.Dialog>
  )
}

const keyOf = (e: NavEntry) => `${e.path}:${e.line}:${e.at}`

function snippet(e: NavEntry): string[] {
  const m = getModel(bufferKey(e.projectId, e.path))
  if (!m || m.isDisposed() || e.line > m.getLineCount()) return []
  const lines = [m.getLineContent(e.line)]
  if (e.line < m.getLineCount()) lines.push(m.getLineContent(e.line + 1))
  return lines
}

function RecentLocations({ projectId }: { projectId: string }) {
  const hide = useRecentPopup((s) => s.hide)
  const [q, setQ] = useState('')
  const [selected, setSelected] = useState('')
  const all = useMemo(
    () =>
      navHistory
        .locations()
        .filter((e) => e.projectId === projectId)
        .map((e) => ({ e, code: snippet(e) })),
    [projectId],
  )
  const shown = useMemo(() => {
    const t = q.trim().toLowerCase()
    if (!t) return all
    return all.filter(({ e, code }) => fuzzyMatch(e.path, t) || code.some((l) => l.toLowerCase().includes(t)))
  }, [all, q])
  const value = shown.some((x) => keyOf(x.e) === selected) ? selected : shown[0] ? keyOf(shown[0].e) : ''
  const open = (e: NavEntry, side: boolean) => {
    hide()
    openFile({ projectId: e.projectId, path: e.path, line: e.line, column: e.column, side })
  }
  // Common indentation of a snippet goes, so short lines stay readable.
  const trimmed = (code: string[]) => {
    const indent = Math.min(...code.filter((l) => l.trim()).map((l) => l.length - l.trimStart().length), 1000)
    return code.map((l) => l.slice(Number.isFinite(indent) ? indent : 0))
  }

  return (
    <Cmdk.Dialog
      open
      onOpenChange={(o) => !o && hide()}
      label="Recent locations"
      className="wb-palette wb-quickopen wb-recent-locations"
      overlayClassName="wb-palette-overlay"
      shouldFilter={false}
      loop
      value={value}
      onValueChange={setSelected}
    >
      <div className="wb-quickopen-heading wb-recent-heading">Recent locations</div>
      <Cmdk.Input
        value={q}
        onValueChange={setQ}
        placeholder="Filter by file or code"
        autoFocus
        onKeyDown={(ev) => {
          const hit = shown.find((x) => keyOf(x.e) === value)
          if (ev.key === 'Enter' && ev.shiftKey && hit) {
            ev.preventDefault()
            open(hit.e, true)
          }
        }}
      />
      <Cmdk.List>
        {!shown.length && <Cmdk.Empty>{all.length ? 'Nothing matches.' : 'No places yet: move around the code and they show up here.'}</Cmdk.Empty>}
        {shown.map(({ e, code }) => (
          <Cmdk.Item key={keyOf(e)} value={keyOf(e)} onSelect={() => open(e, false)} className="wb-recent-location">
            <div className="wb-recent-location-head">
              <FileIcon path={e.path} />
              <span className="wb-quickopen-name">
                {basename(e.path)}:{e.line}
              </span>
              <span className="wb-quickopen-dir wb-ellipsis">{dirname(e.path) === '/' ? '' : dirname(e.path)}</span>
            </div>
            {code.length > 0 && <pre className="wb-recent-location-code">{trimmed(code).join('\n')}</pre>}
          </Cmdk.Item>
        ))}
      </Cmdk.List>
      <div className="wb-quickopen-footer">
        <span className="wb-grow" />
        <Kbd>Enter</Kbd> open <Kbd>Shift+Enter</Kbd> to the side
      </div>
    </Cmdk.Dialog>
  )
}
