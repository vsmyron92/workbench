// How a gesture blends in over the state, and out again: in over GESTURE_BLEND_MS, or a third
// of one pass of a short one, so that a played blink closes the eyes as its pictures show them;
// out over GESTURE_BLEND_MS, after what it plays.

import { describe, expect, it } from 'vitest'
import { rolePart } from './builtin'
import { defaultLook } from './defaultLook'
import { GESTURE_BLEND_MS, compile, createEngine, sample } from './engine'
import type { Clip, Compiled } from './engine'
import { fakeFrames, readTransform, wire } from './fixtures'
import { checkLook } from './look'
import type { Look } from './types'

const FRAME = 1000 / 30

/** The default look with no state animations: nothing but the gesture moves. */
const still = (): Look => ({ ...defaultLook('t1'), states: { idle: [] } })

/** A small dot, and two gestures that hold it 40 px to the right: one of 0.6 s, one of 0.18 s. */
function shoves(): Look {
  const look = checkLook(
    wire({
      viewBox: [0, 0, 200, 200],
      nodes: [{ tag: 'circle', id: 'dot', attrs: { cx: 100, cy: 100, r: 6, fill: '#ffffff' }, box: [94, 94, 12, 12] }],
      pivots: {},
      roles: {},
      animations: {
        long: { duration: 0.6, tracks: [{ target: 'dot', prop: 'translateX', values: [40, 40] }] },
        short: { duration: 0.18, tracks: [{ target: 'dot', prop: 'translateX', values: [40, 40] }] },
      },
      states: { idle: [] },
      mouth: [],
      gaze: null,
    }),
  )
  if (!look) throw new Error('the browser rejects the look')
  return look
}

const once = (name: string, startMs = 0): Clip => ({ name, startMs, weight: 1, mode: 'once', times: 1 })
const txAt = (c: Compiled, clips: Clip[], t: number) => readTransform(sample(c, clips, t).get('dot')?.transform).tx

