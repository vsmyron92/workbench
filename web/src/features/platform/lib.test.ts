import { describe, expect, it } from 'vitest'
import {
  accountHomeError,
  accountIdError,
  accountKind,
  desktopNotifiesHere,
  formatCountdown,
  formatMs,
  hostEntryError,
  hostOf,
  inConfigDir,
  makeSecretRef,
  panelIdFor,
  prependCapped,
  projectPathError,
  publicUrlError,
  restartImpact,
  restartText,
  secretRefFields,
  suggestAccountId,
  timeline,
  updateFraction,
  updatePhaseText,
} from './lib'
import type { ActivityEvent, McpCall } from './types'

describe('desktopNotifiesHere', () => {
  it('leaves notifying to the server on its own computer, unless its OS has no desktop notifications', () => {
    for (const host of ['localhost', '127.0.0.1', '[::1]']) expect(desktopNotifiesHere(host, null)).toBe(true)
    // A Windows server: the browser on its computer notifies instead.
    expect(desktopNotifiesHere('127.0.0.1', 'Desktop notifications are not supported on Windows yet')).toBe(false)
    // Phones and other computers never see the server's desktop.
    expect(desktopNotifiesHere('workbench.example.ts.net', null)).toBe(false)
  })
})

describe('panelIdFor', () => {
  it('follows the documented id conventions', () => {
    expect(panelIdFor('editor', { projectId: 'shop', path: 'src/main.rs', line: 3 })).toBe('editor:shop:src/main.rs')
    expect(panelIdFor('editor', { projectId: null, path: '/tmp/x.png' })).toBe('editor::/tmp/x.png')
    expect(panelIdFor('terminal', { terminalId: 't1' })).toBe('terminal:t1')
    expect(panelIdFor('diff', { projectId: 'p', path: 'a.rs', mode: 'working' })).toBe('diff:p:working::a.rs')
    expect(panelIdFor('diff', { projectId: 'p', path: 'a.rs', mode: 'commit', sha: 'abc' })).toBe('diff:p:commit:abc:a.rs')
    expect(panelIdFor('diff', { projectId: 'p', path: 'a.rs', mode: 'compare', base: 'main', head: 'dev' })).toBe(
      'diff:p:compare:main..dev:a.rs',
    )
    expect(panelIdFor('mr', { projectId: 'p', iid: 12 })).toBe('mr:p:12')
    expect(panelIdFor('pipeline', { projectId: 'p', pipelineId: 99 })).toBe('pipeline:p:99')
    expect(panelIdFor('job', { projectId: 'p', jobId: 5 })).toBe('job:p:5')
    expect(panelIdFor('confluence', { pageId: '229492', mode: 'view' })).toBe('confluence:229492')
    expect(panelIdFor('jira', { key: 'HB-1' })).toBe('jira:HB-1')
    expect(panelIdFor('app', { projectId: 'p', env: 'staging', url: 'https://x' })).toBe('app:p:staging')
    expect(panelIdFor('app', { projectId: 'p', url: 'https://x' })).toBe('app:p:https://x')
    expect(panelIdFor('settings', { section: 'remote' })).toBe('settings')
    expect(panelIdFor('gitlog', { projectId: 'p', path: 'x' })).toBe('gitlog:p')
    expect(panelIdFor('gitlab.issue', { projectId: 'p', iid: 3 })).toBe('gitlab.issue:p:3')
    expect(panelIdFor('pr', { projectId: 'p', number: 7 })).toBe('pr:p:7')
    expect(panelIdFor('gh.issue', { projectId: 'p', number: 8 })).toBe('gh.issue:p:8')
    expect(panelIdFor('gh.run', { projectId: 'p', runId: 42 })).toBe('gh.run:p:42')
    expect(panelIdFor('gh.job', { projectId: 'p', jobId: 43 })).toBe('gh.job:p:43')
    expect(panelIdFor('workspace.home', { scope: 'home' })).toBe('workspace.home')
    expect(panelIdFor('card', { scope: 'p', cardId: 'c1', step: 2 })).toBe('card:p:c1')
  })

  it('is stable for unknown kinds regardless of key order', () => {
    expect(panelIdFor('custom', { b: 1, a: 2 })).toBe(panelIdFor('custom', { a: 2, b: 1 }))
    expect(panelIdFor('custom', {})).toBe('custom')
  })
})

