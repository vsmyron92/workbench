import { beforeEach, describe, expect, it } from 'vitest'
import { sortBookmarks, useBookmarks } from './bookmarks'

const s = () => useBookmarks.getState()

describe('bookmarks', () => {
  beforeEach(() => useBookmarks.setState({ list: [] }))

  it('toggles a line on and off', () => {
    expect(s().toggle('p', 'a.rs', 3)).not.toBeNull()
    expect(s().forFile('p', 'a.rs').map((b) => b.line)).toEqual([3])
    expect(s().toggle('p', 'a.rs', 3)).toBeNull()
    expect(s().list).toEqual([])
  })

  it('gives a mnemonic to one bookmark only and moves it on reuse', () => {
    s().toggle('p', 'a.rs', 3, '1')
    s().toggle('p', 'b.rs', 8, '1')
    const byLine = s().list.map((b) => `${b.path}:${b.line}:${b.mnemonic ?? '-'}`)
    expect(byLine.sort()).toEqual(['a.rs:3:-', 'b.rs:8:1'])
    // Adding a mnemonic to an existing plain bookmark keeps it (and its id).
    const id = s().forFile('p', 'a.rs')[0].id
    s().toggle('p', 'a.rs', 3, 'A')
    expect(s().forFile('p', 'a.rs')).toMatchObject([{ id, mnemonic: 'A' }])
  })

  it('follows line moves and sorts mnemonics first', () => {
    const a = s().toggle('p', 'a.rs', 3)!
    s().toggle('p', 'a.rs', 9, 'B')
    s().toggle('p', 'b.rs', 1, '2')
    s().moveLines([{ id: a.id, line: 5 }])
    expect(s().forFile('p', 'a.rs').map((b) => b.line).sort()).toEqual([5, 9])
    expect(sortBookmarks(s().list).map((b) => b.mnemonic ?? '-')).toEqual(['2', 'B', '-'])
  })
})
