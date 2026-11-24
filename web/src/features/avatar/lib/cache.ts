// What the events of the avatar slice do to the one cached list (`GET /api/avatar`), and
// how a fetched answer is weighed against what events told meanwhile.
//
// Two sources fill the cache. The list comes from the fetch and is the only source of a
// record's design (look, name, voice, on or off, its existence). Events tell what happens
// between fetches: a line is said, a line gets its outcome, a question opens or closes, a
// gesture is held. An answer can be older than an event that has already arrived, and an
// event can be older than an answer: neither may undo the other.
//
// So a record that events changed carries a small note of what they told and when
// (`told`, a client-side field that is never sent). When an answer arrives, its design is
// always taken; of its lines, its question and its held gesture only what the events did
// not tell more recently. A record no event touched is weighed whole, as the contract
// says: the newer of the two.
//
// Pure: lists in, lists out. The same object comes back when nothing changed.

import type { Ask, AvatarInfo, AvatarList, Line } from './types'

/** The server keeps this many lines of a transcript (`MAX_LINES`), newest last. */
export const MAX_LINES = 30
/** Two reports of a held gesture this close together are one play of it. */
const SAME_PLAY_MS = 5000
const MAX_ID_CHARS = 64
const MAX_TEXT_CHARS = 8000
const MAX_CHOICES = 8
const NAME = /^[A-Za-z][A-Za-z0-9_-]{0,31}$/

/**
 * What events told about a record since it was last weighed against an answer, as the
 * time of the event (the envelope's `ts`); 0: nothing. Lines need no such note: they are
 * weighed one by one.
 */
export interface Told {
  ask: number
  held: number
}

/** A record as the cache holds it: the wire record, and what events told about it. */
export type Kept = AvatarInfo & { told?: Told }

/** A gesture to play once, or to stop: the one thing an event tells that the list does not hold. */
export interface ActSignal {
  terminalId: string
  /** The server's number of the event. It counts from 0 again when the server restarts, so it tells no repeat. */
  seq: number
  play?: string
  /** 1 to 5. */
  times?: number
  /** A name, or `all`. */
  stop?: string
}

export interface Reduced {
  /** The cache after the event: the same object when nothing changed. */
  list: AvatarList | undefined
  /** The list is to be fetched again. */
  refetch: boolean
  /** For the overlay: a gesture to play or stop. */
  act: ActSignal | null
  /** A terminal that is gone: what this browser remembers of it goes too. */
  removed: string | null
}

// ---------------------------------------------------------------- small checks

const idLike = (v: unknown): v is string => typeof v === 'string' && v.length > 0 && v.length <= MAX_ID_CHARS
const num = (v: unknown): number => (typeof v === 'number' && Number.isFinite(v) ? v : 0)
const isRecord = (v: unknown): v is Kept => typeof v === 'object' && v !== null && idLike((v as AvatarInfo).terminalId)
const field = (data: unknown, key: string): unknown => (typeof data === 'object' && data !== null ? (data as Record<string, unknown>)[key] : undefined)

function lineOf(v: unknown): Line | null {
  if (typeof v !== 'object' || v === null) return null
  const l = v as Line
  if (!idLike(l.id) || typeof l.text !== 'string' || l.text.length > MAX_TEXT_CHARS) return null
  if ((l.from !== 'agent' && l.from !== 'user') || typeof l.at !== 'number' || !Number.isFinite(l.at)) return null
  return l
}

function askOf(v: unknown): Ask | null {
  if (typeof v !== 'object' || v === null) return null
  const a = v as Ask
  if (!idLike(a.id) || !idLike(a.lineId) || typeof a.text !== 'string' || a.text.length > MAX_TEXT_CHARS) return null
  if (!Array.isArray(a.choices) || a.choices.length > MAX_CHOICES || !a.choices.every((c) => typeof c === 'string')) return null
  if (typeof a.freeText !== 'boolean' || typeof a.waiting !== 'boolean') return null
  return a
}

