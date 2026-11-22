// Who is on stage: which avatars show (the sleeping minute, another project's question),
// which one is the active one and how sticky it is, and what of a transcript is unseen.

import { describe, expect, it } from 'vitest'
import { DESIGNING_MS, MAX_FIGURES, NOBODY, SHRINK_AFTER_MS, SLEEPING_SHOWN_MS, figures, front, isDesigning, mostUrgent, nextLeave, noteExits, pickActive, presented, sameStage, seenAfter, transcript, unacknowledged, unseen, whoShows } from './stage'
import type { Active, ActiveFacts, Candidate, Exits, Session } from './stage'
import type { Ask, AvatarInfo, Line } from './types'

const line = (id: string, over: Partial<Line> = {}): Line => ({ id, at: 1, from: 'agent', text: `line ${id}`, ...over })
const ask = (id: string, lineId: string): Ask => ({ id, lineId, text: 'Shall I?', choices: [], freeText: true, askedAt: 1, waiting: false })
const avatar = (terminalId: string, over: Partial<AvatarInfo> = {}): AvatarInfo => ({ terminalId, projectId: 'p1', on: true, name: '', designed: false, revision: 1, look: null, voice: {}, via: 'tools', held: null, ask: null, lines: [], createdAt: 1, updatedAt: 1, ...over })
const session = (id: string, over: Partial<Session> = {}): Session => ({ id, projectId: 'p1', agent: true, open: true, running: true, pinned: false, order: 0, createdAt: 1, ...over })
const none: Exits = new Map()

describe('noteExits', () => {
  it('notes when this window saw a session exit, and that one seen exited first did so long ago', () => {
    let exits = noteExits(none, [session('a'), session('b', { running: false }), session('sh', { agent: false })], 1000)
    expect([...exits]).toEqual([
      ['a', 'running'],
      ['b', Number.NEGATIVE_INFINITY],
    ])
    exits = noteExits(exits, [session('a', { running: false }), session('b', { running: false })], 5000)
    expect(exits.get('a')).toBe(5000)
    expect(exits.get('b')).toBe(Number.NEGATIVE_INFINITY)
    // The time stays what it was first seen at.
    const later = noteExits(exits, [session('a', { running: false }), session('b', { running: false })], 9000)
    expect(later).toBe(exits)
    // Restarted: running again, and a later exit is a new one.
    const again = noteExits(later, [session('a'), session('b', { running: false })], 10_000)
    expect(again.get('a')).toBe('running')
    expect(noteExits(again, [session('a', { running: false }), session('b', { running: false })], 12_000).get('a')).toBe(12_000)
  })

  it('forgets a session that is gone', () => {
    const exits = noteExits(none, [session('a'), session('b')], 1)
    expect([...noteExits(exits, [session('b')], 2).keys()]).toEqual(['b'])
  })
})

