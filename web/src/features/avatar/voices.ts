// This browser's voices, and one sentence to try them with.
//
// A browser's voices are of two kinds: the ones on the device, and online ones, which send
// what is said to the browser maker's speech service. The user allows the second kind
// explicitly (rule 7), so every voice here is marked as one or the other.
//
// Contract module for the platform feature: `useBrowserVoices()` and `testVoice()` (with
// the types `BrowserVoice` and `TestVoiceResult`). Its Settings section lists the voices and
// has the Test voice button. The other exports are the avatar feature's own.

import { useSyncExternalStore } from 'react'
import { dictationLive } from '@/features/agents/lib/dictation'
import { VOICES_WAIT_MS, browserSpeech, pickVoice, usableVoices } from './lib/speech'
import type { SynthLike, UtteranceLike, VoiceLike } from './lib/speech'
import { useAvatarPrefs } from './store'

export interface BrowserVoice {
  name: string
  /** A BCP 47 tag, as the browser gives it. */
  lang: string
  /** On this device. false: an online voice, which sends what it says to the browser maker's speech service. */
  onDevice: boolean
  /** The browser's own default voice. */
  default: boolean
}

/** A browser lists a few hundred voices at most; a list is never longer than this here. */
const MAX_VOICES = 500
const NO_VOICES: readonly BrowserVoice[] = Object.freeze([])

/** The voices a synthesizer has now, each once, in the browser's order. Empty where there is none, or while they still load. */
export function listVoices(synth: Pick<SynthLike, 'getVoices'> | null | undefined): BrowserVoice[] {
  let all: unknown
  try {
    all = synth?.getVoices()
  } catch {
    all = null
  }
  if (!Array.isArray(all)) return []
  const out: BrowserVoice[] = []
  const seen = new Set<string>()
  for (const v of all as (VoiceLike | null)[]) {
    if (typeof v !== 'object' || v === null || typeof v.name !== 'string' || typeof v.lang !== 'string' || !v.name) continue
    const key = `${v.lang}\n${v.name}`
    if (seen.has(key)) continue
    seen.add(key)
    out.push({ name: v.name, lang: v.lang, onDevice: v.localService === true, default: v.default === true })
    if (out.length >= MAX_VOICES) break
  }
  return out
}

/** A language tag's language, lower case ("en" of "en-GB" and of Android's "en_US"). */
const base = (lang: unknown): string => (typeof lang === 'string' ? lang.replace(/_/g, '-').toLowerCase().split('-')[0] : '')

/**
 * What this browser can say a language with: voices on the device, only online ones, or
 * none. The first voice choice has a form for each (Hear it speaks at once; the sentence
 * about online voices with Allow online voices; "This browser has no voices").
 */
export function voicesFor(voices: readonly Pick<BrowserVoice, 'lang' | 'onDevice'>[], lang: string): 'device' | 'online' | 'none' {
  const want = base(lang) || 'en'
  let online = false
  for (const v of Array.isArray(voices) ? voices : []) {
    if (base(v?.lang) !== want) continue
    if (v.onDevice === true) return 'device'
    online = true
  }
  return online ? 'online' : 'none'
}

/** The speech of a window, as far as listing goes. */
type SpeechWindow = Parameters<typeof browserSpeech>[0]

interface Timers {
  setTimeout(cb: () => void, ms: number): number
  clearTimeout(id: number): void
}

/**
 * A window's voices as an external store: `read` gives the same list object until the
 * browser's list changes, `subscribe` tells when it may have (voices load after the page).
 * `listed` says whether the browser has listed them yet: it gave voices, it said its list
 * changed, or, with `timers`, it kept quiet for as long as the speaker waits for them
 * (`VOICES_WAIT_MS`, from the first subscription). Before that an empty list says nothing.
 */
export function voiceSource(win: SpeechWindow, timers?: Timers) {
  const { synth } = browserSpeech(win)
  let held: readonly BrowserVoice[] = NO_VOICES
  /** A browser without speech has no voices for good. */
  let known = synth === null
  const told = new Set<() => void>()
  let wait: number | null = null
  const stopWaiting = () => {
    if (wait !== null) timers?.clearTimeout(wait)
    wait = null
  }
  const read = (): readonly BrowserVoice[] => {
    const now = listVoices(synth)
    const same = now.length === held.length && now.every((v, i) => v.name === held[i].name && v.lang === held[i].lang && v.onDevice === held[i].onDevice && v.default === held[i].default)
    if (!same) held = now.length === 0 ? NO_VOICES : now
    if (held.length > 0) known = true
    return held
  }
  const listed = (): boolean => {
    if (!known) read()
    if (known) stopWaiting()
    return known
  }
  const subscribe = (changed: () => void): (() => void) => {
    if (!synth) return () => {}
    const heard = () => {
      known = true
      stopWaiting()
      changed()
    }
    try {
      synth.addEventListener('voiceschanged', heard)
    } catch {
      return () => {}
    }
    told.add(changed)
    if (timers && wait === null && !listed())
      wait = timers.setTimeout(() => {
        wait = null
        known = true
        for (const tell of told) tell()
      }, VOICES_WAIT_MS)
    return () => {
      told.delete(changed)
      if (told.size === 0) stopWaiting()
      try {
        synth.removeEventListener('voiceschanged', heard)
      } catch {
        // Not listening then.
      }
    }
  }
  return { read, listed, subscribe }
}

let source: ReturnType<typeof voiceSource> | null = null
const ofThisWindow = () => (source ??= typeof window === 'undefined' ? voiceSource(null) : voiceSource(window, { setTimeout: (cb, ms) => window.setTimeout(cb, ms), clearTimeout: (id) => window.clearTimeout(id) }))
const none = () => NO_VOICES
const listedOnServer = () => true

