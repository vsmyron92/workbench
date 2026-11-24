// The microphone button, rendered on the server: nothing where the browser cannot
// recognize speech, a reason (not a dead button) where it only needs https, its
// listening look, and the label another field gives it. Then the hook behind it, taken
// from one server render: the language it listens in and where its problems go.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { useToasts } from '@/shell/actions'
import { DictateButton, useDictation, type Dictating, type UseDictationOptions } from './Dictate'
import { INSECURE_MESSAGE, type Problem, type Recognizer } from './lib/dictation'

const noop = () => {}
const ctor = class {} as unknown as new () => Recognizer

function render(over: Partial<Dictating>, label?: string) {
  const d: Dictating = { support: { ok: true, ctor }, state: 'idle', toggle: noop, cancel: noop, ...over }
  return renderToStaticMarkup(createElement(DictateButton, label === undefined ? { d } : { d, label }))
}

describe('DictateButton', () => {
  it('is absent in a browser without speech recognition', () => {
    expect(render({ support: { ok: false, reason: 'unsupported', message: 'no' } })).toBe('')
  })

  it('offers the microphone, and says where the audio goes', () => {
    const html = render({})
    expect(html).toContain('aria-pressed="false"')
    expect(html).toContain('wb-ag-mic')
    expect(html).toContain('your browser’s speech recognition hears it, not Workbench')
    expect(html).not.toContain('Listening')
    expect(html).not.toContain('disabled')
  })

  it('keeps the button on plain http, with the reason as its label', () => {
    const html = render({ support: { ok: false, reason: 'insecure', message: INSECURE_MESSAGE } })
    expect(html).toContain('unavailable')
    expect(html).toContain('needs a secure connection')
    expect(html).not.toContain('disabled')
  })

  it('shows that it listens', () => {
    const html = render({ state: 'listening' })
    expect(html).toContain('Listening…')
    expect(html).toContain('wb-ag-mic listening')
    expect(html).toContain('aria-pressed="true"')
    expect(html).toContain('your browser’s speech recognition hears it, not Workbench')
    // Waiting for the browser's permission prompt: pressed, but not yet listening.
    const asking = render({ state: 'starting' })
    expect(asking).toContain('aria-pressed="true"')
    expect(asking).not.toContain('Listening')
    expect(asking).not.toContain('listening')
  })

  it('is called what another field calls it, and keeps the prompt’s label without one', () => {
    const reply = 'Speak your reply (your browser’s speech recognition hears it, not Workbench)'
    const html = render({}, reply)
    expect(html).toContain(`aria-label="${reply}"`)
    expect(html).toContain(`title="${reply}"`)
    expect(html).not.toContain('Speak the prompt')
    // The same button otherwise, listening look included.
    expect(html).toContain('wb-ag-mic')
    expect(render({ state: 'listening' }, reply)).toContain('wb-ag-mic listening')
    for (const none of [undefined, '']) expect(render({}, none)).toContain('aria-label="Speak the prompt (your browser’s speech recognition hears it, not Workbench)"')
  })

  it('says why it cannot work whatever the field calls it', () => {
    const reply = 'Speak your reply'
    const insecure = render({ support: { ok: false, reason: 'insecure', message: INSECURE_MESSAGE } }, reply)
    expect(insecure).toContain('needs a secure connection')
    expect(insecure).not.toContain(reply)
    expect(render({ support: { ok: false, reason: 'unsupported', message: 'no' } }, reply)).toBe('')
  })
})

/** The browser's recognizer, as far as the hook goes: the test plays its events. */
class Rec implements Recognizer {
  static made: Rec[] = []
  lang = ''
  continuous = false
  interimResults = false
  onaudiostart: (() => void) | null = null
  onresult: Recognizer['onresult'] = null
  onerror: ((e: { error?: string }) => void) | null = null
  onend: (() => void) | null = null
  constructor() {
    Rec.made.push(this)
  }
  start() {}
  stop() {}
  abort() {}
}

describe('useDictation', () => {
  const live: Dictating[] = []
  afterEach(() => {
    // Nothing a test left listening leaks into the next one.
    for (const d of live.splice(0)) d.cancel()
    Rec.made = []
    useToasts.setState({ toasts: [] })
    vi.unstubAllGlobals()
  })

  /** The hook as a field holds it after its first render, in a browser whose `window` is `host`. */
  function hook(opts?: UseDictationOptions, host: object = { SpeechRecognition: Rec }): Dictating {
    // Toasts expire on a `window` timer.
    vi.stubGlobal('window', { setTimeout: () => 0, ...host })
    vi.stubGlobal('navigator', { language: 'de-DE' })
    const held: { d?: Dictating } = {}
    const Field = () => {
      held.d = useDictation({ current: null }, () => {}, opts)
      return null
    }
    renderToStaticMarkup(createElement(Field))
    live.push(held.d!)
    return held.d!
  }
  const toasts = () => useToasts.getState().toasts.map((t) => `${t.level}: ${t.message}`)

  it('listens in the browser’s language and toasts what goes wrong, as the prompt’s microphone always did', () => {
    const d = hook()
    d.toggle()
    expect(Rec.made).toHaveLength(1)
    expect(Rec.made[0]).toMatchObject({ lang: 'de-DE', continuous: true, interimResults: true })
    Rec.made[0].onerror?.({ error: 'no-speech' })
    expect(toasts()).toEqual(['info: No speech heard. Click the microphone and try again.'])
  })

  it('says in a toast why it cannot work on plain http', () => {
    hook(undefined, { SpeechRecognition: Rec, isSecureContext: false }).toggle()
    expect(Rec.made).toEqual([])
    expect(toasts()).toEqual([`info: ${INSECURE_MESSAGE}`])
  })

  it('listens in the language a field asks for, and in the browser’s when it names none', () => {
    hook({ lang: 'uk-UA' }).toggle()
    hook({ lang: '' }).toggle()
    hook({}).toggle()
    expect(Rec.made.map((r) => r.lang)).toEqual(['uk-UA', 'de-DE', 'de-DE'])
  })

  it('hands what goes wrong to a field that asks for it, and toasts nothing', () => {
    const told: Problem[] = []
    const d = hook({ onProblem: (p) => told.push(p) })
    d.toggle()
    Rec.made[0].onerror?.({ error: 'not-allowed' })
    expect(told).toEqual([{ level: 'error', message: 'The microphone is blocked for this site. Allow it in the browser’s site settings, then try again.' }])
    // A stop the field asked for is not a problem, there as anywhere.
    Rec.made[0].onerror?.({ error: 'aborted' })
    expect(told).toHaveLength(1)
    // Plain http: the reason goes the same way.
    hook({ onProblem: (p) => told.push(p) }, { SpeechRecognition: Rec, isSecureContext: false }).toggle()
    expect(told[1]).toEqual({ level: 'info', message: INSECURE_MESSAGE })
    expect(toasts()).toEqual([])
  })
})