describe('whoShows', () => {
  const base = { enabled: true, projectId: 'p1', exits: none, now: 100_000 }

  it('shows the avatars that are on and whose session is an open agent session of the project on screen', () => {
    const avatars = [avatar('a'), avatar('off', { on: false }), avatar('closed'), avatar('shell'), avatar('other', { projectId: 'p2' }), avatar('ghost')]
    const sessions = [session('a'), session('off'), session('closed', { open: false }), session('shell', { agent: false }), session('other', { projectId: 'p2' })]
    expect(whoShows({ ...base, avatars, sessions })).toEqual([{ terminalId: 'a', foreign: false, leavesAt: null }])
  })

  it('shows nothing while avatars are disabled on the server', () => {
    expect(whoShows({ ...base, enabled: false, avatars: [avatar('a')], sessions: [session('a')] })).toEqual([])
  })

  it('keeps a session that exited for a minute, asleep, and takes it back when it runs again', () => {
    const avatars = [avatar('a')]
    const exited = [session('a', { running: false })]
    const exits: Exits = new Map([['a', 100_000]])
    expect(whoShows({ ...base, avatars, sessions: exited, exits, now: 100_000 })).toEqual([{ terminalId: 'a', foreign: false, leavesAt: 100_000 + SLEEPING_SHOWN_MS }])
    expect(whoShows({ ...base, avatars, sessions: exited, exits, now: 100_000 + SLEEPING_SHOWN_MS - 1 })).toHaveLength(1)
    expect(whoShows({ ...base, avatars, sessions: exited, exits, now: 100_000 + SLEEPING_SHOWN_MS })).toEqual([])
    // It had exited before this window knew it: not shown at all.
    expect(whoShows({ ...base, avatars, sessions: exited, exits: new Map([['a', Number.NEGATIVE_INFINITY]]) })).toEqual([])
    // Restarted.
    expect(whoShows({ ...base, avatars, sessions: [session('a')], exits: new Map([['a', 'running']]), now: 999_999 })).toEqual([{ terminalId: 'a', foreign: false, leavesAt: null }])
  })

  it('shows another project’s avatar only while it has an open question and its session runs', () => {
    const q = ask('Q1', 'L1')
    const sessions = [session('here'), session('there', { projectId: 'p2' }), session('quiet', { projectId: 'p2' }), session('dead', { projectId: 'p2', running: false })]
    const avatars = [avatar('there', { projectId: 'p2', ask: q }), avatar('quiet', { projectId: 'p2' }), avatar('dead', { projectId: 'p2', ask: q }), avatar('here')]
    expect(whoShows({ ...base, avatars, sessions })).toEqual([
      { terminalId: 'here', foreign: false, leavesAt: null },
      { terminalId: 'there', foreign: true, leavesAt: null },
    ])
    // The same avatars are the project's own once that project is on screen (and the other project's is gone: it does not ask).
    const exits: Exits = new Map([['dead', Number.NEGATIVE_INFINITY]])
    expect(whoShows({ ...base, projectId: 'p2', avatars, sessions, exits }).map((s) => [s.terminalId, s.foreign])).toEqual([
      ['quiet', false],
      ['there', false],
    ])
  })

  it('orders them like the tabs of the agents column: pinned first, then the server’s order', () => {
    const sessions = [session('c', { order: 3 }), session('a', { order: 1 }), session('p', { order: 9, pinned: true }), session('b', { order: 1, createdAt: 5 })]
    const avatars = ['a', 'b', 'c', 'p'].map((id) => avatar(id))
    expect(whoShows({ ...base, avatars, sessions }).map((s) => s.terminalId)).toEqual(['p', 'a', 'b', 'c'])
  })

  it('treats a session without a project as one of the window without a project', () => {
    const avatars = [avatar('a', { projectId: null })]
    expect(whoShows({ ...base, projectId: null, avatars, sessions: [session('a', { projectId: null })] })).toHaveLength(1)
    expect(whoShows({ ...base, projectId: 'p1', avatars, sessions: [session('a', { projectId: null })] })).toEqual([])
  })

  it('tells when the stage changes by itself next, and whether two stagings are the same', () => {
    const staged = [
      { terminalId: 'a', foreign: false, leavesAt: 9000 },
      { terminalId: 'b', foreign: false, leavesAt: null },
      { terminalId: 'c', foreign: false, leavesAt: 7000 },
    ]
    expect(nextLeave(staged)).toBe(7000)
    expect(nextLeave([staged[1]])).toBeNull()
    expect(sameStage(staged, staged.map((s) => ({ ...s })))).toBe(true)
    expect(sameStage(staged, staged.slice(0, 2))).toBe(false)
    expect(sameStage(staged, [staged[0], { ...staged[1], foreign: true }, staged[2]])).toBe(false)
  })
})

describe('presented', () => {
  it('leaves a record of the project as it is', () => {
    const a = avatar('a', { lines: [line('L1'), line('L2')] })
    expect(presented(a, false)).toBe(a)
  })

  it('presents of another project’s avatar only the line of its open question', () => {
    const a = avatar('a', { ask: ask('Q1', 'L2'), lines: [line('L1'), line('L2', { askId: 'Q1' }), line('L3'), line('L4', { from: 'user' })] })
    const only = presented(a, true)
    expect(only.lines.map((l) => l.id)).toEqual(['L2'])
    expect(only.ask).toBe(a.ask)
    expect(only.look).toBe(a.look)
    // The same object for the same record: nothing downstream is rebuilt.
    expect(presented(a, true)).toBe(only)
    expect(presented(avatar('b', { lines: [line('L1')] }), true).lines).toEqual([])
  })
})

