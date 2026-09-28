import { describe, expect, it } from 'vitest'
import { nextScratchName } from './scratches'

describe('scratch names', () => {
  it('takes the first free name, CLion style', () => {
    expect(nextScratchName([], 'md')).toBe('scratch.md')
    expect(nextScratchName(['scratch.md', 'scratch.http'], 'md')).toBe('scratch_2.md')
    expect(nextScratchName(['scratch.md', 'scratch_2.md', 'Scratch_3.MD'], 'md')).toBe('scratch_4.md')
    expect(nextScratchName(['scratch.md'], 'http')).toBe('scratch.http')
  })
})
