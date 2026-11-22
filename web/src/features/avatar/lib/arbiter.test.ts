// The arbiter with the server and the speaker faked: who asks for a line and when, what
// happens to the lease, and what is acknowledged.

import { describe, expect, it } from 'vitest'
import { CLAIM_TIMEOUT_MS, RENEW_EVERY_MS, RENEW_FRESH_MS, claimDelayMs, createArbiter, lineKey } from './arbiter'
import type { ArbiterHost, ArbiterState } from './arbiter'
import { fakeClock, flush } from './fixtures'
import { SILENT_MIN_MS, createSpeaker } from './speech'
import type { SpeakItem, Speaker, SpeakerEvents, SynthLike, UtteranceLike, VoiceLike } from './speech'
import type { Ask, AvatarInfo, Line, Outcome } from './types'

/** A speaker that only records, and reports what the test tells it to. */
class FakeSpeaker implements Speaker {
  items: SpeakItem[] = []
  stops: (string | undefined)[] = []
  drops: string[] = []
  held = new Map<string, SpeakItem>()
  listeners = new Set<Partial<SpeakerEvents>>()
  cur: SpeakItem | null = null
  enqueue(item: SpeakItem): void {
    this.items.push(item)
    this.held.set(item.key, item)
  }
  stop(key?: string): void {
    this.stops.push(key)
    const k = key ?? this.cur?.key
    if (k === undefined || !this.held.has(k)) return
    this.end(k, 'skipped')
  }
  drop(terminalId: string): void {
    this.drops.push(terminalId)
  }
  unlock(): void {}
  setPrefs(): void {}
  current() {
    return this.cur ? { key: this.cur.key, terminalId: this.cur.terminalId } : null
  }
  mouth(): number {
    return -1
  }
  subscribe(on: Partial<SpeakerEvents>): () => void {
    this.listeners.add(on)
    return () => {
      this.listeners.delete(on)
    }
  }
  dispose(): void {}
  /** The speaker begins to present a line. */
  start(key: string, engine: 'browser' | 'silent' = 'browser', voiceName: string | null = engine === 'browser' ? 'Daniel' : null): void {
    this.cur = this.held.get(key) ?? null
    for (const l of this.listeners) l.start?.(key, engine, ['x'], voiceName)
  }
  /** The speaker reports that the chunk at `index` is being said (or shown) now. */
  chunk(key: string, index: number): void {
    for (const l of this.listeners) l.chunk?.(key, index)
  }
  end(key: string, outcome: 'spoken' | 'shown' | 'skipped' | 'failed'): void {
    this.held.delete(key)
    if (this.cur?.key === key) this.cur = null
    for (const l of this.listeners) l.end?.(key, outcome)
  }
  get last(): SpeakItem {
    return this.items[this.items.length - 1]
  }
}

interface Claim {
  terminalId: string
  lineId: string
  release: boolean
  resolve(answer: { speak: boolean; leaseMs: number; retryMs: number }): void
  reject(error: Error): void
}

const YES = { speak: true, leaseMs: 16_000, retryMs: 0 }
const NO = { speak: false, leaseMs: 0, retryMs: 0 }

function setup(over: Partial<ArbiterState> = {}, speaker: Speaker = new FakeSpeaker()) {
  const clock = fakeClock(1000)
  const claims: Claim[] = []
  const heard: string[] = []
  const world = { visible: true, idle: 0, hidden: new Set<string>(), blocked: new Set<string>(), onScreen: new Set<string>() }
  const host: ArbiterHost = {
    tab: 'tab-A',
    now: clock.now,
    setTimeout: clock.setTimeout,
    clearTimeout: clock.clearTimeout,
    visible: () => world.visible,
    idleMs: () => world.idle,
    claim: (terminalId, lineId, body) => {
      expect(body.tab).toBe('tab-A')
      return new Promise((resolve, reject) => claims.push({ terminalId, lineId, release: body.release === true, resolve, reject }))
    },
    heard: (terminalId, lineId, outcome, voice) => {
      heard.push([terminalId, lineId, outcome, voice].filter((x) => x !== undefined).join(' '))
      return Promise.resolve()
    },
  }
  const state = (more: Partial<ArbiterState> = {}): ArbiterState => ({
    voice: true,
    unlocked: true,
    show: true,
    shown: (tid) => !world.hidden.has(tid),
    blocked: (tid) => world.blocked.has(tid),
    onScreen: (tid) => world.onScreen.has(tid),
    ...over,
    ...more,
  })
  const arbiter = createArbiter(host, speaker, state())
  /** The claims that ask for a line (not the ones that give one back). */
  const asked = () => claims.filter((c) => !c.release)
  const released = () => claims.filter((c) => c.release).map((c) => `${c.terminalId} ${c.lineId}`)
  /** The claim delay passes and the server answers the newest claim. */
  const answer = async (a: { speak: boolean; leaseMs: number; retryMs: number } | Error = YES) => {
    clock.advance(0)
    const c = asked().at(-1)
    if (!c) throw new Error('nothing was claimed')
    if (a instanceof Error) c.reject(a)
    else c.resolve(a)
    await flush()
  }
  return { clock, claims, heard, world, host, state, arbiter, speaker: speaker as FakeSpeaker, asked, released, answer }
}

let stamp = 1000
const line = (id: string, over: Partial<Line> = {}): Line => ({ id, at: ++stamp, from: 'agent', text: `Line ${id}.`, ...over })
function avatar(terminalId: string, lines: Line[], over: Partial<AvatarInfo> = {}): AvatarInfo {
  return { terminalId, projectId: 'p1', on: true, name: 'Pip', designed: true, revision: 1, look: null, voice: { lang: 'en-GB' }, via: 'tools', held: null, ask: null, lines, createdAt: 1, updatedAt: 2, ...over }
}
const ask = (lineId: string): Ask => ({ id: 'Q1', lineId, text: 'Push?', choices: ['Yes', 'No'], freeText: true, askedAt: 5, waiting: true })

describe('lineKey', () => {
  it('names a line by its terminal, its id and its time', () => {
    expect(lineKey('t1', { id: 'Lab12-3', at: 1700 })).toBe('t1:Lab12-3:1700')
    expect(lineKey('t1', { id: 'L1', at: 1 })).not.toBe(lineKey('t2', { id: 'L1', at: 1 }))
    expect(lineKey('t1', { id: 'L1', at: 1 })).not.toBe(lineKey('t1', { id: 'L1', at: 2 }))
  })
})

describe('claimDelayMs', () => {
  it('is 900 ms for a hidden tab and at most 600 ms for a visible one, shorter the more recently it was used', () => {
    expect(claimDelayMs(false, 0)).toBe(900)
    expect(claimDelayMs(false, 1e9)).toBe(900)
    expect(claimDelayMs(true, 0)).toBe(0)
    expect(claimDelayMs(true, 1000)).toBe(5)
    expect(claimDelayMs(true, 60_000)).toBe(300)
    expect(claimDelayMs(true, 120_000)).toBe(600)
    expect(claimDelayMs(true, 3_600_000)).toBe(600)
    expect(claimDelayMs(true, Infinity)).toBe(600)
    expect(claimDelayMs(true, -50)).toBe(0)
    expect(claimDelayMs(true, Number.NaN)).toBe(600)
    // The tab used last asks before any other visible tab, and both before a hidden one.
    expect(claimDelayMs(true, 2000)).toBeLessThan(claimDelayMs(true, 30_000))
    expect(claimDelayMs(true, 1e9)).toBeLessThan(claimDelayMs(false, 0))
  })
})

describe('createArbiter: what is offered', () => {
  it('presents every agent line without an outcome once, and nothing else', () => {
    const s = setup({ voice: false })
    const lines = [line('L1', { outcome: 'spoken' }), line('L2', { from: 'user' }), line('L3'), line('L4', { outcome: 'unheard' }), line('L5')]
    s.arbiter.offer(avatar('t1', lines))
    expect(s.speaker.items.map((i) => i.lineId)).toEqual(['L3', 'L5'])
    expect(s.speaker.last).toMatchObject({ key: lineKey('t1', lines[4]), terminalId: 't1', text: 'Line L5.', lang: 'en-GB', voice: { lang: 'en-GB' }, seed: 't1', mode: 'silent' })
    // The cache changes for other reasons and the record is offered again and again.
    for (let i = 0; i < 5; i++) s.arbiter.offer(avatar('t1', lines))
    expect(s.speaker.items).toHaveLength(2)
    s.arbiter.offer(avatar('t1', [...lines, line('L6', { lang: 'de-DE' })]))
    expect(s.speaker.items.map((i) => i.lineId)).toEqual(['L3', 'L5', 'L6'])
    expect(s.speaker.last.lang).toBe('de-DE')
    expect(s.asked()).toEqual([])
  })

  it('takes a line said before it existed, or learned from a refetch, the same way', () => {
    const s = setup({ voice: false })
    s.arbiter.offer(avatar('t1', [line('L1'), line('L2')]))
    expect(s.speaker.items.map((i) => i.lineId)).toEqual(['L1', 'L2'])
  })

  it('ignores what is no record and no line', () => {
    const s = setup({ voice: false })
    for (const bad of [null, undefined, 7, {}, { terminalId: 7 }]) s.arbiter.offer(bad as unknown as AvatarInfo)
    s.arbiter.offer(avatar('t1', [null, 7, { id: 7 }, { id: 'L1', from: 'agent' }, { id: 'L2', from: 'agent', text: 'ok', at: 1 }] as unknown as Line[]))
    s.arbiter.offer(avatar('t2', 'lines' as unknown as Line[]))
    expect(s.speaker.items.map((i) => i.lineId)).toEqual(['L2'])
    s.arbiter.settled('t1', null as unknown as Line)
    s.arbiter.settled('nowhere', line('L9', { outcome: 'spoken' }))
    s.arbiter.settled('t1', line('L2', { outcome: 'weird' as Outcome }))
    s.arbiter.setState(null as unknown as ArbiterState)
    s.arbiter.forget('nowhere')
  })

  it('drops an avatar the user turned off', () => {
    const s = setup({ voice: false })
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    s.speaker.start(lineKey('t1', l1), 'silent')
    s.arbiter.offer(avatar('t1', [l1, line('L2')], { on: false }))
    expect(s.speaker.drops).toEqual(['t1'])
    expect(s.speaker.items).toHaveLength(1)
    // Nothing more is said to the server about its lines.
    expect(s.heard).toEqual(['t1 L1 shown'])
  })

  it('puts an ask’s question before other lines, and the session on screen before others', () => {
    const s = setup({ voice: false })
    s.world.onScreen.add('t2')
    const q = line('L2', { askId: 'Q1' })
    s.arbiter.offer(avatar('t1', [line('L1'), q], { ask: ask('L2') }))
    s.arbiter.offer(avatar('t2', [line('L3')]))
    s.arbiter.offer(avatar('t2', [line('L4', { askId: 'Q2' })], { ask: { ...ask('L4'), id: 'Q2' } }))
    expect(s.speaker.items.map((i) => [i.lineId, i.priority])).toEqual([['L1', 0], ['L2', 2], ['L3', 1], ['L4', 3]])
  })
})

