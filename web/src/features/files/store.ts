// Client state of the files slice that must outlive a component: tree state per
// project (tool windows unmount when hidden), the active editor, search inputs,
// the quick-open dialog, and recently opened files.

import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import type { SearchParams } from './api'
import type { DirState } from './treeModel'

// ---------------------------------------------------------------- tree

export interface TreeState {
  dirs: Record<string, DirState>
  expanded: string[]
  selected: string | null
  filter: string
}

const EMPTY_TREE: TreeState = { dirs: {}, expanded: [], selected: null, filter: '' }

function loadExpanded(pid: string): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(`wb.files.expanded.${pid}`) ?? '[]')
    return Array.isArray(v) ? v.filter((x) => typeof x === 'string').slice(0, 500) : []
  } catch {
    return []
  }
}

function saveExpanded(pid: string, expanded: string[]) {
  try {
    localStorage.setItem(`wb.files.expanded.${pid}`, JSON.stringify(expanded.slice(0, 500)))
  } catch {
    /* quota */
  }
}

interface TreeStore {
  trees: Record<string, TreeState>
  get: (pid: string) => TreeState
  update: (pid: string, fn: (t: TreeState) => Partial<TreeState>) => void
}

export const useTreeStore = create<TreeStore>()((set, get) => ({
  trees: {},
  get: (pid) => get().trees[pid] ?? { ...EMPTY_TREE, expanded: loadExpanded(pid) },
  update: (pid, fn) =>
    set((s) => {
      const cur = s.trees[pid] ?? { ...EMPTY_TREE, expanded: loadExpanded(pid) }
      const patch = fn(cur)
      const next = { ...cur, ...patch }
      if (patch.expanded) saveExpanded(pid, next.expanded)
      return { trees: { ...s.trees, [pid]: next } }
    }),
}))

export function useTree(pid: string): TreeState {
  return useTreeStore((s) => s.trees[pid]) ?? useTreeStore.getState().get(pid)
}

// ---------------------------------------------------------------- active editor

export interface ActiveEditor {
  panelId: string
  projectId: string | null
  path: string
  /** Current selection text (empty when none). */
  selection: () => string
  /** Move the cursor (quick open `:line`). */
  goto: (line: number, column?: number) => void
}

export const useActiveEditor = create<{ current: ActiveEditor | null; set: (e: ActiveEditor | null) => void }>()((set) => ({
  current: null,
  set: (current) => set({ current }),
}))

// ---------------------------------------------------------------- search

export interface SearchInputs extends SearchParams {
  replaceOpen: boolean
  replacement: string
}

const EMPTY_SEARCH: SearchInputs = { q: '', regex: false, case: false, word: false, glob: '', replaceOpen: false, replacement: '' }

export const useSearchStore = create<{
  inputs: Record<string, SearchInputs>
  /** Bumped to focus the search input (Find in Files). */
  focusTick: number
  set: (pid: string, patch: Partial<SearchInputs>) => void
  focus: () => void
}>()(
  // Per browser tab: the query survives a reload, not a new window.
  persist(
    (set) => ({
      inputs: {},
      focusTick: 0,
      set: (pid, patch) => set((s) => ({ inputs: { ...s.inputs, [pid]: { ...(s.inputs[pid] ?? EMPTY_SEARCH), ...patch } } })),
      focus: () => set((s) => ({ focusTick: s.focusTick + 1 })),
    }),
    { name: 'wb.files.search.v1', storage: createJSONStorage(() => sessionStorage), partialize: (s) => ({ inputs: s.inputs }) },
  ),
)

export function useSearchInputs(pid: string): SearchInputs {
  return useSearchStore((s) => s.inputs[pid]) ?? EMPTY_SEARCH
}

// ---------------------------------------------------------------- quick open

export const useQuickOpen = create<{
  open: boolean
  initial: string
  /** Pick mode ("Compare With…"): the chosen file goes here instead of opening. */
  pick: { title: string; run: (path: string) => void } | null
  show: (initial?: string) => void
  choose: (title: string, run: (path: string) => void) => void
  hide: () => void
}>()((set) => ({
  open: false,
  initial: '',
  pick: null,
  show: (initial = '') => set({ open: true, initial, pick: null }),
  choose: (title, run) => set({ open: true, initial: '', pick: { title, run } }),
  hide: () => set({ open: false, pick: null }),
}))

// ---------------------------------------------------------------- recent files

const RECENT_MAX = 40

export function recentFiles(pid: string): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(`wb.files.recent.${pid}`) ?? '[]')
    return Array.isArray(v) ? v.filter((x) => typeof x === 'string') : []
  } catch {
    return []
  }
}

export function noteRecent(pid: string | null, path: string) {
  if (!pid) return
  const list = [path, ...recentFiles(pid).filter((p) => p !== path)].slice(0, RECENT_MAX)
  try {
    localStorage.setItem(`wb.files.recent.${pid}`, JSON.stringify(list))
  } catch {
    /* quota */
  }
}

export function forgetRecent(pid: string, path: string) {
  try {
    localStorage.setItem(`wb.files.recent.${pid}`, JSON.stringify(recentFiles(pid).filter((p) => p !== path)))
  } catch {
    /* quota */
  }
}
