// The microphone button, rendered on the server: nothing where the browser cannot
// recognize speech, a reason (not a dead button) where it only needs https, and its
// listening look.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { DictateButton, type Dictating } from './Dictate'
import { INSECURE_MESSAGE, type Recognizer } from './lib/dictation'

const noop = () => {}
const ctor = class {} as unknown as new () => Recognizer

function render(over: Partial<Dictating>) {
  const d: Dictating = { support: { ok: true, ctor }, state: 'idle', toggle: noop, cancel: noop, ...over }
  return renderToStaticMarkup(createElement(DictateButton, { d }))
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
})