describe('createArbiter: claiming', () => {
  it('asks for the line after the claim delay, with its tab id, and says it when the answer is yes', async () => {
    const s = setup()
    s.world.idle = 60_000
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    expect(s.asked()).toEqual([])
    s.clock.advance(299)
    expect(s.asked()).toEqual([])
    s.clock.advance(1)
    expect(s.asked()).toHaveLength(1)
    expect(s.asked()[0]).toMatchObject({ terminalId: 't1', lineId: 'L1' })
    expect(s.speaker.items).toEqual([])
    await s.answer(YES)
    expect(s.speaker.items).toHaveLength(1)
    expect(s.speaker.last.mode).toBe('voice')
    const key = lineKey('t1', l1)
    s.speaker.start(key, 'browser', 'Daniel')
    expect(s.heard).toEqual([])
    s.speaker.end(key, 'spoken')
    expect(s.heard).toEqual(['t1 L1 spoken Daniel'])
    expect(s.released()).toEqual([])
    expect(s.clock.pending()).toBe(0)
  })

  it('waits 900 ms in a hidden tab', () => {
    const s = setup()
    s.world.visible = false
    s.arbiter.offer(avatar('t1', [line('L1')]))
    s.clock.advance(899)
    expect(s.asked()).toEqual([])
    s.clock.advance(1)
    expect(s.asked()).toHaveLength(1)
  })

  it('never asks when it could not speak: voice off, locked, hidden, blocked', async () => {
    const cases: [string, Partial<ArbiterState>, (w: ReturnType<typeof setup>['world']) => void, number][] = [
      ['voice off', { voice: false }, () => {}, 1],
      ['locked', { unlocked: false }, () => {}, 1],
      ['avatars hidden', { show: false }, () => {}, 0],
      ['not shown here', {}, (w) => void w.hidden.add('t1'), 0],
      ['blocked', {}, (w) => void w.blocked.add('t1'), 1],
    ]
    for (const [name, state, prepare, presented] of cases) {
      const s = setup(state)
      prepare(s.world)
      s.arbiter.offer(avatar('t1', [line('L1')]))
      s.clock.advance(5000)
      await flush()
      expect(s.claims, name).toEqual([])
      // What it does show, it shows as a caption; a hidden avatar is not presented at all.
      expect(s.speaker.items.map((i) => i.mode), name).toEqual(presented ? ['silent'] : [])
      expect(s.heard, name).toEqual([])
    }
  })

  it('shows the caption when another tab has the line', async () => {
    const s = setup()
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer(NO)
    expect(s.speaker.last.mode).toBe('silent')
    const key = lineKey('t1', l1)
    s.speaker.start(key, 'silent')
    expect(s.heard).toEqual(['t1 L1 shown'])
    s.speaker.end(key, 'shown')
    expect(s.heard).toEqual(['t1 L1 shown'])
    expect(s.released()).toEqual([])
  })

  it('shows the caption when the claim fails or throws', async () => {
    const s = setup()
    s.arbiter.offer(avatar('t1', [line('L1')]))
    await s.answer(new Error('offline'))
    expect(s.speaker.items.map((i) => i.mode)).toEqual(['silent'])
    expect(s.clock.pending()).toBe(0)

    const t = setup()
    t.host.claim = () => {
      throw new Error('no fetch')
    }
    t.arbiter.offer(avatar('t1', [line('L1')]))
    t.clock.advance(0)
    expect(t.speaker.items.map((i) => i.mode)).toEqual(['silent'])
    expect(t.clock.pending()).toBe(0)
  })

  it('does not wait for a claim that never answers, and gives back a yes that comes too late', async () => {
    const s = setup()
    s.arbiter.offer(avatar('t1', [line('L1')]))
    s.clock.advance(0)
    expect(s.asked()).toHaveLength(1)
    s.clock.advance(CLAIM_TIMEOUT_MS - 1)
    expect(s.speaker.items).toEqual([])
    s.clock.advance(1)
    expect(s.speaker.items.map((i) => i.mode)).toEqual(['silent'])
    s.asked()[0].resolve(YES)
    await flush()
    expect(s.released()).toEqual(['t1 L1'])
    expect(s.speaker.items).toHaveLength(1)
  })

  it('tries once more when the other tab’s lease has run out and the line still has no outcome', async () => {
    const s = setup()
    // A hidden tab: its caption is seen by nobody, so it acknowledges nothing.
    s.world.visible = false
    const l1 = line('L1')
    const key = lineKey('t1', l1)
    s.arbiter.offer(avatar('t1', [l1]))
    s.clock.advance(900)
    s.asked()[0].resolve({ speak: false, leaseMs: 0, retryMs: 12_000 })
    await flush()
    s.speaker.start(key, 'silent')
    s.speaker.end(key, 'shown')
    expect(s.heard).toEqual([])
    s.clock.advance(12_000)
    expect(s.asked()).toHaveLength(1)
    s.clock.advance(50)
    expect(s.asked()).toHaveLength(2)
    s.asked()[1].resolve(YES)
    await flush()
    expect(s.speaker.last.mode).toBe('voice')
    s.speaker.start(key)
    s.speaker.end(key, 'spoken')
    expect(s.heard).toEqual(['t1 L1 spoken Daniel'])
    expect(s.clock.pending()).toBe(0)
  })

  it('does not try again for a line that got an outcome, and never shows a caption twice', async () => {
    const s = setup()
    const l1 = line('L1')
    const key = lineKey('t1', l1)
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer({ speak: false, leaseMs: 0, retryMs: 5000 })
    s.speaker.start(key, 'silent')
    s.speaker.end(key, 'shown')
    s.clock.advance(6000)
    expect(s.asked()).toHaveLength(1)

    const t = setup()
    t.world.visible = false
    const l2 = line('L2')
    t.arbiter.offer(avatar('t1', [l2]))
    t.clock.advance(900)
    t.asked()[0].resolve({ speak: false, leaseMs: 0, retryMs: 5000 })
    await flush()
    t.speaker.end(lineKey('t1', l2), 'shown')
    t.clock.advance(5050)
    t.asked()[1].resolve({ speak: false, leaseMs: 0, retryMs: 5000 })
    await flush()
    t.clock.advance(20_000)
    expect(t.asked()).toHaveLength(2)
    expect(t.speaker.items).toHaveLength(1)
  })
})

