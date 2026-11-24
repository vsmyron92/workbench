// The bubble's timeline (every timing of it), the click-through guard, and what a reply
// the user is writing answers.

import { describe, expect, it } from 'vitest'
import { CHOICE_GUARD_MS, CLOSE_AFTER_MS, EMPTY, HOLD_MOVE_PX, HOLD_MS, HOVER_DWELL_MS, IN_BOX, MAX_DRAFT_CHARS, NOTE_MS, NO_DRAFT, THROUGH_MS, armGuard, askChanged, askedToHear, choicesReady, clicksThrough, closeByUser, edited, guardStep, guardWake, holding, noteEdited, noteStep, noteWake, noted, openByUser, planChoice, planSend, pressed, questionChanged, readingMs, replyNoted, replyShown, sentNote, settle, voiceForm, whyDisabled } from './bubble'
import type { Facts, Presenting, Scene } from './bubble'
import { SHRINK_AFTER_MS } from './stage'
import type { Candidate } from './stage'
import type { Ask } from './types'

const c = (id: string, over: Partial<Candidate> = {}): Candidate => ({ id, blocked: false, askId: null, draft: false, ...over })
const facts = (now: number, over: Partial<Facts> = {}): Facts => ({ now, shown: [c('a'), c('b')], onScreen: null, presenting: null, hovered: null, focused: null, overGroup: false, inBubble: false, pointer: null, focusInBubble: false, focusInReply: false, listening: false, held: false, ...over })
const saying = (terminalId: string, lineId: string, over: Partial<Presenting> = {}): Presenting => ({ terminalId, lineId, silent: false, chars: 30, ...over })
const ask = (id: string, over: Partial<Ask> = {}): Ask => ({ id, lineId: 'L1', text: 'Shall I push?', choices: ['Yes, push it', 'Not yet'], freeText: true, askedAt: 1, waiting: true, ...over })

/** Settles the scene at each of the given moments with the same facts, and returns the last one. */
function run(scene: Scene, times: number[], over: Partial<Facts> = {}): Scene {
  let s = scene
  for (const t of times) s = settle(s, facts(t, over))
  return s
}

describe('readingMs', () => {
  it('is two seconds and one more for every fifteen characters', () => {
    expect(readingMs(0)).toBe(2000)
    expect(readingMs(15)).toBe(3000)
    expect(readingMs(150)).toBe(12_000)
    expect(readingMs(Number.NaN)).toBe(2000)
    expect(readingMs(-5)).toBe(2000)
  })
})

describe('the guard', () => {
  const at = (x: number, y: number) => ({ x, y })

  it('lets clicks through for half a second after the bubble opened or changed by itself', () => {
    const g = armGuard(1000, at(10, 10))
    expect(g).toEqual({ since: 1000, phase: 'timed', from: { x: 10, y: 10 } })
    expect(guardStep(g, 1000 + THROUGH_MS - 1, false, at(10, 10))).toBe(g)
    expect(guardWake(g, 1000)).toBe(1000 + THROUGH_MS)
  })

  it('ends at once when the pointer was not inside as the half second ran out', () => {
    const g = armGuard(1000, at(10, 10))
    expect(guardStep(g, 1000 + THROUGH_MS, false, at(10, 10))).toBeNull()
    // Nowhere at all (a touch or a pen that was lifted) is not inside either.
    expect(guardStep(armGuard(1000), 1000 + THROUGH_MS, false, null)).toBeNull()
    expect(guardStep(null, 1, true, at(1, 1))).toBeNull()
  })

  it('then holds a pointer that rests inside: the bubble takes it, its controls do not act', () => {
    const g = armGuard(1000, at(100, 100))
    const held = guardStep(g, 1000 + THROUGH_MS, true, at(100, 100))
    expect(held).toEqual({ since: 1000, phase: 'hold', from: { x: 100, y: 100 } })
    // A pointer that does not move away is held as long as it rests there.
    expect(guardStep(held, 60_000, true, at(100, 100))).toBe(held)
    expect(guardStep(held, 60_000, true, at(100 + HOLD_MOVE_PX - 1, 100))).toBe(held)
    expect(guardWake(held, 60_000)).toBeNull()
  })

  it('lets go when the pointer leaves the bubble once', () => {
    const held = guardStep(armGuard(1000, at(100, 100)), 1600, true, at(100, 100))
    expect(held?.phase).toBe('hold')
    expect(guardStep(held, 1700, false, at(100, 100))).toBeNull()
    // Lifting a finger counts as leaving: it is nowhere then.
    expect(guardStep(held, 1700, false, null)).toBeNull()
  })

  it('lets go when a second has passed and the pointer has moved 16 px from where it was', () => {
    const held = guardStep(armGuard(1000, at(100, 100)), 1500, true, at(100, 100))
    // Moved on, but before the second was up: still held, and looked at again when it is.
    const early = guardStep(held, 1000 + HOLD_MS - 1, true, at(100 + HOLD_MOVE_PX, 100))
    expect(early).toBe(held)
    expect(guardWake(held, 1500)).toBe(1000 + HOLD_MS)
    expect(guardStep(held, 1000 + HOLD_MS, true, at(100 + HOLD_MOVE_PX, 100))).toBeNull()
    // Every direction counts the same: 16 px as the crow flies.
    expect(guardStep(held, 1000 + HOLD_MS, true, at(100 + 12, 100 - 11))).toBeNull()
    expect(guardStep(held, 1000 + HOLD_MS, true, at(100 + 11, 100 - 11))).toBe(held)
    // Moved and back again: it is where it was.
    expect(guardStep(held, 5000, true, at(100, 101))).toBe(held)
  })

  it('takes the place where the pointer is first seen when it was nowhere as the bubble opened', () => {
    const g = armGuard(1000, null)
    expect(g.from).toBeNull()
    const seen = guardStep(g, 1200, true, at(40, 40))
    expect(seen).toEqual({ since: 1000, phase: 'timed', from: { x: 40, y: 40 } })
    // Once known, it stays: the same object while nothing else changes.
    expect(guardStep(seen, 1300, true, at(80, 80))).toBe(seen)
    const held = guardStep(seen, 1500, true, at(50, 40))
    expect(held?.from).toEqual({ x: 40, y: 40 })
    expect(guardStep(held, 2000, true, at(56, 40))).toBeNull()
    // A place that is no number is no place.
    expect(armGuard(1000, at(Number.NaN, 1)).from).toBeNull()
  })

  it('is what clicksThrough and holding read from the scene', () => {
    const open = (guard: Scene['guard'], bubble: Scene['bubble'] = 'self'): Scene => ({ ...EMPTY, bubble, guard })
    const timed = armGuard(0)
    const held = { ...timed, phase: 'hold' as const }
    expect([clicksThrough(open(timed)), holding(open(timed))]).toEqual([true, false])
    expect([clicksThrough(open(held)), holding(open(held))]).toEqual([false, true])
    expect([clicksThrough(open(null)), holding(open(null))]).toEqual([false, false])
    expect([clicksThrough(open(held, 'closed')), holding(open(timed, 'closed'))]).toEqual([false, false])
  })
})

