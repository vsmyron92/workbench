// Client state of the Workspace feature: dialogs, the phone's selected card, and
// per-browser preferences (scope choice, archive toggle).

import { create } from 'zustand'
import { persist } from 'zustand/middleware'

export interface MobileSelection {
  scope: string
  cardId: string
  step?: number
}

interface WsUi {
  /** Scope for the "New card" dialog, or null when closed. */
  newCardScope: string | null
  pickerOpen: boolean
  mobile: MobileSelection | null
  openNewCard: (scope: string) => void
  closeNewCard: () => void
  setPicker: (open: boolean) => void
  setMobile: (sel: MobileSelection | null) => void
}

export const useWsUi = create<WsUi>()((set) => ({
  newCardScope: null,
  pickerOpen: false,
  mobile: null,
  openNewCard: (scope) => set({ newCardScope: scope }),
  closeNewCard: () => set({ newCardScope: null }),
  setPicker: (open) => set({ pickerOpen: open }),
  setMobile: (mobile) => set({ mobile }),
}))

/** A markdown file being edited: what the editor holds and what it started from. */
export interface MdDraft {
  text: string
  /** Revision (sha256) of the file the edit is based on (updated on save). */
  base: string
  baseText: string
}

export function draftKey(scope: string, cardId: string, path: string): string {
  return `${scope}\n${cardId}\n${path}`
}

export function isDirty(d: MdDraft | undefined): boolean {
  return !!d && d.text !== d.baseText
}

interface WsDrafts {
  /** By `draftKey`. An entry means the file is in edit mode. */
  drafts: Record<string, MdDraft>
  put: (key: string, d: MdDraft) => void
  setText: (key: string, text: string) => void
  /** Record a save (keeps text typed while it was in flight). */
  saved: (key: string, base: string, baseText: string) => void
  drop: (key: string) => void
  dropCard: (scope: string, cardId: string) => void
}

/**
 * Markdown drafts outlive the editor (as in Mr. Mak): switching steps, an agent
 * opening another step, or a reload keep them; coming back restores the editor.
 * Changed drafts are kept in this tab's sessionStorage.
 */
const DRAFTS_STORAGE = 'wb.workspace.drafts.v1'

function loadDrafts(): Record<string, MdDraft> {
  try {
    const raw = typeof sessionStorage === 'undefined' ? null : sessionStorage.getItem(DRAFTS_STORAGE)
    const parsed: unknown = raw ? JSON.parse(raw) : {}
    const out: Record<string, MdDraft> = {}
    if (parsed && typeof parsed === 'object') {
      for (const [k, v] of Object.entries(parsed as Record<string, Partial<MdDraft>>)) {
        if (v && typeof v.text === 'string' && typeof v.base === 'string' && typeof v.baseText === 'string') out[k] = { text: v.text, base: v.base, baseText: v.baseText }
      }
    }
    return out
  } catch {
    return {}
  }
}

function saveDrafts(drafts: Record<string, MdDraft>) {
  try {
    const dirty = Object.fromEntries(Object.entries(drafts).filter(([, d]) => isDirty(d)))
    if (Object.keys(dirty).length) sessionStorage.setItem(DRAFTS_STORAGE, JSON.stringify(dirty))
    else sessionStorage.removeItem(DRAFTS_STORAGE)
  } catch {
    /* storage full or blocked: the in-memory draft still survives step switches */
  }
}

export const useWsDrafts = create<WsDrafts>()((set) => ({
  drafts: loadDrafts(),
  put: (key, d) => set((s) => ({ drafts: { ...s.drafts, [key]: d } })),
  setText: (key, text) =>
    set((s) => {
      const d = s.drafts[key]
      return d && d.text !== text ? { drafts: { ...s.drafts, [key]: { ...d, text } } } : s
    }),
  saved: (key, base, baseText) =>
    set((s) => {
      const d = s.drafts[key]
      return d ? { drafts: { ...s.drafts, [key]: { ...d, base, baseText } } } : s
    }),
  drop: (key) =>
    set((s) => {
      if (!(key in s.drafts)) return s
      const drafts = { ...s.drafts }
      delete drafts[key]
      return { drafts }
    }),
  dropCard: (scope, cardId) =>
    set((s) => {
      const prefix = draftKey(scope, cardId, '')
      return { drafts: Object.fromEntries(Object.entries(s.drafts).filter(([k]) => !k.startsWith(prefix))) }
    }),
}))

/** Whether any draft has unsaved changes (the unload warning). */
export function anyDirtyDraft(): boolean {
  return Object.values(useWsDrafts.getState().drafts).some(isDirty)
}

if (typeof window !== 'undefined') {
  let timer: ReturnType<typeof setTimeout> | undefined
  useWsDrafts.subscribe((s) => {
    clearTimeout(timer)
    timer = setTimeout(() => saveDrafts(s.drafts), 400)
  })
  window.addEventListener('pagehide', () => {
    clearTimeout(timer)
    saveDrafts(useWsDrafts.getState().drafts)
  })
}

interface WsPrefs {
  /** Tool window and phone list: the project's cards or Home's. */
  listScope: 'project' | 'home'
  showArchived: boolean
  setListScope: (s: 'project' | 'home') => void
  setShowArchived: (v: boolean) => void
}

export const useWsPrefs = create<WsPrefs>()(
  persist(
    (set) => ({
      listScope: 'project',
      showArchived: false,
      setListScope: (listScope) => set({ listScope }),
      setShowArchived: (showArchived) => set({ showArchived }),
    }),
    { name: 'wb.workspace.v1' },
  ),
)
