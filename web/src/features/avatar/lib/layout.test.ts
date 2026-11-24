// Where the avatars sit: the layout that applies, the default place, the clamp into the
// workspace window, the corners, and what a drag stores.

import { describe, expect, it } from 'vitest'
import { ANCHOR_SIDE_PX, ANCHOR_TOP_PX, COLUMN_PX, DRAG_START_PX, EDGE_PX, MIN_WIDE_HEIGHT, MIN_WIDE_WIDTH, PANEL_EDGE_PX, PANEL_MAX_PX, REACH_PX, SIDE_EDGE_PX, bubbleShift, corner, defaultAnchor, dragStarts, dragged, layoutMode, panelPlace, panelTop, phoneFace, place, smallFigures, storedAnchor, toStored } from './layout'
import type { Corner, Rect, Size } from './layout'

const viewport: Size = { width: 1600, height: 900 }
/** The workspace window right of a 500 px agents column, above a 24 px status bar. */
const work: Rect = { left: 500, top: 0, right: 1600, bottom: 876 }
/** The dock area inside it: under the 36 px top bar, between the two 40 px stripes. */
const dock: Rect = { left: 540, top: 36, right: 1560, bottom: 876 }
const group: Size = { width: 64, height: 83 }
const half = COLUMN_PX / 2

describe('layoutMode', () => {
  it('is the phone form in the phone shell, whatever else holds', () => {
    expect(layoutMode({ mobile: true, workOpen: true, work })).toBe('phone')
    expect(layoutMode({ mobile: true, workOpen: false, work: null })).toBe('phone')
  })

  it('is the floating group while the workspace window is open and large enough', () => {
    expect(layoutMode({ mobile: false, workOpen: true, work })).toBe('wide')
    expect(layoutMode({ mobile: false, workOpen: true, work: { left: 0, top: 0, right: MIN_WIDE_WIDTH, bottom: MIN_WIDE_HEIGHT } })).toBe('wide')
  })

  it('is the top bar face while the workspace window is collapsed, missing or too small', () => {
    expect(layoutMode({ mobile: false, workOpen: false, work })).toBe('bar')
    expect(layoutMode({ mobile: false, workOpen: true, work: null })).toBe('bar')
    expect(layoutMode({ mobile: false, workOpen: true, work: { left: 0, top: 0, right: MIN_WIDE_WIDTH - 1, bottom: 900 } })).toBe('bar')
    expect(layoutMode({ mobile: false, workOpen: true, work: { left: 0, top: 0, right: 900, bottom: MIN_WIDE_HEIGHT - 1 } })).toBe('bar')
    // A collapsed window measures as nothing.
    expect(layoutMode({ mobile: false, workOpen: true, work: { left: 0, top: 0, right: 0, bottom: 0 } })).toBe('bar')
    expect(layoutMode({ mobile: false, workOpen: true, work: { left: Number.NaN, top: 0, right: 900, bottom: 900 } })).toBe('bar')
  })

  it('shows the smaller figure in the phone form and in a narrow panel', () => {
    expect(smallFigures('phone', 1200)).toBe(true)
    expect(smallFigures('bar', 420)).toBe(true)
    expect(smallFigures('bar', 640)).toBe(false)
    expect(smallFigures('wide', 300)).toBe(false)
  })
})

describe('the default place', () => {
  it('is the dock area’s top right, below its chrome rows', () => {
    const anchor = defaultAnchor(dock)
    expect(anchor).toEqual({ x: dock.right - ANCHOR_SIDE_PX - half, y: dock.top + ANCHOR_TOP_PX, edge: 'top' })
    const p = place(anchor, group, work, viewport)
    // The contract's formula: top = rect.top + 104, right = innerWidth - rect.right + 16.
    expect(p.css).toEqual({ top: dock.top + 104, right: viewport.width - dock.right + 16 })
    expect(p.side).toBe('right')
    expect(p.tail).toBe('right')
  })

  it('follows the dock area when a tool window takes room from it', () => {
    const narrower = { ...dock, right: 1180 }
    expect(place(defaultAnchor(narrower), group, work, viewport).css).toEqual({ top: 140, right: viewport.width - 1180 + 16 })
  })
})

