// Where the avatars sit: which of the layouts applies, where the floating group is
// placed inside the workspace window, and what a drag or a "Move to" stores.
//
// Wide (desktop shell, workspace window open and large enough): the group floats over
// the workspace window and never leaves its rectangle, so it never sits on the agents
// column, where permission requests are answered. Compact (workspace window collapsed, or
// the phone shell): a face in a top bar and the bubble as a panel under it.
//
// The group is a column of figures that hugs one side: figures grow towards the middle of
// the window, and the bubble opens on that side too. Its place is one point: the centre
// line of the column of resting figures, and the group's top edge (or its bottom edge,
// in the lower half of the window, so that it grows upwards there).
//
// Pure: rectangles and sizes in viewport pixels come in, numbers go out. Nothing is measured here.

/** A rectangle in viewport pixels, as `getBoundingClientRect` gives it. */
export interface Rect {
  left: number
  top: number
  right: number
  bottom: number
}

export interface Size {
  width: number
  height: number
}

/** `wide`: the floating group. `bar`: the desktop shell's top bar face. `phone`: the phone's header face. */
export type Mode = 'wide' | 'bar' | 'phone'

/** The workspace window must be at least this large for the floating group (the group with its bubble fits beside its right edge). */
export const MIN_WIDE_WIDTH = 480
export const MIN_WIDE_HEIGHT = 320
/** The width of the column of resting figures: a stage of 56 px and its ring. */
export const COLUMN_PX = 64
/** How far the active figure and its bubble reach from the column's centre line towards the middle: 140 px of figure, less half the column, 12 px of gap and 300 px of bubble. */
export const REACH_PX = 420
/** The default place: below the dock's chrome rows, a little in from its right edge. */
export const ANCHOR_TOP_PX = 104
export const ANCHOR_SIDE_PX = 16
/** What stays free between the group or its bubble and the edge of the workspace window. */
export const EDGE_PX = 8
/** The same at the sides, where a label may be a little wider than its figure. */
export const SIDE_EDGE_PX = 20
/** A press becomes a drag after this much movement. */
export const DRAG_START_PX = 4
/** Narrower than this, the compact panel takes the phone's figure sizes. */
export const SMALL_PANEL_WIDTH = 480
const PANEL_GAP_PX = 6
const PHONE_FACE_RIGHT_PX = 8

const width = (r: Rect): number => r.right - r.left
const height = (r: Rect): number => r.bottom - r.top
const finite = (r: Rect | null | undefined): r is Rect => !!r && [r.left, r.top, r.right, r.bottom].every((n) => typeof n === 'number' && Number.isFinite(n))
const clamp = (n: number, lo: number, hi: number): number => (hi < lo ? lo : Math.min(Math.max(n, lo), hi))
const fraction = (n: number): number => (Number.isFinite(n) ? Math.min(Math.max(n, 0), 1) : 0)

/** Which layout applies. `work`: the rectangle of the workspace window, or null when it is not on screen. */
export function layoutMode(i: { mobile: boolean; workOpen: boolean; work: Rect | null }): Mode {
  if (i.mobile) return 'phone'
  if (!i.workOpen || !finite(i.work)) return 'bar'
  return width(i.work) >= MIN_WIDE_WIDTH && height(i.work) >= MIN_WIDE_HEIGHT ? 'wide' : 'bar'
}

/** Whether the compact panel shows the smaller figure (the phone's sizes). */
export function smallFigures(mode: Mode, viewportWidth: number): boolean {
  return mode === 'phone' || (mode === 'bar' && viewportWidth < SMALL_PANEL_WIDTH)
}

// ---------------------------------------------------------------- the group's place

export interface Anchor {
  /** The centre line of the column of resting figures. */
  x: number
  /** The group's top edge, or its bottom edge. */
  y: number
  edge: 'top' | 'bottom'
}

export interface Placement extends Anchor {
  /** The side the group hugs. Figures grow towards the other one, and the bubble opens there. */
  side: 'left' | 'right'
  /** Where the bubble's tail points: at the figure, on this side of the bubble. */
  tail: 'left' | 'right'
  /** The group's absolute position inside the layer: one of `left` and `right`, one of `top` and `bottom`. */
  css: { left?: number; right?: number; top?: number; bottom?: number }
}

