import { describe, expect, it } from 'vitest'
import {
  accountOf,
  bytesText,
  canActAlone,
  catalogState,
  compact,
  effectiveFilters,
  groupSkills,
  installChoices,
  installedCount,
  invokeText,
  isFiltering,
  listed,
  matches,
  matchesCatalog,
  needsAttention,
  NO_FILTERS,
  oneLine,
  scopeTitle,
  skillLink,
  statusWord,
  summarize,
  tokensText,
  worstIssue,
} from './logic'
import type { AgentReport, CatalogEntry, SkillInfo, SkillsReport } from './types'

const flags = { manualOnly: false, modelOnly: false, preapproves: [], hooks: false, forks: null, shell: false, scripts: false, paths: [], model: null, argumentHint: null }

function skill(over: Partial<SkillInfo> & { name: string }): SkillInfo {
  return {
    id: `s-${over.name}`,
    qualified: over.name,
    kind: 'skill',
    scope: 'user',
    description: `The ${over.name} skill`,
    descriptionFromText: false,
    agents: ['claude'],
    accounts: ['claude'],
    path: `~/.claude/skills/${over.name}`,
    invoke: [{ agent: 'claude', text: `/${over.name}` }],
    tokens: 20,
    bytes: 400,
    lines: 12,
    files: 0,
    flags,
    status: 'active',
    issues: [],
    ...over,
  }
}

const agent = (over: Partial<AgentReport> & { id: string }): AgentReport => ({
  cli: 'claude',
  label: 'Claude Code',
  installed: true,
  enabled: true,
  mcp: true,
  folder: '~/.claude',
  skills: 0,
  listingTokens: 0,
  ...over,
})

describe('numbers', () => {
  it('writes counts and sizes short', () => {
    expect([compact(0), compact(999), compact(1000), compact(1234), compact(9999), compact(12_345)]).toEqual(['0', '999', '1.0k', '1.2k', '10.0k', '12k'])
    expect([tokensText(1), tokensText(87), tokensText(2500)]).toEqual(['≈ 1 token', '≈ 87 tokens', '≈ 2.5k tokens'])
    expect([bytesText(12), bytesText(2048), bytesText(300 * 1024), bytesText(3 * 1024 * 1024)]).toEqual(['12 B', '2.0 KB', '300 KB', '3.0 MB'])
  })

  it('puts a description on one line, cut at a word', () => {
    expect(oneLine('Short one.')).toBe('Short one.')
    expect(oneLine('a\n  b   c')).toBe('a b c')
    expect(oneLine('word '.repeat(60), 40)).toMatch(/^word( word)*…$/)
    expect(oneLine('word '.repeat(60), 40).length).toBeLessThanOrEqual(41)
    expect(oneLine('x'.repeat(100), 40)).toBe(`${'x'.repeat(100).slice(0, 40)}…`)
  })
})

describe('filters', () => {
  const s = skill({ name: 'deploy-checks', description: 'Run the pre-deploy checks. Use when shipping.', scope: 'project', path: '.claude/skills/deploy-checks' })

  it('needs every word of the query somewhere in the skill', () => {
    expect(matches(s, { ...NO_FILTERS, query: '' })).toBe(true)
    expect(matches(s, { ...NO_FILTERS, query: 'deploy shipping' })).toBe(true)
    expect(matches(s, { ...NO_FILTERS, query: 'DEPLOY project' })).toBe(true)
    expect(matches(s, { ...NO_FILTERS, query: 'deploy nothing' })).toBe(false)
  })

  it('filters by CLI and by what needs a look', () => {
    expect(matches(s, { ...NO_FILTERS, clis: ['codex'] })).toBe(false)
    expect(matches(s, { ...NO_FILTERS, clis: ['codex', 'claude'] })).toBe(true)
    const bad = skill({ name: 'bad', issues: [{ level: 'warn', code: 'no_description', message: 'x' }] })
    const note = skill({ name: 'note', issues: [{ level: 'info', code: 'long_text', message: 'x' }] })
    expect([needsAttention(s), needsAttention(bad), needsAttention(note)]).toEqual([false, true, false])
    expect(matches(bad, { ...NO_FILTERS, attention: true }) && !matches(s, { ...NO_FILTERS, attention: true }) && !matches(note, { ...NO_FILTERS, attention: true })).toBe(true)
    expect(worstIssue(skill({ name: 'x', issues: [{ level: 'info', code: 'a', message: '1' }, { level: 'error', code: 'b', message: '2' }] }))?.code).toBe('b')
    expect(worstIssue(s)).toBeNull()
  })

  it('leaves "to review" off while nothing needs a review, so a filter left on cannot hide every skill', () => {
    const on = { ...NO_FILTERS, attention: true, query: 'deploy' }
    expect(effectiveFilters(on, 2)).toBe(on)
    const off = effectiveFilters(on, 0)
    expect(off).toEqual({ ...NO_FILTERS, query: 'deploy' })
    expect(matches(s, off)).toBe(true)
    expect(matches(s, on)).toBe(false)
    // The other filters are untouched, and a filter that is not on is returned as it is.
    expect(effectiveFilters(NO_FILTERS, 0)).toBe(NO_FILTERS)
    expect(isFiltering(NO_FILTERS)).toBe(false)
    expect([isFiltering({ ...NO_FILTERS, query: ' x ' }), isFiltering({ ...NO_FILTERS, query: '  ' }), isFiltering({ ...NO_FILTERS, clis: ['kimi'] }), isFiltering({ ...NO_FILTERS, attention: true })]).toEqual([true, false, true, true])
  })
})

