// This browser's voices and the test sentence, with a synthesizer that only records:
// nothing here is ever sent to a browser's speech service.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { fakeClock } from './lib/fixtures'
import { VOICES_WAIT_MS } from './lib/speech'
import type { SynthLike, UtteranceLike, VoiceLike } from './lib/speech'
import { TEST_GIVE_UP_MS, TEST_SENTENCE, createVoiceTest, listVoices, testVoice, useBrowserVoices, useVoicesListed, voiceSource, voicesFor } from './voices'
import type { BrowserVoice, VoiceTestHost } from './voices'

const voice = (name: string, lang: string, localService = true, isDefault = false): VoiceLike => ({ name, lang, localService, default: isDefault })

class Utterance implements UtteranceLike {
  voice: unknown = null
  lang = ''
  pitch = 0
  rate = 0
  volume = 0
  onstart: (() => void) | null = null
  onend: (() => void) | null = null
  onerror: ((e: { error?: string }) => void) | null = null
  onboundary: ((e: { charIndex: number }) => void) | null = null
  text: string
  constructor(text: string) {
    this.text = text
  }
}

class Synth implements SynthLike {
  voices: VoiceLike[]
  spoken: Utterance[] = []
  calls: string[] = []
  listeners = new Set<() => void>()
  constructor(voices: VoiceLike[]) {
    this.voices = voices
  }
  speak(u: UtteranceLike): void {
    this.calls.push('speak')
    this.spoken.push(u as Utterance)
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
}

const LOCAL = [voice('Daniel', 'en-GB', true, true), voice('Anna', 'de-DE')]
const ONLINE = [voice('Google UK English Female', 'en-GB', false), voice('Google Deutsch', 'de-DE', false)]

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('listVoices', () => {
  it('marks each voice as on this device or online, in the browser’s order', () => {
    expect(listVoices(new Synth([...LOCAL, ...ONLINE]))).toEqual<BrowserVoice[]>([
      { name: 'Daniel', lang: 'en-GB', onDevice: true, default: true },
      { name: 'Anna', lang: 'de-DE', onDevice: true, default: false },
      { name: 'Google UK English Female', lang: 'en-GB', onDevice: false, default: false },
      { name: 'Google Deutsch', lang: 'de-DE', onDevice: false, default: false },
    ])
  })

  it('lists a voice once, leaves out what is no voice, and never more than a few hundred', () => {
    const odd = [voice('Daniel', 'en-GB'), voice('Daniel', 'en-GB'), voice('Daniel', 'en-AU'), null, 7, { name: 7, lang: 'en' }, { name: 'x' }, voice('', 'en'), { name: 'Maybe', lang: 'fr-FR', localService: 'yes' }]
    expect(listVoices(new Synth(odd as VoiceLike[]))).toEqual([
      { name: 'Daniel', lang: 'en-GB', onDevice: true, default: false },
      { name: 'Daniel', lang: 'en-AU', onDevice: true, default: false },
      // Only a voice the browser says is local counts as on the device.
      { name: 'Maybe', lang: 'fr-FR', onDevice: false, default: false },
    ])
    expect(listVoices(new Synth(Array.from({ length: 2000 }, (_, i) => voice(`V${i}`, 'en-US'))))).toHaveLength(500)
  })

  it('is empty where the browser has no speech, while voices load, and when asking fails', () => {
    expect(listVoices(null)).toEqual([])
    expect(listVoices(undefined)).toEqual([])
    expect(listVoices(new Synth([]))).toEqual([])
    expect(listVoices({ getVoices: () => 'none' as unknown as VoiceLike[] })).toEqual([])
    expect(
      listVoices({
        getVoices: () => {
          throw new Error('not now')
        },
      }),
    ).toEqual([])
  })
})

describe('voicesFor', () => {
  const list = (voices: VoiceLike[]) => listVoices(new Synth(voices))

  it('says what the browser can say a language with', () => {
    expect(voicesFor(list([...LOCAL, ...ONLINE]), 'en-GB')).toBe('device')
    // Any voice of the language will do, as for speaking.
    expect(voicesFor(list(LOCAL), 'en-US')).toBe('device')
    expect(voicesFor(list(LOCAL), 'de')).toBe('device')
    expect(voicesFor(list(ONLINE), 'en-US')).toBe('online')
    expect(voicesFor(list([...ONLINE, voice('Anna', 'de-DE')]), 'en-US')).toBe('online')
    expect(voicesFor(list(LOCAL), 'fr-FR')).toBe('none')
    expect(voicesFor([], 'en-US')).toBe('none')
  })

  it('reads language tags as browsers write them, and takes English when it is given none', () => {
    expect(voicesFor(list([voice('Android', 'en_US')]), 'EN-gb')).toBe('device')
    expect(voicesFor(list(LOCAL), '')).toBe('device')
    expect(voicesFor(list([voice('Anna', 'de-DE')]), '')).toBe('none')
    expect(voicesFor(null as unknown as BrowserVoice[], 'en')).toBe('none')
  })
})

describe('voiceSource', () => {
  const win = (synth: Synth | null) => ({ speechSynthesis: synth ?? undefined, SpeechSynthesisUtterance: Utterance })

  it('gives the same list until the browser’s list changes', () => {
    const synth = new Synth([])
    const source = voiceSource(win(synth))
    const empty = source.read()
    expect(empty).toEqual([])
    expect(source.read()).toBe(empty)
    // Voices load after the page: the browser says so, and the list is another one.
    let told = 0
    const off = source.subscribe(() => told++)
    synth.voices = LOCAL
    for (const l of synth.listeners) l()
    expect(told).toBe(1)
    const loaded = source.read()
    expect(loaded.map((v) => v.name)).toEqual(['Daniel', 'Anna'])
    expect(source.read()).toBe(loaded)
    // The browser makes new voice objects on every call: that alone is no change.
    synth.voices = LOCAL.map((v) => ({ ...v }))
    expect(source.read()).toBe(loaded)
    synth.voices = [...LOCAL, ...ONLINE]
    expect(source.read()).not.toBe(loaded)
    expect(source.read()).toHaveLength(4)
    off()
    expect(synth.listeners.size).toBe(0)
  })

  it('is an empty list for good in a browser without speech', () => {
    for (const w of [null, undefined, {}, win(null), { speechSynthesis: new Synth(LOCAL) }]) {
      const source = voiceSource(w)
      expect(source.read()).toEqual([])
      expect(source.read()).toBe(source.read())
      source.subscribe(() => {
        throw new Error('never told')
      })()
    }
  })

  it('renders as no voices on the server, and in a test', () => {
    const List = () => createElement('span', null, String(useBrowserVoices().length))
    expect(renderToStaticMarkup(createElement(List))).toBe('<span>0</span>')
  })

  it('knows when the browser has listed its voices: an empty list says nothing before that', () => {
    // Voices load after the page: the browser says so, with a list or, where it has none, with an empty one.
    const late = new Synth([])
    const source = voiceSource(win(late))
    expect(source.read()).toEqual([])
    expect(source.listed()).toBe(false)
    const off = source.subscribe(() => {})
    for (const l of late.listeners) l()
    expect(source.listed()).toBe(true)
    off()
    // A list that has voices is listed.
    const ready = voiceSource(win(new Synth(LOCAL)))
    expect(ready.listed()).toBe(true)
    // So is a list that filled without a word from the browser.
    const quiet = new Synth([])
    const filled = voiceSource(win(quiet))
    expect(filled.listed()).toBe(false)
    quiet.voices = LOCAL
    expect(filled.listed()).toBe(true)
    // A browser without speech has none for good.
    expect(voiceSource(null).listed()).toBe(true)
    expect(voiceSource(win(null)).listed()).toBe(true)
  })

  it('takes an empty list as it is once the browser kept quiet for as long as the speaker waits', () => {
    const clock = fakeClock(0)
    const mute = new Synth([])
    const source = voiceSource(win(mute), clock)
    let told = 0
    const off = source.subscribe(() => told++)
    clock.advance(VOICES_WAIT_MS - 1)
    expect([source.listed(), told]).toEqual([false, 0])
    clock.advance(1)
    expect([source.listed(), told]).toEqual([true, 1])
    off()
    expect(clock.pending()).toBe(0)
    // Nobody listens any more: the wait is not kept running.
    const gone = voiceSource(win(new Synth([])), clock)
    gone.subscribe(() => {})()
    expect(clock.pending()).toBe(0)
    // And one that is listed is not waited on.
    voiceSource(win(new Synth(LOCAL)), clock).subscribe(() => {})
    expect(clock.pending()).toBe(0)
  })

  it('renders as listed on the server, and in a test', () => {
    const Known = () => createElement('span', null, String(useVoicesListed()))
    expect(renderToStaticMarkup(createElement(Known))).toBe('<span>true</span>')
  })
})

describe('the test sentence', () => {
  function setup(voices: VoiceLike[] | null, over: Partial<VoiceTestHost> = {}) {
    const clock = fakeClock(0)
    const synth = voices === null ? null : new Synth(voices)
    const state = { mic: false, clicked: true }
    const host: VoiceTestHost = { synth, utterance: (text) => new Utterance(text), clicked: () => state.clicked, micLive: () => state.mic, setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout, ...over }
    return { clock, synth: synth as Synth, state, test: createVoiceTest(host) }
  }
  const PREFS = { volume: 0.6, speed: 1.2, online: false }

  it('says one fixed sentence with the browser’s preferences, and never cancels anything', () => {
    const s = setup([...ONLINE, ...LOCAL])
    expect(s.test(PREFS)).toBe('spoken')
    expect(s.synth.spoken).toHaveLength(1)
    const u = s.synth.spoken[0]
    expect(u.text).toBe(TEST_SENTENCE)
    expect(TEST_SENTENCE).toBe('This is how an avatar sounds in this browser.')
    // The voice object itself, as the browser gave it; an English one for an English sentence.
    expect(u.voice).toBe(s.synth.voices[2])
    expect(u).toMatchObject({ lang: 'en-GB', pitch: 1, rate: 1.2, volume: 0.6 })
    expect(s.synth.calls).toEqual(['resume', 'speak'])
    u.onend?.()
    expect(s.clock.pending()).toBe(0)
  })

  it('keeps volume and speed in their ranges', () => {
    const s = setup(LOCAL)
    s.test({ volume: 9, speed: 0.1, online: false })
    expect(s.synth.spoken[0]).toMatchObject({ rate: 0.7, volume: 1 })
    s.synth.spoken[0].onend?.()
    s.test({ volume: Number.NaN, speed: 'fast' as unknown as number, online: false })
    expect(s.synth.spoken[1]).toMatchObject({ rate: 1, volume: 1 })
  })

  it('uses an online voice only when the user allowed them', () => {
    const s = setup(ONLINE)
    expect(s.test(PREFS)).toBe('online-only')
    expect(s.synth.calls).toEqual([])
    expect(s.test({ ...PREFS, online: true })).toBe('spoken')
    expect(s.synth.spoken[0].voice).toBe(s.synth.voices[0])
    // With voices of both kinds and online ones not allowed, it is the one on the device.
    const both = setup([...ONLINE, ...LOCAL])
    both.test(PREFS)
    expect((both.synth.spoken[0].voice as VoiceLike).localService).toBe(true)
  })

  it('says it with the voice that is asked for, if that one may be used', () => {
    const s = setup([...LOCAL, ...ONLINE])
    expect(s.test(PREFS, 'Anna')).toBe('spoken')
    expect(s.synth.spoken[0]).toMatchObject({ voice: s.synth.voices[1], lang: 'de-DE' })
    s.synth.spoken[0].onend?.()
    expect(s.test(PREFS, 'Google Deutsch')).toBe('online-only')
    expect(s.test(PREFS, 'Nobody')).toBe('no-voices')
    expect(s.synth.spoken).toHaveLength(1)
    expect(s.test({ ...PREFS, online: true }, 'Google Deutsch')).toBe('spoken')
  })

  it('falls back to another language’s voice where the browser has no English one', () => {
    const s = setup([voice('Anna', 'de-DE'), voice('Amélie', 'fr_FR', true, true)])
    expect(s.test(PREFS)).toBe('spoken')
    // The browser's default voice, with its language written the way utterances take it.
    expect(s.synth.spoken[0]).toMatchObject({ voice: s.synth.voices[1], lang: 'fr-FR' })
  })

  it('says nothing unless the user is acting: the sentence never speaks by itself', () => {
    const s = setup([...LOCAL, ...ONLINE])
    s.state.clicked = false
    expect(s.test({ ...PREFS, online: true })).toBe('no-click')
    expect(s.test(PREFS, 'Daniel')).toBe('no-click')
    expect(s.synth.calls).toEqual([])
    expect(s.clock.pending()).toBe(0)
    s.state.clicked = true
    expect(s.test(PREFS)).toBe('spoken')
    // A host that cannot say whether the user acts is no licence to speak.
    const odd = setup(LOCAL, {
      clicked: () => {
        throw new Error('no user activation here')
      },
    })
    expect(odd.test(PREFS)).toBe('no-click')
    expect(odd.synth.calls).toEqual([])
  })

  it('says why it stays silent: no speech, no voices, a live microphone, a test still being said', () => {
    expect(setup(null).test(PREFS)).toBe('no-speech')
    expect(setup([]).test(PREFS)).toBe('no-voices')
    const s = setup(LOCAL)
    s.state.mic = true
    expect(s.test(PREFS)).toBe('listening')
    expect(s.synth.calls).toEqual([])
    s.state.mic = false
    expect(s.test(PREFS)).toBe('spoken')
    // Clicked again while it speaks: once is enough.
    expect(s.test(PREFS)).toBe('busy')
    expect(s.synth.spoken).toHaveLength(1)
    s.synth.spoken[0].onerror?.({ error: 'interrupted' })
    expect(s.test(PREFS)).toBe('spoken')
    expect(s.synth.calls.includes('cancel')).toBe(false)
  })

  it('works again when the browser never reports the end', () => {
    const s = setup(LOCAL)
    s.test(PREFS)
    s.clock.advance(TEST_GIVE_UP_MS - 1)
    expect(s.test(PREFS)).toBe('busy')
    s.clock.advance(1)
    expect(s.test(PREFS)).toBe('spoken')
    // The first one's late end does not free the second.
    s.synth.spoken[0].onend?.()
    expect(s.test(PREFS)).toBe('busy')
    s.synth.spoken[1].onend?.()
    expect(s.clock.pending()).toBe(0)
  })

  it('copes with a browser that refuses', () => {
    const throwing = setup(LOCAL)
    throwing.synth.speak = () => {
      throw new Error('not allowed')
    }
    expect(throwing.test(PREFS)).toBe('no-speech')
    expect(throwing.clock.pending()).toBe(0)
    // Nothing is held: the next click tries again.
    expect(throwing.test(PREFS)).toBe('no-speech')
    const noCtor = setup(LOCAL, {
      utterance: () => {
        throw new Error('This browser has no speech synthesis.')
      },
    })
    expect(noCtor.test(PREFS)).toBe('no-speech')
    const micThrows = setup(LOCAL, {
      micLive: () => {
        throw new Error('no dictation here')
      },
    })
    expect(micThrows.test(PREFS)).toBe('spoken')
    const voicesThrow = setup(LOCAL)
    voicesThrow.synth.getVoices = () => {
      throw new Error('not now')
    }
    expect(voicesThrow.test(PREFS)).toBe('no-voices')
  })

  it('is silent where there is no window at all', () => {
    expect(testVoice()).toBe('no-speech')
    expect(testVoice('Daniel')).toBe('no-speech')
  })

  it('reads the preferences of this browser when it is clicked', async () => {
    const synth = new Synth(ONLINE)
    const data = new Map<string, string>([['wb.avatar.v1', JSON.stringify({ state: { volume: 0.25, speed: 1.4, online: true }, version: 1 })]])
    vi.stubGlobal('window', {
      speechSynthesis: synth,
      SpeechSynthesisUtterance: Utterance,
      setTimeout: () => 1,
      clearTimeout: () => {},
      localStorage: { getItem: (k: string) => data.get(k) ?? null, setItem: (k: string, v: string) => void data.set(k, v), removeItem: () => {} },
    })
    vi.resetModules()
    const fresh = await import('./voices')
    // A browser that cannot tell whether the user acts (no `navigator.userActivation`) is taken at its word.
    expect(fresh.testVoice()).toBe('spoken')
    // Online voices are allowed in this browser, which has no others: the stored choice was read.
    expect(synth.spoken[0]).toMatchObject({ text: TEST_SENTENCE, voice: ONLINE[0], rate: 1.4, volume: 0.25 })
    expect(fresh.testVoice()).toBe('busy')
    expect(synth.calls).toEqual(['resume', 'speak'])
  })

  it('asks the browser whether the user is acting', async () => {
    const synth = new Synth(LOCAL)
    const activation = { isActive: false }
    vi.stubGlobal('window', { speechSynthesis: synth, SpeechSynthesisUtterance: Utterance, navigator: { userActivation: activation }, setTimeout: () => 1, clearTimeout: () => {} })
    vi.resetModules()
    const fresh = await import('./voices')
    // Called from a timer, an effect, a message: not a word.
    expect(fresh.testVoice()).toBe('no-click')
    expect(synth.calls).toEqual([])
    activation.isActive = true
    expect(fresh.testVoice()).toBe('spoken')
    expect(synth.spoken.map((u) => u.text)).toEqual([TEST_SENTENCE])
  })
})
