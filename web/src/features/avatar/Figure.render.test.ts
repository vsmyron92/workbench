// The figure, rendered on the server: what is drawn for a design, for none and for one that
// is rejected; that the stage is the app's (a fixed, bounded size; the SVG hidden from
// assistive technology); that nothing of a hostile design reaches the markup; and what the
// app shows around the figure. Effects do not run here: the engine binding is looked at in
// a real browser.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, describe, expect, it, vi } from 'vitest'
import pipRaw from '../../../../server/src/avatar/examples/pip.look.json?raw'
import tockRaw from '../../../../server/src/avatar/examples/tock.look.json?raw'
import { DESIGNING, DESIGN_NOT_SHOWN, FACE_PX, FIGURE_PX, FIGURE_PX_PHONE, FIGURE_RING_PX, Figure, FigureButton, StillLook, VOICE_OFF, drawnLook, keepFocus, ringFor, sizeStyle, stateWords } from './Figure'
import type { FigureButtonProps, FigureProps } from './Figure'
import { defaultLook } from './lib/defaultLook'
import { compile } from './lib/engine'
import { richLook, wire } from './lib/fixtures'
import { faceViewBox } from './lib/look'
import { AVATAR_STATES } from './lib/types'
import type { AvatarState, Look, SessionTone } from './lib/types'

const pip = (): Look => JSON.parse(pipRaw) as Look
const tock = (): Look => JSON.parse(tockRaw) as Look

const figure = (over: Partial<FigureProps> = {}): FigureProps => ({ look: null, seed: 't1', revision: 1, idPrefix: 'f1-', size: FIGURE_PX.rest, engine: null, state: 'idle', reducedMotion: false, mouth: null, gaze: null, ...over })
const button = (over: Partial<FigureButtonProps> = {}): FigureButtonProps => ({ ...figure(), ring: 'idle', name: 'Pip', title: 'Fix the flaky upload test', stateLabel: 'Idle', ...over })
const draw = (over: Partial<FigureProps> = {}) => renderToStaticMarkup(createElement(Figure, figure(over)))
const drawButton = (over: Partial<FigureButtonProps> = {}) => renderToStaticMarkup(createElement(FigureButton, button(over)))

afterEach(() => {
  vi.restoreAllMocks()
})

describe('drawnLook', () => {
  it('draws the default look of the session while it has no design', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    for (const none of [null, undefined]) {
      const drawn = drawnLook(none, 't1')
      expect(drawn).toEqual({ look: defaultLook('t1'), own: false, rejected: false })
      expect(drawn.look).toBe(defaultLook('t1'))
    }
    expect(warn).not.toHaveBeenCalled()
  })

  it('draws the session’s design when the checker and the engine accept it: the same checked look for the same design object', () => {
    const raw = pip()
    const drawn = drawnLook(raw, 't1')
    expect(drawn.own).toBe(true)
    expect(drawn.rejected).toBe(false)
    // A rebuilt object with equal content, never the one that came from the server.
    expect(drawn.look).not.toBe(raw)
    expect(drawn.look).toEqual(raw)
    expect(compile(drawn.look)).not.toBeNull()
    // The record changes for other reasons and is drawn again and again: the look keeps its identity.
    expect(drawnLook(raw, 't1').look).toBe(drawn.look)
    expect(drawnLook(raw, 'another seed').look).toBe(drawn.look)
    // A new design object is a new look.
    expect(drawnLook(pip(), 't1').look).not.toBe(drawn.look)
  })

  it('draws the default look for a design the checker rejects, and says so once', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const hostile = { ...pip(), nodes: [{ tag: 'script', text: 'alert(1)' }] }
    for (let i = 0; i < 3; i++) expect(drawnLook(hostile, 't2')).toEqual({ look: defaultLook('t2'), own: false, rejected: true })
    expect(warn).toHaveBeenCalledTimes(1)
    expect(String(warn.mock.calls[0][0])).toContain('could not be shown here')
    // The warning carries nothing of the design.
    expect(warn.mock.calls[0]).toHaveLength(1)
  })

  it('draws the default look for a design the engine cannot animate', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    // The checker takes a mouth without `closed`; the engine finds that it cannot morph from the artwork's own path.
    const stiff = { ...pip(), mouth: [{ target: 'mouth', open: 'M 0 0 L 9 9' }] }
    expect(drawnLook(stiff, 't1')).toMatchObject({ own: false, rejected: true })
    expect(drawnLook(stiff, 't1').look).toBe(defaultLook('t1'))
    expect(warn).toHaveBeenCalledTimes(1)
  })

  it('draws the default look for what is no design at all', () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    for (const odd of ['<svg/>', 7, true, [], {}, { viewBox: 'x' }]) expect(drawnLook(odd, 't1'), JSON.stringify(odd)).toMatchObject({ look: defaultLook('t1'), own: false, rejected: true })
  })
})