describe('groups', () => {
  const agents = [agent({ id: 'claude', skills: 4 }), agent({ id: 'claude-work', label: 'Claude · Work', folder: '~/.claude-work', skills: 1 })]

  it('puts the project first, then each account\'s own, then plugins, claude.ai and the built-in ones', () => {
    const skills = [
      skill({ name: 'zeta', kind: 'builtin', scope: 'builtin', path: '' }),
      skill({ name: 'a', scope: 'synced', qualified: 'anthropic-skills:a', path: '~/.claude/skills/synced/b/a' }),
      skill({ name: 'p2', scope: 'plugin', scopeLabel: 'stripe', qualified: 'stripe:p2', path: '~/.claude/plugins/x' }),
      skill({ name: 'mine' }),
      skill({ name: 'theirs', path: '~/.claude-work/skills/theirs', accounts: ['claude-work'] }),
      skill({ name: 'cmd', kind: 'command', path: '~/.claude/commands/cmd.md' }),
      skill({ name: 'proj', scope: 'project', path: '.claude/skills/proj' }),
      skill({ name: 'deep', scope: 'nested', scopeLabel: 'apps/web', qualified: 'apps/web:deep', path: 'apps/web/.claude/skills/deep' }),
    ]
    const groups = groupSkills(skills, agents)
    expect(groups.map((g) => g.title)).toEqual(['Project', 'Personal · Claude Code', 'Personal · Claude · Work', 'Plugin · stripe', 'claude.ai', 'Built in'])
    expect(groups[0].skills.map((s) => s.name)).toEqual(['deep', 'proj'])
    // Skills before commands inside a group.
    expect(groups[1].skills.map((s) => s.name)).toEqual(['mine', 'cmd'])
  })

  it('names the personal group plainly when there is one account, and a shared folder by itself', () => {
    const one = [agent({ id: 'claude', skills: 2 })]
    const skills = [skill({ name: 'mine' }), skill({ name: 'shared', agents: ['codex', 'kimi'], accounts: ['codex', 'kimi'], path: '~/.agents/skills/shared' })]
    const groups = groupSkills(skills, one)
    expect(groups.map((g) => [g.title, g.hint])).toEqual([
      ['Personal', '~/.claude'],
      ['Personal · shared by agents', '~/.agents/skills'],
    ])
  })

  it('finds the account of a folder by whole path segments', () => {
    const agents2 = [agent({ id: 'a', folder: '~/.claude' }), agent({ id: 'b', folder: '~/.claude-work' })]
    expect(accountOf('~/.claude/skills/x', agents2)?.id).toBe('a')
    expect(accountOf('~/.claude-work/skills/x', agents2)?.id).toBe('b')
    expect(accountOf('~/.claudex/skills/x', agents2)).toBeUndefined()
    // A Windows server writes both with backslashes.
    const win = [agent({ id: 'a', folder: '~\\.claude' }), agent({ id: 'b', folder: '~\\.claude-work' })]
    expect(accountOf('~\\.claude\\skills\\x', win)?.id).toBe('a')
    expect(accountOf('~\\.claude-work\\skills\\x', win)?.id).toBe('b')
  })

  it('leaves Workbench\'s own skills to its catalog', () => {
    expect(listed([skill({ name: 'a' }), skill({ name: 'wb', workbench: 'wb' })]).map((s) => s.name)).toEqual(['a'])
  })
})

