// What the status-bar widget shows and what its menu offers, from the avatar list, the
// project's sessions and this browser's preferences. Nothing in here acts: what an entry
// does goes out through callbacks.

import { Check, CircleStop, PowerOff, Settings, Smile } from 'lucide-react'
import type { TerminalInfo } from '@/api/types'
import type { MenuEntry } from '@/ui'
import type { AvatarList } from './lib/types'
import { STATUS_WORDS, giveEntry, moreSessions, shownTitle, turnOffEntry } from './words'

/** As many sessions are listed in the menu; the rest are counted in one line. */
export const MAX_SESSION_ENTRIES = 12

/** What the widget needs to know of an agent session. */
export interface WidgetSession {
  id: string
  title: string
  /** It has not exited. */
  running: boolean
}

/** The order of the agents column's tabs: pinned first, then as the server lists them. */
function tabOrder(a: TerminalInfo, b: TerminalInfo): number {
  const order = (t: TerminalInfo): number => (typeof t.order === 'number' ? t.order : 0)
  const created = (t: TerminalInfo): number => (typeof t.createdAt === 'number' ? t.createdAt : 0)
  return Number(b.pinned === true) - Number(a.pinned === true) || order(a) - order(b) || created(a) - created(b) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)
}

/** The open agent sessions of the project on screen, in the order of their tabs. */
export function projectSessions(terminals: readonly TerminalInfo[] | undefined, projectId: string | null): WidgetSession[] {
  const here = projectId ?? null
  return (Array.isArray(terminals) ? terminals : [])
    .filter((t) => t?.kind === 'agent' && typeof t.id === 'string' && t.open === true && (t.projectId ?? null) === here)
    .sort(tabOrder)
    .map((t) => ({ id: t.id, title: typeof t.title === 'string' ? t.title : '', running: t.status !== 'exited' }))
}

export interface WidgetInput {
  /** `GET /api/avatar`; undefined while it has not answered. */
  list: AvatarList | undefined
  /** `projectSessions` of the project on screen. */
  sessions: readonly WidgetSession[]
  /** This browser's preferences. */
  show: boolean
  voice: boolean | null
  /** How many avatars this window shows now. */
  shown: number
  /** This tab presents a line now, aloud or as a caption: there is something to stop. */
  presenting: boolean
}

/** One session in the menu: it can be given an avatar, or its avatar can be turned off. */
export interface SessionEntry {
  session: WidgetSession
  action: 'give' | 'off'
  /** It used an avatar tool without having an avatar. */
  asked: boolean
}

/**
 * The menu's entry for each session of the project. A session whose avatar is on can have
 * it turned off; a running one without can be given one (again, if the user turned it
 * off); one that exited without an avatar has no entry. Sessions that asked for an avatar
 * come first. `more`: how many do not fit.
 */
export function sessionEntries(list: AvatarList | undefined, sessions: readonly WidgetSession[]): { entries: SessionEntry[]; more: number } {
  const avatars = list?.avatars
  const askers = list?.asked
  const on = new Set<string>()
  for (const a of Array.isArray(avatars) ? avatars : []) if (a?.on === true && typeof a.terminalId === 'string') on.add(a.terminalId)
  const asked = new Set<string>()
  for (const id of Array.isArray(askers) ? askers : []) if (typeof id === 'string') asked.add(id)
  const first: SessionEntry[] = []
  const rest: SessionEntry[] = []
  for (const session of sessions) {
    if (on.has(session.id)) rest.push({ session, action: 'off', asked: false })
    else if (session.running) (asked.has(session.id) ? first : rest).push({ session, action: 'give', asked: asked.has(session.id) })
  }
  const all = [...first, ...rest]
  return { entries: all.slice(0, MAX_SESSION_ENTRIES), more: Math.max(all.length - MAX_SESSION_ENTRIES, 0) }
}

export interface WidgetView {
  /** The widget is in the status bar at all: avatars are enabled, and there is something to show or to do. */
  visible: boolean
  /** Avatars are hidden in this browser: nothing is shown and nothing is said here. */
  hidden: boolean
  /** How many avatars this window shows. */
  count: number
  /** The crossed speaker: voice is off, or not chosen yet. */
  muted: boolean
  /** The tooltip and the accessible name. */
  title: string
}

/** The face, its number and its crossed speaker. */
export function widgetView(i: WidgetInput): WidgetView {
  const enabled = i.list?.enabled === true
  const hidden = i.show !== true
  const count = hidden ? 0 : Math.max(0, Math.floor(Number.isFinite(i.shown) ? i.shown : 0))
  const { entries } = sessionEntries(i.list, i.sessions)
  const asked = entries.filter((e) => e.asked).length
  return {
    // While avatars are hidden the widget stays: it is where they are shown again.
    visible: enabled && (hidden || count > 0 || entries.length > 0),
    hidden,
    count,
    muted: !hidden && i.voice !== true,
    title: hidden ? STATUS_WORDS.hiddenTitle : shownTitle(count, i.voice === true, asked),
  }
}

export interface WidgetActions {
  stop(): void
  voice(on: boolean): void
  show(on: boolean): void
  give(session: WidgetSession): void
  turnOff(session: WidgetSession): void
  settings(): void
}

/**
 * The widget's menu, in the order of 10.6: Stop speaking; Voice in this browser; Show
 * avatars; one entry for each agent session of the project; Avatar settings.
 */
export function widgetMenu(i: WidgetInput, a: WidgetActions): MenuEntry[] {
  const items: MenuEntry[] = [
    // The same icon as in an avatar's own menu: a plain square would read as an empty check box beside the check marks.
    { label: STATUS_WORDS.stop, icon: CircleStop, disabled: !i.presenting, run: () => a.stop() },
    { label: STATUS_WORDS.voice, icon: i.voice === true ? Check : undefined, run: () => a.voice(i.voice !== true) },
    { label: STATUS_WORDS.show, icon: i.show === true ? Check : undefined, run: () => a.show(i.show !== true) },
  ]
  const { entries, more } = sessionEntries(i.list, i.sessions)
  if (entries.length > 0) {
    items.push('separator')
    for (const e of entries) {
      if (e.action === 'off') items.push({ label: turnOffEntry(e.session.title), icon: PowerOff, run: () => a.turnOff(e.session) })
      else items.push({ label: giveEntry(e.session.title, e.asked), icon: Smile, run: () => a.give(e.session) })
    }
    if (more > 0) items.push({ label: moreSessions(more), disabled: true, run: () => undefined })
  }
  items.push('separator', { label: STATUS_WORDS.settings, icon: Settings, run: () => a.settings() })
  return items
}

/**
 * How high the menu host draws these entries, in CSS px (ui.css: a 26 px row for each
 * item, 9 px for a separator, 4 px of padding and a 1 px border around). The widget sits
 * at the bottom of the window, so its menu is placed above it.
 */
export function menuHeight(items: readonly MenuEntry[]): number {
  let height = 10
  for (const item of items) height += item === 'separator' ? 9 : 26
  return height
}
