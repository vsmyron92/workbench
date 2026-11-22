// The menu of an avatar (its "More" control), as entries for the shell's menu host. What
// it offers depends only on what comes in; what an entry does goes out through callbacks.

import { Check, CircleStop, EyeOff, PowerOff, RotateCcw, Settings } from 'lucide-react'
import type { MenuEntry } from '@/ui'
import { DESIGN_NOT_SHOWN } from './Figure'
import type { Corner, Mode } from './lib/layout'

export const MENU_WORDS = {
  stop: 'Stop speaking',
  voice: 'Voice in this browser',
  hide: 'Hide all avatars',
  defaultLook: 'Use the default look',
  turnOff: 'Turn off for this session',
  reduceMotion: 'Reduce motion',
  roam: 'Walk around',
  settings: 'Avatar settings',
  reset: 'Reset position',
} as const

const CORNERS: readonly { corner: Corner; label: string }[] = [
  { corner: 'top-right', label: 'Move to top right' },
  { corner: 'bottom-right', label: 'Move to bottom right' },
  { corner: 'bottom-left', label: 'Move to bottom left' },
  { corner: 'top-left', label: 'Move to top left' },
]

export interface MenuInput {
  mode: Mode
  /** This tab presents a line now (aloud or as a caption): there is something to stop. */
  presenting: boolean
  /** The voice preference of this browser. */
  voice: boolean | null
  reduceMotion: boolean
  /** The avatars walk about the workspace window (the preference; reduced motion keeps them in their column all the same). */
  roam: boolean
  /** The system asks for reduced motion: walking is not on offer. */
  systemReduced?: boolean
  /** The session drew a look of its own. */
  designed: boolean
  /** That look cannot be shown in this browser: the default look stands in. */
  rejected: boolean
  /** The group was moved from its default place. */
  moved: boolean
  /** The other avatars on stage, where only one is in sight (the compact forms). */
  others: readonly { id: string; label: string }[]
}

export interface MenuActions {
  stop(): void
  voice(on: boolean): void
  hide(): void
  defaultLook(): void
  turnOff(): void
  move(corner: Corner): void
  resetPosition(): void
  reduceMotion(on: boolean): void
  roam(on: boolean): void
  settings(): void
  show(id: string): void
}

/** The entries of one avatar's menu, in the order of 10.1. */
export function avatarMenu(i: MenuInput, a: MenuActions): MenuEntry[] {
  const items: MenuEntry[] = []
  if (i.rejected) items.push({ label: DESIGN_NOT_SHOWN, disabled: true, run: () => undefined }, 'separator')
  if (i.mode !== 'wide' && i.others.length > 0) {
    for (const other of i.others) items.push({ label: other.label, run: () => a.show(other.id) })
    items.push('separator')
  }
  items.push(
    // A plain square would read as an empty check box beside the entries that carry a check mark.
    { label: MENU_WORDS.stop, icon: CircleStop, disabled: !i.presenting, run: () => a.stop() },
    { label: MENU_WORDS.voice, icon: i.voice === true ? Check : undefined, run: () => a.voice(i.voice !== true) },
    { label: MENU_WORDS.hide, icon: EyeOff, run: () => a.hide() },
    'separator',
    { label: MENU_WORDS.defaultLook, icon: RotateCcw, disabled: !i.designed, run: () => a.defaultLook() },
    { label: MENU_WORDS.turnOff, icon: PowerOff, run: () => a.turnOff() },
  )
  /** They walk: where the column stands matters only as the place they start from. */
  const walking = i.mode === 'wide' && i.roam && !i.reduceMotion && i.systemReduced !== true
  if (i.mode === 'wide' && !walking) {
    items.push('separator')
    for (const c of CORNERS) items.push({ label: c.label, run: () => a.move(c.corner) })
    items.push({ label: MENU_WORDS.reset, disabled: !i.moved, run: () => a.resetPosition() })
  }
  items.push('separator')
  // Walking is the wide layout's, and reduced motion keeps them still whatever this says.
  if (i.mode === 'wide') items.push({ label: MENU_WORDS.roam, icon: i.roam ? Check : undefined, disabled: i.reduceMotion || i.systemReduced === true, run: () => a.roam(!i.roam) })
  items.push({ label: MENU_WORDS.reduceMotion, icon: i.reduceMotion ? Check : undefined, run: () => a.reduceMotion(!i.reduceMotion) })
  // The phone has no Settings panel: its More tab holds what there is.
  if (i.mode !== 'phone') items.push({ label: MENU_WORDS.settings, icon: Settings, run: () => a.settings() })
  return items
}
