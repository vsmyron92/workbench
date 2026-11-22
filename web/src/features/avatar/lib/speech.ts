// Presenting a line: aloud through the browser's speech synthesis, or silently as a caption.
//
// One speaker per window. It presents one line at a time across all avatars, with one of
// two engines: `browser` (one utterance per chunk of the line) or `silent` (the caption
// alone, for as long as the line would take to say). Both drive the same lip sync timeline.
//
// What goes wrong in real browsers is handled here and nowhere else: an utterance that
// never starts or never ends (a watchdog per chunk), events of an utterance that was
// already given up (a generation number), a stop that arrives as an `error` event, speech
// that the browser refuses before the first user gesture, voices that load late.
//
// Everything a browser provides is passed in (`SpeakerHost`): the synthesizer, the
// utterance constructor, the clock and timers. Nothing here reads a global.

import { stableHash } from './types'
import type { Voice } from './types'

// ---------------------------------------------------------------- text

/** The rate browsers speak at, in characters a second at rate 1 (measured: about 15). */
export const CHARS_PER_SECOND = 15

/**
 * The longest chunk one utterance holds: ten seconds of speech, at most 200 characters,
 * never under 60. Online voices stop after about fifteen seconds of one utterance.
 */
export function chunkLimit(charsPerSecond: number): number {
  const cps = typeof charsPerSecond === 'number' && Number.isFinite(charsPerSecond) && charsPerSecond > 0 ? charsPerSecond : CHARS_PER_SECOND
  return Math.max(60, Math.min(200, Math.floor(10 * cps)))
}

const SENTENCE_END = new Set(['.', '!', '?', '…', '。', '！', '？'])
const CLAUSE_END = new Set([',', ';', ':', '，', '；', '：', '、'])

/**
 * A line cut into chunks of at most `maxChars`: at sentence ends where it can, then at
 * commas, then at spaces, and only then inside a word. White space is collapsed.
 */
export function chunkText(text: string, maxChars: number): string[] {
  const max = typeof maxChars === 'number' && Number.isFinite(maxChars) ? Math.max(1, Math.floor(maxChars)) : 200
  let rest = typeof text === 'string' ? text.replace(/\s+/g, ' ').trim() : ''
  const out: string[] = []
  while (rest.length > max) {
    let sentence = -1
    let clause = -1
    let space = -1
    for (let i = 0; i < max; i++) {
      const ch = rest[i]
      // A break after this character counts only when the piece ends there: before a space.
      const ends = rest[i + 1] === ' '
      if (SENTENCE_END.has(ch) && ends) sentence = i + 1
      else if (CLAUSE_END.has(ch) && ends) clause = i + 1
      // A space right after the last character that fits is a break too.
      if (rest[i + 1] === ' ') space = i + 1
    }
    let cut = sentence > 0 ? sentence : clause > 0 ? clause : space > 0 ? space : max
    // Never between the two halves of one character.
    const code = rest.charCodeAt(cut - 1)
    if (cut === max && code >= 0xd800 && code <= 0xdbff && cut > 1) cut--
    out.push(rest.slice(0, cut).trim())
    rest = rest.slice(cut).trim()
  }
  if (rest) out.push(rest)
  return out.filter((chunk) => chunk.length > 0)
}

