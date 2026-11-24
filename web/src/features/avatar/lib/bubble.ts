// The bubble of the active avatar: when it opens and closes by itself, how it guards
// against a click that was meant for something else, the notes it shows about a reply,
// and what a reply the user is writing answers.
//
// One bubble is on screen at a time: the active avatar's. It opens by itself for a line
// that is presented here and for an open question, and closes four seconds after the
// last reason to stay ended (a line presented silently is given the time to read it
// first). The user opens and closes it with a click, and what the user closed does not
// come back for the same line or the same question.
//
// `settle` is the whole timeline as one function: the scene as it was, the facts as they
// are now and the clock come in, the scene as it is now comes out, with the time at
// which it has to be looked at again. It never looks at a clock itself.

import { matchChoice } from './speech'
import { NOBODY, SHRINK_AFTER_MS, pickActive } from './stage'
import type { Active, Candidate } from './stage'
import type { Ask } from './types'

/** A bubble that opened by itself closes this long after its last reason to stay. */
export const CLOSE_AFTER_MS = 4000
/** A line presented silently stays this long first: two seconds and one more for every fifteen characters. */
export const READ_BASE_MS = 2000
export const READ_CHARS_PER_SECOND = 15
/** A bubble that opened or changed by itself lets clicks through for this long. */
export const THROUGH_MS = 500
/**
 * A pointer that rested inside a bubble when `THROUGH_MS` ran out lets its controls act
 * once it has left the bubble, or once this long has passed since the bubble opened or
 * changed and the pointer is `HOLD_MOVE_PX` or more away from where it was then.
 */
export const HOLD_MS = 1000
export const HOLD_MOVE_PX = 16
/** A question's choices take no click this long after it appeared or changed. */
export const CHOICE_GUARD_MS = 600
/** The pointer has to rest on a figure this long before it counts as hovered: passing over it does nothing. */
export const HOVER_DWELL_MS = 150
/** The server takes a reply of at most this many characters (`MAX_REPLY_CHARS`). */
export const MAX_DRAFT_CHARS = 4000

/** How long a line of `chars` characters stays to be read after it was presented silently. */
export function readingMs(chars: number): number {
  const n = typeof chars === 'number' && Number.isFinite(chars) && chars > 0 ? chars : 0
  return READ_BASE_MS + (n / READ_CHARS_PER_SECOND) * 1000
}

// ---------------------------------------------------------------- the guard against stray clicks

/** A place in viewport pixels. */
export interface Point {
  x: number
  y: number
}

/**
 * A bubble that opened or changed by itself guards against a click that was on its way to
 * something else. `timed`: for `THROUGH_MS` clicks go through it. `hold`: the pointer was
 * inside the bubble's rectangle when that time ran out, and has been since: the bubble
 * takes the pointer (nothing under it is clicked) but its controls do not act on it. That
 * ends when the pointer leaves the rectangle, or when `HOLD_MS` have passed since the bubble
 * opened or changed and the pointer is `HOLD_MOVE_PX` or more away from `from`. No guard
 * (null): the bubble and its controls take the pointer. Keys are never guarded, and neither
 * is a bubble whose reply box the user is writing in.
 */
export interface Guard {
  since: number
  phase: 'timed' | 'hold'
  /** Where the pointer was when the bubble opened or changed, or where it was first seen after that; null: not seen yet. */
  from: Point | null
}

const pointOf = (at: Point | null | undefined): Point | null => (at && Number.isFinite(at.x) && Number.isFinite(at.y) ? { x: at.x, y: at.y } : null)

export const armGuard = (now: number, at: Point | null = null): Guard => ({ since: now, phase: 'timed', from: pointOf(at) })

/** Whether the pointer is far enough from where it was when the guard was armed to show that it is being moved on purpose. */
const movedOn = (guard: Guard, at: Point | null): boolean => !!guard.from && !!at && Math.hypot(at.x - guard.from.x, at.y - guard.from.y) >= HOLD_MOVE_PX

/**
 * The guard at `now`. `inside`: the pointer is inside the bubble's rectangle; `at`: where it
 * is (null: nowhere, which a touch or a pen is once it has been lifted). The same object
 * while nothing changes.
 */
