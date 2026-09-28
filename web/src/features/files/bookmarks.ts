// Bookmarks (CLion): F11 toggles one on the caret's line, Ctrl+F11 with a mnemonic
// (a digit or a letter), Shift+F11 lists them (typing a mnemonic there jumps to it).
// Kept per browser, for every project; the editors move them with the lines they
// mark while a file is open.

import { create } from 'zustand'
import { persist } from 'zustand/middleware'

export interface Bookmark {
  id: string
  projectId: string | null
  path: string
  /** 1-based. */
  line: number
  /** `0`–`9` or `A`–`Z`, unique among all bookmarks. */
  mnemonic?: string
  created: number
}

export const MNEMONICS = [...'1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ']

const sameFile = (b: Bookmark, projectId: string | null, path: string) => b.projectId === projectId && b.path === path

interface BookmarkState {
  list: Bookmark[]
  /** Bookmark the line, or remove the bookmark it has. Returns the bookmark now there, if any. */
  toggle: (projectId: string | null, path: string, line: number, mnemonic?: string) => Bookmark | null
  remove: (id: string) => void
  /** The editor moved bookmarks with their lines (edits above them). */
  moveLines: (moves: { id: string; line: number }[]) => void
  forFile: (projectId: string | null, path: string) => Bookmark[]
}

export const useBookmarks = create<BookmarkState>()(
  persist(
    (set, get) => ({
      list: [],
      toggle: (projectId, path, line, mnemonic) => {
        const list = get().list
        const here = list.find((b) => sameFile(b, projectId, path) && b.line === line)
        if (here && (!mnemonic || here.mnemonic === mnemonic)) {
          set({ list: list.filter((b) => b.id !== here.id) })
          return null
        }
        // A mnemonic belongs to one bookmark: taking it moves it here.
        const rest = list.filter((b) => b.id !== here?.id).map((b) => (mnemonic && b.mnemonic === mnemonic ? { ...b, mnemonic: undefined } : b))
        const b: Bookmark = { id: here?.id ?? `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`, projectId, path, line, mnemonic, created: here?.created ?? Date.now() }
        set({ list: [...rest, b] })
        return b
      },
      remove: (id) => set({ list: get().list.filter((b) => b.id !== id) }),
      moveLines: (moves) => {
        if (!moves.length) return
        const to = new Map(moves.map((m) => [m.id, m.line]))
        let changed = false
        const list = get().list.map((b) => {
          const line = to.get(b.id)
          if (line === undefined || line === b.line) return b
          changed = true
          return { ...b, line }
        })
        if (changed) set({ list })
      },
      forFile: (projectId, path) => get().list.filter((b) => sameFile(b, projectId, path)),
    }),
    { name: 'wb.bookmarks.v1', partialize: (s) => ({ list: s.list }) },
  ),
)

/** Order for the list: mnemonics first (digits, then letters), then newest. */
export function sortBookmarks(list: Bookmark[]): Bookmark[] {
  const rank = (b: Bookmark) => (b.mnemonic ? MNEMONICS.indexOf(b.mnemonic) : MNEMONICS.length)
  return [...list].sort((a, b) => rank(a) - rank(b) || b.created - a.created)
}
