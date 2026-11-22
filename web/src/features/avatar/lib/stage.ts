// Who is on stage: which avatars this window shows, in which order, which of them is the
// active one (the large one, whose bubble may be open), and what of a transcript the user
// has not seen yet.
//
// Shown are the avatars that are on and whose session is an open agent session of the
// project on screen. A session that exited sleeps for a minute and then leaves. Another
// project's avatar is shown only while it has an open question, and only that question
// is presented here.
//
// Pure: records, a few facts of each session and the clock come in. No React, no DOM.

import type { AvatarInfo, Line, SessionTone } from './types'

/** An exited session whose tab is still open keeps its sleeping avatar this long. */
export const SLEEPING_SHOWN_MS = 60_000
/** The active avatar shrinks this long after its last reason to be large ended. */
export const SHRINK_AFTER_MS = 6000
/** As many figures stand beside each other; the rest are behind a "+n" chip. */
export const MAX_FIGURES = 4
/** "Designing its look…" is said for this long after a session started, at most. */
export const DESIGNING_MS = 180_000
/** A transcript the user opened shows this many lines. */
export const LOG_LINES = 6

// ---------------------------------------------------------------- sessions

/** What this module needs to know of a terminal. */
export interface Session {
  id: string
  projectId: string | null
  /** An agent session (only those have avatars). */
  agent: boolean
  /** Its tab is open. */
  open: boolean
  /** It has not exited. */
  running: boolean
  pinned: boolean
  order: number
  createdAt: number
}

/** The order of the agents column's tabs: pinned first, then as the server lists them. */
function tabOrder(a: Session, b: Session): number {
  return Number(b.pinned) - Number(a.pinned) || a.order - b.order || a.createdAt - b.createdAt || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)
}

/**
 * Per agent session: `running`, or when this window saw it exit. A session that had
 * exited before this window first saw it exited long ago (minus infinity): the clock of
 * the server is never compared with the browser's.
 */
export type Exits = ReadonlyMap<string, number | 'running'>

/** `prev` brought up to date with the sessions as they are now. The same map when nothing changed. */
export function noteExits(prev: Exits, sessions: readonly Session[], now: number): Exits {
  const next = new Map<string, number | 'running'>()
  let changed = false
  for (const s of sessions) {
    if (!s.agent) continue
    const was = prev.get(s.id)
    const is = s.running ? 'running' : was === 'running' ? now : typeof was === 'number' ? was : Number.NEGATIVE_INFINITY
    next.set(s.id, is)
    if (is !== was) changed = true
  }
  return changed || next.size !== prev.size ? next : prev
}

// ---------------------------------------------------------------- who shows

export interface Staged {
  terminalId: string
  /** A session of another project: here only for its open question. */
  foreign: boolean
  /** Its session exited: it sleeps, and leaves the stage at this time. null: it stays. */
  leavesAt: number | null
}

export interface StageInput {
  /** Avatars are enabled on the server. */
  enabled: boolean
  avatars: readonly AvatarInfo[]
  sessions: readonly Session[]
  /** The project on screen. */
  projectId: string | null
  exits: Exits
  now: number
}

/**
 * The avatars this window shows, the current project's first, each group in the order of
 * its sessions' tabs.
 */
