// Repositories of a project (docs/ARCHITECTURE.md "Repositories of a project").
//
// The git, GitLab and GitHub views work on ONE repository at a time. They address it by a
// *scope id*: the project id for the project's default repository (so a project with one
// repository sees no change anywhere), `<project id>::<repository id>` for another. Project
// ids are slugs and never hold `:`. A scope id is what those features pass around as their
// "projectId": it keys their queries and panels, URL builders split it into
// `/api/projects/{pid}/…?repo=<id>`, and whatever needs the real project (opening a file,
// asking an agent) takes `scopeProject(scope)`.

import { create } from 'zustand'
import { createJSONStorage, persist, type StateStorage } from 'zustand/middleware'
import { ApiError } from './client'
import type { ProjectSummary, RepoSummary } from './types'

/** Id of the repository that holds the project root. */
export const ROOT_REPO = '.'

const SEP = '::'

/** A repository as git, GitLab and GitHub views address it (see the file comment). */
export type Scope = string

/** The scope of repository `repo` of a project; `null` (or empty) is the default repository. */
export function scopeOf(projectId: string, repo: string | null | undefined): Scope {
  return repo ? `${projectId}${SEP}${repo}` : projectId
}

export function splitScope(scope: Scope): { projectId: string; repo: string | null } {
  const i = scope.indexOf(SEP)
  if (i < 0) return { projectId: scope, repo: null }
  return { projectId: scope.slice(0, i), repo: scope.slice(i + SEP.length) || null }
}

/** The real project id a scope belongs to. */
export function scopeProject(scope: Scope): string {
  return splitScope(scope).projectId
}

/** The repository id of a scope; `null`: the default repository. */
export function scopeRepo(scope: Scope): string | null {
  return splitScope(scope).repo
}

/** `''` for the default repository, else `?repo=<encoded id>`. */
export function scopeQuery(scope: Scope): string {
  const repo = scopeRepo(scope)
  return repo ? `?repo=${encodeURIComponent(repo)}` : ''
}

/** `url` with the scope's repository added as the `repo` query parameter (`url` may already have a query). */
export function withRepo(url: string, scope: Scope): string {
  const repo = scopeRepo(scope)
  if (!repo) return url
  return `${url}${url.includes('?') ? '&' : '?'}repo=${encodeURIComponent(repo)}`
}

/** Does `key` (a query key) belong to project `projectId`, whichever repository its scope names? */
export function inProject(key: unknown, projectId: string): boolean {
  return typeof key === 'string' && (key === projectId || key.startsWith(projectId + SEP))
}

// ---------------------------------------------------------------- events

/** The repository id an event's `data` names (`data.repo`), if it does. */
export function eventRepo(data: unknown): string | null {
  const repo = (data as { repo?: unknown } | null | undefined)?.repo
  return typeof repo === 'string' && repo ? repo : null
}

/** The scope an event is about: its project's repository `data.repo`, or the default one. */
export function scopeOfEvent(projectId: string, data: unknown): Scope {
  return scopeOfRepo(projectId, knownRepos(projectId), eventRepo(data))
}

/**
 * Does a query key's scope (`key[1]`) belong to what an event is about? The one repository
 * `data.repo` names, else (an event with none) every repository of the project.
 */
export function eventMatches(key: unknown, projectId: string, data: unknown): boolean {
  return eventRepo(data) ? key === scopeOfEvent(projectId, data) : inProject(key, projectId)
}

// ---------------------------------------------------------------- pure helpers over a project's repositories

export function defaultRepo(repos: readonly RepoSummary[]): RepoSummary | null {
  return repos.find((r) => r.default) ?? repos[0] ?? null
}

export function isDefaultRepo(repos: readonly RepoSummary[], id: string): boolean {
  return defaultRepo(repos)?.id === id
}

/** The scope of repository `id`: the plain project id when it is the default one (or unknown). */
export function scopeOfRepo(projectId: string, repos: readonly RepoSummary[], id: string | null | undefined): Scope {
  if (!id || isDefaultRepo(repos, id) || !repos.some((r) => r.id === id)) return projectId
  return scopeOf(projectId, id)
}

/**
 * The repository that holds a project-relative path: the deepest one whose directory
 * strictly contains it, which is how the git slice resolves a path too (a nested
 * repository's own directory is an entry of the repository above it: a submodule). The
 * root repository `.` holds whatever no nested one does.
 */
