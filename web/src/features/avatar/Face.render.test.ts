// The small face, rendered on the server: the head of the look inside the size it is given,
// the ring, the unseen dot, the count, and the "+n" chip.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, describe, expect, it, vi } from 'vitest'
import pipRaw from '../../../../server/src/avatar/examples/pip.look.json?raw'
import { Face, FaceMark, MoreFaces } from './Face'
import type { FaceProps } from './Face'
import { FACE_PX, FIGURE_PX, FIGURE_RING_PX, drawnLook } from './Figure'
import { faceViewBox } from './lib/look'
import type { Look } from './lib/types'

const pip = JSON.parse(pipRaw) as Look
const face = (over: Partial<FaceProps> = {}) => renderToStaticMarkup(createElement(Face, { look: pip, seed: 't1', idPrefix: 'c1-', ring: 'idle', label: 'Pip, Idle', ...over }))

afterEach(() => {
  vi.restoreAllMocks()
})

describe('Face', () => {
  it('is a button that shows the head of the look, named by its label', () => {
    const html = face({ ring: 'working', label: 'Pip, Working' })
    expect(html).toMatch(/^<button type="button" class="wb-av-face" aria-label="Pip, Working" title="Pip, Working">/)
    expect(html).toContain('<span class="wb-av-facemark working"')
    // The head, not the whole figure.
    const head = faceViewBox(drawnLook(pip, 't1').look).join(' ')
    expect(head).not.toBe('0 0 200 200')
    expect(html).toContain(`viewBox="${head}"`)
    expect(html).toContain('aria-hidden="true"')
    expect(html).toContain('id="c1-mouth"')
    expect(html).toContain('data-drawn="own"')
    for (const absent of ['wb-av-dot', 'wb-av-count', 'aria-expanded', 'unseen']) expect(html, absent).not.toContain(absent)
  })

  it('is as large as the place it is given: its stage fills it', () => {
    // 28 px in the top bar and 40 px on the phone: no room is kept for a ring, since none is drawn.
    expect(face()).toContain('--wb-av-size:28px')
    expect(face({ size: FACE_PX.phone })).toContain('--wb-av-size:40px')
    expect(face({ size: 4 })).toContain('--wb-av-size:12px')
    expect(face({ size: Number.NaN })).toContain('--wb-av-size:28px')
    // Never a stage beyond the limit of a stage.
    expect(face({ size: 900 })).toContain('--wb-av-size:132px')
  })

  it('shows the unseen dot and a count', () => {
    expect(face({ unseen: true })).toContain('<span class="wb-av-dot"></span>')
    expect(face({ unseen: true })).toContain('class="wb-av-facemark idle unseen"')
    expect(face({ count: 2 })).toContain('<span class="wb-av-count">2</span>')
    expect(face({ count: 2, countTone: 'attention' })).toContain('<span class="wb-av-count attention">2</span>')
    expect(face({ count: 120 })).toContain('<span class="wb-av-count">99+</span>')
    expect(face({ count: 3.9 })).toContain('<span class="wb-av-count">3</span>')
    for (const none of [0, -1, Number.NaN, undefined]) expect(face({ count: none })).not.toContain('wb-av-count')
  })

  it('says whether its bubble is open, when it has one', () => {
    expect(face({ open: true })).toContain('aria-expanded="true"')
    expect(face({ open: false })).toContain('aria-expanded="false"')
  })

  it('shows the default look’s face for a session without a design, and for a design that is rejected', () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const none = face({ look: null })
    expect(none).toContain('data-drawn="default"')
    expect(none).toContain('var(--')
    expect(face({ look: { nodes: [{ tag: 'script' }] } })).toBe(none)
  })

  it('takes only a ring of the style sheet as a class', () => {
    expect(face({ ring: 'wb-modal' as 'idle' })).toContain('class="wb-av-facemark idle"')
    expect(renderToStaticMarkup(createElement(MoreFaces, { count: 1, ring: 'x y' as 'idle', label: 'one more' }))).toContain('class="wb-av-facemark idle"')
  })

  it('shows its label as text', () => {
    const html = face({ label: '<img src=x onerror=alert(1)>' })
    expect(html).not.toContain('<img')
    expect(html).toContain('aria-label="&lt;img src=x onerror=alert(1)&gt;"')
  })
})

describe('FaceMark', () => {
  it('is the face without the button, for a place that is a control already', () => {
    const html = renderToStaticMarkup(createElement(FaceMark, { look: pip, seed: 't1', idPrefix: 's1-', ring: 'speaking', size: 18, unseen: true, count: 4 }))
    expect(html).toMatch(/^<span class="wb-av-facemark speaking unseen" style="--wb-av-size:18px">/)
    expect(html).not.toContain('<button')
    expect(html).toContain('<span class="wb-av-count">4</span>')
  })
})

describe('MoreFaces', () => {
  const more = (over: Partial<Parameters<typeof MoreFaces>[0]> = {}) => renderToStaticMarkup(createElement(MoreFaces, { count: 3, ring: 'attention', label: '3 more avatars', ...over }))

  it('is a chip that says how many more there are, and opens a menu', () => {
    const html = more({ unseen: true })
    expect(html).toMatch(/^<button type="button" class="wb-av-face" aria-label="3 more avatars" title="3 more avatars" aria-haspopup="menu">/)
    expect(html).toContain('<span class="wb-av-stage wb-av-more" style="--wb-av-size:28px">+3</span>')
    expect(html).toContain('class="wb-av-facemark attention unseen"')
    expect(html).toContain('wb-av-dot')
    expect(html).not.toContain('<svg')
  })

  it('is as large as a resting figure when it stands beside figures', () => {
    // The overlay gives it a resting figure's stage and FIGURE_RING_PX, which is nothing now that no ring is drawn.
    expect(FIGURE_PX.rest + FIGURE_RING_PX).toBe(56)
    expect(more({ size: FIGURE_PX.rest + FIGURE_RING_PX })).toContain('--wb-av-size:56px')
  })

  it('keeps its number sane', () => {
    expect(more({ count: 250 })).toContain('>+99<')
    expect(more({ count: -4 })).toContain('>+0<')
    expect(more({ count: Number.NaN })).toContain('>+0<')
    expect(more({ count: 2.7 })).toContain('>+2<')
  })
})
