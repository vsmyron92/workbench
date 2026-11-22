// The engine in three layers: `compile` (a look into numbers), `sample` (clips into poses,
// pure) and `createEngine` (the frame loop, with the frame scheduler and the nodes faked).

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { defaultLook } from './defaultLook'
import { BUDGET_FRAMES, GESTURE_BLEND_MS, MOUTH_TAIL_MS, STATE_FADE_MS, compile, createEngine, ease, easeMouth, sample } from './engine'
import type { Clip, Compiled, Pose } from './engine'
import { fakeFrames, readTransform, sampleLook, wire } from './fixtures'
import { checkLook, lookElements } from './look'
import type { Animation, Look, Track } from './types'

/** A plain rig with no roles (so no built-in motion gets in the way), plus whatever a test adds. */
function rig(animations: Record<string, Animation> = {}, extra: Partial<Look> = {}): Look {
  return {
    viewBox: [0, 0, 200, 200],
    nodes: [
      {
        tag: 'g',
        id: 'body',
        box: [50, 20, 100, 160],
        children: [
          {
            tag: 'g',
            id: 'head',
            attrs: { transform: 'translate(0 -2)' },
            box: [60, 20, 80, 70],
            children: [
              { tag: 'g', id: 'eyes', box: [70, 40, 60, 20], children: [{ tag: 'g', id: 'pupils', box: [80, 46, 40, 8] }] },
              { tag: 'path', id: 'mouth', attrs: { d: 'M 84 70 Q 100 74 116 70' }, box: [84, 70, 32, 4] },
            ],
          },
          { tag: 'circle', id: 'dot', attrs: { cx: 100, cy: 120, r: 6, fill: '#ff0000', opacity: 0.5 }, box: [94, 114, 12, 12] },
          { tag: 'ellipse', id: 'oval', attrs: { cx: 100, cy: 150, rx: 8, ry: 2 }, box: [92, 148, 16, 4] },
          { tag: 'g', id: 'loose' },
        ],
      },
    ],
    pivots: { head: [100, 90] },
    roles: {},
    animations,
    states: {},
    mouth: [],
    gaze: null,
    ...extra,
  }
}

/** The same rig with two gradients and a part painted with them: `ball`, filled with `sky` and outlined with `sun`. */
function painted(animations: Record<string, Animation> = {}, extra: Partial<Look> = {}): Look {
  const look = rig(animations, extra)
  look.nodes.unshift({
    tag: 'defs',
    children: [
      { tag: 'linearGradient', id: 'sky', children: [{ tag: 'stop', attrs: { offset: 0, stopColor: '#112233' } }, { tag: 'stop', attrs: { offset: 1, stopColor: '#445566' } }] },
      { tag: 'radialGradient', id: 'sun', children: [{ tag: 'stop', attrs: { offset: 0, stopColor: '#ffee88' } }] },
    ],
  })
  look.nodes.push({ tag: 'circle', id: 'ball', attrs: { cx: 30, cy: 30, r: 10, fillRef: 'sky', strokeRef: 'sun', strokeWidth: 2 }, box: [20, 20, 20, 20] })
  return look
}

const one = (track: Partial<Track> & Pick<Track, 'prop' | 'values'>, more: Partial<Animation> = {}): Animation => ({ duration: 1, ...more, tracks: [{ target: 'dot', ...track }] })
const clip = (name: string, over: Partial<Clip> = {}): Clip => ({ name, startMs: 0, weight: 1, mode: 'state', times: 1, ...over })

function compiled(look: Look): Compiled {
  const c = compile(look)
  if (!c) throw new Error('the look does not compile')
  return c
}

/** The pose of one part. */
function pose(c: Compiled, clips: Clip[], tMs: number, id = 'dot', mouth: number | null = null, gaze: readonly [number, number] | null = null, reducedMotion = false): Pose {
  const p = sample(c, clips, tMs, mouth, gaze, { reducedMotion }).get(id)
  if (!p) throw new Error(`no pose for ${id}`)
  return p
}
const moved = (c: Compiled, clips: Clip[], tMs: number, id = 'dot', own = '') => readTransform(pose(c, clips, tMs, id).transform, own)

const NUM = '-?\\d+(?:\\.\\d+)?'
const TRANSFORM = new RegExp(`^(?:(?:translate|rotate|scale|skewX|skewY|matrix)\\(${NUM}(?: ${NUM})*\\)(?: (?=[a-z])|$))+$`)
const ATTR = new RegExp(`^(?:${NUM}|rgb\\(\\d+,\\d+,\\d+\\)|rgba\\(\\d+,\\d+,\\d+,${NUM}\\)|none|currentColor|[A-Za-z](?: ${NUM})*(?: [A-Za-z](?: ${NUM})*)*)$`)
/** A gradient a part rests in, as a pose names it: an id of the look. */
const REF = /^[A-Za-z][A-Za-z0-9_-]{0,31}$/

/** Everything the engine may write is a string it formatted itself from finite numbers; a gradient is named by its id. */
function expectFormatted(poses: Map<string, Pose>) {
  for (const [id, p] of poses) {
    if (p.transform !== undefined) expect(p.transform, `${id} transform`).toMatch(TRANSFORM)
    for (const [name, value] of p.attrs) if (value !== null) expect(value, `${id} ${name}`).toMatch(ATTR)
    for (const [name, ref] of p.refs ?? []) {
      expect(['fill', 'stroke'], `${id} ${name}`).toContain(name)
      expect(ref, `${id} ${name}`).toMatch(REF)
      expect(p.attrs.has(name), `${id} ${name}`).toBe(false)
    }
  }
}

describe('ease', () => {
  it('eases a segment', () => {
    expect(ease('linear', 0.3)).toBe(0.3)
    expect(ease('in', 0.5)).toBe(0.25)
    expect(ease('out', 0.5)).toBe(0.75)
    expect(ease('inOut', 0.25)).toBe(0.125)
    expect(ease('inOut', 0.75)).toBe(0.875)
    // Past the target and back.
    expect(ease('outBack', 0.5)).toBeCloseTo(1.0877, 3)
    expect(ease('outBack', 1)).toBeCloseTo(1, 9)
    expect(ease('outBack', 0)).toBeCloseTo(0, 9)
    // Holds the start, jumps at the end.
    expect(ease('step', 0.99)).toBe(0)
    expect(ease('step', 1)).toBe(1)
    for (const kind of ['linear', 'in', 'out', 'inOut'] as const) {
      expect(ease(kind, 0)).toBe(0)
      expect(ease(kind, 1)).toBe(1)
    }
    expect(ease('nonsense' as 'linear', 0.4)).toBe(0.4)
  })
})

