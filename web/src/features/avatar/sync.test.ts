// Events and fetches against a real query cache: an event that arrives while a fetch is
// on its way, a fetch that is older than what events told, and what is handed on to the
// overlay and the preferences. `fetch` is a stand-in that answers when the test says so.

import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { avatarKeys, avatarsQuery } from './api'
import type { ActSignal } from './lib/cache'
import type { Ask, AvatarInfo, AvatarList, Line, Look } from './lib/types'
import { LIST_EVENTS, applyEvent } from './sync'

const line = (id: string, at: number, over: Partial<Line> = {}): Line => ({ id, at, from: 'agent', text: `line ${id}`, ...over })
const ask = (id: string, lineId: string): Ask => ({ id, lineId, text: 'Shall I push?', choices: ['Yes', 'No'], freeText: true, askedAt: 1, waiting: true })
const record = (terminalId: string, over: Partial<AvatarInfo> = {}): AvatarInfo => ({ terminalId, projectId: 'p1', on: true, name: '', designed: false, revision: 0, look: null, voice: {}, via: 'tools', held: null, ask: null, lines: [], createdAt: 1, updatedAt: 10, ...over })
const list = (avatars: AvatarInfo[], over: Partial<AvatarList> = {}): AvatarList => ({ enabled: true, sessions: 'asked', avatars, asked: [], ...over })
const look = { viewBox: [0, 0, 200, 200], nodes: [], pivots: {}, roles: {}, animations: {}, states: {}, mouth: [], gaze: null } as unknown as Look

/** Requests that wait for the test to answer them. */
let waiting: ((body: AvatarList) => void)[] = []
let requests = 0
let qc: QueryClient
let stop: () => void = () => {}
let acts: ActSignal[] = []
let removed: string[] = []
const sinks = { act: (a: ActSignal) => void acts.push(a), removed: (id: string) => void removed.push(id) }
const event = (type: string, data: unknown, ts: number) => applyEvent(qc, { type, data, ts }, sinks)
const held = () => qc.getQueryData<AvatarList>(avatarKeys.all)
const of = (id: string) => held()?.avatars.find((a) => a.terminalId === id)
const flush = async () => {
  for (let i = 0; i < 6; i++) await new Promise((r) => setTimeout(r, 0))
}
/** Answers the oldest request that is still waiting. */
const answer = async (body: AvatarList) => {
  const next = waiting.shift()
  if (!next) throw new Error('no request is waiting')
  next(body)
  await flush()
}

beforeEach(async () => {
  waiting = []
  requests = 0
  acts = []
  removed = []
  vi.stubGlobal('fetch', (_url: string, init: { signal?: AbortSignal } = {}) => {
    requests++
    return new Promise<Response>((resolve, reject) => {
      const respond = (body: AvatarList) => resolve(new Response(JSON.stringify(body), { status: 200 }))
      waiting.push(respond)
      // A fetch that is replaced is aborted: its answer never arrives.
      init.signal?.addEventListener('abort', () => {
        waiting = waiting.filter((w) => w !== respond)
        reject(new DOMException('aborted', 'AbortError'))
      })
    })
  })
  qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  // The provider's query, with an observer as `useAvatars` is one: invalidating fetches again.
  const observer = new QueryObserver(qc, avatarsQuery(qc))
  stop = observer.subscribe(() => {})
  await flush()
})

afterEach(() => {
  stop()
  qc.clear()
  vi.unstubAllGlobals()
})