describe('place: the clamp into the workspace window', () => {
  it('never lets the column leave the workspace window, so it never sits on the agents column', () => {
    for (const x of [-500, 0, 499, 520, 800, 1599, 5000]) {
      for (const y of [-100, 0, 400, 875, 5000]) {
        for (const edge of ['top', 'bottom'] as const) {
          const p = place({ x, y, edge }, group, work, viewport)
          expect(p.x - half).toBeGreaterThanOrEqual(work.left + SIDE_EDGE_PX)
          expect(p.x + half).toBeLessThanOrEqual(work.right - SIDE_EDGE_PX)
          const top = p.edge === 'top' ? p.y : p.y - group.height
          expect(top).toBeGreaterThanOrEqual(work.top + EDGE_PX)
          expect(top + group.height).toBeLessThanOrEqual(work.bottom - EDGE_PX)
        }
      }
    }
  })

  it('keeps the bubble inside too: the group hugs a side that has room for it', () => {
    for (let x = work.left; x <= work.right; x += 37) {
      const p = place({ x, y: 300, edge: 'top' }, group, work, viewport)
      if (p.side === 'right') expect(p.x - REACH_PX).toBeGreaterThanOrEqual(work.left + EDGE_PX)
      else expect(p.x + REACH_PX).toBeLessThanOrEqual(work.right - EDGE_PX)
      expect(p.tail).toBe(p.side)
    }
  })

  it('hugs the nearer side where both have room, and the nearer allowed place where the middle has none', () => {
    expect(place({ x: 900, y: 300, edge: 'top' }, group, work, viewport).side).toBe('left')
    expect(place({ x: 1200, y: 300, edge: 'top' }, group, work, viewport).side).toBe('right')
    // A workspace window of 700 px: a bubble fits only beside a group near one of its edges.
    const narrow: Rect = { left: 0, top: 0, right: 700, bottom: 800 }
    const nearLeft = place({ x: 300, y: 300, edge: 'top' }, group, narrow, { width: 700, height: 800 })
    expect([nearLeft.side, nearLeft.x]).toEqual(['left', 700 - EDGE_PX - REACH_PX])
    const nearRight = place({ x: 400, y: 300, edge: 'top' }, group, narrow, { width: 700, height: 800 })
    expect([nearRight.side, nearRight.x]).toEqual(['right', EDGE_PX + REACH_PX])
    // The narrowest window the floating group is shown in: the bubble just fits beside a group at either edge.
    const least: Rect = { left: 0, top: 0, right: MIN_WIDE_WIDTH, bottom: MIN_WIDE_HEIGHT }
    const leastView: Size = { width: MIN_WIDE_WIDTH, height: MIN_WIDE_HEIGHT }
    const atLeft = place({ x: 100, y: 0, edge: 'top' }, group, least, leastView)
    expect([atLeft.side, atLeft.x + REACH_PX]).toEqual(['left', MIN_WIDE_WIDTH - EDGE_PX])
    const atRight = place({ x: 300, y: 0, edge: 'top' }, group, least, leastView)
    expect([atRight.side, atRight.x - REACH_PX]).toEqual(['right', EDGE_PX])
    // Narrower still (the compact form is shown then): the right edge, where the default place is.
    const tooNarrow = place({ x: 100, y: 0, edge: 'top' }, group, { ...least, right: 440 }, { width: 440, height: MIN_WIDE_HEIGHT })
    expect([tooNarrow.side, tooNarrow.x]).toEqual(['right', 440 - SIDE_EDGE_PX - half])
  })

  it('positions the group by the edges it hugs', () => {
    const left = place({ x: 700, y: 300, edge: 'top' }, group, work, viewport)
    expect(left.css).toEqual({ left: 700 - half, top: 300 })
    const low = place({ x: 1400, y: 800, edge: 'bottom' }, group, work, viewport)
    expect(low.css).toEqual({ right: viewport.width - (1400 + half), bottom: viewport.height - 800 })
  })

  it('keeps a group at the bottom inside as it grows: it grows upwards', () => {
    const at = { x: 1400, y: 868, edge: 'bottom' as const }
    const small = place(at, { width: 64, height: 83 }, work, viewport)
    const tall = place(at, { width: 140, height: 400 }, work, viewport)
    expect(small.css.bottom).toBe(tall.css.bottom)
    expect(tall.y - 400).toBeGreaterThanOrEqual(work.top + EDGE_PX)
  })

  it('shows the top of a group that is taller than the window', () => {
    const p = place({ x: 1400, y: 860, edge: 'bottom' }, { width: 140, height: 2000 }, work, viewport)
    expect([p.edge, p.y]).toEqual(['top', work.top + EDGE_PX])
    expect(place({ x: 1400, y: 500, edge: 'top' }, { width: 140, height: 2000 }, work, viewport).y).toBe(work.top + EDGE_PX)
  })

  it('copes with a place that is no number', () => {
    const p = place({ x: Number.NaN, y: Number.NaN, edge: 'top' }, { width: 0, height: Number.NaN }, work, viewport)
    expect(Number.isFinite(p.x) && Number.isFinite(p.y)).toBe(true)
    expect(p.side).toBe('right')
  })
})