describe('createArbiter: the lease', () => {
  it('does not renew a fresh lease, renews an older one, and tells the speaker when it is lost', async () => {
    const s = setup()
    s.arbiter.offer(avatar('t1', [line('L1')]))
    await s.answer(YES)
    const confirm = s.speaker.last.confirm as () => Promise<boolean>
    expect(await confirm()).toBe(true)
    expect(s.asked()).toHaveLength(1)
    // The line waited behind others: when it reaches the head of the queue the holder asks again.
    s.clock.advance(RENEW_FRESH_MS)
    const renewed = confirm()
    expect(s.asked()).toHaveLength(2)
    s.asked()[1].resolve(YES)
    expect(await renewed).toBe(true)
    s.clock.advance(RENEW_FRESH_MS)
    const lost = confirm()
    s.asked()[2].resolve(NO)
    expect(await lost).toBe(false)
    // Lost is lost: there is nothing to give back.
    expect(s.released()).toEqual([])
    s.clock.advance(RENEW_FRESH_MS)
    const failed = confirm()
    s.asked()[3].reject(new Error('offline'))
    expect(await failed).toBe(false)
  })

  it('renews the lease while the line is said, as a chunk starts and on a timer, never while it is fresh, and keeps it to the end', async () => {
    const s = setup()
    const l1 = line('L1')
    const key = lineKey('t1', l1)
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer(YES)
    s.speaker.start(key, 'browser')
    // The first chunk starts while the lease is fresh: nothing to renew.
    s.speaker.chunk(key, 0)
    expect(s.asked()).toHaveLength(1)
    s.clock.advance(5000)
    s.speaker.chunk(key, 1)
    expect(s.asked()).toHaveLength(2)
    expect(s.claims.at(-1)).toMatchObject({ terminalId: 't1', lineId: 'L1', release: false })
    // The next chunk comes soon after: the lease is fresh again.
    s.clock.advance(RENEW_FRESH_MS - 1)
    s.speaker.chunk(key, 2)
    expect(s.asked()).toHaveLength(2)
    s.asked()[1].resolve(YES)
    await flush()
    // That chunk takes long: half the 16 s lease after the last renewal, the timer renews it.
    s.clock.advance(8000 - RENEW_FRESH_MS)
    expect(s.asked()).toHaveLength(2)
    s.clock.advance(1)
    expect(s.asked()).toHaveLength(3)
    // A renewal that fails takes nothing away: the lease is held as far as this tab knows, and the timer asks again soon.
    s.asked()[2].reject(new Error('offline'))
    await flush()
    s.clock.advance(RENEW_FRESH_MS - 1)
    expect(s.asked()).toHaveLength(3)
    s.clock.advance(1)
    expect(s.asked()).toHaveLength(4)
    s.asked()[3].resolve(YES)
    await flush()
    s.speaker.end(key, 'spoken')
    expect(s.heard).toEqual(['t1 L1 spoken Daniel'])
    expect(s.speaker.stops).toEqual([])
    expect(s.released()).toEqual([])
    expect(s.clock.pending()).toBe(0)
  })

  it('renews nothing for a caption or a line the user asked for, and gives a renewed lease back when the page goes away', async () => {
    const s = setup()
    const l1 = line('L1')
    const k1 = lineKey('t1', l1)
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer(YES)
    // The speaker found a microphone open: a caption, and its lease went back.
    s.speaker.start(k1, 'silent')
    s.clock.advance(12_000)
    s.speaker.chunk(k1, 1)
    expect(s.asked()).toHaveLength(1)
    s.speaker.end(k1, 'shown')
    // "Hear it": said without a lease.
    s.arbiter.replay(avatar('t1', [{ ...l1, outcome: 'shown' }]), l1)
    s.speaker.start(k1)
    s.clock.advance(12_000)
    s.speaker.chunk(k1, 1)
    expect(s.asked()).toHaveLength(1)
    s.speaker.end(k1, 'spoken')
    expect(s.released()).toEqual(['t1 L1'])

    const t = setup()
    const l2 = line('L2')
    const k2 = lineKey('t1', l2)
    t.arbiter.offer(avatar('t1', [l2]))
    await t.answer(YES)
    t.speaker.start(k2)
    t.clock.advance(12_000)
    t.speaker.chunk(k2, 1)
    t.asked()[1].resolve(YES)
    await flush()
    t.arbiter.release()
    expect(t.speaker.stops).toEqual([k2])
    expect(t.released()).toEqual(['t1 L2'])
    expect(t.heard).toEqual([])
  })

  it('stops saying a line whose lease the server gave to another tab, without a word to the server', async () => {
    const s = setup()
    const l1 = line('L1')
    const key = lineKey('t1', l1)
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer(YES)
    s.speaker.start(key)
    s.clock.advance(5000)
    s.speaker.chunk(key, 1)
    // The lease had lapsed and another tab took it: that tab says the line now.
    s.asked()[1].resolve({ speak: false, leaseMs: 0, retryMs: 30_000 })
    await flush()
    expect(s.speaker.stops).toEqual([key])
    expect(s.heard).toEqual([])
    expect(s.released()).toEqual([])
    s.clock.advance(60_000)
    expect(s.asked()).toHaveLength(2)
    expect(s.clock.pending()).toBe(0)

    // A plain no takes the line from nobody (it is over, stale, or its avatar is off): it goes on, and is not renewed again.
    const t = setup()
    const l2 = line('L2')
    const k2 = lineKey('t1', l2)
    t.arbiter.offer(avatar('t1', [l2]))
    await t.answer(YES)
    t.speaker.start(k2)
    t.clock.advance(5000)
    t.speaker.chunk(k2, 1)
    t.asked()[1].resolve(NO)
    await flush()
    // Nor by the timer: it ended with the lease.
    expect(t.clock.pending()).toBe(0)
    t.clock.advance(12_000)
    t.speaker.chunk(k2, 2)
    expect(t.asked()).toHaveLength(2)
    expect(t.speaker.stops).toEqual([])
    t.speaker.end(k2, 'spoken')
    expect(t.heard).toEqual(['t1 L2 spoken Daniel'])
    expect(t.released()).toEqual([])
  })

  /**
   * Two tabs of one browser against a server that keeps leases as device.rs does (staleness
   * plays no part in a minute): A visible, B hidden. What each was answered, as `<ms> speak|no`.
   */
  function twoTabs(l1: Line) {
    const clock = fakeClock(1000)
    const T0 = clock.now()
    const leases = new Map<string, { tab: string; until: number }>()
    const world = { outcome: null as Outcome | null }
    const serve = (tab: string, lineId: string, release: boolean) => {
      const now = clock.now()
      const lease = leases.get(lineId)
      if (release) {
        if (lease?.tab === tab) leases.delete(lineId)
        return NO
      }
      if (world.outcome !== null && world.outcome !== 'shown' && world.outcome !== 'hidden') return NO
      if (lease && lease.until > now && lease.tab !== tab) return { speak: false, leaseMs: 0, retryMs: lease.until - now }
      const leaseMs = Math.floor((l1.text.length * 1000) / 15) + 15_000
      leases.set(lineId, { tab, until: now + leaseMs })
      return { speak: true, leaseMs, retryMs: 0 }
    }
    const tab = (id: string, visible: boolean) => {
      const speaker = new FakeSpeaker()
      const answers: string[] = []
      const host: ArbiterHost = {
        tab: id,
        now: clock.now,
        setTimeout: clock.setTimeout,
        clearTimeout: clock.clearTimeout,
        visible: () => visible,
        idleMs: () => 0,
        claim: (_tid, lineId, body) => {
          const answer = serve(body.tab, lineId, body.release === true)
          if (body.release !== true) answers.push(`${Math.round((clock.now() - T0) / 100) * 100} ${answer.speak ? 'speak' : 'no'}`)
          return Promise.resolve(answer)
        },
        heard: (_tid, _lineId, o) => {
          if (world.outcome === null || o === 'spoken') world.outcome = o
          return Promise.resolve()
        },
      }
      const state: ArbiterState = { voice: true, unlocked: true, show: true, shown: () => true, blocked: () => false, onScreen: () => false }
      return { speaker, answers, arbiter: createArbiter(host, speaker, state) }
    }
    const until = async (ms: number) => {
      clock.advance(0)
      await flush()
      while (clock.now() < T0 + ms) {
        clock.advance(Math.min(100, T0 + ms - clock.now()))
        await flush()
      }
    }
    return { clock, world, a: tab('tab-A', true), b: tab('tab-B', false), until }
  }

  it('keeps a long line from a hidden tab while its holder still says it', async () => {
    const l1 = line('L1', { text: 'One two three four five six seven eight nine ten. '.repeat(8).trim() })
    const key = lineKey('t1', l1)
    const { clock, world, a, b, until } = twoTabs(l1)
    a.arbiter.offer(avatar('t1', [l1]))
    b.arbiter.offer(avatar('t1', [l1]))
    await until(0)
    // A, visible, asks at once and says it: four chunks about twelve seconds apart.
    expect(a.speaker.last.mode).toBe('voice')
    a.speaker.start(key)
    a.speaker.chunk(key, 0)
    // B, hidden, asks after 900 ms and shows the caption silently, to nobody.
    await until(1000)
    expect(b.speaker.last.mode).toBe('silent')
    b.speaker.start(key, 'silent')
    for (const [at, index] of [[12_000, 1], [24_000, 2], [36_000, 3]]) {
      await until(at)
      a.speaker.chunk(key, index)
    }
    await until(38_000)
    b.speaker.end(key, 'shown')
    await until(46_000)
    a.speaker.end(key, 'spoken')
    b.arbiter.settled('t1', { ...l1, outcome: 'spoken' })
    await until(90_000)
    // B tried once more when the first lease would have run out, and was told no: A still had it.
    expect(b.answers).toEqual(['900 no', '41700 no'])
    expect(b.speaker.items.map((i) => i.mode)).toEqual(['silent'])
    // A renewed ten seconds after each renewal, and as each chunk started once the lease was no longer fresh.
    expect(a.answers).toEqual(['0 speak', '10000 speak', '12000 speak', '22000 speak', '24000 speak', '34000 speak', '36000 speak', '46000 speak'])
    expect(world.outcome).toBe('spoken')
    expect(clock.pending()).toBe(0)
  })

  it('keeps a line of one slow chunk from a hidden tab: the holder renews while it says it, not only as a chunk starts', async () => {
    // About 160 characters: one chunk, and a lease of about 26 s. The voice takes 30 s for it.
    const l1 = line('L1', { text: 'Every chunk of this line is said slowly. '.repeat(4).trim() })
    const key = lineKey('t1', l1)
    const { clock, world, a, b, until } = twoTabs(l1)
    a.arbiter.offer(avatar('t1', [l1]))
    b.arbiter.offer(avatar('t1', [l1]))
    await until(0)
    a.speaker.start(key)
    a.speaker.chunk(key, 0)
    await until(1000)
    b.speaker.start(key, 'silent')
    await until(12_000)
    b.speaker.end(key, 'shown')
    await until(29_900)
    a.speaker.end(key, 'spoken')
    b.arbiter.settled('t1', { ...l1, outcome: 'spoken' })
    await until(90_000)
    // B's one more try is told no, and B never says the line.
    expect(b.speaker.items.map((i) => i.mode)).toEqual(['silent'])
    expect(b.answers).toEqual(['900 no', '25900 no'])
    expect(a.answers).toEqual(['0 speak', '10000 speak', '20000 speak'])
    expect(world.outcome).toBe('spoken')
    expect(clock.pending()).toBe(0)
  })

  it('asks again two seconds after a renewal that failed, so that one failure leaves time before the lease runs out', async () => {
    const s = setup()
    const l = line('L1')
    const key = lineKey('t1', l)
    s.arbiter.offer(avatar('t1', [l]))
    await s.answer(YES)
    const t0 = s.clock.now()
    s.speaker.start(key)
    s.speaker.chunk(key, 0)
    // The 16 s lease is renewed after half of it, and that renewal fails.
    s.clock.advance(8000)
    expect(s.asked()).toHaveLength(2)
    s.asked()[1].reject(new Error('offline'))
    await flush()
    // A chunk that starts meanwhile does not put the next try off by a whole interval.
    s.clock.advance(1000)
    s.speaker.chunk(key, 1)
    expect(s.asked()).toHaveLength(2)
    s.clock.advance(RENEW_FRESH_MS - 1001)
    expect(s.asked()).toHaveLength(2)
    s.clock.advance(1)
    expect(s.asked()).toHaveLength(3)
    expect(s.clock.now() - t0).toBe(8000 + RENEW_FRESH_MS)
    // A network that keeps failing is asked every two seconds, no more often; a request that throws counts the same.
    const claim = s.host.claim
    s.host.claim = () => {
      throw new Error('offline')
    }
    s.asked()[2].reject(new Error('offline'))
    await flush()
    s.clock.advance(RENEW_FRESH_MS)
    s.host.claim = claim
    expect(s.asked()).toHaveLength(3)
    s.clock.advance(RENEW_FRESH_MS - 1)
    expect(s.asked()).toHaveLength(3)
    s.clock.advance(1)
    expect(s.asked()).toHaveLength(4)
    expect(s.clock.now() - t0).toBe(8000 + 3 * RENEW_FRESH_MS)
    // Renewed: the next renewal is half the lease later again.
    s.asked()[3].resolve(YES)
    await flush()
    s.clock.advance(8000 - 1)
    expect(s.asked()).toHaveLength(4)
    s.clock.advance(1)
    expect(s.asked()).toHaveLength(5)
    s.asked()[4].resolve(YES)
    await flush()
    s.speaker.end(key, 'spoken')
    expect(s.heard).toEqual(['t1 L1 spoken Daniel'])
    expect(s.speaker.stops).toEqual([])
    expect(s.clock.pending()).toBe(0)
  })

  it('renews on a timer every ten seconds, or half the lease, and stops with the line, the lease and the arbiter', async () => {
    const lease = (leaseMs: number) => ({ speak: true, leaseMs, retryMs: 0 })
    const saying = async (answer = YES) => {
      const s = setup()
      const l = line('L1')
      const key = lineKey('t1', l)
      s.arbiter.offer(avatar('t1', [l]))
      await s.answer(answer)
      s.speaker.start(key)
      s.speaker.chunk(key, 0)
      return { s, key }
    }
    // A lease of 25 s: every ten seconds, while one chunk goes on and on; the line ends, and so does the timer.
    const { s, key } = await saying(lease(25_000))
    for (const n of [2, 3]) {
      s.clock.advance(RENEW_EVERY_MS - 1)
      expect(s.asked()).toHaveLength(n - 1)
      s.clock.advance(1)
      expect(s.asked()).toHaveLength(n)
      s.asked()[n - 1].resolve(lease(25_000))
      await flush()
    }
    s.clock.advance(5000)
    s.speaker.end(key, 'spoken')
    expect(s.clock.pending()).toBe(0)
    expect(s.released()).toEqual([])
    // A lease of 16 s: half of it.
    const half = await saying(lease(16_000))
    half.s.clock.advance(7999)
    expect(half.s.asked()).toHaveLength(1)
    half.s.clock.advance(1)
    expect(half.s.asked()).toHaveLength(2)
    // A renewal answered with a plain no: the line goes on, and nothing renews it any more.
    half.s.asked()[1].resolve(NO)
    await flush()
    expect(half.s.clock.pending()).toBe(0)
    half.s.clock.advance(60_000)
    expect(half.s.asked()).toHaveLength(2)
    half.s.speaker.end(half.key, 'spoken')
    expect(half.s.heard).toEqual(['t1 L1 spoken Daniel'])
    // A no with a time to try again: another tab says the line now, and this one falls silent.
    const lost = await saying()
    lost.s.clock.advance(8000)
    lost.s.asked()[1].resolve({ speak: false, leaseMs: 0, retryMs: 20_000 })
    await flush()
    expect(lost.s.speaker.stops).toEqual([lost.key])
    expect(lost.s.heard).toEqual([])
    expect(lost.s.clock.pending()).toBe(0)
    // The browser engine gave up and the silent one shows the rest: the lease goes back, and its renewal with it.
    const gaveUp = await saying()
    gaveUp.s.speaker.start(gaveUp.key, 'silent')
    expect(gaveUp.s.released()).toEqual(['t1 L1'])
    expect(gaveUp.s.clock.pending()).toBe(0)
    // The arbiter is disposed of in the middle: nothing is asked any more.
    const gone = await saying()
    gone.s.arbiter.dispose()
    gone.s.clock.advance(60_000)
    expect(gone.s.asked()).toHaveLength(1)
    expect(gone.s.clock.pending()).toBe(0)
  })

  it('does not say a line it could no longer say when its turn comes, and gives the lease back', async () => {
    const s = setup()
    s.arbiter.offer(avatar('t1', [line('L1')]))
    await s.answer(YES)
    const confirm = s.speaker.last.confirm as () => Promise<boolean>
    s.arbiter.setState(s.state({ voice: false }))
    expect(await confirm()).toBe(false)
    expect(s.asked()).toHaveLength(1)
    expect(s.released()).toEqual(['t1 L1'])
  })

  it('gives the lease back when the line ends up as a caption', async () => {
    const s = setup()
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer(YES)
    // The speaker found a microphone open, or the line had waited too long.
    s.speaker.start(lineKey('t1', l1), 'silent')
    expect(s.released()).toEqual(['t1 L1'])
    expect(s.heard).toEqual(['t1 L1 shown'])
    s.speaker.end(lineKey('t1', l1), 'shown')
    expect(s.released()).toEqual(['t1 L1'])
  })

  it('releases every lease and stops speech when the page goes away, and posts nothing else', async () => {
    const s = setup()
    const l1 = line('L1')
    const l2 = line('L2')
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer(YES)
    s.arbiter.offer(avatar('t2', [l2]))
    await s.answer(YES)
    s.arbiter.offer(avatar('t3', [line('L3')]))
    s.clock.advance(0)
    s.speaker.start(lineKey('t1', l1))
    s.arbiter.release()
    expect(s.speaker.stops.sort()).toEqual([lineKey('t1', l1), lineKey('t2', l2)].sort())
    expect(s.released().sort()).toEqual(['t1 L1', 't2 L2'])
    expect(s.heard).toEqual([])
    // The claim that was still on its way is given back when it says yes.
    s.asked()[2].resolve(YES)
    await flush()
    expect(s.released().sort()).toEqual(['t1 L1', 't2 L2', 't3 L3'])
    expect(s.speaker.items).toHaveLength(2)
    expect(s.clock.pending()).toBe(0)
    // The page came back (it was only put aside): the same lines are not presented again.
    s.arbiter.offer(avatar('t1', [l1]))
    s.clock.advance(5000)
    expect(s.speaker.items).toHaveLength(2)
  })

  it('posts nothing but releases once disposed', async () => {
    const s = setup()
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer(YES)
    s.arbiter.offer(avatar('t2', [line('L2')]))
    s.clock.advance(0)
    s.speaker.start(lineKey('t1', l1))
    s.arbiter.dispose()
    expect(s.released()).toEqual(['t1 L1'])
    s.asked()[1].resolve(YES)
    await flush()
    expect(s.released()).toEqual(['t1 L1', 't2 L2'])
    expect(s.speaker.listeners.size).toBe(0)
    s.speaker.end(lineKey('t1', l1), 'spoken')
    s.arbiter.offer(avatar('t3', [line('L3')]))
    s.arbiter.settled('t1', { ...l1, outcome: 'skipped' })
    s.arbiter.setState(s.state())
    s.arbiter.forget('t1')
    s.arbiter.release()
    s.arbiter.dispose()
    s.clock.advance(60_000)
    expect(s.heard).toEqual([])
    expect(s.claims).toHaveLength(4)
    expect(s.clock.pending()).toBe(0)
  })
})

