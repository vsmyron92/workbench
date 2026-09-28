import { describe, expect, it } from 'vitest'
import { asUrl, triggerBefore } from './triggers'

describe('editor suggestion triggers', () => {
  it('offers people after @', () => {
    expect(triggerBefore('Hi @')).toEqual({ kind: 'mention', query: '', length: 1 })
    expect(triggerBefore('Hi @ann')).toEqual({ kind: 'mention', query: 'ann', length: 4 })
    expect(triggerBefore('@Ann')).toEqual({ kind: 'mention', query: 'Ann', length: 4 })
    expect(triggerBefore('(@bo')).toEqual({ kind: 'mention', query: 'bo', length: 3 })
  })

  it('does not fire inside words, e-mail addresses or after a space', () => {
    expect(triggerBefore('me@example')).toBeNull()
    expect(triggerBefore('Hi @ann ')).toBeNull()
    expect(triggerBefore('Hi @￼')).toBeNull()
  })

  it('offers pages after [[', () => {
    expect(triggerBefore('see [[')).toEqual({ kind: 'page', query: '', length: 2 })
    expect(triggerBefore('see [[Run book')).toEqual({ kind: 'page', query: 'Run book', length: 10 })
    expect(triggerBefore('see [[done]] x')).toBeNull()
  })

  it('recognizes web addresses in the link picker', () => {
    expect(asUrl(' https://example.com/a?b=1 ')).toBe('https://example.com/a?b=1')
    expect(asUrl('example.com/docs')).toBe('https://example.com/docs')
    expect(asUrl('mailto:me@example.com')).toBe('mailto:me@example.com')
    expect(asUrl('Runbook')).toBeNull()
    expect(asUrl('two words.com')).toBeNull()
  })
})
