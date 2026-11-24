// The avatar's menu: what it offers in each layout, in the order of the contract, and
// what each entry does.

import { describe, expect, it, vi } from 'vitest'
import type { MenuEntry } from '@/ui'
import { DESIGN_NOT_SHOWN } from './Figure'
import { avatarMenu } from './menu'
import type { MenuActions, MenuInput } from './menu'

const input = (over: Partial<MenuInput> = {}): MenuInput => ({ mode: 'wide', presenting: false, voice: false, reduceMotion: false, roam: false, designed: true, rejected: false, moved: false, others: [], ...over })

function actions() {
  const calls: string[] = []
  const a: MenuActions = {
    stop: () => calls.push('stop'),
    voice: (on) => calls.push(`voice ${on}`),
    hide: () => calls.push('hide'),
    defaultLook: () => calls.push('defaultLook'),
    turnOff: () => calls.push('turnOff'),
    move: (corner) => calls.push(`move ${corner}`),
    resetPosition: () => calls.push('reset'),
    reduceMotion: (on) => calls.push(`reduce ${on}`),
    roam: (on) => calls.push(`roam ${on}`),
    settings: () => calls.push('settings'),
    show: (id) => calls.push(`show ${id}`),
  }
  return { a, calls }
}

const labels = (items: MenuEntry[]): string[] => items.map((it) => (it === 'separator' ? '-' : it.label))
const item = (items: MenuEntry[], label: string) => {
  const found = items.find((it) => it !== 'separator' && it.label === label)
  if (!found || found === 'separator') throw new Error(`no entry ${label}`)
  return found
}