describe('createArbiter: what it acknowledges', () => {
  const spoken = async (s: ReturnType<typeof setup>, l: Line, tid = 't1') => {
    s.arbiter.offer(avatar(tid, [l]))
    await s.answer(YES)
    s.speaker.start(lineKey(tid, l))
    return lineKey(tid, l)
  }

  it('reports the outcome the speaker gives: spoken with the voice, skipped, failed', async () => {
    const s = setup()
    s.speaker.end(await spoken(s, line('L1')), 'spoken')
    s.speaker.end(await spoken(s, line('L2'), 't2'), 'skipped')
    s.speaker.end(await spoken(s, line('L3'), 't3'), 'failed')
    expect(s.heard).toEqual(['t1 L1 spoken Daniel', 't2 L2 skipped', 't3 L3 failed'])
    expect(s.released()).toEqual([])
  })

  it('acknowledges "shown" from a visible tab only, when the caption starts', () => {
    const s = setup({ voice: false })
    const l1 = line('L1')
    const l2 = line('L2')
    s.arbiter.offer(avatar('t1', [l1]))
    s.world.visible = false
    s.speaker.start(lineKey('t1', l1), 'silent')
    s.speaker.end(lineKey('t1', l1), 'shown')
    expect(s.heard).toEqual([])
    // The user came back to the tab while the caption was up.
    s.arbiter.offer(avatar('t1', [l1, l2]))
    s.speaker.start(lineKey('t1', l2), 'silent')
    s.world.visible = true
    s.speaker.end(lineKey('t1', l2), 'shown')
    expect(s.heard).toEqual(['t1 L2 shown'])
  })

  it('acknowledges a line that gave way to a newer one as shown', () => {
    const s = setup({ voice: false })
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    // It never started: the speaker dropped it for the avatar's next line.
    s.speaker.end(lineKey('t1', l1), 'shown')
    expect(s.heard).toEqual(['t1 L1 shown'])
  })

  it('reports the better outcome after the weaker one, and never the same twice', async () => {
    const s = setup()
    const l1 = line('L1')
    const key = await spoken(s, l1)
    // The engine failed in the middle: the rest is a caption, and the line counts as failed.
    s.speaker.start(key, 'silent')
    s.speaker.end(key, 'failed')
    expect(s.heard).toEqual(['t1 L1 shown', 't1 L1 failed'])
    s.speaker.end(key, 'failed')
    s.arbiter.settled('t1', { ...l1, outcome: 'failed' })
    expect(s.heard).toHaveLength(2)
  })

  it('stops a line that was skipped on another device, and never calls it spoken afterwards', async () => {
    const s = setup()
    const l1 = line('L1')
    const key = await spoken(s, l1)
    s.arbiter.settled('t1', { ...l1, outcome: 'skipped' })
    expect(s.speaker.stops).toEqual([key])
    expect(s.heard).toEqual([])
    expect(s.released()).toEqual(['t1 L1'])

    // The last word still came out here before the stop arrived.
    const t = setup()
    const l2 = line('L2')
    const k2 = await spoken(t, l2)
    const real = t.speaker.stop.bind(t.speaker)
    t.speaker.stop = (key?: string) => {
      t.speaker.stops.push(key)
    }
    t.arbiter.settled('t1', { ...l2, outcome: 'skipped' })
    t.speaker.stop = real
    t.speaker.end(k2, 'spoken')
    expect(t.heard).toEqual([])
  })

  it('lets go of a line whose speaker no longer answers', async () => {
    const s = setup()
    const l1 = line('L1')
    await spoken(s, l1)
    // The speaker was disposed first: it reports nothing any more.
    s.speaker.stop = () => {}
    s.arbiter.settled('t1', { ...l1, outcome: 'unheard' })
    // The line is over for the arbiter all the same: the record can drop it, and it is forgotten.
    s.arbiter.offer(avatar('t1', []))
    s.arbiter.offer(avatar('t1', [l1]))
    s.clock.advance(0)
    expect(s.asked()).toHaveLength(2)
  })

  it('learns the same from the record as from the event', async () => {
    const s = setup()
    const l1 = line('L1')
    const key = await spoken(s, l1)
    s.arbiter.offer(avatar('t1', [{ ...l1, outcome: 'skipped' }]))
    expect(s.speaker.stops).toEqual([key])
    expect(s.heard).toEqual([])
  })

  it('goes on saying a line that another tab only showed', async () => {
    const s = setup()
    const l1 = line('L1')
    const key = await spoken(s, l1)
    for (const outcome of ['shown', 'hidden'] as const) s.arbiter.settled('t1', { ...l1, outcome })
    expect(s.speaker.stops).toEqual([])
    s.speaker.end(key, 'spoken')
    expect(s.heard).toEqual(['t1 L1 spoken Daniel'])
  })

  it('still asks for a line that another tab already showed, and stops asking for one that is over', async () => {
    const s = setup()
    s.world.idle = 60_000
    const l1 = line('L1')
    const l2 = line('L2')
    const l3 = line('L3')
    s.arbiter.offer(avatar('t1', [l1]))
    s.arbiter.offer(avatar('t2', [l2]))
    s.arbiter.offer(avatar('t3', [l3]))
    // Other devices are quicker: a caption there, a skip there, and one that was said there.
    s.arbiter.settled('t1', { ...l1, outcome: 'shown' })
    s.arbiter.settled('t2', { ...l2, outcome: 'skipped' })
    s.arbiter.settled('t3', { ...l3, outcome: 'spoken' })
    s.clock.advance(300)
    expect(s.asked().map((c) => c.lineId)).toEqual(['L1'])
    // What was said elsewhere is a caption here; what was skipped is nothing.
    expect(s.speaker.items.map((i) => [i.lineId, i.mode])).toEqual([['L3', 'silent']])
    await s.answer(YES)
    expect(s.speaker.items.map((i) => [i.lineId, i.mode])).toEqual([['L3', 'silent'], ['L1', 'voice']])
    s.speaker.start(lineKey('t3', l3), 'silent')
    s.speaker.end(lineKey('t3', l3), 'shown')
    // The server knows better already: nothing weaker is sent after it.
    expect(s.heard).toEqual([])
  })

  it('gives back a yes for a line that was settled while the claim was on its way', async () => {
    const s = setup()
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    s.clock.advance(0)
    s.arbiter.settled('t1', { ...l1, outcome: 'unheard' })
    s.asked()[0].resolve(YES)
    await flush()
    expect(s.released()).toEqual(['t1 L1'])
    expect(s.speaker.items).toEqual([])
    expect(s.clock.pending()).toBe(0)
  })

  it('forgets an avatar: its lines stop, the speaker drops it, the server hears no more of it', async () => {
    const s = setup()
    const l1 = line('L1')
    const key = await spoken(s, l1)
    s.arbiter.offer(avatar('t1', [l1, line('L2')]))
    s.arbiter.forget('t1')
    expect(s.speaker.stops).toEqual([key])
    expect(s.speaker.drops).toEqual(['t1'])
    expect(s.released()).toEqual(['t1 L1'])
    s.clock.advance(5000)
    await flush()
    expect(s.heard).toEqual([])
    expect(s.asked()).toHaveLength(1)
    expect(s.clock.pending()).toBe(0)
  })
})

