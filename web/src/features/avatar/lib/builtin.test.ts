// The built-in motion: roles to animations, amplitudes from the viewBox, and which
// animations each state plays.

import { describe, expect, it } from 'vitest'
import { BUILTIN, builtinAnimations, gazeTravel, resolves, rolePart, stateList } from './builtin'
import { animationProblems, richLook, sampleLook, wire } from './fixtures'
import { checkLook } from './look'
import { AVATAR_STATES } from './types'
import type { Animation, Look, Role } from './types'

/** A look with just these roles, each a group with a box (and a pivot when asked). */
function withRoles(roles: Role[], opts: { pivots?: boolean; viewBox?: [number, number, number, number] } = {}): Look {
  const boxes: Record<Role, [number, number, number, number]> = { body: [60, 40, 80, 130], head: [70, 40, 60, 50], eyes: [80, 55, 40, 12], pupils: [85, 58, 30, 6], armL: [40, 100, 20, 40], armR: [140, 100, 20, 40] }
  const joints: Record<Role, [number, number]> = { body: [100, 170], head: [100, 90], eyes: [100, 61], pupils: [100, 61], armL: [60, 100], armR: [140, 100] }
  const look: Look = { viewBox: opts.viewBox ?? [0, 0, 200, 200], nodes: [], pivots: {}, roles: {}, animations: {}, states: {}, mouth: [], gaze: null }
  for (const role of roles) {
    look.nodes.push({ tag: 'g', id: role, box: boxes[role] })
    look.roles[role] = role
    if (opts.pivots) look.pivots[role] = joints[role]
  }
  return look
}

const names = (look: Look) => [...builtinAnimations(look).keys()]
const tracksOf = (a: Animation | undefined, target: string) => (a?.tracks ?? []).filter((t) => t.target === target)

