// Loads directory listings into the tree store and keeps loaded directories fresh
// from `fs.changed` (only the affected, already-loaded directories are re-listed),
// whether or not the Files tool window is currently shown.

import { ApiError } from '@/api/client'
import { subscribe } from '@/api/events'
import { filesApi } from './api'
import { isWithin } from './paths'
import { useTreeStore } from './store'
import { dirsToReload, pruneDirs } from './treeModel'

const inflight = new Map<string, AbortController>()

export async function loadDir(pid: string, dir: string, quiet = false): Promise<void> {
  const key = `${pid}\0${dir}`
  inflight.get(key)?.abort()
  const ctl = new AbortController()
  inflight.set(key, ctl)
  const store = useTreeStore.getState()
  const had = !!store.get(pid).dirs[dir]?.entries
  if (!quiet || !had) {
    store.update(pid, (t) => ({ dirs: { ...t.dirs, [dir]: { ...t.dirs[dir], loading: true, error: undefined } } }))
  }
  try {
    const l = await filesApi.list(pid, dir, ctl.signal)
    useTreeStore.getState().update(pid, (t) => ({
      dirs: { ...t.dirs, [dir]: { entries: l.entries, truncated: l.truncated, total: l.total } },
    }))
  } catch (e) {
    if (ctl.signal.aborted) return
    if (e instanceof ApiError && (e.status === 404 || e.status === 400) && dir !== '') {
      // The folder is gone (or no longer a folder): forget it.
      useTreeStore.getState().update(pid, (t) => ({
        dirs: pruneDirs(t.dirs, dir),
        expanded: t.expanded.filter((x) => !isWithin(dir, x)),
      }))
    } else {
      const msg = e instanceof Error ? e.message : String(e)
      useTreeStore.getState().update(pid, (t) => ({ dirs: { ...t.dirs, [dir]: { ...t.dirs[dir], loading: false, error: msg } } }))
    }
  } finally {
    if (inflight.get(key) === ctl) inflight.delete(key)
  }
}

/** Re-list these directories if they are loaded. */
export function refreshDirs(pid: string, dirs: string[]) {
  const loaded = useTreeStore.getState().get(pid).dirs
  for (const d of new Set(dirs)) if (loaded[d]?.entries) void loadDir(pid, d, true)
}

/** Forget a folder (and everything below it) that no longer exists. */
export function forgetDir(pid: string, dir: string) {
  if (!dir) return
  useTreeStore.getState().update(pid, (t) => ({
    dirs: pruneDirs(t.dirs, dir),
    expanded: t.expanded.filter((x) => !isWithin(dir, x)),
  }))
}

/**
 * Refresh after `fs.changed`: parents first, then the changed folders that still
 * exist in their parent's fresh listing (vanished ones are forgotten, not re-listed).
 */
async function refreshAfterChange(pid: string, dirs: string[]) {
  const loaded = () => useTreeStore.getState().get(pid).dirs
  const set = new Set(dirs)
  const parents = [...set].filter((d) => d === '' || !set.has(parentDir(d)))
  const children = [...set].filter((d) => !parents.includes(d))
  await Promise.all(parents.map((d) => (loaded()[d]?.entries ? loadDir(pid, d, true) : Promise.resolve())))
  for (const d of children) {
    const parentEntries = loaded()[parentDir(d)]?.entries
    if (parentEntries && !parentEntries.some((e) => e.path === d)) forgetDir(pid, d)
    else if (loaded()[d]?.entries) void loadDir(pid, d, true)
  }
}

function parentDir(p: string): string {
  const i = p.lastIndexOf('/')
  return i < 0 ? '' : p.slice(0, i)
}

export function refreshAll(pid: string) {
  refreshDirs(pid, Object.keys(useTreeStore.getState().get(pid).dirs))
}

let started = false
const pendingByProject = new Map<string, { paths: Set<string>; overflow: boolean; timer: number }>()

/** Subscribe once (from the files provider). */
export function startTreeSync() {
  if (started) return
  started = true
  subscribe('fs.changed', (ev) => {
    const pid = ev.projectId
    if (!pid || !useTreeStore.getState().trees[pid]) return
    const data = ev.data as { paths?: string[]; overflow?: boolean }
    let p = pendingByProject.get(pid)
    if (!p) {
      p = { paths: new Set(), overflow: false, timer: 0 }
      pendingByProject.set(pid, p)
    }
    for (const x of data.paths ?? []) p.paths.add(x)
    p.overflow ||= !!data.overflow
    window.clearTimeout(p.timer)
    p.timer = window.setTimeout(() => {
      pendingByProject.delete(pid)
      const dirs = useTreeStore.getState().get(pid).dirs
      const loaded = Object.keys(dirs).filter((d) => dirs[d]?.entries)
      void refreshAfterChange(pid, dirsToReload([...p!.paths], loaded, p!.overflow))
    }, 120)
  })
  subscribe('resync', () => {
    for (const pid of Object.keys(useTreeStore.getState().trees)) refreshAll(pid)
  })
}
