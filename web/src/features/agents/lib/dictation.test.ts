import { afterEach, describe, expect, it } from 'vitest'
import {
  dictationProblem,
  INSECURE_MESSAGE,
  needsSpace,
  place,
  spokenText,
  speechSupport,
  startDictation,
  type DictationOptions,
  type DictationState,
  type Field,
  type Problem,
  type Recognizer,
  type RecognizerEvent,
} from './dictation'

/** A recognizer the test drives by hand, like the browser would. */
class Fake implements Recognizer {
  static made: Fake[] = []
  static failStart: Error | null = null
  lang = ''
  continuous = false
  interimResults = false
  onaudiostart: (() => void) | null = null
  onresult: ((e: RecognizerEvent) => void) | null = null
  onerror: ((e: { error?: string }) => void) | null = null
  onend: (() => void) | null = null
  started = 0
  stopped = 0
  aborted = 0
  constructor() {
    Fake.made.push(this)
  }
  start() {
    if (Fake.failStart) throw Fake.failStart
    this.started++
  }
  stop() {
    this.stopped++
  }
  abort() {
    this.aborted++
  }
  /** The browser reports what it has heard so far, one transcript per result. */
  hear(...transcripts: string[]) {
    this.onresult?.({ results: transcripts.map((transcript) => [{ transcript }]) })
  }
}

const OPTS: DictationOptions = { lang: 'en-US', continuous: true }

/** A text field and what dictation told it. */
function harness(field: Field) {
  const log = { value: field.value, caret: field.end, states: [] as DictationState[], problems: [] as string[], levels: [] as string[] }
  const hooks = {
    field: () => field,
    text: (value: string, caret: number) => {
      log.value = value
      log.caret = caret
    },
    state: (s: DictationState) => log.states.push(s),
    problem: (p: Problem) => {
      log.problems.push(p.message)
      log.levels.push(p.level)
    },
  }
  return { log, hooks }
}

const open = (field: Field, opts: DictationOptions = OPTS) => {
  const h = harness(field)
  const d = startDictation(Fake, opts, h.hooks)
  return { ...h, d, rec: Fake.made[Fake.made.length - 1] }
}

afterEach(() => {
  // Whatever a test left listening must not leak into the next one.
  for (const r of Fake.made) r.onend?.()
  Fake.made = []
  Fake.failStart = null
})

describe('speechSupport', () => {
  it('finds the recognizer, with or without the prefix', () => {
    expect(speechSupport({ SpeechRecognition: Fake })).toEqual({ ok: true, ctor: Fake })
    expect(speechSupport({ webkitSpeechRecognition: Fake, isSecureContext: true })).toEqual({ ok: true, ctor: Fake })
  })

  it('says why it cannot work', () => {
    expect(speechSupport({})).toMatchObject({ ok: false, reason: 'unsupported' })
    expect(speechSupport({ SpeechRecognition: Fake, isSecureContext: false })).toEqual({ ok: false, reason: 'insecure', message: INSECURE_MESSAGE })
  })
})

describe('needsSpace', () => {
  it('spaces words and leaves existing spaces alone', () => {
    expect(needsSpace('fix', 'the bug')).toBe(true)
    expect(needsSpace('fix ', 'the bug')).toBe(false)
    expect(needsSpace('fix', ' the bug')).toBe(false)
    expect(needsSpace('fix\n', 'the bug')).toBe(false)
    expect(needsSpace('', 'fix')).toBe(false)
    expect(needsSpace('fix', '')).toBe(false)
  })

  it('does not split scripts written without spaces', () => {
    expect(needsSpace('こんにちは', '世界')).toBe(false)
    expect(needsSpace('你好', '世界')).toBe(false)
    expect(needsSpace('fix', '世界')).toBe(false)
    expect(needsSpace('世界', 'fix')).toBe(false)
  })
})

describe('spokenText', () => {
  it('joins results whether or not the browser starts them with a space', () => {
    expect(spokenText([[{ transcript: 'fix the' }], [{ transcript: ' failing test' }]])).toBe('fix the failing test')
    expect(spokenText([[{ transcript: 'fix the' }], [{ transcript: 'failing test' }]])).toBe('fix the failing test')
  })

  it('skips empty results and takes the best hypothesis', () => {
    expect(spokenText([[{ transcript: '  ' }], [{ transcript: 'hello' }, { transcript: 'hollow' }], []])).toBe('hello')
    expect(spokenText([])).toBe('')
  })
})

describe('place', () => {
  it('puts speech between the text around the caret, spaced', () => {
    expect(place('Fix', 'the bug', ' now')).toEqual({ value: 'Fix the bug now', caret: 'Fix the bug'.length })
    expect(place('Fix ', 'the bug', '')).toEqual({ value: 'Fix the bug', caret: 11 })
    expect(place('', 'fix the bug', '')).toEqual({ value: 'fix the bug', caret: 11 })
    expect(place('Fix', 'it', 'now')).toEqual({ value: 'Fix it now', caret: 6 })
    expect(place('Fix\n', 'it', '')).toEqual({ value: 'Fix\nit', caret: 6 })
  })

  it('leaves the text as it was when nothing was heard', () => {
    expect(place('Fix', '  ', ' now')).toEqual({ value: 'Fix now', caret: 3 })
  })
})

