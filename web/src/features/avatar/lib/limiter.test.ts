// The engine's limiter: whatever the tracks, gestures, drives and inputs of a look make
// together, what the frame loop writes turns back no sooner than REVERSAL_MS after it last
// turned, so it never flashes more than three times a second. And what never came near that
// (a blink, lip sync, a gaze, a slow fade, a nod) is written exactly as `sample` gives it.
//
// The figures run at 30 frames a second on the fake frames of the engine tests; a flash is a
// pair of opposing changes of at least a tenth of the range, counted in any one second.

import { describe, expect, it } from 'vitest'
import { stateList } from './builtin'
import { defaultLook } from './defaultLook'
import { GAZE_EASE_MS, REVERSAL_MS, STATE_FADE_MS, compile, createEngine, easeMouth, isLarge, sample } from './engine'
import type { Clip, Compiled, FigureHandle, WalkInputs } from './engine'
import { fakeFrames, readTransform, wire } from './fixtures'
import { checkLook } from './look'
import type { Animation, AvatarState, Look, LookNode, Track } from './types'

const FPS = 30
const FRAME = 1000 / FPS

/**
 * The most back-and-forth flashes in any one second of a series sampled at `fps`: changes of
 * at least `least` between turning points, counted in pairs.
 */
function flashesPerSecond(series: readonly number[], fps = FPS, least = 0.1): number {
  const events: number[] = []
  let dir = 0
  let extreme = series[0]
  for (let i = 1; i < series.length; i++) {
    const v = series[i]
    if (dir === 0) {
      if (Math.abs(v - series[0]) >= least) {
        dir = v > series[0] ? 1 : -1
        extreme = v
        events.push(i)
      }
    } else if (dir === 1) {
      if (v > extreme) extreme = v
      else if (extreme - v >= least) {
        dir = -1
        extreme = v
        events.push(i)
      }
    } else if (v < extreme) {
      extreme = v
    } else if (v - extreme >= least) {
      dir = 1
      extreme = v
      events.push(i)
    }
  }
  let worst = 0
  for (let a = 0; a < events.length; a++) {
    let b = a
    while (b + 1 < events.length && events[b + 1] - events[a] < fps) b++
    worst = Math.max(worst, Math.floor((b - a + 1) / 2))
  }
  return worst
}

// ---------------------------------------------------------------- the looks

/** A dark backdrop, a large white slab in a group, a large disc, and a narrow bar. */
const STAGE: LookNode[] = [
  { tag: 'rect', id: 'bg', attrs: { x: 0, y: 0, width: 200, height: 200, fill: '#202020' }, box: [0, 0, 200, 200] },
  { tag: 'g', id: 'grp', box: [10, 10, 180, 180], children: [{ tag: 'rect', id: 'slab', attrs: { x: 10, y: 10, width: 180, height: 180, fill: '#ffffff' }, box: [10, 10, 180, 180] }] },
  { tag: 'circle', id: 'dot', attrs: { cx: 100, cy: 100, r: 90, fill: '#ffffff' }, box: [10, 10, 180, 180] },
  { tag: 'rect', id: 'bar', attrs: { x: 95, y: 10, width: 10, height: 180, fill: '#ffffff' }, box: [95, 10, 10, 180] },
]

/** A look as the server stores it and the browser checks it: what reaches the engine. */
function stored(extra: Partial<Look>): Look {
  const look = checkLook(wire({ viewBox: [0, 0, 200, 200], nodes: STAGE, pivots: { slab: [100, 100], dot: [100, 100] }, roles: {}, animations: {}, states: {}, mouth: [], gaze: null, ...extra }))
  if (!look) throw new Error('the browser rejects the look')
  return look
}

/** Sixteen constant opacity tracks of the slab, dark and light in turn, each `gap` seconds after the one before. */
const strobe = (gap: number, more: Partial<Animation> = {}): Animation => ({
  duration: Math.max(16 * gap, 0.17),
  ...more,
  tracks: Array.from({ length: 16 }, (_, k): Track => ({ target: 'slab', prop: 'opacity', values: k % 2 === 0 ? [0, 0] : [1, 1], delay: Math.round(k * gap * 1000) / 1000 })),
})