export function guardStep(guard: Guard | null, now: number, inside: boolean, at: Point | null = null): Guard | null {
  if (!guard) return null
  let g = guard
  const seen = g.from === null ? pointOf(at) : null
  if (seen) g = { ...g, from: seen }
  if (g.phase === 'timed') {
    if (now < g.since + THROUGH_MS) return g
    // A pointer that was not inside when the time ran out: the bubble takes it at once.
    if (!inside) return null
    g = { ...g, phase: 'hold' }
  }
  if (!inside) return null
  if (now >= g.since + HOLD_MS && movedOn(g, pointOf(at))) return null
  return g
}

/** When a guard that `guardStep` gave at `now` has to be looked at again by itself, or null. */
export function guardWake(guard: Guard | null, now: number): number | null {
  if (!guard) return null
  if (guard.phase === 'timed') return guard.since + THROUGH_MS
  // A pointer that moved on before the second was up lets go when it is.
  return now < guard.since + HOLD_MS ? guard.since + HOLD_MS : null
}

const sameGuard = (a: Guard | null, b: Guard | null): boolean => a === b || (!!a && !!b && a.since === b.since && a.phase === b.phase && a.from?.x === b.from?.x && a.from?.y === b.from?.y)

// ---------------------------------------------------------------- the scene

/** The line this tab presents now. */
export interface Presenting {
  terminalId: string
  lineId: string
  /** As a caption only (not aloud). */
  silent: boolean
  /** The length of the line. */
  chars: number
}

export interface Facts {
  now: number
  /** The avatars on stage, in their order. */
  shown: readonly Candidate[]
  /** The session on screen. */
  onScreen: string | null
  presenting: Presenting | null
  /** A figure the pointer rests on. */
  hovered: string | null
  /** The avatar that holds focus. */
  focused: string | null
  /** The pointer is over the group of figures. */
  overGroup: boolean
  /** The pointer is inside the bubble's rectangle (whether or not the bubble takes it). */
  inBubble: boolean
  /** Where the pointer is; null: nowhere in this window (a touch or a pen that was lifted is nowhere). */
  pointer: Point | null
  /** Focus is inside the bubble. */
  focusInBubble: boolean
  /** Focus is in the bubble's reply box: the user is writing in it, and a click there is no stray one. */
  focusInReply: boolean
  /** The microphone of the bubble's reply box listens: the user dictates, wherever the pointer and focus are. */
  listening: boolean
  /** The compact form while the session on screen has a pending permission request: the panel closes and does not open by itself. */
  held: boolean
}

export interface Scene {
  active: Active
  /** The active avatar's bubble: closed, opened by itself, or opened by the user. */
  bubble: 'closed' | 'self' | 'user'
  /** The line the bubble shows as its caption: the one presented here last. */
  lineId: string | null
  /** That line is being presented now. */
  presenting: boolean
  silent: boolean
  chars: number
  /** When the bubble opened by itself. */
  openedAt: number
  /** When the presentation of its line ended. */
  endedAt: number | null
  /** When the pointer, focus or the question last let go of it. */
  releasedAt: number | null
  /** One of them holds it open now. */
  gripped: boolean
  guard: Guard | null
  /** A line the user asked to hear (Hear it): when it is presented, that is the user's doing and no change of the bubble by itself. Until the next line starts. */
  toHear: string | null
  /**
   * The user opened it: its reply box takes focus once it is on screen, so that what the user
   * types goes to this avatar and not to the terminal that had focus (`focusTaken` after). Never
   * for a bubble that opened by itself: the user is typing somewhere else then.
   */
  focus: boolean
  /** The question on screen, and since when: its choices take no click at first. */
  askId: string | null
  askAt: number
  /** What the user closed each avatar's bubble on, the latest first: it does not come back by itself for that line or that question. */
  shut: readonly Shut[]
  /** The active avatar's session needed the user when the scene was last settled. */
  blocked: boolean
  held: boolean
  /** When the scene has to be settled again although no fact changed; null: not by itself. */
  wakeAt: number | null
}

/** What the user closed an avatar's bubble on: the line it showed, the question it showed. */
export interface Shut {
  id: string
  line: string | null
  ask: string | null
}

/** What was closed is remembered for this many avatars, the latest first. */
const SHUT_KEPT = 8
const NOTHING_SHUT: readonly Shut[] = Object.freeze([])