describe('settle: a line is presented', () => {
  it('makes its avatar the active one and opens the bubble by itself, click-through at first', () => {
    const s = settle(EMPTY, facts(1000, { presenting: saying('a', 'L1') }))
    expect(s.active).toEqual({ id: 'a', idleSince: null })
    expect([s.bubble, s.lineId, s.presenting]).toEqual(['self', 'L1', true])
    expect(clicksThrough(s)).toBe(true)
    expect(s.wakeAt).toBe(1000 + THROUGH_MS)
    const later = settle(s, facts(1000 + THROUGH_MS, { presenting: saying('a', 'L1') }))
    expect(clicksThrough(later)).toBe(false)
    expect(later.wakeAt).toBeNull()
  })

  it('closes four seconds after a line that was said aloud ended', () => {
    let s = run(EMPTY, [0, 600], { presenting: saying('a', 'L1') })
    s = settle(s, facts(5000))
    expect([s.bubble, s.presenting, s.endedAt]).toEqual(['self', false, 5000])
    expect(s.wakeAt).toBe(5000 + CLOSE_AFTER_MS)
    expect(settle(s, facts(5000 + CLOSE_AFTER_MS - 1)).bubble).toBe('self')
    expect(settle(s, facts(5000 + CLOSE_AFTER_MS)).bubble).toBe('closed')
  })

  it('lets a line presented silently be read first: two seconds and one for every fifteen characters, then the four', () => {
    let s = run(EMPTY, [0, 600], { presenting: saying('a', 'L1', { silent: true, chars: 150 }) })
    s = settle(s, facts(10_000))
    const closeAt = 10_000 + readingMs(150) + CLOSE_AFTER_MS
    expect(closeAt).toBe(26_000)
    expect(s.wakeAt).toBe(closeAt)
    expect(settle(s, facts(closeAt - 1)).bubble).toBe('self')
    expect(settle(s, facts(closeAt)).bubble).toBe('closed')
  })

  it('goes by how the line ended: one that the voice gave up on is read too', () => {
    let s = settle(EMPTY, facts(0, { presenting: saying('a', 'L1', { silent: false, chars: 30 }) }))
    s = settle(s, facts(2000, { presenting: saying('a', 'L1', { silent: true, chars: 30 }) }))
    s = settle(s, facts(4000))
    expect(s.wakeAt).toBe(4000 + readingMs(30) + CLOSE_AFTER_MS)
  })

  it('shrinks the figure six seconds after the line ended, two after the bubble closed', () => {
    let s = run(EMPTY, [0, 600], { presenting: saying('a', 'L1') })
    s = settle(s, facts(5000))
    expect(s.active).toEqual({ id: 'a', idleSince: 5000 })
    s = settle(s, facts(5000 + CLOSE_AFTER_MS))
    expect([s.bubble, s.active.id]).toEqual(['closed', 'a'])
    expect(s.wakeAt).toBe(5000 + SHRINK_AFTER_MS)
    s = settle(s, facts(5000 + SHRINK_AFTER_MS))
    expect(s.active.id).toBeNull()
    expect(s.wakeAt).toBeNull()
  })

  it('does not shrink the figure beside a bubble that is still being read', () => {
    let s = run(EMPTY, [0, 600], { presenting: saying('a', 'L1', { silent: true, chars: 300 }) })
    s = settle(s, facts(1000))
    const closeAt = 1000 + readingMs(300) + CLOSE_AFTER_MS
    s = settle(s, facts(1000 + SHRINK_AFTER_MS + 500))
    expect([s.bubble, s.active.id]).toEqual(['self', 'a'])
    // When the bubble goes, so does the large figure, in the same step.
    s = settle(s, facts(closeAt))
    expect([s.bubble, s.active.id]).toEqual(['closed', null])
  })

  it('shows the next line in the same bubble, and lets clicks through again because it changed', () => {
    let s = run(EMPTY, [0, 600], { presenting: saying('a', 'L1') })
    expect(clicksThrough(s)).toBe(false)
    s = settle(s, facts(3000, { presenting: saying('a', 'L2') }))
    expect([s.bubble, s.lineId]).toEqual(['self', 'L2'])
    expect(clicksThrough(s)).toBe(true)
  })

  it('moves to the avatar whose line is presented next, with a bubble of its own', () => {
    let s = run(EMPTY, [0, 600], { presenting: saying('a', 'L1') })
    s = settle(s, facts(3000, { presenting: saying('b', 'L7') }))
    expect(s.active.id).toBe('b')
    expect([s.bubble, s.lineId, s.openedAt]).toEqual(['self', 'L7', 3000])
  })

  it('stays the same object while nothing changes', () => {
    const s = run(EMPTY, [0, 600], { presenting: saying('a', 'L1') })
    expect(settle(s, facts(700, { presenting: saying('a', 'L1') }))).toBe(s)
    expect(settle(EMPTY, facts(5))).toBe(EMPTY)
  })
})

