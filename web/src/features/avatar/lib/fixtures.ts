// Stand-ins for the tests of `lib/`: a look as the server would store it, and hand-driven
// replacements for what a browser provides (timers and a clock, the frame scheduler, SVG
// nodes). Imported by `*.test.ts` only.

import type { EngineHost, EngineNode } from './engine'
import type { Animation, Look } from './types'

/**
 * A small character as the server stores one: every tag, references as bare ids, a
 * transform on a group, numbers where SVG had percentages, roles, pivots, a path mouth and
 * a few animations of its own. A fresh object on every call.
 */
export function sampleLook(): Look {
  return {
    viewBox: [0, 0, 200, 200],
    nodes: [
      {
        tag: 'defs',
        children: [
          {
            tag: 'linearGradient',
            id: 'skin',
            attrs: { x1: 0, y1: 0, x2: 0, y2: 1, gradientUnits: 'objectBoundingBox', spreadMethod: 'pad' },
            children: [
              { tag: 'stop', attrs: { offset: 0, stopColor: '#b6f092' } },
              { tag: 'stop', attrs: { offset: 1, stopColor: '#2f8f4bcc', stopOpacity: 0.9 } },
            ],
          },
          {
            tag: 'radialGradient',
            id: 'glow',
            attrs: { cx: 0.5, cy: 0.5, r: 0.5, fx: 0.4, fy: 0.3, gradientTransform: 'rotate(10 0.5 0.5)' },
            children: [
              { tag: 'stop', attrs: { offset: 0, stopColor: 'currentColor' } },
              { tag: 'stop', attrs: { offset: 1, stopColor: '#00000000' } },
            ],
          },
          { tag: 'clipPath', id: 'clip', attrs: { clipPathUnits: 'userSpaceOnUse' }, children: [{ tag: 'circle', attrs: { cx: 100, cy: 80, r: 60 } }] },
        ],
      },
      { tag: 'ellipse', id: 'shadow', attrs: { cx: 100, cy: 180, rx: 46, ry: 7, fill: '#000000', opacity: 0.25 }, box: [54, 173, 92, 14] },
      {
        tag: 'g',
        id: 'body',
        attrs: { stroke: '#256b3a', strokeWidth: 2.5, strokeLinejoin: 'round' },
        box: [40, 20, 120, 160],
        children: [
          { tag: 'rect', id: 'torso', attrs: { x: 70, y: 110, width: 60, height: 70, rx: 18, ry: 18, fillRef: 'skin' }, box: [70, 110, 60, 70] },
          {
            tag: 'g',
            id: 'head',
            attrs: { transform: 'translate(0 -2)', clipPathRef: 'clip' },
            box: [50, 30, 100, 90],
            children: [
              { tag: 'circle', id: 'face', attrs: { cx: 100, cy: 80, r: 50, fillRef: 'skin' }, box: [50, 30, 100, 100] },
              { tag: 'circle', id: 'cheek', attrs: { cx: 70, cy: 100, r: 6, fill: '#ff7a93', fillOpacity: 0.8, stroke: 'none' }, box: [64, 94, 12, 12] },
              {
                tag: 'g',
                id: 'eyes',
                box: [70, 60, 60, 24],
                children: [
                  { tag: 'ellipse', attrs: { cx: 82, cy: 72, rx: 9, ry: 12, fill: '#ffffff' } },
                  { tag: 'ellipse', attrs: { cx: 118, cy: 72, rx: 9, ry: 12, fill: '#ffffff' } },
                  {
                    tag: 'g',
                    id: 'pupils',
                    box: [78, 68, 44, 8],
                    children: [
                      { tag: 'circle', attrs: { cx: 82, cy: 72, r: 4, fill: '#14241b', stroke: 'none' } },
                      { tag: 'circle', attrs: { cx: 118, cy: 72, r: 4, fill: '#14241b', stroke: 'none' } },
                    ],
                  },
                ],
              },
              { tag: 'path', id: 'mouth', attrs: { d: 'M 84 104 Q 100 110 116 104', fill: 'none', strokeLinecap: 'round' }, box: [84, 104, 32, 6] },
              { tag: 'line', id: 'brow', attrs: { x1: 74, y1: 54, x2: 90, y2: 52, strokeWidth: 3 }, box: [74, 52, 16, 2] },
            ],
          },
          { tag: 'path', id: 'armR', attrs: { d: 'M 130 124 Q 150 132 152 154', fill: 'none', strokeWidth: 9, strokeLinecap: 'round' }, box: [130, 124, 22, 30] },
          { tag: 'polygon', id: 'badge', attrs: { points: '96 140 104 140 100 148', fill: 'currentColor', fillRule: 'evenodd', clipRule: 'nonzero' }, box: [96, 140, 8, 8] },
          { tag: 'polyline', attrs: { points: '80 160 100 166 120 160', fill: 'none', strokeMiterlimit: 4, strokeOpacity: 0.5 } },
        ],
      },
      { tag: 'text', id: 'label', attrs: { x: 100, y: 196, fontSize: 12, fontWeight: 'bold', textAnchor: 'middle', dominantBaseline: 'alphabetic', letterSpacing: 0.5, fill: 'currentColor' }, text: 'Pip', box: [90, 186, 20, 12] },
    ],
    pivots: { body: [100, 180], head: [100, 128], armR: [130, 124] },
    roles: { body: 'body', head: 'head', eyes: 'eyes', pupils: 'pupils', armR: 'armR' },
    animations: {
      hop: { duration: 0.6, tracks: [{ target: 'body', prop: 'translateY', values: [0, 4, -18, 0, 2, 0], ease: 'out' }] },
      blush: { duration: 2, loop: true, tracks: [{ target: 'cheek', prop: 'fill', values: ['#ff7a93', '#ff2255', null], times: [0, 0.5, 1] }] },
      grin: { duration: 1, tracks: [{ target: 'mouth', prop: 'd', values: [null, 'M 84 102 Q 100 122 116 102', null] }] },
      peek: { duration: 1.5, every: [3, 6], tracks: [{ target: ['pupils', 'brow'], prop: 'translateX', values: [0, 3, 0], ease: ['inOut', 'out'] }] },
    },
    states: { done: ['breathe', 'blink', 'hop'], waiting: [] },
    mouth: [{ target: 'mouth', open: 'M 84 102 Q 100 130 116 102' }],
    gaze: 4,
  }
}

