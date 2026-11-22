// The palette commands: when each one is listed, what it is called, and that it does the
// one thing its name says.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { qk } from '@/api/queries'
import type { AgentInfo, TerminalInfo } from '@/api/types'
import { useAgentsUi } from '@/features/agents/store'
import type { Command, CommandContext } from '@/shell/types'
import { avatarKeys } from './api'
import { avatarCommands, commandPlan, type CommandInput } from './commands'
import feature from './index'
import type { Ask, AvatarInfo, AvatarList } from './lib/types'
import { setQueryClient } from './queryAccess'
import { useAvatarRuntime } from './runtime'

/** What the commands did, as the stand-in actions noted it. (Hoisted: the stand-ins are made before this file's own lines run.) */
const done = vi.hoisted(() => [] as string[])
vi.mock('./actions', () => ({
  toggleShow: () => void done.push('toggle show'),
  toggleVoice: () => void done.push('toggle voice'),
  stopSpeaking: () => {
    done.push('stop')
    return true
  },
  replyTo: (id: string) => {
    done.push(`reply ${id}`)
    return true
  },
  giveAvatar: async (s: { id: string; title: string }) => void done.push(`give ${s.id} ${s.title}`),
  turnAvatarOff: async (s: { id: string }) => void done.push(`off ${s.id}`),
  openAvatarSettings: () => void done.push('settings'),
  // The status-bar widget's own two (the feature module is loaded here too): no command uses them.
  setShow: (on: boolean) => void done.push(`show ${on}`),
  setVoice: (on: boolean) => void done.push(`voice ${on}`),
}))

function agent(over: Partial<AgentInfo> = {}): AgentInfo {
  return { sessionId: 's', provider: 'claude', providerId: 'claude', state: 'idle', unread: false, model: null, effort: null, permissionMode: null, remoteControl: false, remoteUrl: null, title: null, lastMessage: null, attention: null, contextPct: null, costUsd: null, usage: null, priced: false, lastEventAt: 0, pendingPermission: null, resumeCost: null, artifacts: [], ...over }
}

function term(id: string, over: Partial<TerminalInfo> = {}, a: Partial<AgentInfo> = {}): TerminalInfo {
  return { id, kind: 'agent', title: `Session ${id}`, projectId: 'shop', cwd: '/w', argv: [], status: 'running', exit: null, createdAt: 1, lastOutputAt: 0, cols: 80, rows: 24, open: true, pinned: false, color: null, order: 1, agent: agent(a), meta: {}, ...over }
}

function avatar(terminalId: string, over: Partial<AvatarInfo> = {}): AvatarInfo {
  return { terminalId, projectId: 'shop', on: true, name: '', designed: false, revision: 0, look: null, voice: {}, via: 'tools', held: null, ask: null, lines: [], createdAt: 1, updatedAt: 1, ...over }
}

const ask = (id: string): Ask => ({ id, lineId: `L-${id}`, text: 'Which one?', choices: [], freeText: true, askedAt: 1, waiting: true })
const list = (avatars: AvatarInfo[] = [], over: Partial<AvatarList> = {}): AvatarList => ({ enabled: true, sessions: 'asked', avatars, asked: [], ...over })
const input = (over: Partial<CommandInput> = {}): CommandInput => ({ list: list(), terminals: [], projectId: 'shop', onScreen: null, shown: [], active: null, presenting: false, ...over })

