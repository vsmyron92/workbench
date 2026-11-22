// The overlay's view of a session: taken from the terminals list with the agents
// feature's own words, and the same object while nothing in it changes.

import { describe, expect, it } from 'vitest'
import type { AgentInfo, AgentState, PendingPermission, TerminalInfo } from '@/api/types'
import { sessionOf, sessionView, sessionViews } from './view'
import type { SessionView } from './view'

const agent = (state: AgentState, over: Partial<AgentInfo> = {}): AgentInfo => ({
  sessionId: 's',
  provider: 'claude',
  providerId: null,
  state,
  unread: false,
  model: null,
  effort: null,
  permissionMode: null,
  remoteControl: false,
  remoteUrl: null,
  title: null,
  lastMessage: null,
  attention: null,
  contextPct: null,
  costUsd: null,
  usage: null,
  priced: false,
  lastEventAt: 0,
  pendingPermission: null,
  resumeCost: null,
  artifacts: [],
  ...over,
})

const term = (id: string, state: AgentState = 'idle', over: Partial<TerminalInfo> = {}, agentOver: Partial<AgentInfo> = {}): TerminalInfo => ({
  id,
  kind: 'agent',
  title: `Session ${id}`,
  projectId: 'p1',
  cwd: '/',
  argv: [],
  status: 'running',
  exit: null,
  createdAt: 1000,
  lastOutputAt: 0,
  cols: 80,
  rows: 24,
  open: true,
  pinned: false,
  color: null,
  order: 0,
  agent: agent(state, agentOver),
  meta: {},
  ...over,
})

const permission: PendingPermission = { id: 'r1', tool: 'Bash', summary: 'Permission to run `ls`', since: 1 }

describe('sessionView', () => {
  it('uses the tab’s own tone and words', () => {
    expect(sessionView(term('a', 'working'))).toMatchObject({ id: 'a', projectId: 'p1', title: 'Session a', provider: 'claude', tone: 'working', label: 'Working', working: true, running: true, blocked: false })
    expect(sessionView(term('a', 'idle', {}, { unread: true }))).toMatchObject({ tone: 'unread', label: 'Done' })
    expect(sessionView(term('a', 'error'))).toMatchObject({ tone: 'error', label: 'Error' })
    expect(sessionView(term('a', 'idle', { status: 'exited', exit: { code: 0, signal: null, at: 5 } }))).toMatchObject({ tone: 'exited', label: 'Exited', running: false })
  })

  it('is blocked while the session needs a permission or an answer, and says why nothing can be pasted', () => {
    expect(sessionView(term('a', 'needs_permission'))).toMatchObject({ blocked: true, refuses: 'dialog', pending: false })
    expect(sessionView(term('a', 'needs_input'))).toMatchObject({ blocked: true, refuses: 'dialog' })
    expect(sessionView(term('a', 'starting'))).toMatchObject({ blocked: false, refuses: 'starting' })
    expect(sessionView(term('a', 'working'))).toMatchObject({ blocked: false, refuses: null })
    // An exited session shows no dialog any more.
    expect(sessionView(term('a', 'needs_permission', { status: 'exited' }))).toMatchObject({ blocked: false, refuses: null })
  })

  it('knows a permission request Workbench can answer', () => {
    expect(sessionView(term('a', 'needs_permission', {}, { pendingPermission: permission })).pending).toBe(true)
    expect(sessionView(term('a', 'needs_permission', { status: 'exited' }, { pendingPermission: permission })).pending).toBe(false)
  })

  it('knows a session that was started with the Avatar option, strictly', () => {
    expect(sessionView(term('a', 'idle', { meta: { avatar: true } })).withOption).toBe(true)
    expect(sessionView(term('a', 'idle', { meta: { avatar: 'true' } })).withOption).toBe(false)
    expect(sessionView(term('a')).withOption).toBe(false)
  })

  it('takes a provider it does not know for a custom one', () => {
    expect(sessionView(term('a', 'idle', {}, { provider: 'hal' as never })).provider).toBe('custom')
    expect(sessionView(term('a', 'idle', { agent: null })).provider).toBe('custom')
  })
})

describe('sessionViews', () => {
  it('holds the agent sessions only, by id', () => {
    const views = sessionViews(new Map(), [term('a'), term('sh', 'idle', { kind: 'shell', agent: null }), term('b', 'working')])
    expect([...views.keys()]).toEqual(['a', 'b'])
  })

  it('keeps the object of a session that did not change, and the map when none did', () => {
    const first = sessionViews(new Map(), [term('a'), term('b')])
    // Another update of the list: new objects everywhere, the same content.
    const again = sessionViews(first, [term('a', 'idle', { lastOutputAt: 99 }), term('b')])
    expect(again).toBe(first)
    const changed = sessionViews(first, [term('a'), term('b', 'working')])
    expect(changed).not.toBe(first)
    expect(changed.get('a')).toBe(first.get('a'))
    expect(changed.get('b')).not.toBe(first.get('b'))
    expect((changed.get('b') as SessionView).tone).toBe('working')
    // A session that is gone leaves the map.
    expect([...sessionViews(changed, [term('b', 'working')]).keys()]).toEqual(['b'])
    expect(sessionViews(first, undefined).size).toBe(0)
  })
})

describe('sessionOf', () => {
  it('is what the stage decides with', () => {
    expect(sessionOf(term('a', 'idle', { pinned: true, order: 3 }))).toEqual({ id: 'a', projectId: 'p1', agent: true, open: true, running: true, pinned: true, order: 3, createdAt: 1000 })
    expect(sessionOf(term('s', 'idle', { kind: 'shell', agent: null, open: false, status: 'exited', projectId: null }))).toMatchObject({ agent: false, open: false, running: false, projectId: null })
  })
})
