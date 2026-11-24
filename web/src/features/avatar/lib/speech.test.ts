// The speaker with a synthesizer the test drives by hand, as a browser would: utterances
// that start late, never start, never end, fail, or are interrupted from elsewhere.

import { describe, expect, it } from 'vitest'
import { fakeClock, flush } from './fixtures'
import {
  CANCEL_GAP_MS,
  CONFIRM_WAIT_MS,
  END_WATCHDOG_MS,
  SILENT_MIN_MS,
  START_WATCHDOG_MS,
  VOICES_WAIT_MS,
  WAIT_SILENT_MS,
  browserSpeech,
  chunkLimit,
  chunkText,
  createSpeaker,
  matchChoice,
  mouthTimeline,
  pickVoice,
  pitchFor,
  usableVoices,
} from './speech'
import type { SpeakItem, SpeakPrefs, SpeakerHost, SynthLike, UtteranceLike, VoiceLike } from './speech'
import { stableHash } from './types'

const voice = (name: string, lang = 'en-US', localService = true): VoiceLike => ({ name, lang, localService, default: false })

class FakeUtterance implements UtteranceLike {
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

/** A `speechSynthesis` that does nothing by itself: the test fires the events. */
class FakeSynth implements SynthLike {
  spoken: FakeUtterance[] = []
  calls: string[] = []
  voices: VoiceLike[] = [voice('Daniel', 'en-GB'), voice('Samantha', 'en-US')]
  listeners = new Set<() => void>()
  failSpeak: Error | null = null
  speak(u: UtteranceLike): void {
    this.calls.push('speak')
    if (this.failSpeak) throw this.failSpeak
    this.spoken.push(u as FakeUtterance)
  }
  cancel(): void {
    this.calls.push('cancel')
  }
  resume(): void {
    this.calls.push('resume')
  }
  getVoices(): VoiceLike[] {
    return this.voices
  }
  addEventListener(_type: 'voiceschanged', cb: () => void): void {
    this.listeners.add(cb)
  }
  removeEventListener(_type: 'voiceschanged', cb: () => void): void {
    this.listeners.delete(cb)
  }
  get last(): FakeUtterance {
    return this.spoken[this.spoken.length - 1]
  }
  count(call: string): number {
    return this.calls.filter((c) => c === call).length
  }
}

const PREFS: SpeakPrefs = { voice: true, volume: 1, speed: 1, online: false }

function setup(prefs: Partial<SpeakPrefs> = {}, opts: { synth?: FakeSynth | null } = {}) {
  const clock = fakeClock(1000)
  const synth = opts.synth === undefined ? new FakeSynth() : opts.synth
  const events: string[] = []
  const state = { unlocked: true, mic: false }
  const host: SpeakerHost = {
    synth,
    utterance: (text) => new FakeUtterance(text),
    now: clock.now,
    setTimeout: clock.setTimeout,
    clearTimeout: clock.clearTimeout,
    unlocked: () => state.unlocked,
    micLive: () => state.mic,
  }
  const speaker = createSpeaker(host, { ...PREFS, ...prefs }, {
    start: (key, engine, chunks, voiceName) => events.push(`start ${key} ${engine} ${voiceName} ${chunks.length}`),
    chunk: (key, index) => events.push(`chunk ${key} ${index}`),
    end: (key, outcome) => events.push(`end ${key} ${outcome}`),
    locked: () => events.push('locked'),
  })
  /** The synthesizer reports the start and the end of the utterance it holds, after `ms` of speech. */
  const sayIt = (ms = 500) => {
    const u = (synth as FakeSynth).last
    u.onstart?.()
    clock.advance(ms)
    u.onend?.()
  }
  return { clock, synth: synth as FakeSynth, events, state, host, speaker, sayIt }
}

let n = 0
const item = (over: Partial<SpeakItem> = {}): SpeakItem => {
  const terminalId = over.terminalId ?? 't1'
  const lineId = over.lineId ?? `L${++n}`
  return { key: `${terminalId}:${lineId}`, terminalId, lineId, text: 'Hello there.', lang: 'en-GB', voice: {}, seed: terminalId, mode: 'voice', priority: 0, ...over }
}

describe('chunkLimit', () => {
  it('is ten seconds of speech, at most 200 characters and at least 60', () => {
    expect(chunkLimit(15)).toBe(150)
    expect(chunkLimit(10.59)).toBe(105)
    expect(chunkLimit(30)).toBe(200)
    expect(chunkLimit(3)).toBe(60)
    expect(chunkLimit(0)).toBe(150)
    expect(chunkLimit(-4)).toBe(150)
    expect(chunkLimit(Number.NaN)).toBe(150)
    expect(chunkLimit(Infinity)).toBe(150)
  })
})

describe('chunkText', () => {
  it('leaves a short line whole and collapses white space', () => {
    expect(chunkText('The build is green.', 150)).toEqual(['The build is green.'])
    expect(chunkText('  The   build\n is\tgreen.  ', 150)).toEqual(['The build is green.'])
    expect(chunkText('', 150)).toEqual([])
    expect(chunkText('   ', 150)).toEqual([])
    expect(chunkText(undefined as unknown as string, 150)).toEqual([])
  })

  it('cuts at sentence ends first, then at commas, then at spaces, then inside a word', () => {
    expect(chunkText('One two three. Four five six, seven eight nine ten.', 30)).toEqual(['One two three.', 'Four five six,', 'seven eight nine ten.'])
    expect(chunkText('First sentence here. Second one! Third? Fourth and last.', 34)).toEqual(['First sentence here. Second one!', 'Third? Fourth and last.'])
    expect(chunkText('alpha beta, gamma delta epsilon zeta eta theta', 24)).toEqual(['alpha beta,', 'gamma delta epsilon zeta', 'eta theta'])
    expect(chunkText('alpha beta gamma delta', 12)).toEqual(['alpha beta', 'gamma delta'])
    expect(chunkText('abcdefghijklmnopqrstuvwxyz', 10)).toEqual(['abcdefghij', 'klmnopqrst', 'uvwxyz'])
    // A full stop inside a number or a name is no sentence end.
    expect(chunkText('Version 2.31 of node.js is here and it is fast', 20)).toEqual(['Version 2.31 of', 'node.js is here and', 'it is fast'])
  })

  it('never makes a chunk longer than asked, and loses no word', () => {
    const text = 'The tests pass now, all four hundred of them; the flaky one was a race in the watcher. I fixed it by waiting for the first event! Shall I open the pull request, or do you want to look at the diff first? Either is fine.'
    for (const max of [1, 7, 20, 60, 100, 150, 200]) {
      const chunks = chunkText(text, max)
      for (const chunk of chunks) expect(chunk.length, `max ${max}`).toBeLessThanOrEqual(max)
      expect(chunks.join('').replace(/\s/g, ''), `max ${max}`).toBe(text.replace(/\s/g, ''))
    }
    expect(chunkText(text, 150)).toHaveLength(2)
    expect(chunkText('abc', Number.NaN)).toEqual(['abc'])
    expect(chunkText('abc', 0)).toEqual(['a', 'b', 'c'])
  })

  it('does not cut a character in two', () => {
    const chunks = chunkText('😀'.repeat(12), 5)
    expect(chunks.join('')).toBe('😀'.repeat(12))
    for (const chunk of chunks) expect([...chunk].every((ch) => ch === '😀')).toBe(true)
  })
})

describe('matchChoice', () => {
  it('finds the choice an answer equals, case and punctuation aside', () => {
    const choices = ['Yes, push it', 'No', 'Show me the diff first!']
    expect(matchChoice('yes push it', choices)).toBe(0)
    expect(matchChoice('  YES, PUSH IT.  ', choices)).toBe(0)
    expect(matchChoice('No.', choices)).toBe(1)
    expect(matchChoice('show me   the diff first', choices)).toBe(2)
    expect(matchChoice('yes', choices)).toBeNull()
    expect(matchChoice('no way', choices)).toBeNull()
    expect(matchChoice('', choices)).toBeNull()
    expect(matchChoice('?!', choices)).toBeNull()
    expect(matchChoice('no', [])).toBeNull()
  })

  it('never guesses between two choices that read the same', () => {
    expect(matchChoice('yes', ['Yes', 'yes!'])).toBeNull()
    expect(matchChoice('yes', ['Yes', 'No', 7 as unknown as string])).toBe(0)
    expect(matchChoice(7 as unknown as string, ['7'])).toBeNull()
    expect(matchChoice('x', 'x' as unknown as string[])).toBeNull()
  })
})

describe('voices', () => {
  const all = [voice('Daniel', 'en-GB'), voice('Samantha', 'en-US'), voice('Google UK English Female', 'en-GB', false), voice('Anna', 'de-DE'), voice('Google Deutsch', 'de-DE', false), voice('Karen', 'en_AU')]
  const taken = (o: Record<string, number> = {}) => new Map(Object.entries(o))

  it('uses the voices on the device, and online ones only when allowed', () => {
    expect(usableVoices(all, { online: false }).map((v) => v.name)).toEqual(['Daniel', 'Samantha', 'Anna', 'Karen'])
    expect(usableVoices(all, { online: true })).toHaveLength(6)
    expect(usableVoices([null, 7, { name: 'x' }, voice('ok')] as unknown as VoiceLike[], { online: true }).map((v) => v.name)).toEqual(['ok'])
    expect(usableVoices(null as unknown as VoiceLike[], { online: true })).toEqual([])
  })

  it('takes the first preferred fragment that matches a voice of the language', () => {
    const pool = usableVoices(all, { online: true })
    expect(pickVoice(pool, { lang: 'en-GB', prefer: ['Zira', 'female', 'Daniel'] }, 't1', { taken: taken() })?.name).toBe('Google UK English Female')
    expect(pickVoice(pool, { lang: 'en-US', prefer: ['DANIEL'] }, 't1', { taken: taken() })?.name).toBe('Daniel')
    // A fragment that only matches a voice of another language is no match.
    expect(pickVoice(pool, { lang: 'en-US', prefer: ['Anna', 'Deutsch'] }, 't1', { taken: taken() })?.lang.startsWith('en')).toBe(true)
    expect(pickVoice(pool, { lang: 'en-US', prefer: ['', '  ', 7 as unknown as string] }, 't1', { taken: taken() })).not.toBeNull()
  })

  it('else takes a voice of the exact language first, the least used one', () => {
    const pool = usableVoices(all, { online: false })
    expect(pickVoice(pool, { lang: 'en-US' }, 't1', { taken: taken() })?.name).toBe('Samantha')
    expect(pickVoice(pool, { lang: 'en-GB' }, 't1', { taken: taken() })?.name).toBe('Daniel')
    expect(pickVoice(pool, { lang: 'en-AU' }, 't1', { taken: taken() })?.name).toBe('Karen')
    expect(pickVoice(pool, { lang: 'de' }, 't1', { taken: taken() })?.name).toBe('Anna')
    // No voice of en-NZ: any English one, the one fewest others use.
    expect(pickVoice(pool, { lang: 'en-NZ' }, 't1', { taken: taken({ Daniel: 1, Samantha: 2, Karen: 1 }) })?.name).toMatch(/Daniel|Karen/)
    expect(pickVoice(pool, { lang: 'en-NZ' }, 't1', { taken: taken({ Daniel: 1, Samantha: 2 }) })?.name).toBe('Karen')
    expect(pickVoice(pool, {}, 't1', { taken: taken() })?.name).toBe('Samantha')
    expect(pickVoice(pool, { lang: 'en-GB' }, 't1', { lang: 'de-DE', taken: taken() })?.name).toBe('Anna')
  })

  it('decides ties by a stable hash of the seed, and spreads avatars over the voices', () => {
    const pool = [voice('A'), voice('B'), voice('C')]
    const pick = (seed: string, t = taken()) => pickVoice(pool, {}, seed, { taken: t })?.name
    expect(pick('t1')).toBe(pick('t1'))
    expect(pick('t1')).toBe(['A', 'B', 'C'][stableHash('t1') % 3])
    const used = new Set<string>()
    for (let i = 0; i < 60; i++) used.add(pick(`terminal-${i}`) ?? '')
    expect([...used].sort()).toEqual(['A', 'B', 'C'])
    // Three avatars one after the other end up with three voices.
    const counts = new Map<string, number>()
    for (const seed of ['x1', 'x2', 'x3']) {
      const name = pick(seed, counts) as string
      counts.set(name, (counts.get(name) ?? 0) + 1)
    }
    expect([...counts.values()]).toEqual([1, 1, 1])
  })

  it('keeps the voice remembered for the terminal while it is still there', () => {
    const pool = [voice('A'), voice('B'), voice('C')]
    expect(pickVoice(pool, {}, 't1', { taken: taken({ B: 5 }), remembered: 'B' })?.name).toBe('B')
    expect(pickVoice(pool, {}, 't1', { taken: taken(), remembered: 'Gone' })).not.toBeNull()
    // The agent's own preference comes before what was remembered.
    expect(pickVoice(pool, { prefer: ['c'] }, 't1', { taken: taken(), remembered: 'B' })?.name).toBe('C')
  })

  it('drops a remembered voice that no longer speaks the avatar’s language, where the browser has one that does', () => {
    // The avatar spoke before it chose a voice of its own (en-US), then designed one in en-GB.
    const pool = [voice('Stub English', 'en-US'), voice('Stub British', 'en-GB')]
    expect(pickVoice(pool, { lang: 'en-GB' }, 't1', { taken: taken(), remembered: 'Stub English' })?.name).toBe('Stub British')
    expect(pickVoice(pool, {}, 't1', { lang: 'en-gb', taken: taken(), remembered: 'Stub English' })?.name).toBe('Stub British')
    // Still the language's: it stays.
    expect(pickVoice(pool, { lang: 'en-US' }, 't1', { taken: taken({ 'Stub English': 3 }), remembered: 'Stub English' })?.name).toBe('Stub English')
    // No voice of the exact language here: one of the base language that was remembered stays.
    expect(pickVoice([voice('Stub English', 'en-US'), voice('Other', 'en-US')], { lang: 'en-GB' }, 't1', { taken: taken(), remembered: 'Stub English' })?.name).toBe('Stub English')
    expect(pickVoice(pool, { lang: 'en' }, 't1', { taken: taken(), remembered: 'Stub British' })?.name).toBe('Stub British')
  })

  it('gives nothing when no usable voice speaks the language', () => {
    expect(pickVoice([voice('Anna', 'de-DE')], { lang: 'en-US' }, 't1', { taken: taken() })).toBeNull()
    expect(pickVoice([], {}, 't1', { taken: taken() })).toBeNull()
    expect(pickVoice(null as unknown as VoiceLike[], {}, 't1', { taken: taken() })).toBeNull()
  })

  it('steps pitches apart when a voice repeats and the agent set none', () => {
    expect(pitchFor({ pitch: 1.2 }, 't1', 3)).toBe(1.2)
    expect(pitchFor({ pitch: 9 }, 't1', 0)).toBe(1.4)
    expect(pitchFor({ pitch: 0.1 }, 't1', 0)).toBe(0.7)
    const steps = [0, 1, 2].map((repeats) => pitchFor({}, 't1', repeats))
    expect(steps[0]).toBeCloseTo(1, 1)
    expect(steps[1]).toBeCloseTo(0.85, 1)
    expect(steps[2]).toBeCloseTo(1.15, 1)
    for (const p of steps) {
      expect(p).toBeGreaterThanOrEqual(0.7)
      expect(p).toBeLessThanOrEqual(1.4)
    }
    // Derived per session: stable, and a little different from one session to the next.
    expect(pitchFor({}, 'abc', 0)).toBe(pitchFor({}, 'abc', 0))
    expect(Math.abs(pitchFor({}, 'abc', 0) - 1)).toBeLessThanOrEqual(0.03)
    expect(new Set(Array.from({ length: 40 }, (_, i) => pitchFor({}, `t${i}`, 0))).size).toBeGreaterThan(3)
    expect(pitchFor({ pitch: Number.NaN }, 't1', 0)).toBeCloseTo(1, 1)
    expect(pitchFor({}, 't1', Number.NaN)).toBeCloseTo(1, 1)
    expect(pitchFor({}, 't1', 4)).toBeCloseTo(0.85, 1)
  })
})

describe('mouthTimeline', () => {
  it('lasts as long as the text takes to say', () => {
    expect(mouthTimeline('abcdefghijklmno', 15).durationMs).toBe(1000)
    expect(mouthTimeline('abcdefghijklmno', 30).durationMs).toBe(500)
    expect(mouthTimeline('', 15).durationMs).toBe(0)
    expect(mouthTimeline('abc', Number.NaN).durationMs).toBe(200)
    expect(mouthTimeline(undefined as unknown as string, 15).at(10)).toBe(0)
  })

  it('opens for vowels, closes for m, b and p, and pauses at punctuation', () => {
    // Ten letters of equal share, one a second each.
    const t = mouthTimeline('ambopelifu', 1)
    const at = (i: number) => t.at(i * 1000 + 500)
    expect(at(0)).toBe(1)
    expect(at(1)).toBe(0)
    expect(at(2)).toBe(0)
    expect(at(3)).toBe(0.85)
    expect(at(4)).toBe(0)
    expect(at(5)).toBe(0.7)
    expect(at(7)).toBe(0.5)
    expect(at(8)).toBe(0.15)
    expect(at(9)).toBe(0.55)
    const pause = mouthTimeline('a. a', 4)
    // The full stop takes more of the second than a letter does, and the mouth is shut for it.
    expect(pause.at(100)).toBe(1)
    expect(pause.at(400)).toBe(0)
    expect(pause.at(950)).toBe(1)
    expect(mouthTimeline('a,a', 3).at(500)).toBe(0)
  })

  it('is closed before the start and after the end, and never out of range', () => {
    const t = mouthTimeline('Hello, world! Это тест. 你好', 15)
    expect(t.at(-1)).toBe(0)
    expect(t.at(t.durationMs)).toBe(0)
    expect(t.at(t.durationMs + 5000)).toBe(0)
    expect(t.at(Number.NaN)).toBe(0)
    let moved = 0
    for (let ms = 0; ms < t.durationMs; ms += 10) {
      const v = t.at(ms)
      expect(v).toBeGreaterThanOrEqual(0)
      expect(v).toBeLessThanOrEqual(1)
      if (v > 0.4) moved++
    }
    expect(moved).toBeGreaterThan(10)
  })

  it('shifts to where the voice says it is', () => {
    const t = mouthTimeline('ambopelifu', 1)
    // The voice reaches the fourth letter (the o) two seconds late.
    t.realign(3, 5000)
    expect(t.at(5500)).toBe(0.85)
    expect(t.at(4500)).toBe(0)
    t.realign(0, 0)
    expect(t.at(500)).toBe(1)
    t.realign(99, 0)
    t.realign(Number.NaN, 0)
    t.realign(2, Infinity)
    expect(t.at(500)).toBe(1)
  })
})

describe('browserSpeech', () => {
  it('takes the synthesizer and the utterance constructor of the window it is given', () => {
    const synth = new FakeSynth()
    const got = browserSpeech({ speechSynthesis: synth, SpeechSynthesisUtterance: FakeUtterance })
    expect(got.synth).toBe(synth)
    const u = got.utterance('Hello.')
    expect(u).toBeInstanceOf(FakeUtterance)
    expect((u as FakeUtterance).text).toBe('Hello.')
  })

  it('gives no synthesizer where the browser has none, so lines become captions', () => {
    for (const win of [null, undefined, {}, { speechSynthesis: new FakeSynth() }, { SpeechSynthesisUtterance: FakeUtterance }, { speechSynthesis: 'yes', SpeechSynthesisUtterance: FakeUtterance }, { speechSynthesis: {}, SpeechSynthesisUtterance: FakeUtterance }]) {
      expect(browserSpeech(win).synth).toBeNull()
    }
    expect(() => browserSpeech({}).utterance('x')).toThrow('no speech synthesis')
    const clock = fakeClock()
    const events: string[] = []
    const speaker = createSpeaker(
      { ...browserSpeech({}), now: clock.now, setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout, unlocked: () => true, micLive: () => false },
      PREFS,
      { start: (key, engine) => events.push(`start ${key} ${engine}`), chunk: () => {}, end: (key, outcome) => events.push(`end ${key} ${outcome}`), locked: () => {} },
    )
    speaker.enqueue(item({ key: 'k1' }))
    clock.advance(SILENT_MIN_MS)
    expect(events).toEqual(['start k1 silent', 'end k1 failed'])
  })
})

describe('createSpeaker: saying a line', () => {
  it('says one utterance, resumed first, with voice, language, pitch, rate and volume set', () => {
    const s = setup({ volume: 0.6, speed: 1.2 })
    s.speaker.enqueue(item({ key: 'k1', voice: { rate: 1.1, pitch: 1.3 }, lang: 'en-GB' }))
    expect(s.synth.calls).toEqual(['resume', 'speak'])
    const u = s.synth.last
    expect(u.text).toBe('Hello there.')
    expect((u.voice as VoiceLike).name).toBe('Daniel')
    expect(u.lang).toBe('en-GB')
    expect(u.pitch).toBe(1.3)
    expect(u.rate).toBeCloseTo(1.32, 9)
    expect(u.volume).toBe(0.6)
    expect(s.speaker.current()).toEqual({ key: 'k1', terminalId: 't1' })
    expect(s.events).toEqual(['start k1 browser Daniel 1'])
    s.sayIt(800)
    expect(s.events).toEqual(['start k1 browser Daniel 1', 'chunk k1 0', 'end k1 spoken'])
    expect(s.speaker.current()).toBeNull()
    // It never cancelled anything: there was nothing of its own to cancel.
    expect(s.synth.count('cancel')).toBe(0)
    expect(s.clock.pending()).toBe(0)
  })

  it('never cancels at start-up or when it has nothing in flight', () => {
    const s = setup()
    s.speaker.stop()
    s.speaker.stop('nothing')
    s.speaker.drop('t1')
    s.speaker.setPrefs({ ...PREFS, voice: false })
    s.speaker.dispose()
    expect(s.synth.calls).toEqual([])
  })

  it('says a long line chunk by chunk, each after the one before has ended', () => {
    const s = setup()
    const text = 'The tests pass now, all four hundred of them; the flaky one was a race in the watcher. I fixed it by waiting for the first event! Shall I open the pull request, or do you want to look at the diff first? Either is fine.'
    s.speaker.enqueue(item({ key: 'k1', text }))
    expect(s.synth.spoken).toHaveLength(1)
    s.sayIt(6000)
    expect(s.synth.spoken).toHaveLength(2)
    expect(s.synth.spoken.map((u) => u.text).join(' ')).toBe(text)
    for (const u of s.synth.spoken) expect(u.text.length).toBeLessThanOrEqual(150)
    s.sayIt(4000)
    expect(s.events).toEqual(['start k1 browser Daniel 2', 'chunk k1 0', 'chunk k1 1', 'end k1 spoken'])
  })

  it('always sets a language, and falls back when the line’s is no language tag', () => {
    const s = setup()
    s.speaker.enqueue(item({ lang: 'en-US; rm -rf', voice: { lang: 'en-GB' } }))
    expect(s.synth.last.lang).toBe('en-GB')
    s.sayIt()
    s.speaker.enqueue(item({ lang: '', voice: {} }))
    expect(s.synth.last.lang).toBe('en-US')
  })

  it('moves the mouth of the avatar that speaks, and of no other', () => {
    const s = setup()
    expect(s.speaker.mouth('t1')).toBe(-1)
    s.speaker.enqueue(item({ text: 'aaaaaaaaaaaaaaa mmmmmmmmmmmmmmm' }))
    // Asked, not yet started: presenting, mouth shut.
    expect(s.speaker.mouth('t1')).toBe(0)
    expect(s.speaker.mouth('t2')).toBe(-1)
    s.synth.last.onstart?.()
    s.clock.advance(300)
    expect(s.speaker.mouth('t1')).toBe(1)
    s.clock.advance(1200)
    expect(s.speaker.mouth('t1')).toBe(0)
    s.synth.last.onend?.()
    expect(s.speaker.mouth('t1')).toBe(-1)
  })

  it('follows the voice’s own word boundaries where it reports them', () => {
    const s = setup()
    s.speaker.enqueue(item({ text: 'mmmmm aaaaa' }))
    s.synth.last.onstart?.()
    s.clock.advance(100)
    expect(s.speaker.mouth('t1')).toBe(0)
    // The voice is already at the second word.
    s.synth.last.onboundary?.({ charIndex: 6 })
    s.clock.advance(50)
    expect(s.speaker.mouth('t1')).toBe(1)
  })
})

describe('createSpeaker: what goes wrong in browsers', () => {
  it('ignores events of an utterance it has given up', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'k1' }))
    const old = s.synth.last
    old.onstart?.()
    s.speaker.stop('k1')
    expect(s.events).toEqual(['start k1 browser Daniel 1', 'chunk k1 0', 'end k1 skipped'])
    expect(s.synth.count('cancel')).toBe(1)
    s.speaker.enqueue(item({ key: 'k2' }))
    // The cancel makes the browser report the old utterance as interrupted, late.
    old.onerror?.({ error: 'interrupted' })
    old.onend?.()
    old.onstart?.()
    old.onboundary?.({ charIndex: 3 })
    // None of that is taken for the new line.
    expect(s.events.slice(3)).toEqual(['start k2 browser Daniel 1'])
    expect(s.speaker.mouth('t1')).toBe(0)
    s.clock.advance(CANCEL_GAP_MS)
    s.sayIt()
    expect(s.events.slice(3)).toEqual(['start k2 browser Daniel 1', 'chunk k2 0', 'end k2 spoken'])
  })

  it('waits 80 ms after its own cancel before it speaks again', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'k1' }))
    s.synth.last.onstart?.()
    s.speaker.enqueue(item({ key: 'k2', terminalId: 't2' }))
    s.speaker.stop('k1')
    expect(s.synth.spoken).toHaveLength(1)
    s.clock.advance(CANCEL_GAP_MS - 1)
    expect(s.synth.spoken).toHaveLength(1)
    s.clock.advance(1)
    expect(s.synth.spoken).toHaveLength(2)
    expect(s.synth.calls).toEqual(['resume', 'speak', 'cancel', 'resume', 'speak'])
  })

  it('gives up a chunk that does not start within 4 s and goes on with the next', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'k1', text: `${'One two three four five six seven eight nine ten. '.repeat(3)}Eleven.`.repeat(2) }))
    expect(s.synth.spoken).toHaveLength(1)
    s.clock.advance(START_WATCHDOG_MS - 1)
    expect(s.synth.count('cancel')).toBe(0)
    s.clock.advance(1)
    expect(s.synth.count('cancel')).toBe(1)
    s.clock.advance(CANCEL_GAP_MS)
    expect(s.synth.spoken).toHaveLength(2)
    // The next one works, and the count of misses starts again: one more miss does not fail the line.
    s.sayIt(3000)
    expect(s.synth.spoken).toHaveLength(3)
    s.clock.advance(START_WATCHDOG_MS)
    expect(s.synth.count('cancel')).toBe(2)
    expect(s.events).toEqual(['start k1 browser Daniel 3', 'chunk k1 1', 'end k1 spoken'])
  })

  it('ignores the late events of a chunk it gave up, while the same line goes on', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'k1', text: 'One two three four five six seven eight nine ten. '.repeat(9) }))
    const stuck = s.synth.last
    s.clock.advance(START_WATCHDOG_MS + CANCEL_GAP_MS)
    expect(s.synth.spoken).toHaveLength(2)
    s.synth.last.onstart?.()
    // Now the browser reports the first utterance after all: started, a boundary, interrupted, ended.
    stuck.onstart?.()
    stuck.onboundary?.({ charIndex: 40 })
    stuck.onerror?.({ error: 'interrupted' })
    stuck.onend?.()
    expect(s.synth.spoken).toHaveLength(2)
    expect(s.events).toEqual(['start k1 browser Daniel 3', 'chunk k1 1'])
    expect(s.speaker.current()?.key).toBe('k1')
    s.clock.advance(3000)
    s.synth.last.onend?.()
    expect(s.synth.spoken).toHaveLength(3)
    s.sayIt(3000)
    expect(s.events).toEqual(['start k1 browser Daniel 3', 'chunk k1 1', 'chunk k1 2', 'end k1 spoken'])
  })

  it('does not call a line spoken of which no chunk ever started', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'k1' }))
    s.clock.advance(START_WATCHDOG_MS)
    expect(s.synth.count('cancel')).toBe(1)
    expect(s.events).toEqual(['start k1 browser Daniel 1', 'end k1 failed'])
  })

  it('gives up a chunk that starts but never ends, six seconds after it should have', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'k1', text: 'abcdefghijklmno' }))
    s.synth.last.onstart?.()
    // Fifteen characters: one second of speech.
    s.clock.advance(1000 + END_WATCHDOG_MS - 1)
    expect(s.events).toEqual(['start k1 browser Daniel 1', 'chunk k1 0'])
    s.clock.advance(1)
    expect(s.synth.count('cancel')).toBe(1)
    // That was the last chunk: the line counts as said.
    expect(s.events.at(-1)).toBe('end k1 spoken')
  })

  it('fails the engine for the line after two chunks in a row were given up, and shows the rest', () => {
    const s = setup()
    const text = `${'One two three four five six seven eight nine ten. '.repeat(3)}`.repeat(3)
    s.speaker.enqueue(item({ key: 'k1', text }))
    s.clock.advance(START_WATCHDOG_MS + CANCEL_GAP_MS)
    expect(s.synth.spoken).toHaveLength(2)
    s.clock.advance(START_WATCHDOG_MS)
    expect(s.synth.count('cancel')).toBe(2)
    // No third utterance: the caption goes on alone.
    s.clock.advance(CANCEL_GAP_MS)
    expect(s.synth.spoken).toHaveLength(2)
    expect(s.events).toEqual(['start k1 browser Daniel 3', 'start k1 silent null 3', 'chunk k1 2'])
    s.clock.advance(60_000)
    expect(s.events.at(-1)).toBe('end k1 failed')
    // The next line gets a fresh try.
    s.speaker.enqueue(item({ key: 'k2' }))
    expect(s.synth.spoken).toHaveLength(3)
  })

  it('takes "interrupted" and "canceled" for a stop, never for a failure', () => {
    for (const error of ['interrupted', 'canceled']) {
      const s = setup()
      s.speaker.enqueue(item({ key: 'k1' }))
      s.synth.last.onstart?.()
      s.synth.last.onerror?.({ error })
      expect(s.events, error).toEqual(['start k1 browser Daniel 1', 'chunk k1 0', 'end k1 skipped'])
      expect(s.synth.count('cancel')).toBe(0)
    }
  })

  it('locks on "not-allowed": the line is a caption, and so is every line until it is unlocked', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'k1' }))
    s.synth.last.onerror?.({ error: 'not-allowed' })
    expect(s.events).toEqual(['start k1 browser Daniel 1', 'locked', 'start k1 silent null 1', 'chunk k1 0'])
    s.clock.advance(SILENT_MIN_MS)
    expect(s.events.at(-1)).toBe('end k1 shown')
    s.speaker.enqueue(item({ key: 'k2' }))
    expect(s.synth.spoken).toHaveLength(1)
    expect(s.events.slice(5)).toEqual(['locked', 'start k2 silent null 1', 'chunk k2 0'])
    s.clock.advance(SILENT_MIN_MS)
    // The user's gesture: one utterance of nothing, and speech works again.
    s.speaker.unlock()
    expect(s.synth.spoken).toHaveLength(2)
    expect(s.synth.last.text).toBe('')
    expect(s.synth.last.volume).toBe(0)
    s.synth.last.onend?.()
    s.speaker.enqueue(item({ key: 'k3' }))
    expect(s.synth.last.text).toBe('Hello there.')
    s.sayIt()
    expect(s.events.at(-1)).toBe('end k3 spoken')
  })

  it('presents silently while the page has had no gesture yet', () => {
    const s = setup()
    s.state.unlocked = false
    s.speaker.enqueue(item({ key: 'k1' }))
    expect(s.synth.calls).toEqual([])
    expect(s.events).toEqual(['locked', 'start k1 silent null 1', 'chunk k1 0'])
    s.clock.advance(SILENT_MIN_MS)
    expect(s.events.at(-1)).toBe('end k1 shown')
    s.state.unlocked = true
    s.speaker.unlock()
    s.speaker.enqueue(item({ key: 'k2' }))
    expect(s.synth.spoken.map((u) => u.text)).toEqual(['', 'Hello there.'])
  })

  it('unlocks without a word when voice is off, and locks again if the gesture was no gesture', () => {
    const off = setup({ voice: false })
    off.speaker.unlock()
    expect(off.synth.calls).toEqual([])
    const s = setup()
    s.speaker.unlock()
    s.synth.last.onerror?.({ error: 'not-allowed' })
    expect(s.events).toEqual(['locked'])
    s.speaker.enqueue(item({ key: 'k1' }))
    expect(s.events.slice(1)).toEqual(['locked', 'start k1 silent null 1', 'chunk k1 0'])
  })

  it('fails the line on any other error, and keeps the caption', () => {
    for (const error of ['synthesis-failed', 'audio-busy', 'network', 'language-unavailable', 'voice-unavailable', 'text-too-long', undefined]) {
      const s = setup()
      s.speaker.enqueue(item({ key: 'k1' }))
      s.synth.last.onerror?.({ error })
      expect(s.events, String(error)).toEqual(['start k1 browser Daniel 1', 'start k1 silent null 1', 'chunk k1 0'])
      s.clock.advance(SILENT_MIN_MS)
      expect(s.events.at(-1)).toBe('end k1 failed')
    }
  })

  it('fails the line when the synthesizer throws', () => {
    const s = setup()
    s.synth.failSpeak = new Error('boom')
    s.speaker.enqueue(item({ key: 'k1' }))
    s.clock.advance(SILENT_MIN_MS)
    expect(s.events).toEqual(['start k1 browser Daniel 1', 'start k1 silent null 1', 'chunk k1 0', 'end k1 failed'])
    expect(s.clock.pending()).toBe(0)
  })

  it('waits up to two seconds for the voices on first use', () => {
    const s = setup()
    s.synth.voices = []
    s.speaker.enqueue(item({ key: 'k1' }))
    expect(s.synth.calls).toEqual([])
    expect(s.synth.listeners.size).toBe(1)
    s.clock.advance(700)
    s.synth.voices = [voice('Late', 'en-GB')]
    for (const cb of s.synth.listeners) cb()
    expect(s.synth.listeners.size).toBe(0)
    expect((s.synth.last.voice as VoiceLike).name).toBe('Late')
    s.sayIt()
    expect(s.events.at(-1)).toBe('end k1 spoken')
  })

  it('fails the line when no voice comes, and does not wait a second time', () => {
    const s = setup()
    s.synth.voices = []
    s.speaker.enqueue(item({ key: 'k1' }))
    s.clock.advance(VOICES_WAIT_MS - 1)
    expect(s.events).toEqual([])
    s.clock.advance(1)
    expect(s.events).toEqual(['start k1 silent null 1', 'chunk k1 0'])
    expect(s.synth.listeners.size).toBe(0)
    s.clock.advance(SILENT_MIN_MS)
    expect(s.events.at(-1)).toBe('end k1 failed')
    s.speaker.enqueue(item({ key: 'k2' }))
    expect(s.events.slice(3)).toEqual(['start k2 silent null 1', 'chunk k2 0'])
  })

  it('does not use an online voice the user did not allow', () => {
    const s = setup()
    s.synth.voices = [voice('Google UK English Female', 'en-GB', false)]
    s.speaker.enqueue(item({ key: 'k1' }))
    expect(s.synth.calls).toEqual([])
    s.clock.advance(SILENT_MIN_MS)
    expect(s.events.at(-1)).toBe('end k1 failed')
    s.speaker.setPrefs({ ...PREFS, online: true })
    s.speaker.enqueue(item({ key: 'k2' }))
    expect((s.synth.last.voice as VoiceLike).name).toBe('Google UK English Female')
  })

  it('stops a line an online voice is saying when the allow is withdrawn, and hands it no further chunk', () => {
    const s = setup({ online: true })
    s.synth.voices = [voice('Google UK English Female', 'en-GB', false)]
    // Three chunks.
    s.speaker.enqueue(item({ key: 'k1', text: 'One two three four five six seven eight nine ten. '.repeat(8) }))
    s.synth.last.onstart?.()
    s.clock.advance(3000)
    // Settings, or the same preference arriving from another window.
    s.speaker.setPrefs({ ...PREFS, online: false })
    const cancels = s.synth.count('cancel')
    const events = [...s.events]
    // The browser ends the chunk it was saying: nothing of the line goes to the online service after the no.
    s.synth.last.onend?.()
    s.clock.advance(60_000)
    expect(s.synth.spoken.map((u) => (u.voice as VoiceLike).localService)).toEqual([false])
    // At once: the chunk being said is cancelled, and the line ends as a stopped one.
    expect(cancels).toBe(1)
    expect(events).toEqual(['start k1 browser Google UK English Female 3', 'chunk k1 0', 'end k1 skipped'])
    expect(s.events).toEqual(events)
    expect(s.clock.pending()).toBe(0)
  })

  it('goes on with a voice on the device when the allow for online voices is withdrawn', () => {
    const s = setup({ online: true })
    s.speaker.enqueue(item({ key: 'k1', text: 'One two three four five six seven eight nine ten. '.repeat(8) }))
    s.synth.last.onstart?.()
    s.speaker.setPrefs({ ...PREFS, online: false })
    expect(s.synth.count('cancel')).toBe(0)
    s.clock.advance(3000)
    s.synth.last.onend?.()
    s.sayIt(3000)
    s.sayIt(3000)
    expect(s.synth.spoken.map((u) => (u.voice as VoiceLike).name)).toEqual(['Daniel', 'Daniel', 'Daniel'])
    expect(s.events).toEqual(['start k1 browser Daniel 3', 'chunk k1 0', 'chunk k1 1', 'chunk k1 2', 'end k1 spoken'])
  })

  it('fails the line in a browser that cannot speak at all', () => {
    const s = setup({}, { synth: null })
    s.speaker.enqueue(item({ key: 'k1' }))
    s.clock.advance(SILENT_MIN_MS)
    expect(s.events).toEqual(['start k1 silent null 1', 'chunk k1 0', 'end k1 failed'])
    s.speaker.unlock()
  })

  it('learns how fast a voice speaks, and waits accordingly for the next chunk’s end', () => {
    const s = setup()
    // Thirty characters in three seconds: ten a second, not fifteen.
    s.speaker.enqueue(item({ key: 'k1', text: 'abcdefghijklmno abcdefghijklmn' }))
    s.sayIt(3000)
    s.speaker.enqueue(item({ key: 'k2', text: 'abcdefghijklmno abcdefghijklmn' }))
    s.synth.last.onstart?.()
    // At fifteen a second the watchdog would bark after 2 + 6 s. It has learned better.
    s.clock.advance(2000 + END_WATCHDOG_MS + 1)
    expect(s.synth.count('cancel')).toBe(0)
    s.clock.advance(1000)
    expect(s.synth.count('cancel')).toBe(1)
  })
})

