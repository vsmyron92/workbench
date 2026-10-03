// REST calls and queries of the terminals slice (server/src/terminals/routes.rs).

import { useQuery } from '@tanstack/react-query'
import { api } from '@/api/client'
import { qk } from '@/api/queries'
import type { AgentProvider, TerminalInfo } from '@/api/types'
import { openPanel, toastError } from '@/shell/actions'
import { terminalPanelId } from './lib/sessions'
import { updateCachedTerminal } from './queryAccess'

/** GET /api/agents/history?projectId=&provider= */
export interface HistoryEntry {
  id: string
  provider: string
  title: string
  firstPrompt: string | null
  lastMessage: string | null
  lastActivity: number
  sizeBytes: number
  gitBranch: string | null
  terminalId: string | null
  open: boolean
}

/** A permission choice of a provider (terminals/providers.rs PermissionPreset). */
export interface PermissionPreset {
  id: string
  label: string
  description: string
  /** Skips approvals (Codex: and the sandbox). Never a default; confirmed on start. */
  dangerous: boolean
}

/** GET /api/agents/defaults → providers[] */
export interface ProviderInfo {
  id: string
  kind: AgentProvider
  label: string
  command: string
  enabled: boolean
  available: boolean
  /** Why it cannot start (missing command, disabled). */
  reason: string | null
  installHint: string | null
  /** The variable that moves the CLI's files (login included) and its value for this provider; `home` is null for the CLI's default account. */
  homeVar?: string | null
  home?: string | null
  /** hooks (Claude), rollout (Codex), activity (Kimi, custom: an estimate from output). */
  stateSource: 'hooks' | 'rollout' | 'activity'
  initialPrompt: 'argv' | 'paste'
  supports: {
    resume: boolean
    fork: boolean
    model: boolean
    mcp: boolean
    remoteControl: boolean
    addDirs: boolean
    history: boolean
    /** Its permission requests can be answered from Workbench (Claude Code). */
    answerPermissions?: boolean
  }
  efforts: string[]
  permissionModes: PermissionPreset[]
  defaults: { model: string | null; effort: string | null; permissionMode: string | null }
}

/** GET /api/agents/external */
export interface ExternalSession {
  pid: number
  sessionId: string
  name: string | null
  cwd: string
  status: string | null
  startedAt: number | null
  updatedAt: number | null
  remoteUrl: string | null
  projectId: string | null
  entrypoint: string | null
}

/** GET /api/agents/defaults */
export interface AgentDefaults {
  command: string
  commandFound: boolean
  model: string | null
  effort: string | null
  permissionMode: string | null
  remoteControl: boolean
  restoreOnStart: boolean
  statusline: boolean
  /** Claude Code permission requests are answerable from Workbench (`[agents] answer_permissions`). */
  answerPermissions: boolean
  /** How long (s) a request stays answerable here; null when answering is off. */
  permissionWait: number | null
  addDirs: string[]
  starters: { name: string; prompt: string }[]
  efforts: string[]
  permissionModes: string[]
  providers: ProviderInfo[]
  defaultProvider: string
  /** `[agents.providers]` entries that were left out, and why. */
  providerWarnings: string[]
}

export interface NewAgentRequest {
  projectId: string
  /** `[agents.providers.<id>]`; omitted: the default provider. */
  provider?: string
  prompt?: string
  name?: string
  model?: string
  effort?: string
  permissionMode?: string
  remoteControl?: boolean
  resume?: string
  fork?: boolean
  cwd?: string
  addDirs?: string[]
  cols?: number
  rows?: number
  /** Run the session in the project's running dev container (its CLI must be installed there). */
  inContainer?: boolean
}

/**
 * The agent CLIs installed in the project's running dev container (devcontainer REST:
 * `agents` of GET /api/projects/{pid}/devcontainer): name → path there, or null.
 */
export function useContainerAgents(projectId: string | null, running: boolean) {
  return useQuery({
    queryKey: ['agents', 'container', projectId ?? ''],
    queryFn: () => api.get<{ agents: Record<string, string | null>; inContainer: boolean }>(`/api/projects/${encodeURIComponent(projectId!)}/devcontainer`),
    enabled: !!projectId && running,
    staleTime: 30_000,
  })
}

