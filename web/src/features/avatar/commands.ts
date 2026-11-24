// The palette commands of the avatar feature (desktop). Each does exactly what its name
// says, and is listed only while it has something to act on: `commandPlan` decides that
// from the avatar list, the terminals, the session on screen and what is on stage, and
// `avatarCommands` reads those from where they live when the palette asks.

import { CircleStop, Eye, PowerOff, Reply, Settings, Smile, Volume2 } from 'lucide-react'
import type { TerminalInfo } from '@/api/types'
import { useAgentsUi } from '@/features/agents/store'
import type { Command, CommandContext } from '@/shell/types'
import { giveAvatar, openAvatarSettings, replyTo, stopSpeaking, toggleShow, toggleVoice, turnAvatarOff } from './actions'
import { front } from './lib/stage'
import type { AvatarList } from './lib/types'
import { cachedAvatars, cachedTerminals } from './queryAccess'
import { useAvatarRuntime } from './runtime'
import { sessionView } from './view'
import { projectSessions, type WidgetSession } from './widget'
import { COMMAND_WORDS, called, replyCommand } from './words'

export interface CommandInput {
  /** `GET /api/avatar`; undefined while it has not answered. */
  list: AvatarList | undefined
  terminals: readonly TerminalInfo[] | undefined
  /** The project on screen. */
  projectId: string | null
  /** The session on screen: the selected tab of the agents column, or null for its home tab. */
  onScreen: string | null
  /** The avatars this window shows, in order (the runtime store's `shown`). */
  shown: readonly string[]
  /** The active avatar, if one is. */
  active: string | null
  /** This tab presents a line now. */
  presenting: boolean
}

export interface CommandPlan {
  /** Avatars are enabled on the server: without that only the settings are offered. */
  enabled: boolean
  /** There is a line to stop. */
  stop: boolean
  /** Whose reply box "Reply to …" opens, and what it is called: the active avatar, else one that waits with a question, else the session on screen's, else the first. */
  reply: { id: string; called: string } | null
  /** "This session", when it can be given an avatar: the session on screen runs and has none that is on. */
  give: WidgetSession | null
  /** "This session", when its avatar is on. */
  turnOff: WidgetSession | null
}

const NOTHING: CommandPlan = Object.freeze({ enabled: false, stop: false, reply: null, give: null, turnOff: null })

/** Which commands have something to act on, and on what. */
export function commandPlan(i: CommandInput): CommandPlan {
  const list = i.list
  if (list?.enabled !== true) return NOTHING
  const records = new Map<string, AvatarList['avatars'][number]>()
  for (const a of Array.isArray(list.avatars) ? list.avatars : []) if (typeof a?.terminalId === 'string') records.set(a.terminalId, a)
  const terminals = new Map<string, TerminalInfo>()
  for (const t of Array.isArray(i.terminals) ? i.terminals : []) if (typeof t?.id === 'string') terminals.set(t.id, t)

  // Whom a reply without a name is for: as the overlay puts one avatar in front.
  const shown = i.shown.filter((id) => records.has(id) && terminals.has(id))
  const waiting = shown.filter((id) => {
    const t = terminals.get(id)
    return records.get(id)?.ask != null && t !== undefined && !sessionView(t).blocked
  })
  const target = front(shown, i.active, i.onScreen, waiting)
  const record = target === null ? undefined : records.get(target)
  const reply = target !== null && record ? { id: target, called: called(typeof record.name === 'string' ? record.name : '', terminals.get(target)?.title ?? '') } : null

  // "This session" is the one whose tab is selected in the agents column.
  const here = i.onScreen === null ? undefined : projectSessions(i.terminals, i.projectId).find((s) => s.id === i.onScreen)
  const on = here !== undefined && records.get(here.id)?.on === true
  return {
    enabled: true,
    stop: i.presenting === true,
    reply,
    give: here !== undefined && !on && here.running ? here : null,
    turnOff: here !== undefined && on ? here : null,
  }
}

/** An entry of a record keyed by a project id: its own entries only. */
const own = <V,>(record: Readonly<Record<string, V>>, key: string): V | undefined => (Object.hasOwn(record, key) ? record[key] : undefined)

/** What the palette lists for the avatar feature. Read when the palette opens, so a title can name the avatar it is about. */
export function avatarCommands(ctx: CommandContext): Command[] {
  const runtime = useAvatarRuntime.getState()
  const plan = commandPlan({
    list: cachedAvatars(),
    terminals: cachedTerminals(),
    projectId: ctx.projectId,
    onScreen: own(useAgentsUi.getState().columnTab, ctx.projectId ?? '') ?? null,
    shown: runtime.shown,
    active: runtime.active,
    presenting: runtime.presenting !== null,
  })
  const group = COMMAND_WORDS.group
  const { reply, give, turnOff } = plan
  return [
    { id: 'avatar.show', title: COMMAND_WORDS.show, group, icon: Eye, keywords: ['avatars', 'character', 'overlay', 'visible'], when: () => plan.enabled, run: () => toggleShow() },
    { id: 'avatar.voice', title: COMMAND_WORDS.voice, group, icon: Volume2, keywords: ['speech', 'speak', 'sound', 'mute', 'captions', 'aloud'], when: () => plan.enabled, run: () => toggleVoice() },
    { id: 'avatar.stop', title: COMMAND_WORDS.stop, group, icon: CircleStop, keywords: ['quiet', 'silence', 'skip', 'mute'], when: () => plan.stop, run: () => void stopSpeaking() },
    {
      id: 'avatar.reply',
      title: replyCommand(reply?.called ?? ''),
      group,
      icon: Reply,
      keywords: ['answer', 'talk', 'message', 'say', 'write'],
      when: () => reply !== null,
      run: () => {
        if (reply) replyTo(reply.id)
      },
    },
    {
      id: 'avatar.give',
      title: COMMAND_WORDS.give,
      group,
      icon: Smile,
      keywords: ['add', 'character', 'turn on', 'enable'],
      when: () => give !== null,
      run: async () => {
        if (give) await giveAvatar(give)
      },
    },
    {
      id: 'avatar.off',
      title: COMMAND_WORDS.turnOff,
      group,
      icon: PowerOff,
      keywords: ['remove', 'disable', 'stop', 'hide'],
      when: () => turnOff !== null,
      run: async () => {
        if (turnOff) await turnAvatarOff(turnOff)
      },
    },
    { id: 'avatar.settings', title: COMMAND_WORDS.settings, group, icon: Settings, keywords: ['preferences', 'voice', 'speed', 'volume', 'motion', 'online voices'], run: () => openAvatarSettings() },
  ]
}
