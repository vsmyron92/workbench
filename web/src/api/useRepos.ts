// Hooks over a project's repositories (see `repos.ts`).

import { createElement, useEffect, type ComponentType } from 'react'
import { useProjects } from './queries'
import { resolveRepo, scopeOfRepo, setActiveRepo, useActiveRepoStore, type Scope } from './repos'
import type { RepoSummary } from './types'

const NONE: readonly RepoSummary[] = []

/** The repositories of a project, the default one first. */
export function useRepos(projectId: string | null | undefined): readonly RepoSummary[] {
  const projects = useProjects()
  return (projectId ? projects.data?.find((p) => p.id === projectId)?.repos : undefined) ?? NONE
}

/** The repository the project's git and CI views are on, and how to change it. */
export function useActiveRepo(projectId: string | null | undefined) {
  const repos = useRepos(projectId)
  const chosen = useActiveRepoStore((s) => (projectId ? s.active[projectId] : undefined))
  const repo = resolveRepo(repos, chosen)
  // A repository that is gone from the project: back to the default one.
  useEffect(() => {
    if (projectId && chosen && repos.length && !repos.some((r) => r.id === chosen)) setActiveRepo(projectId, null)
  }, [projectId, chosen, repos])
  return { repo, repos, set: (id: string | null) => projectId && setActiveRepo(projectId, id) }
}

/** The scope id of the project's active repository (the project id itself for the default one). */
export function useGitScope(projectId: string | null): Scope | null {
  const { repo, repos } = useActiveRepo(projectId)
  return projectId ? scopeOfRepo(projectId, repos, repo?.id) : null
}

/**
 * A tool window, widget or tab that gets the shell's project id: renders `Component` on the
 * project's active repository, with the scope id as its `projectId`.
 */
export function withGitScope<P extends { projectId: string | null }>(Component: ComponentType<P>): ComponentType<P> {
  function Scoped(props: P) {
    const scope = useGitScope(props.projectId)
    return createElement(Component, { ...props, projectId: scope })
  }
  Scoped.displayName = `Scoped(${Component.displayName ?? Component.name ?? 'Component'})`
  return Scoped
}
