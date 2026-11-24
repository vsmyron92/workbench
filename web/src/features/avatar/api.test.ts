// The device routes as the browser calls them: paths, bodies, the device key header, and
// the one request that must survive the page. `fetch` only records here.

import { QueryClient } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError, setDeviceKey } from '@/api/client'
import { AVATAR_EVENTS, arbiterCalls, avatarApi, avatarKeys, avatarsQuery, takeFetched } from './api'
import type { AvatarInfo, AvatarList } from './lib/types'

interface Call {
  url: string
  method: string
  headers: Record<string, string>
  body: unknown
  keepalive: boolean
  credentials: string | undefined
}

let calls: Call[] = []
let answer: { status: number; body: unknown } = { status: 200, body: { ok: true } }

beforeEach(() => {
  calls = []
  answer = { status: 200, body: { ok: true } }
  setDeviceKey('device-key-1')
  vi.stubGlobal('fetch', async (url: string, init: RequestInit = {}) => {
    calls.push({
      url: String(url),
      method: init.method ?? 'GET',
      headers: { ...(init.headers as Record<string, string>) },
      body: typeof init.body === 'string' ? JSON.parse(init.body) : init.body,
      keepalive: init.keepalive === true,
      credentials: init.credentials,
    })
    return new Response(typeof answer.body === 'string' ? answer.body : JSON.stringify(answer.body), { status: answer.status })
  })
})

afterEach(() => {
  setDeviceKey(null)
  vi.unstubAllGlobals()
})

const sent = () => calls.map((c) => `${c.method} ${c.url}`)

