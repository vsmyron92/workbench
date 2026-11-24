// The id of this page load, as the arbiter gives it to the server with every claim.

import { describe, expect, it } from 'vitest'
import { makeTab } from './rig'

describe('makeTab', () => {
  it('is twelve random bytes as base64url: sixteen characters the server accepts', () => {
    const tab = makeTab((bytes) => bytes.fill(0))
    expect(tab).toBe('AAAAAAAAAAAAAAAA')
    expect(makeTab((bytes) => bytes.fill(255))).toBe('________________')
    // "Man" three times over, and then a tail that needs the last two letters of the alphabet.
    expect(makeTab((bytes) => bytes.set([77, 97, 110, 77, 97, 110, 77, 97, 110, 251, 239, 254]))).toBe('TWFuTWFuTWFu--_-')
  })

  it('asks for exactly twelve bytes, and differs when they do', () => {
    let asked = 0
    const one = makeTab((bytes) => {
      asked = bytes.length
      bytes.set([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12])
    })
    const other = makeTab((bytes) => bytes.set([12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1]))
    expect(asked).toBe(12)
    expect(one).not.toBe(other)
    for (const tab of [one, other]) expect(tab).toMatch(/^[A-Za-z0-9_-]{16}$/)
  })
})
