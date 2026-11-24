// The frame loop's guard against what a figure costs the browser to draw: frames that keep
// coming late while a figure with a filter, a mask or a clip moves freeze the costliest such
// figure, one at a time, and an ordinary slow moment freezes nothing.

import { describe, expect, it } from 'vitest'
import { LATE_FRAMES, LATE_MOST_MS, LATE_MS, LATE_SPAN_MS, ON_TIME_MS, createEngine } from './engine'
import { fakeFrames, readTransform } from './fixtures'
import type { Animation, Look } from './types'

/** 10 units a second along x, for ever: the part's x tells whether the figure still moves. */
const travel: Animation = { duration: 10, loop: true, tracks: [{ target: 'dot', prop: 'translateX', values: [0, 100] }] }

/** A dot that travels, painted with what `paint` adds (a filter of `steps` blurs, a mask, a clip). */
function look(paint: Record<string, string | number> = {}, steps = 1, states: Look['states'] = { idle: ['travel'] }): Look {
  return {
    viewBox: [0, 0, 200, 200],
    nodes: [
      {
        tag: 'defs',
        children: [
          { tag: 'filter', id: 'soft', children: Array.from({ length: steps }, () => ({ tag: 'feGaussianBlur' as const, attrs: { stdDeviation: 2 } })) },
          { tag: 'mask', id: 'cut', children: [{ tag: 'rect', attrs: { x: 0, y: 0, width: 200, height: 200, fill: '#ffffff' } }] },
          { tag: 'clipPath', id: 'frame', children: [{ tag: 'rect', attrs: { x: 0, y: 0, width: 200, height: 200 } }] },
        ],
      },
      { tag: 'circle', id: 'dot', attrs: { cx: 100, cy: 100, r: 10, fill: '#3366cc', ...paint }, box: [90, 90, 20, 20] },
    ],
    pivots: {},
    roles: {},
    animations: { travel },
    states,
    mouth: [],
    gaze: null,
  }
}

const filtered = (steps = 1) => look({ filterRef: 'soft' }, steps)

/** An engine and figures of the looks given, each on nodes of its own. */
function stage(...looks: { look: Look; reducedMotion?: boolean }[]) {
  const f = fakeFrames()
  const engine = createEngine(f.host)
  const figures = looks.map(({ look, reducedMotion = false }) => {
    const nodes = fakeFrames()
    const handle = engine.attach(look, (id) => nodes.find(id), { state: 'idle', reducedMotion })
    return { handle, x: () => readTransform(nodes.nodes.get('dot')?.attrs.get('transform')).tx, writes: () => nodes.find('dot').writes.length }
  })
  // Frames on time for a while first: the figures move.
  f.run(400)
  return { f, engine, figures }
}