// ---------------------------------------------------------------- running a figure

interface Film {
  state?: AvatarState
  reduced?: boolean
  seconds: number
  /** Before each frame: plays, stops, a new state. */
  script?: (frame: number, h: FigureHandle) => void
  inputs?: (frame: number) => WalkInputs | null
  mouth?: (frame: number) => number
}

/** A figure at 30 frames a second, and `read` after each frame. */
function film(look: Look, opts: Film, read: (f: ReturnType<typeof fakeFrames>) => number): number[] {
  const f = fakeFrames()
  const engine = createEngine(f.host)
  const handle = engine.attach(look, (id) => f.find(id), { state: opts.state ?? 'idle', reducedMotion: opts.reduced ?? false })
  let frame = 0
  if (opts.inputs) {
    const inputs = opts.inputs
    handle.setInputs(() => inputs(frame))
  }
  if (opts.mouth) {
    const mouth = opts.mouth
    handle.setMouth(() => mouth(frame))
  }
  const out: number[] = []
  for (frame = 0; frame <= opts.seconds * FPS; frame++) {
    opts.script?.(frame, handle)
    f.frame(FRAME)
    out.push(read(f))
  }
  expect(handle.frozen()).toBe(false)
  return out
}

const at = (frame: number, ms: number) => frame === Math.round(ms / FRAME)
const opacityOf = (id: string) => (f: ReturnType<typeof fakeFrames>) => Number(f.nodes.get(id)?.attrs.get('opacity') ?? 1)
const transformOf = (f: ReturnType<typeof fakeFrames>, id: string) => readTransform(f.nodes.get(id)?.attrs.get('transform'))
/** How much of the slab is drawn: its scaled area over its area. */
const coverage = (f: ReturnType<typeof fakeFrames>) => {
  const t = transformOf(f, 'slab')
  return Math.abs(t.sx * t.sy)
}