describe('ringFor and stateWords', () => {
  it('shows the session’s own tone, except while the avatar speaks or listens', () => {
    const tones: SessionTone[] = ['working', 'attention', 'error', 'unread', 'idle', 'starting', 'exited']
    for (const tone of tones) {
      for (const state of AVATAR_STATES) expect(ringFor(state, tone)).toBe(state === 'speaking' || state === 'listening' ? state : tone)
    }
    expect(ringFor('speaking', 'working')).toBe('speaking')
    expect(ringFor('listening', 'idle')).toBe('listening')
    // An open question does not change the ring: the session is working, or idle.
    expect(ringFor('waiting', 'working')).toBe('working')
  })

  it('uses the tab’s own words, plus Speaking, Listening and Asks you', () => {
    const words = (state: AvatarState) => stateWords(state, 'Needs permission')
    expect(words('speaking')).toBe('Speaking')
    expect(words('listening')).toBe('Listening')
    expect(words('waiting')).toBe('Asks you')
    for (const state of ['idle', 'working', 'attention', 'error', 'done', 'sleeping'] as const) expect(words(state)).toBe('Needs permission')
  })
})

describe('sizes', () => {
  it('are the ones of the contract', () => {
    expect(FIGURE_PX).toEqual({ rest: 56, active: 132 })
    expect(FIGURE_PX_PHONE).toEqual({ rest: 48, active: 96 })
    expect(FACE_PX).toEqual({ bar: 28, phone: 40 })
    // No ring is drawn round a figure: the "+n" chip beside the figures is as large as a resting one.
    expect(FIGURE_RING_PX).toBe(0)
  })

  it('never make a stage larger than 132 px, whatever is asked', () => {
    const px = (size: number) => (sizeStyle(size) as Record<string, string>)['--wb-av-size']
    expect(px(56)).toBe('56px')
    expect(px(132)).toBe('132px')
    expect(px(133)).toBe('132px')
    expect(px(4000)).toBe('132px')
    expect(px(47.6)).toBe('48px')
    expect(px(0)).toBe('12px')
    expect(px(-5)).toBe('12px')
    expect(px(Number.NaN)).toBe('56px')
    expect(px(Infinity)).toBe('56px')
    expect(draw({ size: 9999 })).toContain('--wb-av-size:132px')
  })
})

