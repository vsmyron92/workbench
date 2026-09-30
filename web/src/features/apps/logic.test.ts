import { describe, expect, it } from 'vitest'
import {
  appPanelId,
  applyEnvEvent,
  applyRunEvent,
  defaultRun,
  envShortLabel,
  formatLatency,
  groupRuns,
  healthTone,
  isLoopbackHost,
  pathOnOrigin,
  pillEnvs,
  runStateLabel,
  runTone,
  runUrl,
  sparkline,
  uptime,
} from './logic'
import type { EnvView, RunConfigView, RunView } from './types'

function cfg(p: Partial<RunConfigView> = {}): RunConfigView {
  return { kind: 'task', command: 'x', cwd: '.', freePort: false, dependsOn: [], env: {}, hasStop: false, hasStatus: false, ...p }
}
function run(name: string, p: Partial<RunView> = {}, c: Partial<RunConfigView> = {}): RunView {
  return { name, state: 'stopped', config: cfg(c), portInUse: false, problems: [], ...p }
}
function env(p: Partial<EnvView> = {}): EnvView {
  return {
    name: 'production',
    kind: 'production',
    url: 'https://example.app',
    config: { host: null, target: null, health: null, version: null, auth: null, logs: [], commands: [], deploy: null },
    health: { status: 'unknown', history: [] },
    version: null,
    preview: { mode: 'direct' },
    ...p,
  }
}

describe('runs', () => {
  it('groups by group, then kind, suggested last', () => {
    const g = groupRuns([
      run('s', {}, { group: 'suggested' }),
      run('t', {}, { kind: 'test' }),
      run('api', {}, { kind: 'server', group: 'dev' }),
      run('u', {}, { group: 'unity' }),
      run('ed', {}, { kind: 'editor' }),
    ])
    expect(g.map((x) => x.id)).toEqual(['dev', 'test', 'unity', 'tools', 'suggested'])
    expect(g[0].label).toBe('Development')
    expect(g.at(-1)!.label).toBe('Suggested from docs')
  })

  it('picks a server as the default selection', () => {
    expect(defaultRun([run('lint'), run('web', {}, { kind: 'server' })])).toBe('web')
    expect(defaultRun([run('s', {}, { group: 'suggested' }), run('lint')])).toBe('lint')
    // A deploy script is never what a fresh project's Run button starts.
    expect(defaultRun([run('deploy (web)', {}, { group: 'deploy' }), run('lint')])).toBe('lint')
    const g = groupRuns([run('deploy (web)', {}, { group: 'deploy' }), run('s', {}, { group: 'suggested' }), run('lint')])
    expect(g.map((x) => [x.id, x.label])).toEqual([
      ['tasks', 'Tasks'],
      ['deploy', 'Deploy & release'],
      ['suggested', 'Suggested from docs'],
    ])
    expect(defaultRun([])).toBeNull()
  })

  it('labels and tones states', () => {
    expect(runStateLabel(run('w', { state: 'ready', port: 5173 }))).toBe('ready :5173')
    expect(runStateLabel(run('w', { state: 'starting', phase: 'waiting for api' }))).toBe('waiting for api')
    expect(runStateLabel(run('t', { state: 'exited', result: { passed: 12, failed: 0 } }))).toBe('12 passed')
    expect(runStateLabel(run('t', { state: 'failed', result: { passed: 3, failed: 2 } }))).toBe('3 passed, 2 failed')
    expect(runStateLabel(run('t', { state: 'failed', exit: { code: 101, signal: null, at: 0 } }))).toBe('failed (101)')
    expect(runStateLabel(run('t', { state: 'exited', exit: { code: 3, signal: null, at: 0 } }))).toBe('exit 3')
    expect(runStateLabel(run('w', { state: 'exited', terminated: true, exit: { code: 1, signal: 'Hangup', at: 0, terminated: true } }))).toBe('terminated')
    // A test run cut short: what it counted so far is no success.
    const cut = run('t', { state: 'exited', terminated: true, result: { passed: 0, failed: 0 }, exit: { code: 1, signal: null, at: 0, terminated: true } })
    expect(runStateLabel(cut)).toBe('0 passed · terminated')
    expect(runTone(cut)).toBe('warning')
    expect(runTone(run('w', { state: 'exited', terminated: true }))).toBe('warning')
    expect(runTone(run('t', { state: 'exited', result: { passed: 2, failed: 0 } }))).toBe('success')
    expect(runStateLabel(run('t'))).toBe('')
    expect(runTone(run('w', { state: 'ready' }))).toBe('success')
    expect(runTone(run('w', { state: 'running', error: 'not ready after 60s' }))).toBe('warning')
    expect(runTone(run('t', { state: 'exited', result: { passed: 1, failed: 1 } }))).toBe('danger')
    expect(runTone(run('t', { state: 'failed' }))).toBe('danger')
  })

  it('applies run.state events, clearing absent fields', () => {
    const before = run('web', { state: 'failed', error: 'boom', terminalId: 't1' })
    const after = applyRunEvent({ ...before, terminated: true }, { state: 'starting', phase: 'waiting for api' })
    expect(after.state).toBe('starting')
    expect(after.error).toBeUndefined()
    expect(after.terminated).toBeUndefined()
    expect(after.terminalId).toBeUndefined()
    expect(after.config).toBe(before.config)
  })

  it('computes preview URLs', () => {
    expect(runUrl(run('w', { url: 'http://localhost:5173/' }))).toBe('http://localhost:5173/')
    expect(runUrl(run('w', {}, { preview: 'http://localhost:4173/' }))).toBe('http://localhost:4173/')
    expect(runUrl(run('api', {}, { kind: 'server', port: 8080 }))).toBe('http://localhost:8080/')
    expect(runUrl(run('t', {}, { kind: 'test' }))).toBeNull()
  })
})

