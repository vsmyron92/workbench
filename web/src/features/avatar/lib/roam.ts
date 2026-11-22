// Where the avatars walk. While they have nothing to say they stroll about the workspace
// window, rest, visit one another and greet; one that speaks, asks or is wanted by the user
// stands still, and the others near it look its way.
//
// Each avatar has a place: the point between its feet, in viewport pixels. One step takes
// the facts of the moment (who must stand still, where the pointer is, what is in the way)
// and the time that passed, and moves every place a little. What the figures should do
// besides (wave at a visitor, cheer for one that finished) comes out as cues.
//
// Pure: no DOM, no clock, no randomness of its own. The world is changed in place.

/** A point in viewport pixels. */
export interface Point {
  x: number
  y: number
}

/** A rectangle in viewport pixels. */
export interface Box {
  left: number
  top: number
  right: number
  bottom: number
}

/** Something the walkers keep out of. `of`: the avatar it belongs to (its own large figure, its own bubble), which it is not in the way of. */
export interface Obstacle extends Box {
  of?: string
}

// ---------------------------------------------------------------- how they move

/** A stroll, a walk to visit, and the hurry to a place where a bubble has room: pixels a second. */
export const STROLL_PX_S = 34
export const VISIT_PX_S = 58
export const HURRY_PX_S = 520
/** How long one rests between walks: an idle one, and one whose session works. */
export const REST_MS: readonly [number, number] = [3500, 12_000]
export const BUSY_REST_MS: readonly [number, number] = [9000, 26_000]
/** And one that came to stand where it is in the way of something the user presses. */
export const IN_THE_WAY_REST_MS: readonly [number, number] = [800, 2200]
/** This near to its home (its place in the column, or where the user put it) a figure is at home: it may stay, whatever is under it. */
export const HOME_PX = 90
/** A walk or a visit that came no nearer to its end for this long is given up (going round another avatar takes a few seconds). */
export const STUCK_MS = 4000
/** And one that did not really move for this long is given up at once: something holds it back. */
export const HELD_BACK_MS = 1100
/** A walker that is held back for this long stands (its hop ends) until it moves again. */
export const STAND_AFTER_MS = 150
/** A step that comes this long after the last one comes after a pause (a hidden tab): nobody sets off at the same moment as the others. */
export const LONG_PAUSE_MS = 2000
/** What stays free between the rooms of two avatars that each need one. */
export const POST_GAP_PX = 12
/** How far a walk leads from where it begins. */
export const REACH_PX = 260
export const BUSY_REACH_PX = 70
/** The first visit comes soon; then there is this long between two visits of the same avatar. */
export const FIRST_VISIT_MS: readonly [number, number] = [6000, 18_000]
export const VISIT_EVERY_MS: readonly [number, number] = [25_000, 70_000]
/** Of the times it could visit, it does so this often; otherwise it strolls. */
export const VISIT_CHANCE = 0.45
/** Two that meet stand this far apart, and stay this long. */
export const MEET_GAP_PX = 62
export const MEET_MS: readonly [number, number] = [3500, 6500]
/** The one that is visited answers the greeting after this. */
export const ANSWER_AFTER_MS = 450
/** A host that walks stops and waits when its guest is this near. */
export const NOTICE_PX = 150
/** Closer than this, two avatars step apart. */
export const APART_PX = 58
/** A pointer this near stops a walk: the figure can be pressed. */
export const NEAR_POINTER_PX = 64
/** After the pointer, a drag or a reason to stand still, it stays a while. */
export const AFTER_POINTER_MS = 1500
export const AFTER_DROP_MS = 15_000
export const AFTER_STAYING_MS = 2500
/** Those nearer than this to one that speaks look at it, and cheer when it finishes. */
export const AUDIENCE_PX = 420
/** At most this many cheer for one that finished. */
export const CHEERS = 3
/** One hop of the walk for this much way. */
export const STEP_PX = 22
/** What is kept free around something in the way. */
const CLEAR_PX = 6
/** The hop and the lean of a walk end over about this long. */
const SETTLE_MS = 120
/** A walk slows down this near to its end. */
const SLOW_WITHIN_PX = 24
/** Arrived. */
const AT_PX = 1.5
/** A step longer than this is taken as this: a tab that was hidden does not make them jump. */
export const MAX_STEP_MS = 100
/** As many cues wait to be taken, at most. */
const MAX_CUES = 32

// ---------------------------------------------------------------- the world