describe('compile', () => {
  it('accepts a stored look and the default look, once per look object', () => {
    const look = checkLook(wire(sampleLook())) as Look
    const c = compile(look)
    expect(c).not.toBeNull()
    expect(c?.look).toBe(look)
    expect(Object.isFrozen(c)).toBe(true)
    expect(compile(look)).toBe(c)
    expect(compile(defaultLook('t1'))).not.toBeNull()
    expect(compile(rig())).not.toBeNull()
  })

  it('rejects a value that does not fit its prop', () => {
    const bad: Animation[] = [
      one({ prop: 'translateX', values: [0, '10' as unknown as number] }),
      one({ prop: 'opacity', values: [0, true as unknown as number] }),
      one({ prop: 'opacity', values: [0, Number.NaN] }),
      one({ prop: 'translateX', values: [0, Infinity] }),
      one({ prop: 'fill', values: ['#ff0000', 'none'] }),
      one({ prop: 'fill', values: ['#ff0000', 'url(#x)'] }),
      one({ prop: 'fill', values: ['#ff0000', 5 as unknown as string] }),
      one({ prop: 'd', values: ['M 0 0', 5 as unknown as string], target: 'mouth' }),
      one({ prop: 'd', values: ['M 0 0 L 1 1', 'M 0 0 Q 1 1 2 2'], target: 'mouth' }),
      one({ prop: 'd', values: ['M 0 0 A 5 5 0 0 1 9 9', 'M 0 0 A 5 5 0 1 1 9 9'], target: 'mouth' }),
      one({ prop: 'd', values: ['M 0 0', 'javascript:alert(1)'], target: 'mouth' }),
      one({ prop: 'd', values: ['M 0 0', 'M 1 1'] }),
      one({ prop: 'r', values: [1, 2], target: 'mouth' }),
      one({ prop: 'width', values: [1, 2] }),
      one({ prop: 'translateX', values: [0, 1], target: 'nowhere' }),
      one({ prop: 'translateX', values: [0, 1], target: 'constructor' }),
      one({ prop: 'translateX', values: [0, 1], target: [] }),
      one({ prop: 'translateX', values: [0, 1], target: 7 as unknown as string }),
      one({ prop: 'constructor' as 'opacity', values: [0, 1] }),
      one({ prop: 'translateX', values: [5] }),
      one({ prop: 'translateX', values: Array.from({ length: 17 }, (_, i) => i) }),
      one({ prop: 'translateX', values: 'far' as unknown as number[] }),
      one({ prop: 'translateX', values: [0, 1, 2], times: [0, 0.5, 0.5] }),
      one({ prop: 'translateX', values: [0, 1, 2], times: [0, 0.7, 0.4] }),
      one({ prop: 'translateX', values: [0, 1, 2], times: [0.1, 0.5, 1] }),
      one({ prop: 'translateX', values: [0, 1, 2], times: [0, 0.5, 0.9] }),
      one({ prop: 'translateX', values: [0, 1, 2], times: [0, Number.NaN, 1] }),
      one({ prop: 'translateX', values: [0, 1], ease: 'bounce' as 'linear' }),
      one({ prop: 'translateX', values: [0, 1] }, { duration: 0 }),
      one({ prop: 'translateX', values: [0, 1] }, { duration: -1 }),
      one({ prop: 'translateX', values: [0, 1] }, { duration: Number.NaN }),
      one({ prop: 'translateX', values: [0, 1] }, { duration: Infinity }),
      { duration: 1, tracks: [] },
      { duration: 1, tracks: 'none' as unknown as Track[] },
      { duration: 1, tracks: [null as unknown as Track] },
      null as unknown as Animation,
    ]
    for (const [i, animation] of bad.entries()) expect(compile(rig({ a: animation })), `case ${i}`).toBeNull()
    expect(compile(rig({ constructor: one({ prop: 'opacity', values: [0, 1] }) }))).toBeNull()
    expect(compile(rig(Object.fromEntries(Array.from({ length: 33 }, (_, i) => [`a${i}`, one({ prop: 'opacity', values: [0, 1] })]))))).toBeNull()
    expect(compile(null as unknown as Look)).toBeNull()
    expect(compile('look' as unknown as Look)).toBeNull()
  })

  it('rejects a mouth that cannot move', () => {
    expect(compile(rig({}, { mouth: [{ target: 'mouth', open: 'M 84 68 Q 100 90 116 68' }] }))).not.toBeNull()
    expect(compile(rig({}, { mouth: [{ target: 'mouth', open: 'M 84 68 L 116 68' }] }))).toBeNull()
    expect(compile(rig({}, { mouth: [{ target: 'mouth', open: 'M 0 0 L 9 9', closed: 'M 1 1 L 8 8' }] }))).not.toBeNull()
    expect(compile(rig({}, { mouth: [{ target: 'dot', open: 'M 0 0' }] }))).toBeNull()
    expect(compile(rig({}, { mouth: [{ target: 'nowhere', prop: 'scaleY', open: 2 }] }))).toBeNull()
    expect(compile(rig({}, { mouth: [{ target: 'dot', prop: 'ry', open: 2 }] }))).toBeNull()
    expect(compile(rig({}, { mouth: [{ target: 'oval', prop: 'ry', open: 9 }] }))).not.toBeNull()
    expect(compile(rig({}, { mouth: [{ target: 'oval', prop: 'opacity' as 'ry', open: 1 }] }))).toBeNull()
    expect(compile(rig({}, { mouth: [{ target: 'oval', prop: 'scaleY', open: 'wide' as unknown as number }] }))).toBeNull()
    expect(compile(rig({}, { mouth: Array.from({ length: 5 }, () => ({ target: 'oval', prop: 'scaleY' as const, open: 2 })) }))).toBeNull()
  })

  it('colours a part that is painted with a gradient, and names the gradient it rests in', () => {
    const look = checkLook(wire(sampleLook())) as Look
    look.animations.tint = { duration: 2, loop: true, tracks: [{ target: ['face', 'cheek'], prop: 'fill', values: ['#ff0000', '#0000ff', '#ff0000'] }] }
    const c = compiled(look)
    const poses = sample(c, [clip('tint')], 500, null, null)
    // The cheek has a plain colour, the face a gradient: the track colours both.
    expect(poses.get('cheek')?.attrs.get('fill')).toBe('rgb(128,0,128)')
    expect(poses.get('face')?.attrs.get('fill')).toBe('rgb(128,0,128)')
    expect(poses.get('face')?.refs).toBeUndefined()
    // At rest the face is in its gradient: the pose names it and holds no colour for it.
    const rest = sample(c, [], 0, null, null)
    expect(rest.get('face')?.attrs.has('fill')).toBe(false)
    expect([...(rest.get('face')?.refs ?? [])]).toEqual([['fill', 'skin']])
    expect(rest.get('cheek')?.refs).toBeUndefined()
    expect(rest.get('cheek')?.attrs.get('fill')).toBe('rgb(255,122,147)')
  })

  it('has no colour to blend a gradient with: null holds the keyframe beside it, and a weight does not thin the colour', () => {
    const c = compiled(painted({
      flush: one({ target: 'ball', prop: 'fill', values: [null, '#00ff00', null] }),
      edge: one({ target: 'ball', prop: 'stroke', values: ['#ff0000', '#0000ff'] }),
    }))
    expect(pose(c, [clip('flush')], 0, 'ball').attrs.get('fill')).toBe('rgb(0,255,0)')
    expect(pose(c, [clip('flush', { weight: 0.25 })], 500, 'ball').attrs.get('fill')).toBe('rgb(0,255,0)')
    // While its fill is coloured, its outline is still the other gradient.
    expect([...(pose(c, [clip('flush')], 500, 'ball').refs ?? [])]).toEqual([['stroke', 'sun']])
    expect(pose(c, [clip('edge', { mode: 'once' })], 500, 'ball').attrs.get('stroke')).toBe('rgb(128,0,128)')
    // Played: both gradients again.
    const after = pose(c, [clip('flush'), clip('edge', { mode: 'once' })], 5000, 'ball')
    expect([...(after.refs ?? [])].sort()).toEqual([['fill', 'sky'], ['stroke', 'sun']])
    expect(after.attrs.has('fill') || after.attrs.has('stroke')).toBe(false)
  })

  it('leaves a paint it could not put back alone: one of the app’s tokens, a reference to no gradient', () => {
    const look = painted({ tint: one({ target: ['themed', 'lost', 'odd', 'worse', 'dot'], prop: 'fill', values: ['#ff0000', '#0000ff'] }) })
    look.nodes.push(
      { tag: 'circle', id: 'themed', attrs: { fillToken: 'accent' } },
      { tag: 'circle', id: 'lost', attrs: { fillRef: 'nowhere' } },
      { tag: 'circle', id: 'odd', attrs: { fillRef: 'dot' } },
      { tag: 'circle', id: 'worse', attrs: { fillRef: 'sky) url(//x' } },
    )
    const c = compiled(look)
    const poses = sample(c, [clip('tint')], 500, null, null)
    expect(poses.get('dot')?.attrs.get('fill')).toBe('rgb(128,0,128)')
    for (const id of ['themed', 'lost', 'odd', 'worse']) expect(poses.has(id), id).toBe(false)
    // The default look is painted with tokens throughout: none of its parts is given a paint.
    for (const p of sample(compiled(defaultLook('t1')), [clip('glow'), clip('wave', { mode: 'once' })], 300, 0.5, [1, 0]).values()) {
      expect(p.refs).toBeUndefined()
      expect(p.attrs.has('fill') || p.attrs.has('stroke')).toBe(false)
    }
  })
})