describe('the limiter: what the frame loop writes does not flash', () => {
  it('limits the paint of every part and every channel of a large one: a large part covers an eighth of the viewBox at its largest scale, reaches far, or has no box', () => {
    expect(REVERSAL_MS).toBe(150)
    const c = compile(stored({ animations: { grow: { duration: 1, tracks: [{ target: 'bar', prop: 'width', values: [10, 70] }, { target: 'slab', prop: 'translateY', values: [0, 1] }] } } })) as Compiled
    expect(isLarge(c, 'slab')).toBe(true)
    // 10 by 180 is 4.5% of the viewBox, but its width reaches 0.35 of the viewBox's.
    expect(isLarge(c, 'bar')).toBe(true)
    const narrow = compile(stored({ animations: { grow: { duration: 1, tracks: [{ target: 'bar', prop: 'width', values: [10, 69] }] } } })) as Compiled
    expect(isLarge(narrow, 'bar')).toBe(false)
    // Scaled by three (or by what it is drawn in), 10 by 180 covers more than an eighth.
    const scaled = (target: string) => compile(stored({ animations: { grow: { duration: 1, tracks: [{ target, prop: 'scaleX', values: [1, -3] }] } } })) as Compiled
    expect(isLarge(scaled('bar'), 'bar')).toBe(true)
    const nested = stored({
      nodes: [{ tag: 'g', id: 'frame', box: [95, 10, 10, 180], children: [{ tag: 'rect', id: 'bar', attrs: { x: 95, y: 10, width: 10, height: 180 }, box: [95, 10, 10, 180] }] }, { tag: 'g', id: 'loose' }],
      animations: { grow: { duration: 1, tracks: [{ target: 'frame', prop: 'scale', values: [1, 2] }] }, wave: { duration: 1, tracks: [{ target: ['bar', 'loose'], prop: 'rotate', values: [0, 10] }] } },
      pivots: {},
    })
    expect(isLarge(compile(nested) as Compiled, 'bar')).toBe(true)
    // A part without a box could be anything.
    expect(isLarge(compile(nested) as Compiled, 'loose')).toBe(true)
    // The default look: the body and the head are large; the eyes, the pupils, the mouth and the arms are not.
    const d = compile(defaultLook('t1')) as Compiled
    expect(['body', 'head'].filter((id) => isLarge(d, id))).toEqual(['body', 'head'])
    for (const id of ['eyes', 'pupils', 'mouth', 'armL', 'armR']) expect(isLarge(d, id), id).toBe(false)
  })

  it('keeps sixteen staggered constant opacity tracks to three flashes a second: a state one-shot, a recurring one, a gesture, with and without reduced motion', () => {
    for (const gap of [0.034, 0.05, 0.1]) {
      for (const reduced of [false, true]) {
        const oneShot = stored({ animations: { strobe: strobe(gap) }, states: { idle: [], working: ['strobe'] } })
        const shot = film(oneShot, { reduced, seconds: 4, script: (i, h) => void (at(i, 500) && h.setState('working')) }, opacityOf('slab'))
        expect(flashesPerSecond(shot), `one-shot ${gap} ${reduced}`).toBeLessThanOrEqual(3)
        // It did flash it: the slab went dark and light again.
        expect(Math.min(...shot), `one-shot ${gap} ${reduced}`).toBe(0)
        const every = stored({ animations: { strobe: strobe(gap, { every: [1, 1] }) }, states: { idle: ['strobe'] } })
        expect(flashesPerSecond(film(every, { reduced, seconds: 6 }, opacityOf('slab'))), `every ${gap} ${reduced}`).toBeLessThanOrEqual(3)
        const gesture = stored({ animations: { strobe: strobe(gap) }, states: { idle: [] } })
        const played = film(gesture, { reduced, seconds: 4, script: (i, h) => void (at(i, 500) && h.play('strobe', 'once')) }, opacityOf('slab'))
        expect(flashesPerSecond(played), `gesture ${gap} ${reduced}`).toBeLessThanOrEqual(3)
        // A gesture blends each track in over 150 ms: without reduced motion it only dims.
        expect(Math.min(...played)).toBeLessThan(reduced ? 0.01 : 0.9)
      }
    }
    // Without the limiter the one-shot flashes five times a second and more.
    const c = compile(stored({ animations: { strobe: strobe(0.1) }, states: { working: ['strobe'] } })) as Compiled
    const raw = Array.from({ length: 4 * FPS }, (_, i) => Number(sample(c, [{ name: 'strobe', startMs: 500, weight: 1, mode: 'state', times: 1 }], i * FRAME).get('slab')?.attrs.get('opacity') ?? 1))
    expect(flashesPerSecond(raw)).toBeGreaterThanOrEqual(5)
  })

  it('keeps a gesture replayed four times a second, and two dips in turn, to three flashes a second', () => {
    const look = stored({ animations: { dark: { duration: 0.2, tracks: [{ target: 'slab', prop: 'opacity', values: [0, 0] }] }, light: { duration: 0.2, tracks: [{ target: 'slab', prop: 'opacity', values: [null, null] }] } } })
    /** The number of the 250 ms slot from 0.5 s on that begins at this frame, for the first 30 slots; else null. */
    const slot = (i: number): number | null => {
      const k = Math.floor((i * FRAME - 500) / 250)
      return i * FRAME >= 500 && k < 30 && k !== Math.floor(((i - 1) * FRAME - 500) / 250) ? k : null
    }
    for (const reduced of [false, true]) {
      const again = film(look, { reduced, seconds: 9, script: (i, h) => void (slot(i) !== null && h.play('dark', 'once')) }, opacityOf('slab'))
      expect(flashesPerSecond(again), `again ${reduced}`).toBeLessThanOrEqual(3)
      expect(Math.min(...again)).toBe(0)
      const dips = film(look, { reduced, seconds: 9, script: (i, h) => void (slot(i) !== null && h.play((slot(i) ?? 0) % 2 === 0 ? 'dark' : 'light', 'once')) }, opacityOf('slab'))
      expect(flashesPerSecond(dips), `dips ${reduced}`).toBeLessThanOrEqual(3)
      expect(Math.min(...dips)).toBe(0)
    }
  })

  it('keeps an opacity drive on pace to three flashes a second while its input jitters', () => {
    const look = stored({ drives: [{ input: 'pace', target: 'slab', prop: 'opacity', values: [1, 0] }] })
    for (const every of [1, 2, 3]) {
      const series = film(look, { seconds: 4, inputs: (i) => ({ pace: Math.floor(i / every) % 2 }) }, opacityOf('slab'))
      expect(flashesPerSecond(series), `every ${every} frames`).toBeLessThanOrEqual(3)
      expect(Math.min(...series), `every ${every} frames`).toBeLessThan(1)
    }
  })

  it('keeps a large part that steps its size or its place to three flashes a second', () => {
    const scaleStep = stored({ animations: { blink: { duration: 0.1, loop: true, ease: 'step', tracks: [{ target: 'slab', prop: 'scale', values: [1, 0, 1], times: [0, 0.5, 1] }] } }, states: { idle: ['blink'] } })
    expect(flashesPerSecond(film(scaleStep, { seconds: 4 }, coverage))).toBeLessThanOrEqual(3)
    const c = compile(scaleStep) as Compiled
    const raw = Array.from({ length: 4 * FPS }, (_, i) => {
      const t = readTransform(sample(c, [{ name: 'blink', startMs: 0, weight: 1, mode: 'state', times: 1 }], i * FRAME).get('slab')?.transform)
      return Math.abs(t.sx * t.sy)
    })
    expect(flashesPerSecond(raw)).toBeGreaterThanOrEqual(10)

    const pulse = stored({ animations: { pulse: { duration: 0.1, loop: true, tracks: [{ target: 'dot', prop: 'r', values: [90, 0, 90] }] } }, states: { idle: ['pulse'] } })
    expect(flashesPerSecond(film(pulse, { seconds: 4 }, (f) => (Number(f.nodes.get('dot')?.attrs.get('r') ?? 90) / 90) ** 2))).toBeLessThanOrEqual(3)

    const widen = stored({ animations: { widen: { duration: 0.1, loop: true, tracks: [{ target: 'bar', prop: 'width', values: [10, 180, 10] }] } }, states: { idle: ['widen'] } })
    expect(flashesPerSecond(film(widen, { seconds: 4 }, (f) => Number(f.nodes.get('bar')?.attrs.get('width') ?? 10) / 180))).toBeLessThanOrEqual(3)

    // Out of view and back: the slab's share that is still in the viewBox.
    const shove = stored({
      animations: { shove: { duration: 1.6, tracks: Array.from({ length: 16 }, (_, k): Track => ({ target: 'slab', prop: 'translateX', values: k % 2 === 0 ? [400, 400] : [-400, -400], delay: Math.round(k * 100) / 1000 })) } },
      states: { idle: [], working: ['shove'] },
    })
    const inView = (f: ReturnType<typeof fakeFrames>) => Math.max(0, 180 - Math.abs(transformOf(f, 'slab').tx)) / 180
    for (const reduced of [true, false]) {
      const series = film(shove, { reduced, seconds: 4, script: (i, h) => void (at(i, 500) && h.setState('working')) }, inView)
      expect(flashesPerSecond(series), `reduced ${reduced}`).toBeLessThanOrEqual(3)
      expect(Math.min(...series)).toBe(0)
    }
  })

  it('keeps a mouth entry and a mouth drive that scale a large part to three flashes a second at syllable rate', () => {
    const syllables = (i: number) => Math.floor(i / 3) % 2
    const entry = stored({ mouth: [{ target: 'slab', prop: 'scale', open: 0, closed: 1 }] })
    const opened = film(entry, { seconds: 4, mouth: syllables }, coverage)
    expect(flashesPerSecond(opened)).toBeLessThanOrEqual(3)
    expect(Math.min(...opened)).toBeLessThan(0.7)
    const drive = stored({ drives: [{ input: 'mouth', target: 'slab', prop: 'scale', values: [1, 0] }] })
    const driven = film(drive, { seconds: 4, mouth: syllables }, coverage)
    expect(flashesPerSecond(driven)).toBeLessThanOrEqual(3)
    expect(Math.min(...driven)).toBeLessThan(0.7)
  })

  it('releases a held channel when the gesture that moved it ends inside the hold: at rest within REVERSAL_MS and a frame', () => {
    const look = stored({
      animations: {
        shove: { duration: 0.1, tracks: [{ target: 'slab', prop: 'translateX', values: [40, 40] }] },
        pull: { duration: 0.1, tracks: [{ target: 'slab', prop: 'translateX', values: [-40, -40] }] },
      },
    })
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const handle = engine.attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: true })
    const tx = () => transformOf(f, 'slab').tx
    let clock = -FRAME
    const frame = () => {
      f.frame(FRAME)
      clock += FRAME
    }
    frame()
    handle.play('shove', 'once')
    frame()
    expect(tx()).toBe(40)
    // Back to rest, which turns it: the turn is accepted, and the pull goes on the same way.
    while (tx() !== 0) frame()
    const turned = clock
    handle.play('pull', 'once')
    frame()
    expect(tx()).toBe(-40)
    // The pull is over before REVERSAL_MS: going back would turn it again, so it is held.
    const ends = turned + 100
    let held = false
    while (clock <= ends + FRAME) {
      frame()
      if (clock > ends + 1 && tx() === -40) held = true
    }
    expect(held).toBe(true)
    // Nothing plays any more, and yet the loop goes on and lets it go.
    expect(f.waiting()).toBe(1)
    while (tx() !== 0 && clock < ends + 1000) frame()
    expect(tx()).toBe(0)
    expect(clock - turned).toBeGreaterThanOrEqual(REVERSAL_MS)
    expect(clock - ends).toBeLessThanOrEqual(REVERSAL_MS + FRAME + 1e-6)
    expect(f.find('slab').attrs.has('transform')).toBe(false)
  })
})

