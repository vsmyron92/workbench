// REST of the devcontainer slice (server/src/devcontainer/routes.rs).

import { useQuery, type QueryClient } from '@tanstack/react-query'
import { api, ApiError } from '@/api/client'
import { qk } from '@/api/queries'
import type { TerminalInfo } from '@/api/types'
import { openPanel, toast, toastError } from '@/shell/actions'
import type { DcView, Proposal } from './types'

const enc = encodeURIComponent
const base = (pid: string) => `/api/projects/${enc(pid)}/devcontainer`

export const dcKeys = {
  view: (pid: string) => ['devcontainer', pid] as const,
}

let client: QueryClient | null = null
/** Set by the provider so actions outside React refresh the caches. */
export function setDcQueryClient(qc: QueryClient | null) {
  client = qc
}

export function refreshDc(pid: string) {
  void client?.invalidateQueries({ queryKey: dcKeys.view(pid) })
  void client?.invalidateQueries({ queryKey: qk.projects })
  void client?.invalidateQueries({ queryKey: ['apps', 'runs', pid] })
}

export function useDevcontainer(pid: string | null, enabled = true) {
  return useQuery({
    queryKey: dcKeys.view(pid ?? ''),
    queryFn: ({ signal }) => api.get<DcView>(base(pid!), undefined, signal),
    enabled: !!pid && enabled,
    staleTime: 10_000,
  })
}

export function fetchView(pid: string, config?: string | null): Promise<DcView> {
  return api.get<DcView>(base(pid), config ? { config } : undefined)
}

/** The panel's stable id. */
export function panelId(pid: string) {
  return `devcontainer:${pid}`
}

export function openDevcontainerPanel(pid: string) {
  openPanel({ kind: 'devcontainer', id: panelId(pid), title: 'Dev container', params: { projectId: pid } })
}

export function openTerminalPanel(t: Pick<TerminalInfo, 'id' | 'title'>) {
  openPanel({ kind: 'terminal', id: `terminal:${t.id}`, title: t.title, params: { terminalId: t.id } })
}

/**
 * POST start|rebuild with the hash of the plan the user approved. `stale`: the plan
 * changed between the dialog and the click (409 approval_required).
 */
export async function startWithApproval(
  pid: string,
  config: string | null,
  hash: string,
  rebuild: boolean,
): Promise<{ ok: true; terminalId: string } | { ok: false; stale: boolean }> {
  try {
    const r = await api.post<{ terminalId: string }>(`${base(pid)}/${rebuild ? 'rebuild' : 'start'}`, { config, approve: hash })
    refreshDc(pid)
    openTerminalPanel({ id: r.terminalId, title: 'Dev container' })
    return { ok: true, terminalId: r.terminalId }
  } catch (e) {
    if (e instanceof ApiError && e.code === 'approval_required') return { ok: false, stale: true }
    toastError(e, rebuild ? 'Could not rebuild the dev container' : 'Could not start the dev container')
    return { ok: false, stale: false }
  }
}

export async function stopContainer(pid: string) {
  try {
    await api.post(`${base(pid)}/stop`)
    toast('success', 'Dev container stopped')
  } catch (e) {
    toastError(e, 'Could not stop the dev container')
  }
  refreshDc(pid)
}

export async function removeContainer(pid: string) {
  try {
    await api.post(`${base(pid)}/remove`, { confirm: true })
    toast('success', 'Dev container removed')
  } catch (e) {
    toastError(e, 'Could not remove the dev container')
  }
  refreshDc(pid)
}

export async function saveSettings(pid: string, body: { useContainer?: boolean; config?: string; hostRun?: { name: string; host: boolean } }) {
  try {
    await api.put(`${base(pid)}/settings`, body)
  } catch (e) {
    toastError(e)
  }
  refreshDc(pid)
}

export async function openContainerShell(pid: string): Promise<TerminalInfo | null> {
  try {
    const t = await api.post<TerminalInfo>('/api/terminals', { kind: 'shell', projectId: pid, container: true })
    openTerminalPanel(t)
    return t
  } catch (e) {
    toastError(e, 'Could not open a shell in the dev container')
    return null
  }
}

export function fetchProposal(pid: string) {
  return api.get<Proposal>(`${base(pid)}/scaffold`)
}

export async function writeProposal(pid: string, path: string, content: string): Promise<boolean> {
  try {
    await api.post(`${base(pid)}/scaffold`, { path, content })
    toast('success', `${path} created`)
    refreshDc(pid)
    return true
  } catch (e) {
    toastError(e, 'Could not create the config')
    return false
  }
}
