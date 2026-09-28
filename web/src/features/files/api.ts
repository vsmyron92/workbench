// REST client and types for the files slice (server/src/files/**), plus the
// cross-slice git shapes it consumes (docs/ARCHITECTURE.md#cross-slice-data-contracts).

import { api } from '@/api/client'

/** files/listing.rs Entry */
export interface FileEntry {
  name: string
  path: string
  kind: 'file' | 'dir' | 'symlink'
  size: number
  mtime: number
  ignored: boolean
  hidden: boolean
  sensitive: boolean
  target?: 'file' | 'dir' | 'broken'
}

export interface Listing {
  path: string
  entries: FileEntry[]
  truncated: boolean
  total: number
}

/** files/content.rs FileContent */
export interface FileContent {
  path: string
  content: string | null
  binary: boolean
  size: number
  mtime: number
  etag: string | null
  tooLarge: boolean
  sensitive: boolean
  encoding: 'utf-8' | 'utf-8-bom' | 'unknown' | null
  mime: string
  readOnly: boolean
}

export interface FileStat {
  path: string
  exists: boolean
  kind: 'file' | 'dir' | 'other' | null
  size: number
  mtime: number
  etag: string | null
}

export interface WriteResult {
  path: string
  etag: string
  mtime: number
  size: number
}

export interface OpResult {
  ok: boolean
  path: string
  trashedWith?: 'gio' | 'trash-spec'
}

export interface UploadResult {
  path: string
  name: string
  size: number
}

export interface SearchParams {
  q: string
  regex: boolean
  case: boolean
  word: boolean
  glob: string
}

export interface SearchHit {
  path: string
  line: number
  column: number
  endColumn: number
  preview: string
  previewOffset: number
}

/** GET /api/projects/{pid}/files/todos: TODO / FIXME / XXX / HACK comments. */
export interface TodoItem {
  path: string
  line: number
  column: number
  endColumn: number
  kind: 'TODO' | 'FIXME' | 'XXX' | 'HACK'
  text: string
}
export interface TodoResult {
  items: TodoItem[]
  truncated: boolean
  filesSearched: number
  elapsedMs: number
}

export interface SearchResult {
  matches: SearchHit[]
  truncated: boolean
  timedOut: boolean
  filesSearched: number
  filesMatched: number
  sensitiveSkipped: number
  elapsedMs: number
}

export interface ReplacePreviewFile {
  path: string
  etag: string
  count: number
  lines: { line: number; before: string; after: string }[]
}

export interface ReplaceResult {
  files?: ReplacePreviewFile[]
  replaced: { path: string; count: number; etag: string }[]
  conflicts: { path: string; message: string }[]
  total: number
}

export interface FindResult {
  results: { path: string; score: number; positions: number[] }[]
  matched: number
  indexed: number
  indexTruncated: boolean
}

// ---------------------------------------------------------------- git (owned by the git slice)

type StatusCode = ' ' | 'M' | 'A' | 'D' | 'R' | 'C' | 'T' | 'U' | '?' | '!'

export interface GitStatusFile {
  path: string
  origPath?: string
  index: StatusCode
  worktree: StatusCode
  conflict: boolean
}

export interface GitStatus {
  branch: string | null
  head: string | null
  upstream: string | null
  ahead: number
  behind: number
  state: string
  stashes: number
  files: GitStatusFile[]
}

export interface GitFileDiff {
  path: string
  oldPath?: string
  original: string
  modified: string
  binary: boolean
  tooLarge: boolean
  hunks: { header: string; oldStart: number; oldLines: number; newStart: number; newLines: number }[]
  fingerprint: string
}

export interface GitBlame {
  lines: { line: number; sha: string; author: string; time: number; summary: string }[]
}

// ---------------------------------------------------------------- calls

const p = (pid: string) => `/api/projects/${encodeURIComponent(pid)}`

export const filesApi = {
  list: (pid: string, path: string, signal?: AbortSignal) => api.get<Listing>(`${p(pid)}/files/list`, { path }, signal),

  /** Project file (`pid`) or, with `pid === null`, an absolute path in an allowed root (read-only). */
  read: (pid: string | null, path: string, allowSensitive = false) =>
    pid
      ? api.get<FileContent>(`${p(pid)}/files/read`, { path, allowSensitive: allowSensitive || undefined })
      : api.get<FileContent>('/api/fs/read', { path, allowSensitive: allowSensitive || undefined }),

  stat: (pid: string | null, path: string, allowSensitive = false) =>
    pid
      ? api.get<FileStat>(`${p(pid)}/files/stat`, { path, allowSensitive: allowSensitive || undefined })
      : api.get<FileStat>('/api/fs/stat', { path, allowSensitive: allowSensitive || undefined }),

  rawUrl: (pid: string | null, path: string, opts: { download?: boolean; allowSensitive?: boolean; v?: number | string } = {}) =>
    api.url(pid ? `${p(pid)}/files/raw` : '/api/fs/raw', {
      path,
      download: opts.download || undefined,
      allowSensitive: opts.allowSensitive || undefined,
      v: opts.v,
    }),

  write: (pid: string, path: string, content: string, etag: string | null, force = false) =>
    api.put<WriteResult>(`${p(pid)}/files/write`, { path, content, etag, force }),

  op: (pid: string, op: 'mkdir' | 'create' | 'rename' | 'copy' | 'delete', path: string, to?: string) =>
    api.post<OpResult>(`${p(pid)}/files/op`, { op, path, to }),

  upload: (pid: string, dir: string, file: File) => api.post<UploadResult>(`${p(pid)}/files/upload`, file, { dir, name: file.name }),

  find: (pid: string, q: string, max = 50, signal?: AbortSignal) => api.get<FindResult>(`${p(pid)}/files/find`, { q, max }, signal),

  todos: (pid: string, signal?: AbortSignal) => api.get<TodoResult>(`${p(pid)}/files/todos`, undefined, signal),

  search: (pid: string, s: SearchParams, max: number, signal?: AbortSignal) =>
    api.get<SearchResult>(`${p(pid)}/search`, { q: s.q, regex: s.regex, case: s.case, word: s.word, glob: s.glob, max }, signal),

  replace: (
    pid: string,
    s: SearchParams & { replacement: string; paths: string[]; expected?: Record<string, string>; dryRun?: boolean },
  ) => api.post<ReplaceResult>(`${p(pid)}/search/replace`, s),

  gitStatus: (pid: string, signal?: AbortSignal) => api.get<GitStatus>(`${p(pid)}/git/status`, undefined, signal),
  gitDiff: (pid: string, path: string) => api.get<GitFileDiff>(`${p(pid)}/git/diff`, { path, mode: 'working' }),
  gitBlame: (pid: string, path: string) => api.get<GitBlame>(`${p(pid)}/git/blame`, { path }),
}
