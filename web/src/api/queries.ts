// Queries used by the shell and by several features.

import { useQuery } from '@tanstack/react-query'
import type { QueryClient } from '@tanstack/react-query'
import { api } from './client'
import { subscribe } from './events'
import { noteProjects } from './repos'
import type { ProjectDetail, ProjectSummary, TerminalInfo } from './types'

export const qk = {
  projects: ['projects'] as const,
  project: (id: string) => ['project', id] as const,
  terminals: ['terminals'] as const,
}

/**
 * Once per app (not per `useProjects` caller): keep the project list and details
 * fresh. `git.changed` can move a branch shown in the list; bursts (a commit, a
 * rebase) coalesce into one refetch.
 */
export function installProjectsSync(qc: QueryClient): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined
  const offs = [
    subscribe('projects.changed', () => {
      void qc.invalidateQueries({ queryKey: qk.projects })
      void qc.invalidateQueries({ queryKey: ['project'] })
    }),
    subscribe('git.changed', () => {
      clearTimeout(timer)
      timer = setTimeout(() => void qc.invalidateQueries({ queryKey: qk.projects }), 400)
    }),
  ]
  return () => {
    clearTimeout(timer)
    offs.forEach((o) => o())
  }
}

export function useProjects() {
  return useQuery({
    queryKey: qk.projects,
    queryFn: async ({ signal }) => {
      const projects = await api.get<ProjectSummary[]>('/api/projects', undefined, signal)
      noteProjects(projects) // code outside React (commands, event handlers) reads the repositories from here
      return projects
    },
  })
}

export function useProject(id: string | null | undefined) {
  return useQuery({
    queryKey: qk.project(id ?? ''),
    queryFn: ({ signal }) => api.get<ProjectDetail>(`/api/projects/${id}`, undefined, signal),
    enabled: !!id,
  })
}

/**
 * All terminals (agents, shells, runs, commands). The terminals slice keeps this
 * cache fresh from `terminal.*` events.
 */
export function useTerminals() {
  return useQuery({
    queryKey: qk.terminals,
    queryFn: ({ signal }) => api.get<TerminalInfo[]>('/api/terminals', undefined, signal),
    retry: false,
  })
}
