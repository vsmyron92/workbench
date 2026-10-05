// What the Live tab shows, by session: the watched expressions and the recent readings of each
// (`debug.live` events and a snapshot when the tab opens). Pure reducers, so they can be tested.

import { create } from 'zustand'
import { pushSample } from './logic'
import type { LiveEvent, LiveSample, LiveSnapshot } from './types'

export interface LiveSession {
  items: LiveSnapshot['items']
  intervalMs: number
  /** Newest last, by item id. */
  samples: Record<number, LiveSample[]>
  /** The snapshot arrived (before it, events may have come in for items not known yet). */
  loaded: boolean
  /** Reading by pausing the program is allowed (only meaningful for a session whose `liveMode` is `pausing`). */
  pausing: boolean
  /** How long a round of reading keeps the program stopped (average, ms); null until one has run. */
  pauseMs: number | null
}

export const emptyLive = (): LiveSession => ({ items: [], intervalMs: 250, samples: {}, loaded: false, pausing: false, pauseMs: null })

/** Readings for the items that are still watched. */
function keep(samples: LiveSession['samples'], items: LiveSession['items']): LiveSession['samples'] {
  const ids = new Set(items.map((i) => i.id))
  return Object.fromEntries(Object.entries(samples).filter(([id]) => ids.has(Number(id))))
}

/** The session after a snapshot: the list and the interval replaced, history kept, and an item without any
 *  history started from its last reading. */
export function loadSnapshot(cur: LiveSession | undefined, snap: LiveSnapshot): LiveSession {
  const base = cur ?? emptyLive()
  const samples = keep(base.samples, snap.items)
  for (const item of snap.items) {
    const last = snap.last[String(item.id)]
    if (last && !samples[item.id]?.length) samples[item.id] = [last]
  }
  return { items: snap.items, intervalMs: snap.intervalMs, samples, loaded: true, pausing: !!snap.pausing, pauseMs: snap.pauseMs ?? null }
}

export function applyLive(cur: LiveSession | undefined, ev: LiveEvent): LiveSession {
  let next = cur ?? emptyLive()
  if (ev.items) next = { ...next, items: ev.items, samples: keep(next.samples, ev.items) }
  if (ev.intervalMs !== undefined) next = { ...next, intervalMs: ev.intervalMs }
  if (ev.pausing !== undefined) next = { ...next, pausing: ev.pausing }
  if (ev.pauseMs !== undefined) next = { ...next, pauseMs: ev.pauseMs }
  if (ev.samples?.length) {
    const samples = { ...next.samples }
    for (const s of ev.samples) {
      // A reading of an item the list does not have (a removal that is still on its way) is dropped, once the list is known.
      if (next.loaded && !next.items.some((i) => i.id === s.id)) continue
      samples[s.id] = pushSample(samples[s.id], s)
    }
    next = { ...next, samples }
  }
  return next
}

interface LiveState {
  sessions: Record<string, LiveSession>
  load: (sid: string, snap: LiveSnapshot) => void
  apply: (ev: LiveEvent) => void
  forget: (sid: string) => void
}

export const useLive = create<LiveState>()((set) => ({
  sessions: {},
  load: (sid, snap) => set((st) => ({ sessions: { ...st.sessions, [sid]: loadSnapshot(st.sessions[sid], snap) } })),
  apply: (ev) => set((st) => ({ sessions: { ...st.sessions, [ev.sessionId]: applyLive(st.sessions[ev.sessionId], ev) } })),
  forget: (sid) =>
    set((st) => {
      if (!(sid in st.sessions)) return st
      const { [sid]: _gone, ...rest } = st.sessions
      return { sessions: rest }
    }),
}))
