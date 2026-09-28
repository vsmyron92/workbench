import { describe, expect, it } from 'vitest'
import { toggledCase } from './editorKeymap'

describe('toggledCase', () => {
  it('upper-cases mixed or lower text and lower-cases upper text, like CLion', () => {
    expect(toggledCase('totalArea')).toBe('TOTALAREA')
    expect(toggledCase('TOTALAREA')).toBe('totalarea')
    expect(toggledCase('MAX_len')).toBe('MAX_LEN')
    expect(toggledCase('123')).toBe('123')
  })
})