/** Best first, as the server ranks them; a line without an outcome is below them all. */
const RANK: ReadonlyMap<string, number> = new Map([
  ['spoken', 6],
  ['skipped', 5],
  ['failed', 4],
  ['shown', 3],
  ['hidden', 2],
  ['unheard', 1],
])
const rank = (line: Line): number => (typeof line.outcome === 'string' ? (RANK.get(line.outcome) ?? 0) : 0)

/** Whether two values of the list say the same (JSON values: order matters in arrays only). */
export function sameValue(a: unknown, b: unknown, depth = 0): boolean {
  if (a === b) return true
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null || depth > 64) return false
  if (Array.isArray(a) !== Array.isArray(b)) return false
  if (Array.isArray(a) && Array.isArray(b)) return a.length === b.length && a.every((v, i) => sameValue(v, b[i], depth + 1))
  const ka = Object.keys(a)
  if (ka.length !== Object.keys(b).length) return false
  return ka.every((k) => Object.hasOwn(b, k) && sameValue((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k], depth + 1))
}

// ---------------------------------------------------------------- one record

const NOTHING_TOLD: Told = { ask: 0, held: 0 }

const toldOf = (rec: Kept): Told => {
  const t = rec.told
  return typeof t === 'object' && t !== null ? { ask: num(t.ask), held: num(t.held) } : NOTHING_TOLD
}

/**
 * At most `MAX_LINES` lines, as the server keeps them (`push_line`): the oldest go first, but
 * never `keep`, the line of the open question.
 */
function trim(lines: Line[], keep: string | null | undefined): Line[] {
  const out = lines.slice()
  while (out.length > MAX_LINES) {
    const oldest = out.findIndex((l) => l?.id !== keep)
    out.splice(oldest < 0 ? 0 : oldest, 1)
  }
  return out
}

/** The lines with one line put in: in place of the one with its id (never a worse outcome for a better), else as the newest. `keep`: the open question's line. */
function putLine(lines: readonly Line[], line: Line, keep: string | null | undefined): readonly Line[] {
  const all = Array.isArray(lines) ? lines : []
  const i = all.findIndex((l) => l?.id === line.id)
  if (i < 0) return trim([...all, line], keep)
  const had = all[i]
  if (rank(had) > rank(line) || sameValue(had, line)) return all
  const next = all.slice()
  next[i] = line
  return next
}

/**
 * A record after an event changed it: the change, the time raised to the event's (the
 * server raises `updatedAt` with every change, and so does what stands in for it here),
 * and the note of what was told. The same record when nothing differs.
 */
function change(rec: Kept, patch: Partial<AvatarInfo>, ts: number, told: Partial<Told> = {}): Kept {
  const before = toldOf(rec)
  const after: Told = { ask: Math.max(before.ask, num(told.ask)), held: Math.max(before.held, num(told.held)) }
  const keys = Object.keys(patch) as (keyof AvatarInfo)[]
  const differs = keys.some((k) => !sameValue(rec[k], patch[k]))
  if (!differs && rec.told !== undefined && after.ask === before.ask && after.held === before.held) return rec
  // An event that only repeats what is known still says how recent that knowledge is.
  return { ...rec, ...(differs ? patch : {}), updatedAt: Math.max(num(rec.updatedAt), ts), told: after }
}

function replaceAt(list: AvatarList, i: number, rec: Kept): AvatarList {
  if (list.avatars[i] === rec) return list
  const avatars = list.avatars.slice()
  avatars[i] = rec
  return { ...list, avatars }
}

function without(list: AvatarList | undefined, terminalId: string): AvatarList | undefined {
  if (!list || !Array.isArray(list.avatars) || !list.avatars.some((a) => a?.terminalId === terminalId)) return list
  return { ...list, avatars: list.avatars.filter((a) => a?.terminalId !== terminalId) }
}

// ---------------------------------------------------------------- events

/**
 * The cache after one event of the bus. `type` and `data` are what the socket delivered,
 * `ts` the envelope's time. An event about an avatar the list does not hold (or that
 * arrives before there is a list) cannot be applied: the list is fetched again instead.
 * What is no event of this slice, and what is malformed, changes nothing.
 */
