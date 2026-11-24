// Collapsing the workspace window shrinks the browser window to the agents window, at the
// same position; expanding it gives back the width it took. Browsers only let a page resize
// its own window when it is an app window (`chrome --app`, an installed app) or a popup it
// opened: in a normal tab `resizeTo` does nothing, and the agents window simply takes the
// whole width.
//
// `useIsMobile` is here too: a window narrowed this way stays the desktop layout, so which
// of the two shells is on screen is decided beside it.

import { useSyncExternalStore } from 'react'
import { useUi } from '@/state/store'

const KEY = 'wb.work.freed.v1'
/** Narrower than this, a desktop browser gets the phone layout (`useIsMobile`). */
export const PHONE_WIDTH = 768

// While the workspace window is collapsed the agents window is narrow on purpose: the page
// must stay the desktop layout until the window is wide again, or expanding would be
// impossible (the phone layout has no such button).
let hold = typeof window !== 'undefined' && !useUi.getState().workOpen
const holders = new Set<() => void>()

export function desktopHeld(): boolean {
  return hold
}

export function subscribeDesktopHold(cb: () => void): () => void {
  holders.add(cb)
  return () => void holders.delete(cb)
}

function setHold(v: boolean) {
  if (hold === v) return
  hold = v
  holders.forEach((f) => f())
}

if (typeof window !== 'undefined') {
  window.addEventListener('resize', () => {
    if (hold && useUi.getState().workOpen && window.innerWidth > PHONE_WIDTH) setHold(false)
  })
}

const mobileQuery = `(max-width: ${PHONE_WIDTH}px), (pointer: coarse) and (max-width: 1024px)`

/**
 * Whether the phone shell is the one on screen (App.tsx mounts it): a narrow window, a
 * touch screen up to tablet width, or `?mobile` in the address. A component that calls it
 * renders again when the shell switches.
 */
export function useIsMobile(): boolean {
  return useSyncExternalStore(
    (cb) => {
      const m = matchMedia(mobileQuery)
      m.addEventListener('change', cb)
      const off = subscribeDesktopHold(cb)
      return () => {
        m.removeEventListener('change', cb)
        off()
      }
    },
    () => {
      if (new URLSearchParams(location.search).has('mobile')) return true
      if (!matchMedia(mobileQuery).matches) return false
      // A desktop window narrowed by collapsing the workspace window stays the desktop layout.
      return !(desktopHeld() && !matchMedia('(pointer: coarse)').matches)
    },
  )
}

/** The outer width the browser window gets so the page is `agentsWidth` wide, and the width this frees. */
export function collapseTarget(outerWidth: number, innerWidth: number, agentsWidth: number): { width: number; freed: number } | null {
  const chrome = Math.max(0, outerWidth - innerWidth)
  const width = Math.round(agentsWidth + chrome)
  const freed = outerWidth - width
  return freed > 40 ? { width, freed } : null
}

function store(v: number | null) {
  try {
    if (v === null) localStorage.removeItem(KEY)
    else localStorage.setItem(KEY, String(v))
  } catch {
    /* storage blocked */
  }
}

/** The workspace window was collapsed (`open` false) or expanded. `agentsWidth`: the agents window's width while both show. */
export function fitWindowToWorkWindow(open: boolean, agentsWidth: number): void {
  setHold(!open || window.innerWidth <= PHONE_WIDTH)
  try {
    if (open) {
      let freed = 0
      try {
        freed = Number(localStorage.getItem(KEY))
      } catch {
        /* storage blocked */
      }
      store(null)
      if (freed > 0) window.resizeTo(window.outerWidth + freed, window.outerHeight)
      return
    }
    const t = collapseTarget(window.outerWidth, window.innerWidth, agentsWidth)
    if (!t) return
    const before = window.outerWidth
    // Left and top stay where they are: the window shrinks from its right edge.
    window.resizeTo(t.width, window.outerHeight)
    // A tab refuses: nothing to give back later.
    store(window.outerWidth < before ? before - window.outerWidth : null)
  } catch {
    /* a browser that blocks it */
  }
}
