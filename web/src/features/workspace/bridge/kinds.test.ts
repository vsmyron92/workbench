import { describe, expect, it } from 'vitest'
import type { TerminalInfo } from '@/api/types'
import { BridgeError, MAX_RESULT_BYTES, parseRequest, runQuery, shapeRepos, shapeRuns, shapeSessions, type BridgeIo } from './kinds'
import { bridgeScope } from './useReportBridge'

const ctx = { projectId: 'shop' }

/** Records every call and answers from a table of path → JSON. */
function fake(answers: Record<string, unknown>) {
  const calls: { method: 'GET' | 'POST'; path: string; query?: unknown; body?: unknown }[] = []
  const io: BridgeIo = {
    get: async (path, query) => {
      calls.push({ method: 'GET', path, query })
      if (!(path in answers)) throw new Error(`unexpected GET ${path}`)
      return answers[path] as never
    },
    post: async (path, body) => {
      calls.push({ method: 'POST', path, body })
      if (!(path in answers)) throw new Error(`unexpected POST ${path}`)
      return answers[path] as never
    },
  }
  return { io, calls }
}

const terminal = (over: Partial<TerminalInfo> & { agent?: Partial<NonNullable<TerminalInfo['agent']>> | null }): TerminalInfo =>
  ({
    id: 't1',
    kind: 'agent',
    title: 'Fix the flaky test',
    projectId: 'shop',
    cwd: '/home/me/secret-place',
    argv: ['claude', '--settings', '/home/me/.cache/x.json'],
    status: 'running',
    exit: null,
    meta: { remoteUrl: 'https://claude.ai/code/abc', token: 'SECRET' },
    ...over,
    agent:
      over.agent === null
        ? null
        : {
            sessionId: 'sid',
            provider: 'claude',
            state: 'working',
            unread: false,
            model: 'Opus',
            effort: 'high',
            contextPct: 41,
            costUsd: 0.5,
            lastEventAt: 7,
            lastMessage: 'the private answer',
            attention: 'Permission to run `cat ~/.ssh/id_rsa`',
            ...over.agent,
          },
  }) as unknown as TerminalInfo

describe('parsing a report’s message', () => {
  it('takes only workbench:query messages with a usable id, kind and small params', () => {
    expect(parseRequest(null)).toBeNull()
    expect(parseRequest('x')).toBeNull()
    expect(parseRequest({ type: 'workbench:open-link', href: 'https://x' })).toBeNull()
    expect(parseRequest({ type: 'workbench:query', id: 3, kind: 'sessions' })).toEqual({ id: 3, kind: 'sessions', params: {} })
    expect(parseRequest({ type: 'workbench:query', id: 3, kind: 'ci.runs', params: { limit: 5 } })).toEqual({ id: 3, kind: 'ci.runs', params: { limit: 5 } })
    // The id comes back to the report even when the rest is wrong; without one nothing can be answered.
    expect(parseRequest({ type: 'workbench:query', id: 4, kind: '' })).toMatchObject({ id: 4, invalid: expect.any(String) })
    expect(parseRequest({ type: 'workbench:query', id: 4, kind: 'x'.repeat(41) })).toMatchObject({ id: 4 })
    expect(parseRequest({ type: 'workbench:query', id: 4, kind: 'sessions', params: [1] })).toMatchObject({ id: 4 })
    expect(parseRequest({ type: 'workbench:query', id: 4, kind: 'sessions', params: { big: 'x'.repeat(5000) } })).toMatchObject({ id: 4 })
    for (const id of [0, -1, 1.5, '3', null, Number.NaN]) expect(parseRequest({ type: 'workbench:query', id, kind: 'sessions' })).toEqual({ id: null, invalid: expect.any(String) })
  })
})