describe('settle: what holds the bubble open', () => {
  const ended = (): Scene => settle(run(EMPTY, [0, 600], { presenting: saying('a', 'L1') }), facts(1000))

  it('stays while the pointer is inside, and closes four seconds after it left', () => {
    let s = settle(ended(), facts(2000, { inBubble: true }))
    s = settle(s, facts(60_000, { inBubble: true }))
    expect(s.bubble).toBe('self')
    expect(s.wakeAt).toBeNull()
    s = settle(s, facts(61_000))
    expect(s.wakeAt).toBe(61_000 + CLOSE_AFTER_MS)
    expect(settle(s, facts(61_000 + CLOSE_AFTER_MS)).bubble).toBe('closed')
  })

  it('stays while focus is inside', () => {
    let s = settle(ended(), facts(2000, { focusInBubble: true }))
    s = settle(s, facts(90_000, { focusInBubble: true }))
    expect(s.bubble).toBe('self')
    s = settle(s, facts(91_000))
    expect(settle(s, facts(91_000 + CLOSE_AFTER_MS - 1)).bubble).toBe('self')
    expect(settle(s, facts(91_000 + CLOSE_AFTER_MS)).bubble).toBe('closed')
  })

  it('stays while a question is open, and closes four seconds after it was answered', () => {
    const asking = { shown: [c('a', { askId: 'Q1' }), c('b')] }
    let s = run(EMPTY, [0, 600], { ...asking, presenting: saying('a', 'L1') })
    s = run(s, [1000, 500_000], asking)
    expect([s.bubble, s.active.id, s.wakeAt]).toEqual(['self', 'a', null])
    s = settle(s, facts(500_100))
    expect(s.askId).toBeNull()
    expect(s.wakeAt).toBe(500_100 + CLOSE_AFTER_MS)
    expect(settle(s, facts(500_100 + CLOSE_AFTER_MS)).bubble).toBe('closed')
  })

  it('opens by itself for a question that is already open when the page loads', () => {
    const s = settle(EMPTY, facts(1000, { shown: [c('a'), c('b', { askId: 'Q9' })] }))
    expect([s.active.id, s.bubble, s.lineId, s.askId, s.askAt]).toEqual(['b', 'self', null, 'Q9', 1000])
    expect(clicksThrough(s)).toBe(true)
  })

  it('lets clicks through again when the question changes', () => {
    let s = run(EMPTY, [0, 600], { shown: [c('a', { askId: 'Q1' })] })
    expect(clicksThrough(s)).toBe(false)
    s = settle(s, facts(5000, { shown: [c('a', { askId: 'Q2' })] }))
    expect([s.askId, s.askAt, clicksThrough(s)]).toEqual(['Q2', 5000, true])
  })

  it('holds a pointer that was inside when the half second ran out, until it has left once', () => {
    const under = { presenting: saying('a', 'L1'), inBubble: true, pointer: { x: 500, y: 200 } }
    let s = settle(EMPTY, facts(0, under))
    expect([clicksThrough(s), holding(s), s.guard?.from]).toEqual([true, false, { x: 500, y: 200 }])
    s = settle(s, facts(THROUGH_MS, under))
    // Nothing under it is clicked any more, and its controls do not act yet.
    expect([clicksThrough(s), holding(s)]).toEqual([false, true])
    expect(s.wakeAt).toBe(HOLD_MS)
    s = settle(s, facts(20_000, under))
    expect([clicksThrough(s), holding(s), s.wakeAt]).toEqual([false, true, null])
    expect(settle(s, facts(20_000, under))).toBe(s)
    s = settle(s, facts(20_001, { presenting: saying('a', 'L1'), pointer: { x: 500, y: 400 } }))
    expect([clicksThrough(s), holding(s)]).toEqual([false, false])
  })

  it('lets a held pointer act once a second has passed and it moved on 16 px', () => {
    const at = (x: number) => ({ presenting: saying('a', 'L1'), inBubble: true, pointer: { x, y: 200 } })
    let s = run(EMPTY, [0, THROUGH_MS], at(500))
    expect(holding(s)).toBe(true)
    // Moved on before the second was up: held, and woken when it is.
    s = settle(s, facts(800, at(500 + HOLD_MOVE_PX)))
    expect([holding(s), s.wakeAt]).toEqual([true, HOLD_MS])
    s = settle(s, facts(HOLD_MS, at(500 + HOLD_MOVE_PX)))
    expect([holding(s), clicksThrough(s), s.wakeAt]).toEqual([false, false, null])
    // A pointer that did not move as far is held on.
    const still = settle(run(EMPTY, [0, THROUGH_MS], at(500)), facts(5000, at(500 + HOLD_MOVE_PX - 1)))
    expect(holding(still)).toBe(true)
  })

  it('holds again from the start when the bubble changes under a held pointer', () => {
    const under = (lineId: string) => ({ presenting: saying('a', lineId), inBubble: true, pointer: { x: 500, y: 200 } })
    let s = run(EMPTY, [0, THROUGH_MS, 3000], under('L1'))
    expect(holding(s)).toBe(true)
    s = settle(s, facts(3100, under('L2')))
    expect([clicksThrough(s), s.guard?.since]).toEqual([true, 3100])
    s = settle(s, facts(3100 + THROUGH_MS, under('L2')))
    expect(holding(s)).toBe(true)
  })

  it('takes taps after the half second where the last tap was: a lifted finger is nowhere', () => {
    // The tap that came before the bubble opened is over: the pointer is nowhere and not inside.
    let s = settle(EMPTY, facts(0, { presenting: saying('a', 'L1'), pointer: null }))
    s = settle(s, facts(THROUGH_MS, { presenting: saying('a', 'L1'), pointer: null }))
    expect([clicksThrough(s), holding(s), s.guard]).toEqual([false, false, null])
  })

  it('does not guard again when the line on screen is said once more: nothing in the bubble changed by itself', () => {
    const pointer = { x: 500, y: 200 }
    let s = run(EMPTY, [0, THROUGH_MS], { presenting: saying('a', 'L1', { silent: true }) })
    expect(s.guard).toBeNull()
    // The caption ended; the bubble stays to be read, and the user clicks Hear it in it.
    s = settle(s, facts(2000, { inBubble: true, pointer }))
    expect([s.bubble, s.presenting, s.lineId]).toEqual(['self', false, 'L1'])
    const again = { presenting: saying('a', 'L1'), inBubble: true, pointer }
    s = settle(s, facts(2100, again))
    expect([s.presenting, s.guard, clicksThrough(s)]).toEqual([true, null, false])
    s = settle(s, facts(2100 + THROUGH_MS, again))
    expect([clicksThrough(s), holding(s)]).toEqual([false, false])
  })

  it('does not guard a line the user asked to hear, and guards the next one the avatar says by itself', () => {
    const pointer = { x: 500, y: 200 }
    const asking = { shown: [c('a', { askId: 'Q1' })] }
    // A question that was open when the page loaded: its line was never presented here.
    let s = run(EMPTY, [0, THROUGH_MS], asking)
    expect([s.bubble, s.lineId, s.guard]).toEqual(['self', null, null])
    s = askedToHear(s, 'a', 'L1')
    expect(s.toHear).toBe('L1')
    expect(askedToHear(s, 'a', 'L1')).toBe(s)
    // Only the open bubble's avatar, and only an open bubble.
    expect(askedToHear(s, 'b', 'L1')).toBe(s)
    expect(askedToHear(EMPTY, 'a', 'L1')).toBe(EMPTY)
    s = settle(s, facts(3000, { ...asking, presenting: saying('a', 'L1'), inBubble: true, pointer }))
    expect([s.lineId, s.toHear, s.guard]).toEqual(['L1', null, null])
    s = settle(s, facts(3000 + THROUGH_MS, { ...asking, presenting: saying('a', 'L1'), inBubble: true, pointer }))
    expect(holding(s)).toBe(false)
    // The avatar's next line changes the bubble by itself.
    s = settle(s, facts(6000, { ...asking, inBubble: true, pointer }))
    s = settle(s, facts(7000, { ...asking, presenting: saying('a', 'L2'), inBubble: true, pointer }))
    expect([clicksThrough(s), s.guard?.since]).toEqual([true, 7000])
  })

  it('does not guard a bubble whose reply box the user is writing in', () => {
    const pointer = { x: 500, y: 200 }
    const asking = [c('a', { askId: 'Q1' })]
    let s = run(EMPTY, [0, THROUGH_MS], { shown: asking })
    expect(s.guard).toBeNull()
    // The user clicked into the reply box and types, the mouse resting on the bubble.
    const writing = { shown: [c('a', { askId: 'Q1', draft: true })], inBubble: true, pointer, focusInBubble: true, focusInReply: true }
    s = settle(s, facts(2000, writing))
    // A line of the avatar changes the bubble under it: no click goes through, and nothing is held or dimmed.
    s = settle(s, facts(3000, { ...writing, presenting: saying('a', 'L2') }))
    expect([s.lineId, s.guard, clicksThrough(s)]).toEqual(['L2', null, false])
    s = settle(s, facts(3000 + THROUGH_MS, { ...writing, presenting: saying('a', 'L2') }))
    expect([clicksThrough(s), holding(s)]).toEqual([false, false])
    // The same change with focus elsewhere is guarded.
    const elsewhere = { ...writing, focusInBubble: false, focusInReply: false }
    let other = settle(s, facts(6000, elsewhere))
    other = settle(other, facts(7000, { ...elsewhere, presenting: saying('a', 'L3') }))
    expect(clicksThrough(other)).toBe(true)
    // A guard that holds ends when focus goes into the reply box (by a key): the user is working in it.
    other = settle(other, facts(7000 + THROUGH_MS, { ...elsewhere, presenting: saying('a', 'L3') }))
    expect(holding(other)).toBe(true)
    other = settle(other, facts(7600, { ...writing, presenting: saying('a', 'L3') }))
    expect([holding(other), other.guard]).toEqual([false, null])
  })
})