describe('createSpeaker: the silent engine', () => {
  it('shows a line for as long as it would take to say, at least a second and a half', () => {
    const s = setup({ voice: false })
    s.speaker.enqueue(item({ key: 'k1', text: 'Hi.' }))
    expect(s.events).toEqual(['start k1 silent null 1', 'chunk k1 0'])
    expect(s.synth.calls).toEqual([])
    s.clock.advance(SILENT_MIN_MS - 1)
    expect(s.speaker.current()?.key).toBe('k1')
    s.clock.advance(1)
    expect(s.events.at(-1)).toBe('end k1 shown')
    // Forty-five characters at fifteen a second: three seconds.
    s.speaker.enqueue(item({ key: 'k2', text: 'abcdefghi '.repeat(4) + 'abcde' }))
    s.clock.advance(2999)
    expect(s.speaker.current()?.key).toBe('k2')
    s.clock.advance(1)
    expect(s.speaker.current()).toBeNull()
    // Faster for an avatar that speaks faster, and for a user who likes it faster.
    s.speaker.setPrefs({ ...PREFS, voice: false, speed: 1.5 })
    s.speaker.enqueue(item({ key: 'k3', text: 'abcdefghi '.repeat(8) + 'abcdefghij', voice: { rate: 1.3 } }))
    s.clock.advance(90 / (15 * 1.3 * 1.5) * 1000 - 1)
    expect(s.speaker.current()?.key).toBe('k3')
    s.clock.advance(2)
    expect(s.speaker.current()).toBeNull()
  })

  it('goes through the chunks and moves the mouth, like speech does', () => {
    const s = setup({ voice: false })
    const text = `${'Aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaaa. '.repeat(3)}`.repeat(2)
    s.speaker.enqueue(item({ key: 'k1', text }))
    let opened = 0
    for (let t = 0; t < 30_000 && s.speaker.current(); t += 50) {
      if (s.speaker.mouth('t1') > 0.5) opened++
      expect(s.speaker.mouth('t2')).toBe(-1)
      s.clock.advance(50)
    }
    expect(opened).toBeGreaterThan(50)
    expect(s.events).toEqual(['start k1 silent null 3', 'chunk k1 0', 'chunk k1 1', 'chunk k1 2', 'end k1 shown'])
    expect(s.speaker.mouth('t1')).toBe(-1)
  })

  it('presents silently what it is told to, voice or no voice', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'k1', mode: 'silent' }))
    expect(s.synth.calls).toEqual([])
    s.clock.advance(SILENT_MIN_MS)
    expect(s.events).toEqual(['start k1 silent null 1', 'chunk k1 0', 'end k1 shown'])
  })

  it('ends a line of nothing at once', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'k1', text: '   ' }))
    expect(s.events).toEqual(['end k1 shown'])
    s.speaker.enqueue(item({ key: 'k2', text: 7 as unknown as string }))
    expect(s.events).toEqual(['end k1 shown', 'end k2 shown'])
  })
})