describe('environments', () => {
  it('maps health to tones and labels', () => {
    expect(healthTone('up')).toBe('success')
    expect(healthTone('down')).toBe('danger')
    expect(healthTone(undefined)).toBe('muted')
    expect(envShortLabel({ name: 'production', kind: 'production' })).toBe('prod')
    expect(envShortLabel({ name: 'staging', kind: 'staging' })).toBe('staging')
    expect(formatLatency(182.4)).toBe('182 ms')
    expect(formatLatency(2500)).toBe('2.5 s')
    expect(uptime([{ t: 1, ok: true, ms: 1 }, { t: 2, ok: false, ms: null }])).toBe('1/2')
  })

  it('orders pills production first', () => {
    const e = pillEnvs([env({ name: 'dev', kind: 'development' }), env({ name: 'staging', kind: 'staging' }), env()])
    expect(e.map((x) => x.name)).toEqual(['production', 'staging', 'dev'])
    expect(pillEnvs([env(), env(), env(), env()], 2)).toHaveLength(2)
  })

  it('applies env.health events and caps the history', () => {
    const history = Array.from({ length: 60 }, (_, i) => ({ t: i, ok: true, ms: 100 }))
    const e = env({ health: { status: 'up', history } })
    const next = applyEnvEvent(e, { env: 'production', status: 'down', error: 'connection failed', checkedAt: 99, sample: { t: 99, ok: false, ms: null } })!
    expect(next.health.status).toBe('down')
    expect(next.health.history).toHaveLength(60)
    expect(next.health.history.at(-1)).toEqual({ t: 99, ok: false, ms: null })
    expect(next.health.error).toBe('connection failed')
    expect(applyEnvEvent(e, { env: 'production', previewChanged: true })).toBeNull()
    expect(applyEnvEvent(e, { env: 'production', deploying: 't1' })).toBeNull()
    const v = applyEnvEvent(e, { env: 'production', status: 'up', version: '4b8e2508', checkedAt: 5 })!
    expect(v.version?.sha).toBe('4b8e2508')
  })

  it('builds sparkline geometry', () => {
    const s = sparkline(
      [
        { t: 1, ok: true, ms: 100 },
        { t: 2, ok: false, ms: null },
        { t: 3, ok: true, ms: 200 },
      ],
      118,
      20,
    )
    expect(s.max).toBe(200)
    expect(s.fails).toHaveLength(1)
    const pts = s.line.split(' ')
    expect(pts).toHaveLength(2)
    expect(pts[1]).toBe('118,2')
    expect(sparkline([], 100, 20).line).toBe('')
  })
})

describe('urls and ids', () => {
  it('builds stable panel ids', () => {
    expect(appPanelId('shop', { env: 'staging' })).toBe('app:shop:staging')
    expect(appPanelId('shop', { run: 'dev (app/web)' })).toBe('app:shop:dev (app/web)')
  })
  it('recognises loopback hosts', () => {
    expect(isLoopbackHost('127.0.0.1')).toBe(true)
    expect(isLoopbackHost('localhost')).toBe(true)
    expect(isLoopbackHost('[::1]')).toBe(true)
    expect(isLoopbackHost('box.tailnet.ts.net')).toBe(false)
  })
  it('maps URLs onto an origin', () => {
    expect(pathOnOrigin('https://staging.x.app/a?b=1#c', 'https://staging.x.app')).toBe('/a?b=1#c')
    expect(pathOnOrigin('https://other.app/a', 'https://staging.x.app')).toBeNull()
    expect(pathOnOrigin('not a url', 'https://staging.x.app')).toBeNull()
  })
})