describe('avatarMenu', () => {
  it('offers what 10.1 lists, in that order, for the column of figures', () => {
    expect(labels(avatarMenu(input(), actions().a))).toEqual([
      'Stop speaking',
      'Voice in this browser',
      'Hide all avatars',
      '-',
      'Use the default look',
      'Turn off for this session',
      '-',
      'Move to top right',
      'Move to bottom right',
      'Move to bottom left',
      'Move to top left',
      'Reset position',
      '-',
      'Walk around',
      'Reduce motion',
      'Avatar settings',
    ])
  })

  it('has no place to move the column to while the figures walk about, and switches walking on and off', () => {
    const { a, calls } = actions()
    const walking = avatarMenu(input({ roam: true }), a)
    expect(labels(walking)).toEqual(['Stop speaking', 'Voice in this browser', 'Hide all avatars', '-', 'Use the default look', 'Turn off for this session', '-', 'Walk around', 'Reduce motion', 'Avatar settings'])
    expect(item(walking, 'Walk around').icon).toBeDefined()
    item(walking, 'Walk around').run()
    const standing = avatarMenu(input(), a)
    expect(item(standing, 'Walk around').icon).toBeUndefined()
    item(standing, 'Walk around').run()
    expect(calls).toEqual(['roam false', 'roam true'])
    // Reduced motion, the user's or the system's, keeps them in their column: the column can be moved, and walking is not on offer.
    for (const still of [input({ roam: true, reduceMotion: true }), input({ roam: true, systemReduced: true })]) {
      const items = avatarMenu(still, a)
      expect(labels(items)).toContain('Move to top left')
      expect(item(items, 'Walk around').disabled).toBe(true)
    }
    // The compact forms have no walk.
    expect(labels(avatarMenu(input({ mode: 'bar', roam: true }), a))).not.toContain('Walk around')
  })

  it('has nothing to move in the compact forms, and no settings entry on the phone', () => {
    expect(labels(avatarMenu(input({ mode: 'bar' }), actions().a))).toEqual(['Stop speaking', 'Voice in this browser', 'Hide all avatars', '-', 'Use the default look', 'Turn off for this session', '-', 'Reduce motion', 'Avatar settings'])
    expect(labels(avatarMenu(input({ mode: 'phone' }), actions().a))).toEqual(['Stop speaking', 'Voice in this browser', 'Hide all avatars', '-', 'Use the default look', 'Turn off for this session', '-', 'Reduce motion'])
  })

  it('lists the other avatars first where only one is in sight', () => {
    const { a, calls } = actions()
    const others = [
      { id: 't2', label: 'Tock' },
      { id: 't3', label: 'Fix the flaky upload test' },
    ]
    const items = avatarMenu(input({ mode: 'phone', others }), a)
    expect(labels(items).slice(0, 4)).toEqual(['Tock', 'Fix the flaky upload test', '-', 'Stop speaking'])
    item(items, 'Tock').run()
    expect(calls).toEqual(['show t2'])
    // The floating group shows them all: nothing to list.
    expect(labels(avatarMenu(input({ others }), a))[0]).toBe('Stop speaking')
  })

  it('says that a design could not be shown, as an entry that does nothing', () => {
    const items = avatarMenu(input({ rejected: true }), actions().a)
    expect(labels(items).slice(0, 2)).toEqual([DESIGN_NOT_SHOWN, '-'])
    expect(item(items, DESIGN_NOT_SHOWN).disabled).toBe(true)
    expect(item(items, DESIGN_NOT_SHOWN).run()).toBeUndefined()
  })

  it('disables what has nothing to act on', () => {
    const idle = avatarMenu(input({ designed: false }), actions().a)
    expect(item(idle, 'Stop speaking').disabled).toBe(true)
    expect(item(idle, 'Use the default look').disabled).toBe(true)
    expect(item(idle, 'Reset position').disabled).toBe(true)
    const busy = avatarMenu(input({ presenting: true, moved: true }), actions().a)
    expect(item(busy, 'Stop speaking').disabled).toBe(false)
    expect(item(busy, 'Use the default look').disabled).toBe(false)
    expect(item(busy, 'Reset position').disabled).toBe(false)
  })

  it('marks voice and reduced motion while they are on, and toggles them', () => {
    const off = actions()
    const itemsOff = avatarMenu(input({ voice: null, reduceMotion: false }), off.a)
    expect(item(itemsOff, 'Voice in this browser').icon).toBeUndefined()
    expect(item(itemsOff, 'Reduce motion').icon).toBeUndefined()
    item(itemsOff, 'Voice in this browser').run()
    item(itemsOff, 'Reduce motion').run()
    expect(off.calls).toEqual(['voice true', 'reduce true'])

    const on = actions()
    const itemsOn = avatarMenu(input({ voice: true, reduceMotion: true }), on.a)
    expect(item(itemsOn, 'Voice in this browser').icon).toBeDefined()
    expect(item(itemsOn, 'Reduce motion').icon).toBe(item(itemsOn, 'Voice in this browser').icon)
    item(itemsOn, 'Voice in this browser').run()
    item(itemsOn, 'Reduce motion').run()
    expect(on.calls).toEqual(['voice false', 'reduce false'])
  })

  it('does what each entry says', () => {
    const { a, calls } = actions()
    const items = avatarMenu(input({ presenting: true, moved: true }), a)
    for (const label of ['Stop speaking', 'Hide all avatars', 'Use the default look', 'Turn off for this session', 'Move to top right', 'Move to bottom right', 'Move to bottom left', 'Move to top left', 'Reset position', 'Avatar settings']) item(items, label).run()
    expect(calls).toEqual(['stop', 'hide', 'defaultLook', 'turnOff', 'move top-right', 'move bottom-right', 'move bottom-left', 'move top-left', 'reset', 'settings'])
  })

  it('is plain data: no entry runs anything while the menu is built', () => {
    const spy = vi.fn()
    const a = new Proxy({} as MenuActions, { get: () => spy })
    avatarMenu(input({ presenting: true, moved: true, rejected: true, others: [{ id: 'x', label: 'X' }], mode: 'bar' }), a)
    expect(spy).not.toHaveBeenCalled()
  })
})