describe('createSpeaker: the microphone and the voice switch', () => {
  it('starts no line aloud while a microphone is live', () => {
    const s = setup()
    s.state.mic = true
    s.speaker.enqueue(item({ key: 'k1' }))
    expect(s.synth.calls).toEqual([])
    s.clock.advance(SILENT_MIN_MS)
    expect(s.events).toEqual(['start k1 silent null 1', 'chunk k1 0', 'end k1 shown'])
  })

  it('stops the line being said when a microphone goes live', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'k1', text: 'A long line that takes a while to say aloud.' }))
    s.synth.last.onstart?.()
    s.clock.advance(1000)
    expect(s.speaker.current()?.key).toBe('k1')
    s.state.mic = true
    s.clock.advance(200)
    expect(s.synth.count('cancel')).toBe(1)
    expect(s.events.at(-1)).toBe('end k1 skipped')
    expect(s.clock.pending()).toBe(0)
  })

  it('stops the line being said when voice is turned off, and shows the next ones', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'k1' }))
    s.speaker.enqueue(item({ key: 'k2', terminalId: 't2' }))
    s.synth.last.onstart?.()
    s.speaker.setPrefs({ ...PREFS, voice: false })
    expect(s.synth.count('cancel')).toBe(1)
    expect(s.events).toEqual(['start k1 browser Daniel 1', 'chunk k1 0', 'end k1 skipped', 'start k2 silent null 1', 'chunk k2 0'])
    expect(s.synth.spoken).toHaveLength(1)
    // A caption is not stopped by the switch.
    s.speaker.setPrefs({ ...PREFS, voice: true })
    s.speaker.setPrefs({ ...PREFS, voice: false })
    expect(s.speaker.current()?.key).toBe('k2')
  })

  it('takes new volume and speed for the next utterance, within their ranges', () => {
    const s = setup()
    s.speaker.setPrefs({ voice: true, volume: 7, speed: 0.1, online: false })
    s.speaker.enqueue(item())
    expect(s.synth.last.volume).toBe(1)
    expect(s.synth.last.rate).toBe(0.7)
    s.sayIt()
    s.speaker.setPrefs({ voice: true, volume: Number.NaN, speed: Number.NaN, online: false })
    s.speaker.enqueue(item({ voice: { rate: 99 } }))
    expect(s.synth.last.volume).toBe(1)
    expect(s.synth.last.rate).toBe(1.3)
  })
})