describe('applyEvent', () => {
  it('listens to the events of the slice, a saved config, the socket’s greeting, a reconnect and a removed terminal, and not to the nudge', () => {
    expect([...LIST_EVENTS].sort()).toEqual(['avatar.act', 'avatar.ask', 'avatar.changed', 'avatar.line', 'avatar.say', 'hello', 'resync', 'settings.changed', 'terminal.removed'])
  })

  it('fetches again once this tab listens: a question opened between the first answer and the subscription is not lost', async () => {
    await answer(list([record('t1')]))
    expect(requests).toBe(1)
    // The question opened after the answer was made and before the socket subscribed.
    const q = ask('Q1', 'L1')
    event('hello', { version: '0.0.0', startedAt: 1 }, 30)
    expect(qc.getQueryState(avatarKeys.all)?.isInvalidated).toBe(true)
    await flush()
    expect(requests).toBe(2)
    await answer(list([record('t1', { updatedAt: 25, ask: q, lines: [line('L1', 22, { askId: 'Q1' })] })]))
    expect(of('t1')?.ask).toEqual(q)
  })

  it('replaces the first fetch when the greeting comes while it is still on its way', async () => {
    expect(held()).toBeUndefined()
    event('hello', { version: '0.0.0', startedAt: 1 }, 5)
    await flush()
    expect(requests).toBe(2)
    expect(waiting).toHaveLength(1)
  })

  it('puts a line into the cache without fetching anything', async () => {
    await answer(list([record('t1')]))
    expect(requests).toBe(1)
    event('avatar.say', { terminalId: 't1', line: line('L1', 20) }, 21)
    event('avatar.line', { terminalId: 't1', line: line('L1', 20, { outcome: 'shown' }) }, 25)
    await flush()
    expect(of('t1')?.lines).toEqual([line('L1', 20, { outcome: 'shown' })])
    expect(requests).toBe(1)
  })

  it('keeps a line that was said while a fetch was on its way, and takes the design that fetch brings', async () => {
    await answer(list([record('t1')]))
    // The session designs its look, then says a line at once.
    event('avatar.changed', { terminalId: 't1', revision: 1, on: true }, 100)
    await flush()
    expect(requests).toBe(2)
    event('avatar.say', { terminalId: 't1', line: line('L1', 105) }, 106)
    // The answer was made between the two: it has the design and not the line.
    await answer(list([record('t1', { updatedAt: 100, revision: 1, designed: true, look, name: 'Pip' })]))
    expect([of('t1')?.name, of('t1')?.revision, of('t1')?.designed, of('t1')?.look, of('t1')?.lines.map((l) => l.id)]).toEqual(['Pip', 1, true, look, ['L1']])
    // The same with an answer that already knows the line: it is there once.
    event('avatar.changed', { terminalId: 't1', revision: 2, on: true }, 200)
    await flush()
    event('avatar.line', { terminalId: 't1', line: line('L1', 105, { outcome: 'spoken' }) }, 201)
    await answer(list([record('t1', { updatedAt: 200, revision: 2, designed: true, look, name: 'Pip the second', lines: [line('L1', 105, { outcome: 'shown' })] })]))
    expect([of('t1')?.name, of('t1')?.revision, of('t1')?.lines]).toEqual(['Pip the second', 2, [line('L1', 105, { outcome: 'spoken' })]])
  })

  it('keeps the same look object while the design is the same, so that a figure is not built again', async () => {
    await answer(list([record('t1', { revision: 1, designed: true, look })]))
    const first = of('t1')?.look
    event('avatar.say', { terminalId: 't1', line: line('L1', 20) }, 21)
    event('settings.changed', {}, 30)
    await flush()
    await answer(list([record('t1', { updatedAt: 20, revision: 1, designed: true, look: JSON.parse(JSON.stringify(look)) as Look, lines: [line('L1', 20)] })]))
    expect(of('t1')?.look).toBe(first)
    expect(requests).toBe(2)
  })

  it('replaces a fetch that is on its way when an event says the list changed again', async () => {
    await answer(list([record('t1')]))
    event('avatar.changed', { terminalId: 't1', revision: 1, on: true }, 100)
    await flush()
    event('avatar.changed', { terminalId: 't1', revision: 2, on: true }, 110)
    await flush()
    expect(requests).toBe(3)
    // Only the newest request is still waiting for its answer.
    expect(waiting).toHaveLength(1)
    await answer(list([record('t1', { updatedAt: 110, revision: 2, name: 'second design' })]))
    expect(of('t1')?.name).toBe('second design')
  })

  it('asks for the list when an event is about an avatar it does not hold', async () => {
    await answer(list([]))
    event('avatar.say', { terminalId: 't9', line: line('L1', 20) }, 21)
    await flush()
    expect(requests).toBe(2)
    await answer(list([record('t9', { updatedAt: 20, lines: [line('L1', 20)] })]))
    expect(of('t9')?.lines.map((l) => l.id)).toEqual(['L1'])
  })

  it('turns an avatar off at once and drops a removed one, before the answer arrives', async () => {
    await answer(list([record('t1'), record('t2')]))
    event('avatar.changed', { terminalId: 't1', revision: 0, on: false }, 50)
    expect(of('t1')?.on).toBe(false)
    event('avatar.changed', { terminalId: 't2', revision: 0, on: true, removed: true }, 51)
    expect(held()?.avatars.map((a) => a.terminalId)).toEqual(['t1'])
  })

  it('opens and closes a question from its events, and a stale answer does not bring a closed one back', async () => {
    await answer(list([record('t1')]))
    const q = ask('Q1', 'L1')
    event('avatar.say', { terminalId: 't1', line: line('L1', 20, { askId: 'Q1' }), ask: q }, 21)
    event('avatar.ask', { terminalId: 't1', change: 'opened', ask: q }, 22)
    expect(of('t1')?.ask).toEqual(q)
    event('settings.changed', {}, 23)
    await flush()
    event('avatar.ask', { terminalId: 't1', change: 'answered', ask: null }, 40)
    // The answer was made at 30, while the question was open.
    await answer(list([record('t1', { updatedAt: 30, ask: q, lines: [line('L1', 20, { askId: 'Q1' })] })]))
    expect(of('t1')?.ask).toBeNull()
  })

  it('hands a gesture to the overlay and keeps a held one in the record', async () => {
    await answer(list([record('t1')]))
    event('avatar.act', { terminalId: 't1', play: 'wave', mode: 'once', times: 2, seq: 1 }, 30)
    event('avatar.act', { terminalId: 't1', play: 'lean', mode: 'hold', seq: 2 }, 31)
    event('avatar.act', { terminalId: 't1', stop: 'lean', seq: 3 }, 32)
    expect(acts).toEqual([
      { terminalId: 't1', seq: 1, play: 'wave', times: 2 },
      { terminalId: 't1', seq: 3, stop: 'lean' },
    ])
    expect(of('t1')?.held).toBeNull()
    expect(requests).toBe(1)
  })

  it('drops the record of a removed terminal and says which terminal it was', async () => {
    await answer(list([record('t1'), record('t2')]))
    event('terminal.removed', { id: 't1' }, 60)
    event('terminal.removed', { id: 'shell7' }, 61)
    expect(held()?.avatars.map((a) => a.terminalId)).toEqual(['t2'])
    expect(removed).toEqual(['t1', 'shell7'])
    expect(requests).toBe(1)
  })

  it('fetches again after a saved config and after a reconnect, and ignores what is no event of its own', async () => {
    await answer(list([record('t1')]))
    event('settings.changed', {}, 70)
    await flush()
    await answer(list([record('t1')], { enabled: false }))
    expect(held()?.enabled).toBe(false)
    event('resync', null, 80)
    await flush()
    expect(requests).toBe(3)
    await answer(list([record('t1')]))
    event('avatar.nudge', { terminalId: 't1', askId: 'Q1', name: 'Pip', question: '?' }, 90)
    event('terminal.updated', { id: 't1' }, 91)
    await flush()
    expect(requests).toBe(3)
    expect(acts).toEqual([])
    expect(removed).toEqual([])
  })

  it('copes with an event before the list has arrived: the list is asked for again', async () => {
    // The first request is still waiting.
    expect(held()).toBeUndefined()
    event('avatar.say', { terminalId: 't1', line: line('L1', 20) }, 21)
    await flush()
    expect(requests).toBe(2)
    await answer(list([record('t1', { updatedAt: 20, lines: [line('L1', 20)] })]))
    expect(of('t1')?.lines.map((l) => l.id)).toEqual(['L1'])
  })
})