describe('sample: one track', () => {
  it('interpolates between keyframes, evenly spaced by default', () => {
    const c = compiled(rig({ slide: one({ prop: 'translateX', values: [0, 10, 0] }) }))
    expect(moved(c, [clip('slide')], 0).tx).toBe(0)
    expect(moved(c, [clip('slide')], 250).tx).toBe(5)
    expect(moved(c, [clip('slide')], 500).tx).toBe(10)
    expect(moved(c, [clip('slide')], 750).tx).toBe(5)
    expect(pose(c, [clip('slide')], 250).transform).toBe('translate(5 0)')
    // Started later: the same motion, later.
    expect(moved(c, [clip('slide', { startMs: 1000 })], 1250).tx).toBe(5)
    // Not started yet: the part is at rest.
    expect(pose(c, [clip('slide', { startMs: 1000 })], 500).transform).toBeUndefined()
  })

  it('follows its times', () => {
    const c = compiled(rig({ slide: one({ prop: 'translateX', values: [0, 10, 0], times: [0, 0.2, 1] }) }))
    expect(moved(c, [clip('slide')], 100).tx).toBe(5)
    expect(moved(c, [clip('slide')], 200).tx).toBe(10)
    expect(moved(c, [clip('slide')], 600).tx).toBe(5)
  })

  it('takes the animation’s times and ease when it has none of its own', () => {
    const c = compiled(rig({ slide: one({ prop: 'translateX', values: [0, 10, 0] }, { times: [0, 0.2, 1], ease: 'in' }) }))
    expect(moved(c, [clip('slide')], 100).tx).toBe(2.5)
    const own = compiled(rig({ slide: one({ prop: 'translateX', values: [0, 10, 0], times: [0, 0.5, 1], ease: 'linear' }, { times: [0, 0.2, 1], ease: 'in' }) }))
    expect(moved(own, [clip('slide')], 250).tx).toBe(5)
  })

  it('eases each segment by its own ease', () => {
    const c = compiled(rig({ slide: one({ prop: 'translateX', values: [0, 10, 20, 30, 40, 50], ease: ['in', 'out', 'inOut', 'outBack', 'step'] }) }))
    const at = (ms: number) => moved(c, [clip('slide')], ms).tx
    expect(at(100)).toBe(2.5)
    expect(at(300)).toBe(17.5)
    expect(at(450)).toBe(21.25)
    expect(at(700)).toBeCloseTo(40.88, 2)
    expect(at(900)).toBe(40)
    expect(at(999)).toBe(40)
    expect(at(1000)).toBe(50)
  })

  it('loops, and starts a loop at its phase', () => {
    const c = compiled(rig({ spin: one({ prop: 'rotate', values: [0, 100] }, { loop: true }), late: one({ prop: 'rotate', values: [0, 100], phase: 0.25 }, { loop: true }) }))
    expect(moved(c, [clip('spin')], 250).rot).toBe(25)
    expect(moved(c, [clip('spin')], 1250).rot).toBe(25)
    expect(moved(c, [clip('spin')], 7500).rot).toBe(50)
    expect(moved(c, [clip('late')], 0).rot).toBe(25)
    expect(moved(c, [clip('late')], 500).rot).toBe(75)
    expect(moved(c, [clip('late')], 800).rot).toBeCloseTo(5, 6)
  })

  it('starts a delayed track of a one-shot later, and plays all of it', () => {
    const c = compiled(rig({ go: { duration: 1, tracks: [{ target: 'dot', prop: 'translateX', values: [0, 10] }, { target: 'dot', prop: 'translateY', values: [0, 10], delay: 0.5 }] } }))
    expect(moved(c, [clip('go')], 250)).toMatchObject({ tx: 2.5, ty: 0 })
    expect(moved(c, [clip('go')], 750)).toMatchObject({ tx: 7.5, ty: 2.5 })
    expect(moved(c, [clip('go')], 1500).ty).toBe(10)
  })

  it('takes null for the artwork’s own value', () => {
    const c = compiled(rig({
      fade: one({ prop: 'opacity', values: [null, 0, null] }),
      tint: one({ prop: 'fill', values: [null, '#0000ff'] }),
      grow: one({ prop: 'r', values: [null, 12] }),
      nudge: one({ prop: 'translateX', values: [null, 10, null] }),
      smile: one({ target: 'mouth', prop: 'd', values: [null, 'M 84 68 Q 100 90 116 68', null] }),
    }))
    expect(pose(c, [clip('fade')], 0).attrs.get('opacity')).toBe('0.5')
    expect(pose(c, [clip('fade')], 250).attrs.get('opacity')).toBe('0.25')
    expect(pose(c, [clip('fade')], 500).attrs.get('opacity')).toBe('0')
    expect(pose(c, [clip('tint')], 500).attrs.get('fill')).toBe('rgb(128,0,128)')
    expect(pose(c, [clip('grow')], 500).attrs.get('r')).toBe('9')
    expect(moved(c, [clip('nudge')], 250).tx).toBe(5)
    expect(pose(c, [clip('smile')], 0, 'mouth').attrs.get('d')).toBe('M 84 70 Q 100 74 116 70')
    expect(pose(c, [clip('smile')], 250, 'mouth').attrs.get('d')).toBe('M 84 69 Q 100 82 116 69')
    expect(pose(c, [clip('smile')], 500, 'mouth').attrs.get('d')).toBe('M 84 68 Q 100 90 116 68')
  })

  it('holds the neighbouring keyframe where the artwork has no plain value to return to', () => {
    const look = rig({
      a: one({ target: 'plain', prop: 'fill', values: [null, '#0000ff', null] }),
      b: one({ target: 'hollow', prop: 'stroke', values: [null, '#0000ff'] }),
      c: one({ target: 'themed', prop: 'fill', values: [null, '#0000ff'] }),
      e: one({ target: 'mouth', prop: 'd', values: [null, 'M 0 0 L 9 9'] }),
    })
    look.nodes.push({ tag: 'g', attrs: { fill: '#00ff00', strokeWidth: 3 }, children: [{ tag: 'circle', id: 'plain' }] }, { tag: 'circle', id: 'hollow' }, { tag: 'circle', id: 'themed', attrs: { fill: 'currentColor' } })
    look.animations.w = one({ target: 'plain', prop: 'strokeWidth', values: [null, 9] })
    const c = compiled(look)
    // Inherited from the group around it: that is the value null stands for, and nothing is written back.
    expect(pose(c, [clip('a')], 250, 'plain').attrs.get('fill')).toBe('rgb(0,128,128)')
    expect(pose(c, [], 0, 'plain').attrs.get('fill')).toBeNull()
    expect(pose(c, [clip('w')], 500, 'plain').attrs.get('stroke-width')).toBe('6')
    // No stroke at all: it comes out of nothing.
    expect(pose(c, [clip('b')], 500, 'hollow').attrs.get('stroke')).toBe('rgba(0,0,255,0.5)')
    expect(pose(c, [], 0, 'hollow').attrs.get('stroke')).toBeNull()
    // The theme's colour is not known here: the keyframe beside it is held, and the word is put back afterwards.
    expect(pose(c, [clip('c')], 100, 'themed').attrs.get('fill')).toBe('rgb(0,0,255)')
    expect(pose(c, [], 0, 'themed').attrs.get('fill')).toBe('currentColor')
    // A path of another structure than the artwork's: no morph from it, only to it.
    expect(pose(c, [clip('e')], 300, 'mouth').attrs.get('d')).toBe('M 0 0 L 9 9')
    expect(pose(c, [], 0, 'mouth').attrs.get('d')).toBe('M 84 70 Q 100 74 116 70')
  })

  it('interpolates colours per channel, alpha included, and path data by its numbers but never an arc flag', () => {
    const c = compiled(rig({
      tint: one({ prop: 'fill', values: ['#ff000000', '#0000ffff'] }),
      arc: one({ target: 'mouth', prop: 'd', values: ['M 0 0 A 5 5 0 1 0 10 10', 'M 2 2 A 9 9 90 1 0 20 20'] }),
    }))
    expect(pose(c, [clip('tint')], 250).attrs.get('fill')).toBe('rgba(191,0,64,0.25)')
    expect(pose(c, [clip('tint')], 1000).attrs.get('fill')).toBe('rgb(0,0,255)')
    expect(pose(c, [clip('arc')], 500, 'mouth').attrs.get('d')).toBe('M 1 1 A 7 7 45 1 0 15 15')
  })

  it('applies one track to every target of its list', () => {
    const c = compiled(rig({ both: one({ target: ['dot', 'oval', 'dot'], prop: 'translateY', values: [0, 8] }) }))
    const poses = sample(c, [clip('both')], 500, null, null)
    expect(poses.get('dot')?.transform).toBe('translate(0 4)')
    expect(poses.get('oval')?.transform).toBe('translate(0 4)')
  })
})

describe('sample: pivots and the part’s own transform', () => {
  it('turns and scales about the pivot, the centre of the box, or for the body its lower edge', () => {
    const turn = (target: string): Animation => ({ duration: 1, tracks: [{ target, prop: 'rotate', values: [30, 30] }, { target, prop: 'scaleX', values: [2, 2] }] })
    const look = rig({ head: turn('head'), dot: turn('dot'), body: turn('body'), loose: turn('loose') }, { roles: { body: 'body' } })
    const c = compiled(look)
    // Its own transform comes first and the animated one acts inside it.
    expect(pose(c, [clip('head')], 0, 'head').transform).toBe('translate(0 -2) rotate(30 100 90) translate(100 90) scale(2 1) translate(-100 -90)')
    expect(pose(c, [clip('dot')], 0).transform).toBe('rotate(30 100 120) translate(100 120) scale(2 1) translate(-100 -120)')
    expect(pose(c, [clip('body')], 0, 'body').transform).toBe('rotate(30 100 180) translate(100 180) scale(2 1) translate(-100 -180)')
    // Neither a pivot nor a box: the middle of the viewBox.
    expect(pose(c, [clip('loose')], 0, 'loose').transform).toBe('rotate(30 100 100) translate(100 100) scale(2 1) translate(-100 -100)')
  })

  it('leaves out what is the identity, and puts the part’s own transform back when nothing moves it', () => {
    const c = compiled(rig({ still: { duration: 1, tracks: [{ target: 'head', prop: 'translateX', values: [0, 0] }, { target: 'head', prop: 'scale', values: [1, 1] }, { target: 'dot', prop: 'rotate', values: [0, 0] }] } }))
    expect(pose(c, [clip('still')], 300, 'head').transform).toBe('translate(0 -2)')
    expect(pose(c, [clip('still')], 300, 'dot').transform).toBeUndefined()
    expect(pose(c, [], 0, 'head').transform).toBe('translate(0 -2)')
  })

  it('writes the rest pose for every part it may move, so a part returns to the artwork', () => {
    const c = compiled(rig({ fade: one({ prop: 'opacity', values: [1, 0] }), grow: one({ prop: 'r', values: [6, 9] }), wide: one({ target: 'oval', prop: 'strokeWidth', values: [1, 4] }) }))
    const rest = sample(c, [], 0, null, null)
    expect([...rest.keys()].sort()).toEqual(['dot', 'oval'])
    expect(rest.get('dot')?.attrs).toEqual(new Map<string, string | null>([['opacity', '0.5'], ['r', '6']]))
    // The artwork sets no stroke width: the attribute is removed again.
    expect(rest.get('oval')?.attrs).toEqual(new Map<string, string | null>([['stroke-width', null]]))
    expect(rest.get('dot')?.transform).toBeUndefined()
  })
})