describe('createArbiter: hidden and blocked avatars', () => {
  it('leaves the lines of an avatar it does not show alone, and presents them when it is shown', async () => {
    const s = setup()
    s.world.hidden.add('t1')
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    s.clock.advance(5000)
    expect(s.claims).toEqual([])
    expect(s.speaker.items).toEqual([])
    s.world.hidden.delete('t1')
    s.arbiter.setState(s.state())
    await s.answer(YES)
    expect(s.speaker.items.map((i) => i.lineId)).toEqual(['L1'])
    // Unless the line got its outcome meanwhile.
    const t = setup({ show: false })
    const l2 = line('L2')
    t.arbiter.offer(avatar('t1', [l2]))
    t.arbiter.offer(avatar('t1', [{ ...l2, outcome: 'hidden' }]))
    t.arbiter.setState(t.state({ show: true }))
    t.clock.advance(5000)
    expect(t.claims).toEqual([])
    expect(t.speaker.items).toEqual([])
  })

  it('stops presenting an avatar that is hidden in the middle, without a word to the server', async () => {
    const s = setup()
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer(YES)
    s.speaker.start(lineKey('t1', l1))
    s.arbiter.offer(avatar('t2', [line('L2')]))
    s.arbiter.setState(s.state({ show: false }))
    expect(s.speaker.stops).toEqual([lineKey('t1', l1)])
    expect(s.heard).toEqual([])
    expect(s.released()).toEqual(['t1 L1'])
    s.clock.advance(5000)
    expect(s.asked()).toHaveLength(1)
  })

  it('goes quiet when the session needs the user: speech stops, what waits is shown silently', async () => {
    const s = setup()
    const l1 = line('L1')
    const l2 = line('L2')
    const l3 = line('L3')
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer(YES)
    s.speaker.start(lineKey('t1', l1))
    s.arbiter.offer(avatar('t1', [l1, l2]))
    await s.answer(YES)
    s.arbiter.offer(avatar('t1', [l1, l2, l3]))
    expect(s.speaker.items.map((i) => i.mode)).toEqual(['voice', 'voice'])
    // A permission request opens in the session.
    s.world.blocked.add('t1')
    s.arbiter.setState(s.state())
    // The line being said stops; it was on screen, so it counts as shown, not as skipped by the user.
    expect(s.speaker.stops).toEqual([lineKey('t1', l1), lineKey('t1', l2)])
    expect(s.heard).toEqual(['t1 L1 shown'])
    // The one that waited is queued again as a caption, and the one that was about to be asked for too.
    expect(s.speaker.items.map((i) => [i.lineId, i.mode])).toEqual([['L1', 'voice'], ['L2', 'voice'], ['L2', 'silent'], ['L3', 'silent']])
    expect(s.released().sort()).toEqual(['t1 L1', 't1 L2'])
    s.clock.advance(5000)
    expect(s.asked()).toHaveLength(2)
    // Told again and again while the request is open: nothing is queued twice.
    for (let i = 0; i < 5; i++) s.arbiter.setState(s.state())
    expect(s.speaker.items).toHaveLength(4)
    // Another session is not affected.
    s.arbiter.offer(avatar('t2', [line('L9')]))
    s.clock.advance(0)
    expect(s.asked()).toHaveLength(3)
  })

  it('leaves a caption on screen when its session becomes blocked', () => {
    const s = setup({ voice: false })
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    s.speaker.start(lineKey('t1', l1), 'silent')
    s.world.blocked.add('t1')
    s.arbiter.setState(s.state())
    expect(s.speaker.stops).toEqual([])
    expect(s.heard).toEqual(['t1 L1 shown'])
  })

  it('drops a claim it was about to make when voice is turned off, and shows the caption', async () => {
    const s = setup()
    s.world.idle = 60_000
    s.arbiter.offer(avatar('t1', [line('L1')]))
    s.arbiter.offer(avatar('t2', [line('L2')]))
    s.clock.advance(300)
    expect(s.asked()).toHaveLength(2)
    s.arbiter.offer(avatar('t3', [line('L3')]))
    s.arbiter.setState(s.state({ voice: false }))
    expect(s.speaker.items.map((i) => [i.lineId, i.mode])).toEqual([['L1', 'silent'], ['L2', 'silent'], ['L3', 'silent']])
    // The answers that were on their way are given back.
    s.asked()[0].resolve(YES)
    s.asked()[1].resolve(NO)
    await flush()
    expect(s.released()).toEqual(['t1 L1'])
    s.clock.advance(5000)
    expect(s.asked()).toHaveLength(2)
    expect(s.clock.pending()).toBe(0)
  })
})

