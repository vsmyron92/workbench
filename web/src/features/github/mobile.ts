// What the phone's GitHub tab shows, and which panels it takes on a phone.
// Kept apart from MobileGithub.tsx so the feature module can route panels
// without loading the tab's code.

import { create } from 'zustand'
import { activeScope, scopeProject, scopeRepo, setActiveRepo } from '@/api/repos'
import { useUi } from '@/state/store'

export type GhMobileView =
  | { kind: 'list' }
  | { kind: 'run'; pid: string; id: number }
  | { kind: 'job'; pid: string; id: number; runId: number | null }
  | { kind: 'pr'; pid: string; number: number }
  | { kind: 'issue'; pid: string; number: number }

export const useGhMobile = create<{ view: GhMobileView; setView: (v: GhMobileView) => void }>()((set) => ({
  view: { kind: 'list' },
  setView: (view) => set({ view }),
}))

/** The view for a GitHub panel (`gh.run`, `gh.job`, `pr`, `gh.issue`), if it is one. */
export function mobileViewFor(kind: string, params: Record<string, unknown>, currentProject: string | null): GhMobileView | null {
  // The panel's `projectId` is a repository scope id; without one, the current project's active repository.
  const pid = typeof params.projectId === 'string' && params.projectId ? params.projectId : currentProject ? activeScope(currentProject) : null
  if (!pid) return null
  const num = (k: string) => (typeof params[k] === 'number' ? (params[k] as number) : null)
  const runId = num('runId')
  const jobId = num('jobId')
  const n = num('number')
  if (kind === 'gh.run' && runId !== null) return { kind: 'run', pid, id: runId }
  if (kind === 'gh.job' && jobId !== null) return { kind: 'job', pid, id: jobId, runId }
  if (kind === 'pr' && n !== null) return { kind: 'pr', pid, number: n }
  if (kind === 'gh.issue' && n !== null) return { kind: 'issue', pid, number: n }
  return null
}

/** `openPanel` on a phone: show the panel in the GitHub tab (switching to its project). */
export function openOnPhone(panel: { kind: string; params: Record<string, unknown> }): boolean {
  const ui = useUi.getState()
  const view = mobileViewFor(panel.kind, panel.params, ui.projectId)
  if (!view || view.kind === 'list') return false
  // Switch to the panel's project and repository, which the tab then shows.
  const project = scopeProject(view.pid)
  if (project !== ui.projectId) ui.setProject(project)
  setActiveRepo(project, scopeRepo(view.pid))
  useGhMobile.getState().setView(view)
  return true
}
