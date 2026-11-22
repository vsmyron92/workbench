// The look is data from an agent: these tests are mostly attacks. Whatever is off must
// reject the whole look, and what is rendered may hold only tags and attributes of the
// tables, whatever the data says.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { defaultLook } from './defaultLook'
import { richLook, sampleLook, wire } from './fixtures'
import {
  LIMITS,
  MAX_CHAIN_LINKS,
  MORE_LIMITS,
  boxThrough,
  checkLook,
  cleanIdPrefix,
  drivesOn,
  faceViewBox,
  fmt,
  formatPath,
  formatTransform,
  invert,
  isEase,
  isId,
  lookElements,
  lookIndex,
  parseHex,
  parsePath,
  parseTransform,
  propFits,
  rootBox,
  samePathShape,
  trackEases,
  trackTimes,
  transformMatrix,
} from './look'
import type { Look, LookNode } from './types'

type Json = Record<string, unknown>

/** The sample look as it comes off the wire, changed by `change`, through `checkLook`. */
function checked(change?: (look: Json) => void): Look | null {
  const look = wire(sampleLook()) as Json
  change?.(look)
  return checkLook(look)
}

const nodesOf = (look: Json) => look.nodes as Json[]
/** The node with an id, anywhere in the tree. */
function find(look: Json, id: string): Json {
  const walk = (nodes: Json[]): Json | null => {
    for (const node of nodes) {
      if (node.id === id) return node
      const inner = walk((node.children as Json[] | undefined) ?? [])
      if (inner) return inner
    }
    return null
  }
  const found = walk(nodesOf(look))
  if (!found) throw new Error(`no node ${id}`)
  return found
}
const attrsOf = (look: Json, id: string) => find(look, id).attrs as Json
const animation = (look: Json, name: string) => (look.animations as Record<string, Json>)[name]
const track = (look: Json, name: string, index = 0) => (animation(look, name).tracks as Json[])[index]

const render = (look: Look, prefix = 'p-') => renderToStaticMarkup(createElement('svg', null, lookElements(look, prefix)))

const ALLOWED_TAGS = new Set(['svg', 'g', 'path', 'circle', 'ellipse', 'rect', 'line', 'polyline', 'polygon', 'text', 'defs', 'linearGradient', 'radialGradient', 'stop', 'clipPath'])
const ALLOWED_ATTRS = new Set([
  'id',
  'transform',
  'fill',
  'stroke',
  'stroke-width',
  'stroke-linecap',
  'stroke-linejoin',
  'stroke-miterlimit',
  'fill-opacity',
  'stroke-opacity',
  'opacity',
  'fill-rule',
  'clip-rule',
  'clip-path',
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
])

