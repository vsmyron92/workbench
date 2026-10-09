// Shared hooks of the files slice.

import { useEffect, useMemo, useRef } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useEvent } from '@/api/events'
import { useProject, useProjects } from '@/api/queries'
import { useRepos } from '@/api/useRepos'
import { filesApi } from './api'
import { isScratch, SCRATCH_ID } from './scratchStore'
import { buildVcsIndex, EMPTY_VCS, type VcsIndex } from './vcs'

/**
 * The git slice's own cache entry for `GET …/git/status` (a cross-slice contract,
 * docs/ARCHITECTURE.md). Sharing it means one request per change for the tree
 * colours, the Commit window and the status bar, and no refresh logic here: the git
 * slice's provider invalidates it on `git.changed`, `fs.changed` (debounced) and
 * `resync`. A project with several repositories asks for all of them at once
 * (`?repo=all`, project-relative paths) under a key of its own; a project with one
 * shares the plain status.
 */
export const gitStatusKey = (pid: string, all = false) => (all ? (['git', pid, 'status', 'all'] as const) : (['git', pid, 'status'] as const))

/**
 * The git status for `projectId`. Any error (the git feature missing, not a
 * repository) just means "no colours".
 */
export function useGitStatus(projectId: string | null, enabled = true) {
  const all = useRepos(projectId).length > 1
  return useQuery({
    queryKey: gitStatusKey(projectId ?? '', all),
    queryFn: ({ signal }) => filesApi.gitStatus(projectId!, signal, all),
    // Scratch files are no repository.
    enabled: !!projectId && !isScratch(projectId) && enabled,
    retry: false,
    staleTime: 15_000,
  })
}

export function useVcsIndex(projectId: string | null): VcsIndex {
  const { data } = useGitStatus(projectId)
  return useMemo(() => (data ? buildVcsIndex(data) : EMPTY_VCS), [data])
}

export function useProjectSummary(projectId: string | null) {
  const { data } = useProjects()
  // The scratch files' project is never listed: its summary comes from its detail.
  const scratch = useProject(isScratch(projectId) ? SCRATCH_ID : null).data?.summary ?? null
  return isScratch(projectId) ? scratch : (data?.find((p) => p.id === projectId) ?? null)
}

/** Call `fn` (debounced) when `fs.changed` touches `path` of `projectId`. */
export function useFileChanged(projectId: string | null, path: string, fn: () => void, delay = 150) {
  const ref = useRef(fn)
  ref.current = fn
  const timer = useRef<number | undefined>(undefined)
  useEvent<{ paths?: string[]; overflow?: boolean }>('fs.changed', (ev) => {
    if (!projectId || ev.projectId !== projectId) return
    const hit = ev.data.overflow || (ev.data.paths ?? []).some((p) => p === path || path.startsWith(p + '/'))
    if (!hit) return
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => ref.current(), delay)
  })
  useEvent('resync', () => ref.current())
  useEffect(() => () => window.clearTimeout(timer.current), [])
}
