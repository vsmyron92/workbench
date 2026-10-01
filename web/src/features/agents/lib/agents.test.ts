import { describe, expect, it } from 'vitest'
import type { AgentInfo, TerminalInfo } from '@/api/types'
import { backoff, FrameRouter, OSC52_MAX, osc52WriteAllowed, validSize, wheelDecision, wheelLines } from './protocol'
import {
  agentMeta,
  canKill,
  colorCss,
  bottomTabs,
  columnTabs,
  counts,
  formatCost,
  lingering,
  needsAttention,
  nextAttention,
  quotePath,
  removeTerminal,
  restartMode,
  sortSessions,
  stateLabel,
  tabAfterClose,
  tone,
  upsertTerminal,
} from './sessions'

function agent(over: Partial<AgentInfo> = {}): AgentInfo {
  return {
    sessionId: 's',
    provider: 'claude',
    providerId: 'claude',
    state: 'idle',
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
    lastEventAt: 0,
    pendingPermission: null,
    ...over,
  }
}

function term(id: string, over: Partial<TerminalInfo> = {}, a: Partial<AgentInfo> | null = {}): TerminalInfo {
  return {
    id,
    kind: 'agent',
    title: id,
    projectId: 'p',
    cwd: '/w',
    argv: [],
    status: 'running',
    exit: null,
    createdAt: 1,
    lastOutputAt: 0,
    cols: 80,
    rows: 24,
    open: true,
    pinned: false,
    color: null,
    order: 1,
    agent: a === null ? null : agent(a),
    meta: {},
    ...over,
  }
}

describe('session state', () => {
  it('maps states to tones and labels', () => {
    expect(tone(term('a', {}, { state: 'working' }))).toBe('working')
    expect(tone(term('a', {}, { state: 'needs_permission' }))).toBe('attention')
    expect(tone(term('a', {}, { state: 'idle', unread: true }))).toBe('unread')
    expect(tone(term('a', { status: 'exited' }, { state: 'working' }))).toBe('exited')
    expect(stateLabel(term('a', {}, { state: 'needs_input' }))).toBe('Needs input')
    expect(stateLabel(term('a', { status: 'exited', exit: { code: 2, signal: null, at: 0 } }))).toBe('Exited (2)')
    expect(stateLabel(term('a', { status: 'exited', exit: { code: 129, signal: null, at: 0 } }))).toBe('Exited')
  })

  it('counts attention and sorts attention first', () => {
    const list = [
      term('idle', {}, { state: 'idle', lastEventAt: 50 }),
      term('work', {}, { state: 'working', lastEventAt: 10 }),
      term('perm', {}, { state: 'needs_permission', lastEventAt: 1 }),
      term('done', {}, { state: 'idle', unread: true, lastEventAt: 5 }),
      term('dead', { status: 'exited' }, { state: 'needs_input' }),
      term('shell', { kind: 'shell' }, null),
    ]
    expect(sortSessions(list.filter((t) => t.kind === 'agent')).map((t) => t.id)).toEqual(['perm', 'done', 'work', 'idle', 'dead'])
    expect(counts(list)).toEqual({ attention: 2, working: 1, running: 4 })
    expect(needsAttention(list[4])).toBe(false)
    expect(nextAttention(list, null)?.id).toBe('perm')
    expect(nextAttention(list, 'perm')?.id).toBe('done')
    expect(nextAttention(list, 'done')?.id).toBe('perm')
    expect(nextAttention([list[0]], null)).toBeNull()
  })

  it('updates the cache immutably', () => {
    const a = term('a')
    const list = upsertTerminal(undefined, a)
    const changed = upsertTerminal(list, { ...a, title: 'renamed' })
    expect(list[0].title).toBe('a')
    expect(changed[0].title).toBe('renamed')
    expect(upsertTerminal(changed, term('b')).map((t) => t.id)).toEqual(['a', 'b'])
    expect(removeTerminal(changed, 'a')).toEqual([])
  })

  it('restarts deploys and env commands only where they came from', () => {
    const cmd = (meta: Record<string, unknown>) => term('c', { kind: 'command', meta }, null)
    expect(restartMode(term('a'))).toBe('self')
    expect(restartMode(term('s', { kind: 'shell' }, null))).toBe('self')
    expect(restartMode(term('r', { kind: 'run', meta: { run: 'api' } }, null))).toBe('run')
    expect(restartMode(term('r', { kind: 'run', meta: { run: 'api' }, projectId: null }, null))).toBe('owner')
    expect(restartMode(cmd({ env: 'production', action: 'deploy' }))).toBe('owner')
    expect(restartMode(cmd({ env: 'production', action: 'command', command: 'migrate' }))).toBe('owner')
    expect(restartMode(cmd({}))).toBe('owner')
    expect(restartMode(cmd({ env: 'staging', action: 'logs' }))).toBe('self')
    expect(restartMode(cmd({ remoteControlServer: true }))).toBe('self')
    expect(restartMode(cmd({ restartable: true }))).toBe('self')
  })

  it('offers Kill while background jobs outlive the process', () => {
    const shell = (over: Partial<TerminalInfo>) => term('s', { kind: 'shell', ...over }, null)
    expect(canKill(shell({}))).toBe(true)
    expect(canKill(shell({ status: 'exited' }))).toBe(false)
    expect(canKill(shell({ status: 'exited', lingering: 2 }))).toBe(true)
    expect(lingering(shell({ status: 'exited', lingering: 2 }))).toBe(2)
    expect(lingering(shell({ lingering: 2 }))).toBe(0)
  })

  it('formats meta, costs, colours and paths', () => {
    expect(agentMeta(agent({ model: 'Haiku 4.5', effort: 'low', contextPct: 18.4, costUsd: 0.025 }))).toEqual(['Haiku 4.5', 'low', '18% ctx', '$0.03'])
    expect(formatCost(0.004)).toBe('<$0.01')
    expect(formatCost(null)).toBeNull()
    expect(colorCss('success')).toBe('var(--success)')
    expect(colorCss('#a1b2c3')).toBe('#a1b2c3')
    expect(colorCss('nope')).toBeUndefined()
    expect(quotePath('/tmp/a.png')).toBe('/tmp/a.png')
    expect(quotePath("/tmp/it's here")).toBe(`'/tmp/it'\\''s here'`)
  })
})

