// The Commit window grouped by changelist (JetBrains): one node per changelist (the
// active one in bold), conflicts and unversioned files apart, a checkbox per file
// and list for what the next commit includes (partially included files come from
// the line checkboxes of their HEAD → working tree diff). Files move between lists
// by drag and drop or the context menu.

import { useEffect, useMemo, useRef, useState } from 'react'
import {
  ArrowRightLeft,
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  FileCode,
  FileDiff,
  FileText,
  GitCompareArrows,
  GitMerge,
  History,
  ListPlus,
  PackagePlus,
  Pencil,
  Plus,
  Star,
  Trash2,
  Undo2,
} from 'lucide-react'
import { IconButton, showMenu, type MenuEntry } from '@/ui'
import { useChangelists } from './api'
import {
  askReview,
  compareWithBranch,
  copyText,
  deleteChangelist,
  editChangelist,
  moveToChangelist,
  openConflict,
  openDiff,
  openFile,
  openGitLog,
  rollback,
  setActiveChangelist,
  shelveChanges,
  stagePaths,
} from './actions'
import { defaultIncluded, groupChangelists, groupState, headCode, type ChangelistGroups } from './changelistView'
import { splitPath, statusClass, statusLabel } from './logic'
import { useDrafts, useInclusion, useProjectInclusion } from './store'
import type { Changelist, GitStatus, GitStatusFile } from './types'

const DRAG_TYPE = 'application/x-workbench-changes'
const RENDER_CAP = 1000

type NodeId = string // 'list:<id>' | 'conflicts' | 'unversioned'
type Row = { node: NodeId; file: GitStatusFile }
const rowKey = (node: NodeId, path: string) => `${node}\u0000${path}`

/** Which files (and lines) the changelist view commits. */
export function useCommitSelection(pid: string, st: GitStatus | undefined, enabled = true) {
  const cls = useChangelists(pid, enabled)
  const groups = useMemo(() => groupChangelists(st?.files ?? [], cls.data), [st, cls.data])
  const inc = useProjectInclusion(pid)
  const included = useMemo(() => {
    const live = new Set([...groups.lists.flatMap((g) => g.files.map((f) => f.path)), ...groups.unversioned.map((f) => f.path)])
    const base = inc.included ? new Set(inc.included) : defaultIncluded(groups)
    return new Set([...base].filter((p) => live.has(p)))
  }, [groups, inc.included])
  const partial = useMemo(() => new Set(Object.keys(inc.partial).filter((p) => included.has(p))), [inc.partial, included])
  return { cls, groups, included, partial, partialSel: inc.partial }
}