describe('createSpeaker: the queue', () => {
  it('presents one line at a time, across avatars, in order of arrival', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'a', terminalId: 't1' }))
    s.speaker.enqueue(item({ key: 'b', terminalId: 't2' }))
    s.speaker.enqueue(item({ key: 'c', terminalId: 't3' }))
    expect(s.synth.spoken).toHaveLength(1)
    s.sayIt()
    expect(s.synth.spoken).toHaveLength(2)
    s.sayIt()
    s.sayIt()
    expect(s.events.filter((e) => e.startsWith('end'))).toEqual(['end a spoken', 'end b spoken', 'end c spoken'])
  })

  it('takes an ask’s question first, then the session on screen, then the rest', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'now', terminalId: 't0' }))
    s.speaker.enqueue(item({ key: 'plain', terminalId: 't1', priority: 0 }))
    s.speaker.enqueue(item({ key: 'onscreen', terminalId: 't2', priority: 1 }))
    s.speaker.enqueue(item({ key: 'ask', terminalId: 't3', priority: 2 }))
    s.speaker.enqueue(item({ key: 'plain2', terminalId: 't4', priority: Number.NaN }))
    for (let i = 0; i < 5; i++) s.sayIt()
    expect(s.events.filter((e) => e.startsWith('end')).map((e) => e.split(' ')[1])).toEqual(['now', 'ask', 'onscreen', 'plain', 'plain2'])
  })

  it('keeps one waiting line per avatar: a newer one takes its place, and the old one counts as shown', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'a1', terminalId: 't1' }))
    s.speaker.enqueue(item({ key: 'a2', terminalId: 't1' }))
    s.speaker.enqueue(item({ key: 'b1', terminalId: 't2' }))
    s.speaker.enqueue(item({ key: 'a3', terminalId: 't1' }))
    expect(s.events).toEqual(['start a1 browser Daniel 1', 'end a2 shown'])
    s.sayIt()
    s.sayIt()
    s.sayIt()
    // The line that took a2's place has its own turn, after b1.
    expect(s.events.filter((e) => e.startsWith('end'))).toEqual(['end a2 shown', 'end a1 spoken', 'end b1 spoken', 'end a3 spoken'])
  })

  it('takes the same line twice as one', () => {
    const s = setup()
    const first = item({ key: 'a', terminalId: 't1' })
    s.speaker.enqueue(first)
    s.speaker.enqueue({ ...first })
    const waiting = item({ key: 'b', terminalId: 't2' })
    s.speaker.enqueue(waiting)
    s.speaker.enqueue({ ...waiting, mode: 'silent' })
    s.sayIt()
    // The second offer of b changed how it is presented, not its place.
    expect(s.events).toEqual(['start a browser Daniel 1', 'chunk a 0', 'end a spoken', 'start b silent null 1', 'chunk b 0'])
  })

  it('presents a line silently when it has waited 20 s', () => {
    const s = setup()
    // Three chunks of seven seconds each.
    s.speaker.enqueue(item({ key: 'long', terminalId: 't1', text: 'One two three four five six seven eight nine ten. '.repeat(9) }))
    s.speaker.enqueue(item({ key: 'waits', terminalId: 't2' }))
    s.sayIt(7000)
    s.sayIt(7000)
    s.synth.last.onstart?.()
    s.clock.advance(4000)
    // A line that comes late has not waited long when its turn comes.
    s.speaker.enqueue(item({ key: 'fresh', terminalId: 't3' }))
    s.clock.advance(3000)
    s.synth.last.onend?.()
    expect(s.events.slice(-3)).toEqual(['end long spoken', 'start waits silent null 1', 'chunk waits 0'])
    s.clock.advance(SILENT_MIN_MS)
    expect(s.events.slice(-2)).toEqual(['end waits shown', 'start fresh browser Daniel 1'])
    expect(WAIT_SILENT_MS).toBe(20_000)
  })

  it('gives every one of 20 lines from 3 avatars exactly one end, never two at once', () => {
    const s = setup()
    const keys: string[] = []
    for (let i = 0; i < 20; i++) {
      const it = item({ terminalId: `t${i % 3}`, key: `line${i}`, text: `Line number ${i} is a few words long.` })
      keys.push(it.key)
      s.speaker.enqueue(it)
    }
    // One is being said, one waits for each avatar, and the others have given way.
    expect(s.events.filter((e) => e.startsWith('end'))).toHaveLength(20 - 1 - 3)
    let guard = 0
    while (s.speaker.current() && guard++ < 50) {
      const before = s.synth.spoken.length
      s.sayIt(700)
      expect(s.synth.spoken.length - before).toBeLessThanOrEqual(1)
    }
    const ends = s.events.filter((e) => e.startsWith('end'))
    expect(ends).toHaveLength(20)
    expect(new Set(ends.map((e) => e.split(' ')[1])).size).toBe(20)
    // The first one, then the newest of each avatar, in the order they came.
    expect(ends.filter((e) => e.endsWith('spoken')).map((e) => e.split(' ')[1])).toEqual(['line0', 'line17', 'line18', 'line19'])
    expect(s.events.filter((e) => e.startsWith('start'))).toHaveLength(4)
    expect(s.clock.pending()).toBe(0)
    expect(s.speaker.current()).toBeNull()
  })

  it('stops a line by key, also one that waits, and drops an avatar’s lines', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'a', terminalId: 't1' }))
    s.speaker.enqueue(item({ key: 'b', terminalId: 't2' }))
    s.speaker.enqueue(item({ key: 'c', terminalId: 't3' }))
    s.speaker.stop('b')
    expect(s.events).toEqual(['start a browser Daniel 1', 'end b skipped'])
    s.speaker.stop('nothing')
    s.speaker.drop('t3')
    expect(s.events).toHaveLength(2)
    s.speaker.drop('t1')
    expect(s.events).toEqual(['start a browser Daniel 1', 'end b skipped', 'end a skipped'])
    expect(s.speaker.current()).toBeNull()
    s.clock.advance(5000)
    expect(s.synth.spoken).toHaveLength(1)
  })

  it('asks before it says a line that waited, and shows it when the answer is no, late or an error', async () => {
    const answers: Record<string, () => Promise<boolean>> = {
      yes: () => Promise.resolve(true),
      no: () => Promise.resolve(false),
      error: () => Promise.reject(new Error('offline')),
      never: () => new Promise<boolean>(() => {}),
      throws: () => {
        throw new Error('broken')
      },
      odd: () => Promise.resolve('yes' as unknown as boolean),
    }
    for (const [name, confirm] of Object.entries(answers)) {
      const s = setup()
      s.speaker.enqueue(item({ key: 'k1', confirm }))
      expect(s.synth.calls, name).toEqual([])
      expect(s.speaker.current()?.key).toBe('k1')
      await flush()
      if (name === 'never') s.clock.advance(CONFIRM_WAIT_MS)
      if (name === 'yes') {
        expect(s.events, name).toEqual(['start k1 browser Daniel 1'])
        s.sayIt()
        expect(s.events.at(-1)).toBe('end k1 spoken')
      } else {
        expect(s.events, name).toEqual(['start k1 silent null 1', 'chunk k1 0'])
        s.clock.advance(SILENT_MIN_MS)
        expect(s.events.at(-1), name).toBe('end k1 shown')
      }
      expect(s.clock.pending(), name).toBe(0)
    }
  })

  it('forgets an answer that comes after the line was stopped', async () => {
    const s = setup()
    let answer: (ok: boolean) => void = () => {}
    s.speaker.enqueue(item({ key: 'k1', confirm: () => new Promise<boolean>((resolve) => (answer = resolve)) }))
    s.speaker.stop('k1')
    expect(s.events).toEqual(['end k1 skipped'])
    answer(true)
    await flush()
    expect(s.synth.calls).toEqual([])
    expect(s.events).toEqual(['end k1 skipped'])
  })
})