describe('createArbiter: avatars hidden', () => {
  it('presents nothing while the avatars are hidden, whatever else it could do: no claim, no caption, no speech', async () => {
    for (const state of [{}, { voice: false }, { unlocked: false }] as Partial<ArbiterState>[]) {
      const s = setup({ show: false, ...state })
      s.world.onScreen.add('t1')
      s.arbiter.offer(avatar('t1', [line('L1'), line('L2', { askId: 'Q1' })], { ask: ask('L2') }))
      s.clock.advance(5000)
      await flush()
      expect(s.claims).toEqual([])
      expect(s.speaker.items).toEqual([])
      // The always-loaded provider tells the server "hidden"; the arbiter says nothing.
      expect(s.heard).toEqual([])
      expect(s.clock.pending()).toBe(0)
    }
  })

  it('has no way to speak without showing', async () => {
    const s = setup()
    // @ts-expect-error Hidden means silent: an arbiter state has no audio-only mode.
    const odd: ArbiterState = { ...s.state({ show: false }), audioOnly: true }
    s.arbiter.setState(odd)
    s.arbiter.offer(avatar('t1', [line('L1')]))
    s.clock.advance(5000)
    await flush()
    expect(s.claims).toEqual([])
    expect(s.speaker.items).toEqual([])
  })

  it('falls silent when the avatars are hidden: speech and captions stop, without a word to the server', async () => {
    const s = setup()
    const said = line('L1')
    const shown = line('L2')
    const waiting = line('L3')
    s.arbiter.offer(avatar('t1', [said]))
    await s.answer(YES)
    s.arbiter.offer(avatar('t2', [shown]))
    await s.answer(NO)
    s.world.idle = 60_000
    s.arbiter.offer(avatar('t3', [waiting]))
    s.speaker.start(lineKey('t1', said))
    s.arbiter.setState(s.state({ show: false }))
    expect([...s.speaker.stops].sort()).toEqual([lineKey('t1', said), lineKey('t2', shown)].sort())
    expect(s.heard).toEqual([])
    expect(s.released()).toEqual(['t1 L1'])
    // The line that was about to be asked for is not asked for.
    s.clock.advance(5000)
    expect(s.asked()).toHaveLength(2)
    expect(s.clock.pending()).toBe(0)
    // Shown again: what was cut off is not started over; what was never looked at gets its turn.
    s.world.idle = 0
    s.arbiter.setState(s.state())
    s.clock.advance(0)
    expect(s.asked().map((c) => c.lineId)).toEqual(['L1', 'L2', 'L3'])
    expect(s.speaker.items.map((i) => i.lineId)).toEqual(['L1', 'L2'])
  })
})

describe('createArbiter: a line the user asks to hear', () => {
  /** Voice is not chosen yet: the line is a caption, shown and over. */
  const captioned = (s: ReturnType<typeof setup>, l: Line, tid = 't1') => {
    s.arbiter.offer(avatar(tid, [l]))
    s.speaker.start(lineKey(tid, l), 'silent')
    s.speaker.end(lineKey(tid, l), 'shown')
    return lineKey(tid, l)
  }

  it('says a line that was only shown, without asking the server for it, and reports that it was said', () => {
    const s = setup({ voice: false })
    const l1 = line('L1', { lang: 'de-DE' })
    const key = captioned(s, l1)
    expect(s.heard).toEqual(['t1 L1 shown'])
    // "Hear it": the tab's state may still say voice is off; the user asked.
    expect(s.arbiter.replay(avatar('t1', [{ ...l1, outcome: 'shown' }]), l1)).toBe(true)
    expect(s.speaker.items).toHaveLength(2)
    expect(s.speaker.last).toMatchObject({ key, terminalId: 't1', lineId: 'L1', text: 'Line L1.', lang: 'de-DE', seed: 't1', mode: 'voice' })
    // No lease: nothing to renew when its turn comes.
    expect(s.speaker.last.confirm).toBeUndefined()
    s.speaker.start(key)
    s.speaker.end(key, 'spoken')
    expect(s.heard).toEqual(['t1 L1 shown', 't1 L1 spoken Daniel'])
    expect(s.claims).toEqual([])
    expect(s.clock.pending()).toBe(0)
    // The record is offered again and again: the line is not presented a third time.
    s.arbiter.setState(s.state({ voice: true }))
    s.arbiter.offer(avatar('t1', [{ ...l1, outcome: 'spoken' }]))
    s.clock.advance(5000)
    expect(s.speaker.items).toHaveLength(2)
    expect(s.claims).toEqual([])
  })

  it('goes before everything that waits', () => {
    const s = setup({ voice: false })
    s.world.onScreen.add('t2')
    const l1 = line('L1')
    captioned(s, l1)
    s.arbiter.offer(avatar('t2', [line('L2', { askId: 'Q1' })], { ask: ask('L2') }))
    const question = s.speaker.last.priority
    s.arbiter.replay(avatar('t1', [l1]), l1)
    expect(s.speaker.last.priority).toBeGreaterThan(question)
  })

  it('reports a stop and a failure of such a line like any other', () => {
    const s = setup({ voice: false })
    const l1 = line('L1')
    const l2 = line('L2')
    const k1 = captioned(s, l1)
    const k2 = captioned(s, l2, 't2')
    s.arbiter.replay(avatar('t1', [l1]), l1)
    s.speaker.start(k1)
    // The user's Stop.
    s.speaker.stop(k1)
    s.arbiter.replay(avatar('t2', [l2]), l2)
    s.speaker.start(k2)
    s.speaker.start(k2, 'silent')
    s.speaker.end(k2, 'failed')
    expect(s.heard).toEqual(['t1 L1 shown', 't2 L2 shown', 't1 L1 skipped', 't2 L2 failed'])
    expect(s.claims).toEqual([])
  })

  it('is recorded as spoken after an earlier skip, and is not stopped by what the record already says', async () => {
    // Skipped in this tab, then asked for after all.
    const s = setup()
    const l1 = line('L1')
    const key = lineKey('t1', l1)
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer(YES)
    s.speaker.start(key)
    s.speaker.stop(key)
    expect(s.heard).toEqual(['t1 L1 skipped'])
    const skipped = avatar('t1', [{ ...l1, outcome: 'skipped' }])
    expect(s.arbiter.replay(skipped, l1)).toBe(true)
    s.speaker.start(key)
    // The cache changes for other reasons while it is said.
    for (let i = 0; i < 3; i++) s.arbiter.offer(skipped)
    s.arbiter.settled('t1', { ...l1, outcome: 'skipped' })
    expect(s.speaker.stops).toEqual([key])
    s.speaker.end(key, 'spoken')
    expect(s.heard).toEqual(['t1 L1 skipped', 't1 L1 spoken Daniel'])

    // Skipped on another device while it was said here: this tab fell silent without a word. Asked for again, it reports.
    const u = setup()
    const l3 = line('L3')
    const k3 = lineKey('t1', l3)
    u.arbiter.offer(avatar('t1', [l3]))
    await u.answer(YES)
    u.speaker.start(k3)
    u.arbiter.settled('t1', { ...l3, outcome: 'skipped' })
    expect(u.heard).toEqual([])
    expect(u.arbiter.replay(avatar('t1', [{ ...l3, outcome: 'skipped' }]), l3)).toBe(true)
    u.speaker.start(k3)
    u.speaker.end(k3, 'spoken')
    expect(u.heard).toEqual(['t1 L3 spoken Daniel'])

    // Skipped on another device before this tab ever saw it: the arbiter learns of the line from the request.
    const t = setup()
    const l2 = line('L2', { outcome: 'skipped' })
    t.arbiter.offer(avatar('t1', [l2]))
    expect(t.speaker.items).toEqual([])
    expect(t.arbiter.replay(avatar('t1', [l2]), l2)).toBe(true)
    t.speaker.start(lineKey('t1', l2))
    t.arbiter.offer(avatar('t1', [l2]))
    expect(t.speaker.stops).toEqual([])
    t.speaker.end(lineKey('t1', l2), 'spoken')
    expect(t.heard).toEqual(['t1 L2 spoken Daniel'])
  })

  it('says nothing new to the server when the line was said before', () => {
    const s = setup()
    const l1 = line('L1', { outcome: 'spoken' })
    s.arbiter.replay(avatar('t1', [l1]), l1)
    s.speaker.start(lineKey('t1', l1))
    s.speaker.end(lineKey('t1', l1), 'spoken')
    expect(s.heard).toEqual([])
  })

  it('stops when the line is skipped on another device while it is said here', () => {
    const s = setup({ voice: false })
    const l1 = line('L1')
    const key = captioned(s, l1)
    s.arbiter.replay(avatar('t1', [{ ...l1, outcome: 'shown' }]), l1)
    s.speaker.start(key)
    s.arbiter.settled('t1', { ...l1, outcome: 'skipped' })
    expect(s.speaker.stops).toEqual([key])
    expect(s.heard).toEqual(['t1 L1 shown'])
  })

  it('hands the caption that is on screen to the speaker again, to be started over aloud', () => {
    const s = setup({ voice: false })
    const l1 = line('L1')
    const key = lineKey('t1', l1)
    s.arbiter.offer(avatar('t1', [l1]))
    s.speaker.start(key, 'silent')
    expect(s.arbiter.replay(avatar('t1', [l1]), l1)).toBe(true)
    // The caption is not stopped first: the speaker decides whether it can say it.
    expect(s.speaker.stops).toEqual([])
    expect(s.speaker.items.map((i) => [i.lineId, i.mode])).toEqual([['L1', 'silent'], ['L1', 'voice']])
    // It can: the voice starts, and the line ends as a spoken one.
    s.speaker.start(key)
    s.speaker.end(key, 'spoken')
    expect(s.heard).toEqual(['t1 L1 shown', 't1 L1 spoken Daniel'])

    // It cannot (a microphone is open): the caption stays as it was and ends as one.
    const t = setup({ voice: false })
    const l2 = line('L2')
    const k2 = lineKey('t1', l2)
    t.arbiter.offer(avatar('t1', [l2]))
    t.speaker.start(k2, 'silent')
    t.arbiter.replay(avatar('t1', [l2]), l2)
    // A permission request opens meanwhile: a caption on screen is left alone, as always.
    t.world.blocked.add('t1')
    t.arbiter.setState(t.state())
    expect(t.speaker.stops).toEqual([])
    t.speaker.end(k2, 'shown')
    expect(t.heard).toEqual(['t1 L2 shown'])
    expect(t.speaker.items).toHaveLength(2)
  })

  it('gives a waiting caption the voice, in its place in the queue', () => {
    const s = setup({ voice: false })
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    expect(s.arbiter.replay(avatar('t1', [l1]), l1)).toBe(true)
    expect(s.speaker.items.map((i) => [i.key, i.mode])).toEqual([[lineKey('t1', l1), 'silent'], [lineKey('t1', l1), 'voice']])
    // Asked twice: once is enough.
    expect(s.arbiter.replay(avatar('t1', [l1]), l1)).toBe(true)
    expect(s.speaker.items).toHaveLength(2)
  })

  it('leaves a line alone that is being said here already', async () => {
    const s = setup()
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer(YES)
    expect(s.arbiter.replay(avatar('t1', [l1]), l1)).toBe(true)
    s.speaker.start(lineKey('t1', l1))
    expect(s.arbiter.replay(avatar('t1', [l1]), l1)).toBe(true)
    expect(s.speaker.items).toHaveLength(1)
    // It keeps its lease, and reports as the line it is.
    expect(s.speaker.last.confirm).toBeTypeOf('function')
    s.speaker.end(lineKey('t1', l1), 'spoken')
    expect(s.heard).toEqual(['t1 L1 spoken Daniel'])
    expect(s.released()).toEqual([])
  })

  it('takes a line whose claim is on its way, and gives back the yes that comes late', async () => {
    const s = setup()
    s.world.idle = 60_000
    const l1 = line('L1')
    const l2 = line('L2')
    // One is still waiting to ask, the other has asked.
    s.arbiter.offer(avatar('t1', [l1]))
    s.clock.advance(300)
    s.arbiter.offer(avatar('t2', [l2]))
    expect(s.asked()).toHaveLength(1)
    s.arbiter.replay(avatar('t1', [l1]), l1)
    s.arbiter.replay(avatar('t2', [l2]), l2)
    expect(s.speaker.items.map((i) => [i.lineId, i.mode, i.confirm])).toEqual([['L1', 'voice', undefined], ['L2', 'voice', undefined]])
    s.asked()[0].resolve(YES)
    await flush()
    expect(s.released()).toEqual(['t1 L1'])
    s.clock.advance(5000)
    expect(s.asked()).toHaveLength(1)
    expect(s.speaker.items).toHaveLength(2)
    expect(s.clock.pending()).toBe(0)
  })

  it('does nothing for an avatar that is hidden, off or quiet, and for what is no agent line', () => {
    const l1 = line('L1')
    const hidden = setup({ show: false })
    expect(hidden.arbiter.replay(avatar('t1', [l1]), l1)).toBe(false)
    const elsewhere = setup()
    elsewhere.world.hidden.add('t1')
    expect(elsewhere.arbiter.replay(avatar('t1', [l1]), l1)).toBe(false)
    // Rule 6: an avatar whose session needs the user says nothing, whoever asks.
    const blocked = setup()
    blocked.world.blocked.add('t1')
    expect(blocked.arbiter.replay(avatar('t1', [l1]), l1)).toBe(false)
    const s = setup()
    expect(s.arbiter.replay(avatar('t1', [l1], { on: false }), l1)).toBe(false)
    expect(s.arbiter.replay(avatar('t1', []), line('L2', { from: 'user' }))).toBe(false)
    for (const bad of [null, undefined, 7, {}, { id: 'L3', from: 'agent' }]) expect(s.arbiter.replay(avatar('t1', []), bad as unknown as Line)).toBe(false)
    for (const bad of [null, undefined, 7, {}, { terminalId: 7 }]) expect(s.arbiter.replay(bad as unknown as AvatarInfo, l1)).toBe(false)
    for (const x of [hidden, elsewhere, blocked, s]) {
      expect(x.speaker.items).toEqual([])
      expect(x.claims).toEqual([])
      expect(x.heard).toEqual([])
    }
    s.arbiter.dispose()
    expect(s.arbiter.replay(avatar('t1', [l1]), l1)).toBe(false)
  })

  it('stops with its session like any other line', () => {
    const s = setup({ voice: false })
    const l1 = line('L1')
    const key = captioned(s, l1)
    s.arbiter.replay(avatar('t1', [l1]), l1)
    s.speaker.start(key)
    // A permission request opens: the avatar stops talking, and that is no skip by the user.
    s.world.blocked.add('t1')
    s.arbiter.setState(s.state())
    expect(s.speaker.stops).toEqual([key])
    expect(s.heard).toEqual(['t1 L1 shown'])
    // Forgotten with its avatar, too.
    const t = setup({ voice: false })
    const l2 = line('L2')
    const k2 = captioned(t, l2)
    t.arbiter.replay(avatar('t1', [l2]), l2)
    t.speaker.start(k2)
    t.arbiter.forget('t1')
    expect(t.speaker.stops).toEqual([k2])
    expect(t.heard).toEqual(['t1 L2 shown'])
  })
})

