// The "Files" tool window: CLion's Project view. A lazy tree (one directory per
// request) with VCS colours, gitignored files in the ignored colour, a filter,
// keyboard navigation, a context menu, and drag and drop (paths out to agent
// terminals, OS files in as uploads, tree items onto folders to move them).

import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent, type KeyboardEvent, type MouseEvent } from 'react'
import { ChevronDown, ChevronRight, ChevronsDownUp, Crosshair, FilePlus, FolderGit2, FolderPlus, NotebookPen, RefreshCw, X } from 'lucide-react'
import { toast } from '@/shell/actions'
import { EmptyState, ErrorBox, IconButton, Input, showMenu, Spinner } from '@/ui'
import type { FileEntry } from './api'
import { deleteEntry, moveEntry, newEntry, renameEntry, treeMenu, uploadFiles, type Target } from './fileActions'
import { useProjectSummary, useVcsIndex } from './hooks'
import { FileIcon } from './icons'
import { absolutePath, openFile, revealInTree, revealRequests } from './openers'
import { dirname } from './paths'
import { chooseScratchKind } from './scratches'
import { isScratch, SCRATCH_ID, useFilesView } from './scratchStore'
import { useActiveEditor, useTreeStore, type TreeState } from './store'
import { loadDir, refreshAll } from './treeLoader'
import { flattenTree, type TreeRow } from './treeModel'
import { vcsKindOf, type VcsIndex } from './vcs'
import { VirtualList, type VirtualListHandle } from './VirtualList'

const ROW = 22
const DRAG_MIME = 'application/x-workbench-path'
const INTERNAL_MIME = 'application/x-workbench-file'

export function FilesToolWindow({ projectId }: { projectId: string | null }) {
  // CLion's view selector: the project, or the scratch files (the same for every project).
  const scratches = useFilesView((s) => s.scratches)
  if (scratches) return <FileTree key={SCRATCH_ID} projectId={SCRATCH_ID} />
  if (!projectId) return <EmptyState icon={FolderGit2} title="No project selected" />
  return <FileTree key={projectId} projectId={projectId} />
}

interface Row extends TreeRow {
  root?: boolean
}