export const EMPTY: Scene = Object.freeze({
  active: NOBODY,
  bubble: 'closed',
  lineId: null,
  presenting: false,
  silent: false,
  chars: 0,
  openedAt: 0,
  endedAt: null,
  releasedAt: null,
  gripped: false,
  guard: null,
  toHear: null,
  focus: false,
  askId: null,
  askAt: 0,
  shut: NOTHING_SHUT,
  blocked: false,
  held: false,
  wakeAt: null,
})

const shutOf = (scene: Scene, id: string): Shut | undefined => scene.shut.find((x) => x.id === id)
/** What was closed, without this avatar's (the same list when it has nothing closed). */
const unshut = (shut: readonly Shut[], id: string): readonly Shut[] => (shut.some((x) => x.id === id) ? shut.filter((x) => x.id !== id) : shut)
const sameShut = (a: readonly Shut[], b: readonly Shut[]): boolean => a === b || (a.length === b.length && a.every((x, i) => x.id === b[i].id && x.line === b[i].line && x.ask === b[i].ask))

/**
 * The scene of an avatar that just became the active one: nothing of the last one's bubble
 * is left, but what the user closed. That is remembered per avatar, so what was closed
 * stays closed whichever avatar is active in between.
 */
const fresh = (prev: Scene, active: Active): Scene => ({ ...EMPTY, active, held: prev.held, shut: prev.shut })

const SCENE_KEYS = (Object.keys(EMPTY) as (keyof Scene)[]).filter((k) => k !== 'active' && k !== 'guard' && k !== 'shut')
/** Whether two scenes say the same. The scene that was is kept then, so that settling again changes nothing. */
const same = (a: Scene, b: Scene): boolean => SCENE_KEYS.every((k) => a[k] === b[k]) && a.active.id === b.active.id && a.active.idleSince === b.active.idleSince && sameGuard(a.guard, b.guard) && sameShut(a.shut, b.shut)

function pass(prev: Scene, f: Facts): Scene {
  const now = f.now
  const active = pickActive(prev.active, {
    now,
    shown: f.shown,
    onScreen: f.onScreen,
    presenting: f.presenting?.terminalId ?? null,
    hovered: f.hovered,
    focused: f.focused,
    overGroup: f.overGroup || (prev.bubble !== 'closed' && f.inBubble),
    opened: prev.bubble === 'user',
    bubbleOpen: prev.bubble !== 'closed',
    listening: f.listening,
  })
  const s: Scene = active.id === prev.active.id ? { ...prev, active } : fresh(prev, active)
  const wakes: number[] = []
  const me = active.id === null ? undefined : f.shown.find((c) => c.id === active.id)

  if (!me) {
    const none = { ...fresh(prev, NOBODY), held: f.held }
    return same(none, prev) ? prev : none
  }

  const open = (by: 'self') => {
    s.bubble = by
    s.openedAt = now
    s.guard = armGuard(now, f.pointer)
  }
  const changed = () => {
    if (s.bubble !== 'closed') s.guard = armGuard(now, f.pointer)
  }

  // The compact form with a permission request on screen, and a session that needs the user: the bubble closes.
  if ((f.held && !s.held) || (me.blocked && !s.blocked)) {
    s.bubble = 'closed'
    s.guard = null
  }
  s.held = f.held
  s.blocked = me.blocked
  /** It does not open by itself now. */
  const quiet = me.blocked || f.held
  /** Its question keeps the bubble open (a guest's does not: it is shown while it is presented, and again when the user asks). */
  const asks = me.askId !== null && me.guest !== true
  /** What the user closed this avatar's bubble on. */
  const closed = shutOf(s, me.id)

  // A line of it is presented here.
  const mine = f.presenting && f.presenting.terminalId === me.id ? f.presenting : null
  if (mine) {
    if (!s.presenting || s.lineId !== mine.lineId) {
      // The line on screen said once more, or a line the user asked to hear: nothing changed by itself.
      const byItself = s.lineId !== mine.lineId && s.toHear !== mine.lineId
      s.lineId = mine.lineId
      s.presenting = true
      s.endedAt = null
      s.toHear = null
      if (s.bubble !== 'closed') {
        if (byItself) changed()
      } else if (!quiet && closed?.line !== mine.lineId) open('self')
    }
    s.silent = mine.silent
    s.chars = mine.chars
  } else if (s.presenting) {
    s.presenting = false
    s.endedAt = now
  }

  // Its question.
  if (me.askId !== s.askId) {
    s.askId = me.askId
    s.askAt = now
    if (me.askId === null) s.releasedAt = now
    else changed()
  }
  if (asks && s.bubble === 'closed' && !quiet && closed?.ask !== me.askId) open('self')

  // What holds it open: the pointer or focus inside, a dictation into its reply box, or a question.
  const gripped = s.bubble !== 'closed' && (f.inBubble || f.focusInBubble || f.listening || asks)
  if (s.gripped && !gripped) s.releasedAt = now
  s.gripped = gripped

  // It closes by itself four seconds after the last reason to stay; a line presented silently is read first.
  if (s.bubble === 'self' && !s.presenting && !gripped) {
    const read = s.endedAt !== null && s.silent ? readingMs(s.chars) : 0
    const closeAt = Math.max((s.endedAt ?? s.openedAt) + read, s.releasedAt ?? Number.NEGATIVE_INFINITY) + CLOSE_AFTER_MS
    if (now >= closeAt) {
      s.bubble = 'closed'
      s.gripped = false
    } else wakes.push(closeAt)
  }

  // A user who writes in the reply box works in the bubble: no click of theirs is on its way to something else.
  s.guard = s.bubble === 'closed' || f.focusInReply ? null : guardStep(s.guard, now, f.inBubble, f.pointer)
  if (s.bubble === 'closed') s.toHear = null
  // Only a bubble the user opened takes focus.
  if (s.bubble !== 'user') s.focus = false
  const guardAt = guardWake(s.guard, now)
  if (guardAt !== null) wakes.push(guardAt)
  if (s.active.idleSince !== null && s.bubble === 'closed') wakes.push(Math.max(s.active.idleSince + SHRINK_AFTER_MS, now))
  s.wakeAt = wakes.length > 0 ? Math.min(...wakes) : null
  return same(s, prev) ? prev : s
}