/** Every tag and attribute name of some markup, and every id and reference in it. */
function names(markup: string) {
  const tags = new Set<string>()
  const attrs = new Set<string>()
  const ids: string[] = []
  const refs: string[] = []
  const values: string[] = []
  for (const open of markup.matchAll(/<([A-Za-z][\w:-]*)((?:\s+[^\s=>/]+(?:="[^"]*")?)*)\s*\/?>/g)) {
    tags.add(open[1])
    for (const attr of open[2].matchAll(/\s+([^\s=>/]+)(?:="([^"]*)")?/g)) {
      attrs.add(attr[1])
      values.push(attr[2] ?? '')
      if (attr[1] === 'id') ids.push(attr[2])
      for (const ref of (attr[2] ?? '').matchAll(/url\(#([^)]*)\)/g)) refs.push(ref[1])
    }
  }
  return { tags, attrs, ids, refs, values }
}

function expectOnlyAllowed(markup: string, prefix: string) {
  const found = names(markup)
  for (const tag of found.tags) expect(ALLOWED_TAGS.has(tag), `tag ${tag}`).toBe(true)
  for (const attr of found.attrs) expect(ALLOWED_ATTRS.has(attr), `attribute ${attr}`).toBe(true)
  for (const id of found.ids) expect(id.startsWith(prefix), `id ${id}`).toBe(true)
  for (const ref of found.refs) expect(ref.startsWith(prefix), `reference ${ref}`).toBe(true)
  return found
}

describe('LIMITS', () => {
  it('are the contract values and cannot be changed', () => {
    expect(LIMITS).toEqual({ elements: 300, depth: 10, animations: 32, tracks: 16, keyframes: 16, dChars: 4000, pointsChars: 2000, texts: 6, textChars: 40, stops: 16, attrs: 32 })
    expect(Object.isFrozen(LIMITS)).toBe(true)
  })
})

describe('checkLook accepts', () => {
  it('a stored look, and gives back an equal one', () => {
    const sent = sampleLook()
    const got = checkLook(wire(sent))
    expect(got).not.toBeNull()
    expect(got).toEqual(sent)
  })

  it('a rebuilt object that shares nothing with what came in', () => {
    const input = wire(sampleLook())
    const got = checkLook(input)
    const seen = new Set<object>()
    const collect = (v: unknown) => {
      if (typeof v !== 'object' || v === null) return
      seen.add(v)
      for (const inner of Object.values(v)) collect(inner)
    }
    collect(input)
    let shared = 0
    let objects = 0
    const compare = (v: unknown) => {
      if (typeof v !== 'object' || v === null) return
      objects++
      if (seen.has(v)) shared++
      for (const inner of Object.values(v)) compare(inner)
    }
    compare(got)
    expect(objects).toBeGreaterThan(50)
    expect(shared).toBe(0)
    // Changing what came in afterwards changes nothing in what came out.
    ;(input as Json).viewBox = [0, 0, 1, 1]
    attrsOf(input as Json, 'shadow').fill = 'url(http://example.invalid/x)'
    expect(got?.viewBox).toEqual([0, 0, 200, 200])
    expect(JSON.stringify(got)).not.toContain('example.invalid')
  })

  it('a look with nothing but artwork: the rest gets its empty defaults', () => {
    const got = checkLook({ viewBox: [0, 0, 100, 50], nodes: [{ tag: 'circle', attrs: { cx: 5, cy: 5, r: 2 } }] })
    expect(got).toEqual({ viewBox: [0, 0, 100, 50], nodes: [{ tag: 'circle', attrs: { cx: 5, cy: 5, r: 2 } }], pivots: {}, roles: {}, animations: {}, states: {}, mouth: [], gaze: null })
  })

  it('null where an optional field is simply not there', () => {
    const got = checked((look) => {
      look.gaze = null
      look.pivots = null
      const shadow = find(look, 'shadow')
      shadow.children = null
      shadow.text = null
      animation(look, 'hop').loop = null
      animation(look, 'hop').every = null
      track(look, 'hop').phase = null
    })
    expect(got).not.toBeNull()
    expect(got?.pivots).toEqual({})
    expect(got?.gaze).toBeNull()
  })

  it('negative zero as zero', () => {
    const got = checkLook({ viewBox: [-0, 0, 100, 100], nodes: [{ tag: 'circle', attrs: { cx: -0, cy: 5, r: 2 } }] })
    expect(Object.is(got?.viewBox[0], 0)).toBe(true)
    expect(Object.is(got?.nodes[0].attrs?.cx, 0)).toBe(true)
  })

  it('records without a prototype, so a lookup by a name from data finds nothing inherited', () => {
    const got = checked() as Look
    for (const map of [got.pivots, got.roles, got.animations, got.states]) {
      expect(Object.getPrototypeOf(map)).toBeNull()
      expect((map as Record<string, unknown>)['constructor']).toBeUndefined()
      expect((map as Record<string, unknown>)['toString']).toBeUndefined()
    }
    expect(Object.getPrototypeOf(got.nodes[1].attrs)).toBeNull()
  })
})

describe('checkLook rejects', () => {
  it('anything that is not a JSON object with a viewBox and nodes', () => {
    for (const value of [null, undefined, 1, 'look', true, [], [sampleLook()], new Map(), new Date(), () => sampleLook()]) expect(checkLook(value)).toBeNull()
    expect(checkLook({})).toBeNull()
    expect(checkLook({ viewBox: [0, 0, 200, 200] })).toBeNull()
    expect(checkLook({ nodes: [] })).toBeNull()
    expect(checkLook(Object.create(sampleLook()))).toBeNull()
    // An instance of a class is no JSON object, whatever fields it has.
    class Lookalike {
      viewBox = [0, 0, 200, 200]
      nodes = []
    }
    expect(checkLook(new Lookalike())).toBeNull()
    expect(checkLook({ viewBox: [0, 0, 200, 200], nodes: [new (class Node {
      tag = 'g'
    })()] })).toBeNull()
  })

  it('what throws or changes while it is read', () => {
    const trap = {
      viewBox: [0, 0, 200, 200],
      get nodes(): never {
        throw new Error('boom')
      },
    }
    expect(checkLook(trap)).toBeNull()
    expect(checkLook(new Proxy({}, { ownKeys: () => { throw new Error('boom') } }))).toBeNull()
    expect(checkLook(new Proxy({ viewBox: [0, 0, 200, 200], nodes: [] }, { getPrototypeOf: () => { throw new Error('boom') } }))).toBeNull()
    // A value that reads as one thing the first time and another the second: it is read once.
    let reads = 0
    const shifty = {
      tag: 'circle',
      get attrs() {
        return reads++ === 0 ? { cx: 1 } : { onClick: 'alert(1)' }
      },
    }
    const got = checkLook({ viewBox: [0, 0, 200, 200], nodes: [shifty] })
    expect(got?.nodes[0].attrs).toEqual({ cx: 1 })
    expect(reads).toBe(1)
  })

  it('an unknown key anywhere', () => {
    expect(checked((l) => void (l.svg = '<svg/>'))).toBeNull()
    expect(checked((l) => void (l.name = 'Pip'))).toBeNull()
    expect(checked((l) => void (find(l, 'shadow').onClick = 'x'))).toBeNull()
    expect(checked((l) => void (find(l, 'shadow').style = 'x'))).toBeNull()
    expect(checked((l) => void (animation(l, 'hop').repeat = 3))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').origin = [0, 0]))).toBeNull()
    expect(checked((l) => void ((l.mouth as Json[])[0].origin = [0, 0]))).toBeNull()
  })

  it('a viewBox that is not four sound numbers of 16 to 4096 units', () => {
    for (const viewBox of [[0, 0, 200], [0, 0, 200, 200, 1], [0, 0, 15, 200], [0, 0, 200, 4097], [0, 0, -200, 200], ['0', 0, 200, 200], [0, 0, Number.NaN, 200], [0, 0, Infinity, 200], [1e7, 0, 200, 200], '0 0 200 200', null]) {
      expect(checked((l) => void (l.viewBox = viewBox)), JSON.stringify(viewBox)).toBeNull()
    }
  })

  it('tags outside the table, the names every object inherits included', () => {
    for (const tag of ['script', 'style', 'foreignObject', 'iframe', 'image', 'a', 'use', 'mask', 'pattern', 'symbol', 'marker', 'switch', 'filter', 'animate', 'set', 'svg', 'tspan', 'G', 'constructor', '__proto__', 'toString', 'hasOwnProperty', 'valueOf', '', 1, null, ['g'], { tag: 'g' }]) {
      expect(checked((l) => void (find(l, 'shadow').tag = tag)), String(tag)).toBeNull()
    }
    expect(checked((l) => void delete find(l, 'shadow').tag)).toBeNull()
  })

  it('attribute keys outside the table: handlers, React props, links, inherited names', () => {
    for (const key of ['dangerouslySetInnerHTML', 'children', 'ref', 'key', 'style', 'className', 'class', 'onClick', 'onclick', 'onLoad', 'href', 'xlinkHref', 'xlink:href', 'filter', 'mask', 'strokeDasharray', 'fontFamily', 'tabIndex', 'role', 'data-x', 'aria-label', 'constructor', '__proto__', 'toString', 'hasOwnProperty', 'stroke-width', 'FILL']) {
      const look = wire(sampleLook()) as Json
      // `__proto__` must arrive as an own key, as JSON.parse makes it.
      Object.defineProperty(attrsOf(look, 'shadow'), key, { value: 'x', enumerable: true, configurable: true, writable: true })
      expect(checkLook(look), key).toBeNull()
    }
    const parsed = JSON.parse('{"viewBox":[0,0,200,200],"nodes":[{"tag":"circle","attrs":{"__proto__":{"onClick":"x"},"cx":1}}]}') as unknown
    expect(checkLook(parsed)).toBeNull()
    expect(({} as Json).onClick).toBeUndefined()
  })

  it('an attribute on a tag that does not have it', () => {
    expect(checked((l) => void (attrsOf(l, 'shadow').d = 'M 0 0'))).toBeNull()
    expect(checked((l) => void (attrsOf(l, 'shadow').r = 3))).toBeNull()
    expect(checked((l) => void (attrsOf(l, 'mouth').points = '0 0'))).toBeNull()
    expect(checked((l) => void (attrsOf(l, 'torso').fontSize = 12))).toBeNull()
    expect(checked((l) => void (attrsOf(l, 'skin').fill = '#ff0000'))).toBeNull()
    expect(checked((l) => void (attrsOf(l, 'clip').transform = 'scale(2)'))).toBeNull()
    expect(checked((l) => void ((nodesOf(l)[0] as Json).attrs = { fill: '#000000' }))).toBeNull()
  })

  it('values of the wrong type: objects, arrays, booleans, null', () => {
    for (const value of [{}, { __html: '<img>' }, [], [1], true, false, null, undefined]) {
      for (const [id, key] of [
        ['shadow', 'cx'],
        ['shadow', 'fill'],
        ['shadow', 'opacity'],
        ['mouth', 'd'],
        ['head', 'transform'],
        ['torso', 'fillRef'],
        ['badge', 'points'],
        ['label', 'fontWeight'],
      ]) {
        if (value === undefined) continue
        expect(checked((l) => void (attrsOf(l, id)[key] = value)), `${id}.${key} = ${JSON.stringify(value)}`).toBeNull()
      }
    }
    expect(checked((l) => void (find(l, 'shadow').attrs = []))).toBeNull()
    expect(checked((l) => void (find(l, 'shadow').attrs = 'fill:red'))).toBeNull()
    expect(checked((l) => void (find(l, 'shadow').id = 7))).toBeNull()
    expect(checked((l) => void (find(l, 'label').text = { toString: () => 'x' }))).toBeNull()
    expect(checked((l) => void (find(l, 'label').text = ['x']))).toBeNull()
  })

  it('url(...) and javascript: in any value', () => {
    const bad = ['url(http://example.invalid/x.svg#a)', 'url(#skin)', 'url("javascript:alert(1)")', 'javascript:alert(1)', 'expression(alert(1))', 'red; background: url(x)', 'var(--accent)', 'inherit', 'red', '#12', '#gggggg', 'rgb(1,2,3)']
    for (const value of bad) {
      expect(checked((l) => void (attrsOf(l, 'shadow').fill = value)), `fill ${value}`).toBeNull()
      expect(checked((l) => void (attrsOf(l, 'body').stroke = value)), `stroke ${value}`).toBeNull()
      expect(checked((l) => void (((find(l, 'skin').children as Json[])[0].attrs as Json).stopColor = value)), `stop ${value}`).toBeNull()
      expect(checked((l) => void (track(l, 'blush').values = ['#ff7a93', value, null])), `keyframe ${value}`).toBeNull()
    }
    for (const value of ['url(#x)', 'javascript:alert(1)', 'translate(1) url(x)', 'translate(1);', 'translate(calc(1))', 'translate(1 2 3)', 'rotate(1 2)', 'matrix(1 2 3)', 'perspective(1)', 'translate(1e999)', 'translate(NaN)', 'scale(1) scale(1) scale(1) scale(1) scale(1) scale(1) scale(1) scale(1) scale(1)']) {
      expect(checked((l) => void (attrsOf(l, 'head').transform = value)), `transform ${value}`).toBeNull()
    }
    for (const value of ['M 0 0 url(x)', 'javascript:1', 'M 0 0 <script>', 'M 0 0 L 1e999 1', 'M 0 0 X 1 1', '1 2 M 3 4', 'M 0 0 A 5 5 0 2 1 9 9', 'M 0 0 "']) {
      expect(checked((l) => void (attrsOf(l, 'mouth').d = value)), `d ${value}`).toBeNull()
    }
    for (const value of ['0 0 url(x)', '1,2;3', 'a b', '1 2 NaN']) expect(checked((l) => void (attrsOf(l, 'badge').points = value)), `points ${value}`).toBeNull()
    for (const value of ['url(#clip)', 'clip) url(http://x', '#clip', 'a b', '']) expect(checked((l) => void (attrsOf(l, 'head').clipPathRef = value)), `ref ${value}`).toBeNull()
    for (const value of ['roundish', 'ROUND', 'url(x)']) expect(checked((l) => void (attrsOf(l, 'mouth').strokeLinecap = value)), `cap ${value}`).toBeNull()
  })

  it('numbers that are not finite or out of all proportion', () => {
    for (const value of [Number.NaN, Infinity, -Infinity, 1e7, -1e7, '5', '5px', '1e3']) {
      expect(checked((l) => void (attrsOf(l, 'shadow').cx = value)), String(value)).toBeNull()
    }
    // A stop is a colour or the theme's text colour, never nothing.
    expect(checked((l) => void (((find(l, 'skin').children as Json[])[0].attrs as Json).stopColor = 'none'))).toBeNull()
    expect(checked((l) => void (((find(l, 'skin').children as Json[])[0].attrs as Json).stopColor = 'currentColor'))).not.toBeNull()
    expect(checked((l) => void (attrsOf(l, 'shadow').fill = 'none'))).not.toBeNull()
    expect(checked((l) => void (attrsOf(l, 'shadow').opacity = '50'))).toBeNull()
    // A percentage only where SVG has one (opacities, gradient coordinates, offsets), never for a plain coordinate.
    expect(checked((l) => void (attrsOf(l, 'shadow').opacity = '50%'))).not.toBeNull()
    expect(checked((l) => void (attrsOf(l, 'glow').cx = '50%'))).not.toBeNull()
    expect(checked((l) => void (((find(l, 'skin').children as Json[])[1].attrs as Json).offset = '100%'))).not.toBeNull()
    expect(checked((l) => void (attrsOf(l, 'shadow').cx = '50%'))).toBeNull()
    expect(checked((l) => void (attrsOf(l, 'shadow').opacity = '50%; fill: red'))).toBeNull()
  })

  it('the app’s own token attributes, which only the default look may carry', () => {
    expect(checked((l) => void (attrsOf(l, 'shadow').fillToken = 'accent'))).toBeNull()
    expect(checked((l) => void (attrsOf(l, 'body').strokeToken = 'plot-1'))).toBeNull()
    expect(checked((l) => void (((find(l, 'skin').children as Json[])[0].attrs as Json).stopColorToken = 'fg'))).toBeNull()
    expect(checkLook(wire(defaultLook('t1')))).toBeNull()
  })

  it('two paints for one property', () => {
    expect(checked((l) => void (attrsOf(l, 'torso').fill = '#ff0000'))).toBeNull()
    expect(checked((l) => {
      attrsOf(l, 'body').strokeRef = 'skin'
    })).toBeNull()
  })

  it('ids that are not ids: reserved names, duplicates, odd characters', () => {
    for (const id of ['constructor', 'prototype', 'toString', 'toLocaleString', 'valueOf', 'hasOwnProperty', 'isPrototypeOf', 'propertyIsEnumerable', 'all', 'none', '__proto__', '1abc', 'a b', 'a"b', 'x'.repeat(33), '', 'é']) {
      expect(checked((l) => void (find(l, 'shadow').id = id)), id).toBeNull()
    }
    expect(checked((l) => void (find(l, 'shadow').id = 'body'))).toBeNull()
    expect(checked((l) => void (find(l, 'shadow').id = 'x'.repeat(32)))).not.toBeNull()
  })

  it('references that do not resolve to the right kind of element', () => {
    expect(checked((l) => void (attrsOf(l, 'torso').fillRef = 'nowhere'))).toBeNull()
    expect(checked((l) => void (attrsOf(l, 'torso').fillRef = 'clip'))).toBeNull()
    expect(checked((l) => void (attrsOf(l, 'torso').fillRef = 'body'))).toBeNull()
    expect(checked((l) => void (attrsOf(l, 'head').clipPathRef = 'skin'))).toBeNull()
    expect(checked((l) => void (attrsOf(l, 'head').clipPathRef = 'constructor'))).toBeNull()
  })

  it('elements where the content model has no place for them', () => {
    const circle = { tag: 'circle', attrs: { cx: 1, cy: 1, r: 1 } }
    expect(checked((l) => void (find(l, 'shadow').children = [circle]))).toBeNull()
    expect(checked((l) => void (find(l, 'label').children = [circle]))).toBeNull()
    expect(checked((l) => void nodesOf(l).push({ tag: 'stop', attrs: { offset: 0 } }))).toBeNull()
    expect(checked((l) => void nodesOf(l).push({ tag: 'linearGradient', id: 'loose' }))).toBeNull()
    expect(checked((l) => void nodesOf(l).push({ tag: 'clipPath', id: 'loose', children: [circle] }))).toBeNull()
    expect(checked((l) => void (find(l, 'clip').children as Json[]).push({ tag: 'g' }))).toBeNull()
    expect(checked((l) => void (find(l, 'clip').children as Json[]).push({ tag: 'text', text: 'x' }))).toBeNull()
    expect(checked((l) => void (find(l, 'skin').children as Json[]).push(circle))).toBeNull()
    // Defs hold groups, shapes and text to use, but no use, no defs and no stop of their own.
    expect(checked((l) => void ((nodesOf(l)[0].children as Json[]).push({ tag: 'g' })))).not.toBeNull()
    expect(checked((l) => void ((nodesOf(l)[0].children as Json[]).push({ tag: 'use', attrs: { useRef: 'face' } })))).toBeNull()
    expect(checked((l) => void ((nodesOf(l)[0].children as Json[]).push({ tag: 'defs' })))).toBeNull()
    expect(checked((l) => void ((nodesOf(l)[0].children as Json[]).push({ tag: 'stop', attrs: { offset: 0 } })))).toBeNull()
    // A clip path's own shapes may not be clipped again.
    expect(checked((l) => void (((find(l, 'clip').children as Json[])[0].attrs as Json).clipPathRef = 'clip'))).toBeNull()
    expect(checked((l) => void (find(l, 'shadow').text = 'hello'))).toBeNull()
  })

  it('text that is not one plain line of at most 40 characters', () => {
    const cp = (n: number) => String.fromCodePoint(n)
    for (const text of ['a\nb', 'a\tb', `a${cp(0)}b`, `a${cp(0x1b)}[31m`, `a${cp(0x85)}b`, `a${cp(0x2028)}b`, `a${cp(0x2029)}b`, `a${cp(0x202e)}b`, `a${cp(0x2066)}b`, `a${cp(0x200f)}b`, 'x'.repeat(41), '😀'.repeat(41)]) {
      expect(checked((l) => void (find(l, 'label').text = text)), JSON.stringify(text)).toBeNull()
    }
    expect(checked((l) => void (find(l, 'label').text = '😀'.repeat(40)))?.nodes).toBeDefined()
    expect(checked((l) => void (find(l, 'label').text = '<img src=x onerror=alert(1)>'))).not.toBeNull()
  })

  it('more than the limits allow', () => {
    const circle = () => ({ tag: 'circle', attrs: { cx: 1, cy: 1, r: 1 } })
    const many = (n: number) => Array.from({ length: n }, circle)
    expect(checkLook({ viewBox: [0, 0, 200, 200], nodes: many(LIMITS.elements) })).not.toBeNull()
    expect(checkLook({ viewBox: [0, 0, 200, 200], nodes: many(LIMITS.elements + 1) })).toBeNull()
    expect(checkLook({ viewBox: [0, 0, 200, 200], nodes: [{ tag: 'g', children: many(LIMITS.elements - 1) }] })).not.toBeNull()
    expect(checkLook({ viewBox: [0, 0, 200, 200], nodes: [{ tag: 'g', children: many(LIMITS.elements) }] })).toBeNull()

    const nested = (depth: number): LookNode => (depth <= 1 ? { tag: 'g' } : { tag: 'g', children: [nested(depth - 1)] })
    expect(checkLook({ viewBox: [0, 0, 200, 200], nodes: [nested(LIMITS.depth)] })).not.toBeNull()
    expect(checkLook({ viewBox: [0, 0, 200, 200], nodes: [nested(LIMITS.depth + 1)] })).toBeNull()
    expect(checkLook({ viewBox: [0, 0, 200, 200], nodes: [nested(5000)] })).toBeNull()

    const texts = (n: number) => Array.from({ length: n }, () => ({ tag: 'text', text: 'x' }))
    expect(checkLook({ viewBox: [0, 0, 200, 200], nodes: texts(LIMITS.texts) })).not.toBeNull()
    expect(checkLook({ viewBox: [0, 0, 200, 200], nodes: texts(LIMITS.texts + 1) })).toBeNull()

    const stops = (n: number) => ({ viewBox: [0, 0, 200, 200], nodes: [{ tag: 'defs', children: [{ tag: 'linearGradient', id: 'g1', children: Array.from({ length: n }, () => ({ tag: 'stop', attrs: { offset: 0.5 } })) }] }] })
    expect(checkLook(stops(LIMITS.stops))).not.toBeNull()
    expect(checkLook(stops(LIMITS.stops + 1))).toBeNull()

    const long = (chars: number) => `M 0 0${' L 1 1'.repeat((chars - 5) / 6)}`.padEnd(chars, ' ')
    expect(long(LIMITS.dChars)).toHaveLength(LIMITS.dChars)
    expect(checked((l) => void (attrsOf(l, 'armR').d = long(LIMITS.dChars)))).not.toBeNull()
    expect(checked((l) => void (attrsOf(l, 'armR').d = long(LIMITS.dChars + 1)))).toBeNull()
    expect(checked((l) => void (attrsOf(l, 'badge').points = '1 2 '.repeat(LIMITS.pointsChars / 4)))).not.toBeNull()
    expect(checked((l) => void (attrsOf(l, 'badge').points = `${'1 2 '.repeat(LIMITS.pointsChars / 4)}3`))).toBeNull()

    expect(checked((l) => {
      const all = l.animations as Json
      for (let i = 0; i < LIMITS.animations; i++) all[`extra${i}`] = wire(sampleLook().animations.hop)
    })).toBeNull()
    expect(checked((l) => void (animation(l, 'hop').tracks = Array.from({ length: LIMITS.tracks + 1 }, () => wire(sampleLook().animations.hop.tracks[0]))))).toBeNull()
    expect(checked((l) => {
      animation(l, 'hop').duration = 10
      track(l, 'hop').values = Array.from({ length: LIMITS.keyframes + 1 }, (_, i) => i)
    })).toBeNull()
    const drives = (n: number) => Array.from({ length: n }, () => ({ target: 'mouth', open: 'M 84 102 Q 100 130 116 102' }))
    expect(checked((l) => void (l.mouth = drives(MORE_LIMITS.mouthDrives)))).not.toBeNull()
    expect(checked((l) => void (l.mouth = drives(MORE_LIMITS.mouthDrives + 1)))).toBeNull()
    expect(checked((l) => void ((l.states as Json).idle = ['a', 'b', 'c', 'd', 'e', 'f']))).not.toBeNull()
    expect(checked((l) => void ((l.states as Json).idle = ['a', 'b', 'c', 'd', 'e', 'f', 'g']))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').target = Array.from({ length: MORE_LIMITS.targets }, () => 'body')))).not.toBeNull()
    expect(checked((l) => void (track(l, 'hop').target = Array.from({ length: MORE_LIMITS.targets + 1 }, () => 'body')))).toBeNull()
    // One path keyframe of 1000 characters at most, 6000 in all.
    const key = (chars: number) => `M 0 0${' L 1 1'.repeat((chars - 5) / 6)}`.padEnd(chars, ' ')
    const morph = (chars: number, tracks: number) => (l: Json) => {
      animation(l, 'grin').tracks = Array.from({ length: tracks }, () => ({ target: 'mouth', prop: 'd', values: [key(chars), key(chars)] }))
    }
    expect(checked(morph(MORE_LIMITS.dKeyframeChars, 3))).not.toBeNull()
    expect(checked(morph(MORE_LIMITS.dKeyframeChars + 1, 1))).toBeNull()
    expect(checked(morph(MORE_LIMITS.dKeyframeChars, 4))).toBeNull()
    expect(MORE_LIMITS).toEqual({ targets: 16, dKeyframeChars: 1000, dKeyframesTotal: 6000, stateNames: 6, mouthDrives: 4, transforms: 8, filters: 8, filterPrimitives: 8, dashes: 8, parents: 64, drives: 16 })
    expect(Object.isFrozen(MORE_LIMITS)).toBe(true)
  })
})

describe('checkLook on pivots, roles and states', () => {
  it('takes only names it knows and parts that exist', () => {
    expect(checked((l) => void ((l.roles as Json).tail = 'body'))).toBeNull()
    for (const name of ['constructor', '__proto__', 'toString', 'hasOwnProperty']) {
      expect(checked((l) => void Object.defineProperty(l.roles, name, { value: 'body', enumerable: true })), `role ${name}`).toBeNull()
      expect(checked((l) => void Object.defineProperty(l.states, name, { value: [], enumerable: true })), `state ${name}`).toBeNull()
      expect(checked((l) => void Object.defineProperty(l.pivots, name, { value: [1, 2], enumerable: true })), `pivot ${name}`).toBeNull()
      expect(checked((l) => void Object.defineProperty(l.animations, name, { value: wire(sampleLook().animations.hop), enumerable: true })), `animation ${name}`).toBeNull()
      expect(checked((l) => void ((l.roles as Json).head = name)), `role target ${name}`).toBeNull()
      expect(checked((l) => void ((l.states as Json).idle = [name])), `state entry ${name}`).toBeNull()
      expect(checked((l) => void (track(l, 'hop').target = name)), `track target ${name}`).toBeNull()
      expect(checked((l) => void ((l.mouth as Json[])[0].target = name)), `mouth target ${name}`).toBeNull()
    }
    expect(checked((l) => void ((l.roles as Json).head = 'nowhere'))).toBeNull()
    // Gradients, clip paths and what is inside them are not parts.
    expect(checked((l) => void ((l.roles as Json).head = 'skin'))).toBeNull()
    expect(checked((l) => void ((l.pivots as Json).clip = [1, 2]))).toBeNull()
    expect(checked((l) => void ((l.pivots as Json).nowhere = [1, 2]))).toBeNull()
    expect(checked((l) => void ((l.pivots as Json).body = [1]))).toBeNull()
    expect(checked((l) => void ((l.pivots as Json).body = [1, Number.NaN]))).toBeNull()
    expect(checked((l) => void ((l.pivots as Json).body = { x: 1, y: 2 }))).toBeNull()
    expect(checked((l) => void ((l.states as Json).dancing = ['hop']))).toBeNull()
    expect(checked((l) => void ((l.states as Json).idle = 'hop'))).toBeNull()
    expect(checked((l) => void ((l.states as Json).idle = [1]))).toBeNull()
    expect(checked((l) => void (l.gaze = -1))).toBeNull()
    expect(checked((l) => void (l.gaze = '3'))).toBeNull()
    expect(checked((l) => void (l.gaze = 0))?.gaze).toBe(0)
    // At most 5% of the viewBox width.
    expect(checked((l) => void (l.gaze = 10))?.gaze).toBe(10)
    expect(checked((l) => void (l.gaze = 10.5))).toBeNull()
  })
})

describe('checkLook on animations', () => {
  it('keeps the timing rules of the format', () => {
    expect(checked((l) => void (animation(l, 'hop').duration = 0.05))).toBeNull()
    expect(checked((l) => void (animation(l, 'hop').duration = 31))).toBeNull()
    expect(checked((l) => void (animation(l, 'hop').duration = '1'))).toBeNull()
    expect(checked((l) => void (animation(l, 'hop').loop = 'yes'))).toBeNull()
    expect(checked((l) => void (animation(l, 'peek').loop = true))).toBeNull()
    expect(checked((l) => void (animation(l, 'peek').loop = false))).not.toBeNull()
    for (const every of [[0.5, 3], [3, 2], [3, 121], [3], [3, 4, 5], 'often', [Number.NaN, 4]]) expect(checked((l) => void (animation(l, 'peek').every = every)), JSON.stringify(every)).toBeNull()
    expect(checked((l) => void (animation(l, 'hop').tracks = []))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').phase = 1.5))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').delay = 0.7))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').delay = 0.6))).not.toBeNull()
  })

  it('wants times that start at 0, end at 1 and only go up', () => {
    for (const times of [
      [0, 0.2, 0.2, 0.6, 0.8, 1],
      [0, 0.4, 0.3, 0.6, 0.8, 1],
      [0.1, 0.2, 0.3, 0.6, 0.8, 1],
      [0, 0.2, 0.3, 0.6, 0.8, 0.9],
      [0, 0.2, 0.3, 0.6, 0.8, 1.5],
      [0, 0.5, 1],
      [0, 0.2, 0.2005, 0.6, 0.8, 1],
      [0, 0.2, Number.NaN, 0.6, 0.8, 1],
      'even',
    ]) {
      expect(checked((l) => void (track(l, 'hop').times = times)), JSON.stringify(times)).toBeNull()
    }
    expect(checked((l) => void (animation(l, 'hop').times = [0, 1, 0.5]))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').times = [0, 0.1, 0.3, 0.6, 0.8, 1]))).not.toBeNull()
  })

  it('wants one ease, or one per segment, from the list', () => {
    expect(checked((l) => void (track(l, 'hop').ease = 'bounce'))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').ease = 'constructor'))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').ease = ['in', 'out']))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').ease = ['in', 'out', 'inOut', 'outBack', 'step']))).not.toBeNull()
    expect(checked((l) => void (animation(l, 'hop').ease = ['in', 7]))).toBeNull()
  })

  it('wants a target that exists and a prop that fits it', () => {
    expect(checked((l) => void (track(l, 'hop').target = 'nowhere'))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').target = ['body', 'nowhere']))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').target = []))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').target = 'skin'))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').target = 7))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').prop = 'points'))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').prop = 'constructor'))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').prop = 'd'))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').prop = 'r'))).toBeNull()
    expect(checked((l) => {
      track(l, 'hop').target = 'shadow'
      track(l, 'hop').prop = 'rx'
      track(l, 'hop').values = [46, 40, 46]
    })).not.toBeNull()
    // The same target twice in a list is one target.
    expect(checked((l) => void (track(l, 'peek').target = ['pupils', 'pupils', 'brow']))?.animations.peek.tracks[0].target).toEqual(['pupils', 'brow'])
  })

  it('wants values of the prop’s type and range', () => {
    expect(checked((l) => void (track(l, 'hop').values = [0, '4', 0]))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').values = [0]))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').values = [0, Infinity]))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').values = [0, { valueOf: () => 1 }]))).toBeNull()
    expect(checked((l) => void (track(l, 'hop').values = [0, true]))).toBeNull()
    expect(checked((l) => void (track(l, 'grin').values = [null, 5, null]))).toBeNull()
    expect(checked((l) => {
      track(l, 'hop').prop = 'opacity'
      track(l, 'hop').values = [1, 1.5, 1]
    })).toBeNull()
    expect(checked((l) => {
      track(l, 'hop').prop = 'scale'
      track(l, 'hop').values = [1, 11, 1]
    })).toBeNull()
    expect(checked((l) => {
      track(l, 'hop').prop = 'rotate'
      track(l, 'hop').values = [0, 3601, 0]
    })).toBeNull()
    for (const colour of ['none', 'currentColor', 'url(#skin)', 'transparent', '#fff', 7]) expect(checked((l) => void (track(l, 'blush').values = ['#ff7a93', colour, null])), String(colour)).toBeNull()
  })

  it('wants path keyframes of one structure', () => {
    expect(checked((l) => void (track(l, 'grin').values = ['M 84 102 Q 100 122 116 102', 'M 84 102 L 100 122 L 116 102']))).toBeNull()
    expect(checked((l) => void (track(l, 'grin').values = ['M 84 102 Q 100 122 116 102', 'M 84 102 Q 100 122 116 102 Z']))).toBeNull()
    expect(checked((l) => void (track(l, 'grin').values = ['M 0 0 A 5 5 0 0 1 9 9', 'M 0 0 A 5 5 0 1 1 9 9']))).toBeNull()
    expect(checked((l) => void (track(l, 'grin').values = ['M 0 0 A 5 5 0 0 1 9 9', 'M 1 1 A 6 6 0 0 1 8 8']))).not.toBeNull()
    expect(checked((l) => void (track(l, 'grin').values = ['M 0 0', `M 0 0${' L 1 1'.repeat(400)}`]))).toBeNull()
  })

  it('keeps paint and opacity keyframes 0.17 s apart and never lets them step', () => {
    const flash = (duration: number, values: unknown[], extra?: (t: Json) => void) =>
      checked((l) => {
        animation(l, 'blush').duration = duration
        track(l, 'blush').values = values
        delete track(l, 'blush').times
        extra?.(track(l, 'blush'))
      })
    expect(flash(0.34, ['#000000', '#ffffff', '#000000'])).not.toBeNull()
    expect(flash(0.3, ['#000000', '#ffffff', '#000000'])).toBeNull()
    expect(flash(0.25, ['#000000', '#ffffff', '#000000', '#ffffff', '#000000', '#ffffff', '#000000', '#ffffff', '#000000', '#ffffff', '#000000', '#ffffff', '#000000', '#ffffff', '#000000', '#ffffff'])).toBeNull()
    expect(flash(2, ['#000000', '#ffffff', '#000000'], (t) => void (t.times = [0, 0.05, 1]))).toBeNull()
    expect(flash(2, ['#000000', '#ffffff', '#000000'], (t) => void (t.ease = 'step'))).toBeNull()
    expect(flash(2, ['#000000', '#ffffff', '#000000'], (t) => void (t.ease = ['linear', 'step']))).toBeNull()
    expect(checked((l) => {
      animation(l, 'blush').ease = 'step'
    })).toBeNull()
    const fade = (duration: number) =>
      checked((l) => {
        animation(l, 'hop').duration = duration
        track(l, 'hop').target = 'shadow'
        track(l, 'hop').prop = 'opacity'
        track(l, 'hop').values = [1, 0, 1]
      })
    expect(fade(0.3)).toBeNull()
    expect(fade(0.34)).not.toBeNull()
  })

  it('keeps every other keyframe pair 0.04 s apart', () => {
    expect(checked((l) => void (animation(l, 'hop').duration = 0.1))).toBeNull()
    expect(checked((l) => void (animation(l, 'hop').duration = 0.2))).not.toBeNull()
    // A step is fine for motion.
    expect(checked((l) => void (track(l, 'hop').ease = 'step'))).not.toBeNull()
  })

  it('takes the animation’s own times and ease as the default of its tracks', () => {
    expect(checked((l) => {
      animation(l, 'blush').times = [0, 0.02, 1]
      delete track(l, 'blush').times
    })).toBeNull()
    expect(trackTimes({ values: [1, 2, 3] }, { times: [0, 0.2, 1] })).toEqual([0, 0.2, 1])
    expect(trackTimes({ values: [1, 2, 3], times: [0, 0.7, 1] }, { times: [0, 0.2, 1] })).toEqual([0, 0.7, 1])
    expect(trackTimes({ values: [1, 2, 3, 4, 5] }, { times: [0, 0.2, 1] })).toEqual([0, 0.25, 0.5, 0.75, 1])
    expect(trackEases({ values: [1, 2, 3] }, { ease: 'out' })).toEqual(['out', 'out'])
    expect(trackEases({ values: [1, 2, 3], ease: ['in', 'step'] }, { ease: 'out' })).toEqual(['in', 'step'])
    expect(trackEases({ values: [1, 2, 3] }, { ease: ['in', 'out', 'in'] })).toEqual(['linear', 'linear'])
    expect(trackEases({ values: [1, 2, 3] }, {})).toEqual(['linear', 'linear'])
  })
})