describe('Figure', () => {
  it('draws the look on the app’s stage, the SVG hidden from assistive technology', () => {
    const html = draw({ look: pip(), idPrefix: ':r7:' })
    expect(html).toMatch(/^<span class="wb-av-stage" data-drawn="own" style="--wb-av-size:56px"><svg class="wb-av-art" viewBox="0 0 200 200" aria-hidden="true" focusable="false">/)
    expect(html.endsWith('</svg></span>')).toBe(true)
    // Ids and references under the cleaned prefix (React's useId gives ":r7:").
    expect(html).toContain('id="r7mouth"')
    expect(html).toContain('clip-path="url(#r7eyeClip)"')
    expect(html).not.toContain(':r7:')
    // One style only, the stage's own; no class but the two of the app; no handler.
    expect(html.match(/ style=/g)).toHaveLength(1)
    expect(html.match(/ class=/g)).toHaveLength(2)
    expect(html).not.toMatch(/ on[a-z]+=/i)
  })

  it('draws the default look for a session without a design, and for one whose design is rejected', () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const none = draw()
    expect(none).toContain('data-drawn="default"')
    expect(none).toContain('var(--')
    const hostile = {
      ...pip(),
      nodes: [
        ...pip().nodes,
        { tag: 'g', attrs: { style: 'position:fixed;inset:0', className: 'wb-modal', onClick: 'alert(1)', href: 'javascript:alert(1)' }, children: [{ tag: 'foreignObject' }] },
        { tag: 'path', attrs: { d: 'M 0 0', dangerouslySetInnerHTML: { __html: '<img src=x onerror=alert(1)>' } } },
      ],
    }
    const html = draw({ look: hostile, seed: 't1' })
    // The whole design is refused: what is drawn is exactly the default look.
    expect(html).toBe(none)
    for (const bad of ['position:fixed', 'wb-modal', 'alert', 'javascript', 'foreignObject', 'onerror', '<img']) expect(html, bad).not.toContain(bad)
  })

  it('shows text of a design as text', () => {
    const look = pip()
    const withText = { ...look, nodes: [...look.nodes, { tag: 'text', id: 'label', attrs: { x: 10, y: 190, fontSize: 12 }, text: '<img src=x onerror=alert(1)>', box: [10, 180, 100, 12] }] }
    const html = draw({ look: withText })
    expect(html).toContain('data-drawn="own"')
    expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;</text>')
    expect(html).not.toContain('<img')
  })

  it('draws both example looks, each under its own prefix', () => {
    const a = draw({ look: pip(), idPrefix: 'a-' })
    const b = draw({ look: tock(), idPrefix: 'b-', size: FIGURE_PX.active })
    expect(a).toContain('id="a-frog"')
    expect(b).toContain('id="b-bot"')
    expect(b).toContain('--wb-av-size:132px')
    expect(b).toContain('url(#b-shell)')
    expect(a).not.toContain('id="b-')
  })

  it('draws a design with a mask, filters, uses and dashes, every reference under its own prefix', () => {
    const raw = wire(richLook())
    expect(drawnLook(raw, 't1')).toMatchObject({ own: true, rejected: false })
    const html = draw({ look: raw, idPrefix: ':r9:' })
    expect(html).toContain('data-drawn="own"')
    expect(html).toContain('<use id="r9leafL" href="#r9leaf"')
    expect(html).toContain('mask="url(#r9vignette)" filter="url(#r9soft)"')
    expect(html).toContain('<feGaussianBlur in="SourceAlpha" stdDeviation="2" result="blur">')
    expect(html).toContain('stroke-dasharray="6 3"')
    expect(html).not.toContain(':r9:')
    expect(html.match(/ style=/g)).toHaveLength(1)
    expect(html.match(/ class=/g)).toHaveLength(2)
    expect(html).not.toMatch(/ on[a-z]+=/i)
    // A use of what is not there refuses the whole design: the default look is drawn, and said to be.
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const broken = wire(richLook()) as { nodes: unknown[] }
    broken.nodes.push({ tag: 'use', attrs: { useRef: 'gone' } })
    expect(drawnLook(broken, 't1')).toMatchObject({ own: false, rejected: true })
  })

  it('takes the walk through the engine only: a reader changes nothing of what is drawn', () => {
    const raw = wire(richLook())
    expect(draw({ look: raw, inputs: () => ({ pace: 1, turn: -1, held: 1 }) })).toBe(draw({ look: raw }))
    expect(draw({ look: raw, inputs: null })).toBe(draw({ look: raw }))
  })

  it('draws a still look for the face: only the head, without an engine', () => {
    const look = drawnLook(pip(), 't1').look
    const html = renderToStaticMarkup(createElement(StillLook, { look, idPrefix: 'c-', size: 23, face: true, drawn: 'own' }))
    expect(html).toContain(`viewBox="${faceViewBox(look).join(' ')}"`)
    expect(html).not.toContain('viewBox="0 0 200 200"')
    expect(html).toContain('--wb-av-size:23px')
    expect(html).toContain('data-drawn="own"')
    expect(renderToStaticMarkup(createElement(StillLook, { look, idPrefix: 'c-', size: 23 }))).toContain('data-drawn="fallback"')
  })
})