describe('settle: what the user does', () => {
  it('opens the bubble of the avatar that was pressed, and it stays open', () => {
    let s = pressed(EMPTY, 'b', 100)
    expect([s.active.id, s.bubble, clicksThrough(s)]).toEqual(['b', 'user', false])
    s = run(s, [200, 900_000])
    expect([s.active.id, s.bubble, s.wakeAt]).toEqual(['b', 'user', null])
    // Not even a line of another avatar takes the stage from it.
    s = settle(s, facts(900_100, { presenting: saying('a', 'L1') }))
    expect([s.active.id, s.bubble]).toEqual(['b', 'user'])
  })

  it('closes it on the second press, and the figure shrinks six seconds later', () => {
    let s = settle(pressed(EMPTY, 'b', 100), facts(200))
    s = pressed(s, 'b', 5000)
    expect(s.bubble).toBe('closed')
    s = settle(s, facts(5000))
    expect(s.active).toEqual({ id: 'b', idleSince: 5000 })
    expect(settle(s, facts(5000 + SHRINK_AFTER_MS)).active.id).toBeNull()
  })

  it('opens another avatar’s bubble at once, whatever the first one was doing', () => {
    let s = run(EMPTY, [0, 600], { presenting: saying('a', 'L1'), overGroup: true })
    s = pressed(s, 'b', 700)
    s = settle(s, facts(700, { presenting: saying('a', 'L1'), overGroup: true }))
    expect([s.active.id, s.bubble, s.lineId]).toEqual(['b', 'user', null])
  })

  it('does not bring back by itself what the user closed: not for the same line, not for the same question', () => {
    const asking = { shown: [c('a', { askId: 'Q1' })], presenting: saying('a', 'L1') }
    let s = run(EMPTY, [0, 600], asking)
    s = closeByUser(s, 700)
    s = run(s, [700, 5000], asking)
    expect(s.bubble).toBe('closed')
    s = run(s, [6000, 60_000], { shown: asking.shown })
    expect([s.bubble, s.active.id]).toEqual(['closed', 'a'])
    // A new line brings it back, and so does another question.
    expect(settle(s, facts(61_000, { shown: asking.shown, presenting: saying('a', 'L2') })).bubble).toBe('self')
    expect(settle(s, facts(61_000, { shown: [c('a', { askId: 'Q2' })] })).bubble).toBe('self')
    expect(closeByUser(EMPTY, 5)).toBe(EMPTY)
  })

  it('lets another avatar take the stage from a draft kept behind a closed bubble', () => {
    const drafted = [c('a', { askId: 'Q1', draft: true }), c('b')]
    // A asks, the user types into its reply box, and the question is answered on another device.
    let s = run(EMPTY, [0, 600, 2000], { shown: drafted })
    expect([s.active.id, s.bubble]).toEqual(['a', 'self'])
    const kept = [c('a', { draft: true }), c('b')]
    s = run(s, [3000, 3000 + CLOSE_AFTER_MS, 60_000], { shown: kept })
    expect(s.bubble).toBe('closed')
    // B says a line and then asks: its bubble opens.
    s = run(s, [61_000, 61_600], { shown: kept, presenting: saying('b', 'LB') })
    expect([s.active.id, s.bubble, s.lineId]).toEqual(['b', 'self', 'LB'])
    s = run(s, [70_000, 70_600], { shown: [c('a', { draft: true }), c('b', { askId: 'Q2' })] })
    expect([s.active.id, s.bubble, s.askId]).toEqual(['b', 'self', 'Q2'])
    // The same after the user typed and closed the bubble with Escape.
    let t = run(EMPTY, [0, 600], { shown: [c('a', { askId: 'Q1' }), c('b')] })
    t = closeByUser(settle(t, facts(1000, { shown: [c('a', { askId: 'Q1', draft: true }), c('b')] })), 1500)
    t = run(t, [1600, 2000, 2600], { shown: [c('a', { askId: 'Q1', draft: true }), c('b')], presenting: saying('b', 'LB') })
    expect([t.active.id, t.bubble]).toEqual(['b', 'self'])
  })

  it('does not bring back by itself a question the user closed when its avatar takes the stage again', () => {
    const shown = [c('a', { askId: 'Q1' }), c('b')]
    let s = run(EMPTY, [0, 600], { shown })
    expect([s.active.id, s.bubble]).toEqual(['a', 'self'])
    s = closeByUser(s, 1000)
    // B says a line and takes the stage; its bubble closes by itself.
    s = run(s, [2000, 2100], { shown, presenting: saying('b', 'LB') })
    expect([s.active.id, s.bubble]).toEqual(['b', 'self'])
    s = run(s, [3000, 8000, 12_000, 20_000, 30_000], { shown })
    expect([s.active.id, s.bubble]).toEqual(['a', 'closed'])
    // A bubble the user closes on a line of B forgets nothing of A's question.
    let t = run(EMPTY, [0, 600], { shown })
    t = closeByUser(t, 1000)
    t = run(t, [2000, 2100], { shown, presenting: saying('b', 'LB') })
    t = closeByUser(t, 2500)
    t = run(t, [3000, 9000, 20_000], { shown })
    expect([t.active.id, t.bubble]).toEqual(['a', 'closed'])
    // Another question of A opens it.
    expect(settle(s, facts(31_000, { shown: [c('a', { askId: 'Q2' }), c('b')] })).bubble).toBe('self')
  })

  it('remembers what the user closed for each avatar: opening another one forgets nothing of it', () => {
    const shown = [c('a', { askId: 'Q1' }), c('b')]
    let s = run(EMPTY, [0, 600], { shown })
    s = closeByUser(s, 1000)
    // The user clicks B's figure to read its transcript, and closes it again.
    s = settle(openByUser(s, 'b'), facts(2000, { shown, overGroup: true }))
    expect([s.active.id, s.bubble]).toEqual(['b', 'user'])
    s = closeByUser(s, 3000)
    s = run(s, [3000, 9000, 20_000, 30_000], { shown })
    expect([s.active.id, s.bubble, s.askId]).toEqual(['a', 'closed', 'Q1'])
    // Opening A's bubble again undoes that close: once a block that closed it is over, Q1 opens by itself.
    s = settle(openByUser(s, 'a'), facts(31_000, { shown }))
    expect(s.bubble).toBe('user')
    s = settle(s, facts(31_100, { shown: [c('a', { askId: 'Q1', blocked: true }), c('b')] }))
    expect(s.bubble).toBe('closed')
    expect(settle(s, facts(31_200, { shown })).bubble).toBe('self')
  })

  it('remembers two questions the user closed, one of each avatar', () => {
    const both = [c('a', { askId: 'Q1' }), c('b', { askId: 'QB' })]
    let s = run(EMPTY, [0, 600], { shown: both, onScreen: 'a' })
    expect([s.active.id, s.bubble, s.askId]).toEqual(['a', 'self', 'Q1'])
    s = closeByUser(s, 1000)
    // B's question line takes the stage, and the user closes that bubble too.
    s = run(s, [2000, 2600], { shown: both, onScreen: 'a', presenting: saying('b', 'LB') })
    expect([s.active.id, s.bubble, s.askId]).toEqual(['b', 'self', 'QB'])
    s = closeByUser(s, 3000)
    // QB is answered on the phone, and A takes the stage again: Q1 stays closed.
    const after = [c('a', { askId: 'Q1' }), c('b')]
    s = run(s, [4000, 10_000, 20_000, 30_000], { shown: after, onScreen: 'a' })
    expect([s.active.id, s.bubble, s.askId]).toEqual(['a', 'closed', 'Q1'])
    // Nor does B's line, presented once more, open B's bubble by itself.
    s = run(s, [31_000, 31_600], { shown: after, onScreen: 'a', presenting: saying('b', 'LB') })
    expect([s.active.id, s.bubble]).toEqual(['b', 'closed'])
  })

  it('lets clicks through a bubble the user opened when a line changes it', () => {
    let s = settle(openByUser(EMPTY, 'a'), facts(100))
    expect(clicksThrough(s)).toBe(false)
    s = settle(s, facts(200, { presenting: saying('a', 'L3') }))
    expect([s.bubble, s.lineId, clicksThrough(s)]).toEqual(['user', 'L3', true])
    // It stays the user's: it does not close after the line.
    s = run(s, [5000, 600_000])
    expect(s.bubble).toBe('user')
  })
})

describe('settle: a session that needs the user', () => {
  it('closes the bubble when the session needs a permission or an answer, and does not reopen it by itself', () => {
    const asking = [c('a', { askId: 'Q1' })]
    let s = run(EMPTY, [0, 600], { shown: asking, presenting: saying('a', 'L1') })
    expect(s.bubble).toBe('self')
    const blocked = [c('a', { askId: 'Q1', blocked: true })]
    s = settle(s, facts(700, { shown: blocked, presenting: saying('a', 'L1') }))
    expect(s.bubble).toBe('closed')
    // Neither a waiting line that is presented silently meanwhile nor the question opens it.
    s = run(s, [800, 5000], { shown: blocked, presenting: saying('a', 'L2', { silent: true }) })
    expect(s.bubble).toBe('closed')
    // The user still can, and it stays.
    s = settle(openByUser(s, 'a'), facts(5100, { shown: blocked }))
    expect(s.bubble).toBe('user')
  })

  it('keeps a bubble the user opened for a blocked avatar that was not the active one, whichever way it was opened', () => {
    const shown = [c('a'), c('b', { blocked: true })]
    // A tap on the phone's face, the palette, the +n menu: nothing made it the active one first.
    let s = settle(openByUser(EMPTY, 'b', true), facts(100, { shown }))
    expect([s.active.id, s.bubble]).toEqual(['b', 'user'])
    s = run(s, [5000, 600_000], { shown })
    expect(s.bubble).toBe('user')
    expect(settle(pressed(EMPTY, 'b', 100, true), facts(100, { shown })).bubble).toBe('user')
    // While another avatar presents a line, a click on the blocked one opens its bubble, not the presenter's again.
    let t = run(EMPTY, [0, 600], { presenting: saying('a', 'L1'), shown })
    expect([t.active.id, t.bubble]).toEqual(['a', 'self'])
    t = settle(openByUser(t, 'b', true), facts(700, { presenting: saying('a', 'L1'), shown, overGroup: true }))
    expect([t.active.id, t.bubble]).toEqual(['b', 'user'])
    t = settle(t, facts(800, { presenting: saying('a', 'L1'), shown }))
    expect([t.active.id, t.bubble]).toEqual(['b', 'user'])
  })

  it('closes a bubble the user opened too when the block arrives', () => {
    let s = settle(openByUser(EMPTY, 'a'), facts(100))
    s = settle(s, facts(200, { shown: [c('a', { blocked: true })] }))
    expect(s.bubble).toBe('closed')
  })

  it('opens again for the question once the session no longer needs the user', () => {
    let s = run(EMPTY, [0, 600], { shown: [c('a', { askId: 'Q1' })] })
    s = settle(s, facts(700, { shown: [c('a', { askId: 'Q1', blocked: true })], hovered: 'a' }))
    expect([s.active.id, s.bubble]).toEqual(['a', 'closed'])
    s = settle(s, facts(900, { shown: [c('a', { askId: 'Q1' })] }))
    expect(s.bubble).toBe('self')
  })

  it('makes no avatar active for a line it presents while blocked', () => {
    const s = settle(EMPTY, facts(100, { shown: [c('a', { blocked: true })], presenting: saying('a', 'L1', { silent: true }) }))
    expect(s).toBe(EMPTY)
  })
})

