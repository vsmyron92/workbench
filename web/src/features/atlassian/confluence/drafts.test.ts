import { beforeEach, describe, expect, it } from 'vitest'
import { draftKey, inlineDraftKey, pendingInline, pruneDrafts, useCommentDrafts, withoutDraft } from './drafts'

describe('comment drafts', () => {
  beforeEach(() => useCommentDrafts.setState({ drafts: {} }))

  it('keys drafts by project, page, kind and target', () => {
    expect(draftKey('p1', '42', 'reply', '9008')).toBe('p1|42|reply|9008')
    expect(draftKey(null, '42', 'footer')).toBe('|42|footer|')
    expect(draftKey('p1', '42', 'edit', '9008')).not.toBe(draftKey('p1', '42', 'reply', '9008'))
  })

  it('keeps the text and the version an edit started from until cleared', () => {
    const key = draftKey('p1', '42', 'edit', '9008')
    const { put, clear } = useCommentDrafts.getState()
    put(key, { text: 'old body', version: 2 })
    put(key, { text: 'my rewrite' })
    expect(useCommentDrafts.getState().drafts[key]).toMatchObject({ text: 'my rewrite', version: 2 })
    clear(key)
    expect(useCommentDrafts.getState().drafts[key]).toBeUndefined()
  })

  it('bounds how many drafts are kept and for how long', () => {
    const now = 1_000_000_000_000
    const many = Object.fromEntries(Array.from({ length: 60 }, (_, i) => [`k${i}`, { text: 'x', at: now - i * 1000 }]))
    const kept = pruneDrafts(many, now)
    expect(Object.keys(kept)).toHaveLength(50)
    expect(kept.k0).toBeDefined()
    expect(kept.k59).toBeUndefined()
    expect(pruneDrafts({ old: { text: 'x', at: now - 8 * 24 * 3600_000 } }, now)).toEqual({})
  })

  it('lists the unsent inline comments of a page with their anchors', () => {
    const a = { selection: 'mock data', matchIndex: 0, matchCount: 1 }
    const b = { selection: 'Rust', matchIndex: 1, matchCount: 2 }
    const { put } = useCommentDrafts.getState()
    put(inlineDraftKey('p1', '42', a), { text: 'first', anchor: a })
    put(inlineDraftKey('p1', '42', b), { text: 'second', anchor: b })
    put(inlineDraftKey('p1', '43', a), { text: 'other page', anchor: a })
    put(inlineDraftKey('p1', '42', { ...a, selection: 'empty' }), { text: '  ', anchor: a })
    const drafts = useCommentDrafts.getState().drafts
    expect(pendingInline(drafts, 'p1', '42').map((d) => d.anchor.selection).sort()).toEqual(['Rust', 'mock data'])
    expect(pendingInline(drafts, 'p1', '42', inlineDraftKey('p1', '42', b)).map((d) => d.anchor)).toEqual([a])
  })

  it('removes a draft without touching the others', () => {
    const d = { a: { text: '1', at: 1 }, b: { text: '2', at: 2 } }
    expect(withoutDraft(d, 'a')).toEqual({ b: { text: '2', at: 2 } })
    expect(withoutDraft(d, 'zz')).toBe(d)
  })
})