export function whoShows(input: StageInput): Staged[] {
  if (input.enabled !== true || !Array.isArray(input.avatars)) return []
  const sessions = new Map<string, Session>()
  for (const s of input.sessions) sessions.set(s.id, s)
  const here = input.projectId ?? null
  const own: { session: Session; staged: Staged }[] = []
  const foreign: { session: Session; staged: Staged }[] = []
  for (const a of input.avatars) {
    if (typeof a !== 'object' || a === null || a.on !== true) continue
    const session = sessions.get(a.terminalId)
    if (!session || !session.agent || !session.open) continue
    if ((session.projectId ?? null) !== here) {
      // Another project's avatar: only while it asks, and only a session that could take the answer.
      if (a.ask && session.running) foreign.push({ session, staged: { terminalId: a.terminalId, foreign: true, leavesAt: null } })
      continue
    }
    if (session.running) {
      own.push({ session, staged: { terminalId: a.terminalId, foreign: false, leavesAt: null } })
      continue
    }
    const exit = input.exits.get(a.terminalId)
    // Not noted yet (the caller notes exits first): it exited just now.
    const exitedAt = typeof exit === 'number' ? exit : input.now
    const leavesAt = exitedAt + SLEEPING_SHOWN_MS
    if (input.now < leavesAt) own.push({ session, staged: { terminalId: a.terminalId, foreign: false, leavesAt } })
  }
  const byTab = (a: { session: Session }, b: { session: Session }) => tabOrder(a.session, b.session)
  return [...own.sort(byTab), ...foreign.sort(byTab)].map((x) => x.staged)
}

/** When the stage changes by itself next (a sleeping avatar leaves), or null. */
export function nextLeave(staged: readonly Staged[]): number | null {
  let at: number | null = null
  for (const s of staged) if (s.leavesAt !== null && (at === null || s.leavesAt < at)) at = s.leavesAt
  return at
}

/** Whether two stagings show the same avatars in the same way. */
export function sameStage(a: readonly Staged[], b: readonly Staged[]): boolean {
  return a.length === b.length && a.every((s, i) => s.terminalId === b[i].terminalId && s.foreign === b[i].foreign && s.leavesAt === b[i].leavesAt)
}

const QUESTIONS = new WeakMap<AvatarInfo, AvatarInfo>()

/**
 * The record as this window presents it. Of another project's avatar only the line of its
 * open question is presented: its other lines are neither shown nor said here, and the
 * arbiter, which knows no projects, is offered this record. The same object for the same
 * record.
 */
export function presented(avatar: AvatarInfo, foreign: boolean): AvatarInfo {
  if (!foreign) return avatar
  const known = QUESTIONS.get(avatar)
  if (known) return known
  const lineId = avatar.ask?.lineId
  const only = { ...avatar, lines: Array.isArray(avatar.lines) ? avatar.lines.filter((l) => l?.id === lineId && l.from === 'agent') : [] }
  QUESTIONS.set(avatar, only)
  return only
}

/**
 * Which figures stand beside each other and which are behind the chip. The order never
 * changes; only the active avatar, when it would be behind the chip, takes the last place.
 */
export function figures(order: readonly string[], active: string | null, max = MAX_FIGURES): { visible: string[]; more: string[] } {
  const room = Math.max(1, Math.floor(max))
  if (order.length <= room) return { visible: [...order], more: [] }
  const visible = order.slice(0, room)
  const more = order.slice(room)
  if (active !== null && more.includes(active)) {
    const gives = visible[room - 1]
    visible[room - 1] = active
    more[more.indexOf(active)] = gives
  }
  return { visible, more }
}

/**
 * The avatar the compact form stands for, and the one a reply without a name is for: the
 * active one, else one that waits with a question (`waiting`, in the order of the stage:
 * a guest rests until the user turns to it, and where there is room for one face only,
 * that face is its), else the one of the session on screen, else the first.
 */
export function front(order: readonly string[], active: string | null, onScreen: string | null, waiting: readonly string[] = []): string | null {
  if (active !== null && order.includes(active)) return active
  if (onScreen !== null && waiting.includes(onScreen) && order.includes(onScreen)) return onScreen
  const asks = waiting.find((id) => order.includes(id))
  if (asks !== undefined) return asks
  if (onScreen !== null && order.includes(onScreen)) return onScreen
  return order[0] ?? null
}

// ---------------------------------------------------------------- who is active

export interface Candidate {
  id: string
  /** Its session needs a permission or an answer: it is quiet, and neither a line nor a question makes it the active one. */
  blocked: boolean
  /** Its open question, or null. */
  askId: string | null
  /** Its reply box is on screen and holds text. A draft kept while the box is not on screen is false here (`replyShown`). */
  draft: boolean
  /**
   * A session of another project, here for its open question. It is a guest: its question
   * is presented like any line, but does not hold the stage afterwards. It rests, with the
   * attention ring, until the user turns to it.
   */
  guest?: boolean
}