/**
 * The scene now. The same object comes back while nothing changed, so it can be called
 * as often as facts change or a timer fires.
 */
export function settle(prev: Scene, f: Facts): Scene {
  let scene = prev
  // A bubble that closes lets its avatar go, and another one may take the stage: until nothing moves any more.
  for (let i = 0; i < 4; i++) {
    const next = pass(scene, f)
    if (next === scene) break
    scene = next
  }
  return scene
}

/**
 * The user opened this avatar's bubble (a click on its figure, its face, its Reply control, a
 * menu, the palette): it becomes the active one, whatever else holds, and its reply box takes
 * focus (`focus`). `blocked`: its session needs the user now. The bubble the user opened stays
 * all the same; only a block that arrives later closes it. What the user had closed of this
 * avatar is open again; what was closed of the others stays so.
 */
export function openByUser(prev: Scene, id: string, blocked = false): Scene {
  const base = prev.active.id === id ? prev : { ...fresh(prev, { id, idleSince: null }), blocked }
  return { ...base, active: { id, idleSince: null }, bubble: 'user', guard: null, gripped: false, focus: true, shut: unshut(prev.shut, id) }
}

/** The overlay moved focus into the reply box the user opened, or found none to move it into: it is not moved again. */
export const focusTaken = (prev: Scene): Scene => (prev.focus ? { ...prev, focus: false } : prev)

/** Where an Esc was pressed: inside the overlay (its bubble, a figure, the face), in a terminal, with nothing focused, or anywhere else. */
export type EscapeFrom = 'overlay' | 'terminal' | 'nowhere' | 'elsewhere'

/**
 * Whether an Esc pressed outside the overlay closes the bubble and goes no further (inside it,
 * the overlay's own keys close any open bubble). Only a bubble the user opened: one that opened
 * by itself leaves the key to whatever has focus, as a bare Esc interrupts an agent CLI. And
 * only from a terminal or from nothing focused: a menu, a dialog, an editor or a field keeps
 * its own Esc.
 */
export function escapeCloses(scene: Scene, from: EscapeFrom): boolean {
  return scene.bubble === 'user' && (from === 'terminal' || from === 'nowhere')
}