function comparable(text: string): string {
  return text
    .normalize('NFKC')
    .toLowerCase()
    .replace(/[\p{P}\p{S}]+/gu, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}

/**
 * The choice a typed or dictated answer names: the index of the one choice it equals when
 * case and punctuation are ignored, else null. It never guesses: two choices that read the
 * same give null.
 */
export function matchChoice(text: string, choices: readonly string[]): number | null {
  if (typeof text !== 'string' || !Array.isArray(choices)) return null
  const said = comparable(text)
  if (!said) return null
  let found: number | null = null
  for (let i = 0; i < choices.length; i++) {
    const choice: unknown = choices[i]
    if (typeof choice !== 'string' || comparable(choice) !== said) continue
    if (found !== null) return null
    found = i
  }
  return found
}

// ---------------------------------------------------------------- voices

/** The parts of a `SpeechSynthesisVoice` that matter here. */
export interface VoiceLike {
  name: string
  lang: string
  /** On this device; false: the browser maker's speech service. */
  localService: boolean
  default: boolean
}

const isVoice = (v: unknown): v is VoiceLike => typeof v === 'object' && v !== null && typeof (v as VoiceLike).name === 'string' && typeof (v as VoiceLike).lang === 'string'

/** The voices this browser may use: the ones on the device, and online ones only when the user allowed them. */
export function usableVoices<V extends VoiceLike>(voices: readonly V[], opts: { online: boolean }): V[] {
  if (!Array.isArray(voices)) return []
  return voices.filter((v) => isVoice(v) && (v.localService === true || opts?.online === true))
}

/** A language tag as browsers compare it: lower case, with "-" (Android writes "en_US"). */
const tag = (lang: unknown): string => (typeof lang === 'string' ? lang.replace(/_/g, '-').toLowerCase() : '')
const base = (lang: string): string => lang.split('-')[0]

/**
 * The voice an avatar speaks with. `voices` are the usable ones. In order: the first
 * `prefer` fragment that matches a voice of the base language; the voice remembered for
 * this terminal, while it is still there; else, from the voices of the base language (the
 * ones of the exact language first), the one the fewest other avatars use, ties decided by
 * a stable hash of the seed. null: no usable voice of that language.
 */
export function pickVoice<V extends VoiceLike>(voices: readonly V[], voice: Voice, seed: string, opts: { lang?: string; taken: ReadonlyMap<string, number>; remembered?: string }): V | null {
  if (!Array.isArray(voices)) return null
  const want = tag(opts?.lang ?? voice?.lang) || 'en-us'
  const pool = voices.filter((v) => isVoice(v) && base(tag(v.lang)) === base(want))
  if (pool.length === 0) return null
  const prefer: unknown = voice?.prefer
  if (Array.isArray(prefer)) {
    for (const fragment of prefer.slice(0, 5)) {
      if (typeof fragment !== 'string' || !fragment.trim()) continue
      const needle = fragment.trim().toLowerCase()
      const hit = pool.find((v) => v.name.toLowerCase().includes(needle))
      if (hit) return hit
    }
  }
  if (typeof opts?.remembered === 'string') {
    const kept = pool.find((v) => v.name === opts.remembered)
    if (kept) return kept
  }
  const exact = pool.filter((v) => tag(v.lang) === want)
  const from = exact.length > 0 ? exact : pool
  const used = (v: V): number => {
    const n = opts?.taken instanceof Map ? opts.taken.get(v.name) : undefined
    return typeof n === 'number' && n > 0 ? n : 0
  }
  const least = Math.min(...from.map(used))
  const ties = from.filter((v) => used(v) === least).sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
  return ties[stableHash(String(seed)) % ties.length]
}

const PITCH_STEPS: readonly number[] = [1, 0.85, 1.15]

/**
 * The pitch an avatar speaks at: the agent's own (0.7 to 1.4), else one derived for the
 * session. `repeats` is how many other avatars of this browser already use the same voice:
 * each of them steps apart (1, then 0.85, then 1.15), so two sessions sharing a voice do
 * not sound alike; a small detune from the seed keeps even those apart from a third.
 */
export function pitchFor(voice: Voice, seed: string, repeats: number): number {
  const own: unknown = voice?.pitch
  if (typeof own === 'number' && Number.isFinite(own)) return Math.min(Math.max(own, 0.7), 1.4)
  const n = typeof repeats === 'number' && Number.isFinite(repeats) && repeats > 0 ? Math.floor(repeats) : 0
  const detune = ((stableHash(String(seed)) % 7) - 3) * 0.01
  return Math.round((PITCH_STEPS[n % PITCH_STEPS.length] + detune) * 100) / 100
}

// ---------------------------------------------------------------- lip sync

export interface MouthTimeline {
  durationMs: number
  /** How open the mouth is `ms` after the chunk started: 0 to 1. */
  at(ms: number): number
  /** The voice says it reached the character at `charIndex` now, `ms` after the start: shift the rest to fit. */
  realign(charIndex: number, ms: number): void
}

const OPEN: ReadonlyMap<string, number> = new Map([
  ...[...'aáàâäãåāαая'].map((c): [string, number] => [c, 1]),
  ...[...'oóòôöõōοωоё'].map((c): [string, number] => [c, 0.85]),
  ...[...'eéèêëēεηеэє'].map((c): [string, number] => [c, 0.7]),
  ...[...'uúùûüūυую'].map((c): [string, number] => [c, 0.55]),
  ...[...'iíìîïīιиіїыy'].map((c): [string, number] => [c, 0.5]),
  ...[...'mbpмбп'].map((c): [string, number] => [c, 0]),
  ...[...'fvwфв'].map((c): [string, number] => [c, 0.15]),
])
const PAUSE_LONG = new Set(['.', '!', '?', '…', '。', '！', '？'])
const PAUSE_SHORT = new Set([',', ';', ':', '—', '–', '，', '；', '：', '、'])

/**
 * When the mouth is how far open while `text` is spoken at `charsPerSecond`: vowels open
 * it, m, b and p close it, punctuation is a pause. An estimate, not phonetics: it is
 * enough for a small face, and `realign` corrects it where a voice reports its position.
 */
export function mouthTimeline(text: string, charsPerSecond: number): MouthTimeline {
  const source = typeof text === 'string' ? text : ''
  const chars = [...source.toLowerCase()]
  const cps = typeof charsPerSecond === 'number' && Number.isFinite(charsPerSecond) && charsPerSecond > 0 ? charsPerSecond : CHARS_PER_SECOND
  // Each character takes a share of the whole: a pause takes longer than a letter.
  const level: number[] = []
  const weight: number[] = []
  for (const ch of chars) {
    if (PAUSE_LONG.has(ch)) {
      level.push(0)
      weight.push(3.5)
    } else if (PAUSE_SHORT.has(ch)) {
      level.push(0)
      weight.push(2)
    } else if (/\s/.test(ch)) {
      level.push(0.12)
      weight.push(0.6)
    } else {
      const known = OPEN.get(ch)
      // A letter this table does not know (another script): half open, varied a little so it still moves.
      level.push(known ?? (/[\p{L}\p{N}]/u.test(ch) ? 0.3 + ((ch.codePointAt(0) ?? 0) % 5) * 0.1 : 0.2))
      weight.push(1)
    }
  }
  const total = weight.reduce((a, b) => a + b, 0)
  const durationMs = total > 0 ? (source.length / cps) * 1000 : 0
  const starts: number[] = []
  let sum = 0
  for (const w of weight) {
    starts.push(total > 0 ? (sum / total) * durationMs : 0)
    sum += w
  }
  // `charIndex` of a boundary event counts UTF-16 units; the table above counts characters.
  const unitToChar: number[] = []
  chars.forEach((ch, i) => {
    for (let k = 0; k < ch.length; k++) unitToChar.push(i)
  })
  let shift = 0
  return {
    durationMs,
    at(ms) {
      if (typeof ms !== 'number' || !Number.isFinite(ms) || starts.length === 0) return 0
      const t = ms - shift
      if (t < 0 || t >= durationMs) return 0
      let lo = 0
      let hi = starts.length
      while (lo < hi) {
        const mid = (lo + hi) >>> 1
        if (starts[mid] <= t) lo = mid + 1
        else hi = mid
      }
      return level[Math.max(lo - 1, 0)] ?? 0
    },
    realign(charIndex, ms) {
      if (typeof charIndex !== 'number' || typeof ms !== 'number' || !Number.isFinite(charIndex) || !Number.isFinite(ms)) return
      const index = unitToChar[Math.max(0, Math.floor(charIndex))]
      if (index === undefined) return
      shift = ms - starts[index]
    },
  }
}

// ---------------------------------------------------------------- the speaker

/** The parts of a `SpeechSynthesisUtterance` the speaker sets and listens to. */
export interface UtteranceLike {
  voice: unknown
  lang: string
  pitch: number
  rate: number
  volume: number
  onstart: (() => void) | null
  onend: (() => void) | null
  onerror: ((e: { error?: string }) => void) | null
  onboundary: ((e: { charIndex: number }) => void) | null
}

/** The parts of `speechSynthesis` the speaker uses. It never calls `pause()`. */
export interface SynthLike {
  speak(u: UtteranceLike): void
  cancel(): void
  resume(): void
  getVoices(): VoiceLike[]
  addEventListener(type: 'voiceschanged', cb: () => void): void
  removeEventListener(type: 'voiceschanged', cb: () => void): void
}

export interface SpeakerHost {
  /** null: this browser cannot speak. */
  synth: SynthLike | null
  utterance(text: string): UtteranceLike
  now(): number
  setTimeout(cb: () => void, ms: number): number
  clearTimeout(id: number): void
  /** Whether this page load has had a user gesture: before one, browsers refuse to speak. */
  unlocked(): boolean
  /** Whether a dictation is live anywhere in the page: nothing is spoken into an open microphone. */
  micLive(): boolean
}

/**
 * The synthesizer and the utterance constructor of a window, as the speaker wants them.
 * The DOM's own typings do not fit `SynthLike` and `UtteranceLike` (its event handlers take
 * the event objects of the DOM), so the one cast lives here. `win` is the window, passed
 * in: nothing in this file reads a global. A browser without speech synthesis gives
 * `synth: null`, and the speaker then shows captions.
 */
export function browserSpeech(win: { speechSynthesis?: unknown; SpeechSynthesisUtterance?: unknown } | null | undefined): Pick<SpeakerHost, 'synth' | 'utterance'> {
  const synth: unknown = win?.speechSynthesis
  const Utterance: unknown = win?.SpeechSynthesisUtterance
  const usable = typeof synth === 'object' && synth !== null && typeof (synth as SynthLike).speak === 'function' && typeof Utterance === 'function'
  return {
    synth: usable ? (synth as SynthLike) : null,
    utterance(text) {
      if (typeof Utterance !== 'function') throw new Error('This browser has no speech synthesis.')
      return new (Utterance as new (text: string) => UtteranceLike)(text)
    },
  }
}

export interface SpeakPrefs {
  /** The user chose voice in this browser. */
  voice: boolean
  /** 0 to 1. */
  volume: number
  /** 0.7 to 1.5; multiplies the avatar's own rate. */
  speed: number
  /** The user allowed online voices (what is said then goes to the browser maker's speech service). */
  online: boolean
  /** The voice remembered for each terminal (the preferences' `voices`), so an avatar keeps its voice across reloads. */
  voices?: Readonly<Record<string, string>>
}

export interface SpeakItem {
  /** `lineKey(terminalId, line)`. */
  key: string
  terminalId: string
  lineId: string
  text: string
  lang: string
  voice: Voice
  /** The terminal id: voice and pitch derive from it. */
  seed: string
  /** `voice`: say it if this browser can right now; `silent`: the caption alone. */
  mode: 'voice' | 'silent'
  /** Higher first: an ask's question before other lines, the session on screen before others. */
  priority: number
  /**
   * Asked when a `voice` line reaches the head of the queue: whether this tab may still
   * say it (the arbiter renews its lease). false, or no answer in time: it is presented silently.
   */
  confirm?: () => Promise<boolean>
}

export interface SpeakerEvents {
  /**
   * A presentation begins. It can come twice for one line: when the browser engine gives
   * up in the middle, the rest is presented by the silent one.
   */
  start(key: string, engine: 'browser' | 'silent', chunks: readonly string[], voiceName: string | null): void
  /** The chunk at `index` is being said (or shown) now. */
  chunk(key: string, index: number): void
  /**
   * The line is over. `spoken`: the engine finished it; `shown`: a caption only (voice off,
   * not chosen or locked, or the line gave way to a newer one before it started);
   * `skipped`: stopped; `failed`: voice was on and the browser could not say it.
   */
  end(key: string, outcome: 'spoken' | 'shown' | 'skipped' | 'failed'): void
  /** The browser refused to speak: a user gesture is needed (`unlock`). */
  locked(): void
}

export interface Speaker {
  /**
   * A line to present when its turn comes. The same line again keeps its turn and takes
   * the new item. The line being presented is left alone, with one exception: a caption
   * that is asked for in mode `voice` starts over aloud, when voice is on and no
   * microphone is live (the user chose to hear it; see `Arbiter.replay`).
   */
  enqueue(item: SpeakItem): void
  /** Stops the line being presented (with a key: only that line, also while it waits). Its outcome is `skipped`. */
  stop(key?: string): void
  /** Everything of one avatar: its line being presented stops, its waiting line is forgotten. */
  drop(terminalId: string): void
  /** Call inside a user gesture's handler: the speaker says one empty utterance, which is what unlocks speech. */
  unlock(): void
  setPrefs(p: SpeakPrefs): void
  current(): { key: string; terminalId: string } | null
  /**
   * How open this avatar's mouth is now, 0 to 1, while one of its lines is presented;
   * a negative number when none is. Made for `FigureHandle.setMouth`.
   */
  mouth(terminalId: string): number
  /** Further listeners (the arbiter's). Returns the function that removes them. */
  subscribe(on: Partial<SpeakerEvents>): () => void
  dispose(): void
}

/** A line that has waited this long is presented silently when its turn comes. */
export const WAIT_SILENT_MS = 20_000
/** No `start` this long after `speak()`: the chunk is given up. */
export const START_WATCHDOG_MS = 4000
/** No `end` this long after a chunk should have ended: it is given up. */
export const END_WATCHDOG_MS = 6000
/** After a `cancel()` the next `speak()` waits this long: browsers drop an utterance spoken straight after one. */
export const CANCEL_GAP_MS = 80
/** The first use waits this long for the browser's voices to load. */
export const VOICES_WAIT_MS = 2000
/** A silent presentation lasts at least this long. */
export const SILENT_MIN_MS = 1500
/** How long the speaker waits for `confirm` before it presents the line silently. */
export const CONFIRM_WAIT_MS = 3000
const MIC_POLL_MS = 150
const MAX_TEXT_CHARS = 2000
const MAX_LEARNED = 64
const MAX_ASSIGNED = 256
const LANG = /^[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,8}){0,2}$/