/** What JSON does to a look on its way from the server: a deep copy that shares nothing. */
export function wire<T>(value: T): unknown {
  return JSON.parse(JSON.stringify(value))
}

/** A clock and timers the test moves by hand. */
export function fakeClock(start = 0) {
  let now = start
  let nextId = 1
  const timers = new Map<number, { at: number; cb: () => void }>()
  return {
    now: () => now,
    setTimeout(cb: () => void, ms: number): number {
      const id = nextId++
      timers.set(id, { at: now + Math.max(0, ms), cb })
      return id
    },
    clearTimeout(id: number): void {
      timers.delete(id)
    },
    /** Moves time forward, running every timer that comes due, in order. */
    advance(ms: number): void {
      const end = now + ms
      for (;;) {
        let next: [number, { at: number; cb: () => void }] | null = null
        for (const entry of timers) if (entry[1].at <= end && (!next || entry[1].at < next[1].at)) next = entry
        if (!next) break
        timers.delete(next[0])
        now = Math.max(now, next[1].at)
        next[1].cb()
      }
      now = end
    },
    /** How many timers are still set. */
    pending: () => timers.size,
  }
}

/** Lets the promises that are already settled run their reactions. */
export async function flush(times = 5): Promise<void> {
  for (let i = 0; i < times; i++) await Promise.resolve()
}

/** An SVG node that records what is written to it. */
export class FakeNode implements EngineNode {
  attrs = new Map<string, string>()
  writes: [string, string | null][] = []
  /** Set to make every write throw, as a node of a torn-down document might. */
  broken = false
  /** Called on every write: a test makes a figure dear with it. */
  onWrite: (() => void) | null = null
  setAttribute(name: string, value: string): void {
    if (this.broken) throw new Error('node is gone')
    this.onWrite?.()
    this.attrs.set(name, value)
    this.writes.push([name, value])
  }
  removeAttribute(name: string): void {
    if (this.broken) throw new Error('node is gone')
    this.onWrite?.()
    this.attrs.delete(name)
    this.writes.push([name, null])
  }
}

