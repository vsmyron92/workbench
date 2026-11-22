// The cache of the avatar list: what each event of the bus does to it, and how a fetched
// answer is weighed against what events told while it was on its way.

import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it } from 'vitest'
import { MAX_LINES, mergeFetched, reduce, sameValue } from './cache'
import type { Kept } from './cache'
import type { Ask, AvatarInfo, AvatarList, Line, Look } from './types'

const line = (id: string, at: number, over: Partial<Line> = {}): Line => ({ id, at, from: 'agent', text: `line ${id}`, ...over })
const ask = (id: string, lineId: string, over: Partial<Ask> = {}): Ask => ({ id, lineId, text: 'Shall I push?', choices: ['Yes', 'No'], freeText: true, askedAt: 100, waiting: true, ...over })
const record = (terminalId: string, over: Partial<AvatarInfo> = {}): AvatarInfo => ({ terminalId, projectId: 'p1', on: true, name: '', designed: false, revision: 1, look: null, voice: {}, via: 'tools', held: null, ask: null, lines: [], createdAt: 1, updatedAt: 10, ...over })
const list = (avatars: AvatarInfo[], over: Partial<AvatarList> = {}): AvatarList => ({ enabled: true, sessions: 'asked', avatars, asked: [], ...over })
const of = (l: AvatarList | undefined, terminalId: string): Kept => l!.avatars.find((a) => a.terminalId === terminalId) as Kept
const ids = (rec: AvatarInfo): string[] => rec.lines.map((l) => l.id)

describe('reduce: avatar.say', () => {
  it('adds the line as the newest, raises the time and notes that an event told it', () => {
    const held = list([record('t1', { lines: [line('L1', 5, { outcome: 'spoken' })] }), record('t2')])
    const r = reduce(held, 'avatar.say', { terminalId: 't1', line: line('L2', 20) }, 21)
    expect(r).toMatchObject({ refetch: false, act: null, removed: null })
    expect(ids(of(r.list, 't1'))).toEqual(['L1', 'L2'])
    expect(of(r.list, 't1').updatedAt).toBe(21)
    expect(of(r.list, 't1').told).toEqual({ ask: 0, held: 0 })
    // The other record, and the list's own fields, are the same objects.
    expect(of(r.list, 't2')).toBe(held.avatars[1])
    expect(r.list).not.toBe(held)
    expect(held.avatars[0].lines).toHaveLength(1)
  })

  it('opens the question the line asks', () => {
    const q = ask('Q1', 'L2')
    const r = reduce(list([record('t1')]), 'avatar.say', { terminalId: 't1', line: line('L2', 20, { askId: 'Q1' }), ask: q }, 22)
    expect(of(r.list, 't1').ask).toEqual(q)
    expect(of(r.list, 't1').told).toEqual({ ask: 22, held: 0 })
  })

  it('does not say a line twice, and never trades a known outcome for none', () => {
    const held = list([record('t1', { lines: [line('L1', 5, { outcome: 'shown' })] })])
    const once = reduce(held, 'avatar.say', { terminalId: 't1', line: line('L1', 5) }, 30)
    expect(of(once.list, 't1').lines).toEqual([line('L1', 5, { outcome: 'shown' })])
  })

  it('keeps at most as many lines as the server does', () => {
    const lines = Array.from({ length: MAX_LINES }, (_, i) => line(`L${i}`, i))
    const r = reduce(list([record('t1', { lines })]), 'avatar.say', { terminalId: 't1', line: line('Lnew', 99) }, 99)
    expect(of(r.list, 't1').lines).toHaveLength(MAX_LINES)
    expect(ids(of(r.list, 't1'))[0]).toBe('L1')
    expect(ids(of(r.list, 't1')).at(-1)).toBe('Lnew')
  })

  it('keeps the open question’s line when the transcript is full, as the server does, and drops the oldest other one', () => {
    const lines = Array.from({ length: MAX_LINES }, (_, i) => line(`L${i}`, i))
    const held = list([record('t1', { lines, ask: ask('Q1', 'L0') })])
    const r = reduce(held, 'avatar.say', { terminalId: 't1', line: line('Lnew', 99) }, 99)
    expect(of(r.list, 't1').lines).toHaveLength(MAX_LINES)
    expect(ids(of(r.list, 't1'))[0]).toBe('L0')
    expect(ids(of(r.list, 't1'))).not.toContain('L1')
    expect(ids(of(r.list, 't1')).at(-1)).toBe('Lnew')
    // Its outcome from another device lands in its place, not as the newest line.
    const heard = reduce(r.list, 'avatar.line', { terminalId: 't1', line: line('L0', 0, { outcome: 'spoken' }) }, 100)
    expect(ids(of(heard.list, 't1'))[0]).toBe('L0')
    expect(of(heard.list, 't1').lines[0].outcome).toBe('spoken')
    expect(ids(of(heard.list, 't1'))).toHaveLength(MAX_LINES)
    // A reply of the user that arrives while it is open keeps it too.
    const replied = reduce(r.list, 'avatar.line', { terminalId: 't1', line: line('Lu', 101, { from: 'user', text: 'Later' }) }, 101)
    expect([ids(of(replied.list, 't1'))[0], ids(of(replied.list, 't1')).includes('L2'), ids(of(replied.list, 't1')).at(-1)]).toEqual(['L0', false, 'Lu'])
  })

  it('asks for the list when it cannot apply the line: no such avatar, or no list yet', () => {
    const held = list([record('t1')])
    expect(reduce(held, 'avatar.say', { terminalId: 't9', line: line('L1', 5) }, 6)).toEqual({ list: held, refetch: true, act: null, removed: null })
    expect(reduce(undefined, 'avatar.say', { terminalId: 't1', line: line('L1', 5) }, 6)).toEqual({ list: undefined, refetch: true, act: null, removed: null })
  })

  it('takes the line and asks for the list when the question cannot be read', () => {
    const r = reduce(list([record('t1')]), 'avatar.say', { terminalId: 't1', line: line('L2', 20), ask: { id: 'Q1' } }, 22)
    expect(ids(of(r.list, 't1'))).toEqual(['L2'])
    expect(of(r.list, 't1').ask).toBeNull()
    expect(r.refetch).toBe(true)
  })

  it('ignores what is no line', () => {
    const held = list([record('t1')])
    for (const bad of [{ terminalId: 't1' }, { terminalId: 't1', line: null }, { terminalId: 't1', line: { id: 'L1', at: 1, from: 'robot', text: 'x' } }, { terminalId: 't1', line: { id: 'L1', at: 'now', from: 'agent', text: 'x' } }, { terminalId: 7, line: line('L1', 1) }, null, 'say', { terminalId: 't1', line: line('L1', 1, { text: 'x'.repeat(9000) }) }]) {
      expect(reduce(held, 'avatar.say', bad, 5)).toEqual({ list: held, refetch: false, act: null, removed: null })
    }
  })
})

