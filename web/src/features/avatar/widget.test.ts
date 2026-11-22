// The status-bar widget as a pure function of the avatar list, the project's sessions and
// this browser's preferences: what the face shows, and what its menu offers and does.

import { Check, CircleStop, PowerOff, Settings, Smile } from 'lucide-react'
import { describe, expect, it } from 'vitest'
import type { TerminalInfo } from '@/api/types'
import type { MenuEntry, MenuItem } from '@/ui'
import { whoShows } from './lib/stage'
import type { AvatarInfo, AvatarList } from './lib/types'
import { sessionOf } from './view'
import { MAX_SESSION_ENTRIES, menuHeight, projectSessions, sessionEntries, widgetMenu, widgetView, type WidgetActions, type WidgetInput, type WidgetSession } from './widget'

function term(id: string, over: Partial<TerminalInfo> = {}): TerminalInfo {
  return { id, kind: 'agent', title: `Session ${id}`, projectId: 'shop', cwd: '/w', argv: [], status: 'running', exit: null, createdAt: 1, lastOutputAt: 0, cols: 80, rows: 24, open: true, pinned: false, color: null, order: 1, agent: null, meta: {}, ...over }
}

function avatar(terminalId: string, over: Partial<AvatarInfo> = {}): AvatarInfo {
  return { terminalId, projectId: 'shop', on: true, name: '', designed: false, revision: 0, look: null, voice: {}, via: 'tools', held: null, ask: null, lines: [], createdAt: 1, updatedAt: 1, ...over }
}

const list = (avatars: AvatarInfo[] = [], asked: string[] = [], over: Partial<AvatarList> = {}): AvatarList => ({ enabled: true, sessions: 'asked', avatars, asked, ...over })
const session = (id: string, running = true, title = `Session ${id}`): WidgetSession => ({ id, title, running })
const input = (over: Partial<WidgetInput> = {}): WidgetInput => ({ list: list(), sessions: [], show: true, voice: null, shown: 0, presenting: false, ...over })

function actions() {
  const calls: string[] = []
  const a: WidgetActions = {
    stop: () => calls.push('stop'),
    voice: (on) => calls.push(`voice ${on}`),
    show: (on) => calls.push(`show ${on}`),
    give: (s) => calls.push(`give ${s.id} ${s.title}`),
    turnOff: (s) => calls.push(`off ${s.id}`),
    settings: () => calls.push('settings'),
  }
  return { a, calls }
}

const labels = (items: MenuEntry[]): string[] => items.map((it) => (it === 'separator' ? '-' : it.label))
function item(items: MenuEntry[], label: string): MenuItem {
  const found = items.find((it) => it !== 'separator' && it.label === label)
  if (!found || found === 'separator') throw new Error(`no entry ${label}`)
  return found
}

describe('projectSessions', () => {
  it('lists the open agent sessions of the project on screen', () => {
    const terminals = [
      term('a'),
      term('shell', { kind: 'shell' }),
      term('other', { projectId: 'docs' }),
      term('closed', { open: false }),
      term('gone', { status: 'exited' }),
      term('none', { projectId: null }),
    ]
    expect(projectSessions(terminals, 'shop')).toEqual([
      { id: 'a', title: 'Session a', running: true },
      { id: 'gone', title: 'Session gone', running: false },
    ])
    // Sessions of no project belong to the window that shows no project, and only to it.
    expect(projectSessions(terminals, null).map((s) => s.id)).toEqual(['none'])
    expect(projectSessions(undefined, 'shop')).toEqual([])
    expect(projectSessions([null as unknown as TerminalInfo, term('a', { title: undefined as unknown as string })], 'shop')).toEqual([{ id: 'a', title: '', running: true }])
  })

  it('keeps the order of the agents column’s tabs, which is the order avatars stand in', () => {
    const terminals = [term('c', { order: 3 }), term('a', { order: 2 }), term('p', { order: 9, pinned: true }), term('b', { order: 2, createdAt: 5 }), term('z', { order: 2, createdAt: 5 })]
    const order = projectSessions(terminals, 'shop').map((s) => s.id)
    expect(order).toEqual(['p', 'a', 'b', 'z', 'c'])
    const staged = whoShows({ enabled: true, avatars: terminals.map((t) => avatar(t.id)), sessions: terminals.map(sessionOf), projectId: 'shop', exits: new Map(), now: 0 })
    expect(staged.map((s) => s.terminalId)).toEqual(order)
    // The list given is not reordered in place.
    expect(terminals.map((t) => t.id)).toEqual(['c', 'a', 'p', 'b', 'z'])
  })
})

