// What the overlay measures: the shell's windows it places itself in, and where the
// pointer is. The decisions made with these numbers are in lib/layout.ts; here is only
// the reading of the page.
//
// The shell's elements are found by their contract class names: `.wb-workwin` (the
// workspace window), `.wb-dock-area` (the dock inside it) and `.wb-mobile-top` (the
// phone's header). They are lazy and may be replaced, so they are looked up again
// whenever the layout may have changed, never kept.

import { useEffect, useRef, useState } from 'react'
import type { Rect, Size } from './lib/layout'

export interface Geometry {
  viewport: Size
  /** The workspace window; null while it is not on screen (collapsed, or the phone shell). */
  work: Rect | null
  /** The dock area inside it. */
  dock: Rect | null
  /** The phone shell's header. */
  header: Rect | null
  /** The top bar that holds the compact face (the desktop shell). */
  bar: Rect | null
}

const EMPTY: Geometry = { viewport: { width: 0, height: 0 }, work: null, dock: null, header: null, bar: null }
/** While an element the layout needs is not there yet (the shell is still loading), it is looked for this often. */
const LOOK_AGAIN_MS = 500

function rectOf(el: Element | null | undefined): Rect | null {
  if (!el) return null
  const r = el.getBoundingClientRect()
  // An element that is not laid out (`display: none`) measures as nothing.
  if (r.width <= 0 || r.height <= 0) return null
  return { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

const sameRect = (a: Rect | null, b: Rect | null): boolean => a === b || (!!a && !!b && a.left === b.left && a.top === b.top && a.right === b.right && a.bottom === b.bottom)
const sameGeometry = (a: Geometry, b: Geometry): boolean => a.viewport.width === b.viewport.width && a.viewport.height === b.viewport.height && sameRect(a.work, b.work) && sameRect(a.dock, b.dock) && sameRect(a.header, b.header) && sameRect(a.bar, b.bar)

/**
 * The rectangles the overlay is laid out with, kept current: on a resize of the window,
 * on a resize of any of the elements, and when the shell changes (`mobile`, `workOpen`,
 * the bar's slot).
 */
export function useGeometry(mobile: boolean, workOpen: boolean, barSlot: HTMLElement | null): Geometry {
  const [geometry, setGeometry] = useState<Geometry>(EMPTY)
  useEffect(() => {
    let watched: Element[] = []
    let timer: number | null = null
    const observer = typeof ResizeObserver === 'function' ? new ResizeObserver(() => measure()) : null
    function measure() {
      const work = document.querySelector('.wb-workwin')
      const dock = document.querySelector('.wb-dock-area')
      const header = document.querySelector('.wb-mobile-top')
      const bar = barSlot?.parentElement ?? null
      const found = [work, dock, header, bar].filter((el): el is Element => el !== null)
      if (observer && (found.length !== watched.length || found.some((el, i) => el !== watched[i]))) {
        observer.disconnect()
        for (const el of found) observer.observe(el)
        watched = found
      }
      const next: Geometry = { viewport: { width: window.innerWidth, height: window.innerHeight }, work: rectOf(work), dock: rectOf(dock), header: rectOf(header), bar: rectOf(bar) }
      setGeometry((prev) => (sameGeometry(prev, next) ? prev : next))
      // The shell is a lazy chunk: what the layout needs may not be there yet.
      const missing = mobile ? !header : !work || !dock
      if (missing && timer === null) {
        timer = window.setTimeout(() => {
          timer = null
          measure()
        }, LOOK_AGAIN_MS)
      }
    }
    measure()
    window.addEventListener('resize', measure)
    return () => {
      window.removeEventListener('resize', measure)
      observer?.disconnect()
      if (timer !== null) window.clearTimeout(timer)
    }
  }, [mobile, workOpen, barSlot])
  return geometry
}

/** The size of an element, kept current. `null` until it is there. */
export function useSize(el: HTMLElement | null): Size | null {
  const [size, setSize] = useState<Size | null>(null)
  useEffect(() => {
    if (!el) {
      setSize(null)
      return
    }
    const read = () => {
      const r = el.getBoundingClientRect()
      setSize((prev) => (prev && prev.width === r.width && prev.height === r.height ? prev : { width: r.width, height: r.height }))
    }
    read()
    if (typeof ResizeObserver !== 'function') return
    const observer = new ResizeObserver(read)
    observer.observe(el)
    return () => observer.disconnect()
  }, [el])
  return size
}

/** Whether a media query matches, kept current. */
export function useMedia(query: string): boolean {
  const [matches, setMatches] = useState(() => typeof matchMedia === 'function' && matchMedia(query).matches)
  useEffect(() => {
    if (typeof matchMedia !== 'function') return
    const m = matchMedia(query)
    const changed = () => setMatches(m.matches)
    changed()
    m.addEventListener('change', changed)
    return () => m.removeEventListener('change', changed)
  }, [query])
  return matches
}

/** Whether the user is looking at this window: the tab is visible and the window has focus. */
export function useAttention(): { visible: boolean; focused: boolean } {
  const read = () => ({ visible: document.visibilityState === 'visible', focused: document.hasFocus() })
  const [state, setState] = useState(read)
  useEffect(() => {
    const changed = () => {
      const next = read()
      setState((prev) => (prev.visible === next.visible && prev.focused === next.focused ? prev : next))
    }
    changed()
    document.addEventListener('visibilitychange', changed)
    window.addEventListener('focus', changed)
    window.addEventListener('blur', changed)
    return () => {
      document.removeEventListener('visibilitychange', changed)
      window.removeEventListener('focus', changed)
      window.removeEventListener('blur', changed)
    }
  }, [])
  return state
}

/** Where the pointer was last seen, in viewport pixels; null: not in this window (it left, or a touch or a pen was lifted). */
export interface PointerAt {
  x: number
  y: number
  /** When it last moved, on the clock of `performance.now()`. */
  movedAt: number
}

/** A pointer event as `followPointer` reads it. */
export interface PointerSeen {
  type: string
  pointerType: string
  buttons: number
  clientX: number
  clientY: number
}

/** Whether a pointer event tells where the pointer is: any of a mouse, of a touch or a pen only while it is pressed. */
export function counts(e: Pick<PointerSeen, 'type' | 'pointerType' | 'buttons'>): boolean {
  return e.pointerType === 'mouse' || e.type === 'pointerdown' || e.buttons !== 0
}

/** What `followPointer` listens on: the window, and the page's root element (the pointer leaves the page there). */
export interface PointerSources {
  win: {
    addEventListener(type: string, cb: (e: PointerSeen) => void, opts?: { capture: boolean; passive: boolean }): void
    removeEventListener(type: string, cb: (e: PointerSeen) => void, opts?: { capture: boolean }): void
  }
  page: {
    addEventListener(type: string, cb: () => void): void
    removeEventListener(type: string, cb: () => void): void
  }
}

/**
 * Keeps `at` current from a window's pointer events and calls `changed` whenever it
 * changed. Listeners of the capture phase that take nothing: a bubble that lets clicks
 * through receives no pointer events of its own, so where the pointer is has to be learned
 * from the window.
 *
 * Only a mouse rests somewhere. A touch or a pen is somewhere while it is pressed, and
 * nowhere once it was lifted (a pen that hovers is not counted either): the place of the
 * last tap is never taken for a pointer that rests on a bubble (`guardStep`). Returns the
 * function that stops it.
 */
export function followPointer(src: PointerSources, at: { current: PointerAt | null }, changed: () => void, now: () => number): () => void {
  const moved = (e: PointerSeen) => {
    if (!counts(e)) return
    const was = at.current
    // A move without a move (the page moved under a pointer that rests): it is where it was, since when it was.
    const rests = e.type === 'pointermove' && was !== null && was.x === e.clientX && was.y === e.clientY
    at.current = { x: e.clientX, y: e.clientY, movedAt: rests ? was.movedAt : now() }
    changed()
  }
  const gone = () => {
    if (at.current === null) return
    at.current = null
    changed()
  }
  // A finger or a pen that lifts is nowhere; a mouse stays where it was.
  const lifted = (e: PointerSeen) => {
    if (e.pointerType !== 'mouse') gone()
  }
  const opts = { capture: true, passive: true }
  src.win.addEventListener('pointermove', moved, opts)
  src.win.addEventListener('pointerdown', moved, opts)
  src.win.addEventListener('pointerup', lifted, opts)
  src.win.addEventListener('pointercancel', gone, opts)
  src.page.addEventListener('pointerleave', gone)
  // The window's own blur only (one in the capture phase would hear every element that loses focus).
  src.win.addEventListener('blur', gone)
  return () => {
    src.win.removeEventListener('pointermove', moved, { capture: true })
    src.win.removeEventListener('pointerdown', moved, { capture: true })
    src.win.removeEventListener('pointerup', lifted, { capture: true })
    src.win.removeEventListener('pointercancel', gone, { capture: true })
    src.page.removeEventListener('pointerleave', gone)
    src.win.removeEventListener('blur', gone)
  }
}

/** The pointer's place in a ref that is always current (`followPointer`), and a call whenever it changed, at most once a frame. */
export function usePointer(onChange: () => void): { current: PointerAt | null } {
  const at = useRef<PointerAt | null>(null)
  const told = useRef(onChange)
  told.current = onChange
  useEffect(() => {
    let frame: number | null = null
    const tell = () => {
      if (frame !== null) return
      frame = window.requestAnimationFrame(() => {
        frame = null
        told.current()
      })
    }
    const stop = followPointer({ win: window, page: document.documentElement }, at, tell, () => performance.now())
    return () => {
      stop()
      if (frame !== null) window.cancelAnimationFrame(frame)
    }
  }, [])
  return at
}

/** Whether a point is inside an element's rectangle. */
export function inside(el: Element | null | undefined, at: PointerAt | null): boolean {
  if (!el || !at) return false
  const r = el.getBoundingClientRect()
  return at.x >= r.left && at.x <= r.right && at.y >= r.top && at.y <= r.bottom
}