const clamp = (n: unknown, min: number, max: number, fallback: number): number => (typeof n === 'number' && Number.isFinite(n) ? Math.min(Math.max(n, min), max) : fallback)

interface Waiting {
  item: SpeakItem
  /** When it joined the queue. */
  at: number
  seq: number
}

interface Run {
  item: SpeakItem
  engine: 'pending' | 'browser' | 'silent'
  chunks: string[]
  timeline: MouthTimeline | null
  chunkAt: number
  voiceName: string | null
  /** Its voice is the browser maker's speech service: it may speak only while the user allows online voices. */
  online: boolean
  rate: number
  /** How many chunks the synthesizer reported as started. */
  started: number
  /** Watchdog expiries in a row. */
  expiries: number
  timers: Set<number>
}

/**
 * The window's speaker. `on` hears every event; so does anything given to `subscribe`.
 * Events are delivered after the speaker's own state has changed, so a listener may call
 * back into it.
 */
export function createSpeaker(host: SpeakerHost, prefs: SpeakPrefs, on: SpeakerEvents): Speaker {
  const listeners = new Set<Partial<SpeakerEvents>>([on])
  let settings = cleanPrefs(prefs)
  let queue: Waiting[] = []
  let cur: Run | null = null
  let seq = 0
  /** Bumped with every `speak()` and every give-up: an event of an older generation is ignored. */
  let gen = 0
  /** The utterances the browser still holds. Kept until their `end` or `error`, so they are not collected mid-speech. */
  let inflight: UtteranceLike[] = []
  /** When this speaker last called `cancel()`; null: never. */
  let cancelledAt: number | null = null
  /** The browser refused to speak; cleared by `unlock`. */
  let refused = false
  let voicesWaited = false
  let disposed = false
  let pumping = false
  /** Terminal -> the voice it got, and how many others had that voice before it. */
  const assigned = new Map<string, { name: string; repeats: number }>()
  /** Voice -> characters a second at rate 1, learned from chunks that were spoken. */
  const learned = new Map<string, number>()
  const loose = new Set<number>()

  function cleanPrefs(p: SpeakPrefs): SpeakPrefs {
    const voices: unknown = p?.voices
    return {
      voice: p?.voice === true,
      volume: clamp(p?.volume, 0, 1, 1),
      speed: clamp(p?.speed, 0.7, 1.5, 1),
      online: p?.online === true,
      voices: typeof voices === 'object' && voices !== null ? (voices as Readonly<Record<string, string>>) : undefined,
    }
  }

  const emit = <K extends keyof SpeakerEvents>(name: K, ...args: Parameters<SpeakerEvents[K]>) => {
    if (disposed) return
    for (const listener of listeners) {
      const fn = listener[name] as ((...a: Parameters<SpeakerEvents[K]>) => void) | undefined
      if (typeof fn !== 'function') continue
      try {
        fn(...args)
      } catch {
        // A listener that throws must not stop the line or the other listeners.
      }
    }
  }

  const now = (): number => {
    const t = host.now()
    return typeof t === 'number' && Number.isFinite(t) ? t : 0
  }

  /** A timer of the line being presented: cleared when the line ends. */
  const later = (run: Run, cb: () => void, ms: number): number => {
    const id = host.setTimeout(() => {
      run.timers.delete(id)
      if (cur === run && !disposed) cb()
    }, Math.max(0, ms))
    run.timers.add(id)
    return id
  }
  const clear = (run: Run, id: number | null) => {
    if (id === null) return
    run.timers.delete(id)
    host.clearTimeout(id)
  }
  const clearAll = (run: Run) => {
    for (const id of run.timers) host.clearTimeout(id)
    run.timers.clear()
  }

  const micLive = (): boolean => {
    try {
      return host.micLive() === true
    } catch {
      return false
    }
  }
  const unlocked = (): boolean => {
    try {
      return host.unlocked() === true && !refused
    } catch {
      return false
    }
  }

  /** Only ever for this speaker's own utterances, never at start-up. */
  const cancelOwn = () => {
    if (inflight.length === 0 || !host.synth) return
    gen++
    inflight = []
    cancelledAt = now()
    try {
      host.synth.cancel()
    } catch {
      // Nothing to cancel then.
    }
  }

  const rateOf = (item: SpeakItem): number => clamp(clamp(item.voice?.rate, 0.8, 1.3, 1) * settings.speed, 0.5, 2, 1)
  const langOf = (item: SpeakItem): string => (typeof item.lang === 'string' && LANG.test(item.lang) ? item.lang : typeof item.voice?.lang === 'string' && LANG.test(item.voice.lang) ? item.voice.lang : 'en-US')
  const textOf = (item: SpeakItem): string => (typeof item.text === 'string' ? item.text.slice(0, MAX_TEXT_CHARS) : '')

  const finish = (run: Run, outcome: 'spoken' | 'shown' | 'skipped' | 'failed') => {
    if (cur !== run) return
    clearAll(run)
    gen++
    cur = null
    emit('end', run.item.key, outcome)
    pump()
  }

  /** The caption alone, from chunk `from` on, for as long as it would take to say; then `outcome`. */
  const silent = (run: Run, from: number, outcome: 'shown' | 'failed') => {
    if (cur !== run) return
    clearAll(run)
    gen++
    run.engine = 'silent'
    run.timeline = null
    run.voiceName = null
    const startedAt = now()
    emit('start', run.item.key, 'silent', run.chunks, null)
    if (cur !== run) return
    const cps = CHARS_PER_SECOND * run.rate
    const step = (i: number) => {
      if (cur !== run) return
      if (i >= run.chunks.length) {
        run.timeline = null
        later(run, () => finish(run, outcome), SILENT_MIN_MS - (now() - startedAt))
        return
      }
      run.timeline = mouthTimeline(run.chunks[i], cps)
      run.chunkAt = now()
      emit('chunk', run.item.key, i)
      if (cur !== run) return
      later(run, () => step(i + 1), run.timeline.durationMs)
    }
    step(Math.max(0, from))
  }

  const voicesNow = (): VoiceLike[] => {
    try {
      const list = host.synth?.getVoices()
      return Array.isArray(list) ? list : []
    } catch {
      return []
    }
  }

  /** The browser's voices; the first use waits for them to load, at most two seconds. */
  const withVoices = (run: Run, then: (voices: VoiceLike[]) => void) => {
    const list = voicesNow()
    const synth = host.synth
    if (list.length > 0 || voicesWaited || !synth) {
      then(list)
      return
    }
    let done = false
    const ready = () => {
      if (done) return
      done = true
      voicesWaited = true
      try {
        synth.removeEventListener('voiceschanged', ready)
      } catch {
        // Not listening then.
      }
      host.clearTimeout(timer)
      loose.delete(timer)
      if (cur === run && !disposed) then(voicesNow())
    }
    const timer = host.setTimeout(ready, VOICES_WAIT_MS)
    loose.add(timer)
    try {
      synth.addEventListener('voiceschanged', ready)
    } catch {
      // The timeout still answers.
    }
  }

  const cpsOf = (run: Run): number => (learned.get(run.voiceName ?? '') ?? CHARS_PER_SECOND) * run.rate

  const learn = (run: Run, chars: number, ms: number) => {
    if (!run.voiceName || chars < 8 || !(ms >= 200)) return
    const perSecond = clamp(chars / (ms / 1000) / run.rate, 6, 40, CHARS_PER_SECOND)
    const before = learned.get(run.voiceName)
    learned.delete(run.voiceName)
    learned.set(run.voiceName, before === undefined ? perSecond : before * 0.6 + perSecond * 0.4)
    if (learned.size > MAX_LEARNED) learned.delete(learned.keys().next().value as string)
  }

  /** One chunk as one utterance, watched from `speak()` to its `end`. */
  const say = (run: Run, voice: VoiceLike, pitch: number, i: number) => {
    if (cur !== run) return
    const synth = host.synth
    if (i >= run.chunks.length) {
      // Not one chunk ever started: nothing was said, whatever was asked of the synthesizer.
      finish(run, run.started > 0 ? 'spoken' : 'failed')
      return
    }
    if (!synth) {
      silent(run, i, 'failed')
      return
    }
    // A dictation that started meanwhile: the line stops rather than talk into the microphone.
    // So it does when voice was turned off, or online voices were no longer allowed for an online one.
    if (micLive() || !settings.voice || (run.online && !settings.online)) {
      cancelOwn()
      finish(run, 'skipped')
      return
    }
    const gap = cancelledAt === null ? 0 : cancelledAt + CANCEL_GAP_MS - now()
    if (gap > 0) {
      later(run, () => say(run, voice, pitch, i), gap)
      return
    }
    run.timeline = null
    const chunk = run.chunks[i]
    let utterance: UtteranceLike
    try {
      utterance = host.utterance(chunk)
    } catch {
      silent(run, i, 'failed')
      return
    }
    const mine = ++gen
    let started = false
    let watchdog: number | null = null
    const settle = () => {
      inflight = inflight.filter((u) => u !== utterance)
    }
    /** The chunk is given up: cancel it and go on; twice in a row and the engine has failed for this line. */
    const expire = () => {
      if (mine !== gen || cur !== run) return
      cancelOwn()
      run.timeline = null
      run.expiries++
      if (run.expiries >= 2) silent(run, i + 1, 'failed')
      else say(run, voice, pitch, i + 1)
    }
    utterance.voice = voice
    utterance.lang = langOf(run.item)
    utterance.pitch = pitch
    utterance.rate = run.rate
    utterance.volume = settings.volume
    utterance.onstart = () => {
      if (mine !== gen || cur !== run || started) return
      started = true
      run.started++
      clear(run, watchdog)
      run.chunkAt = now()
      run.timeline = mouthTimeline(chunk, cpsOf(run))
      watchdog = later(run, expire, run.timeline.durationMs + END_WATCHDOG_MS)
      emit('chunk', run.item.key, i)
    }
    utterance.onboundary = (e) => {
      if (mine !== gen || cur !== run || !started) return
      run.timeline?.realign(e?.charIndex, now() - run.chunkAt)
    }
    utterance.onend = () => {
      settle()
      if (mine !== gen || cur !== run) return
      clear(run, watchdog)
      if (started) learn(run, chunk.length, now() - run.chunkAt)
      run.timeline = null
      run.expiries = 0
      say(run, voice, pitch, i + 1)
    }
    utterance.onerror = (e) => {
      settle()
      if (mine !== gen || cur !== run) return
      clear(run, watchdog)
      run.timeline = null
      const code = typeof e?.error === 'string' ? e.error : ''
      if (code === 'interrupted' || code === 'canceled') {
        // A stop from somewhere else (another tab shares the browser's speech queue): never a failure.
        finish(run, 'skipped')
      } else if (code === 'not-allowed') {
        refused = true
        emit('locked')
        silent(run, i, 'shown')
      } else {
        silent(run, i, 'failed')
      }
    }
    inflight.push(utterance)
    // The watchdog is set before `speak()`: a synthesizer may report the start at once.
    watchdog = later(run, expire, START_WATCHDOG_MS)
    try {
      synth.resume()
      synth.speak(utterance)
    } catch {
      settle()
      silent(run, i, 'failed')
    }
  }

  const aloud = (run: Run) => {
    withVoices(run, (all) => {
      if (cur !== run) return
      // Things may have changed while the voices loaded or the lease was renewed.
      if (!settings.voice || micLive()) {
        silent(run, 0, 'shown')
        return
      }
      if (!unlocked()) {
        emit('locked')
        silent(run, 0, 'shown')
        return
      }
      const tid = run.item.terminalId
      const taken = new Map<string, number>()
      for (const [other, a] of assigned) if (other !== tid) taken.set(a.name, (taken.get(a.name) ?? 0) + 1)
      const kept = assigned.get(tid)?.name ?? (settings.voices && Object.hasOwn(settings.voices, tid) ? settings.voices[tid] : undefined)
      const voice = pickVoice(usableVoices(all, { online: settings.online }), run.item.voice, run.item.seed, { lang: langOf(run.item), taken, remembered: typeof kept === 'string' ? kept : undefined })
      if (!voice) {
        // Voice is on and this browser has no voice it may use for the language.
        silent(run, 0, 'failed')
        return
      }
      const had = assigned.get(tid)
      const mine = had?.name === voice.name ? had : { name: voice.name, repeats: taken.get(voice.name) ?? 0 }
      assigned.set(tid, mine)
      // Avatars that were never dropped do not pile up for ever: the one that spoke first longest ago goes.
      if (assigned.size > MAX_ASSIGNED) assigned.delete(assigned.keys().next().value as string)
      run.engine = 'browser'
      run.voiceName = voice.name
      run.online = voice.localService !== true
      const pitch = pitchFor(run.item.voice, run.item.seed, mine.repeats)
      emit('start', run.item.key, 'browser', run.chunks, voice.name)
      if (cur !== run) return
      // While it speaks: a dictation that starts stops the line.
      const watch = () => {
        if (cur !== run || run.engine !== 'browser') return
        if (micLive()) {
          cancelOwn()
          finish(run, 'skipped')
          return
        }
        later(run, watch, MIC_POLL_MS)
      }
      later(run, watch, MIC_POLL_MS)
      say(run, voice, pitch, 0)
    })
  }

  const begin = (w: Waiting) => {
    const item = w.item
    const rate = rateOf(item)
    const run: Run = { item, engine: 'pending', chunks: chunkText(textOf(item), chunkLimit(CHARS_PER_SECOND * rate)), timeline: null, chunkAt: 0, voiceName: null, online: false, rate, started: 0, expiries: 0, timers: new Set() }
    cur = run
    if (run.chunks.length === 0) {
      // Nothing to say and nothing to show.
      finish(run, 'shown')
      return
    }
    if (item.mode !== 'voice' || !settings.voice || now() - w.at >= WAIT_SILENT_MS || micLive()) {
      silent(run, 0, 'shown')
      return
    }
    if (!unlocked()) {
      emit('locked')
      if (cur === run) silent(run, 0, 'shown')
      return
    }
    if (!host.synth) {
      silent(run, 0, 'failed')
      return
    }
    if (typeof item.confirm !== 'function') {
      aloud(run)
      return
    }
    // The lease may have lapsed while the line waited: ask before saying it.
    let answered = false
    const answer = (ok: boolean) => {
      if (answered) return
      answered = true
      clear(run, timer)
      if (cur !== run) return
      if (ok) aloud(run)
      else silent(run, 0, 'shown')
    }
    const timer = later(run, () => answer(false), CONFIRM_WAIT_MS)
    try {
      item.confirm().then(
        (ok) => answer(ok === true),
        () => answer(false),
      )
    } catch {
      answer(false)
    }
  }

  function pump() {
    if (pumping || disposed) return
    pumping = true
    try {
      while (!cur && queue.length > 0 && !disposed) {
        // An ask's question before other lines, the session on screen before others, then arrival.
        let best = 0
        for (let i = 1; i < queue.length; i++) {
          const a = queue[i]
          const b = queue[best]
          const pa = clamp(a.item.priority, -1e6, 1e6, 0)
          const pb = clamp(b.item.priority, -1e6, 1e6, 0)
          if (pa > pb || (pa === pb && a.seq < b.seq)) best = i
        }
        const [next] = queue.splice(best, 1)
        begin(next)
      }
    } finally {
      pumping = false
    }
  }

  return {
    enqueue(item) {
      if (disposed || typeof item !== 'object' || item === null || typeof item.key !== 'string' || typeof item.terminalId !== 'string') return
      if (cur?.item.key === item.key) {
        // The line on screen as a caption, asked for again aloud (the user chose to hear it): it starts over, with the voice.
        if (item.mode === 'voice' && cur.engine === 'silent' && settings.voice && !micLive()) {
          clearAll(cur)
          begin({ item, at: now(), seq: seq++ })
        }
        return
      }
      const same = queue.find((w) => w.item.key === item.key)
      if (same) {
        // The same line again (its mode or its place may have changed): it keeps its turn.
        same.item = item
        return
      }
      // One waiting line per avatar: a newer one takes the place of the one that waits.
      const waiting = queue.find((w) => w.item.terminalId === item.terminalId)
      queue = queue.filter((w) => w !== waiting)
      queue.push({ item, at: now(), seq: seq++ })
      if (waiting) emit('end', waiting.item.key, 'shown')
      pump()
    },
    stop(key) {
      if (disposed) return
      if (cur && (key === undefined || cur.item.key === key)) {
        const run = cur
        cancelOwn()
        finish(run, 'skipped')
        return
      }
      if (key === undefined) return
      const waiting = queue.find((w) => w.item.key === key)
      if (!waiting) return
      queue = queue.filter((w) => w !== waiting)
      emit('end', key, 'skipped')
    },
    drop(terminalId) {
      if (disposed) return
      queue = queue.filter((w) => w.item.terminalId !== terminalId)
      assigned.delete(terminalId)
      if (cur?.item.terminalId === terminalId) {
        const run = cur
        cancelOwn()
        finish(run, 'skipped')
      }
    },
    unlock() {
      if (disposed) return
      refused = false
      const synth = host.synth
      // An utterance of nothing, said inside the gesture's handler: that is what browsers count.
      if (!synth || !settings.voice || inflight.length > 0) return
      try {
        const u = host.utterance('')
        u.volume = 0
        u.lang = 'en-US'
        const forget = () => {
          inflight = inflight.filter((x) => x !== u)
        }
        u.onend = forget
        u.onerror = (e) => {
          forget()
          if (e?.error === 'not-allowed' && !disposed) {
            refused = true
            emit('locked')
          }
        }
        inflight.push(u)
        const timer = host.setTimeout(() => {
          loose.delete(timer)
          forget()
        }, START_WATCHDOG_MS)
        loose.add(timer)
        synth.resume()
        synth.speak(u)
      } catch {
        // The next line finds out whether speech works.
      }
    },
    setPrefs(p) {
      if (disposed) return
      const was = settings.voice
      const wasOnline = settings.online
      settings = cleanPrefs(p)
      // Turning voice off stops the line being said. So does withdrawing the allow for online
      // voices while an online one says it: not one more word of it goes to that service.
      if (cur?.engine === 'browser' && ((was && !settings.voice) || (wasOnline && !settings.online && cur.online))) {
        const run = cur
        cancelOwn()
        finish(run, 'skipped')
      }
    },
    current: () => (cur ? { key: cur.item.key, terminalId: cur.item.terminalId } : null),
    mouth(terminalId) {
      if (!cur || cur.item.terminalId !== terminalId) return -1
      if (!cur.timeline) return 0
      return Math.min(Math.max(cur.timeline.at(now() - cur.chunkAt), 0), 1)
    },
    subscribe(listener) {
      if (typeof listener !== 'object' || listener === null) return () => {}
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    dispose() {
      if (disposed) return
      // Speech stops at once, and nothing is reported any more.
      if (cur) clearAll(cur)
      cancelOwn()
      disposed = true
      gen++
      for (const id of loose) host.clearTimeout(id)
      loose.clear()
      queue = []
      cur = null
      listeners.clear()
    },
  }
}
