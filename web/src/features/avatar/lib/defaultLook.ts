// The default look: what a session shows before it has designed anything, and what a
// design that cannot be shown falls back to.
//
// It is written in the look format, like an agent's own: a small round character with a
// body, a head, eyes, pupils, two arms and a mouth, all named as roles, so Workbench's
// built-in motion drives it. Its colours are the app's tokens only (the token attributes
// of look.ts, which server data can never carry), so it follows the theme; its hue is one
// of the eight chart colours, picked by a stable hash of the seed (the terminal id). It
// animates transforms and opacity only, because a token colour cannot be interpolated.
//
// Pure: no DOM, no globals.

import { parsePath } from './look'
import { deepFreeze, stableHash } from './types'
import type { Box, Look, LookNode, Tag } from './types'

/** The hues a default look can have: the chart series tokens of theme/tokens.css, in their fixed order. */
const HUES: readonly string[] = ['plot-1', 'plot-2', 'plot-3', 'plot-4', 'plot-5', 'plot-6', 'plot-7', 'plot-8']

type Attrs = Record<string, string | number>

function el(tag: Tag, attrs: Attrs, id?: string, children?: LookNode[]): LookNode {
  const node: LookNode = { tag }
  if (id !== undefined) node.id = id
  if (Object.keys(attrs).length > 0) node.attrs = attrs
  if (children) node.children = children
  return node
}

const num = (attrs: Attrs | undefined, key: string): number => {
  const v = attrs?.[key]
  return typeof v === 'number' ? v : 0
}

/** The box of a node's own geometry. Enough for the shapes below: none of them carries a transform. */
function boxOf(node: LookNode): Box | null {
  const a = node.attrs
  switch (node.tag) {
    case 'circle':
      return [num(a, 'cx') - num(a, 'r'), num(a, 'cy') - num(a, 'r'), 2 * num(a, 'r'), 2 * num(a, 'r')]
    case 'ellipse':
      return [num(a, 'cx') - num(a, 'rx'), num(a, 'cy') - num(a, 'ry'), 2 * num(a, 'rx'), 2 * num(a, 'ry')]
    case 'rect':
      return [num(a, 'x'), num(a, 'y'), num(a, 'width'), num(a, 'height')]
    case 'line':
      return [Math.min(num(a, 'x1'), num(a, 'x2')), Math.min(num(a, 'y1'), num(a, 'y2')), Math.abs(num(a, 'x2') - num(a, 'x1')), Math.abs(num(a, 'y2') - num(a, 'y1'))]
    case 'path': {
      // Absolute M, L and Q only: every number pair is a point (control points included).
      const nums = parsePath(a?.d)?.nums ?? []
      let box: Box | null = null
      for (let i = 0; i + 1 < nums.length; i += 2) box = union(box, [nums[i], nums[i + 1], 0, 0])
      return box
    }
    case 'g': {
      let box: Box | null = null
      for (const child of node.children ?? []) box = union(box, boxOf(child))
      return box
    }
    default:
      return null
  }
}

function union(a: Box | null, b: Box | null): Box | null {
  if (!a) return b
  if (!b) return a
  const x = Math.min(a[0], b[0])
  const y = Math.min(a[1], b[1])
  return [x, y, Math.max(a[0] + a[2], b[0] + b[2]) - x, Math.max(a[1] + a[3], b[1] + b[3]) - y]
}

const round = (n: number) => Math.round(n * 1000) / 1000

/** Every part with an id gets its rest-pose box, as the server gives one to an agent's parts. */
function withBoxes(nodes: LookNode[]): LookNode[] {
  for (const node of nodes) {
    if (node.id !== undefined) {
      const box = boxOf(node)
      if (box) node.box = [round(box[0]), round(box[1]), round(box[2]), round(box[3])]
    }
    if (node.children) withBoxes(node.children)
  }
  return nodes
}