/** What the walk has to know of one avatar, each step. */
export interface Walker {
  id: string
  /**
   * It stands where it is: it speaks, asks, is the active one, is hovered or focused, its
   * session needs the user, or it holds a gesture.
   */
  stays: boolean
  /** Where it stands instead (a place where its bubble has room). Only with `stays`. */
  post: Point | null
  /** The user drags it: it is at this point. */
  held: Point | null
  /** Its session works: it paces a little and visits nobody. */
  busy: boolean
  /** Its session exited: it sleeps where it is. */
  asleep: boolean
  /**
   * The pointer rests on it and nothing else wants it. It waits where it stands in nobody's
   * way; on top of something the user presses it walks on.
   */
  pointed?: boolean
}

export type Mode = 'rest' | 'walk' | 'visit' | 'meet'

/** One avatar's place and what it is doing. */
export interface Body {
  id: string
  /** The point between its feet. */
  x: number
  y: number
  mode: Mode
  /**
   * Where it belongs: its place in the column they start from (it follows the column when the
   * area changes), or where the user dropped it. With nowhere quiet to go, it goes there.
   */
  home: Point
  /** The user dropped it where its home is now: the home no longer follows the column. */
  dropped: boolean
  /** Where its walk leads. */
  to: Point | null
  /** The nearest it came to the end of its walk or visit, and when: one that comes no nearer is given up. */
  best: number
  bestAt: number
  /** When its rest or its meeting ends. */
  until: number
  /** The avatar it walks to or stands with. */
  partner: string | null
  /** It came to its partner (it greets first); false: it was visited. */
  guest: boolean
  /** A cue that is still to come in this meeting, and when. */
  cueAt: number | null
  cueKind: CueKind | null
  /** The avatar it looks at: the one it meets, or one that speaks. */
  looks: string | null
  /** It moved by itself in the last step (a stroll, a visit): the figure walks. One that is held back stands. */
  walking: boolean
  /** How long it has tried to walk without really moving. */
  heldBack: number
  /** The way it walked since it last stood, for the phase of its hop. */
  walked: number
  /** The hop of its walk: 0 on the ground, up to 1. */
  hop: number
  /** How it leans into its walk: -1 (to the left) to 1. */
  lean: number
  /** No visit before this. */
  visitAfter: number
  /** It stood still for a reason in the last step. */
  stayed: boolean
}

export type CueKind = 'greet' | 'answer' | 'nod' | 'cheer'

/** Something a figure should do now: `greet` a host, `answer` a guest, `nod` in a talk, `cheer` for one that finished. */
export interface Cue {
  id: string
  kind: CueKind
  /** The avatar it is for. */
  toward: string
}

export interface World {
  bodies: Map<string, Body>
  /** What came up and was not taken yet: whoever shows the figures takes them out. */
  cues: Cue[]
}

export function newWorld(): World {
  return { bodies: new Map(), cues: [] }
}

export interface StepInput {
  /** A clock in milliseconds that only goes forwards. */
  now: number
  /** The time since the last step, in milliseconds. */
  dt: number
  /** Where feet may be. */
  bounds: Box
  /** The avatars on stage, in their order. */
  walkers: readonly Walker[]
  /** Where a new one appears: its place in the column they start from. */
  spawn: (id: string, index: number) => Point
  /** What is in the way: an open bubble, the app's own line beside a figure, the large figure. */
  obstacles: readonly Obstacle[]
  pointer: Point | null
  /** The height of a resting figure. */
  size: number
  /** The avatar whose line is presented now. */
  speaker: string | null
  /** Avatars whose session finished since the last step. */
  finished: readonly string[]
  /** A number from 0 (included) to 1 (excluded). */
  random: () => number
  /**
   * Whether a figure whose feet are at this point stands in nobody's way: nothing under it
   * that the user presses or types into. A walk ends at such a place where there is one, and
   * a figure that stands elsewhere moves on soon. Absent: every place is quiet.
   */
  quiet?: (at: Point) => boolean
}

// ---------------------------------------------------------------- small helpers

const between = (range: readonly [number, number], random: () => number): number => range[0] + (range[1] - range[0]) * random()
const clamp = (n: number, lo: number, hi: number): number => (hi < lo ? (lo + hi) / 2 : Math.min(Math.max(n, lo), hi))
const finite = (n: number, fallback: number): number => (Number.isFinite(n) ? n : fallback)
const distance = (a: Point, b: Point): number => Math.hypot(a.x - b.x, a.y - b.y)

/** A point brought inside a box. */
export function into(p: Point, box: Box): Point {
  return { x: clamp(finite(p.x, box.left), box.left, box.right), y: clamp(finite(p.y, box.top), box.top, box.bottom) }
}

