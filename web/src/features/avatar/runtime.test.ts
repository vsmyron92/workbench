// What the always-loaded parts of the feature share: the store that tells what is on
// stage, the requests to an overlay that may not be mounted, gestures on their way to it,
// and the watch on the tab's input (the last use, the first gesture).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { OverlayActions } from './runtime'

type Runtime = typeof import('./runtime')
type Prefs = typeof import('./store')

let rt: Runtime
let prefs: Prefs

/** A window as far as the input watch needs one: listeners that the test can fire. */
function fakeWindow(userActivation?: { hasBeenActive?: boolean }) {
  const listeners = new Map<string, Set<(e: { isTrusted?: boolean }) => void>>()
  let now = 0
  return {
    addEventListener: (type: string, cb: (e: { isTrusted?: boolean }) => void, opts: { capture: boolean; passive: boolean }) => {
      expect(opts).toEqual({ capture: true, passive: true })
      if (!listeners.has(type)) listeners.set(type, new Set())
      listeners.get(type)?.add(cb)
    },
    removeEventListener: (type: string, cb: (e: { isTrusted?: boolean }) => void) => void listeners.get(type)?.delete(cb),
    performance: { now: () => now },
    navigator: userActivation ? { userActivation } : {},
    fire: (type: string, e: { isTrusted?: boolean } = { isTrusted: true }) => listeners.get(type)?.forEach((cb) => cb(e)),
    at: (ms: number) => {
      now = ms
    },
    count: () => [...listeners.values()].reduce((n, set) => n + set.size, 0),
  }
}

beforeEach(async () => {
  vi.resetModules()
  rt = await import('./runtime')
  prefs = await import('./store')
})

afterEach(() => {
  rt.setOverlayActions(null)
  rt.resetInput()
})

describe('useAvatarRuntime', () => {
  it('starts with nothing on stage', () => {
    expect(rt.useAvatarRuntime.getState()).toMatchObject({ barSlot: null, shown: [], staged: [], active: null, open: null, presenting: null, locked: false })
    expect(rt.isStaged('t1')).toBe(false)
  })

  it('tells what changed, and nobody when nothing did', () => {
    const seen: string[] = []
    const off = rt.useAvatarRuntime.subscribe((s) => seen.push(JSON.stringify([s.shown, s.staged, s.active, s.open, s.presenting, s.locked])))
    const { tell } = rt.useAvatarRuntime.getState()
    tell({ shown: ['t1', 't2'], staged: ['t1'], active: 't1', open: { terminalId: 't1', by: 'self' }, presenting: { terminalId: 't1', lineId: 'L1', aloud: false }, locked: true })
    expect(seen).toHaveLength(1)
    // The same again, as new arrays and objects: no news.
    tell({ shown: ['t1', 't2'], staged: ['t1'], active: 't1', open: { terminalId: 't1', by: 'self' }, presenting: { terminalId: 't1', lineId: 'L1', aloud: false }, locked: true })
    tell({})
    expect(seen).toHaveLength(1)
    expect(rt.isStaged('t1')).toBe(true)
    expect(rt.isStaged('t2')).toBe(false)
    tell({ open: { terminalId: 't1', by: 'user' } })
    tell({ presenting: { terminalId: 't1', lineId: 'L1', aloud: true } })
    tell({ shown: ['t2', 't1'] })
    tell({ active: null, open: null, presenting: null, locked: false, staged: [] })
    expect(seen).toHaveLength(5)
    expect(rt.useAvatarRuntime.getState()).toMatchObject({ shown: ['t2', 't1'], staged: [], active: null, open: null, presenting: null, locked: false })
    off()
  })

  it('keeps the place of the compact face', () => {
    const slot = { tagName: 'SPAN' } as unknown as HTMLElement
    rt.useAvatarRuntime.getState().setBarSlot(slot)
    expect(rt.useAvatarRuntime.getState().barSlot).toBe(slot)
    rt.useAvatarRuntime.getState().setBarSlot(null)
    expect(rt.useAvatarRuntime.getState().barSlot).toBeNull()
  })
})

describe('avatarActions', () => {
  it('has nothing to stop, open or reply to while no overlay is mounted', () => {
    expect(rt.avatarActions.stop()).toBe(false)
    expect(rt.avatarActions.open('t1')).toBe(false)
    expect(rt.avatarActions.reply()).toBe(false)
    expect(rt.avatarActions.reply('t1')).toBe(false)
  })

  it('changes only the preference for voice then', () => {
    rt.avatarActions.voice(true)
    expect(prefs.useAvatarPrefs.getState().voice).toBe(true)
    rt.avatarActions.voice(false)
    expect(prefs.useAvatarPrefs.getState().voice).toBe(false)
  })

  it('tells whether an overlay is mounted, for whoever has to decide without one', () => {
    expect(rt.hasOverlay()).toBe(false)
    rt.setOverlayActions({ stop: () => false, open: () => false, reply: () => false, voice: () => {} })
    expect(rt.hasOverlay()).toBe(true)
    rt.setOverlayActions(null)
    expect(rt.hasOverlay()).toBe(false)
  })

  it('asks the overlay while one is mounted, and nobody again once it is gone', () => {
    const calls: string[] = []
    const overlay: OverlayActions = {
      stop: () => {
        calls.push('stop')
        return true
      },
      open: (id) => {
        calls.push(`open ${id}`)
        return id === 't1'
      },
      reply: (id) => {
        calls.push(`reply ${id ?? 'the active one'}`)
        return true
      },
      voice: (on) => void calls.push(`voice ${on}`),
    }
    rt.setOverlayActions(overlay)
    expect([rt.avatarActions.stop(), rt.avatarActions.open('t1'), rt.avatarActions.open('t9'), rt.avatarActions.reply(), rt.avatarActions.reply('t2')]).toEqual([true, true, false, true, true])
    rt.avatarActions.voice(true)
    expect(calls).toEqual(['stop', 'open t1', 'open t9', 'reply the active one', 'reply t2', 'voice true'])
    // The overlay decides what turning voice on needs: the preference was not touched from here.
    expect(prefs.useAvatarPrefs.getState().voice).toBeNull()
    rt.setOverlayActions(null)
    expect(rt.avatarActions.stop()).toBe(false)
  })
})

