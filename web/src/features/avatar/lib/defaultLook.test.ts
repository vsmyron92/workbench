// The default look: what the app itself draws must be a sound look in the look format,
// painted with the app's tokens only, and alive through the built-in motion.

import { describe, expect, it } from 'vitest'
import { BUILTIN, builtinAnimations, resolves, stateList } from './builtin'
import { defaultHue, defaultLook } from './defaultLook'
import { compile } from './engine'
import { animationProblems, wire } from './fixtures'
import { TOKENS, checkLook, lookIndex, rootBox } from './look'
import { AVATAR_STATES, stableHash } from './types'
import type { Look, LookNode } from './types'

describe('defaultLook', () => {
  const hexFor = (look: Look): unknown => {
    // The same look with a plain colour in place of each token: what a server could have stored.
    const swap = (node: LookNode): unknown => {
      const attrs: Record<string, string | number> = {}
      for (const [key, value] of Object.entries(node.attrs ?? {})) {
        if (key === 'fillToken') attrs.fill = '#3987e5'
        else if (key === 'strokeToken') attrs.stroke = '#3987e5'
        else if (key === 'stopColorToken') attrs.stopColor = '#3987e5'
        else attrs[key] = value
      }
      return { ...node, attrs, children: node.children?.map(swap) }
    }
    return wire({ ...look, nodes: look.nodes.map(swap) })
  }

  it('is written in the look format: with plain colours for its tokens it passes the check', () => {
    const look = defaultLook('t1')
    expect(checkLook(wire(look))).toBeNull()
    const plain = checkLook(hexFor(look))
    expect(plain).not.toBeNull()
    expect(plain?.roles).toEqual({ body: 'body', head: 'head', eyes: 'eyes', pupils: 'pupils', armL: 'armL', armR: 'armR' })
    expect(plain?.mouth).toHaveLength(1)
  })

  it('uses the app’s tokens for every colour, and no colour literal', () => {
    const used = new Set<string>()
    const walk = (nodes: readonly LookNode[]) => {
      for (const node of nodes) {
        for (const [key, value] of Object.entries(node.attrs ?? {})) {
          if (key.endsWith('Token')) {
            expect(TOKENS.has(String(value)), String(value)).toBe(true)
            used.add(String(value))
          }
          if (key === 'fill' || key === 'stroke' || key === 'stopColor') expect(value).toBe('none')
          expect(String(value)).not.toMatch(/#[0-9a-f]{3,8}|rgb|hsl/i)
        }
        walk(node.children ?? [])
      }
    }
    const look = defaultLook('t1')
    walk(look.nodes)
    expect([...used].sort()).toEqual(['accent', 'bg-elevated', 'fg', defaultHue('t1')].sort())
    expect(JSON.stringify(look.animations)).not.toMatch(/#[0-9a-f]{3,8}/i)
  })

  it('picks one of the eight hues by a stable hash of the seed', () => {
    const hues = new Set<string>()
    for (let i = 0; i < 400; i++) hues.add(defaultHue(`terminal-${i}`))
    expect([...hues].sort()).toEqual(['plot-1', 'plot-2', 'plot-3', 'plot-4', 'plot-5', 'plot-6', 'plot-7', 'plot-8'])
    expect(defaultHue('a1b2c3')).toBe(defaultHue('a1b2c3'))
    expect(defaultHue('t1')).toBe(`plot-${(stableHash('t1') % 8) + 1}`)
    expect(JSON.stringify(defaultLook('a1b2c3'))).toContain(`"fillToken":"${defaultHue('a1b2c3')}"`)
    // A seed that is no string still gives a look.
    expect(defaultLook(undefined as unknown as string).nodes.length).toBeGreaterThan(0)
  })

  it('is one frozen object per hue, so its identity is stable', () => {
    const a = defaultLook('t1')
    expect(defaultLook('t1')).toBe(a)
    expect(Object.isFrozen(a)).toBe(true)
    expect(Object.isFrozen(a.nodes)).toBe(true)
    expect(Object.isFrozen(a.animations.glow.tracks[0].values)).toBe(true)
    expect(() => {
      ;(a as { gaze: number | null }).gaze = 99
    }).toThrow()
  })

  it('animates transforms and opacity only, and keeps the flash rule', () => {
    const look = defaultLook('t1')
    for (const [name, a] of Object.entries(look.animations)) {
      for (const t of a.tracks) expect(['translateX', 'translateY', 'rotate', 'scale', 'scaleX', 'scaleY', 'opacity'], name).toContain(t.prop)
    }
    for (const drive of look.mouth) expect('prop' in drive && ['scaleY', 'scaleX', 'scale', 'translateY'].includes(drive.prop)).toBe(true)
    const glow = look.animations.glow
    expect((glow.duration / (glow.tracks[0].values.length - 1))).toBeGreaterThanOrEqual(0.17)
  })

  it('has a box for every part, where its geometry is', () => {
    const look = defaultLook('t1')
    for (const [id, entry] of lookIndex(look)) {
      if (entry.inert) continue
      expect(entry.node.box, id).toHaveLength(4)
    }
    expect(rootBox(look, 'bulb')).toEqual([93.5, 20.5, 13, 13])
    expect(rootBox(look, 'shadow')).toEqual([63, 168.5, 74, 11])
    const body = rootBox(look, 'body') as number[]
    const head = rootBox(look, 'head') as number[]
    // The body holds the head; nothing leaves the viewBox.
    expect(body[1]).toBe(head[1])
    expect(body[0]).toBeGreaterThanOrEqual(0)
    expect(body[1]).toBeGreaterThanOrEqual(0)
    expect(body[0] + body[2]).toBeLessThanOrEqual(200)
    expect(body[1] + body[3]).toBeLessThanOrEqual(200)
    // No part carries a transform of its own: the engine adds its own after it.
    expect(JSON.stringify(look.nodes)).not.toContain('"transform"')
  })

  it('compiles, with every built-in and its own two animations', () => {
    const look = defaultLook('t1')
    expect(compile(look)).not.toBeNull()
    expect([...builtinAnimations(look).keys()].sort()).toEqual([...BUILTIN].sort())
    for (const [name, a] of builtinAnimations(look)) expect(animationProblems(a), name).toEqual([])
    for (const [name, a] of Object.entries(look.animations)) expect(animationProblems(a), name).toEqual([])
    expect(stateList(look, 'idle')).toEqual(['breathe', 'blink', 'sway'])
    expect(stateList(look, 'working')).toEqual(['breathe', 'think', 'blink', 'glow'])
    expect(stateList(look, 'speaking')).toEqual(['breathe', 'talk', 'blink'])
    expect(stateList(look, 'sleeping')).toEqual(['doze'])
    for (const state of AVATAR_STATES) for (const name of stateList(look, state)) expect(resolves(look, name), name).toBe(true)
    // With the built-in motion written out as its own animations it is still a look an agent could have sent.
    const written = { ...look, animations: { ...look.animations, ...Object.fromEntries(builtinAnimations(look)) } }
    expect(checkLook(hexFor(written))).not.toBeNull()
  })
})