/** The rectangle a figure of `size` covers when its feet are at `p`. */
function figureBox(p: Point, size: number): Box {
  return { left: p.x - size / 2, top: p.y - size, right: p.x + size / 2, bottom: p.y }
}

const overlaps = (a: Box, b: Box): boolean => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
const grown = (b: Box, by: number): Box => ({ left: b.left - by, top: b.top - by, right: b.right + by, bottom: b.bottom + by })

/** Whether a figure whose feet are at `p` would stand on something that is in the way. */
function blocked(p: Point, size: number, obstacles: readonly Box[]): boolean {
  const box = figureBox(p, size)
  return obstacles.some((o) => overlaps(box, grown(o, CLEAR_PX)))
}

/** Where feet may be inside a window's rectangle: the whole figure stays inside it. */
export function feetBounds(work: Box, size: number, edge: number, sideEdge: number): Box {
  return { left: work.left + sideEdge + size / 2, top: work.top + edge + size, right: work.right - sideEdge - size / 2, bottom: work.bottom - edge }
}

/** Where an avatar at `from` looks when it looks at `to`: a vector of length up to 1, as the engine's gaze takes it. */
export function gazeToward(from: Point, to: Point, full: number): readonly [number, number] {
  const dx = to.x - from.x
  const dy = to.y - from.y
  const far = Math.max(Math.hypot(dx, dy), full, 1)
  return [dx / far, dy / far]
}

// ---------------------------------------------------------------- where a bubble has room

export interface Post {
  /** Where the avatar stands while its bubble may be open. */
  at: Point
  /** The side of the bubble its tail is on: the bubble opens on the other side of the figure. */
  tail: 'left' | 'right'
}

/**
 * Where an avatar that is wanted stands, and on which side its bubble opens. It stays where
 * it is when the bubble has room on one side (towards the middle of the window when on
 * both); otherwise it steps sideways to the nearest place where it has. `figure`: the size
 * of the large figure; `reach`: the bubble's width and its gap to the figure.
 */
export function postFor(at: Point, work: Box, figure: number, reach: number, edge: number): Post {
  const half = figure / 2
  const lo = work.left + edge + half
  const hi = Math.max(lo, work.right - edge - half)
  /** The bubble on the left of the figure needs the figure this far right, and the other way round. */
  const leftFrom = work.left + edge + reach + half
  const rightUntil = work.right - edge - reach - half
  const y = clamp(finite(at.y, work.bottom), Math.min(work.top + edge + figure, work.bottom - edge), work.bottom - edge)
  let x = clamp(finite(at.x, hi), lo, hi)
  const canLeft = leftFrom <= hi
  const canRight = rightUntil >= lo
  const middle = (work.left + work.right) / 2
  let tail: 'left' | 'right'
  if (canLeft && canRight && x >= leftFrom && x <= rightUntil) {
    // Room on both sides: the bubble opens towards the middle of the window.
    tail = x < middle ? 'left' : 'right'
  } else if (canLeft && x >= leftFrom) {
    tail = 'right'
  } else if (canRight && x <= rightUntil) {
    tail = 'left'
  } else if (canRight && (!canLeft || x - rightUntil <= leftFrom - x)) {
    // No room where it stands: a step to the nearer place that has.
    tail = 'left'
    x = rightUntil
  } else if (canLeft) {
    tail = 'right'
    x = leftFrom
  } else {
    // Too narrow for a bubble on either side: the right edge, as the column of figures does.
    tail = 'right'
    x = hi
  }
  return { at: { x: clamp(x, lo, hi), y }, tail }
}

/** The rectangle an avatar at its post takes with its bubble: the large figure, and `height` of bubble on the side it opens. */
export function claimOf(post: Post, figure: number, reach: number, height: number): Box {
  const half = figure / 2
  const top = post.at.y - figure
  const bottom = top + Math.max(height, figure)
  return post.tail === 'left' ? { left: post.at.x - half, top, right: post.at.x + half + reach, bottom } : { left: post.at.x - half - reach, top, right: post.at.x + half, bottom }
}

/**
 * The post of an avatar that needs room, where its figure and its bubble cover nobody else's
 * (`taken`: the rooms of the others): where it wishes when that is free; else at the nearest
 * height that is; else beside what is in the way; else where it wished (there is no room: the
 * bubbles overlap).
 */