describe('gestures on their way to the overlay', () => {
  it('reach whoever listens, and are not kept for later', () => {
    const got: unknown[] = []
    rt.emitAct({ terminalId: 't1', seq: 1, play: 'wave', times: 1 })
    const off = rt.onAct((act) => got.push(act))
    rt.emitAct({ terminalId: 't1', seq: 2, stop: 'all' })
    expect(got).toEqual([{ terminalId: 't1', seq: 2, stop: 'all' }])
    off()
    rt.emitAct({ terminalId: 't1', seq: 3, play: 'nod', times: 1 })
    expect(got).toHaveLength(1)
  })

  it('all arrive, also one whose number came before: the server counts from 0 again after a restart', () => {
    const got: string[] = []
    const off = rt.onAct((act) => got.push(`${act.seq} ${act.play}`))
    rt.emitAct({ terminalId: 't1', seq: 1, play: 'wave', times: 1 })
    // A restart of the same version; the overlay stays mounted and the session greets again.
    rt.emitAct({ terminalId: 't1', seq: 1, play: 'wave', times: 1 })
    expect(got).toEqual(['1 wave', '1 wave'])
    off()
  })

  it('do not stop at a listener that throws', () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {})
    const got: number[] = []
    const offBad = rt.onAct(() => {
      throw new Error('a listener failed')
    })
    const offGood = rt.onAct((act) => got.push(act.seq))
    rt.emitAct({ terminalId: 't1', seq: 4, play: 'wave', times: 1 })
    expect(got).toEqual([4])
    expect(error).toHaveBeenCalledTimes(1)
    error.mockRestore()
    offBad()
    offGood()
  })
})

describe('the input watch', () => {
  it('knows how long ago the tab was last used: page load counts, then every pointer or key input', () => {
    const win = fakeWindow()
    const stop = rt.watchInput(win)
    expect(rt.idleMs(5000)).toBe(5000)
    win.at(4000)
    win.fire('pointermove', {})
    expect(rt.idleMs(5000)).toBe(1000)
    win.at(4600)
    win.fire('keydown')
    expect(rt.idleMs(5000)).toBe(400)
    win.at(4900)
    win.fire('pointerdown')
    expect(rt.idleMs(5000)).toBe(100)
    // Never negative, whatever the clocks say.
    expect(rt.idleMs(1)).toBe(0)
    stop()
    expect(win.count()).toBe(0)
  })

  it('knows the first user gesture: a trusted press of a pointer or a key, not a movement and not a scripted event', () => {
    const win = fakeWindow()
    const stop = rt.watchInput(win)
    expect(rt.hadGesture(win)).toBe(false)
    win.fire('pointermove', { isTrusted: true })
    win.fire('pointerdown', { isTrusted: false })
    win.fire('keydown', {})
    expect(rt.hadGesture(win)).toBe(false)
    win.fire('keydown', { isTrusted: true })
    expect(rt.hadGesture(win)).toBe(true)
    stop()
  })

  it('believes the browser where it can tell that the page was used already', () => {
    expect(rt.hadGesture(fakeWindow({ hasBeenActive: true }))).toBe(true)
    expect(rt.hadGesture(fakeWindow({ hasBeenActive: false }))).toBe(false)
    expect(rt.hadGesture(null)).toBe(false)
  })

  it('calls those who wait for a gesture inside its handler, every time, until they leave', () => {
    const win = fakeWindow()
    const stop = rt.watchInput(win)
    const order: string[] = []
    const off = rt.onGesture(() => order.push(`unlock, gesture known: ${rt.hadGesture(win)}`))
    win.fire('pointermove', { isTrusted: true })
    win.fire('pointerdown', { isTrusted: false })
    expect(order).toEqual([])
    win.fire('pointerdown')
    win.fire('keydown')
    expect(order).toEqual(['unlock, gesture known: true', 'unlock, gesture known: true'])
    off()
    win.fire('pointerdown')
    expect(order).toHaveLength(2)
    stop()
  })

  it('does not let one that throws keep the others from unlocking', () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {})
    const win = fakeWindow()
    const stop = rt.watchInput(win)
    let unlocked = false
    rt.onGesture(() => {
      throw new Error('a listener failed')
    })
    rt.onGesture(() => {
      unlocked = true
    })
    win.fire('pointerdown')
    expect(unlocked).toBe(true)
    expect(error).toHaveBeenCalledTimes(1)
    error.mockRestore()
    stop()
  })
})