describe('checkLook on the mouth', () => {
  it('takes a path morph or a number, on a part that has the prop', () => {
    expect(checked((l) => void (l.mouth = [{ target: 'mouth', open: 'M 84 102 Q 100 130 116 102', closed: 'M 84 104 Q 100 106 116 104' }]))).not.toBeNull()
    expect(checked((l) => void (l.mouth = [{ target: 'mouth', open: 'M 84 102 Q 100 130 116 102', closed: 'M 84 104 L 116 104' }]))).toBeNull()
    expect(checked((l) => void (l.mouth = [{ target: 'shadow', open: 'M 84 102 Q 100 130 116 102' }]))).toBeNull()
    expect(checked((l) => void (l.mouth = [{ target: 'mouth', open: 5 }]))).toBeNull()
    expect(checked((l) => void (l.mouth = [{ target: 'mouth', open: 'javascript:alert(1)' }]))).toBeNull()
    expect(checked((l) => void (l.mouth = [{ target: 'shadow', prop: 'ry', open: 12, closed: 2 }]))).not.toBeNull()
    expect(checked((l) => void (l.mouth = [{ target: 'shadow', prop: 'scaleY', open: 2 }]))?.mouth).toEqual([{ target: 'shadow', prop: 'scaleY', open: 2 }])
    expect(checked((l) => void (l.mouth = [{ target: 'shadow', prop: 'height', open: 2 }]))).toBeNull()
    expect(checked((l) => void (l.mouth = [{ target: 'shadow', prop: 'opacity', open: 1 }]))).toBeNull()
    expect(checked((l) => void (l.mouth = [{ target: 'shadow', prop: 'scaleY', open: 'wide' }]))).toBeNull()
    expect(checked((l) => void (l.mouth = [{ target: 'shadow', prop: 'scaleY', open: 20 }]))).toBeNull()
    expect(checked((l) => void (l.mouth = { target: 'mouth', open: 'M 0 0' }))).toBeNull()
  })
})