describe('sample: several animations at once', () => {
  const steady = (prop: Track['prop'], value: number): Animation => one({ prop, values: [value, value] }, { loop: true })

  it('adds translations and rotations and multiplies scales', () => {
    const c = compiled(rig({ a: steady('translateX', 10), b: steady('translateX', 4), c: steady('rotate', 30), d: steady('rotate', 15), e: steady('scale', 2), f: steady('scaleY', 1.5), g: steady('translateY', -3) }))
    const all = ['a', 'b', 'c', 'd', 'e', 'f', 'g'].map((name) => clip(name))
    expect(moved(c, all, 100)).toEqual({ tx: 14, ty: -3, rot: 45, sx: 2, sy: 3, pivot: [100, 120] })
  })

  it('weighs each: t*w, r*w and 1 + (s-1)*w', () => {
    const c = compiled(rig({ a: steady('translateX', 10), c: steady('rotate', 30), e: steady('scale', 2), f: steady('scaleY', 0.5) }))
    const half = [clip('a', { weight: 0.5 }), clip('c', { weight: 0.25 }), clip('e', { weight: 0.5 }), clip('f', { weight: 0.5 })]
    expect(moved(c, half, 100)).toMatchObject({ tx: 5, rot: 7.5, sx: 1.5, sy: 1.125 })
    // No weight: nothing. More than one: one. Not a number: ignored.
    expect(pose(c, [clip('a', { weight: 0 })], 100).transform).toBeUndefined()
    expect(moved(c, [clip('a', { weight: 7 })], 100).tx).toBe(10)
    expect(pose(c, [clip('a', { weight: Number.NaN })], 100).transform).toBeUndefined()
  })

  it('lets the one started last win every other prop, and fades by weight', () => {
    const c = compiled(rig({ dim: steady('opacity', 0.2), bright: steady('opacity', 0.8) }))
    expect(pose(c, [clip('dim'), clip('bright', { startMs: 50 })], 100).attrs.get('opacity')).toBe('0.8')
    expect(pose(c, [clip('dim', { startMs: 50 }), clip('bright')], 100).attrs.get('opacity')).toBe('0.2')
    // In the order given when they started together.
    expect(pose(c, [clip('dim'), clip('bright')], 100).attrs.get('opacity')).toBe('0.8')
    // Fading in over the other one, and over the artwork's own 0.5.
    expect(pose(c, [clip('dim'), clip('bright', { startMs: 50, weight: 0.5 })], 100).attrs.get('opacity')).toBe('0.5')
    expect(pose(c, [clip('bright', { weight: 0.5 })], 100).attrs.get('opacity')).toBe('0.65')
  })

  it('lets a gesture’s value take the place of the state’s, blended in and out over 150 ms', () => {
    const c = compiled(rig({ sway: steady('rotate', 10), drift: steady('translateY', 4), bow: one({ prop: 'rotate', values: [40, 40] }) }))
    const state = [clip('sway'), clip('drift')]
    const at = (ms: number) => moved(c, [...state, clip('bow', { mode: 'once', startMs: 1000 })], ms)
    expect(at(999).rot).toBe(10)
    expect(at(1000 + GESTURE_BLEND_MS / 2).rot).toBe(25)
    expect(at(1000 + GESTURE_BLEND_MS).rot).toBe(40)
    expect(at(1900).rot).toBe(40)
    // It has played: it gives way again.
    expect(at(2000 + GESTURE_BLEND_MS / 2).rot).toBe(25)
    expect(at(2000 + GESTURE_BLEND_MS).rot).toBe(10)
    // Only the prop it animates: the state's drift goes on underneath.
    expect(at(1500).ty).toBe(4)
  })

  it('replaces both scale factors with a gesture’s scale, and only one with its scaleY', () => {
    const c = compiled(rig({ puff: steady('scale', 2), squash: one({ prop: 'scaleY', values: [0.5, 0.5] }), shrink: one({ prop: 'scale', values: [0.5, 0.5] }) }))
    expect(moved(c, [clip('puff'), clip('squash', { mode: 'hold' })], 500)).toMatchObject({ sx: 2, sy: 0.5 })
    expect(moved(c, [clip('puff'), clip('shrink', { mode: 'hold' })], 500)).toMatchObject({ sx: 0.5, sy: 0.5 })
  })

  it('plays a gesture once, several times, held or looping', () => {
    const c = compiled(rig({ lift: one({ prop: 'translateY', values: [0, -20] }), round: one({ prop: 'translateY', values: [0, -20] }, { loop: true }) }))
    const at = (mode: Clip['mode'], ms: number, times = 1) => moved(c, [clip('lift', { mode, times })], ms).ty
    expect(at('once', 500)).toBe(-10)
    expect(at('once', 1000)).toBe(-20)
    expect(at('once', 1000 + GESTURE_BLEND_MS)).toBe(0)
    expect(at('once', 1500, 3)).toBe(-10)
    expect(at('once', 2750, 3)).toBe(-15)
    expect(at('once', 3000 + GESTURE_BLEND_MS, 3)).toBe(0)
    expect(at('once', 2500, 99)).toBe(-10)
    expect(at('once', 5000 + GESTURE_BLEND_MS, 99)).toBe(0)
    // Held: the last frame stays.
    expect(at('hold', 5000)).toBe(-20)
    expect(at('hold', 500)).toBe(-10)
    expect(moved(c, [clip('round', { mode: 'loop' })], 5250).ty).toBe(-5)
    // Being stopped: its weight takes it out.
    expect(moved(c, [clip('lift', { mode: 'hold', weight: 0.5 })], 5000).ty).toBe(-10)
  })

  it('lets a one-shot of a state list fade its leftover out over 200 ms, relative props only', () => {
    const c = compiled(rig({ lean: { duration: 1, tracks: [{ target: 'dot', prop: 'translateX', values: [0, 10] }, { target: 'dot', prop: 'opacity', values: [0.5, 1] }] } }))
    const at = (ms: number) => pose(c, [clip('lean')], ms)
    expect(readTransform(at(1000).transform).tx).toBe(10)
    expect(at(1000).attrs.get('opacity')).toBe('1')
    expect(readTransform(at(1000 + STATE_FADE_MS / 2).transform).tx).toBe(5)
    // Every other prop is the artwork's again at once.
    expect(at(1000 + STATE_FADE_MS / 2).attrs.get('opacity')).toBe('0.5')
    expect(at(1000 + STATE_FADE_MS).transform).toBeUndefined()
  })

  it('plays one pass of a recurring animation from its start', () => {
    const c = compiled(rig({ wink: one({ prop: 'scaleY', values: [1, 0.1, 1] }, { duration: 0.2, every: [2, 4] }) }))
    expect(moved(c, [clip('wink', { startMs: 3000 })], 2900).sy).toBe(1)
    expect(moved(c, [clip('wink', { startMs: 3000 })], 3100).sy).toBe(0.1)
    expect(moved(c, [clip('wink', { startMs: 3000 })], 3600).sy).toBe(1)
  })

  it('ignores a clip whose name means nothing, and anything that is not a clip', () => {
    const c = compiled(rig({ a: steady('translateX', 10) }))
    const odd = [clip('nope'), clip('constructor'), clip('__proto__'), clip('toString'), clip('hasOwnProperty'), clip('a', { startMs: Number.NaN }), clip('a', { startMs: Infinity }), null, undefined, 7, 'a', { name: 7 }] as unknown as Clip[]
    expect(pose(c, odd, 100).transform).toBeUndefined()
    expect(moved(c, [...odd, clip('a')], 100).tx).toBe(10)
    expect(sample(c, 'a' as unknown as Clip[], 100, null, null).get('dot')?.transform).toBeUndefined()
    // A moment that is not a number, or a look that was never compiled: nothing at all.
    expect(sample(c, [clip('a')], Number.NaN, null, null).size).toBe(0)
    expect(sample(c, [clip('a')], Infinity, null, null).size).toBe(0)
    expect(sample({ look: rig() }, [clip('a')], 0, null, null).size).toBe(0)
  })
})