describe('frames that keep coming late', () => {
  it('freeze a moving figure with a filter within three of its frames when each takes seconds, and the others go on', () => {
    const { f, figures } = stage({ look: filtered() }, { look: look() })
    const [heavy, plain] = figures
    for (let i = 1; i < LATE_FRAMES; i++) {
      f.frame(5000)
      expect(heavy.handle.frozen()).toBe(false)
    }
    f.frame(5000)
    expect(heavy.handle.frozen()).toBe(true)
    expect(plain.handle.frozen()).toBe(false)
    // It stands in its rest pose and writes nothing more; the other one keeps moving.
    const stuck = heavy.writes()
    expect(heavy.x()).toBe(0)
    const moving = plain.x()
    f.run(1000)
    expect(heavy.writes()).toBe(stuck)
    expect(plain.x()).toBeGreaterThan(moving)
    expect(heavy.handle.play('travel')).toBe(false)
  })

  it('freeze it in about three seconds when frames come a few a second', () => {
    const { f, figures } = stage({ look: filtered() })
    const frames = LATE_SPAN_MS / 250
    for (let i = 1; i < frames; i++) f.frame(250)
    expect(figures[0].handle.frozen()).toBe(false)
    f.frame(250)
    expect(figures[0].handle.frozen()).toBe(true)
  })

  it('freeze a figure with a mask or a clip as well', () => {
    const paints: Record<string, string>[] = [{ maskRef: 'cut' }, { clipPathRef: 'frame' }]
    for (const paint of paints) {
      const { f, figures } = stage({ look: look(paint) })
      for (let i = 0; i < LATE_FRAMES; i++) f.frame(5000)
      expect(figures[0].handle.frozen(), JSON.stringify(paint)).toBe(true)
    }
  })

  it('freeze the costliest figure first, one at a time, and never one without a filter, a mask or a clip', () => {
    const { f, figures } = stage({ look: filtered(1) }, { look: filtered(4) }, { look: look({ opacity: 0.5 }) })
    const [light, heavy, plain] = figures
    for (let i = 0; i < LATE_FRAMES; i++) f.frame(5000)
    expect([light.handle.frozen(), heavy.handle.frozen(), plain.handle.frozen()]).toEqual([false, true, false])
    for (let i = 0; i < LATE_FRAMES; i++) f.frame(5000)
    expect([light.handle.frozen(), heavy.handle.frozen(), plain.handle.frozen()]).toEqual([true, true, false])
    // Nothing is left to freeze: the plain figure moves on however late the frames come.
    for (let i = 0; i < 4 * LATE_FRAMES; i++) f.frame(5000)
    expect(plain.handle.frozen()).toBe(false)
    const at = plain.x()
    f.run(400)
    expect(plain.x()).toBeGreaterThan(at)
  })

  it('freeze no figure that stands still: reduced motion, or a state that moves nothing', () => {
    const { f, figures } = stage({ look: filtered(4), reducedMotion: true }, { look: look({ filterRef: 'soft' }, 4, { idle: [] }) })
    for (let i = 0; i < 6 * LATE_FRAMES; i++) f.frame(5000)
    expect(figures.map((figure) => figure.handle.frozen())).toEqual([false, false])
  })

  it('start counting again after time the tab was away', () => {
    const { f, engine, figures } = stage({ look: filtered() })
    for (let i = 1; i < LATE_FRAMES; i++) f.frame(5000)
    engine.setRunning(false)
    f.frame(600_000)
    engine.setRunning(true)
    f.frame(40)
    for (let i = 1; i < LATE_FRAMES; i++) f.frame(5000)
    expect(figures[0].handle.frozen()).toBe(false)
    f.frame(5000)
    expect(figures[0].handle.frozen()).toBe(true)
  })
})

describe('an ordinary slow moment', () => {
  /** Frames on time (40 ms) for `ms`. */
  const onTime = (f: ReturnType<typeof fakeFrames>, ms: number) => f.run(ms)

  it('freezes nothing: one long pause, a garbage collection now and then, a page busy for a second or two', () => {
    const moments: [string, (f: ReturnType<typeof fakeFrames>) => void][] = [
      // A tab that comes back without having been stopped: one frame ten minutes late.
      ['a long pause', (f) => f.frame(600_000)],
      // A collection of 300 ms every second and a half, for a minute.
      [
        'garbage collections',
        (f) => {
          for (let i = 0; i < 40; i++) {
            f.frame(300)
            onTime(f, ON_TIME_MS + 200)
          }
        },
      ],
      // A second of work at once, then a few slow frames while the page settles.
      [
        'a page busy for a second',
        (f) => {
          f.frame(1000)
          for (let i = 0; i < 5; i++) f.frame(150)
        },
      ],
      // Two seconds of frames late by half again, and two seconds of work at once.
      [
        'a page busy for two seconds',
        (f) => {
          for (let i = 0; i < 13; i++) f.frame(150)
          onTime(f, ON_TIME_MS)
          f.frame(2000)
          f.frame(LATE_MS)
        },
      ],
    ]
    for (const [name, moment] of moments) {
      const { f, figures } = stage({ look: filtered(4) })
      moment(f)
      onTime(f, 2000)
      expect(figures[0].handle.frozen(), name).toBe(false)
      const at = figures[0].x()
      onTime(f, 400)
      expect(figures[0].x(), name).not.toBe(at)
    }
  })

  it('is counted in the units the guard says: a late frame counts for a second at most, a second on time starts again', () => {
    expect(LATE_MS).toBeGreaterThanOrEqual(3 * 33)
    expect(LATE_FRAMES * LATE_MOST_MS).toBeGreaterThanOrEqual(LATE_SPAN_MS)
    // Two late frames, then a second on time, then two more: never three in a run.
    const { f, figures } = stage({ look: filtered() })
    for (let round = 0; round < 10; round++) {
      f.frame(5000)
      f.frame(5000)
      onTime(f, ON_TIME_MS + 40)
    }
    expect(figures[0].handle.frozen()).toBe(false)
  })
})
