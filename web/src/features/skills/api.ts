// REST of the skills slice (server/src/skills/routes.rs).

import { useQuery } from '@tanstack/react-query'
import { api } from '@/api/client'
import type { SkillDetail, SkillFile, SkillsReport, InstallState } from './types'

const enc = encodeURIComponent
const base = (pid: string) => `/api/projects/${enc(pid)}/skills`

export const sk = {
  all: ['skills'] as const,
  report: (pid: string) => ['skills', pid] as const,
  detail: (pid: string, id: string) => ['skills', pid, 'detail', id] as const,
  file: (pid: string, id: string, path: string) => ['skills', pid, 'file', id, path] as const,
}

export const skillsApi = {
  report: (pid: string, signal?: AbortSignal) => api.get<SkillsReport>(base(pid), undefined, signal),
  detail: (pid: string, id: string, signal?: AbortSignal) => api.get<SkillDetail>(`${base(pid)}/${enc(id)}`, undefined, signal),
  file: (pid: string, id: string, path: string, signal?: AbortSignal) => api.get<SkillFile>(`${base(pid)}/${enc(id)}/file`, { path }, signal),
  install: (skill: string, account: string, force = false) => api.post<{ state: InstallState; path: string }>('/api/skills/install', { skill, account, force }),
  remove: (skill: string, account: string, force = false) => api.post<{ ok: boolean }>('/api/skills/remove', { skill, account, force }),
}

/**
 * The skills of a project. Read from disk on every request, so a short staleness lets the window
 * show what an agent or the user changed a moment ago without asking every time it re-renders.
 */
export function useSkillsReport(projectId: string | null) {
  return useQuery({
    queryKey: sk.report(projectId ?? ''),
    queryFn: ({ signal }) => skillsApi.report(projectId ?? '', signal),
    enabled: !!projectId,
    staleTime: 5_000,
    // Skills are edited elsewhere (an editor, an agent's file tools) and nothing watches the folders: coming
    // back to the window is the moment to look again.
    refetchOnWindowFocus: true,
  })
}

export function useSkillDetail(projectId: string, id: string) {
  return useQuery({
    queryKey: sk.detail(projectId, id),
    queryFn: ({ signal }) => skillsApi.detail(projectId, id, signal),
    staleTime: 5_000,
    refetchOnWindowFocus: true,
  })
}

export function useSkillFile(projectId: string, id: string, path: string | null) {
  return useQuery({
    queryKey: sk.file(projectId, id, path ?? ''),
    queryFn: ({ signal }) => skillsApi.file(projectId, id, path ?? '', signal),
    enabled: path !== null,
    staleTime: 5_000,
  })
}
