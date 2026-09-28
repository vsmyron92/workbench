// REST client, types and react-query hooks for /api/workspace/** (server/src/workspace).

import { useInfiniteQuery, useQuery } from '@tanstack/react-query'
import { api } from '@/api/client'

export type ViewerKind = 'html' | 'markdown' | 'image' | 'gallery' | 'compare3d' | 'pdf' | 'video' | 'audio' | 'text' | 'file'
export type CardStatus = 'active' | 'done' | 'archived'
/** Step viewers a user can pick ('auto' = by extension). */
export const VIEWERS = ['auto', 'html', 'markdown', 'image', 'gallery', 'compare3d', 'pdf', 'video', 'audio', 'text'] as const

export interface WorkspaceStep {
  index: number
  name: string
  /** Relative to the card folder. */
  path: string
  viewer?: string
  kind: ViewerKind
  exists: boolean
  size?: number
  /** Changes when the file does. */
  mtime?: number
}

export interface WorkspaceCard {
  /** `repo:<id>` for cards from a project's own workspace/workspace.json. */
  id: string
  scope: string
  scopeName: string
  origin: 'workbench' | 'repo'
  title: string
  description: string
  icon?: string
  type: string
  category: string
  created: string
  updated?: string
  folder: string
  folderPath: string
  steps: WorkspaceStep[]
  defaultStep?: number
  defaultIndex?: number
  status: CardStatus | string
  pinned: boolean
  sample: boolean
  archived: boolean
  touchedAt: number
  /** Title, description, steps and files can change (not a repository card). */
  editable: boolean
  /** Capability URL prefix of the card folder: `/view/<grant>/<folder>/`. */
  base: string
  /** When `base` stops working (ms). Fetching the card again renews it. */
  grantExpiresAt?: number
  thumb?: string
}

export interface ScopeInfo {
  id: string
  name: string
  total: number
  active: number
  repoCards: number
  hasRepoRegistry: boolean
  error?: string
}

export interface CardList {
  scope: string
  cards: WorkspaceCard[]
  warnings: string[]
}

export interface FileEntry {
  name: string
  path: string
  dir: boolean
  size: number
  mtime: number
  kind: ViewerKind | 'dir'
}

export interface FileList {
  path: string
  entries: FileEntry[]
  /** More entries after this page. */
  truncated: boolean
  /** Entries in the folder (folders first, then files, by name). */
  total?: number
  offset?: number
}

export interface TextContent {
  path: string
  text: string
  revision: string
  size: number
  editable: boolean
}

/** workspace/trash.rs TrashItem: a deleted card, restorable until deleted for good. */
export interface TrashItem {
  /** The item's folder name in the trash. */
  id: string
  scope: string
  scopeName: string
  cardId: string | null
  title: string
  description: string
  category: string
  folder: string | null
  /** Unix ms. */
  deletedAt: number
  /** The card's files are in the trash (else its folder stayed with another card). */
  moved: boolean
  files: number
  bytes: number
  filesCapped: boolean
  restorable: boolean
  problem?: string
}

export interface CardPatch {
  title?: string
  description?: string
  category?: string
  status?: CardStatus
  pinned?: boolean
  defaultStep?: number | null
}

const e = encodeURIComponent
const cardUrl = (scope: string, id: string) => `/api/workspace/${e(scope)}/cards/${e(id)}`

