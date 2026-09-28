// Palette ranking and keyboard-shortcut routing, kept free of React for tests.

import { defaultFilter } from 'cmdk'
import type { Command } from './types'

/**
 * Commands matching `query`, best first. cmdk's own sorting cannot reorder groups,
 * so the palette filters here and renders one flat list while searching. An exact
 * or prefix title match always beats a fuzzy one.
 */
export function rankCommands(commands: Command[], query: string): Command[] {
  const q = query.trim().toLowerCase()
  if (!q) return commands
  const scored = commands
    .map((c, i) => {
      const title = c.title.toLowerCase()
      let score = defaultFilter(c.title, q, c.keywords ?? [])
      if (score <= 0) return { c, i, score }
      if (title === q || title.replace(/[….]+$/, '') === q) score += 3
      else if (title.startsWith(q)) score += 2
      else if (title.split(/[\s/·:()-]+/).some((w) => w.startsWith(q))) score += 1
      return { c, i, score }
    })
    .filter((x) => x.score > 0)
  scored.sort((a, b) => b.score - a.score || a.i - b.i)
  return scored.map((x) => x.c)
}

/** Drop generated commands whose title a feature command already has ("Show Git Log"). */
export function dedupeByTitle(featureCmds: Command[], generated: Command[]): Command[] {
  const titles = new Set(featureCmds.map((c) => c.title.toLowerCase()))
  return [...featureCmds, ...generated.filter((c) => !titles.has(c.title.toLowerCase()))]
}

export function matchShortcut(e: Pick<KeyboardEvent, 'key' | 'code' | 'ctrlKey' | 'metaKey' | 'shiftKey' | 'altKey'>, shortcut: string, isMac = isMacPlatform()): boolean {
  const parts = shortcut.toLowerCase().split('+')
  const key = parts.pop()!
  const mod = parts.includes('mod')
  if (mod && !(isMac ? e.metaKey : e.ctrlKey)) return false
  if (!mod && (e.ctrlKey || e.metaKey) && !parts.includes('ctrl')) return false
  if (parts.includes('shift') !== e.shiftKey) return false
  if (parts.includes('alt') !== e.altKey) return false
  return e.key.toLowerCase() === key || e.code.toLowerCase() === `key${key}` || e.code.toLowerCase() === `digit${key}`
}

export function isMacPlatform() {
  return typeof navigator !== 'undefined' && navigator.platform.toLowerCase().includes('mac')
}

/** Shortcuts that work everywhere, terminals included: open the palette. */
export const GLOBAL_PALETTE_SHORTCUT = 'mod+shift+p'

/**
 * Whether a feature shortcut may act on this key event. A terminal owns every key
 * typed into it (Ctrl+T, Ctrl+K, Ctrl+P, Alt+digit mean something to bash and to
 * Claude Code), like CLion's "Override IDE shortcuts". Keys a focused widget
 * already handled (Monaco's own bindings, xterm) are theirs too.
 */
export function shortcutsMayHandle(e: Pick<KeyboardEvent, 'defaultPrevented' | 'target'>, command?: Pick<Command, 'global'>): boolean {
  if (command?.global) return true
  if (e.defaultPrevented) return false
  const el = e.target as { closest?: (s: string) => unknown } | null
  return !(el && typeof el.closest === 'function' && el.closest('.xterm'))
}

type KeyLike = Pick<KeyboardEvent, 'key' | 'repeat'>

/**
 * Double Shift (Search Everywhere, as in CLion): two taps of Shift alone, each
 * released within `windowMs` of its press and the second pressed within `windowMs`
 * of the first release. Any other key between or during them (Shift+A while typing)
 * cancels it. Feed it every keydown and keyup; it never consumes events.
 */
export function doubleShiftDetector(onDouble: () => void, windowMs = 400) {
  let lastTap = -Infinity
  let pressedAt = -Infinity
  let alone = false
  return {
    keydown(e: KeyLike, now: number) {
      if (e.key === 'Shift') {
        if (!e.repeat) {
          alone = true
          pressedAt = now
        }
      } else {
        alone = false
        lastTap = -Infinity
      }
    },
    keyup(e: KeyLike, now: number) {
      if (e.key !== 'Shift') return
      const tap = alone && now - pressedAt <= windowMs
      alone = false
      if (!tap) {
        lastTap = -Infinity
        return
      }
      if (pressedAt - lastTap <= windowMs) {
        lastTap = -Infinity
        onDouble()
      } else {
        lastTap = now
      }
    },
    /** Focus moved away (a window blur): start over. */
    reset() {
      alone = false
      lastTap = -Infinity
    },
  }
}
