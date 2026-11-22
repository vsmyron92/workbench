// The server's example looks (its golden files) and its limits, read from the server's own
// sources: what the validator stores is what the browser accepts with equal content,
// compiles, and draws from its tables. The files are loaded with `?raw` from outside web/
// (vite reads them from disk, tsc takes the type from vite/client), as the Confluence
// converter's test loads its sample; every `*.look.json` of the examples is one of them.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import lookRs from '../../../../../server/src/avatar/look.rs?raw'
import { resolves, stateList } from './builtin'
import { defaultLook } from './defaultLook'
import { compile, createEngine, sample } from './engine'
import type { Clip } from './engine'
import { fakeFrames } from './fixtures'
import { LIMITS, MORE_LIMITS, TOKENS, checkLook, cleanIdPrefix, faceViewBox, lookElements, lookIndex } from './look'
import { AVATAR_STATES } from './types'
import type { Look, LookNode } from './types'

const FILES: Record<string, string> = import.meta.glob('../../../../../server/src/avatar/examples/*.look.json', { query: '?raw', import: 'default', eager: true })

/** Every example look, by its name (the file name without `.look.json`), in the order of the names. */
const GOLDEN: readonly (readonly [string, string])[] = Object.entries(FILES)
  .map(([path, raw]): [string, string] => [path.replace(/^.*\//, '').replace(/\.look\.json$/, ''), raw])
  .sort((a, b) => a[0].localeCompare(b[0]))
const rawOf = (name: string): string => GOLDEN.find(([n]) => n === name)?.[1] ?? ''

/** A golden file as the server would send it, checked. */
function golden(raw: string): Look {
  const look = checkLook(JSON.parse(raw))
  if (!look) throw new Error('the browser rejects a look the server stores')
  return look
}

// ---------------------------------------------------------------- reading static markup

interface Element {
  tag: string
  attrs: Map<string, string>
  text: string
}

/**
 * The elements of React's static markup, in document order. It throws on anything that is
 * not a tag, a closing tag or text, so nothing in the markup goes unread.
 */
function elements(html: string): Element[] {
  const out: Element[] = []
  const open: Element[] = []
  const tag = /<(\/?)([A-Za-z][A-Za-z0-9]*)((?:\s+[^\s=<>"'/]+(?:="[^"]*")?)*)\s*(\/?)>/y
  let at = 0
  while (at < html.length) {
    if (html[at] !== '<') {
      const next = html.indexOf('<', at)
      const end = next === -1 ? html.length : next
      const inside = open[open.length - 1]
      if (!inside) throw new Error(`text outside the root: ${html.slice(at, end)}`)
      inside.text += html.slice(at, end)
      at = end
      continue
    }
    tag.lastIndex = at
    const m = tag.exec(html)
    if (!m) throw new Error(`not a tag at ${at}: ${html.slice(at, at + 60)}`)
    at = tag.lastIndex
    if (m[1] === '/') {
      if (open.pop()?.tag !== m[2]) throw new Error(`</${m[2]}> closes nothing`)
      continue
    }
    const el: Element = { tag: m[2], attrs: new Map(), text: '' }
    for (const a of m[3].matchAll(/\s+([^\s=<>"'/]+)(?:="([^"]*)")?/g)) {
      if (el.attrs.has(a[1])) throw new Error(`<${el.tag}> has ${a[1]} twice`)
      el.attrs.set(a[1], a[2] ?? '')
    }
    out.push(el)
    if (m[4] !== '/') open.push(el)
  }
  if (open.length > 0) throw new Error(`<${open[open.length - 1].tag}> is not closed`)
  return out
}

/** The tags a look may draw, and the root the app draws them in. */
const TAGS = new Set([
  'svg',
  'g',
  'path',
  'circle',
  'ellipse',
  'rect',
  'line',
  'polyline',
  'polygon',
  'text',
  'defs',
  'linearGradient',
  'radialGradient',
  'stop',
  'clipPath',
  'use',
  'mask',
  'filter',
  'feGaussianBlur',
  'feDropShadow',
  'feOffset',
  'feFlood',
  'feColorMatrix',
  'feComposite',
  'feBlend',
  'feMerge',
  'feMergeNode',
  'feMorphology',
  'feTurbulence',
  'feDisplacementMap',
])

/** Every attribute a look may put on an element, by the name the DOM gets. `viewBox` is the root's, set by the app. */
const ATTRS = new Set([
  'viewBox',
  'id',
  'transform',
  'fill',
  'stroke',
  'stroke-width',
  'stroke-linecap',
  'stroke-linejoin',
  'stroke-miterlimit',
  'stroke-dasharray',
  'stroke-dashoffset',
  'fill-opacity',
  'stroke-opacity',
  'opacity',
  'fill-rule',
  'clip-rule',
  'clip-path',
  'mask',
  'filter',
  'href',
  'd',
  'cx',
  'cy',
  'r',
  'rx',
  'ry',
  'x',
  'y',
  'width',
  'height',
  'x1',
  'y1',
  'x2',
  'y2',
  'points',
  'font-size',
  'font-weight',
  'text-anchor',
  'dominant-baseline',
  'letter-spacing',
  'gradientUnits',
  'gradientTransform',
  'spreadMethod',
  'fx',
  'fy',
  'offset',
  'stop-color',
  'stop-opacity',
  'clipPathUnits',
  'maskUnits',
  'maskContentUnits',
  'mask-type',
  'filterUnits',
  'primitiveUnits',
  'color-interpolation-filters',
  'in',
  'in2',
  'result',
  'stdDeviation',
  'dx',
  'dy',
  'flood-color',
  'flood-opacity',
  'type',
  'values',
  'operator',
  'k1',
  'k2',
  'k3',
  'k4',
  'mode',
  'radius',
  'baseFrequency',
  'numOctaves',
  'seed',
  'scale',
  'xChannelSelector',
  'yChannelSelector',
])

/** The attributes that may hold a reference to another element, as `url(#…)`; a `use` names its part as `#…`. */
const REFERRING = new Set(['fill', 'stroke', 'clip-path', 'mask', 'filter'])

function countNodes(nodes: readonly LookNode[]): number {
  let n = 0
  for (const node of nodes) n += 1 + countNodes(node.children ?? [])
  return n
}

/** Draws a look as the app does and checks everything about the markup that the trust rules promise. */
function expectDrawnFromTables(look: Look, prefix: string, tokens: boolean) {
  const clean = cleanIdPrefix(prefix)
  const html = renderToStaticMarkup(createElement('svg', { viewBox: look.viewBox.join(' ') }, lookElements(look, prefix)))
  const all = elements(html)
  expect(all[0].tag).toBe('svg')
  // Every node of the look is drawn, once.
  expect(all).toHaveLength(1 + countNodes(look.nodes))
  const ids = new Set<string>()
  for (const el of all) {
    expect(TAGS.has(el.tag), `<${el.tag}>`).toBe(true)
    if (el.tag !== 'text') expect(el.text.trim(), `text in <${el.tag}>`).toBe('')
    for (const [name, value] of el.attrs) {
      expect(ATTRS.has(name), `${el.tag} ${name}`).toBe(true)
      if (name === 'href') expect(el.tag, `href on <${el.tag}>`).toBe('use')
      if (name === 'id') {
        expect(value.startsWith(clean), `id ${value}`).toBe(true)
        expect(ids.has(value), `id ${value} twice`).toBe(false)
        ids.add(value)
      }
    }
  }
  // Every id of the look is there under the prefix, and nothing else carries an id.
  expect([...ids].sort()).toEqual([...lookIndex(look).keys()].map((id) => clean + id).sort())
  // A reference points to an element of this figure and of nothing else; no other value is a URL.
  let references = 0
  const token = new Set(TOKENS.values())
  for (const el of all) {
    for (const [name, value] of el.attrs) {
      if (value.includes('url(')) {
        expect(REFERRING.has(name), `${el.tag} ${name}="${value}"`).toBe(true)
        const m = /^url\(#([A-Za-z0-9_-]+)\)$/.exec(value)
        expect(m, `${el.tag} ${name}="${value}"`).not.toBeNull()
        expect(ids.has(m?.[1] ?? ''), `${value} resolves`).toBe(true)
        references++
      } else if (name === 'href') {
        const m = /^#([A-Za-z0-9_-]+)$/.exec(value)
        expect(m, `${el.tag} href="${value}"`).not.toBeNull()
        expect(ids.has(m?.[1] ?? ''), `${value} resolves`).toBe(true)
        references++
      } else if (value.includes('var(')) {
        // Only the app's own default look is painted with the theme's tokens.
        expect(tokens && token.has(value), `${el.tag} ${name}="${value}"`).toBe(true)
      } else {
        expect(value, `${el.tag} ${name}`).not.toMatch(/[<>&"'`;{}\\]|javascript:|data:/i)
      }
    }
  }
  return { html, ids, references }
}

// ---------------------------------------------------------------- the golden looks

describe('the server’s example looks', () => {
  it('are there: the two of the start and every one added since', () => {
    expect(GOLDEN.map(([name]) => name)).toEqual(expect.arrayContaining(['pip', 'tock']))
    for (const [name, raw] of GOLDEN) expect(raw.length, name).toBeGreaterThan(100)
  })

  it('are accepted with equal content', () => {
    for (const [name, raw] of GOLDEN) {
      const stored: unknown = JSON.parse(raw)
      const look = checkLook(stored)
      expect(look, name).not.toBeNull()
      // By content, not by text: the checker writes keys in its own order, into records without a prototype.
      expect(look, name).toEqual(stored)
      expect(look, name).not.toBe(stored)
      // And what it gives back is accepted again, unchanged.
      expect(checkLook(JSON.parse(JSON.stringify(look))), name).toEqual(look)
    }
  })

  it('compile, with every animation and state list of their own', () => {
    for (const [name, raw] of GOLDEN) {
      const look = golden(raw)
      const c = compile(look)
      expect(c, name).not.toBeNull()
      if (!c) continue
      for (const animation of Object.keys(look.animations)) {
        expect(resolves(look, animation), `${name} ${animation}`).toBe(true)
        // It moves something: a clip of it changes the pose of at least one part.
        const rest = sample(c, [], 0)
        let moved = false
        for (const t of [120, 400, 900, 1700]) {
          for (const [id, pose] of sample(c, [{ name: animation, startMs: 0, weight: 1, mode: 'once', times: 1 }], t)) {
            const was = rest.get(id)
            if (pose.transform !== was?.transform || [...pose.attrs].some(([k, v]) => was?.attrs.get(k) !== v)) moved = true
          }
        }
        expect(moved, `${name} ${animation} moves nothing`).toBe(true)
      }
      // A look with roles has something to play in every state; one without has only its own animations.
      const roled = Object.keys(look.roles).length > 0
      for (const state of AVATAR_STATES) {
        const list = stateList(look, state)
        if (roled) expect(list.length, `${name} ${state}`).toBeGreaterThan(0)
        else for (const entry of list) expect(Object.hasOwn(look.animations, entry), `${name} ${state} ${entry}`).toBe(true)
        for (const entry of list) expect(resolves(look, entry), `${name} ${state} ${entry}`).toBe(true)
      }
    }
  })

  it('render to markup that holds only allow-listed tags and attributes, and prefixed ids and references', () => {
    for (const [name, raw] of GOLDEN) {
      const look = golden(raw)
      // React's useId gives ":r5:": the prefix is cleaned before it is printed.
      const drawn = expectDrawnFromTables(look, `:r5:${name}-`, false)
      // Pip and Tock paint with gradients and clip; every other example is held to the same rules, whatever it refers to.
      if (name === 'pip' || name === 'tock') expect(drawn.references, name).toBeGreaterThan(0)
      expect(drawn.html).not.toContain(':r5:')
      const first = [...lookIndex(look).keys()][0]
      expect(drawn.html).toContain(`id="r5${name}-${first}"`)
      // Two figures of one look on a page share no id.
      const other = expectDrawnFromTables(look, 'b-', false)
      for (const id of other.ids) expect(drawn.ids.has(id), id).toBe(false)
    }
    expect(expectDrawnFromTables(golden(rawOf('pip')), 'p-', false).html).toContain('id="p-mouth"')
    // Pip's two letters (it snores) are text nodes.
    expect(expectDrawnFromTables(golden(rawOf('pip')), 'p-', false).html.match(/<text [^>]*>[^<]+<\/text>/g)).toHaveLength(2)
  })

  it('give the engine every part it asks for, under the prefix they were drawn with', () => {
    for (const [name, raw] of GOLDEN) {
      const look = golden(raw)
      const { ids } = expectDrawnFromTables(look, 'g1-', false)
      const f = fakeFrames()
      const engine = createEngine(f.host)
      const asked: string[] = []
      const handle = engine.attach(
        look,
        (id) => {
          asked.push(id)
          return ids.has(`g1-${id}`) ? f.find(id) : null
        },
        { state: 'idle', reducedMotion: false, idPrefix: 'g1-' },
      )
      expect(asked.length, name).toBeGreaterThan(name === 'pip' || name === 'tock' ? 5 : 0)
      for (const id of asked) expect(ids.has(`g1-${id}`), `${name} ${id}`).toBe(true)
      // Walking, turning, held and let go: every input the overlay feeds.
      let walk = { pace: 0, turn: 0, held: 0 }
      handle.setInputs(() => walk)
      for (const state of AVATAR_STATES) {
        handle.setState(state)
        f.run(300)
        walk = { pace: walk.pace === 0 ? 1 : 0, turn: walk.turn === 1 ? -1 : 1, held: walk.held === 0 ? 1 : 0 }
        f.run(300)
      }
      expect(handle.frozen(), name).toBe(false)
      handle.detach()
      engine.dispose()
    }
  })

  it('are only ever posed with strings made of numbers', () => {
    for (const [name, raw] of GOLDEN) {
      const look = golden(raw)
      const c = compile(look)
      if (!c) throw new Error(`${name} does not compile`)
      const names = [...Object.keys(look.animations), 'breathe', 'blink', 'think', 'talk', 'listen', 'alert', 'doze', 'wave', 'cheer', 'nod', 'shake']
      let strings = 0
      for (let t = 0; t <= 9000; t += 61) {
        const clips: Clip[] = names.map((n, i) => ({ name: n, startMs: i * 70, weight: ((t + i * 17) % 100) / 100, mode: (['state', 'once', 'hold', 'loop'] as const)[(i + Math.floor(t / 800)) % 4], times: 1 + (i % 3) }))
        const inputs = { mouth: (t % 500) / 500, gaze: [Math.sin(t), Math.cos(t)] as const, pace: (t % 700) / 700, turn: Math.sin(t / 300), held: (t % 900) / 900 }
        for (const [id, pose] of sample(c, clips, t, inputs)) {
          if (pose.transform !== undefined) {
            strings++
            expect(pose.transform, `${name} ${id}`).toMatch(/^[a-zA-Z0-9 ().-]+$/)
          }
          for (const [attr, value] of pose.attrs) {
            if (value === null) continue
            strings++
            expect(value, `${name} ${id} ${attr}`).toMatch(/^[a-zA-Z0-9 (),.-]+$/)
          }
        }
      }
      expect(strings, name).toBeGreaterThan(1000)
    }
  })

  it('have a square face inside their viewBox, around the head when they name one', () => {
    for (const [name, raw] of GOLDEN) {
      const look = golden(raw)
      const [x, y, w, h] = faceViewBox(look)
      expect(w, name).toBe(h)
      expect(w, name).toBeGreaterThan(0)
      expect(w, name).toBeLessThanOrEqual(look.viewBox[2])
      expect(x, name).toBeGreaterThanOrEqual(look.viewBox[0])
      expect(y, name).toBeGreaterThanOrEqual(look.viewBox[1])
      expect(x + w, name).toBeLessThanOrEqual(look.viewBox[0] + look.viewBox[2])
      expect(y + h, name).toBeLessThanOrEqual(look.viewBox[1] + look.viewBox[3])
      // It shows the head, not the whole figure.
      if (look.roles.head !== undefined) expect(w, name).toBeLessThan(look.viewBox[2])
    }
  })
})

describe('the default look', () => {
  it('is drawn from the same tables, painted with the theme’s tokens', () => {
    const drawn = expectDrawnFromTables(defaultLook('t1'), 'd-', true)
    expect(drawn.html).toContain('var(--')
    expect(drawn.references).toBeGreaterThan(0)
  })
})

// ---------------------------------------------------------------- the limits

describe('the limits', () => {
  /** `pub const MAX_NAME: usize = N;`, one per line: the form look.rs promises this test. */
  const consts = new Map<string, number>()
  for (const m of lookRs.matchAll(/^pub const (MAX_[A-Z0-9_]+): usize = (\d+);/gm)) consts.set(m[1], Number(m[2]))

  it('are read from look.rs, every one of them', () => {
    expect(consts.size).toBeGreaterThanOrEqual(22)
    // No limit is written in a form this test cannot read.
    expect(lookRs.match(/^pub const MAX_/gm)).toHaveLength(consts.size)
    for (const [name, value] of consts) expect(Number.isSafeInteger(value) && value > 0, name).toBe(true)
  })

  it('are the server’s: LIMITS', () => {
    const pairs: Record<keyof typeof LIMITS, string> = {
      elements: 'MAX_ELEMENTS',
      depth: 'MAX_DEPTH',
      animations: 'MAX_ANIMATIONS',
      tracks: 'MAX_TRACKS',
      keyframes: 'MAX_KEYFRAMES',
      dChars: 'MAX_D_CHARS',
      pointsChars: 'MAX_POINTS_CHARS',
      texts: 'MAX_TEXTS',
      textChars: 'MAX_TEXT_CHARS',
      stops: 'MAX_STOPS',
      attrs: 'MAX_ATTRS',
    }
    expect(Object.keys(LIMITS).sort()).toEqual(Object.keys(pairs).sort())
    for (const [key, name] of Object.entries(pairs)) {
      expect(consts.has(name), name).toBe(true)
      expect(LIMITS[key as keyof typeof LIMITS], `${key} against ${name}`).toBe(consts.get(name))
    }
  })

  it('are the server’s: MORE_LIMITS', () => {
    const pairs: Record<keyof typeof MORE_LIMITS, string> = {
      targets: 'MAX_TARGETS',
      dKeyframeChars: 'MAX_D_KEYFRAME_CHARS',
      dKeyframesTotal: 'MAX_D_KEYFRAMES_TOTAL',
      stateNames: 'MAX_STATE_NAMES',
      mouthDrives: 'MAX_MOUTH_DRIVES',
      transforms: 'MAX_TRANSFORMS',
      filters: 'MAX_FILTERS',
      filterPrimitives: 'MAX_FILTER_PRIMITIVES',
      dashes: 'MAX_DASHES',
      parents: 'MAX_PARENTS',
      drives: 'MAX_DRIVES',
    }
    expect(Object.keys(MORE_LIMITS).sort()).toEqual(Object.keys(pairs).sort())
    for (const [key, name] of Object.entries(pairs)) {
      expect(consts.has(name), name).toBe(true)
      expect(MORE_LIMITS[key as keyof typeof MORE_LIMITS], `${key} against ${name}`).toBe(consts.get(name))
    }
  })

  it('hold for the example looks with room to spare', () => {
    for (const [name, raw] of GOLDEN) {
      const look = golden(raw)
      expect(countNodes(look.nodes), name).toBeLessThanOrEqual(LIMITS.elements)
      expect(Object.keys(look.animations).length, name).toBeLessThanOrEqual(LIMITS.animations)
      expect(look.mouth.length, name).toBeLessThanOrEqual(MORE_LIMITS.mouthDrives)
      for (const list of Object.values(look.states)) expect(list?.length ?? 0, name).toBeLessThanOrEqual(MORE_LIMITS.stateNames)
      expect(Object.keys(look.parents ?? {}).length, name).toBeLessThanOrEqual(MORE_LIMITS.parents)
      expect((look.drives ?? []).length, name).toBeLessThanOrEqual(MORE_LIMITS.drives)
    }
  })
})
