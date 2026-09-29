import { describe, expect, it } from 'vitest'
import { initials, issuePrompt, jqlFilters, lozengeTone, pagePrompt, parseIssueKey, parsePageRef, pushRecent, setupSummary, statusTone } from './links'
import type { ProjectConfig } from '@/api/types'

describe('references', () => {
  it('parses page ids and URLs', () => {
    expect(parsePageRef('65601')).toBe('65601')
    expect(parsePageRef(' https://x.atlassian.net/wiki/spaces/DESIGN/pages/65601/Design+Notes ')).toBe('65601')
    expect(parsePageRef('https://x.atlassian.net/wiki/spaces/D/pages/edit-v2/12?draftShareId=1')).toBe('12')
    expect(parsePageRef('https://x.atlassian.net/wiki/pages/viewpage.action?spaceKey=D&pageId=77')).toBe('77')
    expect(parsePageRef('hello')).toBeNull()
    expect(parsePageRef('')).toBeNull()
  })

  it('parses issue keys', () => {
    expect(parseIssueKey('abc-12')).toBe('ABC-12')
    expect(parseIssueKey('https://x.atlassian.net/browse/SHOP-7')).toBe('SHOP-7')
    expect(parseIssueKey('https://x.atlassian.net/jira/software/projects/A/boards/1?selectedIssue=A-3')).toBe('A-3')
    expect(parseIssueKey('ABC')).toBeNull()
    expect(parseIssueKey('12-ABC')).toBeNull()
  })
})

describe('jql filters', () => {
  it('always offers assigned-to-me', () => {
    expect(jqlFilters(null).map((f) => f.id)).toEqual(['mine', 'reported', 'watching'])
  })

  it('adds project filters from links.jira', () => {
    const config = { schema: 1, project: { id: 'p', name: 'p', root: '/' }, links: { jira: { site: '', project_keys: ['shop', 'x y'], jql: 'project = SHOP' } } } as ProjectConfig
    const f = jqlFilters(config)
    expect(f.map((x) => x.id)).toEqual(['mine', 'project', 'open', 'recent', 'reported', 'watching'])
    expect(f.find((x) => x.id === 'open')!.jql).toBe('project in (SHOP) AND statusCategory != Done ORDER BY updated DESC')
  })
})

describe('small helpers', () => {
  it('maps tones', () => {
    expect(statusTone('done')).toBe('success')
    expect(statusTone('indeterminate')).toBe('accent')
    expect(statusTone('new')).toBeUndefined()
    expect(lozengeTone('Green')).toBe('success')
    expect(lozengeTone('grey')).toBeUndefined()
  })

  it('builds agent prompts with the safety instructions', () => {
    const p = pagePrompt({ id: '1', title: 'GDD', version: 11, webUrl: 'https://s/wiki/1' }, '  Tighten section 4. ')
    expect(p).toContain('page id 1, version 11')
    expect(p).toContain('Tighten section 4.')
    expect(p).toContain('baseVersion=11')
    expect(p).toContain('<ac:inline-comment-marker>')
    const i = issuePrompt({ key: 'A-1', summary: 'Fix', webUrl: 'https://s/browse/A-1', descriptionMarkdown: 'x'.repeat(7000) })
    expect(i).toContain('Work on Jira issue A-1: Fix')
    expect(i).toContain('…(truncated')
  })

  it('makes initials and bounded recent lists', () => {
    expect(initials('Ada Lovelace')).toBe('AL')
    expect(initials('')).toBe('?')
    let list: { id: string }[] = []
    for (let i = 0; i < 20; i++) list = pushRecent(list, { id: String(i % 12) })
    expect(list.length).toBe(12)
    expect(list[0].id).toBe('7')
  })
})

describe('setup messages', () => {
  // As server/src/atlassian/client.rs SETUP_HELP words it.
  const help =
    'Add to config.toml:\n\n[atlassian]\nsite = "https://<your-site>.atlassian.net"\nemail = "you@example.com"\ntoken = "atlassian"\n\n[secrets]\natlassian = { file = "~/.atlassian_token" }'

  it('drops the server copy of the config snippet', () => {
    expect(setupSummary(`Atlassian is not set up. ${help}`)).toBe('Atlassian is not set up.')
    expect(setupSummary(`No Atlassian API token configured. ${help}`)).toBe('No Atlassian API token configured.')
    expect(setupSummary(help)).toBe('Atlassian is not set up.')
  })

  it('keeps other messages whole', () => {
    const m = 'secret "atlassian": cannot read ~/.atlassian_token: No such file or directory'
    expect(setupSummary(m)).toBe(m)
    expect(setupSummary('Atlassian rejected the credentials for me@x.dev (HTTP 401). Check [atlassian] email and the API token.')).toContain('HTTP 401')
    expect(setupSummary(null)).toBe('Atlassian is not set up.')
  })
})
