// What is open and active in this window's avatar overlay, for the parts of the feature
// that are loaded with the app and live outside the overlay: the widget in the top bar
// gives the compact face its place, the status-bar widget and the palette commands read
// what is on stage and ask the overlay to act, and the provider hands the overlay what
// it noticed before the overlay was loaded (the last input, the first user gesture, a
// gesture an event asked for).
//
// Always loaded and small: no look, no engine and no speech in here.

import { create } from 'zustand'
import type { ActSignal } from './lib/cache'
import { useAvatarPrefs } from './store'

export interface AvatarRuntime {
  /** Where the desktop shell's top bar takes the compact face (the feature's `columnbar` widget). */
  barSlot: HTMLElement | null
  /** The avatars this window shows: their terminal ids, in order. Empty while there are none, or avatars are hidden. */
  shown: readonly string[]
  /** Those whose figure or face is on screen right now (in the compact form only the one in front). */
  staged: readonly string[]
  /** The active avatar: the large one, whose bubble may be open. */
  active: string | null
  /** Whose bubble is open, and who opened it. */
  open: { terminalId: string; by: 'self' | 'user' } | null
  /** The line this tab presents now; `aloud`: with the voice. */
  presenting: { terminalId: string; lineId: string; aloud: boolean } | null
  /** Voice is on, but the browser wants a click first. */
  locked: boolean
}

interface RuntimeStore extends AvatarRuntime {
  setBarSlot: (el: HTMLElement | null) => void
  /** Changes what is told; nothing is written while nothing differs. */
  tell: (patch: Partial<AvatarRuntime>) => void
}

const sameIds = (a: readonly string[], b: readonly string[]): boolean => a.length === b.length && a.every((id, i) => id === b[i])

/** What is on stage, for whoever shows or commands it from outside the overlay. Read-only for them: the provider and the overlay write it. */
export const useAvatarRuntime = create<RuntimeStore>()((set, get) => ({
  barSlot: null,
  shown: [],
  staged: [],
  active: null,
  open: null,
  presenting: null,
  locked: false,
  setBarSlot: (barSlot) => {
    if (get().barSlot !== barSlot) set({ barSlot })
  },
  tell: (patch) => {
    const s = get()
    const next: Partial<AvatarRuntime> = {}
    if (patch.shown !== undefined && !sameIds(patch.shown, s.shown)) next.shown = patch.shown
    if (patch.staged !== undefined && !sameIds(patch.staged, s.staged)) next.staged = patch.staged
    if (patch.active !== undefined && patch.active !== s.active) next.active = patch.active
    if (patch.open !== undefined && (patch.open?.terminalId !== s.open?.terminalId || patch.open?.by !== s.open?.by)) next.open = patch.open
    if (patch.presenting !== undefined && (patch.presenting?.terminalId !== s.presenting?.terminalId || patch.presenting?.lineId !== s.presenting?.lineId || patch.presenting?.aloud !== s.presenting?.aloud)) next.presenting = patch.presenting
    if (patch.locked !== undefined && patch.locked !== s.locked) next.locked = patch.locked
    if (Object.keys(next).length > 0) set(next)
  },
}))

/** Whether this session's avatar is on screen in this window now (what the agents feature's "finished" toast asks). */
export function isStaged(terminalId: string): boolean {
  return useAvatarRuntime.getState().staged.includes(terminalId)
}

// ---------------------------------------------------------------- what the overlay does on request

export interface OverlayActions {
  /** Stops what this tab presents, aloud or as a caption. false: it presents nothing. */
  stop(): boolean
  /** Opens this avatar's bubble (without an id: the active one's, else the one in front) and moves focus into its reply box. false: there is none. */
  reply(terminalId?: string): boolean
  /** Opens this avatar's bubble. false: it is not on stage. */
  open(terminalId: string): boolean
  /** Turns voice on or off in this browser. Turning it on is the user's click: it unlocks speech, and asks before online voices are used. */
  voice(on: boolean): void
}

let overlay: OverlayActions | null = null

/** The overlay registers what it can do while it is mounted; `null` when it goes. */
export function setOverlayActions(actions: OverlayActions | null) {
  overlay = actions
}

