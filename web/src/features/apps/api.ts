// Data access for the apps feature: queries, actions and panel helpers.

import { useQuery, type QueryClient } from '@tanstack/react-query'
import { api, ApiError } from '@/api/client'
import type { TerminalInfo } from '@/api/types'
import { confirmDialog, openPanel, toast, toastError } from '@/shell/actions'
import { appPanelId, runUrl } from './logic'
import type { DeployPlan, EnvView, HealthView, ProxyUrl, RunView, VersionInfo } from './types'

export const appsKeys = {
  runs: (pid: string) => ['apps', 'runs', pid] as const,
  envs: (pid: string) => ['apps', 'envs', pid] as const,
}

const enc = encodeURIComponent
const base = (pid: string) => `/api/projects/${enc(pid)}`

/** Set by the AppsEvents provider so commands (outside React) can read the cache. */
let client: QueryClient | null = null
export function setAppsQueryClient(qc: QueryClient | null) {
  client = qc
}
export function cachedRuns(pid: string | null): RunView[] {
  return (pid && client?.getQueryData<RunView[]>(appsKeys.runs(pid))) || []
}
export function cachedEnvs(pid: string | null): EnvView[] {
  return (pid && client?.getQueryData<EnvView[]>(appsKeys.envs(pid))) || []
}
function refresh(pid: string) {
  void client?.invalidateQueries({ queryKey: appsKeys.runs(pid) })
}

export function useRuns(pid: string | null) {
  return useQuery({
    queryKey: appsKeys.runs(pid ?? ''),
    queryFn: ({ signal }) => api.get<RunView[]>(`${base(pid!)}/runs`, undefined, signal),
    enabled: !!pid,
    staleTime: 15_000,
  })
}

export function useEnvs(pid: string | null) {
  return useQuery({
    queryKey: appsKeys.envs(pid ?? ''),
    queryFn: ({ signal }) => api.get<EnvView[]>(`${base(pid!)}/envs`, undefined, signal),
    enabled: !!pid,
    staleTime: 60_000,
  })
}

// ---------------------------------------------------------------- runs

function confirmFreePort(name: string, why: string) {
  return confirmDialog({
    title: 'Port in use',
    message: `${why}.\n\nWorkbench will stop it (its own run, or fuser -k <port>/tcp for another process) and then start “${name}”.`,
    confirmLabel: 'Free port and start',
    danger: true,
  })
}

function confirmRisky(name: string, why: string, command?: string) {
  return confirmDialog({
    title: `Run “${name}”?`,
    message: command ? `${why}.\n\n${command}` : why,
    confirmLabel: 'Run',
    danger: true,
  })
}

/**
 * Start (or restart) a run. Runs that deploy, release or reach a remote host
 * (`needsConfirm`) are confirmed first. The server checks the port live — the cached
 * `portInUse` may be stale — and answers `409 port_in_use` (unless the config has
 * `free_port = true`) or `428 confirmation_required` (e.g. for a dependency); each
 * is asked about once, then the start is retried. Returns false if cancelled or failed.
 */
export async function startRun(pid: string, name: string, restart = false): Promise<boolean> {
  const path = `${base(pid)}/runs/${enc(name)}/${restart ? 'restart' : 'start'}`
  const known = cachedRuns(pid).find((r) => r.name === name)
  let confirmed = false
  let freePort = false
  if (known?.needsConfirm) {
    if (!(await confirmRisky(name, 'It may deploy, release or reach a remote host', known.config.command))) return false
    confirmed = true
  }
  for (;;) {
    try {
      await api.post<RunView>(path, { freePort, confirmed })
      refresh(pid)
      return true
    } catch (e) {
      if (e instanceof ApiError && e.code === 'port_in_use' && !freePort) {
        const ok = await confirmFreePort(name, e.message)
        refresh(pid)
        if (!ok) return false
        freePort = true
        continue
      }
      if (e instanceof ApiError && e.code === 'confirmation_required' && !confirmed) {
        if (!(await confirmRisky(name, e.message))) return false
        confirmed = true
        continue
      }
      toastError(e, `Could not ${restart ? 'restart' : 'start'} ${name}`)
      return false
    }
  }
}

export async function stopRun(pid: string, name: string) {
  try {
    await api.post<RunView>(`${base(pid)}/runs/${enc(name)}/stop`)
    refresh(pid)
  } catch (e) {
    toastError(e, `Could not stop ${name}`)
  }
}

export async function stopAllRuns(pid: string) {
  try {
    const r = await api.post<{ stopped: number }>(`${base(pid)}/runs/stop-all`)
    toast('info', r.stopped ? `Stopped ${r.stopped} run${r.stopped === 1 ? '' : 's'}` : 'Nothing was running')
    refresh(pid)
  } catch (e) {
    toastError(e, 'Stop all')
  }
}

