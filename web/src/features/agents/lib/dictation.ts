// Voice input for the new-session prompt. The browser's own speech recognition (the Web
// Speech API: Chrome, Edge, Safari) turns what is said into text; Workbench never
// receives the audio, and the browser decides where it is processed.
//
// Everything here is plain TypeScript with the recognizer passed in, so it runs under
// vitest without a browser. `Dictate.tsx` is the React side.

/** The parts of a `SpeechRecognition` that dictation uses (TypeScript's DOM lib has no typings for it). */
export interface Recognizer {
  lang: string
  continuous: boolean
  interimResults: boolean
  onaudiostart: (() => void) | null
  onresult: ((e: RecognizerEvent) => void) | null
  onerror: ((e: { error?: string }) => void) | null
  onend: (() => void) | null
  start(): void
  stop(): void
  abort(): void
}

/** `results[i][0].transcript`: the best hypothesis of each result, final or not. */
export interface RecognizerEvent {
  results: ArrayLike<ArrayLike<{ transcript: string }>>
}

export type RecognizerCtor = new () => Recognizer

export type Support = { ok: true; ctor: RecognizerCtor } | { ok: false; reason: 'unsupported' | 'insecure'; message: string }

/** The slice of `window` that decides whether voice input can work. */
export interface SpeechHost {
  SpeechRecognition?: RecognizerCtor
  webkitSpeechRecognition?: RecognizerCtor
  isSecureContext?: boolean
}

export const INSECURE_MESSAGE =
  'Voice input needs a secure connection: open Workbench on this computer (localhost) or over https, for example with `tailscale serve`.'

/**
 * Whether this page can dictate. A browser without speech recognition (Firefox) is
 * `unsupported`; one that has it but is on plain http is `insecure`, which the user can fix.
 */
export function speechSupport(host: SpeechHost): Support {
  const ctor = host.SpeechRecognition ?? host.webkitSpeechRecognition
  if (!ctor) return { ok: false, reason: 'unsupported', message: 'This browser has no speech recognition.' }
  if (host.isSecureContext === false) return { ok: false, reason: 'insecure', message: INSECURE_MESSAGE }
  return { ok: true, ctor }
}

// Scripts written without spaces between words: no space is put between a result and the
// text beside it when either side is one.
const UNSPACED = /[぀-ヿ㐀-䶿一-鿿豈-﫿ｦ-ﾟ]/

/** Whether speech joined to text needs a space between `left` and `right`. */
export function needsSpace(left: string, right: string): boolean {
  if (!left || !right) return false
  if (/\s$/.test(left) || /^\s/.test(right)) return false
  return !(UNSPACED.test(left.slice(-1)) || UNSPACED.test(right[0]))
}

/** Everything the recognizer has heard in this session, in order. */
export function spokenText(results: RecognizerEvent['results']): string {
  let out = ''
  for (let i = 0; i < results.length; i++) {
    // Chrome starts later results with a space, Android and Safari do not: trim, then join.
    const part = results[i]?.[0]?.transcript?.trim()
    if (!part) continue
    out += needsSpace(out, part) ? ` ${part}` : part
  }
  return out
}

/** The field with `spoken` between `before` and `after`, and the caret's place (just after the speech). */
export function place(before: string, spoken: string, after: string): { value: string; caret: number } {
  const text = spoken.trim()
  if (!text) return { value: before + after, caret: before.length }
  const head = before + (needsSpace(before, text) ? ' ' : '') + text
  return { value: head + (needsSpace(text, after) ? ' ' : '') + after, caret: head.length }
}

/** Something the user should be told: `info` for what a retry fixes, `error` for what it does not. */
export interface Problem {
  level: 'info' | 'error'
  message: string
}

const failed = (message: string): Problem => ({ level: 'error', message })