export interface ActiveFacts {
  now: number
  /** The avatars on stage, in their order. */
  shown: readonly Candidate[]
  /** The session on screen: of several avatars that ask, its own goes first. */
  onScreen: string | null
  /** The avatar whose line this tab presents now. */
  presenting: string | null
  /** A figure the pointer rests on. */
  hovered: string | null
  /** The avatar that holds focus (its figure, its bubble or its reply box). */
  focused: string | null
  /** The pointer is over the group or the bubble. */
  overGroup: boolean
  /** The user opened the active avatar's bubble. */
  opened: boolean
  /** Its bubble is on screen, whoever opened it: it does not shrink beside an open bubble. */
  bubbleOpen: boolean
  /** The microphone of its bubble's reply box listens: the user is answering, also before the first words came. */
  listening: boolean
}

export interface Active {
  id: string | null
  /** When its last reason to be the active one ended; null while one holds. It shrinks `SHRINK_AFTER_MS` later. */
  idleSince: number | null
}

export const NOBODY: Active = Object.freeze({ id: null, idleSince: null })

/**
 * The active avatar after the facts changed. One presents a line, has an open question,
 * is hovered or focused, or was opened by the user. It does not change while the pointer
 * is over the group, focus is inside it, or its open bubble's reply box holds text or
 * listens to the user, and never away from a bubble the user opened; otherwise a line
 * that is presented takes the stage at once, and everything else waits until the active
 * one has had its six seconds. A draft kept while its reply box is not on screen (behind
 * a closed bubble, or in a bubble that has no box now) holds nothing: it is there again
 * when the box is.
 */
export function pickActive(prev: Active, f: ActiveFacts): Active {
  const byId = new Map<string, Candidate>()
  for (const c of f.shown) byId.set(c.id, c)
  const on = (id: string | null): id is string => id !== null && byId.has(id)
  const was = on(prev.id) ? prev.id : null
  const presenting = on(f.presenting) && !byId.get(f.presenting)?.blocked ? f.presenting : null
  const asking = f.shown.filter((c) => c.askId !== null && !c.blocked && c.guest !== true).map((c) => c.id)
  const hovered = on(f.hovered) ? f.hovered : null
  const focused = on(f.focused) ? f.focused : null
  const reason = (id: string) => id === presenting || asking.includes(id) || id === hovered || id === focused
  const sticky = was !== null && (f.overGroup || focused !== null || (f.bubbleOpen && (f.listening || byId.get(was)?.draft === true)))
  /** Whoever has a reason, when the stage is free: a question first (the session on screen's before others). */
  const fresh = (): string | null => (on(f.onScreen) && asking.includes(f.onScreen) ? f.onScreen : (asking[0] ?? null)) ?? focused ?? hovered

  /** The same object while nothing changed: whoever holds the result is not told about nothing. */
  const is = (id: string | null, idleSince: number | null): Active => (id === prev.id && idleSince === prev.idleSince ? prev : id === null ? NOBODY : { id, idleSince })

  if (was !== null && (f.opened || sticky)) return is(was, null)
  if (presenting !== null && presenting !== was) return is(presenting, null)
  if (was === null) return is(fresh(), null)
  if (reason(was)) return is(was, null)
  const idleSince = prev.idleSince ?? f.now
  if (f.bubbleOpen || f.now - idleSince < SHRINK_AFTER_MS) return is(was, idleSince)
  return is(fresh(), null)
}

// ---------------------------------------------------------------- seen and unseen

const agentIds = (lines: readonly Line[]): string[] => (Array.isArray(lines) ? lines.filter((l) => l?.from === 'agent' && typeof l.id === 'string').map((l) => l.id) : [])

