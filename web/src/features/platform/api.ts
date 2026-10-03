// Queries and mutations for the platform slice's REST routes.

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import { useEvent } from '@/api/events'
import { toast } from '@/shell/actions'
import { prependCapped, restartText } from './lib'
import type {
  ActivityEvent,
  ActivityFeed,
  ApplyResult,
  GlobalConfig,
  LocalModels,
  McpCall,
  McpOverview,
  ProjectSettings,
  PushInfo,
  RawConfig,
  RemoteInfo,
  SecretsInfo,
  SettingsInfo,
  UpdateStatus,
  UsageInfo,
} from './types'

/** Every platform query lives under ['platform', …] so `settings.changed` can refresh them all. */
export const pk = {
  all: ['platform'] as const,
  settings: ['platform', 'settings'] as const,
  raw: ['platform', 'raw'] as const,
  secrets: ['platform', 'secrets'] as const,
  project: (id: string) => ['platform', 'project', id] as const,
  remote: ['platform', 'remote'] as const,
  mcp: (projectId: string | null) => ['platform', 'mcp', projectId ?? ''] as const,
  activity: ['platform-activity'] as const,
  push: ['platform', 'push'] as const,
  update: ['platform', 'update'] as const,
  usage: ['platform', 'usage'] as const,
}

/** Which agent accounts are at their usage limit, and how full their windows are; `agent.usage` keeps it live. */
export function useAccountUsage() {
  const qc = useQueryClient()
  useEvent('agent.usage', () => void qc.invalidateQueries({ queryKey: pk.usage }))
  return useQuery({ queryKey: pk.usage, queryFn: () => api.get<UsageInfo>('/api/agents/usage'), staleTime: 30_000, refetchInterval: 60_000 })
}

/** Say an account is at its limit until `until` (ms), or usable again (`null`). */
export function setAccountLimit(provider: string, until: number | null) {
  return api.put<unknown>(`/api/agents/usage/${encodeURIComponent(provider)}`, { limitedUntil: until })
}

/** What a model server of your own serves (or why it does not answer). */
export function probeLocalModels(server: string, url: string) {
  return api.post<LocalModels>('/api/agents/local-models', { server, url })
}

/** The running version, the latest release and an install's progress; `platform.update` keeps it live. */
export function useUpdate() {
  const qc = useQueryClient()
  useEvent<UpdateStatus>('platform.update', (ev) => qc.setQueryData(pk.update, ev.data))
  return useQuery({ queryKey: pk.update, queryFn: () => api.get<UpdateStatus>('/api/platform/update'), staleTime: 60_000 })
}

/** The VAPID key and the devices with push; `push.changed` keeps it fresh on every device. */
export function usePushInfo() {
  const qc = useQueryClient()
  useEvent('push.changed', () => void qc.invalidateQueries({ queryKey: pk.push }))
  return useQuery({ queryKey: pk.push, queryFn: () => api.get<PushInfo>('/api/push'), staleTime: 60_000 })
}

export function useSettings() {
  return useQuery({ queryKey: pk.settings, queryFn: () => api.get<SettingsInfo>('/api/settings') })
}

export function useRawConfig() {
  return useQuery({ queryKey: pk.raw, queryFn: () => api.get<RawConfig>('/api/settings/raw'), staleTime: Infinity })
}

export function useSecrets() {
  return useQuery({ queryKey: pk.secrets, queryFn: () => api.get<SecretsInfo>('/api/settings/secrets') })
}

export function useProjectSettings(projectId: string | null) {
  return useQuery({
    queryKey: pk.project(projectId ?? ''),
    queryFn: () => api.get<ProjectSettings>(`/api/settings/projects/${encodeURIComponent(projectId ?? '')}`),
    enabled: !!projectId,
    staleTime: Infinity,
  })
}

export function useRemote(opts: { refetchInterval?: number } = {}) {
  return useQuery({
    queryKey: pk.remote,
    queryFn: () => api.get<RemoteInfo>('/api/platform/remote'),
    staleTime: 60_000,
    refetchInterval: opts.refetchInterval,
  })
}

export function useMcpOverview(projectId: string | null) {
  return useQuery({
    queryKey: pk.mcp(projectId),
    queryFn: () => api.get<McpOverview>('/api/platform/mcp-servers', { projectId }),
  })
}

/**
 * Recent MCP tool calls and notable events, kept live from `mcp.call` and
 * `platform.activity` events (the server keeps the last 500 of each).
 */
export function useActivityFeed() {
  const qc = useQueryClient()
  const q = useQuery({
    queryKey: pk.activity,
    queryFn: () => api.get<ActivityFeed>('/api/platform/activity'),
    staleTime: Infinity,
  })
  useEvent<McpCall>('mcp.call', (ev) =>
    qc.setQueryData<ActivityFeed>(pk.activity, (old) => (old ? { ...old, calls: prependCapped(old.calls, ev.data) } : old)),
  )
  useEvent<ActivityEvent>('platform.activity', (ev) =>
    qc.setQueryData<ActivityFeed>(pk.activity, (old) => (old ? { ...old, events: prependCapped(old.events, ev.data) } : old)),
  )
  useEvent('resync', () => qc.invalidateQueries({ queryKey: pk.activity }))
  return q
}

/** Toast what a save did (restart needed, warnings). */
export function reportApply(r: ApplyResult, what = 'Settings saved') {
  if (r.restartRequired.length) {
    toast('warning', `${what}. Restart Workbench to apply the new ${restartText(r.restartRequired)}.`, { timeout: 9000 })
  } else {
    toast('success', what)
  }
  if (r.warnings.length) {
    toast('warning', r.warnings.length === 1 ? r.warnings[0] : `${r.warnings.length} warnings`, {
      detail: r.warnings.length > 1 ? r.warnings.slice(0, 4).join('\n') : undefined,
      timeout: 9000,
    })
  }
}

type Patch = Partial<{ [K in keyof GlobalConfig]: GlobalConfig[K] | Partial<NonNullable<GlobalConfig[K]>> | null }>

/** Structured config change (keeps comments in config.toml). */
export async function patchSettings(patch: Patch, baseHash?: string): Promise<ApplyResult> {
  return api.patch<ApplyResult>('/api/settings', { patch, baseHash })
}