/** What to tell the user about a recognizer's `error` code; null when nothing is worth saying. */
export function dictationProblem(code: string | undefined, lang: string): Problem | null {
  switch (code) {
    case 'aborted':
      return null // asked for: a stop, or another field took over
    case 'no-speech':
      return { level: 'info', message: 'No speech heard. Click the microphone and try again.' }
    case 'audio-capture':
      return failed('No microphone found.')
    case 'not-allowed':
      return failed('The microphone is blocked for this site. Allow it in the browser’s site settings, then try again.')
    case 'service-not-allowed':
      return failed('The browser’s speech recognition is not available (turned off by the browser or its policy).')
    case 'network':
      return failed('The browser could not reach its speech service. Voice input needs an internet connection.')
    case 'language-not-supported':
      return failed(`Speech recognition does not support ${lang}.`)
    default:
      return failed(`Voice input stopped (${code ?? 'unknown error'}).`)
  }
}

/** The text field dictation writes to, as it is when listening starts. */
export interface Field {
  value: string
  /** Selection: speech replaces it (an empty one is the caret). */
  start: number
  end: number
}

export type DictationState = 'idle' | 'starting' | 'listening'

export interface DictationHooks {
  field(): Field
  /** The field's new text, and where its caret goes. */
  text(value: string, caret: number): void
  state(s: DictationState): void
  problem(p: Problem): void
}

export interface Dictation {
  /** Stop listening and let what was heard so far arrive. */
  stop(): void
  /** Stop at once: nothing more reaches the field (the user typed, or the form went away). */
  cancel(): void
}

export interface DictationOptions {
  lang: string
  /**
   * One recognition per utterance (false) or until stopped (true). Android's recognizer
   * repeats earlier words in continuous mode, so touch devices take one utterance and the
   * button is tapped again for more.
   */
  continuous: boolean
}

/** The running dictation, if any: only one field listens at a time. */
let live: Dictation | null = null

/**
 * Listen and write what is heard into the field, from where its caret is. Each result
 * replaces the previous guess (`before` + speech + `after`), so interim text is corrected in
 * place and nothing is duplicated; the caller cancels when the user edits the field, as the
 * speech would overwrite the edit.
 */
export function startDictation(ctor: RecognizerCtor, opts: DictationOptions, hooks: DictationHooks): Dictation {
  live?.cancel()
  const field = hooks.field()
  const before = field.value.slice(0, field.start)
  const after = field.value.slice(field.end)
  const rec = new ctor()
  rec.lang = opts.lang
  rec.continuous = opts.continuous
  rec.interimResults = true
  // `over`: nothing this recognizer says is wanted any more. `stopping`: its last words
  // still are, but the user has already seen the microphone go off.
  let over = false
  let stopping = false
  // What the field holds after the last write (or when listening began).
  let shown = field.value

  const detach = () => {
    over = true
    rec.onaudiostart = rec.onresult = rec.onerror = rec.onend = null
    if (live === me) live = null
  }
  const me: Dictation = {
    stop() {
      if (over || stopping) return
      stopping = true
      hooks.state('idle')
      try {
        rec.stop()
      } catch {
        // already stopped
      }
    },
    cancel() {
      if (over) return
      detach()
      hooks.state('idle')
      try {
        rec.abort()
      } catch {
        // already stopped
      }
    },
  }

  rec.onaudiostart = () => {
    if (!over && !stopping) hooks.state('listening')
  }
  rec.onresult = (e) => {
    if (over) return
    if (!stopping) hooks.state('listening')
    const spoken = spokenText(e.results)
    // Nothing heard (yet): a selected passage stays until there are words to replace it.
    if (!spoken) return
    const { value, caret } = place(before, spoken, after)
    // Chrome ends an utterance by repeating its last guess as the final result. Writing the
    // same text again would change nothing on screen but leave the caret to be moved on
    // whatever the user types next.
    if (value === shown) return
    shown = value
    hooks.text(value, caret)
  }
  rec.onerror = (e) => {
    if (over) return
    const problem = dictationProblem(e.error, opts.lang)
    if (problem) hooks.problem(problem)
  }
  rec.onend = () => {
    if (over) return
    detach()
    hooks.state('idle')
  }

  live = me
  hooks.state('starting')
  try {
    rec.start()
  } catch (e) {
    detach()
    hooks.state('idle')
    hooks.problem(failed(`Voice input could not start: ${e instanceof Error ? e.message : String(e)}`))
  }
  return me
}
