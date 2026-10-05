// The readings the plot viewer draws, by session and watched item. The Live tab keeps 120 per item in
// immutable arrays, which is right for a sparkline; a plot needs minutes of them, arriving up to 20 times
// a second, so these are plain growing arrays outside React state: `ingest` appends, and the views read
// them when they draw. `usePlotClock` is what tells a view that something new has come (at most once per
// frame).

import { useSyncExternalStore } from 'react'
import { numericValue } from './logic'
import type { Track } from './plotMath'
import type { LiveEvent, LiveHistory, LiveSnapshot } from './types'

/** Readings kept per item: 10 minutes at the fastest rate (50 ms), over an hour at the default. */
export const PLOT_MAX_READINGS = 12_000
/** Old readings are cut off in chunks of this size, so that most appends cost nothing. */
const TRIM_SLACK = 1_500

interface SessionReadings {
  tracks: Map<number, Track>
  /** The ids on the watch list once it is known; a reading of another id is a removal still on its way. */
  known?: Set<number>
}

const sessions = new Map<string, SessionReadings>()
/** Sessions that were forgotten: a late event or snapshot of one must not bring its buffer back. */
const gone = new Set<string>()
const MAX_GONE = 500
let version = 0
const listeners = new Set<() => void>()
let scheduled = false

function notify() {
  version++
  if (scheduled) return
  scheduled = true
  const later = typeof requestAnimationFrame === 'function' ? requestAnimationFrame : (f: () => void) => setTimeout(f, 16)
  later(() => {
    scheduled = false
    listeners.forEach((l) => l())
  })
}

function of(sid: string): SessionReadings | undefined {
  if (gone.has(sid)) return undefined
  let s = sessions.get(sid)
  if (!s) {
    s = { tracks: new Map() }
    sessions.set(sid, s)
  }
  return s
}

/** `n` readings with no exact digits. */
const blank = (n: number): (string | undefined)[] => Array.from({ length: n }, () => undefined)

/** The digits of a whole number a double cannot hold exactly (a 64-bit value arrives as text), else undefined. */
function exactDigits(v: unknown): string | undefined {
  return typeof v === 'string' && /^-?\d+$/.test(v) && !Number.isSafeInteger(Number(v)) ? v : undefined
}

function append(tr: Track, t: number, v: number, raw?: string) {
  const n = tr.t.length
  if (n && t < tr.t[n - 1]) {
    // The clock stepped back: what came before no longer lines up with what comes now.
    tr.t.length = 0
    tr.v.length = 0
    tr.raw = undefined
  } else if (n && t === tr.t[n - 1]) {
    tr.v[n - 1] = v
    if (raw !== undefined) (tr.raw ??= blank(n))[n - 1] = raw
    else if (tr.raw) tr.raw[n - 1] = undefined
    return
  }
  if (raw !== undefined && !tr.raw) tr.raw = blank(tr.t.length)
  tr.t.push(t)
  tr.v.push(v)
  tr.raw?.push(raw)
  if (tr.t.length > PLOT_MAX_READINGS + TRIM_SLACK) {
    const cut = tr.t.length - PLOT_MAX_READINGS
    tr.t.splice(0, cut)
    tr.v.splice(0, cut)
    tr.raw?.splice(0, cut)
  }
}

function setKnown(s: SessionReadings, ids: number[]) {
  s.known = new Set(ids)
  for (const id of s.tracks.keys()) if (!s.known.has(id)) s.tracks.delete(id) // deleting while iterating a Map is defined
}

/** A `debug.live` event: the watch list changed, or a round of readings came in. */
export function ingest(ev: LiveEvent) {
  const s = of(ev.sessionId)
  if (!s) return
  if (ev.items) setKnown(s, ev.items.map((i) => i.id))
  for (const r of ev.samples ?? []) {
    if ((s.known && !s.known.has(r.id)) || !Number.isFinite(r.t)) continue
    let tr = s.tracks.get(r.id)
    if (!tr) {
      tr = { t: [], v: [] }
      s.tracks.set(r.id, tr)
    }
    // A failed reading, and one that is no number (bytes, text), leave a gap.
    append(tr, r.t, r.e !== undefined ? Number.NaN : (numericValue(r.v) ?? Number.NaN), r.e !== undefined ? undefined : exactDigits(r.v))
  }
  notify()
}

/** The Live tab's snapshot: the watch list, and each item's last reading when nothing is known of it yet. */
export function seed(sid: string, snap: LiveSnapshot) {
  const s = of(sid)
  if (!s) return
  setKnown(s, snap.items.map((i) => i.id))
  for (const item of snap.items) {
    const last = snap.last[String(item.id)]
    if (!last || s.tracks.get(item.id)?.t.length) continue
    if (!Number.isFinite(last.t)) continue
    const tr: Track = { t: [], v: [] }
    s.tracks.set(item.id, tr)
    append(tr, last.t, last.e !== undefined ? Number.NaN : (numericValue(last.v) ?? Number.NaN), last.e !== undefined ? undefined : exactDigits(last.v))
  }
  notify()
}

/**
 * The readings the server kept (a page that was reloaded, a plot opened late): put in front of what has been collected
 * here, so nothing is doubled and nothing that arrived meanwhile is lost. Items the watch list no longer has are skipped.
 */
export function seedHistory(sid: string, h: LiveHistory) {
  const s = of(sid)
  if (!s) return
  for (const [key, data] of Object.entries(h.series)) {
    const id = Number(key)
    if (!Number.isInteger(id) || (s.known && !s.known.has(id)) || data.t.length !== data.v.length) continue
    const cur = s.tracks.get(id)
    const firstHere = cur?.t[0] ?? Number.POSITIVE_INFINITY
    let n = data.t.findIndex((t) => t >= firstHere) // the server's readings older than ours
    if (n < 0) n = data.t.length
    if (n === 0) continue
    let raw: (string | undefined)[] | undefined
    for (const [i, text] of Object.entries(data.exact ?? {})) if (Number(i) < n) (raw ??= blank(n))[Number(i)] = text
    const here = cur?.t.length ?? 0
    const merged: Track = { t: [...data.t.slice(0, n), ...(cur?.t ?? [])], v: [...data.v.slice(0, n).map((x) => x ?? Number.NaN), ...(cur?.v ?? [])] }
    if (raw || cur?.raw) merged.raw = [...(raw ?? blank(n)), ...(cur?.raw ?? blank(here))]
    const over = merged.t.length - PLOT_MAX_READINGS
    if (over > 0) {
      merged.t.splice(0, over)
      merged.v.splice(0, over)
      merged.raw?.splice(0, over)
    }
    s.tracks.set(id, merged)
  }
  notify()
}

/** The session is gone. */
export function forgetReadings(sid: string) {
  gone.add(sid)
  if (gone.size > MAX_GONE) gone.delete(gone.values().next().value as string)
  if (sessions.delete(sid)) notify()
}

/** The readings of a watched item, or undefined before the first one. Live: the arrays keep growing. */
export function trackOf(sid: string | undefined, itemId: number | undefined): Track | undefined {
  if (!sid || itemId === undefined) return undefined
  return sessions.get(sid)?.tracks.get(itemId)
}

/** Forget everything (tests). */
export function resetReadings() {
  sessions.clear()
  gone.clear()
  notify()
}

function subscribe(l: () => void) {
  listeners.add(l)
  return () => {
    listeners.delete(l)
  }
}

/** A number that changes, at most once per frame, whenever readings arrive; re-render on it to draw them. */
export function usePlotClock(): number {
  return useSyncExternalStore(subscribe, () => version)
}
