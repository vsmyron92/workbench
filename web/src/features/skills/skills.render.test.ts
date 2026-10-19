// The Skills tool window and the skill panel, rendered on the server from seeded queries: what a
// reader sees for a project's skills, a skill that needs a look, one that Workbench would not read,
// and one of Workbench's own that is not installed yet. (The filters keep their state in a zustand
// store, whose server snapshot is its initial state: they are tested in logic.test.ts.)

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { describe, expect, it } from 'vitest'
import { sk } from './api'
import { SkillPanel } from './SkillPanel'
import { SkillsToolWindow } from './SkillsToolWindow'
import type { AgentReport, SkillDetail, SkillFlags, SkillInfo, SkillsReport } from './types'

const flags: SkillFlags = { manualOnly: false, modelOnly: false, preapproves: [], hooks: false, forks: null, shell: false, scripts: false, paths: [], model: null, argumentHint: null }

function skill(over: Partial<SkillInfo> & { name: string }): SkillInfo {
  return {
    id: `s-${over.name}`,
    qualified: over.name,
    kind: 'skill',
    scope: 'project',
    description: `The ${over.name} skill. Use when asked about ${over.name}.`,
    descriptionFromText: false,
    agents: ['claude'],
    accounts: ['claude'],
    path: `~/workspace/shop/.claude/skills/${over.name}`,
    invoke: [{ agent: 'claude', text: `/${over.name}` }],
    tokens: 30,
    bytes: 900,
    lines: 20,
    files: 0,
    flags,
    status: 'active',
    issues: [],
    ...over,
  }
}

const claude: AgentReport = { id: 'claude', cli: 'claude', label: 'Claude Code', installed: true, enabled: true, mcp: true, folder: '~/.claude', skills: 4, listingTokens: 400 }

const report: SkillsReport = {
  projectId: 'shop',
  generatedAt: 1,
  agents: [claude],
  skills: [
    skill({ name: 'checkout-flow' }),
    skill({ name: 'broken', scope: 'user', path: '~/.claude/skills/broken', issues: [{ level: 'error', code: 'front_matter', message: 'the front matter is not valid YAML' }] }),
    skill({ name: 'pay', qualified: 'payments:pay', scope: 'plugin', scopeLabel: 'payments', path: '~/.claude/plugins/cache/x/pay', status: 'disabled', statusReason: 'the plugin payments is not enabled', tokens: 0 }),
    skill({ name: 'init', kind: 'builtin', scope: 'builtin', path: '', description: 'Start a CLAUDE.md' }),
  ],
  catalog: [
    { name: 'workbench', description: 'Routes work in a Workbench session to the right tool.', files: 2, bytes: 4000, lines: 90, installs: [{ account: 'claude', state: 'available', path: '~/.claude/skills/workbench' }] },
    { name: 'workbench-report-card', description: 'Hands reports over as Workspace cards.', files: 3, bytes: 9000, lines: 80, installs: [{ account: 'claude', state: 'installed', path: '~/.claude/skills/workbench-report-card' }] },
  ],
  notes: [],
}

function client(details: Record<string, SkillDetail> = {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  qc.setQueryData(sk.report('shop'), report)
  for (const [id, d] of Object.entries(details)) qc.setQueryData(sk.detail('shop', id), d)
  return qc
}

const render = (qc: QueryClient, el: ReturnType<typeof createElement>) => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc }, el))
const windowHtml = () => render(client(), createElement(SkillsToolWindow, { projectId: 'shop' }))
const panelHtml = (skillId: string, d: SkillDetail) =>
  render(client({ [skillId]: d }), createElement(SkillPanel, { id: `skill:shop:${skillId}`, params: { projectId: 'shop', skillId }, setParams: () => {}, setTitle: () => {}, close: () => {}, visible: true, active: true }))

describe('the Skills tool window', () => {
  it('counts what is available, off and to review, and lists the groups with their skills', () => {
    const html = windowHtml()
    // Three skills can be used (the plugin's is off): the project's, the broken one and the built-in one.
    expect(html).toContain('<b>3</b>')
    expect(html).toContain('skills available')
    expect(html).toContain('off or hidden')
    expect(html).toContain('to review')
    for (const group of ['Project', 'Personal', 'Plugin · payments', 'Built in', 'Workbench']) expect(html).toContain(group)
    // A plugin's skill is named without its namespace (the group says it); an off one says so.
    expect(html).toContain('>pay<')
    expect(html).toContain('>off<')
    expect(html).toContain('checkout-flow')
    expect(html).toContain('Start a CLAUDE.md')
    // The agent's row shows what its listing costs.
    expect(html).toContain('Claude Code')
    expect(html).toContain('400 tokens')
  })

  it('warns on a skill that needs a look and offers Workbench\'s own skills with their state', () => {
    const html = windowHtml()
    expect(html).toContain('the front matter is not valid YAML')
    expect(html).toContain('workbench-report-card')
    expect(html).toContain('>installed<')
    expect(html).toContain('optional')
    expect(html).toContain('Nothing is installed until you do')
  })
})