export function placePost(wish: Post, taken: readonly Box[], work: Box, figure: number, reach: number, edge: number, height: number): Post {
  const clear = (p: Post): boolean => {
    const room = claimOf(p, figure, reach, height)
    return !taken.some((t) => overlaps(room, grown(t, POST_GAP_PX)))
  }
  if (taken.length === 0 || clear(wish)) return wish
  const top = Math.min(work.top + edge + figure, work.bottom - edge)
  const bottom = work.bottom - edge
  let best: Post | null = null
  const nearer = (p: Post) => {
    if (best === null || distance(p.at, wish.at) < distance(best.at, wish.at)) best = p
  }
  // Up and down from where it wished, in small steps: the nearest height at which nothing is covered.
  for (let y = top; y <= bottom; y += 16) {
    const p = { at: { x: wish.at.x, y }, tail: wish.tail }
    if (clear(p)) nearer(p)
  }
  if (best !== null) return best
  // No height is free there: beside each room that is in the way, the bubble opening away from it, where that fits in the area.
  const y = clamp(wish.at.y, top, bottom)
  const inside = (p: Post): boolean => {
    const room = claimOf(p, figure, reach, height)
    return room.left >= work.left + edge && room.right <= work.right - edge
  }
  for (const t of taken) {
    const beside: Post[] = [
      { at: { x: t.left - POST_GAP_PX - figure / 2 - 1, y }, tail: 'right' },
      { at: { x: t.right + POST_GAP_PX + figure / 2 + 1, y }, tail: 'left' },
    ]
    for (const p of beside) if (inside(p) && clear(p)) nearer(p)
  }
  return best ?? wish
}

/** The points of a figure that are looked under to see whether it stands on something: its middle and four points of its box. */
export function samplePoints(at: Point, size: number): Point[] {
  return [
    { x: at.x, y: at.y - size * 0.5 },
    { x: at.x - size * 0.32, y: at.y - size * 0.2 },
    { x: at.x + size * 0.32, y: at.y - size * 0.2 },
    { x: at.x - size * 0.32, y: at.y - size * 0.8 },
    { x: at.x + size * 0.32, y: at.y - size * 0.8 },
  ]
}

// ---------------------------------------------------------------- who is there

function newBody(id: string, at: Point, now: number, random: () => number): Body {
  return {
    id,
    x: at.x,
    y: at.y,
    mode: 'rest',
    home: { x: at.x, y: at.y },
    dropped: false,
    to: null,
    best: Number.POSITIVE_INFINITY,
    bestAt: now,
    until: now + between([1200, 4500], random),
    partner: null,
    guest: false,
    cueAt: null,
    cueKind: null,
    looks: null,
    walking: false,
    heldBack: 0,
    walked: 0,
    hop: 0,
    lean: 0,
    visitAfter: now + between(FIRST_VISIT_MS, random),
    stayed: false,
  }
}

/** Ends what `b` does with its partner; the partner goes on by itself at its next step. */
function leave(b: Body, now: number, rest: number) {
  b.mode = 'rest'
  b.to = null
  b.partner = null
  b.guest = false
  b.cueAt = null
  b.cueKind = null
  b.until = Math.max(b.until, now + rest)
}

/**
 * The bodies brought up to date with who is on stage: new ones appear where `spawn` puts
 * them, those that left are gone, and the home of every one the user did not drop is its
 * place in the column as it is now. Called by `step`, and by whoever needs the places before
 * the first step.
 */
export function attend(world: World, i: Pick<StepInput, 'walkers' | 'spawn' | 'bounds' | 'now' | 'random'>): World {
  const there = new Set<string>()
  i.walkers.forEach((w, index) => {
    there.add(w.id)
    const place = into(i.spawn(w.id, index), i.bounds)
    const b = world.bodies.get(w.id)
    if (!b) world.bodies.set(w.id, newBody(w.id, place, i.now, i.random))
    else if (!b.dropped) b.home = place
  })
  for (const id of world.bodies.keys()) if (!there.has(id)) world.bodies.delete(id)
  // Cues nobody took are not kept for ever, and none for an avatar that left.
  world.cues = world.cues.filter((c) => there.has(c.id)).slice(-MAX_CUES)
  return world
}

// ---------------------------------------------------------------- one step

