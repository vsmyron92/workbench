// REST of the Services tool window: /api/docker (server/src/devcontainer/services.rs).

import { useQuery } from '@tanstack/react-query'
import { api } from '@/api/client'
import type { TerminalInfo } from '@/api/types'
import { openPanel, toast, toastError } from '@/shell/actions'

const enc = encodeURIComponent

export interface PortMap {
  port: number
  proto: string
  hostIp?: string
  hostPort?: number
}

export interface ComposeInfo {
  project: string
  service: string
  workingDir: string
  configFiles: string[]
  number?: number
}

export interface DockerContainer {
  id: string
  name: string
  image: string
  /** running, exited, created, paused, restarting, removing, dead */
  state: string
  /** Docker's words: "Up 2 hours (healthy)", "Exited (0) 3 days ago". */
  status: string
  createdAt: string
  ports: PortMap[]
  compose?: ComposeInfo
  /** The folder a dev container was made for. */
  devcontainer?: string
  projectId?: string
}

export interface ContainerDetail {
  id: string
  name: string
  image: string
  imageId: string
  state: string
  running: boolean
  paused: boolean
  createdAt: string
  startedAt: string | null
  finishedAt: string | null
  exitCode: number | null
  error: string | null
  health: string | null
  restartPolicy: string | null
  restartCount: number
  command: string
  workingDir: string
  user: string
  hostname: string
  networkMode: string
  ports: PortMap[]
  networks: { name: string; ip: string; gateway: string; aliases: string[] }[]
  mounts: { type: string; source: string; destination: string; rw: boolean }[]
  /** Values of secret-looking names and URL passwords are masked (••••). */
  env: [string, string][]
  labels: [string, string][]
  compose: ComposeInfo | null
  devcontainer: string | null
  projectId: string | null
  inspect: unknown
}

export interface DockerImage {
  id: string
  /** `<none>` for a dangling image. */
  repository: string
  tag: string
  createdAt: string
  size: string
  /** Names of the containers made from it. */
  containers: string[]
}

export interface ImageDetail {
  id: string
  tags: string[]
  digests: string[]
  createdAt: string
  size: number
  architecture: string
  os: string
  entrypoint: string
  cmd: string
  workingDir: string
  user: string
  exposedPorts: string[]
  layers: number
  env: [string, string][]
  labels: [string, string][]
  inspect: unknown
}

export const dockerKeys = {
  all: ['docker'] as const,
  containers: ['docker', 'containers'] as const,
  container: (id: string) => ['docker', 'container', id] as const,
  images: ['docker', 'images'] as const,
  image: (id: string) => ['docker', 'image', id] as const,
}

export function useContainers(enabled = true) {
  return useQuery({
    queryKey: dockerKeys.containers,
    queryFn: ({ signal }) => api.get<{ containers: DockerContainer[]; error?: string }>('/api/docker/containers', undefined, signal),
    enabled,
    // "Up 5 minutes" ages; changes themselves arrive as docker.changed.
    refetchInterval: 60_000,
  })
}

export function useContainer(id: string | null) {
  return useQuery({
    queryKey: dockerKeys.container(id ?? ''),
    queryFn: ({ signal }) => api.get<ContainerDetail>(`/api/docker/containers/${enc(id!)}`, undefined, signal),
    enabled: !!id,
    retry: false,
  })
}

export function useImages(enabled = true) {
  return useQuery({
    queryKey: dockerKeys.images,
    queryFn: ({ signal }) => api.get<{ images: DockerImage[]; error?: string }>('/api/docker/images', undefined, signal),
    enabled,
  })
}

export function useImage(id: string | null) {
  return useQuery({
    queryKey: dockerKeys.image(id ?? ''),
    queryFn: ({ signal }) => api.get<ImageDetail>(`/api/docker/images/${enc(id!)}`, undefined, signal),
    enabled: !!id,
    retry: false,
  })
}

export type ContainerAction = 'start' | 'stop' | 'restart' | 'pause' | 'unpause' | 'kill' | 'remove'
export type ComposeAction = 'start' | 'stop' | 'restart' | 'down'

const PAST: Record<ContainerAction | ComposeAction, string> = {
  start: 'started',
  stop: 'stopped',
  restart: 'restarted',
  pause: 'paused',
  unpause: 'unpaused',
  kill: 'killed',
  remove: 'removed',
  down: 'taken down',
}

/** Run an action; `false` when it failed (already reported). */
export async function containerAction(id: string, name: string, action: ContainerAction, force = false): Promise<boolean> {
  try {
    await api.post(`/api/docker/containers/${enc(id)}/${action}`, action === 'remove' ? { force } : {})
    toast('success', `${name} ${PAST[action]}`)
    return true
  } catch (e) {
    toastError(e, `Could not ${action} ${name}`)
    return false
  }
}

export async function composeAction(project: string, action: ComposeAction): Promise<boolean> {
  try {
    await api.post(`/api/docker/compose/${enc(project)}/${action}`)
    toast('success', `${project} ${PAST[action]}`)
    return true
  } catch (e) {
    toastError(e, `Could not ${action} ${project}`)
    return false
  }
}

async function openTerminal(id: string, what: 'logs' | 'shell', projectId: string | null) {
  try {
    const t = await api.post<TerminalInfo>(`/api/docker/containers/${enc(id)}/${what}`, { projectId })
    openPanel({ kind: 'terminal', id: `terminal:${t.id}`, title: t.title, params: { terminalId: t.id } })
  } catch (e) {
    toastError(e, what === 'logs' ? 'Could not follow the log' : 'Could not open a shell')
  }
}

/** A terminal following the container's log (`docker logs -f`). */
export const showLogs = (id: string, projectId: string | null) => openTerminal(id, 'logs', projectId)
/** A shell inside the container (bash when it has one). */
export const openShell = (id: string, projectId: string | null) => openTerminal(id, 'shell', projectId)

export async function removeImage(ref: string): Promise<boolean> {
  try {
    await api.post('/api/docker/images/remove', { ref })
    toast('success', `Removed ${ref}`)
    return true
  } catch (e) {
    toastError(e, `Could not remove ${ref}`)
    return false
  }
}

export async function pruneImages() {
  try {
    const r = await api.post<{ reclaimed: string | null }>('/api/docker/images/prune')
    toast('success', r.reclaimed ? `Dangling images removed: ${r.reclaimed} reclaimed` : 'Dangling images removed')
  } catch (e) {
    toastError(e, 'Could not prune images')
  }
}