describe('summary', () => {
  const report = (skills: SkillInfo[], agents: AgentReport[]): SkillsReport => ({ projectId: 'p', generatedAt: 0, agents, skills, catalog: [], notes: [] })

  it('counts what can be used, what is off, what needs a look and what can act alone', () => {
    const r = report(
      [
        skill({ name: 'a' }),
        skill({ name: 'b', status: 'name-only' }),
        skill({ name: 'c', status: 'disabled' }),
        skill({ name: 'd', status: 'shadowed', issues: [{ level: 'info', code: 'shadowed', message: '' }] }),
        skill({ name: 'e', issues: [{ level: 'error', code: 'front_matter', message: '' }], flags: { ...flags, preapproves: ['Bash(*)'] } }),
        skill({ name: 'wb', workbench: 'wb', issues: [{ level: 'warn', code: 'x', message: '' }] }),
      ],
      [agent({ id: 'claude', skills: 4, listingTokens: 900 }), agent({ id: 'codex', cli: 'codex', label: 'Codex', installed: false, skills: 0 })],
    )
    const s = summarize(r)
    expect([s.usable, s.inactive, s.attention, s.powerful]).toEqual([4, 2, 1, 1])
    // A CLI that is not here and has no skills is not a row.
    expect(s.rows.map((x) => x.account.id)).toEqual(['claude'])
    expect(s.rows[0]).toMatchObject({ count: 4, tokens: 900, measured: false })
  })

  it('prefers what a session really received over the estimate', () => {
    const a = agent({ id: 'claude', skills: 3, listingTokens: 500, session: { count: 45, chars: 25_000, tokens: 6250, atMs: 1, version: '2.1.301' } })
    expect(summarize(report([], [a])).rows[0]).toMatchObject({ tokens: 6250, measured: true })
  })

  it('knows what can act without asking', () => {
    expect(canActAlone(skill({ name: 'x' }))).toBe(false)
    expect(canActAlone(skill({ name: 'x', flags: { ...flags, hooks: true } }))).toBe(true)
    expect(canActAlone(skill({ name: 'x', flags: { ...flags, shell: true } }))).toBe(true)
  })
})

describe('words', () => {
  it('says where a skill is from', () => {
    expect(scopeTitle(skill({ name: 'x', scope: 'project' }))).toBe('Project skill')
    expect(scopeTitle(skill({ name: 'x', scope: 'nested', scopeLabel: 'apps/web' }))).toBe('Nested skill · apps/web')
    expect(scopeTitle(skill({ name: 'x', scope: 'plugin', scopeLabel: 'stripe', kind: 'command' }))).toBe('Plugin command · stripe')
    expect(scopeTitle(skill({ name: 'x', scope: 'user', workbench: 'x' }))).toBe('Workbench skill, installed')
    expect(scopeTitle(skill({ name: 'x', scope: 'builtin', kind: 'builtin' }))).toBe('Built into Claude Code')
  })

  it('says what keeps a skill from being used', () => {
    expect(statusWord(skill({ name: 'x' }))).toBeNull()
    expect(statusWord(skill({ name: 'x', status: 'disabled' }))?.text).toBe('off')
    expect(statusWord(skill({ name: 'x', status: 'shadowed' }))?.text).toBe('hidden')
    expect(statusWord(skill({ name: 'x', status: 'name-only' }))?.text).toBe('name only')
    expect(statusWord(skill({ name: 'x', flags: { ...flags, manualOnly: true } }))?.text).toBe('manual')
  })

  it('names how to start a skill, Claude Code\'s way first', () => {
    expect(invokeText(skill({ name: 'x', invoke: [{ agent: 'codex', text: '$x' }, { agent: 'claude', text: '/x' }] }))).toBe('/x')
    expect(invokeText(skill({ name: 'x', invoke: [{ agent: 'codex', text: '$x' }] }))).toBe('$x')
    expect(invokeText(skill({ name: 'x', invoke: [] }))).toBeNull()
  })
})