describe('commandPlan', () => {
  it('offers nothing but the settings while avatars are switched off, or the list is not known', () => {
    const busy = { terminals: [term('a')], onScreen: 'a', shown: ['a'], active: 'a', presenting: true }
    const nothing = { enabled: false, stop: false, reply: null, give: null, turnOff: null }
    expect(commandPlan(input({ ...busy, list: undefined }))).toEqual(nothing)
    expect(commandPlan(input({ ...busy, list: list([avatar('a')], { enabled: false }) }))).toEqual(nothing)
    expect(commandPlan(input({ ...busy, list: list([avatar('a')], { enabled: 1 as unknown as boolean }) }))).toEqual(nothing)
    expect(commandPlan(input({ ...busy, list: list([avatar('a')]) })).enabled).toBe(true)
  })

  it('has a line to stop only while this tab presents one', () => {
    expect(commandPlan(input()).stop).toBe(false)
    expect(commandPlan(input({ presenting: true })).stop).toBe(true)
  })

  describe('"this session" is the one whose tab is selected', () => {
    const terminals = [term('a'), term('b'), term('sh', { kind: 'shell', agent: null }), term('other', { projectId: 'docs' }), term('gone', { status: 'exited' }), term('closed', { open: false })]
    const of = (onScreen: string | null, avatars: AvatarInfo[] = []) => {
      const p = commandPlan(input({ terminals, onScreen, list: list(avatars) }))
      return [p.give?.id ?? null, p.turnOff?.id ?? null]
    }

    it('can be given an avatar while it runs and has none that is on', () => {
      expect(of('a')).toEqual(['a', null])
      // Another session's avatar says nothing about this one.
      expect(of('a', [avatar('b')])).toEqual(['a', null])
      // Turned off by the user: it can be given one again.
      expect(of('a', [avatar('a', { on: false })])).toEqual(['a', null])
      // The title travels with it, for what is told afterwards.
      expect(commandPlan(input({ terminals, onScreen: 'a' })).give).toEqual({ id: 'a', title: 'Session a', running: true })
    })

    it('can have its avatar turned off while that is on, also after it exited', () => {
      expect(of('a', [avatar('a')])).toEqual([null, 'a'])
      expect(of('gone', [avatar('gone')])).toEqual([null, 'gone'])
    })

    it('is nobody on the home tab, for a shell, for another project’s session, a closed tab or an unknown id', () => {
      for (const id of [null, 'sh', 'other', 'closed', 'nowhere']) expect(of(id, [avatar('other'), avatar('closed')]), String(id)).toEqual([null, null])
      // A session that exited without an avatar has nothing to be given.
      expect(of('gone')).toEqual([null, null])
    })
  })

  describe('the reply goes to the avatar in front', () => {
    const terminals = [term('a', { title: 'Fix the tests' }), term('b', { title: 'Write the docs' }), term('c', { title: 'Review' })]
    const avatars = [avatar('a'), avatar('b', { name: 'Pip' }), avatar('c')]
    const reply = (over: Partial<CommandInput>) => commandPlan(input({ terminals, list: list(avatars), shown: ['a', 'b', 'c'], ...over })).reply

    it('is nobody while nothing is on stage', () => {
      expect(reply({ shown: [] })).toBeNull()
      // An id the window is told of, but without its record or its session: nobody to reply to.
      expect(reply({ shown: ['zz'] })).toBeNull()
      expect(reply({ shown: ['a'], terminals: [] })).toBeNull()
    })

    it('is the active one first, called by its own name or its session’s title', () => {
      expect(reply({ active: 'b' })).toEqual({ id: 'b', called: 'Pip' })
      expect(reply({ active: 'c', onScreen: 'a' })).toEqual({ id: 'c', called: 'Review' })
    })

    it('else one that waits with a question, the session on screen’s before others', () => {
      const asking = [avatar('a'), avatar('b', { name: 'Pip', ask: ask('Q1') }), avatar('c', { ask: ask('Q2') })]
      expect(reply({ list: list(asking) })).toEqual({ id: 'b', called: 'Pip' })
      expect(reply({ list: list(asking), onScreen: 'c' })).toEqual({ id: 'c', called: 'Review' })
      expect(reply({ list: list(asking), onScreen: 'a' })).toEqual({ id: 'b', called: 'Pip' })
    })

    it('does not count a question of a session that needs a permission or an answer first', () => {
      const asking = [avatar('a'), avatar('b', { name: 'Pip', ask: ask('Q1') }), avatar('c')]
      const blocked = [terminals[0], term('b', { title: 'Write the docs' }, { state: 'needs_permission' }), terminals[2]]
      expect(reply({ list: list(asking), terminals: blocked })).toEqual({ id: 'a', called: 'Fix the tests' })
    })

    it('else the session on screen’s, else the first', () => {
      expect(reply({ onScreen: 'c' })).toEqual({ id: 'c', called: 'Review' })
      expect(reply({ onScreen: null })).toEqual({ id: 'a', called: 'Fix the tests' })
      expect(reply({ onScreen: 'not-on-stage' })).toEqual({ id: 'a', called: 'Fix the tests' })
      // An active id that left the stage counts for nothing.
      expect(reply({ active: 'left', onScreen: 'b' })).toEqual({ id: 'b', called: 'Pip' })
    })

    it('cuts a long title to a few words', () => {
      const long = [term('a', { title: 'Refactor the importer so that it can take the new export format' })]
      expect(commandPlan(input({ terminals: long, list: list([avatar('a')]), shown: ['a'] })).reply).toEqual({ id: 'a', called: 'Refactor the importer so that i…' })
    })
  })
})