describe('createArbiter: what it remembers', () => {
  it('forgets a line once the record no longer lists it', () => {
    const s = setup({ voice: false })
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    s.speaker.end(lineKey('t1', l1), 'shown')
    // Still listed, without an outcome yet (the acknowledgement is on its way): not presented again.
    s.arbiter.offer(avatar('t1', [l1]))
    expect(s.speaker.items).toHaveLength(1)
    // Thirty newer lines pushed it out of the record.
    s.arbiter.offer(avatar('t1', [line('L2', { outcome: 'spoken' })]))
    s.arbiter.offer(avatar('t1', [l1]))
    expect(s.speaker.items).toHaveLength(2)
  })

  it('keeps a line that is still in flight although the record dropped it', async () => {
    const s = setup()
    const l1 = line('L1')
    s.arbiter.offer(avatar('t1', [l1]))
    await s.answer(YES)
    s.speaker.start(lineKey('t1', l1))
    s.arbiter.offer(avatar('t1', []))
    s.speaker.end(lineKey('t1', l1), 'spoken')
    expect(s.heard).toEqual(['t1 L1 spoken Daniel'])
  })

  it('remembers a bounded number of avatars, the ones offered longest ago go first', () => {
    const s = setup({ voice: false })
    const lines = new Map<string, Line>()
    for (let i = 0; i < 400; i++) {
      const l = line('L1')
      lines.set(`t${i}`, l)
      s.arbiter.offer(avatar(`t${i}`, [l]))
      s.speaker.end(lineKey(`t${i}`, l), 'shown')
    }
    expect(s.speaker.items).toHaveLength(400)
    // A recent one is remembered: its line is not presented a second time.
    s.arbiter.offer(avatar('t399', [lines.get('t399') as Line]))
    s.arbiter.offer(avatar('t300', [lines.get('t300') as Line]))
    expect(s.speaker.items).toHaveLength(400)
    // The first ones were let go to make room: the arbiter knows nothing of them any more.
    s.arbiter.offer(avatar('t0', [lines.get('t0') as Line]))
    expect(s.speaker.items).toHaveLength(401)
    expect(s.clock.pending()).toBe(0)
  })

  it('never lets go of an avatar with a line in flight to make room', () => {
    const s = setup({ voice: false })
    const first = line('L1')
    s.arbiter.offer(avatar('busy', [first]))
    s.speaker.start(lineKey('busy', first), 'silent')
    for (let i = 0; i < 300; i++) {
      const l = line('L1')
      s.arbiter.offer(avatar(`t${i}`, [l]))
      s.speaker.end(lineKey(`t${i}`, l), 'shown')
    }
    s.arbiter.offer(avatar('busy', [first]))
    expect(s.speaker.items.filter((i) => i.terminalId === 'busy')).toHaveLength(1)
    s.speaker.end(lineKey('busy', first), 'shown')
    expect(s.heard.filter((h) => h.startsWith('busy'))).toEqual(['busy L1 shown'])
  })

  it('leaves no timer behind after a burst of lines', async () => {
    const s = setup()
    for (let i = 0; i < 40; i++) {
      const l = line(`L${i}`)
      s.arbiter.offer(avatar(`t${i % 5}`, [l]))
      s.clock.advance(0)
      const c = s.asked().at(-1) as Claim
      if (i % 4 === 0) c.reject(new Error('offline'))
      else if (i % 4 === 1) c.resolve(NO)
      else if (i % 4 === 2) c.resolve(YES)
      // Every fourth claim never answers.
      await flush()
      s.clock.advance(CLAIM_TIMEOUT_MS)
      const key = lineKey(`t${i % 5}`, l)
      s.speaker.start(key, i % 4 === 2 ? 'browser' : 'silent')
      s.speaker.end(key, i % 4 === 2 ? 'spoken' : 'shown')
    }
    expect(s.speaker.items).toHaveLength(40)
    expect(s.heard).toHaveLength(40)
    expect(s.heard.filter((h) => h.includes('spoken'))).toHaveLength(10)
    expect(s.clock.pending()).toBe(0)
    expect(s.speaker.held.size).toBe(0)
  })
})

