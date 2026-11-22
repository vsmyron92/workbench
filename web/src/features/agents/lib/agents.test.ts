import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AgentInfo, PendingPermission, TerminalInfo } from '@/api/types'
import { useToasts } from '@/shell/actions'
import { avatarShows, setAvatarShows, setAvatarsOn } from '@/shell/avatarBridge'
import { SessionCard } from '../AgentsHome'
import { dismissPermissionToasts } from '../permissionToasts'
import { attentionToast, type Attention } from '../providers'
import { backoff, FrameRouter, OSC52_MAX, osc52WriteAllowed, validSize, wheelDecision, wheelLines } from './protocol'
import {
  agentMeta,
  avatarBadge,
  avatarNote,
  canKill,
  colorCss,
  bottomTabs,
  columnTabs,
  counts,
  tabPlace,
  formatCost,
  formatTokens,
  lingering,
  needsAttention,
  nextAttention,
  quotePath,
  removeTerminal,
  restartMode,
  resumeNote,
  artifactPages,
  isArtifactUrl,
  ultracodeNote,
  sortSessions,
  stateLabel,
  tabAfterClose,
  tone,
  upsertTerminal,
  workingSessions,
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
    usage: null,
    priced: false,
    lastEventAt: 0,
    pendingPermission: null,
    resumeCost: null,
    artifacts: [],
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
  it('counts tokens the way people say them', () => {
    expect(formatTokens(182_340)).toBe('182k')
    expect(formatTokens(999)).toBe('999')
    expect(formatTokens(1_250_000)).toBe('1.3M')
    expect(formatTokens(2_000_000)).toBe('2M')
    expect(formatTokens(-1)).toBe('?')
  })

  it('tells a resumed conversation that its cache ran out, and says nothing while it is warm', () => {
    const cold = agent({ resumeCost: { contextTokens: 182_340, secondsSinceLastResponse: 5400, cacheExpired: true, estimatedCacheWriteUsd: 1.1396 } })
    const note = resumeNote(cold)
    expect(note?.label).toBe('Cache expired · $1.14')
    expect(note?.title).toContain('182k tokens')
    // No estimate: still told, without a price. Warm, unknown or not an agent: nothing.
    expect(resumeNote(agent({ resumeCost: { contextTokens: 5000, secondsSinceLastResponse: null, cacheExpired: true, estimatedCacheWriteUsd: null } }))?.label).toBe('Cache expired')
    expect(resumeNote(agent({ resumeCost: { contextTokens: 5000, secondsSinceLastResponse: 60, cacheExpired: false, estimatedCacheWriteUsd: 0.2 } }))).toBeNull()
    // A service whose own prices are known (DeepSeek): the CLI's estimate is not shown.
    expect(resumeNote(agent({ priced: true, resumeCost: { contextTokens: 5000, secondsSinceLastResponse: null, cacheExpired: true, estimatedCacheWriteUsd: 1 } }))).toBeNull()
    expect(resumeNote(agent())).toBeNull()
    expect(resumeNote(null)).toBeNull()
  })

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
    expect(workingSessions(list).map((t) => t.id)).toEqual(['work'])
    expect(workingSessions(undefined)).toEqual([])
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
    expect(agentMeta(agent({ usage: { input: 250, cacheRead: 750, cacheCreation: 0, output: 10 }, costUsd: 0.01 }))).toContain('75% cached')
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
    // A closed session's saved screen, a terminal of a project that is gone, one that is gone, and one that is a tab anyway.
    expect(ids(columnTabs(list, 'shop', ['other', 'closed', 'gone', 'a2', 'other']))).toEqual(['pin', 'sh', 'a2', 'run', 'other', 'closed'])
  })

  it("shows a terminal opened on request in its own project's column", () => {
    const projects = ['shop', 'docs']
    const t = (id: string) => list.find((x) => x.id === id)
    // The project's own open terminal: a tab already. A closed one: an extra of its project.
    expect(tabPlace(t('a2'), 'shop', projects)).toEqual({ project: 'shop', extra: false })
    expect(tabPlace(t('closed'), 'shop', projects)).toEqual({ project: 'shop', extra: true })
    // Another project's: that project's column, not the current one.
    expect(tabPlace(t('other'), 'shop', projects)).toEqual({ project: 'docs', extra: false })
    expect(tabPlace({ ...t('other')!, open: false }, 'shop', projects)).toEqual({ project: 'docs', extra: true })
    // Its project is not listed any more, or it belongs to none: an extra of the current column.
    expect(tabPlace(t('other'), 'shop', ['shop'])).toEqual({ project: 'shop', extra: true })
    expect(tabPlace(t('free'), 'shop', projects)).toEqual({ project: 'shop', extra: true })
    expect(tabPlace(t('free'), null, projects)).toEqual({ project: null, extra: false })
    // Not listed yet (just created): the current column, until it arrives.
    expect(tabPlace(undefined, 'shop', projects)).toEqual({ project: 'shop', extra: true })
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

describe('pages published to claude.ai', () => {
  const ok = 'https://claude.ai/artifact/9f2c1d7e-aaaa-4bbb-8ccc-0123456789ab'
  it('only claude.ai artifact addresses are links', () => {
    expect(isArtifactUrl(ok)).toBe(true)
    expect(isArtifactUrl('https://claude.ai/code/artifact/9f2c1d7e-aaaa-4bbb-8ccc-0123456789ab')).toBe(true)
    for (const bad of ['http://claude.ai/artifact/9f2c1d7e-aaaa', 'https://claude.ai.evil.example/artifact/9f2c1d7e-aaaa', 'javascript:alert(1)', 'https://claude.ai/artifact/x', `${ok}?a=b`, null, 5]) {
      expect(isArtifactUrl(bad)).toBe(false)
    }
  })
  it('lists the session’s pages newest first and drops what is not a claude.ai page', () => {
    const a = agent({ artifacts: [{ url: ok, title: 'CI health', at: 2 }, { url: 'https://evil.example/x', title: 'x', at: 1 }, { url: ok.replace('9f2c', 'aaaa'), title: null, at: 0 }] })
    expect(artifactPages(a)).toEqual([
      { url: ok, label: 'CI health' },
      { url: ok.replace('9f2c', 'aaaa'), label: 'Page' },
    ])
    expect(artifactPages(agent())).toEqual([])
    expect(artifactPages(null)).toEqual([])
  })
})

describe('ultracode sessions', () => {
  it('say so on their tab and card only when the session was started with it', () => {
    const t = (meta: Record<string, unknown>) => ({ id: 't', meta, agent: agent() }) as unknown as TerminalInfo
    expect(ultracodeNote(t({ ultracode: true }))?.label).toBe('Ultracode')
    expect(ultracodeNote(t({ ultracode: true }))?.title).toContain('more tokens')
    for (const meta of [{}, { ultracode: false }, { ultracode: 'yes' }, { forkedFrom: 'x' }]) expect(ultracodeNote(t(meta))).toBeNull()
    expect(ultracodeNote(null)).toBeNull()
  })
})

describe('avatar sessions', () => {
  it('say so on their tab and card only when the session was started with one', () => {
    const t = (meta: Record<string, unknown>) => ({ id: 't', meta, agent: agent() }) as unknown as TerminalInfo
    expect(avatarNote(t({ avatar: true }))).toEqual({ label: 'Avatar', title: 'This session has a live avatar' })
    // Only the server's own `true` counts: not a look-alike, and not what another option set.
    for (const meta of [{}, { avatar: false }, { avatar: 'true' }, { avatar: 1 }, { avatar: {} }, { ultracode: true }, { movedFrom: 't0' }]) expect(avatarNote(t(meta))).toBeNull()
    expect(avatarNote(null)).toBeNull()
    expect(avatarNote(undefined)).toBeNull()
    // A terminal without meta (a hand-built or older record) is no reason to fail.
    expect(avatarNote({ id: 't' } as unknown as TerminalInfo)).toBeNull()
    // The two badges do not depend on each other.
    expect([avatarNote(t({ avatar: true, ultracode: true }))?.label, ultracodeNote(t({ avatar: true, ultracode: true }))?.label]).toEqual(['Avatar', 'Ultracode'])
  })

  it('wear the badge by the live record once it is known, and by how they were started until then', () => {
    const t = (meta: Record<string, unknown>) => ({ id: 't', meta, agent: agent() }) as unknown as TerminalInfo
    const badge = { label: 'Avatar', title: 'This session has a live avatar' }
    // Not known (the list has not loaded, or the avatar feature is gone): what the session was started with.
    expect(avatarBadge(t({ avatar: true }), null)).toEqual(badge)
    expect(avatarBadge(t({}), null)).toBeNull()
    expect(avatarBadge(t({ avatar: 'true' }), null)).toBeNull()
    expect(avatarBadge(null, null)).toBeNull()
    // Known: the record decides. A session given an avatar later wears the badge.
    expect(avatarBadge(t({}), true)).toEqual(badge)
    expect(avatarBadge(t({ avatar: true }), true)).toEqual(badge)
    // And one whose avatar the user turned off does not, although it was started with the option.
    expect(avatarBadge(t({ avatar: true }), false)).toBeNull()
    expect(avatarBadge(t({}), false)).toBeNull()
    // A caller without the live answer falls back like "not known"; nothing but a plain yes shows it.
    expect(avatarBadge(t({ avatar: true }), undefined as unknown as null)).toEqual(badge)
    expect(avatarBadge(t({}), 'yes' as unknown as boolean)).toBeNull()
    // The same words as the note of a session started with the option.
    expect(avatarBadge(t({}), true)).toEqual(avatarNote(t({ avatar: true })))
  })

  it('show it on their card as the avatar feature tells it, session by session', () => {
    const card = (id: string, meta: Record<string, unknown>) => renderToStaticMarkup(createElement(QueryClientProvider, { client: new QueryClient() }, createElement(SessionCard, { t: term(id, { meta }) })))
    const badge = '<span class="wb-badge accent" title="This session has a live avatar">Avatar</span>'
    try {
      // Nothing known yet: the session started with the option says so, the other does not.
      setAvatarsOn(null)
      expect(card('t1', { avatar: true })).toContain(badge)
      expect(card('t2', {})).not.toContain('live avatar')
      // The list arrived: t2 was given an avatar from a device, and t1's was turned off.
      setAvatarsOn(new Set(['t2']))
      expect(card('t1', { avatar: true })).not.toContain('live avatar')
      expect(card('t2', {})).toContain(badge)
      // Both on; then nobody has one.
      setAvatarsOn(new Set(['t1', 't2']))
      expect(card('t1', { avatar: true })).toContain(badge)
      expect(card('t2', {})).toContain(badge)
      setAvatarsOn(new Set())
      expect(card('t1', { avatar: true })).not.toContain('live avatar')
      // It stands beside the Ultracode badge, not instead of it.
      setAvatarsOn(new Set(['t1']))
      const both = card('t1', { ultracode: true })
      expect(both).toContain(badge)
      expect(both).toContain('>Ultracode</span>')
    } finally {
      setAvatarsOn(null)
    }
  })
})

describe('attention toasts', () => {
  const permission: PendingPermission = { id: 'p1', tool: 'Bash', summary: 'Permission to run `npm test`', since: 1, detail: 'npm test', complete: true }

  beforeEach(() => {
    vi.useFakeTimers()
    // The notifier toasts only in a visible window, and toasts expire on `window` timers.
    vi.stubGlobal('window', globalThis)
    vi.stubGlobal('document', { visibilityState: 'visible' })
  })
  afterEach(() => {
    setAvatarShows(null)
    dismissPermissionToasts('t1')
    useToasts.setState({ toasts: [] })
    vi.unstubAllGlobals()
    vi.useRealTimers()
  })

  /** The toasts these events leave on screen. */
  function toasted(...events: Attention[]): string[] {
    useToasts.setState({ toasts: [] })
    for (const a of events) attentionToast(a)
    return useToasts.getState().toasts.map((t) => t.message)
  }
  const done = (terminalId: string): Attention => ({ terminalId, state: 'idle', message: 'All tests pass', title: 'Build' })

  it('tell that a session finished while nothing shows its avatar', () => {
    // No avatar feature registered (or none loaded yet): as before.
    expect(avatarShows('t1')).toBe(false)
    expect(toasted(done('t1'))).toEqual(['Build finished'])
  })

  it('leave "finished" to an avatar that is on screen here, for that session only', () => {
    setAvatarShows((id) => id === 't1')
    expect([avatarShows('t1'), avatarShows('t2')]).toEqual([true, false])
    expect(toasted(done('t1'))).toEqual([])
    expect(toasted(done('t2'))).toEqual(['Build finished'])
    // The avatar feature went away (signed out, or its overlay closed): toasts again.
    setAvatarShows(null)
    expect(avatarShows('t1')).toBe(false)
    expect(toasted(done('t1'))).toEqual(['Build finished'])
  })

  it('still tell what waits for the user, avatar or not', () => {
    setAvatarShows(() => true)
    const seen = toasted(
      { terminalId: 't1', state: 'needs_input', message: 'Which branch?', title: 'Build' },
      { terminalId: 't1', state: 'error', message: 'API error', title: 'Build' },
      { terminalId: 't1', state: 'needs_permission', message: 'Permission to run npm test', title: 'Build', permission },
    )
    expect(seen).toEqual(['Build: Which branch?', 'Build: API error', 'Build asks for permission'])
    // The request's toast keeps its Allow and Deny.
    expect(useToasts.getState().toasts.at(-1)?.actions?.map((a) => a.label)).toEqual(['Allow', 'Deny', 'Open'])
  })

  it('are shown when the avatar’s answer fails, or is not a plain yes', () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {})
    try {
      setAvatarShows(() => {
        throw new Error('boom')
      })
      expect(avatarShows('t1')).toBe(false)
      expect(toasted(done('t1'))).toEqual(['Build finished'])
      expect(errors).toHaveBeenCalled()
      setAvatarShows((() => 'yes') as unknown as (id: string) => boolean)
      expect(avatarShows('t1')).toBe(false)
      expect(toasted(done('t1'))).toEqual(['Build finished'])
    } finally {
      errors.mockRestore()
    }
  })

  it('stay quiet in a hidden window, as before', () => {
    vi.stubGlobal('document', { visibilityState: 'hidden' })
    expect(toasted(done('t1'))).toEqual([])
  })
})