describe('figures and the front avatar', () => {
  it('shows up to four and puts the rest behind the chip', () => {
    expect(MAX_FIGURES).toBe(4)
    expect(figures(['a', 'b', 'c'], null)).toEqual({ visible: ['a', 'b', 'c'], more: [] })
    expect(figures(['a', 'b', 'c', 'd'], 'd')).toEqual({ visible: ['a', 'b', 'c', 'd'], more: [] })
    expect(figures(['a', 'b', 'c', 'd', 'e', 'f'], null)).toEqual({ visible: ['a', 'b', 'c', 'd'], more: ['e', 'f'] })
  })

  it('gives the active avatar the last place when it would be behind the chip', () => {
    expect(figures(['a', 'b', 'c', 'd', 'e', 'f'], 'f')).toEqual({ visible: ['a', 'b', 'c', 'f'], more: ['e', 'd'] })
    expect(figures(['a', 'b', 'c', 'd', 'e', 'f'], 'b')).toEqual({ visible: ['a', 'b', 'c', 'd'], more: ['e', 'f'] })
    expect(figures(['a', 'b', 'c'], 'c', 2)).toEqual({ visible: ['a', 'c'], more: ['b'] })
  })

  it('stands in the compact form for the active avatar, else the session on screen, else the first', () => {
    expect(front(['a', 'b', 'c'], 'c', 'b')).toBe('c')
    expect(front(['a', 'b', 'c'], null, 'b')).toBe('b')
    expect(front(['a', 'b', 'c'], 'gone', 'gone')).toBe('a')
    expect(front([], null, null)).toBeNull()
  })

  it('stands for one that waits with a question before the session on screen, and never before the active one', () => {
    expect(front(['a', 'b', 'c'], null, 'a', ['c'])).toBe('c')
    expect(front(['a', 'b', 'c'], 'b', 'a', ['c'])).toBe('b')
    // Of several, the session on screen's first, else the first on stage.
    expect(front(['a', 'b', 'c'], null, 'c', ['b', 'c'])).toBe('c')
    expect(front(['a', 'b', 'c'], null, 'a', ['b', 'c'])).toBe('b')
    // One that is not on stage waits for nobody here.
    expect(front(['a', 'b'], null, 'b', ['gone'])).toBe('b')
    expect(front(['a', 'b'], null, 'gone', ['gone'])).toBe('a')
    expect(front([], null, null, ['a'])).toBeNull()
  })
})

