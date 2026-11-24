// The avatar's style sheet, read as text: tokens only and no colour literal, the feature's
// own class prefix, the stage's containment and limit, the layer below the app's own
// overlays, and reduced motion. Nothing else in the repository checks a style sheet.

import { describe, expect, it } from 'vitest'

/**
 * A style sheet as text. vitest answers every `.css` import with an empty string (`?raw`
 * included), and the app's tsconfig has no node types, so the file is read through a
 * `node:fs` whose name the type checker is not shown.
 */
async function sheet(path: string): Promise<string> {
  const fs = 'node:fs'
  const { readFileSync } = (await import(/* @vite-ignore */ fs)) as { readFileSync(file: URL, encoding: 'utf8'): string }
  return readFileSync(new URL(path, import.meta.url), 'utf8')
}

const css = await sheet('./avatar.css')
const tokens = await sheet('../../theme/tokens.css')

/** The sheet without its comments. */
const rules = css.replace(/\/\*[\s\S]*?\*\//g, '')

/** The declarations of every rule whose selector list holds `selector` as a whole selector. */
function block(selector: string): string {
  const found: string[] = []
  for (const m of rules.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (m[1].split(',').some((s) => s.trim() === selector)) found.push(m[2])
  }
  if (found.length === 0) throw new Error(`no rule for ${selector}`)
  return found.join('\n')
}

describe('avatar.css', () => {
  it('is read: this test looks at the real sheets', () => {
    expect(css.length).toBeGreaterThan(5000)
    expect(css).toContain('.wb-av-stage')
    expect(tokens).toContain('--agent-working')
  })

  it('holds no colour literal: every colour is a theme token', () => {
    expect(rules).not.toMatch(/#[0-9a-fA-F]{3,8}\b/)
    expect(rules).not.toMatch(/\b(?:rgb|rgba|hsl|hsla|hwb|lab|lch|oklab|oklch)\(/)
    // No colour by name either: a colour only ever comes out of var() or color-mix() of tokens (or is transparent).
    for (const m of rules.matchAll(/(?:^|[;{\s])(?:color|background|background-color|border-color|outline-color|border(?:-[a-z]+)?|outline|box-shadow|fill|stroke)\s*:\s*([^;}]+)/g)) {
      const value = m[1].replace(/var\(--[a-z0-9-]+(?:,[^)]*)?\)/g, 'V').replace(/color-mix\([^()]*(?:\([^()]*\)[^()]*)*\)/g, 'V')
      expect(value, m[0].trim()).not.toMatch(/\b(?:white|black|red|green|blue|gray|grey|yellow|orange|purple|pink|silver|navy|teal|aqua|lime|maroon|olive|fuchsia)\b/)
    }
  })

  it('uses only tokens that exist, and its own variables', () => {
    const known = new Set([...tokens.matchAll(/(--[a-z0-9-]+)\s*:/g)].map((m) => m[1]))
    const used = new Set([...rules.matchAll(/var\((--[a-z0-9-]+)/g)].map((m) => m[1]))
    expect(used.size).toBeGreaterThan(10)
    for (const name of used) expect(known.has(name) || name.startsWith('--wb-av-'), name).toBe(true)
    // The ring is the agents' state colours; speaking is the accent, listening the recording colour.
    for (const name of ['--agent-working', '--agent-attention', '--agent-idle', '--agent-error']) expect(used.has(name), name).toBe(true)
    expect(block('.wb-av-facemark.speaking')).toContain('--wb-av-ring: var(--accent)')
    expect(block('.wb-av-facemark.listening')).toContain('--wb-av-ring: var(--danger)')
    expect(block('.wb-av-figure.working')).toContain('--wb-av-ring: var(--agent-working)')
    expect(block('.wb-av-figure.attention')).toContain('--wb-av-ring: var(--agent-attention)')
    expect(block('.wb-av-figure.error')).toContain('--wb-av-ring: var(--agent-error)')
  })

  it('names its classes wb-av-, and styles foreign ones only inside its own', () => {
    /** State and variant words that stand beside a `wb-av-` class, and the shared components the bubble is made of. */
    const beside = new Set(['working', 'starting', 'attention', 'error', 'unread', 'exited', 'speaking', 'listening', 'active', 'unseen', 'designing', 'user', 'now', 'changed', 'through', 'hold', 'panel', 'tail-left', 'tail-right', 'still', 'left', 'draggable', 'dragging', 'roam', 'small', 'claude', 'codex', 'kimi', 'gemini', 'aider', 'wb-btn', 'wb-icon-btn', 'wb-textarea', 'wb-spinner'])
    let selectors = 0
    for (const m of rules.matchAll(/([^{}@]+)\{[^{}]*\}/g)) {
      for (const selector of m[1].split(',')) {
        const s = selector.trim()
        if (!s || /^(?:from|to|\d+%)$/.test(s)) continue
        selectors++
        expect(s, s).toContain('.wb-av-')
        for (const c of s.matchAll(/\.([A-Za-z0-9_-]+)/g)) expect(c[1].startsWith('wb-av-') || beside.has(c[1]), `${s}: .${c[1]}`).toBe(true)
        // Never an element or an id of the page at large.
        expect(s.replace(/\.[A-Za-z0-9_-]+/g, ''), s).not.toMatch(/#|\bbody\b|\bhtml\b|:root|\*/)
      }
    }
    expect(selectors).toBeGreaterThan(80)
    for (const name of rules.matchAll(/@keyframes\s+([A-Za-z0-9_-]+)/g)) expect(name[1].startsWith('wb-av-'), name[1]).toBe(true)
  })

  it('keeps the artwork in a clipped box of the app’s own, never larger than 132 px', () => {
    const stage = block('.wb-av-stage')
    for (const rule of ['overflow: hidden', 'contain: strict', 'isolation: isolate', 'max-width: 132px', 'max-height: 132px', 'width: var(--wb-av-size)', 'height: var(--wb-av-size)']) expect(stage, rule).toContain(rule)
    // Nothing of the box is seen: no backdrop, no border, no rounded corner. Only the figure is.
    expect(stage).not.toMatch(/background|box-shadow|border(?:-[a-z]+)?\s*:/)
    // The state shows as a glow around the figure's own outline, never as a frame.
    const ring = block('.wb-av-ring')
    expect(ring).toContain('filter: var(--wb-av-edge)')
    expect(ring).not.toMatch(/border(?:-[a-z]+)?\s*:|background|padding/)
    expect(block('.wb-av-figure.attention .wb-av-ring')).toContain('var(--wb-av-glow)')
    // The name and the chip come with the large figure only.
    expect(block('.wb-av-figure:not(.active) .wb-av-chip')).toContain('display: none')
    const art = block('.wb-av-art')
    for (const rule of ['pointer-events: none', 'user-select: none', 'color: var(--fg)', 'font-family: var(--font-ui)']) expect(art, rule).toContain(rule)
  })

  it('fades a face out at its lower edge, where the crop to the head cuts through the body', () => {
    expect(block('.wb-av-facemark .wb-av-stage:not(.wb-av-more)')).toMatch(/mask-image:\s*linear-gradient\(to bottom, var\(--fg\) \d+%, transparent\)/)
    // A figure is drawn whole, and the "+n" chip is a box to see: neither of them fades.
    expect(block('.wb-av-stage')).not.toContain('mask')
    expect(block('.wb-av-stage.wb-av-more')).not.toContain('mask')
    // The edge line and the glow are drawn around what the mask leaves: nothing masks what draws them.
    expect(rules).not.toMatch(/\.wb-av-ring[^{}]*\{[^{}]*mask/)
  })

  it('keeps the states apart where nothing moves: by how far the glow reaches as well as by its colour', () => {
    const ring = block('.wb-av-ring')
    const reach = (name: string) => Number(new RegExp(`${name}:\\s*drop-shadow\\(0 0 (\\d+)px`).exec(ring)?.[1])
    expect(reach('--wb-av-glow')).toBe(4)
    expect(reach('--wb-av-halo')).toBeGreaterThan(reach('--wb-av-glow'))
    for (const kind of ['figure', 'facemark']) {
      // A session that needs the user, and an avatar that speaks or listens: the glow and a wider one around it.
      for (const state of ['attention', 'speaking', 'listening']) expect(block(`.wb-av-${kind}.${state} .wb-av-ring`), `${kind} ${state}`).toContain('filter: var(--wb-av-edge) var(--wb-av-glow) var(--wb-av-halo)')
      // An error and an unread line: the glow alone. So a still listening figure is no failed one, and a speaking one
      // is no unread one, though each pair shares its colour.
      for (const state of ['error', 'unread']) {
        const filter = block(`.wb-av-${kind}.${state} .wb-av-ring`)
        expect(filter, `${kind} ${state}`).toContain('filter: var(--wb-av-edge) var(--wb-av-glow)')
        expect(filter, `${kind} ${state}`).not.toContain('--wb-av-halo')
      }
      // A session that works: the faint glow, narrower than both.
      for (const state of ['working', 'starting']) {
        const filter = block(`.wb-av-${kind}.${state} .wb-av-ring`)
        expect(filter, `${kind} ${state}`).toContain('filter: var(--wb-av-edge) drop-shadow(0 0 2px var(--wb-av-ring))')
        expect(filter, `${kind} ${state}`).not.toMatch(/--wb-av-glow|--wb-av-halo/)
      }
    }
  })

  it('puts the unseen dot near the figure, gives the name room in the panel, and flashes nothing on a tap', () => {
    // The figure rests inside the middle 70% of its square: the dot's centre is a quarter of the way in, not at the corner.
    const dot = block('.wb-av-dot')
    for (const rule of ['top: calc(25% - 6px)', 'right: calc(25% - 6px)', 'width: 12px', 'height: 12px']) expect(dot, rule).toContain(rule)
    // A face keeps its dot at its corner.
    expect(block('.wb-av-facemark .wb-av-dot')).toContain('top: -3px')
    // The panel cuts off what passes its edge: the figure has room there for its glow, and at its sides for its name.
    const overhang = /margin-left:\s*-(\d+)px/.exec(block('.wb-av-label'))?.[1]
    expect(overhang).toBe('11')
    expect(block('.wb-av-bubble.panel .wb-av-bubble-figure')).toContain(`padding: 4px ${overhang}px`)
    for (const target of ['.wb-av-figure-btn', '.wb-av-face']) expect(block(target), target).toContain('-webkit-tap-highlight-color: transparent')
  })

  it('sits on layer 700, below the palette, modals and toasts, and takes no pointer but for its own pieces', () => {
    const layer = block('.wb-av-layer')
    expect(layer).toContain('z-index: 700')
    expect(layer).toContain('position: fixed')
    expect(layer).toContain('pointer-events: none')
    // No other layer in the sheet. What else is ordered is ordered inside the layer (walking figures: the large
    // one above the others, a dragged one above those, and the "+n" chip beside them above every walking figure)
    // or inside a figure, which is a stacking context of its own.
    expect([...rules.matchAll(/z-index:\s*(-?\d+)/g)].map((m) => Number(m[1])).sort((a, b) => a - b)).toEqual([1, 1, 2, 3, 4, 700])
    expect(block('.wb-av-slot.roam')).toContain('z-index: 1')
    expect(block('.wb-av-slot.roam.active')).toContain('z-index: 2')
    expect(block('.wb-av-slot.roam.dragging')).toContain('z-index: 3')
    expect(block('.wb-av-slot.roam ~ .wb-av-group')).toContain('z-index: 4')
    expect(block('.wb-av-controls')).toContain('z-index: 1')
    expect(block('.wb-av-figure')).toContain('isolation: isolate')
    expect(block('.wb-av-layer .wb-av-bubble')).toContain('pointer-events: auto')
    expect(block('.wb-av-bubble.through')).toContain('pointer-events: none')
    // The click-through form is stated after the rule it has to win over.
    expect(rules.indexOf('.wb-av-layer .wb-av-bubble.through')).toBeGreaterThan(rules.indexOf('.wb-av-layer .wb-av-bubble {'))
  })

  it('lets a click through everything inside a bubble that lets clicks through, the figure of the compact panel too', () => {
    // A piece that takes the pointer by itself would take it inside a bubble that does not: each one gives it up there.
    const takers = [...rules.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter((m) => /pointer-events:\s*auto/.test(m[2])).flatMap((m) => m[1].split(',').map((s) => s.trim()))
    expect(takers).toContain('.wb-av-layer .wb-av-figure')
    for (const taker of takers) {
      const piece = taker.split(/\s+/).at(-1) ?? ''
      if (piece === '.wb-av-bubble') continue
      expect(block(`.wb-av-layer .wb-av-bubble.through ${piece}`), piece).toContain('pointer-events: none')
    }
  })

  it('lets a bubble that holds the pointer take it, with its controls inactive and dimmed', () => {
    // The bubble itself keeps the pointer: no rule of the held form gives it away.
    expect(rules).not.toMatch(/\.wb-av-bubble\.hold\s*[,{]/)
    for (const control of ['button', 'textarea']) expect(block(`.wb-av-bubble.hold ${control}`), control).toContain('pointer-events: none')
    // Dimmed as a disabled button is, with no colour of its own.
    for (const control of ['.wb-btn', '.wb-icon-btn', '.wb-textarea']) {
      const dim = block(`.wb-av-bubble.hold ${control}`)
      expect(dim, control).toContain('opacity: 0.5')
      expect(dim, control).not.toMatch(/color|background|border/)
    }
  })

  it('stops what it animates under reduced motion, the system’s and the user’s', () => {
    const system = /@media \(prefers-reduced-motion: reduce\)\s*\{([\s\S]*?)\n\}/.exec(rules)?.[1] ?? ''
    expect(system).toContain('.wb-av-stage')
    expect(system).toContain('transition: none')
    expect(system).toContain('animation: none')
    expect(block('.wb-av-layer.still .wb-av-ring')).toContain('animation: none')
    expect(block('.wb-av-layer.still .wb-av-stage')).toContain('transition: none')
    // Every animation and transition of the sheet is one of those two rules' business.
    const animated = [...rules.matchAll(/([^{}]+)\{[^{}]*\banimation:\s*wb-av-[^{}]*\}/g)].flatMap((m) => m[1].split(',').map((s) => s.trim()))
    expect(animated.length).toBeGreaterThan(0)
    for (const selector of animated) expect(/\.wb-av-(?:art|ring)$/.test(selector), selector).toBe(true)
  })

  it('gives touch screens larger targets', () => {
    const coarse = /@media \(pointer: coarse\)\s*\{([\s\S]*?)\n\}/.exec(rules)?.[1] ?? ''
    expect(coarse).toContain('.wb-btn.wb-av-choice')
    expect(coarse).toContain('min-height: 36px')
  })

  it('gives a question’s choices a look of their own', () => {
    const choice = block('.wb-btn.wb-av-choice')
    expect(choice).toContain('border-radius: 14px')
    expect(choice).toContain('var(--accent)')
    // Nothing of the permission card: not its classes, not its colour.
    expect(rules).not.toContain('wb-ag-perm')
    expect(choice).not.toContain('--warning')
    expect(choice).not.toContain('--agent-attention')
  })
})