export function repoOfPath(repos: readonly RepoSummary[], path: string): RepoSummary | null {
  const rel = path.replace(/^\.\//, '')
  let best: RepoSummary | null = null
  let bestLen = -1
  for (const r of repos) {
    const len = r.id === ROOT_REPO ? 0 : rel.startsWith(r.id + '/') ? r.id.length + 1 : -1
    if (len > bestLen) {
      best = r
      bestLen = len
    }
  }
  return best
}

/**
 * The `repo` query value for a request about one project-relative file: the repository
 * that holds it, `undefined` for the default one (the server's own default).
 */
export function repoParam(repos: readonly RepoSummary[], path: string): string | undefined {
  const r = repoOfPath(repos, path)
  return r && !isDefaultRepo(repos, r.id) ? r.id : undefined
}

/** The scope of the repository that holds a project-relative file. */
export function scopeOfPath(projectId: string, repos: readonly RepoSummary[], path: string): Scope {
  return scopeOf(projectId, repoParam(repos, path))
}

export type Forge = 'gitlab' | 'github'

/** Where the repository a scope names is hosted on `forge`, according to the project list (`null`: not there). */
export function repoForge(project: ProjectSummary | null | undefined, scope: Scope, forge: Forge): { host: string; path: string } | null {
  if (!project) return null
  const repos = project.repos ?? []
  // A payload without repositories (an older server): the project's own.
  if (!repos.length) return project[forge]
  const id = scopeRepo(scope)
  return ((id ? repos.find((r) => r.id === id) : undefined) ?? defaultRepo(repos))?.[forge] ?? null
}

/** Is any repository of the project on `forge`? (Whether the forge's tool windows and tabs apply.) */
export function anyRepoOn(project: ProjectSummary | null | undefined, forge: Forge): boolean {
  if (!project) return false
  return project.repos?.length ? project.repos.some((r) => !!r[forge]) : !!project[forge]
}

/** The repository a scope names (the default one for a plain project id). */
export function repoOfScope(repos: readonly RepoSummary[], scope: Scope): RepoSummary | null {
  const id = scopeRepo(scope)
  return (id ? repos.find((r) => r.id === id) : undefined) ?? defaultRepo(repos)
}

/** The repository `activeId` names, else the default one (a repository that disappeared). */
export function resolveRepo(repos: readonly RepoSummary[], activeId: string | null | undefined): RepoSummary | null {
  return (activeId ? repos.find((r) => r.id === activeId) : undefined) ?? defaultRepo(repos)
}

// ---------------------------------------------------------------- what the project list says (for code outside React)

const known = new Map<string, readonly RepoSummary[]>()

/** Remember each project's repositories; `useProjects` calls it with every fresh list. */
export function noteProjects(projects: readonly ProjectSummary[]) {
  known.clear()
  for (const p of projects) known.set(p.id, p.repos ?? [])
}

/** The repositories of a project as of the last project list (empty when unknown). */
export function knownRepos(projectId: string): readonly RepoSummary[] {
  return known.get(projectId) ?? []
}

// ---------------------------------------------------------------- the active repository per project

interface ActiveRepoState {
  /** Project id → repository id, only for repositories other than the default one. */
  active: Record<string, string>
  set: (projectId: string, repoId: string | null) => void
}

/** localStorage that never throws (blocked or full storage): the choice then lasts until the page closes. */
const safeStorage: StateStorage = {
  getItem: (k) => {
    try {
      return globalThis.localStorage.getItem(k)
    } catch {
      return null
    }
  },
  setItem: (k, v) => {
    try {
      globalThis.localStorage.setItem(k, v)
    } catch {
      /* blocked or full */
    }
  },
  removeItem: (k) => {
    try {
      globalThis.localStorage.removeItem(k)
    } catch {
      /* blocked */
    }
  },
}

/** Which repository each project's git and CI views are on (per browser). */
export const useActiveRepoStore = create<ActiveRepoState>()(
  persist(
    (set) => ({
      active: {},
      set: (projectId, repoId) =>
        set((s) => {
          const active = { ...s.active }
          if (repoId) active[projectId] = repoId
          else delete active[projectId]
          return { active }
        }),
    }),
    { name: 'wb.repo.v1', storage: createJSONStorage(() => safeStorage) },
  ),
)

/** Choose the repository of `projectId` (the default one resets the choice). */
export function setActiveRepo(projectId: string, repoId: string | null) {
  useActiveRepoStore.getState().set(projectId, repoId && isDefaultRepo(knownRepos(projectId), repoId) ? null : repoId)
}

/** The scope of the repository that holds `path`, as of the last project list (code outside React). */
export function scopeOfFile(projectId: string, path: string): Scope {
  return scopeOfPath(projectId, knownRepos(projectId), path)
}

/** The directory of the scope's repository, relative to the project root; `''` for the root repository. */
export function repoDir(scope: Scope): string {
  const { projectId, repo } = splitScope(scope)
  const id = repo ?? defaultRepo(knownRepos(projectId))?.id ?? ROOT_REPO
  return id === ROOT_REPO ? '' : id
}

/**
 * A path a forge reports (relative to its repository) as a project-relative path, the kind
 * the editor and the files tree use: a repository below the project root adds its directory.
 */
export function fileInProject(scope: Scope, repoPath: string): string {
  const dir = repoDir(scope)
  const path = repoPath.replace(/^\.\//, '').replace(/^\/+/, '')
  return dir ? `${dir}/${path}` : path
}

/** A sentence that tells an agent (whose session runs at the project root) which directory a scope's repository is. */
export function repoNote(scope: Scope): string {
  const dir = repoDir(scope)
  return dir ? `\n\n(This is about the repository in the \`${dir}\` directory of the project.)` : ''
}

/**
 * The scope a GitLab or GitHub command acts on: the active repository when it is on `forge`,
 * else the project's first repository that is (the commands are listed for the project).
 */
export function forgeScope(projectId: string, forge: Forge): Scope {
  const repos = knownRepos(projectId)
  const active = resolveRepo(repos, useActiveRepoStore.getState().active[projectId])
  const on = active?.[forge] ? active : (repos.find((r) => r[forge]) ?? active)
  return scopeOfRepo(projectId, repos, on?.id)
}

/** The scope of the active repository of a project, for code outside React (commands, event handlers). */
export function activeScope(projectId: string): Scope {
  const repos = knownRepos(projectId)
  const chosen = resolveRepo(repos, useActiveRepoStore.getState().active[projectId])
  return scopeOfRepo(projectId, repos, chosen?.id)
}

/**
 * A route answered `unknown_repo` for a query keyed by a scope (the repository is gone from
 * the project): the project goes back to its default repository. Returns the project id when
 * it did, so the caller can refresh the project list.
 */
export function resetUnknownRepo(error: unknown, queryKey: readonly unknown[]): string | null {
  if (!(error instanceof ApiError) || error.code !== 'unknown_repo') return null
  const scope = queryKey[1]
  if (typeof scope !== 'string' || !scopeRepo(scope)) return null
  const projectId = scopeProject(scope)
  setActiveRepo(projectId, null)
  return projectId
}
