import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from './client'
import {
  activeScope,
  anyRepoOn,
  eventMatches,
  fileInProject,
  forgeScope,
  inProject,
  isDefaultRepo,
  knownRepos,
  noteProjects,
  repoDir,
  repoForge,
  repoNote,
  repoOfPath,
  repoParam,
  resetUnknownRepo,
  resolveRepo,
  scopeOf,
  scopeOfEvent,
  scopeOfFile,
  scopeOfRepo,
  scopeProject,
  scopeQuery,
  setActiveRepo,
  splitScope,
  useActiveRepoStore,
  withRepo,
} from './repos'
import type { ProjectSummary, RepoSummary } from './types'

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

const gl = { host: 'gitlab.com', path: 'acme/x' }
const REPOS = [repo('.', { default: true, gitlab: gl }), repo('services/api', { github: { host: 'github.com', path: 'acme/api' } }), repo('web')]

const project = (id: string, repos: RepoSummary[], extra: Partial<ProjectSummary> = {}): ProjectSummary =>
  ({ id, name: id, repos, gitlab: null, github: null, ...extra }) as ProjectSummary

afterEach(() => {
  noteProjects([])
  useActiveRepoStore.setState({ active: {} })
})

describe('scope ids', () => {
  it('are the project id for the default repository and project::repo for another', () => {
    expect(scopeOf('shop', null)).toBe('shop')
    expect(scopeOf('shop', '')).toBe('shop')
    expect(scopeOf('shop', 'services/api')).toBe('shop::services/api')
    expect(splitScope('shop')).toEqual({ projectId: 'shop', repo: null })
    expect(splitScope('shop::services/api')).toEqual({ projectId: 'shop', repo: 'services/api' })
    expect(splitScope('shop::')).toEqual({ projectId: 'shop', repo: null })
    expect(scopeProject('shop::web')).toBe('shop')
  })

  it('add the repository to a URL as a query parameter, whatever the URL already has', () => {
    expect(scopeQuery('shop')).toBe('')
    expect(scopeQuery('shop::services/api')).toBe('?repo=services%2Fapi')
    expect(withRepo('/api/projects/shop/git/log', 'shop')).toBe('/api/projects/shop/git/log')
    expect(withRepo('/api/projects/shop/git/log', 'shop::web')).toBe('/api/projects/shop/git/log?repo=web')
    expect(withRepo('/api/projects/shop/git/log?limit=5', 'shop::web')).toBe('/api/projects/shop/git/log?limit=5&repo=web')
    expect(withRepo('/api/projects/shop/git/log', 'shop::a b/c')).toBe('/api/projects/shop/git/log?repo=a%20b%2Fc')
  })

  it('group the query keys of every repository of a project', () => {
    expect(inProject('shop', 'shop')).toBe(true)
    expect(inProject('shop::web', 'shop')).toBe(true)
    expect(inProject('shop-2', 'shop')).toBe(false)
    expect(inProject('shop-2::web', 'shop')).toBe(false)
    expect(inProject(undefined, 'shop')).toBe(false)
  })
})