/**
 * Whether the bubble offers "Click to hear <name>" under the line it would say (`line`):
 * while voice is on and the page has had no gesture yet (`unlocked` false). And after a
 * gesture unlocked speech, still under the line it stood under then (`stoodUnder`), until
 * that line is heard: the press on the button is itself such a gesture, and the button must
 * not go from under the pointer before its click says the line.
 */
export function offersToHear(i: { voice: boolean | null; unlocked: boolean; spoke: boolean; line: string | null; stoodUnder: string | null | undefined }): boolean {
  if (i.voice !== true || !i.spoke) return false
  return !i.unlocked || (i.line !== null && i.stoodUnder === i.line)
}

/**
 * The user closed the bubble: it stays closed for the line and the question it showed (a
 * bubble that showed none keeps what was closed of its avatar before), whichever avatar
 * is active in between.
 */
export function closeByUser(prev: Scene, now: number): Scene {
  if (prev.bubble === 'closed') return prev
  const id = prev.active.id
  const was = id === null ? undefined : shutOf(prev, id)
  const line = prev.lineId ?? was?.line ?? null
  const ask = prev.askId ?? was?.ask ?? null
  const shut = id === null || (line === null && ask === null) ? prev.shut : [{ id, line, ask }, ...unshut(prev.shut, id)].slice(0, SHUT_KEPT)
  return { ...prev, bubble: 'closed', guard: null, gripped: false, focus: false, releasedAt: now, shut }
}

/**
 * Whether the bubble of avatar `id` shows its reply box now: with its question when that
 * takes a typed answer, and in the transcript the user opened. A draft holds the stage
 * only while its box is on screen.
 */
export function replyShown(scene: Scene, id: string, ask: Pick<Ask, 'freeText'> | null): boolean {
  if (scene.active.id !== id || scene.bubble === 'closed') return false
  return ask ? ask.freeText !== false : scene.bubble === 'user'
}

/**
 * The user asked the open bubble of avatar `id` to say a line (Hear it, Click to hear, Allow
 * online voices): when it is presented, the bubble does not take that for a change by
 * itself and lets no click through.
 */
export function askedToHear(prev: Scene, id: string, lineId: string): Scene {
  if (prev.bubble === 'closed' || prev.active.id !== id || prev.toHear === lineId) return prev
  return { ...prev, toHear: lineId }
}

/** A press on a figure or a face: it closes the bubble that is open for this avatar, else opens it. */
export function pressed(prev: Scene, id: string, now: number, blocked = false): Scene {
  return prev.active.id === id && prev.bubble !== 'closed' ? closeByUser(prev, now) : openByUser(prev, id, blocked)
}

/** Whether clicks go through the bubble now. */
export const clicksThrough = (scene: Scene): boolean => scene.bubble !== 'closed' && scene.guard?.phase === 'timed'

/** Whether the bubble takes the pointer now but its controls do not act on it (they look inactive then). */
export const holding = (scene: Scene): boolean => scene.bubble !== 'closed' && scene.guard?.phase === 'hold'

/** Whether a question's choices take a click now: not in the first moment after it appeared or changed. */
export const choicesReady = (scene: Scene, now: number): boolean => now - scene.askAt >= CHOICE_GUARD_MS

// ---------------------------------------------------------------- the reply

/** What the reply box of one avatar holds. */
export interface Draft {
  text: string
  /** The question that was on screen when the user began: the reply answers it. null: a message to the session. */
  askId: string | null
  /** Some of it was dictated. */
  voice: boolean
}

export const NO_DRAFT: Draft = Object.freeze({ text: '', askId: null, voice: false })

/**
 * The draft after the user typed or dictated. `onScreen`: the question the bubble shows
 * now. A draft that begins is for that question; one that goes on stays what it was for.
 * `begunFor`: for dictated words, the question that was on screen when the microphone was
 * pressed. The user began then, not when the recognizer gave its first words, so a draft
 * that begins with them is for that question.
 */
export function edited(draft: Draft | undefined, text: string, onScreen: string | null, dictated: boolean, begunFor?: string | null): Draft {
  const was = draft ?? NO_DRAFT
  const next = typeof text === 'string' ? text.slice(0, MAX_DRAFT_CHARS) : ''
  if (next === '') return NO_DRAFT
  if (was.text === '') return { text: next, askId: dictated && begunFor !== undefined ? begunFor : onScreen, voice: dictated }
  return { text: next, askId: was.askId, voice: was.voice || dictated }
}