describe('createSpeaker: a caption the user asks to hear', () => {
  const LONG = `${'Aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaaa. '.repeat(3)}`.repeat(2)

  it('starts the caption on screen over aloud, without ending it first', () => {
    const s = setup({ voice: false })
    const caption = item({ key: 'k1', text: LONG, mode: 'silent' })
    s.speaker.enqueue(caption)
    s.speaker.enqueue(item({ key: 'k2', terminalId: 't2', mode: 'silent' }))
    // Into its second chunk.
    s.clock.advance(11_000)
    expect(s.events).toEqual(['start k1 silent null 3', 'chunk k1 0', 'chunk k1 1'])
    // "Hear it": voice is on from now, and the same line is asked for aloud.
    s.speaker.setPrefs({ ...PREFS, voice: true })
    s.speaker.enqueue({ ...caption, mode: 'voice' })
    expect(s.events.slice(3)).toEqual(['start k1 browser Daniel 3'])
    // No timer of the caption is left: only the watch for a microphone and the watchdog of the first chunk.
    expect(s.clock.pending()).toBe(2)
    expect(s.synth.spoken).toHaveLength(1)
    expect(s.synth.last.text.startsWith('Aaaa')).toBe(true)
    expect(s.speaker.current()?.key).toBe('k1')
    // The caption's own clock is gone: nothing of it ends the line or moves it on.
    s.clock.advance(3000)
    expect(s.events).toHaveLength(4)
    for (let i = 0; i < 3; i++) s.sayIt()
    expect(s.events.slice(4)).toEqual(['chunk k1 0', 'chunk k1 1', 'chunk k1 2', 'end k1 spoken', 'start k2 silent null 1', 'chunk k2 0'])
    s.clock.advance(SILENT_MIN_MS)
    expect(s.events.at(-1)).toBe('end k2 shown')
    expect(s.clock.pending()).toBe(0)
  })

  it('leaves the line being presented alone otherwise', () => {
    // Voice is off: a caption stays a caption.
    const off = setup({ voice: false })
    const a = item({ key: 'k1', mode: 'silent' })
    off.speaker.enqueue(a)
    off.speaker.enqueue({ ...a, mode: 'voice' })
    expect(off.events).toEqual(['start k1 silent null 1', 'chunk k1 0'])
    expect(off.synth.calls).toEqual([])
    // A microphone is open.
    const mic = setup()
    mic.state.mic = true
    const b = item({ key: 'k1' })
    mic.speaker.enqueue(b)
    mic.speaker.enqueue({ ...b })
    expect(mic.events).toEqual(['start k1 silent null 1', 'chunk k1 0'])
    expect(mic.synth.calls).toEqual([])
    // Asked for as a caption again.
    const same = setup()
    const c = item({ key: 'k1', mode: 'silent' })
    same.speaker.enqueue(c)
    same.speaker.enqueue({ ...c })
    expect(same.events).toEqual(['start k1 silent null 1', 'chunk k1 0'])
    // It is being said already.
    const said = setup()
    const d = item({ key: 'k1' })
    said.speaker.enqueue(d)
    said.synth.last.onstart?.()
    said.speaker.enqueue({ ...d })
    expect(said.synth.spoken).toHaveLength(1)
    expect(said.events).toEqual(['start k1 browser Daniel 1', 'chunk k1 0'])
    for (const s of [off, mic, same]) {
      s.clock.advance(SILENT_MIN_MS)
      expect(s.events.at(-1)).toBe('end k1 shown')
      expect(s.clock.pending()).toBe(0)
    }
  })

  it('says a caption that the browser refused to speak, once speech is unlocked', () => {
    const s = setup()
    const line = item({ key: 'k1' })
    s.speaker.enqueue(line)
    s.synth.last.onerror?.({ error: 'not-allowed' })
    expect(s.events).toEqual(['start k1 browser Daniel 1', 'locked', 'start k1 silent null 1', 'chunk k1 0'])
    // Asked for before the gesture: it is still locked, and says so again.
    s.speaker.enqueue({ ...line })
    expect(s.events.slice(4)).toEqual(['locked', 'start k1 silent null 1', 'chunk k1 0'])
    // "Click to hear": the gesture unlocks, and the line on screen is asked for again.
    s.speaker.unlock()
    s.synth.last.onend?.()
    s.speaker.enqueue({ ...line })
    expect(s.events.slice(7)).toEqual(['start k1 browser Daniel 1'])
    expect(s.synth.last.text).toBe('Hello there.')
    s.sayIt()
    expect(s.events.slice(8)).toEqual(['chunk k1 0', 'end k1 spoken'])
    // (The empty utterance of the unlock is watched for four seconds.)
    s.clock.advance(START_WATCHDOG_MS)
    expect(s.clock.pending()).toBe(0)
  })

  it('fails such a line where the browser has no voice for it', () => {
    const s = setup({ voice: false })
    s.synth.voices = []
    const line = item({ key: 'k1', mode: 'silent' })
    s.speaker.enqueue(line)
    s.speaker.setPrefs({ ...PREFS, voice: true })
    s.speaker.enqueue({ ...line, mode: 'voice' })
    // The first use waits for the voices; none come.
    s.clock.advance(VOICES_WAIT_MS)
    expect(s.events).toEqual(['start k1 silent null 1', 'chunk k1 0', 'start k1 silent null 1', 'chunk k1 0'])
    s.clock.advance(SILENT_MIN_MS)
    expect(s.events.at(-1)).toBe('end k1 failed')
    expect(s.clock.pending()).toBe(0)
  })

  it('gives a waiting caption the voice when its turn comes', () => {
    const s = setup({ voice: false })
    s.speaker.enqueue(item({ key: 'a', terminalId: 't1', mode: 'silent' }))
    const waiting = item({ key: 'b', terminalId: 't2', mode: 'silent' })
    s.speaker.enqueue(waiting)
    s.speaker.setPrefs({ ...PREFS, voice: true })
    s.speaker.enqueue({ ...waiting, mode: 'voice' })
    s.clock.advance(SILENT_MIN_MS)
    expect(s.events).toEqual(['start a silent null 1', 'chunk a 0', 'end a shown', 'start b browser Daniel 1'])
  })
})