/**
 * What of a transcript the user has not seen: `newest`, the newest agent line (the dot is
 * shown while it is not the seen one), and `from`, the first agent line after the one seen
 * last (null: everything was seen). `seenId` is what the preferences remember.
 */
export function unseen(lines: readonly Line[], seenId: string | undefined): { newest: string | null; from: string | null } {
  const ids = agentIds(lines)
  if (ids.length === 0) return { newest: null, from: null }
  const newest = ids[ids.length - 1]
  const at = typeof seenId === 'string' ? ids.lastIndexOf(seenId) : -1
  // Never seen, or seen so long ago that the line has left the transcript: all of it is new.
  if (at < 0) return { newest, from: ids[0] }
  return { newest, from: ids[at + 1] ?? null }
}

/** What to remember once the line `shownId` was on screen: the later of it and what was remembered. */
export function seenAfter(lines: readonly Line[], seenId: string | undefined, shownId: string): string | undefined {
  const ids = agentIds(lines)
  const shown = ids.lastIndexOf(shownId)
  if (shown < 0) return seenId
  const seen = typeof seenId === 'string' ? ids.lastIndexOf(seenId) : -1
  return shown > seen ? shownId : seenId
}

/**
 * What a transcript the user opened shows: the last few lines, and the first of them the
 * user has not seen (where it starts to read). When the unseen lines begin before what is
 * shown, everything the agent says in it is new.
 */
export function transcript(lines: readonly Line[], seenId: string | undefined, count = LOG_LINES): { lines: Line[]; unseenFrom: string | null } {
  const all = Array.isArray(lines) ? lines : []
  const shown = all.slice(-Math.max(1, count))
  const { from } = unseen(all, seenId)
  if (from === null) return { lines: shown, unseenFrom: null }
  const first = all.findIndex((l) => l?.id === from)
  const start = all.length - shown.length
  return { lines: shown, unseenFrom: first >= start ? from : (shown.find((l) => l?.from === 'agent')?.id ?? null) }
}

// ---------------------------------------------------------------- small facts

/** Lines of the avatars on stage that nobody acknowledged yet, for a window that hides its avatars. */
export function unacknowledged(avatars: readonly AvatarInfo[], told: ReadonlySet<string>): { terminalId: string; lineId: string; key: string }[] {
  const out: { terminalId: string; lineId: string; key: string }[] = []
  for (const a of avatars) {
    if (typeof a !== 'object' || a === null || !Array.isArray(a.lines)) continue
    for (const l of a.lines) {
      if (l?.from !== 'agent' || typeof l.id !== 'string' || typeof l.outcome === 'string') continue
      const key = `${a.terminalId}:${l.id}`
      if (!told.has(key)) out.push({ terminalId: a.terminalId, lineId: l.id, key })
    }
  }
  return out
}

/**
 * Whether the label under the default look reads "Designing its look…": the session was
 * started with the option, has not designed yet, is working, and started less than three
 * minutes ago.
 */
export function isDesigning(i: { withOption: boolean; designed: boolean; working: boolean; startedAt: number; now: number }): boolean {
  return i.withOption && !i.designed && i.working && i.now - i.startedAt < DESIGNING_MS && i.now >= i.startedAt - DESIGNING_MS
}

/** What a ring can show (`Ring` of Figure.tsx). */
export type RingLike = SessionTone | 'speaking' | 'listening'

const URGENCY: ReadonlyMap<string, number> = new Map([
  ['attention', 9],
  ['error', 8],
  ['speaking', 7],
  ['listening', 6],
  ['unread', 5],
  ['working', 4],
  ['starting', 3],
  ['idle', 2],
  ['exited', 1],
])

/** The ring of the "+n" chip: the most urgent among the avatars behind it. */
export function mostUrgent(rings: readonly RingLike[]): RingLike {
  let best: RingLike = 'idle'
  let score = 0
  for (const ring of rings) {
    const s = URGENCY.get(ring) ?? 0
    if (s > score) {
      best = ring
      score = s
    }
  }
  return best
}