describe('the device routes', () => {
  it('reads the list and one avatar', async () => {
    answer.body = { enabled: true, sessions: 'asked', avatars: [], asked: ['t9'] }
    expect(await avatarApi.list()).toEqual({ enabled: true, sessions: 'asked', avatars: [], asked: ['t9'] })
    await avatarApi.get('ab12cd34ef56')
    expect(sent()).toEqual(['GET /api/avatar', 'GET /api/avatar/ab12cd34ef56'])
    expect(calls[0].body).toBeUndefined()
    expect(avatarKeys.all).toEqual(['avatar'])
  })

  it('asks for a line with its tab id', async () => {
    answer.body = { speak: true, leaseMs: 16_000, retryMs: 0 }
    expect(await avatarApi.claim('t1', 'Lab12-3', { tab: 'tab-A' })).toEqual({ speak: true, leaseMs: 16_000, retryMs: 0 })
    expect(calls).toHaveLength(1)
    expect(calls[0]).toMatchObject({ method: 'POST', url: '/api/avatar/t1/lines/Lab12-3/claim', body: { tab: 'tab-A' }, keepalive: false, credentials: 'same-origin' })
    expect(calls[0].headers).toMatchObject({ 'X-Workbench-Key': 'device-key-1', 'Content-Type': 'application/json' })
  })

  it('gives a lease back with a request that survives the page, and the device key', async () => {
    answer.body = { speak: false, leaseMs: 0, retryMs: 0 }
    expect(await avatarApi.claim('t1', 'Lab12-3', { tab: 'tab-A', release: true })).toEqual({ speak: false, leaseMs: 0, retryMs: 0 })
    expect(calls).toHaveLength(1)
    expect(calls[0]).toEqual({
      url: '/api/avatar/t1/lines/Lab12-3/claim',
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Workbench-Key': 'device-key-1' },
      body: { tab: 'tab-A', release: true },
      keepalive: true,
      credentials: 'same-origin',
    })
    // A browser that was never given a key sends none, and the server refuses: there is nothing to make up.
    setDeviceKey(null)
    await avatarApi.claim('t1', 'Lab12-3', { tab: 'tab-A', release: true })
    expect(calls[1].headers).toEqual({ 'Content-Type': 'application/json' })
  })

  it('turns a refused release into an ApiError like any other call, and a refusal that is no JSON too', async () => {
    answer = { status: 401, body: { error: { code: 'unauthorized', message: 'sign in first' } } }
    await expect(avatarApi.claim('t1', 'L1', { tab: 'x', release: true })).rejects.toMatchObject({ status: 401, code: 'unauthorized', message: 'sign in first' })
    answer = { status: 502, body: 'Bad Gateway' }
    const failed = await avatarApi.claim('t1', 'L1', { tab: 'x', release: true }).catch((e: unknown) => e)
    expect(failed).toBeInstanceOf(ApiError)
    expect(failed).toMatchObject({ status: 502, code: 'http_502' })
  })

  it('reports what became of a line, with the voice that said it', async () => {
    await avatarApi.heard('t1', 'L1', 'spoken', 'Daniel')
    await avatarApi.heard('t1', 'L2', 'shown')
    await avatarApi.heard('t1', 'L3', 'hidden')
    expect(calls.map((c) => [c.method, c.url, c.body])).toEqual([
      ['POST', '/api/avatar/t1/lines/L1/heard', { outcome: 'spoken', voice: 'Daniel' }],
      ['POST', '/api/avatar/t1/lines/L2/heard', { outcome: 'shown' }],
      ['POST', '/api/avatar/t1/lines/L3/heard', { outcome: 'hidden' }],
    ])
    expect(calls.every((c) => c.headers['X-Workbench-Key'] === 'device-key-1' && !c.keepalive)).toBe(true)
  })

  it('sends an answer bound to the question on screen, and a message without one', async () => {
    answer.body = { delivered: 'answer' }
    expect(await avatarApi.reply('t1', { askId: 'Qab12-1', choice: 0, via: 'click' })).toEqual({ delivered: 'answer' })
    await avatarApi.reply('t1', { askId: 'Qab12-1', text: 'Not yet', via: 'typed' })
    await avatarApi.reply('t1', { askId: 'Qab12-1', text: 'no', via: 'voice' })
    await avatarApi.reply('t1', { text: 'Stop after this step.', via: 'typed' })
    expect(calls.map((c) => c.url)).toEqual(Array.from({ length: 4 }, () => '/api/avatar/t1/reply'))
    expect(calls.map((c) => c.body)).toEqual([
      // The first choice is 0: it must not be lost as "nothing".
      { askId: 'Qab12-1', choice: 0, via: 'click' },
      { askId: 'Qab12-1', text: 'Not yet', via: 'typed' },
      { askId: 'Qab12-1', text: 'no', via: 'voice' },
      { text: 'Stop after this step.', via: 'typed' },
    ])
  })

  it('shows the server’s reason when a reply is refused', async () => {
    answer = { status: 409, body: { error: { code: 'not_pending', message: 'that question is no longer open' } } }
    const refused = await avatarApi.reply('t1', { askId: 'Q1', text: 'yes', via: 'typed' }).catch((e: unknown) => e)
    expect(refused).toBeInstanceOf(ApiError)
    expect(refused).toMatchObject({ status: 409, code: 'not_pending', message: 'that question is no longer open' })
  })

  it('dismisses a question, and turns an avatar on, off and back to the default look', async () => {
    await avatarApi.dismissAsk('t1', 'Qab12-1')
    await avatarApi.on('t1')
    await avatarApi.off('t1')
    await avatarApi.reset('t1')
    expect(sent()).toEqual(['POST /api/avatar/t1/asks/Qab12-1/dismiss', 'POST /api/avatar/t1/on', 'POST /api/avatar/t1/off', 'POST /api/avatar/t1/reset'])
    expect(calls.map((c) => c.body)).toEqual([{}, {}, {}, {}])
  })

  it('never lets an id change the path', async () => {
    await avatarApi.get('../agents')
    await avatarApi.heard('a/b', 'L 1?x', 'shown')
    await avatarApi.dismissAsk('t1', 'Q#1')
    expect(calls.map((c) => c.url)).toEqual(['/api/avatar/..%2Fagents', '/api/avatar/a%2Fb/lines/L%201%3Fx/heard', '/api/avatar/t1/asks/Q%231/dismiss'])
  })
})

describe('the arbiter’s two requests', () => {
  it('are the claim and heard routes', async () => {
    answer.body = { speak: true, leaseMs: 1, retryMs: 0 }
    expect(await arbiterCalls.claim('t1', 'L1', { tab: 'tab-A' })).toEqual({ speak: true, leaseMs: 1, retryMs: 0 })
    await arbiterCalls.claim('t1', 'L1', { tab: 'tab-A', release: true })
    expect(await arbiterCalls.heard('t1', 'L1', 'spoken', 'Daniel')).toBeUndefined()
    await arbiterCalls.heard('t1', 'L1', 'skipped')
    expect(calls.map((c) => [c.url.split('/').pop(), c.keepalive, c.body])).toEqual([
      ['claim', false, { tab: 'tab-A' }],
      ['claim', true, { tab: 'tab-A', release: true }],
      ['heard', false, { outcome: 'spoken', voice: 'Daniel' }],
      ['heard', false, { outcome: 'skipped' }],
    ])
  })

  it('never report "unheard": that is the server’s word', async () => {
    await arbiterCalls.heard('t1', 'L1', 'unheard')
    expect(calls).toEqual([])
  })
})