describe('the blend of a gesture', () => {
  it('blends a gesture of 450 ms or more in over 150 ms, a shorter one over a third of its pass, and either out over 150 ms', () => {
    expect(GESTURE_BLEND_MS).toBe(150)
    const c = compile(shoves()) as Compiled
    // 0.6 s: 150 ms in, 150 ms out.
    expect(txAt(c, [once('long')], 75)).toBeCloseTo(20, 6)
    expect(txAt(c, [once('long')], 150)).toBeCloseTo(40, 6)
    expect(txAt(c, [once('long')], 675)).toBeCloseTo(20, 6)
    expect(txAt(c, [once('long')], 751)).toBe(0)
    // 0.18 s: 60 ms in; out after it has played, over the whole 150 ms, which takes nothing from it.
    expect(txAt(c, [once('short')], 30)).toBeCloseTo(20, 6)
    expect(txAt(c, [once('short')], 60)).toBeCloseTo(40, 6)
    expect(txAt(c, [once('short')], 180)).toBeCloseTo(40, 6)
    expect(txAt(c, [once('short')], 255)).toBeCloseTo(20, 6)
    expect(txAt(c, [once('short')], 300)).toBeCloseTo(8, 6)
    expect(txAt(c, [once('short')], 331)).toBe(0)
    // Held or looping, it blends in the same way.
    expect(txAt(c, [{ ...once('short'), mode: 'hold' }], 30)).toBeCloseTo(20, 6)
    expect(txAt(c, [{ ...once('long'), mode: 'hold' }], 75)).toBeCloseTo(20, 6)
  })

  it('closes the eyes in a blink played as a gesture, as its pictures show them', () => {
    const c = compile(still()) as Compiled
    // Its lowest keyframe, scaleY 0.1 at 90 ms, is reached: the blend took 60 ms.
    expect(readTransform(sample(c, [once('blink')], 90).get('eyes')?.transform).sy).toBeCloseTo(0.1, 6)
    /** The lowest scaleY of the eyes on a figure that plays a blink on a frame, drawn every `ms`. */
    const lowest = (ms: number): number => {
      const f = fakeFrames()
      const handle = createEngine(f.host).attach(still(), (id) => f.find(id), { state: 'idle', reducedMotion: false })
      f.frame(ms)
      expect(handle.play('blink', 'once')).toBe(true)
      let least = 1
      for (let i = 0; i < 15; i++) {
        f.frame(ms)
        least = Math.min(least, readTransform(f.nodes.get('eyes')?.attrs.get('transform')).sy)
      }
      // And then they are open again, at rest.
      expect(f.nodes.get('eyes')?.attrs.has('transform')).toBe(false)
      return least
    }
    // A frame on its lowest keyframe shows it closed.
    expect(lowest(45)).toBeLessThanOrEqual(0.15)
    // At 30 frames a second the nearest frame is 10 ms after it: 0.2 (it was 0.47).
    expect(lowest(FRAME)).toBeLessThanOrEqual(0.2 + 1e-9)
  })

  it('leaves a wave as it was: after its first 150 ms it is the whole of its keyframes', () => {
    const c = compile(still()) as Compiled
    const wave = rolePart(c.look, 'armR') ?? 'armR'
    const rot = (clips: Clip[], t: number) => readTransform(sample(c, clips, t).get(wave)?.transform).rot
    // Its keyframes as a state plays them, added to a rest of 0: what the gesture shows at its full weight.
    const full = (t: number) => rot([{ ...once('wave'), mode: 'state' }], t)
    for (let t = 150; t <= 1400; t += 25) expect(rot([once('wave')], t), `at ${t} ms`).toBeCloseTo(full(t), 6)
    // Before that it blends in over 150 ms, as before.
    expect(rot([once('wave')], 75)).toBeCloseTo(full(75) * 0.5, 1)
    expect(Math.abs(full(75))).toBeGreaterThan(1)
  })

  it('still fades a stopped gesture out over 150 ms, a short one too', () => {
    for (const [name, blend] of [
      ['short', 150],
      ['long', 150],
    ] as const) {
      const f = fakeFrames()
      const handle = createEngine(f.host).attach(shoves(), (id) => f.find(id), { state: 'idle', reducedMotion: false })
      const tx = () => readTransform(f.nodes.get('dot')?.attrs.get('transform')).tx
      f.frame(FRAME)
      handle.play(name, 'hold')
      for (let i = 0; i < 10; i++) f.frame(FRAME)
      expect(tx(), name).toBeCloseTo(40, 6)
      handle.stop(name)
      // The next frame it is on its way back, not at rest.
      f.frame(FRAME)
      expect(tx(), name).toBeCloseTo(40 * (1 - FRAME / blend), 1)
      let frames = 1
      while (tx() !== 0 && frames < 20) {
        f.frame(FRAME)
        frames++
      }
      // At rest once the blend has passed, within a frame.
      expect(frames * FRAME, name).toBeGreaterThanOrEqual(blend)
      expect(frames * FRAME, name).toBeLessThan(blend + FRAME)
    }
  })

  it('blends nothing with reduced motion, as before: the last frame from the start, and nothing after it has played or was stopped', () => {
    for (const name of ['short', 'long'] as const) {
      const ms = name === 'short' ? 180 : 600
      const f = fakeFrames()
      const handle = createEngine(f.host).attach(shoves(), (id) => f.find(id), { state: 'idle', reducedMotion: true })
      const tx = () => readTransform(f.nodes.get('dot')?.attrs.get('transform')).tx
      f.frame(FRAME)
      handle.play(name, 'once')
      // The k-th frame after the play is k frames into the gesture (away from the moment it ends).
      for (let k = 1; k * FRAME < ms + 2 * FRAME; k++) {
        f.frame(FRAME)
        if (k * FRAME < ms - 1) expect(tx(), `${name} at ${k * FRAME}`).toBe(40)
        if (k * FRAME > ms + 1) expect(tx(), `${name} at ${k * FRAME}`).toBe(0)
      }
      handle.play(name, 'hold')
      f.frame(FRAME)
      expect(tx(), name).toBe(40)
      handle.stop(name)
      f.frame(FRAME)
      expect(tx(), name).toBe(0)
    }
  })
})

describe('what plays on a figure', () => {
  it('names a played gesture until it has played and given way, a held one until it is stopped, and none once detached', () => {
    const f = fakeFrames()
    const handle = createEngine(f.host).attach(shoves(), (id) => f.find(id), { state: 'idle', reducedMotion: false })
    f.frame(FRAME)
    expect(handle.playing()).toEqual([])
    expect(handle.play('short', 'once')).toBe(true)
    expect(handle.playing()).toEqual(['short'])
    // 180 ms and the 150 ms it gives way over.
    let t = 0
    while (handle.playing().length > 0 && t < 1000) {
      f.frame(FRAME)
      t += FRAME
    }
    expect(t).toBeGreaterThan(330 - FRAME)
    expect(t).toBeLessThanOrEqual(330 + FRAME)
    // (A held one ends at the next play: the one played once comes first.)
    handle.play('short', 'once')
    handle.play('long', 'hold')
    expect(handle.playing()).toEqual(['short', 'long'])
    for (let i = 0; i < 30; i++) f.frame(FRAME)
    expect(handle.playing()).toEqual(['long'])
    // A stopped one only gives way: it no longer plays.
    handle.stop('long')
    expect(handle.playing()).toEqual([])
    handle.play('long', 'hold')
    expect(handle.playing()).toEqual(['long'])
    handle.detach()
    expect(handle.playing()).toEqual([])
  })
})