describe('createSpeaker: voices of several avatars', () => {
  it('spreads avatars over the voices and remembers each one’s', () => {
    const s = setup()
    s.synth.voices = [voice('A', 'en-GB'), voice('B', 'en-GB'), voice('C', 'en-GB')]
    const heard = new Map<string, string>()
    for (const terminalId of ['t1', 't2', 't3', 't1', 't2', 't3']) {
      s.speaker.enqueue(item({ terminalId }))
      const name = (s.synth.last.voice as VoiceLike).name
      if (heard.has(terminalId)) expect(name).toBe(heard.get(terminalId))
      heard.set(terminalId, name)
      s.sayIt()
    }
    expect(new Set(heard.values()).size).toBe(3)
  })

  it('steps the pitch apart when two avatars have to share a voice', () => {
    const s = setup()
    s.synth.voices = [voice('Only', 'en-GB')]
    const pitches: number[] = []
    for (const terminalId of ['t1', 't2', 't3', 't1']) {
      s.speaker.enqueue(item({ terminalId }))
      pitches.push(s.synth.last.pitch)
      s.sayIt()
    }
    expect(pitches[0]).toBeCloseTo(1, 1)
    expect(pitches[1]).toBeCloseTo(0.85, 1)
    expect(pitches[2]).toBeCloseTo(1.15, 1)
    expect(pitches[3]).toBe(pitches[0])
    // An avatar that left gives its place back.
    s.speaker.drop('t2')
    s.speaker.enqueue(item({ terminalId: 't4' }))
    expect(s.synth.last.pitch).toBeCloseTo(1.15, 1)
  })

  it('remembers a bounded number of avatars', () => {
    const s = setup()
    s.synth.voices = [voice('Only', 'en-GB')]
    for (let i = 0; i < 300; i++) {
      s.speaker.enqueue(item({ terminalId: `t${i}` }))
      s.sayIt(50)
    }
    // The first avatars were let go: a new one does not count three hundred others before it.
    s.speaker.enqueue(item({ terminalId: 'late' }))
    expect(s.synth.last.pitch).toBeGreaterThanOrEqual(0.7)
    s.sayIt(50)
    // And the first one is a new avatar again when it comes back: it joins the back of the row.
    s.speaker.enqueue(item({ terminalId: 't0' }))
    expect(s.synth.last.pitch).toBeCloseTo([1, 0.85, 1.15][256 % 3], 1)
  })

  it('takes a voice of the avatar’s new language when it designs one after it spoke with another', () => {
    const s = setup()
    s.synth.voices = [voice('Stub English', 'en-US'), voice('Stub British', 'en-GB')]
    // The greeting, before the design: the default language.
    s.speaker.enqueue(item({ terminalId: 't1', lang: 'en-US' }))
    expect((s.synth.last.voice as VoiceLike).name).toBe('Stub English')
    s.sayIt()
    // Designed with an en-GB voice: the next lines are said with the British one.
    s.speaker.enqueue(item({ terminalId: 't1', lang: 'en-GB', voice: { lang: 'en-GB' } }))
    expect((s.synth.last.voice as VoiceLike).name).toBe('Stub British')
    expect(s.synth.last.lang).toBe('en-GB')
    s.sayIt()
    // Also when the preferences remember the first voice from an earlier page load.
    const later = setup({ voices: { t1: 'Stub English' } })
    later.synth.voices = [voice('Stub English', 'en-US'), voice('Stub British', 'en-GB')]
    later.speaker.enqueue(item({ terminalId: 't1', lang: 'en-GB', voice: { lang: 'en-GB' } }))
    expect((later.synth.last.voice as VoiceLike).name).toBe('Stub British')
  })

  it('uses the voice the preferences remember for a terminal', () => {
    const s = setup({ voices: { t1: 'C', constructor: 'A' } })
    s.synth.voices = [voice('A', 'en-GB'), voice('B', 'en-GB'), voice('C', 'en-GB')]
    s.speaker.enqueue(item({ terminalId: 't1' }))
    expect((s.synth.last.voice as VoiceLike).name).toBe('C')
    s.sayIt()
    // A terminal id that is also the name of something every object has finds nothing by accident.
    s.speaker.enqueue(item({ terminalId: 'toString' }))
    expect(['A', 'B']).toContain((s.synth.last.voice as VoiceLike).name)
  })
})

