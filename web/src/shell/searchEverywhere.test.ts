import { describe, expect, it } from 'vitest'
import { doubleShiftDetector } from './paletteSearch'
import { matchPositions } from '@/features/lsp/searchProvider'

const shift = { key: 'Shift', repeat: false }

describe('doubleShiftDetector', () => {
  const run = (steps: [('down' | 'up'), { key: string; repeat?: boolean }, number][]) => {
    let fired = 0
    const d = doubleShiftDetector(() => fired++)
    for (const [kind, e, t] of steps) {
      if (kind === 'down') d.keydown({ repeat: false, ...e }, t)
      else d.keyup({ repeat: false, ...e }, t)
    }
    return fired
  }
  it('fires on two quick presses of Shift alone', () => {
    expect(run([['down', shift, 0], ['up', shift, 50], ['down', shift, 150], ['up', shift, 200]])).toBe(1)
  })
  it('ignores slow presses', () => {
    expect(run([['down', shift, 0], ['up', shift, 50], ['down', shift, 600], ['up', shift, 650]])).toBe(0)
  })
  it('ignores Shift used as a modifier (typing capitals)', () => {
    expect(run([['down', shift, 0], ['down', { key: 'A' }, 10], ['up', shift, 50], ['down', shift, 100], ['up', shift, 150]])).toBe(0)
    expect(run([['down', shift, 0], ['up', shift, 50], ['down', { key: 'b' }, 60], ['down', shift, 100], ['up', shift, 150]])).toBe(0)
  })
  it('does not fire again on a third press', () => {
    expect(run([['down', shift, 0], ['up', shift, 40], ['down', shift, 80], ['up', shift, 120], ['down', shift, 160], ['up', shift, 200]])).toBe(1)
  })
  it('ignores a long hold followed by a tap', () => {
    expect(run([['down', shift, 0], ['down', { key: 'Shift', repeat: true }, 30], ['up', shift, 900], ['down', shift, 1000], ['up', shift, 1050]])).toBe(0)
  })
})

describe('matchPositions', () => {
  it('finds the query letters in order', () => {
    expect(matchPositions('totalArea', 'tA')).toEqual([0, 3])
    expect(matchPositions('describe', 'desc')).toEqual([0, 1, 2, 3])
    expect(matchPositions('Circle', 'xyz')).toEqual([])
  })
})
