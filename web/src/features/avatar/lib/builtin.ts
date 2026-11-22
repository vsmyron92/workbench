// Workbench's built-in motion. It is written against roles, not against one drawing: for
// any look it is generated from the look's roles, boxes and pivots, with amplitudes scaled
// to the viewBox. A look with roles is therefore alive before its agent wrote one track.
//
// What it produces are ordinary animations in the look format, so the engine plays them
// like an agent's own; an agent animation with a built-in's name replaces the built-in.
//
// Pure: no DOM, no globals.

import { MORE_LIMITS, isId, lookIndex, rootBox, viewBoxOf } from './look'
import { deepFreeze } from './types'
import type { Animation, AvatarState, Look, Role, Track } from './types'

/** The built-in animations, by name. Which of them a look gets depends on its roles. */
export const BUILTIN: readonly string[] = ['breathe', 'blink', 'think', 'talk', 'listen', 'alert', 'doze', 'wave', 'cheer', 'nod', 'shake']

/** What a state plays when the agent's `states` has no entry for it. */
const DEFAULT_STATES: ReadonlyMap<AvatarState, readonly string[]> = new Map<AvatarState, readonly string[]>([
  ['idle', ['breathe', 'blink']],
  ['working', ['breathe', 'think', 'blink']],
  ['speaking', ['breathe', 'talk', 'blink']],
  ['listening', ['breathe', 'listen', 'blink']],
  ['waiting', ['breathe', 'blink']],
  ['attention', ['alert', 'blink']],
  ['error', ['alert', 'blink']],
  ['done', ['breathe', 'blink', 'nod']],
  ['sleeping', ['doze']],
])

/** Pupil travel when the look gives none, and the most a look may ask for, as parts of the viewBox width. */
const GAZE_DEFAULT = 0.03
const GAZE_MOST = 0.05

/**
 * How far the pupils travel, in viewBox units: the look's `gaze`, at most 5% of the
 * viewBox width; 3% when it gives none; 0 switches gaze off.
 */
export function gazeTravel(look: Look): number {
  const width = viewBoxOf(look)[2]
  const own: unknown = look?.gaze
  if (typeof own !== 'number' || !Number.isFinite(own)) return width * GAZE_DEFAULT
  return Math.min(Math.max(own, 0), width * GAZE_MOST)
}

/** The id a role names, when that part exists and can be animated. */
export function rolePart(look: Look, role: Role): string | null {
  const roles: unknown = look?.roles
  if (typeof roles !== 'object' || roles === null || !Object.hasOwn(roles, role)) return null
  const id: unknown = (roles as Record<string, unknown>)[role]
  if (!isId(id)) return null
  const entry = lookIndex(look).get(id)
  return entry && !entry.inert ? id : null
}

const round = (n: number) => Math.round(n * 1000) / 1000

/**
 * Which way an arm lifts: -1 turns it anticlockwise on screen, which raises an arm on the
 * right half of the figure; an arm on the left half is raised by +1.
 */
function liftSign(look: Look, arm: string, role: 'armL' | 'armR', middle: number): number {
  const box = rootBox(look, arm)
  if (!box) return role === 'armR' ? -1 : 1
  const centre = box[0] + box[2] / 2
  if (Math.abs(centre - middle) < 1e-6) return role === 'armR' ? -1 : 1
  return centre > middle ? -1 : 1
}

/**
 * How far an arm turns about its joint to point `above` degrees over the horizon, away
 * from the body: worked out from where the arm lies at rest (its box against its pivot).
 * null when the look gives the arm no joint or no box.
 */
function raiseTurn(look: Look, arm: string, sign: number, above: number): number | null {
  const pivots: unknown = look?.pivots
  if (typeof pivots !== 'object' || pivots === null || !Object.hasOwn(pivots, arm)) return null
  const pivot: unknown = (pivots as Record<string, unknown>)[arm]
  const box: unknown = lookIndex(look).get(arm)?.node.box
  if (!Array.isArray(pivot) || !Array.isArray(box) || box.length !== 4 || ![pivot[0], pivot[1], ...box].every((n) => typeof n === 'number' && Number.isFinite(n))) return null
  const dx = box[0] + box[2] / 2 - pivot[0]
  const dy = box[1] + box[3] / 2 - pivot[1]
  if (Math.hypot(dx, dy) < 1e-6) return null
  // Screen angles: 0 points right, 90 down. The raised arm points up and outwards.
  const rest = (Math.atan2(dy, dx) * 180) / Math.PI
  const want = sign < 0 ? -above : above - 180
  let turn = want - rest
  while (turn > 180) turn -= 360
  while (turn <= -180) turn += 360
  return Math.abs(turn) < 15 ? sign * 15 : round(turn)
}