/** Whether the question changed under the draft: the bubble says so, and the next Enter does not send yet. */
export function askChanged(draft: Draft | undefined, onScreen: string | null): boolean {
  return !!draft && draft.text.trim() !== '' && draft.askId !== onScreen
}

/** The body of `POST /api/avatar/{tid}/reply`. */
export interface ReplyBody {
  askId?: string
  text?: string
  choice?: number
  via: 'click' | 'typed' | 'voice'
}

export type SendPlan =
  /** Nothing to send. */
  | { kind: 'nothing' }
  /** The question changed under the draft: it is now for what is on screen, and another Enter sends it. */
  | { kind: 'confirm'; draft: Draft }
  | { kind: 'send'; body: ReplyBody }

/**
 * What Enter (or Send) does with a draft. `ask`: the question on screen now. An answer
 * that names one of the choices is sent as that choice; without a question the text is a
 * message to the session.
 */
export function planSend(draft: Draft | undefined, ask: Ask | null): SendPlan {
  const text = draft ? draft.text.trim() : ''
  if (!draft || text === '') return { kind: 'nothing' }
  const onScreen = ask?.id ?? null
  if (draft.askId !== onScreen) return { kind: 'confirm', draft: { ...draft, askId: onScreen } }
  const via = draft.voice ? 'voice' : 'typed'
  if (!ask) return { kind: 'send', body: { text, via } }
  // The reply box is not offered for a question that takes choices only.
  if (ask.freeText === false && matchChoice(text, ask.choices) === null) return { kind: 'nothing' }
  const choice = matchChoice(text, ask.choices)
  return { kind: 'send', body: choice === null ? { askId: ask.id, text, via } : { askId: ask.id, choice, via } }
}

/**
 * What a click on a choice sends: the question that was on screen when the button was
 * pressed, and the index of the choice. null: the click does not count (the question
 * appeared or changed a moment ago, or there is no such choice).
 */
export function planChoice(ask: Ask | null, index: number, scene: Scene, now: number): ReplyBody | null {
  if (!ask || !Array.isArray(ask.choices) || !Number.isInteger(index) || index < 0 || index >= ask.choices.length) return null
  if (scene.askId === ask.id && !choicesReady(scene, now)) return null
  return { askId: ask.id, choice: index, via: 'click' }
}

/** What the bubble says of a reply a dialog left in the session's input box: the user sends it there. */
export const IN_BOX = "It is in the session's input box: answer its dialog, then press Enter there."

/** What the bubble says after a reply was delivered (`inBox`: pasted, and its Enter withheld by a dialog). */
export function sentNote(delivered: 'answer' | 'prompt' | 'inBox', working: boolean): { kind: 'sent' } | { kind: 'sentLater' } | { kind: 'info'; text: string } {
  if (delivered === 'inBox') return { kind: 'info', text: IN_BOX }
  return delivered === 'prompt' && working ? { kind: 'sentLater' } : { kind: 'sent' }
}

// ---------------------------------------------------------------- the notes about a reply

/** "Sent.", "Sent. It reads this at its next step." and "The question changed." go this long after they appeared, at the latest. */
export const NOTE_MS = 4000

/** The notes the bubble shows under a reply (the view's `BubbleNote`). */
export type NoteKind = 'sent' | 'sentLater' | 'changed' | 'error' | 'info'

/** The avatar's line this tab presented last (or presents now), and when that began. */
export interface LastLine {
  lineId: string
  since: number
}

/** A note in an avatar's bubble, with what it belongs to. */
export interface Noted<N extends { kind: NoteKind } = { kind: NoteKind }> {
  note: N
  /** When it appeared: one that lapses goes `NOTE_MS` later. */
  at: number
  /** When what it is about began (the reply was sent, the question changed): a line of the avatar presented since takes it away. */
  since: number
  /** The one line that does not: the line on screen then, said again, or for "The question changed." the new question's. null: none. */
  line: string | null
}