/** This browser's voices, each marked as on this device or online. The list follows the browser's (`voiceschanged`). */
export function useBrowserVoices(): readonly BrowserVoice[] {
  const s = ofThisWindow()
  return useSyncExternalStore(s.subscribe, s.read, none)
}

/** Whether this browser has listed its voices yet (`voiceSource`): until then an empty `useBrowserVoices()` does not mean it has none. */
export function useVoicesListed(): boolean {
  const s = ofThisWindow()
  return useSyncExternalStore(s.subscribe, s.listed, listedOnServer)
}

// ---------------------------------------------------------------- the test sentence

/** The one sentence Test voice says. Fixed and harmless: with an online voice it is sent to the browser maker's speech service. */
export const TEST_SENTENCE = 'This is how an avatar sounds in this browser.'

/**
 * `spoken`: handed to the browser to say. `no-speech`: this browser cannot speak.
 * `no-click`: the call did not come from a click or a key press, and the sentence is never
 * said by itself. `no-voices`: the browser has no voice it could use. `online-only`: it has
 * only online voices (or the voice asked for is one) and they are not allowed. `listening`:
 * a dictation is live, and nothing is said into an open microphone. `busy`: the last test
 * is still being said.
 */
export type TestVoiceResult = 'spoken' | 'no-speech' | 'no-click' | 'no-voices' | 'online-only' | 'listening' | 'busy'

export interface VoiceTestHost {
  synth: SynthLike | null
  utterance(text: string): UtteranceLike
  /** Whether the user is acting right now (`navigator.userActivation.isActive`). A browser that cannot tell answers true. */
  clicked(): boolean
  micLive(): boolean
  setTimeout(cb: () => void, ms: number): number
  clearTimeout(id: number): void
}

/** A test that neither ended nor failed after this long is given up, so the button works again. */
export const TEST_GIVE_UP_MS = 20_000

/**
 * The test sentence for one window. It never cancels anything: a line that is being said
 * goes on, and the sentence follows it. The utterance is held until it ends, because
 * browsers stop reporting on one that was collected.
 */
export function createVoiceTest(host: VoiceTestHost) {
  let saying: UtteranceLike | null = null
  return (prefs: { volume: number; speed: number; online: boolean }, voiceName?: string): TestVoiceResult => {
    const synth = host.synth
    if (!synth) return 'no-speech'
    let clicked = false
    let live = false
    try {
      clicked = host.clicked() === true
      live = host.micLive() === true
    } catch {
      live = false
    }
    if (!clicked) return 'no-click'
    if (live) return 'listening'
    if (saying) return 'busy'
    let all: VoiceLike[] = []
    try {
      const list = synth.getVoices()
      all = Array.isArray(list) ? list.filter((v) => typeof v === 'object' && v !== null && typeof v.name === 'string' && typeof v.lang === 'string') : []
    } catch {
      all = []
    }
    if (all.length === 0) return 'no-voices'
    const usable = usableVoices(all, { online: prefs?.online === true })
    let voice: VoiceLike | null
    if (typeof voiceName === 'string' && voiceName) {
      voice = usable.find((v) => v.name === voiceName) ?? null
      if (!voice) return all.some((v) => v.name === voiceName) ? 'online-only' : 'no-voices'
    } else {
      // An English voice for an English sentence; a browser without one still shows that it speaks.
      voice = pickVoice(usable, {}, 'test', { lang: 'en-US', taken: new Map() }) ?? usable.find((v) => v.default === true) ?? usable[0] ?? null
      if (!voice) return 'online-only'
    }
    const within = (n: unknown, min: number, max: number): number => (typeof n === 'number' && Number.isFinite(n) ? Math.min(Math.max(n, min), max) : 1)
    let timer: number | null = null
    try {
      const u = host.utterance(TEST_SENTENCE)
      const over = () => {
        if (saying !== u) return
        saying = null
        if (timer !== null) host.clearTimeout(timer)
        timer = null
      }
      u.voice = voice
      u.lang = voice.lang.replace(/_/g, '-') || 'en-US'
      u.pitch = 1
      u.rate = within(prefs?.speed, 0.7, 1.5)
      u.volume = within(prefs?.volume, 0, 1)
      u.onend = over
      u.onerror = over
      saying = u
      timer = host.setTimeout(over, TEST_GIVE_UP_MS)
      synth.resume()
      synth.speak(u)
    } catch {
      // The browser refused outright: nothing is being said, and the button works again at once.
      saying = null
      if (timer !== null) host.clearTimeout(timer)
      return 'no-speech'
    }
    return 'spoken'
  }
}

let test: ReturnType<typeof createVoiceTest> | null = null

/**
 * Says the test sentence with this browser's preferences (volume, speed, whether online
 * voices are allowed): with `voiceName` that voice, else the one an avatar would get.
 * It speaks only from a click or a key press: called at any other time it answers
 * `no-click` and says nothing.
 */
export function testVoice(voiceName?: string): TestVoiceResult {
  if (typeof window === 'undefined') return 'no-speech'
  const w = window
  test ??= createVoiceTest({
    ...browserSpeech(w),
    clicked: () => {
      const acting: unknown = w.navigator?.userActivation?.isActive
      return acting !== false
    },
    micLive: dictationLive,
    setTimeout: (cb, ms) => w.setTimeout(cb, ms),
    clearTimeout: (id) => w.clearTimeout(id),
  })
  const { volume, speed, online } = useAvatarPrefs.getState()
  return test({ volume, speed, online }, voiceName)
}