function FileTree({ projectId }: { projectId: string }) {
  const project = useProjectSummary(projectId)
  const scratch = isScratch(projectId)
  const tree = useTreeStore((s) => s.trees[projectId])
  const update = useTreeStore((s) => s.update)
  const vcs = useVcsIndex(projectId)
  const list = useRef<VirtualListHandle>(null)
  const filterRef = useRef<HTMLInputElement>(null)
  const pendingReveal = useRef<string | null>(null)
  const [dropDir, setDropDir] = useState<string | null>(null)

  // Materialize this project's tree state and load the root.
  useEffect(() => {
    if (!useTreeStore.getState().trees[projectId]) update(projectId, () => ({}))
  }, [projectId, update])

  const expanded = useMemo(() => new Set(tree?.expanded ?? []), [tree?.expanded])

  // Load the root and every expanded directory that is not loaded yet.
  useEffect(() => {
    if (!tree) return
    for (const d of ['', ...tree.expanded]) {
      const st = tree.dirs[d]
      if (!st || (!st.entries && !st.loading && !st.error)) void loadDir(projectId, d)
    }
  }, [projectId, tree])

  const rows: Row[] = useMemo(() => {
    if (!tree) return []
    const inner = flattenTree(tree.dirs, expanded, tree.filter).map((r) => ({ ...r, depth: r.depth + 1 }))
    const rootEntry: FileEntry = {
      name: project?.name ?? projectId,
      path: '',
      kind: 'dir',
      size: 0,
      mtime: 0,
      ignored: false,
      hidden: false,
      sensitive: false,
    }
    return [{ entry: rootEntry, depth: 0, isDir: true, expanded: true, loading: !!tree.dirs['']?.loading, match: false, root: true }, ...inner]
  }, [tree, expanded, project?.name, projectId])

  const selected = tree?.selected ?? null
  const selectedIndex = rows.findIndex((r) => r.entry.path === (selected ?? '\0'))

  const select = useCallback((path: string | null) => update(projectId, () => ({ selected: path })), [projectId, update])

  const toggle = useCallback(
    (path: string, open?: boolean) => {
      update(projectId, (t: TreeState) => {
        const set = new Set(t.expanded)
        const want = open ?? !set.has(path)
        if (want) set.add(path)
        else for (const x of set) if (x === path || x.startsWith(path + '/')) set.delete(x)
        return { expanded: [...set] }
      })
    },
    [projectId, update],
  )

  const target = useCallback(
    (row: Row): Target => ({ projectId, rootAbs: project?.rootAbs, path: row.entry.path, isDir: row.isDir }),
    [projectId, project?.rootAbs],
  )

  const activate = (row: Row) => {
    if (row.root || row.more !== undefined) return
    if (row.isDir) toggle(row.entry.path)
    else openFile({ projectId, path: row.entry.path })
  }

  // Scroll a revealed file into view once its folders are loaded.
  useEffect(() => {
    const onReveal = (pid: string, path: string) => {
      if (pid === projectId) pendingReveal.current = path
    }
    revealRequests.add(onReveal)
    return () => {
      revealRequests.delete(onReveal)
    }
  }, [projectId])
  useEffect(() => {
    if (pendingReveal.current && selectedIndex >= 0 && rows[selectedIndex]?.entry.path === pendingReveal.current) {
      list.current?.scrollToIndex(selectedIndex, 'center')
      pendingReveal.current = null
    }
  }, [rows, selectedIndex])

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target instanceof HTMLInputElement) return
    const idx = Math.max(0, selectedIndex)
    const row = rows[idx]
    const move = (i: number) => {
      const n = Math.max(0, Math.min(rows.length - 1, i))
      select(rows[n].entry.path)
      list.current?.scrollToIndex(n)
    }
    switch (e.key) {
      case 'ArrowDown':
        move(selectedIndex < 0 ? 0 : idx + 1)
        break
      case 'ArrowUp':
        move(idx - 1)
        break
      case 'Home':
        move(0)
        break
      case 'End':
        move(rows.length - 1)
        break
      case 'PageDown':
        move(idx + 20)
        break
      case 'PageUp':
        move(idx - 20)
        break
      case 'ArrowRight':
        if (row?.isDir && !row.root) {
          if (!row.expanded) toggle(row.entry.path, true)
          else move(idx + 1)
        }
        break
      case 'ArrowLeft':
        if (row?.isDir && row.expanded && !row.root) toggle(row.entry.path, false)
        else if (row && !row.root) {
          const parent = dirname(row.entry.path)
          select(parent === '/' ? '' : parent)
          const pi = rows.findIndex((r) => r.entry.path === (parent === '/' ? '' : parent))
          if (pi >= 0) list.current?.scrollToIndex(pi)
        }
        break
      case 'Enter':
        if (row) activate(row)
        break
      case 'F2':
        if (row && !row.root && row.more === undefined) void renameEntry(target(row))
        break
      case 'Delete':
        if (row && !row.root && row.more === undefined) void deleteEntry(target(row))
        break
      case 'Escape':
        if (tree?.filter) update(projectId, () => ({ filter: '' }))
        break
      default:
        // Speed search: typing filters the tree.
        if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey && e.key !== ' ') {
          update(projectId, (t) => ({ filter: t.filter + e.key }))
          filterRef.current?.focus()
        } else return
    }
    e.preventDefault()
  }

  const onContextMenu = (e: MouseEvent, row: Row) => {
    e.preventDefault()
    e.stopPropagation()
    if (row.more !== undefined) return
    select(row.entry.path)
    showMenu(e, treeMenu(target(row)))
  }

  // ---------------------------------------------------------------- drag and drop

  const dropTargetDir = (row: Row | null) => (row ? (row.isDir ? row.entry.path : dirname(row.entry.path) === '/' ? '' : dirname(row.entry.path)) : '')

  const onDragStart = (e: DragEvent, row: Row) => {
    if (row.root && !project?.rootAbs) return
    const abs = absolutePath(project?.rootAbs, row.entry.path)
    e.dataTransfer.setData(DRAG_MIME, abs)
    e.dataTransfer.setData('text/plain', abs)
    e.dataTransfer.setData(INTERNAL_MIME, JSON.stringify({ projectId, path: row.entry.path }))
    e.dataTransfer.effectAllowed = 'copyMove'
  }

  const acceptsDrop = (e: DragEvent) => {
    const types = Array.from(e.dataTransfer.types)
    return types.includes('Files') || types.includes(INTERNAL_MIME)
  }

  const onDragOver = (e: DragEvent, row: Row | null) => {
    if (!acceptsDrop(e)) return
    e.preventDefault()
    e.stopPropagation()
    e.dataTransfer.dropEffect = Array.from(e.dataTransfer.types).includes('Files') ? 'copy' : 'move'
    const d = dropTargetDir(row)
    if (d !== dropDir) setDropDir(d)
  }

  const onDrop = (e: DragEvent, row: Row | null) => {
    if (!acceptsDrop(e)) return
    e.preventDefault()
    e.stopPropagation()
    setDropDir(null)
    const dir = dropTargetDir(row)
    const internal = e.dataTransfer.getData(INTERNAL_MIME)
    if (internal) {
      try {
        const src = JSON.parse(internal) as { projectId: string; path: string }
        if (src.projectId === projectId && src.path) void moveEntry(projectId, src.path, dir)
      } catch {
        /* not ours */
      }
      return
    }
    const files: File[] = []
    let folders = 0
    for (const item of Array.from(e.dataTransfer.items)) {
      if (item.kind !== 'file') continue
      const entry = item.webkitGetAsEntry?.()
      if (entry?.isDirectory) {
        folders++
        continue
      }
      const f = item.getAsFile()
      if (f) files.push(f)
    }
    if (folders) toast('warning', 'Folders cannot be uploaded; drop the files instead')
    void uploadFiles(projectId, dir, files)
  }

  if (!tree) return null
  const rootState = tree.dirs['']
  const locate = () => {
    const a = useActiveEditor.getState().current
    if (a && a.projectId === projectId) revealInTree(projectId, a.path)
  }

  return (
    <div className="wb-fill wb-files" onKeyDown={onKeyDown}>
      <div className="wb-files-toolbar">
        <div className="wb-files-filter">
          <Input
            ref={filterRef}
            small
            placeholder="Filter loaded files"
            value={tree.filter}
            onChange={(e) => update(projectId, () => ({ filter: e.target.value }))}
            onKeyDown={(e) => {
              if (e.key === 'Escape') {
                update(projectId, () => ({ filter: '' }))
                list.current?.element()?.focus()
              } else if (e.key === 'ArrowDown' || e.key === 'Enter') {
                e.preventDefault()
                const first = rows.find((r) => r.match) ?? rows[1]
                if (first) select(first.entry.path)
                list.current?.element()?.focus()
              }
            }}
          />
          {tree.filter && (
            <button className="wb-files-filter-clear" aria-label="Clear filter" onClick={() => update(projectId, () => ({ filter: '' }))}>
              <X size={12} />
            </button>
          )}
        </div>
        <IconButton
          icon={NotebookPen}
          size="small"
          label={scratch ? 'Back to the project' : 'Scratch Files'}
          active={scratch}
          onClick={() => useFilesView.getState().setScratches(!scratch)}
        />
        {scratch ? (
          <IconButton icon={FilePlus} size="small" label="New Scratch File… (Ctrl+Alt+Shift+Insert)" onClick={(e) => chooseScratchKind(e)} />
        ) : (
          <IconButton icon={FilePlus} size="small" label="New File…" onClick={() => void newEntry(projectId, selectedDir(rows, selectedIndex), 'file')} />
        )}
        <IconButton icon={FolderPlus} size="small" label="New Folder…" onClick={() => void newEntry(projectId, selectedDir(rows, selectedIndex), 'folder')} />
        <IconButton icon={Crosshair} size="small" label="Locate Opened File (Alt+F1)" onClick={locate} />
        <IconButton icon={ChevronsDownUp} size="small" label="Collapse All" onClick={() => update(projectId, () => ({ expanded: [] }))} />
        <IconButton icon={RefreshCw} size="small" label="Reload from Disk" onClick={() => refreshAll(projectId)} />
      </div>
      {rootState?.error && !rootState.entries ? (
        <ErrorBox error={rootState.error} onRetry={() => void loadDir(projectId, '')} />
      ) : (
        <VirtualList
          ref={list}
          className={`wb-tree${dropDir === '' ? ' drop-root' : ''}`}
          tabIndex={0}
          role="tree"
          aria-label="Project files"
          count={rows.length}
          rowHeight={ROW}
          onDragOver={(e) => onDragOver(e, null)}
          onDragLeave={(e) => {
            if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropDir(null)
          }}
          onDrop={(e) => onDrop(e, null)}
          renderRow={(i) => {
            const row = rows[i]
            return (
              <TreeRowView
                row={row}
                vcs={vcs}
                selected={i === selectedIndex}
                dropTarget={dropDir !== null && row.isDir && dropDir === row.entry.path}
                filter={tree.filter}
                rootPath={project?.root}
                onClick={() => select(row.entry.path)}
                onDoubleClick={() => activate(row)}
                onToggle={() => !row.root && toggle(row.entry.path)}
                onContextMenu={(e) => onContextMenu(e, row)}
                onDragStart={(e) => onDragStart(e, row)}
                onDragOver={(e) => onDragOver(e, row)}
                onDrop={(e) => onDrop(e, row)}
              />
            )
          }}
        />
      )}
    </div>
  )
}