describe('Workbench\'s own skills', () => {
  it('offers what the state allows, and asks before replacing edits', () => {
    expect(installChoices('available').map((c) => c.label)).toEqual(['Install'])
    expect(installChoices('installed').map((c) => c.label)).toEqual(['Remove'])
    expect(installChoices('update').map((c) => [c.label, c.run])).toEqual([['Update', 'install'], ['Remove', 'remove']])
    expect(installChoices('foreign')).toEqual([])
    const modified = installChoices('modified')
    expect(modified.every((c) => c.force && c.danger)).toBe(true)
    // Nothing that does not replace edits is forced or dangerous.
    expect(installChoices('available')[0]).toMatchObject({ force: false, danger: false })
  })

  it('counts the accounts that have it', () => {
    const e: CatalogEntry = {
      name: 'n',
      description: '',
      files: 1,
      bytes: 1,
      lines: 1,
      installs: [
        { account: 'a', state: 'installed', path: '' },
        { account: 'b', state: 'available', path: '' },
        { account: 'c', state: 'modified', path: '' },
        { account: 'd', state: 'foreign', path: '' },
      ],
    }
    expect(installedCount(e)).toBe(2)
  })
})

describe('the catalog rows', () => {
  const entry = (states: CatalogEntry['installs'][number]['state'][]): CatalogEntry => ({
    name: 'workbench',
    description: 'Use Workbench from an agent session',
    files: 1,
    bytes: 1,
    lines: 1,
    installs: states.map((state, i) => ({ account: `a${i}`, state, path: '' })),
  })

  it('summarizes the accounts in one word', () => {
    expect(catalogState(entry([]))?.text).toBe('no agent here')
    expect(catalogState(entry(['available']))).toBeNull()
    expect(catalogState(entry(['installed']))?.text).toBe('installed')
    expect(catalogState(entry(['installed', 'installed']))?.text).toBe('installed')
    expect(catalogState(entry(['installed', 'available']))?.text).toBe('installed 1 of 2')
    expect(catalogState(entry(['installed', 'update']))?.text).toBe('update')
    expect(catalogState(entry(['installed', 'modified', 'update']))?.text).toBe('edited')
    expect(catalogState(entry(['foreign']))?.text).toBe('name taken')
  })

  it('is left out of a filter about problems or about CLIs that get no Workbench tools', () => {
    const e = entry(['available'])
    expect(matchesCatalog(e, NO_FILTERS)).toBe(true)
    expect(matchesCatalog(e, { ...NO_FILTERS, query: 'optional agent' })).toBe(true)
    expect(matchesCatalog(e, { ...NO_FILTERS, query: 'nothing' })).toBe(false)
    expect(matchesCatalog(e, { ...NO_FILTERS, attention: true })).toBe(false)
    expect(matchesCatalog(e, { ...NO_FILTERS, clis: ['kimi'] })).toBe(false)
    expect(matchesCatalog(e, { ...NO_FILTERS, clis: ['kimi', 'codex'] })).toBe(true)
  })
})

describe('links inside a skill', () => {
  const known = new Set(['SKILL.md', 'references/a.md', 'references/deep/b.md', 'scripts/run.sh'])

  it('resolves a relative link against the file it is in, and only to files the skill has', () => {
    expect(skillLink('SKILL.md', 'references/a.md', known)).toBe('references/a.md')
    expect(skillLink('SKILL.md', './references/a.md#top', known)).toBe('references/a.md')
    expect(skillLink('references/a.md', 'deep/b.md?x=1', known)).toBe('references/deep/b.md')
    expect(skillLink('references/deep/b.md', '../a.md', known)).toBe('references/a.md')
    expect(skillLink('references/a.md', '../SKILL.md', known)).toBe('SKILL.md')
    expect(skillLink('SKILL.md', 'references/missing.md', known)).toBeNull()
  })

  it('leaves everything else to the browser, and never throws on a hostile href', () => {
    for (const href of ['', '#section', 'https://example.com/x', 'mailto:a@b.c', 'javascript:alert(1)', '/etc/passwd', '\\\\host\\share', 'C:\\x', '../../outside.md', '%E0%A4%A', 'references/%zz']) {
      expect(skillLink('SKILL.md', href, known), href).toBeNull()
    }
    // Climbing out of the skill's folder never lands on a file of it, even when the name matches one.
    expect(skillLink('SKILL.md', '../SKILL.md', known)).toBeNull()
    expect(skillLink('references/a.md', '../../SKILL.md', known)).toBeNull()
  })
})