describe('the skill panel', () => {
  const found = (info: SkillInfo, over: Partial<SkillDetail> = {}): SkillDetail => ({ skill: info, text: '# Title\n\nBody.\n', truncated: false, files: [], filesTruncated: false, ...over })

  it('shows what a skill is for, where, who reads it, how to start it, what it costs and what it can do alone', () => {
    const html = panelHtml(
      's-power',
      found(skill({ name: 'power', flags: { ...flags, preapproves: ['Bash(git *)'], shell: true, scripts: true }, files: 2, whenToUse: 'when releasing', inSession: true }), {
        files: [
          { path: 'references', size: 0, dir: true },
          { path: 'references/a.md', size: 120, dir: false },
        ],
      }),
    )
    expect(html).toContain('The power skill. Use when asked about power.')
    expect(html).toContain('When to use:')
    expect(html).toContain('pre-approves 1 tool')
    expect(html).toContain('runs shell commands')
    expect(html).toContain('has scripts')
    expect(html).toContain('/power')
    expect(html).toContain('≈ 30 tokens in every session')
    expect(html).toContain('~/workspace/shop/.claude/skills/power')
    // The files and the text are there to read.
    expect(html).toContain('references')
    expect(html).toContain('a.md')
    expect(html).toContain('SKILL.md')
    expect(html).toContain('Rendered')
    expect(html).toContain('Source')
  })

  it('does not say a nested skill is in every session: it is listed once a file below its folder is read', () => {
    const html = panelHtml('s-deploy', found(skill({ name: 'deploy', qualified: 'apps/web:deploy', scope: 'nested', scopeLabel: 'apps/web', path: '~/workspace/shop/apps/web/.claude/skills/deploy', tokens: 25 })))
    expect(html).toContain('≈ 25 tokens, once an agent reads a file below apps/web')
    expect(html).not.toContain('tokens in every session')
  })

  it('says what is wrong with a skill, and does not call a broken file one with no description', () => {
    const html = panelHtml(
      's-broken',
      found(skill({ name: 'broken', description: 'First line', descriptionFromText: true, issues: [{ level: 'error', code: 'front_matter', message: 'the front matter is not valid YAML' }] })),
    )
    expect(html).toContain('the front matter is not valid YAML')
    expect(html).toContain('This is the first line of its text')
    expect(html).not.toContain('The file has no description')
  })

  it('says a skill that links out of the project was not read, and shows nothing it did not determine', () => {
    const html = panelHtml(
      's-linked',
      found(skill({ name: 'linked', description: '', issues: [{ level: 'warn', code: 'link_out', message: 'This skill is a link to a file outside the project.' }] }), { text: null }),
    )
    expect(html).toContain('Not read')
    expect(html).toContain('a link to a file outside the project')
    for (const word of ['Costs', 'Size', 'Last session', 'Rendered']) expect(html).not.toContain(word)
  })

  it('shows a built-in skill without files, and one of Workbench\'s own with what installing does', () => {
    const builtin = panelHtml('b-init', found(skill({ name: 'init', kind: 'builtin', scope: 'builtin', path: '', description: 'Start a CLAUDE.md' }), { text: null }))
    expect(builtin).toContain('Built into Claude Code')
    expect(builtin).toContain('or it was removed after that session began')
    expect(builtin).toContain('Start a CLAUDE.md')
    expect(builtin).not.toContain('Rendered')

    const own = panelHtml('wb:workbench', {
      catalog: report.catalog[0],
      text: '---\nname: workbench\n---\n# Working inside Workbench\n',
      truncated: false,
      files: [{ path: 'references/a.md', size: 100, dir: false }],
      filesTruncated: false,
    })
    expect(own).toContain('Workbench skill · optional')
    expect(own).toContain('Install for an agent account')
    expect(own).toContain('not installed')
    expect(own).toContain('>Install<')
    expect(own).toContain('~/.claude/skills/workbench')
    expect(own).toContain('in every session of an account that has it')
  })

  it('has nowhere to install when no agent that gets Workbench\'s tools is set up', () => {
    const none = panelHtml('wb:workbench', { catalog: { ...report.catalog[0], installs: [] }, text: '# x\n', truncated: false, files: [], filesTruncated: false })
    expect(none).toContain('neither is set up on this computer')
    expect(none).not.toContain('>Install<')
  })
})