describe('pickActive', () => {
  const c = (id: string, over: Partial<Candidate> = {}): Candidate => ({ id, blocked: false, askId: null, draft: false, ...over })
  const facts = (over: Partial<ActiveFacts> = {}): ActiveFacts => ({ now: 0, shown: [c('a'), c('b'), c('c')], onScreen: null, presenting: null, hovered: null, focused: null, overGroup: false, opened: false, bubbleOpen: false, listening: false, ...over })
  const active = (id: string, idleSince: number | null = null): Active => ({ id, idleSince })

  it('is nobody while nothing gives a reason', () => {
    expect(pickActive(NOBODY, facts())).toBe(NOBODY)
  })

  it('is the avatar that presents a line, has an open question, is hovered or focused', () => {
    expect(pickActive(NOBODY, facts({ presenting: 'b' }))).toEqual(active('b'))
    expect(pickActive(NOBODY, facts({ shown: [c('a'), c('b', { askId: 'Q1' })] }))).toEqual(active('b'))
    expect(pickActive(NOBODY, facts({ hovered: 'c' }))).toEqual(active('c'))
    expect(pickActive(NOBODY, facts({ focused: 'a' }))).toEqual(active('a'))
  })

  it('prefers a question over a hover, and the question of the session on screen over another', () => {
    const shown = [c('a', { askId: 'Q1' }), c('b', { askId: 'Q2' }), c('c')]
    expect(pickActive(NOBODY, facts({ shown, hovered: 'c' }))).toEqual(active('a'))
    expect(pickActive(NOBODY, facts({ shown, onScreen: 'b' }))).toEqual(active('b'))
    expect(pickActive(NOBODY, facts({ shown, onScreen: 'c' }))).toEqual(active('a'))
  })

  it('lets a line that is presented take the stage from an avatar that only has a question', () => {
    const shown = [c('a', { askId: 'Q1' }), c('b')]
    expect(pickActive(active('a'), facts({ shown, presenting: 'b' }))).toEqual(active('b'))
    // And gives it back once the line and its six seconds are over.
    const idle = pickActive(active('b'), facts({ shown, now: 1000 }))
    expect(idle).toEqual(active('b', 1000))
    expect(pickActive(idle, facts({ shown, now: 1000 + SHRINK_AFTER_MS - 1 }))).toBe(idle)
    expect(pickActive(idle, facts({ shown, now: 1000 + SHRINK_AFTER_MS }))).toEqual(active('a'))
  })

  it('does not change while the pointer is over the group', () => {
    const prev = active('a', 500)
    expect(pickActive(prev, facts({ now: 99_999, overGroup: true, presenting: 'b' }))).toEqual(active('a'))
    expect(pickActive(prev, facts({ now: 99_999, overGroup: true, hovered: 'b' }))).toEqual(active('a'))
    // The pointer left: the line that is still being presented takes the stage.
    expect(pickActive(active('a'), facts({ now: 99_999, presenting: 'b' }))).toEqual(active('b'))
  })

  it('does not change while focus is inside the group', () => {
    expect(pickActive(active('a'), facts({ focused: 'b', presenting: 'c' }))).toEqual(active('a'))
    expect(pickActive(active('a'), facts({ focused: 'a', presenting: 'c' }))).toEqual(active('a'))
  })

  it('does not change while the reply box of the active avatar holds text', () => {
    const shown = [c('a', { draft: true }), c('b')]
    expect(pickActive(active('a', 0), facts({ shown, now: 60_000, presenting: 'b', bubbleOpen: true }))).toEqual(active('a'))
    // A draft of another avatar does not hold the stage.
    expect(pickActive(active('b', 0), facts({ shown, now: 60_000, presenting: 'a', bubbleOpen: true }))).toEqual(active('a'))
  })

  it('lets a draft kept behind a closed bubble hold nothing: the reply box is not on screen', () => {
    const shown = [c('a', { draft: true }), c('b')]
    expect(pickActive(active('a', 0), facts({ shown, now: 60_000, presenting: 'b', bubbleOpen: false }))).toEqual(active('b'))
    expect(pickActive(active('a', 0), facts({ shown, now: 60_000, presenting: 'b', bubbleOpen: true }))).toEqual(active('a'))
    // Nor does it keep the figure large once its six seconds are over.
    expect(pickActive(active('a', 0), facts({ shown, now: SHRINK_AFTER_MS }))).toBe(NOBODY)
  })

  it('does not change while the microphone of the active avatar’s open bubble listens, before any words came', () => {
    const shown = [c('a', { askId: 'Q1' }), c('b')]
    // A tap on the microphone leaves focus where it was, and a lifted finger is nowhere: only the dictation holds.
    expect(pickActive(active('a'), facts({ shown, presenting: 'b', bubbleOpen: true, listening: true }))).toEqual(active('a'))
    expect(pickActive(active('a'), facts({ shown, presenting: 'b', bubbleOpen: true }))).toEqual(active('b'))
    // A microphone in no open bubble holds nothing.
    expect(pickActive(active('a'), facts({ shown, presenting: 'b', listening: true }))).toEqual(active('b'))
    const prev = active('a')
    expect(pickActive(prev, facts({ shown, presenting: 'b', bubbleOpen: true, listening: true }))).toBe(prev)
  })

  it('never leaves a bubble the user opened', () => {
    expect(pickActive(active('a', 0), facts({ now: 60_000, opened: true, bubbleOpen: true, presenting: 'b', shown: [c('a'), c('b', { askId: 'Q1' })] }))).toEqual(active('a'))
  })

  it('shrinks six seconds after its last reason ended, and not beside an open bubble', () => {
    const prev = pickActive(NOBODY, facts({ now: 100, hovered: 'a' }))
    expect(prev).toEqual(active('a'))
    const left = pickActive(prev, facts({ now: 2000 }))
    expect(left).toEqual(active('a', 2000))
    expect(pickActive(left, facts({ now: 2000 + SHRINK_AFTER_MS - 1 }))).toBe(left)
    expect(pickActive(left, facts({ now: 2000 + SHRINK_AFTER_MS }))).toBe(NOBODY)
    // The bubble of a line that was presented silently is still being read.
    expect(pickActive(left, facts({ now: 60_000, bubbleOpen: true }))).toBe(left)
    // A reason that comes back starts the count over.
    const back = pickActive(left, facts({ now: 4000, hovered: 'a' }))
    expect(back).toEqual(active('a'))
    expect(pickActive(back, facts({ now: 5000 }))).toEqual(active('a', 5000))
  })

  it('stays the same object while nothing changes, whatever keeps it', () => {
    const prev = active('a')
    expect(pickActive(prev, facts({ presenting: 'a' }))).toBe(prev)
    expect(pickActive(prev, facts({ shown: [c('a', { askId: 'Q1' })] }))).toBe(prev)
    expect(pickActive(prev, facts({ overGroup: true }))).toBe(prev)
    expect(pickActive(prev, facts({ focused: 'b' }))).toBe(prev)
    expect(pickActive(prev, facts({ shown: [c('a', { draft: true })], bubbleOpen: true }))).toBe(prev)
    expect(pickActive(prev, facts({ opened: true, bubbleOpen: true }))).toBe(prev)
    expect(pickActive(prev, facts({ hovered: 'a' }))).toBe(prev)
    const idle = active('a', 100)
    expect(pickActive(idle, facts({ now: 200 }))).toBe(idle)
    expect(pickActive(idle, facts({ now: 999_999, bubbleOpen: true }))).toBe(idle)
    expect(pickActive(NOBODY, facts({ now: 5 }))).toBe(NOBODY)
    // Asked again with the answer it gave, it gives that answer back.
    for (const f of [facts({ presenting: 'b' }), facts({ hovered: 'c' }), facts({ shown: [c('a'), c('b', { askId: 'Q' })] }), facts({ now: 50_000 })]) {
      const once = pickActive(idle, f)
      expect(pickActive(once, f)).toBe(once)
    }
  })

  it('makes no avatar active for a line or a question while its session needs the user', () => {
    const shown = [c('a', { blocked: true, askId: 'Q1' }), c('b')]
    expect(pickActive(NOBODY, facts({ shown, presenting: 'a' }))).toBe(NOBODY)
    // A hover or a click still does.
    expect(pickActive(NOBODY, facts({ shown, hovered: 'a' }))).toEqual(active('a'))
  })

  it('gives the stage to no guest for its question: another project’s avatar rests until the user turns to it', () => {
    const shown = [c('a'), c('g', { askId: 'Q1', guest: true })]
    expect(pickActive(NOBODY, facts({ shown }))).toBe(NOBODY)
    // Its question is presented like any line, and then it has its six seconds like any other.
    const speaking = pickActive(NOBODY, facts({ shown, presenting: 'g' }))
    expect(speaking).toEqual(active('g'))
    const after = pickActive(speaking, facts({ shown, now: 1000 }))
    expect(after).toEqual(active('g', 1000))
    expect(pickActive(after, facts({ shown, now: 1000 + SHRINK_AFTER_MS }))).toBe(NOBODY)
    // A hover or a click still makes it the active one, and the project's own question goes before it.
    expect(pickActive(NOBODY, facts({ shown, hovered: 'g' }))).toEqual(active('g'))
    expect(pickActive(NOBODY, facts({ shown: [c('a', { askId: 'Q2' }), c('g', { askId: 'Q1', guest: true })], onScreen: 'g' }))).toEqual(active('a'))
  })

  it('lets go of an avatar that left the stage', () => {
    expect(pickActive(active('gone'), facts())).toBe(NOBODY)
    expect(pickActive(active('gone'), facts({ presenting: 'b', opened: true, overGroup: true }))).toEqual(active('b'))
    expect(pickActive(NOBODY, facts({ presenting: 'nobody', hovered: 'nobody', focused: 'nobody', onScreen: 'nobody' }))).toBe(NOBODY)
  })
})