describe('builtinAnimations', () => {
  it('lists the eleven built-ins', () => {
    expect(BUILTIN).toEqual(['breathe', 'blink', 'think', 'talk', 'listen', 'alert', 'doze', 'wave', 'cheer', 'nod', 'shake'])
  })

  it('gives a look with every role every built-in, each a sound animation', () => {
    const look = withRoles(['body', 'head', 'eyes', 'pupils', 'armL', 'armR'], { pivots: true })
    const all = builtinAnimations(look)
    expect([...all.keys()].sort()).toEqual([...BUILTIN].sort())
    for (const [name, a] of all) expect(animationProblems(a), name).toEqual([])
    expect(all.get('breathe')?.loop).toBe(true)
    expect(all.get('breathe')?.duration).toBe(3.6)
    expect(all.get('blink')?.every).toEqual([2.4, 6])
    expect(all.get('blink')?.duration).toBe(0.18)
    expect(all.get('blink')?.tracks[0]).toMatchObject({ target: 'eyes', prop: 'scaleY', values: [1, 0.1, 1] })
    expect(all.get('think')?.duration).toBe(2.8)
    expect(all.get('talk')?.duration).toBe(0.9)
    expect(all.get('alert')?.duration).toBe(1.2)
    for (const name of ['wave', 'cheer', 'nod', 'shake']) {
      expect(all.get(name)?.loop, name).toBeUndefined()
      expect(all.get(name)?.every, name).toBeUndefined()
    }
    for (const name of ['breathe', 'think', 'talk', 'listen', 'alert', 'doze']) expect(all.get(name)?.loop, name).toBe(true)
  })

  it('generates animations an agent could have written: each one passes the check of the look format', () => {
    const subsets: Role[][] = [['body', 'head', 'eyes', 'pupils', 'armL', 'armR'], ['body'], ['head'], ['eyes', 'body'], ['pupils'], ['armL', 'armR'], ['body', 'armR']]
    for (const roles of subsets) {
      for (const pivots of [true, false]) {
        for (const viewBox of [[0, 0, 200, 200], [-50, -20, 16, 16], [100, 100, 4096, 2000]] as [number, number, number, number][]) {
          const look = withRoles(roles, { pivots, viewBox })
          const all = builtinAnimations(look)
          expect(all.size, roles.join()).toBeGreaterThan(0)
          const written = { ...look, animations: Object.fromEntries(all) }
          expect(checkLook(wire(written)), `${roles.join()} ${pivots} ${viewBox.join()}`).not.toBeNull()
        }
      }
    }
  })

  it('gives each role what needs it and nothing else', () => {
    expect(names(withRoles([]))).toEqual([])
    // The table of the contract, role by role: the server tells the agent the same.
    expect(names(withRoles(['body'])).sort()).toEqual(['alert', 'breathe', 'cheer', 'listen', 'nod', 'shake', 'talk'])
    expect(names(withRoles(['eyes']))).toEqual(['blink'])
    expect(names(withRoles(['eyes', 'body'])).sort()).toEqual(['alert', 'blink', 'breathe', 'cheer', 'doze', 'listen', 'nod', 'shake', 'talk'])
    expect(names(withRoles(['head'])).sort()).toEqual(['listen', 'nod', 'shake', 'talk', 'think'])
    expect(names(withRoles(['pupils']))).toEqual(['think'])
    expect(names(withRoles(['armL']))).toEqual(['wave'])
    expect(names(withRoles(['armR']))).toEqual(['wave'])
    expect(names(withRoles(['armL', 'armR'])).sort()).toEqual(['cheer', 'wave'])
  })

  it('prefers the head for a nod and the pupils for a thought', () => {
    const both = builtinAnimations(withRoles(['body', 'head', 'pupils']))
    expect(both.get('talk')?.tracks.every((t) => t.target === 'head')).toBe(true)
    expect(both.get('nod')?.tracks[0]).toMatchObject({ target: 'head', prop: 'translateY' })
    expect(both.get('think')?.tracks.every((t) => t.target === 'pupils')).toBe(true)
    const bodyOnly = builtinAnimations(withRoles(['body']))
    expect(bodyOnly.get('talk')?.tracks[0]).toMatchObject({ target: 'body', prop: 'rotate' })
    expect(bodyOnly.get('nod')?.tracks[0]).toMatchObject({ target: 'body', prop: 'scaleY' })
    // Gaze switched off: the pupils stay where they are and the head thinks instead.
    const still = withRoles(['head', 'pupils'])
    still.gaze = 0
    expect(builtinAnimations(still).get('think')?.tracks[0]).toMatchObject({ target: 'head', prop: 'rotate' })
    // No head to tilt: nothing thinks, as "0 keeps them still" promises.
    const noHead = withRoles(['pupils', 'body'])
    noHead.gaze = 0
    expect(builtinAnimations(noHead).has('think')).toBe(false)
    // A gaze above 0, however small: they think as far as it goes.
    const small = withRoles(['pupils', 'body'])
    small.gaze = 0.5
    expect(builtinAnimations(small).get('think')?.tracks[0]).toMatchObject({ target: 'pupils', prop: 'translateX', values: [0, -0.4, -0.4, 0.3, 0] })
  })

  it('scales its amplitudes to the viewBox', () => {
    const small = builtinAnimations(withRoles(['body', 'head', 'pupils']))
    const large = builtinAnimations(withRoles(['body', 'head', 'pupils'], { viewBox: [0, 0, 2000, 2000] }))
    const peak = (a: Animation | undefined, prop: string) => Math.max(...(a?.tracks.find((t) => t.prop === prop)?.values ?? []).map((v) => Math.abs(Number(v))))
    expect(peak(small.get('nod'), 'translateY')).toBe(6)
    expect(peak(large.get('nod'), 'translateY')).toBe(60)
    expect(peak(small.get('shake'), 'translateX')).toBe(5)
    expect(peak(large.get('shake'), 'translateX')).toBe(50)
    // 3% of the width, and four fifths of that for the sideways drift of a thought.
    expect(peak(small.get('think'), 'translateX')).toBeCloseTo(4.8, 5)
    expect(peak(large.get('think'), 'translateX')).toBeCloseTo(48, 5)
    // Angles and scale factors have no unit: they stay.
    expect(peak(small.get('talk'), 'rotate')).toBe(peak(large.get('talk'), 'rotate'))
  })

  it('hops only as high as the viewBox has room for', () => {
    const roomy = withRoles(['body'])
    const hop = (look: Look) => Math.max(...(builtinAnimations(look).get('alert')?.tracks.find((t) => t.prop === 'translateY')?.values ?? []).map((v) => -Number(v)))
    const stretch = (look: Look) => Math.max(...(builtinAnimations(look).get('alert')?.tracks.find((t) => t.prop === 'scaleY')?.values ?? []).map(Number))
    // The body starts at y 40: room for the full hop of 7%.
    expect(hop(roomy)).toBe(14)
    expect(stretch(roomy)).toBe(1.05)
    const tight = withRoles(['body'])
    tight.nodes[0].box = [60, 10, 80, 160]
    expect(hop(tight)).toBeCloseTo(3.6, 5)
    expect(hop(tight) + (stretch(tight) - 1) * 160).toBeLessThanOrEqual(10 - 4 + 1e-6)
    const none = withRoles(['body'])
    none.nodes[0].box = [60, 0, 80, 180]
    expect(hop(none)).toBe(3)
    // Moved up by a transform on a parent: the room is measured where the body really is.
    const moved = withRoles(['body'])
    moved.nodes = [{ tag: 'g', attrs: { transform: 'translate(0 -30)' }, children: moved.nodes }]
    expect(hop(moved)).toBeCloseTo(3.6, 5)
  })

  it('raises an arm outwards and upwards about its joint, whichever side it is on', () => {
    const look = withRoles(['body', 'armL', 'armR'], { pivots: true })
    const wave = builtinAnimations(look).get('wave')
    // The right arm waves when there is one.
    const turns = tracksOf(wave, 'armR').find((t) => t.prop === 'rotate')?.values as number[]
    expect(tracksOf(wave, 'armL')).toEqual([])
    // It hangs straight down from (140, 100) to its box centre (150, 120)... down and out: about 63 degrees under the horizon.
    expect(turns[0]).toBe(0)
    expect(turns[5]).toBe(0)
    expect(turns[1]).toBeCloseTo(-(63.435 + 42), 2)
    expect(turns[2]).toBeCloseTo(turns[1] + 32, 5)
    const cheer = builtinAnimations(look).get('cheer')
    const left = tracksOf(cheer, 'armL')[0].values as number[]
    const right = tracksOf(cheer, 'armR')[0].values as number[]
    // Mirror images.
    expect(left.map((v) => (v === 0 ? 0 : -v))).toEqual(right)
    expect(right[1]).toBeLessThan(0)
    expect(tracksOf(cheer, 'body').map((t) => t.prop).sort()).toEqual(['scaleY', 'translateY'])
    // Only a left arm: it does the waving.
    const lefty = builtinAnimations(withRoles(['body', 'armL'], { pivots: true })).get('wave')
    expect((tracksOf(lefty, 'armL')[0].values as number[])[1]).toBeGreaterThan(0)
  })

  it('wiggles an arm that has no joint instead of spinning it', () => {
    const wave = builtinAnimations(withRoles(['body', 'armR'])).get('wave')
    const rotate = tracksOf(wave, 'armR').find((t) => t.prop === 'rotate')?.values as number[]
    expect(Math.max(...rotate.map(Math.abs))).toBeLessThanOrEqual(16)
    expect(tracksOf(wave, 'armR').map((t) => t.prop).sort()).toEqual(['rotate', 'translateY'])
  })

  it('raises an arm on screen whatever transform it is drawn in', () => {
    // The left arm drawn again as the right one in a mirror about x 100: on screen it is the plain right arm.
    const mirrored = (pivots: boolean) => {
      const look = withRoles(['body', 'armL'], { pivots })
      look.nodes.push({ tag: 'g', attrs: { transform: 'translate(200 0) scale(-1 1)' }, children: [{ tag: 'g', id: 'armR', box: [40, 100, 20, 40] }] })
      look.roles.armR = 'armR'
      if (pivots) look.pivots.armR = [60, 100]
      return look
    }
    const plain = builtinAnimations(withRoles(['body', 'armL', 'armR'], { pivots: true }))
    const got = builtinAnimations(mirrored(true))
    const turned = (a: Animation | undefined, arm: string) => (tracksOf(a, arm)[0].values as number[]).map((v) => (v === 0 ? 0 : -v))
    // It turns as far, the other way round inside the mirror: up and out on screen.
    expect(tracksOf(got.get('wave'), 'armR')[0].values).toEqual(turned(plain.get('wave'), 'armR'))
    expect(tracksOf(got.get('wave'), 'armR')[0].values).toEqual([0, 105.435, 73.435, 105.435, 73.435, 0])
    expect(tracksOf(got.get('cheer'), 'armR')[0].values).toEqual(turned(plain.get('cheer'), 'armR'))
    // Without a joint it wiggles the other way round and is lifted up the screen all the same.
    const loose = builtinAnimations(mirrored(false)).get('wave')
    expect(tracksOf(loose, 'armR').map((t) => [t.prop, t.values])).toEqual([
      ['rotate', [0, 16, -10, 16, -10, 0]],
      ['translateY', [0, -6, -6, -6, -6, 0]],
    ])
    // Turned a quarter, its lift is along its own x.
    const quarter = withRoles(['body', 'armR'])
    quarter.nodes[1].attrs = { transform: 'rotate(90 150 120)' }
    expect(tracksOf(builtinAnimations(quarter).get('wave'), 'armR').map((t) => [t.prop, t.values])).toEqual([
      ['rotate', [0, -16, 10, -16, 10, 0]],
      ['translateY', [0, 0, 0, 0, 0, 0]],
      ['translateX', [0, -6, -6, -6, -6, 0]],
    ])
    // An arm drawn with its own turn: worked out where it really points, so it ends 42 over the horizon.
    const own = withRoles(['body', 'armR'], { pivots: true })
    own.nodes[1].attrs = { transform: 'rotate(-40 140 100)' }
    expect((tracksOf(builtinAnimations(own).get('wave'), 'armR')[0].values as number[])[1]).toBe(-65.435)
  })

  it('makes the pupils think up and aside on screen and as far as the gaze, whatever transform they are in', () => {
    // The pupils of withRoles, drawn inside a group with this transform: what think moves them by.
    const think = (transform: string) => {
      const look = withRoles(['pupils'], { pivots: true })
      look.nodes = [{ tag: 'g', attrs: { transform }, children: look.nodes }]
      const tracks = builtinAnimations(look).get('think')?.tracks ?? []
      return ['translateX', 'translateY'].map((prop) => tracks.find((t) => t.prop === prop)?.values)
    }
    // Moved only: as before, 3% of the width (6), up and to the left on screen.
    const plain = [
      [0, -4.8, -4.8, 3.6, 0],
      [0, -5.4, -5.4, -4.2, 0],
    ]
    expect(think('translate(5 7)')).toStrictEqual(plain)
    // Twice as large: half as far inside, as far on screen. Their gaze goes as far.
    expect(think('scale(2)')).toStrictEqual([
      [0, -2.4, -2.4, 1.8, 0],
      [0, -2.7, -2.7, -2.1, 0],
    ])
    // Upside down, mirrored and turned a quarter: inside, the way that is up and aside on screen.
    expect(think('scale(1 -1)')).toStrictEqual([plain[0], [0, 5.4, 5.4, 4.2, 0]])
    expect(think('scale(-1 1)')).toStrictEqual([[0, 4.8, 4.8, -3.6, 0], plain[1]])
    expect(think('rotate(90)')).toStrictEqual([
      [0, -5.4, -5.4, -4.2, 0],
      [0, 4.8, 4.8, -3.6, 0],
    ])
    // No -0 at rest, whichever way the matrix turns (toStrictEqual tells -0 from 0).
    expect(think('rotate(180)')).toStrictEqual([
      [0, 4.8, 4.8, -3.6, 0],
      [0, 5.4, 5.4, 4.2, 0],
    ])
  })

  it('ignores roles that name nothing drawable', () => {
    const look = withRoles(['body', 'head'])
    look.roles.eyes = 'nowhere'
    look.roles.head = 'constructor'
    ;(look.roles as Record<string, string>).tail = 'body'
    look.nodes.push({ tag: 'defs', children: [{ tag: 'clipPath', id: 'c', children: [{ tag: 'circle', id: 'inside' }] }] })
    look.roles.pupils = 'inside'
    look.roles.armL = 'c'
    expect(rolePart(look, 'eyes')).toBeNull()
    expect(rolePart(look, 'head')).toBeNull()
    expect(rolePart(look, 'pupils')).toBeNull()
    expect(rolePart(look, 'armL')).toBeNull()
    expect(rolePart(look, 'body')).toBe('body')
    expect(names(look).sort()).toEqual(['alert', 'breathe', 'cheer', 'listen', 'nod', 'shake', 'talk'])
    expect(names({} as Look)).toEqual([])
    expect(names(null as unknown as Look)).toEqual([])
  })

  it('hands out a map of its own, and builds once per look', () => {
    const look = withRoles(['body'])
    const first = builtinAnimations(look)
    first.delete('breathe')
    first.set('hack', { duration: 1, tracks: [] })
    const second = builtinAnimations(look)
    expect(second.has('breathe')).toBe(true)
    expect(second.has('hack')).toBe(false)
    expect(second.get('alert')).toBe(first.get('alert'))
    expect(Object.isFrozen(second.get('alert')?.tracks[0].values)).toBe(true)
  })

  it('limits the pupils’ travel', () => {
    expect(gazeTravel(withRoles([]))).toBe(6)
    const look = withRoles([])
    look.gaze = 4
    expect(gazeTravel(look)).toBe(4)
    look.gaze = 50
    expect(gazeTravel(look)).toBe(10)
    look.gaze = 0
    expect(gazeTravel(look)).toBe(0)
    look.gaze = -3
    expect(gazeTravel(look)).toBe(0)
    look.gaze = Number.NaN
    expect(gazeTravel(look)).toBe(6)
  })
})

