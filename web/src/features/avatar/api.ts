// REST calls, the one query and the event payloads of the avatar slice
// (server/src/avatar/routes.rs). Only the device routes: what a signed-in browser may read
// and do. The session's own side (design, say, ask, act) is MCP tools and hook routes, and
// nothing here reaches them.
//
// Everything goes through `api` (cookie, device key, `ApiError`). The one exception is
// giving a lease back: that request must still leave while the page goes away, so it is a
// `fetch` with `keepalive` and the same device key header.

import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'
import { ApiError, api, getDeviceKey } from '@/api/client'
import type { ArbiterHost } from './lib/arbiter'
import { mergeFetched } from './lib/cache'
import type { Ask, AvatarInfo, AvatarList, Line, Outcome } from './lib/types'

const enc = encodeURIComponent
const of = (terminalId: string) => `/api/avatar/${enc(terminalId)}`
const lineOf = (terminalId: string, lineId: string) => `${of(terminalId)}/lines/${enc(lineId)}`

/** The feature's one query: `GET /api/avatar`. Events only update this cache; nothing else is fetched. */
export const avatarKeys = {
  all: ['avatar'] as const,
}

/** `POST …/claim`: whether this tab may say the line, for how long, and when to ask again if not. */
export interface ClaimAnswer {
  speak: boolean
  leaseMs: number
  retryMs: number
}

/** What a browser may report of a line. `unheard` is the server's own word for a line nobody acknowledged. */
export type HeardOutcome = Exclude<Outcome, 'unheard'>

/**
 * `POST …/reply`. With `askId` it answers exactly that question (`choice`: the index into
 * its choices, from 0; or `text`); without one the text is a message to the session.
 */
export interface ReplyRequest {
  askId?: string
  text?: string
  choice?: number
  via: 'click' | 'typed' | 'voice'
}

/** `answer`: a waiting call got it. `prompt`: it was pasted into the session as its next message. */
export interface ReplyAnswer {
  delivered: 'answer' | 'prompt'
}

/** `POST …/on`. `briefed: false`: the avatar is on, but the session could not be told now (it shows a dialog or is starting). */
export type TurnedOn = AvatarInfo & { briefed?: boolean }

/**
 * A POST that still leaves when the page is going away. `api.post` cannot ask for
 * `keepalive`; the device key travels in the same header as there.
 */
async function postLeaving<T>(path: string, body: unknown): Promise<T> {
  const key = getDeviceKey()
  const headers: Record<string, string> = { 'Content-Type': 'application/json' }
  if (key) headers['X-Workbench-Key'] = key
  const res = await fetch(path, { method: 'POST', credentials: 'same-origin', keepalive: true, headers, body: JSON.stringify(body) })
  const text = await res.text()
  let data: unknown = undefined
  if (text) {
    try {
      data = JSON.parse(text)
    } catch {
      data = text
    }
  }
  if (!res.ok) {
    const err = (data as { error?: { code?: string; message?: string } } | undefined)?.error
    throw new ApiError(res.status, err?.code ?? 'http_' + res.status, err?.message ?? res.statusText)
  }
  return data as T
}

export const avatarApi = {
  /** Every avatar, whether avatars are enabled, and the sessions that asked for one. */
  list: (signal?: AbortSignal) => api.get<AvatarList>('/api/avatar', undefined, signal),
  /** One session's avatar; 404 (`not_found`) when it has none. */
  get: (terminalId: string, signal?: AbortSignal) => api.get<AvatarInfo>(of(terminalId), undefined, signal),
  /**
   * Asks for the lease of a line, or with `release: true` gives it back. The release is
   * sent so that it survives the page (`pagehide`, sign-out).
   */
  claim: (terminalId: string, lineId: string, body: { tab: string; release?: true }): Promise<ClaimAnswer> => {
    const path = `${lineOf(terminalId, lineId)}/claim`
    return body.release === true ? postLeaving<ClaimAnswer>(path, { tab: body.tab, release: true }) : api.post<ClaimAnswer>(path, { tab: body.tab })
  },
  /** What became of a line in this browser; `voice`: the browser voice that said it. The server keeps the best outcome. */
  heard: (terminalId: string, lineId: string, outcome: HeardOutcome, voice?: string) => api.post<{ ok: true }>(`${lineOf(terminalId, lineId)}/heard`, voice === undefined ? { outcome } : { outcome, voice }),
  /** An answer to the open question, or a message. May take some seconds: the server waits for a pause in the session's typing. */
  reply: (terminalId: string, body: ReplyRequest) => api.post<ReplyAnswer>(`${of(terminalId)}/reply`, body),
  /** Closes the open question unanswered. */
  dismissAsk: (terminalId: string, askId: string) => api.post<{ ok: true }>(`${of(terminalId)}/asks/${enc(askId)}/dismiss`, {}),
  /** Gives the session an avatar, or turns its avatar on again. */
  on: (terminalId: string) => api.post<TurnedOn>(`${of(terminalId)}/on`, {}),
  /** Off on every device; the look is kept. */
  off: (terminalId: string) => api.post<AvatarInfo>(`${of(terminalId)}/off`, {}),
  /** Back to the default look. */
  reset: (terminalId: string) => api.post<AvatarInfo>(`${of(terminalId)}/reset`, {}),
}

