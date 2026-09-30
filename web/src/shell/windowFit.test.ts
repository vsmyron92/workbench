import { describe, expect, it } from 'vitest'
import { collapseTarget } from './windowFit'

describe('collapse target', () => {
  it('keeps the window frame and makes the page as wide as the agents window', () => {
    // 1600 px window with 16 px of frame: the page shrinks from 1584 to 560.
    expect(collapseTarget(1600, 1584, 560)).toEqual({ width: 576, freed: 1024 })
  })
  it('does nothing when there is nothing to free', () => {
    expect(collapseTarget(600, 584, 560)).toBeNull()
    expect(collapseTarget(500, 500, 560)).toBeNull()
  })
})