describe('a stored position', () => {
  it('is fractions of the viewport, read as the top edge above the middle and the bottom edge below it', () => {
    expect(storedAnchor({ x: 0.5, y: 0.25 }, viewport, work)).toEqual({ x: 800, y: 225, edge: 'top' })
    expect(storedAnchor({ x: 0.9, y: 0.9 }, viewport, work)).toEqual({ x: 1440, y: 810, edge: 'bottom' })
    // Out of range: brought into it.
    expect(storedAnchor({ x: 7, y: -3 }, viewport, work)).toEqual({ x: 1600, y: 0, edge: 'top' })
  })

  it('reads back exactly where a drag left the group, in either half', () => {
    for (const at of [
      { x: 1300, y: 200, edge: 'top' as const },
      { x: 700, y: 700, edge: 'top' as const },
      { x: 1500, y: 860, edge: 'bottom' as const },
      { x: 900, y: 300, edge: 'bottom' as const },
    ]) {
      const dropped = place(at, group, work, viewport)
      const stored = toStored(dropped, group, work, viewport)
      const again = place(storedAnchor(stored, viewport, work), group, work, viewport)
      const top = (p: { y: number; edge: string }) => (p.edge === 'top' ? p.y : p.y - group.height)
      expect(again.x).toBeCloseTo(dropped.x, 6)
      expect(top(again)).toBeCloseTo(top(dropped), 6)
      expect(again.side).toBe(dropped.side)
    }
  })

  it('stores a group in the lower half by its bottom edge', () => {
    const stored = toStored({ x: 1400, y: 700, edge: 'top' }, group, work, viewport)
    expect(stored.y).toBeCloseTo((700 + group.height) / viewport.height, 9)
    expect(storedAnchor(stored, viewport, work).edge).toBe('bottom')
    const upper = toStored({ x: 1400, y: 300, edge: 'bottom' }, group, work, viewport)
    expect(upper.y).toBeCloseTo((300 - group.height) / viewport.height, 9)
    expect(storedAnchor(upper, viewport, work).edge).toBe('top')
  })

  it('stays inside the window when the window gets smaller', () => {
    const stored = toStored(place({ x: 1500, y: 860, edge: 'bottom' }, group, work, viewport), group, work, viewport)
    const smaller: Size = { width: 1100, height: 600 }
    const smallWork: Rect = { left: 500, top: 0, right: 1100, bottom: 576 }
    const p = place(storedAnchor(stored, smaller, smallWork), group, smallWork, smaller)
    expect(p.x + half).toBeLessThanOrEqual(smallWork.right - SIDE_EDGE_PX)
    expect(p.y).toBeLessThanOrEqual(smallWork.bottom - EDGE_PX)
  })
})

describe('the corners', () => {
  it('are the dock area’s, in from its chrome as the default place is', () => {
    const at = (which: Corner) => place(storedAnchor(corner(which, dock, work, viewport), viewport, work), group, work, viewport)
    const tr = at('top-right')
    expect(tr.css).toEqual(place(defaultAnchor(dock), group, work, viewport).css)
    const tl = at('top-left')
    expect(tl.side).toBe('left')
    expect(tl.css.left).toBeCloseTo(dock.left + ANCHOR_SIDE_PX, 6)
    expect(tl.css.top).toBeCloseTo(dock.top + ANCHOR_TOP_PX, 6)
    const br = at('bottom-right')
    expect(br.edge).toBe('bottom')
    expect(br.css.right).toBeCloseTo(viewport.width - dock.right + ANCHOR_SIDE_PX, 6)
    expect(br.css.bottom).toBeCloseTo(viewport.height - (dock.bottom - ANCHOR_SIDE_PX), 6)
    const bl = at('bottom-left')
    expect([bl.side, bl.edge]).toEqual(['left', 'bottom'])
    expect(bl.css.left).toBeCloseTo(dock.left + ANCHOR_SIDE_PX, 6)
  })

  it('stay in their half when the dock area is short', () => {
    const short: Rect = { ...dock, bottom: 120 }
    expect(storedAnchor(corner('top-left', short, work, viewport), viewport, work).edge).toBe('top')
    expect(storedAnchor(corner('bottom-left', short, work, viewport), viewport, work).edge).toBe('bottom')
  })
})