function artwork(hue: string): LookNode[] {
  const arm: Attrs = { fill: 'none', strokeToken: hue, strokeWidth: 9.5, strokeLinecap: 'round' }
  const eye: Attrs = { cy: 75, rx: 8.2, ry: 10.4 }
  return withBoxes([
    // The pupils stay inside the eyes however far they look.
    el('defs', {}, undefined, [el('clipPath', {}, 'eyeClip', [el('ellipse', { cx: 82.5, ...eye }), el('ellipse', { cx: 117.5, ...eye })])]),
    el('ellipse', { cx: 100, cy: 174, rx: 37, ry: 5.5, fillToken: 'fg', opacity: 0.14 }, 'shadow'),
    el('g', {}, 'body', [
      el('ellipse', { cx: 100, cy: 143.5, rx: 30, ry: 27.5, fillToken: hue }),
      el('ellipse', { cx: 100, cy: 149, rx: 17.5, ry: 15.5, fillToken: 'fg', opacity: 0.16 }),
      el('g', {}, 'head', [
        el('line', { x1: 100, y1: 40, x2: 100, y2: 32, strokeToken: hue, strokeWidth: 4.5, strokeLinecap: 'round' }),
        el('circle', { cx: 100, cy: 27, r: 6.5, fillToken: 'accent' }, 'bulb'),
        el('ellipse', { cx: 100, cy: 79.5, rx: 50.5, ry: 42, fillToken: hue }),
        el('rect', { x: 60.5, y: 51, width: 79, height: 59, rx: 26, fillToken: 'bg-elevated' }),
        el('ellipse', { cx: 70.5, cy: 94, rx: 5.5, ry: 3.8, fillToken: hue, opacity: 0.45 }),
        el('ellipse', { cx: 129.5, cy: 94, rx: 5.5, ry: 3.8, fillToken: hue, opacity: 0.45 }),
        el('g', {}, 'eyes', [
          el('ellipse', { cx: 82.5, ...eye, fillToken: 'fg' }),
          el('ellipse', { cx: 117.5, ...eye, fillToken: 'fg' }),
          el('g', { clipPathRef: 'eyeClip' }, undefined, [
            el('g', {}, 'pupils', [el('circle', { cx: 83.6, cy: 77.2, r: 4, fillToken: 'bg-elevated' }), el('circle', { cx: 118.6, cy: 77.2, r: 4, fillToken: 'bg-elevated' })]),
          ]),
        ]),
        // A small smile at rest; lip sync stretches it downwards from its upper edge.
        el('path', { d: 'M 88 92.5 Q 100 93.2 112 92.5 Q 100 100.5 88 92.5 Z', fillToken: 'fg' }, 'mouth'),
      ]),
      // The arms come last, so a raised one shows in front of the head; the hands keep it readable there.
      el('g', {}, 'armL', [el('path', { d: 'M 73 128 Q 55 133 52 153', ...arm }), el('circle', { cx: 52, cy: 154, r: 6.2, fillToken: 'accent' })]),
      el('g', {}, 'armR', [el('path', { d: 'M 127 128 Q 145 133 148 153', ...arm }), el('circle', { cx: 148, cy: 154, r: 6.2, fillToken: 'accent' })]),
    ]),
  ])
}

function make(hue: string): Look {
  return {
    viewBox: [0, 0, 200, 200],
    nodes: artwork(hue),
    pivots: { body: [100, 171], head: [100, 117], eyes: [100, 75], armL: [73, 128], armR: [127, 128], mouth: [100, 92.5] },
    roles: { body: 'body', head: 'head', eyes: 'eyes', pupils: 'pupils', armL: 'armL', armR: 'armR' },
    animations: {
      // The lamp on its head breathes while it works or waits for the user.
      glow: { duration: 1.8, loop: true, tracks: [{ target: 'bulb', prop: 'opacity', values: [1, 0.4, 1], ease: 'inOut' }] },
      sway: { duration: 4.4, loop: true, tracks: [{ target: 'head', prop: 'rotate', values: [-1.5, 1.5, -1.5], ease: 'inOut' }] },
    },
    states: {
      idle: ['breathe', 'blink', 'sway'],
      working: ['breathe', 'think', 'blink', 'glow'],
      waiting: ['breathe', 'blink', 'sway', 'glow'],
      attention: ['alert', 'blink', 'glow'],
    },
    mouth: [{ target: 'mouth', prop: 'scaleY', open: 3.4 }],
    gaze: 3.4,
  }
}

const MADE = new Map<string, Look>()

/**
 * The default look for a session. `seed` is the terminal id: the same session always gets
 * the same hue. Looks of one hue are one shared, frozen object, so its identity is stable
 * across calls (a figure is not rebuilt on every render).
 */
export function defaultLook(seed: string): Look {
  const hue = defaultHue(seed)
  let look = MADE.get(hue)
  if (!look) {
    look = deepFreeze(make(hue))
    MADE.set(hue, look)
  }
  return look
}

/** The token a seed's default look is painted in (`plot-1` to `plot-8`). */
export function defaultHue(seed: string): string {
  return HUES[stableHash(typeof seed === 'string' ? seed : '') % HUES.length]
}