describe('formatting', () => {
  it('counts down', () => {
    expect(formatCountdown(600_000)).toBe('10:00')
    expect(formatCountdown(61_001)).toBe('1:02')
    expect(formatCountdown(-5)).toBe('0:00')
  })
  it('formats durations', () => {
    expect(formatMs(87)).toBe('87 ms')
    expect(formatMs(1234)).toBe('1.2 s')
    expect(formatMs(42_000)).toBe('42 s')
    expect(formatMs(125_000)).toBe('2 m 5 s')
  })
  it('names restart keys', () => {
    expect(restartText(['server.bind', 'server.tls'])).toBe('bind address and TLS certificate')
  })
})

describe('validation', () => {
  it('checks allowed host entries', () => {
    expect(hostEntryError('box.tailnet.ts.net')).toBeNull()
    expect(hostEntryError('192.168.1.5:7777')).toBeNull()
    expect(hostEntryError('https://box')).not.toBeNull()
    expect(hostEntryError('box/path')).not.toBeNull()
    expect(hostEntryError('  ')).not.toBeNull()
  })
  it('checks public URLs', () => {
    expect(publicUrlError('')).toBeNull()
    expect(publicUrlError('https://box.tailnet.ts.net')).toBeNull()
    expect(publicUrlError('ftp://x')).not.toBeNull()
    expect(publicUrlError('https://u:p@x')).not.toBeNull()
    expect(publicUrlError('nope')).not.toBeNull()
    expect(hostOf('https://box.ts.net:8443/x')).toBe('box.ts.net:8443')
  })
  it('checks project paths as the server OS reads them', () => {
    for (const os of ['linux', undefined, 'windows']) {
      expect(projectPathError('/srv/code', os)).toBeNull()
      expect(projectPathError('~/workspace', os)).toBeNull()
      expect(projectPathError('workspace', os)).not.toBeNull()
    }
    // Windows forms only for a Windows server.
    for (const p of ['D:\\code', 'C:/Users/me/src', '~\\src', '\\\\server\\share']) {
      expect(projectPathError(p, 'windows')).toBeNull()
    }
    expect(projectPathError('D:\\code', 'linux')).toBe('Use an absolute path or ~/…')
    expect(projectPathError('D:code', 'windows')).not.toBeNull()
    expect(projectPathError('code\\app', 'windows')).not.toBeNull()
  })
  it('suggests files in the config dir the server reports, with its separator', () => {
    expect(inConfigDir('~/.config/workbench', 'tls', 'cert.pem')).toBe('~/.config/workbench/tls/cert.pem')
    expect(inConfigDir('~\\AppData\\Roaming\\workbench', 'tls', 'key.pem')).toBe('~\\AppData\\Roaming\\workbench\\tls\\key.pem')
    expect(inConfigDir('/srv/wb/config/', 'tls', 'cert.pem')).toBe('/srv/wb/config/tls/cert.pem')
    expect(inConfigDir(undefined, 'tls', 'cert.pem')).toBe('~/.config/workbench/tls/cert.pem')
  })
})

describe('secret references', () => {
  it('round-trips through the editor fields', () => {
    for (const r of [
      { file: '~/.gitlab_token' },
      { env: 'GITLAB_TOKEN' },
      { keyring: 'workbench/gitlab' },
      { dotenv: { path: '.env', key: 'TOKEN' } },
      { command: ['pass', 'show', 'x'] },
    ]) {
      const f = secretRefFields(r)
      expect(makeSecretRef(f.source, f.location, f.key)).toEqual(r)
    }
  })
  it('rejects incomplete references', () => {
    expect(typeof makeSecretRef('env', 'not valid')).toBe('string')
    expect(typeof makeSecretRef('keyring', 'nosep')).toBe('string')
    expect(typeof makeSecretRef('dotenv', '.env', '')).toBe('string')
    expect(typeof makeSecretRef('file', '  ')).toBe('string')
  })
})