/**
 * Whether an overlay is mounted in this window. Whoever turns voice on asks: a mounted
 * overlay does it inside the click (it also unlocks its speaker), and where none is, the
 * caller has to look at this browser's voices itself.
 */
export function hasOverlay(): boolean {
  return overlay !== null
}

/**
 * What the status-bar widget and the palette commands ask of the overlay. While no
 * overlay is mounted (nothing to show, avatars hidden) there is nothing to stop, to open
 * or to reply to; voice is then only the preference.
 */
export const avatarActions: OverlayActions = {
  stop: () => overlay?.stop() ?? false,
  reply: (terminalId) => overlay?.reply(terminalId) ?? false,
  open: (terminalId) => overlay?.open(terminalId) ?? false,
  voice: (on) => {
    if (overlay) overlay.voice(on)
    else useAvatarPrefs.getState().setPrefs({ voice: on === true })
  },
}

// ---------------------------------------------------------------- gestures asked for by events

const actWatchers = new Set<(act: ActSignal) => void>()

/** The overlay listens for gestures to play once or to stop (`avatar.act`). Returns the function that stops listening. */
export function onAct(cb: (act: ActSignal) => void): () => void {
  actWatchers.add(cb)
  return () => void actWatchers.delete(cb)
}

/** The provider passes a gesture on. Nobody listens while no overlay is mounted: a gesture is not kept for later. */
export function emitAct(act: ActSignal) {
  for (const cb of Array.from(actWatchers)) {
    try {
      cb(act)
    } catch (e) {
      console.error('[workbench] an avatar gesture failed', e)
    }
  }
}

// ---------------------------------------------------------------- input in this tab

/** When the tab last had pointer or key input, on the clock of `performance.now()`. Page load counts as input. */
let lastInput = 0
/** This page load has seen a trusted press of a pointer or a key. */
let gestured = false
const gestureWatchers = new Set<() => void>()

interface InputWindow {
  addEventListener(type: string, cb: (e: { isTrusted?: boolean }) => void, opts: { capture: boolean; passive: boolean }): void
  removeEventListener(type: string, cb: (e: { isTrusted?: boolean }) => void, opts: { capture: boolean }): void
  performance?: { now(): number }
  navigator?: { userActivation?: { hasBeenActive?: boolean } }
}

/**
 * Watches a window's pointer and key input: when it was last used (the tab used last is
 * the one that speaks), and the first user gesture (before one, browsers refuse to speak).
 * Listeners of the capture phase that take nothing. Returns the function that removes them.
 */
export function watchInput(win: InputWindow): () => void {
  const now = () => win.performance?.now() ?? 0
  const moved = () => {
    lastInput = now()
  }
  const pressed = (e: { isTrusted?: boolean }) => {
    lastInput = now()
    if (e?.isTrusted !== true) return
    gestured = true
    for (const cb of Array.from(gestureWatchers)) {
      try {
        // Still inside the event handler: what is called here counts as the user's own act.
        cb()
      } catch (e) {
        console.error('[workbench] the avatar unlock failed', e)
      }
    }
  }
  const opts = { capture: true, passive: true }
  win.addEventListener('pointermove', moved, opts)
  win.addEventListener('pointerdown', pressed, opts)
  win.addEventListener('keydown', pressed, opts)
  return () => {
    win.removeEventListener('pointermove', moved, { capture: true })
    win.removeEventListener('pointerdown', pressed, { capture: true })
    win.removeEventListener('keydown', pressed, { capture: true })
  }
}

/** Time since the last pointer or key input in this tab, in milliseconds. */
export function idleMs(now: number): number {
  return Math.max(0, now - lastInput)
}

/** Whether this page load has had a user gesture, as far as the browser or the watch above can tell. */
export function hadGesture(win: Pick<InputWindow, 'navigator'> | null | undefined): boolean {
  return gestured || win?.navigator?.userActivation?.hasBeenActive === true
}

/** Calls `cb` inside the handler of every trusted press of a pointer or a key. Returns the function that stops it. */
export function onGesture(cb: () => void): () => void {
  gestureWatchers.add(cb)
  return () => void gestureWatchers.delete(cb)
}

/** For tests: as if the page had just loaded. */
export function resetInput() {
  lastInput = 0
  gestured = false
  gestureWatchers.clear()
}