const record = (terminalId: string, updatedAt: number, name = ''): AvatarInfo => ({ terminalId, projectId: 'p1', on: true, name, designed: false, revision: 1, look: null, voice: {}, via: 'tools', held: null, ask: null, lines: [], createdAt: 1, updatedAt })
const list = (avatars: AvatarInfo[], over: Partial<AvatarList> = {}): AvatarList => ({ enabled: true, sessions: 'asked', avatars, asked: [], ...over })

describe('takeFetched', () => {
  it('takes the answer when nothing is held, and when nothing held is newer', () => {
    const fetched = list([record('t1', 5)])
    expect(takeFetched(undefined, fetched)).toBe(fetched)
    expect(takeFetched(list([record('t1', 5, 'old'), record('t2', 9)]), fetched)).toBe(fetched)
  })

  it('ignores a fetched record that is older than the one it holds', () => {
    const newer = record('t1', 9, 'Pip')
    const out = takeFetched(list([newer, record('t3', 4)]), list([record('t1', 5), record('t2', 7), record('t3', 4, 'same age')], { asked: ['t8'], enabled: false }))
    expect(out.avatars[0]).toBe(newer)
    expect(out.avatars.map((a) => [a.terminalId, a.updatedAt, a.name])).toEqual([['t1', 9, 'Pip'], ['t2', 7, ''], ['t3', 4, 'same age']])
    // Everything else is the answer's: the settings, who asked, and which avatars exist at all.
    expect(out).toMatchObject({ enabled: false, asked: ['t8'] })
    expect(takeFetched(list([record('gone', 99)]), list([])).avatars).toEqual([])
  })

  it('copes with what is no list', () => {
    const fetched = list([record('t1', 5)])
    expect(takeFetched({ avatars: 'none' } as unknown as AvatarList, fetched)).toBe(fetched)
    const odd = { enabled: true } as unknown as AvatarList
    expect(takeFetched(list([]), odd)).toBe(odd)
    expect(takeFetched(list([null as unknown as AvatarInfo, record('t1', 9)]), fetched).avatars[0].updatedAt).toBe(9)
  })
})

describe('the query', () => {
  it('is the list under the key ["avatar"], never stale by age', async () => {
    const qc = new QueryClient()
    const query = avatarsQuery(qc)
    expect(query.queryKey).toEqual(['avatar'])
    expect(query.staleTime).toBe(Infinity)
    answer.body = list([record('t1', 5)])
    expect(await qc.fetchQuery(query)).toEqual(list([record('t1', 5)]))
    expect(sent()).toEqual(['GET /api/avatar'])
    expect(qc.getQueryData(avatarKeys.all)).toEqual(list([record('t1', 5)]))
  })

  it('weighs the answer against what the cache holds when it arrives, not when the request left', async () => {
    const qc = new QueryClient()
    let respond: ((r: Response) => void) | null = null
    vi.stubGlobal('fetch', () => new Promise<Response>((resolve) => (respond = resolve)))
    const fetching = qc.fetchQuery(avatarsQuery(qc))
    await new Promise((r) => setTimeout(r, 0))
    expect(respond).not.toBeNull()
    // The request is on its way when an event changes a record (and the provider writes it into the cache).
    qc.setQueryData(avatarKeys.all, list([record('t1', 9, 'said meanwhile')]))
    ;(respond as unknown as (r: Response) => void)(new Response(JSON.stringify(list([record('t1', 5, 'as fetched'), record('t2', 7)]))))
    const got = await fetching
    expect(got.avatars.map((a) => [a.terminalId, a.updatedAt, a.name])).toEqual([['t1', 9, 'said meanwhile'], ['t2', 7, '']])
    expect(qc.getQueryData(avatarKeys.all)).toEqual(got)
  })
})

describe('the events', () => {
  it('are named as the server emits them', () => {
    expect(AVATAR_EVENTS).toEqual({ changed: 'avatar.changed', say: 'avatar.say', ask: 'avatar.ask', line: 'avatar.line', act: 'avatar.act', nudge: 'avatar.nudge' })
  })
})
