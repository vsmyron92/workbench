// REST client for the HTTP Client (server/src/apps/http_client.rs).

import { api } from '@/api/client'

export interface HttpEnvs {
  envs: string[]
  requests: { line: number; endLine: number; method: string; name: string | null }[]
}

export interface HttpResult {
  path: string
  line: number
  name: string | null
  env: string | null
  /** As sent, with private env values masked. */
  request: { method: string; url: string; headers: [string, string][]; body: string | null }
  status: number
  statusText: string
  headers: [string, string][]
  body: string
  binary: boolean
  truncated: boolean
  size: number
  elapsedMs: number
  finalUrl: string
  contentType: string | null
  at: number
}

const p = (pid: string) => `/api/projects/${encodeURIComponent(pid)}/http`

export const httpApi = {
  envs: (pid: string, path: string, signal?: AbortSignal) => api.get<HttpEnvs>(`${p(pid)}/envs`, { path }, signal),
  run: (pid: string, path: string, line: number, env: string | null) => api.post<HttpResult>(`${p(pid)}/run`, { path, line, env }),
}
