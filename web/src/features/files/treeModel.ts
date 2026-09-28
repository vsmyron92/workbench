// Pure model of the lazy file tree: loaded directories + expanded set → visible rows.

import type { FileEntry } from './api'
import { dirname, isWithin } from './paths'

export interface DirState {
  entries?: FileEntry[]
  loading?: boolean
  error?: string
  truncated?: boolean
  total?: number
}

export interface TreeRow {
  entry: FileEntry
  depth: number
  isDir: boolean
  expanded: boolean
  loading: boolean
  /** The row matches the filter itself (not just an ancestor of a match). */
  match: boolean
  /** Placeholder after a truncated folder listing: how many entries are not shown. */
  more?: number
}

export function isDirEntry(e: Pick<FileEntry, 'kind' | 'target'>): boolean {
  return e.kind === 'dir' || (e.kind === 'symlink' && e.target === 'dir')
}

/**
 * Depth-first rows for everything visible. With a `filter`, only loaded entries
 * whose name contains it (case-insensitively) are shown, plus their ancestors,
 * which are shown expanded.
 */
export function flattenTree(dirs: Record<string, DirState>, expanded: ReadonlySet<string>, filter = ''): TreeRow[] {
  const f = filter.trim().toLowerCase()
  const rows: TreeRow[] = []
  if (!f) {
    const walk = (dir: string, depth: number) => {
      const st = dirs[dir]
      for (const e of st?.entries ?? []) {
        const isDir = isDirEntry(e)
        const open = isDir && expanded.has(e.path)
        rows.push({ entry: e, depth, isDir, expanded: open, loading: !!dirs[e.path]?.loading, match: false })
        if (open) walk(e.path, depth + 1)
      }
      if (st?.truncated && st.entries && st.total) {
        const n = st.total - st.entries.length
        const entry = { name: `${n} more not shown (use Go to File)`, path: `${dir}/\u0000more`, kind: 'file' as const, size: 0, mtime: 0, ignored: false, hidden: false, sensitive: false }
        rows.push({ entry, depth, isDir: false, expanded: false, loading: false, match: false, more: n })
      }
    }
    walk('', 0)
    return rows
  }
  // Which loaded paths match or contain a match.
  const keep = new Map<string, boolean>()
  const visit = (dir: string): boolean => {
    let any = false
    for (const e of dirs[dir]?.entries ?? []) {
      const self = e.name.toLowerCase().includes(f)
      const below = isDirEntry(e) && dirs[e.path]?.entries ? visit(e.path) : false
      if (self || below) {
        keep.set(e.path, self)
        any = true
      }
    }
    return any
  }
  visit('')
  const walk = (dir: string, depth: number) => {
    for (const e of dirs[dir]?.entries ?? []) {
      if (!keep.has(e.path)) continue
      const isDir = isDirEntry(e)
      const hasKeptChildren = isDir && (dirs[e.path]?.entries ?? []).some((c) => keep.has(c.path))
      rows.push({ entry: e, depth, isDir, expanded: hasKeptChildren, loading: false, match: keep.get(e.path)! })
      if (hasKeptChildren) walk(e.path, depth + 1)
    }
  }
  walk('', 0)
  return rows
}

/** Loaded directories that must be re-listed after `fs.changed {paths}`. */
export function dirsToReload(changed: string[], loaded: Iterable<string>, overflow = false): string[] {
  const loadedSet = new Set(loaded)
  if (overflow) return [...loadedSet]
  const out = new Set<string>()
  for (const p of changed) {
    const parent = dirname(p) === '/' ? '' : dirname(p)
    if (loadedSet.has(parent)) out.add(parent)
    if (loadedSet.has(p)) out.add(p)
  }
  return [...out]
}

/** Drop cached listings under `dir` (after it was deleted or renamed). */
export function pruneDirs(dirs: Record<string, DirState>, dir: string): Record<string, DirState> {
  const out: Record<string, DirState> = {}
  for (const [k, v] of Object.entries(dirs)) if (!(dir !== '' && isWithin(dir, k))) out[k] = v
  return out
}