function moveTowards(b: Body, to: Point, speed: number, dt: number): number {
  const dx = to.x - b.x
  const dy = to.y - b.y
  const d = Math.hypot(dx, dy)
  if (d <= AT_PX) {
    b.x = to.x
    b.y = to.y
    return d
  }
  // Slower near the end, so that it comes to a stand instead of stopping dead.
  const pace = d < SLOW_WITHIN_PX ? Math.max(speed * (d / SLOW_WITHIN_PX), Math.min(speed, 12)) : speed
  const way = Math.min(d, (pace * dt) / 1000)
  b.x += (dx / d) * way
  b.y += (dy / d) * way
  if (Math.abs(dx) > 0.01) b.lean += ((dx / d) * Math.min(1, pace / STROLL_PX_S) - b.lean) * Math.min(1, dt / SETTLE_MS)
  return way
}

/** How often a place to stroll to is looked for near by, and then anywhere. */
const TRIES_NEAR = 6
const TRIES_ANYWHERE = 6

/** A walk or a visit begins: how near it comes to its end, and whether it moves at all, is watched from now. */
function setOut(b: Body, now: number) {
  b.best = Number.POSITIVE_INFINITY
  b.bestAt = now
  b.heldBack = 0
}

/**
 * Whether a walk or a visit towards `to` is to be given up: it did not really move for a
 * second (something holds it back), or came no nearer for some seconds (it goes round and
 * round something that stands where it wants to be).
 */
function stuck(b: Body, to: Point, now: number): boolean {
  if (b.heldBack > HELD_BACK_MS) return true
  const d = distance(b, to)
  if (d < b.best - 1) {
    b.best = d
    b.bestAt = now
    return false
  }
  return now - b.bestAt > STUCK_MS
}

/**
 * One step: every avatar's place and doing after `dt` milliseconds. The world is changed in
 * place and returned.
 */