describe('dictationProblem', () => {
  it('stays quiet about a stop it asked for', () => {
    expect(dictationProblem('aborted', 'en-US')).toBeNull()
  })

  it('calls a silent microphone a hint and the rest failures', () => {
    expect(dictationProblem('no-speech', 'en-US')?.level).toBe('info')
    for (const code of ['not-allowed', 'audio-capture', 'network', 'service-not-allowed', 'language-not-supported', 'weird', undefined]) {
      expect(dictationProblem(code, 'en-US')?.level, String(code)).toBe('error')
    }
  })

  it('explains what the user can do', () => {
    expect(dictationProblem('not-allowed', 'en-US')?.message).toContain('site settings')
    expect(dictationProblem('audio-capture', 'en-US')?.message).toContain('No microphone')
    expect(dictationProblem('network', 'en-US')?.message).toContain('internet')
    expect(dictationProblem('language-not-supported', 'uk-UA')?.message).toContain('uk-UA')
    expect(dictationProblem('weird', 'en-US')?.message).toContain('weird')
    expect(dictationProblem(undefined, 'en-US')?.message).toContain('unknown error')
  })
})

describe('startDictation', () => {
  it('listens in the user’s language and shows interim results', () => {
    const { rec, log } = open({ value: '', start: 0, end: 0 }, { lang: 'de-DE', continuous: false })
    expect(rec).toMatchObject({ lang: 'de-DE', continuous: false, interimResults: true, started: 1 })
    expect(log.states).toEqual(['starting'])
  })

  it('goes starting, listening, idle', () => {
    const { rec, log } = open({ value: '', start: 0, end: 0 })
    rec.onaudiostart?.()
    rec.hear('fix')
    rec.onend?.()
    expect(log.states).toEqual(['starting', 'listening', 'listening', 'idle'])
  })

  it('corrects interim text in place instead of repeating it', () => {
    const { rec, log } = open({ value: 'Please', start: 6, end: 6 })
    rec.hear('fix')
    expect(log.value).toBe('Please fix')
    rec.hear('fix the')
    expect(log.value).toBe('Please fix the')
    rec.hear('fix the failing', ' test')
    expect(log.value).toBe('Please fix the failing test')
    expect(log.caret).toBe('Please fix the failing test'.length)
  })

  it('does not write the same text twice (the final result repeats the last guess)', () => {
    const writes: string[] = []
    const field: Field = { value: 'Fix', start: 3, end: 3 }
    const rec = (() => {
      startDictation(Fake, OPTS, { field: () => field, text: (v) => writes.push(v), state: () => {}, problem: () => {} })
      return Fake.made[Fake.made.length - 1]
    })()
    rec.hear('this')
    rec.hear('this')
    rec.hear('this', ' now')
    rec.hear('this now')
    expect(writes).toEqual(['Fix this', 'Fix this now'])
  })

  it('keeps a selected passage until there are words to replace it', () => {
    const writes: string[] = []
    const field: Field = { value: 'Fix THIS now', start: 4, end: 8 }
    startDictation(Fake, OPTS, { field: () => field, text: (v) => writes.push(v), state: () => {}, problem: () => {} })
    const rec = Fake.made[Fake.made.length - 1]
    rec.hear('  ')
    expect(writes).toEqual([])
    rec.hear('that')
    expect(writes).toEqual(['Fix that now'])
  })

  it('writes at the caret and replaces a selection', () => {
    const middle = open({ value: 'Fix  now', start: 4, end: 4 })
    middle.rec.hear('it')
    expect(middle.log.value).toBe('Fix it now')
    const selected = open({ value: 'Fix THIS now', start: 4, end: 8 })
    selected.rec.hear('the login')
    expect(selected.log.value).toBe('Fix the login now')
  })

  it('stop() lets the last words arrive without the microphone coming back on', () => {
    const { rec, log, d } = open({ value: '', start: 0, end: 0 })
    rec.onaudiostart?.()
    d.stop()
    expect(rec.stopped).toBe(1)
    rec.hear('almost forgot this')
    rec.onend?.()
    expect(log.value).toBe('almost forgot this')
    expect(log.states).toEqual(['starting', 'listening', 'idle', 'idle'])
    d.stop()
    expect(rec.stopped).toBe(1)
  })

  it('cancel() drops late words, so an edit or a sent prompt is not overwritten', () => {
    const { rec, log, d } = open({ value: 'Fix', start: 3, end: 3 })
    rec.hear('this')
    d.cancel()
    expect(rec.aborted).toBe(1)
    // Handlers are gone; even one the browser still holds would be ignored.
    expect(rec.onresult).toBeNull()
    expect(log.value).toBe('Fix this')
    d.cancel()
    expect(rec.aborted).toBe(1)
  })

  it('lets only one field listen: the next start cancels the previous', () => {
    const first = open({ value: '', start: 0, end: 0 })
    const second = open({ value: '', start: 0, end: 0 })
    expect(first.rec.aborted).toBe(1)
    expect(first.log.states.at(-1)).toBe('idle')
    second.rec.hear('hello')
    expect(second.log.value).toBe('hello')
    expect(first.log.value).toBe('')
  })

  it('reports why it stopped, once, and nothing for its own abort', () => {
    const { rec, log } = open({ value: '', start: 0, end: 0 })
    rec.onerror?.({ error: 'not-allowed' })
    rec.onend?.()
    expect(log.problems).toHaveLength(1)
    expect(log.problems[0]).toContain('microphone is blocked')
    expect(log.levels).toEqual(['error'])
    expect(log.states.at(-1)).toBe('idle')
    const quiet = open({ value: '', start: 0, end: 0 })
    quiet.rec.onerror?.({ error: 'aborted' })
    expect(quiet.log.problems).toEqual([])
  })

  it('survives a recognizer that cannot start', () => {
    Fake.failStart = new Error('already started')
    const { log } = open({ value: 'x', start: 1, end: 1 })
    expect(log.states).toEqual(['starting', 'idle'])
    expect(log.problems).toEqual(['Voice input could not start: already started'])
    // And it does not stay "live": the next dictation starts clean.
    Fake.failStart = null
    const next = open({ value: '', start: 0, end: 0 })
    expect(next.rec.started).toBe(1)
  })
})
