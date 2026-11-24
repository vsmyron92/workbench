// The bubble, rendered on the server, in each of its forms: who speaks, a line, the
// transcript, a question with its choices, the reply box, the first voice choice, the notes
// and the form for a session that needs a decision. And that whatever an agent wrote is
// shown as text.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { Dictating } from '@/features/agents/Dictate'
import type { Recognizer } from '@/features/agents/lib/dictation'
import { Bubble, MAX_REPLY_CHARS, WORDS, answerGoesTo, calledAs, calledHere, clickToHear, isTextField, repeatsActivation, replyTo, sendsOn } from './Bubble'
import type { BubbleNote, BubbleProps } from './Bubble'
import type { Ask, Line } from './lib/types'

const noop = () => {}
const ctor = class {} as unknown as new () => Recognizer
const dictating = (over: Partial<Dictating> = {}): Dictating => ({ support: { ok: true, ctor }, state: 'idle', toggle: noop, cancel: noop, ...over })

const bubble = (over: Partial<BubbleProps> = {}) => renderToStaticMarkup(createElement(Bubble, { provider: 'claude', title: 'Fix the flaky upload test', name: 'Pip', ...over }))
const line = (id: string, text: string, over: Partial<Line> = {}): Line => ({ id, at: 1, from: 'agent', text, ...over })
const ask = (over: Partial<Ask> = {}): Ask => ({ id: 'Q1', lineId: 'L9', text: 'Shall I push the branch now?', choices: ['Yes, push it', 'Not yet'], freeText: true, askedAt: 1, waiting: true, ...over })