describe('reduce: avatar.line', () => {
  it('gives a line its outcome and the voice that said it', () => {
    const held = list([record('t1', { lines: [line('L1', 5), line('L2', 6)] })])
    const r = reduce(held, 'avatar.line', { terminalId: 't1', line: line('L1', 5, { outcome: 'spoken', voice: 'Daniel' }) }, 40)
    expect(of(r.list, 't1').lines).toEqual([line('L1', 5, { outcome: 'spoken', voice: 'Daniel' }), line('L2', 6)])
    expect(of(r.list, 't1').updatedAt).toBe(40)
  })

  it('keeps the better outcome when an older report arrives late', () => {
    const held = list([record('t1', { lines: [line('L1', 5, { outcome: 'spoken' })] })])
    const r = reduce(held, 'avatar.line', { terminalId: 't1', line: line('L1', 5, { outcome: 'shown' }) }, 40)
    expect(of(r.list, 't1').lines[0].outcome).toBe('spoken')
    for (const [worse, better] of [['hidden', 'shown'], ['shown', 'failed'], ['failed', 'skipped'], ['skipped', 'spoken'], ['unheard', 'hidden']] as const) {
      const up = reduce(list([record('t1', { lines: [line('L1', 5, { outcome: worse })] })]), 'avatar.line', { terminalId: 't1', line: line('L1', 5, { outcome: better }) }, 40)
      expect(of(up.list, 't1').lines[0].outcome).toBe(better)
      const down = reduce(list([record('t1', { lines: [line('L1', 5, { outcome: better })] })]), 'avatar.line', { terminalId: 't1', line: line('L1', 5, { outcome: worse }) }, 40)
      expect(of(down.list, 't1').lines[0].outcome).toBe(better)
    }
  })

  it('adds a reply of the user, which is never said', () => {
    const r = reduce(list([record('t1', { lines: [line('L1', 5)] })]), 'avatar.line', { terminalId: 't1', line: line('L2', 9, { from: 'user', text: 'Yes', via: 'click', askId: 'Q1' }) }, 9)
    expect(of(r.list, 't1').lines.map((l) => [l.id, l.from])).toEqual([['L1', 'agent'], ['L2', 'user']])
  })

  it('changes nothing when it only repeats what is known, but for the note that an event told it', () => {
    const held = list([record('t1', { lines: [line('L1', 5, { outcome: 'shown' })] })])
    const first = reduce(held, 'avatar.line', { terminalId: 't1', line: line('L1', 5, { outcome: 'shown' }) }, 40)
    expect(of(first.list, 't1').lines).toBe(held.avatars[0].lines)
    expect(of(first.list, 't1').told).toEqual({ ask: 0, held: 0 })
    const second = reduce(first.list, 'avatar.line', { terminalId: 't1', line: line('L1', 5, { outcome: 'shown' }) }, 41)
    expect(second.list).toBe(first.list)
  })
})