describe('agents column tabs', () => {
  const ids = (l: TerminalInfo[]) => l.map((t) => t.id)
  const list = [
    term('run', { kind: 'run', projectId: 'shop', order: 3, createdAt: 30 }, null),
    term('a2', { projectId: 'shop', order: 2, createdAt: 20 }),
    term('sh', { kind: 'shell', projectId: 'shop', order: 1, createdAt: 10 }, null),
    term('pin', { projectId: 'shop', order: 9, createdAt: 90, pinned: true }),
    term('closed', { projectId: 'shop', open: false, order: 4 }),
    term('other', { projectId: 'docs', order: 5 }),
    term('free', { kind: 'shell', projectId: null, order: 6 }, null),
  ]

  it("lists the project's open terminals of every kind, pinned first, in a stable order", () => {
    expect(ids(columnTabs(list, 'shop'))).toEqual(['pin', 'sh', 'a2', 'run'])
    expect(ids(columnTabs(list, 'docs'))).toEqual(['other'])
    // Without a project: the terminals that belong to none.
    expect(ids(columnTabs(list, null))).toEqual(['free'])
    expect(columnTabs(undefined, 'shop')).toEqual([])
  })

  it('adds the terminals opened on request after them, once each, and only while they exist', () => {
    // A closed session's saved screen, another project's session, one that is gone, and one that is a tab anyway.
    expect(ids(columnTabs(list, 'shop', ['other', 'closed', 'gone', 'a2', 'other']))).toEqual(['pin', 'sh', 'a2', 'run', 'other', 'closed'])
  })

  it("leaves the Terminal tool window's shells to it", () => {
    expect(ids(columnTabs(list, 'shop', [], ['sh', 'gone']))).toEqual(['pin', 'a2', 'run'])
    // Its tabs: the project's open terminals among its ids, in the order they were started.
    expect(ids(bottomTabs(list, 'shop', ['sh', 'closed', 'other', 'gone', 'a2']))).toEqual(['sh', 'a2'])
    expect(bottomTabs(undefined, 'shop', ['sh'])).toEqual([])
  })

  it('shows the neighbour of a tab that closes, the home tab after the last one', () => {
    const tabs = columnTabs(list, 'shop')
    expect(tabAfterClose(tabs, 'sh')).toBe('a2')
    expect(tabAfterClose(tabs, 'run')).toBe('a2')
    expect(tabAfterClose(columnTabs(list, 'docs'), 'other')).toBeNull()
    expect(tabAfterClose(tabs, 'nope')).toBeNull()
  })
})