export const wsApi = {
  scopes: (signal?: AbortSignal) => api.get<ScopeInfo[]>('/api/workspace/scopes', undefined, signal),
  cards: (scope: string, signal?: AbortSignal) =>
    api.get<CardList>(scope === 'all' ? '/api/workspace/cards' : `/api/workspace/${e(scope)}/cards`, undefined, signal),
  card: (scope: string, id: string, signal?: AbortSignal) => api.get<WorkspaceCard>(cardUrl(scope, id), undefined, signal),
  create: (scope: string, body: { title: string; description?: string; category?: string }) =>
    api.post<WorkspaceCard>(`/api/workspace/${e(scope)}/cards`, body),
  patch: (scope: string, id: string, body: CardPatch) => api.patch<WorkspaceCard>(cardUrl(scope, id), body),
  /** Moves the card to the Workspace trash; `trashItem` restores it. */
  remove: (scope: string, id: string) => api.del<{ ok: boolean; trashItem?: string }>(cardUrl(scope, id)),
  addStep: (scope: string, id: string, body: { name?: string; path: string; viewer?: string }) =>
    api.post<WorkspaceCard>(`${cardUrl(scope, id)}/steps`, body),
  patchStep: (scope: string, id: string, index: number, body: { name?: string; viewer?: string | null; position?: number; expectPath?: string }) =>
    api.patch<WorkspaceCard>(`${cardUrl(scope, id)}/steps/${index}`, body),
  deleteStep: (scope: string, id: string, index: number, expectPath?: string) =>
    api.del<WorkspaceCard>(`${cardUrl(scope, id)}/steps/${index}`, { path: expectPath }),
  files: (scope: string, id: string, path: string, signal?: AbortSignal, offset = 0) =>
    api.get<FileList>(`${cardUrl(scope, id)}/files`, { path, offset: offset || undefined }, signal),
  content: (scope: string, id: string, path: string, signal?: AbortSignal) =>
    api.get<TextContent>(`${cardUrl(scope, id)}/content`, { path }, signal),
  save: (scope: string, id: string, body: { path: string; text: string; revision?: string; force?: boolean }) =>
    api.put<{ path: string; revision: string; size: number }>(`${cardUrl(scope, id)}/content`, body),
  upload: (scope: string, id: string, file: Blob, name: string, opts: { dir?: string; step?: boolean } = {}) =>
    api.post<{ path: string; size: number; stepIndex: number | null; card: WorkspaceCard }>(`${cardUrl(scope, id)}/upload`, file, {
      name,
      dir: opts.dir,
      step: opts.step ? 'true' : undefined,
    }),
  grant: (scope: string, id: string) => api.post<{ base: string; expiresAt: number }>(`${cardUrl(scope, id)}/grant`),
  /** `scope` may be `all`. */
  trash: (scope: string, signal?: AbortSignal) => api.get<{ scope: string; items: TrashItem[] }>(`/api/workspace/${e(scope)}/trash`, undefined, signal),
  restore: (scope: string, item: string) => api.post<WorkspaceCard>(`/api/workspace/${e(scope)}/trash/${e(item)}/restore`),
  purge: (scope: string, item: string) => api.del<{ ok: boolean }>(`/api/workspace/${e(scope)}/trash/${e(item)}`),
  emptyTrash: (scope: string) => api.del<{ ok: boolean; removed: number }>(`/api/workspace/${e(scope)}/trash`),
}

export const wk = {
  all: ['workspace'] as const,
  scopes: ['workspace', 'scopes'] as const,
  cards: (scope: string) => ['workspace', 'cards', scope] as const,
  card: (scope: string, id: string) => ['workspace', 'card', scope, id] as const,
  files: (scope: string, id: string, path: string) => ['workspace', 'files', scope, id, path] as const,
  content: (scope: string, id: string, path: string) => ['workspace', 'content', scope, id, path] as const,
  trash: (scope: string) => ['workspace', 'trash', scope] as const,
}

export function useTrash(scope: string, enabled = true) {
  return useQuery({ queryKey: wk.trash(scope), queryFn: ({ signal }) => wsApi.trash(scope, signal), enabled, staleTime: 5_000 })
}

export function useScopes() {
  return useQuery({ queryKey: wk.scopes, queryFn: ({ signal }) => wsApi.scopes(signal), staleTime: 10_000 })
}

/**
 * Every card comes with a grant (`base`) that lasts 12 hours; the server hands out
 * the same one while it has more than an hour left. Fetching again at least every
 * 20 minutes (and on focus, e.g. after the laptop slept) keeps what is on screen
 * working: thumbnails, lazy report images, video seeks, links inside a report.
 */
export const GRANT_REFRESH_MS = 20 * 60_000

export function useCards(scope: string | null) {
  return useQuery({
    queryKey: wk.cards(scope ?? ''),
    queryFn: ({ signal }) => wsApi.cards(scope!, signal),
    enabled: !!scope,
    staleTime: 10_000,
    refetchInterval: GRANT_REFRESH_MS,
  })
}

export function useCard(scope: string, id: string) {
  return useQuery({
    queryKey: wk.card(scope, id),
    queryFn: ({ signal }) => wsApi.card(scope, id, signal),
    staleTime: GRANT_REFRESH_MS,
    refetchInterval: GRANT_REFRESH_MS,
    refetchOnWindowFocus: true,
    retry: (n, err) => n < 2 && !(err instanceof Error && 'status' in err && (err as { status: number }).status === 404),
  })
}

export function useCardFiles(scope: string, id: string, path: string, enabled = true) {
  return useQuery({
    queryKey: wk.files(scope, id, path),
    queryFn: ({ signal }) => wsApi.files(scope, id, path, signal),
    enabled,
  })
}

/** A folder page by page (galleries of more than one page load the rest on demand). */
export function useCardFilePages(scope: string, id: string, path: string) {
  return useInfiniteQuery({
    queryKey: [...wk.files(scope, id, path), 'pages'] as const,
    queryFn: ({ signal, pageParam }) => wsApi.files(scope, id, path, signal, pageParam),
    initialPageParam: 0,
    getNextPageParam: (last) => (last.truncated ? (last.offset ?? 0) + last.entries.length : undefined),
  })
}

export function useCardText(scope: string, id: string, path: string, enabled = true) {
  return useQuery({
    queryKey: wk.content(scope, id, path),
    queryFn: ({ signal }) => wsApi.content(scope, id, path, signal),
    enabled,
    retry: false,
  })
}