describe('createSpeaker: listeners and the end', () => {
  it('tells further listeners too, until they leave', () => {
    const s = setup()
    const heard: string[] = []
    const leave = s.speaker.subscribe({ end: (key, outcome) => heard.push(`${key} ${outcome}`) })
    s.speaker.enqueue(item({ key: 'k1' }))
    s.sayIt()
    leave()
    s.speaker.enqueue(item({ key: 'k2' }))
    s.sayIt()
    expect(heard).toEqual(['k1 spoken'])
    expect(s.speaker.subscribe(null as unknown as object)()).toBeUndefined()
  })

  it('goes on when a listener throws', () => {
    const s = setup()
    s.speaker.subscribe({
      start: () => {
        throw new Error('listener')
      },
      end: () => {
        throw new Error('listener')
      },
    })
    s.speaker.enqueue(item({ key: 'k1' }))
    s.speaker.enqueue(item({ key: 'k2', terminalId: 't2' }))
    s.sayIt()
    s.sayIt()
    expect(s.events.filter((e) => e.startsWith('end'))).toEqual(['end k1 spoken', 'end k2 spoken'])
  })

  it('lets a listener call back into it', () => {
    const s = setup()
    let again = true
    s.speaker.subscribe({
      end: () => {
        if (!again) return
        again = false
        s.speaker.enqueue(item({ key: 'next', terminalId: 't9' }))
        expect(s.speaker.current()?.key).toBe('next')
      },
      start: (key) => {
        if (key === 'gone') s.speaker.stop('gone')
      },
    })
    s.speaker.enqueue(item({ key: 'k1' }))
    s.sayIt()
    s.sayIt()
    s.speaker.enqueue(item({ key: 'gone' }))
    expect(s.events.slice(-2)).toEqual(['start gone browser Daniel 1', 'end gone skipped'])
    expect(s.speaker.current()).toBeNull()
    s.clock.advance(10_000)
    expect(s.clock.pending()).toBe(0)
  })

  it('stops speaking at once when disposed, and says nothing more to anyone', () => {
    const s = setup()
    s.speaker.enqueue(item({ key: 'k1' }))
    s.speaker.enqueue(item({ key: 'k2', terminalId: 't2' }))
    const u = s.synth.last
    u.onstart?.()
    s.speaker.dispose()
    expect(s.synth.count('cancel')).toBe(1)
    expect(s.events).toEqual(['start k1 browser Daniel 1', 'chunk k1 0'])
    u.onerror?.({ error: 'interrupted' })
    u.onend?.()
    s.clock.advance(60_000)
    s.speaker.enqueue(item({ key: 'k3' }))
    s.speaker.stop()
    s.speaker.unlock()
    s.speaker.setPrefs(PREFS)
    s.speaker.drop('t1')
    s.speaker.dispose()
    expect(s.events).toHaveLength(2)
    expect(s.synth.spoken).toHaveLength(1)
    expect(s.speaker.current()).toBeNull()
    expect(s.speaker.mouth('t1')).toBe(-1)
    expect(s.clock.pending()).toBe(0)
  })

  it('ignores what is no line', () => {
    const s = setup()
    for (const bad of [null, undefined, 7, 'line', {}, { key: 7, terminalId: 't1' }, { key: 'k', terminalId: null }]) s.speaker.enqueue(bad as unknown as SpeakItem)
    expect(s.events).toEqual([])
    expect(s.speaker.current()).toBeNull()
  })
})