describe('sample: mouth and gaze', () => {
  it('morphs a path mouth between closed and open', () => {
    const c = compiled(rig({}, { mouth: [{ target: 'mouth', open: 'M 84 66 Q 100 94 116 66' }] }))
    const d = (open: number | null) => pose(c, [], 0, 'mouth', open).attrs.get('d')
    // Not applied: the artwork's own shape.
    expect(d(null)).toBe('M 84 70 Q 100 74 116 70')
    expect(d(0)).toBe('M 84 70 Q 100 74 116 70')
    expect(d(0.5)).toBe('M 84 68 Q 100 84 116 68')
    expect(d(1)).toBe('M 84 66 Q 100 94 116 66')
    expect(d(7)).toBe('M 84 66 Q 100 94 116 66')
    expect(d(-3)).toBe('M 84 70 Q 100 74 116 70')
    expect(d(Number.NaN)).toBe('M 84 70 Q 100 74 116 70')
    const shut = compiled(rig({}, { mouth: [{ target: 'mouth', open: 'M 84 66 Q 100 94 116 66', closed: 'M 84 72 Q 100 72 116 72' }] }))
    expect(pose(shut, [], 0, 'mouth', 0).attrs.get('d')).toBe('M 84 72 Q 100 72 116 72')
    expect(pose(shut, [], 0, 'mouth', null).attrs.get('d')).toBe('M 84 70 Q 100 74 116 70')
  })

  it('drives a number: a scale or a shift in place of what tracks say, or an attribute', () => {
    const look = rig(
      { wobble: one({ target: 'oval', prop: 'scaleY', values: [3, 3] }, { loop: true }), stretch: one({ target: 'oval', prop: 'scaleX', values: [2, 2] }, { loop: true }) },
      { mouth: [{ target: 'oval', prop: 'scaleY', open: 4 }, { target: 'oval', prop: 'ry', open: 10 }, { target: 'dot', prop: 'translateY', open: 6, closed: 1 }, { target: 'dot', prop: 'r', open: 12, closed: 4 }] },
    )
    const c = compiled(look)
    const clips = [clip('wobble'), clip('stretch')]
    // Not presenting: the tracks and the artwork apply.
    expect(readTransform(pose(c, clips, 0, 'oval').transform)).toMatchObject({ sx: 2, sy: 3 })
    expect(pose(c, clips, 0, 'oval').attrs.get('ry')).toBe('2')
    // Presenting: the mouth has the last word on its own prop; closed is 1 for a scale and the artwork's value for an attribute.
    expect(readTransform(pose(c, clips, 0, 'oval', 0).transform)).toMatchObject({ sx: 2, sy: 1 })
    expect(readTransform(pose(c, clips, 0, 'oval', 0.5).transform)).toMatchObject({ sx: 2, sy: 2.5 })
    expect(pose(c, clips, 0, 'oval', 0.5).attrs.get('ry')).toBe('6')
    expect(readTransform(pose(c, clips, 0, 'dot', 0.5).transform).ty).toBe(3.5)
    expect(pose(c, clips, 0, 'dot', 0).attrs.get('r')).toBe('4')
    expect(pose(c, clips, 0, 'dot', 1).attrs.get('r')).toBe('12')
    expect(pose(c, clips, 0, 'dot', null).attrs.get('r')).toBe('6')
  })

  it('eases the mouth towards its target by the time that passed', () => {
    expect(easeMouth(0, 1, 60)).toBeCloseTo(1 - Math.exp(-1), 9)
    expect(easeMouth(0, 1, 120)).toBeCloseTo(1 - Math.exp(-2), 9)
    expect(easeMouth(1, 0, 60)).toBeCloseTo(Math.exp(-1), 9)
    expect(easeMouth(0.4, 0.4, 500)).toBe(0.4)
    // Two frames of 30 ms are one of 60: the frame rate does not change the motion.
    expect(easeMouth(easeMouth(0, 1, 30), 1, 30)).toBeCloseTo(easeMouth(0, 1, 60), 9)
    expect(easeMouth(0.3, 1, 0)).toBe(0.3)
    expect(easeMouth(0.3, 1, -5)).toBe(0.3)
    expect(easeMouth(0.3, 1, Number.NaN)).toBe(0.3)
    expect(easeMouth(Number.NaN, 1, 16)).toBe(0)
    expect(easeMouth(0.3, Infinity, 16)).toBe(0)
  })

  it('moves the pupils by at most the look’s travel', () => {
    const look = (gaze: number | null) => rig({ drift: one({ target: 'pupils', prop: 'translateX', values: [1, 1] }, { loop: true }) }, { roles: { pupils: 'pupils' }, gaze })
    const c = compiled(look(null))
    const at = (v: readonly [number, number] | null, clips: Clip[] = []) => readTransform(pose(c, clips, 0, 'pupils', null, v).transform)
    expect(at(null)).toMatchObject({ tx: 0, ty: 0 })
    // 3% of the viewBox width.
    expect(at([1, 0])).toMatchObject({ tx: 6, ty: 0 })
    expect(at([0, -0.5])).toMatchObject({ tx: 0, ty: -3 })
    // Longer than 1 is 1.
    expect(at([3, 4])).toMatchObject({ tx: 3.6, ty: 4.8 })
    expect(at([Number.NaN, 0])).toMatchObject({ tx: 0, ty: 0 })
    // Added like a translate track.
    expect(at([1, 0], [clip('drift')])).toMatchObject({ tx: 7 })
    expect(readTransform(pose(compiled(look(2)), [], 0, 'pupils', null, [1, 0]).transform).tx).toBe(2)
    // At most 5% of the width; 0 switches it off.
    expect(readTransform(pose(compiled(look(50)), [], 0, 'pupils', null, [1, 0]).transform).tx).toBe(10)
    const off = compiled(look(0))
    expect(pose(off, [], 0, 'pupils', null, [1, 0]).transform).toBeUndefined()
    // No pupils role: nothing to move.
    expect(sample(compiled(rig()), [], 0, null, [1, 0]).size).toBe(0)
  })
})

describe('sample: reduced motion', () => {
  it('plays no loop, no recurring animation and no gaze; shows the last frame of a gesture; still moves the mouth', () => {
    const look = rig(
      {
        float: one({ prop: 'translateY', values: [0, -6, 0] }, { loop: true }),
        wink: one({ prop: 'scaleY', values: [1, 0.1, 1] }, { duration: 0.2, every: [2, 4] }),
        point: one({ prop: 'rotate', values: [0, 80, 60] }),
        settle: one({ prop: 'translateX', values: [0, 9, 3] }),
      },
      { roles: { pupils: 'pupils' }, mouth: [{ target: 'mouth', open: 'M 84 66 Q 100 94 116 66' }] },
    )
    const c = compiled(look)
    const calm = (clips: Clip[], ms: number, id = 'dot') => pose(c, clips, ms, id, null, [1, 0], true)
    expect(calm([clip('float'), clip('wink', { startMs: 0 })], 100).transform).toBeUndefined()
    expect(calm([], 0, 'pupils').transform).toBeUndefined()
    // No sweep, no blend: the pose it ends in, for as long as it would have played.
    expect(readTransform(calm([clip('point', { mode: 'once' })], 10).transform).rot).toBe(60)
    expect(readTransform(calm([clip('point', { mode: 'once' })], 900).transform).rot).toBe(60)
    expect(calm([clip('point', { mode: 'once' })], 1001).transform).toBeUndefined()
    expect(readTransform(calm([clip('point', { mode: 'hold' })], 99999).transform).rot).toBe(60)
    expect(readTransform(calm([clip('settle')], 10).transform).tx).toBe(3)
    expect(calm([clip('settle')], 1001).transform).toBeUndefined()
    expect(pose(c, [], 0, 'mouth', 1, null, true).attrs.get('d')).toBe('M 84 66 Q 100 94 116 66')
  })
})