describe('createArbiter with the real speaker', () => {
  class Utterance implements UtteranceLike {
    voice: unknown = null
    lang = ''
    pitch = 1
    rate = 1
    volume = 1
    onstart: (() => void) | null = null
    onend: (() => void) | null = null
    onerror: ((e: { error?: string }) => void) | null = null
    onboundary: ((e: { charIndex: number }) => void) | null = null
    text: string
    constructor(text: string) {
      this.text = text
    }
  }

  function world(voice = true) {
    const spoken: Utterance[] = []
    let cancels = 0
    const voices: VoiceLike[] = [{ name: 'Daniel', lang: 'en-GB', localService: true, default: true }]
    const synth: SynthLike = { speak: (u) => void spoken.push(u as Utterance), cancel: () => void cancels++, resume: () => {}, getVoices: () => voices, addEventListener: () => {}, removeEventListener: () => {} }
    const clock = fakeClock(1000)
    let mic = false
    const speaker = createSpeaker(
      { synth, utterance: (text) => new Utterance(text), now: clock.now, setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout, unlocked: () => true, micLive: () => mic },
      { voice, volume: 1, speed: 1, online: false },
      { start: () => {}, chunk: () => {}, end: () => {}, locked: () => {} },
    )
    const claims: Claim[] = []
    const heard: string[] = []
    const host: ArbiterHost = {
      tab: 'tab-A',
      now: clock.now,
      setTimeout: clock.setTimeout,
      clearTimeout: clock.clearTimeout,
      visible: () => true,
      idleMs: () => 0,
      claim: (terminalId, lineId, body) => new Promise((resolve, reject) => claims.push({ terminalId, lineId, release: body.release === true, resolve, reject })),
      heard: (terminalId, lineId, outcome, v) => {
        heard.push([terminalId, lineId, outcome, v].filter((x) => x !== undefined).join(' '))
        return Promise.resolve()
      },
    }
    const state: ArbiterState = { voice, unlocked: true, show: true, shown: () => true, blocked: () => false, onScreen: () => false }
    const arbiter = createArbiter(host, speaker, state)
    return { clock, spoken, cancels: () => cancels, speaker, claims, heard, arbiter, state, setMic: (on: boolean) => void (mic = on) }
  }

  it('says a line it won and tells the server which voice said it', async () => {
    const w = world()
    w.arbiter.offer(avatar('t1', [line('L1', { text: 'The build is green.' })]))
    w.clock.advance(0)
    w.claims[0].resolve(YES)
    await flush()
    expect(w.spoken.map((u) => u.text)).toEqual(['The build is green.'])
    w.spoken[0].onstart?.()
    w.clock.advance(1200)
    w.spoken[0].onend?.()
    expect(w.heard).toEqual(['t1 L1 spoken Daniel'])
    expect(w.claims.filter((c) => c.release)).toEqual([])
    expect(w.clock.pending()).toBe(0)
  })

  it('shows a line another tab won, and acknowledges the caption', async () => {
    const w = world()
    w.arbiter.offer(avatar('t1', [line('L1')]))
    w.clock.advance(0)
    w.claims[0].resolve(NO)
    await flush()
    expect(w.spoken).toEqual([])
    expect(w.heard).toEqual(['t1 L1 shown'])
    expect(w.speaker.current()?.terminalId).toBe('t1')
    w.clock.advance(SILENT_MIN_MS)
    expect(w.speaker.current()).toBeNull()
    expect(w.heard).toEqual(['t1 L1 shown'])
  })

  it('renews the lease of a line that waited behind another, and shows it when the lease is lost', async () => {
    const w = world()
    w.arbiter.offer(avatar('t1', [line('L1')]))
    w.clock.advance(0)
    w.claims[0].resolve(YES)
    await flush()
    w.spoken[0].onstart?.()
    w.arbiter.offer(avatar('t2', [line('L2')]))
    w.clock.advance(0)
    w.claims[1].resolve(YES)
    await flush()
    // The first line takes a while; the second one's lease is old by then.
    w.clock.advance(RENEW_FRESH_MS + 500)
    w.spoken[0].onend?.()
    expect(w.heard).toEqual(['t1 L1 spoken Daniel'])
    expect(w.claims).toHaveLength(3)
    expect(w.spoken).toHaveLength(1)
    w.claims[2].resolve(NO)
    await flush()
    expect(w.spoken).toHaveLength(1)
    expect(w.heard).toEqual(['t1 L1 spoken Daniel', 't2 L2 shown'])
    w.clock.advance(SILENT_MIN_MS)
    expect(w.speaker.current()).toBeNull()
    expect(w.clock.pending()).toBe(0)
  })

  it('renews the lease as each chunk of a long line starts, and while a chunk takes long', async () => {
    const w = world()
    w.arbiter.offer(avatar('t1', [line('L1', { text: 'One two three four five six seven eight nine ten. '.repeat(8).trim() })]))
    w.clock.advance(0)
    w.claims[0].resolve(YES)
    await flush()
    expect(w.spoken).toHaveLength(1)
    // The lease is fresh when the first chunk starts.
    w.spoken[0].onstart?.()
    expect(w.claims).toHaveLength(1)
    for (const i of [1, 2]) {
      w.clock.advance(5000)
      w.spoken[i - 1].onend?.()
      w.spoken[i].onstart?.()
      expect(w.claims.map((c) => c.release)).toEqual(i === 1 ? [false, false] : [false, false, false])
      w.claims[i].resolve(YES)
      await flush()
    }
    // The last chunk is slow: half the 16 s lease after the last renewal, it is renewed again.
    w.clock.advance(8000)
    expect(w.claims.map((c) => c.release)).toEqual([false, false, false, false])
    w.claims[3].resolve(YES)
    await flush()
    w.clock.advance(1000)
    w.spoken[2].onend?.()
    expect(w.spoken).toHaveLength(3)
    expect(w.heard).toEqual(['t1 L1 spoken Daniel'])
    expect(w.claims.filter((c) => c.release)).toEqual([])
    expect(w.clock.pending()).toBe(0)
  })

  it('reports a line that a microphone cut short as skipped', async () => {
    const w = world()
    w.arbiter.offer(avatar('t1', [line('L1', { text: 'This one takes a little longer to say.' })]))
    w.clock.advance(0)
    w.claims[0].resolve(YES)
    await flush()
    w.spoken[0].onstart?.()
    w.clock.advance(400)
    w.setMic(true)
    w.clock.advance(200)
    expect(w.cancels()).toBe(1)
    expect(w.heard).toEqual(['t1 L1 skipped'])
  })

  it('acknowledges nothing for the line that would have been next when the page goes away', async () => {
    const w = world(false)
    // Two captions: one on screen, one waiting behind it.
    w.arbiter.offer(avatar('t1', [line('L1')]))
    w.arbiter.offer(avatar('t2', [line('L2')]))
    expect(w.heard).toEqual(['t1 L1 shown'])
    w.arbiter.release()
    // Stopping the first lets the speaker start the second for an instant: that is no caption anyone saw.
    expect(w.heard).toEqual(['t1 L1 shown'])
    expect(w.speaker.current()).toBeNull()
    expect(w.claims).toEqual([])
  })

  it('says the caption on screen aloud when the user asks to hear it, and one that has left the screen too', () => {
    const w = world(false)
    const l1 = line('L1', { text: 'The build is green.' })
    w.arbiter.offer(avatar('t1', [l1]))
    expect(w.heard).toEqual(['t1 L1 shown'])
    expect(w.spoken).toEqual([])
    // "Hear it": the speaker gets the choice first, then the line is asked for.
    w.speaker.setPrefs({ voice: true, volume: 1, speed: 1, online: false })
    expect(w.arbiter.replay(avatar('t1', [{ ...l1, outcome: 'shown' }]), l1)).toBe(true)
    expect(w.spoken.map((u) => u.text)).toEqual(['The build is green.'])
    w.spoken[0].onstart?.()
    w.clock.advance(1200)
    w.spoken[0].onend?.()
    // Never "skipped" in between: the caption gave way to its own voice.
    expect(w.heard).toEqual(['t1 L1 shown', 't1 L1 spoken Daniel'])
    expect(w.speaker.current()).toBeNull()

    const l2 = line('L2', { text: 'Tests pass.' })
    w.speaker.setPrefs({ voice: false, volume: 1, speed: 1, online: false })
    w.arbiter.offer(avatar('t2', [l2]))
    w.clock.advance(SILENT_MIN_MS)
    expect(w.speaker.current()).toBeNull()
    w.speaker.setPrefs({ voice: true, volume: 1, speed: 1, online: false })
    expect(w.arbiter.replay(avatar('t2', [{ ...l2, outcome: 'shown' }]), l2)).toBe(true)
    expect(w.spoken.map((u) => u.text)).toEqual(['The build is green.', 'Tests pass.'])
    w.spoken[1].onstart?.()
    w.clock.advance(800)
    w.spoken[1].onend?.()
    expect(w.heard).toEqual(['t1 L1 shown', 't1 L1 spoken Daniel', 't2 L2 shown', 't2 L2 spoken Daniel'])
    expect(w.claims).toEqual([])
    expect(w.cancels()).toBe(0)
    expect(w.clock.pending()).toBe(0)
  })

  it('stops speech at once on release, and the server only hears of the lease', async () => {
    const w = world()
    w.arbiter.offer(avatar('t1', [line('L1')]))
    w.clock.advance(0)
    w.claims[0].resolve(YES)
    await flush()
    w.spoken[0].onstart?.()
    w.arbiter.release()
    expect(w.cancels()).toBe(1)
    expect(w.heard).toEqual([])
    expect(w.claims.filter((c) => c.release).map((c) => c.lineId)).toEqual(['L1'])
    expect(w.speaker.current()).toBeNull()
  })
})
