import { describe, expect, it } from 'vitest'
import { anchorOf, occurrences } from './selection'

describe('inline comment anchors', () => {
  const text = 'Army cap is 30. The army cap grows. Army cap!'

  it('counts non-overlapping occurrences from the start', () => {
    expect(occurrences(text, 'Army cap')).toEqual([0, 36])
    expect(occurrences('aaaa', 'aa')).toEqual([0, 2])
    expect(occurrences('abc', '')).toEqual([])
  })

  it('finds which occurrence was selected', () => {
    expect(anchorOf(text, 'Army cap', 36)).toEqual({ selection: 'Army cap', matchIndex: 1, matchCount: 2 })
    expect(anchorOf(text, 'grows', text.indexOf('grows'))).toEqual({ selection: 'grows', matchIndex: 0, matchCount: 1 })
  })

  it('drops surrounding whitespace and moves the offset with it', () => {
    expect(anchorOf(text, ' army cap ', 19)).toEqual({ selection: 'army cap', matchIndex: 0, matchCount: 1 })
  })

  it('refuses what Confluence cannot anchor', () => {
    expect(anchorOf(text, '   ', 0)).toHaveProperty('error')
    expect(anchorOf('one\ntwo', 'one\ntwo', 0)).toHaveProperty('error')
    expect(anchorOf(text, 'x'.repeat(1001), 0)).toHaveProperty('error')
    expect(anchorOf(text, 'Army', 5)).toHaveProperty('error')
    // "aa" starting at 1 in "aaa" overlaps the first occurrence.
    expect(anchorOf('aaa', 'aa', 1)).toHaveProperty('error')
  })
})