describe('sample: what it writes', () => {
  it('is always a string it formatted from finite numbers', () => {
    const look = checkLook(wire(sampleLook())) as Look
    const c = compiled(look)
    const names = ['hop', 'blush', 'grin', 'peek', 'breathe', 'blink', 'think', 'talk', 'listen', 'alert', 'doze', 'wave', 'cheer', 'nod', 'shake']
    let poses = 0
    for (let t = 0; t <= 6000; t += 37) {
      const clips: Clip[] = names.map((name, i) => clip(name, { startMs: i * 90, weight: ((t + i * 13) % 100) / 100, mode: (['state', 'once', 'hold', 'loop'] as const)[(i + Math.floor(t / 500)) % 4], times: 1 + (i % 5) }))
      const got = sample(c, clips, t, (t % 700) / 700, [Math.sin(t), Math.cos(t)])
      expectFormatted(got)
      poses += got.size
    }
    expect(poses).toBeGreaterThan(1000)
    expectFormatted(sample(compiled(defaultLook('t1')), [clip('glow'), clip('sway'), clip('cheer', { mode: 'once' })], 300, 0.7, [1, 1]))
  })

  it('leaves a part out rather than write a number that is not finite', () => {
    const c = compiled(rig({ far: one({ prop: 'translateX', values: [0, 9e5] }, { loop: true }), near: one({ target: 'oval', prop: 'translateX', values: [0, 5] }, { loop: true }) }))
    // Twenty clips of the same far travel, each at the top of the range, at a weight no clip should have.
    const clips = Array.from({ length: 20 }, () => clip('far', { weight: 1 }))
    const got = sample(c, [...clips, clip('near')], 999.999, null, null)
    expectFormatted(got)
    expect(got.get('oval')?.transform).toMatch(/^translate\(/)
    const huge = compiled(rig({ a: one({ prop: 'scale', values: [10, 10] }, { loop: true }) }))
    // Scales multiply: enough of them and the product is no number any more. The part is left out; nothing throws.
    const many = sample(huge, Array.from({ length: 400 }, () => clip('a')), 0, null, null)
    expect(many.has('dot')).toBe(false)
  })
})

describe('createEngine', () => {
  const steady = (target: string, prop: Track['prop'], value: number): Animation => ({ duration: 1, loop: true, tracks: [{ target, prop, values: [value, value] }] })
  const travel = (target = 'dot'): Animation => ({ duration: 10, loop: true, tracks: [{ target, prop: 'translateX', values: [0, 100] }] })

  /** A look whose idle state runs `travel`: 10 units a second, so a part's x tells the engine's time. */
  const clockLook = (extra: Record<string, Animation> = {}, states: Look['states'] = {}) => rig({ travel: travel(), ...extra }, { states: { idle: ['travel'], ...states } })
  const xOf = (f: ReturnType<typeof fakeFrames>, id = 'dot', own = '') => readTransform(f.nodes.get(id)?.attrs.get('transform'), own).tx

  it('asks for frames only while a figure is attached, and puts the artwork back on detach', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    expect(f.waiting()).toBe(0)
    const look = rig({ travel: travel(), dim: steady('dot', 'opacity', 0.1), turn: steady('head', 'rotate', 20) }, { states: { idle: ['travel', 'dim', 'turn'] } })
    const handle = engine.attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: false })
    expect(f.waiting()).toBe(1)
    f.run(1000)
    expect(f.waiting()).toBe(1)
    expect(f.find('dot').attrs.get('opacity')).toBe('0.1')
    expect(f.find('head').attrs.get('transform')).toBe('translate(0 -2) rotate(20 100 90)')
    expect(xOf(f)).toBeGreaterThan(5)
    handle.detach()
    // The artwork's own values: its opacity, its own transform, no transform where it has none.
    expect(f.find('dot').attrs.get('opacity')).toBe('0.5')
    expect(f.find('dot').attrs.has('transform')).toBe(false)
    expect(f.find('head').attrs.get('transform')).toBe('translate(0 -2)')
    expect(f.waiting()).toBe(0)
    const writes = f.find('dot').writes.length
    f.run(500)
    handle.detach()
    handle.setState('working')
    expect(handle.play('travel')).toBe(false)
    expect(f.find('dot').writes.length).toBe(writes)
  })

  it('writes nothing to a part that is at rest', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    engine.attach(rig({ travel: travel() }), (id) => f.find(id), { state: 'idle', reducedMotion: false })
    f.run(2000)
    expect(f.find('dot').writes).toEqual([])
  })

  it('samples at most 30 times a second', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    engine.attach(clockLook(), (id) => f.find(id), { state: 'idle', reducedMotion: false })
    // A 120 Hz display for one second.
    for (let i = 0; i < 120; i++) f.frame(1000 / 120)
    const writes = f.find('dot').writes.length
    expect(writes).toBeGreaterThan(20)
    expect(writes).toBeLessThanOrEqual(30)
    // Time is not lost with the skipped frames.
    expect(xOf(f)).toBeGreaterThan(9.5)
    expect(xOf(f)).toBeLessThanOrEqual(10)
  })

  it('counts visible time only', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    engine.attach(clockLook(), (id) => f.find(id), { state: 'idle', reducedMotion: false })
    f.run(1000)
    const before = xOf(f)
    expect(before).toBeCloseTo(10, 0)
    engine.setRunning(false)
    expect(f.waiting()).toBe(0)
    // The tab is away for ten minutes; no frame comes, and when it is back the motion goes on from where it was.
    f.frame(600_000)
    engine.setRunning(true)
    expect(f.waiting()).toBe(1)
    f.frame(40)
    expect(xOf(f)).toBe(before)
    f.frame(40)
    expect(xOf(f)).toBeCloseTo(before + 0.4, 5)
    engine.setRunning(true)
    expect(f.waiting()).toBe(1)
  })

  it('does not fast-forward when time jumps by an hour', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    engine.attach(clockLook(), (id) => f.find(id), { state: 'idle', reducedMotion: false })
    f.run(1000)
    const before = xOf(f)
    f.frame(3_600_000)
    // One long frame counts for a quarter of a second, no more.
    expect(xOf(f)).toBeCloseTo(before + 2.5, 5)
    f.frame(-5000)
    f.frame(40)
    expect(xOf(f)).toBeCloseTo(before + 2.9, 5)
    for (const odd of [Number.NaN, Infinity]) {
      const due = f.waiting()
      expect(due).toBe(1)
      f.frame(odd)
    }
    expect(f.waiting()).toBe(1)
  })

  it('keeps the other figures going when one figure’s node throws, and leaves that one static', () => {
    const f = fakeFrames()
    const g = fakeFrames()
    const engine = createEngine(f.host)
    const good = engine.attach(clockLook(), (id) => f.find(id), { state: 'idle', reducedMotion: false })
    const bad = engine.attach(clockLook(), (id) => g.find(id), { state: 'idle', reducedMotion: false })
    f.run(400)
    expect(readTransform(g.nodes.get('dot')?.attrs.get('transform')).tx).toBeGreaterThan(0)
    g.find('dot').broken = true
    f.run(400)
    expect(bad.frozen()).toBe(true)
    expect(good.frozen()).toBe(false)
    const stuck = g.find('dot').writes.length
    const moving = xOf(f)
    g.find('dot').broken = false
    f.run(400)
    expect(g.find('dot').writes.length).toBe(stuck)
    expect(xOf(f)).toBeGreaterThan(moving)
    expect(f.waiting()).toBe(1)
    expect(bad.play('travel')).toBe(false)
    bad.detach()
    good.detach()
    expect(f.waiting()).toBe(0)
  })

  it('survives a reader that throws', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const look = rig({ travel: travel() }, { states: { idle: ['travel'] }, roles: { pupils: 'pupils' }, mouth: [{ target: 'mouth', open: 'M 84 66 Q 100 94 116 66' }] })
    const handle = engine.attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: false })
    handle.setMouth(() => {
      throw new Error('no speaker')
    })
    handle.setGaze(() => {
      throw new Error('no pointer')
    })
    f.run(400)
    expect(handle.frozen()).toBe(false)
    expect(xOf(f)).toBeGreaterThan(0)
  })

  it('freezes a figure whose frames cost over 4 ms on average, in its rest pose', () => {
    const f = fakeFrames()
    const g = fakeFrames()
    const engine = createEngine(f.host)
    const light = engine.attach(clockLook(), (id) => f.find(id), { state: 'idle', reducedMotion: false })
    const heavy = engine.attach(clockLook(), (id) => g.find(id), { state: 'idle', reducedMotion: false })
    f.find('dot').onWrite = () => f.spend(3)
    g.find('dot').onWrite = () => f.spend(5)
    for (let i = 0; i < BUDGET_FRAMES - 1; i++) f.frame(40)
    expect(heavy.frozen()).toBe(false)
    f.frame(40)
    f.frame(40)
    expect(heavy.frozen()).toBe(true)
    expect(light.frozen()).toBe(false)
    expect(g.find('dot').attrs.has('transform')).toBe(false)
    const frozenAt = g.find('dot').writes.length
    f.run(2000)
    expect(g.find('dot').writes.length).toBe(frozenAt)
    expect(light.frozen()).toBe(false)
    expect(xOf(f)).toBeGreaterThan(20)
    heavy.setState('working')
    expect(heavy.play('travel')).toBe(false)
  })

  it('keeps an animation of both states running untouched, fades the others over 200 ms', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const look = clockLook({ up: steady('oval', 'translateY', -10), turn: steady('head', 'rotate', 20) }, { idle: ['travel', 'up'], working: ['travel', 'turn'] })
    const handle = engine.attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: false })
    const y = () => readTransform(f.nodes.get('oval')?.attrs.get('transform')).ty
    const rot = () => readTransform(f.nodes.get('head')?.attrs.get('transform'), 'translate(0 -2)').rot
    f.run(1000)
    expect(y()).toBe(-10)
    expect(rot()).toBe(0)
    const before = xOf(f)
    handle.setState('working')
    f.frame(40)
    // Same instance, same phase: it only went on by one frame.
    expect(xOf(f)).toBeCloseTo(before + 0.4, 5)
    f.frame(60)
    expect(y()).toBeCloseTo(-5, 5)
    expect(rot()).toBeCloseTo(10, 5)
    f.run(200)
    expect(f.find('oval').attrs.has('transform')).toBe(false)
    expect(rot()).toBe(20)
    // Back again before the fade is over: it turns round where it is.
    handle.setState('idle')
    f.frame(100)
    handle.setState('working')
    f.frame(40)
    // Half-way out after 100 ms; the way back to full takes 200 ms again.
    expect(rot()).toBeCloseTo(12, 5)
    handle.setState('working')
    f.run(400)
    expect(rot()).toBe(20)
    // And all the while the shared one went on at its own pace: 840 ms more.
    expect(xOf(f)).toBeCloseTo(before + 8.4, 5)
  })

  it('starts a recurring animation after a random wait and keeps its schedule across states', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const wink: Animation = { duration: 0.2, every: [2, 4], tracks: [{ target: 'oval', prop: 'scaleY', values: [1, 0.1, 1] }] }
    const look = rig({ wink }, { states: { idle: ['wink'], working: ['wink'] } })
    const handle = engine.attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: false })
    const starts: number[] = []
    let down = false
    const watch = (ms: number) => {
      for (let t = 0; t < ms; t += 40) {
        f.frame(40)
        const sy = readTransform(f.nodes.get('oval')?.attrs.get('transform')).sy
        if (sy < 0.9 && !down) starts.push(f.time())
        down = sy < 0.9
      }
    }
    // Half of [0, 4 s] first, then half-way between 2 and 4 s from start to start.
    watch(1900)
    expect(starts).toEqual([])
    watch(300)
    expect(starts).toHaveLength(1)
    handle.setState('working')
    watch(6000)
    expect(starts).toHaveLength(3)
    expect(starts[1] - starts[0]).toBeGreaterThanOrEqual(2960)
    expect(starts[1] - starts[0]).toBeLessThanOrEqual(3040)
    expect(starts[2] - starts[1]).toBeGreaterThanOrEqual(2960)
    expect(starts[2] - starts[1]).toBeLessThanOrEqual(3040)
  })

  it('plays a one-shot of a state list each time the state is entered, but not when the page loads into it', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const jump: Animation = { duration: 0.4, tracks: [{ target: 'oval', prop: 'translateY', values: [0, -20, 0] }] }
    const look = rig({ jump, travel: travel() }, { states: { done: ['travel', 'jump'], idle: ['travel'], error: ['travel', 'jump'] } })
    const handle = engine.attach(look, (id) => f.find(id), { state: 'done', reducedMotion: false })
    const peak = (ms: number) => {
      let top = 0
      for (let t = 0; t < ms; t += 40) {
        f.frame(40)
        top = Math.min(top, readTransform(f.nodes.get('oval')?.attrs.get('transform')).ty)
      }
      return top
    }
    expect(peak(1000)).toBe(0)
    handle.setState('idle')
    expect(peak(400)).toBe(0)
    handle.setState('done')
    expect(peak(1000)).toBeLessThan(-15)
    // Both states list it: it is the same instance, and it has played.
    handle.setState('error')
    expect(peak(1000)).toBe(0)
    handle.setState('idle')
    f.run(400)
    handle.setState('error')
    expect(peak(1000)).toBeLessThan(-15)
  })

  it('plays gestures by name and refuses names that mean nothing', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const look = rig({ lift: { duration: 0.4, tracks: [{ target: 'oval', prop: 'translateY', values: [0, -20] }] }, round: { duration: 0.4, loop: true, tracks: [{ target: 'oval', prop: 'translateX', values: [0, 8, 0] }] } })
    const handle = engine.attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: false })
    const at = () => readTransform(f.nodes.get('oval')?.attrs.get('transform'))
    for (const name of ['nope', 'constructor', '__proto__', 'toString', 'hasOwnProperty', '', 7 as unknown as string]) expect(handle.play(name), String(name)).toBe(false)
    expect(handle.play('lift')).toBe(true)
    f.run(400)
    expect(at().ty).toBeLessThan(-15)
    f.run(400)
    expect(f.find('oval').attrs.has('transform')).toBe(false)
    // Held: the last frame stays until it is stopped, then gives way over 150 ms.
    expect(handle.play('lift', 'hold')).toBe(true)
    f.run(2000)
    expect(at().ty).toBe(-20)
    handle.stop('lift')
    f.frame(80)
    expect(at().ty).toBeLessThan(0)
    expect(at().ty).toBeGreaterThan(-20)
    f.run(200)
    expect(f.find('oval').attrs.has('transform')).toBe(false)
    // A held gesture ends at the next play.
    handle.play('lift', 'hold')
    f.run(1000)
    handle.play('round', 'loop')
    f.run(1000)
    expect(at().ty).toBe(0)
    f.run(100)
    handle.stop('all')
    f.run(400)
    expect(f.find('oval').attrs.has('transform')).toBe(false)
    // Only an animation declared as a loop loops: this one plays once.
    handle.play('lift', 'loop')
    f.run(1200)
    expect(f.find('oval').attrs.has('transform')).toBe(false)
    handle.play('lift', 'once', 99)
    f.run(1920)
    expect(at().ty).toBeLessThan(0)
    f.run(400)
    expect(f.find('oval').attrs.has('transform')).toBe(false)
    handle.stop('nope')
    handle.stop('constructor')
  })

  it('applies the mouth only while a line is presented and for a moment after it', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const look = rig({}, { mouth: [{ target: 'oval', prop: 'ry', open: 12, closed: 2 }] })
    const handle = engine.attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: false })
    let want = -1
    handle.setMouth(() => want)
    const ry = () => Number(f.find('oval').attrs.get('ry'))
    f.run(400)
    expect(f.find('oval').writes).toEqual([])
    want = 1
    f.frame(40)
    f.frame(40)
    // It opens towards its target, it does not jump there.
    expect(ry()).toBeGreaterThan(2)
    expect(ry()).toBeLessThan(12)
    f.run(600)
    expect(ry()).toBeCloseTo(12, 1)
    want = 0
    f.run(600)
    expect(ry()).toBeCloseTo(2, 1)
    // The line is over: closed for a moment more, then the artwork's own value again.
    want = 1
    f.run(400)
    want = -1
    f.frame(40)
    expect(ry()).toBeLessThan(12)
    expect(ry()).toBeGreaterThan(2)
    f.run(MOUTH_TAIL_MS + 80)
    expect(f.find('oval').attrs.get('ry')).toBe('2')
    handle.setMouth(null)
    want = 1
    f.run(200)
    expect(f.find('oval').attrs.get('ry')).toBe('2')
  })

  it('turns the pupils where it is told, and lets them glance about when it is told nothing', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const look = rig({}, { roles: { pupils: 'pupils' } })
    const handle = engine.attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: false })
    const at = () => readTransform(f.nodes.get('pupils')?.attrs.get('transform'))
    let told: readonly [number, number] | null = [1, 0]
    handle.setGaze(() => told)
    f.frame(40)
    f.frame(40)
    expect(at().tx).toBeGreaterThan(0)
    expect(at().tx).toBeLessThan(6)
    f.run(1000)
    expect(at().tx).toBeCloseTo(6, 1)
    told = [0, 30]
    f.run(1000)
    expect(at()).toMatchObject({ tx: 0 })
    expect(at().ty).toBeCloseTo(6, 1)
    // Nothing to look at: back to the middle, and a glance some seconds later.
    told = null
    f.run(1900)
    expect(f.find('pupils').attrs.has('transform')).toBe(false)
    let glanced = false
    for (let t = 0; t < 4000; t += 40) {
      f.frame(40)
      const p = at()
      glanced ||= Math.hypot(p.tx, p.ty) > 1
      expect(Math.hypot(p.tx, p.ty)).toBeLessThanOrEqual(6)
    }
    expect(glanced).toBe(true)
    handle.setReducedMotion(true)
    f.run(200)
    expect(f.find('pupils').attrs.has('transform')).toBe(false)
  })

  it('reduces motion when asked: loops stop, a gesture shows its last frame', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const look = clockLook({ point: { duration: 0.4, tracks: [{ target: 'oval', prop: 'rotate', values: [0, 80, 60] }] } })
    const handle = engine.attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: true })
    f.run(1000)
    expect(f.find('dot').writes).toEqual([])
    handle.play('point')
    f.frame(40)
    expect(readTransform(f.nodes.get('oval')?.attrs.get('transform')).rot).toBe(60)
    f.run(500)
    expect(f.find('oval').attrs.has('transform')).toBe(false)
    handle.setReducedMotion(false)
    f.run(400)
    expect(xOf(f)).toBeGreaterThan(0)
  })

  it('gives a look that does not compile a handle that does nothing', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const broken = rig({ a: { duration: 1, tracks: [{ target: 'nowhere', prop: 'rotate', values: [0, 1] }] } })
    const handle = engine.attach(broken, (id) => f.find(id), { state: 'idle', reducedMotion: false })
    expect(f.waiting()).toBe(0)
    expect(handle.play('a')).toBe(false)
    handle.setState('working')
    handle.setMouth(() => 1)
    handle.setGaze(() => [1, 0])
    handle.setReducedMotion(true)
    handle.stop('all')
    expect(handle.frozen()).toBe(false)
    handle.detach()
    expect(f.nodes.size).toBe(0)
  })

  it('copes with parts it cannot find and a finder that throws', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const look = clockLook({ up: steady('oval', 'translateY', -10) }, { idle: ['travel', 'up'] })
    const handle = engine.attach(
      look,
      (id) => {
        if (id === 'oval') throw new Error('no such node')
        return id === 'dot' ? f.find(id) : null
      },
      { state: 'idle', reducedMotion: false },
    )
    f.run(400)
    expect(handle.frozen()).toBe(false)
    expect(xOf(f)).toBeGreaterThan(0)
    expect(f.nodes.has('oval')).toBe(false)
  })

  it('stops for good when disposed', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const handle = engine.attach(clockLook(), (id) => f.find(id), { state: 'idle', reducedMotion: false })
    f.run(400)
    engine.dispose()
    expect(f.waiting()).toBe(0)
    expect(f.find('dot').attrs.has('transform')).toBe(false)
    engine.setRunning(true)
    expect(f.waiting()).toBe(0)
    const writes = f.find('dot').writes.length
    handle.setState('working')
    handle.detach()
    engine.dispose()
    expect(engine.attach(clockLook(), (id) => f.find(id), { state: 'idle', reducedMotion: false }).play('travel')).toBe(false)
    expect(f.find('dot').writes.length).toBe(writes)
    expect(f.waiting()).toBe(0)
  })

  it('gives a part painted with a gradient its gradient back after a colour track, and on detach', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const look = painted({
      flash: { duration: 0.6, tracks: [{ target: 'ball', prop: 'fill', values: ['#ff0000', '#0000ff'] }, { target: 'ball', prop: 'translateX', values: [0, 6] }] },
      glint: { duration: 1, loop: true, tracks: [{ target: 'ball', prop: 'stroke', values: ['#ffffff', '#000000', '#ffffff'] }] },
    }, { states: { working: ['glint'] } })
    // React's useId gives ":r1:": the engine cleans the prefix the way the renderer does.
    const prefix = ':r1:-'
    const rendered = renderToStaticMarkup(createElement('svg', null, lookElements(look, prefix)))
    expect(rendered).toContain('fill="url(#r1-sky)"')
    expect(rendered).toContain('stroke="url(#r1-sun)"')
    const handle = engine.attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: false, idPrefix: prefix })
    f.run(400)
    // At rest nothing is written: the gradients are what was rendered.
    expect(f.find('ball').writes).toEqual([])
    expect(handle.play('flash')).toBe(true)
    f.run(320)
    expect(f.find('ball').attrs.get('fill')).toMatch(/^rgb\(\d+,0,\d+\)$/)
    expect(f.find('ball').attrs.has('stroke')).toBe(false)
    f.run(2000)
    // Played: exactly the reference that was rendered.
    expect(f.find('ball').attrs.get('fill')).toBe('url(#r1-sky)')
    expect(f.find('ball').attrs.has('transform')).toBe(false)
    // A state's own loop colours the outline for as long as the state lasts.
    handle.setState('working')
    f.run(600)
    expect(f.find('ball').attrs.get('stroke')).toMatch(/^rgb\(/)
    handle.setState('idle')
    f.run(600)
    expect(f.find('ball').attrs.get('stroke')).toBe('url(#r1-sun)')
    // Detached in the middle of both: both gradients are back.
    handle.setState('working')
    handle.play('flash', 'hold')
    f.run(400)
    expect(f.find('ball').attrs.get('fill')).toBe('rgb(85,0,170)')
    expect(f.find('ball').attrs.get('stroke')).toMatch(/^rgb\(/)
    handle.detach()
    expect(f.find('ball').attrs.get('fill')).toBe('url(#r1-sky)')
    expect(f.find('ball').attrs.get('stroke')).toBe('url(#r1-sun)')
    for (const [name, value] of f.find('ball').writes) if (name === 'fill' || name === 'stroke') expect(value).toMatch(/^(?:rgb\(\d+,\d+,\d+\)|url\(#r1-(?:sky|sun)\))$/)
  })

  it('never touches such a paint when it was not told what the figure was rendered with', () => {
    const f = fakeFrames()
    const g = fakeFrames()
    const engine = createEngine(f.host)
    const look = painted({ flash: { duration: 0.6, tracks: [{ target: ['ball', 'dot'], prop: 'fill', values: ['#ff0000', '#0000ff'] }, { target: 'ball', prop: 'translateX', values: [0, 6] }] } })
    const handle = engine.attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: false })
    // An empty prefix is a prefix: the figure was rendered without one.
    const bare = engine.attach(look, (id) => g.find(id), { state: 'idle', reducedMotion: false, idPrefix: '' })
    handle.play('flash')
    bare.play('flash')
    f.run(320)
    // The rest of the animation plays: the part moves, and a part with a plain colour is coloured.
    expect(readTransform(f.find('ball').attrs.get('transform')).tx).toBeGreaterThan(0)
    expect(f.find('dot').attrs.get('fill')).toMatch(/^rgb\(/)
    expect(g.find('ball').attrs.get('fill')).toMatch(/^rgb\(/)
    f.run(2000)
    handle.detach()
    bare.detach()
    expect(f.find('ball').writes.filter(([name]) => name === 'fill' || name === 'stroke')).toEqual([])
    expect(f.find('dot').attrs.get('fill')).toBe('rgb(255,0,0)')
    expect(g.find('ball').attrs.get('fill')).toBe('url(#sky)')
  })

  it('only ever writes strings it formatted from finite numbers, to attributes of its own table', () => {
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const look = checkLook(wire(sampleLook())) as Look
    look.animations.tint = { duration: 1.2, tracks: [{ target: ['face', 'torso', 'cheek'], prop: 'fill', values: ['#ff0000', '#0000ff', '#00ff00'] }] }
    const handle = engine.attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: false, idPrefix: 'p-' })
    let t = 0
    handle.setMouth(() => (Math.floor(t / 900) % 2 ? Math.abs(Math.sin(t / 90)) : -1))
    handle.setGaze(() => (Math.floor(t / 1500) % 2 ? [Math.cos(t / 300), Math.sin(t / 300)] : null))
    const states = ['idle', 'working', 'speaking', 'listening', 'waiting', 'attention', 'error', 'done', 'sleeping'] as const
    const gestures = ['hop', 'grin', 'blush', 'wave', 'cheer', 'nod', 'shake', 'peek', 'tint']
    for (let i = 0; i < 600; i++) {
      t += 40
      if (i % 23 === 0) handle.setState(states[(i / 23) % states.length])
      if (i % 17 === 0) handle.play(gestures[(i / 17) % gestures.length], (['once', 'hold', 'loop'] as const)[i % 3], 1 + (i % 4))
      if (i % 61 === 0) handle.stop('all')
      f.frame(40)
    }
    const allowed = new Set(['transform', 'opacity', 'fill', 'stroke', 'stroke-width', 'd', 'cx', 'cy', 'r', 'rx', 'ry', 'x', 'y', 'width', 'height', 'x1', 'y1', 'x2', 'y2'])
    let writes = 0
    for (const [id, node] of f.nodes) {
      for (const [name, value] of node.writes) {
        writes++
        expect(allowed.has(name), `${id} ${name}`).toBe(true)
        if (value === null) continue
        expect(typeof value).toBe('string')
        // The one string that is not made of numbers: the gradient a part returns to, under the figure's prefix.
        if ((id === 'face' || id === 'torso') && name === 'fill' && value.startsWith('url')) expect(value).toBe('url(#p-skin)')
        else expect(value, `${id} ${name}`).toMatch(name === 'transform' ? TRANSFORM : ATTR)
      }
    }
    expect(writes).toBeGreaterThan(2000)
    expect(handle.frozen()).toBe(false)
    handle.detach()
    // Everything is the artwork's again.
    expect(f.find('mouth').attrs.get('d')).toBe('M 84 104 Q 100 110 116 104')
    expect(f.find('cheek').attrs.get('fill')).toBe('rgb(255,122,147)')
    for (const id of ['face', 'torso']) {
      expect(f.find(id).writes.some(([name, value]) => name === 'fill' && value !== null && value.startsWith('rgb')), id).toBe(true)
      expect(f.find(id).attrs.get('fill'), id).toBe('url(#p-skin)')
    }
    for (const id of ['body', 'eyes', 'pupils', 'armR', 'brow']) expect(f.find(id).attrs.has('transform'), id).toBe(false)
    expect(f.find('head').attrs.get('transform')).toBe('translate(0 -2)')
  })
})
