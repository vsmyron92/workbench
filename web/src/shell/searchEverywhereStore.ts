// Open state of Search Everywhere (shell/SearchEverywhere.tsx), apart from the
// component so the palette and features can open it without importing it.

import { create } from 'zustand'

export const SEARCH_ALL = 'all'

export const useSearchEverywhere = create<{ open: boolean; tab: string; show: (tab?: string) => void; hide: () => void }>()((set) => ({
  open: false,
  tab: SEARCH_ALL,
  show: (tab = SEARCH_ALL) => set({ open: true, tab }),
  hide: () => set({ open: false }),
}))

/** Open Search Everywhere, optionally on one tab ('files', 'symbols', 'actions', 'text'…). */
export function openSearchEverywhere(tab?: string) {
  useSearchEverywhere.getState().show(tab)
}
