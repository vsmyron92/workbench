import { describe, expect, it } from 'vitest'
import { dedupeByTitle, matchShortcut, rankCommands, shortcutsMayHandle } from './paletteSearch'
import type { ProjectSummary } from '@/api/types'
import type { Command } from './types'
import { activeTab, visibleFor } from './visibility'

const cmd = (id: string, title: string, extra: Partial<Command> = {}): Command => ({ id, title, run: () => {}, ...extra })

// Registration order as in the real palette: agents and git come before Workbench.
const COMMANDS = [
  cmd('agents.home', 'Agents home', { group: 'Agents' }),
  cmd('agents.newShell', 'New shell', { group: 'Agents' }),
  cmd('agents.resume', 'Resume agent session…', { group: 'Agents' }),
  cmd('platform.remote', 'Start Remote Control server', { group: 'Agents' }),
  cmd('git.branches', 'Branches…', { group: 'Git' }),
  cmd('git.commit', 'Commit…', { group: 'Git', shortcut: 'alt+0' }),
  cmd('files.goto', 'Go to File…', { group: 'Files', shortcut: 'mod+p' }),
  cmd('platform.settings', 'Settings', { group: 'Workbench', keywords: ['preferences', 'config'] }),
  cmd('core.toggleTheme', 'Switch to light theme', { group: 'Workbench' }),
]

describe('command palette ranking', () => {
  it('puts the command whose title matches first, whatever its group', () => {
    expect(rankCommands(COMMANDS, 'Settings')[0].id).toBe('platform.settings')
    expect(rankCommands(COMMANDS, 'settings')[0].id).toBe('platform.settings')
    expect(rankCommands(COMMANDS, 'commit')[0].id).toBe('git.commit')
    expect(rankCommands(COMMANDS, 'theme')[0].id).toBe('core.toggleTheme')
    expect(rankCommands(COMMANDS, 'go to')[0].id).toBe('files.goto')
  })

  it('matches keywords and keeps everything when the query is empty', () => {
    expect(rankCommands(COMMANDS, 'preferences')[0].id).toBe('platform.settings')
    expect(rankCommands(COMMANDS, '  ')).toBe(COMMANDS)
    expect(rankCommands(COMMANDS, 'zzzzqq')).toEqual([])
  })

  it('drops generated commands a feature already provides', () => {
    const out = dedupeByTitle([cmd('git.log', 'Show Git Log', { shortcut: 'alt+9' })], [cmd('core.tw.gitlog', 'Show Git Log'), cmd('core.tw.files', 'Show Files')])
    expect(out.map((c) => c.id)).toEqual(['git.log', 'core.tw.files'])
  })
})

describe('shortcut routing', () => {
  const key = (k: string, mods: Partial<KeyboardEvent> = {}) =>
    ({ key: k, code: `Key${k.toUpperCase()}`, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...mods }) as KeyboardEvent

  it('matches modifiers exactly', () => {
    expect(matchShortcut(key('t', { ctrlKey: true }), 'mod+t', false)).toBe(true)
    expect(matchShortcut(key('t', { ctrlKey: true, shiftKey: true }), 'mod+t', false)).toBe(false)
    expect(matchShortcut(key('t'), 'mod+t', false)).toBe(false)
  })

  it('never takes keys typed into a terminal, or keys a widget handled', () => {
    const inTerminal = { closest: (s: string) => (s === '.xterm' ? {} : null) }
    const elsewhere = { closest: () => null }
    // Ctrl+T in a shell is transpose-chars (and Claude Code's task list), not `git pull`.
    expect(shortcutsMayHandle({ defaultPrevented: false, target: inTerminal as unknown as EventTarget })).toBe(false)
    expect(shortcutsMayHandle({ defaultPrevented: false, target: elsewhere as unknown as EventTarget })).toBe(true)
    // Monaco's own Ctrl+Shift+A (ask agent about the selection) wins.
    expect(shortcutsMayHandle({ defaultPrevented: true, target: elsewhere as unknown as EventTarget })).toBe(false)
    expect(shortcutsMayHandle({ defaultPrevented: false, target: inTerminal as unknown as EventTarget }, { global: true })).toBe(true)
    expect(shortcutsMayHandle({ defaultPrevented: false, target: null })).toBe(true)
  })
})

describe('per-project tool windows and phone tabs', () => {
  const project = (extra: Partial<ProjectSummary>) => ({ id: 'p', name: 'p', gitlab: null, github: null, ...extra }) as unknown as ProjectSummary
  const tabs = [
    { id: 'agents' },
    { id: 'ci', when: (p: ProjectSummary | null) => !!p?.gitlab },
    { id: 'github', when: (p: ProjectSummary | null) => !!p?.github },
  ]
  const onGitlab = project({ gitlab: { host: 'gitlab.com', path: 'g/p' } } as Partial<ProjectSummary>)
  const onGithub = project({ github: { host: 'github.com', path: 'o/r' } } as Partial<ProjectSummary>)

  it('shows only what applies to the project', () => {
    expect(visibleFor(tabs, onGithub).map((t) => t.id)).toEqual(['agents', 'github'])
    expect(visibleFor(tabs, onGitlab).map((t) => t.id)).toEqual(['agents', 'ci'])
    expect(visibleFor(tabs, project({})).map((t) => t.id)).toEqual(['agents'])
    expect(visibleFor(tabs, null).map((t) => t.id)).toEqual(['agents'])
  })

  it('falls back to the first visible tab when the saved one does not apply', () => {
    const visible = visibleFor(tabs, onGitlab)
    expect(activeTab(visible, 'github')?.id).toBe('agents')
    expect(activeTab(visible, 'ci')?.id).toBe('ci')
    expect(activeTab([], 'ci')).toBeUndefined()
  })
})