describe('lookElements', () => {
  it('renders a checked look with tags and attributes of the tables only, ids and references prefixed', () => {
    const markup = render(checked() as Look, 'fig7-')
    const found = expectOnlyAllowed(markup, 'fig7-')
    expect([...found.tags].sort()).toEqual([...ALLOWED_TAGS].sort())
    expect(found.ids).toContain('fig7-body')
    expect(found.refs.sort()).toEqual(['fig7-clip', 'fig7-skin', 'fig7-skin'])
    expect(markup).toContain('<circle id="fig7-face" cx="100" cy="80" r="50" fill="url(#fig7-skin)"')
    expect(markup).toContain('clip-path="url(#fig7-clip)"')
    expect(markup).toContain('stroke-width="2.5"')
    expect(markup).toContain('stop-color="#b6f092"')
    expect(markup).toContain('>Pip</text>')
    expect(markup).not.toContain('style=')
    expect(markup).not.toContain('class=')
  })

  it('renders text as text', () => {
    const look = checked((l) => void (find(l, 'label').text = '<img src=x onerror=alert(1)> **b**')) as Look
    const markup = render(look)
    expect(markup).toContain('&lt;img src=x onerror=alert(1)&gt; **b**')
    expect(markup).not.toContain('<img')
    expectOnlyAllowed(markup, 'p-')
  })

  it('keeps to the tables on a hostile but valid look', () => {
    const look = checked((l) => {
      find(l, 'label').text = '"><script>alert(1)</script>'
      find(l, 'shadow').id = 'onload'
      find(l, 'badge').id = 'style'
      attrsOf(l, 'head').transform = 'matrix(1 0 0 1 0 0) translate(1,2) rotate(3) scale(1 -1) skewX(4) skewY(5)'
      ;(l.animations as Json).href = wire(sampleLook().animations.hop)
      ;(l.animations as Json).onClick = wire(sampleLook().animations.hop)
      ;(l.states as Json).idle = ['href', 'onClick', 'className']
    }) as Look
    expect(look).not.toBeNull()
    const markup = render(look, 'x1-')
    const found = expectOnlyAllowed(markup, 'x1-')
    expect(found.ids).toContain('x1-onload')
    expect(found.attrs.has('onload')).toBe(false)
    expect(markup).not.toContain('<script')
    expect(markup).toContain('&lt;script&gt;')
  })

  it('never prints what the tables do not hold, even from data that skipped the check', () => {
    const hostile = {
      viewBox: [0, 0, 200, 200],
      nodes: [
        { tag: 'script', text: 'alert(1)' },
        { tag: 'foreignObject', children: [{ tag: 'iframe', attrs: { src: 'http://example.invalid' } }] },
        { tag: 'constructor' },
        { tag: '__proto__' },
        { tag: 'toString' },
        { tag: 'a', attrs: { href: 'javascript:alert(1)' }, children: [{ tag: 'circle' }] },
        {
          tag: 'circle',
          id: 'x" onload="alert(1)',
          attrs: {
            onClick: 'alert(1)',
            onclick: 'alert(1)',
            style: 'position:fixed;inset:0',
            className: 'wb-modal',
            class: 'wb-modal',
            dangerouslySetInnerHTML: { __html: '<img src=x onerror=alert(1)>' },
            children: 'x',
            ref: 'x',
            key: 'x',
            href: 'http://example.invalid',
            xlinkHref: 'http://example.invalid',
            fill: 'url(http://example.invalid/x)',
            stroke: 'javascript:alert(1)',
            transform: 'translate(0) onload=alert(1)',
            cx: '5" onload="alert(1)',
            cy: { valueOf: () => 5 },
            r: Number.NaN,
            fillRef: 'a) url(http://example.invalid',
            clipPathRef: 'constructor',
            opacity: true,
          },
        },
        { tag: 'path', id: 'ok', attrs: { d: 'M 0 0 L 5 5" onload="alert(1)', strokeLinecap: 'round; fill: red' } },
        { tag: 'text', text: { toString: () => '<b>' }, children: [{ tag: 'script' }] },
        { tag: 'g', id: 'constructor', children: 'not a list' },
        { tag: 'stop', attrs: { stopColor: '#ff0000' } },
        null,
        7,
        'circle',
      ],
    } as unknown as Look
    const markup = render(hostile, 'h-')
    const found = expectOnlyAllowed(markup, 'h-')
    expect([...found.tags].sort()).toEqual(['circle', 'g', 'path', 'svg', 'text'])
    expect([...found.attrs].sort()).toEqual(['id'])
    expect(found.ids).toEqual(['h-ok'])
    for (const word of ['script', 'onload', 'onclick', 'style', 'class', 'href', 'javascript', 'example.invalid', 'iframe', 'alert']) expect(markup, word).not.toContain(word)
    expect(lookElements(null as unknown as Look, 'p-')).toEqual([])
    expect(lookElements({} as Look, 'p-')).toEqual([])
  })

  it('cleans the prefix it is given', () => {
    const markup = render(checked() as Look, ':r1:" onload="x')
    expect(markup).toContain('id="r1onloadxbody"')
    expect(markup).not.toContain('onload=')
    // The same cleaning is there for whoever looks the nodes up again.
    expect(cleanIdPrefix(':r1:" onload="x')).toBe('r1onloadx')
    expect(cleanIdPrefix(':r2:')).toBe('r2')
    expect(cleanIdPrefix('avatar-t1_')).toBe('avatar-t1_')
    expect(cleanIdPrefix('x'.repeat(50))).toHaveLength(32)
    expect(cleanIdPrefix(undefined as unknown as string)).toBe('')
  })

  it('prints the app’s tokens as variables, from its own table only', () => {
    const markup = render(defaultLook('t1'), 'd-')
    expectOnlyAllowed(markup, 'd-')
    expect(markup).toMatch(/fill="var\(--plot-[1-8]\)"/)
    expect(markup).toContain('fill="var(--bg-elevated)"')
    expect(markup).toContain('fill="var(--fg)"')
    expect(markup).toContain('fill="var(--accent)"')
    expect(markup).toMatch(/stroke="var\(--plot-[1-8]\)"/)
    const odd = { viewBox: [0, 0, 200, 200], nodes: [{ tag: 'circle', attrs: { fillToken: 'accent); background: url(x', strokeToken: 'constructor' } }, { tag: 'circle', attrs: { fillToken: 'danger' } }] } as unknown as Look
    expect(render(odd)).toBe('<svg><circle></circle><circle></circle></svg>')
  })

  it('stops at the limits whatever it is given', () => {
    const wordy = { viewBox: [0, 0, 200, 200], nodes: [{ tag: 'text', text: 'x'.repeat(5000) }, { tag: 'text', text: '😀'.repeat(60) }] } as unknown as Look
    expect(render(wordy)).toBe(`<svg><text>${'x'.repeat(LIMITS.textChars)}</text><text>${'😀'.repeat(LIMITS.textChars)}</text></svg>`)
    const many = { viewBox: [0, 0, 200, 200], nodes: Array.from({ length: 2000 }, () => ({ tag: 'circle' })) } as unknown as Look
    expect(render(many).split('<circle').length - 1).toBe(LIMITS.elements)
    let deep: LookNode = { tag: 'g' }
    for (let i = 0; i < 100; i++) deep = { tag: 'g', children: [deep] }
    expect(render({ viewBox: [0, 0, 200, 200], nodes: [deep] } as unknown as Look).split('<g').length - 1).toBe(LIMITS.depth)
  })
})

describe('a storm of hostile looks', () => {
  /** A small deterministic generator, so a failure can be found again. */
  function random(seed: number) {
    let a = seed >>> 0
    return () => {
      a = (a + 0x6d2b79f5) >>> 0
      let t = a
      t = Math.imul(t ^ (t >>> 15), t | 1)
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296
    }
  }
  const PAYLOADS: unknown[] = [
    'url(http://example.invalid/x)',
    'url(#skin)',
    'javascript:alert(1)',
    '"><script>alert(1)</script>',
    "' onload='alert(1)",
    'expression(alert(1))',
    '<img src=x onerror=alert(1)>',
    'constructor',
    '__proto__',
    'toString',
    '',
    'none',
    'M 0 0 L 9 9',
    'translate(1 2)',
    '#ff0000',
    0,
    -1,
    1e9,
    Number.NaN,
    Infinity,
    true,
    false,
    null,
    [],
    [1, 2],
    {},
    { __html: '<b>x</b>' },
    { tag: 'script' },
  ]
  const KEYS = ['dangerouslySetInnerHTML', 'style', 'className', 'onClick', 'onload', 'href', 'xlinkHref', 'children', 'ref', 'key', 'constructor', '__proto__', 'toString', 'fillToken', 'tag', 'attrs', 'id', 'text', 'fill', 'd', 'transform']
  /** What no attribute value may hold, whatever the data said. (Text content may: it is text.) */
  const BAD_VALUES = ['example.invalid', 'javascript', 'onload', 'alert', 'expression', 'script', '<', '>', '"', "'", 'http']
  /** Everything about some markup that the tables do not allow. */
  const violations = (markup: string, prefix: string): string[] => {
    const out: string[] = []
    const found = names(markup)
    for (const tag of found.tags) if (!ALLOWED_TAGS.has(tag)) out.push(`tag ${tag}`)
    for (const attr of found.attrs) if (!ALLOWED_ATTRS.has(attr)) out.push(`attribute ${attr}`)
    for (const id of found.ids) if (!id.startsWith(prefix)) out.push(`id ${id}`)
    for (const ref of found.refs) if (!ref.startsWith(prefix)) out.push(`reference ${ref}`)
    for (const value of found.values) for (const word of BAD_VALUES) if (value.includes(word)) out.push(`${word} in the value ${value}`)
    // No element was opened by a payload: every "<" of the markup starts a tag of the tables or closes one.
    for (const open of markup.matchAll(/<\/?([A-Za-z][\w:-]*)/g)) if (!ALLOWED_TAGS.has(open[1])) out.push(`<${open[1]}`)
    return out
  }

  it('never throws, never shares, and never renders a name or a value outside the tables', () => {
    const rnd = random(20261120)
    const pick = <T,>(list: readonly T[]): T => list[Math.floor(rnd() * list.length)]
    /** Every object and array inside a value. */
    const containers = (root: unknown): object[] => {
      const out: object[] = []
      const walk = (v: unknown) => {
        if (typeof v !== 'object' || v === null) return
        out.push(v)
        for (const inner of Object.values(v)) walk(inner)
      }
      walk(root)
      return out
    }
    let accepted = 0
    let rejected = 0
    for (let round = 0; round < 1200; round++) {
      const look = wire(sampleLook()) as Json
      const hits = 1 + Math.floor(rnd() * 3)
      for (let h = 0; h < hits; h++) {
        const where = pick(containers(look)) as Record<string, unknown>
        const keys = Object.keys(where)
        const mode = rnd()
        if (mode < 0.55 && keys.length > 0) where[pick(keys)] = pick(PAYLOADS)
        else if (mode < 0.85 && !Array.isArray(where)) Object.defineProperty(where, pick(KEYS), { value: pick(PAYLOADS), enumerable: true, configurable: true, writable: true })
        else if (keys.length > 0) delete where[pick(keys)]
      }
      const before = JSON.stringify(look)
      const got = checkLook(look)
      const bad: string[] = JSON.stringify(look) === before ? [] : ['the input was changed']
      if (got) {
        accepted++
        bad.push(...violations(render(got, 'f-'), 'f-'))
      } else {
        rejected++
      }
      // And the renderer by itself, on the data as it came.
      bad.push(...violations(render(look as unknown as Look, 'r-'), 'r-'))
      if (bad.length > 0) expect(bad, `round ${round}: ${before}`).toEqual([])
    }
    // Most changes break the look; a deleted optional field or an equal value does not.
    expect(rejected).toBeGreaterThan(800)
    expect(accepted).toBeGreaterThan(20)
    expect(Object.keys({})).toEqual([])
    expect(({} as Json).fill).toBeUndefined()
  })
})