export function reduce(list: AvatarList | undefined, type: string, data: unknown, ts: number): Reduced {
  const same: Reduced = { list, refetch: false, act: null, removed: null }
  const again: Reduced = { ...same, refetch: true }
  const at = num(ts)
  const usable = !!list && Array.isArray(list.avatars)
  const terminalId = field(data, 'terminalId')
  const index = usable && idLike(terminalId) ? list.avatars.findIndex((a) => a?.terminalId === terminalId) : -1
  const rec = index >= 0 && list ? (list.avatars[index] as Kept) : null

  switch (type) {
    case 'settings.changed':
    case 'resync':
    // The socket's greeting: this tab listens from now on. What was told before it did, after
    // the first answer was made, is in neither, so the list is fetched once more.
    case 'hello':
      return again

    case 'terminal.removed': {
      const id = field(data, 'id')
      if (!idLike(id)) return same
      return { ...same, list: without(list, id), removed: id }
    }

    case 'avatar.changed': {
      if (!idLike(terminalId)) return same
      // Design, name, voice, on or off, existence: only the list tells them whole.
      if (field(data, 'removed') === true) return { ...again, list: without(list, terminalId) }
      const on = field(data, 'on')
      // Off is not to wait for the answer: the avatar falls silent at once.
      if (rec && list && typeof on === 'boolean' && rec.on !== on) return { ...again, list: replaceAt(list, index, { ...rec, on }) }
      return again
    }

    case 'avatar.say': {
      const line = lineOf(field(data, 'line'))
      if (!idLike(terminalId) || !line) return same
      if (!rec || !list) return again
      const said = field(data, 'ask')
      const ask = said === undefined ? null : askOf(said)
      const next = ask ? change(rec, { lines: putLine(rec.lines, line, ask.lineId) as Line[], ask }, at, { ask: at }) : change(rec, { lines: putLine(rec.lines, line, rec.ask?.lineId) as Line[] }, at)
      // A question that cannot be read is asked for with the list.
      return { ...same, list: replaceAt(list, index, next), refetch: said !== undefined && said !== null && !ask }
    }

    case 'avatar.line': {
      const line = lineOf(field(data, 'line'))
      if (!idLike(terminalId) || !line) return same
      if (!rec || !list) return again
      return { ...same, list: replaceAt(list, index, change(rec, { lines: putLine(rec.lines, line, rec.ask?.lineId) as Line[] }, at)) }
    }

    case 'avatar.ask': {
      if (!idLike(terminalId)) return same
      const told = field(data, 'ask')
      const ask = told === null ? null : askOf(told)
      if (told !== null && !ask) return again
      if (!rec || !list) return again
      return { ...same, list: replaceAt(list, index, change(rec, { ask }, at, { ask: at })) }
    }

    case 'avatar.act': {
      if (!idLike(terminalId)) return same
      const play = field(data, 'play')
      const stop = field(data, 'stop')
      const mode = field(data, 'mode')
      const seq = num(field(data, 'seq'))
      if (typeof stop === 'string' && NAME.test(stop)) {
        const act: ActSignal = { terminalId, seq, stop }
        if (!rec || !list) return { ...same, act }
        const ends = rec.held !== null && (stop === 'all' || rec.held?.name === stop)
        return { ...same, act, list: replaceAt(list, index, change(rec, ends ? { held: null } : {}, at, { held: at })) }
      }
      if (typeof play !== 'string' || !NAME.test(play)) return same
      if (mode === 'hold' || mode === 'loop') {
        // The figure plays what the record holds: nothing to signal.
        if (!rec || !list) return again
        return { ...same, list: replaceAt(list, index, change(rec, { held: { name: play, mode, since: at } }, at, { held: at })) }
      }
      const times = Math.min(Math.max(Math.floor(num(field(data, 'times')) || 1), 1), 5)
      const act: ActSignal = { terminalId, seq, play, times }
      if (!rec || !list) return { ...same, act }
      // A held or looping gesture ends at the next play.
      return { ...same, act, list: replaceAt(list, index, change(rec, rec.held !== null ? { held: null } : {}, at, { held: at })) }
    }

    default:
      return same
  }
}

