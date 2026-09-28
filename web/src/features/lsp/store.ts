// Client state of the lsp slice's windows and popups.

import { create } from 'zustand'
import type { editor } from 'monaco-editor'
import type { LspHierarchyItem, LspWorkspaceEdit } from './api'
import type { FlatSymbol, Loc } from './convert'

// ---------------------------------------------------------------- Find Usages

export interface UsageSearch {
  id: number
  projectId: string
  /** The symbol's name. */
  title: string
  /** Where the search started (shown, and excluded from nothing). */
  origin: { uri: string; line: number }
  state: 'loading' | 'done' | 'error'
  error?: string
  locs: Loc[]
  at: number
}

const MAX_SEARCHES = 8

export const useUsages = create<{
  searches: UsageSearch[]
  active: number | null
  add: (s: Omit<UsageSearch, 'id' | 'at'>) => number
  update: (id: number, patch: Partial<UsageSearch>) => void
  select: (id: number) => void
  close: (id: number) => void
}>()((set) => ({
  searches: [],
  active: null,
  add: (s) => {
    const id = Date.now() + Math.random()
    set((st) => ({ searches: [...st.searches, { ...s, id, at: Date.now() }].slice(-MAX_SEARCHES), active: id }))
    return id
  },
  update: (id, patch) => set((st) => ({ searches: st.searches.map((x) => (x.id === id ? { ...x, ...patch } : x)) })),
  select: (id) => set({ active: id }),
  close: (id) =>
    set((st) => {
      const searches = st.searches.filter((x) => x.id !== id)
      return { searches, active: st.active === id ? (searches[searches.length - 1]?.id ?? null) : st.active }
    }),
}))

// ---------------------------------------------------------------- Hierarchy

export type HierarchyKind = 'call' | 'type'
export type HierarchyDirection = 'incoming' | 'outgoing' | 'supertypes' | 'subtypes'

export interface HierarchyView {
  /** Changes with every new root, so the tree starts over. */
  id: number
  projectId: string
  kind: HierarchyKind
  root: LspHierarchyItem
  direction: HierarchyDirection
}

export const useHierarchy = create<{
  view: HierarchyView | null
  show: (v: Omit<HierarchyView, 'id'>) => void
  setDirection: (d: HierarchyDirection) => void
}>()((set) => ({
  view: null,
  show: (v) => set({ view: { ...v, id: Date.now() + Math.random() } }),
  setDirection: (direction) => set((s) => (s.view ? { view: { ...s.view, direction, id: Date.now() + Math.random() } } : s)),
}))

// ---------------------------------------------------------------- popups

export interface ChooserState {
  title: string
  locs: Loc[]
  /** Screen position (below the caret). */
  x: number
  y: number
  /** Editor to return focus to. */
  editor?: editor.ICodeEditor
}

export interface StructureState {
  editor: editor.ICodeEditor
  title: string
  symbols: FlatSymbol[]
  loading: boolean
  error?: string
}

export interface RenameState {
  projectId: string
  server: string
  uri: string
  position: { line: number; character: number }
  oldName: string
  /** Step 2: the edit to preview. */
  edit?: LspWorkspaceEdit
  newName?: string
  editor?: editor.ICodeEditor
}

export interface EnableState {
  projectId: string
  /** The server that made us ask (the banner's file). */
  serverId?: string
}

export interface MessageRequest {
  server: string
  message: string
  level: number
  actions: { title: string }[]
  resolve: (a: { title: string } | null) => void
}

export const usePopups = create<{
  chooser: ChooserState | null
  structure: StructureState | null
  gotoSymbol: { projectId: string } | null
  rename: RenameState | null
  enable: EnableState | null
  logs: { projectId: string; serverId: string } | null
  messages: MessageRequest[]
  set: (p: Partial<{ chooser: ChooserState | null; structure: StructureState | null; gotoSymbol: { projectId: string } | null; rename: RenameState | null; enable: EnableState | null; logs: { projectId: string; serverId: string } | null }>) => void
  pushMessage: (m: MessageRequest) => void
  popMessage: (m: MessageRequest) => void
}>()((set) => ({
  chooser: null,
  structure: null,
  gotoSymbol: null,
  rename: null,
  enable: null,
  logs: null,
  messages: [],
  set: (p) => set(p),
  pushMessage: (m) => set((s) => ({ messages: [...s.messages, m].slice(-5) })),
  popMessage: (m) => set((s) => ({ messages: s.messages.filter((x) => x !== m) })),
}))

// ---------------------------------------------------------------- banner dismissals

const DISMISS_KEY = 'wb.lsp.dismissed.v1'

function loadDismissed(): Record<string, number> {
  try {
    const v = JSON.parse(localStorage.getItem(DISMISS_KEY) ?? '{}')
    return v && typeof v === 'object' ? v : {}
  } catch {
    return {}
  }
}

export const useDismissed = create<{ map: Record<string, number>; dismiss: (key: string) => void }>()((set) => ({
  map: loadDismissed(),
  dismiss: (key) =>
    set((s) => {
      const map = { ...s.map, [key]: Date.now() }
      try {
        localStorage.setItem(DISMISS_KEY, JSON.stringify(map))
      } catch {
        /* quota */
      }
      return { map }
    }),
}))
