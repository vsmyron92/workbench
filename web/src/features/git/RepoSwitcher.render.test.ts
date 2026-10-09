// The repository switcher, rendered on the server: only a project with several repositories
// has one, and the status bar names the repository the git items are about.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, describe, expect, it } from 'vitest'
import { qk } from '@/api/queries'
import { noteProjects, useActiveRepoStore } from '@/api/repos'
import type { ProjectSummary, RepoSummary } from '@/api/types'
import { gk } from './api'
import { RepoChip, RepoTopbarWidget } from './RepoSwitcher'
import { GitStatusbarWidget } from './Widgets'
import type { GitStatus } from './types'

const repo = (id: string, o: Partial<RepoSummary> = {}): RepoSummary => ({
  id,
  name: id === '.' ? 'shop' : id,
  path: id === '.' ? '' : id,
  default: false,
  remote: null,
  gitlab: null,
  github: null,
  ...o,
})

const status = (branch: string): GitStatus => ({
  branch,
  head: 'abcdef1234567890',
  upstream: null,
  ahead: 0,
  behind: 0,
  state: 'clean',
  stashes: 0,
  files: [],
  upstreamGone: false,
  stateDetail: {},
  truncated: false,
})

function render(repos: RepoSummary[], el: (qc: QueryClient) => ReturnType<typeof createElement>, cached?: { scope: string; branch: string }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retryOnMount: false } } })
  const projects = [{ id: 'shop', name: 'shop', repos } as ProjectSummary]
  qc.setQueryData(qk.projects, projects)
  noteProjects(projects)
  if (cached) qc.setQueryData(gk.status(cached.scope), status(cached.branch))
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc }, el(qc)))
}

const several = [repo('.', { default: true }), repo('services/api'), repo('web')]

afterEach(() => {
  noteProjects([])
  useActiveRepoStore.setState({ active: {} })
})

describe('repository switcher', () => {
  it('is a top bar button naming the active repository, for a project with several', () => {
    const html = render(several, () => createElement(RepoTopbarWidget, { projectId: 'shop' }))
    expect(html).toContain('git-repo-btn')
    expect(html).toContain('aria-haspopup="listbox"')
    expect(html).toContain('<span class="name">shop</span>')
  })

  it('shows nothing for a project with one repository, or none, or no project', () => {
    expect(render([repo('.', { default: true })], () => createElement(RepoTopbarWidget, { projectId: 'shop' }))).toBe('')
    expect(render([], () => createElement(RepoTopbarWidget, { projectId: 'shop' }))).toBe('')
    expect(render(several, () => createElement(RepoTopbarWidget, { projectId: null }))).toBe('')
    expect(render([repo('.', { default: true })], () => createElement(RepoChip, { scope: 'shop' }))).toBe('')
  })

  it('has a chip for the Commit and Git Log windows, a button or, in a panel, a label', () => {
    expect(render(several, () => createElement(RepoChip, { scope: 'shop::web' }))).toContain('<button class="git-repo-chip"')
    const label = render(several, () => createElement(RepoChip, { scope: 'shop::web', readOnly: true }))
    expect(label).toContain('<span class="git-repo-chip"')
    expect(label).toContain('web')
  })

  it('puts the repository before the branch in the status bar, only with several', () => {
    const bar = (repos: RepoSummary[]) => render(repos, () => createElement(GitStatusbarWidget, { projectId: 'shop::web' }), { scope: 'shop::web', branch: 'feature' })
    const html = bar(several)
    expect(html.indexOf('>web<')).toBeGreaterThan(-1)
    expect(html.indexOf('>web<')).toBeLessThan(html.indexOf('feature'))
    expect(bar([repo('.', { default: true })])).not.toContain('git-repo')
  })
})