describe('reduce: avatar.ask', () => {
  it('follows the question through every change', () => {
    let held: AvatarList | undefined = list([record('t1')])
    const q = ask('Q1', 'L1')
    held = reduce(held, 'avatar.ask', { terminalId: 't1', change: 'opened', ask: q }, 50).list
    expect(of(held, 't1').ask).toEqual(q)
    held = reduce(held, 'avatar.ask', { terminalId: 't1', change: 'waiter_left', ask: { ...q, waiting: false } }, 60).list
    expect(of(held, 't1').ask?.waiting).toBe(false)
    held = reduce(held, 'avatar.ask', { terminalId: 't1', change: 'waiting', ask: q }, 61).list
    expect(of(held, 't1').ask?.waiting).toBe(true)
    for (const change of ['answered', 'dismissed', 'superseded', 'withdrawn', 'session_ended', 'moved_on']) {
      const closed = reduce(held, 'avatar.ask', { terminalId: 't1', change, ask: null }, 70)
      expect(of(closed.list, 't1').ask, change).toBeNull()
      expect(of(closed.list, 't1').told, change).toEqual({ ask: 70, held: 0 })
      expect(closed.refetch).toBe(false)
    }
  })

  it('asks for the list when the question cannot be read, or the avatar is not held', () => {
    const held = list([record('t1')])
    expect(reduce(held, 'avatar.ask', { terminalId: 't1', change: 'opened', ask: { id: 'Q1', text: 5 } }, 50)).toEqual({ list: held, refetch: true, act: null, removed: null })
    expect(reduce(held, 'avatar.ask', { terminalId: 't1', change: 'opened' }, 50).refetch).toBe(true)
    expect(reduce(held, 'avatar.ask', { terminalId: 't7', change: 'opened', ask: ask('Q1', 'L1') }, 50)).toEqual({ list: held, refetch: true, act: null, removed: null })
    expect(reduce(held, 'avatar.ask', { terminalId: 't1', change: 'opened', ask: ask('Q1', 'L1', { choices: ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i'] }) }, 50).refetch).toBe(true)
  })
})

describe('reduce: avatar.act', () => {
  it('signals a gesture that plays once, and ends what was held', () => {
    const held = list([record('t1', { held: { name: 'lean', mode: 'hold', since: 5 } })])
    const r = reduce(held, 'avatar.act', { terminalId: 't1', play: 'wave', mode: 'once', times: 3, seq: 7 }, 80)
    expect(r.act).toEqual({ terminalId: 't1', seq: 7, play: 'wave', times: 3 })
    expect(of(r.list, 't1').held).toBeNull()
    expect(of(r.list, 't1').told).toEqual({ ask: 0, held: 80 })
    // Without a mode it plays once; times stay within 1 to 5.
    expect(reduce(held, 'avatar.act', { terminalId: 't1', play: 'wave', seq: 8 }, 81).act).toEqual({ terminalId: 't1', seq: 8, play: 'wave', times: 1 })
    expect(reduce(held, 'avatar.act', { terminalId: 't1', play: 'wave', times: 99, seq: 9 }, 81).act?.times).toBe(5)
    expect(reduce(held, 'avatar.act', { terminalId: 't1', play: 'wave', times: -2, seq: 9 }, 81).act?.times).toBe(1)
  })

  it('puts a held or looping gesture into the record, where the figure plays it from', () => {
    for (const mode of ['hold', 'loop'] as const) {
      const r = reduce(list([record('t1')]), 'avatar.act', { terminalId: 't1', play: 'lean', mode, seq: 1 }, 90)
      expect(r.act).toBeNull()
      expect(of(r.list, 't1').held).toEqual({ name: 'lean', mode, since: 90 })
      expect(of(r.list, 't1').told).toEqual({ ask: 0, held: 90 })
    }
  })

  it('stops one gesture by name, or all', () => {
    const held = list([record('t1', { held: { name: 'lean', mode: 'hold', since: 5 } })])
    const named = reduce(held, 'avatar.act', { terminalId: 't1', stop: 'lean', seq: 2 }, 95)
    expect(named.act).toEqual({ terminalId: 't1', seq: 2, stop: 'lean' })
    expect(of(named.list, 't1').held).toBeNull()
    expect(of(reduce(held, 'avatar.act', { terminalId: 't1', stop: 'all', seq: 3 }, 95).list, 't1').held).toBeNull()
    const other = reduce(held, 'avatar.act', { terminalId: 't1', stop: 'wave', seq: 4 }, 95)
    expect(other.act).toEqual({ terminalId: 't1', seq: 4, stop: 'wave' })
    expect(of(other.list, 't1').held).toEqual({ name: 'lean', mode: 'hold', since: 5 })
  })

  it('still signals for an avatar the list does not hold, and takes no name that is none', () => {
    const held = list([record('t1')])
    expect(reduce(held, 'avatar.act', { terminalId: 't5', play: 'wave', seq: 1 }, 5)).toEqual({ list: held, refetch: false, act: { terminalId: 't5', seq: 1, play: 'wave', times: 1 }, removed: null })
    expect(reduce(held, 'avatar.act', { terminalId: 't5', play: 'lean', mode: 'hold', seq: 1 }, 5).refetch).toBe(true)
    for (const bad of ['', 'constructor()', '<img>', 'a b', '9lives', 'x'.repeat(40), 7, null]) {
      expect(reduce(held, 'avatar.act', { terminalId: 't1', play: bad, seq: 1 }, 5)).toEqual({ list: held, refetch: false, act: null, removed: null })
      expect(reduce(held, 'avatar.act', { terminalId: 't1', stop: bad, seq: 1 }, 5)).toEqual({ list: held, refetch: false, act: null, removed: null })
    }
  })
})

describe('reduce: what invalidates and what removes', () => {
  it('fetches the list again on avatar.changed, and turns an avatar off at once', () => {
    const held = list([record('t1'), record('t2')])
    const design = reduce(held, 'avatar.changed', { terminalId: 't1', revision: 2, on: true }, 5)
    expect(design).toEqual({ list: held, refetch: true, act: null, removed: null })
    const off = reduce(held, 'avatar.changed', { terminalId: 't1', revision: 1, on: false }, 5)
    expect(off.refetch).toBe(true)
    expect(of(off.list, 't1').on).toBe(false)
    expect(of(off.list, 't2')).toBe(held.avatars[1])
    // A session that only asked for an avatar has no record: the list tells who asked.
    expect(reduce(held, 'avatar.changed', { terminalId: 't8', revision: 0, on: false }, 5)).toEqual({ list: held, refetch: true, act: null, removed: null })
    expect(reduce(undefined, 'avatar.changed', { terminalId: 't1', revision: 2, on: true }, 5)).toEqual({ list: undefined, refetch: true, act: null, removed: null })
  })

  it('drops a removed avatar at once', () => {
    const held = list([record('t1'), record('t2')])
    const r = reduce(held, 'avatar.changed', { terminalId: 't1', revision: 3, on: true, removed: true }, 5)
    expect(r.list?.avatars.map((a) => a.terminalId)).toEqual(['t2'])
    expect(r.refetch).toBe(true)
  })

  it('fetches again when the settings changed and after a reconnect', () => {
    const held = list([record('t1')])
    expect(reduce(held, 'settings.changed', {}, 5)).toEqual({ list: held, refetch: true, act: null, removed: null })
    expect(reduce(held, 'resync', null, 5)).toEqual({ list: held, refetch: true, act: null, removed: null })
  })

  it('fetches again once this tab hears from the bus: what was said before it listened is in the answer', () => {
    const held = list([record('t1')])
    expect(reduce(held, 'hello', { version: 'x', startedAt: 1 }, 5)).toEqual({ list: held, refetch: true, act: null, removed: null })
    expect(reduce(undefined, 'hello', { version: 'x', startedAt: 1 }, 5)).toEqual({ list: undefined, refetch: true, act: null, removed: null })
  })

  it('drops the record of a removed terminal and names it, so its preferences go too', () => {
    const held = list([record('t1'), record('t2')])
    const r = reduce(held, 'terminal.removed', { id: 't2' }, 5)
    expect(r.list?.avatars.map((a) => a.terminalId)).toEqual(['t1'])
    expect(r).toMatchObject({ refetch: false, removed: 't2' })
    // A terminal without an avatar: the list is the same object, the id is still named.
    expect(reduce(held, 'terminal.removed', { id: 't9' }, 5)).toEqual({ list: held, refetch: false, act: null, removed: 't9' })
    expect(reduce(undefined, 'terminal.removed', { id: 't9' }, 5)).toEqual({ list: undefined, refetch: false, act: null, removed: 't9' })
    expect(reduce(held, 'terminal.removed', {}, 5).removed).toBeNull()
  })

  it('ignores the nudge, which is the notifier’s, and every event of another slice', () => {
    const held = list([record('t1')])
    for (const type of ['avatar.nudge', 'terminal.updated', 'agent.attention', 'constructor', '__proto__', '']) {
      expect(reduce(held, type, { terminalId: 't1', askId: 'Q1', name: 'Pip', question: '?' }, 5)).toEqual({ list: held, refetch: false, act: null, removed: null })
    }
  })
})

describe('mergeFetched: a record no event touched', () => {
  it('is the newer of the two, whole', () => {
    const newer = record('t1', { updatedAt: 9, name: 'Pip' })
    const fetched = list([record('t1', { updatedAt: 5 }), record('t2', { updatedAt: 7 })])
    const out = mergeFetched(list([newer]), fetched)
    expect(out.avatars[0]).toBe(newer)
    expect(out.avatars[1]).toBe(fetched.avatars[1])
    const same = list([record('t1', { updatedAt: 9, name: 'fetched' })])
    expect(mergeFetched(list([newer]), same)).toBe(same)
    expect(mergeFetched(undefined, fetched)).toBe(fetched)
  })

  it('lets the answer say which avatars exist, and everything of the list itself', () => {
    const out = mergeFetched(list([record('gone', { updatedAt: 99 }), record('t1', { updatedAt: 9 })], { asked: ['a'] }), list([record('t1', { updatedAt: 5 })], { enabled: false, asked: ['b'] }))
    expect(out.avatars.map((a) => a.terminalId)).toEqual(['t1'])
    expect(out).toMatchObject({ enabled: false, asked: ['b'] })
  })
})

describe('mergeFetched: a record events changed', () => {
  const look = { viewBox: [0, 0, 200, 200], nodes: [], pivots: {}, roles: {}, animations: {}, states: {}, mouth: [], gaze: null } as unknown as Look

  it('always takes the design from the answer, also when the answer is older than the event', () => {
    // Designed at 100, said at 105; the fetch the design asked for arrives after the say's event.
    const held = reduce(list([record('t1', { updatedAt: 50 })]), 'avatar.say', { terminalId: 't1', line: line('L1', 105) }, 107).list
    for (const fetchedAt of [100, 105, 107, 110]) {
      const answer = record('t1', { updatedAt: fetchedAt, revision: 2, designed: true, look, name: 'Pip', voice: { lang: 'en-GB' }, lines: fetchedAt >= 105 ? [line('L1', 105)] : [] })
      const out = of(mergeFetched(held, list([answer])), 't1')
      expect([out.revision, out.designed, out.name, out.look, out.voice], String(fetchedAt)).toEqual([2, true, 'Pip', look, { lang: 'en-GB' }])
      // And the line the event told is still there, once.
      expect(ids(out), String(fetchedAt)).toEqual(['L1'])
    }
  })

  it('keeps a line said after the answer was made, and drops one the server dropped before', () => {
    let held: AvatarList | undefined = list([record('t1', { updatedAt: 50, lines: [line('L1', 40, { outcome: 'spoken' }), line('L2', 45)] })])
    held = reduce(held, 'avatar.say', { terminalId: 't1', line: line('L3', 60) }, 61).list
    // The answer was made at 55: L2 is gone there (the tab was closed), L3 was not said yet.
    const out = of(mergeFetched(held, list([record('t1', { updatedAt: 55, lines: [line('L1', 40, { outcome: 'spoken' })] })])), 't1')
    expect(ids(out)).toEqual(['L1', 'L3'])
    expect(out.told).toEqual({ ask: 0, held: 0 })
    expect(out.updatedAt).toBe(61)
  })

  it('keeps the better outcome of a line, whoever knows it', () => {
    let held: AvatarList | undefined = list([record('t1', { updatedAt: 50, lines: [line('L1', 40), line('L2', 45)] })])
    held = reduce(held, 'avatar.line', { terminalId: 't1', line: line('L1', 40, { outcome: 'spoken', voice: 'Daniel' }) }, 70).list
    const out = of(mergeFetched(held, list([record('t1', { updatedAt: 60, lines: [line('L1', 40, { outcome: 'shown' }), line('L2', 45, { outcome: 'unheard' })] })])), 't1')
    expect(out.lines).toEqual([line('L1', 40, { outcome: 'spoken', voice: 'Daniel' }), line('L2', 45, { outcome: 'unheard' })])
  })

  it('keeps the question an event told more recently, and takes the answer’s otherwise', () => {
    const q1 = ask('Q1', 'L1', { askedAt: 40 })
    const q2 = ask('Q2', 'L2', { askedAt: 80 })
    // Closed by an event at 70; the answer was made at 60, when it was still open.
    let held = reduce(list([record('t1', { updatedAt: 50, ask: q1 })]), 'avatar.ask', { terminalId: 't1', change: 'answered', ask: null }, 70).list
    expect(of(mergeFetched(held, list([record('t1', { updatedAt: 60, ask: q1 })])), 't1').ask).toBeNull()
    // The same time on both sides: the event still stands.
    expect(of(mergeFetched(held, list([record('t1', { updatedAt: 70, ask: q1 })])), 't1').ask).toBeNull()
    // Opened while this tab was not listening; the answer knows, the last event here is older.
    expect(of(mergeFetched(held, list([record('t1', { updatedAt: 85, ask: q2 })])), 't1').ask).toEqual(q2)
    // An event about lines says nothing of the question: a missed question is taken from the answer.
    held = reduce(list([record('t1', { updatedAt: 50 })]), 'avatar.line', { terminalId: 't1', line: line('L9', 90, { outcome: 'unheard' }) }, 95).list
    expect(of(mergeFetched(held, list([record('t1', { updatedAt: 85, ask: q2, lines: [line('L2', 80)] })])), 't1').ask).toEqual(q2)
  })

  it('weighs a held gesture the same way, and never starts the same play over', () => {
    let held = reduce(list([record('t1', { updatedAt: 50 })]), 'avatar.act', { terminalId: 't1', play: 'lean', mode: 'hold', seq: 1 }, 72).list
    // The answer names the same play with the server's own time.
    const sameAnswer = list([record('t1', { updatedAt: 80, held: { name: 'lean', mode: 'hold', since: 71 } })])
    expect(of(mergeFetched(held, sameAnswer), 't1').held).toEqual({ name: 'lean', mode: 'hold', since: 72 })
    // An answer made before the play does not end it.
    expect(of(mergeFetched(held, list([record('t1', { updatedAt: 60 })])), 't1').held).toEqual({ name: 'lean', mode: 'hold', since: 72 })
    // A newer answer does: it was stopped while this tab was not listening.
    expect(of(mergeFetched(held, list([record('t1', { updatedAt: 99 })])), 't1').held).toBeNull()
    // Another play of the same gesture, much later, is another play.
    held = reduce(list([record('t1', { updatedAt: 50, held: { name: 'lean', mode: 'hold', since: 10 } })]), 'avatar.line', { terminalId: 't1', line: line('L1', 20) }, 21).list
    expect(of(mergeFetched(held, list([record('t1', { updatedAt: 9000, held: { name: 'lean', mode: 'hold', since: 8990 } })])), 't1').held?.since).toBe(8990)
  })

  it('stands as fetched once nothing an event told is newer, and is then weighed whole again', () => {
    const held = reduce(list([record('t1', { updatedAt: 50, lines: [line('L1', 40)] })]), 'avatar.line', { terminalId: 't1', line: line('L1', 40, { outcome: 'shown' }) }, 61).list
    const answer = list([record('t1', { updatedAt: 60, lines: [line('L1', 40, { outcome: 'spoken' })] })])
    const out = mergeFetched(held, answer)
    expect(out).toBe(answer)
    expect(of(out, 't1').told).toBeUndefined()
    expect(of(out, 't1').updatedAt).toBe(60)
  })

  it('does not lose a question to an event about a line that arrives during a refetch after a reconnect', () => {
    // While the socket was down, Q2 opened (the answer knows it). Right after the reconnect another device acknowledges L2.
    const q2 = ask('Q2', 'L2', { askedAt: 80 })
    const held = reduce(list([record('t1', { updatedAt: 50 })]), 'avatar.line', { terminalId: 't1', line: line('L2', 80, { outcome: 'shown' }) }, 90).list
    const out = of(mergeFetched(held, list([record('t1', { updatedAt: 82, ask: q2, lines: [line('L2', 80)] })])), 't1')
    expect(out.ask).toEqual(q2)
    expect(out.lines).toEqual([line('L2', 80, { outcome: 'shown' })])
  })

  it('never holds more lines than the server keeps', () => {
    const fetchedLines = Array.from({ length: MAX_LINES }, (_, i) => line(`L${i}`, i))
    const held = reduce(list([record('t1', { updatedAt: 50, lines: fetchedLines })]), 'avatar.say', { terminalId: 't1', line: line('Lnew', 200) }, 201).list
    const out = of(mergeFetched(held, list([record('t1', { updatedAt: 60, lines: fetchedLines })])), 't1')
    expect(out.lines).toHaveLength(MAX_LINES)
    expect(ids(out).at(-1)).toBe('Lnew')
  })

  it('keeps the open question’s line when it trims what it holds to what the server keeps', () => {
    const fetchedLines = Array.from({ length: MAX_LINES }, (_, i) => line(`L${i}`, i))
    const held = reduce(list([record('t1', { updatedAt: 50, lines: fetchedLines })]), 'avatar.say', { terminalId: 't1', line: line('Lnew', 200) }, 201).list
    const out = of(mergeFetched(held, list([record('t1', { updatedAt: 60, lines: fetchedLines, ask: ask('Q1', 'L0') })])), 't1')
    expect(out.lines).toHaveLength(MAX_LINES)
    expect(ids(out)[0]).toBe('L0')
    expect(ids(out)).not.toContain('L1')
    expect(ids(out).at(-1)).toBe('Lnew')
  })
})

describe('through the query cache', () => {
  it('keeps the note of what events told when the cache shares structure between versions', () => {
    const qc = new QueryClient()
    const key = ['avatar']
    qc.setQueryData(key, list([record('t1', { updatedAt: 50, lines: [line('L1', 40)] })]))
    const apply = (type: string, data: unknown, ts: number) => qc.setQueryData(key, reduce(qc.getQueryData<AvatarList>(key), type, data, ts).list)
    apply('avatar.say', { terminalId: 't1', line: line('L2', 60) }, 61)
    apply('avatar.ask', { terminalId: 't1', change: 'opened', ask: ask('Q1', 'L2') }, 62)
    const held = qc.getQueryData<AvatarList>(key)
    expect(of(held, 't1').told).toEqual({ ask: 62, held: 0 })
    // A stale answer (made at 55) lands: the line and the question stay, the design is taken.
    const merged = mergeFetched(held, list([record('t1', { updatedAt: 55, name: 'Pip', lines: [line('L1', 40)] })]))
    qc.setQueryData(key, merged)
    const now = of(qc.getQueryData<AvatarList>(key), 't1')
    expect([now.name, ids(now), now.ask?.id]).toEqual(['Pip', ['L1', 'L2'], 'Q1'])
  })
})

describe('sameValue', () => {
  it('compares what the list holds by content', () => {
    expect(sameValue({ a: [1, { b: null }], c: 'x' }, { c: 'x', a: [1, { b: null }] })).toBe(true)
    expect(sameValue([1, 2], [2, 1])).toBe(false)
    expect(sameValue({ a: 1 }, { a: 1, b: undefined })).toBe(false)
    expect(sameValue(null, {})).toBe(false)
    expect(sameValue([], {})).toBe(false)
    expect(sameValue(1, 1)).toBe(true)
    expect(sameValue('1', 1)).toBe(false)
  })
})
