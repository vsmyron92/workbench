// How an event of the bus reaches the avatar list in the query cache. The decisions are
// `reduce` (lib/cache.ts); this is the wiring to the cache, kept out of the provider so
// that it can run without React: one event in, the cache updated, the list fetched again
// when the event could not tell it all, and what the cache does not hold handed on.

import type { QueryClient } from '@tanstack/react-query'
import { AVATAR_EVENTS, avatarKeys } from './api'
import { reduce, type ActSignal } from './lib/cache'
import type { AvatarList } from './lib/types'

/**
 * What changes the list: the slice's own events, a saved config, the socket's greeting (the
 * server sends it once this tab is subscribed: what happened between the first answer and
 * then is fetched), a reconnect, and a terminal that is gone. (The nudge is the notifier's.)
 */
export const LIST_EVENTS: readonly string[] = [AVATAR_EVENTS.changed, AVATAR_EVENTS.say, AVATAR_EVENTS.ask, AVATAR_EVENTS.line, AVATAR_EVENTS.act, 'settings.changed', 'hello', 'resync', 'terminal.removed']

export interface EventSinks {
  /** A gesture to play once, or to stop: for the overlay, if one is mounted. */
  act(act: ActSignal): void
  /** A terminal is gone: what this browser remembers of it goes too. */
  removed(terminalId: string): void
}

/** Applies one event to the cached list. Events only ever change the cache; nothing else is fetched. */
export function applyEvent(qc: QueryClient, ev: { type: string; data: unknown; ts: number }, sinks: EventSinks): void {
  const held = qc.getQueryData<AvatarList>(avatarKeys.all)
  const r = reduce(held, ev.type, ev.data, ev.ts)
  if (r.list !== held && r.list !== undefined) qc.setQueryData<AvatarList>(avatarKeys.all, r.list)
  if (r.refetch) {
    // A fetch that is on its way is replaced: its answer could be older than this event. The
    // cache does that by itself once it holds a list; the very first fetch has to be cancelled by hand.
    if (held === undefined) void qc.cancelQueries({ queryKey: avatarKeys.all }).then(() => qc.invalidateQueries({ queryKey: avatarKeys.all }))
    else void qc.invalidateQueries({ queryKey: avatarKeys.all })
  }
  if (r.act) sinks.act(r.act)
  if (r.removed !== null) sinks.removed(r.removed)
}