describe('settle: another project’s avatar', () => {
  const guest = [c('a'), c('g', { askId: 'Q1', guest: true })]

  it('opens no bubble for a question that is only open: the guest rests', () => {
    const s = settle(EMPTY, facts(1000, { shown: guest }))
    expect(s).toBe(EMPTY)
  })

  it('shows its question while it is presented, and lets the bubble go afterwards although the question is still open', () => {
    let s = run(EMPTY, [0, 600], { shown: guest, presenting: saying('g', 'L1') })
    expect([s.active.id, s.bubble, s.askId]).toEqual(['g', 'self', 'Q1'])
    s = settle(s, facts(3000, { shown: guest }))
    expect(s.wakeAt).toBe(3000 + CLOSE_AFTER_MS)
    s = settle(s, facts(3000 + CLOSE_AFTER_MS, { shown: guest }))
    expect([s.bubble, s.active.id]).toEqual(['closed', 'g'])
    s = settle(s, facts(3000 + SHRINK_AFTER_MS, { shown: guest }))
    expect(s.active.id).toBeNull()
    // And it does not come back by itself.
    expect(settle(s, facts(900_000, { shown: guest }))).toBe(s)
  })

  it('opens with its question when the user turns to it, and stays', () => {
    let s = settle(openByUser(EMPTY, 'g'), facts(100, { shown: guest }))
    expect([s.active.id, s.bubble, s.askId]).toEqual(['g', 'user', 'Q1'])
    s = settle(s, facts(900_000, { shown: guest }))
    expect(s.bubble).toBe('user')
    expect(choicesReady(s, 100 + CHOICE_GUARD_MS)).toBe(true)
  })
})

describe('settle: a reply being dictated', () => {
  it('keeps the bubble of a guest’s question open while the user dictates its answer, and lets it go after', () => {
    const shown = [c('a'), c('g', { askId: 'Q1', guest: true })]
    const ended = run(EMPTY, [0, 600], { shown, presenting: saying('g', 'L1', { silent: true, chars: 30 }) })
    let s = run(ended, [1000, 60_000], { shown, listening: true })
    expect([s.active.id, s.bubble]).toEqual(['g', 'self'])
    s = settle(s, facts(61_000, { shown }))
    expect(s.wakeAt).toBe(61_000 + CLOSE_AFTER_MS)
    expect(settle(s, facts(61_000 + CLOSE_AFTER_MS, { shown })).bubble).toBe('closed')
    // Without a dictation it closes once the line was read.
    expect(run(ended, [1000, 1000 + readingMs(30) + CLOSE_AFTER_MS], { shown }).bubble).toBe('closed')
  })

  it('keeps the stage while the user dictates, also before the first words came, when another avatar presents a line', () => {
    // The phone: A's own question in its panel. A tap on the microphone leaves focus where it was, and the finger is lifted.
    const shown = [c('a', { askId: 'Q1' }), c('b')]
    const asked = run(EMPTY, [0, 600], { shown })
    expect([asked.active.id, asked.bubble]).toEqual(['a', 'self'])
    const s = run(asked, [1000, 1600], { shown, listening: true, presenting: saying('b', 'LB', { silent: true }) })
    expect([s.active.id, s.bubble, s.askId]).toEqual(['a', 'self', 'Q1'])
    expect(settle(s, facts(1700, { shown, listening: true, presenting: saying('b', 'LB', { silent: true }) }))).toBe(s)
    // Without a dictation the line takes the stage at once.
    const t = run(asked, [1000, 1600], { shown, presenting: saying('b', 'LB', { silent: true }) })
    expect([t.active.id, t.bubble, t.lineId]).toEqual(['b', 'self', 'LB'])
  })
})

describe('the reply box', () => {
  it('is on screen with a question that takes typed answers, and in the transcript the user opened', () => {
    const self = run(EMPTY, [0, 600], { presenting: saying('a', 'L1') })
    expect([self.active.id, self.bubble]).toEqual(['a', 'self'])
    expect(replyShown(self, 'a', null)).toBe(false)
    expect(replyShown(self, 'a', ask('Q1'))).toBe(true)
    expect(replyShown(self, 'a', ask('Q1', { freeText: false }))).toBe(false)
    // Only the active avatar's bubble is on screen.
    expect(replyShown(self, 'b', ask('Q1'))).toBe(false)
    const user = openByUser(EMPTY, 'a')
    expect(replyShown(user, 'a', null)).toBe(true)
    expect(replyShown(user, 'a', ask('Q1'))).toBe(true)
    expect(replyShown(user, 'a', ask('Q1', { freeText: false }))).toBe(false)
    expect(replyShown(closeByUser(user, 5), 'a', ask('Q1'))).toBe(false)
    expect(replyShown(EMPTY, 'a', ask('Q1'))).toBe(false)
  })

  it('lets another avatar’s line take the stage from a draft kept in a bubble that has no reply box', () => {
    // The candidates as the overlay gives them: a kept draft counts only while its reply box is on screen.
    let asking: Ask | null = ask('Q1')
    const shownIn = (s: Scene): Candidate[] => [c('a', { askId: asking?.id ?? null, draft: replyShown(s, 'a', asking) }), c('b')]
    const steps = (scene: Scene, times: number[], over: Partial<Facts> = {}) => {
      let s = scene
      for (const t of times) s = settle(s, facts(t, { ...over, shown: shownIn(s) }))
      return s
    }
    // A asks Q1, the user types into its reply box, Q1 is answered on the phone and the bubble closes: the draft is kept.
    let s = steps(EMPTY, [0, 600, 2000])
    expect([s.active.id, s.bubble, shownIn(s)[0].draft]).toEqual(['a', 'self', true])
    asking = null
    s = steps(s, [3000, 3000 + CLOSE_AFTER_MS, 60_000])
    expect(s.bubble).toBe('closed')
    // A says L2 silently: its bubble opens by itself with the caption, and no reply box.
    s = steps(s, [61_000, 61_600], { presenting: saying('a', 'L2', { silent: true }) })
    expect([s.active.id, s.bubble, s.lineId, shownIn(s)[0].draft]).toEqual(['a', 'self', 'L2', false])
    // L2 ends; while it is still being read the speaker presents B's line silently, and B's caption is shown.
    s = steps(s, [64_000])
    expect(s.bubble).toBe('self')
    s = steps(s, [64_100, 64_700], { presenting: saying('b', 'LB', { silent: true }) })
    expect([s.active.id, s.bubble, s.lineId]).toEqual(['b', 'self', 'LB'])
  })
})