describe('repositories of a project', () => {
  it('finds the deepest repository that holds a path, like the server', () => {
    const nested = [...REPOS, repo('services/api/vendor')]
    const of = (p: string) => repoOfPath(nested, p)?.id
    expect(of('README.md')).toBe('.')
    expect(of('./README.md')).toBe('.')
    expect(of('services/api/src/main.rs')).toBe('services/api')
    // A nested repository's own folder is an entry of the one above it (a submodule's gitlink).
    expect(of('services/api')).toBe('.')
    expect(of('services/api/vendor/x.c')).toBe('services/api/vendor')
    expect(of('services/other.txt')).toBe('.')
    expect(of('web-extra/x')).toBe('.')
    expect(of('web/index.html')).toBe('web')
    expect(repoOfPath([], 'x')).toBeNull()
    // A folder of repositories that is none itself: a path outside all of them has none.
    expect(repoOfPath([repo('a'), repo('b')], 'c/x')).toBeNull()
  })

  it('knows the default repository and falls back to it for one that is gone', () => {
    expect(isDefaultRepo(REPOS, '.')).toBe(true)
    expect(isDefaultRepo(REPOS, 'web')).toBe(false)
    // The default is the first one when none says so (a folder of repositories).
    expect(resolveRepo([repo('a'), repo('b')], null)?.id).toBe('a')
    expect(resolveRepo(REPOS, 'web')?.id).toBe('web')
    expect(resolveRepo(REPOS, 'gone')?.id).toBe('.')
    expect(resolveRepo([], 'web')).toBeNull()
  })

  it('maps a repository to its scope and a file to the scope of its repository', () => {
    expect(scopeOfRepo('shop', REPOS, '.')).toBe('shop')
    expect(scopeOfRepo('shop', REPOS, 'web')).toBe('shop::web')
    expect(scopeOfRepo('shop', REPOS, 'gone')).toBe('shop')
    expect(scopeOfRepo('shop', REPOS, null)).toBe('shop')
    expect(repoParam(REPOS, 'src/a.rs')).toBeUndefined()
    expect(repoParam(REPOS, 'services/api/src/a.rs')).toBe('services/api')
    noteProjects([project('shop', REPOS)])
    expect(scopeOfFile('shop', 'web/index.html')).toBe('shop::web')
    expect(scopeOfFile('shop', 'README.md')).toBe('shop')
  })

  it('turns a path the forge reports into a project path, and tells agents where the repository is', () => {
    noteProjects([project('shop', REPOS), project('farm', [repo('a', { default: true }), repo('b')])])
    expect(fileInProject('shop', './spec/a_spec.rb')).toBe('spec/a_spec.rb')
    expect(fileInProject('shop::services/api', 'src/main.rs')).toBe('services/api/src/main.rs')
    expect(repoDir('shop')).toBe('')
    expect(repoNote('shop')).toBe('')
    expect(repoNote('shop::web')).toContain('`web`')
    // A folder of repositories: the default one is below the root too.
    expect(fileInProject('farm', 'x.c')).toBe('a/x.c')
    expect(repoDir('farm::b')).toBe('b')
  })
})

describe('forges of a repository', () => {
  const p = project('shop', REPOS)

  it('reads the forge of the repository a scope names', () => {
    expect(repoForge(p, 'shop', 'gitlab')).toEqual(gl)
    expect(repoForge(p, 'shop', 'github')).toBeNull()
    expect(repoForge(p, 'shop::services/api', 'gitlab')).toBeNull()
    expect(repoForge(p, 'shop::services/api', 'github')?.path).toBe('acme/api')
    expect(repoForge(undefined, 'shop', 'gitlab')).toBeNull()
  })

  it('tells whether any repository is on a forge (the tool windows’ and tabs’ `when`)', () => {
    expect(anyRepoOn(p, 'gitlab')).toBe(true)
    expect(anyRepoOn(p, 'github')).toBe(true)
    expect(anyRepoOn(project('x', [repo('.', { default: true })]), 'gitlab')).toBe(false)
    expect(anyRepoOn(null, 'gitlab')).toBe(false)
    // A payload without repositories keeps the project's own answer.
    expect(anyRepoOn(project('old', [], { gitlab: gl }), 'gitlab')).toBe(true)
    expect(anyRepoOn(project('old', [], { gitlab: gl }), 'github')).toBe(false)
    expect(repoForge(project('old', [], { gitlab: gl }), 'old', 'gitlab')).toEqual(gl)
  })

  it('acts, for a command, on the active repository when it is on the forge, else the first that is', () => {
    noteProjects([p])
    expect(forgeScope('shop', 'gitlab')).toBe('shop')
    expect(forgeScope('shop', 'github')).toBe('shop::services/api')
    setActiveRepo('shop', 'services/api')
    expect(forgeScope('shop', 'github')).toBe('shop::services/api')
    expect(forgeScope('shop', 'gitlab')).toBe('shop')
    setActiveRepo('shop', 'web')
    expect(forgeScope('shop', 'github')).toBe('shop::services/api')
  })
})