/**
 * The arbiter's two requests (`ArbiterHost.claim` and `.heard`), ready to be spread into
 * its host. `unheard` is never a browser's to report, so it is not sent.
 */
export const arbiterCalls: Pick<ArbiterHost, 'claim' | 'heard'> = {
  claim: (terminalId, lineId, body) => avatarApi.claim(terminalId, lineId, body),
  heard: async (terminalId, lineId, outcome, voice) => {
    if (outcome === 'unheard') return
    await avatarApi.heard(terminalId, lineId, outcome, voice)
  },
}

/**
 * What the cache holds after a `GET /api/avatar` answered. A fetched record that is older
 * than the one held (`updatedAt`) is ignored: the answer was already on its way when
 * something newer arrived. Whether an avatar exists at all is the answer's to say.
 *
 * A record that events changed since it was fetched is weighed part by part instead
 * (`mergeFetched` in lib/cache.ts): the design is always the answer's, and of its lines,
 * question and held gesture only what no event told more recently.
 */
export function takeFetched(held: AvatarList | undefined, fetched: AvatarList): AvatarList {
  return mergeFetched(held, fetched)
}

/**
 * The feature's query, for `useQuery` and for a provider that fetches by hand. The cache
 * is read when the answer arrives, not when the request leaves: what events changed
 * meanwhile is what `takeFetched` has to weigh the answer against. The always-loaded
 * provider keeps the cache fresh from events, so it is never stale by age.
 */
export function avatarsQuery(qc: Pick<QueryClient, 'getQueryData'>) {
  return {
    queryKey: avatarKeys.all,
    queryFn: async ({ signal }: { signal?: AbortSignal }): Promise<AvatarList> => {
      const fetched = await avatarApi.list(signal)
      return takeFetched(qc.getQueryData<AvatarList>(avatarKeys.all), fetched)
    },
    staleTime: Infinity,
  }
}

/** Every avatar (`avatarKeys.all`). */
export function useAvatars() {
  return useQuery(avatarsQuery(useQueryClient()))
}

// ---------------------------------------------------------------- events (the `data` of each)

/** The event types of the slice, as `subscribe` and `useEvent` take them. */
export const AVATAR_EVENTS = {
  changed: 'avatar.changed',
  say: 'avatar.say',
  ask: 'avatar.ask',
  line: 'avatar.line',
  act: 'avatar.act',
  nudge: 'avatar.nudge',
} as const

/** `avatar.changed`: the record's design, name, voice, on/off or existence changed. Invalidates `avatarKeys.all`. */
export interface AvatarChanged {
  terminalId: string
  revision: number
  on: boolean
  removed?: true
}

/** `avatar.say`: present this line now. With `ask`: the line is that question. */
export interface AvatarSay {
  terminalId: string
  line: Line
  ask?: Ask
}

export type AskChange = 'opened' | 'waiter_left' | 'waiting' | 'answered' | 'dismissed' | 'superseded' | 'withdrawn' | 'session_ended' | 'moved_on'

/** `avatar.ask`: the open question changed. `ask` is what is open now, or null. */
export interface AvatarAsked {
  terminalId: string
  change: AskChange
  ask: Ask | null
}

/** `avatar.line`: a line changed without being said (its outcome, a reply of the user). */
export interface AvatarLineChanged {
  terminalId: string
  line: Line
}

/** `avatar.act`: play a gesture, or stop one (`stop`: a name or `all`). At most four a second for one session. */
export interface AvatarAct {
  terminalId: string
  play?: string
  mode?: 'once' | 'hold' | 'loop'
  times?: number
  stop?: string
  seq: number
}

/** `avatar.nudge`: for the server's notifier only. Browsers ignore it. */
export interface AvatarNudge {
  terminalId: string
  askId: string
  name: string
  question: string
}