function selectedDir(rows: Row[], index: number): string {
  const r = rows[index]
  if (!r) return ''
  if (r.isDir) return r.entry.path
  const d = dirname(r.entry.path)
  return d === '/' ? '' : d
}

function Highlight({ text, filter }: { text: string; filter: string }) {
  const f = filter.trim().toLowerCase()
  const i = f ? text.toLowerCase().indexOf(f) : -1
  if (i < 0) return <>{text}</>
  return (
    <>
      {text.slice(0, i)}
      <mark className="wb-files-mark">{text.slice(i, i + f.length)}</mark>
      {text.slice(i + f.length)}
    </>
  )
}

function TreeRowView({
  row,
  vcs,
  selected,
  dropTarget,
  filter,
  rootPath,
  onClick,
  onDoubleClick,
  onToggle,
  onContextMenu,
  onDragStart,
  onDragOver,
  onDrop,
}: {
  row: Row
  vcs: VcsIndex
  selected: boolean
  dropTarget: boolean
  filter: string
  rootPath?: string
  onClick: () => void
  onDoubleClick: () => void
  onToggle: () => void
  onContextMenu: (e: MouseEvent) => void
  onDragStart: (e: DragEvent) => void
  onDragOver: (e: DragEvent) => void
  onDrop: (e: DragEvent) => void
}) {
  const e = row.entry
  if (row.more !== undefined) {
    return (
      <div className="wb-tree-row more" style={{ paddingLeft: 4 + row.depth * 14 + 18 }}>
        <span className="wb-tree-extra">{e.name}</span>
      </div>
    )
  }
  const kind = row.root ? null : e.ignored ? 'ignored' : row.isDir ? (vcs.dirs.get(e.path) ?? vcsKindOf(vcs, e.path)) : vcsKindOf(vcs, e.path)
  const cls = ['wb-tree-row', selected && 'selected', dropTarget && 'drop', row.root && 'root'].filter(Boolean).join(' ')
  return (
    <div
      className={cls}
      role="treeitem"
      aria-selected={selected}
      aria-expanded={row.isDir ? row.expanded : undefined}
      style={{ paddingLeft: 4 + row.depth * 14 }}
      title={row.root ? rootPath : e.path + (e.kind === 'symlink' ? ` → symlink${e.target === 'broken' ? ' (outside the project or broken)' : ''}` : '')}
      draggable={!row.root}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContextMenu}
      onDragStart={onDragStart}
      onDragOver={onDragOver}
      onDrop={onDrop}
    >
      <span
        className="wb-tree-chevron"
        onClick={(ev) => {
          ev.stopPropagation()
          onToggle()
        }}
      >
        {row.isDir && !row.root ? row.expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} /> : null}
      </span>
      {row.root ? (
        <FolderGit2 size={15} className="wb-ficon folder" />
      ) : (
        <FileIcon path={e.path} dir={row.isDir} open={row.expanded} symlink={e.kind === 'symlink' && !row.isDir} sensitive={e.sensitive} />
      )}
      <span className={`wb-tree-name${kind ? ` wb-vcs-${kind}` : ''}${row.root ? ' strong' : ''}${filter && !row.match && !row.root ? ' dim' : ''}`}>
        <Highlight text={e.name} filter={row.match ? filter : ''} />
      </span>
      {row.root && rootPath && <span className="wb-tree-extra">{rootPath}</span>}
      {e.kind === 'symlink' && <span className="wb-tree-extra">↗</span>}
      {row.loading && <Spinner size={10} />}
    </div>
  )
}