/** The default place: the dock area's top right, below its chrome rows. */
export function defaultAnchor(dock: Rect): Anchor {
  return { x: dock.right - ANCHOR_SIDE_PX - COLUMN_PX / 2, y: dock.top + ANCHOR_TOP_PX, edge: 'top' }
}

/** A stored position (fractions of the viewport) as a place: above the middle of the workspace window it is the group's top edge, below it its bottom edge. */
export function storedAnchor(pos: { x: number; y: number }, viewport: Size, work: Rect): Anchor {
  const y = fraction(pos.y) * viewport.height
  return { x: fraction(pos.x) * viewport.width, y, edge: y < (work.top + work.bottom) / 2 ? 'top' : 'bottom' }
}

/**
 * The place brought into the workspace window: the column inside it, the group inside
 * it as far as its height allows, and on a side where the bubble has room. Between two
 * such sides it hugs the nearer edge.
 */
export function place(anchor: Anchor, group: Size, work: Rect, viewport: Size): Placement {
  const half = COLUMN_PX / 2
  const lo = work.left + SIDE_EDGE_PX + half
  const hi = Math.max(lo, work.right - SIDE_EDGE_PX - half)
  /** Hugging the right side, the bubble opens to the left and needs this much room there; and the other way round. */
  const rightFrom = Math.max(lo, work.left + EDGE_PX + REACH_PX)
  const leftUntil = Math.min(hi, work.right - EDGE_PX - REACH_PX)
  const canRight = rightFrom <= hi
  const canLeft = leftUntil >= lo
  let x = clamp(Number.isFinite(anchor.x) ? anchor.x : hi, lo, hi)
  let side: 'left' | 'right'
  if (canLeft && canRight && x >= rightFrom && x <= leftUntil) {
    // Room on both sides: it hugs the side it is nearer to.
    side = x < (work.left + work.right) / 2 ? 'left' : 'right'
  } else if (canRight && x >= rightFrom) {
    side = 'right'
  } else if (canLeft && x <= leftUntil) {
    side = 'left'
  } else if (canLeft && (!canRight || x - leftUntil <= rightFrom - x)) {
    side = 'left'
    x = leftUntil
  } else {
    // Too narrow for a bubble on either side: the right edge, where the default place is.
    side = 'right'
    x = canRight ? rightFrom : hi
  }

  const top = work.top + EDGE_PX
  const bottom = Math.max(top, work.bottom - EDGE_PX)
  const h = Math.max(0, Number.isFinite(group.height) ? group.height : 0)
  let edge: 'top' | 'bottom' = anchor.edge === 'bottom' ? 'bottom' : 'top'
  let y = Number.isFinite(anchor.y) ? anchor.y : top
  if (edge === 'bottom' && h > bottom - top) {
    // Taller than the window: its top is what stays in sight.
    edge = 'top'
    y = top
  } else if (edge === 'bottom') {
    y = clamp(y, top + h, bottom)
  } else {
    y = clamp(y, top, bottom - h)
  }

  const css: Placement['css'] = {}
  if (side === 'right') css.right = viewport.width - (x + half)
  else css.left = x - half
  if (edge === 'top') css.top = y
  else css.bottom = viewport.height - y
  return { x, y, edge, side, tail: side, css }
}

/** The place while the group is dragged by (`dx`, `dy`) from where the drag began. */
export function dragged(from: Anchor, dx: number, dy: number): Anchor {
  return { x: from.x + dx, y: from.y + dy, edge: from.edge }
}

/** Whether a press has moved far enough to be a drag. */
export function dragStarts(dx: number, dy: number): boolean {
  return Math.hypot(dx, dy) >= DRAG_START_PX
}

/**
 * What to store for a place (after a drag): fractions of the viewport. A group whose
 * middle is in the upper half of the workspace window is stored by its top edge, one in
 * the lower half by its bottom edge, which is how `storedAnchor` reads it back.
 */