describe('settle: the compact form with a permission request on screen', () => {
  it('closes the panel when the request arrives, and does not open it by itself while it waits', () => {
    const asking = [c('a', { askId: 'Q1' })]
    let s = run(EMPTY, [0, 600], { shown: asking })
    s = settle(s, facts(700, { shown: asking, held: true }))
    expect([s.bubble, s.held]).toEqual(['closed', true])
    s = run(s, [800, 9000], { shown: asking, held: true, presenting: saying('a', 'L5') })
    expect(s.bubble).toBe('closed')
    // A tap opens it all the same.
    s = settle(openByUser(s, 'a'), facts(9100, { shown: asking, held: true }))
    expect(s.bubble).toBe('user')
    // And a new request closes it again.
    s = settle(s, facts(9200, { shown: asking }))
    s = settle(s, facts(9300, { shown: asking, held: true }))
    expect(s.bubble).toBe('closed')
  })

  it('opens for the question that is still open once the request was answered', () => {
    const asking = [c('a', { askId: 'Q1' })]
    let s = settle(EMPTY, facts(0, { shown: asking, held: true }))
    expect([s.active.id, s.bubble]).toEqual(['a', 'closed'])
    s = settle(s, facts(1000, { shown: asking }))
    expect(s.bubble).toBe('self')
  })
})

describe('settle: it comes to rest', () => {
  /** A small generator of facts, the same on every run. */
  function walk(seed: number) {
    let x = seed
    const next = () => {
      x = (Math.imul(x, 1664525) + 1013904223) >>> 0
      return x / 0x100000000
    }
    const pick = <T,>(items: readonly T[]): T => items[Math.floor(next() * items.length)]
    return { next, pick }
  }

  it('gives the same scene back when it is asked again with the same facts, and never asks to be woken in the past', () => {
    for (let seed = 1; seed <= 40; seed++) {
      const { next, pick } = walk(seed)
      let scene: Scene = EMPTY
      let now = 1000
      for (let step = 0; step < 120; step++) {
        now += pick([1, 30, 400, 2500, 7000])
        const ids = ['a', 'b', 'c']
        const shown = ids.filter(() => next() < 0.85).map((id) => c(id, { blocked: next() < 0.15, askId: next() < 0.3 ? pick(['Q1', 'Q2']) : null, draft: next() < 0.15, guest: next() < 0.2 }))
        const f = facts(now, {
          shown,
          onScreen: pick([null, 'a', 'b']),
          presenting: next() < 0.4 ? saying(pick(ids), pick(['L1', 'L2', 'L3']), { silent: next() < 0.5, chars: Math.floor(next() * 200) }) : null,
          hovered: next() < 0.2 ? pick(ids) : null,
          focused: next() < 0.15 ? pick(ids) : null,
          overGroup: next() < 0.2,
          inBubble: next() < 0.25,
          // Mostly resting, now and then moved a little or far, or nowhere.
          pointer: next() < 0.15 ? null : { x: pick([100, 100, 100, 108, 130]), y: pick([200, 200, 213]) },
          focusInBubble: next() < 0.1,
          listening: next() < 0.1,
          held: next() < 0.15,
        })
        // Now and then the user writes in the reply box.
        if (next() < 0.06) Object.assign(f, { focusInBubble: true, focusInReply: true })
        // The user does something now and then.
        const what = next()
        if (what < 0.08) scene = pressed(scene, pick(ids), now)
        else if (what < 0.12) scene = closeByUser(scene, now)
        else if (what < 0.16) scene = askedToHear(scene, pick(ids), pick(['L1', 'L2', 'L3']))
        const settled = settle(scene, f)
        expect(settle(settled, f), `seed ${seed} step ${step}`).toBe(settled)
        expect(settled.wakeAt === null || settled.wakeAt > now, `seed ${seed} step ${step}: wakes at ${settled.wakeAt}, now ${now}`).toBe(true)
        // A moment later, before anything is due, nothing has changed either.
        if (settled.wakeAt === null || settled.wakeAt > now + 1) expect(settle(settled, { ...f, now: now + 1 }), `seed ${seed} step ${step} a moment later`).toBe(settled)
        // What is on stage is consistent: a bubble belongs to an avatar that is shown.
        if (settled.bubble !== 'closed') expect(shown.some((s) => s.id === settled.active.id), `seed ${seed} step ${step}`).toBe(true)
        scene = settled
      }
    }
  })

  it('does not keep changing while the pointer rests in a bubble whose question was just answered', () => {
    const asking = { shown: [c('a', { askId: 'Q1' })], inBubble: true }
    let s = run(EMPTY, [0, 600, 3000], asking)
    s = settle(s, facts(3100, { shown: [c('a')], inBubble: true }))
    expect(s.bubble).toBe('self')
    expect(settle(s, facts(3100, { shown: [c('a')], inBubble: true }))).toBe(s)
    expect(settle(s, facts(3200, { shown: [c('a')], inBubble: true }))).toBe(s)
    expect(settle(s, facts(900_000, { shown: [c('a')], inBubble: true }))).toBe(s)
  })
})

describe('the choices', () => {
  it('take no click for 600 ms after the question appeared or changed', () => {
    const q = ask('Q1')
    const s = settle(EMPTY, facts(1000, { shown: [c('a', { askId: 'Q1' })] }))
    expect(choicesReady(s, 1000 + CHOICE_GUARD_MS - 1)).toBe(false)
    expect(planChoice(q, 0, s, 1000 + CHOICE_GUARD_MS - 1)).toBeNull()
    expect(planChoice(q, 0, s, 1000 + CHOICE_GUARD_MS)).toEqual({ askId: 'Q1', choice: 0, via: 'click' })
    expect(planChoice(q, 1, s, 99_000)).toEqual({ askId: 'Q1', choice: 1, via: 'click' })
  })

  it('send the question that was on screen when the button was pressed, and no choice that is none', () => {
    const s = settle(EMPTY, facts(1000, { shown: [c('a', { askId: 'Q2' })] }))
    // The button belongs to Q1, which the bubble showed before the scene took note of Q2.
    expect(planChoice(ask('Q1'), 0, s, 5000)?.askId).toBe('Q1')
    for (const index of [-1, 2, 1.5, Number.NaN]) expect(planChoice(ask('Q1'), index, s, 5000)).toBeNull()
    expect(planChoice(null, 0, s, 5000)).toBeNull()
  })
})