describe('events', () => {
  it('name the repository in data.repo, and refresh every repository of the project without one', () => {
    noteProjects([project('shop', REPOS)])
    expect(scopeOfEvent('shop', { repo: 'web' })).toBe('shop::web')
    expect(scopeOfEvent('shop', { repo: '.' })).toBe('shop')
    expect(scopeOfEvent('shop', {})).toBe('shop')
    expect(scopeOfEvent('shop', null)).toBe('shop')
    expect(eventMatches('shop::web', 'shop', { repo: 'web' })).toBe(true)
    expect(eventMatches('shop', 'shop', { repo: 'web' })).toBe(false)
    expect(eventMatches('shop', 'shop', { repo: '.' })).toBe(true)
    expect(eventMatches('shop::web', 'shop', {})).toBe(true)
    expect(eventMatches('shop', 'shop', undefined)).toBe(true)
    expect(eventMatches('other::web', 'shop', {})).toBe(false)
  })
})

describe('the active repository', () => {
  it('is kept per project, and choosing the default one clears the choice', () => {
    noteProjects([project('shop', REPOS)])
    expect(activeScope('shop')).toBe('shop')
    setActiveRepo('shop', 'web')
    expect(useActiveRepoStore.getState().active).toEqual({ shop: 'web' })
    expect(activeScope('shop')).toBe('shop::web')
    setActiveRepo('shop', '.')
    expect(useActiveRepoStore.getState().active).toEqual({})
    expect(activeScope('shop')).toBe('shop')
  })

  it('falls back to the default repository when the chosen one is gone', () => {
    noteProjects([project('shop', REPOS)])
    useActiveRepoStore.setState({ active: { shop: 'gone' } })
    expect(activeScope('shop')).toBe('shop')
    expect(knownRepos('nobody')).toEqual([])
  })

  it('goes back to the default when a route says the repository is unknown', () => {
    useActiveRepoStore.setState({ active: { shop: 'web' } })
    const unknown = new ApiError(404, 'unknown_repo', 'project "shop" has no repository "web"')
    expect(resetUnknownRepo(new ApiError(404, 'not_found', 'x'), ['git', 'shop::web', 'status'])).toBeNull()
    expect(resetUnknownRepo(unknown, ['git', 'shop', 'status'])).toBeNull()
    expect(resetUnknownRepo(unknown, ['projects'])).toBeNull()
    expect(useActiveRepoStore.getState().active).toEqual({ shop: 'web' })
    expect(resetUnknownRepo(unknown, ['git', 'shop::web', 'status'])).toBe('shop')
    expect(useActiveRepoStore.getState().active).toEqual({})
  })
})

describe('persistence', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.resetModules()
  })

  it('keeps the choice in localStorage under wb.repo.v1', async () => {
    const data = new Map<string, string>()
    vi.stubGlobal('localStorage', {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => void data.set(k, v),
      removeItem: (k: string) => void data.delete(k),
    })
    vi.resetModules()
    const fresh = await import('./repos')
    fresh.useActiveRepoStore.getState().set('shop', 'web')
    expect(JSON.parse(data.get('wb.repo.v1')!).state).toEqual({ active: { shop: 'web' } })
    // A new page load reads it back.
    vi.resetModules()
    const again = await import('./repos')
    expect(again.useActiveRepoStore.getState().active).toEqual({ shop: 'web' })
  })

  it('works without storage (a blocked browser)', async () => {
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('blocked')
      },
      setItem: () => {
        throw new Error('blocked')
      },
      removeItem: () => {},
    })
    vi.resetModules()
    const fresh = await import('./repos')
    fresh.useActiveRepoStore.getState().set('shop', 'web')
    expect(fresh.useActiveRepoStore.getState().active).toEqual({ shop: 'web' })
  })
})