describe('stateList', () => {
  const full = withRoles(['body', 'head', 'eyes', 'pupils', 'armL', 'armR'])

  it('is the default map for a look whose roles allow everything', () => {
    expect(stateList(full, 'idle')).toEqual(['breathe', 'blink'])
    expect(stateList(full, 'working')).toEqual(['breathe', 'think', 'blink'])
    expect(stateList(full, 'speaking')).toEqual(['breathe', 'talk', 'blink'])
    expect(stateList(full, 'listening')).toEqual(['breathe', 'listen', 'blink'])
    expect(stateList(full, 'waiting')).toEqual(['breathe', 'blink'])
    expect(stateList(full, 'attention')).toEqual(['alert', 'blink'])
    expect(stateList(full, 'error')).toEqual(['alert', 'blink'])
    expect(stateList(full, 'done')).toEqual(['breathe', 'blink', 'nod'])
    expect(stateList(full, 'sleeping')).toEqual(['doze'])
  })

  it('filters the default map by what the roles allow', () => {
    const eyes = withRoles(['eyes'])
    expect(stateList(eyes, 'working')).toEqual(['blink'])
    // Dozing needs a body to breathe with as well: without one the state shows what idle shows.
    expect(stateList(eyes, 'sleeping')).toEqual(['blink'])
    expect(stateList(withRoles(['eyes', 'body']), 'sleeping')).toEqual(['doze'])
    const body = withRoles(['body'])
    expect(stateList(body, 'attention')).toEqual(['alert'])
    expect(stateList(body, 'working')).toEqual(['breathe'])
  })

  it('falls back to idle’s list, and to nothing when there is nothing', () => {
    const arm = withRoles(['armR'])
    for (const state of AVATAR_STATES) expect(stateList(arm, state), state).toEqual([])
    const custom = withRoles(['armR'])
    custom.animations.float = { duration: 3, loop: true, tracks: [{ target: 'armR', prop: 'translateY', values: [0, -4, 0] }] }
    custom.states.idle = ['float']
    // No built-in applies to "attention" here: it shows what idle shows.
    expect(stateList(custom, 'attention')).toEqual(['float'])
    expect(stateList(custom, 'sleeping')).toEqual(['float'])
  })

  it('takes the agent’s entry first, an empty one too', () => {
    const look = checkLook(wire(sampleLook())) as Look
    expect(stateList(look, 'done')).toEqual(['breathe', 'blink', 'hop'])
    expect(stateList(look, 'waiting')).toEqual([])
    expect(stateList(look, 'idle')).toEqual(['breathe', 'blink'])
  })

  it('leaves out names that mean nothing, repeats, and everything past six', () => {
    const look = withRoles(['body', 'eyes'])
    look.animations.spin = { duration: 1, loop: true, tracks: [{ target: 'body', prop: 'rotate', values: [0, 360] }] }
    look.states.idle = ['spin', 'think', 'constructor', '__proto__', 'toString', 'spin', 'nowhere', 'blink']
    expect(stateList(look, 'idle')).toEqual(['spin', 'blink'])
    look.states.working = ['spin', 'blink', 'breathe', 'alert', 'doze', 'nod', 'shake', 'talk']
    expect(stateList(look, 'working')).toHaveLength(6)
    ;(look.states as Record<string, unknown>).done = 'spin'
    expect(stateList(look, 'done')).toEqual(['breathe', 'blink', 'nod'])
    // A state that is none: what idle shows.
    expect(stateList(look, 'constructor' as 'idle')).toEqual(['spin', 'blink'])
    expect(resolves(look, 'spin')).toBe(true)
    expect(resolves(look, 'breathe')).toBe(true)
    expect(resolves(look, 'wave')).toBe(false)
    expect(resolves(look, 'hasOwnProperty')).toBe(false)
    expect(resolves(look, 7)).toBe(false)
  })

  it('gives a look that names no role no built-in motion, whatever its parts are called and whatever rig it has', () => {
    const look = checkLook(wire(richLook())) as Look
    expect(Object.keys(look.roles)).toEqual([])
    expect(names(look)).toEqual([])
    for (const state of AVATAR_STATES) expect(stateList(look, state).filter((n) => !Object.hasOwn(look.animations, n)), state).toEqual([])
    expect(stateList(look, 'idle')).toEqual(['twirl'])
    expect(stateList(look, 'sleeping')).toEqual(['twirl'])
    for (const name of BUILTIN) expect(resolves(look, name), name).toBe(false)
    // Parts named like the roles are no roles.
    const named = withRoles(['body', 'head', 'eyes', 'pupils', 'armL', 'armR'])
    named.roles = {}
    expect(names(named)).toEqual([])
  })

  it('lets an agent animation take a built-in’s name', () => {
    const look = withRoles(['body'])
    look.animations.breathe = { duration: 2, loop: true, tracks: [{ target: 'body', prop: 'translateY', values: [0, -3, 0] }] }
    expect(stateList(look, 'idle')).toEqual(['breathe'])
    expect(builtinAnimations(look).get('breathe')?.duration).toBe(3.6)
  })
})