describe('the geometry of a look', () => {
  it('finds every part and where it sits', () => {
    const look = checked() as Look
    const index = lookIndex(look)
    expect(index.get('body')?.tag).toBe('g')
    expect(index.get('pupils')?.ancestors.map((n) => n.id)).toEqual(['body', 'head', 'eyes'])
    expect(index.get('skin')?.inert).toBe(true)
    expect(index.get('face')?.inert).toBe(false)
    expect(lookIndex(look)).toBe(index)
    // The head is moved up by 2: its box in the viewBox is too.
    expect(rootBox(look, 'head')).toEqual([50, 28, 100, 90])
    expect(rootBox(look, 'cheek')).toEqual([64, 92, 12, 12])
    expect(rootBox(look, 'shadow')).toEqual([54, 173, 92, 14])
    expect(rootBox(look, 'nowhere')).toBeNull()
    expect(rootBox(look, 'skin')).toBeNull()
  })

  it('frames a face: a square around the head with a tenth to spare, inside the viewBox', () => {
    const look = checked() as Look
    // The head is 100 x 90 at (50, 28): a square of 120, centred on it and pushed inside the viewBox.
    expect(faceViewBox(look)).toEqual([40, 13, 120, 120])
    expect(faceViewBox(checked((l) => void delete (l.roles as Json).head) as Look)).toEqual([0, 0, 200, 200])
    const top = checked((l) => void (find(l, 'head').box = [0, 0, 60, 40])) as Look
    expect(faceViewBox(top)).toEqual([0, 0, 72, 72])
    const huge = checked((l) => void (find(l, 'head').box = [-500, -500, 2000, 2000])) as Look
    expect(faceViewBox(huge)).toEqual([0, 0, 200, 200])
    const flat = checked((l) => void (find(l, 'head').box = [10, 10, 0, 0])) as Look
    expect(faceViewBox(flat)).toEqual([0, 0, 200, 200])
    const wide = checkLook({ viewBox: [0, 0, 400, 100], nodes: [{ tag: 'g', id: 'h', box: [150, 10, 100, 80] }], roles: { head: 'h' } }) as Look
    expect(faceViewBox(wide)).toEqual([150, 0, 100, 100])
    expect(faceViewBox(defaultLook('t1'))[2]).toBeLessThan(200)
  })

  it('reads transforms as numbers', () => {
    expect(parseTransform('translate(10 20) rotate(-10 84 82), scale(2)')).toEqual([
      { fn: 'translate', args: [10, 20] },
      { fn: 'rotate', args: [-10, 84, 82] },
      { fn: 'scale', args: [2] },
    ])
    expect(parseTransform('')).toEqual([])
    expect(parseTransform('  ')).toEqual([])
    expect(parseTransform('translate(1,2)skewX(3)')).toHaveLength(2)
    for (const bad of ['translate', 'translate()', 'translate(1 2', 'rotate(1 2)', 'translate(a)', 'url(#a)', 'translate(1) ;', 'translate(1)x', 7, null, 'translate(1) '.repeat(9), `translate(${'1'.repeat(1100)})`]) {
      expect(parseTransform(bad), String(bad)).toBeNull()
    }
    expect(formatTransform(parseTransform('translate(10,20)   rotate(-10.5 84 82)') ?? [])).toBe('translate(10 20) rotate(-10.5 84 82)')
    expect(formatTransform([{ fn: 'translate', args: [Number.NaN] }])).toBeNull()
    const m = transformMatrix(parseTransform('translate(10 20) scale(2)') ?? [])
    expect(boxThrough(m, [1, 1, 2, 3])).toEqual([12, 22, 4, 6])
    const quarter = boxThrough(transformMatrix(parseTransform('rotate(90)') ?? []), [0, 0, 10, 20])
    expect(quarter.map((n) => Math.round(n))).toEqual([-20, 0, 20, 10])
  })

  it('reads path data as a template and numbers', () => {
    const p = parsePath('M 84 128 Q 100 134 116 128 Z')
    expect(p?.commands).toBe('MQZ')
    expect(p?.nums).toEqual([84, 128, 100, 134, 116, 128])
    expect(formatPath(p as NonNullable<typeof p>, [1, 2, 3.14159, 4, 5, 6])).toBe('M 1 2 Q 3.142 4 5 6 Z')
    expect(parsePath('M1-2.5.5,3e1')?.nums).toEqual([1, -2.5, 0.5, 30])
    // The two flags of an arc are numbers of their own, also when written together.
    const arc = parsePath('M 0 0 A 5 5 0 0 1 9 9 5 5 0 10 3 3')
    expect(arc?.nums).toEqual([0, 0, 5, 5, 0, 0, 1, 9, 9, 5, 5, 0, 1, 0, 3, 3])
    expect(arc?.flags.filter(Boolean)).toHaveLength(4)
    expect(parsePath('M 0 0 A 5 5 0 01 9 9')?.nums).toEqual([0, 0, 5, 5, 0, 0, 1, 9, 9])
    expect(formatPath(arc as NonNullable<typeof arc>, (arc as NonNullable<typeof arc>).nums.map((n) => n + 0.4))).toBe('M 0.4 0.4 A 5.4 5.4 0.4 0 1 9.4 9.4 5.4 5.4 0.4 1 0 3.4 3.4')
    expect(parsePath('')?.commands).toBe('')
    for (const bad of ['5 5', 'M 0 0 url(x)', 'M 0 0 B 1 1', 'M 0 0 A 5 5 0 2 1 9 9', 'M 1e999 0', 7, null, {}]) expect(parsePath(bad), String(bad)).toBeNull()
    const a = parsePath('M 0 0 L 1 1') as NonNullable<ReturnType<typeof parsePath>>
    expect(samePathShape(a, parsePath('M 5 5 L 9 9') as typeof a)).toBe(true)
    expect(samePathShape(a, parsePath('M 5 5 l 9 9') as typeof a)).toBe(false)
    expect(samePathShape(a, parsePath('M 5 5 L 9 9 9 9') as typeof a)).toBe(false)
    expect(formatPath(a, [0, 0, Number.NaN, 1])).toBeNull()
  })

  it('formats numbers without exponents, without "-0", and refuses what is not finite', () => {
    expect(fmt(1.5, 2)).toBe('1.5')
    expect(fmt(2, 3)).toBe('2')
    expect(fmt(-0, 2)).toBe('0')
    expect(fmt(-0.0001, 2)).toBe('0')
    expect(fmt(1e-9, 3)).toBe('0')
    expect(fmt(123456789.125, 2)).toBe('123456789.13')
    expect(fmt(100, 0)).toBe('100')
    expect(fmt(Number.NaN, 2)).toBeNull()
    expect(fmt(Infinity, 2)).toBeNull()
    expect(fmt(1e21, 2)).toBeNull()
  })

  it('knows names, colours and which prop fits which element', () => {
    expect(isId('armR')).toBe(true)
    for (const bad of ['constructor', '__proto__', 'hasOwnProperty', 'a b', '', 7, null, 'x'.repeat(33)]) expect(isId(bad), String(bad)).toBe(false)
    expect(parseHex('#ff8000')).toEqual([255, 128, 0, 1])
    expect(parseHex('#FF800080')?.[3]).toBeCloseTo(0.502, 2)
    for (const bad of ['#fff', 'red', 'none', 'url(#a)', 7, null]) expect(parseHex(bad)).toBeNull()
    expect(propFits('d', 'path')).toBe(true)
    expect(propFits('d', 'circle')).toBe(false)
    expect(propFits('x', 'text')).toBe(true)
    expect(propFits('r', 'ellipse')).toBe(false)
    expect(propFits('rotate', 'g')).toBe(true)
    expect(propFits('rotate', 'stop')).toBe(false)
    expect(propFits('constructor' as 'd', 'path')).toBe(false)
  })
})

// ---------------------------------------------------------------- the richer artwork and the free rig

/** The rich look as it comes off the wire, changed by `change`, through `checkLook`. */
function rich(change?: (look: Json) => void): Look | null {
  const look = wire(richLook()) as Json
  change?.(look)
  return checkLook(look)
}
const defsOf = (look: Json) => (look.nodes as Json[])[0].children as Json[]
const filterSteps = (look: Json, id: string) => find(look, id).children as Json[]
const drivesOf = (look: Json) => look.drives as Json[]
const parentsOf = (look: Json) => look.parents as Json
const circle = (over: Json = {}) => ({ tag: 'circle', attrs: { cx: 1, cy: 1, r: 1 }, ...over })