describe('sessionEntries', () => {
  it('offers to give an avatar to a running session without one, and to turn off one that is on', () => {
    const sessions = [session('a'), session('b'), session('c')]
    const { entries, more } = sessionEntries(list([avatar('b'), avatar('c', { on: false })]), sessions)
    expect(entries.map((e) => [e.session.id, e.action, e.asked])).toEqual([
      ['a', 'give', false],
      ['b', 'off', false],
      // Turned off by the user: it can be given one again (the look was kept).
      ['c', 'give', false],
    ])
    expect(more).toBe(0)
  })

  it('has no entry for a session that exited without an avatar, and keeps the one of a sleeping avatar', () => {
    const { entries } = sessionEntries(list([avatar('b')]), [session('a', false), session('b', false)])
    expect(entries.map((e) => [e.session.id, e.action])).toEqual([['b', 'off']])
  })

  it('puts the sessions that asked for an avatar first, and marks them', () => {
    const sessions = [session('a'), session('b'), session('c'), session('d')]
    const { entries } = sessionEntries(list([avatar('a')], ['d', 'c', 'a', 'gone']), sessions)
    expect(entries.map((e) => [e.session.id, e.action, e.asked])).toEqual([
      ['c', 'give', true],
      ['d', 'give', true],
      // It has one by now: nothing is asked for any more.
      ['a', 'off', false],
      ['b', 'give', false],
    ])
  })

  it('lists a bounded number, and counts the rest', () => {
    const sessions = Array.from({ length: MAX_SESSION_ENTRIES + 5 }, (_, i) => session(`s${i}`))
    const { entries, more } = sessionEntries(list([], [`s${MAX_SESSION_ENTRIES + 4}`]), sessions)
    expect(entries).toHaveLength(MAX_SESSION_ENTRIES)
    expect(more).toBe(5)
    // The one that asked is among those listed, however far back its tab is.
    expect(entries[0].session.id).toBe(`s${MAX_SESSION_ENTRIES + 4}`)
  })

  it('takes a list that has not loaded, or is not what it should be, as no avatars', () => {
    expect(sessionEntries(undefined, [session('a')]).entries.map((e) => e.action)).toEqual(['give'])
    const odd = { enabled: true, sessions: 'asked', avatars: 'none', asked: { a: true } } as unknown as AvatarList
    expect(sessionEntries(odd, [session('a')]).entries.map((e) => [e.action, e.asked])).toEqual([['give', false]])
    const holes = list([null as unknown as AvatarInfo, avatar('a', { on: 'yes' as unknown as boolean })], [7 as unknown as string])
    expect(sessionEntries(holes, [session('a')]).entries.map((e) => e.action)).toEqual(['give'])
  })
})

