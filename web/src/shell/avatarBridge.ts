// What the avatar feature tells the agents feature, so the agents feature imports no
// feature: whether a session's avatar is on screen in this window (its "finished" toast
// would repeat what the avatar already shows), and which sessions have an avatar that is
// on (the badge on the session card and the session header follows the live record).
// The avatar feature registers both; the agents feature asks.

import { useSyncExternalStore } from 'react'

type Shows = (terminalId: string) => boolean

let shows: Shows | null = null

/** Called by the avatar feature's provider; `null` when it goes away. */
export function setAvatarShows(fn: Shows | null) {
  shows = fn
}

/** Whether this session's avatar is shown here now. False while nothing is registered, and when the answer fails. */
export function avatarShows(terminalId: string): boolean {
  if (!shows) return false
  try {
    return shows(terminalId) === true
  } catch (e) {
    // The caller then tells the user itself, as it does for a session without an avatar.
    console.error('[workbench] the avatar bridge failed', e)
    return false
  }
}

/** The sessions whose avatar is on; null: not known (the list has not loaded, or the feature is gone). */
let on: ReadonlySet<string> | null = null
const watchers = new Set<() => void>()

function sameIds(a: ReadonlySet<string> | null, b: ReadonlySet<string> | null): boolean {
  if (a === null || b === null) return a === b
  if (a.size !== b.size) return false
  for (const id of a) if (!b.has(id)) return false
  return true
}

/**
 * Called by the avatar feature's provider on every change of its list: the terminal ids of
 * the sessions whose avatar is on. `null` while the list is unknown, and when the feature
 * goes away. Nobody is told when nothing differs.
 */
export function setAvatarsOn(ids: ReadonlySet<string> | null) {
  let next: ReadonlySet<string> | null = null
  if (ids !== null && typeof ids === 'object' && typeof ids[Symbol.iterator] === 'function') {
    // A copy: what the caller does with its own set afterwards changes nothing here.
    const copy = new Set<string>()
    for (const id of ids) if (typeof id === 'string') copy.add(id)
    next = copy
  }
  if (sameIds(on, next)) return
  on = next
  // A copy: a watcher may leave, or another join, while they are told.
  for (const watcher of Array.from(watchers)) {
    try {
      watcher()
    } catch (e) {
      console.error('[workbench] the avatar bridge failed', e)
    }
  }
}

/** Whether this session's avatar is on, as the live record says. `null`: not known, and the caller falls back to what the session was started with. */
export function avatarOn(terminalId: string): boolean | null {
  return on === null ? null : on.has(terminalId)
}

/** Calls `changed` whenever `avatarOn` may answer differently. Returns the function that stops it. */
export function watchAvatarsOn(changed: () => void): () => void {
  watchers.add(changed)
  return () => void watchers.delete(changed)
}

/** `avatarOn(terminalId)` as a hook: the component renders again when the answer changes. */
export function useAvatarOn(terminalId: string): boolean | null {
  const read = () => avatarOn(terminalId)
  return useSyncExternalStore(watchAvatarsOn, read, read)
}