describe('what a report is told', () => {
  it('lists the sessions of its own project and nothing from their screens', async () => {
    const { io, calls } = fake({
      '/api/terminals': [terminal({}), terminal({ id: 't2', projectId: 'other' }), terminal({ id: 't3', agent: null }), terminal({ id: 't4', status: 'exited' as never })],
    })
    const out = (await runQuery('sessions', {}, ctx, io)) as ReturnType<typeof shapeSessions>
    expect(calls).toEqual([{ method: 'GET', path: '/api/terminals', query: undefined }])
    expect(out.sessions.map((s) => s.id)).toEqual(['t1', 't4'])
    expect(out.sessions[0]).toEqual({
      id: 't1', title: 'Fix the flaky test', provider: 'claude', state: 'working', unread: false, model: 'Opus', effort: 'high',
      contextPct: 41, costUsd: 0.5, lastEventAt: 7, running: true,
    })
    const json = JSON.stringify(out)
    for (const leak of ['secret-place', 'SECRET', 'private answer', 'id_rsa', '--settings', 'claude.ai']) expect(json).not.toContain(leak)
  })

  it('shows repositories without their folders on this computer', async () => {
    const repo = {
      id: 'main', name: 'shop', path: '/home/me/workspace/shop', default: true, branch: 'main', head: 'abc1234', upstream: 'origin/main', ahead: 1, behind: 0,
      changed: 3, conflicts: 0, state: 'clean', gitlab: null, github: { host: 'github.com', path: 'me/shop' },
    }
    const { io, calls } = fake({ '/api/projects/shop/git/repos': [repo, { id: 'broken', name: 'x', error: { code: 'not_a_repo', message: 'fatal: /home/me/x is not a repository' } }] })
    const out = (await runQuery('git.repos', {}, ctx, io)) as ReturnType<typeof shapeRepos>
    expect(calls[0].path).toBe('/api/projects/shop/git/repos')
    expect(out.repos[0]).toMatchObject({ id: 'main', branch: 'main', ahead: 1, changed: 3, github: { host: 'github.com', path: 'me/shop' }, gitlab: null, error: null })
    expect(out.repos[1]).toMatchObject({ id: 'broken', error: 'not_a_repo' })
    expect(JSON.stringify(out)).not.toMatch(/\/home\/me|fatal/)
  })

  it('reads CI from the repository’s own forge, or the one asked for', async () => {
    const runsPage = { items: [{ id: 9, displayTitle: 'Fix', headBranch: 'main', headSha: 'abc', status: 'completed', conclusion: 'success', event: 'push', createdAt: '2026-11-01T00:00:00Z', duration: 61, htmlUrl: 'https://github.com/me/shop/actions/runs/9', actor: { login: 'someone', token: 'x' }, headCommit: { message: 'private' } }] }
    const pipes = { items: [{ id: 4, commitTitle: 'Build', ref: 'dev', sha: 'def', status: 'failed', source: 'push', createdAt: '2026-11-02T00:00:00Z', duration: 30, webUrl: 'javascript:alert(1)', user: { username: 'u' } }] }
    const answers = {
      '/api/projects/shop/git/repos': [{ id: 'main', default: true, github: { host: 'github.com', path: 'me/shop' }, gitlab: null }],
      '/api/projects/shop/github/actions/runs': runsPage,
      '/api/projects/shop/gitlab/pipelines': pipes,
    }
    const gh = fake(answers)
    const a = (await runQuery('ci.runs', { limit: 5 }, ctx, gh.io)) as ReturnType<typeof shapeRuns>
    expect(gh.calls[1]).toEqual({ method: 'GET', path: '/api/projects/shop/github/actions/runs', query: { repo: undefined, branch: undefined, perPage: 5 } })
    expect(a.provider).toBe('github')
    expect(a.runs[0]).toEqual({ id: 9, title: 'Fix', status: 'completed', conclusion: 'success', branch: 'main', sha: 'abc', event: 'push', createdAt: '2026-11-01T00:00:00Z', durationSec: 61, url: 'https://github.com/me/shop/actions/runs/9' })
    expect(JSON.stringify(a)).not.toMatch(/someone|private|token/)

    const gl = fake(answers)
    const b = (await runQuery('ci.runs', { provider: 'gitlab', branch: 'dev' }, ctx, gl.io)) as ReturnType<typeof shapeRuns>
    expect(gl.calls).toEqual([{ method: 'GET', path: '/api/projects/shop/gitlab/pipelines', query: { repo: undefined, ref: 'dev', perPage: 10 } }])
    expect(b.runs[0]).toMatchObject({ title: 'Build', branch: 'dev', status: 'failed', url: null })

    const none = fake({ '/api/projects/shop/git/repos': [{ id: 'main', default: true, github: null, gitlab: null }] })
    await expect(runQuery('ci.runs', {}, ctx, none.io)).rejects.toMatchObject({ code: 'unavailable' })
  })

  it('reads series, views and statistics of the data viewer with checked parameters', async () => {
    const { io, calls } = fake({
      '/api/projects/shop/data': { series: [{ name: 'temp', kind: 'gauge', unit: '°C', label: 'Temp', group: 'rig', count: 100, tStart: 0, tEnd: 99, last: 21.5, wall: false, updatedAt: 5, secretField: 'x' }], plots: [{ x: 1 }], sources: [{ path: '/home/me/capture.csv' }] },
      '/api/projects/shop/data/view': { views: { temp: { kind: 'gauge', count: 2, t: [0, 1], v: [1, null], n: 2, thinned: false } }, extra: 'dropped' },
      '/api/projects/shop/data/stats': { stats: { temp: { mean: 2 } } },
    })
    const list = (await runQuery('data.series', {}, ctx, io)) as { series: Record<string, unknown>[] }
    expect(list.series[0]).toEqual({ name: 'temp', kind: 'gauge', unit: '°C', label: 'Temp', group: 'rig', count: 100, tStart: 0, tEnd: 99, last: 21.5, wall: false, updatedAt: 5 })
    expect(JSON.stringify(list)).not.toMatch(/capture\.csv|secretField/)

    const view = (await runQuery('data.view', { series: ['temp'], from: 0, buckets: 99_999 }, ctx, io)) as { views: unknown }
    expect(calls.at(-1)).toEqual({ method: 'POST', path: '/api/projects/shop/data/view', body: { series: ['temp'], from: 0, to: undefined, buckets: 2000 } })
    expect(view).toEqual({ views: { temp: { kind: 'gauge', count: 2, t: [0, 1], v: [1, null], n: 2, thinned: false } } })
    expect(await runQuery('data.stats', { series: ['temp'], to: 9 }, ctx, io)).toEqual({ stats: { temp: { mean: 2 } } })

    for (const params of [{}, { series: [] }, { series: 'temp' }, { series: [1] }, { series: ['a\nb'] }, { series: Array(17).fill('s') }, { series: ['temp'], from: 'now' }, { series: ['temp'], buckets: 0 }, { series: ['temp'], buckets: 1.5 }]) {
      await expect(runQuery('data.view', params, ctx, io), JSON.stringify(params)).rejects.toMatchObject({ code: 'bad_request' })
    }
  })

  it('refuses unknown kinds and answers that are too large, and never names another project', async () => {
    const { io, calls } = fake({ '/api/projects/shop/data': { series: [] } })
    await expect(runQuery('files.read', { path: '/etc/passwd' }, ctx, io)).rejects.toMatchObject({ code: 'bad_request' })
    await expect(runQuery('constructor', {}, ctx, io)).rejects.toBeInstanceOf(BridgeError)
    expect(calls).toEqual([])
    // A report cannot pick the project: a `projectId` parameter is not read.
    await runQuery('data.series', { projectId: 'other' }, ctx, io)
    expect(calls.map((c) => c.path)).toEqual(['/api/projects/shop/data'])
    // The project of the card is encoded into the path, never trusted as a path.
    const odd = fake({ [`/api/projects/${encodeURIComponent('a/b')}/data`]: { series: [] } })
    await runQuery('data.series', {}, { projectId: 'a/b' }, odd.io)
    const big = fake({ '/api/projects/shop/data': { series: Array.from({ length: 500 }, (_, i) => ({ name: 'n'.repeat(200) + i, label: 'l'.repeat(200), group: 'g'.repeat(200), unit: 'u'.repeat(40) })) } })
    expect(JSON.stringify((await runQuery('data.series', {}, ctx, big.io)) as object).length).toBeLessThan(MAX_RESULT_BYTES)
    const huge = fake({ '/api/projects/shop/data/view': { views: { a: { t: Array.from({ length: 20_000 }, (_, i) => i * 1.234567890123), v: Array.from({ length: 20_000 }, (_, i) => i * 1.234567890123), w: Array.from({ length: 20_000 }, (_, i) => i * 1.234567890123), x: Array.from({ length: 20_000 }, (_, i) => i * 1.234567890123), y: Array.from({ length: 20_000 }, (_, i) => i * 1.234567890123), z: Array.from({ length: 20_000 }, (_, i) => i * 1.234567890123), u: Array.from({ length: 20_000 }, (_, i) => i * 1.234567890123) } } } })
    await expect(runQuery('data.view', { series: ['a'] }, ctx, huge.io)).rejects.toMatchObject({ code: 'too_large' })
  })
})

describe('which cards get live data', () => {
  it('only cards of a project that were made here', () => {
    expect(bridgeScope({ origin: 'workbench', scope: 'shop' })).toEqual({ projectId: 'shop' })
    expect(bridgeScope({ origin: 'repo', scope: 'shop' })).toMatchObject({ reason: expect.stringContaining('repository') })
    expect(bridgeScope({ origin: 'workbench', scope: 'home' })).toMatchObject({ reason: expect.stringContaining('project') })
    expect(bridgeScope({ origin: 'workbench', scope: 'wb-sandbox' })).toMatchObject({ reason: expect.any(String) })
    expect(bridgeScope(undefined)).toMatchObject({ reason: expect.any(String) })
  })
})