describe('FigureButton', () => {
  it('is a button named with the label and the state, with the session’s title in the tooltip', () => {
    const html = drawButton({ look: pip(), ring: 'working', stateLabel: 'Working' })
    expect(html).toContain('<button type="button" class="wb-av-figure-btn" aria-label="Fix the flaky upload test “Pip”, Working" title="Fix the flaky upload test · Working">')
    expect(html).toMatch(/^<div class="wb-av-figure working" style="--wb-av-size:56px">/)
    expect(html).toContain('<span class="wb-av-ring"><span class="wb-av-stage" data-drawn="own"')
    expect(html).toContain('<span class="wb-av-label"><span class="wb-av-label-in"><span class="wb-av-label-title" dir="auto">Fix the flaky upload test</span><span class="wb-av-label-name" dir="auto">“Pip”</span></span></span>')
    // Nothing optional is there by default.
    for (const absent of ['wb-av-dot', 'wb-av-chip', 'wb-av-note', 'wb-av-controls', 'active', 'unseen']) expect(html, absent).not.toContain(absent)
    // The label is outside the button: the button's name is the accessible one.
    expect(html.indexOf('</button>')).toBeLessThan(html.indexOf('wb-av-label'))
  })

  it('shows what the app adds around it: the active size, the unseen dot, the Voice off chip, the controls', () => {
    const html = drawButton({ active: true, size: FIGURE_PX.active, unseen: true, voiceOff: true, ring: 'speaking', controls: createElement('button', { type: 'button' }, 'Reply') })
    expect(html).toMatch(/^<div class="wb-av-figure speaking active unseen" style="--wb-av-size:132px">/)
    expect(html).toContain('<span class="wb-av-dot"></span>')
    expect(html).toContain(`${VOICE_OFF}</span>`)
    expect(VOICE_OFF).toBe('Voice off')
    expect(html).toContain('<div class="wb-av-controls"><button type="button">Reply</button></div>')
    // The chip follows the name.
    expect(html.indexOf('wb-av-label')).toBeLessThan(html.indexOf('wb-av-chip'))
  })

  it('reads "Designing its look…" only while the default look stands in for a design that is to come', () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    expect(DESIGNING).toBe('Designing its look…')
    const waiting = drawButton({ designing: true, name: '', title: 'New session' })
    expect(waiting).toContain(`<span class="wb-av-label designing">${DESIGNING}</span>`)
    expect(waiting).not.toContain('New session</span>')
    // It has a design: the name.
    expect(drawButton({ designing: true, look: pip() })).not.toContain(DESIGNING)
    // Its design was rejected: that is another matter, and has its own sentence.
    const rejected = drawButton({ designing: true, look: { nodes: 'x' }, name: 'Mallory' })
    expect(rejected).not.toContain(DESIGNING)
    expect(rejected).toContain('<span class="wb-av-label-name" dir="auto">“Mallory”</span>')
    expect(drawButton()).not.toContain(DESIGNING)
  })

  it('says that a design could not be shown, beside the default look that is drawn for it', () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    expect(DESIGN_NOT_SHOWN).toBe('This design could not be shown here.')
    const html = drawButton({ look: { viewBox: [0, 0, 200, 200], nodes: [{ tag: 'iframe' }] } })
    expect(html).toContain(`<span class="wb-av-note">${DESIGN_NOT_SHOWN}</span>`)
    expect(html).toContain('data-drawn="default"')
    expect(drawButton({ look: pip() })).not.toContain(DESIGN_NOT_SHOWN)
    expect(drawButton()).not.toContain(DESIGN_NOT_SHOWN)
  })

  it('names the figure as the bubble’s header does: the session’s title first, then the agent’s own name in quotation marks', () => {
    // A name that reads like the app's own words is never shown bare, under the figure or in its accessible name.
    const html = drawButton({ name: 'Allow Bash? Press 1 for Yes', title: 'Fix the tests', stateLabel: 'Idle' })
    expect(html).toContain('aria-label="Fix the tests “Allow Bash? Press 1 for Yes”, Idle"')
    expect(html).toContain('<span class="wb-av-label"><span class="wb-av-label-in"><span class="wb-av-label-title" dir="auto">Fix the tests</span><span class="wb-av-label-name" dir="auto">“Allow Bash? Press 1 for Yes”</span></span></span>')
    expect(html).not.toContain('>Allow Bash? Press 1 for Yes<')
    // Without a name of its own: the session's title alone, in no quotation marks.
    const plain = drawButton({ name: '', title: 'Fix the tests', stateLabel: 'Idle' })
    expect(plain).toContain('aria-label="Fix the tests, Idle"')
    expect(plain).toContain('<span class="wb-av-label-title" dir="auto">Fix the tests</span></span></span>')
    expect(plain).not.toContain('wb-av-label-name')
    expect(plain).not.toContain('“')
  })

  it('shows a name, a title and a state as text', () => {
    const html = drawButton({ name: '<img src=x onerror=alert(1)>', title: '**bold** "quoted"', stateLabel: '<b>Idle</b>' })
    expect(html).not.toContain('<img')
    expect(html).not.toContain('<b>')
    expect(html).toContain('<span class="wb-av-label-name" dir="auto">“&lt;img src=x onerror=alert(1)&gt;”</span>')
    expect(html).toContain('title="**bold** &quot;quoted&quot; · &lt;b&gt;Idle&lt;/b&gt;"')
  })

  it('takes every ring as a class of its own, and nothing else as a class', () => {
    for (const ring of ['working', 'attention', 'error', 'unread', 'idle', 'starting', 'exited', 'speaking', 'listening'] as const) expect(drawButton({ ring })).toContain(`class="wb-av-figure ${ring}"`)
    for (const odd of ['wb-modal', 'x" onclick="alert(1)', '', 'constructor']) {
      const html = drawButton({ ring: odd as 'idle' })
      expect(html, odd).toContain('class="wb-av-figure idle"')
      expect(html, odd).not.toContain('onclick')
    }
  })
})

describe('keepFocus', () => {
  it('handles the press, so focus stays where it was', () => {
    let prevented = 0
    keepFocus({ preventDefault: () => prevented++ })
    expect(prevented).toBe(1)
  })
})