function build(look: Look): Map<string, Animation> {
  const out = new Map<string, Animation>()
  const [vx, vy, width, height] = viewBoxOf(look)
  const body = rolePart(look, 'body')
  const head = rolePart(look, 'head')
  const eyes = rolePart(look, 'eyes')
  const pupils = rolePart(look, 'pupils')
  const armL = rolePart(look, 'armL')
  const armR = rolePart(look, 'armR')

  if (body) {
    out.set('breathe', {
      duration: 3.6,
      loop: true,
      ease: 'inOut',
      tracks: [
        { target: body, prop: 'scaleY', values: [1, 1.025, 1] },
        { target: body, prop: 'scaleX', values: [1, 0.99, 1] },
      ],
    })
  }

  if (eyes) {
    out.set('blink', { duration: 0.18, every: [2.4, 6], tracks: [{ target: eyes, prop: 'scaleY', values: [1, 0.1, 1] }] })
  }

  // A look that switched its gaze off keeps its pupils still where it has a head to tilt instead.
  const travel = gazeTravel(look)
  if (pupils && (travel > 0 || !head)) {
    const drift = travel > 0 ? travel : width * GAZE_DEFAULT
    const times = [0, 0.2, 0.55, 0.8, 1]
    out.set('think', {
      duration: 2.8,
      loop: true,
      ease: 'inOut',
      times,
      tracks: [
        { target: pupils, prop: 'translateX', values: [0, -0.8, -0.8, 0.6, 0].map((k) => round(k * drift) || 0) },
        { target: pupils, prop: 'translateY', values: [0, -0.9, -0.9, -0.7, 0].map((k) => round(k * drift) || 0) },
      ],
    })
  } else if (head) {
    out.set('think', { duration: 2.8, loop: true, ease: 'inOut', times: [0, 0.2, 0.55, 0.8, 1], tracks: [{ target: head, prop: 'rotate', values: [0, 4, 4, -2.5, 0] }] })
  }

  if (head) {
    const dip = round(height * 0.012)
    out.set('talk', {
      duration: 0.9,
      loop: true,
      ease: 'inOut',
      tracks: [
        { target: head, prop: 'translateY', values: [0, dip, 0, round(dip * 0.6), 0] },
        { target: head, prop: 'rotate', values: [0, 1.2, 0, -1.2, 0] },
      ],
    })
    out.set('listen', { duration: 2.4, loop: true, ease: 'inOut', tracks: [{ target: head, prop: 'rotate', values: [-5, -6.2, -5] }] })
  } else if (body) {
    out.set('talk', { duration: 0.9, loop: true, ease: 'inOut', tracks: [{ target: body, prop: 'rotate', values: [0, 1.6, 0, -1.6, 0] }] })
    out.set('listen', { duration: 2.4, loop: true, ease: 'inOut', tracks: [{ target: body, prop: 'rotate', values: [-2.5, -3.2, -2.5] }] })
  }

  // How high the figure may hop, and how far it may stretch while it does, before the viewBox cuts it off.
  const bodyBox = body ? rootBox(look, body) : null
  const room = bodyBox ? Math.max(bodyBox[1] - vy - height * 0.02, 0) : height * 0.1
  const hop = round(Math.max(Math.min(height * 0.07, room * 0.6), height * 0.015))
  const stretch = round(1 + Math.max(Math.min(0.05, bodyBox && bodyBox[3] > 0 ? (room * 0.4) / bodyBox[3] : 0.05), 0.01))

  if (body) {
    const times = [0, 0.14, 0.42, 0.7, 0.84, 1]
    out.set('alert', {
      duration: 1.2,
      loop: true,
      times,
      tracks: [
        { target: body, prop: 'translateY', values: [0, 0, -hop, 0, 0, 0], ease: ['linear', 'out', 'in', 'linear', 'linear'] },
        { target: body, prop: 'scaleY', values: [1, 0.93, stretch, 0.94, 1, 1], ease: 'inOut' },
      ],
    })
  }

  if (eyes && body) {
    out.set('doze', {
      duration: 5.2,
      loop: true,
      tracks: [
        { target: eyes, prop: 'scaleY', values: [0.1, 0.1] },
        { target: body, prop: 'scaleY', values: [1, 1.035, 1], ease: 'inOut' },
        { target: body, prop: 'scaleX', values: [1, 0.985, 1], ease: 'inOut' },
      ],
    })
  }

  const middle = bodyBox ? bodyBox[0] + bodyBox[2] / 2 : vx + width / 2
  const lift = round(height * 0.03)
  /**
   * One arm raised to `above` degrees over the horizon and swung `back` degrees from there
   * and up again: about its joint when it has one, else a small wiggle in place.
   */
  const armTracks = (arm: string, role: 'armL' | 'armR', above: number, back: number): Track[] => {
    const sign = liftSign(look, arm, role, middle)
    const turn = raiseTurn(look, arm, sign, above)
    if (turn !== null) {
      const eased = round(turn - Math.sign(turn) * back)
      return [{ target: arm, prop: 'rotate', values: [0, turn, eased, turn, eased, 0], ease: 'inOut' }]
    }
    return [
      { target: arm, prop: 'rotate', values: [0, sign * 16, sign * -10, sign * 16, sign * -10, 0], ease: 'inOut' },
      { target: arm, prop: 'translateY', values: [0, -lift, -lift, -lift, -lift, 0], ease: 'inOut' },
    ]
  }

  const waver: ['armL' | 'armR', string] | null = armR ? ['armR', armR] : armL ? ['armL', armL] : null
  if (waver) {
    out.set('wave', { duration: 1.4, tracks: armTracks(waver[1], waver[0], 42, 32) })
  }

  // Both arms, or the body alone; with everything, everything joins in.
  if ((armL && armR) || body) {
    const tracks: Track[] = []
    if (armL) tracks.push(...armTracks(armL, 'armL', 38, 12))
    if (armR) tracks.push(...armTracks(armR, 'armR', 38, 12))
    if (body) {
      tracks.push({ target: body, prop: 'translateY', values: [0, -hop, 0, round(-hop * 0.5), 0, 0], ease: ['out', 'in', 'out', 'in', 'linear'] })
      tracks.push({ target: body, prop: 'scaleY', values: [1, stretch, 0.95, round(1 + (stretch - 1) * 0.5), 0.98, 1], ease: 'inOut' })
    }
    out.set('cheer', { duration: 1.2, tracks })
  }

  if (head) {
    const dip = round(height * 0.03)
    const swing = round(width * 0.025)
    out.set('nod', { duration: 0.7, ease: 'inOut', tracks: [{ target: head, prop: 'translateY', values: [0, dip, 0, round(dip * 0.6), 0] }] })
    out.set('shake', { duration: 0.8, ease: 'inOut', tracks: [{ target: head, prop: 'translateX', values: [0, -swing, swing, round(-swing * 0.7), round(swing * 0.5), 0] }] })
  } else if (body) {
    out.set('nod', { duration: 0.7, ease: 'inOut', tracks: [{ target: body, prop: 'scaleY', values: [1, 0.95, 1, 0.97, 1] }] })
    out.set('shake', { duration: 0.8, ease: 'inOut', tracks: [{ target: body, prop: 'rotate', values: [0, -3, 3, -2, 1.5, 0] }] })
  }

  return out
}