describe('seen and unseen', () => {
  const lines = [line('L1'), line('U1', { from: 'user' }), line('L2'), line('L3')]

  it('names the newest agent line and the first one not seen', () => {
    expect(unseen(lines, undefined)).toEqual({ newest: 'L3', from: 'L1' })
    expect(unseen(lines, 'L1')).toEqual({ newest: 'L3', from: 'L2' })
    expect(unseen(lines, 'L3')).toEqual({ newest: 'L3', from: null })
    // What was seen has left the transcript: everything in it is new.
    expect(unseen(lines, 'L0')).toEqual({ newest: 'L3', from: 'L1' })
    // A reply of the user is no line of the avatar.
    expect(unseen(lines, 'U1')).toEqual({ newest: 'L3', from: 'L1' })
    expect(unseen([line('U1', { from: 'user' })], undefined)).toEqual({ newest: null, from: null })
    expect(unseen([], 'L1')).toEqual({ newest: null, from: null })
  })

  it('remembers the later of what was seen and what is on screen now', () => {
    expect(seenAfter(lines, undefined, 'L2')).toBe('L2')
    expect(seenAfter(lines, 'L1', 'L3')).toBe('L3')
    expect(seenAfter(lines, 'L3', 'L1')).toBe('L3')
    expect(seenAfter(lines, 'L0', 'L1')).toBe('L1')
    // A line that is not in the transcript, or is the user's, changes nothing.
    expect(seenAfter(lines, 'L1', 'L9')).toBe('L1')
    expect(seenAfter(lines, 'L1', 'U1')).toBe('L1')
  })

  it('shows the last six lines of a transcript, starting to read at the first unseen one', () => {
    const many = Array.from({ length: 9 }, (_, i) => line(`L${i + 1}`))
    expect(transcript(many, 'L7').lines.map((l) => l.id)).toEqual(['L4', 'L5', 'L6', 'L7', 'L8', 'L9'])
    expect(transcript(many, 'L7').unseenFrom).toBe('L8')
    expect(transcript(many, 'L9').unseenFrom).toBeNull()
    // The unseen lines begin before what is shown: all of it is new.
    expect(transcript(many, 'L1').unseenFrom).toBe('L4')
    expect(transcript([line('U0', { from: 'user' }), ...many].slice(0, 6), undefined).unseenFrom).toBe('L1')
    expect(transcript([], undefined)).toEqual({ lines: [], unseenFrom: null })
    expect(transcript(many, 'L7', 2).lines.map((l) => l.id)).toEqual(['L8', 'L9'])
  })
})