describe('avatarCommands', () => {
  const ctx: CommandContext = { projectId: 'shop', project: null }
  let cache: Map<string, unknown>

  /** The commands the palette would list: the shell drops those whose `when` says no. */
  const listed = (): Command[] => avatarCommands(ctx).filter((c) => !c.when || c.when(ctx))
  const titles = (): string[] => listed().map((c) => c.title)
  const byId = (id: string): Command => {
    const found = avatarCommands(ctx).find((c) => c.id === id)
    if (!found) throw new Error(`no command ${id}`)
    return found
  }

  beforeEach(() => {
    done.length = 0
    cache = new Map()
    setQueryClient({ getQueryData: ((key: readonly unknown[]) => cache.get(JSON.stringify(key))) as never })
    useAgentsUi.setState({ columnTab: {} })
    useAvatarRuntime.getState().tell({ shown: [], staged: [], active: null, open: null, presenting: null, locked: false })
  })
  afterEach(() => setQueryClient(null))

  const stage = (avatars: AvatarInfo[], terminals: TerminalInfo[], over: Partial<AvatarList> = {}) => {
    cache.set(JSON.stringify(avatarKeys.all), list(avatars, over))
    cache.set(JSON.stringify(qk.terminals), terminals)
  }

  it('is what the feature module gives the palette', () => {
    expect(feature.commands).toBe(avatarCommands)
  })

  it('lists only the settings before anything is known, or while avatars are switched off', () => {
    expect(titles()).toEqual(['Avatar Settings'])
    stage([avatar('a')], [term('a')], { enabled: false })
    useAgentsUi.setState({ columnTab: { shop: 'a' } })
    useAvatarRuntime.getState().tell({ shown: ['a'], presenting: { terminalId: 'a', lineId: 'L1', aloud: true } })
    expect(titles()).toEqual(['Avatar Settings'])
  })

  it('lists showing and voice whenever avatars are enabled, and the rest only when there is something to act on', () => {
    stage([], [])
    expect(titles()).toEqual(['Avatar: Show or Hide Avatars', 'Avatar: Voice On or Off', 'Avatar Settings'])
  })

  it('lists every command of 10.6 when each has something to act on, under one group and without a shortcut', () => {
    // Session a is on screen without an avatar; b has one, is on stage and presents a line.
    stage([avatar('b', { name: 'Pip' })], [term('a'), term('b')])
    useAgentsUi.setState({ columnTab: { shop: 'a' } })
    useAvatarRuntime.getState().tell({ shown: ['b'], active: 'b', presenting: { terminalId: 'b', lineId: 'L1', aloud: false } })
    expect(titles()).toEqual(['Avatar: Show or Hide Avatars', 'Avatar: Voice On or Off', 'Avatar: Stop Speaking', 'Avatar: Reply to Pip…', 'Avatar: Give This Session an Avatar', 'Avatar Settings'])
    // The session on screen is b now: its avatar can be turned off, and nothing is left to give.
    useAgentsUi.setState({ columnTab: { shop: 'b' } })
    expect(titles()).toEqual(['Avatar: Show or Hide Avatars', 'Avatar: Voice On or Off', 'Avatar: Stop Speaking', 'Avatar: Reply to Pip…', 'Avatar: Turn Off for This Session', 'Avatar Settings'])
    const all = avatarCommands(ctx)
    expect(all.map((c) => c.id)).toEqual(['avatar.show', 'avatar.voice', 'avatar.stop', 'avatar.reply', 'avatar.give', 'avatar.off', 'avatar.settings'])
    for (const c of all) {
      expect(c.group).toBe('Avatar')
      expect(c.shortcut).toBeUndefined()
      expect(c.global).toBeUndefined()
      expect(c.icon).toBeDefined()
    }
    expect(new Set(all.map((c) => c.title)).size).toBe(all.length)
  })

  it('reads the tab of the project on screen, not another project’s', () => {
    stage([], [term('a'), term('x', { projectId: 'docs' })])
    useAgentsUi.setState({ columnTab: { docs: 'x', shop: null } })
    expect(titles()).not.toContain('Avatar: Give This Session an Avatar')
    useAgentsUi.setState({ columnTab: { docs: 'x', shop: 'a' } })
    expect(titles()).toContain('Avatar: Give This Session an Avatar')
    // A project id is data: it never reaches what an object inherits.
    expect(avatarCommands({ projectId: 'constructor', project: null }).find((c) => c.id === 'avatar.give')?.when?.(ctx)).toBe(false)
    expect(avatarCommands({ projectId: null, project: null }).find((c) => c.id === 'avatar.give')?.when?.(ctx)).toBe(false)
  })

  it('does exactly what each name says', async () => {
    stage([avatar('b', { name: 'Pip' })], [term('a', { title: 'Fix the tests' }), term('b')])
    useAgentsUi.setState({ columnTab: { shop: 'a' } })
    useAvatarRuntime.getState().tell({ shown: ['b'], active: 'b', presenting: { terminalId: 'b', lineId: 'L1', aloud: true } })
    for (const id of ['avatar.show', 'avatar.voice', 'avatar.stop', 'avatar.reply', 'avatar.give', 'avatar.settings']) await byId(id).run(ctx)
    expect(done).toEqual(['toggle show', 'toggle voice', 'stop', 'reply b', 'give a Fix the tests', 'settings'])
    done.length = 0
    useAgentsUi.setState({ columnTab: { shop: 'b' } })
    await byId('avatar.off').run(ctx)
    expect(done).toEqual(['off b'])
  })

  it('does nothing when run although it is not listed', async () => {
    stage([], [term('a')])
    for (const id of ['avatar.reply', 'avatar.give', 'avatar.off']) {
      expect(byId(id).when?.(ctx)).toBe(false)
      await byId(id).run(ctx)
    }
    expect(done).toEqual([])
  })

  it('names the avatar a reply goes to, and replies to that one', async () => {
    stage([avatar('a'), avatar('b', { name: 'Pip', ask: ask('Q1') })], [term('a', { title: 'Fix the tests' }), term('b')])
    useAvatarRuntime.getState().tell({ shown: ['a', 'b'] })
    // Pip waits with a question: the reply is for it, although a comes first.
    expect(byId('avatar.reply').title).toBe('Avatar: Reply to Pip…')
    await byId('avatar.reply').run(ctx)
    expect(done).toEqual(['reply b'])
    // The user turned to a: it is the active one.
    useAvatarRuntime.getState().tell({ active: 'a' })
    expect(byId('avatar.reply').title).toBe('Avatar: Reply to Fix the tests…')
  })

  it('is the only command of the feature that opens a reply box', async () => {
    stage([avatar('b', { ask: ask('Q1') })], [term('a'), term('b')])
    useAgentsUi.setState({ columnTab: { shop: 'b' } })
    useAvatarRuntime.getState().tell({ shown: ['b'], active: 'b', presenting: { terminalId: 'b', lineId: 'L1', aloud: true } })
    for (const c of avatarCommands(ctx)) if (c.id !== 'avatar.reply') await c.run(ctx)
    expect(done.filter((d) => d.startsWith('reply'))).toEqual([])
  })
})
