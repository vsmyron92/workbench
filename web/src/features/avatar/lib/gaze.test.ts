// The gaze is a vector on screen: pupils drawn inside a mirrored or turned transform look
// where they are asked to (the pointer, an open question, an avatar met), as the session's
// pictures draw them (server/src/avatar/preview.rs, the pose).

import { describe, expect, it } from 'vitest'
import { undoLinear, undone } from './builtin'
import { compile, sample } from './engine'
import { readTransform } from './fixtures'
import type { Look, LookNode } from './types'

/** Pupils of 3 at (100, 80), drawn plainly or inside a group with this transform. */
function eyes(transform?: string, gaze: number | null = null): Look {
  const pupils: LookNode = { tag: 'circle', id: 'pupils', attrs: { cx: 100, cy: 80, r: 3 }, box: [97, 77, 6, 6] }
  const nodes: LookNode[] = transform ? [{ tag: 'g', attrs: { transform }, children: [pupils] }] : [pupils]
  return { viewBox: [0, 0, 200, 200], nodes, pivots: {}, roles: { pupils: 'pupils' }, animations: {}, states: {}, mouth: [], gaze }
}

/** How the pupils are moved, in their own coordinates, looking along `gaze`. */
function moved(look: Look, gaze: readonly [number, number]) {
  const c = compile(look)
  if (!c) throw new Error('the look does not compile')
  return readTransform(sample(c, [], 0, { gaze }).get('pupils')?.transform)
}

describe('the gaze', () => {
  it('moves plain pupils as far as it is asked, 3% of the width by default', () => {
    expect(moved(eyes(), [1, 0])).toMatchObject({ tx: 6, ty: 0 })
    expect(moved(eyes(), [0, -1])).toMatchObject({ tx: 0, ty: -6 })
    // A group that only moves them changes nothing.
    expect(moved(eyes('translate(10 5)'), [1, 0])).toMatchObject({ tx: 6, ty: 0 })
  })

  it('looks right on screen in a figure mirrored to face the other way', () => {
    // Inside the mirror their own x runs leftwards: they move -6 of it, which is 6 to the right on screen.
    expect(moved(eyes('translate(200 0) scale(-1 1)'), [1, 0])).toMatchObject({ tx: -6, ty: 0 })
    expect(moved(eyes('translate(200 0) scale(-1 1)'), [0, 1])).toMatchObject({ tx: 0, ty: 6 })
    expect(moved(eyes('matrix(-1 0 0 1 200 0)', 4), [-1, 0])).toMatchObject({ tx: 4, ty: 0 })
  })

  it('looks right on screen in a turned or scaled figure, and as far', () => {
    // Turned a quarter, their own -y points right on screen.
    expect(moved(eyes('rotate(90 100 80)'), [1, 0])).toMatchObject({ tx: 0, ty: -6 })
    // Drawn at twice the size, they move half as far in their own units: 6 on screen.
    expect(moved(eyes('translate(-100 -80) scale(2)'), [1, 0])).toMatchObject({ tx: 3, ty: 0 })
    // Squashed flat there is nothing to undo: the vector is used as it is.
    expect(moved(eyes('scale(1 0)'), [1, 0])).toMatchObject({ tx: 6, ty: 0 })
  })

  it('undoes the linear part of a matrix only where there is one to undo', () => {
    expect(undoLinear(undefined)).toBeNull()
    expect(undoLinear([1, 0, 0, 1, 30, -4])).toBeNull()
    expect(undoLinear([1, 0, 0, 1e-12, 0, 0])).toBeNull()
    expect(undoLinear([-1, 0, 0, 1, 200, 0])).toEqual([-1, 0, 0, 1])
    expect(undone(undoLinear([2, 0, 0, 0.5, 0, 0]), 6, 3)).toEqual([3, 6])
    expect(undone(null, 6, -2)).toEqual([6, -2])
  })
})