/** A note that appears `now`, about what began at `since` (by default now), and belongs to `line`. */
export const noted = <N extends { kind: NoteKind }>(note: N, now: number, line: string | null, since: number = now): Noted<N> => ({ note, at: now, since: Math.min(since, now), line })

/** Whether a note goes by itself after `NOTE_MS`: a note about a reply that went, or about the question that changed. An error or an info waits for the user. */
const lapses = (kind: NoteKind): boolean => kind === 'sent' || kind === 'sentLater' || kind === 'changed'

/**
 * The note at `now`, or null once it has gone. `last`: the avatar's line this tab
 * presented last. Every note goes when another line of the avatar is presented (also one
 * that began while the reply was still on its way); a note that lapses also `NOTE_MS`
 * after it appeared. The same object while it stays.
 */
export function noteStep<N extends { kind: NoteKind }>(n: Noted<N> | null | undefined, now: number, last: LastLine | null | undefined): Noted<N> | null {
  if (!n) return null
  if (last && last.since > n.since && last.lineId !== n.line) return null
  if (lapses(n.note.kind) && now >= n.at + NOTE_MS) return null
  return n
}

/**
 * The note about a reply whose answer came back at `now`. `sent`: the line this tab had
 * presented last when the reply was sent, and when that was; `last`: the one it presented
 * last now. "Sent." is about the reply, so a line the avatar began while it was on its way
 * comes after it and takes it away. An error is about now: the user sees that the reply was
 * refused, until the next line. So is an info (the reply waits in the session's input box):
 * the user has something to do.
 */
export function replyNoted<N extends { kind: NoteKind }>(note: N, now: number, sent: { line: string | null; since: number }, last: LastLine | null | undefined): Noted<N> {
  if (note.kind === 'error' || note.kind === 'info') return noted(note, now, last?.lineId ?? null)
  return noted(note, now, sent.line, sent.since)
}

/** When a note has to be looked at again by itself, or null. */
export const noteWake = (n: Noted | null | undefined): number | null => (n && lapses(n.note.kind) ? n.at + NOTE_MS : null)

/** The user changed the draft (typed, dictated): an error or an info goes; a note that lapses keeps its time. Sending again gives a note of its own. */
export function noteEdited<N extends { kind: NoteKind }>(n: Noted<N> | null | undefined): Noted<N> | null {
  return n && lapses(n.note.kind) ? n : null
}

/**
 * Whether the question on screen changed under a draft: it was `before` (undefined: not
 * known yet), it is `now`, and the draft was begun for another one. The bubble says so
 * then ("The question changed."), and once more when Enter only takes note of it.
 */
export function questionChanged(draft: Draft | undefined, before: string | null | undefined, now: string | null): boolean {
  return before !== undefined && before !== now && askChanged(draft, now)
}

/**
 * Which form of the voice choice the bubble shows under what the avatar said (`spoke`).
 * Before voice was chosen (`voice` null): the first choice, `offer` (Hear it, Captions
 * only), or the form "Hear it" led to (`asked`). With voice on: the form of the first
 * choice for a line this browser may not say, so the user sees why it stays a caption.
 * `can` is what the browser can say the line's language with (`voicesFor`): only online
 * voices while they are not allowed gives `online` (Allow online voices, Captions only),
 * no voice at all `none`, once the browser has listed its voices (`listed`: before that an
 * empty list says nothing). Voice off, or a line that can be said: no form.
 */
export function voiceForm(i: { voice: boolean | null; online: boolean; can: 'device' | 'online' | 'none'; listed: boolean; asked?: 'online' | 'none' | null; spoke: boolean }): 'offer' | 'online' | 'none' | null {
  if (!i.spoke) return null
  if (i.voice === null) return i.asked ?? 'offer'
  if (i.voice !== true) return null
  if (i.can === 'none') return i.listed === true ? 'none' : null
  return i.can === 'online' && i.online !== true ? 'online' : null
}

/**
 * Why the choices and the reply box cannot be used now. A reply is pasted into the
 * session unless a call of the session waits for it, and nothing is pasted while the
 * session shows a dialog or is still starting.
 */
export function whyDisabled(i: { composeBlocked: boolean; starting: boolean; waiting: boolean }): 'dialog' | 'starting' | null {
  if (!i.composeBlocked || i.waiting) return null
  return i.starting ? 'starting' : 'dialog'
}
