// REST client and types for Local History (server/src/files/history).

import { api } from '@/api/client'

export type HistoryKind = 'save' | 'disk' | 'agent' | 'base' | 'deleted' | 'label' | 'auto'

/** files/history EntryOut */
export interface HistoryEntry {
  id: number
  /** Unix ms. */
  ts: number
  /** Project-relative; for labels the folder or file it was put on (`''` = project). */
  path: string
  kind: HistoryKind
  label?: string
  size: number
  /** sha256 of the content (the files etag of that version). */
  hash?: string
  /** Agent edits: the session's terminal id and title. */
  by?: string
  who?: string
}

export type Untracked = 'sensitive' | 'git' | 'ignored' | 'binary' | 'tooLarge'

export interface HistoryPage {
  path: string
  entries: HistoryEntry[]
  truncated: boolean
  untracked?: Untracked
}

export interface HistoryRevision {
  entry: HistoryEntry
  content: string | null
  lossy: boolean
  previous: HistoryEntry | null
}

export interface HistoryStats {
  entries: number
  files: number
  bytes: number
  maxBytes: number
  retentionDays: number
  maxVersions: number
  maxFileBytes: number
}

/** GET …/files/history/session?by=: what an agent session changed (Review Changes). */
export interface SessionFile {
  path: string
  /** The version before the session's first edit (null: none recorded, a file it created). */
  before: HistoryEntry | null
  first: HistoryEntry
  last: HistoryEntry
  edits: number
  latest: HistoryEntry
  /** Someone else changed it after the session's last edit. */
  changedSince: boolean
  deleted: boolean
}
export interface SessionChanges {
  by: string
  who: string | null
  files: SessionFile[]
}

const p = (pid: string) => `/api/projects/${encodeURIComponent(pid)}/files/history`

export const historyApi = {
  file: (pid: string, path: string, before?: number, signal?: AbortSignal) =>
    api.get<HistoryPage>(p(pid), { path, before, limit: 200 }, signal),
  dir: (pid: string, path: string, before?: number, signal?: AbortSignal) =>
    api.get<HistoryPage>(`${p(pid)}/dir`, { path, before, limit: 200 }, signal),
  revision: (pid: string, id: number, signal?: AbortSignal) => api.get<HistoryRevision>(`${p(pid)}/revision`, { id }, signal),
  label: (pid: string, path: string, label: string) => api.post<HistoryEntry>(`${p(pid)}/label`, { path, label }),
  stats: (pid: string, signal?: AbortSignal) => api.get<HistoryStats>(`${p(pid)}/stats`, undefined, signal),
  session: (pid: string, by: string, signal?: AbortSignal) => api.get<SessionChanges>(`${p(pid)}/session`, { by }, signal),
}

export const hk = {
  all: (pid: string) => ['files', 'history', pid] as const,
  list: (pid: string, path: string, dir: boolean) => ['files', 'history', pid, dir ? 'dir' : 'file', path] as const,
  /** Revisions never change: kept apart from the lists that events invalidate. */
  revision: (pid: string, id: number) => ['files', 'history-rev', pid, id] as const,
  stats: (pid: string) => ['files', 'history', pid, 'stats'] as const,
  session: (pid: string, by: string) => ['files', 'history', pid, 'session', by] as const,
}