// ---------------------------------------------------------------- what it leaves alone

describe('the limiter: what it leaves as it is', () => {
  interface Plan {
    state: AvatarState
    seconds: number
    /** A gesture to play once before this frame, by name. */
    play?: (frame: number) => string | null
    gaze?: (frame: number) => readonly [number, number]
    mouth?: (frame: number) => number
  }

  /**
   * Runs a figure at 30 frames a second and checks each frame against `sample` with the
   * clips and inputs the loop gives it, worked out here as the loop does: the state's loops
   * fading in from the start, gestures played once, the mouth and the pupils eased.
   */
  function expectAsSampled(look: Look, plan: Plan): number {
    const c = compile(look) as Compiled
    const loops = stateList(look, plan.state)
    const f = fakeFrames()
    const engine = createEngine(f.host)
    const handle = engine.attach(look, (id) => f.find(id), { state: plan.state, reducedMotion: false })
    let frame = 0
    const gazeAt = plan.gaze ?? (() => [0, 0] as const)
    const mouthAt = plan.mouth ?? (() => -1)
    handle.setGaze(() => gazeAt(frame))
    handle.setMouth(() => mouthAt(frame))
    const rest = sample(c, [], 0)
    const played: Clip[] = []
    let time = 0
    let last: number | null = null
    let clock = 0
    let open = 0
    let until = -1
    let gx = 0
    let gy = 0
    let compared = 0
    for (frame = 0; frame <= plan.seconds * FPS; frame++) {
      const name = plan.play?.(frame)
      if (name) {
        expect(handle.play(name, 'once')).toBe(true)
        played.push({ name, startMs: clock, weight: 1, mode: 'once', times: 1 })
      }
      f.frame(FRAME)
      // The loop's time, as it counts it: the first frame counts none.
      time += FRAME
      const dt = last === null ? 0 : Math.min(time - last, 250)
      last = time
      clock += dt
      const want = mouthAt(frame)
      if (want >= 0) until = clock + 120
      let mouth: number | null = null
      if (clock < until) {
        open = easeMouth(open, want >= 0 ? Math.min(want, 1) : 0, dt)
        mouth = open
      } else {
        open = 0
      }
      const [ax, ay] = gazeAt(frame)
      const k = dt > 0 ? 1 - Math.exp(-dt / GAZE_EASE_MS) : 0
      gx += (ax - gx) * k
      gy += (ay - gy) * k
      const clips: Clip[] = [...loops.map((n): Clip => ({ name: n, startMs: 0, weight: Math.min(clock / STATE_FADE_MS, 1), mode: 'state', times: 1 })), ...played]
      const poses = sample(c, clips, clock, { mouth, gaze: [gx, gy], pace: 0, turn: 0, held: 0 })
      for (const [id, pose] of poses) {
        const node = f.nodes.get(id)
        const wrote = (name: string) => node?.writes.some(([n]) => n === name) === true
        const now = (name: string, atRest: string | null | undefined) => (wrote(name) ? (node?.attrs.get(name) ?? null) : (atRest ?? null))
        expect(now('transform', rest.get(id)?.transform), `${id} transform at frame ${frame}`).toBe(pose.transform ?? null)
        for (const [name, value] of pose.attrs) expect(now(name, rest.get(id)?.attrs.get(name)), `${id} ${name} at frame ${frame}`).toBe(value)
        compared++
      }
    }
    return compared
  }

  /** The default look with only these state animations: nothing random plays. */
  const quiet = (states: Look['states']): Look => ({ ...defaultLook('t1'), states })

  it('writes the built-in blink and talk, the pupils’ gaze and lip sync exactly as `sample` gives them', () => {
    const look = quiet({ idle: [], speaking: ['breathe', 'talk'] })
    const syllable = (i: number) => (i < 20 || i > 150 ? -1 : [0.9, 0.1, 0.7, 0.2, 1, 0][Math.floor(i / 2) % 6])
    const compared = expectAsSampled(look, {
      state: 'speaking',
      seconds: 6,
      play: (i) => (i % 40 === 10 ? 'blink' : null),
      gaze: (i) => [Math.cos(i / 7) * 0.6, Math.sin(i / 5) * 0.6],
      mouth: syllable,
    })
    expect(compared).toBeGreaterThan(500)
  })

  it('writes a slow fade of a large part and a nod of the head exactly as `sample` gives them', () => {
    // The fastest fade a look may have, 0.17 s from keyframe to keyframe, played now and then.
    const fade: Animation = { duration: 0.68, ease: 'inOut', tracks: [{ target: 'slab', prop: 'opacity', values: [1, 0.2, 1, 0.2, 1] }] }
    expectAsSampled(stored({ animations: { fade } }), { state: 'idle', seconds: 5, play: (i) => (i % 45 === 3 ? 'fade' : null) })
    // A state's loop of 0.2 s a keyframe, from the moment it fades in.
    const pulse: Animation = { duration: 0.8, loop: true, ease: 'inOut', tracks: [{ target: 'slab', prop: 'opacity', values: [1, 0.2, 1, 0.2, 1] }] }
    expectAsSampled(stored({ animations: { pulse }, states: { idle: ['pulse'] } }), { state: 'idle', seconds: 4 })
    const d = compile(defaultLook('t1')) as Compiled
    expect(isLarge(d, 'head')).toBe(true)
    expectAsSampled(quiet({ idle: [] }), { state: 'idle', seconds: 5, play: (i) => (i % 30 === 3 ? 'nod' : null) })
  })

  it('applies to `sample` called directly not at all, nor to the rest pose on detach', () => {
    const look = stored({ animations: { strobe: strobe(0.05) }, states: { working: ['strobe'] } })
    const c = compile(look) as Compiled
    const clips: Clip[] = [{ name: 'strobe', startMs: 0, weight: 1, mode: 'state', times: 1 }]
    expect(sample(c, clips, 10).get('slab')?.attrs.get('opacity')).toBe('0')
    expect(sample(c, clips, 60).get('slab')?.attrs.get('opacity')).toBe('1')
    expect(sample(c, clips, 110).get('slab')?.attrs.get('opacity')).toBe('0')
    const f = fakeFrames()
    const handle = createEngine(f.host).attach(look, (id) => f.find(id), { state: 'idle', reducedMotion: false })
    f.frame(FRAME)
    handle.setState('working')
    for (let i = 0; i < 6; i++) f.frame(FRAME)
    expect(f.find('slab').writes.length).toBeGreaterThan(0)
    handle.detach()
    expect(f.find('slab').attrs.has('opacity')).toBe(false)
    expect(f.find('slab').writes.at(-1)).toEqual(['opacity', null])
  })
})