export function openTerminal(terminalId: string, title?: string) {
  openPanel({ kind: 'terminal', id: `terminal:${terminalId}`, params: { terminalId }, title })
}

export function openRunOutput(r: RunView) {
  if (r.terminalId) openTerminal(r.terminalId, r.name)
  else toast('info', `${r.name} has no output yet`)
}

/** Pin a run to the host (or back into the project's dev container): devcontainer REST. */
export async function setRunOnHost(pid: string, name: string, host: boolean) {
  try {
    await api.put(`/api/projects/${enc(pid)}/devcontainer/settings`, { hostRun: { name, host } })
    toast('success', host ? `${name} runs on the host` : `${name} runs in the dev container`)
  } catch (e) {
    toastError(e)
  }
  refresh(pid)
}

export function openRunPreview(pid: string, r: RunView) {
  const url = runUrl(r)
  if (!url) return toast('info', `${r.name} has no URL to preview`)
  openPanel({ kind: 'app', id: appPanelId(pid, { run: r.name }), title: `▶ ${r.name}`, params: { projectId: pid, run: r.name, url } })
}

// ---------------------------------------------------------------- environments

export function openEnvPreview(pid: string, e: Pick<EnvView, 'name' | 'url'>) {
  openPanel({ kind: 'app', id: appPanelId(pid, { env: e.name }), title: e.name, params: { projectId: pid, env: e.name, url: e.url } })
}

export function openExternal(url: string) {
  window.open(url, '_blank', 'noopener,noreferrer')
}

export async function checkEnv(pid: string, name: string): Promise<HealthView | null> {
  try {
    return await api.post<HealthView>(`${base(pid)}/envs/${enc(name)}/check`)
  } catch (e) {
    toastError(e, `Check ${name}`)
    return null
  }
}

export async function checkAllEnvs(pid: string) {
  try {
    const envs = await api.post<EnvView[]>(`${base(pid)}/envs/check`)
    client?.setQueryData(appsKeys.envs(pid), envs)
    const down = envs.filter((e) => e.health.status === 'down' || e.health.status === 'degraded')
    if (!envs.length) toast('info', 'This project has no environments')
    else if (down.length) toast('warning', `${down.map((e) => e.name).join(', ')}: ${down[0].health.error ?? down[0].health.status}`)
    else toast('success', `All ${envs.length} environment${envs.length === 1 ? '' : 's'} checked`)
  } catch (e) {
    toastError(e, 'Check environments')
  }
}

export async function checkVersion(pid: string, name: string): Promise<VersionInfo | null> {
  try {
    const v = await api.post<VersionInfo>(`${base(pid)}/envs/${enc(name)}/version`)
    void client?.invalidateQueries({ queryKey: appsKeys.envs(pid) })
    if (v.error) toast('warning', `${name} version: ${v.error}`)
    return v
  } catch (e) {
    toastError(e, `${name} version`)
    return null
  }
}

export async function openEnvLogs(pid: string, env: string, log?: string) {
  try {
    const t = await api.post<TerminalInfo>(`${base(pid)}/envs/${enc(env)}/logs`, { name: log ?? null })
    openTerminal(t.id, t.title)
  } catch (e) {
    toastError(e, `${env} logs`)
  }
}

export async function runEnvCommand(pid: string, env: string, cmd: { name: string; command: string; confirm: boolean }) {
  if (cmd.confirm) {
    const ok = await confirmDialog({
      title: `${cmd.name} on ${env}?`,
      message: cmd.command,
      confirmLabel: 'Run',
      danger: true,
    })
    if (!ok) return
  }
  try {
    const t = await api.post<TerminalInfo>(`${base(pid)}/envs/${enc(env)}/command`, { name: cmd.name, confirmed: cmd.confirm })
    openTerminal(t.id, t.title)
  } catch (e) {
    toastError(e, cmd.name)
  }
}

export function deployCheck(pid: string, env: string, sha?: string) {
  return api.post<DeployPlan>(`${base(pid)}/envs/${enc(env)}/deploy/check`, { sha: sha || null })
}

export async function deploy(pid: string, env: string, sha: string, confirmation: string | boolean): Promise<TerminalInfo | null> {
  try {
    const t = await api.post<TerminalInfo>(`${base(pid)}/envs/${enc(env)}/deploy`, { sha, confirmation })
    void client?.invalidateQueries({ queryKey: appsKeys.envs(pid) })
    openTerminal(t.id, t.title)
    return t
  } catch (e) {
    toastError(e, `Deploy to ${env}`)
    return null
  }
}

export function proxyUrl(pid: string, env: string, path: string) {
  return api.get<ProxyUrl>(`${base(pid)}/envs/${enc(env)}/proxy-url`, { path })
}

export async function redetect() {
  try {
    await api.post('/api/projects/reload')
    toast('success', 'Projects re-detected')
  } catch (e) {
    toastError(e, 'Re-detect')
  }
}