describe('activity', () => {
  const call = (id: number, at: number, extra: Partial<McpCall> = {}): McpCall => ({
    id,
    at,
    terminalId: null,
    projectId: null,
    session: null,
    tool: 'x',
    ok: true,
    mutating: false,
    ms: 1,
    summary: '',
    error: null,
    ...extra,
  })
  const event = (id: number, at: number, level: ActivityEvent['level'] = 'info'): ActivityEvent => ({
    id,
    at,
    kind: 'env',
    level,
    projectId: null,
    terminalId: null,
    title: '',
    message: '',
  })

  it('caps and dedupes prepends', () => {
    let list = [call(1, 1)]
    list = prependCapped(list, call(2, 2), 2)
    list = prependCapped(list, call(3, 3), 2)
    expect(list.map((c) => c.id)).toEqual([3, 2])
    expect(prependCapped(list, call(3, 4)).map((c) => c.id)).toEqual([3, 2])
  })

  it('merges and filters the timeline', () => {
    const calls = [call(3, 30, { mutating: true }), call(1, 10, { ok: false })]
    const events = [event(2, 20), event(4, 40, 'error')]
    const all = timeline(calls, events, { show: 'all', writesOnly: false, errorsOnly: false, since: 0 })
    expect(all.map((i) => i.at)).toEqual([40, 30, 20, 10])
    expect(timeline(calls, events, { show: 'all', writesOnly: true, errorsOnly: false, since: 0 }).map((i) => i.at)).toEqual([30])
    expect(timeline(calls, events, { show: 'all', writesOnly: false, errorsOnly: true, since: 0 }).map((i) => i.at)).toEqual([40, 10])
    expect(timeline(calls, events, { show: 'events', writesOnly: false, errorsOnly: false, since: 20 }).map((i) => i.at)).toEqual([40])
  })
})

describe('updates', () => {
  const mb = 1024 * 1024

  it('says what an update is doing', () => {
    expect(updatePhaseText('idle', null)).toBeNull()
    expect(updatePhaseText('checking', null)).toBe('Looking for a newer release…')
    expect(updatePhaseText('downloading', null, '0.6.0')).toBe('Downloading 0.6.0…')
    expect(updatePhaseText('downloading', { received: 0, total: 0 })).toBe('Downloading…')
    expect(updatePhaseText('downloading', { received: 12.34 * mb, total: 28 * mb }, '0.6.0')).toBe('Downloading 0.6.0: 12.3 of 28.0 MB')
    expect(updatePhaseText('verifying', null)).toContain('SHA-256')
    expect(updatePhaseText('installing', null)).toBe('Installing…')
    expect(updatePhaseText('restarting', null)).toBe('Restarting Workbench…')
  })

  it('measures only a download of a known size', () => {
    expect(updateFraction('downloading', { received: 7 * mb, total: 28 * mb })).toBe(0.25)
    expect(updateFraction('downloading', { received: 30 * mb, total: 28 * mb })).toBe(1)
    expect(updateFraction('downloading', { received: 5, total: 0 })).toBeNull()
    expect(updateFraction('downloading', null)).toBeNull()
    expect(updateFraction('installing', { received: 1, total: 2 })).toBeNull()
  })

  it('says what a restart stops', () => {
    const agent = { kind: 'agent' as const, working: false }
    const working = { kind: 'agent' as const, working: true }
    const shell = { kind: 'shell' as const, working: false }
    const run = { kind: 'run' as const, working: false }
    const command = { kind: 'command' as const, working: false }
    expect(restartImpact([], true)).toBe('Nothing is running in its terminals. Workbench is back in a few seconds.')
    expect(restartImpact([agent], true)).toBe('1 agent session will stop. Agent sessions resume after the restart.')
    expect(restartImpact([working, working, agent, shell, run, command], true)).toBe(
      '3 agent sessions, 1 shell and 2 runs will stop (2 agents are working right now). Agent sessions resume after the restart; shells start again under their last screen, without what ran in them; runs are not started again.',
    )
    expect(restartImpact([working], false)).toBe(
      '1 agent session will stop (1 agent is working right now). Agent sessions are not resumed (agents.restore_on_start is off) but stay in the history.',
    )
    expect(restartImpact([shell, shell], true)).toBe('2 shells will stop. Shells start again under their last screen, without what ran in them.')
    expect(restartImpact([agent, run], true)).toBe('1 agent session and 1 run will stop. Agent sessions resume after the restart; runs are not started again.')
  })
})