export function step(world: World, i: StepInput): World {
  attend(world, i)
  const pause = finite(i.dt, 0)
  const dt = clamp(pause, 0, MAX_STEP_MS)
  const facts = new Map<string, Walker>()
  for (const w of i.walkers) facts.set(w.id, w)
  const bodies = world.bodies
  const list = [...bodies.values()]
  const quiet = (p: Point): boolean => !i.quiet || i.quiet(p)
  /** Where `b` belongs, inside the area as it is now. */
  const homeOf = (b: Body): Point => into(b.home, i.bounds)
  /** Those the pointer rests on, where they stand in nobody's way: they wait for it. */
  const pointed = new Set<string>()
  for (const b of list) {
    const w = facts.get(b.id)
    if (w?.pointed === true && !w.stays && !w.held && !w.asleep && quiet(b)) pointed.add(b.id)
  }
  const free = (b: Body | undefined): b is Body => {
    if (!b) return false
    const w = facts.get(b.id)
    return !!w && !w.stays && !w.held && !w.asleep && !pointed.has(b.id)
  }
  const near = (b: Body): boolean => i.pointer !== null && Math.hypot(i.pointer.x - b.x, i.pointer.y - (b.y - i.size / 2)) <= NEAR_POINTER_PX
  /** It waits for a pointer that came near, where it stands on nothing the user presses: there the pointer may be for it. */
  const waits = (b: Body): boolean => near(b) && quiet(b)
  /** Those that do not move out of anyone's way: wanted, dragged, asleep, or in a meeting. */
  const standing = list.filter((o) => !free(o) || o.mode === 'meet')
  /** A host is visited where it stands in nobody's way: at home, or on a quiet place. */
  const atEase = (o: Body): boolean => distance(o, homeOf(o)) <= HOME_PX || quiet(o)

  /**
   * A place to stroll to from where `b` stands: inside the bounds, not on something in the
   * way, not beside one that does not move nor where another one is walking to (it would not
   * get there), and quiet: near by first, then anywhere (one whose session works looks near
   * by only). Where nothing is quiet (an editor or a terminal fills the area) it goes home and
   * stays there. null: it stays where it is.
   */
  const somewhere = (b: Body, reach: number, busy: boolean): Point | null => {
    const open = (p: Point): boolean =>
      !blocked(p, i.size, i.obstacles) && !standing.some((o) => o.id !== b.id && distance(p, o) < APART_PX) && !list.some((o) => o.id !== b.id && o.to !== null && distance(p, o.to) < APART_PX)
    const tries = TRIES_NEAR + (busy ? 0 : TRIES_ANYWHERE)
    for (let n = 0; n < tries; n++) {
      let p: Point
      if (n < TRIES_NEAR) {
        const angle = i.random() * Math.PI * 2
        const far = reach * (0.35 + 0.65 * i.random())
        // Avatars walk more to the side than up and down: the window is wider than it is high.
        p = into({ x: b.x + Math.cos(angle) * far, y: b.y + Math.sin(angle) * far * 0.6 }, i.bounds)
      } else {
        p = { x: i.bounds.left + (i.bounds.right - i.bounds.left) * i.random(), y: i.bounds.top + (i.bounds.bottom - i.bounds.top) * i.random() }
      }
      if (distance(p, b) > 12 && open(p) && quiet(p)) return p
    }
    const home = homeOf(b)
    if (distance(b, home) <= HOME_PX) return null
    for (let n = 0; n < TRIES_NEAR; n++) {
      const angle = i.random() * Math.PI * 2
      const far = HOME_PX * 0.5 * i.random()
      const p = into({ x: home.x + Math.cos(angle) * far, y: home.y + Math.sin(angle) * far }, i.bounds)
      if (distance(p, b) > 12 && open(p)) return p
    }
    return null
  }

  // After a pause nobody sets off at the same moment as the others.
  if (pause > LONG_PAUSE_MS) for (const b of list) if (b.mode === 'rest' && b.until <= i.now) b.until = i.now + between([300, 6000], i.random)

  /** Where each one was before this step, and how fast it meant to go by itself (0: it did not). */
  const began = new Map<string, { x: number; y: number; pace: number }>()

  for (const b of list) {
    const w = facts.get(b.id)
    if (!w) continue
    let pace = 0
    began.set(b.id, { x: b.x, y: b.y, pace: 0 })

    if (w.held) {
      const at = into(w.held, i.bounds)
      b.x = at.x
      b.y = at.y
      // Where the user puts it is where it belongs from now on.
      b.home = { x: at.x, y: at.y }
      b.dropped = true
      leave(b, i.now, AFTER_DROP_MS)
      b.stayed = true
    } else if (w.stays || pointed.has(b.id)) {
      if (w.stays && w.post) moveTowards(b, into(w.post, i.bounds), HURRY_PX_S, dt)
      leave(b, i.now, AFTER_STAYING_MS)
      b.stayed = true
    } else if (w.asleep) {
      leave(b, i.now, 0)
      b.until = i.now + 60_000
      b.stayed = false
    } else {
      b.stayed = false
      const partner = b.partner ? bodies.get(b.partner) : undefined
      const partnerBusy = partner ? facts.get(partner.id)?.busy === true : false
      switch (b.mode) {
        case 'rest': {
          if (waits(b)) b.until = Math.max(b.until, i.now + AFTER_POINTER_MS)
          if (i.now < b.until) break
          const hosts = w.busy || i.now < b.visitAfter ? [] : list.filter((o) => o.id !== b.id && free(o) && (o.mode === 'rest' || o.mode === 'walk') && !facts.get(o.id)?.busy && atEase(o))
          if (hosts.length > 0 && i.random() < VISIT_CHANCE) {
            const host = hosts[Math.min(hosts.length - 1, Math.floor(i.random() * hosts.length))]
            b.mode = 'visit'
            b.partner = host.id
            b.guest = true
            b.to = null
            setOut(b, i.now)
            break
          }
          const to = somewhere(b, w.busy ? BUSY_REACH_PX : REACH_PX, w.busy)
          if (to) {
            b.mode = 'walk'
            b.to = to
            setOut(b, i.now)
          } else {
            // Nowhere to go (at home with nothing quiet about, or hemmed in): it stays, and looks again later.
            b.until = i.now + between(w.busy ? BUSY_REST_MS : REST_MS, i.random)
          }
          break
        }
        case 'walk': {
          if (!b.to || waits(b)) {
            leave(b, i.now, AFTER_POINTER_MS)
            break
          }
          // Something is in the way: this walk ends here.
          if (stuck(b, b.to, i.now)) {
            leave(b, i.now, between([800, 2000], i.random))
            break
          }
          pace = STROLL_PX_S
          moveTowards(b, b.to, pace, dt)
          if (distance(b, b.to) <= AT_PX) {
            b.mode = 'rest'
            b.to = null
            // Where it is in the way of something the user presses it does not stay long, unless it is at home.
            const inTheWay = distance(b, homeOf(b)) > HOME_PX && !quiet(b)
            b.until = i.now + between(inTheWay ? IN_THE_WAY_REST_MS : w.busy ? BUSY_REST_MS : REST_MS, i.random)
          }
          break
        }
        case 'visit': {
          // The host went away, got something to do, or is with another; or one of the two has work to do: the visit is off.
          if (!partner || !free(partner) || (partner.mode !== 'rest' && partner.mode !== 'walk') || w.busy || partnerBusy || waits(b)) {
            leave(b, i.now, between([600, 1800], i.random))
            b.visitAfter = i.now + between([4000, 9000], i.random)
            break
          }
          // Beside the host, on the side it comes from; on the other side when something that does not move is there.
          const beside = (side: number): Point | null => {
            const p = into({ x: partner.x + side * MEET_GAP_PX, y: partner.y }, i.bounds)
            const room = Math.abs(p.x - partner.x) >= MEET_GAP_PX - 4 && !blocked(p, i.size, i.obstacles) && !standing.some((o) => o.id !== b.id && o.id !== partner.id && distance(p, o) < APART_PX)
            return room ? p : null
          }
          const side = b.x <= partner.x ? -1 : 1
          const to = beside(side) ?? beside(-side)
          // No room beside the host, or no way to it: the visit is off, and the host goes on by itself.
          if (!to || stuck(b, to, i.now)) {
            leave(b, i.now, between([600, 1800], i.random))
            b.visitAfter = i.now + between([8000, 20_000], i.random)
            break
          }
          // A host on a stroll sees its guest coming: it stops, waits and looks that way.
          if (distance(b, partner) <= NOTICE_PX) {
            if (partner.mode === 'walk') {
              partner.mode = 'rest'
              partner.to = null
            }
            partner.until = Math.max(partner.until, i.now + 2500)
            partner.looks = b.id
          }
          b.to = to
          pace = VISIT_PX_S
          moveTowards(b, to, pace, dt)
          if (distance(b, to) > AT_PX * 2) break
          // They meet: the guest greets, the host stops and answers.
          const until = i.now + between(MEET_MS, i.random)
          b.mode = 'meet'
          b.to = null
          b.until = until
          b.looks = partner.id
          b.cueAt = i.now + (until - i.now) * 0.55
          b.cueKind = 'nod'
          partner.mode = 'meet'
          partner.to = null
          partner.partner = b.id
          partner.guest = false
          partner.until = until
          partner.looks = b.id
          partner.cueAt = i.now + ANSWER_AFTER_MS
          partner.cueKind = 'answer'
          world.cues.push({ id: b.id, kind: 'greet', toward: partner.id })
          break
        }
        case 'meet': {
          // The other one left or is wanted, or one of the two has work to do now: the meeting is over.
          if (!partner || partner.mode !== 'meet' || partner.partner !== b.id || !free(partner) || w.busy || partnerBusy) {
            leave(b, i.now, between([500, 1500], i.random))
            b.looks = null
            b.visitAfter = i.now + between(VISIT_EVERY_MS, i.random)
            break
          }
          b.looks = partner.id
          if (b.cueAt !== null && b.cueKind !== null && i.now >= b.cueAt) {
            world.cues.push({ id: b.id, kind: b.cueKind, toward: partner.id })
            b.cueAt = null
            b.cueKind = null
          }
          if (i.now >= b.until) {
            // They part, each at its own moment: the guest first.
            const rest: readonly [number, number] = b.guest ? [300, 900] : [1500, 4000]
            leave(b, i.now, between(rest, i.random))
            b.looks = null
            b.visitAfter = i.now + between(VISIT_EVERY_MS, i.random)
          }
          break
        }
      }
    }
    const was = began.get(b.id)
    if (was) was.pace = pace
  }

  // Two that stand too close step apart: only those that are free to move do.
  for (let a = 0; a < list.length; a++) {
    for (let c = a + 1; c < list.length; c++) {
      const p = list[a]
      const q = list[c]
      const d = distance(p, q)
      if (d >= APART_PX) continue
      const moves = [p, q].filter((b) => free(b) && b.mode !== 'meet')
      if (moves.length === 0) continue
      // On the same spot: apart along the line through them, the earlier one to the left.
      const ux = d > 0.01 ? (q.x - p.x) / d : 1
      const uy = d > 0.01 ? (q.y - p.y) / d : 0
      const push = Math.min(APART_PX - d, (120 * dt) / 1000) / moves.length
      for (const b of moves) {
        const sign = b === p ? -1 : 1
        b.x += ux * push * sign
        b.y += uy * push * sign
        // One that is on its way goes round the other: sideways, on the side its way leads to.
        if (!b.to || (b.mode !== 'walk' && b.mode !== 'visit')) continue
        const side = -uy * (b.to.x - b.x) + ux * (b.to.y - b.y) >= 0 ? 1 : -1
        const slide = ((b.mode === 'visit' ? VISIT_PX_S : STROLL_PX_S) * dt) / 1000
        b.x += -uy * side * slide
        b.y += ux * side * slide
      }
    }
  }

  for (const b of list) {
    const w = facts.get(b.id)
    if (!w) continue
    // Out of what is in the way, by the shortest way; a walk that leads into it ends. One that only stands (it
    // has a question, the pointer is on it) or sleeps gives way as well: only one at its post and a dragged one
    // stay where they are.
    const gives = !w.held && !(w.stays && w.post)
    if (gives && i.obstacles.length > 0) {
      const onItsWay = free(b) && (b.mode === 'walk' || b.mode === 'visit') ? b.to : null
      for (const o of i.obstacles) {
        // Its own figure and its own bubble are not in its way.
        if (o.of === b.id) continue
        const box = figureBox(b, i.size)
        const wall = grown(o, CLEAR_PX)
        if (!overlaps(box, wall)) continue
        const ways = [
          { dx: wall.left - box.right, dy: 0 },
          { dx: wall.right - box.left, dy: 0 },
          { dx: 0, dy: wall.top - box.bottom },
          { dx: 0, dy: wall.bottom - box.top },
        ].sort((m, n) => Math.abs(m.dx) + Math.abs(m.dy) - (Math.abs(n.dx) + Math.abs(n.dy)))
        const limit = (200 * dt) / 1000
        b.x += clamp(ways[0].dx, -limit, limit)
        b.y += clamp(ways[0].dy, -limit, limit)
        // One that is on its way slides along the edge, towards the end of it that its way leads past. It is
        // making its way round: that it comes no nearer meanwhile does not end its walk (one that cannot move does).
        if (!onItsWay) continue
        b.bestAt = i.now
        const slide = ((b.mode === 'visit' ? VISIT_PX_S : STROLL_PX_S) * dt) / 1000
        if (ways[0].dx !== 0) {
          const down = onItsWay.y - i.size / 2 > wall.bottom || (onItsWay.y - i.size / 2 >= wall.top && b.y - i.size / 2 >= (wall.top + wall.bottom) / 2)
          b.y += down ? slide : -slide
        } else {
          const right = onItsWay.x > wall.right || (onItsWay.x >= wall.left && b.x >= (wall.left + wall.right) / 2)
          b.x += right ? slide : -slide
        }
      }
      if (free(b) && b.mode === 'walk' && b.to && blocked(b.to, i.size, i.obstacles)) leave(b, i.now, between([600, 1500], i.random))
    }
    const at = into(b, i.bounds)
    b.x = at.x
    b.y = at.y
  }

  // The figure walks as far as it really moved: one that is held back stands, its hop and its lean settle.
  for (const b of list) {
    const was = began.get(b.id)
    if (!was) continue
    const moved = Math.hypot(b.x - was.x, b.y - was.y)
    if (was.pace > 0 && moved >= (0.3 * was.pace * dt) / 1000) {
      b.heldBack = 0
      b.walked += moved
      b.hop = Math.abs(Math.sin((b.walked / STEP_PX) * Math.PI))
      b.walking = true
      continue
    }
    b.heldBack = was.pace > 0 ? b.heldBack + dt : 0
    b.walking = was.pace > 0 && b.heldBack < STAND_AFTER_MS
    if (b.walking) continue
    b.walked = 0
    const settle = Math.min(1, dt / SETTLE_MS)
    b.hop = Math.max(0, b.hop - settle)
    b.lean -= b.lean * settle
    if (Math.abs(b.lean) < 0.01) b.lean = 0
  }

  // Those near one that speaks look at it; a meeting keeps its own gaze, and so does a host whose guest is near.
  const speaker = i.speaker !== null ? bodies.get(i.speaker) : undefined
  const awaited = new Map<string, string>()
  for (const b of list) if (b.mode === 'visit' && b.partner && distance(b, bodies.get(b.partner) ?? b) <= NOTICE_PX) awaited.set(b.partner, b.id)
  for (const b of list) {
    if (b.mode === 'meet') continue
    if (b.mode === 'visit' && b.partner) b.looks = b.partner
    else b.looks = awaited.get(b.id) ?? (speaker && speaker.id !== b.id && distance(b, speaker) <= AUDIENCE_PX ? speaker.id : null)
  }

  // One that finished is cheered by those near it.
  for (const id of i.finished) {
    const done = bodies.get(id)
    if (!done) continue
    const crowd = list.filter((b) => b.id !== id && free(b) && b.mode !== 'meet' && distance(b, done) <= AUDIENCE_PX).sort((m, n) => distance(m, done) - distance(n, done))
    for (const b of crowd.slice(0, CHEERS)) world.cues.push({ id: b.id, kind: 'cheer', toward: id })
  }
  return world
}