describe('widgetView', () => {
  it('is not in the status bar while avatars are switched off, or the list is not known', () => {
    const busy = { sessions: [session('a')], shown: 1 }
    expect(widgetView(input({ ...busy, list: undefined })).visible).toBe(false)
    expect(widgetView(input({ ...busy, list: list([avatar('a')], [], { enabled: false }) })).visible).toBe(false)
    expect(widgetView(input({ ...busy, list: list([avatar('a')], [], { enabled: 'yes' as unknown as boolean }) })).visible).toBe(false)
    expect(widgetView(input({ ...busy, list: list([avatar('a')]) })).visible).toBe(true)
  })

  it('is there while there is an avatar to show or a session to give one, and not otherwise', () => {
    // Nothing to show and nobody to give one: no widget.
    expect(widgetView(input()).visible).toBe(false)
    expect(widgetView(input({ sessions: [session('a', false)] })).visible).toBe(false)
    // A running session can be given one.
    expect(widgetView(input({ sessions: [session('a')] }))).toMatchObject({ visible: true, hidden: false, count: 0 })
    // An avatar is shown (another project's, here for its question, counts too).
    expect(widgetView(input({ shown: 1 }))).toMatchObject({ visible: true, count: 1 })
    // A sleeping avatar that has left the stage can still be turned off.
    expect(widgetView(input({ list: list([avatar('a')]), sessions: [session('a', false)] }))).toMatchObject({ visible: true, count: 0 })
  })

  it('shows the number of avatars shown, and a crossed speaker while voice is off or not chosen', () => {
    const three = { list: list([avatar('a'), avatar('b'), avatar('c')]), sessions: [session('a'), session('b'), session('c')], shown: 3 }
    expect(widgetView(input({ ...three, voice: null }))).toEqual({ visible: true, hidden: false, count: 3, muted: true, title: '3 avatars shown · voice off' })
    expect(widgetView(input({ ...three, voice: false }))).toMatchObject({ count: 3, muted: true, title: '3 avatars shown · voice off' })
    expect(widgetView(input({ ...three, voice: true }))).toMatchObject({ count: 3, muted: false, title: '3 avatars shown · voice on' })
    expect(widgetView(input({ ...three, shown: 1, voice: true })).title).toBe('1 avatar shown · voice on')
    expect(widgetView(input({ sessions: [session('a')], voice: true }))).toMatchObject({ count: 0, muted: false, title: 'No avatar shown · voice on' })
    // A count that is no count is none.
    expect(widgetView(input({ ...three, shown: Number.NaN })).count).toBe(0)
    expect(widgetView(input({ ...three, shown: 2.9 })).count).toBe(2)
  })

  it('says in its tooltip that sessions asked for an avatar', () => {
    const view = widgetView(input({ list: list([], ['a', 'b']), sessions: [session('a'), session('b'), session('c')] }))
    expect(view.title).toBe('No avatar shown · voice off · 2 sessions ask for one')
  })

  it('shows that avatars are hidden: hidden means silent, so no number and no speaker', () => {
    const view = widgetView(input({ list: list([avatar('a')]), sessions: [session('a')], show: false, voice: true, shown: 0 }))
    expect(view).toEqual({ visible: true, hidden: true, count: 0, muted: false, title: 'Avatars are hidden in this browser: nothing is shown or said here' })
    // Whatever the window is told meanwhile: hidden shows no count.
    expect(widgetView(input({ show: false, shown: 3 }))).toMatchObject({ hidden: true, count: 0 })
    // It stays in the status bar with nothing else to do: this is where avatars are shown again.
    expect(widgetView(input({ show: false })).visible).toBe(true)
    expect(widgetView(input({ show: false, list: list([], [], { enabled: false }) })).visible).toBe(false)
  })
})