describe('socket protocol', () => {
  const bytes = (s: string) => new TextEncoder().encode(s)

  it('treats the first binary frame as a snapshot and routes markers', () => {
    const r = new FrameRouter()
    const first = r.binary(bytes('snap'))
    expect(first.kind).toBe('snapshot')
    expect(r.binary(bytes('live')).kind).toBe('data')
    r.text('{"t":"resync","cols":100,"rows":30}')
    const again = r.binary(bytes('snap2'))
    expect(again).toMatchObject({ kind: 'snapshot', cols: 100, rows: 30 })
    expect(r.binary(bytes('x')).kind).toBe('data')
    expect(r.text('{"t":"exit","code":0,"signal":null}')).toEqual({ kind: 'exit', code: 0, signal: null })
    expect(r.text('{"t":"running"}')).toEqual({ kind: 'running' })
    expect(r.text('not json')).toEqual({ kind: 'none' })
    r.reset()
    expect(r.binary(bytes('s')).kind).toBe('snapshot')
  })

  it('routes size announcements', () => {
    const r = new FrameRouter()
    expect(r.text('{"t":"size","cols":50,"rows":20}')).toEqual({ kind: 'size', cols: 50, rows: 20 })
    expect(r.text('{"t":"size","cols":5,"rows":2}')).toEqual({ kind: 'none' })
    // A size frame does not turn the next binary frame into live data or a snapshot.
    expect(r.binary(bytes('snap')).kind).toBe('snapshot')
    r.text('{"t":"size","cols":60,"rows":20}')
    expect(r.binary(bytes('x')).kind).toBe('data')
  })

  it('never lets programs read the clipboard and only write it from a focused terminal', () => {
    expect(osc52WriteAllowed('c', 'copied', true)).toBe(true)
    expect(osc52WriteAllowed('c', 'copied', false)).toBe(false)
    expect(osc52WriteAllowed('p', 'copied', true)).toBe(false)
    expect(osc52WriteAllowed('c', '', true)).toBe(false)
    expect(osc52WriteAllowed('c', 'x'.repeat(OSC52_MAX + 1), true)).toBe(false)
  })

  it('validates sizes and backs off', () => {
    expect(validSize(80, 24)).toBe(true)
    expect(validSize(19, 24)).toBe(false)
    expect(validSize(80, 201)).toBe(false)
    expect(backoff(0)).toBe(500)
    expect(backoff(10)).toBe(5000)
  })

  it('scrolls locally only on a normal buffer with history under mouse reporting', () => {
    expect(wheelDecision('normal', 100, 'any')).toBe('local')
    expect(wheelDecision('normal', 0, 'any')).toBe('default')
    expect(wheelDecision('normal', 100, 'none')).toBe('default')
    expect(wheelDecision('alternate', 100, 'any')).toBe('default')
    expect(wheelLines(120, 0, 20, 30)).toBe(6)
    expect(wheelLines(3, 1, 20, 30)).toBe(3)
    expect(wheelLines(1, 2, 20, 30)).toBe(30)
  })
})
