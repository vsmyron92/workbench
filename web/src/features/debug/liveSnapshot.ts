// Fetching the watch list of a session: the Live tab's store and the plot viewer's readings both start from it.

import { useEffect } from 'react'
import { debugApi } from './api'
import { useLive } from './liveStore'
import { PLOT_MAX_READINGS, seed, seedHistory } from './plotBuffer'
import type { DebugSession, LiveSnapshot } from './types'

/** Sessions whose kept readings were fetched (once each: after that, events keep the readings). */
const fetchedHistory = new Set<string>()

/**
 * Fetch the session's watch list and hand it to the Live tab's store and to the plot readings. The first time for a
 * session it also fetches the readings the server kept, so a page that was reloaded (or a plot opened late) starts with
 * the last minutes instead of an empty chart.
 */
export async function refreshLive(pid: string, sid: string, signal?: AbortSignal): Promise<LiveSnapshot> {
  const snap = await debugApi.liveList(pid, sid, signal)
  useLive.getState().load(sid, snap)
  seed(sid, snap)
  if (!fetchedHistory.has(sid)) {
    fetchedHistory.add(sid)
    debugApi
      .liveHistory(pid, sid, { limit: PLOT_MAX_READINGS }, signal)
      .then((h) => seedHistory(sid, h))
      .catch(() => fetchedHistory.delete(sid)) // try again at the next look
  }
  return snap
}

/** Load the watch list when a view of the session opens (events keep it fresh from then on). */
export function useLiveSnapshot(s: Pick<DebugSession, 'projectId' | 'id'> | null | undefined) {
  const pid = s?.projectId
  const sid = s?.id
  useEffect(() => {
    if (!pid || !sid) return
    const ctl = new AbortController()
    refreshLive(pid, sid, ctl.signal).catch(() => {})
    return () => ctl.abort()
  }, [pid, sid])
}