const BUILT = new WeakMap<Look, ReadonlyMap<string, Animation>>()

const NOTHING: ReadonlyMap<string, Animation> = new Map()

function built(look: Look): ReadonlyMap<string, Animation> {
  if (typeof look !== 'object' || look === null) return NOTHING
  let made = BUILT.get(look)
  if (!made) {
    made = build(look)
    // Shared by every figure of this look: nobody changes them.
    for (const animation of made.values()) deepFreeze(animation)
    BUILT.set(look, made)
  }
  return made
}

/**
 * The built-in animations this look's roles allow, by name. Generated once per look
 * object; the map is the caller's own, the animations in it are shared and frozen.
 */
export function builtinAnimations(look: Look): Map<string, Animation> {
  return new Map(built(look))
}

/** Whether a name in a state list or a gesture means something for this look: the agent's animation first, then the built-in. */
export function resolves(look: Look, name: unknown): name is string {
  if (!isId(name)) return false
  const own: unknown = look?.animations
  if (typeof own === 'object' && own !== null && Object.hasOwn(own, name)) return true
  return built(look).has(name)
}

/**
 * The animations a state plays, in order: the agent's entry for it, else the default map
 * filtered by what the roles allow, else `idle`'s. Names that mean nothing for this look
 * are left out; at most 6.
 */
export function stateList(look: Look, state: AvatarState): string[] {
  const usable = (names: readonly unknown[]): string[] => {
    const out: string[] = []
    for (const name of names) {
      if (out.length >= MORE_LIMITS.stateNames) break
      if (resolves(look, name) && !out.includes(name)) out.push(name)
    }
    return out
  }
  const states: unknown = look?.states
  if (typeof states === 'object' && states !== null && DEFAULT_STATES.has(state) && Object.hasOwn(states, state)) {
    const own: unknown = (states as Record<string, unknown>)[state]
    // An empty entry is a choice too: nothing moves in that state.
    if (Array.isArray(own)) return usable(own)
  }
  const fallback = usable(DEFAULT_STATES.get(state) ?? [])
  if (fallback.length > 0 || state === 'idle') return fallback
  return stateList(look, 'idle')
}