describe('widgetMenu', () => {
  it('offers what 10.6 lists, in that order', () => {
    const i = input({ list: list([avatar('b')], ['c']), sessions: [session('a', true, 'Fix the tests'), session('b', true, 'Write the docs'), session('c', true, 'Review')] })
    expect(labels(widgetMenu(i, actions().a))).toEqual([
      'Stop speaking',
      'Voice in this browser',
      'Show avatars',
      '-',
      'Review: give an avatar (asked)',
      'Fix the tests: give an avatar',
      'Write the docs: turn avatar off',
      '-',
      'Avatar settings',
    ])
  })

  it('has no session part without sessions', () => {
    expect(labels(widgetMenu(input(), actions().a))).toEqual(['Stop speaking', 'Voice in this browser', 'Show avatars', '-', 'Avatar settings'])
  })

  it('does what each entry says', () => {
    const { a, calls } = actions()
    const i = input({ list: list([avatar('b')]), sessions: [session('a', true, 'Fix the tests'), session('b', true, 'Write the docs')], presenting: true })
    const menu = widgetMenu(i, a)
    for (const label of ['Stop speaking', 'Voice in this browser', 'Show avatars', 'Fix the tests: give an avatar', 'Write the docs: turn avatar off', 'Avatar settings']) item(menu, label).run()
    // Voice was not on: it is turned on. Avatars were shown: they are hidden.
    expect(calls).toEqual(['stop', 'voice true', 'show false', 'give a Fix the tests', 'off b', 'settings'])
  })

  it('turns voice and the avatars the other way from where they are', () => {
    const { a, calls } = actions()
    item(widgetMenu(input({ voice: true, show: false }), a), 'Voice in this browser').run()
    item(widgetMenu(input({ voice: true, show: false }), a), 'Show avatars').run()
    item(widgetMenu(input({ voice: false }), a), 'Voice in this browser').run()
    item(widgetMenu(input({ voice: null }), a), 'Voice in this browser').run()
    expect(calls).toEqual(['voice false', 'show true', 'voice true', 'voice true'])
  })

  it('marks what is on with a check, and has nothing to stop while nothing is presented', () => {
    const off = widgetMenu(input({ voice: null, show: false }), actions().a)
    expect(item(off, 'Voice in this browser').icon).toBeUndefined()
    expect(item(off, 'Show avatars').icon).toBeUndefined()
    expect(item(off, 'Stop speaking').disabled).toBe(true)
    const on = widgetMenu(input({ voice: true, show: true, presenting: true }), actions().a)
    expect(item(on, 'Voice in this browser').icon).toBe(Check)
    expect(item(on, 'Show avatars').icon).toBe(Check)
    expect(item(on, 'Stop speaking').disabled).toBe(false)
    expect(item(on, 'Stop speaking').icon).toBe(CircleStop)
    expect(item(on, 'Avatar settings').icon).toBe(Settings)
    // Voice that is off is not checked: only a plain yes is.
    expect(item(widgetMenu(input({ voice: false }), actions().a), 'Voice in this browser').icon).toBeUndefined()
  })

  it('gives each session entry the icon of what it does', () => {
    const menu = widgetMenu(input({ list: list([avatar('b')]), sessions: [session('a', true, 'A'), session('b', true, 'B')] }), actions().a)
    expect(item(menu, 'A: give an avatar').icon).toBe(Smile)
    expect(item(menu, 'B: turn avatar off').icon).toBe(PowerOff)
  })

  it('names a session by its title as text, cut so that what the entry does stays in sight', () => {
    const long = 'Refactor the importer so that it can take the new export format of the warehouse'
    const hostile = '<img src=x onerror=alert(1)> **bold**'
    const menu = widgetMenu(input({ sessions: [session('a', true, long), session('b', true, hostile), session('c', true, '')] }), actions().a)
    expect(labels(menu).slice(4, 7)).toEqual(['Refactor the importer so tha…: give an avatar', '<img src=x onerror=alert(1)>…: give an avatar', 'Untitled session: give an avatar'])
    // A label is a plain string: the menu host prints it as a text node.
    for (const it of menu) if (it !== 'separator') expect(typeof it.label).toBe('string')
  })

  it('counts the sessions that do not fit in one line that does nothing', () => {
    const sessions = Array.from({ length: MAX_SESSION_ENTRIES + 3 }, (_, n) => session(`s${n}`, true, `S${n}`))
    const { a, calls } = actions()
    const menu = widgetMenu(input({ sessions }), a)
    expect(labels(menu)).toHaveLength(3 + 1 + MAX_SESSION_ENTRIES + 1 + 1 + 1)
    const more = item(menu, '3 more sessions')
    expect(more.disabled).toBe(true)
    more.run()
    expect(calls).toEqual([])
  })

  it('knows how high the menu host draws it, to sit above the status bar', () => {
    expect(menuHeight([])).toBe(10)
    const menu = widgetMenu(input({ sessions: [session('a'), session('b')] }), actions().a)
    // Six rows of 26 px, two separators of 9 px, 4 px of padding and 1 px of border around.
    expect(labels(menu).filter((l) => l === '-')).toHaveLength(2)
    expect(menuHeight(menu)).toBe(6 * 26 + 2 * 9 + 10)
  })
})
