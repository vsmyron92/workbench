// REST client and types of the lsp slice (server/src/lsp/routes.rs).

import type { QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'

export type ServerState = 'off' | 'starting' | 'indexing' | 'ready' | 'stopped' | 'crashed' | 'failed' | 'unavailable' | 'disabled'
export type Mode = 'auto' | 'host' | 'container'

export interface Progress {
  title: string
  message?: string
  percentage?: number
}

/** One server in `GET /api/projects/{pid}/lsp`. */
export interface ServerStatus {
  id: string
  label: string
  languages: string[]
  extensions: string[]
  /** Configured command line. */
  command: string
  preset: boolean
  /** `enabled` in config.toml. */
  enabled: boolean
  /** Turned off for this project. */
  disabledHere: boolean
  available: boolean
  /** Why it cannot run (not on PATH, rustup component missing, not in the container). */
  missing: string | null
  installHint: string
  side: 'host' | 'container' | null
  state: ServerState
  progress: Progress | null
  error: string | null
  restarts: number
  pid: number | null
  startedAt: number | null
  /** The running command (with where). */
  running: string | null
  serverInfo: { name?: string; version?: string } | null
  openDocs: number
  /** The project has its root markers, or it is in use. */
  relevant: boolean
}

export interface Counts {
  errors: number
  warnings: number
  infos: number
  hints: number
  files: number
}

export interface LspStatus {
  projectId: string
  enabled: boolean
  mode: Mode
  enabledAt: number | null
  devcontainer: { state: string; inContainer: boolean } | null
  servers: ServerStatus[]
  counts: Counts
  warnings: string[]
}

export interface LogLine {
  seq: number
  ts: number
  stream: 'stderr' | 'log' | 'message' | 'workbench'
  text: string
}

// ---------------------------------------------------------------- LSP shapes (the subset used)

export interface LspPosition {
  line: number
  character: number
}
export interface LspRange {
  start: LspPosition
  end: LspPosition
}
export interface LspLocation {
  uri: string
  range: LspRange
}
export interface LspLocationLink {
  originSelectionRange?: LspRange
  targetUri: string
  targetRange: LspRange
  targetSelectionRange: LspRange
}
export interface LspDiagnostic {
  range: LspRange
  severity?: 1 | 2 | 3 | 4
  code?: string | number
  codeDescription?: { href: string }
  source?: string
  message: string
  tags?: number[]
  relatedInformation?: { location: LspLocation; message: string }[]
  data?: unknown
  /** Added by Workbench: the server that reported it. */
  server?: string
}
export interface LspTextEdit {
  range: LspRange
  newText: string
}
export interface LspWorkspaceEdit {
  changes?: Record<string, LspTextEdit[]>
  documentChanges?: (
    | { textDocument: { uri: string; version?: number | null }; edits: (LspTextEdit | (LspTextEdit & { annotationId: string }))[] }
    | { kind: 'create' | 'rename' | 'delete'; uri?: string; oldUri?: string; newUri?: string }
  )[]
}
export interface LspCommand {
  title: string
  command: string
  arguments?: unknown[]
}
export interface LspCodeAction {
  title: string
  kind?: string
  diagnostics?: LspDiagnostic[]
  isPreferred?: boolean
  disabled?: { reason: string }
  edit?: LspWorkspaceEdit
  command?: LspCommand
  data?: unknown
}
/** A call or type hierarchy item (LSP 3.16 / 3.17). */
export interface LspHierarchyItem {
  name: string
  kind: number
  tags?: number[]
  detail?: string
  uri: string
  range: LspRange
  selectionRange: LspRange
  data?: unknown
}
export interface LspIncomingCall {
  from: LspHierarchyItem
  /** Call sites, in `from`. */
  fromRanges: LspRange[]
}
export interface LspOutgoingCall {
  to: LspHierarchyItem
  /** Call sites, in the item asked about. */
  fromRanges: LspRange[]
}

export interface LspSymbolInformation {
  name: string
  kind: number
  tags?: number[]
  containerName?: string
  location: LspLocation | { uri: string }
  /** Added by Workbench when every server was asked. */
  _server?: string
}
export interface LspDocumentSymbol {
  name: string
  detail?: string
  kind: number
  tags?: number[]
  range: LspRange
  selectionRange: LspRange
  children?: LspDocumentSymbol[]
}

export interface DiagnosticsFile {
  uri: string
  path: string
  diagnostics: LspDiagnostic[]
}

// ---------------------------------------------------------------- REST

const base = (pid: string) => `/api/projects/${encodeURIComponent(pid)}/lsp`

export const lspApi = {
  status: (pid: string, signal?: AbortSignal) => api.get<LspStatus>(base(pid), undefined, signal),
  enable: (pid: string, mode?: Mode) => api.post<LspStatus>(`${base(pid)}/enable`, mode ? { mode } : {}),
  disable: (pid: string) => api.post<LspStatus>(`${base(pid)}/disable`),
  settings: (pid: string, body: { mode?: Mode; disabledServers?: string[] }) => api.put<LspStatus>(`${base(pid)}/settings`, body),
  restart: (pid: string, sid: string) => api.post<LspStatus>(`${base(pid)}/servers/${encodeURIComponent(sid)}/restart`),
  stop: (pid: string, sid: string) => api.post<LspStatus>(`${base(pid)}/servers/${encodeURIComponent(sid)}/stop`),
  log: (pid: string, sid: string, tail = 1000) =>
    api.get<{ server: string; running: boolean; lines: LogLine[] }>(`${base(pid)}/servers/${encodeURIComponent(sid)}/log`, { tail }),
  diagnostics: (pid: string, signal?: AbortSignal) =>
    api.get<{ files: DiagnosticsFile[]; counts: Counts; truncated: boolean }>(`${base(pid)}/diagnostics`, undefined, signal),
  source: (pid: string, uri: string) => api.get<{ uri: string; path: string; name: string; content: string }>(`${base(pid)}/source`, { uri }),
}

export const lspKeys = {
  status: (pid: string) => ['lsp', 'status', pid] as const,
  diagnostics: (pid: string) => ['lsp', 'diagnostics', pid] as const,
}

let queryClient: QueryClient | null = null

/** Set by the provider: lets non-React code refresh the caches. */
export function setLspQueryClient(qc: QueryClient | null) {
  queryClient = qc
}

export function lspQueryClient(): QueryClient | null {
  return queryClient
}

/** Apply a status answer (enable, restart…) to the cache at once. */
export function putStatus(s: LspStatus) {
  queryClient?.setQueryData(lspKeys.status(s.projectId), s)
}
