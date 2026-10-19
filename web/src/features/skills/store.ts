// What the Skills tool window remembers while the page is open: its filters, the groups the user
// folded, and the skill whose panel was opened last. Nothing here is kept across a reload, and
// nothing about the skills themselves: they come from the server on every look.

import { create } from 'zustand'
import { NO_FILTERS, type Filters } from './logic'

interface SkillsUi {
  filters: Filters
  /** Group key → folded (true) or opened (false) by the user; absent: the default. */
  folded: Record<string, boolean>
  selected: string | null
  setFilters: (f: Partial<Filters>) => void
  toggleFolded: (key: string, now: boolean) => void
  select: (id: string | null) => void
}

export const useSkillsUi = create<SkillsUi>((set) => ({
  filters: NO_FILTERS,
  folded: {},
  selected: null,
  setFilters: (f) => set((s) => ({ filters: { ...s.filters, ...f } })),
  toggleFolded: (key, now) => set((s) => ({ folded: { ...s.folded, [key]: now } })),
  select: (id) => set({ selected: id }),
}))
