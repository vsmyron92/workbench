// One WebSocket per tab on /api/events/ws, fanned out to subscribers by event type.
// Reconnects with backoff; after a reconnect (or a server "lagged" notice) every
// subscriber of the special type "resync" is called, and `installResync` (mounted
// once) refetches every active query. When the device is signed out the server
// closes the socket with CLOSE_SESSION_ENDED and the app shows the login screen.

import { useEffect, useRef } from 'react'
import type { QueryClient, QueryKey } from '@tanstack/react-query'
import { CLOSE_SESSION_ENDED, notifyUnauthorized, wsUrl } from './client'
import type { WbEvent } from './types'

type Handler = (ev: WbEvent) => void

const handlers = new Map<string, Set<Handler>>()
let socket: WebSocket | null = null
let retry = 0
let started = false
let connectedListeners = new Set<(up: boolean) => void>()
let connected = false

function dispatch(ev: WbEvent) {
  handlers.get(ev.type)?.forEach((h) => h(ev))
  handlers.get('*')?.forEach((h) => h(ev))
}

function setConnected(up: boolean) {
  if (connected === up) return
  connected = up
  connectedListeners.forEach((l) => l(up))
}

function connect() {
  socket = new WebSocket(wsUrl('/api/events/ws'))
  let pingTimer: number | undefined
  socket.onopen = () => {
    const wasRetry = retry > 0
    retry = 0
    setConnected(true)
    pingTimer = window.setInterval(() => socket?.readyState === WebSocket.OPEN && socket.send('{"type":"ping"}'), 25_000)
    if (wasRetry) dispatch({ type: 'resync', data: null, ts: Date.now() })
  }
  socket.onmessage = (m) => {
    try {
      const ev = JSON.parse(m.data as string) as WbEvent
      if (ev.type === 'lagged') dispatch({ type: 'resync', data: null, ts: Date.now() })
      else dispatch(ev)
    } catch {
      /* ignore malformed frames */
    }
  }
  socket.onclose = (ev) => {
    window.clearInterval(pingTimer)
    setConnected(false)
    if (ev.code === CLOSE_SESSION_ENDED) {
      // Revoked or signed out elsewhere: stop here; signing in again reloads the page.
      started = false
      notifyUnauthorized()
      return
    }
    retry = Math.min(retry + 1, 8)
    window.setTimeout(connect, Math.min(500 * 2 ** retry, 10_000))
  }
}

/** Start the event socket (idempotent). Called once the user is signed in. */
export function startEvents() {
  if (started) return
  started = true
  connect()
}

export function isEventsConnected() {
  return connected
}

export function onEventsConnection(fn: (up: boolean) => void): () => void {
  connectedListeners.add(fn)
  return () => connectedListeners.delete(fn)
}

/** Subscribe outside React. `type` may be "*" for every event. Returns an unsubscribe. */
export function subscribe(type: string, fn: Handler): () => void {
  let set = handlers.get(type)
  if (!set) handlers.set(type, (set = new Set()))
  set.add(fn)
  return () => set!.delete(fn)
}

/** React hook: call `fn` for every event of `type` (always the latest `fn`). */
export function useEvent<T = unknown>(type: string, fn: (ev: WbEvent<T>) => void) {
  const ref = useRef(fn)
  ref.current = fn
  useEffect(() => subscribe(type, (ev) => ref.current(ev as WbEvent<T>)), [type])
}

/**
 * Invalidate react-query keys when events arrive. `keyFor` returns the key to
 * invalidate for an event (or null to skip). After a reconnect everything is
 * refetched once by `installResync`, not by each caller.
 */
export function useInvalidateOn(qc: QueryClient, types: string[], keyFor: (ev: WbEvent) => QueryKey | null) {
  const ref = useRef(keyFor)
  ref.current = keyFor
  useEffect(() => {
    const offs = types.map((t) =>
      subscribe(t, (ev) => {
        const k = ref.current(ev)
        if (k) qc.invalidateQueries({ queryKey: k })
      }),
    )
    return () => offs.forEach((o) => o())
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [qc, types.join('|')])
}

/** Once per app: a reconnect or a `lagged` notice refetches every active query, once. */
export function installResync(qc: QueryClient): () => void {
  return subscribe('resync', () => void qc.invalidateQueries())
}