// ---------------------------------------------------------------- a fetched answer

/** The answer's lines, and of the held ones what the answer cannot know yet. `won`: something of the held lines stays. `keep`: the open question's line. */
function mergeLines(held: readonly Line[], fetched: readonly Line[], fetchedAt: number, keep: string | null | undefined): { lines: Line[]; won: boolean } {
  const mine = new Map<string, Line>()
  for (const l of Array.isArray(held) ? held : []) if (idLike(l?.id)) mine.set(l.id, l)
  const listed = new Set<string>()
  let won = false
  const lines: Line[] = (Array.isArray(fetched) ? fetched : []).map((f) => {
    const h = idLike(f?.id) ? mine.get(f.id) : undefined
    if (idLike(f?.id)) listed.add(f.id)
    // An outcome only ever gets better: the better one is the newer knowledge.
    if (h && rank(h) > rank(f)) {
      won = true
      return h
    }
    return f
  })
  // A held line the answer does not list: said after the answer was made (it stays), or dropped by the server before (it goes).
  for (const h of mine.values()) {
    if (listed.has(h.id) || !(num(h.at) > fetchedAt)) continue
    lines.push(h)
    won = true
  }
  return { lines: lines.length > MAX_LINES ? trim(lines, keep) : lines, won }
}

/** Whether two reports of a held gesture are the same play of it (the event's time and the server's differ a little). */
function samePlay(a: AvatarInfo['held'], b: AvatarInfo['held']): boolean {
  return !!a && !!b && a.name === b.name && a.mode === b.mode && Math.abs(num(a.since) - num(b.since)) <= SAME_PLAY_MS
}

function weigh(held: Kept, fetched: AvatarInfo): Kept {
  if (held.told === undefined) {
    // No event touched it since it was fetched: the newer of the two, whole.
    return num(held.updatedAt) > num(fetched.updatedAt) ? held : fetched
  }
  const told = toldOf(held)
  const fetchedAt = num(fetched.updatedAt)
  const askStays = told.ask > 0 && told.ask >= fetchedAt
  const heldStays = told.held > 0 && told.held >= fetchedAt
  const ask = askStays ? held.ask : fetched.ask
  const { lines, won } = mergeLines(held.lines, fetched.lines, fetchedAt, ask?.lineId)
  // The same play keeps its time: a figure must not start a held gesture over because two clocks named it differently.
  const gesture = heldStays || samePlay(held.held, fetched.held) ? held.held : fetched.held
  if (!won && !askStays && !heldStays) {
    // Nothing the events told is newer than the answer: it stands as fetched.
    return gesture === fetched.held ? fetched : { ...fetched, held: gesture }
  }
  return {
    ...fetched,
    lines,
    ask,
    held: gesture,
    updatedAt: Math.max(num(held.updatedAt), fetchedAt),
    told: { ask: askStays ? told.ask : 0, held: heldStays ? told.held : 0 },
  }
}

/**
 * What the cache holds after `GET /api/avatar` answered. Which avatars exist, and
 * everything of the list itself, is the answer's to say. Of each record the design is the
 * answer's too; its lines, question and held gesture are weighed against what events told
 * while the answer was on its way. The answer itself comes back when nothing held is newer.
 */
export function mergeFetched(held: AvatarList | undefined, fetched: AvatarList): AvatarList {
  if (!held || !Array.isArray(held.avatars) || !fetched || !Array.isArray(fetched.avatars)) return fetched
  const mine = new Map<string, Kept>()
  for (const a of held.avatars) if (isRecord(a)) mine.set(a.terminalId, a)
  let kept = false
  const avatars = fetched.avatars.map((f) => {
    const h = isRecord(f) ? mine.get(f.terminalId) : undefined
    if (!h) return f
    const out = weigh(h, f)
    if (out !== f) kept = true
    return out
  })
  return kept ? { ...fetched, avatars } : fetched
}