describe('dragging', () => {
  it('starts after four pixels of movement', () => {
    expect(DRAG_START_PX).toBe(4)
    expect(dragStarts(3, 0)).toBe(false)
    expect(dragStarts(2, 2)).toBe(false)
    expect(dragStarts(4, 0)).toBe(true)
    expect(dragStarts(-3, -3)).toBe(true)
  })

  it('moves the place by the pointer’s movement, keeping the edge it is held by', () => {
    expect(dragged({ x: 1000, y: 300, edge: 'bottom' }, -40, 25)).toEqual({ x: 960, y: 325, edge: 'bottom' })
  })
})

describe('the bubble and the compact forms', () => {
  it('moves the bubble up only as far as it must to stay inside', () => {
    expect(bubbleShift(140, 200, work)).toBe(0)
    // Its bottom would leave the window by 50 px.
    expect(bubbleShift(700, 218, work)).toBe(-50)
    // Taller than the window: its top stays in sight.
    expect(bubbleShift(300, 2000, work)).toBe(work.top + EDGE_PX - 300)
    expect(bubbleShift(Number.NaN, 100, work)).toBe(0)
  })

  it('opens the compact panel under the top bar', () => {
    expect(panelTop({ left: 0, top: 0, right: 420, bottom: 36 }, 36)).toBe(42)
    expect(panelTop(null, 44)).toBe(50)
  })

  it('keeps the compact panel of a desktop window under the face, at most 480 px wide', () => {
    expect([PANEL_MAX_PX, PANEL_EDGE_PX]).toEqual([480, 8])
    // A 900 px window whose face sits at the right end of the agents window's top bar, ending at 880.
    const at = panelPlace('bar', 42, 880, { width: 900, height: 700 })
    expect(at).toEqual({ top: 42, left: 400, width: 480 })
    // Its right edge is under the face's right edge, wherever the face is.
    expect(panelPlace('bar', 42, 1200, viewport)).toEqual({ top: 42, left: 720, width: 480 })
    // A face near the left: the panel keeps inside the window, from its left edge.
    expect(panelPlace('bar', 42, 300, viewport)).toEqual({ top: 42, left: PANEL_EDGE_PX, width: 480 })
    // A face past the right edge (a bar that scrolls): inside all the same.
    expect(panelPlace('bar', 42, 2000, viewport)).toEqual({ top: 42, left: viewport.width - PANEL_EDGE_PX - 480, width: 480 })
    // The face not measured yet: at the window's right edge.
    expect(panelPlace('bar', 42, null, viewport)).toEqual({ top: 42, left: viewport.width - PANEL_EDGE_PX - 480, width: 480 })
    expect(panelPlace('bar', 42, Number.NaN, viewport)).toEqual(panelPlace('bar', 42, null, viewport))
  })

  it('lets the compact panel span the window on the phone, and where 480 px do not fit', () => {
    const edge = { top: 50, left: PANEL_EDGE_PX, right: PANEL_EDGE_PX }
    expect(panelPlace('phone', 50, 380, { width: 390, height: 844 })).toEqual(edge)
    // A phone held sideways is still the phone.
    expect(panelPlace('phone', 50, 830, { width: 844, height: 390 })).toEqual(edge)
    // A desktop window too narrow for 480 px and the edges: from edge to edge, which is narrower.
    expect(panelPlace('bar', 50, 400, { width: 480 + 2 * PANEL_EDGE_PX, height: 600 })).toEqual(edge)
    expect(panelPlace('bar', 50, 400, { width: 480 + 2 * PANEL_EDGE_PX + 1, height: 600 })).toEqual({ top: 50, left: PANEL_EDGE_PX, width: 480 })
  })

  it('puts the phone’s face into the free right part of the header', () => {
    expect(phoneFace({ left: 0, top: 0, right: 390, bottom: 44 }, 40, 44)).toEqual({ top: 2, right: 8 })
    expect(phoneFace(null, 40, 44)).toEqual({ top: 2, right: 8 })
  })
})