export const agentKeys = {
  history: (projectId: string, provider = 'claude') => ['agents', 'history', projectId, provider] as const,
  external: ['agents', 'external'] as const,
  defaults: (projectId: string | null) => ['agents', 'defaults', projectId ?? ''] as const,
}

export function useAgentHistory(projectId: string | null, provider = 'claude', enabled = true) {
  return useQuery({
    queryKey: agentKeys.history(projectId ?? '', provider),
    queryFn: () => api.get<HistoryEntry[]>('/api/agents/history', { projectId, provider, limit: 150 }),
    enabled: !!projectId && enabled,
    staleTime: 15_000,
  })
}

export function useExternalSessions(enabled = true) {
  return useQuery({
    queryKey: agentKeys.external,
    queryFn: () => api.get<ExternalSession[]>('/api/agents/external'),
    enabled,
    refetchInterval: enabled ? 15_000 : false,
    staleTime: 10_000,
  })
}

export function useAgentDefaults(projectId: string | null) {
  return useQuery({
    queryKey: agentKeys.defaults(projectId),
    queryFn: () => api.get<AgentDefaults>('/api/agents/defaults', { projectId }),
    staleTime: 60_000,
  })
}

/** Show a terminal as a tab of the agents column (on a phone: full screen in the Agents tab). */
export function openTerminal(t: Pick<TerminalInfo, 'id' | 'title'>, focus = true) {
  openPanel({ kind: 'terminal', id: terminalPanelId(t.id), title: t.title, params: { terminalId: t.id }, focus })
}

export const terminalsApi = {
  createAgent: (req: NewAgentRequest) => api.post<TerminalInfo>('/api/agents', req),
  /** `container`: true in the project's dev container, false on the host, omitted: the project's default. */
  createShell: (projectId: string | null, cwd?: string, container?: boolean) =>
    api.post<TerminalInfo>('/api/terminals', { kind: 'shell', projectId, cwd, container }),
  patch: (id: string, body: { title?: string; color?: string | null; pinned?: boolean; order?: number; open?: boolean }) =>
    api.patch<TerminalInfo>(`/api/terminals/${id}`, body),
  close: (id: string, forget = false) => api.del<{ ok: boolean }>(`/api/terminals/${id}`, { forget: forget || undefined }),
  restart: (id: string) => api.post<TerminalInfo>(`/api/terminals/${id}/restart`),
  kill: (id: string) => api.post<TerminalInfo>(`/api/terminals/${id}/kill`),
  seen: (id: string) => api.post<TerminalInfo>(`/api/terminals/${id}/seen`),
  input: (id: string, text: string, submit = true, paste = true) => api.post(`/api/terminals/${id}/input`, { text, submit, paste }),
  keys: (id: string, keys: string[]) => api.post(`/api/terminals/${id}/keys`, { keys }),
  text: (id: string, lines = 200) => api.get<{ text: string }>(`/api/terminals/${id}/text`, { lines }),
  attach: (id: string, file: Blob) => api.post<{ path: string; quoted: string }>(`/api/terminals/${id}/attachment`, file),
  remoteControl: (body: { projectId: string; spawn: string; name?: string; permissionMode?: string }) =>
    api.post<TerminalInfo>('/api/agents/remote-control', body),
  /** Answer a pending permission request (devices only; 409 once it is settled elsewhere). */
  answerPermission: (id: string, body: Record<string, unknown>) => api.post<TerminalInfo>(`/api/agents/${id}/permission`, body),
}

export function openAgentsHome() {
  openPanel({ kind: 'agents.home', id: 'agents.home', title: 'Agents' })
}

/** Start a session and open it. Returns null (after a toast) on failure. */
export async function startAgent(req: NewAgentRequest): Promise<TerminalInfo | null> {
  try {
    const t = await terminalsApi.createAgent(req)
    updateCachedTerminal(t)
    openTerminal(t)
    return t
  } catch (e) {
    toastError(e, 'Could not start the session')
    return null
  }
}

export { qk }
