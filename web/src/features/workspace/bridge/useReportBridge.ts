// Answers the `workbench:query` messages of one report frame (see `kinds.ts` for what that may read).
//
// A report asks; nothing is read until the user allowed that card. Requests that come before the answer
// wait, so the report's `await workbench.query(…)` simply resolves once the user clicks Allow. A card of a
// repository, and one outside a project (Home, the Sandbox), has no live data to give.

import { useCallback, useEffect, useRef, useState, type RefObject } from 'react'
import { api, ApiError } from '@/api/client'
import { isProjectless } from '../logic'
import type { WorkspaceCard } from '../api'
import { BridgeError, parseRequest, runQuery, type BridgeCode, type BridgeIo, type BridgeRequest } from './kinds'
import { bridgeKey, useBridgePrefs } from './store'

const io: BridgeIo = {
  get: (path, query) => api.get(path, query),
  post: (path, body) => api.post(path, body),
}

/** Requests answered at once, and waiting for the user's decision. */
const MAX_IN_FLIGHT = 6
const MAX_WAITING = 16
/** Answers per second a frame may ask for. */
const MAX_PER_SECOND = 20

export type BridgeState = 'unavailable' | 'off' | 'asking' | 'declined' | 'allowed'

export interface ReportBridge {
  state: BridgeState
  /** Why a card has no live data (`state === 'unavailable'`). */
  reason: string | null
  allow: () => void
  /** Say no for now: the report's requests are refused until it is reloaded. */
  decline: () => void
  revoke: () => void
}

/** The project whose data a card's report may read, or why it has none. */
export function bridgeScope(card: Pick<WorkspaceCard, 'origin' | 'scope'> | undefined): { projectId: string } | { reason: string } {
  if (!card) return { reason: 'This page is not a card of a project' }
  if (card.origin !== 'workbench') return { reason: 'Cards that come from a repository are never given live data' }
  if (isProjectless(card.scope)) return { reason: 'Live data belongs to a project: this card is in Home or the Sandbox' }
  return { projectId: card.scope }
}

export function useReportBridge(frameRef: RefObject<HTMLIFrameElement | null>, card: WorkspaceCard | undefined, resetKey: unknown): ReportBridge {
  const scope = bridgeScope(card)
  const key = card ? bridgeKey(card.scope, card.id) : ''
  const allowedInStore = useBridgePrefs((s) => !!key && !!s.allowed[key])
  const allowInStore = useBridgePrefs((s) => s.allow)
  const revokeInStore = useBridgePrefs((s) => s.revoke)
  const [asking, setAsking] = useState(false)
  const [declined, setDeclined] = useState(false)
  const waiting = useRef<BridgeRequest[]>([])
  const inFlight = useRef(0)
  const recent = useRef<number[]>([])
  const live = useRef({ scope, allowed: allowedInStore, declined: false })
  live.current = { scope, allowed: allowedInStore, declined }

  const reply = useCallback(
    (target: Window | null, id: number, result: { ok: true; data: unknown } | { ok: false; code: BridgeCode; message: string }) => {
      // The frame may have gone or moved on: only the window we framed is ever answered.
      if (!target || frameRef.current?.contentWindow !== target) return
      target.postMessage({ type: 'workbench:result', id, ...result }, '*')
    },
    [frameRef],
  )

  const answer = useCallback(
    async (source: Window, req: BridgeRequest) => {
      const s = live.current.scope
      if (!('projectId' in s)) return reply(source, req.id, { ok: false, code: 'unavailable', message: s.reason })
      inFlight.current += 1
      try {
        const data = await runQuery(req.kind, req.params, { projectId: s.projectId }, io)
        reply(source, req.id, { ok: true, data })
      } catch (e) {
        if (e instanceof BridgeError) reply(source, req.id, { ok: false, code: e.code, message: e.message })
        else reply(source, req.id, { ok: false, code: 'failed', message: e instanceof ApiError ? e.message.slice(0, 300) : 'the request failed' })
      } finally {
        inFlight.current -= 1
      }
    },
    [reply],
  )

  // A new load of the report starts over: nothing is waiting for a frame that is gone.
  useEffect(() => {
    waiting.current = []
    setAsking(false)
    setDeclined(false)
  }, [resetKey])

  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      const frame = frameRef.current
      if (!frame || e.source !== frame.contentWindow || !e.source) return
      const source = e.source as Window
      const req = parseRequest(e.data)
      if (!req) return
      if ('invalid' in req) {
        if (req.id !== null) reply(source, req.id, { ok: false, code: 'bad_request', message: req.invalid })
        return
      }
      const cur = live.current
      if (!('projectId' in cur.scope)) return reply(source, req.id, { ok: false, code: 'unavailable', message: cur.scope.reason })
      if (cur.declined) return reply(source, req.id, { ok: false, code: 'denied', message: 'the user did not allow this report to read live data' })
      const now = Date.now()
      recent.current = recent.current.filter((t) => now - t < 1000)
      if (recent.current.length >= MAX_PER_SECOND || inFlight.current >= MAX_IN_FLIGHT) {
        return reply(source, req.id, { ok: false, code: 'busy', message: 'too many requests: ask less often' })
      }
      recent.current.push(now)
      if (!cur.allowed) {
        if (waiting.current.length >= MAX_WAITING) return reply(source, req.id, { ok: false, code: 'busy', message: 'waiting for the user: too many requests' })
        waiting.current.push(req)
        setAsking(true)
        return
      }
      void answer(source, req)
    }
    window.addEventListener('message', onMessage)
    return () => window.removeEventListener('message', onMessage)
  }, [frameRef, reply, answer])

  const settle = useCallback(
    (allowed: boolean) => {
      const queue = waiting.current
      waiting.current = []
      setAsking(false)
      const source = frameRef.current?.contentWindow
      if (!source) return
      for (const req of queue) {
        if (allowed) void answer(source, req)
        else reply(source, req.id, { ok: false, code: 'denied', message: 'the user did not allow this report to read live data' })
      }
    },
    [frameRef, answer, reply],
  )

  const allow = useCallback(() => {
    if (!key) return
    allowInStore(key)
    setDeclined(false)
    live.current.allowed = true
    settle(true)
  }, [key, allowInStore, settle])
  const decline = useCallback(() => {
    setDeclined(true)
    live.current.declined = true
    settle(false)
  }, [settle])
  const revoke = useCallback(() => {
    if (key) revokeInStore(key)
    live.current.allowed = false
    setDeclined(false)
  }, [key, revokeInStore])

  const state: BridgeState = 'reason' in scope ? 'unavailable' : allowedInStore ? 'allowed' : asking ? 'asking' : declined ? 'declined' : 'off'
  return { state, reason: 'reason' in scope ? scope.reason : null, allow, decline, revoke }
}