const RICH_TAGS = new Set([...ALLOWED_TAGS, 'use', 'mask', 'filter', 'feGaussianBlur', 'feDropShadow', 'feOffset', 'feFlood', 'feColorMatrix', 'feComposite', 'feBlend', 'feMerge', 'feMergeNode', 'feMorphology', 'feTurbulence', 'feDisplacementMap'])
const RICH_ATTRS = new Set([
  ...ALLOWED_ATTRS,
  'href',
  'mask',
  'filter',
  'stroke-dasharray',
  'stroke-dashoffset',
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

/** Every tag and attribute of some markup is in the rich tables; ids, references and hrefs carry the prefix. */
function expectOnlyRich(markup: string, prefix: string) {
  const found = names(markup)
  for (const tag of found.tags) expect(RICH_TAGS.has(tag), `tag ${tag}`).toBe(true)
  for (const attr of found.attrs) expect(RICH_ATTRS.has(attr), `attribute ${attr}`).toBe(true)
  for (const id of found.ids) expect(id.startsWith(prefix), `id ${id}`).toBe(true)
  for (const ref of found.refs) expect(ref.startsWith(prefix), `reference ${ref}`).toBe(true)
  for (const href of markup.matchAll(/ href="([^"]*)"/g)) expect(href[1].startsWith(`#${prefix}`), `href ${href[1]}`).toBe(true)
  return found
}

describe('checkLook on use, masks, filters and dashes', () => {
  it('accepts them, and gives back an equal look', () => {
    const sent = richLook()
    const got = checkLook(wire(sent))
    expect(got).not.toBeNull()
    expect(got).toEqual(sent)
    expect(checkLook(wire(got))).toEqual(got)
    // A use with an id is a part: animated, turned about its pivot, given a role, a parent, a drive.
    expect(rich((l) => void ((l.pivots as Json).leafL = [60, 125]))).not.toBeNull()
    expect(rich((l) => void (l.roles = { armL: 'leafL' }))).not.toBeNull()
    expect(rich((l) => void (parentsOf(l).leafL = 'hand'))).not.toBeNull()
  })

  it('a use only of a group, a shape or a text of the look that is drawn by itself and holds no use', () => {
    const useOf = (target: string, over: Json = {}) => ({ tag: 'use', attrs: { useRef: target, ...over } })
    expect(rich((l) => void nodesOf(l).push(useOf('torso')))).not.toBeNull()
    expect(rich((l) => void nodesOf(l).push(useOf('head', { x: -3, y: 4, opacity: 0.5, strokeDasharray: '2 1' })))).not.toBeNull()
    // A target after the use is a target too.
    expect(rich((l) => void nodesOf(l).unshift(useOf('hand')))).not.toBeNull()
    for (const target of ['nowhere', 'fade', 'vignette', 'soft', 'leafL', 'constructor', 'url(#leaf)', '#leaf', '']) {
      expect(rich((l) => void nodesOf(l).push(useOf(target))), target).toBeNull()
    }
    // Inside a clip path or a mask: nothing to repeat.
    expect(rich((l) => {
      defsOf(l).push({ tag: 'clipPath', id: 'cut', children: [circle({ id: 'hole' })] })
      nodesOf(l).push(useOf('hole'))
    })).toBeNull()
    expect(rich((l) => {
      ;(find(l, 'vignette').children as Json[]).push(circle({ id: 'spot' }))
      nodesOf(l).push(useOf('spot'))
    })).toBeNull()
    // Around the use, or holding a use: it would repeat itself.
    expect(rich((l) => void (find(l, 'spin').children as Json[]).push(useOf('spin')))).toBeNull()
    expect(rich((l) => void (find(l, 'spin').children as Json[]).push(useOf('body')))).toBeNull()
    expect(rich((l) => {
      defsOf(l).push({ tag: 'g', id: 'pair', children: [useOf('leaf')] })
      nodesOf(l).push(useOf('pair'))
    })).toBeNull()
    // A use names what it shows, and by nothing but `useRef`; it has no children and no size of its own.
    expect(rich((l) => void nodesOf(l).push({ tag: 'use' }))).toBeNull()
    expect(rich((l) => void nodesOf(l).push({ tag: 'use', attrs: { x: 1 } }))).toBeNull()
    for (const key of ['href', 'xlinkHref', 'xlink:href', 'width', 'height', 'r', 'd']) expect(rich((l) => void (attrsOf(l, 'leafL')[key] = key === 'r' ? 3 : '#leaf')), key).toBeNull()
    expect(rich((l) => void (find(l, 'leafL').children = [circle()]))).toBeNull()
    // Not straight in defs, a clip path or a mask.
    expect(rich((l) => void defsOf(l).push(useOf('leaf')))).toBeNull()
    expect(rich((l) => void (find(l, 'vignette').children as Json[]).push(useOf('leaf')))).toBeNull()
  })

  it('counts a use with what it shows against the element limit', () => {
    const look = (circles: number, uses: number, shown: number) => ({
      viewBox: [0, 0, 200, 200],
      nodes: [
        { tag: 'defs', children: [{ tag: 'g', id: 'blob', children: Array.from({ length: shown - 1 }, () => circle()) }] },
        ...Array.from({ length: circles }, () => circle()),
        ...Array.from({ length: uses }, () => ({ tag: 'use', attrs: { useRef: 'blob' } })),
      ],
    })
    // defs, the group of 10 and its circles: 11 elements; each use 1 and the 10 it shows.
    expect(checkLook(look(300 - 11 - 3 * 11, 3, 10))).not.toBeNull()
    expect(checkLook(look(300 - 11 - 3 * 11 + 1, 3, 10))).toBeNull()
    expect(checkLook(look(0, 26, 10))).not.toBeNull()
    expect(checkLook(look(0, 27, 10))).toBeNull()
  })

  it('a mask in defs, of groups and shapes that use gradients and nothing else', () => {
    const mask = (l: Json) => find(l, 'vignette')
    expect(rich((l) => void (mask(l).attrs = { maskType: 'alpha', x: '-10%', y: '-10%', width: '120%', height: '120%', maskUnits: 'objectBoundingBox', maskContentUnits: 'objectBoundingBox' }))).not.toBeNull()
    for (const [key, value] of [['maskType', 'luminosity'], ['maskType', 'ALPHA'], ['maskUnits', 'screen'], ['x', 'auto'], ['width', '10px'], ['mask-type', 'alpha'], ['transform', 'scale(2)'], ['fill', '#ffffff'], ['opacity', 0.5]] as const) {
      expect(rich((l) => void ((mask(l).attrs as Json)[key] = value)), `${key} ${value}`).toBeNull()
    }
    for (const child of [{ tag: 'text', text: 'x' }, { tag: 'use', attrs: { useRef: 'leaf' } }, { tag: 'defs' }, { tag: 'mask', id: 'inner' }, { tag: 'clipPath', id: 'inner' }, { tag: 'g', children: [{ tag: 'text', text: 'x' }] }, { tag: 'g', children: [{ tag: 'defs' }] }]) {
      expect(rich((l) => void (mask(l).children as Json[]).push(child)), JSON.stringify(child)).toBeNull()
    }
    expect(rich((l) => void (mask(l).children as Json[]).push({ tag: 'g', children: [{ tag: 'g', children: [circle({ attrs: { cx: 1, cy: 1, r: 1, strokeRef: 'fade' } })] }] }))).not.toBeNull()
    // What a mask is made of is not clipped, masked or filtered itself; nor is what a clip path is made of.
    for (const key of ['clipPathRef', 'maskRef', 'filterRef']) {
      const value = key === 'clipPathRef' ? 'cut' : key === 'maskRef' ? 'vignette' : 'soft'
      expect(rich((l) => {
        defsOf(l).push({ tag: 'clipPath', id: 'cut', children: [circle()] })
        ;(((mask(l).children as Json[])[0].attrs as Json)[key] = value)
      }), `mask ${key}`).toBeNull()
      expect(rich((l) => {
        defsOf(l).push({ tag: 'clipPath', id: 'cut', children: [circle({ attrs: { cx: 1, cy: 1, r: 1, [key]: value } })] })
      }), `clip ${key}`).toBeNull()
    }
    // A mask belongs in defs, and a reference names a mask of this look.
    expect(rich((l) => void nodesOf(l).push({ tag: 'mask', id: 'loose', children: [circle()] }))).toBeNull()
    expect(rich((l) => void (find(l, 'spin').children as Json[]).push({ tag: 'mask', id: 'loose' }))).toBeNull()
    for (const ref of ['soft', 'fade', 'nowhere', 'url(#vignette)', 'constructor']) expect(rich((l) => void (attrsOf(l, 'body').maskRef = ref)), ref).toBeNull()
    expect(rich((l) => void (attrsOf(l, 'hand').maskRef = 'vignette'))).not.toBeNull()
    expect(rich((l) => void (attrsOf(l, 'leafL').maskRef = 'vignette'))).not.toBeNull()
  })

  it('a filter in defs, of the listed primitives with the listed attributes', () => {
    const steps = (l: Json) => filterSteps(l, 'soft')
    const add = (step: Json) => (l: Json) => void steps(l).splice(0, 0, step)
    for (const tag of ['feImage', 'feTile', 'feSpotLight', 'fePointLight', 'feDistantLight', 'feDiffuseLighting', 'feSpecularLighting', 'feComponentTransfer', 'feFuncA', 'feConvolveMatrix', 'animate', 'set', 'script']) {
      expect(rich(add({ tag })), tag).toBeNull()
    }
    // A primitive belongs in a filter, a merge node in a merge, a filter in defs.
    expect(rich((l) => void nodesOf(l).push({ tag: 'feGaussianBlur', attrs: { stdDeviation: '1' } }))).toBeNull()
    expect(rich((l) => void defsOf(l).push({ tag: 'feFlood' }))).toBeNull()
    expect(rich(add({ tag: 'feMergeNode', attrs: { in: 'SourceGraphic' } }))).toBeNull()
    expect(rich(add({ tag: 'feOffset', children: [{ tag: 'feMergeNode' }] }))).toBeNull()
    expect(rich(add({ tag: 'feMerge', children: [circle()] }))).toBeNull()
    expect(rich((l) => void nodesOf(l).push({ tag: 'filter', id: 'loose' }))).toBeNull()
    expect(rich((l) => void steps(l).push(circle()))).toBeNull()
    // Attributes of the primitive's own list only (no region of their own, no presentation).
    for (const [tag, key, value] of [
      ['feOffset', 'stdDeviation', '1'],
      ['feFlood', 'in', 'SourceGraphic'],
      ['feTurbulence', 'in', 'SourceGraphic'],
      ['feGaussianBlur', 'x', 0],
      ['feGaussianBlur', 'fill', '#ff0000'],
      ['feGaussianBlur', 'transform', 'scale(2)'],
      ['feBlend', 'operator', 'over'],
      ['feComposite', 'mode', 'multiply'],
      ['feMergeNode', 'result', 'x'],
      ['feImage', 'href', '#leaf'],
    ] as const) {
      expect(rich(add({ tag, attrs: { [key]: value } })), `${tag} ${key}`).toBeNull()
    }
    for (const filterKey of ['fill', 'transform', 'href', 'filterRes', 'maskType']) expect(rich((l) => void ((find(l, 'soft').attrs as Json)[filterKey] = 'x')), filterKey).toBeNull()
    expect(rich((l) => void ((find(l, 'soft').attrs as Json).colorInterpolationFilters = 'linearRGB'))).not.toBeNull()
    expect(rich((l) => void ((find(l, 'soft').attrs as Json).colorInterpolationFilters = 'auto'))).toBeNull()
    // Every value as its kind wants it.
    const one = (tag: string, attrs: Json) => rich(add({ tag, attrs }))
    expect(one('feComposite', { operator: 'arithmetic', k1: 0.5, k2: -1, k3: 2, k4: 0 })).not.toBeNull()
    expect(one('feComposite', { operator: 'lighter' })).toBeNull()
    expect(one('feComposite', { k1: '0.5' })).toBeNull()
    expect(one('feMorphology', { operator: 'arithmetic' })).toBeNull()
    for (const mode of ['normal', 'multiply', 'screen', 'overlay', 'darken', 'lighten', 'color-dodge', 'color-burn', 'hard-light', 'soft-light', 'difference', 'exclusion']) expect(one('feBlend', { mode }), mode).not.toBeNull()
    for (const mode of ['hue', 'luminosity', 'plus-lighter', 'Multiply']) expect(one('feBlend', { mode }), mode).toBeNull()
    for (const type of ['matrix', 'saturate', 'hueRotate', 'luminanceToAlpha']) expect(one('feColorMatrix', { type }), type).not.toBeNull()
    expect(one('feColorMatrix', { type: 'fractalNoise' })).toBeNull()
    expect(one('feTurbulence', { type: 'saturate' })).toBeNull()
    expect(one('feColorMatrix', { values: Array.from({ length: 20 }, (_, i) => i / 10).join(' ') })).not.toBeNull()
    expect(one('feColorMatrix', { values: Array.from({ length: 21 }, (_, i) => i / 10).join(' ') })).toBeNull()
    expect(one('feColorMatrix', { values: 0.5 })).not.toBeNull()
    for (const values of ['1,0', '1  0', ' 1', '1 ', '', 'a', '1 NaN', '1e999']) expect(one('feColorMatrix', { values }), JSON.stringify(values)).toBeNull()
    for (const [key, ok, bad] of [['xChannelSelector', 'A', 'X'], ['yChannelSelector', 'B', 'r']] as const) {
      expect(one('feDisplacementMap', { [key]: ok }), ok).not.toBeNull()
      expect(one('feDisplacementMap', { [key]: bad }), bad).toBeNull()
    }
    expect(one('feFlood', { floodColor: 'currentColor', floodOpacity: '50%' })).not.toBeNull()
    for (const colour of ['none', 'red', '#fff', 'url(#fade)', 'var(--accent)']) expect(one('feFlood', { floodColor: colour }), colour).toBeNull()
    expect(one('feTurbulence', { seed: -4.5 })).not.toBeNull()
  })

  it('a filter’s numbers within what the viewBox allows', () => {
    const one = (tag: string, attrs: Json, viewBox = [0, 0, 200, 200]) =>
      rich((l) => {
        l.viewBox = viewBox
        filterSteps(l, 'soft').unshift({ tag, attrs })
      })
    // A blur and a displacement up to a quarter of the width, a morphology up to a tenth; a frequency 0 to 1; 1 to 4 octaves.
    expect(one('feGaussianBlur', { stdDeviation: '50' })).not.toBeNull()
    expect(one('feGaussianBlur', { stdDeviation: '50 0' })).not.toBeNull()
    expect(one('feGaussianBlur', { stdDeviation: 50 })).not.toBeNull()
    for (const v of ['51', '50.01', '-1', '1 2 3', 51]) expect(one('feGaussianBlur', { stdDeviation: v }), String(v)).toBeNull()
    expect(one('feGaussianBlur', { stdDeviation: '100' }, [0, 0, 400, 200])).not.toBeNull()
    expect(one('feDropShadow', { stdDeviation: '51' })).toBeNull()
    expect(one('feMorphology', { radius: '20 10' })).not.toBeNull()
    for (const v of ['21', '1 2 3', 21]) expect(one('feMorphology', { radius: v }), String(v)).toBeNull()
    expect(one('feDisplacementMap', { scale: -50 })).not.toBeNull()
    for (const v of [51, -51, '4']) expect(one('feDisplacementMap', { scale: v }), String(v)).toBeNull()
    expect(one('feTurbulence', { baseFrequency: '0 1' })).not.toBeNull()
    for (const v of ['1.1', '-0.1', '0.1 0.2 0.3']) expect(one('feTurbulence', { baseFrequency: v }), v).toBeNull()
    for (const n of [1, 4]) expect(one('feTurbulence', { numOctaves: n }), String(n)).not.toBeNull()
    for (const n of [0, 5, 1.5, '2', -1]) expect(one('feTurbulence', { numOctaves: n }), String(n)).toBeNull()
  })

  it('a primitive’s in and in2: the source graphic, its alpha, or the result of an earlier primitive of the same filter', () => {
    const steps = (l: Json) => filterSteps(l, 'soft')
    const inOf = (l: Json, i: number) => steps(l)[i].attrs as Json
    expect(rich((l) => void (inOf(l, 0).in = 'SourceGraphic'))).not.toBeNull()
    // Later, unknown, another filter's, a DOM id, a name SVG has but an avatar does not draw.
    for (const name of ['shadow', 'nowhere', 'noise', 'leaf', 'BackgroundImage', 'FillPaint', 'url(#blur)', 'constructor', '']) {
      expect(rich((l) => void (inOf(l, 1).in = name)), name).toBeNull()
    }
    expect(rich((l) => void (inOf(l, 3).in2 = 'shadow'))).toBeNull()
    expect(rich((l) => void (((steps(l)[4].children as Json[])[0].attrs as Json).in = 'later'))).toBeNull()
    // A result is a name of the id pattern.
    for (const name of ['1x', 'a b', 'constructor', 'x'.repeat(33)]) expect(rich((l) => void (inOf(l, 0).result = name)), name).toBeNull()
    // The same name twice: the later one counts from where it is given.
    expect(rich((l) => {
      inOf(l, 2).result = 'blur'
      inOf(l, 3).in = 'blur'
    })).not.toBeNull()
    // Results are the filter's own: they are no ids of the look, and may share a name with one.
    expect(rich((l) => {
      inOf(l, 0).result = 'leaf'
      inOf(l, 1).in = 'leaf'
    })).not.toBeNull()
  })

  it('at most 8 primitives in a filter (merge nodes not counted), 8 nodes in a merge, 8 filters in a look', () => {
    const blur = () => ({ tag: 'feGaussianBlur', attrs: { stdDeviation: '1' } })
    const node = () => ({ tag: 'feMergeNode', attrs: { in: 'SourceGraphic' } })
    const filter = (id: string, primitives: number, nodes = 0) => ({ tag: 'filter', id, children: [...Array.from({ length: primitives - (nodes > 0 ? 1 : 0) }, blur), ...(nodes > 0 ? [{ tag: 'feMerge', children: Array.from({ length: nodes }, node) }] : [])] })
    const withFilter = (f: Json) => rich((l) => void defsOf(l).push(f))
    expect(withFilter(filter('more', MORE_LIMITS.filterPrimitives))).not.toBeNull()
    expect(withFilter(filter('more', MORE_LIMITS.filterPrimitives + 1))).toBeNull()
    expect(withFilter(filter('more', MORE_LIMITS.filterPrimitives, 8))).not.toBeNull()
    expect(withFilter(filter('more', 2, 9))).toBeNull()
    // The rich look has two already.
    const filters = (n: number) => rich((l) => void defsOf(l).push(...Array.from({ length: n }, (_, i) => filter(`f${i}`, 1))))
    expect(filters(MORE_LIMITS.filters - 2)).not.toBeNull()
    expect(filters(MORE_LIMITS.filters - 1)).toBeNull()
  })

  it('a filter only by reference to a filter element of the look, never a CSS filter', () => {
    for (const ref of ['blur(2px)', 'url(#soft)', 'grayscale(1)', 'vignette', 'fade', 'nowhere', 'none']) expect(rich((l) => void (attrsOf(l, 'body').filterRef = ref)), ref).toBeNull()
    expect(rich((l) => void (attrsOf(l, 'torso').filterRef = 'grain'))).not.toBeNull()
    expect(rich((l) => void (attrsOf(l, 'body').filter = 'url(#soft)'))).toBeNull()
  })

  it('dashes: up to 8 numbers of 0 or more, or none; an offset is a number', () => {
    const dash = (value: unknown) => rich((l) => void (attrsOf(l, 'torso').strokeDasharray = value))
    for (const ok of ['4', '4 2', '0 0', '1 2 3 4 5 6 7 8', 'none', 3, 0, '0.5 1.25']) expect(dash(ok), String(ok)).not.toBeNull()
    for (const bad of ['1 2 3 4 5 6 7 8 9', '-1 2', '1,2', '1  2', ' 1', '1 ', '', 'auto', '4px', -1, '1e999', null]) {
      if (bad === null) continue
      expect(dash(bad), String(bad)).toBeNull()
    }
    expect(rich((l) => void (attrsOf(l, 'torso').strokeDashoffset = -7.5))).not.toBeNull()
    expect(rich((l) => void (attrsOf(l, 'torso').strokeDashoffset = '7'))).toBeNull()
    // On groups, text and uses too.
    expect(rich((l) => void (attrsOf(l, 'body').strokeDasharray = '3 3'))).not.toBeNull()
    expect(rich((l) => void (attrsOf(l, 'leafL').strokeDashoffset = 2))).not.toBeNull()
    expect(checkLook({ viewBox: [0, 0, 100, 100], nodes: [{ tag: 'text', attrs: { strokeDasharray: '1 1' }, text: 'x' }] })).not.toBeNull()
    // Not on a gradient, a stop, a mask or a filter.
    expect(rich((l) => void ((find(l, 'fade').attrs as Json).strokeDasharray = '1'))).toBeNull()
    expect(rich((l) => void ((find(l, 'vignette').attrs as Json).strokeDasharray = '1'))).toBeNull()
  })
})

describe('checkLook on parents', () => {
  it('takes part to part, neither inside the other', () => {
    expect(rich()?.parents).toEqual({ hand: 'head', leafR: 'torso' })
    expect(rich((l) => void (parentsOf(l).spin = 'torso'))).not.toBeNull()
    // Nested in its parent, or the parent nested in it.
    expect(rich((l) => void (parentsOf(l).head = 'spin'))).toBeNull()
    expect(rich((l) => void (parentsOf(l).head = 'body'))).toBeNull()
    expect(rich((l) => void (parentsOf(l).body = 'head'))).toBeNull()
    expect(rich((l) => void (parentsOf(l).hand = 'hand'))).toBeNull()
    // Parts only: nothing in defs, no gradient, mask or filter, nothing that is not there.
    for (const name of ['leaf', 'fade', 'vignette', 'soft', 'nowhere', 'constructor']) {
      expect(rich((l) => void (parentsOf(l).hand = name)), `parent ${name}`).toBeNull()
      expect(rich((l) => void Object.defineProperty(parentsOf(l), name, { value: 'torso', enumerable: true })), `child ${name}`).toBeNull()
    }
    for (const value of [7, null, ['torso'], { id: 'torso' }]) {
      if (value === null) continue
      expect(rich((l) => void (parentsOf(l).hand = value)), JSON.stringify(value)).toBeNull()
    }
    expect(rich((l) => void (l.parents = ['hand', 'head']))).toBeNull()
    // Absent, null or empty: none, and nothing is written for it.
    expect(rich((l) => void delete l.parents)).not.toHaveProperty('parents')
    expect(rich((l) => void (l.parents = null))).not.toHaveProperty('parents')
    expect(rich((l) => void (l.parents = {}))).not.toHaveProperty('parents')
  })

  it('no cycle, no chain longer than eight links, at most 64 entries', () => {
    const chainLook = (links: number, extra: Record<string, string> = {}) => {
      const ids = Array.from({ length: links + 1 }, (_, i) => `p${i}`)
      const parents: Record<string, string> = { ...extra }
      for (let i = 0; i < links; i++) parents[ids[i]] = ids[i + 1]
      return checkLook({ viewBox: [0, 0, 200, 200], nodes: ids.map((id) => circle({ id, box: [0, 0, 2, 2] })), parents })
    }
    expect(MAX_CHAIN_LINKS).toBe(8)
    expect(chainLook(MAX_CHAIN_LINKS)).not.toBeNull()
    expect(chainLook(MAX_CHAIN_LINKS + 1)).toBeNull()
    expect(chainLook(2, { p2: 'p0' })).toBeNull()
    expect(chainLook(1, { p1: 'p0' })).toBeNull()
    const many = (n: number) => {
      const nodes = Array.from({ length: n + 1 }, (_, i) => circle({ id: `c${i}` }))
      const parents: Record<string, string> = {}
      for (let i = 1; i <= n; i++) parents[`c${i}`] = 'c0'
      return checkLook({ viewBox: [0, 0, 200, 200], nodes, parents })
    }
    expect(many(MORE_LIMITS.parents)).not.toBeNull()
    expect(many(MORE_LIMITS.parents + 1)).toBeNull()
  })
})

describe('checkLook on drives', () => {
  it('takes an input, parts, a prop that fits them, and values like a track’s', () => {
    expect(rich()?.drives).toEqual(richLook().drives)
    const drive = (over: Json) => rich((l) => void drivesOf(l).push({ input: 'pace', target: 'torso', prop: 'translateY', values: [0, -4], ...over }))
    for (const input of ['lookX', 'lookY', 'mouth', 'pace', 'turn', 'held']) expect(drive({ input }), input).not.toBeNull()
    for (const input of ['walk', 'speed', 'LookX', 'constructor', '', 7, null]) expect(drive({ input }), String(input)).toBeNull()
    expect(drive({ target: ['torso', 'hand', 'torso'] })?.drives?.at(-1)?.target).toEqual(['torso', 'hand'])
    for (const target of ['leaf', 'fade', 'nowhere', [], Array.from({ length: MORE_LIMITS.targets + 1 }, () => 'torso')]) expect(drive({ target }), JSON.stringify(target)).toBeNull()
    expect(drive({ target: Array.from({ length: MORE_LIMITS.targets }, () => 'torso') })).not.toBeNull()
    expect(drive({ prop: 'd', target: 'hand', values: ['M 140 120 L 160 130', 'M 140 120 L 160 110'] })).not.toBeNull()
    expect(drive({ prop: 'd', target: 'hand', values: ['M 140 120 L 160 130', 'M 140 120 Q 150 100 160 110'] })).toBeNull()
    expect(drive({ prop: 'd', target: 'torso', values: ['M 0 0 L 1 1', 'M 0 0 L 2 2'] })).toBeNull()
    expect(drive({ prop: 'rx', values: [20, 30, null] })).not.toBeNull()
    expect(drive({ prop: 'points' })).toBeNull()
    for (const values of [[0], Array.from({ length: LIMITS.keyframes + 1 }, (_, i) => i), [0, '4'], [0, Infinity], 'up']) expect(drive({ values }), JSON.stringify(values)).toBeNull()
    expect(drive({ prop: 'skewX', values: [0, 81] })).toBeNull()
    // No time: no phase, no delay, no times; `at` in their place.
    for (const key of ['phase', 'delay', 'times', 'duration', 'loop']) expect(drive({ [key]: 0.5 }), key).toBeNull()
    expect(drive({ values: [0, 2, 4], at: [0, 0.2, 1] })).not.toBeNull()
    for (const at of [[0, 1], [0, 0.5, 0.9], [0.1, 0.5, 1], [0, 0.6, 0.5], [0, 0.5, 0.5], 'even']) expect(drive({ values: [0, 2, 4], at }), JSON.stringify(at)).toBeNull()
    expect(drive({ values: [0, 2, 4], ease: ['outBounce', [0.1, 0.2, 0.3, 0.4]] })).not.toBeNull()
    expect(drive({ values: [0, 2, 4], ease: ['in'] })).toBeNull()
  })

  it('lets only the inputs that move slowly drive a colour or an opacity', () => {
    const paint = (input: string, prop: string) => {
      const values = prop === 'fill' || prop === 'stroke' ? ['#000000', '#ffffff'] : [1, 0.2]
      return rich((l) => void drivesOf(l).push({ input, target: 'torso', prop, values }))
    }
    for (const prop of ['fill', 'stroke', 'opacity', 'fillOpacity', 'strokeOpacity']) {
      for (const input of ['pace', 'turn', 'held']) expect(paint(input, prop), `${input} ${prop}`).not.toBeNull()
      for (const input of ['lookX', 'lookY', 'mouth']) expect(paint(input, prop), `${input} ${prop}`).toBeNull()
    }
    // Everything else moves with any input.
    for (const input of ['lookX', 'mouth']) expect(paint(input, 'strokeWidth'), input).not.toBeNull()
  })

  it('keeps a driven colour or opacity from flashing: no jump, no swing, values far apart along the input', () => {
    const paint = (over: Json) => rich((l) => void drivesOf(l).push({ input: 'held', target: 'torso', prop: 'opacity', values: [1, 0.2], ...over }))
    expect(paint({})).not.toBeNull()
    for (const ease of ['step', 'outElastic', 'outBounce', [0, 3, 1, -2], [0.3, -0.5, 0.7, 1]]) expect(paint({ ease }), JSON.stringify(ease)).toBeNull()
    for (const ease of ['in', 'outBack', 'inOutBack', [0, 0, 0, 1], [0.25, 0.1, 0.25, 1]]) expect(paint({ ease }), JSON.stringify(ease)).not.toBeNull()
    // Two values along pace and held, three along turn (which runs twice as far), at least 0.34 apart there.
    expect(paint({ values: [1, 0, 1] })).toBeNull()
    expect(paint({ input: 'pace', values: [1, 0, 1, 0] })).toBeNull()
    expect(paint({ input: 'turn', values: [1, 0, 1] })).not.toBeNull()
    expect(paint({ input: 'turn', values: [1, 0, 1], at: [0, 0.4, 1] })).not.toBeNull()
    expect(paint({ input: 'turn', values: [1, 0, 1], at: [0, 0.2, 1] })).toBeNull()
    expect(paint({ input: 'turn', values: [1, 0, 1, 0] })).toBeNull()
    // What is not paint may swing and stand close.
    expect(paint({ prop: 'translateY', values: [0, 4, 0, 4, 0], ease: 'outBounce' })).not.toBeNull()
  })

  it('at most 16, and none at all is no list', () => {
    const drives = (n: number) => rich((l) => void (l.drives = Array.from({ length: n }, () => ({ input: 'turn', target: 'body', prop: 'rotate', values: [-5, 5] }))))
    expect(drives(MORE_LIMITS.drives)?.drives).toHaveLength(MORE_LIMITS.drives)
    expect(drives(MORE_LIMITS.drives + 1)).toBeNull()
    expect(drives(0)).not.toHaveProperty('drives')
    expect(rich((l) => void (l.drives = null))).not.toHaveProperty('drives')
    expect(rich((l) => void (l.drives = { input: 'turn' }))).toBeNull()
    expect(drivesOn(rich() as Look, 'turn')).toBe(true)
    expect(drivesOn(rich() as Look, 'lookY')).toBe(false)
    expect(drivesOn(checked() as Look, 'pace')).toBe(false)
    expect(drivesOn(null as unknown as Look, 'pace')).toBe(false)
  })

  it('counts d keyframes of drives with those of the tracks', () => {
    const key = (chars: number) => `M 0 0${' L 1 1'.repeat((chars - 5) / 6)}`.padEnd(chars, ' ')
    const look = (driven: number) =>
      checked((l) => {
        animation(l, 'grin').tracks = Array.from({ length: 2 }, () => ({ target: 'mouth', prop: 'd', values: [key(MORE_LIMITS.dKeyframeChars), key(MORE_LIMITS.dKeyframeChars)] }))
        l.drives = Array.from({ length: driven }, () => ({ input: 'mouth', target: 'mouth', prop: 'd', values: [key(MORE_LIMITS.dKeyframeChars), key(MORE_LIMITS.dKeyframeChars)] }))
      })
    expect(look(1)).not.toBeNull()
    expect(look(2)).toBeNull()
  })
})

describe('checkLook on the new props and eases', () => {
  it('skews like it rotates, up to 80 degrees either way', () => {
    for (const prop of ['skewX', 'skewY']) {
      expect(checked((l) => void Object.assign(track(l, 'hop'), { prop, values: [0, 80, -80] })), prop).not.toBeNull()
      expect(checked((l) => void Object.assign(track(l, 'hop'), { prop, values: [0, 80.5] })), prop).toBeNull()
      expect(propFits(prop as 'skewX', 'use')).toBe(true)
      expect(propFits(prop as 'skewX', 'stop')).toBe(false)
    }
  })

  it('takes fill and stroke opacities from 0 to 1 under the paint rule, and a dash offset as any number', () => {
    for (const prop of ['fillOpacity', 'strokeOpacity']) {
      const fade = (duration: number, values: unknown[], ease?: unknown) =>
        checked((l) => {
          animation(l, 'hop').duration = duration
          Object.assign(track(l, 'hop'), { target: 'shadow', prop, values, ease })
          if (ease === undefined) delete track(l, 'hop').ease
        })
      expect(fade(0.34, [1, 0, 1]), prop).not.toBeNull()
      expect(fade(0.3, [1, 0, 1]), prop).toBeNull()
      expect(fade(1, [1, 1.5]), prop).toBeNull()
      expect(fade(1, [1, 0], 'step'), prop).toBeNull()
    }
    expect(checked((l) => void Object.assign(track(l, 'hop'), { target: 'shadow', prop: 'strokeDashoffset', values: [0, -400, 1e5] }))).not.toBeNull()
    expect(checked((l) => void Object.assign(track(l, 'hop'), { target: 'skin', prop: 'strokeDashoffset', values: [0, 1] }))).toBeNull()
  })

  it('takes the new eases by name, and a bezier as four numbers for all segments or one per segment', () => {
    for (const ease of ['inBack', 'inOutBack', 'outElastic', 'outBounce', [0.25, 0.1, 0.25, 1], [0, -2, 1, 3], [1, 0, 0, 1]]) {
      expect(checked((l) => void (track(l, 'hop').ease = ease)), JSON.stringify(ease)).not.toBeNull()
      expect(checked((l) => void (animation(l, 'hop').ease = ease)), JSON.stringify(ease)).not.toBeNull()
    }
    expect(checked((l) => void (track(l, 'hop').ease = ['in', [0.4, 0, 0.2, 1], 'outBounce', [0, 0, 1, 1], 'step']))?.animations.hop.tracks[0].ease).toEqual(['in', [0.4, 0, 0.2, 1], 'outBounce', [0, 0, 1, 1], 'step'])
    for (const ease of [[-0.1, 0, 0.2, 1], [0.4, 0, 1.1, 1], [0.4, -2.1, 0.2, 1], [0.4, 0, 0.2, 3.1], [0.4, 0, 0.2], [0.4, 0, 0.2, 1, 0], ['in', 0, 0.2, 1], [0.4, 0, 0.2, Number.NaN], [[0.4, 0, 0.2]], 'bezier', 'inback']) {
      expect(checked((l) => void (track(l, 'hop').ease = ease)), JSON.stringify(ease)).toBeNull()
    }
    // A list of beziers has one per segment, like a list of names.
    expect(checked((l) => void (track(l, 'hop').ease = [[0.4, 0, 0.2, 1], [0.4, 0, 0.2, 1]]))).toBeNull()
    // A paint never steps, and a bezier is no step.
    expect(checked((l) => void (track(l, 'blush').ease = [0, 0, 1, 0]))).not.toBeNull()
    expect(trackEases({ values: [1, 2, 3] }, { ease: [0.4, 0, 0.2, 1] })).toEqual([[0.4, 0, 0.2, 1], [0.4, 0, 0.2, 1]])
    expect(trackEases({ values: [1, 2, 3, 4, 5] }, { ease: [0.4, 0, 0.2, 1] })).toEqual(Array.from({ length: 4 }, () => [0.4, 0, 0.2, 1]))
    expect(trackEases({ values: [1, 2, 3], ease: ['inBack', [0, 0, 1, 1]] }, {})).toEqual(['inBack', [0, 0, 1, 1]])
    expect(isEase([0.4, 0, 0.2, 1])).toBe(true)
    expect(isEase([0.4, 0, 1.2, 1])).toBe(false)
    expect(isEase('outBounce')).toBe(true)
    expect(isEase('bounce')).toBe(false)
  })
})

describe('lookElements on the richer artwork', () => {
  it('draws masks, filters, uses and dashes under their SVG names, every reference prefixed and the filter’s own names not', () => {
    const look = rich() as Look
    const markup = render(look, 'p-')
    const found = expectOnlyRich(markup, 'p-')
    const drawnTags = ['svg', 'defs', 'linearGradient', 'stop', 'mask', 'rect', 'g', 'circle', 'filter', 'ellipse', 'use', 'path', 'feGaussianBlur', 'feOffset', 'feFlood', 'feComposite', 'feMerge', 'feMergeNode', 'feTurbulence', 'feColorMatrix', 'feDisplacementMap', 'feMorphology', 'feBlend', 'feDropShadow']
    expect([...found.tags].sort()).toEqual(drawnTags.sort())
    expect(markup).toContain('<use id="p-leafL" href="#p-leaf" x="50" y="120" transform="rotate(-20 60 125)"></use>')
    expect(markup).toContain('<use id="p-leafR" href="#p-leaf" x="130" y="120" filter="url(#p-grain)"></use>')
    expect(markup).toContain('<g id="p-body" mask="url(#p-vignette)" filter="url(#p-soft)">')
    expect(markup).toContain('<mask id="p-vignette" maskUnits="userSpaceOnUse" maskContentUnits="userSpaceOnUse" x="0" y="0" width="200" height="200" mask-type="luminance">')
    expect(markup).toContain('<filter id="p-soft" x="-20%" y="-20%" width="140%" height="140%" filterUnits="objectBoundingBox" primitiveUnits="userSpaceOnUse" color-interpolation-filters="sRGB">')
    expect(markup).toContain('<feGaussianBlur in="SourceAlpha" stdDeviation="2" result="blur"></feGaussianBlur>')
    expect(markup).toContain('<feFlood flood-color="#00000080" flood-opacity="0.6" result="ink"></feFlood>')
    expect(markup).toContain('<feComposite in="ink" in2="moved" operator="in" result="shadow"></feComposite>')
    expect(markup).toContain('<feMergeNode in="shadow"></feMergeNode>')
    expect(markup).toContain('<feTurbulence type="fractalNoise" baseFrequency="0.8 0.6" numOctaves="2" seed="3" result="noise"></feTurbulence>')
    expect(markup).toContain('<feDisplacementMap in="SourceGraphic" in2="grey" scale="4" xChannelSelector="R" yChannelSelector="G" result="shaken"></feDisplacementMap>')
    expect(markup).toContain('stroke-dasharray="6 3" stroke-dashoffset="0"')
    expect(markup).toContain('stroke-dasharray="none"')
    // The names a filter passes between its primitives are no ids.
    expect(found.ids).not.toContain('p-blur')
    expect(markup).not.toMatch(/(?:in|in2|result)="p-/)
    // Every id of the look is drawn once, nothing else has one.
    expect([...found.ids].sort()).toEqual([...lookIndex(look).keys()].map((id) => `p-${id}`).sort())
  })

  it('never draws a use that could repeat itself or name what is not there, and counts what a use shows', () => {
    const hostile = {
      viewBox: [0, 0, 200, 200],
      nodes: [
        { tag: 'g', id: 'loop', children: [{ tag: 'use', attrs: { useRef: 'loop' } }] },
        { tag: 'use', attrs: { useRef: 'loop' } },
        { tag: 'use', attrs: { useRef: 'gone' } },
        { tag: 'use', attrs: { useRef: 'url(http://example.invalid/x.svg#a)' } },
        { tag: 'use', attrs: { href: 'http://example.invalid/x.svg#a', xlinkHref: 'javascript:alert(1)' } },
        { tag: 'circle', id: 'ok' },
        { tag: 'use', attrs: { useRef: 'ok', onClick: 'alert(1)' } },
      ],
    } as unknown as Look
    const markup = render(hostile, 'h-')
    expectOnlyRich(markup, 'h-')
    // Only the use of the circle is drawn; the group's own use would repeat the group inside itself.
    expect(markup.match(/<use /g)).toHaveLength(1)
    expect(markup).toContain('<use href="#h-ok"></use>')
    for (const word of ['example.invalid', 'javascript', 'onclick', 'alert', 'gone']) expect(markup, word).not.toContain(word)
    // 299 circles leave room for one element: a use that shows a group of three is not drawn.
    const crowded = { viewBox: [0, 0, 200, 200], nodes: [{ tag: 'g', id: 'trio', children: [{ tag: 'circle' }, { tag: 'circle' }] }, ...Array.from({ length: 296 }, () => ({ tag: 'circle' })), { tag: 'use', attrs: { useRef: 'trio' } }, { tag: 'circle', id: 'last' }] } as unknown as Look
    const drawn = render(crowded, 'c-')
    expect(drawn).not.toContain('<use')
    expect(drawn).toContain('id="c-last"')
  })

  it('keeps a filter’s and a mask’s content to their own tables whatever the data', () => {
    const hostile = {
      viewBox: [0, 0, 200, 200],
      nodes: [
        {
          tag: 'defs',
          children: [
            { tag: 'filter', id: 'f', attrs: { style: 'x', href: '#x' }, children: [{ tag: 'feImage', attrs: { href: 'http://example.invalid/x.png' } }, { tag: 'feGaussianBlur', attrs: { stdDeviation: '1e9', in: 'url(#x)', result: '" onload="x' } }, { tag: 'circle' }, { tag: 'feMerge', children: [{ tag: 'feGaussianBlur' }, { tag: 'feMergeNode', attrs: { in: 'javascript:alert(1)' } }] }] },
            { tag: 'mask', id: 'm', attrs: { maskType: 'evil', mask: 'url(#m)' }, children: [{ tag: 'text', text: 'leak' }, { tag: 'use', attrs: { useRef: 'm' } }, { tag: 'g', children: [{ tag: 'text', text: 'leak' }, { tag: 'rect', attrs: { maskRef: 'm', filterRef: 'f' } }] }] },
          ],
        },
        { tag: 'feFlood', attrs: { floodColor: '#ff0000' } },
        { tag: 'g', attrs: { maskRef: 'url(#m)', filterRef: 'blur(3px)', strokeDasharray: '1;2' } },
      ],
    } as unknown as Look
    const markup = render(hostile, 'h-')
    expectOnlyRich(markup, 'h-')
    for (const word of ['example.invalid', 'javascript', 'onload', 'feImage', 'leak', 'evil', 'blur(', '1e9', 'style']) expect(markup, word).not.toContain(word)
    expect(markup).toContain('<feGaussianBlur></feGaussianBlur>')
    expect(markup).toContain('<feMerge><feMergeNode></feMergeNode></feMerge>')
    // What a rect inside a mask may not carry is still printed by the renderer: the check is what refuses it, the printed reference stays inside the figure.
    expect(markup).toMatch(/<mask id="h-m"><g><rect mask="url\(#h-m\)" filter="url\(#h-f\)"><\/rect><\/g><\/mask>/)
    expect(markup).not.toContain('<feFlood')
    expect(checkLook(hostile)).toBeNull()
  })
})

describe('a storm of hostile looks on the richer artwork', () => {
  function random(seed: number) {
    let a = seed >>> 0
    return () => {
      a = (a + 0x6d2b79f5) >>> 0
      let t = a
      t = Math.imul(t ^ (t >>> 15), t | 1)
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296
    }
  }
  const PAYLOADS: unknown[] = ['url(http://example.invalid/x)', 'url(#soft)', '#leaf', 'http://example.invalid/x.svg#leaf', 'javascript:alert(1)', '"><script>alert(1)</script>', 'blur(2px)', 'SourceGraphic', 'BackgroundImage', 'constructor', '__proto__', 'leaf', 'vignette', 'soft', 'none', '1 2 3 4 5 6 7 8 9', '0.5', 0, -1, 1e9, Number.NaN, true, null, [], [0.4, 0, 0.2, 1], {}, { tag: 'feImage' }, { tag: 'use', attrs: { useRef: 'body' } }]
  const KEYS = ['href', 'xlinkHref', 'useRef', 'maskRef', 'filterRef', 'in', 'in2', 'result', 'style', 'onload', 'mask', 'filter', 'stdDeviation', 'strokeDasharray', 'tag', 'attrs', 'id', 'children', 'parents', 'drives', 'input', 'at']
  const BAD_VALUES = ['example.invalid', 'javascript', 'onload', 'alert', 'script', 'blur(', '<', '>', '"', "'", 'http']

  it('never throws, and never renders a name, a reference or a value outside the tables', () => {
    const rnd = random(20261122)
    const pick = <T,>(items: readonly T[]): T => items[Math.floor(rnd() * items.length)]
    const containers = (root: unknown): object[] => {
      const out: object[] = []
      const walk = (v: unknown) => {
        if (typeof v !== 'object' || v === null) return
        out.push(v)
        for (const inner of Object.values(v)) walk(inner)
      }
      walk(root)
      return out
    }
    const violations = (markup: string, prefix: string): string[] => {
      const out: string[] = []
      const found = names(markup)
      for (const tag of found.tags) if (!RICH_TAGS.has(tag)) out.push(`tag ${tag}`)
      for (const attr of found.attrs) if (!RICH_ATTRS.has(attr)) out.push(`attribute ${attr}`)
      for (const id of found.ids) if (!id.startsWith(prefix)) out.push(`id ${id}`)
      for (const ref of found.refs) if (!ref.startsWith(prefix)) out.push(`reference ${ref}`)
      for (const href of markup.matchAll(/ href="([^"]*)"/g)) if (!href[1].startsWith(`#${prefix}`)) out.push(`href ${href[1]}`)
      for (const value of found.values) for (const word of BAD_VALUES) if (value.includes(word)) out.push(`${word} in the value ${value}`)
      for (const open of markup.matchAll(/<\/?([A-Za-z][\w:-]*)/g)) if (!RICH_TAGS.has(open[1])) out.push(`<${open[1]}`)
      return out
    }
    let accepted = 0
    let rejected = 0
    for (let round = 0; round < 1200; round++) {
      const look = wire(richLook()) as Json
      const hits = 1 + Math.floor(rnd() * 3)
      for (let h = 0; h < hits; h++) {
        const where = pick(containers(look)) as Record<string, unknown>
        const keys = Object.keys(where)
        const mode = rnd()
        if (mode < 0.55 && keys.length > 0) where[pick(keys)] = pick(PAYLOADS)
        else if (mode < 0.85 && !Array.isArray(where)) Object.defineProperty(where, pick(KEYS), { value: pick(PAYLOADS), enumerable: true, configurable: true, writable: true })
        else if (keys.length > 0) delete where[pick(keys)]
      }
      const before = JSON.stringify(look)
      const got = checkLook(look)
      const bad: string[] = JSON.stringify(look) === before ? [] : ['the input was changed']
      if (got) {
        accepted++
        bad.push(...violations(render(got, 'f-'), 'f-'))
      } else {
        rejected++
      }
      bad.push(...violations(render(look as unknown as Look, 'r-'), 'r-'))
      if (bad.length > 0) expect(bad, `round ${round}: ${before}`).toEqual([])
    }
    expect(rejected).toBeGreaterThan(800)
    expect(accepted).toBeGreaterThan(20)
  })
})

describe('matrices', () => {
  it('invert what can be inverted, and nothing else', () => {
    const m = transformMatrix(parseTransform('translate(10 20) rotate(30) scale(2 3)') ?? [])
    const back = invert(m)
    expect(back).not.toBeNull()
    const product = transformMatrix([{ fn: 'matrix', args: [...m] }, { fn: 'matrix', args: [...(back ?? [])] }])
    product.forEach((n, i) => expect(n).toBeCloseTo([1, 0, 0, 1, 0, 0][i], 12))
    expect(invert([0, 0, 0, 0, 5, 5])).toBeNull()
    expect(invert([1, 2, 2, 4, 0, 0])).toBeNull()
    expect(invert([Number.NaN, 0, 0, 1, 0, 0])).toBeNull()
  })
})
