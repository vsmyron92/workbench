import { describe, expect, it } from 'vitest'
import { APART_PX, AUDIENCE_PX, BUSY_REACH_PX, HELD_BACK_MS, HOME_PX, HURRY_PX_S, MEET_GAP_PX, POST_GAP_PX, STROLL_PX_S, VISIT_PX_S, attend, claimOf, feetBounds, gazeToward, into, newWorld, placePost, postFor, samplePoints, step } from './roam'
import type { Box, Cue, Point, Post, StepInput, Walker, World } from './roam'

/** A generator with a seed: the same walk every time. */
function seeded(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const BOUNDS: Box = { left: 100, top: 100, right: 900, bottom: 600 }
const SIZE = 56
const walker = (id: string, over: Partial<Walker> = {}): Walker => ({ id, stays: false, post: null, held: null, busy: false, asleep: false, ...over })
const column = (_id: string, index: number): Point => ({ x: 860, y: 180 + index * 64 })

interface Run {
  world: World
  now: number
  cues: Cue[]
  /** Advances by `ms` in steps of a frame; `each` may change the facts before every step. */
  go(ms: number, each?: (input: StepInput) => void): void
  input: StepInput
}

function run(walkers: Walker[], seed = 7, over: Partial<StepInput> = {}): Run {
  const random = seeded(seed)
  const r: Run = {
    world: newWorld(),
    now: 0,
    cues: [],
    input: { now: 0, dt: 16, bounds: BOUNDS, walkers, spawn: column, obstacles: [], pointer: null, size: SIZE, speaker: null, finished: [], random, ...over },
    go(ms, each) {
      const end = r.now + ms
      while (r.now < end) {
        r.now += 16
        r.input.now = r.now
        each?.(r.input)
        step(r.world, r.input)
        r.cues.push(...r.world.cues.splice(0))
      }
    },
  }
  return r
}

const body = (r: Run, id: string) => {
  const b = r.world.bodies.get(id)
  if (!b) throw new Error(`no body ${id}`)
  return b
}
const inBounds = (p: Point, box: Box = BOUNDS) => p.x >= box.left - 1e-6 && p.x <= box.right + 1e-6 && p.y >= box.top - 1e-6 && p.y <= box.bottom + 1e-6

describe('who is on stage', () => {
  it('puts a new avatar where the column has it, and takes one that left away', () => {
    const r = run([walker('a'), walker('b')])
    attend(r.world, r.input)
    expect([body(r, 'a').x, body(r, 'a').y, body(r, 'b').y]).toEqual([860, 180, 244])
    r.input.walkers = [walker('b')]
    r.go(16)
    expect([...r.world.bodies.keys()]).toEqual(['b'])
  })

  it('brings a place that is outside into the bounds', () => {
    const r = run([walker('a')], 1, { spawn: () => ({ x: 5000, y: -40 }) })
    attend(r.world, r.input)
    expect([body(r, 'a').x, body(r, 'a').y]).toEqual([BOUNDS.right, BOUNDS.top])
    expect(into({ x: Number.NaN, y: 300 }, BOUNDS)).toEqual({ x: BOUNDS.left, y: 300 })
  })
})

describe('a stroll', () => {
  it('rests, walks at a stroll, hops while it walks and stands again, inside the bounds', () => {
    const r = run([walker('a')])
    let walked = false
    let fastest = 0
    let hopped = 0
    let last = { x: 860, y: 180 }
    r.go(60_000, () => {
      const b = r.world.bodies.get('a')
      if (!b) return
      fastest = Math.max(fastest, Math.hypot(b.x - last.x, b.y - last.y) / 0.016)
      last = { x: b.x, y: b.y }
      if (b.walking) walked = true
      hopped = Math.max(hopped, b.hop)
      expect(inBounds(b)).toBe(true)
    })
    expect(walked).toBe(true)
    expect(fastest).toBeGreaterThan(STROLL_PX_S * 0.5)
    expect(fastest).toBeLessThanOrEqual(STROLL_PX_S + 0.5)
    expect(hopped).toBeGreaterThan(0.9)
    // It moved from where it began, and stands at rest between walks.
    r.go(30_000)
    const b = body(r, 'a')
    expect(Math.hypot(b.x - 860, b.y - 180)).toBeGreaterThan(10)
  })

  it('comes down from its hop and straightens up when it stands', () => {
    const r = run([walker('a')])
    r.go(40_000)
    // Make it stand: the pointer rests on it.
    r.go(2000, (i) => (i.pointer = { x: body(r, 'a').x, y: body(r, 'a').y - SIZE / 2 }))
    const b = body(r, 'a')
    expect([b.walking, b.hop, b.lean]).toEqual([false, 0, 0])
  })

  it('does not walk while the pointer is near, so that it can be pressed', () => {
    const r = run([walker('a')])
    const at = { x: 860, y: 180 }
    r.go(40_000, (i) => (i.pointer = { x: at.x + 10, y: at.y - SIZE / 2 }))
    expect([body(r, 'a').x, body(r, 'a').y]).toEqual([at.x, at.y])
  })

  it('keeps its strolls short while its session works', () => {
    const r = run([walker('a', { busy: true })], 3)
    let longest = 0
    let from: Point | null = null
    let was = 'rest'
    r.go(600_000, () => {
      const b = r.world.bodies.get('a')
      if (!b) return
      if (was === 'rest' && b.mode === 'walk' && b.to) {
        from = { x: b.x, y: b.y }
        longest = Math.max(longest, Math.hypot(b.to.x - from.x, b.to.y - from.y))
      }
      was = b.mode
    })
    expect(longest).toBeGreaterThan(0)
    expect(longest).toBeLessThanOrEqual(BUSY_REACH_PX + 1e-6)
  })

  it('does not set off with all the others after a pause', () => {
    const r = run([walker('a'), walker('b'), walker('c'), walker('d')], 6)
    r.go(16)
    // The tab was hidden for ten minutes: every rest is long over.
    r.now += 600_000
    r.input.now = r.now
    r.input.dt = 600_000
    step(r.world, r.input)
    r.input.dt = 16
    const untils = ['a', 'b', 'c', 'd'].map((id) => body(r, id).until - r.now)
    for (const u of untils) {
      expect(u).toBeGreaterThanOrEqual(300 - 16)
      expect(u).toBeLessThanOrEqual(6000)
    }
    expect(new Set(untils.map((u) => Math.round(u / 50))).size).toBeGreaterThan(1)
    expect(['a', 'b', 'c', 'd'].every((id) => body(r, id).mode === 'rest')).toBe(true)
  })

  it('paces little while its session works, and never while it sleeps', () => {
    const idle = run([walker('a')], 3)
    const busy = run([walker('a', { busy: true })], 3)
    const asleep = run([walker('a', { asleep: true })], 3)
    let far = { idle: 0, busy: 0 }
    idle.go(120_000, () => (far.idle = Math.max(far.idle, Math.hypot((idle.world.bodies.get('a')?.x ?? 860) - 860, (idle.world.bodies.get('a')?.y ?? 180) - 180))))
    busy.go(120_000, () => (far.busy = Math.max(far.busy, Math.hypot((busy.world.bodies.get('a')?.x ?? 860) - 860, (busy.world.bodies.get('a')?.y ?? 180) - 180))))
    asleep.go(120_000)
    expect(far.idle).toBeGreaterThan(far.busy)
    expect(far.busy).toBeLessThan(260)
    expect([body(asleep, 'a').x, body(asleep, 'a').y]).toEqual([860, 180])
  })
})

describe('standing still', () => {
  it('stays where it is while it is wanted, and a while after', () => {
    const r = run([walker('a', { stays: true })])
    r.go(30_000)
    expect([body(r, 'a').x, body(r, 'a').y, body(r, 'a').stayed]).toEqual([860, 180, true])
    r.input.walkers = [walker('a')]
    r.go(2000)
    expect([body(r, 'a').x, body(r, 'a').y, body(r, 'a').stayed]).toEqual([860, 180, false])
  })

  it('hurries to the place where its bubble has room', () => {
    const r = run([walker('a', { stays: true, post: { x: 500, y: 300 } })])
    r.go(1200)
    const b = body(r, 'a')
    expect([Math.round(b.x), Math.round(b.y)]).toEqual([500, 300])
    // Faster than any stroll, and without the hop of a walk.
    expect(Math.hypot(860 - 500, 180 - 300) / 1.2).toBeLessThan(HURRY_PX_S)
    expect(b.walking).toBe(false)
  })

  it('goes with the drag, inside the bounds, and stays where it was dropped', () => {
    const r = run([walker('a', { held: { x: 300, y: 400 } })])
    r.go(16)
    expect([body(r, 'a').x, body(r, 'a').y]).toEqual([300, 400])
    r.input.walkers = [walker('a', { held: { x: -50, y: 9000 } })]
    r.go(16)
    expect([body(r, 'a').x, body(r, 'a').y]).toEqual([BOUNDS.left, BOUNDS.bottom])
    r.input.walkers = [walker('a')]
    r.go(10_000)
    expect([body(r, 'a').x, body(r, 'a').y]).toEqual([BOUNDS.left, BOUNDS.bottom])
  })
})

describe('meeting', () => {
  it('one visits another: it greets, the host answers, they look at each other, and part', () => {
    const r = run([walker('a'), walker('b')], 11)
    let met: { gap: number; looksA: string | null; looksB: string | null } | null = null
    let fastest = 0
    let last = new Map<string, Point>()
    r.go(180_000, () => {
      const a = r.world.bodies.get('a')
      const b = r.world.bodies.get('b')
      if (!a || !b) return
      for (const x of [a, b]) {
        const was = last.get(x.id)
        if (was && x.mode === 'visit') fastest = Math.max(fastest, Math.hypot(x.x - was.x, x.y - was.y) / 0.016)
        last.set(x.id, { x: x.x, y: x.y })
      }
      if (!met && a.mode === 'meet' && b.mode === 'meet') met = { gap: Math.hypot(a.x - b.x, a.y - b.y), looksA: a.looks, looksB: b.looks }
    })
    expect(met).not.toBeNull()
    const m = met as unknown as { gap: number; looksA: string | null; looksB: string | null }
    expect(m.gap).toBeGreaterThan(MEET_GAP_PX - 6)
    expect(m.gap).toBeLessThan(MEET_GAP_PX + 6)
    expect([m.looksA, m.looksB]).toEqual(['b', 'a'])
    // Its own pace is the visit's; stepping round its host on the way may add to it for a moment, but it never jumps.
    expect(fastest).toBeLessThanOrEqual(VISIT_PX_S * 2 + 125)
    const kinds = r.cues.map((c) => c.kind)
    const first = kinds.indexOf('greet')
    expect(first).toBeGreaterThanOrEqual(0)
    expect(kinds.indexOf('answer')).toBeGreaterThan(first)
    const greet = r.cues[first]
    const answer = r.cues[kinds.indexOf('answer')]
    expect([greet.toward, answer.id, answer.toward]).toEqual([answer.id, greet.toward, greet.id])
    // They do not stay together for ever.
    expect(['rest', 'walk', 'visit', 'meet']).toContain(body(r, 'a').mode)
    expect(r.cues.filter((c) => c.kind === 'greet').length).toBeLessThan(8)
  })

  it('a host on a stroll stops and looks when its guest comes near', () => {
    const r = run([walker('a'), walker('b')], 11)
    let waited = false
    r.go(180_000, () => {
      const a = r.world.bodies.get('a')
      const b = r.world.bodies.get('b')
      if (!a || !b) return
      for (const [guest, host] of [[a, b], [b, a]] as const) {
        if (guest.mode === 'visit' && guest.partner === host.id && Math.hypot(guest.x - host.x, guest.y - host.y) < 140) {
          expect(host.mode).toBe('rest')
          expect(host.looks).toBe(guest.id)
          waited = true
        }
      }
    })
    expect(waited).toBe(true)
  })

  it('comes to the host’s other side when one that stands is in the way, and calls the visit off when both sides are taken', () => {
    const stay = (id: string) => walker(id, { stays: true })
    /** A world in which `guest` is on its way to visit `host`. Nothing is quiet, so the host stays where the test put it. */
    const visiting = (others: Walker[], places: Record<string, Point>) => {
      const r = run([walker('guest'), walker('host'), ...others], 5, { spawn: (id) => places[id], quiet: () => false })
      attend(r.world, r.input)
      const g = body(r, 'guest')
      g.mode = 'visit'
      g.partner = 'host'
      g.guest = true
      return r
    }
    // The host rests between two that stand still: nobody can stand beside it. The visit is off at once, nobody greets,
    // and the host is not kept waiting.
    const hemmed = visiting([stay('left'), stay('right')], { host: { x: 500, y: 300 }, left: { x: 440, y: 300 }, right: { x: 560, y: 300 }, guest: { x: 380, y: 400 } })
    hemmed.go(64)
    expect([body(hemmed, 'guest').mode, body(hemmed, 'guest').partner]).toEqual(['rest', null])
    expect(hemmed.cues).toEqual([])
    expect(body(hemmed, 'host').looks).toBeNull()

    // With only the left side taken, the guest that comes from the left goes round and meets its host on the right.
    const one = visiting([stay('left')], { host: { x: 500, y: 300 }, left: { x: 440, y: 300 }, guest: { x: 250, y: 300 } })
    let met: Point | null = null
    one.go(40_000, () => {
      const g = one.world.bodies.get('guest')
      if (!met && g?.mode === 'meet') met = { x: g.x, y: g.y }
    })
    expect(met).not.toBeNull()
    expect((met as unknown as Point).x).toBeGreaterThan(body(one, 'host').x)
    expect([body(one, 'left').x, body(one, 'left').y]).toEqual([440, 300])
    expect(one.cues.map((c) => c.kind).slice(0, 2)).toEqual(['greet', 'answer'])
  })

  it('calls a visit off, and ends a meeting, when one of the two gets work to do', () => {
    const places: Record<string, Point> = { host: { x: 700, y: 300 }, guest: { x: 250, y: 300 } }
    const setUp = () => {
      const r = run([walker('guest'), walker('host')], 5, { spawn: (id) => places[id], quiet: () => false })
      attend(r.world, r.input)
      const g = body(r, 'guest')
      g.mode = 'visit'
      g.partner = 'host'
      g.guest = true
      return r
    }
    // On the way: the guest's session starts to work.
    const onTheWay = setUp()
    onTheWay.go(1600)
    expect(body(onTheWay, 'guest').mode).toBe('visit')
    onTheWay.input.walkers = [walker('guest', { busy: true }), walker('host')]
    onTheWay.go(64)
    expect([body(onTheWay, 'guest').mode, body(onTheWay, 'guest').partner]).toEqual(['rest', null])
    onTheWay.go(20_000)
    expect(onTheWay.cues).toEqual([])
    // While they stand together: the host's session starts to work.
    const together = setUp()
    together.go(20_000, (i) => {
      if (together.world.bodies.get('guest')?.mode === 'meet') i.walkers = [walker('guest'), walker('host', { busy: true })]
    })
    expect(together.cues.some((c) => c.kind === 'greet')).toBe(true)
    expect([body(together, 'guest').mode === 'meet', body(together, 'host').mode === 'meet']).toEqual([false, false])
  })

  it('visits a host only where it stands in nobody’s way', () => {
    // Nothing is quiet. The host is far from its home (the test put it there): nobody comes to it.
    const r = run([walker('guest'), walker('host')], 5, { quiet: () => false })
    attend(r.world, r.input)
    const host = body(r, 'host')
    host.x = 300
    host.y = 500
    r.go(200_000, () => {
      const h = r.world.bodies.get('host')
      // Kept away from home, as if it had just stopped there.
      if (h && h.mode === 'rest') {
        h.x = 300
        h.y = 500
        h.until = r.now + 5000
      }
      expect(r.world.bodies.get('guest')?.partner ?? null).toBeNull()
    })
    // At home it is visited, quiet or not.
    const home = run([walker('guest'), walker('host')], 5, { quiet: () => false })
    home.go(200_000)
    expect(home.cues.some((c) => c.kind === 'greet')).toBe(true)
  })

  it('visits nobody who must stand still, sleeps or works, and nobody visits while it works', () => {
    for (const over of [{ stays: true }, { asleep: true }, { busy: true }] as Partial<Walker>[]) {
      const r = run([walker('a'), walker('b', over)], 5)
      r.go(240_000)
      expect(r.cues.filter((c) => c.kind === 'greet')).toEqual([])
    }
    const both = run([walker('a', { busy: true }), walker('b', { busy: true })], 5)
    both.go(240_000)
    expect(both.cues).toEqual([])
  })

  it('ends a meeting when one of the two is wanted or leaves', () => {
    const r = run([walker('a'), walker('b')], 11)
    r.go(180_000, (i) => {
      const a = r.world.bodies.get('a')
      if (a?.mode === 'meet') i.walkers = [walker('a'), walker('b', { stays: true })]
    })
    // b stands for its own reason now, a is by itself again.
    expect(body(r, 'b').partner).toBeNull()
    r.go(3000)
    expect(body(r, 'a').mode === 'meet').toBe(false)
    expect(body(r, 'a').partner === 'b' && body(r, 'a').mode === 'meet').toBe(false)

    const gone = run([walker('a'), walker('b')], 11)
    gone.go(180_000, (i) => {
      const a = gone.world.bodies.get('a')
      if (a?.mode === 'meet') i.walkers = [walker('a')]
    })
    gone.go(3000)
    expect([...gone.world.bodies.keys()]).toEqual(['a'])
    expect(body(gone, 'a').mode === 'meet').toBe(false)
  })
})

describe('keeping apart', () => {
  it('two on the same spot step apart; one that must stand still is not moved', () => {
    const same = run([walker('a'), walker('b')], 2, { spawn: () => ({ x: 500, y: 300 }) })
    same.go(3000, (i) => (i.pointer = { x: 500, y: 300 - SIZE / 2 }))
    const a = body(same, 'a')
    const b = body(same, 'b')
    expect(Math.hypot(a.x - b.x, a.y - b.y)).toBeGreaterThanOrEqual(APART_PX - 1)

    const fixed = run([walker('a', { stays: true }), walker('b')], 2, { spawn: () => ({ x: 500, y: 300 }) })
    fixed.go(3000, (i) => (i.pointer = { x: 500, y: 300 - SIZE / 2 }))
    expect([body(fixed, 'a').x, body(fixed, 'a').y]).toEqual([500, 300])
    expect(Math.hypot(body(fixed, 'b').x - 500, body(fixed, 'b').y - 300)).toBeGreaterThanOrEqual(APART_PX - 1)
  })

  it('leaves what is in the way, and does not walk into it', () => {
    const bubble: Box = { left: 400, top: 200, right: 700, bottom: 420 }
    const r = run([walker('a')], 4, { spawn: () => ({ x: 550, y: 330 }), obstacles: [bubble] })
    const clear = (p: Point) => !(p.x + SIZE / 2 > bubble.left && p.x - SIZE / 2 < bubble.right && p.y > bubble.top && p.y - SIZE < bubble.bottom)
    r.go(4000)
    expect(clear(body(r, 'a'))).toBe(true)
    r.go(120_000, () => {
      const b = r.world.bodies.get('a')
      if (b && b.mode === 'rest' && r.now > 8000) expect(clear(b)).toBe(true)
    })
  })

  it('rests where it is in nobody’s way, when there is such a place', () => {
    // The left half of the window is full of things to press; the right half is empty.
    const quiet = (p: Point) => p.x >= 500
    const r = run([walker('a')], 9, { spawn: () => ({ x: 300, y: 300 }), quiet })
    let rests = 0
    let inTheWay = 0
    let was = 'rest'
    r.go(400_000, () => {
      const b = r.world.bodies.get('a')
      if (!b) return
      // Counted when a walk ends.
      if (was === 'walk' && b.mode === 'rest') {
        rests++
        if (!quiet(b)) inTheWay++
      }
      was = b.mode
    })
    expect(rests).toBeGreaterThan(8)
    expect(inTheWay).toBeLessThanOrEqual(1)
    expect(body(r, 'a').x).toBeGreaterThanOrEqual(500)
  })

  it('goes home and stays there when nothing is quiet (an editor fills the window)', () => {
    // At home already: it does not pace about on top of what the user works in.
    const home = run([walker('a')], 9, { quiet: () => false })
    home.go(120_000)
    expect([body(home, 'a').x, body(home, 'a').y]).toEqual([860, 180])

    // Away from home when everything becomes something to press: back it goes, and rests as long as at any quiet place.
    let busy = false
    const away = run([walker('a')], 9, { quiet: () => !busy })
    away.go(60_000)
    const before = { x: body(away, 'a').x, y: body(away, 'a').y }
    expect(Math.hypot(before.x - 860, before.y - 180)).toBeGreaterThan(HOME_PX)
    busy = true
    away.go(120_000)
    expect(Math.hypot(body(away, 'a').x - 860, body(away, 'a').y - 180)).toBeLessThanOrEqual(HOME_PX)
    const at = { x: body(away, 'a').x, y: body(away, 'a').y }
    away.go(60_000)
    expect([body(away, 'a').x, body(away, 'a').y]).toEqual([at.x, at.y])

    // Where the user dropped it is its home from then on.
    const dropped = run([walker('a', { held: { x: 300, y: 500 } })], 9, { quiet: () => false })
    dropped.go(32)
    dropped.input.walkers = [walker('a')]
    dropped.go(120_000)
    expect([body(dropped, 'a').x, body(dropped, 'a').y]).toEqual([300, 500])
    expect(body(dropped, 'a').home).toEqual({ x: 300, y: 500 })
  })

  it('gives up a walk that something standing in its way keeps it from ending', () => {
    const r = run([walker('a')], 7)
    let to: Point | null = null
    r.go(60_000, (i) => {
      const a = r.world.bodies.get('a')
      // Once it walks, another avatar comes to stand exactly where the walk leads.
      if (!to && a?.mode === 'walk' && a.to) {
        to = { x: a.to.x, y: a.to.y }
        const target = to
        i.walkers = [walker('a'), walker('wall', { stays: true })]
        i.spawn = (id, index) => (id === 'wall' ? target : column(id, index))
      }
    })
    expect(to).not.toBeNull()
    const a = body(r, 'a')
    const wall = body(r, 'wall')
    // The one that stands was not moved, and the walker did not go on walking against it for ever.
    expect([wall.x, wall.y]).toEqual([(to as unknown as Point).x, (to as unknown as Point).y])
    let walkingAgainstIt = 0
    r.go(20_000, () => {
      const b = r.world.bodies.get('a')
      if (b?.walking && b.to && Math.hypot(b.to.x - wall.x, b.to.y - wall.y) < APART_PX) walkingAgainstIt++
    })
    expect(walkingAgainstIt).toBe(0)
    expect(Math.hypot(a.x - wall.x, a.y - wall.y)).toBeGreaterThanOrEqual(APART_PX - 1)
  })

  it('never sets out for a place beside one that stands still', () => {
    const r = run([walker('a'), walker('wall', { stays: true })], 3, { spawn: (id) => (id === 'wall' ? { x: 500, y: 350 } : { x: 520, y: 420 }) })
    r.go(240_000, () => {
      const a = r.world.bodies.get('a')
      if (a?.mode === 'walk' && a.to) expect(Math.hypot(a.to.x - 500, a.to.y - 350)).toBeGreaterThanOrEqual(APART_PX)
    })
  })

  it('is not in the way of itself: its own figure and bubble move nobody who owns them', () => {
    // The pointer rests on a figure: it is the active one, and its own box is among what is in the way.
    const own = { left: 472, top: 244, right: 528, bottom: 300, of: 'a' }
    const r = run([walker('a', { stays: true })], 4, { spawn: () => ({ x: 500, y: 300 }), obstacles: [own] })
    r.go(5000)
    expect([body(r, 'a').x, body(r, 'a').y]).toEqual([500, 300])
    // Another one that only stands there does give way to it.
    const other = run([walker('a', { stays: true }), walker('b', { stays: true })], 4, { spawn: (id) => (id === 'a' ? { x: 500, y: 300 } : { x: 510, y: 300 }), obstacles: [own] })
    other.go(5000)
    expect([body(other, 'a').x, body(other, 'a').y]).toEqual([500, 300])
    expect(Math.abs(body(other, 'b').x - 500)).toBeGreaterThanOrEqual(56)
  })

  it('a sleeping one gives way to a bubble too', () => {
    const bubble: Box = { left: 400, top: 200, right: 700, bottom: 420 }
    const inside = (p: Point) => p.x + SIZE / 2 > bubble.left && p.x - SIZE / 2 < bubble.right && p.y > bubble.top && p.y - SIZE < bubble.bottom
    const r = run([walker('sleeper', { asleep: true })], 4, { spawn: () => ({ x: 450, y: 330 }), obstacles: [bubble] })
    r.go(4000)
    expect(inside(body(r, 'sleeper'))).toBe(false)
  })

  it('one that only stands gives way to a bubble that opens over it; one at its post does not', () => {
    const bubble: Box = { left: 400, top: 200, right: 700, bottom: 420 }
    const inside = (p: Point) => p.x + SIZE / 2 > bubble.left && p.x - SIZE / 2 < bubble.right && p.y > bubble.top && p.y - SIZE < bubble.bottom
    const r = run([walker('asks', { stays: true }), walker('posted', { stays: true, post: { x: 600, y: 330 } })], 4, { spawn: (id) => (id === 'asks' ? { x: 450, y: 330 } : { x: 600, y: 330 }), obstacles: [bubble] })
    r.go(4000)
    expect(inside(body(r, 'asks'))).toBe(false)
    expect([body(r, 'posted').x, body(r, 'posted').y]).toEqual([600, 330])
  })

  it('follows the bounds when the window becomes smaller', () => {
    const r = run([walker('a')], 4)
    r.go(16)
    const small: Box = { left: 100, top: 100, right: 400, bottom: 300 }
    r.input.bounds = small
    r.go(16)
    expect(inBounds(body(r, 'a'), small)).toBe(true)
  })
})

describe('home', () => {
  it('follows the column when the area shrinks and grows, so that nobody paces where nothing is quiet', () => {
    // An editor fills the area: nothing is quiet. A tool window opens on the right and takes 380 px.
    const r = run([walker('a'), walker('b')], 9, { quiet: () => false })
    r.go(20_000)
    const small: Box = { ...BOUNDS, right: BOUNDS.right - 380 }
    r.input.bounds = small
    r.go(30_000)
    // They stand in the column at the new edge, and stay.
    for (const id of ['a', 'b']) expect(Math.hypot(body(r, id).x - body(r, id).home.x, body(r, id).y - body(r, id).home.y)).toBeLessThanOrEqual(HOME_PX)
    expect(body(r, 'a').home.x).toBe(small.right)
    let walks = 0
    let was = new Map<string, string>()
    r.go(180_000, () => {
      for (const id of ['a', 'b']) {
        const b = r.world.bodies.get(id)
        if (b && was.get(id) === 'rest' && b.mode === 'walk') walks++
        if (b) was.set(id, b.mode)
      }
    })
    expect(walks).toBe(0)
    // The tool window closes: the column, and they with it, are where they were.
    r.input.bounds = BOUNDS
    r.go(120_000)
    expect(body(r, 'a').home).toEqual({ x: 860, y: 180 })
    for (const id of ['a', 'b']) expect(Math.hypot(body(r, id).x - body(r, id).home.x, body(r, id).y - body(r, id).home.y)).toBeLessThanOrEqual(HOME_PX)
  })

  it('keeps a figure the user dropped where it was dropped, inside the area as it is', () => {
    const r = run([walker('a', { held: { x: 800, y: 500 } })], 9, { quiet: () => false })
    r.go(32)
    r.input.walkers = [walker('a')]
    r.go(30_000)
    expect([body(r, 'a').x, body(r, 'a').y, body(r, 'a').dropped]).toEqual([800, 500, true])
    r.input.bounds = { ...BOUNDS, right: 600 }
    let walks = 0
    let was = 'rest'
    r.go(120_000, () => {
      const b = r.world.bodies.get('a')
      if (b && was === 'rest' && b.mode === 'walk') walks++
      if (b) was = b.mode
    })
    expect(body(r, 'a').x).toBe(600)
    expect(walks).toBe(0)
    // Its home is still where the user put it: it goes back when there is room again.
    r.input.bounds = BOUNDS
    r.go(120_000)
    expect(Math.hypot(body(r, 'a').x - 800, body(r, 'a').y - 500)).toBeLessThanOrEqual(HOME_PX)
  })
})

describe('held back', () => {
  it('stands instead of walking on the spot, and gives the walk up after a second', () => {
    const r = run([walker('a')], 7)
    let to: Point | null = null
    let blockedAt: number | null = null
    let stoodAt: number | null = null
    let gaveUpAt: number | null = null
    r.go(90_000, (i) => {
      const a = r.world.bodies.get('a')
      if (!a) return
      if (!to && a.mode === 'walk' && a.to && Math.hypot(a.to.x - a.x, a.to.y - a.y) > 120) {
        // Another avatar comes to stand right in front of it, on its way.
        to = { x: a.to.x, y: a.to.y }
        const d = Math.hypot(a.to.x - a.x, a.to.y - a.y)
        const wall = { x: a.x + ((a.to.x - a.x) / d) * 70, y: a.y + ((a.to.y - a.y) / d) * 70 }
        // A row of them, so that there is no way round.
        const row = [-2, -1, 0, 1, 2].map((n) => ({ x: wall.x - ((a.to!.y - a.y) / d) * n * 50, y: wall.y + ((a.to!.x - a.x) / d) * n * 50 }))
        i.walkers = [walker('a'), ...row.map((_, n) => walker(`w${n}`, { stays: true }))]
        i.spawn = (id, index) => (id.startsWith('w') ? row[Number(id.slice(1))] : column(id, index))
        blockedAt = r.now
      }
      if (blockedAt !== null && stoodAt === null && a.mode === 'walk' && !a.walking && r.now - blockedAt > 600) stoodAt = r.now
      if (blockedAt !== null && gaveUpAt === null && a.mode !== 'walk' && r.now - blockedAt > 600) gaveUpAt = r.now
    })
    expect(to).not.toBeNull()
    expect(gaveUpAt).not.toBeNull()
    // It stood (no hop) before it gave up, and gave up about a second after it was held back, not four.
    expect(stoodAt).not.toBeNull()
    expect((gaveUpAt as unknown as number) - (stoodAt as unknown as number)).toBeLessThan(HELD_BACK_MS + 400)
  })

  it('never sets out for a place another one is walking to', () => {
    const r = run([walker('a'), walker('b'), walker('c')], 8)
    r.go(400_000, () => {
      const walking = [...r.world.bodies.values()].filter((b) => b.mode === 'walk' && b.to)
      for (let m = 0; m < walking.length; m++) for (let n = m + 1; n < walking.length; n++) expect(Math.hypot(walking[m].to!.x - walking[n].to!.x, walking[m].to!.y - walking[n].to!.y)).toBeGreaterThanOrEqual(APART_PX - 1e-6)
    })
  })

  it('goes round a bubble that lies across its way', () => {
    const bubble: Box = { left: 500, top: 250, right: 560, bottom: 560 }
    const r = run([walker('a')], 4, { spawn: () => ({ x: 300, y: 400 }), obstacles: [bubble] })
    attend(r.world, r.input)
    const a = body(r, 'a')
    a.mode = 'walk'
    a.to = { x: 800, y: 400 }
    a.best = Number.POSITIVE_INFINITY
    a.bestAt = 0
    let arrived: number | null = null
    r.go(60_000, () => {
      const b = r.world.bodies.get('a')
      if (arrived === null && b && Math.hypot(b.x - 800, b.y - 400) < 3) arrived = r.now
    })
    expect(arrived).not.toBeNull()
    expect(arrived as unknown as number).toBeLessThan(45_000)
  })
})

describe('the pointer', () => {
  it('is waited for where the figure stands on nothing, and not where it stands on something to press', () => {
    // On a quiet place a pointer that comes near stops the walk.
    const quietPlace = run([walker('a')], 7)
    quietPlace.go(40_000)
    quietPlace.go(3000, (i) => (i.pointer = { x: body(quietPlace, 'a').x, y: body(quietPlace, 'a').y - SIZE / 2 }))
    expect(body(quietPlace, 'a').walking).toBe(false)

    // On top of a button it walks on, pointer or not: it is not what the pointer came for.
    const onButton = run([walker('a')], 7, { spawn: () => ({ x: 300, y: 300 }), quiet: (p) => p.x >= 500 })
    let walked = false
    onButton.go(20_000, (i) => {
      const b = onButton.world.bodies.get('a')
      if (!b) return
      i.pointer = { x: b.x, y: b.y - SIZE / 2 }
      if (b.walking) walked = true
    })
    expect(walked).toBe(true)
    expect(body(onButton, 'a').x).toBeGreaterThanOrEqual(500)
  })

  it('keeps a figure it rests on where it stands on nothing, and lets one on a button go', () => {
    const pinned = run([walker('a', { pointed: true })], 7)
    pinned.go(60_000)
    expect([body(pinned, 'a').x, body(pinned, 'a').y, body(pinned, 'a').stayed]).toEqual([860, 180, true])
    const let_go = run([walker('a', { pointed: true })], 7, { spawn: () => ({ x: 300, y: 300 }), quiet: (p) => p.x >= 500 })
    let_go.go(60_000)
    expect(body(let_go, 'a').x).toBeGreaterThanOrEqual(500)
  })
})

describe('the others', () => {
  it('those near one that speaks look at it, those far away do not', () => {
    const places: Record<string, Point> = { speaker: { x: 200, y: 300 }, near: { x: 200 + AUDIENCE_PX - 40, y: 300 }, far: { x: 200 + AUDIENCE_PX + 200, y: 300 } }
    const stay = (id: string) => walker(id, { stays: true })
    const r = run([stay('speaker'), stay('near'), stay('far')], 1, { spawn: (id) => places[id], speaker: 'speaker' })
    r.go(32)
    expect([body(r, 'near').looks, body(r, 'far').looks, body(r, 'speaker').looks]).toEqual(['speaker', null, null])
    r.input.speaker = null
    r.go(32)
    expect(body(r, 'near').looks).toBeNull()
  })

  it('cheer for one that finished: those near and free, once', () => {
    const places: Record<string, Point> = { done: { x: 300, y: 300 }, friend: { x: 380, y: 300 }, wanted: { x: 300, y: 380 }, far: { x: 300 + AUDIENCE_PX + 100, y: 550 } }
    const r = run([walker('done', { stays: true }), walker('friend'), walker('wanted', { stays: true }), walker('far')], 1, { spawn: (id) => places[id] })
    r.go(16)
    r.input.finished = ['done']
    r.go(16)
    r.input.finished = []
    r.go(160)
    expect(r.cues).toEqual([{ id: 'friend', kind: 'cheer', toward: 'done' }])
  })
})

describe('where a bubble has room', () => {
  const work: Box = { left: 0, top: 0, right: 1000, bottom: 700 }
  const post = (x: number, y = 400) => postFor({ x, y }, work, 132, 312, 8)

  it('stays where it is when the bubble fits, and opens it towards the middle', () => {
    expect(post(500)).toEqual({ at: { x: 500, y: 400 }, tail: 'right' })
    expect(post(450)).toEqual({ at: { x: 450, y: 400 }, tail: 'left' })
    // Near the right edge the bubble opens to its left (its tail on the right), and the other way round.
    expect(post(900)).toEqual({ at: { x: 900, y: 400 }, tail: 'right' })
    expect(post(100)).toEqual({ at: { x: 100, y: 400 }, tail: 'left' })
  })

  it('keeps the large figure inside the window', () => {
    expect(post(990, 20)).toEqual({ at: { x: 1000 - 8 - 66, y: 8 + 132 }, tail: 'right' })
    expect(post(-20, 900)).toEqual({ at: { x: 8 + 66, y: 700 - 8 }, tail: 'left' })
  })

  it('steps to the nearer place with room in a window too narrow to have it everywhere', () => {
    const narrow: Box = { left: 0, top: 0, right: 520, bottom: 700 }
    // The bubble needs the figure at 386 or more (to open on its left), or at 134 or less.
    expect(postFor({ x: 300, y: 400 }, narrow, 132, 312, 8)).toEqual({ at: { x: 386, y: 400 }, tail: 'right' })
    expect(postFor({ x: 200, y: 400 }, narrow, 132, 312, 8)).toEqual({ at: { x: 134, y: 400 }, tail: 'left' })
    const tiny: Box = { left: 0, top: 0, right: 300, bottom: 700 }
    expect(postFor({ x: 100, y: 400 }, tiny, 132, 312, 8)).toEqual({ at: { x: 300 - 8 - 66, y: 400 }, tail: 'right' })
  })
})

describe('rooms for bubbles', () => {
  const work: Box = { left: 0, top: 0, right: 1600, bottom: 800 }
  const [FIGURE, REACH, EDGE, HEIGHT] = [132, 312, 8, 240]
  const overlap = (a: Box, b: Box) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
  const room = (p: Post) => claimOf(p, FIGURE, REACH, HEIGHT)

  it('takes the figure and the bubble on the side it opens', () => {
    expect(claimOf({ at: { x: 600, y: 400 }, tail: 'left' }, FIGURE, REACH, HEIGHT)).toEqual({ left: 534, top: 268, right: 978, bottom: 508 })
    expect(claimOf({ at: { x: 600, y: 400 }, tail: 'right' }, FIGURE, REACH, HEIGHT)).toEqual({ left: 222, top: 268, right: 666, bottom: 508 })
    // Never less high than the figure itself.
    expect(claimOf({ at: { x: 600, y: 400 }, tail: 'left' }, FIGURE, REACH, 40).bottom).toBe(400)
  })

  it('leaves a wish alone that covers nobody', () => {
    const wish: Post = { at: { x: 600, y: 400 }, tail: 'left' }
    expect(placePost(wish, [], work, FIGURE, REACH, EDGE, HEIGHT)).toBe(wish)
    expect(placePost(wish, [room({ at: { x: 600, y: 760 }, tail: 'left' })], work, FIGURE, REACH, EDGE, HEIGHT)).toBe(wish)
  })

  it('keeps two bubbles that face each other apart, though their figures are far from each other', () => {
    // A stands at 600 and opens to the right, B at 1100 and opens to the left: the bubbles would lie on each other.
    const a: Post = postFor({ x: 600, y: 400 }, work, FIGURE, REACH, EDGE)
    const wish: Post = postFor({ x: 1100, y: 420 }, work, FIGURE, REACH, EDGE)
    expect([a.tail, wish.tail]).toEqual(['left', 'right'])
    expect(overlap(room(a), room(wish))).toBe(true)
    const b = placePost(wish, [room(a)], work, FIGURE, REACH, EDGE, HEIGHT)
    expect(overlap(room(a), room(b))).toBe(false)
    // At the same place, at another height, with a gap between them; and inside the area.
    expect(b.at.x).toBe(wish.at.x)
    expect(Math.min(Math.abs(room(b).top - room(a).bottom), Math.abs(room(a).top - room(b).bottom))).toBeGreaterThanOrEqual(POST_GAP_PX)
    expect(b.at.y).toBeGreaterThanOrEqual(EDGE + FIGURE)
    expect(b.at.y).toBeLessThanOrEqual(work.bottom - EDGE)
  })

  it('steps beside what is in the way when no height is free, and stays where it wished when nothing is', () => {
    const low: Box = { left: 0, top: 0, right: 1600, bottom: 330 }
    const a: Post = postFor({ x: 600, y: 300 }, low, FIGURE, REACH, EDGE)
    const wish: Post = postFor({ x: 680, y: 310 }, low, FIGURE, REACH, EDGE)
    const b = placePost(wish, [room(a)], low, FIGURE, REACH, EDGE, HEIGHT)
    expect(overlap(room(a), room(b))).toBe(false)
    expect(b.at.x).not.toBe(wish.at.x)
    // Rooms everywhere: there is nowhere to go, and it stays.
    const full: Box = { left: -5000, top: -5000, right: 5000, bottom: 5000 }
    expect(placePost(wish, [full], low, FIGURE, REACH, EDGE, HEIGHT)).toBe(wish)
  })
})

describe('small things', () => {
  it('looks under the middle of a figure and four points of its box', () => {
    const points = samplePoints({ x: 100, y: 200 }, 50)
    expect(points).toHaveLength(5)
    expect(points[0]).toEqual({ x: 100, y: 175 })
    for (const p of points) {
      expect(p.x).toBeGreaterThanOrEqual(75)
      expect(p.x).toBeLessThanOrEqual(125)
      expect(p.y).toBeGreaterThanOrEqual(150)
      expect(p.y).toBeLessThanOrEqual(200)
    }
    // Both sides, near the feet and near the head.
    expect(new Set(points.map((p) => `${Math.sign(p.x - 100)}${p.y < 175 ? 'h' : p.y > 175 ? 'f' : 'm'}`)).size).toBe(5)
  })

  it('gives the feet room for the whole figure', () => {
    expect(feetBounds({ left: 0, top: 0, right: 1000, bottom: 700 }, 56, 8, 20)).toEqual({ left: 48, top: 64, right: 952, bottom: 692 })
  })

  it('looks towards another, further the further away it is', () => {
    expect(gazeToward({ x: 0, y: 0 }, { x: 260, y: 0 }, 260)).toEqual([1, 0])
    expect(gazeToward({ x: 0, y: 0 }, { x: 0, y: 130 }, 260)).toEqual([0, 0.5])
    expect(gazeToward({ x: 5, y: 5 }, { x: 5, y: 5 }, 260)).toEqual([0, 0])
  })
})