describe('the reply draft', () => {
  it('is for the question on screen when the user began, and stays so while the user goes on', () => {
    const begun = edited(undefined, 'Y', 'Q1', false)
    expect(begun).toEqual({ text: 'Y', askId: 'Q1', voice: false })
    expect(edited(begun, 'Ye', 'Q2', false)).toEqual({ text: 'Ye', askId: 'Q1', voice: false })
    expect(edited(undefined, 'hello', null, false).askId).toBeNull()
  })

  it('is for the question on screen when the microphone was pressed, not when the first words arrived', () => {
    // Q1 was on screen at the tap; Q2 replaced it before the recognizer gave its first words.
    const spoken = edited(undefined, 'yes', 'Q2', true, 'Q1')
    expect(spoken).toEqual({ text: 'yes', askId: 'Q1', voice: true })
    // So Enter only takes note of the change, as for a typed draft.
    expect(planSend(spoken, ask('Q2'))).toEqual({ kind: 'confirm', draft: { text: 'yes', askId: 'Q2', voice: true } })
    // A message begun with no question on screen does not answer one that opened while the user spoke.
    expect(planSend(edited(undefined, 'how far are you', 'Q3', true, null), ask('Q3')).kind).toBe('confirm')
    // A draft that goes on stays what it was for, and typing knows no tap.
    expect(edited({ text: 'Yes', askId: 'Q1', voice: false }, 'Yes please', 'Q2', true, 'Q2').askId).toBe('Q1')
    expect(edited(undefined, 'y', 'Q2', false).askId).toBe('Q2')
  })

  it('starts over once it is empty', () => {
    const begun = edited(undefined, 'Y', 'Q1', true)
    expect(edited(begun, '', 'Q2', false)).toBe(NO_DRAFT)
    expect(edited(edited(begun, '', 'Q2', false), 'n', 'Q2', false)).toEqual({ text: 'n', askId: 'Q2', voice: false })
  })

  it('remembers that some of it was dictated, also when the user types on', () => {
    const spoken = edited(undefined, 'yes push it', 'Q1', true)
    expect(spoken.voice).toBe(true)
    expect(edited(spoken, 'yes, push it', 'Q1', false).voice).toBe(true)
    expect(edited(edited(undefined, 'a', null, false), 'a b', null, true).voice).toBe(true)
  })

  it('never holds more than the server takes', () => {
    expect(edited(undefined, 'x'.repeat(MAX_DRAFT_CHARS + 50), null, false).text).toHaveLength(MAX_DRAFT_CHARS)
  })

  it('says that the question changed under it, and needs another Enter then', () => {
    const draft = edited(undefined, 'Not yet', 'Q1', false)
    expect(askChanged(draft, 'Q1')).toBe(false)
    expect(askChanged(draft, 'Q2')).toBe(true)
    expect(askChanged(draft, null)).toBe(true)
    expect(askChanged(NO_DRAFT, 'Q2')).toBe(false)
    expect(askChanged(undefined, 'Q2')).toBe(false)
    // The first Enter only takes note: the draft is now for what is on screen.
    const first = planSend(draft, ask('Q2'))
    expect(first).toEqual({ kind: 'confirm', draft: { text: 'Not yet', askId: 'Q2', voice: false } })
    // The second sends it, to the question on screen now.
    expect(planSend((first as { draft: typeof draft }).draft, ask('Q2'))).toEqual({ kind: 'send', body: { askId: 'Q2', choice: 1, via: 'typed' } })
    // The question is gone: after the note it is a message.
    const gone = planSend(draft, null)
    expect(gone).toEqual({ kind: 'confirm', draft: { text: 'Not yet', askId: null, voice: false } })
    expect(planSend((gone as { draft: typeof draft }).draft, null)).toEqual({ kind: 'send', body: { text: 'Not yet', via: 'typed' } })
  })

  it('sends typed text as the answer to the question, and as a message without one', () => {
    expect(planSend(edited(undefined, '  Use the staging branch ', 'Q1', false), ask('Q1'))).toEqual({ kind: 'send', body: { askId: 'Q1', text: 'Use the staging branch', via: 'typed' } })
    expect(planSend(edited(undefined, 'How far are you?', null, false), null)).toEqual({ kind: 'send', body: { text: 'How far are you?', via: 'typed' } })
  })

  it('sends an answer that names a choice as that choice, typed or dictated', () => {
    expect(planSend(edited(undefined, 'yes push it', 'Q1', true), ask('Q1'))).toEqual({ kind: 'send', body: { askId: 'Q1', choice: 0, via: 'voice' } })
    expect(planSend(edited(undefined, 'NOT YET.', 'Q1', false), ask('Q1'))).toEqual({ kind: 'send', body: { askId: 'Q1', choice: 1, via: 'typed' } })
    expect(planSend(edited(undefined, 'maybe later', 'Q1', true), ask('Q1'))).toEqual({ kind: 'send', body: { askId: 'Q1', text: 'maybe later', via: 'voice' } })
  })

  it('sends nothing when there is nothing, and no free text to a question that takes choices only', () => {
    expect(planSend(undefined, null)).toEqual({ kind: 'nothing' })
    expect(planSend(NO_DRAFT, ask('Q1'))).toEqual({ kind: 'nothing' })
    expect(planSend({ text: '   ', askId: 'Q1', voice: false }, ask('Q1'))).toEqual({ kind: 'nothing' })
    const strict = ask('Q1', { freeText: false })
    expect(planSend(edited(undefined, 'something else', 'Q1', false), strict)).toEqual({ kind: 'nothing' })
    expect(planSend(edited(undefined, 'not yet', 'Q1', false), strict)).toEqual({ kind: 'send', body: { askId: 'Q1', choice: 1, via: 'typed' } })
  })

  it('says "Sent." or that the session reads it at its next step', () => {
    expect(sentNote('prompt', false)).toEqual({ kind: 'sent' })
    expect(sentNote('prompt', true)).toEqual({ kind: 'sentLater' })
    expect(sentNote('answer', true)).toEqual({ kind: 'sent' })
    expect(sentNote('answer', false)).toEqual({ kind: 'sent' })
  })

  it('says where a reply a dialog left in the session\'s input box is, and what the user does now', () => {
    const note = { kind: 'info', text: "It is in the session's input box: answer its dialog, then press Enter there." }
    expect(IN_BOX).toBe(note.text)
    expect(sentNote('inBox', false)).toEqual(note)
    expect(sentNote('inBox', true)).toEqual(note)
  })

  it('offers the first voice choice before voice was chosen, and the form Hear it led to', () => {
    const form = (over: Partial<Parameters<typeof voiceForm>[0]> = {}) => voiceForm({ voice: null, online: false, can: 'device', listed: true, spoke: true, ...over })
    expect(form()).toBe('offer')
    expect(form({ asked: 'online' })).toBe('online')
    expect(form({ asked: 'none', can: 'online' })).toBe('none')
    expect(form({ asked: null })).toBe('offer')
    // Only under something the avatar said.
    expect(form({ spoke: false })).toBeNull()
  })

  it('says why a line stays a caption while voice is on but nothing may say it', () => {
    const on = (over: Partial<Parameters<typeof voiceForm>[0]> = {}) => voiceForm({ voice: true, online: false, can: 'device', listed: true, spoke: true, ...over })
    // Only online voices for the line's language, and they are not allowed: the form that allows them (or Captions only).
    expect(on({ can: 'online' })).toBe('online')
    // No voice for it at all.
    expect(on({ can: 'none' })).toBe('none')
    expect(on({ can: 'none', online: true })).toBe('none')
    // A voice it may use: nothing to say.
    expect(on()).toBeNull()
    expect(on({ can: 'online', online: true })).toBeNull()
    // What Hear it led to before is the first choice's: it does not stay once voice is on.
    expect(on({ asked: 'online', can: 'device' })).toBeNull()
    expect(on({ can: 'online', spoke: false })).toBeNull()
    // Voice off ("Captions only" turned it off): no form, whatever the browser has.
    for (const can of ['device', 'online', 'none'] as const) expect(voiceForm({ voice: false, online: false, can, listed: true, spoke: true })).toBeNull()
  })

  it('says that this browser has no voices only once the browser has listed them', () => {
    const on = (over: Partial<Parameters<typeof voiceForm>[0]> = {}) => voiceForm({ voice: true, online: false, can: 'none', listed: true, spoke: true, ...over })
    // Browsers list their voices some time after the page loaded: an empty list says nothing until then.
    expect(on({ listed: false })).toBeNull()
    expect(on({ listed: false, online: true })).toBeNull()
    expect(on()).toBe('none')
    // What Hear it found out is a click's, and stays as it was.
    expect(voiceForm({ voice: null, online: false, can: 'none', listed: false, asked: 'none', spoke: true })).toBe('none')
  })

  it('is disabled while the session shows a dialog or is starting, unless a call of the session waits for the answer', () => {
    expect(whyDisabled({ composeBlocked: false, starting: false, waiting: false })).toBeNull()
    expect(whyDisabled({ composeBlocked: true, starting: false, waiting: false })).toBe('dialog')
    expect(whyDisabled({ composeBlocked: true, starting: true, waiting: false })).toBe('starting')
    expect(whyDisabled({ composeBlocked: true, starting: false, waiting: true })).toBeNull()
    expect(whyDisabled({ composeBlocked: true, starting: true, waiting: true })).toBeNull()
  })
})