export function toStored(at: Anchor, group: Size, work: Rect, viewport: Size): { x: number; y: number } {
  const h = Math.max(0, Number.isFinite(group.height) ? group.height : 0)
  const top = at.edge === 'top' ? at.y : at.y - h
  const upper = top + h / 2 < (work.top + work.bottom) / 2
  return { x: fraction(at.x / viewport.width), y: fraction((upper ? top : top + h) / viewport.height) }
}

export type Corner = 'top-right' | 'bottom-right' | 'bottom-left' | 'top-left'

/**
 * A corner of the dock area as a stored position: in from the tool-window stripes and the
 * dock's chrome rows, as the default place is. `work` decides which half a place is in.
 */
export function corner(which: Corner, dock: Rect, work: Rect, viewport: Size): { x: number; y: number } {
  const middle = (work.top + work.bottom) / 2
  const x = which === 'top-right' || which === 'bottom-right' ? dock.right - ANCHOR_SIDE_PX - COLUMN_PX / 2 : dock.left + ANCHOR_SIDE_PX + COLUMN_PX / 2
  // A top corner is read back by its top edge only above the middle, a bottom corner by its bottom edge only below it.
  const y = which === 'top-right' || which === 'top-left' ? Math.min(dock.top + ANCHOR_TOP_PX, middle - 1) : Math.max(dock.bottom - ANCHOR_SIDE_PX, middle)
  return { x: fraction(x / viewport.width), y: fraction(y / viewport.height) }
}

// ---------------------------------------------------------------- the bubble and the compact forms

/**
 * How far the bubble is moved from its figure's top edge so that it stays inside the
 * workspace window: 0 where it fits, negative to move it up. `slotTop`: the top of the
 * figure it belongs to.
 */
export function bubbleShift(slotTop: number, bubbleHeight: number, work: Rect): number {
  /** No further up than this: its top stays inside. */
  const least = work.top + EDGE_PX - slotTop
  /** At least this far up: its bottom stays inside. */
  const most = work.bottom - EDGE_PX - bubbleHeight - slotTop
  // Up only as far as needed; when it is taller than the window, its top is what stays in sight.
  const shift = Math.max(Math.min(0, most), least)
  return Number.isFinite(shift) ? Math.round(shift) : 0
}

/** Where the compact panel begins: under the top bar. */
export function panelTop(bar: Rect | null, fallback: number): number {
  return (finite(bar) ? bar.bottom : fallback) + PANEL_GAP_PX
}

/** The compact panel in the desktop shell's top bar is at most this wide: under the face, not across the window. */
export const PANEL_MAX_PX = 480
/** What stays free between the compact panel and the window's edges. */
export const PANEL_EDGE_PX = 8

/**
 * Where the compact panel sits, as the absolute position of its place. On the phone it
 * spans the window from edge to edge. In the desktop shell's top bar it is at most
 * `PANEL_MAX_PX` wide and its right edge is under the face's right edge (`faceRight`; the
 * window's right edge when the face is not measured yet), kept inside the window.
 */
export function panelPlace(mode: Mode, top: number, faceRight: number | null, viewport: Size): { top: number; left: number; right?: number; width?: number } {
  const room = Math.max(0, (Number.isFinite(viewport.width) ? viewport.width : 0) - 2 * PANEL_EDGE_PX)
  if (mode === 'phone' || room <= PANEL_MAX_PX) return { top, left: PANEL_EDGE_PX, right: PANEL_EDGE_PX }
  const width = PANEL_MAX_PX
  const rightEdge = clamp(typeof faceRight === 'number' && Number.isFinite(faceRight) ? faceRight : viewport.width - PANEL_EDGE_PX, PANEL_EDGE_PX + width, viewport.width - PANEL_EDGE_PX)
  return { top, left: Math.round(rightEdge - width), width }
}

/** Where the phone's face sits: in the free right part of the header, centred in its height. */
export function phoneFace(header: Rect | null, size: number, fallbackHeight: number): { top: number; right: number } {
  const top = finite(header) ? header.top : 0
  const h = finite(header) ? height(header) : fallbackHeight
  return { top: Math.round(top + Math.max(0, (h - size) / 2)), right: PHONE_FACE_RIGHT_PX }
}