describe('accounts', () => {
  it('suggests a provider name from the kind and the label', () => {
    expect(suggestAccountId('claude', 'Work')).toBe('claude-work')
    expect(suggestAccountId('codex', '  Team A / 2 ')).toBe('codex-team-a-2')
    expect(suggestAccountId('kimi', '')).toBe('')
    expect(suggestAccountId('claude', '日本')).toBe('')
    expect(suggestAccountId('claude', 'x'.repeat(60))).toHaveLength(32)
    expect(accountIdError(suggestAccountId('claude', 'x'.repeat(60)), [])).toBeNull()
  })

  it('refuses names the server would, and the built-in ones', () => {
    expect(accountIdError('claude-work', [])).toBeNull()
    expect(accountIdError('', [])).toMatch(/Enter/)
    expect(accountIdError('Claude Work', [])).toMatch(/Lowercase/)
    expect(accountIdError('-x', [])).toMatch(/Lowercase/)
    expect(accountIdError('codex', [])).toMatch(/built-in/)
    expect(accountIdError('claude-work', ['claude-work'])).toMatch(/exists/)
  })

  it('wants a folder of its own, spelled as a path', () => {
    const others = [{ id: 'claude-work', home: '~/.claude-work/' }]
    expect(accountHomeError('claude', '~/.claude-personal', others)).toBeNull()
    expect(accountHomeError('claude', '/home/me/.claude-x', others)).toBeNull()
    expect(accountHomeError('claude', 'C:\\Users\\me\\.claude-x', others)).toBeNull()
    expect(accountHomeError('claude', '', others)).toMatch(/Enter/)
    expect(accountHomeError('claude', '.claude-x', others)).toMatch(/absolute/)
    expect(accountHomeError('claude', '~/.claude-work', others)).toMatch(/claude-work/)
    expect(accountHomeError('claude', '~/.claude/', others)).toMatch(/default account/)
    expect(accountHomeError('codex', '~/.claude', [])).toBeNull()
    expect(accountHomeError('codex', '~/${secret:x}', [])).toMatch(/plain path/)
  })

  it('knows Gemini’s home holds .gemini and Aider’s is a keys file', () => {
    expect(accountHomeError('gemini', '~/.gemini-work', [])).toBeNull()
    expect(accountHomeError('gemini', '~/', [])).toMatch(/default account/)
    expect(accountHomeError('aider', '~/.aider-work.env', [])).toBeNull()
    expect(accountHomeError('aider', '~/.aider-work.env', [{ id: 'aider-a', home: '~/.aider-work.env' }])).toBe('Already the keys file of “aider-a”')
    expect(accountHomeError('aider', '~/keys/', [])).toMatch(/file/)
    expect(accountHomeError('aider', '', [])).toMatch(/\.env file/)
    expect(accountIdError('gemini', [])).toMatch(/built-in/)
    expect(accountIdError('aider', [])).toMatch(/built-in/)
    expect(accountKind('aider')?.suggest('work')).toBe('~/.aider-work.env')
    expect(accountKind('gemini')?.homeVar).toBe('GEMINI_CLI_HOME')
    expect(accountKind('custom')).toBeUndefined()
  })
})