describe('the notes about a reply', () => {
  const sent = noted({ kind: 'sent' as const }, 1000, 'L1')
  const said = (lineId: string, since: number) => ({ lineId, since })

  it('keep "Sent." for four seconds at the latest, the same object until then', () => {
    expect(sent).toEqual({ note: { kind: 'sent' }, at: 1000, since: 1000, line: 'L1' })
    expect(noteStep(sent, 1000 + NOTE_MS - 1, said('L1', 500))).toBe(sent)
    expect(noteWake(sent)).toBe(1000 + NOTE_MS)
    expect(noteStep(sent, 1000 + NOTE_MS, said('L1', 500))).toBeNull()
    for (const kind of ['sentLater', 'changed'] as const) {
      const n = noted({ kind }, 1000, null)
      expect(noteStep(n, 1000 + NOTE_MS - 1, null)).toBe(n)
      expect(noteStep(n, 1000 + NOTE_MS, null)).toBeNull()
    }
  })

  it('take every note away when another line of the avatar is presented', () => {
    expect(noteStep(sent, 1200, said('L2', 1100))).toBeNull()
    // And it stays away when that line has ended: the last line is still the other one.
    const error = noted({ kind: 'error' as const, text: 'A dialog is open in the session.' }, 1000, 'L1')
    expect(noteStep(error, 1200, said('L2', 1100))).toBeNull()
    expect(noteStep(error, 900_000, said('L2', 1100))).toBeNull()
    // Its own line said again (Hear it) is no other line.
    expect(noteStep(sent, 1200, said('L1', 1100))).toBe(sent)
    // A note given while no line was presented goes with the first one that is.
    expect(noteStep(noted({ kind: 'sent' as const }, 1000, null), 1100, said('L1', 1050))).toBeNull()
    expect(noteStep(null, 1, null)).toBeNull()
    expect(noteStep(undefined, 1, said('L1', 0))).toBeNull()
  })

  it('count a line that began while the reply was on its way, and none that began before', () => {
    // Sent at 1000 with L1 on screen; the answer came back at 3000.
    const late = noted({ kind: 'sent' as const }, 3000, 'L1', 1000)
    expect(late).toEqual({ note: { kind: 'sent' }, at: 3000, since: 1000, line: 'L1' })
    // The avatar said L2 at 2000, before "Sent." could appear: it is not shown under it.
    expect(noteStep(late, 3000, said('L2', 2000))).toBeNull()
    // A line that began before the reply was sent is what the reply answered.
    expect(noteStep(late, 3000, said('L0', 900))).toBe(late)
    // The time limit runs from when the note appeared.
    expect(noteStep(late, 3000 + NOTE_MS - 1, said('L1', 500))).toBe(late)
    expect(noteWake(late)).toBe(3000 + NOTE_MS)
    // Never about something that begins later than the note.
    expect(noted({ kind: 'sent' as const }, 1000, null, 5000).since).toBe(1000)
  })

  it('show a refusal that came back while the avatar said another line, until the line after it', () => {
    // Sent at 1000 with L1 on screen; the avatar began L2 at 1200; the server's answer came back at 1900.
    const sentAt = { line: 'L1', since: 1000 }
    const meanwhile = said('L2', 1200)
    const refused = replyNoted({ kind: 'error' as const, text: 'A dialog is open in the session.' }, 1900, sentAt, meanwhile)
    expect(noteStep(refused, 1900, meanwhile)).toBe(refused)
    // It waits for the user, as an error does, and the next line takes it away.
    expect(noteStep(refused, 900_000, meanwhile)).toBe(refused)
    expect(noteWake(refused)).toBeNull()
    expect(noteStep(refused, 5000, said('L3', 4000))).toBeNull()
    // "Sent." is about the reply: the line said while it was on its way came after it.
    const ok = replyNoted({ kind: 'sent' as const }, 1900, sentAt, meanwhile)
    expect(ok).toEqual({ note: { kind: 'sent' }, at: 1900, since: 1000, line: 'L1' })
    expect(noteStep(ok, 1900, meanwhile)).toBeNull()
    expect(noteStep(replyNoted({ kind: 'sentLater' as const }, 1900, sentAt, meanwhile), 1900, meanwhile)).toBeNull()
    // With no line in between, both are shown; an error with no line presented yet goes with the first one.
    const before = said('L1', 500)
    expect(noteStep(replyNoted({ kind: 'sent' as const }, 1900, sentAt, before), 1900, before)?.note.kind).toBe('sent')
    expect(noteStep(replyNoted({ kind: 'error' as const, text: 'x' }, 1900, sentAt, before), 1900, before)?.note.kind).toBe('error')
    const first = replyNoted({ kind: 'error' as const, text: 'x' }, 1900, { line: null, since: 1000 }, null)
    expect(first.line).toBeNull()
    expect(noteStep(first, 3000, said('L1', 2500))).toBeNull()
  })

  it('show where a reply a dialog left in the input box is, also after a line said while it went in', () => {
    // Sent at 1000 with L1 on screen; the avatar began L2 at 1200; "inBox" came back at 1900.
    const sentAt = { line: 'L1', since: 1000 }
    const meanwhile = said('L2', 1200)
    const inBox = replyNoted(sentNote('inBox', false), 1900, sentAt, meanwhile)
    expect(inBox).toEqual({ note: { kind: 'info', text: IN_BOX }, at: 1900, since: 1900, line: 'L2' })
    // The user has something to do: it waits for them, and goes with the next line or an edit.
    expect(noteStep(inBox, 900_000, meanwhile)).toBe(inBox)
    expect(noteWake(inBox)).toBeNull()
    expect(noteStep(inBox, 5000, said('L3', 4000))).toBeNull()
    expect(noteEdited(inBox)).toBeNull()
  })

  it('keep "The question changed." while the new question is said, and let the next line take it away', () => {
    const changed = noted({ kind: 'changed' as const }, 1000, 'L5')
    expect(noteStep(changed, 1100, said('L5', 1050))).toBe(changed)
    expect(noteStep(changed, 1100, said('L4', 900))).toBe(changed)
    expect(noteStep(changed, 2000, said('L6', 1900))).toBeNull()
  })

  it('keep an error or an info until the user does something, without a time of their own', () => {
    const info = noted({ kind: 'info' as const, text: 'No speech heard.' }, 1000, null)
    expect(noteStep(info, 900_000, null)).toBe(info)
    expect(noteStep(info, 900_000, said('L1', 500))).toBe(info)
    expect(noteWake(info)).toBeNull()
    // Typing or dictating takes it away; it takes nothing away from "Sent.".
    expect(noteEdited(info)).toBeNull()
    expect(noteEdited(noted({ kind: 'error' as const, text: 'x' }, 1, null))).toBeNull()
    expect(noteEdited(sent)).toBe(sent)
    expect(noteEdited(noted({ kind: 'changed' as const }, 1, null))?.note.kind).toBe('changed')
    expect(noteEdited(null)).toBeNull()
    expect(noteWake(null)).toBeNull()
  })

  it('know when the question on screen changed under a draft', () => {
    const draft = edited(undefined, 'Not yet', 'Q1', false)
    expect(questionChanged(draft, 'Q1', 'Q2')).toBe(true)
    // The question went away: the draft is a message now, and the bubble says so.
    expect(questionChanged(draft, 'Q1', null)).toBe(true)
    // Nothing changed, nothing was known before, or no draft to speak of.
    expect(questionChanged(draft, 'Q1', 'Q1')).toBe(false)
    expect(questionChanged(draft, undefined, 'Q2')).toBe(false)
    expect(questionChanged(NO_DRAFT, 'Q1', 'Q2')).toBe(false)
    expect(questionChanged(undefined, 'Q1', 'Q2')).toBe(false)
    expect(questionChanged({ text: '  ', askId: 'Q1', voice: false }, 'Q1', 'Q2')).toBe(false)
    // A draft begun for the question that comes back is no change.
    expect(questionChanged(draft, 'Q2', 'Q1')).toBe(false)
  })
})

describe('constants of the contract', () => {
  it('are what 10.1 and the decisions say', () => {
    expect([CLOSE_AFTER_MS, THROUGH_MS, CHOICE_GUARD_MS, SHRINK_AFTER_MS]).toEqual([4000, 500, 600, 6000])
    expect(HOVER_DWELL_MS).toBeLessThan(THROUGH_MS)
    // D11 and D13.
    expect([HOLD_MS, HOLD_MOVE_PX, NOTE_MS]).toEqual([1000, 16, 4000])
    expect(HOLD_MS).toBeGreaterThan(THROUGH_MS)
  })
})