const HOSTILE = '<img src=x onerror=alert(1)>'
const MARKDOWN = '**bold** [link](javascript:alert(1)) `code` # heading <script>alert(2)</script>'
/** What HTML an agent's text must never become. */
const expectNoMarkupFromText = (html: string) => {
  for (const tag of ['<img', '<script', '<a ', '<strong', '<em>', '<b>', '<code', '<h1', '<iframe', '<svg onload']) expect(html, tag).not.toContain(tag)
  expect(html).not.toMatch(/ on[a-z]+="/)
  expect(html).not.toContain('href=')
}

describe('the wording', () => {
  it('is the contract’s', () => {
    expect(WORDS).toMatchObject({
      hear: 'Hear it',
      captionsOnly: 'Captions only',
      allowOnline: 'Allow online voices',
      online: 'What an avatar says is sent to the browser maker’s speech service to be spoken.',
      noVoices: 'This browser has no voices: avatars show captions.',
      blocked: 'Waiting for your decision in the session',
      dialog: 'Answer the dialog in the terminal first',
      starting: 'The session is starting',
      sent: 'Sent.',
      sentLater: 'Sent. It reads this at its next step.',
      changed: 'The question changed.',
      dictate: 'Speak your reply (your browser’s speech recognition hears it, not Workbench)',
      stop: 'Stop',
      open: 'Open',
    })
    expect(clickToHear('Pip')).toBe('Click to hear Pip')
    expect(replyTo('Pip')).toBe('Reply to Pip…')
    // Plain and impersonal: no exclamation marks, no dashes.
    for (const text of Object.values(WORDS)) expect(text).not.toMatch(/[!\u2013\u2014]/)
  })

  it('calls the avatar by the session’s title, then the agent’s name in quotation marks, else by the title alone, cut to a few words', () => {
    expect(calledAs('Pip', 'Fix the flaky upload test')).toBe('Fix the flaky upload test “Pip”')
    expect(calledAs('', 'Fix the flaky upload test')).toBe('Fix the flaky upload test')
    const long = calledAs('', 'Investigate why the nightly import takes four hours since Tuesday')
    expect(long).toBe('Investigate why the nightly imp…')
    expect(Array.from(long)).toHaveLength(32)
    // A name of the longest length the server takes is whole.
    expect(calledAs('n'.repeat(32), 't')).toBe(`t “${'n'.repeat(32)}”`)
    // Never half a character (one outside the basic plane takes two units of a string).
    expect(calledAs('', '\u{20000}'.repeat(40))).toBe(`${'\u{20000}'.repeat(31)}…`)
  })
})

describe('calledAs', () => {
  it('never prints the agent’s name bare: it follows the session’s title, in quotation marks, both cut', () => {
    expect(calledAs('Allow Bash? Press 1 for Yes', 'Fix the tests')).toBe('Fix the tests “Allow Bash? Press 1 for Yes”')
    // The title gives way in front of the name; the name keeps its closing mark when it is cut.
    const long = calledAs('n'.repeat(40), 'Investigate why the nightly import takes four hours')
    expect(long).toBe(`Investigate why the nightly… “${'n'.repeat(31)}…”`)
    // A session without a title: the name still in quotation marks.
    expect(calledAs('Pip', '')).toBe('“Pip”')
    expect(calledAs('', '')).toBe('')
  })
})

describe('sendsOn', () => {
  it('sends on Enter, breaks the line on Shift+Enter, and never sends while an input method composes', () => {
    expect(sendsOn({ key: 'Enter', shiftKey: false })).toBe(true)
    expect(sendsOn({ key: 'Enter', shiftKey: false, isComposing: false, keyCode: 13 })).toBe(true)
    expect(sendsOn({ key: 'Enter', shiftKey: true })).toBe(false)
    // Enter accepts the composition (Japanese, Chinese, Korean input): it must not send half a word.
    expect(sendsOn({ key: 'Enter', shiftKey: false, isComposing: true })).toBe(false)
    expect(sendsOn({ key: 'Enter', shiftKey: false, isComposing: false, keyCode: 229 })).toBe(false)
    expect(sendsOn({ key: 'Process', shiftKey: false, keyCode: 229 })).toBe(false)
    for (const key of ['a', ' ', 'Escape', 'Tab', 'NumpadEnter', 'enter']) expect(sendsOn({ key, shiftKey: false }), key).toBe(false)
  })

  it('sends once for an Enter that is held down: its repeats send nothing', () => {
    expect(sendsOn({ key: 'Enter', shiftKey: false, repeat: true })).toBe(false)
    expect(sendsOn({ key: 'Enter', shiftKey: false, repeat: false })).toBe(true)
  })

  it('lets an Enter held down on a button of the bubble click it once: its repeats are cancelled', () => {
    const button = { tagName: 'BUTTON' }
    expect(repeatsActivation({ key: 'Enter', repeat: true, target: button })).toBe(true)
    expect(repeatsActivation({ key: 'Enter', repeat: false, target: button })).toBe(false)
    expect(repeatsActivation({ key: 'Enter', target: button })).toBe(false)
    // The reply box has its own rule (sendsOn): a held Shift+Enter breaks lines there.
    expect(repeatsActivation({ key: 'Enter', repeat: true, target: { tagName: 'TEXTAREA' } })).toBe(false)
    // A held Space clicks once, when it is let go; no other key clicks.
    for (const key of [' ', 'a', 'Escape', 'Tab', 'enter']) expect(repeatsActivation({ key, repeat: true, target: button }), key).toBe(false)
  })

  it('knows a text field from every other control', () => {
    expect(isTextField({ tagName: 'TEXTAREA' })).toBe(true)
    expect(isTextField({ tagName: 'INPUT' })).toBe(true)
    for (const other of [{ tagName: 'BUTTON' }, { tagName: 'DIV' }, { tagName: 'textarea' }, {}, null, undefined, 'TEXTAREA']) expect(isTextField(other)).toBe(false)
  })
})

describe('who speaks', () => {
  it('names the session first, in the app’s own words, and the agent’s name last, in quotation marks', () => {
    const html = bubble({ project: 'warehouse-etl' })
    const order = ['wb-av-picon claude', '<span class="wb-av-title">Fix the flaky upload test</span>', '<span class="wb-av-project">warehouse-etl</span>', '<span class="wb-av-name" dir="auto">“Pip”</span>']
    const at = order.map((piece) => html.indexOf(piece))
    expect(at.every((i) => i >= 0)).toBe(true)
    expect([...at].sort((a, b) => a - b)).toEqual(at)
    expect(html).toContain('role="group" aria-label="Avatar of Fix the flaky upload test"')
  })

  it('leaves out the project of the current project, and the quotation marks of an agent without a name', () => {
    const html = bubble({ name: '' })
    expect(html).not.toContain('wb-av-project')
    expect(html).not.toContain('wb-av-name')
    expect(html).not.toContain('“')
    expect(bubble({ project: null })).not.toContain('wb-av-project')
    expect(bubble({ project: '' })).not.toContain('wb-av-project')
  })

  it('shows the icon of the CLI that runs the session, from the app’s own table', () => {
    for (const kind of ['claude', 'codex', 'kimi', 'gemini', 'aider', 'custom'] as const) expect(bubble({ provider: kind })).toContain(`wb-av-picon ${kind}"`)
    // A kind the table does not know picks nothing from data: the plain terminal icon.
    const odd = bubble({ provider: 'x" onload="alert(1)' as 'custom' })
    expect(odd).toContain('wb-av-picon custom"')
    expect(odd).not.toContain('onload')
    expect(bubble({ provider: 'constructor' as 'custom' })).toContain('wb-av-picon custom"')
  })

  it('shows a title, a project and a name as text', () => {
    const html = bubble({ title: HOSTILE, project: MARKDOWN, name: HOSTILE })
    expectNoMarkupFromText(html)
    expect(html).toContain('<span class="wb-av-title">&lt;img src=x onerror=alert(1)&gt;</span>')
    expect(html).toContain('“&lt;img src=x onerror=alert(1)&gt;”')
    expect(html).toContain('**bold** [link](javascript:alert(1))')
  })
})

describe('a line', () => {
  it('is a caption that a screen reader announces while this tab shows it silently', () => {
    const html = bubble({ caption: { lineId: 'L1', text: 'The build is green again.' } })
    expect(html).toContain('<div class="wb-av-caption" aria-live="polite" dir="auto">The build is green again.</div>')
    for (const absent of ['wb-av-log', 'wb-av-choices', 'wb-av-reply', 'wb-av-voice', 'wb-av-stop', 'wb-av-blocked', 'wb-av-notice']) expect(html, absent).not.toContain(absent)
  })

  it('is not announced a second time while this tab says it aloud, and can be stopped', () => {
    const html = bubble({ caption: { text: 'Tests pass.' }, aloud: true, onStop: noop })
    expect(html).toContain('aria-live="off"')
    expect(html).toMatch(/<button type="button" class="wb-btn small wb-av-stop">.*Stop<\/button>/)
    // The icon of the menu's "Stop speaking", never a bare square that reads as an empty check box.
    const stop = /<button type="button" class="wb-btn small wb-av-stop">(<svg[^>]*>)/.exec(html)?.[1] ?? ''
    expect(stop).toContain('lucide-circle-stop')
    expect(stop).not.toMatch(/lucide-square[ "]/)
    // No Stop for a caption, nor without a way to stop.
    expect(bubble({ caption: { text: 'Tests pass.' }, onStop: noop })).not.toContain('wb-av-stop')
    expect(bubble({ caption: { text: 'Tests pass.' }, aloud: true })).not.toContain('wb-av-stop')
  })

  it('sets off the part that is being said from what was said and what is to come', () => {
    const chunks = ['The first sentence.', 'The second one.', 'And a third.']
    const html = bubble({ caption: { text: chunks.join(' '), chunks, index: 1 } })
    expect(html).toContain('<span class="wb-av-said">The first sentence.</span><span class="wb-av-now"> The second one.</span><span class="wb-av-next"> And a third.</span>')
    // Without chunks, or with an index that points nowhere, it is the plain line.
    for (const caption of [{ text: 'Plain.' }, { text: 'Plain.', chunks, index: -1 }, { text: 'Plain.', chunks: [], index: 0 }, { text: 'Plain.', chunks, index: 0.5 }]) {
      expect(bubble({ caption })).toContain('dir="auto">Plain.</div>')
    }
  })

  it('keeps the parts of a line once it was presented: nothing is put in their place for a screen reader to announce again', () => {
    const chunks = ['The first sentence.', 'The second one.', 'And a third.']
    const parts = '<span>The first sentence.</span><span> The second one.</span><span> And a third.</span>'
    for (const index of [chunks.length, 7]) {
      const html = bubble({ caption: { lineId: 'L1', text: 'The first sentence. The second one. And a third.', chunks, index } })
      expect(html, String(index)).toContain(`<div class="wb-av-caption" aria-live="polite" dir="auto">${parts}</div>`)
    }
  })

  it('carries the line’s language only when it is a language tag', () => {
    expect(bubble({ caption: { text: 'Guten Tag.', lang: 'de-DE' } })).toContain('aria-live="polite" lang="de-DE" dir="auto"')
    for (const lang of ['x" onmouseover="alert(1)', 'de DE', '', 'english-language-please']) {
      const html = bubble({ caption: { text: 'Guten Tag.', lang } })
      expect(html, lang).not.toContain('lang=')
      expect(html, lang).not.toContain('onmouseover')
    }
  })

  it('shows what an agent wrote as text: no HTML, no Markdown', () => {
    for (const text of [HOSTILE, MARKDOWN]) {
      const html = bubble({ caption: { text, chunks: [text], index: 0 } })
      expectNoMarkupFromText(html)
    }
    expect(bubble({ caption: { text: HOSTILE } })).toContain('&lt;img src=x onerror=alert(1)&gt;')
    const md = bubble({ caption: { text: MARKDOWN } })
    expect(md).toContain('**bold** [link](javascript:alert(1)) `code` # heading &lt;script&gt;alert(2)&lt;/script&gt;')
  })
})

describe('the transcript', () => {
  const lines = [line('L1', 'Reading the failing test.', { outcome: 'spoken' }), line('L2', 'Stop after this step.', { from: 'user', via: 'typed' }), line('L3', 'Understood.'), line('L4', 'The fixture is changed.')]

  it('is a log of the last lines, the user’s own marked as such', () => {
    const html = bubble({ log: lines })
    expect(html).toContain('<div class="wb-av-log" role="log" aria-live="polite">')
    expect(html.match(/<div class="wb-av-line[ "]/g)).toHaveLength(4)
    expect(html).toContain('<div class="wb-av-line user"><span class="wb-av-who">You</span><span class="wb-av-line-text" dir="auto">Stop after this step.</span></div>')
    expect(html).toContain('<div class="wb-av-line"><span class="wb-av-line-text" dir="auto">Reading the failing test.</span></div>')
    expect(html).not.toContain('wb-av-caption')
  })

  it('starts at the first line the user has not seen', () => {
    const html = bubble({ log: lines, unseenFrom: 'L3' })
    expect(html).toContain('<div class="wb-av-line"><span class="wb-av-line-text" dir="auto">Reading the failing test.</span>')
    expect(html).toContain('<div class="wb-av-line unseen"><span class="wb-av-line-text" dir="auto">Understood.</span>')
    expect(html).toContain('<div class="wb-av-line unseen"><span class="wb-av-line-text" dir="auto">The fixture is changed.</span>')
    // A reply of the user is never "unseen" by the user.
    expect(bubble({ log: lines, unseenFrom: 'L1' })).toContain('<div class="wb-av-line user">')
    expect(bubble({ log: lines, unseenFrom: 'nowhere' })).not.toContain('unseen')
  })

  it('marks the line that is being presented, with the part that is being said', () => {
    const html = bubble({ log: lines, caption: { lineId: 'L4', text: 'The fixture is changed.', chunks: ['The fixture', 'is changed.'], index: 1 } })
    expect(html).toContain('<div class="wb-av-line now"><span class="wb-av-line-text" dir="auto"><span class="wb-av-said">The fixture</span><span class="wb-av-now"> is changed.</span></span></div>')
    expect(html.match(/wb-av-line now/g)).toHaveLength(1)
  })

  it('is not announced while this tab says its line aloud, and keeps the parts of that line once it was said', () => {
    const said = { lineId: 'L4', text: 'The fixture is changed.', chunks: ['The fixture', 'is changed.'] }
    expect(bubble({ log: lines, caption: { ...said, index: 0 }, aloud: true })).toContain('<div class="wb-av-log" role="log" aria-live="off">')
    expect(bubble({ log: lines, caption: { ...said, index: 0 }, aloud: false })).toContain('<div class="wb-av-log" role="log" aria-live="polite">')
    // Over: no longer marked as the one being presented, its parts are where they were.
    const over = bubble({ log: lines, caption: { ...said, index: 2 } })
    expect(over).toContain('<div class="wb-av-line"><span class="wb-av-line-text" dir="auto"><span>The fixture</span><span> is changed.</span></span></div>')
    expect(over).not.toContain('wb-av-line now')
  })

  it('shows every line as text, and an empty log as nothing', () => {
    const html = bubble({ log: [line('L1', HOSTILE), line('L2', MARKDOWN, { from: 'user' }), line('L3', 'Hallo', { lang: 'de-DE' }), line('L4', 'x', { lang: '"><script>' })] })
    expectNoMarkupFromText(html)
    expect(html).toContain('lang="de-DE"')
    expect(html.match(/lang=/g)).toHaveLength(1)
    expect(bubble({ log: [] })).toContain('<div class="wb-av-log" role="log" aria-live="polite"></div>')
  })
})

describe('a question', () => {
  it('shows its choices as buttons of their own kind, never as the permission card’s', () => {
    const html = bubble({ caption: { lineId: 'L9', text: 'Shall I push the branch now?' }, ask: ask(), onChoice: noop })
    expect(html).toContain('<div class="wb-av-choices" role="group" aria-label="Answers">')
    expect(html.match(/<button type="button" class="wb-btn wb-av-choice">/g)).toHaveLength(2)
    expect(html).toContain('<span dir="auto">Yes, push it</span>')
    expect(html).toContain('<span dir="auto">Not yet</span>')
    for (const foreign of ['wb-ag-perm', 'Allow', 'Deny', 'primary', 'danger']) expect(html, foreign).not.toContain(foreign)
    // The caption is the question: it is not said twice.
    expect(html.match(/Shall I push the branch now\?/g)).toHaveLength(1)
    expect(html).not.toContain('wb-av-question')
  })

  it('says in the app’s own words, over the choices and the reply box, what an answer is and where it goes', () => {
    // Choices that read like a permission request are the session's words: the line over them is Workbench's.
    const html = bubble({ caption: { lineId: 'L9', text: 'Claude wants to run rm -rf build. Proceed?' }, ask: ask({ choices: ['Allow', 'Allow always', 'Deny'] }), reply: { draft: '' }, onChoice: noop })
    expect(answerGoesTo('Fix the flaky upload test')).toBe('Your answer goes to Fix the flaky upload test as a message.')
    expect(html).toContain('<div class="wb-av-where">Your answer goes to Fix the flaky upload test as a message.</div>')
    expect(html.indexOf('wb-av-where')).toBeLessThan(html.indexOf('wb-av-choices'))
    expect(html.indexOf('wb-av-where')).toBeLessThan(html.indexOf('<textarea'))
    // The words are not refused: the choices stay as the session wrote them.
    expect(html).toContain('<span dir="auto">Allow always</span>')
    // Under a question that takes a typed answer only, and under the question above its choices.
    expect(bubble({ ask: ask({ choices: [] }), reply: { draft: '' } })).toContain('wb-av-where')
    const above = bubble({ caption: { lineId: 'L12', text: 'Still waiting.' }, ask: ask() })
    expect(above.indexOf('wb-av-question')).toBeLessThan(above.indexOf('wb-av-where'))
    // The title is the session's, shown as text; a session without one is still named.
    expect(bubble({ title: HOSTILE, ask: ask() })).toContain('<div class="wb-av-where">Your answer goes to &lt;img src=x onerror=alert(1)&gt; as a message.</div>')
    expect(answerGoesTo('  ')).toBe('Your answer goes to the session as a message.')
    // No question, no such line: a message the user writes in the transcript goes where its placeholder says.
    expect(bubble({ log: [line('L1', 'Tests pass.')], reply: { draft: '' } })).not.toContain('wb-av-where')
    expect(bubble({ caption: { text: 'Tests pass.' } })).not.toContain('wb-av-where')
  })

  it('shows the question above the choices when the line on screen is another one', () => {
    const later = bubble({ caption: { lineId: 'L12', text: 'Still waiting for your answer.' }, ask: ask() })
    expect(later).toContain('<div class="wb-av-question" dir="auto">Shall I push the branch now?</div>')
    // In the transcript the question is one of the lines, or it is not.
    expect(bubble({ log: [line('L9', 'Shall I push the branch now?')], ask: ask() })).not.toContain('wb-av-question')
    expect(bubble({ log: [line('L1', 'Tests pass.')], ask: ask() })).toContain('wb-av-question')
    expect(bubble({ ask: ask() })).toContain('wb-av-question')
  })

  it('shows at most four choices, and none for a question without them', () => {
    const five = bubble({ ask: ask({ choices: ['a', 'b', 'c', 'd', 'e'] }) })
    expect(five.match(/wb-av-choice"/g)).toHaveLength(4)
    expect(five).not.toContain('<span dir="auto">e</span>')
    expect(bubble({ ask: ask({ choices: [] }) })).not.toContain('wb-av-choices')
    expect(bubble({ ask: ask({ choices: [7, null, 'ok'] as unknown as string[] }) }).match(/wb-av-choice"/g)).toHaveLength(1)
  })

  it('offers to dismiss it only when there is a way to', () => {
    expect(bubble({ ask: ask(), onDismiss: noop })).toMatch(/<button type="button" class="wb-btn ghost small wb-av-dismiss">Dismiss<\/button>/)
    expect(bubble({ ask: ask() })).not.toContain('Dismiss')
  })

  it('shows choices and the question as text', () => {
    const html = bubble({ ask: ask({ text: MARKDOWN, choices: [HOSTILE, '[x](http://evil.example)'] }) })
    expectNoMarkupFromText(html)
    expect(html).toContain('<span dir="auto">&lt;img src=x onerror=alert(1)&gt;</span>')
    expect(html).toContain('<span dir="auto">[x](http://evil.example)</span>')
  })
})

describe('the reply box', () => {
  it('is a text field with the avatar’s name, a microphone that says where the audio goes, and Send', () => {
    const html = bubble({ reply: { draft: '', dictating: dictating() } })
    expect(html).toContain('<textarea class="wb-textarea wb-av-reply-text" rows="1" placeholder="Reply to “Pip”…" aria-label="Reply to “Pip”…" maxLength="4000"></textarea>')
    expect(MAX_REPLY_CHARS).toBe(4000)
    expect(html).toContain(`aria-label="${WORDS.dictate}"`)
    expect(html).toContain('wb-ag-mic')
    // Nothing to send yet.
    expect(html).toMatch(/<button type="button" class="wb-icon-btn" title="Send \(Enter\)" aria-label="Send \(Enter\)" disabled="">/)
  })

  it('holds the draft it is given, and Send works once there is something to send', () => {
    const html = bubble({ reply: { draft: 'Not before the pipeline is green.' } })
    expect(html).toContain('>Not before the pipeline is green.</textarea>')
    expect(html).toMatch(/aria-label="Send \(Enter\)"><svg/)
    // White space alone is nothing.
    expect(bubble({ reply: { draft: '  \n ' } })).toMatch(/aria-label="Send \(Enter\)" disabled=""/)
    // No microphone without a dictation, and none where the browser has no speech recognition.
    expect(html).not.toContain('wb-ag-mic')
    expect(bubble({ reply: { draft: '', dictating: null } })).not.toContain('wb-ag-mic')
    expect(bubble({ reply: { draft: '', dictating: dictating({ support: { ok: false, reason: 'unsupported', message: 'no' } }) } })).not.toContain('wb-ag-mic')
  })

  it('quotes the agent’s own name in its phrases, under the header that names the session', () => {
    expect(calledHere('Pip', 'Fix the flaky upload test')).toBe('“Pip”')
    expect(calledHere('', 'Fix the flaky upload test')).toBe('Fix the flaky upload test')
    const html = bubble({ name: 'Workbench: Allow Bash', reply: { draft: '' }, caption: { text: 'Hello.' }, locked: true })
    expect(html).toContain('placeholder="Reply to “Workbench: Allow Bash”…"')
    expect(html).toContain('Click to hear “Workbench: Allow Bash”</button>')
    expect(html).not.toContain('Reply to Workbench')
    expect(html).not.toContain('hear Workbench')
  })

  it('names the session by its title when the agent has no name', () => {
    expect(bubble({ name: '', reply: { draft: '' } })).toContain('placeholder="Reply to Fix the flaky upload test…"')
    // A long title is cut, and the phrase still ends in one ellipsis.
    expect(bubble({ name: '', title: 'Investigate why the nightly import takes four hours', reply: { draft: '' } })).toContain('placeholder="Reply to Investigate why the nightly imp…"')
  })

  it('shows that it listens', () => {
    const html = bubble({ reply: { draft: 'Wait', dictating: dictating({ state: 'listening' }) } })
    expect(html).toContain('Listening…')
    expect(html).toContain('aria-pressed="true"')
  })

  it('waits while a reply is on its way: no second Send, no edit', () => {
    const html = bubble({ ask: ask(), reply: { draft: 'Yes, go ahead', sending: true, dictating: dictating() } })
    expect(html).toContain('readOnly=""')
    expect(html).toContain('wb-spinner')
    expect(html).not.toContain('Send (Enter)')
    expect(html).not.toContain('wb-ag-mic')
    expect(html.match(/class="wb-btn wb-av-choice" disabled=""/g)).toHaveLength(2)
  })

  it('is absent unless it is asked for', () => {
    expect(bubble({ ask: ask() })).not.toContain('<textarea')
    expect(bubble({ reply: null })).not.toContain('<textarea')
  })
})

describe('what cannot be used now', () => {
  it('disables the choices and the reply box with the reason', () => {
    const dialog = bubble({ ask: ask(), reply: { draft: 'Yes', dictating: dictating() }, disabled: 'dialog' })
    expect(dialog).toContain(`<div class="wb-av-reason" role="status">${WORDS.dialog}</div>`)
    expect(dialog.match(/class="wb-btn wb-av-choice" disabled=""/g)).toHaveLength(2)
    expect(dialog).toMatch(/<textarea [^>]*disabled=""/)
    expect(dialog).toMatch(/aria-label="Send \(Enter\)" disabled=""/)
    // No microphone into a field that takes nothing.
    expect(dialog).not.toContain('wb-ag-mic')
    const starting = bubble({ reply: { draft: '' }, disabled: 'starting' })
    expect(starting).toContain(`role="status">${WORDS.starting}</div>`)
    expect(starting).not.toContain(WORDS.dialog)
  })

  it('says nothing of it where there is nothing to disable', () => {
    expect(bubble({ caption: { text: 'Tests pass.' }, disabled: 'dialog' })).not.toContain('wb-av-reason')
    expect(bubble({ ask: ask(), reply: { draft: '' } })).not.toContain('wb-av-reason')
  })
})

describe('the first voice choice', () => {
  const caption = { lineId: 'L1', text: 'I found the cause.' }

  it('offers Hear it and Captions only', () => {
    const html = bubble({ caption, voiceChoice: 'offer' })
    expect(html).toMatch(/<div class="wb-av-voice"><button type="button" class="wb-btn small"><svg[^>]*>.*?<\/svg>Hear it<\/button><button type="button" class="wb-btn ghost small">Captions only<\/button><\/div>/)
    expect(html).not.toContain(WORDS.online)
    expect(html).not.toContain(WORDS.allowOnline)
    expect(html).not.toContain(WORDS.noVoices)
  })

  it('says what online voices mean before it allows them', () => {
    const html = bubble({ caption, voiceChoice: 'online' })
    expect(html).toContain(`<div class="wb-av-voice online"><p>${WORDS.online}</p>`)
    expect(html).toContain('speech service')
    expect(html).toMatch(/Allow online voices<\/button><button type="button" class="wb-btn ghost small">Captions only<\/button>/)
    // The sentence comes first, and there is no way to hear it without allowing.
    expect(html.indexOf(WORDS.online)).toBeLessThan(html.indexOf(WORDS.allowOnline))
    expect(html).not.toContain('Hear it')
  })

  it('says that the browser has no voices', () => {
    const html = bubble({ caption, voiceChoice: 'none' })
    expect(html).toContain(`<div class="wb-av-voice none" role="status"><p>${WORDS.noVoices}</p></div>`)
    for (const absent of ['Hear it', 'Captions only', 'Allow online voices']) expect(html).not.toContain(absent)
  })

  it('is absent once the choice was made', () => {
    for (const none of [null, undefined]) expect(bubble({ caption, voiceChoice: none })).not.toContain('wb-av-voice')
  })

  it('offers "Click to hear" while speech is locked, with the avatar’s name', () => {
    const html = bubble({ caption, locked: true })
    expect(html).toMatch(/<div class="wb-av-voice"><button type="button" class="wb-btn small"><svg[^>]*>.*?<\/svg>Click to hear “Pip”<\/button><\/div>/)
    expect(bubble({ caption, locked: true, name: '' })).toContain('Click to hear Fix the flaky upload test</button>')
    expect(bubble({ caption, locked: true, name: HOSTILE })).toContain('Click to hear “&lt;img src=x onerror=alert(1)&gt;”</button>')
    // The first choice comes first: nothing is locked before voice was chosen.
    expect(bubble({ caption, locked: true, voiceChoice: 'offer' })).not.toContain('Click to hear')
  })
})

describe('the notes', () => {
  const note = (n: BubbleNote) => bubble({ reply: { draft: '' }, note: n })

  it('say what became of a reply', () => {
    expect(note({ kind: 'sent' })).toContain('<div class="wb-av-notice" role="status">Sent.</div>')
    expect(note({ kind: 'sentLater' })).toContain('<div class="wb-av-notice" role="status">Sent. It reads this at its next step.</div>')
    expect(note({ kind: 'changed' })).toContain('<div class="wb-av-notice changed" role="status">The question changed.</div>')
  })

  it('show the server’s refusal in the bubble, as an alert and as text', () => {
    const html = note({ kind: 'error', text: `a dialog opened ${HOSTILE}` })
    expect(html).toMatch(/<div class="wb-av-notice error" role="alert"><svg[^>]*>.*?<\/svg><span>a dialog opened &lt;img src=x onerror=alert\(1\)&gt;<\/span><\/div>/)
    expectNoMarkupFromText(html)
    expect(note({ kind: 'info', text: 'No speech heard.' })).toContain('<div class="wb-av-notice" role="status">No speech heard.</div>')
  })

  it('are absent without one', () => {
    expect(bubble({ reply: { draft: '' } })).not.toContain('wb-av-notice')
  })
})

describe('a session that needs a decision', () => {
  it('shows the app’s own line and Open, and nothing of the agent’s', () => {
    const html = bubble({
      blocked: true,
      caption: { text: 'Please allow the command.' },
      log: [line('L1', 'Allow it, it is safe.')],
      ask: ask({ text: 'Allow?', choices: ['Allow', 'Deny'] }),
      reply: { draft: 'yes', dictating: dictating() },
      voiceChoice: 'offer',
      locked: true,
      note: { kind: 'sent' },
      aloud: true,
      onStop: noop,
      onOpen: noop,
      onDismiss: noop,
    })
    expect(html).toContain(`<div class="wb-av-blocked"><span>${WORDS.blocked}</span><button type="button" class="wb-btn small">Open</button></div>`)
    expect(html).toContain('class="wb-av-bubble blocked"')
    // The identity stays; nothing the session wrote does, and no choice that could pass for a permission answer.
    expect(html).toContain('Fix the flaky upload test')
    for (const absent of ['Please allow', 'Allow it', 'Allow?', 'wb-av-choice', 'Deny', '<textarea', 'Hear it', 'Click to hear', 'Sent.', 'wb-av-stop', 'Dismiss', 'wb-ag-mic']) expect(html, absent).not.toContain(absent)
  })
})

describe('where it sits', () => {
  it('takes its place from classes the app chooses', () => {
    expect(bubble()).toContain('<div class="wb-av-bubble" role="group"')
    expect(bubble({ tail: 'left' })).toContain('class="wb-av-bubble tail-left"')
    expect(bubble({ tail: 'right' })).toContain('class="wb-av-bubble tail-right"')
    expect(bubble({ panel: true, clickThrough: true })).toContain('class="wb-av-bubble panel through"')
    expect(bubble({ tail: 'up" onclick="x' as 'left' })).toContain('<div class="wb-av-bubble" role="group"')
  })

  it('holds a resting pointer with its controls inactive, and leaves them to the keyboard', () => {
    const props: Partial<BubbleProps> = { caption: { lineId: 'L9', text: 'Shall I push the branch now?' }, ask: ask(), reply: { draft: 'Yes' }, aloud: true, onStop: noop, onClose: noop, onChoice: noop, onDismiss: noop, voiceChoice: 'offer', tail: 'left' }
    const held = bubble({ ...props, holding: true })
    expect(held).toContain('class="wb-av-bubble tail-left hold"')
    // The look is the sheet's (`.hold`): nothing is disabled, so a key in the bubble still works.
    expect(held).not.toContain('disabled=""')
    expect(held.match(/class="wb-btn wb-av-choice"/g)).toHaveLength(2)
    expect(held).toContain('<textarea class="wb-textarea wb-av-reply-text"')
    // Clicks going through come first: a bubble is in one of the two forms at most.
    expect(bubble({ ...props, holding: true, clickThrough: true })).toContain('class="wb-av-bubble tail-left through"')
    expect(bubble({ ...props, holding: false })).toContain('class="wb-av-bubble tail-left"')
    // The blocked form holds the same way.
    expect(bubble({ blocked: true, holding: true, onOpen: noop })).toContain('class="wb-av-bubble hold blocked"')
  })

  it('holds the stage it is given, and a way to close it', () => {
    const html = bubble({ panel: true, figure: createElement('span', { className: 'wb-av-stage' }), caption: { text: 'Tests pass.' }, onClose: noop })
    expect(html).toContain('<div class="wb-av-main"><div class="wb-av-bubble-figure"><span class="wb-av-stage"></span></div><div class="wb-av-body">')
    expect(html).toMatch(/<button type="button" class="wb-icon-btn small" title="Close" aria-label="Close">/)
    expect(bubble()).not.toContain('aria-label="Close"')
    expect(bubble()).not.toContain('wb-av-bubble-figure')
  })
})
