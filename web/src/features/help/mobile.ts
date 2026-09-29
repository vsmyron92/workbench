import { create } from 'zustand'

/** The phone's Help, shown inside the More tab: closed, or open on a page (undefined = the list). */
interface MobileHelp {
  open: boolean
  slug: string | undefined
  show: (slug?: string) => void
  setSlug: (slug: string | undefined) => void
  close: () => void
}

export const useMobileHelp = create<MobileHelp>((set) => ({
  open: false,
  slug: undefined,
  show: (slug) => set({ open: true, slug }),
  setSlug: (slug) => set({ slug }),
  close: () => set({ open: false, slug: undefined }),
}))