export function ChangelistView({ pid, st, onSelection }: { pid: string; st: GitStatus; onSelection?: (paths: string[]) => void }) {
  const { cls, groups, included, partial } = useCommitSelection(pid, st)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [anchor, setAnchor] = useState<string | null>(null)
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({})
  const [dropOn, setDropOn] = useState<string | null>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const setIncluded = (paths: Iterable<string>) => useInclusion.getState().setIncluded(pid, [...new Set(paths)])
  const activeList = groups.lists.find((g) => g.list.active)?.list ?? groups.lists[0]?.list

  const nodes = useMemo(() => {
    const v: { id: NodeId; files: GitStatusFile[] }[] = []
    if (groups.conflicts.length) v.push({ id: 'conflicts', files: groups.conflicts })
    for (const g of groups.lists) v.push({ id: `list:${g.list.id}`, files: g.files })
    if (groups.unversioned.length) v.push({ id: 'unversioned', files: groups.unversioned })
    return v
  }, [groups])

  const flat = useMemo(() => {
    const v: Row[] = []
    for (const n of nodes) if (!collapsed[n.id]) for (const f of n.files.slice(0, RENDER_CAP)) v.push({ node: n.id, file: f })
    return v
  }, [nodes, collapsed])

  useEffect(() => {
    setSelected((prev) => {
      const live = new Set(flat.map((r) => rowKey(r.node, r.file.path)))
      const next = new Set([...prev].filter((k) => live.has(k)))
      return next.size === prev.size ? prev : next
    })
  }, [flat])

  const selectedRows = (fallback?: Row): Row[] => {
    const keys = fallback && !selected.has(rowKey(fallback.node, fallback.file.path)) ? [rowKey(fallback.node, fallback.file.path)] : [...selected]
    return flat.filter((r) => keys.includes(rowKey(r.node, r.file.path)))
  }
  useEffect(() => {
    onSelection?.(flat.filter((r) => selected.has(rowKey(r.node, r.file.path))).map((r) => r.file.path))
  }, [selected, flat, onSelection])

  const openRow = (r: Row, preview: boolean) => {
    if (r.node === 'conflicts') openConflict(pid, r.file.path)
    else if (r.node === 'unversioned') openDiff(pid, r.file.path, 'working', { preview })
    else openDiff(pid, r.file.path, 'compare', { base: 'HEAD', head: '', preview, oldPath: r.file.origPath })
  }

  const toggleFiles = (paths: string[], on?: boolean) => {
    const next = new Set(included)
    const all = paths.every((p) => included.has(p) && !partial.has(p))
    const want = on ?? !all
    for (const p of paths) {
      if (want) next.add(p)
      else next.delete(p)
      if (partial.has(p)) useInclusion.getState().setPartial(pid, p, null)
    }
    setIncluded(next)
  }

  const onRowClick = (e: React.MouseEvent, r: Row) => {
    const k = rowKey(r.node, r.file.path)
    if (e.ctrlKey || e.metaKey) {
      setSelected((prev) => {
        const n = new Set(prev)
        if (n.has(k)) n.delete(k)
        else n.add(k)
        return n
      })
      setAnchor(k)
      return
    }
    if (e.shiftKey && anchor) {
      const a = flat.findIndex((x) => rowKey(x.node, x.file.path) === anchor)
      const b = flat.findIndex((x) => rowKey(x.node, x.file.path) === k)
      if (a >= 0 && b >= 0) {
        const [lo, hi] = a < b ? [a, b] : [b, a]
        setSelected(new Set(flat.slice(lo, hi + 1).map((x) => rowKey(x.node, x.file.path))))
        return
      }
    }
    setSelected(new Set([k]))
    setAnchor(k)
    openRow(r, true)
  }

  const lists = groups.lists.map((g) => g.list)
  const moveEntries = (paths: string[], except?: string): MenuEntry[] => [
    ...lists
      .filter((l) => l.id !== except)
      .map<MenuEntry>((l) => ({ label: `Move to “${l.name}”`, icon: ArrowRightLeft, run: () => void moveToChangelist(pid, paths, l.id) })),
    { label: 'Move to New Changelist…', icon: ListPlus, run: () => editChangelist(pid, undefined, paths) },
  ]

  const fileMenu = (e: React.MouseEvent, r: Row) => {
    const k = rowKey(r.node, r.file.path)
    if (!selected.has(k)) {
      setSelected(new Set([k]))
      setAnchor(k)
    }
    const rows = selectedRows(r)
    const paths = rows.map((x) => x.file.path)
    const single = rows.length === 1
    const tracked = rows.filter((x) => x.node.startsWith('list:')).map((x) => x.file.path)
    const items: MenuEntry[] = []
    if (r.node === 'conflicts') items.push({ label: 'Resolve…', icon: GitMerge, run: () => openConflict(pid, r.file.path) }, 'separator')
    else items.push({ label: 'Show Diff', icon: FileDiff, shortcut: 'Enter', run: () => rows.forEach((x) => openRow(x, false)) })
    items.push({ label: 'Open File', icon: FileCode, shortcut: 'F4', disabled: r.file.worktree === 'D', run: () => paths.forEach((p) => openFile(pid, p)) }, 'separator')
    if (r.node === 'unversioned') items.push({ label: 'Add to VCS', icon: Plus, run: () => void stagePaths(pid, paths) })
    if (tracked.length) items.push(...moveEntries(tracked, r.node.slice(5)))
    if (r.node !== 'conflicts') {
      items.push(
        'separator',
        { label: 'Shelve Changes…', icon: PackagePlus, run: () => shelveChanges(pid, paths, { name: single ? splitPath(paths[0]).name : '' }) },
        { label: 'Rollback…', icon: Undo2, shortcut: 'Del', danger: true, run: () => void rollback(pid, paths, 'all') },
      )
    }
    items.push('separator')
    if (single) {
      items.push(
        { label: 'Show History', icon: History, run: () => openGitLog(pid, { path: r.file.path }) },
        { label: 'Compare with Branch…', icon: GitCompareArrows, disabled: r.node === 'unversioned', run: () => compareWithBranch(pid, r.file.path) },
      )
    }
    items.push(
      { label: single ? 'Copy Path' : `Copy ${rows.length} Paths`, icon: Copy, run: () => void copyText(paths.join('\n')) },
      { label: 'Ask Agent to Review', icon: Bot, disabled: !single, run: () => void askReview(pid, r.file.path, false) },
    )
    showMenu(e, items)
  }

  const listMenu = (e: { clientX: number; clientY: number; preventDefault?: () => void }, l: Changelist, files: string[]) => {
    showMenu(e, [
      { label: 'Set Active Changelist', icon: Star, disabled: l.active, run: () => void setActiveChangelist(pid, l.id) },
      {
        label: 'Commit Changelist…',
        icon: Check,
        disabled: !files.length,
        run: () => {
          setIncluded(files)
          for (const p of partial) useInclusion.getState().setPartial(pid, p, null)
          useDrafts.getState().focus()
        },
      },
      { label: 'Shelve Changes…', icon: PackagePlus, disabled: !files.length, run: () => shelveChanges(pid, files, { changelist: l.id, name: l.name }) },
      'separator',
      { label: 'New Changelist…', icon: ListPlus, run: () => editChangelist(pid) },
      { label: 'Edit Changelist…', icon: Pencil, run: () => editChangelist(pid, l) },
      { label: 'Delete Changelist…', icon: Trash2, danger: true, disabled: lists.length < 2, run: () => void deleteChangelist(pid, l, activeList?.name ?? '') },
    ])
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!flat.length) return
    const cur = flat.findIndex((r) => selected.has(rowKey(r.node, r.file.path)))
    const at = (i: number) => {
      const r = flat[Math.max(0, Math.min(flat.length - 1, i))]
      const k = rowKey(r.node, r.file.path)
      setSelected(new Set([k]))
      setAnchor(k)
      listRef.current?.querySelector(`[data-key="${CSS.escape(k)}"]`)?.scrollIntoView({ block: 'nearest' })
    }
    const rows = selectedRows()
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      at(cur + 1)
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      at(cur < 0 ? 0 : cur - 1)
    } else if (e.key === 'Enter' && rows.length) {
      e.preventDefault()
      rows.forEach((r) => openRow(r, false))
    } else if (e.key === ' ' && rows.length) {
      e.preventDefault()
      toggleFiles(rows.filter((r) => r.node !== 'conflicts').map((r) => r.file.path))
    } else if (e.key === 'Delete' && rows.length) {
      e.preventDefault()
      void rollback(pid, rows.filter((r) => r.node !== 'conflicts').map((r) => r.file.path), 'all')
    } else if (e.key === 'F4' && rows.length) {
      e.preventDefault()
      rows.forEach((r) => openFile(pid, r.file.path))
    }
  }

  const dragPaths = (r: Row) => {
    const rows = selected.has(rowKey(r.node, r.file.path)) ? selectedRows() : [r]
    return rows.filter((x) => x.node.startsWith('list:')).map((x) => x.file.path)
  }

  return (
    <div className="git-changes" ref={listRef} tabIndex={0} onKeyDown={onKeyDown}>
      {nodes.map((n) => {
        const isList = n.id.startsWith('list:')
        const list = isList ? lists.find((l) => `list:${l.id}` === n.id)! : null
        const paths = n.files.map((f) => f.path)
        const state = groupState(paths, included, partial)
        const title = n.id === 'conflicts' ? 'Merge Conflicts' : n.id === 'unversioned' ? 'Unversioned Files' : list!.name
        return (
          <div key={n.id}>
            <div
              className={`git-section git-cl-head${dropOn === n.id ? ' drop' : ''}${list?.active ? ' active' : ''}`}
              onClick={() => setCollapsed((c) => ({ ...c, [n.id]: !c[n.id] }))}
              onContextMenu={(e) => {
                if (!list) return
                e.stopPropagation()
                listMenu(e, list, paths)
              }}
              onDragOver={(e) => {
                if (!isList || !e.dataTransfer.types.includes(DRAG_TYPE)) return
                e.preventDefault()
                setDropOn(n.id)
              }}
              onDragLeave={() => setDropOn((d) => (d === n.id ? null : d))}
              onDrop={(e) => {
                setDropOn(null)
                if (!list) return
                try {
                  const moved = JSON.parse(e.dataTransfer.getData(DRAG_TYPE)) as string[]
                  e.preventDefault()
                  void moveToChangelist(pid, moved, list.id)
                } catch {
                  /* not ours */
                }
              }}
              title={list?.comment || undefined}
            >
              {collapsed[n.id] ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
              {n.id !== 'conflicts' && (
                <TriCheck
                  state={state}
                  disabled={!paths.length}
                  label={`Include ${title} in the commit`}
                  onChange={() => toggleFiles(paths)}
                />
              )}
              <span className={n.id === 'conflicts' ? 'git-c-conflict' : n.id === 'unversioned' ? 'git-c-untracked' : 'name'}>{title}</span>
              {list?.active && <span className="git-cl-active">active</span>}
              <span className="count">
                {n.files.length} file{n.files.length === 1 ? '' : 's'}
              </span>
              {list && (
                <span className="actions" onClick={(e) => e.stopPropagation()}>
                  {!list.active && <IconButton size="small" icon={Star} label="Set active" onClick={() => void setActiveChangelist(pid, list.id)} />}
                  <IconButton size="small" icon={PackagePlus} label="Shelve changes…" disabled={!paths.length} onClick={() => shelveChanges(pid, paths, { changelist: list.id, name: list.name })} />
                  <IconButton
                    size="small"
                    icon={ChevronDown}
                    label="More"
                    onClick={(e) => {
                      const b = (e.currentTarget as HTMLElement).getBoundingClientRect()
                      listMenu({ clientX: b.left, clientY: b.bottom }, list, paths)
                    }}
                  />
                </span>
              )}
            </div>
            {!collapsed[n.id] &&
              n.files.slice(0, RENDER_CAP).map((f) => {
                const k = rowKey(n.id, f.path)
                const code = n.id === 'conflicts' ? 'U' : headCode(f)
                const { name, dir } = splitPath(f.path)
                const renamed = f.origPath && (f.index === 'R' || f.index === 'C') ? f.origPath : null
                return (
                  <div
                    key={f.path}
                    className={`git-row git-cl-row${selected.has(k) ? ' selected' : ''}`}
                    data-key={k}
                    draggable={isList}
                    onDragStart={(e) => {
                      const moved = dragPaths({ node: n.id, file: f })
                      e.dataTransfer.effectAllowed = 'move'
                      e.dataTransfer.setData(DRAG_TYPE, JSON.stringify(moved))
                      e.dataTransfer.setData('text/plain', moved.join('\n'))
                    }}
                    onClick={(e) => onRowClick(e, { node: n.id, file: f })}
                    onDoubleClick={() => openRow({ node: n.id, file: f }, false)}
                    onContextMenu={(e) => fileMenu(e, { node: n.id, file: f })}
                    title={`${statusLabel(code)}: ${renamed ? `${renamed} → ` : ''}${f.path}`}
                  >
                    {n.id !== 'conflicts' && (
                      <TriCheck
                        state={included.has(f.path) ? (partial.has(f.path) ? 'some' : 'all') : 'none'}
                        label={`Include ${f.path} in the commit`}
                        onChange={() => toggleFiles([f.path])}
                      />
                    )}
                    <FileText size={14} className="icon" />
                    <span className={`name ${statusClass(code)}`}>{name}</span>
                    <span className="dir">{renamed ? `${dir}${dir ? ' ' : ''}← ${renamed}` : dir}</span>
                    {partial.has(f.path) && <span className="git-cl-partial">partial</span>}
                    <span className={`letter ${statusClass(code)}`}>{code === '?' ? 'U' : code}</span>
                    <span className="actions" onClick={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()}>
                      {n.id === 'conflicts' ? (
                        <IconButton size="small" icon={GitMerge} label="Resolve…" onClick={() => openConflict(pid, f.path)} />
                      ) : (
                        <IconButton size="small" icon={Undo2} label="Rollback…" onClick={() => void rollback(pid, [f.path], 'all')} />
                      )}
                    </span>
                  </div>
                )
              })}
            {!collapsed[n.id] && n.files.length > RENDER_CAP && <div className="git-more">{n.files.length - RENDER_CAP} more files not shown</div>}
            {!collapsed[n.id] && isList && !n.files.length && <div className="git-more">No changes{list?.active ? ': new changes go here' : ''}</div>}
          </div>
        )
      })}
      {cls.error && <div className="git-more wb-warning">Changelists are unavailable: {(cls.error as Error).message}</div>}
      <NoChanges groups={groups} branch={st.branch} />
    </div>
  )
}

function NoChanges({ groups, branch }: { groups: ChangelistGroups; branch: string | null }) {
  const total = groups.conflicts.length + groups.unversioned.length + groups.lists.reduce((n, g) => n + g.files.length, 0)
  if (total) return null
  return (
    <div className="wb-empty" style={{ height: 'auto', padding: '18px 12px' }}>
      <Check size={22} className="icon" />
      <div className="wb-small">No local changes{branch ? ` on ${branch}` : ''}</div>
    </div>
  )
}

/** A checkbox with an indeterminate state (partly included). */
export function TriCheck({ state, onChange, label, disabled }: { state: 'all' | 'some' | 'none'; onChange: () => void; label: string; disabled?: boolean }) {
  const ref = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = state === 'some'
  }, [state])
  return (
    <input
      ref={ref}
      type="checkbox"
      className="git-tri"
      checked={state === 'all'}
      disabled={disabled}
      aria-label={label}
      title={label}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onChange={onChange}
    />
  )
}