/** The frame scheduler, the clock and the randomness of an engine, driven by the test. */
export function fakeFrames() {
  let nextId = 1
  let time = 0
  let clock = 0
  let random = 0.5
  const pending = new Map<number, (tMs: number) => void>()
  const nodes = new Map<string, FakeNode>()
  const host: EngineHost = {
    requestFrame(cb) {
      const id = nextId++
      pending.set(id, cb)
      return id
    },
    cancelFrame(id) {
      pending.delete(id)
    },
    random: () => random,
    now: () => clock,
  }
  return {
    host,
    nodes,
    /** The node for an id, made on first use: what `find` gives the engine. */
    find(id: string): FakeNode {
      let node = nodes.get(id)
      if (!node) {
        node = new FakeNode()
        nodes.set(id, node)
      }
      return node
    },
    /** One frame, `ms` after the last one. */
    frame(ms = 40): void {
      time += ms
      const due = [...pending.values()]
      pending.clear()
      for (const cb of due) cb(time)
    },
    /** Frames of 40 ms until `ms` have passed. */
    run(ms: number): void {
      for (let t = 0; t < ms; t += 40) this.frame(40)
    },
    /** How many frames are requested. */
    waiting: () => pending.size,
    /** Work that takes this long: moves the clock a figure's frames are timed with. */
    spend(ms: number): void {
      clock += ms
    },
    setRandom(value: number): void {
      random = value
    },
    time: () => time,
  }
}

export interface ReadTransform {
  tx: number
  ty: number
  rot: number
  sx: number
  sy: number
  pivot: [number, number] | null
}

/**
 * The numbers of a transform the engine wrote, after the part's own transform `own`:
 * `translate(tx ty) rotate(deg px py) translate(px py) scale(sx sy) translate(-px -py)`,
 * each part only when it is not the identity.
 */
export function readTransform(text: string | undefined, own = ''): ReadTransform {
  const out: ReadTransform = { tx: 0, ty: 0, rot: 0, sx: 1, sy: 1, pivot: null }
  if (!text) return out
  if (!text.startsWith(own)) throw new Error(`"${text}" does not start with the part's own transform "${own}"`)
  const calls = [...text.slice(own.length).matchAll(/([a-zA-Z]+)\(([^)]*)\)/g)].map((m) => ({ fn: m[1], args: m[2].split(' ').map(Number) }))
  let end = calls.length
  if (end >= 3 && calls[end - 1].fn === 'translate' && calls[end - 2].fn === 'scale' && calls[end - 3].fn === 'translate') {
    out.sx = calls[end - 2].args[0]
    out.sy = calls[end - 2].args[1]
    out.pivot = [calls[end - 3].args[0], calls[end - 3].args[1]]
    end -= 3
  }
  if (end >= 1 && calls[end - 1].fn === 'rotate') {
    out.rot = calls[end - 1].args[0]
    out.pivot = [calls[end - 1].args[1], calls[end - 1].args[2]]
    end -= 1
  }
  if (end >= 1 && calls[end - 1].fn === 'translate') {
    out.tx = calls[end - 1].args[0]
    out.ty = calls[end - 1].args[1]
    end -= 1
  }
  if (end !== 0) throw new Error(`unexpected transform "${text}"`)
  return out
}

/**
 * What is wrong with an animation by the rules the server keeps for an agent's own: the
 * app's built-in and default animations keep them too. Empty: nothing.
 */
export function animationProblems(a: Animation): string[] {
  const out: string[] = []
  if (!(a.duration >= 0.1 && a.duration <= 30)) out.push(`duration ${a.duration}`)
  if (a.loop === true && a.every !== undefined) out.push('both loop and every')
  if (a.tracks.length < 1 || a.tracks.length > 16) out.push(`${a.tracks.length} tracks`)
  for (const t of a.tracks) {
    const n = t.values.length
    if (n < 2 || n > 16) out.push(`${t.prop}: ${n} keyframes`)
    if (!t.values.every((v) => typeof v === 'number' && Number.isFinite(v))) out.push(`${t.prop}: a value that is no number`)
    const times = t.times ?? a.times
    if (times && times.length !== n) out.push(`${t.prop}: ${times.length} times for ${n} keyframes`)
    const step = times ? Math.min(...times.slice(1).map((x, i) => x - times[i])) : 1 / (n - 1)
    // Opacity is a paint: its keyframes are at least 0.17 s apart, every other pair 0.04 s.
    const least = t.prop === 'opacity' ? 0.17 : 0.04
    if (step * a.duration < least - 1e-9) out.push(`${t.prop}: keyframes ${step * a.duration} s apart`)
    const ease = t.ease ?? a.ease
    if (Array.isArray(ease) && ease.length !== n - 1) out.push(`${t.prop}: ${ease.length} eases for ${n - 1} segments`)
    if (t.prop === 'opacity' && (ease === 'step' || (Array.isArray(ease) && ease.includes('step')))) out.push('opacity steps')
  }
  return out
}