describe('small facts', () => {
  it('lists the lines nobody acknowledged, each once', () => {
    const avatars = [avatar('a', { lines: [line('L1', { outcome: 'spoken' }), line('L2'), line('U1', { from: 'user' }), line('L3')] }), avatar('b', { lines: [line('L1')] })]
    expect(unacknowledged(avatars, new Set(['a:L2']))).toEqual([
      { terminalId: 'a', lineId: 'L3', key: 'a:L3' },
      { terminalId: 'b', lineId: 'L1', key: 'b:L1' },
    ])
    expect(unacknowledged([], new Set())).toEqual([])
  })

  it('says "Designing its look" only for a fresh session with the option that is working and has not designed', () => {
    const fresh = { withOption: true, designed: false, working: true, startedAt: 1000, now: 1000 + DESIGNING_MS - 1 }
    expect(isDesigning(fresh)).toBe(true)
    expect(isDesigning({ ...fresh, now: 1000 + DESIGNING_MS })).toBe(false)
    expect(isDesigning({ ...fresh, withOption: false })).toBe(false)
    expect(isDesigning({ ...fresh, designed: true })).toBe(false)
    expect(isDesigning({ ...fresh, working: false })).toBe(false)
  })

  it('gives the chip the most urgent ring of the avatars behind it', () => {
    expect(mostUrgent(['idle', 'working', 'unread'])).toBe('unread')
    expect(mostUrgent(['working', 'attention', 'error'])).toBe('attention')
    expect(mostUrgent(['exited'])).toBe('exited')
    expect(mostUrgent(['speaking', 'working'])).toBe('speaking')
    expect(mostUrgent([])).toBe('idle')
  })
})
